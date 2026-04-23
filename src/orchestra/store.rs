use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::orchestra::types::{
    ArtifactIndex, Diagnosis, OrchestratorState, TaskId, TaskReport, TaskSpec,
    ValidationReport, Verdict,
};

// ── Run status enum ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    Done,
    Failed,
    Abandoned,
}

impl RunStatus {
    fn from_phase(phase: &str) -> Self {
        match phase {
            "Done" => Self::Done,
            "Finalizing" => Self::Running,
            p if p.contains("Failed") => Self::Failed,
            _ => Self::Running,
        }
    }
}

// ── RunSummary ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RunSummary {
    pub run_id: String,
    pub goal: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub status: RunStatus,
    pub completed: usize,
    pub failed: usize,
    pub deferred: usize,
}

// ── Store ─────────────────────────────────────────────────────────────────────

pub struct Store {
    root: PathBuf,
}

impl Store {
    // ── Constructors ──────────────────────────────────────────────────────────

    /// Create a brand-new run directory and acquire the workspace lock.
    pub fn create(workspace: &Path, goal: &str) -> Result<Self> {
        let run_id = current_unix_secs().to_string();
        let runs_dir = workspace.join(".allux").join("runs");
        fs::create_dir_all(&runs_dir)
            .with_context(|| format!("create runs dir: {}", runs_dir.display()))?;

        acquire_lock(&runs_dir)?;

        let root = runs_dir.join(&run_id);
        fs::create_dir_all(&root)?;
        fs::create_dir_all(root.join("tasks"))?;
        fs::create_dir_all(root.join("artifacts"))?;

        ensure_gitignore(workspace)?;

        let store = Store { root };
        // Write initial state stub so list_runs can read it immediately
        let now = current_unix_secs();
        let initial = OrchestratorState {
            run_id: run_id.clone(),
            goal: goal.to_string(),
            created_at: now,
            updated_at: now,
            mode: crate::orchestra::types::FailurePolicy::Interactive,
            plan: Vec::new(),
            cursor: None,
            phase: crate::orchestra::types::OrchestratorPhase::Planning,
            completed_l1: Vec::new(),
            failed_l1: Vec::new(),
            deferred_l1: Vec::new(),
            artifacts_index: store.root.join("artifacts").join("index.json"),
        };
        store.persist_state(&initial)?;
        Ok(store)
    }

    /// Open an existing run directory for resumption.
    pub fn open(workspace: &Path, run_id: &str) -> Result<Self> {
        let runs_dir = workspace.join(".allux").join("runs");
        acquire_lock(&runs_dir)?;
        let root = runs_dir.join(run_id);
        if !root.exists() {
            anyhow::bail!("run {} not found at {}", run_id, root.display());
        }
        Ok(Store { root })
    }

    /// The run_id is the directory name.
    pub fn run_id(&self) -> &str {
        self.root.file_name().and_then(|n| n.to_str()).unwrap_or("")
    }

    // ── Distilled layer ───────────────────────────────────────────────────────

    pub fn persist_state(&self, s: &OrchestratorState) -> Result<()> {
        atomic_write_json(&self.root.join("state.json"), s)
    }

    pub fn load_state(&self) -> Result<OrchestratorState> {
        load_json(&self.root.join("state.json"))
    }

    pub fn write_plan(&self, plan: &[TaskSpec]) -> Result<()> {
        let v: Vec<&TaskSpec> = plan.iter().collect();
        atomic_write_json(&self.root.join("plan.json"), &v)
    }

    pub fn load_plan(&self) -> Result<Vec<TaskSpec>> {
        load_json(&self.root.join("plan.json"))
    }

    pub fn write_task_spec(&self, spec: &TaskSpec) -> Result<()> {
        let dir = self.task_dir(&spec.id);
        fs::create_dir_all(&dir)?;
        atomic_write_json(&dir.join("spec.json"), spec)
    }

    pub fn load_task_spec(&self, id: &TaskId) -> Result<TaskSpec> {
        load_json(&self.task_dir(id).join("spec.json"))
    }

    pub fn write_subtasks(&self, parent: &TaskId, subs: &[TaskSpec]) -> Result<()> {
        let dir = self.task_dir(parent);
        fs::create_dir_all(&dir)?;
        let v: Vec<&TaskSpec> = subs.iter().collect();
        atomic_write_json(&dir.join("subtasks.json"), &v)
    }

