use std::collections::HashMap;
use std::sync::Arc;

use tracing::{debug, error, info, warn};

use crate::app_event::AppEvent;
use crate::engine::pipeline::parse_body::parse_mime_body;
use crate::engine::pipeline::EmailPipeline;
use crate::engine::traits::accounts::{Account, MailAccounts};
use crate::engine::traits::folders::{Folder, MailFolders};
use crate::engine::traits::messages::{MailMessages, Message};
use crate::event_bus::EventSender;
use crate::goa::types::{GoaMailAccount, ImapConfig, TlsMode};
use crate::goa::GoaClient;
use crate::sync::imap::ImapFolder;

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
    goa: tokio::sync::Mutex<GoaClient>,
    accounts: Arc<dyn MailAccounts>,
    folders: Arc<dyn MailFolders>,
    messages: Arc<dyn MailMessages>,
    sender: EventSender,
    pipeline: EmailPipeline,
    running: std::sync::atomic::AtomicBool,
    /// Cached IMAP configs keyed by GOA account ID, populated after initial sync.
    imap_configs: tokio::sync::RwLock<HashMap<String, ImapConfig>>,
}

impl SyncEngine {
    /// Create a new SyncEngine. Connects to GOA over D-Bus.
    pub async fn new(
        accounts: Arc<dyn MailAccounts>,
        folders: Arc<dyn MailFolders>,
        messages: Arc<dyn MailMessages>,
        sender: EventSender,
    ) -> anyhow::Result<Arc<Self>> {
        let goa = GoaClient::new().await?;
        Ok(Arc::new(Self {
            goa: tokio::sync::Mutex::new(goa),
            accounts,
            folders,
            messages,
            sender,
            pipeline: EmailPipeline::new(),
            running: std::sync::atomic::AtomicBool::new(false),
            imap_configs: tokio::sync::RwLock::new(HashMap::new()),
        }))
    }

