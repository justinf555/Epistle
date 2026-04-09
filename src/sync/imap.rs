use async_imap::types::{Flag, NameAttribute};
use futures::TryStreamExt;
use tokio::net::TcpStream;
use tokio_native_tls::TlsConnector;

use crate::engine::pipeline::{RawAddress, RawEmail};
use crate::goa::types::{AuthMethod, ImapConfig, TlsMode};

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
        TlsMode::None => discover_plain(config, auth).await,
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

// ── Message Fetching ────────────────────────────────────────────────────────

/// Connect to an IMAP server, SELECT the given folder, FETCH message
/// envelopes + flags + body structure in batches, and disconnect.
///
/// Returns a Vec of [`RawEmail`] ready for the processing pipeline.
/// Fetches in batches of `batch_size` UIDs, newest first.
pub async fn fetch_messages(
    config: &ImapConfig,
    auth: &AuthMethod,
    folder: &str,
    batch_size: u32,
) -> Result<Vec<RawEmail>, ImapError> {
    match config.tls_mode {
        TlsMode::Implicit => fetch_messages_implicit(config, auth, folder, batch_size).await,
        TlsMode::StartTls => fetch_messages_starttls(config, auth, folder, batch_size).await,
        TlsMode::None => fetch_messages_plain(config, auth, folder, batch_size).await,
    }
}

async fn fetch_messages_implicit(
    config: &ImapConfig,
    auth: &AuthMethod,
    folder: &str,
    batch_size: u32,
) -> Result<Vec<RawEmail>, ImapError> {
    let tcp = TcpStream::connect((&*config.host, config.port)).await?;
    let tls = tls_connector(config)?;
    let tls_stream = tls.connect(&config.host, tcp).await.map_err(|e| {
        ImapError::Io(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, e))
    })?;

    let mut client = async_imap::Client::new(tls_stream);
    client.read_response().await.transpose()?;

    let mut session = authenticate(client, auth).await?;
    let messages = fetch_from_session(&mut session, folder, batch_size).await?;
    session.logout().await?;
    Ok(messages)
}

async fn fetch_messages_starttls(
    config: &ImapConfig,
    auth: &AuthMethod,
    folder: &str,
    batch_size: u32,
) -> Result<Vec<RawEmail>, ImapError> {
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
    let messages = fetch_from_session(&mut session, folder, batch_size).await?;
    session.logout().await?;
    Ok(messages)
}

async fn fetch_messages_plain(
    config: &ImapConfig,
    auth: &AuthMethod,
    folder: &str,
    batch_size: u32,
) -> Result<Vec<RawEmail>, ImapError> {
    let tcp = TcpStream::connect((&*config.host, config.port)).await?;
    let mut client = async_imap::Client::new(tcp);
    client.read_response().await.transpose()?;

    let mut session = authenticate(client, auth).await?;
    let messages = fetch_from_session(&mut session, folder, batch_size).await?;
    session.logout().await?;
    Ok(messages)
}

/// SELECT folder, then FETCH envelopes in batches of UIDs (newest first).
async fn fetch_from_session<T>(
    session: &mut async_imap::Session<T>,
    folder: &str,
    batch_size: u32,
) -> Result<Vec<RawEmail>, ImapError>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let mailbox = session.select(folder).await?;
    let exists = mailbox.exists;

    if exists == 0 {
        return Ok(vec![]);
    }

    let mut all_messages = Vec::new();

    // Fetch in batches, newest first (highest sequence numbers first)
    let mut end = exists;
    while end > 0 {
        let start = if end > batch_size { end - batch_size + 1 } else { 1 };
        let range = format!("{}:{}", start, end);

        // BODYSTRUCTURE omitted — imap-proto's parser fails on complex nested
        // MIME structures (e.g. multipart/related with inline images and
        // MIME-encoded filenames). Attachment detection deferred to Phase 2
        // when we parse the body ourselves with mail-parser.
        let fetches: Vec<_> = session
            .fetch(&range, "(UID ENVELOPE FLAGS INTERNALDATE)")
            .await?
            .try_collect()
            .await?;

        for fetch in &fetches {
            let uid = match fetch.uid {
                Some(uid) => uid,
                None => continue,
            };

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

            all_messages.push(raw);
        }

        if start == 1 {
            break;
        }
        end = start - 1;
    }

    Ok(all_messages)
}

