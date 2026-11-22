//! The model store: browse it, download a model (one at a time), pause, cancel, update (delete + download), delete.
//! Same contract as server.py's model_store()/downloads()/model_change(); the contract tests compare the two.
//!
//! What the device gives (tested on TiinyOS 0.1.39): the store at /api/v1/models/online_models, live download state
//! in /api/v1/models/?including_downloading=true, POST …/{id}/download and DELETE …/{id} (the id keeps its "/").
//! An update is a delete + a fresh download; need_update flags one (false for every model while the Tiiny is offline).

use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::device::{call, disk_space, workspace, LOCKED};
use crate::AppState;

/// Left free after a download, like the jobs' own check.
const SPACE_FLOOR_GB: f64 = 2.0;
const STORE_TTL: Duration = Duration::from_secs(60);
const STORE_KEEP: [&str; 21] = [
    "name",
    "type",
    "params",
    "npu_usage",
    "desc",
    "supported",
    "input",
    "output",
    "context_window",
    "load_time",
    "capabilities",
    "thinking",
    "reasoning_levels",
    "image_size",
    "steps",
    "guidance_scale",
    "downloads",
    "likes",
    "created_time",
    "version_label",
    "recommend_rank",
];

/// (when read, device url, the store's entries in its own order)
pub(crate) type StoreCache = (Instant, String, Vec<(String, Value)>);

type Answer = (u16, Value);

fn fail(code: u16, message: &str) -> Answer {
    (code, json!({"error": {"message": message}}))
}

/// A device row's state, normalised: downloading, paused, error, or downloaded (installed, loaded or not).
/// A device row's state, normalised: downloaded (installed, loaded or not), downloading, paused, error, or "absent"
/// (not on the Tiiny; device_rows drops those). A paused download reads "download_stop" on 0.1.39 (tested
/// 2026-11-21). A status this studio doesn't know counts as a download under way: never installed, loadable or
/// deletable.
pub(crate) fn live_status(row: &Value) -> &'static str {
    let s = match row.get("status") {
        None | Some(Value::Null) => "",
        Some(Value::String(s)) => s.as_str(),
        Some(_) => "?",
    };
    match s {
        "downloaded" | "running" | "stopped" | "" => "downloaded",
        "not_downloaded" => "absent",
        "download_stop" => "paused",
        "error" => "error",
        s if s.contains("pause") => "paused",
        _ => "downloading",
    }
}

fn under_way(status: &str) -> bool {
    matches!(status, "downloading" | "paused" | "error")
}

/// The device's own words for a refusal, else its status code.
fn reason(res: &Value, code: u16) -> String {
    let e = res.get("error");
    let msg = match e {
        Some(Value::Object(o)) => o.get("message").and_then(Value::as_str),
        Some(Value::String(s)) => Some(s.as_str()),
        _ => None,
    }
    .filter(|m| !m.is_empty())
    .or_else(|| {
        res.get("detail")
            .and_then(Value::as_str)
            .filter(|m| !m.is_empty())
    });
    msg.map_or_else(|| code.to_string(), String::from)
}

/// Every model on the Tiiny, downloads included, in the device's order; or the catalog's error.
async fn device_rows(state: &AppState) -> Result<Vec<(String, Value)>, Answer> {
    let (code, res) = call(
        state,
        "GET",
        "/api/v1/models/?including_downloading=true",
        None,
        15,
        None,
    )
    .await;
    let unreachable = res
        .pointer("/error/message")
        .and_then(Value::as_str)
        .is_some_and(|m| m.starts_with("Device unreachable"));
    if code == 502 && !unreachable {
        return Err((
            502,
            json!({"error": {"message": LOCKED, "type": "device_locked"}}),
        ));
    }
    if code != 200 {
        return Err((code, res));
    }
    let mut out: Vec<(String, Value)> = vec![];
    for r in res
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(id) = r
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            if live_status(r) != "absent" {
                put(&mut out, id, r.clone());
            }
        }
    }
    Ok(out)
}

/// Insert or replace by id, keeping the first position (like a Python dict).
fn put(list: &mut Vec<(String, Value)>, id: &str, v: Value) {
    match list.iter_mut().find(|(k, _)| k == id) {
        Some(slot) => slot.1 = v,
        None => list.push((id.to_string(), v)),
    }
}

fn find<'a>(list: &'a [(String, Value)], id: &str) -> Option<&'a Value> {
    list.iter().find(|(k, _)| k == id).map(|(_, v)| v)
}

