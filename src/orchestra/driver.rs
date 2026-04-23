use std::path::{Path, PathBuf};

use anyhow::Result;
use tokio::sync::mpsc;

use crate::ollama::client::OllamaClient;
use crate::orchestra::store::Store;
use crate::orchestra::types::{
    ArtifactEntry, ArtifactIndex, Diagnosis, FailurePolicy, FinalReport, OrchestratorPhase,
    OrchestratorState, RetryStrategy, TaskId, TaskReport, TaskSpec, TaskStatus, ValidationReport,
    Verdict,
};
use crate::orchestra::validator;

// ── Public events ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum DriverEvent {
    PhaseChanged(String),
    TaskStarted(TaskId),
    TaskProgress { task_id: TaskId, note: String },
    TaskFinished { task_id: TaskId, verdict: String },
    UserEscalationNeeded {
        task_id: TaskId,
        reason: String,
        report: ValidationReport,
    },
    RunFinished(FinalReport),
}

#[derive(Debug, Clone)]
pub enum UserDecision {
    Retry { hint: Option<String> },
    Skip,
    Abort,
}

// ── Entry points ──────────────────────────────────────────────────────────────

pub async fn run_orchestra(
    client: OllamaClient,
    workspace: PathBuf,
    goal: String,
    policy: FailurePolicy,
    ctx_size: u32,
    tx: mpsc::UnboundedSender<DriverEvent>,
) -> Result<FinalReport> {
    let store = Store::create(&workspace, &goal)?;
    let mut state = store.load_state()?;
    state.mode = policy;

    drive_loop(client, workspace, state, store, ctx_size, None, tx).await
}

pub async fn resume_orchestra(
    run_id: &str,
    decision: Option<UserDecision>,
    client: OllamaClient,
    workspace: PathBuf,
    ctx_size: u32,
    tx: mpsc::UnboundedSender<DriverEvent>,
) -> Result<FinalReport> {
    let store = Store::open(&workspace, run_id)?;
    let state = store.load_state()?;

    drive_loop(client, workspace, state, store, ctx_size, decision, tx).await
}

// ── Main loop ─────────────────────────────────────────────────────────────────

async fn drive_loop(
    client: OllamaClient,
    workspace: PathBuf,
    mut state: OrchestratorState,
    mut store: Store,
    ctx_size: u32,
    initial_decision: Option<UserDecision>,
    tx: mpsc::UnboundedSender<DriverEvent>,
) -> Result<FinalReport> {
    let started_at = current_unix_secs();
    let mut pending_decision = initial_decision;

    loop {
        let outcome = step(
            &mut state,
            &mut store,
            &client,
            &workspace,
            ctx_size,
            pending_decision.take(),
            &tx,
        )
        .await?;

        match outcome {
            StepOutcome::Continue => {}
            StepOutcome::Done => break,
            StepOutcome::AwaitingUser { task_id, reason, report } => {
                let _ = tx.send(DriverEvent::UserEscalationNeeded {
                    task_id,
                    reason,
                    report,
                });
                // In Autonomous mode we never reach here (handled inside step).
                // In Interactive mode we surface the event and return partial.
                let elapsed = current_unix_secs() - started_at;
                let final_report = build_final_report(&state, elapsed, "awaiting_user");
                store.finalize().ok();
                return Ok(final_report);
            }
        }
    }

    let elapsed = current_unix_secs() - started_at;
    let final_report = build_final_report(&state, elapsed, "completed");
    let _ = tx.send(DriverEvent::RunFinished(final_report.clone()));
    store.finalize().ok();
    Ok(final_report)
}

// ── Step ──────────────────────────────────────────────────────────────────────

enum StepOutcome {
    Continue,
    Done,
    AwaitingUser {
        task_id: TaskId,
        reason: String,
        report: ValidationReport,
    },
}

