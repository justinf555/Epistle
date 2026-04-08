<!-- markdownlint-disable -->
# Epistle — IMAP Sync Engine Design

**Version:** 0.2 (Revised)  
**Status:** To be detailed during Phase 1 implementation  
**Date:** 2026-04-08  
**Companion Documents:** `gnome_mail_architecture.md`, `v1_scope.md`, `goa_integration.md`

---

## Approach

This document will be fleshed out incrementally as the sync engine is built. The outline below captures the key design areas and known decisions. Detail will be added as implementation reveals the right patterns.

---

## 1. Foundation

- Built on `async-imap` crate (IMAP4rev1 + IDLE + CONDSTORE)
- Runs on a dedicated Tokio runtime, never on GTK main loop
- Credentials retrieved from GOA via the `goa/` module (see `goa_integration.md`)
- Lives **inside** the `MailEngine` implementation — not a separate consumer of traits
- The sync service is spawned by `MailEngine::open()` and shut down by `MailEngine::close()`
- Shares the SQLite connection pool and event sender with the rest of `MailEngine`

## 2. Bidirectional Sync Model

The sync engine is bidirectional:

- **Inbound (server → local):** Fetch new messages, pull flag changes, detect folder changes
- **Outbound (local → server):** Push user actions (archive, delete, flag, move) from the local action queue to IMAP

### 2.1 Inbound — Initial Full Sync

_To be detailed — covers first-time folder enumeration, message header fetch, body fetch strategy (headers-first vs full fetch), and how we handle large mailboxes on first connect._

### 2.2 Inbound — Incremental Sync

- UID-based tracking: `UID FETCH {last_uid+1}:* (FLAGS BODY.PEEK[])`
- Flag changes: `UID FETCH 1:{last_uid} (FLAGS)`, narrowed with `CHANGEDSINCE {modseq}` when CONDSTORE is available
- QRESYNC deferred to post-v1

### 2.3 Inbound — Folder Sync

_To be detailed — covers IMAP folder LIST, special-use folder detection (\Inbox, \Sent, \Drafts, \Trash, \Archive), folder hierarchy mapping to local DB, handling folder renames/deletes._

### 2.4 Outbound — Action Queue

User actions in the GTK app (archive, delete, flag, move) write to the local database **and** insert a row into the `sync_actions` table. The sync service polls this table and translates queued actions into IMAP commands.

```
sync_actions table:
┌──────┬──────────────┬────────┬─────────────────┬──────────┬────────────┐
│ id   │ message_id   │ action │ params          │ status   │ created_at │
├──────┼──────────────┼────────┼─────────────────┼──────────┼────────────┤
│ 1    │ uuid-1234    │ move   │ → Archive       │ deferred │ 12:00:00   │
│ 2    │ uuid-5678    │ flag   │ +\Seen          │ pending  │ 12:00:01   │
│ 3    │ uuid-9012    │ flag   │ +\Flagged       │ synced   │ 11:59:50   │
│ 4    │ uuid-3456    │ delete │                 │ failed   │ 11:58:00   │
└──────┴──────────────┴────────┴─────────────────┴──────────┴────────────┘
```

**Action statuses:**

| Status | Meaning |
|---|---|
| `deferred` | Written by an undoable action. Not yet eligible for sync. Waiting for undo window to expire. |
| `pending` | Ready to sync. Either written by a non-undoable action, or promoted from `deferred` after undo window expired. |
| `synced` | Successfully pushed to IMAP. Can be cleaned up. |
| `failed` | IMAP command failed. Will be retried with exponential backoff. |

**IMAP command translation:**

