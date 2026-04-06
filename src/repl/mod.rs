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

mod banner;
mod chat_only;
mod markdown;
use crate::ollama::{
    client::OllamaClient,
    types::{ChatOptions, LlmResponse, Message, ToolCallItem},
};
use crate::tools;

const SYSTEM_PROMPT: &str = "\
You are Ollero, a local code assistant powered by Ollama. \
You help with software engineering tasks. \
You have access to tools: read_file, write_file, edit_file, glob, grep, tree, bash. \
Use them to explore and modify the codebase when needed. \
Always prefer reading files before editing them. \
Be concise and precise.";

const SYSTEM_PROMPT_CHAT_ONLY: &str = "\
You are Ollero, a local code assistant. This session is in chat-only mode: \
the model cannot call tools (read_file, grep, etc.). Use the workspace snapshot below. \
For shell steps, put each command (or script) in a fenced block with language bash or sh — the app will offer to run them. \
For file changes, use a fenced block and put the target path on the opening fence line after the language \
(e.g. ```markdown SUMMARY.md or ```rust src/lib.rs), or as the first line inside the block as // path: rel/path.rs (or # path: for Python). \
That lets the app save without asking for a filename. They can enable native tools with /model and a tool-capable model (e.g. llama3.2). Be concise.";

const MAX_TOOL_ROUNDS: usize = 10;

const SYSTEM_PROMPT_PLAN: &str = "\
You are Ollero, a local code assistant powered by Ollama. \
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

/// Spinner on stderr while the model has not emitted visible text yet (Claude-style “Thinking…”).
fn spawn_thinking_spinner(active: Arc<AtomicBool>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        const FRAMES: &[char] = &['|', '/', '-', '\\'];
        let mut i = 0usize;
        let mut tick = tokio::time::interval(Duration::from_millis(120));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut stderr = io::stderr();
        while active.load(Ordering::Relaxed) {
            tick.tick().await;
            if !active.load(Ordering::Relaxed) {
                break;
            }
            let c = FRAMES[i % FRAMES.len()];
            i += 1;
            let _ = write!(
                stderr,
                "\r\x1b[K{}",
                banner::accent_dim(&format!("{c}  Thinking…"))
            );
            let _ = stderr.flush();
        }
        let _ = write!(stderr, "\r\x1b[K");
        let _ = stderr.flush();
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
    input: InputReader,
    permissions: PermissionStore,
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
}

impl Repl {
    pub fn new(config: Config, workspace_root: PathBuf) -> Self {
        let client = OllamaClient::new(&config.ollama_url, &config.model);
        let history = vec![Message::system(Self::compose_system_prompt(
            &workspace_root,
            &SessionMode::Agent,
            true,
        ))];
        Self {
            client,
            history,
            config,
            workspace_root,
            mode: SessionMode::Agent,
            model_supports_tools: true,
            input: InputReader::new(),
            permissions: PermissionStore::new(),
        }
    }

