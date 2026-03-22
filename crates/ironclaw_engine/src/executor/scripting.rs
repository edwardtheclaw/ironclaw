//! Tier 1 executor: embedded Python via Monty.
//!
//! Executes LLM-generated Python code using the Monty interpreter. Tool
//! calls happen as regular function calls in the code — Monty suspends at
//! each unknown function, and we delegate to the `EffectExecutor`.
//!
//! Follows the RLM (Recursive Language Model) pattern: thread context is
//! injected as Python variables (not LLM attention input), and `llm_query()`
//! enables recursive subagent spawning from within code.

use std::sync::Arc;
use std::time::Duration;

use monty::{
    ExcType, ExtFunctionResult, LimitedTracker, MontyException, MontyObject, MontyRun,
    NameLookupResult, PrintWriter, ResourceLimits, RunProgress,
};
use tracing::{debug, warn};

use crate::capability::lease::LeaseManager;
use crate::capability::policy::{PolicyDecision, PolicyEngine};
use crate::traits::effect::{EffectExecutor, ThreadExecutionContext};
use crate::traits::llm::{LlmBackend, LlmCallConfig};
use crate::types::error::EngineError;
use crate::types::event::EventKind;
use crate::types::message::{MessageRole, ThreadMessage};
use crate::types::step::{ActionResult, LlmResponse, TokenUsage};
use crate::types::thread::Thread;

/// Result of executing a code block.
pub struct CodeExecutionResult {
    /// The Python return value, converted to JSON.
    pub return_value: serde_json::Value,
    /// Captured print output.
    pub stdout: String,
    /// All action calls that were made during execution.
    pub action_results: Vec<ActionResult>,
    /// Events generated during execution.
    pub events: Vec<EventKind>,
    /// If set, execution was interrupted for approval.
    pub need_approval: Option<crate::runtime::messaging::ThreadOutcome>,
    /// Tokens used by recursive llm_query() calls.
    pub recursive_tokens: TokenUsage,
}

/// Default resource limits for Monty execution.
fn default_limits() -> ResourceLimits {
    ResourceLimits::new()
        .max_duration(Duration::from_secs(30))
        .max_allocations(1_000_000)
        .max_memory(64 * 1024 * 1024) // 64 MB
}

/// Maximum length of output metadata included in LLM context.
const OUTPUT_METADATA_MAX_PREVIEW: usize = 120;

/// Build a compact metadata summary of code output instead of the full text.
pub fn compact_output_metadata(stdout: &str, return_value: &serde_json::Value) -> String {
    let mut parts = Vec::new();

    if !stdout.is_empty() {
        let preview: String = stdout.chars().take(OUTPUT_METADATA_MAX_PREVIEW).collect();
        let truncated = if stdout.len() > OUTPUT_METADATA_MAX_PREVIEW {
            "..."
        } else {
            ""
        };
        parts.push(format!(
            "stdout ({} chars): {preview}{truncated}",
            stdout.len()
        ));
    }

    if *return_value != serde_json::Value::Null {
        let val_str = serde_json::to_string(return_value).unwrap_or_default();
        let preview: String = val_str.chars().take(OUTPUT_METADATA_MAX_PREVIEW).collect();
        let truncated = if val_str.len() > OUTPUT_METADATA_MAX_PREVIEW {
            "..."
        } else {
            ""
        };
        parts.push(format!(
            "return ({} chars): {preview}{truncated}",
            val_str.len()
        ));
    }

    if parts.is_empty() {
        "[code executed, no output]".into()
    } else {
        format!("[code output] {}", parts.join("; "))
    }
}

// ── Context injection (RLM 3.4) ────────────────────────────

