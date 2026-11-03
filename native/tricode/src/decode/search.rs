//! Grid-size estimation strategies and the search around their estimates.

use super::DecodeDebug;
use super::count::{count_columns_by_pitch, count_rows_by_clustering};
use super::detect::{Anchors, Triangle};
use super::lattice::{Lattice, LatticeFit};
use super::read::{
    Decoded, FLIPS, Flip, PackedBits, pack_data_bits, sample_cells, try_unmix_decode,
};
use image::GrayImage;
use rayon::prelude::*;
use std::collections::HashSet;
use std::sync::Mutex;
use tracing::{debug, trace};

/// Inputs shared by all grid-estimation strategies.
pub(crate) struct GridContext<'a> {
    pub(crate) triangles: &'a [Triangle],
    pub(crate) anchors: &'a Anchors,
    /// Grid size estimated from the anchors alone.
    pub(crate) row_naive: usize,
    pub(crate) col_naive: usize,
    pub(crate) fit: Option<LatticeFit>,
}

impl GridContext<'_> {
    /// The lattice to read a `rows × cols` grid from: the fitted one when it describes that grid,
    /// otherwise the one spanned by the anchors.
    fn lattice_for(&self, rows: usize, cols: usize) -> Lattice {
        match &self.fit {
            Some(fit) if (fit.rows, fit.cols) == (rows, cols) => fit.lattice,
            _ => Lattice::from_anchors(self.anchors, rows, cols),
        }
    }
}

/// A named way of estimating the grid size. Returns `(rows, cols)` estimates, most likely first.
struct GridStrategy {
    name: &'static str,
    estimates: fn(&GridContext) -> Vec<(usize, usize)>,
}

/// Strategies in the order they are tried.
static GRID_STRATEGIES: &[GridStrategy] = &[
    GridStrategy {
        name: "lattice-fit",
        estimates: |ctx| ctx.fit.iter().map(|fit| (fit.rows, fit.cols)).collect(),
    },
    GridStrategy {
        name: "naive",
        estimates: |ctx| vec![(ctx.row_naive, ctx.col_naive)],
    },
    GridStrategy {
        name: "cluster+pitch",
        estimates: |ctx| vec![cluster_pitch_estimate(ctx)],
    },
];

/// How far (in rows and columns) around each estimate the decoder searches.
const GRID_SEARCH_RADIUS: isize = 5;

/// A grid that decoded, and how it was read.
struct GridMatch {
    rows: usize,
    cols: usize,
    flip: Flip,
    packed: PackedBits,
    decoded: Decoded,
}

/// Reads the grid for every strategy's estimates until one decodes. On failure, the result has
/// `error` set and describes the fitted grid (or the cluster+pitch estimate without one).
pub(crate) fn try_grid_strategies(gray_img: &GrayImage, ctx: &GridContext) -> DecodeDebug {
    let base = DecodeDebug {
        text: None,
        content_type: String::new(),
        error: None,
        image_width: gray_img.width(),
        image_height: gray_img.height(),
        total_triangles: ctx.triangles.len(),
        row_count: ctx.row_naive,
        column_count: ctx.col_naive,
        bit_count: 0,
        annotated_png: Vec::new(),
        decode_method: String::new(),
        ecc_level: None,
        corrected_bytes: 0,
        erased_bytes: 0,
    };

    // Each strategy yields grid estimates. Every estimate is tried as-is, then
    // its ±GRID_SEARCH_RADIUS neighbourhood (closest first). Already-tried grids are skipped.
    let mut tried: HashSet<(usize, usize)> = HashSet::new();
    let mut failures: Vec<String> = Vec::new();
    for strategy in GRID_STRATEGIES {
        let mut attempted = 0usize;
        let mut last_err = String::from("no new candidates");
        for estimate in (strategy.estimates)(ctx) {
            let grids: Vec<(usize, usize)> = with_neighbourhood(ctx, estimate)
                .into_iter()
                .filter(|&grid| tried.insert(grid))
                .collect();
            attempted += grids.len();
            match search_grids(gray_img, ctx, &grids) {
                Ok(found) => return success(base, strategy.name, estimate, found),
                Err(Some(e)) => last_err = e,
                Err(None) => {}
            }
        }
        failures.push(format!(
            "{} ({} grids tried): {}",
            strategy.name, attempted, last_err
        ));
    }
    failure(base, gray_img, ctx, failures)
}

/// Reads `grids` in parallel and returns the first one (in their order) that decodes. Each grid
/// costs every flip × mask × level, which adds up when nothing decodes. On failure, returns the
/// error of reading the first grid unflipped, if there was a first grid.
fn search_grids(
    gray_img: &GrayImage,
    ctx: &GridContext,
    grids: &[(usize, usize)],
) -> Result<GridMatch, Option<String>> {
    let first_err: Mutex<Option<String>> = Mutex::new(None);
    grids
        .par_iter()
        .enumerate()
        .find_map_first(|(index, &(rows, cols))| {
            read_grid(gray_img, ctx, rows, cols)
                .map_err(|e| {
                    if index == 0
                        && let Ok(mut slot) = first_err.lock()
                    {
                        *slot = Some(e);
                    }
                })
                .ok()
        })
        .ok_or_else(|| first_err.into_inner().ok().flatten())
}

