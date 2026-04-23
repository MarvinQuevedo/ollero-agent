use anyhow::{anyhow, Result};

use crate::ollama::client::OllamaClient;
use crate::ollama::types::{ChatOptions, LlmResponse, Message};
use crate::orchestra::alf::{self, AlfError, FromAlf, ToAlf};
use crate::orchestra::types::{
    CheckOutcome, Diagnosis, Event, RetryStrategy, TaskReport, TaskSpec,
    ValidationReport,
};

// ── Public API ────────────────────────────────────────────────────────────────

pub async fn diagnose(
    client: &OllamaClient,
    spec: &TaskSpec,
    report: &TaskReport,
    validation: &ValidationReport,
    last_tool_event: Option<&Event>,
    ctx_size: u32,
) -> Result<Diagnosis> {
    // --- Deterministic short-circuits (no LLM) ---
    if let Some(shortcut) = deterministic_shortcut(spec, report, validation) {
        return Ok(shortcut);
    }

    let user_prompt = build_user_prompt(spec, report, validation, last_tool_event);
    let system_prompt = build_system_prompt();
    let options = ChatOptions { temperature: Some(0.1), num_ctx: Some(ctx_size) };
    let messages = vec![
        Message::system(&system_prompt),
        Message::user(&user_prompt),
    ];

    let raw = llm_text(client, &messages, options.clone()).await?;
    match parse_diagnosis(&raw) {
        Ok(d) => return Ok(d),
        Err(first_err) => {
            let retry_msg = format!(
                "{user_prompt}\n\n[PREVIOUS ATTEMPT FAILED: {first_err}. \
                 Reply with valid ALF only, no prose, no code fences.]"
            );
            let messages2 = vec![
                Message::system(&system_prompt),
                Message::user(&retry_msg),
            ];
            let raw2 = llm_text(client, &messages2, options).await?;
            match parse_diagnosis(&raw2) {
                Ok(d) => Ok(d),
                Err(_) => Ok(fallback_diagnosis()),
            }
        }
    }
}

// ── Deterministic short-circuits ──────────────────────────────────────────────

fn deterministic_shortcut(
    spec: &TaskSpec,
    report: &TaskReport,
    validation: &ValidationReport,
) -> Option<Diagnosis> {
    use crate::orchestra::types::TaskStatus;

    // Worker already said NeedsReview — don't retry
    if report.status == TaskStatus::NeedsReview {
        return Some(Diagnosis {
            root_cause: "Worker marked task as needs_review".into(),
            strategy: RetryStrategy::EscalateToUser,
            hint: None,
        });
    }

    let failures: Vec<&(String, CheckOutcome)> = validation
        .outcomes
        .iter()
        .filter(|(_, o)| matches!(o, CheckOutcome::Fail { .. }))
        .collect();

    // Only FileExists failed and worker said Ok → hint to create that file
    if failures.len() == 1 {
        if let (name, CheckOutcome::Fail { reason }) = failures[0] {
            if name.starts_with("FileExists(") {
                let path = name
                    .trim_start_matches("FileExists(")
                    .trim_end_matches(')');
                return Some(Diagnosis {
                    root_cause: format!("Worker did not create {path}"),
                    strategy: RetryStrategy::RetryWithHint,
                    hint: Some(format!(
                        "The file `{path}` is missing; create it. Error: {reason}"
                    )),
                });
            }

            // CommandExitsZero timeout (exit 124)
            if name.starts_with("CommandExitsZero(") && reason.contains("timed out") {
                return Some(Diagnosis {
                    root_cause: "Command timed out".into(),
                    strategy: RetryStrategy::RetryAsIs,
                    hint: None,
                });
            }
        }
    }

    // Only Soft signals and score ≥ 0.6 → NeedsReview, do not retry
    if failures.is_empty() && validation.score >= 0.6 {
        return Some(Diagnosis {
            root_cause: "Validation passed with soft signals; needs human review".into(),
            strategy: RetryStrategy::EscalateToUser,
            hint: None,
        });
    }

    // Check if this is the second attempt with the same failures — suggest replanning
    // (This heuristic is applied by the driver's attempt tracking; here we just emit
    //  the signal for same-failures-twice. The driver will call diagnose per attempt.)
    let _ = spec; // reserved for future per-spec heuristics
    None
}

// ── System prompt ─────────────────────────────────────────────────────────────

