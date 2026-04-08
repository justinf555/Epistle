# Epistle — V1 Feature Scope

**Version:** 1.0  
**Status:** Approved  
**Date:** 2026-04-08  
**Companion Document:** `gnome_mail_architecture.md` (v0.2)

---

## Scope Principle

V1 ships a **fully functional daily-driver email client** for GNOME users with Gmail, Microsoft 365, or standard IMAP accounts. A user should be able to switch from Geary or web Gmail and not feel blocked on any routine email task. The scope targets completeness for common workflows, not feature parity with Outlook or Thunderbird.

---

## In Scope

### Account Management

| Feature | Details |
|---|---|
| GOA account discovery | Enumerate mail-enabled accounts via `org.gnome.OnlineAccounts` D-Bus interface |
| Gmail via GOA | `OAuth2Based.GetAccessToken()` → IMAP XOAUTH2 |
| Microsoft 365 via GOA | `OAuth2Based.GetAccessToken()` → IMAP XOAUTH2 |
| Generic IMAP/SMTP via GOA | `PasswordBased.GetPassword()` → SASL PLAIN over TLS; benefits from GNOME 50 Mail Autoconfig |
| Multi-account support | Unified inbox + per-account folder trees in sidebar |
| Account lifecycle signals | Listen for `AccountAdded`, `AccountRemoved`, `AccountChanged` from GOA D-Bus |
| Account pip indicators | Coloured dot on messages identifying which account |

### Sync Engine

| Feature | Details |
|---|---|
| IMAP client | `async-imap` crate — IMAP4rev1 with extensions |
| Incremental sync | UID-based tracking: fetch new messages since last known UID |
| CONDSTORE flag sync | Use `CHANGEDSINCE` modifier when server supports it; fall back to full flag fetch |
| IMAP IDLE | Push notifications for new mail on active folder; polling fallback with exponential backoff |
| Background Portal | Register with `org.freedesktop.portal.Background` to keep sync engine running when window is closed. Shown in GNOME system tray as background app. **Degraded mode:** if user denies permission or portal is unavailable (non-GNOME desktop), sync only operates while the window is open. A one-time `AdwBanner` informs the user that background sync is disabled and new mail notifications require the app to be open. |
| NetworkManager awareness | Subscribe to NetworkManager D-Bus signals for online/offline state changes. Go to offline mode immediately on network drop; trigger sync immediately on network return. Avoids blind retry with backoff. |
| Offline storage | SQLite metadata + filesystem `.eml` files (atomic write-then-rename) |
| Bidirectional flag sync | Read/unread, starred/flagged — local changes pushed to server, server changes pulled to local |
| Folder sync | Full IMAP folder hierarchy with special-use folder detection (Inbox, Sent, Drafts, Trash, Archive) |
| Connection management | Per-provider connection limits (Gmail: 15, Yahoo: 5), auto-reconnect with exponential backoff |
| SMTP send | Via GOA credentials — XOAUTH2 for OAuth providers, PLAIN for password providers |

### Reading Email

| Feature | Details |
|---|---|
| Conversation/thread view | Grouped by `References`/`In-Reply-To` headers (RFC 5256); subject-based heuristic fallback |
| HTML rendering | WebKitGTK with `ammonia` sanitisation, JS disabled, CSP injected |
| Plain text fallback | Rendered as `GtkLabel` or simple WebView for messages with no HTML part |
| Collapsible message cards | Collapsed = native GTK (avatar, sender, date, preview text); expanded = WebKitWebView |
| Quoted text collapse | Detect `>` prefix, `<blockquote>`, Gmail `gmail_quote` div; collapse with "Show quoted text" toggle |
| Inline images (CID) | Custom URI scheme handler on shared `WebContext` |
| WebView height management | `evaluate_javascript("document.body.scrollHeight")` on `WEBKIT_LOAD_FINISHED` |
| Lazy WebView lifecycle | Create on expand, destroy on collapse when >3 messages expanded simultaneously |
| External image blocking | Blocked by default; per-message "Load remote images" option |
| Attachments — view & save | Display as chips below message; open via Flatpak file portal; save via file picker portal |
| Calendar invite rendering | Detect `text/calendar` MIME parts; parse ICS via `icalendar` crate; render inline invite card (event summary, date/time, location, attendees) as a native GTK widget in the message view |
| Calendar invite RSVP | Accept / Decline / Tentative buttons on invite card; generates ICS `REPLY` and sends via SMTP; updates card state to show response |
| Push to EDS calendar | Accepted events pushed to Evolution Data Server calendar via `libecal`; EDS syncs upstream to Google Calendar / Microsoft / etc. |

### Compose & Send

