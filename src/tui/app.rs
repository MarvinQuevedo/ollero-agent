//! Application state and logic for the TUI.

use std::path::PathBuf;

use tokio::sync::mpsc;
use unicode_width::UnicodeWidthChar;

use crate::compression::{self, CompressionMode};
use crate::config::Config;
use crate::orchestra::types::FailurePolicy;
use crate::monitor::SharedMetrics;
use crate::ollama::client::OllamaClient;
use crate::ollama::types::{ChatOptions, Message, ToolCallItem};
use crate::permissions::{Decision, PermissionStore};
use crate::tools;
use crate::workspace;

use super::event::{self, AppEvent};
use super::widgets::chat_panel::ChatMessage;

// ── Constants ───────────────────────────────────────────────────────────────

const MAX_TOOL_ROUNDS: usize = 10000;

const SYSTEM_PROMPT: &str = "\
You are Allux, a local code assistant powered by Ollama. \
You help with software engineering tasks. \
You have access to tools: read_file, write_file, edit_file, glob, grep, tree, bash. \
Use them to explore and modify the codebase when needed. \
Always prefer reading files before editing them. \
Be concise and precise.";

const SYSTEM_PROMPT_CHAT_ONLY: &str = "\
You are Allux, a local code assistant. This session is in chat-only mode: \
Ollama does not expose tool calling for this model, so you cannot invoke tools yourself. \
The user can load disk context with slash commands. \
For shell steps, put each command in a fenced block with language bash or sh. \
Be concise.";

// ── Session mode ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionMode {
    Chat,
    Agent,
    Plan,
    Orchestra,
}

impl SessionMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Chat      => "chat",
            Self::Agent     => "agent",
            Self::Plan      => "plan",
            Self::Orchestra => "orchestra",
        }
    }
}

// ── Agent phase (state machine) ─────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum AgentPhase {
    /// Waiting for user input.
    Idle,
    /// Waiting for the LLM to respond.
    WaitingForLlm,
    /// Asking the user for permission (bash, edit, write).
    #[allow(dead_code)]
    WaitingForPermission {
        tool_name: String,
        command: String,
        /// Index in the current tool calls batch.
        call_index: usize,
        /// The full batch of tool calls.
        pending_calls: Vec<ToolCallItem>,
        /// Results accumulated so far.
        results: Vec<Message>,
    },
    /// Executing tool calls.
    ExecutingTools,
}

// ── Permission modal state ──────────────────────────────────────────────────

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct PermissionPrompt {
    pub tool_name: String,
    pub command: String,
    pub detail: String,
    pub options: Vec<(&'static str, &'static str)>,
}

// ── App ─────────────────────────────────────────────────────────────────────

pub struct App {
    // ── Core state ──
    pub client: OllamaClient,
    pub history: Vec<Message>,
    pub config: Config,
    pub workspace_root: PathBuf,
    pub mode: SessionMode,
    pub model_supports_tools: bool,
    pub verbose_tools: bool,
    pub compression_mode: CompressionMode,
    pub permissions: PermissionStore,
    pub metrics: SharedMetrics,
    pub session_id: Option<String>,
    pub orchestra_policy: FailurePolicy,

    // ── UI state ──
    pub chat_messages: Vec<ChatMessage>,
    pub scroll_offset: usize,
    /// When true, new streaming chunks auto-scroll to bottom.
    /// Disabled when the user scrolls up; re-enabled on scroll_to_bottom().
    pub auto_scroll: bool,
    pub phase: AgentPhase,
    pub spinner_frame: usize,
    pub should_quit: bool,
    pub status_message: Option<String>,
    /// Tick counter for auto-clearing transient status messages.
    pub status_msg_ticks: u8,
    /// Ctrl+C was pressed once; waiting for second press or timeout to clear.
    pub ctrl_c_pending: bool,
    pub ctrl_c_tick_count: u8,
    pub permission_prompt: Option<PermissionPrompt>,

    // ── Text selection ──
    /// Start of text selection (row, col) in rendered chat lines.
    pub selection_start: Option<(usize, u16)>,
    /// End of text selection (row, col) in rendered chat lines.
    pub selection_end: Option<(usize, u16)>,
    /// Whether user is currently dragging to select.
    pub selecting: bool,
    /// The chat area rect (set each frame for mouse coordinate mapping).
    pub chat_area: ratatui::layout::Rect,
    /// First absolute line visible in the chat panel (set each frame).
    pub chat_view_start: usize,
    /// Total number of rendered lines in the chat (set each frame).
    pub chat_total_lines: usize,
    /// Last click time + position for double-click detection.
    pub last_click: Option<(std::time::Instant, u16, u16)>,

    // ── Streaming ──
    pub streaming_text: String,
    pub current_tool_round: usize,
    /// Input queued while LLM is busy. Sent automatically when phase goes Idle.
    pub queued_input: Option<String>,

    // ── Event channel (for sending events from tool execution, etc.) ──
    pub event_tx: mpsc::UnboundedSender<AppEvent>,

    // ── Orchestra mode ──
    pub orchestra_run_id: Option<String>,
    pub orchestra_events_rx: Option<mpsc::UnboundedReceiver<crate::orchestra::DriverEvent>>,
    pub orchestra_handle: Option<tokio::task::JoinHandle<anyhow::Result<crate::orchestra::types::FinalReport>>>,
    pub pending_escalation: Option<PendingEscalation>,
}

/// Holds the context of a worker task that needs a human decision.
pub struct PendingEscalation {
    pub task_id: crate::orchestra::types::TaskId,
    pub reason: String,
    pub report: crate::orchestra::types::ValidationReport,
}

impl App {
    pub fn new(
        config: Config,
        workspace_root: PathBuf,
        metrics: SharedMetrics,
        event_tx: mpsc::UnboundedSender<AppEvent>,
    ) -> Self {
        let client = OllamaClient::new(&config.ollama_url, &config.model);
        let compression_mode = CompressionMode::from_str_loose(&config.compression_mode)
            .unwrap_or(CompressionMode::Auto);
        let orchestra_policy = FailurePolicy::from_str_loose(&config.orchestra_policy)
            .unwrap_or(FailurePolicy::Interactive);

        let system_prompt = Self::compose_system_prompt(&workspace_root, &SessionMode::Agent, true);
        let history = vec![Message::system(system_prompt)];

        Self {
            client,
            history,
            config,
            workspace_root: workspace_root.clone(),
            mode: SessionMode::Agent,
            model_supports_tools: true,
            verbose_tools: false,
            compression_mode,
            permissions: PermissionStore::new(&workspace_root),
            metrics,
            session_id: None,
            orchestra_policy,

            chat_messages: Vec::new(),
            scroll_offset: 0,
            auto_scroll: true,
            phase: AgentPhase::Idle,
            spinner_frame: 0,
            should_quit: false,
            status_message: None,
            status_msg_ticks: 0,
            ctrl_c_pending: false,
            ctrl_c_tick_count: 0,
            permission_prompt: None,

            selection_start: None,
            selection_end: None,
            selecting: false,
            chat_area: ratatui::layout::Rect::default(),
            chat_view_start: 0,
            chat_total_lines: 0,
            last_click: None,

            streaming_text: String::new(),
            current_tool_round: 0,
            queued_input: None,

            event_tx,

            orchestra_run_id: None,
            orchestra_events_rx: None,
            orchestra_handle: None,
            pending_escalation: None,
        }
    }