| Action | IMAP Commands |
|---|---|
| `move` | `UID MOVE {uid} "target_folder"` if server supports MOVE extension (RFC 6851); otherwise `UID COPY` → `UID STORE +FLAGS (\Deleted)` → `UID EXPUNGE` |
| `flag` | `UID STORE {uid} +FLAGS (\Seen)` or `-FLAGS (\Seen)` etc. |
| `delete` | `UID MOVE {uid} "Trash"` (or copy+delete if no MOVE support) |
| `send` | Not IMAP — handled by SMTP pipeline. After SMTP success, a `move` action copies the sent message to the Sent folder. |

**Outbound sync loop:**

```
loop {
    1. Query sync_actions WHERE status = 'pending' ORDER BY created_at
    2. For each action:
       a. Look up message IMAP UID and folder from messages table
       b. Translate action to IMAP commands
       c. Execute on IMAP connection
       d. On success → set status = 'synced'
       e. On failure → set status = 'failed', increment retry_count
    3. Batch flag changes where possible (multiple flags on same folder)
    4. Clean up synced actions older than 24 hours
    5. Sleep or wait for notification of new pending actions
}
```

### 2.5 Outbound — Undo Pattern

Actions that support undo (archive, delete) follow a deferred sync pattern:

```
t=0s  User clicks Archive
      → MailStore writes move to local DB (message.folder_id → archive folder)
      → MailStore inserts sync_actions row with status = 'deferred'
      → MailStore emits MessageMoved event
      → UI updates immediately (message disappears from inbox)
      → Command layer shows AdwToast with Undo button (5 second timer)

t=5s  Undo window expires (no undo clicked)
      → Command layer updates sync_actions status: deferred → pending
      → Sync service picks it up on next poll, pushes to IMAP

— or —

t=3s  User clicks Undo
      → Command layer calls MailStore::move_to_folder(message_id, original_folder_id)
      → MailStore writes reversal to local DB
      → MailStore deletes the deferred sync_actions row (net zero — nothing to sync)
      → MailStore emits MessageMoved event (back to inbox)
      → UI updates (message reappears)
      → No IMAP commands were ever issued
```

Non-undoable actions (flag changes, mark read/unread) skip the `deferred` state and go directly to `pending`.

## 3. IMAP IDLE

### 3.1 IDLE Target Strategy

IDLE on **all standard-role folders** (Inbox, Sent, Drafts, Archive, Trash) per account, capped at `provider_connection_limit - 2` to always reserve headroom for active sync operations (fetching, pushing flags, moving messages). SMTP send uses a separate connection to a different server and does not count against the IMAP budget.

**Connection budget per provider:**

| Provider | IMAP Limit | Reserved for sync | Available for IDLE | Folders covered |
|---|---|---|---|---|
| Gmail | 15 | 2 | 13 | All 5 standard + custom folders |
| Outlook/M365 | 10-20 | 2 | 8-18 | All 5 standard + custom folders |
| Yahoo | 5 | 2 | 3 | Inbox, Sent, Drafts (prioritised) |
| Self-hosted | Typically unlimited | 2 | All | All standard + custom folders |

**IDLE priority order** when budget is constrained (drop from bottom first):

1. Inbox (highest — where incoming mail arrives)
2. Sent (detect changes from other devices)
3. Drafts (detect draft edits from other devices)
4. Archive (low value — changes are mostly user-initiated)
5. Trash (lowest — changes are almost always user-initiated)

Folders beyond the standard five (custom/user folders, Gmail labels) are not IDLE'd — they are polled on the standard interval.

### 3.2 IDLE Connection Lifecycle

- Each IDLE target folder gets one persistent IMAP connection
- IDLE connections are established after initial sync completes
- IMAP IDLE has a server-side timeout (typically 29 minutes per RFC 2177). The client re-issues IDLE before timeout.
- On IDLE notification (new message, flag change), the connection exits IDLE, performs an incremental sync on that folder, then re-enters IDLE
- _To be detailed — covers re-issue timing, handling of simultaneous IDLE breaks across folders._

### 3.3 Fallback to Polling

If a server does not support the IDLE extension (detected via CAPABILITY response):

