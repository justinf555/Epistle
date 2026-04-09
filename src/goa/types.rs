use thiserror::Error;

// ── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum GoaError {
    #[error("D-Bus error: {0}")]
    DBus(#[from] zbus::Error),

    #[error("D-Bus fdo error: {0}")]
    Fdo(#[from] zbus::fdo::Error),

    #[error("account not found: {goa_id}")]
    AccountNotFound { goa_id: String },

    #[error("account {goa_id} does not support mail")]
    MailNotSupported { goa_id: String },

    #[error("failed to parse port from host string: {host}")]
    PortParse { host: String },

    #[error("credentials unavailable for {goa_id}: {reason}")]
    CredentialUnavailable { goa_id: String, reason: String },
}

// ── Enums ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsMode {
    Implicit,
    StartTls,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderType {
    Google,
    Microsoft,
    GenericImap,
}

impl ProviderType {
    pub fn from_goa_string(s: &str) -> Self {
        match s {
            "google" => Self::Google,
            "ms_graph" | "windows_live" => Self::Microsoft,
            _ => Self::GenericImap,
        }
    }

    pub fn is_oauth(&self) -> bool {
        matches!(self, Self::Google | Self::Microsoft)
    }

    pub fn as_goa_str(&self) -> &'static str {
        match self {
            Self::Google => "google",
            Self::Microsoft => "ms_graph",
            Self::GenericImap => "imap_smtp",
        }
    }
}

#[derive(Clone)]
pub enum AuthMethod {
    XOAuth2 { token: String },
    Plain { username: String, password: String },
}

impl std::fmt::Debug for AuthMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthMethod::XOAuth2 { .. } => write!(f, "AuthMethod::XOAuth2(<redacted>)"),
            AuthMethod::Plain { username, .. } => {
                write!(f, "AuthMethod::Plain {{ username: {username:?}, password: <redacted> }}")
            }
        }
    }
}

// ── Config Structs ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImapConfig {
    pub host: String,
    pub port: u16,
    pub tls_mode: TlsMode,
    pub username: String,
    pub accept_invalid_certs: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub tls_mode: TlsMode,
    pub username: String,
    pub accept_invalid_certs: bool,
    pub auth_mechanisms: SmtpAuthMechanisms,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmtpAuthMechanisms {
    pub xoauth2: bool,
    pub plain: bool,
    pub login: bool,
}

// ── Aggregate Account ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct GoaMailAccount {
    pub goa_id: String,
    pub provider_type: ProviderType,
    pub provider_name: String,
    pub email_address: String,
    pub display_name: Option<String>,
    pub presentation_identity: String,
    pub attention_needed: bool,
    pub imap_config: ImapConfig,
    pub smtp_config: Option<SmtpConfig>,
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Parse a host string that may contain a `:port` suffix.
/// Returns `(host, port)`, using `default_port` when no suffix is present.
pub fn parse_host_port(raw: &str, default_port: u16) -> Result<(String, u16), GoaError> {
    // Handle IPv6 addresses like [::1]:993
    if let Some(bracket_end) = raw.find(']') {
        let after = &raw[bracket_end + 1..];
        if let Some(port_str) = after.strip_prefix(':') {
            let port = port_str
                .parse::<u16>()
                .map_err(|_| GoaError::PortParse { host: raw.to_string() })?;
            return Ok((raw[..bracket_end + 1].to_string(), port));
        }
        return Ok((raw[..bracket_end + 1].to_string(), default_port));
    }

    // Regular host:port or plain hostname
    match raw.rsplit_once(':') {
        Some((host, port_str)) => match port_str.parse::<u16>() {
            Ok(port) => Ok((host.to_string(), port)),
            Err(_) => Ok((raw.to_string(), default_port)),
        },
        None => Ok((raw.to_string(), default_port)),
    }
}

/// Determine TLS mode from GOA's `use_ssl` and `use_tls` boolean properties.
pub fn resolve_tls_mode(use_ssl: bool, use_tls: bool) -> TlsMode {
    if use_ssl {
        TlsMode::Implicit
    } else if use_tls {
        TlsMode::StartTls
    } else {
        TlsMode::None
    }
}

/// Default IMAP port for a given TLS mode.
pub fn default_imap_port(tls_mode: TlsMode) -> u16 {
    match tls_mode {
        TlsMode::Implicit => 993,
        TlsMode::StartTls | TlsMode::None => 143,
    }
}

/// Default SMTP port for a given TLS mode.
pub fn default_smtp_port(tls_mode: TlsMode) -> u16 {
    match tls_mode {
        TlsMode::Implicit => 465,
        TlsMode::StartTls => 587,
        TlsMode::None => 25,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_host_port_with_port() {
        let (host, port) = parse_host_port("imap.gmail.com:993", 143).unwrap();
        assert_eq!(host, "imap.gmail.com");
        assert_eq!(port, 993);
    }

    #[test]
    fn parse_host_port_without_port() {
        let (host, port) = parse_host_port("imap.gmail.com", 993).unwrap();
        assert_eq!(host, "imap.gmail.com");
        assert_eq!(port, 993);
    }

    #[test]
    fn parse_host_port_ipv6() {
        let (host, port) = parse_host_port("[::1]:993", 143).unwrap();
        assert_eq!(host, "[::1]");
        assert_eq!(port, 993);
    }

    #[test]
    fn resolve_tls_ssl() {
        assert_eq!(resolve_tls_mode(true, false), TlsMode::Implicit);
    }

    #[test]
    fn resolve_tls_starttls() {
        assert_eq!(resolve_tls_mode(false, true), TlsMode::StartTls);
    }

    #[test]
    fn provider_type_mapping() {
        assert_eq!(ProviderType::from_goa_string("google"), ProviderType::Google);
        assert_eq!(ProviderType::from_goa_string("ms_graph"), ProviderType::Microsoft);
        assert_eq!(ProviderType::from_goa_string("imap_smtp"), ProviderType::GenericImap);
    }
}
