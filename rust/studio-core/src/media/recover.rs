//! Results left on the device (media.py Media.recover): a finished clip or song the studio never saved (it restarted
//! mid-render, or another studio made it) is pulled into the gallery. device_seen.json lists every device id the studio
//! saved, deleted or skipped; the first look only records what's there, so the device's history is never imported.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};

use super::{py_round, quote};
use crate::settings::Target;
use crate::{device, jobs::now, AppState};

/// Songs and loops older than this are never pulled from the device.
const MUSIC_SECONDS: f64 = 24.0 * 3600.0;

fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_string()
}

/// The list a gateway endpoint answers; empty when the firmware lacks it (404); None when it can't be read.
async fn device_list(state: &AppState, to: &Target, path: &str, key: &str) -> Option<Vec<Value>> {
    let (code, res) = device::call(state, "GET", path, None, 20, Some((&to.url, &to.key))).await;
    if code == 404 {
        return Some(vec![]);
    }
    let items = res.get(key).and_then(Value::as_array).filter(|_| code == 200)?;
    Some(items.iter().filter(|x| x.is_object()).cloned().collect())
}

/// Ids of the music sessions on the device now, or None when it can't say.
pub async fn session_ids(state: &AppState, to: &Target) -> Option<BTreeSet<String>> {
    if to.url.is_empty() {
        return None;
    }
    let sessions = device_list(state, to, "/v1/music/sessions", "sessions").await?;
    Some(sessions.iter().filter_map(|s| s.get("session_id").and_then(Value::as_str).map(String::from)).collect())
}

/// A file from the gateway: the bytes of a 200 answer, nothing otherwise.
async fn fetch(state: &AppState, to: &Target, path: &str, timeout: u64) -> Option<Vec<u8>> {
    let res = state.http.get(format!("{}{path}", to.url)).bearer_auth(&to.key).timeout(Duration::from_secs(timeout)).send().await.ok()?;
    if res.status().as_u16() != 200 {
        return None;
    }
    res.bytes().await.ok().map(|b| b.to_vec())
}

/// A number the device sent as a number or as text ("2.29167").
fn num(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn when(x: &Value) -> f64 {
    x.get("created_at").and_then(Value::as_f64).unwrap_or(0.0)
}

fn text<'a>(x: &'a Value, k: &str) -> &'a str {
    x.get(k).and_then(Value::as_str).unwrap_or("")
}

/// Save what the Tiiny holds that the gallery doesn't. Skipped while a job has the device (its own clip or session
/// would look unclaimed); the page asks again on its next load.
pub async fn recover(state: &AppState) -> Value {
    let nothing = json!({"recovered": [], "baseline": false});
    let to = state.settings.current(); // one snapshot: the address and key this pass uses throughout
    if to.url.is_empty() {
        return nothing;
    }
    let Ok(_device) = state.device_lock.try_lock() else { return nothing };
    let vids = device_list(state, &to, "/v1/videos", "data").await;
    let sessions = device_list(state, &to, "/v1/music/sessions", "sessions").await;
    let Some(seen) = state.store.seen_ids() else {
        // first look: everything there now is history
        let (Some(vids), Some(sessions)) = (vids, sessions) else { return nothing };
        let ids: Vec<String> = vids
            .iter()
            .filter(|v| text(v, "status") == "completed")
            .filter_map(|v| v.get("id").and_then(Value::as_str))
            .chain(sessions.iter().filter_map(|s| s.get("session_id").and_then(Value::as_str)))
            .map(String::from)
            .collect();
        state.store.mark_seen(&ids, true);
        return json!({"recovered": [], "baseline": true});
    };
    let mut out = vec![];
    let mut vids: Vec<Value> = vids.unwrap_or_default().into_iter().filter(|v| text(v, "status") == "completed" && !text(v, "id").is_empty() && !seen.contains(text(v, "id"))).collect();
    vids.sort_by(|a, b| when(a).partial_cmp(&when(b)).unwrap_or(std::cmp::Ordering::Equal));
    for v in &vids {
        if let Some(e) = recover_video(state, &to, v).await {
            out.push(e);
        }
    }
    let recent = now() - MUSIC_SECONDS;
    let mut new: Vec<Value> = sessions
        .unwrap_or_default()
        .into_iter()
        .filter(|s| text(s, "status") == "completed" && !text(s, "session_id").is_empty() && !seen.contains(text(s, "session_id")) && when(s) >= recent)
        .collect();
    new.sort_by(|a, b| when(a).partial_cmp(&when(b)).unwrap_or(std::cmp::Ordering::Equal));
    for s in &new {
        if let Some(e) = recover_session(state, &to, s).await {
            out.push(e);
        }
    }
    json!({"recovered": out, "baseline": false})
}

