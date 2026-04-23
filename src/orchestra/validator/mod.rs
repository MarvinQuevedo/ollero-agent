pub mod auto;
pub mod checks;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::orchestra::types::{
    Check, CheckOutcome, FileChange, TaskSpec, ValidationReport, Verdict,
};

// ── FileSnapshot ──────────────────────────────────────────────────────────────

/// State of a single file captured before the worker starts.
#[derive(Debug, Clone, Default)]
pub struct FileState {
    pub exists: bool,
    pub size: u64,
    pub mtime: u64,
    /// SHA-256 hex; only computed for files ≤ 2 MB.
    pub sha256: Option<String>,
}

/// Snapshot of all files of interest captured before the worker runs.
#[derive(Debug, Clone, Default)]
pub struct FileSnapshot {
    pub files: BTreeMap<PathBuf, FileState>,
}

impl FileSnapshot {
    /// Capture state of a specific set of paths.
    pub fn capture(workspace: &Path, paths: &[PathBuf]) -> Self {
        let mut snap = FileSnapshot::default();
        for rel in paths {
            let abs = workspace.join(rel);
            let state = if abs.exists() {
                let meta = std::fs::metadata(&abs).ok();
                let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                let mtime = meta
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let sha256 = if size <= 2 * 1024 * 1024 {
                    std::fs::read(&abs).ok().map(|b| sha256_hex(&b))
                } else {
                    None
                };
                FileState { exists: true, size, mtime, sha256 }
            } else {
                FileState::default()
            };
            snap.files.insert(rel.clone(), state);
        }
        snap
    }

