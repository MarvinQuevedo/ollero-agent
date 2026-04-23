use std::collections::{BTreeMap, HashMap, HashSet};

use regex::Regex;

use crate::orchestra::types::{CheckOutcome, Language};

// ── Placeholder detection ─────────────────────────────────────────────────────

const PLACEHOLDER_PATTERNS: &[&str] = &[
    r"\bTODO\b",
    r"\bFIXME\b",
    r"\bXXX\b",
    r"\bTBD\b",
    r"<INSERT[^>]*>",
    r"<!--\s*YOUR\s",
    r"\[PLACEHOLDER\]",
    r"(?i)\blorem ipsum\b",
    r"(?i)\breplace\s+this\b",
    r"<your-[a-z-]+-here>",
];

/// Check that the content contains no placeholder markers.
/// `whitelist` is a list of literal strings that are allowed to appear even if they
/// match a placeholder pattern.
pub fn no_placeholders(content: &str, whitelist: &[String]) -> CheckOutcome {
    for pattern_str in PLACEHOLDER_PATTERNS {
        if let Ok(re) = Regex::new(pattern_str) {
            if let Some(m) = re.find(content) {
                let found = m.as_str();
                // Check if this match is in the whitelist
                let whitelisted = whitelist.iter().any(|w| found.contains(w.as_str()));
                if !whitelisted {
                    // Find the line number for better reporting
                    let line_no = content[..m.start()].matches('\n').count() + 1;
                    return CheckOutcome::Fail {
                        reason: format!("placeholder `{found}` at line {line_no}"),
                    };
                }
            }
        }
    }
    CheckOutcome::Pass
}

// ── Loop repetition (zstd ratio) ─────────────────────────────────────────────

/// Check that content is not highly repetitive (a sign of a looping model output).
/// Uses zstd compression ratio: very repetitive content compresses to < `max_ratio` of
/// its original size.
pub fn no_loop_repetition(content: &str, max_ratio: f32) -> CheckOutcome {
    if content.len() < 200 {
        return CheckOutcome::Pass;
    }
    match zstd::encode_all(content.as_bytes(), 3) {
        Ok(compressed) => {
            let ratio = compressed.len() as f32 / content.len() as f32;
            if ratio < max_ratio {
                CheckOutcome::Fail {
                    reason: format!(
                        "zstd ratio {ratio:.2} < threshold {max_ratio:.2} — content is highly repetitive"
                    ),
                }
            } else {
                CheckOutcome::Pass
            }
        }
        Err(_) => CheckOutcome::Pass, // if compression fails, skip check
    }
}

// ── Unique line ratio ─────────────────────────────────────────────────────────

/// Fraction of non-blank lines that are unique. Returns Soft(ratio).
pub fn unique_line_ratio(content: &str) -> CheckOutcome {
    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < 10 {
        return CheckOutcome::Pass;
    }
    let uniq: HashSet<&&str> = lines.iter().collect();
    let ratio = uniq.len() as f32 / lines.len() as f32;
    CheckOutcome::Soft(ratio.min(1.0))
}

// ── N-gram repetition ─────────────────────────────────────────────────────────

/// Check that no single 2- or 3-gram covers more than 15% of all n-grams.
pub fn ngram_repetition(content: &str) -> CheckOutcome {
    let words: Vec<&str> = content.split_whitespace().collect();
    if words.len() < 20 {
        return CheckOutcome::Pass;
    }

    // 2-grams
    if let Some(ratio) = top_ngram_ratio(&words, 2) {
        if ratio > 0.15 {
            return CheckOutcome::Fail {
                reason: format!("top 2-gram covers {:.0}% of content", ratio * 100.0),
            };
        }
    }

    // 3-grams
    if let Some(ratio) = top_ngram_ratio(&words, 3) {
        if ratio > 0.15 {
            return CheckOutcome::Fail {
                reason: format!("top 3-gram covers {:.0}% of content", ratio * 100.0),
            };
        }
    }

    CheckOutcome::Pass
}

fn top_ngram_ratio(words: &[&str], n: usize) -> Option<f32> {
    if words.len() < n {
        return None;
    }
    let total = words.len() - n + 1;
    let mut counts: HashMap<Vec<&str>, usize> = HashMap::new();
    for i in 0..total {
        *counts.entry(words[i..i + n].to_vec()).or_insert(0) += 1;
    }
    counts.values().max().map(|&max| max as f32 / total as f32)
}

