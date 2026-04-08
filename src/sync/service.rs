use std::sync::Arc;

use gtk::glib;

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
    pub fn new(
        goa: GoaClient,
        accounts: Arc<dyn MailAccounts>,
        folders: Arc<dyn MailFolders>,
    ) -> Arc<Self> {
        Arc::new(Self {
            goa: tokio::sync::Mutex::new(goa),
            accounts,
            folders,
        })
    }

    /// Subscribe to the event bus. Reacts to lifecycle events.
    ///
    /// Uses the free `event_bus::subscribe` function so this can be called
    /// without a `&EventBus` reference (e.g. from within an async block).
    pub fn subscribe(self: &Arc<Self>) {
        let engine = Arc::clone(self);
        crate::event_bus::subscribe(move |event| {
            if matches!(event, AppEvent::AppStarted) {
                let engine = Arc::clone(&engine);
                glib::MainContext::default().spawn_local(async move {
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

        // Persist accounts — MailEngine emits AccountsChanged
        self.accounts.sync_accounts(&domain_accounts).await?;

        // Discover IMAP folders for each account
        for (account, goa_account) in domain_accounts.iter().zip(goa_accounts.iter()) {
            match self.goa.lock().await.get_imap_auth(&account.goa_id).await {
                Ok(auth) => {
                    match crate::sync::imap::discover_folders(&goa_account.imap_config, &auth).await
                    {
                        Ok(imap_folders) => {
                            let folders: Vec<Folder> =
                                imap_folders.iter().map(imap_to_folder).collect();
                            eprintln!(
                                "  {} folders for {}",
                                folders.len(),
                                account.email_address
                            );
                            // Persist folders — MailEngine emits FoldersChanged
                            self.folders.sync_folders(account, &folders).await?;
                        }
                        Err(e) => {
                            eprintln!(
                                "  IMAP folder discovery failed for {}: {e}",
                                account.email_address
                            );
                        }
                    }
                }
                Err(e) => {
                    eprintln!(
                        "  Failed to get IMAP credentials for {}: {e}",
                        account.email_address
                    );
                }
            }
        }

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
