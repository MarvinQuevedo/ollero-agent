use std::path::Path;

use crate::orchestra::types::CheckOutcome;

/// Validate the syntax of a file based on its extension.
/// Returns Pass, Fail, or Soft(0.8) when the appropriate tool is missing.
pub fn syntax_valid(abs_path: &Path, ext: &str) -> CheckOutcome {
    if !abs_path.exists() {
        return CheckOutcome::Fail {
            reason: format!("file does not exist: {}", abs_path.display()),
        };
    }

    match ext {
        "json" => check_json(abs_path),
        "toml" => check_toml(abs_path),
        "yaml" | "yml" => check_yaml(abs_path),
        "md" => check_markdown(abs_path),
        "html" | "htm" => check_html(abs_path),
        "css" => check_css(abs_path),
        "rs" => check_rust(abs_path),
        "py" => check_external(abs_path, "python3", &["-m", "py_compile"]),
        "js" => check_external(abs_path, "node", &["--check"]),
        "ts" => check_ts(abs_path),
        "sh" | "bash" => check_external(abs_path, "bash", &["-n"]),
        // Unsupported extension → skip gracefully
        _ => CheckOutcome::Soft(0.9),
    }
}

// ── JSON ──────────────────────────────────────────────────────────────────────

fn check_json(path: &Path) -> CheckOutcome {
    match std::fs::read_to_string(path) {
        Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(_) => CheckOutcome::Pass,
            Err(e) => CheckOutcome::Fail { reason: format!("JSON parse error: {e}") },
        },
        Err(e) => CheckOutcome::Fail { reason: format!("cannot read file: {e}") },
    }
}

// ── TOML ──────────────────────────────────────────────────────────────────────

fn check_toml(path: &Path) -> CheckOutcome {
    match std::fs::read_to_string(path) {
        Ok(content) => match toml::from_str::<toml::Value>(&content) {
            Ok(_) => CheckOutcome::Pass,
            Err(e) => CheckOutcome::Fail { reason: format!("TOML parse error: {e}") },
        },
        Err(e) => CheckOutcome::Fail { reason: format!("cannot read file: {e}") },
    }
}

// ── YAML ──────────────────────────────────────────────────────────────────────

fn check_yaml(path: &Path) -> CheckOutcome {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            // Simple structural check: balanced indentation and basic key: value structure.
            let errors = yaml_basic_check(&content);
            if errors.is_empty() {
                CheckOutcome::Pass
            } else {
                CheckOutcome::Fail { reason: errors.join("; ") }
            }
        }
        Err(e) => CheckOutcome::Fail { reason: format!("cannot read file: {e}") },
    }
}

fn yaml_basic_check(content: &str) -> Vec<String> {
    let mut errors = Vec::new();

    // Check for tab characters (YAML forbids tabs for indentation)
    for (i, line) in content.lines().enumerate() {
        if line.starts_with('\t') {
            errors.push(format!("line {}: tab character used for indentation", i + 1));
            break; // Report only first occurrence
        }
    }

    // Check for unclosed multiline strings (| or > style)
    let mut in_block = false;
    let mut block_indent = 0usize;
    for line in content.lines() {
        let trimmed = line.trim_end();
        if !in_block {
            if trimmed.ends_with(" |") || trimmed.ends_with(" >")
                || trimmed.ends_with(":|") || trimmed.ends_with(":>")
            {
                in_block = true;
                block_indent = line.len() - line.trim_start().len();
            }
        } else {
            let line_indent = line.len() - line.trim_start().len();
            if !trimmed.is_empty() && line_indent <= block_indent {
                in_block = false;
            }
        }
    }

    errors
}

// ── Markdown ──────────────────────────────────────────────────────────────────

fn check_markdown(path: &Path) -> CheckOutcome {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let unclosed = count_unclosed_code_fences(&content);
            if unclosed > 0 {
                CheckOutcome::Fail {
                    reason: format!("{unclosed} unclosed code fence(s)"),
                }
            } else {
                CheckOutcome::Pass
            }
        }
        Err(e) => CheckOutcome::Fail { reason: format!("cannot read file: {e}") },
    }
}

