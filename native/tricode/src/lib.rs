//! Tricode: a 2D code drawn as a grid of interlocking triangles.
//!
//! - [`generate_image_debug`] packs a payload, frames it with Reed-Solomon codewords
//!   ([`frame`]), masks the bits ([`mask`]) and renders them as triangles ([`generate`]).
//! - [`decode_image`] finds the triangles in an image, fits the grid, reads the bits back and
//!   undoes every step ([`decode`]).

mod decode;
mod frame;
mod generate;
mod grid;
mod mask;
mod payload;
mod png;
mod reed_solomon;

#[cfg(test)]
mod tests;

pub use decode::{DecodeDebug, MIN_LUMA_GAP, decode_image};
pub use frame::{DEFAULT_ECC_LEVEL, EccLevel};
pub use generate::{
    GenerateDebug, GenerateImageParams, ImageStyle, MAX_OVERLAY_PIXELS, Overlay,
    generate_image_debug,
};
pub use grid::{DEFAULT_TRIANGLE_WIDTH, MAX_IMAGE_PIXELS, MAX_TRIANGLE_WIDTH, MIN_TRIANGLE_WIDTH};
pub use mask::MASK_NAMES;
