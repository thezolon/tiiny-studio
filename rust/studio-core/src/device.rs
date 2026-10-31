//! Requests to the active Tiiny (server.py device(), catalog(), device_stats()).

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::AppState;

pub const NOT_CONFIGURED: &str =
    "No device configured yet. Open Settings and enter your Tiiny's address and API key.";

/// JSON request to the device; answers (status, JSON) like server.py device(): 503 when nothing is
/// configured, 502 when unreachable, the device's own status and JSON (or its text as an error) otherwise.
/// `to` overrides the active device's (url, key) for testing an address before it's saved.
pub async fn call(
    state: &AppState,
    method: &str,
    path: &str,
    body: Option<Value>,
    timeout: u64,
    to: Option<(&str, &str)>,
) -> (u16, Value) {
    let cur = state.settings.current();
    let (url, key) = to.unwrap_or((&cur.url, &cur.key));
    if url.is_empty() {
        return (503, json!({"error": {"message": NOT_CONFIGURED}}));
    }
    let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET);
    let mut req = state
        .http
        .request(method, format!("{url}{path}"))
        .bearer_auth(key)
        .timeout(Duration::from_secs(timeout));
    if let Some(b) = body {
        req = req.json(&b);
    }
    match req.send().await {
        Ok(res) => {
            let code = res.status().as_u16();
            let raw = res.bytes().await.unwrap_or_default();
            if raw.is_empty() {
                return (code, json!({}));
            }
            match serde_json::from_slice::<Value>(&raw) {
                Ok(v) => (code, v),
                Err(_) => {
                    // an error answer's own text; a 200 that isn't JSON (a login page, a proxy) is an error too
                    let text: String = String::from_utf8_lossy(&raw).chars().take(500).collect();
                    (
                        if code >= 400 { code } else { 502 },
                        json!({"error": {"message": text}}),
                    )
                }
            }
        }
        Err(e) => (
            502,
            json!({"error": {"message": format!("Device unreachable: {e}")}}),
        ),
    }
}

/// urllib.parse.quote(s, safe=""): everything but letters, digits and _.-~ is %-encoded (per UTF-8 byte).
pub(crate) fn quote(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"_.-~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// Python truthiness of a JSON value (null, false, 0, "" and empty containers are false).
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn workspace(kind: &str) -> &'static str {
    match kind {
        "Text-to-Image" | "Image-to-Image" => "image",
        "Text-to-Speech" | "ASR" | "Music Generation" => "audio",
        "Image-to-3D" => "3d",
        "Text-to-Video" => "video",
        "Image-Text-to-Text" | "Text Generation" => "text",
        _ => "other",
    }
}

