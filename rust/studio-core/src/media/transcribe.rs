//! Speech to text through the gateway's /v1/audio/transcriptions (media.py run_transcribe).

use serde_json::{json, Value};

use super::audio::{asr_parts, strip, to_asr_wav};
use super::{decode_uri, err, http_bytes, json_reply, multipart, save};
use crate::jobs::{Ctx, JobError, JobResult};

pub async fn run(ctx: Ctx) -> JobResult {
    let req = &ctx.req;
    let (ctype, audio) = decode_uri(req.get("audio").and_then(Value::as_str).unwrap_or(""))?;
    let asr_audio = if ctype.contains("wav") {
        to_asr_wav(&audio).map_err(|e| JobError(e.to_string()))?
    } else {
        None
    };
    let ext = match ctype.as_str() {
        "audio/mpeg" => "mp3",
        "audio/mp4" => "m4a",
        "audio/webm" => "webm",
        "audio/ogg" => "ogg",
        "audio/flac" => "flac",
        _ => "wav", // incl. audio/wav and audio/x-wav
    };
    let mut fields = vec![
        ("model", ctx.model.clone()),
        ("response_format", "json".to_string()),
    ];
    let language = strip(req.get("language").and_then(Value::as_str).unwrap_or("")).to_string();
    if !language.is_empty() {
        fields.push(("language", language.clone()));
    }
    // long recordings go in parts: the Tiiny's speech-to-text fails past ~90 s
    let parts: Vec<(String, Vec<u8>, String)> = match &asr_audio {
        Some(a) if !a.is_empty() => asr_parts(a)
            .into_iter()
            .map(|p| ("audio.wav".to_string(), p, "audio/wav".to_string()))
            .collect(),
        _ => vec![(format!("audio.{ext}"), audio.clone(), ctype.clone())],
    };
    let url = format!("{}/v1/audio/transcriptions", ctx.target.url);
    let mut texts = vec![];
    for (i, (name, data, kind)) in parts.iter().enumerate() {
        ctx.update(json!({"message": if parts.len() > 1 { format!("Transcribing, part {} of {}", i + 1, parts.len()) } else { "Transcribing".to_string() },
                          "progress": i * 100 / parts.len()}));
        let (body, mp) = multipart(&fields, &[("file", (name, data.as_slice(), kind.as_str()))]);
        let (code, res) = http_bytes(&ctx, "POST", &url, Some((body, mp)), 3600, true).await?;
        let (code, res) = json_reply(code, res)?;
        if code != 200 || res.get("text").is_none() {
            return Err(JobError(err(&res)));
        }
        texts.push(res["text"].as_str().unwrap_or("").trim().to_string());
        if ctx.cancelled() {
            return Err(JobError("Cancelled".into()));
        }
    }
    let text = texts
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let src = if req.get("ephemeral").is_some_and(crate::device::truthy) {
        None
    } else {
        Some(save(&ctx, &format!("{}-audio.{ext}", ctx.id), &audio)?)
    };
    Ok(
        json!({"kind": "transcript", "params": {"model": ctx.model, "language": if language.is_empty() { Value::Null } else { json!(language) }, "name": req.get("name")},
        "sources": src.into_iter().collect::<Vec<_>>(), "outputs": [], "text": text}),
    )
}
