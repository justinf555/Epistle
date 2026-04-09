//! Body fetch worker with priority and background channels.
//!
//! Accepts `FetchBodyRequest` items from two sources:
//! - **Priority channel**: user-initiated (clicked a message) — always checked first
//! - **Background channel**: prefetch from header worker — processed when priority is empty
//!
//! Uses `tokio::select!` with `biased` to ensure priority requests are never starved.

use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{debug, error, warn};

use crate::app_event::AppEvent;
use crate::engine::body_store::BodyStore;
use crate::engine::pipeline::parse_body::parse_mime_body;
use crate::event_bus::EventSender;
use crate::goa::types::ImapConfig;
use crate::sync::pool::SyncTaskPool;

/// A request to fetch a message body.
#[derive(Debug, Clone)]
pub struct FetchBodyRequest {
    pub uid: u32,
    pub uuid: String,
    pub account_id: String,
    pub folder_name: String,
    pub priority: bool,
}

/// Body fetch worker that processes priority and background requests.
pub struct BodyWorker {
    priority_rx: mpsc::Receiver<FetchBodyRequest>,
    background_rx: mpsc::Receiver<FetchBodyRequest>,
    pool: Arc<SyncTaskPool>,
    body_store: Arc<BodyStore>,
    sender: EventSender,
    /// Cached IMAP configs — shared with SyncEngine via Arc<RwLock>.
    imap_configs: Arc<tokio::sync::RwLock<std::collections::HashMap<String, ImapConfig>>>,
    /// Cached provider types — shared with SyncEngine via Arc<RwLock>.
    provider_types: Arc<tokio::sync::RwLock<std::collections::HashMap<String, String>>>,
}

impl BodyWorker {
    pub fn new(
        priority_rx: mpsc::Receiver<FetchBodyRequest>,
        background_rx: mpsc::Receiver<FetchBodyRequest>,
        pool: Arc<SyncTaskPool>,
        body_store: Arc<BodyStore>,
        sender: EventSender,
        imap_configs: Arc<tokio::sync::RwLock<std::collections::HashMap<String, ImapConfig>>>,
        provider_types: Arc<tokio::sync::RwLock<std::collections::HashMap<String, String>>>,
    ) -> Self {
        Self {
            priority_rx,
            background_rx,
            pool,
            body_store,
            sender,
            imap_configs,
            provider_types,
        }
    }

    /// Run the worker loop. Returns when both channels are closed.
    pub async fn run(&mut self) {
        debug!("Body worker started");
        loop {
            let request = tokio::select! {
                biased;
                Some(mut req) = self.priority_rx.recv() => {
                    debug!(uid = req.uid, uuid = %req.uuid, "Priority body request");
                    req.priority = true;
                    req
                }
                Some(mut req) = self.background_rx.recv() => {
                    debug!(uid = req.uid, uuid = %req.uuid, "Background body request");
                    req.priority = false;
                    req
                }
                else => {
                    debug!("Body worker channels closed, shutting down");
                    break;
                }
            };

            if let Err(e) = self.fetch_body(&request).await {
                error!(
                    uid = request.uid,
                    uuid = %request.uuid,
                    error = %e,
                    "Body fetch failed"
                );
            }
        }
    }

    async fn fetch_body(&self, req: &FetchBodyRequest) -> anyhow::Result<()> {
        let t0 = std::time::Instant::now();

        // Check if .eml already exists on disk
        if self.body_store.has_eml(&req.account_id, &req.uuid).await {
            let t_disk = t0.elapsed();
            if let Some(raw) = self.body_store.read_eml(&req.account_id, &req.uuid).await? {
                let t_read = t0.elapsed();
                let body = parse_mime_body(&raw);
                let t_parse = t0.elapsed();
                debug!(
                    uid = req.uid,
                    bytes = raw.len(),
                    disk_check_ms = t_disk.as_millis() as u64,
                    read_ms = (t_read - t_disk).as_millis() as u64,
                    parse_ms = (t_parse - t_read).as_millis() as u64,
                    total_ms = t_parse.as_millis() as u64,
                    "Body served from disk"
                );
                self.sender.send(AppEvent::MessageBodyFetched {
                    account_id: req.account_id.clone(),
                    folder_name: req.folder_name.clone(),
                    uid: req.uid,
                    body,
                });
                return Ok(());
            }
        }

        // Get IMAP config
        let config = {
            let configs = self.imap_configs.read().await;
            configs.get(&req.account_id).cloned()
        };
        let config = match config {
            Some(c) => c,
            None => {
                warn!(account_id = %req.account_id, "No IMAP config for body fetch");
                return Ok(());
            }
        };

        let max_conns = {
            let providers = self.provider_types.read().await;
            providers
                .get(&req.account_id)
                .map(|p| crate::sync::pool::max_connections_for_provider(p))
                .unwrap_or(10)
        };

        // Fetch from IMAP
        debug!(
            uid = req.uid,
            uuid = %req.uuid,
            account_id = %req.account_id,
            folder = %req.folder_name,
            "Fetching body from IMAP"
        );
        let t_imap_start = t0.elapsed();
        let mut guard = if req.priority {
            self.pool.acquire_priority(&req.account_id, &config, max_conns).await?
        } else {
            self.pool.acquire(&req.account_id, &config, max_conns).await?
        };
        let t_pool = t0.elapsed();
        let session = guard.session();

        session.select(&req.folder_name).await?;
        let t_select = t0.elapsed();

        let uid_str = req.uid.to_string();
        let fetches = match session.uid_fetch(&uid_str, "BODY[]").await {
            Ok(f) => f,
            Err(e) => {
                guard.poison();
                return Err(e.into());
            }
        };
        let t_fetch = t0.elapsed();

        let raw_bytes = fetches
            .iter()
            .find_map(|f| f.body().map(|b| b.to_vec()))
            .ok_or_else(|| crate::sync::imap::ImapError::MessageNotFound { uid: req.uid })?;

        // Store .eml
        self.body_store
            .store_eml(&req.account_id, &req.uuid, &raw_bytes)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to store .eml: {e}"))?;
        let t_store = t0.elapsed();

        // Parse and emit
        let body = parse_mime_body(&raw_bytes);
        let t_parse = t0.elapsed();

        debug!(
            uid = req.uid,
            bytes = raw_bytes.len(),
            pool_ms = (t_pool - t_imap_start).as_millis() as u64,
            select_ms = (t_select - t_pool).as_millis() as u64,
            fetch_ms = (t_fetch - t_select).as_millis() as u64,
            store_ms = (t_store - t_fetch).as_millis() as u64,
            parse_ms = (t_parse - t_store).as_millis() as u64,
            total_ms = t_parse.as_millis() as u64,
            "Body fetched and stored"
        );

        self.sender.send(AppEvent::MessageBodyFetched {
            account_id: req.account_id.clone(),
            folder_name: req.folder_name.clone(),
            uid: req.uid,
            body,
        });

        Ok(())
    }
}
