use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ── Identifiers ───────────────────────────────────────────────────────────────

pub type TaskId = String;

// ── Top-level state ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorState {
    pub run_id: String,
    pub goal: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub mode: FailurePolicy,
    pub plan: Vec<TaskId>,
    pub cursor: Option<TaskId>,
    pub phase: OrchestratorPhase,
    pub completed_l1: Vec<TaskId>,
    pub failed_l1: Vec<TaskId>,
    pub deferred_l1: Vec<TaskId>,
    pub artifacts_index: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum OrchestratorPhase {
    Planning,
    ExpandingL2 { l1: TaskId },
    ExecutingTask { l1: TaskId, l2: TaskId },
    Validating { l1: TaskId, l2: TaskId },
    Diagnosing { l1: TaskId, l2: TaskId, attempt: u32 },
    Finalizing,
    Done,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FailurePolicy {
    Interactive,
    Autonomous,
}

impl FailurePolicy {
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "interactive" => Some(Self::Interactive),
            "autonomous"  => Some(Self::Autonomous),
            _ => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Interactive => "interactive",
            Self::Autonomous  => "autonomous",
        }
    }
}

// ── TaskSpec ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSpec {
    pub id: TaskId,
    pub parent: Option<TaskId>,
    pub title: String,
    pub description: String,
    pub deps: Vec<TaskId>,
    pub expected_files: Vec<ExpectedFile>,
    pub expected_keywords: Vec<String>,
    pub extra_commands: Vec<String>,
    pub skip_checks: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub max_rounds: u32,
}

