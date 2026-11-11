//! This module contains actors.
//! To build a solid app, avoid communicating by sharing memory.
//! Focus on message passing instead.

mod decode;
mod generation;
mod log_filter;

use crate::logging::FilterHandle;
use decode::DecodeActor;
use generation::GenerationActor;
use log_filter::LogFilterActor;
use messages::prelude::Context;
use tokio::spawn;

// `tokio_with_wasm` is tokio itself on native targets and a JavaScript-backed shim on the web.
use tokio_with_wasm::alias as tokio;

/// Creates and spawns the actors in the async system.
pub async fn create_actors(log_filter: Option<FilterHandle>) {
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

    if let Some(handle) = log_filter {
        let log_filter_context = Context::new();
        let log_filter_actor = LogFilterActor::new(log_filter_context.address(), handle);
        spawn(log_filter_context.run(log_filter_actor));
    }
}