    // ── System prompt ───────────────────────────────────────────────────────

    fn compose_system_prompt(
        root: &std::path::Path,
        mode: &SessionMode,
        model_supports_tools: bool,
    ) -> String {
        let intro = match mode {
            SessionMode::Chat => SYSTEM_PROMPT_CHAT_ONLY,
            SessionMode::Agent | SessionMode::Plan | SessionMode::Orchestra => {
                if model_supports_tools {
                    SYSTEM_PROMPT
                } else {
                    SYSTEM_PROMPT_CHAT_ONLY
                }
            }
        };
        format!("{intro}\n\n{}", workspace::snapshot(root))
    }

    pub fn rebuild_system_prompt(&mut self) {
        let content = Self::compose_system_prompt(
            &self.workspace_root,
            &self.mode,
            self.model_supports_tools,
        );
        if let Some(first) = self.history.first_mut() {
            if first.role == "system" {
                first.content = content;
                return;
            }
        }
        self.history.insert(0, Message::system(content));
    }

    // ── Context tracking ────────────────────────────────────────────────────

    pub fn history_char_count(&self) -> usize {
        self.history.iter().map(|m| m.content.len()).sum()
    }

    pub fn context_pct(&self) -> f64 {
        let budget = (self.config.context_size as usize) * 3;
        if budget == 0 {
            return 0.0;
        }
        ((self.history_char_count() as f64 / budget as f64) * 100.0).min(100.0)
    }

    // ── Submit user input ───────────────────────────────────────────────────

    pub fn submit_user_input(&mut self, input: String) {
        if input.is_empty() {
            return;
        }

        // Orchestra mode: treat input as a goal or escalation decision
        if self.mode == SessionMode::Orchestra {
            self.chat_messages.push(ChatMessage::User(input.clone()));
            if self.orchestra_handle.is_none() {
                self.start_orchestra_run(input);
            } else {
                // User typed something while a run is active — ignore or queue hint
                self.chat_messages.push(ChatMessage::System(
                    "Orchestra run in progress. Use /orchestra cancel to stop.".into(),
                ));
            }
            return;
        }

        // Add to chat display
        self.chat_messages.push(ChatMessage::User(input.clone()));

        // Add to LLM history
        self.history.push(Message::user(&input));

        // Start the LLM call
        self.start_llm_call();
    }

    /// Queue input to be sent after the current operation finishes.
    pub fn enqueue_input(&mut self, input: String) {
        self.chat_messages
            .push(ChatMessage::System(format!("Queued: {input}")));
        self.queued_input = Some(input);
        self.scroll_to_bottom();
    }

    /// If there's queued input and we're idle, dispatch it.
    fn flush_queued_input(&mut self) {
        if self.phase == AgentPhase::Idle {
            if let Some(input) = self.queued_input.take() {
                self.submit_user_input(input);
            }
        }
    }

    pub fn start_llm_call(&mut self) {
        self.phase = AgentPhase::WaitingForLlm;
        self.streaming_text.clear();
        self.spinner_frame = 0;

        let tools_defs = tools::all_definitions();
        let use_tools = matches!(self.mode, SessionMode::Agent | SessionMode::Plan)
            && self.model_supports_tools;

        let client = self.client.clone();
        let history = self.history.clone();
        let options = ChatOptions {
            temperature: None,
            num_ctx: Some(self.config.context_size),
        };
        let event_tx = self.event_tx.clone();

        tokio::spawn(async move {
            let (stream_tx, stream_rx) = mpsc::unbounded_channel();

            // Forward stream events to the app event channel
            event::forward_stream_events(stream_rx, event_tx);

            if use_tools {
                client
                    .chat_streaming(&history, Some(&tools_defs), Some(options), stream_tx)
                    .await;
            } else {
                client
                    .chat_streaming(&history, None, Some(options), stream_tx)
                    .await;
            }
        });
    }

    // ── Handle stream events ────────────────────────────────────────────────

    pub fn on_stream_chunk(&mut self, text: String) {
        self.streaming_text.push_str(&text);
        // Only auto-scroll if user hasn't scrolled up
        if self.auto_scroll {
            self.scroll_offset = 0;
        }
    }

    pub fn on_stream_done(&mut self, content: String, prompt_tokens: u32, completion_tokens: u32) {
        self.phase = AgentPhase::Idle;
        self.history.push(Message::assistant(&content));
        self.chat_messages.push(ChatMessage::Assistant(content));
        self.streaming_text.clear();
        self.status_message = Some(format!(
            "tokens: {} in · {} out",
            prompt_tokens, completion_tokens
        ));
        self.scroll_to_bottom();
        self.flush_queued_input();
    }

    pub fn on_stream_tool_calls(
        &mut self,
        calls: Vec<ToolCallItem>,
        text: String,
        _prompt_tokens: u32,
        _completion_tokens: u32,
    ) {
        self.streaming_text.clear();

        // Show tool calls in chat
        let names: Vec<String> = calls
            .iter()
            .map(|c| {
                let detail = tool_call_detail(&c.function.name, &c.function.arguments);
                if detail.is_empty() {
                    format!("  {} {}", "\u{26A1}", c.function.name)
                } else {
                    format!("  {} {} {}", "\u{26A1}", c.function.name, detail)
                }
            })
            .collect();
        self.chat_messages
            .push(ChatMessage::ToolHeader(names.join("\n")));

        // Store in history
        self.history
            .push(Message::assistant_tool_calls(calls.clone(), &text));

        // Execute tool calls
        self.execute_tool_calls(calls);
    }

