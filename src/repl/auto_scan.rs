//! When the model cannot use tools, detect “read my project / status” style asks
//! and attach a bounded tree + glob + key file contents before calling the LLM.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Result;

use crate::tools;

const MAX_SCAN_BYTES: usize = 100_000;
/// Max paths listed in the combined “source files” section (after filtering).
const MAX_LISTED_PATHS: usize = 280;

/// Path segments that usually mean dependencies / build output — skip in globs.
const SKIP_DIR_MARKERS: &[&str] = &[
    "/node_modules/",
    "/target/",
    "/.git/",
    "/dist/",
    "/build/",
    "/out/",
    "/.next/",
    "/.nuxt/",
    "/__pycache__/",
    "/.venv/",
    "/venv/",
    "/vendor/",
    "/Pods/",
    "/.turbo/",
    "/.cache/",
    "/coverage/",
    "/.gradle/",
];

/// Globs for “source-like” files we list (not file contents — just paths). Order: roughly by stack.
const SOURCE_GLOBS: &[&str] = &[
    "**/*.rs",
    "**/*.py",
    "**/*.go",
    "**/*.ts",
    "**/*.tsx",
    "**/*.mts",
    "**/*.cts",
    "**/*.js",
    "**/*.jsx",
    "**/*.mjs",
    "**/*.c",
    "**/*.h",
    "**/*.cpp",
    "**/*.hpp",
    "**/*.cc",
    "**/*.java",
    "**/*.kt",
    "**/*.kts",
    "**/*.cs",
    "**/*.swift",
    "**/*.rb",
    "**/*.php",
    "**/*.vue",
    "**/*.svelte",
    "**/*.zig",
    "**/*.lua",
    "**/*.sh",
    "**/*.ps1",
    "**/*.sql",
    "**/*.toml",
    "**/*.yaml",
    "**/*.yml",
    "**/*.md",
    "**/*.css",
    "**/*.scss",
    "**/*.html",
    "**/Dockerfile",
    "**/Makefile",
    "**/tsconfig.json",
    "**/jsconfig.json",
];

/// True if the user message is asking for a broad read / status of the repo.
pub fn should_trigger(msg: &str) -> bool {
    let m = msg.to_lowercase();
    let t = m.trim();
    if t.len() < 12 {
        return false;
    }

    const TRIGGERS: &[&str] = &[
        "read my files",
        "read the files",
        "read my project",
        "scan the project",
        "scan project",
        "project status",
        "current state",
        "current progress",
        "overall progress",
        "what's in this project",
        "whats in this project",
        "what is in this project",
        "analyze the codebase",
        "analyze the project",
        "analyse the project",
        "lee mis archivos",
        "lee los archivos",
        "lee el proyecto",
        "lee mi proyecto",
        "revisa el proyecto",
        "revisa los archivos",
        "revisa mi proyecto",
        "estado del proyecto",
        "estado actual",
        "progreso actual",
        "cuál es el progreso",
        "cual es el progreso",
        "qué progreso",
        "que progreso",
        "dime el progreso",
        "dime cual es el progreso",
        "dime cuál es el progreso",
        "cuál es el estado",
        "cual es el estado",
        "analiza el proyecto",
        "analiza los archivos",
        "qué hay en el proyecto",
        "que hay en el proyecto",
        "qué contiene",
        "que contiene",
        "resume el proyecto",
        "resumen del proyecto",
        "lectura completa del proyecto",
        "lectura completa del repositorio",
        "lectura completa del repo",
    ];

    TRIGGERS.iter().any(|p| t.contains(p))
}

