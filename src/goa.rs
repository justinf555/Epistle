//! GNOME Online Accounts D-Bus integration.
//!
//! This module is fully self-contained — it does not import from any other
//! Epistle module. It is designed for future extraction into a standalone crate.

mod proxies;
pub mod types;

use std::collections::HashMap;

use zbus::fdo::ObjectManagerProxy;
use zbus::names::OwnedInterfaceName;
use zbus::zvariant::OwnedObjectPath;
use zbus::Connection;

use proxies::{GoaAccountProxy, GoaMailProxy, GoaOAuth2BasedProxy, GoaPasswordBasedProxy};
pub use types::*;

const GOA_SERVICE: &str = "org.gnome.OnlineAccounts";
const GOA_MANAGER_PATH: &str = "/org/gnome/OnlineAccounts";
const MAIL_INTERFACE: &str = "org.gnome.OnlineAccounts.Mail";

#[derive(Debug)]
pub struct GoaClient {
    connection: Connection,
    /// Cached mapping of GOA account ID → D-Bus object path.
    account_paths: HashMap<String, OwnedObjectPath>,
}

impl GoaClient {
    /// Connect to the session D-Bus and create a new GOA client.
    pub async fn new() -> Result<Self, GoaError> {
        let connection = Connection::session().await?;
        Ok(Self {
            connection,
            account_paths: HashMap::new(),
        })
    }

    /// Discover all mail-enabled, non-disabled, IMAP-supported accounts from GOA.
    ///
    /// Calls `GetManagedObjects()` on the GOA ObjectManager, filters for accounts
    /// with the Mail interface, and parses their properties into `GoaMailAccount`s.
    /// Also populates the internal account path cache for credential retrieval.
    pub async fn discover_accounts(&mut self) -> Result<Vec<GoaMailAccount>, GoaError> {
        let manager = ObjectManagerProxy::builder(&self.connection)
            .destination(GOA_SERVICE)?
            .path(GOA_MANAGER_PATH)?
            .build()
            .await?;

        let objects = manager.get_managed_objects().await?;

        let mail_iface: OwnedInterfaceName = MAIL_INTERFACE
            .try_into()
            .expect("valid interface name");

        let mut accounts = Vec::new();
        self.account_paths.clear();

        for (path, interfaces) in &objects {
            // Skip objects that don't implement the Mail interface
            if !interfaces.contains_key(&mail_iface) {
                continue;
            }

            match self.read_account(path).await {
                Ok(Some(account)) => {
                    self.account_paths
                        .insert(account.goa_id.clone(), path.clone());
                    accounts.push(account);
                }
                Ok(None) => {} // filtered out (disabled or no IMAP)
                Err(e) => {
                    // Log and skip individual account failures
                    eprintln!("warning: skipping GOA account at {path}: {e}");
                }
            }
        }

        Ok(accounts)
    }

    /// Read a single account's properties from D-Bus proxies and filter.
    /// Returns `None` if the account is disabled or doesn't support IMAP.
    async fn read_account(
        &self,
        path: &OwnedObjectPath,
    ) -> Result<Option<GoaMailAccount>, GoaError> {
        let account_proxy = GoaAccountProxy::builder(&self.connection)
            .path(path)?
            .build()
            .await?;

        // Filter: skip disabled accounts
        if account_proxy.mail_disabled().await.unwrap_or(true) {
            return Ok(None);
        }

        let mail_proxy = GoaMailProxy::builder(&self.connection)
            .path(path)?
            .build()
            .await?;

        // Filter: skip accounts without IMAP support
        if !mail_proxy.imap_supported().await.unwrap_or(false) {
            return Ok(None);
        }

        let goa_id = account_proxy.id().await?;
        let provider_type = ProviderType::from_goa_string(&account_proxy.provider_type().await?);
        let provider_name = account_proxy.provider_name().await?;
        let presentation_identity = account_proxy.presentation_identity().await?;
        let attention_needed = account_proxy.attention_needed().await.unwrap_or(false);

        let email_address = mail_proxy.email_address().await?;
        let display_name = mail_proxy.name().await.ok().filter(|s| !s.is_empty());

        let imap_config = self.read_imap_config(&mail_proxy).await?;
        let smtp_config = self.read_smtp_config(&mail_proxy).await?;

        Ok(Some(GoaMailAccount {
            goa_id,
            provider_type,
            provider_name,
            email_address,
            display_name,
            presentation_identity,
            attention_needed,
            imap_config,
            smtp_config,
        }))
    }

    async fn read_imap_config(
        &self,
        mail: &GoaMailProxy<'_>,
    ) -> Result<ImapConfig, GoaError> {
        let use_ssl = mail.imap_use_ssl().await.unwrap_or(false);
        let use_tls = mail.imap_use_tls().await.unwrap_or(false);
        let tls_mode = resolve_tls_mode(use_ssl, use_tls);
        let default_port = default_imap_port(tls_mode);

        let raw_host = mail.imap_host().await?;
        let (host, port) = parse_host_port(&raw_host, default_port)?;
        let username = mail.imap_user_name().await.unwrap_or_default();
        let accept_invalid_certs = mail.imap_accept_ssl_errors().await.unwrap_or(false);

        Ok(ImapConfig {
            host,
            port,
            tls_mode,
            username,
            accept_invalid_certs,
        })
    }

