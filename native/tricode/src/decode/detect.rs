//! Finding triangles in a black-and-white image, and the four corner anchors among them.

use super::DARK_LUMA;
use image::GrayImage;
use imageproc::contrast::{ThresholdType, threshold};
use imageproc::distance_transform::Norm;
use imageproc::morphology::dilate;
use tracing::trace;

/// A triangle whose bounding-box area exceeds this multiple of the largest anchor area
/// is considered a cluster/merge false positive and is discarded before grid reading.
const FALSE_POSITIVE_AREA_FACTOR: f32 = 1.2;

/// Triangles fill about half of their bounding box; blobs outside this range aren't triangles.
const FILL_RATIO: std::ops::RangeInclusive<f32> = 0.3..=0.8;

/// Blobs of fewer pixels than this are noise.
const MIN_AREA: usize = 20;

/// Accepted bounding-box width / height of a triangle.
const ASPECT_RATIO: std::ops::RangeInclusive<f32> = 0.3..=3.0;

/// A black blob shaped like a triangle: its centroid, orientation and bounding box.
#[derive(Debug, Clone)]
pub(crate) struct Triangle {
    pub(crate) center_x: f32,
    pub(crate) center_y: f32,
    /// Apex at the top; otherwise the triangle points down.
    pub(crate) pointing_up: bool,
    pub(crate) min_x: u32,
    pub(crate) min_y: u32,
    pub(crate) max_x: u32,
    pub(crate) max_y: u32,
}

impl Triangle {
    /// Bounding-box width, as the distance between its outermost pixel columns.
    pub(crate) fn bbox_width(&self) -> f32 {
        (self.max_x - self.min_x) as f32
    }

    /// Bounding-box height, as the distance between its outermost pixel rows.
    pub(crate) fn bbox_height(&self) -> f32 {
        (self.max_y - self.min_y) as f32
    }

    fn bbox_area(&self) -> f32 {
        self.bbox_width() * self.bbox_height()
    }
}

/// Find triangles in a grayscale image using connected component analysis.
///
/// The black shapes are eroded by `erosion` pixels before they are traced, which cuts the thin
/// bridges between touching triangles, and each traced blob is then grown back to its original size.
///
/// Possible improvement: erode-then-grow rounds off the triangle tips, and small triangles lose a
/// noticeable part of their shape. Using the eroded blobs only as seeds and assigning every black
/// pixel of the original image to its nearest seed (a watershed-style split) would keep the exact
/// shapes, and so more accurate centroids and orientations.
pub(crate) fn find_triangles(img: &GrayImage, erosion: u8) -> Result<Vec<Triangle>, String> {
    let (width, height) = img.dimensions();
    if width == 0 || height == 0 {
        return Err("Invalid image dimensions".to_string());
    }

    // The image is already black and white (see `decode_image`); this only makes it binary for
    // images passed in directly by tests.
    let binary = threshold(img, DARK_LUMA, ThresholdType::Binary);

    // Erode the black regions to separate touching triangles. imageproc treats non-zero (white)
    // pixels as the foreground, so growing the white background by `erosion` erodes the black.
    let eroded = dilate(&binary, Norm::LInf, erosion);

    let mut triangles = Vec::new();
    let mut visited = vec![vec![false; width as usize]; height as usize];
    // Which component last claimed each pixel while growing back, so the buffer is shared.
    let mut grown_by = vec![0u32; (width * height) as usize];
    let mut component_id = 0u32;

    for y in 0..height {
        for x in 0..width {
            if visited[y as usize][x as usize] || eroded.get_pixel(x, y)[0] != 0 {
                continue;
            }
            let eroded_component = flood_fill(&eroded, &mut visited, x, y);
            component_id += 1;
            let component = dilate_component(
                eroded_component,
                &binary,
                erosion,
                &mut grown_by,
                component_id,
            );
            triangles.extend(analyze_component(&component));
        }
    }

    Ok(triangles)
}

/// Grow a component back by up to `radius` steps (8-connected), only through pixels that are black
/// in the original binary image, so it can't reach across a gap into a neighbouring triangle.
/// `grown_by` marks the pixels already in the component with `id`; it is reused across components.
fn dilate_component(
    mut component: Vec<(u32, u32)>,
    binary: &GrayImage,
    radius: u8,
    grown_by: &mut [u32],
    id: u32,
) -> Vec<(u32, u32)> {
    let (width, height) = binary.dimensions();
    let index = |x: u32, y: u32| (y * width + x) as usize;

    for &(x, y) in &component {
        grown_by[index(x, y)] = id;
    }

    // Breadth-first: each step expands only the pixels added by the previous one.
    let mut frontier_start = 0;
    for _ in 0..radius {
        let frontier_end = component.len();
        for i in frontier_start..frontier_end {
            let (x, y) = component[i];
            for ny in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                    let n = index(nx, ny);
                    if grown_by[n] != id && binary.get_pixel(nx, ny)[0] == 0 {
                        grown_by[n] = id;
                        component.push((nx, ny));
                    }
                }
            }
        }
        frontier_start = frontier_end;
    }

    component
}

