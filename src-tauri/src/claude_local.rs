// Parse Claude Code's local conversation log to compute token usage history.
// Files live at ~/.claude/projects/<dir>/*.jsonl, one JSON event per line.
// We only care about assistant messages: { type: "assistant", message: { model, usage: {...} }, timestamp }.

use chrono::{DateTime, Datelike, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct LineRecord {
    #[serde(default)]
    #[serde(rename = "type")]
    record_type: Option<String>,
    timestamp: Option<String>,
    message: Option<MessagePart>,
}

#[derive(Debug, Deserialize)]
struct MessagePart {
    #[serde(default)]
    model: Option<String>,
    usage: Option<UsagePart>,
}

#[derive(Debug, Deserialize)]
struct UsagePart {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct DailyBucket {
    pub date: String,           // YYYY-MM-DD
    pub input: u64,
    pub output: u64,
    pub cache_create: u64,
    pub cache_read: u64,
}

impl DailyBucket {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_create + self.cache_read
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ClaudeTokenSummary {
    /// Today's totals (UTC date).
    pub today: DailyBucket,
    /// Sum across the past 7 days (today inclusive).
    pub last_7_days_total: u64,
    /// Per-day buckets for the last 7 days (oldest first).
    pub daily: Vec<DailyBucket>,
    /// Top models (descending token total) for the 7-day window.
    pub top_models: Vec<(String, u64)>,
    pub fetched_at: DateTime<Utc>,
}

fn projects_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".claude/projects")
}

fn list_jsonl_files(root: &PathBuf) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(root) else { return files };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Ok(sub) = fs::read_dir(&path) {
                for sub_entry in sub.flatten() {
                    let p = sub_entry.path();
                    if p.extension().map(|e| e == "jsonl").unwrap_or(false) {
                        files.push(p);
                    }
                }
            }
        } else if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
            files.push(path);
        }
    }
    files
}

/// Scan all `~/.claude/projects/**/*.jsonl` and aggregate token usage for the
/// last 7 days. Stateless — re-reads everything each call.
/// Benchmarked at ~70 ms for 3500 lines / 11 MB on M-series Mac.
pub fn scan_token_history() -> ClaudeTokenSummary {
    let root = projects_dir();
    let files = list_jsonl_files(&root);

    let now = Utc::now();
    let today = now.date_naive();
    let cutoff = now - Duration::days(7);

    // date(YYYY-MM-DD) -> bucket
    let mut by_day: BTreeMap<String, DailyBucket> = BTreeMap::new();
    // model -> total tokens
    let mut by_model: std::collections::HashMap<String, u64> =
        std::collections::HashMap::new();

    for file in files {
        let Ok(content) = fs::read_to_string(&file) else { continue };
        for line in content.lines() {
            if line.is_empty() {
                continue;
            }
            let Ok(rec) = serde_json::from_str::<LineRecord>(line) else { continue };
            if rec.record_type.as_deref() != Some("assistant") {
                continue;
            }
            let Some(ts) = rec.timestamp else { continue };
            let Ok(dt) = DateTime::parse_from_rfc3339(&ts) else { continue };
            let dt_utc = dt.with_timezone(&Utc);
            if dt_utc < cutoff {
                continue;
            }
            let Some(msg) = rec.message else { continue };
            let Some(usage) = msg.usage else { continue };

            let date_key = dt_utc.date_naive().to_string();
            let entry = by_day.entry(date_key.clone()).or_default();
            entry.date = date_key;
            entry.input += usage.input_tokens;
            entry.output += usage.output_tokens;
            entry.cache_create += usage.cache_creation_input_tokens;
            entry.cache_read += usage.cache_read_input_tokens;

            if let Some(model) = msg.model {
                let total = usage.input_tokens
                    + usage.output_tokens
                    + usage.cache_creation_input_tokens
                    + usage.cache_read_input_tokens;
                *by_model.entry(model).or_insert(0) += total;
            }
        }
    }

    // Pad missing days with zero buckets so the timeline is continuous.
    let mut daily: Vec<DailyBucket> = Vec::with_capacity(7);
    for offset in (0..7).rev() {
        let d = (now - Duration::days(offset)).date_naive().to_string();
        daily.push(by_day.remove(&d).unwrap_or(DailyBucket {
            date: d,
            ..Default::default()
        }));
    }

    let today_bucket = daily
        .iter()
        .find(|b| b.date == today.to_string())
        .cloned()
        .unwrap_or_default();

    let last_7_days_total: u64 = daily.iter().map(|b| b.total()).sum();

    let mut top_models: Vec<(String, u64)> = by_model.into_iter().collect();
    top_models.sort_by(|a, b| b.1.cmp(&a.1));
    top_models.truncate(5);

    ClaudeTokenSummary {
        today: today_bucket,
        last_7_days_total,
        daily,
        top_models,
        fetched_at: now,
    }
}
