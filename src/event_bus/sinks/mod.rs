//! Event bus sinks — subscribers that consume events by category.
//!
//! Each sink runs as a background task, filtering events and routing
//! them to the appropriate subsystem.

pub mod audit_sink;
pub mod metrics_sink;
pub mod sse_sink;
pub mod state_sink;

use std::sync::Arc;

use crate::event_bus::EventBus;

/// Spawn all configured sinks as background tasks.
///
/// Returns `JoinHandle`s so the caller can abort them on shutdown.
pub fn spawn_sinks(
    bus: &EventBus,
    sse_tx: Option<tokio::sync::broadcast::Sender<crate::events::DomainEvent>>,
    state_bus: Option<Arc<crate::state_bus::StateBus>>,
    observer: Option<Arc<dyn crate::observability::Observer>>,
    audit_store: Option<Arc<dyn crate::db::AuditStore>>,
) -> Vec<tokio::task::JoinHandle<()>> {
    let mut handles = Vec::new();

    // SSE sink — bridges Domain events to the web gateway
    if let Some(tx) = sse_tx {
        handles.push(sse_sink::spawn(bus, tx));
    }

    // State sink — bridges StateChange events to the StateBus
    if let Some(sb) = state_bus {
        handles.push(state_sink::spawn(bus, sb));
    }

    // Metrics sink — bridges Telemetry/Metric events to Observer
    if let Some(obs) = observer {
        handles.push(metrics_sink::spawn(bus, obs));
    }

    // Audit sink — persists Audit events to the database
    if let Some(store) = audit_store {
        handles.push(audit_sink::spawn(bus, store));
    }

    handles
}