/// Build Monty input variables from thread state.
///
/// Injects thread context as Python variables so the LLM's code can
/// access it selectively (RLM pattern: context as variable, not attention input).
fn build_context_inputs(thread: &Thread) -> (Vec<String>, Vec<MontyObject>) {
    let mut names = Vec::new();
    let mut values = Vec::new();

    // `context` — thread messages as a list of dicts
    let messages: Vec<MontyObject> = thread
        .messages
        .iter()
        .map(|msg| {
            let mut pairs = vec![
                (
                    MontyObject::String("role".into()),
                    MontyObject::String(format!("{:?}", msg.role)),
                ),
                (
                    MontyObject::String("content".into()),
                    MontyObject::String(msg.content.clone()),
                ),
            ];
            if let Some(ref name) = msg.action_name {
                pairs.push((
                    MontyObject::String("action_name".into()),
                    MontyObject::String(name.clone()),
                ));
            }
            MontyObject::dict(pairs)
        })
        .collect();
    names.push("context".into());
    values.push(MontyObject::List(messages));

    // `goal` — the thread's goal string
    names.push("goal".into());
    values.push(MontyObject::String(thread.goal.clone()));

    // `step_number` — current step index
    names.push("step_number".into());
    values.push(MontyObject::Int(thread.step_count as i64));

    // `previous_results` — dict of {call_id: result_json} from prior action results
    let result_pairs: Vec<(MontyObject, MontyObject)> = thread
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::ActionResult)
        .filter_map(|m| {
            let call_id = m.action_call_id.as_ref()?;
            Some((
                MontyObject::String(call_id.clone()),
                MontyObject::String(m.content.clone()),
            ))
        })
        .collect();
    names.push("previous_results".into());
    values.push(MontyObject::dict(result_pairs));

    (names, values)
}

/// Execute a Python code block using Monty.
///
/// Thread context is injected as Python variables (RLM pattern).
/// Unknown function calls suspend the VM and route to the `EffectExecutor`.
/// `llm_query(prompt, context)` calls spawn recursive child LLM calls.
#[allow(clippy::too_many_arguments)]
pub async fn execute_code(
    code: &str,
    thread: &Thread,
    llm: &Arc<dyn LlmBackend>,
    effects: &Arc<dyn EffectExecutor>,
    leases: &LeaseManager,
    policy: &PolicyEngine,
    context: &ThreadExecutionContext,
    capability_policies: &[crate::types::capability::PolicyRule],
) -> Result<CodeExecutionResult, EngineError> {
    let mut stdout = String::new();
    let mut action_results = Vec::new();
    let mut events = Vec::new();
    let mut recursive_tokens = TokenUsage::default();

    // Build context variables (RLM 3.4)
    let (input_names, input_values) = build_context_inputs(thread);

    // Parse and compile (wrap in catch_unwind — Monty 0.0.x can panic)
    let runner = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        MontyRun::new(code.to_string(), "step.py", input_names)
    })) {
        Ok(Ok(runner)) => runner,
        Ok(Err(e)) => {
            return Err(EngineError::Effect {
                reason: format!("Python parse error: {e}"),
            });
        }
        Err(_) => {
            return Err(EngineError::Effect {
                reason: "Monty VM panicked during code parsing".into(),
            });
        }
    };

    // Start execution with resource limits and context inputs
    let tracker = LimitedTracker::new(default_limits());

    let run_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runner.start(input_values, tracker, PrintWriter::Collect(&mut stdout))
    }));

    let mut progress = match run_result {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            return Err(EngineError::Effect {
                reason: format!("Python execution error: {e}"),
            });
        }
        Err(_) => {
            return Err(EngineError::Effect {
                reason: "Monty VM panicked during execution start".into(),
            });
        }
    };

    // Drive the execution loop — suspend at each function call
    let mut call_counter = 0u32;
    loop {
        match progress {
            RunProgress::Complete(obj) => {
                return Ok(CodeExecutionResult {
                    return_value: monty_to_json(&obj),
                    stdout,
                    action_results,
                    events,
                    need_approval: None,
                    recursive_tokens,
                });
            }

            RunProgress::FunctionCall(call) => {
                call_counter += 1;
                let call_id = format!("code_call_{call_counter}");
                let action_name = call.function_name.clone();
                let params = monty_args_to_json(&call.args, &call.kwargs);

                debug!(action = %action_name, call_id = %call_id, "Monty: function call");

                // Handle llm_query() — recursive subagent call (RLM 3.5)
                let ext_result = if action_name == "llm_query" {
                    handle_llm_query(&call.args, &call.kwargs, llm, &mut recursive_tokens).await
                } else {
                    // Regular tool dispatch through lease + policy
                    let dispatch = dispatch_action(
                        &action_name,
                        &call_id,
                        params.clone(),
                        thread,
                        effects,
                        leases,
                        policy,
                        context,
                        capability_policies,
                        &mut action_results,
                        &mut events,
                    )
                    .await;

                    match dispatch {
                        DispatchResult::Ok(r) => r,
                        DispatchResult::NeedApproval => {
                            return Ok(CodeExecutionResult {
                                return_value: serde_json::Value::Null,
                                stdout,
                                action_results,
                                events,
                                need_approval: Some(
                                    crate::runtime::messaging::ThreadOutcome::NeedApproval {
                                        action_name,
                                        call_id,
                                        parameters: params,
                                    },
                                ),
                                recursive_tokens,
                            });
                        }
                    }
                };

                // Resume Monty
                progress = resume_monty(
                    call.resume(ext_result, PrintWriter::Collect(&mut stdout)),
                )?;
            }

            RunProgress::NameLookup(lookup) => {
                let name = lookup.name.clone();
                debug!(name = %name, "Monty: unresolved name");
                progress = resume_monty(
                    lookup.resume(
                        NameLookupResult::Undefined,
                        PrintWriter::Collect(&mut stdout),
                    ),
                )?;
            }

            RunProgress::OsCall(os_call) => {
                warn!(function = ?os_call.function, "Monty: OS call denied");
                let err = ExtFunctionResult::Error(MontyException::new(
                    ExcType::OSError,
                    Some("OS operations are not permitted in CodeAct scripts".into()),
                ));
                progress = resume_monty(
                    os_call.resume(err, PrintWriter::Collect(&mut stdout)),
                )?;
            }

            RunProgress::ResolveFutures(_) => {
                return Err(EngineError::Effect {
                    reason: "async/await is not supported in CodeAct scripts".into(),
                });
            }
        }
    }
}

