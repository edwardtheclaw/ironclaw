//! Hybrid store adapter — in-memory for ephemeral data, workspace for durable knowledge.
//!
//! Threads, steps, events, and leases are ephemeral (per-session).
//! MemoryDocs (lessons, specs, playbooks from reflection) persist to the
//! workspace so the engine learns across restarts.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::debug;

use ironclaw_engine::{
    CapabilityLease, DocId, DocType, EngineError, LeaseId, MemoryDoc, Project, ProjectId, Step,
    Store, Thread, ThreadEvent, ThreadId, ThreadState,
    types::mission::{Mission, MissionId, MissionStatus},
};

use crate::workspace::Workspace;

/// Workspace path prefix for engine memory docs.
const ENGINE_DOCS_PREFIX: &str = "engine/docs";

/// Hybrid store: in-memory for session data, workspace for durable knowledge.
pub struct HybridStore {
    // ── Ephemeral (in-memory, per-session) ──
    threads: RwLock<HashMap<ThreadId, Thread>>,
    steps: RwLock<HashMap<ThreadId, Vec<Step>>>,
    events: RwLock<HashMap<ThreadId, Vec<ThreadEvent>>>,
    projects: RwLock<HashMap<ProjectId, Project>>,
    leases: RwLock<HashMap<LeaseId, CapabilityLease>>,
    missions: RwLock<HashMap<MissionId, Mission>>,

    // ── Durable (workspace-backed, survives restarts) ──
    /// In-memory cache of docs (always in sync with workspace).
    docs: RwLock<HashMap<DocId, MemoryDoc>>,
    /// Workspace for persistent storage. None if workspace unavailable.
    workspace: Option<Arc<Workspace>>,
}

impl HybridStore {
    pub fn new(workspace: Option<Arc<Workspace>>) -> Self {
        Self {
            threads: RwLock::new(HashMap::new()),
            steps: RwLock::new(HashMap::new()),
            events: RwLock::new(HashMap::new()),
            projects: RwLock::new(HashMap::new()),
            leases: RwLock::new(HashMap::new()),
            missions: RwLock::new(HashMap::new()),
            docs: RwLock::new(HashMap::new()),
            workspace,
        }
    }

    /// Load existing docs from workspace on startup.
    pub async fn load_docs_from_workspace(&self) {
        let Some(ref ws) = self.workspace else {
            return;
        };

        // List all engine doc files
        let entries = match ws.list(ENGINE_DOCS_PREFIX).await {
            Ok(entries) => entries,
            Err(e) => {
                debug!("no engine docs in workspace: {e}");
                return;
            }
        };

        let mut loaded = 0;
        for entry in &entries {
            if entry.is_directory || !entry.path.ends_with(".json") {
                continue;
            }
            match ws.read(&entry.path).await {
                Ok(ws_doc) => {
                    if let Ok(doc) = serde_json::from_str::<MemoryDoc>(&ws_doc.content) {
                        self.docs.write().await.insert(doc.id, doc);
                        loaded += 1;
                    }
                }
                Err(e) => {
                    debug!(path = %entry.path, "failed to read engine doc: {e}");
                }
            }
        }

        if loaded > 0 {
            debug!(loaded, "loaded engine docs from workspace");
        }
    }

    /// Persist a MemoryDoc to workspace.
    async fn persist_doc(&self, doc: &MemoryDoc) {
        let Some(ref ws) = self.workspace else {
            return;
        };

        let path = doc_workspace_path(doc);
        let json = match serde_json::to_string_pretty(doc) {
            Ok(j) => j,
            Err(e) => {
                debug!("failed to serialize doc: {e}");
                return;
            }
        };

        if let Err(e) = ws.write(&path, &json).await {
            debug!(path = %path, "failed to persist engine doc: {e}");
        }
    }
}

/// Build workspace path for a MemoryDoc.
fn doc_workspace_path(doc: &MemoryDoc) -> String {
    let type_dir = match doc.doc_type {
        DocType::Summary => "summaries",
        DocType::Lesson => "lessons",
        DocType::Playbook => "playbooks",
        DocType::Issue => "issues",
        DocType::Spec => "specs",
        DocType::Note => "notes",
    };
    format!("{ENGINE_DOCS_PREFIX}/{type_dir}/{}.json", doc.id.0)
}

#[async_trait::async_trait]
impl Store for HybridStore {
    // ── Thread (ephemeral) ──────────────────────────────────

    async fn save_thread(&self, thread: &Thread) -> Result<(), EngineError> {
        self.threads.write().await.insert(thread.id, thread.clone());
        Ok(())
    }

    async fn load_thread(&self, id: ThreadId) -> Result<Option<Thread>, EngineError> {
        Ok(self.threads.read().await.get(&id).cloned())
    }

