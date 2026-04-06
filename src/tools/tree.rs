use anyhow::Result;

const MAX_ENTRIES: usize = 300;

pub fn run_tree(path: &str, depth: usize) -> Result<String> {
    let root = std::path::Path::new(path);
    if !root.exists() {
        anyhow::bail!("Path '{path}' does not exist.");
    }

    let mut out = String::new();
    let mut count = 0;
    out.push_str(&format!("{path}\n"));
    build_tree(root, "", depth, 0, &mut out, &mut count);

    if count >= MAX_ENTRIES {
        out.push_str("... [truncated]\n");
    }

    Ok(out)
}

fn build_tree(
    dir: &std::path::Path,
    prefix: &str,
    max_depth: usize,
    current_depth: usize,
    out: &mut String,
    count: &mut usize,
) {
    if current_depth >= max_depth || *count >= MAX_ENTRIES {
        return;
    }

    let Ok(mut entries) = std::fs::read_dir(dir) else { return };

    let mut items: Vec<std::path::PathBuf> = Vec::new();
    while let Some(Ok(entry)) = entries.next() {
        let p = entry.path();
        if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
        }
        items.push(p);
    }
    items.sort();

    let len = items.len();
    for (i, path) in items.iter().enumerate() {
        if *count >= MAX_ENTRIES {
            break;
        }
        let is_last = i == len - 1;
        let connector = if is_last { "└── " } else { "├── " };
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");

        out.push_str(&format!("{prefix}{connector}{name}\n"));
        *count += 1;

        if path.is_dir() {
            let new_prefix = format!("{}{}", prefix, if is_last { "    " } else { "│   " });
            build_tree(path, &new_prefix, max_depth, current_depth + 1, out, count);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tree_shows_src() {
        let result = run_tree(".", 2).unwrap();
        assert!(result.contains("src"));
        assert!(result.contains("Cargo.toml"));
    }

    #[test]
    fn test_tree_nonexistent_path() {
        let result = run_tree("/nonexistent/path/xyz", 2);
        assert!(result.is_err());
    }

    #[test]
    fn test_tree_depth_limits() {
        let shallow = run_tree(".", 1).unwrap();
        let deep = run_tree(".", 3).unwrap();
        // Deeper should have more content
        assert!(deep.len() >= shallow.len());
    }
}
