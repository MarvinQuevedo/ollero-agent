//! Token compression utilities for reducing context window usage.
//!
//! All strategies are lossless or near-lossless: they preserve semantic meaning
//! while reducing character/token count. No external dependencies required.

use std::fmt;

/// When compression is applied during the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionMode {
    /// Always compress every tool output and message before storing in history.
    Always,
    /// Compress only when the context window approaches its budget limit.
    Auto,
    /// Never compress automatically; user triggers compression manually via `/compress`.
    Manual,
}

impl CompressionMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Always  => "always",
            Self::Auto    => "auto",
            Self::Manual  => "manual",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::Always  => "compress all tool outputs and messages immediately",
            Self::Auto    => "compress only when approaching context window limit",
            Self::Manual  => "no automatic compression; use /compress to trigger",
        }
    }

    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "always" | "on"  => Some(Self::Always),
            "auto"           => Some(Self::Auto),
            "manual" | "off" => Some(Self::Manual),
            _ => None,
        }
    }
}

impl fmt::Display for CompressionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
    }
}

/// Compression level that determines which strategies to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionLevel {
    /// Light: only strip ANSI codes, trailing whitespace, collapse blank lines.
    Light,
    /// Standard: light + deduplicate consecutive lines + compact JSON + compact line numbers.
    Standard,
    /// Aggressive: standard + smart truncation (keep head+tail, drop repetitive middle).
    Aggressive,
}

/// Result of a compression operation with before/after stats.
#[derive(Debug)]
pub struct CompressResult {
    pub text: String,
    pub original_len: usize,
    pub compressed_len: usize,
}

impl CompressResult {
    pub fn ratio(&self) -> f64 {
        if self.original_len == 0 {
            return 1.0;
        }
        self.compressed_len as f64 / self.original_len as f64
    }
}

/// Apply compression to tool output based on the tool name and level.
pub fn compress_tool_output(tool_name: &str, output: &str, level: CompressionLevel) -> CompressResult {
    let original_len = output.len();

    if output.is_empty() || original_len < 100 {
        return CompressResult {
            text: output.to_string(),
            original_len,
            compressed_len: output.len(),
        };
    }

    let text = match level {
        CompressionLevel::Light => compress_light(output),
        CompressionLevel::Standard => compress_standard(tool_name, output),
        CompressionLevel::Aggressive => compress_aggressive(tool_name, output),
    };

    let compressed_len = text.len();
    CompressResult { text, original_len, compressed_len }
}

/// Compress a generic message (assistant/user) for history compaction.
pub fn compress_message(content: &str, level: CompressionLevel) -> String {
    if content.len() < 100 {
        return content.to_string();
    }
    match level {
        CompressionLevel::Light => compress_light(content),
        CompressionLevel::Standard => compress_standard("message", content),
        CompressionLevel::Aggressive => compress_aggressive("message", content),
    }
}

// ── Light compression ─────────────────────────────────────────────────────

fn compress_light(text: &str) -> String {
    let text = strip_ansi_codes(text);
    let text = collapse_blank_lines(&text);
    trim_trailing_whitespace(&text)
}

// ── Standard compression ──────────────────────────────────────────────────

fn compress_standard(tool_name: &str, text: &str) -> String {
    let mut text = compress_light(text);

    // Tool-specific compressions
    match tool_name {
        "bash" => {
            text = deduplicate_consecutive_lines(&text, 3);
            text = compact_json_in_text(&text);
        }
        "read_file" => {
            text = compact_line_number_prefix(&text);
        }
        "grep" => {
            text = deduplicate_consecutive_lines(&text, 2);
        }
        "glob" | "tree" => {
            // Already compact, just light compression
        }
        _ => {
            text = compact_json_in_text(&text);
            text = deduplicate_consecutive_lines(&text, 3);
        }
    }

    text
}

// ── Aggressive compression ────────────────────────────────────────────────

fn compress_aggressive(tool_name: &str, text: &str) -> String {
    let mut text = compress_standard(tool_name, text);

    // Smart truncation: if still very large, keep head + tail
    const AGGRESSIVE_THRESHOLD: usize = 8_000;
    if text.len() > AGGRESSIVE_THRESHOLD {
        text = smart_truncate(&text, AGGRESSIVE_THRESHOLD);
    }

    text
}

