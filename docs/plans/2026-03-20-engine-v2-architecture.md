# IronClaw Engine v2: Unified Thread-Capability-CodeAct Architecture

**Date:** 2026-03-20
**Status:** Draft
**Goal:** Replace IronClaw's ~10 fragmented abstractions with a unified execution model built on 5 primitives: Thread, Step, Capability, MemoryDoc, Project. Developed as a standalone crate (`ironclaw_engine`) that can be swapped in when it passes all acceptance tests.

---

## Motivation

IronClaw currently has Session, Job, Routine, Channel, Tool, Skill, Hook, Observer, Extension, and LoopDelegate as separate abstractions. All share common patterns (lifecycle, messaging, state, capabilities) but are implemented independently. This causes:

- Duplicated logic across ChatDelegate, JobDelegate, ContainerDelegate
- Inconsistent state machines (SessionState vs JobState vs RoutineState)
- Three separate permission systems (ApprovalRequirement, ApprovalContext, SkillTrust)
- No structured learning from completed work
- No project-level context scoping (all memory in one flat namespace)
- The agentic loop can only do one tool call per LLM turn (no control flow)

## Design Principles

1. **Conversation is not execution** — UI surfaces (chat) are separate from work units (threads)
2. **Everything is a thread** — conversations, jobs, sub-agents, routines are all threads with different types
3. **Capabilities unify tools + skills + hooks** — one install gives you actions, knowledge, and policies
4. **Effects, not commands** — capabilities declare their effect types; a deterministic policy engine enforces boundaries
5. **Memory is docs, not logs** — durable knowledge is structured (summaries, lessons, playbooks), not raw history
6. **CodeAct for capable models** — LLMs write code that composes tools, queries history, and spawns threads
7. **Context as variable, not attention input** (RLM pattern) — thread context is a Python variable in the REPL, not tokens in the LLM window. The model writes code to selectively access it, avoiding context rot on long inputs
8. **Recursive subagent spawning** (RLM pattern) — code can call `llm_query()` to spawn child threads inline. Results are stored as variables, not injected into the parent's context window
9. **Event sourcing from day one** — every thread records a complete execution trace for replay/debugging/reflection

## The Five Primitives

| Primitive | Purpose | Replaces |
|-----------|---------|----------|
| **Thread** | Unit of work with lifecycle, parent-child tree, capability leases | Session + Job + Routine + Sub-agent |
| **Step** | Unit of execution (one LLM call + its tool/code executions) | Agentic loop iteration + tool calls |
| **Capability** | Unit of effect (actions + knowledge + policies) | Tool + Skill + Hook + Extension |
| **MemoryDoc** | Unit of durable knowledge (summaries, lessons, playbooks) | Workspace memory blobs |
| **Project** | Unit of context (scopes memory, threads, missions) | Flat workspace namespace |

## Crate Structure

Single crate: `crates/ironclaw_engine/`

