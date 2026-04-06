//! When the model cannot call tools, it often emits shell commands in fenced blocks.
//! We strip those from the visible reply and offer to run them locally (with confirmation).
//! Non-shell code blocks are kept in the display text AND collected so the caller can
//! offer to save them to a file.

const SHELL_LANGS: &[&str] = &[
    "bash", "sh", "shell", "zsh", "fish",
    "cmd", "batch", "bat",
    "powershell", "pwsh", "ps1",
];

/// A non-shell code block extracted from the LLM reply.
pub struct FileBlock {
    /// Language tag as written by the model (e.g. "rust", "python", "batch").
    pub lang: String,
    /// Content to write (trimmed); leading `// path:` / `# file:` lines are stripped when they carry the path.
    pub content: String,
    /// Path from fence info (` ```rust src/main.rs`) or first-line marker; used so we need not ask the user.
    pub suggested_path: Option<String>,
}

/// True if `s` looks like a relative/absolute file path or filename (not a version number).
fn looks_like_file_path(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() {
        return false;
    }
    if s.contains('/') || s.contains('\\') {
        return true;
    }
    if s.eq_ignore_ascii_case("dockerfile")
        || s.eq_ignore_ascii_case("makefile")
        || s.eq_ignore_ascii_case("justfile")
    {
        return true;
    }
    let Some((base, ext)) = s.rsplit_once('.') else {
        return false;
    };
    if base.is_empty() || ext.is_empty() {
        return false;
    }
    // avoid "1.2.3" style crumbs
    if ext.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    ext.chars().all(|c| c.is_alphanumeric() || c == '_') && (1..=12).contains(&ext.len())
}

fn infer_lang_from_filename(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "rs" => "rust",
        "py" => "python",
        "js" => "javascript",
        "ts" => "typescript",
        "tsx" => "typescript",
        "jsx" => "javascript",
        "md" | "mdx" => "markdown",
        "toml" => "toml",
        "json" => "json",
        "yml" | "yaml" => "yaml",
        "go" => "go",
        "c" => "c",
        "h" => "c",
        "cpp" | "cc" | "cxx" => "cpp",
        "java" => "java",
        "rb" => "ruby",
        "html" | "htm" => "html",
        "css" => "css",
        "sql" => "sql",
        "sh" => "bash",
        "ps1" => "powershell",
        "bat" | "cmd" => "batch",
        _ => "text",
    }
    .to_string()
}

/// Parse ```info line```: `rust`, `rust src/main.rs`, or `SUMMARY.md` (path-only).
fn parse_fence_info_line(info: &str) -> (String, Option<String>) {
    let parts: Vec<&str> = info.split_whitespace().collect();
    if parts.is_empty() {
        return (String::new(), None);
    }
    let first = parts[0];
    let first_lc = first
        .to_lowercase()
        .split('{')
        .next()
        .unwrap_or("")
        .to_string();

    if parts.len() == 1 && looks_like_file_path(first) {
        return (infer_lang_from_filename(first), Some(first.to_string()));
    }

    let path = parts
        .get(1)
        .filter(|p| looks_like_file_path(p))
        .map(|s| (*s).to_string());

    (first_lc, path)
}

/// First-line markers: `// path: x`, `# file: x`, `<!-- path: x -->`.
fn strip_leading_path_marker(body: &str) -> (Option<String>, String) {
    let lines: Vec<&str> = body.lines().collect();
    let mut i = 0;
    while i < lines.len() && lines[i].trim().is_empty() {
        i += 1;
    }
    if i >= lines.len() {
        return (None, body.trim().to_string());
    }
    let first = lines[i].trim();
    let path = try_parse_path_from_first_line(first);
    if let Some(p) = path {
        let rest = lines[(i + 1)..].join("\n");
        return (Some(p), rest.trim().to_string());
    }
    (None, body.trim().to_string())
}

fn try_parse_path_from_first_line(line: &str) -> Option<String> {
    let t = line.trim();

    if t.starts_with("<!--") {
        let end = t.find("-->")?;
        let inner = t[4..end].trim();
        let lower = inner.to_lowercase();
        let rest = if lower.starts_with("path:") {
            inner[5..].trim()
        } else if lower.starts_with("file:") {
            inner[5..].trim()
        } else {
            return None;
        };
        let p = rest.trim_matches(|c| c == '`' || c == '"' || c == '\'');
        return looks_like_file_path(p).then(|| p.to_string());
    }

    let body = t.strip_prefix("//").or_else(|| t.strip_prefix('#'))?.trim();
    let lower = body.to_lowercase();
    let rest = if lower.starts_with("path:") {
        body[5..].trim()
    } else if lower.starts_with("file:") {
        body[5..].trim()
    } else {
        return None;
    };
    let p = rest.trim_matches(|c| c == '`' || c == '"' || c == '\'');
    looks_like_file_path(p).then(|| p.to_string())
}

