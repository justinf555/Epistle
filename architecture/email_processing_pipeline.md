<!-- markdownlint-disable -->
# Epistle — Email Processing Pipeline

**Version:** 0.2  
**Status:** Architecture defined, implementation in progress  
**Date:** 2026-04-09  
**Companion Documents:** `gnome_mail_architecture.md`, `v1_scope.md`, `message-pipeline.drawio`

---

## Approach

The pipeline uses a **visitor pattern** — a sequence of independent processing steps that each inspect the available data and either do their work or silently skip. This means the same pipeline handles both partial messages (headers-only from Phase 1 sync) and full messages (headers + body from Phase 2 background fill), with no branching at the call site.

Real design decisions around sanitisation rules and MIME edge cases will be discovered by running real emails through the pipeline.

---

## 1. Pipeline Architecture

### 1.1 Core Trait

```rust
/// A single processing step in the email pipeline.
/// Each step inspects what data is available in RawEmail and
/// populates the corresponding fields on MailMessage.
/// Steps MUST be idempotent — re-running with more data fills gaps,
/// never overwrites valid data.
trait ProcessingStep: Send + Sync {
    fn process(&self, message: &mut MailMessage, raw: &RawEmail);
}
```

### 1.2 Input: RawEmail

The sync layer produces `RawEmail` — a container for whatever IMAP returned. Fields are all `Option` to represent partial fetches.

```rust
/// Raw data from IMAP FETCH, before any processing.
/// All fields are Option — partial fetches leave body/structure as None.
pub struct RawEmail {
    pub uid: u32,
    pub flags: Vec<String>,

    // Phase 1: always present (from ENVELOPE/FETCH)
    pub envelope: Option<ImapEnvelope>,   // subject, from, to, cc, date, message-id, in-reply-to

    // Phase 1: present if BODYSTRUCTURE was fetched
    pub body_structure: Option<BodyStructure>,

    // Phase 2: present when body parts are fetched
    pub raw_headers: Option<Vec<u8>>,     // full RFC 5322 headers
    pub raw_body: Option<Vec<u8>>,        // full message body (text parts only, not attachments)
}
```

### 1.3 Output: MailMessage

The engine's domain type. Partially populated after Phase 1, fully populated after Phase 2.

```rust
/// Clean, processed email message — the engine's domain type.
/// Fields populated incrementally as more data becomes available.
pub struct MailMessage {
    // Identity (always set)
    pub uid: u32,
    pub account_id: String,
    pub folder_name: String,

    // Metadata (Phase 1 — from envelope)
    pub message_id: Option<String>,
    pub subject: Option<String>,
    pub from: Option<String>,           // decoded display name + address
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub date: Option<chrono::DateTime<chrono::Utc>>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,

    // Flags (Phase 1 — always present)
    pub is_read: bool,
    pub is_flagged: bool,
    pub is_answered: bool,
    pub is_draft: bool,

    // Body-dependent (Phase 2 — None until body is fetched)
    pub preview: Option<String>,         // first ~200 chars of text body
    pub content_type: Option<String>,    // "text/plain", "text/html", "multipart/alternative"
    pub has_attachments: bool,

    // Attachment metadata (from BODYSTRUCTURE or body parsing)
    pub attachments: Vec<AttachmentMeta>,

    // Threading (populated by threading module after pipeline)
    pub thread_id: Option<String>,
}

pub struct AttachmentMeta {
    pub filename: Option<String>,
    pub mime_type: String,
    pub size: u64,
    pub content_disposition: String,     // "attachment" or "inline"
}
```

### 1.4 Processing Steps (Visitors)

Steps run in order. Each step checks whether its required data is present in `RawEmail` and either processes or returns immediately.

