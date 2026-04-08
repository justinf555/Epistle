<!-- markdownlint-disable -->
# Epistle — GOA Integration Design

**Version:** 1.0  
**Date:** 2026-04-08  
**Companion Documents:** `gnome_mail_architecture.md` (v0.2), `v1_scope.md`

---

## 1. Overview

GNOME Online Accounts (GOA) is the sole authentication and account management path for Epistle v1. The app does not implement its own OAuth flows, account setup forms, or credential storage. Users add email accounts in **GNOME Settings → Online Accounts**, and Epistle discovers them via D-Bus.

This design eliminates OAuth provider verification (Google consent screen, Microsoft app registration), redirect URI handling, and token lifecycle management from Epistle's codebase entirely.

### 1.1 What GOA Provides

| Capability | How Epistle Uses It |
|---|---|
| Account discovery | Enumerate all mail-enabled accounts at startup and on change |
| OAuth token management (Gmail, M365) | Call `GetAccessToken()` → use token for IMAP/SMTP XOAUTH2 |
| Password management (generic IMAP) | Call `GetPassword("imap-password")` → use for IMAP SASL PLAIN |
| Server configuration | Read IMAP/SMTP host, port, SSL/TLS settings from account properties |
| Mail Autoconfig (GNOME 50+) | GOA auto-discovers server settings from email domain for generic providers |
| Account lifecycle | D-Bus signals for account added, removed, or re-authenticated |

### 1.2 What Epistle Handles

| Capability | Details |
|---|---|
| IMAP connection using GOA-provided credentials | `async-imap` with XOAUTH2 or PLAIN auth |
| SMTP connection using GOA-provided credentials | XOAUTH2 or PLAIN auth |
| Local account state | `accounts` table mapping GOA account IDs to internal state |
| Token expiry handling | Re-call `GetAccessToken()` when IMAP auth fails or token approaches expiry |
| Offline operation | Continue operating from local storage when GOA or network is unavailable |

---

## 2. D-Bus Interface Reference

GOA accounts are exposed as D-Bus objects under the path `/org/gnome/OnlineAccounts/Accounts/{id}`. Each object implements multiple interfaces depending on what the account supports.

### 2.1 Account Object Manager

| | |
|---|---|
| **Bus** | Session bus |
| **Service** | `org.gnome.OnlineAccounts` |
| **Object path** | `/org/gnome/OnlineAccounts` |
| **Interface** | `org.freedesktop.DBus.ObjectManager` |
| **Method** | `GetManagedObjects()` → dictionary of all account objects and their interfaces |

This is the entry point. Call `GetManagedObjects()` to get all accounts, then filter for objects that implement `org.gnome.OnlineAccounts.Mail`.

### 2.2 org.gnome.OnlineAccounts.Account (base interface)

Every account object implements this interface.

| Property | Type | Description |
|---|---|---|
| `Id` | string | Unique, stable account identifier — use as foreign key in local `accounts` table |
| `ProviderType` | string | Provider identifier: `"google"`, `"ms_graph"`, `"imap_smtp"`, etc. |
| `ProviderName` | string | Human-readable provider name for UI display |
| `ProviderIcon` | string | Serialised `GIcon` for provider branding |
| `Identity` | string | Provider-level identity (typically the email address) |
| `PresentationIdentity` | string | UI-suitable display string (typically the email address) |
| `MailDisabled` | boolean | If `true`, the user has disabled mail for this account — skip it |
| `AttentionNeeded` | boolean | If `true`, credentials are invalid — user must re-authenticate in GNOME Settings |
| `IsTemporary` | boolean | If `true`, account exists only for this session |

| Method | Signature | Description |
|---|---|---|
| `EnsureCredentials()` | `() → (expires_in: int32)` | Validates credentials are still good; returns seconds until expiry. Call before connecting. |

### 2.3 org.gnome.OnlineAccounts.Mail

