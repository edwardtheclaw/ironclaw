# IronClaw — Mega Product Extension Audit

**Date**: 2026-03-26
**Auditor**: Claude Sonnet 4.6 (automated read-only analysis)
**Version audited**: 0.21.0 (commit 709ca18)

---

## 1. Repo Summary and Current Purpose

**IronClaw** is a secure, local-first personal AI assistant written in Rust. It runs as a personal server (CLI daemon, systemd/launchd service, or Docker container) and routes user interactions through a unified agentic loop powered by a pluggable LLM backend.

**Core pillars:**

| Pillar | Implementation |
|--------|---------------|
| **Privacy-first** | All data stored locally (libSQL or PostgreSQL); secrets AES-256-GCM encrypted |
| **Self-expanding** | WASM-sandboxed tools, MCP protocol, dynamic tool building |
| **Multi-channel** | CLI/TUI, web browser, HTTP webhook, Signal, Telegram/Slack via WASM |
| **Defense-in-depth** | Prompt injection defense, network proxying, capability-based tool access |
| **Autonomous** | Background jobs, cron routines, heartbeat system, self-repair |

**Who it serves today:**

- **Power users** running a local AI agent for automation, research, and productivity
- **Developers** building custom skills and WASM tools
- **Privacy-conscious users** who refuse cloud-processed memory
- **Operators** deploying IronClaw as a personal server behind a tunnel (Cloudflare, ngrok, Tailscale)

---

## 2. Current Workflow Map

```
┌─────────────────────────────────────────────────────────┐
│                      INPUT CHANNELS                      │
│  CLI/TUI  │  Web Browser  │  HTTP Webhook  │  WASM Channels │
└─────────────────────────────────────────────────────────┘
                            │
                    ChannelManager
                  (merges streams via tokio)
                            │
                    ┌───────▼───────┐
                    │  AgentLoop    │
                    │  (router)     │
                    └───────┬───────┘
                  ┌─────────┴─────────┐
             Chat Turn           Background Job
          (Dispatcher +          (JobDelegate +
           SkillInjection)        Planning phase)
                  │                    │
          ┌───────▼────────────────────▼───────┐
          │     Shared Agentic Loop Engine      │
          │  LLM → Tool Calls → Iterate         │
          └───────┬────────────────────┬───────┘
         Built-in Tools         WASM / MCP Tools
         (file, shell,          (sandboxed,
          memory, http…)         third-party)
                  │
          ┌───────▼───────┐
          │  Workspace    │  ←── Hybrid FTS + vector memory
          │  (libSQL/PG)  │
          └───────────────┘
                  │
          Response via Channel (SSE / WebSocket / TUI)
```

**Hooks intercept at 6 points:** BeforeInbound → BeforeToolCall → BeforeOutbound → TransformResponse → OnSessionStart → OnSessionEnd

**Skills inject domain-specific prompt context** deterministically before each LLM turn, gated by keyword/pattern scoring with trust-based tool attenuation.

---

## 3. User / Operator Value Today

### Delivered Value

- **Persistent memory across conversations** — hybrid search (FTS + vector) via Reciprocal Rank Fusion; four memory tools available to the agent
- **Autonomous background jobs** — parallel job scheduling with Docker sandbox for untrusted code, isolated context per job
- **Proactive heartbeat** — reads HEARTBEAT.md checklist every 30 minutes; notifies user if findings
- **Scheduled routines** — cron and event-triggered background tasks, no external cron needed
- **7 LLM backends** — NEAR AI, OpenAI, Anthropic, Ollama, OpenAI-compatible, Tinfoil, AWS Bedrock; resilience via circuit breaker + failover
- **Multi-channel** — interact from CLI, browser, Signal, Slack, Telegram (WASM), HTTP webhook
- **WASM extensions** — capability-gated third-party tools with fuel metering, memory limits, and network allowlisting
- **MCP support** — connect to any Model Context Protocol server (HTTP, stdio, Unix)
- **Skills system** — SKILL.md prompt extensions with ClawHub registry; deterministic selection without extra LLM calls
- **Security stack** — prompt injection defense, secrets keychain, network proxy with domain allowlist, approval gates