// ── Individual strategies ────────────────────────────────────────────────

/// Strip ANSI escape codes (colors, cursor movement, etc.)
fn strip_ansi_codes(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip ESC [ ... final_byte sequences (CSI)
            if chars.peek() == Some(&'[') {
                chars.next(); // consume '['
                // consume until we hit a letter (0x40-0x7E)
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if next.is_ascii_alphabetic() || next == '~' || next == '@' {
                        break;
                    }
                }
            } else if chars.peek() == Some(&']') {
                // OSC sequence: ESC ] ... ST (or BEL)
                chars.next();
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if next == '\x07' || next == '\\' {
                        break;
                    }
                }
            }
            // else: skip standalone ESC
        } else {
            result.push(c);
        }
    }

    result
}

/// Collapse runs of 3+ blank lines into a single blank line.
fn collapse_blank_lines(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut consecutive_blanks = 0u32;

    for line in text.lines() {
        if line.trim().is_empty() {
            consecutive_blanks += 1;
            if consecutive_blanks <= 1 {
                result.push('\n');
            }
        } else {
            consecutive_blanks = 0;
            result.push_str(line);
            result.push('\n');
        }
    }

    // Remove trailing newline if the original didn't have one
    if !text.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }

    result
}

/// Remove trailing whitespace from each line.
fn trim_trailing_whitespace(text: &str) -> String {
    text.lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Deduplicate consecutive identical lines, replacing with a count marker.
/// `threshold` = minimum repetitions before deduplication kicks in.
fn deduplicate_consecutive_lines(text: &str, threshold: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < threshold {
        return text.to_string();
    }

    let mut result = String::with_capacity(text.len());
    let mut i = 0;

    while i < lines.len() {
        let current = lines[i];
        let mut count = 1usize;

        // Count consecutive identical lines
        while i + count < lines.len() && lines[i + count] == current {
            count += 1;
        }

        if count >= threshold {
            result.push_str(current);
            result.push('\n');
            result.push_str(&format!("[... repeated {} more times ...]\n", count - 1));
            i += count;
        } else {
            for _ in 0..count {
                result.push_str(current);
                result.push('\n');
            }
            i += count;
        }
    }

    if !text.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }

    result
}

/// Compact line number prefixes in read_file output.
/// Converts "   1 | code" to "1|code" (saves ~4 chars per line).
fn compact_line_number_prefix(text: &str) -> String {
    let mut result = String::with_capacity(text.len());

    for line in text.lines() {
        // Match pattern: optional whitespace, digits, " | ", content
        if let Some(pipe_pos) = line.find(" | ") {
            let prefix = &line[..pipe_pos];
            if prefix.trim().chars().all(|c| c.is_ascii_digit()) {
                let num = prefix.trim();
                let content = &line[pipe_pos + 3..]; // skip " | "
                result.push_str(num);
                result.push('|');
                result.push_str(content);
                result.push('\n');
                continue;
            }
        }
        result.push_str(line);
        result.push('\n');
    }

    if !text.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }

    result
}

/// Detect and compact inline JSON objects/arrays in text.
/// Removes unnecessary whitespace from JSON-like structures.
fn compact_json_in_text(text: &str) -> String {
    // Try to parse the entire text as JSON first
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        // If the whole thing is JSON, compact it
        if let Ok(compact) = serde_json::to_string(&value) {
            if compact.len() < text.len() {
                return compact;
            }
        }
    }

    // Otherwise, look for JSON blocks within the text
    let mut result = String::with_capacity(text.len());
    let mut in_json = false;
    let mut json_buf = String::new();
    let mut brace_depth = 0i32;
    let mut bracket_depth = 0i32;

    for line in text.lines() {
        let trimmed = line.trim();

        if !in_json {
            // Detect start of a JSON block
            if (trimmed.starts_with('{') || trimmed.starts_with('['))
                && !trimmed.starts_with("{...")
            {
                in_json = true;
                json_buf.clear();
                brace_depth = 0;
                bracket_depth = 0;
            }
        }

        if in_json {
            json_buf.push_str(line);
            json_buf.push('\n');

            for c in trimmed.chars() {
                match c {
                    '{' => brace_depth += 1,
                    '}' => brace_depth -= 1,
                    '[' => bracket_depth += 1,
                    ']' => bracket_depth -= 1,
                    _ => {}
                }
            }

            if brace_depth <= 0 && bracket_depth <= 0 {
                // Try to compact the JSON block
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(json_buf.trim()) {
                    if let Ok(compact) = serde_json::to_string(&value) {
                        result.push_str(&compact);
                        result.push('\n');
                    } else {
                        result.push_str(&json_buf);
                    }
                } else {
                    result.push_str(&json_buf);
                }
                in_json = false;
                json_buf.clear();
            }
        } else {
            result.push_str(line);
            result.push('\n');
        }
    }

    // If we were still in a JSON block, flush it
    if !json_buf.is_empty() {
        result.push_str(&json_buf);
    }

    if !text.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }

    result
}