/// Parse `text` and separate shell blocks from everything else.
///
/// Returns:
/// - `display`    — the prose to render in the terminal (shell blocks removed, file blocks kept)
/// - `shell_cmds` — bodies of shell-language blocks, offered to run
/// - `file_blocks`— non-shell code blocks, offered to save to a file
pub fn strip_shell_fences(text: &str) -> (String, Vec<String>, Vec<FileBlock>) {
    let mut display = String::with_capacity(text.len());
    let mut cmds = Vec::new();
    let mut files = Vec::new();
    let mut cursor = text;

    while let Some(idx) = cursor.find("```") {
        display.push_str(&cursor[..idx]);
        cursor = &cursor[idx + 3..];

        let (info, after_info) = match cursor.find('\n') {
            Some(nl) => (cursor[..nl].trim(), &cursor[nl + 1..]),
            None => {
                display.push_str("```");
                display.push_str(cursor);
                return (display, cmds, files);
            }
        };

        let (fence_lang, path_from_fence) = parse_fence_info_line(info);

        cursor = after_info;
        let Some(close) = cursor.find("```") else {
            display.push_str("```");
            display.push_str(info);
            display.push('\n');
            display.push_str(cursor);
            return (display, cmds, files);
        };

        let body = &cursor[..close];
        cursor = &cursor[close + 3..];

        if SHELL_LANGS.contains(&fence_lang.as_str()) {
            let c = body.trim();
            if !c.is_empty() {
                cmds.push(c.to_string());
            }
            // Shell blocks are NOT shown in prose — handled via Run? [y/N].
        } else {
            // Keep file blocks in the display so the user can read the code.
            display.push_str("```");
            display.push_str(info);
            display.push('\n');
            display.push_str(body);
            display.push_str("```");

            let (path_from_body, content_for_write) = strip_leading_path_marker(body);
            let suggested_path = path_from_fence.or(path_from_body);
            let lang_label = if fence_lang.is_empty() {
                "text".into()
            } else {
                fence_lang.clone()
            };

            if !content_for_write.is_empty() {
                files.push(FileBlock {
                    lang: lang_label,
                    content: content_for_write,
                    suggested_path,
                });
            }
        }
    }

    display.push_str(cursor);
    (display, cmds, files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_bash_collects_command() {
        let s = "Do this:\n```bash\ncargo test\n```\nDone.";
        let (d, c, f) = strip_shell_fences(s);
        assert_eq!(c, vec!["cargo test"]);
        assert!(f.is_empty());
        assert!(d.contains("Do this"));
        assert!(d.contains("Done"));
        assert!(!d.contains("cargo test"));
    }

    #[test]
    fn keeps_rust_fence_and_collects_file_block() {
        let s = "```rust\nfn x() {}\n```";
        let (d, c, f) = strip_shell_fences(s);
        assert!(c.is_empty());
        assert_eq!(d, s);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].lang, "rust");
        assert_eq!(f[0].content, "fn x() {}");
    }

    #[test]
    fn multiple_shell_blocks() {
        let s = "```sh\na\n```\n```sh\nb\n```";
        let (_d, c, f) = strip_shell_fences(s);
        assert_eq!(c, vec!["a", "b"]);
        assert!(f.is_empty());
    }

    #[test]
    fn batch_treated_as_shell() {
        let s = "```batch\n@echo off\necho Hola\n```";
        let (_d, c, f) = strip_shell_fences(s);
        assert_eq!(c, vec!["@echo off\necho Hola"]);
        assert!(f.is_empty());
    }

    #[test]
    fn powershell_treated_as_shell() {
        let s = "```powershell\nWrite-Host 'hi'\n```";
        let (_d, c, _f) = strip_shell_fences(s);
        assert_eq!(c, vec!["Write-Host 'hi'"]);
    }

    #[test]
    fn non_shell_block_collected_as_file_block() {
        let s = "Save this:\n```python\nprint('hello')\n```";
        let (d, c, f) = strip_shell_fences(s);
        assert!(c.is_empty());
        assert!(d.contains("```python"));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].lang, "python");
        assert!(f[0].content.contains("print"));
        assert!(f[0].suggested_path.is_none());
    }

    #[test]
    fn fence_line_lang_plus_path_sets_suggested_path() {
        let s = "```markdown SUMMARY.md\n# Title\nbody\n```";
        let (_d, c, f) = strip_shell_fences(s);
        assert!(c.is_empty());
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].lang, "markdown");
        assert_eq!(f[0].suggested_path.as_deref(), Some("SUMMARY.md"));
        assert_eq!(f[0].content, "# Title\nbody");
    }

    #[test]
    fn fence_path_only_infers_lang() {
        let s = "```CONFIG.toml\nkey = 1\n```";
        let (_d, c, f) = strip_shell_fences(s);
        assert!(c.is_empty());
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].suggested_path.as_deref(), Some("CONFIG.toml"));
        assert_eq!(f[0].lang, "toml");
        assert_eq!(f[0].content, "key = 1");
    }

    #[test]
    fn rust_fence_second_token_path() {
        let s = "```rust src/lib.rs\npub fn x() {}\n```";
        let (_d, c, f) = strip_shell_fences(s);
        assert!(c.is_empty());
        assert_eq!(f[0].suggested_path.as_deref(), Some("src/lib.rs"));
        assert_eq!(f[0].content, "pub fn x() {}");
    }

    #[test]
    fn leading_slash_slash_path_stripped_from_content() {
        let s = "```rust\n// path: src/foo.rs\nlet n = 1;\n```";
        let (_d, c, f) = strip_shell_fences(s);
        assert!(c.is_empty());
        assert_eq!(f[0].suggested_path.as_deref(), Some("src/foo.rs"));
        assert_eq!(f[0].content, "let n = 1;");
    }
}