    pub fn get(&self, path: &Path) -> Option<&FileState> {
        self.files.get(path)
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    // Simple SHA-256 using ring-compatible approach via std — we use a naive
    // implementation to avoid adding a crypto dep just for integrity checks.
    // In practice these are used only for change-detection, not security.
    use std::fmt::Write;
    let digest = sha256_naive(data);
    let mut s = String::with_capacity(64);
    for b in digest {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn sha256_naive(data: &[u8]) -> [u8; 32] {
    // RFC 6234 / FIPS 180-4 SHA-256 — small standalone implementation.
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5,
        0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
        0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
        0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
        0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
        0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
        0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];

    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];

    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg: Vec<u8> = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i*4], chunk[i*4+1], chunk[i*4+2], chunk[i*4+3]]);
        }
        for i in 16..64 {
            let s0 = w[i-15].rotate_right(7) ^ w[i-15].rotate_right(18) ^ (w[i-15] >> 3);
            let s1 = w[i-2].rotate_right(17) ^ w[i-2].rotate_right(19) ^ (w[i-2] >> 10);
            w[i] = w[i-16].wrapping_add(s0).wrapping_add(w[i-7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] =
            [h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]];
        for i in 0..64 {
            let s1  = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch  = (e & f) ^ ((!e) & g);
            let tmp1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0  = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let tmp2 = s0.wrapping_add(maj);
            hh = g; g = f; f = e;
            e = d.wrapping_add(tmp1);
            d = c; c = b; b = a;
            a = tmp1.wrapping_add(tmp2);
        }
        h[0] = h[0].wrapping_add(a); h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c); h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e); h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g); h[7] = h[7].wrapping_add(hh);
    }

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i*4..i*4+4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Run all checks derived from `spec` against the workspace, returning a full report.
pub fn validate(spec: &TaskSpec, workspace: &Path, pre: &FileSnapshot) -> ValidationReport {
    let mut outcomes: Vec<(String, CheckOutcome)> = Vec::new();

    // Build checks from spec's expected_files
    for ef in &spec.expected_files {
        let abs = workspace.join(&ef.path);

        match ef.change {
            FileChange::Create | FileChange::Modify => {
                // FileExists
                let name = format!("FileExists({})", ef.path.display());
                let outcome = checks::structural::file_exists(&abs);
                outcomes.push((name, outcome));

                // FileSizeInRange (use spec bounds or defaults)
                if let Some(min) = ef.min_bytes {
                    let max = ef.max_bytes.unwrap_or(u64::MAX);
                    let name = format!("FileSizeInRange({})", ef.path.display());
                    outcomes.push((name, checks::structural::file_size_in_range(&abs, min, max)));
                } else {
                    let min = default_min_bytes(&ef.path);
                    let name = format!("FileSizeInRange({})", ef.path.display());
                    outcomes.push((name, checks::structural::file_size_in_range(&abs, min, u64::MAX)));
                }

                // DiffHasChanges for Modify
                if ef.change == FileChange::Modify {
                    let name = format!("DiffHasChanges({})", ef.path.display());
                    let pre_state = pre.get(&ef.path);
                    outcomes.push((name, checks::structural::diff_has_changes(&abs, &ef.path, pre_state)));
                }

                // SyntaxValid (where applicable)
                if let Some(ext) = ef.path.extension().and_then(|e| e.to_str()) {
                    if !spec.skip_checks.iter().any(|s| s == "syntax") {
                        let name = format!("SyntaxValid({})", ef.path.display());
                        let outcome = checks::syntax::syntax_valid(&abs, ext);
                        outcomes.push((name, outcome));
                    }
                }

                // Content checks (if file is readable)
                if let Ok(content) = std::fs::read_to_string(&abs) {
                    // Placeholders
                    if !spec.skip_checks.iter().any(|s| s == "no_placeholders") {
                        let name = format!("NoPlaceholders({})", ef.path.display());
                        outcomes.push((name, checks::content::no_placeholders(&content, &[])));
                    }

                    // Keyword presence
                    if !spec.expected_keywords.is_empty() {
                        let name = format!("KeywordsPresent({})", ef.path.display());
                        outcomes.push((name, checks::content::keywords_present(
                            &content,
                            &spec.expected_keywords,
                            0.4,
                        )));
                    }

                    // Loop repetition
                    if !spec.skip_checks.iter().any(|s| s == "no_loop_repetition") {
                        let name = format!("NoLoopRepetition({})", ef.path.display());
                        outcomes.push((name, checks::content::no_loop_repetition(&content, 0.15)));
                    }

                    // Entropy
                    if !spec.skip_checks.iter().any(|s| s == "entropy") {
                        let name = format!("Entropy({})", ef.path.display());
                        outcomes.push((name, checks::content::entropy_reasonable(content.as_bytes())));
                    }

                    // Empty critical blocks
                    if !spec.skip_checks.iter().any(|s| s == "no_empty_critical_blocks") {
                        if let Some(ext) = ef.path.extension().and_then(|e| e.to_str()) {
                            let name = format!("NoEmptyCriticalBlocks({})", ef.path.display());
                            outcomes.push((name, checks::content::no_empty_critical_blocks(&content, ext)));
                        }
                    }
                }
            }
            FileChange::Delete => {
                // File should NOT exist after deletion
                let name = format!("FileDeleted({})", ef.path.display());
                let outcome = if abs.exists() {
                    CheckOutcome::Fail { reason: "file still exists after expected deletion".into() }
                } else {
                    CheckOutcome::Pass
                };
                outcomes.push((name, outcome));
            }
        }
    }

    // Cross-file references check
    for ef in &spec.expected_files {
        if matches!(ef.change, FileChange::Create | FileChange::Modify) {
            let abs = workspace.join(&ef.path);
            if abs.exists() {
                if let Some(ext) = ef.path.extension().and_then(|e| e.to_str()) {
                    if matches!(ext, "html" | "md" | "js" | "ts" | "py" | "rs") {
                        if !spec.skip_checks.iter().any(|s| s == "references") {
                            let name = format!("ReferencesResolve({})", ef.path.display());
                            outcomes.push((name, checks::cross_file::references_resolve(&abs, workspace)));
                        }
                    }
                }
            }
        }
    }

    // Explicit Check entries from the spec
    for check in explicit_checks_from_spec(spec) {
        let (name, outcome) = run_explicit_check(&check, workspace, pre);
        outcomes.push((name, outcome));
    }

    // Auto-detected extra checks
    let auto_checks = auto::detect_extra_checks(spec, workspace);
    for check in auto_checks {
        let (name, outcome) = run_explicit_check(&check, workspace, pre);
        outcomes.push((name, outcome));
    }

    ValidationReport::aggregate(spec.id.clone(), outcomes)
}

/// Convert extra commands in spec to `CommandExitsZero` checks.
fn explicit_checks_from_spec(spec: &TaskSpec) -> Vec<Check> {
    spec.extra_commands
        .iter()
        .map(|cmd| Check::CommandExitsZero { cmd: cmd.clone(), cwd: None })
        .collect()
}

