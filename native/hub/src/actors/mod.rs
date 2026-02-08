//! This module contains actors.
//! To build a solid app, avoid communicating by sharing memory.
//! Focus on message passing instead.

mod generation;
mod decode;

use generation::GenerationActor;
use decode::DecodeActor;
use messages::prelude::Context;
use tokio::spawn;

// Uncomment below to target the web.
// use tokio_with_wasm::alias as tokio;

/// Creates and spawns the actors in the async system.
pub async fn create_actors() {
    // Though simple async tasks work, using the actor model
    // is highly recommended for state management
    // to achieve modularity and scalability in your app.
    // Actors keep ownership of their state and run in their own loops,
    // handling messages from other actors or external sources,
    // such as websockets or timers.

    // Create actor contexts.
    let generation_context = Context::new();
    let generation_addr = generation_context.address();

    let decode_context = Context::new();
    let decode_addr = decode_context.address();

    // Spawn the actors.
    let generation_actor = GenerationActor::new(generation_addr.clone());
    spawn(generation_context.run(generation_actor));

    let decode_actor = DecodeActor::new(decode_addr.clone());
    spawn(decode_context.run(decode_actor));
}
