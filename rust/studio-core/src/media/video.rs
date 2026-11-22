//! Text or a first (and last) frame to a clip with sound (MiniMax-H3-Turbo) through the gateway's /v1/videos: submit,
//! poll, download the MP4, then delete the device's copy (media.py run_video). Sizes and steps are the model's own
//! (/v1/video/info).

use std::time::Duration;

use base64::Engine;
use serde_json::{json, Map, Value};

use super::{decode_uri, ensure_running, err, http, http_bytes, save};
use crate::jobs::{Ctx, JobError, JobResult};

/// The longest clip the video model renders (MiniMax-H3-Turbo, TiinyOS 0.1.38: 8 s and longer fail at once with
/// "no exported bucket fits a request", though the submit accepts them; docs/DEVICE-API.md).
pub const MAX_SECONDS: i64 = 7;

/// How long a video job rides out a dead link (polls with no answer) before it gives up.
const LOST_SECS: u64 = 300;

/// The NPU's error when the video model doesn't fit next to the other loaded models, in plain words; the device's own
/// text stays at the end for bug reports (#90, media.py video_error).
fn explain(e: String) -> String {
    if e.contains("Weight installation failed") {
        format!("The Tiiny couldn't fit the video model next to the other loaded models. Unload the others in the Models panel, then try again. (Tiiny: {e})")
    } else if e.contains("exceeded the gateway time limit") || e.contains("GENERATION_TIMEOUT") {
        // the render goes on after the gateway gives up (#80, TiinyOS 0.1.38)
        format!("The Tiiny stopped this video at its 1-hour limit, but the render may still be using the NPU. Unloading the video model frees it (the Tiiny then unloads every loaded model). For next time: 480p, fewer steps or fewer seconds. (Tiiny: {e})")
    } else {
        e
    }
}

/// A whole number from the request (Python's isinstance(x, int): not a float, not a bool).
fn int_of(req: &Value, k: &str) -> Option<i64> {
    req.get(k)
        .filter(|v| v.is_i64() || v.is_u64())
        .and_then(Value::as_i64)
}

