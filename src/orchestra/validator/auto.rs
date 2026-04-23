use std::path::Path;

use crate::orchestra::types::{Check, TaskSpec};

/// Infer extra checks from workspace signals (lock files, manifests, CI configs).
/// These supplement the checks already inferred from `spec.expected_files`.
pub fn detect_extra_checks(spec: &TaskSpec, workspace: &Path) -> Vec<Check> {
    // Only add build checks if the spec doesn't already skip them.
    let skip_build = spec.skip_checks.iter().any(|s| s == "build" || s == "auto");
    if skip_build {
        return Vec::new();
    }

    let mut checks = Vec::new();

    // Rust project — cargo check
    if workspace.join("Cargo.toml").exists() {
        checks.push(Check::CommandExitsZero {
            cmd: "cargo check --quiet".into(),
            cwd: None,
        });
    }

    // Node / JS / TS project
    if workspace.join("package.json").exists() {
        if has_npm_script(workspace, "build") {
            checks.push(Check::CommandExitsZero {
                cmd: "npm run build".into(),
                cwd: None,
            });
        }
        if has_npm_script(workspace, "test") {
            checks.push(Check::CommandExitsZero {
                cmd: "npm test -- --passWithNoTests".into(),
                cwd: None,
            });
        }
    }

    // Python project — syntax check all .py files
    if workspace.join("pyproject.toml").exists() || workspace.join("setup.py").exists() {
        let py_files = collect_files_with_ext(workspace, "py");
        if !py_files.is_empty() {
            let args = py_files
                .iter()
                .map(|p| shell_escape(p.to_str().unwrap_or("")))
                .collect::<Vec<_>>()
                .join(" ");
            checks.push(Check::CommandExitsZero {
                cmd: format!("python3 -m py_compile {args}"),
                cwd: None,
            });
        }
    }

    // GitHub Actions / CI workflows — actionlint if available
    let workflows_dir = workspace.join(".github").join("workflows");
    if workflows_dir.is_dir() && which("actionlint") {
        checks.push(Check::CommandExitsZero {
            cmd: "actionlint".into(),
            cwd: None,
        });
    }

    // SQL files — sqlfluff if available
    if !collect_files_with_ext(workspace, "sql").is_empty() && which("sqlfluff") {
        checks.push(Check::CommandExitsZero {
            cmd: "sqlfluff lint --dialect ansi .".into(),
            cwd: None,
        });
    }

    checks
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn has_npm_script(workspace: &Path, script: &str) -> bool {
    let pkg = workspace.join("package.json");
    let Ok(content) = std::fs::read_to_string(&pkg) else {
        return false;
    };
    let needle = format!(r#""{script}""#);
    if let Some(scripts_pos) = content.find(r#""scripts""#) {
        content[scripts_pos..].contains(&needle)
    } else {
        false
    }
}

fn collect_files_with_ext(workspace: &Path, ext: &str) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    collect_recursive(workspace, ext, &mut out);
    out
}

fn collect_recursive(dir: &Path, ext: &str, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') || name == "node_modules" || name == "target" {
                continue;
            }
            collect_recursive(&path, ext, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some(ext) {
            out.push(path);
        }
    }
}

fn which(bin: &str) -> bool {
    std::process::Command::new("which")
        .arg(bin)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
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

    fn empty_spec() -> TaskSpec {
        TaskSpec::default()
    }

    #[test]
    fn test_no_signals_yields_empty() {
        let dir = tmp();
        let checks = detect_extra_checks(&empty_spec(), dir.path());
        assert!(checks.is_empty());
    }

    #[test]
    fn test_cargo_toml_adds_cargo_check() {
        let dir = tmp();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"").unwrap();
        let checks = detect_extra_checks(&empty_spec(), dir.path());
        assert!(checks.iter().any(|c| matches!(
            c,
            Check::CommandExitsZero { cmd, .. } if cmd.starts_with("cargo check")
        )));
    }

    #[test]
    fn test_package_json_without_scripts_no_npm() {
        let dir = tmp();
        fs::write(dir.path().join("package.json"), r#"{"name":"x"}"#).unwrap();
        let checks = detect_extra_checks(&empty_spec(), dir.path());
        assert!(!checks.iter().any(|c| matches!(
            c,
            Check::CommandExitsZero { cmd, .. } if cmd.contains("npm")
        )));
    }

    #[test]
    fn test_package_json_with_build_script() {
        let dir = tmp();
        fs::write(
            dir.path().join("package.json"),
            r#"{"name":"x","scripts":{"build":"tsc"}}"#,
        )
        .unwrap();
        let checks = detect_extra_checks(&empty_spec(), dir.path());
        assert!(checks.iter().any(|c| matches!(
            c,
            Check::CommandExitsZero { cmd, .. } if cmd.contains("npm run build")
        )));
    }

    #[test]
    fn test_skip_auto_suppresses_all() {
        let dir = tmp();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"").unwrap();
        let spec = TaskSpec {
            skip_checks: vec!["auto".into()],
            ..Default::default()
        };
        let checks = detect_extra_checks(&spec, dir.path());
        assert!(checks.is_empty());
    }

    #[test]
    fn test_shell_escape() {
        assert_eq!(shell_escape("foo/bar.py"), "'foo/bar.py'");
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
    }

    #[test]
    fn test_collect_files_skips_node_modules() {
        let dir = tmp();
        let nm = dir.path().join("node_modules");
        fs::create_dir_all(&nm).unwrap();
        fs::write(nm.join("dep.py"), "x=1").unwrap();
        fs::write(dir.path().join("main.py"), "x=1").unwrap();
        let found = collect_files_with_ext(dir.path(), "py");
        assert_eq!(found.len(), 1);
        assert!(found[0].ends_with("main.py"));
    }
}
