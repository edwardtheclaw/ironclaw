//! WebhookAuditHook — records every inbound webhook delivery to the audit log.
//!
//! Fires on `HookEvent::Inbound` for messages from the "webhook" channel.
//! Records the channel, a SHA-256 hash of the content, and user_id.
//! HMAC validity is always `Some(true)` at this point because the HTTP channel
//! rejects invalid HMAC signatures before the hook fires.

use std::sync::Arc;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::db::Database;
use crate::history::WebhookEventRecord;
use crate::hooks::hook::{Hook, HookCategory, HookContext, HookError, HookEvent, HookOutcome, HookPoint};

/// Hook that writes inbound webhook events to the audit log table.
pub struct WebhookAuditHook {
    db: Arc<dyn Database>,
}

impl WebhookAuditHook {
    pub fn new(db: Arc<dyn Database>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl Hook for WebhookAuditHook {
    fn name(&self) -> &str {
        "webhook_audit"
    }

    fn hook_points(&self) -> &[HookPoint] {
        &[HookPoint::BeforeInbound]
    }

    fn category(&self) -> HookCategory {
        HookCategory::Audit
    }

    async fn execute(
        &self,
        event: &HookEvent,
        _ctx: &HookContext,
    ) -> Result<HookOutcome, HookError> {
        if let HookEvent::Inbound {
            user_id,
            channel,
            content,
            ..
        } = event
        {
            // Only audit messages from the webhook channel.
            if channel != "webhook" {
                return Ok(HookOutcome::ok());
            }

            let payload_hash = format!("{:x}", Sha256::digest(content.as_bytes()));
            let record = WebhookEventRecord {
                id: Uuid::new_v4(),
                received_at: chrono::Utc::now(),
                channel: channel.clone(),
                // HMAC is always valid here: http.rs returns 403 before the hook fires.
                hmac_valid: Some(true),
                payload_hash,
                user_id: user_id.clone(),
            };

            if let Err(e) = self.db.insert_webhook_event(&record).await {
                tracing::warn!("webhook_audit: failed to record event: {}", e);
            }
        }
        Ok(HookOutcome::ok())
    }
}
