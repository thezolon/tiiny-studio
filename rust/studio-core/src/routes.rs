//! Every route the UI calls, with the same paths, status codes and JSON as server.py's Handler.

use axum::{
    body::Bytes,
    extract::{Path, Query, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{any, get, post},
    Json, Router,
};
use rust_embed::RustEmbed;
use serde_json::{json, Value};
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::device::{self, quote};
use crate::settings::{device_url, hostname, normalize_url, same_origin};
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

/// What the access middleware needs besides the app state.
struct Guards {
    /// STUDIO_HOSTS: extra names answered when the studio is open to the network
    hosts: Vec<String>,
    password: AuthGuard,
}

type GuardState = (AppState, Arc<Guards>);

pub fn router(state: AppState) -> Router {
    let guards = Arc::new(Guards {
        hosts: allowed_hosts_from_env(),
        password: AuthGuard::new(),
    });
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
        .route("/api/update", get(update))
        .route("/api/model/plan", get(model_plan))
        .route("/api/model/info", get(model_info))
        .route("/api/model/launch", post(model_launch))
        .route("/api/model/stop", post(model_stop))
        .route("/api/model/unload_all", post(unload_all))
        .route("/api/store", get(model_store))
        .route("/api/downloads", get(downloads))
        .route("/api/model/{action}", post(model_change))
        .route("/api/device/shutdown", post(shutdown))
        .route("/api/icon/{org}/{name}", get(icon))
        .route(
            "/api/extract",
            post(extract_document).layer(axum::extract::DefaultBodyLimit::max(MAX_BODY)),
        )
        .route(
            "/mcp",
            any(crate::mcp::endpoint).layer(axum::extract::DefaultBodyLimit::max(MCP_BODY)),
        )
        .merge(crate::library::routes())
        .fallback(not_found)
        // most routes only ever get small JSON; the few that take uploads set their own, larger limit (body_limit_for)
        .layer(axum::extract::DefaultBodyLimit::max(SMALL_BODY))
        .layer(middleware::from_fn(body_limit))
        .layer(middleware::from_fn_with_state(
            (state.clone(), guards.clone()),
            authorized,
        ))
        .layer(middleware::from_fn_with_state(
            (state.clone(), guards),
            same_site,
        ))
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

/// What the page may load and do. The UI is static files of its own origin (no CDN, no inline scripts); styles may
/// be inline (style="" attributes), pictures/audio/video may be blob: or data: (results, previews, recordings), the
/// PDF.js worker and model-viewer use blob: workers and WebAssembly. Nothing may frame the studio.
const CSP: &str = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; \
img-src 'self' data: blob:; media-src 'self' data: blob:; font-src 'self' data:; connect-src 'self' data: blob:; \
worker-src 'self' blob:; object-src 'none'; frame-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";

/// Anti-framing, no content sniffing and a CSP on every answer.
async fn security_headers(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(
        header::X_FRAME_OPTIONS,
        header::HeaderValue::from_static("DENY"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        header::HeaderValue::from_static(CSP),
    );
    h.insert(
        header::REFERRER_POLICY,
        header::HeaderValue::from_static("no-referrer"),
    );
    res
}

/// STUDIO_HOSTS: extra names the studio answers to when it is shared on a network, comma separated (the name a
/// reverse proxy forwards, e.g. `studio.example.org`).
fn allowed_hosts_from_env() -> Vec<String> {
    parse_hosts(&std::env::var("STUDIO_HOSTS").unwrap_or_default())
}

fn parse_hosts(list: &str) -> Vec<String> {
    list.split([',', ' ', ';'])
        .map(|h| host_name(&h.trim().to_ascii_lowercase()).to_string())
        .filter(|h| !h.is_empty())
        .collect()
}

/// The host name of a Host header value ("nas.lan:7860" -> "nas.lan", "[::1]:7860" -> "::1").
fn host_name(h: &str) -> &str {
    match h.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None if h.matches(':').count() > 1 => h, // a bare IPv6 address
        None => h.rsplit_once(':').map_or(h, |(n, _)| n),
    }
}

/// Which names a studio opened to a network answers to. A page that re-points its own public name at the studio
/// (DNS rebinding) sends that name as Host, so a public domain name is refused unless STUDIO_HOSTS lists it. Fine
/// without listing: IP addresses (an attacker can't make a browser send another site's name with one), localhost,
/// names with no dot (the computer's own name, as in `http://nas:7860`) and the private-network suffixes.
fn host_allowed(name: &str, extra: &[String]) -> bool {
    let name = name.trim_end_matches('.');
    name.parse::<std::net::IpAddr>().is_ok()
        || !name.contains('.')
        || [".local", ".lan", ".home.arpa", ".localdomain", ".internal"]
            .iter()
            .any(|s| name.ends_with(s))
        || extra.iter().any(|h| h == name)
}

/// Keeps other websites out. Without this, any page open in the browser could POST here (wipe everything, power
/// off the Tiiny) or, by pointing its own domain at the studio (DNS rebinding), read the gallery. So: unless the
/// studio is opened to the LAN, only localhost addresses are answered; opened to the LAN, only names that
/// `host_allowed` accepts; and a change (anything but GET/HEAD) from a browser must come from the studio's own page.
async fn same_site(
    State((state, guards)): State<GuardState>,
    req: Request,
    next: Next,
) -> Response {
    match foreign(
        req.method(),
        req.headers(),
        state.config.lan_access,
        &guards.hosts,
    ) {
        Some(why) => error(StatusCode::FORBIDDEN, why),
        None => next.run(req).await,
    }
}

fn foreign(
    method: &axum::http::Method,
    headers: &axum::http::HeaderMap,
    lan_access: bool,
    extra_hosts: &[String],
) -> Option<&'static str> {
    let text = |h| {
        headers
            .get(h)
            .and_then(|v: &header::HeaderValue| v.to_str().ok())
            .map(str::to_ascii_lowercase)
    };
    let host = text(header::HOST);
    if let Some(h) = &host {
        let name = host_name(h);
        if !lan_access {
            if !matches!(name, "localhost" | "127.0.0.1" | "::1") {
                return Some("This studio only answers at localhost.");
            }
        } else if !host_allowed(name, extra_hosts) {
            return Some(
                "This studio doesn't answer to that name. Add it to STUDIO_HOSTS, or open the studio by its IP address.",
            );
        }
    }
    if matches!(*method, axum::http::Method::GET | axum::http::Method::HEAD) {
        return None;
    }
    // browsers send Origin with every cross-site POST/DELETE (and "null" from sandboxed or file pages)
    let origin = text(header::ORIGIN)?;
    let authority = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"));
    if authority.is_some() && authority == host.as_deref() {
        return None;
    }
    Some("Requests from other websites aren't allowed.")
}

/// Optional password (STUDIO_PASSWORD): HTTP basic auth, any username, on every request.
async fn authorized(
    State((state, guards)): State<GuardState>,
    req: Request,
    next: Next,
) -> Response {
    // the MCP endpoint checks its own bearer token (agents don't have the studio password); same_site still applies
    if req.uri().path() == "/mcp" {
        return next.run(req).await;
    }
    let password = &state.config.password;
    let ip = client_ip(req.extensions());
    if password.is_empty() {
        return next.run(req).await;
    }
    // after a run of wrong passwords every attempt waits, the right one included, so guessing doesn't pay
    if let Some(wait) = guards.password.wait(ip) {
        return too_many_attempts(wait);
    }
    let sent = req.headers().get(header::AUTHORIZATION).is_some();
    let given = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|b| base64_decode(b.trim()))
        .and_then(|raw| String::from_utf8(raw).ok())
        .map(|s| {
            s.split_once(':')
                .map(|(_, p)| p.to_string())
                .unwrap_or_default()
        });
    if given.is_some_and(|g| constant_time_eq(g.as_bytes(), password.as_bytes())) {
        guards.password.succeeded(ip);
        return next.run(req).await;
    }
    if sent {
        // a browser's first request has no credentials yet (that is the prompt, not a guess)
        guards.password.failed(ip, "password");
    }
    (
        StatusCode::UNAUTHORIZED,
        [(
            header::WWW_AUTHENTICATE,
            r#"Basic realm="Tiiny Studio", charset="UTF-8""#,
        )],
    )
        .into_response()
}

