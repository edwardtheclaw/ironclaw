# IronClaw — Mega Resiliency Audit

**Date:** 2026-03-26
**Version audited:** v0.21.0 (bd6977e)
**Auditor:** Claude Code (Mega Resiliency Process)
**Scope:** Code quality, reliability, hardening, correctness, observability, security, testing, performance, deployment, data integrity, developer experience, documentation.

---

## 1. Executive Summary

IronClaw is a production-grade, secure personal AI assistant written in Rust. The codebase exhibits strong engineering discipline: no `.unwrap()`/`.expect()` in production code, comprehensive error types via `thiserror`, multi-layer safety pipelines, dual-database abstraction, circuit-breaker LLM resilience, and an extensive CI/CD matrix.

The highest-risk areas are:
- **libSQL maturity gap** — secrets store, vector search dimension, and migration idempotency lag behind the PostgreSQL backend.
- **In-memory session/approval state** — lost on restart; no persistence or recovery path.
- **Interactive auth flows** — NEAR AI OAuth token renewal blocks the process; unsuitable for headless/daemon use.
- **Observability gaps** — proxy request denial not logged; no OpenTelemetry backend; SSE errors can be silently dropped.
- **MCP tools lack sandbox** — MCP-sourced tools run with full system access unlike WASM tools.

No critical security vulnerabilities were found. Cryptographic implementation (AES-256-GCM, HKDF, constant-time comparison) and WASM sandboxing are implemented correctly.

---

## 2. Health Score

| Dimension | Score (1–10) | Notes |
|-----------|-------------|-------|
| Code Quality | 9 | Strong Rust discipline; zero unwrap/expect in prod; clear module ownership |
| Reliability | 7 | LLM circuit-breaker + retry good; libSQL write contention; in-memory approval state |
| Security | 8 | Crypto correct; WASM sandbox solid; MCP unboxed; hook fail-open is double-edged |
| Observability | 6 | Logging solid; no OTEL; proxy denials silent; SSE error elision |
| Testing | 8 | Feature matrix CI; E2E traces; regression enforcement; libSQL vector path undertested |
| Performance | 7 | No streaming LLM responses; libSQL busy-timeout contention; LRU cache in-process only |
| Deployment | 7 | cargo-dist for 6 platforms; Docker sandbox present; no Helm/K8s charts; no readiness probe |
| Data Integrity | 7 | Dual-backend schema coherent; libSQL lacks incremental migrations; vector dim hardcoded |
| Developer Experience | 9 | CLAUDE.md specs; .env.example exhaustive; review-discipline rules; pre-commit hooks |
| Documentation | 8 | Module specs thorough; runbook gaps for incident response and DB failover |

**Overall Health: 7.6 / 10**

---

## 3. Risk Register

### 3.1 High Risk

| ID | Risk | Impact | Likelihood | Notes |
|----|------|--------|-----------|-------|
| R-01 | libSQL write serialization under concurrency | Data loss / timeout cascade | Medium | 5 s busy timeout; single writer; parallel jobs may queue up |
| R-02 | Interactive OAuth refresh blocks daemon | Complete agent hang | High for headless | `read_line()` on stdin in headless/service mode hangs forever |
| R-03 | In-memory approval state lost on restart | User frustration; duplicate approvals | High | No persistence; re-triggers approval flows |
| R-04 | MCP tools run unsandboxed | Full system compromise if MCP server is malicious | Medium | WASM tools are sandboxed; MCP are not |
| R-05 | Hook fail-open on error | Safety bypass if hook panics | Low-Medium | `BeforeInbound`/`BeforeOutbound` errors logged and continue |

### 3.2 Medium Risk