```
crates/ironclaw_engine/
  Cargo.toml
  src/
    lib.rs                    # Public API, re-exports

    types/                    # Core data structures (no async, no I/O)
      mod.rs
      error.rs                # EngineError, ThreadError, StepError, CapabilityError
      thread.rs               # Thread, ThreadId, ThreadState, ThreadType, ThreadConfig
      step.rs                 # Step, StepId, StepStatus, ExecutionTier, ActionCall, ActionResult
      capability.rs           # Capability, ActionDef, EffectType, CapabilityLease, PolicyRule
      memory.rs               # MemoryDoc, DocId, DocType
      project.rs              # Project, ProjectId
      event.rs                # ThreadEvent, EventKind (event sourcing)
      provenance.rs           # Provenance enum (User, System, ToolOutput, LlmGenerated, etc.)
      message.rs              # ThreadMessage, MessageRole
      conversation.rs         # ConversationSurface, ConversationEntry (Phase 5)
      mission.rs              # Mission, MissionId (Phase 4)

    traits/                   # External dependency abstractions
      mod.rs
      llm.rs                  # LlmBackend trait
      store.rs                # Store trait (thread/step/event/project/doc/lease CRUD)
      effect.rs               # EffectExecutor trait
      code_runner.rs          # CodeRunner trait (Phase 3)

    capability/               # Capability management
      mod.rs
      registry.rs             # CapabilityRegistry
      lease.rs                # LeaseManager (grant, check, consume, revoke, expire)
      policy.rs               # PolicyEngine (deterministic effect-level allow/deny)
      provenance.rs           # ProvenanceTracker (taint analysis at effect boundaries, Phase 4)

    runtime/                  # Thread lifecycle management
      mod.rs
      manager.rs              # ThreadManager (spawn, supervise, stop, inject messages)
      tree.rs                 # ThreadTree (parent-child relationships)
      messaging.rs            # ThreadMailbox, ThreadSignal (inter-thread communication)
      conversation.rs         # ConversationManager (UI surface → thread routing, Phase 5)

    executor/                 # Step execution
      mod.rs
      loop_engine.rs          # ExecutionLoop (core loop replacing run_agentic_loop)
      structured.rs           # Tier 0: structured tool calls
      scripting.rs            # Tier 1: embedded Python via Monty (Phase 3)
      context.rs              # Context builder (thread state + project docs + capabilities)
      intent.rs               # Tool intent nudge detection

    memory/                   # Memory document system
      mod.rs
      store.rs                # MemoryStore (project-scoped doc operations)
      retrieval.rs            # RetrievalEngine (context building from project docs)

    reflection/               # Post-thread reflection pipeline
      mod.rs
      pipeline.rs             # ReflectionPipeline (summarize, extract lessons, detect issues)
      learning.rs             # Tool reliability learning, playbook promotion

    testing/                  # Test utilities (cfg(test))
      mod.rs
      mock_llm.rs             # MockLlmBackend (queued responses)
      mock_store.rs           # MockStore (in-memory HashMap storage)
      mock_effect.rs          # MockEffectExecutor (configurable results)
```

Dependencies (minimal — no main crate dependency):
- `tokio` (sync, time, macros, rt), `serde` + `serde_json`, `thiserror`, `tracing`, `uuid`, `chrono`, `async-trait`
- `monty` (git dep) — embedded Python interpreter for CodeAct (Tier 1)

---

## Phase 1: Foundation (Types + Traits + Thread Lifecycle)

**Goal:** Get the crate compiling with all core types, trait definitions, and thread state machine. No execution yet.

### 1.1 Crate scaffolding
- Create `crates/ironclaw_engine/Cargo.toml` (follow `ironclaw_safety` pattern)
- Add to workspace `members` in root `Cargo.toml`

### 1.2 Core types
All files in `src/types/`. Pure data structures with `Serialize`/`Deserialize`.

**`error.rs`** — Error hierarchy:
- `EngineError` (top-level: Thread, Step, Capability, Store, Llm, Effect, InvalidTransition, NotFound, LeaseExpired, LeaseDenied, MaxIterations)
- `ThreadError` (AlreadyRunning, Terminal, ParentNotRunning)
- `StepError` (Timeout, ActionDenied)
- `CapabilityError` (NotFound, EffectDenied)

**`thread.rs`** — Thread state machine:
```
Created → Running → Waiting → Running (resume)
                  → Suspended → Running (resume)
                  → Completed → Reflecting → Done
                  → Failed
```
- `ThreadState::can_transition_to(target) -> bool`
- `ThreadState::is_terminal() -> bool` (Done, Failed)
- `ThreadState::is_active() -> bool` (Running, Waiting)
- `Thread::transition_to(state) -> Result<(), EngineError>` (validates + records event)

**`step.rs`** — Step + LLM response types:
- `LlmResponse::Text(String)` or `LlmResponse::ActionCalls { calls, content }`
- `ActionCall { id, action_name, parameters }`
- `ActionResult { call_id, action_name, output, is_error, duration }`
- `TokenUsage { input_tokens, output_tokens, cache_read_tokens, cache_write_tokens }`
- `ExecutionTier` enum — Phase 1: only `Structured`

