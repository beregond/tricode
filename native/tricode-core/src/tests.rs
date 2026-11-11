//! End-to-end tests: generate codes, damage or transform them, and decode them back.

use crate::decode::*;
use crate::frame::*;
use crate::generate::*;
use crate::grid::*;
use crate::mask::*;
use image::codecs::png::PngEncoder;
use image::imageops::{FilterType, resize};
use image::{ExtendedColorType, ImageEncoder, Rgba, RgbaImage};
use test_log::test;

const STYLES: [ImageStyle; 3] = [ImageStyle::Tiles, ImageStyle::Stain, ImageStyle::Horizon];
const RATIOS: [(u32, u32); 3] = [(1, 1), (16, 9), (3, 4)];
/// Kept small: triangle detection is slow in debug builds.
const PAYLOAD_LENGTHS: [usize; 3] = [5, 40, 150];

/// Generation parameters for `text` with the defaults the tests share: black Tiles on white, 1:1,
/// default size and level, seed 42, no colour variance.
pub(crate) fn params(text: &str) -> GenerateImageParams<'_> {
    GenerateImageParams {
        payload: text,
        payload_type: "text",
        width_ratio: 1,
        height_ratio: 1,
        style: ImageStyle::Tiles,
        primary_color: 0xFF000000,
        secondary_color: 0xFFFFFFFF,
        bg_color: 0xFFFFFFFF,
        seed: 42,
        primary_variance: false,
        secondary_variance: false,
        mask: None,
        triangle_width: DEFAULT_TRIANGLE_WIDTH,
        ecc_level: DEFAULT_ECC_LEVEL,
        overlay: None,
    }
}

pub(crate) fn payload(len: usize) -> String {
    (0..len).map(|i| (b'a' + (i % 26) as u8) as char).collect()
}

fn generate(style: ImageStyle, ratio: (u32, u32), len: usize) -> GenerateDebug {
    generate_at(style, ratio, len, DEFAULT_TRIANGLE_WIDTH)
}

fn generate_at(
    style: ImageStyle,
    ratio: (u32, u32),
    len: usize,
    triangle_width: u32,
) -> GenerateDebug {
    generate_image_debug(GenerateImageParams {
        width_ratio: ratio.0,
        height_ratio: ratio.1,
        style,
        triangle_width,
        ..params(&payload(len))
    })
    .expect("generation failed")
}

/// Every style × ratio × payload length, with a label for assertion messages.
pub(crate) fn all_cases() -> impl Iterator<Item = (String, usize, GenerateDebug)> {
    STYLES.into_iter().flat_map(|style| {
        RATIOS.into_iter().flat_map(move |ratio| {
            PAYLOAD_LENGTHS.into_iter().map(move |len| {
                let label = format!("{:?} {:?} len={}", style, ratio, len);
                (label, len, generate(style, ratio, len))
            })
        })
    })
}

fn rescale(png: &[u8], factor: f32) -> Vec<u8> {
    let img = image::load_from_memory(png)
        .expect("load failed")
        .to_rgba8();
    let width = (img.width() as f32 * factor).round() as u32;
    let height = (img.height() as f32 * factor).round() as u32;
    let scaled = resize(&img, width, height, FilterType::Triangle);
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(scaled.as_raw(), width, height, ExtendedColorType::Rgba8)
        .expect("encode failed");
    out
}

fn assert_decodes_exactly(label: &str, len: usize, truth: (usize, usize), png: &[u8]) {
    let decoded = decode_image(png).expect("decode errored");
    assert_eq!(
        (decoded.row_count, decoded.column_count),
        truth,
        "{label}: wrong grid"
    );
    assert_eq!(
        decoded.decode_method, "lattice-fit",
        "{label}: needed a search"
    );
    assert_eq!(
        decoded.text.as_deref(),
        Some(payload(len).as_str()),
        "{label}: wrong text"
    );
}

pub(crate) fn grid_of(generated: &GenerateDebug) -> (usize, usize) {
    (
        generated.grid_rows as usize,
        generated.grid_columns as usize,
    )
}

/// Committed decode fixtures, named `{style}_{w}x{h}_len{len}_{rows}x{cols}.png`. They pin
/// the decoder against images from a known renderer, so a renderer change can't hide a
/// decoder regression.
const FIXTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/decode");
const FIXTURE_PAYLOAD_LENGTHS: [usize; 4] = [5, 40, 150, 400];
/// Fixtures are rendered at the original 51 px width, so they also cover a non-default size.
const FIXTURE_TRIANGLE_WIDTH: u32 = 51;

