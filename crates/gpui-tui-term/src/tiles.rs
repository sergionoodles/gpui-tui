//! Splits a frame into cell-aligned tiles, so that only the tiles that changed are sent.
//!
//! Each tile is its own kitty image, placed over a block of cells. A tile's pixels are scaled to
//! its cells, so the frame may be rendered at a lower resolution than the terminal shows.

use crate::graphics::{CellArea, PixelFormat};
use crate::presenter::Frame;

/// Rough tile edge length in frame pixels. Smaller tiles send less around a small change, larger
/// tiles cost fewer commands and compress better.
const TARGET_TILE_PIXELS: u32 = 128;
const RGBA_BYTES: usize = 4;
const OPAQUE: u8 = u8::MAX;

/// A run of cells along one axis and the frame pixels drawn over them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Span {
    pub cell_start: u16,
    pub cells: u16,
    pub pixel_start: u32,
    pub pixels: u32,
}

impl Span {
    fn pixel_range(&self) -> std::ops::Range<usize> {
        let start = self.pixel_start as usize;
        start..start + self.pixels as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Tile {
    /// Position in the layout, row by row; stable while the layout stays the same.
    pub index: usize,
    pub column: Span,
    pub row: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TileLayout {
    columns: Vec<Span>,
    rows: Vec<Span>,
}

impl TileLayout {
    /// Tiles for a `frame_width × frame_height` frame shown over `area`.
    pub(crate) fn new(frame_width: u32, frame_height: u32, area: CellArea) -> Self {
        Self {
            columns: spans(area.columns, frame_width),
            rows: spans(area.rows, frame_height),
        }
    }

    pub(crate) fn tiles(&self) -> impl Iterator<Item = Tile> + '_ {
        let columns_per_row = self.columns.len();
        self.rows
            .iter()
            .enumerate()
            .flat_map(move |(row_index, row)| {
                self.columns
                    .iter()
                    .enumerate()
                    .map(move |(column_index, column)| Tile {
                        index: row_index * columns_per_row + column_index,
                        column: *column,
                        row: *row,
                    })
            })
    }

    pub(crate) fn len(&self) -> usize {
        self.columns.len() * self.rows.len()
    }
}

/// Whether the tile's pixels differ between two frames of the same size.
pub(crate) fn tile_differs(previous: &[u8], frame: Frame<'_>, tile: &Tile) -> bool {
    tile_rows(frame, tile).any(|range| previous.get(range.clone()) != frame.pixels.get(range))
}

/// The tile's pixels, rows top to bottom. Opaque tiles drop the alpha channel to save a quarter
/// of the bytes.
pub(crate) fn tile_pixels(frame: Frame<'_>, tile: &Tile) -> (PixelFormat, Vec<u8>) {
    let rows = || tile_rows(frame, tile).filter_map(|range| frame.pixels.get(range));
    let is_opaque = rows().all(|row| {
        row.as_chunks::<RGBA_BYTES>()
            .0
            .iter()
            .all(|[_, _, _, alpha]| *alpha == OPAQUE)
    });
    if !is_opaque {
        return (PixelFormat::Rgba, rows().flatten().copied().collect());
    }
    let pixel_count = tile.column.pixels as usize * tile.row.pixels as usize;
    let mut rgb = Vec::with_capacity(pixel_count * PixelFormat::Rgb.bytes_per_pixel());
    for row in rows() {
        for [red, green, blue, _] in row.as_chunks::<RGBA_BYTES>().0 {
            rgb.extend_from_slice(&[*red, *green, *blue]);
        }
    }
    (PixelFormat::Rgb, rgb)
}

/// Byte ranges of the tile's rows within the frame.
fn tile_rows(frame: Frame<'_>, tile: &Tile) -> impl Iterator<Item = std::ops::Range<usize>> {
    let stride = frame.width as usize * RGBA_BYTES;
    let columns = tile.column.pixel_range();
    let start_in_row = columns.start * RGBA_BYTES;
    let end_in_row = columns.end * RGBA_BYTES;
    tile.row
        .pixel_range()
        .map(move |y| y * stride + start_in_row..y * stride + end_in_row)
}

/// Splits `cells` (shown with `pixels` frame pixels) into tiles of whole cells.
fn spans(cells: u16, pixels: u32) -> Vec<Span> {
    let cells = cells.max(1);
    let tile_cells = tile_cells(cells, pixels);
    (0..cells)
        .step_by(usize::from(tile_cells))
        .map(|cell_start| {
            let cell_end = cell_start.saturating_add(tile_cells).min(cells);
            let pixel_start = cell_boundary(cell_start, cells, pixels);
            Span {
                cell_start,
                cells: cell_end - cell_start,
                pixel_start,
                pixels: cell_boundary(cell_end, cells, pixels) - pixel_start,
            }
        })
        .filter(|span| span.pixels > 0)
        .collect()
}

/// The frame pixel where `cell` starts; cells are spread evenly over the frame.
fn cell_boundary(cell: u16, cells: u16, pixels: u32) -> u32 {
    let numerator = u64::from(cell) * u64::from(pixels) + u64::from(cells) / 2;
    u32::try_from(numerator / u64::from(cells)).unwrap_or(u32::MAX)
}

/// Cells per tile, close to the target size. Prefers a count whose tiles cover a whole number of
/// pixels, so the terminal scales every tile by the same factor and no seams show.
fn tile_cells(cells: u16, pixels: u32) -> u16 {
    let pixels_per_cell = (f64::from(pixels) / f64::from(cells)).max(f64::MIN_POSITIVE);
    let target = (f64::from(TARGET_TILE_PIXELS) / pixels_per_cell).round();
    let base = (target as u16).clamp(1, cells);
    let has_whole_pixels =
        |count: u16| (u64::from(count) * u64::from(pixels)).is_multiple_of(u64::from(cells));
    (base..=base.saturating_mul(2).min(cells))
        .find(|count| has_whole_pixels(*count))
        .unwrap_or(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(columns: u16, rows: u16) -> CellArea {
        CellArea { columns, rows }
    }

    /// Checks that spans cover every cell and pixel exactly once, in order.
    fn assert_covers(spans: &[Span], cells: u16, pixels: u32) {
        let mut next_cell = 0;
        let mut next_pixel = 0;
        for span in spans {
            assert_eq!((span.cell_start, span.pixel_start), (next_cell, next_pixel));
            next_cell += span.cells;
            next_pixel += span.pixels;
        }
        assert_eq!((next_cell, next_pixel), (cells, pixels));
    }

    #[test]
    fn full_resolution_tiles_follow_cell_edges() {
        // 80×24 cells of 10×20 pixels.
        let layout = TileLayout::new(800, 480, area(80, 24));
        assert_covers(&layout.columns, 80, 800);
        assert_covers(&layout.rows, 24, 480);
        assert!(
            layout
                .columns
                .iter()
                .all(|span| span.pixels == span.cells as u32 * 10)
        );
        assert_eq!(layout.columns[0].cells, 13);
        assert_eq!(layout.len(), layout.tiles().count());
    }

    #[test]
    fn half_resolution_tiles_cover_whole_pixels() {
        // Cells 9 pixels wide rendered at half size: 4.5 frame pixels per cell, so tiles need an
        // even number of cells.
        let layout = TileLayout::new(450, 180, area(100, 20));
        assert_covers(&layout.columns, 100, 450);
        let full_tiles = &layout.columns[..layout.columns.len() - 1];
        assert!(full_tiles.iter().all(|span| span.cells % 2 == 0));
        assert!(
            full_tiles
                .iter()
                .all(|span| span.pixels * 2 == span.cells as u32 * 9)
        );
    }

    #[test]
    fn tiny_frames_have_no_empty_tiles() {
        let layout = TileLayout::new(3, 1, area(10, 4));
        assert!(
            layout
                .tiles()
                .all(|tile| tile.column.pixels > 0 && tile.row.pixels > 0)
        );
        assert_eq!(
            layout.tiles().map(|tile| tile.column.pixels).sum::<u32>(),
            3
        );
    }

    #[test]
    fn only_changed_tiles_differ_and_opaque_tiles_drop_alpha() {
        let width = 300;
        let height = 2;
        let previous = vec![OPAQUE; width * height * RGBA_BYTES];
        let mut pixels = previous.clone();
        // Make the last pixel of the first row red.
        let last_pixel = (width - 1) * RGBA_BYTES;
        pixels[last_pixel..last_pixel + RGBA_BYTES].copy_from_slice(&[OPAQUE, 0, 0, OPAQUE]);
        let frame = Frame {
            pixels: &pixels,
            width: width as u32,
            height: height as u32,
        };

        let layout = TileLayout::new(frame.width, frame.height, area(30, 1));
        let changed: Vec<Tile> = layout
            .tiles()
            .filter(|tile| tile_differs(&previous, frame, tile))
            .collect();
        assert_eq!(changed.len(), 1);
        let tile = changed[0];
        assert_eq!(tile.index, layout.len() - 1);

        let (format, bytes) = tile_pixels(frame, &tile);
        assert_eq!(format, PixelFormat::Rgb);
        assert_eq!(bytes.len(), tile.column.pixels as usize * height * 3);
        assert_eq!(
            &bytes[bytes.len() / 2 - 3..bytes.len() / 2],
            &[OPAQUE, 0, 0]
        );
    }

    #[test]
    fn translucent_tiles_keep_alpha() {
        let pixels = [1, 2, 3, 128];
        let frame = Frame {
            pixels: &pixels,
            width: 1,
            height: 1,
        };
        let layout = TileLayout::new(1, 1, area(1, 1));
        let tile = layout.tiles().next().expect("one tile");
        assert_eq!(
            tile_pixels(frame, &tile),
            (PixelFormat::Rgba, pixels.to_vec())
        );
    }
}