    pub fn load_subtasks(&self, parent: &TaskId) -> Result<Vec<TaskSpec>> {
        load_json(&self.task_dir(parent).join("subtasks.json"))
    }

    pub fn write_report(&self, id: &TaskId, attempt: u32, r: &TaskReport) -> Result<()> {
        let dir = self.attempt_dir(id, attempt);
        fs::create_dir_all(&dir)?;
        atomic_write_json(&dir.join("report.json"), r)
    }

    pub fn write_validation(&self, id: &TaskId, attempt: u32, v: &ValidationReport) -> Result<()> {
        let dir = self.attempt_dir(id, attempt);
        fs::create_dir_all(&dir)?;
        atomic_write_json(&dir.join("validation.json"), v)
    }

    pub fn write_diagnosis(&self, id: &TaskId, attempt: u32, d: &Diagnosis) -> Result<()> {
        let dir = self.attempt_dir(id, attempt);
        fs::create_dir_all(&dir)?;
        atomic_write_json(&dir.join("diagnosis.json"), d)
    }

    pub fn write_diff(&self, id: &TaskId, attempt: u32, diff: &str) -> Result<()> {
        let dir = self.attempt_dir(id, attempt);
        fs::create_dir_all(&dir)?;
        atomic_write_str(&dir.join("diff.patch"), diff)
    }

    /// Write latest pointer: `{ "attempt": N, "verdict": "..." }`.
    pub fn write_latest(&self, id: &TaskId, attempt: u32, verdict: Verdict) -> Result<()> {
        #[derive(Serialize)]
        struct Latest { attempt: u32, verdict: String }
        let verdict_str = match verdict {
            Verdict::Ok        => "ok",
            Verdict::Failed    => "failed",
            Verdict::Uncertain => "uncertain",
        };
        let dir = self.task_dir(id);
        fs::create_dir_all(&dir)?;
        atomic_write_json(&dir.join("latest.json"), &Latest {
            attempt,
            verdict: verdict_str.into(),
        })
    }

    pub fn update_artifacts(&self, idx: &ArtifactIndex) -> Result<()> {
        atomic_write_json(&self.root.join("artifacts").join("index.json"), idx)
    }

    pub fn load_artifacts(&self) -> Result<ArtifactIndex> {
        let p = self.root.join("artifacts").join("index.json");
        if p.exists() {
            load_json(&p)
        } else {
            Ok(ArtifactIndex::default())
        }
    }

    // ── Raw events layer ──────────────────────────────────────────────────────

    /// Append an event as a JSON line to `events.log`.
    pub fn append_event(&self, ev: &impl Serialize) -> Result<()> {
        let log_path = self.root.join("events.log");
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("open events.log: {}", log_path.display()))?;

        let line = serde_json::to_string(ev)?;
        writeln!(file, "{line}")?;

        // Soft cap: warn if > 50 MB
        if let Ok(meta) = file.metadata() {
            if meta.len() > 50 * 1024 * 1024 {
                eprintln!("WARNING: events.log exceeds 50 MB for run {}", self.run_id());
            }
        }
        Ok(())
    }

    /// Compress events.log to events.log.zst and remove the original.
    /// Called after a run completes.
    pub fn finalize(&self) -> Result<()> {
        let log_path = self.root.join("events.log");
        let zst_path = self.root.join("events.log.zst");

        if log_path.exists() {
            let data = fs::read(&log_path)?;
            let compressed = zstd::encode_all(data.as_slice(), 3)?;
            atomic_write_bytes(&zst_path, &compressed)?;
            fs::remove_file(&log_path)?;
        }

        // Remove lock file on finalize (best-effort)
        let lock_path = self.root.parent()
            .map(|p| p.join(".lock"))
            .unwrap_or_else(|| PathBuf::from(".lock"));
        let _ = fs::remove_file(&lock_path);

        Ok(())
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn task_dir(&self, id: &TaskId) -> PathBuf {
        // L2 tasks like "T01.02" → stored under tasks/T01/T01.02/
        if let Some(dot) = id.find('.') {
            let parent = &id[..dot];
            self.root.join("tasks").join(parent).join(id)
        } else {
            self.root.join("tasks").join(id)
        }
    }

    fn attempt_dir(&self, id: &TaskId, attempt: u32) -> PathBuf {
        self.task_dir(id).join("attempts").join(format!("{attempt:02}"))
    }
}

// ── list_runs ─────────────────────────────────────────────────────────────────