| Feature | Details |
|---|---|
| Reply, Reply All, Forward | Standard actions from thread view header |
| Compose new message | From sidebar button or keyboard shortcut (`c`) |
| Rich text editor | WebKitWebView with `contenteditable="true"`; toolbar for bold, italic, underline, lists, links |
| Plain text compose mode | Toggle to plain text; switches to `GtkTextView` or strips formatting |
| Two-zone compose model | Top: editable compose area; bottom: read-only sanitised quoted thread history |
| To / CC / BCC chip input | `GtkFlowBox` + `GtkEntry`; RFC 5322 validation on Enter/Tab/comma |
| Contact autocomplete | `GtkPopover` with `GtkListBox`; sourced from EDS address books via D-Bus (`zbus`), supplemented by local `recent_addresses` table. Results deduplicated by email address at the application level. EDS contacts rank higher, `recent_addresses` ranked by `use_count`. |
| Add to Contacts | Action on email addresses in message view and recent addresses — pushes a VCard to EDS via D-Bus, which syncs upstream to Google Contacts / Microsoft / etc. |
| Attachments — add | File picker via Flatpak portal; displayed as removable chips |
| Draft autosave | `GLib::timeout_add_seconds(30)` on dirty flag; atomic write to `{uuid}.draft.eml` |
| Undo send | 5-second `AdwToast` window before SMTP submission; cancel returns to compose |
| HTML cleanup | Post-process `contenteditable` output to normalise messy DOM |
| CSS inlining | Convert `<style>` blocks to inline `style=` attributes (required for Gmail/Outlook compatibility) |
| Plain text fallback | Strip HTML to generate `text/plain` MIME alternative part |
| MIME assembly | `multipart/mixed` (if attachments) wrapping `multipart/alternative` (`text/plain` + `text/html`); headers: `From`, `To`, `CC`, `Date`, `Message-ID`, `References`, `In-Reply-To` |
| Send error handling | On SMTP failure, message stays in Outbox; retry with exponential backoff on reconnect |

### Navigation & Organisation

| Feature | Details |
|---|---|
| Three-pane layout | `AdwNavigationSplitView` — sidebar (210px), message list (280px), content (flex) |
| Unified smart folders | Cross-account virtual views for Inbox, Sent, Drafts, Archive, and Trash — queried by folder `role` across all active accounts. Unified Inbox is the default view. Follows the Apple Mail model. |
| Per-account folder trees | Sidebar section per account with full folder hierarchy including standard and custom folders |
| Archive | Single action — moves message to Archive folder |
| Delete | Moves message to Trash folder |
| Move to folder | Folder picker dialog |
| Star / flag | Toggle; synced bidirectionally to IMAP `\Flagged` |
| Mark read / unread | Toggle; synced bidirectionally to IMAP `\Seen` |
| Full-text search | FTS5 virtual table over `from_addr`, `subject`, `body_text`; single `MATCH` query |
| Search results view | Results displayed with folder origin label |

### Polish & GNOME Integration

| Feature | Details |
|---|---|
| Dark mode | Automatic via `AdwStyleManager` / system preference |
| Adaptive layout | Collapses to single-pane drill-down below 600px via `AdwNavigationSplitView` |
| Empty states | `AdwStatusPage` for empty inbox, empty search results, empty folder |
| Offline / error banners | `AdwBanner` for connection issues, sync errors |
| Keyboard navigation | `j`/`k` navigate list, `e` archive, `#` delete, `r` reply, `a` reply all, `f` forward, `c` compose, `/` search, `?` shortcut overlay |
| System notifications | New mail notifications via GLib / Flatpak notification portal |
| Settings | `AdwPreferencesWindow` — accounts list (links to GNOME Settings), plain text signature per account, notification toggle per account |
| GNOME Shell search provider | Implement `org.gnome.Shell.SearchProvider2` D-Bus interface — exposes email search to system-wide GNOME Shell search (Super key). Queries FTS5 index, returns sender/subject/preview. Clicking a result opens Epistle to that thread. Registered via `.ini` file in Meson build. |
| D-Bus activation | App is D-Bus activatable — required for search provider, also enables single-instance and deep linking from notifications |
| `mailto:` URI handler | Register as `x-scheme-handler/mailto` in `.desktop` file. Clicking a `mailto:` link anywhere on the desktop opens Epistle's compose window with the address pre-filled. Standard behaviour for a desktop email client. |
| Flatpak packaging | Meson build, Flatpak manifest, Flathub-ready |

### Security

| Feature | Details |
|---|---|
| HTML sanitisation | `ammonia` crate — all email HTML sanitised before reaching WebKit |
| JS disabled in display WebViews | Defence-in-depth alongside sanitisation |
| CSP injection | `default-src 'none'; style-src 'unsafe-inline'` on every message load |
| External resource blocking | No remote image/resource loading by default |
| TLS required | All IMAP and SMTP connections; certificate validation enforced |
| Credentials via GOA only | App never stores passwords or tokens directly — retrieved from GOA/Keyring at runtime |
| Attachments via portal | Opened through Flatpak file portal; app never executes attachments |
| Compose quote sanitisation | Quoted HTML sanitised through `ammonia` before injection into compose WebView |

