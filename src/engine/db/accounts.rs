use super::{Database, DbError};

/// A row from the accounts table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AccountRow {
    pub goa_id: String,
    pub provider_type: String,
    pub email_address: String,
    pub display_name: Option<String>,
}

impl Database {
    /// Return all active accounts, ordered by email address.
    pub async fn list_active_accounts(&self) -> Result<Vec<AccountRow>, DbError> {
        let rows = sqlx::query_as::<_, AccountRow>(
            "SELECT goa_id, provider_type, email_address, display_name
             FROM accounts WHERE active = 1 ORDER BY email_address",
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    /// Insert or update an account using flat domain fields.
    ///
    /// Used by the engine trait layer which works with domain types, not GOA types.
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_account_fields(
        &self,
        goa_id: &str,
        provider_type: &str,
        email_address: &str,
        display_name: Option<&str>,
        imap_host: &str,
        imap_port: u16,
        imap_tls_mode: &str,
        smtp_host: Option<&str>,
        smtp_port: Option<u16>,
        smtp_tls_mode: Option<&str>,
    ) -> Result<(), DbError> {
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
        .bind(goa_id)
        .bind(provider_type)
        .bind(email_address)
        .bind(display_name)
        .bind(imap_host)
        .bind(imap_port as i32)
        .bind(imap_tls_mode)
        .bind(smtp_host)
        .bind(smtp_port.map(|p| p as i32))
        .bind(smtp_tls_mode)
        .execute(self.pool())
        .await?;

        Ok(())
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
