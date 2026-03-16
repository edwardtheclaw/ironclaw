//! Unified event bus — the single source of truth for system events.
//!
//! All producers (agent, tools, scheduler, channels) emit events through one
//! `EventBus`. Sinks subscribe and filter by category or payload type:
//!
//! - **SSE sink** → forwards `Domain` payloads to `SseManager` (web gateway)
//! - **Audit sink** → persists `Audit` events to the append-only audit log
//! - **State sink** → forwards `StateChange` payloads for cache invalidation
//! - **Metrics sink** → delegates `Metric`/`Telemetry` to `Observer` trait
//!
//! Hook events remain separate — hooks are bidirectional interceptors (can
//! reject/modify), the bus is unidirectional fire-and-forget.

pub mod bus;
pub mod event;
pub mod sinks;

pub use bus::EventBus;
pub use event::{
    EventCategory, EventContext, EventPayload, EventSource, SystemEvent, TelemetryPayload,
};
