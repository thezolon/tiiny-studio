//! Jobs and the library: /api/jobs, history, /images/*, voices, saved chats, export, wipe, smart search.
//! Same routes and JSON as server.py.

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde_json::{json, Value};

use crate::routes::{error, mime, read_json, str_of};
use crate::store::{safe_file, save_json, shred, word};
use crate::{device, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/jobs", get(list_jobs).post(submit_job))
        .route("/api/jobs/{id}", get(one_job))
        .route("/api/jobs/{id}/cancel", post(cancel_job))
        .route("/api/history", get(history))
        .route("/api/device/recover", post(recover))
        .route("/api/history/delete", post(delete_entries))
        .route("/api/history/{id}", delete(delete_entry))
        .route("/api/history/{id}/fav", post(fav))
        .route("/images/{name}", get(image))
        .route("/api/voices", get(voices).post(save_voice))
        .route("/api/voices/{id}", delete(delete_voice))
        .route("/api/chats", get(chats))
        .route("/api/chats/{id}", get(chat).post(save_chat))
        .route("/api/chats/{id}/delete", post(delete_chat))
        .route("/api/privacy/wipe", post(wipe))
        .route("/api/export", post(export))
        .route("/api/search", post(search))
}

fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "not found")
}

async fn list_jobs(State(state): State<AppState>) -> Json<Value> {
    Json(Value::Array(state.jobs.list()))
}

/// One job, polled fast while a chat reply streams in.
async fn one_job(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.jobs.get(&id).filter(|_| word(&id)) {
        Some(j) => Json(j).into_response(),
        None => error(StatusCode::NOT_FOUND, "no such job"),
    }
}

async fn submit_job(State(state): State<AppState>, body: Bytes) -> Response {
    let mut req = match read_json(&body) {
        Ok(v) if v.is_object() => v,
        Ok(_) => return error(StatusCode::BAD_REQUEST, "expected a JSON object"),
        Err(e) => return error(StatusCode::BAD_REQUEST, &e),
    };
    let obj = req.as_object_mut().unwrap();
    let kind = obj.remove("kind").and_then(|k| k.as_str().map(String::from)).unwrap_or_default();
    if kind == "chat" {
        // the chat's speak tool uses a saved voice
        if let Some(vid) = obj.remove("voice_id").filter(device::truthy) {
            obj.insert("voice".into(), state.store.voice_for(vid.as_str().unwrap_or("")));
        }
    }
    match state.jobs.submit(&state, &kind, req) {
        Ok(v) => Json(v).into_response(),
        Err(e) => error(StatusCode::BAD_REQUEST, &e.0),
    }
}

async fn cancel_job(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let ok = word(&id) && state.jobs.cancel(&state, &id).await;
    (if ok { StatusCode::OK } else { StatusCode::NOT_FOUND }, Json(json!({"ok": true}))).into_response()
}

async fn history(State(state): State<AppState>) -> Json<Value> {
    let _g = state.store.lock();
    Json(Value::Array(state.store.load_history()))
}

/// Save what the Tiiny holds that the gallery doesn't (clips and songs left by a restart or another studio).
async fn recover(State(state): State<AppState>) -> Json<Value> {
    Json(crate::media::recover::recover(&state).await)
}

/// The history entries with these ids, read before they are deleted (their Tiiny sessions go with them).
fn entries_of(state: &AppState, ids: &[String]) -> Vec<Value> {
    let _g = state.store.lock();
    state.store.load_history().into_iter().filter(|e| ids.iter().any(|i| i == str_of(e, "id"))).collect()
}

fn forget(state: &AppState, ids: &[String]) {
    for id in ids {
        state.jobs.remove(id);
        state.search_cache.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    }
}

async fn delete_entries(State(state): State<AppState>, body: Bytes) -> Response {
    let req = match read_json(&body) {
        Ok(v) => v,
        Err(e) => return error(StatusCode::BAD_REQUEST, &e),
    };
    let mut ids: Vec<String> = req.get("ids").and_then(Value::as_array).into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect();
    ids.sort();
    ids.dedup();
    let sessions = crate::media::recover::sessions_of(&entries_of(&state, &ids));
    state.store.delete_entries(&ids);
    forget(&state, &ids);
    crate::media::recover::delete_device_sessions(&state, sessions); // the Tiiny's copy goes too
    Json(json!({"ok": true, "deleted": ids.len()})).into_response()
}

async fn delete_entry(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if !word(&id) {
        return not_found();
    }
    let ids = vec![id];
    let sessions = crate::media::recover::sessions_of(&entries_of(&state, &ids));
    state.store.delete_entries(&ids);
    forget(&state, &ids); // the finished job's in-memory copy (prompt, label) goes too
    crate::media::recover::delete_device_sessions(&state, sessions);
    Json(json!({"ok": true})).into_response()
}