    fn compose_system_prompt(
        root: &std::path::Path,
        mode: &SessionMode,
        model_supports_tools: bool,
    ) -> String {
        let intro = match mode {
            SessionMode::Chat => SYSTEM_PROMPT_CHAT_ONLY,
            SessionMode::Agent => {
                if model_supports_tools { SYSTEM_PROMPT } else { SYSTEM_PROMPT_CHAT_ONLY }
            }
            SessionMode::Plan => {
                if model_supports_tools { SYSTEM_PROMPT_PLAN } else { SYSTEM_PROMPT_CHAT_ONLY }
            }
        };
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

    fn chat_options(&self) -> ChatOptions {
        ChatOptions {
            temperature: None,
            num_ctx: Some(self.config.context_size),
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        let mut stdout = io::stdout();

        banner::print_welcome(
            env!("CARGO_PKG_VERSION"),
            &self.client.model,
            &self.workspace_root,
        );

        loop {
            println!(
                "{}",
                banner::accent("────────────────────────────────────────────────────────────────────────")
            );
            let prompt = banner::accent(">").bold().to_string();
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

            self.history.push(Message::user(&input));
            self.run_agentic_loop(&mut stdout).await;
            println!();
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
            print!("{} ", banner::accent("●").bold());
            let _ = stdout.flush();

            let mut streamed_text = String::new();
            let options = self.chat_options();

            // In Chat mode, never pass tools. In Agent/Plan, use tools if model supports them.
            let use_tools = matches!(self.mode, SessionMode::Agent | SessionMode::Plan)
                && self.model_supports_tools;

            let tools_arg: Option<&[_]> = if use_tools { Some(&tools) } else { None };

            let spin_active = Arc::new(AtomicBool::new(true));
            let spin_on_text = spin_active.clone();
            let spinner = spawn_thinking_spinner(spin_active.clone());

            let mut result = self
                .client
                .chat(
                    &self.history,
                    tools_arg,
                    Some(options.clone()),
                    |chunk| {
                        if use_tools && !chunk.is_empty() {
                            spin_on_text.store(false, Ordering::SeqCst);
                        }
                        streamed_text.push_str(chunk);
                    },
                )
                .await;

            finish_thinking_spinner(&spin_active, spinner).await;

            // ── Auto-fallback: model doesn't support tools ───────────────────
            if let Err(ref e) = result {
                if self.model_supports_tools && e.to_string().contains("does not support tools") {
                    println!();
                    println!(
                        "{}",
                        format!(
                            "  ⚠ Model '{}' does not support tools — falling back to chat within this session.",
                            self.client.model
                        )
                        .yellow()
                    );
                    self.model_supports_tools = false;
                    self.rebuild_system_prompt();
                    streamed_text.clear();

                    let spin2 = Arc::new(AtomicBool::new(true));
                    let sp2 = spawn_thinking_spinner(spin2.clone());
                    result = self
                        .client
                        .chat(&self.history, None, Some(options), |chunk| {
                            streamed_text.push_str(chunk);
                        })
                        .await;
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
                    println!(
                        "{}",
                        format!(
                            "[tokens — prompt:{} completion:{} total:{}]",
                            stats.prompt_tokens, stats.completion_tokens, stats.total()
                        )
                        .dimmed()
                    );
                    self.history.push(Message::assistant(&content));
                    return;
                }

                Ok(LlmResponse::ToolCalls { calls, stats }) => {
                    println!();
                    if !calls.is_empty() {
                        println!(
                            "  {} {}",
                            banner::accent("◇"),
                            format!("Using {} tool(s)…", calls.len()).dimmed()
                        );
                        println!(
                            "{}",
                            format!(
                                "  [tokens — prompt:{} completion:{} total:{}]",
                                stats.prompt_tokens,
                                stats.completion_tokens,
                                stats.total()
                            )
                            .dimmed()
                        );
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

        let result = self
            .client
            .chat(&plan_messages, None, Some(options), |chunk| {
                plan_text.push_str(chunk);
            })
            .await;

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
        const PLAN_PROMPT: &str = "Execute this plan? [y/N]: ";
        let vis = PLAN_PROMPT.chars().count();
        let confirmed = match self.input.read_line(PLAN_PROMPT, vis, None) {
            Ok(Some(s)) => s.trim().to_lowercase().starts_with('y'),
            _ => false,
        };

        if !confirmed {
            self.history.pop(); // remove assistant plan
            self.history.pop(); // remove user message
            println!("{}", "  Plan rejected.".dimmed());
            return false;
        }

        // Tell the LLM to proceed
        self.history.push(Message::user("Proceed with the plan."));
        println!("{}", "  Executing…".dimmed());
        println!();
        let _ = stdout.flush();
        true
    }

    /// After a chat-only reply, offer to run extracted shell blocks (not shown as duplicate prose).
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
                    }
                    Err(e) => eprintln!("{}", format!("{e}").red()),
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

        for call in calls {
            let name = &call.function.name;
            let args = &call.function.arguments;

            // ── Permission gate for bash ────────────────────────────────────────
            if name == "bash" {
                let command = args["command"].as_str().unwrap_or("(unknown)");

                if !self.permissions.is_session_granted(command) {
                    println!();
                    println!("  {} bash command:", "⚠".yellow().bold());
                    println!("  {}", format!("> {command}").bold());

                    const PERM_PROMPT: &str =
                        "  Allow? [y] once  [s] session  [n] deny: ";
                    let vis = PERM_PROMPT.chars().count();

                    let decision = match self.input.read_line(PERM_PROMPT, vis, None) {
                        Ok(Some(s)) => PermissionStore::parse_input(&s),
                        Ok(None) | Err(_) => {
                            use crate::permissions::Decision;
                            Decision::Deny
                        }
                    };

                    use crate::permissions::Decision;
                    match decision {
                        Decision::AllowSession => {
                            self.permissions.grant_session(command);
                            // fall through to dispatch
                        }
                        Decision::AllowOnce => {
                            // fall through to dispatch, no grant stored
                        }
                        Decision::Deny => {
                            println!("  {}", "✗ denied".red());
                            results.push(Message::tool_result(
                                name.clone(),
                                "Permission denied: user rejected the bash command.",
                            ));
                            continue;
                        }
                    }
                }
            }
            // ───────────────────────────────────────────────────────────────────

            // Print: ⚙ read_file(path="src/main.rs") — Claude-style visible tool use
            print!("    {} {}(", "⚙".cyan(), name.bold());
            if let Some(obj) = args.as_object() {
                let summary: Vec<String> = obj
                    .iter()
                    .take(2)
                    .map(|(k, v)| {
                        let val = v.as_str().unwrap_or("…");
                        let short = if val.len() > 50 { &val[..50] } else { val };
                        format!("{k}={short:?}")
                    })
                    .collect();
                print!("{}", summary.join(", ").dimmed());
            }
            print!(") ");
            let _ = stdout.flush();

            let output = match tools::dispatch(name, args).await {
                Ok(out) => {
                    println!("{}", "✓".green());
                    out
                }
                Err(e) => {
                    println!("{}", "✗".red());
                    format!("Error executing {name}: {e}")
                }
            };

            results.push(Message::tool_result(name.clone(), output));
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
            _ => None,
        }
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