**`capability.rs`** — Effect typing + leases:
- `EffectType` enum: ReadLocal, ReadExternal, WriteLocal, WriteExternal, CredentialedNetwork, Compute, Financial
- `ActionDef { name, description, parameters_schema, effects: Vec<EffectType>, requires_approval }`
- `Capability { name, description, actions, knowledge: Vec<String>, policies: Vec<PolicyRule> }`
- `CapabilityLease { id, thread_id, capability_name, granted_actions, granted_at, expires_at, max_uses, uses_remaining, revoked }`
- `PolicyRule { name, condition: PolicyCondition, effect: PolicyEffect }`
- `PolicyCondition` enum: Always, ActionMatches { pattern }, EffectTypeIs(EffectType)
- `PolicyEffect` enum: Allow, Deny, RequireApproval

**`message.rs`** — Engine's own message type (simpler than `ChatMessage`):
- `MessageRole` enum: System, User, Assistant, ActionResult
- `ThreadMessage { role, content, provenance, action_call_id, action_name, action_calls, timestamp }`
- Constructors: `system()`, `user()`, `assistant()`, `assistant_with_actions()`, `action_result()`

**`event.rs`** — Event sourcing:
- `EventKind` enum: StateChanged, StepStarted, StepCompleted, StepFailed, ActionExecuted, ActionFailed, LeaseGranted, LeaseRevoked, LeaseExpired, MessageAdded, ChildSpawned, ChildCompleted, ApprovalRequested, ApprovalReceived

**`project.rs`**, **`memory.rs`**, **`provenance.rs`** — Straightforward structs.

### 1.3 Trait definitions
- `LlmBackend` — `complete(messages, actions, config) -> LlmOutput`, `model_name() -> &str`
- `Store` — Thread/Step/Event/Project/MemoryDoc/Lease CRUD (~20 methods)
- `EffectExecutor` — `execute_action(name, params, lease, ctx) -> ActionResult`, `available_actions(leases) -> Vec<ActionDef>`

### 1.4 Tests
- Thread state machine: all valid/invalid transitions
- ThreadMessage constructors
- CapabilityLease expiry checks (time-based, use-based)

### Verification
```bash
cargo check -p ironclaw_engine
cargo clippy -p ironclaw_engine --all-targets -- -D warnings
cargo test -p ironclaw_engine
```

---

## Phase 2: Execution Engine (Tier 0 — Structured Tool Calls)

**Goal:** A working execution loop that is functionally equivalent to the current `run_agentic_loop()`. Thread spawning, capability leasing, policy enforcement, event logging.

### 2.1 Capability management
- `CapabilityRegistry` — register/get/list capabilities and their actions
- `LeaseManager` — grant leases (scoped, time-limited, use-limited), check validity, consume uses, revoke, expire stale. State: `RwLock<HashMap<LeaseId, CapabilityLease>>`
- `PolicyEngine` — deterministic evaluation: `evaluate(action_def, lease, thread_context) -> PolicyDecision`. Check order: global policies → capability policies → action-level `requires_approval` → effect type against lease. Deny > RequireApproval > Allow

### 2.2 Thread runtime
- `ThreadTree` — in-memory parent-child tracking. `add_child()`, `parent_of()`, `children_of()`, `remove()`, `ancestors()`
- `ThreadMailbox` + `ThreadSignal` — `mpsc`-based inter-thread messaging. Signals: Stop, Suspend, Resume, InjectMessage, ChildCompleted
- `ThreadManager` — orchestrator. `spawn_thread()` creates thread + leases + `ExecutionLoop`, wraps in tokio task. `stop_thread()`, `inject_message()`, `get_thread_state()`. Holds `Arc<dyn Store/LlmBackend/EffectExecutor>` + capability/lease/policy

### 2.3 Execution loop
- `build_step_context()` — assemble messages + action definitions from thread state + active leases
- `execute_action_calls()` — Tier 0: for each ActionCall, find lease → check policy → consume use → call `EffectExecutor` → record result + event. Returns NeedApproval if policy requires it
- `ExecutionLoop::run()` — core loop mirroring `run_agentic_loop()`:
  1. `signal_rx.try_recv()` → handle Stop, Suspend, InjectMessage
  2. `build_step_context()` → messages + actions
  3. `llm.complete()` → LlmOutput
  4. If `LlmResponse::Text` → check tool intent nudge, return if final
  5. If `LlmResponse::ActionCalls` → `execute_action_calls()`, add results to messages
  6. Record Step, emit events
  7. Check max_iterations, force_text on final iterations
  8. Repeat

