//! Create (or, with source images, edit) through the gateway's /v1/images/generations (media.py run_image). With
//! mode "refs" the images are only references for a new picture (Qwen-Image composes freely from them, at any size).

use base64::Engine;
use serde_json::{json, Map, Value};

use super::{decode_uri, err, save};
use crate::jobs::{Ctx, JobError, JobResult};

const IMAGE_FIELDS: [&str; 8] = ["model", "prompt", "negative_prompt", "n", "size", "seed", "steps", "guidance_scale"];

/// Seconds between warm-up retries (STUDIO_WARMUP_WAIT, default 10; tests shorten it).
fn warmup_wait() -> std::time::Duration {
    let secs = std::env::var("STUDIO_WARMUP_WAIT").ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(10.0);
    std::time::Duration::from_secs_f64(secs.max(0.0))
}

/// int() of a request number the UI may send as a number or a string.
fn as_int(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)).or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

pub async fn run(ctx: Ctx) -> JobResult {
    let req = &ctx.req;
    let mut body = Map::new();
    for k in IMAGE_FIELDS {
        match req.get(k) {
            None | Some(Value::Null) => {}
            Some(Value::String(s)) if s.is_empty() => {}
            Some(v) => {
                body.insert(k.into(), v.clone());
            }
        }
    }
    body.insert("response_format".into(), json!("b64_json"));
    let sources: Vec<String> = req.get("images").and_then(Value::as_array).into_iter().flatten().filter_map(|s| s.as_str().map(String::from)).collect();
    if sources.len() == 1 {
        body.insert("image".into(), json!(sources[0]));
    } else if !sources.is_empty() {
        body.insert("images".into(), json!(sources)); // several references (Qwen-Image takes up to 10) go in "images", an array
    }
    let refs = !sources.is_empty() && req.get("mode").and_then(Value::as_str) == Some("refs");
    // The device only makes one image per request ("batch_size other than 1 is not supported"), so several
    // images are separate requests with consecutive seeds, grouped into one result.
    let count = body.remove("n").as_ref().and_then(as_int).filter(|n| *n != 0).unwrap_or(1).clamp(1, 4);
    body.insert("n".into(), json!(1));
    let seed0 = body.get("seed").cloned();
    let mut data: Vec<Value> = vec![];
    for i in 0..count {
        if ctx.cancelled() {
            return Err(JobError("Cancelled".into()));
        }
        let verb = if refs { "Composing" } else if sources.is_empty() { "Generating" } else { "Editing" };
        let message = if count == 1 { verb.to_string() } else { format!("{verb} image {} of {count}", i + 1) };
        let progress = if count > 1 { ((i as f64) / (count as f64) * 100.0).round() as i64 } else { 0 };
        ctx.update(json!({"message": message, "progress": progress}));
        if let Some(s) = seed0.as_ref().and_then(as_int) {
            body.insert("seed".into(), json!(s + i));
        }
        let (mut code, mut one) = ctx.call("POST", "/v1/images/generations", Some(Value::Object(body.clone())), 3600).await;
        // A model that just reported "running" may not be ready yet: its first request answers "xh2a backend
        // fail" (seen with Qwen-Image right after a load). Give it up to a minute before giving up.
        for _ in 0..6 {
            if code == 200 || !data.is_empty() || !err(&one).contains("backend fail") || ctx.cancelled() {
                break;
            }
            ctx.update(json!({"message": "Waiting for the model to warm up"}));
            tokio::time::sleep(warmup_wait()).await;
            (code, one) = ctx.call("POST", "/v1/images/generations", Some(Value::Object(body.clone())), 3600).await;
        }
        let got = one.get("data").and_then(Value::as_array).filter(|d| !d.is_empty()).cloned();
        match got {
            Some(d) if code == 200 => data.extend(d),
            _ if !data.is_empty() => break, // keep the images that did come out
            _ => return Err(JobError(err(&one))),
        }
    }
    body.insert("n".into(), json!(count));
    body.insert("seed".into(), seed0.unwrap_or(Value::Null));
    let mut src_names = vec![];
    for (i, uri) in sources.iter().enumerate() {
        let (ctype, bytes) = decode_uri(uri)?;
        let ext = match ctype.as_str() {
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            _ => "png",
        };
        src_names.push(save(&ctx, &format!("{}-src{i}.{ext}", ctx.id), &bytes));
    }
    let mut outputs = vec![];
    for (i, d) in data.iter().enumerate() {
        if let Some(b64) = d.get("b64_json").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            let bytes = base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| JobError(format!("Error: {e}")))?;
            outputs.push(save(&ctx, &format!("{}-{i}.png", ctx.id), &bytes));
        }
    }
    let params: Map<String, Value> = body.into_iter().filter(|(k, _)| k != "image" && k != "response_format").collect();
    Ok(json!({"kind": "image", "mode": if refs { "refs" } else if sources.is_empty() { "create" } else { "edit" }, "params": params, "sources": src_names, "outputs": outputs}))
}
