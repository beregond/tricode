//! `tricode`: generate and read tricodes from the terminal.
//!
//! Decoded text goes to stdout, everything else (warnings, errors, debug info, logs) to stderr, so
//! the output can be piped. Logs follow `RUST_LOG` (default `warn`), e.g. `RUST_LOG=tricode_core=trace`.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::SystemTime;

use clap::{Parser, Subcommand, ValueEnum};
use image::{ImageFormat, ImageReader};
use tracing_subscriber::EnvFilter;

/// Longest side of an overlay image; larger ones are scaled down, as the app does.
const OVERLAY_MAX_SIDE: u32 = 1024;

#[derive(Parser)]
#[command(name = "tricode", version, about = "Generate and read tricodes")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Write TEXT as a tricode PNG.
    Encode(EncodeArgs),
    /// Read a tricode from an image (PNG, JPEG or WebP) and print its text.
    Decode {
        input: PathBuf,
        /// Print the content type and how the code was read to stderr.
        #[arg(short, long)]
        verbose: bool,
        /// Write the input annotated with the detected triangles and grid to this PNG file.
        #[arg(long, value_name = "FILE")]
        annotated: Option<PathBuf>,
    },
}

#[derive(clap::Args)]
struct EncodeArgs {
    text: String,
    /// Output PNG file.
    #[arg(short, long)]
    output: PathBuf,
    /// What TEXT holds, stored in the code; `plain` writes the bare text without an envelope.
    #[arg(long = "type", value_enum, default_value_t = PayloadType::Text)]
    payload_type: PayloadType,
    #[arg(long, value_enum, default_value_t = Style::Tiles)]
    style: Style,
    #[arg(long, value_enum, default_value_t = Ecc::Medium)]
    ecc: Ecc,
    /// Aspect ratio of the code, as WIDTH:HEIGHT.
    #[arg(long, default_value = "21:13", value_parser = parse_ratio)]
    ratio: (u32, u32),
    /// Triangle base width in pixels (odd).
    #[arg(long, default_value_t = tricode_core::DEFAULT_TRIANGLE_WIDTH)]
    triangle_width: u32,
    /// Colour of the primary triangles, as RRGGBB (always opaque).
    #[arg(long, default_value = "000000", value_parser = parse_color)]
    primary: u32,
    /// Colour of the secondary triangles, as RRGGBB, RRGGBBAA or `transparent` [default: the
    /// background colour].
    #[arg(long, value_parser = parse_color)]
    secondary: Option<u32>,
    /// Colour of the margin and the borders between shapes, as RRGGBB, RRGGBBAA or `transparent`.
    #[arg(long, default_value = "FFFFFF", value_parser = parse_color)]
    bg: u32,
    /// Shift each primary shape's brightness a little, picked by the seed.
    #[arg(long)]
    primary_variance: bool,
    /// Shift each secondary shape's brightness a little, picked by the seed.
    #[arg(long)]
    secondary_variance: bool,
    /// Seed of the Horizon grouping and of colour variance [default: the current time; printed
    /// with --verbose].
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    seed: Option<u64>,
    /// Mask the data with this mask instead of the most balanced one (for comparing masks).
    #[arg(long, value_enum)]
    mask: Option<Mask>,
    /// Image (PNG, JPEG or WebP) drawn over the centre of the code; the cells under it are left
    /// out and repaired by the error correction.
    #[arg(long, value_name = "FILE")]
    overlay: Option<PathBuf>,
    /// Size of the overlay, in percent of the largest the error correction level allows.
    #[arg(long, default_value_t = 100.0, value_parser = parse_percent, requires = "overlay")]
    overlay_size: f64,
    /// Print the grid size, mask and other generation details to stderr.
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum PayloadType {
    Text,
    Url,
    Plain,
}

impl PayloadType {
    fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Url => "url",
            Self::Plain => "plain",
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum Style {
    Tiles,
    Stain,
    Horizon,
}

impl From<Style> for tricode_core::ImageStyle {
    fn from(style: Style) -> Self {
        match style {
            Style::Tiles => Self::Tiles,
            Style::Stain => Self::Stain,
            Style::Horizon => Self::Horizon,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum Ecc {
    Low,
    Medium,
    Quartile,
    High,
}

impl From<Ecc> for tricode_core::EccLevel {
    fn from(ecc: Ecc) -> Self {
        match ecc {
            Ecc::Low => Self::Low,
            Ecc::Medium => Self::Medium,
            Ecc::Quartile => Self::Quartile,
            Ecc::High => Self::High,
        }
    }
}

/// The masks of `tricode_core::MASK_NAMES`, in the same order.
#[derive(Clone, Copy, ValueEnum)]
enum Mask {
    Checkerboard,
    Blocks,
    RightWave,
    LeftWave,
    VerticalLines,
    HorizontalLines,
    None,
}

fn parse_ratio(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s
        .split_once(':')
        .ok_or("expected WIDTH:HEIGHT, e.g. 21:13")?;
    let parse = |n: &str| match n.trim().parse::<u32>() {
        Ok(v) if v > 0 => Ok(v),
        _ => Err(format!("{n:?} is not a positive whole number")),
    };
    Ok((parse(w)?, parse(h)?))
}

/// Parses `RRGGBB`, `RRGGBBAA` (an optional leading `#`) or `transparent` into ARGB.
fn parse_color(s: &str) -> Result<u32, String> {
    if s.eq_ignore_ascii_case("transparent") {
        return Ok(0);
    }
    let hex = s.strip_prefix('#').unwrap_or(s);
    let value = match hex.len() {
        6 | 8 if hex.chars().all(|c| c.is_ascii_hexdigit()) => u32::from_str_radix(hex, 16),
        _ => return Err("expected RRGGBB, RRGGBBAA or transparent, e.g. 1A2B3C".to_string()),
    }
    .map_err(|e| e.to_string())?;
    Ok(if hex.len() == 6 {
        0xFF00_0000 | value
    } else {
        value.rotate_right(8)
    })
}

fn parse_percent(s: &str) -> Result<f64, String> {
    match s.trim_end_matches('%').parse::<f64>() {
        Ok(v) if v > 0.0 && v <= 100.0 => Ok(v),
        _ => Err("expected a percentage above 0 and at most 100".to_string()),
    }
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let result = match Cli::parse().command {
        Command::Encode(args) => encode(args),
        Command::Decode {
            input,
            verbose,
            annotated,
        } => decode(&input, verbose, annotated.as_deref()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("tricode: {e}");
            ExitCode::FAILURE
        }
    }
}

fn encode(args: EncodeArgs) -> Result<(), String> {
    let ecc_level = tricode_core::EccLevel::from(args.ecc);
    let seed = args.seed.unwrap_or_else(clock_seed);
    let overlay_png = args.overlay.as_deref().map(overlay_png).transpose()?;
    let overlay = overlay_png.as_deref().map(|png| tricode_core::Overlay {
        png,
        share: ecc_level.max_overlay_share() * args.overlay_size / 100.0,
    });
    let debug = tricode_core::generate_image_debug(tricode_core::GenerateImageParams {
        payload: &args.text,
        payload_type: args.payload_type.name(),
        width_ratio: args.ratio.0,
        height_ratio: args.ratio.1,
        style: args.style.into(),
        primary_color: args.primary,
        secondary_color: args.secondary.unwrap_or(args.bg),
        bg_color: args.bg,
        seed,
        primary_variance: args.primary_variance,
        secondary_variance: args.secondary_variance,
        mask: args.mask.map(|mask| mask as usize),
        triangle_width: args.triangle_width,
        ecc_level,
        overlay,
    })?;
    if args.verbose {
        let mask = tricode_core::MASK_NAMES
            .get(debug.mask_index)
            .copied()
            .unwrap_or("?");
        eprintln!(
            "grid:               {} × {}",
            debug.grid_rows, debug.grid_columns
        );
        eprintln!("encoded bytes:      {}", debug.encoded_bytes);
        eprintln!("data bits:          {}", debug.data_bits_used);
        eprintln!("padding bits:       {}", debug.padding_bits);
        eprintln!("mask:               {mask} ({})", debug.mask_index);
        eprintln!(
            "colour transitions: {} of {} shared edges",
            debug.colour_transitions, debug.shared_edges
        );
        eprintln!("seed:               {seed}");
    }
    if let Some(warning) = &debug.decode_warning {
        eprintln!("tricode: warning: {warning}");
    }
    fs::write(&args.output, &debug.png_bytes)
        .map_err(|e| format!("can't write {}: {e}", args.output.display()))
}

fn decode(input: &Path, verbose: bool, annotated: Option<&Path>) -> Result<(), String> {
    let bytes = fs::read(input).map_err(|e| format!("can't read {}: {e}", input.display()))?;
    let debug = tricode_core::decode_image(&as_png(&bytes, input)?)?;
    if verbose {
        eprintln!(
            "image:           {} × {} px",
            debug.image_width, debug.image_height
        );
        eprintln!("triangles:       {}", debug.total_triangles);
        eprintln!(
            "grid:            {} × {}",
            debug.row_count, debug.column_count
        );
        eprintln!("bits:            {}", debug.bit_count);
        eprintln!("method:          {}", debug.decode_method);
        if debug.text.is_some() {
            let level = debug.ecc_level.map_or("?", |level| level.name());
            eprintln!("content type:    {}", debug.content_type);
            eprintln!("ecc level:       {level}");
            eprintln!("corrected bytes: {}", debug.corrected_bytes);
            eprintln!("erased bytes:    {}", debug.erased_bytes);
        }
    }
    if let Some(path) = annotated {
        if debug.annotated_png.is_empty() {
            eprintln!("tricode: warning: no annotated image to write");
        } else {
            fs::write(path, &debug.annotated_png)
                .map_err(|e| format!("can't write {}: {e}", path.display()))?;
        }
    }
    match debug.text {
        Some(text) => {
            println!("{text}");
            Ok(())
        }
        None => Err(debug
            .error
            .unwrap_or_else(|| "no tricode found".to_string())),
    }
}

/// The current unix time in seconds, as the library picks a seed of 0; resolved here so
/// `--verbose` can print it.
fn clock_seed() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(42, |d| d.as_secs())
        .max(1)
}

/// `bytes` as PNG: returned unchanged if they already are, otherwise re-encoded, since the
/// decoder reads only PNG.
fn as_png(bytes: &[u8], path: &Path) -> Result<Vec<u8>, String> {
    if image::guess_format(bytes).ok() == Some(ImageFormat::Png) {
        return Ok(bytes.to_vec());
    }
    encode_png(&load_image(bytes, path)?)
}

/// Reads an overlay image, scaled down to `OVERLAY_MAX_SIDE` and re-encoded as PNG.
fn overlay_png(path: &Path) -> Result<Vec<u8>, String> {
    let bytes = fs::read(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
    let mut image = load_image(&bytes, path)?;
    if image.width().max(image.height()) > OVERLAY_MAX_SIDE {
        image = image.thumbnail(OVERLAY_MAX_SIDE, OVERLAY_MAX_SIDE);
    }
    encode_png(&image)
}

fn load_image(bytes: &[u8], path: &Path) -> Result<image::DynamicImage, String> {
    ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .decode()
        .map_err(|e| format!("can't read {} as an image: {e}", path.display()))
}

fn encode_png(image: &image::DynamicImage) -> Result<Vec<u8>, String> {
    let mut png = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|e| format!("can't encode PNG: {e}"))?;
    Ok(png)
}
