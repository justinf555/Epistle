use sha2::{Digest, Sha256};

use super::{Database, DbError};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MessageRow {
    pub id: i64,
    pub account_id: String,
    pub folder_name: String,
    pub uid: i64,
    pub message_id: Option<String>,
    pub subject: Option<String>,
    pub sender: Option<String>,
    pub to_addresses: Option<String>,
    pub cc_addresses: Option<String>,
    pub date: Option<String>,
    pub in_reply_to: Option<String>,
    pub reference_ids: Option<String>,
    pub is_read: bool,
    pub is_flagged: bool,
    pub is_answered: bool,
    pub is_draft: bool,
    pub preview: Option<String>,
    pub content_type: Option<String>,
    pub has_attachments: bool,
    pub body_text: Option<String>,
    pub body_html: Option<String>,
}

/// Fields for upserting a message. Passed as a slice to bulk operations.
pub struct MessageFields<'a> {
    pub uid: u32,
    pub message_id: Option<&'a str>,
    pub subject: Option<&'a str>,
    pub sender: Option<&'a str>,
    pub to_addresses: Option<&'a str>,
    pub cc_addresses: Option<&'a str>,
    pub date: Option<&'a str>,
    pub in_reply_to: Option<&'a str>,
    pub reference_ids: Option<&'a str>,
    pub is_read: bool,
    pub is_flagged: bool,
    pub is_answered: bool,
    pub is_draft: bool,
    pub preview: Option<&'a str>,
    pub content_type: Option<&'a str>,
    pub has_attachments: bool,
}

/// Result of a bulk upsert — which UIDs were inserted, updated, or unchanged.
#[derive(Debug, Default)]
pub struct UpsertResult {
    pub inserted: Vec<u32>,
    pub updated: Vec<u32>,
}

impl UpsertResult {
    pub fn has_changes(&self) -> bool {
        !self.inserted.is_empty() || !self.updated.is_empty()
    }
}

impl Database {
    /// Bulk upsert messages for a folder within a transaction. Skips rows where
    /// content hasn't changed (via content_hash). Returns which UIDs were
    /// inserted vs updated (unchanged UIDs are omitted).
    pub async fn bulk_upsert_messages(
        &self,
        account_id: &str,
        folder_name: &str,
        messages: &[MessageFields<'_>],
    ) -> Result<UpsertResult, DbError> {
        let mut tx = self.pool().begin().await?;

        // Snapshot existing UIDs + hashes for this folder
        let existing: std::collections::HashMap<i64, String> = sqlx::query_as::<_, (i64, String)>(
            "SELECT uid, content_hash FROM messages
             WHERE account_id = ? AND folder_name = ? AND content_hash IS NOT NULL",
        )
        .bind(account_id)
        .bind(folder_name)
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .collect();

        let mut result = UpsertResult::default();

        for msg in messages {
            let hash = message_content_hash(msg);
            let was_existing = existing.contains_key(&(msg.uid as i64));
            let hash_changed = existing
                .get(&(msg.uid as i64))
                .map(|h| h != &hash)
                .unwrap_or(true);

            let rows = sqlx::query(
                "INSERT INTO messages (
                    account_id, folder_name, uid, message_id, subject, sender,
                    to_addresses, cc_addresses, date, in_reply_to, reference_ids,
                    is_read, is_flagged, is_answered, is_draft,
                    preview, content_type, has_attachments, content_hash
                 )
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(account_id, folder_name, uid) DO UPDATE SET
                     message_id    = excluded.message_id,
                     subject       = excluded.subject,
                     sender        = excluded.sender,
                     to_addresses  = excluded.to_addresses,
                     cc_addresses  = excluded.cc_addresses,
                     date          = excluded.date,
                     in_reply_to   = excluded.in_reply_to,
                     reference_ids = excluded.reference_ids,
                     is_read       = excluded.is_read,
                     is_flagged    = excluded.is_flagged,
                     is_answered   = excluded.is_answered,
                     is_draft      = excluded.is_draft,
                     preview       = COALESCE(excluded.preview, messages.preview),
                     content_type  = COALESCE(excluded.content_type, messages.content_type),
                     has_attachments = excluded.has_attachments,
                     content_hash  = excluded.content_hash
                 WHERE content_hash IS NOT excluded.content_hash",
            )
            .bind(account_id)
            .bind(folder_name)
            .bind(msg.uid)
            .bind(msg.message_id)
            .bind(msg.subject)
            .bind(msg.sender)
            .bind(msg.to_addresses)
            .bind(msg.cc_addresses)
            .bind(msg.date)
            .bind(msg.in_reply_to)
            .bind(msg.reference_ids)
            .bind(msg.is_read)
            .bind(msg.is_flagged)
            .bind(msg.is_answered)
            .bind(msg.is_draft)
            .bind(msg.preview)
            .bind(msg.content_type)
            .bind(msg.has_attachments)
            .bind(&hash)
            .execute(&mut *tx)
            .await?;

            if rows.rows_affected() > 0 {
                if was_existing && hash_changed {
                    result.updated.push(msg.uid);
                } else if !was_existing {
                    result.inserted.push(msg.uid);
                }
            }
        }

        tx.commit().await?;
        Ok(result)
    }

