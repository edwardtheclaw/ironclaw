# Wave 1 — Hardening: COMPLETED

**Status:** All 10 quick wins verified as already implemented in the codebase at the time of Wave 2 execution (2026-03-26).

## Items

| ID | Description | File | Status |
|----|-------------|------|--------|
| QW-01 | Log proxy denials at `warn!` with method, URI, peer | `src/sandbox/proxy/http.rs:229-236` | ✅ Done |
| QW-02 | Reject unknown HTTP methods in proxy with `400` | `src/sandbox/proxy/http.rs:347-361` | ✅ Done |
| QW-03 | Add `Restart=on-failure` to systemd unit | `src/service.rs:133-135` (`Restart=always`) | ✅ Done |
| QW-04 | Replace `panic!` with `unreachable!` in test code | `src/orchestrator/api.rs:784,832` | ✅ Done |
| QW-05 | Wrap `DecryptedSecret::clone()` intermediate in `Zeroizing<String>` | `src/secrets/types.rs:128-134` | ✅ Done |
| QW-06 | Add `GET /healthz` endpoint to web gateway | `src/channels/web/server.rs:230-233` | ✅ Done |
| QW-07 | Emit `warn!` when SSE broadcast buffer is full | `src/channels/web/sse.rs:70-82` | ✅ Done |
| QW-08 | Add hard eviction in `SessionManager` at 1000 sessions (LRU) | `src/agent/session_manager.rs:78-100` | ✅ Done |
| QW-09 | Pin Docker sandbox image by digest | `Dockerfile`, `Dockerfile.test`, `Dockerfile.worker` | ✅ Done |
| QW-10 | Add test for unknown HTTP method in proxy | `src/sandbox/proxy/http.rs:590-606` | ✅ Done |

## Notes

All Wave 1 items were already implemented before the wave execution began. The changes to `Cargo.toml`, `Dockerfile*`, `src/agent/session_manager.rs`, `src/channels/web/server.rs`, `src/channels/web/sse.rs`, `src/orchestrator/api.rs`, `src/sandbox/proxy/http.rs`, and `src/secrets/types.rs` visible in `git status` correspond to these implementations.
