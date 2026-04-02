//! WebhookAuditStore implementation for LibSqlBackend.

use async_trait::async_trait;
use libsql::params;
use uuid::Uuid;

use super::{LibSqlBackend, fmt_ts, get_opt_bool, get_text, get_ts};
use crate::db::WebhookAuditStore;
use crate::error::DatabaseError;
use crate::history::WebhookEventRecord;

#[async_trait]
impl WebhookAuditStore for LibSqlBackend {
    async fn insert_webhook_event(
        &self,
        record: &WebhookEventRecord,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect().await?;
        let hmac_valid: Option<i64> = record.hmac_valid.map(|v| if v { 1 } else { 0 });
        conn.execute(
            r#"
            INSERT INTO webhook_events (id, received_at, channel, hmac_valid, payload_hash, user_id)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT (id) DO NOTHING
            "#,
            params![
                record.id.to_string(),
                fmt_ts(&record.received_at),
                record.channel.as_str(),
                hmac_valid,
                record.payload_hash.as_str(),
                record.user_id.as_str()
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn list_webhook_events(
        &self,
        user_id: &str,
        limit: i64,
    ) -> Result<Vec<WebhookEventRecord>, DatabaseError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                r#"
                SELECT id, received_at, channel, hmac_valid, payload_hash, user_id
                FROM webhook_events
                WHERE user_id = ?1
                ORDER BY received_at DESC
                LIMIT ?2
                "#,
                params![user_id, limit],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut events = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            let id: Uuid = get_text(&row, 0)
                .parse()
                .map_err(|e: uuid::Error| DatabaseError::Query(e.to_string()))?;
            let hmac_valid: Option<bool> = get_opt_bool(&row, 3);
            events.push(WebhookEventRecord {
                id,
                received_at: get_ts(&row, 1),
                channel: get_text(&row, 2),
                hmac_valid,
                payload_hash: get_text(&row, 4),
                user_id: get_text(&row, 5),
            });
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::libsql::LibSqlBackend;
    use crate::db::Database;
    use chrono::Utc;

    #[tokio::test]
    async fn webhook_event_round_trip() {
        let backend = LibSqlBackend::new_memory().await.unwrap();
        backend.run_migrations().await.unwrap();

        let record = WebhookEventRecord {
            id: Uuid::new_v4(),
            received_at: Utc::now(),
            channel: "webhook".to_string(),
            hmac_valid: Some(true),
            payload_hash: "abc123".to_string(),
            user_id: "user1".to_string(),
        };

        backend.insert_webhook_event(&record).await.unwrap();
        let events = backend.list_webhook_events("user1", 10).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, record.id);
        assert_eq!(events[0].channel, "webhook");
        assert_eq!(events[0].hmac_valid, Some(true));
        assert_eq!(events[0].payload_hash, "abc123");
    }

    #[tokio::test]
    async fn list_returns_newest_first() {
        let backend = LibSqlBackend::new_memory().await.unwrap();
        backend.run_migrations().await.unwrap();

        for i in 0..3u8 {
            let record = WebhookEventRecord {
                id: Uuid::new_v4(),
                received_at: Utc::now(),
                channel: "webhook".to_string(),
                hmac_valid: None,
                payload_hash: format!("hash{i}"),
                user_id: "user1".to_string(),
            };
            backend.insert_webhook_event(&record).await.unwrap();
        }

        let events = backend.list_webhook_events("user1", 10).await.unwrap();
        assert_eq!(events.len(), 3);
        // Newest first: received_at DESC
        assert!(events[0].received_at >= events[1].received_at);
    }
}
