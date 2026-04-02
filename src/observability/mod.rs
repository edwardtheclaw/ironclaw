//! Observability subsystem: trait-based event and metric recording.
//!
//! Provides a pluggable [`Observer`] trait with multiple backends:
//!
//! | Backend | Description |
//! |---------|-------------|
//! | `noop`  | Zero overhead, discards everything (default) |
//! | `log`   | Emits structured events via `tracing` |
//! | `multi` | Fan-out to multiple backends simultaneously |
//!
//! The [`create_observer`] factory builds the right backend from
//! [`ObservabilityConfig`]. Future backends (OpenTelemetry, Prometheus)
//! can be added by implementing [`Observer`].

mod log;
mod multi;
mod noop;
pub mod prometheus;
pub mod traits;

pub use self::log::LogObserver;
pub use self::multi::MultiObserver;
pub use self::noop::NoopObserver;
pub use self::prometheus::{PrometheusMetrics, PrometheusObserver};
pub use self::traits::{Observer, ObserverEvent, ObserverMetric};

/// Configuration for the observability backend.
#[derive(Debug, Clone)]
pub struct ObservabilityConfig {
    /// Backend name: "none", "noop", "log", "prometheus".
    pub backend: String,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            backend: "none".into(),
        }
    }
}

/// Create an observer from configuration.
///
/// Returns a [`NoopObserver`] for "none"/"noop" (or unknown values),
/// a [`LogObserver`] for "log", and a [`PrometheusObserver`] for "prometheus".
///
/// For the Prometheus backend, the shared [`PrometheusMetrics`] handle is
/// returned alongside the observer so callers can expose `GET /metrics`.
pub fn create_observer(
    config: &ObservabilityConfig,
) -> (Box<dyn Observer>, Option<std::sync::Arc<PrometheusMetrics>>) {
    match config.backend.as_str() {
        "log" => (Box::new(LogObserver), None),
        "prometheus" => {
            let (obs, metrics) = PrometheusObserver::new();
            (Box::new(obs), Some(metrics))
        }
        _ => (Box::new(NoopObserver), None),
    }
}

#[cfg(test)]
mod tests {
    use crate::observability::*;

    #[test]
    fn default_config_is_none() {
        let cfg = ObservabilityConfig::default();
        assert_eq!(cfg.backend, "none");
    }

    #[test]
    fn factory_returns_noop_for_none() {
        let cfg = ObservabilityConfig {
            backend: "none".into(),
        };
        let (obs, metrics) = create_observer(&cfg);
        assert_eq!(obs.name(), "noop");
        assert!(metrics.is_none());
    }

    #[test]
    fn factory_returns_noop_for_empty() {
        let cfg = ObservabilityConfig {
            backend: String::new(),
        };
        let (obs, _) = create_observer(&cfg);
        assert_eq!(obs.name(), "noop");
    }

    #[test]
    fn factory_returns_prometheus_for_prometheus() {
        let cfg = ObservabilityConfig {
            backend: "prometheus".into(),
        };
        let (obs, metrics) = create_observer(&cfg);
        assert_eq!(obs.name(), "prometheus");
        assert!(metrics.is_some());
    }

    #[test]
    fn factory_returns_log_for_log() {
        let cfg = ObservabilityConfig {
            backend: "log".into(),
        };
        let (obs, _) = create_observer(&cfg);
        assert_eq!(obs.name(), "log");
    }

    #[test]
    fn factory_returns_noop_for_noop() {
        let cfg = ObservabilityConfig {
            backend: "noop".into(),
        };
        let (obs, _) = create_observer(&cfg);
        assert_eq!(obs.name(), "noop");
    }
}