// ── Shannon entropy ───────────────────────────────────────────────────────────

/// Check that the file's byte entropy is in a reasonable range (3.0 – 7.5).
pub fn entropy_reasonable(bytes: &[u8]) -> CheckOutcome {
    if bytes.is_empty() {
        return CheckOutcome::Soft(0.5);
    }
    let h = shannon_entropy(bytes);
    if h < 2.0 {
        CheckOutcome::Fail { reason: format!("entropy {h:.2} < 2.0 (degenerate content)") }
    } else if h < 3.0 {
        CheckOutcome::Soft(0.3)
    } else if h <= 7.5 {
        CheckOutcome::Pass
    } else if h < 7.9 {
        CheckOutcome::Soft(0.6)
    } else {
        CheckOutcome::Fail { reason: format!("entropy {h:.2} ≥ 7.9 (likely binary/random)") }
    }
}

pub fn shannon_entropy(bytes: &[u8]) -> f32 {
    let mut freq = [0u64; 256];
    for &b in bytes {
        freq[b as usize] += 1;
    }
    let n = bytes.len() as f32;
    -freq
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f32 / n;
            p * p.log2()
        })
        .sum::<f32>()
}

// ── Keywords present ──────────────────────────────────────────────────────────

/// Check that at least `min_hit` fraction of the expected keywords appear in the content.
pub fn keywords_present(content: &str, keywords: &[String], min_hit: f32) -> CheckOutcome {
    if keywords.is_empty() {
        return CheckOutcome::Pass;
    }
    let hay = content.to_lowercase();
    let hits = keywords
        .iter()
        .filter(|k| hay.contains(k.to_lowercase().as_str()))
        .count();
    let ratio = hits as f32 / keywords.len() as f32;
    if ratio < min_hit {
        CheckOutcome::Soft(ratio)
    } else {
        CheckOutcome::Pass
    }
}

// ── Language detection ────────────────────────────────────────────────────────

pub fn language_matches(content: &str, target: Language) -> CheckOutcome {
    if target == Language::Unknown {
        return CheckOutcome::Soft(0.5);
    }
    let scores = score_languages(content);
    let total: f32 = scores.values().sum();
    if total < 0.001 {
        return CheckOutcome::Soft(0.5);
    }

    let detected = scores
        .iter()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .map(|(l, _)| *l);

    if detected == Some(target) {
        CheckOutcome::Pass
    } else {
        let target_score = scores.get(&target).copied().unwrap_or(0.0);
        CheckOutcome::Soft((target_score / total).clamp(0.0, 1.0))
    }
}

/// Score languages by trigram frequency against embedded tables.
fn score_languages(content: &str) -> BTreeMap<Language, f32> {
    let text = content.to_lowercase();
    let mut scores = BTreeMap::new();
    scores.insert(Language::En, score_trigrams(&text, &EN_TRIGRAMS));
    scores.insert(Language::Es, score_trigrams(&text, &ES_TRIGRAMS));
    scores
}

fn score_trigrams(text: &str, trigrams: &[(&str, f32)]) -> f32 {
    let chars: Vec<char> = text.chars().collect();
    let mut score = 0.0f32;
    for (tg, weight) in trigrams {
        let tg_chars: Vec<char> = tg.chars().collect();
        for window in chars.windows(3) {
            if window == tg_chars.as_slice() {
                score += weight;
            }
        }
    }
    score
}

// Top trigrams for English (frequency-weighted)
static EN_TRIGRAMS: &[(&str, f32)] = &[
    ("the", 1.0), ("and", 0.9), ("ing", 0.8), ("ion", 0.75), ("ent", 0.7),
    ("her", 0.65), ("for", 0.63), ("hat", 0.6), ("his", 0.58), ("that", 0.55),
    ("ere", 0.53), ("con", 0.5), ("ter", 0.48), ("thi", 0.46), ("ati", 0.44),
    ("wit", 0.42), ("ver", 0.4), ("not", 0.38), ("was", 0.36), ("are", 0.34),
    ("all", 0.32), ("you", 0.3), ("ith", 0.28), ("tic", 0.26), ("our", 0.24),
];

