//! Audit sink — persists `Audit`-category events to the append-only audit log.
//!
//! Batches events (up to 32, or 500ms timeout) before flushing to the database.
//! On DB failure, falls back to a local JSONL file so audit data is never lost.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;

use crate::db::AuditStore;
use crate::event_bus::EventBus;
use crate::event_bus::event::{EventCategory, SystemEvent};

/// Maximum events to batch before flushing.
const BATCH_SIZE: usize = 32;

/// Maximum time to wait before flushing a partial batch.
const FLUSH_INTERVAL: Duration = Duration::from_millis(500);

/// Spawn the audit sink as a background task.
pub fn spawn(bus: &EventBus, store: Arc<dyn AuditStore>) -> tokio::task::JoinHandle<()> {
    let mut rx = bus.subscribe();

    tokio::spawn(async move {
        let mut batch: Vec<Arc<SystemEvent>> = Vec::with_capacity(BATCH_SIZE);
        let mut flush_timer = tokio::time::interval(FLUSH_INTERVAL);
        // First tick completes immediately — skip it.
        flush_timer.tick().await;

        loop {
            tokio::select! {
                result = rx.recv() => {
                    match result {
                        Ok(event) => {
                            if event.category == EventCategory::Audit {
                                batch.push(event);
                                if batch.len() >= BATCH_SIZE {
                                    flush_batch(&store, &mut batch).await;
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(skipped = n, "Audit sink lagged behind event bus");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            // Flush remaining events before shutdown.
                            if !batch.is_empty() {
                                flush_batch(&store, &mut batch).await;
                            }
                            tracing::debug!("Event bus closed, audit sink shutting down");
                            break;
                        }
                    }
                }
                _ = flush_timer.tick() => {
                    if !batch.is_empty() {
                        flush_batch(&store, &mut batch).await;
                    }
                }
            }
        }
    })
}

async fn flush_batch(store: &Arc<dyn AuditStore>, batch: &mut Vec<Arc<SystemEvent>>) {
    let records: Vec<crate::db::AuditRecord> = batch
        .iter()
        .map(|e| crate::db::AuditRecord {
            event_id: e.id,
            event_type: event_type_name(&e.payload),
            source_module: e.source.module.clone(),
            source_component: e.source.component.clone(),
            category: format!("{:?}", e.category),
            session_id: e.context.session_id,
            thread_id: e.context.thread_id,
            job_id: e.context.job_id,
            user_id: e.context.user_id.clone(),
            payload: serde_json::to_value(&e.payload).unwrap_or_default(),
            created_at: e.timestamp,
        })
        .collect();

    if let Err(e) = store.append_audit_events(&records).await {
        tracing::error!(count = records.len(), error = %e, "Failed to persist audit events to DB, falling back to file");
        fallback_to_file(&records);
    }

    batch.clear();
}

/// Extract a short event type name from the payload for indexing.
fn event_type_name(payload: &crate::event_bus::event::EventPayload) -> String {
    use crate::event_bus::event::EventPayload;
    match payload {
        EventPayload::Domain(_) => "domain".to_string(),
        EventPayload::StateChange(_) => "state_change".to_string(),
        EventPayload::Telemetry(_) => "telemetry".to_string(),
        EventPayload::StateTransition { .. } => "state_transition".to_string(),
        EventPayload::ToolExecution { .. } => "tool_execution".to_string(),
        EventPayload::AuthEvent { .. } => "auth_event".to_string(),
        EventPayload::ConfigChange { .. } => "config_change".to_string(),
    }
}

/// Fallback: append audit records as JSONL to a local file.
fn fallback_to_file(records: &[crate::db::AuditRecord]) {
    let fallback_path = crate::bootstrap::ironclaw_base_dir().join("audit.fallback.jsonl");
    let file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&fallback_path)
    {
        Ok(f) => f,
        Err(e) => {
            tracing::error!(path = %fallback_path.display(), error = %e, "Cannot open audit fallback file");
            return;
        }
    };

    let mut writer = std::io::BufWriter::new(file);
    for record in records {
        if let Err(e) = serde_json::to_writer(&mut writer, record) {
            tracing::error!(error = %e, "Failed to write audit record to fallback file");
        } else {
            use std::io::Write;
            let _ = writer.write_all(b"\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_type_names() {
        use crate::event_bus::event::EventPayload;

        assert_eq!( // safety: test-only
            event_type_name(&EventPayload::StateTransition {
                entity_type: "t".into(),
                entity_id: "i".into(),
                from_state: "a".into(),
                to_state: "b".into(),
                reason: None,
            }),
            "state_transition"
        );
        assert_eq!( // safety: test-only
            event_type_name(&EventPayload::ToolExecution {
                tool_name: "echo".into(),
                parameters_hash: "abc".into(),
                duration_ms: 10,
                success: true,
                error: None,
            }),
            "tool_execution"
        );
    }
}
