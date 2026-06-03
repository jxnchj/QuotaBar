// Kimi (kimi.com) web-subscription usage fetcher.
//
// Faithful port of CodexBar's Kimi provider (the recipe the user confirmed works):
//   POST https://www.kimi.com/apiv2/kimi.gateway.billing.v1.BillingService/GetUsages
//   Auth: the `kimi-auth` JWT, sent BOTH as `Authorization: Bearer <jwt>` and
//         `Cookie: kimi-auth=<jwt>`.
//   Plus session headers decoded from the JWT payload (device_id/ssid/sub).
//   Body: {"scope": ["FEATURE_CODING"]}.
// Response carries the coding subscription's weekly request quota (primary) and a
// 5-hour rate-limit window (secondary). Values are request counts, not tokens.
//
// No local credential on disk — the user pastes the kimi-auth token in settings.

use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

const USAGE_URL: &str =
    "https://www.kimi.com/apiv2/kimi.gateway.billing.v1.BillingService/GetUsages";
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";

// --- Error -------------------------------------------------------------------

#[derive(Debug)]
pub enum FetchError {
    BadToken,
    Network(String),
    Unauthorized,
    BadResponse(String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchError::BadToken => {
                write!(f, "Kimi 令牌格式不对（应为 kimi-auth 的 JWT，以 eyJ 开头）")
            }
            FetchError::Network(e) => write!(f, "Network error: {e}"),
            FetchError::Unauthorized => write!(f, "Kimi 令牌已过期，请重新粘贴 kimi-auth"),
            FetchError::BadResponse(s) => write!(f, "Unexpected response: {s}"),
        }
    }
}

impl std::error::Error for FetchError {}

// --- Token handling ----------------------------------------------------------

fn is_jwt(s: &str) -> bool {
    s.starts_with("eyJ") && s.split('.').count() == 3
}

/// Accept the token in any of the forms a user is likely to paste:
/// a raw JWT, a `kimi-auth=<jwt>` cookie fragment, or a full Cookie header.
pub fn extract_token(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(idx) = raw.find("kimi-auth=") {
        let rest = &raw[idx + "kimi-auth=".len()..];
        let end = rest
            .find(|c: char| c == ';' || c.is_whitespace())
            .unwrap_or(rest.len());
        let tok = rest[..end].trim();
        if is_jwt(tok) {
            return Some(tok.to_string());
        }
    }
    if is_jwt(raw) {
        return Some(raw.to_string());
    }
    None
}

/// Best-effort: read the `kimi-auth` cookie straight from the local Chrome
/// profile, decrypted via the OS keychain by the `rookie` crate. Triggers a
/// one-time macOS keychain authorization prompt (same as CodexBar). Returns
/// None if Chrome isn't installed, the user isn't logged in to kimi.com, or
/// decryption fails (e.g. Chrome's newer app-bound cookie encryption) — the
/// caller then falls back to the manually-pasted token.
#[cfg(target_os = "macos")]
pub fn auto_read_token() -> Option<String> {
    let cookies = rookie::chrome(Some(vec!["kimi.com".to_string()])).ok()?;
    cookies
        .into_iter()
        .find(|c| c.name == "kimi-auth")
        .map(|c| c.value)
        .filter(|v| is_jwt(v))
}

#[cfg(not(target_os = "macos"))]
pub fn auto_read_token() -> Option<String> {
    None
}

#[derive(Default)]
struct SessionInfo {
    device_id: Option<String>,
    ssid: Option<String>,
    sub: Option<String>,
}

/// Decode the JWT payload (base64url, no padding) to pull session identifiers.
/// Missing/invalid payloads just yield empty fields — the headers are optional.
fn decode_session(jwt: &str) -> SessionInfo {
    let mut info = SessionInfo::default();
    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() != 3 {
        return info;
    }
    let payload = parts[1].trim_end_matches('=');
    let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload) else {
        return info;
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return info;
    };
    info.device_id = v["device_id"].as_str().map(String::from);
    info.ssid = v["ssid"].as_str().map(String::from);
    info.sub = v["sub"].as_str().map(String::from);
    info
}

