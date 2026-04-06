use anyhow::Result;

const MAX_LINES: usize = 500;

pub fn run_read_file(path: &str) -> Result<String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Cannot read '{path}': {e}"))?;

    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let truncated = total > MAX_LINES;
    let shown = lines.len().min(MAX_LINES);

    let mut out = String::new();
    for (i, line) in lines.iter().take(shown).enumerate() {
        out.push_str(&format!("{:>4} | {}\n", i + 1, line));
    }

    if truncated {
        out.push_str(&format!(
            "\n[... truncated: showing {shown}/{total} lines ...]"
        ));
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_read_file_with_line_numbers() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "line one").unwrap();
        writeln!(f, "line two").unwrap();

        let out = run_read_file(f.path().to_str().unwrap()).unwrap();
        assert!(out.contains("   1 | line one"));
        assert!(out.contains("   2 | line two"));
    }

    #[test]
    fn test_read_file_not_found() {
        let result = run_read_file("/tmp/ollero_nonexistent_xyz.txt");
        assert!(result.is_err());
    }
}
