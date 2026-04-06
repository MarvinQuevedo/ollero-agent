use std::fs;
use std::path::Path;

/// Max bytes embedded from a single manifest file.
const MAX_MANIFEST_CHARS: usize = 12_000;
/// Max total snapshot size (UTF-8 char-safe trim happens in caller if needed).
const MAX_SNAPSHOT_CHARS: usize = 20_000;

const SKIP_DIR_NAMES: &[&str] = &[
    "target",
    "node_modules",
    ".git",
    "dist",
    "build",
    ".cache",
    "__pycache__",
    ".venv",
    "venv",
];

/// Build a markdown snapshot of `root` for the system prompt (manifests + top-level listing).
pub fn snapshot(root: &Path) -> String {
    let mut out = String::new();
    out.push_str("### Workspace snapshot\n\n");

    let display_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    out.push_str(&format!(
        "- **Project root:** `{}`\n\
         - Ollero reads this folder when it starts; run `/context refresh` after `cd` if you moved.\n\n",
        display_root.display()
    ));

    append_file_block(&mut out, &root.join("Cargo.toml"), "Cargo.toml", "toml");
    append_file_block(&mut out, &root.join("package.json"), "package.json", "json");
    append_file_block(&mut out, &root.join("pyproject.toml"), "pyproject.toml", "toml");
    append_file_block(&mut out, &root.join("go.mod"), "go.mod", "text");

    append_top_level(&mut out, root);

    if out.len() > MAX_SNAPSHOT_CHARS {
        truncate_utf8(&mut out, MAX_SNAPSHOT_CHARS);
        out.push_str("\n\n… *(snapshot truncated)*\n");
    }

    out
}

fn append_file_block(out: &mut String, path: &Path, label: &str, fence: &str) {
    if !path.is_file() {
        return;
    }
    let Ok(raw) = fs::read_to_string(path) else {
        out.push_str(&format!("\n#### {label}\n*(unreadable)*\n"));
        return;
    };
    out.push_str(&format!("\n#### {label}\n```{fence}\n"));
    push_truncated_chars(out, &raw, MAX_MANIFEST_CHARS);
    out.push_str("\n```\n");
}

fn append_top_level(out: &mut String, root: &Path) {
    out.push_str("\n#### Top-level entries\n");
    let Ok(read_dir) = fs::read_dir(root) else {
        out.push_str("*(could not read directory)*\n");
        return;
    };

    let mut entries: Vec<_> = read_dir.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());

    let mut n = 0usize;
    for e in entries {
        let name = e.file_name().to_string_lossy().to_string();
        if name == "." || name == ".." {
            continue;
        }

        if SKIP_DIR_NAMES.contains(&name.as_str()) {
            out.push_str(&format!("- `{name}/` *(content omitted)*\n"));
            n += 1;
            continue;
        }

        let meta = e.path();
        let kind = if meta.is_dir() { "dir" } else { "file" };
        out.push_str(&format!("- `{name}` ({kind})\n"));
        n += 1;
        if n >= 64 {
            out.push_str("- … *(more entries omitted)*\n");
            break;
        }
    }

    if n == 0 {
        out.push_str("*(empty)*\n");
    }
}

fn push_truncated_chars(out: &mut String, text: &str, max_chars: usize) {
    if text.chars().count() <= max_chars {
        out.push_str(text);
        return;
    }
    let taken: String = text.chars().take(max_chars).collect();
    out.push_str(&taken);
    out.push_str("\n… *(truncated)*");
}

fn truncate_utf8(s: &mut String, max_bytes: usize) {
    if s.len() <= max_bytes {
        return;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_includes_cargo_toml() {
        let dir = std::env::current_dir().expect("cwd");
        let s = snapshot(&dir);
        assert!(s.contains("Workspace snapshot"));
        assert!(s.contains("Project root"));
        if dir.join("Cargo.toml").is_file() {
            assert!(s.contains("Cargo.toml"));
        }
    }

    #[test]
    fn snapshot_unreadable_dir_still_has_header() {
        let dir = std::env::temp_dir().join("ollero_workspace_test_nonexistent_dir");
        let _ = std::fs::remove_dir_all(&dir);
        let s = snapshot(&dir);
        assert!(s.contains("Workspace snapshot"));
        assert!(s.contains("Project root"));
    }
}
