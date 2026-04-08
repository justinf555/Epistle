use async_trait::async_trait;

use super::accounts::Account;

// ── Domain Types ────────────────────────────────────────────────────────────

/// A mailbox folder as known to Epistle.
#[derive(Debug, Clone)]
pub struct Folder {
    /// Full IMAP mailbox name (e.g. "INBOX", "[Gmail]/Sent Mail").
    pub name: String,
    /// IMAP hierarchy delimiter (e.g. "/" or ".").
    pub delimiter: Option<String>,
    /// RFC 6154 special-use role: "inbox", "sent", "drafts", "archive", "trash", "junk".
    pub role: Option<String>,
}

// ── Trait ────────────────────────────────────────────────────────────────────

/// Folder discovery and persistence.
///
/// Wraps IMAP folder LIST results into local storage. Every write emits
/// events via the implementation's `EventSender`.
#[async_trait]
pub trait MailFolders: Send + Sync {
    /// Store folders discovered from IMAP LIST for an account.
    /// Emits `FoldersChanged` after persisting.
    async fn sync_folders(&self, account: &Account, folders: &[Folder]) -> anyhow::Result<()>;

    /// Read all folders for an account, ordered by role then name.
    async fn list_folders(&self, account_id: &str) -> anyhow::Result<Vec<Folder>>;
}
