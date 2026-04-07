use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use colored::Colorize;
use tokio::time::MissedTickBehavior;

use crate::config::Config;
use crate::input::InputReader;
use crate::permissions::PermissionStore;

mod auto_scan;
mod banner;
mod chat_only;
mod markdown;
use crate::ollama::{
    client::OllamaClient,
    types::{ChatOptions, LlmResponse, Message, ToolCallItem},
};
use crate::compression::{self, CompressionLevel, CompressionMode};
use crate::tools;

const SYSTEM_PROMPT: &str = "\
You are Allux, a local code assistant powered by Ollama. \
You help with software engineering tasks. \
You have access to tools: read_file, write_file, edit_file, glob, grep, tree, bash. \
Use them to explore and modify the codebase when needed. \
Always prefer reading files before editing them. \
Be concise and precise.";

const SYSTEM_PROMPT_CHAT_ONLY: &str = "\
You are Allux, a local code assistant. This session is in chat-only mode: \
Ollama does not expose tool calling for this model (e.g. many Gemma builds), so you cannot invoke read_file/grep yourself. \
The user can load disk context with slash commands: `/read <path>` reads a file into the conversation; `/glob <pattern> [dir]` lists paths; `/tree [path] [depth]` shows a folder tree. \
For broad questions (e.g. \u{201c}read my files\u{201d}, project status), Allux may auto-attach a tree + file list + key manifests before your reply—use that content; do not say you cannot read files when it is present. \
Those results appear as user messages—use them to answer. Also use the workspace snapshot below. \
For shell steps, put each command in a fenced block with language bash or sh — the app will offer to run them; if the user accepts, the command output is stored in the conversation for your next reply. \
For file changes, put the target path on the opening fence line after the language (e.g. ```rust src/lib.rs) or as // path: rel/path.rs inside the block. \
To get native tool use back, they can `/model` a tool-capable model (e.g. llama3.2). Be concise.";

//how set unlimited?
const MAX_TOOL_ROUNDS: usize = 10000;

const SYSTEM_PROMPT_PLAN: &str = "\
You are Allux, a local code assistant powered by Ollama. \
You help with software engineering tasks. \
You have access to tools: read_file, write_file, edit_file, glob, grep, tree, bash. \
Use them to explore and modify the codebase when needed. \
Always prefer reading files before editing them. \
Be concise and precise. \
When asked to create a plan, list numbered steps without calling any tools.";

/// The user-chosen session mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionMode {
    /// Pure conversation — no tools sent to the model.
    Chat,
    /// Autonomous tool use (default): LLM calls tools freely up to MAX_TOOL_ROUNDS.
    Agent,
    /// Show a numbered plan first; the user confirms before execution begins.
    Plan,
}

impl SessionMode {
    fn label(&self) -> &'static str {
        match self {
            Self::Chat  => "chat",
            Self::Agent => "agent",
            Self::Plan  => "plan",
        }
    }
}

/// Extract a human-readable detail from a tool call's arguments.
/// e.g. `read_file {"path":"src/main.rs"}` → `src/main.rs`
fn tool_call_detail(name: &str, args: &serde_json::Value) -> String {
    let s = |key: &str| args.get(key).and_then(|v| v.as_str()).map(|s| s.to_string());
    match name {
        "read_file" | "write_file" => s("path").unwrap_or_default(),
        "edit_file" => s("path").unwrap_or_default(),
        "bash" => {
            let cmd = s("command").unwrap_or_default();
            if cmd.len() > 60 {
                format!("{}…", &cmd[..59])
            } else {
                cmd
            }
        }
        "grep" => {
            let pattern = s("pattern").unwrap_or_default();
            let dir = s("path").unwrap_or_default();
            if dir.is_empty() { pattern } else { format!("{pattern} in {dir}") }
        }
        "glob" => s("pattern").unwrap_or_default(),
        "tree" => s("path").unwrap_or_else(|| ".".to_string()),
        _ => String::new(),
    }
}

/// Spinner on stdout while the model has not emitted visible text yet.
///
/// Renders immediately (no initial delay) so the user always sees feedback.
fn spawn_thinking_spinner(active: Arc<AtomicBool>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        const FRAMES: &[char] = &[
            '\u{280B}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283C}',
            '\u{2834}', '\u{2826}', '\u{2827}', '\u{2807}', '\u{280F}',
        ];
        let mut stdout = io::stdout();
        let mut i = 0usize;

        // Render the first frame immediately (no tick delay).
        if active.load(Ordering::Relaxed) {
            let c = FRAMES[0];
            let spin_char = format!("{c}").truecolor(100, 149, 237);
            let label = "Thinking\u{2026}".truecolor(70, 110, 180).dimmed();
            let _ = write!(stdout, "\r\x1b[K  {spin_char}  {label}");
            let _ = stdout.flush();
            i = 1;
        }

        let mut tick = tokio::time::interval(Duration::from_millis(80));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

        while active.load(Ordering::Relaxed) {
            tick.tick().await;
            if !active.load(Ordering::Relaxed) {
                break;
            }
            let c = FRAMES[i % FRAMES.len()];
            i += 1;
            let spin_char = format!("{c}").truecolor(100, 149, 237);
            let label = "Thinking\u{2026}".truecolor(70, 110, 180).dimmed();
            let _ = write!(stdout, "\r\x1b[K  {spin_char}  {label}");
            let _ = stdout.flush();
        }
        // Clear the spinner line.
        let _ = write!(stdout, "\r\x1b[K");
        let _ = stdout.flush();
    })
}

async fn finish_thinking_spinner(active: &Arc<AtomicBool>, handle: tokio::task::JoinHandle<()>) {
    active.store(false, Ordering::SeqCst);
    let _ = handle.await;
}

pub struct Repl {
    client: OllamaClient,
    history: Vec<Message>,
    config: Config,
    workspace_root: PathBuf,
    /// User-chosen execution mode.
    mode: SessionMode,
    /// False after Ollama reports the current model does not support tools.
    /// Reset to true when `/model` changes.
    model_supports_tools: bool,
    verbose_tools: bool,
    /// When to apply token compression: always, auto (at budget limit), or manual.
    compression_mode: CompressionMode,
    input: InputReader,
    permissions: PermissionStore,
    /// ID of the current session file (if saved/resumed).
    session_id: Option<String>,
}

enum SlashAction {
    Print(String),
    ClearHistory,
    ShowModel,
    ListModels,
    SetModel(String),
    ContextRefresh,
    ContextShow,
    ShowMode,
    SetMode(SessionMode),
    ToggleVerboseTools,
    /// Read a file from disk and inject into history (chat-only / any model).
    ReadFile(String),
    Glob {
        pattern: String,
        dir: Option<String>,
    },
    Tree {
        path: String,
        depth: usize,
    },
    /// Save current session to disk.
    SaveSession,
    /// List saved sessions.
    ListSessions,
    /// Resume a saved session by ID.
    ResumeSession(String),
    /// Show current compression mode and stats.
    CompressShow,
    /// Set compression mode (always / auto / manual).
    CompressSetMode(CompressionMode),
    /// Manually trigger compression on current history now.
    CompressNow,
    /// AI-powered summarization: sends old messages to the LLM for semantic compression.
    CompressAi,
}

