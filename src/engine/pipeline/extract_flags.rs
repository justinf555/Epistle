//! Maps IMAP flag strings to boolean fields. Always succeeds.

use crate::engine::traits::messages::Message;

use super::types::{ProcessingStep, RawEmail};

pub struct ExtractFlags;

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
