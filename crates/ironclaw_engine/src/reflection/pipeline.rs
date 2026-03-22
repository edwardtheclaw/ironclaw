//! Reflection pipeline — produces structured knowledge from completed threads.
//!
//! After a thread completes, the reflection pipeline uses the LLM to:
//! 1. Summarize what the thread accomplished
//! 2. Extract lessons from failures and workarounds
//! 3. Detect unresolved issues
//! 4. Identify missing capabilities
//!
//! Each produces a MemoryDoc stored in the thread's project scope.

use std::sync::Arc;

use tracing::debug;

use crate::traits::llm::{LlmBackend, LlmCallConfig};
use crate::types::error::EngineError;
use crate::types::event::EventKind;
use crate::types::memory::{DocType, MemoryDoc};
use crate::types::message::ThreadMessage;
use crate::types::step::{LlmResponse, TokenUsage};
use crate::types::thread::Thread;

/// Result of running the reflection pipeline on a completed thread.
pub struct ReflectionResult {
    /// Memory docs produced by reflection.
    pub docs: Vec<MemoryDoc>,
    /// Total tokens used by reflection LLM calls.
    pub tokens_used: TokenUsage,
}

/// Run the reflection pipeline on a completed thread.
///
/// Produces structured knowledge (MemoryDocs) from the thread's messages
/// and events. Uses the LLM for summarization and analysis.
pub async fn reflect(
    thread: &Thread,
    llm: &Arc<dyn LlmBackend>,
) -> Result<ReflectionResult, EngineError> {
    let mut docs = Vec::new();
    let mut total_tokens = TokenUsage::default();

    // Build a transcript of the thread's work for the LLM to analyze
    let transcript = build_transcript(thread);

    // 1. Summary doc
    let (summary_doc, tokens) =
        produce_doc(thread, llm, DocType::Summary, &transcript, SUMMARY_PROMPT).await?;
    docs.push(summary_doc);
    total_tokens.input_tokens += tokens.input_tokens;
    total_tokens.output_tokens += tokens.output_tokens;

    // 2. Lessons (only if there were errors)
    let had_errors = thread.events.iter().any(|e| {
        matches!(
            e.kind,
            EventKind::ActionFailed { .. } | EventKind::StepFailed { .. }
        )
    });
    if had_errors {
        let (lesson_doc, tokens) =
            produce_doc(thread, llm, DocType::Lesson, &transcript, LESSON_PROMPT).await?;
        docs.push(lesson_doc);
        total_tokens.input_tokens += tokens.input_tokens;
        total_tokens.output_tokens += tokens.output_tokens;
    }

    // 3. Issues (if thread failed or had unresolved problems)
    let thread_failed = thread.state == crate::types::thread::ThreadState::Failed;
    if thread_failed || had_errors {
        let (issue_doc, tokens) =
            produce_doc(thread, llm, DocType::Issue, &transcript, ISSUE_PROMPT).await?;
        // Only add if the LLM produced non-trivial content
        if issue_doc.content.len() > 20 {
            docs.push(issue_doc);
        }
        total_tokens.input_tokens += tokens.input_tokens;
        total_tokens.output_tokens += tokens.output_tokens;
    }

    debug!(
        thread_id = %thread.id,
        docs_produced = docs.len(),
        total_tokens = total_tokens.total(),
        "reflection complete"
    );

    Ok(ReflectionResult {
        docs,
        tokens_used: total_tokens,
    })
}

// ── Prompts ─────────────────────────────────────────────────

const SUMMARY_PROMPT: &str = "\
Summarize what this thread accomplished in 2-4 sentences. Include:
- The goal and whether it was achieved
- Key results or outputs
- Tools/actions that were used
Be factual and concise.";

const LESSON_PROMPT: &str = "\
Extract lessons learned from this thread's execution. Focus on:
- Errors encountered and how they were resolved (or not)
- Workarounds that were discovered
- Surprising findings about tool behavior
- Patterns that could be reused in similar tasks
Write each lesson as a single clear sentence. If there are no meaningful lessons, write 'No lessons.'.";

const ISSUE_PROMPT: &str = "\
Identify any unresolved issues from this thread. Focus on:
- Errors that were not resolved
- Tasks that could not be completed
- Missing tools or capabilities that were needed
- Data quality issues encountered
If there are no unresolved issues, write 'No issues.'.";

// ── Helpers ─────────────────────────────────────────────────

/// Build a concise transcript of the thread's work.
fn build_transcript(thread: &Thread) -> String {
    let mut parts = Vec::new();

    parts.push(format!("Goal: {}", thread.goal));
    parts.push(format!("Steps: {}", thread.step_count));
    parts.push(format!("Tokens used: {}", thread.total_tokens_used));
    parts.push(format!("State: {:?}", thread.state));

    // Include messages (truncated for very long threads)
    let max_messages = 30;
    let messages = if thread.messages.len() > max_messages {
        &thread.messages[thread.messages.len() - max_messages..]
    } else {
        &thread.messages
    };

    parts.push("\n--- Messages ---".into());
    for msg in messages {
        let role = format!("{:?}", msg.role);
        let content_preview: String = msg.content.chars().take(500).collect();
        let truncated = if msg.content.len() > 500 { "..." } else { "" };
        parts.push(format!("[{role}] {content_preview}{truncated}"));
    }

    // Include notable events
    let error_events: Vec<String> = thread
        .events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ActionFailed { action_name, error, .. } => {
                Some(format!("Action '{action_name}' failed: {error}"))
            }
            EventKind::StepFailed { error, .. } => Some(format!("Step failed: {error}")),
            _ => None,
        })
        .collect();

    if !error_events.is_empty() {
        parts.push("\n--- Errors ---".into());
        for err in error_events {
            parts.push(err);
        }
    }

    parts.join("\n")
}

/// Produce a single MemoryDoc by asking the LLM to analyze the transcript.
async fn produce_doc(
    thread: &Thread,
    llm: &Arc<dyn LlmBackend>,
    doc_type: DocType,
    transcript: &str,
    prompt: &str,
) -> Result<(MemoryDoc, TokenUsage), EngineError> {
    let messages = vec![
        ThreadMessage::system(format!(
            "You are analyzing a completed agent thread. Here is the transcript:\n\n{transcript}"
        )),
        ThreadMessage::user(prompt.to_string()),
    ];

    let config = LlmCallConfig {
        force_text: true,
        ..LlmCallConfig::default()
    };

    let output = llm.complete(&messages, &[], &config).await?;

    let content = match output.response {
        LlmResponse::Text(t) => t,
        LlmResponse::ActionCalls { content, .. } | LlmResponse::Code { content, .. } => {
            content.unwrap_or_default()
        }
    };

    let title = match doc_type {
        DocType::Summary => format!("Summary: {}", thread.goal),
        DocType::Lesson => format!("Lessons: {}", thread.goal),
        DocType::Issue => format!("Issues: {}", thread.goal),
        DocType::Playbook => format!("Playbook: {}", thread.goal),
        DocType::Spec => format!("Spec: {}", thread.goal),
        DocType::Note => format!("Note: {}", thread.goal),
    };

    let doc = MemoryDoc::new(thread.project_id, doc_type, title, content)
        .with_source_thread(thread.id);

    Ok((doc, output.usage))
}
