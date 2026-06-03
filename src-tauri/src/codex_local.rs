// Compute Codex CLI token-usage history from rollout transcripts.
//
// Why not state_5.sqlite? The `threads` table only stores `tokens_used`, which is
// a *cumulative lifetime total per thread*, keyed by the thread's creation date.
// Codex users keep long-lived threads open for days, so bucketing that counter by
// any single date is wrong — a thread created on the 11th but used today reports
// all of today's tokens under the 11th, leaving "today" at zero.
//
// The correct source is the per-turn event stream in the rollout JSONL files at
// ~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl. Each turn emits an event_msg of
// payload.type == "token_count" carrying:
//   payload.info.last_token_usage.total_tokens  -> the delta for THAT turn
//   timestamp                                   -> real per-turn ISO timestamp
// Summing the deltas grouped by local date gives accurate daily consumption.

use chrono::{DateTime, Duration, Local, Utc};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration as StdDuration, SystemTime};

fn sessions_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".codex/sessions")
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct CodexDailyBucket {
    pub date: String,
    pub tokens: u64,
    pub sessions: u32, // distinct rollout sessions active that day
}

#[derive(Debug, Clone, Serialize)]
pub struct CodexLocalSummary {
    pub today_tokens: u64,
    pub today_sessions: u32,
    pub last_7_days_total: u64,
    pub daily: Vec<CodexDailyBucket>,
    pub top_models: Vec<(String, u64)>,
    pub fetched_at: DateTime<Utc>,
}

/// Recursively collect `*.jsonl` rollout files modified since `cutoff`.
/// The mtime pre-filter skips closed sessions cheaply: a file last written
/// before the cutoff cannot contain any events newer than the cutoff.
fn collect_recent(root: &Path, cutoff: SystemTime, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(root) else { return };
    for entry in rd.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            collect_recent(&path, cutoff, out);
        } else if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
            if let Ok(md) = entry.metadata() {
                if md.modified().map(|m| m >= cutoff).unwrap_or(false) {
                    out.push(path);
                }
            }
        }
    }
}

/// Cheap substring extraction of the active model from a JSONL line, e.g.
/// `..."model":"gpt-5.5"...` -> Some("gpt-5.5"). Avoids a full JSON parse on
/// the many non-token_count lines.
fn extract_model(line: &str) -> Option<String> {
    let idx = line.find("\"model\":\"")?;
    let rest = &line[idx + 9..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

pub fn scan_token_history() -> Option<CodexLocalSummary> {
    let root = sessions_dir();
    if !root.exists() {
        return None;
    }

    let now = Utc::now();
    // Bucket strictly to the last 7 days by event timestamp; pre-filter files by
    // mtime with an extra day of margin so boundary sessions aren't dropped.
    let event_cutoff = now - Duration::days(7);
    let mtime_cutoff = SystemTime::now() - StdDuration::from_secs(8 * 86_400);

    let mut files = Vec::new();
    collect_recent(&root, mtime_cutoff, &mut files);

    let today = Local::now().date_naive().to_string();

    let mut by_day_tokens: BTreeMap<String, u64> = BTreeMap::new();
    let mut by_day_sessions: BTreeMap<String, u32> = BTreeMap::new();
    let mut by_model: HashMap<String, u64> = HashMap::new();
    let mut any_event = false;

    for file in &files {
        let Ok(content) = fs::read_to_string(file) else { continue };
        let mut last_model: Option<String> = None;
        // Days this particular session contributed to (for distinct-session counts).
        let mut active_days: HashSet<String> = HashSet::new();

        for line in content.lines() {
            // Track the most recent model declared in the transcript.
            if line.contains("\"model\":\"") {
                if let Some(m) = extract_model(line) {
                    last_model = Some(m);
                }
            }
            if !line.contains("\"token_count\"") {
                continue;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
            let payload = &v["payload"];
            if payload["type"] != "token_count" {
                continue;
            }
            let Some(delta) = payload["info"]["last_token_usage"]["total_tokens"].as_u64() else {
                continue;
            };
            if delta == 0 {
                continue;
            }
            let Some(ts) = v["timestamp"].as_str() else { continue };
            let Ok(parsed) = DateTime::parse_from_rfc3339(ts) else { continue };
            if parsed.with_timezone(&Utc) < event_cutoff {
                continue;
            }
            let day = parsed.with_timezone(&Local).date_naive().to_string();

            any_event = true;
            *by_day_tokens.entry(day.clone()).or_insert(0) += delta;
            active_days.insert(day);

            let model = last_model.clone().unwrap_or_else(|| "unknown".to_string());
            *by_model.entry(model).or_insert(0) += delta;
        }

        for day in active_days {
            *by_day_sessions.entry(day).or_insert(0) += 1;
        }
    }

    if !any_event {
        return None;
    }

    // Build a continuous 7-day timeline (oldest -> newest), padding empty days.
    let mut daily: Vec<CodexDailyBucket> = Vec::with_capacity(7);
    for offset in (0..7).rev() {
        let d = (Local::now() - Duration::days(offset)).date_naive().to_string();
        let tokens = by_day_tokens.get(&d).copied().unwrap_or(0);
        let sessions = by_day_sessions.get(&d).copied().unwrap_or(0);
        daily.push(CodexDailyBucket { date: d, tokens, sessions });
    }

    let today_bucket = daily.iter().find(|b| b.date == today).cloned().unwrap_or_default();
    let last_7_days_total: u64 = daily.iter().map(|b| b.tokens).sum();

    let mut top_models: Vec<(String, u64)> = by_model.into_iter().collect();
    top_models.sort_by(|a, b| b.1.cmp(&a.1));
    top_models.truncate(3);

    Some(CodexLocalSummary {
        today_tokens: today_bucket.tokens,
        today_sessions: today_bucket.sessions,
        last_7_days_total,
        daily,
        top_models,
        fetched_at: now,
    })
}
