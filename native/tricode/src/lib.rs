/// A triangle whose bounding-box area exceeds this multiple of the largest anchor area
/// is considered a cluster/merge false positive and is discarded before grid reading.
const FALSE_POSITIVE_AREA_FACTOR: f32 = 1.2;

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

/// The width the 1 px Tiles and 3 px Horizon borders were designed at; borders scale from it.
const BORDER_REFERENCE_WIDTH: f64 = 51.0;

/// Bit box height in pixels: the height of an equilateral triangle `triangle_width` wide.
fn triangle_height(triangle_width: u32) -> u32 {
    ((triangle_width as f64) * (3.0_f64.sqrt() / 2.0)).ceil() as u32
}

/// Checks that a triangle width is odd (so each triangle is symmetric around a pixel column and
/// ends in a one-pixel tip) and within [`MIN_TRIANGLE_WIDTH`]..=[`MAX_TRIANGLE_WIDTH`].
fn validate_triangle_width(triangle_width: u32) -> Result<(), String> {
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
fn image_size(width_squares: u32, height_squares: u32, triangle_width: u32) -> (u32, u32) {
    let triangle_height = triangle_height(triangle_width);
    let width = width_squares.div_ceil(2) * triangle_width + (triangle_width / 2) * 2;
    let height = height_squares * triangle_height + (triangle_height / 2) * 2;
    (width, height)
}

/// Fails when the image would exceed [`MAX_IMAGE_PIXELS`], naming the largest width that fits.
fn check_image_pixels(
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

use image::codecs::png::PngEncoder;
use image::{
    ExtendedColorType, GrayImage, ImageBuffer, ImageEncoder, ImageFormat, ImageReader, Rgba,
    RgbaImage,
};
use imageproc::contrast::{ThresholdType, threshold};
use imageproc::distance_transform::Norm;
use imageproc::morphology::dilate;
use rayon::prelude::*;

mod reed_solomon;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Cursor;

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
/// 2. If the bytes are valid UTF-8, return them as `("plain", string)`: codes that hold bare
///    text. This comes before generic msgpack because every ASCII byte is also a msgpack integer
///    (`"abcde"` would read as `97`), while msgpack structures are rarely valid UTF-8, as their
///    headers are bytes of `0x80` and up.
/// 3. If it *is* valid msgpack, serialise the decoded value as a JSON string and return
///    `("plain", json)`.
/// 4. Only if all three steps fail do we return `Err`.
pub fn unpack_payload(bytes: &[u8]) -> Result<(String, String), String> {
    // Step 1 — try to match the expected named-map envelope.
    if let Some(envelope) = unpack_envelope(bytes) {
        return Ok(envelope);
    }

    // Step 2 — bare UTF-8 text.
    if let Ok(s) = std::str::from_utf8(bytes) {
        return Ok(("plain".to_string(), s.to_string()));
    }

    // Step 3 — valid msgpack but wrong shape: decode to a generic JSON value and
    // return the JSON representation so the caller still gets something useful.
    let mut de = rmp_serde::Deserializer::new(bytes);
    if let Ok(json_val) = serde_json::Value::deserialize(&mut de) {
        let json_str = serde_json::to_string(&json_val).unwrap_or_else(|_| json_val.to_string());
        return Ok(("plain".to_string(), json_str));
    }

    Err(format!(
        "payload is not a recognised msgpack envelope, UTF-8 text, or valid msgpack ({} bytes)",
        bytes.len()
    ))
}

/// Step 1 of [`unpack_payload`]: the `{"type": ..., "value": ...}` envelope written by
/// [`pack_payload`], as `(kind, value)`.
fn unpack_envelope(bytes: &[u8]) -> Option<(String, String)> {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(rename = "type")]
        kind: String,
        value: String,
    }

    // `to_vec_named` writes a msgpack map with string keys; we must deserialise
    // in the same named/map mode — rmp_serde's default `from_slice` uses the
    // compact array format which is incompatible with named maps.
    let mut de = rmp_serde::Deserializer::new(bytes);
    let envelope = Envelope::deserialize(&mut de).ok()?;
    Some((envelope.kind, envelope.value))
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
        let is_fg = layout_cells(encoded_data, rows, cols);

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
    pub mask_index: usize,     // index into MASKS of the mask the data was XORed with
    pub colour_transitions: usize, // shared triangle edges between cells of different colour
    pub shared_edges: usize,   // all triangle edges shared by two cells
    /// Whether the code still decodes with its overlay; `None` without one.
    pub overlay_decodes: Option<bool>,
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
    generate_image_debug(GenerateImageParams {
        payload,
        payload_type,
        width_ratio,
        height_ratio,
        style: ImageStyle::Tiles,
        fg_color: 0xFF000000,
        bg_color: 0xFFFFFFFF,
        horizon_seed: 0,
        mask: None,
        triangle_width: DEFAULT_TRIANGLE_WIDTH,
        ecc_level: DEFAULT_ECC_LEVEL,
        overlay: None,
    })
    .map(|debug| debug.png_bytes)
    .unwrap_or_default()
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
    /// Index into [`MASK_NAMES`] of the mask to XOR the data with, for comparing masks.
    /// `None` (or an out-of-range index) picks the most balanced one, as normal generation does.
    pub mask: Option<usize>,
    /// Triangle base width in pixels: odd, within [`MIN_TRIANGLE_WIDTH`]..=[`MAX_TRIANGLE_WIDTH`].
    /// Usually [`DEFAULT_TRIANGLE_WIDTH`]; larger values give a higher-resolution image.
    pub triangle_width: u32,
    /// How much damage the code survives; higher levels need a larger grid.
    pub ecc_level: EccLevel,
    /// An image drawn over the centre of the code, if any.
    pub overlay: Option<Overlay<'a>>,
}

/// An image drawn over the centre of a code, such as a logo or a line of text. The triangles under
/// it are left out, so the decoder reads them as damage that the error correction repairs.
#[derive(Debug, Clone, Copy)]
pub struct Overlay<'a> {
    /// PNG bytes of the image; its transparent parts show the background.
    pub png: &'a [u8],
    /// Share of the code's area the overlay's box takes, above 0 and at most
    /// [`EccLevel::max_overlay_share`] of the code's level.
    pub share: f64,
}

/// Largest overlay image accepted, in pixels; larger ones should be scaled down before sending.
pub const MAX_OVERLAY_PIXELS: u64 = 16_000_000;

/// Generates a PNG image encoding the payload text, returning full debug info.
pub fn generate_image_debug(params: GenerateImageParams<'_>) -> Result<GenerateDebug, String> {
    validate_triangle_width(params.triangle_width)?;
    let payload_bytes = pack_payload(params.payload_type, params.payload)?;
    let mut debug = create_image_from_payload(&payload_bytes, &params)?;
    if params.overlay.is_some() {
        // The overlay's share is kept within what the level repairs, but what it covers varies;
        // reading the code back tells for sure.
        let decoded = decode_image(&debug.png_bytes)?;
        debug.overlay_decodes = Some(decoded.text.as_deref() == Some(params.payload));
    }
    Ok(debug)
}

/// The triangle grid in pixel coordinates, shared by every cell so that neighbouring triangles
/// are cut from the same lines.
///
/// Vertices sit on pixel centres horizontally (`col_pitch` apart, so a triangle's base spans
/// `2 · col_pitch + 1` pixels: the odd triangle width) and on the boundaries between pixel rows
/// vertically, so rows split the image cleanly. Every slanted edge therefore lies on one straight
/// line across all rows, and a shared edge is rounded identically from both sides.
struct TriangleGrid {
    rows: i64,
    cols: i64,
    col_pitch: f64,
    row_pitch: f64,
    /// x of column 0's left vertex.
    origin_x: f64,
    /// y of the line above row 0.
    origin_y: f64,
}

impl TriangleGrid {
    fn new(rows: u32, cols: u32, triangle_width: u32) -> Self {
        let triangle_height = triangle_height(triangle_width);
        Self {
            rows: rows as i64,
            cols: cols as i64,
            col_pitch: (triangle_width / 2) as f64,
            row_pitch: triangle_height as f64,
            origin_x: (triangle_width / 2) as f64,
            origin_y: (triangle_height / 2) as f64 - 0.5,
        }
    }

    /// The cell containing the point, which may lie outside the grid.
    fn cell_at(&self, x: f64, y: f64) -> (i64, i64) {
        let v = (y - self.origin_y) / self.row_pitch;
        let row = v.floor() as i64;
        let t = v - row as f64; // 0 at the row's top line, 1 at its bottom line
        let u = (x - self.origin_x) / self.col_pitch;
        // Column strip k (between vertices k and k+1) holds the edge between cells k-1 and k.
        let strip = u.floor() as i64;
        let left = strip - 1;
        let edge = if points_down_i(row, left) {
            (strip + 1) as f64 - t
        } else {
            strip as f64 + t
        };
        (row, if u < edge { left } else { strip })
    }

    /// Corners of a cell's triangle.
    fn vertices(&self, row: i64, col: i64) -> [(f64, f64); 3] {
        let left = self.origin_x + col as f64 * self.col_pitch;
        let (mid, right) = (left + self.col_pitch, left + 2.0 * self.col_pitch);
        let top = self.origin_y + row as f64 * self.row_pitch;
        let bottom = top + self.row_pitch;
        if points_down_i(row, col) {
            [(left, top), (right, top), (mid, bottom)]
        } else {
            [(mid, top), (right, bottom), (left, bottom)]
        }
    }

    /// Lattice coordinates `(k, line)` of a vertex: its column of vertices and its row line.
    fn vertex_index(&self, (x, y): (f64, f64)) -> (i64, i64) {
        (
            ((x - self.origin_x) / self.col_pitch).round() as i64,
            ((y - self.origin_y) / self.row_pitch).round() as i64,
        )
    }

    fn vertex_at(&self, (k, line): (i64, i64)) -> (f64, f64) {
        (
            self.origin_x + k as f64 * self.col_pitch,
            self.origin_y + line as f64 * self.row_pitch,
        )
    }

    /// The six edges leaving a vertex, clockwise on screen from east: as offsets to the vertex at
    /// the far end, in vertex coordinates.
    const RAYS: [(i64, i64); 6] = [(2, 0), (1, 1), (-1, 1), (-2, 0), (-1, -1), (1, -1)];

    /// The six cells around a vertex; cell `i` lies clockwise of ray `i` (between rays `i` and
    /// `i + 1`).
    fn cells_around((k, line): (i64, i64)) -> [(i64, i64); 6] {
        [
            (line, k),
            (line, k - 1),
            (line, k - 2),
            (line - 1, k - 2),
            (line - 1, k - 1),
            (line - 1, k),
        ]
    }

    fn ray_vector(&self, ray: usize) -> (f64, f64) {
        let (dk, dline) = Self::RAYS[ray];
        (dk as f64 * self.col_pitch, dline as f64 * self.row_pitch)
    }

    /// Whether a point inside `(row, col)` lies on a border: within `width` of an edge between
    /// its shape and another, on its own side.
    ///
    /// Each edge's border is a strip cut off at both ends by the line halving the corner there
    /// (a mitre join, as when stroking a polygon outline). At outward corners the strips meet and
    /// cut the corner off cleanly; at inward ones they meet in a sharp point instead of the round
    /// fillet that measuring to the nearest point of another shape would give.
    fn on_border(
        &self,
        (x, y): (f64, f64),
        (row, col): (i64, i64),
        width: f64,
        shape_of: impl Fn(i64, i64) -> Option<usize>,
    ) -> bool {
        let corners = self.vertices(row, col);
        // A strip within reach has to come within `width` of this cell's own edges first.
        let near_own_edge =
            (0..3).any(|i| distance_to_segment((x, y), corners[i], corners[(i + 1) % 3]) < width);
        if !near_own_edge {
            return false;
        }

        let own = shape_of(row, col);
        let in_shape = |(r, c): (i64, i64)| shape_of(r, c) == own;
        // Every edge within reach leaves one of this cell's corners.
        corners.iter().any(|&corner| {
            let vertex = self.vertex_index(corner);
            let around = Self::cells_around(vertex);
            (0..6).any(|ray| {
                // The edge separates the shape from another when exactly one side is in it.
                let clockwise_in = in_shape(around[ray]);
                if clockwise_in == in_shape(around[(ray + 5) % 6]) {
                    return false;
                }
                let (dx, dy) = self.ray_vector(ray);
                let (px, py) = (x - corner.0, y - corner.1);
                // On the shape's side of the edge, and within `width` of its line.
                let cross = dx * py - dy * px;
                if (cross > 0.0) != clockwise_in || cross.abs() / dx.hypot(dy) >= width {
                    return false;
                }
                let far =
                    self.vertex_at((vertex.0 + Self::RAYS[ray].0, vertex.1 + Self::RAYS[ray].1));
                self.within_mitre((x, y), vertex, ray, clockwise_in, &in_shape)
                    && self.within_mitre(
                        (x, y),
                        self.vertex_index(far),
                        (ray + 3) % 6,
                        !clockwise_in,
                        &in_shape,
                    )
            })
        })
    }

    /// Whether the point lies on the edge's side of the line halving the shape's corner at
    /// `vertex`, where the edge is `ray` and the shape is on its clockwise side if `clockwise`.
    fn within_mitre(
        &self,
        (x, y): (f64, f64),
        vertex: (i64, i64),
        ray: usize,
        clockwise: bool,
        in_shape: &impl Fn((i64, i64)) -> bool,
    ) -> bool {
        let around = Self::cells_around(vertex);
        // How many 60° sectors the shape's corner spans, starting next to the edge.
        let sector = |step: usize| {
            if clockwise {
                (ray + step) % 6
            } else {
                (ray + 6 - 1 - step) % 6
            }
        };
        let span = (0..6)
            .take_while(|&step| in_shape(around[sector(step)]))
            .count();
        let other_ray = if clockwise {
            (ray + span) % 6
        } else {
            (ray + 6 - span % 6) % 6
        };
        let unit = |(dx, dy): (f64, f64)| {
            let length = dx.hypot(dy);
            (dx / length, dy / length)
        };
        let (ax, ay) = unit(self.ray_vector(ray));
        let (bx, by) = unit(self.ray_vector(other_ray));
        // The halving line's direction: through the shape for corners under 180°, out of it
        // (pointing away from both edges) for wider ones.
        let (hx, hy) = match span {
            0..=2 => (ax + bx, ay + by),
            3 => {
                if clockwise {
                    (-ay, ax)
                } else {
                    (ay, -ax)
                }
            }
            _ => (-(ax + bx), -(ay + by)),
        };
        let (vx, vy) = self.vertex_at(vertex);
        let side = |(px, py): (f64, f64)| hx * py - hy * px;
        side((x - vx, y - vy)) * side((ax, ay)) >= 0.0
    }
}

