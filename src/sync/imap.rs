use async_imap::types::{Flag, NameAttribute};
use futures::TryStreamExt;
use tokio::net::TcpStream;
use tokio_native_tls::TlsConnector;

use crate::engine::pipeline::{RawAddress, RawEmail};
use crate::goa::types::{AuthMethod, ImapConfig, TlsMode};
use crate::sync::pool::ImapSession;

use thiserror::Error;
use tokio_native_tls::native_tls;

#[derive(Debug, Error)]
pub enum ImapError {
    #[error("IMAP error: {0}")]
    Imap(#[from] async_imap::error::Error),

    #[error("TLS error: {0}")]
    Tls(#[from] native_tls::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("authentication failed: {0}")]
    Auth(String),

    #[error("message not found: UID {uid}")]
    MessageNotFound { uid: u32 },
}

impl ImapError {
    /// Whether this error indicates an authentication failure.
    pub fn is_auth_error(&self) -> bool {
        matches!(self, Self::Auth(_))
    }
}

/// Connect to an IMAP server and authenticate, returning a type-erased session.
///
/// Handles all three TLS modes (implicit, STARTTLS, plain).
pub async fn connect(config: &ImapConfig, auth: &AuthMethod) -> Result<ImapSession, ImapError> {
    match config.tls_mode {
        TlsMode::Implicit => {
            let tcp = TcpStream::connect((&*config.host, config.port)).await?;
            let tls = tls_connector(config)?;
            let tls_stream = tls.connect(&config.host, tcp).await.map_err(|e| {
                ImapError::Io(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, e))
            })?;
            let mut client = async_imap::Client::new(tls_stream);
            client.read_response().await.transpose()?;
            let session = authenticate(client, auth).await?;
            Ok(ImapSession::Tls(session))
        }
        TlsMode::StartTls => {
            let tcp = TcpStream::connect((&*config.host, config.port)).await?;
            let mut client = async_imap::Client::new(tcp);
            client.read_response().await.transpose()?;
            client
                .run_command_and_check_ok("STARTTLS", None)
                .await
                .map_err(async_imap::error::Error::from)?;
            let inner = client.into_inner();
            let tls = tls_connector(config)?;
            let tls_stream = tls.connect(&config.host, inner).await.map_err(|e| {
                ImapError::Io(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, e))
            })?;
            let client = async_imap::Client::new(tls_stream);
            let session = authenticate(client, auth).await?;
            Ok(ImapSession::Tls(session))
        }
        TlsMode::None => {
            tracing::warn!(
                host = %config.host,
                "Connecting to IMAP without TLS — credentials will be sent in plaintext"
            );
            let tcp = TcpStream::connect((&*config.host, config.port)).await?;
            let mut client = async_imap::Client::new(tcp);
            client.read_response().await.transpose()?;
            let session = authenticate(client, auth).await?;
            Ok(ImapSession::Plain(session))
        }
    }
}

/// A folder discovered from IMAP LIST.
#[derive(Debug, Clone)]
pub struct ImapFolder {
    pub name: String,
    pub delimiter: Option<String>,
    pub role: Option<String>,
}

/// Connect to an IMAP server, authenticate, list folders, and disconnect.
///
/// This is a single-shot operation: connect → auth → LIST → logout.
pub async fn discover_folders(
    config: &ImapConfig,
    auth: &AuthMethod,
) -> Result<Vec<ImapFolder>, ImapError> {
    match config.tls_mode {
        TlsMode::Implicit => discover_implicit(config, auth).await,
        TlsMode::StartTls => discover_starttls(config, auth).await,
        TlsMode::None => {
            tracing::warn!(host = %config.host, "Connecting to IMAP without TLS — credentials will be sent in plaintext");
            discover_plain(config, auth).await
        }
    }
}

async fn discover_implicit(
    config: &ImapConfig,
    auth: &AuthMethod,
) -> Result<Vec<ImapFolder>, ImapError> {
    let tcp = TcpStream::connect((&*config.host, config.port)).await?;
    let tls = tls_connector(config)?;
    let tls_stream = tls.connect(&config.host, tcp).await.map_err(|e| {
        ImapError::Io(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, e))
    })?;

    let mut client = async_imap::Client::new(tls_stream);
    client.read_response().await.transpose()?;

    let mut session = authenticate(client, auth).await?;
    let folders = list_folders(&mut session).await?;
    session.logout().await?;
    Ok(folders)
}

async fn discover_starttls(
    config: &ImapConfig,
    auth: &AuthMethod,
) -> Result<Vec<ImapFolder>, ImapError> {
    let tcp = TcpStream::connect((&*config.host, config.port)).await?;
    let mut client = async_imap::Client::new(tcp);
    client.read_response().await.transpose()?;
    client
        .run_command_and_check_ok("STARTTLS", None)
        .await
        .map_err(async_imap::error::Error::from)?;

    let inner = client.into_inner();
    let tls = tls_connector(config)?;
    let tls_stream = tls.connect(&config.host, inner).await.map_err(|e| {
        ImapError::Io(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, e))
    })?;

    let client = async_imap::Client::new(tls_stream);
    let mut session = authenticate(client, auth).await?;
    let folders = list_folders(&mut session).await?;
    session.logout().await?;
    Ok(folders)
}

async fn discover_plain(
    config: &ImapConfig,
    auth: &AuthMethod,
) -> Result<Vec<ImapFolder>, ImapError> {
    let tcp = TcpStream::connect((&*config.host, config.port)).await?;
    let mut client = async_imap::Client::new(tcp);
    client.read_response().await.transpose()?;

    let mut session = authenticate(client, auth).await?;
    let folders = list_folders(&mut session).await?;
    session.logout().await?;
    Ok(folders)
}