### 2.4 Memory stubs
- `MemoryStore` — thin wrapper: `create_doc()`, `get_doc()`, `update_doc()`, `list_by_type()`
- `RetrievalEngine` — stub returning empty vec

### 2.5 Tests (comprehensive)
- Simple text response: MockLlm returns text → thread Created→Running→Completed→Done
- Tool call then text: MockLlm returns ActionCalls then text → effect executor called, result in messages
- Multi-tool parallel: Multiple ActionCalls in one response → all executed, all results recorded
- Max iterations: MockLlm always returns actions → loop stops at limit
- Stop signal: Send Stop → clean termination
- Inject message: Send InjectMessage during loop → appears in context
- Lease expiry (uses): max_uses=1 → first OK, second fails
- Lease expiry (time): expires_at in past → immediate failure
- Policy deny: Financial effect blocked → ActionDenied
- Policy require approval: → returns NeedApproval outcome
- Event sourcing: run loop → verify all events recorded in order
- Tool intent nudge: "Let me search..." text → nudge injected, capped at max
- Child thread spawning: spawn from parent → tree relationships correct, child completion event on parent

### Verification
```bash
cargo test -p ironclaw_engine
# All existing tests still pass:
cargo test
```

---

## Phase 3: CodeAct Executor (Tier 1 — Monty Python + RLM Pattern)

**Goal:** LLMs write Python code that composes tools, uses control flow, queries thread context as data, and recursively spawns sub-agents. Uses the Monty interpreter (Pydantic) for sandboxed in-process execution. Follows the Recursive Language Model (RLM) pattern: context as a variable, not attention input.

**Status:** Implemented (Phases 3.1–3.3). Phases 3.4–3.5 are the RLM enhancements.

### 3.1 Monty integration (DONE)

`executor/scripting.rs` — Embeds the Monty Python interpreter (git dep, v0.0.8).

**Execution model:**
1. `MontyRun::new(code, "step.py", input_names)` — parse Python code
2. `runner.start(inputs, tracker, print_writer)` — begin execution with resource limits
3. Loop over `RunProgress` suspension points:
   - `FunctionCall` → find lease → check policy → call `EffectExecutor` → resume with result
   - `NameLookup` → resolve or raise `NameError`
   - `OsCall` → deny with `OSError`
   - `ResolveFutures` → error (async not supported)
   - `Complete` → return value + captured stdout
4. All execution wrapped in `catch_unwind` (Monty 0.0.x can panic)

**Resource limits:** 30s timeout, 64MB memory, 1M allocations, recursion depth 1000.

**Tool dispatch:** Unknown function calls in Python suspend the VM via `RunProgress::FunctionCall`. The engine routes through the same lease → policy → `EffectExecutor` pipeline as structured tool calls:
```python
result = web_fetch(url="https://example.com")   # suspends → EffectExecutor
data = memory_search(query="deployment")          # suspends → EffectExecutor
for item in result["items"]:                      # control flow in Python
    memory_write(key=item["id"], value=item["summary"])
```

**Type conversion:** `monty_to_json()` / `json_to_monty()` bidirectional conversion between `MontyObject` and `serde_json::Value`.

### 3.2 LlmResponse::Code variant (DONE)

New `LlmResponse::Code { code, content }` variant alongside `Text` and `ActionCalls`. The `ExecutionLoop` routes `Code` responses to `scripting::execute_code()` instead of `structured::execute_action_calls()`.

### 3.3 ExecutionLoop integration (DONE)

The loop handles `LlmResponse::Code`:
- Records assistant message with code
- Sets `step.tier = ExecutionTier::Scripting`
- Executes via `scripting::execute_code()`
- Records events and action results
- Captures stdout + return value as context for next iteration
- Handles `NeedApproval` outcome (pauses thread)

### 3.4 RLM: Context as variables (TO IMPLEMENT)