/// [`points_down`] for cell indices that may lie outside the grid.
fn points_down_i(row: i64, col: i64) -> bool {
    (row + col).rem_euclid(2) == 0
}

fn distance_to_segment((px, py): (f64, f64), (ax, ay): (f64, f64), (bx, by): (f64, f64)) -> f64 {
    let (dx, dy) = (bx - ax, by - ay);
    let t = (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    (px - ax - t * dx).hypot(py - ay - t * dy)
}

/// Which drawn shape each cell belongs to; `None` for background cells. Pixels within the border
/// width of a different shape (or of the background) are left blank, so every shape is drawn
/// shrunk by the border and separated from its neighbours.
fn cell_shapes(
    cells: &[Vec<bool>],
    style: ImageStyle,
    horizon_layout: Option<&HorizonLayout>,
) -> Vec<Vec<Option<usize>>> {
    let cols = cells.first().map_or(0, Vec::len);
    // Each cell on its own gets a unique id; joined Horizon groups share one past those.
    let singles = cells.len() * cols;
    cells
        .iter()
        .enumerate()
        .map(|(row, cells_in_row)| {
            cells_in_row
                .iter()
                .enumerate()
                .map(|(col, &is_fg)| {
                    is_fg.then(|| match style {
                        ImageStyle::Stain => 0,
                        ImageStyle::Tiles => row * cols + col,
                        ImageStyle::Horizon => horizon_layout
                            .and_then(|hl| hl.group_id[row][col])
                            .map_or(row * cols + col, |group| singles + group),
                    })
                })
                .collect()
        })
        .collect()
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

/// Human-readable names of [`MASKS`], in the same order.
pub const MASK_NAMES: [&str; 7] = [
    "Checkerboard",
    "2×2 blocks",
    "Right wave",
    "Left wave",
    "Vertical lines",
    "Horizontal lines",
    "None",
];

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

/// Orientation of the triangle in cell `(row, col)`, as the renderer draws it.
fn points_down(row: usize, col: usize) -> bool {
    (row + col).is_multiple_of(2)
}

/// Foreground (bit = 1) flag of every grid cell, laid out as the renderer draws them: orientation
/// marks in the corners of the first and last rows, then the data bits MSB first, then zero
/// padding.
fn layout_cells(data: &[u8], rows: u32, cols: u32) -> Vec<Vec<bool>> {
    let (rows, cols) = (rows as usize, cols as usize);
    let mut bits = (0..data.len() * 8).map(|i| (data[i / 8] >> (7 - i % 8)) & 1 == 1);
    (0..rows)
        .map(|row| {
            (0..cols)
                .map(|col| {
                    let is_edge_row = row == 0 || row + 1 == rows;
                    if is_edge_row && (col == 0 || col + 1 == cols) {
                        true
                    } else if is_edge_row && (col == 1 || col + 2 == cols) {
                        false
                    } else {
                        bits.next().unwrap_or(false)
                    }
                })
                .collect()
        })
        .collect()
}

/// Counts the triangle edges shared by two cells, returning `(differing, total)`: how many of them
/// separate cells of different colour, out of all of them. Every cell shares an edge with its
/// right-hand neighbour, and a pointing-down cell also shares its top edge with the cell above it.
fn colour_transitions(cells: &[Vec<bool>]) -> (usize, usize) {
    let (mut differing, mut total) = (0, 0);
    for (row, row_cells) in cells.iter().enumerate() {
        for (col, &cell) in row_cells.iter().enumerate() {
            let neighbours = [
                row_cells.get(col + 1).copied(),
                (row > 0 && points_down(row, col)).then(|| cells[row - 1][col]),
            ];
            for neighbour in neighbours.into_iter().flatten() {
                total += 1;
                if neighbour != cell {
                    differing += 1;
                }
            }
        }
    }
    (differing, total)
}

/// XORs `encoded_data` with a predefined mask (tiled to cover the grid) and returns the result
/// together with the index of the mask used. `forced` selects a mask by index; otherwise (or
/// when it's out of range) the mask giving the most colour changes between neighbouring triangles
/// is picked (see [`colour_transitions`]), which breaks the image into fine texture instead of
/// large solid patches. Ties go to the mask with the most even split of 1-bits and 0-bits.
fn mix_pixels(
    encoded_data: &[u8],
    width_squares: u32,
    height_squares: u32,
    forced: Option<usize>,
) -> (Vec<u8>, usize) {
    if let Some(index) = forced.filter(|&i| i < MASKS.len()) {
        return (apply_mask(encoded_data, MASKS[index], width_squares), index);
    }

    let total_bits = encoded_data.len() * 8;

    MASKS
        .iter()
        .map(|mask| apply_mask(encoded_data, mask, width_squares))
        .enumerate()
        .min_by_key(|(_, mixed)| {
            let (transitions, _) =
                colour_transitions(&layout_cells(mixed, height_squares, width_squares));
            let ones = mixed.iter().map(|b| b.count_ones() as i64).sum::<i64>();
            let zeros = total_bits as i64 - ones;
            (std::cmp::Reverse(transitions), (ones - zeros).abs())
        })
        .map(|(index, mixed)| (mixed, index))
        .unwrap_or_else(|| (encoded_data.to_vec(), MASKS.len() - 1))
}

/// Reverse of [`mix_pixels`]: yields `data` unmixed with every predefined mask.
/// The mask used when encoding isn't stored, so callers try each candidate
/// until Reed-Solomon decoding succeeds.
fn unmix_pixels(data: &[u8], width_squares: u32) -> impl Iterator<Item = Vec<u8>> + '_ {
    MASKS
        .iter()
        .map(move |mask| apply_mask(data, mask, width_squares))
}

/// Bytes of data a `rows × cols` grid holds: every cell but the 8 orientation marks is a bit.
fn grid_capacity(rows: usize, cols: usize) -> usize {
    (rows * cols).saturating_sub(8) / 8
}

/// The smallest grid (as `(columns, rows)`) of the requested aspect ratio whose capacity holds
/// `payload_len` bytes at `level`.
fn grid_for_payload(payload_len: usize, width: u32, height: u32, level: EccLevel) -> (u32, u32) {
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

fn create_image_from_payload(
    payload: &[u8],
    params: &GenerateImageParams<'_>,
) -> Result<GenerateDebug, String> {
    if payload.len() > u16::MAX as usize {
        return Err(format!("payload of {} bytes is too long", payload.len()));
    }
    let GenerateImageParams {
        width_ratio,
        height_ratio,
        style,
        horizon_seed,
        mask,
        triangle_width,
        ..
    } = *params;
    let fg = argb_to_rgba(params.fg_color, true);
    let bg = argb_to_rgba(params.bg_color, false);
    let overlay = params
        .overlay
        .map(|overlay| load_overlay(overlay, params.ecc_level))
        .transpose()?;

    // Calculate dimensions maintaining the aspect ratio and ensuring enough bit squares
    let gcd_val = gcd(width_ratio, height_ratio);
    let normalized_width = width_ratio / gcd_val;
    let normalized_height = height_ratio / gcd_val;
    let (width_squares, height_squares) = grid_for_payload(
        payload.len(),
        normalized_width,
        normalized_height,
        params.ecc_level,
    );
    check_image_pixels(width_squares, height_squares, triangle_width)?;

    // The codewords fill every whole byte of the grid. The few cells left over after the last
    // byte are zero-padded *before* mixing, so the mask also covers them instead of leaving them
    // blank.
    let capacity = grid_capacity(height_squares as usize, width_squares as usize);
    let encoded_data = encode_frame(payload, capacity, params.ecc_level)?;
    let data_capacity = ((width_squares * height_squares) as usize).saturating_sub(8);
    let data_bits_used = encoded_data.len() * 8;
    let padding_bits = data_capacity - data_bits_used;
    let mut padded = encoded_data.clone();
    padded.resize(data_capacity.div_ceil(8), 0);

    let (data, mask_index) = mix_pixels(&padded, width_squares, height_squares, mask);
    let (colour_transitions, shared_edges) =
        colour_transitions(&layout_cells(&data, height_squares, width_squares));

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

    // Actual pixel dimensions (including padding)
    let (width, height) = image_size(width_squares, height_squares, triangle_width);

    let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_pixel(width, height, bg);

    // Border width on each side of an edge, measured perpendicular to it. It scales with the
    // triangle (never below 1 px), so a larger triangle width enlarges the whole image rather than
    // thinning the borders.
    let border_at_reference: f64 = match style {
        ImageStyle::Horizon => 3.0,
        ImageStyle::Tiles => 1.0,
        ImageStyle::Stain => 0.0,
    };
    let border = if border_at_reference == 0.0 {
        0.0
    } else {
        (border_at_reference * triangle_width as f64 / BORDER_REFERENCE_WIDTH)
            .round()
            .max(1.0)
    };

    let grid = TriangleGrid::new(height_squares, width_squares, triangle_width);
    let mut shapes = cell_shapes(
        &layout_cells(&data, height_squares, width_squares),
        style,
        horizon_layout.as_ref(),
    );
    let overlay_box = overlay
        .as_ref()
        .map(|(image, share)| overlay_box(image, *share, width, height));
    if let Some(overlay_box) = &overlay_box {
        clear_cells_under(&mut shapes, &grid, overlay_box);
    }
    let shape_of = |row: i64, col: i64| -> Option<usize> {
        if row < 0 || col < 0 || row >= grid.rows || col >= grid.cols {
            return None;
        }
        shapes[row as usize][col as usize]
    };

    // Whether a point is painted: inside a shape and off its borders.
    let painted = |x: f64, y: f64| -> bool {
        let (row, col) = grid.cell_at(x, y);
        shape_of(row, col).is_some()
            && !(border > 0.0 && grid.on_border((x, y), (row, col), border, shape_of))
    };

    // Anti-aliasing: a pixel whose corners and centre all agree is taken as fully painted or
    // not; one the outline crosses is sampled on a grid and blended by the share painted. Corners
    // are shared between neighbouring pixels, so they are computed a row at a time, and bands of
    // rows are rendered in parallel.
    let corner_row =
        |y: f64| -> Vec<bool> { (0..=width).map(|px| painted(px as f64 - 0.5, y)).collect() };
    let coverage = |x: f64, y: f64, corners: [bool; 4]| -> f64 {
        let centre = painted(x, y);
        if corners.iter().all(|&c| c == centre) {
            return if centre { 1.0 } else { 0.0 };
        }
        let offset = |i: u32| (i as f64 + 0.5) / AA_SAMPLES as f64 - 0.5;
        let hits = (0..AA_SAMPLES)
            .flat_map(|i| (0..AA_SAMPLES).map(move |j| (i, j)))
            .filter(|&(i, j)| painted(x + offset(i), y + offset(j)))
            .count();
        hits as f64 / (AA_SAMPLES * AA_SAMPLES) as f64
    };
    let row_bytes = width as usize * 4;
    img.par_chunks_mut(row_bytes * BAND_ROWS)
        .enumerate()
        .for_each(|(band, rows)| {
            let first_row = band * BAND_ROWS;
            let mut above = corner_row(first_row as f64 - 0.5);
            for (i, row) in rows.chunks_mut(row_bytes).enumerate() {
                let y = (first_row + i) as f64;
                let below = corner_row(y + 0.5);
                for (px, pixel) in row.chunks_mut(4).enumerate() {
                    let corners = [above[px], above[px + 1], below[px], below[px + 1]];
                    let painted_share = coverage(px as f64, y, corners);
                    if painted_share > 0.0 {
                        pixel.copy_from_slice(&mix(bg, fg, painted_share).0);
                    }
                }
                above = below;
            }
        });

    if let (Some((image, _)), Some(overlay_box)) = (&overlay, &overlay_box) {
        draw_overlay(&mut img, image, overlay_box, triangle_width);
    }

    let mut png_bytes = Vec::new();
    PngEncoder::new(&mut png_bytes)
        .write_image(img.as_raw(), width, height, ExtendedColorType::Rgba8)
        .map_err(|e| format!("Failed to encode PNG: {}", e))?;

    Ok(GenerateDebug {
        png_bytes,
        encoded_bytes: encoded_data.len(),
        grid_rows: height_squares,
        grid_columns: width_squares,
        data_bits_used,
        padding_bits,
        mask_index,
        colour_transitions,
        shared_edges,
        overlay_decodes: None,
    })
}

/// Decodes an overlay's image and checks its share against what `level` repairs.
fn load_overlay(overlay: Overlay<'_>, level: EccLevel) -> Result<(RgbaImage, f64), String> {
    let max_share = level.max_overlay_share();
    if !(overlay.share > 0.0 && overlay.share <= max_share) {
        return Err(format!(
            "Overlay must take between 0% and {:.0}% of a {} level code, got {:.1}%",
            max_share * 100.0,
            level.name(),
            overlay.share * 100.0
        ));
    }
    let reader = ImageReader::with_format(Cursor::new(overlay.png), ImageFormat::Png);
    let (w, h) = reader
        .into_dimensions()
        .map_err(|e| format!("Failed to read overlay image: {}", e))?;
    if w as u64 * h as u64 > MAX_OVERLAY_PIXELS {
        return Err(format!(
            "Overlay image of {}×{} px is larger than {} MP",
            w,
            h,
            MAX_OVERLAY_PIXELS / 1_000_000
        ));
    }
    let image = image::load_from_memory_with_format(overlay.png, ImageFormat::Png)
        .map_err(|e| format!("Failed to read overlay image: {}", e))?
        .to_rgba8();
    Ok((image, overlay.share))
}

/// A rectangle in pixels: left, top, right and bottom edges.
#[derive(Debug, Clone, Copy)]
struct PixelBox {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

/// Largest share of the code's width or height the overlay box spans, so a very wide or tall
/// overlay can't reach the corner anchors.
const MAX_OVERLAY_SPAN: f64 = 0.8;

/// The box an overlay is drawn in: centred, `share` of the code's area, with the image's aspect
/// ratio (narrowed to [`MAX_OVERLAY_SPAN`] of the code when it is very wide or tall).
fn overlay_box(image: &RgbaImage, share: f64, width: u32, height: u32) -> PixelBox {
    let (width, height) = (width as f64, height as f64);
    let aspect = image.width().max(1) as f64 / image.height().max(1) as f64;
    let area = share * width * height;
    let box_width = (area * aspect).sqrt().min(width * MAX_OVERLAY_SPAN);
    let box_height = (area / aspect).sqrt().min(height * MAX_OVERLAY_SPAN);
    PixelBox {
        left: (width - box_width) / 2.0,
        top: (height - box_height) / 2.0,
        right: (width + box_width) / 2.0,
        bottom: (height + box_height) / 2.0,
    }
}

/// Leaves out every cell whose triangle reaches into the box, so whole triangles disappear rather
/// than being cut, and borders run around the gap like around any other background.
fn clear_cells_under(shapes: &mut [Vec<Option<usize>>], grid: &TriangleGrid, area: &PixelBox) {
    for (row, cells) in shapes.iter_mut().enumerate() {
        for (col, shape) in cells.iter_mut().enumerate() {
            let corners = grid.vertices(row as i64, col as i64);
            let (xs, ys) = (corners.map(|c| c.0), corners.map(|c| c.1));
            let min = |v: [f64; 3]| v.into_iter().fold(f64::INFINITY, f64::min);
            let max = |v: [f64; 3]| v.into_iter().fold(f64::NEG_INFINITY, f64::max);
            if min(xs) < area.right
                && max(xs) > area.left
                && min(ys) < area.bottom
                && max(ys) > area.top
            {
                *shape = None;
            }
        }
    }
}

/// Gap between the overlay's image and the edge of its box, as a share of the triangle width.
const OVERLAY_MARGIN: f64 = 0.15;

/// Draws the overlay's image scaled to fit its box (keeping its aspect ratio), composited over the
/// code with its alpha.
fn draw_overlay(img: &mut RgbaImage, image: &RgbaImage, area: &PixelBox, triangle_width: u32) {
    let margin = OVERLAY_MARGIN * triangle_width as f64;
    let (room_w, room_h) = (
        (area.right - area.left - 2.0 * margin).max(1.0),
        (area.bottom - area.top - 2.0 * margin).max(1.0),
    );
    let scale = (room_w / image.width().max(1) as f64).min(room_h / image.height().max(1) as f64);
    let (w, h) = (
        ((image.width() as f64 * scale).round() as u32).max(1),
        ((image.height() as f64 * scale).round() as u32).max(1),
    );
    let scaled = image::imageops::resize(image, w, h, image::imageops::FilterType::Lanczos3);
    let x0 = ((area.left + area.right - w as f64) / 2.0).round() as u32;
    let y0 = ((area.top + area.bottom - h as f64) / 2.0).round() as u32;
    for (x, y, src) in scaled.enumerate_pixels() {
        let (px, py) = (x0 + x, y0 + y);
        if px < img.width() && py < img.height() {
            let dst = img.get_pixel_mut(px, py);
            *dst = over(*src, *dst);
        }
    }
}

/// Porter-Duff "over": `src` composited on top of `dst`, both with straight alpha.
fn over(src: Rgba<u8>, dst: Rgba<u8>) -> Rgba<u8> {
    let (sa, da) = (src[3] as f64 / 255.0, dst[3] as f64 / 255.0);
    let out_a = sa + da * (1.0 - sa);
    if out_a == 0.0 {
        return Rgba([0, 0, 0, 0]);
    }
    let channel =
        |i: usize| ((src[i] as f64 * sa + dst[i] as f64 * da * (1.0 - sa)) / out_a).round() as u8;
    Rgba([
        channel(0),
        channel(1),
        channel(2),
        (out_a * 255.0).round() as u8,
    ])
}

/// Rows rendered together by one parallel task.
const BAND_ROWS: usize = 16;

/// Samples per side of the grid used to anti-alias pixels on an outline (so 16 per pixel).
const AA_SAMPLES: u32 = 4;

/// Linear mix of two colours, alpha included: `from` at 0, `to` at 1. Colours aren't weighted by
/// alpha, so an edge over a transparent background keeps a light fringe the decoder still reads as
/// background, at the cost of a faint halo over dark backdrops.
fn mix(from: Rgba<u8>, to: Rgba<u8>, amount: f64) -> Rgba<u8> {
    Rgba(std::array::from_fn(|i| {
        (from[i] as f64 + (to[i] as f64 - from[i] as f64) * amount).round() as u8
    }))
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

/// How much of every codeword is spent on error correction. Not stored in the code: the decoder
/// tries every level, and the frame's CRC tells which one fits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EccLevel {
    /// About 7% of the bytes can be wrong.
    Low,
    /// About 15% of the bytes can be wrong.
    Medium,
    /// About 25% of the bytes can be wrong.
    Quartile,
    /// About 30% of the bytes can be wrong.
    High,
}

/// The level codes are generated at unless another is chosen.
pub const DEFAULT_ECC_LEVEL: EccLevel = EccLevel::Medium;

impl EccLevel {
    pub const ALL: [EccLevel; 4] = [
        EccLevel::Low,
        EccLevel::Medium,
        EccLevel::Quartile,
        EccLevel::High,
    ];

    /// The level's name, as shown in the app.
    pub fn name(self) -> &'static str {
        match self {
            EccLevel::Low => "Low",
            EccLevel::Medium => "Medium",
            EccLevel::Quartile => "Quartile",
            EccLevel::High => "High",
        }
    }

    /// 0 = Low, 1 = Medium, 2 = Quartile, 3 = High; anything else is Medium.
    pub fn from_u32(value: u32) -> Self {
        match value {
            0 => EccLevel::Low,
            2 => EccLevel::Quartile,
            3 => EccLevel::High,
            _ => EccLevel::Medium,
        }
    }

    /// Largest share of the code's area an [`Overlay`] may take at this level. These are the
    /// solid covers `survives_a_covered_centre` checks, so a code with an overlay this large still
    /// decodes.
    pub fn max_overlay_share(self) -> f64 {
        match self {
            EccLevel::Low => 0.02,
            EccLevel::Medium => 0.06,
            EccLevel::Quartile => 0.15,
            EccLevel::High => 0.18,
        }
    }

    /// Share of every codeword's bytes that are correction bytes. A codeword corrects half as
    /// many wrong bytes as it has correction bytes (or as many erased ones).
    fn ecc_share(self) -> f64 {
        match self {
            EccLevel::Low => 0.14,
            EccLevel::Medium => 0.30,
            EccLevel::Quartile => 0.50,
            EccLevel::High => 0.60,
        }
    }
}

/// Bytes of a frame around the payload: a 2-byte length before it and a 4-byte CRC-32 after.
const FRAME_OVERHEAD: usize = 6;

/// Splits `capacity` bytes into Reed-Solomon codewords for `level`, as `(data, ecc)` byte counts.
/// Depends only on the grid's capacity and the level, so the decoder can rebuild it. Codewords
/// differ in length by at most one byte. Empty when the capacity is too small for any frame.
fn block_layout(capacity: usize, level: EccLevel) -> Vec<(usize, usize)> {
    if capacity < FRAME_OVERHEAD + 2 {
        return Vec::new();
    }
    let blocks = capacity.div_ceil(reed_solomon::MAX_CODEWORD);
    let (base, longer) = (capacity / blocks, capacity % blocks);
    (0..blocks)
        .map(|i| {
            let n = base + usize::from(i >= blocks - longer);
            // An even number of correction bytes, at least 2, leaving at least one data byte.
            let ecc = (2.0 * (n as f64 * level.ecc_share() / 2.0).round()) as usize;
            let ecc = ecc.clamp(2, n - 1);
            (n - ecc, ecc)
        })
        .collect()
}

/// Payload bytes a grid of `capacity` bytes holds at `level`.
fn payload_capacity(capacity: usize, level: EccLevel) -> usize {
    let data: usize = block_layout(capacity, level).iter().map(|&(k, _)| k).sum();
    data.saturating_sub(FRAME_OVERHEAD).min(u16::MAX as usize)
}

/// Where each byte of the grid comes from, as `(codeword, index)`: byte 0 of every codeword, then
/// byte 1 of every codeword, and so on. Interleaving spreads a damaged patch of the image over all
/// codewords instead of overwhelming one of them.
fn interleave_order(layout: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let longest = layout.iter().map(|&(k, e)| k + e).max().unwrap_or(0);
    (0..longest)
        .flat_map(|i| {
            layout
                .iter()
                .enumerate()
                .filter(move |&(_, &(k, e))| i < k + e)
                .map(move |(block, _)| (block, i))
        })
        .collect()
}

/// Encodes `payload` into exactly `capacity` grid bytes at `level`: the frame
/// `[length: u16 BE][payload][CRC-32 BE of length and payload]`, zero-padded to the data capacity,
/// split into Reed-Solomon codewords and interleaved.
pub fn encode_frame(payload: &[u8], capacity: usize, level: EccLevel) -> Result<Vec<u8>, String> {
    if payload.len() > payload_capacity(capacity, level) {
        return Err(format!(
            "{} payload bytes don't fit in {} grid bytes",
            payload.len(),
            capacity
        ));
    }
    let layout = block_layout(capacity, level);

    let mut frame = Vec::with_capacity(capacity);
    frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    frame.extend_from_slice(payload);
    frame.extend_from_slice(&crc32fast::hash(&frame).to_be_bytes());
    frame.resize(layout.iter().map(|&(k, _)| k).sum(), 0);

    let mut rest = frame.as_slice();
    let mut codewords = Vec::with_capacity(layout.len());
    for &(k, ecc) in &layout {
        let (data, tail) = rest.split_at(k);
        codewords.push(reed_solomon::encode(data, ecc)?);
        rest = tail;
    }
    Ok(interleave_order(&layout)
        .into_iter()
        .map(|(block, i)| codewords[block][i])
        .collect())
}

/// A frame recovered by [`decode_frame`].
struct DecodedFrame {
    payload: Vec<u8>,
    /// Bytes the error correction had to change.
    corrected_bytes: usize,
}

/// Reverse of [`encode_frame`] for one level. `erased[i]` marks grid bytes read from cells the
/// decoder couldn't tell apart, which cost half as much correction as wrong bytes. Fails unless
/// every codeword corrects and the CRC matches.
fn decode_frame(bytes: &[u8], erased: &[bool], level: EccLevel) -> Result<DecodedFrame, String> {
    let layout = block_layout(bytes.len(), level);
    if layout.is_empty() {
        return Err("grid too small".to_string());
    }
    let mut codewords: Vec<Vec<u8>> = layout
        .iter()
        .map(|&(k, e)| Vec::with_capacity(k + e))
        .collect();
    let mut erasures: Vec<Vec<usize>> = vec![Vec::new(); layout.len()];
    for (pos, (block, i)) in interleave_order(&layout).into_iter().enumerate() {
        codewords[block].push(bytes[pos]);
        if erased.get(pos).copied().unwrap_or(false) {
            erasures[block].push(i);
        }
    }

    let mut frame = Vec::new();
    let mut corrected_bytes = 0;
    for ((codeword, erasures), &(k, ecc)) in codewords.iter_mut().zip(&erasures).zip(&layout) {
        // Erasures can be wrong too (a cell that only looked unsure), so retry without them.
        let changed = reed_solomon::correct(codeword, ecc, erasures)
            .or_else(|e| {
                if erasures.is_empty() {
                    Err(e)
                } else {
                    reed_solomon::correct(codeword, ecc, &[])
                }
            })
            .map_err(|e| format!("{:?} codeword: {}", level, e))?;
        corrected_bytes += changed;
        frame.extend_from_slice(&codeword[..k]);
    }

    let &[hi, lo, ..] = frame.as_slice() else {
        return Err("frame too short".to_string());
    };
    let len = u16::from_be_bytes([hi, lo]) as usize;
    let end = 2 + len;
    let crc = frame
        .get(end..end + 4)
        .ok_or_else(|| format!("{:?}: length {} exceeds the frame", level, len))?;
    if crc32fast::hash(&frame[..end]).to_be_bytes() != crc {
        return Err(format!("{:?}: CRC mismatch", level));
    }
    Ok(DecodedFrame {
        payload: frame[2..end].to_vec(),
        corrected_bytes,
    })
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
    /// "lattice-fit", "naive", "cluster+pitch", any of them with a " ±5" suffix when found in the neighbourhood, or "all failed".
    pub decode_method: String,
    /// Error-correction level the code was read at; `None` on failure.
    pub ecc_level: Option<EccLevel>,
    /// Bytes the error correction repaired: how damaged the code was.
    pub corrected_bytes: usize,
    /// Bytes read from cells the decoder was unsure about (see [`UNSURE_DARKNESS`]).
    pub erased_bytes: usize,
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

    // Try the strongest erosion first: it is what default-sized images need, so they decode on the
    // first attempt. Smaller erosions are only tried when that fails.
    let mut first_failure: Option<DecodeAttempt> = None;
    let mut failures: Vec<String> = Vec::new();
    for radius in EROSION_RADII {
        match decode_with_erosion(&gray_img, radius) {
            Ok(mut attempt) if attempt.debug.text.is_some() => {
                if radius != EROSION_RADII[0] {
                    attempt.debug.decode_method += &format!(" (erosion {})", radius);
                }
                return attempt.with_annotation(&img);
            }
            Ok(attempt) => {
                let error = attempt.debug.error.clone().unwrap_or_default();
                failures.push(format!("erosion {}:\n{}", radius, error));
                first_failure.get_or_insert(attempt);
            }
            Err(e) => failures.push(format!("erosion {}: {}", radius, e)),
        }
    }

    // Every erosion failed — report the first attempt that got as far as reading a grid.
    let failures = failures.join("\n");
    match first_failure {
        Some(mut attempt) => {
            attempt.debug.error = Some(format!("All attempts failed.\n{}", failures));
            attempt.with_annotation(&img)
        }
        None => Err(failures),
    }
}

/// Erosion radii (in pixels) tried in order. Erosion peels this many pixels off every black shape
/// so that touching triangles (Stain style, or bridges left by blur) come apart before the blobs are
/// traced. Two pixels suit the default triangle size, but small triangles don't survive it, so the
/// decoder falls back to gentler erosion when a decode fails.
const EROSION_RADII: [u8; 3] = [2, 1, 0];

/// One decode attempt at a given erosion radius, before its annotated image is drawn.
struct DecodeAttempt {
    /// `annotated_png` is left empty; [`DecodeAttempt::with_annotation`] fills it in.
    debug: DecodeDebug,
    triangles: Vec<Triangle>,
    anchors: Anchors,
}

impl DecodeAttempt {
    /// Draws the detected triangles over `img`. Done only for the attempt that is returned.
    fn with_annotation(self, img: &ImageBuffer<Rgba<u8>, Vec<u8>>) -> Result<DecodeDebug, String> {
        let mut annotated_img = img.clone();
        draw_rectangles(&mut annotated_img, &self.triangles, &self.anchors);

        let mut annotated_png = Vec::new();
        PngEncoder::new(&mut annotated_png)
            .write_image(
                annotated_img.as_raw(),
                annotated_img.width(),
                annotated_img.height(),
                ExtendedColorType::Rgba8,
            )
            .map_err(|e| format!("Failed to encode annotated image: {}", e))?;

        Ok(DecodeDebug {
            annotated_png,
            ..self.debug
        })
    }
}

/// Detects triangles with the given erosion radius and tries every grid strategy on them.
/// Returns `Err` only when no anchors are found; a failed decode is `Ok` with `error` set.
fn decode_with_erosion(gray_img: &GrayImage, erosion: u8) -> Result<DecodeAttempt, String> {
    // Find triangles in the image
    let triangles = find_triangles(gray_img, erosion)?;

    // Find the four anchor triangles at the grid corners
    let anchors = find_anchors(&triangles).ok_or("Could not find anchor triangles")?;

    let triangles = discard_merged_triangles(triangles, &anchors);

    eprintln!("DEBUG anchors (erosion {}):", erosion);
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

    let debug = {
        let ctx = GridContext {
            triangles: &triangles,
            anchors: &anchors,
            row_naive,
            col_naive,
            fit: fit_lattice(&triangles, &anchors),
        };
        try_grid_strategies(gray_img, &ctx)
    };
    Ok(DecodeAttempt {
        debug,
        triangles,
        anchors,
    })
}

/// Reads the grid for every strategy's estimates until one decodes. On failure, the result has
/// `error` set and describes the fitted grid (or the cluster+pitch estimate without one).
fn try_grid_strategies(gray_img: &GrayImage, ctx: &GridContext) -> DecodeDebug {
    let base = DecodeDebug {
        text: None,
        content_type: String::new(),
        error: None,
        image_width: gray_img.width(),
        image_height: gray_img.height(),
        total_triangles: ctx.triangles.len(),
        row_count: ctx.row_naive,
        column_count: ctx.col_naive,
        bit_count: 0,
        annotated_png: Vec::new(),
        decode_method: String::new(),
        ecc_level: None,
        corrected_bytes: 0,
        erased_bytes: 0,
    };

    // Each strategy yields grid estimates. Every estimate is tried as-is, then
    // its ±GRID_SEARCH_RADIUS neighbourhood (closest first). Already-tried grids are skipped.
    let mut tried: HashSet<(usize, usize)> = HashSet::new();
    let mut failures: Vec<String> = Vec::new();

    for strategy in GRID_STRATEGIES {
        let mut attempted = 0usize;
        let mut last_err = String::from("no new candidates");

        for estimate in (strategy.estimates)(ctx) {
            let grids: Vec<(usize, usize)> = with_neighbourhood(ctx, estimate)
                .into_iter()
                .filter(|&grid| tried.insert(grid))
                .collect();
            attempted += grids.len();
            // Grids are tried in parallel: each costs every flip × mask × level, which adds up when
            // nothing decodes. `find_map_first` still returns the best-ranked grid that decodes.
            let first_err: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
            let found = grids
                .par_iter()
                .enumerate()
                .find_map_first(|(index, &(rows, cols))| {
                    let lattice = ctx.lattice_for(rows, cols);
                    let cells = sample_cells(gray_img, rows, cols, &lattice);
                    for flip in FLIPS {
                        let packed = pack_data_bits(&cells, flip);
                        match try_unmix_decode(&packed, rows, cols) {
                            Ok(decoded) => return Some((rows, cols, flip, packed, decoded)),
                            Err(e) if index == 0 && flip.name().is_none() => {
                                if let Ok(mut slot) = first_err.lock() {
                                    *slot = Some(e);
                                }
                            }
                            Err(_) => {}
                        }
                    }
                    None
                });
            if let Some((rows, cols, flip, packed, decoded)) = found {
                let mut method = if (rows, cols) == estimate {
                    strategy.name.to_string()
                } else {
                    format!("{} ±{}", strategy.name, GRID_SEARCH_RADIUS)
                };
                if let Some(name) = flip.name() {
                    method += &format!(" ({})", name);
                }
                eprintln!("DEBUG '{}' succeeded: rows={} cols={}", method, rows, cols);
                return DecodeDebug {
                    text: Some(decoded.text),
                    content_type: decoded.kind,
                    row_count: rows,
                    column_count: cols,
                    bit_count: packed.bit_count,
                    decode_method: method,
                    ecc_level: Some(decoded.level),
                    corrected_bytes: decoded.corrected_bytes,
                    erased_bytes: packed.erased.iter().filter(|&&e| e).count(),
                    ..base
                };
            }
            if let Some(e) = first_err.into_inner().ok().flatten() {
                last_err = e;
            }
        }
        failures.push(format!(
            "{} ({} grids tried): {}",
            strategy.name, attempted, last_err
        ));
    }

    // All strategies failed — report using the fitted grid, or the cluster+pitch estimate without one.
    let (rows, cols) = match &ctx.fit {
        Some(fit) => (fit.rows, fit.cols),
        None => cluster_pitch_estimate(ctx),
    };
    let lattice = ctx.lattice_for(rows, cols);
    let bit_count =
        pack_data_bits(&sample_cells(gray_img, rows, cols, &lattice), FLIPS[0]).bit_count;
    DecodeDebug {
        error: Some(failures.join("\n")),
        row_count: rows,
        column_count: cols,
        bit_count,
        decode_method: "all failed".to_string(),
        ..base
    }
}

/// Remove false positives: any triangle significantly larger than the biggest anchor
/// is a cluster/merge artefact and should be discarded.
fn discard_merged_triangles(triangles: Vec<Triangle>, anchors: &Anchors) -> Vec<Triangle> {
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

    triangles
}

/// Inputs shared by all grid-estimation strategies.
struct GridContext<'a> {
    triangles: &'a [Triangle],
    anchors: &'a Anchors,
    row_naive: usize,
    col_naive: usize,
    fit: Option<LatticeFit>,
}

impl GridContext<'_> {
    /// The lattice to read a `rows × cols` grid from: the fitted one when it describes that grid,
    /// otherwise the one spanned by the anchors.
    fn lattice_for(&self, rows: usize, cols: usize) -> Lattice {
        match &self.fit {
            Some(fit) if (fit.rows, fit.cols) == (rows, cols) => fit.lattice,
            _ => Lattice::from_anchors(self.anchors, rows, cols),
        }
    }
}