/// Reads a `rows × cols` grid through every flip until one decodes. On failure, returns the error
/// of the unflipped reading.
fn read_grid(
    gray_img: &GrayImage,
    ctx: &GridContext,
    rows: usize,
    cols: usize,
) -> Result<GridMatch, String> {
    let cells = sample_cells(gray_img, rows, cols, &ctx.lattice_for(rows, cols));
    let mut unflipped_err = String::new();
    for flip in FLIPS {
        let packed = pack_data_bits(&cells, flip);
        match try_unmix_decode(&packed, rows, cols) {
            Ok(decoded) => {
                return Ok(GridMatch {
                    rows,
                    cols,
                    flip,
                    packed,
                    decoded,
                });
            }
            Err(e) if flip.name().is_none() => unflipped_err = e,
            Err(_) => {}
        }
    }
    Err(unflipped_err)
}

/// The result of a grid that decoded, found by `strategy` around `estimate`.
fn success(
    base: DecodeDebug,
    strategy: &str,
    estimate: (usize, usize),
    found: GridMatch,
) -> DecodeDebug {
    let GridMatch {
        rows,
        cols,
        flip,
        packed,
        decoded,
    } = found;
    let mut method = if (rows, cols) == estimate {
        strategy.to_string()
    } else {
        format!("{} ±{}", strategy, GRID_SEARCH_RADIUS)
    };
    if let Some(name) = flip.name() {
        method += &format!(" ({})", name);
    }
    debug!(%method, rows, cols, "grid strategy succeeded");
    DecodeDebug {
        text: Some(decoded.text),
        content_type: decoded.kind,
        row_count: rows,
        column_count: cols,
        bit_count: packed.bit_count,
        decode_method: method,
        ecc_level: Some(decoded.level),
        corrected_bytes: decoded.corrected_bytes,
        erased_bytes: packed.erased.iter().filter(|&&e| e).count(),
        ..base
    }
}

/// The result when every strategy failed, describing the fitted grid, or the cluster+pitch
/// estimate without one.
fn failure(
    base: DecodeDebug,
    gray_img: &GrayImage,
    ctx: &GridContext,
    failures: Vec<String>,
) -> DecodeDebug {
    let (rows, cols) = match &ctx.fit {
        Some(fit) => (fit.rows, fit.cols),
        None => cluster_pitch_estimate(ctx),
    };
    // The estimate of a cropped or badly damaged image can fall under 2×2, which no grid is (the
    // grid search skips those too).
    let (rows, cols) = (rows.max(2), cols.max(2));
    let cells = sample_cells(gray_img, rows, cols, &ctx.lattice_for(rows, cols));
    DecodeDebug {
        error: Some(failures.join("\n")),
        row_count: rows,
        column_count: cols,
        bit_count: pack_data_bits(&cells, FLIPS[0]).bit_count,
        decode_method: "all failed".to_string(),
        ..base
    }
}

/// Cluster + pitch estimates, clamped to naive ± 1.
fn cluster_pitch_estimate(ctx: &GridContext) -> (usize, usize) {
    let row_raw = count_rows_by_clustering(ctx.triangles);
    let (col_raw, pitch_px) = count_columns_by_pitch(ctx.triangles, ctx.anchors);
    let rows = row_raw.clamp(ctx.row_naive.saturating_sub(1), ctx.row_naive + 1);
    let cols = col_raw.clamp(ctx.col_naive.saturating_sub(1), ctx.col_naive + 1);
    trace!(
        rows,
        row_raw, cols, col_raw, pitch_px, "cluster+pitch estimate"
    );
    (rows, cols)
}

/// `estimate` and every grid within ±GRID_SEARCH_RADIUS of it, ordered by how well the detected
/// triangles fit each grid (see [`Lattice::misfit`]), closest to `estimate` first on ties.
/// Grids smaller than 2×2 are left out.
fn with_neighbourhood(ctx: &GridContext, estimate: (usize, usize)) -> Vec<(usize, usize)> {
    let (rows, cols) = estimate;
    let mut candidates: Vec<(f32, isize, (usize, usize))> = (-GRID_SEARCH_RADIUS
        ..=GRID_SEARCH_RADIUS)
        .flat_map(|dr| (-GRID_SEARCH_RADIUS..=GRID_SEARCH_RADIUS).map(move |dc| (dr, dc)))
        .filter_map(|(dr, dc)| {
            let r = usize::try_from(rows as isize + dr).ok()?;
            let c = usize::try_from(cols as isize + dc).ok()?;
            if r < 2 || c < 2 {
                return None;
            }
            let misfit = ctx.lattice_for(r, c).misfit(ctx.triangles, r, c);
            Some((misfit, dr.abs().max(dc.abs()), (r, c)))
        })
        .collect();
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    candidates.into_iter().map(|(_, _, grid)| grid).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::EROSION_RADII;
    use crate::decode::detect::{discard_merged_triangles, find_anchors, find_triangles};
    use crate::tests::{all_cases, grid_of};
    use test_log::test;

    #[test]
    fn misfit_ranks_true_grid_first() {
        for (label, _, generated) in all_cases() {
            let gray = image::load_from_memory(&generated.png_bytes)
                .expect("load failed")
                .to_luma8();
            let triangles = find_triangles(&gray, EROSION_RADII[0]).expect("no triangles");
            let anchors = find_anchors(&triangles).expect("no anchors");
            let triangles = discard_merged_triangles(triangles, &anchors);
            // Without a fitted lattice, so only the anchor-spanned lattices are ranked.
            let ctx = GridContext {
                triangles: &triangles,
                anchors: &anchors,
                row_naive: 0,
                col_naive: 0,
                fit: None,
            };
            let truth = grid_of(&generated);
            let off_estimate = (truth.0 + 3, truth.1 - 4);
            let ranked = with_neighbourhood(&ctx, off_estimate);
            assert_eq!(ranked.first(), Some(&truth), "{label}");
        }
    }
}
