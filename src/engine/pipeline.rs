//! Email Processing Pipeline — visitor pattern over raw IMAP data.
//!
//! Each [`ProcessingStep`] inspects the available data in [`RawEmail`] and
//! populates corresponding fields on the domain [`Message`]. Steps that
//! need data not yet fetched (e.g. body for preview) are no-ops.
//!
//! The same pipeline handles both Phase 1 (envelope+flags) and Phase 2
//! (body content) without branching at the call site.

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

// ── Pipeline ────────────────────────────────────────────────────────────────

/// The email processing pipeline. Runs all steps in order.
pub struct EmailPipeline {
    steps: Vec<Box<dyn ProcessingStep>>,
}

impl EmailPipeline {
    pub fn new() -> Self {
        Self {
            steps: vec![
                Box::new(ExtractMetadata),
                Box::new(ExtractFlags),
                Box::new(ExtractBodyStructure),
                Box::new(ExtractPreview),
                Box::new(DetectContentType),
            ],
        }
    }

    /// Run all steps against a message. Each step skips gracefully if its
    /// required data is missing from `raw`.
    pub fn process(&self, message: &mut Message, raw: &RawEmail) {
        for step in &self.steps {
            step.process(message, raw);
        }
    }
}

// ── Step: Extract Metadata ──────────────────────────────────────────────────

/// Decodes envelope fields: subject, from, to, cc, date, message-id, in-reply-to.
/// Always succeeds — envelope data is always present in Phase 1.
struct ExtractMetadata;

impl ProcessingStep for ExtractMetadata {
    fn process(&self, message: &mut Message, raw: &RawEmail) {
        message.uid = raw.uid;

        if let Some(ref subject) = raw.subject {
            message.subject = Some(decode_bytes(subject));
        }

        if let Some(ref from) = raw.from {
            if let Some(addr) = from.first() {
                message.sender = Some(format_address(addr));
            }
        }

        if let Some(ref to) = raw.to {
            message.to_addresses = to.iter().map(format_address).collect();
        }

        if let Some(ref cc) = raw.cc {
            message.cc_addresses = cc.iter().map(format_address).collect();
        }

        if let Some(ref date) = raw.date {
            message.date = Some(decode_bytes(date));
        }

        if let Some(ref msg_id) = raw.message_id {
            message.message_id = Some(strip_angle_brackets(&decode_bytes(msg_id)));
        }

        if let Some(ref irt) = raw.in_reply_to {
            message.in_reply_to = Some(strip_angle_brackets(&decode_bytes(irt)));
        }
    }
}

// ── Step: Extract Flags ─────────────────────────────────────────────────────

/// Maps IMAP flag strings to boolean fields. Always succeeds.
struct ExtractFlags;

impl ProcessingStep for ExtractFlags {
    fn process(&self, message: &mut Message, raw: &RawEmail) {
        for flag in &raw.flags {
            match flag.as_str() {
                "\\Seen" => message.is_read = true,
                "\\Flagged" => message.is_flagged = true,
                "\\Answered" => message.is_answered = true,
                "\\Draft" => message.is_draft = true,
                _ => {}
            }
        }
    }
}

// ── Step: Extract Body Structure ────────────────────────────────────────────

/// Populates `has_attachments` from BODYSTRUCTURE. No-op if not available.
struct ExtractBodyStructure;

impl ProcessingStep for ExtractBodyStructure {
    fn process(&self, message: &mut Message, raw: &RawEmail) {
        if let Some(has) = raw.has_attachments {
            message.has_attachments = has;
        }
    }
}

// ── Step: Extract Preview ───────────────────────────────────────────────────

/// Extracts first ~200 chars of text body for message list preview.
/// No-op if body has not been fetched yet (Phase 1).
struct ExtractPreview;

impl ProcessingStep for ExtractPreview {
    fn process(&self, message: &mut Message, raw: &RawEmail) {
        if let Some(ref text) = raw.body_text {
            let preview: String = text
                .chars()
                .filter(|c| !c.is_control() || *c == ' ')
                .take(200)
                .collect();
            message.preview = Some(preview.trim().to_string());
        }
    }
}

// ── Step: Detect Content Type ───────────────────────────────────────────────

/// Determines content type from body. No-op if body not yet fetched.
struct DetectContentType;