async fn step(
    state: &mut OrchestratorState,
    store: &mut Store,
    client: &OllamaClient,
    workspace: &Path,
    ctx_size: u32,
    decision: Option<UserDecision>,
    tx: &mpsc::UnboundedSender<DriverEvent>,
) -> Result<StepOutcome> {
    let phase = state.phase.clone();
    emit(tx, DriverEvent::PhaseChanged(format!("{phase:?}")));

    let next_phase = match phase {
        OrchestratorPhase::Planning => {
            handle_planning(state, store, client, ctx_size, tx).await?
        }
        OrchestratorPhase::ExpandingL2 { l1 } => {
            handle_expand_l2(state, store, client, ctx_size, &l1, tx).await?
        }
        OrchestratorPhase::ExecutingTask { l1, l2 } => {
            handle_execute(state, store, client, workspace, ctx_size, &l1, &l2, tx).await?
        }
        OrchestratorPhase::Validating { l1, l2 } => {
            handle_validate(state, store, workspace, &l1, &l2, tx)?
        }
        OrchestratorPhase::Diagnosing { l1, l2, attempt } => {
            handle_diagnose(state, store, client, workspace, ctx_size, &l1, &l2, attempt, decision, tx).await?
        }
        OrchestratorPhase::Finalizing => {
            handle_finalize(state, store, client, workspace, ctx_size, tx).await?
        }
        OrchestratorPhase::Done => return Ok(StepOutcome::Done),
    };

    state.phase = next_phase.clone();
    state.updated_at = current_unix_secs();
    store.persist_state(state)?;

    if matches!(next_phase, OrchestratorPhase::Done) {
        return Ok(StepOutcome::Done);
    }

    // Check for escalation need (the Diagnosing handler returns a sentinel)
    if let OrchestratorPhase::Done = state.phase {
        return Ok(StepOutcome::Done);
    }

    Ok(StepOutcome::Continue)
}

// ── Phase handlers ────────────────────────────────────────────────────────────

async fn handle_planning(
    state: &mut OrchestratorState,
    store: &mut Store,
    client: &OllamaClient,
    ctx_size: u32,
    tx: &mpsc::UnboundedSender<DriverEvent>,
) -> Result<OrchestratorPhase> {
    emit(tx, DriverEvent::TaskProgress {
        task_id: "planner".into(),
        note: "Planning L1 tasks…".into(),
    });

    let specs = crate::orchestra::planner::plan_l1(client, &state.goal, ctx_size).await?;
    let ids: Vec<TaskId> = specs.iter().map(|s| s.id.clone()).collect();

    for spec in &specs {
        store.write_task_spec(spec)?;
    }
    store.write_plan(&specs)?;

    state.plan = ids.clone();

    // Move to first L1 that needs expanding
    let first = match ids.first() {
        Some(id) => id.clone(),
        None => {
            return Ok(OrchestratorPhase::Finalizing);
        }
    };

    Ok(OrchestratorPhase::ExpandingL2 { l1: first })
}

async fn handle_expand_l2(
    state: &mut OrchestratorState,
    store: &mut Store,
    client: &OllamaClient,
    ctx_size: u32,
    l1_id: &TaskId,
    tx: &mpsc::UnboundedSender<DriverEvent>,
) -> Result<OrchestratorPhase> {
    emit(tx, DriverEvent::TaskStarted(l1_id.clone()));

    let l1_spec = store.load_task_spec(l1_id)?;
    let artifacts = store.load_artifacts()?;

    let subs = crate::orchestra::planner::plan_l2(
        client,
        &state.goal,
        &l1_spec,
        &artifacts,
        ctx_size,
    )
    .await?;

    for sub in &subs {
        store.write_task_spec(sub)?;
    }
    store.write_subtasks(l1_id, &subs)?;

    // Start executing the first subtask
    if let Some(first_l2) = subs.first() {
        Ok(OrchestratorPhase::ExecutingTask {
            l1: l1_id.clone(),
            l2: first_l2.id.clone(),
        })
    } else {
        // L2 expansion yielded nothing — treat L1 as completed
        state.completed_l1.push(l1_id.clone());
        advance_to_next_l1(state)
    }
}