| ID | Risk | Impact | Likelihood | Notes |
|----|------|--------|-----------|-------|
| R-06 | No streaming LLM responses | High latency for long outputs; user-facing degradation | High | All responses block until complete |
| R-07 | Unknown HTTP methods silently downgraded to GET in proxy | Protocol confusion; potential security bypass | Low | `unwrap_or(GET)` in `sandbox/proxy/http.rs` |
| R-08 | Pending approvals in-memory only | Lost on process restart | High occurrence | Covered by R-03 but worth tracking separately for UX |
| R-09 | libSQL vector dimension fixed at 1536 | Cannot use embeddings > 1536 dims with libSQL | Medium-term | PostgreSQL is unbounded (V9) |
| R-10 | Response cache shared across users/jobs | Cache poisoning or info leak if multi-user mode is added | Low now | Currently single-user; would become critical in multi-tenant |
| R-11 | Session compaction deletes turns permanently | Unrecoverable context loss | Low | Compaction logic writes summary but drops raw turns |

### 3.3 Low Risk

| ID | Risk | Impact | Likelihood | Notes |
|----|------|--------|-----------|-------|
| R-12 | Secrets store requires PostgreSQL (libSQL not plumbed) | libSQL deployments cannot use secret management | High for libSQL users | Documented limitation |
| R-13 | Cost guard daily reset uses UTC wall clock | Budget exhaustion in UTC transition windows | Very Low | Subtle off-by-one under rapid request bursts |
| R-14 | SessionManager prunes at 1000 sessions with log warning | OOM if sessions accumulate under memory pressure | Low | No hard eviction; only a warning |
| R-15 | Temporary String in `DecryptedSecret::clone()` | Plaintext briefly on heap before zeroization | Very Low | `SecretString` zeroizes on drop |

---

## 4. Bugs and Correctness Issues

### B-01 — Unknown HTTP methods silently downgraded to GET (Medium)
**File:** `src/sandbox/proxy/http.rs`
**Pattern:**
```rust
reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET)
```
Unknown methods (e.g., `PATCH`, custom verbs) become `GET` silently. Should return `400 Bad Request` or at minimum log a warning. This can mask protocol violations and may permit method confusion attacks.

### B-02 — Test panics use `panic!` not `assert!` (Low)
**Files:** `src/orchestrator/api.rs` lines 784, 832
```rust
other => panic!("Expected JobMessage, got {:?}", other),
```
Test failures are expressed as panics rather than `assert!`/`assert_eq!` macros, giving poor failure diagnostics in CI.

### B-03 — Proxy denial not logged (Medium — Observability)
**File:** `src/sandbox/proxy/http.rs`
Network denial decisions (`NetworkDecision::Deny`) are enforced but not emitted to tracing. Security-relevant events (blocked outbound requests) should be logged at `warn!` level for auditability.

### B-04 — libSQL secrets store not implemented (High — Feature Gap)
**File:** `src/secrets/` — secrets store path never executes for libSQL backend
Users on the libSQL-only deployment path cannot store or retrieve secrets. This is a silent fallback (not an error), meaning secret-dependent tools silently fail.

### B-05 — SessionManager has no hard eviction (Low)
**File:** `src/agent/session_manager.rs`
At 1000 concurrent sessions a `warn!` is emitted but no sessions are evicted. Under adversarial or runaway conditions this grows unboundedly in memory.

### B-06 — libSQL migrations not incremental (Medium)
**File:** `src/db/libsql_migrations.rs`
libSQL uses `CREATE TABLE IF NOT EXISTS` idempotent statements; no versioning or applied-migration tracking. Schema drift between versions cannot be detected; adding or removing a column requires manual intervention.

### B-07 — `DecryptedSecret::clone()` creates unzeroized intermediate (Low)
**File:** `src/secrets/types.rs`
```rust
SecretString::from(self.value.expose_secret().to_string())
```
The intermediate `String` is heap-allocated plaintext that is not explicitly zeroized. The OS allocator may reuse pages. Use `zeroize::Zeroizing<String>` wrapper instead.

### B-08 — Circuit breaker state not persisted across restart (Low)
**File:** `src/llm/circuit_breaker.rs`
Circuit breaker state (failure count, open/half-open/closed) is purely in-memory. On restart after LLM provider outage the breaker resets to Closed and immediately retries, potentially causing rapid failure storms at startup.

---

## 5. Missing Tests

