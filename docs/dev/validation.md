---
layout: default
title: Validation Suite
parent: Development
nav_order: 3
---

# Allux Validation Suite (Autonomous CLI)
{: .no_toc }

This suite validates the full autonomous workflow against a real sample project:
- Analyze existing code
- Apply file changes
- Run checks/tests
- Request follow-up updates
- Process multiple prompts sequentially

<details open markdown="block">
<summary>Table of contents</summary>
{: .text-delta }
1. TOC
{:toc}
</details>

---

## 1. Sample Project

Use: `sandbox/sample-rust-app`

The sample intentionally includes:
- A small Rust binary
- An inefficient/verbose implementation in `src/lib.rs`
- Tests in `src/lib.rs`

---

## 2. Single Prompt Validation

Run one autonomous prompt:

```bash
node --experimental-strip-types scripts/allux-cli.ts ask "Work only inside sandbox/sample-rust-app. Improve the implementation in src/lib.rs for clarity, keep behavior, then run cargo test there. Respond in English." --autonomous --max-rounds 8 --verbose
```

**Expected:**
- Tool calls include `read_file`, `replace_in_file` or `write_file`, and `bash`
- Output confirms what changed
- `cargo test` succeeds

---

## 3. Multi-Input Sequential Validation

Use batch file: `validation/prompts-sequential.txt`

```bash
node --experimental-strip-types scripts/allux-cli.ts ask --batch-file validation/prompts-sequential.txt --autonomous --max-rounds 8 --verbose
```

**Behavior:**
- Prompts are executed one by one
- Each prompt waits for completion before next starts
- Errors are reported and execution continues by default (use `--stop-on-error` to halt)

---

## 4. Master Supervisor Validation

Run managed cycles with ready token:

```bash
node --experimental-strip-types scripts/master-automejora.ts --cycles 2 --interval-ms 1000 --prompt "Work inside sandbox/sample-rust-app only. Apply one safe improvement and run cargo test. Include CLI_READY_TO_RESTART only when done." --max-rounds 8
```

**Expected:**
- Supervisor waits for child completion
- If child returns non-zero, it restarts
- If `scripts/allux-cli.ts` changed, next cycle uses the new version
- Exits early when ready token is detected

---

## 5. Regression Checks

After autonomous runs:

```bash
cargo check
git status --short
```

**Review:**
- Ensure intended files changed
- No accidental large edits
- No broken build
