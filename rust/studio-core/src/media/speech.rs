//! Voice-clone speech through the gateway's /v1/audio/speech (media.py run_speech); long text is voiced a few
//! sentences at a time and stitched together.

use base64::Engine;
use serde_json::{json, Map, Value};

use super::audio::{join_wavs, split_for_speech, strip, wav_seconds};
use super::{decode_uri, ensure_running, err, http_bytes, json_body, py_round, save};
use crate::jobs::{Ctx, JobError, JobResult};

fn text<'a>(req: &'a Value, k: &str) -> &'a str {
    req.get(k).and_then(Value::as_str).unwrap_or("")
}

pub async fn run(ctx: Ctx) -> JobResult {
    let req = &ctx.req;
    let said = strip(text(req, "text")).to_string();
    if said.is_empty() {
        return Err(JobError("Nothing to say".into()));
    }
    if !req.get("ref_audio").is_some_and(crate::device::truthy) {
        return Err(JobError("This model only speaks in a cloned voice: add a reference clip".into()));
    }
    if strip(text(req, "ref_text")).is_empty() {
        return Err(JobError("Add what's said in the reference clip (its transcript) \u{2014} voice cloning needs both".into()));
    }
    let (_, reference) = decode_uri(text(req, "ref_audio"))?;
    // The gateway's OpenAI-style /v1/audio/speech takes the clone fields at the top level and works on
    // every link (the USB cable blocks the TTS container's own port).
    let mut body = Map::new();
    body.insert("model".into(), json!(ctx.model));
    body.insert("input".into(), json!(said));
    body.insert("ref_audio".into(), json!(base64::engine::general_purpose::STANDARD.encode(&reference)));
    body.insert("response_format".into(), json!("wav"));
    let mut config = Map::new();
    for k in ["ref_text", "language", "instruct"] {
        if !strip(text(req, k)).is_empty() {
            let v = json!(strip(text(req, k)));
            body.insert(k.into(), v.clone());
            config.insert(k.into(), v);
        }
    }
    let model = ctx.model.clone();
    ensure_running(&ctx, &model).await?;
    // Long text is voiced a few sentences at a time (the model has a context limit) and stitched together.
    let chunks = split_for_speech(&said, 320);
    let url = format!("{}/v1/audio/speech", ctx.state.settings.current().url);
    let mut parts: Vec<Vec<u8>> = vec![];
    for (i, chunk) in chunks.iter().enumerate() {
        if ctx.cancelled() {
            return Err(JobError("Cancelled".into()));
        }
        let message = if chunks.len() == 1 { "Synthesising speech".to_string() } else { format!("Speaking part {} of {}", i + 1, chunks.len()) };
        let progress = if chunks.len() > 1 { py_round(i as f64 / chunks.len() as f64 * 100.0) } else { 0 };
        ctx.update(json!({"message": message, "progress": progress}));
        body.insert("input".into(), json!(chunk));
        let (code, res) = http_bytes(&ctx, "POST", &url, json_body(&Value::Object(body.clone())), 3600, true).await?;
        match res {
            Ok(wav) if code == 200 && wav.starts_with(b"RIFF") => parts.push(wav),
            Ok(other) => return Err(JobError(err(&json!(String::from_utf8_lossy(&other))))),
            Err(v) => return Err(JobError(err(&v))),
        }
    }
    let joined = join_wavs(&parts).map_err(|e| JobError(e.to_string()))?;
    let out = save(&ctx, &format!("{}-speech.wav", ctx.id), &joined);
    let src = save(&ctx, &format!("{}-ref.wav", ctx.id), &reference);
    let duration = wav_seconds(&joined).map_err(|e| JobError(e.to_string()))?;
    Ok(json!({"kind": "speech", "params": {"model": model, "text": said, "language": config.get("language"), "parts": chunks.len(),
        "instruct": config.get("instruct"), "voice": req.get("voice_name"), "ref_text": config.get("ref_text")},
        "sources": [src], "outputs": [out], "duration": duration}))
}