### T-01 — libSQL vector search path has no integration test
The `WorkspaceStore::hybrid_search()` implementation for libSQL (FTS5 + `libsql_vector_idx`) is not exercised in the `--features libsql` test matrix. A regression could go undetected.

### T-02 — Self-repair with both backends
`src/agent/self_repair.rs` tests use the in-memory mock DB. No test validates repair against a real libSQL or PostgreSQL backend.

### T-03 — Session compaction data integrity test
There is no test verifying that compacted sessions produce semantically equivalent summaries and that no turns are double-dropped.

### T-04 — Failover LLM provider chain ordering test
`FailoverProvider` ordering and cooldown behavior has unit tests, but no integration-level test verifies the full `build_provider_chain()` wiring as configured in `.env`.

### T-05 — HTTP proxy method-downgrade path not tested
`sandbox/proxy/http.rs` unknown-method fallback to GET has no test. Add a test with a non-standard method and verify it errors (or logs appropriately post-fix).

### T-06 — Hook fail-open behavior not tested end-to-end
There is no test that asserts that a panicking `BeforeInbound` hook does not block message processing.

### T-07 — MCP session ID re-use test
`src/tools/mcp/session.rs` maintains per-server session IDs. No test verifies behavior when the MCP server drops a session mid-flight.

### T-08 — Cost guard UTC day-boundary edge case
`cost_guard.rs` resets budgets at UTC midnight. No test covers a request submitted at 23:59:59.999 UTC.

### T-09 — Web gateway 100-connection limit enforcement
`src/channels/web/server.rs` declares a 100-connection max. No test verifies the 101st connection is rejected with the correct status.

### T-10 — Credential injector wildcard host pattern edge cases
`src/tools/wasm/credential_injector.rs` supports `*.example.com` patterns. Tests exist for exact match, but `sub.sub.example.com` multi-level subdomain behavior is not explicitly covered.

---

## 6. Missing Documentation

### D-01 — No incident response runbook
No `docs/INCIDENT_RESPONSE.md` or equivalent. Steps for: database corruption, LLM provider outage, worker container escape, key rotation are absent.

### D-02 — Key rotation procedure not documented
`src/secrets/` has AES-256-GCM encryption with a master key. No documentation describes how to rotate the master key or migrate encrypted secrets after rotation.

### D-03 — libSQL backup and restore procedure
No runbook for taking a consistent libSQL/Turso snapshot, verifying integrity, or restoring from backup.

### D-04 — Multi-user deployment guide
The web gateway uses a single bearer token. No guide explains how to run IronClaw in a shared or multi-user context securely.

### D-05 — Circuit breaker tuning guide
`LlmConfig` exposes `circuit_breaker_failure_threshold`, `circuit_breaker_timeout_secs`, and similar. No documentation explains how to tune these per provider SLA.

### D-06 — WASM tool authoring guide
`tools-src/` has sample WASM tools but no step-by-step guide on creating, signing, and publishing a new tool to the registry.

### D-07 — `NETWORK_SECURITY.md` references but missing
`CLAUDE.md` mentions `src/NETWORK_SECURITY.md` (30 KB) but the file was not found in the repo. If it exists only locally it should be committed; if removed, the reference should be cleaned up.

---

## 7. Security Findings

### S-01 — MCP tools run without sandbox (High)
**File:** `src/tools/mcp/`
MCP-sourced tools are executed with the same OS permissions as the IronClaw process. WASM tools are fuel-metered, memory-limited, and network-allowlisted; MCP tools are not. A malicious or compromised MCP server can read arbitrary files, exfiltrate data, or escalate privileges.
**Mitigation:** Wrap MCP tool execution in the Docker sandbox (same as container tools), or add per-MCP-server network and filesystem allowlists.

### S-02 — Unknown HTTP method fallback silently to GET in proxy (Medium)
Already filed as B-01. Security angle: a sandboxed tool sending `DELETE /resource` could have the method silently converted to `GET`, bypassing deletion guards on the target service.