fn tls_connector(config: &ImapConfig) -> Result<TlsConnector, ImapError> {
    let mut builder = native_tls::TlsConnector::builder();
    if config.accept_invalid_certs {
        builder.danger_accept_invalid_certs(true);
    }
    let connector = builder.build()?;
    Ok(TlsConnector::from(connector))
}

async fn authenticate<T>(
    client: async_imap::Client<T>,
    auth: &AuthMethod,
) -> Result<async_imap::Session<T>, ImapError>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    match auth {
        AuthMethod::Plain { username, password } => client
            .login(username, password)
            .await
            .map_err(|(e, _)| ImapError::Auth(e.to_string())),
        AuthMethod::XOAuth2 { token } => {
            let authenticator = XOAuth2Auth { token };
            client
                .authenticate("XOAUTH2", authenticator)
                .await
                .map_err(|(e, _)| ImapError::Auth(e.to_string()))
        }
    }
}

async fn list_folders<T>(
    session: &mut async_imap::Session<T>,
) -> Result<Vec<ImapFolder>, ImapError>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let names: Vec<_> = session
        .list(Some(""), Some("*"))
        .await?
        .try_collect()
        .await?;

    let mut folders = Vec::new();
    for name in &names {
        if name
            .attributes()
            .iter()
            .any(|a| matches!(a, NameAttribute::NoSelect))
        {
            continue;
        }

        let role = detect_role(name.name(), name.attributes());
        folders.push(ImapFolder {
            name: name.name().to_string(),
            delimiter: name.delimiter().map(|s| s.to_string()),
            role,
        });
    }

    Ok(folders)
}

/// Detect folder role from IMAP LIST attributes (RFC 6154) with name fallbacks.
fn detect_role(name: &str, attrs: &[NameAttribute<'_>]) -> Option<String> {
    for attr in attrs {
        let role = match attr {
            NameAttribute::Extension(s) if s.eq_ignore_ascii_case("\\Inbox") => "inbox",
            NameAttribute::Sent => "sent",
            NameAttribute::Drafts => "drafts",
            NameAttribute::Archive => "archive",
            NameAttribute::Trash => "trash",
            NameAttribute::Junk => "junk",
            _ => continue,
        };
        return Some(role.to_string());
    }

    // Fallback: detect by name for servers without RFC 6154 support
    if name.eq_ignore_ascii_case("INBOX") {
        return Some("inbox".to_string());
    }

    None
}

// ── Fetch Response Parsing ──────────────────────────────────────────────────

/// Convert an `async_imap::types::Fetch` response into a [`RawEmail`].
///
/// Reusable across both one-shot connections and pool-based operations.
pub fn fetch_to_raw_email(fetch: &async_imap::types::Fetch) -> Option<RawEmail> {
    let uid = fetch.uid?;

    let flags: Vec<String> = fetch
        .flags()
        .map(|f| match f {
            Flag::Seen => "\\Seen".to_string(),
            Flag::Answered => "\\Answered".to_string(),
            Flag::Flagged => "\\Flagged".to_string(),
            Flag::Deleted => "\\Deleted".to_string(),
            Flag::Draft => "\\Draft".to_string(),
            Flag::Recent => "\\Recent".to_string(),
            Flag::MayCreate => "\\MayCreate".to_string(),
            Flag::Custom(s) => s.to_string(),
        })
        .collect();

    let mut raw = RawEmail {
        uid,
        flags,
        subject: None,
        from: None,
        to: None,
        cc: None,
        date: None,
        message_id: None,
        in_reply_to: None,
        internal_date: fetch.internal_date().map(|dt| dt.to_rfc3339()),
        has_attachments: None,
        body_text: None,
    };

    if let Some(envelope) = fetch.envelope() {
        raw.subject = envelope.subject.as_ref().map(|s| s.to_vec());
        raw.date = envelope.date.as_ref().map(|d| d.to_vec());
        raw.message_id = envelope.message_id.as_ref().map(|m| m.to_vec());
        raw.in_reply_to = envelope.in_reply_to.as_ref().map(|r| r.to_vec());

        raw.from = envelope.from.as_ref().map(|addrs| {
            addrs.iter().map(imap_addr_to_raw).collect()
        });
        raw.to = envelope.to.as_ref().map(|addrs| {
            addrs.iter().map(imap_addr_to_raw).collect()
        });
        raw.cc = envelope.cc.as_ref().map(|addrs| {
            addrs.iter().map(imap_addr_to_raw).collect()
        });
    }

    Some(raw)
}

fn imap_addr_to_raw(addr: &async_imap::imap_proto::types::Address<'_>) -> RawAddress {
    RawAddress {
        name: addr.name.as_ref().map(|n: &std::borrow::Cow<'_, [u8]>| n.to_vec()),
        mailbox: addr.mailbox.as_ref().map(|m: &std::borrow::Cow<'_, [u8]>| m.to_vec()),
        host: addr.host.as_ref().map(|h: &std::borrow::Cow<'_, [u8]>| h.to_vec()),
    }
}

/// XOAUTH2 SASL authenticator for OAuth providers (Gmail, Microsoft).
struct XOAuth2Auth<'a> {
    token: &'a str,
}

impl async_imap::Authenticator for XOAuth2Auth<'_> {
    type Response = Vec<u8>;

    fn process(&mut self, _challenge: &[u8]) -> Self::Response {
        format!("user=\x01auth=Bearer {}\x01\x01", self.token).into_bytes()
    }
}
