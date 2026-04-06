use anyhow::Result;

const MAX_RESULTS: usize = 200;

pub fn run_glob(pattern: &str, dir: Option<&str>) -> Result<String> {
    let base = dir.unwrap_or(".");
    let full_pattern = format!("{base}/{pattern}");

    let paths = glob::glob(&full_pattern)
        .map_err(|e| anyhow::anyhow!("Invalid glob pattern '{pattern}': {e}"))?;

    let mut results: Vec<String> = Vec::new();
    for entry in paths {
        match entry {
            Ok(path) => results.push(path.display().to_string()),
            Err(e) => eprintln!("Warning: glob entry error: {e}"),
        }
        if results.len() >= MAX_RESULTS {
            results.push(format!("[... truncated at {MAX_RESULTS} results ...]"));
            break;
        }
    }

    if results.is_empty() {
        return Ok(format!("No files matching '{pattern}'."));
    }

    Ok(results.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_finds_rust_files() {
        let result = run_glob("**/*.rs", Some("src")).unwrap();
        assert!(result.contains(".rs"), "Expected .rs files in result");
    }

    #[test]
    fn test_glob_no_match() {
        let result = run_glob("**/*.xyz_nonexistent", None).unwrap();
        assert!(result.contains("No files"));
    }

    #[test]
    fn test_glob_invalid_pattern() {
        // glob crate is lenient, but verify it doesn't panic
        let result = run_glob("***", None);
        // Either ok or error is fine, just shouldn't panic
        let _ = result;
    }
}