    /// Return all messages for a given folder, ordered by date descending (newest first).
    pub async fn list_messages(
        &self,
        account_id: &str,
        folder_name: &str,
    ) -> Result<Vec<MessageRow>, DbError> {
        let rows = sqlx::query_as::<_, MessageRow>(
            "SELECT id, account_id, folder_name, uid, message_id, subject, sender,
                    to_addresses, cc_addresses, date, in_reply_to, reference_ids,
                    is_read, is_flagged, is_answered, is_draft,
                    preview, content_type, has_attachments,
                    body_text, body_html
             FROM messages
             WHERE account_id = ? AND folder_name = ?
             ORDER BY date DESC, uid DESC",
        )
        .bind(account_id)
        .bind(folder_name)
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    /// Return messages for specific UIDs in a folder.
    pub async fn list_messages_by_uids(
        &self,
        account_id: &str,
        folder_name: &str,
        uids: &[u32],
    ) -> Result<Vec<MessageRow>, DbError> {
        if uids.is_empty() {
            return Ok(vec![]);
        }
        // Build placeholder list for IN clause
        let placeholders: Vec<String> = uids.iter().map(|_| "?".to_string()).collect();
        let sql = format!(
            "SELECT id, account_id, folder_name, uid, message_id, subject, sender,
                    to_addresses, cc_addresses, date, in_reply_to, reference_ids,
                    is_read, is_flagged, is_answered, is_draft,
                    preview, content_type, has_attachments,
                    body_text, body_html
             FROM messages
             WHERE account_id = ? AND folder_name = ? AND uid IN ({})
             ORDER BY date DESC, uid DESC",
            placeholders.join(", ")
        );
        let mut query = sqlx::query_as::<_, MessageRow>(&sql)
            .bind(account_id)
            .bind(folder_name);
        for uid in uids {
            query = query.bind(*uid);
        }
        let rows = query.fetch_all(self.pool()).await?;
        Ok(rows)
    }

    /// Get cached body for a specific message. Returns None if message not found.
    pub async fn get_message_body(
        &self,
        account_id: &str,
        folder_name: &str,
        uid: u32,
    ) -> Result<Option<(Option<String>, Option<String>)>, DbError> {
        let row = sqlx::query_as::<_, (Option<String>, Option<String>)>(
            "SELECT body_text, body_html FROM messages
             WHERE account_id = ? AND folder_name = ? AND uid = ?",
        )
        .bind(account_id)
        .bind(folder_name)
        .bind(uid)
        .fetch_optional(self.pool())
        .await?;
        Ok(row)
    }

    /// Update the cached body for a message. Returns true if a row was updated.
    pub async fn update_message_body(
        &self,
        account_id: &str,
        folder_name: &str,
        uid: u32,
        body_text: Option<&str>,
        body_html: Option<&str>,
    ) -> Result<bool, DbError> {
        let result = sqlx::query(
            "UPDATE messages SET body_text = ?, body_html = ?
             WHERE account_id = ? AND folder_name = ? AND uid = ?",
        )
        .bind(body_text)
        .bind(body_html)
        .bind(account_id)
        .bind(folder_name)
        .bind(uid)
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() > 0)
    }
}

