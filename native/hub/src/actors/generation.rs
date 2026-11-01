use crate::signals::{GeneratedImage, SmallText};
use async_trait::async_trait;
use messages::prelude::{Actor, Address, Context, Notifiable};
use rinf::{DartSignal, RustSignal, debug_print};
use tokio::task::JoinSet;

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
        debug_print!("Received text: {}", msg.text);

        let style = tricode::ImageStyle::from_u32(msg.style);
        let result = tricode::generate_image_debug(tricode::GenerateImageParams {
            payload: &msg.text,
            payload_type: &msg.payload_type,
            width_ratio: msg.width_ratio,
            height_ratio: msg.height_ratio,
            style,
            fg_color: msg.fg_color,
            bg_color: msg.bg_color,
            horizon_seed: msg.horizon_seed,
            mask: msg.mask.map(|m| m as usize),
            triangle_width: msg.triangle_width,
            ecc_level: tricode::EccLevel::from_u32(msg.ecc_level),
            overlay: (!msg.overlay_png.is_empty()).then_some(tricode::Overlay {
                png: &msg.overlay_png,
                share: msg.overlay_share,
            }),
        });
        let signal = match result {
            Ok(debug) => GeneratedImage {
                png_bytes: debug.png_bytes,
                error: String::new(),
                encoded_bytes: debug.encoded_bytes as u32,
                grid_rows: debug.grid_rows,
                grid_columns: debug.grid_columns,
                data_bits_used: debug.data_bits_used as u32,
                padding_bits: debug.padding_bits as u32,
                mask_index: debug.mask_index as u32,
                mask_name: tricode::MASK_NAMES
                    .get(debug.mask_index)
                    .copied()
                    .unwrap_or_default()
                    .to_string(),
                colour_transitions: debug.colour_transitions as u32,
                shared_edges: debug.shared_edges as u32,
                decode_warning: debug.decode_warning.unwrap_or_default(),
            },
            Err(error) => GeneratedImage {
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
            },
        };
        signal.send_signal_to_dart();
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
