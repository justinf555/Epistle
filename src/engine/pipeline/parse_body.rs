//! Parse raw RFC 5322 message bytes and extract body parts.
//!
//! Uses `mail-parser` for MIME tree traversal, multipart handling,
//! charset decoding, and content transfer decoding. Inline images
//! referenced by `cid:` URIs are resolved to `data:` URIs at parse
//! time so they render without a custom URI scheme handler.

use base64::prelude::*;
use mail_parser::MimeHeaders;

use crate::engine::traits::messages::MessageBody;

/// Parse raw RFC 5322 message bytes and extract text/html body parts.
///
/// Any inline MIME parts with a Content-ID header have their `cid:`
/// references in the HTML body replaced with base64 `data:` URIs.
pub fn parse_mime_body(raw: &[u8]) -> MessageBody {
    let message = mail_parser::MessageParser::default().parse(raw);
    match message {
        Some(msg) => {
            let mut body_html = msg.body_html(0).map(|s| s.into_owned());
            let body_text = msg.body_text(0).map(|s| s.into_owned());

            // Replace cid: references with data: URIs for inline images.
            if let Some(ref mut html) = body_html {
                resolve_cid_images(html, &msg);
            }

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

/// Walk all MIME parts, find those with a Content-ID header, and replace
/// matching `cid:` references in the HTML with `data:` URIs.
fn resolve_cid_images(html: &mut String, msg: &mail_parser::Message<'_>) {
    let mut resolved = 0u32;

    for part in &msg.parts {
        let Some(cid) = part.content_id() else {
            continue;
        };
        let content = part.contents();
        if content.is_empty() {
            continue;
        }

        let mime_type = part
            .content_type()
            .map(|ct| {
                if let Some(ref sub) = ct.c_subtype {
                    format!("{}/{}", ct.c_type, sub)
                } else {
                    ct.c_type.to_string()
                }
            })
            .unwrap_or_else(|| "application/octet-stream".to_string());

        let b64 = BASE64_STANDARD.encode(content);
        let data_uri = format!("data:{mime_type};base64,{b64}");

        // Content-ID may be stored with or without angle brackets;
        // cid: references in HTML omit them. Strip brackets for matching.
        let cid_bare = cid.trim_start_matches('<').trim_end_matches('>');
        let target = format!("cid:{cid_bare}");
        if html.contains(&target) {
            *html = html.replace(&target, &data_uri);
            resolved += 1;
            tracing::debug!(cid = cid_bare, mime_type, bytes = content.len(), "Resolved CID image");
        } else {
            tracing::debug!(cid = cid_bare, "CID part found but no matching reference in HTML");
        }
    }

    if resolved > 0 {
        tracing::debug!(resolved, "CID image resolution complete");
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

    #[test]
    fn resolves_cid_images_to_data_uris() {
        // Construct a multipart/related message with an inline image.
        let raw = b"From: alice@example.com\r\n\
Subject: CID test\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/related; boundary=\"boundary42\"\r\n\
\r\n\
--boundary42\r\n\
Content-Type: text/html\r\n\
\r\n\
<html><body><img src=\"cid:logo@example.com\"></body></html>\r\n\
--boundary42\r\n\
Content-Type: image/png\r\n\
Content-ID: <logo@example.com>\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
iVBORw0KGgo=\r\n\
--boundary42--\r\n";

        let body = parse_mime_body(raw);
        let html = body.body_html.expect("should have HTML body");
        assert!(
            !html.contains("cid:logo@example.com"),
            "cid: reference should be replaced"
        );
        assert!(
            html.contains("data:image/png;base64,"),
            "should contain data: URI"
        );
    }

    #[test]
    fn no_cid_parts_leaves_html_unchanged() {
        let raw = b"From: alice@example.com\r\n\
Subject: No CID\r\n\
Content-Type: text/html\r\n\
\r\n\
<html><body><p>No images</p></body></html>";

        let body = parse_mime_body(raw);
        let html = body.body_html.expect("should have HTML body");
        assert!(html.contains("No images"));
    }
}