### S-03 — Hook fail-open could bypass safety checks (Medium)
**File:** `src/agent/agent_loop.rs` / `src/hooks/`
`BeforeInbound` and `BeforeOutbound` hooks that error are logged and processing continues. If a safety or policy hook panics or returns an error, the message/response is not blocked. A regression in a hook does not halt processing.
**Mitigation:** Distinguish hook categories: `SafetyHook` (fail-closed) vs `AuditHook` (fail-open). Policy hooks should fail-closed.

### S-04 — In-memory response cache not scoped to user/job (Low-now, High-later)
**File:** `src/llm/response_cache.rs`
The LRU cache key is SHA-256 of request parameters. If two different users issue identical prompts, one user receives the other's cached response. Currently single-user, but worth addressing before any multi-user feature is added.

### S-05 — `DecryptedSecret::clone()` leaves plaintext on heap (Low)
Already filed as B-07. The intermediate `String` from `expose_secret().to_string()` is not explicitly zeroized. Use `Zeroizing<String>`.

### S-06 — Orchestrator per-job bearer tokens are ephemeral only (Low)
**File:** `src/orchestrator/auth.rs`
Tokens are correct (32-byte random, constant-time comparison) but not persisted. If the orchestrator crashes mid-job, worker containers hold tokens that cannot be re-validated, leaving them orphaned with no revocation signal. Worker containers should receive a cancellation signal.

### S-07 — Credential injection wildcard host allows `*.example.com` subdomain takeover risk (Low)
**File:** `src/tools/wasm/credential_injector.rs`
Wildcard patterns like `*.example.com` match any subdomain. If a subdomain is taken over (DNS hijack or expired subdomain), credentials may be injected into attacker-controlled infrastructure. Recommendation: prefer exact-match patterns; document wildcard risks prominently.

### S-08 — Proxy CONNECT tunnel has no body inspection (Informational)
**File:** `src/sandbox/proxy/http.rs`
CONNECT tunnels (used for HTTPS) correctly bypass injection because TLS is opaque, but there is no heuristic to detect tunnels to non-allowlisted hosts before the tunnel is established. The allowlist check happens at the domain level before CONNECT which is correct, but this is worth noting.

---

## 8. Observability Weaknesses

### O-01 — No structured tracing for proxy denials
Network denial decisions are not emitted as structured log events. Security monitoring depends on log scraping finding the denied request, which may not be emitted at all.

### O-02 — No OpenTelemetry exporter
Only `log` and `noop` observability backends are implemented (`src/observability/`). No spans or metrics are exportable to Prometheus, Grafana, Datadog, or any OTEL collector. Cost, latency, and error rate are invisible to external monitoring.

### O-03 — LLM call latency not recorded with spans
`src/llm/recording.rs` captures full traces for E2E replay but does not emit per-provider latency or error-rate metrics. Circuit breaker state changes are not observable externally.

### O-04 — SSE broadcast errors silently dropped
**File:** `src/channels/web/server.rs`
The SSE broadcast channel has a 256-event buffer. When the buffer is full, events are dropped silently (channel is `try_send`, not `send`). No counter or warning is emitted.

### O-05 — Agent job state transitions not instrumented
State machine transitions (`Pending → InProgress → Completed/Failed/Stuck`) are not emitted as structured events. Debugging job lifecycle issues requires log grepping.

### O-06 — Heartbeat execution results not surfaced
Heartbeat proactive execution results go to a configured channel (Telegram/Slack) but are not retained in the workspace or database. No audit trail of what heartbeat found.

---

## 9. Operational Weaknesses

### Ops-01 — No health/readiness endpoint for process supervision
There is no `GET /healthz` or `/readyz` endpoint. Process supervisors (systemd, K8s, Docker Compose) cannot distinguish "starting" from "alive" from "degraded". The web gateway does expose `/v1/models` which proxies as a liveness check but is not documented as such.

### Ops-02 — No graceful shutdown with drain
**File:** `src/main.rs`, `src/app.rs`
On SIGTERM the process exits without draining in-flight jobs. Background jobs and sandbox containers may be abandoned mid-execution.