/// The black pixels 4-connected to `(start_x, start_y)`, marking them in `visited`.
fn flood_fill(
    binary: &GrayImage,
    visited: &mut [Vec<bool>],
    start_x: u32,
    start_y: u32,
) -> Vec<(u32, u32)> {
    let (width, height) = binary.dimensions();
    let mut component = Vec::new();
    let mut stack = vec![(start_x, start_y)];

    while let Some((x, y)) = stack.pop() {
        if x >= width
            || y >= height
            || visited[y as usize][x as usize]
            || binary.get_pixel(x, y)[0] != 0
        {
            continue;
        }

        visited[y as usize][x as usize] = true;
        component.push((x, y));

        if x > 0 {
            stack.push((x - 1, y));
        }
        if x < width - 1 {
            stack.push((x + 1, y));
        }
        if y > 0 {
            stack.push((x, y - 1));
        }
        if y < height - 1 {
            stack.push((x, y + 1));
        }
    }

    component
}

/// The triangle a connected component makes, or `None` when its size or shape rules it out.
fn analyze_component(component: &[(u32, u32)]) -> Option<Triangle> {
    if component.len() < MIN_AREA {
        return None;
    }

    let (mut min_x, mut min_y, mut max_x, mut max_y) = (u32::MAX, u32::MAX, 0, 0);
    let (mut sum_x, mut sum_y) = (0u64, 0u64);
    for &(x, y) in component {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
        sum_x += x as u64;
        sum_y += y as u64;
    }

    let area = component.len() as f32;
    let width = (max_x - min_x + 1) as f32;
    let height = (max_y - min_y + 1) as f32;
    if !FILL_RATIO.contains(&(area / (width * height))) || !ASPECT_RATIO.contains(&(width / height))
    {
        return None;
    }

    // A triangle has more of its pixels on the side of its base: in the bottom half when its apex
    // points up.
    let mid_y = (min_y + max_y) / 2;
    let top_pixels = component.iter().filter(|&&(_, y)| y < mid_y).count();
    let pointing_up = component.len() - top_pixels > top_pixels;

    Some(Triangle {
        center_x: sum_x as f32 / area,
        center_y: sum_y as f32 / area,
        pointing_up,
        min_x,
        min_y,
        max_x,
        max_y,
    })
}

/// The four corner anchor triangles of a tricode grid.
#[derive(Debug, Clone)]
pub(crate) struct Anchors {
    pub(crate) top_left: Triangle,
    pub(crate) top_right: Triangle,
    pub(crate) bottom_left: Triangle,
    pub(crate) bottom_right: Triangle,
}

impl Anchors {
    /// All four anchors, for iteration.
    pub(crate) fn all(&self) -> [&Triangle; 4] {
        [
            &self.top_left,
            &self.top_right,
            &self.bottom_left,
            &self.bottom_right,
        ]
    }
}

/// Finds the four corner anchors: the triangles whose bounding-box centres lie closest to each
/// corner of the box around all triangles.
pub(crate) fn find_anchors(triangles: &[Triangle]) -> Option<Anchors> {
    let max_x = triangles.iter().map(|t| t.max_x).max()? as f32;
    let max_y = triangles.iter().map(|t| t.max_y).max()? as f32;
    let closest_to = |(corner_x, corner_y): (f32, f32)| {
        let distance = |t: &Triangle| {
            let center_x = (t.min_x as f32 + t.max_x as f32) / 2.0;
            let center_y = (t.min_y as f32 + t.max_y as f32) / 2.0;
            (center_x - corner_x).powi(2) + (center_y - corner_y).powi(2)
        };
        // The first of equally close triangles wins.
        triangles
            .iter()
            .min_by(|a, b| distance(a).total_cmp(&distance(b)))
            .cloned()
    };

    Some(Anchors {
        top_left: closest_to((0.0, 0.0))?,
        top_right: closest_to((max_x, 0.0))?,
        bottom_left: closest_to((0.0, max_y))?,
        bottom_right: closest_to((max_x, max_y))?,
    })
}

/// Remove false positives: any triangle significantly larger than the biggest anchor
/// is a cluster/merge artefact and should be discarded.
pub(crate) fn discard_merged_triangles(
    triangles: Vec<Triangle>,
    anchors: &Anchors,
) -> Vec<Triangle> {
    let max_anchor_area = anchors
        .all()
        .iter()
        .map(|t| t.bbox_area())
        .fold(0.0f32, f32::max);
    let area_threshold = max_anchor_area * FALSE_POSITIVE_AREA_FACTOR;
    let before = triangles.len();
    let triangles: Vec<Triangle> = triangles
        .into_iter()
        .filter(|t| t.bbox_area() <= area_threshold)
        .collect();
    trace!(
        max_anchor_area,
        area_threshold,
        removed = before - triangles.len(),
        remaining = triangles.len(),
        "false-positive filter"
    );
    triangles
}
