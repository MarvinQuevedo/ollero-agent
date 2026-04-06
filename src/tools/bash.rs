use anyhow::Result;

const MAX_OUTPUT_BYTES: usize = 20_000;
const TIMEOUT_SECS: u64 = 30;

pub async fn run_bash(command: &str) -> Result<String> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(TIMEOUT_SECS),
        tokio::process::Command::new(shell())
            .args(shell_args(command))
            .output(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Command timed out after {TIMEOUT_SECS}s: {command}"))?
    .map_err(|e| anyhow::anyhow!("Failed to spawn command: {e}"))?;

    let stdout = truncate(&output.stdout, MAX_OUTPUT_BYTES);
    let stderr = truncate(&output.stderr, MAX_OUTPUT_BYTES);
    let exit_code = output.status.code().unwrap_or(-1);

    let mut result = String::new();

    if !stdout.is_empty() {
        result.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str("[stderr]\n");
        result.push_str(&stderr);
    }
    if exit_code != 0 {
        result.push_str(&format!("\n[exit code: {exit_code}]"));
    }

    if result.is_empty() {
        result = "(no output)".into();
    }

    Ok(result)
}

fn truncate(bytes: &[u8], max: usize) -> String {
    let s = String::from_utf8_lossy(bytes).into_owned();
    if s.len() > max {
        format!("{}\n[... truncated]", &s[..max])
    } else {
        s
    }
}

#[cfg(target_os = "windows")]
fn shell() -> &'static str {
    "cmd"
}

#[cfg(not(target_os = "windows"))]
fn shell() -> &'static str {
    "sh"
}

#[cfg(target_os = "windows")]
fn shell_args(command: &str) -> Vec<&str> {
    vec!["/C", command]
}

#[cfg(not(target_os = "windows"))]
fn shell_args(command: &str) -> Vec<&str> {
    vec!["-c", command]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_bash_echo() {
        let result = run_bash("echo hello").await.unwrap();
        assert!(result.trim().contains("hello"));
    }

    #[tokio::test]
    async fn test_bash_exit_code_on_failure() {
        let result = run_bash("exit 1").await.unwrap();
        assert!(result.contains("exit code: 1"));
    }

    #[tokio::test]
    async fn test_bash_captures_stderr() {
        #[cfg(not(target_os = "windows"))]
        {
            let result = run_bash("echo error >&2").await.unwrap();
            assert!(result.contains("error") || result.contains("stderr"));
        }
    }
}
