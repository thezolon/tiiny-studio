//! Every route the UI calls, with the same paths, status codes and JSON as server.py's Handler.

use axum::{
    body::Bytes,
    extract::{Path, Query, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use rust_embed::RustEmbed;
use serde_json::{json, Value};

use crate::device;
use crate::settings::{hostname, normalize_url};
use crate::AppState;

#[derive(RustEmbed)]
#[folder = "../../static/"]
struct Static;

/// Top-level docs/*.md, served as plain text for the in-app help links.
#[derive(RustEmbed)]
#[folder = "../../docs/"]
#[include = "*.md"]
#[exclude = "*/*"]
struct Docs;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/static/{*path}", get(static_file))
        .route("/docs/{name}", get(doc))
        .route("/api/settings", get(settings).post(save_settings))
        .route("/api/settings/test", post(test_settings))
        .route("/api/open-folder", post(open_folder))
        .route("/api/clipboard-image", get(clipboard_image))
        .route("/api/settings/{action}", post(device_action))
        .route("/api/models", get(models))
        .route("/api/catalog", get(catalog))
        .route("/api/stats", get(stats))
        .route("/api/busy", get(busy))
        .route("/api/model/plan", get(model_plan))
        .route("/api/model/info", get(model_info))
        .route("/api/model/launch", post(model_launch))
        .route("/api/model/stop", post(model_stop))
        .route("/api/model/unload_all", post(unload_all))
        .route("/api/device/shutdown", post(shutdown))
        .route("/api/icon/{org}/{name}", get(icon))
        .route("/api/extract", post(extract_document))
        .merge(crate::library::routes())
        .fallback(not_found)
        // same cap as server.py (for a body without a Content-Length too); /api/extract checks its own 100 MB
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY))
        .layer(middleware::from_fn(body_limit))
        .layer(middleware::from_fn_with_state(state.clone(), authorized))
        .layer(middleware::from_fn_with_state(state.clone(), same_site))
        .with_state(state)
}

/// Keeps other websites out. Without this, any page open in the browser could POST here (wipe everything, power
/// off the Tiiny) or, by pointing its own domain at 127.0.0.1 (DNS rebinding), read the gallery. So: unless the
/// studio is opened to the LAN, only localhost addresses are answered; and a change (anything but GET/HEAD) from a
/// browser must come from the studio's own page.
async fn same_site(State(state): State<AppState>, req: Request, next: Next) -> Response {
    match foreign(req.method(), req.headers(), state.config.lan_access) {
        Some(why) => error(StatusCode::FORBIDDEN, why),
        None => next.run(req).await,
    }
}

fn foreign(method: &axum::http::Method, headers: &axum::http::HeaderMap, lan_access: bool) -> Option<&'static str> {
    let text = |h| headers.get(h).and_then(|v: &header::HeaderValue| v.to_str().ok()).map(str::to_ascii_lowercase);
    let host = text(header::HOST);
    if let (false, Some(h)) = (lan_access, &host) {
        let name = match h.strip_prefix('[') {
            Some(rest) => rest.split(']').next().unwrap_or(""),
            None => h.rsplit_once(':').map_or(h.as_str(), |(n, _)| n),
        };
        if !matches!(name, "localhost" | "127.0.0.1" | "::1") {
            return Some("This studio only answers at localhost.");
        }
    }
    if matches!(*method, axum::http::Method::GET | axum::http::Method::HEAD) {
        return None;
    }
    // browsers send Origin with every cross-site POST/DELETE (and "null" from sandboxed or file pages)
    let origin = text(header::ORIGIN)?;
    let authority = origin.strip_prefix("http://").or_else(|| origin.strip_prefix("https://"));
    if authority.is_some() && authority == host.as_deref() {
        return None;
    }
    Some("Requests from other websites aren't allowed.")
}