/// Rewrites the fixtures from the current renderer. Only run after an intentional format change:
/// `tricode-regen-fixtures` (or `cargo test -p tricode-core --release -- --ignored regenerate_decode_fixtures`)
#[test]
#[ignore]
fn regenerate_decode_fixtures() {
    std::fs::create_dir_all(FIXTURES_DIR).expect("create fixtures dir");
    for style in STYLES {
        for ratio in RATIOS {
            for len in FIXTURE_PAYLOAD_LENGTHS {
                let generated = generate_at(style, ratio, len, FIXTURE_TRIANGLE_WIDTH);
                let name = format!(
                    "{}_{}x{}_len{}_{}x{}.png",
                    format!("{:?}", style).to_lowercase(),
                    ratio.0,
                    ratio.1,
                    len,
                    generated.grid_rows,
                    generated.grid_columns
                );
                std::fs::write(
                    format!("{FIXTURES_DIR}/{name}"),
                    to_grayscale_png(&generated.png_bytes),
                )
                .expect("write fixture");
            }
        }
    }
}

/// Re-encodes a rendered (pure black and white) PNG as 8-bit grayscale, which the decoder
/// sees identically but takes about a fifth of the space.
fn to_grayscale_png(png: &[u8]) -> Vec<u8> {
    let gray = image::load_from_memory(png)
        .expect("load failed")
        .to_luma8();
    let mut out = Vec::new();
    PngEncoder::new_with_quality(
        &mut out,
        image::codecs::png::CompressionType::Best,
        image::codecs::png::FilterType::Adaptive,
    )
    .write_image(
        gray.as_raw(),
        gray.width(),
        gray.height(),
        ExtendedColorType::L8,
    )
    .expect("encode failed");
    out
}

/// Parses `{style}_{w}x{h}_len{len}_{rows}x{cols}.png` into `(len, (rows, cols))`.
fn parse_fixture_name(name: &str) -> Option<(usize, (usize, usize))> {
    let mut parts = name.strip_suffix(".png")?.split('_');
    let len = parts.nth(2)?.strip_prefix("len")?.parse().ok()?;
    let (rows, cols) = parts.next()?.split_once('x')?;
    Some((len, (rows.parse().ok()?, cols.parse().ok()?)))
}

#[test]
fn decodes_fixtures() {
    let mut names: Vec<String> = std::fs::read_dir(FIXTURES_DIR)
        .expect("fixtures dir missing")
        .map(|entry| {
            entry
                .expect("read fixtures dir")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.ends_with(".png"))
        .collect();
    names.sort();
    assert_eq!(
        names.len(),
        STYLES.len() * RATIOS.len() * FIXTURE_PAYLOAD_LENGTHS.len(),
        "fixture count"
    );
    for name in names {
        let (len, truth) = parse_fixture_name(&name).expect("bad fixture name");
        let png = std::fs::read(format!("{FIXTURES_DIR}/{name}")).expect("read fixture");
        assert_decodes_exactly(&name, len, truth, &png);
    }
}

/// Mirrors a PNG horizontally and/or vertically, as a selfie camera or a code seen through
/// glass would.
fn flip(png: &[u8], horizontally: bool, vertically: bool) -> Vec<u8> {
    let mut img = image::load_from_memory(png)
        .expect("load failed")
        .to_luma8();
    if horizontally {
        img = image::imageops::flip_horizontal(&img);
    }
    if vertically {
        img = image::imageops::flip_vertical(&img);
    }
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            ExtendedColorType::L8,
        )
        .expect("encode failed");
    out
}

/// Decodes every fixture flipped horizontally, vertically and both ways.
fn assert_fixtures_decode_flipped(horizontally: bool, vertically: bool, suffix: &str) {
    for entry in std::fs::read_dir(FIXTURES_DIR).expect("fixtures dir missing") {
        let name = entry.expect("read fixtures dir").file_name();
        let name = name.to_string_lossy();
        let Some((len, truth)) = parse_fixture_name(&name) else {
            continue;
        };
        let png = std::fs::read(format!("{FIXTURES_DIR}/{name}")).expect("read fixture");
        let decoded = decode_image(&flip(&png, horizontally, vertically)).expect("decode errored");
        let label = format!("{name} {suffix}");
        assert_eq!(
            (decoded.row_count, decoded.column_count),
            truth,
            "{label}: wrong grid"
        );
        assert_eq!(
            decoded.text.as_deref(),
            Some(payload(len).as_str()),
            "{label}: wrong text"
        );
        assert_eq!(
            decoded.decode_method,
            format!("lattice-fit ({suffix})"),
            "{label}: wrong method"
        );
    }
}

#[test]
fn decodes_horizontally_flipped_fixtures() {
    assert_fixtures_decode_flipped(true, false, "flipped horizontally");
}