/// Failed password / MCP-token attempts, counted per client address (so one machine on the LAN guessing can't
/// lock the real user out). The first `FREE_FAILURES` wrong tries of an address cost nothing; after that, its
/// attempts are refused for a time that doubles with each further failure (1 s, 2 s, 4 s ... up to a minute), and
/// a good attempt, or ten quiet minutes, starts over. At most `MAX_CLIENTS` addresses are remembered (the one
/// quiet longest is dropped first). Behind a reverse proxy every client has the proxy's address, so they share
/// one count there.
pub(crate) struct AuthGuard(Mutex<Vec<(IpAddr, u32, Instant)>>);

const FREE_FAILURES: u32 = 10;
const MAX_LOCKOUT_SECS: u64 = 60;
const FORGET_AFTER: Duration = Duration::from_secs(600);
const MAX_CLIENTS: usize = 256;

pub(crate) static MCP_GUARD: AuthGuard = AuthGuard::new();

/// Who is asking: the connection's address (the unspecified address when the server wasn't given it, as in tests).
pub(crate) fn client_ip(ext: &axum::http::Extensions) -> IpAddr {
    ext.get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|c| c.0.ip().to_canonical())
        .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED))
}

impl AuthGuard {
    pub(crate) const fn new() -> Self {
        AuthGuard(Mutex::new(Vec::new()))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<(IpAddr, u32, Instant)>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Seconds an attempt from `ip` has to wait now, if it has to.
    pub(crate) fn wait(&self, ip: IpAddr) -> Option<u64> {
        self.wait_at(ip, Instant::now())
    }

    fn wait_at(&self, ip: IpAddr, now: Instant) -> Option<u64> {
        let (_, n, last) = *self.lock().iter().find(|(a, _, _)| *a == ip)?;
        if n < FREE_FAILURES {
            return None;
        }
        let lockout =
            Duration::from_secs((1u64 << (n - FREE_FAILURES).min(6)).min(MAX_LOCKOUT_SECS));
        lockout
            .checked_sub(now.saturating_duration_since(last))
            .map(|left| left.as_secs() + 1)
    }

    pub(crate) fn failed(&self, ip: IpAddr, what: &str) {
        self.failed_at(ip, Instant::now(), what);
    }

    fn failed_at(&self, ip: IpAddr, now: Instant, what: &str) {
        let mut g = self.lock();
        let i = match g.iter().position(|(a, _, _)| *a == ip) {
            Some(i) => i,
            None => {
                if g.len() >= MAX_CLIENTS {
                    // drop the address that has been quiet the longest
                    if let Some(old) = (0..g.len()).min_by_key(|&k| g[k].2) {
                        g.swap_remove(old);
                    }
                }
                g.push((ip, 0, now));
                g.len() - 1
            }
        };
        let e = &mut g[i];
        if now.saturating_duration_since(e.2) > FORGET_AFTER {
            e.1 = 0;
        }
        e.1 = e.1.saturating_add(1);
        e.2 = now;
        if e.1 == FREE_FAILURES {
            eprintln!(
                "Too many wrong {what} attempts from {ip}: its further attempts are slowed down."
            );
        }
    }

    pub(crate) fn succeeded(&self, ip: IpAddr) {
        self.lock().retain(|(a, _, _)| *a != ip);
    }
}

pub(crate) fn too_many_attempts(wait: u64) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::RETRY_AFTER, wait.to_string())],
        Json(
            json!({"error": {"message": "Too many wrong attempts. Wait a moment and try again."}}),
        ),
    )
        .into_response()
}

pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
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
    (
        StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_GATEWAY),
        Json(body),
    )
        .into_response()
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
pub(crate) const MAX_BODY: usize = 150 << 20;
const TOO_LARGE: &str = "That request is too large (over 150 MB).";
/// Settings, history, search, export, model and device calls: a few KB in practice.
const SMALL_BODY: usize = 2 << 20;
/// A saved chat can carry its attached pictures.
pub(crate) const CHAT_BODY: usize = 32 << 20;
/// MCP calls (an agent may pass a picture or clip as base64).
const MCP_BODY: usize = 32 << 20;

/// The body size a route accepts; the routes that take uploads or chats set the same number with a layer.
fn body_limit_for(method: &axum::http::Method, path: &str) -> usize {
    if *method != axum::http::Method::POST && *method != axum::http::Method::PUT {
        return SMALL_BODY;
    }
    match path {
        "/api/jobs" | "/api/extract" | "/api/voices" => MAX_BODY,
        "/mcp" => MCP_BODY,
        p if p.starts_with("/api/chats/") && !p.ends_with("/delete") => CHAT_BODY,
        _ => SMALL_BODY,
    }
}

async fn body_limit(req: Request, next: Next) -> Response {
    let len = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<usize>().ok());
    if len.is_some_and(|n| n > MAX_BODY) {
        return error(StatusCode::PAYLOAD_TOO_LARGE, TOO_LARGE);
    }
    if len.is_some_and(|n| n > body_limit_for(req.method(), req.uri().path())) {
        return error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "That request is too large for this part of the studio.",
        );
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
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '-')
}

async fn static_file(Path(path): Path<String>) -> Response {
    if !allowed_static(&path) {
        return not_found().await;
    }
    embedded(&path, mime(&path))
}

async fn doc(Path(name): Path<String>) -> Response {
    let ok = name.ends_with(".md")
        && name[..name.len() - 3]
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-');
    match Docs::get(&name).filter(|_| ok && !name.starts_with(".md")) {
        Some(f) => (
            [
                (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            f.data.into_owned(),
        )
            .into_response(),
        None => error(StatusCode::NOT_FOUND, "not found"),
    }
}

fn embedded(path: &str, ctype: &str) -> Response {
    match Static::get(path) {
        Some(file) => (
            [
                (header::CONTENT_TYPE, ctype),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            file.data.into_owned(),
        )
            .into_response(),
        None => error(StatusCode::NOT_FOUND, "not found"),
    }
}

/// server.py MIME, by extension; anything else is application/octet-stream.
pub fn mime(path: &str) -> &'static str {
    match path
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
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
    let mut v = without_mcp_token(state.settings.public());
    v["can_open_folder"] = json!(can_open_folder(&req));
    Json(v)
}

/// The MCP token goes only to the call that asks for it (a POST to /api/settings/mcp, which the page makes when the
/// user turns MCP on, asks for a new token or presses "Show token"): not in every settings answer.
fn without_mcp_token(mut v: Value) -> Value {
    if let Some(m) = v.get_mut("mcp").and_then(Value::as_object_mut) {
        let set = m.remove("token").is_some_and(|t| t.as_str() != Some(""));
        m.insert("token_set".into(), json!(set));
    }
    v
}

/// Opening the results folder only makes sense on the computer running the studio, with a desktop:
/// not in Docker, not for a phone or another PC on the LAN.
fn can_open_folder(req: &Request) -> bool {
    let local = req
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .is_some_and(|c| c.0.ip().is_loopback() || c.0.ip().to_canonical().is_loopback());
    let desktop = cfg!(any(windows, target_os = "macos"))
        || std::env::var_os("DISPLAY").is_some()
        || std::env::var_os("WAYLAND_DISPLAY").is_some();
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
            Some(b) => (
                [
                    (header::CONTENT_TYPE, "image/png"),
                    (header::CACHE_CONTROL, "no-store"),
                ],
                b,
            )
                .into_response(),
            None => error(
                StatusCode::NOT_FOUND,
                "There's no picture on the clipboard.",
            ),
        };
    }
    #[allow(unreachable_code)]
    not_found().await
}