async fn fav(State(state): State<AppState>, Path(id): Path<String>, body: Bytes) -> Response {
    if !word(&id) {
        return not_found();
    }
    let req = match read_json(&body) {
        Ok(v) => v,
        Err(e) => return error(StatusCode::BAD_REQUEST, &e),
    };
    let fav = req.get("fav").map(device::truthy).unwrap_or(true);
    state.store.edit_history(&id, |e| {
        if fav {
            e.insert("fav".into(), json!(true));
        } else {
            e.remove("fav");
        }
    });
    Json(json!({"ok": true, "fav": fav})).into_response()
}

async fn image(State(state): State<AppState>, Path(name): Path<String>, headers: HeaderMap) -> Response {
    if !safe_file(&name) {
        return not_found();
    }
    let Ok(b) = std::fs::read(state.store.images.join(&name)) else { return not_found() };
    let common = [(header::CONTENT_TYPE, mime(&name)), (header::CACHE_CONTROL, "no-store"), (header::ACCEPT_RANGES, "bytes")];
    // one byte range ("bytes=a-b", "bytes=a-", "bytes=-n"): a <video> asks for them to seek
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok()).and_then(|r| r.strip_prefix("bytes=")).and_then(|r| r.split_once('-'));
    let Some((a, z)) = range.filter(|(a, z)| (!a.is_empty() || !z.is_empty()) && a.chars().chain(z.chars()).all(|c| c.is_ascii_digit())) else {
        return (common, b).into_response();
    };
    let len = b.len() as u64;
    let last = len.saturating_sub(1);
    let (start, end) = if a.is_empty() {
        (len.saturating_sub(z.parse().unwrap_or(0)), last)
    } else {
        (a.parse().unwrap_or(u64::MAX), z.parse().unwrap_or(last).min(last))
    };
    if len == 0 || start > end {
        return (StatusCode::RANGE_NOT_SATISFIABLE, [(header::CONTENT_RANGE, format!("bytes */{len}"))]).into_response();
    }
    let part = b[start as usize..=end as usize].to_vec();
    (StatusCode::PARTIAL_CONTENT, common, [(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"))], part).into_response()
}

async fn voices(State(state): State<AppState>) -> Json<Value> {
    let _g = state.store.lock();
    Json(Value::Array(state.store.load_voices()))
}

/// The base64 payload of a data URI (media.py decode_uri).
pub(crate) fn decode_uri(uri: &str) -> Result<(String, Vec<u8>), String> {
    use base64::Engine;
    let bad = || "Expected a base64 data URI".to_string();
    let rest = uri.strip_prefix("data:").ok_or_else(bad)?;
    let (meta, data) = rest.split_once(";base64,").ok_or_else(bad)?;
    let ctype = meta.split(';').next().filter(|t| !t.is_empty()).unwrap_or("application/octet-stream").to_string();
    let clean: String = data.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD.decode(clean).map_err(|e| e.to_string())?;
    Ok((ctype, bytes))
}

async fn save_voice(State(state): State<AppState>, body: Bytes) -> Response {
    let req = match read_json(&body) {
        Ok(v) => v,
        Err(e) => return error(StatusCode::BAD_REQUEST, &e),
    };
    let audio = match decode_uri(str_of(&req, "audio")) {
        Ok((_, a)) => a,
        Err(e) => return error(StatusCode::BAD_REQUEST, &e),
    };
    let vid = uuid::Uuid::new_v4().simple().to_string()[..10].to_string();
    let file = format!("voice-{vid}.wav");
    let _ = std::fs::write(state.store.images.join(&file), audio);
    let name = Some(str_of(&req, "name").trim()).filter(|n| !n.is_empty()).unwrap_or("Voice").chars().take(60).collect::<String>();
    let voice = json!({"id": vid, "name": name, "file": file, "text": str_of(&req, "text").trim(), "created": crate::jobs::now() as i64});
    let _g = state.store.lock();
    let mut voices = state.store.load_voices();
    voices.insert(0, voice.clone());
    state.store.save_voices(&voices);
    Json(voice).into_response()
}

async fn delete_voice(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if !word(&id) {
        return not_found();
    }
    let _g = state.store.lock();
    let voices = state.store.load_voices();
    let (gone, keep): (Vec<Value>, Vec<Value>) = voices.into_iter().partition(|v| str_of(v, "id") == id);
    for v in gone {
        let f = str_of(&v, "file");
        if safe_file(f) {
            shred(&state.store.images.join(f));
        }
    }
    state.store.save_voices(&keep);
    Json(json!({"ok": true})).into_response()
}

async fn chats(State(state): State<AppState>) -> Json<Value> {
    Json(Value::Array(state.store.list_chats()))
}

async fn chat(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let path = state.store.chat_path(&id);
    match std::fs::read(&path).ok().filter(|_| word(&id)) {
        Some(b) => ([(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "no-cache")], b).into_response(),
        None => error(StatusCode::NOT_FOUND, "no such chat"),
    }
}

async fn save_chat(State(state): State<AppState>, Path(id): Path<String>, body: Bytes) -> Response {
    if !word(&id) {
        return not_found();
    }
    let mut convo = match read_json(&body) {
        Ok(v) if v.is_object() => v,
        _ => return error(StatusCode::BAD_REQUEST, "bad chat"),
    };
    convo["id"] = json!(id);
    convo["updated"] = json!(crate::jobs::now() as i64);
    save_json(&state.store.chat_path(&id), &convo);
    Json(json!({"ok": true})).into_response()
}

/// Gone completely: the file is overwritten before removal.
async fn delete_chat(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if !word(&id) {
        return not_found();
    }
    let path = state.store.chat_path(&id);
    if path.is_file() {
        shred(&path);
    }
    Json(json!({"ok": true})).into_response()
}

/// Delete every result, source, saved voice and chat (settings are kept).
async fn wipe(State(state): State<AppState>) -> Response {
    if state.jobs.any_active() {
        return error(StatusCode::CONFLICT, "Jobs are still running or queued; wipe after they finish.");
    }
    let g = state.store.lock();
    let sessions = crate::media::recover::sessions_of(&state.store.load_history());
    let files: Vec<_> = std::fs::read_dir(&state.store.images).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    for p in &files {
        shred(p);
    }
    let entries = state.store.load_history().len();
    let voices = state.store.load_voices().len();
    for e in std::fs::read_dir(&state.store.chats).into_iter().flatten().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) == Some("json") {
            shred(&p);
        }
    }
    state.store.save_history(&[]);
    state.store.save_voices(&[]);
    state.jobs.clear();
    state.search_cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
    drop(g);
    crate::media::recover::delete_device_sessions(&state, sessions);
    Json(json!({"ok": true, "entries": entries, "voices": voices, "files": files.len()})).into_response()
}