fn normalized_path_for_filter(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

fn should_list_path(path: &str) -> bool {
    let n = normalized_path_for_filter(path);
    !SKIP_DIR_MARKERS.iter().any(|m| n.contains(m))
}

/// Collect paths from several globs, dedupe, filter noisy dirs, cap count.
fn collect_source_paths(base: &str) -> String {
    let mut set: BTreeSet<String> = BTreeSet::new();
    let mut capped = false;

    'patterns: for pattern in SOURCE_GLOBS {
        if set.len() >= MAX_LISTED_PATHS {
            capped = true;
            break;
        }
        let chunk = match tools::run_glob(pattern, Some(base)) {
            Ok(s) => s,
            Err(_) => continue,
        };
        for line in chunk.lines() {
            if set.len() >= MAX_LISTED_PATHS {
                capped = true;
                break 'patterns;
            }
            let line = line.trim();
            if line.is_empty()
                || line.starts_with('[')
                || line.starts_with("No files matching")
            {
                continue;
            }
            if should_list_path(line) {
                set.insert(line.to_string());
            }
        }
    }

    if set.is_empty() {
        return "(no matching source paths after filters)".to_string();
    }

    let mut out = set.into_iter().collect::<Vec<_>>().join("\n");
    if capped {
        out.push_str("\n[… list capped; use /glob or /read for more …]");
    }
    out
}

/// Build markdown-style text: tree, multi-language path listing, then key manifests / entry files.
pub fn build_scan(root: &Path) -> Result<String> {
    let base = root
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Workspace path must be UTF-8"))?;

    let mut parts: Vec<String> = Vec::new();
    parts.push(
        "## Auto-loaded project overview (read from disk by Ollero)\n\
         Use this to answer; do not claim you lack access to these paths."
            .to_string(),
    );

    match tools::run_tree(base, 3) {
        Ok(t) => parts.push(format!("### Directory tree (depth ≤3)\n```\n{t}\n```")),
        Err(e) => parts.push(format!("### Directory tree\n_(unavailable: {e})_")),
    }

    let listed = collect_source_paths(base);
    parts.push(format!(
        "### Source-like files (paths only; common extensions; node_modules/target/etc. excluded)\n\
         ```\n{listed}\n```"
    ));

    append_file_if_exists(root, "Cargo.toml", &mut parts);
    append_file_if_exists(root, "package.json", &mut parts);
    append_file_if_exists(root, "go.mod", &mut parts);
    append_file_if_exists(root, "pyproject.toml", &mut parts);
    for name in ["README.md", "readme.md", "Readme.md", "README.TXT"] {
        append_file_if_exists(root, name, &mut parts);
    }
    append_file_if_exists(root, "src/main.rs", &mut parts);
    append_file_if_exists(root, "src/lib.rs", &mut parts);

    let mut out = parts.join("\n\n");
    if out.len() > MAX_SCAN_BYTES {
        truncate_utf8(&mut out, MAX_SCAN_BYTES);
        out.push_str("\n\n[… auto-scan truncated to save context …]");
    }
    Ok(out)
}

fn append_file_if_exists(root: &Path, rel: &str, parts: &mut Vec<String>) {
    let p = root.join(rel);
    if !p.is_file() {
        return;
    }
    let Some(s) = p.to_str() else {
        return;
    };
    match tools::run_read_file(s) {
        Ok(body) => parts.push(format!("### `{rel}`\n```\n{body}\n```")),
        Err(e) => parts.push(format!("### `{rel}`\n_(read error: {e})_")),
    }
}

fn truncate_utf8(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_spanish_user_phrase() {
        let msg = "lee mis archivos y dime cual es el progreso, el estado actual";
        assert!(should_trigger(msg));
    }

    #[test]
    fn trigger_english() {
        assert!(should_trigger("Please read my files and summarize"));
    }

    #[test]
    fn no_trigger_short() {
        assert!(!should_trigger("hi"));
    }

    #[test]
    fn filter_skips_node_modules() {
        assert!(!should_list_path("src/foo/node_modules/pkg/index.js"));
        assert!(!should_list_path(r"C:\app\target\debug\foo.exe"));
        assert!(should_list_path("src/main.rs"));
    }
}
