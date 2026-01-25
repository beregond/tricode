use std::time::Duration;

use crate::signals::{BigBool, GeneratedBytes, SmallNumber, SmallText};
use async_trait::async_trait;
use messages::prelude::{Actor, Address, Context, Handler, Notifiable};
use rinf::{DartSignal, RustSignal, debug_print};
use tokio::task::JoinSet;
use tokio::time::interval;

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
        SmallNumber { number: 7 }.send_signal_to_dart();
        //return;
        let generated_bytes = tricode::generate_code(&msg.text);
        GeneratedBytes {
            bytes: generated_bytes,
        }
        .send_signal_to_dart();
    }
}

// Implementing the `Handler` trait
// allows an actor's loop to respond to a specific message type.
#[async_trait]
impl Handler<BigBool> for GenerationActor {
    type Result = bool;
    async fn handle(&mut self, msg: BigBool, _: &Context<Self>) -> bool {
        msg.send_signal_to_dart();
        false
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
