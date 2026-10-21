//! The picture the terminal shows: placed images composited over the cell grid.

use image::RgbaImage;

use crate::kitty::DecodedImage;

/// The latest frame, as the terminal would display it.
pub struct Screen {
    pub image: RgbaImage,
    /// Size of the drawn area in cells.
    pub columns: u32,
    pub rows: u32,
}

impl Screen {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        self.image.get_pixel(x, y).0
    }
}

/// Where an image is placed: the cursor cell and the cell size, in pixels.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Placement {
    pub column: u32,
    pub row: u32,
    pub cell_width: u32,
    pub cell_height: u32,
}

/// Draws `decoded` onto `screen` at `placement`, scaled to its cell area (nearest neighbour)
/// like kitty's `c=`/`r=` keys. The screen grows to hold it.
pub(crate) fn composite(
    screen: Option<Screen>,
    decoded: &DecodedImage,
    placement: Placement,
) -> Screen {
    let source = &decoded.image;
    let target_width = match decoded.columns {
        0 => source.width(),
        columns => columns * placement.cell_width,
    };
    let target_height = match decoded.rows {
        0 => source.height(),
        rows => rows * placement.cell_height,
    };
    let left = placement.column * placement.cell_width;
    let top = placement.row * placement.cell_height;

    let mut image = grown(
        screen.map(|screen| screen.image),
        left + target_width,
        top + target_height,
    );
    for y in 0..target_height {
        let source_y = y * source.height() / target_height;
        for x in 0..target_width {
            let source_x = x * source.width() / target_width;
            image.put_pixel(left + x, top + y, *source.get_pixel(source_x, source_y));
        }
    }
    Screen {
        columns: image.width() / placement.cell_width.max(1),
        rows: image.height() / placement.cell_height.max(1),
        image,
    }
}

/// `image` enlarged to at least `width × height`, keeping its pixels.
fn grown(image: Option<RgbaImage>, width: u32, height: u32) -> RgbaImage {
    let Some(image) = image else {
        return RgbaImage::new(width, height);
    };
    if image.width() >= width && image.height() >= height {
        return image;
    }
    let mut larger = RgbaImage::new(width.max(image.width()), height.max(image.height()));
    for (x, y, pixel) in image.enumerate_pixels() {
        larger.put_pixel(x, y, *pixel);
    }
    larger
}