/// Every installed model with live status, tagged with the studio workspace that uses it.
pub async fn catalog(state: &AppState) -> (u16, Value) {
    let (code, inst) = call(
        state,
        "GET",
        "/api/v1/models/?including_downloading=false",
        None,
        15,
        None,
    )
    .await;
    if code != 200 {
        return (code, inst);
    }
    let (_, run) = call(state, "GET", "/api/v1/models/running", None, 15, None).await;
    let instances = run.get("instances").cloned().unwrap_or(json!({}));
    let list = |k: &str| {
        instances
            .get(k)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let running: Vec<Value> = list("running");
    let mut pending: Vec<String> = list("pending")
        .iter()
        .filter_map(|r| r.get("model_id").and_then(Value::as_str).map(String::from))
        .collect();
    pending.extend(
        run.get("pending")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(String::from)),
    );
    let running_of = |mid: &str| {
        running
            .iter()
            .find(|r| r.get("model_id").and_then(Value::as_str) == Some(mid))
    };
    const KEEP: [&str; 16] = [
        "id",
        "display_name",
        "params",
        "type",
        "input",
        "output",
        "supported",
        "desc",
        "npu_usage",
        "load_time",
        "costMemory",
        "size",
        "version",
        "thinking",
        "reasoning_levels",
        "capabilities",
    ];
    let mut models = vec![];
    for m in inst
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let mid = m.get("id").and_then(Value::as_str).unwrap_or("");
        let mut row = serde_json::Map::new();
        for k in KEEP {
            row.insert(k.into(), m.get(k).cloned().unwrap_or(Value::Null));
        }
        let r = running_of(mid);
        let status = if r.is_some() {
            "running"
        } else if pending.iter().any(|p| p == mid) {
            "loading"
        } else {
            "stopped"
        };
        row.insert("status".into(), json!(status));
        let active = r
            .and_then(|r| r.get("active_request_count"))
            .is_some_and(truthy);
        row.insert("busy".into(), json!(active || state.jobs_running_on(mid)));
        row.insert(
            "workspace".into(),
            json!(workspace(
                m.get("type").and_then(Value::as_str).unwrap_or("")
            )),
        );
        models.push(Value::Object(row));
    }
    let used: f64 = running
        .iter()
        .map(|r| r.get("npu_usage").and_then(Value::as_f64).unwrap_or(0.0))
        .sum();
    let used = if used.fract() == 0.0 {
        json!(used as i64)
    } else {
        json!(used)
    };
    let cur = state.settings.current();
    (
        200,
        json!({
            "models": models, "npu": {"used": used, "total": 100}, "direct_access": direct_access(state).await,
            "device": {"id": state.settings.active(), "name": cur.id.as_ref().map(|_| cur.name.clone()), "host": cur.host()},
            "last_switch": state.settings.last_switch(),
        }),
    )
}

/// Can this connection reach model containers directly? None when nothing is loaded. Cached 30 s.
async fn direct_access(state: &AppState) -> Option<bool> {
    let host = state.settings.current().host();
    {
        let c = state.direct_cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((t, h, v)) = &*c {
            if t.elapsed() < Duration::from_secs(30) && *h == host {
                return *v;
            }
        }
    }
    let (code, run) = call(state, "GET", "/api/v1/models/running", None, 10, None).await;
    let running = if code == 200 {
        run.pointer("/instances/running")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    } else {
        vec![]
    };
    let port = running
        .iter()
        .find_map(|r| r.get("port").and_then(Value::as_u64).filter(|p| *p != 0));
    let v = match port {
        Some(p) => Some(port_open(&host, p as u16, Duration::from_millis(1500)).await),
        None => None,
    };
    *state.direct_cache.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), host, v));
    v
}

pub async fn port_open(host: &str, port: u16, timeout: Duration) -> bool {
    matches!(
        tokio::time::timeout(timeout, tokio::net::TcpStream::connect((host, port))).await,
        Ok(Ok(_))
    )
}

/// (free GB, total GB) of the Tiiny's storage from /api/v1/sys/status, or None when it can't be read. Cached for
/// STUDIO_DISK_TTL seconds (60; the contract tests set 0) per device address. Results are written on the device first.
pub async fn disk_space(state: &AppState) -> Option<(f64, f64)> {
    let ttl = std::env::var("STUDIO_DISK_TTL")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(60.0);
    let url = state.settings.current().url;
    {
        let c = state.disk_cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((t, u, v)) = &*c {
            if *u == url && t.elapsed().as_secs_f64() < ttl {
                return *v;
            }
        }
    }
    let (code, st) = call(state, "GET", "/api/v1/sys/status", None, 5, None).await;
    let num = |k: &str| {
        st.get("disk")
            .and_then(|d| d.get(k))
            .and_then(Value::as_f64)
    };
    let val = match (code, num("total_bytes"), num("used_bytes")) {
        (200, Some(total), Some(used)) if total > 0.0 => {
            Some(((total - used).max(0.0) / 1e9, total / 1e9))
        }
        _ => None,
    };
    *state.disk_cache.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), url, val));
    val
}

