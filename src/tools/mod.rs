mod bash;
mod edit_file;
mod glob_tool;
mod grep_tool;
mod read_file;
mod tree;
mod write_file;

use anyhow::Result;
use serde_json::json;

use crate::ollama::types::ToolDefinition;

pub use bash::run_bash;
pub use edit_file::run_edit_file;
pub use glob_tool::run_glob;
pub use grep_tool::run_grep;
pub use read_file::run_read_file;
pub use tree::run_tree;
pub use write_file::run_write_file;

/// Dispatch a tool call by name with its JSON arguments.
pub async fn dispatch(name: &str, args: &serde_json::Value) -> Result<String> {
    let result = match name {
        "read_file" => {
            let path = require_str(args, "path")?;
            run_read_file(path)
        }
        "write_file" => {
            let path = require_str(args, "path")?;
            let content = require_str(args, "content")?;
            run_write_file(path, content)
        }
        "edit_file" => {
            let path = require_str(args, "path")?;
            let old_str = require_str(args, "old_str")?;
            let new_str = require_str(args, "new_str")?;
            run_edit_file(path, old_str, new_str)
        }
        "glob" => {
            let pattern = require_str(args, "pattern")?;
            let dir = args["dir"].as_str();
            run_glob(pattern, dir)
        }
        "grep" => {
            let pattern = require_str(args, "pattern")?;
            let path = args["path"].as_str().unwrap_or(".");
            let case_insensitive = args["case_insensitive"].as_bool().unwrap_or(false);
            run_grep(pattern, path, case_insensitive)
        }
        "tree" => {
            let path = args["path"].as_str().unwrap_or(".");
            let depth = args["depth"].as_u64().unwrap_or(3) as usize;
            run_tree(path, depth)
        }
        "bash" => {
            let command = require_str(args, "command")?;
            run_bash(command).await
        }
        unknown => Err(anyhow::anyhow!("Unknown tool: {unknown}")),
    }?;

    Ok(result)
}

fn require_str<'a>(args: &'a serde_json::Value, key: &str) -> Result<&'a str> {
    args[key]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing required argument: {key}"))
}

/// All tool definitions to include in every LLM request.
pub fn all_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition::function(
            "read_file",
            "Read the full contents of a file. Returns file content with line numbers.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file to read" }
                },
                "required": ["path"]
            }),
        ),
        ToolDefinition::function(
            "write_file",
            "Create or overwrite a file with the given content.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to write" },
                    "content": { "type": "string", "description": "Content to write" }
                },
                "required": ["path", "content"]
            }),
        ),
        ToolDefinition::function(
            "edit_file",
            "Replace an exact string in a file with a new string. old_str must match exactly.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file" },
                    "old_str": { "type": "string", "description": "Exact string to replace" },
                    "new_str": { "type": "string", "description": "Replacement string" }
                },
                "required": ["path", "old_str", "new_str"]
            }),
        ),
        ToolDefinition::function(
            "glob",
            "Find files matching a glob pattern (e.g. '**/*.rs'). Returns matching paths.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern" },
                    "dir": { "type": "string", "description": "Base directory (default: current dir)" }
                },
                "required": ["pattern"]
            }),
        ),
        ToolDefinition::function(
            "grep",
            "Search for a regex pattern in files. Returns matching lines with file:line context.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Regex pattern to search" },
                    "path": { "type": "string", "description": "File or directory to search (default: current dir)" },
                    "case_insensitive": { "type": "boolean", "description": "Case-insensitive search" }
                },
                "required": ["pattern"]
            }),
        ),
        ToolDefinition::function(
            "tree",
            "Show the directory tree structure.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Root path (default: current dir)" },
                    "depth": { "type": "integer", "description": "Max depth (default: 3)" }
                }
            }),
        ),
        ToolDefinition::function(
            "bash",
            "Execute a shell command and return its output. Use for builds, tests, git, etc.",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to execute" }
                },
                "required": ["command"]
            }),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_definitions_have_names() {
        let defs = all_definitions();
        assert!(!defs.is_empty());
        for d in &defs {
            assert!(!d.function.name.is_empty());
            assert!(!d.function.description.is_empty());
        }
    }

    #[tokio::test]
    async fn test_dispatch_unknown_tool() {
        let result = dispatch("nonexistent_tool", &json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_dispatch_read_missing_arg() {
        let result = dispatch("read_file", &json!({})).await;
        assert!(result.is_err());
    }
}