---

## Out of Scope — V1

| Feature | Rationale | Revisit |
|---|---|---|
| **Snooze** | Not IMAP-native. Requires local-only state management and a design decision about where snoozed state lives. Not standard in desktop clients (Gmail web only). | v2 |
| **Send later / scheduled send** | Requires a persistent background service or scheduler that runs when app is closed. Gmail/Outlook handle this server-side. | v2 |
| **QRESYNC (RFC 7162)** | Performance optimisation, not functional requirement. Standard UID-based sync is sufficient for typical mailbox sizes. Geary ships without it. | v2 — contribute to `async-imap` or wrapper layer |
| **Custom IMAP implementation** | `async-imap` covers IMAP4rev1 + IDLE + CONDSTORE. Extend or PR upstream if gaps found. | Ongoing |
| **Microsoft Graph API** | M365 still supports IMAP with OAuth via GOA. Graph API is a large surface for marginal v1 benefit. Monitor Microsoft's IMAP deprecation timeline. | v1.1 or v2 |
| **S/MIME / PGP encryption** | Niche user base. Significant key management and UI complexity. | v2 |
| **Full calendar management** | Epistle is an email client, not a PIM suite. Calendar invite handling (receive, RSVP, push to EDS) is in scope. Full calendar views, event creation, and calendar editing are not. | Out of scope |
| **Labels / tags** | Gmail labels already appear as IMAP folders. Custom local tagging is a separate feature. | v2 |
| **Filters / rules** | Server-side Sieve is the correct approach. Client-side rules duplicate server state. | v2 — Sieve management UI |
| **Multiple selection + bulk actions** | Significant UI effort (selection mode, action bar, bulk IMAP commands). Ship single-action first. | v1.1 |
| **Conversation muting** | Gmail-specific concept with no IMAP equivalent. | v2 — local-only flag |
| **Rich text signature editor** | v1 ships plain text signatures. Rich HTML signature editing is a compose-within-compose problem. | v1.1 |
| **Import / export (mbox, PST)** | Migration tooling. Important for adoption but not daily use. | v1.1 |
| **Printing** | Rarely used. Requires GTK print integration. | v1.1 |
| **Custom notification sounds** | System default is sufficient. | v1.1 |
| **GTK5 compatibility layer** | GTK5 timeline is unclear. Do not prematurely abstract. | When GTK5 ships |
| **Custom account setup UI** | GOA handles account setup for all supported providers (OAuth + Autoconfig). No need for in-app account configuration forms in v1. | v1.1 — if demand for non-GOA providers |
| **Undo for move/flag operations** | Undo send is in scope. Undo for archive and delete is in scope (via `AdwToast`). Undo for move-to-folder and flag changes is cut. | v1.1 |

---

## What a V1 User Can Do

A user who installs Epistle v1 can:

1. Add their Gmail, Microsoft 365, or IMAP account through GNOME Settings (GOA)
2. Open Epistle and see a unified inbox across all configured accounts
3. Read HTML emails rendered safely with inline images
4. Navigate conversations in a threaded view with collapsible messages
5. Reply, reply all, and forward with rich text formatting
6. Compose new messages with contact autocomplete from GNOME Contacts
7. Attach files, view received attachments, and save them to disk
8. Archive, delete, star, and mark messages read/unread — all synced to the server
9. Search across all mail by sender, subject, or body text
10. Receive a calendar invite, see it rendered inline, and accept/decline — with the event pushed to their GNOME calendar
11. Add a sender to GNOME Contacts directly from a message
12. Work fully offline with everything stored locally
13. Use keyboard shortcuts for fast navigation
14. Receive system notifications for new mail

This covers the complete daily workflow for a typical email user switching from Geary, web Gmail, or Apple Mail.

---

## Success Criteria

V1 is ready to ship when:

- [ ] A user can complete a full email workflow (receive → read → reply → archive) without leaving the app
- [ ] Multi-account unified inbox works with at least Gmail + one generic IMAP provider
- [ ] HTML emails from Gmail, Outlook, Apple Mail, and common mailing lists render correctly
- [ ] Compose produces emails that render correctly in Gmail, Outlook, and Apple Mail
- [ ] Offline mode works — all previously synced mail is readable and searchable without network
- [ ] No credential data exists outside of GOA/GNOME Keyring
- [ ] App passes Flathub submission requirements
- [ ] Keyboard-only navigation is possible for all core actions