/// A named way of estimating the grid size. Returns `(rows, cols)` estimates, most likely first.
struct GridStrategy {
    name: &'static str,
    estimates: fn(&GridContext) -> Vec<(usize, usize)>,
}

/// Strategies in the order they are tried.
static GRID_STRATEGIES: &[GridStrategy] = &[
    GridStrategy {
        name: "lattice-fit",
        estimates: lattice_fit_estimates,
    },
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

fn lattice_fit_estimates(ctx: &GridContext) -> Vec<(usize, usize)> {
    ctx.fit.iter().map(|fit| (fit.rows, fit.cols)).collect()
}

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

/// `estimate` and every grid within ±GRID_SEARCH_RADIUS of it, ordered by how well the detected
/// triangles fit each grid (see [`Lattice::misfit`]), closest to `estimate` first on ties.
/// Grids smaller than 2×2 are left out.
fn with_neighbourhood(ctx: &GridContext, estimate: (usize, usize)) -> Vec<(usize, usize)> {
    let (rows, cols) = estimate;
    let mut candidates: Vec<(f32, isize, (usize, usize))> = (-GRID_SEARCH_RADIUS
        ..=GRID_SEARCH_RADIUS)
        .flat_map(|dr| (-GRID_SEARCH_RADIUS..=GRID_SEARCH_RADIUS).map(move |dc| (dr, dc)))
        .filter_map(|(dr, dc)| {
            let r = usize::try_from(rows as isize + dr).ok()?;
            let c = usize::try_from(cols as isize + dc).ok()?;
            if r < 2 || c < 2 {
                return None;
            }
            let misfit = ctx.lattice_for(r, c).misfit(ctx.triangles, r, c);
            Some((misfit, dr.abs().max(dc.abs()), (r, c)))
        })
        .collect();
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    candidates.into_iter().map(|(_, _, grid)| grid).collect()
}

/// Rows advance by a full bit box and columns by half of one (neighbouring triangles interlock),
/// so this is the row pitch divided by the column pitch of a rendered grid. The decoder doesn't know
/// the triangle width, so this uses the default one; rounding moves the ratio by under 5% between
/// the smallest and largest widths, well inside `ROW_PITCH_TOLERANCE`.
fn pitch_aspect() -> f32 {
    triangle_height(DEFAULT_TRIANGLE_WIDTH) as f32 / (DEFAULT_TRIANGLE_WIDTH / 2) as f32
}

/// Distance, in row pitches, from a triangle's centroid to the centre of its cell. A pointing-down
/// triangle's centroid lies a third of its height below its base, i.e. this far above the centre;
/// a pointing-up one lies this far below it.
const CENTROID_OFFSET: f32 = 1.0 / 6.0;

/// Search range for the column pitch, as multiples of half the median triangle width. Borders and
/// erosion make triangles narrower than their cell, so the true pitch is at or above 1×; the range
/// stays clear of 0.5× and 2×, where the lattice would also line up.
const COL_PITCH_SEARCH: (f32, f32) = (0.8, 1.7);

/// How far the row pitch may stray from `col_pitch · pitch_aspect()`.
const ROW_PITCH_TOLERANCE: f32 = 0.15;

/// Least-squares refinement passes after the initial period estimate.
const FIT_ITERATIONS: usize = 3;

/// Triangles further than this (in pitches) from their nearest lattice point don't take part in
/// the least-squares fit.
const FIT_INLIER_DISTANCE: f32 = 0.3;

/// A fitted lattice is rejected when fewer triangles than this have the orientation their cell
/// requires.
const MIN_PARITY_AGREEMENT: f32 = 0.9;

/// Index into per-orientation arrays: 0 for pointing down, 1 for pointing up.
fn orientation_group(pointing_down: bool) -> usize {
    usize::from(!pointing_down)
}

/// Triangle centroid positions of a grid, in image pixels.
///
/// Cell `(row, col)` has its centroid at `(x0[g] + col·col_pitch, y0[g] + row·row_pitch)`, where
/// `g` is the cell's [`orientation_group`]. Cell `(0, 0)` is the top-left anchor, which always
/// points down. Centroids are used rather than bounding boxes because a border shrinks a triangle
/// evenly from every side, which leaves its centroid where it was.
#[derive(Debug, Clone, Copy)]
struct Lattice {
    x0: [f32; 2],
    y0: [f32; 2],
    col_pitch: f32,
    row_pitch: f32,
}

impl Lattice {
    /// The lattice spanned by the anchors for a `rows × cols` grid (both at least 2).
    fn from_anchors(anchors: &Anchors, rows: usize, cols: usize) -> Self {
        let top_left = &anchors.top_left;
        let col_pitch = (anchors.top_right.center_x - top_left.center_x) / (cols - 1) as f32;
        // A pointing-up bottom-left anchor sits an extra 2·CENTROID_OFFSET pitches lower.
        let span = anchors.bottom_left.center_y - top_left.center_y;
        let last_row = (rows - 1) as f32;
        let row_pitch = if points_down(rows - 1, 0) {
            span / last_row
        } else {
            span / (last_row + 2.0 * CENTROID_OFFSET)
        };
        Self {
            x0: [top_left.center_x; 2],
            y0: [
                top_left.center_y,
                top_left.center_y + 2.0 * CENTROID_OFFSET * row_pitch,
            ],
            col_pitch,
            row_pitch,
        }
    }

    fn centroid(&self, row: usize, col: usize) -> (f32, f32) {
        let g = orientation_group(points_down(row, col));
        (
            self.x0[g] + col as f32 * self.col_pitch,
            self.y0[g] + row as f32 * self.row_pitch,
        )
    }

    /// How badly the triangles fit a `rows × cols` grid on this lattice: the mean squared distance
    /// (in pitches) from each centroid to its nearest cell, plus the fraction of triangles outside
    /// the grid or with the wrong orientation for their cell. 0 is a perfect fit.
    fn misfit(&self, triangles: &[Triangle], rows: usize, cols: usize) -> f32 {
        if triangles.is_empty() {
            return f32::MAX;
        }
        let mut total = 0.0f32;
        for t in triangles {
            let g = orientation_group(!t.pointing_up);
            let col_f = (t.center_x - self.x0[g]) / self.col_pitch;
            let row_f = (t.center_y - self.y0[g]) / self.row_pitch;
            let (col, row) = (col_f.round(), row_f.round());
            total += (col_f - col).powi(2) + (row_f - row).powi(2);
            let inside = row >= 0.0 && col >= 0.0 && (row as usize) < rows && (col as usize) < cols;
            if !inside || points_down(row as usize, col as usize) == t.pointing_up {
                total += 1.0;
            }
        }
        total / triangles.len() as f32
    }
}

/// A lattice fitted to every detected triangle, and the grid size it implies.
#[derive(Debug, Clone, Copy)]
struct LatticeFit {
    lattice: Lattice,
    rows: usize,
    cols: usize,
}

/// Fits a [`Lattice`] to all triangles, so that every one of them (not just the anchors) pins down
/// the pitch, then reads the grid size off the anchors' lattice indices.
///
/// Returns `None` when the fit fails its sanity checks: anchors that don't form a rectangle, too
/// many triangles with the wrong orientation for their cell, or a row/column pitch ratio that
/// doesn't match the renderer.
fn fit_lattice(triangles: &[Triangle], anchors: &Anchors) -> Option<LatticeFit> {
    if triangles.len() < 4 {
        return None;
    }

    let mut widths: Vec<f32> = triangles
        .iter()
        .map(|t| (t.max_x - t.min_x + 1) as f32)
        .collect();
    widths.sort_by(f32::total_cmp);
    let half_width = widths[widths.len() / 2] / 2.0;

    let samples = |pos: fn(&Triangle) -> f32| -> Vec<AxisSample> {
        triangles
            .iter()
            .map(|t| AxisSample {
                pos: pos(t),
                group: orientation_group(!t.pointing_up),
            })
            .collect()
    };

    // Columns first: there are usually more of them, and the row pitch then only needs a narrow
    // search around `col_pitch · pitch_aspect()`.
    let x = fit_axis(
        &samples(|t| t.center_x),
        [0.0, 0.0],
        half_width * COL_PITCH_SEARCH.0,
        half_width * COL_PITCH_SEARCH.1,
    )?;
    let expected_row_pitch = x.pitch * pitch_aspect();
    let y = fit_axis(
        &samples(|t| t.center_y),
        [CENTROID_OFFSET, -CENTROID_OFFSET],
        expected_row_pitch * (1.0 - ROW_PITCH_TOLERANCE),
        expected_row_pitch * (1.0 + ROW_PITCH_TOLERANCE),
    )?;

    let index = |t: &Triangle| {
        let g = orientation_group(!t.pointing_up);
        (
            y.index(t.center_y, g).round() as i64,
            x.index(t.center_x, g).round() as i64,
        )
    };
    let (top, left) = index(&anchors.top_left);
    let (top_right_row, right) = index(&anchors.top_right);
    let (bottom, bottom_left_col) = index(&anchors.bottom_left);
    let bottom_right = index(&anchors.bottom_right);

    let parity_matches = triangles
        .iter()
        .filter(|t| {
            let (r, c) = index(t);
            (((r - top) + (c - left)).rem_euclid(2) == 0) != t.pointing_up
        })
        .count();
    let parity = parity_matches as f32 / triangles.len() as f32;
    let aspect = y.pitch / x.pitch / pitch_aspect();

    eprintln!(
        "DEBUG lattice-fit: col_pitch={:.3} row_pitch={:.3} aspect={:.3} parity={:.3} anchors tl=({},{}) tr=({},{}) bl=({},{}) br=({},{})",
        x.pitch,
        y.pitch,
        aspect,
        parity,
        top,
        left,
        top_right_row,
        right,
        bottom,
        bottom_left_col,
        bottom_right.0,
        bottom_right.1
    );

    if top_right_row != top || bottom_left_col != left || bottom_right != (bottom, right) {
        eprintln!("DEBUG lattice-fit rejected: anchors don't form a rectangle");
        return None;
    }
    if parity < MIN_PARITY_AGREEMENT {
        eprintln!("DEBUG lattice-fit rejected: orientation parity too low");
        return None;
    }
    if (aspect - 1.0).abs() > ROW_PITCH_TOLERANCE {
        eprintln!("DEBUG lattice-fit rejected: pitch aspect off");
        return None;
    }

    let rows = usize::try_from(bottom - top + 1).ok()?;
    let cols = usize::try_from(right - left + 1).ok()?;
    if rows < 2 || cols < 2 {
        return None;
    }

    // Re-base so that cell (0, 0) is the top-left anchor.
    let lattice = Lattice {
        x0: x.intercepts.map(|a| a + left as f32 * x.pitch),
        y0: y.intercepts.map(|a| a + top as f32 * y.pitch),
        col_pitch: x.pitch,
        row_pitch: y.pitch,
    };
    Some(LatticeFit {
        lattice,
        rows,
        cols,
    })
}

/// A triangle's centroid position along one axis, and its [`orientation_group`].
#[derive(Debug, Clone, Copy)]
struct AxisSample {
    pos: f32,
    group: usize,
}

/// One axis of a fitted lattice: a sample in group `g` at lattice index `k` sits at
/// `intercepts[g] + k·pitch`. Indices are relative to an arbitrary lattice point.
#[derive(Debug, Clone, Copy)]
struct AxisFit {
    intercepts: [f32; 2],
    pitch: f32,
}

impl AxisFit {
    /// Fractional lattice index of a position.
    fn index(&self, pos: f32, group: usize) -> f32 {
        (pos - self.intercepts[group]) / self.pitch
    }
}

/// Fits one lattice axis to `samples`. `offsets[g]` is how far (in pitches) group `g` sits from
/// the cell centre, which lets both orientations share one set of lattice points.
///
/// The pitch is first found by [`dominant_period`], then refined by least squares with one shared
/// pitch and a separate intercept per group, so a systematic centroid shift in one orientation
/// doesn't bias the pitch.
fn fit_axis(samples: &[AxisSample], offsets: [f32; 2], lo: f32, hi: f32) -> Option<AxisFit> {
    let shifted: Vec<(f32, f32)> = samples.iter().map(|s| (s.pos, offsets[s.group])).collect();
    let (pitch, origin) = dominant_period(&shifted, lo, hi)?;
    let mut fit = AxisFit {
        intercepts: offsets.map(|o| origin - o * pitch),
        pitch,
    };

    for _ in 0..FIT_ITERATIONS {
        // Per group: count, Σk, Σv, Σk², Σkv over inliers.
        let mut sums = [[0.0f64; 5]; 2];
        for s in samples {
            let k_f = fit.index(s.pos, s.group);
            let k = k_f.round();
            if (k_f - k).abs() > FIT_INLIER_DISTANCE {
                continue;
            }
            let (k, v) = (k as f64, s.pos as f64);
            let sum = &mut sums[s.group];
            sum[0] += 1.0;
            sum[1] += k;
            sum[2] += v;
            sum[3] += k * k;
            sum[4] += k * v;
        }

        let (mut sxx, mut sxy) = (0.0f64, 0.0f64);
        for [n, sk, sv, skk, skv] in sums {
            if n > 0.0 {
                sxx += skk - sk * sk / n;
                sxy += skv - sk * sv / n;
            }
        }
        if sxx <= 0.0 {
            return None;
        }
        let pitch = sxy / sxx;
        let intercept =
            |[n, sk, sv, ..]: [f64; 5]| (n > 0.0).then(|| ((sv - pitch * sk) / n) as f32);
        let pitch = pitch as f32;
        fit.intercepts = match (intercept(sums[0]), intercept(sums[1])) {
            (Some(a), Some(b)) => [a, b],
            (Some(a), None) => [a, a + (offsets[0] - offsets[1]) * pitch],
            (None, Some(b)) => [b + (offsets[1] - offsets[0]) * pitch, b],
            (None, None) => return None,
        };
        fit.pitch = pitch;
    }

    Some(fit)
}

/// Finds the period in `lo..=hi` at which the `(position, offset)` samples line up best: the one
/// that maximises the length of their summed phase vector, with phase `position / period + offset`.
/// Returns `(period, origin)`, where `origin` is the position of one lattice point.
///
/// Unlike rounding a single span by a single size, every sample contributes, so the error of
/// individual centroids averages out.
fn dominant_period(samples: &[(f32, f32)], lo: f32, hi: f32) -> Option<(f32, f32)> {
    if samples.is_empty() || !(lo > 0.0 && hi > lo) {
        return None;
    }
    let min = samples.iter().map(|s| s.0).fold(f32::MAX, f32::min);
    let max = samples.iter().map(|s| s.0).fold(f32::MIN, f32::max);
    // Between two tried periods the phase at the far end of the span must drift by well under a
    // cycle, or the peak could be stepped over.
    let span = (max - min).max(hi) as f64;
    let step = (lo as f64).powi(2) / (16.0 * span);
    let steps = (((hi - lo) as f64 / step).ceil() as usize).min(100_000);

    let mut best: Option<(f64, f32, f32)> = None;
    for i in 0..=steps {
        let period = lo as f64 + (hi - lo) as f64 * i as f64 / steps.max(1) as f64;
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for &(pos, offset) in samples {
            let phase = std::f64::consts::TAU * ((pos - min) as f64 / period + offset as f64);
            re += phase.cos();
            im += phase.sin();
        }
        let strength = re.hypot(im);
        if best.is_none_or(|(b, _, _)| strength > b) {
            let origin = min as f64 + im.atan2(re) / std::f64::consts::TAU * period;
            best = Some((strength, period as f32, origin as f32));
        }
    }
    best.map(|(_, period, origin)| (period, origin))
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

/// Find triangles in a grayscale image using connected component analysis.
///
/// The black shapes are eroded by `erosion` pixels before they are traced, which cuts the thin
/// bridges between touching triangles, and each traced blob is then grown back to its original size.
///
/// Possible improvement: erode-then-grow rounds off the triangle tips, and small triangles lose a
/// noticeable part of their shape. Using the eroded blobs only as seeds and assigning every black
/// pixel of the original image to its nearest seed (a watershed-style split) would keep the exact
/// shapes, and so more accurate centroids and orientations.
fn find_triangles(img: &GrayImage, erosion: u8) -> Result<Vec<Triangle>, String> {
    let width = img.width();
    let height = img.height();

    if width == 0 || height == 0 {
        return Err("Invalid image dimensions".to_string());
    }

    // Apply binary threshold to separate black and white regions
    let binary = threshold(img, 128, ThresholdType::Binary);

    // Erode the black regions to separate touching triangles. imageproc treats non-zero (white)
    // pixels as the foreground, so growing the white background by `erosion` erodes the black.
    let eroded = dilate(&binary, Norm::LInf, erosion);

    // Find connected components in the eroded image
    let mut triangles = Vec::new();
    let mut visited = vec![vec![false; width as usize]; height as usize];
    // Which component last claimed each pixel while growing back, so the buffer is shared.
    let mut grown_by = vec![0u32; (width * height) as usize];
    let mut component_id = 0u32;

    for y in 0..height {
        for x in 0..width {
            if !visited[y as usize][x as usize] && eroded.get_pixel(x, y)[0] == 0 {
                // Found a new black region in eroded image
                let eroded_component = flood_fill(&eroded, &mut visited, x, y);

                // Dilate the component back to approximate original size
                component_id += 1;
                let dilated_component = dilate_component(
                    eroded_component,
                    &binary,
                    erosion,
                    &mut grown_by,
                    component_id,
                );

                if let Some(triangle) = analyze_component(&dilated_component, x, y) {
                    triangles.push(triangle);
                }
            }
        }
    }

    Ok(triangles)
}

/// Grow a component back by up to `radius` steps (8-connected), only through pixels that are black
/// in the original binary image, so it can't reach across a gap into a neighbouring triangle.
/// `grown_by` marks the pixels already in the component with `id`; it is reused across components.
fn dilate_component(
    mut component: Vec<(u32, u32)>,
    binary: &GrayImage,
    radius: u8,
    grown_by: &mut [u32],
    id: u32,
) -> Vec<(u32, u32)> {
    let width = binary.width();
    let height = binary.height();
    let index = |x: u32, y: u32| (y * width + x) as usize;

    for &(x, y) in &component {
        grown_by[index(x, y)] = id;
    }

    // Breadth-first: each step expands only the pixels added by the previous one.
    let mut frontier_start = 0;
    for _ in 0..radius {
        let frontier_end = component.len();
        for i in frontier_start..frontier_end {
            let (x, y) = component[i];
            for ny in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                    let n = index(nx, ny);
                    if grown_by[n] != id && binary.get_pixel(nx, ny)[0] == 0 {
                        grown_by[n] = id;
                        component.push((nx, ny));
                    }
                }
            }
        }
        frontier_start = frontier_end;
    }

    component
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

/// Fraction of the pixels in a small square around (cx, cy) that are dark (< 128).
fn sample_region_darkness(img: &GrayImage, cx: f32, cy: f32, radius: u32) -> f32 {
    let r = radius as i32;
    let x0 = (cx as i32 - r).max(0) as u32;
    let y0 = (cy as i32 - r).max(0) as u32;
    let x1 = ((cx as i32 + r).max(0) as u32).min(img.width().saturating_sub(1));
    let y1 = ((cy as i32 + r).max(0) as u32).min(img.height().saturating_sub(1));

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
    if total == 0 {
        return 0.0;
    }
    dark as f32 / total as f32
}

/// A cell whose sampled darkness falls strictly between these is neither clearly a triangle nor
/// clearly background (the edge of a logo, a scratch, text drawn over the code), so the byte it
/// belongs to is treated as an erasure.
const UNSURE_DARKNESS: (f32, f32) = (0.2, 0.8);

/// A decoded payload and how it was recovered.
struct Decoded {
    kind: String,
    text: String,
    level: EccLevel,
    corrected_bytes: usize,
}

/// Tries every mask and error-correction level on `packed` until a frame decodes and its payload
/// unpacks.
fn try_unmix_decode(packed: &PackedBits, rows: usize, cols: usize) -> Result<Decoded, String> {
    let capacity = grid_capacity(rows, cols);
    if packed.bytes.len() < capacity {
        return Err("grid has fewer bits than its capacity".to_string());
    }
    let erased = &packed.erased[..capacity];
    let mut last_err = "no masks to try".to_string();
    for candidate in unmix_pixels(&packed.bytes, cols as u32) {
        // Highest level first: a single-codeword grid at a level is also a valid codeword at every
        // lower level (its generator's roots include theirs), so a lower level would decode it too
        // but report the wrong level and correct less.
        for level in EccLevel::ALL.into_iter().rev() {
            let result = decode_frame(&candidate[..capacity], erased, level).and_then(|frame| {
                let (kind, text) = unpack_payload(&frame.payload)?;
                Ok(Decoded {
                    kind,
                    text,
                    level,
                    corrected_bytes: frame.corrected_bytes,
                })
            });
            match result {
                Ok(decoded) => return Ok(decoded),
                Err(e) => last_err = e,
            }
        }
    }
    Err(last_err)
}

/// A mirror image of the code: which axes the image was flipped along.
#[derive(Debug, Clone, Copy)]
struct Flip {
    horizontal: bool,
    vertical: bool,
}

impl Flip {
    /// Appended to `decode_method` when the code was read through this flip.
    fn name(self) -> Option<&'static str> {
        match (self.horizontal, self.vertical) {
            (false, false) => None,
            (true, false) => Some("flipped horizontally"),
            (false, true) => Some("flipped vertically"),
            (true, true) => Some("flipped both ways"),
        }
    }
}