fn imap_addr_to_raw(addr: &async_imap::imap_proto::types::Address<'_>) -> RawAddress {
    RawAddress {
        name: addr.name.as_ref().map(|n: &std::borrow::Cow<'_, [u8]>| n.to_vec()),
        mailbox: addr.mailbox.as_ref().map(|m: &std::borrow::Cow<'_, [u8]>| m.to_vec()),
        host: addr.host.as_ref().map(|h: &std::borrow::Cow<'_, [u8]>| h.to_vec()),
    }
}

// ── Single-Message Body Fetch ────────────────────────────────────────────────

/// Connect to IMAP, SELECT the folder, UID FETCH the full message body,
/// and return the raw RFC 5322 bytes. Single-shot connection.
pub async fn fetch_message_body(
    config: &ImapConfig,
    auth: &AuthMethod,
    folder: &str,
    uid: u32,
) -> Result<Vec<u8>, ImapError> {
    match config.tls_mode {
        TlsMode::Implicit => fetch_body_implicit(config, auth, folder, uid).await,
        TlsMode::StartTls => fetch_body_starttls(config, auth, folder, uid).await,
        TlsMode::None => fetch_body_plain(config, auth, folder, uid).await,
    }
}

async fn fetch_body_implicit(
    config: &ImapConfig,
    auth: &AuthMethod,
    folder: &str,
    uid: u32,
) -> Result<Vec<u8>, ImapError> {
    let tcp = TcpStream::connect((&*config.host, config.port)).await?;
    let tls = tls_connector(config)?;
    let tls_stream = tls.connect(&config.host, tcp).await.map_err(|e| {
        ImapError::Io(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, e))
    })?;

    let mut client = async_imap::Client::new(tls_stream);
    client.read_response().await.transpose()?;

    let mut session = authenticate(client, auth).await?;
    let body = fetch_body_from_session(&mut session, folder, uid).await?;
    session.logout().await?;
    Ok(body)
}

async fn fetch_body_starttls(
    config: &ImapConfig,
    auth: &AuthMethod,
    folder: &str,
    uid: u32,
) -> Result<Vec<u8>, ImapError> {
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
    let body = fetch_body_from_session(&mut session, folder, uid).await?;
    session.logout().await?;
    Ok(body)
}

async fn fetch_body_plain(
    config: &ImapConfig,
    auth: &AuthMethod,
    folder: &str,
    uid: u32,
) -> Result<Vec<u8>, ImapError> {
    let tcp = TcpStream::connect((&*config.host, config.port)).await?;
    let mut client = async_imap::Client::new(tcp);
    client.read_response().await.transpose()?;

    let mut session = authenticate(client, auth).await?;
    let body = fetch_body_from_session(&mut session, folder, uid).await?;
    session.logout().await?;
    Ok(body)
}

/// SELECT folder, UID FETCH the full message body for a single UID.
async fn fetch_body_from_session<T>(
    session: &mut async_imap::Session<T>,
    folder: &str,
    uid: u32,
) -> Result<Vec<u8>, ImapError>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    session.select(folder).await?;

    let uid_str = uid.to_string();
    let fetches: Vec<_> = session
        .uid_fetch(&uid_str, "BODY[]")
        .await?
        .try_collect()
        .await?;

    for fetch in &fetches {
        if let Some(body) = fetch.body() {
            tracing::debug!(uid, bytes = body.len(), "Fetched message body");
            return Ok(body.to_vec());
        }
    }

    Err(ImapError::Auth(format!("No body returned for UID {uid}")))
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