/// List all saved Orchestra runs for a workspace, most recent first.
pub fn list_runs(workspace: &Path) -> Result<Vec<RunSummary>> {
    let runs_dir = workspace.join(".allux").join("runs");
    if !runs_dir.exists() {
        return Ok(Vec::new());
    }

    let mut summaries: Vec<RunSummary> = Vec::new();
    for entry in fs::read_dir(&runs_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') {
            continue; // skip .lock, .archive, etc.
        }

        let state_path = path.join("state.json");
        if !state_path.exists() {
            continue;
        }

        let Ok(state): std::result::Result<OrchestratorState, _> = load_json(&state_path) else {
            continue;
        };

        let phase_label = format!("{:?}", state.phase);
        let status = RunStatus::from_phase(&phase_label);

        summaries.push(RunSummary {
            run_id: state.run_id,
            goal: state.goal.chars().take(80).collect(),
            created_at: state.created_at,
            updated_at: state.updated_at,
            status,
            completed: state.completed_l1.len(),
            failed: state.failed_l1.len(),
            deferred: state.deferred_l1.len(),
        });
    }

    summaries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(summaries)
}

// ── Atomic write helpers ──────────────────────────────────────────────────────

fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let json = serde_json::to_string_pretty(value)?;
    atomic_write_str(path, &json)
}

fn atomic_write_str(path: &Path, content: &str) -> Result<()> {
    atomic_write_bytes(path, content.as_bytes())
}

fn atomic_write_bytes(path: &Path, content: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or("dat")
    ));
    {
        let file = fs::File::create(&tmp)
            .with_context(|| format!("create tmp file: {}", tmp.display()))?;
        let mut w = BufWriter::new(file);
        w.write_all(content)?;
        w.flush()?;
    }
    fs::rename(&tmp, path)
        .with_context(|| format!("rename {} → {}", tmp.display(), path.display()))?;
    Ok(())
}

fn load_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("parse JSON from {}", path.display()))
}

// ── Lock file ─────────────────────────────────────────────────────────────────

fn acquire_lock(runs_dir: &Path) -> Result<()> {
    fs::create_dir_all(runs_dir)?;
    let lock_path = runs_dir.join(".lock");

    if lock_path.exists() {
        // Check if the PID in the lock file is still alive.
        if let Ok(content) = fs::read_to_string(&lock_path) {
            let pid_str = content.trim();
            if let Ok(pid) = pid_str.parse::<u32>() {
                if pid_is_alive(pid) {
                    anyhow::bail!(
                        "Another allux orchestra instance (PID {pid}) is using this workspace. \
                         Remove {} if stale.",
                        lock_path.display()
                    );
                }
            }
        }
        // Stale lock — remove it.
        let _ = fs::remove_file(&lock_path);
    }

    let my_pid = std::process::id();
    fs::write(&lock_path, my_pid.to_string())?;
    Ok(())
}

