use async_trait::async_trait;

// ── Domain Types ────────────────────────────────────────────────────────────

/// A processed email message as known to Epistle.
///
/// Fields are populated incrementally: Phase 1 (headers/flags) fills metadata,
/// Phase 2 (body fetch) writes the .eml file to disk.
#[derive(Debug, Clone)]
pub struct Message {
    /// Internal stable identifier (UUID v4).
    pub uuid: String,
    /// IMAP UID within the folder.
    pub uid: u32,
    /// Account this message belongs to.
    pub account_id: String,
    /// Folder this message lives in (e.g. "INBOX").
    pub folder_name: String,

    // ── Metadata (Phase 1 — from envelope) ──────────────────────────────
    /// RFC 2822 Message-ID header (angle brackets stripped).
    pub message_id: Option<String>,
    /// Decoded subject line.
    pub subject: Option<String>,
    /// Decoded sender (display name + address).
    pub sender: Option<String>,
    /// Decoded To addresses.
    pub to_addresses: Vec<String>,
    /// Decoded Cc addresses.
    pub cc_addresses: Vec<String>,
    /// Parsed date in ISO 8601 / RFC 3339 format (from sender's Date header).
    pub date: Option<String>,
    /// Server-received date (IMAP INTERNALDATE) — used for sorting.
    pub internal_date: Option<String>,
    /// In-Reply-To header (for threading).
    pub in_reply_to: Option<String>,
    /// References header (space-separated Message-IDs, for threading).
    pub references: Vec<String>,

    // ── Flags (Phase 1 — always present) ────────────────────────────────
    pub is_read: bool,
    pub is_flagged: bool,
    pub is_answered: bool,
    pub is_draft: bool,

    // ── Body-dependent (Phase 2 — None until body is fetched) ───────────
    /// First ~200 chars of text body for message list preview.
    pub preview: Option<String>,
    /// Content type: "text/plain", "text/html", "multipart/alternative".
    pub content_type: Option<String>,
    /// Whether the message has attachments (from BODYSTRUCTURE or body).
    pub has_attachments: bool,
}

/// Extracted body content from a message (parsed from .eml at render time).
#[derive(Debug, Clone)]
pub struct MessageBody {
    pub body_text: Option<String>,
    pub body_html: Option<String>,
}

// ── Trait ────────────────────────────────────────────────────────────────────

/// Message storage and retrieval.
///
/// Receives processed messages from the pipeline, persists them, and
/// emits events via the implementation's `EventSender`.
#[async_trait]
pub trait MailMessages: Send + Sync {
    /// Store messages for a folder. Runs them through the processing pipeline,
    /// persists results, and emits `MessagesChanged` if data changed.
    async fn sync_messages(
        &self,
        account_id: &str,
        folder_name: &str,
        messages: &[Message],
    ) -> anyhow::Result<()>;

    /// Read all messages for a folder, ordered by date descending (newest first).
    async fn list_messages(
        &self,
        account_id: &str,
        folder_name: &str,
    ) -> anyhow::Result<Vec<Message>>;

    /// Read a page of messages for a folder, ordered newest first.
    async fn list_messages_page(
        &self,
        account_id: &str,
        folder_name: &str,
        limit: u32,
        offset: u32,
    ) -> anyhow::Result<Vec<Message>>;

    /// Get the UUID for a message by its IMAP UID.
    async fn get_uuid(
        &self,
        account_id: &str,
        folder_name: &str,
        uid: u32,
    ) -> anyhow::Result<Option<String>>;

    /// Get uuid, uid, and internal_date for messages in a folder since a cutoff date.
    async fn list_messages_since(
        &self,
        account_id: &str,
        folder_name: &str,
        since: &str,
    ) -> anyhow::Result<Vec<(String, u32, Option<String>)>>;

    /// Get all UIDs for a folder (for differential sync).
    async fn list_local_uids(
        &self,
        account_id: &str,
        folder_name: &str,
    ) -> anyhow::Result<std::collections::HashSet<u32>>;

    /// Update flags for a single message by UID. Emits MessagesUpdated if changed.
    async fn update_flags(
        &self,
        account_id: &str,
        folder_name: &str,
        uid: u32,
        is_read: bool,
        is_flagged: bool,
        is_answered: bool,
        is_draft: bool,
    ) -> anyhow::Result<bool>;

    /// Remove messages by UID. Emits MessagesRemoved.
    async fn delete_messages_by_uids(
        &self,
        account_id: &str,
        folder_name: &str,
        uids: &[u32],
    ) -> anyhow::Result<u64>;
}
