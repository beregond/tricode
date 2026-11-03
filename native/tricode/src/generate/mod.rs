//! Rendering a payload as a PNG of triangles.

mod geometry;
mod horizon;
mod overlay;
mod render;

use crate::decode::{MIN_LUMA_GAP, decode_image};
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
use overlay::{clear_cells_under, draw_overlay, load_overlay, overlay_box};
use render::{border_width, cell_shapes, paint_shapes};

pub use overlay::{MAX_OVERLAY_PIXELS, Overlay};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageStyle {
    /// Black triangles with a 1-pixel gap (border excluded) between neighbours.
    Tiles,
    /// Black triangles filled edge-to-edge with no gap.
    Stain,
    /// Foreground triangles joined into diagonal groups, each drawn as one shape with a border
    /// around it (see [`HorizonLayout`]).
    Horizon,
}

impl ImageStyle {
    /// 0 = Tiles, 1 = Stain, 2 = Horizon; anything else is Tiles.
    pub fn from_u32(v: u32) -> Self {
        match v {
            1 => Self::Stain,
            2 => Self::Horizon,
            _ => Self::Tiles,
        }
    }
}

/// Parameters for [`generate_image_debug`].
pub struct GenerateImageParams<'a> {
    pub payload: &'a str,
    pub payload_type: &'a str,
    pub width_ratio: u32,
    pub height_ratio: u32,
    pub style: ImageStyle,
    /// Triangle colour, as ARGB (`0xAARRGGBB`); always drawn opaque.
    pub fg_color: u32,
    /// Background colour, as ARGB (`0xAARRGGBB`); may be transparent.
    pub bg_color: u32,
    /// Seed of the Horizon grouping; 0 picks one from the clock.
    pub horizon_seed: u64,
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
    pub png_bytes: Vec<u8>,
    pub encoded_bytes: usize, // bytes after Reed-Solomon encoding
    pub grid_rows: u32,
    pub grid_columns: u32,
    pub data_bits_used: usize, // bits actually written from encoded_data
    pub padding_bits: usize,   // zero-fill bits to complete the grid
    pub mask_index: usize,     // index into MASKS of the mask the data was XORed with
    pub colour_transitions: usize, // shared triangle edges between cells of different colour
    pub shared_edges: usize,   // all triangle edges shared by two cells
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
    // Converted exactly as the decoder converts the image.
    let luma = |argb| {
        let pixel = RgbaImage::from_pixel(1, 1, argb_to_rgba(argb, true));
        image::DynamicImage::ImageRgba8(pixel).to_luma8()[(0, 0)][0]
    };
    let (fg, bg) = (luma(params.fg_color), luma(params.bg_color));
    if fg.abs_diff(bg) < MIN_LUMA_GAP {
        return Some(format!(
            "The triangle and background colours are too close in brightness to tell apart \
             ({fg} and {bg} of 255; they need to be at least {MIN_LUMA_GAP} apart)."
        ));
    }
    let decodes = decode_image(png).is_ok_and(|d| d.text.as_deref() == Some(params.payload));
    match (decodes, params.overlay.is_some()) {
        (true, _) => None,
        // The overlay's share is kept within what the level repairs, but what it covers varies.
        (false, true) => Some(
            "This code doesn't decode with its overlay. Make the overlay smaller or raise the \
             error correction."
                .to_string(),
        ),
        (false, false) => Some("This code doesn't decode.".to_string()),
    }
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
    let fg = argb_to_rgba(params.fg_color, true);
    let bg = argb_to_rgba(params.bg_color, false);
    let overlay = params
        .overlay
        .map(|overlay| load_overlay(overlay, params.ecc_level))
        .transpose()?;
    let grid_data = plan_grid(payload, params)?;
    let GridData { rows, cols, .. } = grid_data;

    let cells = layout_cells(&grid_data.data, rows, cols);
    let (colour_transitions, shared_edges) = colour_transitions(&cells);
    let horizon_layout = (params.style == ImageStyle::Horizon)
        .then(|| HorizonLayout::from_seed(rows, cols, params.horizon_seed, &grid_data.data));
    let mut shapes = cell_shapes(&cells, params.style, horizon_layout.as_ref());

    let (width, height) = image_size(cols, rows, params.triangle_width);
    let grid = TriangleGrid::new(rows, cols, params.triangle_width);
    let overlay = overlay.map(|(image, share)| {
        let area = overlay_box(&image, share, width, height);
        clear_cells_under(&mut shapes, &grid, &area);
        (image, area)
    });

    let mut img = RgbaImage::from_pixel(width, height, bg);
    let border = border_width(params.style, params.triangle_width);
    paint_shapes(&mut img, &grid, &shapes, border, fg, bg);
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
