//! Configuration for IronClaw.
//!
//! Settings are loaded with priority: **DB/TOML > env > default**.
//!
//! DB and TOML are merged into a single `Settings` struct before
//! resolution (DB wins over TOML when both set the same field).
//! Resolvers then check settings before env vars.
//!
//! For concrete (non-`Option`) fields, a settings value equal to the
//! built-in default is treated as "unset" and falls through to env.
//!
//! Exceptions:
//! - Bootstrap configs (database, secrets): env-only (DB not yet available)
//! - Security-sensitive fields (allow_local_tools, allow_full_access,
//!   cost limits, auth tokens): env-only
//! - API keys: env/secrets store only
//!
//! `DATABASE_URL` lives in `~/.ironclaw/.env` (loaded via dotenvy early
//! in startup).

pub mod acp;
mod agent;
mod builder;
mod channels;
mod database;
mod ethereum;
pub(crate) mod embeddings;
mod heartbeat;
pub(crate) mod helpers;
mod hygiene;
pub(crate) mod llm;
pub mod relay;
mod routines;
mod safety;
mod sandbox;
mod search;
mod secrets;
mod skills;
mod transcription;
mod tunnel;
mod wasm;
pub(crate) mod workspace;

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, Once};

use crate::error::ConfigError;
use crate::settings::Settings;

// Re-export all public types so `crate::config::FooConfig` continues to work.
pub use self::agent::AgentConfig;
pub use self::builder::BuilderModeConfig;
pub use self::channels::{
    ChannelsConfig, CliConfig, DEFAULT_GATEWAY_PORT, GatewayConfig, GatewayOidcConfig, HttpConfig,
    SignalConfig,
};
pub use self::database::{DatabaseBackend, DatabaseConfig, SslMode, default_libsql_path};
pub use self::embeddings::{DEFAULT_EMBEDDING_CACHE_SIZE, EmbeddingsConfig};
pub use self::ethereum::EthereumConfig;
pub use self::heartbeat::HeartbeatConfig;
pub use self::hygiene::HygieneConfig;
pub use self::llm::default_session_path;
pub use self::relay::RelayConfig;
pub use self::routines::RoutineConfig;
pub use self::safety::SafetyConfig;
use self::safety::resolve_safety_config;
pub use self::sandbox::{AcpModeConfig, ClaudeCodeConfig, SandboxModeConfig};
pub use self::search::WorkspaceSearchConfig;
pub use self::secrets::SecretsConfig;
pub use self::skills::SkillsConfig;
pub use self::transcription::TranscriptionConfig;
pub use self::tunnel::TunnelConfig;
pub use self::wasm::WasmConfig;
pub use self::workspace::WorkspaceConfig;
pub use crate::llm::config::{
    BedrockConfig, CacheRetention, GeminiOauthConfig, LlmConfig, NearAiConfig, OAUTH_PLACEHOLDER,
    OpenAiCodexConfig, RegistryProviderConfig,
};
pub use crate::llm::session::SessionConfig;

// Thread-safe env var override helpers (replaces unsafe `std::env::set_var`
// for mid-process env mutations in multi-threaded contexts).
pub use self::helpers::{env_or_override, set_runtime_env};

/// Thread-safe overlay for injected env vars (secrets loaded from DB).
///
/// Used by `inject_llm_keys_from_secrets()` to make API keys available to
/// `optional_env()` without unsafe `set_var` calls. `optional_env()` checks
/// real env vars first, then falls back to this overlay.
///
/// Uses `Mutex<HashMap>` instead of `OnceLock` so that both
/// `inject_os_credentials()` and `inject_llm_keys_from_secrets()` can merge
/// their data. Whichever runs first initialises the map; the second merges in.
static INJECTED_VARS: LazyLock&lt;Mutex&lt;HashMap&lt;String, String&gt;&gt;&gt; =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static WARNED_EXPLICIT_DEFAULT_OWNER_ID: Once = Once::new();

/// Main configuration for the agent.
#[derive(Debug, Clone)]
pub struct Config {
    pub owner_id: String,
    pub database: DatabaseConfig,
    pub llm: LlmConfig,
    pub embeddings: EmbeddingsConfig,
    pub ethereum: EthereumConfig,
    pub tunnel: TunnelConfig,
    pub channels: ChannelsConfig,
    pub agent: AgentConfig,
    pub safety: SafetyConfig,
    pub wasm: WasmConfig,
    pub secrets: SecretsConfig,
    pub builder: BuilderModeConfig,
    pub heartbeat: HeartbeatConfig,
    pub hygiene: HygieneConfig,
    pub routines: RoutineConfig,
    pub sandbox: SandboxModeConfig,
    pub claude_code: ClaudeCodeConfig,
    pub acp: AcpModeConfig,
    pub skills: SkillsConfig,
    pub transcription: TranscriptionConfig,
    pub search: WorkspaceSearchConfig,
    pub workspace: WorkspaceConfig,
    pub observability: crate::observability::ObservabilityConfig,
    /// Channel-relay integration (Slack via external relay service).
    /// Present only when both `CHANNEL_RELAY_URL` and `CHANNEL_RELAY_API_KEY` are set.
    pub relay: Option&lt;RelayConfig&gt;,
}

impl Config {
    /// Create a full Config for integration tests without reading env vars.
    ///
    /// Requires the `libsql` feature. Sets up:
    /// - libSQL database at the given path
    /// - WASM and embeddings disabled
    /// - Skills enabled with the given directories
    /// - Heartbeat, routines, sandbox, builder all disabled
    /// - Safety with injection check off, 100k output limit
    #[cfg(feature = "libsql")]
    pub fn for_testing(
        libsql_path: std::path::PathBuf,
        skills_dir: std::path::PathBuf,
        installed_skills_dir: std::path::PathBuf,
    ) -> Self {
        Self {
            owner_id: "default".to_string(),