fn run_explicit_check(check: &Check, workspace: &Path, pre: &FileSnapshot) -> (String, CheckOutcome) {
    match check {
        Check::FileExists { path } => {
            let abs = workspace.join(path);
            (format!("FileExists({})", path.display()), checks::structural::file_exists(&abs))
        }
        Check::FileSizeInRange { path, min, max } => {
            let abs = workspace.join(path);
            (format!("FileSizeInRange({})", path.display()),
             checks::structural::file_size_in_range(&abs, *min, *max))
        }
        Check::DiffHasChanges { path } => {
            let abs = workspace.join(path);
            let pre_state = pre.get(path);
            (format!("DiffHasChanges({})", path.display()),
             checks::structural::diff_has_changes(&abs, path, pre_state))
        }
        Check::SyntaxValid { path } => {
            let abs = workspace.join(path);
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            (format!("SyntaxValid({})", path.display()),
             checks::syntax::syntax_valid(&abs, ext))
        }
        Check::NoPlaceholders { path, whitelist } => {
            let abs = workspace.join(path);
            let content = std::fs::read_to_string(&abs).unwrap_or_default();
            (format!("NoPlaceholders({})", path.display()),
             checks::content::no_placeholders(&content, whitelist))
        }
        Check::NoLoopRepetition { path, max_ratio } => {
            let abs = workspace.join(path);
            let content = std::fs::read_to_string(&abs).unwrap_or_default();
            (format!("NoLoopRepetition({})", path.display()),
             checks::content::no_loop_repetition(&content, *max_ratio))
        }
        Check::KeywordsPresent { path, keywords, min_hit } => {
            let abs = workspace.join(path);
            let content = std::fs::read_to_string(&abs).unwrap_or_default();
            (format!("KeywordsPresent({})", path.display()),
             checks::content::keywords_present(&content, keywords, *min_hit))
        }
        Check::LanguageMatches { path, lang } => {
            let abs = workspace.join(path);
            let content = std::fs::read_to_string(&abs).unwrap_or_default();
            (format!("LanguageMatches({})", path.display()),
             checks::content::language_matches(&content, *lang))
        }
        Check::NoEmptyCriticalBlocks { path } => {
            let abs = workspace.join(path);
            let content = std::fs::read_to_string(&abs).unwrap_or_default();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            (format!("NoEmptyCriticalBlocks({})", path.display()),
             checks::content::no_empty_critical_blocks(&content, ext))
        }
        Check::ReferencesResolve { path } => {
            let abs = workspace.join(path);
            (format!("ReferencesResolve({})", path.display()),
             checks::cross_file::references_resolve(&abs, workspace))
        }
        Check::CommandExitsZero { cmd, cwd } => {
            let cwd_path = cwd.as_ref().map(|p| workspace.join(p))
                .unwrap_or_else(|| workspace.to_path_buf());
            (format!("CommandExitsZero({cmd})"),
             checks::execution::command_exits_zero(cmd, &cwd_path, 60))
        }
        Check::ManualReview { note } => {
            (format!("ManualReview"), checks::manual::manual_review(note))
        }
    }
}

fn default_min_bytes(path: &Path) -> u64 {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html")        => 100,
        Some("css")         => 50,
        Some("js") | Some("ts") => 30,
        Some("rs")          => 30,
        Some("py")          => 20,
        Some("md")          => 40,
        Some("json")        => 2,
        Some("toml")        => 10,
        _                   => 1,
    }
}

// ── Human-readable rendering ──────────────────────────────────────────────────

