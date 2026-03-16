//! The unified event bus.
//!
//! Single broadcast channel through which all system events flow.
//! Sinks subscribe and filter by category or payload type.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Utc;
use tokio::sync::broadcast;

use super::event::{
    EventCategory, EventContext, EventPayload, EventSource, SystemEvent, TelemetryPayload,
};

/// Buffer size for the broadcast channel.
const BUS_BUFFER: usize = 1024;

/// Unified event bus backed by `broadcast::Sender<Arc<SystemEvent>>`.
///
/// `Arc`-wrapping avoids deep-cloning payloads across multiple sinks.
/// The monotonic sequence counter ensures total ordering.
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Arc<SystemEvent>>,
    seq: Arc<AtomicU64>,
}

impl EventBus {
    /// Create a new event bus.
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BUS_BUFFER);
        Self {
            tx,
            seq: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Emit a raw event with explicit category.
    pub fn emit(
        &self,
        source: EventSource,
        category: EventCategory,
        context: EventContext,
        payload: EventPayload,
    ) {
        let event = Arc::new(SystemEvent {
            id: self.seq.fetch_add(1, Ordering::Relaxed),
            timestamp: Utc::now(),
            source,
            category,
            context,
            payload,
        });
        // Ignore send error (no active receivers is fine).
        let _ = self.tx.send(event);
    }

    /// Emit an event, auto-classifying category from the payload.
    pub fn emit_auto(&self, source: EventSource, context: EventContext, payload: EventPayload) {
        let category = payload.default_category();
        self.emit(source, category, context, payload);
    }

    /// Emit a `DomainEvent` (most common path — SSE broadcast).
    pub fn emit_domain(
        &self,
        source: EventSource,
        context: EventContext,
        event: crate::events::DomainEvent,
    ) {
        self.emit(
            source,
            EventCategory::Ephemeral,
            context,
            EventPayload::Domain(event),
        );
    }

    /// Emit a `StateChange` for cache invalidation.
    pub fn emit_state_change(&self, change: crate::state_bus::StateChange) {
        self.emit(
            EventSource::new("system", "state_bus"),
            EventCategory::StateChange,
            EventContext::empty(),
            EventPayload::StateChange(change),
        );
    }

    /// Emit a state machine transition (recorded in audit log).
    #[allow(clippy::too_many_arguments)]
    pub fn emit_transition(
        &self,
        source: EventSource,
        context: EventContext,
        entity_type: impl Into<String>,
        entity_id: impl Into<String>,
        from_state: impl Into<String>,
        to_state: impl Into<String>,
        reason: Option<String>,
    ) {
        self.emit(
            source,
            EventCategory::Audit,
            context,
            EventPayload::StateTransition {
                entity_type: entity_type.into(),
                entity_id: entity_id.into(),
                from_state: from_state.into(),
                to_state: to_state.into(),
                reason,
            },
        );
    }

    /// Emit a tool execution record.
    #[allow(clippy::too_many_arguments)]
    pub fn emit_tool_execution(
        &self,
        source: EventSource,
        context: EventContext,
        tool_name: impl Into<String>,
        parameters_hash: impl Into<String>,
        duration_ms: u64,
        success: bool,
        error: Option<String>,
    ) {
        self.emit(
            source,
            EventCategory::Audit,
            context,
            EventPayload::ToolExecution {
                tool_name: tool_name.into(),
                parameters_hash: parameters_hash.into(),
                duration_ms,
                success,
                error,
            },
        );
    }

    /// Emit a telemetry event.
    pub fn emit_telemetry(
        &self,
        source: EventSource,
        context: EventContext,
        telemetry: TelemetryPayload,
    ) {
        self.emit(
            source,
            EventCategory::Metric,
            context,
            EventPayload::Telemetry(telemetry),
        );
    }

    /// Subscribe to all events on this bus.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<SystemEvent>> {
        self.tx.subscribe()
    }

    /// Get the current sequence number (for testing/debugging).
    pub fn current_seq(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::DomainEvent;
    use tokio_stream::StreamExt;
    use tokio_stream::wrappers::BroadcastStream;

    #[tokio::test]
    async fn emit_and_receive() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        bus.emit_domain(
            EventSource::new("test", "unit"),
            EventContext::empty(),
            DomainEvent::Heartbeat,
        );

        let event = rx.recv().await.expect("should receive event"); // safety: test-only
        assert_eq!(event.id, 1); // safety: test-only
        assert_eq!(event.category, EventCategory::Ephemeral); // safety: test-only
        assert!(matches!( // safety: test-only
            event.payload,
            EventPayload::Domain(DomainEvent::Heartbeat)
        ));
    }

    #[tokio::test]
    async fn monotonic_sequence() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        for _ in 0..5 {
            bus.emit_domain(
                EventSource::new("test", "seq"),
                EventContext::empty(),
                DomainEvent::Heartbeat,
            );
        }

        let mut last_id = 0;
        for _ in 0..5 {
            let event = rx.recv().await.expect("should receive event"); // safety: test-only
            assert!(event.id > last_id, "IDs must be monotonically increasing"); // safety: test-only
            last_id = event.id;
        }
    }

    #[tokio::test]
    async fn multiple_subscribers() {
        let bus = EventBus::new();
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        bus.emit_state_change(crate::state_bus::StateChange::ConfigReloaded);

        let e1 = rx1.recv().await.expect("subscriber 1 should receive"); // safety: test-only
        let e2 = rx2.recv().await.expect("subscriber 2 should receive"); // safety: test-only
        assert_eq!(e1.id, e2.id); // safety: test-only
    }

    #[tokio::test]
    async fn no_subscriber_does_not_panic() {
        let bus = EventBus::new();
        bus.emit_domain(
            EventSource::new("test", "noop"),
            EventContext::empty(),
            DomainEvent::Heartbeat,
        );
        // Should not panic
    }

    #[tokio::test]
    async fn auto_category_from_payload() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        bus.emit_auto(
            EventSource::new("test", "auto"),
            EventContext::empty(),
            EventPayload::StateTransition {
                entity_type: "thread".into(),
                entity_id: "abc".into(),
                from_state: "Idle".into(),
                to_state: "Processing".into(),
                reason: None,
            },
        );

        let event = rx.recv().await.expect("should receive event"); // safety: test-only
        assert_eq!(event.category, EventCategory::Audit); // safety: test-only
    }

    #[tokio::test]
    async fn stream_adapter_works() {
        let bus = EventBus::new();
        let rx = bus.subscribe();
        let mut stream = BroadcastStream::new(rx);

        bus.emit_domain(
            EventSource::new("test", "stream"),
            EventContext::empty(),
            DomainEvent::Heartbeat,
        );

        let event = stream // safety: test-only
            .next()
            .await
            .expect("stream should yield") // safety: test-only
            .expect("no lag"); // safety: test-only
        assert_eq!(event.id, 1); // safety: test-only
    }

    #[tokio::test]
    async fn clone_shares_bus() {
        let bus1 = EventBus::new();
        let bus2 = bus1.clone();
        let mut rx = bus1.subscribe();

        bus2.emit_domain(
            EventSource::new("test", "clone"),
            EventContext::empty(),
            DomainEvent::Heartbeat,
        );

        let event = rx.recv().await.expect("should receive from cloned bus"); // safety: test-only
        assert_eq!(event.id, 1); // safety: test-only
    }
}
