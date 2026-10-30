//! Instrument loops and songs (media.py run_loop, run_song, _compose, song_progress, _song_entry).

use std::time::Duration;

use base64::Engine;
use serde_json::{json, Map, Value};

use super::audio::strip;
use super::{decode_uri, ensure_running, err, http, http_bytes, instance_url, json_body, port_open, py_round, running_instance, save};
use crate::device::truthy;
use crate::jobs::{Ctx, JobError, JobResult};

const LOOP_CONFIG: [&str; 4] = ["seconds_total", "steps", "cfg_scale", "seed"];
const SONG_CONFIG: [&str; 9] = ["caption", "lyrics", "generation_type", "seconds_total", "seed", "steps", "temperature", "top_k", "top_p"];
const FULL_SONG_SECONDS: f64 = 240.0; // at or above this, the gateway path renders the model's full-length arrangement

/// AceStep's own settings, sent nested as config.ace_step (at the top level it answers "Extra inputs are not permitted").
const ACE_STEP_KEYS: [&str; 5] = ["bpm", "key_scale", "time_signature", "vocal_language", "negative_prompt"];

/// True for song models that only take the gateway's plain /generate, with no preview, resume or own-port API (AceStep,
/// YuE2: "... preview generation is not implemented"); false for SongGeneration. Read from the model's music service
/// (/v1/music/info, 0.1.38); by its id when the device doesn't say.
async fn plain_song_model(ctx: &Ctx, model: &str) -> bool {
    let (code, info) = crate::device::call(&ctx.state, "GET", &format!("/v1/music/info?model={}", quote(model)), None, 15, None).await;
    match info.get("service_name").and_then(Value::as_str).filter(|_| code == 200) {
        Some(service) => !service.starts_with("SongGeneration"),
        None => model.starts_with("ACE-Step/"),
    }
}

/// {k: req[k] for k in keys if req.get(k) not in (None, "")}
fn pick(req: &Value, keys: &[&str]) -> Map<String, Value> {
    keys.iter()
        .filter_map(|k| match req.get(*k) {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if s.is_empty() => None,
            Some(v) => Some((k.to_string(), v.clone())),
        })
        .collect()
}

/// urllib.parse.quote(s, safe="").
fn quote(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"_.-~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// float(v) of a request value.
fn to_float(v: &Value) -> Result<f64, JobError> {
    match v {
        Value::Number(n) => Ok(n.as_f64().unwrap_or(0.0)),
        Value::Bool(b) => Ok(*b as u8 as f64),
        Value::String(s) => s.trim().parse().map_err(|_| JobError(format!("ValueError: could not convert string to float: '{s}'"))),
        _ => Err(JobError("TypeError: float() argument must be a string or a real number".into())),
    }
}

/// str() of a JSON value in an f-string.
fn show(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn text<'a>(req: &'a Value, k: &str) -> &'a str {
    req.get(k).and_then(Value::as_str).unwrap_or("")
}

pub async fn run_loop(ctx: Ctx) -> JobResult {
    let req = &ctx.req;
    let prompt = strip(text(req, "prompt")).to_string();
    if prompt.is_empty() {
        return Err(JobError("Describe the loop".into()));
    }
    let config = pick(req, &LOOP_CONFIG);
    let model = ctx.model.clone();
    ensure_running(&ctx, &model).await?;
    ctx.update(json!({"message": "Generating loop"}));
    // Gateway music route (Tiiny SDK, Oct 2026) works on every link, the USB cable included. Foundation-1
    // still takes its native body ({prompt, config}); older firmware without the route falls back to the port.
    let gw = format!("{}/v1/music/generate/wav", ctx.state.settings.current().url);
    let body = json!({"model": model, "prompt": prompt, "config": config});
    let (mut code, mut res) = http_bytes(&ctx, "POST", &gw, json_body(&body), 3600, true).await?;
    if code == 404 && res.as_ref().is_err_and(|v| v.get("detail").is_some()) {
        let url = instance_url(&ctx, &model).await?;
        let body = json!({"prompt": prompt, "config": config});
        (code, res) = http_bytes(&ctx, "POST", &format!("{url}/v1/generate/wav"), json_body(&body), 3600, false).await?;
    }
    let wav = match res {
        Ok(w) if code == 200 => w,
        Ok(other) => return Err(JobError(err(&json!(String::from_utf8_lossy(&other))))),
        Err(v) => return Err(JobError(err(&v))),
    };
    let out = save(&ctx, &format!("{}-loop.wav", ctx.id), &wav);
    let mut params = Map::new();
    params.insert("model".into(), json!(model));
    params.insert("prompt".into(), json!(prompt));
    params.extend(config.clone());
    for k in ["bpm", "bars", "key"] {
        params.insert(k.into(), req.get(k).cloned().unwrap_or(Value::Null));
    }
    Ok(json!({"kind": "loop", "params": params, "sources": [], "outputs": [out], "duration": config.get("seconds_total")}))
}