#[test]
fn decodes_vertically_flipped_fixtures() {
    assert_fixtures_decode_flipped(false, true, "flipped vertically");
}

#[test]
fn decodes_fixtures_flipped_both_ways() {
    assert_fixtures_decode_flipped(true, true, "flipped both ways");
}

/// Swaps black and white, as a code drawn in light colours on a dark background would be.
fn invert(png: &[u8]) -> Vec<u8> {
    let mut img = image::load_from_memory(png)
        .expect("load failed")
        .to_luma8();
    image::imageops::invert(&mut img);
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            ExtendedColorType::L8,
        )
        .expect("encode failed");
    out
}

#[test]
fn decodes_inverted_fixtures() {
    for entry in std::fs::read_dir(FIXTURES_DIR).expect("fixtures dir missing") {
        let name = entry.expect("read fixtures dir").file_name();
        let name = name.to_string_lossy();
        let Some((len, truth)) = parse_fixture_name(&name) else {
            continue;
        };
        let png = std::fs::read(format!("{FIXTURES_DIR}/{name}")).expect("read fixture");
        let decoded = decode_image(&invert(&png)).expect("decode errored");
        assert_eq!(
            (decoded.row_count, decoded.column_count),
            truth,
            "{name} inverted: wrong grid"
        );
        assert_eq!(
            decoded.text.as_deref(),
            Some(payload(len).as_str()),
            "{name} inverted: wrong text"
        );
        assert_eq!(
            decoded.decode_method, "lattice-fit (inverted)",
            "{name} inverted: wrong method"
        );
    }
}

#[test]
fn warns_when_the_code_does_not_read_back() {
    let generate_in = |primary_color, bg_color| {
        generate_image_debug(GenerateImageParams {
            primary_color,
            secondary_color: bg_color,
            bg_color,
            ..params(&payload(40))
        })
        .expect("generation failed")
    };
    // Both light, both dark, and light on dark: far enough apart, so they all read.
    for (primary, bg) in [
        (0xFF90ADD3, 0xFFC0CAD7),
        (0xFF1B2A41, 0xFF5A6A80),
        (0xFFC0CAD7, 0xFF1E2A33),
    ] {
        let generated = generate_in(primary, bg);
        assert_eq!(generated.decode_warning, None, "{primary:08X} on {bg:08X}");
    }
    let warning = generate_in(0xFFB4BECB, 0xFFC0CAD7).decode_warning;
    assert!(
        warning
            .as_deref()
            .is_some_and(|w| w.contains("too close in brightness")),
        "{warning:?}"
    );
}

/// Secondary in their own colour, between the primary colour and the background, are drawn and
/// still read as zeros.
#[test]
fn decodes_secondary_triangles_in_their_own_colour() {
    for style in STYLES {
        let generated = generate_image_debug(GenerateImageParams {
            style,
            primary_color: 0xFF1B2A41,
            secondary_color: 0xFFB8C4D0,
            bg_color: 0xFFFFFFFF,
            ..params(&payload(40))
        })
        .expect("generation failed");
        assert_eq!(generated.decode_warning, None, "{style:?}");
        let img = image::load_from_memory(&generated.png_bytes)
            .expect("load failed")
            .to_rgba8();
        assert!(
            img.pixels().any(|p| p.0 == [0xB8, 0xC4, 0xD0, 0xFF]),
            "{style:?}: no secondary triangle drawn"
        );
    }
}

/// A translucent secondary colour is drawn over the background rather than replacing it, and a
/// transparent one leaves the background showing.
#[test]
fn draws_the_secondary_colour_over_the_background() {
    let pixels = |secondary_color| {
        let generated = generate_image_debug(GenerateImageParams {
            style: ImageStyle::Stain,
            secondary_color,
            bg_color: 0xFF0000FF,
            primary_color: 0xFFFFFFFF,
            ..params(&payload(40))
        })
        .expect("generation failed");
        let img = image::load_from_memory(&generated.png_bytes)
            .expect("load failed")
            .to_rgba8();
        img.pixels()
            .map(|p| p.0)
            .collect::<std::collections::HashSet<_>>()
    };
    // Half-opaque red over opaque blue.
    assert!(pixels(0x80FF0000).contains(&[0x80, 0x00, 0x7F, 0xFF]));
    let transparent = pixels(0x00FF0000);
    assert!(transparent.contains(&[0x00, 0x00, 0xFF, 0xFF]));
    assert!(
        transparent.iter().all(|p| p[3] == 0xFF),
        "a pixel lost the background"
    );
}

