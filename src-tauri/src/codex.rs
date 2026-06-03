// Codex CLI usage fetcher.
// Data source: ~/.codex/auth.json (access_token + refresh_token written by Codex CLI).
// Endpoint: GET https://chatgpt.com/backend-api/wham/usage
// Auth: Authorization: Bearer <access_token>
// No WebKit / browser cookie needed — the JWT is available locally.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const WHAM_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Safari/605.1.15";

// --- Error -------------------------------------------------------------------

#[derive(Debug)]
pub enum FetchError {
    NoAuthFile,
    ReadAuth(String),
    ParseAuth(String),
    Network(String),
    Unauthorized,
    BadResponse(String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchError::NoAuthFile => write!(f, "~/.codex/auth.json 不存在（未安装 Codex CLI？）"),
            FetchError::ReadAuth(e) => write!(f, "读取 auth.json 失败: {e}"),
            FetchError::ParseAuth(e) => write!(f, "解析 auth.json 失败: {e}"),
            FetchError::Network(e) => write!(f, "Network error: {e}"),
            FetchError::Unauthorized => write!(f, "Token 已过期，请重新登录 Codex CLI"),
            FetchError::BadResponse(s) => write!(f, "Unexpected response: {s}"),
        }
    }
}

impl std::error::Error for FetchError {}

// --- Auth file ---------------------------------------------------------------

#[derive(Deserialize)]
struct AuthFile {
    tokens: AuthTokens,
}

#[derive(Deserialize)]
struct AuthTokens {
    access_token: String,
}

fn auth_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".codex/auth.json")
}

fn read_access_token() -> Result<String, FetchError> {
    let path = auth_path();
    if !path.exists() {
        return Err(FetchError::NoAuthFile);
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| FetchError::ReadAuth(e.to_string()))?;
    let auth: AuthFile = serde_json::from_str(&text)
        .map_err(|e| FetchError::ParseAuth(e.to_string()))?;
    Ok(auth.tokens.access_token)
}

// --- API response shapes -----------------------------------------------------

#[derive(Deserialize)]
struct WhamResponse {
    rate_limit: Option<WhamRateLimit>,
    plan_type: Option<String>,
    email: Option<String>,
}

#[derive(Deserialize)]
struct WhamRateLimit {
    primary_window: Option<WhamWindow>,
    secondary_window: Option<WhamWindow>,
}

#[derive(Deserialize)]
struct WhamWindow {
    used_percent: Option<f64>,
    limit_window_seconds: Option<u64>,
    reset_after_seconds: Option<u64>,
    reset_at: Option<u64>,          // unix timestamp
}

// --- Public types ------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct CodexWindow {
    pub percent: f64,
    pub window_label: String,       // "5h" / "7d"
    pub resets_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodexUsage {
    pub primary: Option<CodexWindow>,   // 5-hour
    pub secondary: Option<CodexWindow>, // 7-day
    pub plan_type: Option<String>,
    pub email: Option<String>,
    pub fetched_at: DateTime<Utc>,
}

fn window_label(limit_secs: Option<u64>) -> String {
    match limit_secs {
        Some(s) if s <= 7200 => "2h".into(),
        Some(s) if s <= 21600 => "5h".into(),
        Some(s) if s <= 86400 => "1d".into(),
        Some(_) => "7d".into(),
        None => "?".into(),
    }
}

fn unix_to_dt(ts: u64) -> DateTime<Utc> {
    DateTime::from_timestamp(ts as i64, 0).unwrap_or_else(|| Utc::now())
}

fn wham_to_window(w: WhamWindow) -> CodexWindow {
    let pct = w.used_percent.unwrap_or(0.0);
    let label = window_label(w.limit_window_seconds);

    // Prefer explicit reset_at; fall back to now + reset_after_seconds.
    let resets_at = if let Some(ts) = w.reset_at {
        Some(unix_to_dt(ts))
    } else if let Some(after) = w.reset_after_seconds {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Some(unix_to_dt(now + after))
    } else {
        None
    };

    CodexWindow { percent: pct, window_label: label, resets_at }
}

// --- Main fetch --------------------------------------------------------------

pub async fn fetch() -> Result<CodexUsage, FetchError> {
    let token = read_access_token()?;

    let client = wreq::Client::builder()
        .emulation(wreq_util::Emulation::Safari18_5)
        .build()
        .map_err(|e| FetchError::Network(e.to_string()))?;

    let resp = client
        .get(WHAM_URL)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
        .header("User-Agent", USER_AGENT)
        .header("Referer", "https://chatgpt.com/")
        .send()
        .await
        .map_err(|e| FetchError::Network(e.to_string()))?;

    let status = resp.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(FetchError::Unauthorized);
    }
    if !status.is_success() {
        return Err(FetchError::BadResponse(format!("HTTP {status}")));
    }

    let raw: WhamResponse = resp
        .json()
        .await
        .map_err(|e| FetchError::BadResponse(format!("parse: {e}")))?;

    let rl = raw.rate_limit.unwrap_or(WhamRateLimit {
        primary_window: None,
        secondary_window: None,
    });

    Ok(CodexUsage {
        primary: rl.primary_window.map(wham_to_window),
        secondary: rl.secondary_window.map(wham_to_window),
        plan_type: raw.plan_type,
        email: raw.email,
        fetched_at: Utc::now(),
    })
}
