//! Reading a code back from an image: black-and-white conversion, triangle detection, grid search
//! and error correction, retried over cut-offs, polarities and erosion radii.

mod annotate;
mod count;
mod detect;
mod lattice;
mod read;
mod search;

use crate::frame::EccLevel;
use crate::png::encode_png;
use annotate::draw_rectangles;
use count::{calculate_column_count, calculate_row_count};
use detect::{Anchors, Triangle, discard_merged_triangles, find_anchors, find_triangles};
use image::{GrayImage, RgbaImage};
use imageproc::contrast::{ThresholdType, otsu_level, threshold};
use lattice::fit_lattice;
use search::{GridContext, try_grid_strategies};
use tracing::{instrument, trace};

#[cfg(test)]
pub(crate) use read::FLIPS;

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
    /// "lattice-fit", "naive", "cluster+pitch", any of them with a " ±5" suffix when found in the
    /// neighbourhood, or "all failed". A flip, a gentler erosion, a negative image (light triangles
    /// on dark) and the fallback cut-off each add a suffix, in that order: " (flipped …)",
    /// " (erosion N)", " (inverted)", " (fixed cut-off)".
    pub decode_method: String,
    /// Error-correction level the code was read at; `None` on failure.
    pub ecc_level: Option<EccLevel>,
    /// Bytes the error correction repaired: how damaged the code was.
    pub corrected_bytes: usize,
    /// Bytes read from cells the decoder was unsure about (see `UNSURE_DARKNESS`).
    pub erased_bytes: usize,
}

/// Mid-grey: the decoder's fallback cut-off between triangle and background. The decoder works on a
/// black-and-white image, so after the split pixels below this are dark (part of a triangle).
const DARK_LUMA: u8 = 128;

/// Least brightness difference between the triangle and background colours a generated code may
/// have. A clean PNG reads with much less; this leaves room for screens, print and cameras. A guess,
/// not measured on photos.
pub const MIN_LUMA_GAP: u8 = 24;

/// Erosion radii (in pixels) tried in order. Erosion peels this many pixels off every black shape
/// so that touching triangles (Stain style, or bridges left by blur) come apart before the blobs are
/// traced. Two pixels suit the default triangle size, but small triangles don't survive it, so the
/// decoder falls back to gentler erosion when a decode fails.
pub(crate) const EROSION_RADII: [u8; 3] = [2, 1, 0];

/// Decodes a code from an image in any format the `image` crate was built with (PNG only).
#[instrument(level = "debug", skip_all, fields(bytes = png_bytes.len()))]
pub fn decode_image(png_bytes: &[u8]) -> Result<DecodeDebug, String> {
    if png_bytes.is_empty() {
        return Err("Empty image data".to_string());
    }
    let img = image::load_from_memory(png_bytes)
        .map_err(|e| format!("Failed to load image: {}", e))?
        .to_rgba8();
    let gray_img = image::DynamicImage::ImageRgba8(img.clone()).to_luma8();

    // Try the strongest erosion first: it is what default-sized images need, so they decode on the
    // first attempt. Smaller erosions are only tried when that fails.
    let mut first_failure: Option<DecodeAttempt> = None;
    let mut failures: Vec<String> = Vec::new();
    for (cut_off, is_fixed) in cut_offs(&gray_img) {
        for (binary, is_inverted) in polarities(&gray_img, cut_off) {
            let attempt_name = format!(
                "cut-off {}{}",
                cut_off,
                if is_inverted { ", inverted" } else { "" }
            );
            for radius in EROSION_RADII {
                match decode_with_erosion(&binary, radius) {
                    Ok(mut attempt) if attempt.debug.text.is_some() => {
                        if radius != EROSION_RADII[0] {
                            attempt.debug.decode_method += &format!(" (erosion {})", radius);
                        }
                        if is_inverted {
                            attempt.debug.decode_method += " (inverted)";
                        }
                        if is_fixed {
                            attempt.debug.decode_method += " (fixed cut-off)";
                        }
                        return attempt.with_annotation(&img);
                    }
                    Ok(attempt) => {
                        let error = attempt.debug.error.clone().unwrap_or_default();
                        failures.push(format!("{attempt_name}, erosion {radius}:\n{error}"));
                        first_failure.get_or_insert(attempt);
                    }
                    Err(e) => failures.push(format!("{attempt_name}, erosion {radius}: {e}")),
                }
            }
        }
    }

    // Every attempt failed — report the first one that got as far as reading a grid.
    let failures = failures.join("\n");
    match first_failure {
        Some(mut attempt) => {
            attempt.debug.error = Some(format!("All attempts failed.\n{}", failures));
            attempt.with_annotation(&img)
        }
        None => Err(failures),
    }
}

