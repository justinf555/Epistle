use std::sync::Arc;

use tracing::{debug, error, info, warn};

use crate::app_event::AppEvent;
use crate::engine::pipeline::EmailPipeline;
use crate::engine::traits::accounts::{Account, MailAccounts};
use crate::engine::traits::folders::{Folder, MailFolders};
use crate::engine::traits::messages::{MailMessages, Message};
use crate::goa::types::{GoaMailAccount, TlsMode};
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
    pipeline: EmailPipeline,
    running: std::sync::atomic::AtomicBool,
}

impl SyncEngine {
    /// Create a new SyncEngine. Connects to GOA over D-Bus.
    pub async fn new(
        accounts: Arc<dyn MailAccounts>,
        folders: Arc<dyn MailFolders>,
        messages: Arc<dyn MailMessages>,
    ) -> anyhow::Result<Arc<Self>> {
        let goa = GoaClient::new().await?;
        Ok(Arc::new(Self {
            goa: tokio::sync::Mutex::new(goa),
            accounts,
            folders,
            messages,
            pipeline: EmailPipeline::new(),
            running: std::sync::atomic::AtomicBool::new(false),
        }))
    }

    /// Start the sync service. Subscribes to lifecycle events and reacts.
    pub fn start(self: &Arc<Self>) {
        self.running.store(true, std::sync::atomic::Ordering::Relaxed);
        let engine = Arc::clone(self);
        crate::event_bus::subscribe(move |event| {
            match event {
                AppEvent::AppStarted if engine.running.load(std::sync::atomic::Ordering::Relaxed) => {
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
                _ => {}
            }
        });
    }

    /// Stop the sync service. Events will be ignored until start() is called again.
    pub fn stop(&self) {
        self.running.store(false, std::sync::atomic::Ordering::Relaxed);
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
