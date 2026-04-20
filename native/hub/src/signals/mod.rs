use rinf::{DartSignal, RustSignal};
use serde::{Deserialize, Serialize};

/// To send data from Dart to Rust, use `DartSignal`.
#[derive(Deserialize, DartSignal)]
pub struct SmallText {
    pub text: String,
    pub width_ratio: u32,
    pub height_ratio: u32,
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
    // debug info (only meaningful in debug builds / when kDebugMode is true in Dart)
    pub image_width: u32,
    pub image_height: u32,
    pub total_triangles: u32,
    pub row_count: u32,
    pub column_count: u32,
    pub bit_count: u32,
    pub annotated_image: Vec<u8>, // PNG bytes of annotated image, empty on early failure
}

/// PNG image bytes generated from an input payload.
#[derive(Serialize, RustSignal)]
pub struct GeneratedImage {
    pub png_bytes: Vec<u8>,
}