/// `/tree`, `/tree src`, `/tree src 4`
fn parse_tree_slash_args(rest: &str) -> (String, usize) {
    let rest = rest.trim();
    if rest.is_empty() {
        return (".".to_string(), 3);
    }
    let parts: Vec<&str> = rest.split_whitespace().collect();
    if parts.len() == 1 {
        return (parts[0].to_string(), 3);
    }
    if let Ok(d) = parts.last().expect("len >= 2").parse::<usize>() {
        let path = parts[..parts.len() - 1].join(" ");
        (path, d.clamp(1, 20))
    } else {
        (parts.join(" "), 3)
    }
}

/// `/glob **/*.rs` or `/glob **/*.rs src`
fn parse_glob_slash_args(rest: &str) -> Option<(String, Option<String>)> {
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }
    let parts: Vec<&str> = rest.split_whitespace().collect();
    if parts.len() == 1 {
        return Some((parts[0].to_string(), None));
    }
    let pattern = parts[0].to_string();
    let dir = parts[1..].join(" ");
    Some((pattern, Some(dir)))
}

impl Repl {
    pub fn new(config: Config, workspace_root: PathBuf) -> Self {
        let client = OllamaClient::new(&config.ollama_url, &config.model);
        let history = vec![Message::system(Self::compose_system_prompt(
            &workspace_root,
            &SessionMode::Agent,
            true,
        ))];
        let compression_mode = CompressionMode::from_str_loose(&config.compression_mode)
            .unwrap_or(CompressionMode::Auto);
        Self {
            client,
            history,
            config,
            workspace_root: workspace_root.clone(),
            mode: SessionMode::Agent,
            model_supports_tools: true,
            verbose_tools: false,
            compression_mode,
            input: InputReader::new(),
            permissions: PermissionStore::new(&workspace_root),
            session_id: None,
        }
    }

    fn get_installed_skills(root: &std::path::Path) -> Vec<(String, String)> {
        let mut skills = Vec::new();
        let pattern = root.join(".agents").join("skills").join("*").join("SKILL.md");
        if let Some(glob_str) = pattern.to_str() {
            if let Ok(paths) = glob::glob(glob_str) {
                for entry in paths.flatten() {
                    if let Ok(content) = std::fs::read_to_string(&entry) {
                        let name = entry.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or("Unknown");
                        skills.push((name.to_string(), content));
                    }
                }
            }
        }
        skills
    }

    fn compose_system_prompt(
        root: &std::path::Path,
        mode: &SessionMode,
        model_supports_tools: bool,
    ) -> String {
        let mut intro = match mode {
            SessionMode::Chat => SYSTEM_PROMPT_CHAT_ONLY,
            SessionMode::Agent => {
                if model_supports_tools { SYSTEM_PROMPT } else { SYSTEM_PROMPT_CHAT_ONLY }
            }
            SessionMode::Plan => {
                if model_supports_tools { SYSTEM_PROMPT_PLAN } else { SYSTEM_PROMPT_CHAT_ONLY }
            }
        }.to_string();

        intro.push_str("\n\n<workflow_hint>\nIf you need domain knowledge (e.g. best practices, UI design), you MUST use bash to install skills BEFORE writing code:\n`npx --yes skills add <owner/repo> --skill <name> -y` (e.g. npx --yes skills add anthropics/skills --skill frontend-design -y).\nCrucially, to save context memory, if a currently installed skill is no longer relevant to the user's prompt, YOU MUST uninstall it using:\n`npx --yes skills rm <name> -y`.\n</workflow_hint>");

        let skills = Self::get_installed_skills(root);
        if !skills.is_empty() {
            intro.push_str("\n\n<installed_skills>\nYou have the following domain skills installed natively. Apply these rules stringently in your solutions:\n");
            for (name, content) in skills {
                intro.push_str(&format!("\n\n--- SKILL: {} ---\n{}", name, content));
            }
            intro.push_str("\n</installed_skills>");
        }

        format!("{intro}\n\n{}", crate::workspace::snapshot(root))
    }

    fn rebuild_system_prompt(&mut self) {
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

    fn refresh_system_prompt_from_disk(&mut self) -> Result<()> {
        self.workspace_root = std::env::current_dir()?;
        let content = Self::compose_system_prompt(
            &self.workspace_root,
            &self.mode,
            self.model_supports_tools,
        );
        if let Some(first) = self.history.first_mut() {
            if first.role == "system" {
                first.content = content;
            } else {
                self.history.insert(0, Message::system(content));
            }
        } else {
            self.history.push(Message::system(content));
        }
        Ok(())
    }

    /// Estimate total characters in conversation history.
    fn history_char_count(&self) -> usize {
        self.history.iter().map(|m| m.content.len()).sum()
    }

    /// Build ContextInfo for the current state.
    fn context_info(&self) -> banner::ContextInfo<'_> {
        banner::ContextInfo {
            used_chars: self.history_char_count(),
            budget_chars: (self.config.context_size as usize) * 3,
            context_size: self.config.context_size,
            model: &self.client.model,
        }
    }

    /// Render a divider that includes context usage info.
    fn context_divider(&self) -> String {
        let ctx = self.context_info();
        banner::divider_with_context(&ctx)
    }

    /// Evict old messages when history exceeds the context budget.
    ///
    /// Two-phase strategy:
    /// 1. First pass: compress tool results and long messages in-place (lossless).
    /// 2. If still over budget: evict oldest non-system messages, keeping a tail window.
    ///
    /// We estimate ~4 chars per token. With context_size tokens total,
    /// we reserve 25% for response, so budget = context_size * 3 chars.
    fn compact_history(&mut self) {
        // In Manual mode, never auto-compress — user triggers via /compress now
        if self.compression_mode == CompressionMode::Manual {
            return;
        }

        let budget_chars = (self.config.context_size as usize) * 3;
        let current = self.history_char_count();

        if current <= budget_chars {
            return;
        }

        self.run_compression_pass();
    }

    /// Force-compress history: phase 1 compresses in-place, phase 2 evicts oldest.
    /// Called automatically (Auto/Always modes) or manually via `/compress now`.
    fn run_compression_pass(&mut self) -> (usize, usize) {
        let budget_chars = (self.config.context_size as usize) * 3;
        let before_total = self.history_char_count();

        // ── Phase 1: Compress existing messages in-place ──────────────────
        let keep_tail = 6usize;
        let compressible_end = self.history.len().saturating_sub(keep_tail);

        let mut phase1_saved = 0usize;
        for i in 1..compressible_end {
            let msg = &self.history[i];
            if msg.content.len() < 200 {
                continue;
            }
            let original_len = msg.content.len();
            let compressed = match msg.role.as_str() {
                "tool" => {
                    let tool_name = msg.tool_name.as_deref().unwrap_or("unknown");
                    compression::compress_tool_output(tool_name, &msg.content, CompressionLevel::Aggressive).text
                }
                "assistant" | "user" => {
                    compression::compress_message(&msg.content, CompressionLevel::Standard)
                }
                _ => continue,
            };
            if compressed.len() < original_len {
                phase1_saved += original_len - compressed.len();
                self.history[i].content = compressed;
            }
        }

        // Re-check after compression
        let current = self.history_char_count();
        if current <= budget_chars {
            return (before_total, self.history_char_count());
        }

        // ── Phase 2: Evict oldest non-system messages ─────────────────────
        let min_keep = 1 + keep_tail;

        if self.history.len() <= min_keep {
            return (before_total, self.history_char_count());
        }

        let evict_end = self.history.len().saturating_sub(keep_tail);

        let evicted_info: Vec<(String, usize)> = self.history[1..evict_end]
            .iter()
            .map(|m| (m.role.clone(), m.content.len()))
            .collect();

        if evicted_info.is_empty() {
            return (before_total, self.history_char_count());
        }

        let summary = compression::build_eviction_summary(&evicted_info);

        let mut new_history = Vec::with_capacity(1 + 1 + keep_tail);
        new_history.push(self.history[0].clone()); // system
        new_history.push(Message::system(summary));
        new_history.extend_from_slice(&self.history[evict_end..]);
        self.history = new_history;

        (before_total, self.history_char_count())
    }

