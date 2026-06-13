//! The ingest pipeline: a non-blocking submit handle backed by a background
//! batched writer, plus the compaction task.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;
use tracing_batteries::prelude::*;

use super::compactor;
use crate::config::StorageConfig;
use crate::store::{Store, StoredEvent};

const QUEUE_CAPACITY: usize = 16_384;
const BATCH_SIZE: usize = 512;
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);

/// Cloneable handle for submitting events into the pipeline.
#[derive(Clone)]
pub struct Ingest {
    tx: mpsc::Sender<StoredEvent>,
}

impl Ingest {
    /// Non-blocking submit; drops (with a warning) if the queue is saturated so a
    /// flood can never block request handling.
    pub fn submit(&self, event: StoredEvent) {
        if let Err(err) = self.tx.try_send(event) {
            warn!("ingest queue full; dropping event ({err})");
        }
    }
}

/// Spawn the background writer + compactor and return the submit handle.
pub fn spawn(store: Arc<Store>, storage: StorageConfig) -> Ingest {
    let (tx, rx) = mpsc::channel(QUEUE_CAPACITY);
    tokio::spawn(writer_loop(store.clone(), rx));
    tokio::spawn(compactor::run(store, storage));
    Ingest { tx }
}

async fn writer_loop(store: Arc<Store>, mut rx: mpsc::Receiver<StoredEvent>) {
    // Track already-registered sources in memory to avoid a store hit per event;
    // seed it once from the persisted sources.
    let mut known_sources: HashSet<String> = {
        let store = store.clone();
        match tokio::task::spawn_blocking(move || store.list_sources()).await {
            Ok(Ok(sources)) => sources.into_iter().map(|s| s.uri).collect(),
            _ => HashSet::new(),
        }
    };

    let mut batch: Vec<StoredEvent> = Vec::with_capacity(BATCH_SIZE);
    let mut flush = tokio::time::interval(FLUSH_INTERVAL);
    flush.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            received = rx.recv() => match received {
                Some(event) => {
                    batch.push(event);
                    if batch.len() >= BATCH_SIZE {
                        flush_batch(&store, &mut batch, &mut known_sources).await;
                    }
                }
                None => {
                    flush_batch(&store, &mut batch, &mut known_sources).await;
                    break;
                }
            },
            _ = flush.tick() => flush_batch(&store, &mut batch, &mut known_sources).await,
        }
    }
}

/// Persist the current batch off the async runtime (redb writes are synchronous),
/// auto-registering any newly-seen sources as unassigned.
async fn flush_batch(
    store: &Arc<Store>,
    batch: &mut Vec<StoredEvent>,
    known_sources: &mut HashSet<String>,
) {
    if batch.is_empty() {
        return;
    }
    let events = std::mem::take(batch);

    // Distinct sources in this batch not yet known to this process.
    let mut new_sources: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for event in &events {
        if !known_sources.contains(&event.source) && seen.insert(event.source.clone()) {
            new_sources.push(event.source.clone());
        }
    }

    let store = store.clone();
    let to_register = new_sources.clone();
    let result = tokio::task::spawn_blocking(move || -> crate::errors::Result<()> {
        store.append_events(&events)?;
        for uri in &to_register {
            store.register_source_if_absent(uri)?;
        }
        Ok(())
    })
    .await;

    match result {
        Ok(Ok(())) => known_sources.extend(new_sources),
        Ok(Err(err)) => error!("failed to persist events: {err}"),
        Err(err) => error!("event writer task panicked: {err}"),
    }
}
