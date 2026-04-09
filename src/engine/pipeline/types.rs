//! Raw input types from the sync layer and the processing step trait.

use crate::engine::traits::messages::Message;

// ── Raw input from sync layer ───────────────────────────────────────────────

/// Raw data from IMAP FETCH, before any processing.
/// All fields are `Option` to represent partial fetches.
#[derive(Debug, Clone)]
pub struct RawEmail {
    pub uid: u32,
    pub flags: Vec<String>,

    // Phase 1: from ENVELOPE
    pub subject: Option<Vec<u8>>,
    pub from: Option<Vec<RawAddress>>,
    pub to: Option<Vec<RawAddress>>,
    pub cc: Option<Vec<RawAddress>>,
    pub date: Option<Vec<u8>>,
    pub message_id: Option<Vec<u8>>,
    pub in_reply_to: Option<Vec<u8>>,

    // Phase 1: INTERNALDATE (server-received time, used for sorting)
    pub internal_date: Option<String>,

    // Phase 1: from BODYSTRUCTURE
    pub has_attachments: Option<bool>,

    // Phase 2: body content (not used in Phase 1)
    pub body_text: Option<String>,
}

/// A raw IMAP address (name, mailbox, host as bytes).
#[derive(Debug, Clone)]
pub struct RawAddress {
    pub name: Option<Vec<u8>>,
    pub mailbox: Option<Vec<u8>>,
    pub host: Option<Vec<u8>>,
}

// ── Processing step trait ───────────────────────────────────────────────────

/// A single processing step in the email pipeline.
///
/// Each step inspects what data is available in `RawEmail` and populates
/// corresponding fields on `Message`. Steps MUST be idempotent.
pub trait ProcessingStep: Send + Sync {
    fn process(&self, message: &mut Message, raw: &RawEmail);
}