/// Colour variance shades the shapes differently, the same way for the same seed, and the code
/// still decodes.
#[test]
fn colour_variance_shades_shapes_by_seed() {
    for style in STYLES {
        let generate_with = |seed| {
            generate_image_debug(GenerateImageParams {
                style,
                seed,
                primary_variance: true,
                secondary_variance: true,
                ..params(&payload(40))
            })
            .expect("generation failed")
        };
        let generated = generate_with(42);
        assert_eq!(generated.decode_warning, None, "{style:?}");
        assert_eq!(
            generated.png_bytes,
            generate_with(42).png_bytes,
            "{style:?}"
        );
        assert_ne!(
            generated.png_bytes,
            generate_with(43).png_bytes,
            "{style:?}"
        );
        let img = image::load_from_memory(&generated.png_bytes)
            .expect("load failed")
            .to_rgba8();
        let shades: std::collections::HashSet<_> = img.pixels().map(|p| p.0).collect();
        assert!(shades.len() > 10, "{style:?}: only {} shades", shades.len());
    }
}

/// Two light colours (as the Zero Dawn preset uses) are split by Otsu's cut-off at once; the
/// fixed mid-grey one would see no triangles at all.
#[test]
fn decodes_two_light_colours_without_a_fallback() {
    for style in STYLES {
        let generated = generate_image_debug(GenerateImageParams {
            style,
            primary_color: 0xFF90ADD3,
            secondary_color: 0xFFC0CAD7,
            bg_color: 0xFFC0CAD7,
            ..params(&payload(40))
        })
        .expect("generation failed");
        let decoded = decode_image(&generated.png_bytes).expect("decode errored");
        assert_eq!(
            decoded.text.as_deref(),
            Some(payload(40).as_str()),
            "{style:?}"
        );
        assert_eq!(decoded.decode_method, "lattice-fit", "{style:?}");
    }
}

/// A partial code (a photo that cut most of it off) estimates a grid under 2×2, which used to
/// overflow when the failure was reported.
#[test]
fn fails_cleanly_on_a_partial_code() {
    let code = generate(ImageStyle::Tiles, (1, 1), 40);
    let full = image::load_from_memory(&code.png_bytes)
        .expect("load failed")
        .to_luma8();
    let (w, h) = full.dimensions();
    for (x, y, width, height) in [(0, 0, w / 3, h / 3), (0, 0, w / 8, h)] {
        let part = image::imageops::crop_imm(&full, x, y, width, height).to_image();
        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(part.as_raw(), width, height, ExtendedColorType::L8)
            .expect("encode failed");
        if let Ok(decoded) = decode_image(&png) {
            assert_eq!(decoded.text, None, "{width}×{height} decoded");
        }
    }
}

/// A code generated with light triangles on a dark background, in colour, and flipped.
#[test]
fn decodes_a_negative_colour_code_flipped() {
    for style in STYLES {
        let generated = generate_image_debug(GenerateImageParams {
            style,
            primary_color: 0xFFFFE680,
            secondary_color: 0xFF102040,
            bg_color: 0xFF102040,
            ..params(&payload(40))
        })
        .expect("generation failed");
        let decoded =
            decode_image(&flip(&generated.png_bytes, true, false)).expect("decode errored");
        assert_eq!(
            decoded.text.as_deref(),
            Some(payload(40).as_str()),
            "{style:?}: wrong text"
        );
        assert_eq!(
            decoded.decode_method, "lattice-fit (flipped horizontally) (inverted)",
            "{style:?}: wrong method"
        );
    }
}

/// Codes holding bare UTF-8 instead of a payload envelope must decode in every orientation,
/// so telling the right reading apart can't rely on the envelope.
#[test]
fn decodes_flipped_codes_without_an_envelope() {
    for style in STYLES {
        for ratio in RATIOS {
            // Read unflipped, a horizontally flipped 1×1 code of length 218 unpacks as the
            // msgpack integer 11, so it fails when only the envelope is checked.
            let texts = [5, 40, 150, 218]
                .map(payload)
                .into_iter()
                .chain(["a".to_string(), "zażółć gęślą jaźń 🔺".to_string()]);
            for text in texts {
                let generated = create_image_from_payload(
                    text.as_bytes(),
                    &GenerateImageParams {
                        payload_type: "",
                        width_ratio: ratio.0,
                        height_ratio: ratio.1,
                        style,
                        ..params("")
                    },
                )
                .expect("generation failed");
                for flip_case in FLIPS {
                    let png = flip(
                        &generated.png_bytes,
                        flip_case.horizontal,
                        flip_case.vertical,
                    );
                    let decoded = decode_image(&png).expect("decode errored");
                    let label = format!("{:?} {:?} {:?} {:?}", style, ratio, text, flip_case);
                    assert_eq!(
                        (decoded.content_type.as_str(), decoded.text.as_deref()),
                        ("plain", Some(text.as_str())),
                        "{label}: wrong text"
                    );
                    let expected_method = match flip_case.name() {
                        Some(name) => format!("lattice-fit ({name})"),
                        None => "lattice-fit".to_string(),
                    };
                    assert_eq!(
                        decoded.decode_method, expected_method,
                        "{label}: wrong method"
                    );
                }
            }
        }
    }
}