fn build_system_prompt() -> String {
    r#"You are a failure diagnoser for a software agent. You are given ONE
failed task and its validation results. Your job is to decide what to do
next.

HARD RULES:
- Reply in ALF with a single Diagnosis record.
- Do NOT attempt to solve the task. Only diagnose.
- Do NOT call tools.
- hint (if present) MUST be concrete and under 300 chars.
- Do NOT wrap in code fences. Do NOT add prose.

FORMAT:
- Each line is `key value`. Terminate with `.` on its own line.
- Use `-` for empty / none.

Diagnosis fields:
  root_cause — one line, ≤ 200 chars
  strategy   — one of: RetryAsIs | RetryWithHint | ReplanSubtree | Skip | EscalateToUser
  hint       — ≤ 300 chars, or `-` (required when strategy is RetryWithHint)

Strategy guidance:
- RetryAsIs      transient failure (network, timeout); same spec should work.
- RetryWithHint  worker misunderstood one concrete thing; `hint` must state it.
- ReplanSubtree  the L2 plan itself was wrong; parent L1 needs re-expansion.
- Skip           task is not critical and blocking the run is worse than missing it.
- EscalateToUser the failure requires information the agent does not have.

EXAMPLE:
root_cause Worker created styles.css but did not create index.html
strategy RetryWithHint
hint Start by creating src/index.html; styles.css already exists on disk
."#
    .to_string()
}

// ── User prompt ───────────────────────────────────────────────────────────────

fn build_user_prompt(
    spec: &TaskSpec,
    report: &TaskReport,
    validation: &ValidationReport,
    last_tool_event: Option<&Event>,
) -> String {
    let spec_alf = alf::write(&spec.to_alf());
    let report_alf = alf::write(&report.to_alf());

    // Top-5 failures first, then soft signals
    let mut outcome_lines: Vec<String> = Vec::new();
    let failures: Vec<_> = validation
        .outcomes
        .iter()
        .filter(|(_, o)| matches!(o, CheckOutcome::Fail { .. }))
        .take(5)
        .collect();
    let softs: Vec<_> = validation
        .outcomes
        .iter()
        .filter(|(_, o)| matches!(o, CheckOutcome::Soft(_)))
        .take(3)
        .collect();

    for (name, outcome) in failures.iter().chain(softs.iter()) {
        let detail = match outcome {
            CheckOutcome::Fail { reason } => format!("Fail: {reason}"),
            CheckOutcome::Soft(s) => format!("Soft: {s:.2}"),
            CheckOutcome::Pass => "Pass".into(),
        };
        outcome_lines.push(format!("- {name}: {detail}"));
    }

    let tool_section = if let Some(ev) = last_tool_event {
        let payload_str = serde_json::to_string(&ev.payload).unwrap_or_default();
        let truncated = cap_chars(&payload_str, 400);
        format!("\nLast tool event:\n  output {truncated}")
    } else {
        String::new()
    };

    format!(
        "Task spec:\n{spec_alf}\n.\n\nWorker report:\n{report_alf}\n.\n\nValidation outcomes (failures first):\n{outcomes}\n{tool_section}",
        outcomes = outcome_lines.join("\n"),
    )
}

// ── Parse + validate ──────────────────────────────────────────────────────────

fn parse_diagnosis(raw: &str) -> Result<Diagnosis> {
    let cleaned = strip_prose(raw);
    let rec = alf::parse_one(&cleaned).map_err(|e| anyhow!("ALF parse error: {e:?}"))?;
    let d = Diagnosis::from_alf(&rec).map_err(|e: AlfError| anyhow!("decode error: {e:?}"))?;

    if d.root_cause.len() > 200 {
        return Err(anyhow!("root_cause too long: {} chars", d.root_cause.len()));
    }
    if let Some(ref h) = d.hint {
        if h.len() > 300 {
            return Err(anyhow!("hint too long: {} chars", h.len()));
        }
    }
    if d.strategy == RetryStrategy::RetryWithHint && d.hint.is_none() {
        return Err(anyhow!("RetryWithHint requires a non-empty hint"));
    }

    Ok(d)
}

fn fallback_diagnosis() -> Diagnosis {
    Diagnosis {
        root_cause: "diagnoser did not return valid ALF".into(),
        strategy: RetryStrategy::EscalateToUser,
        hint: None,
    }
}

