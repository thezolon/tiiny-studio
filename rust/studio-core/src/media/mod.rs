//! Job kinds (media.py KINDS): image, ocr, speech, transcribe, loop, song, mesh, video, assist, and chat.py's chat.
//! Each arrives with its stage (3: image/assist/ocr, 4: chat, 5: audio and 3D); until then the kind is unknown.
//! The plumbing below is media.py's Media helpers.

mod assist;
mod audio;
mod image;
mod mesh;
mod music;
mod ocr;
pub mod recover;
mod speech;
pub mod tags;
mod transcribe;
mod video;

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::device::quote;
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
#[cfg(not(test))]
const LOAD_TIMEOUT: Duration = Duration::from_secs(420); // the biggest models take ~2 minutes to load
#[cfg(test)]
const LOAD_TIMEOUT: Duration = Duration::from_millis(300);
#[cfg(not(test))]
const LOAD_POLL: Duration = Duration::from_secs(3);
#[cfg(test)]
const LOAD_POLL: Duration = Duration::from_millis(20);
const LAUNCH_RETRIES: usize = 2;
#[cfg(not(test))]
const LAUNCH_RETRY_WAIT: Duration = Duration::from_secs(4);
#[cfg(test)]
const LAUNCH_RETRY_WAIT: Duration = Duration::from_millis(20);

pub async fn running_ids(ctx: &Ctx) -> Vec<String> {
    let (code, run) = ctx.call("GET", "/api/v1/models/running", None, 15).await;
    if code != 200 {
        return vec![];
    }
    running_of(&run)
}