    /// AI-powered summarization: sends old messages to the LLM to produce
    /// a semantic summary, then replaces them with it.
    ///
    /// Keeps the system prompt (index 0) and the last `keep_tail` messages.
    /// Everything in between is summarized via an LLM call.
    async fn compress_with_ai(&mut self) -> Result<(usize, usize)> {
        let keep_tail = 6usize;
        let min_keep = 1 + keep_tail;

        if self.history.len() <= min_keep {
            anyhow::bail!("Not enough messages to summarize (need more than {min_keep})");
        }

        let before_chars = self.history_char_count();
        let summarize_end = self.history.len().saturating_sub(keep_tail);

        // Collect messages to summarize (indices 1..summarize_end)
        let messages_to_summarize: Vec<(String, String, Option<String>)> = self.history
            [1..summarize_end]
            .iter()
            .map(|m| (m.role.clone(), m.content.clone(), m.tool_name.clone()))
            .collect();

        if messages_to_summarize.is_empty() {
            anyhow::bail!("No messages to summarize");
        }

        let msg_count = messages_to_summarize.len();
        let summarize_prompt = compression::build_ai_summarize_prompt(&messages_to_summarize);

        // Call the LLM with a summarization prompt
        let summarize_messages = vec![
            Message::system(compression::ai_summarize_system_prompt().to_string()),
            Message::user(summarize_prompt),
        ];

        let options = ChatOptions {
            temperature: Some(0.1), // low temperature for factual summary
            num_ctx: Some(self.config.context_size),
        };

        let mut summary_text = String::new();
        let result = self
            .client
            .chat(&summarize_messages, None, Some(options), |chunk| {
                summary_text.push_str(chunk);
            })
            .await;

        match result {
            Ok(_) if !summary_text.trim().is_empty() => {
                // Build the compressed summary message
                let summary = format!(
                    "[AI-compressed context: {msg_count} messages summarized]\n\n{summary_text}"
                );

                // Rebuild history: system + summary + recent tail
                let mut new_history = Vec::with_capacity(1 + 1 + keep_tail);
                new_history.push(self.history[0].clone()); // system prompt
                new_history.push(Message::system(summary));
                new_history.extend_from_slice(&self.history[summarize_end..]);
                self.history = new_history;

                let after_chars = self.history_char_count();
                Ok((before_chars, after_chars))
            }
            Ok(_) => anyhow::bail!("LLM returned empty summary"),
            Err(e) => anyhow::bail!("AI summarization failed: {e}"),
        }
    }

    fn chat_options(&self) -> ChatOptions {
        ChatOptions {
            temperature: None,
            num_ctx: Some(self.config.context_size),
        }
    }

    /// When the model will not receive tools this turn, expand "read project / status" asks.
    fn wrap_user_input_with_auto_scan(&self, input: &str) -> String {
        let agent_will_use_tools = matches!(self.mode, SessionMode::Agent | SessionMode::Plan)
            && self.model_supports_tools;
        if agent_will_use_tools || !auto_scan::should_trigger(input) {
            return input.to_string();
        }
        match auto_scan::build_scan(&self.workspace_root) {
            Ok(scan) if !scan.trim().is_empty() => {
                println!("{}", "  Gathering project tree and key files…".dimmed());
                format!("{scan}\n\n---\n**User question:**\n{input}")
            }
            Ok(_) => input.to_string(),
            Err(e) => {
                eprintln!("{}", format!("  (auto-scan skipped: {e})").dimmed());
                input.to_string()
            }
        }
    }

    /// Same as [`wrap_user_input_with_auto_scan`], but mutates the last user message (first-turn tool fallback).
    fn merge_auto_scan_into_last_user_message(&mut self) {
        let Some(last) = self.history.last_mut() else {
            return;
        };
        if last.role != "user" || !auto_scan::should_trigger(&last.content) {
            return;
        }
        let scan = match auto_scan::build_scan(&self.workspace_root) {
            Ok(s) if !s.trim().is_empty() => s,
            Ok(_) => return,
            Err(e) => {
                eprintln!("{}", format!("  (auto-scan skipped: {e})").dimmed());
                return;
            }
        };
        println!("{}", "  Gathering project tree and key files…".dimmed());
        let q = last.content.clone();
        last.content = format!("{scan}\n\n---\n**User question:**\n{q}");
    }

    pub async fn run(&mut self) -> Result<()> {
        let mut stdout = io::stdout();

        let skills = Self::get_installed_skills(&self.workspace_root)
            .into_iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>();

        banner::print_welcome(
            env!("CARGO_PKG_VERSION"),
            &self.client.model,
            &self.workspace_root,
            &skills,
        );

        loop {
            println!("{}", self.context_divider());
            let prompt = banner::accent("❯").bold().to_string();
            let input = match self.input.read_line(&prompt, 1, Some(banner::INPUT_FOOTER)) {
                Ok(Some(s)) if !s.is_empty() => s,
                Ok(Some(_)) => continue,
                Ok(None) => break, // Ctrl+D
                Err(e) => {
                    eprintln!("Input error: {e}");
                    break;
                }
            };

            if matches!(input.as_str(), "/quit" | "/exit" | "/q") {
                println!("{}", "Goodbye.".dimmed());
                break;
            }

            if let Some(action) = self.parse_slash(&input) {
                self.handle_slash(action).await;
                continue;
            }

            let user_payload = self.wrap_user_input_with_auto_scan(&input);
            self.history.push(Message::user(user_payload));
            self.run_agentic_loop(&mut stdout).await;
            println!();
        }

        // Auto-save session on exit if there are user messages
        let has_user_msgs = self.history.iter().any(|m| m.role == "user");
        if has_user_msgs {
            if let Ok(path) = crate::session::save(
                &self.history,
                &self.client.model,
                &self.workspace_root,
                self.session_id.as_deref(),
            ) {
                let id = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
                println!(
                    "  {} Session auto-saved (id: {})",
                    "\u{2713}".truecolor(100, 200, 100),
                    id.truecolor(100, 180, 255)
                );
            }
        }

        Ok(())
    }