### Power-User Differentiators

- Undo/redo with checkpoints across conversation turns
- Self-repair: stuck job detection and automatic recovery
- Dynamic tool building (WASM from source, validated on build)
- Context compaction strategies (MoveToWorkspace, Summarize, Truncate)
- Cost guard with per-job spend limits
- OpenAI-compatible proxy endpoint (`/v1/chat/completions`)

---

## 4. Obvious Product Gaps

These are gaps explicitly acknowledged in code comments, CLAUDE.md, or clearly visible from the feature surface.

| # | Gap | Evidence |
|---|-----|----------|
| 1 | **No OpenTelemetry / Prometheus backend** | CLAUDE.md "only log and noop backends" |
| 2 | **No tool versioning or rollback** | CLAUDE.md limitation #6 |
| 3 | **Domain tools are stubs** | marketplace.rs, restaurant.rs — no real impl |
| 4 | **MCP has no streaming** | CLAUDE.md limitation #3 |
| 5 | **WIT bindgen auto-schema stubbed** | CLAUDE.md limitation #4 |
| 6 | **No capability UX for built tools** | CLAUDE.md "empty capabilities; need UX" |
| 7 | **libSQL secrets store not wired** | Code exists in `LibSqlSecretsStore`, not plumbed |
| 8 | **libSQL settings reload skipped** | `Config::from_db` skipped for libSQL |
| 9 | **No conversation export** | No export endpoint or file output |
| 10 | **No web UI conversation search** | Thread list exists; no keyword search over history |
| 11 | **No visual routine/job editor** | API exists; web UI has no CRUD forms |
| 12 | **No push notifications** | SSE-only; no web push, no email digest |
| 13 | **No webhook delivery audit log** | Inbound HMAC failures are logged, not queryable |
| 14 | **Single-user web gateway** | No multi-user auth, no per-user isolation |
| 15 | **No memory hygiene config in UI** | Settings exist in env; not exposed in web UI |

---

## 5. Adjacent Feature Opportunities

Ranked by **value × safety** (value = user impact; safety = implementation confidence, no regressions, no speculative scope).

### Tier 1 — Highest Confidence, Lowest Regret

These extend existing traits/infrastructure with minimal new surface area.

#### OBS-01: OpenTelemetry Observer Backend
- **What**: Implement `Observer` trait with OpenTelemetry OTLP export. The `multi` backend fans out to both `log` and OTLP simultaneously.
- **Why it fits**: Trait exists, factory exists, two backends already exist as templates. No new interfaces needed.
- **Value**: Unlocks integration with Grafana, Jaeger, Datadog, Honeycomb for operators running IronClaw in production.
- **Files**: `src/observability/`, new `src/observability/opentelemetry.rs`, `Cargo.toml` (add `opentelemetry`, `opentelemetry-otlp`)
- **Effort**: ~2 days

#### OBS-02: Prometheus `/metrics` Endpoint
- **What**: Add a `/metrics` HTTP route to the webhook server that exports Prometheus text format. Implement a `PrometheusObserver` that accumulates counters/gauges behind `Arc<Mutex<>>`.
- **Why it fits**: `webhook_server.rs` already composes routes; `Observer` trait provides the hook points; no new architectural patterns.
- **Value**: Standard scrape target for Prometheus/VictoriaMetrics. Operators get dashboards with zero instrumentation code.
- **Files**: `src/observability/prometheus.rs`, `src/channels/webhook_server.rs`
- **Effort**: ~2 days

#### DB-01: Wire libSQL Secrets Store
- **What**: Plumb `LibSqlSecretsStore` into `app.rs` startup; add migration for secrets table in `libsql_migrations.rs`; add integration test.
- **Why it fits**: Implementation already exists; it's a wiring task, not a new design.
- **Value**: libSQL users gain parity with PostgreSQL for secret storage; currently secrets only work properly with PG.
- **Files**: `src/app.rs`, `src/db/libsql/`, `src/secrets/`
- **Effort**: ~1 day