/// Optional password (STUDIO_PASSWORD): HTTP basic auth, any username, on every request.
async fn authorized(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let password = &state.config.password;
    if password.is_empty() {
        return next.run(req).await;
    }
    let given = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|b| base64_decode(b.trim()))
        .and_then(|raw| String::from_utf8(raw).ok())
        .map(|s| s.split_once(':').map(|(_, p)| p.to_string()).unwrap_or_default());
    if given.is_some_and(|g| constant_time_eq(g.as_bytes(), password.as_bytes())) {
        return next.run(req).await;
    }
    (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, r#"Basic realm="Tiiny Studio", charset="UTF-8""#)]).into_response()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}

pub fn error(code: StatusCode, message: &str) -> Response {
    (code, Json(json!({"error": {"message": message}}))).into_response()
}

fn reply((code, body): (u16, Value)) -> Response {
    (StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_GATEWAY), Json(body)).into_response()
}

/// The request body as a JSON object, like Handler.read_json(): empty means {}; anything else that isn't an object is
/// an error to answer 400 with (same words as server.py).
pub(crate) fn read_json(body: &Bytes) -> Result<Value, String> {
    if body.is_empty() {
        return Ok(json!({}));
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(v) if v.is_object() => Ok(v),
        Ok(_) => Err("The request body must be a JSON object.".into()),
        Err(_) => Err("The request body isn't valid JSON.".into()),
    }
}

/// The largest request body: pictures, voice clips and video frames arrive as base64 JSON; /api/extract checks its own 100 MB.
const MAX_BODY: usize = 150 << 20;
const TOO_LARGE: &str = "That request is too large (over 150 MB).";

async fn body_limit(req: Request, next: Next) -> Response {
    let len = req.headers().get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<usize>().ok());
    if len.is_some_and(|n| n > MAX_BODY) {
        return error(StatusCode::PAYLOAD_TOO_LARGE, TOO_LARGE);
    }
    next.run(req).await
}

pub(crate) fn str_of<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

async fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "not found")
}

async fn index() -> Response {
    embedded("index.html", "text/html; charset=utf-8")
}

/// Same files server.py serves: `name`, `vendor/name`, `vendor/pdfjs/name`, `vendor/pdfjs/cmaps/name`.
fn allowed_static(path: &str) -> bool {
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    safe_name(name) && matches!(dir, "" | "vendor" | "vendor/pdfjs" | "vendor/pdfjs/cmaps")
}

fn safe_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '-')
}

async fn static_file(Path(path): Path<String>) -> Response {
    if !allowed_static(&path) {
        return not_found().await;
    }
    embedded(&path, mime(&path))
}

async fn doc(Path(name): Path<String>) -> Response {
    let ok = name.ends_with(".md") && name[..name.len() - 3].chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-');
    match Docs::get(&name).filter(|_| ok && !name.starts_with(".md")) {
        Some(f) => ([(header::CONTENT_TYPE, "text/plain; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], f.data.into_owned()).into_response(),
        None => error(StatusCode::NOT_FOUND, "not found"),
    }
}

fn embedded(path: &str, ctype: &str) -> Response {
    match Static::get(path) {
        Some(file) => ([(header::CONTENT_TYPE, ctype), (header::CACHE_CONTROL, "no-cache")], file.data.into_owned()).into_response(),
        None => error(StatusCode::NOT_FOUND, "not found"),
    }
}

/// server.py MIME, by extension; anything else is application/octet-stream.
pub fn mime(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" => "image/jpeg",
        "webp" => "image/webp",
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "webm" => "audio/webm",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "glb" => "model/gltf-binary",
        "mp4" => "video/mp4",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        _ => "application/octet-stream",
    }
}

async fn settings(State(state): State<AppState>, req: Request) -> Json<Value> {
    let mut v = state.settings.public();
    v["can_open_folder"] = json!(can_open_folder(&req));
    Json(v)
}

/// Opening the results folder only makes sense on the computer running the studio, with a desktop:
/// not in Docker, not for a phone or another PC on the LAN.
fn can_open_folder(req: &Request) -> bool {
    let local = req
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .is_some_and(|c| c.0.ip().is_loopback() || c.0.ip().to_canonical().is_loopback());
    let desktop = cfg!(any(windows, target_os = "macos")) || std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
    local && desktop && !std::path::Path::new("/.dockerenv").exists()
}

/// The picture on the system clipboard as a PNG, for the Paste buttons in the desktop app (its webview won't let
/// the page read the clipboard). Only the desktop build has it, and only for requests from this computer.
async fn clipboard_image(State(state): State<AppState>, req: Request) -> Response {
    if !state.config.app || !can_open_folder(&req) {
        return not_found().await;
    }
    #[cfg(feature = "clipboard")]
    {
        let png = tokio::task::spawn_blocking(|| -> Option<Vec<u8>> {
            let img = arboard::Clipboard::new().ok()?.get_image().ok()?;
            let mut out = Vec::new();
            let mut enc = png::Encoder::new(&mut out, img.width as u32, img.height as u32);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().ok()?.write_image_data(&img.bytes).ok()?;
            Some(out)
        })
        .await
        .ok()
        .flatten();
        return match png {
            Some(b) => ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "no-store")], b).into_response(),
            None => error(StatusCode::NOT_FOUND, "There's no picture on the clipboard."),
        };
    }
    #[allow(unreachable_code)]
    not_found().await
}