    pub fn on_stream_error(&mut self, error: String) {
        // Check for async system messages piggybacking on stream error channel
        if error.starts_with("MODELS:\n") || error.starts_with("Model unloaded") {
            let msg = error.strip_prefix("MODELS:\n").unwrap_or(&error);
            self.chat_messages.push(ChatMessage::System(msg.to_string()));
            self.scroll_to_bottom();
            return;
        }

        self.phase = AgentPhase::Idle;
        self.streaming_text.clear();

        // Check if model doesn't support tools
        if self.model_supports_tools && error.contains("does not support tools") {
            self.model_supports_tools = false;
            self.rebuild_system_prompt();
            self.chat_messages.push(ChatMessage::System(format!(
                "Model '{}' does not support tools. Falling back to chat mode.",
                self.client.model
            )));
            // Retry without tools
            self.start_llm_call();
            return;
        }

        self.chat_messages
            .push(ChatMessage::Error(format!("Error: {}", error)));
        // Remove the failed user message from history
        if self.history.last().map(|m| m.role.as_str()) == Some("user") {
            self.history.pop();
        }
        self.flush_queued_input();
    }

    // ── Tool execution ──────────────────────────────────────────────────────

    fn execute_tool_calls(&mut self, calls: Vec<ToolCallItem>) {
        self.phase = AgentPhase::ExecutingTools;
        self.current_tool_round += 1;

        let event_tx = self.event_tx.clone();

        tokio::spawn(async move {
            for call in &calls {
                let name = &call.function.name;
                let args = &call.function.arguments;

                // Execute tool (permissions are checked in the TUI layer for bash/edit)
                let output = match tools::dispatch(name, args, true).await {
                    Ok(out) => out,
                    Err(e) => format!("Error executing {name}: {e}"),
                };

                let _ = event_tx.send(AppEvent::ToolResult {
                    name: name.clone(),
                    output: output.clone(),
                });
            }
        });
    }

    pub fn on_tool_result(&mut self, name: String, output: String) {
        // Add tool result to history
        self.history
            .push(Message::tool_result(name.clone(), output.clone()));

        // Show compact result in chat (single line, no embedded newlines)
        let first_line = output
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();
        let preview = if first_line.len() > 120 {
            format!("{}…", &first_line[..120])
        } else {
            first_line.to_string()
        };
        self.chat_messages
            .push(ChatMessage::ToolResult(name, preview));

        // Check if this was the last tool result for this batch
        // For simplicity, we'll start a new LLM call after each tool result
        // In production, we'd batch all results first
        // Start new LLM round
        if self.current_tool_round < MAX_TOOL_ROUNDS {
            self.start_llm_call();
        }
        self.scroll_to_bottom();
    }

    // ── Permission handling ─────────────────────────────────────────────────

    pub fn handle_permission_response(&mut self, decision: Decision) {
        if let Some(prompt) = self.permission_prompt.take() {
            match decision {
                Decision::AllowOnce => {}
                Decision::AllowSession => {
                    self.permissions.grant_session(&prompt.command);
                }
                Decision::AllowFamily => {
                    self.permissions.grant_family(&prompt.command);
                }
                Decision::AllowWorkspace => {
                    self.permissions.grant_workspace(&prompt.command);
                }
                Decision::AllowGlobal => {
                    self.permissions.grant_global(&prompt.command);
                }
                Decision::Deny => {
                    self.chat_messages
                        .push(ChatMessage::System("Permission denied.".into()));
                    return;
                }
            }
        }
    }

    // ── Scroll ──────────────────────────────────────────────────────────────

    pub fn scroll_up(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(amount);
        self.auto_scroll = false; // user is reading history
    }

    pub fn scroll_down(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
        if self.scroll_offset == 0 {
            self.auto_scroll = true; // back at bottom
        }
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
        self.auto_scroll = true;
    }

    // ── Slash commands ──────────────────────────────────────────────────────

    pub fn handle_slash_command(&mut self, input: &str) -> bool {
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return false;
        }

        let (cmd, rest) = match trimmed.find(char::is_whitespace) {
            Some(pos) => (&trimmed[..pos], trimmed[pos..].trim()),
            None => (trimmed, ""),
        };