#### DB-02: libSQL Settings Reload
- **What**: Implement `Config::from_db` for libSQL (currently a no-op/skip) so live settings changes via `memory_write` or the settings API take effect without restart.
- **Why it fits**: Config struct and DB settings trait exist for PostgreSQL; this is implementing the same path for libSQL.
- **Value**: Settings like LLM model, heartbeat interval, hygiene cadence can be changed at runtime without restarting the daemon.
- **Files**: `src/config/mod.rs`, `src/db/libsql/settings.rs`
- **Effort**: ~1 day

#### EXPORT-01: Conversation Export (JSON + Markdown)
- **What**: Add `/api/chat/threads/{id}/export?format=json|markdown` endpoint. JSON includes full turn metadata; Markdown produces a readable transcript.
- **Why it fits**: Thread model is fully stored in DB; `ConversationStore` already exposes `get_thread()` and `get_turns()`; web channel already has 150+ API endpoints.
- **Value**: Users can archive conversations, share transcripts, import into other tools. High request frequency for personal AI tools.
- **Files**: `src/channels/web/handlers/chat.rs`, new serialization helpers
- **Effort**: ~1 day

#### SEARCH-01: Web UI Thread/History Search
- **What**: Add `/api/chat/search?q=...` endpoint that queries conversation turn text via FTS (the workspace already has FTS; add a thread-scoped FTS query). Expose in the web UI thread list.
- **Why it fits**: FTS infrastructure exists; `ConversationStore` has the turn data; it's a new query, not a new pattern.
- **Value**: Users often need to find "that conversation where I discussed X". Currently impossible without manual memory_write.
- **Files**: `src/db/libsql/conversations.rs`, `src/db/postgres/history/`, `src/channels/web/handlers/chat.rs`
- **Effort**: ~2 days

#### HOOKS-01: Webhook Delivery Audit Log
- **What**: Persist inbound webhook events (timestamp, channel, HMAC result, payload hash) to a `webhook_events` table. Add `/api/webhooks/events` endpoint. Surface in web UI.
- **Why it fits**: `http.rs` already validates HMAC; adding a `BeforeInbound` hook that writes to DB is the natural pattern.
- **Value**: Operators debugging integrations can see which webhooks arrived, which failed validation, and replay attempts.
- **Files**: `src/channels/http.rs`, `src/hooks/`, `src/db/libsql/`, new handler
- **Effort**: ~2 days

---

### Tier 2 — Medium Scope, High Value

#### MCP-01: MCP Streaming Support
- **What**: Extend `McpTransport` trait with `call_tool_stream()` returning a `Stream<Item = ToolChunk>`. Implement for HTTP (chunked transfer / SSE). Route streaming output to the SSE event bus as `tool_result_chunk` events.
- **Why it fits**: Protocol trait exists; streaming exists elsewhere (LLM responses already stream); SSE infrastructure is fully operational.
- **Value**: MCP servers that do long-running work (web scraping, code generation) can stream partial results. Required for MCP parity with direct LLM streaming.
- **Files**: `src/tools/mcp/client.rs`, `src/tools/mcp/http_transport.rs`, `src/channels/web/types.rs`
- **Effort**: ~1 week

#### WASM-01: WIT Bindgen Auto-Schema Extraction
- **What**: Parse the `.wit` file bundled in a WASM artifact to auto-generate the tool's JSON schema (parameter names, types, descriptions) instead of requiring manual schema in the module.
- **Why it fits**: WASM loader (`src/tools/wasm/loader.rs`) already reads module metadata; the builder system has scaffolding for WIT; `wit-parser` crate is the natural dependency.
- **Value**: Dramatically reduces friction for WASM tool authors. Currently every tool needs a manually-maintained schema that can drift from the WIT definition.
- **Files**: `src/tools/wasm/loader.rs`, `src/tools/builder/`, `Cargo.toml`
- **Effort**: ~3 days

#### TOOLS-01: Tool Capability Grant UI
- **What**: Add web UI for viewing and modifying the capability set of a built WASM tool (network allowlist, filesystem access, rate limit overrides). Persist to `tool_capabilities` DB table. Surface in `/api/extensions/{name}/capabilities`.
- **Why it fits**: The allowlist and limits infrastructure exists in `src/tools/wasm/allowlist.rs` and `limits.rs`; the extensions API handler already exists.
- **Value**: Removes the only remaining blocker for using dynamically-built tools in production — operators currently have no way to grant network access to their custom tools.
- **Files**: `src/tools/wasm/allowlist.rs`, `src/channels/web/handlers/extensions.rs`, `src/db/libsql/`
- **Effort**: ~4 days

