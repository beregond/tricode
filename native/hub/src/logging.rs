//! Sends `tracing` events from hub and tricode-core to Dart as `RustLog` signals, so Rust and Dart
//! logs end up in one place on every platform (stderr goes nowhere on mobile and web).

use crate::signals::RustLog;
use rinf::RustSignal;
use std::fmt::{Debug, Write};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Subscriber, error};
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{Layer, Registry, reload};

/// Swaps the active filter when Dart sends a `RustLogFilter`.
pub type FilterHandle = reload::Handle<Targets, Registry>;

/// The filter until Dart sends its own: our crates at debug, dependencies at info. Release builds
/// drop debug events at compile time anyway (`release_max_level_info`).
fn default_filter() -> Targets {
    Targets::new()
        .with_default(Level::INFO)
        .with_target("hub", Level::DEBUG)
        .with_target("tricode_core", Level::DEBUG)
}

/// Installs the Dart-forwarding subscriber and routes panics through it. Returns `None` when a
/// subscriber was already set.
pub fn init() -> Option<FilterHandle> {
    let (filter, handle) = reload::Layer::new(default_filter());
    let installed = tracing_subscriber::registry()
        .with(filter)
        .with(DartLayer)
        .try_init()
        .is_ok();
    // Replaces rinf's hook, which only reports panics in debug builds.
    std::panic::set_hook(Box::new(|info| error!(target: "panic", "{info}")));
    installed.then_some(handle)
}

/// Formats each event with its spans and fields and sends it to Dart.
struct DartLayer;

/// A span's fields, formatted once when the span is created.
struct SpanFields(String);

impl<S> Layer<S> for DartLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let mut fields = FieldWriter::default();
        attrs.record(&mut fields);
        span.extensions_mut().insert(SpanFields(fields.fields));
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let mut fields = FieldWriter::default();
        values.record(&mut fields);
        if let Some(SpanFields(existing)) = span.extensions_mut().get_mut::<SpanFields>() {
            existing.push_str(&fields.fields);
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        // Spans first, e.g. "decode_image{bytes=1234}: decode_with_erosion{erosion=2}: ".
        let mut message = String::new();
        if let Some(scope) = ctx.event_scope(event) {
            for span in scope.from_root() {
                message.push_str(span.name());
                if let Some(SpanFields(fields)) = span.extensions().get::<SpanFields>()
                    && !fields.is_empty()
                {
                    let _ = write!(message, "{{{}}}", fields.trim_start());
                }
                message.push_str(": ");
            }
        }
        let mut writer = FieldWriter::default();
        event.record(&mut writer);
        message.push_str(&writer.message);
        message.push_str(&writer.fields);

        let metadata = event.metadata();
        deliver(RustLog {
            level: metadata.level().to_string(),
            target: metadata.target().to_string(),
            message,
        });
    }
}

/// Queue of events for the main thread to send, set by `forward_from_workers`. On the web, rinf
/// can only send signals from the browser's main thread: from a web worker (where
/// `spawn_blocking` runs decoding and generation) it silently drops them, panics included.
#[cfg(target_family = "wasm")]
static WEB_LOG_QUEUE: std::sync::OnceLock<tokio::sync::mpsc::UnboundedSender<RustLog>> =
    std::sync::OnceLock::new();

/// Sends `log` to Dart, through `WEB_LOG_QUEUE` on the web.
fn deliver(log: RustLog) {
    #[cfg(target_family = "wasm")]
    let log = match WEB_LOG_QUEUE.get() {
        Some(queue) => match queue.send(log) {
            Ok(()) => return,
            Err(unsent) => unsent.0,
        },
        None => log,
    };
    log.send_signal_to_dart();
}

/// On the web, starts queueing events and returns the task, to run on the main thread, that
/// sends them to Dart. Does nothing on other platforms, where any thread can send signals.
/// Not an `async fn`: the queue must be in place before the first worker starts, not when the
/// task is first polled.
#[cfg_attr(not(target_family = "wasm"), allow(clippy::manual_async_fn))]
pub fn forward_from_workers() -> impl Future<Output = ()> {
    #[cfg(target_family = "wasm")]
    let queue = {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let _ = WEB_LOG_QUEUE.set(sender);
        receiver
    };
    async move {
        #[cfg(target_family = "wasm")]
        {
            let mut queue = queue;
            while let Some(log) = queue.recv().await {
                log.send_signal_to_dart();
            }
        }
    }
}

/// Collects the `message` field and the others as ` key=value` pairs.
#[derive(Default)]
struct FieldWriter {
    message: String,
    fields: String,
}

impl Visit for FieldWriter {
    fn record_f64(&mut self, field: &Field, value: f64) {
        let _ = write!(self.fields, " {}={:.3}", field.name(), value);
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            let _ = write!(self.fields, " {}={}", field.name(), value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let _ = write!(self.fields, " {}={:?}", field.name(), value);
        }
    }
}
