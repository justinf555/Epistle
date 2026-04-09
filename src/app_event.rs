use crate::engine::traits::accounts::Account;
use crate::engine::traits::folders::Folder;
use crate::engine::traits::messages::{Message, MessageBody};

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

    /// New messages added to a folder.
    MessagesAdded {
        account_id: String,
        folder_name: String,
        messages: Vec<Message>,
    },

    /// Existing messages updated (flags, preview, etc.) in a folder.
    MessagesUpdated {
        account_id: String,
        folder_name: String,
        messages: Vec<Message>,
    },

    /// Messages removed from a folder.
    MessagesRemoved {
        account_id: String,
        folder_name: String,
        uids: Vec<u32>,
    },

    /// UI requests the body for a specific message.
    MessageBodyRequested {
        account_id: String,
        folder_name: String,
        uid: u32,
    },

    /// Body content fetched, parsed, and cached for a message.
    MessageBodyFetched {
        account_id: String,
        folder_name: String,
        uid: u32,
        body: MessageBody,
    },

    /// User selected a folder in the sidebar.
    FolderSelected {
        account_id: String,
        folder_name: String,
    },

    /// User selected a message in the message list.
    MessageSelected {
        account_id: String,
        folder_name: String,
        uid: u32,
        subject: Option<String>,
        sender: Option<String>,
        date: Option<String>,
    },
}