#### ROUTINE-01: Routine Visual Editor (Web UI)
- **What**: Add CRUD forms in the web UI for creating/editing routines — cron expression builder, trigger condition editor, action type picker (job vs message vs skill invocation).
- **Why it fits**: Routine CRUD API already exists; `src/channels/web/handlers/routines.rs` handles it; this is purely a frontend addition.
- **Value**: Currently, routines must be created via API calls or direct DB manipulation. A visual editor makes the heartbeat/automation features accessible to non-developer users.
- **Files**: `src/channels/web/static/` (JS/HTML), `src/channels/web/handlers/routines.rs`
- **Effort**: ~3 days

#### SKILL-01: Skill Authoring UI
- **What**: Web UI editor for SKILL.md files — YAML frontmatter form (name, keywords, patterns, tags, required tools) + Markdown body editor with live preview of activation score.
- **Why it fits**: Skills are already stored in filesystem; `src/channels/web/handlers/skills.rs` exposes list/install/remove; this adds read/write of trusted skill files.
- **Value**: Makes skills accessible to users who don't want to hand-edit YAML in `~/.ironclaw/skills/`. Skills are one of IronClaw's strongest differentiators; friction here loses users.
- **Files**: `src/channels/web/handlers/skills.rs`, `src/channels/web/static/`, `src/skills/parser.rs`
- **Effort**: ~3 days

#### NOTIFY-01: Web Push Notifications
- **What**: Implement Web Push Protocol (RFC 8030) for browser notifications. When heartbeat finds issues, a job completes, or an approval is needed, push a notification even when the browser tab is closed. Add `/api/push/subscribe` endpoint.
- **Why it fits**: SSE infrastructure already delivers these events; Web Push is a browser standard with a straightforward `vapid` + `web-push` crate path. Approval events already exist as SSE events.
- **Value**: Makes IronClaw useful as a server-side agent — users don't need the browser tab open to be notified of job completions or required approvals.
- **Files**: New `src/channels/web/push.rs`, `src/channels/web/server.rs`, `src/channels/web/static/`
- **Effort**: ~3 days

#### MEM-01: Memory Hygiene Config in Web UI
- **What**: Expose memory hygiene settings (retention windows by document type, cadence) in the `/api/settings` path and add a web UI settings panel for them. Currently env-var-only.
- **Why it fits**: `MemoryHygieneConfig` parses from env; the settings persistence layer exists; this adds a settings → hygiene config binding.
- **Value**: Users can tune memory expiry without restarting the daemon. Crucial for long-running server deployments.
- **Files**: `src/config/hygiene.rs`, `src/settings.rs`, `src/channels/web/handlers/settings.rs`
- **Effort**: ~1 day

#### TOOLS-02: Tool Versioning with Rollback
- **What**: Add `version` field to tool manifests and WASM artifacts. Store previous versions in a `tool_versions` table. Add `tool rollback <name> <version>` CLI and `/api/extensions/{name}/rollback` API.
- **Why it fits**: Extension manifest types already exist in `registry/manifest.rs`; installer already downloads and verifies artifacts; adding version history is an additive DB + API change.
- **Value**: Safe tool updates — if a new WASM tool version breaks a workflow, the user can roll back in one command.
- **Files**: `src/registry/manifest.rs`, `src/registry/installer.rs`, `src/db/libsql/`, `src/cli/registry.rs`
- **Effort**: ~4 days

---

### Tier 3 — Deferred (Confidence < 80% or Scope > 2 weeks)