Present on accounts that support email. Provides all IMAP and SMTP connection details.

**IMAP Properties:**

| Property | Type | Description |
|---|---|---|
| `ImapSupported` | boolean | `true` if IMAP is available for this account |
| `ImapHost` | string | IMAP server — hostname, IPv4, or IPv6; may include `:port` suffix |
| `ImapUserName` | string | Username for IMAP authentication; may be blank if auth is token-only |
| `ImapUseSsl` | boolean | If `true`, connect on port 993 with implicit TLS |
| `ImapUseTls` | boolean | If `true`, connect on port 143 then STARTTLS |
| `ImapAcceptSslErrors` | boolean | If `true`, skip certificate validation (user opted in) |
| `EmailAddress` | string | The account's email address |
| `Name` | string | Full name associated with the account (since GOA 3.8) |

**SMTP Properties:**

| Property | Type | Description |
|---|---|---|
| `SmtpSupported` | boolean | `true` if SMTP sending is available |
| `SmtpHost` | string | SMTP server; same format as `ImapHost` |
| `SmtpUserName` | string | Username for SMTP authentication |
| `SmtpUseAuth` | boolean | If `true`, SMTP requires authentication |
| `SmtpUseSsl` | boolean | If `true`, connect with implicit TLS (typically port 465) |
| `SmtpUseTls` | boolean | If `true`, use STARTTLS (typically port 587) |
| `SmtpAuthLogin` | boolean | Server supports LOGIN SASL mechanism |
| `SmtpAuthPlain` | boolean | Server supports PLAIN SASL mechanism |
| `SmtpAuthXoauth2` | boolean | Server supports XOAUTH2 (Gmail, Outlook) |
| `SmtpAcceptSslErrors` | boolean | Skip certificate validation |

### 2.4 org.gnome.OnlineAccounts.OAuth2Based

Present on OAuth providers (Google, Microsoft). Used to retrieve access tokens.

| Method | Signature | Description |
|---|---|---|
| `GetAccessToken()` | `() → (access_token: string, expires_in: int32)` | Returns a current OAuth 2.0 access token. GOA handles refresh transparently. `expires_in` is seconds until expiry, or 0 if unknown. |