Inspired by Recursive Language Models (arXiv:2512.24601). The key insight: **the prompt is an environment variable, not attention input.** The LLM never sees the full thread context in its window — it writes code to access it selectively.

**Implementation:**
- Pass thread state as Monty input variables via `MontyRun::new(code, "step.py", input_names)`:
  - `context` — full thread message history as a Python list of dicts
  - `goal` — the thread's goal string
  - `step_number` — current step index
  - `previous_results` — dict of `{call_id: result}` from prior steps
- Use compact output metadata between code steps: `"[code output: 4,532 chars]"` instead of full stdout in chat history
- The LLM's chat context stays lean; the full data lives in REPL variables

**Before (current):** Full context in LLM attention window
```
System: You are an agent...
User: Analyze these 1000 items...
[1000 items in context]
Assistant: ```python result = web_fetch(...)```
```

**After (RLM pattern):** Context as a variable
```
System: You have access to `context` (1000 items) and `previous_results`.
Write Python to accomplish the goal.
Assistant: ```python
items = context  # never loaded into LLM window
for batch in [items[i:i+100] for i in range(0, len(items), 100)]:
    result = llm_query("summarize these items", batch)
    # result is a variable, not injected into parent context
```

### 3.5 RLM: Recursive `llm_query()` within code (TO IMPLEMENT)

Expose `llm_query(prompt, context)` as a callable inside the Monty environment. When code calls it, Monty suspends via `FunctionCall`. The engine:
1. Spawns a child thread with the given prompt and context
2. Runs the child to completion (inline, blocking the parent's code)
3. Returns the child's result as a `MontyObject`

This enables the core RLM patterns:
```python
# Partition + Map + Reduce
chunks = [context[i:i+1000] for i in range(0, len(context), 1000)]
summaries = []
for chunk in chunks:
    summary = llm_query("Summarize this section", chunk)
    summaries.append(summary)  # variable, not in parent's LLM context
final = llm_query("Combine these summaries", summaries)

