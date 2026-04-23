use anyhow::{anyhow, Context, Result};

use crate::ollama::client::OllamaClient;
use crate::ollama::types::{ChatOptions, LlmResponse, Message};
use crate::orchestra::alf::{self, AlfError, FromAlf, ToAlf};
use crate::orchestra::types::{ArtifactIndex, TaskId, TaskSpec};

// ── Public API ────────────────────────────────────────────────────────────────

/// Break a user goal into high-level L1 tasks (no parent).
pub async fn plan_l1(
    client: &OllamaClient,
    goal: &str,
    ctx_size: u32,
) -> Result<Vec<TaskSpec>> {
    let goal_capped = cap_chars(goal, 2_000);
    let prompt = build_l1_prompt(&goal_capped);
    call_planner_with_retry(client, &prompt, None, ctx_size).await
}

/// Break one L1 task into concrete L2 subtasks.
pub async fn plan_l2(
    client: &OllamaClient,
    goal: &str,
    l1: &TaskSpec,
    artifacts: &ArtifactIndex,
    ctx_size: u32,
) -> Result<Vec<TaskSpec>> {
    let goal_capped = cap_chars(goal, 2_000);
    let prompt = build_l2_prompt(&goal_capped, l1, artifacts);
    call_planner_with_retry(client, &prompt, Some(&l1.id), ctx_size).await
}

// ── Prompt builders ───────────────────────────────────────────────────────────

fn build_l1_prompt(goal: &str) -> String {
    format!(
        r#"You are a task planner for a software engineering assistant.
Your only job is to break down the user's goal into a short, ordered list
of high-level tasks, each completable in under 10 minutes of work.

HARD RULES:
- Reply in ALF. Emit one or more TaskSpec records separated by a line containing only `.`.
- 3 to 8 tasks total. Fewer if the goal is small.
- id is `T01`, `T02`, ... in order.
- parent is `-` (L1 tasks have no parent).
- deps lists earlier ids only, comma-separated, or `-`.
- files lists concrete relative paths with `:+` (create), `:~` (modify), or `:-` (delete). Use `-` if genuinely unknown.
- kw is the list of literal words/phrases from the user's goal that must appear in the produced artifacts.
- cmd is extra shell commands for verification (e.g. `npm run build`), comma-separated, or `-`.
- Do NOT wrap the reply in code fences. Do NOT add prose.

FORMAT:
- Reply in ALF. One or more records separated by a line containing only `.`.
- Each line is `key value`. Arrays are comma-separated.
- Use `-` for empty/none. Do NOT use quotes.
- Close multi-line blocks with `:end`.
- Do NOT add prose before or after the record(s).

EXAMPLE:
id T01
title Project scaffold
desc Initialize project structure and install dependencies
parent -
deps -
kw setup, scaffold
files package.json:+, tsconfig.json:+
cmd npm install
skip -
tools -
max_rounds 4
.
id T02
title Homepage component
desc Create the landing page React component
parent -
deps T01
kw homepage, landing
files src/pages/index.tsx:+
cmd -
skip -
tools -
max_rounds 4
.

User goal:
{goal}
"#
    )
}

fn build_l2_prompt(goal: &str, l1: &TaskSpec, artifacts: &ArtifactIndex) -> String {
    let l1_alf = alf::write(&l1.to_alf());
    let artifacts_body = render_artifacts_compact(artifacts);
    let parent_id = &l1.id;
    format!(
        r#"You are a task planner. You are given ONE high-level task. Break it into
2 to 6 concrete subtasks that a code-writing worker can execute in order.

HARD RULES:
- Reply in ALF. Emit one or more TaskSpec records separated by `.` lines.
- id is `{parent_id}.01`, `{parent_id}.02`, ...
- parent MUST be `{parent_id}`.
- Use the provided artifact list to reference files that already exist; do not recreate them.
- files lists only the files this specific subtask touches.

FORMAT:
- Reply in ALF. One or more records separated by a line containing only `.`.
- Each line is `key value`. Arrays are comma-separated.
- Use `-` for empty/none. Do NOT use quotes.
- Close multi-line blocks with `:end`.
- Do NOT add prose before or after the record(s).

Parent task:
{l1_alf}
.

artifacts:
{artifacts_body}
:end

Original user goal:
{goal}
"#
    )
}