/// Show the results folder in the system file manager (Explorer, Finder, or the Linux desktop's). With `{"id": …}`,
/// that result's own file is shown selected (#58): only a file the entry owns, found by name, never a path from the request.
async fn open_folder(State(state): State<AppState>, req: Request) -> Response {
    if !can_open_folder(&req) {
        return error(
            StatusCode::FORBIDDEN,
            "The folder can only be opened on the computer running the studio.",
        );
    }
    let body = axum::body::to_bytes(req.into_body(), 4096)
        .await
        .unwrap_or_default();
    let id = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|v| v.get("id").and_then(Value::as_str).map(String::from));
    let dir = state.store.images.clone();
    let file = match id {
        None => None,
        Some(id) => {
            let entry = state
                .store
                .load_history()
                .into_iter()
                .find(|e| e.get("id").and_then(Value::as_str) == Some(id.as_str()));
            let Some(name) = entry
                .as_ref()
                .and_then(|e| crate::store::entry_files(e).into_iter().next())
                .filter(|n| crate::store::safe_file(n))
            else {
                return error(StatusCode::NOT_FOUND, "That result isn't in the library.");
            };
            let path = dir.join(name);
            if !path.is_file() {
                return error(
                    StatusCode::NOT_FOUND,
                    "That result's file is gone from the folder.",
                );
            }
            Some(path)
        }
    };
    let quiet = |mut c: std::process::Command| {
        c.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    };
    match &file {
        // Explorer wants `/select,"path"` as one raw argument (Rust's own quoting would wrap the comma too)
        #[cfg(windows)]
        Some(f) => {
            use std::os::windows::process::CommandExt;
            let mut c = std::process::Command::new("explorer");
            c.raw_arg(format!("/select,\"{}\"", f.display()));
            let _ = quiet(c);
        }
        #[cfg(target_os = "macos")]
        Some(f) => {
            let mut c = std::process::Command::new("open");
            c.arg("-R").arg(f);
            let _ = quiet(c);
        }
        // Linux desktops (Files, Dolphin, …) select a file through the freedesktop FileManager1 D-Bus call
        #[cfg(not(any(windows, target_os = "macos")))]
        Some(f) => {
            let mut c = std::process::Command::new("dbus-send");
            c.args([
                "--session",
                "--dest=org.freedesktop.FileManager1",
                "--type=method_call",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
            ])
            .arg(format!("array:string:{}", file_uri(f)))
            .arg("string:");
            let shown = c
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            if !shown {
                let mut c = std::process::Command::new("xdg-open");
                c.arg(&dir);
                let _ = quiet(c);
            }
        }
        None => {
            let program = if cfg!(windows) {
                "explorer"
            } else if cfg!(target_os = "macos") {
                "open"
            } else {
                "xdg-open"
            };
            let mut c = std::process::Command::new(program);
            c.arg(&dir); // a fixed folder of ours, never a path from the request
            let _ = quiet(c);
        }
    }
    Json(json!({"ok": true})).into_response()
}

/// `file://` URI for a local path, percent-encoding everything but unreserved characters and slashes.
#[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
fn file_uri(p: &std::path::Path) -> String {
    let mut out = String::from("file://");
    for b in p.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Add or update a saved device ({id?, name, url, key}); without an id on first run, the active one.
async fn save_settings(State(state): State<AppState>, body: Bytes) -> Response {
    let req = match read_json(&body) {
        Ok(v) => v,
        Err(e) => return error(StatusCode::BAD_REQUEST, &e),
    };
    let new = req.get("new").is_some_and(device::truthy);
    state.settings.take_save_error();
    let did = Some(str_of(&req, "id"))
        .filter(|s| !s.is_empty())
        .map(String::from)
        .or_else(|| if new { None } else { state.settings.active() });
    if let Err(e) = state.settings.upsert(
        did.as_deref(),
        str_of(&req, "name"),
        str_of(&req, "url"),
        str_of(&req, "key").trim(),
    ) {
        return error(StatusCode::BAD_REQUEST, &e);
    }
    if let Some(e) = state.settings.take_save_error() {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Couldn't save your changes: {e}"),
        );
    }
    let check = state.clone();
    tokio::spawn(async move { check.settings.check(&check.http).await });
    Json(without_mcp_token(state.settings.public())).into_response()
}

