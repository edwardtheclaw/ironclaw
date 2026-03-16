//! SSE sink — filters `Domain` payloads and forwards to `SseManager`.
//!
//! Drop-in replacement for direct `sse_tx.send()` calls. The web gateway's
//! SSE wire format is unchanged because `DomainEvent` serialization is
//! identical.

use std::sync::Arc;

use tokio::sync::broadcast;

use crate::event_bus::EventBus;
use crate::event_bus::event::{EventPayload, SystemEvent};
use crate::events::DomainEvent;

/// Spawn the SSE sink as a background task.
pub fn spawn(
    bus: &EventBus,
    sse_tx: broadcast::Sender<DomainEvent>,
) -> tokio::task::JoinHandle<()> {
    let mut rx = bus.subscribe();

    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => forward_if_domain(&event, &sse_tx),
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "SSE sink lagged behind event bus");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    tracing::debug!("Event bus closed, SSE sink shutting down");
                    break;
                }
            }
        }
    })
}

fn forward_if_domain(event: &Arc<SystemEvent>, sse_tx: &broadcast::Sender<DomainEvent>) {
    if let EventPayload::Domain(ref domain_event) = event.payload {
        // Ignore send error — no SSE subscribers is fine.
        let _ = sse_tx.send(domain_event.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_bus::event::{EventContext, EventSource};

    #[tokio::test]
    async fn forwards_domain_events() {
        let bus = EventBus::new();
        let (sse_tx, mut sse_rx) = broadcast::channel::<DomainEvent>(16);

        let _handle = spawn(&bus, sse_tx);

        bus.emit_domain(
            EventSource::new("test", "sse"),
            EventContext::empty(),
            DomainEvent::Heartbeat,
        );

        let received = tokio::time::timeout(std::time::Duration::from_millis(100), sse_rx.recv())
            .await
            .expect("should receive within timeout") // safety: test-only
            .expect("should not error"); // safety: test-only

        assert!(matches!(received, DomainEvent::Heartbeat)); // safety: test-only
    }

    #[tokio::test]
    async fn ignores_non_domain_events() {
        let bus = EventBus::new();
        let (sse_tx, mut sse_rx) = broadcast::channel::<DomainEvent>(16);

        let _handle = spawn(&bus, sse_tx);

        // Emit a non-domain event
        bus.emit_state_change(crate::state_bus::StateChange::ConfigReloaded);

        // Then emit a domain event so we know the sink is running
        bus.emit_domain(
            EventSource::new("test", "sse"),
            EventContext::empty(),
            DomainEvent::Heartbeat,
        );

        let received = tokio::time::timeout(std::time::Duration::from_millis(100), sse_rx.recv())
            .await
            .expect("should receive within timeout") // safety: test-only
            .expect("should not error"); // safety: test-only

        // Only the Heartbeat should arrive, not the StateChange
        assert!(matches!(received, DomainEvent::Heartbeat)); // safety: test-only
    }
}
