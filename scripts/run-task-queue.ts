#!/usr/bin/env -S npx tsx
// @ts-nocheck
/**
 * Ejecuta T01–T10 desde TASKS_OLLERO_TOOL_ACTIONS.md en orden, una corrida por tarea.
 * Para tras demasiados fallos consecutivos (exit≠0).
 *
 * Uso:
 *   npx tsx scripts/run-task-queue.ts
 *   npx tsx scripts/run-task-queue.ts --dry-list
 *   npx tsx scripts/run-task-queue.ts --from T04 --to T07
 *   npx tsx scripts/run-task-queue.ts --strict
 *
 * Por defecto NO usa --require-write (el modelo suele quedarse en bucles de read_file
 * y el modo forzado bloquea antes de editar). Con --strict se exige al menos un write
 * en tareas marcadas como "idealmente con commit de docs".
 */

import { spawn } from "node:child_process";
import path from "path";

type TaskSpec = {
  id: string;
  /** Si true y --strict: pasa --require-write a ollero-cli */
  preferWrite: boolean;
  /** Regex para --require-validation (bash); vacío = no exigir */
  requireValidation: string;
  /** Regex paths de escritura; undefined = default seguro src+scripts TS + TASKS */
  writeAllowRegex?: string;
  /** Anexado como --system */
  systemExtra?: string;
};

const DEFAULT_WRITE =
  "^src/|^scripts/(ollero-cli|master-automejora)\\.ts$|^TASKS_OLLERO_TOOL_ACTIONS\\.md$";

const MODEL = process.env.OLLERO_QUEUE_MODEL ?? "qwen3-coder:30b";
const MAX_ROUNDS = process.env.OLLERO_QUEUE_MAX_ROUNDS ?? "16";
const MAX_ERRORS = Number(process.env.OLLERO_QUEUE_MAX_ERRORS ?? "3");

const VERIFY_ONLY =
  "First: read relevant files. If the requested capability already exists and works, do NOT duplicate it; only run cargo test (or minimal checks) and report PASS with file:line evidence. If something is clearly missing, make the smallest safe change within allowed paths.";

const QUEUE: TaskSpec[] = [
  {
    id: "T01",
    preferWrite: false,
    requireValidation: "cargo test|ollama|api/tags",
    systemExtra: `${VERIFY_ONLY} Task T01: baseline only — do not edit any repository files.`,
  },
  {
    id: "T02",
    preferWrite: true,
    requireValidation: "cargo test",
    writeAllowRegex: DEFAULT_WRITE,
    systemExtra: `${VERIFY_ONLY} Task T02: improve or verify CLI documentation in TASKS_OLLERO_TOOL_ACTIONS.md / ollero-cli help; use replace_in_file after at most 2 reads of the same file.`,
  },
  {
    id: "T03",
    preferWrite: false,
    requireValidation: "cargo test",
    systemExtra: `${VERIFY_ONLY} Task T03: verify list/show/run/ask exist in scripts/ollero-cli.ts; do not rewrite the whole file.`,
  },
  {
    id: "T04",
    preferWrite: false,
    requireValidation: "cargo test",
    systemExtra: `${VERIFY_ONLY} Task T04: verify run logs markdown under .ollero-cli/runs with metrics; fix only if broken.`,
  },
  {
    id: "T05",
    preferWrite: false,
    requireValidation: "cargo test",
    systemExtra: `${VERIFY_ONLY} Task T05: verify ask command exists; fix only if broken.`,
  },
  {
    id: "T06",
    preferWrite: true,
    requireValidation: "cargo test",
    writeAllowRegex: DEFAULT_WRITE,
    systemExtra: `${VERIFY_ONLY} Task T06: ensure smoke-test checklist exists in TASKS (add bullets if missing).`,
  },
  {
    id: "T07",
    preferWrite: true,
    requireValidation: "cargo test",
    writeAllowRegex: DEFAULT_WRITE,
    systemExtra: `${VERIFY_ONLY} Task T07: document Rust REPL vs TS CLI workflow in TASKS (short subsection).`,
  },
  {
    id: "T08",
    preferWrite: true,
    requireValidation: "cargo test",
    writeAllowRegex: DEFAULT_WRITE,
    systemExtra: `${VERIFY_ONLY} Task T08: tighten scope rules text in TASKS only; no Rust logic change unless truly needed.`,
  },
  {
    id: "T09",
    preferWrite: false,
    requireValidation: "cargo test|git status",
    systemExtra: `${VERIFY_ONLY} Task T09: summarize change set and propose commit message in final answer only — do NOT run git commit or push.`,
  },
  {
    id: "T10",
    preferWrite: true,
    requireValidation: "cargo test",
    writeAllowRegex: DEFAULT_WRITE,
    systemExtra: `${VERIFY_ONLY} Task T10: add or refresh next-5-improvements plan in TASKS section T10.`,
  },
];

