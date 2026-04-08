//! Integration tests for GOA D-Bus discovery and credential retrieval.
//!
//! These tests require a live GNOME session with at least one mail-enabled
//! account configured in GNOME Settings → Online Accounts.
//!
//! Run with: make integration-test

use epistle::goa::{GoaClient, ProviderType};

#[ignore]
#[tokio::test]
async fn discover_accounts_finds_at_least_one() {
    let mut client = GoaClient::new()
        .await
        .expect("failed to connect to session D-Bus");

    let accounts = client
        .discover_accounts()
        .await
        .expect("discover_accounts failed");

    assert!(
        !accounts.is_empty(),
        "no mail accounts found — configure at least one in GNOME Settings"
    );

    for account in &accounts {
        println!(
            "found account: {} ({}) — {} [{}]",
            account.email_address,
            account.provider_name,
            account.goa_id,
            match &account.provider_type {
                ProviderType::Google => "google/oauth",
                ProviderType::Microsoft => "microsoft/oauth",
                ProviderType::GenericImap => "generic/password",
            }
        );

        // IMAP config must have a non-empty host
        assert!(!account.imap_config.host.is_empty(), "IMAP host is empty");
        assert!(account.imap_config.port > 0, "IMAP port is 0");

        // Email address should be present
        assert!(!account.email_address.is_empty(), "email address is empty");
    }
}

#[ignore]
#[tokio::test]
async fn ensure_credentials_succeeds() {
    let mut client = GoaClient::new().await.unwrap();
    let accounts = client.discover_accounts().await.unwrap();
    let account = accounts.first().expect("no accounts to test with");

    let expires_in = client
        .ensure_credentials(&account.goa_id)
        .await
        .expect("ensure_credentials failed — account may need re-auth in GNOME Settings");

    println!(
        "credentials valid for {}: expires_in={}s",
        account.email_address, expires_in
    );
}

#[ignore]
#[tokio::test]
async fn get_imap_auth_returns_credential() {
    let mut client = GoaClient::new().await.unwrap();
    let accounts = client.discover_accounts().await.unwrap();
    let account = accounts.first().expect("no accounts to test with");

    let auth = client
        .get_imap_auth(&account.goa_id)
        .await
        .expect("get_imap_auth failed");

    match &auth {
        epistle::goa::AuthMethod::XOAuth2 { token } => {
            assert!(!token.is_empty(), "OAuth token is empty");
            println!("got OAuth token for {} ({}B)", account.email_address, token.len());
        }
        epistle::goa::AuthMethod::Plain { username, password } => {
            assert!(!password.is_empty(), "password is empty");
            println!("got password for {} (user={})", account.email_address, username);
        }
    }
}
