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
pub fn generate_image(payload: &str, width_ratio: u32, height_ratio: u32) -> Vec<u8> {
    let mut hasher = DefaultHasher::new();
    payload.hash(&mut hasher);
    let seed = hasher.finish();

    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    
    // Calculate dimensions based on ratio, keeping a reasonable size
    let base_size = 128u32;
    let gcd = gcd(width_ratio, height_ratio);
    let normalized_width = width_ratio / gcd;
    let normalized_height = height_ratio / gcd;
    
    // Scale to get reasonable image size
    let scale = base_size / normalized_width.max(normalized_height);
    let width = normalized_width * scale.max(1);
    let height = normalized_height * scale.max(1);
    
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

/// Calculate greatest common divisor for ratio normalization
fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let temp = b;
        b = a % b;
        a = temp;
    }
    a
}