fn pid_is_alive(pid: u32) -> bool {
    // POSIX: kill(pid, 0) returns 0 if the process exists.
    #[cfg(unix)]
    {
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        result == 0
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

// ── .gitignore append ─────────────────────────────────────────────────────────

fn ensure_gitignore(workspace: &Path) -> Result<()> {
    let gi = workspace.join(".gitignore");
    const ALLUX_LINE: &str = ".allux/";
    let content = fs::read_to_string(&gi).unwrap_or_default();
    if !content.lines().any(|l| l.trim() == ALLUX_LINE.trim_end_matches('/')) &&
       !content.lines().any(|l| l.trim() == ALLUX_LINE) {
        let mut f = fs::OpenOptions::new().create(true).append(true).open(&gi)?;
        if !content.is_empty() && !content.ends_with('\n') {
            writeln!(f)?;
        }
        writeln!(f, "{ALLUX_LINE}")?;
    }
    Ok(())
}

// ── Misc helpers ──────────────────────────────────────────────────────────────

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
    use crate::orchestra::types::{
        CheckOutcome, ExpectedFile, FileChange, OrchestratorPhase, TaskStatus, Verdict,
    };
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn tmp() -> TempDir {
        TempDir::new().unwrap()
    }

    fn make_state(run_id: &str, goal: &str) -> OrchestratorState {
        OrchestratorState {
            run_id: run_id.into(),
            goal: goal.into(),
            created_at: 1000,
            updated_at: 2000,
            mode: crate::orchestra::types::FailurePolicy::Interactive,
            plan: Vec::new(),
            cursor: None,
            phase: OrchestratorPhase::Planning,
            completed_l1: Vec::new(),
            failed_l1: Vec::new(),
            deferred_l1: Vec::new(),
            artifacts_index: PathBuf::from(".allux/runs/test/artifacts/index.json"),
        }
    }

    #[test]
    fn test_create_and_persist_state() {
        let dir = tmp();
        let store = Store::create(dir.path(), "Build a landing page").unwrap();
        let state = store.load_state().unwrap();
        assert_eq!(state.goal, "Build a landing page");
    }

    #[test]
    fn test_open_existing_run() {
        let dir = tmp();
        let store = Store::create(dir.path(), "Test goal").unwrap();
        let run_id = store.run_id().to_string();

        // Drop original store (releases any in-memory state)
        drop(store);

        // Remove lock to allow re-open in same process
        let lock_path = dir.path().join(".allux").join("runs").join(".lock");
        let _ = fs::remove_file(&lock_path);

        let store2 = Store::open(dir.path(), &run_id).unwrap();
        let state = store2.load_state().unwrap();
        assert_eq!(state.run_id, run_id);
    }

    #[test]
    fn test_persist_and_reload_state() {
        let dir = tmp();
        let store = Store::create(dir.path(), "goal").unwrap();
        let run_id = store.run_id().to_string();

        let mut state = make_state(&run_id, "goal");
        state.completed_l1 = vec!["T01".into()];
        store.persist_state(&state).unwrap();

        let loaded = store.load_state().unwrap();
        assert_eq!(loaded.completed_l1, vec!["T01"]);
    }

    #[test]
    fn test_write_and_load_plan() {
        let dir = tmp();
        let store = Store::create(dir.path(), "goal").unwrap();

        let plan = vec![
            TaskSpec { id: "T01".into(), title: "First task".into(), ..Default::default() },
            TaskSpec { id: "T02".into(), title: "Second task".into(), ..Default::default() },
        ];
        store.write_plan(&plan).unwrap();

        let loaded = store.load_plan().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, "T01");
    }

    #[test]
    fn test_write_and_load_task_spec() {
        let dir = tmp();
        let store = Store::create(dir.path(), "goal").unwrap();

        let spec = TaskSpec {
            id: "T01".into(),
            title: "Build index.html".into(),
            expected_files: vec![ExpectedFile {
                path: PathBuf::from("index.html"),
                change: FileChange::Create,
                min_bytes: None,
                max_bytes: None,
            }],
            ..Default::default()
        };
        store.write_task_spec(&spec).unwrap();

        let loaded = store.load_task_spec(&"T01".into()).unwrap();
        assert_eq!(loaded.title, "Build index.html");
        assert_eq!(loaded.expected_files.len(), 1);
    }

    #[test]
    fn test_write_subtasks() {
        let dir = tmp();
        let store = Store::create(dir.path(), "goal").unwrap();

        let parent = TaskSpec { id: "T01".into(), ..Default::default() };
        store.write_task_spec(&parent).unwrap();

        let subs = vec![
            TaskSpec { id: "T01.01".into(), parent: Some("T01".into()), ..Default::default() },
            TaskSpec { id: "T01.02".into(), parent: Some("T01".into()), ..Default::default() },
        ];
        store.write_subtasks(&"T01".into(), &subs).unwrap();

        let loaded = store.load_subtasks(&"T01".into()).unwrap();
        assert_eq!(loaded.len(), 2);
    }

    #[test]
    fn test_write_attempt_files() {
        let dir = tmp();
        let store = Store::create(dir.path(), "goal").unwrap();
        let run_id = store.run_id().to_string();

        let report = TaskReport {
            task_id: "T01.01".into(),
            attempt: 1,
            status: TaskStatus::Ok,
            summary: "Created the file".into(),
            files_touched: vec![PathBuf::from("index.html")],
            started_at: 1000,
            finished_at: 2000,
            worker_tool_calls: 3,
            tokens_used: Some(512),
        };
        store.write_report(&"T01.01".into(), 1, &report).unwrap();

        let validation = crate::orchestra::types::ValidationReport {
            task_id: "T01.01".into(),
            outcomes: vec![("FileExists".into(), CheckOutcome::Pass)],
            verdict: Verdict::Ok,
            score: 1.0,
        };
        store.write_validation(&"T01.01".into(), 1, &validation).unwrap();
        store.write_diff(&"T01.01".into(), 1, "--- a/x\n+++ b/x\n").unwrap();
        store.write_latest(&"T01.01".into(), 1, Verdict::Ok).unwrap();

        // Verify files exist
        let attempt_dir = store.attempt_dir(&"T01.01".into(), 1);
        assert!(attempt_dir.join("report.json").exists());
        assert!(attempt_dir.join("validation.json").exists());
        assert!(attempt_dir.join("diff.patch").exists());
        assert!(store.task_dir(&"T01.01".into()).join("latest.json").exists());
    }

    #[test]
    fn test_artifacts() {
        let dir = tmp();
        let store = Store::create(dir.path(), "goal").unwrap();

        let idx = store.load_artifacts().unwrap(); // defaults to empty
        assert!(idx.entries.is_empty());

        let mut idx2 = ArtifactIndex::default();
        idx2.entries.insert(
            PathBuf::from("index.html"),
            crate::orchestra::types::ArtifactEntry {
                created_by: "T01".into(),
                description: "Landing page".into(),
                size_bytes: 1024,
                sha256: "abc123".into(),
            },
        );
        store.update_artifacts(&idx2).unwrap();

        let loaded = store.load_artifacts().unwrap();
        assert_eq!(loaded.entries.len(), 1);
    }

    #[test]
    fn test_append_event() {
        let dir = tmp();
        let store = Store::create(dir.path(), "goal").unwrap();

        #[derive(Serialize)]
        struct TestEvent { kind: &'static str }
        store.append_event(&TestEvent { kind: "test" }).unwrap();
        store.append_event(&TestEvent { kind: "test2" }).unwrap();

        let log_path = store.root.join("events.log");
        let content = fs::read_to_string(&log_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_finalize_compresses_log() {
        let dir = tmp();
        let store = Store::create(dir.path(), "goal").unwrap();

        #[derive(Serialize)]
        struct Ev { msg: &'static str }
        store.append_event(&Ev { msg: "hello" }).unwrap();

        let log_path = store.root.join("events.log");
        let zst_path = store.root.join("events.log.zst");
        assert!(log_path.exists());
        assert!(!zst_path.exists());

        store.finalize().unwrap();
        assert!(!log_path.exists());
        assert!(zst_path.exists());
    }

    #[test]
    fn test_list_runs_empty_workspace() {
        let dir = tmp();
        let runs = list_runs(dir.path()).unwrap();
        assert!(runs.is_empty());
    }

    #[test]
    fn test_list_runs_returns_created_runs() {
        let dir = tmp();
        let _s1 = Store::create(dir.path(), "First goal").unwrap();

        // Remove lock to allow second create
        let lock_path = dir.path().join(".allux").join("runs").join(".lock");
        let _ = fs::remove_file(&lock_path);
        std::thread::sleep(std::time::Duration::from_secs(1)); // ensure different timestamp
        let _s2 = Store::create(dir.path(), "Second goal").unwrap();

        let runs = list_runs(dir.path()).unwrap();
        assert_eq!(runs.len(), 2);
        // Most recent first
        assert_eq!(runs[0].goal, "Second goal");
    }

    #[test]
    fn test_atomic_write_crash_safe() {
        let dir = tmp();
        let path = dir.path().join("test.json");

        // Write initial content
        atomic_write_json(&path, &"initial").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), r#""initial""#);

        // Write new content — old file should remain intact until rename
        atomic_write_json(&path, &"updated").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), r#""updated""#);
        // No .tmp file left behind
        assert!(!dir.path().join("test.tmp").exists());
    }

    #[test]
    fn test_gitignore_appended() {
        let dir = tmp();
        let _ = Store::create(dir.path(), "goal").unwrap();
        let gi = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(gi.contains(".allux/"));
    }

    #[test]
    fn test_gitignore_not_duplicated() {
        let dir = tmp();
        fs::write(dir.path().join(".gitignore"), ".allux/\n").unwrap();
        let _ = Store::create(dir.path(), "goal").unwrap();

        // Remove lock to allow second create
        let lock_path = dir.path().join(".allux").join("runs").join(".lock");
        let _ = fs::remove_file(&lock_path);
        std::thread::sleep(std::time::Duration::from_secs(1));
        let _ = Store::create(dir.path(), "goal2").unwrap();

        let gi = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert_eq!(gi.matches(".allux/").count(), 1);
    }
}
