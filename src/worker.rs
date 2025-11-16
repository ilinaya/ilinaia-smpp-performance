use std::{str::FromStr, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use futures::{FutureExt, StreamExt, future::BoxFuture, stream::FuturesUnordered};
use rusmpp::{
    CommandId,
    pdus::{BindTransceiver, BindTransmitter, SubmitSm},
    types::{COctetString, OctetString},
    values::{DataCoding, EsmClass, RegisteredDelivery, ServiceType},
};
use rusmpp::Pdu;
use rusmpp::tlvs::TlvValue;
use rusmppc::{ConnectionBuilder, Event, error::Error as ClientError};
use tokio::time::{self, Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use crate::{
    bind_tracker::{BindState, BindTracker},
    config::{BindType, Config, MessageConfig},
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

    match config.smpp.bind_type {
        BindType::Trx => {
            client
                .bind_transceiver(
                    build_bind_trx_pdu(&config).context("failed to build TRX bind request")?,
                )
                .await
                .context("failed to bind as TRX")?;
        }
        BindType::Tx => {
            client
                .bind_transmitter(
                    build_bind_tx_pdu(&config).context("failed to build TX bind request")?,
                )
                .await
                .context("failed to bind as TX")?;
        }
    }

    tracker.set_state(idx, BindState::Bound).await;

    let submit_template = build_submit_sm(&config.message)?;
    let client_for_events = client.clone();
    let event_shutdown = shutdown.clone();

    // Track submit_sm send times by message_id so we can compute DLR delay
    let sent_index: Arc<dashmap::DashMap<String, Instant>> =
        Arc::new(dashmap::DashMap::with_capacity(1024));
    let sent_index_events = sent_index.clone();
    let metrics_for_events = metrics.clone();

    tokio::spawn(async move {
        while let Some(event) = events.next().await {
            if event_shutdown.is_cancelled() {
                break;
            }

            match event {
                Event::Incoming(command) => {
                    tracing::debug!(bind = idx, ?command, "Incoming command");
                    if command.id() == CommandId::DeliverSm {
                        let _ = client_for_events
                            .deliver_sm_resp(
                                command.sequence_number(),
                                rusmpp::pdus::DeliverSmResp::default(),
                            )
                            .await;

                        if let Some(pdu) = command.pdu() {
                            if let Pdu::DeliverSm(deliver) = pdu {
                                for tlv in deliver.tlvs().iter() {
                                    if let rusmpp::tlvs::TlvTag::ReceiptedMessageId = tlv.tag() {
                                        if let Some(val) = tlv.value() {
                                            if let TlvValue::ReceiptedMessageId(co) = val {
                                                let id = co.as_str().to_string();
                                                if let Some(start) = sent_index_events.remove(&id).map(|e| e.1) {
                                                    metrics_for_events.record_dlr(idx, start.elapsed());
                                                }
                                            }
                                        }
                                    }
                                }
                                // Record DLR outcome if MessageState TLV is present
                                for tlv in deliver.tlvs().iter() {
                                    if let rusmpp::tlvs::TlvTag::MessageState = tlv.tag() {
                                        if let Some(val) = tlv.value() {
                                            if let TlvValue::MessageState(ms) = val {
                                                use rusmpp::values::MessageState as MS;
                                                let delivered = matches!(ms, MS::Delivered);
                                                let failed = matches!(ms, MS::Undeliverable | MS::Rejected | MS::Expired | MS::Deleted);
                                                metrics_for_events.record_dlr_status(idx, delivered, failed);
                                                metrics_for_events.record_dlr_state(idx, *ms);
                                            }
                                        }
                                    }
                                }
                                // Fallback: parse id/stat from textual short_message if TLVs missing
                                let sm_text_res = deliver.short_message().to_str();
                                if let Ok(sm_text) = sm_text_res {
                                    if let Some((id, ms)) = parse_textual_dlr(sm_text) {
                                        if let Some(start) = sent_index_events.remove(&id).map(|e| e.1) {
                                            metrics_for_events.record_dlr(idx, start.elapsed());
                                        }
                                        metrics_for_events.record_dlr_state(idx, ms);
                                        let delivered = matches!(ms, rusmpp::values::MessageState::Delivered);
                                        let failed = matches!(ms, rusmpp::values::MessageState::Undeliverable
                                            | rusmpp::values::MessageState::Rejected
                                            | rusmpp::values::MessageState::Expired
                                            | rusmpp::values::MessageState::Deleted);
                                        metrics_for_events.record_dlr_status(idx, delivered, failed);
                                    }
                                }
                            }
                        }
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
        sent_index,
    )
    .await?;

    client.unbind().await.ok();
    client.close().await.ok();
    Ok(())
}

fn build_bind_trx_pdu(config: &Config) -> Result<BindTransceiver> {
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

fn build_bind_tx_pdu(config: &Config) -> Result<BindTransmitter> {
    Ok(BindTransmitter::builder()
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
        .data_coding(DataCoding::from(message.data_coding))
        .registered_delivery(if message.request_dlr {
            RegisteredDelivery::request_all()
        } else {
            RegisteredDelivery::default()
        })
        .short_message(OctetString::from_str(&message.body)?)
        .build())
}

fn parse_textual_dlr(text: &str) -> Option<(String, rusmpp::values::MessageState)> {
    let mut id: Option<String> = None;
    let mut stat: Option<String> = None;
    // Tokenize on whitespace; fields are key:value
    for token in text.split_whitespace() {
        if let Some(rest) = token.strip_prefix("id:") {
            if !rest.is_empty() {
                id = Some(rest.to_string());
            }
        } else if let Some(rest) = token.strip_prefix("stat:") {
            if !rest.is_empty() {
                stat = Some(rest.to_string());
            }
        }
        if id.is_some() && stat.is_some() {
            break;
        }
    }
    let id = id?;
    let ms = map_stat_to_message_state(stat.as_deref().unwrap_or(""));
    Some((id, ms))
}

fn map_stat_to_message_state(stat: &str) -> rusmpp::values::MessageState {
    use rusmpp::values::MessageState as MS;
    match stat.to_ascii_uppercase().as_str() {
        "DELIVRD" | "DELIVERED" => MS::Delivered,
        "ENROUTE" => MS::Enroute,
        "EXPIRED" => MS::Expired,
        "DELETED" => MS::Deleted,
        "UNDELIV" | "UNDELIVERABLE" => MS::Undeliverable,
        "ACCEPTD" | "ACCEPTED" => MS::Accepted,
        "REJECTD" | "REJECTED" => MS::Rejected,
        "UNKNOWN" => MS::Unknown,
        _ => MS::Unknown,
    }
}

async fn drive_submit_loop(
    idx: usize,
    client: rusmppc::Client,
    submit_template: SubmitSm,
    metrics: Arc<Metrics>,
    tracker: Arc<BindTracker>,
    config: &Config,
    shutdown: CancellationToken,
    sent_index: Arc<dashmap::DashMap<String, Instant>>,
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
            sent_index,
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
            sent_index,
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
    sent_index: Arc<dashmap::DashMap<String, Instant>>,
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
                handle_outcome(idx, outcome, &metrics, &tracker, &sent_index).await;
                queue_if_capacity(
                    &mut inflight,
                    max_inflight,
                    client.clone(),
                    submit_template.clone(),
                );
            }
        }
    }

    drain_inflight(idx, inflight, &metrics, &tracker, &sent_index).await;
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
    sent_index: Arc<dashmap::DashMap<String, Instant>>,
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
                handle_outcome(idx, outcome, &metrics, &tracker, &sent_index).await;
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

    drain_inflight(idx, inflight, &metrics, &tracker, &sent_index).await;
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
    sent_index: &Arc<dashmap::DashMap<String, Instant>>,
) {
    match outcome {
        (Ok(resp), latency) => {
            tracing::debug!(bind = idx, ?resp, "SubmitSmResp");
            metrics.record_success(idx, latency);
            let message_id = resp.message_id().as_str().to_string();
            tracker
                .set_last_message_id(idx, Some(message_id.clone()))
                .await;
            sent_index.insert(message_id, Instant::now());
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
    sent_index: &Arc<dashmap::DashMap<String, Instant>>,
) {
    while let Some(outcome) = inflight.next().await {
        handle_outcome(idx, outcome, metrics, tracker, sent_index).await;
    }
}