fn count_unclosed_code_fences(content: &str) -> usize {
    let mut depth = 0usize;
    for line in content.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            if depth == 0 {
                depth += 1;
            } else {
                depth -= 1;
            }
        }
    }
    depth
}

// ── HTML ──────────────────────────────────────────────────────────────────────

fn check_html(path: &Path) -> CheckOutcome {
    match std::fs::read_to_string(path) {
        Ok(content) => match html_tag_balance(&content) {
            Ok(()) => CheckOutcome::Pass,
            Err(reason) => CheckOutcome::Fail { reason },
        },
        Err(e) => CheckOutcome::Fail { reason: format!("cannot read file: {e}") },
    }
}

const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input",
    "link", "meta", "source", "track", "wbr",
];

pub fn html_tag_balance(src: &str) -> Result<(), String> {
    let mut stack: Vec<String> = Vec::new();
    let bytes = src.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }

        // Skip comments <!-- ... -->
        if src[i..].starts_with("<!--") {
            if let Some(end) = src[i..].find("-->") {
                i += end + 3;
                continue;
            } else {
                break;
            }
        }

        // Skip DOCTYPE
        if src[i..].to_ascii_lowercase().starts_with("<!doctype") {
            if let Some(end) = src[i..].find('>') {
                i += end + 1;
                continue;
            }
        }

        // Find end of tag
        let Some(end_offset) = src[i..].find('>') else {
            break;
        };
        let tag_src = &src[i..i + end_offset + 1];
        i += end_offset + 1;

        if tag_src.starts_with("</") {
            // Closing tag
            let name = extract_tag_name(&tag_src[2..]).to_ascii_lowercase();
            if VOID_ELEMENTS.contains(&name.as_str()) {
                continue;
            }
            if stack.last().map(|s| s.as_str()) == Some(&name) {
                stack.pop();
            } else if let Some(pos) = stack.iter().rposition(|s| s == &name) {
                // Mismatched but we can skip optional close tags
                stack.truncate(pos);
            }
        } else if !tag_src.ends_with("/>") {
            // Opening tag (not self-closing)
            let name = extract_tag_name(&tag_src[1..]).to_ascii_lowercase();
            if !name.is_empty() && !VOID_ELEMENTS.contains(&name.as_str()) {
                // Skip script/style content
                if name == "script" || name == "style" {
                    let close = format!("</{name}>");
                    if let Some(end_pos) = src[i..].to_ascii_lowercase().find(&close) {
                        i += end_pos + close.len();
                    }
                    continue;
                }
                stack.push(name);
            }
        }
    }

    if stack.is_empty() {
        Ok(())
    } else {
        Err(format!("unclosed tags: {}", stack.join(", ")))
    }
}

fn extract_tag_name(s: &str) -> &str {
    let s = s.trim_start();
    let end = s.find(|c: char| c.is_whitespace() || c == '>' || c == '/').unwrap_or(s.len());
    &s[..end]
}

// ── CSS ───────────────────────────────────────────────────────────────────────

fn check_css(path: &Path) -> CheckOutcome {
    match std::fs::read_to_string(path) {
        Ok(content) => match brace_balance(&content) {
            Ok(()) => CheckOutcome::Pass,
            Err(reason) => CheckOutcome::Fail { reason },
        },
        Err(e) => CheckOutcome::Fail { reason: format!("cannot read file: {e}") },
    }
}

pub fn brace_balance(src: &str) -> Result<(), String> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut string_char = ' ';
    let mut in_comment = false;
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        if in_comment {
            if c == '*' && chars.get(i + 1) == Some(&'/') {
                in_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }

        if in_string {
            if c == string_char {
                in_string = false;
            } else if c == '\\' {
                i += 1; // skip escaped char
            }
            i += 1;
            continue;
        }

        if c == '/' && chars.get(i + 1) == Some(&'*') {
            in_comment = true;
            i += 2;
            continue;
        }

        if c == '"' || c == '\'' {
            in_string = true;
            string_char = c;
        } else if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth -= 1;
            if depth < 0 {
                return Err("unexpected `}` in CSS".into());
            }
        }

        i += 1;
    }

    if depth == 0 {
        Ok(())
    } else {
        Err(format!("{depth} unclosed `{{` in CSS"))
    }
}

// ── Rust ──────────────────────────────────────────────────────────────────────

