---
layout: default
title: Orchestra Engine
parent: Architecture
nav_order: 3
---

# Orchestra Engine
{: .no_toc }

Orchestra is Allux's multi-step structured execution engine. This document
describes the internal architecture for contributors and advanced users.

<details open markdown="block">
<summary>Table of contents</summary>
{: .text-delta }
1. TOC
{:toc}
</details>

---

## Design goals

| Goal | Mechanism |
|------|-----------|
| Prevent context rot | Each task runs in an isolated micro-session with its own history |
| Resumability | Full state machine persisted after every phase transition |
| Deterministic validation | No LLM in the common validation path; only checks pass/fail |
| Bounded execution | `max_rounds` + `MAX_TOOL_CALLS` cap per worker session |
| Composable failure handling | Diagnoser decides retry strategy; driver enforces it |

---

## Component map

```
TUI (app.rs)
  │  DriverEvent stream (mpsc::unbounded)
  ▼
Driver (driver.rs)           ← state machine
  ├── Planner (planner.rs)   ← LLM: goal → TaskSpec list (L1 + L2)
  ├── Worker (worker.rs)     ← LLM: executes one leaf task
  ├── Validator (validator/) ← deterministic: checks pass/fail/soft
  ├── Diagnoser (diagnoser.rs) ← LLM: failure → RetryStrategy
  └── Store (store.rs)       ← disk persistence, artifact index
```

---

## State machine phases

```
Planning
   │
   ▼
ExpandingL2 { l1 }
   │
   ▼
ExecutingTask { l1, l2 }
   │
   ▼
Validating { l1, l2 }
   │
   ├─ pass ──────────────────────────────► advance_to_next_l1
   │
   └─ fail ──► Diagnosing { l1, l2, attempt }
                   │
                   ├─ RetryAsIs     ──► ExecutingTask (same spec)
                   ├─ RetryWithHint ──► ExecutingTask (with hint)
                   ├─ ReplanSubtree ──► ExpandingL2 (re-expand L1)
                   ├─ Skip          ──► advance_to_next_l1
                   └─ EscalateToUser
                       ├─ Interactive → emit UserEscalationNeeded, pause
                       └─ Autonomous  → defer, advance_to_next_l1

                                         advance_to_next_l1
                                           │
                                           ├─ next pending L2 → ExecutingTask
                                           ├─ L1 complete → next pending L1 → ExpandingL2
                                           └─ all done → Finalizing → Done
```

---

## Wire format: ALF

All LLM ↔ driver boundaries use **Allux Line Format (ALF)**, a token-efficient
line-based format:

```
key value
array_key item1, item2, item3
multiline_key:
  line one
  line two
:end
.
```

- Each record ends with `.` on its own line.
- Arrays are comma-separated on a single line.
- `-` means empty / none.
- No JSON, no YAML, no code fences.

ALF keeps prompts short, reduces tokenization noise, and is trivially parseable
without a full JSON parser.

---

## Planner

The planner runs twice per run:

**L1 planning** (`plan_l1`): converts the user's goal into 3–8 milestone-level
`TaskSpec` records. Each L1 task has an `id` like `T01`, `T02`, …

**L2 expansion** (`plan_l2`): converts one L1 milestone into 2–6 concrete leaf
`TaskSpec` records. Each L2 task has an `id` like `T01.01`, `T01.02`, …

Both functions retry once on malformed ALF output before returning an error.

---

## Worker

Each L2 task runs in an isolated micro-session:

1. Build system prompt with the original goal (capped at 500 chars) and the
   current artifact index.
2. Build user prompt with the task spec in ALF and an optional hint.
3. Agentic loop: up to `spec.max_rounds` LLM round-trips, max 12 tool calls.
4. On `LlmResponse::Text`: parse the `FinalReport` ALF record.
5. On `LlmResponse::ToolCalls`: dispatch tools, append results to history.

The worker returns a `TaskReport` with status `ok | failed | needs_review`.

---

## Validator

Validation is fully deterministic (no LLM):

| Check type | Description |
|-----------|-------------|
| `FileExists(path)` | File must exist on disk |
| `FileSizeMin(path, bytes)` | File must be at least N bytes |
| `FileSizeMax(path, bytes)` | File must be at most N bytes |
| `FileContains(path, pattern)` | File must contain the pattern |
| `FileMissing(path)` | File must NOT exist |
| `ContentLanguage(path, lang, ratio)` | Language detection (soft signal) |
| `NoDuplicateSymbols(path, kind)` | No duplicate function/class names |
| `CrossFileConsistency(paths)` | Rust mod declarations match files |
| `CommandExitsZero(cmd, cwd)` | Shell command must exit 0 |
| `ManualReview(note)` | Always soft; flags for human review |

Auto-detection adds `CommandExitsZero` checks based on project files found in
the workspace (Cargo.toml, package.json, pyproject.toml).

Each check returns `Pass | Fail { reason } | Soft(score)`. The overall
`ValidationReport` score is `(passes + 0.5 * softs) / total`.

---

## Diagnoser

The diagnoser runs only when validation fails. It first tries deterministic
short-circuits (no LLM):

- Worker said `NeedsReview` → `EscalateToUser`
- Single `FileExists` failure → `RetryWithHint` (create that file)
- Timeout (`exit 124`) → `RetryAsIs`
- No hard failures, score ≥ 0.6 → `EscalateToUser`

If no short-circuit matches, the LLM is called with the task spec, worker
report, and top-5 validation failures. The LLM returns a `Diagnosis` record
with `root_cause`, `strategy`, and optional `hint`. Two parse failures fall
back to `EscalateToUser`.

---

## Store & persistence

```
<workspace>/.allux/runs/<run-id>/
├── state.json          # OrchestratorState (phase, counters, run_id)
├── plan.json           # L1 TaskSpec list
├── tasks/
│   ├── T01.json        # L1 spec
│   ├── T01.01.json     # L2 spec
│   ├── T01.01.report.1.json
│   ├── T01.01.validation.1.json
│   ├── T01.01.diagnosis.1.json
│   └── T01.01.latest.json  # { attempt, verdict }
├── artifacts/
│   └── index.json      # ArtifactIndex { path → { description, size_bytes } }
└── events.log          # newline-delimited JSON (compressed to .zst on finalize)
```

Writes are atomic: data is written to a `.tmp` file then renamed. An advisory
lock at `.allux/runs/.lock` prevents two processes from driving the same
workspace simultaneously.

---

## Artifact index

Each time a worker touches a file, the driver records it in the `ArtifactIndex`.
Later workers receive the index in their system prompt:

```
artifacts:
src/main.rs   entry point, 1240B
src/auth.rs   authentication module, 2800B
:end
```

This allows later tasks to discover what earlier tasks created without reading
every file.

---

## Event stream

The driver emits `DriverEvent` values over a `mpsc::UnboundedSender` to the TUI:

| Event | When |
|-------|------|
| `RunStarted(run_id)` | Start of `drive_loop` |
| `PhaseChanged(phase)` | Every state transition |
| `TaskStarted(id)` | Worker micro-session begins |
| `TaskProgress { id, note }` | Milestone logs within a phase |
| `TaskFinished { id, verdict }` | After validation |
| `UserEscalationNeeded { id, reason, report }` | Interactive mode failure |
| `RunFinished(FinalReport)` | Run complete |

The TUI drains this channel on every tick and renders events as chat messages.
</content>
</invoke>