/// The model ids a /running answer lists as running.
fn running_of(run: &Value) -> Vec<String> {
    run.pointer("/instances/running")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|r| {
            r.get("model_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        })
        .collect()
}

/// The ids listed in a /running answer under `instances.pending` (or `pending`), None when the device lists none.
fn pending_ids(run: &Value) -> Option<Vec<String>> {
    let list = run
        .pointer("/instances/pending")
        .or_else(|| run.get("pending"))
        .and_then(Value::as_array)?;
    Some(
        list.iter()
            .filter_map(|p| {
                p.as_str()
                    .or_else(|| p.get("model_id").and_then(Value::as_str))
            })
            .map(String::from)
            .collect(),
    )
}

async fn wait_loaded(ctx: &Ctx, mid: &str) -> Result<(), JobError> {
    let t0 = Instant::now();
    let (mut seen_loading, mut device_lists_loading) = (false, false);
    while t0.elapsed() < LOAD_TIMEOUT {
        if ctx.cancelled() {
            return Err(JobError("Cancelled".into()));
        }
        let (code, run) = ctx.call("GET", "/api/v1/models/running", None, 15).await;
        if code == 200 {
            if running_of(&run).iter().any(|m| m == mid) {
                return Ok(());
            }
            if let Some(p) = pending_ids(&run) {
                device_lists_loading = true;
                seen_loading |= p.iter().any(|m| m == mid);
            }
        }
        tokio::time::sleep(LOAD_POLL).await;
    }
    Err(JobError(if device_lists_loading && !seen_loading {
        format!(
            "{} never started loading: the Tiiny accepted the request but didn't list the model as loading. Try loading it from the Models panel.",
            short(mid)
        )
    } else {
        format!("{} didn't finish loading", short(mid))
    }))
}

/// POST launch for `model` on `to`. A 409 `cleanup_failed` (an earlier stop didn't finish cleaning up; the device says
/// it must be retried, and the second launch worked in the one case on record) is tried again, twice, after a short wait.
pub(crate) async fn launch_on(
    state: &crate::AppState,
    to: (&str, &str),
    model: &str,
    cancelled: impl Fn() -> bool,
) -> (u16, Value) {
    let path = format!("/api/v1/models/{}/launch", quote(model));
    let mut retries = 0;
    loop {
        let (code, res) = crate::device::call(
            state,
            "POST",
            &path,
            Some(json!({"model_id": model})),
            120,
            Some(to),
        )
        .await;
        let text = res.to_string().to_lowercase();
        let retryable =
            code == 409 && (text.contains("cleanup_failed") || text.contains("must be retried"));
        if !retryable || retries >= LAUNCH_RETRIES || cancelled() {
            return (code, res);
        }
        retries += 1;
        tokio::time::sleep(LAUNCH_RETRY_WAIT).await;
    }
}

async fn launch(ctx: &Ctx, model: &str) -> (u16, Value) {
    let to = (ctx.target.url.as_str(), ctx.target.key.as_str());
    launch_on(&ctx.state, to, model, || ctx.cancelled()).await
}

/// Load target, unloading whatever the device says must go. Returns the models it unloaded. When the load then fails
/// or is stopped, the models it unloaded are loaded back (best effort) before the error comes back.
pub async fn make_room(ctx: &Ctx, target: &str) -> Result<Vec<String>, JobError> {
    let mut unloaded = vec![];
    match make_room_into(ctx, target, &mut unloaded).await {
        Ok(()) => Ok(unloaded),
        Err(e) if unloaded.is_empty() => Err(e),
        Err(e) => {
            let back = put_back(ctx, target, &unloaded).await;
            Err(match back {
                // a stopped job's own "Cancelled" cut the wait short; the launches were sent
                _ if ctx.cancelled() => e,
                Ok(()) => e,
                Err(b) => JobError(format!("{e} ({b})")),
            })
        }
    }
}

async fn make_room_into(
    ctx: &Ctx,
    target: &str,
    unloaded: &mut Vec<String>,
) -> Result<(), JobError> {
    if running_ids(ctx).await.iter().any(|m| m == target) {
        return Ok(());
    }
    for _ in 0..4 {
        // one unload may not be enough
        let (code, plan) = ctx
            .call(
                "GET",
                &format!("/api/v1/models/{}/unload_candidate", quote(target)),
                None,
                30,
            )
            .await;
        if code != 200 {
            return Err(JobError(err(&plan)));
        }
        if !plan
            .get("unload_required")
            .is_some_and(crate::device::truthy)
        {
            break;
        }
        for victim in plan
            .get("model_id")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            ctx.update(json!({"message": format!("Unloading {} to make room", short(victim))}));
            let (code, res) = ctx
                .call(
                    "POST",
                    &format!("/api/v1/models/{}/stop", quote(victim)),
                    None,
                    120,
                )
                .await;
            if code != 200 {
                return Err(JobError(err(&res)));
            }
            unloaded.push(victim.to_string());
        }
    }
    ctx.update(json!({"message": format!("Loading {}", short(target))}));
    let (code, res) = launch(ctx, target).await;
    if code != 200 {
        return Err(JobError(err(&res)));
    }
    wait_loaded(ctx, target).await
}

/// After a tool or helper: if other models had to make room, unload target and load them back.
pub async fn put_back(ctx: &Ctx, target: &str, unloaded: &[String]) -> Result<(), JobError> {
    if unloaded.is_empty() {
        return Ok(()); // everything fitted: leave the model loaded for next time
    }
    ctx.update(json!({"message": format!("Unloading {}", short(target))}));
    ctx.call(
        "POST",
        &format!("/api/v1/models/{}/stop", quote(target)),
        None,
        120,
    )
    .await;
    let mut failed = vec![];
    for mid in unloaded {
        ctx.update(json!({"message": format!("Loading {} back", short(mid))}));
        let (code, res) = launch(ctx, mid).await;
        if code != 200 {
            failed.push(format!("Couldn't load {} back: {}", short(mid), err(&res)));
        }
    }
    if !failed.is_empty() {
        // the others were still tried (one refusal doesn't strand the rest)
        return Err(JobError(failed.join("; ")));
    }
    for mid in unloaded {
        wait_loaded(ctx, mid).await?;
    }
    Ok(())
}

// ---------- thinking: what a chat model does, from its own /v1/models "thinking" object (TiinyOS 0.1.36+) ----------
/// The model's thinking object from /v1/models, or None (older TiinyOS, or the model isn't loaded).
pub async fn thinking_of(ctx: &Ctx, model: &str) -> Option<Value> {
    let (code, res) = ctx.call("GET", "/v1/models", None, 15).await;
    if code != 200 {
        return None;
    }
    let item = res
        .get("data")
        .and_then(Value::as_array)?
        .iter()
        .find(|x| x.get("id").and_then(Value::as_str) == Some(model))?;
    item.get("thinking").filter(|t| t.is_object()).cloned()
}

/// "none" (never reasons), "switch" (enable_thinking turns it on/off) or "always" (reasons whatever it's sent, like
/// gpt-oss; tested on 0.1.38: neither enable_thinking nor reasoning_effort changes it reliably).
pub fn thinking_mode(model: &str, thinking: Option<&Value>) -> &'static str {
    match thinking {
        // no thinking object: older TiinyOS, where gpt-oss also rejected enable_thinking
        None => {
            if model.contains("gpt-oss") {
                "always"
            } else {
                "switch"
            }
        }
        Some(t) if !t.get("supported").is_some_and(crate::device::truthy) => "none",
        Some(t) => {
            if t.get("toggleable").is_some_and(crate::device::truthy) {
                "switch"
            } else {
                "always"
            }
        }
    }
}

