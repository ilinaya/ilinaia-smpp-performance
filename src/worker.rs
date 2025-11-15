use std::{str::FromStr, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use futures::{FutureExt, StreamExt, future::BoxFuture, stream::FuturesUnordered};
use rusmpp::{
    CommandId,
    pdus::{BindTransceiver, SubmitSm},
    types::{COctetString, OctetString},
    values::{EsmClass, RegisteredDelivery, ServiceType},
};
use rusmppc::{ConnectionBuilder, Event, error::Error as ClientError};
use tokio::time::{self, Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use crate::{
    bind_tracker::{BindState, BindTracker},
    config::{Config, MessageConfig},
    metrics::Metrics,
};

pub async fn spawn_bind(
    idx: usize,
    config: Arc<Config>,
    metrics: Arc<Metrics>,
    tracker: Arc<BindTracker>,
    shutdown: CancellationToken,
) {
    tracker.set_state(idx, BindState::Connecting).await;

    let result = run_bind(idx, config, metrics, tracker.clone(), shutdown.clone()).await;

    if let Err(err) = result {
        tracing::error!(bind = idx, error = ?err, "Bind task failed");
        tracker
            .set_state(idx, BindState::Error(err.to_string()))
            .await;
    }
}

async fn run_bind(
    idx: usize,
    config: Arc<Config>,
    metrics: Arc<Metrics>,
    tracker: Arc<BindTracker>,
    shutdown: CancellationToken,
) -> Result<()> {
    let (client, mut events) = ConnectionBuilder::new()
        .enquire_link_interval(Duration::from_secs(5))
        .response_timeout(Duration::from_secs(5))
        .connect(&config.smpp.connection_uri())
        .await
        .context("failed to connect to SMPP server")?;

    client
        .bind_transceiver(build_bind_pdu(&config).context("failed to build bind request")?)
        .await
        .context("failed to bind")?;

    tracker.set_state(idx, BindState::Bound).await;

    let submit_template = build_submit_sm(&config.message)?;
    let client_for_events = client.clone();
    let event_shutdown = shutdown.clone();

    tokio::spawn(async move {
        while let Some(event) = events.next().await {
            if event_shutdown.is_cancelled() {
                break;
            }

            match event {
                Event::Incoming(command) => {
                    tracing::debug!(bind = idx, ?command, "Incoming command");
                    // Automatically ACK DeliverSm to keep the server happy.
                    if command.id() == CommandId::DeliverSm {
                        let _ = client_for_events
                            .deliver_sm_resp(
                                command.sequence_number(),
                                rusmpp::pdus::DeliverSmResp::default(),
                            )
                            .await;
                    }
                }
                Event::Error(err) => {
                    tracing::warn!(bind = idx, ?err, "Background error");
                }
            }
        }
    });

    drive_submit_loop(
        idx,
        client.clone(),
        submit_template,
        metrics,
        tracker.clone(),
        &config,
        shutdown,
    )
    .await?;

    client.unbind().await.ok();
    client.close().await.ok();
    Ok(())
}

fn build_bind_pdu(config: &Config) -> Result<BindTransceiver> {
    Ok(BindTransceiver::builder()
        .system_id(COctetString::from_str(&config.smpp.system_id)?)
        .password(COctetString::from_str(&config.smpp.password)?)
        .system_type(if let Some(system_type) = &config.smpp.system_type {
            COctetString::from_str(system_type)?
        } else {
            COctetString::empty()
        })
        .addr_ton(config.message.source_ton())
        .addr_npi(config.message.source_npi())
        .address_range(COctetString::empty())
        .build())
}

fn build_submit_sm(message: &MessageConfig) -> Result<SubmitSm> {
    let service_type = if let Some(raw) = message.service_type.as_deref() {
        if raw.is_empty() {
            ServiceType::default()
        } else {
            ServiceType::new(COctetString::from_str(raw)?)
        }
    } else {
        ServiceType::default()
    };

    Ok(SubmitSm::builder()
        .service_type(service_type)
        .source_addr_ton(message.source_ton())
        .source_addr_npi(message.source_npi())
        .source_addr(COctetString::from_str(&message.source_addr)?)
        .dest_addr_ton(message.destination_ton())
        .dest_addr_npi(message.destination_npi())
        .destination_addr(COctetString::from_str(&message.destination_addr)?)
        .esm_class(EsmClass::default())
        .registered_delivery(RegisteredDelivery::default())
        .short_message(OctetString::from_str(&message.body)?)
        .build())
}

async fn drive_submit_loop(
    idx: usize,
    client: rusmppc::Client,
    submit_template: SubmitSm,
    metrics: Arc<Metrics>,
    tracker: Arc<BindTracker>,
    config: &Config,
    shutdown: CancellationToken,
) -> Result<()> {
    let max_tps = config.load.max_tps_per_bind();
    let max_inflight = config.load.inflight_per_bind().max(1);

    let inflight: FuturesUnordered<BoxFuture<'static, SubmissionOutcome>> = FuturesUnordered::new();

    if max_tps == 0 {
        drive_unthrottled_loop(
            idx,
            inflight,
            metrics,
            max_inflight,
            client,
            submit_template,
            tracker,
            shutdown,
        )
        .await
    } else {
        drive_throttled_loop(
            idx,
            inflight,
            metrics,
            max_inflight,
            client,
            submit_template,
            max_tps,
            tracker,
            shutdown,
        )
        .await
    }
}

async fn drive_unthrottled_loop(
    idx: usize,
    mut inflight: FuturesUnordered<BoxFuture<'static, SubmissionOutcome>>,
    metrics: Arc<Metrics>,
    max_inflight: usize,
    client: rusmppc::Client,
    submit_template: SubmitSm,
    tracker: Arc<BindTracker>,
    shutdown: CancellationToken,
) -> Result<()> {
    fill_inflight(
        &mut inflight,
        max_inflight,
        client.clone(),
        submit_template.clone(),
    );

    while !shutdown.is_cancelled() {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            Some(outcome) = inflight.next() => {
                handle_outcome(idx, outcome, &metrics, &tracker).await;
                queue_if_capacity(
                    &mut inflight,
                    max_inflight,
                    client.clone(),
                    submit_template.clone(),
                );
            }
        }
    }

    drain_inflight(idx, inflight, &metrics, &tracker).await;
    Ok(())
}

async fn drive_throttled_loop(
    idx: usize,
    mut inflight: FuturesUnordered<BoxFuture<'static, SubmissionOutcome>>,
    metrics: Arc<Metrics>,
    max_inflight: usize,
    client: rusmppc::Client,
    submit_template: SubmitSm,
    max_tps: u32,
    tracker: Arc<BindTracker>,
    shutdown: CancellationToken,
) -> Result<()> {
    const TICK_MS: u64 = 10;
    let ticks_per_sec = (1000 / TICK_MS) as u32;
    let mut allowance = 0u32;
    let mut remainder = 0u32;
    let mut ticker = time::interval(Duration::from_millis(TICK_MS));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    while !shutdown.is_cancelled() {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            Some(outcome) = inflight.next(), if !inflight.is_empty() => {
                handle_outcome(idx, outcome, &metrics, &tracker).await;
            }
            _ = ticker.tick() => {
                allowance += max_tps / ticks_per_sec;
                remainder += max_tps % ticks_per_sec;
                if remainder >= ticks_per_sec {
                    allowance += 1;
                    remainder -= ticks_per_sec;
                }

                while allowance > 0 && inflight.len() < max_inflight {
                    inflight.push(submit_once(client.clone(), submit_template.clone()));
                    allowance -= 1;
                }
            }
        }
    }

    drain_inflight(idx, inflight, &metrics, &tracker).await;
    Ok(())
}

