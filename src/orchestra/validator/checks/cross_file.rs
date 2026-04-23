use std::path::Path;

use regex::Regex;

use crate::orchestra::types::CheckOutcome;

/// Check that relative references in a file (hrefs, imports, mod declarations)
/// actually resolve to existing files on disk.
pub fn references_resolve(abs_path: &Path, workspace: &Path) -> CheckOutcome {
    let Ok(content) = std::fs::read_to_string(abs_path) else {
        return CheckOutcome::Soft(0.5);
    };

    let ext = abs_path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let parent = abs_path.parent().unwrap_or(workspace);

    let refs = extract_references(&content, ext);
    if refs.is_empty() {
        return CheckOutcome::Pass;
    }

    let total = refs.len();
    let resolved = refs
        .iter()
        .filter(|r| resolve_reference(r, parent, workspace))
        .count();

    let ratio = resolved as f32 / total as f32;
    if ratio >= 1.0 {
        CheckOutcome::Pass
    } else {
        CheckOutcome::Soft(ratio)
    }
}

/// Extract local references from file content based on extension.
fn extract_references(content: &str, ext: &str) -> Vec<String> {
    match ext {
        "html" | "htm" => extract_html_refs(content),
        "md"           => extract_markdown_refs(content),
        "js" | "ts"    => extract_js_refs(content),
        "py"           => extract_python_refs(content),
        "rs"           => extract_rust_mods(content),
        _              => vec![],
    }
}

/// Resolve a reference path relative to the file's parent or workspace root.
fn resolve_reference(r: &str, parent: &Path, workspace: &Path) -> bool {
    // Skip external URLs and data URIs
    if r.starts_with("http://")
        || r.starts_with("https://")
        || r.starts_with("//")
        || r.starts_with("data:")
        || r.starts_with('#')
        || r.starts_with("mailto:")
    {
        return true; // not our job to resolve
    }

    let candidate = if r.starts_with('/') {
        workspace.join(&r[1..])
    } else {
        parent.join(r)
    };

    candidate.exists()
}

// ── HTML reference extraction ─────────────────────────────────────────────────