// ── llm_query() — recursive subagent (RLM 3.5) ─────────────

/// Handle a `llm_query(prompt, context)` call from within Python code.
///
/// Spawns a single-shot LLM call with the given prompt and context.
/// The result is returned as a MontyObject (string), not injected into
/// the parent's attention window (RLM pattern: symbolic composition).
async fn handle_llm_query(
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    llm: &Arc<dyn LlmBackend>,
    recursive_tokens: &mut TokenUsage,
) -> ExtFunctionResult {
    // Extract prompt (first arg or kwarg "prompt")
    let prompt = extract_string_arg(args, kwargs, "prompt", 0);
    let context_arg = extract_string_arg(args, kwargs, "context", 1);

    let prompt = match prompt {
        Some(p) => p,
        None => {
            return ExtFunctionResult::Error(MontyException::new(
                ExcType::TypeError,
                Some("llm_query() requires a 'prompt' argument".into()),
            ));
        }
    };

    // Build messages for the child LLM call
    let mut messages = Vec::new();
    if let Some(ctx) = context_arg {
        messages.push(ThreadMessage::system(format!(
            "You are a sub-agent. Here is the context:\n\n{ctx}"
        )));
    }
    messages.push(ThreadMessage::user(prompt));

    // Make the LLM call (no tools — pure text completion)
    let config = LlmCallConfig {
        force_text: true,
        ..LlmCallConfig::default()
    };

    match llm.complete(&messages, &[], &config).await {
        Ok(output) => {
            recursive_tokens.input_tokens += output.usage.input_tokens;
            recursive_tokens.output_tokens += output.usage.output_tokens;

            let response_text = match output.response {
                LlmResponse::Text(text) => text,
                LlmResponse::ActionCalls { content, .. } => content.unwrap_or_default(),
                LlmResponse::Code { content, .. } => content.unwrap_or_default(),
            };
            ExtFunctionResult::Return(MontyObject::String(response_text))
        }
        Err(e) => ExtFunctionResult::Error(MontyException::new(
            ExcType::RuntimeError,
            Some(format!("llm_query failed: {e}")),
        )),
    }
}