pub async fn run(ctx: Ctx) -> JobResult {
    let ctx = &ctx;
    let req = &ctx.req;
    let s = |k: &str| req.get(k).and_then(Value::as_str).unwrap_or("");
    let prompt = s("prompt").trim().to_string();
    if prompt.is_empty() && s("image").is_empty() {
        return Err(JobError(
            "Describe the clip, or give it a picture to start from.".into(),
        ));
    }
    let model = ctx.model.clone();
    let secs = int_of(req, "seconds").unwrap_or(2);
    if !(1..=MAX_SECONDS).contains(&secs) {
        return Err(JobError(format!(
            "Videos can be 1 to {MAX_SECONDS} seconds long: the Tiiny's video model can't render {secs} s."
        )));
    }
    ensure_running(ctx, &model).await?;
    let url = format!("{}/v1/videos", ctx.target.url);
    let size = Some(s("size"))
        .filter(|x| !x.is_empty())
        .unwrap_or("832x480")
        .to_string();
    let mut body = Map::new();
    body.insert("model".into(), json!(model));
    body.insert(
        "prompt".into(),
        json!(if prompt.is_empty() {
            "Bring this picture to life"
        } else {
            &prompt
        }),
    );
    body.insert("size".into(), json!(size));
    body.insert("seconds".into(), json!(secs.to_string()));
    for k in ["steps", "seed"] {
        if let Some(v) = int_of(req, k) {
            body.insert(k.into(), json!(v));
        }
    }
    body.entry("steps").or_insert(json!(4)); // the device's own default (7) makes a 2 s clip take ~12 minutes instead of ~7
    let mut sources = vec![];
    // the start frame; the device ignores any end frame (0.1.38), so none is sent
    if !s("image").is_empty() {
        let (ctype, data) = decode_uri(s("image"))?;
        sources.push(save(
            ctx,
            &format!("{}-frame0.{}", ctx.id, super::image::sniff(&data).0),
            &data,
        )?);
        body.insert(
            "image".into(),
            json!(format!(
                "data:{ctype};base64,{}",
                base64::engine::general_purpose::STANDARD.encode(&data)
            )),
        );
    }
    let body = Value::Object(body);
    let (code, res) = http(ctx, "POST", &url, Some(body.clone()), 120, true).await?;
    if code >= 300 {
        return Err(JobError(explain(err(&res))));
    }
    let vid = res
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if vid.is_empty() {
        return Err(JobError("The Tiiny accepted the video but gave no id for it, so it can't be followed.".into()));
    }
    ctx.set_remote_task(Some(format!("DELETE {url}/{vid}"))); // deleting a clip stops its render (there's no /cancel)
    let mut lost_since: Option<std::time::Instant> = None; // no answer since: a render takes many minutes, so ride out a dropped link
    let (v, st) = loop {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let (code, v) = http(ctx, "GET", &format!("{url}/{vid}"), None, 30, true)
            .await
            .unwrap_or((0, json!({})));
        if ctx.cancelled() {
            return Err(JobError("Cancelled".into()));
        }
        if matches!(code, 401 | 403 | 404) {
            // the clip is gone (the Tiiny restarted, or it expired) or the key was refused: not "busy"
            return Err(JobError(format!(
                "The Tiiny no longer knows this video ({}). If it finished, it may be kept on the device.",
                err(&v)
            )));
        }
        let st = if code < 500 {
            v.get("status")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        } else {
            String::new()
        };
        if ["completed", "failed", "cancelled"].contains(&st.as_str()) {
            break (v, st);
        }
        lost_since = if st.is_empty() {
            Some(lost_since.unwrap_or_else(std::time::Instant::now))
        } else {
            None
        };
        if lost_since.is_some_and(|t| t.elapsed().as_secs() >= LOST_SECS) {
            return Err(JobError("Lost the Tiiny while it rendered this video. If it finished, it's kept on the device for an hour.".into()));
        }
        if lost_since.is_some() {
            ctx.update(json!({"message": "No answer from the Tiiny yet (it's usually just busy). Still waiting…"}));
            continue;
        }
        ctx.update(json!({"progress": v.get("progress").and_then(Value::as_f64).unwrap_or(0.0) as i64,
                          "message": if st == "in_progress" { "Rendering" } else { "Waiting in the device queue" }}));
    };
    ctx.set_remote_task(None);
    if st != "completed" {
        let e = v
            .get("error")
            .filter(|e| !e.is_null())
            .cloned()
            .unwrap_or(json!(st));
        return Err(JobError(explain(err(&e))));
    }
    ctx.update(json!({"message": "Downloading"}));
    let mut got = (0, Err(json!("Device unreachable")));
    for _ in 0..5 {
        // the same: a dropped link shouldn't lose a finished clip
        got = match http_bytes(ctx, "GET", &format!("{url}/{vid}/content"), None, 600, true).await {
            Ok(r) => r,
            Err(e) => (0, Err(json!(e.0))),
        };
        if got.0 == 200 || (got.0 > 0 && got.0 < 500) || ctx.cancelled() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    let (code, data) = got;
    let data = match data {
        Ok(d) if code == 200 && !d.is_empty() => d,
        Ok(_) => return Err(JobError("Couldn't fetch the video: empty".into())),
        Err(e) => return Err(JobError(format!("Couldn't fetch the video: {}", err(&e)))),
    };
    let out = save(ctx, &format!("{}-video.mp4", ctx.id), &data)?;
    let _ = http(ctx, "DELETE", &format!("{url}/{vid}"), None, 30, true).await; // the device keeps it an hour otherwise
    let frames = v.get("frames").and_then(Value::as_f64).unwrap_or(0.0);
    let fps = v
        .get("fps")
        .and_then(Value::as_f64)
        .filter(|f| *f > 0.0)
        .unwrap_or(24.0);
    let duration = if frames > 0.0 {
        (frames / fps * 10.0).round() / 10.0
    } else {
        secs as f64
    };
    let mut params = json!({"model": model, "prompt": prompt, "size": size, "seconds": secs});
    for k in ["steps", "seed"] {
        if let Some(x) = body.get(k) {
            params[k] = x.clone();
        }
    }
    Ok(
        json!({"kind": "video", "params": params, "sources": sources, "outputs": [out], "duration": duration}),
    )
}

#[cfg(test)]
mod tests {
    use super::explain;

    #[test]
    fn explains_the_npu_room_error() {
        let raw = "Weight installation failed at 0x7f00: status=UNINITIALIZED(1)";
        let e = explain(raw.into());
        assert!(e.starts_with("The Tiiny couldn't fit the video model"));
        assert!(e.ends_with(&format!("(Tiiny: {raw})")));
        let t = explain("video generation exceeded the gateway time limit".into());
        assert!(t.starts_with("The Tiiny stopped this video at its 1-hour limit"));
        assert_eq!(
            explain("prompt must be a non-empty string".into()),
            "prompt must be a non-empty string"
        );
    }
}
