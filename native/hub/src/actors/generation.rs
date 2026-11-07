use crate::signals::{CodeStyle, EccLevel, GeneratedImage, PayloadType, SmallText};
use async_trait::async_trait;
use messages::prelude::{Actor, Address, Context, Notifiable};
use rinf::{DartSignal, RustSignal};
use tokio::task::JoinSet;
use tracing::{debug, error, warn};

// Uncomment below to target the web.
// use tokio_with_wasm::alias as tokio;

/// The generation actor.
pub struct GenerationActor {
    /// Owned tasks that are canceled when the actor is dropped.
    _owned_tasks: JoinSet<()>,
}

// Implementing the `Actor` trait for `GenerationActor`.
// This defines `GenerationActor` as an actor in the async system.
impl Actor for GenerationActor {}

impl GenerationActor {
    /// Creates the actor and initializes its fields.
    pub fn new(self_addr: Address<Self>) -> Self {
        let mut _owned_tasks = JoinSet::new();
        _owned_tasks.spawn(Self::listen_to_dart(self_addr.clone()));
        GenerationActor { _owned_tasks }
    }
}

// Implementing the `Notifiable` trait
// allows an actor's loop to listen for a specific message type.
#[async_trait]
impl Notifiable<SmallText> for GenerationActor {
    async fn notify(&mut self, msg: SmallText, _: &Context<Self>) {
        // Rendering and reading the code back take up to seconds, which would stall the
        // single-threaded runtime.
        let signal = match tokio::task::spawn_blocking(move || generate(&msg)).await {
            Ok(signal) => signal,
            Err(e) => {
                error!(error = %e, "generation task failed");
                failure(format!("Generation failed: {e}"))
            }
        };
        signal.send_signal_to_dart();
    }
}

impl From<CodeStyle> for tricode::ImageStyle {
    fn from(style: CodeStyle) -> Self {
        match style {
            CodeStyle::Tiles => Self::Tiles,
            CodeStyle::Stain => Self::Stain,
            CodeStyle::Horizon => Self::Horizon,
        }
    }
}

impl From<EccLevel> for tricode::EccLevel {
    fn from(level: EccLevel) -> Self {
        match level {
            EccLevel::Low => Self::Low,
            EccLevel::Medium => Self::Medium,
            EccLevel::Quartile => Self::Quartile,
            EccLevel::High => Self::High,
        }
    }
}

impl From<PayloadType> for &'static str {
    /// The envelope's `type`, as `tricode::GenerateImageParams::payload_type` takes it.
    fn from(kind: PayloadType) -> Self {
        match kind {
            PayloadType::Text => "text",
            PayloadType::Url => "url",
            PayloadType::Plain => "plain",
        }
    }
}

/// Generates the code `msg` asks for, as the signal to send back.
fn generate(msg: &SmallText) -> GeneratedImage {
    let result = tricode::generate_image_debug(tricode::GenerateImageParams {
        payload: &msg.text,
        payload_type: msg.payload_type.into(),
        width_ratio: msg.width_ratio,
        height_ratio: msg.height_ratio,
        style: msg.style.into(),
        primary_color: msg.primary_color,
        secondary_color: msg.secondary_color,
        bg_color: msg.bg_color,
        seed: msg.seed,
        primary_variance: msg.primary_variance,
        secondary_variance: msg.secondary_variance,
        mask: msg.mask.map(|m| m as usize),
        triangle_width: msg.triangle_width,
        ecc_level: msg.ecc_level.into(),
        overlay: (!msg.overlay_png.is_empty()).then_some(tricode::Overlay {
            png: &msg.overlay_png,
            share: msg.overlay_share,
        }),
    });
    let report = match result {
        Ok(report) => report,
        Err(error) => return failure(error),
    };
    debug!(
        // The payload is user content: log its size, never the text.
        chars = msg.text.chars().count(),
        rows = report.grid_rows,
        cols = report.grid_columns,
        "generated"
    );
    if let Some(warning) = &report.decode_warning {
        warn!(%warning, "generated code may not read back");
    }
    GeneratedImage {
        png_bytes: report.png_bytes,
        error: String::new(),
        encoded_bytes: report.encoded_bytes as u32,
        grid_rows: report.grid_rows,
        grid_columns: report.grid_columns,
        data_bits_used: report.data_bits_used as u32,
        padding_bits: report.padding_bits as u32,
        mask_index: report.mask_index as u32,
        mask_name: tricode::MASK_NAMES
            .get(report.mask_index)
            .copied()
            .unwrap_or_default()
            .to_string(),
        colour_transitions: report.colour_transitions as u32,
        shared_edges: report.shared_edges as u32,
        decode_warning: report.decode_warning.unwrap_or_default(),
    }
}

/// The signal for a generation that failed with `error`.
fn failure(error: String) -> GeneratedImage {
    GeneratedImage {
        png_bytes: Vec::new(),
        error,
        encoded_bytes: 0,
        grid_rows: 0,
        grid_columns: 0,
        data_bits_used: 0,
        padding_bits: 0,
        mask_index: 0,
        mask_name: String::new(),
        colour_transitions: 0,
        shared_edges: 0,
        decode_warning: String::new(),
    }
}

impl GenerationActor {
    /// Listen to an external source, which in this case is Dart.
    async fn listen_to_dart(mut self_addr: Address<Self>) {
        let receiver = SmallText::get_dart_signal_receiver();
        while let Some(signal_pack) = receiver.recv().await {
            let _ = self_addr.notify(signal_pack.message).await;
        }
    }
}