        match cmd {
            "/quit" | "/exit" | "/q" => {
                self.should_quit = true;
            }
            "/help" | "/?" => {
                self.chat_messages.push(ChatMessage::System(
                    "Ctrl+D             — exit when input is empty\n\
                     /quit /exit /q     — exit anytime\n\
                     /help              — this message\n\
                     /clear             — reset conversation\n\
                     /history           — show conversation history\n\
                     /context           — print workspace snapshot\n\
                     /context refresh   — rescan cwd and update system context\n\
                     /model             — show current model\n\
                     /model list        — list available models\n\
                     /model <name>      — switch model\n\
                     /mode              — show current mode (chat / agent / plan)\n\
                     /mode chat         — chat only, no tools\n\
                     /mode agent        — autonomous tool use (default)\n\
                     /mode plan         — show a numbered plan before executing\n\
                     /mode orchestra    — multi-step structured execution\n\
                     /orchestra         — orchestra commands (list/resume/cancel)\n\
                     /policy <name>     — set orchestra failure policy (interactive/autonomous)\n\
                     /retry [hint]      — retry escalated task (with optional hint)\n\
                     /skip              — skip escalated task and continue\n\
                     /abort             — abort escalated task or entire run\n\
                     /verbose           — toggle compact/verbose tool call log\n\
                     /save              — save current session to disk\n\
                     /sessions          — list saved sessions\n\
                     /resume <id>       — resume a saved session\n\
                     /read <path>       — read a file and add it to context\n\
                     /glob <pat> [dir]  — list matching paths, add to context\n\
                     /tree [path] [n]   — directory tree (default . depth 3)\n\
                     /compress          — show compression mode and stats\n\
                     /compress always   — compress all tool outputs immediately\n\
                     /compress auto     — compress when approaching limit (default)\n\
                     /compress manual   — no auto-compression\n\
                     /compress now      — manually compress history\n\
                     /compress ai       — LLM-powered semantic summary\n\
                     /unload            — unload model from VRAM/RAM\n\n\
                     Actions (expert prompts sent to LLM):\n\
                     /commit            — auto-commit with smart message\n\
                     /review            — code review of recent changes\n\
                     /fix               — find and fix build errors\n\
                     /test              — run tests and fix failures\n\
                     /refactor <file>   — refactor a file\n\
                     /explain <file>    — explain a file in detail\n\
                     /find <desc>       — find code by description\n\
                     /todo              — list all TODOs/FIXMEs\n\
                     /deps              — analyze project dependencies\n\
                     /doc <file>        — generate documentation\n\
                     /scaffold <t> <n>  — scaffold a new component\n\
                     /changelog         — generate changelog from git\n\
                     /doctor            — diagnose project health\n\
                     /perf <file>       — analyze performance\n\
                     /security          — security audit\n\n\
                     Scroll: PageUp/PageDown, mouse wheel\n\
                     Copy:   Esc to toggle copy mode\n\
                     Submit: Enter | Ctrl+D exit"
                        .into(),
                ));
            }
            "/clear" => {
                self.history = vec![Message::system(Self::compose_system_prompt(
                    &self.workspace_root,
                    &self.mode,
                    self.model_supports_tools,
                ))];
                self.chat_messages.clear();
                self.chat_messages
                    .push(ChatMessage::System("Conversation cleared.".into()));
            }
            "/history" => {
                let lines: Vec<String> = self
                    .history
                    .iter()
                    .filter(|m| m.role != "system")
                    .map(|m| {
                        let preview = &m.content[..m.content.len().min(80)];
                        format!("[{}] {}", m.role, preview)
                    })
                    .collect();
                if lines.is_empty() {
                    self.chat_messages
                        .push(ChatMessage::System("No messages yet.".into()));
                } else {
                    self.chat_messages
                        .push(ChatMessage::System(lines.join("\n")));
                }
            }
            "/context" => {
                if rest.is_empty() || rest == "show" {
                    let snapshot = workspace::snapshot(&self.workspace_root);
                    self.chat_messages.push(ChatMessage::System(format!(
                        "Workspace: {}\n{}",
                        self.workspace_root.display(),
                        snapshot
                    )));
                } else if rest == "refresh" {
                    self.workspace_root =
                        std::env::current_dir().unwrap_or(self.workspace_root.clone());
                    self.rebuild_system_prompt();
                    self.chat_messages.push(ChatMessage::System(
                        "Workspace context refreshed.".into(),
                    ));
                } else {
                    self.chat_messages.push(ChatMessage::System(
                        "Usage: /context or /context refresh".into(),
                    ));
                }
            }
            "/model" => {
                if rest.is_empty() {
                    self.chat_messages.push(ChatMessage::System(format!(
                        "Current model: {}",
                        self.client.model
                    )));
                } else if rest == "list" {
                    // Async model list
                    let base_url = self.client.base_url().to_string();
                    let tx = self.event_tx.clone();
                    tokio::spawn(async move {
                        match OllamaClient::list_models(&base_url).await {
                            Ok(models) => {
                                let list: String = models
                                    .iter()
                                    .map(|m| {
                                        format!(
                                            "  {} ({} {})",
                                            m.name,
                                            m.details.parameter_size,
                                            m.details.quantization_level
                                        )
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                // Send as a stream error to show as system message
                                let _ = tx.send(AppEvent::StreamError(format!(
                                    "MODELS:\n{list}"
                                )));
                            }
                            Err(e) => {
                                let _ = tx.send(AppEvent::StreamError(format!(
                                    "Error listing models: {e}"
                                )));
                            }
                        }
                    });
                    self.chat_messages
                        .push(ChatMessage::System("Loading model list...".into()));
                } else {
                    self.config.model = rest.to_string();
                    self.client.model = rest.to_string();
                    self.model_supports_tools = true;
                    self.rebuild_system_prompt();
                    let _ = self.config.save();
                    self.chat_messages.push(ChatMessage::System(format!(
                        "Model set to: {}",
                        rest
                    )));
                }
            }
            "/mode" => {
                if rest.is_empty() {
                    self.chat_messages.push(ChatMessage::System(format!(
                        "Current mode: {}\n  chat       — no tools, pure conversation\n  agent      — autonomous tool use (default)\n  plan       — show numbered plan first\n  orchestra  — multi-step structured execution",
                        self.mode.label()
                    )));
                } else {
                    let new_mode = match rest {
                        "chat"      => Some(SessionMode::Chat),
                        "agent"     => Some(SessionMode::Agent),
                        "plan"      => Some(SessionMode::Plan),
                        "orchestra" => Some(SessionMode::Orchestra),
                        _ => None,
                    };
                    if let Some(m) = new_mode {
                        self.mode = m;
                        self.rebuild_system_prompt();
                        self.chat_messages.push(ChatMessage::System(format!(
                            "Mode set to: {}",
                            self.mode.label()
                        )));
                    } else {
                        self.chat_messages.push(ChatMessage::System(format!(
                            "Unknown mode: {rest}. Use chat, agent, plan, or orchestra."
                        )));
                    }
                }
            }
            "/orchestra" => {
                match rest {
                    "" => {
                        self.chat_messages.push(ChatMessage::System(
                            "Orchestra mode — multi-step structured execution.\n\
                             Usage:\n\
                             /orchestra <goal>         — start a new run\n\
                             /orchestra list           — list past runs\n\
                             /orchestra resume <id>    — resume a paused run\n\
                             /orchestra cancel         — cancel the current run\n\
                             Switch mode with /mode orchestra, then type your goal."
                                .into(),
                        ));
                    }
                    "list" => {
                        let workspace = std::env::current_dir().unwrap_or_default();
                        match crate::orchestra::list_runs(&workspace) {
                            Ok(runs) if runs.is_empty() => {
                                self.chat_messages.push(ChatMessage::System(
                                    "No saved Orchestra runs found.".into(),
                                ));
                            }
                            Ok(runs) => {
                                let mut lines = vec![format!("Orchestra runs ({} total):", runs.len())];
                                for r in runs.iter().take(10) {
                                    let status = format!("{:?}", r.status);
                                    lines.push(format!(
                                        "  {}  [{}]  \"{}\"",
                                        r.run_id, status, r.goal
                                    ));
                                }
                                lines.push(String::new());
                                lines.push("Use /orchestra resume <id> to continue.".into());
                                self.chat_messages.push(ChatMessage::System(lines.join("\n")));
                            }
                            Err(e) => {
                                self.chat_messages.push(ChatMessage::Error(format!("{e}")));
                            }
                        }
                    }
                    _ if rest.starts_with("resume ") => {
                        let run_id = rest.trim_start_matches("resume ").trim().to_string();
                        if run_id.is_empty() {
                            self.chat_messages.push(ChatMessage::System(
                                "Usage: /orchestra resume <run-id>".into(),
                            ));
                        } else if self.orchestra_handle.is_some() {
                            self.chat_messages.push(ChatMessage::System(
                                "A run is already active. Use /orchestra cancel first.".into(),
                            ));
                        } else {
                            self.resume_orchestra_run(run_id);
                        }
                    }
                    "cancel" => {
                        if let Some(handle) = self.orchestra_handle.take() {
                            handle.abort();
                            self.orchestra_events_rx = None;
                            self.pending_escalation = None;
                            self.chat_messages.push(ChatMessage::System(
                                "Orchestra run cancelled.".into(),
                            ));
                        } else {
                            self.chat_messages.push(ChatMessage::System(
                                "No active Orchestra run to cancel.".into(),
                            ));
                        }
                    }
                    _ => {
                        // Treat the rest as a goal to run directly
                        let goal = rest.to_string();
                        self.start_orchestra_run(goal);
                    }
                }
            }
            "/policy" => {
                match rest {
                    "" => {
                        self.chat_messages.push(ChatMessage::System(format!(
                            "Orchestra failure policy: {}\n\
                             /policy interactive — escalate to user on task failure\n\
                             /policy autonomous  — defer failures and continue",
                            self.orchestra_policy.label()
                        )));
                    }
                    _ => {
                        if let Some(policy) = FailurePolicy::from_str_loose(rest) {
                            self.orchestra_policy = policy;
                            self.config.orchestra_policy = policy.label().to_string();
                            let _ = self.config.save();
                            self.chat_messages.push(ChatMessage::System(format!(
                                "Orchestra policy set to: {}",
                                policy.label()
                            )));
                        } else {
                            self.chat_messages.push(ChatMessage::System(format!(
                                "Unknown policy: {rest}. Use interactive or autonomous."
                            )));
                        }
                    }
                }
            }
            "/verbose" => {
                self.verbose_tools = !self.verbose_tools;
                self.chat_messages.push(ChatMessage::System(format!(
                    "Verbose tool log: {}",
                    if self.verbose_tools { "ON" } else { "OFF (compact)" }
                )));
            }
            "/read" => {
                if rest.is_empty() {
                    self.chat_messages
                        .push(ChatMessage::System("Usage: /read <path>".into()));
                } else {
                    match tools::run_read_file(rest) {
                        Ok(text) => {
                            let inject = format!(
                                "Contents of `{rest}` (loaded with /read):\n\n{text}"
                            );
                            self.history.push(Message::user(inject));
                            let preview = if text.len() > 500 {
                                format!("{}...", &text[..500])
                            } else {
                                text
                            };
                            self.chat_messages.push(ChatMessage::System(format!(
                                "── {rest} ── (added to context)\n{preview}"
                            )));
                        }
                        Err(e) => {
                            self.chat_messages
                                .push(ChatMessage::Error(format!("{e}")));
                        }
                    }
                }
            }
            "/glob" => {
                if rest.is_empty() {
                    self.chat_messages
                        .push(ChatMessage::System("Usage: /glob <pattern> [dir]".into()));
                } else {
                    let parts: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
                    let pattern = parts[0];
                    let dir = parts.get(1).map(|s| s.trim());
                    match tools::run_glob(pattern, dir) {
                        Ok(text) => {
                            let inject = format!(
                                "Glob `{pattern}`{}:\n\n{text}",
                                dir.map(|d| format!(" in `{d}`")).unwrap_or_default()
                            );
                            self.history.push(Message::user(inject));
                            self.chat_messages.push(ChatMessage::System(format!(
                                "{text}\n(added to context)"
                            )));
                        }
                        Err(e) => {
                            self.chat_messages
                                .push(ChatMessage::Error(format!("{e}")));
                        }
                    }
                }
            }
            "/tree" => {
                let parts: Vec<&str> = rest.split_whitespace().collect();
                let path = if parts.is_empty() { "." } else { parts[0] };
                let depth: usize = parts
                    .get(1)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(3)
                    .clamp(1, 20);
                match tools::run_tree(path, depth) {
                    Ok(text) => {
                        let inject = format!(
                            "Directory tree for `{path}` (depth {depth}):\n\n{text}"
                        );
                        self.history.push(Message::user(inject));
                        self.chat_messages.push(ChatMessage::System(format!(
                            "{text}\n(added to context)"
                        )));
                    }
                    Err(e) => {
                        self.chat_messages
                            .push(ChatMessage::Error(format!("{e}")));
                    }
                }
            }
            "/save" => {
                match crate::session::save(
                    &self.history,
                    &self.client.model,
                    &self.workspace_root,
                    self.session_id.as_deref(),
                ) {
                    Ok(path) => {
                        let id = path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("unknown")
                            .to_string();
                        self.session_id = Some(id.clone());
                        self.chat_messages.push(ChatMessage::System(format!(
                            "Session saved (id: {id})"
                        )));
                    }
                    Err(e) => {
                        self.chat_messages
                            .push(ChatMessage::Error(format!("Error saving: {e}")));
                    }
                }
            }
            "/sessions" => match crate::session::list() {
                Ok(sessions) if sessions.is_empty() => {
                    self.chat_messages
                        .push(ChatMessage::System("No saved sessions found.".into()));
                }
                Ok(sessions) => {
                    let mut lines = Vec::new();
                    lines.push(format!("Saved sessions ({} total):", sessions.len()));
                    lines.push(String::new());
                    for (i, s) in sessions.iter().take(15).enumerate() {
                        lines.push(format!(
                            "  {:<3} ID: {}  \"{}\"  ({}, {} msgs)",
                            i + 1, s.id, s.name, s.model, s.message_count
                        ));
                    }
                    if sessions.len() > 15 {
                        lines.push(format!("  ... +{} more", sessions.len() - 15));
                    }
                    lines.push(String::new());
                    lines.push("Use /resume <ID> to restore a session.".into());
                    self.chat_messages
                        .push(ChatMessage::System(lines.join("\n")));
                }
                Err(e) => {
                    self.chat_messages
                        .push(ChatMessage::Error(format!("Error listing sessions: {e}")));
                }
            },
            "/resume" => {
                if rest.is_empty() {
                    self.chat_messages
                        .push(ChatMessage::System("Usage: /resume <session-id>".into()));
                } else {
                    match crate::session::load(rest) {
                        Ok(session) => {
                            let sys = Self::compose_system_prompt(
                                &self.workspace_root,
                                &self.mode,
                                self.model_supports_tools,
                            );
                            self.history = vec![Message::system(sys)];
                            self.history.extend(session.messages.clone());
                            self.session_id = Some(rest.to_string());

                            // Rebuild visual chat from restored messages
                            self.chat_messages.clear();
                            self.chat_messages.push(ChatMessage::System(format!(
                                "Resumed session \"{}\" ({} messages, model: {})",
                                session.name,
                                session.messages.len(),
                                session.model
                            )));
                            for msg in &session.messages {
                                match msg.role.as_str() {
                                    "user" => {
                                        // Show a short preview, not the full injected context
                                        let preview = if msg.content.len() > 200 {
                                            format!("{}...", &msg.content[..200])
                                        } else {
                                            msg.content.clone()
                                        };
                                        self.chat_messages.push(ChatMessage::User(preview));
                                    }
                                    "assistant" => {
                                        self.chat_messages
                                            .push(ChatMessage::Assistant(msg.content.clone()));
                                    }
                                    "tool" => {
                                        let name = msg.tool_name.as_deref().unwrap_or("tool");
                                        let first_line = msg.content
                                            .lines()
                                            .find(|l| !l.trim().is_empty())
                                            .unwrap_or("")
                                            .trim();
                                        let preview = if first_line.len() > 120 {
                                            format!("{}…", &first_line[..120])
                                        } else {
                                            first_line.to_string()
                                        };
                                        self.chat_messages.push(ChatMessage::ToolResult(
                                            name.to_string(),
                                            preview,
                                        ));
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Err(e) => {
                            self.chat_messages
                                .push(ChatMessage::Error(format!("Error loading session: {e}")));
                        }
                    }
                }
            }
            "/compress" => {
                if rest.is_empty() {
                    // Show compression stats
                    let total_chars = self.history_char_count();
                    let total_tokens = compression::estimate_tokens_from_chars(total_chars);
                    let budget_chars = (self.config.context_size as usize) * 3;
                    let usage_pct = if budget_chars > 0 {
                        (total_chars as f64 / budget_chars as f64) * 100.0
                    } else {
                        0.0
                    };
                    self.chat_messages.push(ChatMessage::System(format!(
                        "Compression mode: {}\n\
                         History: {} msgs, {} chars (~{} tokens), {:.1}% of budget",
                        self.compression_mode.label(),
                        self.history.len(),
                        total_chars,
                        total_tokens,
                        usage_pct
                    )));
                } else {
                    match rest {
                        "now" => {
                            // Trigger manual compression
                            let before = self.history_char_count();
                            // Simple: evict old messages
                            let keep_tail = 8usize;
                            if self.history.len() > 1 + keep_tail {
                                let evict_end = self.history.len() - keep_tail;
                                let mut new_hist = vec![self.history[0].clone()];
                                new_hist.push(Message::system(format!(
                                    "[Compressed: {} messages evicted]",
                                    evict_end - 1
                                )));
                                new_hist.extend_from_slice(&self.history[evict_end..]);
                                self.history = new_hist;
                            }
                            let after = self.history_char_count();
                            let saved = before.saturating_sub(after);
                            if saved > 0 {
                                self.chat_messages.push(ChatMessage::System(format!(
                                    "Compressed: {} -> {} chars (-{} chars, ~{} tokens freed)",
                                    before, after, saved, saved / 4
                                )));
                            } else {
                                self.chat_messages.push(ChatMessage::System(
                                    "Nothing to compress — history is already compact.".into(),
                                ));
                            }
                        }
                        "ai" | "summarize" => {
                            self.chat_messages.push(ChatMessage::System(
                                "AI compression is not yet available in TUI mode.".into(),
                            ));
                        }
                        "always" | "auto" | "manual" => {
                            if let Some(mode) = CompressionMode::from_str_loose(rest) {
                                self.compression_mode = mode;
                                self.config.compression_mode = mode.label().to_string();
                                let _ = self.config.save();
                                self.chat_messages.push(ChatMessage::System(format!(
                                    "Compression mode: {} — {}",
                                    mode.label(),
                                    mode.description()
                                )));
                            }
                        }
                        _ => {
                            self.chat_messages.push(ChatMessage::System(
                                "Usage: /compress [now|ai|always|auto|manual]".into(),
                            ));
                        }
                    }
                }
            }
            "/unload" => {
                let client = self.client.clone();
                let tx = self.event_tx.clone();
                tokio::spawn(async move {
                    match client.unload_model().await {
                        Ok(()) => {
                            let _ = tx.send(AppEvent::StreamError(
                                "Model unloaded from VRAM/RAM.".into(),
                            ));
                        }
                        Err(e) => {
                            let _ = tx.send(AppEvent::StreamError(format!(
                                "Could not unload model: {e}"
                            )));
                        }
                    }
                });
                self.chat_messages
                    .push(ChatMessage::System("Unloading model...".into()));
            }
            "/retry" => {
                if self.pending_escalation.is_none() {
                    self.chat_messages.push(ChatMessage::System(
                        "No pending escalation to retry.".into(),
                    ));
                } else {
                    let hint = if rest.is_empty() { None } else { Some(rest.to_string()) };
                    self.handle_orchestra_escalation(
                        crate::orchestra::UserDecision::Retry { hint },
                    );
                }
            }
            "/skip" => {
                if self.pending_escalation.is_none() {
                    self.chat_messages.push(ChatMessage::System(
                        "No pending escalation to skip.".into(),
                    ));
                } else {
                    self.handle_orchestra_escalation(
                        crate::orchestra::UserDecision::Skip,
                    );
                }
            }
            "/abort" => {
                if self.pending_escalation.is_some() {
                    self.handle_orchestra_escalation(
                        crate::orchestra::UserDecision::Abort,
                    );
                } else if let Some(handle) = self.orchestra_handle.take() {
                    handle.abort();
                    self.orchestra_events_rx = None;
                    self.chat_messages.push(ChatMessage::System(
                        "Orchestra run aborted.".into(),
                    ));
                } else {
                    self.chat_messages.push(ChatMessage::System(
                        "Nothing to abort.".into(),
                    ));
                }
            }
            _ => {
                // Try action expansion (expert prompts)
                if let Some((display, prompt)) = crate::actions::try_expand(trimmed) {
                    self.chat_messages
                        .push(ChatMessage::User(display));
                    self.history.push(Message::user(&prompt));
                    self.start_llm_call();
                } else {
                    self.chat_messages.push(ChatMessage::System(format!(
                        "Unknown command: {cmd}. Type /help for available commands."
                    )));
                }
            }
        }

        self.scroll_to_bottom();
        true
    }

    // ── Tick ────────────────────────────────────────────────────────────────

    pub fn on_tick(&mut self) {
        if self.phase == AgentPhase::WaitingForLlm || self.phase == AgentPhase::ExecutingTools {
            self.spinner_frame = self.spinner_frame.wrapping_add(1);
        }

        // Drain Orchestra driver events on every tick
        self.drain_orchestra_events();

        // Ctrl+C timeout: ~15 ticks = 3 seconds at 200ms/tick
        if self.ctrl_c_pending {
            self.ctrl_c_tick_count += 1;
            if self.ctrl_c_tick_count >= 15 {
                self.ctrl_c_pending = false;
                self.ctrl_c_tick_count = 0;
                self.status_message = None;
            }
        }

        // Auto-clear transient status messages (e.g. "Copied!") after ~1.5s
        if let Some(ref msg) = self.status_message {
            if !self.ctrl_c_pending && (msg == "Copied!" || msg == "Word copied!") {
                self.status_msg_ticks += 1;
                if self.status_msg_ticks >= 8 {
                    self.status_message = None;
                    self.status_msg_ticks = 0;
                }
            }
        }

        // Safety: if selecting got stuck, clear it
        if self.selecting && self.selection_start == self.selection_end {
            self.selecting = false;
        }
    }

    /// Called when Ctrl+C is pressed. Returns true if the app should quit.
    pub fn handle_ctrl_c(&mut self) -> bool {
        if self.ctrl_c_pending {
            // Second press within timeout → quit
            return true;
        }

        // First press
        if self.phase != AgentPhase::Idle {
            // Cancel current operation
            self.phase = AgentPhase::Idle;
            self.streaming_text.clear();
            self.chat_messages
                .push(ChatMessage::System("Cancelled.".into()));
            self.scroll_to_bottom();
        }

        self.ctrl_c_pending = true;
        self.ctrl_c_tick_count = 0;
        self.status_message = Some("Press Ctrl+C again to exit".into());
        false
    }

    /// Clear Ctrl+C pending state (called when user types something).
    pub fn clear_ctrl_c(&mut self) {
        if self.ctrl_c_pending {
            self.ctrl_c_pending = false;
            self.ctrl_c_tick_count = 0;
            self.status_message = None;
        }
    }

    // ── Text selection ──────────────────────────────────────────────────────

    /// Convert screen column to text column (subtract chat area border offset).
    fn mouse_to_col(&self, screen_col: u16) -> u16 {
        // LEFT border = 1 char
        screen_col.saturating_sub(self.chat_area.x + 1)
    }

    /// Mouse down inside the chat area — start selection or double-click word select.
    /// Returns true if this was a double-click (word selected).
    pub fn start_selection(&mut self, row: u16, col: u16) -> bool {
        let now = std::time::Instant::now();
        let is_double = if let Some((prev_time, prev_row, prev_col)) = self.last_click {
            now.duration_since(prev_time).as_millis() < 400
                && prev_row == row
                && (prev_col as i16 - col as i16).unsigned_abs() <= 2
        } else {
            false
        };
        self.last_click = Some((now, row, col));

        if is_double {
            self.selecting = false;
            return true;
        }

        let line = self.mouse_to_line(row);
        let text_col = self.mouse_to_col(col);
        self.selection_start = Some((line, text_col));
        self.selection_end = Some((line, text_col));
        self.selecting = true;
        false
    }

    /// Select the word at the given screen position using plain text lines.
    pub fn select_word_at(&mut self, row: u16, col: u16, plain_lines: &[String]) {
        let vis_col = self.mouse_to_col(col) as usize;
        let abs_line = self.mouse_to_line(row);
        if abs_line >= plain_lines.len() {
            return;
        }
        let line = &plain_lines[abs_line];
        let chars: Vec<char> = line.chars().collect();

        // Map visual column → char index
        let char_idx = visual_col_to_char_idx(&chars, vis_col);
        if char_idx >= chars.len() {
            return;
        }

        let is_word_char = |c: char| c.is_alphanumeric() || c == '_' || c == '-' || c == '.';

        if !is_word_char(chars[char_idx]) {
            return;
        }

        // Find word boundaries in char indices
        let mut start = char_idx;
        while start > 0 && is_word_char(chars[start - 1]) {
            start -= 1;
        }
        let mut end = char_idx;
        while end < chars.len() && is_word_char(chars[end]) {
            end += 1;
        }

        // Convert char indices back to visual columns for selection
        let vis_start = char_idx_to_visual_col(&chars, start);
        let vis_end = char_idx_to_visual_col(&chars, end);

        self.selection_start = Some((abs_line, vis_start as u16));
        self.selection_end = Some((abs_line, vis_end as u16));
        self.selecting = false;
    }

    /// Mouse drag — extend selection, auto-scroll at edges.
    pub fn extend_selection(&mut self, row: u16, col: u16) {
        if !self.selecting {
            return;
        }
        // Auto-scroll when dragging near edges of the chat area
        if row <= self.chat_area.y + 1 {
            self.scroll_up(1);
        } else if row >= self.chat_area.y + self.chat_area.height.saturating_sub(2) {
            self.scroll_down(1);
        }
        let line = self.mouse_to_line(row);
        let text_col = self.mouse_to_col(col);
        self.selection_end = Some((line, text_col));
    }

    /// Mouse up — finalize selection, copy to clipboard.
    pub fn finish_selection(&mut self) {
        self.selecting = false;
        // Selection stays visible until next click
    }

    /// Clear any active selection.
    pub fn clear_selection(&mut self) {
        self.selection_start = None;
        self.selection_end = None;
        self.selecting = false;
    }

    /// Convert a mouse screen row to an absolute line index in the rendered chat.
    fn mouse_to_line(&self, screen_row: u16) -> usize {
        // Border offset: chat area has a left/right border, inner area starts 1 row in
        let inner_y = self.chat_area.y;
        let rel = screen_row.saturating_sub(inner_y) as usize;
        self.chat_view_start + rel
    }

    /// Get the normalized selection range: (start_line, start_col, end_line, end_col).
    pub fn selection_range(&self) -> Option<(usize, u16, usize, u16)> {
        let (sl, sc) = self.selection_start?;
        let (el, ec) = self.selection_end?;
        if sl == el && sc == ec {
            return None; // empty selection
        }
        // Normalize: start before end
        if sl < el || (sl == el && sc <= ec) {
            Some((sl, sc, el, ec))
        } else {
            Some((el, ec, sl, sc))
        }
    }

    /// Copy selected text to system clipboard (macOS: pbcopy).
    pub fn copy_selection_to_clipboard(&self, all_lines: &[String]) {
        let Some((sl, sc, el, ec)) = self.selection_range() else {
            return;
        };
        let mut selected = String::new();
        for (i, line) in all_lines.iter().enumerate() {
            if i < sl || i > el {
                continue;
            }
            let chars: Vec<char> = line.chars().collect();
            // sc/ec are visual columns — convert to char indices
            let ci_start = visual_col_to_char_idx(&chars, sc as usize);
            let ci_end = visual_col_to_char_idx(&chars, ec as usize);
            if sl == el {
                let s = ci_start.min(chars.len());
                let e = ci_end.min(chars.len());
                selected.extend(&chars[s..e]);
            } else if i == sl {
                let s = ci_start.min(chars.len());
                selected.extend(&chars[s..]);
                selected.push('\n');
            } else if i == el {
                let e = ci_end.min(chars.len());
                selected.extend(&chars[..e]);
            } else {
                selected.push_str(line);
                selected.push('\n');
            }
        }
        if !selected.is_empty() {
            // Use pbcopy on macOS, xclip on Linux
            let cmd = if cfg!(target_os = "macos") {
                "pbcopy"
            } else {
                "xclip -selection clipboard"
            };
            if let Ok(mut child) = std::process::Command::new("sh")
                .args(["-c", cmd])
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    use std::io::Write;
                    let _ = stdin.write_all(selected.as_bytes());
                }
                let _ = child.wait();
            }
        }
    }
}

// ── Visual column ↔ char index mapping ──────────────────────────────────────

/// Convert a visual column (screen position) to a char index,
/// accounting for wide Unicode characters that take 2 columns.
fn visual_col_to_char_idx(chars: &[char], vis_col: usize) -> usize {
    let mut col = 0usize;
    for (i, &c) in chars.iter().enumerate() {
        if col >= vis_col {
            return i;
        }
        col += c.width().unwrap_or(1);
    }
    chars.len() // past the end
}

/// Convert a char index to a visual column position.
fn char_idx_to_visual_col(chars: &[char], char_idx: usize) -> usize {
    chars[..char_idx.min(chars.len())]
        .iter()
        .map(|c| c.width().unwrap_or(1))
        .sum()
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn tool_call_detail(name: &str, args: &serde_json::Value) -> String {
    let s = |key: &str| args.get(key).and_then(|v| v.as_str()).map(|s| s.to_string());
    match name {
        "read_file" | "write_file" | "edit_file" => s("path").unwrap_or_default(),
        "bash" => {
            let cmd = s("command").unwrap_or_default();
            if cmd.len() > 60 {
                format!("{}...", &cmd[..59])
            } else {
                cmd
            }
        }
        "grep" => {
            let pattern = s("pattern").unwrap_or_default();
            let dir = s("path").unwrap_or_default();
            if dir.is_empty() {
                pattern
            } else {
                format!("{pattern} in {dir}")
            }
        }
        "glob" => s("pattern").unwrap_or_default(),
        "tree" => s("path").unwrap_or_else(|| ".".to_string()),
        _ => String::new(),
    }
}

// ── Orchestra helpers (impl App) ──────────────────────────────────────────────

impl App {
    /// Launch a new Orchestra run for the given goal in a background task.
    pub fn start_orchestra_run(&mut self, goal: String) {
        if self.orchestra_handle.is_some() {
            self.chat_messages.push(ChatMessage::System(
                "An Orchestra run is already in progress. Wait for it to finish or /orchestra cancel.".into(),
            ));
            return;
        }

        self.chat_messages.push(ChatMessage::System(format!(
            "Starting Orchestra run: \"{}\"", goal
        )));

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let client = self.client.clone();
        let workspace = self.workspace_root.clone();
        let policy = self.orchestra_policy;
        let ctx_size = self.config.context_size;
        let goal_clone = goal.clone();

        let handle = tokio::spawn(async move {
            crate::orchestra::run_orchestra(
                client,
                workspace,
                goal_clone,
                policy,
                ctx_size,
                tx,
            )
            .await
        });

        self.orchestra_handle = Some(handle);
        self.orchestra_events_rx = Some(rx);
        self.mode = SessionMode::Orchestra;
    }

    /// Drain pending Orchestra driver events and render them as chat messages.
    /// Called on every tick.
    pub fn drain_orchestra_events(&mut self) {
        use crate::orchestra::DriverEvent;

        let mut events = Vec::new();
        if let Some(rx) = &mut self.orchestra_events_rx {
            while let Ok(ev) = rx.try_recv() {
                events.push(ev);
            }
        }

        for ev in events {
            let msg = match ev {
                DriverEvent::RunStarted(run_id) => {
                    self.orchestra_run_id = Some(run_id.clone());
                    ChatMessage::System(format!("[run] {run_id}"))
                }
                DriverEvent::PhaseChanged(p) => {
                    ChatMessage::System(format!("[phase] {p}"))
                }
                DriverEvent::TaskStarted(id) => {
                    ChatMessage::System(format!("▶ Task started: {id}"))
                }
                DriverEvent::TaskProgress { task_id, note } => {
                    ChatMessage::System(format!("  {task_id}: {note}"))
                }
                DriverEvent::TaskFinished { task_id, verdict } => {
                    let glyph = match verdict.as_str() {
                        "Ok" => "✓",
                        "Failed" => "✗",
                        _ => "⚠",
                    };
                    ChatMessage::System(format!("{glyph} {task_id}: {verdict}"))
                }
                DriverEvent::UserEscalationNeeded { task_id, reason, report } => {
                    self.pending_escalation = Some(PendingEscalation {
                        task_id: task_id.clone(),
                        reason: reason.clone(),
                        report,
                    });
                    ChatMessage::System(format!(
                        "⚠ Escalation needed for {task_id}: {reason}\n\
                         Reply with: /retry [hint] | /skip | /abort"
                    ))
                }
                DriverEvent::RunFinished(fr) => {
                    self.orchestra_run_id = Some(fr.run_id.clone());
                    self.orchestra_handle = None;
                    self.orchestra_events_rx = None;
                    self.pending_escalation = None;
                    ChatMessage::System(format!(
                        "Orchestra run complete.\n{}", fr.summary
                    ))
                }
            };
            self.chat_messages.push(msg);
            self.scroll_to_bottom();
        }

        // Check if the handle completed
        let finished = self.orchestra_handle
            .as_ref()
            .map(|h| h.is_finished())
            .unwrap_or(false);

        if finished {
            // RunFinished event already shows the result; just clean up the handle.
            self.orchestra_handle = None;
            self.orchestra_events_rx = None;
        }
    }

    /// Resume an existing Orchestra run by its stored run_id.
    pub fn resume_orchestra_run(&mut self, run_id: String) {
        self.chat_messages.push(ChatMessage::System(format!(
            "Resuming Orchestra run: {run_id}"
        )));

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let client = self.client.clone();
        let workspace = self.workspace_root.clone();
        let ctx_size = self.config.context_size;
        let id = run_id.clone();

        let handle = tokio::spawn(async move {
            crate::orchestra::resume_orchestra(
                &id,
                None,
                client,
                workspace,
                ctx_size,
                tx,
            )
            .await
        });

        self.orchestra_run_id = Some(run_id);
        self.orchestra_handle = Some(handle);
        self.orchestra_events_rx = Some(rx);
        self.mode = SessionMode::Orchestra;
    }

    /// Handle an escalation decision from the user.
    pub fn handle_orchestra_escalation(&mut self, decision: crate::orchestra::UserDecision) {
        if let Some(esc) = self.pending_escalation.take() {
            let run_id = self.orchestra_run_id.clone().unwrap_or_default();
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let client = self.client.clone();
            let workspace = self.workspace_root.clone();
            let ctx_size = self.config.context_size;

            let handle = tokio::spawn(async move {
                crate::orchestra::resume_orchestra(
                    &run_id,
                    Some(decision),
                    client,
                    workspace,
                    ctx_size,
                    tx,
                )
                .await
            });

            self.orchestra_handle = Some(handle);
            self.orchestra_events_rx = Some(rx);

            self.chat_messages.push(ChatMessage::System(format!(
                "Resuming task {}…", esc.task_id
            )));
        }
    }
}
