//! Effect executor for reflection threads.
//!
//! Provides read-only tools that let the reflection CodeAct thread
//! introspect the completed thread, query existing knowledge, and
//! verify tool names against the capability registry.

use std::sync::Arc;

use crate::capability::registry::CapabilityRegistry;
use crate::memory::RetrievalEngine;
use crate::traits::effect::{EffectExecutor, ThreadExecutionContext};
use crate::traits::store::Store;
use crate::types::capability::{ActionDef, CapabilityLease, EffectType};
use crate::types::error::EngineError;
use crate::types::project::ProjectId;
use crate::types::step::ActionResult;

/// EffectExecutor that provides reflection-specific read-only tools.
pub struct ReflectionExecutor {
    store: Arc<dyn Store>,
    capabilities: Arc<CapabilityRegistry>,
    transcript: String,
    project_id: ProjectId,
}

impl ReflectionExecutor {
    pub fn new(
        store: Arc<dyn Store>,
        capabilities: Arc<CapabilityRegistry>,
        transcript: String,
        project_id: ProjectId,
    ) -> Self {
        Self {
            store,
            capabilities,
            transcript,
            project_id,
        }
    }

    fn action_defs() -> Vec<ActionDef> {
        vec![
            ActionDef {
                name: "get_transcript".into(),
                description: "Get the full execution transcript of the completed thread, \
                              including messages, tool calls, errors, and outcomes."
                    .into(),
                parameters_schema: serde_json::json!({"type": "object", "properties": {}}),
                effects: vec![EffectType::ReadLocal],
                requires_approval: false,
            },
            ActionDef {
                name: "query_memory".into(),
                description: "Search existing memory docs in this project for prior knowledge. \
                              Use to check if a lesson or issue has already been recorded."
                    .into(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Search query"},
                        "max_docs": {"type": "integer", "description": "Max results (default 5)"}
                    },
                    "required": ["query"]
                }),
                effects: vec![EffectType::ReadLocal],
                requires_approval: false,
            },
            ActionDef {
                name: "check_tool_exists".into(),
                description: "Check if a tool/action exists in the capability registry. \
                              Returns whether it exists and lists similar tool names if not found."
                    .into(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "Tool name to check"}
                    },
                    "required": ["name"]
                }),
                effects: vec![EffectType::ReadLocal],
                requires_approval: false,
            },
            ActionDef {
                name: "list_tools".into(),
                description: "List all available tools/actions in the capability registry.".into(),
                parameters_schema: serde_json::json!({"type": "object", "properties": {}}),
                effects: vec![EffectType::ReadLocal],
                requires_approval: false,
            },
        ]
    }
}

#[async_trait::async_trait]
impl EffectExecutor for ReflectionExecutor {
    async fn execute_action(
        &self,
        action_name: &str,
        parameters: serde_json::Value,
        _lease: &CapabilityLease,
        _context: &ThreadExecutionContext,
    ) -> Result<ActionResult, EngineError> {
        let start = std::time::Instant::now();
        let output = match action_name {
            "get_transcript" => serde_json::json!({ "transcript": self.transcript }),

            "query_memory" => {
                let query = parameters["query"].as_str().unwrap_or("");
                let max_docs = parameters["max_docs"].as_u64().unwrap_or(5) as usize;
                let retrieval = RetrievalEngine::new(Arc::clone(&self.store));
                let docs = retrieval
                    .retrieve_context(self.project_id, query, max_docs)
                    .await?;
                let results: Vec<serde_json::Value> = docs
                    .iter()
                    .map(|d| {
                        serde_json::json!({
                            "type": format!("{:?}", d.doc_type),
                            "title": &d.title,
                            "content": &d.content,
                        })
                    })
                    .collect();
                serde_json::json!({ "docs": results, "count": results.len() })
            }

            "check_tool_exists" => {
                let name = parameters["name"].as_str().unwrap_or("");
                let exists = self.capabilities.find_action(name).is_some();
                let similar: Vec<String> = if exists {
                    vec![]
                } else {
                    // Find tools with similar names (substring or edit-distance-like match)
                    let name_lower = name.to_lowercase();
                    // Normalize: replace hyphens with underscores and vice versa for matching
                    let alt_name = if name.contains('_') {
                        name.replace('_', "-")
                    } else {
                        name.replace('-', "_")
                    };
                    self.capabilities
                        .all_actions()
                        .iter()
                        .filter(|a| {
                            let a_lower = a.name.to_lowercase();
                            a_lower.contains(&name_lower)
                                || name_lower.contains(&a_lower)
                                || a.name == alt_name
                        })
                        .map(|a| a.name.clone())
                        .collect()
                };
                serde_json::json!({ "exists": exists, "similar": similar })
            }

            "list_tools" => {
                let tools: Vec<serde_json::Value> = self
                    .capabilities
                    .all_actions()
                    .iter()
                    .map(|a| {
                        serde_json::json!({
                            "name": &a.name,
                            "description": &a.description,
                        })
                    })
                    .collect();
                serde_json::json!({ "tools": tools, "count": tools.len() })
            }

            _ => {
                return Err(EngineError::Effect {
                    reason: format!("unknown reflection action: {action_name}"),
                });
            }
        };

        Ok(ActionResult {
            call_id: String::new(),
            action_name: action_name.into(),
            output,
            is_error: false,
            duration: start.elapsed(),
        })
    }

