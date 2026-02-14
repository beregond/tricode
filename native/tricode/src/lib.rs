use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageBuffer, ImageEncoder, Rgba, GrayImage};
use imageproc::contrast::{threshold, ThresholdType};
use reed_solomon_erasure::galois_8::ReedSolomon;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

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
    
    // Select 4 corner triangles
    let outermost = select_outermost_triangles(&triangles, 4);
    
    // Count rows by finding two corners with different orientations
    let row_count = calculate_row_count(&outermost);
    eprintln!("DEBUG: Detected {} rows", row_count);

    // Count columns from corner spacing
    let column_count = calculate_column_count(&triangles, &outermost);
    eprintln!("DEBUG: Detected {} columns", column_count);
    
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
    pub pointing_up: bool, // true if apex points up, false if points down
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

/// Find triangles in a grayscale image using connected component analysis
fn find_triangles(img: &GrayImage) -> Result<Vec<Triangle>, String> {
    let width = img.width();
    let height = img.height();
    
    if width == 0 || height == 0 {
        return Err("Invalid image dimensions".to_string());
    }
    
    // Apply binary threshold to separate black and white regions
    let binary = threshold(img, 128, ThresholdType::Binary);
    
    // Erode the binary image to separate touching triangles
    let eroded = erode_binary(&binary, 2);
    
    // Find connected components in the eroded image
    let mut triangles = Vec::new();
    let mut visited = vec![vec![false; width as usize]; height as usize];
    
    for y in 0..height {
        for x in 0..width {
            if !visited[y as usize][x as usize] && eroded.get_pixel(x, y)[0] == 0 {
                // Found a new black region in eroded image
                let eroded_component = flood_fill(&eroded, &mut visited, x, y);
                
                // Dilate the component back to approximate original size
                let dilated_component = dilate_component(&eroded_component, &binary, 2);
                
                if let Some(triangle) = analyze_component(&dilated_component, x, y) {
                    triangles.push(triangle);
                }
            }
        }
    }
    
    Ok(triangles)
}

/// Erode black regions in a binary image by removing boundary pixels
/// Erosion reduces black regions, helping to separate touching regions
fn erode_binary(binary: &GrayImage, radius: u32) -> GrayImage {
    let width = binary.width();
    let height = binary.height();
    let mut result = binary.clone();
    
    for _ in 0..radius {
        let mut eroded = result.clone();
        
        for y in 0..height {
            for x in 0..width {
                if result.get_pixel(x, y)[0] == 0 {
                    // This is a black pixel; check if all neighbors are also black
                    let mut is_eroded = false;
                    
                    for dx in -1..=1i32 {
                        for dy in -1..=1i32 {
                            let nx = (x as i32 + dx) as u32;
                            let ny = (y as i32 + dy) as u32;
                            
                            if nx >= width || ny >= height || result.get_pixel(nx, ny)[0] != 0 {
                                is_eroded = true;
                                break;
                            }
                        }
                        if is_eroded {
                            break;
                        }
                    }
                    
                    if is_eroded {
                        eroded.put_pixel(x, y, image::Luma([255])); // Make white
                    }
                }
            }
        }
        result = eroded;
    }
    
    result
}

/// Dilate a component back to approximate its original size
/// Only expands into pixels that are black in the original binary image
fn dilate_component(component: &[(u32, u32)], binary: &GrayImage, radius: u32) -> Vec<(u32, u32)> {
    let width = binary.width();
    let height = binary.height();
    let mut expanded: std::collections::HashSet<(u32, u32)> = component.iter().copied().collect();
    
    for _ in 0..radius {
        let mut new_expanded = expanded.clone();
        
        for &(x, y) in &expanded {
            // Check all neighbors within a 3x3 region
            for dx in -1..=1i32 {
                for dy in -1..=1i32 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    
                    let nx = (x as i32 + dx) as u32;
                    let ny = (y as i32 + dy) as u32;
                    
                    // Only expand into pixels that are black in the original
                    if nx < width && ny < height && binary.get_pixel(nx, ny)[0] == 0 {
                        new_expanded.insert((nx, ny));
                    }
                }
            }
        }
        expanded = new_expanded;
    }
    
    expanded.into_iter().collect()
}
fn flood_fill(binary: &GrayImage, visited: &mut Vec<Vec<bool>>, start_x: u32, start_y: u32) -> Vec<(u32, u32)> {
    let width = binary.width();
    let height = binary.height();
    let mut component = Vec::new();
    let mut stack = vec![(start_x, start_y)];
    
    while let Some((x, y)) = stack.pop() {
        if x >= width || y >= height || visited[y as usize][x as usize] {
            continue;
        }
        
        if binary.get_pixel(x, y)[0] != 0 {
            continue; // Not a black pixel
        }
        
        visited[y as usize][x as usize] = true;
        component.push((x, y));
        
        // Add neighbors to stack
        if x > 0 {
            stack.push((x - 1, y));
        }
        if x < width - 1 {
            stack.push((x + 1, y));
        }
        if y > 0 {
            stack.push((x, y - 1));
        }
        if y < height - 1 {
            stack.push((x, y + 1));
        }
    }
    
    component
}