    /// Core loop: handles Chat, Agent, and Plan modes.
    async fn run_agentic_loop(&mut self, stdout: &mut io::Stdout) {
        // ── Plan mode: planning phase before execution ───────────────────────
        if self.mode == SessionMode::Plan {
            if !self.plan_phase(stdout).await {
                return; // user rejected; user message already removed from history
            }
            // user approved → "Proceed." is now the last message in history
        }

        let tools = tools::all_definitions();

        for _round in 0..MAX_TOOL_ROUNDS {
            // Compact history if approaching context window limit
            self.compact_history();

            let mut streamed_text = String::new();
            let options = self.chat_options();

            // In Chat mode, never pass tools. In Agent/Plan, use tools if model supports them.
            let use_tools = matches!(self.mode, SessionMode::Agent | SessionMode::Plan)
                && self.model_supports_tools;

            let tools_arg: Option<&[_]> = if use_tools { Some(&tools) } else { None };

            // Show spinner while waiting — prints immediately so user never sees a "stuck" screen.
            let spin_active = Arc::new(AtomicBool::new(true));
            let spin_on_text = spin_active.clone();
            let spinner = spawn_thinking_spinner(spin_active.clone());

            let mut result = tokio::select! {
                res = self.client.chat(
                    &self.history,
                    tools_arg,
                    Some(options.clone()),
                    |chunk| {
                        if use_tools && !chunk.is_empty() {
                            spin_on_text.store(false, Ordering::SeqCst);
                        }
                        streamed_text.push_str(chunk);
                    },
                ) => res,
                _ = tokio::signal::ctrl_c() => Err(anyhow::anyhow!("Cancelled by user (Ctrl+C)")),
            };

            finish_thinking_spinner(&spin_active, spinner).await;

            // Print the response prefix after spinner clears, so it's always visible.
            print!("{}", banner::response_prefix());
            let _ = stdout.flush();

            // ── Auto-fallback: model doesn't support tools ───────────────────
            if let Err(ref e) = result {
                if self.model_supports_tools && e.to_string().contains("does not support tools") {
                    println!();
                    println!(
                        "{}",
                        format!(
                            "  ⚠ Model '{}' does not support tools — falling back to chat for this session.\n\
                             \x20     Use `/read <path>`, `/glob <pattern>`, `/tree` to load files into context, or `/model llama3.2` (or another tool-capable model).",
                            self.client.model
                        )
                        .yellow()
                    );
                    self.model_supports_tools = false;
                    self.rebuild_system_prompt();
                    streamed_text.clear();

                    self.merge_auto_scan_into_last_user_message();

                    let spin2 = Arc::new(AtomicBool::new(true));
                    let sp2 = spawn_thinking_spinner(spin2.clone());
                    result = tokio::select! {
                        res = self.client.chat(&self.history, None, Some(options), |chunk| {
                            streamed_text.push_str(chunk);
                        }) => res,
                        _ = tokio::signal::ctrl_c() => Err(anyhow::anyhow!("Cancelled by user (Ctrl+C)")),
                    };
                    finish_thinking_spinner(&spin2, sp2).await;
                }
            }

            match result {
                Err(e) => {
                    println!();
                    let msg = e.to_string();
                    let display = Self::user_facing_request_error(&msg);
                    eprintln!("{}", format!("Error: {display}").red());
                    self.history.pop();
                    return;
                }

                Ok(LlmResponse::Text { content, stats }) => {
                    println!();
                    // Use chat rendering when mode is Chat OR when the model has no tools.
                    let in_chat_render =
                        self.mode == SessionMode::Chat || !self.model_supports_tools;
                    if in_chat_render {
                        let (visible, shell_cmds, file_blocks) =
                            chat_only::strip_shell_fences(&content);
                        let rendered = markdown::to_terminal(&visible);
                        print!("{}", rendered.trim_end());
                        if !rendered.ends_with('\n') {
                            println!();
                        }
                        for cmd in shell_cmds {
                            self.offer_run_suggested_command(&cmd).await;
                        }
                        for block in file_blocks {
                            self.offer_save_file_block(&block).await;
                        }
                    } else {
                        let rendered = markdown::to_terminal(&content);
                        print!("{}", rendered.trim_end());
                        if !rendered.ends_with('\n') {
                            println!();
                        }
                    }
                    self.history.push(Message::assistant(&content));
                    banner::print_token_usage(&stats);
                    return;
                }

                Ok(LlmResponse::ToolCalls { calls, stats }) => {
                    println!();
                    if !calls.is_empty() {
                        for call in &calls {
                            let name = &call.function.name;
                            let detail = tool_call_detail(name, &call.function.arguments);
                            if detail.is_empty() {
                                println!(
                                    "  {} {}",
                                    "⚡".truecolor(100, 149, 237),
                                    name.truecolor(140, 140, 160)
                                );
                            } else {
                                println!(
                                    "  {} {} {}",
                                    "⚡".truecolor(100, 149, 237),
                                    name.truecolor(140, 140, 160),
                                    detail.truecolor(100, 100, 120)
                                );
                            }
                        }
                        banner::print_token_usage(&stats);
                    }
                    self.history.push(Message::assistant_tool_calls(calls.clone()));
                    let tool_messages = self.execute_tool_calls(&calls, stdout).await;
                    self.history.extend(tool_messages);
                }
            }
        }

        eprintln!(
            "{}",
            format!("[Warning: reached max tool rounds ({MAX_TOOL_ROUNDS})]").yellow()
        );
    }

    /// Short, non-leaky error text when Ollama rejects tool use (release builds).
    fn user_facing_request_error(raw: &str) -> String {
        if raw.contains("does not support tools") {
            #[cfg(debug_assertions)]
            return raw.to_string();
            #[cfg(not(debug_assertions))]
            return "This model cannot use tools in Ollama. Try `/model` with a tool-capable model (e.g. llama3.2), or stay in chat-only mode with bash/sh fenced commands.".to_string();
        }
        raw.to_string()
    }

    /// Plan mode: call the LLM once (no tools) to get a numbered plan, display it,
    /// ask the user whether to proceed.
    ///
    /// Returns `true` if the user confirmed (a "Proceed." message is pushed to history).
    /// Returns `false` if the user rejected (the user's original message is popped from history).
    async fn plan_phase(&mut self, stdout: &mut io::Stdout) -> bool {
        loop {
            // Build a temporary messages slice: same history but the last user message asks for a plan.
            let mut plan_messages = self.history.clone();
            if let Some(last) = plan_messages.last_mut() {
                if last.role == "user" {
                    last.content.push_str(
                        "\n\nBefore executing anything, reply with a concise numbered plan \
                         of the exact steps you will take. Do not call any tools. \
                         Do not start executing. I will confirm the plan before you proceed.",
                    );
                }
            }

            let spin_active = Arc::new(AtomicBool::new(true));
            let spinner = spawn_thinking_spinner(spin_active.clone());
            let mut plan_text = String::new();
            let options = self.chat_options();

            let result = tokio::select! {
                res = self.client.chat(&plan_messages, None, Some(options), |chunk| {
                    plan_text.push_str(chunk);
                }) => res,
                _ = tokio::signal::ctrl_c() => Err(anyhow::anyhow!("Cancelled by user (Ctrl+C)")),
            };

            finish_thinking_spinner(&spin_active, spinner).await;

            if let Err(e) = result {
                println!();
                eprintln!("{}", format!("Error during planning: {e}").red());
                self.history.pop(); // remove the user message
                return false;
            }

            // Display the plan
            println!();
            println!("{}", "📋  Plan:".bold().cyan());
            let rendered = markdown::to_terminal(&plan_text);
            print!("{}", rendered.trim_end());
            if !rendered.ends_with('\n') {
                println!();
            }
            println!();

            // Push plan to history so the LLM remembers it during execution
            self.history.push(Message::assistant(&plan_text));

            // Ask user
            loop {
                const PLAN_PROMPT: &str = "Plan: [y]es / [n]o / [s]ave / <feedback>: ";
                let vis = PLAN_PROMPT.chars().count();
                let input = match self.input.read_line(PLAN_PROMPT, vis, None) {
                    Ok(Some(s)) => s.trim().to_string(),
                    _ => {
                        self.history.pop(); // remove assistant plan
                        self.history.pop(); // remove user message
                        return false;
                    }
                };

                let t = input.to_lowercase();
                if t == "y" || t == "yes" {
                    self.history.push(Message::user("Proceed with the plan as agreed."));
                    println!("{}", "  Executing…".dimmed());
                    println!();
                    let _ = stdout.flush();
                    return true;
                } else if t == "n" || t == "no" {
                    self.history.pop(); // remove assistant plan
                    self.history.pop(); // remove user message
                    println!("{}", "  Plan rejected.".dimmed());
                    return false;
                } else if t == "s" || t == "save" {
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let filename = format!("plan_{}.md", ts);
                    let save_path = self.workspace_root.join(&filename);
                    match std::fs::write(&save_path, &plan_text) {
                        Ok(_) => {
                            let msg = format!("✓ Saved plan to {}", filename);
                            println!("  {}", msg.green());
                        }
                        Err(e) => {
                            let msg = format!("✗ Error saving plan: {}", e);
                            eprintln!("  {}", msg.red());
                        }
                    }
                    continue; // ask again
                } else if !input.is_empty() {
                    // Treat as feedback
                    println!("{}", "  Iterating on plan with your feedback…".dimmed());
                    self.history.push(Message::user(format!("Feedback on the plan:\n\n{}\n\nPlease provide a new updated plan based on this feedback.", input)));
                    break; // break inner loop to generate a new plan
                }
            }
        }
    }

