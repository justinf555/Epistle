//! Extracts first ~200 chars of text body for message list preview.
//! No-op if body has not been fetched yet (Phase 1).

use crate::engine::traits::messages::Message;

use super::types::{ProcessingStep, RawEmail};

pub struct ExtractPreview;

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
