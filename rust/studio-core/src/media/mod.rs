//! Job kinds (media.py KINDS): image, ocr, speech, transcribe, loop, song, mesh, assist, and chat.py's chat.
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

use std::time::Duration;

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
        #[cfg(test)]
        "test" => Some(|ctx| Box::pin(test_kind(ctx))),
        _ => None,
    }
}

fn short(model: &str) -> &str {
    model.rsplit('/').next().unwrap_or(model)
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