// --- API response shapes -----------------------------------------------------

#[derive(Deserialize)]
struct UsageResponse {
    usages: Vec<Usage>,
}

#[derive(Deserialize)]
struct Usage {
    scope: String,
    detail: UsageDetail,
    limits: Option<Vec<RateLimit>>,
}

#[derive(Deserialize)]
struct RateLimit {
    detail: UsageDetail,
}

// Numeric fields arrive as strings on the wire.
#[derive(Deserialize)]
struct UsageDetail {
    limit: String,
    #[serde(default)]
    used: Option<String>,
    #[serde(default)]
    remaining: Option<String>,
    #[serde(rename = "resetTime", default)]
    reset_time: Option<String>,
}

// --- Public types ------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct KimiWindow {
    pub percent: f64,
    pub window_label: String, // "周" / "5h"
    pub used: Option<u64>,
    pub limit: Option<u64>,
    pub resets_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct KimiUsage {
    pub weekly: Option<KimiWindow>,     // primary: weekly request quota
    pub rate_limit: Option<KimiWindow>, // secondary: 5-hour rate window
    pub fetched_at: DateTime<Utc>,
}

fn parse_dt(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn to_window(d: &UsageDetail, label: &str) -> KimiWindow {
    let limit = d.limit.parse::<u64>().ok();
    let remaining = d.remaining.as_deref().and_then(|s| s.parse::<u64>().ok());
    let used = d
        .used
        .as_deref()
        .and_then(|s| s.parse::<u64>().ok())
        .or_else(|| match (limit, remaining) {
            (Some(l), Some(r)) => Some(l.saturating_sub(r)),
            _ => None,
        });
    let percent = match (used, limit) {
        (Some(u), Some(l)) if l > 0 => (u as f64 / l as f64) * 100.0,
        _ => 0.0,
    };
    let percent = (percent * 10.0).round() / 10.0;
    KimiWindow {
        percent,
        window_label: label.to_string(),
        used,
        limit,
        resets_at: d.reset_time.as_deref().and_then(parse_dt),
    }
}

// --- Main fetch --------------------------------------------------------------

pub async fn fetch(token_input: &str) -> Result<KimiUsage, FetchError> {
    let token = extract_token(token_input).ok_or(FetchError::BadToken)?;
    let session = decode_session(&token);

    let client = wreq::Client::builder()
        .emulation(wreq_util::Emulation::Chrome137)
        .build()
        .map_err(|e| FetchError::Network(e.to_string()))?;

    let body = serde_json::json!({ "scope": ["FEATURE_CODING"] });

    let mut req = client
        .post(USAGE_URL)
        .header("Authorization", format!("Bearer {token}"))
        .header("Cookie", format!("kimi-auth={token}"))
        .header("Origin", "https://www.kimi.com")
        .header("Referer", "https://www.kimi.com/code/console")
        .header("Accept", "*/*")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("User-Agent", USER_AGENT)
        .header("connect-protocol-version", "1")
        .header("x-language", "en-US")
        .header("x-msh-platform", "web")
        .header("r-timezone", "Asia/Shanghai");
    if let Some(d) = &session.device_id {
        req = req.header("x-msh-device-id", d);
    }
    if let Some(s) = &session.ssid {
        req = req.header("x-msh-session-id", s);
    }
    if let Some(s) = &session.sub {
        req = req.header("x-traffic-id", s);
    }

    let resp = req
        .json(&body)
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

    let raw: UsageResponse = resp
        .json()
        .await
        .map_err(|e| FetchError::BadResponse(format!("parse: {e}")))?;

    let coding = raw
        .usages
        .iter()
        .find(|u| u.scope == "FEATURE_CODING")
        .ok_or_else(|| FetchError::BadResponse("FEATURE_CODING scope not found".into()))?;

    let weekly = Some(to_window(&coding.detail, "周"));
    let rate_limit = coding
        .limits
        .as_ref()
        .and_then(|v| v.first())
        .map(|rl| to_window(&rl.detail, "5h"));

    Ok(KimiUsage {
        weekly,
        rate_limit,
        fetched_at: Utc::now(),
    })
}