pub async fn run_song(ctx: Ctx) -> JobResult {
    let req = &ctx.req;
    let model = ctx.model.clone();
    let mut config = pick(req, &SONG_CONFIG);
    if config.get("generation_type") != Some(&json!("bgm")) && strip(config.get("lyrics").and_then(Value::as_str).unwrap_or("")).is_empty() {
        return Err(JobError("A song with vocals needs lyrics (or switch to Instrumental)".into()));
    }
    let mut sources = vec![];
    if req.get("reference_audio").is_some_and(truthy) {
        let (_, reference) = decode_uri(text(req, "reference_audio"))?;
        config.insert("reference_audio_base64".into(), json!(base64::engine::general_purpose::STANDARD.encode(&reference)));
        sources.push(save(&ctx, &format!("{}-styleref.wav", ctx.id), &reference));
    }
    let ace = req.get("ace_step").cloned().unwrap_or(Value::Null);
    let ace = pick(&ace, &ACE_STEP_KEYS);
    if !ace.is_empty() {
        config.insert("ace_step".into(), Value::Object(ace));
    }
    let inst = running_instance(&ctx, &model).await?;
    let plain = plain_song_model(&ctx, &model).await;
    let cur = ctx.state.settings.current();
    let host = cur.host();
    let gw = format!("{}/v1/music/generate", cur.url);
    let port = inst.get("port").and_then(Value::as_u64);
    let direct = !plain && port_open(&host, port).await && !req.get("preview").is_some_and(truthy) && !req.get("resume_session").is_some_and(truthy);
    let direct_url = direct.then(|| format!("http://{host}:{}", port.unwrap_or(0)));
    let progress = tokio::spawn(song_progress(ctx.clone(), direct_url));
    let composed = compose(&ctx, &mut config, &inst, direct, plain, &gw, &host).await;
    progress.abort();
    let (code, res, route, url) = composed?;
    if code != 200 || !res.get("success").is_some_and(truthy) || !res.get("audio_data").is_some_and(truthy) {
        return Err(JobError(err(&res)));
    }
    song_entry(&ctx, &res, &config, sources, route, url).await
}

async fn compose(ctx: &Ctx, config: &mut Map<String, Value>, inst: &Value, direct: bool, plain: bool, gw: &str, host: &str) -> Result<(u16, Value, &'static str, Option<String>), JobError> {
    let req = &ctx.req;
    let model = &ctx.model;
    let post = |path: &str, config: Value, session: Option<Value>| {
        let mut body = json!({"model": model, "config": config});
        if let Some(s) = session {
            body["session_id"] = s;
        }
        (format!("{gw}{path}"), body)
    };
    if plain {
        // AceStep (0.1.36+) and YuE2 (0.1.38): the gateway's plain /generate composes the whole song at seconds_total
        // on every link; they have no preview or resume. A preview request becomes an 8-second song.
        if req.get("preview").is_some_and(truthy) && !config.get("seconds_total").is_some_and(truthy) {
            config.insert("seconds_total".into(), json!(8));
        }
        ctx.update(json!({"message": "Composing"}));
        let (url, body) = post("", json!(config), None);
        let (code, res) = http(ctx, "POST", &url, Some(body), 3600, true).await?;
        return Ok((code, res, "gateway", None));
    }
    if req.get("resume_session").is_some_and(truthy) {
        // Continue a saved preview to the full-length song (the gateway renders the whole arrangement).
        ctx.update(json!({"message": "Composing the full song (this takes a while)"}));
        config.remove("seconds_total");
        let (url, body) = post("/resume", json!(config), req.get("resume_session").cloned());
        let (code, res) = http(ctx, "POST", &url, Some(body), 3600, true).await?;
        return Ok((code, res, "gateway-full", None));
    }
    if req.get("preview").is_some_and(truthy) {
        ctx.update(json!({"message": "Composing an 8-second preview"}));
        config.remove("seconds_total");
        let mut c = config.clone();
        c.insert("preview_seconds".into(), json!(8));
        let (url, body) = post("/preview", json!(c), None);
        let (code, res) = http(ctx, "POST", &url, Some(body), 3600, true).await?;
        return Ok((code, res, "preview", None));
    }
    if direct {
        // Direct to the model's port (Wi-Fi/LAN): composes a complete song at exactly seconds_total.
        let url = format!("http://{host}:{}", inst.get("port").and_then(Value::as_u64).unwrap_or(0));
        ctx.update(json!({"message": "Composing"}));
        let (code, res) = http(ctx, "POST", &format!("{url}/v1/generate"), Some(json!({"config": config})), 3600, false).await?;
        return Ok((code, res, "direct", Some(url)));
    }
    // Over links that only expose the gateway (the USB cable). Its plain /generate fails ("Session not
    // found", TiinyOS 0.1.35) and /resume ignores seconds_total, so: a preview of the requested length (the
    // opening of a full-length arrangement), or preview + resume for a full-length song.
    let secs = match config.remove("seconds_total").filter(truthy) {
        Some(v) => to_float(&v)?,
        None => 60.0,
    };
    let route;
    let (code, res);
    if secs >= FULL_SONG_SECONDS {
        route = "gateway-full";
        ctx.update(json!({"message": "Composing the opening"}));
        let mut c = config.clone();
        c.insert("preview_seconds".into(), json!(8));
        let (url, body) = post("/preview", json!(c), None);
        let (c1, r1) = http(ctx, "POST", &url, Some(body), 3600, true).await?;
        match r1.get("session_id").filter(|s| truthy(s)).cloned() {
            Some(sid) if c1 == 200 => {
                ctx.update(json!({"message": "Composing the full song (this takes a while)"}));
                let (url, body) = post("/resume", json!(config), Some(sid));
                (code, res) = http(ctx, "POST", &url, Some(body), 3600, true).await?;
            }
            _ => (code, res) = (c1, r1),
        }
    } else {
        route = "gateway-preview";
        ctx.update(json!({"message": "Composing"}));
        let mut c = config.clone();
        c.insert("preview_seconds".into(), json!(secs));
        let (url, body) = post("/preview", json!(c), None);
        (code, res) = http(ctx, "POST", &url, Some(body), 3600, true).await?;
    }
    config.insert("seconds_total".into(), json!(secs));
    Ok((code, res, route, None))
}

