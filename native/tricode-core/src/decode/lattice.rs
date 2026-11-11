//! Fitting the lattice of triangle centroids, which gives the grid's pitch, origin and size.

use super::detect::{Anchors, Triangle};
use crate::grid::{DEFAULT_TRIANGLE_WIDTH, points_down, triangle_height};
use tracing::{debug, trace};

/// Rows advance by a full bit box and columns by half of one (neighbouring triangles interlock),
/// so this is the row pitch divided by the column pitch of a rendered grid. The decoder doesn't know
/// the triangle width, so this uses the default one; rounding moves the ratio by under 5% between
/// the smallest and largest widths, well inside `ROW_PITCH_TOLERANCE`.
fn pitch_aspect() -> f32 {
    triangle_height(DEFAULT_TRIANGLE_WIDTH) as f32 / (DEFAULT_TRIANGLE_WIDTH / 2) as f32
}

/// Distance, in row pitches, from a triangle's centroid to the centre of its cell. A pointing-down
/// triangle's centroid lies a third of its height below its base, i.e. this far above the centre;
/// a pointing-up one lies this far below it.
const CENTROID_OFFSET: f32 = 1.0 / 6.0;

/// Search range for the column pitch, as multiples of half the median triangle width. Borders and
/// erosion make triangles narrower than their cell, so the true pitch is at or above 1×; the range
/// stays clear of 0.5× and 2×, where the lattice would also line up.
const COL_PITCH_SEARCH: (f32, f32) = (0.8, 1.7);

/// How far the row pitch may stray from `col_pitch · pitch_aspect()`.
const ROW_PITCH_TOLERANCE: f32 = 0.15;

/// Least-squares refinement passes after the initial period estimate.
const FIT_ITERATIONS: usize = 3;

/// Triangles further than this (in pitches) from their nearest lattice point don't take part in
/// the least-squares fit.
const FIT_INLIER_DISTANCE: f32 = 0.3;

/// A fitted lattice is rejected when fewer triangles than this have the orientation their cell
/// requires.
const MIN_PARITY_AGREEMENT: f32 = 0.9;

/// Index into per-orientation arrays: 0 for pointing down, 1 for pointing up.
fn orientation_group(pointing_down: bool) -> usize {
    usize::from(!pointing_down)
}

/// Triangle centroid positions of a grid, in image pixels.
///
/// Cell `(row, col)` has its centroid at `(x0[g] + col·col_pitch, y0[g] + row·row_pitch)`, where
/// `g` is the cell's [`orientation_group`]. Cell `(0, 0)` is the top-left anchor, which always
/// points down. Centroids are used rather than bounding boxes because a border shrinks a triangle
/// evenly from every side, which leaves its centroid where it was.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Lattice {
    pub(crate) x0: [f32; 2],
    pub(crate) y0: [f32; 2],
    pub(crate) col_pitch: f32,
    pub(crate) row_pitch: f32,
}

impl Lattice {
    /// The lattice spanned by the anchors for a `rows × cols` grid (both at least 2).
    pub(crate) fn from_anchors(anchors: &Anchors, rows: usize, cols: usize) -> Self {
        let top_left = &anchors.top_left;
        let col_pitch = (anchors.top_right.center_x - top_left.center_x) / (cols - 1) as f32;
        // A pointing-up bottom-left anchor sits an extra 2·CENTROID_OFFSET pitches lower.
        let span = anchors.bottom_left.center_y - top_left.center_y;
        let last_row = (rows - 1) as f32;
        let row_pitch = if points_down(rows - 1, 0) {
            span / last_row
        } else {
            span / (last_row + 2.0 * CENTROID_OFFSET)
        };
        Self {
            x0: [top_left.center_x; 2],
            y0: [
                top_left.center_y,
                top_left.center_y + 2.0 * CENTROID_OFFSET * row_pitch,
            ],
            col_pitch,
            row_pitch,
        }
    }

    pub(crate) fn centroid(&self, row: usize, col: usize) -> (f32, f32) {
        let g = orientation_group(points_down(row, col));
        (
            self.x0[g] + col as f32 * self.col_pitch,
            self.y0[g] + row as f32 * self.row_pitch,
        )
    }

