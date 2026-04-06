use serde::{Deserialize, Serialize};

/// A message in the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallItem>>,
    /// Required by Ollama for `role: "tool"` messages (pairs result with `tool_calls[].function.name`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
            tool_calls: None,
            tool_name: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
            tool_calls: None,
            tool_name: None,
        }
    }

    pub fn assistant_tool_calls(calls: Vec<ToolCallItem>) -> Self {
        Self {
            role: "assistant".into(),
            content: String::new(),
            tool_calls: Some(calls),
            tool_name: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
            tool_calls: None,
            tool_name: None,
        }
    }

    /// Tool result message sent back to the LLM (must include `tool_name` per Ollama API).
    pub fn tool_result(tool_name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".into(),
            content: content.into(),
            tool_calls: None,
            tool_name: Some(tool_name.into()),
        }
    }
}

// ── Tool call types (in LLM response) ────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolCallItem {
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: serde_json::Value,
}

// ── Tool definition (sent in request) ────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub kind: &'static str, // always "function"
    pub function: FunctionDefinition,
}

#[derive(Debug, Serialize)]
pub struct FunctionDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: serde_json::Value,
}

impl ToolDefinition {
    pub fn function(name: &'static str, description: &'static str, parameters: serde_json::Value) -> Self {
        Self {
            kind: "function",
            function: FunctionDefinition { name, description, parameters },
        }
    }
}

// ── Request / Response ────────────────────────────────────────────────────────

/// Request body for POST /api/chat
#[derive(Debug, Serialize)]
pub struct ChatRequest<'a> {
    pub model: &'a str,
    pub messages: &'a [Message],
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<&'a [ToolDefinition]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<ChatOptions>,
}

/// Ollama model options
#[derive(Debug, Serialize, Clone)]
pub struct ChatOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_ctx: Option<u32>,
}

/// One chunk from the streaming response
#[derive(Debug, Deserialize)]
pub struct ChatChunk {
    pub message: ChunkMessage,
    pub done: bool,
    #[serde(default)]
    pub eval_count: u32,
    #[serde(default)]
    pub prompt_eval_count: u32,
}

#[derive(Debug, Deserialize)]
pub struct ChunkMessage {
    pub content: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCallItem>,
}

/// Result of a completed LLM turn.
pub enum LlmResponse {
    /// The model returned text.
    Text { content: String, stats: ResponseStats },
    /// The model wants to call tools.
    ToolCalls { calls: Vec<ToolCallItem>, stats: ResponseStats },
}

/// Aggregated stats from a completed response
#[derive(Debug, Default)]
pub struct ResponseStats {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

impl ResponseStats {
    pub fn total(&self) -> u32 {
        self.prompt_tokens + self.completion_tokens
    }
}

/// Response from GET /api/tags
#[derive(Debug, Deserialize)]
pub struct TagsResponse {
    pub models: Vec<ModelInfo>,
}

#[derive(Debug, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub details: ModelDetails,
}

#[derive(Debug, Deserialize)]
pub struct ModelDetails {
    pub parameter_size: String,
    pub quantization_level: String,
}
