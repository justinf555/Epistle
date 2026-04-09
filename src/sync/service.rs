use std::collections::HashMap;
use std::sync::Arc;

use tracing::{debug, error, info, warn};

use crate::app_event::AppEvent;
use crate::engine::body_store::BodyStore;
use crate::engine::pipeline::parse_body::parse_mime_body;
use crate::engine::pipeline::EmailPipeline;
use crate::engine::traits::accounts::{Account, MailAccounts};
use crate::engine::traits::folders::{Folder, MailFolders};
use crate::engine::traits::messages::{MailMessages, Message};
use crate::event_bus::EventSender;
use crate::goa::types::{GoaMailAccount, ImapConfig, TlsMode};
use crate::goa::GoaClient;
use crate::sync::imap::ImapFolder;
use crate::sync::pool::{max_connections_for_provider, SyncTaskPool};

/// The sync engine — owns all protocol I/O (GOA, IMAP).
///
/// Listens for lifecycle events, discovers accounts and folders via
/// external services, converts protocol types to domain types, and
/// writes results into the MailEngine via trait objects.
///
/// Knows nothing about GTK, the database, or event emission —
/// it just pushes domain data into the engine, which handles the rest.
/// Batch size for IMAP FETCH operations.
const FETCH_BATCH_SIZE: u32 = 200;

pub struct SyncEngine {
    goa: Arc<tokio::sync::Mutex<GoaClient>>,
    pool: Arc<SyncTaskPool>,
    accounts: Arc<dyn MailAccounts>,
    folders: Arc<dyn MailFolders>,
    messages: Arc<dyn MailMessages>,
    body_store: Arc<BodyStore>,
    sender: EventSender,
    pipeline: EmailPipeline,
    running: std::sync::atomic::AtomicBool,
    /// Cached IMAP configs keyed by GOA account ID, populated after initial sync.
    imap_configs: tokio::sync::RwLock<HashMap<String, ImapConfig>>,
    /// Cached provider types keyed by GOA account ID, for connection limit lookup.
    provider_types: tokio::sync::RwLock<HashMap<String, String>>,
}

impl SyncEngine {
    /// Create a new SyncEngine. Connects to GOA over D-Bus.
    pub async fn new(
        accounts: Arc<dyn MailAccounts>,
        folders: Arc<dyn MailFolders>,
        messages: Arc<dyn MailMessages>,
        body_store: Arc<BodyStore>,
        sender: EventSender,
    ) -> anyhow::Result<Arc<Self>> {
        let goa = Arc::new(tokio::sync::Mutex::new(GoaClient::new().await?));
        let pool = Arc::new(SyncTaskPool::new(Arc::clone(&goa)));
        Ok(Arc::new(Self {
            goa,
            pool,
            accounts,
            folders,
            messages,
            body_store,
            sender,
            pipeline: EmailPipeline::new(),
            running: std::sync::atomic::AtomicBool::new(false),
            imap_configs: tokio::sync::RwLock::new(HashMap::new()),
            provider_types: tokio::sync::RwLock::new(HashMap::new()),
        }))
    }

    /// Start the sync service. Subscribes to lifecycle events and reacts.
    /// Must only be called once — guarded by `running` flag.
    pub fn start(self: &Arc<Self>) {
        if self.running.swap(true, std::sync::atomic::Ordering::AcqRel) {
            tracing::warn!("SyncEngine::start() called more than once — ignoring");
            return;
        }
        self.pool.spawn_reaper();
        let engine = Arc::clone(self);
        crate::event_bus::subscribe(move |event| {
            match event {
                AppEvent::AppStarted if engine.running.load(std::sync::atomic::Ordering::Acquire) => {
                    let engine = Arc::clone(&engine);
                    tokio::spawn(async move {
                        if let Err(e) = engine.run_initial_sync().await {
                            error!("Initial sync failed: {e}");
                        }
                    });
                }
                AppEvent::AppShutdown => {
                    info!("Shutting down sync engine");
                    let engine = Arc::clone(&engine);
                    tokio::spawn(async move {
                        engine.pool.shutdown().await;
                        engine.stop();
                    });
                }
                AppEvent::MessageBodyRequested { account_id, folder_name, uid } => {
                    let engine = Arc::clone(&engine);
                    let account_id = account_id.clone();
                    let folder_name = folder_name.clone();
                    let uid = *uid;
                    tokio::spawn(async move {
                        if let Err(e) = engine.fetch_and_emit_body(&account_id, &folder_name, uid).await {
                            error!(
                                account_id = %account_id,
                                folder_name = %folder_name,
                                uid,
                                error = %e,
                                "Body fetch failed"
                            );
                        }
                    });
                }
                _ => {}
            }
        });
    }

