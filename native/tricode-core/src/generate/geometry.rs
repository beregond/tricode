//! The triangle grid in pixel coordinates, used to decide which cell and border each pixel is on.

use crate::grid::triangle_height;

/// The triangle grid in pixel coordinates, shared by every cell so that neighbouring triangles
/// are cut from the same lines.
///
/// Vertices sit on pixel centres horizontally (`col_pitch` apart, so a triangle's base spans
/// `2 · col_pitch + 1` pixels: the odd triangle width) and on the boundaries between pixel rows
/// vertically, so rows split the image cleanly. Every slanted edge therefore lies on one straight
/// line across all rows, and a shared edge is rounded identically from both sides.
pub(crate) struct TriangleGrid {
    rows: i64,
    cols: i64,
    col_pitch: f64,
    row_pitch: f64,
    /// x of column 0's left vertex.
    origin_x: f64,
    /// y of the line above row 0.
    origin_y: f64,
}

impl TriangleGrid {
    pub(crate) fn new(rows: u32, cols: u32, triangle_width: u32) -> Self {
        let triangle_height = triangle_height(triangle_width);
        Self {
            rows: rows as i64,
            cols: cols as i64,
            col_pitch: (triangle_width / 2) as f64,
            row_pitch: triangle_height as f64,
            origin_x: (triangle_width / 2) as f64,
            origin_y: (triangle_height / 2) as f64 - 0.5,
        }
    }

    /// Whether the cell lies inside the grid.
    pub(crate) fn contains(&self, row: i64, col: i64) -> bool {
        (0..self.rows).contains(&row) && (0..self.cols).contains(&col)
    }

    /// The cell containing the point, which may lie outside the grid.
    pub(crate) fn cell_at(&self, x: f64, y: f64) -> (i64, i64) {
        let v = (y - self.origin_y) / self.row_pitch;
        let row = v.floor() as i64;
        let t = v - row as f64; // 0 at the row's top line, 1 at its bottom line
        let u = (x - self.origin_x) / self.col_pitch;
        // Column strip k (between vertices k and k+1) holds the edge between cells k-1 and k.
        let strip = u.floor() as i64;
        let left = strip - 1;
        let edge = if points_down_i(row, left) {
            (strip + 1) as f64 - t
        } else {
            strip as f64 + t
        };
        (row, if u < edge { left } else { strip })
    }

    /// Corners of a cell's triangle.
    pub(crate) fn vertices(&self, row: i64, col: i64) -> [(f64, f64); 3] {
        let left = self.origin_x + col as f64 * self.col_pitch;
        let (mid, right) = (left + self.col_pitch, left + 2.0 * self.col_pitch);
        let top = self.origin_y + row as f64 * self.row_pitch;
        let bottom = top + self.row_pitch;
        if points_down_i(row, col) {
            [(left, top), (right, top), (mid, bottom)]
        } else {
            [(mid, top), (right, bottom), (left, bottom)]
        }
    }

    /// Lattice coordinates `(k, line)` of a vertex: its column of vertices and its row line.
    fn vertex_index(&self, (x, y): (f64, f64)) -> (i64, i64) {
        (
            ((x - self.origin_x) / self.col_pitch).round() as i64,
            ((y - self.origin_y) / self.row_pitch).round() as i64,
        )
    }

    fn vertex_at(&self, (k, line): (i64, i64)) -> (f64, f64) {
        (
            self.origin_x + k as f64 * self.col_pitch,
            self.origin_y + line as f64 * self.row_pitch,
        )
    }

    /// The six edges leaving a vertex, clockwise on screen from east: as offsets to the vertex at
    /// the far end, in vertex coordinates.
    const RAYS: [(i64, i64); 6] = [(2, 0), (1, 1), (-1, 1), (-2, 0), (-1, -1), (1, -1)];

    /// The six cells around a vertex; cell `i` lies clockwise of ray `i` (between rays `i` and
    /// `i + 1`).
    fn cells_around((k, line): (i64, i64)) -> [(i64, i64); 6] {
        [
            (line, k),
            (line, k - 1),
            (line, k - 2),
            (line - 1, k - 2),
            (line - 1, k - 1),
            (line - 1, k),
        ]
    }

    fn ray_vector(&self, ray: usize) -> (f64, f64) {
        let (dk, dline) = Self::RAYS[ray];
        (dk as f64 * self.col_pitch, dline as f64 * self.row_pitch)
    }