    /// How badly the triangles fit a `rows × cols` grid on this lattice: the mean squared distance
    /// (in pitches) from each centroid to its nearest cell, plus the fraction of triangles outside
    /// the grid or with the wrong orientation for their cell. 0 is a perfect fit.
    pub(crate) fn misfit(&self, triangles: &[Triangle], rows: usize, cols: usize) -> f32 {
        if triangles.is_empty() {
            return f32::MAX;
        }
        let mut total = 0.0f32;
        for t in triangles {
            let g = orientation_group(!t.pointing_up);
            let col_f = (t.center_x - self.x0[g]) / self.col_pitch;
            let row_f = (t.center_y - self.y0[g]) / self.row_pitch;
            let (col, row) = (col_f.round(), row_f.round());
            total += (col_f - col).powi(2) + (row_f - row).powi(2);
            let inside = row >= 0.0 && col >= 0.0 && (row as usize) < rows && (col as usize) < cols;
            if !inside || points_down(row as usize, col as usize) == t.pointing_up {
                total += 1.0;
            }
        }
        total / triangles.len() as f32
    }
}

/// A lattice fitted to every detected triangle, and the grid size it implies.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LatticeFit {
    pub(crate) lattice: Lattice,
    pub(crate) rows: usize,
    pub(crate) cols: usize,
}

/// Fits a [`Lattice`] to all triangles, so that every one of them (not just the anchors) pins down
/// the pitch, then reads the grid size off the anchors' lattice indices.
///
/// Returns `None` when the fit fails its sanity checks: anchors that don't form a rectangle, too
/// many triangles with the wrong orientation for their cell, or a row/column pitch ratio that
/// doesn't match the renderer.
pub(crate) fn fit_lattice(triangles: &[Triangle], anchors: &Anchors) -> Option<LatticeFit> {
    if triangles.len() < 4 {
        return None;
    }
    let (x, y) = fit_axes(triangles)?;

    let index = |t: &Triangle| {
        let g = orientation_group(!t.pointing_up);
        (
            y.index(t.center_y, g).round() as i64,
            x.index(t.center_x, g).round() as i64,
        )
    };
    let (top, left) = index(&anchors.top_left);
    let (top_right_row, right) = index(&anchors.top_right);
    let (bottom, bottom_left_col) = index(&anchors.bottom_left);
    let bottom_right = index(&anchors.bottom_right);

    let parity_matches = triangles
        .iter()
        .filter(|t| {
            let (r, c) = index(t);
            (((r - top) + (c - left)).rem_euclid(2) == 0) != t.pointing_up
        })
        .count();
    let parity = parity_matches as f32 / triangles.len() as f32;
    let aspect = y.pitch / x.pitch / pitch_aspect();

    trace!(
        col_pitch = x.pitch,
        row_pitch = y.pitch,
        aspect,
        parity,
        top_left = ?(top, left),
        top_right = ?(top_right_row, right),
        bottom_left = ?(bottom, bottom_left_col),
        ?bottom_right,
        "lattice fit"
    );

    if top_right_row != top || bottom_left_col != left || bottom_right != (bottom, right) {
        debug!("lattice fit rejected: anchors don't form a rectangle");
        return None;
    }
    if parity < MIN_PARITY_AGREEMENT {
        debug!(parity, "lattice fit rejected: orientation parity too low");
        return None;
    }
    if (aspect - 1.0).abs() > ROW_PITCH_TOLERANCE {
        debug!(aspect, "lattice fit rejected: pitch aspect off");
        return None;
    }

    let rows = usize::try_from(bottom - top + 1).ok()?;
    let cols = usize::try_from(right - left + 1).ok()?;
    if rows < 2 || cols < 2 {
        return None;
    }

    // Re-base so that cell (0, 0) is the top-left anchor.
    let lattice = Lattice {
        x0: x.intercepts.map(|a| a + left as f32 * x.pitch),
        y0: y.intercepts.map(|a| a + top as f32 * y.pitch),
        col_pitch: x.pitch,
        row_pitch: y.pitch,
    };
    Some(LatticeFit {
        lattice,
        rows,
        cols,
    })
}