# Verification
answer = llm_query("What is X?", context)
verified = llm_query(f"Is this answer correct: {answer}", context)
```

**Key RLM properties preserved:**
- **Symbolic handle to context** — the parent LLM never sees child outputs in its attention window
- **Unbounded output** — variables in the REPL can exceed the context window
- **Recursive decomposition** — the model decides how to partition work, not the architect

### 3.6 Tests
- **Simple code execution:** `x = 1 + 2` → returns 3, no tool calls
- **Tool call from code:** `result = web_fetch(url="...")` → `FunctionCall` suspension → effect executor called → result returned to Python
- **Multiple tool calls in loop:** `for i in range(3): fetch(url=urls[i])` → 3 effect executor calls
- **Context as variable:** Code accesses `context[0]` → correct value from thread messages
- **Compact metadata:** After code step, context has metadata summary not full stdout
- **`llm_query()` recursive call:** Code calls `llm_query("summarize", data)` → child thread spawned → result returned as variable
- **Resource limits:** Infinite loop → Monty `TimeoutError`
- **OS call denied:** `import os; os.listdir(".")` → `OSError`
- **VM panic recovery:** Monty panics → `catch_unwind` returns `EngineError`, thread doesn't crash
- **Policy deny in code:** Code calls denied action → Python `RuntimeError` raised
- **Approval needed in code:** Code calls approval-required action → `NeedApproval` returned, code halted

---

## Phase 4: Memory, Reflection, and Learning

**Goal:** The agent learns from its work. Completed threads produce structured knowledge (summaries, lessons, playbooks) that improve future threads.

### 4.1 Project-scoped retrieval
- `RetrievalEngine::retrieve_context(project_id, query, max_docs)` — keyword + semantic search over project's memory docs
- Context builder uses retrieval: thread state + project docs (summaries, lessons, playbooks) + capability descriptions
- The LLM gets relevant project knowledge, not raw history

### 4.2 Reflection pipeline
After thread completes (state → Completed), optionally spawns a Reflection-type thread:
1. **Summarize** → produce `DocType::Summary` doc
2. **Extract lessons** → scan for failures, workarounds, discoveries → produce `DocType::Lesson` docs
3. **Detect issues** → find problems that weren't resolved → produce `DocType::Issue` docs
4. **Detect missing capabilities** → "no tool available" patterns → produce `DocType::Spec` docs
5. **Promote playbooks** → successful multi-step procedures → produce `DocType::Playbook` docs

Reflection is itself a thread running CodeAct — it's recursive.

### 4.3 Provenance tracking
Every data value tagged with origin:
- `Provenance::User` — direct user input
- `Provenance::System` — system prompt, config
- `Provenance::ToolOutput { action_name }` — result from a capability action
- `Provenance::LlmGenerated` — LLM output
- `Provenance::Reflection { source_thread_id }` — from reflection pipeline
- `Provenance::MemoryRetrieval { doc_id }` — from project memory

The policy engine uses provenance at effect boundaries:
- LlmGenerated data cannot flow into Financial effects without approval
- ToolOutput from untrusted sources triggers extra validation
- User-provenance data is trusted (no taint)

### 4.4 Missions (long-running goals)
```rust
pub struct Mission {
    pub id: MissionId,
    pub project_id: ProjectId,
    pub goal: String,
    pub status: MissionStatus, // Active, Paused, Completed, Failed
    pub cadence: MissionCadence, // Cron, OnEvent, OnPush, Manual
    pub thread_history: Vec<ThreadId>, // past threads spawned by this mission
    pub success_criteria: Option<String>,
}
```
Missions spawn threads on cadence, track progress across runs, and adapt based on reflection docs.

### 4.5 Tool reliability learning
Track per-action metrics:
- Success rate (EMA)
- Avg latency
- Common failure patterns
- Last N results

Feed into context builder so the LLM knows "this tool has been flaky recently."

### 4.6 Tests
- Reflection produces correct doc types for a completed thread with failures
- Retrieval returns project-scoped docs, not cross-project
- Provenance taint blocks financial effects from LLM-generated data
- Mission spawns thread on cadence, tracks history
- Tool reliability metrics update correctly after successes/failures

---

## Phase 5: Conversation Surface + Multi-Channel Integration

**Goal:** Conversations (UI) are cleanly separated from threads (execution). Multiple channels route to the same thread model.

### 5.1 ConversationSurface
```rust
pub struct ConversationSurface {
    pub id: ConversationId,
    pub channel: String,        // "telegram", "slack", "web", "cli"
    pub user_id: String,
    pub entries: Vec<ConversationEntry>,
    pub active_threads: Vec<ThreadId>,
}

