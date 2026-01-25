use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageBuffer, ImageEncoder, Rgba};
use rand::Rng;
use rand_chacha::ChaCha8Rng;
use rand::SeedableRng;

pub fn generate_code(message: &str) -> Vec<u8> {
    // Return the input string as raw bytes for now.
    message.as_bytes().to_vec()
}

/// Generates a pseudo-random PNG image seeded by the payload text.
pub fn generate_image(payload: &str) -> Vec<u8> {
    let mut hasher = DefaultHasher::new();
    payload.hash(&mut hasher);
    let seed = hasher.finish();

    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let (width, height) = (128u32, 128u32);
    let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(width, height);

    for pixel in img.pixels_mut() {
        let r: u8 = rng.gen_range(0..=255);
        let g: u8 = rng.gen_range(0..=255);
        let b: u8 = rng.gen_range(0..=255);
        *pixel = Rgba([r, g, b, 255]);
    }

    let mut png_bytes = Vec::new();
    let mut encoder = PngEncoder::new(&mut png_bytes);
    if encoder.write_image(
        img.as_raw(),
        width,
        height,
        ExtendedColorType::Rgba8,
    ).is_err() {
        return Vec::new();
    }

    png_bytes
}