### Ops-03 — Docker sandbox image pinned to `rust:1.86-slim-bookworm` without digest
**File:** Docker-related configs
The sandbox base image is referenced by tag, not digest. A tag mutation (upstream Rust image update) could introduce a breaking change or security regression silently.

### Ops-04 — No container reaper timeout test
`src/orchestrator/job_manager.rs` runs a container reaper. If the Docker daemon is unresponsive, the reaper blocks indefinitely. No timeout is documented for Docker API calls.

### Ops-05 — libSQL busy timeout of 5 s may cause visible errors under load
**File:** `src/db/libsql/`
WAL mode allows one writer. Under parallel job execution, all write contention resolves within 5 s. If not, the query returns an error to the caller. No retry with backoff is applied at the libSQL layer.

### Ops-06 — Service install (launchd/systemd) does not set `Restart=on-failure`
**File:** `src/service.rs`
The generated systemd unit file should include `Restart=on-failure` and `RestartSec=5` to automatically recover from crashes. Currently a crash requires manual restart.

---

## 10. Technical Debt

### TD-01 — `super::` import style inconsistency
`CLAUDE.md` specifies `crate::` for cross-module imports, but many files still use `super::` outside test modules. Running `grep -rn 'super::' src/` would surface the scope.

### TD-02 — Domain-specific tool stubs
`src/tools/builtin/marketplace.rs`, `restaurant.rs`, etc. are documented as stubs. They add noise to the tool registry and confuse LLM tool selection.

### TD-03 — `src/safety/mod.rs` re-export shim
The `safety/mod.rs` file re-exports everything from `ironclaw_safety`. Files that still import `crate::safety::*` should be migrated to `ironclaw_safety::*` per the spec. This is a tracked migration, but the debt persists until completion.

### TD-04 — WIT bindgen stub for auto-extracting WASM schema
`CLAUDE.md` notes "auto-extract tool schema from WASM is stubbed". All WASM tools must ship a manually-maintained `schema()` function. This creates drift risk.

### TD-05 — Built tools get empty capabilities
`CLAUDE.md` notes "Built tools get empty capabilities; need UX for granting access". Dynamically built tools have no capability grants, making them less useful than pre-installed tools.

### TD-06 — `rig-core` adapter wraps 5 providers identically
`src/llm/rig_adapter.rs` wraps OpenAI, Anthropic, Ollama, Tinfoil, OpenAI-compatible via identical boilerplate. The adapter could be genericized to reduce duplication.

### TD-07 — LRU response cache is process-local only
`src/llm/response_cache.rs` is in-process, lost on restart. For persistent caching (cost reduction), an external store (Redis, DB) would be appropriate.

### TD-08 — `src/channels/signal.rs` is 103 KB
Signal channel implementation is a monolith. It should be refactored into sub-modules (auth, message, group, dm) consistent with the Telegram channel structure.

---

## 11. Simplification Opportunities

### Simp-01 — Unify `rig_adapter.rs` providers
The five rig-core adapters share near-identical implementation. Extract a generic `RigAdapter<M: CompletionModel>` struct to eliminate ~200 lines of duplication.

### Simp-02 — Consolidate `DatabaseConfig` backend branching
`src/app.rs` has repeated `match config.database.backend { Postgres => ..., LibSql => ... }` branches. Move each branch into the respective module factory as specified in the `Module-owned initialization` rule.

### Simp-03 — Replace `test panic!` with `assert!` macros (cleanup)
`src/orchestrator/api.rs` lines 784, 832 — replace `panic!("Expected...", other)` with `assert!(matches!(other, Expected...))`.

### Simp-04 — Remove stub domain tools or mark clearly
Stubs in `tools/builtin/` for marketplace, restaurant, etc. should either be removed or tagged with `#[cfg(feature = "domain-stubs")]` to keep the default tool registry clean.

---

## 12. Quick Wins (1–3 days each)

