use sha2::{Digest, Sha256};

use super::{Database, DbError};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FolderRow {
    pub id: i64,
    pub account_id: String,
    pub name: String,
    pub delimiter: Option<String>,
    pub role: Option<String>,
}

/// Fields for upserting a folder. Passed as a slice to bulk operations.
pub struct FolderFields<'a> {
    pub name: &'a str,
    pub delimiter: Option<&'a str>,
    pub role: Option<&'a str>,
}

impl Database {
    /// Bulk upsert folders for an account within a transaction. Skips rows where
    /// content hasn't changed (via content_hash). Returns `true` if any rows were modified.
    pub async fn bulk_upsert_folders(
        &self,
        account_id: &str,
        folders: &[FolderFields<'_>],
    ) -> Result<bool, DbError> {
        let mut tx = self.pool().begin().await?;
        let mut changed = false;

        for folder in folders {
            let hash = folder_content_hash(folder);
            let result = sqlx::query(
                "INSERT INTO folders (account_id, name, delimiter, role, content_hash)
                 VALUES (?, ?, ?, ?, ?)
                 ON CONFLICT(account_id, name) DO UPDATE SET
                     delimiter    = excluded.delimiter,
                     role         = excluded.role,
                     content_hash = excluded.content_hash
                 WHERE content_hash IS NOT excluded.content_hash",
            )
            .bind(account_id)
            .bind(folder.name)
            .bind(folder.delimiter)
            .bind(folder.role)
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

fn folder_content_hash(f: &FolderFields<'_>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(f.name.as_bytes());
    hasher.update(b"|");
    hasher.update(f.delimiter.unwrap_or("").as_bytes());
    hasher.update(b"|");
    hasher.update(f.role.unwrap_or("").as_bytes());
    format!("{:x}", hasher.finalize())
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

        let folders = [
            FolderFields { name: "INBOX", delimiter: Some("/"), role: Some("inbox") },
            FolderFields { name: "[Gmail]/Sent Mail", delimiter: Some("/"), role: Some("sent") },
            FolderFields { name: "Custom Label", delimiter: Some("/"), role: None },
        ];

        let changed = db.bulk_upsert_folders("acct1", &folders).await.unwrap();
        assert!(changed);

        let rows = db.list_folders("acct1").await.unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].name, "INBOX");
        assert_eq!(rows[0].role.as_deref(), Some("inbox"));
        assert_eq!(rows[1].name, "[Gmail]/Sent Mail");
        assert_eq!(rows[2].name, "Custom Label");
        assert!(rows[2].role.is_none());

        // Second upsert with same data — no changes
        let changed = db.bulk_upsert_folders("acct1", &folders).await.unwrap();
        assert!(!changed);
    }
}
