use async_trait::async_trait;

// ── Domain Types ────────────────────────────────────────────────────────────
// These use only std types — traits/ does not import from goa/ or engine/.

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
    /// SMTP server host (absent if SMTP not supported).
    pub smtp_host: Option<String>,
    /// SMTP server port.
    pub smtp_port: Option<u16>,
    /// Whether the account needs re-authentication in GNOME Settings.
    pub attention_needed: bool,
}

/// Credentials for authenticating to an IMAP or SMTP server.
#[derive(Debug, Clone)]
pub enum AuthCredential {
    XOAuth2 { token: String },
    Plain { username: String, password: String },
}

/// Result of validating account credentials.
#[derive(Debug, Clone)]
pub enum CredentialStatus {
    /// Credentials are valid; `expires_in_secs` is seconds until expiry (0 if unknown).
    Valid { expires_in_secs: i32 },
    /// Account needs re-authentication in GNOME Settings.
    AttentionNeeded,
}

// ── Trait ────────────────────────────────────────────────────────────────────

/// Account discovery and credential retrieval.
///
/// Wraps the GOA D-Bus integration. The trait boundary enables testing with
/// mock accounts that don't require a live D-Bus session.
#[async_trait]
pub trait MailAccounts: Send + Sync {
    /// Discover all mail-enabled accounts from GOA.
    async fn discover_accounts(&self) -> anyhow::Result<Vec<Account>>;

    /// Get a single account by GOA ID.
    async fn get_account(&self, goa_id: &str) -> anyhow::Result<Option<Account>>;

    /// Get IMAP credentials (token or password via GOA).
    async fn get_imap_credentials(&self, goa_id: &str) -> anyhow::Result<AuthCredential>;

    /// Get SMTP credentials.
    async fn get_smtp_credentials(&self, goa_id: &str) -> anyhow::Result<AuthCredential>;

    /// Check if account needs re-authentication.
    async fn ensure_credentials(&self, goa_id: &str) -> anyhow::Result<CredentialStatus>;
}