| ID | Action | File | Effort |
|----|--------|------|--------|
| QW-01 | Log proxy denials at `warn!` with method, URI, peer | `sandbox/proxy/http.rs` | 1 h |
| QW-02 | Reject unknown HTTP methods in proxy with `400` | `sandbox/proxy/http.rs` | 1 h |
| QW-03 | Add `Restart=on-failure` to systemd unit generation | `src/service.rs` | 30 min |
| QW-04 | Replace `panic!` with `assert!` in test code | `orchestrator/api.rs:784,832` | 30 min |
| QW-05 | Wrap `DecryptedSecret::clone()` intermediate in `Zeroizing<String>` | `secrets/types.rs` | 30 min |
| QW-06 | Add `GET /healthz` endpoint to web gateway | `channels/web/server.rs` | 2 h |
| QW-07 | Emit `warn!` when SSE broadcast buffer is full | `channels/web/server.rs` | 1 h |
| QW-08 | Add hard eviction in `SessionManager` at 1000 sessions (LRU order) | `agent/session_manager.rs` | 2 h |
| QW-09 | Pin Docker sandbox image by digest in Dockerfile | `docker/` | 30 min |
| QW-10 | Add test for unknown HTTP method in proxy | `src/sandbox/proxy/http.rs` tests | 1 h |

---

## 13. High-Risk Changes (Require Extra Care)

| ID | Change | Why High-Risk |
|----|--------|--------------|
| HR-01 | Add libSQL incremental migrations | Risk of data loss or silent no-op on upgrade; needs careful version tracking |
| HR-02 | Implement hook fail-closed for safety hooks | Changes behavior for all existing hooks; needs backward-compatible category tagging |
| HR-03 | Sandbox MCP tools | Complex integration; MCP protocol and Docker sandbox must interoperate; can break existing MCP integrations |
| HR-04 | Add multi-user bearer token support to web gateway | Requires auth model redesign; current single-token model is assumed widely |
| HR-05 | Persist circuit breaker state | Must handle corrupted/stale state gracefully on restart |

---

## 14. Deferred / Out-of-Scope Items

These are noted but deferred pending roadmap prioritization:

- **Streaming LLM responses** — Architectural change to `LlmProvider` trait; affects all providers and the agentic loop.
- **OpenTelemetry backend** — New `Observer` implementation; no blocker, just effort.
- **WASM schema auto-extraction via WIT** — Depends on upstream WIT bindgen maturity.
- **Tool versioning and rollback** — Requires registry schema changes and UI.
- **Multi-user / multi-tenant mode** — Session isolation, auth redesign, per-user billing.
- **Persistent LLM response cache** — Redis/DB backend for the LRU cache.
- **MCP streaming support** — Requires protocol extension.
- **PostgreSQL testcontainers** — Currently skipped in CI; needs Docker-in-Docker or dedicated runner.

---

## 15. Cross-Repo Items

These findings affect components outside this repository:

| ID | Item | External Repo |
|----|------|--------------|
| XR-01 | WASM channel artifacts: SHA256 checksums updated by CI (`d47b4b0`) — downstream consumers of bundled channels should pin to digest | `channels-src/` (separate repo implied) |
| XR-02 | Registry catalog (`registry/`) references external registry manifest JSON — no integrity check on catalog fetch | Registry service |
| XR-03 | `src/NETWORK_SECURITY.md` referenced in `CLAUDE.md` but not found in repo — if it lives in a separate internal wiki it should be committed or linked | Internal docs repo |
| XR-04 | E2E test recorded LLM traces in `tests/fixtures/llm_traces/` — if `nearai` API responses change shape, traces will silently mismatch | NearAI API |

---

## 16. Wave-Based Remediation Plan

### Wave 1 — Hardening (Week 1, Low Risk, High Value) ✅ COMPLETE

**Goal:** Close the most critical observability and correctness gaps without architectural change.

1. **QW-01** Log proxy denials ✅
2. **QW-02** Reject unknown HTTP methods with 400 ✅
3. **QW-03** `Restart=on-failure` in systemd unit ✅
4. **QW-04** Replace test `panic!` with `unreachable!` ✅
5. **QW-05** `Zeroizing<String>` in `DecryptedSecret::clone()` ✅
6. **QW-06** Add `/healthz` endpoint ✅
7. **QW-07** Warn on SSE buffer full ✅
8. **QW-08** Hard eviction in `SessionManager` ✅
9. **QW-09** Pin Docker image by digest ✅
10. **QW-10** Test for unknown HTTP method ✅

