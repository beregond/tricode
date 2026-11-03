//! Bit masks XORed over the grid, so the code shows fine texture instead of solid patches.

use crate::grid::{layout_cells, points_down};

/// Applies a 2D bitmask (tiled over the grid) to `data` via XOR.
/// Each bit at grid position `(row, col)` is flipped when `mask[row % mask_rows][col % mask_cols] == 1`.
fn apply_mask(data: &[u8], mask: &[&[u8]], width_squares: u32) -> Vec<u8> {
    let mask_rows = mask.len();
    let mask_cols = mask[0].len();
    let mut result = data.to_vec();

    for bit_pos in 0..data.len() * 8 {
        let row = (bit_pos / width_squares as usize) % mask_rows;
        let col = (bit_pos % width_squares as usize) % mask_cols;

        if mask[row][col] == 1 {
            let byte_idx = bit_pos / 8;
            let bit_offset = 7 - (bit_pos % 8); // MSB first, matching renderer
            result[byte_idx] ^= 1 << bit_offset;
        }
    }

    result
}

/// Human-readable names of [`MASKS`], in the same order.
pub const MASK_NAMES: [&str; 7] = [
    "Checkerboard",
    "2×2 blocks",
    "Right wave",
    "Left wave",
    "Vertical lines",
    "Horizontal lines",
    "None",
];

/// Predefined bitmasks, tiled over the grid. Shared by encoding and decoding.
#[rustfmt::skip]
pub(crate) static MASKS: &[&[&[u8]]] = &[
    // Checkerboard (alternating per triangle)
    &[
        &[1, 0, 1, 0],
        &[0, 1, 0, 1],
        &[1, 0, 1, 0],
        &[0, 1, 0, 1],
    ],
    // 2×2 blocks
    &[
        &[1, 1, 0, 0],
        &[1, 1, 0, 0],
        &[0, 0, 1, 1],
        &[0, 0, 1, 1],
    ],
    // Right wave
    &[
        &[1, 0, 1, 1],
        &[0, 1, 1, 0],
        &[1, 1, 0, 1],
        &[1, 0, 1, 1],
    ],
    // Left wave
    &[
        &[1, 1, 0, 1],
        &[0, 1, 1, 0],
        &[1, 0, 1, 1],
        &[1, 1, 0, 1],
    ],
    // Vertical lines
    &[
        &[1, 0, 1, 0],
        &[1, 0, 1, 0],
        &[1, 0, 1, 0],
        &[1, 0, 1, 0],
    ],
    // Horizontal lines
    &[
        &[1, 1, 1, 1],
        &[0, 0, 0, 0],
        &[1, 1, 1, 1],
        &[0, 0, 0, 0],
    ],
    // Empty pattern, to check original data as well
    &[
        &[0, 0, 0, 0],
        &[0, 0, 0, 0],
        &[0, 0, 0, 0],
        &[0, 0, 0, 0],
    ],
];

/// Counts the triangle edges shared by two cells, returning `(differing, total)`: how many of them
/// separate cells of different colour, out of all of them. Every cell shares an edge with its
/// right-hand neighbour, and a pointing-down cell also shares its top edge with the cell above it.
pub(crate) fn colour_transitions(cells: &[Vec<bool>]) -> (usize, usize) {
    let (mut differing, mut total) = (0, 0);
    for (row, row_cells) in cells.iter().enumerate() {
        for (col, &cell) in row_cells.iter().enumerate() {
            let neighbours = [
                row_cells.get(col + 1).copied(),
                (row > 0 && points_down(row, col)).then(|| cells[row - 1][col]),
            ];
            for neighbour in neighbours.into_iter().flatten() {
                total += 1;
                if neighbour != cell {
                    differing += 1;
                }
            }
        }
    }
    (differing, total)
}

/// XORs `encoded_data` with a predefined mask (tiled to cover the grid) and returns the result
/// together with the index of the mask used. `forced` selects a mask by index; otherwise (or
/// when it's out of range) the mask giving the most colour changes between neighbouring triangles
/// is picked (see [`colour_transitions`]), which breaks the image into fine texture instead of
/// large solid patches. Ties go to the mask with the most even split of 1-bits and 0-bits.
pub(crate) fn mix_pixels(
    encoded_data: &[u8],
    width_squares: u32,
    height_squares: u32,
    forced: Option<usize>,
) -> (Vec<u8>, usize) {
    if let Some(index) = forced.filter(|&i| i < MASKS.len()) {
        return (apply_mask(encoded_data, MASKS[index], width_squares), index);
    }

    let total_bits = encoded_data.len() * 8;

    MASKS
        .iter()
        .map(|mask| apply_mask(encoded_data, mask, width_squares))
        .enumerate()
        .min_by_key(|(_, mixed)| {
            let (transitions, _) =
                colour_transitions(&layout_cells(mixed, height_squares, width_squares));
            let ones = mixed.iter().map(|b| b.count_ones() as i64).sum::<i64>();
            let zeros = total_bits as i64 - ones;
            (std::cmp::Reverse(transitions), (ones - zeros).abs())
        })
        .map(|(index, mixed)| (mixed, index))
        .unwrap_or_else(|| (encoded_data.to_vec(), MASKS.len() - 1))
}

/// Reverse of [`mix_pixels`]: yields `data` unmixed with every predefined mask.
/// The mask used when encoding isn't stored, so callers try each candidate
/// until Reed-Solomon decoding succeeds.
pub(crate) fn unmix_pixels(data: &[u8], width_squares: u32) -> impl Iterator<Item = Vec<u8>> + '_ {
    MASKS
        .iter()
        .map(move |mask| apply_mask(data, mask, width_squares))
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_log::test;

    #[test]
    fn colour_transitions_counts_shared_triangle_edges() {
        // Row 1 has one pointing-down cell, (1, 1), whose top edge it shares with (0, 1).
        let cells = vec![vec![true, false, true], vec![true, true, false]];
        assert_eq!(colour_transitions(&cells), (4, 5));
    }
}