/// Bit orders tried when reading a grid, unflipped first. Grids always have an even row count and
/// an odd column count, so a mirrored code is still a valid lattice with the top-left cell pointing
/// down, and the orientation marks are symmetric: only the order of the data bits changes, and
/// Reed-Solomon decoding tells which order is right.
const FLIPS: [Flip; 4] = [
    Flip {
        horizontal: false,
        vertical: false,
    },
    Flip {
        horizontal: true,
        vertical: false,
    },
    Flip {
        horizontal: false,
        vertical: true,
    },
    Flip {
        horizontal: true,
        vertical: true,
    },
];

/// Samples how dark each grid cell is (see [`sample_region_darkness`]), at the centroid of its
/// triangle. Indexed `[row][col]` as the cells appear in the image.
fn sample_cells(
    img: &GrayImage,
    row_count: usize,
    column_count: usize,
    lattice: &Lattice,
) -> Vec<Vec<f32>> {
    if row_count < 2 || column_count < 2 {
        return Vec::new();
    }

    // Sampling radius: ~15% of the column step to stay well inside the triangle body
    let sample_radius = ((lattice.col_pitch * 0.15).round() as u32).max(2);

    eprintln!(
        "DEBUG grid: origin=({:.1},{:.1}) step=({:.1},{:.1}) radius={}",
        lattice.x0[0], lattice.y0[0], lattice.col_pitch, lattice.row_pitch, sample_radius
    );

    (0..row_count)
        .map(|row| {
            (0..column_count)
                .map(|col| {
                    let (cx, cy) = lattice.centroid(row, col);
                    sample_region_darkness(img, cx, cy, sample_radius)
                })
                .collect()
        })
        .collect()
}

