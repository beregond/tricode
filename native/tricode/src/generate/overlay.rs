//! Images drawn over the centre of a code: the cells under them are left out, and the error
//! correction repairs them.

use super::geometry::TriangleGrid;
use crate::frame::EccLevel;
use image::{ImageFormat, ImageReader, Rgba, RgbaImage};
use std::io::Cursor;

/// An image drawn over the centre of a code, such as a logo or a line of text. The triangles under
/// it are left out, so the decoder reads them as damage that the error correction repairs.
#[derive(Debug, Clone, Copy)]
pub struct Overlay<'a> {
    /// PNG bytes of the image; its transparent parts show the background.
    pub png: &'a [u8],
    /// Share of the code's area the overlay's box takes, above 0 and at most
    /// [`EccLevel::max_overlay_share`] of the code's level.
    pub share: f64,
}

/// Largest overlay image accepted, in pixels; larger ones should be scaled down before sending.
pub const MAX_OVERLAY_PIXELS: u64 = 16_000_000;

/// Decodes an overlay's image and checks its share against what `level` repairs.
pub(crate) fn load_overlay(
    overlay: Overlay<'_>,
    level: EccLevel,
) -> Result<(RgbaImage, f64), String> {
    let max_share = level.max_overlay_share();
    if !(overlay.share > 0.0 && overlay.share <= max_share) {
        return Err(format!(
            "Overlay must take between 0% and {:.0}% of a {} level code, got {:.1}%",
            max_share * 100.0,
            level.name(),
            overlay.share * 100.0
        ));
    }
    let reader = ImageReader::with_format(Cursor::new(overlay.png), ImageFormat::Png);
    let (w, h) = reader
        .into_dimensions()
        .map_err(|e| format!("Failed to read overlay image: {}", e))?;
    if w as u64 * h as u64 > MAX_OVERLAY_PIXELS {
        return Err(format!(
            "Overlay image of {}×{} px is larger than {} MP",
            w,
            h,
            MAX_OVERLAY_PIXELS / 1_000_000
        ));
    }
    let image = image::load_from_memory_with_format(overlay.png, ImageFormat::Png)
        .map_err(|e| format!("Failed to read overlay image: {}", e))?
        .to_rgba8();
    Ok((image, overlay.share))
}

/// A rectangle in pixels: left, top, right and bottom edges.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PixelBox {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

/// Largest share of the code's width or height the overlay box spans, so a very wide or tall
/// overlay can't reach the corner anchors.
const MAX_OVERLAY_SPAN: f64 = 0.8;

/// The box an overlay is drawn in: centred, `share` of the code's area, with the image's aspect
/// ratio (narrowed to [`MAX_OVERLAY_SPAN`] of the code when it is very wide or tall).
pub(crate) fn overlay_box(image: &RgbaImage, share: f64, width: u32, height: u32) -> PixelBox {
    let (width, height) = (width as f64, height as f64);
    let aspect = image.width().max(1) as f64 / image.height().max(1) as f64;
    let area = share * width * height;
    let box_width = (area * aspect).sqrt().min(width * MAX_OVERLAY_SPAN);
    let box_height = (area / aspect).sqrt().min(height * MAX_OVERLAY_SPAN);
    PixelBox {
        left: (width - box_width) / 2.0,
        top: (height - box_height) / 2.0,
        right: (width + box_width) / 2.0,
        bottom: (height + box_height) / 2.0,
    }
}

/// Leaves out every cell whose triangle reaches into the box, so whole triangles disappear rather
/// than being cut, and borders run around the gap like around any other background.
pub(crate) fn clear_cells_under(
    shapes: &mut [Vec<Option<usize>>],
    grid: &TriangleGrid,
    area: &PixelBox,
) {
    for (row, cells) in shapes.iter_mut().enumerate() {
        for (col, shape) in cells.iter_mut().enumerate() {
            let corners = grid.vertices(row as i64, col as i64);
            let (xs, ys) = (corners.map(|c| c.0), corners.map(|c| c.1));
            let min = |v: [f64; 3]| v.into_iter().fold(f64::INFINITY, f64::min);
            let max = |v: [f64; 3]| v.into_iter().fold(f64::NEG_INFINITY, f64::max);
            if min(xs) < area.right
                && max(xs) > area.left
                && min(ys) < area.bottom
                && max(ys) > area.top
            {
                *shape = None;
            }
        }
    }
}

/// Gap between the overlay's image and the edge of its box, as a share of the triangle width.
const OVERLAY_MARGIN: f64 = 0.15;

/// Draws the overlay's image scaled to fit its box (keeping its aspect ratio), composited over the
/// code with its alpha.
pub(crate) fn draw_overlay(
    img: &mut RgbaImage,
    image: &RgbaImage,
    area: &PixelBox,
    triangle_width: u32,
) {
    let margin = OVERLAY_MARGIN * triangle_width as f64;
    let (room_w, room_h) = (
        (area.right - area.left - 2.0 * margin).max(1.0),
        (area.bottom - area.top - 2.0 * margin).max(1.0),
    );
    let scale = (room_w / image.width().max(1) as f64).min(room_h / image.height().max(1) as f64);
    let (w, h) = (
        ((image.width() as f64 * scale).round() as u32).max(1),
        ((image.height() as f64 * scale).round() as u32).max(1),
    );
    let scaled = image::imageops::resize(image, w, h, image::imageops::FilterType::Lanczos3);
    let x0 = ((area.left + area.right - w as f64) / 2.0).round() as u32;
    let y0 = ((area.top + area.bottom - h as f64) / 2.0).round() as u32;
    for (x, y, src) in scaled.enumerate_pixels() {
        let (px, py) = (x0 + x, y0 + y);
        if px < img.width() && py < img.height() {
            let dst = img.get_pixel_mut(px, py);
            *dst = over(*src, *dst);
        }
    }
}

/// Porter-Duff "over": `src` composited on top of `dst`, both with straight alpha.
fn over(src: Rgba<u8>, dst: Rgba<u8>) -> Rgba<u8> {
    let (sa, da) = (src[3] as f64 / 255.0, dst[3] as f64 / 255.0);
    let out_a = sa + da * (1.0 - sa);
    if out_a == 0.0 {
        return Rgba([0, 0, 0, 0]);
    }
    let channel =
        |i: usize| ((src[i] as f64 * sa + dst[i] as f64 * da * (1.0 - sa)) / out_a).round() as u8;
    Rgba([
        channel(0),
        channel(1),
        channel(2),
        (out_a * 255.0).round() as u8,
    ])
}