    async fn read_smtp_config(
        &self,
        mail: &GoaMailProxy<'_>,
    ) -> Result<Option<SmtpConfig>, GoaError> {
        if !mail.smtp_supported().await.unwrap_or(false) {
            return Ok(None);
        }

        let use_ssl = mail.smtp_use_ssl().await.unwrap_or(false);
        let use_tls = mail.smtp_use_tls().await.unwrap_or(false);
        let tls_mode = resolve_tls_mode(use_ssl, use_tls);
        let default_port = default_smtp_port(tls_mode);

        let raw_host = mail.smtp_host().await?;
        let (host, port) = parse_host_port(&raw_host, default_port)?;
        let username = mail.smtp_user_name().await.unwrap_or_default();
        let accept_invalid_certs = mail.smtp_accept_ssl_errors().await.unwrap_or(false);

        let auth_mechanisms = SmtpAuthMechanisms {
            xoauth2: mail.smtp_auth_xoauth2().await.unwrap_or(false),
            plain: mail.smtp_auth_plain().await.unwrap_or(false),
            login: mail.smtp_auth_login().await.unwrap_or(false),
        };

        Ok(Some(SmtpConfig {
            host,
            port,
            tls_mode,
            username,
            accept_invalid_certs,
            auth_mechanisms,
        }))
    }

    /// Get the D-Bus object path for a GOA account ID.
    /// Requires `discover_accounts()` to have been called first.
    fn object_path(&self, goa_id: &str) -> Result<&OwnedObjectPath, GoaError> {
        self.account_paths
            .get(goa_id)
            .ok_or_else(|| GoaError::AccountNotFound {
                goa_id: goa_id.to_string(),
            })
    }

    /// Retrieve IMAP authentication credentials for the given account.
    /// Uses OAuth2 for Google/Microsoft, password for generic IMAP.
    pub async fn get_imap_auth(&self, goa_id: &str) -> Result<AuthMethod, GoaError> {
        let path = self.object_path(goa_id)?;

        let account_proxy = GoaAccountProxy::builder(&self.connection)
            .path(path)?
            .build()
            .await?;

        let provider_type =
            ProviderType::from_goa_string(&account_proxy.provider_type().await?);

        if provider_type.is_oauth() {
            let oauth_proxy = GoaOAuth2BasedProxy::builder(&self.connection)
                .path(path)?
                .build()
                .await?;

            let (token, _expires_in) = oauth_proxy.get_access_token().await.map_err(|_| {
                GoaError::CredentialUnavailable {
                    goa_id: goa_id.to_string(),
                    reason: "OAuth2 GetAccessToken failed".to_string(),
                }
            })?;

            Ok(AuthMethod::XOAuth2 { token })
        } else {
            let password_proxy = GoaPasswordBasedProxy::builder(&self.connection)
                .path(path)?
                .build()
                .await?;

            let password =
                password_proxy
                    .get_password("imap-password")
                    .await
                    .map_err(|_| GoaError::CredentialUnavailable {
                        goa_id: goa_id.to_string(),
                        reason: "PasswordBased GetPassword failed".to_string(),
                    })?;

            let username = GoaMailProxy::builder(&self.connection)
                .path(path)?
                .build()
                .await?
                .imap_user_name()
                .await
                .unwrap_or_default();

            Ok(AuthMethod::Plain { username, password })
        }
    }

    /// Retrieve SMTP authentication credentials for the given account.
    pub async fn get_smtp_auth(&self, goa_id: &str) -> Result<AuthMethod, GoaError> {
        let path = self.object_path(goa_id)?;

        let account_proxy = GoaAccountProxy::builder(&self.connection)
            .path(path)?
            .build()
            .await?;

        let provider_type =
            ProviderType::from_goa_string(&account_proxy.provider_type().await?);

        if provider_type.is_oauth() {
            let oauth_proxy = GoaOAuth2BasedProxy::builder(&self.connection)
                .path(path)?
                .build()
                .await?;

            let (token, _expires_in) = oauth_proxy.get_access_token().await.map_err(|_| {
                GoaError::CredentialUnavailable {
                    goa_id: goa_id.to_string(),
                    reason: "OAuth2 GetAccessToken failed".to_string(),
                }
            })?;

            Ok(AuthMethod::XOAuth2 { token })
        } else {
            let password_proxy = GoaPasswordBasedProxy::builder(&self.connection)
                .path(path)?
                .build()
                .await?;

            let password = password_proxy
                .get_password("smtp-password")
                .await
                .map_err(|_| GoaError::CredentialUnavailable {
                    goa_id: goa_id.to_string(),
                    reason: "PasswordBased GetPassword failed".to_string(),
                })?;

            let username = GoaMailProxy::builder(&self.connection)
                .path(path)?
                .build()
                .await?
                .smtp_user_name()
                .await
                .unwrap_or_default();

            Ok(AuthMethod::Plain { username, password })
        }
    }

    /// Validate that credentials are still valid for the given account.
    /// Returns seconds until credential expiry, or an error if re-auth is needed.
    pub async fn ensure_credentials(&self, goa_id: &str) -> Result<i32, GoaError> {
        let path = self.object_path(goa_id)?;

        let account_proxy = GoaAccountProxy::builder(&self.connection)
            .path(path)?
            .build()
            .await?;

        let expires_in = account_proxy.ensure_credentials().await.map_err(|_| {
            GoaError::CredentialUnavailable {
                goa_id: goa_id.to_string(),
                reason: "EnsureCredentials failed — account may need re-authentication"
                    .to_string(),
            }
        })?;

        Ok(expires_in)
    }
}
