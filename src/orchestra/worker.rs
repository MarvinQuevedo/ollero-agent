use anyhow::Result;
use std::path::PathBuf;

use crate::ollama::client::OllamaClient;
use crate::ollama::types::{ChatOptions, LlmResponse, Message, ToolCallItem};
use crate::orchestra::alf::{self, FromAlf, ToAlf};
use crate::orchestra::types::{
    ArtifactIndex, TaskReport, TaskSpec, TaskStatus,
};

const MAX_TOOL_CALLS: u32 = 12;

// ── Public API ────────────────────────────────────────────────────────────────

/// Run a micro-session agentic loop for one leaf task and return a TaskReport.
///
/// The caller (driver) fills in `attempt`, `started_at`, `finished_at` after
/// this returns.
pub async fn run_worker(
    client: &OllamaClient,
    spec: &TaskSpec,
    goal: &str,
    artifacts: &ArtifactIndex,
    ctx_size: u32,
    hint: Option<&str>,
    _quiet: bool,
) -> Result<TaskReport> {
    let started_at = current_unix_secs();

    let tool_defs = filtered_tools(&spec.allowed_tools);
    let tools_slice: Option<&[_]> = if tool_defs.is_empty() {
        None
    } else {
        Some(&tool_defs)
    };

    let options = ChatOptions {
        temperature: Some(0.3),
        num_ctx: Some(ctx_size),
    };

    let system = build_system_prompt(spec, goal, artifacts);
    let user = build_user_prompt(spec, hint);

    let mut history: Vec<Message> = vec![
        Message::system(&system),
        Message::user(&user),
    ];

    let mut total_tool_calls: u32 = 0;
    let mut total_tokens: u32 = 0;

    for _round in 0..spec.max_rounds {
        let response = client
            .chat(&history, tools_slice, Some(options.clone()), |_| {})
            .await?;

        match response {
            LlmResponse::Text { content, stats } => {
                total_tokens += stats.completion_tokens;
                let finished_at = current_unix_secs();
                let mut report = parse_final_report(&content, spec);
                report.started_at = started_at;
                report.finished_at = finished_at;
                report.worker_tool_calls = total_tool_calls;
                report.tokens_used = Some(total_tokens);
                return Ok(report);
            }
            LlmResponse::ToolCalls { calls, text, stats } => {
                total_tokens += stats.completion_tokens;

                // Append assistant turn with tool calls
                history.push(Message::assistant_tool_calls(calls.clone(), &text));

                for call in &calls {
                    if total_tool_calls >= MAX_TOOL_CALLS {
                        let finished_at = current_unix_secs();
                        return Ok(TaskReport {
                            task_id: spec.id.clone(),
                            attempt: 0,
                            status: TaskStatus::Failed,
                            summary: "tool call quota exceeded".into(),
                            files_touched: Vec::new(),
                            started_at,
                            finished_at,
                            worker_tool_calls: total_tool_calls,
                            tokens_used: Some(total_tokens),
                        });
                    }

                    let result = dispatch_call(call).await;
                    total_tool_calls += 1;

                    let tool_result = match result {
                        Ok(output) => output,
                        Err(e) => format!("ERROR: {e}"),
                    };

                    history.push(Message {
                        role: "tool".into(),
                        content: tool_result,
                        tool_calls: None,
                        tool_name: Some(call.function.name.clone()),
                    });
                }
            }
        }
    }

    // Max rounds exhausted without a final report
    let finished_at = current_unix_secs();
    Ok(TaskReport {
        task_id: spec.id.clone(),
        attempt: 0,
        status: TaskStatus::Failed,
        summary: format!("max rounds ({}) reached without completing the task", spec.max_rounds),
        files_touched: Vec::new(),
        started_at,
        finished_at,
        worker_tool_calls: total_tool_calls,
        tokens_used: Some(total_tokens),
    })
}

// ── Prompt builders ───────────────────────────────────────────────────────────

