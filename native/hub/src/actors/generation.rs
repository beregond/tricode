use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::signals::{BigBool, GeneratedBytes, GeneratedImage, SmallNumber, SmallText};
use async_trait::async_trait;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageBuffer, ImageEncoder, Rgba};
use messages::prelude::{Actor, Address, Context, Handler, Notifiable};
use rand::Rng;
use rand_chacha::ChaCha8Rng;
use rand::SeedableRng;
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
        SmallNumber { number: 7 }.send_signal_to_dart();
        //return;
        let generated_bytes = tricode::generate_code(&msg.text);
        GeneratedBytes {
            bytes: generated_bytes,
        }
        .send_signal_to_dart();

        let png_bytes = Self::generate_image_bytes(&msg.text);
        if !png_bytes.is_empty() {
            GeneratedImage {
                png_bytes,
            }
            .send_signal_to_dart();
        }
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

    /// Generates a pseudo-random PNG image seeded by the payload text.
    fn generate_image_bytes(payload: &str) -> Vec<u8> {
        let mut hasher = DefaultHasher::new();
        payload.hash(&mut hasher);
        let seed = hasher.finish();

        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let (width, height) = (128u32, 128u32);
        let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(width, height);

        for pixel in img.pixels_mut() {
            let r: u8 = rng.gen_range(0..=255);
            let g: u8 = rng.gen_range(0..=255);
            let b: u8 = rng.gen_range(0..=255);
            *pixel = Rgba([r, g, b, 255]);
        }

        let mut png_bytes = Vec::new();
        let mut encoder = PngEncoder::new(&mut png_bytes);
        if let Err(e) = encoder.write_image(
            img.as_raw(),
            width,
            height,
            ExtendedColorType::Rgba8,
        ) {
            debug_print!("Failed to encode PNG: {e}");
            return Vec::new();
        }

        png_bytes
    }
}
