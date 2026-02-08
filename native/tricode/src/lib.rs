use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageBuffer, ImageEncoder, Rgba, GrayImage};
use imageproc::edges::canny;
use imageproc::contrast::{threshold, ThresholdType};
use reed_solomon_erasure::galois_8::ReedSolomon;
use serde::{Deserialize, Serialize};

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
    const PIXELS_PER_BIT_WIDTH: u32 = 51; // Bit box width in pixels
    let pixels_per_bit_height = ((PIXELS_PER_BIT_WIDTH as f64) * (3.0_f64.sqrt() / 2.0)).ceil() as u32;

    // Calculate total bits needed, we add 8 bits for border marks
    let total_bits = (encoded_data.len() + 1 ) * 8;
    
    // Calculate dimensions maintaining the aspect ratio and ensuring enough bit squares
    let gcd_val = gcd(width_ratio, height_ratio);
    let mut normalized_width = width_ratio / gcd_val;
    let mut normalized_height = height_ratio / gcd_val;

    // Find optimal integer dimensions using the previously written function
    let (mut width_squares, mut height_squares) =
        find_optimal_factors(total_bits as u32, normalized_width, normalized_height);

    // Calculate padding
    let padding_x = PIXELS_PER_BIT_WIDTH / 2;
    let padding_y = pixels_per_bit_height / 2;
    
    // Actual pixel dimensions (including padding)
    let width_without_padding = ((width_squares + 1) / 2) * PIXELS_PER_BIT_WIDTH;
    let height_without_padding = height_squares * pixels_per_bit_height;
    let width = width_without_padding + (padding_x * 2);
    let height = height_without_padding + (padding_y * 2);
    
    let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(width, height);
    
    // Fill with white background
    for pixel in img.pixels_mut() {
        *pixel = Rgba([255, 255, 255, 255]);
    }

    // Fill image with encoded data bits (each bit as a 4x4 square)
    let mut bit_index = 0;
    for square_y in 0..height_squares {
        for square_x in 0..width_squares {
            // Check if this is an orientation mark position
            let is_first_row = square_y == 0;
            let is_last_row = square_y == height_squares - 1;
            let is_first_col = square_x == 0;
            let is_second_col = square_x == 1;
            let is_before_last_col = square_x == width_squares.saturating_sub(2);
            let is_last_col = square_x == width_squares - 1;
            
            let bit = if (is_first_row || is_last_row) && 
                         (is_first_col || is_last_col) {
                // Orientation marks: 1st and last positions are always 1 (black)
                1
            } else if (is_first_row || is_last_row) && 
                      (is_second_col || is_before_last_col) {
                // Orientation marks: 2nd and before-last positions are always 0 (white)
                0
            } else {
                // Normal data bit
                let byte_index = bit_index / 8;
                if byte_index < encoded_data.len() {
                    let bit_offset = 7 - (bit_index % 8); // MSB first
                    let bit_value = (encoded_data[byte_index] >> bit_offset) & 1;
                    bit_index += 1;
                    bit_value
                } else {
                    0 // Fill remaining squares with 0 (white)
                }
            };
            
            // 0 = white, 1 = black
            let (color, border) = if bit == 0 { (255, 255) } else { (0, 255) };
            let rgba = Rgba([color, color, color, 255]);
            

            let w = PIXELS_PER_BIT_WIDTH as f64;
            let h = pixels_per_bit_height as f64;
            let center_x = (w - 1.0) / 2.0;
            let denom = (h - 1.0).max(1.0);
            let _border_color = Rgba([border, border, border, 255]);

            if bit == 1 {
                // Fill the equilateral triangle with border
                let mut pointing_down = square_x % 2 == 0;
                if square_y % 2 == 1 {
                    pointing_down = !pointing_down;
                }

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
                            let base_x = square_x * (PIXELS_PER_BIT_WIDTH / 2);
                            let px = padding_x + base_x + dx;
                            let py = padding_y + square_y * pixels_per_bit_height + dy;

                            // Check if this pixel is on a border
                            // TODO: maybe simpler?
                            let is_base = if pointing_down { dy == 0 } else { dy == pixels_per_bit_height - 1 };
                            let is_tip = if pointing_down { dy == pixels_per_bit_height - 1 } else { dy == 0 };
                            let is_left_edge = (x - center_x).abs() >= half_width - 0.5;
                            let is_right_edge = (x - center_x).abs() >= half_width - 1.5;

                            let is_border = is_base || is_tip || is_left_edge || is_right_edge;
                            if ! is_border {
                                img.put_pixel(px, py, rgba);
                            }
                        }
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
/// - A must be odd
/// - B must be a multiple of 2 (even)
/// - The ratio ((A+1)/2) / (B * sqrt(3)/2) is as close as possible to X/Y
/// - A * B is minimized while satisfying the above constraints
/// 
/// Returns (A, B) where A and B are positive integers.
pub fn find_optimal_factors(c: u32, x: u32, y: u32) -> (u32, u32) {
    if c == 0 || x == 0 || y == 0 {
        return (1, c.max(1));
    }
    
    // Target ratio: X/Y
    // We want: ((A+1)/2) / (B * sqrt(3)/2) ≈ X/Y
    // Simplifying: (A+1) / (B * sqrt(3)) ≈ X/Y
    // Therefore: A ≈ B * sqrt(3) * X/Y - 1
    // 
    // With constraint A * B >= C:
    // (B * sqrt(3) * X/Y - 1) * B >= C (approximately)
    // B^2 * sqrt(3) * X/Y >= C
    // B >= sqrt(C * Y / (sqrt(3) * X))
    
    let target_ratio = (x as f64) / (y as f64);
    let sqrt3 = 3.0_f64.sqrt();
    
    // Calculate ideal starting point for B (must be even)
    let b_ideal = ((c as f64 * y as f64) / (sqrt3 * x as f64)).sqrt();
    let mut b_start = b_ideal.floor().max(2.0) as u32;
    // Ensure b_start is even
    if b_start % 2 != 0 {
        b_start += 1;
    }
    
    let mut best_solution: Option<(u32, u32, f64, u32)> = None; // (A, B, error, product)
    
    // Search for the best solution in a range around the ideal B
    // Only consider even values for B
    let search_start = b_start.saturating_sub(100).max(2);
    let search_end = b_start + 200;
    
    for b in (search_start..=search_end).step_by(2) {
        // Calculate minimum A such that A * B >= C
        let mut a = ((c as f64) / (b as f64)).ceil() as u32;
        a = a.max(1);
        
        // Ensure A is odd
        if a % 2 == 0 {
            a += 1;
        }
        
        let product = a * b;
        if product < c {
            continue; // Doesn't satisfy constraint
        }
        
        // Calculate actual ratio: ((A+1)/2) / (B * sqrt(3)/2)
        let actual_ratio = ((a as f64 + 1.0) / 2.0) / ((b as f64) * sqrt3 / 2.0);
        let ratio_error = (actual_ratio - target_ratio).abs();
        
        // Update best solution if:
        // 1. This is the first valid solution, OR
        // 2. This has a smaller ratio error, OR
        // 3. Same ratio error but smaller product
        let is_better = match best_solution {
            None => true,
            Some((_, _, best_err, best_prod)) => {
                ratio_error < best_err || 
                (ratio_error == best_err && product < best_prod)
            }
        };
        
        if is_better {
            best_solution = Some((a, b, ratio_error, product));
            
            // Early exit if we found a very good match
            if ratio_error < 0.0001 {
                break;
            }
        }
    }
    
    // Return best solution or fallback
    match best_solution {
        Some((a, b, _, _)) => (a, b),
        None => {
            // Fallback: ensure A is odd and B is even
            let mut a = c;
            if a % 2 == 0 {
                a += 1;
            }
            (a, 2)
        }
    }
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


/// Decodes a PNG image back to the original text payload.
/// Currently returns dummy text for testing.
/// TODO: Implement actual image decoding and Reed-Solomon error correction.
pub fn decode_image(png_bytes: &[u8]) -> Result<String, String> {
    if png_bytes.is_empty() {
        return Err("Empty image data".to_string());
    }
    
    // Load the image
    let img = image::load_from_memory(png_bytes)
        .map_err(|e| format!("Failed to load image: {}", e))?
        .to_rgba8();
    
    // Convert to grayscale for processing
    let gray_img = image::DynamicImage::ImageRgba8(img.clone()).to_luma8();
    
    // Find triangles in the image
    let triangles = find_triangles(&gray_img)?;
    
    // Select 10 outermost triangles
    let outermost = select_outermost_triangles(&triangles, 10);
    
    // Draw rectangles on the image
    let mut annotated_img = img.clone();
    draw_rectangles(&mut annotated_img, &outermost);
    
    // Encode annotated image to PNG
    let mut png_bytes_vec = Vec::new();
    let mut encoder = PngEncoder::new(&mut png_bytes_vec);
    encoder.write_image(
        annotated_img.as_raw(),
        annotated_img.width(),
        annotated_img.height(),
        ExtendedColorType::Rgba8,
    ).map_err(|e| format!("Failed to encode image: {}", e))?;
    
    // Encode to base64 for JSON transmission
    let base64_image = base64_encode(&png_bytes_vec);
    
    // Create response structure
    let response = DecodeResponse {
        image_width: gray_img.width(),
        image_height: gray_img.height(),
        total_triangles: triangles.len(),
        orientation_marks: outermost,
        annotated_image: base64_image,
    };
    
    // Return as JSON
    serde_json::to_string_pretty(&response)
        .map_err(|e| format!("Failed to serialize response: {}", e))
}

/// Represents a detected triangle region
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Triangle {
    pub center_x: f32,
    pub center_y: f32,
    pub area: f32,
    pub is_black: bool, // true if filled with black, false if white
    pub min_x: u32,     // Bounding box - left edge
    pub min_y: u32,     // Bounding box - top edge
    pub max_x: u32,     // Bounding box - right edge
    pub max_y: u32,     // Bounding box - bottom edge
}

/// Response structure for decode_image
#[derive(Debug, Serialize, Deserialize)]
pub struct DecodeResponse {
    pub image_width: u32,
    pub image_height: u32,
    pub total_triangles: usize,
    pub orientation_marks: Vec<Triangle>,
    pub annotated_image: String, // Base64 encoded PNG with red rectangles
}

/// Find triangles in a grayscale image
fn find_triangles(img: &GrayImage) -> Result<Vec<Triangle>, String> {
    let width = img.width();
    let height = img.height();
    
    if width == 0 || height == 0 {
        return Err("Invalid image dimensions".to_string());
    }
    
    // Apply binary threshold to separate black and white regions
    let binary = threshold(img, 128, ThresholdType::Binary);
    
    // Apply edge detection (for future use in more sophisticated detection)
    let _edges = canny(&binary, 50.0, 100.0);
    
    let mut triangles = Vec::new();
    
    // Simple approach: scan the image in a grid pattern
    // and look for triangle-like patterns
    // We'll look for regions with triangular intensity patterns
    
    // Estimate triangle size based on image dimensions
    // Assume we have at least 5x5 triangles in the smallest dimension
    let min_dim = width.min(height);
    let estimated_triangle_size = (min_dim / 10).max(20); // At least 20 pixels
    
    let step = estimated_triangle_size / 2; // Overlap regions
    
    for y in (0..height).step_by(step as usize) {
        for x in (0..width).step_by(step as usize) {
            // Check if this region looks like a triangle
            if let Some(triangle) = analyze_region(
                &binary,
                x,
                y,
                estimated_triangle_size,
                estimated_triangle_size,
            ) {
                triangles.push(triangle);
            }
        }
    }
    
    Ok(triangles)
}

/// Select N triangles with the most extreme x/y positions (outer corners)
/// Only considers black triangles
fn select_outermost_triangles(triangles: &[Triangle], count: usize) -> Vec<Triangle> {
    // Filter for only black triangles
    let mut black_triangles: Vec<_> = triangles.iter().filter(|t| t.is_black).cloned().collect();
    
    if black_triangles.is_empty() {
        return Vec::new();
    }
    
    // Sort by distance from center, taking the most extreme (furthest from center)
    if black_triangles.is_empty() {
        return Vec::new();
    }
    
    // Calculate image center
    let avg_x = black_triangles.iter().map(|t| t.center_x).sum::<f32>() / black_triangles.len() as f32;
    let avg_y = black_triangles.iter().map(|t| t.center_y).sum::<f32>() / black_triangles.len() as f32;
    
    // Sort by distance from center (descending)
    black_triangles.sort_by(|a, b| {
        let dist_a = (a.center_x - avg_x).powi(2) + (a.center_y - avg_y).powi(2);
        let dist_b = (b.center_x - avg_x).powi(2) + (b.center_y - avg_y).powi(2);
        dist_b.partial_cmp(&dist_a).unwrap_or(std::cmp::Ordering::Equal)
    });
    
    // Take the first `count` triangles
    black_triangles.into_iter().take(count).collect()
}

/// Analyze a region to see if it contains a triangle pattern
fn analyze_region(
    img: &GrayImage,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Option<Triangle> {
    let img_width = img.width();
    let img_height = img.height();
    
    // Make sure region is within bounds
    if x + width > img_width || y + height > img_height {
        return None;
    }
    
    // Sample pixels in the region
    let mut black_pixels = 0;
    let mut white_pixels = 0;
    let mut total_pixels = 0;
    
    for dy in 0..height {
        for dx in 0..width {
            let px = x + dx;
            let py = y + dy;
            
            if px < img_width && py < img_height {
                let pixel = img.get_pixel(px, py)[0];
                total_pixels += 1;
                
                if pixel < 128 {
                    black_pixels += 1;
                } else {
                    white_pixels += 1;
                }
            }
        }
    }
    
    if total_pixels == 0 {
        return None;
    }
    
    // Check if region has significant color (not mostly gray/mixed)
    let black_ratio = black_pixels as f32 / total_pixels as f32;
    let white_ratio = white_pixels as f32 / total_pixels as f32;
    
    // A triangle should be mostly one color
    // Account for borders and anti-aliasing
    if black_ratio > 0.4 || white_ratio > 0.4 {
        Some(Triangle {
            center_x: x as f32 + width as f32 / 2.0,
            center_y: y as f32 + height as f32 / 2.0,
            area: (width * height) as f32,
            is_black: black_ratio > white_ratio,
            min_x: x,
            min_y: y,
            max_x: x + width,
            max_y: y + height,
        })
    } else {
        None
    }
}

/// Draw red rectangles on the image based on triangle bounding boxes
fn draw_rectangles(img: &mut ImageBuffer<Rgba<u8>, Vec<u8>>, triangles: &[Triangle]) {
    let red = Rgba([255, 0, 0, 255]);
    let border_width = 2;
    
    for triangle in triangles {
        // Draw rectangle borders
        let min_x = triangle.min_x.max(0).min(img.width().saturating_sub(1));
        let min_y = triangle.min_y.max(0).min(img.height().saturating_sub(1));
        let max_x = triangle.max_x.min(img.width());
        let max_y = triangle.max_y.min(img.height());
        
        // Top border
        for x in min_x..max_x {
            for dy in 0..border_width {
                let y = (min_y + dy).min(img.height() - 1);
                if x < img.width() && y < img.height() {
                    img.put_pixel(x, y, red);
                }
            }
        }
        
        // Bottom border
        for x in min_x..max_x {
            for dy in 0..border_width {
                let y = (max_y.saturating_sub(1 + dy)).max(0);
                if x < img.width() && y < img.height() {
                    img.put_pixel(x, y, red);
                }
            }
        }
        
        // Left border
        for y in min_y..max_y {
            for dx in 0..border_width {
                let x = (min_x + dx).min(img.width() - 1);
                if x < img.width() && y < img.height() {
                    img.put_pixel(x, y, red);
                }
            }
        }
        
        // Right border
        for y in min_y..max_y {
            for dx in 0..border_width {
                let x = (max_x.saturating_sub(1 + dx)).max(0);
                if x < img.width() && y < img.height() {
                    img.put_pixel(x, y, red);
                }
            }
        }
    }
}

/// Encode bytes to base64
fn base64_encode(data: &[u8]) -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    
    for chunk in data.chunks(3) {
        let b1 = chunk[0];
        let b2 = if chunk.len() > 1 { chunk[1] } else { 0 };
        let b3 = if chunk.len() > 2 { chunk[2] } else { 0 };
        
        let n = ((b1 as u32) << 16) | ((b2 as u32) << 8) | (b3 as u32);
        
        result.push(CHARSET[((n >> 18) & 63) as usize] as char);
        result.push(CHARSET[((n >> 12) & 63) as usize] as char);
        
        if chunk.len() > 1 {
            result.push(CHARSET[((n >> 6) & 63) as usize] as char);
        } else {
            result.push('=');
        }
        
        if chunk.len() > 2 {
            result.push(CHARSET[(n & 63) as usize] as char);
        } else {
            result.push('=');
        }
    }
    
    result
}