| Step | Required Data | Phase | Behaviour |
|---|---|---|---|
| `ExtractMetadata` | `envelope` | 1 | Decodes MIME-encoded subject, parses addresses, normalises date. Always succeeds — envelope is always present. |
| `ExtractFlags` | `flags` | 1 | Maps IMAP flag strings (`\Seen`, `\Flagged`, etc.) to boolean fields. Always succeeds. |
| `ExtractBodyStructure` | `body_structure` | 1 | Populates `has_attachments` and `attachments` metadata from BODYSTRUCTURE. No-op if not fetched. |
| `ExtractPreview` | `raw_body` | 2 | Parses body, extracts first ~200 chars of text content. No-op if body not yet fetched. |
| `DetectContentType` | `raw_body` | 2 | Determines whether message is plain text, HTML, or multipart. No-op if body not yet fetched. |

> **Note:** HTML sanitisation (`ammonia`) is **not** a pipeline step. It is a rendering concern handled in the UI layer when displaying the email in a WebKitWebView. The pipeline stores the raw body as-is.

### 1.5 Pipeline Execution

```rust
pub struct EmailPipeline {
    steps: Vec<Box<dyn ProcessingStep>>,
}

impl EmailPipeline {
    pub fn new() -> Self {
        Self {
            steps: vec![
                Box::new(ExtractMetadata),
                Box::new(ExtractFlags),
                Box::new(ExtractBodyStructure),
                Box::new(ExtractPreview),
                Box::new(DetectContentType),
            ],
        }
    }

    /// Run all steps. Each step inspects available data and skips if needed.
    pub fn process(&self, message: &mut MailMessage, raw: &RawEmail) {
        for step in &self.steps {
            step.process(message, raw);
        }
    }
}
```

### 1.6 Two-Phase Flow

```
Phase 1 (initial sync — fast, headers only):
  IMAP FETCH (ENVELOPE FLAGS UID BODYSTRUCTURE)
      → RawEmail { envelope: Some, flags: [...], body_structure: Some, raw_body: None }
      → Pipeline runs → ExtractMetadata ✓, ExtractFlags ✓, ExtractBodyStructure ✓,
                         ExtractPreview (skip), DetectContentType (skip)
      → MailMessage with metadata + flags + attachment list (no preview/body)
      → Persist to DB, emit MessagesChanged

Phase 2 (background fill — slower, body content):
  IMAP FETCH (BODY[TEXT] for text parts, batches of 50-100)
      → RawEmail { ..., raw_body: Some }
      → Pipeline runs → all steps execute, body-dependent steps now fill in their fields
      → MailMessage with preview, content type, sanitised HTML
      → Merge into DB (update, don't overwrite), emit MessagesChanged
```

### 1.7 Pipeline Naming

Two pipelines share the same visitor pattern (`ProcessingStep` trait), but serve different purposes:

| Pipeline | When | Responsibility | Location |
|---|---|---|---|
| **EmailProcessingPipeline** | Sync time | Raw IMAP → clean `MailMessage` for storage | `engine/pipeline.rs` |
| **EmailViewPipeline** | Render time | Stored content → safe displayable output (HTML sanitisation, CID image resolution, etc.) | `ui/` (TBD) |

The processing pipeline stores raw body content as-is. The view pipeline transforms it for display. This separation means re-rendering with updated sanitisation rules never requires re-syncing.

### 1.8 Design Principles

- **One code path**: no `if partial { ... } else { ... }` at the call site
- **Additive**: new steps (e.g. `DetectCalendarInvite`) are just another visitor
- **Idempotent**: re-running with more data fills gaps, never corrupts existing fields
- **Testable**: each step is a pure function from (MailMessage, RawEmail) → MailMessage
- **No I/O**: the pipeline itself does no network or database work

---

## 2. MIME Parsing

- Crate: `mail-parser`
- _To be detailed — covers MIME tree traversal, multipart handling, charset decoding, malformed message tolerance._

## 3. Content Extraction

- _To be detailed — covers HTML body part selection (prefer text/html, fall back to text/plain), handling multipart/alternative, inline image (CID) extraction and storage._

## 4. HTML Sanitisation

