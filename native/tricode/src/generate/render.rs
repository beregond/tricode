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

/// Which drawn shape each cell belongs to; `None` for background cells. Pixels within the border
/// width of a different shape (or of the background) are left blank, so every shape is drawn
/// shrunk by the border and separated from its neighbours.
pub(crate) fn cell_shapes(
    cells: &[Vec<bool>],
    style: ImageStyle,
    horizon_layout: Option<&HorizonLayout>,
) -> Vec<Vec<Option<usize>>> {
    let cols = cells.first().map_or(0, Vec::len);
    // Each cell on its own gets a unique id; joined Horizon groups share one past those.
    let singles = cells.len() * cols;
    cells
        .iter()
        .enumerate()
        .map(|(row, cells_in_row)| {
            cells_in_row
                .iter()
                .enumerate()
                .map(|(col, &is_fg)| {
                    is_fg.then(|| match style {
                        ImageStyle::Stain => 0,
                        ImageStyle::Tiles => row * cols + col,
                        ImageStyle::Horizon => horizon_layout
                            .and_then(|hl| hl.group_id[row][col])
                            .map_or(row * cols + col, |group| singles + group),
                    })
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

/// Paints `fg` over `img` (filled with `bg`) wherever a pixel lies inside a shape and off its
/// borders, blending pixels the outline crosses.
///
/// Anti-aliasing: a pixel whose corners and centre all agree is taken as fully painted or not; one
/// the outline crosses is sampled on a grid and blended by the share painted. Corners are shared
/// between neighbouring pixels, so they are computed a row at a time, and bands of rows are
/// rendered in parallel.
pub(crate) fn paint_shapes(
    img: &mut RgbaImage,
    grid: &TriangleGrid,
    shapes: &[Vec<Option<usize>>],
    border: f64,
    fg: Rgba<u8>,
    bg: Rgba<u8>,
) {
    let width = img.width();
    let shape_of = |row: i64, col: i64| -> Option<usize> {
        grid.contains(row, col)
            .then(|| shapes[row as usize][col as usize])
            .flatten()
    };
    let painted = |x: f64, y: f64| -> bool {
        let (row, col) = grid.cell_at(x, y);
        shape_of(row, col).is_some()
            && !(border > 0.0 && grid.on_border((x, y), (row, col), border, shape_of))
    };
    let corner_row =
        |y: f64| -> Vec<bool> { (0..=width).map(|px| painted(px as f64 - 0.5, y)).collect() };
    let coverage = |x: f64, y: f64, corners: [bool; 4]| -> f64 {
        let centre = painted(x, y);
        if corners.iter().all(|&c| c == centre) {
            return if centre { 1.0 } else { 0.0 };
        }
        let offset = |i: u32| (i as f64 + 0.5) / AA_SAMPLES as f64 - 0.5;
        let hits = (0..AA_SAMPLES)
            .flat_map(|i| (0..AA_SAMPLES).map(move |j| (i, j)))
            .filter(|&(i, j)| painted(x + offset(i), y + offset(j)))
            .count();
        hits as f64 / (AA_SAMPLES * AA_SAMPLES) as f64
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
                    let painted_share = coverage(px as f64, y, corners);
                    if painted_share > 0.0 {
                        pixel.copy_from_slice(&mix(bg, fg, painted_share).0);
                    }
                }
                above = below;
            }
        });
}

/// Linear mix of two colours, alpha included: `from` at 0, `to` at 1. Colours aren't weighted by
/// alpha, so an edge over a transparent background keeps a light fringe the decoder still reads as
/// background, at the cost of a faint halo over dark backdrops.
fn mix(from: Rgba<u8>, to: Rgba<u8>, amount: f64) -> Rgba<u8> {
    Rgba(std::array::from_fn(|i| {
        (from[i] as f64 + (to[i] as f64 - from[i] as f64) * amount).round() as u8
    }))
}
