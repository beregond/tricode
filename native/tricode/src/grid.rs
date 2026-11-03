//! Grid geometry shared by the generator and the decoder: triangle and image sizes, how many bytes
//! a grid holds, and which cell each bit lands in.

use crate::frame::{EccLevel, FRAME_OVERHEAD, payload_capacity};

/// Triangle base width in pixels used when nothing else is asked for. It is also the smallest
/// allowed: below it the 1 px gaps of the Tiles style don't survive any resampling.
pub const DEFAULT_TRIANGLE_WIDTH: u32 = 37;
/// Smallest allowed triangle base width in pixels.
pub const MIN_TRIANGLE_WIDTH: u32 = DEFAULT_TRIANGLE_WIDTH;
/// Largest allowed triangle base width in pixels (about 4× the default).
pub const MAX_TRIANGLE_WIDTH: u32 = 149;
/// Largest image the generator renders, in pixels. The image is held in memory as RGBA while it is
/// drawn, so this is about 190 MiB; long payloads hit it even at the default triangle width.
pub const MAX_IMAGE_PIXELS: u64 = 50_000_000;

/// Bit box height in pixels: the height of an equilateral triangle `triangle_width` wide.
pub(crate) fn triangle_height(triangle_width: u32) -> u32 {
    ((triangle_width as f64) * (3.0_f64.sqrt() / 2.0)).ceil() as u32
}

/// Checks that a triangle width is odd (so each triangle is symmetric around a pixel column and
/// ends in a one-pixel tip) and within [`MIN_TRIANGLE_WIDTH`]..=[`MAX_TRIANGLE_WIDTH`].
pub(crate) fn validate_triangle_width(triangle_width: u32) -> Result<(), String> {
    if !(MIN_TRIANGLE_WIDTH..=MAX_TRIANGLE_WIDTH).contains(&triangle_width) {
        return Err(format!(
            "Triangle width must be between {} and {} px, got {}",
            MIN_TRIANGLE_WIDTH, MAX_TRIANGLE_WIDTH, triangle_width
        ));
    }
    if triangle_width.is_multiple_of(2) {
        return Err(format!(
            "Triangle width must be odd, got {}",
            triangle_width
        ));
    }
    Ok(())
}

/// Image size in pixels for a grid: half a cell of margin on every side, and columns overlapping by
/// half a triangle.
pub(crate) fn image_size(
    width_squares: u32,
    height_squares: u32,
    triangle_width: u32,
) -> (u32, u32) {
    let triangle_height = triangle_height(triangle_width);
    let width = width_squares.div_ceil(2) * triangle_width + (triangle_width / 2) * 2;
    let height = height_squares * triangle_height + (triangle_height / 2) * 2;
    (width, height)
}

/// Fails when the image would exceed [`MAX_IMAGE_PIXELS`], naming the largest width that fits.
pub(crate) fn check_image_pixels(
    width_squares: u32,
    height_squares: u32,
    triangle_width: u32,
) -> Result<(), String> {
    let pixels = |w| {
        let (width, height) = image_size(width_squares, height_squares, w);
        width as u64 * height as u64
    };
    let requested = pixels(triangle_width);
    if requested <= MAX_IMAGE_PIXELS {
        return Ok(());
    }
    let limit = format!(
        "Image would be {:.1} MP, over the {} MP limit",
        requested as f64 / 1e6,
        MAX_IMAGE_PIXELS / 1_000_000
    );
    match (MIN_TRIANGLE_WIDTH..=triangle_width)
        .rev()
        .step_by(2)
        .find(|&w| pixels(w) <= MAX_IMAGE_PIXELS)
    {
        Some(w) => Err(format!(
            "{limit}; the largest triangle width that fits is {w} px"
        )),
        None => Err(format!(
            "{limit} even at the smallest triangle width; shorten the payload"
        )),
    }
}

/// Orientation of the triangle in cell `(row, col)`, as the renderer draws it.
pub(crate) fn points_down(row: usize, col: usize) -> bool {
    (row + col).is_multiple_of(2)
}

/// Whether cell `(row, col)` of a `rows × cols` grid is one of the 8 orientation marks: the first
/// two and last two cells of the first and last rows. They carry no data.
pub(crate) fn is_orientation_mark(row: usize, col: usize, rows: usize, cols: usize) -> bool {
    (row == 0 || row + 1 == rows) && (col < 2 || col + 2 >= cols)
}

/// Foreground (bit = 1) flag of every grid cell, laid out as the renderer draws them: orientation
/// marks (the corner cells set, the ones next to them clear), then the data bits MSB first, then
/// zero padding.
pub(crate) fn layout_cells(data: &[u8], rows: u32, cols: u32) -> Vec<Vec<bool>> {
    let (rows, cols) = (rows as usize, cols as usize);
    let mut bits = (0..data.len() * 8).map(|i| (data[i / 8] >> (7 - i % 8)) & 1 == 1);
    (0..rows)
        .map(|row| {
            (0..cols)
                .map(|col| {
                    if is_orientation_mark(row, col, rows, cols) {
                        col == 0 || col + 1 == cols
                    } else {
                        bits.next().unwrap_or(false)
                    }
                })
                .collect()
        })
        .collect()
}

/// Packs bits into bytes, MSB first; the last byte is zero-padded.
pub(crate) fn pack_bits(bits: &[bool]) -> Vec<u8> {
    bits.chunks(8)
        .map(|chunk| {
            chunk
                .iter()
                .enumerate()
                .fold(0u8, |acc, (i, &b)| acc | (u8::from(b) << (7 - i)))
        })
        .collect()
}

/// Bytes of data a `rows × cols` grid holds: every cell but the 8 orientation marks is a bit.
pub(crate) fn grid_capacity(rows: usize, cols: usize) -> usize {
    (rows * cols).saturating_sub(8) / 8
}

/// The smallest grid (as `(columns, rows)`) of the requested aspect ratio whose capacity holds
/// `payload_len` bytes at `level`.
pub(crate) fn grid_for_payload(
    payload_len: usize,
    width: u32,
    height: u32,
    level: EccLevel,
) -> (u32, u32) {
    // Start from the ideal byte count and grow until the grid's own layout fits the payload.
    let mut bytes =
        ((payload_len + FRAME_OVERHEAD) as f64 / (1.0 - level.ecc_share())).ceil() as usize;
    loop {
        let (cols, rows) = find_optimal_factors((bytes * 8 + 8) as u32, width, height);
        let capacity = grid_capacity(rows as usize, cols as usize);
        let fits = payload_capacity(capacity, level);
        if fits >= payload_len {
            return (cols, rows);
        }
        bytes += (payload_len - fits).max(1);
    }
}

/// Calculate greatest common divisor for ratio normalization
pub(crate) fn gcd(mut a: u32, mut b: u32) -> u32 {
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
pub(crate) fn find_optimal_factors(c: u32, x: u32, y: u32) -> (u32, u32) {
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