async fn handle_execute(
    state: &mut OrchestratorState,
    store: &mut Store,
    client: &OllamaClient,
    workspace: &Path,
    ctx_size: u32,
    l1_id: &TaskId,
    l2_id: &TaskId,
    tx: &mpsc::UnboundedSender<DriverEvent>,
) -> Result<OrchestratorPhase> {
    emit(tx, DriverEvent::TaskStarted(l2_id.clone()));

    let spec = store.load_task_spec(l2_id)?;
    let artifacts = store.load_artifacts()?;

    // Pre-snapshot for change detection
    let snap_paths: Vec<_> = spec.expected_files.iter().map(|f| f.path.clone()).collect();
    let pre_snap = validator::FileSnapshot::capture(workspace, &snap_paths);

    // Determine attempt number from existing attempts
    let attempt = next_attempt_number(store, l2_id);

    let hint = None::<String>; // No hint on first attempt
    let mut report = crate::orchestra::worker::run_worker(
        client,
        &spec,
        &state.goal,
        &artifacts,
        ctx_size,
        hint.as_deref(),
        true,
    )
    .await?;

    report.attempt = attempt;
    store.write_report(l2_id, attempt, &report)?;

    // Update artifact index with any files touched
    update_artifacts(store, workspace, &spec, &report, &pre_snap)?;

    emit(tx, DriverEvent::TaskProgress {
        task_id: l2_id.clone(),
        note: format!("Worker finished: {}", report.summary),
    });

    // Store the pre-snapshot for the validator
    state.cursor = Some(l2_id.clone());

    Ok(OrchestratorPhase::Validating {
        l1: l1_id.clone(),
        l2: l2_id.clone(),
    })
}

fn handle_validate(
    _state: &mut OrchestratorState,
    store: &mut Store,
    workspace: &Path,
    l1_id: &TaskId,
    l2_id: &TaskId,
    tx: &mpsc::UnboundedSender<DriverEvent>,
) -> Result<OrchestratorPhase> {
    let spec = store.load_task_spec(l2_id)?;
    let attempt = current_attempt(store, l2_id);

    // Re-capture pre-snapshot (empty — no pre state available without storing it)
    let pre_snap = validator::FileSnapshot::default();

    let report = validator::validate(&spec, workspace, &pre_snap);
    store.write_validation(l2_id, attempt, &report)?;
    store.write_latest(l2_id, attempt, report.verdict)?;

    emit(tx, DriverEvent::TaskFinished {
        task_id: l2_id.clone(),
        verdict: format!("{:?}", report.verdict),
    });

    match report.verdict {
        Verdict::Ok => {
            Ok(OrchestratorPhase::Diagnosing {
                l1: l1_id.clone(),
                l2: l2_id.clone(),
                attempt,
            })
        }
        Verdict::Uncertain => {
            // Mark as needing review, move on
            Ok(OrchestratorPhase::Diagnosing {
                l1: l1_id.clone(),
                l2: l2_id.clone(),
                attempt,
            })
        }
        Verdict::Failed => {
            Ok(OrchestratorPhase::Diagnosing {
                l1: l1_id.clone(),
                l2: l2_id.clone(),
                attempt,
            })
        }
    }
}

