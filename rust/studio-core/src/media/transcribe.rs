//! Speech to text through the gateway's /v1/audio/transcriptions (media.py run_transcribe).

use serde_json::{json, Value};

use super::audio::{asr_parts, strip, to_asr_wav, wav_seconds};
use super::{decode_uri, err, http_bytes, json_reply, multipart, save};
use crate::jobs::{Ctx, JobError, JobResult};

pub async fn run(ctx: Ctx) -> JobResult {
    let req = &ctx.req;
    let (ctype, audio) = decode_uri(req.get("audio").and_then(Value::as_str).unwrap_or(""))?;
    // The Tiiny's speech-to-text takes WAV: other formats (FLAC, MP3...) can hang with no answer, and the long-audio
    // cut below needs the samples. The page converts to WAV before it sends; the studio has no decoder of its own,
    // so say so here instead of passing the file on and waiting for the device to give up.
    let is_wav = ctype.contains("wav") || (audio.starts_with(b"RIFF") && audio.get(8..12) == Some(b"WAVE"));
    if !is_wav {
        return Err(JobError(format!(
            "Transcription needs a WAV file, and this one is {ctype}. Convert it to WAV (16 kHz mono is best) and try again."
        )));
    }
    // a WAV the reader can't make sense of goes to the device as it is
    let asr_audio = to_asr_wav(&audio).unwrap_or(None);
    let ext = "wav";
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
        _ => vec![("audio.wav".to_string(), audio.clone(), "audio/wav".to_string())],
    };
    let url = format!("{}/v1/audio/transcriptions", ctx.target.url);
    let mut texts = vec![];
    for (i, (name, data, kind)) in parts.iter().enumerate() {
        ctx.update(json!({"message": if parts.len() > 1 { format!("Transcribing, part {} of {}", i + 1, parts.len()) } else { "Transcribing".to_string() },
                          "progress": i * 100 / parts.len()}));
        let (body, mp) = multipart(&fields, &[("file", (name, data.as_slice(), kind.as_str()))]);
        // a few times the clip's length, not an hour: the device may never answer
        let secs = wav_seconds(data).ok().flatten().unwrap_or(600.0);
        let timeout = (120.0 + secs * 3.0).min(900.0) as u64;
        let (code, res) = cancellable(&ctx, http_bytes(&ctx, "POST", &url, Some((body, mp)), timeout, true)).await??;
        let (code, res) = json_reply(code, res)?;
        if code != 200 || res.get("text").is_none() {
            return Err(JobError(err(&res)));
        }
        texts.push(res["text"].as_str().unwrap_or("").trim().to_string());
        if i + 1 < parts.len() && ctx.cancelled() {
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

/// Wait for `work`, but give up (dropping the request) when the job is cancelled: the device may hold a request for
/// a long time and a transcription has no remote task to stop.
async fn cancellable<T>(ctx: &Ctx, work: impl std::future::Future<Output = T>) -> Result<T, JobError> {
    tokio::select! {
        r = work => Ok(r),
        () = async {
            while !ctx.cancelled() {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        } => Err(JobError("Cancelled".into())),
    }
}
