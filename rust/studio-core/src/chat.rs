//! Chat with the loaded model, which can also use the studio: create or edit images, voice text, make loops
//! and songs. When a tool needs a model that doesn't fit next to the chat model, the chat model is unloaded,
//! the tool's model loaded, the work done, and the chat model loaded back before the reply goes on.
//! Port of chat.py: runs as a "chat" job in the shared device queue (it holds the device lock for the whole
//! turn, so the tools run inline). The reply streams into job["partial"], which the page polls.

use std::time::Duration;

use base64::Engine;
use serde_json::{json, Map, Value};

use crate::jobs::{now, Ctx, JobError, JobResult};
use crate::media::{err, make_room, put_back, running_ids};

const MAX_TOOL_ROUNDS: usize = 4;

pub const SYSTEM: &str = "You are the assistant in Tiiny Studio, a local media studio running on the user's own Tiiny AI device. \
Be direct and helpful. Use Markdown when it helps. When the user wants something made (an image, an edit \
of an image, speech, a music loop, a song), call the matching tool instead of describing it; write rich, \
specific prompts for it. Nothing is made unless you call a tool: never claim you made or changed something \
without calling the tool in this turn. The result appears in the chat and in the studio's gallery, so after the \
tool returns just tell the user briefly what you made. Making things can take a minute or more because models \
may be swapped on the device.";

fn tool_specs() -> Vec<(&'static str, Value)> {
    vec![
        (
            "create_image",
            json!({"description": "Create a new image from a detailed text-to-image prompt.",
            "parameters": {"type": "object", "properties": {
                "prompt": {"type": "string", "description": "Detailed visual description: subject, setting, style, lighting."},
                "shape": {"type": "string", "enum": ["square", "portrait", "landscape", "wide"], "description": "Aspect ratio."}},
                "required": ["prompt"]}}),
        ),
        (
            "edit_image",
            json!({"description": "Edit the most recent image in the conversation (attached by the user or made earlier) with a plain-language instruction.",
            "parameters": {"type": "object", "properties": {
                "instruction": {"type": "string", "description": "What to change, and what to keep the same."}},
                "required": ["instruction"]}}),
        ),
        (
            "speak",
            json!({"description": "Read text aloud in the user's saved voice and return the audio.",
            "parameters": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}}),
        ),
        (
            "make_loop",
            json!({"description": "Make a short instrument loop for music production.",
            "parameters": {"type": "object", "properties": {
                "tags": {"type": "string", "description": "Comma-separated tags: one instrument first, then character, phrase and effect tags."},
                "bpm": {"type": "integer", "minimum": 100, "maximum": 150}},
                "required": ["tags"]}}),
        ),
        (
            "make_song",
            json!({"description": "Make a short song preview (about 8 seconds) that the user can continue to a full song in the Audio workspace.",
            "parameters": {"type": "object", "properties": {
                "caption": {"type": "string", "description": "Genre, mood, instruments, voice."},
                "lyrics": {"type": "string", "description": "Lyrics with [verse] and [chorus] tags on their own lines. Empty for an instrumental."}},
                "required": ["caption"]}}),
        ),
    ]
}

fn shape(name: &str) -> Option<&'static str> {
    Some(match name {
        "square" => "1024x1024",
        "portrait" => "768x1024",
        "landscape" => "1024x768",
        "wide" => "1024x576",
        _ => return None,
    })
}

