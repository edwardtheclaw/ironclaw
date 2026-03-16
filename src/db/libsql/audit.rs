//! AuditStore implementation for libSQL.

use async_trait::async_trait;
use uuid::Uuid;

use crate::db::{AuditFilter, AuditRecord, AuditStore};
use crate::error::DatabaseError;

use super::LibSqlBackend;

fn parse_opt_uuid(row: &libsql::Row, idx: i32) -> Option<Uuid> {
    super::get_opt_text(row, idx).and_then(|s| Uuid::parse_str(&s).ok())
}

#[async_trait]
impl AuditStore for LibSqlBackend {
    async fn append_audit_events(&self, events: &[AuditRecord]) -> Result<(), DatabaseError> {
        if events.is_empty() {
            return Ok(());
        }

        let conn = self.connect().await?;

        // Use a transaction for the batch insert.
        conn.execute("BEGIN", ())
            .await
            .map_err(|e| DatabaseError::Query(format!("audit begin: {e}")))?;

        for event in events {
            conn.execute(
                "INSERT INTO audit_log (event_id, event_type, source_module, source_component, \
                 category, session_id, thread_id, job_id, user_id, payload, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                libsql::params![
                    event.event_id as i64,
                    event.event_type.clone(),
                    event.source_module.clone(),
                    event.source_component.clone(),
                    event.category.clone(),
                    event.session_id.map(|u| u.to_string()),
                    event.thread_id.map(|u| u.to_string()),
                    event.job_id.map(|u| u.to_string()),
                    event.user_id.clone(),
                    serde_json::to_string(&event.payload).unwrap_or_default(),
                    super::fmt_ts(&event.created_at),
                ],
            )
            .await
            .map_err(|e| DatabaseError::Query(format!("audit insert: {e}")))?;
        }

        conn.execute("COMMIT", ())
            .await
            .map_err(|e| DatabaseError::Query(format!("audit commit: {e}")))?;

        Ok(())
    }

    async fn query_audit_log(
        &self,
        filter: &AuditFilter,
    ) -> Result<Vec<AuditRecord>, DatabaseError> {
        let conn = self.connect().await?;

        let mut query = String::from(
            "SELECT event_id, event_type, source_module, source_component, category, \
             session_id, thread_id, job_id, user_id, payload, created_at \
             FROM audit_log WHERE 1=1",
        );
        let mut params: Vec<libsql::Value> = Vec::new();
        let mut idx = 1;

        if let Some(ref sid) = filter.session_id {
            query.push_str(&format!(" AND session_id = ?{idx}"));
            params.push(sid.to_string().into());
            idx += 1;
        }
        if let Some(ref jid) = filter.job_id {
            query.push_str(&format!(" AND job_id = ?{idx}"));
            params.push(jid.to_string().into());
            idx += 1;
        }
        if let Some(ref uid) = filter.user_id {
            query.push_str(&format!(" AND user_id = ?{idx}"));
            params.push(uid.clone().into());
            idx += 1;
        }
        if let Some(ref et) = filter.event_type {
            query.push_str(&format!(" AND event_type = ?{idx}"));
            params.push(et.clone().into());
            idx += 1;
        }
        if let Some(ref after) = filter.after {
            query.push_str(&format!(" AND created_at > ?{idx}"));
            params.push(super::fmt_ts(after).into());
            idx += 1;
        }
        if let Some(ref before) = filter.before {
            query.push_str(&format!(" AND created_at < ?{idx}"));
            params.push(super::fmt_ts(before).into());
            idx += 1;
        }

        query.push_str(" ORDER BY created_at DESC");

        let limit = filter.limit.unwrap_or(1000);
        query.push_str(&format!(" LIMIT ?{idx}"));
        params.push(limit.into());

        let rows = conn
            .query(&query, libsql::params_from_iter(params))
            .await
            .map_err(|e| DatabaseError::Query(format!("audit_log query: {e}")))?;

        let mut records = Vec::new();
        let mut rows = rows;
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(format!("audit_log row: {e}")))?
        {
            let event_id: i64 = super::get_i64(&row, 0);
            let payload_str: String = super::get_text(&row, 9);
            let payload: serde_json::Value = serde_json::from_str(&payload_str).unwrap_or_default();

            records.push(AuditRecord {
                event_id: event_id as u64,
                event_type: super::get_text(&row, 1),
                source_module: super::get_text(&row, 2),
                source_component: super::get_text(&row, 3),
                category: super::get_text(&row, 4),
                session_id: parse_opt_uuid(&row, 5),
                thread_id: parse_opt_uuid(&row, 6),
                job_id: parse_opt_uuid(&row, 7),
                user_id: super::get_opt_text(&row, 8),
                payload,
                created_at: super::get_ts(&row, 10),
            });
        }

        Ok(records)
    }
}
