use crate::signals::{DecodeResult, ImageBytes};
use async_trait::async_trait;
use messages::prelude::{Actor, Address, Context, Notifiable};
use rinf::{DartSignal, RustSignal, debug_print};
use tokio::task::JoinSet;

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
        debug_print!("Received {} bytes to decode", msg.bytes.len());
        
        // Decode the image using the tricode crate
        let signal = match tricode::decode_image(&msg.bytes) {
            Ok(debug) => DecodeResult {
                success: debug.text.is_some(),
                text: debug.text.unwrap_or_default(),
                error: debug.error.unwrap_or_default(),
                content_type: debug.content_type,
                image_width: debug.image_width,
                image_height: debug.image_height,
                total_triangles: debug.total_triangles as u32,
                row_count: debug.row_count as u32,
                column_count: debug.column_count as u32,
                bit_count: debug.bit_count as u32,
                annotated_image: debug.annotated_png,
                decode_method: debug.decode_method,
            },
            Err(e) => {
                debug_print!("Error decoding image: {}", e);
                DecodeResult {
                    success: false,
                    text: String::new(),
                    error: e,
                    content_type: String::new(),
                    image_width: 0,
                    image_height: 0,
                    total_triangles: 0,
                    row_count: 0,
                    column_count: 0,
                    bit_count: 0,
                    annotated_image: Vec::new(),
                    decode_method: String::new(),
                }
            }
        };
        
        signal.send_signal_to_dart();
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
