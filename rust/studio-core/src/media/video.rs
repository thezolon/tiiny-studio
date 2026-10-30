//! Text or a first (and last) frame to a clip with sound (MiniMax-H3-Turbo) through the gateway's /v1/videos: submit,
//! poll, download the MP4, then delete the device's copy (media.py run_video). Sizes and steps are the model's own
//! (/v1/video/info).

use std::time::Duration;

use base64::Engine;
use serde_json::{json, Map, Value};

use super::{decode_uri, ensure_running, err, http, http_bytes, save};
use crate::jobs::{Ctx, JobError, JobResult};

/// The request fields for the first and last frame.
const FRAME_FIELDS: [&str; 2] = ["image", "last_image"]; // first frame tested on 0.1.38; the last-frame name is a guess

/// A failed or cancelled clip keeps none of the frames it saved.
pub async fn run(ctx: Ctx) -> JobResult {
    let mut saved = vec![];
    let result = make(&ctx, &mut saved).await;
    if result.is_err() {
        for name in &saved {
            let _ = std::fs::remove_file(ctx.state.store.images.join(name));
        }
    }
    result
}

/// A whole number from the request (Python's isinstance(x, int): not a float, not a bool).
fn int_of(req: &Value, k: &str) -> Option<i64> {
    req.get(k).filter(|v| v.is_i64() || v.is_u64()).and_then(Value::as_i64)
}

async fn make(ctx: &Ctx, saved: &mut Vec<String>) -> JobResult {
    let req = &ctx.req;
    let s = |k: &str| req.get(k).and_then(Value::as_str).unwrap_or("");
    let prompt = s("prompt").trim().to_string();
    if prompt.is_empty() && s("image").is_empty() {
        return Err(JobError("Describe the clip, or give it a picture to start from.".into()));
    }
    let model = ctx.model.clone();
    ensure_running(ctx, &model).await?;
    let url = format!("{}/v1/videos", ctx.state.settings.current().url);
    let secs = int_of(req, "seconds").filter(|x| (1..=20).contains(x)).unwrap_or(2);
    let size = Some(s("size")).filter(|x| !x.is_empty()).unwrap_or("832x480").to_string();
    let mut body = Map::new();
    body.insert("model".into(), json!(model));
    body.insert("prompt".into(), json!(if prompt.is_empty() { "Bring this picture to life" } else { &prompt }));
    body.insert("size".into(), json!(size));
    body.insert("seconds".into(), json!(secs.to_string()));
    for k in ["steps", "seed"] {
        if let Some(v) = int_of(req, k) {
            body.insert(k.into(), json!(v));
        }
    }
    let mut sources = vec![];
    for (i, k) in ["image", "last_image"].iter().enumerate() {
        if s(k).is_empty() {
            continue;
        }
        let (ctype, data) = decode_uri(s(k))?;
        let name = save(ctx, &format!("{}-frame{i}.png", ctx.id), &data);
        saved.push(name.clone());
        sources.push(name);
        body.insert(FRAME_FIELDS[i].into(), json!(format!("data:{ctype};base64,{}", base64::engine::general_purpose::STANDARD.encode(&data))));
    }
    let body = Value::Object(body);
    let (code, res) = http(ctx, "POST", &url, Some(body.clone()), 120, true).await?;
    if code >= 300 {
        return Err(JobError(err(&res)));
    }
    let vid = res.get("id").and_then(Value::as_str).unwrap_or("").to_string();
    ctx.set_remote_task(Some(format!("DELETE {url}/{vid}"))); // deleting a clip stops its render (there's no /cancel)
    let mut silent = 0;
    let (v, st) = loop {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let (_, v) = http(ctx, "GET", &format!("{url}/{vid}"), None, 30, true).await?;
        if ctx.cancelled() {
            return Err(JobError("Cancelled".into()));
        }
        let st = v.get("status").and_then(Value::as_str).unwrap_or("").to_string();
        if ["completed", "failed", "cancelled"].contains(&st.as_str()) {
            break (v, st);
        }
        silent = if st.is_empty() { silent + 1 } else { 0 };
        if silent >= 20 {
            return Err(JobError("The device stopped reporting on this video".into()));
        }
        ctx.update(json!({"progress": v.get("progress").and_then(Value::as_f64).unwrap_or(0.0) as i64,
                          "message": if st == "in_progress" { "Rendering" } else { "Waiting in the device queue" }}));
    };
    ctx.set_remote_task(None);
    if st != "completed" {
        let e = v.get("error").filter(|e| !e.is_null()).cloned().unwrap_or(json!(st));
        return Err(JobError(err(&e)));
    }
    ctx.update(json!({"message": "Downloading"}));
    let (code, data) = http_bytes(ctx, "GET", &format!("{url}/{vid}/content"), None, 600, true).await?;
    let data = match data {
        Ok(d) if code == 200 && !d.is_empty() => d,
        Ok(_) => return Err(JobError("Couldn't fetch the video: empty".into())),
        Err(e) => return Err(JobError(format!("Couldn't fetch the video: {}", err(&e)))),
    };
    let out = save(ctx, &format!("{}-video.mp4", ctx.id), &data);
    saved.push(out.clone());
    let _ = http(ctx, "DELETE", &format!("{url}/{vid}"), None, 30, true).await; // the device keeps it an hour otherwise
    let frames = v.get("frames").and_then(Value::as_f64).unwrap_or(0.0);
    let fps = v.get("fps").and_then(Value::as_f64).filter(|f| *f > 0.0).unwrap_or(24.0);
    let duration = if frames > 0.0 { (frames / fps * 10.0).round() / 10.0 } else { secs as f64 };
    let mut params = json!({"model": model, "prompt": prompt, "size": size, "seconds": secs});
    for k in ["steps", "seed"] {
        if let Some(x) = body.get(k) {
            params[k] = x.clone();
        }
    }
    Ok(json!({"kind": "video", "params": params, "sources": sources, "outputs": [out], "duration": duration}))
}