/// The Model Store's entries, cached STUDIO_STORE_TTL seconds (60; the contract tests set 0) per device.
async fn store_facts(state: &AppState) -> Result<Vec<(String, Value)>, Answer> {
    let url = state.settings.current().url;
    let ttl = std::env::var("STUDIO_STORE_TTL")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .map_or(STORE_TTL, |s| Duration::from_secs_f64(s.max(0.0)));
    {
        let c = state.store_cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((t, u, v)) = &*c {
            if *u == url && t.elapsed() < ttl {
                return Ok(v.clone());
            }
        }
    }
    let (code, res) = call(state, "GET", "/api/v1/models/online_models", None, 30, None).await;
    if code != 200 {
        return Err((code, res));
    }
    let items = match &res {
        Value::Array(a) => a.clone(),
        Value::Object(o) => o
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        _ => vec![],
    };
    let mut data = vec![];
    for m in items {
        if let Some(id) = m
            .get("model_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            let id = id.to_string();
            put(&mut data, &id, m);
        }
    }
    *state.store_cache.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((Instant::now(), url, data.clone()));
    Ok(data)
}

/// What the store said last for this device (names and sizes), when it can't be read now.
fn stale_facts(state: &AppState) -> Vec<(String, Value)> {
    let url = state.settings.current().url;
    state
        .store_cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .filter(|(_, u, _)| *u == url)
        .map(|(_, _, v)| v.clone())
        .unwrap_or_default()
}

/// The next disk_space() reads the device again (after something was downloaded, deleted or cancelled).
fn forget_disk(state: &AppState) {
    *state.disk_cache.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The tag with the biggest download (the first of equals), or none.
fn biggest_tag(m: &Value) -> Option<&Value> {
    let mut best: Option<(&Value, f64)> = None;
    for t in m
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|t| t.is_object())
    {
        let size = t.get("size").and_then(Value::as_f64).unwrap_or(0.0);
        if best.is_none_or(|(_, b)| size > b) {
            best = Some((t, size));
        }
    }
    best.map(|(t, _)| t)
}

/// Download size in bytes (as the device sent it), or null.
fn store_size(m: Option<&Value>) -> Value {
    m.and_then(biggest_tag)
        .and_then(|t| t.get("size"))
        .filter(|s| s.as_f64().is_some_and(|n| n != 0.0))
        .cloned()
        .unwrap_or(Value::Null)
}