    /// Start the sync service. Subscribes to lifecycle events and reacts.
    /// Must only be called once — guarded by `running` flag.
    pub fn start(self: &Arc<Self>) {
        if self.running.swap(true, std::sync::atomic::Ordering::AcqRel) {
            tracing::warn!("SyncEngine::start() called more than once — ignoring");
            return;
        }
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
                    engine.stop();
                }
                AppEvent::MessageBodyRequested { account_id, folder_name, uid } => {
                    let engine = Arc::clone(&engine);
                    let account_id = account_id.clone();
                    let folder_name = folder_name.clone();
                    let uid = *uid;
                    tokio::spawn(async move {
                        if let Err(e) = engine.fetch_and_cache_body(&account_id, &folder_name, uid).await {
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

        // Cache IMAP configs for later use by fetch_and_cache_body
        {
            let mut configs = self.imap_configs.write().await;
            configs.clear();
            for goa_account in &goa_accounts {
                configs.insert(
                    goa_account.goa_id.clone(),
                    goa_account.imap_config.clone(),
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

        // ── Phase 1: Fetch messages from inbox ─────────────────────────────
        // Currently inbox-only to avoid IMAP throttling on initial sync.
        // To expand to other folders: iterate the synced folders list, prioritise
        // by role (inbox → sent → drafts → etc.), and call fetch_messages() for
        // each. The fetch_messages() and sync_messages() APIs already accept any
        // folder name — only this call site is inbox-specific.
        //
        // Re-acquire IMAP credentials for message fetch (tokens may have refreshed)
        let mut inbox_tasks = Vec::new();
        {
            let goa = self.goa.lock().await;
            for (account, goa_account) in domain_accounts.iter().zip(goa_accounts.iter()) {
                debug!(email = %account.email_address, "Fetching IMAP credentials for message sync");
                match goa.get_imap_auth(&account.goa_id).await {
                    Ok(auth) => {
                        inbox_tasks.push((account.clone(), goa_account.imap_config.clone(), auth));
                    }
                    Err(e) => {
                        warn!(
                            email = %account.email_address,
                            error = %e,
                            "Failed to get IMAP credentials for message sync"
                        );
                    }
                }
            }
        }

        debug!(count = inbox_tasks.len(), "Starting inbox message fetch");
        let msg_futures: Vec<_> = inbox_tasks
            .into_iter()
            .map(|(account, config, auth)| {
                let messages_impl = Arc::clone(&self.messages);
                let pipeline = &self.pipeline;
                async move {
                    debug!(
                        email = %account.email_address,
                        host = %config.host,
                        batch_size = FETCH_BATCH_SIZE,
                        "Connecting to IMAP for message fetch"
                    );
                    match crate::sync::imap::fetch_messages(
                        &config,
                        &auth,
                        "INBOX",
                        FETCH_BATCH_SIZE,
                    )
                    .await
                    {
                        Ok(raw_emails) => {
                            debug!(
                                email = %account.email_address,
                                raw_count = raw_emails.len(),
                                "Fetched raw messages, running pipeline"
                            );

                            let messages: Vec<Message> = raw_emails
                                .iter()
                                .map(|raw| {
                                    let mut msg = Message {
                                        uid: raw.uid,
                                        account_id: account.goa_id.clone(),
                                        folder_name: "INBOX".to_string(),
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
                                        body_text: None,
                                        body_html: None,
                                    };
                                    pipeline.process(&mut msg, raw);
                                    msg
                                })
                                .collect();

                            info!(
                                email = %account.email_address,
                                count = messages.len(),
                                folder = "INBOX",
                                "Synced messages"
                            );

                            if let Err(e) = messages_impl
                                .sync_messages(&account.goa_id, "INBOX", &messages)
                                .await
                            {
                                error!(
                                    email = %account.email_address,
                                    error = %e,
                                    "Failed to persist messages"
                                );
                            }
                        }
                        Err(e) => {
                            error!(
                                email = %account.email_address,
                                error = %e,
                                "IMAP message fetch failed"
                            );
                        }
                    }
                }
            })
            .collect();

        futures::future::join_all(msg_futures).await;

        info!("Initial sync complete");
        Ok(())
    }

    /// Fetch a single message body from IMAP, parse MIME, cache in DB, emit event.
    async fn fetch_and_cache_body(
        &self,
        account_id: &str,
        folder_name: &str,
        uid: u32,
    ) -> anyhow::Result<()> {
        debug!(account_id, folder_name, uid, "Fetching message body");

        // Check DB cache first
        if let Some(body) = self.messages.get_body(account_id, folder_name, uid).await? {
            if body.body_text.is_some() || body.body_html.is_some() {
                debug!(uid, "Body already cached, emitting from cache");
                self.sender.send(AppEvent::MessageBodyFetched {
                    account_id: account_id.to_string(),
                    folder_name: folder_name.to_string(),
                    uid,
                    body,
                });
                return Ok(());
            }
        }

        // Get IMAP config from cache (populated during initial sync)
        let config = {
            let configs = self.imap_configs.read().await;
            configs.get(account_id).cloned()
        };
        let config = match config {
            Some(c) => c,
            None => {
                // Cache miss — re-discover from GOA (e.g. new account added)
                warn!(account_id, "IMAP config not cached, re-discovering from GOA");
                let c = {
                    let mut goa = self.goa.lock().await;
                    let goa_accounts = goa.discover_accounts().await?;
                    goa_accounts
                        .iter()
                        .find(|a| a.goa_id == account_id)
                        .map(|a| a.imap_config.clone())
                        .ok_or_else(|| anyhow::anyhow!("Account {account_id} not found in GOA"))?
                    // goa guard dropped here
                };
                self.imap_configs.write().await.insert(account_id.to_string(), c.clone());
                c
            }
        };

        // Get fresh auth credentials (tokens may have short TTL)
        let auth = self.goa.lock().await.get_imap_auth(account_id).await?;

        // Fetch from IMAP
        let raw_bytes = crate::sync::imap::fetch_message_body(&config, &auth, folder_name, uid).await?;

        // Parse MIME
        let body = parse_mime_body(&raw_bytes);

        // Cache in DB
        self.messages
            .cache_body(
                account_id,
                folder_name,
                uid,
                body.body_text.as_deref(),
                body.body_html.as_deref(),
            )
            .await?;

        debug!(
            uid,
            has_html = body.body_html.is_some(),
            has_text = body.body_text.is_some(),
            "Body fetched and cached"
        );

        // Emit event
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
