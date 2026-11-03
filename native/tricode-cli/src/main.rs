//! `tricode`: generate and read tricodes from the terminal.
//!
//! Decoded text goes to stdout, everything else (warnings, errors, logs) to stderr, so the output
//! can be piped. Logs follow `RUST_LOG` (default `warn`), e.g. `RUST_LOG=tricode=trace`.

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "tricode", version, about = "Generate and read tricodes")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Write TEXT as a tricode PNG.
    Encode {
        text: String,
        /// Output PNG file.
        #[arg(short, long)]
        output: PathBuf,
        #[arg(long, value_enum, default_value_t = Style::Tiles)]
        style: Style,
        #[arg(long, value_enum, default_value_t = Ecc::Medium)]
        ecc: Ecc,
        /// Aspect ratio of the code, as WIDTH:HEIGHT.
        #[arg(long, default_value = "21:13", value_parser = parse_ratio)]
        ratio: (u32, u32),
        /// Triangle base width in pixels (odd).
        #[arg(long, default_value_t = tricode::DEFAULT_TRIANGLE_WIDTH)]
        triangle_width: u32,
    },
    /// Read a tricode from a PNG and print its text.
    Decode { input: PathBuf },
}

#[derive(Clone, Copy, ValueEnum)]
enum Style {
    Tiles,
    Stain,
    Horizon,
}

impl From<Style> for tricode::ImageStyle {
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

impl From<Ecc> for tricode::EccLevel {
    fn from(ecc: Ecc) -> Self {
        match ecc {
            Ecc::Low => Self::Low,
            Ecc::Medium => Self::Medium,
            Ecc::Quartile => Self::Quartile,
            Ecc::High => Self::High,
        }
    }
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

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    match run(Cli::parse().command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("tricode: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: Command) -> Result<(), String> {
    match command {
        Command::Encode {
            text,
            output,
            style,
            ecc,
            ratio: (width_ratio, height_ratio),
            triangle_width,
        } => {
            let debug = tricode::generate_image_debug(tricode::GenerateImageParams {
                payload: &text,
                payload_type: "text",
                width_ratio,
                height_ratio,
                style: style.into(),
                fg_color: 0xFF00_0000,
                bg_color: 0xFFFF_FFFF,
                horizon_seed: 0,
                mask: None,
                triangle_width,
                ecc_level: ecc.into(),
                overlay: None,
            })?;
            if let Some(warning) = &debug.decode_warning {
                eprintln!("tricode: warning: {warning}");
            }
            fs::write(&output, &debug.png_bytes)
                .map_err(|e| format!("can't write {}: {e}", output.display()))
        }
        Command::Decode { input } => {
            let png =
                fs::read(&input).map_err(|e| format!("can't read {}: {e}", input.display()))?;
            let debug = tricode::decode_image(&png)?;
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
    }
}
