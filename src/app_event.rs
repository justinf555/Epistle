use crate::engine::traits::accounts::Account;
use crate::engine::traits::folders::Folder;
use crate::engine::traits::messages::Message;

/// Application-layer event type.
///
/// Delivered to all [`EventBus`](crate::event_bus::EventBus) subscribers on the
/// GTK main thread. Backend tasks emit events; UI components subscribe and react.
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// Application has started — SyncEngine should begin discovery.
    AppStarted,

    /// Application is shutting down — components should clean up.
    AppShutdown,

    /// Account discovery completed — sidebar should show account sections.
    AccountsChanged { accounts: Vec<Account> },

    /// IMAP folder discovery completed for one account.
    FoldersChanged {
        account_id: String,
        email_address: String,
        folders: Vec<Folder>,
    },

    /// Message sync completed for one folder.
    MessagesChanged {
        account_id: String,
        folder_name: String,
        messages: Vec<Message>,
    },
}
