//! Painting cells as shapes: which shape each cell belongs to, its border, and anti-aliased edges.

use super::ImageStyle;
use super::geometry::TriangleGrid;
use super::horizon::HorizonLayout;
use image::{Rgba, RgbaImage};
use rayon::prelude::*;

/// The width the 1 px Tiles and 3 px Horizon borders were designed at; borders scale from it.
const BORDER_REFERENCE_WIDTH: f64 = 51.0;

/// Rows rendered together by one parallel task.
const BAND_ROWS: usize = 16;

/// Samples per side of the grid used to anti-alias pixels on an outline (so 16 per pixel).
const AA_SAMPLES: u32 = 4;

/// A drawn shape: one cell, or cells joined into one outline, all primary or all secondary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Shape {
    /// Unique among the shapes of the same colour.
    id: usize,
    /// Drawn in the primary colour (bit 1) rather than the secondary one (bit 0).
    pub(crate) primary: bool,
}

impl Shape {
    #[cfg(test)]
    pub(crate) fn new(id: usize, primary: bool) -> Self {
        Self { id, primary }
    }
}

/// Which drawn shape each cell belongs to; `None` only where the overlay cleared it. Pixels within
/// the border width of a different shape (or of the background) are left blank, so every shape is
/// drawn shrunk by the border and separated from its neighbours. `horizon_layouts` holds the
/// groups of primary cells and of secondary cells, in that order.
pub(crate) fn cell_shapes(
    cells: &[Vec<bool>],
    style: ImageStyle,
    horizon_layouts: Option<&[HorizonLayout; 2]>,
) -> Vec<Vec<Option<Shape>>> {
    let cols = cells.first().map_or(0, Vec::len);
    // Each cell on its own gets a unique id; joined Horizon groups share one past those. Stain has
    // no borders, so its ids only tell the cells apart for colour variance.
    let singles = cells.len() * cols;
    cells
        .iter()
        .enumerate()
        .map(|(row, cells_in_row)| {
            cells_in_row
                .iter()
                .enumerate()
                .map(|(col, &primary)| {
                    let id = match style {
                        ImageStyle::Stain | ImageStyle::Tiles => row * cols + col,
                        ImageStyle::Horizon => horizon_layouts
                            .and_then(|[primary_layout, secondary_layout]| {
                                (if primary {
                                    primary_layout
                                } else {
                                    secondary_layout
                                })
                                .group_id[row][col]
                            })
                            .map_or(row * cols + col, |group| singles + group),
                    };
                    Some(Shape { id, primary })
                })
                .collect()
        })
        .collect()
}

/// Border width on each side of an edge, measured perpendicular to it. It scales with the
/// triangle (never below 1 px), so a larger triangle width enlarges the whole image rather than
/// thinning the borders.
pub(crate) fn border_width(style: ImageStyle, triangle_width: u32) -> f64 {
    let border_at_reference: f64 = match style {
        ImageStyle::Horizon => 3.0,
        ImageStyle::Tiles => 1.0,
        ImageStyle::Stain => return 0.0,
    };
    (border_at_reference * triangle_width as f64 / BORDER_REFERENCE_WIDTH)
        .round()
        .max(1.0)
}

/// Paints every cell in its colour from `colours` over `img` (filled with `bg`) wherever a pixel
/// lies inside its shape and off its borders, blending pixels the outlines cross.
///
/// Anti-aliasing: a pixel whose corners and centre all agree is taken as fully painted or not; one
/// the outline crosses is sampled on a grid and blended by the share painted in each colour.
/// Corners are shared between neighbouring pixels, so they are computed a row at a time, and bands
/// of rows are rendered in parallel.
pub(crate) fn paint_shapes(
    img: &mut RgbaImage,
    grid: &TriangleGrid,
    shapes: &[Vec<Option<Shape>>],
    border: f64,
    colours: &[Vec<Rgba<u8>>],
    bg: Rgba<u8>,
) {
    let width = img.width();
    let shape_of = |row: i64, col: i64| -> Option<Shape> {
        grid.contains(row, col)
            .then(|| shapes[row as usize][col as usize])
            .flatten()
    };
    // The colour a point is painted in; `None` where the background shows.
    let painted = |x: f64, y: f64| -> Option<Rgba<u8>> {
        let (row, col) = grid.cell_at(x, y);
        shape_of(row, col)?;
        (!(border > 0.0 && grid.on_border((x, y), (row, col), border, shape_of)))
            .then(|| colours[row as usize][col as usize])
    };
    let corner_row = |y: f64| -> Vec<Option<Rgba<u8>>> {
        (0..=width).map(|px| painted(px as f64 - 0.5, y)).collect()
    };
    // The pixel's colour, if anything is painted on it. Colours are mixed linearly, alpha
    // included, and aren't weighted by alpha, so an edge over a transparent background keeps a
    // light fringe the decoder still reads as background, at the cost of a faint halo over dark
    // backdrops.
    let pixel_colour = |x: f64, y: f64, corners: [Option<Rgba<u8>>; 4]| -> Option<Rgba<u8>> {
        let centre = painted(x, y);
        if corners.iter().all(|&c| c == centre) {
            return centre;
        }
        let offset = |i: u32| (i as f64 + 0.5) / AA_SAMPLES as f64 - 0.5;
        // Summed differences from the background, so a colour equal to it changes nothing.
        let mut sums = [0i32; 4];
        let mut hits = 0;
        for i in 0..AA_SAMPLES {
            for j in 0..AA_SAMPLES {
                if let Some(colour) = painted(x + offset(i), y + offset(j)) {
                    for (sum, (&c, &b)) in sums.iter_mut().zip(colour.0.iter().zip(&bg.0)) {
                        *sum += c as i32 - b as i32;
                    }
                    hits += 1;
                }
            }
        }
        let samples = (AA_SAMPLES * AA_SAMPLES) as f64;
        (hits > 0).then(|| {
            Rgba(std::array::from_fn(|i| {
                (bg[i] as f64 + sums[i] as f64 / samples).round() as u8
            }))
        })
    };

    let row_bytes = width as usize * 4;
    img.par_chunks_mut(row_bytes * BAND_ROWS)
        .enumerate()
        .for_each(|(band, rows)| {
            let first_row = band * BAND_ROWS;
            let mut above = corner_row(first_row as f64 - 0.5);
            for (i, row) in rows.chunks_mut(row_bytes).enumerate() {
                let y = (first_row + i) as f64;
                let below = corner_row(y + 0.5);
                for (px, pixel) in row.chunks_mut(4).enumerate() {
                    let corners = [above[px], above[px + 1], below[px], below[px + 1]];
                    if let Some(colour) = pixel_colour(px as f64, y, corners) {
                        pixel.copy_from_slice(&colour.0);
                    }
                }
                above = below;
            }
        });
}
