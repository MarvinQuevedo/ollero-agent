use std::path::Path;
use std::time::Duration;

use crate::orchestra::types::CheckOutcome;

/// Run a shell command in `cwd` with a timeout; pass iff exit code 0.
pub fn command_exits_zero(cmd: &str, cwd: &Path, timeout_s: u64) -> CheckOutcome {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    let (prog, args) = match parts.split_first() {
        Some(p) => p,
        None => {
            return CheckOutcome::Fail {
                reason: "empty command string".into(),
            }
        }
    };

    let mut child = match std::process::Command::new(prog)
        .args(args)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return CheckOutcome::Fail {
                reason: format!("failed to spawn `{cmd}`: {e}"),
            }
        }
    };

    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_s);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() {
                    CheckOutcome::Pass
                } else {
                    let code = status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into());
                    CheckOutcome::Fail {
                        reason: format!("`{cmd}` exited with status {code}"),
                    }
                };
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    return CheckOutcome::Fail {
                        reason: format!("`{cmd}` timed out after {timeout_s}s"),
                    };
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                return CheckOutcome::Fail {
                    reason: format!("error waiting for `{cmd}`: {e}"),
                };
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_true_exits_zero() {
        let outcome = command_exits_zero("true", Path::new("/tmp"), 5);
        assert_eq!(outcome, CheckOutcome::Pass);
    }

    #[test]
    fn test_false_fails() {
        match command_exits_zero("false", Path::new("/tmp"), 5) {
            CheckOutcome::Fail { .. } => {}
            other => panic!("expected Fail, got {:?}", other),
        }
    }

    #[test]
    fn test_missing_command_fails() {
        match command_exits_zero("__no_such_binary_exists__", Path::new("/tmp"), 5) {
            CheckOutcome::Fail { reason } => {
                assert!(reason.contains("__no_such_binary_exists__"));
            }
            other => panic!("expected Fail, got {:?}", other),
        }
    }

    #[test]
    fn test_empty_command_fails() {
        match command_exits_zero("", Path::new("/tmp"), 5) {
            CheckOutcome::Fail { reason } => {
                assert!(reason.contains("empty"));
            }
            other => panic!("expected Fail, got {:?}", other),
        }
    }

    #[test]
    fn test_timeout_kills_process() {
        let start = std::time::Instant::now();
        let outcome = command_exits_zero("sleep 60", Path::new("/tmp"), 1);
        let elapsed = start.elapsed();
        assert!(elapsed.as_secs() < 10, "should have timed out quickly");
        match outcome {
            CheckOutcome::Fail { reason } => {
                assert!(reason.contains("timed out"));
            }
            other => panic!("expected Fail(timeout), got {:?}", other),
        }
    }

    #[test]
    fn test_cwd_respected() {
        use tempfile::TempDir;
        use std::fs;
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("marker.txt");
        fs::write(&marker, "x").unwrap();
        let outcome = command_exits_zero("ls marker.txt", dir.path(), 5);
        assert_eq!(outcome, CheckOutcome::Pass);
    }
}
