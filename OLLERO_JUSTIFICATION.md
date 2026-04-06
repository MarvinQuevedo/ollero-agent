# Ollero — Feature Justification & Resource Analysis

> Why each feature exists, what uses AI vs pure software, and how we optimize local resource usage.

---

## Table of Contents

1. [AI vs Pure Software — Complete Map](#1-ai-vs-pure-software--complete-map)
2. [Feature-by-Feature Justification](#2-feature-by-feature-justification)
3. [Resource Usage Profile](#3-resource-usage-profile)
4. [Optimization Strategies](#4-optimization-strategies)
5. [Processing Cost Matrix](#5-processing-cost-matrix)

---

## 1. AI vs Pure Software — Complete Map

Every feature in Ollero falls into one of three categories:

```
┌─────────────────────────────────────────────────────────────────┐
│                                                                 │
│  PURE SOFTWARE (no AI, no LLM calls)                           │
│  ─────────────────────────────────────                          │
│  These run instantly, cost zero GPU/CPU inference time,         │
│  and are deterministic. They are the backbone.                  │
│                                                                 │
│    - TUI rendering (ratatui)                                    │
│    - File reading (read_file tool)                              │
│    - File searching (glob, grep tools)                          │
│    - Directory tree (tree tool)                                 │
│    - File editing (edit_file — exact string replace)            │
│    - File writing (write_file)                                  │
│    - Bash execution (bash tool — just spawn a process)          │
│    - Permission evaluation (rule matching, grant lookup)        │
│    - Permission storage (JSON read/write)                       │
│    - Diff computation (similar crate — Myers algorithm)         │
│    - Diff rendering (coloring, formatting)                      │
│    - Undo system (file backup/restore)                          │
│    - Session save/load (JSON serialization)                     │
│    - Session listing (read directory, sort by date)             │
│    - Config parsing (.Ollero.toml, config.toml)                  │
│    - Config TUI tab (display, edit settings)                    │
│    - Token counting (heuristic or tiktoken)                     │
│    - Token usage display (stats, gauge bars)                    │
│    - Slash command parsing (/model, /undo, etc.)                │
│    - Project detection (scan for Cargo.toml, package.json)      │
│    - Gitignore parsing (ignore crate)                           │
│    - Markdown rendering (pulldown-cmark → terminal spans)       │
│    - Syntax highlighting (syntect — static grammar matching)    │
│    - HTML cleaning for web_fetch (scraper crate)                │
│    - Clipboard copy (arboard)                                   │
│    - Mouse handling (crossterm events)                          │
│    - Scrollbar rendering (geometry math)                        │
│    - Virtual scrolling (offset calculations)                    │
│    - Render cache (hash map + eviction)                         │
│    - Desktop notifications (notify-rust)                        │
│    - MCP server process management (spawn, stdio pipe)          │
│    - MCP tool discovery (JSON-RPC protocol)                     │
│    - MCP tool routing (name lookup, forward params)             │
│    - CLI argument parsing (clap)                                │
│    - Ollama health check (HTTP GET)                             │
│    - Model listing (HTTP GET /api/tags)                         │
│    - Web search HTTP request (send query, parse response)       │
│    - Web fetch HTTP request (download page)                     │
│    - Streaming response parsing (JSON line-by-line)             │
│    - Input handling (keyboard events, paste detection)          │
│    - Structured logging (tracing crate)                         │
│                                                                 │
│  AI-POWERED (requires LLM inference via Ollama)                │
│  ──────────────────────────────────────────                     │
│  These consume GPU time and VRAM. Each call costs seconds       │
│  of inference and tokens from the context window.               │
│                                                                 │
│    - Conversation responses (core chat)                         │
│    - Tool selection (LLM decides which tool to call)            │
│    - Tool argument generation (LLM fills tool parameters)       │
│    - Multi-step reasoning (LLM chains multiple tool calls)      │
│    - History compression (LLM summarizes old messages)          │
│    - Session title generation (LLM creates short title)         │
│    - "Explain command" in confirmations (LLM explains risk)     │
│                                                                 │
│  HYBRID (mostly software, AI assists occasionally)             │
│  ──────────────────────────────────────────                     │
│  Software does the heavy lifting; AI is called only when        │
│  the software layer can't decide on its own.                    │
│                                                                 │
│    - Context resolution (software ranks files, AI could         │
│      re-rank if budget allows)                                  │
│    - Smart truncation (software truncates by lines, AI          │
│      could pick the most relevant section if budget allows)     │
│    - Project map summary (software detects languages and        │
│      structure, AI generates a natural language summary)        │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### The Ratio

```
Pure Software:  ~40 features  (85% of all features)
AI-Powered:      ~7 features  (12% of all features)
Hybrid:          ~3 features  ( 3% of all features)

This means: Ollero is a software tool FIRST, that happens to use AI
for the parts where human-like reasoning is genuinely needed.
The AI is the brain; everything else is the body.
```

---

## 2. Feature-by-Feature Justification

### 2.1 TUI Rendering (Pure Software)

**Why it exists:** The user needs to see the conversation, diffs, tool results, and confirmations in a structured, readable format. A raw terminal with print statements would be unusable for multi-turn conversations with tool calls.

**Why not AI:** Rendering is deterministic. Given the same content and terminal width, the output must always be identical. There is no ambiguity, no reasoning — just layout math. AI would be slower, non-deterministic, and wasteful here.

**Processing cost:** LOW. Ratatui uses a double-buffer that only redraws changed cells. Virtual scrolling skips offscreen messages. The render cache prevents re-computation of unchanged blocks.

---

### 2.2 File Operations: read_file, glob, grep, tree (Pure Software)

**Why they exist:** The LLM cannot see your filesystem. These tools are its eyes. Without them, the LLM would have to guess file contents, which is useless.

**Why not AI:** Reading a file is a syscall. Searching with regex is a deterministic algorithm (finite automaton). AI cannot improve on `O(n)` text search — it would be slower and less accurate.

**Why read_file instead of including all files in context:**
- A typical project has 500+ files. Including all of them would consume the entire context window before the user even asks a question.
- `read_file` lets the LLM request only what it needs, on demand.
- This is the single biggest optimization for local models with small context windows.

**Processing cost:** NEGLIGIBLE. File I/O is microseconds. `ignore` crate uses the same optimized walker as `ripgrep`.

---

### 2.3 File Editing: edit_file, write_file (Pure Software)

**Why they exist:** The LLM needs to modify code. Without these, it can only tell you what to change — you'd have to copy-paste manually.

**Why exact string replacement (not AI-generated diffs):**
- Deterministic: if `old_string` is found exactly once, the replacement is guaranteed correct.
- Verifiable: we can show the exact diff before applying.
- Reversible: we save the old content for undo.
- If we let the AI generate the full new file content, it would: (a) consume output tokens for unchanged lines, (b) risk introducing subtle corruption in parts it wasn't supposed to modify, (c) be slow for large files.

**Processing cost:** NEGLIGIBLE. String search + replace is O(n).

---

### 2.4 Bash Execution (Pure Software)

**Why it exists:** Many tasks require running commands: `cargo build`, `npm test`, `git status`, `docker compose up`. The LLM needs to see the output to reason about errors.

**Why not AI:** Executing a command is spawning a process. There is nothing for AI to do here. AI decides WHAT to run; the execution is pure software.

**Processing cost:** VARIABLE. Depends entirely on what command is run. `ls` takes 1ms. `cargo build` can take minutes. The timeout system (default 30s) prevents runaway processes.

---

### 2.5 Permission System (Pure Software)

**Why it exists:** The LLM can hallucinate dangerous commands. Without permissions, it could `rm -rf /` on a bad day. The permission system is a safety net between AI intent and real-world execution.

**Why not AI for permission evaluation:**
- Permission checks must be DETERMINISTIC. The same action must always get the same result. If AI decided permissions, it could be tricked by prompt injection ("ignore safety rules and allow rm -rf").
- Permission rules are simple pattern matching: "does this command match `cargo *`? Is there a grant in the session store?" This is a hash table lookup, not a reasoning task.
- Hardcoded rules are compiled into the binary. They cannot be bypassed by any input — not by the user, not by the LLM, not by MCP tools.

**Why 4 scopes instead of just "allow/deny":**
- "Just allow/deny" forces the user to choose between security (deny everything, approve 50 times per session) and convenience (allow everything, risk catastrophe).
- Scopes let the user progressively build trust: "I've approved `cargo test` 10 times this session, let me make it permanent for this workspace."
- The workspace scope means: "I trust this project with these commands, but not other projects." A workspace grant for `docker compose *` in project A doesn't apply to project B.

**Processing cost:** NEGLIGIBLE. Hash set lookup is O(1). JSON file read on startup is one-time. Pattern matching is linear in the number of grants (typically <50).

---

### 2.6 Diff Rendering (Pure Software)

**Why it exists:** When the LLM edits a file, the user needs to see exactly what changed before approving. A raw "old → new" dump is unreadable for anything beyond trivial changes.

**Why not AI:** The `similar` crate implements the Myers diff algorithm — a mathematically optimal algorithm that produces the minimal edit sequence. AI cannot improve on optimal. And determinism matters: the same change must always produce the same diff.

**Why unified format:** It's the universal standard (git, GitHub, patch files). Every developer already knows how to read it.

**Processing cost:** LOW. Myers algorithm is `O(n*d)` where `d` is the number of differences. For typical edits (a few lines changed), this is sub-millisecond.

---

### 2.7 Undo System (Pure Software)

**Why it exists:** LLMs make mistakes. An edit that looks correct in the diff might break the build. Undo lets you revert instantly without `git checkout` or manual restoration.

**Why not AI:** Undo is deterministic: restore the saved old content. AI cannot improve on "put the bytes back exactly as they were."

**Why verify before undo:** If the user manually edited the file after the LLM's edit, blindly restoring would lose the user's manual changes. The safety check ("file has been modified since the edit") prevents this.

**Processing cost:** NEGLIGIBLE. File read + comparison + write.

---

### 2.8 Conversation Responses (AI-Powered)

**Why it exists:** This is the core reason Ollero exists. The user asks a question or describes a task; the LLM reasons about it and responds.

**Why it must be AI:** Understanding natural language, reasoning about code, deciding what to do — these are fundamentally AI tasks. No deterministic algorithm can "understand" what the user means by "fix the auth bug."

**Processing cost:** HIGH. This is the most expensive operation in Ollero.
- Input tokens: system prompt + project meta + conversation history + tool results = 2K-30K tokens
- Output tokens: response text + tool calls = 100-2K tokens
- GPU time: 2-30 seconds depending on model size and input length
- VRAM: constant while model is loaded (7B = ~6GB, 14B = ~12GB)

**Optimization:** Minimize input tokens by smart context management. Every token saved in context = faster inference.

---

### 2.9 Tool Selection & Argument Generation (AI-Powered)

**Why it exists:** The LLM needs to decide WHICH tool to call and with WHAT arguments. "Search for the auth bug" → `grep { pattern: "auth.*error", path: "src/" }`.

**Why it must be AI:** Translating intent ("find the bug") into structured tool calls (`grep` with specific regex) requires understanding. This is function calling — a core LLM capability.

**Why not hardcoded rules:** The user's intent is infinitely variable. "Fix the bug" could mean grep → read → edit. "Add a test" could mean read → write. "Deploy" could mean bash. No finite rule set covers this.

**Processing cost:** INCLUDED in conversation responses. Tool calls are generated as part of the LLM's output. The cost is the output tokens for the JSON tool call (typically 50-200 tokens per call).

---

### 2.10 Multi-Step Reasoning (AI-Powered)

**Why it exists:** Real tasks require multiple steps. "Fix the auth bug" might be: grep for "auth" → read the file → understand the bug → edit the fix → run tests → check results → report. The LLM orchestrates this entire chain.

**Why it must be AI:** Each step depends on the results of the previous step. The LLM reads grep output, decides which file to open, reads it, understands the bug, formulates a fix. This is reasoning, not pattern matching.

**Processing cost:** HIGH. Each step in the chain is a separate LLM call (with the full conversation history). A 5-step chain = 5 inference calls. This is the most GPU-intensive use pattern.

**Optimization:** 
- Keep tool results concise (truncate large outputs)
- Don't re-send unchanged context between steps
- Compress history when it exceeds soft limit

---

### 2.11 History Compression (AI-Powered)

**Why it exists:** After 20+ turns, the conversation history exceeds the model's context window. Without compression, Ollero would simply stop working. Compression lets conversations continue indefinitely.

**Why it must be AI:** Summarizing a conversation requires understanding what was discussed, what decisions were made, what's still relevant. A deterministic "keep last N messages" loses critical context (e.g., the user said "we're using hexagonal architecture" 15 messages ago).

**Why not just truncate:** Truncation is dumb — it throws away the oldest messages regardless of importance. The first message might contain the most important instruction ("don't use unwrap, always use Result"). Summarization preserves the key facts.

**Processing cost:** MEDIUM. One LLM call to summarize ~10 old messages into ~200 tokens. Happens infrequently (every ~20 turns, or when approaching the soft limit).

**Optimization:** 
- Only compress when approaching soft limit, not proactively
- Use a shorter prompt specifically for summarization (no tools, no project context)
- Cache the summary — don't re-summarize already-summarized content

---

### 2.12 Session Title Generation (AI-Powered)

**Why it exists:** When listing sessions (`/sessions`), the user needs to recognize which conversation is which. Timestamps and IDs are meaningless. "Fix auth token expiry bug" is instantly recognizable.

**Why it must be AI:** Generating a meaningful 5-10 word summary of a conversation requires understanding. Heuristics like "first sentence of first message" often produce garbage.

**Processing cost:** LOW. One short LLM call (~100 input tokens, ~20 output tokens). Happens once per session, after the first exchange. Uses a minimal prompt without tools.

**Optimization:**
- Call this ONCE, after the first user-assistant exchange
- Use low temperature (0.1) for deterministic titles
- If the model is busy (streaming a response), queue it for later
- If title generation fails, fall back to timestamp

---

### 2.13 Session Save/Resume/List (Pure Software)

**Why it exists:** Without persistence, closing the terminal kills the conversation forever. Session management lets you resume exactly where you left off — crucial for multi-hour tasks.

**Why not AI:** Serializing a struct to JSON and writing it to disk is a syscall. Listing files in a directory is a syscall. No reasoning needed.

**Processing cost:** LOW. JSON serialization of a typical session is ~100KB. Write to SSD takes <1ms. Auto-save after each turn adds negligible overhead.

---

### 2.14 Token Tracking (Pure Software)

**Why it exists:** Even with local models, the user needs to know:
- How much context window is consumed (am I near the limit?)
- How fast the model is generating (tokens/second)
- How long inference is taking

**Why not AI:** Ollama provides `eval_count`, `eval_duration`, `prompt_eval_count` in every response. Parsing these numbers is arithmetic, not reasoning.

**Processing cost:** NEGLIGIBLE. Integer addition and division per response.

---

### 2.15 MCP Protocol Support (Pure Software + AI routing)

**Why it exists:** Ollero can't have built-in tools for every possible service (databases, Jira, GitHub, Slack, internal APIs). MCP lets external servers expose tools that the LLM can discover and use — infinite extensibility without modifying Ollero's code.

**What's software:** Process spawning, JSON-RPC communication, tool discovery, config parsing, connection management.

**What's AI:** The LLM decides WHEN to call an MCP tool and with what arguments, just like built-in tools.

**Processing cost:** The MCP infrastructure is NEGLIGIBLE (process management, JSON-RPC). The AI cost is the same as any tool call (~100-200 output tokens per call).

---

### 2.16 Web Search & Fetch (Pure Software)

**Why it exists:** The LLM's knowledge has a cutoff date. For current docs, new APIs, or error messages, it needs internet access.

**Why not AI for the search/fetch itself:** HTTP requests are software. HTML parsing is software. The AI decides WHAT to search and INTERPRETS the results — the fetching itself is pure I/O.

**Why HTML cleaning is important:** A raw HTML page is 90% boilerplate (nav, footer, scripts, ads). Feeding raw HTML to the LLM wastes context tokens on garbage. The `scraper` crate extracts the readable content — typically reducing a 100KB page to 5KB of useful text.

**Processing cost:** NETWORK-BOUND. The HTTP request takes 200ms-2s depending on the server. HTML parsing is <10ms. Token cost is the cleaned content included in the next LLM call.

---

### 2.17 Markdown Rendering (Pure Software)

**Why it exists:** LLMs naturally output markdown. Without rendering, the user sees raw `**bold**` and `\`\`\`code blocks\`\`\``. Rendered markdown with syntax highlighting is dramatically more readable.

**Why not AI:** Markdown is a formal grammar. `pulldown-cmark` parses it deterministically in O(n). Syntax highlighting via `syntect` uses TextMate grammars — another formal system. No ambiguity, no reasoning needed.

**Processing cost:** LOW. Parsing is linear. Syntax highlighting is slightly more expensive (grammar matching) but still sub-millisecond for typical code blocks. The render cache ensures each block is only rendered once.

---

### 2.18 Virtual Scrolling & Render Cache (Pure Software)

**Why they exist:** Without virtual scrolling, a 200-message conversation would render all 200 messages every frame — even though only ~30 are visible. Without caching, every resize would re-render everything.

**Why not AI:** These are standard UI optimization techniques. Viewport culling is geometry math. Cache eviction is LRU. AI would add latency to something that needs to be instant (16ms frame budget).

**Processing cost:** The WHOLE POINT is to reduce processing cost. Virtual scrolling skips ~85% of messages. Caching prevents redundant re-rendering. These are net savings, not costs.

---

### 2.19 Context Resolution (Hybrid)

**Why it exists:** When the user says "fix the auth bug", Ollero needs to figure out which files to include in the LLM's context. Including everything wastes tokens. Including nothing leaves the LLM blind.

**The software part (no AI):**
- Extract file paths mentioned in the user's message ("look at src/auth.rs")
- Find recently accessed files in this session
- Match keywords against file names ("auth" → `src/auth/`, `tests/auth_test.rs`)
- Rank by recency, path match, and file size
- Fit to token budget

**The AI part (optional, budget-dependent):**
- If the software heuristics aren't confident (no files match, vague query), ask the LLM: "Which files should I look at for this task?"
- This costs one LLM call but prevents the more expensive scenario of the LLM calling `glob` + `read_file` 10 times to find the right file.

**Processing cost:** Software ranking is NEGLIGIBLE. The optional AI re-ranking costs ~500 input tokens + ~100 output tokens.

---

### 2.20 Smart Truncation (Hybrid)

**Why it exists:** A file with 5000 lines can't fit in context. We need to show the most relevant portion.

**The software part (no AI):**
- If the user mentioned a function name, find that function and include ±50 lines around it
- Always keep imports and struct/type declarations (first ~30 lines)
- Replace omitted sections with `... (N lines omitted) ...`

**The AI part (rare):**
- If the software can't determine relevance (no specific function mentioned, file is uniformly important), the LLM could be asked to pick the most relevant section
- This is rarely needed in practice; the software heuristics handle >95% of cases

**Processing cost:** Software truncation is O(n) string operations. The rare AI call would cost ~1000 input tokens.

---

### 2.21 Project Map Summary (Hybrid)

**Why it exists:** The system prompt includes a brief description of the project so the LLM understands the codebase before the user even asks anything.

**The software part:** Detect languages, count files, list directories, identify key files, read Cargo.toml/package.json for dependencies.

**The AI part:** Generate a 2-3 sentence natural language summary from the detected facts. "This is a Rust web server using Actix and Diesel, with 45 source files across 8 modules. The main entry point is src/main.rs."

**Processing cost:** Software detection is NEGLIGIBLE (directory walk + file name matching). The one-time AI summary costs ~300 input tokens + ~50 output tokens. Done once on startup, cached for the entire session.

---

## 3. Resource Usage Profile

### 3.1 What Consumes Resources

```
RESOURCE: GPU / VRAM
────────────────────
Used by: LLM inference (conversation, tool calling, compression, title gen)
Constant: VRAM occupied by loaded model (6-24GB depending on model)
Variable: Inference time scales with input + output token count
Peak: Multi-step tool chains (5+ chained LLM calls)
Idle: Zero GPU usage when waiting for user input or executing tools

RESOURCE: CPU
─────────────
Used by: TUI rendering, file I/O, diff computation, regex search, 
         JSON parsing, HTTP requests, process spawning
Peak: Syntax highlighting large files, regex search across large codebase
Idle: Minimal (event loop waiting for input)

RESOURCE: RAM
─────────────
Used by: Conversation history, render cache, undo stack, session data,
         file contents during search
Constant: ~50MB base (TUI + data structures + loaded config)
Variable: Render cache (budget-limited), undo stack (50 entries max),
          conversation history (compressed to soft limit)
Peak: Loading a large file for reading (~10MB max)

RESOURCE: DISK
──────────────
Used by: Session persistence, permission storage, config files, logs
Typical: 100KB-1MB per session file
Growth: ~100 sessions * 1MB = ~100MB max (configurable limit)

RESOURCE: NETWORK
─────────────────
Used by: Ollama API (localhost), web search, web fetch, MCP servers
Ollama: Streaming JSON over localhost (very fast, no internet)
Web: Only when user triggers web_search/web_fetch
MCP: Depends on server type (stdio = local, HTTP = network)
```

### 3.2 Processing Time Breakdown (typical session)

```
Operation                      Time         Frequency        Total Impact
─────────────────────────────────────────────────────────────────────────
LLM response (streaming)       3-30s        Every turn       ██████████ HIGH
LLM tool call chain            5-60s        Complex tasks    ██████████ HIGH
File read (read_file)          <1ms         Very frequent    ░░░░░░░░░░ NEGLIGIBLE
Regex search (grep)            10-500ms     Frequent         █░░░░░░░░░ LOW
Glob search                    5-100ms      Frequent         ░░░░░░░░░░ NEGLIGIBLE
File edit (edit_file)          <1ms         Moderate         ░░░░░░░░░░ NEGLIGIBLE
Bash command                   1ms-120s     Moderate         █████░░░░░ VARIABLE
Diff computation               <1ms         Per edit         ░░░░░░░░░░ NEGLIGIBLE
Markdown rendering             1-5ms        Per message      ░░░░░░░░░░ NEGLIGIBLE
Syntax highlighting            5-50ms       Per code block   █░░░░░░░░░ LOW
TUI frame render               1-3ms        60fps            █░░░░░░░░░ LOW
History compression            3-10s        Every ~20 turns  ██░░░░░░░░ MEDIUM (rare)
Session save                   <1ms         Every turn       ░░░░░░░░░░ NEGLIGIBLE
Permission check               <0.1ms       Every tool call  ░░░░░░░░░░ NEGLIGIBLE
Web search                     200ms-2s     Occasional       ██░░░░░░░░ MEDIUM (rare)
HTML cleaning                  5-50ms       Per web fetch    ░░░░░░░░░░ NEGLIGIBLE
Token counting                 <0.1ms       Every response   ░░░░░░░░░░ NEGLIGIBLE
Project detection              50-200ms     Once on startup  ░░░░░░░░░░ NEGLIGIBLE
```

---

## 4. Optimization Strategies

### 4.1 Minimize LLM Input Tokens (biggest impact)

Every token in the input must be processed by the model. Reducing input tokens directly reduces inference time.

```
Strategy                                 Tokens Saved      Impact
──────────────────────────────────────────────────────────────────
Smart file truncation (show relevant     500-5000/file      ████████
  section, not entire file)
History compression (summarize old       2000-20000         ██████████
  messages into 200-token summary)
Concise system prompt (no fluff,         200-500            ██
  every word earns its place)
Tool result truncation (cap output       500-5000/result    ████████
  at 10K chars, summarize rest)
Never include binary files               1000-100000        ██████████
Respect .gitignore (never scan           Prevents waste     ██████████
  node_modules, dist, .git)
Project meta summary instead of          500-2000           ████
  full file list
Evict tool results after LLM uses them   500-5000/result    ████████
```

### 4.2 Minimize LLM Output Tokens

Output tokens are generated one by one. Fewer = faster response.

```
Strategy                                 Impact
──────────────────────────────────────────────────
System prompt instructs concise          ████████
  responses ("be brief, no fluff")
Tool schemas are minimal (short          ██
  descriptions, no redundant fields)
Temperature 0.1 for code (less           ███
  exploration = more direct answers)
```

### 4.3 Minimize LLM Call Count

Each call has overhead (HTTP roundtrip, prompt processing, context loading).

```
Strategy                                 Impact
──────────────────────────────────────────────────
Batch tool results (if LLM made 3        ████████
  tool calls, send all 3 results in
  one message, not 3 separate calls)
Pre-include obviously relevant           ██████
  files in context (avoid the LLM
  needing to call read_file to
  discover what it already needs)
Session title: generate only ONCE        ██
  after first exchange, not every turn
History compression: batch (compress      ████
  10 messages at once, not one by one)
```

### 4.4 TUI Performance

```
Strategy                                 Impact
──────────────────────────────────────────────────
Virtual scrolling (skip offscreen        ██████████
  messages entirely)
Render cache (don't re-render            ████████
  unchanged message blocks)
Progressive remeasure (max 12 msgs       ██████
  re-measured per frame)
Incremental markdown (only re-parse      ████████
  new chunks during streaming, not
  entire accumulated text)
Double-buffered rendering (ratatui       ████
  only redraws changed cells)
Viewport culling (0 margin — exact       ████
  skip of offscreen content)
```

### 4.5 File System Performance

```
Strategy                                 Impact
──────────────────────────────────────────────────
ignore crate (same engine as ripgrep,    ██████████
  skips .gitignore patterns, no
  scanning of node_modules etc.)
Lazy file reading (don't read files      ████████
  until the LLM specifically asks)
File content cache within session        ████
  (don't re-read unchanged files)
Limit grep results (cap at 50            ████
  matches, not unlimited)
```

---

## 5. Processing Cost Matrix

Summary of which features are cheap vs expensive to run:

```
┌─────────────────────────────────────────────────────────────────────┐
│                                                                     │
│  ZERO COST (pure data structures, no computation worth measuring)  │
│  ─────────────────────────────────────────────────────────          │
│  Permission check          Slash command parse                     │
│  Token count arithmetic    Config read                             │
│  Session save/load         Undo push/pop                           │
│  Grant storage             Clipboard copy                          │
│  MCP tool routing          CLI arg parse                           │
│                                                                     │
│  LOW COST (<10ms, instant feel)                                    │
│  ─────────────────────────────                                     │
│  File read                 File edit/write                         │
│  Diff computation          Glob search                             │
│  TUI frame render          Markdown parse                          │
│  Project detection         Gitignore parse                         │
│  HTML cleaning             JSON serialization                      │
│  Directory tree            Notification send                       │
│                                                                     │
│  MEDIUM COST (10ms-500ms, noticeable if repeated)                  │
│  ────────────────────────────────────────────────                   │
│  Grep (large codebase)     Syntax highlighting                     │
│  Render cache rebuild      MCP tool discovery                      │
│  Web search HTTP call      Model list HTTP call                    │
│                                                                     │
│  HIGH COST (1s-60s, user waits, GPU busy)                          │
│  ────────────────────────────────────────                           │
│  LLM conversation response                                        │
│  LLM multi-step tool chain                                         │
│  LLM history compression                                           │
│  Web page fetch + clean (network latency)                          │
│  Bash long-running commands (cargo build, docker, etc.)            │
│                                                                     │
│  CONSTANT COST (always consuming while Ollero runs)                 │
│  ─────────────────────────────────────────────────                  │
│  VRAM for loaded Ollama model (6-24GB)                             │
│  RAM for TUI + data structures (~50MB)                             │
│  CPU for event loop (minimal, <1%)                                 │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

### The Core Insight

```
┌─────────────────────────────────────────────────────────────────┐
│                                                                 │
│  95% of Ollero's code handles pure software tasks that cost      │
│  nearly zero processing time.                                   │
│                                                                 │
│  5% of Ollero's code (the Ollama client) drives 99% of the      │
│  processing cost.                                               │
│                                                                 │
│  Therefore: EVERY optimization that reduces LLM token count     │
│  or LLM call count has 100x more impact than optimizing         │
│  any software feature.                                          │
│                                                                 │
│  The Context Manager is the most important piece of code        │
│  in the entire project. A good Context Manager can cut          │
│  inference time in half. A bad one doubles it.                  │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```