/// Extract a string argument by name (kwarg) or position (positional arg).
fn extract_string_arg(
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    name: &str,
    position: usize,
) -> Option<String> {
    // Check kwargs first
    for (k, v) in kwargs {
        if let MontyObject::String(key) = k
            && key == name
        {
            return Some(monty_to_string(v));
        }
    }
    // Then positional
    args.get(position).map(monty_to_string)
}

/// Convert any MontyObject to a string representation.
fn monty_to_string(obj: &MontyObject) -> String {
    match obj {
        MontyObject::String(s) => s.clone(),
        MontyObject::None => "None".into(),
        MontyObject::Bool(b) => b.to_string(),
        MontyObject::Int(i) => i.to_string(),
        MontyObject::Float(f) => f.to_string(),
        other => serde_json::to_string(&monty_to_json(other)).unwrap_or_else(|_| format!("{other:?}")),
    }
}

// ── Dispatch result ─────────────────────────────────────────

enum DispatchResult {
    Ok(ExtFunctionResult),
    NeedApproval,
}

/// Dispatch an action call through lease + policy + effect executor.
#[allow(clippy::too_many_arguments)]
async fn dispatch_action(
    action_name: &str,
    call_id: &str,
    params: serde_json::Value,
    thread: &Thread,
    effects: &Arc<dyn EffectExecutor>,
    leases: &LeaseManager,
    policy: &PolicyEngine,
    context: &ThreadExecutionContext,
    capability_policies: &[crate::types::capability::PolicyRule],
    action_results: &mut Vec<ActionResult>,
    events: &mut Vec<EventKind>,
) -> DispatchResult {
    // Find lease
    let lease = match leases.find_lease_for_action(thread.id, action_name).await {
        Some(l) => l,
        None => {
            events.push(EventKind::ActionFailed {
                step_id: context.step_id,
                action_name: action_name.into(),
                call_id: call_id.into(),
                error: format!("no lease for action '{action_name}'"),
            });
            return DispatchResult::Ok(ExtFunctionResult::NotFound(action_name.into()));
        }
    };

    // Find action definition and check policy
    let action_def = effects
        .available_actions(std::slice::from_ref(&lease))
        .await
        .ok()
        .and_then(|actions| actions.into_iter().find(|a| a.name == action_name));

    if let Some(ref action_def) = action_def {
        match policy.evaluate(action_def, &lease, capability_policies) {
            PolicyDecision::Deny { reason } => {
                events.push(EventKind::ActionFailed {
                    step_id: context.step_id,
                    action_name: action_name.into(),
                    call_id: call_id.into(),
                    error: reason.clone(),
                });
                return DispatchResult::Ok(ExtFunctionResult::Error(MontyException::new(
                    ExcType::RuntimeError,
                    Some(format!("denied: {reason}")),
                )));
            }
            PolicyDecision::RequireApproval { .. } => {
                events.push(EventKind::ApprovalRequested {
                    action_name: action_name.into(),
                    call_id: call_id.into(),
                });
                return DispatchResult::NeedApproval;
            }
            PolicyDecision::Allow => {}
        }
    }

    // Consume lease use
    if let Err(e) = leases.consume_use(lease.id).await {
        return DispatchResult::Ok(ExtFunctionResult::Error(MontyException::new(
            ExcType::RuntimeError,
            Some(format!("lease exhausted: {e}")),
        )));
    }

    // Execute the action
    match effects
        .execute_action(action_name, params, &lease, context)
        .await
    {
        Ok(result) => {
            events.push(EventKind::ActionExecuted {
                step_id: context.step_id,
                action_name: action_name.into(),
                call_id: call_id.into(),
                duration_ms: result.duration.as_millis() as u64,
            });
            let monty_obj = json_to_monty(&result.output);
            action_results.push(result);
            DispatchResult::Ok(ExtFunctionResult::Return(monty_obj))
        }
        Err(e) => {
            action_results.push(ActionResult {
                call_id: call_id.into(),
                action_name: action_name.into(),
                output: serde_json::json!({"error": e.to_string()}),
                is_error: true,
                duration: Duration::ZERO,
            });
            events.push(EventKind::ActionFailed {
                step_id: context.step_id,
                action_name: action_name.into(),
                call_id: call_id.into(),
                error: e.to_string(),
            });
            DispatchResult::Ok(ExtFunctionResult::Error(MontyException::new(
                ExcType::RuntimeError,
                Some(e.to_string()),
            )))
        }
    }
}

