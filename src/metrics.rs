use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct Metrics {
    pub upstream_messages: Arc<Counter>,
    pub upstream_errors: Arc<Counter>,
    pub reconnect_attempts: Arc<Counter>,
}

#[derive(Default)]
pub struct Counter {
    value: AtomicU64,
}

impl Counter {
    pub fn increment(&self, delta: u64) {
        self.value.fetch_add(delta, Ordering::Relaxed);
    }

    pub fn get(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }
}
