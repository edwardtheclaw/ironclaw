//! Core event types for the unified event bus.
//!
//! `SystemEvent` is the tagged envelope that wraps all event payloads with
//! metadata (source, category, context). All events flow through one bus;
//! sinks filter by category or payload type.

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

/// Monotonic event envelope carrying metadata + payload.
#[derive(Debug, Clone, Serialize)]
pub struct SystemEvent {
    /// Monotonic sequence number assigned by the bus.
    pub id: u64,
    /// When the event was created.
    pub timestamp: DateTime<Utc>,
    /// Which module/component produced this event.
    pub source: EventSource,
    /// Classification controlling sink routing.
    pub category: EventCategory,
    /// Contextual identifiers for correlation.
    pub context: EventContext,
    /// The event-specific data.
    pub payload: EventPayload,
}

/// Which module and component produced the event.
#[derive(Debug, Clone, Serialize)]
pub struct EventSource {
    /// Top-level module (e.g. "agent", "worker", "orchestrator").
    pub module: String,
    /// Specific component within the module (e.g. "dispatcher", "scheduler").
    pub component: String,
}

impl EventSource {
    pub fn new(module: impl Into<String>, component: impl Into<String>) -> Self {
        Self {
            module: module.into(),
            component: component.into(),
        }
    }
}

/// Event classification for sink routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum EventCategory {
    /// Security-relevant events that must be persisted (append-only audit log).
    Audit,
    /// Transient events (SSE broadcast, status updates) — OK to drop.
    Ephemeral,
    /// State machine transitions — recorded for debugging and audit.
    StateChange,
    /// Numeric metrics and telemetry.
    Metric,
}

/// Contextual identifiers for event correlation.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EventContext {
    /// Session ID (if applicable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Uuid>,
    /// Thread ID (if applicable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Uuid>,
    /// Job ID (if applicable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<Uuid>,
    /// User ID (if applicable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

impl EventContext {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn with_job(job_id: Uuid) -> Self {
        Self {
            job_id: Some(job_id),
            ..Default::default()
        }
    }

    pub fn with_thread(session_id: Uuid, thread_id: Uuid) -> Self {
        Self {
            session_id: Some(session_id),
            thread_id: Some(thread_id),
            ..Default::default()
        }
    }

    pub fn with_user(user_id: impl Into<String>) -> Self {
        Self {
            user_id: Some(user_id.into()),
            ..Default::default()
        }
    }
}

/// Telemetry payload for metrics events.
#[derive(Debug, Clone, Serialize)]
pub enum TelemetryPayload {
    /// LLM call latency and token usage.
    LlmCall {
        provider: String,
        model: String,
        duration_ms: u64,
        tokens_used: Option<u64>,
        success: bool,
    },
    /// Channel message processed.
    ChannelMessage { channel: String, direction: String },
    /// Heartbeat tick.
    HeartbeatTick,
}

/// The event-specific data carried inside a `SystemEvent`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum EventPayload {
    /// Wraps an existing `DomainEvent` — SSE wire format unchanged.
    Domain(crate::events::DomainEvent),

    /// State invalidation notification (wraps existing `StateChange`).
    StateChange(crate::state_bus::StateChange),

    /// Telemetry / metrics data.
    Telemetry(TelemetryPayload),

    /// A validated state machine transition.
    StateTransition {
        entity_type: String,
        entity_id: String,
        from_state: String,
        to_state: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },

    /// Tool execution record.
    ToolExecution {
        tool_name: String,
        /// SHA-256 prefix of parameters (not the raw params — privacy).
        parameters_hash: String,
        duration_ms: u64,
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// Authentication / authorization event.
    AuthEvent {
        action: String,
        target: String,
        success: bool,
    },

    /// Configuration change.
    ConfigChange { key: String, changed_by: String },
}

impl EventPayload {
    /// Classify this payload into a category for sink routing.
    pub fn default_category(&self) -> EventCategory {
        match self {
            Self::Domain(_) => EventCategory::Ephemeral,
            Self::StateChange(_) => EventCategory::StateChange,
            Self::Telemetry(_) => EventCategory::Metric,
            Self::StateTransition { .. } => EventCategory::Audit,
            Self::ToolExecution { .. } => EventCategory::Audit,
            Self::AuthEvent { .. } => EventCategory::Audit,
            Self::ConfigChange { .. } => EventCategory::Audit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_source_construction() {
        let source = EventSource::new("agent", "dispatcher");
        assert_eq!(source.module, "agent"); // safety: test-only
        assert_eq!(source.component, "dispatcher"); // safety: test-only
    }

    #[test]
    fn event_context_builders() {
        let ctx = EventContext::empty();
        assert!(ctx.session_id.is_none()); // safety: test-only

        let job_id = Uuid::new_v4();
        let ctx = EventContext::with_job(job_id);
        assert_eq!(ctx.job_id, Some(job_id)); // safety: test-only

        let sid = Uuid::new_v4();
        let tid = Uuid::new_v4();
        let ctx = EventContext::with_thread(sid, tid);
        assert_eq!(ctx.session_id, Some(sid)); // safety: test-only
        assert_eq!(ctx.thread_id, Some(tid)); // safety: test-only

        let ctx = EventContext::with_user("alice");
        assert_eq!(ctx.user_id.as_deref(), Some("alice")); // safety: test-only
    }

    #[test]
    fn payload_default_categories() {
        assert_eq!( // safety: test-only
            EventPayload::Domain(crate::events::DomainEvent::Heartbeat).default_category(),
            EventCategory::Ephemeral
        );
        assert_eq!( // safety: test-only
            EventPayload::StateTransition {
                entity_type: "thread".into(),
                entity_id: "abc".into(),
                from_state: "Idle".into(),
                to_state: "Processing".into(),
                reason: None,
            }
            .default_category(),
            EventCategory::Audit
        );
        assert_eq!( // safety: test-only
            EventPayload::Telemetry(TelemetryPayload::HeartbeatTick).default_category(),
            EventCategory::Metric
        );
    }
}
