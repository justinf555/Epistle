//! Parse raw RFC 5322 message bytes and extract body parts.
//!
//! Uses `mail-parser` for MIME tree traversal, multipart handling,
//! charset decoding, and content transfer decoding.

use crate::engine::traits::messages::MessageBody;

/// Parse raw RFC 5322 message bytes and extract text/html body parts.
pub fn parse_mime_body(raw: &[u8]) -> MessageBody {
    let message = mail_parser::MessageParser::default().parse(raw);
    match message {
        Some(msg) => {
            let body_html = msg.body_html(0).map(|s| s.into_owned());
            let body_text = msg.body_text(0).map(|s| s.into_owned());

            tracing::debug!(
                has_html = body_html.is_some(),
                has_text = body_text.is_some(),
                html_len = body_html.as_ref().map(|s| s.len()).unwrap_or(0),
                text_len = body_text.as_ref().map(|s| s.len()).unwrap_or(0),
                "Parsed MIME body"
            );

            MessageBody {
                body_text,
                body_html,
            }
        }
        None => {
            tracing::warn!("Failed to parse MIME message");
            MessageBody {
                body_text: None,
                body_html: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plain_text_message() {
        let raw = b"From: alice@example.com\r\n\
                     Subject: Hello\r\n\
                     Content-Type: text/plain\r\n\
                     \r\n\
                     Hello, this is a test message.";
        let body = parse_mime_body(raw);
        assert!(body.body_text.is_some());
        assert!(body.body_text.unwrap().contains("Hello, this is a test message."));
        // mail-parser may generate an HTML representation of plain text
    }

    #[test]
    fn parse_html_message() {
        let raw = b"From: alice@example.com\r\n\
                     Subject: Hello\r\n\
                     Content-Type: text/html\r\n\
                     \r\n\
                     <html><body><p>Hello!</p></body></html>";
        let body = parse_mime_body(raw);
        assert!(body.body_html.is_some());
        assert!(body.body_html.unwrap().contains("<p>Hello!</p>"));
    }

    #[test]
    fn parse_invalid_message() {
        let body = parse_mime_body(b"not a valid email");
        // mail-parser may still return something or nothing
        // Just ensure we don't panic
        let _ = body;
    }
}
