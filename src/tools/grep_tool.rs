use anyhow::Result;
use std::io::{BufRead, BufReader};

const MAX_MATCHES: usize = 100;

pub fn run_grep(pattern: &str, path: &str, case_insensitive: bool) -> Result<String> {
    let regex = {
        let mut builder = regex::RegexBuilder::new(pattern);
        builder.case_insensitive(case_insensitive);
        builder.build().map_err(|e| anyhow::anyhow!("Invalid regex '{pattern}': {e}"))?
    };

    let root = std::path::Path::new(path);
    let mut matches: Vec<String> = Vec::new();

    search_path(root, &regex, &mut matches)?;

    if matches.is_empty() {
        return Ok(format!("No matches for '{pattern}' in '{path}'."));
    }

    let truncated = matches.len() > MAX_MATCHES;
    matches.truncate(MAX_MATCHES);
    let mut out = matches.join("\n");
    if truncated {
        out.push_str(&format!("\n[... truncated at {MAX_MATCHES} matches ...]"));
    }
    Ok(out)
}

fn search_path(
    path: &std::path::Path,
    regex: &regex::Regex,
    matches: &mut Vec<String>,
) -> Result<()> {
    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let p = entry.path();
            // Skip hidden dirs and common noise dirs
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') || name == "target" || name == "node_modules" {
                    continue;
                }
            }
            search_path(&p, regex, matches)?;
            if matches.len() >= MAX_MATCHES {
                return Ok(());
            }
        }
    } else if path.is_file() {
        // Only search text-like files
        if !is_text_file(path) {
            return Ok(());
        }
        if let Ok(file) = std::fs::File::open(path) {
            let reader = BufReader::new(file);
            for (line_num, line) in reader.lines().enumerate() {
                let Ok(line) = line else { break };
                if regex.is_match(&line) {
                    matches.push(format!("{}:{}: {}", path.display(), line_num + 1, line));
                    if matches.len() >= MAX_MATCHES {
                        return Ok(());
                    }
                }
            }
        }
    }
    Ok(())
}

fn is_text_file(path: &std::path::Path) -> bool {
    let text_extensions = [
        "rs", "toml", "json", "yaml", "yml", "md", "txt", "ts", "js", "py",
        "go", "c", "cpp", "h", "hpp", "java", "kt", "swift", "rb", "sh",
        "html", "css", "xml", "sql", "env", "gitignore", "lock",
    ];
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| text_extensions.contains(&ext))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grep_finds_pattern_in_src() {
        let result = run_grep("fn main", "src", false).unwrap();
        assert!(result.contains("main.rs"), "Expected main.rs in grep results");
    }

    #[test]
    fn test_grep_case_insensitive() {
        let result = run_grep("FN MAIN", "src", true).unwrap();
        assert!(result.contains("main.rs"));
    }

    #[test]
    fn test_grep_no_match() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("sample.txt");
        std::fs::write(&f, "hello\n").unwrap();
        let result = run_grep(
            "XYZZY_NONEXISTENT_TOKEN_12345",
            dir.path().to_str().unwrap(),
            false,
        )
        .unwrap();
        assert!(result.contains("No matches"));
    }

    #[test]
    fn test_grep_invalid_regex() {
        let result = run_grep("[invalid(", "src", false);
        assert!(result.is_err());
    }
}
