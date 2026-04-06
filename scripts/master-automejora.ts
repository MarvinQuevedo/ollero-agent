#!/usr/bin/env -S npx tsx
// @ts-nocheck

import { readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import path from "node:path";
import { spawn } from "node:child_process";

type Options = {
  model: string;
  intervalMs: number;
  cycles: number;
  maxRounds: number;
  keepRuns: number;
  mode: "ask" | "run";
  taskId: string;
  prompt: string;
  readyToken: string;
  exitOnReady: boolean;
  requireWrite: boolean;
  requireValidation: string;
  requireValidationSuccess: boolean;
  writeAllowRegex: string;
  prewriteMaxInspectRounds: number;
  prevalidationMaxPostWriteRounds: number;
};

const DEFAULTS: Options = {
  model: "qwen3.5:9b",
  intervalMs: 1500,
  cycles: 0, // 0 = infinite
  maxRounds: 10,
  keepRuns: 20,
  mode: "ask",
  taskId: "T01",
  prompt:
    "You are working in the current local repo. Apply one small safe Rust improvement, validate with cargo check, and report the change.",
  readyToken: "CLI_READY_TO_RESTART",
  exitOnReady: true,
  requireWrite: true,
  requireValidation: "cargo check|cargo test",
  requireValidationSuccess: true,
  // Allow Rust sources and the two TS drivers (self-improve loop edits ollero-cli; master may edit itself).
  writeAllowRegex: "^src/|^scripts/(ollero-cli|master-automejora)\\.ts$",
  prewriteMaxInspectRounds: 2,
  prevalidationMaxPostWriteRounds: 1,
};

function now(): string {
  return new Date().toISOString();
}

function log(message: string): void {
  console.log(`[${now()}] [master] ${message}`);
}

function parseArgs(argv: string[]): Options {
  const options: Options = { ...DEFAULTS };
  const flags = new Map<string, string | true>();
  const positional: string[] = [];

  for (let i = 0; i < argv.length; i += 1) {
    const token = argv[i];
    if (token.startsWith("--")) {
      const key = token.slice(2);
      const next = argv[i + 1];
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

  if (typeof flags.get("model") === "string") options.model = String(flags.get("model"));
  if (typeof flags.get("interval-ms") === "string") options.intervalMs = Number(flags.get("interval-ms"));
  if (typeof flags.get("cycles") === "string") options.cycles = Number(flags.get("cycles"));
  if (typeof flags.get("max-rounds") === "string") options.maxRounds = Number(flags.get("max-rounds"));
  if (typeof flags.get("keep-runs") === "string") options.keepRuns = Number(flags.get("keep-runs"));
  if (typeof flags.get("mode") === "string") options.mode = String(flags.get("mode")) === "run" ? "run" : "ask";
  if (typeof flags.get("task") === "string") options.taskId = String(flags.get("task"));
  if (typeof flags.get("prompt") === "string") options.prompt = String(flags.get("prompt"));
  if (typeof flags.get("ready-token") === "string") options.readyToken = String(flags.get("ready-token"));
  if (flags.has("no-exit-on-ready")) options.exitOnReady = false;
  if (flags.has("no-require-write")) options.requireWrite = false;
  if (typeof flags.get("require-validation") === "string") {
    options.requireValidation = String(flags.get("require-validation"));
  }
  if (flags.has("allow-failed-validation")) {
    options.requireValidationSuccess = false;
  }
  if (typeof flags.get("write-allow-regex") === "string") {
    options.writeAllowRegex = String(flags.get("write-allow-regex"));
  }
  if (typeof flags.get("prewrite-max-inspect-rounds") === "string") {
    options.prewriteMaxInspectRounds = Number(flags.get("prewrite-max-inspect-rounds"));
  }
  if (typeof flags.get("prevalidation-max-postwrite-rounds") === "string") {
    options.prevalidationMaxPostWriteRounds = Number(
      flags.get("prevalidation-max-postwrite-rounds"),
    );
  }

  if (positional.length > 0) {
    options.prompt = positional.join(" ");
  }

  return options;
}

async function sha256File(filePath: string): Promise<string> {
  const data = await readFile(filePath);
  return createHash("sha256").update(data).digest("hex");
}

function buildCliArgs(options: Options): string[] {
  const strictContract = [
    "Mandatory execution contract:",
    "1) Perform at least one real write_file or replace_in_file operation.",
    "2) For existing large files (e.g. scripts/ollero-cli.ts), use replace_in_file with a minimal unique old_string — do not paste the whole file into write_file.",
    "3) The edited path must satisfy the allowed write regex.",
    "4) Run validation command matching required regex (cargo check or cargo test).",
    "5) If not completed, continue tool-calling and do not finalize with summaries.",
  ].join(" ");
  const askPromptWithContract =
    options.mode === "ask"
      ? `${options.prompt}\n\n${strictContract}`
      : options.prompt;
  const common = [
    "--experimental-strip-types",
    "scripts/ollero-cli.ts",
    options.mode,
    options.mode === "run" ? options.taskId : askPromptWithContract,
    "--autonomous",
    "--verbose",
    "--model",
    options.model,
    "--max-rounds",
    String(options.maxRounds),
    "--keep-runs",
    String(options.keepRuns),
    "--ready-token",
    options.readyToken,
  ];
  if (options.requireWrite) {
    common.push("--require-write");
  }
  if (options.requireValidation.trim()) {
    common.push("--require-validation", options.requireValidation);
  }
  if (!options.requireValidationSuccess) {
    common.push("--allow-failed-validation");
  }
  if (options.writeAllowRegex.trim()) {
    common.push("--write-allow-regex", options.writeAllowRegex);
  }
  common.push(
    "--prewrite-max-inspect-rounds",
    String(Math.max(0, options.prewriteMaxInspectRounds)),
  );
  common.push(
    "--prevalidation-max-postwrite-rounds",
    String(Math.max(0, options.prevalidationMaxPostWriteRounds)),
  );
  return common;
}

function runOneCycle(options: Options): Promise<number> {
  const args = buildCliArgs(options);
  log(`starting cycle process: node ${args.join(" ")}`);
  return new Promise((resolve) => {
    const child = spawn("node", args, {
      cwd: process.cwd(),
      stdio: "inherit",
      shell: false,
    });
    child.on("exit", (code) => {
      resolve(code ?? 0);
    });
  });
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  const cliPath = path.resolve(process.cwd(), "scripts", "ollero-cli.ts");

  let cycle = 0;
  let previousHash = await sha256File(cliPath);
  log(`watching CLI file: ${cliPath}`);
  log(`initial hash: ${previousHash.slice(0, 12)}...`);
  log(
    `config -> mode=${options.mode} model=${options.model} maxRounds=${options.maxRounds} intervalMs=${options.intervalMs} cycles=${options.cycles || "infinite"} readyToken=${options.readyToken} exitOnReady=${options.exitOnReady} requireWrite=${options.requireWrite} requireValidation=${options.requireValidation} requireValidationSuccess=${options.requireValidationSuccess} writeAllowRegex=${options.writeAllowRegex} prewriteMaxInspectRounds=${options.prewriteMaxInspectRounds} prevalidationMaxPostWriteRounds=${options.prevalidationMaxPostWriteRounds}`,
  );

  while (options.cycles <= 0 || cycle < options.cycles) {
    cycle += 1;
    log(`cycle ${cycle} start`);

    const exitCode = await runOneCycle(options);
    log(`cycle ${cycle} end with exit=${exitCode}`);

    const currentHash = await sha256File(cliPath);
    if (currentHash !== previousHash) {
      log(
        `detected ollero-cli.ts change (${previousHash.slice(0, 12)} -> ${currentHash.slice(0, 12)}), reloading next cycle`,
      );
      previousHash = currentHash;
    }

    if (exitCode !== 0) {
      if (exitCode === 42) {
        log("child reported unmet readiness gates (token/write/validation), restarting automatically");
      } else {
        log("cycle ended with non-zero code, restarting automatically after delay");
      }
    } else if (options.exitOnReady) {
      log("child exited successfully with ready token; stopping master loop");
      break;
    }

    await sleep(Math.max(0, options.intervalMs));
  }

  log("master finished requested cycles");
}

main().catch((err) => {
  log(`fatal error: ${err instanceof Error ? err.message : String(err)}`);
  process.exit(1);
});