    /// Stop the sync service. Events will be ignored until start() is called again.
    pub fn stop(&self) {
        self.running.store(false, std::sync::atomic::Ordering::Release);
    }

    /// Run account discovery + folder sync for all accounts.
    async fn run_initial_sync(&self) -> anyhow::Result<()> {
        info!("Starting initial sync");

        debug!("Discovering accounts via GOA");
        let goa_accounts = self.goa.lock().await.discover_accounts().await?;
        let domain_accounts: Vec<Account> = goa_accounts.iter().map(goa_to_account).collect();

        if domain_accounts.is_empty() {
            warn!("No mail accounts found — add one in GNOME Settings → Online Accounts");
        } else {
            info!(count = domain_accounts.len(), "Discovered mail accounts");
            for account in &domain_accounts {
                info!(
                    email = %account.email_address,
                    provider = %account.provider_name,
                    "Found account"
                );
            }
        }

        // Persist accounts — MailEngine emits AccountsChanged if data changed
        debug!("Persisting accounts to database");
        self.accounts.sync_accounts(&domain_accounts).await?;

        // Cache IMAP configs and provider types for later use
        {
            let mut configs = self.imap_configs.write().await;
            let mut providers = self.provider_types.write().await;
            configs.clear();
            providers.clear();
            for goa_account in &goa_accounts {
                configs.insert(
                    goa_account.goa_id.clone(),
                    goa_account.imap_config.clone(),
                );
                providers.insert(
                    goa_account.goa_id.clone(),
                    goa_account.provider_type.as_goa_str().to_string(),
                );
            }
            debug!(count = configs.len(), "Cached IMAP configs");
        }

        // Get IMAP credentials for all accounts (requires GOA lock)
        let mut imap_tasks = Vec::new();
        {
            let goa = self.goa.lock().await;
            for (account, goa_account) in domain_accounts.iter().zip(goa_accounts.iter()) {
                debug!(email = %account.email_address, "Fetching IMAP credentials");
                match goa.get_imap_auth(&account.goa_id).await {
                    Ok(auth) => {
                        imap_tasks.push((account.clone(), goa_account.imap_config.clone(), auth));
                    }
                    Err(e) => {
                        warn!(
                            email = %account.email_address,
                            error = %e,
                            "Failed to get IMAP credentials for folder sync"
                        );
                    }
                }
            }
        }

        // Discover folders in parallel — each IMAP connection is independent
        debug!(count = imap_tasks.len(), "Starting parallel folder discovery");
        let futures: Vec<_> = imap_tasks
            .into_iter()
            .map(|(account, config, auth)| {
                let folders_impl = Arc::clone(&self.folders);
                async move {
                    debug!(
                        email = %account.email_address,
                        host = %config.host,
                        "Connecting to IMAP for folder discovery"
                    );
                    match crate::sync::imap::discover_folders(&config, &auth).await {
                        Ok(imap_folders) => {
                            let folders: Vec<Folder> =
                                imap_folders.iter().map(imap_to_folder).collect();
                            info!(
                                email = %account.email_address,
                                count = folders.len(),
                                "Discovered folders"
                            );
                            for folder in &folders {
                                debug!(
                                    email = %account.email_address,
                                    folder = %folder.name,
                                    role = ?folder.role,
                                    "Found folder"
                                );
                            }
                            if let Err(e) = folders_impl.sync_folders(&account, &folders).await {
                                error!(
                                    email = %account.email_address,
                                    error = %e,
                                    "Failed to persist folders"
                                );
                            }
                        }
                        Err(e) => {
                            error!(
                                email = %account.email_address,
                                error = %e,
                                "IMAP folder discovery failed"
                            );
                        }
                    }
                }
            })
            .collect();

        futures::future::join_all(futures).await;

        // ── Sync messages for all folders ────────────────────────────────
        // Two-stage pipeline per folder:
        //   Stage 1 (ID sync): uid_search → diff → delete removed
        //   Stage 2 (Header fetch): uid_fetch envelopes for new UIDs, newest first
        //
        // Sequential per folder within each account to avoid IMAP throttling.
        for account in &domain_accounts {
            let mut account_folders = match self.folders.list_folders(&account.goa_id).await {
                Ok(f) => f,
                Err(e) => {
                    error!(
                        email = %account.email_address,
                        error = %e,
                        "Failed to list folders for message sync"
                    );
                    continue;
                }
            };
            account_folders.sort_by_key(|f| folder_priority(f.role.as_deref()));

            info!(
                email = %account.email_address,
                folder_count = account_folders.len(),
                "Syncing messages across all folders"
            );

            let config = {
                let configs = self.imap_configs.read().await;
                configs.get(&account.goa_id).cloned()
            };
            let config = match config {
                Some(c) => c,
                None => {
                    error!(email = %account.email_address, "No cached IMAP config");
                    continue;
                }
            };

            let max_conns = {
                let providers = self.provider_types.read().await;
                providers
                    .get(&account.goa_id)
                    .map(|p| max_connections_for_provider(p))
                    .unwrap_or(10)
            };

            for folder in &account_folders {
                if let Err(e) = self
                    .sync_folder(&account.goa_id, &folder.name, &config, max_conns)
                    .await
                {
                    error!(
                        email = %account.email_address,
                        folder = %folder.name,
                        error = %e,
                        "Folder sync failed"
                    );
                }
            }
        }

        info!("Initial sync complete");
        Ok(())
    }