    async fn available_actions(
        &self,
        _leases: &[CapabilityLease],
    ) -> Result<Vec<ActionDef>, EngineError> {
        // Reflection tools are always available regardless of leases
        Ok(Self::action_defs())
    }
}

/// Build the system prompt for a reflection CodeAct thread.
pub fn build_reflection_prompt(actions: &[ActionDef], thread_goal: &str) -> String {
    let mut prompt = String::from(REFLECTION_PREAMBLE);

    prompt.push_str("\n## Available tools (call as Python functions)\n\n");
    for action in actions {
        prompt.push_str(&format!("- `{}(", action.name));
        if let Some(props) = action.parameters_schema.get("properties")
            && let Some(obj) = props.as_object()
        {
            let params: Vec<&str> = obj.keys().map(String::as_str).collect();
            prompt.push_str(&params.join(", "));
        }
        prompt.push_str(&format!(")` — {}\n", action.description));
    }

    prompt.push_str(&format!(
        "\n## Thread Under Analysis\n\nGoal: {thread_goal}\n"
    ));

    prompt.push_str(REFLECTION_POSTAMBLE);
    prompt
}

const REFLECTION_PREAMBLE: &str = "\
You are analyzing a completed agent thread to extract structured knowledge. \
You have tools to inspect the thread's execution, check existing knowledge, \
and verify tool names.

