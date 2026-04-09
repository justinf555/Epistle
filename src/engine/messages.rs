use async_trait::async_trait;

use crate::app_event::AppEvent;
use crate::engine::db::messages::MessageFields;
use crate::engine::db::Database;
use crate::engine::traits::messages::{MailMessages, Message, MessageBody};
use crate::event_bus::EventSender;

/// Concrete implementation of [`MailMessages`] backed by SQLite + EventSender.
///
/// Domain-pure — no IMAP or protocol dependencies.
pub struct MailMessagesImpl {
    db: Database,
    sender: EventSender,
}

impl MailMessagesImpl {
    pub fn new(db: Database, sender: EventSender) -> Self {
        Self { db, sender }
    }
}

#[async_trait]
impl MailMessages for MailMessagesImpl {
    async fn sync_messages(
        &self,
        account_id: &str,
        folder_name: &str,
        messages: &[Message],
    ) -> anyhow::Result<()> {
        // Pre-compute joined strings so MessageFields can borrow them.
        let joined: Vec<_> = messages
            .iter()
            .map(|m| {
                let to_joined = if m.to_addresses.is_empty() {
                    None
                } else {
                    Some(m.to_addresses.join(", "))
                };
                let cc_joined = if m.cc_addresses.is_empty() {
                    None
                } else {
                    Some(m.cc_addresses.join(", "))
                };
                let refs_joined = if m.references.is_empty() {
                    None
                } else {
                    Some(m.references.join(" "))
                };
                (m, to_joined, cc_joined, refs_joined)
            })
            .collect();

        let fields: Vec<MessageFields<'_>> = joined
            .iter()
            .map(|(m, to_joined, cc_joined, refs_joined)| MessageFields {
                uid: m.uid,
                message_id: m.message_id.as_deref(),
                subject: m.subject.as_deref(),
                sender: m.sender.as_deref(),
                to_addresses: to_joined.as_deref(),
                cc_addresses: cc_joined.as_deref(),
                date: m.date.as_deref(),
                in_reply_to: m.in_reply_to.as_deref(),
                reference_ids: refs_joined.as_deref(),
                is_read: m.is_read,
                is_flagged: m.is_flagged,
                is_answered: m.is_answered,
                is_draft: m.is_draft,
                preview: m.preview.as_deref(),
                content_type: m.content_type.as_deref(),
                has_attachments: m.has_attachments,
                internal_date: m.internal_date.as_deref(),
            })
            .collect();

        tracing::debug!(
            account_id,
            folder_name,
            count = fields.len(),
            "Persisting messages to database"
        );

        let result = self
            .db
            .bulk_upsert_messages(account_id, folder_name, &fields)
            .await?;

        if !result.inserted.is_empty() {
            let rows = self
                .db
                .list_messages_by_uids(account_id, folder_name, &result.inserted)
                .await?;
            let added: Vec<Message> = rows.into_iter().map(row_to_message).collect();

            tracing::debug!(
                account_id,
                folder_name,
                count = added.len(),
                "Emitting MessagesAdded"
            );

            self.sender.send(AppEvent::MessagesAdded {
                account_id: account_id.to_string(),
                folder_name: folder_name.to_string(),
                messages: added,
            });
        }

        if !result.updated.is_empty() {
            let rows = self
                .db
                .list_messages_by_uids(account_id, folder_name, &result.updated)
                .await?;
            let updated: Vec<Message> = rows.into_iter().map(row_to_message).collect();

            tracing::debug!(
                account_id,
                folder_name,
                count = updated.len(),
                "Emitting MessagesUpdated"
            );

            self.sender.send(AppEvent::MessagesUpdated {
                account_id: account_id.to_string(),
                folder_name: folder_name.to_string(),
                messages: updated,
            });
        }

        if !result.has_changes() {
            tracing::debug!(account_id, folder_name, "No message changes detected");
        }

        Ok(())
    }

    async fn list_messages(
        &self,
        account_id: &str,
        folder_name: &str,
    ) -> anyhow::Result<Vec<Message>> {
        let rows = self.db.list_messages(account_id, folder_name).await?;
        Ok(rows.into_iter().map(row_to_message).collect())
    }

    async fn cache_body(
        &self,
        account_id: &str,
        folder_name: &str,
        uid: u32,
        body_text: Option<&str>,
        body_html: Option<&str>,
    ) -> anyhow::Result<()> {
        self.db
            .update_message_body(account_id, folder_name, uid, body_text, body_html)
            .await?;
        Ok(())
    }

    async fn get_body(
        &self,
        account_id: &str,
        folder_name: &str,
        uid: u32,
    ) -> anyhow::Result<Option<MessageBody>> {
        let row = self
            .db
            .get_message_body(account_id, folder_name, uid)
            .await?;
        Ok(row.map(|(text, html)| MessageBody {
            body_text: text,
            body_html: html,
        }))
    }
}

fn row_to_message(row: crate::engine::db::messages::MessageRow) -> Message {
    Message {
        uid: row.uid as u32,
        account_id: row.account_id,
        folder_name: row.folder_name,
        message_id: row.message_id,
        subject: row.subject,
        sender: row.sender,
        to_addresses: row
            .to_addresses
            .map(|s| s.split(", ").map(String::from).collect())
            .unwrap_or_default(),
        cc_addresses: row
            .cc_addresses
            .map(|s| s.split(", ").map(String::from).collect())
            .unwrap_or_default(),
        date: row.date,
        internal_date: row.internal_date,
        in_reply_to: row.in_reply_to,
        references: row
            .reference_ids
            .map(|s| s.split(' ').map(String::from).collect())
            .unwrap_or_default(),
        is_read: row.is_read,
        is_flagged: row.is_flagged,
        is_answered: row.is_answered,
        is_draft: row.is_draft,
        preview: row.preview,
        content_type: row.content_type,
        has_attachments: row.has_attachments,
        body_text: row.body_text,
        body_html: row.body_html,
    }
}