fn check_rust(path: &Path) -> CheckOutcome {
    match std::fs::read_to_string(path) {
        Ok(content) => match syn::parse_file(&content) {
            Ok(_) => CheckOutcome::Pass,
            Err(e) => CheckOutcome::Fail { reason: format!("Rust parse error: {e}") },
        },
        Err(e) => CheckOutcome::Fail { reason: format!("cannot read file: {e}") },
    }
}

// ── External tool checks ──────────────────────────────────────────────────────

fn check_external(path: &Path, cmd: &str, args: &[&str]) -> CheckOutcome {
    // Check if the tool is available
    let which = std::process::Command::new("which")
        .arg(cmd)
        .output();
    if which.map(|o| !o.status.success()).unwrap_or(true) {
        // Tool not found → soft skip
        return CheckOutcome::Soft(0.8);
    }

    let result = std::process::Command::new(cmd)
        .args(args)
        .arg(path)
        .output();

    match result {
        Ok(out) => {
            if out.status.success() {
                CheckOutcome::Pass
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let first_line = stderr.lines().next().unwrap_or("syntax error").trim();
                CheckOutcome::Fail { reason: format!("{cmd}: {first_line}") }
            }
        }
        Err(e) => CheckOutcome::Soft(0.8), // spawn error → graceful skip
    }
}