| Property | Type | Description |
|---|---|---|
| `ClientId` | string | The OAuth client ID (GOA's, not ours) |
| `ClientSecret` | string | The OAuth client secret |

### 2.5 org.gnome.OnlineAccounts.PasswordBased

Present on password-authenticated accounts (generic IMAP/SMTP).

| Method | Signature | Description |
|---|---|---|
| `GetPassword(id: string)` | `(id: string) → (password: string)` | Retrieves the password for the given identifier. Known identifiers for mail: `"imap-password"`, `"smtp-password"`. |

---

## 3. Account Discovery Flow

### 3.1 Startup

```
1. Connect to session D-Bus
2. Call GetManagedObjects() on org.gnome.OnlineAccounts ObjectManager
3. For each returned object:
   a. Check: does it implement org.gnome.OnlineAccounts.Mail?
   b. Check: is MailDisabled == false?
   c. Check: is ImapSupported == true?
   d. If all yes → this is a usable mail account
4. For each usable account:
   a. Read Account.Id → use as stable identifier
   b. Read Account.ProviderType → determine auth strategy
   c. Read Mail.* properties → extract IMAP/SMTP config
   d. Read Account.AttentionNeeded → if true, show re-auth banner
   e. Upsert into local accounts table keyed by GOA Account.Id
5. For accounts in local DB that no longer appear in GOA:
   a. Mark as inactive (do not delete — user may re-add)
   b. Stop any active sync for that account
```

### 3.2 Runtime — Account Lifecycle Signals

Subscribe to `org.freedesktop.DBus.ObjectManager` signals on the GOA service:

| Signal | Action |
|---|---|
| `InterfacesAdded` | New account (or mail re-enabled on existing account). Read properties, upsert to local DB, start sync. |
| `InterfacesRemoved` | Account removed (or mail disabled). Mark inactive in local DB, stop sync, show notification. |
| `PropertiesChanged` on `Account` | `AttentionNeeded` changed to `true` → show "Re-authenticate in GNOME Settings" banner. `MailDisabled` changed → add or remove account from active set. |

### 3.3 Auth Strategy Selection

```
match account.provider_type {
    "google" | "ms_graph" | "windows_live" => {
        // OAuth provider — use OAuth2Based interface
        let (token, expires_in) = oauth2_proxy.get_access_token().await?;
        // IMAP auth: XOAUTH2 SASL with token
        // SMTP auth: XOAUTH2 SASL with token
    }
    "imap_smtp" | _ => {
        // Password provider — use PasswordBased interface
        let imap_password = password_proxy.get_password("imap-password").await?;
        let smtp_password = password_proxy.get_password("smtp-password").await?;
        // IMAP auth: SASL PLAIN with username + password
        // SMTP auth: SASL PLAIN with username + password
    }
}
```

---

## 4. Credential Retrieval & Refresh

### 4.1 OAuth Providers (Gmail, Microsoft 365)

**On sync engine connect/reconnect:**

```
1. Call EnsureCredentials() — verifies account is still valid
   - If error or AttentionNeeded → show re-auth banner, skip this account
2. Call OAuth2Based.GetAccessToken()
   - Returns (access_token, expires_in)
   - GOA handles token refresh internally — this call may trigger a refresh
3. Use access_token for IMAP AUTHENTICATE XOAUTH2
4. Cache expires_in to know when to re-fetch
```

**On token expiry during active session:**

```
1. IMAP server rejects command with auth error
2. Call GetAccessToken() again — GOA will have refreshed the token
3. Reconnect IMAP session with new token
4. If GetAccessToken() fails → AttentionNeeded is likely true → show re-auth banner
```

### 4.2 Password Providers (Generic IMAP)

**On sync engine connect/reconnect:**

```
1. Call EnsureCredentials()
2. Call PasswordBased.GetPassword("imap-password")
3. Call PasswordBased.GetPassword("smtp-password")  
4. Use for IMAP LOGIN or AUTHENTICATE PLAIN
```

Passwords don't expire in the same way as tokens, but the user may change their password externally. If IMAP auth fails:

```
1. Call EnsureCredentials() — this may trigger GOA to re-validate
2. If AttentionNeeded → show re-auth banner
3. If credentials still valid but auth fails → connection/server issue, retry with backoff
```

---

## 5. IMAP Connection Configuration

### 5.1 Deriving Connection Parameters from GOA Properties

```rust
struct ImapConfig {
    host: String,       // from ImapHost (strip port if present)
    port: u16,          // from ImapHost port suffix, or default based on SSL/TLS
    tls_mode: TlsMode, // from ImapUseSsl / ImapUseTls
    username: String,   // from ImapUserName
    accept_invalid_certs: bool, // from ImapAcceptSslErrors
}

enum TlsMode {
    Implicit,  // ImapUseSsl == true → connect with TLS on port 993
    StartTls,  // ImapUseTls == true → connect plain on port 143, then STARTTLS
    None,      // both false — should not happen for modern providers
}

enum AuthMethod {
    XOAuth2 { token: String },       // OAuth2Based providers
    Plain { username: String, password: String }, // PasswordBased providers
}
```

### 5.2 Port Resolution

```
if ImapHost contains ":" → parse explicit port from host string
else if ImapUseSsl → port 993
else if ImapUseTls → port 143
else → port 143 (with warning log)
```

Same pattern for SMTP:
```
if SmtpHost contains ":" → parse explicit port
else if SmtpUseSsl → port 465
else if SmtpUseTls → port 587
else → port 25 (with warning log)
```

---

## 6. Error Handling

### 6.1 GOA Unavailable

GOA daemon (`goa-daemon`) may not be running, or the D-Bus service may be unavailable (e.g., non-GNOME desktop environment running the Flatpak).

```
On startup:
  1. Attempt D-Bus connection to org.gnome.OnlineAccounts
  2. If service not found → show AdwStatusPage:
     "No accounts found. Add an email account in GNOME Settings → Online Accounts."
  3. Do not crash. The user may add an account later — listen for the service to appear.
```

### 6.2 Account Attention Needed

When `AttentionNeeded == true`, the account's credentials are invalid. GOA cannot fix this automatically — the user must re-authenticate in GNOME Settings.

```
- Show AdwBanner on the relevant account's folder/inbox:
  "{account_name} needs to be re-authenticated. Open Settings to fix this."
- Banner action button → launch GNOME Settings via D-Bus or `gio::AppInfo`
- Skip sync for this account until AttentionNeeded clears
- Continue syncing other accounts normally
```

### 6.3 Token Retrieval Failure

`GetAccessToken()` or `GetPassword()` may fail due to D-Bus errors, GOA daemon crashes, or keyring access issues.

```
- Log the error with account ID and provider type
- Retry with exponential backoff (1s, 2s, 4s, max 60s)
- After 3 consecutive failures → show error banner for that account
- Do not affect other accounts
```

### 6.4 Error Taxonomy

| Error | Source | Action |
|---|---|---|
| D-Bus service not found | GOA not installed/running | Status page, listen for service |
| `EnsureCredentials()` fails | Account needs re-auth | Set `AttentionNeeded` banner |
| `GetAccessToken()` fails | Token refresh failed | Retry with backoff, then banner |
| `GetPassword()` fails | Keyring access issue | Retry with backoff, then banner |
| IMAP auth rejected with valid token | Server-side issue or token race | Re-fetch token, retry once |
| IMAP auth rejected with valid password | Password changed externally | Call `EnsureCredentials()`, banner if needed |

---

## 7. Local Account State

### 7.1 Accounts Table

The local `accounts` table maps GOA account IDs to internal sync state. This table is created in the Phase 1 migration.

```sql
CREATE TABLE accounts (
    goa_id          TEXT PRIMARY KEY,   -- GOA Account.Id (stable across restarts)
    provider_type   TEXT NOT NULL,       -- "google", "ms_graph", "imap_smtp"
    email_address   TEXT NOT NULL,       -- from Mail.EmailAddress
    display_name    TEXT,                -- from Mail.Name
    imap_host       TEXT NOT NULL,
    imap_port       INTEGER NOT NULL,
    imap_tls_mode   TEXT NOT NULL,       -- "implicit", "starttls", "none"
    smtp_host       TEXT,
    smtp_port       INTEGER,
    smtp_tls_mode   TEXT,
    active          INTEGER NOT NULL DEFAULT 1,  -- 0 if removed from GOA
    last_sync       TEXT,                -- ISO 8601 timestamp of last successful sync
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
```

### 7.2 Sync on Account Changes

| Event | Action |
|---|---|
| New account discovered | Insert row, trigger initial full sync |
| Account removed from GOA | Set `active = 0`, stop sync. Preserve local messages (user may re-add). |
| Account re-added to GOA | Set `active = 1`, resume incremental sync from `last_sync` |
| Account properties changed | Update row (host/port may change if user reconfigured in GOA) |

---

## 8. Flatpak Permissions

The following D-Bus talk-names are required for GOA integration:

```
--talk-name=org.gnome.OnlineAccounts
```

This allows Epistle to:
- Call `GetManagedObjects()` to enumerate accounts
- Read account properties (Mail, Account interfaces)
- Call `GetAccessToken()` and `GetPassword()` for credential retrieval
- Subscribe to `InterfacesAdded`/`InterfacesRemoved`/`PropertiesChanged` signals

No additional permissions are needed. GOA handles keyring access internally — Epistle does not call `org.freedesktop.secrets` directly for email credentials.

**Note:** The architecture doc lists `--talk-name=org.freedesktop.secrets` in Flatpak permissions. This may still be needed if Epistle stores any non-GOA state in the keyring (e.g., app-specific preferences). If not, it can be removed to minimise the permission surface.

---

## 9. Implementation Approach — `zbus`

GOA's D-Bus interfaces are accessed from Rust via the `zbus` crate. The `#[proxy]` macro generates type-safe proxy structs from interface definitions.

### 9.1 Proxy Trait Definitions

The following traits will be defined to match GOA's D-Bus interfaces:

```rust
// org.gnome.OnlineAccounts.Account
#[zbus::proxy(
    interface = "org.gnome.OnlineAccounts.Account",
    default_service = "org.gnome.OnlineAccounts"
)]
trait GoaAccount {
    #[zbus(property)]
    fn id(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn provider_type(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn provider_name(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn presentation_identity(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn mail_disabled(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn attention_needed(&self) -> zbus::Result<bool>;

    fn ensure_credentials(&self) -> zbus::Result<i32>;
}

// org.gnome.OnlineAccounts.Mail
#[zbus::proxy(
    interface = "org.gnome.OnlineAccounts.Mail",
    default_service = "org.gnome.OnlineAccounts"
)]
trait GoaMail {
    #[zbus(property)]
    fn email_address(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn name(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn imap_supported(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn imap_host(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn imap_user_name(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn imap_use_ssl(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn imap_use_tls(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn smtp_supported(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn smtp_host(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn smtp_user_name(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn smtp_use_auth(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn smtp_use_ssl(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn smtp_use_tls(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn smtp_auth_xoauth2(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn smtp_auth_plain(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn smtp_auth_login(&self) -> zbus::Result<bool>;
}

// org.gnome.OnlineAccounts.OAuth2Based
#[zbus::proxy(
    interface = "org.gnome.OnlineAccounts.OAuth2Based",
    default_service = "org.gnome.OnlineAccounts"
)]
trait GoaOAuth2Based {
    fn get_access_token(&self) -> zbus::Result<(String, i32)>;
}

// org.gnome.OnlineAccounts.PasswordBased
#[zbus::proxy(
    interface = "org.gnome.OnlineAccounts.PasswordBased",
    default_service = "org.gnome.OnlineAccounts"
)]
trait GoaPasswordBased {
    fn get_password(&self, id: &str) -> zbus::Result<String>;
}
```

### 9.2 Alternative — `zbus_xmlgen`

The `zbus_xmlgen` tool can auto-generate proxy traits directly from GOA's `dbus-interfaces.xml` file. This avoids hand-writing the traits above and ensures they match the installed GOA version exactly:

```bash
zbus-xmlgen session org.gnome.OnlineAccounts /org/gnome/OnlineAccounts/Accounts/account_1234
```

Evaluate during spike whether hand-written or generated proxies are more maintainable.

### 9.3 Module Structure

The GOA integration code is kept in a self-contained module within Epistle, with a clean boundary that contains no Epistle-specific logic:

```
src/
  goa/
    mod.rs          -- public API: GoaClient, account enumeration, signal subscription
    proxies.rs      -- zbus proxy trait definitions (the 4 interfaces above)
    types.rs        -- ImapConfig, SmtpConfig, AuthMethod, ProviderType enums
```

This separation is intentional — see section 11.

### 9.4 Existing Crates

Two GOA crates exist on crates.io but neither is suitable:

| Crate | Version | Status |
|---|---|---|
| `goa` | 0.0.3 | GObject introspection FFI bindings via `goa-sys`. Pulls in the C `libgoa` dependency. Last updated 2023, stuck at GOA 3.26 feature level. |
| `gnome-online-accounts-rs` | 0.0.1 | Minimal D-Bus wrapper. v0.0.1 with no further development. |

Neither uses `zbus` for pure-Rust D-Bus access, and both appear unmaintained.

---

## 10. Future — Standalone `goa-zbus` Crate

### 10.1 Intent

The GOA module in Epistle should be extracted into a standalone, publishable crate once the codebase has matured and the API has stabilised through real-world usage. There is a clear gap in the Rust ecosystem — the existing `goa` and `gnome-online-accounts-rs` crates are unmaintained and use older approaches (C FFI bindings). A pure-Rust, `zbus`-based GOA crate would benefit any Rust application integrating with GNOME Online Accounts.

### 10.2 Why Not Now

- The convenience API layer (account enumeration helpers, typed config structs, error types) needs to be shaped by real usage in Epistle, not designed speculatively
- Extracting from a working app produces a better crate than designing in a vacuum
- The proxy traits alone (~80 lines) aren't enough to justify a standalone crate — the value is in the higher-level client API that emerges during implementation

### 10.3 Extraction Criteria

Extract into a standalone crate when:

- [ ] Epistle v1 has shipped and the GOA module API has been stable for at least one release cycle
- [ ] The module has been exercised against Gmail, Microsoft 365, and generic IMAP providers
- [ ] Error handling patterns have been validated in production use
- [ ] The public API surface is clean enough that a third-party consumer could use it without reading Epistle's source

### 10.4 Crate Scope (Projected)

The extracted crate would provide:

- `zbus` proxy traits for all GOA D-Bus interfaces (Account, Mail, OAuth2Based, PasswordBased, and potentially Calendar, Contacts for other consumers)
- A `GoaClient` struct that handles D-Bus connection, account enumeration, filtering by service type, and signal subscription
- Typed structs: `ImapConfig`, `SmtpConfig`, `AuthMethod`, `ProviderType`
- Async-first API compatible with both Tokio and `glib::MainContext` runtimes
- No application-specific logic — pure GOA abstraction

### 10.5 Keeping Extraction Easy

To ensure clean extraction later, the following rules apply during Epistle development:

- The `goa/` module must not import from any other Epistle module
- All Epistle-specific logic (mapping to local DB, sync engine integration) lives outside the `goa/` module
- The module's public API uses its own types, not Epistle domain types
- Dependencies are limited to `zbus` and standard library — no GTK, no SQLite, no Epistle crates
- **CI enforcement:** Add a CI job from first commit using `cargo modules` or `cargo depgraph` to verify that `epistle::goa` has no imports from other `epistle::*` modules. This makes the isolation guarantee mechanical rather than relying on code review alone. The same check should apply to `epistle::threading`.

---

## 11. Spike Validation

Before full implementation, the following must be validated in a minimal Flatpak app:

- [ ] `GetManagedObjects()` returns account objects from inside Flatpak sandbox
- [ ] Can filter for `org.gnome.OnlineAccounts.Mail` implementing objects
- [ ] Can read all `Mail.*` properties for a Gmail account
- [ ] Can call `OAuth2Based.GetAccessToken()` for a Gmail account and receive a valid token
- [ ] Can use that token with `async-imap` to authenticate an IMAP session (XOAUTH2)
- [ ] Can call `PasswordBased.GetPassword("imap-password")` for a generic IMAP account
- [ ] Can use that password with `async-imap` to authenticate an IMAP session (PLAIN)
- [ ] Can subscribe to `InterfacesAdded` / `InterfacesRemoved` signals and detect account changes
- [ ] `AttentionNeeded` property is correctly reported when an account needs re-auth
- [ ] Validate `EnsureCredentials()` return type handling in `zbus` 4.x (single-element D-Bus tuple unwrapping for `i32` return)
- [ ] Validate that all proxy methods use `async fn` correctly with `zbus` 4.x
- [ ] All of the above works with `--talk-name=org.gnome.OnlineAccounts` as the only GOA-related Flatpak permission
