// Parse Codex CLI's local state_5.sqlite to compute token usage history.
// Table: threads (tokens_used INTEGER, model TEXT, created_at_ms INTEGER)
// Mirrors the pattern used in claude_local.rs but via SQLite instead of JSONL.

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use std::path::PathBuf;

fn db_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".codex/state_5.sqlite")
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct CodexDailyBucket {
    pub date: String,
    pub tokens: u64,
    pub sessions: u32,
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

pub fn scan_token_history() -> Option<CodexLocalSummary> {
    let path = db_path();
    if !path.exists() {
        return None;
    }

    // rusqlite is not in our deps — use the sqlite3 CLI as a thin shim.
    // This avoids adding a heavy C dependency; the binary is always present on macOS.
    let now = Utc::now();
    let today = now.date_naive().to_string();
    let cutoff_ms = (now - Duration::days(7)).timestamp_millis();

    let query = format!(
        "SELECT date(created_at_ms/1000,'unixepoch','localtime') as day, \
         COALESCE(model,'unknown') as model, \
         COUNT(*) as sessions, \
         SUM(tokens_used) as tokens \
         FROM threads \
         WHERE created_at_ms >= {cutoff_ms} AND tokens_used > 0 \
         GROUP BY day, model ORDER BY day DESC, tokens DESC;"
    );

    let output = std::process::Command::new("sqlite3")
        .arg(path.to_str().unwrap_or(""))
        .arg("-separator")
        .arg("\t")
        .arg(&query)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);

    // day_map: date -> (tokens, sessions)
    let mut day_map: std::collections::BTreeMap<String, (u64, u32)> =
        std::collections::BTreeMap::new();
    // model_map: model -> tokens
    let mut model_map: std::collections::HashMap<String, u64> =
        std::collections::HashMap::new();

    for line in text.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 4 {
            continue;
        }
        let day = parts[0].to_string();
        let model = parts[1].to_string();
        let sessions: u32 = parts[2].parse().unwrap_or(0);
        let tokens: u64 = parts[3].parse().unwrap_or(0);

        let e = day_map.entry(day).or_insert((0, 0));
        e.0 += tokens;
        e.1 += sessions;

        *model_map.entry(model).or_insert(0) += tokens;
    }

    // Build 7-day timeline (oldest → newest).
    let mut daily: Vec<CodexDailyBucket> = Vec::with_capacity(7);
    for offset in (0..7).rev() {
        let d = (now - Duration::days(offset)).date_naive().to_string();
        let (tokens, sessions) = day_map.get(&d).copied().unwrap_or((0, 0));
        daily.push(CodexDailyBucket { date: d, tokens, sessions });
    }

    let today_bucket = daily.iter().find(|b| b.date == today).cloned().unwrap_or_default();
    let last_7_days_total: u64 = daily.iter().map(|b| b.tokens).sum();

    let mut top_models: Vec<(String, u64)> = model_map.into_iter().collect();
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
