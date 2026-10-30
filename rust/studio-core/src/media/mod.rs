//! Job kinds (media.py KINDS): image, ocr, speech, transcribe, loop, song, mesh, video, assist, and chat.py's chat.
//! Each arrives with its stage (3: image/assist/ocr, 4: chat, 5: audio and 3D); until then the kind is unknown.
//! The plumbing below is media.py's Media helpers.

mod assist;
mod audio;
mod image;
mod mesh;
mod music;
mod ocr;
mod speech;
mod transcribe;
mod video;

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::jobs::{Ctx, JobError, KindFn};

pub use assist::{clean_lyrics, clean_reply};

pub fn kind(name: &str) -> Option<KindFn> {
    match name {
        "image" => Some(|ctx| Box::pin(image::run(ctx))),
        "ocr" => Some(|ctx| Box::pin(ocr::run(ctx))),
        "assist" => Some(|ctx| Box::pin(assist::run(ctx))),
        "chat" => Some(|ctx| Box::pin(crate::chat::run(ctx))),
        "speech" => Some(|ctx| Box::pin(speech::run(ctx))),
        "transcribe" => Some(|ctx| Box::pin(transcribe::run(ctx))),
        "loop" => Some(|ctx| Box::pin(music::run_loop(ctx))),
        "song" => Some(|ctx| Box::pin(music::run_song(ctx))),
        "mesh" => Some(|ctx| Box::pin(mesh::run(ctx))),
        "video" => Some(|ctx| Box::pin(video::run(ctx))),
        #[cfg(test)]
        "test" => Some(|ctx| Box::pin(test_kind(ctx))),
        _ => None,
    }
}

fn short(model: &str) -> &str {
    model.rsplit('/').next().unwrap_or(model)
}

// ---------- model juggling: chat tools and helpers load what they need, then put the user's models back ----------
const LOAD_TIMEOUT: Duration = Duration::from_secs(420); // the biggest models take ~2 minutes to load

