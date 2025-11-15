use std::{
    io::Write,
    sync::Arc,
    time::{Duration, Instant},
};

use owo_colors::OwoColorize;
use tokio::{task::JoinHandle, time};
use tokio_util::sync::CancellationToken;

use crate::{
    bind_tracker::{BindState, BindStatus, BindTracker},
    config::{MessageConfig, SmppConfig},
    metrics::{BindSnapshot, Metrics},
};

pub fn spawn_progress_task(
    metrics: Arc<Metrics>,
    tracker: Arc<BindTracker>,
    smpp: Arc<SmppConfig>,
    message: Arc<MessageConfig>,
    shutdown: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut throughput = ThroughputTracker::new();
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    render(&metrics, &tracker, &smpp, &message, &mut throughput).await;
                    break;
                }
                _ = time::sleep(Duration::from_millis(500)) => {
                    render(&metrics, &tracker, &smpp, &message, &mut throughput).await;
                }
            }
        }
    })
}

struct Tracker {
    last_attempts: u64,
    last_instant: Instant,
}

impl Tracker {
    fn new() -> Self {
        Self {
            last_attempts: 0,
            last_instant: Instant::now(),
        }
    }

    fn compute_tps(&mut self, attempts: u64) -> f64 {
        let now = Instant::now();
        let elapsed = now
            .saturating_duration_since(self.last_instant)
            .as_secs_f64();
        let delta = attempts.saturating_sub(self.last_attempts);

        self.last_attempts = attempts;
        self.last_instant = now;

        if elapsed > 0.0 {
            delta as f64 / elapsed
        } else {
            0.0
        }
    }
}

struct ThroughputTracker {
    total: Tracker,
    per_bind: Vec<Tracker>,
}

impl ThroughputTracker {
    fn new() -> Self {
        Self {
            total: Tracker::new(),
            per_bind: Vec::new(),
        }
    }

    fn total_tps(&mut self, attempts: u64) -> f64 {
        self.total.compute_tps(attempts)
    }

    fn bind_tps(&mut self, idx: usize, attempts: u64) -> f64 {
        if idx >= self.per_bind.len() {
            self.per_bind.resize_with(idx + 1, Tracker::new);
        }
        self.per_bind[idx].compute_tps(attempts)
    }
}

async fn render(
    metrics: &Metrics,
    tracker: &BindTracker,
    smpp: &SmppConfig,
    message: &MessageConfig,
    throughput: &mut ThroughputTracker,
) {
    let snapshot = metrics.snapshot();
    let statuses = tracker.snapshot().await;
    let total_tps = throughput.total_tps(snapshot.attempts);

    let success_pct = if snapshot.attempts == 0 {
        0.0
    } else {
        (snapshot.ok as f64 / snapshot.attempts as f64) * 100.0
    };

    let error_pct = if snapshot.attempts == 0 {
        0.0
    } else {
        (snapshot.err as f64 / snapshot.attempts as f64) * 100.0
    };

    let mut stdout = std::io::stdout();
    let _ = write!(stdout, "\x1B[2J\x1B[H"); // Clear screen + move cursor home.

    writeln!(stdout, "{}", "SMPP Load Test Dashboard".bold()).ok();
    writeln!(
        stdout,
        "{}",
        "© 2025 ilinaia.com — Alexey Ilinskiy | Special thanks to Jad K. Haddad (Rusmpp)".italic()
    )
    .ok();
    writeln!(stdout, "{}", "-".repeat(80)).ok();

    let bind_bar: String = statuses
        .iter()
        .enumerate()
        .map(|(idx, status)| format_state(idx, &status.state))
        .collect::<Vec<_>>()
        .join(" ");

    writeln!(stdout, "Bind states: {bind_bar}").ok();
    writeln!(
        stdout,
        "Target: {}:{} | system_id={} | password={} | system_type={}",
        smpp.host,
        smpp.port,
        smpp.system_id,
        smpp.password,
        smpp.system_type
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("-")
    )
    .ok();
    writeln!(
        stdout,
        "Source: {} (TON {} / NPI {}) | Destination: {} (TON {} / NPI {})",
        message.source_addr,
        message.source_ton,
        message.source_npi,
        message.destination_addr,
        message.destination_ton,
        message.destination_npi
    )
    .ok();
    writeln!(stdout, "").ok();

    writeln!(
        stdout,
        "Messages: {} | OK: {} ({success_pct:.1}%) | Err: {} ({error_pct:.1}%)",
        snapshot.attempts.to_string().bold(),
        snapshot.ok.green(),
        snapshot.err.red()
    )
    .ok();
    writeln!(
        stdout,
        "Average latency: {:.2} ms | Total TPS: {:.1}",
        snapshot.avg_latency_ms, total_tps
    )
    .ok();

    writeln!(stdout, "\nPer-bind stats:").ok();
    for (idx, status) in statuses.iter().enumerate() {
        let bind_snapshot = snapshot.per_bind.get(idx).copied().unwrap_or_default();
        let bind_tps = throughput.bind_tps(idx, bind_snapshot.attempts);
        render_bind_line(&mut stdout, idx, status, bind_snapshot, bind_tps).ok();
    }

    stdout.flush().ok();
}

fn render_bind_line(
    stdout: &mut std::io::Stdout,
    idx: usize,
    status: &BindStatus,
    snapshot: BindSnapshot,
    tps: f64,
) -> std::io::Result<()> {
    let last_id = status
        .last_message_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("-");
    writeln!(
        stdout,
        "{} -> TPS {:>8.1} | Avg {:>6.2} ms | OK {:>8} | Err {:>8} | Last ID {}",
        format_state(idx, &status.state),
        tps,
        snapshot.avg_latency_ms,
        snapshot.ok,
        snapshot.err,
        last_id
    )
}

fn format_state(idx: usize, state: &BindState) -> String {
    match state {
        BindState::Pending => format!("[{}]", format!("P{idx}").dimmed()),
        BindState::Connecting => format!("[{}]", format!("C{idx}").yellow()),
        BindState::Bound => format!("[{}]", format!("B{idx}").green()),
        BindState::Error(err) => {
            let trimmed = if err.len() > 24 {
                format!("{}…", &err[..24])
            } else {
                err.clone()
            };
            format!("[{}:{trimmed}]", format!("E{idx}").red())
        }
    }
}