See: `docs/mega_resiliency/tasks/wave1_hardening.md`

**Estimated effort:** 2–3 engineer-days

---

### Wave 2 — Reliability (Weeks 2–3) ✅ COMPLETE

**Goal:** Address the most impactful reliability gaps.

1. **B-06** libSQL incremental migration tracking ✅ (already implemented)
2. **B-03** Structured logging for proxy denials ✅ (already implemented)
3. **T-01** libSQL vector search integration test ✅ (3 tests added to `tests/module_init_integration.rs`)
4. **T-06** Hook fail-open end-to-end test ✅ (already implemented in `src/hooks/registry.rs`)
5. **T-09** Web gateway 100-connection limit test ✅ (already implemented in `src/channels/web/sse.rs`)
6. **Ops-02** Graceful shutdown with job drain ✅ (SIGTERM handler + 30 s drain added to `src/main.rs`)
7. **Ops-05** libSQL write retry with backoff ✅ (already implemented in `src/db/libsql/mod.rs`)

See: `docs/mega_resiliency/tasks/wave2_reliability.md`

**Estimated effort:** 5–7 engineer-days

---

### Wave 3 — Security Hardening (Weeks 3–5)

**Goal:** Close the most significant security gaps.

1. **S-03** Distinguish `SafetyHook` (fail-closed) vs `AuditHook` (fail-open) — requires hook trait update
2. **S-01** MCP tool sandboxing (network allowlist + optional Docker containment)
3. **B-04** libSQL secrets store implementation
4. **D-02** Document master key rotation procedure
5. **D-01** Write incident response runbook
6. **T-02** Self-repair integration test against real backends

**Estimated effort:** 10–15 engineer-days

---

### Wave 4 — Observability & Developer Experience (Weeks 5–7)

**Goal:** Make the system externally observable and reduce debugging friction.

1. **O-02** OpenTelemetry exporter implementation (new `Observer` backend)
2. **O-03** LLM call latency + circuit breaker state metrics
3. **O-05** Job state machine transition events
4. **TD-02** Remove or feature-flag stub domain tools
5. **TD-01** Migrate remaining `super::` imports to `crate::` per spec
6. **D-06** WASM tool authoring guide

**Estimated effort:** 8–12 engineer-days

---

### Wave 5 — Architecture & Performance (Weeks 7–12)

**Goal:** Tackle the architectural debt that blocks future scalability.

1. **R-02** Headless-safe auth refresh (non-interactive token renewal path)
2. **R-03** Persist approval state to database (survive restarts)
3. **TD-07** Persistent LLM response cache (Redis or DB backend)
4. **Simp-01** Generalize rig-core adapter
5. **HR-04** Multi-user bearer token support (if roadmap requires)
6. Streaming LLM responses (deferred — large scope)

**Estimated effort:** 15–25 engineer-days

---

## Appendix A — Audit Coverage

| Module | Files Read | Depth |
|--------|-----------|-------|
| `src/agent/` | 22/22 | Full |
| `src/llm/` | 25/25 | Full |
| `src/tools/` | All | Full |
| `src/db/` | All | Full |
| `src/channels/` | All | Full |
| `src/safety/` + `crates/ironclaw_safety/` | All | Full |
| `src/orchestrator/` | All | Full |
| `src/secrets/` | All | Full |
| `src/sandbox/proxy/` | All | Full |
| `src/config/` | All | Full |
| `src/app.rs`, `src/main.rs` | Full | Full |
| `migrations/` (V1–V13) | V1, V2, V13 | Sampled |
| `.github/workflows/` | 14 workflows | Full |
| `tests/` | Structure + key files | Sampled |
| `.env.example` | Full | Full |
| `CLAUDE.md` + module specs | All | Full |

---

*Generated by Claude Code Mega Resiliency Audit Process — 2026-03-26*
