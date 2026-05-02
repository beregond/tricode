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
        let result = tricode::generate_image_debug(&msg.text, &msg.payload_type, msg.width_ratio, msg.height_ratio, style, msg.fg_color, msg.bg_color);
        if let Some(debug) = result {
            if !debug.png_bytes.is_empty() {
                GeneratedImage {
                    png_bytes: debug.png_bytes,
                    encoded_bytes: debug.encoded_bytes as u32,
                    grid_rows: debug.grid_rows,
                    grid_columns: debug.grid_columns,
                    data_bits_used: debug.data_bits_used as u32,
                    padding_bits: debug.padding_bits as u32,
                }
                .send_signal_to_dart();
            }
        }
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