fn generate_with_level(style: ImageStyle, len: usize, ecc_level: EccLevel) -> GenerateDebug {
    generate_image_debug(GenerateImageParams {
        width_ratio: 16,
        height_ratio: 9,
        style,
        ecc_level,
        ..params(&payload(len))
    })
    .expect("generation failed")
}

/// What is painted over the centre of a code.
#[derive(Debug, Clone, Copy)]
enum Cover {
    Black,
    White,
    /// A fine 3 px checkerboard: detail like a logo or text, which leaves cells unsure.
    Checker,
}

/// Paints `cover` over a centred rectangle of `share` of the image area, with the image's
/// aspect ratio.
fn cover_centre(png: &[u8], share: f64, cover: Cover) -> Vec<u8> {
    let mut img = image::load_from_memory(png)
        .expect("load failed")
        .to_luma8();
    let (w, h) = (img.width(), img.height());
    let (cw, ch) = (
        (w as f64 * share.sqrt()) as u32,
        (h as f64 * share.sqrt()) as u32,
    );
    let (x0, y0) = ((w - cw) / 2, (h - ch) / 2);
    for y in y0..y0 + ch {
        for x in x0..x0 + cw {
            let luma = match cover {
                Cover::Black => 0,
                Cover::White => 255,
                Cover::Checker if (x / 3 + y / 3) % 2 == 0 => 0,
                Cover::Checker => 255,
            };
            img.put_pixel(x, y, image::Luma([luma]));
        }
    }
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(img.as_raw(), w, h, ExtendedColorType::L8)
        .expect("encode failed");
    out
}

#[test]
fn every_level_decodes_and_is_detected() {
    let mut previous_cells = 0;
    for level in EccLevel::ALL {
        let generated = generate_with_level(ImageStyle::Tiles, 150, level);
        let cells = generated.grid_rows * generated.grid_columns;
        assert!(cells > previous_cells, "{level:?}: grid didn't grow");
        previous_cells = cells;
        let decoded = decode_image(&generated.png_bytes).expect("decode errored");
        assert_eq!(
            decoded.text.as_deref(),
            Some(payload(150).as_str()),
            "{level:?}"
        );
        assert_eq!(decoded.ecc_level, Some(level), "{level:?}: wrong level");
        assert_eq!(
            decoded.corrected_bytes, 0,
            "{level:?}: clean code needed correction"
        );
    }
}

/// Share of the image each level must survive being covered at its centre. Measured limits
/// (len 40 to 400 at 16:9) are a little higher; a fine pattern costs less than a solid one
/// because its cells read as unsure and become erasures.
const SURVIVABLE_COVER: [(EccLevel, f64, f64); 4] = [
    (EccLevel::Low, 0.02, 0.05),
    (EccLevel::Medium, 0.06, 0.14),
    (EccLevel::Quartile, 0.15, 0.30),
    (EccLevel::High, 0.18, 0.40),
];

#[test]
fn survives_a_covered_centre() {
    for (level, solid, detailed) in SURVIVABLE_COVER {
        for style in [ImageStyle::Tiles, ImageStyle::Horizon] {
            let generated = generate_with_level(style, 150, level);
            for (cover, share) in [
                (Cover::Black, solid),
                (Cover::White, solid),
                (Cover::Checker, detailed),
            ] {
                let label = format!("{style:?} {level:?} {cover:?} {share}");
                let png = cover_centre(&generated.png_bytes, share, cover);
                let decoded = decode_image(&png).expect("decode errored");
                assert_eq!(
                    decoded.text.as_deref(),
                    Some(payload(150).as_str()),
                    "{label}: {:?}",
                    decoded.error
                );
                assert_eq!(decoded.ecc_level, Some(level), "{label}: wrong level");
                assert!(decoded.corrected_bytes > 0, "{label}: nothing was damaged");
                if let Cover::Checker = cover {
                    assert!(decoded.erased_bytes > 0, "{label}: no erasures");
                }
            }
        }
    }
}