async fn device_action(
    State(state): State<AppState>,
    Path(action): Path<String>,
    body: Bytes,
) -> Response {
    let req = read_json(&body).unwrap_or(json!({}));
    let did = str_of(&req, "id");
    state.settings.take_save_error();
    match action.as_str() {
        "delete" => state.settings.remove(did),
        "move" => state.settings.move_device(
            did,
            if req.get("dir").and_then(Value::as_f64).unwrap_or(-1.0) < 0.0 {
                -1
            } else {
                1
            },
        ),
        "activate" => state.settings.activate(did, None),
        "auto" => state
            .settings
            .set_auto(req.get("auto").map(device::truthy).unwrap_or(true)),
        "swap" => state.settings.set_swap(&req),
        "mcp" => state.settings.set_mcp(&req),
        "updates" => {
            state
                .settings
                .set_update_check(req.get("update_check").map(device::truthy).unwrap_or(false));
            state.update_cache.lock().await.take(); // a fresh answer the next time it's on
        }
        "tags" => state
            .settings
            .set_tag_prompts(req.get("tag_prompts").map(device::truthy).unwrap_or(true)),
        "check" => state.settings.check(&state.http).await,
        _ => return not_found().await,
    }
    if let Some(e) = state.settings.take_save_error() {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Couldn't save your changes: {e}"),
        );
    }
    let v = state.settings.public();
    Json(if action == "mcp" {
        v
    } else {
        without_mcp_token(v)
    })
    .into_response()
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
    let url = Some(normalize_url(str_of(&req, "url")))
        .filter(|u| !u.is_empty())
        .unwrap_or(saved_url.clone());
    if url.is_empty() {
        return Json(json!({"ok": false, "message": "Enter the device address."}));
    }
    let url = match device_url(&url) {
        Ok(u) => u,
        Err(message) => return Json(json!({"ok": false, "message": message})),
    };
    let mut key = str_of(&req, "key").trim().to_string();
    if key.is_empty() && same_origin(&url, &saved_url) {
        key = saved_key; // a saved key is only ever tried against its own origin (scheme, host and port)
    }
    let (code, res) = device::call(&state, "GET", "/v1/models", None, 8, Some((&url, &key))).await;
    if code == 200 {
        // any OpenAI-style relay answers /v1/models; only a Tiiny has its model controls
        let (tiiny, _) = device::call(
            &state,
            "GET",
            "/api/v1/models/running",
            None,
            8,
            Some((&url, &key)),
        )
        .await;
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
        let list = if names.is_empty() {
            ".".to_string()
        } else {
            format!(
                ": {}",
                names.iter().take(4).cloned().collect::<Vec<_>>().join(", ")
            )
        };
        return Json(
            json!({"ok": true, "message": format!("Connected. {} model(s) loaded{list}", names.len()), "loaded": names}),
        );
    }
    if code == 401 || code == 403 {
        let msg = if key.is_empty() {
            "The device needs an API key."
        } else {
            "The device rejected the API key. Copy the API key (not the device ID) from TiinyOS: avatar → Settings → API Key."
        };
        return Json(json!({"ok": false, "message": msg}));
    }
    if crate::settings::in_docker() && hostname(&url).starts_with("172.17.") {
        return Json(json!({"ok": false, "message": DOCKER_USB}));
    }
    if device::is_locked(code, &res) {
        return Json(json!({"ok": false, "message": device::LOCKED}));
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

async fn update(State(state): State<AppState>) -> Json<Value> {
    Json(crate::update::check(&state).await)
}

async fn model_plan(
    State(state): State<AppState>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let mid = q.get("id").map(String::as_str).unwrap_or("");
    if mid.is_empty() {
        return error(StatusCode::BAD_REQUEST, "id required");
    }
    reply(
        device::call(
            &state,
            "GET",
            &format!("/api/v1/models/{}/unload_candidate", quote(mid)),
            None,
            30,
            None,
        )
        .await,
    )
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
async fn model_info(
    State(state): State<AppState>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let mid = q.get("id").map(String::as_str).unwrap_or("");
    if mid.is_empty() {
        return error(StatusCode::BAD_REQUEST, "id required");
    }
    let (code, inst) = device::call(
        &state,
        "GET",
        "/api/v1/models/?including_downloading=false",
        None,
        15,
        None,
    )
    .await;
    if code != 200 {
        return reply((code, inst));
    }
    let Some(row) = inst
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|m| m.get("id").and_then(Value::as_str) == Some(mid))
    else {
        return error(
            StatusCode::NOT_FOUND,
            &format!("{mid} isn't installed on the device"),
        );
    };
    let model_type = row.get("type").cloned().unwrap_or(Value::Null);
    let mut info = Value::Null;
    if let Some(route) = info_route(model_type.as_str().unwrap_or("")) {
        let (code, res) = device::call(
            &state,
            "GET",
            &format!("{route}?model={}", quote(mid)),
            None,
            15,
            None,
        )
        .await;
        if code == 200 && res.is_object() {
            info = res
                .get("data")
                .filter(|d| d.is_object())
                .cloned()
                .unwrap_or(res);
        }
    }
    Json(json!({"id": mid, "type": model_type, "info": info})).into_response()
}

fn model_id(body: &Bytes) -> String {
    read_json(body)
        .ok()
        .map(|v| str_of(&v, "id").to_string())
        .unwrap_or_default()
}

/// Same call the TiinyOS app makes; it returns once the load is accepted.
async fn model_launch(State(state): State<AppState>, body: Bytes) -> Response {
    let mid = model_id(&body);
    if mid.is_empty() {
        return error(StatusCode::BAD_REQUEST, "id required");
    }
    let path = format!("/api/v1/models/{}/launch", quote(&mid));
    reply(
        device::call(
            &state,
            "POST",
            &path,
            Some(json!({"model_id": mid})),
            120,
            None,
        )
        .await,
    )
}

async fn model_stop(State(state): State<AppState>, body: Bytes) -> Response {
    let mid = model_id(&body);
    if mid.is_empty() {
        return error(StatusCode::BAD_REQUEST, "id required");
    }
    if state.jobs.active_on(&mid) {
        return error(
            StatusCode::CONFLICT,
            "That model has a job running; unload after it finishes.",
        );
    }
    reply(
        device::call(
            &state,
            "POST",
            &format!("/api/v1/models/{}/stop", quote(&mid)),
            None,
            120,
            None,
        )
        .await,
    )
}

async fn model_store(State(state): State<AppState>) -> Response {
    reply(crate::downloads::model_store(&state).await)
}

async fn downloads(State(state): State<AppState>) -> Response {
    reply(crate::downloads::downloads(&state).await)
}

/// download, pause, cancel, delete, update (the fixed /api/model/* routes above take precedence)
async fn model_change(
    State(state): State<AppState>,
    Path(action): Path<String>,
    body: Bytes,
) -> Response {
    let queue = matches!(action.as_str(), "queue" | "dequeue" | "resume" | "dismiss");
    if !queue
        && !matches!(
            action.as_str(),
            "download" | "pause" | "cancel" | "delete" | "update"
        )
    {
        return not_found().await;
    }
    match read_json(&body) {
        // in its own task: a closed browser tab must not stop an update between its delete and its download
        Ok(b) => {
            let task = tokio::spawn(async move {
                if queue {
                    crate::downloads::queue_change(&state, &action, &b).await
                } else {
                    crate::downloads::model_change(&state, &action, &b).await
                }
            });
            match task.await {
                Ok(answer) => reply(answer),
                Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "The change failed."),
            }
        }
        Err(e) => error(StatusCode::BAD_REQUEST, &e),
    }
}

