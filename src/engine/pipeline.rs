//! Email Processing Pipeline — visitor pattern over raw IMAP data.
//!
//! Each [`ProcessingStep`] inspects the available data in [`RawEmail`] and
//! populates corresponding fields on the domain [`Message`]. Steps that
//! need data not yet fetched (e.g. body for preview) are no-ops.
//!
//! The same pipeline handles both Phase 1 (envelope+flags) and Phase 2
//! (body content) without branching at the call site.
//!
//! Each step lives in its own module for independent testing and easy
//! extensibility — adding a new step is just a new file + one line here.

mod detect_content_type;
mod extract_body_structure;
mod extract_flags;
mod extract_metadata;
mod extract_preview;
pub mod helpers;
pub mod parse_body;
pub mod sanitise;
pub mod types;

use crate::engine::traits::messages::Message;

pub use types::{ProcessingStep, RawAddress, RawEmail};

use detect_content_type::DetectContentType;
use extract_body_structure::ExtractBodyStructure;
use extract_flags::ExtractFlags;
use extract_metadata::ExtractMetadata;
use extract_preview::ExtractPreview;

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
        tracing::trace!(uid = raw.uid, "Processing message through pipeline");
        for step in &self.steps {
            step.process(message, raw);
        }
        tracing::trace!(
            uid = raw.uid,
            subject = ?message.subject,
            sender = ?message.sender,
            is_read = message.is_read,
            "Pipeline complete"
        );
    }
}

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
            body_text: None,
            body_html: None,
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
            body_text: None,
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
        assert_eq!(
            helpers::format_address(&addr),
            "Alice Smith <alice@example.com>"
        );
    }

    #[test]
    fn format_address_without_name() {
        let addr = RawAddress {
            name: None,
            mailbox: Some(b"alice".to_vec()),
            host: Some(b"example.com".to_vec()),
        };
        assert_eq!(helpers::format_address(&addr), "alice@example.com");
    }
}
