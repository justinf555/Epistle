use sha2::{Digest, Sha256};

use super::{Database, DbError};

/// A row from the accounts table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AccountRow {
    pub goa_id: String,
    pub provider_type: String,
    pub email_address: String,
    pub display_name: Option<String>,
}

/// Fields for upserting an account. Passed as a slice to bulk operations.
pub struct AccountFields<'a> {
    pub goa_id: &'a str,
    pub provider_type: &'a str,
    pub email_address: &'a str,
    pub display_name: Option<&'a str>,
    pub imap_host: &'a str,
    pub imap_port: u16,
    pub imap_tls_mode: &'a str,
    pub smtp_host: Option<&'a str>,
    pub smtp_port: Option<u16>,
    pub smtp_tls_mode: Option<&'a str>,
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

    /// Bulk upsert accounts within a transaction. Skips rows where content
    /// hasn't changed (via content_hash). Returns `true` if any rows were modified.
    pub async fn bulk_upsert_accounts(&self, accounts: &[AccountFields<'_>]) -> Result<bool, DbError> {
        let mut tx = self.pool().begin().await?;
        let mut changed = false;

        for account in accounts {
            let hash = account_content_hash(account);
            let result = sqlx::query(
                "INSERT INTO accounts (goa_id, provider_type, email_address, display_name,
                                       imap_host, imap_port, imap_tls_mode,
                                       smtp_host, smtp_port, smtp_tls_mode, content_hash)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(goa_id) DO UPDATE SET
                     provider_type = excluded.provider_type,
                     email_address = excluded.email_address,
                     display_name  = excluded.display_name,
                     imap_host     = excluded.imap_host,
                     imap_port     = excluded.imap_port,
                     imap_tls_mode = excluded.imap_tls_mode,
                     smtp_host     = excluded.smtp_host,
                     smtp_port     = excluded.smtp_port,
                     smtp_tls_mode = excluded.smtp_tls_mode,
                     content_hash  = excluded.content_hash
                 WHERE content_hash IS NOT excluded.content_hash",
            )
            .bind(account.goa_id)
            .bind(account.provider_type)
            .bind(account.email_address)
            .bind(account.display_name)
            .bind(account.imap_host)
            .bind(account.imap_port as i32)
            .bind(account.imap_tls_mode)
            .bind(account.smtp_host)
            .bind(account.smtp_port.map(|p| p as i32))
            .bind(account.smtp_tls_mode)
            .bind(&hash)
            .execute(&mut *tx)
            .await?;

            if result.rows_affected() > 0 {
                changed = true;
            }
        }

        tx.commit().await?;
        Ok(changed)
    }
}

fn account_content_hash(a: &AccountFields<'_>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(a.provider_type.as_bytes());
    hasher.update(b"|");
    hasher.update(a.email_address.as_bytes());
    hasher.update(b"|");
    hasher.update(a.display_name.unwrap_or("").as_bytes());
    hasher.update(b"|");
    hasher.update(a.imap_host.as_bytes());
    hasher.update(b"|");
    hasher.update(a.imap_port.to_string().as_bytes());
    hasher.update(b"|");
    hasher.update(a.imap_tls_mode.as_bytes());
    hasher.update(b"|");
    hasher.update(a.smtp_host.unwrap_or("").as_bytes());
    hasher.update(b"|");
    hasher.update(a.smtp_port.map(|p| p.to_string()).unwrap_or_default().as_bytes());
    hasher.update(b"|");
    hasher.update(a.smtp_tls_mode.unwrap_or("").as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn insert_and_read_account() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("mail.db")).await.unwrap();

        let accounts = [AccountFields {
            goa_id: "account_1234",
            provider_type: "google",
            email_address: "user@gmail.com",
            display_name: None,
            imap_host: "imap.gmail.com",
            imap_port: 993,
            imap_tls_mode: "implicit",
            smtp_host: None,
            smtp_port: None,
            smtp_tls_mode: None,
        }];

        let changed = db.bulk_upsert_accounts(&accounts).await.unwrap();
        assert!(changed);

        let rows = db.list_active_accounts().await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].email_address, "user@gmail.com");

        // Second upsert with same data — no changes
        let changed = db.bulk_upsert_accounts(&accounts).await.unwrap();
        assert!(!changed);
    }
}