/// Fits the column axis, then the row axis, to the triangles' centroids.
///
/// Columns first: there are usually more of them, and the row pitch then only needs a narrow
/// search around `col_pitch · pitch_aspect()`.
fn fit_axes(triangles: &[Triangle]) -> Option<(AxisFit, AxisFit)> {
    let mut widths: Vec<f32> = triangles
        .iter()
        .map(|t| (t.max_x - t.min_x + 1) as f32)
        .collect();
    widths.sort_by(f32::total_cmp);
    let half_width = widths[widths.len() / 2] / 2.0;

    let samples = |pos: fn(&Triangle) -> f32| -> Vec<AxisSample> {
        triangles
            .iter()
            .map(|t| AxisSample {
                pos: pos(t),
                group: orientation_group(!t.pointing_up),
            })
            .collect()
    };

    let x = fit_axis(
        &samples(|t| t.center_x),
        [0.0, 0.0],
        half_width * COL_PITCH_SEARCH.0,
        half_width * COL_PITCH_SEARCH.1,
    )?;
    let expected_row_pitch = x.pitch * pitch_aspect();
    let y = fit_axis(
        &samples(|t| t.center_y),
        [CENTROID_OFFSET, -CENTROID_OFFSET],
        expected_row_pitch * (1.0 - ROW_PITCH_TOLERANCE),
        expected_row_pitch * (1.0 + ROW_PITCH_TOLERANCE),
    )?;
    Some((x, y))
}

/// A triangle's centroid position along one axis, and its [`orientation_group`].
#[derive(Debug, Clone, Copy)]
struct AxisSample {
    pos: f32,
    group: usize,
}

/// One axis of a fitted lattice: a sample in group `g` at lattice index `k` sits at
/// `intercepts[g] + k·pitch`. Indices are relative to an arbitrary lattice point.
#[derive(Debug, Clone, Copy)]
struct AxisFit {
    intercepts: [f32; 2],
    pitch: f32,
}

impl AxisFit {
    /// Fractional lattice index of a position.
    fn index(&self, pos: f32, group: usize) -> f32 {
        (pos - self.intercepts[group]) / self.pitch
    }
}

/// Fits one lattice axis to `samples`. `offsets[g]` is how far (in pitches) group `g` sits from
/// the cell centre, which lets both orientations share one set of lattice points.
///
/// The pitch is first found by [`dominant_period`], then refined by least squares with one shared
/// pitch and a separate intercept per group, so a systematic centroid shift in one orientation
/// doesn't bias the pitch.
fn fit_axis(samples: &[AxisSample], offsets: [f32; 2], lo: f32, hi: f32) -> Option<AxisFit> {
    let shifted: Vec<(f32, f32)> = samples.iter().map(|s| (s.pos, offsets[s.group])).collect();
    let (pitch, origin) = dominant_period(&shifted, lo, hi)?;
    let mut fit = AxisFit {
        intercepts: offsets.map(|o| origin - o * pitch),
        pitch,
    };

    for _ in 0..FIT_ITERATIONS {
        fit = refine_axis(samples, offsets, fit)?;
    }
    Some(fit)
}

/// One least-squares pass over the samples within [`FIT_INLIER_DISTANCE`] of `fit`'s lattice
/// points: one shared pitch, an intercept per group.
fn refine_axis(samples: &[AxisSample], offsets: [f32; 2], fit: AxisFit) -> Option<AxisFit> {
    // Per group: count, Σk, Σv, Σk², Σkv over inliers.
    let mut sums = [[0.0f64; 5]; 2];
    for s in samples {
        let k_f = fit.index(s.pos, s.group);
        let k = k_f.round();
        if (k_f - k).abs() > FIT_INLIER_DISTANCE {
            continue;
        }
        let (k, v) = (k as f64, s.pos as f64);
        let sum = &mut sums[s.group];
        sum[0] += 1.0;
        sum[1] += k;
        sum[2] += v;
        sum[3] += k * k;
        sum[4] += k * v;
    }

    let (mut sxx, mut sxy) = (0.0f64, 0.0f64);
    for [n, sk, sv, skk, skv] in sums {
        if n > 0.0 {
            sxx += skk - sk * sk / n;
            sxy += skv - sk * sv / n;
        }
    }
    if sxx <= 0.0 {
        return None;
    }
    let pitch = sxy / sxx;
    let intercept = |[n, sk, sv, ..]: [f64; 5]| (n > 0.0).then(|| ((sv - pitch * sk) / n) as f32);
    let pitch = pitch as f32;
    let intercepts = match (intercept(sums[0]), intercept(sums[1])) {
        (Some(a), Some(b)) => [a, b],
        (Some(a), None) => [a, a + (offsets[0] - offsets[1]) * pitch],
        (None, Some(b)) => [b + (offsets[1] - offsets[0]) * pitch, b],
        (None, None) => return None,
    };
    Some(AxisFit { intercepts, pitch })
}

