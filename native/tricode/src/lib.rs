use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageBuffer, ImageEncoder, Rgba};
use rand::Rng;
use rand_chacha::ChaCha8Rng;
use rand::SeedableRng;
use reed_solomon_erasure::galois_8::ReedSolomon;

pub fn generate_code(message: &str) -> Vec<u8> {
    // Return the input string as raw bytes for now.
    message.as_bytes().to_vec()
}

/// Generates a PNG image encoding the payload text.
/// The payload is first encoded with Reed-Solomon error correction,
/// then the binary data is visualized as pixels: 0 bits = white, 1 bits = black.
pub fn generate_image(payload: &str, width_ratio: u32, height_ratio: u32) -> Vec<u8> {
    // Convert payload to bytes and apply Reed-Solomon encoding
    let payload_bytes = payload.as_bytes().to_vec();
    let encoded_data = match reed_solomon_encode(payload_bytes) {
        Ok(data) => data,
        Err(_) => return Vec::new(), // Return empty on encoding error
    };

    create_image_from_binary_data(&encoded_data, width_ratio, height_ratio)
}

/// Creates a PNG image from binary data maintaining the specified aspect ratio.
/// Binary data is encoded as: 0 bits = white pixels, 1 bits = black pixels.
/// Each bit is represented by a 4x4 square of pixels.
fn create_image_from_binary_data(encoded_data: &[u8], width_ratio: u32, height_ratio: u32) -> Vec<u8> {
    const PIXELS_PER_BIT_WIDTH: u32 = 21; // Bit box width in pixels
    let pixels_per_bit_height = ((PIXELS_PER_BIT_WIDTH as f64) * (3.0_f64.sqrt() / 2.0)).ceil() as u32;

    // Calculate total bits needed
    let total_bits = encoded_data.len() * 8;
    
    // Calculate dimensions maintaining the aspect ratio and ensuring enough bit squares
    let gcd_val = gcd(width_ratio, height_ratio);
    let mut normalized_width = width_ratio / gcd_val;
    let mut normalized_height = height_ratio / gcd_val;

    // Find optimal integer dimensions using the previously written function
    let (mut width_squares, mut height_squares) =
        find_optimal_factors(total_bits as u32, normalized_width, normalized_height);

    // Ensure width is odd and height is a multiple of 2 (round up if necessary)
    if width_squares % 2 == 0 {
        width_squares += 1;
    }
    if height_squares % 2 != 0 {
        height_squares += 1;
    }
    
    // Actual pixel dimensions
    let width = width_squares * PIXELS_PER_BIT_WIDTH;
    let height = height_squares * pixels_per_bit_height;
    
    let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(width, height);

    // Fill image with encoded data bits (each bit as a 4x4 square)
    let mut bit_index = 0;
    for square_y in 0..height_squares {
        for square_x in 0..width_squares {
            let bit = if bit_index < total_bits {
                let byte_index = bit_index / 8;
                let bit_offset = 7 - (bit_index % 8); // MSB first
                let bit_value = (encoded_data[byte_index] >> bit_offset) & 1;
                bit_index += 1;
                bit_value
            } else {
                0 // Fill remaining squares with 0 (white)
            };
            
            // 0 = white, 1 = black
            let (color, border) = if bit == 0 { (255, 0) } else { (0, 255) };
            let rgba = Rgba([color, color, color, 255]);
            
            // Fill the equilateral triangle with border
            let mut pointing_down = square_x % 2 == 0;
            if square_y % 2 == 1 {
                pointing_down = !pointing_down;
            }

            let w = PIXELS_PER_BIT_WIDTH as f64;
            let h = pixels_per_bit_height as f64;
            let center_x = (w - 1.0) / 2.0;
            let denom = (h - 1.0).max(1.0);
            let border_color = Rgba([border, border, border, 255]);

            for dy in 0..pixels_per_bit_height {
                for dx in 0..PIXELS_PER_BIT_WIDTH {
                    let y = dy as f64;
                    let x = dx as f64;
                    let t = y / denom;

                    let half_width = if pointing_down {
                        (w / 2.0) * (1.0 - t)
                    } else {
                        (w / 2.0) * t
                    };

                    if (x - center_x).abs() <= half_width {
                        let px = square_x * PIXELS_PER_BIT_WIDTH + dx;
                        let py = square_y * pixels_per_bit_height + dy;

                        // Check if this pixel is on a border
                        // TODO: maybe simpler?
                        let is_base = if pointing_down { dy == 0 } else { dy == pixels_per_bit_height - 1 };
                        let is_tip = if pointing_down { dy == pixels_per_bit_height - 1 } else { dy == 0 };
                        let is_left_edge = (x - center_x).abs() >= half_width - 0.5;
                        let is_right_edge = (x - center_x).abs() >= half_width - 1.5;

                        let use_border = is_base || is_tip || is_left_edge || is_right_edge;
                        let pixel_color = if use_border { border_color } else { rgba };
                        img.put_pixel(px, py, pixel_color);
                    }
                }
            }
        }
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

/// Finds two factors A and B such that:
/// - A * B >= C
/// - The ratio A/B is as close as possible to X/Y
/// - A * B is minimized while satisfying the above constraints
/// 
/// Returns (A, B) where A and B are positive integers.
/// TODO: Double check this math
pub fn find_optimal_factors(c: u32, x: u32, y: u32) -> (u32, u32) {
    if c == 0 || x == 0 || y == 0 {
        return (1, c.max(1));
    }
    
    // Ideal ratio r = X/Y
    // We want A/B ≈ X/Y, so A ≈ B * X/Y
    // With A * B >= C, we get: B * (B * X/Y) >= C
    // Therefore: B >= sqrt(C * Y/X)
    
    let ratio = (x as f64) / (y as f64);
    let b_ideal = ((c as f64 * y as f64) / (x as f64)).sqrt();
    
    // Start from the ideal B and search for the best integer solution
    let b_start = b_ideal.floor() as u32;
    let b_start = b_start.max(1);
    
    let mut best_a = 0;
    let mut best_b = 0;
    let mut best_error = f64::INFINITY;
    let mut best_product = u32::MAX;
    
    // Search in a range around the ideal value
    for b in b_start..=(b_start + 100).max(b_start * 2) {
        // Calculate minimum A such that A * B >= C
        let a = ((c as f64) / (b as f64)).ceil() as u32;
        let a = a.max(1);
        
        let product = a * b;
        if product < c {
            continue; // Doesn't satisfy constraint
        }
        
        // Calculate how close ((A + 1) * 0.5) / B is to X/Y, with B adjusted by sqrt(3)/2
        let a_adjusted = (a as f64 + 1.0) * 0.5;
        let b_adjusted = (b as f64) * (3.0_f64.sqrt() / 2.0);
        let current_ratio = a_adjusted / b_adjusted;
        let ratio_error = (current_ratio - ratio).abs();
        
        // Prefer solutions with smaller error, then smaller product
        if ratio_error < best_error || (ratio_error == best_error && product < best_product) {
            best_error = ratio_error;
            best_product = product;
            best_a = a;
            best_b = b;
        }
        
        // Early exit if we found exact match
        if ratio_error < 0.001 && product >= c {
            break;
        }
    }
    
    // If no solution found in forward search, try backward
    if best_a == 0 {
        for b in (1..=b_start).rev() {
            let a = ((c as f64) / (b as f64)).ceil() as u32;
            let a = a.max(1);
            
            let product = a * b;
            if product < c {
                continue;
            }
            
            let a_adjusted = (a as f64 + 1.0) * 0.5;
            let b_adjusted = (b as f64) * (3.0_f64.sqrt() / 2.0);
            let current_ratio = a_adjusted / b_adjusted;
            let ratio_error = (current_ratio - ratio).abs();
            
            if ratio_error < best_error || (ratio_error == best_error && product < best_product) {
                best_error = ratio_error;
                best_product = product;
                best_a = a;
                best_b = b;
            }
        }
    }
    
    // Fallback: if still no solution, return a valid one
    if best_a == 0 {
        best_a = c;
        best_b = 1;
    }
    
    (best_a, best_b)
}

/// Applies Reed-Solomon error correction codes to the input data.
/// The function adds parity shards (10 by default) to enable error correction.
/// Returns the encoded data with both data and parity shards concatenated.
pub fn reed_solomon_encode(data: Vec<u8>) -> Result<Vec<u8>, String> {
    if data.is_empty() {
        return Err("Input data is empty".to_string());
    }

    // Number of data shards and parity shards
    let data_shard_count = 10;
    let parity_shard_count = 10;
    
    // Create Reed-Solomon encoder
    let rs = ReedSolomon::new(data_shard_count, parity_shard_count)
        .map_err(|e| format!("Failed to create Reed-Solomon encoder: {:?}", e))?;
    
    // Calculate shard size (round up to accommodate all data)
    let shard_size = (data.len() + data_shard_count - 1) / data_shard_count;
    
    // Create shards: split data into data_shard_count shards
    let mut shards = Vec::new();
    for i in 0..data_shard_count {
        let start = i * shard_size;
        let end = ((i + 1) * shard_size).min(data.len());
        
        let mut shard = if start < data.len() {
            data[start..end].to_vec()
        } else {
            Vec::new()
        };
        
        // Pad to shard_size
        shard.resize(shard_size, 0);
        shards.push(shard);
    }
    
    // Add empty parity shards
    for _ in 0..parity_shard_count {
        shards.push(vec![0u8; shard_size]);
    }
    
    // Encode to generate parity shards
    rs.encode(&mut shards)
        .map_err(|e| format!("Failed to encode: {:?}", e))?;
    
    // Prepare output: original data length (4 bytes) + shard size (4 bytes) + all shards
    let mut result = Vec::new();
    result.extend_from_slice(&(data.len() as u32).to_le_bytes());
    result.extend_from_slice(&(shard_size as u32).to_le_bytes());
    
    // Concatenate all shards
    for shard in shards {
        result.extend_from_slice(&shard);
    }
    
    Ok(result)
}

/// Decodes and corrects data that was encoded with Reed-Solomon error correction.
/// Can recover from up to parity_shard_count corrupted or missing shards.
/// Returns the original data.
pub fn reed_solomon_decode(encoded_data: Vec<u8>) -> Result<Vec<u8>, String> {
    if encoded_data.len() < 8 {
        return Err("Encoded data is too short".to_string());
    }
    
    // Read metadata
    let original_len = u32::from_le_bytes([
        encoded_data[0],
        encoded_data[1],
        encoded_data[2],
        encoded_data[3],
    ]) as usize;
    
    let shard_size = u32::from_le_bytes([
        encoded_data[4],
        encoded_data[5],
        encoded_data[6],
        encoded_data[7],
    ]) as usize;
    
    let data_shard_count = 10;
    let parity_shard_count = 10;
    let total_shards = data_shard_count + parity_shard_count;
    
    // Validate encoded data length
    let expected_len = 8 + (total_shards * shard_size);
    if encoded_data.len() != expected_len {
        return Err(format!(
            "Invalid encoded data length: expected {}, got {}",
            expected_len,
            encoded_data.len()
        ));
    }
    
    // Create Reed-Solomon decoder
    let rs = ReedSolomon::new(data_shard_count, parity_shard_count)
        .map_err(|e| format!("Failed to create Reed-Solomon decoder: {:?}", e))?;
    
    // Extract shards
    let mut shards = Vec::new();
    let mut offset = 8;
    for _ in 0..total_shards {
        let shard = encoded_data[offset..offset + shard_size].to_vec();
        shards.push(Some(shard));
        offset += shard_size;
    }
    
    // Verify and reconstruct if needed
    rs.reconstruct(&mut shards)
        .map_err(|e| format!("Failed to reconstruct: {:?}", e))?;
    
    // Extract original data from data shards
    let mut result = Vec::new();
    for i in 0..data_shard_count {
        if let Some(ref shard) = shards[i] {
            result.extend_from_slice(shard);
        }
    }
    
    // Trim to original length
    result.truncate(original_len);
    
    Ok(result)
}