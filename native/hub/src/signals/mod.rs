use rinf::{DartSignal, RustSignal};
use serde::{Deserialize, Serialize};

/// To send data from Dart to Rust, use `DartSignal`.
#[derive(Deserialize, DartSignal)]
pub struct SmallText {
    pub text: String,
    pub width_ratio: u32,
    pub height_ratio: u32,
    /// 0 = Tiles, 1 = Stain, 2 = Horizon
    pub style: u32,
    /// "text", "url", or "plain"
    pub payload_type: String,
    /// Foreground (triangle) color — ARGB u32, alpha is ignored (always fully opaque)
    pub fg_color: u32,
    /// Background color — ARGB u32, alpha 0 means fully transparent
    pub bg_color: u32,
    /// Seed controlling the Horizon style layout; 0 means "use current unix time"
    pub horizon_seed: u64,
}

/// Image bytes to be decoded.
#[derive(Deserialize, DartSignal)]
pub struct ImageBytes {
    pub bytes: Vec<u8>,
}

/// Result of decoding an image — either the recovered text or a failure reason.
/// Debug fields are always populated when the image could be parsed far enough.
#[derive(Serialize, RustSignal)]
pub struct DecodeResult {
    pub success: bool,
    pub text: String,
    pub error: String,
    /// "text", "url", or "plain" (fallback) — extracted from the msgpack envelope on success, empty on failure
    pub content_type: String,
    // debug info (only meaningful in debug builds / when kDebugMode is true in Dart)
    pub image_width: u32,
    pub image_height: u32,
    pub total_triangles: u32,
    pub row_count: u32,
    pub column_count: u32,
    pub bit_count: u32,
    pub annotated_image: Vec<u8>, // PNG bytes of annotated image, empty on early failure
    pub decode_method: String,    // "naive", "cluster+pitch", "brute-force ±1", or "all failed"
}

/// PNG image bytes generated from an input payload.
#[derive(Serialize, RustSignal)]
pub struct GeneratedImage {
    pub png_bytes: Vec<u8>,
    // debug info
    pub encoded_bytes: u32, // bytes after Reed-Solomon encoding
    pub grid_rows: u32,
    pub grid_columns: u32,
    pub data_bits_used: u32, // bits written from encoded data
    pub padding_bits: u32,   // zero-fill bits to complete the grid
}
