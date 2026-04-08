use async_trait::async_trait;

use crate::app_event::AppEvent;
use crate::engine::db::folders::FolderFields;
use crate::engine::db::Database;
use crate::engine::traits::accounts::Account;
use crate::engine::traits::folders::{Folder, MailFolders};
use crate::event_bus::EventSender;

/// Concrete implementation of [`MailFolders`] backed by SQLite + EventSender.
///
/// Domain-pure — no IMAP or protocol dependencies.
pub struct MailFoldersImpl {
    db: Database,
    sender: EventSender,
}

impl MailFoldersImpl {
    pub fn new(db: Database, sender: EventSender) -> Self {
        Self { db, sender }
    }
}

#[async_trait]
impl MailFolders for MailFoldersImpl {
    async fn sync_folders(&self, account: &Account, folders: &[Folder]) -> anyhow::Result<()> {
        let fields: Vec<FolderFields<'_>> = folders
            .iter()
            .map(|f| FolderFields {
                name: &f.name,
                delimiter: f.delimiter.as_deref(),
                role: f.role.as_deref(),
            })
            .collect();

        let changed = self.db.bulk_upsert_folders(&account.goa_id, &fields).await?;

        if changed {
            let folder_rows = self.db.list_folders(&account.goa_id).await?;
            let result_folders: Vec<Folder> = folder_rows.into_iter().map(row_to_folder).collect();

            self.sender.send(AppEvent::FoldersChanged {
                account_id: account.goa_id.clone(),
                email_address: account.email_address.clone(),
                folders: result_folders,
            });
        }

        Ok(())
    }

    async fn list_folders(&self, account_id: &str) -> anyhow::Result<Vec<Folder>> {
        let rows = self.db.list_folders(account_id).await?;
        Ok(rows.into_iter().map(row_to_folder).collect())
    }
}

fn row_to_folder(row: crate::engine::db::folders::FolderRow) -> Folder {
    Folder {
        name: row.name,
        delimiter: row.delimiter,
        role: row.role,
    }
}