/// Analyze a connected component to determine if it's a valid triangle
fn analyze_component(component: &[(u32, u32)], _origin_x: u32, _origin_y: u32) -> Option<Triangle> {
    if component.is_empty() {
        return None;
    }
    
    // Calculate bounding box and centroid
    let mut min_x = u32::MAX;
    let mut min_y = u32::MAX;
    let mut max_x = 0;
    let mut max_y = 0;
    let mut sum_x = 0u64;
    let mut sum_y = 0u64;
    
    for &(x, y) in component {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
        sum_x += x as u64;
        sum_y += y as u64;
    }
    
    let area = component.len() as f32;
    let count = component.len() as f32;
    let center_x = sum_x as f32 / count;
    let center_y = sum_y as f32 / count;
    
    // Filter by area: triangles should have reasonable size
    // Expected area based on bounding box dimensions
    let bbox_area = ((max_x - min_x + 1) * (max_y - min_y + 1)) as f32;
    let fill_ratio = area / bbox_area;
    
    // Triangles typically fill 40-70% of their bounding box
    if fill_ratio < 0.3 || fill_ratio > 0.8 {
        return None;
    }
    
    // Filter by minimum area (avoid noise)
    if area < 20.0 {
        return None;
    }
    
    // Filter by aspect ratio (triangles have reasonable width/height ratio)
    let width = (max_x - min_x + 1) as f32;
    let height = (max_y - min_y + 1) as f32;
    let aspect_ratio = width / height.max(1.0);
    
    // Accept a reasonable range for aspect ratios
    if aspect_ratio < 0.3 || aspect_ratio > 3.0 {
        return None;
    }
    
    // Detect orientation: check pixel density in top vs bottom half
    let mid_y = (min_y + max_y) / 2;
    let mut top_pixels = 0;
    let mut bottom_pixels = 0;
    
    for &(x, y) in component {
        if y < mid_y {
            top_pixels += 1;
        } else {
            bottom_pixels += 1;
        }
    }
    
    // If more pixels in top half, triangle points down (apex at bottom)
    // If more pixels in bottom half, triangle points up (apex at top)
    let pointing_up = bottom_pixels > top_pixels;
    
    Some(Triangle {
        center_x,
        center_y,
        area,
        is_black: true,
        pointing_up,
        min_x,
        min_y,
        max_x,
        max_y,
    })
}

/// Calculate the number of rows based on corner triangles with different orientations
fn calculate_row_count(corners: &[Triangle]) -> usize {
    if corners.len() < 2 {
        return 0;
    }
    
    // Find two triangles with different orientations
    let mut up_triangle: Option<&Triangle> = None;
    let mut down_triangle: Option<&Triangle> = None;
    
    for triangle in corners {
        if triangle.pointing_up && up_triangle.is_none() {
            up_triangle = Some(triangle);
        } else if !triangle.pointing_up && down_triangle.is_none() {
            down_triangle = Some(triangle);
        }
        
        if up_triangle.is_some() && down_triangle.is_some() {
            break;
        }
    }
    
    if let (Some(up), Some(down)) = (up_triangle, down_triangle) {
        // Calculate vertical distance between the two triangles
        let vertical_distance = ((up.min_y - down.max_y) as f32).abs();

        
        // Calculate average triangle height from bounding boxes
        let height1 = (up.max_y - up.min_y) as f32;
        let height2 = (down.max_y - down.min_y) as f32;
        let avg_height = (height1 + height2) / 2.0;
        
        if avg_height > 0.0 {
            // Number of rows: the distance includes borders, so we need to adjust
            // Round the division to get the number of intervals, then add 1 for the row count
            let intervals = (vertical_distance / avg_height).round() as usize;
            // But we're measuring center-to-center of filled triangles with borders,
            // so subtract 1 to account for border overlap
            return if intervals > 0 { intervals } else { 1 };
        }
    }
    
    0
}

