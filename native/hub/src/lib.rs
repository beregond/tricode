//! This `hub` crate is the
//! entry point of the Rust logic.

mod actors;
mod logging;
mod signals;
mod web_threads;

use actors::create_actors;
use rinf::{dart_shutdown, write_interface};
use tokio::spawn;

// `tokio_with_wasm` is tokio itself on native targets and a JavaScript-backed shim on the web.
use tokio_with_wasm::alias as tokio;

write_interface!();

// You can go with any async library, not just `tokio`.
#[tokio::main(flavor = "current_thread")]
async fn main() {
    // Spawn concurrent tasks.
    // Always use non-blocking async functions like `tokio::fs::File::open`.
    // If you must use blocking code, use `tokio::task::spawn_blocking`
    // or the equivalent provided by your async library.
    let log_filter = logging::init();
    spawn(logging::forward_from_workers());
    #[cfg(target_family = "wasm")]
    web_threads::start_rayon_pool();
    spawn(create_actors(log_filter));

    // Keep the main function running until Dart shutdown.
    dart_shutdown().await;
}
