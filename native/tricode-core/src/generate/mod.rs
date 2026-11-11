//! Rendering a payload as a PNG of triangles.

mod geometry;
mod horizon;
mod overlay;
mod render;
mod shade;

use crate::decode::{MIN_LUMA_GAP, decode_rendered};
use crate::frame::{EccLevel, encode_frame};
use crate::grid::{
    check_image_pixels, gcd, grid_capacity, grid_for_payload, image_size, layout_cells,
    validate_triangle_width,
};
use crate::mask::{colour_transitions, mix_pixels};
use crate::payload::pack_payload;
use crate::png::encode_png;
use geometry::TriangleGrid;
use horizon::HorizonLayout;
use image::{Rgba, RgbaImage};
use overlay::{clear_cells_under, draw_overlay, load_overlay, over, overlay_box};
use render::{border_width, cell_shapes, paint_shapes};
use shade::cell_colours;
use std::time::SystemTime;

pub use overlay::{MAX_OVERLAY_PIXELS, Overlay};

/// How the triangles are drawn. Every style decodes the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageStyle {
    /// Triangles with a 1-pixel gap (border excluded) between neighbours.
    Tiles,
    /// Triangles filled edge-to-edge with no gap.
    Stain,
    /// Triangles of the same colour joined into diagonal groups, each drawn as one shape with a
    /// border around it.
    Horizon,
}

/// Parameters for [`generate_image_debug`].
pub struct GenerateImageParams<'a> {
    /// The text to encode.
    pub payload: &'a str,
    /// What the payload is, such as `"text"` or `"url"`; stored in the code and returned as
    /// [`DecodeDebug::content_type`](crate::DecodeDebug::content_type).
    pub payload_type: &'a str,
    /// Width of the image's aspect ratio, such as 16 in 16:9.
    pub width_ratio: u32,
    /// Height of the image's aspect ratio.
    pub height_ratio: u32,
    /// How the triangles are drawn.
    pub style: ImageStyle,
    /// Colour of the primary triangles (bit 1), as ARGB (`0xAARRGGBB`); always drawn opaque.
    pub primary_color: u32,
    /// Colour of the secondary triangles (bit 0), as ARGB; drawn over the background, so a
    /// translucent one tints it and a transparent one leaves it showing. Usually the background
    /// colour, which looks as if only the primary triangles were drawn.
    pub secondary_color: u32,
    /// Colour of the margin and the borders between shapes, as ARGB; may be transparent.
    pub bg_color: u32,
    /// Seed of the Horizon grouping and of colour variance; 0 picks one from the clock.
    pub seed: u64,
    /// Shifts every primary shape's colour a little in brightness (by 0 to 7 points of HSV value), picked by
    /// `seed`.
    pub primary_variance: bool,
    /// The same for the secondary shapes.
    pub secondary_variance: bool,
    /// Index into [`crate::MASK_NAMES`] of the mask to XOR the data with, for comparing masks.
    /// `None` (or an out-of-range index) picks the best one, as normal generation does.
    pub mask: Option<usize>,
    /// Triangle base width in pixels: odd, within [`crate::MIN_TRIANGLE_WIDTH`]..=
    /// [`crate::MAX_TRIANGLE_WIDTH`]. Usually [`crate::DEFAULT_TRIANGLE_WIDTH`]; larger values give
    /// a higher-resolution image.
    pub triangle_width: u32,
    /// How much damage the code survives; higher levels need a larger grid.
    pub ecc_level: EccLevel,
    /// An image drawn over the centre of the code, if any.
    pub overlay: Option<Overlay<'a>>,
}

