use super::{Database, DbError};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FolderRow {
    pub id: i64,
    pub account_id: String,
    pub name: String,
    pub delimiter: Option<String>,
    pub role: Option<String>,
}

impl Database {
    /// Insert or update a folder from IMAP LIST results.
    pub async fn upsert_folder(
        &self,
        account_id: &str,
        name: &str,
        delimiter: Option<&str>,
        role: Option<&str>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO folders (account_id, name, delimiter, role)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(account_id, name) DO UPDATE SET
                 delimiter = excluded.delimiter,
                 role = excluded.role",
        )
        .bind(account_id)
        .bind(name)
        .bind(delimiter)
        .bind(role)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Return all folders for a given account, ordered by role (standard first) then name.
    pub async fn list_folders(&self, account_id: &str) -> Result<Vec<FolderRow>, DbError> {
        let rows = sqlx::query_as::<_, FolderRow>(
            "SELECT id, account_id, name, delimiter, role FROM folders
             WHERE account_id = ?
             ORDER BY
                 CASE role
                     WHEN 'inbox' THEN 0
                     WHEN 'sent' THEN 1
                     WHEN 'drafts' THEN 2
                     WHEN 'archive' THEN 3
                     WHEN 'trash' THEN 4
                     WHEN 'junk' THEN 5
                     ELSE 6
                 END,
                 name",
        )
        .bind(account_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn upsert_and_list_folders() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("mail.db")).await.unwrap();

        // Create an account first
        sqlx::query(
            "INSERT INTO accounts (goa_id, provider_type, email_address, imap_host, imap_port, imap_tls_mode)
             VALUES ('acct1', 'google', 'user@gmail.com', 'imap.gmail.com', 993, 'implicit')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        db.upsert_folder("acct1", "INBOX", Some("/"), Some("inbox")).await.unwrap();
        db.upsert_folder("acct1", "[Gmail]/Sent Mail", Some("/"), Some("sent")).await.unwrap();
        db.upsert_folder("acct1", "Custom Label", Some("/"), None).await.unwrap();

        let folders = db.list_folders("acct1").await.unwrap();
        assert_eq!(folders.len(), 3);
        assert_eq!(folders[0].name, "INBOX");
        assert_eq!(folders[0].role.as_deref(), Some("inbox"));
        assert_eq!(folders[1].name, "[Gmail]/Sent Mail");
        assert_eq!(folders[2].name, "Custom Label");
        assert!(folders[2].role.is_none());
    }
}
