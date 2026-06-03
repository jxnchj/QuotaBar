// Claude.ai web usage fetcher.
// Endpoint: GET https://claude.ai/api/organizations/{org_id}/usage
// Auth: sessionKey cookie (sk-ant-sid01-...).
// Cloudflare passes when we use Safari/Chrome TLS fingerprint via wreq.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

const BASE: &str = "https://claude.ai/api";
const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15";

#[derive(Debug)]
pub enum FetchError {
    Network(String),
    Unauthorized,
    Cloudflare,
    NoOrganization,
    BadResponse(String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchError::Network(e) => write!(f, "Network error: {e}"),
            FetchError::Unauthorized => write!(f, "Unauthorized — sessionKey expired or invalid"),
            FetchError::Cloudflare => write!(f, "Blocked by Cloudflare challenge"),
            FetchError::NoOrganization => write!(f, "No organization found for this account"),
            FetchError::BadResponse(s) => write!(f, "Unexpected response: {s}"),
        }
    }
}

impl std::error::Error for FetchError {}

// ---------- Raw JSON shapes ----------

#[derive(Debug, Deserialize)]
struct OrganizationRaw {
    uuid: String,
    #[serde(default)]
    rate_limit_tier: Option<String>,
    #[serde(default)]
    capabilities: Option<Vec<String>>,
}

impl OrganizationRaw {
    /// A subscription org (Max/Pro/Team/…) vs. an API/eval org. The account can
    /// list several orgs; we want the one whose usage windows we display.
    fn is_subscription(&self) -> bool {
        let cap = self
            .capabilities
            .as_ref()
            .map(|c| c.iter().any(|x| x.starts_with("claude_")))
            .unwrap_or(false);
        let tier = self
            .rate_limit_tier
            .as_deref()
            .map(|t| t.starts_with("default_claude_"))
            .unwrap_or(false);
        cap || tier
    }

    /// Friendly subscription label, e.g. "Max 5x", "Max 20x", "Pro", "Team".
    fn plan_label(&self) -> Option<String> {
        let tier = self.rate_limit_tier.as_deref().unwrap_or("");
        let label = if tier.contains("max_20x") {
            "Max 20x"
        } else if tier.contains("max_5x") {
            "Max 5x"
        } else if tier.contains("max") {
            "Max"
        } else if tier.contains("team") {
            "Team"
        } else if tier.contains("enterprise") {
            "Enterprise"
        } else if tier.contains("pro") {
            "Pro"
        } else if tier.contains("free") {
            "Free"
        } else {
            ""
        };
        if !label.is_empty() {
            return Some(label.to_string());
        }
        // Fall back to capabilities if the tier string is unfamiliar.
        if let Some(caps) = &self.capabilities {
            if caps.iter().any(|c| c == "claude_max") {
                return Some("Max".into());
            }
            if caps.iter().any(|c| c == "claude_pro") {
                return Some("Pro".into());
            }
        }
        None
    }
}

#[derive(Debug, Deserialize)]
struct RateWindowRaw {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UsageResponseRaw {
    five_hour: Option<RateWindowRaw>,
    seven_day: Option<RateWindowRaw>,
    seven_day_opus: Option<RateWindowRaw>,
    seven_day_sonnet: Option<RateWindowRaw>,
}

// ---------- Public shape returned to frontend ----------

#[derive(Debug, Clone, Serialize)]
pub struct RateWindow {
    /// Percentage 0–100 (rounded to 1 decimal).
    pub percent: f64,
    /// ISO 8601 string for when window resets, or null if unknown.
    pub resets_at: Option<DateTime<Utc>>,
}

impl From<RateWindowRaw> for RateWindow {
    fn from(raw: RateWindowRaw) -> Self {
        // Anthropic returns utilization already as a percentage (0..100, e.g. 31.0).
        // Round to 1 decimal for stable display.
        let pct = (raw.utilization.unwrap_or(0.0) * 10.0).round() / 10.0;
        let resets_at = raw
            .resets_at
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));
        RateWindow {
            percent: pct,
            resets_at,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ClaudeUsage {
    pub five_hour: Option<RateWindow>,
    pub seven_day: Option<RateWindow>,
    pub seven_day_opus: Option<RateWindow>,
    pub seven_day_sonnet: Option<RateWindow>,
    pub plan: Option<String>,
    pub fetched_at: DateTime<Utc>,
}

// ---------- Fetcher ----------

pub async fn fetch(session_key: &str) -> Result<ClaudeUsage, FetchError> {
    let client = wreq::Client::builder()
        .emulation(wreq_util::Emulation::Safari18_5)
        .build()
        .map_err(|e| FetchError::Network(e.to_string()))?;

    let cookie = format!("sessionKey={session_key}");

    // Step 1: discover org_id.
    let url = format!("{BASE}/organizations");
    let resp = client
        .get(&url)
        .header("Cookie", &cookie)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/json")
        .header("Referer", "https://claude.ai/settings/usage")
        .header("anthropic-client-platform", "web_claude_ai")
        .send()
        .await
        .map_err(|e| FetchError::Network(e.to_string()))?;

    let status = resp.status();
    if status == 401 || status == 403 {
        // Distinguish Cloudflare challenge from real auth failure by content-type.
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if ct.starts_with("text/html") {
            return Err(FetchError::Cloudflare);
        }
        return Err(FetchError::Unauthorized);
    }
    if !status.is_success() {
        return Err(FetchError::BadResponse(format!(
            "/organizations returned {status}"
        )));
    }

    let orgs: Vec<OrganizationRaw> = resp
        .json()
        .await
        .map_err(|e| FetchError::BadResponse(format!("parse organizations: {e}")))?;
    // Prefer a real subscription org over an API/eval org; fall back to the first.
    let org = orgs
        .iter()
        .find(|o| o.is_subscription())
        .or_else(|| orgs.first())
        .ok_or(FetchError::NoOrganization)?;
    let org_id = org.uuid.clone();
    let plan = org.plan_label();

    // Step 2: pull usage for that org.
    let url = format!("{BASE}/organizations/{org_id}/usage");
    let resp = client
        .get(&url)
        .header("Cookie", &cookie)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/json")
        .header("Referer", "https://claude.ai/settings/usage")
        .header("anthropic-client-platform", "web_claude_ai")
        .send()
        .await
        .map_err(|e| FetchError::Network(e.to_string()))?;

    let status = resp.status();
    if status == 401 || status == 403 {
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if ct.starts_with("text/html") {
            return Err(FetchError::Cloudflare);
        }
        return Err(FetchError::Unauthorized);
    }
    if !status.is_success() {
        return Err(FetchError::BadResponse(format!(
            "/usage returned {status}"
        )));
    }

    let raw: UsageResponseRaw = resp
        .json()
        .await
        .map_err(|e| FetchError::BadResponse(format!("parse usage: {e}")))?;

    Ok(ClaudeUsage {
        five_hour: raw.five_hour.map(Into::into),
        seven_day: raw.seven_day.map(Into::into),
        seven_day_opus: raw.seven_day_opus.map(Into::into),
        seven_day_sonnet: raw.seven_day_sonnet.map(Into::into),
        plan,
        fetched_at: Utc::now(),
    })
}
