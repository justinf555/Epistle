use std::sync::Arc;

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
                            eprintln!("Initial sync failed: {e}");
                        }
                    });
                }
                AppEvent::AppShutdown => {
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
        let goa_accounts = self.goa.lock().await.discover_accounts().await?;
        let domain_accounts: Vec<Account> = goa_accounts.iter().map(goa_to_account).collect();

        eprintln!(
            "Discovered {} mail account(s){}",
            domain_accounts.len(),
            if domain_accounts.is_empty() {
                ". Add one in GNOME Settings → Online Accounts."
            } else {
                ""
            }
        );

        for account in &domain_accounts {
            eprintln!("  • {} ({})", account.email_address, account.provider_name);
        }

        // Persist accounts — MailEngine emits AccountsChanged if data changed
        self.accounts.sync_accounts(&domain_accounts).await?;

        // Get IMAP credentials for all accounts (requires GOA lock)
        let mut imap_tasks = Vec::new();
        {
            let goa = self.goa.lock().await;
            for (account, goa_account) in domain_accounts.iter().zip(goa_accounts.iter()) {
                match goa.get_imap_auth(&account.goa_id).await {
                    Ok(auth) => {
                        imap_tasks.push((account.clone(), goa_account.imap_config.clone(), auth));
                    }
                    Err(e) => {
                        eprintln!(
                            "  Failed to get IMAP credentials for {}: {e}",
                            account.email_address
                        );
                    }
                }
            }
        }

        // Discover folders in parallel — each IMAP connection is independent
        let futures: Vec<_> = imap_tasks
            .into_iter()
            .map(|(account, config, auth)| {
                let folders_impl = Arc::clone(&self.folders);
                async move {
                    match crate::sync::imap::discover_folders(&config, &auth).await {
                        Ok(imap_folders) => {
                            let folders: Vec<Folder> =
                                imap_folders.iter().map(imap_to_folder).collect();
                            eprintln!(
                                "  {} folders for {}",
                                folders.len(),
                                account.email_address
                            );
                            if let Err(e) = folders_impl.sync_folders(&account, &folders).await {
                                eprintln!(
                                    "  Failed to persist folders for {}: {e}",
                                    account.email_address
                                );
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "  IMAP folder discovery failed for {}: {e}",
                                account.email_address
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
                match goa.get_imap_auth(&account.goa_id).await {
                    Ok(auth) => {
                        inbox_tasks.push((account.clone(), goa_account.imap_config.clone(), auth));
                    }
                    Err(e) => {
                        eprintln!(
                            "  Failed to get IMAP credentials for message sync {}: {e}",
                            account.email_address
                        );
                    }
                }
            }
        }

        let msg_futures: Vec<_> = inbox_tasks
            .into_iter()
            .map(|(account, config, auth)| {
                let messages_impl = Arc::clone(&self.messages);
                let pipeline = &self.pipeline;
                async move {
                    match crate::sync::imap::fetch_messages(
                        &config,
                        &auth,
                        "INBOX",
                        FETCH_BATCH_SIZE,
                    )
                    .await
                    {
                        Ok(raw_emails) => {
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

                            eprintln!(
                                "  {} messages in INBOX for {}",
                                messages.len(),
                                account.email_address
                            );

                            if let Err(e) = messages_impl
                                .sync_messages(&account.goa_id, "INBOX", &messages)
                                .await
                            {
                                eprintln!(
                                    "  Failed to persist messages for {}: {e}",
                                    account.email_address
                                );
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "  IMAP message fetch failed for {}: {e}",
                                account.email_address
                            );
                        }
                    }
                }
            })
            .collect();

        futures::future::join_all(msg_futures).await;

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