/// Show the results folder in the system file manager (Explorer, Finder, or the Linux desktop's).
async fn open_folder(State(state): State<AppState>, req: Request) -> Response {
    if !can_open_folder(&req) {
        return error(StatusCode::FORBIDDEN, "The folder can only be opened on the computer running the studio.");
    }
    let dir = state.store.images.clone();
    let program = if cfg!(windows) { "explorer" } else if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    // a fixed folder of ours, never a path from the request
    let _ = std::process::Command::new(program).arg(&dir).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
    Json(json!({"ok": true})).into_response()
}

/// Add or update a saved device ({id?, name, url, key}); without an id on first run, the active one.
async fn save_settings(State(state): State<AppState>, body: Bytes) -> Response {
    let req = match read_json(&body) {
        Ok(v) => v,
        Err(e) => return error(StatusCode::BAD_REQUEST, &e),
    };
    let new = req.get("new").is_some_and(device::truthy);
    let did = Some(str_of(&req, "id")).filter(|s| !s.is_empty()).map(String::from).or_else(|| if new { None } else { state.settings.active() });
    if let Err(e) = state.settings.upsert(did.as_deref(), str_of(&req, "name"), str_of(&req, "url"), str_of(&req, "key").trim()) {
        return error(StatusCode::BAD_REQUEST, &e);
    }
    let check = state.clone();
    tokio::spawn(async move { check.settings.check(&check.http).await });
    Json(state.settings.public()).into_response()
}

async fn device_action(State(state): State<AppState>, Path(action): Path<String>, body: Bytes) -> Response {
    let req = read_json(&body).unwrap_or(json!({}));
    let did = str_of(&req, "id");
    match action.as_str() {
        "delete" => state.settings.remove(did),
        "move" => state.settings.move_device(did, if req.get("dir").and_then(Value::as_f64).unwrap_or(-1.0) < 0.0 { -1 } else { 1 }),
        "activate" => state.settings.activate(did, None),
        "auto" => state.settings.set_auto(req.get("auto").map(device::truthy).unwrap_or(true)),
        "swap" => state.settings.set_swap(&req),
        "check" => state.settings.check(&state.http).await,
        _ => return not_found().await,
    }
    Json(state.settings.public()).into_response()
}

/// Try a device address/key (typed or saved) without saving it.
const NOT_A_TIINY: &str = "This address answers chat requests but has no Tiiny model controls, so it looks like a relay or proxy (for example an OpenAI-compatible gateway). Use your Tiiny's own address and API key from the TiinyOS app.";

/// The USB cable's address is picked from the device's serial in 172.16-172.31; 172.17 is also Docker's default
/// network, so a studio in Docker can't reach it (docs/CONNECTING.md). Said plainly instead of "unreachable".
const DOCKER_USB: &str = "This is a USB address in Docker's own network (172.17.x.x), so the studio in Docker can't reach it. Use the Tiiny's Wi-Fi/LAN address, or the desktop app, or move Docker's network (see the connection guide).";