/// NPU/CPU stats: the gateway's /api/v1/npu/status (every link) plus the optional on-device dashboard
/// (:9109/api/stats) for power and temperatures when it's running. Cached 3 s.
pub async fn stats(state: &AppState) -> Value {
    {
        let c = state.stats_cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((t, v)) = &*c {
            if t.elapsed() < Duration::from_secs(3) {
                return v.clone();
            }
        }
    }
    let mut out = serde_json::Map::new();
    let mut source = vec![];
    let (code, st) = call(state, "GET", "/api/v1/npu/status", None, 5, None).await;
    if code == 200 {
        source.push("gateway");
        let dev = st
            .get("devices")
            .and_then(|d| d.get(0))
            .cloned()
            .unwrap_or(json!({}));
        let g = |v: &Value, k: &str| v.get(k).cloned().unwrap_or(Value::Null);
        for (k, src) in [
            ("npu_util", "util_percent"),
            ("npu_mem_used_mb", "mem_used_mb"),
            ("npu_mem_total_mb", "mem_total_mb"),
            ("npu_temp_c", "temp_c"),
            ("npu_power_w", "power_w"),
        ] {
            out.insert(k.into(), g(&dev, src));
        }
        out.insert(
            "cpu_pct".into(),
            g(
                &st.get("cpu").cloned().unwrap_or(json!({})),
                "total_percent",
            ),
        );
        out.insert(
            "mem_pct".into(),
            g(
                &st.get("memory").cloned().unwrap_or(json!({})),
                "usage_percent",
            ),
        );
        out.insert(
            "npu_status".into(),
            g(&st.get("npu").cloned().unwrap_or(json!({})), "status"),
        );
    }
    let host = state.settings.current().host();
    if !host.is_empty() {
        let res = state
            .http
            .get(format!("http://{host}:9109/api/stats"))
            .timeout(Duration::from_millis(1200))
            .send()
            .await;
        if let Ok(d) = match res {
            Ok(r) => r.json::<Value>().await,
            Err(e) => Err(e),
        } {
            source.push("dashboard");
            let npu = d.get("npu").cloned().unwrap_or(json!({}));
            let prev = |k: &str| out.get(k).cloned().unwrap_or(Value::Null);
            let power = npu
                .get("board_power_w")
                .cloned()
                .unwrap_or_else(|| prev("npu_power_w"));
            let util = npu
                .get("avg_util")
                .cloned()
                .unwrap_or_else(|| prev("npu_util"));
            let temps: Vec<f64> = npu
                .get("temps")
                .and_then(Value::as_object)
                .map(|t| t.values().filter_map(Value::as_f64).collect())
                .unwrap_or_default();
            let temp = if temps.is_empty() {
                prev("npu_temp_c")
            } else {
                json!(temps.iter().cloned().fold(f64::MIN, f64::max))
            };
            out.insert("npu_power_w".into(), power);
            out.insert("npu_util".into(), util);
            out.insert("npu_temp_c".into(), temp);
        }
    }
    let space = disk_space(state).await;
    out.insert(
        "disk_free_gb".into(),
        space.map_or(Value::Null, |(f, _)| json!((f * 10.0).round() / 10.0)),
    );
    out.insert(
        "disk_total_gb".into(),
        space.map_or(Value::Null, |(_, t)| json!((t * 10.0).round() / 10.0)),
    );
    let mut v = json!({"source": source});
    v.as_object_mut().unwrap().extend(out);
    *state.stats_cache.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((Instant::now(), v.clone()));
    v
}

#[cfg(test)]
mod tests {
    use super::quote;

    #[test]
    fn quoting() {
        assert_eq!(quote("Qwen/Qwen3-8B v2"), "Qwen%2FQwen3-8B%20v2");
        assert_eq!(quote("a_b.c-d~e"), "a_b.c-d~e");
        assert_eq!(quote("a b/c"), "a%20b%2Fc");
        assert_eq!(quote("é日"), "%C3%A9%E6%97%A5");
        assert_eq!(quote(""), "");
    }
}
