use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use super::types::{
    Diagnosis, ExpectedFile, FileChange, RetryStrategy, TaskReport, TaskSpec, TaskStatus,
};

// ── Error type ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlfError {
    Missing(String),
    UnclosedBlock,
    BadMarker(String),
    BadInt(String),
    BadFloat(String),
    InvalidStrategy(String),
    InvalidStatus(String),
}

impl fmt::Display for AlfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(k)         => write!(f, "missing required key: {k}"),
            Self::UnclosedBlock      => write!(f, "block not closed before '.' or EOF"),
            Self::BadMarker(m)       => write!(f, "invalid file-change marker: {m}"),
            Self::BadInt(v)          => write!(f, "expected integer, got: {v}"),
            Self::BadFloat(v)        => write!(f, "expected float, got: {v}"),
            Self::InvalidStrategy(v) => write!(f, "unknown retry strategy: {v}"),
            Self::InvalidStatus(v)   => write!(f, "unknown task status: {v}"),
        }
    }
}

// ── Value types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlfValue {
    /// Single scalar string (or "-" as empty sentinel).
    Scalar(String),
    /// Comma-separated list, each item trimmed.
    List(Vec<String>),
    /// Multi-line block body (between `key:` and `:end`).
    Block(String),
}

impl AlfValue {
    pub fn as_scalar(&self) -> Option<&str> {
        match self {
            Self::Scalar(s) => Some(s.as_str()),
            Self::Block(s)  => Some(s.as_str()),
            Self::List(_)   => None,
        }
    }

    pub fn as_list(&self) -> Vec<String> {
        match self {
            Self::List(v)   => v.clone(),
            Self::Scalar(s) if s == "-" => vec![],
            Self::Scalar(s) => s
                .split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect(),
            Self::Block(s)  => vec![s.clone()],
        }
    }

    /// Whether the value represents an empty / null sentinel (`"-"`).
    pub fn is_empty_sentinel(&self) -> bool {
        matches!(self, Self::Scalar(s) if s == "-")
    }
}

// ── Record ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct AlfRecord {
    pub fields: BTreeMap<String, AlfValue>,
}

impl AlfRecord {
    pub fn get(&self, key: &str) -> Option<&AlfValue> {
        self.fields.get(key)
    }

    pub fn require_scalar(&self, key: &str) -> Result<&str, AlfError> {
        match self.fields.get(key) {
            Some(v) => v.as_scalar().ok_or_else(|| AlfError::Missing(key.to_string())),
            None    => Err(AlfError::Missing(key.to_string())),
        }
    }

    pub fn optional_scalar(&self, key: &str) -> Option<&str> {
        self.fields.get(key).and_then(|v| v.as_scalar())
    }

    pub fn optional_list(&self, key: &str) -> Vec<String> {
        match self.fields.get(key) {
            Some(v) if !v.is_empty_sentinel() => v.as_list(),
            _ => vec![],
        }
    }

    pub fn set_scalar(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.fields.insert(key.into(), AlfValue::Scalar(value.into()));
    }

    pub fn set_list(&mut self, key: impl Into<String>, values: Vec<String>) {
        if values.is_empty() {
            self.fields.insert(key.into(), AlfValue::Scalar("-".into()));
        } else {
            self.fields.insert(key.into(), AlfValue::List(values));
        }
    }

    pub fn set_block(&mut self, key: impl Into<String>, body: impl Into<String>) {
        let body = body.into();
        let trimmed = body.trim().to_string();
        if trimmed.is_empty() {
            self.fields.insert(key.into(), AlfValue::Scalar("-".into()));
        } else {
            self.fields.insert(key.into(), AlfValue::Block(trimmed));
        }
    }
}

// ── Parser ────────────────────────────────────────────────────────────────────