/// The running instance of a model (its port etc.), or the "isn't loaded" error the UI shows.
pub async fn running_instance(ctx: &Ctx, model: &str) -> Result<Value, JobError> {
    let (_, run) = ctx.call("GET", "/api/v1/models/running", None, 15).await;
    run.pointer("/instances/running")
        .and_then(Value::as_array)
        .and_then(|r| {
            r.iter()
                .find(|r| r.get("model_id").and_then(Value::as_str) == Some(model))
                .cloned()
        })
        .ok_or_else(|| {
            JobError(format!(
                "{} isn't loaded. Load it from the Models panel.",
                short(model)
            ))
        })
}

/// For work routed through the gateway (port 80): just check the model is loaded.
pub async fn ensure_running(ctx: &Ctx, model: &str) -> Result<(), JobError> {
    running_instance(ctx, model).await.map(|_| ())
}

/// Direct URL of a model's own container (media.py instance_url). Only the music models need this: the gateway
/// has no route for them. Some links (the USB cable) only expose the gateway, so check the port first.
pub async fn instance_url(ctx: &Ctx, model: &str) -> Result<String, JobError> {
    let r = running_instance(ctx, model).await?;
    let host = ctx.target.host();
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
pub async fn http(
    ctx: &Ctx,
    method: &str,
    url: &str,
    body: Option<Value>,
    timeout: u64,
    gateway_auth: bool,
) -> Result<(u16, Value), JobError> {
    let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET);
    let mut req = ctx
        .state
        .http
        .request(method, url)
        .timeout(Duration::from_secs(timeout));
    if gateway_auth {
        req = req.bearer_auth(&ctx.target.key);
    }
    if let Some(b) = body {
        req = req.json(&b);
    }
    let res = req
        .send()
        .await
        .map_err(|e| JobError(format!("Device unreachable: {e}")))?;
    let code = res.status().as_u16();
    let raw = res
        .bytes()
        .await
        .map_err(|e| JobError(format!("Device unreachable: {e}")))?;
    if raw.is_empty() {
        return Ok((code, json!({})));
    }
    match serde_json::from_slice(&raw) {
        Ok(v) => Ok((code, v)),
        // an error answer's own text; a 200 that isn't JSON (a login page, a proxy) is an error answer too
        Err(_) => Ok((
            if code >= 400 { code } else { 502 },
            json!({"error": String::from_utf8_lossy(&raw).chars().take(500).collect::<String>()}),
        )),
    }
}

/// Like `http` with a raw request body (bytes + content type) and raw bytes back for answers below 400 (media.py
/// Media.http with raw=True); error answers come back as JSON like `http`.
pub async fn http_bytes(
    ctx: &Ctx,
    method: &str,
    url: &str,
    body: Option<(Vec<u8>, String)>,
    timeout: u64,
    gateway_auth: bool,
) -> Result<(u16, Result<Vec<u8>, Value>), JobError> {
    let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET);
    let mut req = ctx
        .state
        .http
        .request(method, url)
        .timeout(Duration::from_secs(timeout));
    if gateway_auth {
        req = req.bearer_auth(&ctx.target.key);
    }
    if let Some((b, ctype)) = body {
        req = req.header("Content-Type", ctype).body(b);
    }
    let res = req
        .send()
        .await
        .map_err(|e| JobError(format!("Device unreachable: {e}")))?;
    let code = res.status().as_u16();
    let raw = res
        .bytes()
        .await
        .map_err(|e| JobError(format!("Device unreachable: {e}")))?;
    if code < 400 {
        return Ok((code, Ok(raw.to_vec())));
    }
    let v = serde_json::from_slice(&raw).unwrap_or_else(
        |_| json!({"error": String::from_utf8_lossy(&raw).chars().take(500).collect::<String>()}),
    );
    Ok((code, Err(v)))
}

