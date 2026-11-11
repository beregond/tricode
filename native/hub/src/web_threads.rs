//! A rayon thread pool for the web. Rayon starts its threads with `std::thread::spawn`, which
//! browsers don't support, so its global pool runs everything on the calling thread: a failed
//! decode of a large image then takes minutes instead of seconds. Here each pool thread is a web
//! worker, taken from `tokio_with_wasm`'s blocking pool for good, and the CPU-heavy work runs
//! inside the pool through `in_pool`.
//!
//! Not rayon's global pool: `build_global` blocks until every thread has started, and a browser's
//! main thread can neither block (`Atomics.wait` throws there) nor start a worker while blocked.

#[cfg(target_family = "wasm")]
use std::sync::OnceLock;
#[cfg(target_family = "wasm")]
use tokio_with_wasm::alias as tokio;
#[cfg(target_family = "wasm")]
use tracing::{info, warn};

/// Pool threads at most: each holds its own copy of the decoder's per-grid buffers in the
/// module's 1 GiB of memory.
#[cfg(target_family = "wasm")]
const MAX_THREADS: usize = 8;

#[cfg(target_family = "wasm")]
static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();

/// Starts the pool on web workers, one per logical core up to `MAX_THREADS`. Must run on the
/// browser's main thread, the only one allowed to call `spawn_blocking`. Returns at once; the
/// workers start once the main thread is idle.
#[cfg(target_family = "wasm")]
pub fn start_rayon_pool() {
    let cores = web_sys::window()
        .map(|window| window.navigator().hardware_concurrency() as usize)
        .unwrap_or(1);
    let threads = cores.clamp(1, MAX_THREADS);
    let built = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .spawn_handler(|thread| {
            // Never returns, so the worker stays out of the blocking pool for good.
            drop(tokio::task::spawn_blocking(move || thread.run()));
            Ok(())
        })
        .build();
    match built {
        Ok(pool) => {
            let _ = POOL.set(pool);
            info!(threads, "rayon pool started on web workers");
        }
        Err(e) => warn!(error = %e, "rayon pool not started; decoding runs on one thread"),
    }
}

/// Runs `op` in the pool, so rayon's parallel iterators inside it use every pool thread. Call it
/// from a `spawn_blocking` worker, never the main thread: it blocks until `op` is done. Other
/// platforms just run `op`: rayon's global pool works there.
pub fn in_pool<R: Send>(op: impl FnOnce() -> R + Send) -> R {
    #[cfg(target_family = "wasm")]
    if let Some(pool) = POOL.get() {
        return pool.install(op);
    }
    op()
}
