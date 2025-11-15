use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

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

    fn snapshot(&self) -> BindSnapshot {
        let attempts = self.attempts.load(Ordering::Relaxed);
        let ok = self.success.load(Ordering::Relaxed);
        let err = self.error.load(Ordering::Relaxed);
        let latency = self.latency_micros.load(Ordering::Relaxed);
        let avg_latency_ms = if attempts == 0 {
            0.0
        } else {
            (latency as f64 / attempts as f64) / 1000.0
        };

        BindSnapshot {
            attempts,
            ok,
            err,
            avg_latency_ms,
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
}
