use std::sync::atomic::{AtomicU64, Ordering};

/// Lock-free pool metrics — all counters are AtomicU64.
#[derive(Default, Debug)]
pub struct PoolMetrics {
    pub active_count: AtomicU64,
    pub idle_count: AtomicU64,
    pub total_created: AtomicU64,
    pub total_destroyed: AtomicU64,
    pub total_checkouts: AtomicU64,
    pub total_timeouts: AtomicU64,
    pub total_deadlocks: AtomicU64,
    pub waiters: AtomicU64,
}

impl PoolMetrics {
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            active:          self.active_count.load(Ordering::Relaxed),
            idle:            self.idle_count.load(Ordering::Relaxed),
            total_created:   self.total_created.load(Ordering::Relaxed),
            total_destroyed: self.total_destroyed.load(Ordering::Relaxed),
            total_checkouts: self.total_checkouts.load(Ordering::Relaxed),
            total_timeouts:  self.total_timeouts.load(Ordering::Relaxed),
            total_deadlocks: self.total_deadlocks.load(Ordering::Relaxed),
            waiters:         self.waiters.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MetricsSnapshot {
    pub active: u64,
    pub idle: u64,
    pub total_created: u64,
    pub total_destroyed: u64,
    pub total_checkouts: u64,
    pub total_timeouts: u64,
    pub total_deadlocks: u64,
    pub waiters: u64,
}