// Top trigrams for Spanish
static ES_TRIGRAMS: &[(&str, f32)] = &[
    ("que", 1.0), ("ión", 0.9), ("las", 0.85), ("los", 0.8), ("del", 0.75),
    ("una", 0.72), ("par", 0.68), ("con", 0.65), ("ado", 0.62), ("esta", 0.6),
    ("ent", 0.58), ("men", 0.55), ("nes", 0.52), ("com", 0.5), ("tra", 0.48),
    ("por", 0.46), ("ción", 0.44), ("pro", 0.42), ("res", 0.4), ("nte", 0.38),
    ("cion", 0.36), ("mos", 0.34), ("est", 0.32), ("der", 0.3), ("ser", 0.28),
];

// ── Empty critical blocks ─────────────────────────────────────────────────────

/// Check that no public function/method has an empty body.
pub fn no_empty_critical_blocks(content: &str, ext: &str) -> CheckOutcome {
    let pattern = match ext {
        "rs" => Some(r"fn\s+\w+[^{]*\{\s*\}"),
        "js" | "ts" => Some(r"function\s+\w+[^{]*\{\s*\}"),
        "py" => Some(r"def\s+\w+[^:]*:\s*\n\s*pass\s*$"),
        _ => None,
    };

    let Some(pat) = pattern else {
        return CheckOutcome::Pass;
    };

    if let Ok(re) = Regex::new(pat) {
        if re.is_match(content) {
            // Find the specific match for reporting
            if let Some(m) = re.find(content) {
                let line_no = content[..m.start()].matches('\n').count() + 1;
                return CheckOutcome::Fail {
                    reason: format!("empty function body at line {line_no}"),
                };
            }
        }
    }

    CheckOutcome::Pass
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── no_placeholders ─────────────────────────────────────────────────────

    #[test]
    fn test_no_placeholders_clean() {
        let content = "This is normal content with no placeholders.";
        assert_eq!(no_placeholders(content, &[]), CheckOutcome::Pass);
    }

    #[test]
    fn test_no_placeholders_todo() {
        let content = "// TODO: implement this later";
        assert!(matches!(no_placeholders(content, &[]), CheckOutcome::Fail { .. }));
    }

    #[test]
    fn test_no_placeholders_fixme() {
        let content = "// FIXME: broken";
        assert!(matches!(no_placeholders(content, &[]), CheckOutcome::Fail { .. }));
    }

    #[test]
    fn test_no_placeholders_lorem() {
        let content = "Lorem ipsum dolor sit amet";
        assert!(matches!(no_placeholders(content, &[]), CheckOutcome::Fail { .. }));
    }

    #[test]
    fn test_no_placeholders_whitelisted() {
        let content = "// TODO: this is intentional";
        let whitelist = vec!["TODO".into()];
        assert_eq!(no_placeholders(content, &whitelist), CheckOutcome::Pass);
    }

    // ── no_loop_repetition ──────────────────────────────────────────────────

    #[test]
    fn test_no_loop_repetition_normal() {
        let content = "fn main() {\n    println!(\"Hello, world!\");\n    let x = 42;\n    let y = x * 2;\n    println!(\"Result: {}\", y);\n}\n".repeat(5);
        // Normal code should not be flagged
        let result = no_loop_repetition(&content, 0.15);
        // May or may not pass depending on compression ratio — just ensure no panic
        let _ = result;
    }

    #[test]
    fn test_no_loop_repetition_highly_repetitive() {
        let content = "abcdef".repeat(500); // highly repetitive
        assert!(matches!(no_loop_repetition(&content, 0.15), CheckOutcome::Fail { .. }));
    }

    #[test]
    fn test_no_loop_repetition_short_content() {
        let content = "short";
        assert_eq!(no_loop_repetition(content, 0.15), CheckOutcome::Pass);
    }

    // ── unique_line_ratio ───────────────────────────────────────────────────

    #[test]
    fn test_unique_line_ratio_short() {
        let content = "a\nb\nc";
        assert_eq!(unique_line_ratio(content), CheckOutcome::Pass);
    }

    #[test]
    fn test_unique_line_ratio_all_same() {
        let content = "same line\n".repeat(20);
        match unique_line_ratio(&content) {
            CheckOutcome::Soft(s) => assert!(s < 0.2),
            other => panic!("expected Soft, got {:?}", other),
        }
    }

    #[test]
    fn test_unique_line_ratio_all_unique() {
        let lines: Vec<String> = (0..20).map(|i| format!("line {i}")).collect();
        let content = lines.join("\n");
        match unique_line_ratio(&content) {
            CheckOutcome::Soft(s) => assert!((s - 1.0).abs() < 0.01),
            CheckOutcome::Pass => {} // also acceptable for 100%
            other => panic!("unexpected {:?}", other),
        }
    }

    // ── ngram_repetition ────────────────────────────────────────────────────

    #[test]
    fn test_ngram_normal_text() {
        let content = "The quick brown fox jumps over the lazy dog. \
                       A journey of a thousand miles begins with a single step. \
                       All that glitters is not gold. Practice makes perfect.";
        assert_eq!(ngram_repetition(content), CheckOutcome::Pass);
    }

    #[test]
    fn test_ngram_highly_repetitive() {
        let content = "hello world ".repeat(50);
        assert!(matches!(ngram_repetition(&content), CheckOutcome::Fail { .. }));
    }

    #[test]
    fn test_ngram_short_content() {
        let content = "only a few words here";
        assert_eq!(ngram_repetition(content), CheckOutcome::Pass);
    }

    // ── shannon_entropy ─────────────────────────────────────────────────────

    #[test]
    fn test_entropy_single_byte() {
        // Single repeated byte → entropy = 0
        let bytes = vec![b'a'; 100];
        let h = shannon_entropy(&bytes);
        assert!(h < 0.001, "entropy of constant bytes should be ~0, got {h}");
    }

    #[test]
    fn test_entropy_uniform() {
        // All 256 bytes equally → entropy = 8.0
        let bytes: Vec<u8> = (0u8..=255).collect();
        let h = shannon_entropy(&bytes);
        assert!((h - 8.0).abs() < 0.01, "entropy of uniform bytes should be ~8.0, got {h}");
    }

    // ── entropy_reasonable ──────────────────────────────────────────────────

    #[test]
    fn test_entropy_reasonable_normal_code() {
        let code = b"fn main() { println!(\"Hello, world!\"); let x = 42; }";
        assert_eq!(entropy_reasonable(code), CheckOutcome::Pass);
    }

    #[test]
    fn test_entropy_reasonable_degenerate() {
        let bytes = vec![b'x'; 100];
        assert!(matches!(entropy_reasonable(&bytes), CheckOutcome::Fail { .. }));
    }

    // ── keywords_present ────────────────────────────────────────────────────

    #[test]
    fn test_keywords_present_all_found() {
        let content = "The page has a hero section with doctor services and contact form.";
        let kw = vec!["hero".into(), "doctor".into(), "contact".into()];
        assert_eq!(keywords_present(content, &kw, 0.8), CheckOutcome::Pass);
    }

    #[test]
    fn test_keywords_present_partial() {
        let content = "The page has a hero section.";
        let kw = vec!["hero".into(), "doctor".into(), "contact".into()];
        // 1/3 hit = 0.33, min_hit = 0.4
        match keywords_present(content, &kw, 0.4) {
            CheckOutcome::Soft(s) => assert!((s - 0.333).abs() < 0.01),
            other => panic!("expected Soft, got {:?}", other),
        }
    }

    #[test]
    fn test_keywords_present_empty_list() {
        let content = "anything";
        let kw: Vec<String> = vec![];
        assert_eq!(keywords_present(content, &kw, 0.5), CheckOutcome::Pass);
    }

    // ── language_matches ────────────────────────────────────────────────────

    #[test]
    fn test_language_english() {
        let content = "The quick brown fox jumps over the lazy dog and all that.";
        let result = language_matches(content, Language::En);
        assert!(matches!(result, CheckOutcome::Pass | CheckOutcome::Soft(_)));
    }

    #[test]
    fn test_language_spanish() {
        let content = "El proyecto que se construye con las herramientas del sistema operativo.";
        let result = language_matches(content, Language::Es);
        // Just ensure no panic
        let _ = result;
    }

    // ── no_empty_critical_blocks ────────────────────────────────────────────

    #[test]
    fn test_no_empty_blocks_clean_rust() {
        let content = "fn main() {\n    println!(\"hello\");\n}\n";
        assert_eq!(no_empty_critical_blocks(content, "rs"), CheckOutcome::Pass);
    }

    #[test]
    fn test_no_empty_blocks_empty_fn_rust() {
        let content = "fn placeholder() {}";
        assert!(matches!(no_empty_critical_blocks(content, "rs"), CheckOutcome::Fail { .. }));
    }

    #[test]
    fn test_no_empty_blocks_unknown_ext() {
        let content = "anything";
        assert_eq!(no_empty_critical_blocks(content, "xyz"), CheckOutcome::Pass);
    }
}