    /// Whether a point inside `(row, col)` lies on a border: within `width` of an edge between
    /// its shape and another, on its own side.
    ///
    /// Each edge's border is a strip cut off at both ends by the line halving the corner there
    /// (a mitre join, as when stroking a polygon outline). At outward corners the strips meet and
    /// cut the corner off cleanly; at inward ones they meet in a sharp point instead of the round
    /// fillet that measuring to the nearest point of another shape would give.
    pub(crate) fn on_border<S: PartialEq>(
        &self,
        (x, y): (f64, f64),
        (row, col): (i64, i64),
        width: f64,
        shape_of: impl Fn(i64, i64) -> Option<S>,
    ) -> bool {
        let corners = self.vertices(row, col);
        // A strip within reach has to come within `width` of this cell's own edges first.
        let near_own_edge =
            (0..3).any(|i| distance_to_segment((x, y), corners[i], corners[(i + 1) % 3]) < width);
        if !near_own_edge {
            return false;
        }

        let own = shape_of(row, col);
        let in_shape = |(r, c): (i64, i64)| shape_of(r, c) == own;
        // Every edge within reach leaves one of this cell's corners.
        corners.iter().any(|&corner| {
            let vertex = self.vertex_index(corner);
            let around = Self::cells_around(vertex);
            (0..6).any(|ray| {
                // The edge separates the shape from another when exactly one side is in it.
                let clockwise_in = in_shape(around[ray]);
                if clockwise_in == in_shape(around[(ray + 5) % 6]) {
                    return false;
                }
                let (dx, dy) = self.ray_vector(ray);
                let (px, py) = (x - corner.0, y - corner.1);
                // On the shape's side of the edge, and within `width` of its line.
                let cross = dx * py - dy * px;
                if (cross > 0.0) != clockwise_in || cross.abs() / dx.hypot(dy) >= width {
                    return false;
                }
                let far =
                    self.vertex_at((vertex.0 + Self::RAYS[ray].0, vertex.1 + Self::RAYS[ray].1));
                self.within_mitre((x, y), vertex, ray, clockwise_in, &in_shape)
                    && self.within_mitre(
                        (x, y),
                        self.vertex_index(far),
                        (ray + 3) % 6,
                        !clockwise_in,
                        &in_shape,
                    )
            })
        })
    }

    /// Whether the point lies on the edge's side of the line halving the shape's corner at
    /// `vertex`, where the edge is `ray` and the shape is on its clockwise side if `clockwise`.
    fn within_mitre(
        &self,
        (x, y): (f64, f64),
        vertex: (i64, i64),
        ray: usize,
        clockwise: bool,
        in_shape: &impl Fn((i64, i64)) -> bool,
    ) -> bool {
        let around = Self::cells_around(vertex);
        // How many 60° sectors the shape's corner spans, starting next to the edge.
        let sector = |step: usize| {
            if clockwise {
                (ray + step) % 6
            } else {
                (ray + 6 - 1 - step) % 6
            }
        };
        let span = (0..6)
            .take_while(|&step| in_shape(around[sector(step)]))
            .count();
        let other_ray = if clockwise {
            (ray + span) % 6
        } else {
            (ray + 6 - span % 6) % 6
        };
        let unit = |(dx, dy): (f64, f64)| {
            let length = dx.hypot(dy);
            (dx / length, dy / length)
        };
        let (ax, ay) = unit(self.ray_vector(ray));
        let (bx, by) = unit(self.ray_vector(other_ray));
        // The halving line's direction: through the shape for corners under 180°, out of it
        // (pointing away from both edges) for wider ones.
        let (hx, hy) = match span {
            0..=2 => (ax + bx, ay + by),
            3 => {
                if clockwise {
                    (-ay, ax)
                } else {
                    (ay, -ax)
                }
            }
            _ => (-(ax + bx), -(ay + by)),
        };
        let (vx, vy) = self.vertex_at(vertex);
        let side = |(px, py): (f64, f64)| hx * py - hy * px;
        side((x - vx, y - vy)) * side((ax, ay)) >= 0.0
    }
}

/// [`points_down`] for cell indices that may lie outside the grid.
fn points_down_i(row: i64, col: i64) -> bool {
    (row + col).rem_euclid(2) == 0
}

fn distance_to_segment((px, py): (f64, f64), (ax, ay): (f64, f64), (bx, by): (f64, f64)) -> f64 {
    let (dx, dy) = (bx - ax, by - ay);
    let t = (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    (px - ax - t * dx).hypot(py - ay - t * dy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{MAX_TRIANGLE_WIDTH, MIN_TRIANGLE_WIDTH};
    use test_log::test;

    #[test]
    fn cells_around_a_vertex_match_the_cells_found_there() {
        for triangle_width in [MIN_TRIANGLE_WIDTH, MAX_TRIANGLE_WIDTH] {
            let grid = TriangleGrid::new(6, 9, triangle_width);
            for line in 1..6 {
                // Vertices on a line sit every other column, alternating between lines.
                for k in (line % 2..10).step_by(2) {
                    let (vx, vy) = grid.vertex_at((k, line));
                    for (i, &cell) in TriangleGrid::cells_around((k, line)).iter().enumerate() {
                        // A point a few pixels into the sector between rays i and i + 1.
                        let (ax, ay) = grid.ray_vector(i);
                        let (bx, by) = grid.ray_vector((i + 1) % 6);
                        let (dx, dy) = (ax + bx, ay + by);
                        let scale = 4.0 / dx.hypot(dy);
                        let point = (vx + dx * scale, vy + dy * scale);
                        assert_eq!(
                            grid.cell_at(point.0, point.1),
                            cell,
                            "width {triangle_width}, vertex ({k}, {line}), sector {i}"
                        );
                    }
                }
            }
        }
    }
}