/// Debug information collected during image generation.
#[derive(Debug, Clone)]
pub struct GenerateDebug {
    /// The generated code as a PNG.
    pub png_bytes: Vec<u8>,
    /// Bytes after Reed-Solomon encoding.
    pub encoded_bytes: usize,
    /// Rows of triangles in the grid.
    pub grid_rows: u32,
    /// Columns of triangles in the grid.
    pub grid_columns: u32,
    /// Bits written from the encoded bytes.
    pub data_bits_used: usize,
    /// Zero-fill bits that complete the grid.
    pub padding_bits: usize,
    /// Index into [`MASK_NAMES`](crate::MASK_NAMES) of the mask the data was XORed with.
    pub mask_index: usize,
    /// Shared triangle edges between cells of different colour.
    pub colour_transitions: usize,
    /// All triangle edges shared by two cells.
    pub shared_edges: usize,
    /// Why the generated code doesn't read back; `None` when it decodes to the payload.
    pub decode_warning: Option<String>,
}

/// Generates a PNG image encoding the payload text, returning full debug info.
pub fn generate_image_debug(params: GenerateImageParams<'_>) -> Result<GenerateDebug, String> {
    validate_triangle_width(params.triangle_width)?;
    let payload_bytes = pack_payload(params.payload_type, params.payload)?;
    let mut debug = create_image_from_payload(&payload_bytes, &params)?;
    debug.decode_warning = decode_warning(&debug.png_bytes, &params);
    Ok(debug)
}

/// Reads a generated code back and says why it doesn't decode, if it doesn't. Colours the
/// decoder can't tell apart are caught first, which skips a slow decode that is bound to fail.
fn decode_warning(png: &[u8], params: &GenerateImageParams<'_>) -> Option<String> {
    // Converted exactly as the decoder converts the image, which ignores alpha.
    let luma = |Rgba([r, g, b, _]): Rgba<u8>| {
        let pixel = RgbaImage::from_pixel(1, 1, Rgba([r, g, b, 255]));
        image::DynamicImage::ImageRgba8(pixel).to_luma8()[(0, 0)][0]
    };
    let [primary, secondary, bg] = code_colours(params);
    let (primary, secondary) = (luma(primary), luma(over(secondary, bg)));
    if primary.abs_diff(secondary) < MIN_LUMA_GAP {
        return Some(format!(
            "The primary and secondary triangle colors are too close in brightness to tell apart \
             ({primary} and {secondary} of 255; they need to be at least {MIN_LUMA_GAP} apart)."
        ));
    }
    // Only the decoder's first attempt: a pristine image that fails it fails every fallback too.
    let decodes = decode_rendered(png).as_deref() == Some(params.payload);
    match (decodes, params.overlay.is_some()) {
        (true, _) => None,
        // The overlay's share is kept within what the level repairs, but what it covers varies.
        (false, true) => Some(
            "This code doesn't decode with its logo. Make the logo smaller or raise the \
             error correction."
                .to_string(),
        ),
        (false, false) => Some("This code doesn't decode.".to_string()),
    }
}

/// The primary, secondary and background colours; the secondary is drawn composited over the
/// background.
fn code_colours(params: &GenerateImageParams<'_>) -> [Rgba<u8>; 3] {
    [
        argb_to_rgba(params.primary_color, true),
        argb_to_rgba(params.secondary_color, false),
        argb_to_rgba(params.bg_color, false),
    ]
}

/// `seed`, or the current unix-time seconds for 0.
fn resolve_seed(seed: u64) -> u64 {
    if seed != 0 {
        return seed;
    }
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(42)
}

/// Convert a Flutter/Android-style ARGB `u32` (`0xAARRGGBB`) to an `Rgba<u8>`.
/// If `force_opaque` is true the alpha channel is set to 255 regardless.
fn argb_to_rgba(argb: u32, force_opaque: bool) -> Rgba<u8> {
    let [a, r, g, b] = argb.to_be_bytes();
    Rgba([r, g, b, if force_opaque { 255 } else { a }])
}

/// The grid a payload is drawn on, and the bits that go in it.
struct GridData {
    rows: u32,
    cols: u32,
    /// The frame's bytes, before padding and masking.
    encoded_bytes: usize,
    /// Every data bit of the grid: the frame, zero-padded to the grid and masked.
    data: Vec<u8>,
    mask_index: usize,
    padding_bits: usize,
}