async fn test_settings(State(state): State<AppState>, body: Bytes) -> Json<Value> {
    let req = read_json(&body).unwrap_or(json!({}));
    let cur = state.settings.current();
    let (saved_url, saved_key) = state
        .settings
        .devices()
        .into_iter()
        .find(|d| d.id == str_of(&req, "id"))
        .map(|d| (d.url, d.key))
        .unwrap_or((cur.url, cur.key));
    let url = Some(normalize_url(str_of(&req, "url"))).filter(|u| !u.is_empty()).unwrap_or(saved_url.clone());
    let mut key = str_of(&req, "key").trim().to_string();
    if key.is_empty() && hostname(&url) == hostname(&saved_url) {
        key = saved_key; // a saved key is only ever tried against its own host
    }
    if url.is_empty() {
        return Json(json!({"ok": false, "message": "Enter the device address."}));
    }
    let (code, res) = device::call(&state, "GET", "/v1/models", None, 8, Some((&url, &key))).await;
    if code == 200 {
        // any OpenAI-style relay answers /v1/models; only a Tiiny has its model controls
        let (tiiny, _) = device::call(&state, "GET", "/api/v1/models/running", None, 8, Some((&url, &key))).await;
        if tiiny != 200 {
            return Json(json!({"ok": false, "message": NOT_A_TIINY}));
        }
        let names: Vec<String> = res
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|m| str_of(m, "id").rsplit('/').next().unwrap_or("").to_string())
            .collect();
        let list = if names.is_empty() { ".".to_string() } else { format!(": {}", names.iter().take(4).cloned().collect::<Vec<_>>().join(", ")) };
        return Json(json!({"ok": true, "message": format!("Connected. {} model(s) loaded{list}", names.len()), "loaded": names}));
    }
    if code == 401 || code == 403 {
        let msg = if key.is_empty() { "The device needs an API key." } else { "The device rejected the API key. Copy the API key (not the device ID) from TiinyOS: avatar → Settings → API Key." };
        return Json(json!({"ok": false, "message": msg}));
    }
    if crate::settings::in_docker() && hostname(&url).starts_with("172.17.") {
        return Json(json!({"ok": false, "message": DOCKER_USB}));
    }
    let msg = match res.get("error") {
        Some(Value::Object(e)) => e.get("message").and_then(Value::as_str).map(String::from),
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
    .filter(|m| !m.is_empty())
    .unwrap_or_else(|| format!("Unexpected response (HTTP {code})."));
    Json(json!({"ok": false, "message": msg}))
}

async fn models(State(state): State<AppState>) -> Response {
    reply(device::call(&state, "GET", "/v1/models", None, 10, None).await)
}

async fn catalog(State(state): State<AppState>) -> Response {
    reply(device::catalog(&state).await)
}

async fn stats(State(state): State<AppState>) -> Json<Value> {
    Json(device::stats(&state).await)
}

async fn busy(State(state): State<AppState>) -> Json<Value> {
    Json(json!({"busy": state.device_lock.try_lock().is_err()}))
}

