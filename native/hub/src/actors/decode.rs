use crate::signals::{DecodeResult, ImageBytes};
use async_trait::async_trait;
use messages::prelude::{Actor, Address, Context, Notifiable};
use rinf::{DartSignal, RustSignal};
use tokio::task::JoinSet;
use tracing::{debug, error, info};

// Uncomment below to target the web.
// use tokio_with_wasm::alias as tokio;

/// The decode actor.
pub struct DecodeActor {
    /// Owned tasks that are canceled when the actor is dropped.
    _owned_tasks: JoinSet<()>,
}

impl Actor for DecodeActor {}

impl DecodeActor {
    /// Creates the actor and initializes its fields.
    pub fn new(self_addr: Address<Self>) -> Self {
        let mut _owned_tasks = JoinSet::new();
        _owned_tasks.spawn(Self::listen_to_dart(self_addr.clone()));
        DecodeActor { _owned_tasks }
    }
}

#[async_trait]
impl Notifiable<ImageBytes> for DecodeActor {
    async fn notify(&mut self, msg: ImageBytes, _: &Context<Self>) {
        // Decoding takes up to seconds, which would stall the single-threaded runtime.
        let signal = match tokio::task::spawn_blocking(move || decode(&msg.bytes)).await {
            Ok(signal) => signal,
            Err(e) => {
                error!(error = %e, "decode task failed");
                failure(format!("Decoding failed: {e}"))
            }
        };
        signal.send_signal_to_dart();
    }
}

/// Decodes the image, as the signal to send back.
fn decode(bytes: &[u8]) -> DecodeResult {
    let report = match tricode::decode_image(bytes) {
        Ok(report) => report,
        Err(e) => {
            info!(error = %e, "image not decodable");
            return failure(e);
        }
    };
    match &report.text {
        Some(text) => debug!(
            chars = text.chars().count(),
            method = %report.decode_method,
            corrected = report.corrected_bytes,
            erased = report.erased_bytes,
            "decoded"
        ),
        None => info!(error = ?report.error, "no code found"),
    }
    DecodeResult {
        success: report.text.is_some(),
        text: report.text.unwrap_or_default(),
        error: report.error.unwrap_or_default(),
        content_type: report.content_type,
        image_width: report.image_width,
        image_height: report.image_height,
        total_triangles: report.total_triangles as u32,
        row_count: report.row_count as u32,
        column_count: report.column_count as u32,
        bit_count: report.bit_count as u32,
        annotated_image: report.annotated_png,
        decode_method: report.decode_method,
        ecc_level: report
            .ecc_level
            .map(|level| level.name().to_string())
            .unwrap_or_default(),
        corrected_bytes: report.corrected_bytes as u32,
        erased_bytes: report.erased_bytes as u32,
    }
}

/// The signal for an image that couldn't be read at all.
fn failure(error: String) -> DecodeResult {
    DecodeResult {
        success: false,
        text: String::new(),
        error,
        content_type: String::new(),
        image_width: 0,
        image_height: 0,
        total_triangles: 0,
        row_count: 0,
        column_count: 0,
        bit_count: 0,
        annotated_image: Vec::new(),
        decode_method: String::new(),
        ecc_level: String::new(),
        corrected_bytes: 0,
        erased_bytes: 0,
    }
}

impl DecodeActor {
    /// Listen to an external source, which in this case is Dart.
    async fn listen_to_dart(mut self_addr: Address<Self>) {
        let receiver = ImageBytes::get_dart_signal_receiver();
        while let Some(signal_pack) = receiver.recv().await {
            let _ = self_addr.notify(signal_pack.message).await;
        }
    }
}
