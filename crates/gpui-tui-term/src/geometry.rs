//! Terminal grid size in cells and pixels.

use crate::graphics::CellArea;

/// Cell size used when the terminal reports no pixel sizes at all.
const FALLBACK_CELL_SIZE: (u16, u16) = (8, 16);

/// The terminal's text area: a grid of cells, each `cell_width × cell_height` pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub columns: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

impl Geometry {
    /// Derives the cell size from the text-area pixel size, falling back to a reported cell size
    /// (`CSI 16 t`) and finally a guess. `width_px`/`height_px` may be zero when unknown.
    pub fn new(
        columns: u16,
        rows: u16,
        width_px: u16,
        height_px: u16,
        reported_cell_size: Option<(u16, u16)>,
    ) -> Self {
        let columns = columns.max(1);
        let rows = rows.max(1);
        let measured = (width_px > 0 && height_px > 0)
            .then(|| (width_px / columns, height_px / rows))
            .filter(|(width, height)| *width > 0 && *height > 0);
        let (cell_width, cell_height) = measured
            .or(reported_cell_size)
            .unwrap_or(FALLBACK_CELL_SIZE);

        Self {
            columns,
            rows,
            cell_width,
            cell_height,
        }
    }

    /// Width of the whole grid in pixels. Excludes any padding the terminal draws around it.
    pub fn pixel_width(&self) -> u32 {
        u32::from(self.columns) * u32::from(self.cell_width)
    }

    pub fn pixel_height(&self) -> u32 {
        u32::from(self.rows) * u32::from(self.cell_height)
    }

    pub fn cell_area(&self) -> CellArea {
        CellArea {
            columns: self.columns,
            rows: self.rows,
        }
    }

    /// Pixel position of the centre of a zero-based cell.
    pub fn cell_center(&self, column: u32, row: u32) -> (f32, f32) {
        let x = (column as f32 + 0.5) * f32::from(self.cell_width);
        let y = (row as f32 + 0.5) * f32::from(self.cell_height);
        (x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_size_from_pixel_size_ignores_padding_remainder() {
        let geometry = Geometry::new(80, 24, 805, 410, None);
        assert_eq!((geometry.cell_width, geometry.cell_height), (10, 17));
        assert_eq!(geometry.pixel_width(), 800);
        assert_eq!(geometry.pixel_height(), 408);
    }

    #[test]
    fn falls_back_to_reported_cell_size_then_guess() {
        let reported = Geometry::new(80, 24, 0, 0, Some((9, 18)));
        assert_eq!((reported.cell_width, reported.cell_height), (9, 18));

        let guessed = Geometry::new(80, 24, 0, 0, None);
        assert_eq!(
            (guessed.cell_width, guessed.cell_height),
            FALLBACK_CELL_SIZE
        );
    }

    #[test]
    fn cell_center() {
        let geometry = Geometry::new(10, 10, 100, 200, None);
        assert_eq!(geometry.cell_center(0, 1), (5.0, 30.0));
    }
}
