use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use tokio::sync::broadcast;

use crate::config::Config;
use crate::stt::SttProvider;

/// Capacity of the SSE broadcast ring-buffer (messages).
/// Subscribers that fall more than this many messages behind will receive a
/// `RecvError::Lagged` error (which the SSE handler maps to a dropped event).
const SSE_CHANNEL_CAPACITY: usize = 1024;

// ── Run state ──────────────────────────────────────────────────────────────

/// The current phase of an agent run.
#[derive(Debug, Clone)]
pub enum RunStatus {
    /// No run in progress; server is ready to accept a new prompt.
    Idle,
    /// An agent run is currently executing.
    Running,
    /// The run completed successfully.
    Done,
    /// The run failed or was killed. Holds a human-readable reason.
    Failed { reason: String },
}

impl RunStatus {
    pub fn is_running(&self) -> bool {
        matches!(self, RunStatus::Running)
    }

    pub fn label(&self) -> &'static str {
        match self {
            RunStatus::Idle => "Idle",
            RunStatus::Running => "Running",
            RunStatus::Done => "Done",
            RunStatus::Failed { .. } => "Failed",
        }
    }
}

/// Mutable state for the currently-active (or most-recent) agent run.
pub struct RunState {
    pub status: RunStatus,
    /// Buffered lines of agent stdout/stderr accumulated during the run,
    /// retained for display after the run completes.
    pub output_buf: Vec<String>,
}

impl Default for RunState {
    fn default() -> Self {
        Self {
            status: RunStatus::Idle,
            output_buf: Vec::new(),
        }
    }
}

// ── Conversation state ─────────────────────────────────────────────────────

/// A single ForgeCode conversation known to this server instance.
#[derive(Debug, Clone)]
pub struct ConvEntry {
    /// ForgeCode conversation ID (opaque string, typically a UUID).
    pub id: String,
    /// When the first run of this conversation was initiated.
    pub started_at: SystemTime,
    /// Short display label derived from the first prompt of the conversation.
    pub label: String,
    /// Number of agent runs completed under this conversation.
    pub run_count: u32,
}

impl ConvEntry {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            started_at: SystemTime::now(),
            label: label.into(),
            run_count: 0,
        }
    }

    /// A truncated form of the ID suitable for compact display.
    pub fn short_id(&self) -> &str {
        let s = self.id.as_str();
        &s[..s.len().min(8)]
    }
}

/// Conversation tracking across agent runs.
#[derive(Default)]
pub struct ConvState {
    /// ForgeCode conversation ID currently active.
    /// `None` only before the very first run ever on this server instance;
    /// always `Some` thereafter.
    pub active_id: Option<String>,
    /// All conversation entries seen by this server, most-recent last.
    pub history: Vec<ConvEntry>,
}

impl ConvState {
    /// Return the active `ConvEntry`, if any.
    pub fn active_entry(&self) -> Option<&ConvEntry> {
        let id = self.active_id.as_deref()?;
        self.history.iter().find(|e| e.id == id)
    }

    /// Return the active `ConvEntry` mutably, if any.
    pub fn active_entry_mut(&mut self) -> Option<&mut ConvEntry> {
        let id = self.active_id.as_deref()?.to_owned();
        self.history.iter_mut().find(|e| e.id == id)
    }
}

// ── Commit history ─────────────────────────────────────────────────────────

/// A single entry in the recent-commits list shown in the UI.
#[derive(Debug, Clone)]
pub struct CommitSummary {
    /// Full 40-character SHA.
    pub sha: String,
    /// First 8 characters of the SHA, for compact display.
    pub short_sha: String,
    /// First line of the commit message (the subject).
    pub subject: String,
}

impl CommitSummary {
    pub fn new(sha: impl Into<String>, subject: impl Into<String>) -> Self {
        let sha = sha.into();
        let short_sha = sha[..sha.len().min(8)].to_string();
        Self {
            sha,
            short_sha,
            subject: subject.into(),
        }
    }
}

// ── Application state ──────────────────────────────────────────────────────

/// Shared application state, wrapped in `Arc` and injected into every handler
/// via axum's `State` extractor.
pub struct AppState {
    /// Current run state. Held only briefly for reads/writes; never while
    /// waiting for a subprocess.
    pub run: Mutex<RunState>,
    /// Conversation tracking state.
    pub conv: Mutex<ConvState>,
    /// Prompts issued since the last successful `POST /commit`.
    /// Appended to the commit body when the user triggers a manual commit.
    pub prompts_since_commit: Mutex<Vec<String>>,
    /// Application configuration (read-only after startup).
    pub config: Config,
    /// Broadcast channel for SSE live output. The agent task sends one message
    /// per output line, plus `"__DONE__"` when the run ends. SSE subscribers
    /// forward messages to browser clients.
    pub sse_tx: broadcast::Sender<String>,
    /// Configured speech-to-text provider (built once at startup).
    pub stt: Arc<dyn SttProvider>,
    /// Recent git commits for the diff history nav, most-recent first.
    /// Populated from `git log` at startup and updated after each commit.
    pub commit_history: Mutex<Vec<CommitSummary>>,
}

impl AppState {
    pub fn new(config: Config) -> Arc<Self> {
        let (sse_tx, _) = broadcast::channel(SSE_CHANNEL_CAPACITY);
        let stt = crate::stt::build_provider(&config.stt);
        Arc::new(Self {
            run: Mutex::new(RunState::default()),
            conv: Mutex::new(ConvState::default()),
            prompts_since_commit: Mutex::new(Vec::new()),
            config,
            sse_tx,
            stt,
            commit_history: Mutex::new(Vec::new()),
        })
    }
}