fn quote(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"_.-~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

pub async fn running_ids(ctx: &Ctx) -> Vec<String> {
    let (code, run) = crate::device::call(&ctx.state, "GET", "/api/v1/models/running", None, 15, None).await;
    if code != 200 {
        return vec![];
    }
    run.pointer("/instances/running").and_then(Value::as_array).into_iter().flatten().map(|r| r.get("model_id").and_then(Value::as_str).unwrap_or("").to_string()).collect()
}

async fn wait_loaded(ctx: &Ctx, mid: &str) -> Result<(), JobError> {
    let t0 = Instant::now();
    while t0.elapsed() < LOAD_TIMEOUT {
        if ctx.cancelled() {
            return Err(JobError("Cancelled".into()));
        }
        if running_ids(ctx).await.iter().any(|m| m == mid) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    Err(JobError(format!("{} didn't finish loading", short(mid))))
}

/// Load target, unloading whatever the device says must go. Returns the models it unloaded.
pub async fn make_room(ctx: &Ctx, target: &str) -> Result<Vec<String>, JobError> {
    if running_ids(ctx).await.iter().any(|m| m == target) {
        return Ok(vec![]);
    }
    let st = &ctx.state;
    let mut unloaded = vec![];
    for _ in 0..4 {
        // one unload may not be enough
        let (code, plan) = crate::device::call(st, "GET", &format!("/api/v1/models/{}/unload_candidate", quote(target)), None, 30, None).await;
        if code != 200 {
            return Err(JobError(err(&plan)));
        }
        if !plan.get("unload_required").is_some_and(crate::device::truthy) {
            break;
        }
        for victim in plan.get("model_id").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
            ctx.update(json!({"message": format!("Unloading {} to make room", short(victim))}));
            let (code, res) = crate::device::call(st, "POST", &format!("/api/v1/models/{}/stop", quote(victim)), None, 120, None).await;
            if code != 200 {
                return Err(JobError(err(&res)));
            }
            unloaded.push(victim.to_string());
        }
    }
    ctx.update(json!({"message": format!("Loading {}", short(target))}));
    let (code, res) = crate::device::call(st, "POST", &format!("/api/v1/models/{}/launch", quote(target)), Some(json!({"model_id": target})), 120, None).await;
    if code != 200 {
        return Err(JobError(err(&res)));
    }
    wait_loaded(ctx, target).await?;
    Ok(unloaded)
}

/// After a tool or helper: if other models had to make room, unload target and load them back.
pub async fn put_back(ctx: &Ctx, target: &str, unloaded: &[String]) -> Result<(), JobError> {
    if unloaded.is_empty() {
        return Ok(()); // everything fitted: leave the model loaded for next time
    }
    let st = &ctx.state;
    ctx.update(json!({"message": format!("Unloading {}", short(target))}));
    crate::device::call(st, "POST", &format!("/api/v1/models/{}/stop", quote(target)), None, 120, None).await;
    for mid in unloaded {
        ctx.update(json!({"message": format!("Loading {} back", short(mid))}));
        let (code, res) = crate::device::call(st, "POST", &format!("/api/v1/models/{}/launch", quote(mid)), Some(json!({"model_id": mid})), 120, None).await;
        if code != 200 {
            return Err(JobError(format!("Couldn't load {} back: {}", short(mid), err(&res))));
        }
    }
    for mid in unloaded {
        wait_loaded(ctx, mid).await?;
    }
    Ok(())
}

// ---------- thinking: what a chat model does, from its own /v1/models "thinking" object (TiinyOS 0.1.36+) ----------
/// The model's thinking object from /v1/models, or None (older TiinyOS, or the model isn't loaded).
pub async fn thinking_of(ctx: &Ctx, model: &str) -> Option<Value> {
    let (code, res) = crate::device::call(&ctx.state, "GET", "/v1/models", None, 15, None).await;
    if code != 200 {
        return None;
    }
    let item = res.get("data").and_then(Value::as_array)?.iter().find(|x| x.get("id").and_then(Value::as_str) == Some(model))?;
    item.get("thinking").filter(|t| t.is_object()).cloned()
}

/// "none" (never reasons), "switch" (enable_thinking turns it on/off) or "always" (reasons whatever it's sent, like
/// gpt-oss; tested on 0.1.38: neither enable_thinking nor reasoning_effort changes it reliably).
pub fn thinking_mode(model: &str, thinking: Option<&Value>) -> &'static str {
    match thinking {
        // no thinking object: older TiinyOS, where gpt-oss also rejected enable_thinking
        None => if model.contains("gpt-oss") { "always" } else { "switch" },
        Some(t) if !t.get("supported").is_some_and(crate::device::truthy) => "none",
        Some(t) => if t.get("toggleable").is_some_and(crate::device::truthy) { "switch" } else { "always" },
    }
}

/// The running instance of a model (its port etc.), or the "isn't loaded" error the UI shows.
pub async fn running_instance(ctx: &Ctx, model: &str) -> Result<Value, JobError> {
    let (_, run) = crate::device::call(&ctx.state, "GET", "/api/v1/models/running", None, 15, None).await;
    run.pointer("/instances/running")
        .and_then(Value::as_array)
        .and_then(|r| r.iter().find(|r| r.get("model_id").and_then(Value::as_str) == Some(model)).cloned())
        .ok_or_else(|| JobError(format!("{} isn't loaded. Load it from the Models panel.", short(model))))
}

/// For work routed through the gateway (port 80): just check the model is loaded.
pub async fn ensure_running(ctx: &Ctx, model: &str) -> Result<(), JobError> {
    running_instance(ctx, model).await.map(|_| ())
}

/// Direct URL of a model's own container (media.py instance_url). Only the music models need this: the gateway
/// has no route for them. Some links (the USB cable) only expose the gateway, so check the port first.
pub async fn instance_url(ctx: &Ctx, model: &str) -> Result<String, JobError> {
    let r = running_instance(ctx, model).await?;
    let host = ctx.state.settings.current().host();
    let port = r.get("port").and_then(Value::as_u64);
    if !port_open(&host, port).await {
        return Err(JobError(format!(
            "{} is loaded, but its port isn't reachable at {host}. The USB link only exposes the device's gateway, which has no route for music models \u{2014} switch Settings to the device's Wi-Fi/LAN address to use them.",
            short(model)
        )));
    }
    Ok(format!("http://{host}:{}", port.unwrap_or(0)))
}