/// Calculate the number of columns by clustering triangles into rows
/// and taking the maximum count across rows.
fn calculate_column_count(triangles: &[Triangle], corners: &[Triangle]) -> usize {
    if triangles.is_empty() || corners.len() < 2 {
        return 0;
    }

    // Estimate triangle width (median)
    let mut widths: Vec<f32> = triangles
        .iter()
        .map(|t| (t.max_x - t.min_x) as f32)
        .collect();
    widths.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = widths.len() / 2;
    let median_width = widths[mid].max(1.0);

    // Pick two corner triangles from the same row (top row by default)
    let mut corners_sorted: Vec<&Triangle> = corners.iter().collect();
    corners_sorted.sort_by(|a, b| a.center_y.partial_cmp(&b.center_y).unwrap_or(std::cmp::Ordering::Equal));
    let top_row_y = corners_sorted.first().map(|t| t.center_y).unwrap_or(0.0);
    let row_threshold = median_width * 0.6;

    let mut top_row: Vec<&Triangle> = corners_sorted
        .into_iter()
        .filter(|t| (t.center_y - top_row_y).abs() <= row_threshold)
        .collect();

    if top_row.len() < 2 {
        // Fallback: use the two farthest corners by x
        let mut by_x: Vec<&Triangle> = corners.iter().collect();
        by_x.sort_by(|a, b| a.center_x.partial_cmp(&b.center_x).unwrap_or(std::cmp::Ordering::Equal));
        top_row = by_x.into_iter().take(2).collect();
    }

    if top_row.len() < 2 {
        return 0;
    }

    top_row.sort_by(|a, b| a.center_x.partial_cmp(&b.center_x).unwrap_or(std::cmp::Ordering::Equal));
    let left = top_row.first().unwrap();
    let right = top_row.last().unwrap();

    // M is the number of triangle widths between corners
    let distance = (right.center_x - left.center_x).abs();
    let m = (distance / median_width).round() as usize;

    // Columns = 2*M + 1
    2 * m + 1
}

/// Select triangles from the 4 corners of the image
/// Only considers black triangles closest to each image corner
fn select_outermost_triangles(triangles: &[Triangle], count: usize) -> Vec<Triangle> {
    // Filter for only black triangles
    let black_triangles: Vec<_> = triangles.iter().filter(|t| t.is_black).cloned().collect();
    
    if black_triangles.is_empty() {
        return Vec::new();
    }
    
    // Get image dimensions from triangles
    let max_x = black_triangles.iter().map(|t| t.max_x).max().unwrap_or(0);
    let max_y = black_triangles.iter().map(|t| t.max_y).max().unwrap_or(0);
    
    // Define the 4 corners
    let corners = [
        (0.0, 0.0),           // Top-left
        (max_x as f32, 0.0),  // Top-right
        (0.0, max_y as f32),  // Bottom-left
        (max_x as f32, max_y as f32), // Bottom-right
    ];
    
    // For each corner, find the closest triangle
    let mut selected: [Option<Triangle>; 4] = [None, None, None, None];
    let mut min_distances: [f32; 4] = [f32::MAX; 4];
    
    for triangle in black_triangles {
        let center_x = (triangle.min_x as f32 + triangle.max_x as f32) / 2.0;
        let center_y = (triangle.min_y as f32 + triangle.max_y as f32) / 2.0;
        
        // Find distance to each corner
        for (idx, &(corner_x, corner_y)) in corners.iter().enumerate() {
            let dist = (center_x - corner_x).powi(2) + (center_y - corner_y).powi(2);
            
            // Keep the closest triangle to each corner
            if dist < min_distances[idx] {
                min_distances[idx] = dist;
                selected[idx] = Some(triangle.clone());
            }
        }
    }
    
    // Return selected triangles
    selected.iter()
        .filter_map(|c| c.clone())
        .take(count)
        .collect()
}



/// Draw red rectangles on the image based on triangle bounding boxes
fn draw_rectangles(img: &mut ImageBuffer<Rgba<u8>, Vec<u8>>, triangles: &[Triangle]) {
    let red = Rgba([255, 0, 0, 255]);
    let border_width = 5;
    
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
