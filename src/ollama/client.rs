use anyhow::{Context, Result};
use futures::StreamExt;
use reqwest::Client;

use super::types::{
    ChatOptions, ChatRequest, LlmResponse, Message, ModelInfo, ResponseStats, TagsResponse,
    ToolCallItem, ToolDefinition,
};

pub struct OllamaClient {
    http: Client,
    base_url: String,
    pub model: String,
}

impl OllamaClient {
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            http: Client::new(),
            base_url: base_url.into(),
            model: model.into(),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Send a chat request and stream the response.
    /// - Calls `on_chunk` for each text delta (empty string if tool call).
    /// - Returns `LlmResponse::Text` or `LlmResponse::ToolCalls` when done.
    pub async fn chat<F>(
        &self,
        messages: &[Message],
        tools: Option<&[ToolDefinition]>,
        options: Option<ChatOptions>,
        mut on_chunk: F,
    ) -> Result<LlmResponse>
    where
        F: FnMut(&str),
    {
        let request = ChatRequest {
            model: &self.model,
            messages,
            stream: true,
            tools,
            options,
        };

        let response = self
            .http
            .post(format!("{}/api/chat", self.base_url))
            .json(&request)
            .send()
            .await
            .context("Failed to connect to Ollama. Is it running?")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let msg = format!("Ollama returned {status}: {body}");
            #[cfg(debug_assertions)]
            let msg = {
                let mut m = msg;
                if body.contains("does not support tools") {
                    m.push_str(
                        "\n\nThis model cannot use tools in Ollama. Try: ollama pull llama3.2 \
                         (or llama3.1, qwen2.5, mistral — see https://ollama.com/search?c=tools), \
                         then /model <name> in Ollero.",
                    );
                }
                m
            };
            anyhow::bail!("{msg}");
        }

        let mut stream = response.bytes_stream();
        let mut text_buf = String::new();
        let mut tool_calls: Vec<ToolCallItem> = Vec::new();
        let mut stats = ResponseStats::default();
        let mut raw_buf: Vec<u8> = Vec::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("Stream error")?;
            raw_buf.extend_from_slice(&chunk);

            while let Some(pos) = raw_buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = raw_buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line);
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                match serde_json::from_str::<super::types::ChatChunk>(line) {
                    Ok(parsed) => {
                        // Accumulate tool calls
                        tool_calls.extend(parsed.message.tool_calls);

                        // Stream text
                        if !parsed.message.content.is_empty() {
                            on_chunk(&parsed.message.content);
                            text_buf.push_str(&parsed.message.content);
                        }

                        if parsed.done {
                            stats.prompt_tokens = parsed.prompt_eval_count;
                            stats.completion_tokens = parsed.eval_count;
                        }
                    }
                    Err(e) => {
                        eprintln!("Warning: failed to parse chunk: {e}");
                    }
                }
            }
        }

        if !tool_calls.is_empty() {
            Ok(LlmResponse::ToolCalls { calls: tool_calls, stats })
        } else {
            Ok(LlmResponse::Text { content: text_buf, stats })
        }
    }

    /// List all locally available models.
    pub async fn list_models(base_url: &str) -> Result<Vec<ModelInfo>> {
        let client = Client::new();
        let resp = client
            .get(format!("{base_url}/api/tags"))
            .send()
            .await
            .context("Cannot reach Ollama. Is it running on port 11434?")?;
        let tags: TagsResponse = resp.json().await.context("Failed to parse Ollama model list")?;
        Ok(tags.models)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ollama::types::Message;

    const OLLAMA_URL: &str = "http://localhost:11434";

    async fn ollama_available() -> bool {
        Client::new().get(format!("{OLLAMA_URL}/api/tags")).send().await.is_ok()
    }

    async fn first_model() -> Option<String> {
        OllamaClient::list_models(OLLAMA_URL).await.ok()?.into_iter().next().map(|m| m.name)
    }

    #[tokio::test]
    async fn test_list_models_returns_vec() {
        if !ollama_available().await {
            eprintln!("SKIP: Ollama not running");
            return;
        }
        let models = OllamaClient::list_models(OLLAMA_URL).await.unwrap();
        assert!(!models.is_empty());
        for m in &models {
            assert!(!m.name.is_empty());
        }
    }

    #[tokio::test]
    async fn test_list_models_bad_url_returns_error() {
        let result = OllamaClient::list_models("http://localhost:1").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_chat_bad_model_returns_error() {
        if !ollama_available().await {
            eprintln!("SKIP: Ollama not running");
            return;
        }
        let client = OllamaClient::new(OLLAMA_URL, "nonexistent-model-xyz:latest");
        let messages = vec![Message::user("hi")];
        let result = client.chat(&messages, None, None, |_| {}).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_chat_returns_text_response() {
        if !ollama_available().await {
            eprintln!("SKIP: Ollama not running");
            return;
        }
        let model = match first_model().await {
            Some(m) => m,
            None => return,
        };
        let client = OllamaClient::new(OLLAMA_URL, &model);
        let messages = vec![
            Message::system("Reply with exactly the word: OK"),
            Message::user("Say OK"),
        ];
        let mut output = String::new();
        let result = client.chat(&messages, None, None, |c| output.push_str(c)).await.unwrap();
        assert!(!output.is_empty());
        assert!(matches!(result, LlmResponse::Text { .. }));
        if let LlmResponse::Text { stats, .. } = result {
            assert!(stats.completion_tokens > 0);
        }
    }
}