    /// After a chat-only reply, offer to run extracted shell blocks (not shown as duplicate prose).
    /// If the user runs the command, stdout/stderr is appended to history so the next message can use it.
    async fn offer_run_suggested_command(&mut self, cmd: &str) {
        let cmd = cmd.trim();
        if cmd.is_empty() {
            return;
        }
        println!("{}", format!("⚙ suggested command:\n{}", cmd).cyan());
        const RUN_PROMPT: &str = "Run? [y/N]";
        let vis = RUN_PROMPT.chars().count();
        match self.input.read_line(RUN_PROMPT, vis, None) {
            Ok(Some(s))
                if s.eq_ignore_ascii_case("y") || s.eq_ignore_ascii_case("yes") =>
            {
                match tools::run_bash(cmd).await {
                    Ok(out) => {
                        print!("{out}");
                        if !out.ends_with('\n') {
                            println!();
                        }
                        let inject = format!(
                            "The user approved running this suggested shell command in Allux.\n\n\
                             **Command:** `{cmd}`\n\n**Output:**\n```\n{out}\n```"
                        );
                        self.history.push(Message::user(inject));
                        println!("{}", "  Output added to conversation context.".dimmed());
                    }
                    Err(e) => {
                        eprintln!("{}", format!("{e}").red());
                        let inject = format!(
                            "The user approved running this suggested shell command in Allux, but it failed.\n\n\
                             **Command:** `{cmd}`\n\n**Error:** {e}"
                        );
                        self.history.push(Message::user(inject));
                        println!("{}", "  (failure recorded in conversation context.)".dimmed());
                    }
                }
            }
            Ok(_) | Err(_) => {}
        }
    }

    /// After a chat-only reply, offer to save a non-shell code block.
    /// If the model gave a path (fence line or `// path:`), only confirm [Y/n]; otherwise ask for a filename.
    async fn offer_save_file_block(&mut self, block: &chat_only::FileBlock) {
        let content = block.content.trim();
        if content.is_empty() {
            return;
        }

        if let Some(path) = &block.suggested_path {
            println!(
                "{}",
                format!("💾 Save to `{}`? [Y/n]", path).cyan()
            );
            const CONFIRM_PROMPT: &str = "  Confirm [Y/n]: ";
            let vis = CONFIRM_PROMPT.chars().count();
            match self.input.read_line(CONFIRM_PROMPT, vis, None) {
                Ok(Some(s)) => {
                    let t = s.trim().to_lowercase();
                    if t == "n" || t == "no" {
                        return;
                    }
                }
                Ok(None) | Err(_) => return,
            }
            match tools::run_write_file(path, content) {
                Ok(msg) => println!("  {} {}", "✓".green(), msg),
                Err(e) => eprintln!("  {} {e}", "✗".red()),
            }
            return;
        }

        let ext_hint = lang_to_ext(&block.lang);
        let label = if ext_hint.is_empty() {
            format!("Save {} code block to file?", block.lang)
        } else {
            format!("Save {} code block to file? (e.g. main.{})", block.lang, ext_hint)
        };
        println!("{}", format!("💾 {label}").cyan());

        const SAVE_PROMPT: &str = "  Filename (Enter to skip): ";
        let vis = SAVE_PROMPT.chars().count();

        let filename = match self.input.read_line(SAVE_PROMPT, vis, None) {
            Ok(Some(s)) if !s.trim().is_empty() => s.trim().to_string(),
            _ => return,
        };

        match tools::run_write_file(&filename, content) {
            Ok(msg) => println!("  {} {}", "✓".green(), msg),
            Err(e) => eprintln!("  {} {e}", "✗".red()),
        }
    }

