use std::sync::Arc;

use gtk::glib;

use crate::event_bus::{EventBus, EventSender};

pub mod db;
pub mod traits;

pub mod accounts;
pub mod folders;

use accounts::MailAccountsImpl;
use db::Database;
use folders::MailFoldersImpl;
use traits::accounts::MailAccounts;
use traits::folders::MailFolders;

/// Central owner of the mail engine — database, event bus, and domain trait implementations.
///
/// Created once at startup in `main.rs`. Components receive trait objects and
/// EventBus references from here. The engine itself has no protocol awareness
/// (no GOA, IMAP, etc.).
pub struct MailEngine {
    bus: EventBus,
    sender: EventSender,
    accounts: Arc<dyn MailAccounts>,
    folders: Arc<dyn MailFolders>,
}

impl std::fmt::Debug for MailEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MailEngine").finish_non_exhaustive()
    }
}

impl MailEngine {
    /// Open the database, run migrations, and create trait implementations.
    pub async fn open() -> anyhow::Result<Self> {
        let db_path = glib::user_data_dir().join("epistle").join("mail.db");
        let db = Database::open(&db_path).await?;

        let bus = EventBus::new();
        let sender = bus.sender();

        let accounts = Arc::new(MailAccountsImpl::new(db.clone(), sender.clone()));
        let folders = Arc::new(MailFoldersImpl::new(db, sender.clone()));

        Ok(Self {
            bus,
            sender,
            accounts,
            folders,
        })
    }

    /// The event bus — UI components subscribe here.
    pub fn bus(&self) -> &EventBus {
        &self.bus
    }

    /// A cloneable, Send event sender — for emitting lifecycle events.
    pub fn sender(&self) -> EventSender {
        self.sender.clone()
    }

    /// Account storage trait object.
    pub fn accounts(&self) -> Arc<dyn MailAccounts> {
        Arc::clone(&self.accounts)
    }

    /// Folder storage trait object.
    pub fn folders(&self) -> Arc<dyn MailFolders> {
        Arc::clone(&self.folders)
    }
}