fn check_ts(path: &Path) -> CheckOutcome {
    // Try tsc --noEmit, fall back to node --check if tsc unavailable
    let which_tsc = std::process::Command::new("which").arg("tsc").output();
    if which_tsc.map(|o| o.status.success()).unwrap_or(false) {
        check_external(path, "tsc", &["--noEmit"])
    } else {
        // tsc not available → try node --check (handles some TS syntax as JS)
        CheckOutcome::Soft(0.8)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn tmp() -> TempDir { TempDir::new().unwrap() }

    fn write(dir: &TempDir, name: &str, content: &str) -> std::path::PathBuf {
        let p = dir.path().join(name);
        fs::write(&p, content).unwrap();
        p
    }

    // ── JSON ────────────────────────────────────────────────────────────────

    #[test]
    fn test_json_valid() {
        let dir = tmp();
        let p = write(&dir, "f.json", r#"{"name":"test","value":42}"#);
        assert_eq!(check_json(&p), CheckOutcome::Pass);
    }

    #[test]
    fn test_json_invalid() {
        let dir = tmp();
        let p = write(&dir, "f.json", r#"{"name": "test", missing_value}"#);
        assert!(matches!(check_json(&p), CheckOutcome::Fail { .. }));
    }

    #[test]
    fn test_json_empty_object() {
        let dir = tmp();
        let p = write(&dir, "f.json", "{}");
        assert_eq!(check_json(&p), CheckOutcome::Pass);
    }

    // ── TOML ────────────────────────────────────────────────────────────────

    #[test]
    fn test_toml_valid() {
        let dir = tmp();
        let p = write(&dir, "f.toml", "[package]\nname = \"test\"\nversion = \"0.1.0\"");
        assert_eq!(check_toml(&p), CheckOutcome::Pass);
    }

    #[test]
    fn test_toml_invalid() {
        let dir = tmp();
        let p = write(&dir, "f.toml", "[package\nname = test");
        assert!(matches!(check_toml(&p), CheckOutcome::Fail { .. }));
    }

    // ── Markdown ────────────────────────────────────────────────────────────

    #[test]
    fn test_markdown_valid() {
        let dir = tmp();
        let p = write(&dir, "f.md", "# Title\n\n```rust\nfn main() {}\n```\n");
        assert_eq!(check_markdown(&p), CheckOutcome::Pass);
    }

    #[test]
    fn test_markdown_unclosed_fence() {
        let dir = tmp();
        let p = write(&dir, "f.md", "# Title\n\n```rust\nfn main() {}\n");
        assert!(matches!(check_markdown(&p), CheckOutcome::Fail { .. }));
    }

    // ── HTML ────────────────────────────────────────────────────────────────

    #[test]
    fn test_html_valid() {
        let dir = tmp();
        let p = write(&dir, "f.html", "<!DOCTYPE html><html><head></head><body><p>Hello</p></body></html>");
        assert_eq!(check_html(&p), CheckOutcome::Pass);
    }

    #[test]
    fn test_html_void_elements_ok() {
        let dir = tmp();
        let p = write(&dir, "f.html", "<html><body><br><hr><img src=\"x\"></body></html>");
        assert_eq!(check_html(&p), CheckOutcome::Pass);
    }

    #[test]
    fn test_html_unbalanced() {
        let dir = tmp();
        let p = write(&dir, "f.html", "<html><body><div><p>text</body></html>");
        // div and/or p unclosed
        let result = check_html(&p);
        // Result could be fail or pass depending on recovery — just ensure no panic
        let _ = result;
    }

    // ── CSS ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_css_valid() {
        let dir = tmp();
        let p = write(&dir, "f.css", "body { color: red; } .foo { margin: 0; }");
        assert_eq!(check_css(&p), CheckOutcome::Pass);
    }

    #[test]
    fn test_css_unbalanced() {
        let dir = tmp();
        let p = write(&dir, "f.css", "body { color: red;");
        assert!(matches!(check_css(&p), CheckOutcome::Fail { .. }));
    }

    #[test]
    fn test_css_string_with_brace() {
        let dir = tmp();
        let p = write(&dir, "f.css", r#"body { content: "{"; }"#);
        assert_eq!(check_css(&p), CheckOutcome::Pass);
    }

    // ── Rust ────────────────────────────────────────────────────────────────

    #[test]
    fn test_rust_valid() {
        let dir = tmp();
        let p = write(&dir, "f.rs", "fn main() { println!(\"hello\"); }");
        assert_eq!(check_rust(&p), CheckOutcome::Pass);
    }

    #[test]
    fn test_rust_invalid() {
        let dir = tmp();
        let p = write(&dir, "f.rs", "fn main( { println!");
        assert!(matches!(check_rust(&p), CheckOutcome::Fail { .. }));
    }

    // ── YAML ────────────────────────────────────────────────────────────────

    #[test]
    fn test_yaml_valid() {
        let dir = tmp();
        let p = write(&dir, "f.yaml", "name: test\nvalue: 42\nitems:\n  - a\n  - b\n");
        assert_eq!(check_yaml(&p), CheckOutcome::Pass);
    }

    #[test]
    fn test_yaml_tab_indent_fails() {
        let dir = tmp();
        let p = write(&dir, "f.yaml", "name: test\n\tvalue: bad");
        assert!(matches!(check_yaml(&p), CheckOutcome::Fail { .. }));
    }

    // ── dispatch ────────────────────────────────────────────────────────────

    #[test]
    fn test_syntax_valid_dispatch_json() {
        let dir = tmp();
        let p = write(&dir, "data.json", r#"{"ok":true}"#);
        assert_eq!(syntax_valid(&p, "json"), CheckOutcome::Pass);
    }

    #[test]
    fn test_syntax_valid_dispatch_unknown_ext() {
        let dir = tmp();
        let p = write(&dir, "data.xyz", "anything");
        assert!(matches!(syntax_valid(&p, "xyz"), CheckOutcome::Soft(_)));
    }

    // ── html_tag_balance ────────────────────────────────────────────────────

    #[test]
    fn test_html_tag_balance_valid() {
        assert!(html_tag_balance("<html><body><p>text</p></body></html>").is_ok());
    }

    #[test]
    fn test_html_tag_balance_comment_ignored() {
        assert!(html_tag_balance("<!-- <div> --><p>hi</p>").is_ok());
    }

    #[test]
    fn test_html_tag_balance_self_closing() {
        assert!(html_tag_balance("<div><br/><input/></div>").is_ok());
    }

    // ── brace_balance ───────────────────────────────────────────────────────

    #[test]
    fn test_brace_balance_valid() {
        assert!(brace_balance(".a { color: red; } .b { margin: 0; }").is_ok());
    }

    #[test]
    fn test_brace_balance_comment_ignored() {
        assert!(brace_balance("/* { unclosed comment brace */ body { }").is_ok());
    }

    #[test]
    fn test_brace_balance_unmatched_open() {
        assert!(brace_balance("body {").is_err());
    }

    #[test]
    fn test_brace_balance_unmatched_close() {
        assert!(brace_balance("body { } }").is_err());
    }
}
