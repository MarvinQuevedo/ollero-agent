use anyhow::Result;

pub fn run_write_file(path: &str, content: &str) -> Result<String> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("Cannot create directories for '{path}': {e}"))?;
        }
    }
    std::fs::write(path, content)
        .map_err(|e| anyhow::anyhow!("Cannot write '{path}': {e}"))?;

    let lines = content.lines().count();
    Ok(format!("Written {lines} lines to '{path}'."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_and_verify() {
        let path = std::env::temp_dir().join("ollero_write_test.txt");
        let path_str = path.to_str().unwrap();

        let result = run_write_file(path_str, "hello\nworld\n").unwrap();
        assert!(result.contains("2 lines"));
        assert_eq!(std::fs::read_to_string(path_str).unwrap(), "hello\nworld\n");

        std::fs::remove_file(path_str).unwrap();
    }
}