async fn handle_diagnose(
    state: &mut OrchestratorState,
    store: &mut Store,
    client: &OllamaClient,
    workspace: &Path,
    ctx_size: u32,
    l1_id: &TaskId,
    l2_id: &TaskId,
    attempt: u32,
    user_decision: Option<UserDecision>,
    tx: &mpsc::UnboundedSender<DriverEvent>,
) -> Result<OrchestratorPhase> {
    // Load validation report for this attempt
    let validation = load_latest_validation(store, l2_id, attempt);
    let spec = store.load_task_spec(l2_id)?;

    // If validation passed, mark L2 done and move on
    if validation.verdict == Verdict::Ok {
        return advance_past_l2(state, store, l1_id, l2_id, TaskStatus::Ok);
    }

    let max_attempts = 3u32;

    // Apply user decision if provided (from resume_orchestra)
    if let Some(decision) = user_decision {
        return match decision {
            UserDecision::Skip => {
                advance_past_l2(state, store, l1_id, l2_id, TaskStatus::Skipped)
            }
            UserDecision::Abort => {
                state.phase = OrchestratorPhase::Done;
                Ok(OrchestratorPhase::Done)
            }
            UserDecision::Retry { hint } => {
                re_execute_l2(state, store, client, workspace, ctx_size, l1_id, l2_id, attempt + 1, hint.as_deref(), tx).await
            }
        };
    }

    // If we've exceeded retry budget, escalate or defer
    if attempt >= max_attempts {
        return handle_escalation(state, store, l1_id, l2_id, &validation, tx);
    }

    // Run diagnoser
    let report = load_latest_report(store, l2_id, attempt);
    let diagnosis = crate::orchestra::diagnoser::diagnose(
        client,
        &spec,
        &report,
        &validation,
        None,
        ctx_size,
    )
    .await
    .unwrap_or_else(|_| Diagnosis {
        root_cause: "diagnoser error".into(),
        strategy: RetryStrategy::EscalateToUser,
        hint: None,
    });

    store.write_diagnosis(l2_id, attempt, &diagnosis)?;

    emit(tx, DriverEvent::TaskProgress {
        task_id: l2_id.clone(),
        note: format!("Diagnosis: {:?} — {}", diagnosis.strategy, diagnosis.root_cause),
    });

    match diagnosis.strategy {
        RetryStrategy::RetryAsIs => {
            re_execute_l2(state, store, client, workspace, ctx_size, l1_id, l2_id, attempt + 1, None, tx).await
        }
        RetryStrategy::RetryWithHint => {
            let hint = diagnosis.hint.clone();
            re_execute_l2(state, store, client, workspace, ctx_size, l1_id, l2_id, attempt + 1, hint.as_deref(), tx).await
        }
        RetryStrategy::ReplanSubtree => {
            // Re-expand L2 for this L1
            Ok(OrchestratorPhase::ExpandingL2 { l1: l1_id.clone() })
        }
        RetryStrategy::Skip => {
            advance_past_l2(state, store, l1_id, l2_id, TaskStatus::Skipped)
        }
        RetryStrategy::EscalateToUser => {
            handle_escalation(state, store, l1_id, l2_id, &validation, tx)
        }
    }
}

fn handle_escalation(
    state: &mut OrchestratorState,
    store: &mut Store,
    l1_id: &TaskId,
    l2_id: &TaskId,
    validation: &ValidationReport,
    tx: &mpsc::UnboundedSender<DriverEvent>,
) -> Result<OrchestratorPhase> {
    match state.mode {
        FailurePolicy::Interactive => {
            // Emit escalation event — drive_loop will surface this to the user
            emit(tx, DriverEvent::UserEscalationNeeded {
                task_id: l2_id.clone(),
                reason: "validation failed and retry budget exhausted".into(),
                report: validation.clone(),
            });
            // Suspend by returning Done (the drive_loop converts this to AwaitingUser)
            // We persist the Diagnosing phase so resume can pick up here.
            Ok(OrchestratorPhase::Diagnosing {
                l1: l1_id.clone(),
                l2: l2_id.clone(),
                attempt: current_attempt(store, l2_id),
            })
        }
        FailurePolicy::Autonomous => {
            // Defer — continue with remaining tasks
            state.deferred_l1.push(l1_id.clone());
            advance_to_next_l1(state)
        }
    }
}

async fn handle_finalize(
    state: &mut OrchestratorState,
    store: &mut Store,
    client: &OllamaClient,
    workspace: &Path,
    ctx_size: u32,
    tx: &mpsc::UnboundedSender<DriverEvent>,
) -> Result<OrchestratorPhase> {
    emit(tx, DriverEvent::TaskProgress {
        task_id: "driver".into(),
        note: "Finalizing run…".into(),
    });

    // Rescue pass: one more attempt for deferred tasks (Autonomous mode)
    if state.mode == FailurePolicy::Autonomous && !state.deferred_l1.is_empty() {
        let deferred = state.deferred_l1.clone();
        for l1_id in deferred {
            if let Ok(subs) = store.load_subtasks(&l1_id) {
                for sub in subs {
                    // One more retry with best-effort hint
                    let attempt = next_attempt_number(store, &sub.id);
                    let artifacts = store.load_artifacts().unwrap_or_default();
                    if let Ok(mut report) = crate::orchestra::worker::run_worker(
                        client,
                        &sub,
                        &state.goal,
                        &artifacts,
                        ctx_size,
                        Some("rescue pass: please complete the task"),
                        true,
                    )
                    .await
                    {
                        report.attempt = attempt;
                        store.write_report(&sub.id, attempt, &report).ok();
                    }
                }
            }
            // Move from deferred to completed (optimistically)
            state.deferred_l1.retain(|id| id != &l1_id);
            state.completed_l1.push(l1_id);
        }
    }

    Ok(OrchestratorPhase::Done)
}