fn fill_inflight(
    inflight: &mut FuturesUnordered<BoxFuture<'static, SubmissionOutcome>>,
    max_inflight: usize,
    client: rusmppc::Client,
    submit_template: SubmitSm,
) {
    while inflight.len() < max_inflight {
        inflight.push(submit_once(client.clone(), submit_template.clone()));
    }
}

fn queue_if_capacity(
    inflight: &mut FuturesUnordered<BoxFuture<'static, SubmissionOutcome>>,
    max_inflight: usize,
    client: rusmppc::Client,
    submit_template: SubmitSm,
) {
    if inflight.len() < max_inflight {
        inflight.push(submit_once(client, submit_template));
    }
}

type SubmissionOutcome = (Result<rusmpp::pdus::SubmitSmResp, ClientError>, Duration);

fn submit_once(client: rusmppc::Client, submit: SubmitSm) -> BoxFuture<'static, SubmissionOutcome> {
    async move {
        let start = Instant::now();
        let result = client.submit_sm(submit).await;
        (result, start.elapsed())
    }
    .boxed()
}

async fn handle_outcome(
    idx: usize,
    outcome: SubmissionOutcome,
    metrics: &Arc<Metrics>,
    tracker: &Arc<BindTracker>,
) {
    match outcome {
        (Ok(resp), latency) => {
            tracing::debug!(bind = idx, ?resp, "SubmitSmResp");
            metrics.record_success(idx, latency);
            let message_id = resp.message_id().as_str().to_string();
            tracker.set_last_message_id(idx, Some(message_id)).await;
        }
        (Err(err), latency) => {
            tracing::warn!(bind = idx, ?err, "SubmitSm failed");
            metrics.record_error(idx, latency);
        }
    }
}

async fn drain_inflight(
    idx: usize,
    mut inflight: FuturesUnordered<BoxFuture<'static, SubmissionOutcome>>,
    metrics: &Arc<Metrics>,
    tracker: &Arc<BindTracker>,
) {
    while let Some(outcome) = inflight.next().await {
        handle_outcome(idx, outcome, metrics, tracker).await;
    }
}
