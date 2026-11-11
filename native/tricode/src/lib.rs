//! Tricode: a 2D code drawn as a grid of interlocking triangles.
//!
//! - [`generate_image_debug`] packs a payload, frames it with Reed-Solomon codewords, masks the
//!   bits and renders them as triangles into PNG bytes.
//! - [`decode_image`] finds the triangles in an image, fits the grid, reads the bits back and
//!   undoes every step. It reads mirrored codes, inverted colours and codes with a centre
//!   [`Overlay`].
//!
//! ```no_run
//! use tricode::{
//!     DEFAULT_ECC_LEVEL, DEFAULT_TRIANGLE_WIDTH, GenerateImageParams, ImageStyle, decode_image,
//!     generate_image_debug,
//! };
//!
//! let code = generate_image_debug(GenerateImageParams {
//!     payload: "https://github.com/beregond/tricode",
//!     payload_type: "url",
//!     width_ratio: 1,
//!     height_ratio: 1,
//!     style: ImageStyle::Horizon,
//!     primary_color: 0xFF000000,
//!     secondary_color: 0xFFFFFFFF,
//!     bg_color: 0xFFFFFFFF,
//!     seed: 0,
//!     primary_variance: false,
//!     secondary_variance: false,
//!     mask: None,
//!     triangle_width: DEFAULT_TRIANGLE_WIDTH,
//!     ecc_level: DEFAULT_ECC_LEVEL,
//!     overlay: None,
//! })?;
//! std::fs::write("code.png", &code.png_bytes)?;
//!
//! let read = decode_image(&std::fs::read("code.png")?)?;
//! assert_eq!(read.text.as_deref(), Some("https://github.com/beregond/tricode"));
//! assert_eq!(read.content_type, "url");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

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
