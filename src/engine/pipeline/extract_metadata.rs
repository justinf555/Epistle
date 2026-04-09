//! Decodes envelope fields: subject, from, to, cc, date, message-id, in-reply-to.
//! Always succeeds — envelope data is always present in Phase 1.

use crate::engine::traits::messages::Message;

use super::helpers::{decode_bytes, format_address, strip_angle_brackets};
use super::types::{ProcessingStep, RawEmail};

pub struct ExtractMetadata;

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