fn size_bytes(m: Option<&Value>) -> f64 {
    store_size(m).as_f64().unwrap_or(0.0)
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

fn get_or_null(v: Option<&Value>, k: &str) -> Value {
    v.and_then(|v| v.get(k)).cloned().unwrap_or(Value::Null)
}

fn store_entry(mid: &str, m: Option<&Value>, row: Option<&Value>) -> Value {
    let tag = m.and_then(biggest_tag);
    let status = row.map_or("not_downloaded", live_status);
    let mut out = Map::new();
    out.insert("id".into(), json!(mid));
    for k in STORE_KEEP {
        out.insert(k.into(), get_or_null(m, k));
    }
    out.insert("updated_time".into(), get_or_null(m, "udpate_time"));
    out.insert("in_store".into(), json!(m.is_some()));
    let kind = m
        .or(row)
        .and_then(|v| v.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("");
    out.insert("workspace".into(), json!(workspace(kind)));
    out.insert("size".into(), store_size(m));
    out.insert("update_title".into(), get_or_null(tag, "update_title"));
    out.insert("update_desc".into(), get_or_null(tag, "update_desc"));
    out.insert("installed".into(), json!(status == "downloaded"));
    out.insert("status".into(), json!(status));
    out.insert(
        "progress".into(),
        if under_way(status) {
            get_or_null(row, "progress")
        } else {
            Value::Null
        },
    );
    out.insert(
        "need_update".into(),
        json!(status == "downloaded" && truthy(row.and_then(|r| r.get("need_update")))),
    );
    out.insert("version".into(), get_or_null(row, "version"));
    if m.is_none() {
        // on the Tiiny but not in the store (imported): what the device row says
        for k in [
            "type",
            "params",
            "npu_usage",
            "desc",
            "supported",
            "input",
            "output",
            "load_time",
        ] {
            out.insert(k.into(), get_or_null(row, k));
        }
        out.insert("name".into(), json!(mid.rsplit('/').next().unwrap_or(mid)));
    }
    Value::Object(out)
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// GET /api/store
pub async fn model_store(state: &AppState) -> Answer {
    let rows = match device_rows(state).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    // the installed models can still be managed when the store can't be read; say why it's missing
    let (facts, store_error) = match store_facts(state).await {
        Ok(f) => (f, Value::Null),
        Err((code, res)) => (
            vec![],
            json!(format!(
                "The Model Store can't be read right now: {}",
                reason(&res, code)
            )),
        ),
    };
    let mut models: Vec<Value> = facts
        .iter()
        .map(|(mid, m)| store_entry(mid, Some(m), find(&rows, mid)))
        .collect();
    models.extend(
        rows.iter()
            .filter(|(mid, _)| find(&facts, mid).is_none())
            .map(|(mid, r)| store_entry(mid, None, Some(r))),
    );
    let downloading = models
        .iter()
        .find(|m| m["status"] == "downloading")
        .map_or(Value::Null, |m| m["id"].clone());
    let updates = models.iter().filter(|m| m["need_update"] == true).count();
    let disk = disk_space(state).await.map_or(
        Value::Null,
        |(f, t)| json!({"free_gb": round1(f), "total_gb": round1(t)}),
    );
    (
        200,
        json!({"models": models, "downloading": downloading, "updates": updates, "store_error": store_error,
               "disk": disk}),
    )
}

/// GET /api/downloads: small, for polling (names and sizes from the cached store, never fetched here).
pub async fn downloads(state: &AppState) -> Answer {
    let rows = match device_rows(state).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let facts = stale_facts(state);
    let list: Vec<Value> = rows
        .iter()
        .filter(|(_, r)| under_way(live_status(r)))
        .map(|(mid, r)| {
            json!({"id": mid, "name": name_of(mid, &facts), "status": live_status(r),
                   "progress": get_or_null(Some(r), "progress"), "size": store_size(find(&facts, mid))})
        })
        .collect();
    (200, json!({"downloads": list}))
}

fn name_of(mid: &str, facts: &[(String, Value)]) -> String {
    find(facts, mid)
        .and_then(|m| m.get("name"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map_or_else(
            || mid.rsplit('/').next().unwrap_or(mid).to_string(),
            String::from,
        )
}

/// Like server.py's MODEL_ID: org/name, ASCII letters, digits, _ . - only, and no "." or ".." parts.
fn valid_id(mid: &str) -> bool {
    let parts: Vec<&str> = mid.split('/').collect();
    parts.len() == 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && *p != "."
                && *p != ".."
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        })
}

/// Why a model can't be deleted or updated now (loaded, loading, a job on it), or None.
async fn model_busy(state: &AppState, mid: &str, name: &str) -> Option<String> {
    if state.jobs.active_on(mid) {
        return Some(format!(
            "{name} has a job queued or running. Wait for it to finish first."
        ));
    }
    let (code, run) = call(state, "GET", "/api/v1/models/running", None, 15, None).await;
    if code != 200 {
        return Some(format!(
            "Couldn't check whether {name} is loaded: {}",
            reason(&run, code)
        ));
    }
    let list = |k: &str| {
        run.pointer(&format!("/instances/{k}"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let has = |v: &[Value]| {
        v.iter()
            .any(|r| r.get("model_id").and_then(Value::as_str) == Some(mid))
    };
    if has(&list("running")) {
        return Some(format!("{name} is loaded on the Tiiny. Unload it first."));
    }
    let top = run
        .get("pending")
        .and_then(Value::as_array)
        .is_some_and(|p| p.iter().any(|v| v.as_str() == Some(mid)));
    if has(&list("pending")) || top {
        return Some(format!(
            "{name} is loading on the Tiiny. Wait for it, unload it, then try again."
        ));
    }
    None
}

/// Refuse when `need` more bytes (plus the floor) don't fit. Read fresh: a delete a moment ago may have freed room.
async fn space_problem(state: &AppState, name: &str, need: f64) -> Option<String> {
    forget_disk(state);
    let space = disk_space(state).await?;
    (space.0 < need / 1e9 + SPACE_FLOOR_GB).then(|| {
        format!(
            "Not enough space on the Tiiny: {name} needs {:.1} GB plus {SPACE_FLOOR_GB} GB to spare, and {:.1} GB \
             is free. Delete a model you don't use first.",
            need / 1e9,
            space.0
        )
    })
}

/// Bytes a download still needs: all of it, or what's left of a paused one (the fetched part is already on disk).
fn still_to_fetch(row: Option<&Value>, size: f64) -> f64 {
    match row {
        Some(r) if matches!(live_status(r), "paused" | "error") => {
            let p = num(r.get("progress")).min(100.0);
            size * (1.0 - p / 100.0).max(0.0)
        }
        _ => size,
    }
}

/// A number the device sent (not a bool), else 0.
fn num(v: Option<&Value>) -> f64 {
    v.and_then(Value::as_f64).unwrap_or(0.0)
}

fn other_download(
    rows: &[(String, Value)],
    facts: &[(String, Value)],
    mid: &str,
) -> Option<String> {
    rows.iter()
        .find(|(k, r)| k != mid && live_status(r) == "downloading")
        .map(|(k, _)| {
            format!(
                "One download at a time: {} is downloading. Wait for it to finish, or pause it.",
                name_of(k, facts)
            )
        })
}

/// POST /api/model/{download,pause,cancel,delete,update}. Ids are checked before they go into a device path. Every
/// change runs its checks and its device calls under one lock, so nothing changes between a check and the change.
pub async fn model_change(state: &AppState, action: &str, body: &Value) -> Answer {
    let Some(mid) = body
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    else {
        return fail(400, "id required");
    };
    if !valid_id(mid) {
        return fail(400, "That isn't a model id.");
    }
    if matches!(action, "delete" | "update")
        && body.get("confirm").and_then(Value::as_str) != Some(mid)
    {
        return fail(
            400,
            &format!("Confirm the {action}: send the model's id again as \"confirm\"."),
        );
    }
    let answer = {
        let _guard = state.change_lock.lock().await;
        change_model(state, action, mid).await
    };
    if answer.0 == 200 {
        forget_disk(state); // the Store tab's free space shows the change at once
    }
    answer
}

async fn device_answer(state: &AppState, method: &str, path: String, timeout: u64) -> (u16, Value) {
    call(state, method, &path, None, timeout, None).await
}

async fn change_model(state: &AppState, action: &str, mid: &str) -> Answer {
    let rows = match device_rows(state).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let facts = match store_facts(state).await {
        Ok(f) => f,
        Err(e) if action == "download" => return e,
        Err(_) => stale_facts(state), // pause, cancel, delete and update only need the name from it
    };
    let name = name_of(mid, &facts);
    let row = find(&rows, mid);
    let status = row.map(live_status);
    let refused = |what: &str, res: &Value, code: u16| {
        fail(
            502,
            &format!("The Tiiny didn't {what} {name}: {}", reason(res, code)),
        )
    };
    match action {
        "download" => {
            let Some(entry) = find(&facts, mid) else {
                return fail(404, &format!("{mid} isn't in the Model Store."));
            };
            match status {
                Some("downloaded") => {
                    return fail(409, &format!("{name} is already on your Tiiny."))
                }
                Some("downloading") => {
                    return fail(409, &format!("{name} is already downloading."))
                }
                _ => {}
            }
            let why = match other_download(&rows, &facts, mid) {
                Some(w) => Some(w),
                None => {
                    let need = still_to_fetch(row, size_bytes(Some(entry)));
                    space_problem(state, &name, need).await
                }
            };
            if let Some(w) = why {
                return fail(409, &w);
            }
            let (code, res) =
                device_answer(state, "POST", format!("/api/v1/models/{mid}/download"), 60).await;
            if code >= 300 {
                return refused("start the download of", &res, code);
            }
            return (200, json!({"ok": true, "started": mid}));
        }
        "pause" => {
            if status != Some("downloading") {
                return fail(409, &format!("{name} isn't downloading."));
            }
            let (code, res) = device_answer(
                state,
                "POST",
                format!("/api/v1/models/{mid}/pause_download"),
                30,
            )
            .await;
            return if code < 300 {
                (200, json!({"ok": true}))
            } else {
                refused("pause", &res, code)
            };
        }
        "cancel" => {
            if !status.is_some_and(under_way) {
                return fail(409, &format!("{name} isn't downloading."));
            }
            // a model the Tiiny reports oddly may still be in use
            if let Some(why) = model_busy(state, mid, &name).await {
                return fail(409, &why);
            }
            // /interrupt only stops chat replies (tested 2026-11-21). Cancel = pause, then delete what was fetched
            // (tested).
            if status == Some("downloading") {
                let (code, res) = device_answer(
                    state,
                    "POST",
                    format!("/api/v1/models/{mid}/pause_download"),
                    30,
                )
                .await;
                if code >= 300 {
                    return refused("stop", &res, code);
                }
            }
            let (code, res) =
                device_answer(state, "DELETE", format!("/api/v1/models/{mid}"), 120).await;
            return if code < 300 {
                (200, json!({"ok": true}))
            } else {
                refused("stop", &res, code)
            };
        }
        "delete" | "update" => {}
        _ => return fail(404, "not found"),
    }
    let Some(status) = status else {
        return fail(404, &format!("{name} isn't on your Tiiny."));
    };
    if status != "downloaded" {
        return fail(
            409,
            &format!("{name} is still downloading. Cancel the download instead."),
        );
    }
    if action == "update" {
        if !truthy(row.and_then(|r| r.get("need_update"))) {
            return fail(
                409,
                &format!(
                    "No update is waiting for {name}. (The Tiiny reports none while it has no internet.)"
                ),
            );
        }
        let new = size_bytes(find(&facts, mid));
        let old = match num(row.and_then(|r| r.get("size"))) {
            0.0 => new,
            n => n,
        };
        let why = match other_download(&rows, &facts, mid) {
            Some(w) => Some(w),
            None => space_problem(state, &name, (new - old).max(0.0)).await,
        };
        if let Some(w) = why {
            return fail(409, &w);
        }
    }
    // last, right before the change
    if let Some(why) = model_busy(state, mid, &name).await {
        return fail(409, &why);
    }
    let (code, res) = device_answer(state, "DELETE", format!("/api/v1/models/{mid}"), 120).await;
    if action == "delete" {
        return if code < 300 {
            (200, json!({"ok": true, "deleted": mid}))
        } else {
            refused("delete", &res, code)
        };
    }
    if code >= 300 {
        // an update: did the delete happen anyway (an answer lost on the way back)?
        let gone = matches!(device_rows(state).await, Ok(r) if find(&r, mid).is_none());
        if !gone {
            return fail(
                502,
                &format!(
                    "Couldn't delete the old build of {name} ({}); it's left as it was.",
                    reason(&res, code)
                ),
            );
        }
    }
    let (code, res) =
        device_answer(state, "POST", format!("/api/v1/models/{mid}/download"), 60).await;
    if code >= 300 {
        return (
            502,
            json!({"error": {"message": format!(
                "{name}'s old build was deleted, but the download of the new one didn't start ({}). It's missing \
                 until you download it again.", reason(&res, code)), "type": "update_missing"}}),
        );
    }
    (200, json!({"ok": true, "started": mid}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_org_slash_name() {
        assert!(valid_id("Qwen/Qwen3.5-9B-Turbo"));
        assert!(valid_id("hexgrad/Kokoro-82M"));
        for bad in [
            "", "Qwen", "a/b/c", "../x", "a/..", "a/b c", "a/b?x", "/a", "a/",
        ] {
            assert!(!valid_id(bad), "{bad}");
        }
    }

    #[test]
    fn statuses_normalise() {
        for (s, want) in [
            ("downloading", "downloading"),
            ("paused", "paused"),
            ("download_paused", "paused"),
            ("download_stop", "paused"),
            ("not_downloaded", "absent"),
            ("verifying", "downloading"),
            ("", "downloaded"),
            ("error", "error"),
            ("downloaded", "downloaded"),
            ("running", "downloaded"),
            ("stopped", "downloaded"),
        ] {
            assert_eq!(live_status(&json!({"status": s})), want);
        }
    }

    #[test]
    fn size_is_the_biggest_tag() {
        let m = json!({"tags": [{"size": 5}, {"size": 18_300_000_000_u64}, {"size": 7}]});
        assert_eq!(store_size(Some(&m)), json!(18_300_000_000_u64));
        assert_eq!(store_size(Some(&json!({"tags": []}))), Value::Null);
        assert_eq!(store_size(None), Value::Null);
    }

    #[test]
    fn reasons_come_from_the_device() {
        assert_eq!(
            reason(&json!({"error": {"message": "no net"}}), 500),
            "no net"
        );
        assert_eq!(reason(&json!({"error": "boom"}), 500), "boom");
        assert_eq!(reason(&json!({"detail": "Not Found"}), 404), "Not Found");
        assert_eq!(reason(&json!({}), 503), "503");
    }

    #[test]
    fn an_imported_model_keeps_its_device_facts() {
        let row = json!({"id": "me/Custom", "status": "stopped", "type": "Text Generation", "npu_usage": 9});
        let e = store_entry("me/Custom", None, Some(&row));
        assert_eq!(e["in_store"], false);
        assert_eq!(e["name"], "Custom");
        assert_eq!(e["workspace"], "text");
        assert_eq!(e["installed"], true);
        assert_eq!(e["npu_usage"], 9);
    }
}