async fn recover_video(state: &AppState, to: &Target, v: &Value) -> Option<Value> {
    let vid = text(v, "id");
    let data = fetch(state, to, &format!("/v1/videos/{}/content", quote(vid)), 600).await.filter(|d| !d.is_empty())?;
    let id = new_id();
    let out = format!("{id}-video.mp4");
    std::fs::write(state.store.images.join(&out), data).ok()?; // can't write here: it stays on the device for the next look
    let secs = num(v.get("seconds")).filter(|x| x.is_finite()).map_or(2, |x| py_round(x).max(1));
    let frames = num(v.get("frames")).unwrap_or(0.0);
    let fps = num(v.get("fps")).filter(|f| *f != 0.0).unwrap_or(24.0);
    let duration = if frames > 0.0 && fps > 0.0 { (frames / fps * 10.0).round() / 10.0 } else { secs as f64 };
    let size = Some(text(v, "size")).filter(|s| !s.is_empty()).unwrap_or("832x480");
    let entry = json!({"id": id, "created": now() as i64, "kind": "video", "seconds": 0,
        "params": {"model": text(v, "model"), "prompt": "", "size": size, "seconds": secs, "recovered": true},
        "sources": [], "outputs": [out], "duration": duration});
    if state.store.add_history(entry).is_err() {
        crate::store::shred(&state.store.images.join(&out));
        return None;
    }
    state.store.mark_seen(&[vid.to_string()], false);
    let _ = device::call(state, "DELETE", &format!("/v1/videos/{}", quote(vid)), None, 30, Some((&to.url, &to.key))).await; // the device keeps it an hour otherwise
    Some(json!({"id": id, "kind": "video"}))
}

async fn recover_session(state: &AppState, to: &Target, s: &Value) -> Option<Value> {
    let sid = text(s, "session_id");
    let model = text(s, "model_id");
    let mp3 = fetch(state, to, &format!("/v1/music/sessions/{}/download?format=mp3", quote(sid)), 120).await.filter(|m| m.len() > 1000)?;
    let id = new_id();
    let kind = if model.contains("Foundation-1") { "loop" } else { "song" };
    let out = format!("{id}-{kind}.mp3");
    std::fs::write(state.store.images.join(&out), mp3).ok()?;
    let entry = json!({"id": id, "created": now() as i64, "kind": kind, "seconds": 0, "params": {"model": model, "recovered": true},
        "sources": [], "outputs": [out], "downloads": [], "duration": null, "device": {"music_session": sid}});
    if state.store.add_history(entry).is_err() {
        crate::store::shred(&state.store.images.join(&out));
        return None;
    }
    state.store.mark_seen(&[sid.to_string()], false); // the session stays on the Tiiny until the entry is deleted
    Some(json!({"id": id, "kind": kind}))
}

/// Delete the Tiiny's copy of these songs and loops (their entries are gone). Best effort, in the background (it waits
/// its turn for the device): never blocks or fails the local delete.
pub fn delete_device_sessions(state: &AppState, ids: Vec<String>) {
    let ids: Vec<String> = ids.into_iter().filter(|i| !i.is_empty()).collect();
    let to = state.settings.current();
    if ids.is_empty() || to.url.is_empty() {
        return;
    }
    state.store.mark_seen(&ids, false);
    let state = state.clone();
    tokio::spawn(async move {
        let _device = state.device_lock.lock().await;
        for id in ids {
            let _ = device::call(&state, "DELETE", &format!("/v1/music/sessions/{}", quote(&id)), None, 15, Some((&to.url, &to.key))).await;
        }
    });
}

/// The device session ids of these history entries.
pub fn sessions_of(entries: &[Value]) -> Vec<String> {
    entries.iter().filter_map(|e| e.pointer("/device/music_session").and_then(Value::as_str).map(String::from)).collect()
}