fn build_system_prompt(spec: &TaskSpec, goal: &str, artifacts: &ArtifactIndex) -> String {
    let goal_capped: String = goal.chars().take(500).collect();
    let artifacts_body = render_artifacts(artifacts);
    format!(
        r#"You are a worker agent. You execute ONE concrete task, then stop.

You MUST:
- Stay focused on the task below. Do not work on related tasks.
- Use tools to read, search, edit, and run commands as needed.
- When finished, reply in ALF with a single FinalReport record.
- Keep your summary under 300 characters.

You MUST NOT:
- Invent files that were not created or modified by your actions.
- Continue working after you emit the FinalReport.
- Wrap the FinalReport in code fences.
- Add prose before or after the record.

FORMAT:
- Each line is `key value`. Arrays are comma-separated.
- Terminate the record with a line containing only `.`.
- Use `-` for empty / none.

FinalReport fields:
  status         — `ok` | `failed` | `needs_review`
  summary        — ≤ 300 chars; single line (or use `summary:` ... `:end` block)
  files_touched  — comma-separated relative paths, or `-`

EXAMPLE:
status ok
summary Created src/index.html with hero, services grid, and contact form
files_touched src/index.html, src/styles.css
.

Original user goal (context only, do NOT pursue it directly):
{goal_capped}

artifacts:
{artifacts_body}
:end
"#
    )
}

fn build_user_prompt(spec: &TaskSpec, hint: Option<&str>) -> String {
    let spec_alf = alf::write(&spec.to_alf());
    let hint_section = match hint {
        Some(h) if !h.is_empty() => format!("\nHint from diagnoser:\n{h}\n"),
        _ => String::new(),
    };
    format!(
        "Task to execute:\n{spec_alf}\n.{hint_section}\nReply by calling tools to execute the task, then emit the FinalReport."
    )
}