impl ValidationReport {
    /// Render the report as a human-readable summary for TUI and logs.
    pub fn render_human(&self) -> String {
        let verdict_str = match self.verdict {
            Verdict::Ok        => "OK",
            Verdict::Failed    => "FAILED",
            Verdict::Uncertain => "UNCERTAIN",
        };
        let mut lines = vec![format!(
            "Task {} validation: {} (score {:.2})",
            self.task_id, verdict_str, self.score
        )];
        for (name, outcome) in &self.outcomes {
            let prefix = match outcome {
                CheckOutcome::Pass       => "  ✓",
                CheckOutcome::Fail { .. } => "  ✗",
                CheckOutcome::Soft(_)    => "  ⚠",
            };
            let detail = match outcome {
                CheckOutcome::Pass            => String::new(),
                CheckOutcome::Fail { reason } => format!(": {reason}"),
                CheckOutcome::Soft(s)         => format!(": score {s:.2}"),
            };
            lines.push(format!("{prefix} {name}{detail}"));
        }
        lines.join("\n")
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestra::types::{ExpectedFile, FileChange};
    use std::fs;
    use tempfile::TempDir;

    fn tmp() -> TempDir {
        TempDir::new().unwrap()
    }

    #[test]
    fn test_sha256_known_value() {
        // SHA-256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        let digest = sha256_hex(b"");
        assert_eq!(digest, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    #[test]
    fn test_sha256_determinism() {
        // Same input must always produce same output (we only need determinism for change detection)
        let d1 = sha256_hex(b"abc");
        let d2 = sha256_hex(b"abc");
        assert_eq!(d1, d2);
        // Different inputs must produce different hashes
        let d3 = sha256_hex(b"abd");
        assert_ne!(d1, d3);
        // Output is 64 hex chars
        assert_eq!(d1.len(), 64);
    }

    #[test]
    fn test_file_snapshot_capture() {
        let dir = tmp();
        let path = dir.path().join("test.txt");
        fs::write(&path, "hello").unwrap();

        let rel = PathBuf::from("test.txt");
        let snap = FileSnapshot::capture(dir.path(), &[rel.clone()]);

        let state = snap.get(&rel).unwrap();
        assert!(state.exists);
        assert_eq!(state.size, 5);
        assert!(state.sha256.is_some());
    }

    #[test]
    fn test_file_snapshot_missing_file() {
        let dir = tmp();
        let rel = PathBuf::from("nonexistent.txt");
        let snap = FileSnapshot::capture(dir.path(), &[rel.clone()]);
        let state = snap.get(&rel).unwrap();
        assert!(!state.exists);
    }

    #[test]
    fn test_validate_creates_file() {
        let dir = tmp();
        let content = "fn main() { println!(\"hello\"); }";
        fs::write(dir.path().join("main.rs"), content).unwrap();

        let spec = TaskSpec {
            id: "T01".into(),
            expected_files: vec![ExpectedFile {
                path: PathBuf::from("main.rs"),
                change: FileChange::Create,
                min_bytes: None,
                max_bytes: None,
            }],
            skip_checks: vec!["syntax".into(), "no_placeholders".into(),
                               "no_loop_repetition".into(), "entropy".into(),
                               "no_empty_critical_blocks".into()],
            ..Default::default()
        };

        let pre = FileSnapshot::default();
        let report = validate(&spec, dir.path(), &pre);
        assert_eq!(report.task_id, "T01");
        assert!(!report.outcomes.is_empty());
    }

    #[test]
    fn test_validate_missing_file_fails() {
        let dir = tmp();

        let spec = TaskSpec {
            id: "T02".into(),
            expected_files: vec![ExpectedFile {
                path: PathBuf::from("missing.html"),
                change: FileChange::Create,
                min_bytes: None,
                max_bytes: None,
            }],
            ..Default::default()
        };

        let pre = FileSnapshot::default();
        let report = validate(&spec, dir.path(), &pre);
        // FileExists should fail → overall Failed
        assert_eq!(report.verdict, crate::orchestra::types::Verdict::Failed);
    }

    #[test]
    fn test_render_human_format() {
        let dir = tmp();
        fs::write(dir.path().join("index.html"), "<html><body>hello</body></html>").unwrap();

        let spec = TaskSpec {
            id: "T03".into(),
            expected_files: vec![ExpectedFile {
                path: PathBuf::from("index.html"),
                change: FileChange::Create,
                min_bytes: None,
                max_bytes: None,
            }],
            skip_checks: vec![
                "no_placeholders".into(), "no_loop_repetition".into(),
                "entropy".into(), "no_empty_critical_blocks".into(),
            ],
            ..Default::default()
        };

        let pre = FileSnapshot::default();
        let report = validate(&spec, dir.path(), &pre);
        let rendered = report.render_human();
        assert!(rendered.contains("T03"));
        assert!(rendered.contains("validation:"));
    }
}