- Fall back to polling at a configurable interval (default: 5 minutes)
- Use exponential backoff on repeated connection failures
- _To be detailed — covers polling interval configuration, interaction with NetworkManager state._

## 4. Connection Management

### 4.1 Connection Pool

Each account maintains a pool of IMAP connections:

- **IDLE connections:** One per IDLE target folder (see §3.1), persistent
- **Sync connections:** 2 reserved for active operations (fetch, push flags, move messages), created on demand and returned to pool after use
- Total connections per account capped at the provider's limit

### 4.2 Per-Provider Limits

| Provider | Limit | Detection |
|---|---|---|
| Gmail | 15 | Hardcoded for `google` provider type |
| Yahoo | 5 | Hardcoded for `yahoo` provider type |
| Outlook/M365 | 10 (conservative default) | Hardcoded for `ms_graph` provider type |
| Generic IMAP | 10 (safe default) | Configurable per account if needed |

### 4.3 Reconnection

- On connection drop: exponential backoff (1s, 2s, 4s, 8s, max 60s)
- On auth failure: call `GOA.EnsureCredentials()`, re-fetch token, retry once. If still failing, set `AttentionNeeded` banner and stop retrying for that account.
- On NetworkManager offline signal: drop all connections immediately, do not retry. On online signal: reconnect all accounts and resume sync.
- _To be detailed — covers GOA token refresh interaction, partial connection pool recovery._

## 5. Storage Integration

- Messages written atomically: `{uuid}.eml.tmp` → `rename()` → SQLite row insert
- SQLite row only inserted after filesystem rename succeeds
- The `messages` row insert, `fts5_search` index insert, and `threads` table update are performed in a **single SQLite transaction**. This ensures a message is never visible in the message list but invisible to search, or vice versa. Since `fts5_search` is a virtual table in the same database, this is straightforward with `sqlx`.
- Sync state checkpointed in `sync_state` table per folder
- All user actions write to both the data tables and the `sync_actions` queue in the same transaction

## 6. Event Model

Events originate from `MailStore` — the storage layer emits a `MailEvent` on every write, regardless of who initiated it (sync service or user action via command layer).

```
Sync service writes new message:
  → MailStore::insert_message()
  → writes to SQLite + filesystem
  → emits MailEvent::MessageReceived
  → GTK app drains event, translates to AppEvent, updates UI

User archives a message:
  → CommandHandler calls MailStore::archive()
  → writes to SQLite + sync_actions queue
  → emits MailEvent::MessageMoved
  → GTK app drains event, translates to AppEvent, updates UI
  → CommandHandler shows undo toast (command layer concern, not event)

Same event path, same UI update, regardless of origin.
```

The `Sender<MailEvent>` is owned by the `MailStore` implementation and passed in at construction. Neither the sync service nor the command layer emit events directly — they call `MailStore` methods and the store handles emission.

## 7. Error Handling

_To be detailed — covers network errors, auth errors (trigger GOA re-auth flow), server errors, partial sync recovery, UID validity changes, failed action retry with exponential backoff._

## 8. Offline Mode

When NetworkManager reports offline state:

- Inbound sync pauses — no IMAP connections attempted
- Outbound sync pauses — `pending` actions accumulate in the queue
- User can continue all local actions — archive, delete, flag, compose, search
- All actions queue in `sync_actions` with `pending` status
- When NetworkManager reports online, both inbound and outbound sync resume immediately
- Outbound queue drains, pushing accumulated actions to IMAP

The user experience is seamless — local actions work identically online and offline. The only visible difference is a `AdwBanner` indicating offline state.

## 9. `async-imap` Usage Patterns

_To be detailed as we discover what maps cleanly and what needs workarounds. May include notes on extending or contributing upstream._

---

_This document is a living spec. Sections marked "to be detailed" will be filled in during Phase 1 implementation as design decisions are made and validated against real IMAP servers._
