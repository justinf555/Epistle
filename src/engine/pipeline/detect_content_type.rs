//! Determines content type from body. No-op if body not yet fetched.

use crate::engine::traits::messages::Message;

use super::types::{ProcessingStep, RawEmail};

pub struct DetectContentType;

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
