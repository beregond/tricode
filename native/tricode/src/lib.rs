use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, GrayImage, ImageBuffer, ImageEncoder, Rgba};
use imageproc::contrast::{threshold, ThresholdType};
use reed_solomon_erasure::galois_8::ReedSolomon;
use serde::{Deserialize, Serialize};

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
fn create_image_from_binary_data(
    encoded_data: &[u8],
    width_ratio: u32,
    height_ratio: u32,
) -> Vec<u8> {
    const PIXELS_PER_BIT_WIDTH: u32 = 51; // Bit box width in pixels
    let pixels_per_bit_height =
        ((PIXELS_PER_BIT_WIDTH as f64) * (3.0_f64.sqrt() / 2.0)).ceil() as u32;

    // Calculate total bits needed, we add 8 bits for border marks
    let total_bits = (encoded_data.len() + 1) * 8;

    // Calculate dimensions maintaining the aspect ratio and ensuring enough bit squares
    let gcd_val = gcd(width_ratio, height_ratio);
    let normalized_width = width_ratio / gcd_val;
    let normalized_height = height_ratio / gcd_val;

    // Find optimal integer dimensions using the previously written function
    let (width_squares, height_squares) =
        find_optimal_factors(total_bits as u32, normalized_width, normalized_height);

    // Calculate padding
    let padding_x = PIXELS_PER_BIT_WIDTH / 2;
    let padding_y = pixels_per_bit_height / 2;

    // Actual pixel dimensions (including padding)
    let width_without_padding = width_squares.div_ceil(2) * PIXELS_PER_BIT_WIDTH;
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

            let bit = if (is_first_row || is_last_row) && (is_first_col || is_last_col) {
                // Orientation marks: 1st and last positions are always 1 (black)
                1
            } else if (is_first_row || is_last_row) && (is_second_col || is_before_last_col) {
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
                            let is_base = if pointing_down {
                                dy == 0
                            } else {
                                dy == pixels_per_bit_height - 1
                            };
                            let is_tip = if pointing_down {
                                dy == pixels_per_bit_height - 1
                            } else {
                                dy == 0
                            };
                            let is_left_edge = (x - center_x).abs() >= half_width - 0.5;
                            let is_right_edge = (x - center_x).abs() >= half_width - 1.5;

                            let is_border = is_base || is_tip || is_left_edge || is_right_edge;
                            if !is_border {
                                img.put_pixel(px, py, rgba);
                            }
                        }
                    }
                }
            }
        }
    }

    let mut png_bytes = Vec::new();
    let encoder = PngEncoder::new(&mut png_bytes);
    if encoder
        .write_image(img.as_raw(), width, height, ExtendedColorType::Rgba8)
        .is_err()
    {
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
    if !b_start.is_multiple_of(2) {
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
        if a.is_multiple_of(2) {
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
                ratio_error < best_err || (ratio_error == best_err && product < best_prod)
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
            if a.is_multiple_of(2) {
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
    let shard_size = data.len().div_ceil(data_shard_count);

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

    // Trim to expected length (caller may pass more data than needed)
    let expected_len = 8 + (total_shards * shard_size);
    let encoded_data = if encoded_data.len() > expected_len {
        encoded_data[..expected_len].to_vec()
    } else if encoded_data.len() != expected_len {
        return Err(format!(
            "Invalid encoded data length: expected {}, got {}",
            expected_len,
            encoded_data.len()
        ));
    } else {
        encoded_data
    };

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
    for shard in shards.iter().take(data_shard_count).flatten() {
        result.extend_from_slice(shard);
    }

    // Trim to original length
    result.truncate(original_len);

    Ok(result)
}

/// Decodes a PNG image back to the original text payload.
/// Currently returns dummy text for testing.
/// TODO: Implement actual image decoding and Reed-Solomon error correction.
/// Debug information collected during image decoding.
#[derive(Debug, Clone)]
pub struct DecodeDebug {
    pub text: Option<String>,  // Some on success, None on decode failure
    pub error: Option<String>, // Some on failure, None on success
    pub image_width: u32,
    pub image_height: u32,
    pub total_triangles: usize,
    pub row_count: usize,
    pub column_count: usize,
    pub bit_count: usize,
    pub annotated_png: Vec<u8>,
}

pub fn decode_image(png_bytes: &[u8]) -> Result<DecodeDebug, String> {
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

    // Count rows and columns — naive method first, new methods as fallback.
    let row_naive = calculate_row_count(&outermost);
    let col_naive = calculate_column_count(&triangles, &outermost);

    // Draw rectangles on the annotated image
    let mut annotated_img = img.clone();
    draw_rectangles(&mut annotated_img, &outermost);

    // Encode annotated image to PNG bytes
    let mut annotated_png = Vec::new();
    PngEncoder::new(&mut annotated_png)
        .write_image(
            annotated_img.as_raw(),
            annotated_img.width(),
            annotated_img.height(),
            ExtendedColorType::Rgba8,
        )
        .map_err(|e| format!("Failed to encode annotated image: {}", e))?;

    // Helper: attempt a full decode with the given row/col counts.
    let try_decode = |rows: usize, cols: usize| -> Result<String, String> {
        let (bits, _) = read_bits_from_grid(&gray_img, rows, cols, &outermost);
        let decoded_bytes = reed_solomon_decode(bits)?;
        String::from_utf8(decoded_bytes)
            .map_err(|e| format!("Decoded data is not valid UTF-8: {}", e))
    };

    // --- Attempt 1: naive row/col values ---
    eprintln!("DEBUG attempt 1: rows={} cols={}", row_naive, col_naive);
    match try_decode(row_naive, col_naive) {
        Ok(text) => {
            return Ok(DecodeDebug {
                text: Some(text),
                error: None,
                image_width: gray_img.width(),
                image_height: gray_img.height(),
                total_triangles: triangles.len(),
                row_count: row_naive,
                column_count: col_naive,
                bit_count: {
                    let (_, bc) = read_bits_from_grid(&gray_img, row_naive, col_naive, &outermost);
                    bc
                },
                annotated_png: annotated_png.clone(),
            });
        }
        Err(e1) => {
            eprintln!("DEBUG attempt 1 failed: {}", e1);

            // --- Attempt 2: cluster + pitch estimates, clamped to naive ± 1 ---
            let row_raw = count_rows_by_clustering(&triangles);
            let (col_raw, pitch_px) = count_columns_by_pitch(&triangles, &outermost);
            // Clamp: don't deviate more than 1 from naive — wilder guesses are
            // left to the caller or future attempts.
            let row_adj = row_raw.clamp(row_naive.saturating_sub(1), row_naive + 1);
            let col_adj = col_raw.clamp(col_naive.saturating_sub(1), col_naive + 1);
            eprintln!(
                "DEBUG attempt 2: rows={} (raw {}) cols={} (raw {}, pitch={:.1}px)",
                row_adj, row_raw, col_adj, col_raw, pitch_px
            );

            let (bits2, bit_count2) = read_bits_from_grid(&gray_img, row_adj, col_adj, &outermost);

            let debug = DecodeDebug {
                text: None,
                error: None,
                image_width: gray_img.width(),
                image_height: gray_img.height(),
                total_triangles: triangles.len(),
                row_count: row_adj,
                column_count: col_adj,
                bit_count: bit_count2,
                annotated_png,
            };

            // Attempt 2 result — succeed immediately, or fall through to attempt 3.
            let _e2_msg = match reed_solomon_decode(bits2) {
                Ok(decoded_bytes) => match String::from_utf8(decoded_bytes) {
                    Ok(text) => {
                        return Ok(DecodeDebug {
                            text: Some(text),
                            ..debug
                        })
                    }
                    Err(e) => format!("decoded data is not valid UTF-8: {}", e),
                },
                Err(e) => e,
            };

            // --- Attempt 3: try all ±1 permutations of (row_adj, col_adj) ---
            let offsets: &[(isize, isize)] = &[
                (-1, -1),
                (-1, 0),
                (-1, 1),
                (0, -1),
                (0, 1),
                (1, -1),
                (1, 0),
                (1, 1),
            ];
            eprintln!(
                "DEBUG attempt 3: trying ±1 permutations of rows={} cols={}",
                row_adj, col_adj
            );
            for &(dr, dc) in offsets {
                let r = (row_adj as isize + dr) as usize;
                let c = (col_adj as isize + dc) as usize;
                if r < 2 || c < 2 {
                    continue;
                }
                let (bits3, bit_count3) = read_bits_from_grid(&gray_img, r, c, &outermost);
                if let Ok(decoded) = reed_solomon_decode(bits3) {
                    if let Ok(text) = String::from_utf8(decoded) {
                        eprintln!("DEBUG attempt 3 succeeded: rows={} cols={}", r, c);
                        return Ok(DecodeDebug {
                            text: Some(text),
                            row_count: r,
                            column_count: c,
                            bit_count: bit_count3,
                            ..debug
                        });
                    }
                }
            }

            // All attempts failed — report with the attempt-2 debug values.
            Ok(DecodeDebug {
                error: Some(format!(
                    "All attempts failed.\n1 (naive rows={} cols={}): {}\n2 (adjusted rows={} cols={}): RS/UTF-8 error\n3 (±1 permutations of rows={} cols={}): all failed",
                    row_naive, col_naive, e1, row_adj, col_adj, row_adj, col_adj,
                )),
                ..debug
            })
        }
    }
}

/// Represents a detected triangle region
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Triangle {
    pub center_x: f32,
    pub center_y: f32,
    pub area: f32,
    pub is_black: bool,    // true if filled with black, false if white
    pub pointing_up: bool, // true if apex points up, false if points down
    pub min_x: u32,        // Bounding box - left edge
    pub min_y: u32,        // Bounding box - top edge
    pub max_x: u32,        // Bounding box - right edge
    pub max_y: u32,        // Bounding box - bottom edge
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
fn flood_fill(
    binary: &GrayImage,
    visited: &mut [Vec<bool>],
    start_x: u32,
    start_y: u32,
) -> Vec<(u32, u32)> {
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
    if !(0.3..=0.8).contains(&fill_ratio) {
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
    if !(0.3..=3.0).contains(&aspect_ratio) {
        return None;
    }

    // Detect orientation: check pixel density in top vs bottom half
    let mid_y = (min_y + max_y) / 2;
    let mut top_pixels = 0;
    let mut bottom_pixels = 0;

    for &(_x, y) in component {
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
/// Approach 2: count distinct row clusters by grouping all triangles on their
/// center_y with a tolerance of ~half a cell height.
///
/// Returns the cluster count, or 0 if triangles is empty.
fn count_rows_by_clustering(triangles: &[Triangle]) -> usize {
    if triangles.is_empty() {
        return 0;
    }

    // Estimate a single cell height from the median bounding-box height.
    let mut heights: Vec<f32> = triangles
        .iter()
        .map(|t| (t.max_y - t.min_y) as f32)
        .collect();
    heights.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let cell_h = heights[heights.len() / 2].max(1.0);
    let tolerance = cell_h * 0.5;

    // Collect center_y values sorted ascending.
    let mut ys: Vec<f32> = triangles.iter().map(|t| t.center_y).collect();
    ys.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    // Walk through sorted y values; start a new cluster whenever the gap from
    // the current cluster mean exceeds the tolerance.
    let mut clusters: Vec<f32> = Vec::new(); // stores the running mean of each cluster
    for y in ys {
        if let Some(mean) = clusters.last_mut() {
            if (y - *mean).abs() <= tolerance {
                // Blend y into the running mean (simple online mean).
                // We use the cluster index to keep a proper average.
                // Since we only need the count, a running mean is fine.
                *mean = (*mean + y) / 2.0;
                continue;
            }
        }
        clusters.push(y);
    }

    clusters.len()
}

/// Approach 3: estimate the horizontal cell pitch from the mode of all
/// pairwise Δcenter_x values among triangles that share the same approximate
/// row, then derive the column count from the corner span divided by that
/// pitch.
///
/// Returns (column_count, pitch_px).
fn count_columns_by_pitch(triangles: &[Triangle], corners: &[Triangle]) -> (usize, f32) {
    if triangles.len() < 2 {
        return (0, 1.0);
    }

    // Estimate cell height for row grouping (same as above).
    let mut heights: Vec<f32> = triangles
        .iter()
        .map(|t| (t.max_y - t.min_y) as f32)
        .collect();
    heights.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let cell_h = heights[heights.len() / 2].max(1.0);
    let row_tol = cell_h * 0.5;

    // Group triangles into rows by center_y clusters (reuse the same logic).
    let mut sorted: Vec<&Triangle> = triangles.iter().collect();
    sorted.sort_by(|a, b| {
        a.center_y
            .partial_cmp(&b.center_y)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Proper row grouping with (mean_y, xs) tuples.
    let mut rows: Vec<(f32, Vec<f32>)> = Vec::new();
    for t in &sorted {
        if let Some((mean_y, xs)) = rows.last_mut() {
            if (t.center_y - *mean_y).abs() <= row_tol {
                xs.push(t.center_x);
                *mean_y = (*mean_y + t.center_y) / 2.0;
                continue;
            }
        }
        rows.push((t.center_y, vec![t.center_x]));
    }

    // Collect all adjacent Δx values within every row.
    let mut deltas: Vec<f32> = Vec::new();
    for (_, xs) in &mut rows {
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        for pair in xs.windows(2) {
            let d = pair[1] - pair[0];
            if d > 1.0 {
                // ignore duplicates / noise
                deltas.push(d);
            }
        }
    }

    if deltas.is_empty() {
        return (0, 1.0);
    }

    // Find the mode of deltas by bucketing into bins of width ~2 px and
    // picking the bin with the most entries.  The bin centre is the pitch.
    deltas.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let min_d = deltas[0];
    let bin_width = 3.0_f32; // px — coarse enough to group near-identical values
    let num_bins = ((deltas.last().copied().unwrap_or(min_d) - min_d) / bin_width) as usize + 1;
    let mut bins = vec![0usize; num_bins.max(1)];
    for &d in &deltas {
        let b = ((d - min_d) / bin_width) as usize;
        let b = b.min(bins.len() - 1);
        bins[b] += 1;
    }
    let best_bin = bins
        .iter()
        .enumerate()
        .max_by_key(|&(_, &c)| c)
        .map(|(i, _)| i)
        .unwrap_or(0);
    let pitch = min_d + best_bin as f32 * bin_width + bin_width / 2.0;

    // Derive column count from the corner-to-corner horizontal span.
    if corners.len() < 2 {
        return (0, pitch);
    }
    let min_cx = corners.iter().map(|t| t.center_x).fold(f32::MAX, f32::min);
    let max_cx = corners.iter().map(|t| t.center_x).fold(f32::MIN, f32::max);
    let span = (max_cx - min_cx).abs();
    let col_count = (span / pitch).round() as usize + 1;

    (col_count, pitch)
}

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
    corners_sorted.sort_by(|a, b| {
        a.center_y
            .partial_cmp(&b.center_y)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let top_row_y = corners_sorted.first().map(|t| t.center_y).unwrap_or(0.0);
    let row_threshold = median_width * 0.6;

    let mut top_row: Vec<&Triangle> = corners_sorted
        .into_iter()
        .filter(|t| (t.center_y - top_row_y).abs() <= row_threshold)
        .collect();

    if top_row.len() < 2 {
        // Fallback: use the two farthest corners by x
        let mut by_x: Vec<&Triangle> = corners.iter().collect();
        by_x.sort_by(|a, b| {
            a.center_x
                .partial_cmp(&b.center_x)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        top_row = by_x.into_iter().take(2).collect();
    }

    if top_row.len() < 2 {
        return 0;
    }

    top_row.sort_by(|a, b| {
        a.center_x
            .partial_cmp(&b.center_x)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let left = top_row.first().unwrap();
    let right = top_row.last().unwrap();

    // M is the number of triangle widths between corners
    let distance = (right.center_x - left.center_x).abs();
    let m = (distance / median_width).round() as usize;

    2 * (m - 2) + 1
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
        (0.0, 0.0),                   // Top-left
        (max_x as f32, 0.0),          // Top-right
        (0.0, max_y as f32),          // Bottom-left
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
    selected
        .iter()
        .filter_map(|c| c.clone())
        .take(count)
        .collect()
}

/// Sample a small rectangular region around (cx, cy) in a grayscale image.
/// Returns true if more than half the sampled pixels are dark (< 128).
fn sample_region_is_dark(img: &GrayImage, cx: f32, cy: f32, radius: u32) -> bool {
    let r = radius as i32;
    let x0 = (cx as i32 - r).max(0) as u32;
    let y0 = (cy as i32 - r).max(0) as u32;
    let x1 = ((cx as i32 + r) as u32).min(img.width().saturating_sub(1));
    let y1 = ((cy as i32 + r) as u32).min(img.height().saturating_sub(1));

    let mut dark = 0u32;
    let mut total = 0u32;
    for y in y0..=y1 {
        for x in x0..=x1 {
            if img.get_pixel(x, y)[0] < 128 {
                dark += 1;
            }
            total += 1;
        }
    }
    total > 0 && dark * 2 > total
}

/// Convert a detected triangle's pixel centroid to the bounding-box center of its grid cell.
/// Due to triangle geometry, the pixel centroid is offset from the bbox center by ±h/6:
///   pointing-down: centroid at h/3 from top  → bbox_center_y = centroid_y + h/6
///   pointing-up:   centroid at 2h/3 from top → bbox_center_y = centroid_y - h/6
/// The x-coordinate is symmetric, so centroid_x == bbox_center_x.
fn centroid_to_bbox_center(triangle: &Triangle) -> (f32, f32) {
    let h = (triangle.max_y - triangle.min_y + 1) as f32;
    let correction = h / 6.0;
    let bbox_cy = if triangle.pointing_up {
        triangle.center_y - correction
    } else {
        triangle.center_y + correction
    };
    (triangle.center_x, bbox_cy)
}

/// Read bits from the image by sampling the pixel color at the center of each
/// grid cell.  The orientation-mark cells (first/last row, first 2 and last 2
/// columns) are skipped — they are not data bits.  Returns the data bits packed
/// into bytes (MSB first).
fn read_bits_from_grid(
    img: &GrayImage,
    row_count: usize,
    column_count: usize,
    corners: &[Triangle],
) -> (Vec<u8>, usize) {
    if row_count < 2 || column_count < 2 || corners.len() < 4 {
        return (Vec::new(), 0);
    }

    // Identify the four corner triangles by their relative position in the image.
    // top-left  → minimum (center_x + center_y)
    // top-right → maximum (center_x − center_y)
    // bottom-left  → minimum (center_x − center_y)
    // bottom-right → maximum (center_x + center_y)
    let find = |f: fn(f32, f32) -> f32| -> Option<&Triangle> {
        corners.iter().max_by(|a, b| {
            f(a.center_x, a.center_y)
                .partial_cmp(&f(b.center_x, b.center_y))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    };

    let top_left = corners.iter().min_by(|a, b| {
        (a.center_x + a.center_y)
            .partial_cmp(&(b.center_x + b.center_y))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let top_right = find(|x, y| x - y);
    let bottom_left = corners.iter().min_by(|a, b| {
        (a.center_x - a.center_y)
            .partial_cmp(&(b.center_x - b.center_y))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let (Some(tl), Some(tr), Some(bl)) = (top_left, top_right, bottom_left) else {
        return (Vec::new(), 0);
    };

    // Convert pixel centroids → bounding-box cell centers
    let (tl_cx, tl_cy) = centroid_to_bbox_center(tl);
    let (tr_cx, _tr_cy) = centroid_to_bbox_center(tr);
    let (_bl_cx, bl_cy) = centroid_to_bbox_center(bl);

    let cols_f = (column_count - 1) as f32;
    let rows_f = (row_count - 1) as f32;

    let step_x = (tr_cx - tl_cx) / cols_f;
    let step_y = (bl_cy - tl_cy) / rows_f;

    // Sampling radius: ~15% of the step to stay well inside the triangle body
    let sample_radius = ((step_x * 0.15).round() as u32).max(2);

    eprintln!(
        "DEBUG grid: origin=({:.1},{:.1}) step=({:.1},{:.1}) radius={}",
        tl_cx, tl_cy, step_x, step_y, sample_radius
    );

    let mut bits: Vec<bool> = Vec::new();

    for row in 0..row_count {
        for col in 0..column_count {
            // Skip the 8 fixed orientation-mark cells (first & last row, first
            // 2 and last 2 columns).
            let is_edge_row = row == 0 || row == row_count - 1;
            let is_mark_col =
                col == 0 || col == 1 || col == column_count - 2 || col == column_count - 1;

            if is_edge_row && is_mark_col {
                continue;
            }

            let cx = tl_cx + col as f32 * step_x;
            let cy = tl_cy + row as f32 * step_y;

            bits.push(sample_region_is_dark(img, cx, cy, sample_radius));
        }
    }

    let bit_count = bits.len();

    // Pack bits into bytes (MSB first)
    let bytes: Vec<u8> = bits
        .chunks(8)
        .map(|chunk| {
            chunk
                .iter()
                .enumerate()
                .fold(0u8, |acc, (i, &b)| acc | ((b as u8) << (7 - i)))
        })
        .collect();

    (bytes, bit_count)
}

/// Draw red rectangles on the image based on triangle bounding boxes
fn draw_rectangles(img: &mut ImageBuffer<Rgba<u8>, Vec<u8>>, triangles: &[Triangle]) {
    let red = Rgba([255, 0, 0, 255]);
    let border_width = 5;

    for triangle in triangles {
        // Draw rectangle borders
        let min_x = triangle.min_x.min(img.width().saturating_sub(1));
        let min_y = triangle.min_y.min(img.height().saturating_sub(1));
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
                let y = max_y.saturating_sub(1 + dy);
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
                let x = max_x.saturating_sub(1 + dx);
                if x < img.width() && y < img.height() {
                    img.put_pixel(x, y, red);
                }
            }
        }
    }
}
