use crate::engine::db::accounts::AccountRow;
use crate::engine::db::folders::FolderRow;

/// Application-layer event type.
///
/// Delivered to all [`EventBus`](crate::event_bus::EventBus) subscribers on the
/// GTK main thread. Backend tasks emit events; UI components subscribe and react.
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// GOA account discovery completed — sidebar should show account sections.
    AccountsChanged { accounts: Vec<AccountRow> },

    /// IMAP folder discovery completed for one account.
    FoldersChanged {
        account_id: String,
        email_address: String,
        folders: Vec<FolderRow>,
    },
}