fn render_artifacts_compact(artifacts: &ArtifactIndex) -> String {
    artifacts
        .entries
        .iter()
        .take(80)
        .map(|(path, entry)| {
            format!(
                "{}   {}, {}B",
                path.display(),
                entry.description,
                entry.size_bytes
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ── Retry logic ───────────────────────────────────────────────────────────────

async fn call_planner_with_retry(
    client: &OllamaClient,
    system_prompt: &str,
    parent: Option<&TaskId>,
    ctx_size: u32,
) -> Result<Vec<TaskSpec>> {
    let options = ChatOptions {
        temperature: Some(0.2),
        num_ctx: Some(ctx_size),
    };

    let messages = vec![Message::user(system_prompt)];
    let raw = llm_text(client, &messages, options.clone()).await?;

    match parse_plan_output(&raw, parent) {
        Ok(specs) => return Ok(specs),
        Err(first_err) => {
            // Retry once with the error appended
            let retry_prompt = format!(
                "{system_prompt}\n\n[PREVIOUS ATTEMPT FAILED: {first_err}. \
                 Please fix and try again. Reply with valid ALF only, no prose.]"
            );
            let messages2 = vec![Message::user(&retry_prompt)];
            let raw2 = llm_text(client, &messages2, options).await?;
            parse_plan_output(&raw2, parent)
                .with_context(|| format!("planner failed twice (last error: {first_err})"))
        }
    }
}

async fn llm_text(
    client: &OllamaClient,
    messages: &[Message],
    options: ChatOptions,
) -> Result<String> {
    match client.chat(messages, None, Some(options), |_| {}).await? {
        LlmResponse::Text { content, .. } => Ok(content),
        LlmResponse::ToolCalls { text, .. } => Ok(text),
    }
}

// ── Output parsing + validation ───────────────────────────────────────────────

fn parse_plan_output(raw: &str, parent: Option<&TaskId>) -> Result<Vec<TaskSpec>> {
    let cleaned = strip_surrounding_prose(raw);
    let records = alf::parse(&cleaned).map_err(|e| anyhow!("ALF parse error: {e:?}"))?;
    if records.is_empty() {
        return Err(anyhow!("planner returned empty ALF"));
    }
    let mut specs: Vec<TaskSpec> = records
        .iter()
        .map(TaskSpec::from_alf)
        .collect::<Result<_, AlfError>>()
        .map_err(|e| anyhow!("TaskSpec decode error: {e:?}"))?;

    // Merge extracted keywords into each spec
    for spec in &mut specs {
        let parent_goal = spec.description.clone();
        let extra = crate::orchestra::keywords::extract_keywords(&parent_goal);
        for kw in extra {
            if !spec.expected_keywords.contains(&kw) && spec.expected_keywords.len() < 30 {
                spec.expected_keywords.push(kw);
            }
        }
    }

    validate_ids(&specs, parent)?;
    validate_deps(&specs)?;
    Ok(specs)
}

fn validate_ids(specs: &[TaskSpec], parent: Option<&TaskId>) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for spec in specs {
        if spec.id.is_empty() {
            return Err(anyhow!("task has empty id"));
        }
        if !seen.insert(spec.id.clone()) {
            return Err(anyhow!("duplicate task id: {}", spec.id));
        }
        if spec.title.len() > 80 {
            return Err(anyhow!("title too long for {}: {} chars", spec.id, spec.title.len()));
        }
        if spec.description.len() > 400 {
            return Err(anyhow!("desc too long for {}: {} chars", spec.id, spec.description.len()));
        }
        // Validate parent field
        if let Some(expected_parent) = parent {
            if let Some(ref p) = spec.parent {
                if !p.is_empty() && p != "-" && p != expected_parent {
                    return Err(anyhow!(
                        "task {} has wrong parent: expected {expected_parent}, got {p}",
                        spec.id
                    ));
                }
            }
        }
    }
    Ok(())
}

fn validate_deps(specs: &[TaskSpec]) -> Result<()> {
    let ids: std::collections::HashSet<&str> = specs.iter().map(|s| s.id.as_str()).collect();
    for spec in specs {
        for dep in &spec.deps {
            if dep.is_empty() || dep == "-" {
                continue;
            }
            if !ids.contains(dep.as_str()) {
                return Err(anyhow!(
                    "task {} has unknown dep: {dep} (not in plan)",
                    spec.id
                ));
            }
            // dep must be an earlier task — check order by position
            let self_pos = specs.iter().position(|s| s.id == spec.id).unwrap();
            let dep_pos = specs.iter().position(|s| &s.id == dep);
            if let Some(dep_pos) = dep_pos {
                if dep_pos >= self_pos {
                    return Err(anyhow!(
                        "task {} deps on {dep} which comes later in the plan",
                        spec.id
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Drop prose that may appear before or after ALF record(s).
fn cap_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn strip_surrounding_prose(raw: &str) -> String {
    // Drop leading lines until we hit one that looks like a `key value` line
    // or a standalone `.` separator.
    let alf_key_re = regex::Regex::new(r"^[a-z_][a-z0-9_]*(\s|:$)").unwrap();
    let mut lines = raw.lines();
    let mut out_lines: Vec<&str> = Vec::new();
    let mut started = false;

    for line in lines.by_ref() {
        if !started {
            let trimmed = line.trim();
            if trimmed == "." || alf_key_re.is_match(trimmed) {
                started = true;
                out_lines.push(line);
            }
        } else {
            out_lines.push(line);
        }
    }
    out_lines.join("\n")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_l1_plan() {
        let raw = "id T01\ntitle First task\ndesc Do something useful\nparent -\ndeps -\nkw foo, bar\nfiles src/main.rs:+\ncmd -\nskip -\ntools -\nmax_rounds 4\n.\nid T02\ntitle Second task\ndesc Another thing\nparent -\ndeps T01\nkw baz\nfiles src/lib.rs:+\ncmd -\nskip -\ntools -\nmax_rounds 4\n.";
        let specs = parse_plan_output(raw, None).unwrap();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].id, "T01");
        assert_eq!(specs[1].id, "T02");
    }

    #[test]
    fn test_parse_rejects_duplicate_ids() {
        let raw = "id T01\ntitle A\ndesc B\nparent -\ndeps -\nkw x\nfiles -\ncmd -\nskip -\ntools -\nmax_rounds 4\n.\nid T01\ntitle C\ndesc D\nparent -\ndeps -\nkw y\nfiles -\ncmd -\nskip -\ntools -\nmax_rounds 4\n.";
        assert!(parse_plan_output(raw, None).is_err());
    }

    #[test]
    fn test_parse_rejects_forward_dep() {
        // T01 deps T02 but T02 comes after
        let raw = "id T01\ntitle A\ndesc B\nparent -\ndeps T02\nkw x\nfiles -\ncmd -\nskip -\ntools -\nmax_rounds 4\n.\nid T02\ntitle C\ndesc D\nparent -\ndeps -\nkw y\nfiles -\ncmd -\nskip -\ntools -\nmax_rounds 4\n.";
        assert!(parse_plan_output(raw, None).is_err());
    }

    #[test]
    fn test_strip_surrounding_prose() {
        let raw = "Here is your plan:\n\nid T01\ntitle foo\ndesc bar\nparent -\ndeps -\nkw x\nfiles -\ncmd -\nskip -\ntools -\nmax_rounds 4\n.\n\nHope that helps!";
        let cleaned = strip_surrounding_prose(raw);
        assert!(cleaned.starts_with("id T01"));
    }

    #[test]
    fn test_parse_wrong_parent_rejected() {
        let raw = "id T01.01\ntitle A\ndesc B\nparent T99\ndeps -\nkw x\nfiles -\ncmd -\nskip -\ntools -\nmax_rounds 4\n.";
        assert!(parse_plan_output(raw, Some(&"T01".to_string())).is_err());
    }

    #[test]
    fn test_keywords_merged_into_specs() {
        let raw = "id T01\ntitle Create landing page\ndesc Build a homepage with navigation and contact form\nparent -\ndeps -\nkw nav\nfiles index.html:+\ncmd -\nskip -\ntools -\nmax_rounds 4\n.";
        let specs = parse_plan_output(raw, None).unwrap();
        // The description is mined for keywords and merged
        assert!(!specs[0].expected_keywords.is_empty());
    }
}