Write Python code in ```repl blocks to analyze the thread.";

const REFLECTION_POSTAMBLE: &str = r#"

## Your Task

1. Call `get_transcript()` to read the thread's execution history
2. Analyze the transcript for: successes, failures, tool errors, lessons learned
3. Call `query_memory(query)` to check if similar knowledge already exists
4. For any tool errors with "not found", call `check_tool_exists(name)` to find the correct name
5. Call `FINAL()` with a JSON object containing a `docs` array:

```repl
FINAL({
    "docs": [
        {"type": "summary", "title": "...", "content": "2-4 sentence summary"},
        {"type": "lesson", "title": "...", "content": "what was learned"},
        {"type": "spec", "title": "...", "content": "ALIAS: wrong_name -> correct_name"},
        {"type": "playbook", "title": "...", "content": "1. step one\n2. step two"}
    ]
})
```

Rules:
- Always include a "summary" doc
- Include "lesson" only if there were errors or workarounds
- Include "spec" only if tool-not-found errors occurred (verify with check_tool_exists)
- Include "playbook" only if the thread completed successfully with 2+ tool calls
- Skip docs that duplicate existing knowledge (check with query_memory first)
- Keep content concise — each doc should be a few sentences, not paragraphs"#;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::Utc;

    use crate::capability::registry::CapabilityRegistry;
    use crate::traits::effect::{EffectExecutor, ThreadExecutionContext};
    use crate::traits::store::Store;
    use crate::types::capability::{ActionDef, Capability, CapabilityLease, EffectType, LeaseId};
    use crate::types::error::EngineError;
    use crate::types::event::ThreadEvent;
    use crate::types::memory::{DocId, DocType, MemoryDoc};
    use crate::types::mission::{Mission, MissionId, MissionStatus};
    use crate::types::project::{Project, ProjectId};
    use crate::types::step::{Step, StepId};
    use crate::types::thread::{Thread, ThreadId, ThreadState, ThreadType};

    use super::{ReflectionExecutor, build_reflection_prompt};

    // ── MockStore ──────────────────────────────────────────────

    struct MockStore {
        docs: tokio::sync::Mutex<Vec<MemoryDoc>>,
    }

    impl MockStore {
        fn new(docs: Vec<MemoryDoc>) -> Arc<Self> {
            Arc::new(Self {
                docs: tokio::sync::Mutex::new(docs),
            })
        }

        fn empty() -> Arc<Self> {
            Self::new(vec![])
        }
    }

    #[async_trait::async_trait]
    impl Store for MockStore {
        async fn save_thread(&self, _: &Thread) -> Result<(), EngineError> {
            Ok(())
        }
        async fn load_thread(&self, _: ThreadId) -> Result<Option<Thread>, EngineError> {
            Ok(None)
        }
        async fn list_threads(&self, _: ProjectId) -> Result<Vec<Thread>, EngineError> {
            Ok(vec![])
        }
        async fn update_thread_state(
            &self,
            _: ThreadId,
            _: ThreadState,
        ) -> Result<(), EngineError> {
            Ok(())
        }
        async fn save_step(&self, _: &Step) -> Result<(), EngineError> {
            Ok(())
        }
        async fn load_steps(&self, _: ThreadId) -> Result<Vec<Step>, EngineError> {
            Ok(vec![])
        }
        async fn append_events(&self, _: &[ThreadEvent]) -> Result<(), EngineError> {
            Ok(())
        }
        async fn load_events(&self, _: ThreadId) -> Result<Vec<ThreadEvent>, EngineError> {
            Ok(vec![])
        }
        async fn save_project(&self, _: &Project) -> Result<(), EngineError> {
            Ok(())
        }
        async fn load_project(&self, _: ProjectId) -> Result<Option<Project>, EngineError> {
            Ok(None)
        }
        async fn save_memory_doc(&self, _: &MemoryDoc) -> Result<(), EngineError> {
            Ok(())
        }
        async fn load_memory_doc(&self, _: DocId) -> Result<Option<MemoryDoc>, EngineError> {
            Ok(None)
        }
        async fn list_memory_docs(
            &self,
            project_id: ProjectId,
        ) -> Result<Vec<MemoryDoc>, EngineError> {
            let docs = self.docs.lock().await;
            Ok(docs
                .iter()
                .filter(|d| d.project_id == project_id)
                .cloned()
                .collect())
        }
        async fn save_lease(&self, _: &CapabilityLease) -> Result<(), EngineError> {
            Ok(())
        }
        async fn load_active_leases(
            &self,
            _: ThreadId,
        ) -> Result<Vec<CapabilityLease>, EngineError> {
            Ok(vec![])
        }
        async fn revoke_lease(&self, _: LeaseId, _: &str) -> Result<(), EngineError> {
            Ok(())
        }
        async fn save_mission(&self, _: &Mission) -> Result<(), EngineError> {
            Ok(())
        }
        async fn load_mission(&self, _: MissionId) -> Result<Option<Mission>, EngineError> {
            Ok(None)
        }
        async fn list_missions(&self, _: ProjectId) -> Result<Vec<Mission>, EngineError> {
            Ok(vec![])
        }
        async fn update_mission_status(
            &self,
            _: MissionId,
            _: MissionStatus,
        ) -> Result<(), EngineError> {
            Ok(())
        }
    }

    // ── Helpers ────────────────────────────────────────────────

    fn make_lease() -> CapabilityLease {
        CapabilityLease {
            id: LeaseId::new(),
            thread_id: ThreadId::new(),
            capability_name: "test".into(),
            granted_actions: vec![],
            granted_at: Utc::now(),
            expires_at: None,
            max_uses: None,
            uses_remaining: None,
            revoked: false,
        }
    }

    fn make_ctx() -> ThreadExecutionContext {
        ThreadExecutionContext {
            thread_id: ThreadId::new(),
            thread_type: ThreadType::Reflection,
            project_id: ProjectId::new(),
            user_id: "test".into(),
            step_id: StepId::new(),
        }
    }

    fn make_capability(name: &str, actions: Vec<ActionDef>) -> Capability {
        Capability {
            name: name.into(),
            description: format!("{name} capability"),
            actions,
            knowledge: vec![],
            policies: vec![],
        }
    }

    fn make_action_def(name: &str) -> ActionDef {
        ActionDef {
            name: name.into(),
            description: format!("{name} action"),
            parameters_schema: serde_json::json!({"type": "object", "properties": {}}),
            effects: vec![EffectType::ReadLocal],
            requires_approval: false,
        }
    }

    // ── Tests ──────────────────────────────────────────────────

    #[tokio::test]
    async fn get_transcript_returns_content() {
        let transcript = "Step 1: called web_search\nStep 2: got results\nDone.";
        let project_id = ProjectId::new();
        let executor = ReflectionExecutor::new(
            MockStore::empty(),
            Arc::new(CapabilityRegistry::new()),
            transcript.to_string(),
            project_id,
        );

        let lease = make_lease();
        let ctx = make_ctx();
        let result = executor
            .execute_action("get_transcript", serde_json::json!({}), &lease, &ctx)
            .await
            .unwrap();

        assert_eq!(result.action_name, "get_transcript");
        assert!(!result.is_error);
        assert_eq!(result.output["transcript"].as_str().unwrap(), transcript);
    }

    #[tokio::test]
    async fn query_memory_finds_docs() {
        let project_id = ProjectId::new();
        let docs = vec![
            MemoryDoc::new(
                project_id,
                DocType::Lesson,
                "deployment error",
                "Fix: restart the service",
            ),
            MemoryDoc::new(
                project_id,
                DocType::Summary,
                "weather check",
                "Fetched weather data",
            ),
        ];
        let store = MockStore::new(docs);
        let executor = ReflectionExecutor::new(
            store,
            Arc::new(CapabilityRegistry::new()),
            String::new(),
            project_id,
        );

        let lease = make_lease();
        let ctx = make_ctx();
        let result = executor
            .execute_action(
                "query_memory",
                serde_json::json!({"query": "deployment error", "max_docs": 5}),
                &lease,
                &ctx,
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        let count = result.output["count"].as_u64().unwrap();
        assert!(count >= 1, "expected at least 1 doc, got {count}");

        let docs_arr = result.output["docs"].as_array().unwrap();
        // The deployment error doc should be present
        let has_deployment = docs_arr
            .iter()
            .any(|d| d["title"].as_str().unwrap().contains("deployment"));
        assert!(has_deployment, "expected deployment doc in results");
    }

    #[tokio::test]
    async fn check_tool_exists_found() {
        let mut registry = CapabilityRegistry::new();
        registry.register(make_capability(
            "search",
            vec![make_action_def("web-search")],
        ));

        let project_id = ProjectId::new();
        let executor = ReflectionExecutor::new(
            MockStore::empty(),
            Arc::new(registry),
            String::new(),
            project_id,
        );

        let lease = make_lease();
        let ctx = make_ctx();
        let result = executor
            .execute_action(
                "check_tool_exists",
                serde_json::json!({"name": "web-search"}),
                &lease,
                &ctx,
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.output["exists"], true);
        assert!(result.output["similar"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn check_tool_exists_not_found_suggests_similar() {
        let mut registry = CapabilityRegistry::new();
        registry.register(make_capability(
            "search",
            vec![make_action_def("web-search")],
        ));

        let project_id = ProjectId::new();
        let executor = ReflectionExecutor::new(
            MockStore::empty(),
            Arc::new(registry),
            String::new(),
            project_id,
        );

        let lease = make_lease();
        let ctx = make_ctx();
        let result = executor
            .execute_action(
                "check_tool_exists",
                serde_json::json!({"name": "web_search"}),
                &lease,
                &ctx,
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.output["exists"], false);
        let similar: Vec<String> = result.output["similar"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(
            similar.contains(&"web-search".to_string()),
            "expected 'web-search' in similar list, got: {similar:?}"
        );
    }

    #[tokio::test]
    async fn list_tools_returns_all() {
        let mut registry = CapabilityRegistry::new();
        registry.register(make_capability(
            "search",
            vec![
                make_action_def("web-search"),
                make_action_def("memory-search"),
            ],
        ));
        registry.register(make_capability("files", vec![make_action_def("read-file")]));

        let project_id = ProjectId::new();
        let executor = ReflectionExecutor::new(
            MockStore::empty(),
            Arc::new(registry),
            String::new(),
            project_id,
        );

        let lease = make_lease();
        let ctx = make_ctx();
        let result = executor
            .execute_action("list_tools", serde_json::json!({}), &lease, &ctx)
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.output["count"].as_u64().unwrap(), 3);
        let tools = result.output["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"web-search"));
        assert!(names.contains(&"memory-search"));
        assert!(names.contains(&"read-file"));
    }

    #[test]
    fn build_reflection_prompt_includes_tools() {
        let actions = vec![
            ActionDef {
                name: "get_transcript".into(),
                description: "Get the execution transcript".into(),
                parameters_schema: serde_json::json!({"type": "object", "properties": {}}),
                effects: vec![EffectType::ReadLocal],
                requires_approval: false,
            },
            ActionDef {
                name: "query_memory".into(),
                description: "Search memory docs".into(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"},
                        "max_docs": {"type": "integer"}
                    }
                }),
                effects: vec![EffectType::ReadLocal],
                requires_approval: false,
            },
        ];

        let prompt = build_reflection_prompt(&actions, "analyze deployment failure");
        assert!(
            prompt.contains("get_transcript"),
            "prompt should contain get_transcript tool name"
        );
        assert!(
            prompt.contains("query_memory"),
            "prompt should contain query_memory tool name"
        );
        assert!(
            prompt.contains("analyze deployment failure"),
            "prompt should contain the thread goal"
        );
        assert!(
            prompt.contains("Available tools"),
            "prompt should contain the tools section header"
        );
    }
}