/// Cut-offs to split pixels into triangle and background at, as `(level, is_fixed)`.
///
/// First the cut-off between the image's two brightness peaks (Otsu's method), so any two colours
/// far enough apart are read, not only ones on either side of mid-grey. The fixed mid-grey cut-off
/// stays as a fallback for photos where something around the code skews the histogram.
fn cut_offs(gray_img: &GrayImage) -> Vec<(u8, bool)> {
    let otsu = otsu_level(gray_img);
    let mut cut_offs = vec![(otsu, false)];
    if splits_differently(gray_img, otsu, DARK_LUMA) {
        cut_offs.push((DARK_LUMA, true));
    }
    cut_offs
}

/// The black-and-white images (black = triangle) to read at `cut_off`, as `(image, is_inverted)`,
/// in the order to try them.
///
/// A negative code (light triangles on a dark background) is read inverted. The margin around a
/// code is background, so a dark edge suggests trying the negative first; the other polarity is
/// still tried in case the guess is wrong (a photo with dark surroundings).
fn polarities(gray_img: &GrayImage, cut_off: u8) -> [(GrayImage, bool); 2] {
    let binary = threshold(gray_img, cut_off, ThresholdType::Binary);
    let inverted = threshold(gray_img, cut_off, ThresholdType::BinaryInverted);
    if edge_luminance(&binary) < DARK_LUMA as f32 {
        [(inverted, true), (binary, false)]
    } else {
        [(binary, false), (inverted, true)]
    }
}

/// Mean luminance of the image's outermost pixels. Generated codes have a margin of background
/// there, so it tells a light background from a dark one.
fn edge_luminance(img: &GrayImage) -> f32 {
    let (width, height) = img.dimensions();
    let edge = img
        .enumerate_pixels()
        .filter(|&(x, y, _)| x == 0 || y == 0 || x == width - 1 || y == height - 1);
    let (sum, count) = edge.fold((0u64, 0u64), |(sum, count), (_, _, p)| {
        (sum + p[0] as u64, count + 1)
    });
    sum as f32 / count.max(1) as f32
}

/// Whether two cut-offs put more than 1% of the pixels on different sides. A rendered code is
/// almost all pure colour, so any cut-off between its two colours splits it the same way, and
/// trying a second one would only double the time a failing decode takes.
fn splits_differently(img: &GrayImage, a: u8, b: u8) -> bool {
    let (low, high) = (a.min(b), a.max(b));
    let between = img.pixels().filter(|p| low < p[0] && p[0] <= high).count();
    between * 100 > img.pixels().len()
}

/// One decode attempt at a given erosion radius, before its annotated image is drawn.
struct DecodeAttempt {
    /// `annotated_png` is left empty; [`DecodeAttempt::with_annotation`] fills it in.
    debug: DecodeDebug,
    triangles: Vec<Triangle>,
    anchors: Anchors,
}

impl DecodeAttempt {
    /// Draws the detected triangles over `img`. Done only for the attempt that is returned.
    fn with_annotation(self, img: &RgbaImage) -> Result<DecodeDebug, String> {
        let mut annotated_img = img.clone();
        draw_rectangles(&mut annotated_img, &self.triangles, &self.anchors);
        Ok(DecodeDebug {
            annotated_png: encode_png(&annotated_img)?,
            ..self.debug
        })
    }
}

/// Detects triangles with the given erosion radius and tries every grid strategy on them.
/// Returns `Err` only when no anchors are found; a failed decode is `Ok` with `error` set.
#[instrument(level = "debug", skip(gray_img))]
fn decode_with_erosion(gray_img: &GrayImage, erosion: u8) -> Result<DecodeAttempt, String> {
    let triangles = find_triangles(gray_img, erosion)?;
    let anchors = find_anchors(&triangles).ok_or("Could not find anchor triangles")?;
    let triangles = discard_merged_triangles(triangles, &anchors);
    trace!(erosion, ?anchors, "anchors found");

    let ctx = GridContext {
        triangles: &triangles,
        anchors: &anchors,
        row_naive: calculate_row_count(&anchors),
        col_naive: calculate_column_count(&anchors),
        fit: fit_lattice(&triangles, &anchors),
    };
    let debug = try_grid_strategies(gray_img, &ctx);
    Ok(DecodeAttempt {
        debug,
        triangles,
        anchors,
    })
}
