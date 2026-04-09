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
                Some(req) = self.priority_rx.recv() => {
                    debug!(uid = req.uid, uuid = %req.uuid, "Priority body request");
                    req
                }
                Some(req) = self.background_rx.recv() => {
                    debug!(uid = req.uid, uuid = %req.uuid, "Background body request");
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
        // Check if .eml already exists on disk
        if self.body_store.has_eml(&req.account_id, &req.uuid).await {
            debug!(uid = req.uid, uuid = %req.uuid, "Body already on disk");
            if let Some(raw) = self.body_store.read_eml(&req.account_id, &req.uuid).await? {
                let body = parse_mime_body(&raw);
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
        let mut guard = self.pool.acquire(&req.account_id, &config, max_conns).await?;
        let session = guard.session();

        session.select(&req.folder_name).await?;

        let uid_str = req.uid.to_string();
        let fetches = match session.uid_fetch(&uid_str, "BODY[]").await {
            Ok(f) => f,
            Err(e) => {
                guard.poison();
                return Err(e.into());
            }
        };

        let raw_bytes = fetches
            .iter()
            .find_map(|f| f.body().map(|b| b.to_vec()))
            .ok_or_else(|| crate::sync::imap::ImapError::MessageNotFound { uid: req.uid })?;

        // Store .eml
        self.body_store
            .store_eml(&req.account_id, &req.uuid, &raw_bytes)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to store .eml: {e}"))?;

        // Parse and emit
        let body = parse_mime_body(&raw_bytes);

        debug!(
            uid = req.uid,
            uuid = %req.uuid,
            has_html = body.body_html.is_some(),
            has_text = body.body_text.is_some(),
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