pub async fn port_open(host: &str, port: Option<u64>) -> bool {
    match port.and_then(|p| u16::try_from(p).ok()) {
        Some(p) => crate::device::port_open(host, p, Duration::from_secs(2)).await,
        None => false,
    }
}

/// A request outside the device client (a model's own port, or the gateway with a query string), like
/// media.py Media.http: JSON answers (errors too) come back as (status, JSON); network failures are JobErrors.
pub async fn http(ctx: &Ctx, method: &str, url: &str, body: Option<Value>, timeout: u64, gateway_auth: bool) -> Result<(u16, Value), JobError> {
    let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET);
    let mut req = ctx.state.http.request(method, url).timeout(Duration::from_secs(timeout));
    if gateway_auth {
        req = req.bearer_auth(ctx.state.settings.current().key);
    }
    if let Some(b) = body {
        req = req.json(&b);
    }
    let res = req.send().await.map_err(|e| JobError(format!("Device unreachable: {e}")))?;
    let code = res.status().as_u16();
    let raw = res.bytes().await.map_err(|e| JobError(format!("Device unreachable: {e}")))?;
    if raw.is_empty() {
        return Ok((code, json!({})));
    }
    match serde_json::from_slice(&raw) {
        Ok(v) => Ok((code, v)),
        Err(_) if code >= 400 => Ok((code, json!({"error": String::from_utf8_lossy(&raw).chars().take(500).collect::<String>()}))),
        Err(e) => Err(JobError(format!("Device unreachable: {e}"))),
    }
}

/// Like `http` with a raw request body (bytes + content type) and raw bytes back for answers below 400 (media.py
/// Media.http with raw=True); error answers come back as JSON like `http`.
pub async fn http_bytes(ctx: &Ctx, method: &str, url: &str, body: Option<(Vec<u8>, String)>, timeout: u64, gateway_auth: bool) -> Result<(u16, Result<Vec<u8>, Value>), JobError> {
    let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET);
    let mut req = ctx.state.http.request(method, url).timeout(Duration::from_secs(timeout));
    if gateway_auth {
        req = req.bearer_auth(ctx.state.settings.current().key);
    }
    if let Some((b, ctype)) = body {
        req = req.header("Content-Type", ctype).body(b);
    }
    let res = req.send().await.map_err(|e| JobError(format!("Device unreachable: {e}")))?;
    let code = res.status().as_u16();
    let raw = res.bytes().await.map_err(|e| JobError(format!("Device unreachable: {e}")))?;
    if code < 400 {
        return Ok((code, Ok(raw.to_vec())));
    }
    let v = serde_json::from_slice(&raw).unwrap_or_else(|_| json!({"error": String::from_utf8_lossy(&raw).chars().take(500).collect::<String>()}));
    Ok((code, Err(v)))
}

/// The JSON body of an `http_bytes` answer (what `http` returns for a JSON request).
pub fn json_reply(code: u16, reply: Result<Vec<u8>, Value>) -> Result<(u16, Value), JobError> {
    match reply {
        Err(v) => Ok((code, v)),
        Ok(b) if b.is_empty() => Ok((code, json!({}))),
        Ok(b) => serde_json::from_slice(&b).map(|v| (code, v)).map_err(|e| JobError(format!("JSONDecodeError: {e}"))),
    }
}

/// A JSON request body for `http_bytes`.
pub fn json_body(v: &Value) -> Option<(Vec<u8>, String)> {
    Some((v.to_string().into_bytes(), "application/json".into()))
}