/// A PNG overlay `w`×`h` px: a dark disc with a light fine pattern inside, transparent around
/// it, like a logo.
fn logo_png(w: u32, h: u32) -> Vec<u8> {
    let (cx, cy, r) = (w as f64 / 2.0, h as f64 / 2.0, w.min(h) as f64 / 2.0);
    let img = RgbaImage::from_fn(w, h, |x, y| {
        let d = (x as f64 + 0.5 - cx).hypot(y as f64 + 0.5 - cy);
        match d {
            d if d > r => Rgba([0, 0, 0, 0]),
            d if d < r * 0.6 && (x / 4 + y / 4) % 2 == 0 => Rgba([240, 240, 240, 255]),
            _ => Rgba([20, 20, 60, 255]),
        }
    });
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(img.as_raw(), w, h, ExtendedColorType::Rgba8)
        .expect("encode failed");
    out
}

fn generate_with_overlay(
    style: ImageStyle,
    level: EccLevel,
    overlay: Overlay<'_>,
) -> Result<GenerateDebug, String> {
    generate_image_debug(GenerateImageParams {
        width_ratio: 16,
        height_ratio: 9,
        style,
        ecc_level: level,
        overlay: Some(overlay),
        ..params(&payload(150))
    })
}

#[test]
fn decodes_with_the_largest_overlay_of_each_level() {
    // A square logo and a wide one, as a line of text renders.
    let (square, wide) = (logo_png(200, 200), logo_png(600, 120));
    for level in EccLevel::ALL {
        for style in [ImageStyle::Tiles, ImageStyle::Horizon] {
            for png in [&square, &wide] {
                let overlay = Overlay {
                    png,
                    share: level.max_overlay_share(),
                };
                let generated =
                    generate_with_overlay(style, level, overlay).expect("generation failed");
                assert_eq!(generated.decode_warning, None, "{style:?} {level:?}");
            }
        }
    }
}

#[test]
fn overlay_changes_the_centre_only() {
    let plain = generate_with_level(ImageStyle::Tiles, 150, EccLevel::High);
    let png = logo_png(100, 100);
    let overlay = Overlay {
        png: &png,
        share: 0.1,
    };
    let covered =
        generate_with_overlay(ImageStyle::Tiles, EccLevel::High, overlay).expect("failed");
    let (a, b) = (
        image::load_from_memory(&plain.png_bytes)
            .expect("load")
            .to_rgba8(),
        image::load_from_memory(&covered.png_bytes)
            .expect("load")
            .to_rgba8(),
    );
    assert_eq!(a.dimensions(), b.dimensions());
    let (w, h) = a.dimensions();
    // The corners, where the anchors are, stay untouched; the centre changes.
    for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
        for (dx, dy) in (0..60).flat_map(|i| (0..60).map(move |j| (i, j))) {
            let (px, py) = (x.abs_diff(dx), y.abs_diff(dy));
            assert_eq!(a.get_pixel(px, py), b.get_pixel(px, py));
        }
    }
    assert_ne!(a.get_pixel(w / 2, h / 2), b.get_pixel(w / 2, h / 2));
}

#[test]
fn refuses_bad_overlays() {
    let png = logo_png(50, 50);
    for share in [0.0, -0.1, EccLevel::Medium.max_overlay_share() + 0.01] {
        let overlay = Overlay { png: &png, share };
        let error = generate_with_overlay(ImageStyle::Tiles, EccLevel::Medium, overlay)
            .expect_err("accepted a bad share");
        assert!(error.contains("logo must take"), "{share}: {error}");
    }
    let overlay = Overlay {
        png: b"not a png",
        share: 0.05,
    };
    let error = generate_with_overlay(ImageStyle::Tiles, EccLevel::Medium, overlay)
        .expect_err("accepted garbage");
    assert!(error.contains("logo image"), "{error}");
}

#[test]
fn fails_when_damage_exceeds_the_level() {
    let generated = generate_with_level(ImageStyle::Tiles, 40, EccLevel::Low);
    let png = cover_centre(&generated.png_bytes, 0.2, Cover::Black);
    let decoded = decode_image(&png).expect("decode errored");
    assert_eq!(decoded.text, None);
    assert_eq!(decoded.decode_method, "all failed");
}

#[test]
fn survives_damage_and_a_flip_together() {
    let generated = generate_with_level(ImageStyle::Stain, 150, EccLevel::High);
    let damaged = cover_centre(&generated.png_bytes, 0.15, Cover::Black);
    let decoded = decode_image(&flip(&damaged, true, false)).expect("decode errored");
    assert_eq!(decoded.text.as_deref(), Some(payload(150).as_str()));
    assert_eq!(decoded.decode_method, "lattice-fit (flipped horizontally)");
}

#[test]
fn lattice_fit_survives_non_integer_scaling() {
    for factor in [0.8, 1.37] {
        for style in STYLES {
            let len = 150;
            let generated = generate(style, (16, 9), len);
            let label = format!("{:?} ×{}", style, factor);
            assert_decodes_exactly(
                &label,
                len,
                grid_of(&generated),
                &rescale(&generated.png_bytes, factor),
            );
        }
    }
}

