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
    const PIXELS_PER_BIT: u32 = 4; // Each bit is a 4x4 square
    
    // Calculate dimensions maintaining the aspect ratio and ensuring enough bit squares
    let gcd_val = gcd(width_ratio, height_ratio);
    let mut normalized_width = width_ratio / gcd_val;
    let mut normalized_height = height_ratio / gcd_val;
    
    // Ensure normalized_width is odd
    if normalized_width % 2 == 0 {
        normalized_width += 1;
    }
    
    // Ensure normalized_height is a multiple of 4
    if normalized_height % 4 != 0 {
        normalized_height = ((normalized_height / 4) + 1) * 4;
    }
    
    // Calculate total bits needed
    let total_bits = encoded_data.len() * 8;
    let bits_with_offset = total_bits + (normalized_height / 2) as usize;
    
    // Calculate scaling factor: k^2 * normalized_width * normalized_height >= bits_with_offset 
    let k = ((bits_with_offset as f64) / (normalized_width as f64 * normalized_height as f64))
        .sqrt()
        .ceil() as u32;
    
    // Dimensions in bit squares
    let width_squares = normalized_width * k.max(1);
    let height_squares = normalized_height * k.max(1);
    
    // Actual pixel dimensions
    let width = width_squares * PIXELS_PER_BIT;
    let height = height_squares * PIXELS_PER_BIT;
    
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
            let color = if bit == 0 { 255 } else { 0 };
            let rgba = Rgba([color, color, color, 255]);
            
            // Fill the 4x4 square
            for dy in 0..PIXELS_PER_BIT {
                for dx in 0..PIXELS_PER_BIT {
                    let px = square_x * PIXELS_PER_BIT + dx;
                    let py = square_y * PIXELS_PER_BIT + dy;
                    img.put_pixel(px, py, rgba);
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