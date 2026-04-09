//! Email View Pipeline — HTML sanitisation and plain text conversion.
//!
//! Transforms stored body content into safe, displayable HTML for
//! WebKitWebView rendering. This is the render-time counterpart to
//! the sync-time EmailProcessingPipeline.
//!
//! Pure functions — no I/O, no state, easily testable.

use std::collections::HashSet;

/// Sanitise HTML email body for safe rendering in WebKitWebView.
///
/// - Strips JavaScript and dangerous tags
/// - Allows `cid:`, `data:`, `https:`, and `http:` URL schemes
/// - Adds `noopener noreferrer` to links
pub fn sanitise_html(html: &str) -> String {
    let allowed_schemes: HashSet<&str> = ["cid", "data", "https", "http"].into_iter().collect();

    ammonia::Builder::new()
        .url_schemes(allowed_schemes)
        .link_rel(Some("noopener noreferrer"))
        .add_generic_attributes(["style"])
        .clean(html)
        .to_string()
}

/// Convert plain text to simple HTML for WebKitWebView rendering.
///
/// Escapes HTML entities, converts newlines to `<br>`, wraps in a
/// pre-formatted block with system font.
pub fn plain_text_to_html(text: &str) -> String {
    let escaped = ammonia::clean_text(text);
    format!(
        "<html><body><pre style=\"white-space: pre-wrap; word-wrap: break-word; \
         font-family: system-ui, -apple-system, sans-serif; font-size: 14px; \
         margin: 16px;\">{}</pre></body></html>",
        escaped
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_javascript() {
        let html = r#"<p>Hello</p><script>alert('xss')</script><p>World</p>"#;
        let clean = sanitise_html(html);
        assert!(!clean.contains("script"));
        assert!(!clean.contains("alert"));
        assert!(clean.contains("Hello"));
        assert!(clean.contains("World"));
    }

    #[test]
    fn preserves_remote_images() {
        let html = r#"<img src="https://example.com/image.png"><p>Text</p>"#;
        let clean = sanitise_html(html);
        assert!(clean.contains("https://example.com/image.png"));
        assert!(clean.contains("Text"));
    }

    #[test]
    fn allows_cid_images() {
        let html = r#"<img src="cid:image001.png@01D1234"><p>Text</p>"#;
        let clean = sanitise_html(html);
        assert!(clean.contains("cid:image001.png"));
    }

    #[test]
    fn allows_data_images() {
        let html = r#"<img src="data:image/png;base64,iVBOR"><p>Text</p>"#;
        let clean = sanitise_html(html);
        assert!(clean.contains("data:image/png"));
    }

    #[test]
    fn adds_link_rel() {
        let html = r#"<a href="https://example.com">Link</a>"#;
        let clean = sanitise_html(html);
        assert!(clean.contains("noopener"));
        assert!(clean.contains("noreferrer"));
    }

    #[test]
    fn plain_text_escapes_html() {
        let text = "Hello <world> & friends";
        let html = plain_text_to_html(text);
        assert!(html.contains("&lt;world&gt;"));
        assert!(html.contains("&amp;"));
        assert!(!html.contains("<world>"));
    }
}