    async fn execute_tool_calls(
        &mut self,
        calls: &[ToolCallItem],
        stdout: &mut io::Stdout,
    ) -> Vec<Message> {
        let mut results = Vec::new();
        let mut tool_log: Vec<String> = Vec::new();
        let mut printed_compact = false;

        for call in calls {
            let name = &call.function.name;
            let args = &call.function.arguments;

            // ── Permission gate for bash ────────────────────────────────────────
            if name == "bash" {
                let command = args["command"].as_str().unwrap_or("(unknown)");

                if !self.permissions.is_granted(command) {
                    banner::print_permission_bash(command);

                    const PERM_PROMPT: &str = "  \u{276F} ";
                    let vis = 4;

                    let (decision, raw) = match self.input.read_line(PERM_PROMPT, vis, None) {
                        Ok(Some(s)) => (PermissionStore::parse_input(&s), s),
                        Ok(None) | Err(_) => {
                            use crate::permissions::Decision;
                            (Decision::Deny, String::new())
                        }
                    };

                    use crate::permissions::Decision;
                    match decision {
                        Decision::AllowOnce => {
                            // fall through to dispatch, no grant stored
                        }
                        Decision::AllowSession => {
                            self.permissions.grant_session(command);
                        }
                        Decision::AllowFamily => {
                            self.permissions.grant_family(command);
                        }
                        Decision::AllowWorkspace => {
                            self.permissions.grant_workspace(command);
                            println!("  {}", "Saved to workspace permissions.".truecolor(100, 200, 100));
                        }
                        Decision::AllowGlobal => {
                            self.permissions.grant_global(command);
                            println!("  {}", "Saved to global permissions.".truecolor(100, 200, 100));
                        }
                        Decision::Deny => {
                            println!("  {}", "\u{2717} denied".red());
                            let msg = if raw.trim().is_empty() || raw.trim().to_lowercase() == "n" || raw.trim().to_lowercase() == "no" {
                                "Permission denied: user rejected the bash command.".to_string()
                            } else {
                                format!("Permission denied: user rejected the bash command with response: '{}'", raw.trim())
                            };
                            results.push(Message::tool_result(
                                name.clone(),
                                msg,
                            ));
                            continue;
                        }
                    }
                }
            }

            // ── Permission gate for edit_file / write_file ─────────────────────
            if name == "edit_file" || name == "write_file" {
                let path = args["path"].as_str().unwrap_or("(unknown)");
                let perm_key = format!("{name}:{path}");

                if !self.permissions.is_granted(&perm_key) {
                    if name == "edit_file" {
                        let old_str = args["old_str"].as_str().unwrap_or("");
                        let new_str = args["new_str"].as_str().unwrap_or("");
                        banner::print_permission_edit(path, old_str, new_str);
                    } else {
                        println!();
                        println!("{}", banner::box_top_pub());
                        println!("  {} {}", "Allux wants to write:".bold(), path.cyan().bold());
                        println!("{}", banner::box_bottom_pub());
                    }

                    const PERM_PROMPT: &str = "  \u{276F} ";
                    let vis = 4;
                    let (decision, _raw) = match self.input.read_line(PERM_PROMPT, vis, None) {
                        Ok(Some(s)) => {
                            // For file ops: y=allow, n=deny (simpler than bash)
                            let d = match s.trim().to_lowercase().as_str() {
                                "y" | "yes" | "" => crate::permissions::Decision::AllowOnce,
                                "n" | "no" => crate::permissions::Decision::Deny,
                                other => PermissionStore::parse_input(other),
                            };
                            (d, s)
                        }
                        Ok(None) | Err(_) => (crate::permissions::Decision::Deny, String::new()),
                    };

                    use crate::permissions::Decision;
                    match decision {
                        Decision::AllowOnce => {}
                        Decision::AllowSession => self.permissions.grant_session(&perm_key),
                        Decision::AllowFamily | Decision::AllowWorkspace => {
                            self.permissions.grant_workspace(&perm_key);
                        }
                        Decision::AllowGlobal => {
                            self.permissions.grant_global(&perm_key);
                        }
                        Decision::Deny => {
                            println!("  {}", "\u{2717} denied".red());
                            results.push(Message::tool_result(
                                name.clone(),
                                format!("Permission denied: user rejected {name} on {path}."),
                            ));
                            continue;
                        }
                    }
                }
            }
            // ───────────────────────────────────────────────────────────────────

            let is_bash = name == "bash";

            // Build a short label for this tool call
            let tool_label = if !is_bash {
                let args_str = if let Some(obj) = args.as_object() {
                    let parts: Vec<String> = obj
                        .iter()
                        .take(2)
                        .map(|(k, v)| {
                            let val = v.as_str().unwrap_or("…");
                            let short = if val.len() > 50 { &val[..50] } else { val };
                            format!("{k}={short:?}")
                        })
                        .collect();
                    parts.join(", ")
                } else {
                    String::new()
                };
                format!("{}({})", name, args_str)
            } else {
                String::new()
            };

            if !is_bash {
                tool_log.push(tool_label.clone());
                if self.verbose_tools {
                    print!("    {} {}  ", "▸".truecolor(100, 180, 255), tool_label.truecolor(180, 180, 190));
                    let _ = stdout.flush();
                } else {
                    // Compact: rewrite a single line showing the last tool
                    if printed_compact {
                        use crossterm::{cursor, execute as cexec};
                        let _ = cexec!(stdout, cursor::MoveToPreviousLine(1));
                    }
                    let counter = if tool_log.len() > 1 {
                        format!("[{}/{}] ", tool_log.len(), calls.len())
                    } else {
                        String::new()
                    };
                    use crossterm::{
                        execute as cexec,
                        style::Print,
                        terminal::{self, ClearType},
                    };
                    let _ = cexec!(
                        stdout,
                        terminal::Clear(ClearType::CurrentLine),
                        Print(format!(
                            "    {} {}{}",
                            "▸".truecolor(100, 180, 255),
                            counter.truecolor(100, 100, 120),
                            tool_label.truecolor(140, 140, 160)
                        ))
                    );
                    println!();
                    printed_compact = true;
                }
            }

            let raw_output = match tools::dispatch(name, args).await {
                Ok(out) => {
                    if !is_bash && self.verbose_tools {
                        println!("{}", "✓".green());
                    }
                    out
                }
                Err(e) => {
                    if !is_bash && self.verbose_tools {
                        println!("{}", "✗".red());
                    }
                    format!("Error executing {name}: {e}")
                }
            };

            // Compress tool output based on compression mode.
            let output = if self.compression_mode == CompressionMode::Always {
                let cr = compression::compress_tool_output(name, &raw_output, CompressionLevel::Standard);
                if self.verbose_tools && cr.original_len > 200 && cr.compressed_len < cr.original_len {
                    let saved = cr.original_len - cr.compressed_len;
                    let pct = ((saved as f64 / cr.original_len as f64) * 100.0) as u32;
                    if pct > 5 {
                        println!(
                            "      {} compressed: {} → {} chars (−{}%)",
                            "↘".truecolor(100, 180, 255),
                            cr.original_len, cr.compressed_len, pct
                        );
                    }
                }
                cr.text
            } else {
                // Auto and Manual: store raw output; compression happens later if needed
                raw_output
            };
            results.push(Message::tool_result(name.clone(), output));
        }

        // Compact mode: overwrite last line with final summary
        if !self.verbose_tools && printed_compact {
            use crossterm::{cursor, execute as cexec, style::Print, terminal::{self, ClearType}};
            let _ = cexec!(stdout, cursor::MoveToPreviousLine(1));
            let _ = cexec!(
                stdout,
                terminal::Clear(ClearType::CurrentLine),
                Print(format!(
                    "    {} {} tool(s) completed",
                    "✓".green(),
                    tool_log.len()
                ))
            );
            println!();
        }

        results
    }

