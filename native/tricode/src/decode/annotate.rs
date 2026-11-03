//! The debug image: detected triangles outlined over the input.

use super::detect::{Anchors, Triangle};
use image::{Rgba, RgbaImage};

/// Outline thickness of a detected triangle's box, in pixels; anchors get twice this.
const BOX_THICKNESS: u32 = 2;

/// Draw bounding-box rectangles on the annotated image.
/// All detected triangles are drawn in blue; the four anchors are drawn in red on top.
pub(crate) fn draw_rectangles(img: &mut RgbaImage, triangles: &[Triangle], anchors: &Anchors) {
    let blue = Rgba([0, 0, 255, 255]);
    let red = Rgba([255, 0, 0, 255]);
    for t in triangles {
        draw_box(img, t, blue, BOX_THICKNESS);
    }
    for t in anchors.all() {
        draw_box(img, t, red, BOX_THICKNESS * 2);
    }
}

/// Draws the outline of a triangle's bounding box, `thickness` pixels wide on its inner side.
fn draw_box(img: &mut RgbaImage, t: &Triangle, color: Rgba<u8>, thickness: u32) {
    let (width, height) = img.dimensions();
    let min_x = t.min_x.min(width.saturating_sub(1));
    let min_y = t.min_y.min(height.saturating_sub(1));
    let max_x = t.max_x.min(width);
    let max_y = t.max_y.min(height);

    for d in 0..thickness {
        for x in min_x..max_x {
            img.put_pixel(x, (min_y + d).min(height - 1), color);
            img.put_pixel(x, max_y.saturating_sub(1 + d), color);
        }
        for y in min_y..max_y {
            img.put_pixel((min_x + d).min(width - 1), y, color);
            img.put_pixel(max_x.saturating_sub(1 + d), y, color);
        }
    }
}