fn local_stamp(epoch: i64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(epoch, 0).single().map(|t| t.format("%Y%m%d-%H%M%S").to_string()).unwrap_or_default()
}

/// Zip the chosen results (files + an info.json of their settings), built in memory, never stored.
async fn export(State(state): State<AppState>, body: Bytes) -> Response {
    use std::io::Write;
    let req = match read_json(&body) {
        Ok(v) => v,
        Err(e) => return error(StatusCode::BAD_REQUEST, &e),
    };
    let ids: Vec<&str> = req.get("ids").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    let items: Vec<Value> = {
        let _g = state.store.lock();
        state.store.load_history().into_iter().filter(|e| ids.contains(&str_of(e, "id"))).collect()
    };
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for e in &items {
            let stamp = local_stamp(e.get("created").and_then(Value::as_f64).unwrap_or(0.0) as i64);
            let kind = e.get("kind").and_then(Value::as_str).unwrap_or("image");
            let outputs: Vec<String> = e.get("outputs").and_then(Value::as_array).into_iter().flatten().chain(e.get("downloads").and_then(Value::as_array).into_iter().flatten())
                .filter_map(|v| v.as_str().map(String::from)).collect();
            for (n, f) in outputs.iter().enumerate() {
                let path = state.store.images.join(f);
                if !safe_file(f) {
                    continue;
                }
                if let Ok(data) = std::fs::read(&path) {
                    let suffix = std::path::Path::new(f).extension().map(|x| format!(".{}", x.to_string_lossy())).unwrap_or_default();
                    if z.start_file(format!("{stamp}_{kind}_{}_{n}{suffix}", str_of(e, "id")), opts).is_ok() {
                        let _ = z.write_all(&data);
                    }
                }
            }
        }
        let info: Vec<Value> = items.iter().map(|e| {
            let mut o = serde_json::Map::new();
            for k in ["id", "kind", "mode", "created", "params", "text", "duration", "seconds"] {
                o.insert(k.into(), e.get(k).cloned().unwrap_or(Value::Null));
            }
            Value::Object(o)
        }).collect();
        if z.start_file("info.json", opts).is_ok() {
            let _ = z.write_all(serde_json::to_string_pretty(&info).unwrap_or_default().as_bytes());
        }
        let _ = z.finish();
    }
    let name = format!("attachment; filename=\"tiiny-studio-{}.zip\"", local_stamp(crate::jobs::now() as i64));
    ([(header::CONTENT_TYPE, "application/zip".to_string()), (header::CONTENT_DISPOSITION, name), (header::CACHE_CONTROL, "no-store".into())], buf.into_inner()).into_response()
}

