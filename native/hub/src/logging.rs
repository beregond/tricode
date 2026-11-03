//! Sends `tracing` events from hub and tricode to Dart as `RustLog` signals, so Rust and Dart
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
        .with_target("tricode", Level::DEBUG)
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
        RustLog {
            level: metadata.level().to_string(),
            target: metadata.target().to_string(),
            message,
        }
        .send_signal_to_dart();
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
