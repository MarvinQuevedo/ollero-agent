#!/usr/bin/env -S npx tsx
// @ts-nocheck

/**
 * Minimal CLI to run Ollama prompts by task.
 *
 * Usage examples:
 *   npx tsx scripts/ollero-cli.ts list
 *   npx tsx scripts/ollero-cli.ts show T01
 *   npx tsx scripts/ollero-cli.ts run T01 --model qwen3.5:9b
 *   npx tsx scripts/ollero-cli.ts ask "explica src/repl/mod.rs"
 */

import { mkdir, readdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { exec as execCb, execFile as execFileCb } from "node:child_process";
import path from "node:path";
import { promisify } from "node:util";

type Command = "list" | "show" | "run" | "ask" | "gen-prompts" | "help";

type Task = {
  id: string;
  title: string;
  section: string;
  prompt: string;
};

type CliOptions = {
  model: string;
  url: string;
  tasksFile: string;
  system?: string;
  outDir: string;
  dryRun: boolean;
  autonomous: boolean;
  allowBash: boolean;
  allowWeb: boolean;
  allowFs: boolean;
  maxRounds: number;
  cmdTimeoutMs: number;
  llmTimeoutMs: number;
  keepRuns: number;
  verbose: boolean;
  readyToken?: string;
  batchFile?: string;
  continueOnError: boolean;
  promptsOut: string;
  promptCount: number;
  autoRecoverOllama: boolean;
  ollamaRestartCmd?: string;
  ollamaRecoverRetries: number;
  requireWrite: boolean;
  requireValidation?: string;
  requireValidationSuccess: boolean;
  prewriteMaxInspectRounds: number;
  writeAllowRegex?: string;
  prevalidationMaxPostWriteRounds: number;
};

type RunOutcome = {
  ok: boolean;
  finalText: string;
  analysisWarnings: string[];
  messages: ChatMessage[];
};

const DEFAULT_MODEL = "qwen3.5:9b";
const DEFAULT_URL = "http://localhost:11434";
const DEFAULT_TASKS_FILE = "TASKS_OLLERO_TOOL_ACTIONS.md";
const DEFAULT_OUT_DIR = ".ollero-cli/runs";
const DEFAULT_MAX_ROUNDS = 10;
const DEFAULT_CMD_TIMEOUT_MS = 60_000;
const DEFAULT_LLM_TIMEOUT_MS = 90_000;
const DEFAULT_KEEP_RUNS = 20;
const MAX_TOOL_OUTPUT_CHARS = 12_000;
const DEFAULT_PROMPTS_OUT = "validation/generated-prompts.txt";
const DEFAULT_PROMPT_COUNT = 8;
const DEFAULT_OLLAMA_RECOVER_RETRIES = 2;
const DEFAULT_PREWRITE_MAX_INSPECT_ROUNDS = 4;
const DEFAULT_PREVALIDATION_MAX_POSTWRITE_ROUNDS = 2;

const execAsync = promisify(execCb);
const execFileAsync = promisify(execFileCb);

type ChatMessage = {
  role: "system" | "user" | "assistant" | "tool";
  content: string;
  tool_name?: string;
  tool_calls?: ToolCall[];
};

type ToolCall = {
  function: {
    name: string;
    arguments: unknown;
  };
};

type ToolDefinition = {
  type: "function";
  function: {
    name: string;
    description: string;
    parameters: unknown;
  };
};

type ChatResponse = {
  message?: {
    content?: string;
    tool_calls?: ToolCall[];
  };
  prompt_eval_count?: number;
  eval_count?: number;
};

const AUTONOMOUS_SYSTEM_PROMPT = [
  "You are Ollero operating in autonomous mode.",
  "You are allowed to execute shell commands and use internet tools.",
  "Use tools when they are necessary to complete the task with evidence.",
  "Do not ask for confirmation before using tools.",
  "The runtime OS is Windows.",
  "For shell actions, use Windows PowerShell syntax only.",
  "Avoid Unix-only commands like find/head/pwd/command -v.",
  "Use equivalents like Get-ChildItem, Select-Object -First, Get-Location.",
  "You can read and edit files inside the current workspace using file tools.",
  "Prefer targeted edits: read_file -> replace_in_file/write_file -> verify with shell.",
  "Always respond in English.",
  "Keep actions focused and return a concise final answer with what you executed.",
].join(" ");

function parseArgs(argv: string[]) {
  const [commandRaw, ...rest] = argv;
  const command: Command = (commandRaw as Command) || "help";
  const positional: string[] = [];
  const flags = new Map<string, string | true>();

  for (let i = 0; i < rest.length; i += 1) {
    const token = rest[i];
    if (token.startsWith("--")) {
      const key = token.slice(2);
      const next = rest[i + 1];
      if (!next || next.startsWith("--")) {
        flags.set(key, true);
      } else {
        flags.set(key, next);
        i += 1;
      }
      continue;
    }
    positional.push(token);
  }

  const options: CliOptions = {
    model: String(flags.get("model") ?? DEFAULT_MODEL),
    url: String(flags.get("url") ?? DEFAULT_URL),
    tasksFile: String(flags.get("tasks") ?? DEFAULT_TASKS_FILE),
    outDir: String(flags.get("out") ?? DEFAULT_OUT_DIR),
    system: typeof flags.get("system") === "string" ? String(flags.get("system")) : undefined,
    dryRun: flags.has("dry-run"),
    autonomous: flags.has("autonomous"),
    allowBash: flags.has("autonomous") || flags.has("allow-bash"),
    allowWeb: flags.has("autonomous") || flags.has("allow-web"),
    allowFs: flags.has("autonomous") || flags.has("allow-fs"),
    maxRounds: Number(flags.get("max-rounds") ?? DEFAULT_MAX_ROUNDS),
    cmdTimeoutMs: Number(flags.get("cmd-timeout-ms") ?? DEFAULT_CMD_TIMEOUT_MS),
    llmTimeoutMs: Number(flags.get("llm-timeout-ms") ?? DEFAULT_LLM_TIMEOUT_MS),
    keepRuns: Number(flags.get("keep-runs") ?? DEFAULT_KEEP_RUNS),
    verbose: flags.has("verbose") || flags.has("autonomous"),
    readyToken: typeof flags.get("ready-token") === "string" ? String(flags.get("ready-token")) : undefined,
    batchFile: typeof flags.get("batch-file") === "string" ? String(flags.get("batch-file")) : undefined,
    continueOnError: !flags.has("stop-on-error"),
    promptsOut: String(flags.get("prompts-out") ?? DEFAULT_PROMPTS_OUT),
    promptCount: Number(flags.get("prompt-count") ?? DEFAULT_PROMPT_COUNT),
    autoRecoverOllama: !flags.has("no-ollama-recover"),
    ollamaRestartCmd:
      typeof flags.get("ollama-restart-cmd") === "string"
        ? String(flags.get("ollama-restart-cmd"))
        : undefined,
    ollamaRecoverRetries: Number(
      flags.get("ollama-recover-retries") ?? DEFAULT_OLLAMA_RECOVER_RETRIES,
    ),
    requireWrite: flags.has("require-write"),
    requireValidation:
      typeof flags.get("require-validation") === "string"
        ? String(flags.get("require-validation"))
        : undefined,
    requireValidationSuccess: !flags.has("allow-failed-validation"),
    prewriteMaxInspectRounds: Number(
      flags.get("prewrite-max-inspect-rounds") ?? DEFAULT_PREWRITE_MAX_INSPECT_ROUNDS,
    ),
    writeAllowRegex:
      typeof flags.get("write-allow-regex") === "string"
        ? String(flags.get("write-allow-regex"))
        : undefined,
    prevalidationMaxPostWriteRounds: Number(
      flags.get("prevalidation-max-postwrite-rounds") ??
        DEFAULT_PREVALIDATION_MAX_POSTWRITE_ROUNDS,
    ),
  };

  return { command, positional, options };
}

function printHelp() {
  console.log(
    [
      "Ollero CLI (simple task runner for Ollama)",
      "",
      "Commands:",
      "  list                         List task IDs from markdown file",
      "  show <TASK_ID>               Show task title and prompt",
      "  run <TASK_ID> [--dry-run]    Send task prompt to Ollama",
      "  ask \"<prompt>\"               Send a direct prompt to Ollama",
      "  gen-prompts                   Ask Ollama to generate validation prompts from git changes",
      "  help                         Show this message",
      "",
      "Flags:",
      `  --model <name>   Default: ${DEFAULT_MODEL}`,
      `  --url <url>      Default: ${DEFAULT_URL}`,
      `  --tasks <file>   Default: ${DEFAULT_TASKS_FILE}`,
      `  --out <dir>      Default: ${DEFAULT_OUT_DIR}`,
      "  --system <text>  Optional system prompt",
      "  --autonomous     Enable autonomous tool loop (bash + web tools)",
      "  --allow-bash     Allow shell commands without full autonomous mode",
      "  --allow-web      Allow internet tools without full autonomous mode",
      "  --allow-fs       Allow file read/write/edit tools",
      `  --max-rounds <n> Default: ${DEFAULT_MAX_ROUNDS}`,
      `  --cmd-timeout-ms Default: ${DEFAULT_CMD_TIMEOUT_MS}`,
      `  --llm-timeout-ms Default: ${DEFAULT_LLM_TIMEOUT_MS}`,
      `  --keep-runs <n>  Default: ${DEFAULT_KEEP_RUNS} (log retention)`,
      "  --verbose        Show step-by-step execution logs",
      "  --ready-token <t> Require final answer to include token, else exits non-zero",
      "  --batch-file <p> Run many prompts sequentially (ask mode)",
      "  --stop-on-error  Stop batch execution on first failure",
      `  --prompts-out <p> Default: ${DEFAULT_PROMPTS_OUT}`,
      `  --prompt-count <n> Default: ${DEFAULT_PROMPT_COUNT}`,
      "  --no-ollama-recover Disable automatic Ollama recovery",
      "  --ollama-restart-cmd <cmd> Optional command to restart Ollama process/service",
      `  --ollama-recover-retries <n> Default: ${DEFAULT_OLLAMA_RECOVER_RETRIES}`,
      "  --require-write  Require at least one file write/edit in the run",
      "  --require-validation <regex> Require a matching validation command/output (e.g. \"cargo check|cargo test\")",
      "  --allow-failed-validation Accept validation attempts even if command fails",
      `  --prewrite-max-inspect-rounds <n> Default: ${DEFAULT_PREWRITE_MAX_INSPECT_ROUNDS}`,
      `  --prevalidation-max-postwrite-rounds <n> Default: ${DEFAULT_PREVALIDATION_MAX_POSTWRITE_ROUNDS}`,
      "  --write-allow-regex <regex> Restrict write/replace paths (e.g. \"^src/|^scripts/\")",
      "  --dry-run        Print request without sending",
    ].join("\n"),
  );
}

function logStep(options: CliOptions, message: string): void {
  if (!options.verbose) return;
  const ts = new Date().toISOString();
  console.log(`[${ts}] ${message}`);
}

function resolveWorkspacePath(inputPath: string): string {
  const base = process.cwd();
  const resolved = path.resolve(base, inputPath);
  const rel = path.relative(base, resolved);
  if (rel.startsWith("..") || path.isAbsolute(rel)) {
    throw new Error("Path escapes workspace root.");
  }
  return resolved;
}

async function loadTasks(tasksFile: string): Promise<Task[]> {
  const raw = await readFile(tasksFile, "utf8");
  const sections = raw
    .split(/\r?\n(?=## TASK )/g)
    .filter((s) => s.trimStart().startsWith("## TASK "));

  const tasks: Task[] = [];
  for (const section of sections) {
    const headerMatch = section.match(/^## TASK (T\d+)\s*-\s*(.+)$/m);
    if (!headerMatch) continue;

    const id = headerMatch[1].trim();
    const title = headerMatch[2].trim();
    const promptMatch = section.match(/### PROMPT[\s\S]*?```(?:text|prompt)?\r?\n([\s\S]*?)\r?\n```/m);
    const prompt = promptMatch?.[1]?.trim() ?? "";
    tasks.push({ id, title, section, prompt });
  }
  return tasks;
}

function parseBatchPrompts(raw: string): string[] {
  const separators = /\r?\n---\r?\n/g;
  if (separators.test(raw)) {
    return raw
      .split(separators)
      .map((s) => s.trim())
      .filter(Boolean);
  }
  return raw
    .split(/\r?\n/g)
    .map((line) => line.trim())
    .filter((line) => line.length > 0 && !line.startsWith("#"));
}

function analyzeResponseIssues(finalText: string): string[] {
  const warnings: string[] = [];
  const t = finalText.toLowerCase();
  const patterns: Array<[RegExp, string]> = [
    [/i cannot execute/i, "Model claims it cannot execute commands."],
    [/i don't have (the )?ability to interact with terminal/i, "Model claims no terminal capability."],
    [/would you like me to provide/i, "Model drifted into generic advisory mode."],
    [/as an ai assistant/i, "Model produced generic assistant disclaimer."],
    [/does not exist in the current workspace/i, "Model used incorrect workspace path."],
    [/path .* does not exist/i, "Model reported path-not-found."],
    [/no files were found or modified/i, "Model did not perform requested file updates."],
  ];
  for (const [re, label] of patterns) {
    if (re.test(t)) warnings.push(label);
  }
  return warnings;
}

async function loadBatchPrompts(filePath: string): Promise<string[]> {
  const full = resolveWorkspacePath(filePath);
  const raw = await readFile(full, "utf8");
  const prompts = parseBatchPrompts(raw);
  if (!prompts.length) {
    throw new Error(`No prompts found in batch file: ${filePath}`);
  }
  return prompts;
}

async function collectGitContext(options: CliOptions): Promise<string> {
  const status = await runBash("git status --short", options.cmdTimeoutMs);
  const diff = await runBash("git diff", options.cmdTimeoutMs);
  const staged = await runBash("git diff --cached", options.cmdTimeoutMs);
  const log = await runBash("git log --oneline -10", options.cmdTimeoutMs);
  return [
    "## git status --short",
    status,
    "",
    "## git diff",
    clipOutput(diff, 20_000),
    "",
    "## git diff --cached",
    clipOutput(staged, 20_000),
    "",
    "## git log --oneline -10",
    log,
  ].join("\n");
}

async function pingOllama(baseUrl: string): Promise<boolean> {
  try {
    const res = await fetch(`${baseUrl}/api/tags`);
    return res.ok;
  } catch {
    return false;
  }
}

async function listRunningOllamaModels(baseUrl: string): Promise<string[]> {
  try {
    const res = await fetch(`${baseUrl}/api/ps`);
    if (!res.ok) return [];
    const data = (await res.json()) as { models?: Array<{ name?: string }> };
    return (data.models ?? [])
      .map((m) => String(m.name ?? "").trim())
      .filter(Boolean);
  } catch {
    return [];
  }
}

async function unloadModelViaApi(baseUrl: string, model: string): Promise<void> {
  // keep_alive: 0 asks Ollama to unload the model from memory.
  const payload = {
    model,
    prompt: "",
    stream: false,
    keep_alive: 0,
  };
  const res = await fetch(`${baseUrl}/api/generate`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
  if (!res.ok) {
    const body = await res.text();
    throw new Error(`Failed to unload model '${model}': ${res.status} ${body}`);
  }
}

async function recoverOllama(options: CliOptions, reason: string): Promise<void> {
  logStep(options, `ollama recover start: ${reason}`);

  const running = await listRunningOllamaModels(options.url);
  if (running.length > 0) {
    logStep(options, `ollama recover: unloading ${running.length} model(s) via API`);
  }
  for (const model of running) {
    try {
      await unloadModelViaApi(options.url, model);
      logStep(options, `ollama recover: unloaded ${model}`);
    } catch (err) {
      logStep(
        options,
        `ollama recover: unload failed for ${model}: ${
          err instanceof Error ? err.message : String(err)
        }`,
      );
    }
  }

  // Optional hard restart hook if user provides a platform-specific command.
  if (options.ollamaRestartCmd) {
    try {
      logStep(options, `ollama recover: restart command -> ${options.ollamaRestartCmd}`);
      await runBash(options.ollamaRestartCmd, options.cmdTimeoutMs);
    } catch (err) {
      logStep(
        options,
        `ollama recover: restart command failed: ${
          err instanceof Error ? err.message : String(err)
        }`,
      );
    }
  }

  // Wait briefly and verify health.
  for (let i = 0; i < 5; i += 1) {
    if (await pingOllama(options.url)) {
      logStep(options, "ollama recover: health check OK");
      return;
    }
    await new Promise((r) => setTimeout(r, 1000));
  }
  throw new Error("Ollama recovery failed: health check did not recover.");
}

async function callOllamaWithRetry(
  options: CliOptions,
  messages: ChatMessage[],
  tools?: ToolDefinition[],
  retries = 3,
): Promise<ChatResponse> {
  let lastErr: unknown;
  for (let attempt = 1; attempt <= retries; attempt += 1) {
    try {
      return await callOllama(
        options.url,
        options.model,
        messages,
        tools,
        options.llmTimeoutMs,
      );
    } catch (err) {
      lastErr = err;
      const msg = err instanceof Error ? err.message : String(err);
      logStep(options, `ollama retry ${attempt}/${retries} failed: ${msg}`);
      if (options.autoRecoverOllama && attempt <= options.ollamaRecoverRetries) {
        try {
          await recoverOllama(options, `request failure: ${msg}`);
        } catch (recoverErr) {
          logStep(
            options,
            `ollama recover failed: ${
              recoverErr instanceof Error ? recoverErr.message : String(recoverErr)
            }`,
          );
        }
      }
      if (attempt < retries) {
        await new Promise((r) => setTimeout(r, 1000 * attempt));
      }
    }
  }
  throw lastErr instanceof Error ? lastErr : new Error(String(lastErr));
}

function normalizeGeneratedPrompts(raw: string): string {
  const cleaned = raw
    .replace(/```[\s\S]*?```/g, (m) => m.replace(/```[a-zA-Z]*/g, "").replace(/```/g, ""))
    .trim();
  if (!cleaned) return "";
  const blocks = cleaned
    .split(/\r?\n---\r?\n/g)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
  if (!blocks.length) return "";
  return `${blocks.join("\n---\n")}\n`;
}

function parseGeneratedPrompts(raw: string): string[] {
  return normalizeGeneratedPrompts(raw)
    .split(/\r?\n---\r?\n/g)
    .map((s) => s.trim())
    .filter(Boolean);
}

function validateGeneratedPrompts(prompts: string[], minCount: number): string[] {
  const problems: string[] = [];
  if (prompts.length < minCount) {
    problems.push(`Expected at least ${minCount} prompts, got ${prompts.length}.`);
  }
  const badPatterns = [
    /as an ai assistant/i,
    /would you like/i,
    /siguientes pasos/i,
    /resumen del estado/i,
    /git commit/i,
    /tabla|table/i,
  ];
  prompts.forEach((p, idx) => {
    if (p.length < 40) problems.push(`Prompt ${idx + 1} is too short.`);
    if (!/(work|validate|run|check|edit|update|test)/i.test(p)) {
      problems.push(`Prompt ${idx + 1} lacks actionable validation verbs.`);
    }
    if (badPatterns.some((re) => re.test(p))) {
      problems.push(`Prompt ${idx + 1} contains advisory/report style text.`);
    }
  });
  return problems;
}

async function generateValidationPrompts(options: CliOptions): Promise<void> {
  logStep(options, "collecting git context for prompt generation");
  const gitContext = await collectGitContext(options);
  const attempts = [
    [
      "You generate high-signal validation prompts for autonomous code agents.",
      "Always respond in English.",
      "Output only prompt blocks separated by a line containing exactly ---",
      "No markdown, no headers, no tables, no explanations, no code fences.",
      "Each block must be one executable prompt.",
    ].join(" "),
    [
      "STRICT MODE.",
      "Return ONLY plain text prompts separated by ---.",
      "No intro, no outro, no bullets, no markdown.",
      "Each prompt must start with an imperative verb (Analyze, Validate, Edit, Run, Check, Update).",
    ].join(" "),
  ];

  let acceptedPrompts: string[] = [];
  let lastProblems: string[] = [];
  for (let i = 0; i < attempts.length; i += 1) {
    const messages: ChatMessage[] = [
      { role: "system", content: attempts[i] },
      {
        role: "user",
        content: [
          `Based on this repository git context, generate ${options.promptCount} validation prompts.`,
          "Prompts must test: analysis quality, concrete file edits, command execution, follow-up updates, and regression safety.",
          "Prefer prompts that can be run sequentially in shared session mode.",
          "",
          gitContext,
        ].join("\n"),
      },
    ];
    const result = await callOllamaWithRetry(options, messages, undefined, 3);
    const raw = result.message?.content ?? "";
    const prompts = parseGeneratedPrompts(raw);
    const problems = validateGeneratedPrompts(prompts, options.promptCount);
    if (problems.length === 0) {
      acceptedPrompts = prompts;
      break;
    }
    lastProblems = problems;
    logStep(options, `gen-prompts attempt ${i + 1} rejected: ${problems.join(" | ")}`);
  }

  if (!acceptedPrompts.length) {
    logStep(options, "falling back to iterative one-prompt generation");
    const iterative: string[] = [];
    for (let i = 0; i < options.promptCount; i += 1) {
      const used = iterative.map((p, n) => `${n + 1}. ${p}`).join("\n");
      const messages: ChatMessage[] = [
        {
          role: "system",
          content: [
            "Generate exactly one validation prompt in English.",
            "Output only the prompt text (no markdown, no quotes, no bullets).",
            "Start with an imperative verb: Analyze, Validate, Edit, Run, Check, or Update.",
          ].join(" "),
        },
        {
          role: "user",
          content: [
            `Generate prompt ${i + 1} of ${options.promptCount}.`,
            "Use current git context and make it executable for this repository.",
            "Avoid duplicates with previous prompts.",
            used ? `Previous prompts:\n${used}` : "Previous prompts: (none)",
            "",
            gitContext,
          ].join("\n"),
        },
      ];
      const result = await callOllamaWithRetry(options, messages, undefined, 3);
      const candidate = (result.message?.content ?? "").trim().replace(/\r?\n+/g, " ");
      const candidateProblems = validateGeneratedPrompts([candidate], 1);
      if (candidateProblems.length === 0) {
        iterative.push(candidate);
      } else {
        const fallbackPrompt =
          "Validate the latest Rust changes by checking modified files, running cargo check and cargo test where applicable, and report pass/fail with concrete evidence.";
        iterative.push(fallbackPrompt);
      }
    }
    acceptedPrompts = iterative;
  }
  const normalized = `${acceptedPrompts.join("\n---\n")}\n`;

  const outPath = resolveWorkspacePath(options.promptsOut);
  await mkdir(path.dirname(outPath), { recursive: true });
  await writeFile(outPath, normalized, "utf8");
  console.log(`Generated prompts saved to: ${options.promptsOut}`);
  console.log(`Prompt count: ${acceptedPrompts.length}`);
}

function requireTask(tasks: Task[], id: string): Task {
  const task = tasks.find((t) => t.id.toLowerCase() === id.toLowerCase());
  if (!task) {
    throw new Error(`Task '${id}' not found in tasks file.`);
  }
  if (!task.prompt) {
    throw new Error(`Task '${id}' does not contain a PROMPT block.`);
  }
  return task;
}

async function callOllama(
  url: string,
  model: string,
  messages: ChatMessage[],
  tools?: ToolDefinition[],
  timeoutMs = DEFAULT_LLM_TIMEOUT_MS,
): Promise<ChatResponse> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), Math.max(1000, timeoutMs));
  const payload = { model, stream: false, messages, tools };
  let response: Response;
  try {
    response = await fetch(`${url}/api/chat`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
      signal: controller.signal,
    });
  } catch (err) {
    if (controller.signal.aborted) {
      throw new Error(`Ollama request timed out after ${timeoutMs} ms`);
    }
    throw err;
  } finally {
    clearTimeout(timer);
  }

  if (!response.ok) {
    const body = await response.text();
    throw new Error(`Ollama error ${response.status}: ${body}`);
  }

  return response.json() as Promise<ChatResponse>;
}

function nowStamp(): string {
  return new Date().toISOString().replace(/[:.]/g, "-");
}

async function saveRun(outDir: string, name: string, text: string) {
  await mkdir(outDir, { recursive: true });
  const filename = `${name}-${nowStamp()}.md`;
  const full = path.join(outDir, filename);
  await writeFile(full, text, "utf8");
  return full;
}

async function pruneOldRuns(outDir: string, keepRuns: number): Promise<number> {
  const keep = Math.max(1, keepRuns);
  const entries = await readdir(outDir);
  const files = entries.filter((f) => f.toLowerCase().endsWith(".md"));
  if (files.length <= keep) return 0;

  const dated = await Promise.all(
    files.map(async (file) => {
      const full = path.join(outDir, file);
      const s = await stat(full);
      return { full, mtimeMs: s.mtimeMs };
    }),
  );
  dated.sort((a, b) => b.mtimeMs - a.mtimeMs); // newest first
  const toDelete = dated.slice(keep);
  await Promise.all(toDelete.map((f) => rm(f.full, { force: true })));
  return toDelete.length;
}

function toolsForOptions(options: CliOptions): ToolDefinition[] {
  const tools: ToolDefinition[] = [];
  if (options.allowBash) {
    tools.push({
      type: "function",
      function: {
        name: "bash",
        description: "Execute shell commands on the local machine.",
        parameters: {
          type: "object",
          properties: {
            command: { type: "string" },
          },
          required: ["command"],
        },
      },
    });
  }
  if (options.allowWeb) {
    tools.push({
      type: "function",
      function: {
        name: "web_search",
        description: "Search the web and return top links/snippets.",
        parameters: {
          type: "object",
          properties: {
            query: { type: "string" },
          },
          required: ["query"],
        },
      },
    });
    tools.push({
      type: "function",
      function: {
        name: "web_fetch",
        description: "Fetch URL content and return text excerpt.",
        parameters: {
          type: "object",
          properties: {
            url: { type: "string" },
          },
          required: ["url"],
        },
      },
    });
  }
  if (options.allowFs) {
    tools.push({
      type: "function",
      function: {
        name: "read_file",
        description: "Read a UTF-8 text file from the workspace.",
        parameters: {
          type: "object",
          properties: {
            path: { type: "string" },
          },
          required: ["path"],
        },
      },
    });
    tools.push({
      type: "function",
      function: {
        name: "write_file",
        description: "Write UTF-8 content to a file in workspace (overwrite).",
        parameters: {
          type: "object",
          properties: {
            path: { type: "string" },
            content: { type: "string" },
          },
          required: ["path", "content"],
        },
      },
    });
    tools.push({
      type: "function",
      function: {
        name: "replace_in_file",
        description: "Replace one exact string in a UTF-8 file in workspace.",
        parameters: {
          type: "object",
          properties: {
            path: { type: "string" },
            old_string: { type: "string" },
            new_string: { type: "string" },
          },
          required: ["path", "old_string", "new_string"],
        },
      },
    });
  }
  return tools;
}

function stringifyUnknown(value: unknown): string {
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

function clipOutput(text: string, maxChars = MAX_TOOL_OUTPUT_CHARS): string {
  if (text.length <= maxChars) return text;
  return (
    text.slice(0, maxChars) +
    `\n\n... [truncated ${text.length - maxChars} chars to limit token usage]`
  );
}

async function runBash(command: string, timeoutMs: number): Promise<string> {
  const isWindows = process.platform === "win32";
  const { stdout, stderr } = isWindows
    ? await execFileAsync("powershell", ["-NoProfile", "-Command", command], {
        timeout: timeoutMs,
        maxBuffer: 1024 * 1024 * 8,
      })
    : await execAsync(command, {
        shell: true,
        timeout: timeoutMs,
        maxBuffer: 1024 * 1024 * 8,
      });
  const out = [stdout, stderr].filter(Boolean).join("");
  return clipOutput(out.trim() || "(no output)");
}

function stripHtmlTags(html: string): string {
  return html
    .replace(/<script[\s\S]*?<\/script>/gi, " ")
    .replace(/<style[\s\S]*?<\/style>/gi, " ")
    .replace(/<[^>]+>/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

async function webSearch(query: string): Promise<string> {
  const endpoint = `https://duckduckgo.com/html/?q=${encodeURIComponent(query)}`;
  const response = await fetch(endpoint, {
    headers: {
      "User-Agent": "ollero-cli/0.1",
    },
  });
  if (!response.ok) {
    throw new Error(`web_search failed (${response.status})`);
  }
  const html = await response.text();
  const items: string[] = [];
  const re = /<a[^>]*class="[^"]*result__a[^"]*"[^>]*href="([^"]+)"[^>]*>([\s\S]*?)<\/a>/gi;
  let m: RegExpExecArray | null;
  while ((m = re.exec(html)) && items.length < 5) {
    const href = m[1];
    const title = stripHtmlTags(m[2]);
    if (title) {
      items.push(`- ${title} -> ${href}`);
    }
  }
  if (items.length === 0) {
    const fallback = stripHtmlTags(html).slice(0, 1200);
    return clipOutput(`No structured results parsed.\n${fallback}`);
  }
  return clipOutput(items.join("\n"));
}

async function webFetch(url: string): Promise<string> {
  const response = await fetch(url, {
    headers: {
      "User-Agent": "ollero-cli/0.1",
    },
  });
  if (!response.ok) {
    throw new Error(`web_fetch failed (${response.status})`);
  }
  const text = await response.text();
  return clipOutput(stripHtmlTags(text).slice(0, 5000));
}

async function readTextFileInWorkspace(inputPath: string): Promise<string> {
  const fullPath = resolveWorkspacePath(inputPath);
  const text = await readFile(fullPath, "utf8");
  return clipOutput(text);
}

async function writeTextFileInWorkspace(inputPath: string, content: string): Promise<string> {
  const fullPath = resolveWorkspacePath(inputPath);
  await mkdir(path.dirname(fullPath), { recursive: true });
  await writeFile(fullPath, content, "utf8");
  return `Wrote ${content.length} chars to ${inputPath}`;
}

async function replaceInFileInWorkspace(
  inputPath: string,
  oldString: string,
  newString: string,
): Promise<string> {
  const fullPath = resolveWorkspacePath(inputPath);
  const text = await readFile(fullPath, "utf8");
  const index = text.indexOf(oldString);
  if (index < 0) {
    return "No change: old_string not found.";
  }
  const next = text.replace(oldString, newString);
  await writeFile(fullPath, next, "utf8");
  return `Replaced one occurrence in ${inputPath}`;
}

async function dispatchTool(
  call: ToolCall,
  options: CliOptions,
): Promise<{ name: string; output: string }> {
  const name = call.function.name;
  const args = call.function.arguments ?? {};
  const argsObj =
    typeof args === "object" && args !== null
      ? (args as Record<string, unknown>)
      : ({ value: args } as Record<string, unknown>);

  if (name === "bash") {
    if (!options.allowBash) {
      return { name, output: "Permission denied: bash is disabled." };
    }
    const command = String(argsObj.command ?? "");
    if (!command.trim()) {
      return { name, output: "Error: missing 'command' argument." };
    }
    logStep(options, `tool:bash start -> ${command}`);
    const output = await runBash(command, options.cmdTimeoutMs);
    logStep(options, `tool:bash done (${output.length} chars)`);
    return { name, output };
  }

  if (name === "web_search") {
    if (!options.allowWeb) {
      return { name, output: "Permission denied: web tools are disabled." };
    }
    const query = String(argsObj.query ?? "");
    if (!query.trim()) {
      return { name, output: "Error: missing 'query' argument." };
    }
    logStep(options, `tool:web_search start -> ${query}`);
    const output = await webSearch(query);
    logStep(options, `tool:web_search done (${output.length} chars)`);
    return { name, output };
  }

  if (name === "web_fetch") {
    if (!options.allowWeb) {
      return { name, output: "Permission denied: web tools are disabled." };
    }
    const url = String(argsObj.url ?? "");
    if (!url.trim()) {
      return { name, output: "Error: missing 'url' argument." };
    }
    logStep(options, `tool:web_fetch start -> ${url}`);
    const output = await webFetch(url);
    logStep(options, `tool:web_fetch done (${output.length} chars)`);
    return { name, output };
  }

  if (name === "read_file") {
    if (!options.allowFs) {
      return { name, output: "Permission denied: fs tools are disabled." };
    }
    const filePath = String(argsObj.path ?? "");
    if (!filePath.trim()) {
      return { name, output: "Error: missing 'path' argument." };
    }
    logStep(options, `tool:read_file start -> ${filePath}`);
    const output = await readTextFileInWorkspace(filePath);
    logStep(options, `tool:read_file done (${output.length} chars)`);
    return { name, output };
  }

  if (name === "write_file") {
    if (!options.allowFs) {
      return { name, output: "Permission denied: fs tools are disabled." };
    }
    const filePath = String(argsObj.path ?? "");
    const content = String(argsObj.content ?? "");
    if (!filePath.trim()) {
      return { name, output: "Error: missing 'path' argument." };
    }
    logStep(options, `tool:write_file start -> ${filePath}`);
    const output = await writeTextFileInWorkspace(filePath, content);
    logStep(options, "tool:write_file done");
    return { name, output };
  }

  if (name === "replace_in_file") {
    if (!options.allowFs) {
      return { name, output: "Permission denied: fs tools are disabled." };
    }
    const filePath = String(argsObj.path ?? "");
    const oldString = String(argsObj.old_string ?? "");
    const newString = String(argsObj.new_string ?? "");
    if (!filePath.trim() || !oldString) {
      return { name, output: "Error: missing 'path' or 'old_string' argument." };
    }
    logStep(options, `tool:replace_in_file start -> ${filePath}`);
    const output = await replaceInFileInWorkspace(filePath, oldString, newString);
    logStep(options, "tool:replace_in_file done");
    return { name, output };
  }

  return { name, output: `Unknown tool: ${name}` };
}

async function runPrompt(
  userPrompt: string,
  runName: string,
  options: CliOptions,
  sessionMessages?: ChatMessage[],
): Promise<RunOutcome> {
  logStep(options, `run start -> ${runName}`);
  const tools = toolsForOptions(options);
  const messages: ChatMessage[] = sessionMessages ? [...sessionMessages] : [];
  const systemPrompt =
    options.system ??
    (options.autonomous
      ? AUTONOMOUS_SYSTEM_PROMPT
      : undefined);
  const systemPromptWithReady =
    options.readyToken && systemPrompt
      ? `${systemPrompt}\nWhen all requested work is complete and validated, include this exact token in your final answer: ${options.readyToken}`
      : options.readyToken
        ? `When all requested work is complete and validated, include this exact token in your final answer: ${options.readyToken}`
        : systemPrompt;
  const workspaceConstrainedPrompt = [
    systemPromptWithReady ?? "",
    `Workspace root is: ${process.cwd()}`,
    "All file paths must be relative to this workspace root.",
    "Do not use /workspace paths unless they truly exist in this environment.",
  ]
    .filter(Boolean)
    .join("\n");

  if (workspaceConstrainedPrompt && messages.length === 0) {
    messages.push({ role: "system", content: workspaceConstrainedPrompt });
  }
  messages.push({ role: "user", content: userPrompt });

  if (options.dryRun) {
    console.log(
      `[dry-run] model=${options.model} url=${options.url} autonomous=${options.autonomous} allowBash=${options.allowBash} allowWeb=${options.allowWeb} allowFs=${options.allowFs}`,
    );
    console.log(userPrompt);
    return { ok: true, finalText: userPrompt, analysisWarnings: [], messages };
  }

  const trace: string[] = [];
  let finalText = "";
  let totalPrompt = 0;
  let totalCompletion = 0;
  let totalToolCalls = 0;
  let writeOps = 0;
  let validationHits = 0;
  let validationSuccessHits = 0;
  let validationFailureHits = 0;
  let validationRegex: RegExp | null = null;
  let writeAllowRegex: RegExp | null = null;
  if (options.requireValidation) {
    try {
      validationRegex = new RegExp(options.requireValidation, "i");
    } catch {
      validationRegex = new RegExp(
        options.requireValidation.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"),
        "i",
      );
    }
  }
  if (options.writeAllowRegex) {
    try {
      writeAllowRegex = new RegExp(options.writeAllowRegex);
    } catch {
      writeAllowRegex = null;
    }
  }

  for (let round = 1; round <= Math.max(1, options.maxRounds); round += 1) {
    logStep(options, `round ${round}: calling model ${options.model}`);
    const result = await callOllamaWithRetry(
      options,
      messages,
      tools.length ? tools : undefined,
      3,
    );
    totalPrompt += result.prompt_eval_count ?? 0;
    totalCompletion += result.eval_count ?? 0;

    const assistantContent = result.message?.content ?? "";
    const toolCalls = result.message?.tool_calls ?? [];
    messages.push({ role: "assistant", content: assistantContent, tool_calls: toolCalls });

    if (assistantContent.trim()) {
      logStep(options, `round ${round}: assistant text (${assistantContent.length} chars)`);
      finalText = assistantContent;
      trace.push(`## Assistant round ${round}\n\n${assistantContent}\n`);
    }

    if (!toolCalls.length) {
      if (
        validationRegex &&
        validationFailureHits > 0 &&
        validationSuccessHits === 0 &&
        round < Math.max(1, options.maxRounds)
      ) {
        const followup =
          "Validation failed in previous attempt. You must now edit files to fix the validation errors and then run cargo check or cargo test again. Do not stop with summaries.";
        messages.push({ role: "user", content: followup });
        logStep(
          options,
          `round ${round}: no tool calls but validation previously failed, forcing follow-up`,
        );
        continue;
      }
      logStep(options, `round ${round}: no tool calls, stopping loop`);
      break;
    }

    logStep(options, `round ${round}: received ${toolCalls.length} tool call(s)`);
    totalToolCalls += toolCalls.length;
    trace.push(`## Tool calls round ${round}\n\n${stringifyUnknown(toolCalls)}\n`);

    for (const call of toolCalls) {
      try {
        const forcedWriteMode =
          options.requireWrite &&
          writeOps === 0 &&
          round > Math.max(0, options.prewriteMaxInspectRounds);
        const forcedValidationMode =
          !!validationRegex &&
          writeOps > 0 &&
          validationHits === 0 &&
          round >
            Math.max(
              0,
              options.prewriteMaxInspectRounds + options.prevalidationMaxPostWriteRounds,
            );
        if (forcedWriteMode) {
          const toolName = call.function.name;
          const isEditTool = toolName === "write_file" || toolName === "replace_in_file";
          if (!isEditTool) {
            const blocked =
              "Action blocked: you must perform a real file edit now using write_file or replace_in_file before any more inspect commands.";
            trace.push(`### Tool ${toolName}\n\n${blocked}\n`);
            messages.push({
              role: "tool",
              tool_name: toolName,
              content: blocked,
            });
            logStep(
              options,
              `forced-write-mode blocked tool '${toolName}' at round ${round}`,
            );
            continue;
          }
        }
        if (forcedValidationMode) {
          const toolName = call.function.name;
          const argsObj =
            typeof call.function.arguments === "object" && call.function.arguments !== null
              ? (call.function.arguments as Record<string, unknown>)
              : {};
          const commandText = String(argsObj.command ?? "");
          const isValidatingBash =
            toolName === "bash" &&
            validationRegex &&
            validationRegex.test(commandText);
          if (!isValidatingBash) {
            const blocked =
              "Action blocked: you must run a validation command now (e.g., cargo check or cargo test) before other actions.";
            trace.push(`### Tool ${toolName}\n\n${blocked}\n`);
            messages.push({
              role: "tool",
              tool_name: toolName,
              content: blocked,
            });
            logStep(
              options,
              `forced-validation-mode blocked tool '${toolName}' at round ${round}`,
            );
            continue;
          }
        }
        if (
          options.readyToken &&
          call.function.name === "write_file" &&
          typeof call.function.arguments === "object" &&
          call.function.arguments !== null &&
          String((call.function.arguments as Record<string, unknown>).path ?? "") === options.readyToken
        ) {
          const blocked = `Blocked unsafe write_file target equal to ready token: ${options.readyToken}`;
          trace.push(`### Tool write_file\n\n${blocked}\n`);
          messages.push({
            role: "tool",
            tool_name: "write_file",
            content: blocked,
          });
          continue;
        }
        if (
          writeAllowRegex &&
          (call.function.name === "write_file" || call.function.name === "replace_in_file")
        ) {
          const argsObj =
            typeof call.function.arguments === "object" && call.function.arguments !== null
              ? (call.function.arguments as Record<string, unknown>)
              : {};
          const candidatePath = String(argsObj.path ?? "");
          if (!writeAllowRegex.test(candidatePath)) {
            const blocked = `Blocked write path '${candidatePath}': does not match write-allow-regex '${options.writeAllowRegex}'.`;
            trace.push(`### Tool ${call.function.name}\n\n${blocked}\n`);
            messages.push({
              role: "tool",
              tool_name: call.function.name,
              content: blocked,
            });
            logStep(options, `write path blocked -> ${candidatePath}`);
            continue;
          }
        }
        let commandText = "";
        if (
          validationRegex &&
          call.function.name === "bash" &&
          typeof call.function.arguments === "object" &&
          call.function.arguments !== null
        ) {
          commandText = String(
            (call.function.arguments as Record<string, unknown>).command ?? "",
          );
          if (validationRegex.test(commandText)) {
            validationHits += 1;
          }
        }
        const { name, output } = await dispatchTool(call, options);
        trace.push(`### Tool ${name}\n\n${output}\n`);
        if (name === "write_file" || name === "replace_in_file") {
          if (!/No change: old_string not found\./i.test(output)) {
            writeOps += 1;
          }
        }
        if (validationRegex && name === "bash") {
          if (validationRegex.test(commandText) || validationRegex.test(output)) {
            validationSuccessHits += 1;
          }
        }
        messages.push({
          role: "tool",
          tool_name: name,
          content: output,
        });
      } catch (err) {
        if (
          validationRegex &&
          call.function.name === "bash" &&
          typeof call.function.arguments === "object" &&
          call.function.arguments !== null
        ) {
          const commandText = String(
            (call.function.arguments as Record<string, unknown>).command ?? "",
          );
          if (validationRegex.test(commandText)) {
            validationHits += 1;
            validationFailureHits += 1;
          }
        }
        const output = `Error in tool ${call.function.name}: ${
          err instanceof Error ? err.message : String(err)
        }`;
        trace.push(`### Tool ${call.function.name}\n\n${output}\n`);
        messages.push({
          role: "tool",
          tool_name: call.function.name,
          content: output,
        });
      }
    }
  }

  // Guarantee a final user-facing answer even if we hit max rounds with only tool calls.
  if (!finalText.trim()) {
    logStep(options, "forcing final synthesis without tools");
    messages.push({
      role: "user",
      content:
        "Provide the final answer now in English. Do not call any tools. Summarize findings concisely.",
    });
    const finalResult = await callOllamaWithRetry(options, messages, undefined, 3);
    totalPrompt += finalResult.prompt_eval_count ?? 0;
    totalCompletion += finalResult.eval_count ?? 0;
    finalText = finalResult.message?.content ?? "";
    if (finalText.trim()) {
      logStep(options, `final synthesis received (${finalText.length} chars)`);
      trace.push(`## Assistant final synthesis\n\n${finalText}\n`);
    }
  }

  console.log(finalText || "(no assistant text)");
  console.log(
    `\n---\nPrompt tokens(total): ${totalPrompt}\nCompletion tokens(total): ${totalCompletion}\nTool calls(total): ${totalToolCalls}`,
  );

  const saved = await saveRun(
    options.outDir,
    runName,
    [
      `# Run ${runName}`,
      "",
      "## Prompt",
      "",
      userPrompt,
      "",
      "## Final Response",
      "",
      finalText,
      "",
      "## Metrics",
      "",
      `- prompt_eval_count_total: ${totalPrompt}`,
      `- eval_count_total: ${totalCompletion}`,
      `- tool_calls_total: ${totalToolCalls}`,
      `- autonomous: ${options.autonomous}`,
      `- allow_bash: ${options.allowBash}`,
      `- allow_web: ${options.allowWeb}`,
      `- allow_fs: ${options.allowFs}`,
      `- write_ops: ${writeOps}`,
      `- validation_attempts: ${validationHits}`,
      `- validation_success_hits: ${validationSuccessHits}`,
      `- validation_failure_hits: ${validationFailureHits}`,
      "",
      "## Trace",
      "",
      ...trace,
    ].join("\n"),
  );
  console.log(`Saved: ${saved}`);
  logStep(options, `run saved -> ${saved}`);
  const pruned = await pruneOldRuns(options.outDir, options.keepRuns);
  if (pruned > 0) {
    console.log(`Pruned old runs: ${pruned} (keep-runs=${Math.max(1, options.keepRuns)})`);
    logStep(options, `old runs pruned -> ${pruned}`);
  }
  logStep(options, `run end -> ${runName}`);
  const analysisWarnings = analyzeResponseIssues(finalText);
  if (analysisWarnings.length > 0) {
    logStep(options, `response analysis warnings -> ${analysisWarnings.join(" | ")}`);
  }
  let ok = true;
  if (options.readyToken) {
    const ready = finalText.includes(options.readyToken);
    if (!ready) {
      logStep(options, `ready token missing -> ${options.readyToken}`);
      console.error(
        `Ready token not found in final response. Token required: ${options.readyToken}`,
      );
      ok = false;
    } else {
      logStep(options, `ready token detected -> ${options.readyToken}`);
    }
  }
  if (options.requireWrite && writeOps === 0) {
    console.error("Run rejected: no file write/edit operation was performed.");
    logStep(options, "require-write failed");
    ok = false;
  }
  if (validationRegex && validationHits === 0) {
    console.error(
      `Run rejected: no validation command/output matched regex '${options.requireValidation}'.`,
    );
    logStep(options, `require-validation failed -> ${options.requireValidation}`);
    ok = false;
  }
  if (
    validationRegex &&
    options.requireValidationSuccess &&
    validationSuccessHits === 0
  ) {
    console.error(
      `Run rejected: validation did not succeed for regex '${options.requireValidation}'.`,
    );
    logStep(
      options,
      `require-validation-success failed -> ${options.requireValidation}`,
    );
    ok = false;
  }
  return { ok, finalText, analysisWarnings, messages };
}

async function main() {
  const { command, positional, options } = parseArgs(process.argv.slice(2));

  if (command === "help" || !["list", "show", "run", "ask", "gen-prompts", "help"].includes(command)) {
    printHelp();
    return;
  }

  if (command === "gen-prompts") {
    await generateValidationPrompts(options);
    return;
  }

  if (command === "ask") {
    if (options.batchFile) {
      const prompts = await loadBatchPrompts(options.batchFile);
      console.log(`Batch mode: loaded ${prompts.length} prompt(s) from ${options.batchFile}`);
      let failures = 0;
      let warnings = 0;
      let sessionMessages: ChatMessage[] = [];
      for (let i = 0; i < prompts.length; i += 1) {
        const prompt = prompts[i];
        const runName = `ask-batch-${String(i + 1).padStart(2, "0")}`;
        console.log(`\n===== Batch item ${i + 1}/${prompts.length} =====`);
        try {
          const outcome = await runPrompt(prompt, runName, options, sessionMessages);
          sessionMessages = outcome.messages;
          if (outcome.analysisWarnings.length > 0) {
            warnings += 1;
            console.error(
              `Batch item ${i + 1} warnings: ${outcome.analysisWarnings.join(" | ")}`,
            );
          }
          if (!outcome.ok) {
            failures += 1;
            console.error(`Batch item ${i + 1} failed readiness checks.`);
            if (!options.continueOnError) {
              process.exit(42);
            }
          }
        } catch (err) {
          failures += 1;
          console.error(
            `Batch item ${i + 1} error: ${err instanceof Error ? err.message : String(err)}`,
          );
          if (!options.continueOnError) {
            process.exit(1);
          }
        }
      }
      console.log(
        `\nBatch completed: total=${prompts.length}, failures=${failures}, warnings=${warnings}, continueOnError=${options.continueOnError}`,
      );
      if (failures > 0) {
        process.exit(42);
      }
      return;
    }

    const prompt = positional.join(" ").trim();
    if (!prompt) {
      throw new Error("ask requires a prompt string.");
    }
    const outcome = await runPrompt(prompt, "ask", options);
    if (!outcome.ok) process.exit(42);
    return;
  }

  const tasks = await loadTasks(options.tasksFile);

  if (command === "list") {
    for (const t of tasks) {
      console.log(`${t.id} - ${t.title}`);
    }
    return;
  }

  const taskId = positional[0];
  if (!taskId) {
    throw new Error(`${command} requires <TASK_ID>.`);
  }
  const task = requireTask(tasks, taskId);

  if (command === "show") {
    console.log(`${task.id} - ${task.title}\n`);
    console.log(task.prompt);
    return;
  }

  if (command === "run") {
    const outcome = await runPrompt(task.prompt, task.id, options);
    if (!outcome.ok) process.exit(42);
    return;
  }
}

main().catch((err) => {
  console.error(`Error: ${err instanceof Error ? err.message : String(err)}`);
  process.exit(1);
});
