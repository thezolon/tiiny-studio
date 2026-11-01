//! "Is a newer release out?": GET /api/update. Port of server.py check_update. One anonymous GET to GitHub (no
//! identifiers, nothing about the device), cached for a day; a failure is remembered for a day too and says nothing.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::AppState;

const UPDATE_URL: &str = "https://api.github.com/repos/thezolon/tiiny-studio/releases/latest"; // STUDIO_UPDATE_URL points tests elsewhere
const TTL: Duration = Duration::from_secs(24 * 3600);

/// "v0.10.0" -> [0, 10, 0]; None for anything else (a "-rc1" suffix, "dev"): only plain releases are offered.
fn version_parts(tag: &str) -> Option<Vec<u64>> {
    let tag = tag.trim();
    let tag = tag.strip_prefix('v').unwrap_or(tag);
    if tag.is_empty() {
        return None;
    }
    tag.split('.').map(|n| n.parse::<u64>().ok()).collect()
}

fn newer_version(latest: &str, current: &str) -> bool {
    let (Some(mut a), Some(mut b)) = (version_parts(latest), version_parts(current)) else {
        return false;
    };
    let n = a.len().max(b.len());
    a.resize(n, 0);
    b.resize(n, 0);
    a > b
}

fn none() -> Value {
    json!({"available": false})
}

/// Ask GitHub once. Never fails: any problem (offline, 403/429, bad JSON) answers "not available".
async fn fetch(http: &reqwest::Client) -> Value {
    let url = std::env::var("STUDIO_UPDATE_URL")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| UPDATE_URL.into());
    let sent = http
        .get(url)
        .header("User-Agent", format!("tiiny-studio/{}", crate::version()))
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(10))
        .send()
        .await;
    let rel: Value = match sent {
        Ok(r) if r.status().is_success() => match r.bytes().await {
            Ok(b) if b.len() <= 1_000_000 => serde_json::from_slice(&b).unwrap_or(Value::Null),
            _ => return none(),
        },
        _ => return none(),
    };
    let flag = |k: &str| rel.get(k).is_some_and(crate::device::truthy);
    let Some(tag) = rel.get("tag_name").and_then(Value::as_str) else {
        return none();
    };
    if flag("draft") || flag("prerelease") || !newer_version(tag, crate::version()) {
        return none();
    }
    let text = |k: &str| rel.get(k).and_then(Value::as_str).unwrap_or("");
    json!({
        "available": true,
        "version": tag.trim().trim_start_matches('v'),
        "notes": text("body").chars().take(20000).collect::<String>(),
        "url": text("html_url"),
    })
}

/// The newest release if it is newer than this one, cached for a day. Off in Settings: GitHub isn't contacted.
pub(crate) async fn check(state: &AppState) -> Value {
    if !state.settings.update_check() {
        return none();
    }
    let mut cache = state.update_cache.lock().await; // held while asking, so two callers make one request
    if let Some((when, answer)) = cache.as_ref() {
        if when.elapsed() < TTL {
            return answer.clone();
        }
    }
    let answer = fetch(&state.http).await;
    *cache = Some((Instant::now(), answer.clone()));
    answer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_numerically() {
        assert!(newer_version("v0.10.0", "0.9.1"));
        assert!(newer_version("1.0", "0.9.9"));
        assert!(!newer_version("0.4.0", "0.4.0"));
        assert!(!newer_version("0.4.0", "0.4.1"));
        assert!(!newer_version("0.5.0-rc1", "0.4.0"));
        assert!(!newer_version("0.5.0", "dev"));
    }
}