function parseArgs(argv: string[]): {
  dryList: boolean;
  fromId: string | null;
  toId: string | null;
  strict: boolean;
} {
  let dryList = false;
  let fromId: string | null = null;
  let toId: string | null = null;
  let strict = false;
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === "--dry-list") dryList = true;
    if (argv[i] === "--strict") strict = true;
    if (argv[i] === "--from" && argv[i + 1]) {
      fromId = argv[i + 1].toUpperCase();
      i += 1;
    }
    if (argv[i] === "--to" && argv[i + 1]) {
      toId = argv[i + 1].toUpperCase();
      i += 1;
    }
  }
  if (process.env.OLLERO_QUEUE_STRICT === "1") strict = true;
  return { dryList, fromId, toId, strict };
}

const CARGO_GATE =
  "MANDATORY before final answer: call the bash tool once from the repo root to run `cargo test -q` (or `cargo test`) and keep its output; do not finish with only prose.";

function buildArgs(spec: TaskSpec, strict: boolean): string[] {
  const args = [
    "--experimental-strip-types",
    path.join("scripts", "ollero-cli.ts"),
    "run",
    spec.id,
    "--autonomous",
    "--verbose",
    "--model",
    MODEL,
    "--max-rounds",
    MAX_ROUNDS,
    "--prewrite-max-inspect-rounds",
    "14",
    "--prevalidation-max-postwrite-rounds",
    "4",
  ];
  const system = [`TASK_ID=${spec.id} (follow this task's PROMPT block only).`, CARGO_GATE, spec.systemExtra ?? ""]
    .filter(Boolean)
    .join(" ");
  args.push("--system", system);
  if (strict && spec.preferWrite) {
    args.push("--require-write");
  }
  if (spec.requireValidation.trim()) {
    args.push("--require-validation", spec.requireValidation);
  }
  args.push("--write-allow-regex", spec.writeAllowRegex ?? DEFAULT_WRITE);
  return args;
}

function runNode(args: string[]): Promise<number> {
  return new Promise((resolve) => {
    const child = spawn("node", args, {
      cwd: process.cwd(),
      stdio: "inherit",
      shell: false,
      env: process.env,
    });
    child.on("exit", (code) => resolve(code ?? 1));
  });
}

async function main() {
  const argv = process.argv.slice(2);
  const { dryList, fromId, toId, strict } = parseArgs(argv);

  let list = [...QUEUE];
  if (fromId) {
    const idx = list.findIndex((t) => t.id === fromId);
    if (idx < 0) {
      console.error(`Unknown --from ${fromId}`);
      process.exit(1);
    }
    list = list.slice(idx);
  }
  if (toId) {
    const idx = list.findIndex((t) => t.id === toId);
    if (idx < 0) {
      console.error(`Unknown --to ${toId}`);
      process.exit(1);
    }
    list = list.slice(0, idx + 1);
  }

  console.log("=== Cola de tareas (orden) ===\n");
  for (const t of list) {
    console.log(
      `${t.id}  preferWrite=${t.preferWrite}  validation=${t.requireValidation || "(none)"}`,
    );
  }
  console.log(
    `\nmodel=${MODEL} maxRounds=${MAX_ROUNDS} strict=${strict} (require-write only if strict && preferWrite) stop after ${MAX_ERRORS} consecutive failures\n`,
  );

  if (dryList) {
    return;
  }

  let consecutive = 0;
  let anyFailed = false;
  for (let i = 0; i < list.length; i += 1) {
    const spec = list[i];
    console.log(`\n${"=".repeat(60)}\n>>> ${i + 1}/${list.length}  ${spec.id}\n${"=".repeat(60)}\n`);
    const code = await runNode(buildArgs(spec, strict));
    if (code !== 0) {
      anyFailed = true;
      consecutive += 1;
      console.error(`\n[queue] ${spec.id} failed with exit ${code} (consecutive ${consecutive}/${MAX_ERRORS})\n`);
      if (consecutive >= MAX_ERRORS) {
        console.error("[queue] Too many consecutive failures — stopping.");
        process.exit(42);
      }
    } else {
      consecutive = 0;
      console.log(`\n[queue] ${spec.id} OK\n`);
    }
  }
  console.log("\n[queue] Queue run finished.\n");
  if (anyFailed) {
    process.exit(42);
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
