//! Prometheus metrics backend.
//!
//! Tracks agent lifecycle counters and gauges using lock-free atomics and
//! exports them as a Prometheus text-format scrape endpoint (`GET /metrics`).
//!
//! No external `prometheus` crate is required — the text format is simple
//! enough to generate with `std::fmt`.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

use crate::observability::traits::{Observer, ObserverEvent, ObserverMetric};

/// Shared atomic counters and gauges for Prometheus scraping.
///
/// Held behind `Arc` so both the [`PrometheusObserver`] and the HTTP handler
/// can reference the same data.
#[derive(Debug, Default)]
pub struct PrometheusMetrics {
    pub llm_requests_total: AtomicU64,
    pub llm_responses_ok_total: AtomicU64,
    pub llm_responses_err_total: AtomicU64,
    pub tool_calls_total: AtomicU64,
    pub tool_calls_ok_total: AtomicU64,
    pub tool_calls_err_total: AtomicU64,
    pub agent_turns_total: AtomicU64,
    pub channel_messages_total: AtomicU64,
    pub heartbeat_ticks_total: AtomicU64,
    pub tokens_used_total: AtomicU64,
    /// Nanoseconds total (for computing average latency).
    pub request_latency_ns_total: AtomicU64,
    pub request_latency_count: AtomicU64,
    /// Gauge: active jobs (can go negative during races, clamped to 0 in output).
    pub active_jobs: AtomicI64,
    /// Gauge: queue depth.
    pub queue_depth: AtomicI64,
}

impl PrometheusMetrics {
    /// Render all metrics in Prometheus text exposition format.
    pub fn render(&self) -> String {
        let latency_count = self.request_latency_count.load(Ordering::Relaxed);
        let latency_ns = self.request_latency_ns_total.load(Ordering::Relaxed);
        let latency_seconds_sum = latency_ns as f64 / 1_000_000_000.0;

        let active_jobs = self.active_jobs.load(Ordering::Relaxed).max(0);
        let queue_depth = self.queue_depth.load(Ordering::Relaxed).max(0);

        format!(
            "# HELP ironclaw_llm_requests_total Total LLM requests sent.\n\
             # TYPE ironclaw_llm_requests_total counter\n\
             ironclaw_llm_requests_total {llm_requests}\n\
             # HELP ironclaw_llm_responses_total Total LLM responses received.\n\
             # TYPE ironclaw_llm_responses_total counter\n\
             ironclaw_llm_responses_total{{result=\"ok\"}} {llm_ok}\n\
             ironclaw_llm_responses_total{{result=\"err\"}} {llm_err}\n\
             # HELP ironclaw_tool_calls_total Total tool calls executed.\n\
             # TYPE ironclaw_tool_calls_total counter\n\
             ironclaw_tool_calls_total{{result=\"ok\"}} {tools_ok}\n\
             ironclaw_tool_calls_total{{result=\"err\"}} {tools_err}\n\
             # HELP ironclaw_agent_turns_total Total agent reasoning turns completed.\n\
             # TYPE ironclaw_agent_turns_total counter\n\
             ironclaw_agent_turns_total {turns}\n\
             # HELP ironclaw_channel_messages_total Total channel messages processed.\n\
             # TYPE ironclaw_channel_messages_total counter\n\
             ironclaw_channel_messages_total {channel_msgs}\n\
             # HELP ironclaw_heartbeat_ticks_total Total heartbeat ticks fired.\n\
             # TYPE ironclaw_heartbeat_ticks_total counter\n\
             ironclaw_heartbeat_ticks_total {heartbeats}\n\
             # HELP ironclaw_tokens_used_total Cumulative LLM tokens consumed.\n\
             # TYPE ironclaw_tokens_used_total counter\n\
             ironclaw_tokens_used_total {tokens}\n\
             # HELP ironclaw_request_latency_seconds Request latency histogram (sum/count).\n\
             # TYPE ironclaw_request_latency_seconds summary\n\
             ironclaw_request_latency_seconds_sum {latency_sum}\n\
             ironclaw_request_latency_seconds_count {latency_count}\n\
             # HELP ironclaw_active_jobs Current number of active agent jobs.\n\
             # TYPE ironclaw_active_jobs gauge\n\
             ironclaw_active_jobs {active_jobs}\n\
             # HELP ironclaw_queue_depth Current agent message queue depth.\n\
             # TYPE ironclaw_queue_depth gauge\n\
             ironclaw_queue_depth {queue_depth}\n",
            llm_requests = self.llm_requests_total.load(Ordering::Relaxed),
            llm_ok = self.llm_responses_ok_total.load(Ordering::Relaxed),
            llm_err = self.llm_responses_err_total.load(Ordering::Relaxed),
            tools_ok = self.tool_calls_ok_total.load(Ordering::Relaxed),
            tools_err = self.tool_calls_err_total.load(Ordering::Relaxed),
            turns = self.agent_turns_total.load(Ordering::Relaxed),
            channel_msgs = self.channel_messages_total.load(Ordering::Relaxed),
            heartbeats = self.heartbeat_ticks_total.load(Ordering::Relaxed),
            tokens = self.tokens_used_total.load(Ordering::Relaxed),
            latency_sum = latency_seconds_sum,
            latency_count = latency_count,
            active_jobs = active_jobs,
            queue_depth = queue_depth,
        )
    }
}