fn short(mid: &str) -> &str {
    mid.rsplit('/').next().unwrap_or(mid)
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

// ---------- tools ----------
fn last_image(messages: &[Value]) -> Option<String> {
    for msg in messages.iter().rev() {
        if let Some(parts) = msg.get("content").and_then(Value::as_array) {
            for part in parts.iter().rev() {
                if s(part, "type") == "image_url" {
                    return part
                        .pointer("/image_url/url")
                        .and_then(Value::as_str)
                        .map(String::from);
                }
            }
        }
    }
    None
}

fn file_uri(ctx: &Ctx, name: &str) -> Result<String, JobError> {
    let path = ctx.state.store.images.join(name);
    if !crate::store::safe_file(name) || !path.is_file() {
        return Err(JobError("That image isn't in the gallery any more".into()));
    }
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    let bytes = std::fs::read(&path).map_err(|e| JobError(e.to_string()))?;
    Ok(format!(
        "data:image/{};base64,{}",
        if ext == "jpg" { "jpeg" } else { &ext },
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

fn as_int(v: Option<&Value>) -> Option<i64> {
    let v = v?;
    v.as_i64()
        .or_else(|| v.as_f64().map(|f| f as i64))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        .filter(|n| *n != 0)
}

/// Run one tool inline (we already hold the device lock); returns (gallery entry, text for the model).
async fn run_tool(
    ctx: &Ctx,
    name: &str,
    args: &Value,
    messages: &[Value],
) -> Result<(Option<Value>, String), JobError> {
    let req = &ctx.req;
    let models = req.get("tool_models").cloned().unwrap_or(json!({}));
    let prefs = req.get("prefs").cloned().unwrap_or(json!({}));
    let model_for = |k: &str| {
        models
            .get(k)
            .and_then(Value::as_str)
            .filter(|m| !m.is_empty())
            .map(String::from)
    };
    let (kind, model, sub_req) = match name {
        "create_image" => {
            let size = shape(s(args, "shape"))
                .map(String::from)
                .or_else(|| Some(s(&prefs, "size").to_string()).filter(|x| !x.is_empty()))
                .unwrap_or_else(|| "1024x1024".into());
            (
                "image",
                model_for("image"),
                json!({"prompt": s(args, "prompt"), "size": size}),
            )
        }
        "edit_image" => {
            // newest first: made earlier in this turn, then the latest in the conversation (the page sends it,
            // since images the assistant made aren't part of the messages), then the user's attachments
            let src = ctx
                .get("turn_image")
                .and_then(|v| v.as_str().map(String::from))
                .filter(|x| !x.is_empty())
                .or_else(|| Some(s(req, "last_image").to_string()).filter(|x| !x.is_empty()))
                .or_else(|| last_image(messages));
            let Some(mut src) = src else {
                return Ok((
                    None,
                    "There's no image in the conversation to edit. Ask the user to attach one."
                        .into(),
                ));
            };
            if src.starts_with("/images/") {
                src = file_uri(ctx, src.rsplit('/').next().unwrap_or(""))?;
            }
            (
                "image",
                model_for("edit"),
                json!({"prompt": s(args, "instruction"), "images": [src]}),
            )
        }
        "speak" => {
            let voice = req
                .get("voice")
                .filter(|v| crate::device::truthy(v))
                .cloned();
            let Some(voice) = voice else {
                return Ok((None, "No saved voice to speak with. Tell the user to save a voice in the Audio workspace first.".into()));
            };
            (
                "speech",
                model_for("speech"),
                json!({"text": s(args, "text"), "ref_audio": voice["uri"], "ref_text": voice["text"], "voice_name": voice.get("name")}),
            )
        }
        "make_loop" => {
            let bpm = as_int(args.get("bpm")).unwrap_or(120);
            let seconds = ((8.0 * 4.0 * 60.0 / bpm as f64) * 100.0).round() / 100.0;
            (
                "loop",
                model_for("loop"),
                json!({"prompt": format!("{}, 8 bars, {bpm} BPM", s(args, "tags")), "seconds_total": seconds, "bpm": bpm, "bars": 8}),
            )
        }
        "make_song" => {
            let lyrics = s(args, "lyrics").trim().to_string();
            let gen = if lyrics.is_empty() { "bgm" } else { "mixed" };
            (
                "song",
                model_for("song"),
                json!({"caption": s(args, "caption"), "lyrics": lyrics, "generation_type": gen, "preview": true}),
            )
        }
        _ => return Ok((None, format!("Unknown tool {name}"))),
    };
    let Some(model) = model else {
        return Ok((
            None,
            format!(
                "No model for {} is installed on the device.",
                name.replace('_', " ")
            ),
        ));
    };
    if !ctx.state.settings.swap_allowed("chat") && !running_ids(ctx).await.contains(&model) {
        return Ok((None, format!("{0} isn't loaded, and the user has turned off model swapping for chat in Settings. Tell the \
user to load {0} from the Models panel (or turn swapping on), then ask again.", short(&model))));
    }
    let mut sub_req = sub_req;
    sub_req["model"] = json!(model);
    ctx.update(json!({"tool": {"name": name, "model": model}}));
    let unloaded = make_room(ctx, &model).await?;
    let result = async {
        let sub = ctx.sub(&model, sub_req).own_files(); // its result goes to the gallery at once; a failure takes back only its files
        let started = now();
        let message = match kind {
            "image" => "Making the image",
            "speech" => "Speaking",
            "loop" => "Making the loop",
            _ => "Making the song",
        };
        ctx.update(json!({"message": message}));
        if ctx.cancelled() {
            return Err(JobError("Cancelled".into()));
        }
        let run =
            crate::media::kind(kind).ok_or_else(|| JobError(format!("unknown job kind {kind}")))?;
        let mut entry = match run(sub.clone()).await {
            Ok(e) => e,
            Err(e) => {
                sub.discard_saved();
                return Err(e);
            }
        };
        let e = entry.as_object_mut().unwrap();
        crate::jobs::stamp(e, &sub.id, kind, started);
        e.insert("via".into(), json!("chat"));
        crate::media::tags::embed(
            &ctx.state.store.images,
            &entry,
            ctx.state.settings.tag_prompts(),
        ); // the prompt inside the files (#58)
        if let Err(e) = ctx.state.store.add_history(entry.clone()) {
            sub.discard_saved();
            return Err(JobError(format!("Couldn't save the result: {e}")));
        }
        if kind == "image" {
            if let Some(first) = entry.pointer("/outputs/0").and_then(Value::as_str) {
                ctx.update(json!({"turn_image": format!("/images/{first}")}));
            }
        }
        Ok(entry)
    }
    .await;
    ctx.remove(&["tool_message", "tool_progress"]); // so "Loading … back" shows instead of the tool's last step
    let back = put_back(ctx, &model, &unloaded).await;
    let entry = result?;
    back?;
    let what = match kind {
        "image" => "image",
        "speech" => "audio",
        "loop" => "loop",
        _ => "song preview",
    };
    Ok((
        Some(entry),
        format!(
            "Done: the {what} is shown to the user and saved in the gallery (made with {}).",
            short(&model)
        ),
    ))
}

// ---------- the turn ----------
struct Partial {
    content: String,
    reasoning: String,
    done: String,
    done_reasoning: String,
}

impl Partial {
    fn json(&self) -> Value {
        json!({"content": self.content, "reasoning": self.reasoning, "done": self.done, "done_reasoning": self.done_reasoning})
    }
}

struct Call {
    id: String,
    name: String,
    args: String,
}

/// How long the device may stay silent (no answer yet, or no new chunk) before the reply is given up. Per read, like
/// chat.py's socket timeout: a long answer from a slow model is fine as long as it keeps coming.
const IDLE: Duration = Duration::from_secs(900);
/// How often a silent wait looks at Stop.
const POLL: Duration = Duration::from_millis(500);

/// What one streamed completion said.
struct Streamed {
    content: String,
    calls: Vec<Call>,
    finish: Value,
    /// The last `usage` object the stream carried (token counts), or null.
    usage: Value,
}

/// The stream so far, fed line by line (so chunks may split anywhere).
#[derive(Default)]
struct Acc {
    content: String,
    reasoning: String,
    calls: std::collections::BTreeMap<i64, Call>,
    finish: Value,
    usage: Value,
    done: bool,
}

impl Acc {
    /// One line of the SSE body. Ok(true) when it carried data.
    fn feed(&mut self, raw: &[u8]) -> Result<bool, JobError> {
        let line = String::from_utf8_lossy(raw).trim().to_string();
        if line == "data: [DONE]" || line == "data:[DONE]" {
            self.done = true;
            return Ok(false);
        }
        if !line.starts_with("data:") {
            return Ok(false);
        }
        let d: Value =
            serde_json::from_str(&line[5..]).map_err(|e| JobError(format!("ValueError: {e}")))?;
        if d.get("error").is_some_and(crate::device::truthy) {
            // the gateway reports a failure inside the stream (the text so far stays on the job)
            return Err(JobError(format!("The Tiiny's reply failed: {}", err(&d))));
        }
        if let Some(u) = d.get("usage").filter(|u| u.is_object()) {
            self.usage = u.clone();
        }
        let ch = d.pointer("/choices/0").cloned().unwrap_or(json!({}));
        let delta = ch.get("delta").cloned().unwrap_or(json!({}));
        if let Some(r) = delta
            .get("reasoning_content")
            .and_then(Value::as_str)
            .filter(|x| !x.is_empty())
        {
            self.reasoning.push_str(r);
        }
        if let Some(c) = delta
            .get("content")
            .and_then(Value::as_str)
            .filter(|x| !x.is_empty())
        {
            self.content.push_str(c);
        }
        for tc in delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let slot = self
                .calls
                .entry(tc.get("index").and_then(Value::as_i64).unwrap_or(0))
                .or_insert(Call {
                    id: String::new(),
                    name: String::new(),
                    args: String::new(),
                });
            if let Some(id) = tc
                .get("id")
                .and_then(Value::as_str)
                .filter(|x| !x.is_empty())
            {
                slot.id = id.to_string();
            }
            slot.name.push_str(
                tc.pointer("/function/name")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
            );
            slot.args.push_str(
                tc.pointer("/function/arguments")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
            );
        }
        if let Some(f) = ch.get("finish_reason").filter(|f| crate::device::truthy(f)) {
            self.finish = f.clone();
        }
        Ok(true)
    }
}

/// Wait for `fut`, but look at Stop every POLL and give up after `idle` of silence. Ok(None): the job was stopped.
async fn wait_for<T>(
    ctx: &Ctx,
    idle: Duration,
    fut: impl std::future::Future<Output = T>,
) -> Result<Option<T>, JobError> {
    tokio::pin!(fut);
    let t0 = std::time::Instant::now();
    loop {
        if ctx.cancelled() {
            return Ok(None);
        }
        let Some(left) = idle.checked_sub(t0.elapsed()) else {
            let how = if idle.as_secs() >= 60 {
                format!("{} minutes", idle.as_secs() / 60)
            } else {
                format!("{} seconds", idle.as_secs())
            };
            return Err(JobError(format!(
                "The Tiiny sent nothing for {how}, so the reply was given up."
            )));
        };
        if let Ok(v) = tokio::time::timeout(left.min(POLL), &mut fut).await {
            return Ok(Some(v));
        }
    }
}

/// Tell the Tiiny to stop generating this model's reply (a dropped connection doesn't stop it).
async fn interrupt(ctx: &Ctx, model: &str) {
    ctx.call(
        "POST",
        &format!("/api/v1/models/{}/interrupt", crate::device::quote(model)),
        Some(json!({})),
        15,
    )
    .await;
}

/// POST a streaming chat completion; fill job["partial"] as it arrives. The device may take `idle` for each step
/// (first answer, every chunk), not for the whole reply. When the job is stopped the stream is dropped and the
/// device told to interrupt; what came so far is returned.
async fn stream_completion(
    ctx: &Ctx,
    partial: &mut Partial,
    body: &Value,
    idle: Duration,
) -> Result<Streamed, JobError> {
    let cur = &ctx.target;
    let model = s(body, "model").to_string();
    let send = ctx
        .state
        .http
        .post(format!("{}/v1/chat/completions", cur.url))
        .bearer_auth(&cur.key)
        .json(body)
        .send();
    let sent = wait_for(ctx, idle, send).await?;
    let mut acc = Acc::default();
    let finished = |acc: Acc| Streamed {
        content: acc.content,
        calls: acc.calls.into_values().collect(),
        finish: acc.finish,
        usage: acc.usage,
    };
    let Some(res) = sent else {
        interrupt(ctx, &model).await; // the request may already be with the model
        return Ok(finished(acc));
    };
    let res = res.map_err(|e| JobError(format!("Device unreachable: {e}")))?;
    let code = res.status().as_u16();
    if code >= 400 {
        let raw = res.bytes().await.unwrap_or_default();
        return Err(JobError(if code == 504 {
            "The Tiiny gave up on this chat: a request has about 10 minutes there, and a long conversation or a long attached text on a big model can take longer. Start a shorter chat, attach less, or pick a faster model.".to_string()
        } else {
            match serde_json::from_slice::<Value>(&raw) {
                Ok(v) => err(&v),
                Err(_) => format!("The device answered {code}"),
            }
        }));
    }
    let mut res = res;
    let mut buf: Vec<u8> = vec![];
    loop {
        let Some(chunk) = wait_for(ctx, idle, res.chunk()).await? else {
            interrupt(ctx, &model).await;
            return Ok(finished(acc));
        };
        let chunk = chunk.map_err(|e| JobError(format!("Device unreachable: {e}")))?;
        let Some(chunk) = chunk else { break };
        buf.extend_from_slice(&chunk);
        while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
            let raw: Vec<u8> = buf.drain(..=nl).collect();
            if ctx.cancelled() {
                interrupt(ctx, &model).await;
                return Ok(finished(acc));
            }
            if acc.feed(&raw)? {
                partial.content = format!("{}{}", partial.done, acc.content);
                partial.reasoning = format!("{}{}", partial.done_reasoning, acc.reasoning);
                ctx.update(json!({"partial": partial.json()}));
            }
        }
    }
    if !buf.is_empty() {
        acc.feed(&buf)?; // a last line without its newline
    }
    if !acc.done && !crate::device::truthy(&acc.finish) {
        // neither "[DONE]" nor a finish reason: the connection ended early (the text so far stays on the job)
        return Err(JobError(
            "The Tiiny stopped sending the reply before it was finished. Try again.".into(),
        ));
    }
    Ok(finished(acc))
}

/// The real id behind "default" (the device's alias for the loaded model), so it can be reloaded after a swap, and
/// its thinking object (None when the device doesn't say).
async fn chat_model_id(ctx: &Ctx, wanted: &str) -> Result<(String, Option<Value>), JobError> {
    let (code, res) = ctx.call("GET", "/v1/models", None, 15).await;
    if code == 200 {
        for item in res
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let aliases = item
                .get("aliases")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if s(item, "id") == wanted || aliases.iter().any(|a| a.as_str() == Some(wanted)) {
                return Ok((
                    s(item, "id").to_string(),
                    item.get("thinking").filter(|t| t.is_object()).cloned(),
                ));
            }
        }
    }
    if !wanted.is_empty() && wanted != "default" {
        return Ok((wanted.to_string(), None));
    }
    Err(JobError(
        "No chat model is loaded. Load one from the Models panel.".into(),
    ))
}

/// The request fields that switch a model's thinking on or off, by the mode its /v1/models entry gives.
fn set_thinking(body: &mut Value, mode: &str, thinking: bool) {
    if mode == "always" {
        // reasons whatever it's sent
        body["reasoning_effort"] = json!(if thinking { "medium" } else { "low" });
    } else {
        // "switch", and "none" too (off): some models report they can't think, yet reason until max_tokens runs
        // out unless told not to (media/assist.rs has the same workaround). The flag is ignored where it means nothing.
        body["chat_template_kwargs"] = json!({"enable_thinking": mode == "switch" && thinking});
    }
}

pub async fn run(ctx: Ctx) -> JobResult {
    let req = ctx.req.clone();
    let messages: Vec<Value> = req
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|m| m.is_object())
        .cloned()
        .collect();
    if messages.is_empty() {
        return Err(JobError("Nothing to send".into()));
    }
    // attached documents are untrusted: delimited, screened, and flagged to the user (never silently dropped)
    let attached = crate::safety::has_documents(&messages);
    let pictures = crate::safety::has_images(&messages);
    let (messages, warnings) = crate::safety::guard_messages(&messages);
    if !warnings.is_empty() {
        ctx.update(json!({"warnings": warnings}));
    }
    let wanted = Some(s(&req, "model"))
        .filter(|m| !m.is_empty())
        .unwrap_or("default");
    let (model, thinking_info) = chat_model_id(&ctx, wanted).await?;
    let mode = crate::media::thinking_mode(&model, thinking_info.as_ref());
    ctx.update(json!({"model": model}));
    let wanted_tools: Vec<&str> = req
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let tools: Vec<Value> = tool_specs()
        .into_iter()
        // flagged content: no tools this turn (the warning shown to the user says so)
        .filter(|(n, _)| wanted_tools.contains(n) && warnings.is_empty())
        .map(|(n, spec)| {
            let mut f = Map::new();
            f.insert("name".into(), json!(n));
            f.extend(spec.as_object().cloned().unwrap_or_default());
            json!({"type": "function", "function": f})
        })
        .collect();
    let extra = s(&req, "system").trim();
    let mut rule = String::new();
    if attached {
        rule.push_str(&format!(" {}", crate::safety::RULE));
    }
    if pictures {
        rule.push_str(&format!(" {}", crate::safety::IMAGE_RULE));
    }
    let system = if extra.is_empty() {
        format!("{SYSTEM}{rule}")
    } else {
        format!("{SYSTEM}{rule} {extra}")
    };
    let mut convo = vec![json!({"role": "system", "content": system})];
    convo.extend(messages.iter().cloned());
    let mut partial = Partial {
        content: String::new(),
        reasoning: String::new(),
        done: String::new(),
        done_reasoning: String::new(),
    };
    ctx.update(json!({"partial": partial.json()}));
    let thinking = req.get("thinking").is_some_and(crate::device::truthy);
    let think = mode == "always" || (mode == "switch" && thinking);
    let max_tokens = as_int(req.get("max_tokens")).unwrap_or(4096);
    let mut artifacts: Vec<Value> = vec![];
    let mut finish = Value::Null;
    let mut usage = Value::Null;
    for round in 0..=MAX_TOOL_ROUNDS {
        let mut body =
            json!({"model": model, "messages": convo, "stream": true, "max_tokens": max_tokens});
        set_thinking(&mut body, mode, thinking);
        if !tools.is_empty() && round < MAX_TOOL_ROUNDS {
            body["tools"] = json!(tools);
        }
        ctx.update(json!({"message": if think { "Thinking" } else { "Replying" }}));
        if ctx.cancelled() {
            return Err(JobError("Stopped".into())); // e.g. Stop pressed while a tool ran: no new request to the device
        }
        let Streamed {
            content,
            calls,
            finish: f,
            usage: u,
            ..
        } = stream_completion(&ctx, &mut partial, &body, IDLE).await?;
        finish = f;
        if u.is_object() {
            usage = u;
        }
        if ctx.cancelled() {
            return Err(JobError("Stopped".into()));
        }
        if calls.is_empty() {
            break;
        }
        let call_id = |i: usize, c: &Call| {
            if c.id.is_empty() {
                format!("call_{i}")
            } else {
                c.id.clone()
            }
        };
        convo.push(json!({"role": "assistant", "content": content,
            "tool_calls": calls.iter().enumerate().map(|(i, c)| json!({"id": call_id(i, c), "type": "function",
                "function": {"name": c.name, "arguments": if c.args.is_empty() { "{}" } else { &c.args }}})).collect::<Vec<_>>()}));
        for (i, c) in calls.iter().enumerate() {
            let args: Value = serde_json::from_str(if c.args.is_empty() { "{}" } else { &c.args })
                .ok()
                .filter(Value::is_object)
                .unwrap_or(json!({}));
            let (entry, result) = match run_tool(&ctx, &c.name, &args, &messages).await {
                Ok(r) => r,
                Err(e) if ctx.cancelled() => return Err(e),
                Err(e) => (None, format!("That didn't work: {e}")),
            };
            if ctx.cancelled() {
                return Err(JobError("Stopped".into())); // the tool has run (its result is kept), but nothing more is sent
            }
            if let Some(entry) = entry {
                let prompt = ["prompt", "instruction", "text", "tags", "caption"]
                    .iter()
                    .map(|k| s(&args, k))
                    .find(|v| !v.is_empty())
                    .unwrap_or("");
                artifacts.push(json!({"kind": entry["kind"], "id": entry["id"], "outputs": entry.get("outputs").cloned().unwrap_or(json!([])),
                    "prompt": prompt, "duration": entry.get("duration").cloned().unwrap_or(Value::Null), "tool": c.name}));
                ctx.update(json!({"artifacts": artifacts}));
            }
            convo.push(json!({"role": "tool", "tool_call_id": call_id(i, c), "content": result}));
        }
        // keep what was said before the tool call on screen while the reply goes on
        let sep = if content.trim().is_empty() {
            ""
        } else {
            "\n\n"
        };
        partial.done = format!("{}{sep}", partial.content);
        partial.done_reasoning = partial.reasoning.clone();
        ctx.update(json!({"partial": partial.json()}));
        ctx.remove(&["tool"]);
    }
    if partial.content.trim().is_empty() && artifacts.is_empty() && finish == "length" {
        // an always-reasoning model (gpt-oss) spent max_tokens thinking
        return Err(JobError("The model reasoned until it ran out of room and gave no answer. Try again, or ask for less.".into()));
    }
    let mut entry = json!({"kind": "chat", "params": {"model": model}, "sources": [], "outputs": [], "text": partial.content.trim(),
                           "reasoning": partial.reasoning.trim(), "artifacts": artifacts, "finish": finish});
    if usage.is_object() {
        entry["usage"] = usage; // token counts of the last request, when the device reports them
    }
    if !warnings.is_empty() {
        entry["warnings"] = json!(warnings);
    }
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::Notify;

    fn state() -> crate::AppState {
        crate::AppState::new(crate::Config {
            port: 0,
            host: "127.0.0.1".into(),
            password: String::new(),
            lan_access: false,
            app: false,
            data_dir: std::env::temp_dir()
                .join(format!("studio-chat-{}", uuid::Uuid::new_v4().simple())),
        })
    }

    /// A job context that talks to `url`, outside the job table.
    fn ctx_for(url: &str) -> Ctx {
        let st = state();
        let mut ctx = st.jobs.test_ctx(&st);
        ctx.target.url = url.to_string();
        ctx.target.key = "k".into();
        ctx
    }

    enum Step {
        Send(&'static [u8]),
        Pause(u64),
        /// tell the test the bytes so far have been written
        Signal(Arc<Notify>),
        /// keep the connection open until the client goes away
        Hold,
    }

    /// A device whose chat answer is scripted byte by byte. Every request line is recorded; /interrupt answers at once.
    async fn scripted(status: u16, script: Vec<Step>) -> (String, Arc<Mutex<Vec<String>>>) {
        let hits: Arc<Mutex<Vec<String>>> = Arc::default();
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let script = Arc::new(Mutex::new(Some(script)));
        let log = hits.clone();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = l.accept().await.unwrap();
                sock.set_nodelay(true).unwrap();
                let (script, log) = (script.clone(), log.clone());
                tokio::spawn(async move {
                    let mut head = vec![];
                    let mut b = [0u8; 1];
                    while !head.ends_with(b"\r\n\r\n") {
                        if sock.read(&mut b).await.unwrap_or(0) == 0 {
                            return;
                        }
                        head.push(b[0]);
                    }
                    let head = String::from_utf8_lossy(&head).to_string();
                    let first = head.lines().next().unwrap_or("").to_string();
                    let len = head
                        .lines()
                        .find_map(|l| {
                            l.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap_or(0))
                        })
                        .unwrap_or(0usize);
                    let mut body = vec![0u8; len];
                    let _ = sock.read_exact(&mut body).await;
                    log.lock().unwrap().push(first.clone());
                    if first.contains("/interrupt") {
                        let _ = sock
                            .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}")
                            .await;
                        return;
                    }
                    let Some(script) = script.lock().unwrap().take() else {
                        return;
                    };
                    let head = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n"
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    for step in script {
                        match step {
                            Step::Send(bytes) => {
                                let _ = sock.write_all(bytes).await;
                                let _ = sock.flush().await;
                            }
                            Step::Pause(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
                            Step::Signal(n) => n.notify_one(),
                            Step::Hold => {
                                let _ = tokio::time::timeout(
                                    Duration::from_secs(10),
                                    sock.read(&mut b),
                                )
                                .await;
                            }
                        }
                    }
                });
            }
        });
        (url, hits)
    }

    fn partial() -> Partial {
        Partial {
            content: String::new(),
            reasoning: String::new(),
            done: String::new(),
            done_reasoning: String::new(),
        }
    }

    const HI: &[u8] = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n";

    async fn stream(ctx: &Ctx, p: &mut Partial, idle: Duration) -> Result<Streamed, JobError> {
        stream_completion(ctx, p, &json!({"model": "org/chat", "stream": true}), idle).await
    }

    #[tokio::test]
    async fn a_stream_split_anywhere_reads_the_same() {
        // the pieces cut a JSON line, the multi-byte "é", and the blank line between events
        let (url, _) = scripted(
            200,
            vec![
                Step::Send(b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello ca"),
                Step::Pause(20),
                Step::Send(b"f\xC3"),
                Step::Pause(20),
                Step::Send(b"\xA9\"}}]}\n"),
                Step::Pause(20),
                Step::Send(b"\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":3}}\n\n"),
                Step::Send(b"data: [DONE]"), // no newline at the very end
            ],
        )
        .await;
        let ctx = ctx_for(&url);
        let mut p = partial();
        let out = stream(&ctx, &mut p, IDLE).await.unwrap();
        assert_eq!(out.content, "Hello café");
        assert_eq!(out.finish, json!("stop"));
        assert_eq!(out.usage["prompt_tokens"], json!(7));
    }

    #[tokio::test]
    async fn a_stream_that_ends_early_is_an_error_and_keeps_the_text() {
        let (url, _) = scripted(200, vec![Step::Send(HI)]).await; // then the connection closes: no finish, no [DONE]
        let ctx = ctx_for(&url);
        let mut p = partial();
        let e = stream(&ctx, &mut p, IDLE).await.err().expect("an error");
        assert!(e.0.contains("stopped sending the reply"), "{e}");
        assert_eq!(p.content, "Hi");
    }

    #[tokio::test]
    async fn an_error_inside_the_stream_is_an_error_and_keeps_the_text() {
        let (url, _) = scripted(
            200,
            vec![
                Step::Send(HI),
                Step::Send(b"data: {\"error\":{\"message\":\"model crashed\"}}\n\n"),
                Step::Send(b"data: [DONE]\n\n"),
            ],
        )
        .await;
        let ctx = ctx_for(&url);
        let mut p = partial();
        let e = stream(&ctx, &mut p, IDLE).await.err().expect("an error");
        assert!(e.0.contains("model crashed"), "{e}");
        assert_eq!(p.content, "Hi");
    }

    #[tokio::test]
    async fn stop_in_the_middle_of_a_stream_drops_it_and_interrupts_the_model() {
        let said = Arc::new(Notify::new());
        let (url, hits) = scripted(
            200,
            vec![Step::Send(HI), Step::Signal(said.clone()), Step::Hold],
        )
        .await;
        let ctx = ctx_for(&url);
        let flag = ctx.cancel.clone();
        let run = tokio::spawn(async move {
            let mut p = partial();
            stream(&ctx, &mut p, IDLE).await.map(|o| o.content)
        });
        said.notified().await;
        flag.store(true, Ordering::Relaxed); // Stop, while the device is silent
                                             // Stop may land before or after the first chunk is read: either way the stream ends without an error
        let text = run.await.unwrap().unwrap();
        assert!(text == "Hi" || text.is_empty(), "{text}");
        assert!(hits
            .lock()
            .unwrap()
            .iter()
            .any(|h| h.contains("/api/v1/models/org%2Fchat/interrupt")));
    }

    #[tokio::test]
    async fn a_silent_device_is_given_up_on_per_read_not_for_the_whole_reply() {
        let (url, _) = scripted(200, vec![Step::Send(HI), Step::Hold]).await;
        let ctx = ctx_for(&url);
        let mut p = partial();
        let e = stream(&ctx, &mut p, Duration::from_secs(1))
            .await
            .err()
            .expect("an error");
        assert!(e.0.contains("sent nothing"), "{e}");
        assert_eq!(p.content, "Hi");
        // a slow stream that keeps talking is fine even when it takes longer than the idle limit in all
        let (url, _) = scripted(
            200,
            vec![
                Step::Send(HI),
                Step::Pause(600),
                Step::Send(HI),
                Step::Pause(600),
                Step::Send(b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n"),
            ],
        )
        .await;
        let ctx = ctx_for(&url);
        let out = stream(&ctx, &mut partial(), Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(out.content, "HiHi");
    }

    #[tokio::test]
    async fn the_devices_ten_minute_504_is_explained() {
        let (url, _) = scripted(
            504,
            vec![Step::Send(
                b"{\"error\":\"Upstream model server timed out.\"}",
            )],
        )
        .await;
        let ctx = ctx_for(&url);
        let e = stream(&ctx, &mut partial(), IDLE)
            .await
            .err()
            .expect("an error");
        assert!(e.0.contains("about 10 minutes"), "{e}");
    }

    #[test]
    fn thinking_off_is_sent_whatever_the_device_says_the_model_can_do() {
        let body = |mode: &str, thinking: bool| {
            let mut b = json!({});
            set_thinking(&mut b, mode, thinking);
            b
        };
        let off = json!({"chat_template_kwargs": {"enable_thinking": false}});
        assert_eq!(body("none", false), off);
        assert_eq!(body("none", true), off); // it said it can't think
        assert_eq!(body("switch", false), off);
        assert_eq!(
            body("switch", true)["chat_template_kwargs"]["enable_thinking"],
            json!(true)
        );
        assert_eq!(body("always", false), json!({"reasoning_effort": "low"}));
    }

    /// Stopping a chat while its tool runs ends the turn: no new completion is posted afterwards.
    #[tokio::test(flavor = "multi_thread")]
    async fn stop_during_a_tool_posts_no_new_completion() {
        use axum::body::Body;
        use axum::extract::{Request, State};
        use axum::response::{IntoResponse, Response};

        #[derive(Default)]
        struct Fake {
            completions: AtomicUsize,
            image_started: Notify,
            image_may_finish: AtomicBool,
        }
        async fn fake(State(f): State<Arc<Fake>>, req: Request) -> Response {
            let (m, p) = (req.method().to_string(), req.uri().path().to_string());
            let _ = axum::body::to_bytes(req.into_body(), usize::MAX).await;
            let j = |v: Value| axum::Json(v).into_response();
            match (m.as_str(), p.as_str()) {
                ("GET", "/v1/models") => {
                    j(json!({"data": [{"id": "org/chat", "aliases": ["default"]}]}))
                }
                ("GET", "/api/v1/models/running") => j(
                    json!({"instances": {"running": [{"model_id": "org/img"}, {"model_id": "org/chat"}]}}),
                ),
                ("POST", "/v1/images/generations") => {
                    f.image_started.notify_one();
                    while !f.image_may_finish.load(Ordering::SeqCst) {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    j(json!({"data": [{"b64_json": "eA=="}]}))
                }
                ("POST", "/v1/chat/completions") => {
                    f.completions.fetch_add(1, Ordering::SeqCst);
                    let call = json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "c1",
                        "function": {"name": "create_image", "arguments": "{\"prompt\":\"a duck\"}"}}]},
                        "finish_reason": "tool_calls"}]});
                    Response::builder()
                        .header("content-type", "text/event-stream")
                        .body(Body::from(format!("data: {call}\n\ndata: [DONE]\n\n")))
                        .unwrap()
                }
                _ => j(json!({"ok": true})),
            }
        }
        let f = Arc::new(Fake::default());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let app = axum::Router::new().fallback(fake).with_state(f.clone());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        let s = state();
        s.settings.upsert(None, "Fake", &url, "k").unwrap();
        let job = s
            .jobs
            .submit(
                &s,
                "chat",
                json!({"model": "default", "messages": [{"role": "user", "content": "draw a duck"}],
                "tools": ["create_image"], "tool_models": {"image": "org/img"}}),
            )
            .unwrap();
        let id = job["id"].as_str().unwrap().to_string();
        f.image_started.notified().await; // inside the image tool now
        assert!(s.jobs.cancel(&s, &id).await);
        f.image_may_finish.store(true, Ordering::SeqCst);
        let done = loop {
            let j = s.jobs.get(&id).unwrap();
            if j.get("finished").is_some() {
                break j;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert_eq!(done["status"], json!("cancelled"));
        assert_eq!(
            f.completions.load(Ordering::SeqCst),
            1,
            "a second completion was posted after Stop"
        );
    }
}