| ID | Idea | Reason for Deferral |
|----|------|---------------------|
| MULTIUSER-01 | Multi-user web gateway | Requires rethinking session isolation, auth model, and per-user DB namespacing — architectural, not additive |
| VOICE-01 | Voice channel (transcription → agent) | Transcription module exists but voice capture, VAD, and a real-time audio pipeline are large new surface |
| EMAIL-01 | Inbound email channel (IMAP) | New protocol, new credential surface, complex threading model; not adjacent to existing channels |
| BROWSER-01 | Browser extension channel | Separate distribution artifact, cross-origin messaging, significant frontend work |
| MARKETPLACE-01 | Domain tools (restaurant, marketplace) | Stubs by design; their shape depends on third-party API decisions not yet made |
| FEDERATED-01 | Federated memory across instances | Protocol design work; no clear user demand yet |
| MULTIMODAL-01 | Image/audio generation tools | Image tools partially exist (`image_*.rs`); needs model provider expansion before tool layer |
| SAAS-01 | Multi-tenant hosted deployment | Not local-first; contradicts core design philosophy without explicit user demand |

---

## 6. Rejected Ideas

Ideas considered but **explicitly rejected** for this audit:

| Idea | Rejection Reason |
|------|-----------------|
| "AI-powered skill recommender" | Would add an LLM call on every turn to suggest skills — destroys the deterministic, zero-latency selection pipeline that is a deliberate design choice |
| "Replace libSQL with SQLite" | libSQL is the Turso-compatible superset; replacing it removes cloud sync capabilities without benefit |
| "Add GraphQL API" | The REST API surface is 150+ endpoints and well-typed; GraphQL would double maintenance burden for no new capability |
| "Plug in a new LLM provider (Gemini)" | This is routine implementation work, not a product extension; obvious from existing provider templates |
| "Port TUI to web UI" | TUI and web are separate channels by design; merging them would reduce both |
| "Add analytics/telemetry collection" | Contradicts privacy-first philosophy; IronClaw explicitly stores data locally |

---

## 7. Quick Wins (Ship in < 1 week each)

| ID | Feature | Files Touched | Effort |
|----|---------|---------------|--------|
| DB-01 | Wire libSQL secrets store | `app.rs`, `db/libsql/`, `secrets/` | 1 day |
| DB-02 | libSQL settings reload | `config/mod.rs`, `db/libsql/settings.rs` | 1 day |
| EXPORT-01 | Conversation export (JSON + Markdown) | `web/handlers/chat.rs` | 1 day |
| MEM-01 | Memory hygiene config in web UI | `config/hygiene.rs`, `settings.rs`, `web/handlers/settings.rs` | 1 day |
| SEARCH-01 | Web UI thread/history search | `db/libsql/conversations.rs`, `web/handlers/chat.rs` | 2 days |
| OBS-02 | Prometheus `/metrics` endpoint | `observability/prometheus.rs`, `webhook_server.rs` | 2 days |
| HOOKS-01 | Webhook delivery audit log | `channels/http.rs`, `hooks/`, `db/libsql/` | 2 days |

---

## 8. Medium-Scope Extensions (1–2 weeks each)

| ID | Feature | Effort |
|----|---------|--------|
| OBS-01 | OpenTelemetry OTLP Observer backend | ~2 days |
| WASM-01 | WIT bindgen auto-schema extraction | ~3 days |
| ROUTINE-01 | Routine visual editor in web UI | ~3 days |
| SKILL-01 | Skill authoring UI | ~3 days |
| NOTIFY-01 | Web Push notifications | ~3 days |
| TOOLS-01 | Tool capability grant UI | ~4 days |
| TOOLS-02 | Tool versioning with rollback | ~4 days |
| MCP-01 | MCP streaming support | ~1 week |

---

## 9. Cross-Repo Ideas (Document Only — Do Not Execute Here)

These require work outside this repository or have dependencies on external systems.