impl Default for TaskSpec {
    fn default() -> Self {
        Self {
            id: String::new(),
            parent: None,
            title: String::new(),
            description: String::new(),
            deps: Vec::new(),
            expected_files: Vec::new(),
            expected_keywords: Vec::new(),
            extra_commands: Vec::new(),
            skip_checks: Vec::new(),
            allowed_tools: Vec::new(),
            max_rounds: 4,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectedFile {
    pub path: PathBuf,
    pub change: FileChange,
    pub min_bytes: Option<u64>,
    pub max_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FileChange {
    Create,
    Modify,
    Delete,
}

impl FileChange {
    pub fn from_marker(m: &str) -> Option<Self> {
        match m {
            "+" => Some(Self::Create),
            "~" => Some(Self::Modify),
            "-" => Some(Self::Delete),
            _ => None,
        }
    }

    pub fn to_marker(self) -> &'static str {
        match self {
            Self::Create => "+",
            Self::Modify => "~",
            Self::Delete => "-",
        }
    }
}

// ── TaskReport ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskReport {
    pub task_id: TaskId,
    pub attempt: u32,
    pub status: TaskStatus,
    pub summary: String,
    pub files_touched: Vec<PathBuf>,
    pub started_at: u64,
    pub finished_at: u64,
    pub worker_tool_calls: u32,
    pub tokens_used: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskStatus {
    Ok,
    Failed,
    Skipped,
    NeedsReview,
}

impl TaskStatus {
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "ok"           => Some(Self::Ok),
            "failed"       => Some(Self::Failed),
            "skipped"      => Some(Self::Skipped),
            "needs_review" => Some(Self::NeedsReview),
            _ => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Ok          => "ok",
            Self::Failed      => "failed",
            Self::Skipped     => "skipped",
            Self::NeedsReview => "needs_review",
        }
    }
}

// ── Check catalog ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Check {
    FileExists            { path: PathBuf },
    FileSizeInRange       { path: PathBuf, min: u64, max: u64 },
    DiffHasChanges        { path: PathBuf },
    SyntaxValid           { path: PathBuf },
    NoPlaceholders        { path: PathBuf, whitelist: Vec<String> },
    NoLoopRepetition      { path: PathBuf, max_ratio: f32 },
    KeywordsPresent       { path: PathBuf, keywords: Vec<String>, min_hit: f32 },
    LanguageMatches       { path: PathBuf, lang: Language },
    NoEmptyCriticalBlocks { path: PathBuf },
    ReferencesResolve     { path: PathBuf },
    CommandExitsZero      { cmd: String, cwd: Option<PathBuf> },
    ManualReview          { note: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum Language {
    En,
    Es,
    Unknown,
}

// ── Validation ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CheckOutcome {
    Pass,
    Fail { reason: String },
    Soft(f32),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationReport {
    pub task_id: TaskId,
    pub outcomes: Vec<(String, CheckOutcome)>,
    pub verdict: Verdict,
    pub score: f32,
}

impl ValidationReport {
    /// Aggregate check outcomes into a final verdict and score.
    pub fn aggregate(task_id: TaskId, outcomes: Vec<(String, CheckOutcome)>) -> Self {
        // Any hard fail → Failed immediately.
        let has_fail = outcomes.iter().any(|(_, o)| matches!(o, CheckOutcome::Fail { .. }));
        if has_fail {
            return Self { task_id, outcomes, verdict: Verdict::Failed, score: 0.0 };
        }

        // Compute mean of soft values (Pass = 1.0).
        let scores: Vec<f32> = outcomes.iter().map(|(_, o)| match o {
            CheckOutcome::Pass    => 1.0,
            CheckOutcome::Soft(s) => *s,
            CheckOutcome::Fail { .. } => 0.0,
        }).collect();

        let score = if scores.is_empty() {
            1.0
        } else {
            scores.iter().sum::<f32>() / scores.len() as f32
        };

        let verdict = if score >= 0.7 {
            Verdict::Ok
        } else if score >= 0.5 {
            Verdict::Uncertain
        } else {
            Verdict::Failed
        };

        Self { task_id, outcomes, verdict, score }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Failed,
    Uncertain,
}

// ── Diagnosis ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnosis {
    pub root_cause: String,
    pub strategy: RetryStrategy,
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RetryStrategy {
    RetryAsIs,
    RetryWithHint,
    ReplanSubtree,
    Skip,
    EscalateToUser,
}

impl RetryStrategy {
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.trim() {
            "RetryAsIs"       => Some(Self::RetryAsIs),
            "RetryWithHint"   => Some(Self::RetryWithHint),
            "ReplanSubtree"   => Some(Self::ReplanSubtree),
            "Skip"            => Some(Self::Skip),
            "EscalateToUser"  => Some(Self::EscalateToUser),
            _ => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::RetryAsIs      => "RetryAsIs",
            Self::RetryWithHint  => "RetryWithHint",
            Self::ReplanSubtree  => "ReplanSubtree",
            Self::Skip           => "Skip",
            Self::EscalateToUser => "EscalateToUser",
        }
    }
}

// ── Artifacts ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArtifactIndex {
    pub entries: BTreeMap<PathBuf, ArtifactEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactEntry {
    pub created_by: TaskId,
    pub description: String,
    pub size_bytes: u64,
    pub sha256: String,
}

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub ts: u64,
    pub task_id: Option<TaskId>,
    pub kind: EventKind,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EventKind {
    PlannerCalled        { role: PlannerRole },
    PlannerResult,
    WorkerStarted,
    ToolCall             { name: String },
    ToolResult           { name: String, bytes: usize },
    WorkerFinished,
    ValidationStarted,
    ValidationFinished   { verdict: Verdict },
    DiagnoserCalled,
    DiagnoserResult,
    RetryApplied         { strategy: RetryStrategy },
    UserEscalation,
    PhaseChanged         { from: String, to: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PlannerRole {
    L1,
    L2,
}

// ── FinalReport ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinalReport {
    pub run_id: String,
    pub goal: String,
    pub tasks_ok: Vec<TaskId>,
    pub tasks_failed: Vec<TaskId>,
    pub tasks_skipped: Vec<TaskId>,
    pub summary: String,
    pub elapsed_secs: u64,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_failure_policy_roundtrip() {
        assert_eq!(FailurePolicy::from_str_loose("interactive"), Some(FailurePolicy::Interactive));
        assert_eq!(FailurePolicy::from_str_loose("autonomous"), Some(FailurePolicy::Autonomous));
        assert_eq!(FailurePolicy::from_str_loose("INTERACTIVE"), Some(FailurePolicy::Interactive));
        assert_eq!(FailurePolicy::from_str_loose("unknown"), None);
    }

    #[test]
    fn test_task_status_roundtrip() {
        assert_eq!(TaskStatus::from_str_loose("ok"), Some(TaskStatus::Ok));
        assert_eq!(TaskStatus::from_str_loose("needs_review"), Some(TaskStatus::NeedsReview));
        assert_eq!(TaskStatus::from_str_loose("unknown"), None);
    }

    #[test]
    fn test_file_change_markers() {
        assert_eq!(FileChange::from_marker("+"), Some(FileChange::Create));
        assert_eq!(FileChange::from_marker("~"), Some(FileChange::Modify));
        assert_eq!(FileChange::from_marker("-"), Some(FileChange::Delete));
        assert_eq!(FileChange::from_marker("?"), None);
        assert_eq!(FileChange::Create.to_marker(), "+");
    }

    #[test]
    fn test_retry_strategy_roundtrip() {
        for s in &[
            RetryStrategy::RetryAsIs,
            RetryStrategy::RetryWithHint,
            RetryStrategy::ReplanSubtree,
            RetryStrategy::Skip,
            RetryStrategy::EscalateToUser,
        ] {
            assert_eq!(RetryStrategy::from_str_loose(s.label()), Some(*s));
        }
    }

    #[test]
    fn test_validation_aggregate_hard_fail() {
        let outcomes = vec![
            ("file_exists".into(), CheckOutcome::Pass),
            ("syntax".into(), CheckOutcome::Fail { reason: "parse error".into() }),
        ];
        let report = ValidationReport::aggregate("T01".into(), outcomes);
        assert_eq!(report.verdict, Verdict::Failed);
    }

    #[test]
    fn test_validation_aggregate_all_pass() {
        let outcomes = vec![
            ("a".into(), CheckOutcome::Pass),
            ("b".into(), CheckOutcome::Pass),
        ];
        let report = ValidationReport::aggregate("T01".into(), outcomes);
        assert_eq!(report.verdict, Verdict::Ok);
        assert!((report.score - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_validation_aggregate_soft_boundaries() {
        // score = 0.49 → Failed
        let outcomes = vec![("a".into(), CheckOutcome::Soft(0.49))];
        let report = ValidationReport::aggregate("T01".into(), outcomes);
        assert_eq!(report.verdict, Verdict::Failed);

        // score = 0.50 → Uncertain
        let outcomes = vec![("a".into(), CheckOutcome::Soft(0.50))];
        let report = ValidationReport::aggregate("T01".into(), outcomes);
        assert_eq!(report.verdict, Verdict::Uncertain);

        // score = 0.69 → Uncertain
        let outcomes = vec![("a".into(), CheckOutcome::Soft(0.69))];
        let report = ValidationReport::aggregate("T01".into(), outcomes);
        assert_eq!(report.verdict, Verdict::Uncertain);

        // score = 0.70 → Ok
        let outcomes = vec![("a".into(), CheckOutcome::Soft(0.70))];
        let report = ValidationReport::aggregate("T01".into(), outcomes);
        assert_eq!(report.verdict, Verdict::Ok);
    }

    #[test]
    fn test_orchestrator_state_serde() {
        let state = OrchestratorState {
            run_id: "1234567890".into(),
            goal: "Build a landing page".into(),
            created_at: 1000,
            updated_at: 2000,
            mode: FailurePolicy::Interactive,
            plan: vec!["T01".into(), "T02".into()],
            cursor: Some("T01".into()),
            phase: OrchestratorPhase::Planning,
            completed_l1: vec![],
            failed_l1: vec![],
            deferred_l1: vec![],
            artifacts_index: PathBuf::from(".allux/runs/1234567890/artifacts/index.json"),
        };
        let json = serde_json::to_string(&state).unwrap();
        let recovered: OrchestratorState = serde_json::from_str(&json).unwrap();
        assert_eq!(recovered.run_id, "1234567890");
        assert_eq!(recovered.plan.len(), 2);
        assert_eq!(recovered.mode, FailurePolicy::Interactive);
    }
}
