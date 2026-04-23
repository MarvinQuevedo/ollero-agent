---
layout: default
title: Orchestra Mode
parent: Guides
nav_order: 6
---

# Orchestra Mode
{: .no_toc }

Orchestra mode is Allux's multi-step structured execution engine. Instead of a
single long LLM conversation, Orchestra decomposes a goal into a tree of
short, isolated micro-sessions that run sequentially, each bounded by a round
and tool-call cap. This eliminates "context rot" and makes large, multi-file
tasks reliable and resumable.

<details open markdown="block">
<summary>Table of contents</summary>
{: .text-delta }
1. TOC
{:toc}
</details>

---

## Quick start

```
/mode orchestra
Build a REST API server with authentication and tests
```

Or in a single command:

```
/orchestra Build a REST API server with authentication and tests
```

Allux will:
1. **Plan** a set of L1 milestones (e.g. "Setup project", "Auth module", "Tests").
2. **Expand** each milestone into concrete L2 leaf tasks.
3. **Execute** each leaf task in an isolated micro-session.
4. **Validate** the output deterministically (file existence, syntax, lint, tests).
5. **Diagnose** failures and retry, replan, or escalate as needed.
6. **Finalize** with a summary report saved to disk.

---

## Slash commands

| Command | Description |
|---------|-------------|
| `/orchestra <goal>` | Start a new run with the given goal |
| `/orchestra list` | List past Orchestra runs |
| `/orchestra resume <id>` | Resume a paused or interrupted run |
| `/orchestra cancel` | Abort the currently active run |
| `/retry [hint]` | Retry an escalated task, with an optional hint |
| `/skip` | Skip an escalated task and continue |
| `/abort` | Abort an escalated task (or the whole run) |
| `/policy interactive` | Escalate task failures to you (default) |
| `/policy autonomous` | Defer failures and continue automatically |

---

## Failure policies

### Interactive (default)
When a task fails after all retry attempts, Allux pauses and shows:

```
⚠ Escalation needed for T02.03: Worker did not create src/auth/token.rs
Reply with: /retry [hint] | /skip | /abort
```

You can then:
- `/retry` — retry the task as-is
- `/retry create src/auth/token.rs first` — retry with a specific hint
- `/skip` — mark the task skipped and continue with remaining tasks
- `/abort` — stop the run

### Autonomous
Allux defers failed tasks and completes as much as possible. Deferred tasks
appear in the final report. Use this for unattended runs.

```
/policy autonomous
/orchestra Migrate database schema and update all callers
```

---

## Resuming runs

Orchestra runs are persisted to `<workspace>/.allux/runs/<run-id>/`. If a run
is interrupted (network drop, crash, manual cancel), you can resume:

```
/orchestra list
/orchestra resume abc123def456
```

The run picks up from the last completed task. Completed tasks are not re-executed.

---

## Understanding progress output

```
[run] abc123def456
[phase] Planning
  planner: Planning L1 tasks…
[phase] ExpandingL2
▶ Task started: T01
  T01: Expanding into subtasks…
[phase] ExecutingTask
▶ Task started: T01.01
✓ T01.01: ok
✗ T01.02: failed
⚠ Escalation needed for T01.02: …
```

- `▶` — task started
- `✓` — task passed validation
- `✗` — task failed (will be retried or escalated)
- `⚠` — escalation required (interactive mode only)

---

## Validation

Each task is validated deterministically before being marked complete. Checks
include:

- **FileExists** — required output files were created
- **FileSizeMin / FileSizeMax** — output is within expected bounds
- **FileContains** — file contains required patterns
- **ContentLanguage** — correct language ratio for the file type
- **CommandExitsZero** — build/test/lint command passes (auto-detected)
- **ManualReview** — soft signal; always passes but flags for human review

Auto-detected commands (no config needed):
- `Cargo.toml` present → `cargo check`
- `package.json` present → `npm run build` and `npm test`
- `pyproject.toml` present → `python -m py_compile`

---

## Run artifacts

Each run stores its state under `<workspace>/.allux/runs/<run-id>/`:

```
.allux/runs/abc123def456/
├── state.json          # current phase + counters
├── plan.json           # L1 task list
├── tasks/
│   ├── T01.json        # L1 task spec
│   ├── T01.01.json     # L2 task spec
│   └── …
├── artifacts/
│   └── index.json      # file registry for context sharing
└── events.log          # event log (compressed to .zst when done)
```

`.allux/` is automatically added to `.gitignore` on first run.

---

## Tips

- **Context sharing**: Files created by early tasks are automatically passed as
  artifact context to later tasks.
- **Max rounds / tool calls**: Each micro-session is capped at the task's
  `max_rounds` (default 6) and 12 total tool calls to prevent runaway loops.
- **Model**: Orchestra uses the same model as your current session. Larger
  models (e.g. 70B) produce better plans and more reliable workers.
- **Long goals**: Be specific. "Build a REST API with JWT auth, Postgres,
  and integration tests using Axum" works better than "Build an API".
</content>
</invoke>