    /// Two-stage folder sync: ID sync (diff UIDs) then header fetch (envelopes).
    async fn sync_folder(
        &self,
        account_id: &str,
        folder_name: &str,
        config: &ImapConfig,
        max_conns: usize,
    ) -> anyhow::Result<()> {
        // ── Stage 1: ID Sync ────────────────────────────────────────────
        // Get all UIDs from server, diff against local, delete removed.
        let new_uids = {
            let mut guard = self.pool.acquire(account_id, config, max_conns).await?;
            let session = guard.session();
            session.select(folder_name).await?;

            let server_uids = match session.uid_search("ALL").await {
                Ok(uids) => uids,
                Err(e) => {
                    guard.poison();
                    return Err(e.into());
                }
            };

            let local_uids = self.messages.list_local_uids(account_id, folder_name).await?;

            // Delete messages that no longer exist on server
            let deleted_uids: Vec<u32> = local_uids.difference(&server_uids).copied().collect();
            if !deleted_uids.is_empty() {
                let count = self
                    .messages
                    .delete_messages_by_uids(account_id, folder_name, &deleted_uids)
                    .await?;
                info!(account_id, folder_name, count, "Removed deleted messages");
            }

            // New UIDs to fetch, sorted descending (newest first)
            let mut new: Vec<u32> = server_uids.difference(&local_uids).copied().collect();
            new.sort_unstable_by(|a, b| b.cmp(a));

            debug!(
                account_id,
                folder_name,
                server = server_uids.len(),
                local = local_uids.len(),
                new = new.len(),
                deleted = deleted_uids.len(),
                "ID sync complete"
            );

            new
            // guard dropped — connection returned to pool
        };

        if new_uids.is_empty() {
            return Ok(());
        }

        // ── Stage 2: Header Fetch ───────────────────────────────────────
        // Fetch envelopes in batches of FETCH_BATCH_SIZE, newest first.
        for batch in new_uids.chunks(FETCH_BATCH_SIZE as usize) {
            let uid_set = batch
                .iter()
                .map(|u| u.to_string())
                .collect::<Vec<_>>()
                .join(",");

            let mut guard = self.pool.acquire(account_id, config, max_conns).await?;
            let session = guard.session();
            session.select(folder_name).await?;

            let fetches = match session
                .uid_fetch(&uid_set, "(UID ENVELOPE FLAGS INTERNALDATE)")
                .await
            {
                Ok(f) => f,
                Err(e) => {
                    guard.poison();
                    return Err(e.into());
                }
            };

            let messages: Vec<Message> = fetches
                .iter()
                .filter_map(|fetch| {
                    let raw = crate::sync::imap::fetch_to_raw_email(fetch)?;
                    let mut msg = Message {
                        uuid: uuid::Uuid::new_v4().to_string(),
                        uid: raw.uid,
                        account_id: account_id.to_string(),
                        folder_name: folder_name.to_string(),
                        message_id: None,
                        subject: None,
                        sender: None,
                        to_addresses: vec![],
                        cc_addresses: vec![],
                        date: None,
                        in_reply_to: None,
                        references: vec![],
                        is_read: false,
                        is_flagged: false,
                        is_answered: false,
                        is_draft: false,
                        preview: None,
                        content_type: None,
                        has_attachments: false,
                        internal_date: None,
                    };
                    self.pipeline.process(&mut msg, &raw);
                    Some(msg)
                })
                .collect();

            info!(
                account_id,
                folder_name,
                count = messages.len(),
                "Fetched message headers"
            );

            if let Err(e) = self
                .messages
                .sync_messages(account_id, folder_name, &messages)
                .await
            {
                error!(
                    account_id,
                    folder_name,
                    error = %e,
                    "Failed to persist messages"
                );
            }

            // guard dropped — connection returned to pool
        }

        Ok(())
    }

