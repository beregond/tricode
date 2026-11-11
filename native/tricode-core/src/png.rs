//! PNG encoding shared by the generator and the decoder's debug image.

use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, RgbaImage};

/// Encodes an RGBA image as PNG.
pub(crate) fn encode_png(img: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            ExtendedColorType::Rgba8,
        )
        .map_err(|e| format!("Failed to encode PNG: {}", e))?;
    Ok(png)
}
