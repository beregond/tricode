//! Colour variance: every shape's colour shifted a little in brightness, picked by the seed.

use super::overlay::over;
use super::render::Shape;
use image::Rgba;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Largest shift of a shape's HSV value, in percentage points; shapes only get lighter.
const MAX_VALUE_SHIFT: i32 = 7;

/// The colour every cell is painted in: its shape's colour composited over the background. The
/// colours whose flag in `vary` (primary, secondary) is set are shifted by [`shift_value`], the
/// same for every cell of a shape, picked by `seed`. Cells the overlay cleared take the background.
pub(crate) fn cell_colours(
    shapes: &[Vec<Option<Shape>>],
    [primary, secondary, bg]: [Rgba<u8>; 3],
    seed: u64,
    [vary_primary, vary_secondary]: [bool; 2],
) -> Vec<Vec<Rgba<u8>>> {
    shapes
        .iter()
        .map(|row| {
            row.iter()
                .map(|shape| {
                    let Some(shape) = shape else { return bg };
                    let (base, vary) = if shape.primary {
                        (primary, vary_primary)
                    } else {
                        (secondary, vary_secondary)
                    };
                    let shaded = if vary {
                        shift_value(base, value_shift(seed, shape))
                    } else {
                        base
                    };
                    over(shaded, bg)
                })
                .collect()
        })
        .collect()
}

/// A shape's shift, 0 to [`MAX_VALUE_SHIFT`] points, from the seed and the shape alone.
fn value_shift(seed: u64, shape: &Shape) -> i32 {
    let mut hasher = DefaultHasher::new();
    (seed, shape).hash(&mut hasher);
    (hasher.finish() % (MAX_VALUE_SHIFT as u64 + 1)) as i32
}

/// `colour` with its HSV value moved by `points` percentage points, clamped to 0–100 % rather than
/// wrapping. Hue, saturation and alpha stay; black, having no hue, turns grey.
fn shift_value(colour: Rgba<u8>, points: i32) -> Rgba<u8> {
    let [r, g, b, a] = colour.0;
    let value = r.max(g).max(b) as f64 / 255.0;
    let target = (value + points as f64 / 100.0).clamp(0.0, 1.0);
    let channel = |c: u8| {
        let shifted = if value == 0.0 {
            target * 255.0
        } else {
            c as f64 * target / value
        };
        shifted.round() as u8
    };
    Rgba([channel(r), channel(g), channel(b), a])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shifts_value_keeping_hue_and_clamping() {
        // Value 50 % → 60 %: every channel scales by the same factor.
        assert_eq!(
            shift_value(Rgba([128, 64, 0, 200]), 10),
            Rgba([154, 77, 0, 200])
        );
        assert_eq!(shift_value(Rgba([0, 0, 0, 255]), -10), Rgba([0, 0, 0, 255]));
        assert_eq!(
            shift_value(Rgba([0, 0, 0, 255]), 10),
            Rgba([26, 26, 26, 255])
        );
        assert_eq!(
            shift_value(Rgba([255, 255, 255, 255]), 5),
            Rgba([255, 255, 255, 255])
        );
    }

    #[test]
    fn varies_only_the_flagged_colour() {
        let shapes: Vec<Vec<Option<Shape>>> = vec![
            (0..40)
                .map(|id| Some(Shape::new(id, id % 2 == 0)))
                .collect(),
        ];
        let (primary, secondary, bg) = (
            Rgba([0x40, 0x60, 0x80, 255]),
            Rgba([0xC0, 0xC0, 0xC0, 255]),
            Rgba([255, 255, 255, 255]),
        );
        let colours = cell_colours(&shapes, [primary, secondary, bg], 7, [true, false]);
        let (primaries, secondaries): (Vec<_>, Vec<_>) =
            colours[0].iter().enumerate().partition(|(i, _)| i % 2 == 0);
        assert!(primaries.iter().any(|&(_, &c)| c != primary));
        assert!(secondaries.iter().all(|&(_, &c)| c == secondary));
    }

    #[test]
    fn shifts_only_lighten_and_use_the_whole_range() {
        let shifts: Vec<i32> = (0..500)
            .map(|id| value_shift(7, &Shape::new(id, id % 2 == 0)))
            .collect();
        assert!(shifts.iter().all(|s| (0..=MAX_VALUE_SHIFT).contains(s)));
        assert!(shifts.contains(&0) && shifts.contains(&MAX_VALUE_SHIFT));
    }
}
