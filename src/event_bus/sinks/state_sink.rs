//! State sink — filters `StateChange` payloads and forwards to `StateBus`.
//!
//! Replaces direct `StateBus::publish()` calls. Modules that need state
//! invalidation subscribe to the `StateBus` as before — the sink bridges
//! the unified event bus to the existing invalidation mechanism.

use std::sync::Arc;

use tokio::sync::broadcast;

use crate::event_bus::EventBus;
use crate::event_bus::event::{EventPayload, SystemEvent};
use crate::state_bus::StateBus;

/// Spawn the state sink as a background task.
pub fn spawn(bus: &EventBus, state_bus: Arc<StateBus>) -> tokio::task::JoinHandle<()> {
    let mut rx = bus.subscribe();

    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => forward_if_state_change(&event, &state_bus),
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "State sink lagged behind event bus");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    tracing::debug!("Event bus closed, state sink shutting down");
                    break;
                }
            }
        }
    })
}

fn forward_if_state_change(event: &Arc<SystemEvent>, state_bus: &StateBus) {
    if let EventPayload::StateChange(ref change) = event.payload {
        state_bus.publish(change.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state_bus::StateChange;

    #[tokio::test]
    async fn forwards_state_changes() {
        let bus = EventBus::new();
        let state_bus = Arc::new(StateBus::new());
        let mut state_rx = state_bus.subscribe();

        let _handle = spawn(&bus, Arc::clone(&state_bus));

        bus.emit_state_change(StateChange::ConfigReloaded);

        let received = tokio::time::timeout(std::time::Duration::from_millis(100), state_rx.recv())
            .await
            .expect("should receive within timeout") // safety: test-only
            .expect("should not error"); // safety: test-only

        assert!(matches!(received, StateChange::ConfigReloaded)); // safety: test-only
    }
}
