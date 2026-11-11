use crate::logging::FilterHandle;
use crate::signals::RustLogFilter;
use async_trait::async_trait;
use messages::prelude::{Actor, Address, Context, Notifiable};
use rinf::DartSignal;
use tokio::task::JoinSet;
use tracing::{info, warn};
use tracing_subscriber::filter::Targets;

// `tokio_with_wasm` is tokio itself on native targets and a JavaScript-backed shim on the web.
use tokio_with_wasm::alias as tokio;

/// Applies the log filter Dart asks for.
pub struct LogFilterActor {
    /// Swaps the filter of the subscriber installed by `logging::init`.
    handle: FilterHandle,
    /// Owned tasks that are canceled when the actor is dropped.
    _owned_tasks: JoinSet<()>,
}

impl Actor for LogFilterActor {}

impl LogFilterActor {
    /// Creates the actor and initializes its fields.
    pub fn new(self_addr: Address<Self>, handle: FilterHandle) -> Self {
        let mut _owned_tasks = JoinSet::new();
        _owned_tasks.spawn(Self::listen_to_dart(self_addr.clone()));
        LogFilterActor {
            handle,
            _owned_tasks,
        }
    }
}

#[async_trait]
impl Notifiable<RustLogFilter> for LogFilterActor {
    async fn notify(&mut self, msg: RustLogFilter, _: &Context<Self>) {
        let filter = match msg.directives.parse::<Targets>() {
            Ok(filter) => filter,
            Err(error) => {
                warn!(directives = %msg.directives, %error, "invalid log filter");
                return;
            }
        };
        match self.handle.reload(filter) {
            Ok(()) => info!(directives = %msg.directives, "log filter set"),
            Err(error) => warn!(%error, "could not set log filter"),
        }
    }
}

impl LogFilterActor {
    /// Listen to an external source, which in this case is Dart.
    async fn listen_to_dart(mut self_addr: Address<Self>) {
        let receiver = RustLogFilter::get_dart_signal_receiver();
        while let Some(signal_pack) = receiver.recv().await {
            let _ = self_addr.notify(signal_pack.message).await;
        }
    }
}