fn entry_text(e: &Value) -> String {
    let p = e.get("params").cloned().unwrap_or(json!({}));
    let parts = [p.get("prompt"), p.get("text"), p.get("caption"), p.get("lyrics"), e.get("text")];
    let words: Vec<String> = parts.into_iter().flatten().filter(|v| device::truthy(v)).map(|v| match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }).collect();
    words.join(" ").chars().take(2000).collect()
}

async fn embed(state: &AppState, model: &str, texts: &[String]) -> Result<Vec<Vec<f64>>, String> {
    let mut vecs = vec![];
    for chunk in texts.chunks(32) {
        let (code, res) = device::call(state, "POST", "/v1/embeddings", Some(json!({"model": model, "input": chunk})), 120, None).await;
        if code != 200 {
            let msg = match res.get("error") {
                Some(Value::Object(e)) => e.get("message").and_then(Value::as_str).unwrap_or("").to_string(),
                _ => res.to_string().chars().take(200).collect(),
            };
            return Err(msg);
        }
        let mut data: Vec<Value> = res.get("data").and_then(Value::as_array).cloned().unwrap_or_default();
        data.sort_by_key(|d| d.get("index").and_then(Value::as_i64).unwrap_or(0));
        for d in data {
            vecs.push(d.get("embedding").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_f64).collect());
        }
    }
    Ok(vecs)
}

/// Rank saved results by meaning with the device's embedding model (vectors kept in memory only).
async fn search(State(state): State<AppState>, body: Bytes) -> Response {
    let req = match read_json(&body) {
        Ok(v) => v,
        Err(e) => return error(StatusCode::BAD_REQUEST, &e),
    };
    let query = str_of(&req, "query").trim().to_string();
    let (_, cat) = device::catalog(&state).await;
    let emb: Vec<Value> = cat.get("models").and_then(Value::as_array).into_iter().flatten().filter(|m| str_of(m, "type") == "Text Embedding").cloned().collect();
    let Some(model) = emb.iter().find(|m| str_of(m, "status") == "running").map(|m| str_of(m, "id").to_string()) else {
        let need = emb.first().map(|m| json!(str_of(m, "id"))).unwrap_or(Value::Null);
        return (StatusCode::CONFLICT, Json(json!({"error": {"message": "Smart search needs the embedding model loaded."}, "need_model": need}))).into_response();
    };
    let items: Vec<Value> = {
        let _g = state.store.lock();
        state.store.load_history().into_iter().filter(|e| !entry_text(e).is_empty()).collect()
    };
    let todo: Vec<&Value> = {
        let cache = state.search_cache.lock().unwrap_or_else(|e| e.into_inner());
        items.iter().filter(|e| cache.get(str_of(e, "id")).map(|c| &c.0) != Some(&entry_text(e))).collect()
    };
    let texts: Vec<String> = todo.iter().map(|e| entry_text(e)).collect();
    let vecs = match embed(&state, &model, &texts).await {
        Ok(v) => v,
        Err(e) => return error(StatusCode::BAD_GATEWAY, &e),
    };
    {
        let mut cache = state.search_cache.lock().unwrap_or_else(|e| e.into_inner());
        for ((e, t), v) in todo.iter().zip(texts).zip(vecs) {
            cache.insert(str_of(e, "id").to_string(), (t, v));
        }
    }
    let q = match embed(&state, &model, &[query]).await {
        Ok(mut v) if !v.is_empty() => v.remove(0),
        Ok(_) => vec![],
        Err(e) => return error(StatusCode::BAD_GATEWAY, &e),
    };
    let norm = |v: &[f64]| Some(v.iter().map(|x| x * x).sum::<f64>().sqrt()).filter(|n| *n != 0.0).unwrap_or(1.0);
    let qn = norm(&q);
    let cache = state.search_cache.lock().unwrap_or_else(|e| e.into_inner());
    let mut scored: Vec<(f64, String)> = items.iter().filter_map(|e| {
        let id = str_of(e, "id");
        cache.get(id).map(|(_, v)| (q.iter().zip(v).map(|(a, b)| a * b).sum::<f64>() / (qn * norm(v)), id.to_string()))
    }).collect();
    scored.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let results: Vec<Value> = scored.into_iter().map(|(s, i)| json!({"id": i, "score": (s * 10000.0).round() / 10000.0})).collect();
    Json(json!({"results": results})).into_response()
}
