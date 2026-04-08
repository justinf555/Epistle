use async_trait::async_trait;

// ── Domain Types ────────────────────────────────────────────────────────────
// These use only std types — no GOA, IMAP, or engine imports.

/// An email account as known to Epistle.
#[derive(Debug, Clone)]
pub struct Account {
    /// GOA Account.Id — stable identifier, primary key in local `accounts` table.
    pub goa_id: String,
    /// Provider identifier: "google", "ms_graph", "imap_smtp", etc.
    pub provider_type: String,
    /// Human-readable provider name for UI display.
    pub provider_name: String,
    /// The account's email address.
    pub email_address: String,
    /// Full name associated with the account (may be absent).
    pub display_name: Option<String>,
    /// IMAP server host.
    pub imap_host: String,
    /// IMAP server port.
    pub imap_port: u16,
    /// IMAP TLS mode: "implicit", "starttls", or "none".
    pub imap_tls_mode: String,
    /// SMTP server host (absent if SMTP not supported).
    pub smtp_host: Option<String>,
    /// SMTP server port.
    pub smtp_port: Option<u16>,
    /// SMTP TLS mode: "implicit", "starttls", or "none".
    pub smtp_tls_mode: Option<String>,
    /// Whether the account needs re-authentication in GNOME Settings.
    pub attention_needed: bool,
}

// ── Trait ────────────────────────────────────────────────────────────────────

/// Account persistence — domain-pure storage.
///
/// Every write emits events via the implementation's `EventSender`.
/// Knows nothing about GOA, IMAP, or any protocol — the SyncEngine
/// converts protocol types to `Account` before calling these methods.
#[async_trait]
pub trait MailAccounts: Send + Sync {
    /// Persist accounts into local storage.
    /// Emits `AccountsChanged` after persisting.
    async fn sync_accounts(&self, accounts: &[Account]) -> anyhow::Result<()>;

    /// Read all active accounts from local storage.
    async fn list_accounts(&self) -> anyhow::Result<Vec<Account>>;
}
