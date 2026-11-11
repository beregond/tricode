//! Reading bits off a grid: sampling each cell, undoing flips, masks and error correction.

use super::DARK_LUMA;
use super::lattice::Lattice;
use crate::frame::{EccLevel, decode_frame};
use crate::grid::{grid_capacity, is_orientation_mark, pack_bits};
use crate::mask::unmix_pixels;
use crate::payload::unpack_payload;
use image::GrayImage;
use tracing::trace;

/// A cell whose sampled darkness falls strictly between these is neither clearly a triangle nor
/// clearly background (the edge of a logo, a scratch, text drawn over the code), so the byte it
/// belongs to is treated as an erasure.
const UNSURE_DARKNESS: (f32, f32) = (0.2, 0.8);

/// Share of the column pitch sampled around each centroid: well inside the triangle body.
const SAMPLE_RADIUS_SHARE: f32 = 0.15;

/// A mirror image of the code: which axes the image was flipped along.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Flip {
    pub(crate) horizontal: bool,
    pub(crate) vertical: bool,
}

impl Flip {
    /// Appended to `decode_method` when the code was read through this flip.
    pub(crate) fn name(self) -> Option<&'static str> {
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
pub(crate) const FLIPS: [Flip; 4] = [
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
pub(crate) fn sample_cells(
    img: &GrayImage,
    row_count: usize,
    column_count: usize,
    lattice: &Lattice,
) -> Vec<Vec<f32>> {
    if row_count < 2 || column_count < 2 {
        return Vec::new();
    }
    let sample_radius = ((lattice.col_pitch * SAMPLE_RADIUS_SHARE).round() as u32).max(2);
    trace!(
        row_count,
        column_count,
        origin = ?(lattice.x0[0], lattice.y0[0]),
        step = ?(lattice.col_pitch, lattice.row_pitch),
        sample_radius,
        "sampling grid"
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

/// Fraction of the pixels in a small square around (cx, cy) that are dark (< [`DARK_LUMA`]).
fn sample_region_darkness(img: &GrayImage, cx: f32, cy: f32, radius: u32) -> f32 {
    let r = radius as i32;
    let x0 = (cx as i32 - r).max(0) as u32;
    let y0 = (cy as i32 - r).max(0) as u32;
    let x1 = ((cx as i32 + r).max(0) as u32).min(img.width().saturating_sub(1));
    let y1 = ((cy as i32 + r).max(0) as u32).min(img.height().saturating_sub(1));

    let (mut dark, mut total) = (0u32, 0u32);
    for y in y0..=y1 {
        for x in x0..=x1 {
            dark += u32::from(img.get_pixel(x, y)[0] < DARK_LUMA);
            total += 1;
        }
    }
    if total == 0 {
        return 0.0;
    }
    dark as f32 / total as f32
}

/// Data bits read off a grid.
pub(crate) struct PackedBits {
    /// Packed MSB first; the last byte may be partly padding.
    pub(crate) bytes: Vec<u8>,
    /// Per byte: whether any of its cells was unsure (see [`UNSURE_DARKNESS`]).
    pub(crate) erased: Vec<bool>,
    pub(crate) bit_count: usize,
}

/// Reads the data bits out of sampled `cells` in the order the renderer wrote them, undoing
/// `flip`. The orientation-mark cells are skipped: they are not data bits.
pub(crate) fn pack_data_bits(cells: &[Vec<f32>], flip: Flip) -> PackedBits {
    let row_count = cells.len();
    let column_count = cells.first().map_or(0, Vec::len);
    let mirror = |i: usize, count: usize, flipped: bool| if flipped { count - 1 - i } else { i };

    let mut bits: Vec<bool> = Vec::new();
    let mut unsure: Vec<bool> = Vec::new();
    for row in 0..row_count {
        for col in 0..column_count {
            if is_orientation_mark(row, col, row_count, column_count) {
                continue;
            }
            let image_row = mirror(row, row_count, flip.vertical);
            let image_col = mirror(col, column_count, flip.horizontal);
            let darkness = cells[image_row][image_col];
            bits.push(darkness > 0.5);
            unsure.push(UNSURE_DARKNESS.0 < darkness && darkness < UNSURE_DARKNESS.1);
        }
    }

    PackedBits {
        bytes: pack_bits(&bits),
        erased: unsure
            .chunks(8)
            .map(|chunk| chunk.contains(&true))
            .collect(),
        bit_count: bits.len(),
    }
}

/// A decoded payload and how it was recovered.
pub(crate) struct Decoded {
    pub(crate) kind: String,
    pub(crate) text: String,
    pub(crate) level: EccLevel,
    pub(crate) corrected_bytes: usize,
}

/// Tries every mask and error-correction level on `packed` until a frame decodes and its payload
/// unpacks.
pub(crate) fn try_unmix_decode(
    packed: &PackedBits,
    rows: usize,
    cols: usize,
) -> Result<Decoded, String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GenerateImageParams;
    use crate::decode::EROSION_RADII;
    use crate::decode::detect::{discard_merged_triangles, find_anchors, find_triangles};
    use crate::decode::lattice::fit_lattice;
    use crate::generate::generate_image_debug;
    use crate::mask::{MASK_NAMES, MASKS};
    use crate::tests::{params, payload};
    use test_log::test;

    #[test]
    fn masks_cover_the_padding_at_the_end_of_the_grid() {
        for (index, name) in MASK_NAMES.iter().enumerate() {
            // The codewords fill whole bytes, so the cells after the last byte are padding.
            let generated = generate_image_debug(GenerateImageParams {
                width_ratio: 16,
                height_ratio: 9,
                mask: Some(index),
                ..params(&payload(5))
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
}