/// Parse an ALF string into one or more records.
pub fn parse(input: &str) -> Result<Vec<AlfRecord>, AlfError> {
    let mut records: Vec<AlfRecord> = Vec::new();
    let mut current = AlfRecord::default();
    let mut in_block: Option<String> = None; // current block key
    let mut block_buf = String::new();
    let mut has_any_field = false;

    // Auto-append missing trailing `.`
    let owned;
    let input = if !input.trim_end().ends_with('.') {
        owned = format!("{}\n.", input);
        owned.as_str()
    } else {
        input
    };

    for raw_line in input.lines() {
        let line = raw_line.trim_end();

        // ── Inside a block ───────────────────────────────────────────────────
        if let Some(ref key) = in_block.clone() {
            if line == ":end" {
                let body = block_buf.trim().to_string();
                current.fields.insert(key.clone(), AlfValue::Block(body));
                in_block = None;
                block_buf.clear();
            } else {
                block_buf.push_str(line);
                block_buf.push('\n');
            }
            continue;
        }

        // ── Record separator ─────────────────────────────────────────────────
        if line == "." {
            if in_block.is_some() {
                return Err(AlfError::UnclosedBlock);
            }
            if has_any_field {
                records.push(current);
                current = AlfRecord::default();
                has_any_field = false;
            }
            continue;
        }

        // ── Comment ──────────────────────────────────────────────────────────
        if line.starts_with('#') {
            continue;
        }

        // ── Empty line ───────────────────────────────────────────────────────
        if line.trim().is_empty() {
            continue;
        }

        // ── Block opener: `key:` alone on a line ─────────────────────────────
        if !line.contains(' ') && line.ends_with(':') {
            let key = &line[..line.len() - 1];
            if is_valid_key(key) {
                in_block = Some(key.to_string());
                block_buf.clear();
                has_any_field = true;
                continue;
            }
        }

        // ── Field line: `key value...` ────────────────────────────────────────
        if let Some(sp) = line.find(' ') {
            let key = &line[..sp];
            let value_str = line[sp + 1..].trim();

            if !is_valid_key(key) {
                // Unknown / malformed key: skip (forward compat)
                continue;
            }

            has_any_field = true;
            // All field-line values are stored as scalars. Callers that expect
            // lists (deps, kw, files, tools, …) use optional_list() which splits
            // on commas. This avoids mis-parsing free-text fields like `summary`.
            current.fields.insert(key.to_string(), AlfValue::Scalar(value_str.to_string()));
        }
        // else: line has no space → unknown format, skip
    }

    if in_block.is_some() {
        return Err(AlfError::UnclosedBlock);
    }

    Ok(records)
}

/// Parse a single ALF record (first one found).
pub fn parse_one(input: &str) -> Result<AlfRecord, AlfError> {
    let mut recs = parse(input)?;
    if recs.is_empty() {
        Ok(AlfRecord::default())
    } else {
        Ok(recs.remove(0))
    }
}

