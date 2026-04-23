/// Deterministic keyword extraction from a natural-language goal string.
/// No LLM involved — classical tokenise → stopword filter → light stem → dedup.

// ── Stopwords ─────────────────────────────────────────────────────────────────

const EN_STOPWORDS: &[&str] = &[
    "a","an","the","and","or","but","in","on","at","to","for","of","with",
    "by","from","as","is","are","was","were","be","been","being","have","has",
    "had","do","does","did","will","would","could","should","may","might",
    "shall","can","not","no","so","if","it","its","this","that","these","those",
    "we","us","our","i","my","you","your","he","she","they","their","them",
    "what","which","who","how","when","where","all","any","each","every",
    "some","more","most","other","into","up","out","about","just","also",
    "than","then","there","here","very","own","same","both","only","such",
    "after","before","over","under","between","through",
];

const ES_STOPWORDS: &[&str] = &[
    "el","la","los","las","un","una","unos","unas","de","del","al","en",
    "con","por","para","que","y","o","es","son","era","ser","estar","se",
    "no","si","lo","le","les","me","te","nos","mi","tu","su","sus","mis",
    "tus","como","este","esta","estos","estas","ese","esa","esos","esas",
    "más","muy","pero","hay","tiene","tienen","hacer","haz",
];

// ── Suffix stripping ──────────────────────────────────────────────────────────

/// Remove common EN/ES suffixes to reduce morphological variants.
fn stem_token(token: &str) -> String {
    let t = token;
    for &suffix in &["aciones","ización","tion","sion","ing","ness","ment",
                      "ción","ado","ada","ados","adas","tion","tions",
                      "ings","ments","ed","es","er","ers","ly","tion"] {
        if t.len() > suffix.len() + 3 && t.ends_with(suffix) {
            return t[..t.len() - suffix.len()].to_string();
        }
    }
    t.to_string()
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Extract up to 20 meaningful keywords from a free-text goal.
pub fn extract_keywords(goal: &str) -> Vec<String> {
    let tokens = tokenize_lower(goal);
    let filtered = drop_stopwords(tokens);
    let stemmed = stem_light(filtered);
    dedup_preserving_order(stemmed)
        .into_iter()
        .filter(|t| t.len() >= 3)
        .take(20)
        .collect()
}

fn tokenize_lower(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

fn drop_stopwords(tokens: Vec<String>) -> Vec<String> {
    tokens
        .into_iter()
        .filter(|t| {
            !EN_STOPWORDS.contains(&t.as_str()) && !ES_STOPWORDS.contains(&t.as_str())
        })
        .collect()
}

fn stem_light(tokens: Vec<String>) -> Vec<String> {
    tokens.into_iter().map(|t| stem_token(&t)).collect()
}

fn dedup_preserving_order(tokens: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    tokens.into_iter().filter(|t| seen.insert(t.clone())).collect()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_keywords_basic() {
        let kw = extract_keywords("Create a landing page with a contact form");
        assert!(kw.contains(&"landing".to_string()) || kw.contains(&"land".to_string()));
        assert!(kw.contains(&"page".to_string()));
        assert!(kw.contains(&"contact".to_string()));
        // stopwords removed
        assert!(!kw.contains(&"a".to_string()));
        assert!(!kw.contains(&"with".to_string()));
    }

    #[test]
    fn test_extract_keywords_dedup() {
        let kw = extract_keywords("build build build the the thing");
        let build_count = kw.iter().filter(|k| k.starts_with("build")).count();
        assert!(build_count <= 1);
    }

    #[test]
    fn test_extract_keywords_min_len() {
        let kw = extract_keywords("an at to or a");
        assert!(kw.is_empty());
    }

    #[test]
    fn test_extract_keywords_max_20() {
        let goal = "alpha beta gamma delta epsilon zeta eta theta iota kappa \
                    lambda mu nu xi omicron pi rho sigma tau upsilon phi chi";
        let kw = extract_keywords(goal);
        assert!(kw.len() <= 20);
    }

    #[test]
    fn test_stem_removes_ing() {
        assert_eq!(stem_token("building"), "build");
    }

    #[test]
    fn test_stem_removes_tion() {
        // "creation" ends with "tion" → strips 4 chars → "crea"
        assert_eq!(stem_token("creation"), "crea");
        // "validation" → strips "tion" → "valida"
        assert_eq!(stem_token("validation"), "valida");
    }

    #[test]
    fn test_stem_preserves_short() {
        // "cat" is too short to stem
        assert_eq!(stem_token("cat"), "cat");
    }

    #[test]
    fn test_es_stopwords_removed() {
        let kw = extract_keywords("crear una página de inicio con formulario de contacto");
        assert!(!kw.contains(&"una".to_string()));
        assert!(!kw.contains(&"de".to_string()));
        assert!(kw.iter().any(|k| k.contains("formulario") || k.contains("formula")));
    }
}
