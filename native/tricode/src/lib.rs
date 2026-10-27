/// A triangle whose bounding-box area exceeds this multiple of the largest anchor area
/// is considered a cluster/merge false positive and is discarded before grid reading.
const FALSE_POSITIVE_AREA_FACTOR: f32 = 1.2;

use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, GrayImage, ImageBuffer, ImageEncoder, Rgba};
use imageproc::contrast::{ThresholdType, threshold};
use reed_solomon_erasure::galois_8::ReedSolomon;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Pack a text payload into msgpack: `{"type": "<kind>", "value": "<payload>"}`.
pub fn pack_payload(kind: &str, text: &str) -> Result<Vec<u8>, String> {
    #[derive(Serialize)]
    struct Envelope<'a> {
        #[serde(rename = "type")]
        kind: &'a str,
        value: &'a str,
    }
    let envelope = Envelope { kind, value: text };
    rmp_serde::to_vec_named(&envelope).map_err(|e| format!("msgpack encode error: {}", e))
}

/// Unpack a msgpack blob produced by [`pack_payload`].
/// Returns `(kind, value)` where `kind` is e.g. `"text"`, `"url"`, or `"plain"`.
///
/// Fallback chain:
/// 1. Try to deserialise as the expected `{"type": ..., "value": ...}` envelope → return `(kind, value)`.
/// 2. If the struct match fails but it *is* valid msgpack, serialise the decoded value as a JSON
///    string and return `("plain", json)`.
/// 3. If it is not msgpack at all, try to interpret the raw bytes as UTF-8 and return
///    `("plain", string)`.
/// 4. Only if all three steps fail do we return `Err`.
pub fn unpack_payload(bytes: &[u8]) -> Result<(String, String), String> {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(rename = "type")]
        kind: String,
        value: String,
    }

    // Step 1 — try to match the expected named-map envelope.
    // `to_vec_named` writes a msgpack map with string keys; we must deserialise
    // in the same named/map mode — rmp_serde's default `from_slice` uses the
    // compact array format which is incompatible with named maps.
    let mut de = rmp_serde::Deserializer::new(bytes);
    if let Ok(envelope) = Envelope::deserialize(&mut de) {
        return Ok((envelope.kind, envelope.value));
    }

    // Step 2 — valid msgpack but wrong shape: decode to a generic JSON value and
    // return the JSON representation so the caller still gets something useful.
    let mut de2 = rmp_serde::Deserializer::new(bytes);
    if let Ok(json_val) = serde_json::Value::deserialize(&mut de2) {
        let json_str = serde_json::to_string(&json_val).unwrap_or_else(|_| json_val.to_string());
        return Ok(("plain".to_string(), json_str));
    }

    // Step 3 — not msgpack at all: try raw UTF-8 bytes.
    if let Ok(s) = std::str::from_utf8(bytes) {
        return Ok(("plain".to_string(), s.to_string()));
    }

    Err(format!(
        "payload is not a recognised msgpack envelope, valid msgpack, or UTF-8 text ({} bytes)",
        bytes.len()
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageStyle {
    /// Black triangles with a 1-pixel gap (border excluded) between neighbours.
    Tiles,
    /// Black triangles filled edge-to-edge with no gap.
    Stain,
    /// Each triangle independently drawn with or without border, determined by
    /// a pre-computed [`HorizonLayout`].
    Horizon,
}

impl ImageStyle {
    pub fn from_u32(v: u32) -> Self {
        match v {
            1 => Self::Stain,
            2 => Self::Horizon,
            _ => Self::Tiles,
        }
    }
}

/// Per-triangle group assignment for the [`ImageStyle::Horizon`] style.
///
/// Connected regions of foreground (bit=1) triangles are flood-filled into
/// groups.  Singletons (groups of exactly one triangle) are excluded and
/// stored as `None`.  Each multi-triangle group gets an index that the
/// renderer maps to a cycling palette of colours.
#[derive(Debug, Clone)]
pub struct HorizonLayout {
    /// `rows × cols` grid; `Some(group_id)` for grouped triangles, `None` otherwise.
    pub group_id: Vec<Vec<Option<usize>>>,
}

impl HorizonLayout {
    /// Build the layout from the encoded data and grid dimensions.
    ///
    /// `seed` is used to randomly select which foreground triangles to include
    /// in groups (coin-flip per triangle).  Pass `0` to use the current
    /// unix-time seconds as the seed.
    pub fn from_seed(rows: u32, cols: u32, seed: u64, encoded_data: &[u8]) -> Self {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        use std::time::SystemTime;

        let effective_seed = if seed == 0 {
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(42)
        } else {
            seed
        };

        let mut hasher = DefaultHasher::new();
        effective_seed.hash(&mut hasher);

        // --- Step 1: reconstruct which cells are foreground (bit == 1) ---
        // This mirrors the bit-extraction logic in create_image_from_binary_data.
        let mut is_fg = vec![vec![false; cols as usize]; rows as usize];
        let mut bit_index: usize = 0;
        let last_row = rows as usize - 1;
        let last_col = cols as usize - 1;
        for (row, row_cells) in is_fg.iter_mut().enumerate() {
            for (col, cell) in row_cells.iter_mut().enumerate() {
                let is_first_row = row == 0;
                let is_last_row = row == last_row;
                let is_first_col = col == 0;
                let is_second_col = col == 1;
                let is_before_last_col = col == last_col - 1;
                let is_last_col = col == last_col;

                let bit = if (is_first_row || is_last_row) && (is_first_col || is_last_col) {
                    1u8
                } else if (is_first_row || is_last_row) && (is_second_col || is_before_last_col) {
                    0u8
                } else {
                    let byte_index = bit_index / 8;
                    let bit_val = if byte_index < encoded_data.len() {
                        let bit_offset = 7 - (bit_index % 8);
                        (encoded_data[byte_index] >> bit_offset) & 1
                    } else {
                        0
                    };
                    bit_index += 1;
                    bit_val
                };
                *cell = bit == 1;
            }
        }

        // --- Step 2: select which foreground cells are "candidates" ---
        // Use the low 2 bits of the hash: candidate unless both bits are set,
        // giving a 75% selection rate so groups form more readily.
        let mut candidate = vec![vec![false; cols as usize]; rows as usize];
        for row in 0..rows as usize {
            for col in 0..cols as usize {
                if is_fg[row][col] {
                    row.hash(&mut hasher);
                    col.hash(&mut hasher);
                    candidate[row][col] = hasher.finish() & 3 != 3;
                }
            }
        }

        // --- Step 3: flood-fill connected candidates into groups ---
        use std::collections::VecDeque;

        let neighbours = |row: usize, col: usize| -> Vec<(usize, usize)> {
            let pd = col.is_multiple_of(2) ^ (row % 2 == 1);
            let mut nb = Vec::with_capacity(3);
            if col > 0 {
                nb.push((row, col - 1));
            }
            if col + 1 < cols as usize {
                nb.push((row, col + 1));
            }
            if pd && row > 0 {
                nb.push((row - 1, col));
            }
            if !pd && row + 1 < rows as usize {
                nb.push((row + 1, col));
            }
            nb
        };

        let mut visited = vec![vec![false; cols as usize]; rows as usize];
        let mut bfs_groups: Vec<Vec<(usize, usize)>> = Vec::new();

        for start_row in 0..rows as usize {
            for start_col in 0..cols as usize {
                if !candidate[start_row][start_col] || visited[start_row][start_col] {
                    continue;
                }
                let mut members = Vec::new();
                let mut queue = VecDeque::new();
                queue.push_back((start_row, start_col));
                visited[start_row][start_col] = true;
                while let Some((r, c)) = queue.pop_front() {
                    members.push((r, c));
                    for (nr, nc) in neighbours(r, c) {
                        if candidate[nr][nc] && !visited[nr][nc] {
                            visited[nr][nc] = true;
                            queue.push_back((nr, nc));
                        }
                    }
                }
                bfs_groups.push(members);
            }
        }

        // --- Step 3.5: split each BFS group into diagonal lane sub-groups ---
        //
        // For every member of a BFS group compute its eastern and western lane
        // index.  Both lane families produce candidate sub-groups.  Since each
        // triangle can only belong to one final sub-group we pick greedily:
        // sort all candidate lane-groups (across both families) by descending
        // size, then claim members first-come-first-served.
        //
        // After assignment, a claimed sub-group may be internally disconnected
        // (some members were taken by earlier, larger groups) so we split it
        // again by adjacency with a second BFS pass.

        let mut groups: Vec<Vec<(usize, usize)>> = Vec::new();

        for bfs_group in &bfs_groups {
            // Build lane buckets for both families
            let mut eastern_lanes: std::collections::HashMap<usize, Vec<(usize, usize)>> =
                std::collections::HashMap::new();
            let mut western_lanes: std::collections::HashMap<isize, Vec<(usize, usize)>> =
                std::collections::HashMap::new();

            for &(r, c) in bfs_group {
                let col_coef_eastern = match r {
                    x if x % 2 == 0 => (c + 1) & !1,
                    _ => c & !1,
                };
                let col_coef_western = match r {
                    x if x % 2 == 1 => (c + 1) & !1,
                    _ => c & !1,
                };
                let row_coef = (r / 2) * 2;

                let eastern_lane = r * 2 - row_coef + col_coef_eastern;
                let western_lane = col_coef_western as isize - (r as isize * 2 - row_coef as isize);

                eastern_lanes.entry(eastern_lane).or_default().push((r, c));
                western_lanes.entry(western_lane).or_default().push((r, c));
            }

            // Collect all candidate lane-groups, tag with size for sorting
            let mut candidates: Vec<Vec<(usize, usize)>> = eastern_lanes
                .into_values()
                .chain(western_lanes.into_values())
                .collect();

            // Largest first so big groups claim members before small ones
            candidates.sort_unstable_by_key(|b| std::cmp::Reverse(b.len()));

            // Claim members greedily; track which cells are already taken
            let mut taken = vec![vec![false; cols as usize]; rows as usize];
            let mut claimed: Vec<Vec<(usize, usize)>> = Vec::new();

            for candidate in candidates {
                let free: Vec<(usize, usize)> = candidate
                    .into_iter()
                    .filter(|&(r, c)| !taken[r][c])
                    .collect();
                if free.is_empty() {
                    continue;
                }
                for &(r, c) in &free {
                    taken[r][c] = true;
                }
                claimed.push(free);
            }

            // Split each claimed sub-group by adjacency with a second BFS pass.
            // After greedy claiming, a sub-group may be internally disconnected.
            for sub_group in claimed {
                // Build a set for fast lookup
                let member_set: std::collections::HashSet<(usize, usize)> =
                    sub_group.iter().copied().collect();
                let mut visited_adjacency = vec![vec![false; cols as usize]; rows as usize];

                // BFS over this sub-group to find adjacency-connected components
                for &start in &sub_group {
                    if visited_adjacency[start.0][start.1] {
                        continue;
                    }
                    let mut component = Vec::new();
                    let mut queue = VecDeque::new();
                    queue.push_back(start);
                    visited_adjacency[start.0][start.1] = true;

                    while let Some((r, c)) = queue.pop_front() {
                        component.push((r, c));
                        for (nr, nc) in neighbours(r, c) {
                            if member_set.contains(&(nr, nc)) && !visited_adjacency[nr][nc] {
                                visited_adjacency[nr][nc] = true;
                                queue.push_back((nr, nc));
                            }
                        }
                    }
                    groups.push(component);
                }
            }
        }

        // --- Step 4: assign group IDs, skip singletons ---
        let mut group_id: Vec<Vec<Option<usize>>> = vec![vec![None; cols as usize]; rows as usize];
        let mut colour_index: usize = 0;
        for members in &groups {
            if members.len() < 2 {
                continue; // singleton — leave as None
            }
            for &(r, c) in members {
                group_id[r][c] = Some(colour_index);
            }
            colour_index += 1;
        }

        Self { group_id }
    }
}

/// Debug information collected during image generation.
#[derive(Debug, Clone)]
pub struct GenerateDebug {
    pub png_bytes: Vec<u8>,
    pub encoded_bytes: usize, // bytes after Reed-Solomon encoding
    pub grid_rows: u32,
    pub grid_columns: u32,
    pub data_bits_used: usize, // bits actually written from encoded_data
    pub padding_bits: usize,   // zero-fill bits to complete the grid
}

/// Convert a Flutter/Android-style ARGB `u32` (`0xAARRGGBB`) to an `Rgba<u8>`.
/// If `force_opaque` is true the alpha channel is set to 255 regardless.
fn argb_to_rgba(argb: u32, force_opaque: bool) -> Rgba<u8> {
    let a = ((argb >> 24) & 0xFF) as u8;
    let r = ((argb >> 16) & 0xFF) as u8;
    let g = ((argb >> 8) & 0xFF) as u8;
    let b = (argb & 0xFF) as u8;
    Rgba([r, g, b, if force_opaque { 255 } else { a }])
}

/// Generates a PNG image encoding the payload text.
/// The payload is first encoded with Reed-Solomon error correction,
/// then the binary data is visualized as pixels: 0 bits = white, 1 bits = black.
pub fn generate_image(
    payload: &str,
    payload_type: &str,
    width_ratio: u32,
    height_ratio: u32,
) -> Vec<u8> {
    let payload_bytes = match pack_payload(payload_type, payload) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    let encoded_data = match reed_solomon_encode(payload_bytes) {
        Ok(data) => data,
        Err(_) => return Vec::new(),
    };

    create_image_from_binary_data(
        &encoded_data,
        width_ratio,
        height_ratio,
        ImageStyle::Tiles,
        argb_to_rgba(0xFF000000, true),
        argb_to_rgba(0xFFFFFFFF, false),
        0,
    )
    .png_bytes
}

/// Parameters for [`generate_image_debug`].
pub struct GenerateImageParams<'a> {
    pub payload: &'a str,
    pub payload_type: &'a str,
    pub width_ratio: u32,
    pub height_ratio: u32,
    pub style: ImageStyle,
    pub fg_color: u32,
    pub bg_color: u32,
    pub horizon_seed: u64,
}

/// Generates a PNG image encoding the payload text, returning full debug info.
pub fn generate_image_debug(params: GenerateImageParams<'_>) -> Option<GenerateDebug> {
    let payload_bytes = match pack_payload(params.payload_type, params.payload) {
        Ok(b) => b,
        Err(_) => return None,
    };
    let encoded_data = match reed_solomon_encode(payload_bytes) {
        Ok(data) => data,
        Err(_) => return None,
    };

    Some(create_image_from_binary_data(
        &encoded_data,
        params.width_ratio,
        params.height_ratio,
        params.style,
        argb_to_rgba(params.fg_color, true),
        argb_to_rgba(params.bg_color, false),
        params.horizon_seed,
    ))
}

/// For horizon style there is notation introduced, so each triangle is divided into:
///
/// ```text
///                 / \
///                /   \
///                -----
///              / \   / \
///             /   \ /   \
///            /           \
///           /     / \     \
///          /     /   \     \
///         /     /     \     \
///        /     /       \     \
///       /     /         \     \
///      /     /           \     \
///     /     /             \     \
///     ----- --------------- -----
///   / \   /                 \   / \
///  /   \ /                   \ /   \
///  ----- --------------------- -----
/// ```
///
/// And those regions stay in context of base (so left tip may not be left on the image, if triangle
/// is pointing down).
///
/// Those region are related to surrounding triangles, the interesting X trinalge is our context:
///
/// ```text
///  ------- -------
///  \     /.\     /
///   \   /...\   /
///    \ /..X..\ /
///      -------
///      \     /
///       \   /
///        \ /
/// ```
///
/// Creates a PNG image from binary data maintaining the specified aspect ratio.
/// Binary data is encoded as: 0 bits = background color, 1 bits = foreground color.
/// Each bit is represented by a 4x4 square of pixels.
/// Holds pre-computed constants for triangle rendering, avoiding repeated calculations in loops
struct TriangleRenderConstants {
    w: f64,
    center_x: f64,
    denom: f64,
    slope: f64,
}

impl TriangleRenderConstants {
    fn new(pixels_per_bit_width: u32, pixels_per_bit_height: u32) -> Self {
        let w = pixels_per_bit_width as f64;
        let h = pixels_per_bit_height as f64;
        Self {
            w,
            center_x: (w - 1.0) / 2.0,
            denom: (h - 1.0).max(1.0),
            slope: 2.0 * h / w,
        }
    }
}

/// Pre-computed corner tip positions for triangle rendering
struct TriangleCorners {
    left_tip_x: u32,
    left_tip_y: u32,
    right_tip_x: u32,
    right_tip_y: u32,
}

impl TriangleCorners {
    fn new(
        pointing_down: bool,
        pixels_per_bit_width: u32,
        pixels_per_bit_height: u32,
        border_thickness: f64,
    ) -> Self {
        if pointing_down {
            Self {
                left_tip_x: pixels_per_bit_width - border_thickness as u32,
                left_tip_y: 0,
                right_tip_x: border_thickness as u32,
                right_tip_y: 0,
            }
        } else {
            Self {
                left_tip_x: border_thickness as u32,
                left_tip_y: pixels_per_bit_height,
                right_tip_x: pixels_per_bit_width - border_thickness as u32,
                right_tip_y: pixels_per_bit_height,
            }
        }
    }
}

/// Pre-computed neighbor information for Horizon style rendering
struct NeighborInfo {
    bottom: bool,
    left: bool,
    right: bool,
}

impl NeighborInfo {
    fn compute(
        has_group: bool,
        pointing_down: bool,
        square_x: u32,
        square_y: u32,
        width_squares: u32,
        height_squares: u32,
        horizon_layout: &Option<HorizonLayout>,
    ) -> Self {
        if !has_group {
            return Self {
                bottom: false,
                left: false,
                right: false,
            };
        }

        let current_group = horizon_layout
            .as_ref()
            .and_then(|hl| hl.group_id[square_y as usize][square_x as usize]);

        let bottom = Self::check_bottom_neighbor(
            pointing_down,
            square_y,
            square_x,
            height_squares,
            current_group,
            horizon_layout,
        );

        let left = Self::check_left_neighbor(
            pointing_down,
            square_x,
            square_y,
            width_squares,
            current_group,
            horizon_layout,
        );

        let right = Self::check_right_neighbor(
            pointing_down,
            square_x,
            square_y,
            width_squares,
            current_group,
            horizon_layout,
        );

        Self {
            bottom,
            left,
            right,
        }
    }

    fn check_bottom_neighbor(
        pointing_down: bool,
        square_y: u32,
        square_x: u32,
        height_squares: u32,
        current_group: Option<usize>,
        horizon_layout: &Option<HorizonLayout>,
    ) -> bool {
        let adjacent_y = if pointing_down {
            if square_y == 0 {
                return false;
            }
            square_y - 1
        } else {
            if square_y >= height_squares - 1 {
                return false;
            }
            square_y + 1
        };

        horizon_layout.as_ref().is_some_and(|hl| {
            let adjacent_group = hl.group_id[adjacent_y as usize][square_x as usize];
            current_group.is_some() && current_group == adjacent_group
        })
    }

    fn check_left_neighbor(
        pointing_down: bool,
        square_x: u32,
        square_y: u32,
        width_squares: u32,
        current_group: Option<usize>,
        horizon_layout: &Option<HorizonLayout>,
    ) -> bool {
        let adjacent_x = if pointing_down {
            if square_x >= width_squares - 1 {
                return false;
            }
            square_x + 1
        } else {
            if square_x == 0 {
                return false;
            }
            square_x - 1
        };

        horizon_layout.as_ref().is_some_and(|hl| {
            let adjacent_group = hl.group_id[square_y as usize][adjacent_x as usize];
            current_group.is_some() && current_group == adjacent_group
        })
    }

    fn check_right_neighbor(
        pointing_down: bool,
        square_x: u32,
        square_y: u32,
        width_squares: u32,
        current_group: Option<usize>,
        horizon_layout: &Option<HorizonLayout>,
    ) -> bool {
        let adjacent_x = if pointing_down {
            if square_x == 0 {
                return false;
            }
            square_x - 1
        } else {
            if square_x >= width_squares - 1 {
                return false;
            }
            square_x + 1
        };

        horizon_layout.as_ref().is_some_and(|hl| {
            let adjacent_group = hl.group_id[square_y as usize][adjacent_x as usize];
            current_group.is_some() && current_group == adjacent_group
        })
    }
}

/// Applies a 2D bitmask (tiled over the grid) to `data` via XOR.
/// Each bit at grid position `(row, col)` is flipped when `mask[row % mask_rows][col % mask_cols] == 1`.
fn apply_mask(data: &[u8], mask: &[&[u8]], width_squares: u32) -> Vec<u8> {
    let mask_rows = mask.len();
    let mask_cols = mask[0].len();
    let mut result = data.to_vec();

    for bit_pos in 0..data.len() * 8 {
        let row = (bit_pos / width_squares as usize) % mask_rows;
        let col = (bit_pos % width_squares as usize) % mask_cols;

        if mask[row][col] == 1 {
            let byte_idx = bit_pos / 8;
            let bit_offset = 7 - (bit_pos % 8); // MSB first, matching renderer
            result[byte_idx] ^= 1 << bit_offset;
        }
    }

    result
}

/// Predefined bitmasks, tiled over the grid. Shared by encoding and decoding.
#[rustfmt::skip]
static MASKS: &[&[&[u8]]] = &[
    // Checkerboard (alternating per triangle)
    &[
        &[1, 0, 1, 0],
        &[0, 1, 0, 1],
        &[1, 0, 1, 0],
        &[0, 1, 0, 1],
    ],
    // 2×2 blocks
    &[
        &[1, 1, 0, 0],
        &[1, 1, 0, 0],
        &[0, 0, 1, 1],
        &[0, 0, 1, 1],
    ],
    // Right wave
    &[
        &[1, 0, 1, 1],
        &[0, 1, 1, 0],
        &[1, 1, 0, 1],
        &[1, 0, 1, 1],
    ],
    // Left wave
    &[
        &[1, 1, 0, 1],
        &[0, 1, 1, 0],
        &[1, 0, 1, 1],
        &[1, 1, 0, 1],
    ],
    // Vertical lines
    &[
        &[1, 0, 1, 0],
        &[1, 0, 1, 0],
        &[1, 0, 1, 0],
        &[1, 0, 1, 0],
    ],
    // Horizontal lines
    &[
        &[1, 1, 1, 1],
        &[0, 0, 0, 0],
        &[1, 1, 1, 1],
        &[0, 0, 0, 0],
    ],
    // Empty pattern, to check original data as well
    &[
        &[0, 0, 0, 0],
        &[0, 0, 0, 0],
        &[0, 0, 0, 0],
        &[0, 0, 0, 0],
    ],
];

/// XORs `encoded_data` with each predefined mask (tiled to cover the grid), then
/// returns the result whose ratio of 1-bits to 0-bits is closest to 1
/// (i.e. the most balanced bit distribution).
fn mix_pixels(encoded_data: &[u8], width_squares: u32, _height_squares: u32) -> Vec<u8> {
    let total_bits = encoded_data.len() * 8;

    MASKS
        .iter()
        .map(|mask| apply_mask(encoded_data, mask, width_squares))
        .min_by_key(|mixed| {
            let ones = mixed.iter().map(|b| b.count_ones() as i64).sum::<i64>();
            let zeros = total_bits as i64 - ones;
            // Minimise |ones - zeros| → ratio ones/zeros closest to 1
            (ones - zeros).abs()
        })
        .unwrap_or_else(|| encoded_data.to_vec())
}

/// Reverse of [`mix_pixels`]: yields `data` unmixed with every predefined mask.
/// The mask used when encoding isn't stored, so callers try each candidate
/// until Reed-Solomon decoding succeeds.
fn unmix_pixels(data: &[u8], width_squares: u32) -> impl Iterator<Item = Vec<u8>> + '_ {
    MASKS
        .iter()
        .map(move |mask| apply_mask(data, mask, width_squares))
}

