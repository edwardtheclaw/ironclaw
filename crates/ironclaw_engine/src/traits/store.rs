//! Storage trait for engine persistence.
//!
//! Defines CRUD operations for all engine types. The main crate implements
//! this by wrapping its dual-backend `Database` trait (PostgreSQL + libSQL).

use crate::types::capability::{CapabilityLease, LeaseId};
use crate::types::conversation::{ConversationId, ConversationSurface};
use crate::types::error::EngineError;
use crate::types::event::ThreadEvent;
use crate::types::memory::{DocId, MemoryDoc};
use crate::types::mission::{Mission, MissionId, MissionStatus};
use crate::types::project::{Project, ProjectId};
use crate::types::step::Step;
use crate::types::thread::{Thread, ThreadId, ThreadState};

/// Persistence abstraction for the engine.
#[async_trait::async_trait]
pub trait Store: Send + Sync {
    // ── Thread operations ───────────────────────────────────

    async fn save_thread(&self, thread: &Thread) -> Result<(), EngineError>;
    async fn load_thread(&self, id: ThreadId) -> Result<Option<Thread>, EngineError>;
    async fn list_threads(
        &self,
        project_id: ProjectId,
        user_id: &str,
    ) -> Result<Vec<Thread>, EngineError>;
    async fn update_thread_state(
        &self,
        id: ThreadId,
        state: ThreadState,
    ) -> Result<(), EngineError>;

    // ── Step operations ─────────────────────────────────────

    async fn save_step(&self, step: &Step) -> Result<(), EngineError>;
    async fn load_steps(&self, thread_id: ThreadId) -> Result<Vec<Step>, EngineError>;

    // ── Event operations ────────────────────────────────────

    async fn append_events(&self, events: &[ThreadEvent]) -> Result<(), EngineError>;
    async fn load_events(&self, thread_id: ThreadId) -> Result<Vec<ThreadEvent>, EngineError>;

    // ── Project operations ──────────────────────────────────

    async fn save_project(&self, project: &Project) -> Result<(), EngineError>;
    async fn load_project(&self, id: ProjectId) -> Result<Option<Project>, EngineError>;
    async fn list_projects(&self, user_id: &str) -> Result<Vec<Project>, EngineError> {
        let _ = user_id;
        Ok(Vec::new())
    }

    // ── Conversation operations ─────────────────────────────

    async fn save_conversation(
        &self,
        conversation: &ConversationSurface,
    ) -> Result<(), EngineError> {
        let _ = conversation;
        Ok(())
    }
    async fn load_conversation(
        &self,
        id: ConversationId,
    ) -> Result<Option<ConversationSurface>, EngineError> {
        let _ = id;
        Ok(None)
    }
    async fn list_conversations(
        &self,
        user_id: &str,
    ) -> Result<Vec<ConversationSurface>, EngineError> {
        let _ = user_id;
        Ok(Vec::new())
    }

    // ── Memory doc operations ───────────────────────────────

    async fn save_memory_doc(&self, doc: &MemoryDoc) -> Result<(), EngineError>;
    async fn load_memory_doc(&self, id: DocId) -> Result<Option<MemoryDoc>, EngineError>;
    async fn list_memory_docs(
        &self,
        project_id: ProjectId,
        user_id: &str,
    ) -> Result<Vec<MemoryDoc>, EngineError>;

    /// List memory docs visible to a user: their own docs + shared "system" docs.
    ///
    /// This is the "shared space" pattern: admins can install skills and
    /// knowledge under `user_id="system"`, and they're visible to all users
    /// alongside their personal docs. Used for skill listing, context
    /// retrieval, and any place where shared knowledge should be accessible.
    async fn list_memory_docs_with_shared(
        &self,
        project_id: ProjectId,
        user_id: &str,
    ) -> Result<Vec<MemoryDoc>, EngineError> {
        if user_id == "system" {
            return self.list_memory_docs(project_id, "system").await;
        }
        let mut docs = self.list_memory_docs(project_id, user_id).await?;
        let system_docs = self.list_memory_docs(project_id, "system").await?;
        docs.extend(system_docs);
        Ok(docs)
    }

    // ── Capability lease operations ─────────────────────────

    async fn save_lease(&self, lease: &CapabilityLease) -> Result<(), EngineError>;
    async fn load_active_leases(
        &self,
        thread_id: ThreadId,
    ) -> Result<Vec<CapabilityLease>, EngineError>;
    async fn revoke_lease(&self, lease_id: LeaseId, reason: &str) -> Result<(), EngineError>;

    // ── Mission operations ───────────────────────────────────

    async fn save_mission(&self, mission: &Mission) -> Result<(), EngineError>;
    async fn load_mission(&self, id: MissionId) -> Result<Option<Mission>, EngineError>;
    async fn list_missions(
        &self,
        project_id: ProjectId,
        user_id: &str,
    ) -> Result<Vec<Mission>, EngineError>;
    async fn update_mission_status(
        &self,
        id: MissionId,
        status: MissionStatus,
    ) -> Result<(), EngineError>;

    /// List missions visible to a user: their own + shared "system" missions.
    ///
    /// System learning missions (self-improvement, skill-extraction, etc.) are
    /// created under `user_id="system"` and should be visible/manageable by all
    /// users through the API.
    async fn list_missions_with_shared(
        &self,
        project_id: ProjectId,
        user_id: &str,
    ) -> Result<Vec<Mission>, EngineError> {
        if user_id == "system" {
            return self.list_missions(project_id, "system").await;
        }
        let mut missions = self.list_missions(project_id, user_id).await?;
        let system = self.list_missions(project_id, "system").await?;
        missions.extend(system);
        Ok(missions)
    }

    // ── Admin operations (system-level, cross-tenant) ──────────

    /// List all threads in a project regardless of user.
    /// Used by: recovery, background thread resume at startup.
    async fn list_all_threads(&self, project_id: ProjectId) -> Result<Vec<Thread>, EngineError> {
        let _ = project_id;
        Ok(Vec::new())
    }

    /// List all missions in a project regardless of user.
    /// Used by: cron ticker, event listener, bootstrap.
    async fn list_all_missions(&self, project_id: ProjectId) -> Result<Vec<Mission>, EngineError> {
        let _ = project_id;
        Ok(Vec::new())
    }
}
