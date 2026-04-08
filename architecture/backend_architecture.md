<!-- markdownlint-disable -->
# Epistle — Backend Architecture & Trait Design

**Version:** 1.2  
**Date:** 2026-04-08  
**Companion Documents:** `gnome_mail_architecture.md`, `imap_sync_engine.md`, `v1_scope.md`  
**Change Log:** v1.2 — Added project structure and module conventions; struct-of-trait-objects pattern; sqlx adoption; EDS D-Bus replaces Folks. v1.1 — Added async trait dispatch strategy (#[async_trait] crate).

---

## 1. Principles

Three architectural rules govern the backend:

1. **Every backend component is backed by a trait.** This enables testing (mock any component), flexibility (swap implementations), and enforces clean boundaries between subsystems.

2. **All communication between backend and frontend uses events.** The GTK app never polls. The storage layer emits events on every write. The GTK app drains events on the main loop and updates the UI. This is the same pattern proven in Moments.

3. **All async traits use `#[async_trait]`.** Rust's `async fn` in traits does not support `dyn` dispatch natively — the compiler cannot construct a vtable for an async function because the returned `Future` type differs per implementation. The `async-trait` crate resolves this by boxing the returned future, adding one heap allocation per call. This is the same approach used throughout Moments (28 files) and is standard practice in the Rust/GTK ecosystem. The allocation cost is negligible at our call frequency — trait methods are called on the order of dozens per sync cycle or per user action, not thousands per second.

4. **SQLite access uses `sqlx` with an async connection pool.** `rusqlite` is synchronous and its `Connection` type is not `Send` — using it from async code requires wrapping every call in `spawn_blocking` with a connection pool crate. Instead, we use `sqlx` with its SQLite async driver, which handles the blocking boundary internally and provides `SqlitePool` for thread-safe connection sharing across the sync service (Tokio) and GTK command layer (GLib main loop). This is the same approach proven in Moments. Schema migrations use `sqlx::migrate!()` with numbered SQL files, executed at `MailEngine::open()` time before any other database access.

---

## 2. System Structure

```
┌─────────────────────────────────────────────────────────┐
│                     MailEngine                           │
│                                                         │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  │
│  │  MailStore    │  │  SyncService │  │  MailCompose  │  │
│  │  (SQLite +   │◄─┤  (Tokio,     │  │  (drafts,    │  │
│  │  filesystem)  │  │  async-imap) │  │   MIME asm)  │  │
│  │              │  │              │  │              │  │
│  │  emits ──────┼──┼─── MailEvent ─┼──┼──► channel   │  │
│  └──────┬───────┘  └──────────────┘  └──────────────┘  │
│         │                                               │
│  ┌──────┴───────┐  ┌──────────────┐  ┌──────────────┐  │
│  │ sync_actions  │  │ MailContacts │  │ MailCalendar  │  │
│  │ (action queue)│  │ (Folks+EDS)  │  │ (ICS+EDS)    │  │
│  └──────────────┘  └──────────────┘  └──────────────┘  │
│                                                         │
│  ┌──────────────┐  ┌──────────────┐                    │
│  │ MailAccounts  │  │ MailSearch   │                    │
│  │ (GOA D-Bus)  │  │ (FTS5)       │                    │
│  └──────────────┘  └──────────────┘                    │
└─────────────────────────────────────────────────────────┘
         │ Sender<MailEvent>
         ▼
┌─────────────────────────────────────────────────────────┐
│                    GTK Application                       │
│                                                         │
│  glib::idle_add  →  drain events  →  MailEvent→AppEvent │
│                                                         │
│  EventBus  →  subscribers (UI widgets)                  │
│                                                         │
│  CommandDispatcher  →  calls MailEngine trait methods    │
│                    →  manages undo state                 │
└─────────────────────────────────────────────────────────┘
```

### Key relationships:

- **MailEngine** is the concrete type that implements all sub-traits and owns the sync service. It is constructed at app startup and split into individual trait objects for distribution to consumers.
- **Consumers receive only the traits they need** (struct-of-trait-objects pattern). The GTK app does not hold a single `Box<dyn MailEngine>` — instead, each component receives narrowly-typed trait objects. See §2.1.
- **SyncService** lives inside `MailEngine`. It is not a separate consumer of traits — it's an internal component that shares the `MailStore`'s database pool and event sender.
- **MailStore** is the single source of truth and the single source of events. Both the sync service and the GTK command layer call `MailStore` methods. The store emits `MailEvent`s on every write.
- **CommandDispatcher** (in the GTK layer) routes user actions via trait objects and manages undo state. It does not emit events — `MailStore` does.

### 2.1 Struct-of-Trait-Objects Pattern

Rather than a single `Box<dyn MailEngine>` God trait that forces every consumer and every test mock to implement all sub-traits, the concrete `MailEngine` is split into individual trait objects at app startup. Each consumer receives only the capabilities it needs.

**App startup wiring:**

```rust
// MailEngine implements all sub-traits
let engine = MailEngineImpl::new(config).await?;
let engine = Arc::new(engine);

// Split into narrowly-typed trait objects for each consumer
let app_state = AppState {
    store:    engine.clone() as Arc<dyn MailStore>,
    search:   engine.clone() as Arc<dyn MailSearch>,
    compose:  engine.clone() as Arc<dyn MailCompose>,
    send:     engine.clone() as Arc<dyn MailSend>,
    contacts: engine.clone() as Arc<dyn MailContacts>,
    calendar: engine.clone() as Arc<dyn MailCalendar>,
    accounts: engine.clone() as Arc<dyn MailAccounts>,
};
```

**Consumer wiring — each component takes only what it needs:**

```rust
// Archive command handler: only needs MailStore
fn handle_archive(store: &dyn MailStore, message_id: &Uuid) { ... }

// Compose window: needs MailCompose + MailContacts + MailSend
fn setup_compose(
    compose: &dyn MailCompose,
    contacts: &dyn MailContacts,
    send: &dyn MailSend,
) { ... }

// Shell search provider: only needs MailSearch
fn handle_shell_search(search: &dyn MailSearch, terms: &[String]) { ... }

// Calendar RSVP: needs MailCalendar + MailSend
fn handle_rsvp(
    calendar: &dyn MailCalendar,
    send: &dyn MailSend,
    message_id: &Uuid,
    response: RsvpResponse,
) { ... }
```

**Testing — mock only what the test touches:**

```rust
// Test archive: mock MailStore only, no other traits needed
let mock_store = MockMailStore::new();
mock_store.expect_archive().returning(|_| Ok(()));
handle_archive(&mock_store, &some_id).await;

// Test autocomplete: mock MailContacts only
let mock_contacts = MockMailContacts::new();
mock_contacts.expect_autocomplete().returning(|_, _| Ok(vec![...]));
```

This pattern is more rigorous than the Moments God trait approach (`Box<dyn Library>`), justified by the larger number of sub-traits in Epistle (7 vs Moments' 8, but with more distinct consumer profiles). The wiring cost at app startup is minimal — it's a one-time split of `Arc` references.

---

## 2.2 Project Structure & Module Conventions

Uses the Rust 2018 module style — `engine.rs` as the module root instead of `engine/mod.rs`. No `mod.rs` files anywhere in the project.

```
src/
├── main.rs                  ← entry point, Tokio runtime, GTK init
├── application.rs           ← GtkApplication, lifecycle, event translation,
│                               wires MailEngineImpl → trait objects → UI + commands
│
├── traits/                  ← trait definitions ONLY, no implementation
│   ├── store.rs             ← trait MailStore
│   ├── search.rs            ← trait MailSearch
│   ├── compose.rs           ← trait MailCompose
│   ├── send.rs              ← trait MailSend
│   ├── contacts.rs          ← trait MailContacts
│   ├── calendar.rs          ← trait MailCalendar
│   └── accounts.rs          ← trait MailAccounts
│
├── engine.rs                ← pub struct MailEngineImpl, open(), close()
├── engine/                  ← trait implementations, one per file
│   ├── store.rs             ← impl MailStore for MailEngineImpl
│   ├── search.rs            ← impl MailSearch for MailEngineImpl
│   ├── compose.rs           ← impl MailCompose for MailEngineImpl
│   ├── send.rs              ← impl MailSend for MailEngineImpl
│   ├── contacts.rs          ← impl MailContacts for MailEngineImpl
│   ├── calendar.rs          ← impl MailCalendar for MailEngineImpl
│   └── accounts.rs          ← impl MailAccounts for MailEngineImpl
│
├── ui.rs                    ← UI root, widget registration
├── ui/                      ← GTK4 + libadwaita + WebKitGTK widgets
│   ├── window.rs
│   ├── sidebar.rs
│   ├── message_list.rs
│   ├── thread_view.rs
│   ├── compose.rs
│   ├── settings.rs
│   └── ...
│
├── commands.rs              ← CommandDispatcher, undo state management
├── commands/                ← one command handler per action
│   ├── archive.rs
│   ├── trash.rs
│   ├── flag.rs
│   ├── send.rs
│   └── ...
│
├── events.rs                ← MailEvent enum (backend layer, no GTK types)
├── app_event.rs             ← AppEvent enum (UI layer)
├── event_bus.rs             ← EventBus + EventSender (Moments pattern)
│
├── goa.rs                   ← GOA D-Bus integration (extraction-ready module)
├── goa/
│   ├── proxies.rs           ← zbus proxy trait definitions
│   └── types.rs             ← ImapConfig, SmtpConfig, AuthMethod, ProviderType
│
├── threading.rs             ← JWZ thread construction (extraction-ready module)
├── threading/
│   ├── algorithm.rs         ← thread assignment logic
│   └── normalise.rs         ← subject normalisation (strip Re:/Fwd:, case-fold)
│
└── sync/                    ← internal to engine, not exposed via traits
    ├── service.rs           ← SyncService, IMAP connection lifecycle
    ├── inbound.rs           ← server → local (fetch, flag pull)
    ├── outbound.rs          ← local → server (action queue → IMAP commands)
    └── idle.rs              ← IMAP IDLE management
```

### Module Boundaries

| Module | Imports from | Never imports |
|---|---|---|
| `traits/` | Standard library, shared types only | `engine/`, `ui/`, `commands/`, `goa/`, `threading/` |
| `engine/` | `traits/`, `goa/`, `threading/`, `sync/`, `events.rs` | `ui/`, `commands/`, `app_event.rs` |
| `ui/` | `traits/`, `app_event.rs`, `event_bus.rs` | `engine/`, `sync/`, `goa/`, `threading/` |
| `commands/` | `traits/`, `app_event.rs`, `event_bus.rs` | `engine/`, `sync/`, `goa/`, `threading/` |
| `goa/` | `zbus`, standard library | Everything else — fully self-contained |
| `threading/` | Standard library | Everything else — fully self-contained |
| `application.rs` | Everything — this is the wiring layer | — |

The critical rule: **`engine/` and `ui/` never import each other.** `traits/` is the contract between them. `application.rs` is the only file that knows both exist — it creates `MailEngineImpl`, splits it into `Arc<dyn Trait>` objects, and hands them to UI components and command handlers.

---

## 3. Trait Definitions

All traits use `#[async_trait]` for `dyn` dispatch compatibility (see Principle 3).

**Identifier convention:** All trait methods use `account_id: &str` as the account identifier parameter. This value is the **GOA Account ID** (`org.gnome.OnlineAccounts.Account.Id`) — a stable, unique string assigned by GOA. The same identifier is used as the primary key (`goa_id`) in the local `accounts` table. The terms `account_id` and `goa_id` refer to the same value throughout the codebase.

### 3.1 `MailEngine` — Concrete Type + Lifecycle

`MailEngine` is not a trait used for `dyn` dispatch. It is the concrete implementation type that implements all sub-traits and owns the sync service. It is never passed as `Box<dyn MailEngine>` — instead, it is split into individual `Arc<dyn SubTrait>` references at startup (see §2.1).

```rust
pub struct MailEngineImpl {
    db: SqlitePool,
    events: Sender<MailEvent>,
    sync_service: SyncService,
    tokio_handle: tokio::runtime::Handle,
    // ... internal state
}

impl MailEngineImpl {
    /// Initialize engine: open DB, run migrations, connect to GOA, start sync service
    pub async fn open(
        config: EngineConfig,
        events: Sender<MailEvent>,
        tokio_handle: tokio::runtime::Handle,
    ) -> Result<Self>;

    /// Graceful shutdown: stop sync, close IMAP connections, flush pending writes
    pub async fn close(&self) -> Result<()>;
}

// MailEngineImpl implements all sub-traits
impl MailAccounts for MailEngineImpl { ... }
impl MailStore for MailEngineImpl { ... }
impl MailSearch for MailEngineImpl { ... }
impl MailCompose for MailEngineImpl { ... }
impl MailSend for MailEngineImpl { ... }
impl MailContacts for MailEngineImpl { ... }
impl MailCalendar for MailEngineImpl { ... }
```

`MailSync` and `MailThreading` are not exposed as sub-traits — they are internal to the sync service within `MailEngineImpl`.

### 3.2 `MailAccounts` — Account Discovery & Credentials

```rust
pub trait MailAccounts {
    /// Discover all mail-enabled accounts from GOA
    async fn discover_accounts(&self) -> Result<Vec<Account>>;

    /// Get a single account by GOA ID
    async fn get_account(&self, goa_id: &str) -> Result<Option<Account>>;

    /// Get IMAP credentials (token or password via GOA)
    async fn get_imap_credentials(&self, goa_id: &str) -> Result<AuthCredential>;

    /// Get SMTP credentials
    async fn get_smtp_credentials(&self, goa_id: &str) -> Result<AuthCredential>;

    /// Check if account needs re-authentication
    async fn ensure_credentials(&self, goa_id: &str) -> Result<CredentialStatus>;
}
```

Wraps the `goa/` module. Trait boundary enables testing with mock accounts.

### 3.3 `MailStore` — Local Storage + Event Emission

```rust
pub trait MailStore {
    // --- Folders ---
    async fn list_folders(&self, account_filter: Option<&str>) -> Result<Vec<Folder>>;
    async fn list_unified(&self, role: FolderRole, cursor: Option<MessageCursor>) -> Result<Vec<MessageSummary>>;

    // --- Messages (read) ---
    async fn list_messages(&self, query: MessageQuery) -> Result<Vec<MessageSummary>>;
    async fn get_message(&self, message_id: &Uuid) -> Result<MessageDetail>;
    async fn get_thread(&self, thread_id: &Uuid) -> Result<Vec<MessageDetail>>;
    async fn message_path(&self, message_id: &Uuid) -> Result<PathBuf>;

    // --- Messages (write) — each emits a MailEvent ---
    async fn insert_message(&self, account_id: &str, folder_id: &str, mime_data: &[u8]) -> Result<Uuid>;
    async fn set_flag(&self, message_id: &Uuid, flag: Flag, value: bool) -> Result<()>;
    async fn move_to_folder(&self, message_id: &Uuid, folder_id: &str) -> Result<()>;
    async fn archive(&self, message_id: &Uuid) -> Result<()>;
    async fn trash(&self, message_id: &Uuid) -> Result<()>;

    // --- Action queue ---
    async fn queue_action(&self, action: SyncAction, status: ActionStatus) -> Result<i64>;
    async fn promote_deferred(&self, action_id: i64) -> Result<()>;
    async fn cancel_action(&self, action_id: i64) -> Result<()>;
    async fn pending_actions(&self) -> Result<Vec<QueuedAction>>;
    async fn mark_synced(&self, action_id: i64) -> Result<()>;
    async fn mark_failed(&self, action_id: i64) -> Result<()>;
}
```

Every write method emits a `MailEvent` via the `Sender<MailEvent>` held by the implementation. The caller (sync service or command handler) never emits events directly.

### 3.4 `MailSearch` — Full-Text Search

```rust
pub trait MailSearch {
    /// Search messages via FTS5
    async fn search(&self, query: &str, account_filter: Option<&str>, limit: u32) -> Result<Vec<SearchResult>>;

    /// Index a message (called by sync service at insert time)
    async fn index_message(&self, message_id: &Uuid, from: &str, subject: &str, body_text: &str) -> Result<()>;

    /// Shell search provider entry point
    async fn shell_search(&self, terms: &[String]) -> Result<Vec<ShellSearchResult>>;
}
```

### 3.5 `MailCompose` — Draft Lifecycle & MIME Assembly

```rust
pub trait MailCompose {
    async fn create_draft(&self, account_id: &str) -> Result<Uuid>;
    async fn save_draft(&self, draft_id: &Uuid, content: DraftContent) -> Result<()>;
    async fn load_draft(&self, draft_id: &Uuid) -> Result<DraftContent>;
    async fn delete_draft(&self, draft_id: &Uuid) -> Result<()>;
    async fn assemble_message(&self, draft_id: &Uuid) -> Result<Vec<u8>>;
    async fn prepare_reply(&self, message_id: &Uuid, mode: ReplyMode) -> Result<DraftContent>;
}
```

### 3.6 `MailSend` — SMTP Submission

```rust
pub trait MailSend {
    /// Submit assembled MIME message via SMTP
    async fn send(&self, account_id: &str, mime_bytes: &[u8]) -> Result<()>;

    /// Queue message in outbox (pre-send, during undo window)
    async fn queue_outbox(&self, draft_id: &Uuid) -> Result<()>;

    /// Cancel a queued send (undo)
    async fn cancel_send(&self, draft_id: &Uuid) -> Result<()>;

    /// Retry failed outbox messages
    async fn retry_outbox(&self, account_id: &str) -> Result<()>;
}
```

### 3.7 `MailContacts` — Contact Autocomplete & Management

```rust
#[async_trait]
pub trait MailContacts {
    /// Autocomplete from EDS address books (via D-Bus) + recent_addresses table.
    /// Results deduplicated by email address at the application level.
    async fn autocomplete(&self, query: &str, limit: u32) -> Result<Vec<ContactSuggestion>>;

    /// Record a sent-to address (update use_count or insert into recent_addresses)
    async fn record_sent_address(&self, email: &str, display_name: Option<&str>) -> Result<()>;

    /// Push a contact to EDS via D-Bus (Add to Contacts action)
    async fn add_to_contacts(&self, email: &str, display_name: Option<&str>) -> Result<()>;
}
```

### 3.8 `MailCalendar` — Calendar Invite Handling

```rust
#[async_trait]
pub trait MailCalendar {
    /// Parse a text/calendar MIME part into structured invite data
    async fn parse_invite(&self, ics_data: &[u8]) -> Result<CalendarInvite>;

    /// Generate an ICS REPLY MIME message for an RSVP response
    /// Returns raw MIME bytes — the caller (RsvpCommand) is responsible for
    /// passing these to MailSend::send(). This keeps MailCalendar independent
    /// of MailSend at the trait level.
    async fn generate_rsvp_mime(&self, message_id: &Uuid, response: RsvpResponse) -> Result<Vec<u8>>;

    /// Push an accepted event to EDS calendar via D-Bus
    async fn push_to_calendar(&self, invite: &CalendarInvite) -> Result<()>;
}
```

**Cross-trait orchestration** is handled by `RsvpCommand` in the command layer, not by the trait:

```rust
// commands/rsvp.rs
pub struct RsvpCommand {
    calendar: Arc<dyn MailCalendar>,
    send: Arc<dyn MailSend>,
}

impl RsvpCommand {
    pub async fn execute(&self, message_id: &Uuid, response: RsvpResponse) -> Result<()> {
        let rsvp_bytes = self.calendar.generate_rsvp_mime(message_id, response).await?;
        self.send.send(account_id, &rsvp_bytes).await?;
        if response == RsvpResponse::Accept {
            let invite = self.calendar.parse_invite(/* from message */).await?;
            self.calendar.push_to_calendar(&invite).await?;
        }
        Ok(())
    }
}
```

---

## 4. Event Model

### 4.1 Event Channel

The `Sender<MailEvent>` / `Receiver<MailEvent>` pair uses an **unbounded async channel** (e.g., `async_channel::unbounded()`) that works across the Tokio → GLib boundary. Unbounded is chosen because the GTK drain loop must never block the sync service.

**Large initial sync mitigation:** During initial sync of a large mailbox (100k+ messages), individual `MessageReceived` events are suppressed. Instead, the sync service emits periodic `SyncProgress` events (e.g., every 100 messages) and a single `SyncComplete` at the end, at which point the UI refreshes the message list in bulk. This prevents unbounded channel growth and avoids flooding the GTK main loop with individual updates.

### 4.2 Two-Layer Events (Moments Pattern)

**Layer 1: `MailEvent`** — Emitted by `MailStore` on every write. Backend types only, no GTK dependencies.

```rust
pub enum MailEvent {
    // Lifecycle
    Ready,
    Error { account_id: String, error: String },

    // Account
    AccountDiscovered { account: Account },
    AccountRemoved { goa_id: String },
    AccountAttentionNeeded { goa_id: String },

    // Sync progress
    SyncStarted { account_id: String, folder_id: String },
    SyncProgress { account_id: String, folder_id: String, fetched: u32, total: u32 },
    SyncComplete { account_id: String, folder_id: String },

    // Storage events (emitted by MailStore on every write)
    MessageReceived { message_id: Uuid, folder_id: String, account_id: String },
    MessageFlagsChanged { message_id: Uuid, flags: Flags },
    MessageMoved { message_id: Uuid, from_folder: String, to_folder: String },
    MessageDeleted { message_id: Uuid },
    ThreadUpdated { thread_id: Uuid },
    DraftSaved { draft_id: Uuid },

    // Send
    MessageQueued { draft_id: Uuid },
    MessageSent { draft_id: Uuid },
    SendFailed { draft_id: Uuid, error: String },

    // Network
    NetworkStateChanged { online: bool },

    // Calendar
    InviteResponseSent { message_id: Uuid, response: RsvpResponse },
}
```

**Layer 2: `AppEvent`** — GTK application layer. Translated 1:1 from `MailEvent` at the application boundary, plus UI command events.

```rust
pub enum AppEvent {
    // --- All MailEvent variants mapped 1:1 ---
    // (omitted for brevity — same as above)

    // --- UI command events (from user actions) ---
    ArchiveRequested { message_id: Uuid },
    TrashRequested { message_id: Uuid },
    FlagRequested { message_id: Uuid, flag: Flag, value: bool },
    MoveRequested { message_id: Uuid, folder_id: String },
    ReplyRequested { message_id: Uuid, mode: ReplyMode },
    ComposeRequested { account_id: Option<String> },
    SendRequested { draft_id: Uuid },
    CancelSendRequested { draft_id: Uuid },
    SearchRequested { query: String },
    RsvpRequested { message_id: Uuid, response: RsvpResponse },
    AddToContactsRequested { email: String, display_name: Option<String> },
    UndoRequested { action_id: i64 },
}
```

### 4.2 Event Flow

```
MailStore (any write)
    │ emits MailEvent via Sender<MailEvent>
    ▼
GTK Application (glib::idle_add drains channel)
    │ translates MailEvent → AppEvent
    ▼
EventBus (push-based, thread-local subscribers)
    │ dispatches to all registered UI subscribers
    ▼
UI Widgets (message list, thread view, folder counts, etc.)
```

### 4.3 Command Flow

```
User clicks Archive button
    │ UI emits AppEvent::ArchiveRequested
    ▼
CommandDispatcher
    │ stores undo state: { action_id, message_id, original_folder_id }
    │ calls store.archive(message_id)  // via Arc<dyn MailStore>
    ▼
MailStore::archive()
    │ writes move to local DB
    │ inserts sync_actions row (status: deferred)
    │ emits MailEvent::MessageMoved
    ▼
UI updates (message disappears from list)
CommandDispatcher shows AdwToast (5s undo window)

    ... 5 seconds pass, no undo ...

CommandDispatcher
    │ calls MailStore::promote_deferred(action_id)
    │ drops undo state
    ▼
SyncService (next poll)
    │ picks up pending action
    │ translates to IMAP MOVE command
    │ executes on server
    │ calls MailStore::mark_synced(action_id)
```

### 4.4 Undo Flow

```
User clicks Undo on toast (within 5s)
    │ UI emits AppEvent::UndoRequested { action_id }
    ▼
CommandDispatcher
    │ retrieves stored undo state: { original_folder_id }
    │ calls MailStore::move_to_folder(message_id, original_folder_id)
    │ calls MailStore::cancel_action(action_id)
    ▼
MailStore::move_to_folder()
    │ writes reversal to local DB
    │ emits MailEvent::MessageMoved (back to original folder)
MailStore::cancel_action()
    │ deletes the deferred sync_actions row
    ▼
UI updates (message reappears)
No IMAP commands were ever issued — net zero server impact
```

---

## 5. What Is NOT a Trait

The following are internal implementation details, not exposed as traits:

| Component | Why not a trait |
|---|---|
| **SyncService** | Internal to `MailEngine`. Consumes `MailStore` and `MailAccounts` internally. Not called by GTK app. |
| **MailThreading** | Called by sync service at message insert time. Internal algorithm, not a capability the GTK app needs. Lives in `threading/` module. |
| **FTS5 indexing** | Called by sync service at message insert time via `MailSearch::index_message()`. The GTK app only calls `MailSearch::search()`. |
| **IMAP connection management** | Internal to sync service. Pool management, IDLE lifecycle, reconnect logic — none exposed to GTK app. |
| **GOA D-Bus proxies** | Internal to `MailAccounts` implementation. The `goa/` module's zbus proxies are implementation detail. |

---

## 6. Testing Model

The struct-of-trait-objects pattern means each test mocks only the traits it touches. No God trait mock needed.

| Test Target | Traits Mocked | What's Verified |
|---|---|---|
| Archive command handler | `MailStore` only | `archive()` called, action queued |
| Compose window | `MailCompose` + `MailContacts` | Draft created, autocomplete queried |
| Send flow | `MailSend` + `MailStore` | MIME submitted, outbox managed |
| Shell search provider | `MailSearch` only | FTS5 query issued, results formatted |
| Calendar RSVP | `MailCalendar` + `MailSend` | ICS reply generated, sent via SMTP |
| Sync service logic | `MailStore` (internal) | Actions queued, IMAP commands generated |
| Threading algorithm | None — pure function | Headers in, thread ID out |
| MIME assembly | None — pure function | Draft content in, MIME bytes out |
| Contacts autocomplete | `MailContacts` only | EDS queried, results deduplicated |
| Full integration | Real SQLite, mock IMAP | End-to-end sync and action flows |
