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
    /// Debug: index into `tricode::MASK_NAMES` of the mask to XOR the data with;
    /// `None` picks the most balanced mask, as normal generation does
    pub mask: Option<u32>,
    /// Triangle base width in pixels: odd, between `tricode::MIN_TRIANGLE_WIDTH` and
    /// `tricode::MAX_TRIANGLE_WIDTH`; `tricode::DEFAULT_TRIANGLE_WIDTH` normally
    pub triangle_width: u32,
    /// Error correction: 0 = Low (~7% of the code can be damaged), 1 = Medium (~15%),
    /// 2 = Quartile (~25%), 3 = High (~30%). Higher levels make the grid larger.
    pub ecc_level: u32,
    /// PNG bytes of an image drawn over the centre of the code (a logo, or text rendered by
    /// Dart); empty for none
    pub overlay_png: Vec<u8>,
    /// Share of the code's area the overlay takes: above 0 and at most
    /// `tricode::EccLevel::max_overlay_share` of the level; ignored without an overlay
    pub overlay_share: f64,
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
    pub decode_method: String, // e.g. "lattice-fit", "naive ±5 (flipped horizontally)", or "all failed"
    pub ecc_level: String,     // "Low", "Medium", "Quartile" or "High"; empty on failure
    pub corrected_bytes: u32,  // bytes the error correction repaired
    pub erased_bytes: u32,     // bytes read from cells the decoder was unsure about
}

/// PNG image bytes generated from an input payload, or why generation failed.
#[derive(Serialize, RustSignal)]
pub struct GeneratedImage {
    pub png_bytes: Vec<u8>, // empty on failure
    pub error: String,      // empty on success; the other fields are zero on failure
    // debug info
    pub encoded_bytes: u32, // bytes after Reed-Solomon encoding
    pub grid_rows: u32,
    pub grid_columns: u32,
    pub data_bits_used: u32, // bits written from encoded data
    pub padding_bits: u32,   // zero-fill bits to complete the grid
    pub mask_index: u32,     // index into `tricode::MASK_NAMES` of the mask that was used
    pub mask_name: String,
    pub colour_transitions: u32, // shared triangle edges between cells of different colour
    pub shared_edges: u32,       // all triangle edges shared by two cells
    pub decode_warning: String,  // why the code doesn't read back; empty when it decodes
}

/// One `tracing` event from Rust (hub or tricode), forwarded to the Dart log.
#[derive(Serialize, RustSignal)]
pub struct RustLog {
    /// "TRACE", "DEBUG", "INFO", "WARN" or "ERROR"
    pub level: String,
    /// Module path of the event, e.g. "tricode" or "hub::actors::decode"
    pub target: String,
    /// The message, prefixed by its enclosing spans and followed by its `key=value` fields
    pub message: String,
}

/// Replaces which Rust events reach Dart, e.g. "info,tricode=trace" (`tracing_subscriber`'s
/// `Targets` syntax). Release builds drop debug and trace events at compile time.
#[derive(Deserialize, DartSignal)]
pub struct RustLogFilter {
    pub directives: String,
}