/// multipart/form-data body and content type (media.py Media.multipart); files are (field, (name, data, type)).
pub fn multipart(fields: &[(&str, String)], files: &[(&str, (&str, &[u8], &str))]) -> (Vec<u8>, String) {
    let b = uuid::Uuid::new_v4().simple().to_string();
    let mut body = vec![];
    for (k, v) in fields {
        body.extend_from_slice(format!("--{b}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n").as_bytes());
    }
    for (k, (name, data, ctype)) in files {
        body.extend_from_slice(format!("--{b}\r\nContent-Disposition: form-data; name=\"{k}\"; filename=\"{name}\"\r\nContent-Type: {ctype}\r\n\r\n").as_bytes());
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{b}--\r\n").as_bytes());
    (body, format!("multipart/form-data; boundary={b}"))
}

/// Python round() on a float: halves go to the even number.
pub fn py_round(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// The readable part of an error answer (media.py Media.err).
pub fn err(res: &Value) -> String {
    let text = match res {
        Value::Object(o) => {
            let e = [o.get("error"), o.get("detail")].into_iter().flatten().find(|v| crate::device::truthy(v)).cloned().unwrap_or(res.clone());
            match e {
                Value::Object(ref m) => m.get("message").filter(|v| crate::device::truthy(v)).map(py_str).unwrap_or_else(|| py_json(&e)),
                Value::Array(items) => items
                    .iter()
                    .map(|x| {
                        let loc: Vec<String> = x.get("loc").and_then(Value::as_array).into_iter().flatten().map(py_str).collect();
                        format!("{}: {}", loc.join("."), x.get("msg").map(py_str).unwrap_or_else(|| "None".into()))
                    })
                    .collect::<Vec<_>>()
                    .join("; "),
                other => py_str(&other),
            }
        }
        other => py_str(other),
    };
    text.chars().take(400).collect()
}

/// str() of a JSON value the way Python prints it.
fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        Value::Bool(b) => if *b { "True" } else { "False" }.into(),
        other => other.to_string(),
    }
}

/// json.dumps() spacing (", " and ": ").
fn py_json(v: &Value) -> String {
    match v {
        Value::Object(o) => format!("{{{}}}", o.iter().map(|(k, v)| format!("{}: {}", Value::String(k.clone()), py_json(v))).collect::<Vec<_>>().join(", ")),
        Value::Array(a) => format!("[{}]", a.iter().map(py_json).collect::<Vec<_>>().join(", ")),
        other => other.to_string(),
    }
}

pub fn decode_uri(uri: &str) -> Result<(String, Vec<u8>), JobError> {
    crate::library::decode_uri(uri).map_err(|_| JobError("Expected a base64 data URI".into()))
}

pub fn save(ctx: &Ctx, name: &str, data: &[u8]) -> String {
    let _ = std::fs::write(ctx.state.store.images.join(name), data);
    name.to_string()
}

/// For the job-table tests: waits `ms`, fails when asked, and reports progress like a real kind.
#[cfg(test)]
async fn test_kind(ctx: Ctx) -> crate::jobs::JobResult {
    let ms = ctx.req.get("ms").and_then(Value::as_u64).unwrap_or(10);
    for i in 0..5 {
        if ctx.cancelled() {
            return Err(JobError("stopped".into()));
        }
        ctx.update(json!({"progress": i * 20}));
        tokio::time::sleep(Duration::from_millis(ms / 5)).await;
    }
    if ctx.req.get("fail").is_some() {
        return Err(JobError("it failed".into()));
    }
    Ok(json!({"kind": ctx.req.get("as").cloned().unwrap_or(json!("image")), "outputs": []}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_text() {
        assert_eq!(err(&json!({"error": {"message": "boom"}})), "boom");
        assert_eq!(err(&json!({"detail": [{"loc": ["body", "n"], "msg": "bad"}]})), "body.n: bad");
        assert_eq!(err(&json!({"error": {"code": 1}})), r#"{"code": 1}"#);
        assert_eq!(err(&json!({"x": 1})), r#"{"x": 1}"#);
        assert_eq!(err(&json!("plain")), "plain");
    }
}
