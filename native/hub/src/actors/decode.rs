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
            Ok(text) => DecodeResult { success: true, text, error: String::new() },
            Err(e) => {
                debug_print!("Error decoding image: {}", e);
                DecodeResult { success: false, text: String::new(), error: e }
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
