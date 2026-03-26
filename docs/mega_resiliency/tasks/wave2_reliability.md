# Wave 2 — Reliability: COMPLETED

**Date:** 2026-03-26
**Status:** All actionable items resolved. Items already implemented (B-06, B-03, T-06, T-09, Ops-05) verified in-place. Two items implemented this wave (T-01, Ops-02).

## Items

| ID | Description | File | Status |
|----|-------------|------|--------|
| B-06 | libSQL incremental migration tracking | `src/db/libsql_migrations.rs` — `_migrations` table + `run_incremental()` | ✅ Already done |
| B-03 | Structured logging for proxy denials | `src/sandbox/proxy/http.rs:229-236,279-283` — method/uri/reason fields | ✅ Already done |
| T-01 | libSQL vector search integration test | `tests/module_init_integration.rs` — 3 new tests added | ✅ Implemented this wave |
| T-06 | Hook fail-open end-to-end test | `src/hooks/registry.rs` — fail-open/closed tests present | ✅ Already done |
| T-09 | Web gateway 100-connection limit test | `src/channels/web/sse.rs:306-317` | ✅ Already done |
| Ops-02 | Graceful shutdown with job drain | `src/main.rs` — SIGTERM handler + 30 s drain | ✅ Implemented this wave |
| Ops-05 | libSQL write retry with backoff | `src/db/libsql/mod.rs:128-150` — 3-retry + `busy_timeout=5000` | ✅ Already done |

## Changes Made

### T-01 — libSQL workspace search integration tests
**File:** `tests/module_init_integration.rs`

Added three `#[cfg(feature = "libsql")]` integration tests:
1. `libsql_workspace_fts_search_returns_results` — inserts a chunk, verifies FTS query returns it
2. `libsql_workspace_fts_search_empty_on_no_match` — verifies no-match case does not error
3. `libsql_workspace_hybrid_search_with_embedding_falls_back_to_fts` — passes a 1536-dim embedding alongside an FTS query, verifies the code path does not panic when the `libsql_vector_idx` index is absent (V9 migration removes it) and that FTS fallback still returns results

All three use `LibSqlBackend::new_memory()` — no temp files, no cleanup required.

### Ops-02 — Graceful shutdown with job drain
**File:** `src/main.rs`

Two changes:
1. **SIGTERM handler** (Unix only): wrapped `agent.run()` in `tokio::select!` that races against `SIGTERM`. When a process supervisor sends SIGTERM, the agent exits cleanly instead of being hard-killed.
2. **Scheduler drain**: captured `scheduler_for_drain = agent.scheduler()` before the agent loop, then added a 30-second `tokio::time::timeout(scheduler_for_drain.stop_all())` in the shutdown sequence. Sends Stop signals to all running jobs and aborts background subtasks, giving them a chance to persist state.