/// Data bits read off a grid.
struct PackedBits {
    /// Packed MSB first; the last byte may be partly padding.
    bytes: Vec<u8>,
    /// Per byte: whether any of its cells was unsure (see [`UNSURE_DARKNESS`]).
    erased: Vec<bool>,
    bit_count: usize,
}

/// Reads the data bits out of sampled `cells` in the order the renderer wrote them, undoing
/// `flip`. The orientation-mark cells (first/last row, first 2 and last 2 columns) are skipped —
/// they are not data bits.
fn pack_data_bits(cells: &[Vec<f32>], flip: Flip) -> PackedBits {
    let row_count = cells.len();
    let column_count = cells.first().map_or(0, Vec::len);
    let mut bits: Vec<bool> = Vec::new();
    let mut unsure: Vec<bool> = Vec::new();

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

            let image_row = if flip.vertical {
                row_count - 1 - row
            } else {
                row
            };
            let image_col = if flip.horizontal {
                column_count - 1 - col
            } else {
                col
            };
            let darkness = cells[image_row][image_col];
            bits.push(darkness > 0.5);
            unsure.push(UNSURE_DARKNESS.0 < darkness && darkness < UNSURE_DARKNESS.1);
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
    let erased = unsure
        .chunks(8)
        .map(|chunk| chunk.contains(&true))
        .collect();

    PackedBits {
        bytes,
        erased,
        bit_count,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use image::imageops::{FilterType, resize};

    const STYLES: [ImageStyle; 3] = [ImageStyle::Tiles, ImageStyle::Stain, ImageStyle::Horizon];
    const RATIOS: [(u32, u32); 3] = [(1, 1), (16, 9), (3, 4)];
    /// Kept small: triangle detection is slow in debug builds.
    const PAYLOAD_LENGTHS: [usize; 3] = [5, 40, 150];

    fn payload(len: usize) -> String {
        (0..len).map(|i| (b'a' + (i % 26) as u8) as char).collect()
    }

    fn generate(style: ImageStyle, ratio: (u32, u32), len: usize) -> GenerateDebug {
        generate_at(style, ratio, len, DEFAULT_TRIANGLE_WIDTH)
    }

    fn generate_at(
        style: ImageStyle,
        ratio: (u32, u32),
        len: usize,
        triangle_width: u32,
    ) -> GenerateDebug {
        generate_image_debug(GenerateImageParams {
            payload: &payload(len),
            payload_type: "text",
            width_ratio: ratio.0,
            height_ratio: ratio.1,
            style,
            fg_color: 0xFF000000,
            bg_color: 0xFFFFFFFF,
            horizon_seed: 42,
            mask: None,
            triangle_width,
            ecc_level: DEFAULT_ECC_LEVEL,
            overlay: None,
        })
        .expect("generation failed")
    }

    /// Every style × ratio × payload length, with a label for assertion messages.
    fn all_cases() -> impl Iterator<Item = (String, usize, GenerateDebug)> {
        STYLES.into_iter().flat_map(|style| {
            RATIOS.into_iter().flat_map(move |ratio| {
                PAYLOAD_LENGTHS.into_iter().map(move |len| {
                    let label = format!("{:?} {:?} len={}", style, ratio, len);
                    (label, len, generate(style, ratio, len))
                })
            })
        })
    }

    fn rescale(png: &[u8], factor: f32) -> Vec<u8> {
        let img = image::load_from_memory(png)
            .expect("load failed")
            .to_rgba8();
        let width = (img.width() as f32 * factor).round() as u32;
        let height = (img.height() as f32 * factor).round() as u32;
        let scaled = resize(&img, width, height, FilterType::Triangle);
        let mut out = Vec::new();
        PngEncoder::new(&mut out)
            .write_image(scaled.as_raw(), width, height, ExtendedColorType::Rgba8)
            .expect("encode failed");
        out
    }

    fn assert_decodes_exactly(label: &str, len: usize, truth: (usize, usize), png: &[u8]) {
        let decoded = decode_image(png).expect("decode errored");
        assert_eq!(
            (decoded.row_count, decoded.column_count),
            truth,
            "{label}: wrong grid"
        );
        assert_eq!(
            decoded.decode_method, "lattice-fit",
            "{label}: needed a search"
        );
        assert_eq!(
            decoded.text.as_deref(),
            Some(payload(len).as_str()),
            "{label}: wrong text"
        );
    }

    fn grid_of(generated: &GenerateDebug) -> (usize, usize) {
        (
            generated.grid_rows as usize,
            generated.grid_columns as usize,
        )
    }

    /// Committed decode fixtures, named `{style}_{w}x{h}_len{len}_{rows}x{cols}.png`. They pin
    /// the decoder against images from a known renderer, so a renderer change can't hide a
    /// decoder regression.
    const FIXTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/decode");
    const FIXTURE_PAYLOAD_LENGTHS: [usize; 4] = [5, 40, 150, 400];
    /// Fixtures are rendered at the original 51 px width, so they also cover a non-default size.
    const FIXTURE_TRIANGLE_WIDTH: u32 = 51;

    /// Rewrites the fixtures from the current renderer. Only run after an intentional format change:
    /// `tricode-regen-fixtures` (or `cargo test -p tricode --release -- --ignored regenerate_decode_fixtures`)
    #[test]
    #[ignore]
    fn regenerate_decode_fixtures() {
        std::fs::create_dir_all(FIXTURES_DIR).expect("create fixtures dir");
        for style in STYLES {
            for ratio in RATIOS {
                for len in FIXTURE_PAYLOAD_LENGTHS {
                    let generated = generate_at(style, ratio, len, FIXTURE_TRIANGLE_WIDTH);
                    let name = format!(
                        "{}_{}x{}_len{}_{}x{}.png",
                        format!("{:?}", style).to_lowercase(),
                        ratio.0,
                        ratio.1,
                        len,
                        generated.grid_rows,
                        generated.grid_columns
                    );
                    std::fs::write(
                        format!("{FIXTURES_DIR}/{name}"),
                        to_grayscale_png(&generated.png_bytes),
                    )
                    .expect("write fixture");
                }
            }
        }
    }

    /// Re-encodes a rendered (pure black and white) PNG as 8-bit grayscale, which the decoder
    /// sees identically but takes about a fifth of the space.
    fn to_grayscale_png(png: &[u8]) -> Vec<u8> {
        let gray = image::load_from_memory(png)
            .expect("load failed")
            .to_luma8();
        let mut out = Vec::new();
        PngEncoder::new_with_quality(
            &mut out,
            image::codecs::png::CompressionType::Best,
            image::codecs::png::FilterType::Adaptive,
        )
        .write_image(
            gray.as_raw(),
            gray.width(),
            gray.height(),
            ExtendedColorType::L8,
        )
        .expect("encode failed");
        out
    }

    /// Parses `{style}_{w}x{h}_len{len}_{rows}x{cols}.png` into `(len, (rows, cols))`.
    fn parse_fixture_name(name: &str) -> Option<(usize, (usize, usize))> {
        let mut parts = name.strip_suffix(".png")?.split('_');
        let len = parts.nth(2)?.strip_prefix("len")?.parse().ok()?;
        let (rows, cols) = parts.next()?.split_once('x')?;
        Some((len, (rows.parse().ok()?, cols.parse().ok()?)))
    }

    #[test]
    fn decodes_fixtures() {
        let mut names: Vec<String> = std::fs::read_dir(FIXTURES_DIR)
            .expect("fixtures dir missing")
            .map(|entry| {
                entry
                    .expect("read fixtures dir")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|name| name.ends_with(".png"))
            .collect();
        names.sort();
        assert_eq!(
            names.len(),
            STYLES.len() * RATIOS.len() * FIXTURE_PAYLOAD_LENGTHS.len(),
            "fixture count"
        );
        for name in names {
            let (len, truth) = parse_fixture_name(&name).expect("bad fixture name");
            let png = std::fs::read(format!("{FIXTURES_DIR}/{name}")).expect("read fixture");
            assert_decodes_exactly(&name, len, truth, &png);
        }
    }

    /// Mirrors a PNG horizontally and/or vertically, as a selfie camera or a code seen through
    /// glass would.
    fn flip(png: &[u8], horizontally: bool, vertically: bool) -> Vec<u8> {
        let mut img = image::load_from_memory(png)
            .expect("load failed")
            .to_luma8();
        if horizontally {
            img = image::imageops::flip_horizontal(&img);
        }
        if vertically {
            img = image::imageops::flip_vertical(&img);
        }
        let mut out = Vec::new();
        PngEncoder::new(&mut out)
            .write_image(
                img.as_raw(),
                img.width(),
                img.height(),
                ExtendedColorType::L8,
            )
            .expect("encode failed");
        out
    }

    /// Decodes every fixture flipped horizontally, vertically and both ways.
    fn assert_fixtures_decode_flipped(horizontally: bool, vertically: bool, suffix: &str) {
        for entry in std::fs::read_dir(FIXTURES_DIR).expect("fixtures dir missing") {
            let name = entry.expect("read fixtures dir").file_name();
            let name = name.to_string_lossy();
            let Some((len, truth)) = parse_fixture_name(&name) else {
                continue;
            };
            let png = std::fs::read(format!("{FIXTURES_DIR}/{name}")).expect("read fixture");
            let decoded =
                decode_image(&flip(&png, horizontally, vertically)).expect("decode errored");
            let label = format!("{name} {suffix}");
            assert_eq!(
                (decoded.row_count, decoded.column_count),
                truth,
                "{label}: wrong grid"
            );
            assert_eq!(
                decoded.text.as_deref(),
                Some(payload(len).as_str()),
                "{label}: wrong text"
            );
            assert_eq!(
                decoded.decode_method,
                format!("lattice-fit ({suffix})"),
                "{label}: wrong method"
            );
        }
    }

    #[test]
    fn decodes_horizontally_flipped_fixtures() {
        assert_fixtures_decode_flipped(true, false, "flipped horizontally");
    }

    #[test]
    fn decodes_vertically_flipped_fixtures() {
        assert_fixtures_decode_flipped(false, true, "flipped vertically");
    }

    #[test]
    fn decodes_fixtures_flipped_both_ways() {
        assert_fixtures_decode_flipped(true, true, "flipped both ways");
    }

    /// Codes holding bare UTF-8 instead of a payload envelope must decode in every orientation,
    /// so telling the right reading apart can't rely on the envelope.
    #[test]
    fn decodes_flipped_codes_without_an_envelope() {
        for style in STYLES {
            for ratio in RATIOS {
                // Read unflipped, a horizontally flipped 1×1 code of length 218 unpacks as the
                // msgpack integer 11, so it fails when only the envelope is checked.
                let texts = [5, 40, 150, 218]
                    .map(payload)
                    .into_iter()
                    .chain(["a".to_string(), "zażółć gęślą jaźń 🔺".to_string()]);
                for text in texts {
                    let generated = create_image_from_payload(
                        text.as_bytes(),
                        &GenerateImageParams {
                            payload: "",
                            payload_type: "",
                            width_ratio: ratio.0,
                            height_ratio: ratio.1,
                            style,
                            fg_color: 0xFF000000,
                            bg_color: 0xFFFFFFFF,
                            horizon_seed: 42,
                            mask: None,
                            triangle_width: DEFAULT_TRIANGLE_WIDTH,
                            ecc_level: DEFAULT_ECC_LEVEL,
                            overlay: None,
                        },
                    )
                    .expect("generation failed");
                    for flip_case in FLIPS {
                        let png = flip(
                            &generated.png_bytes,
                            flip_case.horizontal,
                            flip_case.vertical,
                        );
                        let decoded = decode_image(&png).expect("decode errored");
                        let label = format!("{:?} {:?} {:?} {:?}", style, ratio, text, flip_case);
                        assert_eq!(
                            (decoded.content_type.as_str(), decoded.text.as_deref()),
                            ("plain", Some(text.as_str())),
                            "{label}: wrong text"
                        );
                        let expected_method = match flip_case.name() {
                            Some(name) => format!("lattice-fit ({name})"),
                            None => "lattice-fit".to_string(),
                        };
                        assert_eq!(
                            decoded.decode_method, expected_method,
                            "{label}: wrong method"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn unpacks_bare_utf8_before_generic_msgpack() {
        for text in ["a", "abcde", "zażółć gęślą jaźń"] {
            assert_eq!(
                unpack_payload(text.as_bytes()),
                Ok(("plain".to_string(), text.to_string()))
            );
        }
        // Not UTF-8, so still read as msgpack: `true`.
        assert_eq!(
            unpack_payload(&[0xc3]),
            Ok(("plain".to_string(), "true".to_string()))
        );
        let packed = pack_payload("url", "https://example.com").expect("pack");
        assert_eq!(
            unpack_payload(&packed),
            Ok(("url".to_string(), "https://example.com".to_string()))
        );
    }

    fn generate_with_level(style: ImageStyle, len: usize, ecc_level: EccLevel) -> GenerateDebug {
        generate_image_debug(GenerateImageParams {
            payload: &payload(len),
            payload_type: "text",
            width_ratio: 16,
            height_ratio: 9,
            style,
            fg_color: 0xFF000000,
            bg_color: 0xFFFFFFFF,
            horizon_seed: 42,
            mask: None,
            triangle_width: DEFAULT_TRIANGLE_WIDTH,
            ecc_level,
            overlay: None,
        })
        .expect("generation failed")
    }

    /// What is painted over the centre of a code.
    #[derive(Debug, Clone, Copy)]
    enum Cover {
        Black,
        White,
        /// A fine 3 px checkerboard: detail like a logo or text, which leaves cells unsure.
        Checker,
    }

    /// Paints `cover` over a centred rectangle of `share` of the image area, with the image's
    /// aspect ratio.
    fn cover_centre(png: &[u8], share: f64, cover: Cover) -> Vec<u8> {
        let mut img = image::load_from_memory(png)
            .expect("load failed")
            .to_luma8();
        let (w, h) = (img.width(), img.height());
        let (cw, ch) = (
            (w as f64 * share.sqrt()) as u32,
            (h as f64 * share.sqrt()) as u32,
        );
        let (x0, y0) = ((w - cw) / 2, (h - ch) / 2);
        for y in y0..y0 + ch {
            for x in x0..x0 + cw {
                let luma = match cover {
                    Cover::Black => 0,
                    Cover::White => 255,
                    Cover::Checker if (x / 3 + y / 3) % 2 == 0 => 0,
                    Cover::Checker => 255,
                };
                img.put_pixel(x, y, image::Luma([luma]));
            }
        }
        let mut out = Vec::new();
        PngEncoder::new(&mut out)
            .write_image(img.as_raw(), w, h, ExtendedColorType::L8)
            .expect("encode failed");
        out
    }

    #[test]
    fn every_level_decodes_and_is_detected() {
        let mut previous_cells = 0;
        for level in EccLevel::ALL {
            let generated = generate_with_level(ImageStyle::Tiles, 150, level);
            let cells = generated.grid_rows * generated.grid_columns;
            assert!(cells > previous_cells, "{level:?}: grid didn't grow");
            previous_cells = cells;
            let decoded = decode_image(&generated.png_bytes).expect("decode errored");
            assert_eq!(
                decoded.text.as_deref(),
                Some(payload(150).as_str()),
                "{level:?}"
            );
            assert_eq!(decoded.ecc_level, Some(level), "{level:?}: wrong level");
            assert_eq!(
                decoded.corrected_bytes, 0,
                "{level:?}: clean code needed correction"
            );
        }
    }

    /// Share of the image each level must survive being covered at its centre. Measured limits
    /// (len 40 to 400 at 16:9) are a little higher; a fine pattern costs less than a solid one
    /// because its cells read as unsure and become erasures.
    const SURVIVABLE_COVER: [(EccLevel, f64, f64); 4] = [
        (EccLevel::Low, 0.02, 0.05),
        (EccLevel::Medium, 0.06, 0.14),
        (EccLevel::Quartile, 0.15, 0.30),
        (EccLevel::High, 0.18, 0.40),
    ];

    #[test]
    fn survives_a_covered_centre() {
        for (level, solid, detailed) in SURVIVABLE_COVER {
            for style in [ImageStyle::Tiles, ImageStyle::Horizon] {
                let generated = generate_with_level(style, 150, level);
                for (cover, share) in [
                    (Cover::Black, solid),
                    (Cover::White, solid),
                    (Cover::Checker, detailed),
                ] {
                    let label = format!("{style:?} {level:?} {cover:?} {share}");
                    let png = cover_centre(&generated.png_bytes, share, cover);
                    let decoded = decode_image(&png).expect("decode errored");
                    assert_eq!(
                        decoded.text.as_deref(),
                        Some(payload(150).as_str()),
                        "{label}: {:?}",
                        decoded.error
                    );
                    assert_eq!(decoded.ecc_level, Some(level), "{label}: wrong level");
                    assert!(decoded.corrected_bytes > 0, "{label}: nothing was damaged");
                    if let Cover::Checker = cover {
                        assert!(decoded.erased_bytes > 0, "{label}: no erasures");
                    }
                }
            }
        }
    }

    /// A PNG overlay `w`×`h` px: a dark disc with a light fine pattern inside, transparent around
    /// it, like a logo.
    fn logo_png(w: u32, h: u32) -> Vec<u8> {
        let (cx, cy, r) = (w as f64 / 2.0, h as f64 / 2.0, w.min(h) as f64 / 2.0);
        let img = RgbaImage::from_fn(w, h, |x, y| {
            let d = (x as f64 + 0.5 - cx).hypot(y as f64 + 0.5 - cy);
            match d {
                d if d > r => Rgba([0, 0, 0, 0]),
                d if d < r * 0.6 && (x / 4 + y / 4) % 2 == 0 => Rgba([240, 240, 240, 255]),
                _ => Rgba([20, 20, 60, 255]),
            }
        });
        let mut out = Vec::new();
        PngEncoder::new(&mut out)
            .write_image(img.as_raw(), w, h, ExtendedColorType::Rgba8)
            .expect("encode failed");
        out
    }

    fn generate_with_overlay(
        style: ImageStyle,
        level: EccLevel,
        overlay: Overlay<'_>,
    ) -> Result<GenerateDebug, String> {
        generate_image_debug(GenerateImageParams {
            payload: &payload(150),
            payload_type: "text",
            width_ratio: 16,
            height_ratio: 9,
            style,
            fg_color: 0xFF000000,
            bg_color: 0xFFFFFFFF,
            horizon_seed: 42,
            mask: None,
            triangle_width: DEFAULT_TRIANGLE_WIDTH,
            ecc_level: level,
            overlay: Some(overlay),
        })
    }

    #[test]
    fn decodes_with_the_largest_overlay_of_each_level() {
        // A square logo and a wide one, as a line of text renders.
        let (square, wide) = (logo_png(200, 200), logo_png(600, 120));
        for level in EccLevel::ALL {
            for style in [ImageStyle::Tiles, ImageStyle::Horizon] {
                for png in [&square, &wide] {
                    let overlay = Overlay {
                        png,
                        share: level.max_overlay_share(),
                    };
                    let generated =
                        generate_with_overlay(style, level, overlay).expect("generation failed");
                    assert_eq!(generated.overlay_decodes, Some(true), "{style:?} {level:?}");
                }
            }
        }
    }

    #[test]
    fn overlay_changes_the_centre_only() {
        let plain = generate_with_level(ImageStyle::Tiles, 150, EccLevel::High);
        let png = logo_png(100, 100);
        let overlay = Overlay {
            png: &png,
            share: 0.1,
        };
        let covered =
            generate_with_overlay(ImageStyle::Tiles, EccLevel::High, overlay).expect("failed");
        let (a, b) = (
            image::load_from_memory(&plain.png_bytes)
                .expect("load")
                .to_rgba8(),
            image::load_from_memory(&covered.png_bytes)
                .expect("load")
                .to_rgba8(),
        );
        assert_eq!(a.dimensions(), b.dimensions());
        let (w, h) = a.dimensions();
        // The corners, where the anchors are, stay untouched; the centre changes.
        for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
            for (dx, dy) in (0..60).flat_map(|i| (0..60).map(move |j| (i, j))) {
                let (px, py) = (x.abs_diff(dx), y.abs_diff(dy));
                assert_eq!(a.get_pixel(px, py), b.get_pixel(px, py));
            }
        }
        assert_ne!(a.get_pixel(w / 2, h / 2), b.get_pixel(w / 2, h / 2));
    }

    #[test]
    fn refuses_bad_overlays() {
        let png = logo_png(50, 50);
        for share in [0.0, -0.1, EccLevel::Medium.max_overlay_share() + 0.01] {
            let overlay = Overlay { png: &png, share };
            let error = generate_with_overlay(ImageStyle::Tiles, EccLevel::Medium, overlay)
                .expect_err("accepted a bad share");
            assert!(error.contains("Overlay must take"), "{share}: {error}");
        }
        let overlay = Overlay {
            png: b"not a png",
            share: 0.05,
        };
        let error = generate_with_overlay(ImageStyle::Tiles, EccLevel::Medium, overlay)
            .expect_err("accepted garbage");
        assert!(error.contains("overlay image"), "{error}");
    }

    #[test]
    fn fails_when_damage_exceeds_the_level() {
        let generated = generate_with_level(ImageStyle::Tiles, 40, EccLevel::Low);
        let png = cover_centre(&generated.png_bytes, 0.2, Cover::Black);
        let decoded = decode_image(&png).expect("decode errored");
        assert_eq!(decoded.text, None);
        assert_eq!(decoded.decode_method, "all failed");
    }

    #[test]
    fn survives_damage_and_a_flip_together() {
        let generated = generate_with_level(ImageStyle::Stain, 150, EccLevel::High);
        let damaged = cover_centre(&generated.png_bytes, 0.15, Cover::Black);
        let decoded = decode_image(&flip(&damaged, true, false)).expect("decode errored");
        assert_eq!(decoded.text.as_deref(), Some(payload(150).as_str()));
        assert_eq!(decoded.decode_method, "lattice-fit (flipped horizontally)");
    }

    #[test]
    fn block_layout_fills_the_capacity() {
        for capacity in [8, 9, 50, 254, 255, 256, 600, 1070, 5000] {
            for level in EccLevel::ALL {
                let layout = block_layout(capacity, level);
                let total: usize = layout.iter().map(|&(k, e)| k + e).sum();
                assert_eq!(total, capacity, "{capacity} {level:?}");
                for &(k, ecc) in &layout {
                    assert!(k >= 1 && ecc >= 2 && ecc % 2 == 0, "{capacity} {level:?}");
                    assert!(
                        k + ecc <= reed_solomon::MAX_CODEWORD,
                        "{capacity} {level:?}"
                    );
                }
            }
        }
        assert!(block_layout(7, EccLevel::Low).is_empty());
    }

    #[test]
    fn frame_round_trips_and_never_decodes_at_a_higher_level() {
        let payload = b"tricode frame";
        for capacity in [60, 300, 1070] {
            for level in EccLevel::ALL {
                let bytes = encode_frame(payload, capacity, level).expect("encode");
                assert_eq!(bytes.len(), capacity);
                let erased = vec![false; capacity];
                let frame = decode_frame(&bytes, &erased, level).expect("decode");
                assert_eq!(frame.payload, payload);
                // Lower levels may decode it too (see `try_unmix_decode`), higher ones must not.
                for higher in EccLevel::ALL.into_iter().filter(|&l| l as u8 > level as u8) {
                    assert!(
                        decode_frame(&bytes, &erased, higher).is_err(),
                        "{capacity}: {level:?} decoded as {higher:?}"
                    );
                }
            }
        }
        assert!(encode_frame(&[0; 100], 40, EccLevel::High).is_err());
    }

    #[test]
    fn lattice_fit_survives_non_integer_scaling() {
        for factor in [0.8, 1.37] {
            for style in STYLES {
                let len = 150;
                let generated = generate(style, (16, 9), len);
                let label = format!("{:?} ×{}", style, factor);
                assert_decodes_exactly(
                    &label,
                    len,
                    grid_of(&generated),
                    &rescale(&generated.png_bytes, factor),
                );
            }
        }
    }

    #[test]
    fn decodes_at_the_triangle_width_limits() {
        for triangle_width in [MIN_TRIANGLE_WIDTH, 75, MAX_TRIANGLE_WIDTH] {
            for style in STYLES {
                let len = 40;
                let generated = generate_at(style, (16, 9), len, triangle_width);
                let label = format!("{:?} {} px", style, triangle_width);
                assert_decodes_exactly(&label, len, grid_of(&generated), &generated.png_bytes);
            }
        }
    }

    #[test]
    fn borders_scale_with_the_triangle_width() {
        // Same grid at two widths: the larger image must be the smaller one enlarged, so the share
        // of foreground pixels stays about the same instead of growing as fixed borders thin out.
        for style in STYLES {
            let share = |triangle_width| {
                let generated = generate_at(style, (1, 1), 40, triangle_width);
                let gray = image::load_from_memory(&generated.png_bytes)
                    .expect("load failed")
                    .to_luma8();
                // Ink weighted by darkness, so anti-aliased edge pixels count partly.
                gray.pixels()
                    .map(|p| 1.0 - p[0] as f64 / 255.0)
                    .sum::<f64>()
                    / gray.len() as f64
            };
            let (small, large) = (share(MIN_TRIANGLE_WIDTH), share(MAX_TRIANGLE_WIDTH));
            assert!(
                (small - large).abs() < 0.03,
                "{:?}: foreground share {:.3} at {} px vs {:.3} at {} px",
                style,
                small,
                MIN_TRIANGLE_WIDTH,
                large,
                MAX_TRIANGLE_WIDTH
            );
        }
    }

    #[test]
    fn rejects_invalid_triangle_widths() {
        for triangle_width in [MIN_TRIANGLE_WIDTH - 2, 38, MAX_TRIANGLE_WIDTH + 2] {
            let result = generate_image_debug(GenerateImageParams {
                payload: &payload(5),
                payload_type: "text",
                width_ratio: 1,
                height_ratio: 1,
                style: ImageStyle::Tiles,
                fg_color: 0xFF000000,
                bg_color: 0xFFFFFFFF,
                horizon_seed: 0,
                mask: None,
                triangle_width,
                ecc_level: DEFAULT_ECC_LEVEL,
                overlay: None,
            });
            assert!(result.is_err(), "{} px was accepted", triangle_width);
        }
    }

    #[test]
    fn pixel_limit_names_the_largest_width_that_fits() {
        // 400 bytes at 16:9 is a 48×143 grid: about 70 MP at the largest width.
        let grid = grid_of(&generate(ImageStyle::Tiles, (16, 9), 400));
        let (rows, cols) = (grid.0 as u32, grid.1 as u32);
        let error = check_image_pixels(cols, rows, MAX_TRIANGLE_WIDTH).expect_err("fits");
        let fits: u32 = error
            .rsplit("fits is ")
            .next()
            .and_then(|rest| rest.trim_end_matches(" px").parse().ok())
            .unwrap_or_else(|| panic!("no width in {error:?}"));
        assert!(fits % 2 == 1 && fits < MAX_TRIANGLE_WIDTH, "{error}");
        assert!(check_image_pixels(cols, rows, fits).is_ok());
        assert!(check_image_pixels(cols, rows, fits + 2).is_err());

        // Long enough to overflow even at the smallest width.
        let error = generate_image_debug(GenerateImageParams {
            payload: &payload(40_000),
            payload_type: "text",
            width_ratio: 1,
            height_ratio: 1,
            style: ImageStyle::Tiles,
            fg_color: 0xFF000000,
            bg_color: 0xFFFFFFFF,
            horizon_seed: 0,
            mask: None,
            triangle_width: MIN_TRIANGLE_WIDTH,
            ecc_level: DEFAULT_ECC_LEVEL,
            overlay: None,
        })
        .expect_err("a 40 kB payload fits");
        assert!(error.contains("shorten the payload"), "{error}");
    }

    #[test]
    fn edges_follow_straight_grid_lines() {
        // Stain has no borders, so every pixel takes the colour of the cell its centre lies in. The
        // cells are cut by straight lines through the grid's vertices: horizontal lines between
        // rows and slanted ones moving one column pitch per row. Away from those lines, the
        // rendered image must match them exactly, also across row boundaries.
        let triangle_width = MIN_TRIANGLE_WIDTH;
        let generated = generate_at(ImageStyle::Stain, (16, 9), 150, triangle_width);
        let img = image::load_from_memory(&generated.png_bytes)
            .expect("load failed")
            .to_luma8();
        let (rows, cols) = grid_of(&generated);
        let pitch = (triangle_width / 2) as f64;
        let height = triangle_height(triangle_width) as f64;
        let (origin_x, origin_y) = (pitch, (height / 2.0).floor() - 0.5);
        let is_black = |x: f64, y: f64| img.get_pixel(x as u32, y as u32)[0] < 128;

        let mut checked = 0;
        for y in 0..img.height() {
            let v = (y as f64 - origin_y) / height;
            let row = v.floor();
            let t = v - row;
            if row < 0.0 || row >= rows as f64 || t * height < 0.5 || (1.0 - t) * height < 0.5 {
                continue;
            }
            for x in 0..img.width() {
                let u = (x as f64 - origin_x) / pitch;
                // Horizontal distance to the nearest slanted line, in pixels.
                let off_line = |w: f64| (w - w.round()).abs() * pitch;
                if off_line(u - t) < 0.5 || off_line(u + t) < 0.5 {
                    continue;
                }
                // The cell whose span in this pixel row contains the point.
                let Some(col) = (0..cols).find(|&c| {
                    let c = c as f64;
                    let down = (row as usize + c as usize).is_multiple_of(2);
                    let (from, to) = if down {
                        (c + t, c + 2.0 - t)
                    } else {
                        (c + 1.0 - t, c + 1.0 + t)
                    };
                    (from..to).contains(&u)
                }) else {
                    continue;
                };
                // The cell's colour, read at its centroid.
                let down = (row as usize + col).is_multiple_of(2);
                let cx = origin_x + (col as f64 + 1.0) * pitch;
                let cy = origin_y + (row + if down { 1.0 / 3.0 } else { 2.0 / 3.0 }) * height;
                assert_eq!(
                    is_black(x as f64, y as f64),
                    is_black(cx.round(), cy.round()),
                    "pixel ({x}, {y}) differs from cell ({row}, {col})"
                );
                checked += 1;
            }
        }
        assert!(checked > 100_000, "only {checked} pixels checked");
    }

    #[test]
    fn cells_around_a_vertex_match_the_cells_found_there() {
        for triangle_width in [MIN_TRIANGLE_WIDTH, MAX_TRIANGLE_WIDTH] {
            let grid = TriangleGrid::new(6, 9, triangle_width);
            for line in 1..6 {
                // Vertices on a line sit every other column, alternating between lines.
                for k in (line % 2..10).step_by(2) {
                    let (vx, vy) = grid.vertex_at((k, line));
                    for (i, &cell) in TriangleGrid::cells_around((k, line)).iter().enumerate() {
                        // A point a few pixels into the sector between rays i and i + 1.
                        let (ax, ay) = grid.ray_vector(i);
                        let (bx, by) = grid.ray_vector((i + 1) % 6);
                        let (dx, dy) = (ax + bx, ay + by);
                        let scale = 4.0 / dx.hypot(dy);
                        let point = (vx + dx * scale, vy + dy * scale);
                        assert_eq!(
                            grid.cell_at(point.0, point.1),
                            cell,
                            "width {triangle_width}, vertex ({k}, {line}), sector {i}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn outlines_are_anti_aliased() {
        for style in STYLES {
            let generated = generate(style, (1, 1), 40);
            let gray = image::load_from_memory(&generated.png_bytes)
                .expect("load failed")
                .to_luma8();
            let pure = |x: u32, y: u32| matches!(gray.get_pixel(x, y)[0], 0 | 255);
            let mut blended = 0;
            for (x, y, p) in gray.enumerate_pixels() {
                if matches!(p[0], 0 | 255) {
                    continue;
                }
                blended += 1;
                // Blended pixels sit on an outline, so they touch a fully painted or blank one.
                let touches_pure =
                    (x.saturating_sub(1)..=(x + 1).min(gray.width() - 1)).any(|nx| {
                        (y.saturating_sub(1)..=(y + 1).min(gray.height() - 1))
                            .any(|ny| pure(nx, ny))
                    });
                assert!(
                    touches_pure,
                    "{:?}: blended pixel ({x}, {y}) away from any edge",
                    style
                );
            }
            let share = blended as f64 / gray.len() as f64;
            assert!(
                (0.005..0.2).contains(&share),
                "{:?}: {:.3} of pixels blended",
                style,
                share
            );
        }
    }

    #[test]
    fn small_triangles_decode_with_gentler_erosion() {
        // Scaled from 51 px, triangles are ~18 and ~13 px wide: the default 2 px erosion destroys
        // or merges them, so decoding has to fall back to a smaller radius.
        for factor in [0.35, 0.25] {
            for style in STYLES {
                let len = 40;
                let generated = generate_at(style, (16, 9), len, 51);
                let label = format!("{:?} ×{}", style, factor);
                let decoded =
                    decode_image(&rescale(&generated.png_bytes, factor)).expect("decode errored");
                assert_eq!(
                    (decoded.row_count, decoded.column_count),
                    grid_of(&generated),
                    "{label}: wrong grid"
                );
                assert_eq!(
                    decoded.text.as_deref(),
                    Some(payload(len).as_str()),
                    "{label}: wrong text"
                );
                assert!(
                    decoded.decode_method.starts_with("lattice-fit (erosion "),
                    "{label}: expected a gentler-erosion lattice fit, got {}",
                    decoded.decode_method
                );
            }
        }
    }

    #[test]
    fn misfit_ranks_true_grid_first() {
        for (label, _, generated) in all_cases() {
            let gray = image::load_from_memory(&generated.png_bytes)
                .expect("load failed")
                .to_luma8();
            let triangles = find_triangles(&gray, EROSION_RADII[0]).expect("no triangles");
            let anchors = find_anchors(&triangles).expect("no anchors");
            let triangles = discard_merged_triangles(triangles, &anchors);
            // Without a fitted lattice, so only the anchor-spanned lattices are ranked.
            let ctx = GridContext {
                triangles: &triangles,
                anchors: &anchors,
                row_naive: 0,
                col_naive: 0,
                fit: None,
            };
            let truth = grid_of(&generated);
            let off_estimate = (truth.0 + 3, truth.1 - 4);
            let ranked = with_neighbourhood(&ctx, off_estimate);
            assert_eq!(ranked.first(), Some(&truth), "{label}");
        }
    }

    #[test]
    fn every_forced_mask_is_used_and_decodes() {
        assert_eq!(MASK_NAMES.len(), MASKS.len());
        let len = 40;
        for (index, name) in MASK_NAMES.iter().enumerate() {
            let generated = generate_image_debug(GenerateImageParams {
                payload: &payload(len),
                payload_type: "text",
                width_ratio: 16,
                height_ratio: 9,
                style: ImageStyle::Tiles,
                fg_color: 0xFF000000,
                bg_color: 0xFFFFFFFF,
                horizon_seed: 0,
                mask: Some(index),
                triangle_width: DEFAULT_TRIANGLE_WIDTH,
                ecc_level: DEFAULT_ECC_LEVEL,
                overlay: None,
            })
            .expect("generation failed");
            assert_eq!(generated.mask_index, index, "{name}");
            assert_decodes_exactly(name, len, grid_of(&generated), &generated.png_bytes);
        }
    }

    #[test]
    fn colour_transitions_counts_shared_triangle_edges() {
        // Row 1 has one pointing-down cell, (1, 1), whose top edge it shares with (0, 1).
        let cells = vec![vec![true, false, true], vec![true, true, false]];
        assert_eq!(colour_transitions(&cells), (4, 5));
    }

    #[test]
    fn auto_mask_has_the_most_colour_transitions() {
        let params = |mask| GenerateImageParams {
            payload: "Lorem ipsum dolor sit amet, consectetur adipiscing elit.",
            payload_type: "text",
            width_ratio: 21,
            height_ratio: 13,
            style: ImageStyle::Tiles,
            fg_color: 0xFF000000,
            bg_color: 0xFFFFFFFF,
            horizon_seed: 0,
            mask,
            triangle_width: DEFAULT_TRIANGLE_WIDTH,
            ecc_level: DEFAULT_ECC_LEVEL,
            overlay: None,
        };
        let auto = generate_image_debug(params(None)).expect("generation failed");
        for (index, name) in MASK_NAMES.iter().enumerate() {
            let forced = generate_image_debug(params(Some(index))).expect("generation failed");
            assert_eq!(forced.shared_edges, auto.shared_edges);
            assert!(
                auto.colour_transitions >= forced.colour_transitions,
                "{} beats the automatic choice",
                name
            );
        }
    }

    #[test]
    fn masks_cover_the_padding_at_the_end_of_the_grid() {
        for (index, name) in MASK_NAMES.iter().enumerate() {
            // The codewords fill whole bytes, so the cells after the last byte are padding.
            let generated = generate_image_debug(GenerateImageParams {
                payload: &payload(5),
                payload_type: "text",
                width_ratio: 16,
                height_ratio: 9,
                style: ImageStyle::Tiles,
                fg_color: 0xFF000000,
                bg_color: 0xFFFFFFFF,
                horizon_seed: 0,
                mask: Some(index),
                triangle_width: DEFAULT_TRIANGLE_WIDTH,
                ecc_level: DEFAULT_ECC_LEVEL,
                overlay: None,
            })
            .expect("generation failed");
            let cells = (generated.grid_rows * generated.grid_columns) as usize - 8;
            assert_eq!(generated.padding_bits, cells % 8);
            assert!(generated.padding_bits > 0, "{name}: no padding to check");

            let gray = image::load_from_memory(&generated.png_bytes)
                .expect("load failed")
                .to_luma8();
            let triangles = find_triangles(&gray, EROSION_RADII[0]).expect("no triangles");
            let anchors = find_anchors(&triangles).expect("no anchors");
            let triangles = discard_merged_triangles(triangles, &anchors);
            let fit = fit_lattice(&triangles, &anchors).expect("no lattice");
            let PackedBits {
                bytes, bit_count, ..
            } = pack_data_bits(
                &sample_cells(&gray, fit.rows, fit.cols, &fit.lattice),
                FLIPS[0],
            );
            // The padding is zeros, so after XOR every padding cell must show the mask itself.
            let mask = MASKS[index];
            let cols = generated.grid_columns as usize;
            let read: Vec<u8> = (generated.data_bits_used..bit_count)
                .map(|k| (bytes[k / 8] >> (7 - k % 8)) & 1)
                .collect();
            let expected: Vec<u8> = (generated.data_bits_used..bit_count)
                .map(|k| mask[(k / cols) % mask.len()][(k % cols) % mask[0].len()])
                .collect();
            assert_eq!(read, expected, "{name}: padding doesn't carry the mask");
        }
    }

    #[test]
    fn dominant_period_recovers_sub_pixel_pitch() {
        // Every third lattice point missing, positions jittered by up to ±1px.
        let samples: Vec<(f32, f32)> = (0..200)
            .filter(|k| k % 3 != 0)
            .map(|k| {
                (
                    7.3 + k as f32 * 25.4 + ((k * 7919) % 21) as f32 / 10.0 - 1.0,
                    0.0,
                )
            })
            .collect();
        let (period, origin) = dominant_period(&samples, 20.0, 40.0).expect("no period");
        assert!((period - 25.4).abs() < 0.01, "period {period}");
        let k = (origin - 7.3) / 25.4;
        assert!((k - k.round()).abs() < 0.05, "origin {origin}");
    }

    #[test]
    fn anchor_lattice_accounts_for_bottom_left_orientation() {
        let anchor = |cx: f32, cy: f32, pointing_up: bool| Triangle {
            center_x: cx,
            center_y: cy,
            area: 0.0,
            is_black: true,
            pointing_up,
            min_x: 0,
            min_y: 0,
            max_x: 0,
            max_y: 0,
        };
        // 3 rows: the bottom-left cell (2, 0) points down, at exactly 2 pitches.
        let anchors = Anchors {
            top_left: anchor(10.0, 15.0, false),
            top_right: anchor(110.0, 15.0, false),
            bottom_left: anchor(10.0, 105.0, false),
            bottom_right: anchor(110.0, 105.0, false),
        };
        let lattice = Lattice::from_anchors(&anchors, 3, 5);
        assert_eq!((lattice.col_pitch, lattice.row_pitch), (25.0, 45.0));
        // 4 rows: the bottom-left cell (3, 0) points up, a third of a pitch lower than 3 pitches.
        let anchors = Anchors {
            bottom_left: anchor(10.0, 15.0 + 3.0 * 45.0 + 15.0, true),
            ..anchors
        };
        let lattice = Lattice::from_anchors(&anchors, 4, 5);
        assert!((lattice.row_pitch - 45.0).abs() < 1e-4);
        assert!((lattice.centroid(1, 0).1 - (15.0 + 45.0 + 15.0)).abs() < 1e-4);
    }
}
