use crate::goa::types::{GoaMailAccount, TlsMode};

use super::{Database, DbError};

impl Database {
    /// Insert or update an account from GOA discovery.
    /// Preserves `created_at` and `last_sync` on conflict.
    pub async fn upsert_account(&self, account: &GoaMailAccount) -> Result<(), DbError> {
        let provider_type = account.provider_type.as_goa_str();
        let imap_tls = tls_mode_to_str(account.imap_config.tls_mode);
        let smtp_host = account.smtp_config.as_ref().map(|c| c.host.as_str());
        let smtp_port = account.smtp_config.as_ref().map(|c| c.port as i32);
        let smtp_tls = account.smtp_config.as_ref().map(|c| tls_mode_to_str(c.tls_mode));

        sqlx::query(
            "INSERT INTO accounts (goa_id, provider_type, email_address, display_name,
                                   imap_host, imap_port, imap_tls_mode,
                                   smtp_host, smtp_port, smtp_tls_mode)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(goa_id) DO UPDATE SET
                 provider_type = excluded.provider_type,
                 email_address = excluded.email_address,
                 display_name  = excluded.display_name,
                 imap_host     = excluded.imap_host,
                 imap_port     = excluded.imap_port,
                 imap_tls_mode = excluded.imap_tls_mode,
                 smtp_host     = excluded.smtp_host,
                 smtp_port     = excluded.smtp_port,
                 smtp_tls_mode = excluded.smtp_tls_mode",
        )
        .bind(&account.goa_id)
        .bind(provider_type)
        .bind(&account.email_address)
        .bind(&account.display_name)
        .bind(&account.imap_config.host)
        .bind(account.imap_config.port as i32)
        .bind(imap_tls)
        .bind(smtp_host)
        .bind(smtp_port)
        .bind(smtp_tls)
        .execute(self.pool())
        .await?;

        Ok(())
    }
}

fn tls_mode_to_str(mode: TlsMode) -> &'static str {
    match mode {
        TlsMode::Implicit => "implicit",
        TlsMode::StartTls => "starttls",
        TlsMode::None => "none",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn insert_and_read_account() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("mail.db")).await.unwrap();

        sqlx::query(
            "INSERT INTO accounts (goa_id, provider_type, email_address, imap_host, imap_port, imap_tls_mode)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind("account_1234")
        .bind("google")
        .bind("user@gmail.com")
        .bind("imap.gmail.com")
        .bind(993)
        .bind("implicit")
        .execute(db.pool())
        .await
        .unwrap();

        let row: (String, String, i32) = sqlx::query_as(
            "SELECT email_address, imap_host, imap_port FROM accounts WHERE goa_id = ?",
        )
        .bind("account_1234")
        .fetch_one(db.pool())
        .await
        .unwrap();

        assert_eq!(row.0, "user@gmail.com");
        assert_eq!(row.1, "imap.gmail.com");
        assert_eq!(row.2, 993);
    }
}