    async fn list_threads(&self, project_id: ProjectId) -> Result<Vec<Thread>, EngineError> {
        Ok(self
            .threads
            .read()
            .await
            .values()
            .filter(|t| t.project_id == project_id)
            .cloned()
            .collect())
    }

    async fn update_thread_state(
        &self,
        id: ThreadId,
        state: ThreadState,
    ) -> Result<(), EngineError> {
        if let Some(thread) = self.threads.write().await.get_mut(&id) {
            thread.state = state;
        }
        Ok(())
    }

    // ── Step (ephemeral) ────────────────────────────────────

    async fn save_step(&self, step: &Step) -> Result<(), EngineError> {
        self.steps
            .write()
            .await
            .entry(step.thread_id)
            .or_default()
            .push(step.clone());
        Ok(())
    }

    async fn load_steps(&self, thread_id: ThreadId) -> Result<Vec<Step>, EngineError> {
        Ok(self
            .steps
            .read()
            .await
            .get(&thread_id)
            .cloned()
            .unwrap_or_default())
    }

    // ── Event (ephemeral) ───────────────────────────────────

    async fn append_events(&self, events: &[ThreadEvent]) -> Result<(), EngineError> {
        let mut store = self.events.write().await;
        for event in events {
            store
                .entry(event.thread_id)
                .or_default()
                .push(event.clone());
        }
        Ok(())
    }

    async fn load_events(&self, thread_id: ThreadId) -> Result<Vec<ThreadEvent>, EngineError> {
        Ok(self
            .events
            .read()
            .await
            .get(&thread_id)
            .cloned()
            .unwrap_or_default())
    }

    // ── Project (ephemeral) ─────────────────────────────────

    async fn save_project(&self, project: &Project) -> Result<(), EngineError> {
        self.projects
            .write()
            .await
            .insert(project.id, project.clone());
        Ok(())
    }

    async fn load_project(&self, id: ProjectId) -> Result<Option<Project>, EngineError> {
        Ok(self.projects.read().await.get(&id).cloned())
    }

    // ── MemoryDoc (DURABLE — persisted to workspace) ────────

    async fn save_memory_doc(&self, doc: &MemoryDoc) -> Result<(), EngineError> {
        // Save to in-memory cache
        self.docs.write().await.insert(doc.id, doc.clone());
        // Persist to workspace
        self.persist_doc(doc).await;
        Ok(())
    }

    async fn load_memory_doc(&self, id: DocId) -> Result<Option<MemoryDoc>, EngineError> {
        Ok(self.docs.read().await.get(&id).cloned())
    }

    async fn list_memory_docs(&self, project_id: ProjectId) -> Result<Vec<MemoryDoc>, EngineError> {
        Ok(self
            .docs
            .read()
            .await
            .values()
            .filter(|d| d.project_id == project_id)
            .cloned()
            .collect())
    }

    // ── Lease (ephemeral) ───────────────────────────────────

    async fn save_lease(&self, lease: &CapabilityLease) -> Result<(), EngineError> {
        self.leases.write().await.insert(lease.id, lease.clone());
        Ok(())
    }

    async fn load_active_leases(
        &self,
        thread_id: ThreadId,
    ) -> Result<Vec<CapabilityLease>, EngineError> {
        Ok(self
            .leases
            .read()
            .await
            .values()
            .filter(|l| l.thread_id == thread_id && l.is_valid())
            .cloned()
            .collect())
    }

    async fn revoke_lease(&self, lease_id: LeaseId, _reason: &str) -> Result<(), EngineError> {
        if let Some(lease) = self.leases.write().await.get_mut(&lease_id) {
            lease.revoked = true;
        }
        Ok(())
    }

    // ── Mission (ephemeral) ──────────────────────────────────

    async fn save_mission(&self, mission: &Mission) -> Result<(), EngineError> {
        self.missions
            .write()
            .await
            .insert(mission.id, mission.clone());
        Ok(())
    }

    async fn load_mission(&self, id: MissionId) -> Result<Option<Mission>, EngineError> {
        Ok(self.missions.read().await.get(&id).cloned())
    }

    async fn list_missions(&self, project_id: ProjectId) -> Result<Vec<Mission>, EngineError> {
        Ok(self
            .missions
            .read()
            .await
            .values()
            .filter(|m| m.project_id == project_id)
            .cloned()
            .collect())
    }

    async fn update_mission_status(
        &self,
        id: MissionId,
        status: MissionStatus,
    ) -> Result<(), EngineError> {
        if let Some(mission) = self.missions.write().await.get_mut(&id) {
            mission.status = status;
        }
        Ok(())
    }
}