/// Same call the TiinyOS app's "unload all" makes.
async fn unload_all(State(state): State<AppState>) -> Response {
    if state.jobs_active() {
        return error(
            StatusCode::CONFLICT,
            "Jobs are still running or queued; unload after they finish.",
        );
    }
    reply(device::call(&state, "POST", "/api/v1/models/unload_all", None, 180, None).await)
}

/// Same call the TiinyOS app's "Shut down" makes; the device answers and powers off seconds later.
async fn shutdown(State(state): State<AppState>) -> Response {
    if state.jobs_active() {
        return error(
            StatusCode::CONFLICT,
            "Jobs are still running or queued; power off after they finish.",
        );
    }
    reply(device::call(&state, "POST", "/api/v1/sys/shutdown/", None, 20, None).await)
}

/// Text and pictures out of a document sent as the raw body; all in memory, nothing kept.
async fn extract_document(headers: axum::http::HeaderMap, body: Bytes) -> Response {
    let declared: u64 = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if declared > 100 << 20 || body.len() > 100 << 20 {
        return error(StatusCode::PAYLOAD_TOO_LARGE, "That file is over 100 MB.");
    }
    let raw = headers
        .get("x-filename")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("file");
    let name = percent_decode(raw);
    match crate::extract::extract_isolated(name, body).await {
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
            let hex = std::str::from_utf8(&b[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok());
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

/// "device url org/name" the device answered 404 for, until the studio restarts.
static NO_ICON: std::sync::Mutex<std::collections::BTreeSet<String>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

/// Model icons from the device (no key needed), cached in data/icons.
async fn icon(
    State(state): State<AppState>,
    Path((org, name)): Path<(String, String)>,
) -> Response {
    if !safe_name(&org) || !safe_name(&name) {
        return not_found().await;
    }
    let dir = state.config.data_dir.join("icons");
    let _ = std::fs::create_dir_all(&dir);
    let cached = dir.join(format!("{org}__{name}.png"));
    if !cached.exists() {
        let base = state.settings.current().url;
        let key = format!("{base} {org}/{name}");
        if NO_ICON.lock().unwrap().contains(&key) {
            return error(StatusCode::NOT_FOUND, "no icon");
        }
        let url = format!("{base}/api/v1/models/online_models/{org}/{name}/icon");
        let bytes = match state
            .http
            .get(url)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r.bytes().await.ok(),
            Ok(r) if r.status() == StatusCode::NOT_FOUND => {
                // the device has none: don't ask again on every redraw
                NO_ICON.lock().unwrap().insert(key);
                None
            }
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
        Ok(b) => (
            [
                (header::CONTENT_TYPE, "image/png"),
                (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            ],
            b,
        )
            .into_response(),
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
            data_dir: std::env::temp_dir().join(format!(
                "tiiny-studio-test-{}",
                uuid::Uuid::new_v4().simple()
            )),
        })
    }

    async fn send(app: Router, req: Request<Body>) -> (StatusCode, String, Vec<u8>) {
        let res = app.oneshot(req).await.unwrap();
        let ctype = res
            .headers()
            .get(header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default();
        (
            res.status(),
            ctype,
            res.into_body().collect().await.unwrap().to_bytes().to_vec(),
        )
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
        assert_eq!(
            (code, ctype.as_str()),
            (StatusCode::OK, "text/javascript; charset=utf-8")
        );
        let (code, _, _) = get("/static/vendor/pdfjs/pdf.min.mjs").await;
        assert_eq!(code, StatusCode::OK);
    }

    #[tokio::test]
    async fn refuses_other_paths() {
        for p in [
            "/static/../server.py",
            "/static/vendor/../index.html",
            "/static/x/y.js",
            "/static/",
            "/nope",
        ] {
            let (code, _, body) = get(p).await;
            assert_eq!(code, StatusCode::NOT_FOUND, "{p}");
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap(),
                json!({"error": {"message": "not found"}})
            );
        }
    }

    #[tokio::test]
    async fn password() {
        let (code, _, _) = send(app("pw"), Request::get("/").body(Body::empty()).unwrap()).await;
        assert_eq!(code, StatusCode::UNAUTHORIZED);
        // "any:pw" in base64
        let ok = Request::get("/")
            .header(header::AUTHORIZATION, "Basic YW55OnB3")
            .body(Body::empty())
            .unwrap();
        assert_eq!(send(app("pw"), ok).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn other_websites_are_refused() {
        let req = |m: &str, path: &str, host: &str, origin: Option<&str>| {
            let mut r = Request::builder()
                .method(m)
                .uri(path)
                .header(header::HOST, host);
            if let Some(o) = origin {
                r = r.header(header::ORIGIN, o);
            }
            r.header(header::CONTENT_TYPE, "text/plain")
                .body(Body::from("{}"))
                .unwrap()
        };
        // a page elsewhere posting to the studio (CSRF), with and without a sandboxed "null" origin
        for origin in ["https://evil.example", "null", "http://localhost:9999"] {
            let (code, _, _) = send(
                app(""),
                req("POST", "/api/privacy/wipe", "localhost:7860", Some(origin)),
            )
            .await;
            assert_eq!(code, StatusCode::FORBIDDEN, "{origin}");
        }
        // DNS rebinding: someone else's name pointing at 127.0.0.1
        let (code, _, _) = send(
            app(""),
            req("GET", "/api/history", "evil.example:7860", None),
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN);
        // the studio's own page, and tools without a browser (no Origin)
        for host in [
            "localhost:7860",
            "127.0.0.1:7860",
            "[::1]:7860",
            "LOCALHOST",
        ] {
            assert_eq!(
                send(app(""), req("GET", "/", host, None)).await.0,
                StatusCode::OK,
                "{host}"
            );
        }
        let own = req(
            "POST",
            "/api/privacy/wipe",
            "127.0.0.1:7860",
            Some("http://127.0.0.1:7860"),
        );
        assert_eq!(send(app(""), own).await.0, StatusCode::OK);
        assert_eq!(
            send(
                app(""),
                req("POST", "/api/privacy/wipe", "localhost:7860", None)
            )
            .await
            .0,
            StatusCode::OK
        );
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
        let message = |body: Vec<u8>| {
            serde_json::from_slice::<Value>(&body).unwrap()["error"]["message"]
                .as_str()
                .unwrap()
                .to_string()
        };
        for path in [
            "/api/history/delete",
            "/api/export",
            "/api/search",
            "/api/history/abc/fav",
        ] {
            let (code, _, body) = send(app(""), post(path, "not json", None)).await;
            assert_eq!(
                (code, message(body).as_str()),
                (
                    StatusCode::BAD_REQUEST,
                    "The request body isn't valid JSON."
                ),
                "{path}"
            );
            let (code, _, body) = send(app(""), post(path, "[1]", None)).await;
            assert_eq!(
                (code, message(body).as_str()),
                (
                    StatusCode::BAD_REQUEST,
                    "The request body must be a JSON object."
                ),
                "{path}"
            );
        }
        let (code, _, body) = send(
            app(""),
            post("/api/history/delete", "{}", Some(MAX_BODY + 1)),
        )
        .await;
        assert_eq!(
            (code, message(body).as_str()),
            (StatusCode::PAYLOAD_TOO_LARGE, TOO_LARGE)
        );
    }

    fn host_headers(host: &str, origin: Option<&str>) -> axum::http::HeaderMap {
        let mut m = axum::http::HeaderMap::new();
        m.insert(header::HOST, host.parse().unwrap());
        if let Some(o) = origin {
            m.insert(header::ORIGIN, o.parse().unwrap());
        }
        m
    }

    #[test]
    fn lan_access_answers_private_names_but_not_other_sites() {
        use axum::http::Method;
        let h = host_headers;
        let none: &[String] = &[];
        for ok in [
            "nas.lan:7860",
            "nas:7860",
            "192.168.1.50:7860",
            "[fe80::1]:7860",
            "studio.local",
            "localhost:7860",
        ] {
            assert_eq!(
                foreign(&Method::GET, &h(ok, None), true, none),
                None,
                "{ok}"
            );
        }
        assert_eq!(
            foreign(
                &Method::POST,
                &h("192.168.1.50:7860", Some("http://192.168.1.50:7860")),
                true,
                none
            ),
            None
        );
        assert!(foreign(
            &Method::POST,
            &h("192.168.1.50:7860", Some("https://evil.example")),
            true,
            none
        )
        .is_some());
        assert!(foreign(
            &Method::DELETE,
            &h("nas.lan:7860", Some("http://nas.lan:7861")),
            true,
            none
        )
        .is_some());
    }

    #[test]
    fn lan_access_refuses_rebinding_names() {
        use axum::http::Method;
        let h = host_headers;
        let none: &[String] = &[];
        // a page on evil.example that re-points its name at the studio sends it as Host and as Origin: they match
        // each other, so only the Host check stops it, for reads and for changes
        assert!(foreign(&Method::GET, &h("evil.example:7860", None), true, none).is_some());
        assert!(foreign(
            &Method::POST,
            &h("evil.example:7860", Some("http://evil.example:7860")),
            true,
            none
        )
        .is_some());
        assert!(foreign(&Method::GET, &h("nas.lan.evil.example", None), true, none).is_some());
        // a reverse proxy's name works once it is listed
        let listed = parse_hosts("Studio.Example.org, other.example.org:443");
        assert_eq!(
            foreign(
                &Method::POST,
                &h("studio.example.org", Some("https://studio.example.org")),
                true,
                &listed
            ),
            None
        );
        assert!(foreign(&Method::GET, &h("other.example.org", None), true, &listed).is_none());
        assert!(foreign(&Method::GET, &h("evil.example", None), true, &listed).is_some());
        // local-only mode is unchanged: localhost addresses only, the list does not apply
        assert_eq!(
            foreign(&Method::GET, &h("localhost:7860", None), false, none),
            None
        );
        assert_eq!(
            foreign(&Method::GET, &h("[::1]:7860", None), false, none),
            None
        );
        assert!(foreign(&Method::GET, &h("nas.lan:7860", None), false, none).is_some());
        assert!(foreign(&Method::GET, &h("studio.example.org", None), false, &listed).is_some());
    }

    #[tokio::test]
    async fn answers_carry_security_headers() {
        let res = app("")
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let h = res.headers();
        assert_eq!(h[header::X_FRAME_OPTIONS], "DENY");
        assert_eq!(h[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        let csp = h[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
        assert!(csp.contains("frame-ancestors 'none'") && csp.contains("script-src 'self'"));
        // errors and API answers too
        let res = app("")
            .oneshot(Request::get("/nothing-here").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(res.headers().contains_key(header::X_FRAME_OPTIONS));
    }

    #[test]
    fn the_ui_needs_nothing_the_csp_forbids() {
        // no inline <script>, no event-handler attributes, no external script/style/image: the CSP's script-src 'self'
        // relies on it
        let html = String::from_utf8(Static::get("index.html").unwrap().data.into_owned()).unwrap();
        let inline = regex::Regex::new(r"<script(\s[^>]*)?>\s*[^<\s]").unwrap();
        assert!(!inline.is_match(&html), "inline script in index.html");
        let handler = regex::Regex::new(r#"(?i)\son[a-z]+\s*="#).unwrap();
        assert!(
            !handler.is_match(&html),
            "inline event handler in index.html"
        );
        let external = regex::Regex::new(r#"(?i)(src|href)\s*=\s*"https?://"#).unwrap();
        assert!(!external.is_match(&html), "external resource in index.html");
    }

    #[test]
    fn wrong_attempts_are_slowed_down() {
        let g = AuthGuard::new();
        let ip: IpAddr = "192.168.1.20".parse().unwrap();
        let t0 = Instant::now();
        for _ in 0..FREE_FAILURES - 1 {
            g.failed_at(ip, t0, "password");
            assert_eq!(g.wait_at(ip, t0), None);
        }
        g.failed_at(ip, t0, "password");
        assert_eq!(g.wait_at(ip, t0), Some(2)); // a 1 s lockout, rounded up
        assert_eq!(g.wait_at(ip, t0 + Duration::from_secs(2)), None);
        g.failed_at(ip, t0 + Duration::from_secs(2), "password");
        assert!(g.wait_at(ip, t0 + Duration::from_secs(3)).is_some()); // 2 s now
        for i in 0..20 {
            g.failed_at(ip, t0 + Duration::from_secs(10 + i), "password");
        }
        let t = t0 + Duration::from_secs(30);
        assert!(g.wait_at(ip, t).is_some_and(|w| w <= MAX_LOCKOUT_SECS + 1));
        // ten quiet minutes, or a good attempt, start over
        assert_eq!(
            g.wait_at(ip, t + FORGET_AFTER + Duration::from_secs(MAX_LOCKOUT_SECS)),
            None
        );
        g.succeeded(ip);
        assert_eq!(g.wait_at(ip, t), None);
    }

    #[tokio::test]
    async fn only_upload_routes_take_big_bodies() {
        let big = "x".repeat(3 << 20); // over the small limit, under the others
        let post = |path: &str| {
            Request::post(path)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(big.clone()))
                .unwrap()
        };
        for path in ["/api/history/delete", "/api/settings/auto", "/api/search"] {
            let (code, _, _) = send(app(""), post(path)).await;
            assert_eq!(code, StatusCode::PAYLOAD_TOO_LARGE, "{path}");
        }
        for path in ["/api/jobs", "/api/chats/abc", "/api/voices"] {
            let (code, _, _) = send(app(""), post(path)).await;
            assert_ne!(code, StatusCode::PAYLOAD_TOO_LARGE, "{path}");
        }
        assert!(body_limit_for(&axum::http::Method::POST, "/api/extract") == MAX_BODY);
    }

    #[test]
    fn one_clients_failures_do_not_lock_out_another() {
        let g = AuthGuard::new();
        let t0 = Instant::now();
        let guesser: IpAddr = "192.168.1.66".parse().unwrap();
        let user: IpAddr = "192.168.1.20".parse().unwrap();
        for _ in 0..FREE_FAILURES + 3 {
            g.failed_at(guesser, t0, "password");
        }
        assert!(g.wait_at(guesser, t0).is_some());
        assert_eq!(g.wait_at(user, t0), None);
        g.succeeded(user);
        assert!(g.wait_at(guesser, t0).is_some());
        // the table stays bounded, dropping the address quiet the longest
        for i in 0..(MAX_CLIENTS as u32 + 50) {
            let ip = IpAddr::from(std::net::Ipv4Addr::from(0x0a00_0000 + i));
            g.failed_at(ip, t0 + Duration::from_secs(1 + u64::from(i)), "password");
        }
        assert!(g.0.lock().unwrap().len() <= MAX_CLIENTS);
        assert_eq!(g.wait_at(guesser, t0), None, "the oldest was dropped");
    }

    #[tokio::test]
    async fn too_many_wrong_passwords_get_429() {
        let a = app("right-password");
        let basic = |p: &str| {
            use base64::Engine;
            format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(format!("u:{p}"))
            )
        };
        let req = |p: &str| {
            Request::get("/api/settings")
                .header(header::AUTHORIZATION, basic(p))
                .body(Body::empty())
                .unwrap()
        };
        let mut last = StatusCode::OK;
        for _ in 0..FREE_FAILURES + 1 {
            last = a.clone().oneshot(req("wrong")).await.unwrap().status();
        }
        assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
        // even the right password waits while the lockout lasts
        assert_eq!(
            a.clone()
                .oneshot(req("right-password"))
                .await
                .unwrap()
                .status(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }

    #[test]
    fn mcp_token_is_left_out_of_settings_answers() {
        let v = without_mcp_token(json!({"mcp": {"on": true, "token": "abc"}}));
        assert_eq!(v["mcp"], json!({"on": true, "token_set": true}));
        let v = without_mcp_token(json!({"mcp": {"on": false, "token": ""}}));
        assert_eq!(v["mcp"], json!({"on": false, "token_set": false}));
    }

    #[tokio::test]
    async fn unconfigured() {
        let (code, _, body) = get("/api/models").await;
        assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["error"]["message"],
            json!(device::NOT_CONFIGURED)
        );
        let (code, _, _) = get("/api/model/plan").await;
        assert_eq!(code, StatusCode::BAD_REQUEST);
    }
}
