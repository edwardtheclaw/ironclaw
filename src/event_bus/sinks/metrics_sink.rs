//! Metrics sink — filters `Telemetry`/`Metric` events and delegates to `Observer`.
//!
//! Bridges the unified event bus to the existing `Observer` trait so that
//! `LogObserver`, future OpenTelemetry exporters, etc. continue to work.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;

use crate::event_bus::EventBus;
use crate::event_bus::event::{EventCategory, EventPayload, SystemEvent, TelemetryPayload};
use crate::observability::traits::{Observer, ObserverEvent};

/// Spawn the metrics sink as a background task.
pub fn spawn(bus: &EventBus, observer: Arc<dyn Observer>) -> tokio::task::JoinHandle<()> {
    let mut rx = bus.subscribe();

    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => forward_if_metric(&event, &observer),
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "Metrics sink lagged behind event bus");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    tracing::debug!("Event bus closed, metrics sink shutting down");
                    break;
                }
            }
        }
    })
}

fn forward_if_metric(event: &Arc<SystemEvent>, observer: &Arc<dyn Observer>) {
    if event.category != EventCategory::Metric {
        return;
    }

    if let EventPayload::Telemetry(ref telemetry) = event.payload {
        match telemetry {
            TelemetryPayload::LlmCall {
                provider,
                model,
                duration_ms,
                success,
                ..
            } => {
                observer.record_event(&ObserverEvent::LlmResponse {
                    provider: provider.clone(),
                    model: model.clone(),
                    duration: Duration::from_millis(*duration_ms),
                    success: *success,
                    error_message: None,
                });
            }
            TelemetryPayload::ChannelMessage { channel, direction } => {
                observer.record_event(&ObserverEvent::ChannelMessage {
                    channel: channel.clone(),
                    direction: direction.clone(),
                });
            }
            TelemetryPayload::HeartbeatTick => {
                observer.record_event(&ObserverEvent::HeartbeatTick);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_bus::event::{EventContext, EventSource};
    use crate::observability::traits::ObserverMetric;
    use std::sync::Mutex;

    struct RecordingObserver {
        events: Mutex<Vec<String>>,
    }

    impl RecordingObserver {
        fn new() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
            }
        }

        fn recorded(&self) -> Vec<String> {
            self.events.lock().expect("test lock").clone() // safety: test-only
        }
    }

    impl Observer for RecordingObserver {
        fn record_event(&self, event: &ObserverEvent) {
            let name = match event {
                ObserverEvent::LlmResponse { .. } => "llm_response",
                ObserverEvent::ChannelMessage { .. } => "channel_message",
                ObserverEvent::HeartbeatTick => "heartbeat_tick",
                _ => "other",
            };
            self.events
                .lock()
                .expect("test lock") // safety: test-only
                .push(name.to_string());
        }

        fn record_metric(&self, _metric: &ObserverMetric) {}
        fn name(&self) -> &str {
            "test-recorder"
        }
    }

    #[tokio::test]
    async fn forwards_telemetry_to_observer() {
        let bus = EventBus::new();
        let observer = Arc::new(RecordingObserver::new());

        let _handle = spawn(&bus, Arc::clone(&observer) as Arc<dyn Observer>);

        bus.emit_telemetry(
            EventSource::new("test", "metrics"),
            EventContext::empty(),
            TelemetryPayload::HeartbeatTick,
        );

        // Give the sink a moment to process
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let recorded = observer.recorded();
        assert_eq!(recorded, vec!["heartbeat_tick"]); // safety: test-only
    }
}