- Crate: `ammonia`
- _To be detailed — covers allowed tags, allowed attributes, CSS property whitelist, handling of provider-specific markup (Gmail gmail_quote divs, Outlook conditional comments, Apple Mail webkit prefixes)._
- This section will be informed by testing against a corpus of real emails.

## 5. Plain Text Extraction

- _To be detailed — covers stripping HTML for FTS5 body_text column, preview text generation for collapsed message cards._

## 6. Attachment Handling

### 6.1 Strategy: Extract on Demand, Not at Sync Time

Attachments are **not** extracted and cached during sync. The raw `.eml` file contains all attachment data. When a user opens or saves an attachment, it is extracted from the `.eml` at that moment via MIME parsing. This avoids doubling disk usage for every message with attachments.

### 6.2 Attachment Metadata

Attachment metadata (filename, MIME type, size, content disposition) is extracted during sync and stored in the `messages` table (or a related `attachments` table — to be defined during Phase 2 migration). This allows the UI to display attachment chips without re-parsing the `.eml`.

### 6.3 Temporary Extraction for Open/Save

When a user clicks "Open" or "Save" on an attachment:

1. Extract the attachment bytes from the `.eml` via `mail-parser`
2. Write to a temporary file: `{XDG_CACHE_HOME}/epistle/attachments/{message-uuid}/{filename}`
3. Open via Flatpak file portal (Open) or file picker portal (Save As)
4. Temporary files are cleaned up on app exit or after 24 hours

_To be further detailed — covers MIME type detection, inline vs attached disposition, duplicate filename handling._

## 7. Thread Construction

Threading groups messages into conversations. The implementation follows the JWZ (Jamie Zawinski) threading algorithm, the de facto standard used by Thunderbird, Mutt, and most email clients. No suitable Rust crate exists — this is built as a self-contained module within Epistle (`threading/`), with clean boundaries for potential future extraction.

### 7.1 Relevant Headers

Every message can carry three headers that establish the thread graph:

| Header | RFC | Purpose |
|---|---|---|
| `Message-ID` | RFC 2822 §3.6.4 | Globally unique identifier for this message |
| `In-Reply-To` | RFC 2822 §3.6.4 | `Message-ID` of the direct parent message |
| `References` | RFC 2822 §3.6.4 | Ordered list of ancestor `Message-ID`s, oldest first |

These are extracted during MIME parsing (section 2) and stored in the `messages` table.

### 7.2 Algorithm — JWZ Threading (Adapted)

