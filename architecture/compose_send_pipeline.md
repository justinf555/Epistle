<!-- markdownlint-disable -->
# Epistle — Compose & Send Pipeline

**Version:** 0.1 (Outline)  
**Status:** To be detailed during Phase 3 implementation  
**Date:** 2026-04-08  
**Companion Documents:** `gnome_mail_architecture.md`, `v1_scope.md`

---

## Approach

This document will be written alongside implementation. Key design decisions (CSS inlining approach, HTML cleanup strategy, contenteditable behaviour) depend on evaluating real tooling and observing what WebKit actually produces. The outline captures known decisions from the architecture doc.

---

## 1. Editor Surface

- WebKitWebView with `contenteditable="true"`
- Two-zone model: editable compose area (top), read-only quoted history (bottom)
- Toolbar actions via `document.execCommand()` and `evaluate_javascript()`
- JavaScript enabled (runs only trusted app code)
- Quoted content sanitised via `ammonia` before injection into read-only zone
- _To be detailed — covers execCommand mapping, custom JS for the editor, plain text mode toggle._

## 2. Chip / Token Input (To, CC, BCC)

- `GtkFlowBox` with `GtkLabel` chips + trailing `GtkEntry`
- Enter / Tab / comma → validate RFC 5322, create chip
- Backspace on empty entry → remove last chip
- Autocomplete via `GtkPopover` + `GtkListBox` from EDS + local contacts table
- _To be detailed — covers EDS integration, contact ranking by use_count, popover positioning._

## 3. HTML Cleanup

- _To be detailed — covers post-processing contenteditable output to normalise messy DOM (nested divs, empty spans, browser-specific markup). Both Evolution and Geary have equivalent cleanup code._

## 4. CSS Inlining

- _To be detailed — covers converting `<style>` blocks to inline `style=` attributes, which is required because Gmail and Outlook strip `<style>` tags. Needs evaluation of available Rust crates or a custom approach._

## 5. Plain Text Fallback

- _To be detailed — covers stripping HTML to generate the text/plain MIME alternative part._

## 6. MIME Assembly

- Structure: `multipart/mixed` (if attachments) wrapping `multipart/alternative` (`text/plain` + `text/html`)
- Headers: `From`, `To`, `CC`, `Date`, `Message-ID`, `References`, `In-Reply-To`
- _To be detailed — covers header generation, attachment encoding, MIME boundary generation._

## 7. Draft Autosave

- `GLib::timeout_add_seconds(30)` on dirty flag
- Atomic write: `{uuid}.draft.eml` → SQLite row with `flag = DRAFT`
- _To be detailed — covers draft recovery on app restart, draft deletion on send._

## 8. Send Flow

```
Validate (To required, Subject warning)
    → Outbox state
    → 5-second AdwToast undo window
    → CSS inlining
    → Plain text fallback generation
    → MIME assembly
    → SMTP submission (XOAUTH2 or PLAIN via GOA credentials)
    → On success: copy to Sent folder
    → On failure: remain in Outbox, retry with exponential backoff
```

- _To be detailed — covers SMTP connection management, interaction with GOA token refresh, error handling, retry logic._

## 9. Reply / Reply All / Forward

- _To be detailed — covers header construction (In-Reply-To, References), quoted content preparation, forward as attachment vs inline._

---

_This document is a living spec. It will be detailed during Phase 3 as the compose and send pipeline is implemented and tested by sending to Gmail, Outlook, and Apple Mail._
