use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use rusmpp::values::MessageState;

#[derive(Debug)]
pub struct Metrics {
    total_attempts: AtomicU64,
    total_success: AtomicU64,
    total_error: AtomicU64,
    total_latency_micros: AtomicU64,
    per_bind: Vec<BindMetrics>,
}

impl Metrics {
    pub fn new(bind_count: usize) -> Self {
        let per_bind = (0..bind_count).map(|_| BindMetrics::default()).collect();

        Self {
            total_attempts: AtomicU64::new(0),
            total_success: AtomicU64::new(0),
            total_error: AtomicU64::new(0),
            total_latency_micros: AtomicU64::new(0),
            per_bind,
        }
    }

    pub fn record_success(&self, bind_idx: usize, latency: Duration) {
        self.total_attempts.fetch_add(1, Ordering::Relaxed);
        self.total_success.fetch_add(1, Ordering::Relaxed);
        self.add_latency(latency);

        if let Some(bind) = self.per_bind.get(bind_idx) {
            bind.record_success(latency);
        }
    }

    pub fn record_error(&self, bind_idx: usize, latency: Duration) {
        self.total_attempts.fetch_add(1, Ordering::Relaxed);
        self.total_error.fetch_add(1, Ordering::Relaxed);
        self.add_latency(latency);

        if let Some(bind) = self.per_bind.get(bind_idx) {
            bind.record_error(latency);
        }
    }

    pub fn record_dlr(&self, bind_idx: usize, delay: Duration) {
        if let Some(bind) = self.per_bind.get(bind_idx) {
            bind.record_dlr(delay);
        }
    }

    pub fn record_dlr_status(&self, bind_idx: usize, delivered: bool, failed: bool) {
        if let Some(bind) = self.per_bind.get(bind_idx) {
            bind.record_dlr_status(delivered, failed);
        }
    }

    pub fn record_dlr_state(&self, bind_idx: usize, state: MessageState) {
        if let Some(bind) = self.per_bind.get(bind_idx) {
            bind.record_dlr_state(state);
        }
    }

    fn add_latency(&self, latency: Duration) {
        let micros = latency.as_micros();
        let capped = u64::try_from(micros).unwrap_or(u64::MAX);
        self.total_latency_micros
            .fetch_add(capped, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        let attempts = self.total_attempts.load(Ordering::Relaxed);
        let ok = self.total_success.load(Ordering::Relaxed);
        let err = self.total_error.load(Ordering::Relaxed);
        let latency = self.total_latency_micros.load(Ordering::Relaxed);
        let avg_latency_ms = if attempts == 0 {
            0.0
        } else {
            (latency as f64 / attempts as f64) / 1000.0
        };

        let bind_snapshots = self.per_bind.iter().map(BindMetrics::snapshot).collect();

        MetricsSnapshot {
            attempts,
            ok,
            err,
            avg_latency_ms,
            per_bind: bind_snapshots,
        }
    }
}

#[derive(Default, Debug)]
struct BindMetrics {
    attempts: AtomicU64,
    success: AtomicU64,
    error: AtomicU64,
    latency_micros: AtomicU64,
    dlr_received: AtomicU64,
    dlr_latency_micros: AtomicU64,
    dlr_delivered: AtomicU64,
    dlr_failed: AtomicU64,
    dlr_unknown: AtomicU64,
    dlr_enroute: AtomicU64,
    dlr_expired: AtomicU64,
    dlr_deleted: AtomicU64,
    dlr_accepted: AtomicU64,
}

impl BindMetrics {
    fn record_success(&self, latency: Duration) {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        self.success.fetch_add(1, Ordering::Relaxed);
        self.add_latency(latency);
    }

    fn record_error(&self, latency: Duration) {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        self.error.fetch_add(1, Ordering::Relaxed);
        self.add_latency(latency);
    }

    fn add_latency(&self, latency: Duration) {
        let micros = latency.as_micros();
        let capped = u64::try_from(micros).unwrap_or(u64::MAX);
        self.latency_micros.fetch_add(capped, Ordering::Relaxed);
    }