/// Sizes the grid for `payload` at the requested aspect ratio and level, frames the payload into
/// it and masks the bits.
fn plan_grid(payload: &[u8], params: &GenerateImageParams<'_>) -> Result<GridData, String> {
    if payload.len() > u16::MAX as usize {
        return Err(format!("payload of {} bytes is too long", payload.len()));
    }
    let gcd_val = gcd(params.width_ratio, params.height_ratio);
    let (cols, rows) = grid_for_payload(
        payload.len(),
        params.width_ratio / gcd_val,
        params.height_ratio / gcd_val,
        params.ecc_level,
    );
    check_image_pixels(cols, rows, params.triangle_width)?;

    // The codewords fill every whole byte of the grid. The few cells left over after the last
    // byte are zero-padded *before* mixing, so the mask also covers them instead of leaving them
    // blank.
    let capacity = grid_capacity(rows as usize, cols as usize);
    let encoded = encode_frame(payload, capacity, params.ecc_level)?;
    let data_capacity = ((cols * rows) as usize).saturating_sub(8);
    let mut padded = encoded.clone();
    padded.resize(data_capacity.div_ceil(8), 0);
    let (data, mask_index) = mix_pixels(&padded, cols, rows, params.mask);

    Ok(GridData {
        rows,
        cols,
        encoded_bytes: encoded.len(),
        data,
        mask_index,
        padding_bits: data_capacity - encoded.len() * 8,
    })
}

pub(crate) fn create_image_from_payload(
    payload: &[u8],
    params: &GenerateImageParams<'_>,
) -> Result<GenerateDebug, String> {
    let colours = code_colours(params);
    let seed = resolve_seed(params.seed);
    let overlay = params
        .overlay
        .map(|overlay| load_overlay(overlay, params.ecc_level))
        .transpose()?;
    let grid_data = plan_grid(payload, params)?;
    let GridData { rows, cols, .. } = grid_data;

    let cells = layout_cells(&grid_data.data, rows, cols);
    let (colour_transitions, shared_edges) = colour_transitions(&cells);
    let horizon_layouts = (params.style == ImageStyle::Horizon).then(|| {
        let secondary_cells: Vec<Vec<bool>> = cells
            .iter()
            .map(|row| row.iter().map(|&primary| !primary).collect())
            .collect();
        [&cells, &secondary_cells].map(|members| HorizonLayout::from_seed(seed, members))
    });
    let mut shapes = cell_shapes(&cells, params.style, horizon_layouts.as_ref());

    let (width, height) = image_size(cols, rows, params.triangle_width);
    let grid = TriangleGrid::new(rows, cols, params.triangle_width);
    let overlay = overlay.map(|(image, share)| {
        let area = overlay_box(&image, share, width, height);
        clear_cells_under(&mut shapes, &grid, &area);
        (image, area)
    });

    let bg = colours[2];
    let vary = [params.primary_variance, params.secondary_variance];
    let cell_colours = cell_colours(&shapes, colours, seed, vary);
    let mut img = RgbaImage::from_pixel(width, height, bg);
    let border = border_width(params.style, params.triangle_width);
    paint_shapes(&mut img, &grid, &shapes, border, &cell_colours, bg);
    if let Some((image, area)) = &overlay {
        draw_overlay(&mut img, image, area, params.triangle_width);
    }

    Ok(GenerateDebug {
        png_bytes: encode_png(&img)?,
        encoded_bytes: grid_data.encoded_bytes,
        grid_rows: rows,
        grid_columns: cols,
        data_bits_used: grid_data.encoded_bytes * 8,
        padding_bits: grid_data.padding_bits,
        mask_index: grid_data.mask_index,
        colour_transitions,
        shared_edges,
        decode_warning: None,
    })
}