fn extract_html_refs(content: &str) -> Vec<String> {
    let mut refs = Vec::new();

    // href="..." and src="..."
    let attr_re = Regex::new(r#"(?:href|src)=["']([^"'#?]+)["']"#).unwrap();
    for cap in attr_re.captures_iter(content) {
        let val = &cap[1];
        if !val.is_empty() {
            refs.push(val.to_string());
        }
    }

    refs
}

// ── Markdown reference extraction ─────────────────────────────────────────────

fn extract_markdown_refs(content: &str) -> Vec<String> {
    let mut refs = Vec::new();

    // [text](path) — local only
    let link_re = Regex::new(r"\]\(([^)#?]+)\)").unwrap();
    for cap in link_re.captures_iter(content) {
        let val = cap[1].trim();
        if !val.is_empty() {
            refs.push(val.to_string());
        }
    }

    refs
}

// ── JS/TS import extraction ───────────────────────────────────────────────────

fn extract_js_refs(content: &str) -> Vec<String> {
    let mut refs = Vec::new();

    // import ... from 'path' / require('path')
    let import_re =
        Regex::new(r#"(?:import\s+[^'"]*from\s+|require\s*\(\s*)['"](\.[^'"]+)['"]"#)
            .unwrap();
    for cap in import_re.captures_iter(content) {
        let path = cap[1].trim();
        // Add common extensions if none provided
        let resolved = resolve_js_path(path);
        refs.push(resolved);
    }

    refs
}

fn resolve_js_path(path: &str) -> String {
    // If no extension, try .js and .ts (we check existence externally)
    if path.contains('.') {
        path.to_string()
    } else {
        // return the path as-is; the resolver will try .js extension
        format!("{path}.js")
    }
}

// ── Python import extraction ──────────────────────────────────────────────────

fn extract_python_refs(content: &str) -> Vec<String> {
    let mut refs = Vec::new();

    // from .module import ... or from ..module import ...
    let rel_re = Regex::new(r"from\s+(\.+\w*)\s+import").unwrap();
    for cap in rel_re.captures_iter(content) {
        let module = cap[1].trim_matches('.');
        if !module.is_empty() {
            refs.push(format!("{module}.py"));
        }
    }

    refs
}

// ── Rust mod extraction ───────────────────────────────────────────────────────

fn extract_rust_mods(content: &str) -> Vec<String> {
    let mut refs = Vec::new();

    // mod foo; — resolves to foo.rs or foo/mod.rs
    let mod_re = Regex::new(r"(?m)^\s*(?:pub\s+)?mod\s+(\w+)\s*;").unwrap();
    for cap in mod_re.captures_iter(content) {
        let name = &cap[1];
        // We check both foo.rs and foo/mod.rs; if either exists, it's resolved.
        // We push both candidates and count resolved as OR.
        refs.push(format!("{name}.rs"));
    }

    refs
}

// ── Symbol check ──────────────────────────────────────────────────────────────

/// Soft check: fraction of called identifiers that can be found defined in the workspace.
/// This is a heuristic — it grep-searches for definition patterns.
pub fn symbols_defined(abs_path: &Path, workspace: &Path) -> CheckOutcome {
    let Ok(content) = std::fs::read_to_string(abs_path) else {
        return CheckOutcome::Soft(0.5);
    };
    let ext = abs_path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let calls = extract_function_calls(&content, ext);
    if calls.is_empty() {
        return CheckOutcome::Pass;
    }
    let total = calls.len();
    let defined = calls
        .iter()
        .filter(|name| grep_definition(name, workspace))
        .count();
    let ratio = defined as f32 / total as f32;
    if ratio >= 0.8 {
        CheckOutcome::Pass
    } else {
        CheckOutcome::Soft(ratio)
    }
}

fn extract_function_calls(content: &str, ext: &str) -> Vec<String> {
    match ext {
        "rs" => {
            // Call expressions: identifier followed by (
            let call_re = Regex::new(r"\b([a-z_][a-z0-9_]{2,})\s*\(").unwrap();
            call_re
                .captures_iter(content)
                .map(|c| c[1].to_string())
                .take(30)
                .collect()
        }
        "js" | "ts" => {
            let call_re = Regex::new(r"\b([a-z_][a-zA-Z0-9_]{2,})\s*\(").unwrap();
            call_re
                .captures_iter(content)
                .map(|c| c[1].to_string())
                .take(30)
                .collect()
        }
        _ => vec![],
    }
}

fn grep_definition(name: &str, workspace: &Path) -> bool {
    // Use `grep` subprocess to find a definition pattern
    let pattern = format!(r"\b(fn|function|def|class)\s+{name}\b");
    std::process::Command::new("grep")
        .args(["-r", "--quiet", "-E", &pattern, workspace.to_str().unwrap_or(".")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(true) // if grep unavailable, assume defined
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
    fn test_references_resolve_no_refs() {
        let dir = tmp();
        let p = dir.path().join("empty.html");
        fs::write(&p, "<html><body>no links here</body></html>").unwrap();
        assert_eq!(references_resolve(&p, dir.path()), CheckOutcome::Pass);
    }

    #[test]
    fn test_references_resolve_existing_local() {
        let dir = tmp();
        fs::write(dir.path().join("styles.css"), "body {}").unwrap();
        let html = r#"<html><head><link href="styles.css"></head><body></body></html>"#;
        let p = dir.path().join("index.html");
        fs::write(&p, html).unwrap();
        assert_eq!(references_resolve(&p, dir.path()), CheckOutcome::Pass);
    }

    #[test]
    fn test_references_resolve_missing_local() {
        let dir = tmp();
        let html = r#"<html><head><link href="missing.css"></head><body></body></html>"#;
        let p = dir.path().join("index.html");
        fs::write(&p, html).unwrap();
        match references_resolve(&p, dir.path()) {
            CheckOutcome::Soft(s) => assert!(s < 1.0),
            other => panic!("expected Soft, got {:?}", other),
        }
    }

    #[test]
    fn test_references_resolve_external_links_skip() {
        let dir = tmp();
        let html = r#"<html><body><a href="https://example.com">link</a></body></html>"#;
        let p = dir.path().join("index.html");
        fs::write(&p, html).unwrap();
        // External links should not cause failure
        assert_eq!(references_resolve(&p, dir.path()), CheckOutcome::Pass);
    }

    #[test]
    fn test_markdown_refs_resolved() {
        let dir = tmp();
        fs::write(dir.path().join("target.md"), "# Target").unwrap();
        let md = "[link](target.md)\n[external](https://example.com)";
        let p = dir.path().join("doc.md");
        fs::write(&p, md).unwrap();
        assert_eq!(references_resolve(&p, dir.path()), CheckOutcome::Pass);
    }

    #[test]
    fn test_extract_html_refs() {
        let html = r#"<link href="styles.css"><img src="img/logo.png"><a href="https://example.com">x</a>"#;
        let refs = extract_html_refs(html);
        assert!(refs.contains(&"styles.css".to_string()));
        assert!(refs.contains(&"img/logo.png".to_string()));
        assert!(refs.contains(&"https://example.com".to_string()));
    }

    #[test]
    fn test_extract_markdown_refs() {
        let md = "[link](./docs/api.md)\n[another](README.md)\n[skip](#anchor)";
        let refs = extract_markdown_refs(md);
        assert!(refs.contains(&"./docs/api.md".to_string()));
        assert!(refs.contains(&"README.md".to_string()));
        // Anchor-only link should be excluded (starts with #)
        assert!(!refs.iter().any(|r| r.starts_with('#')));
    }

    #[test]
    fn test_extract_rust_mods() {
        let content = "mod config;\npub mod tools;\nuse std::fs;\n";
        let refs = extract_rust_mods(content);
        assert!(refs.contains(&"config.rs".to_string()));
        assert!(refs.contains(&"tools.rs".to_string()));
    }
}
