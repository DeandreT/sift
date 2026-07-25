//! Tracing setup: a standard stderr layer plus a ring buffer feeding the
//! in-app log panel.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use time::OffsetDateTime;
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::prelude::*;

const CAPACITY: usize = 10_000;

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub time: OffsetDateTime,
    pub level: tracing::Level,
    pub target: String,
    pub message: String,
}

/// Bounded, shared buffer of recent log entries.
#[derive(Debug, Clone, Default)]
pub struct LogBuffer {
    entries: Arc<Mutex<VecDeque<LogEntry>>>,
}

impl LogBuffer {
    fn push(&self, entry: LogEntry) {
        let mut entries = self.lock();
        if entries.len() == CAPACITY {
            entries.pop_front();
        }
        entries.push_back(entry);
    }

    /// Visit all entries under the lock, oldest first.
    pub fn for_each(&self, mut f: impl FnMut(&LogEntry)) {
        for entry in self.lock().iter() {
            f(entry);
        }
    }

    pub fn clear(&self) {
        self.lock().clear();
    }

    pub fn len(&self) -> usize {
        self.lock().len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<LogEntry>> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

struct BufferLayer(LogBuffer);

impl<S: tracing::Subscriber> Layer<S> for BufferLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        struct MessageVisitor(String);
        impl tracing::field::Visit for MessageVisitor {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    let _ = write!(self.0, "{value:?}");
                } else {
                    if !self.0.is_empty() {
                        self.0.push(' ');
                    }
                    let _ = write!(self.0, "{}={value:?}", field.name());
                }
            }
        }

        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        let metadata = event.metadata();
        self.0.push(LogEntry {
            time: OffsetDateTime::now_utc(),
            level: *metadata.level(),
            target: metadata.target().to_owned(),
            message: visitor.0,
        });
    }
}

/// Install the global subscriber and return the buffer the UI reads.
pub fn init() -> LogBuffer {
    let buffer = LogBuffer::default();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(
            "info,sift=debug,sift_backend=debug,sift_mgmt=debug,sift_core=debug",
        )
    });
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_target(true))
        .with(BufferLayer(buffer.clone()))
        .init();
    buffer
}