#[test]
fn decodes_at_the_triangle_width_limits() {
    for triangle_width in [MIN_TRIANGLE_WIDTH, 75, MAX_TRIANGLE_WIDTH] {
        for style in STYLES {
            let len = 40;
            let generated = generate_at(style, (16, 9), len, triangle_width);
            let label = format!("{:?} {} px", style, triangle_width);
            assert_decodes_exactly(&label, len, grid_of(&generated), &generated.png_bytes);
        }
    }
}

#[test]
fn borders_scale_with_the_triangle_width() {
    // Same grid at two widths: the larger image must be the smaller one enlarged, so the share
    // of foreground pixels stays about the same instead of growing as fixed borders thin out.
    for style in STYLES {
        let share = |triangle_width| {
            let generated = generate_at(style, (1, 1), 40, triangle_width);
            let gray = image::load_from_memory(&generated.png_bytes)
                .expect("load failed")
                .to_luma8();
            // Ink weighted by darkness, so anti-aliased edge pixels count partly.
            gray.pixels()
                .map(|p| 1.0 - p[0] as f64 / 255.0)
                .sum::<f64>()
                / gray.len() as f64
        };
        let (small, large) = (share(MIN_TRIANGLE_WIDTH), share(MAX_TRIANGLE_WIDTH));
        assert!(
            (small - large).abs() < 0.03,
            "{:?}: foreground share {:.3} at {} px vs {:.3} at {} px",
            style,
            small,
            MIN_TRIANGLE_WIDTH,
            large,
            MAX_TRIANGLE_WIDTH
        );
    }
}

#[test]
fn rejects_invalid_triangle_widths() {
    for triangle_width in [MIN_TRIANGLE_WIDTH - 2, 38, MAX_TRIANGLE_WIDTH + 2] {
        let result = generate_image_debug(GenerateImageParams {
            triangle_width,
            ..params(&payload(5))
        });
        assert!(result.is_err(), "{} px was accepted", triangle_width);
    }
}

#[test]
fn pixel_limit_names_the_largest_width_that_fits() {
    // 400 bytes at 16:9 is a 48×143 grid: about 70 MP at the largest width.
    let grid = grid_of(&generate(ImageStyle::Tiles, (16, 9), 400));
    let (rows, cols) = (grid.0 as u32, grid.1 as u32);
    let error = check_image_pixels(cols, rows, MAX_TRIANGLE_WIDTH).expect_err("fits");
    let fits: u32 = error
        .rsplit("fits is ")
        .next()
        .and_then(|rest| rest.trim_end_matches(" px").parse().ok())
        .unwrap_or_else(|| panic!("no width in {error:?}"));
    assert!(fits % 2 == 1 && fits < MAX_TRIANGLE_WIDTH, "{error}");
    assert!(check_image_pixels(cols, rows, fits).is_ok());
    assert!(check_image_pixels(cols, rows, fits + 2).is_err());

    // Long enough to overflow even at the smallest width.
    let error = generate_image_debug(GenerateImageParams {
        triangle_width: MIN_TRIANGLE_WIDTH,
        ..params(&payload(40_000))
    })
    .expect_err("a 40 kB payload fits");
    assert!(error.contains("shorten the payload"), "{error}");
}

#[test]
fn edges_follow_straight_grid_lines() {
    // Stain has no borders, so every pixel takes the colour of the cell its centre lies in. The
    // cells are cut by straight lines through the grid's vertices: horizontal lines between
    // rows and slanted ones moving one column pitch per row. Away from those lines, the
    // rendered image must match them exactly, also across row boundaries.
    let triangle_width = MIN_TRIANGLE_WIDTH;
    let generated = generate_at(ImageStyle::Stain, (16, 9), 150, triangle_width);
    let img = image::load_from_memory(&generated.png_bytes)
        .expect("load failed")
        .to_luma8();
    let (rows, cols) = grid_of(&generated);
    let pitch = (triangle_width / 2) as f64;
    let height = triangle_height(triangle_width) as f64;
    let (origin_x, origin_y) = (pitch, (height / 2.0).floor() - 0.5);
    let is_black = |x: f64, y: f64| img.get_pixel(x as u32, y as u32)[0] < 128;

    let mut checked = 0;
    for y in 0..img.height() {
        let v = (y as f64 - origin_y) / height;
        let row = v.floor();
        let t = v - row;
        if row < 0.0 || row >= rows as f64 || t * height < 0.5 || (1.0 - t) * height < 0.5 {
            continue;
        }
        for x in 0..img.width() {
            let u = (x as f64 - origin_x) / pitch;
            // Horizontal distance to the nearest slanted line, in pixels.
            let off_line = |w: f64| (w - w.round()).abs() * pitch;
            if off_line(u - t) < 0.5 || off_line(u + t) < 0.5 {
                continue;
            }
            // The cell whose span in this pixel row contains the point.
            let Some(col) = (0..cols).find(|&c| {
                let c = c as f64;
                let down = (row as usize + c as usize).is_multiple_of(2);
                let (from, to) = if down {
                    (c + t, c + 2.0 - t)
                } else {
                    (c + 1.0 - t, c + 1.0 + t)
                };
                (from..to).contains(&u)
            }) else {
                continue;
            };
            // The cell's colour, read at its centroid.
            let down = (row as usize + col).is_multiple_of(2);
            let cx = origin_x + (col as f64 + 1.0) * pitch;
            let cy = origin_y + (row + if down { 1.0 / 3.0 } else { 2.0 / 3.0 }) * height;
            assert_eq!(
                is_black(x as f64, y as f64),
                is_black(cx.round(), cy.round()),
                "pixel ({x}, {y}) differs from cell ({row}, {col})"
            );
            checked += 1;
        }
    }
    assert!(checked > 100_000, "only {checked} pixels checked");
}