Based on [jwz.org/doc/threading.html](https://www.jwz.org/doc/threading.html), adapted for our storage model.

**On message insert (sync engine receives a new message):**

```
1. EXTRACT headers: Message-ID, In-Reply-To, References

2. LOOK UP existing thread:
   a. Parse References header → list of Message-IDs (ordered, oldest first)
   b. Search messages table for any known Message-ID from References list
   c. If found → this message belongs to that thread_id
   d. If not found, check In-Reply-To:
      - Search messages table for In-Reply-To Message-ID
      - If found → same thread_id
   e. If still not found → SUBJECT FALLBACK (step 3)
   f. If no match at all → create new thread, assign new thread_id

3. SUBJECT FALLBACK (only if no header match):
   a. Normalise subject: strip "Re:", "Fwd:", "Fw:", case-fold, trim whitespace
   b. Search threads table for matching subject_normalised
      WHERE last_date > (this message date - 7 days)
   c. If found → assign to that thread_id
   d. If not found → create new thread

4. UPDATE threads table:
   - Set last_date = max(last_date, this message's date)
   - Recalculate unread_count
   - Update participant_ids if new sender
```

### 7.3 Stored Fields

**`messages` table (threading-relevant columns):**

| Column | Type | Description |
|---|---|---|
| `message_id_header` | TEXT | Raw `Message-ID` header value (with angle brackets stripped) |
| `in_reply_to` | TEXT | Raw `In-Reply-To` header value |
| `references` | TEXT | Raw `References` header value (space-separated Message-IDs) |
| `thread_id` | TEXT (UUID) | FK to `threads` table |

**`threads` table:**

| Column | Type | Description |
|---|---|---|
| `thread_id` | TEXT (UUID) | Primary key |
| `account_id` | TEXT | FK to accounts — threads are per-account in v1 |
| `subject_normalised` | TEXT | For subject-based fallback matching |
| `participant_ids` | TEXT | Comma-separated email addresses (for avatar stacks in message list) |
| `last_date` | TEXT | ISO 8601 — date of most recent message in thread |
| `message_count` | INTEGER | Total messages in thread |
| `unread_count` | INTEGER | Unread messages in thread |

### 7.4 Messages Arriving Out of Order

IMAP sync may deliver messages non-chronologically (e.g., a reply arrives before the original). The algorithm handles this:

```
1. Reply arrives first (References: <unknown-id>)
   → No match found → create new thread

2. Original arrives later (Message-ID: <unknown-id>)
   → On insert, check: does any existing message have this Message-ID
     in its References or In-Reply-To?
   → If yes → merge: assign the original to that thread, 
     update thread metadata
```

This means on every message insert, we check in **both directions**:
- Forward: "Does this message reference any known messages?" (standard path)
- Backward: "Do any existing messages reference this message's Message-ID?" (out-of-order repair)

### 7.5 Known Edge Cases

| Problem | Cause | Handling |
|---|---|---|
| Missing headers entirely | Some clients (especially older/corporate) don't set `References` or `In-Reply-To` | Subject fallback with time window |
| Truncated `References` chain | Outlook is known to truncate long `References` chains | `In-Reply-To` is usually preserved; sufficient for direct parent linkage |
| Subject false positives | "Re: Meeting" matches unrelated "Re: Meeting" | 7-day time window on subject fallback prevents stale matches |
| Gmail server-side threading | Gmail groups by subject even without header matches; our client-side threading may differ from Gmail's web UI | Acceptable — our threading is based on RFC headers, which is more correct |
| Mailing list digests | One message contains multiple unrelated messages | Treated as a single message in v1; splitting is v2 at best |
| Cross-account threads | Same conversation spanning two of the user's accounts | Not supported in v1 — threads are per-account (see v1 scope) |
| Forwarded messages | May carry `References` from the original thread but are not part of it | Check: if subject has been modified beyond "Re:"/"Fwd:" stripping, start new thread |

### 7.6 Rebuilding Threads

The `threads` table is a **materialised index**, not source data. It can be rebuilt entirely from the `message_id_header`, `in_reply_to`, and `references` columns in the `messages` table. This provides a recovery path if threading logic is improved or bugs are discovered — re-run the algorithm across all messages to regenerate thread assignments.

### 7.7 Module Structure

```
src/
  threading/
    mod.rs          -- public API: assign_thread(), rebuild_threads()
    algorithm.rs    -- JWZ algorithm implementation
    normalise.rs    -- subject normalisation (strip Re:/Fwd:, case-fold)
```

Same extraction-ready pattern as the `goa/` module. No Epistle-specific imports — takes message headers as input, returns thread assignment as output. SQLite integration lives outside the module.

## 8. Calendar Invite Detection

- Detect `text/calendar` MIME parts during content extraction (section 3)
- Parse ICS data via `icalendar` crate
- _To be detailed — covers extracting event summary, date/time, location, attendees, RSVP status from ICS; rendering as inline GTK widget in message view; generating ICS REPLY for accept/decline/tentative; pushing accepted events to EDS calendar via `libecal`._

## 9. Edge Cases & Compatibility

- _To be populated as we encounter real-world issues during testing. Expected areas: charset issues, malformed MIME, missing headers, provider-specific quirks._

---

_This document is a living spec. Section 1 (pipeline architecture) defines the visitor pattern and two-phase sync strategy. Sections 2–6, 8, and 9 will be detailed during implementation. Section 7 (threading) is specified above and will be implemented alongside the sync engine, since thread assignment happens at message insert time._
