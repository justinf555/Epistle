//! Shared helpers for pipeline processing steps.

use super::types::RawAddress;

/// Decode bytes to a UTF-8 string, replacing invalid sequences.
pub fn decode_bytes(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Format a raw IMAP address as "Display Name <mailbox@host>" or "mailbox@host".
pub fn format_address(addr: &RawAddress) -> String {
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
pub fn strip_angle_brackets(s: &str) -> String {
    s.trim_start_matches('<').trim_end_matches('>').to_string()
}