/// Smart truncation: keep the first N and last M chars, drop the middle.
/// Preserves the head (context) and tail (most recent/relevant output).
fn smart_truncate(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }

    // 60% head, 40% tail
    let head_budget = (max_chars * 60) / 100;
    let tail_budget = max_chars - head_budget - 80; // 80 chars for the marker

    let head = safe_slice(text, 0, head_budget);
    let tail = safe_slice_end(text, tail_budget);

    let omitted = text.len() - head.len() - tail.len();
    // Estimate tokens omitted (~4 chars per token)
    let tokens_est = omitted / 4;

    format!(
        "{}\n\n[... {} chars (~{} tokens) omitted for context budget ...]\n\n{}",
        head, omitted, tokens_est, tail
    )
}

/// Get a safe UTF-8 slice from the start of a string.
fn safe_slice(s: &str, start: usize, max_len: usize) -> &str {
    let end = (start + max_len).min(s.len());
    let mut idx = end;
    while idx > start && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    // Try to break at a newline for cleaner cuts
    if let Some(nl_pos) = s[start..idx].rfind('\n') {
        &s[start..start + nl_pos]
    } else {
        &s[start..idx]
    }
}

/// Get a safe UTF-8 slice from the end of a string.
fn safe_slice_end(s: &str, max_len: usize) -> &str {
    if max_len >= s.len() {
        return s;
    }
    let start = s.len() - max_len;
    let mut idx = start;
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    // Try to break at a newline for cleaner cuts
    if let Some(nl_pos) = s[idx..].find('\n') {
        &s[idx + nl_pos + 1..]
    } else {
        &s[idx..]
    }
}

// ── Public helpers for history management ─────────────────────────────────

/// Estimate token count from character length (~4 chars per token for English/code).
pub fn estimate_tokens(text: &str) -> usize {
    (text.len() + 3) / 4
}

/// Estimate token count from a character count.
pub fn estimate_tokens_from_chars(chars: usize) -> usize {
    (chars + 3) / 4
}