/// Poll the session list for the running song's progress (stays put while the model can't measure it).
async fn song_progress(ctx: Ctx, direct_url: Option<String>) {
    loop {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let polled = match &direct_url {
            Some(u) => http(&ctx, "GET", &format!("{u}/v1/sessions"), None, 5, false).await,
            None => {
                let url = format!("{}/v1/music/sessions?model={}", ctx.state.settings.current().url, quote(&ctx.model));
                http(&ctx, "GET", &url, None, 5, true).await
            }
        };
        let Ok((_, r)) = polled else { continue };
        let best = r
            .get("sessions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|s| !matches!(s.get("status").and_then(Value::as_str), Some("completed" | "failed" | "cancelled")))
            .filter_map(|s| s.get("progress").and_then(Value::as_f64))
            .fold(None, |m: Option<f64>, p| Some(m.map_or(p, |m| m.max(p))));
        if let Some(p) = best.filter(|p| *p > 0.0) {
            ctx.update(json!({"progress": py_round(p * 100.0).min(99)}));
        }
    }
}

fn b64(s: &str) -> Result<Vec<u8>, JobError> {
    base64::engine::general_purpose::STANDARD.decode(s).map_err(|e| JobError(format!("Error: {e}")))
}

async fn song_entry(ctx: &Ctx, res: &Value, config: &Map<String, Value>, sources: Vec<String>, route: &str, url: Option<String>) -> JobResult {
    let req = &ctx.req;
    let mut outputs = vec![save(ctx, &format!("{}-song.wav", ctx.id), &b64(text(res, "audio_data"))?)];
    let mut tracks = Map::new();
    for (name, data) in res.get("audio_outputs").and_then(Value::as_object).into_iter().flatten() {
        let Some(data) = data.as_str().filter(|d| d.chars().count() > 1000 && name.to_lowercase() != "mixed") else { continue }; // "mixed" duplicates the main file
        let mut slug = String::new();
        for c in name.to_lowercase().chars() {
            if c.is_ascii_lowercase() || c.is_ascii_digit() {
                slug.push(c);
            } else if !slug.ends_with('_') {
                slug.push('_');
            }
        }
        let fname = save(ctx, &format!("{}-{slug}.wav", ctx.id), &b64(data)?);
        outputs.push(fname.clone());
        tracks.insert(fname, json!(name));
    }
    let (mut downloads, mut resume) = (vec![], None);
    let sid = res.get("session_id").filter(|s| truthy(s)).map(show);
    if let (Some(url), Some(sid)) = (&url, &sid) {
        http(ctx, "DELETE", &format!("{url}/v1/sessions/{sid}"), None, 15, false).await?;
    } else if let Some(sid) = &sid {
        // Gateway songs can also be fetched as MP3 from their session.
        let dl = format!("{}/v1/music/sessions/{sid}/download?format=mp3", ctx.state.settings.current().url);
        if let Ok((200, Ok(mp3))) = http_bytes(ctx, "GET", &dl, None, 120, true).await {
            if mp3.len() > 1000 {
                downloads.push(save(ctx, &format!("{}-song.mp3", ctx.id), &mp3));
            }
        }
        if route == "preview" {
            resume = Some(json!({"session_id": sid})); // a preview can be continued once
        }
    }
    if req.get("resume_session").is_some_and(truthy) {
        if let Some(from) = req.get("from_entry").filter(|f| truthy(f)).and_then(Value::as_str) {
            ctx.state.store.edit_history(from, |e| {
                e.remove("resume");
            });
        }
    }
    let mut params = Map::new();
    params.insert("model".into(), json!(ctx.model));
    params.insert("route".into(), json!(route));
    params.extend(config.iter().filter(|(k, _)| *k != "reference_audio_base64").map(|(k, v)| (k.clone(), v.clone())));
    let mut entry = json!({"kind": "song", "params": params, "sources": sources, "outputs": outputs, "tracks": tracks,
        "duration": res.get("duration"), "downloads": downloads});
    if let Some(r) = resume {
        entry["resume"] = r;
    }
    Ok(entry)
}