/// The JSON body of an `http_bytes` answer (what `http` returns for a JSON request).
pub fn json_reply(code: u16, reply: Result<Vec<u8>, Value>) -> Result<(u16, Value), JobError> {
    match reply {
        Err(v) => Ok((code, v)),
        Ok(b) if b.is_empty() => Ok((code, json!({}))),
        Ok(b) => Ok(serde_json::from_slice(&b).map_or_else(|_| (502, json!({"error": String::from_utf8_lossy(&b).chars().take(500).collect::<String>()})), |v| (code, v))),
    }
}

/// A JSON request body for `http_bytes`.
pub fn json_body(v: &Value) -> Option<(Vec<u8>, String)> {
    Some((v.to_string().into_bytes(), "application/json".into()))
}

/// A file part of a multipart body: (name, data, content type).
pub type Upload<'a> = (&'a str, &'a [u8], &'a str);

/// multipart/form-data body and content type (media.py Media.multipart); files are (field, (name, data, type)).
pub fn multipart(fields: &[(&str, String)], files: &[(&str, Upload)]) -> (Vec<u8>, String) {
    let b = uuid::Uuid::new_v4().simple().to_string();
    let mut body = vec![];
    for (k, v) in fields {
        body.extend_from_slice(
            format!("--{b}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n")
                .as_bytes(),
        );
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
            let e = [o.get("error"), o.get("detail")]
                .into_iter()
                .flatten()
                .find(|v| crate::device::truthy(v))
                .cloned()
                .unwrap_or(res.clone());
            match e {
                Value::Object(ref m) => m
                    .get("message")
                    .filter(|v| crate::device::truthy(v))
                    .map(py_str)
                    .unwrap_or_else(|| py_json(&e)),
                Value::Array(items) => items
                    .iter()
                    .map(|x| {
                        let loc: Vec<String> = x
                            .get("loc")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .map(py_str)
                            .collect();
                        format!(
                            "{}: {}",
                            loc.join("."),
                            x.get("msg").map(py_str).unwrap_or_else(|| "None".into())
                        )
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
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter()
                .map(|(k, v)| format!("{}: {}", Value::String(k.clone()), py_json(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Array(a) => format!("[{}]", a.iter().map(py_json).collect::<Vec<_>>().join(", ")),
        other => other.to_string(),
    }
}

pub fn decode_uri(uri: &str) -> Result<(String, Vec<u8>), JobError> {
    crate::library::decode_uri(uri).map_err(|_| JobError("Expected a base64 data URI".into()))
}

/// A result's file name with the local date and time in front, so a file manager sorts results by when they were made
/// (#58): `<id>-video.mp4` becomes `2026-11-18_2215_<id>-video.mp4`. Fixed length, always safe, still unique.
pub fn dated(name: &str) -> String {
    format!("{}_{name}", chrono::Local::now().format("%Y-%m-%d_%H%M"))
}

pub fn save(ctx: &Ctx, name: &str, data: &[u8]) -> Result<String, JobError> {
    let name = &dated(name);
    std::fs::write(ctx.state.store.images.join(name), data).map_err(|e| {
        JobError(format!(
            "Couldn't save the result: {}",
            crate::store::io_reason(&e)
        ))
    })?;
    ctx.saved
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(name.to_string());
    Ok(name.to_string())
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
    if ctx.req.get("save").is_some() {
        save(&ctx, &format!("{}-x.png", ctx.id), b"x")?;
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
        assert_eq!(
            err(&json!({"detail": [{"loc": ["body", "n"], "msg": "bad"}]})),
            "body.n: bad"
        );
        assert_eq!(err(&json!({"error": {"code": 1}})), r#"{"code": 1}"#);
        assert_eq!(err(&json!({"x": 1})), r#"{"x": 1}"#);
        assert_eq!(err(&json!("plain")), "plain");
    }

    #[tokio::test]
    async fn a_result_that_cannot_be_saved_fails_the_job() {
        let state = crate::AppState::new(crate::Config {
            port: 0,
            host: "127.0.0.1".into(),
            password: String::new(),
            lan_access: false,
            app: false,
            data_dir: std::env::temp_dir()
                .join(format!("studio-save-{}", uuid::Uuid::new_v4().simple())),
        });
        let ctx = state.jobs.test_ctx(&state);
        let name = save(&ctx, "a.png", b"x").unwrap(); // dated: 2026-11-18_2215_a.png (#58)
        assert!(
            name.len() == "YYYY-MM-DD_HHMM_a.png".len()
                && name.ends_with("_a.png")
                && name.as_bytes()[10] == b'_'
        );
        assert!(state.store.images.join(&name).is_file());
        std::fs::remove_dir_all(&state.store.images).unwrap();
        std::fs::write(&state.store.images, b"").unwrap(); // a file where the folder was
        let JobError(msg) = save(&ctx, "b.png", b"x").unwrap_err();
        assert!(
            msg.starts_with("Couldn't save the result: ")
                && msg.len() > 26
                && !msg.contains("os error"),
            "{msg}"
        );
    }

    /// A device with a loaded chat model that has to make room for an image model.
    #[derive(Default)]
    struct Dev {
        running: std::sync::Mutex<Vec<String>>,
        pending: std::sync::Mutex<Vec<String>>,
        log: std::sync::Mutex<Vec<String>>,
        /// launches of "org/img" answered 409 cleanup_failed before one works
        cleanup_failures: std::sync::atomic::AtomicUsize,
        /// launches of "org/img" answered 500
        refuse_img: std::sync::atomic::AtomicBool,
        /// a launch that is accepted but never shows up
        never_starts: std::sync::atomic::AtomicBool,
    }

    async fn dev_answer(
        axum::extract::State(d): axum::extract::State<std::sync::Arc<Dev>>,
        req: axum::extract::Request,
    ) -> axum::response::Response {
        use axum::response::IntoResponse;
        use std::sync::atomic::Ordering::SeqCst;
        let (m, p) = (req.method().to_string(), req.uri().path().to_string());
        d.log.lock().unwrap().push(format!("{m} {p}"));
        let j = |c: u16, v: Value| {
            (axum::http::StatusCode::from_u16(c).unwrap(), axum::Json(v)).into_response()
        };
        let model = |p: &str| {
            let rest = p.strip_prefix("/api/v1/models/").unwrap_or("");
            let id = rest.rsplit_once('/').map(|x| x.0).unwrap_or(rest);
            id.replace("%2F", "/")
        };
        match (m.as_str(), p.as_str()) {
            ("GET", "/api/v1/models/running") => {
                let run: Vec<Value> = d
                    .running
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|m| json!({"model_id": m}))
                    .collect();
                let pend: Vec<Value> = d
                    .pending
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|m| json!({"model_id": m}))
                    .collect();
                j(200, json!({"instances": {"running": run, "pending": pend}}))
            }
            ("GET", p) if p.ends_with("/unload_candidate") => {
                let victims = d.running.lock().unwrap().clone();
                j(
                    200,
                    json!({"unload_required": !victims.is_empty(), "model_id": victims}),
                )
            }
            ("POST", p) if p.ends_with("/stop") => {
                let id = model(p);
                d.running.lock().unwrap().retain(|m| *m != id);
                j(200, json!({"status": "ok"}))
            }
            ("POST", p) if p.ends_with("/launch") => {
                let id = model(p);
                if id == "org/img" {
                    if d.refuse_img.load(SeqCst) {
                        return j(500, json!({"error": {"message": "no room"}}));
                    }
                    if d.cleanup_failures.load(SeqCst) > 0 {
                        d.cleanup_failures.fetch_sub(1, SeqCst);
                        return j(
                            409,
                            json!({"error": {"code": "cleanup_failed", "message": "runtime cleanup failed and must be retried"}}),
                        );
                    }
                    if d.never_starts.load(SeqCst) {
                        return j(200, json!({"status": "ok"}));
                    }
                }
                d.running.lock().unwrap().push(id);
                j(200, json!({"status": "ok"}))
            }
            _ => j(404, json!({"detail": "Not Found"})),
        }
    }

    async fn device(running: &[&str]) -> (std::sync::Arc<Dev>, crate::AppState, crate::jobs::Ctx) {
        let d = std::sync::Arc::new(Dev::default());
        *d.running.lock().unwrap() = running.iter().map(|m| m.to_string()).collect();
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let app = axum::Router::new()
            .fallback(dev_answer)
            .with_state(d.clone());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        let state = crate::AppState::new(crate::Config {
            port: 0,
            host: "127.0.0.1".into(),
            password: String::new(),
            lan_access: false,
            app: false,
            data_dir: std::env::temp_dir()
                .join(format!("studio-room-{}", uuid::Uuid::new_v4().simple())),
        });
        let mut ctx = state.jobs.test_ctx(&state);
        ctx.target.url = url;
        ctx.target.key = "k".into();
        (d, state, ctx)
    }

    #[tokio::test]
    async fn a_refused_swap_loads_the_users_model_back() {
        let (d, _state, ctx) = device(&["org/chat"]).await;
        d.refuse_img
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let e = make_room(&ctx, "org/img").await.unwrap_err();
        assert_eq!(e.0, "no room"); // the load that failed is what the user hears about
        assert_eq!(*d.running.lock().unwrap(), vec!["org/chat".to_string()]);
    }

    #[tokio::test]
    async fn a_stopped_swap_loads_the_users_model_back() {
        let (d, _state, ctx) = device(&["org/chat"]).await;
        d.never_starts
            .store(true, std::sync::atomic::Ordering::SeqCst);
        ctx.cancel.store(true, std::sync::atomic::Ordering::SeqCst); // Stop while "Loading org/img"
        let e = make_room(&ctx, "org/img").await.unwrap_err();
        assert_eq!(e.0, "Cancelled");
        assert_eq!(*d.running.lock().unwrap(), vec!["org/chat".to_string()]);
    }

    #[tokio::test]
    async fn a_launch_refused_with_cleanup_failed_is_tried_again() {
        let (d, _state, ctx) = device(&["org/chat"]).await;
        d.cleanup_failures
            .store(2, std::sync::atomic::Ordering::SeqCst);
        let unloaded = make_room(&ctx, "org/img").await.unwrap();
        assert_eq!(unloaded, vec!["org/chat".to_string()]);
        let launches = d
            .log
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.ends_with("org%2Fimg/launch"))
            .count();
        assert_eq!(launches, 3);
        // a third refusal in a row is the answer
        let (d, _state, ctx) = device(&["org/chat"]).await;
        d.cleanup_failures
            .store(5, std::sync::atomic::Ordering::SeqCst);
        let e = make_room(&ctx, "org/img").await.unwrap_err();
        assert!(e.0.contains("must be retried"), "{e}");
    }

    #[tokio::test]
    async fn a_load_that_never_starts_says_so() {
        let (d, _state, ctx) = device(&[]).await;
        d.never_starts
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let e = make_room(&ctx, "org/img").await.unwrap_err();
        assert!(e.0.contains("never started loading"), "{e}");
        // a model the device did list as loading is just slow
        d.pending.lock().unwrap().push("org/img".into());
        let e = wait_loaded(&ctx, "org/img").await.unwrap_err();
        assert_eq!(e.0, "img didn't finish loading");
    }

    #[test]
    fn a_200_that_is_not_json_is_an_error_answer() {
        let (code, v) = json_reply(200, Ok(b"<html>Please log in</html>".to_vec())).unwrap();
        assert_eq!(
            (code, err(&v).as_str()),
            (502, "<html>Please log in</html>")
        );
        assert_eq!(
            json_reply(200, Ok(b"{\"a\": 1}".to_vec())).unwrap(),
            (200, json!({"a": 1}))
        );
    }
}