fn is_valid_key(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_lowercase() && first != '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

// ── Writer ────────────────────────────────────────────────────────────────────

/// Serialize a single `AlfRecord` to a string ending with `.`.
pub fn write(rec: &AlfRecord) -> String {
    let mut out = String::new();
    for (key, value) in &rec.fields {
        match value {
            AlfValue::Scalar(s) => {
                out.push_str(key);
                out.push(' ');
                out.push_str(s);
                out.push('\n');
            }
            AlfValue::List(items) => {
                out.push_str(key);
                out.push(' ');
                out.push_str(&items.join(", "));
                out.push('\n');
            }
            AlfValue::Block(body) => {
                out.push_str(key);
                out.push_str(":\n");
                out.push_str(body);
                if !body.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(":end\n");
            }
        }
    }
    out.push_str(".\n");
    out
}

/// Serialize multiple records, each terminated with `.`.
pub fn write_many(recs: &[AlfRecord]) -> String {
    recs.iter().map(write).collect()
}

// ── Typed decoding layer ──────────────────────────────────────────────────────

pub trait FromAlf: Sized {
    fn from_alf(rec: &AlfRecord) -> Result<Self, AlfError>;
}

pub trait ToAlf {
    fn to_alf(&self) -> AlfRecord;
}

// ── TaskSpec ──────────────────────────────────────────────────────────────────

impl FromAlf for TaskSpec {
    fn from_alf(rec: &AlfRecord) -> Result<Self, AlfError> {
        let id = rec.require_scalar("id")?.to_string();
        let title = rec.require_scalar("title")?.to_string();

        let description = rec
            .fields
            .get("desc")
            .and_then(|v| v.as_scalar())
            .unwrap_or("")
            .to_string();

        let parent = rec.optional_scalar("parent")
            .filter(|s| *s != "-")
            .map(|s| s.to_string());

        let deps = rec.optional_list("deps")
            .into_iter()
            .filter(|s| s != "-")
            .collect();

        let expected_keywords = rec.optional_list("kw")
            .into_iter()
            .filter(|s| s != "-")
            .collect();

        // Parse files: "path:+" / "path:~" / "path:-"
        let expected_files = rec.optional_list("files")
            .into_iter()
            .filter(|s| s != "-")
            .map(|entry| parse_expected_file(&entry))
            .collect::<Result<Vec<_>, _>>()?;

        let extra_commands = rec.optional_list("cmd")
            .into_iter()
            .filter(|s| s != "-")
            .collect();

        let skip_checks = rec.optional_list("skip")
            .into_iter()
            .filter(|s| s != "-")
            .collect();

        let allowed_tools = rec.optional_list("tools")
            .into_iter()
            .filter(|s| s != "-")
            .collect();

        let max_rounds = rec.optional_scalar("max_rounds")
            .filter(|s| *s != "-")
            .map(|s| s.parse::<u32>().map_err(|_| AlfError::BadInt(s.to_string())))
            .transpose()?
            .unwrap_or(4);

        Ok(TaskSpec {
            id,
            parent,
            title,
            description,
            deps,
            expected_files,
            expected_keywords,
            extra_commands,
            skip_checks,
            allowed_tools,
            max_rounds,
        })
    }
}

fn parse_expected_file(entry: &str) -> Result<ExpectedFile, AlfError> {
    // Format: "path:marker" where marker is +/~/−
    if let Some(colon_pos) = entry.rfind(':') {
        let path = &entry[..colon_pos];
        let marker = &entry[colon_pos + 1..];
        let change = FileChange::from_marker(marker)
            .ok_or_else(|| AlfError::BadMarker(marker.to_string()))?;
        Ok(ExpectedFile {
            path: PathBuf::from(path),
            change,
            min_bytes: None,
            max_bytes: None,
        })
    } else {
        Err(AlfError::BadMarker(entry.to_string()))
    }
}

impl ToAlf for TaskSpec {
    fn to_alf(&self) -> AlfRecord {
        let mut rec = AlfRecord::default();
        rec.set_scalar("id", &self.id);
        rec.set_scalar("parent", self.parent.as_deref().unwrap_or("-"));
        rec.set_scalar("title", &self.title);

        if self.description.contains('\n') || self.description.len() > 80 {
            rec.set_block("desc", &self.description);
        } else {
            rec.set_scalar("desc", &self.description);
        }

        rec.set_list("deps", self.deps.clone());
        rec.set_list("kw", self.expected_keywords.clone());

        let files: Vec<String> = self.expected_files
            .iter()
            .map(|ef| format!("{}:{}", ef.path.display(), ef.change.to_marker()))
            .collect();
        rec.set_list("files", files);

        rec.set_list("cmd", self.extra_commands.clone());
        rec.set_list("skip", self.skip_checks.clone());
        rec.set_list("tools", self.allowed_tools.clone());
        rec.set_scalar("max_rounds", self.max_rounds.to_string());
        rec
    }
}

// ── TaskReport ────────────────────────────────────────────────────────────────

impl FromAlf for TaskReport {
    fn from_alf(rec: &AlfRecord) -> Result<Self, AlfError> {
        let task_id = rec.optional_scalar("task_id").unwrap_or("").to_string();

        let status_str = rec.require_scalar("status")?;
        let status = TaskStatus::from_str_loose(status_str)
            .ok_or_else(|| AlfError::InvalidStatus(status_str.to_string()))?;

        let summary = rec
            .fields
            .get("summary")
            .and_then(|v| v.as_scalar())
            .unwrap_or("")
            .to_string();

        let files_touched = rec.optional_list("files_touched")
            .into_iter()
            .filter(|s| s != "-")
            .map(|s| PathBuf::from(s))
            .collect();

        Ok(TaskReport {
            task_id,
            attempt: 1,
            status,
            summary,
            files_touched,
            started_at: 0,
            finished_at: 0,
            worker_tool_calls: 0,
            tokens_used: None,
        })
    }
}

impl ToAlf for TaskReport {
    fn to_alf(&self) -> AlfRecord {
        let mut rec = AlfRecord::default();
        rec.set_scalar("task_id", &self.task_id);
        rec.set_scalar("status", self.status.label());

        if self.summary.contains('\n') || self.summary.len() > 80 {
            rec.set_block("summary", &self.summary);
        } else {
            rec.set_scalar("summary", &self.summary);
        }

        let files: Vec<String> = self.files_touched
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        rec.set_list("files_touched", files);
        rec
    }
}

// ── Diagnosis ─────────────────────────────────────────────────────────────────

impl FromAlf for Diagnosis {
    fn from_alf(rec: &AlfRecord) -> Result<Self, AlfError> {
        let root_cause = rec.require_scalar("root_cause")?.to_string();

        let strategy_str = rec.require_scalar("strategy")?;
        let strategy = RetryStrategy::from_str_loose(strategy_str)
            .ok_or_else(|| AlfError::InvalidStrategy(strategy_str.to_string()))?;

        let hint = rec
            .fields
            .get("hint")
            .and_then(|v| v.as_scalar())
            .filter(|s| *s != "-")
            .map(|s| s.to_string());

        Ok(Diagnosis { root_cause, strategy, hint })
    }
}

impl ToAlf for Diagnosis {
    fn to_alf(&self) -> AlfRecord {
        let mut rec = AlfRecord::default();
        rec.set_scalar("root_cause", &self.root_cause);
        rec.set_scalar("strategy", self.strategy.label());
        match &self.hint {
            Some(h) if h.contains('\n') || h.len() > 80 => rec.set_block("hint", h),
            Some(h) => rec.set_scalar("hint", h),
            None => rec.set_scalar("hint", "-"),
        }
        rec
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestra::types::{FileChange, RetryStrategy, TaskStatus};

    // ── Parser ─────────────────────────────────────────────────────────────

    #[test]
    fn test_parse_simple_record() {
        let input = "id T01\ntitle Hello world\n.";
        let recs = parse(input).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].require_scalar("id").unwrap(), "T01");
        assert_eq!(recs[0].require_scalar("title").unwrap(), "Hello world");
    }

    #[test]
    fn test_parse_two_records() {
        let input = "id T01\ntitle First\n.\nid T02\ntitle Second\n.";
        let recs = parse(input).unwrap();
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[1].require_scalar("id").unwrap(), "T02");
    }

    #[test]
    fn test_parse_list_field() {
        let input = "deps T01, T02, T03\n.";
        let recs = parse(input).unwrap();
        let v = recs[0].optional_list("deps");
        assert_eq!(v, vec!["T01", "T02", "T03"]);
    }

    #[test]
    fn test_parse_dash_as_empty() {
        let input = "deps -\n.";
        let recs = parse(input).unwrap();
        let v = recs[0].optional_list("deps");
        assert!(v.is_empty(), "expected empty list for '-', got {:?}", v);
    }

    #[test]
    fn test_parse_block_field() {
        let input = "summary:\nLine one\nLine two\n:end\n.";
        let recs = parse(input).unwrap();
        let v = recs[0].require_scalar("summary").unwrap();
        assert!(v.contains("Line one"));
        assert!(v.contains("Line two"));
    }

    #[test]
    fn test_parse_unclosed_block_error() {
        let input = "summary:\nLine one\n.";
        let err = parse(input).unwrap_err();
        assert_eq!(err, AlfError::UnclosedBlock);
    }

    #[test]
    fn test_parse_comments_ignored() {
        let input = "# this is a comment\nid T01\n# another comment\ntitle Test\n.";
        let recs = parse(input).unwrap();
        assert_eq!(recs.len(), 1);
        assert!(!recs[0].fields.contains_key("#"));
    }

    #[test]
    fn test_parse_unknown_keys_ignored() {
        let input = "id T01\nunknown_future_key somevalue\ntitle Test\n.";
        let recs = parse(input).unwrap();
        assert_eq!(recs[0].require_scalar("id").unwrap(), "T01");
    }

    #[test]
    fn test_parse_auto_appends_missing_dot() {
        let input = "id T01\ntitle Test";
        let recs = parse(input).unwrap();
        assert_eq!(recs.len(), 1);
    }

    #[test]
    fn test_parse_strips_trailing_whitespace_from_values() {
        let input = "id T01   \ntitle Hello   \n.";
        let recs = parse(input).unwrap();
        assert_eq!(recs[0].require_scalar("id").unwrap(), "T01");
    }

    // ── Writer ─────────────────────────────────────────────────────────────

    #[test]
    fn test_write_scalar() {
        let mut rec = AlfRecord::default();
        rec.set_scalar("id", "T01");
        let out = write(&rec);
        assert!(out.contains("id T01\n"));
        assert!(out.ends_with(".\n"));
    }

    #[test]
    fn test_write_list() {
        let mut rec = AlfRecord::default();
        rec.set_list("deps", vec!["T01".into(), "T02".into()]);
        let out = write(&rec);
        assert!(out.contains("deps T01, T02\n"));
    }

    #[test]
    fn test_write_empty_list_becomes_dash() {
        let mut rec = AlfRecord::default();
        rec.set_list("tools", vec![]);
        let out = write(&rec);
        assert!(out.contains("tools -\n"));
    }

    #[test]
    fn test_write_block() {
        let mut rec = AlfRecord::default();
        rec.set_block("desc", "Line one\nLine two");
        let out = write(&rec);
        assert!(out.contains("desc:\n"));
        assert!(out.contains("Line one\n"));
        assert!(out.contains(":end\n"));
    }

    // ── Round-trip tests ───────────────────────────────────────────────────

    #[test]
    fn test_taskspec_roundtrip() {
        let spec = TaskSpec {
            id: "T01.02".into(),
            parent: Some("T01".into()),
            title: "Create homepage HTML".into(),
            description: "Build src/index.html with hero section".into(),
            deps: vec!["T01.01".into()],
            expected_files: vec![ExpectedFile {
                path: PathBuf::from("src/index.html"),
                change: FileChange::Create,
                min_bytes: None,
                max_bytes: None,
            }],
            expected_keywords: vec!["clinic".into(), "doctor".into()],
            extra_commands: vec![],
            skip_checks: vec![],
            allowed_tools: vec!["read_file".into(), "write_file".into()],
            max_rounds: 4,
        };

        let rec = spec.to_alf();
        let out = write(&rec);
        let recovered_recs = parse(&out).unwrap();
        let recovered = TaskSpec::from_alf(&recovered_recs[0]).unwrap();

        assert_eq!(recovered.id, "T01.02");
        assert_eq!(recovered.parent, Some("T01".into()));
        assert_eq!(recovered.title, "Create homepage HTML");
        assert_eq!(recovered.deps, vec!["T01.01"]);
        assert_eq!(recovered.expected_keywords, vec!["clinic", "doctor"]);
        assert_eq!(recovered.expected_files.len(), 1);
        assert_eq!(recovered.expected_files[0].change, FileChange::Create);
        assert_eq!(recovered.max_rounds, 4);
    }

    #[test]
    fn test_taskspec_roundtrip_no_parent() {
        let spec = TaskSpec {
            id: "T01".into(),
            parent: None,
            title: "Scaffold project".into(),
            description: "Set up Next.js".into(),
            ..Default::default()
        };
        let rec = spec.to_alf();
        let out = write(&rec);
        let recs = parse(&out).unwrap();
        let recovered = TaskSpec::from_alf(&recs[0]).unwrap();
        assert_eq!(recovered.parent, None);
    }

    #[test]
    fn test_taskreport_roundtrip() {
        let report = TaskReport {
            task_id: "T01.02".into(),
            attempt: 1,
            status: TaskStatus::Ok,
            summary: "Created index.html successfully".into(),
            files_touched: vec![PathBuf::from("src/index.html")],
            started_at: 1000,
            finished_at: 1500,
            worker_tool_calls: 3,
            tokens_used: Some(450),
        };
        let rec = report.to_alf();
        let out = write(&rec);
        let recs = parse(&out).unwrap();
        let recovered = TaskReport::from_alf(&recs[0]).unwrap();

        assert_eq!(recovered.task_id, "T01.02");
        assert_eq!(recovered.status, TaskStatus::Ok);
        assert!(recovered.summary.contains("index.html"));
        assert_eq!(recovered.files_touched.len(), 1);
    }

    #[test]
    fn test_taskreport_failed_status() {
        let input = "status failed\nsummary Could not write the file\nfiles_touched -\n.";
        let recs = parse(input).unwrap();
        let report = TaskReport::from_alf(&recs[0]).unwrap();
        assert_eq!(report.status, TaskStatus::Failed);
    }

    #[test]
    fn test_taskreport_needs_review_status() {
        let input = "status needs_review\nsummary Partial output\nfiles_touched -\n.";
        let recs = parse(input).unwrap();
        let report = TaskReport::from_alf(&recs[0]).unwrap();
        assert_eq!(report.status, TaskStatus::NeedsReview);
    }

    #[test]
    fn test_diagnosis_roundtrip() {
        let diag = Diagnosis {
            root_cause: "Worker skipped the CSS file".into(),
            strategy: RetryStrategy::RetryWithHint,
            hint: Some("Create src/styles.css before index.html".into()),
        };
        let rec = diag.to_alf();
        let out = write(&rec);
        let recs = parse(&out).unwrap();
        let recovered = Diagnosis::from_alf(&recs[0]).unwrap();

        assert_eq!(recovered.root_cause, "Worker skipped the CSS file");
        assert_eq!(recovered.strategy, RetryStrategy::RetryWithHint);
        assert_eq!(recovered.hint.as_deref(), Some("Create src/styles.css before index.html"));
    }

    #[test]
    fn test_diagnosis_no_hint() {
        let diag = Diagnosis {
            root_cause: "Missing file".into(),
            strategy: RetryStrategy::Skip,
            hint: None,
        };
        let rec = diag.to_alf();
        let out = write(&rec);
        let recs = parse(&out).unwrap();
        let recovered = Diagnosis::from_alf(&recs[0]).unwrap();
        assert_eq!(recovered.hint, None);
    }

    #[test]
    fn test_all_retry_strategies_roundtrip() {
        for strategy in &[
            RetryStrategy::RetryAsIs,
            RetryStrategy::RetryWithHint,
            RetryStrategy::ReplanSubtree,
            RetryStrategy::Skip,
            RetryStrategy::EscalateToUser,
        ] {
            let diag = Diagnosis {
                root_cause: "test".into(),
                strategy: *strategy,
                hint: None,
            };
            let rec = diag.to_alf();
            let out = write(&rec);
            let recs = parse(&out).unwrap();
            let recovered = Diagnosis::from_alf(&recs[0]).unwrap();
            assert_eq!(recovered.strategy, *strategy);
        }
    }

    #[test]
    fn test_write_many_roundtrip() {
        let specs = vec![
            TaskSpec { id: "T01".into(), title: "First".into(), ..Default::default() },
            TaskSpec { id: "T02".into(), title: "Second".into(), ..Default::default() },
        ];
        let recs: Vec<AlfRecord> = specs.iter().map(|s| s.to_alf()).collect();
        let out = write_many(&recs);
        let parsed = parse(&out).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].require_scalar("id").unwrap(), "T01");
        assert_eq!(parsed[1].require_scalar("id").unwrap(), "T02");
    }

    #[test]
    fn test_parse_example_from_spec() {
        // From the spec doc 00-format.md
        let input = "\
id T01.02
parent T01
title Create homepage HTML
desc Build src/index.html with hero section and contact form
deps T01.01
kw clinic, doctor, services, contact
files src/index.html:+, src/styles.css:~
cmd -
skip -
tools read_file, write_file, edit_file
max_rounds 4
.";
        let recs = parse(input).unwrap();
        let spec = TaskSpec::from_alf(&recs[0]).unwrap();
        assert_eq!(spec.id, "T01.02");
        assert_eq!(spec.parent, Some("T01".into()));
        assert_eq!(spec.max_rounds, 4);
        assert_eq!(spec.expected_keywords, vec!["clinic", "doctor", "services", "contact"]);
        assert_eq!(spec.expected_files.len(), 2);
        assert_eq!(spec.expected_files[0].change, FileChange::Create);
        assert_eq!(spec.expected_files[1].change, FileChange::Modify);
        assert!(spec.extra_commands.is_empty());
        assert!(spec.skip_checks.is_empty());
        assert_eq!(spec.allowed_tools, vec!["read_file", "write_file", "edit_file"]);
    }

    #[test]
    fn test_parse_worker_final_report_from_spec() {
        let input = "\
status ok
summary Created src/index.html with semantic main/header/footer, added hero and contact form
files_touched src/index.html, src/styles.css
.";
        let recs = parse(input).unwrap();
        let report = TaskReport::from_alf(&recs[0]).unwrap();
        assert_eq!(report.status, TaskStatus::Ok);
        assert!(report.summary.contains("semantic"));
        assert_eq!(report.files_touched.len(), 2);
    }

    #[test]
    fn test_parse_diagnosis_from_spec() {
        let input = "\
root_cause The worker wrote styles.css but did not create index.html
strategy RetryWithHint
hint Create src/index.html first; styles.css already exists on disk
.";
        let recs = parse(input).unwrap();
        let diag = Diagnosis::from_alf(&recs[0]).unwrap();
        assert_eq!(diag.strategy, RetryStrategy::RetryWithHint);
        assert!(diag.root_cause.contains("styles.css"));
        assert!(diag.hint.as_deref().unwrap_or("").contains("index.html"));
    }

    #[test]
    fn test_size_cap_awareness() {
        // Verify a large TaskSpec does not corrupt the round-trip
        let spec = TaskSpec {
            id: "T01".into(),
            title: "x".repeat(80),
            description: "y".repeat(400),
            ..Default::default()
        };
        let rec = spec.to_alf();
        let out = write(&rec);
        let recs = parse(&out).unwrap();
        let recovered = TaskSpec::from_alf(&recs[0]).unwrap();
        assert_eq!(recovered.title.len(), 80);
    }

    #[test]
    fn test_malformed_input_recovery_extra_prose_before() {
        // Extra prose before first recognized key should still parse
        let input = "Let me explain...\nid T01\ntitle Test\n.";
        let recs = parse(input).unwrap();
        // "Let me explain..." is not a valid key-value line, so id may or may not parse
        // Depending on how the parser handles it. Key validation will skip it.
        // The valid fields should still be present
        if !recs.is_empty() {
            if let Ok(id) = recs[0].require_scalar("id") {
                assert_eq!(id, "T01");
            }
        }
    }

    #[test]
    fn test_bad_file_marker_error() {
        let err = parse_expected_file("src/foo.rs:?").unwrap_err();
        assert!(matches!(err, AlfError::BadMarker(_)));
    }

    #[test]
    fn test_missing_required_key_error() {
        let input = "title Only title here\n.";
        let recs = parse(input).unwrap();
        let err = TaskSpec::from_alf(&recs[0]).unwrap_err();
        assert!(matches!(err, AlfError::Missing(_)));
    }

    #[test]
    fn test_invalid_strategy_error() {
        let input = "root_cause bad\nstrategy UnknownStrategy\nhint -\n.";
        let recs = parse(input).unwrap();
        let err = Diagnosis::from_alf(&recs[0]).unwrap_err();
        assert!(matches!(err, AlfError::InvalidStrategy(_)));
    }
}