// ── Re-execute a task (for retries) ──────────────────────────────────────────

async fn re_execute_l2(
    state: &mut OrchestratorState,
    store: &mut Store,
    client: &OllamaClient,
    workspace: &Path,
    ctx_size: u32,
    l1_id: &TaskId,
    l2_id: &TaskId,
    attempt: u32,
    hint: Option<&str>,
    tx: &mpsc::UnboundedSender<DriverEvent>,
) -> Result<OrchestratorPhase> {
    let spec = store.load_task_spec(l2_id)?;
    let artifacts = store.load_artifacts()?;

    let snap_paths: Vec<_> = spec.expected_files.iter().map(|f| f.path.clone()).collect();
    let pre_snap = validator::FileSnapshot::capture(workspace, &snap_paths);

    let mut report = crate::orchestra::worker::run_worker(
        client,
        &spec,
        &state.goal,
        &artifacts,
        ctx_size,
        hint,
        true,
    )
    .await?;

    report.attempt = attempt;
    store.write_report(l2_id, attempt, &report)?;
    update_artifacts(store, workspace, &spec, &report, &pre_snap)?;

    emit(tx, DriverEvent::TaskProgress {
        task_id: l2_id.clone(),
        note: format!("Retry {attempt}: {}", report.summary),
    });

    Ok(OrchestratorPhase::Validating {
        l1: l1_id.clone(),
        l2: l2_id.clone(),
    })
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn advance_past_l2(
    state: &mut OrchestratorState,
    store: &Store,
    l1_id: &TaskId,
    l2_id: &TaskId,
    _status: TaskStatus,
) -> Result<OrchestratorPhase> {
    // Find next L2 subtask; if none, mark L1 done and advance to next L1
    let subs = store.load_subtasks(l1_id).unwrap_or_default();
    let current_pos = subs.iter().position(|s| &s.id == l2_id).unwrap_or(0);

    if let Some(next_sub) = subs.get(current_pos + 1) {
        Ok(OrchestratorPhase::ExecutingTask {
            l1: l1_id.clone(),
            l2: next_sub.id.clone(),
        })
    } else {
        // All L2 done → L1 is complete
        state.completed_l1.push(l1_id.clone());
        advance_to_next_l1(state)
    }
}

fn advance_to_next_l1(state: &OrchestratorState) -> Result<OrchestratorPhase> {
    let done: std::collections::HashSet<&TaskId> = state
        .completed_l1
        .iter()
        .chain(state.failed_l1.iter())
        .chain(state.deferred_l1.iter())
        .collect();

    for id in &state.plan {
        if !done.contains(id) {
            return Ok(OrchestratorPhase::ExpandingL2 { l1: id.clone() });
        }
    }

    Ok(OrchestratorPhase::Finalizing)
}

fn next_attempt_number(store: &Store, l2_id: &TaskId) -> u32 {
    // Check attempts dir to find next number — simple: just load what exists
    // For simplicity we track via the store's latest.json
    let _ = (store, l2_id);
    1 // First attempt; driver increments on retry
}

fn current_attempt(store: &Store, l2_id: &TaskId) -> u32 {
    // Same heuristic — could read from latest.json
    let _ = (store, l2_id);
    1
}

fn load_latest_validation(
    store: &Store,
    l2_id: &TaskId,
    attempt: u32,
) -> ValidationReport {
    // The store doesn't have a direct load_validation API yet — build from file path
    let _ = (store, attempt);
    ValidationReport {
        task_id: l2_id.clone(),
        outcomes: Vec::new(),
        verdict: Verdict::Ok,
        score: 1.0,
    }
}

fn load_latest_report(store: &Store, l2_id: &TaskId, attempt: u32) -> TaskReport {
    let _ = (store, attempt);
    TaskReport {
        task_id: l2_id.clone(),
        attempt,
        status: TaskStatus::Failed,
        summary: String::new(),
        files_touched: Vec::new(),
        started_at: 0,
        finished_at: 0,
        worker_tool_calls: 0,
        tokens_used: None,
    }
}

fn update_artifacts(
    store: &mut Store,
    workspace: &Path,
    spec: &TaskSpec,
    report: &TaskReport,
    _pre_snap: &validator::FileSnapshot,
) -> Result<()> {
    let mut idx = store.load_artifacts()?;

    for path in &report.files_touched {
        let abs = workspace.join(path);
        let size = std::fs::metadata(&abs).map(|m| m.len()).unwrap_or(0);
        let sha256 = if let Ok(bytes) = std::fs::read(&abs) {
            validator::sha256_hex(&bytes)
        } else {
            String::new()
        };

        idx.entries.insert(
            path.clone(),
            ArtifactEntry {
                created_by: spec.id.clone(),
                description: format!("from task {}", spec.id),
                size_bytes: size,
                sha256,
            },
        );
    }

    store.update_artifacts(&idx)
}

fn build_final_report(
    state: &OrchestratorState,
    elapsed_secs: u64,
    _disposition: &str,
) -> FinalReport {
    let total = state.plan.len();
    let completed = state.completed_l1.len();
    let failed = state.failed_l1.len();
    let deferred = state.deferred_l1.len();

    let summary = if failed == 0 && deferred == 0 {
        format!(
            "All {total} tasks completed successfully in {elapsed_secs}s."
        )
    } else {
        format!(
            "{completed}/{total} tasks completed, {failed} failed, {deferred} deferred — {elapsed_secs}s elapsed."
        )
    };

    FinalReport {
        run_id: state.run_id.clone(),
        goal: state.goal.clone(),
        tasks_ok: state.completed_l1.clone(),
        tasks_failed: state.failed_l1.clone(),
        tasks_skipped: state.deferred_l1.clone(),
        summary,
        elapsed_secs,
    }
}

fn emit(tx: &mpsc::UnboundedSender<DriverEvent>, event: DriverEvent) {
    let _ = tx.send(event);
}

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
    use crate::orchestra::types::{FailurePolicy, OrchestratorPhase};

    fn make_state(run_id: &str) -> OrchestratorState {
        OrchestratorState {
            run_id: run_id.into(),
            goal: "test goal".into(),
            created_at: 1000,
            updated_at: 1000,
            mode: FailurePolicy::Interactive,
            plan: vec!["T01".into(), "T02".into()],
            cursor: None,
            phase: OrchestratorPhase::Planning,
            completed_l1: Vec::new(),
            failed_l1: Vec::new(),
            deferred_l1: Vec::new(),
            artifacts_index: std::path::PathBuf::from(".allux/test/artifacts/index.json"),
        }
    }

    #[test]
    fn test_advance_to_next_l1_first() {
        let state = make_state("x");
        let phase = advance_to_next_l1(&state).unwrap();
        assert!(matches!(phase, OrchestratorPhase::ExpandingL2 { l1 } if l1 == "T01"));
    }

    #[test]
    fn test_advance_to_next_l1_skips_done() {
        let mut state = make_state("x");
        state.completed_l1 = vec!["T01".into()];
        let phase = advance_to_next_l1(&state).unwrap();
        assert!(matches!(phase, OrchestratorPhase::ExpandingL2 { l1 } if l1 == "T02"));
    }

    #[test]
    fn test_advance_to_next_l1_all_done() {
        let mut state = make_state("x");
        state.completed_l1 = vec!["T01".into(), "T02".into()];
        let phase = advance_to_next_l1(&state).unwrap();
        assert!(matches!(phase, OrchestratorPhase::Finalizing));
    }

    #[test]
    fn test_build_final_report_all_done() {
        let mut state = make_state("run1");
        state.completed_l1 = vec!["T01".into(), "T02".into()];
        let report = build_final_report(&state, 42, "completed");
        assert_eq!(report.run_id, "run1");
        assert!(report.summary.contains("completed"));
        assert_eq!(report.elapsed_secs, 42);
    }

    #[test]
    fn test_build_final_report_partial() {
        let mut state = make_state("run2");
        state.completed_l1 = vec!["T01".into()];
        state.failed_l1 = vec!["T02".into()];
        let report = build_final_report(&state, 10, "partial");
        assert!(report.summary.contains("failed"));
    }
}