#[test]
fn outlines_are_anti_aliased() {
    for style in STYLES {
        let generated = generate(style, (1, 1), 40);
        let gray = image::load_from_memory(&generated.png_bytes)
            .expect("load failed")
            .to_luma8();
        let pure = |x: u32, y: u32| matches!(gray.get_pixel(x, y)[0], 0 | 255);
        let mut blended = 0;
        for (x, y, p) in gray.enumerate_pixels() {
            if matches!(p[0], 0 | 255) {
                continue;
            }
            blended += 1;
            // Blended pixels sit on an outline, so they touch a fully painted or blank one.
            let touches_pure = (x.saturating_sub(1)..=(x + 1).min(gray.width() - 1)).any(|nx| {
                (y.saturating_sub(1)..=(y + 1).min(gray.height() - 1)).any(|ny| pure(nx, ny))
            });
            assert!(
                touches_pure,
                "{:?}: blended pixel ({x}, {y}) away from any edge",
                style
            );
        }
        let share = blended as f64 / gray.len() as f64;
        assert!(
            (0.005..0.2).contains(&share),
            "{:?}: {:.3} of pixels blended",
            style,
            share
        );
    }
}

#[test]
fn small_triangles_decode_with_gentler_erosion() {
    // Scaled from 51 px, triangles are ~18 and ~13 px wide: the default 2 px erosion destroys
    // or merges them, so decoding has to fall back to a smaller radius.
    for factor in [0.35, 0.25] {
        for style in STYLES {
            let len = 40;
            let generated = generate_at(style, (16, 9), len, 51);
            let label = format!("{:?} ×{}", style, factor);
            let decoded =
                decode_image(&rescale(&generated.png_bytes, factor)).expect("decode errored");
            assert_eq!(
                (decoded.row_count, decoded.column_count),
                grid_of(&generated),
                "{label}: wrong grid"
            );
            assert_eq!(
                decoded.text.as_deref(),
                Some(payload(len).as_str()),
                "{label}: wrong text"
            );
            assert!(
                decoded.decode_method.starts_with("lattice-fit (erosion "),
                "{label}: expected a gentler-erosion lattice fit, got {}",
                decoded.decode_method
            );
        }
    }
}

#[test]
fn every_forced_mask_is_used_and_decodes() {
    assert_eq!(MASK_NAMES.len(), MASKS.len());
    let len = 40;
    for (index, name) in MASK_NAMES.iter().enumerate() {
        let generated = generate_image_debug(GenerateImageParams {
            width_ratio: 16,
            height_ratio: 9,
            mask: Some(index),
            ..params(&payload(len))
        })
        .expect("generation failed");
        assert_eq!(generated.mask_index, index, "{name}");
        assert_decodes_exactly(name, len, grid_of(&generated), &generated.png_bytes);
    }
}

#[test]
fn auto_mask_has_the_most_colour_transitions() {
    let with_mask = |mask| GenerateImageParams {
        width_ratio: 21,
        height_ratio: 13,
        mask,
        ..params("Lorem ipsum dolor sit amet, consectetur adipiscing elit.")
    };
    let auto = generate_image_debug(with_mask(None)).expect("generation failed");
    for (index, name) in MASK_NAMES.iter().enumerate() {
        let forced = generate_image_debug(with_mask(Some(index))).expect("generation failed");
        assert_eq!(forced.shared_edges, auto.shared_edges);
        assert!(
            auto.colour_transitions >= forced.colour_transitions,
            "{} beats the automatic choice",
            name
        );
    }
}