impl ProcessingStep for DetectContentType {
    fn process(&self, message: &mut Message, raw: &RawEmail) {
        if let Some(ref text) = raw.body_text {
            // Simple heuristic for Phase 1 — will be refined when we parse MIME
            if text.contains('<') && text.contains('>') && text.contains("</") {
                message.content_type = Some("text/html".to_string());
            } else {
                message.content_type = Some("text/plain".to_string());
            }
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Decode bytes to a UTF-8 string, replacing invalid sequences.
fn decode_bytes(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Format a raw IMAP address as "Display Name <mailbox@host>" or "mailbox@host".
fn format_address(addr: &RawAddress) -> String {
    let email = match (&addr.mailbox, &addr.host) {
        (Some(mailbox), Some(host)) => {
            format!("{}@{}", decode_bytes(mailbox), decode_bytes(host))
        }
        (Some(mailbox), None) => decode_bytes(mailbox),
        _ => return String::new(),
    };

    match &addr.name {
        Some(name) => {
            let decoded = decode_bytes(name);
            if decoded.is_empty() {
                email
            } else {
                format!("{} <{}>", decoded, email)
            }
        }
        None => email,
    }
}

/// Strip angle brackets from Message-ID values: "<foo@bar>" → "foo@bar".
fn strip_angle_brackets(s: &str) -> String {
    s.trim_start_matches('<').trim_end_matches('>').to_string()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_message() -> Message {
        Message {
            uid: 0,
            account_id: String::new(),
            folder_name: String::new(),
            message_id: None,
            subject: None,
            sender: None,
            to_addresses: vec![],
            cc_addresses: vec![],
            date: None,
            in_reply_to: None,
            references: vec![],
            is_read: false,
            is_flagged: false,
            is_answered: false,
            is_draft: false,
            preview: None,
            content_type: None,
            has_attachments: false,
        }
    }

    #[test]
    fn extract_metadata_from_envelope() {
        let pipeline = EmailPipeline::new();
        let raw = RawEmail {
            uid: 42,
            flags: vec![],
            subject: Some(b"Hello World".to_vec()),
            from: Some(vec![RawAddress {
                name: Some(b"Alice".to_vec()),
                mailbox: Some(b"alice".to_vec()),
                host: Some(b"example.com".to_vec()),
            }]),
            to: Some(vec![RawAddress {
                name: None,
                mailbox: Some(b"bob".to_vec()),
                host: Some(b"example.com".to_vec()),
            }]),
            cc: None,
            date: Some(b"Mon, 9 Apr 2026 10:00:00 +0000".to_vec()),
            message_id: Some(b"<msg001@example.com>".to_vec()),
            in_reply_to: None,
            has_attachments: None,
            body_text: None,
        };

        let mut msg = empty_message();
        pipeline.process(&mut msg, &raw);

        assert_eq!(msg.uid, 42);
        assert_eq!(msg.subject.as_deref(), Some("Hello World"));
        assert_eq!(msg.sender.as_deref(), Some("Alice <alice@example.com>"));
        assert_eq!(msg.to_addresses, vec!["bob@example.com"]);
        assert_eq!(msg.message_id.as_deref(), Some("msg001@example.com"));
        assert_eq!(msg.date.as_deref(), Some("Mon, 9 Apr 2026 10:00:00 +0000"));
    }

    #[test]
    fn extract_flags() {
        let pipeline = EmailPipeline::new();
        let raw = RawEmail {
            uid: 1,
            flags: vec![
                "\\Seen".to_string(),
                "\\Flagged".to_string(),
                "\\Answered".to_string(),
            ],
            subject: None,
            from: None,
            to: None,
            cc: None,
            date: None,
            message_id: None,
            in_reply_to: None,
            has_attachments: None,
            body_text: None,
        };

        let mut msg = empty_message();
        pipeline.process(&mut msg, &raw);

        assert!(msg.is_read);
        assert!(msg.is_flagged);
        assert!(msg.is_answered);
        assert!(!msg.is_draft);
    }

    #[test]
    fn body_dependent_steps_skip_when_no_body() {
        let pipeline = EmailPipeline::new();
        let raw = RawEmail {
            uid: 1,
            flags: vec![],
            subject: Some(b"Test".to_vec()),
            from: None,
            to: None,
            cc: None,
            date: None,
            message_id: None,
            in_reply_to: None,
            has_attachments: None,
            body_text: None, // no body — Phase 1
        };

        let mut msg = empty_message();
        pipeline.process(&mut msg, &raw);

        assert!(msg.preview.is_none());
        assert!(msg.content_type.is_none());
    }

    #[test]
    fn preview_extracted_when_body_available() {
        let pipeline = EmailPipeline::new();
        let raw = RawEmail {
            uid: 1,
            flags: vec![],
            subject: None,
            from: None,
            to: None,
            cc: None,
            date: None,
            message_id: None,
            in_reply_to: None,
            has_attachments: None,
            body_text: Some("Hello, this is the body of the email.".to_string()),
        };

        let mut msg = empty_message();
        pipeline.process(&mut msg, &raw);

        assert_eq!(
            msg.preview.as_deref(),
            Some("Hello, this is the body of the email.")
        );
        assert_eq!(msg.content_type.as_deref(), Some("text/plain"));
    }

    #[test]
    fn format_address_with_name() {
        let addr = RawAddress {
            name: Some(b"Alice Smith".to_vec()),
            mailbox: Some(b"alice".to_vec()),
            host: Some(b"example.com".to_vec()),
        };
        assert_eq!(format_address(&addr), "Alice Smith <alice@example.com>");
    }

    #[test]
    fn format_address_without_name() {
        let addr = RawAddress {
            name: None,
            mailbox: Some(b"alice".to_vec()),
            host: Some(b"example.com".to_vec()),
        };
        assert_eq!(format_address(&addr), "alice@example.com");
    }
}
