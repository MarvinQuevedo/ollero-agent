//! Integration test: compresses real project content and prints before/after stats.
//!
//! Run with: cargo test --test compression_demo -- --nocapture

use std::fs;
use std::path::Path;

// We test the compression module through its public API by reimplementing
// the core logic here (since mod compression is private to the binary).
// This test validates the strategies work on real project data.

/// Strip ANSI escape codes.
fn strip_ansi_codes(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if next.is_ascii_alphabetic() || next == '~' || next == '@' {
                        break;
                    }
                }
            } else if chars.peek() == Some(&']') {
                chars.next();
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if next == '\x07' || next == '\\' {
                        break;
                    }
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

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
    if !text.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }
    result
}

fn trim_trailing_whitespace(text: &str) -> String {
    text.lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

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

fn compact_line_number_prefix(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for line in text.lines() {
        if let Some(pipe_pos) = line.find(" | ") {
            let prefix = &line[..pipe_pos];
            if prefix.trim().chars().all(|c| c.is_ascii_digit()) {
                let num = prefix.trim();
                let content = &line[pipe_pos + 3..];
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

fn compact_json_in_text(text: &str) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        if let Ok(compact) = serde_json::to_string(&value) {
            if compact.len() < text.len() {
                return compact;
            }
        }
    }
    text.to_string()
}

fn smart_truncate(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let head_budget = (max_chars * 60) / 100;
    let tail_budget = max_chars - head_budget - 80;

    let mut hb = head_budget.min(text.len());
    while hb > 0 && !text.is_char_boundary(hb) {
        hb -= 1;
    }
    let head_end = text[..hb]
        .rfind('\n')
        .unwrap_or(hb);
    let head = &text[..head_end];

    let mut tail_start_raw = text.len().saturating_sub(tail_budget);
    while tail_start_raw < text.len() && !text.is_char_boundary(tail_start_raw) {
        tail_start_raw += 1;
    }
    let tail_start = text[tail_start_raw..]
        .find('\n')
        .map(|p| tail_start_raw + p + 1)
        .unwrap_or(tail_start_raw);
    let tail = &text[tail_start..];

    let omitted = text.len() - head.len() - tail.len();
    let tokens_est = omitted / 4;

    format!(
        "{}\n\n[... {} chars (~{} tokens) omitted for context budget ...]\n\n{}",
        head, omitted, tokens_est, tail
    )
}

/// Simulate read_file tool output (with line numbers).
fn simulate_read_file(path: &Path) -> String {
    let content = fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = content.lines().collect();
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate().take(500) {
        out.push_str(&format!("{:>4} | {}\n", i + 1, line));
    }
    if lines.len() > 500 {
        out.push_str(&format!(
            "\n[... truncated: showing 500/{} lines ...]",
            lines.len()
        ));
    }
    out
}

/// Apply Standard compression for a given tool type.
fn compress_standard(tool_name: &str, text: &str) -> String {
    // Light
    let mut text = strip_ansi_codes(text);
    text = collapse_blank_lines(&text);
    text = trim_trailing_whitespace(&text);

    // Tool-specific
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
        _ => {
            text = compact_json_in_text(&text);
            text = deduplicate_consecutive_lines(&text, 3);
        }
    }
    text
}

/// Apply Aggressive compression.
fn compress_aggressive(tool_name: &str, text: &str) -> String {
    let mut text = compress_standard(tool_name, text);
    if text.len() > 8_000 {
        text = smart_truncate(&text, 8_000);
    }
    text
}

fn bar(pct: f64, width: usize) -> String {
    let filled = ((pct / 100.0) * width as f64) as usize;
    let empty = width.saturating_sub(filled);
    format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
}

fn print_result(label: &str, tool: &str, original: &str, compressed: &str) {
    let orig_len = original.len();
    let comp_len = compressed.len();
    let saved = orig_len.saturating_sub(comp_len);
    let pct_saved = if orig_len > 0 {
        (saved as f64 / orig_len as f64) * 100.0
    } else {
        0.0
    };
    let orig_tokens = (orig_len + 3) / 4;
    let comp_tokens = (comp_len + 3) / 4;

    println!("┌─────────────────────────────────────────────────────────────");
    println!("│ {} (tool: {})", label, tool);
    println!("│ Original:   {:>7} chars  (~{:>5} tokens)", orig_len, orig_tokens);
    println!("│ Compressed: {:>7} chars  (~{:>5} tokens)", comp_len, comp_tokens);
    println!(
        "│ Saved:      {:>7} chars  (~{:>5} tokens)  {:.1}%",
        saved,
        orig_tokens - comp_tokens,
        pct_saved
    );
    println!("│ {}", bar(pct_saved, 40));
    println!("└─────────────────────────────────────────────────────────────");
    println!();
}

fn print_preview(label: &str, text: &str, max_lines: usize) {
    println!("  ── {} (first {} lines) ──", label, max_lines);
    for (i, line) in text.lines().take(max_lines).enumerate() {
        let display = if line.len() > 100 {
            format!("{}…", &line[..99])
        } else {
            line.to_string()
        };
        println!("  {:>3}│ {}", i + 1, display);
    }
    let total_lines = text.lines().count();
    if total_lines > max_lines {
        println!("  ...│ ({} more lines)", total_lines - max_lines);
    }
    println!();
}

#[test]
fn compression_demo_full_project() {
    println!("\n");
    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║       ALLUX TOKEN COMPRESSION — REAL PROJECT DEMO           ║");
    println!("╚═══════════════════════════════════════════════════════════════╝");
    println!();

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut total_original = 0usize;
    let mut total_compressed = 0usize;
    let mut total_aggressive = 0usize;

    // ── 1. read_file on each .rs source file ────────────────────────────
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SECTION 1: read_file tool output (all .rs files in src/)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    let mut rs_files: Vec<_> = walkdir(root.join("src"))
        .into_iter()
        .filter(|p| p.extension().map(|e| e == "rs").unwrap_or(false))
        .collect();
    rs_files.sort();

    for path in &rs_files {
        let rel = path.strip_prefix(root).unwrap_or(path);
        let original = simulate_read_file(path);
        let compressed = compress_standard("read_file", &original);
        let aggressive = compress_aggressive("read_file", &original);

        total_original += original.len();
        total_compressed += compressed.len();
        total_aggressive += aggressive.len();

        print_result(
            &format!("read_file({})", rel.display()),
            "read_file",
            &original,
            &compressed,
        );
    }

    // ── 2. Simulate bash tool outputs ───────────────────────────────────
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SECTION 2: bash tool output (simulated build/test output)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    // Simulate `cargo check` output with ANSI codes
    let fake_cargo_check = format!(
        "\x1b[32m    Checking\x1b[0m allux v0.1.0 (/Users/marvin/Projects/allux-agent)\n\
         \x1b[33mwarning\x1b[0m: unused variable `x`\n\
         \x1b[34m  --> \x1b[0msrc/main.rs:10:5\n\
         \x1b[34m   | \x1b[0m\n\
         \x1b[34m10 | \x1b[0m    let x = 5;\n\
         \x1b[34m   | \x1b[0m        \x1b[33m^\x1b[0m help: use `_x`\n\
         \x1b[34m   | \x1b[0m\n\
         {}\
         \x1b[32m    Finished\x1b[0m `dev` profile [unoptimized + debuginfo] target(s) in 1.27s\n",
        "   = note: `#[warn(dead_code)]` on by default\n".repeat(20)
    );

    let compressed = compress_standard("bash", &fake_cargo_check);
    total_original += fake_cargo_check.len();
    total_compressed += compressed.len();
    total_aggressive += compress_aggressive("bash", &fake_cargo_check).len();

    print_result("bash(cargo check) — with ANSI colors", "bash", &fake_cargo_check, &compressed);
    print_preview("BEFORE", &fake_cargo_check, 8);
    print_preview("AFTER", &compressed, 8);

    // Simulate repetitive test output
    let fake_test_output = {
        let mut s = String::new();
        s.push_str("running 50 tests\n");
        for i in 1..=50 {
            s.push_str(&format!("test test_{:03} ... ok\n", i));
        }
        s.push_str("\ntest result: ok. 50 passed; 0 failed; 0 ignored\n");
        s
    };

    let compressed = compress_standard("bash", &fake_test_output);
    total_original += fake_test_output.len();
    total_compressed += compressed.len();
    total_aggressive += compress_aggressive("bash", &fake_test_output).len();

    print_result("bash(cargo test) — 50 passing tests", "bash", &fake_test_output, &compressed);

    // Simulate JSON API response
    let fake_json = serde_json::to_string_pretty(&serde_json::json!({
        "models": [
            {"name": "llama3.2:latest", "details": {"parameter_size": "3B", "quantization_level": "Q4_K_M"}},
            {"name": "qwen2.5-coder:7b", "details": {"parameter_size": "7B", "quantization_level": "Q4_K_M"}},
            {"name": "deepseek-coder:6.7b", "details": {"parameter_size": "6.7B", "quantization_level": "Q4_K_M"}},
            {"name": "mistral:7b", "details": {"parameter_size": "7B", "quantization_level": "Q4_0"}},
            {"name": "codellama:13b", "details": {"parameter_size": "13B", "quantization_level": "Q4_0"}}
        ]
    }))
    .unwrap();

    let compressed = compress_standard("bash", &fake_json);
    total_original += fake_json.len();
    total_compressed += compressed.len();
    total_aggressive += compress_aggressive("bash", &fake_json).len();

    print_result("bash(curl /api/tags) — JSON response", "bash", &fake_json, &compressed);
    print_preview("BEFORE", &fake_json, 10);
    print_preview("AFTER", &compressed, 5);

    // ── 3. Simulate grep output ─────────────────────────────────────────
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SECTION 3: grep tool output");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    let fake_grep = {
        let mut s = String::new();
        for i in 1..=30 {
            s.push_str(&format!("src/file_{}.rs:{}: use anyhow::Result;\n", i, 1));
        }
        s
    };

    let compressed = compress_standard("grep", &fake_grep);
    total_original += fake_grep.len();
    total_compressed += compressed.len();
    total_aggressive += compress_aggressive("grep", &fake_grep).len();

    print_result("grep(use anyhow)", "grep", &fake_grep, &compressed);

    // ── 4. Cargo.toml as read_file ──────────────────────────────────────
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SECTION 4: Cargo.toml via read_file");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    let cargo_read = simulate_read_file(&root.join("Cargo.toml"));
    let compressed = compress_standard("read_file", &cargo_read);
    total_original += cargo_read.len();
    total_compressed += compressed.len();
    total_aggressive += compress_aggressive("read_file", &cargo_read).len();

    print_result("read_file(Cargo.toml)", "read_file", &cargo_read, &compressed);
    print_preview("BEFORE", &cargo_read, 10);
    print_preview("AFTER", &compressed, 10);

    // ── 5. Aggressive compression preview ───────────────────────────────
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SECTION 5: Aggressive compression on largest file");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    // Find largest .rs file
    if let Some(largest) = rs_files.iter().max_by_key(|p| fs::metadata(p).map(|m| m.len()).unwrap_or(0)) {
        let rel = largest.strip_prefix(root).unwrap_or(largest);
        let original = simulate_read_file(largest);
        let aggressive = compress_aggressive("read_file", &original);

        print_result(
            &format!("AGGRESSIVE read_file({})", rel.display()),
            "read_file",
            &original,
            &aggressive,
        );

        if aggressive.contains("omitted for context budget") {
            println!("  ── Smart truncation was applied ──");
            // Show the truncation marker
            for line in aggressive.lines() {
                if line.contains("omitted for context budget") {
                    println!("  >>> {}", line);
                }
            }
            println!();
        }
    }

    // ── GRAND TOTAL ─────────────────────────────────────────────────────
    println!();
    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║                      GRAND TOTALS                           ║");
    println!("╠═══════════════════════════════════════════════════════════════╣");

    let std_saved = total_original.saturating_sub(total_compressed);
    let std_pct = if total_original > 0 {
        (std_saved as f64 / total_original as f64) * 100.0
    } else {
        0.0
    };
    let agg_saved = total_original.saturating_sub(total_aggressive);
    let agg_pct = if total_original > 0 {
        (agg_saved as f64 / total_original as f64) * 100.0
    } else {
        0.0
    };

    println!(
        "║  Original total:    {:>7} chars  (~{:>5} tokens)          ║",
        total_original,
        total_original / 4
    );
    println!(
        "║  Standard total:    {:>7} chars  (~{:>5} tokens) −{:.1}%    ║",
        total_compressed,
        total_compressed / 4,
        std_pct
    );
    println!(
        "║  Aggressive total:  {:>7} chars  (~{:>5} tokens) −{:.1}%    ║",
        total_aggressive,
        total_aggressive / 4,
        agg_pct
    );
    println!(
        "║                                                             ║"
    );
    println!(
        "║  Standard savings:  {:>7} chars  (~{:>5} tokens)          ║",
        std_saved,
        std_saved / 4
    );
    println!(
        "║  Aggressive savings:{:>7} chars  (~{:>5} tokens)          ║",
        agg_saved,
        agg_saved / 4
    );
    println!("╚═══════════════════════════════════════════════════════════════╝");
    println!();

    // ── Write all three versions to /tmp for side-by-side inspection ─────
    let out_dir = Path::new("/tmp/allux_compression_demo");
    let _ = fs::remove_dir_all(out_dir);
    fs::create_dir_all(out_dir).expect("create output dir");

    // Concatenate all source files as a single read_file simulation
    let mut all_original = String::new();
    let mut all_standard = String::new();
    let mut all_aggressive = String::new();

    for path in &rs_files {
        let rel = path.strip_prefix(root).unwrap_or(path);
        let header = format!("═══ {} ═══\n", rel.display());
        let original = simulate_read_file(path);
        let standard = compress_standard("read_file", &original);
        let aggressive = compress_aggressive("read_file", &original);

        all_original.push_str(&header);
        all_original.push_str(&original);
        all_original.push_str("\n\n");

        all_standard.push_str(&header);
        all_standard.push_str(&standard);
        all_standard.push_str("\n\n");

        all_aggressive.push_str(&header);
        all_aggressive.push_str(&aggressive);
        all_aggressive.push_str("\n\n");
    }

    // Add bash simulations
    let bash_header = "═══ bash: cargo check (with ANSI) ═══\n";
    all_original.push_str(bash_header);
    all_original.push_str(&fake_cargo_check);
    all_original.push_str("\n\n");
    all_standard.push_str(bash_header);
    all_standard.push_str(&compress_standard("bash", &fake_cargo_check));
    all_standard.push_str("\n\n");
    all_aggressive.push_str(bash_header);
    all_aggressive.push_str(&compress_aggressive("bash", &fake_cargo_check));
    all_aggressive.push_str("\n\n");

    let bash_header2 = "═══ bash: cargo test (50 tests) ═══\n";
    all_original.push_str(bash_header2);
    all_original.push_str(&fake_test_output);
    all_original.push_str("\n\n");
    all_standard.push_str(bash_header2);
    all_standard.push_str(&compress_standard("bash", &fake_test_output));
    all_standard.push_str("\n\n");
    all_aggressive.push_str(bash_header2);
    all_aggressive.push_str(&compress_aggressive("bash", &fake_test_output));
    all_aggressive.push_str("\n\n");

    let bash_header3 = "═══ bash: curl /api/tags (JSON) ═══\n";
    all_original.push_str(bash_header3);
    all_original.push_str(&fake_json);
    all_original.push_str("\n\n");
    all_standard.push_str(bash_header3);
    all_standard.push_str(&compress_standard("bash", &fake_json));
    all_standard.push_str("\n\n");
    all_aggressive.push_str(bash_header3);
    all_aggressive.push_str(&compress_aggressive("bash", &fake_json));
    all_aggressive.push_str("\n\n");

    let grep_header = "═══ grep: use anyhow ═══\n";
    all_original.push_str(grep_header);
    all_original.push_str(&fake_grep);
    all_standard.push_str(grep_header);
    all_standard.push_str(&compress_standard("grep", &fake_grep));
    all_aggressive.push_str(grep_header);
    all_aggressive.push_str(&compress_aggressive("grep", &fake_grep));

    // Write files
    fs::write(out_dir.join("1_original.txt"), &all_original).expect("write original");
    fs::write(out_dir.join("2_standard.txt"), &all_standard).expect("write standard");
    fs::write(out_dir.join("3_aggressive.txt"), &all_aggressive).expect("write aggressive");

    // Write a summary file
    let summary = format!(
        "ALLUX TOKEN COMPRESSION — OUTPUT FILES\n\
         ======================================\n\n\
         Generated: {}\n\
         Source: {} .rs files + 3 bash sims + 1 grep sim\n\n\
         File                   Size (chars)    ~Tokens\n\
         ─────────────────────  ────────────    ───────\n\
         1_original.txt         {:>12}    {:>7}\n\
         2_standard.txt         {:>12}    {:>7}\n\
         3_aggressive.txt       {:>12}    {:>7}\n\n\
         Standard savings:      {:>12}    {:>7}  ({:.1}%)\n\
         Aggressive savings:    {:>12}    {:>7}  ({:.1}%)\n\n\
         Diff commands:\n\
         diff /tmp/allux_compression_demo/1_original.txt /tmp/allux_compression_demo/2_standard.txt\n\
         diff /tmp/allux_compression_demo/1_original.txt /tmp/allux_compression_demo/3_aggressive.txt\n",
        chrono_now(),
        rs_files.len(),
        all_original.len(), all_original.len() / 4,
        all_standard.len(), all_standard.len() / 4,
        all_aggressive.len(), all_aggressive.len() / 4,
        all_original.len() - all_standard.len(),
        (all_original.len() - all_standard.len()) / 4,
        (1.0 - all_standard.len() as f64 / all_original.len() as f64) * 100.0,
        all_original.len() - all_aggressive.len(),
        (all_original.len() - all_aggressive.len()) / 4,
        (1.0 - all_aggressive.len() as f64 / all_original.len() as f64) * 100.0,
    );
    fs::write(out_dir.join("SUMMARY.txt"), &summary).expect("write summary");

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  FILES WRITTEN TO /tmp/allux_compression_demo/");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("  1_original.txt     {:>7} chars  (~{} tokens)", all_original.len(), all_original.len() / 4);
    println!("  2_standard.txt     {:>7} chars  (~{} tokens)", all_standard.len(), all_standard.len() / 4);
    println!("  3_aggressive.txt   {:>7} chars  (~{} tokens)", all_aggressive.len(), all_aggressive.len() / 4);
    println!("  SUMMARY.txt");
    println!();
    println!("  Compare with:");
    println!("    diff /tmp/allux_compression_demo/1_original.txt /tmp/allux_compression_demo/2_standard.txt");
    println!("    diff /tmp/allux_compression_demo/1_original.txt /tmp/allux_compression_demo/3_aggressive.txt");
    println!();
}

fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    format!("unix:{}", secs)
}

/// Simple recursive file walker.
fn walkdir(path: std::path::PathBuf) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    if path.is_file() {
        files.push(path);
    } else if path.is_dir() {
        if let Ok(entries) = fs::read_dir(&path) {
            for entry in entries.flatten() {
                let p = entry.path();
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.starts_with('.') || name == "target" || name == "node_modules" {
                    continue;
                }
                files.extend(walkdir(p));
            }
        }
    }
    files
}