fn render_artifacts(artifacts: &ArtifactIndex) -> String {
    if artifacts.entries.is_empty() {
        return "(none)".into();
    }
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

// ── Tool filtering ────────────────────────────────────────────────────────────

fn filtered_tools(
    allowed: &[String],
) -> Vec<crate::ollama::types::ToolDefinition> {
    let all = crate::tools::all_definitions();
    if allowed.is_empty() || (allowed.len() == 1 && allowed[0] == "-") {
        return all;
    }
    all.into_iter()
        .filter(|d| allowed.iter().any(|a| a == d.function.name))
        .collect()
}

// ── Tool dispatch ─────────────────────────────────────────────────────────────

async fn dispatch_call(call: &ToolCallItem) -> Result<String> {
    crate::tools::dispatch(
        &call.function.name,
        &call.function.arguments,
        true, // always quiet in worker
    )
    .await
}

// ── Final report parsing ──────────────────────────────────────────────────────

fn parse_final_report(raw: &str, spec: &TaskSpec) -> TaskReport {
    let cleaned = strip_prose(raw);
    if let Ok(rec) = alf::parse_one(&cleaned) {
        if let Ok(mut report) = TaskReport::from_alf(&rec) {
            report.task_id = spec.id.clone();
            report.summary = report.summary.chars().take(300).collect();
            return report;
        }
    }
    // Fallback: worker did not emit structured ALF
    TaskReport {
        task_id: spec.id.clone(),
        attempt: 0,
        status: TaskStatus::NeedsReview,
        summary: "worker did not return a structured report".into(),
        files_touched: Vec::new(),
        started_at: 0,
        finished_at: 0,
        worker_tool_calls: 0,
        tokens_used: None,
    }
}

fn strip_prose(raw: &str) -> String {
    let alf_key_re = regex::Regex::new(r"^[a-z_][a-z0-9_]*\s").unwrap();
    let mut out_lines: Vec<&str> = Vec::new();
    let mut started = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        if !started {
            if trimmed == "." || alf_key_re.is_match(trimmed) || trimmed.starts_with("status ") {
                started = true;
                out_lines.push(line);
            }
        } else {
            out_lines.push(line);
        }
    }
    out_lines.join("\n")
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn current_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestra::types::{ArtifactIndex, TaskStatus};

    fn make_spec(id: &str, max_rounds: u32) -> TaskSpec {
        TaskSpec {
            id: id.into(),
            title: "Test task".into(),
            description: "Do something".into(),
            max_rounds,
            ..Default::default()
        }
    }

    #[test]
    fn test_parse_final_report_ok() {
        let raw = "status ok\nsummary Created the file successfully\nfiles_touched src/index.html\n.";
        let spec = make_spec("T01.01", 4);
        let report = parse_final_report(raw, &spec);
        assert_eq!(report.status, TaskStatus::Ok);
        assert_eq!(report.summary, "Created the file successfully");
        assert_eq!(report.files_touched, vec![PathBuf::from("src/index.html")]);
        assert_eq!(report.task_id, "T01.01");
    }

    #[test]
    fn test_parse_final_report_failed() {
        let raw = "status failed\nsummary Could not write the file, permission denied\nfiles_touched -\n.";
        let spec = make_spec("T01.02", 4);
        let report = parse_final_report(raw, &spec);
        assert_eq!(report.status, TaskStatus::Failed);
        assert!(report.files_touched.is_empty());
    }

    #[test]
    fn test_parse_final_report_needs_review_fallback() {
        let raw = "I did some work but forgot to emit the report.";
        let spec = make_spec("T01.03", 4);
        let report = parse_final_report(raw, &spec);
        assert_eq!(report.status, TaskStatus::NeedsReview);
        assert!(report.summary.contains("structured report"));
    }

    #[test]
    fn test_parse_final_report_strips_prose() {
        let raw = "Here is my report:\n\nstatus ok\nsummary Done\nfiles_touched a.rs\n.\n\nLet me know!";
        let spec = make_spec("T01.04", 4);
        let report = parse_final_report(raw, &spec);
        assert_eq!(report.status, TaskStatus::Ok);
    }

    #[test]
    fn test_parse_final_report_summary_truncated() {
        let long_summary = "x".repeat(350);
        let raw = format!("status ok\nsummary {long_summary}\nfiles_touched -\n.");
        let spec = make_spec("T01.05", 4);
        let report = parse_final_report(&raw, &spec);
        assert!(report.summary.len() <= 300);
    }

    #[test]
    fn test_build_user_prompt_no_hint() {
        let spec = make_spec("T01.01", 4);
        let prompt = build_user_prompt(&spec, None);
        assert!(prompt.contains("Task to execute:"));
        assert!(!prompt.contains("Hint from diagnoser:"));
    }

    #[test]
    fn test_build_user_prompt_with_hint() {
        let spec = make_spec("T01.01", 4);
        let prompt = build_user_prompt(&spec, Some("Create index.html first"));
        assert!(prompt.contains("Hint from diagnoser:"));
        assert!(prompt.contains("Create index.html first"));
    }

    #[test]
    fn test_build_system_prompt_contains_goal() {
        let spec = make_spec("T01.01", 4);
        let artifacts = ArtifactIndex::default();
        let prompt = build_system_prompt(&spec, "Build a clinic website", &artifacts);
        assert!(prompt.contains("Build a clinic website"));
    }

    #[test]
    fn test_filtered_tools_empty_means_all() {
        let all = crate::tools::all_definitions();
        let filtered = filtered_tools(&[]);
        assert_eq!(filtered.len(), all.len());
    }

    #[test]
    fn test_filtered_tools_subset() {
        let filtered = filtered_tools(&["read_file".into(), "write_file".into()]);
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().any(|t| t.function.name == "read_file"));
        assert!(filtered.iter().any(|t| t.function.name == "write_file"));
    }

    #[test]
    fn test_filtered_tools_dash_means_all() {
        let all = crate::tools::all_definitions();
        let filtered = filtered_tools(&["-".into()]);
        assert_eq!(filtered.len(), all.len());
    }
}