/// urllib.parse.quote(id, safe=""): everything but letters, digits and _.-~ is %-encoded.
fn quote(id: &str) -> String {
    id.bytes().map(|b| if b.is_ascii_alphanumeric() || b"_.-~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

async fn model_plan(State(state): State<AppState>, Query(q): Query<std::collections::HashMap<String, String>>) -> Response {
    let mid = q.get("id").map(String::as_str).unwrap_or("");
    if mid.is_empty() {
        return error(StatusCode::BAD_REQUEST, "id required");
    }
    reply(device::call(&state, "GET", &format!("/api/v1/models/{}/unload_candidate", quote(mid)), None, 30, None).await)
}

/// Where a loaded model reports its own options (#23), by catalog type. Image, OCR and transcription models have no
/// such route: their options come from the catalog row (supported, capabilities, desc).
fn info_route(model_type: &str) -> Option<&'static str> {
    match model_type {
        "Text Generation" | "Image-Text-to-Text" | "Text-to-Speech" => Some("/v1/capabilities"),
        "Music Generation" => Some("/v1/music/info"),
        "Image-to-3D" => Some("/v1/3d/capabilities"),
        "Text-to-Video" => Some("/v1/video/info"),
        _ => None,
    }
}

/// What a model reports about its options: chat (thinking, vision), voice (clone, speakers, languages), music (config
/// schema and defaults), 3D (stages, params). Only a loaded model answers; info is null otherwise.
async fn model_info(State(state): State<AppState>, Query(q): Query<std::collections::HashMap<String, String>>) -> Response {
    let mid = q.get("id").map(String::as_str).unwrap_or("");
    if mid.is_empty() {
        return error(StatusCode::BAD_REQUEST, "id required");
    }
    let (code, inst) = device::call(&state, "GET", "/api/v1/models/?including_downloading=false", None, 15, None).await;
    if code != 200 {
        return reply((code, inst));
    }
    let Some(row) = inst.get("data").and_then(Value::as_array).into_iter().flatten().find(|m| m.get("id").and_then(Value::as_str) == Some(mid)) else {
        return error(StatusCode::NOT_FOUND, &format!("{mid} isn't installed on the device"));
    };
    let model_type = row.get("type").cloned().unwrap_or(Value::Null);
    let mut info = Value::Null;
    if let Some(route) = info_route(model_type.as_str().unwrap_or("")) {
        let (code, res) = device::call(&state, "GET", &format!("{route}?model={}", quote(mid)), None, 15, None).await;
        if code == 200 && res.is_object() {
            info = res.get("data").filter(|d| d.is_object()).cloned().unwrap_or(res);
        }
    }
    Json(json!({"id": mid, "type": model_type, "info": info})).into_response()
}

fn model_id(body: &Bytes) -> String {
    read_json(body).ok().map(|v| str_of(&v, "id").to_string()).unwrap_or_default()
}

/// Same call the TiinyOS app makes; it returns once the load is accepted.
async fn model_launch(State(state): State<AppState>, body: Bytes) -> Response {
    let mid = model_id(&body);
    if mid.is_empty() {
        return error(StatusCode::BAD_REQUEST, "id required");
    }
    let path = format!("/api/v1/models/{}/launch", quote(&mid));
    reply(device::call(&state, "POST", &path, Some(json!({"model_id": mid})), 120, None).await)
}

async fn model_stop(State(state): State<AppState>, body: Bytes) -> Response {
    let mid = model_id(&body);
    if mid.is_empty() {
        return error(StatusCode::BAD_REQUEST, "id required");
    }
    if state.jobs.active_on(&mid) {
        return error(StatusCode::CONFLICT, "That model has a job running; unload after it finishes.");
    }
    reply(device::call(&state, "POST", &format!("/api/v1/models/{}/stop", quote(&mid)), None, 120, None).await)
}

/// Same call the TiinyOS app's "unload all" makes.
async fn unload_all(State(state): State<AppState>) -> Response {
    if state.jobs_active() {
        return error(StatusCode::CONFLICT, "Jobs are still running or queued; unload after they finish.");
    }
    reply(device::call(&state, "POST", "/api/v1/models/unload_all", None, 180, None).await)
}

/// Same call the TiinyOS app's "Shut down" makes; the device answers and powers off seconds later.
async fn shutdown(State(state): State<AppState>) -> Response {
    if state.jobs_active() {
        return error(StatusCode::CONFLICT, "Jobs are still running or queued; power off after they finish.");
    }
    reply(device::call(&state, "POST", "/api/v1/sys/shutdown/", None, 20, None).await)
}

/// Text and pictures out of a document sent as the raw body; all in memory, nothing kept.
async fn extract_document(headers: axum::http::HeaderMap, body: Bytes) -> Response {
    let declared: u64 = headers.get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0);
    if declared > 100 << 20 || body.len() > 100 << 20 {
        return error(StatusCode::PAYLOAD_TOO_LARGE, "That file is over 100 MB.");
    }
    let raw = headers.get("x-filename").and_then(|v| v.to_str().ok()).unwrap_or("file");
    let name = percent_decode(raw);
    match crate::extract::extract(&name, &body) {
        Ok(parts) => Json(json!({"parts": parts})).into_response(),
        Err(e) => error(StatusCode::BAD_REQUEST, &e.0),
    }
}

