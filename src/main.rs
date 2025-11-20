mod bind_tracker;
mod config;
mod metrics;
mod progress;
mod worker;

use std::{path::PathBuf, sync::Arc, sync::atomic::AtomicU64};

use anyhow::Result;
use clap::Parser;
use tokio_util::sync::CancellationToken;

use crate::{
    bind_tracker::BindTracker, config::Config, metrics::Metrics, progress::spawn_progress_task,
    worker::spawn_bind,
};

#[derive(Parser, Debug)]
#[command(author, version, about = "SMPP load generator", long_about = None)]
struct Cli {
    /// Path to the TOML configuration file
    #[arg(short, long, default_value = "config.toml")]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,rusmppc=warn,rusmpp=warn")
        .init();

    let cli = Cli::parse();
    let config = Arc::new(Config::from_file(&cli.config)?);

    if config.load.binds == 0 {
        tracing::warn!("Configuration requested 0 binds. No traffic will be generated.");
    }

    let metrics = Arc::new(Metrics::new(config.load.binds));
    let tracker = Arc::new(BindTracker::new(config.load.binds));
    let shutdown = CancellationToken::new();
    let messages_sent = Arc::new(AtomicU64::new(0));
    let messages_limit = config.load.messages_count;

    let progress_handle = spawn_progress_task(
        metrics.clone(),
        tracker.clone(),
        Arc::new(config.smpp.clone()),
        Arc::new(config.message.clone()),
        shutdown.clone(),
    );

    let mut tasks = Vec::new();
    for idx in 0..config.load.binds {
        let task = tokio::spawn(spawn_bind(
            idx,
            config.clone(),
            metrics.clone(),
            tracker.clone(),
            shutdown.clone(),
            messages_sent.clone(),
            messages_limit,
        ));
        tasks.push(task);
    }

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            println!("\nCtrl+C received. Stopping load test...");
            shutdown.cancel();
        }
        _ = shutdown.cancelled() => {}
    }

    for task in tasks {
        let _ = task.await;
    }

    shutdown.cancel();
    let _ = progress_handle.await;

    println!("Load test stopped.");
    Ok(())
}
