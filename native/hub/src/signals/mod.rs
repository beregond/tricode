use rinf::{DartSignal, RustSignal, SignalPiece};
use serde::{Deserialize, Serialize};

/// To send data from Dart to Rust, use `DartSignal`.
#[derive(Deserialize, DartSignal)]
pub struct SmallText {
    pub text: String,
    pub width_ratio: u32,
    pub height_ratio: u32,
}

/// To send data from Rust to Dart, use `RustSignal`.
#[derive(Serialize, RustSignal)]
pub struct SmallNumber {
    pub number: i32,
}

/// Bytes generated from the `tricode` crate.
#[derive(Serialize, RustSignal)]
pub struct GeneratedBytes {
    pub bytes: Vec<u8>,
}

/// PNG image bytes generated from an input payload.
#[derive(Serialize, RustSignal)]
pub struct GeneratedImage {
    pub png_bytes: Vec<u8>,
}

/// A signal can be nested inside another signal.
#[derive(Serialize, RustSignal)]
pub struct BigBool {
    pub member: bool,
    pub nested: SmallBool,
}

/// To nest a signal inside other signal, use `SignalPiece`.
#[derive(Serialize, SignalPiece)]
pub struct SmallBool(pub bool);