    fn record_dlr(&self, delay: Duration) {
        self.dlr_received.fetch_add(1, Ordering::Relaxed);
        let micros = delay.as_micros();
        let capped = u64::try_from(micros).unwrap_or(u64::MAX);
        self.dlr_latency_micros.fetch_add(capped, Ordering::Relaxed);
    }

    fn record_dlr_status(&self, delivered: bool, failed: bool) {
        if delivered {
            self.dlr_delivered.fetch_add(1, Ordering::Relaxed);
        } else if failed {
            self.dlr_failed.fetch_add(1, Ordering::Relaxed);
        } else {
            self.dlr_unknown.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn record_dlr_state(&self, state: MessageState) {
        match state {
            MessageState::Enroute => { self.dlr_enroute.fetch_add(1, Ordering::Relaxed); }
            MessageState::Delivered => { self.dlr_delivered.fetch_add(1, Ordering::Relaxed); }
            MessageState::Expired => { self.dlr_expired.fetch_add(1, Ordering::Relaxed); }
            MessageState::Deleted => { self.dlr_deleted.fetch_add(1, Ordering::Relaxed); }
            MessageState::Undeliverable => { self.dlr_failed.fetch_add(1, Ordering::Relaxed); }
            MessageState::Accepted => { self.dlr_accepted.fetch_add(1, Ordering::Relaxed); }
            MessageState::Unknown => { self.dlr_unknown.fetch_add(1, Ordering::Relaxed); }
            MessageState::Rejected => { self.dlr_failed.fetch_add(1, Ordering::Relaxed); }
            MessageState::Scheduled | MessageState::Skipped | MessageState::Other(_) => {
                self.dlr_unknown.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn snapshot(&self) -> BindSnapshot {
        let attempts = self.attempts.load(Ordering::Relaxed);
        let ok = self.success.load(Ordering::Relaxed);
        let err = self.error.load(Ordering::Relaxed);
        let latency = self.latency_micros.load(Ordering::Relaxed);
        let dlr = self.dlr_received.load(Ordering::Relaxed);
        let dlr_latency = self.dlr_latency_micros.load(Ordering::Relaxed);
        let avg_latency_ms = if attempts == 0 {
            0.0
        } else {
            (latency as f64 / attempts as f64) / 1000.0
        };
        let avg_dlr_delay_ms = if dlr == 0 {
            0.0
        } else {
            (dlr_latency as f64 / dlr as f64) / 1000.0
        };

        BindSnapshot {
            attempts,
            ok,
            err,
            avg_latency_ms,
            dlr_received: dlr,
            avg_dlr_delay_ms,
            dlr_delivered: self.dlr_delivered.load(Ordering::Relaxed),
            dlr_failed: self.dlr_failed.load(Ordering::Relaxed),
            dlr_unknown: self.dlr_unknown.load(Ordering::Relaxed),
            dlr_enroute: self.dlr_enroute.load(Ordering::Relaxed),
            dlr_expired: self.dlr_expired.load(Ordering::Relaxed),
            dlr_deleted: self.dlr_deleted.load(Ordering::Relaxed),
            dlr_accepted: self.dlr_accepted.load(Ordering::Relaxed),
        }
    }
}

pub struct MetricsSnapshot {
    pub attempts: u64,
    pub ok: u64,
    pub err: u64,
    pub avg_latency_ms: f64,
    pub per_bind: Vec<BindSnapshot>,
}

#[derive(Default, Clone, Copy)]
pub struct BindSnapshot {
    pub attempts: u64,
    pub ok: u64,
    pub err: u64,
    pub avg_latency_ms: f64,
    pub dlr_received: u64,
    pub avg_dlr_delay_ms: f64,
    pub dlr_delivered: u64,
    pub dlr_failed: u64,
    pub dlr_unknown: u64,
    pub dlr_enroute: u64,
    pub dlr_expired: u64,
    pub dlr_deleted: u64,
    pub dlr_accepted: u64,
}