fn create_image_from_binary_data(
    encoded_data: &[u8],
    width_ratio: u32,
    height_ratio: u32,
    style: ImageStyle,
    fg: Rgba<u8>,
    bg: Rgba<u8>,
    horizon_seed: u64,
) -> GenerateDebug {
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

    let data = mix_pixels(encoded_data, width_squares, height_squares);

    // Now that we know the grid size, build the Horizon layout if needed.
    let horizon_layout: Option<HorizonLayout> = if style == ImageStyle::Horizon {
        Some(HorizonLayout::from_seed(
            height_squares,
            width_squares,
            horizon_seed,
            &data,
        ))
    } else {
        None
    };

    // Calculate padding
    let padding_x = PIXELS_PER_BIT_WIDTH / 2;
    let padding_y = pixels_per_bit_height / 2;

    // Actual pixel dimensions (including padding)
    let width_without_padding = width_squares.div_ceil(2) * PIXELS_PER_BIT_WIDTH;
    let height_without_padding = height_squares * pixels_per_bit_height;
    let width = width_without_padding + (padding_x * 2);
    let height = height_without_padding + (padding_y * 2);

    let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(width, height);

    // Fill with background color
    for pixel in img.pixels_mut() {
        *pixel = bg;
    }

    // Pre-compute border thickness once
    let border_thickness: f64 = match style {
        ImageStyle::Horizon => 3.0,
        ImageStyle::Tiles => 1.0,
        ImageStyle::Stain => 0.0,
    };

    // Pre-compute triangle rendering constants once
    let tri_constants = TriangleRenderConstants::new(PIXELS_PER_BIT_WIDTH, pixels_per_bit_height);

    // Fill image with encoded data bits (each bit as a triangle)
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
                if byte_index < data.len() {
                    let bit_offset = 7 - (bit_index % 8); // MSB first
                    let bit_value = (data[byte_index] >> bit_offset) & 1;
                    bit_index += 1;
                    bit_value
                } else {
                    0 // Fill remaining squares with 0 (white)
                }
            };

            // 0 = background, 1 = foreground
            let rgba = if bit == 1 { fg } else { bg };

            if bit == 1 {
                // Determine triangle orientation once per square
                let mut pointing_down = square_x % 2 == 0;
                if square_y % 2 == 1 {
                    pointing_down = !pointing_down;
                }

                // Pre-compute corner tip positions once per triangle
                let corners = TriangleCorners::new(
                    pointing_down,
                    PIXELS_PER_BIT_WIDTH,
                    pixels_per_bit_height,
                    border_thickness,
                );

                // Pre-compute neighbor info once for Horizon style
                let neighbors = if style == ImageStyle::Horizon {
                    let has_group = horizon_layout.as_ref().is_some_and(|hl| {
                        hl.group_id[square_y as usize][square_x as usize].is_some()
                    });
                    NeighborInfo::compute(
                        has_group,
                        pointing_down,
                        square_x,
                        square_y,
                        width_squares,
                        height_squares,
                        &horizon_layout,
                    )
                } else {
                    NeighborInfo {
                        bottom: false,
                        left: false,
                        right: false,
                    }
                };

                // Check if we have a group for Horizon style (computed once)
                let has_group = style == ImageStyle::Horizon
                    && horizon_layout.as_ref().is_some_and(|hl| {
                        hl.group_id[square_y as usize][square_x as usize].is_some()
                    });

                // Render triangle pixel by pixel
                for dy in 0..pixels_per_bit_height {
                    let y = dy as f64;
                    let t = y / tri_constants.denom;

                    let half_width = if pointing_down {
                        (tri_constants.w / 2.0) * (1.0 - t)
                    } else {
                        (tri_constants.w / 2.0) * t
                    };

                    // Pre-compute border checks for this row
                    let is_top_tip = if pointing_down {
                        dy >= pixels_per_bit_height - border_thickness as u32
                    } else {
                        dy < border_thickness as u32
                    };

                    let is_base_region = if pointing_down {
                        dy < border_thickness as u32
                    } else {
                        dy >= pixels_per_bit_height - border_thickness as u32
                    };

                    for dx in 0..PIXELS_PER_BIT_WIDTH {
                        let x = dx as f64;

                        if (x - tri_constants.center_x).abs() <= half_width {
                            let base_x = square_x * (PIXELS_PER_BIT_WIDTH / 2);
                            let px = padding_x + base_x + dx;
                            let py = padding_y + square_y * pixels_per_bit_height + dy;

                            // Determine which sides the pixel belongs to
                            let (left_edge, right_edge) = if pointing_down {
                                (
                                    tri_constants.center_x + half_width,
                                    tri_constants.center_x - half_width,
                                )
                            } else {
                                (
                                    tri_constants.center_x - half_width,
                                    tri_constants.center_x + half_width,
                                )
                            };

                            let is_left_border = if pointing_down {
                                left_edge - x <= border_thickness
                            } else {
                                x - left_edge <= border_thickness
                            };

                            let is_right_border = if pointing_down {
                                x - right_edge <= border_thickness
                            } else {
                                right_edge - x <= border_thickness
                            };

                            let is_border = is_base_region || is_left_border || is_right_border;

                            let is_left_tip = if pointing_down {
                                y < tri_constants.slope * (x - corners.left_tip_x as f64)
                                    + corners.left_tip_y as f64
                                    - 1.0
                            } else {
                                y >= tri_constants.slope * (x - corners.left_tip_x as f64)
                                    + corners.left_tip_y as f64
                            };

                            let is_right_tip = if pointing_down {
                                y <= -tri_constants.slope * (x - corners.right_tip_x as f64)
                                    + corners.right_tip_y as f64
                            } else {
                                y >= -tri_constants.slope * (x - corners.right_tip_x as f64)
                                    + corners.right_tip_y as f64
                                    - 1.0
                            };

                            let is_top = is_right_border && is_left_border && !is_top_tip;
                            let is_left = is_left_border && is_base_region && !is_left_tip;
                            let is_right = is_right_border && is_base_region && !is_right_tip;
                            let is_center = !is_border;
                            let is_left_segment =
                                is_left_border && !is_right_border && !is_base_region;
                            let is_right_segment =
                                is_right_border && !is_left_border && !is_base_region;
                            let is_bottom_segment =
                                is_base_region && !is_left_border && !is_right_border;

                            // Determine if we should draw this pixel
                            let should_draw = match style {
                                ImageStyle::Horizon => {
                                    is_center
                                        || (has_group
                                            && ((neighbors.left && is_left_segment)
                                                || (neighbors.right && is_right_segment)
                                                || (neighbors.left && neighbors.right && is_top)
                                                || (neighbors.bottom && is_bottom_segment)
                                                || (neighbors.left && neighbors.bottom && is_left)
                                                || (neighbors.right
                                                    && neighbors.bottom
                                                    && is_right)))
                                }
                                _ => border_thickness == 0.0 || !is_border,
                            };

                            if should_draw {
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
        return GenerateDebug {
            png_bytes: Vec::new(),
            encoded_bytes: encoded_data.len(),
            grid_rows: height_squares,
            grid_columns: width_squares,
            data_bits_used: bit_index,
            padding_bits: 0,
        };
    }

    let grid_cells = (width_squares * height_squares) as usize;
    let orientation_cells = 8;
    let data_capacity = grid_cells.saturating_sub(orientation_cells);
    let padding_bits = data_capacity.saturating_sub(bit_index);

    GenerateDebug {
        png_bytes,
        encoded_bytes: encoded_data.len(),
        grid_rows: height_squares,
        grid_columns: width_squares,
        data_bits_used: bit_index,
        padding_bits,
    }
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
    pub content_type: String,  // "text", "url", "plain" (fallback), or empty on failure
    pub error: Option<String>, // Some on failure, None on success
    pub image_width: u32,
    pub image_height: u32,
    pub total_triangles: usize,
    pub row_count: usize,
    pub column_count: usize,
    pub bit_count: usize,
    pub annotated_png: Vec<u8>,
    /// Which decode attempt succeeded (or was last tried on failure):
    /// "naive", "cluster+pitch", either with a " ±5" suffix when found in the neighbourhood, or "all failed".
    pub decode_method: String,
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

    // Find the four anchor triangles at the grid corners
    let anchors = find_anchors(&triangles).ok_or("Could not find anchor triangles")?;

    // Remove false positives: any triangle significantly larger than the biggest anchor
    // is a cluster/merge artefact and should be discarded.
    let max_anchor_area = anchors
        .all()
        .iter()
        .map(|t| {
            let w = (t.max_x - t.min_x) as f32;
            let h = (t.max_y - t.min_y) as f32;
            w * h
        })
        .fold(0.0f32, f32::max);
    let area_threshold = max_anchor_area * FALSE_POSITIVE_AREA_FACTOR;
    let before = triangles.len();
    let triangles: Vec<Triangle> = triangles
        .into_iter()
        .filter(|t| {
            let w = (t.max_x - t.min_x) as f32;
            let h = (t.max_y - t.min_y) as f32;
            w * h <= area_threshold
        })
        .collect();
    eprintln!(
        "DEBUG false-positive filter: max_anchor_area={:.0} threshold={:.0} (factor={}) removed={} remaining={}",
        max_anchor_area,
        area_threshold,
        FALSE_POSITIVE_AREA_FACTOR,
        before - triangles.len(),
        triangles.len()
    );

    eprintln!("DEBUG anchors:");
    eprintln!(
        "  top_left:     center=({:.1}, {:.1})  bbox=({}, {})–({}, {})  pointing_{}",
        anchors.top_left.center_x,
        anchors.top_left.center_y,
        anchors.top_left.min_x,
        anchors.top_left.min_y,
        anchors.top_left.max_x,
        anchors.top_left.max_y,
        if anchors.top_left.pointing_up {
            "up"
        } else {
            "down"
        }
    );
    eprintln!(
        "  top_right:    center=({:.1}, {:.1})  bbox=({}, {})–({}, {})  pointing_{}",
        anchors.top_right.center_x,
        anchors.top_right.center_y,
        anchors.top_right.min_x,
        anchors.top_right.min_y,
        anchors.top_right.max_x,
        anchors.top_right.max_y,
        if anchors.top_right.pointing_up {
            "up"
        } else {
            "down"
        }
    );
    eprintln!(
        "  bottom_left:  center=({:.1}, {:.1})  bbox=({}, {})–({}, {})  pointing_{}",
        anchors.bottom_left.center_x,
        anchors.bottom_left.center_y,
        anchors.bottom_left.min_x,
        anchors.bottom_left.min_y,
        anchors.bottom_left.max_x,
        anchors.bottom_left.max_y,
        if anchors.bottom_left.pointing_up {
            "up"
        } else {
            "down"
        }
    );
    eprintln!(
        "  bottom_right: center=({:.1}, {:.1})  bbox=({}, {})–({}, {})  pointing_{}",
        anchors.bottom_right.center_x,
        anchors.bottom_right.center_y,
        anchors.bottom_right.min_x,
        anchors.bottom_right.min_y,
        anchors.bottom_right.max_x,
        anchors.bottom_right.max_y,
        if anchors.bottom_right.pointing_up {
            "up"
        } else {
            "down"
        }
    );

    // Count rows and columns — naive method first, new methods as fallback.
    let row_naive = calculate_row_count(&anchors);
    let col_naive = calculate_column_count(&anchors);

    // Draw rectangles on the annotated image
    let mut annotated_img = img.clone();
    draw_rectangles(&mut annotated_img, &triangles, &anchors);

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

    let ctx = GridContext {
        triangles: &triangles,
        anchors: &anchors,
        row_naive,
        col_naive,
    };

    let base = DecodeDebug {
        text: None,
        content_type: String::new(),
        error: None,
        image_width: gray_img.width(),
        image_height: gray_img.height(),
        total_triangles: triangles.len(),
        row_count: row_naive,
        column_count: col_naive,
        bit_count: 0,
        annotated_png,
        decode_method: String::new(),
    };

    // Each strategy yields grid estimates. Every estimate is tried as-is, then
    // its ±GRID_SEARCH_RADIUS neighbourhood (closest first). Already-tried grids are skipped.
    let mut tried: HashSet<(usize, usize)> = HashSet::new();
    let mut failures: Vec<String> = Vec::new();

    for strategy in GRID_STRATEGIES {
        let mut attempted = 0usize;
        let mut last_err = String::from("no new candidates");

        for estimate in (strategy.estimates)(&ctx) {
            for (rows, cols) in with_neighbourhood(estimate) {
                if rows < 2 || cols < 2 || !tried.insert((rows, cols)) {
                    continue;
                }
                attempted += 1;
                let (bits, bit_count) = read_bits_from_grid(&gray_img, rows, cols, &anchors);
                match try_unmix_decode(&bits, cols) {
                    Ok((kind, text)) => {
                        let method = if (rows, cols) == estimate {
                            strategy.name.to_string()
                        } else {
                            format!("{} ±{}", strategy.name, GRID_SEARCH_RADIUS)
                        };
                        eprintln!("DEBUG '{}' succeeded: rows={} cols={}", method, rows, cols);
                        return Ok(DecodeDebug {
                            text: Some(text),
                            content_type: kind,
                            row_count: rows,
                            column_count: cols,
                            bit_count,
                            decode_method: method,
                            ..base
                        });
                    }
                    Err(e) => last_err = e,
                }
            }
        }
        failures.push(format!(
            "{} ({} grids tried): {}",
            strategy.name, attempted, last_err
        ));
    }

    // All strategies failed — report using the cluster+pitch estimate.
    let (rows, cols) = cluster_pitch_estimate(&ctx);
    let (_, bit_count) = read_bits_from_grid(&gray_img, rows, cols, &anchors);
    Ok(DecodeDebug {
        error: Some(format!("All attempts failed.\n{}", failures.join("\n"))),
        row_count: rows,
        column_count: cols,
        bit_count,
        decode_method: "all failed".to_string(),
        ..base
    })
}

/// Inputs shared by all grid-estimation strategies.
struct GridContext<'a> {
    triangles: &'a [Triangle],
    anchors: &'a Anchors,
    row_naive: usize,
    col_naive: usize,
}

/// A named way of estimating the grid size. Returns `(rows, cols)` estimates, most likely first.
struct GridStrategy {
    name: &'static str,
    estimates: fn(&GridContext) -> Vec<(usize, usize)>,
}

/// Strategies in the order they are tried.
static GRID_STRATEGIES: &[GridStrategy] = &[
    GridStrategy {
        name: "naive",
        estimates: naive_estimates,
    },
    GridStrategy {
        name: "cluster+pitch",
        estimates: cluster_pitch_estimates,
    },
];

/// How far (in rows and columns) around each estimate the decoder searches.
const GRID_SEARCH_RADIUS: isize = 5;

fn naive_estimates(ctx: &GridContext) -> Vec<(usize, usize)> {
    vec![(ctx.row_naive, ctx.col_naive)]
}

/// Cluster + pitch estimates, clamped to naive ± 1.
fn cluster_pitch_estimate(ctx: &GridContext) -> (usize, usize) {
    let row_raw = count_rows_by_clustering(ctx.triangles);
    let (col_raw, pitch_px) = count_columns_by_pitch(ctx.triangles, ctx.anchors);
    let rows = row_raw.clamp(ctx.row_naive.saturating_sub(1), ctx.row_naive + 1);
    let cols = col_raw.clamp(ctx.col_naive.saturating_sub(1), ctx.col_naive + 1);
    eprintln!(
        "DEBUG cluster+pitch: rows={} (raw {}) cols={} (raw {}, pitch={:.1}px)",
        rows, row_raw, cols, col_raw, pitch_px
    );
    (rows, cols)
}

fn cluster_pitch_estimates(ctx: &GridContext) -> Vec<(usize, usize)> {
    vec![cluster_pitch_estimate(ctx)]
}

/// `estimate` itself followed by every grid within ±GRID_SEARCH_RADIUS of it, closest first.
fn with_neighbourhood(estimate: (usize, usize)) -> Vec<(usize, usize)> {
    let (rows, cols) = estimate;
    let mut offsets: Vec<(isize, isize)> = (-GRID_SEARCH_RADIUS..=GRID_SEARCH_RADIUS)
        .flat_map(|dr| (-GRID_SEARCH_RADIUS..=GRID_SEARCH_RADIUS).map(move |dc| (dr, dc)))
        .collect();
    offsets.sort_by_key(|&(dr, dc)| dr.abs().max(dc.abs()));
    offsets
        .into_iter()
        .filter_map(|(dr, dc)| {
            let r = usize::try_from(rows as isize + dr).ok()?;
            let c = usize::try_from(cols as isize + dc).ok()?;
            Some((r, c))
        })
        .collect()
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
        if let Some(mean) = clusters.last_mut()
            && (y - *mean).abs() <= tolerance
        {
            // Blend y into the running mean (simple online mean).
            // We use the cluster index to keep a proper average.
            // Since we only need the count, a running mean is fine.
            *mean = (*mean + y) / 2.0;
            continue;
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
fn count_columns_by_pitch(triangles: &[Triangle], anchors: &Anchors) -> (usize, f32) {
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
        if let Some((mean_y, xs)) = rows.last_mut()
            && (t.center_y - *mean_y).abs() <= row_tol
        {
            xs.push(t.center_x);
            *mean_y = (*mean_y + t.center_y) / 2.0;
            continue;
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

    // Derive column count from the top anchor horizontal span.
    let min_cx = anchors.top_left.center_x.min(anchors.bottom_left.center_x);
    let max_cx = anchors
        .top_right
        .center_x
        .max(anchors.bottom_right.center_x);
    let span = (max_cx - min_cx).abs();
    let col_count = (span / pitch).round() as usize + 1;

    (col_count, pitch)
}

fn calculate_row_count(anchors: &Anchors) -> usize {
    // Use top-left and bottom-left which are on the same column — most reliable vertical span.
    let top = &anchors.top_left;
    let bottom = &anchors.bottom_left;

    let vertical_distance = ((top.min_y as f32) - (bottom.max_y as f32)).abs();
    let height1 = (top.max_y - top.min_y) as f32;
    let height2 = (bottom.max_y - bottom.min_y) as f32;
    let avg_height = (height1 + height2) / 2.0;

    if avg_height > 0.0 {
        let intervals = (vertical_distance / avg_height).round() as usize;
        if intervals > 0 { intervals } else { 1 }
    } else {
        0
    }
}

/// Calculate the number of columns by clustering triangles into rows
/// and taking the maximum count across rows.
fn calculate_column_count(anchors: &Anchors) -> usize {
    // Estimate triangle width from anchors only
    let mut widths: Vec<f32> = anchors
        .all()
        .iter()
        .map(|t| (t.max_x - t.min_x) as f32)
        .collect();
    widths.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median_width = widths[widths.len() / 2].max(1.0);

    // Use top-left and top-right anchors directly — they are explicitly the correct pair.
    let left = &anchors.top_left;
    let right = &anchors.top_right;

    // M is the number of triangle widths between corners
    let distance = (right.center_x - left.center_x).abs();
    let m = (distance / median_width).round() as usize;

    eprintln!(
        "DEBUG col_count: left_cx={:.1} right_cx={:.1} distance={:.1} median_width={:.1} m={} => cols={}",
        left.center_x,
        right.center_x,
        distance,
        median_width,
        m,
        if m < 2 { 0 } else { 2 * (m - 2) + 1 }
    );

    if m < 2 {
        return 0;
    }
    2 * (m - 2) + 1
}

/// Select triangles from the 4 corners of the image
/// Only considers black triangles closest to each image corner
/// The four corner anchor triangles of a tricode grid.
#[derive(Debug, Clone)]
pub struct Anchors {
    pub top_left: Triangle,
    pub top_right: Triangle,
    pub bottom_left: Triangle,
    pub bottom_right: Triangle,
}

impl Anchors {
    /// Returns all four anchors as a slice-compatible array for iteration.
    pub fn all(&self) -> [&Triangle; 4] {
        [
            &self.top_left,
            &self.top_right,
            &self.bottom_left,
            &self.bottom_right,
        ]
    }
}

/// Find the four corner anchor triangles of the grid.
fn find_anchors(triangles: &[Triangle]) -> Option<Anchors> {
    // Filter for only black triangles
    let black_triangles: Vec<_> = triangles.iter().filter(|t| t.is_black).cloned().collect();

    if black_triangles.is_empty() {
        return None;
    }

    // Get image dimensions from triangles
    let max_x = black_triangles.iter().map(|t| t.max_x).max().unwrap_or(0);
    let max_y = black_triangles.iter().map(|t| t.max_y).max().unwrap_or(0);

    // Define the 4 image corners: [top-left, top-right, bottom-left, bottom-right]
    let image_corners: [(f32, f32); 4] = [
        (0.0, 0.0),                   // top-left
        (max_x as f32, 0.0),          // top-right
        (0.0, max_y as f32),          // bottom-left
        (max_x as f32, max_y as f32), // bottom-right
    ];

    let mut selected: [Option<Triangle>; 4] = [None, None, None, None];
    let mut min_distances: [f32; 4] = [f32::MAX; 4];

    for triangle in &black_triangles {
        let center_x = (triangle.min_x as f32 + triangle.max_x as f32) / 2.0;
        let center_y = (triangle.min_y as f32 + triangle.max_y as f32) / 2.0;

        for (idx, &(corner_x, corner_y)) in image_corners.iter().enumerate() {
            let dist = (center_x - corner_x).powi(2) + (center_y - corner_y).powi(2);
            if dist < min_distances[idx] {
                min_distances[idx] = dist;
                selected[idx] = Some(triangle.clone());
            }
        }
    }

    let [
        Some(top_left),
        Some(top_right),
        Some(bottom_left),
        Some(bottom_right),
    ] = selected
    else {
        return None;
    };

    Some(Anchors {
        top_left,
        top_right,
        bottom_left,
        bottom_right,
    })
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

/// Tries every unmixed variant of `bits` until Reed-Solomon decoding and
/// payload unpacking succeed.
fn try_unmix_decode(bits: &[u8], width_squares: usize) -> Result<(String, String), String> {
    let mut last_err = "no masks to try".to_string();
    for candidate in unmix_pixels(bits, width_squares as u32) {
        match reed_solomon_decode(candidate).and_then(|d| unpack_payload(&d)) {
            Ok(result) => return Ok(result),
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

/// Read bits from the image by sampling the pixel color at the center of each
/// grid cell.  The orientation-mark cells (first/last row, first 2 and last 2
/// columns) are skipped — they are not data bits.  Returns the data bits packed
/// into bytes (MSB first).
fn read_bits_from_grid(
    img: &GrayImage,
    row_count: usize,
    column_count: usize,
    anchors: &Anchors,
) -> (Vec<u8>, usize) {
    if row_count < 2 || column_count < 2 {
        return (Vec::new(), 0);
    }

    // Convert pixel centroids → bounding-box cell centers using the named anchor fields.
    let (tl_cx, tl_cy) = centroid_to_bbox_center(&anchors.top_left);
    let (tr_cx, _tr_cy) = centroid_to_bbox_center(&anchors.top_right);
    let (_bl_cx, bl_cy) = centroid_to_bbox_center(&anchors.bottom_left);

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

/// Draw bounding-box rectangles on the annotated image.
/// All detected triangles are drawn in blue; the four anchors are drawn in red on top.
fn draw_rectangles(
    img: &mut ImageBuffer<Rgba<u8>, Vec<u8>>,
    triangles: &[Triangle],
    anchors: &Anchors,
) {
    let blue = Rgba([0, 0, 255, 255]);
    let red = Rgba([255, 0, 0, 255]);
    let border_width = 2u32;

    let draw_box = |img: &mut ImageBuffer<Rgba<u8>, Vec<u8>>,
                    t: &Triangle,
                    color: Rgba<u8>,
                    thickness: u32| {
        let min_x = t.min_x.min(img.width().saturating_sub(1));
        let min_y = t.min_y.min(img.height().saturating_sub(1));
        let max_x = t.max_x.min(img.width());
        let max_y = t.max_y.min(img.height());

        // Top border
        for x in min_x..max_x {
            for dy in 0..thickness {
                let y = (min_y + dy).min(img.height() - 1);
                img.put_pixel(x, y, color);
            }
        }
        // Bottom border
        for x in min_x..max_x {
            for dy in 0..thickness {
                let y = max_y.saturating_sub(1 + dy);
                img.put_pixel(x, y, color);
            }
        }
        // Left border
        for y in min_y..max_y {
            for dx in 0..thickness {
                let x = (min_x + dx).min(img.width() - 1);
                img.put_pixel(x, y, color);
            }
        }
        // Right border
        for y in min_y..max_y {
            for dx in 0..thickness {
                let x = max_x.saturating_sub(1 + dx);
                img.put_pixel(x, y, color);
            }
        }
    };

    // Draw all triangles in blue first
    for t in triangles {
        draw_box(img, t, blue, border_width);
    }
    // Draw anchors in red on top with a thicker border so they stand out
    for t in anchors.all() {
        draw_box(img, t, red, border_width * 2);
    }
}
