//! Populates `has_attachments` from BODYSTRUCTURE. No-op if not available.

use crate::engine::traits::messages::Message;

use super::types::{ProcessingStep, RawEmail};

pub struct ExtractBodyStructure;

impl ProcessingStep for ExtractBodyStructure {
    fn process(&self, message: &mut Message, raw: &RawEmail) {
        if let Some(has) = raw.has_attachments {
            message.has_attachments = has;
        }
    }
}
