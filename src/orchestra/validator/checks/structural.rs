use std::path::Path;

use crate::orchestra::types::CheckOutcome;
use crate::orchestra::validator::{sha256_hex, FileState};

/// Check that a file exists on disk.
pub fn file_exists(abs_path: &Path) -> CheckOutcome {
    if abs_path.exists() {
        CheckOutcome::Pass
    } else {
        CheckOutcome::Fail {
            reason: format!("file does not exist: {}", abs_path.display()),
        }
    }
}

/// Check that a file's size is within [min, max] bytes.
pub fn file_size_in_range(abs_path: &Path, min: u64, max: u64) -> CheckOutcome {
    match std::fs::metadata(abs_path) {
        Ok(meta) => {
            let size = meta.len();
            if size < min {
                CheckOutcome::Fail {
                    reason: format!(
                        "{} bytes < minimum {min} bytes",
                        size
                    ),
                }
            } else if size > max {
                CheckOutcome::Fail {
                    reason: format!("{} bytes > maximum {max} bytes", size),
                }
            } else {
                CheckOutcome::Pass
            }
        }
        Err(_) => CheckOutcome::Fail {
            reason: format!("cannot stat file: {}", abs_path.display()),
        },
    }
}

/// Check that a file was actually modified relative to a pre-worker snapshot.
/// Compares SHA-256 (preferred) or size+mtime.
pub fn diff_has_changes(
    abs_path: &Path,
    rel_path: &Path,
    pre: Option<&FileState>,
) -> CheckOutcome {
    if !abs_path.exists() {
        return CheckOutcome::Fail {
            reason: format!("file does not exist: {}", rel_path.display()),
        };
    }

    let Some(pre_state) = pre else {
        // No pre-snapshot → assume changed (worker created it from nothing).
        return CheckOutcome::Pass;
    };

    if !pre_state.exists {
        // File didn't exist before → new file counts as changed.
        return CheckOutcome::Pass;
    }

    // Compare by SHA-256 when available.
    if let Some(ref pre_hash) = pre_state.sha256 {
        if let Ok(bytes) = std::fs::read(abs_path) {
            if bytes.len() <= 2 * 1024 * 1024 {
                let post_hash = sha256_hex(&bytes);
                return if post_hash != *pre_hash {
                    CheckOutcome::Pass
                } else {
                    CheckOutcome::Fail {
                        reason: "file content unchanged since pre-snapshot".into(),
                    }
                };
            }
        }
    }

    // Fall back to size + mtime comparison.
    if let Ok(meta) = std::fs::metadata(abs_path) {
        let post_size = meta.len();
        let post_mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);

        if post_size != pre_state.size || post_mtime != pre_state.mtime {
            CheckOutcome::Pass
        } else {
            CheckOutcome::Fail {
                reason: "file size and mtime unchanged since pre-snapshot".into(),
            }
        }
    } else {
        CheckOutcome::Fail {
            reason: "cannot stat file after worker".into(),
        }
    }
}

/// Soft check: ratio of bytes added vs removed compared to the pre-snapshot.
/// Returns Soft(ratio) where 1.0 = everything is an addition, 0.0 = pure deletion.
pub fn diff_is_addition(abs_path: &Path, pre: Option<&FileState>) -> CheckOutcome {
    let post_size = std::fs::metadata(abs_path).map(|m| m.len()).unwrap_or(0);

    let pre_size = pre.map(|s| s.size).unwrap_or(0);

    if pre_size == 0 && post_size == 0 {
        return CheckOutcome::Soft(1.0);
    }

    let ratio = if post_size >= pre_size {
        1.0f32
    } else {
        post_size as f32 / pre_size.max(1) as f32
    };

    CheckOutcome::Soft(ratio.clamp(0.0, 1.0))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn tmp() -> TempDir {
        TempDir::new().unwrap()
    }

    #[test]
    fn test_file_exists_pass() {
        let dir = tmp();
        let p = dir.path().join("a.txt");
        fs::write(&p, "x").unwrap();
        assert_eq!(file_exists(&p), CheckOutcome::Pass);
    }

    #[test]
    fn test_file_exists_fail() {
        let dir = tmp();
        let p = dir.path().join("missing.txt");
        assert!(matches!(file_exists(&p), CheckOutcome::Fail { .. }));
    }

    #[test]
    fn test_file_size_in_range_pass() {
        let dir = tmp();
        let p = dir.path().join("f.txt");
        fs::write(&p, "hello world").unwrap(); // 11 bytes
        assert_eq!(file_size_in_range(&p, 5, 20), CheckOutcome::Pass);
    }

    #[test]
    fn test_file_size_too_small() {
        let dir = tmp();
        let p = dir.path().join("f.txt");
        fs::write(&p, "hi").unwrap(); // 2 bytes
        assert!(matches!(file_size_in_range(&p, 10, 100), CheckOutcome::Fail { .. }));
    }

    #[test]
    fn test_file_size_too_large() {
        let dir = tmp();
        let p = dir.path().join("f.txt");
        fs::write(&p, "hello world").unwrap(); // 11 bytes
        assert!(matches!(file_size_in_range(&p, 1, 5), CheckOutcome::Fail { .. }));
    }

    #[test]
    fn test_diff_has_changes_new_file() {
        let dir = tmp();
        let p = dir.path().join("new.txt");
        fs::write(&p, "new content").unwrap();
        // pre_state: file didn't exist
        let pre = FileState::default();
        assert_eq!(
            diff_has_changes(&p, &std::path::PathBuf::from("new.txt"), Some(&pre)),
            CheckOutcome::Pass
        );
    }

    #[test]
    fn test_diff_has_changes_unchanged() {
        let dir = tmp();
        let p = dir.path().join("same.txt");
        let content = "unchanged content";
        fs::write(&p, content).unwrap();

        let pre = FileState {
            exists: true,
            size: content.len() as u64,
            mtime: 0,
            sha256: Some(sha256_hex(content.as_bytes())),
        };
        assert!(matches!(
            diff_has_changes(&p, &std::path::PathBuf::from("same.txt"), Some(&pre)),
            CheckOutcome::Fail { .. }
        ));
    }

    #[test]
    fn test_diff_has_changes_modified() {
        let dir = tmp();
        let p = dir.path().join("mod.txt");
        let old_content = "old";
        fs::write(&p, "new content that is different").unwrap();

        let pre = FileState {
            exists: true,
            size: old_content.len() as u64,
            mtime: 0,
            sha256: Some(sha256_hex(old_content.as_bytes())),
        };
        assert_eq!(
            diff_has_changes(&p, &std::path::PathBuf::from("mod.txt"), Some(&pre)),
            CheckOutcome::Pass
        );
    }

    #[test]
    fn test_diff_is_addition_pure_addition() {
        let dir = tmp();
        let p = dir.path().join("f.txt");
        fs::write(&p, "x".repeat(100)).unwrap();
        // pre was empty
        let pre = FileState { exists: false, size: 0, ..Default::default() };
        assert_eq!(diff_is_addition(&p, Some(&pre)), CheckOutcome::Soft(1.0));
    }

    #[test]
    fn test_diff_is_addition_partial_shrink() {
        let dir = tmp();
        let p = dir.path().join("f.txt");
        fs::write(&p, "x".repeat(50)).unwrap(); // 50 bytes
        let pre = FileState { exists: true, size: 100, ..Default::default() };
        // ratio = 50/100 = 0.5
        match diff_is_addition(&p, Some(&pre)) {
            CheckOutcome::Soft(s) => assert!((s - 0.5).abs() < 0.01),
            other => panic!("expected Soft, got {:?}", other),
        }
    }
}