| Idea | Why Cross-Repo | Notes |
|------|---------------|-------|
| **ClawHub registry improvements** | Registry catalog (`registry/manifest.rs`) consumes from an external registry service; skill/extension publishing changes require server-side work | IronClaw is the client; the registry server is separate |
| **WASM SDK / template generator** | Tool authors need a Rust/AssemblyScript SDK; this is a separate publishable crate (`ironclaw-tool-sdk`) | Would accelerate WASM-01 (WIT bindgen) adoption |
| **claude-code companion mode** | `src/worker/claude_bridge.rs` spawns `claude` CLI; deeper integration (shared context, tool passthrough) requires changes to the Claude Code product | Cross-product dependency |
| **IronClaw mobile companion app** | Native push notification receiver, voice input, Signal-like UI; WASM channel provides protocol but mobile app is a separate artifact | Cross-platform build system, App Store distribution |
| **Shared skills catalog (ClawHub)** | Skills are local; a community catalog requires a backend service with auth, versioning, and review pipeline | Server-side infrastructure |
| **Observability SaaS integration** | Shipping OBS-01 (OTLP) enables integration; but providing pre-built dashboards (Grafana, Datadog) requires work in those systems | Partner/ecosystem work |

---

## 10. Wave-Based Execution Plan

### Wave 1 — Foundation & Unblocking (Highest Confidence, Lowest Regret)

**Theme**: Complete partially-implemented features, fix known gaps, add zero-risk infrastructure.

All Wave 1 items extend existing patterns without introducing new abstractions.

| Priority | ID | Feature | Value Signal | Risk |
|----------|-----|---------|-------------|------|
| 1 | DB-01 | Wire libSQL secrets store | libSQL users blocked on secret storage | Very low — plumbing only |
| 2 | DB-02 | libSQL settings reload | Restartless config for libSQL users | Low — existing PG path is reference |
| 3 | EXPORT-01 | Conversation export | Requested by every personal AI tool user | Very low — serialization only |
| 4 | SEARCH-01 | Web UI thread search | Discoverability of past work | Low — FTS already used in workspace |
| 5 | MEM-01 | Memory hygiene config in UI | Makes automation features tunable | Very low — settings passthrough |
| 6 | OBS-02 | Prometheus /metrics | Standard DevOps expectation | Low — new route + counter map |
| 7 | HOOKS-01 | Webhook delivery audit log | Debuggability for integrations | Low — BeforeInbound hook |

**Wave 1 Definition of Done**: All 7 items shipped, tested (unit + integration), documented in relevant CLAUDE.md files.

---

### Wave 2 — Surface & UX (Medium Scope)

**Theme**: Make powerful features accessible to non-developer users through the web UI.

| Priority | ID | Feature | Dependency |
|----------|-----|---------|-----------|
| 1 | ROUTINE-01 | Routine visual editor | None |
| 2 | SKILL-01 | Skill authoring UI | None |
| 3 | TOOLS-01 | Tool capability grant UI | None |
| 4 | NOTIFY-01 | Web Push notifications | None |
| 5 | WASM-01 | WIT bindgen auto-schema | None |

---

### Wave 3 — Protocol & Integration (Larger Surface)

**Theme**: Expand the integration surface so IronClaw connects to more of the ecosystem.

| Priority | ID | Feature | Dependency |
|----------|-----|---------|-----------|
| 1 | MCP-01 | MCP streaming | None (protocol work) |
| 2 | OBS-01 | OpenTelemetry OTLP | Wave 1 OBS-02 pattern |
| 3 | TOOLS-02 | Tool versioning + rollback | None |

---

### Wave 4 — Deferred (Revisit After Wave 3)

Re-evaluate MULTIUSER-01, VOICE-01, EMAIL-01 after Wave 3. Assess whether user demand and team capacity justify the architectural investment.

---

## Appendix: Current Limitation Reference

From `CLAUDE.md` "Current Limitations" section, mapped to audit items:

| Limitation | Audit Item |
|-----------|-----------|
| Domain-specific tools are stubs | Deferred — shape depends on third-party API decisions |
| Integration tests need testcontainers | Not a product feature; keep as engineering debt |
| MCP: no streaming support | MCP-01 |
| WIT bindgen: auto-extract tool schema is stubbed | WASM-01 |
| Built tools get empty capabilities; need UX | TOOLS-01 |
| No tool versioning or rollback | TOOLS-02 |
| Observability: only log and noop backends | OBS-01, OBS-02 |

---

*Audit generated by automated read-only analysis. All opportunity scores are based on code evidence, not user research. Validate with actual user feedback before committing Wave 2+ roadmap items.*