/// urllib.parse.unquote: %XX sequences as UTF-8 (invalid bytes become U+FFFD).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(v) = hex {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Model icons from the device (no key needed), cached in data/icons.
async fn icon(State(state): State<AppState>, Path((org, name)): Path<(String, String)>) -> Response {
    if !safe_name(&org) || !safe_name(&name) {
        return not_found().await;
    }
    let dir = state.config.data_dir.join("icons");
    let _ = std::fs::create_dir_all(&dir);
    let cached = dir.join(format!("{org}__{name}.png"));
    if !cached.exists() {
        let url = format!("{}/api/v1/models/online_models/{org}/{name}/icon", state.settings.current().url);
        let bytes = match state.http.get(url).timeout(std::time::Duration::from_secs(10)).send().await {
            Ok(r) if r.status().is_success() => r.bytes().await.ok(),
            _ => None,
        };
        match bytes {
            Some(b) => {
                let _ = std::fs::write(&cached, &b);
            }
            None => return error(StatusCode::NOT_FOUND, "no icon"),
        }
    }
    match std::fs::read(&cached) {
        Ok(b) => ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "public, max-age=31536000, immutable")], b).into_response(),
        Err(_) => error(StatusCode::NOT_FOUND, "not found"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn app(password: &str) -> Router {
        crate::build_router(crate::Config {
            port: 0,
            host: "127.0.0.1".into(),
            password: password.into(),
            lan_access: false,
            app: false,
            data_dir: std::env::temp_dir().join(format!("tiiny-studio-test-{}", uuid::Uuid::new_v4().simple())),
        })
    }

    async fn send(app: Router, req: Request<Body>) -> (StatusCode, String, Vec<u8>) {
        let res = app.oneshot(req).await.unwrap();
        let ctype = res.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap().to_string()).unwrap_or_default();
        (res.status(), ctype, res.into_body().collect().await.unwrap().to_bytes().to_vec())
    }

    async fn get(path: &str) -> (StatusCode, String, Vec<u8>) {
        send(app(""), Request::get(path).body(Body::empty()).unwrap()).await
    }

    #[tokio::test]
    async fn serves_the_ui() {
        let (code, ctype, body) = get("/").await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(ctype, "text/html; charset=utf-8");
        assert!(String::from_utf8_lossy(&body).contains("<html"));
        let (code, ctype, _) = get("/static/shared.js").await;
        assert_eq!((code, ctype.as_str()), (StatusCode::OK, "text/javascript; charset=utf-8"));
        let (code, _, _) = get("/static/vendor/pdfjs/pdf.min.mjs").await;
        assert_eq!(code, StatusCode::OK);
    }

    #[tokio::test]
    async fn refuses_other_paths() {
        for p in ["/static/../server.py", "/static/vendor/../index.html", "/static/x/y.js", "/static/", "/nope"] {
            let (code, _, body) = get(p).await;
            assert_eq!(code, StatusCode::NOT_FOUND, "{p}");
            assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), json!({"error": {"message": "not found"}}));
        }
    }

    #[tokio::test]
    async fn password() {
        let (code, _, _) = send(app("pw"), Request::get("/").body(Body::empty()).unwrap()).await;
        assert_eq!(code, StatusCode::UNAUTHORIZED);
        // "any:pw" in base64
        let ok = Request::get("/").header(header::AUTHORIZATION, "Basic YW55OnB3").body(Body::empty()).unwrap();
        assert_eq!(send(app("pw"), ok).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn other_websites_are_refused() {
        let req = |m: &str, path: &str, host: &str, origin: Option<&str>| {
            let mut r = Request::builder().method(m).uri(path).header(header::HOST, host);
            if let Some(o) = origin {
                r = r.header(header::ORIGIN, o);
            }
            r.header(header::CONTENT_TYPE, "text/plain").body(Body::from("{}")).unwrap()
        };
        // a page elsewhere posting to the studio (CSRF), with and without a sandboxed "null" origin
        for origin in ["https://evil.example", "null", "http://localhost:9999"] {
            let (code, _, _) = send(app(""), req("POST", "/api/privacy/wipe", "localhost:7860", Some(origin))).await;
            assert_eq!(code, StatusCode::FORBIDDEN, "{origin}");
        }
        // DNS rebinding: someone else's name pointing at 127.0.0.1
        let (code, _, _) = send(app(""), req("GET", "/api/history", "evil.example:7860", None)).await;
        assert_eq!(code, StatusCode::FORBIDDEN);
        // the studio's own page, and tools without a browser (no Origin)
        for host in ["localhost:7860", "127.0.0.1:7860", "[::1]:7860", "LOCALHOST"] {
            assert_eq!(send(app(""), req("GET", "/", host, None)).await.0, StatusCode::OK, "{host}");
        }
        let own = req("POST", "/api/privacy/wipe", "127.0.0.1:7860", Some("http://127.0.0.1:7860"));
        assert_eq!(send(app(""), own).await.0, StatusCode::OK);
        assert_eq!(send(app(""), req("POST", "/api/privacy/wipe", "localhost:7860", None)).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn unusable_bodies_are_a_400_and_huge_ones_a_413() {
        let post = |path: &str, body: &'static str, len: Option<usize>| {
            let mut r = Request::post(path).header(header::CONTENT_TYPE, "application/json");
            if let Some(n) = len {
                r = r.header(header::CONTENT_LENGTH, n);
            }
            r.body(Body::from(body)).unwrap()
        };
        let message = |body: Vec<u8>| serde_json::from_slice::<Value>(&body).unwrap()["error"]["message"].as_str().unwrap().to_string();
        for path in ["/api/history/delete", "/api/export", "/api/search", "/api/history/abc/fav"] {
            let (code, _, body) = send(app(""), post(path, "not json", None)).await;
            assert_eq!((code, message(body).as_str()), (StatusCode::BAD_REQUEST, "The request body isn't valid JSON."), "{path}");
            let (code, _, body) = send(app(""), post(path, "[1]", None)).await;
            assert_eq!((code, message(body).as_str()), (StatusCode::BAD_REQUEST, "The request body must be a JSON object."), "{path}");
        }
        let (code, _, body) = send(app(""), post("/api/history/delete", "{}", Some(MAX_BODY + 1))).await;
        assert_eq!((code, message(body).as_str()), (StatusCode::PAYLOAD_TOO_LARGE, TOO_LARGE));
    }

    #[test]
    fn lan_access_answers_any_name_but_not_other_sites() {
        use axum::http::{HeaderMap, Method};
        let h = |host: &str, origin: Option<&str>| {
            let mut m = HeaderMap::new();
            m.insert(header::HOST, host.parse().unwrap());
            if let Some(o) = origin {
                m.insert(header::ORIGIN, o.parse().unwrap());
            }
            m
        };
        assert_eq!(foreign(&Method::GET, &h("nas.lan:7860", None), true), None);
        assert_eq!(foreign(&Method::POST, &h("192.168.1.50:7860", Some("http://192.168.1.50:7860")), true), None);
        assert!(foreign(&Method::POST, &h("192.168.1.50:7860", Some("https://evil.example")), true).is_some());
        assert!(foreign(&Method::DELETE, &h("nas.lan:7860", Some("http://nas.lan:7861")), true).is_some());
    }

    #[tokio::test]
    async fn unconfigured() {
        let (code, _, body) = get("/api/models").await;
        assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(serde_json::from_slice::<Value>(&body).unwrap()["error"]["message"], json!(device::NOT_CONFIGURED));
        let (code, _, _) = get("/api/model/plan").await;
        assert_eq!(code, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn quoting() {
        assert_eq!(quote("Qwen/Qwen3-8B v2"), "Qwen%2FQwen3-8B%20v2");
    }
}
