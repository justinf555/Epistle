use async_trait::async_trait;

use crate::app_event::AppEvent;
use crate::engine::db::Database;
use crate::engine::traits::accounts::{Account, MailAccounts};
use crate::event_bus::EventSender;

/// Concrete implementation of [`MailAccounts`] backed by SQLite + EventSender.
///
/// Domain-pure — no GOA, IMAP, or protocol dependencies.
pub struct MailAccountsImpl {
    db: Database,
    sender: EventSender,
}

impl MailAccountsImpl {
    pub fn new(db: Database, sender: EventSender) -> Self {
        Self { db, sender }
    }
}

#[async_trait]
impl MailAccounts for MailAccountsImpl {
    async fn sync_accounts(&self, accounts: &[Account]) -> anyhow::Result<()> {
        for account in accounts {
            self.db
                .upsert_account_fields(
                    &account.goa_id,
                    &account.provider_type,
                    &account.email_address,
                    account.display_name.as_deref(),
                    &account.imap_host,
                    account.imap_port,
                    &account.imap_tls_mode,
                    account.smtp_host.as_deref(),
                    account.smtp_port,
                    account.smtp_tls_mode.as_deref(),
                )
                .await?;
        }

        self.sender.send(AppEvent::AccountsChanged {
            accounts: accounts.to_vec(),
        });

        Ok(())
    }

    async fn list_accounts(&self) -> anyhow::Result<Vec<Account>> {
        let rows = self.db.list_active_accounts().await?;
        Ok(rows
            .into_iter()
            .map(|row| Account {
                goa_id: row.goa_id,
                provider_type: row.provider_type,
                provider_name: String::new(),
                email_address: row.email_address,
                display_name: row.display_name,
                imap_host: String::new(),
                imap_port: 0,
                imap_tls_mode: String::new(),
                smtp_host: None,
                smtp_port: None,
                smtp_tls_mode: None,
                attention_needed: false,
            })
            .collect())
    }
}