    async fn handle_slash(&mut self, action: SlashAction) {
        match action {
            SlashAction::Print(s) => println!("{s}"),
            SlashAction::ClearHistory => {
                let sys = Self::compose_system_prompt(
                    &self.workspace_root,
                    &self.mode,
                    self.model_supports_tools,
                );
                self.history = vec![Message::system(sys)];
                println!("{}", "Conversation cleared.".dimmed());
            }
            SlashAction::ContextRefresh => match self.refresh_system_prompt_from_disk() {
                Ok(()) => println!("{}", "Workspace context refreshed (cwd + snapshot).".dimmed()),
                Err(e) => eprintln!("{}", format!("Could not refresh context: {e}").red()),
            },
            SlashAction::ContextShow => {
                println!(
                    "{}",
                    format!("Workspace root: {}", self.workspace_root.display()).dimmed()
                );
                println!("{}", crate::workspace::snapshot(&self.workspace_root));
            }
            SlashAction::ShowModel => {
                println!("{} {}", "Current model:".bold(), self.client.model.cyan().bold());
            }
            SlashAction::ListModels => {
                match OllamaClient::list_models(self.client.base_url()).await {
                    Ok(models) => {
                        for m in models {
                            let active = if m.name == self.client.model { " ◀ active" } else { "" };
                            println!(
                                "  {}  {}  {}{}",
                                m.name.bold(),
                                m.details.parameter_size.dimmed(),
                                m.details.quantization_level.dimmed(),
                                active.cyan()
                            );
                        }
                    }
                    Err(e) => eprintln!("{}", format!("Error: {e}").red()),
                }
            }
            SlashAction::SetModel(name) => {
                self.config.model = name.clone();
                self.client.model = name;
                self.model_supports_tools = true; // reset; new model may support tools
                self.rebuild_system_prompt();
                if let Err(e) = self.config.save() {
                    eprintln!(
                        "{}",
                        format!("Model updated for session but config save failed: {e}").yellow()
                    );
                } else {
                    println!(
                        "{}  {}  (mode: {})",
                        "Model set to".bold(),
                        self.client.model.cyan().bold(),
                        self.mode.label().dimmed()
                    );
                }
            }
            SlashAction::ShowMode => {
                println!(
                    "{}  {}",
                    "Current mode:".bold(),
                    self.mode.label().cyan().bold()
                );
                println!(
                    "{}",
                    "  chat  — no tools, pure conversation\n\
                     agent — autonomous tool use (default)\n\
                     plan  — show numbered plan first, confirm, then execute"
                        .dimmed()
                );
            }
            SlashAction::SetMode(mode) => {
                self.mode = mode;
                self.rebuild_system_prompt();
                println!(
                    "{}  {}",
                    "Mode set to".bold(),
                    self.mode.label().cyan().bold()
                );
            }
            SlashAction::ToggleVerboseTools => {
                self.verbose_tools = !self.verbose_tools;
                if self.verbose_tools {
                    println!("{}", "  Verbose tool log: ON — all tool calls shown individually".cyan());
                } else {
                    println!("{}", "  Verbose tool log: OFF — compact mode (last 2 shown)".dimmed());
                }
            }
            SlashAction::ReadFile(path) => {
                if path.is_empty() {
                    println!("{}", "Usage: /read <path>".dimmed());
                    return;
                }
                match tools::run_read_file(&path) {
                    Ok(text) => {
                        println!("{}", format!("── {} ──", path).dimmed());
                        println!("{text}");
                        let inject = format!(
                            "The following is the contents of `{path}` (loaded with /read). Use it to answer my questions.\n\n{text}"
                        );
                        self.history.push(Message::user(inject));
                        println!("{}", "  Added to conversation context.".dimmed());
                    }
                    Err(e) => eprintln!("{}", format!("{e}").red()),
                }
            }
            SlashAction::Glob { pattern, dir } => {
                match tools::run_glob(&pattern, dir.as_deref()) {
                    Ok(text) => {
                        println!("{}", text);
                        let inject = format!(
                            "Glob `{pattern}`{}:\n\n{text}",
                            dir.as_ref().map(|d| format!(" in `{d}`")).unwrap_or_default()
                        );
                        self.history.push(Message::user(inject));
                        println!("{}", "  Added to conversation context.".dimmed());
                    }
                    Err(e) => eprintln!("{}", format!("{e}").red()),
                }
            }
            SlashAction::Tree { path, depth } => {
                match tools::run_tree(&path, depth) {
                    Ok(text) => {
                        println!("{}", text);
                        let inject = format!(
                            "Directory tree for `{path}` (depth {depth}, from /tree):\n\n{text}"
                        );
                        self.history.push(Message::user(inject));
                        println!("{}", "  Added to conversation context.".dimmed());
                    }
                    Err(e) => eprintln!("{}", format!("{e}").red()),
                }
            }
            SlashAction::SaveSession => {
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
                        println!(
                            "  {} Session saved (id: {})",
                            "\u{2713}".green(),
                            id.cyan()
                        );
                    }
                    Err(e) => eprintln!("{}", format!("Error saving session: {e}").red()),
                }
            }
            SlashAction::ListSessions => {
                match crate::session::list() {
                    Ok(sessions) if sessions.is_empty() => {
                        println!("{}", "  No saved sessions.".dimmed());
                    }
                    Ok(sessions) => {
                        println!("  {}", "Saved sessions:".bold());
                        for s in sessions.iter().take(10) {
                            let age = format_age(s.updated_at);
                            println!(
                                "    {}  {}  {}  {} msgs  {}",
                                s.id.cyan(),
                                s.name.bold(),
                                s.model.dimmed(),
                                s.message_count,
                                age.dimmed()
                            );
                        }
                        if sessions.len() > 10 {
                            println!("    {} more...", sessions.len() - 10);
                        }
                        println!("  {}", "Use /resume <id> to restore a session.".dimmed());
                    }
                    Err(e) => eprintln!("{}", format!("Error listing sessions: {e}").red()),
                }
            }
            SlashAction::ResumeSession(id) => {
                match crate::session::load(&id) {
                    Ok(session) => {
                        // Rebuild system prompt, then append saved messages
                        let sys = Self::compose_system_prompt(
                            &self.workspace_root,
                            &self.mode,
                            self.model_supports_tools,
                        );
                        self.history = vec![Message::system(sys)];
                        self.history.extend(session.messages);
                        self.session_id = Some(id.clone());
                        println!(
                            "  {} Resumed session \"{}\" ({} messages, model: {})",
                            "\u{2713}".green(),
                            session.name.bold(),
                            self.history.len() - 1,
                            session.model.cyan()
                        );
                    }
                    Err(e) => eprintln!("{}", format!("Error loading session: {e}").red()),
                }
            }
            SlashAction::CompressShow => {
                let total_chars = self.history_char_count();
                let total_tokens = compression::estimate_tokens_from_chars(total_chars);
                let budget_chars = (self.config.context_size as usize) * 3;
                let usage_pct = if budget_chars > 0 {
                    (total_chars as f64 / budget_chars as f64) * 100.0
                } else {
                    0.0
                };
                println!("  {} {}", "Compression mode:".bold(), self.compression_mode.label().cyan().bold());
                println!("  {} {}", "Description:".bold(), self.compression_mode.description().dimmed());
                println!(
                    "  {} {} msgs, {} chars (~{} tokens), {:.1}% of budget",
                    "History:".bold(),
                    self.history.len(),
                    total_chars,
                    total_tokens,
                    usage_pct
                );
            }
            SlashAction::CompressSetMode(new_mode) => {
                self.compression_mode = new_mode;
                self.config.compression_mode = new_mode.label().to_string();
                if let Err(e) = self.config.save() {
                    eprintln!("{}", format!("Mode updated for session but config save failed: {e}").yellow());
                } else {
                    println!(
                        "  {} {} — {}",
                        "Compression mode:".bold(),
                        new_mode.label().cyan().bold(),
                        new_mode.description().dimmed()
                    );
                }
            }
            SlashAction::CompressNow => {
                let (before_chars, after_chars) = self.run_compression_pass();
                let saved = before_chars.saturating_sub(after_chars);
                if saved > 0 {
                    println!(
                        "  {} Compressed: {} → {} chars (−{} chars, ~{} tokens freed)",
                        "\u{2713}".green(),
                        before_chars,
                        after_chars,
                        saved,
                        saved / 4
                    );
                } else {
                    println!("{}", "  Nothing to compress — history is already compact.".dimmed());
                }
            }
            SlashAction::CompressAi => {
                let msg_count = self.history.len();
                if msg_count <= 7 {
                    println!("{}", "  Not enough history to summarize (need more than 7 messages).".dimmed());
                    return;
                }
                println!(
                    "  {} Summarizing {} messages via {}…",
                    "⚡".truecolor(100, 149, 237),
                    msg_count - 7, // subtract system + keep_tail
                    self.client.model.cyan()
                );

                let spin_active = Arc::new(AtomicBool::new(true));
                let spinner = spawn_thinking_spinner(spin_active.clone());

                let result = self.compress_with_ai().await;

                finish_thinking_spinner(&spin_active, spinner).await;

                match result {
                    Ok((before, after)) => {
                        let saved = before.saturating_sub(after);
                        let pct = if before > 0 {
                            ((saved as f64 / before as f64) * 100.0) as u32
                        } else {
                            0
                        };
                        println!(
                            "  {} AI summary: {} → {} chars (−{} chars, ~{} tokens freed, −{}%)",
                            "\u{2713}".green(),
                            before, after, saved, saved / 4, pct
                        );
                        println!(
                            "  {} History now: {} messages",
                            "↘".truecolor(100, 180, 255),
                            self.history.len()
                        );
                    }
                    Err(e) => {
                        eprintln!("{}", format!("  Error: {e}").red());
                    }
                }
            }
        }
    }

    fn parse_slash(&self, input: &str) -> Option<SlashAction> {
        match input {
            "/help" | "/?" => Some(SlashAction::Print(
                "Ctrl+D             — exit when the input line is empty\n\
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
                 /verbose           — toggle compact/verbose tool call log (default: compact)\n\
                 /save              — save current session to disk\n\
                 /sessions          — list saved sessions\n\
                 /resume <id>       — resume a saved session\n\
                 /read <path>       — read a file and add it to context (works without tool models)\n\
                 /glob <pat> [dir]  — list matching paths, add to context\n\
                 /tree [path] [n]   — directory tree (default . depth 3), add to context\n\
                 /compress          — show compression mode and context stats\n\
                 /compress always   — compress all tool outputs immediately\n\
                 /compress auto     — compress only when approaching context limit (default)\n\
                 /compress manual   — no auto-compression; use '/compress now' to trigger\n\
                 /compress now      — manually compress history right now\n\
                 /compress ai       — use the LLM to semantically summarize old messages\n\
                 (Broad \u{201c}read project / status\u{201d} questions auto-attach tree + key files when tools are off.)\n\
                 Chat / no tools: ```bash blocks — run offers; ```lang path/file.md — save uses path (confirm Y/n).\n\
                 Ctrl+C             — clear the current input line"
                    .into(),
            )),
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
                Some(SlashAction::Print(lines.join("\n")))
            }
            "/clear" => Some(SlashAction::ClearHistory),
            "/read" => Some(SlashAction::Print("Usage: /read <path>".into())),
            s if s.starts_with("/read ") => {
                let path = s.trim_start_matches("/read ").trim().to_string();
                if path.is_empty() {
                    Some(SlashAction::Print("Usage: /read <path>".into()))
                } else {
                    Some(SlashAction::ReadFile(path))
                }
            }
            "/glob" => Some(SlashAction::Print("Usage: /glob <pattern> [dir]".into())),
            s if s.starts_with("/glob ") => {
                let rest = s.trim_start_matches("/glob ");
                match parse_glob_slash_args(rest) {
                    Some((pattern, dir)) => Some(SlashAction::Glob { pattern, dir }),
                    None => Some(SlashAction::Print("Usage: /glob <pattern> [dir]".into())),
                }
            }
            s if s == "/tree" || s.starts_with("/tree ") => {
                let rest = s.strip_prefix("/tree").unwrap_or("").trim();
                let (path, depth) = parse_tree_slash_args(rest);
                Some(SlashAction::Tree { path, depth })
            }
            "/context" => Some(SlashAction::ContextShow),
            s if s.starts_with("/context ") => {
                let rest = s.trim_start_matches("/context ").trim();
                match rest {
                    "refresh" => Some(SlashAction::ContextRefresh),
                    _ => Some(SlashAction::Print(
                        "Usage: /context  or  /context refresh".into(),
                    )),
                }
            }
            "/model" => Some(SlashAction::ShowModel),
            s if s.starts_with("/model ") => {
                let rest = s.trim_start_matches("/model ").trim();
                match rest {
                    "" => Some(SlashAction::ShowModel),
                    "list" => Some(SlashAction::ListModels),
                    name => Some(SlashAction::SetModel(name.to_string())),
                }
            }
            "/mode" => Some(SlashAction::ShowMode),
            s if s.starts_with("/mode ") => {
                let rest = s.trim_start_matches("/mode ").trim();
                match rest {
                    "chat"  => Some(SlashAction::SetMode(SessionMode::Chat)),
                    "agent" => Some(SlashAction::SetMode(SessionMode::Agent)),
                    "plan"  => Some(SlashAction::SetMode(SessionMode::Plan)),
                    other => Some(SlashAction::Print(
                        format!("Unknown mode '{other}'. Valid: chat, agent, plan"),
                    )),
                }
            }
            "/verbose" => Some(SlashAction::ToggleVerboseTools),
            "/compress" => Some(SlashAction::CompressShow),
            s if s.starts_with("/compress ") => {
                let rest = s.trim_start_matches("/compress ").trim();
                match rest {
                    "now" => Some(SlashAction::CompressNow),
                    "ai" | "summarize" => Some(SlashAction::CompressAi),
                    other => match CompressionMode::from_str_loose(other) {
                        Some(mode) => Some(SlashAction::CompressSetMode(mode)),
                        None => Some(SlashAction::Print(
                            format!("Unknown compression mode '{other}'. Valid: always, auto, manual, now, ai"),
                        )),
                    },
                }
            }
            "/save" => Some(SlashAction::SaveSession),
            "/sessions" => Some(SlashAction::ListSessions),
            "/resume" => Some(SlashAction::Print("Usage: /resume <session-id>".into())),
            s if s.starts_with("/resume ") => {
                let id = s.trim_start_matches("/resume ").trim().to_string();
                if id.is_empty() {
                    Some(SlashAction::Print("Usage: /resume <session-id>".into()))
                } else {
                    Some(SlashAction::ResumeSession(id))
                }
            }
            _ => None,
        }
    }
}

fn format_age(unix_ts: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let delta = now.saturating_sub(unix_ts);
    if delta < 60 {
        "just now".into()
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86400)
    }
}

/// Map a fenced-code language tag to the most common file extension.
/// Returns an empty string for unknown languages.
fn lang_to_ext(lang: &str) -> &'static str {
    match lang.to_lowercase().as_str() {
        "rust" | "rs"                         => "rs",
        "python" | "py"                       => "py",
        "javascript" | "js"                   => "js",
        "typescript" | "ts"                   => "ts",
        "batch" | "bat" | "cmd"               => "bat",
        "powershell" | "pwsh" | "ps1"         => "ps1",
        "bash" | "sh" | "shell" | "zsh"       => "sh",
        "go"                                  => "go",
        "c"                                   => "c",
        "cpp" | "c++"                         => "cpp",
        "java"                                => "java",
        "ruby" | "rb"                         => "rb",
        "toml"                                => "toml",
        "json"                                => "json",
        "markdown" | "md"                     => "md",
        "yaml" | "yml"                        => "yaml",
        "html"                                => "html",
        "css"                                 => "css",
        "sql"                                 => "sql",
        "dockerfile"                          => "Dockerfile",
        _                                     => "",
    }
}
