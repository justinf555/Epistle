//! zbus proxy trait definitions for GNOME Online Accounts D-Bus interfaces.

#[zbus::proxy(
    interface = "org.gnome.OnlineAccounts.Account",
    default_service = "org.gnome.OnlineAccounts"
)]
pub(crate) trait GoaAccount {
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

#[zbus::proxy(
    interface = "org.gnome.OnlineAccounts.Mail",
    default_service = "org.gnome.OnlineAccounts"
)]
pub(crate) trait GoaMail {
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
    fn imap_accept_ssl_errors(&self) -> zbus::Result<bool>;

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

    #[zbus(property)]
    fn smtp_accept_ssl_errors(&self) -> zbus::Result<bool>;
}

#[zbus::proxy(
    interface = "org.gnome.OnlineAccounts.OAuth2Based",
    default_service = "org.gnome.OnlineAccounts"
)]
pub(crate) trait GoaOAuth2Based {
    fn get_access_token(&self) -> zbus::Result<(String, i32)>;
}

#[zbus::proxy(
    interface = "org.gnome.OnlineAccounts.PasswordBased",
    default_service = "org.gnome.OnlineAccounts"
)]
pub(crate) trait GoaPasswordBased {
    fn get_password(&self, id: &str) -> zbus::Result<String>;
}