pub struct ConversationEntry {
    pub id: EntryId,
    pub sender: EntrySender,    // User or Agent
    pub content: String,
    pub origin_thread_id: Option<ThreadId>,
    pub timestamp: DateTime<Utc>,
}
```

### 5.2 ConversationManager
- Routes incoming channel messages to conversation surfaces
- User message → may spawn new foreground thread or inject into existing
- Multiple threads can be active simultaneously per conversation
- Thread outputs (replies, status updates) appear as conversation entries

### 5.3 Channel adaptation
The existing `Channel` trait stays. A bridge adapter translates:
- `IncomingMessage` → `ConversationEntry` → spawn/inject `Thread`
- `ThreadOutcome` → `ConversationEntry` → `OutgoingResponse`
- `StatusUpdate` events → `ConversationEntry` with metadata

### 5.4 Tests
- Two concurrent threads in one conversation → entries interleaved correctly
- Thread outlives conversation (background) → results appear when user returns
- Channel-agnostic: same thread model works for Telegram, Web, CLI

---

## Phase 6: Advanced Execution (Tier 2-3 + Two-Phase Commit)

**Goal:** Full CodeAct with WASM sandbox (Tier 2) and Docker container (Tier 3). Two-phase commit for high-stakes effects.

### 6.1 Tier 2: WASM sandbox
- Embed Python interpreter (RustPython) or use Starlark compiled to WASM
- Leverage existing `wasmtime` infrastructure from `src/tools/wasm/`
- Fuel metering, memory limits, network allowlisting (all existing)
- Runtime API exposed via WIT interface (extend existing `wit/tool.wit`)

### 6.2 Tier 3: Docker container
- Leverage existing `src/sandbox/` + `src/orchestrator/` infrastructure
- Full Python runtime with `thread.*` and `tools.*` available via HTTP proxy
- Network access through existing sandbox proxy (domain allowlist, credential injection)

### 6.3 Automatic tier selection
Analyze LLM-generated code:
- Pure `tools.*` calls, no I/O → Tier 1 (embedded)
- Uses `tools.web_fetch` or HTTP → Tier 2 (WASM, allowlisted network)
- Uses `tools.shell`, `import os`, filesystem → Tier 3 (Docker)
- Falls back gracefully: if Tier 1 fails with capability error, promote to Tier 2/3

### 6.4 Two-phase commit
For `WriteExternal` + `Financial` effects:
1. **Simulate** — dry-run the effect, return preview
2. **Approve** — user or policy approves
3. **Execute** — actual effect

Replaces current binary approve/deny with richer commit policies:
- `CommitPolicy::Direct` — execute immediately (ReadLocal, ReadExternal)
- `CommitPolicy::Approved` — needs approval before execution (WriteExternal)
- `CommitPolicy::TwoPhase` — simulate → approve → execute (Financial, production deploys)

### 6.5 Tests
- Tier selection routes correctly based on code analysis
- WASM sandbox enforces fuel limits
- Docker container executes with proper isolation
- Two-phase commit: simulate returns preview, approve triggers execution
- Tier escalation: Tier 1 failure promotes to Tier 3

---

## Phase 7: Main Crate Integration

**Goal:** Bridge adapters connect the engine to existing IronClaw infrastructure. Feature-flagged swap.

### 7.1 Bridge adapters (`src/bridge/`)
- `LlmBridgeAdapter` — wraps `Arc<dyn LlmProvider>`, converts `ThreadMessage` ↔ `ChatMessage`, `ActionDef` ↔ `ToolDefinition`
- `StoreBridgeAdapter` — wraps `Arc<dyn Database>`, maps engine CRUD to existing sub-traits. New tables for threads/projects/docs/leases/events (migration V14+)
- `EffectBridgeAdapter` — wraps `ToolRegistry` + `SafetyLayer`. On `execute_action()`: lookup tool → validate params via safety → execute → sanitize output → return. This is where safety logic lives (not in the engine)

### 7.2 Database migrations
New tables:
- `engine_threads` (id, goal, type, state, project_id, parent_id, config_json, metadata, timestamps)
- `engine_steps` (id, thread_id, sequence, status, tier, request_json, response_json, results_json, tokens_json, timestamps)
- `engine_events` (id, thread_id, timestamp, kind_json)
- `engine_projects` (id, name, description, metadata, timestamps)
- `engine_memory_docs` (id, project_id, doc_type, title, content, source_thread_id, tags_json, metadata, timestamps)
- `engine_capability_leases` (id, thread_id, capability_name, granted_actions_json, granted_at, expires_at, max_uses, uses_remaining, revoked)

Both PostgreSQL and libSQL backends (per existing dual-backend requirement).

### 7.3 Feature-flagged swap
```rust
// In app.rs or agent_loop.rs:
#[cfg(feature = "engine_v2")]
{
    let engine = ironclaw_engine::ThreadManager::new(
        Arc::new(LlmBridgeAdapter::new(llm_provider)),
        Arc::new(StoreBridgeAdapter::new(database)),
        Arc::new(EffectBridgeAdapter::new(tool_registry, safety)),
    );
    // Use engine for thread management
}
```

### 7.4 Alternative LoopDelegate
Implement a `EngineV2Delegate` that wraps the engine's `ExecutionLoop` but presents the `LoopDelegate` interface. This enables gradual migration — the existing dispatcher calls `run_agentic_loop()` with either the old ChatDelegate or the new EngineV2Delegate.

### 7.5 Acceptance testing
Use existing `TestRig` + `TraceLlm` infrastructure:
- Load pre-recorded LLM trace fixtures
- Drive the engine via bridge adapters
- Compare output with `verify_trace_expects()`
- All existing fixture tests must pass with identical results

When all tests pass: remove feature flag, make engine the default, deprecate old path.

### 7.6 Tests
- Bridge adapter conversion: ThreadMessage ↔ ChatMessage round-trips correctly
- End-to-end: TestRig drives engine, same output as old loop
- Migration: new tables created for both PostgreSQL and libSQL
- Feature flag: both paths compile and pass tests

---

## Phase 8: Cleanup and Migration

**Goal:** Remove old abstractions, migrate all code to engine model.

### 8.1 Deprecate old types
- `Session` / `Thread` / `Turn` → engine `Thread` + `Step`
- `JobState` / `JobContext` → engine `ThreadState` + `Thread`
- `RoutineEngine` / `Routine` → engine `Mission` + `Thread`
- `SkillSelector` / `LoadedSkill` → engine `Capability` (knowledge)
- `HookPipeline` → engine `Capability` (policies)
- `ApprovalRequirement` / `ApprovalContext` → engine `CapabilityLease` + `PolicyEngine`

### 8.2 Slim down main crate
- Agent module becomes thin adapter over engine
- `app.rs` orchestrates engine startup instead of manually wiring channels/tools/sessions
- Remove `LoopDelegate` and its three implementations
- Remove `SessionManager`, `Scheduler` (replaced by `ThreadManager`)

### 8.3 Sub-crate extraction
Once engine boundaries are stable, split internal modules into sub-crates if beneficial:
- `ironclaw_types` — shared types usable by WASM extensions
- `ironclaw_capability` — if used by tooling/CLI independently
- `ironclaw_codeact` — if the code runner grows complex

---

## Cross-Cutting Concerns

### Security Model
- **Capability leases** replace static permissions. Scoped per-thread, time-limited, use-limited. Blast radius bounded by lease
- **Effect typing** on every action. Policy engine uses effect types (not tool names) for allow/deny
- **Provenance tracking** (Phase 4). Data tagged with origin; taint analysis at effect boundaries
- **Two-phase commit** (Phase 6) for WriteExternal + Financial effects
- **Safety at adapter boundary**. The engine is pure orchestration; `SafetyLayer` (sanitization, leak detection, injection checking) is applied in `EffectBridgeAdapter`

### Observability
- **Event sourcing** replaces ad-hoc `ObserverEvent`. Every thread has a complete event log
- **Trace-based testing** (Phase 4+). Use event logs as golden traces for regression testing
- **Thread-structural events** (thread.started, step.completed, action.executed) vs current per-subsystem events

### Backward Compatibility
- Engine runs alongside existing code (feature flag)
- Bridge adapters translate between engine and existing types
- WASM tools/channels unchanged — they implement `Tool`/`Channel` traits, which the bridge wraps
- MCP tools unchanged — same adapter principle
- Existing tests unmodified — they test the old path; new tests validate the engine

---

## Implementation Order Summary

| Phase | Scope | Depends on | Key deliverable |
|-------|-------|------------|-----------------|
| **1** | Types + traits + state machine | Nothing | Compiling crate with all type definitions |
| **2** | Tier 0 executor + capability + runtime | Phase 1 | Working execution loop equivalent to `run_agentic_loop()` |
| **3** | CodeAct (Tier 1 embedded scripting) | Phase 2 | LLMs write code that composes tools |
| **4** | Reflection + retrieval + provenance + missions | Phase 2 | Agent learns from work, project-scoped memory |
| **5** | Conversation surface + channel integration | Phase 2 | UI separated from execution |
| **6** | Tier 2-3 + two-phase commit | Phase 3 | Full sandboxed code execution |
| **7** | Main crate bridge + acceptance tests | Phase 2+ | Engine passes all existing tests via adapters |
| **8** | Cleanup + migration | Phase 7 | Old abstractions removed |

Phases 3, 4, 5 can proceed in parallel after Phase 2 is complete.

---

## Verification (per phase)

```bash
# Engine crate only:
cargo check -p ironclaw_engine
cargo clippy -p ironclaw_engine --all-targets -- -D warnings
cargo test -p ironclaw_engine

# Full workspace (no regressions):
cargo check
cargo clippy --all --benches --tests --examples --all-features
cargo test

# Phase 7+ acceptance:
cargo test --features engine_v2  # engine-driven tests match existing fixtures
```