fn strip_prose(raw: &str) -> String {
    let alf_key_re = regex::Regex::new(r"^[a-z_][a-z0-9_]*(\s|:$)").unwrap();
    let mut out_lines: Vec<&str> = Vec::new();
    let mut started = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        if !started {
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

fn cap_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestra::types::{CheckOutcome, RetryStrategy, Verdict};

    fn make_validation(outcomes: Vec<(&str, CheckOutcome)>, score: f32) -> ValidationReport {
        ValidationReport {
            task_id: "T01".into(),
            outcomes: outcomes
                .into_iter()
                .map(|(n, o)| (n.to_string(), o))
                .collect(),
            verdict: if score >= 0.7 {
                Verdict::Ok
            } else if score >= 0.5 {
                Verdict::Uncertain
            } else {
                Verdict::Failed
            },
            score,
        }
    }

    fn make_report(status: crate::orchestra::types::TaskStatus) -> TaskReport {
        use std::path::PathBuf;
        TaskReport {
            task_id: "T01".into(),
            attempt: 1,
            status,
            summary: "did stuff".into(),
            files_touched: vec![PathBuf::from("foo.rs")],
            started_at: 0,
            finished_at: 1,
            worker_tool_calls: 1,
            tokens_used: None,
        }
    }

    fn default_spec() -> TaskSpec {
        TaskSpec::default()
    }

    #[test]
    fn test_shortcut_needs_review() {
        use crate::orchestra::types::TaskStatus;
        let report = make_report(TaskStatus::NeedsReview);
        let v = make_validation(vec![], 0.8);
        let d = deterministic_shortcut(&default_spec(), &report, &v).unwrap();
        assert_eq!(d.strategy, RetryStrategy::EscalateToUser);
    }

    #[test]
    fn test_shortcut_file_exists_only() {
        use crate::orchestra::types::TaskStatus;
        let report = make_report(TaskStatus::Failed);
        let v = make_validation(
            vec![(
                "FileExists(src/main.rs)",
                CheckOutcome::Fail { reason: "not found".into() },
            )],
            0.0,
        );
        let d = deterministic_shortcut(&default_spec(), &report, &v).unwrap();
        assert_eq!(d.strategy, RetryStrategy::RetryWithHint);
        assert!(d.hint.as_deref().unwrap_or("").contains("src/main.rs"));
    }

    #[test]
    fn test_shortcut_high_score_only_soft() {
        use crate::orchestra::types::TaskStatus;
        let report = make_report(TaskStatus::Failed);
        let v = make_validation(vec![("syntax", CheckOutcome::Soft(0.8))], 0.8);
        let d = deterministic_shortcut(&default_spec(), &report, &v).unwrap();
        assert_eq!(d.strategy, RetryStrategy::EscalateToUser);
    }

    #[test]
    fn test_shortcut_timeout_returns_retry_as_is() {
        use crate::orchestra::types::TaskStatus;
        let report = make_report(TaskStatus::Failed);
        let v = make_validation(
            vec![(
                "CommandExitsZero(cargo check)",
                CheckOutcome::Fail { reason: "`cargo check` timed out after 60s".into() },
            )],
            0.0,
        );
        let d = deterministic_shortcut(&default_spec(), &report, &v).unwrap();
        assert_eq!(d.strategy, RetryStrategy::RetryAsIs);
    }

    #[test]
    fn test_parse_diagnosis_valid() {
        let raw = "root_cause Worker did not create the file\nstrategy RetryWithHint\nhint Please create src/index.html first\n.";
        let d = parse_diagnosis(raw).unwrap();
        assert_eq!(d.root_cause, "Worker did not create the file");
        assert_eq!(d.strategy, RetryStrategy::RetryWithHint);
        assert!(d.hint.is_some());
    }

    #[test]
    fn test_parse_diagnosis_retry_with_hint_requires_hint() {
        let raw = "root_cause Something\nstrategy RetryWithHint\nhint -\n.";
        // hint is `-` meaning None → should fail validation
        let result = parse_diagnosis(raw);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_diagnosis_root_cause_too_long() {
        let long = "x".repeat(201);
        let raw = format!("root_cause {long}\nstrategy Skip\nhint -\n.");
        assert!(parse_diagnosis(&raw).is_err());
    }

    #[test]
    fn test_fallback_diagnosis() {
        let d = fallback_diagnosis();
        assert_eq!(d.strategy, RetryStrategy::EscalateToUser);
        assert!(d.hint.is_none());
    }
}