/// [`Observer`] backend that writes to [`PrometheusMetrics`] atomics.
pub struct PrometheusObserver {
    pub metrics: Arc<PrometheusMetrics>,
}

impl PrometheusObserver {
    pub fn new() -> (Self, Arc<PrometheusMetrics>) {
        let metrics = Arc::new(PrometheusMetrics::default());
        let observer = Self {
            metrics: Arc::clone(&metrics),
        };
        (observer, metrics)
    }
}

impl Observer for PrometheusObserver {
    fn name(&self) -> &str {
        "prometheus"
    }

    fn record_event(&self, event: &ObserverEvent) {
        match event {
            ObserverEvent::LlmRequest { .. } => {
                self.metrics.llm_requests_total.fetch_add(1, Ordering::Relaxed);
            }
            ObserverEvent::LlmResponse { success, .. } => {
                if *success {
                    self.metrics.llm_responses_ok_total.fetch_add(1, Ordering::Relaxed);
                } else {
                    self.metrics.llm_responses_err_total.fetch_add(1, Ordering::Relaxed);
                }
            }
            ObserverEvent::ToolCallEnd { success, .. } => {
                if *success {
                    self.metrics.tool_calls_ok_total.fetch_add(1, Ordering::Relaxed);
                } else {
                    self.metrics.tool_calls_err_total.fetch_add(1, Ordering::Relaxed);
                }
                self.metrics.tool_calls_total.fetch_add(1, Ordering::Relaxed);
            }
            ObserverEvent::TurnComplete => {
                self.metrics.agent_turns_total.fetch_add(1, Ordering::Relaxed);
            }
            ObserverEvent::ChannelMessage { .. } => {
                self.metrics.channel_messages_total.fetch_add(1, Ordering::Relaxed);
            }
            ObserverEvent::HeartbeatTick => {
                self.metrics.heartbeat_ticks_total.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }

    fn record_metric(&self, metric: &ObserverMetric) {
        match metric {
            ObserverMetric::TokensUsed(n) => {
                self.metrics.tokens_used_total.fetch_add(*n, Ordering::Relaxed);
            }
            ObserverMetric::RequestLatency(d) => {
                self.metrics
                    .request_latency_ns_total
                    .fetch_add(duration_to_nanos(d), Ordering::Relaxed);
                self.metrics.request_latency_count.fetch_add(1, Ordering::Relaxed);
            }
            ObserverMetric::ActiveJobs(n) => {
                self.metrics.active_jobs.store(*n as i64, Ordering::Relaxed);
            }
            ObserverMetric::QueueDepth(n) => {
                self.metrics.queue_depth.store(*n as i64, Ordering::Relaxed);
            }
        }
    }
}

fn duration_to_nanos(d: &Duration) -> u64 {
    d.as_secs()
        .saturating_mul(1_000_000_000)
        .saturating_add(d.subsec_nanos() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observability::traits::{ObserverEvent, ObserverMetric};
    use std::time::Duration;

    #[test]
    fn counters_increment() {
        let (obs, metrics) = PrometheusObserver::new();
        obs.record_event(&ObserverEvent::LlmRequest {
            provider: "test".into(),
            model: "m".into(),
            message_count: 1,
        });
        obs.record_event(&ObserverEvent::LlmResponse {
            provider: "test".into(),
            model: "m".into(),
            duration: Duration::from_millis(100),
            success: true,
            error_message: None,
        });
        obs.record_event(&ObserverEvent::TurnComplete);
        obs.record_metric(&ObserverMetric::TokensUsed(500));

        assert_eq!(metrics.llm_requests_total.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.llm_responses_ok_total.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.agent_turns_total.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.tokens_used_total.load(Ordering::Relaxed), 500);
    }

    #[test]
    fn render_produces_valid_prometheus_text() {
        let (obs, metrics) = PrometheusObserver::new();
        obs.record_event(&ObserverEvent::HeartbeatTick);
        obs.record_metric(&ObserverMetric::ActiveJobs(3));
        let output = metrics.render();
        assert!(output.contains("ironclaw_heartbeat_ticks_total 1"));
        assert!(output.contains("ironclaw_active_jobs 3"));
        assert!(output.contains("# TYPE ironclaw_llm_requests_total counter"));
    }

    #[test]
    fn observer_name_is_prometheus() {
        let (obs, _) = PrometheusObserver::new();
        assert_eq!(obs.name(), "prometheus");
    }
}