/// Build a concise summary of evicted messages for history compaction.
pub fn build_eviction_summary(messages: &[(String, usize)]) -> String {
    // messages = Vec<(role, content_len)>
    let total: usize = messages.len();
    let mut user_count = 0usize;
    let mut assistant_count = 0usize;
    let mut tool_count = 0usize;

    for (role, _) in messages {
        match role.as_str() {
            "user" => user_count += 1,
            "assistant" => assistant_count += 1,
            "tool" => tool_count += 1,
            _ => {}
        }
    }

    let total_chars: usize = messages.iter().map(|(_, len)| len).sum();
    let tokens_est = total_chars / 4;

    let mut parts = Vec::new();
    if user_count > 0 {
        parts.push(format!("{user_count} user"));
    }
    if assistant_count > 0 {
        parts.push(format!("{assistant_count} assistant"));
    }
    if tool_count > 0 {
        parts.push(format!("{tool_count} tool"));
    }

    format!(
        "[Context compressed: {total} messages evicted ({}) — ~{tokens_est} tokens freed. \
         The conversation continues from the most recent messages below.]",
        parts.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_ansi_codes() {
        let input = "\x1b[31mred text\x1b[0m normal";
        assert_eq!(strip_ansi_codes(input), "red text normal");
    }

    #[test]
    fn test_strip_ansi_complex() {
        let input = "\x1b[1;32m✓\x1b[0m test passed\x1b[38;2;100;200;50m colored\x1b[0m";
        let result = strip_ansi_codes(input);
        assert!(!result.contains('\x1b'));
        assert!(result.contains("✓"));
        assert!(result.contains("test passed"));
        assert!(result.contains("colored"));
    }

    #[test]
    fn test_collapse_blank_lines() {
        let input = "line1\n\n\n\n\nline2\n\nline3";
        let result = collapse_blank_lines(input);
        assert_eq!(result, "line1\n\nline2\n\nline3");
    }

    #[test]
    fn test_deduplicate_consecutive() {
        let input = "header\nok\nok\nok\nok\nfooter";
        let result = deduplicate_consecutive_lines(input, 3);
        assert!(result.contains("ok"));
        assert!(result.contains("[... repeated 3 more times ...]"));
        assert!(result.contains("footer"));
    }

    #[test]
    fn test_deduplicate_below_threshold() {
        let input = "a\na\nb";
        let result = deduplicate_consecutive_lines(input, 3);
        assert_eq!(result, "a\na\nb");
    }

    #[test]
    fn test_compact_line_numbers() {
        let input = "   1 | fn main() {\n   2 |     println!(\"hello\");\n   3 | }";
        let result = compact_line_number_prefix(input);
        assert!(result.contains("1|fn main() {"));
        assert!(result.contains("2|    println!(\"hello\");"));
        assert!(result.contains("3|}"));
    }

    #[test]
    fn test_compact_json() {
        let input = "{\n  \"name\": \"test\",\n  \"value\": 42\n}";
        let result = compact_json_in_text(input);
        assert!(result.len() < input.len());
        assert!(result.contains("\"name\":\"test\"") || result.contains("\"name\": \"test\""));
    }

    #[test]
    fn test_smart_truncate() {
        let input = "a".repeat(10_000);
        let result = smart_truncate(&input, 1_000);
        assert!(result.len() < 1_200); // some overhead for marker
        assert!(result.contains("omitted for context budget"));
    }

    #[test]
    fn test_compress_tool_output_small() {
        let result = compress_tool_output("bash", "ok", CompressionLevel::Standard);
        assert_eq!(result.text, "ok");
        assert_eq!(result.ratio(), 1.0);
    }

    #[test]
    fn test_compress_tool_output_with_ansi() {
        let input = format!("\x1b[32m{}\x1b[0m", "x".repeat(200));
        let result = compress_tool_output("bash", &input, CompressionLevel::Light);
        assert!(!result.text.contains('\x1b'));
        assert!(result.compressed_len < result.original_len);
    }

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens("hello world!"), 3); // 12 chars / 4
    }

    #[test]
    fn test_build_eviction_summary() {
        let messages = vec![
            ("user".to_string(), 100),
            ("assistant".to_string(), 500),
            ("tool".to_string(), 200),
            ("tool".to_string(), 300),
        ];
        let summary = build_eviction_summary(&messages);
        assert!(summary.contains("4 messages evicted"));
        assert!(summary.contains("1 user"));
        assert!(summary.contains("1 assistant"));
        assert!(summary.contains("2 tool"));
    }

    #[test]
    fn test_trim_trailing_whitespace() {
        let input = "hello   \nworld  \n  foo  ";
        let result = trim_trailing_whitespace(input);
        assert_eq!(result, "hello\nworld\n  foo");
    }

    #[test]
    fn test_compress_preserves_short_content() {
        let input = "short";
        let result = compress_tool_output("bash", input, CompressionLevel::Aggressive);
        assert_eq!(result.text, "short");
    }

    #[test]
    fn test_full_pipeline_bash() {
        let input = format!(
            "\x1b[32m✓\x1b[0m Compiling...\n{}\nDone!\n\n\n\n\nFinished.",
            "warning: unused variable\n".repeat(50)
        );
        let result = compress_tool_output("bash", &input, CompressionLevel::Standard);
        assert!(result.compressed_len < result.original_len);
        assert!(result.text.contains("Done!"));
        assert!(result.text.contains("Finished."));
    }
}