    /// Fetch a single message body from IMAP, store as .eml, parse, and emit event.
    async fn fetch_and_emit_body(
        &self,
        account_id: &str,
        folder_name: &str,
        uid: u32,
    ) -> anyhow::Result<()> {
        debug!(account_id, folder_name, uid, "Fetching message body");

        // Look up the UUID for this message
        let uuid = self.messages.get_uuid(account_id, folder_name, uid).await?;
        let uuid = match uuid {
            Some(u) => u,
            None => {
                warn!(uid, "No UUID found for message — cannot store body");
                return Ok(());
            }
        };

        // Check if .eml already exists on disk
        if self.body_store.has_eml(&uuid).await {
            debug!(uid, uuid = %uuid, "Body already on disk, parsing from file");
            if let Some(raw) = self.body_store.read_eml(&uuid).await? {
                let body = parse_mime_body(&raw);
                self.sender.send(AppEvent::MessageBodyFetched {
                    account_id: account_id.to_string(),
                    folder_name: folder_name.to_string(),
                    uid,
                    body,
                });
                return Ok(());
            }
        }

        // Get IMAP config from cache
        let config = {
            let configs = self.imap_configs.read().await;
            configs.get(account_id).cloned()
        };
        let config = match config {
            Some(c) => c,
            None => {
                warn!(account_id, "IMAP config not cached, re-discovering from GOA");
                let c = {
                    let mut goa = self.goa.lock().await;
                    let goa_accounts = goa.discover_accounts().await?;
                    goa_accounts
                        .iter()
                        .find(|a| a.goa_id == account_id)
                        .map(|a| a.imap_config.clone())
                        .ok_or_else(|| anyhow::anyhow!("Account {account_id} not found in GOA"))?
                };
                self.imap_configs.write().await.insert(account_id.to_string(), c.clone());
                c
            }
        };

        let max_conns = {
            let providers = self.provider_types.read().await;
            providers
                .get(account_id)
                .map(|p| max_connections_for_provider(p))
                .unwrap_or(10)
        };

        // Acquire pooled connection and fetch body
        let mut guard = self.pool.acquire(account_id, &config, max_conns).await?;
        let session = guard.session();

        session.select(folder_name).await?;

        let uid_str = uid.to_string();
        let fetches = session.uid_fetch(&uid_str, "BODY[]").await;
        let fetches = match fetches {
            Ok(f) => f,
            Err(e) => {
                guard.poison();
                return Err(e.into());
            }
        };

        let raw_bytes = fetches
            .iter()
            .find_map(|f| f.body().map(|b| b.to_vec()))
            .ok_or_else(|| crate::sync::imap::ImapError::MessageNotFound { uid })?;

        // Store .eml to filesystem
        self.body_store.store_eml(&uuid, &raw_bytes).await
            .map_err(|e| anyhow::anyhow!("Failed to store .eml: {e}"))?;

        // Parse MIME and emit event
        let body = parse_mime_body(&raw_bytes);

        debug!(
            uid,
            uuid = %uuid,
            has_html = body.body_html.is_some(),
            has_text = body.body_text.is_some(),
            "Body fetched and stored"
        );

        self.sender.send(AppEvent::MessageBodyFetched {
            account_id: account_id.to_string(),
            folder_name: folder_name.to_string(),
            uid,
            body,
        });

        Ok(())
    }
}

// ── Protocol → Domain conversions ───────────────────────────────────────────

fn tls_mode_to_string(mode: TlsMode) -> String {
    match mode {
        TlsMode::Implicit => "implicit".to_string(),
        TlsMode::StartTls => "starttls".to_string(),
        TlsMode::None => "none".to_string(),
    }
}

fn goa_to_account(goa: &GoaMailAccount) -> Account {
    Account {
        goa_id: goa.goa_id.clone(),
        provider_type: goa.provider_type.as_goa_str().to_string(),
        provider_name: goa.provider_name.clone(),
        email_address: goa.email_address.clone(),
        display_name: goa.display_name.clone(),
        imap_host: goa.imap_config.host.clone(),
        imap_port: goa.imap_config.port,
        imap_tls_mode: tls_mode_to_string(goa.imap_config.tls_mode),
        smtp_host: goa.smtp_config.as_ref().map(|c| c.host.clone()),
        smtp_port: goa.smtp_config.as_ref().map(|c| c.port),
        smtp_tls_mode: goa
            .smtp_config
            .as_ref()
            .map(|c| tls_mode_to_string(c.tls_mode)),
        attention_needed: goa.attention_needed,
    }
}

fn imap_to_folder(imap: &ImapFolder) -> Folder {
    Folder {
        name: imap.name.clone(),
        delimiter: imap.delimiter.clone(),
        role: imap.role.clone(),
    }
}

/// Sort key for folder sync priority: inbox first, then common roles, then the rest.
fn folder_priority(role: Option<&str>) -> u8 {
    match role {
        Some("inbox") => 0,
        Some("sent") => 1,
        Some("drafts") => 2,
        Some("archive") => 3,
        Some("trash") => 4,
        Some("junk") => 5,
        _ => 6,
    }
}