fn message_content_hash(m: &MessageFields<'_>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(m.uid.to_le_bytes());
    hasher.update(b"|");
    hasher.update(m.message_id.unwrap_or("").as_bytes());
    hasher.update(b"|");
    hasher.update(m.subject.unwrap_or("").as_bytes());
    hasher.update(b"|");
    hasher.update(m.sender.unwrap_or("").as_bytes());
    hasher.update(b"|");
    hasher.update(m.date.unwrap_or("").as_bytes());
    hasher.update(b"|");
    hasher.update(if m.is_read { b"1" } else { b"0" });
    hasher.update(if m.is_flagged { b"1" } else { b"0" });
    hasher.update(if m.is_answered { b"1" } else { b"0" });
    hasher.update(if m.is_draft { b"1" } else { b"0" });
    hasher.update(b"|");
    hasher.update(m.preview.unwrap_or("").as_bytes());
    hasher.update(b"|");
    hasher.update(m.content_type.unwrap_or("").as_bytes());
    hasher.update(b"|");
    hasher.update(if m.has_attachments { b"1" } else { b"0" });
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn upsert_and_list_messages() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("mail.db")).await.unwrap();

        // Create an account and folder first
        sqlx::query(
            "INSERT INTO accounts (goa_id, provider_type, email_address, imap_host, imap_port, imap_tls_mode)
             VALUES ('acct1', 'google', 'user@gmail.com', 'imap.gmail.com', 993, 'implicit')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        let messages = [
            MessageFields {
                uid: 1,
                message_id: Some("msg001@gmail.com"),
                subject: Some("Hello World"),
                sender: Some("alice@example.com"),
                to_addresses: Some("user@gmail.com"),
                cc_addresses: None,
                date: Some("2026-04-09T10:00:00Z"),
                in_reply_to: None,
                reference_ids: None,
                is_read: false,
                is_flagged: false,
                is_answered: false,
                is_draft: false,
                preview: None,
                content_type: None,
                has_attachments: false,
            },
            MessageFields {
                uid: 2,
                message_id: Some("msg002@gmail.com"),
                subject: Some("Re: Hello World"),
                sender: Some("bob@example.com"),
                to_addresses: Some("user@gmail.com"),
                cc_addresses: Some("alice@example.com"),
                date: Some("2026-04-09T11:00:00Z"),
                in_reply_to: Some("msg001@gmail.com"),
                reference_ids: Some("msg001@gmail.com"),
                is_read: true,
                is_flagged: true,
                is_answered: false,
                is_draft: false,
                preview: None,
                content_type: None,
                has_attachments: false,
            },
        ];

        let result = db
            .bulk_upsert_messages("acct1", "INBOX", &messages)
            .await
            .unwrap();
        assert_eq!(result.inserted.len(), 2);
        assert!(result.updated.is_empty());
        assert!(result.inserted.contains(&1));
        assert!(result.inserted.contains(&2));

        let rows = db.list_messages("acct1", "INBOX").await.unwrap();
        assert_eq!(rows.len(), 2);
        // Newest first
        assert_eq!(rows[0].subject.as_deref(), Some("Re: Hello World"));
        assert_eq!(rows[1].subject.as_deref(), Some("Hello World"));
        assert!(rows[0].is_read);
        assert!(!rows[1].is_read);

        // Second upsert with same data — no changes
        let result = db
            .bulk_upsert_messages("acct1", "INBOX", &messages)
            .await
            .unwrap();
        assert!(!result.has_changes());
    }

    #[tokio::test]
    async fn upsert_preserves_existing_phase2_fields() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("mail.db")).await.unwrap();

        sqlx::query(
            "INSERT INTO accounts (goa_id, provider_type, email_address, imap_host, imap_port, imap_tls_mode)
             VALUES ('acct1', 'google', 'user@gmail.com', 'imap.gmail.com', 993, 'implicit')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        // Phase 2: message with preview
        let with_preview = [MessageFields {
            uid: 1,
            message_id: Some("msg001@gmail.com"),
            subject: Some("Hello"),
            sender: Some("alice@example.com"),
            to_addresses: None,
            cc_addresses: None,
            date: Some("2026-04-09T10:00:00Z"),
            in_reply_to: None,
            reference_ids: None,
            is_read: false,
            is_flagged: false,
            is_answered: false,
            is_draft: false,
            preview: Some("Hey, how are you?"),
            content_type: Some("text/plain"),
            has_attachments: false,
        }];

        db.bulk_upsert_messages("acct1", "INBOX", &with_preview)
            .await
            .unwrap();

        // Phase 1 re-sync: same message without preview (flag changed)
        let without_preview = [MessageFields {
            uid: 1,
            message_id: Some("msg001@gmail.com"),
            subject: Some("Hello"),
            sender: Some("alice@example.com"),
            to_addresses: None,
            cc_addresses: None,
            date: Some("2026-04-09T10:00:00Z"),
            in_reply_to: None,
            reference_ids: None,
            is_read: true, // flag changed
            is_flagged: false,
            is_answered: false,
            is_draft: false,
            preview: None, // Phase 1 doesn't have this
            content_type: None,
            has_attachments: false,
        }];

        db.bulk_upsert_messages("acct1", "INBOX", &without_preview)
            .await
            .unwrap();

        let rows = db.list_messages("acct1", "INBOX").await.unwrap();
        assert_eq!(rows.len(), 1);
        // Preview preserved via COALESCE
        assert_eq!(rows[0].preview.as_deref(), Some("Hey, how are you?"));
        assert_eq!(rows[0].content_type.as_deref(), Some("text/plain"));
        // Flag updated
        assert!(rows[0].is_read);
    }
}
