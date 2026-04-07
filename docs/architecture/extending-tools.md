---
layout: page
title: Extending Tools
nav_order: 4
---

# Extending the Tool System

Adding a new tool to Allux involves three main steps: implementing the logic, registering the tool in the dispatcher, and defining its interface for the LLM.

## 1. Implement the Tool Logic

Create a new module in `src/tools/`. For example, if you want to add a `list_directory` tool:

`src/tools/list_dir.rs`
```rust
use anyhow::Result;
use std/fs;

pub fn run_list_dir(path: &str) -> Result<String> {
    let entries = fs::read_dir(path)?
        .map(|res| res.map(|e| e.file_name().into_string().unwrap_or_default()))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(entries.join("\n"))
}
```

## 2. Register the Tool in the Dispatcher

Update `src/tools/mod.rs` to include the new module and add the logic to the `dispatch` function.

```rust
// 1. Add the module declaration
mod list_dir;
pub use list_dir::run_list_dir;

// 2. Update the dispatch function
pub async fn dispatch(name: &str, args: &serde_json::Value) -> Result<String> {
    let result = match name {
        // ... existing tools
        "list_dir" => {
            let path = args["path"].as_str().unwrap_or(".");
            run_list_dir(path)
        }
        // ...
    }?;
    Ok(result)
}
```

## 3. Define the JSON Schema for the LLM

Finally, add the `ToolDefinition` to the `all_definitions()` function in `src/tools/mod.rs`. This is what tells the LLM (Ollama) how to format its request.

```rust
ToolDefinition::function(
    "list_dir",
    "List the files in a directory.",
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Path to list" }
        },
        "required": ["path"]
    }),
),
```

## Summary of Workflow

| Step | File | Action |
|:---|:---|:---|
| **Logic** | `src/tools/new_tool.rs` | Implement the Rust function. |
| **Dispatch** | `src/tools/mod.rs` | Add `mod` and update `match` block. |
| **Interface** | `src/tools/mod.rs` | Add `ToolDefinition` to `all_definitions()`. |