/// Wrap Monty resume results with error conversion.
fn resume_monty<T: monty::ResourceTracker>(
    result: Result<RunProgress<T>, MontyException>,
) -> Result<RunProgress<T>, EngineError> {
    result.map_err(|e| EngineError::Effect {
        reason: format!("Python execution error: {e}"),
    })
}

// ── MontyObject ↔ JSON conversion ───────────────────────────

/// Convert a MontyObject to serde_json::Value.
fn monty_to_json(obj: &MontyObject) -> serde_json::Value {
    match obj {
        MontyObject::None => serde_json::Value::Null,
        MontyObject::Bool(b) => serde_json::Value::Bool(*b),
        MontyObject::Int(i) => serde_json::json!(i),
        MontyObject::BigInt(i) => serde_json::Value::String(i.to_string()),
        MontyObject::Float(f) => serde_json::json!(f),
        MontyObject::String(s) => serde_json::Value::String(s.clone()),
        MontyObject::List(items) | MontyObject::Tuple(items) => {
            serde_json::Value::Array(items.iter().map(monty_to_json).collect())
        }
        MontyObject::Dict(pairs) => {
            let map: serde_json::Map<String, serde_json::Value> = pairs
                .into_iter()
                .map(|(k, v)| {
                    let key = match k {
                        MontyObject::String(s) => s.clone(),
                        other => format!("{other:?}"),
                    };
                    (key, monty_to_json(v))
                })
                .collect();
            serde_json::Value::Object(map)
        }
        MontyObject::Set(items) | MontyObject::FrozenSet(items) => {
            serde_json::Value::Array(items.iter().map(monty_to_json).collect())
        }
        MontyObject::Bytes(b) => serde_json::Value::String(
            b.iter().map(|byte| format!("{byte:02x}")).collect(),
        ),
        other => serde_json::Value::String(format!("{other:?}")),
    }
}

/// Convert serde_json::Value to MontyObject.
fn json_to_monty(val: &serde_json::Value) -> MontyObject {
    match val {
        serde_json::Value::Null => MontyObject::None,
        serde_json::Value::Bool(b) => MontyObject::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                MontyObject::Int(i)
            } else if let Some(f) = n.as_f64() {
                MontyObject::Float(f)
            } else {
                MontyObject::String(n.to_string())
            }
        }
        serde_json::Value::String(s) => MontyObject::String(s.clone()),
        serde_json::Value::Array(arr) => {
            MontyObject::List(arr.iter().map(json_to_monty).collect())
        }
        serde_json::Value::Object(map) => MontyObject::dict(
            map.iter()
                .map(|(k, v)| (MontyObject::String(k.clone()), json_to_monty(v)))
                .collect::<Vec<_>>(),
        ),
    }
}

/// Convert Monty function call args + kwargs to a JSON object.
fn monty_args_to_json(
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    if !args.is_empty() {
        map.insert(
            "_args".into(),
            serde_json::Value::Array(args.iter().map(monty_to_json).collect()),
        );
    }
    for (k, v) in kwargs {
        let key = match k {
            MontyObject::String(s) => s.clone(),
            other => format!("{other:?}"),
        };
        map.insert(key, monty_to_json(v));
    }
    serde_json::Value::Object(map)
}
