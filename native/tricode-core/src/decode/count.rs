//! Grid-size estimates that don't need a fitted lattice: from the anchors alone ("naive") and from
//! row clusters and column pitch ("cluster+pitch").

use super::detect::{Anchors, Triangle};
use tracing::trace;

/// Width of the bins Δx values are counted in when looking for the column pitch, in pixels: coarse
/// enough to group near-identical values.
const PITCH_BIN_WIDTH: f32 = 3.0;

/// Middle value of `values` (the upper one of an even count); `values` must not be empty.
fn median(mut values: Vec<f32>) -> f32 {
    values.sort_by(f32::total_cmp);
    values[values.len() / 2]
}

/// Groups triangles into rows by their centre's y, as `(mean y, centre xs)` from the top. A
/// triangle joins the current row when it lies within half the median triangle height of the
/// row's running mean.
fn cluster_rows(triangles: &[Triangle]) -> Vec<(f32, Vec<f32>)> {
    if triangles.is_empty() {
        return Vec::new();
    }
    let cell_height = median(triangles.iter().map(Triangle::bbox_height).collect()).max(1.0);
    let tolerance = cell_height * 0.5;

    let mut sorted: Vec<&Triangle> = triangles.iter().collect();
    sorted.sort_by(|a, b| a.center_y.total_cmp(&b.center_y));

    let mut rows: Vec<(f32, Vec<f32>)> = Vec::new();
    for t in sorted {
        if let Some((mean_y, xs)) = rows.last_mut()
            && (t.center_y - *mean_y).abs() <= tolerance
        {
            xs.push(t.center_x);
            // Not a true mean, but only the count of rows matters.
            *mean_y = (*mean_y + t.center_y) / 2.0;
            continue;
        }
        rows.push((t.center_y, vec![t.center_x]));
    }
    rows
}

/// Approach 2: the number of row clusters (see [`cluster_rows`]); 0 without triangles.
pub(crate) fn count_rows_by_clustering(triangles: &[Triangle]) -> usize {
    cluster_rows(triangles).len()
}

/// Approach 3: estimate the horizontal cell pitch from the mode of the Δcenter_x values between
/// neighbours in each row, then derive the column count from the anchors' span divided by that
/// pitch.
///
/// Returns (column_count, pitch_px).
pub(crate) fn count_columns_by_pitch(triangles: &[Triangle], anchors: &Anchors) -> (usize, f32) {
    if triangles.len() < 2 {
        return (0, 1.0);
    }

    let mut deltas: Vec<f32> = Vec::new();
    for (_, mut xs) in cluster_rows(triangles) {
        xs.sort_by(f32::total_cmp);
        // Under a pixel apart is a duplicate or noise.
        deltas.extend(xs.windows(2).map(|p| p[1] - p[0]).filter(|&d| d > 1.0));
    }
    let Some(pitch) = mode(&mut deltas) else {
        return (0, 1.0);
    };

    let min_cx = anchors.top_left.center_x.min(anchors.bottom_left.center_x);
    let max_cx = anchors
        .top_right
        .center_x
        .max(anchors.bottom_right.center_x);
    let span = (max_cx - min_cx).abs();
    let col_count = (span / pitch).round() as usize + 1;

    (col_count, pitch)
}

/// The most common value, bucketed into bins [`PITCH_BIN_WIDTH`] wide from the smallest one: the
/// centre of the fullest bin (the last of equally full ones). `None` when `values` is empty.
fn mode(values: &mut [f32]) -> Option<f32> {
    values.sort_by(f32::total_cmp);
    let (&min, &max) = (values.first()?, values.last()?);
    let mut bins = vec![0usize; ((max - min) / PITCH_BIN_WIDTH) as usize + 1];
    for &v in values.iter() {
        let bin = (((v - min) / PITCH_BIN_WIDTH) as usize).min(bins.len() - 1);
        bins[bin] += 1;
    }
    let best_bin = bins
        .iter()
        .enumerate()
        .max_by_key(|&(_, &count)| count)
        .map_or(0, |(i, _)| i);
    Some(min + best_bin as f32 * PITCH_BIN_WIDTH + PITCH_BIN_WIDTH / 2.0)
}

/// Rows between the top-left and bottom-left anchors (which share a column, so their vertical
/// span is the most reliable), in average anchor heights.
pub(crate) fn calculate_row_count(anchors: &Anchors) -> usize {
    let top = &anchors.top_left;
    let bottom = &anchors.bottom_left;

    let vertical_distance = ((top.min_y as f32) - (bottom.max_y as f32)).abs();
    let avg_height = (top.bbox_height() + bottom.bbox_height()) / 2.0;

    if avg_height > 0.0 {
        ((vertical_distance / avg_height).round() as usize).max(1)
    } else {
        0
    }
}

/// Columns between the top-left and top-right anchors, from their distance in median anchor
/// widths.
pub(crate) fn calculate_column_count(anchors: &Anchors) -> usize {
    let median_width = median(anchors.all().map(Triangle::bbox_width).to_vec()).max(1.0);
    let left = &anchors.top_left;
    let right = &anchors.top_right;

    // M is the number of triangle widths between corners
    let distance = (right.center_x - left.center_x).abs();
    let m = (distance / median_width).round() as usize;

    trace!(
        left_cx = left.center_x,
        right_cx = right.center_x,
        distance,
        median_width,
        m,
        "naive column count"
    );

    if m < 2 {
        return 0;
    }
    2 * (m - 2) + 1
}