/// Finds the period in `lo..=hi` at which the `(position, offset)` samples line up best: the one
/// that maximises the length of their summed phase vector, with phase `position / period + offset`.
/// Returns `(period, origin)`, where `origin` is the position of one lattice point.
///
/// Unlike rounding a single span by a single size, every sample contributes, so the error of
/// individual centroids averages out.
fn dominant_period(samples: &[(f32, f32)], lo: f32, hi: f32) -> Option<(f32, f32)> {
    if samples.is_empty() || !(lo > 0.0 && hi > lo) {
        return None;
    }
    let min = samples.iter().map(|s| s.0).fold(f32::MAX, f32::min);
    let max = samples.iter().map(|s| s.0).fold(f32::MIN, f32::max);
    // Between two tried periods the phase at the far end of the span must drift by well under a
    // cycle, or the peak could be stepped over.
    let span = (max - min).max(hi) as f64;
    let step = (lo as f64).powi(2) / (16.0 * span);
    let steps = (((hi - lo) as f64 / step).ceil() as usize).min(100_000);

    let mut best: Option<(f64, f32, f32)> = None;
    for i in 0..=steps {
        let period = lo as f64 + (hi - lo) as f64 * i as f64 / steps.max(1) as f64;
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for &(pos, offset) in samples {
            let phase = std::f64::consts::TAU * ((pos - min) as f64 / period + offset as f64);
            re += phase.cos();
            im += phase.sin();
        }
        let strength = re.hypot(im);
        if best.is_none_or(|(b, _, _)| strength > b) {
            let origin = min as f64 + im.atan2(re) / std::f64::consts::TAU * period;
            best = Some((strength, period as f32, origin as f32));
        }
    }
    best.map(|(_, period, origin)| (period, origin))
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_log::test;

    #[test]
    fn dominant_period_recovers_sub_pixel_pitch() {
        // Every third lattice point missing, positions jittered by up to ±1px.
        let samples: Vec<(f32, f32)> = (0..200)
            .filter(|k| k % 3 != 0)
            .map(|k| {
                (
                    7.3 + k as f32 * 25.4 + ((k * 7919) % 21) as f32 / 10.0 - 1.0,
                    0.0,
                )
            })
            .collect();
        let (period, origin) = dominant_period(&samples, 20.0, 40.0).expect("no period");
        assert!((period - 25.4).abs() < 0.01, "period {period}");
        let k = (origin - 7.3) / 25.4;
        assert!((k - k.round()).abs() < 0.05, "origin {origin}");
    }

    #[test]
    fn anchor_lattice_accounts_for_bottom_left_orientation() {
        let anchor = |cx: f32, cy: f32, pointing_up: bool| Triangle {
            center_x: cx,
            center_y: cy,
            pointing_up,
            min_x: 0,
            min_y: 0,
            max_x: 0,
            max_y: 0,
        };
        // 3 rows: the bottom-left cell (2, 0) points down, at exactly 2 pitches.
        let anchors = Anchors {
            top_left: anchor(10.0, 15.0, false),
            top_right: anchor(110.0, 15.0, false),
            bottom_left: anchor(10.0, 105.0, false),
            bottom_right: anchor(110.0, 105.0, false),
        };
        let lattice = Lattice::from_anchors(&anchors, 3, 5);
        assert_eq!((lattice.col_pitch, lattice.row_pitch), (25.0, 45.0));
        // 4 rows: the bottom-left cell (3, 0) points up, a third of a pitch lower than 3 pitches.
        let anchors = Anchors {
            bottom_left: anchor(10.0, 15.0 + 3.0 * 45.0 + 15.0, true),
            ..anchors
        };
        let lattice = Lattice::from_anchors(&anchors, 4, 5);
        assert!((lattice.row_pitch - 45.0).abs() < 1e-4);
        assert!((lattice.centroid(1, 0).1 - (15.0 + 45.0 + 15.0)).abs() < 1e-4);
    }
}
