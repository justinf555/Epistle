use std::sync::Arc;

use crate::app_event::AppEvent;
use crate::engine::traits::accounts::{Account, MailAccounts};
use crate::engine::traits::folders::{Folder, MailFolders};
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
pub struct SyncEngine {
    goa: tokio::sync::Mutex<GoaClient>,
    accounts: Arc<dyn MailAccounts>,
    folders: Arc<dyn MailFolders>,
}

impl SyncEngine {
    /// Create a new SyncEngine. Connects to GOA over D-Bus.
    pub async fn new(
        accounts: Arc<dyn MailAccounts>,
        folders: Arc<dyn MailFolders>,
    ) -> anyhow::Result<Arc<Self>> {
        let goa = GoaClient::new().await?;
        Ok(Arc::new(Self {
            goa: tokio::sync::Mutex::new(goa),
            accounts,
            folders,
        }))
    }

    /// Subscribe to the event bus. Reacts to lifecycle events.
    pub fn subscribe(self: &Arc<Self>) {
        let engine = Arc::clone(self);
        crate::event_bus::subscribe(move |event| {
            if matches!(event, AppEvent::AppStarted) {
                let engine = Arc::clone(&engine);
                tokio::spawn(async move {
                    if let Err(e) = engine.run_initial_sync().await {
                        eprintln!("Initial sync failed: {e}");
                    }
                });
            }
        });
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
