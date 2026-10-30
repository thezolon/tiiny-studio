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
        ("create_image", json!({"description": "Create a new image from a detailed text-to-image prompt.",
            "parameters": {"type": "object", "properties": {
                "prompt": {"type": "string", "description": "Detailed visual description: subject, setting, style, lighting."},
                "shape": {"type": "string", "enum": ["square", "portrait", "landscape", "wide"], "description": "Aspect ratio."}},
                "required": ["prompt"]}})),
        ("edit_image", json!({"description": "Edit the most recent image in the conversation (attached by the user or made earlier) with a plain-language instruction.",
            "parameters": {"type": "object", "properties": {
                "instruction": {"type": "string", "description": "What to change, and what to keep the same."}},
                "required": ["instruction"]}})),
        ("speak", json!({"description": "Read text aloud in the user's saved voice and return the audio.",
            "parameters": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}})),
        ("make_loop", json!({"description": "Make a short instrument loop for music production.",
            "parameters": {"type": "object", "properties": {
                "tags": {"type": "string", "description": "Comma-separated tags: one instrument first, then character, phrase and effect tags."},
                "bpm": {"type": "integer", "minimum": 100, "maximum": 150}},
                "required": ["tags"]}})),
        ("make_song", json!({"description": "Make a short song preview (about 8 seconds) that the user can continue to a full song in the Audio workspace.",
            "parameters": {"type": "object", "properties": {
                "caption": {"type": "string", "description": "Genre, mood, instruments, voice."},
                "lyrics": {"type": "string", "description": "Lyrics with [verse] and [chorus] tags on their own lines. Empty for an instrumental."}},
                "required": ["caption"]}})),
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
                    return part.pointer("/image_url/url").and_then(Value::as_str).map(String::from);
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
    Ok(format!("data:image/{};base64,{}", if ext == "jpg" { "jpeg" } else { &ext }, base64::engine::general_purpose::STANDARD.encode(bytes)))
}

fn as_int(v: Option<&Value>) -> Option<i64> {
    let v = v?;
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)).or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())).filter(|n| *n != 0)
}

/// Run one tool inline (we already hold the device lock); returns (gallery entry, text for the model).
async fn run_tool(ctx: &Ctx, name: &str, args: &Value, messages: &[Value]) -> Result<(Option<Value>, String), JobError> {
    let req = &ctx.req;
    let models = req.get("tool_models").cloned().unwrap_or(json!({}));
    let prefs = req.get("prefs").cloned().unwrap_or(json!({}));
    let model_for = |k: &str| models.get(k).and_then(Value::as_str).filter(|m| !m.is_empty()).map(String::from);
    let (kind, model, sub_req) = match name {
        "create_image" => {
            let size = shape(s(args, "shape")).map(String::from).or_else(|| Some(s(&prefs, "size").to_string()).filter(|x| !x.is_empty())).unwrap_or_else(|| "1024x1024".into());
            ("image", model_for("image"), json!({"prompt": s(args, "prompt"), "size": size}))
        }
        "edit_image" => {
            // newest first: made earlier in this turn, then the latest in the conversation (the page sends it,
            // since images the assistant made aren't part of the messages), then the user's attachments
            let src = ctx.get("turn_image").and_then(|v| v.as_str().map(String::from)).filter(|x| !x.is_empty())
                .or_else(|| Some(s(req, "last_image").to_string()).filter(|x| !x.is_empty()))
                .or_else(|| last_image(messages));
            let Some(mut src) = src else {
                return Ok((None, "There's no image in the conversation to edit. Ask the user to attach one.".into()));
            };
            if src.starts_with("/images/") {
                src = file_uri(ctx, src.rsplit('/').next().unwrap_or(""))?;
            }
            ("image", model_for("edit"), json!({"prompt": s(args, "instruction"), "images": [src]}))
        }
        "speak" => {
            let voice = req.get("voice").filter(|v| crate::device::truthy(v)).cloned();
            let Some(voice) = voice else {
                return Ok((None, "No saved voice to speak with. Tell the user to save a voice in the Audio workspace first.".into()));
            };
            ("speech", model_for("speech"), json!({"text": s(args, "text"), "ref_audio": voice["uri"], "ref_text": voice["text"], "voice_name": voice.get("name")}))
        }
        "make_loop" => {
            let bpm = as_int(args.get("bpm")).unwrap_or(120);
            let seconds = ((8.0 * 4.0 * 60.0 / bpm as f64) * 100.0).round() / 100.0;
            ("loop", model_for("loop"), json!({"prompt": format!("{}, 8 bars, {bpm} BPM", s(args, "tags")), "seconds_total": seconds, "bpm": bpm, "bars": 8}))
        }
        "make_song" => {
            let lyrics = s(args, "lyrics").trim().to_string();
            let gen = if lyrics.is_empty() { "bgm" } else { "mixed" };
            ("song", model_for("song"), json!({"caption": s(args, "caption"), "lyrics": lyrics, "generation_type": gen, "preview": true}))
        }
        _ => return Ok((None, format!("Unknown tool {name}"))),
    };
    let Some(model) = model else {
        return Ok((None, format!("No model for {} is installed on the device.", name.replace('_', " "))));
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
        let sub = ctx.sub(&model, sub_req);
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
        let run = crate::media::kind(kind).ok_or_else(|| JobError(format!("unknown job kind {kind}")))?;
        let mut entry = run(sub.clone()).await?;
        let k = entry.get("kind").and_then(Value::as_str).unwrap_or(kind).to_string();
        let e = entry.as_object_mut().unwrap();
        e.insert("id".into(), json!(sub.id));
        e.insert("created".into(), json!(now() as i64));
        e.insert("kind".into(), json!(k));
        e.insert("seconds".into(), json!(((now() - started) * 10.0).round() / 10.0));
        e.insert("via".into(), json!("chat"));
        ctx.state.store.add_history(entry.clone());
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
    Ok((Some(entry), format!("Done: the {what} is shown to the user and saved in the gallery (made with {}).", short(&model))))
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

/// POST a streaming chat completion; fill job["partial"] as it arrives. Returns (content, reasoning, tool calls, finish).
async fn stream_completion(ctx: &Ctx, partial: &mut Partial, body: &Value) -> Result<(String, String, Vec<Call>, Value), JobError> {
    let cur = ctx.state.settings.current();
    let res = ctx.state.http.post(format!("{}/v1/chat/completions", cur.url)).bearer_auth(&cur.key).json(body)
        .timeout(Duration::from_secs(900)).send().await.map_err(|e| JobError(format!("Device unreachable: {e}")))?;
    let code = res.status().as_u16();
    if code >= 400 {
        let raw = res.bytes().await.unwrap_or_default();
        return Err(JobError(match serde_json::from_slice::<Value>(&raw) {
            Ok(v) => err(&v),
            Err(_) => format!("The device answered {code}"),
        }));
    }
    let (mut content, mut reasoning) = (String::new(), String::new());
    let mut calls: std::collections::BTreeMap<i64, Call> = Default::default();
    let mut finish = Value::Null;
    let mut res = res;
    let mut buf: Vec<u8> = vec![];
    'read: loop {
        let chunk = res.chunk().await.map_err(|e| JobError(format!("Device unreachable: {e}")))?;
        let Some(chunk) = chunk else { break };
        buf.extend_from_slice(&chunk);
        while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
            let raw: Vec<u8> = buf.drain(..=nl).collect();
            if ctx.cancelled() {
                break 'read;
            }
            let line = String::from_utf8_lossy(&raw).trim().to_string();
            if !line.starts_with("data:") || line == "data: [DONE]" {
                continue;
            }
            let d: Value = serde_json::from_str(&line[5..]).map_err(|e| JobError(format!("ValueError: {e}")))?;
            let ch = d.pointer("/choices/0").cloned().unwrap_or(json!({}));
            let delta = ch.get("delta").cloned().unwrap_or(json!({}));
            if let Some(r) = delta.get("reasoning_content").and_then(Value::as_str).filter(|x| !x.is_empty()) {
                reasoning.push_str(r);
            }
            if let Some(c) = delta.get("content").and_then(Value::as_str).filter(|x| !x.is_empty()) {
                content.push_str(c);
            }
            for tc in delta.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
                let slot = calls.entry(tc.get("index").and_then(Value::as_i64).unwrap_or(0)).or_insert(Call { id: String::new(), name: String::new(), args: String::new() });
                if let Some(id) = tc.get("id").and_then(Value::as_str).filter(|x| !x.is_empty()) {
                    slot.id = id.to_string();
                }
                slot.name.push_str(tc.pointer("/function/name").and_then(Value::as_str).unwrap_or(""));
                slot.args.push_str(tc.pointer("/function/arguments").and_then(Value::as_str).unwrap_or(""));
            }
            if let Some(f) = ch.get("finish_reason").filter(|f| crate::device::truthy(f)) {
                finish = f.clone();
            }
            partial.content = format!("{}{content}", partial.done);
            partial.reasoning = format!("{}{reasoning}", partial.done_reasoning);
            ctx.update(json!({"partial": partial.json()}));
        }
    }
    Ok((content, reasoning, calls.into_values().collect(), finish))
}

/// The real id behind "default" (the device's alias for the loaded model), so it can be reloaded after a swap, and
/// its thinking object (None when the device doesn't say).
async fn chat_model_id(ctx: &Ctx, wanted: &str) -> Result<(String, Option<Value>), JobError> {
    let (code, res) = crate::device::call(&ctx.state, "GET", "/v1/models", None, 15, None).await;
    if code == 200 {
        for item in res.get("data").and_then(Value::as_array).into_iter().flatten() {
            let aliases = item.get("aliases").and_then(Value::as_array).cloned().unwrap_or_default();
            if s(item, "id") == wanted || aliases.iter().any(|a| a.as_str() == Some(wanted)) {
                return Ok((s(item, "id").to_string(), item.get("thinking").filter(|t| t.is_object()).cloned()));
            }
        }
    }
    if !wanted.is_empty() && wanted != "default" {
        return Ok((wanted.to_string(), None));
    }
    Err(JobError("No chat model is loaded. Load one from the Models panel.".into()))
}

pub async fn run(ctx: Ctx) -> JobResult {
    let req = ctx.req.clone();
    let messages: Vec<Value> = req.get("messages").and_then(Value::as_array).into_iter().flatten().filter(|m| m.is_object()).cloned().collect();
    if messages.is_empty() {
        return Err(JobError("Nothing to send".into()));
    }
    // attached documents are untrusted: delimited, screened, and flagged to the user (never silently dropped)
    let attached = crate::safety::has_documents(&messages);
    let (messages, warnings) = crate::safety::guard_messages(&messages);
    if !warnings.is_empty() {
        ctx.update(json!({"warnings": warnings}));
    }
    let wanted = Some(s(&req, "model")).filter(|m| !m.is_empty()).unwrap_or("default");
    let (model, thinking_info) = chat_model_id(&ctx, wanted).await?;
    let mode = crate::media::thinking_mode(&model, thinking_info.as_ref());
    ctx.update(json!({"model": model}));
    let wanted_tools: Vec<&str> = req.get("tools").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    let tools: Vec<Value> = tool_specs().into_iter().filter(|(n, _)| wanted_tools.contains(n)).map(|(n, spec)| {
        let mut f = Map::new();
        f.insert("name".into(), json!(n));
        f.extend(spec.as_object().cloned().unwrap_or_default());
        json!({"type": "function", "function": f})
    }).collect();
    let extra = s(&req, "system").trim();
    let rule = if attached { format!(" {}", crate::safety::RULE) } else { String::new() };
    let system = if extra.is_empty() { format!("{SYSTEM}{rule}") } else { format!("{SYSTEM}{rule} {extra}") };
    let mut convo = vec![json!({"role": "system", "content": system})];
    convo.extend(messages.iter().cloned());
    let mut partial = Partial { content: String::new(), reasoning: String::new(), done: String::new(), done_reasoning: String::new() };
    ctx.update(json!({"partial": partial.json()}));
    let thinking = req.get("thinking").is_some_and(crate::device::truthy);
    let think = mode == "always" || (mode == "switch" && thinking);
    let max_tokens = as_int(req.get("max_tokens")).unwrap_or(4096);
    let mut artifacts: Vec<Value> = vec![];
    let mut finish = Value::Null;
    for round in 0..=MAX_TOOL_ROUNDS {
        let mut body = json!({"model": model, "messages": convo, "stream": true, "max_tokens": max_tokens});
        if mode == "always" {
            // reasons whatever it's sent
            body["reasoning_effort"] = json!(if thinking { "medium" } else { "low" });
        } else if mode == "switch" {
            body["chat_template_kwargs"] = json!({"enable_thinking": thinking});
        }
        if !tools.is_empty() && round < MAX_TOOL_ROUNDS {
            body["tools"] = json!(tools);
        }
        ctx.update(json!({"message": if think { "Thinking" } else { "Replying" }}));
        let (content, _reasoning, calls, f) = stream_completion(&ctx, &mut partial, &body).await?;
        finish = f;
        if ctx.cancelled() {
            return Err(JobError("Stopped".into()));
        }
        if calls.is_empty() {
            break;
        }
        let call_id = |i: usize, c: &Call| if c.id.is_empty() { format!("call_{i}") } else { c.id.clone() };
        convo.push(json!({"role": "assistant", "content": content,
            "tool_calls": calls.iter().enumerate().map(|(i, c)| json!({"id": call_id(i, c), "type": "function",
                "function": {"name": c.name, "arguments": if c.args.is_empty() { "{}" } else { &c.args }}})).collect::<Vec<_>>()}));
        for (i, c) in calls.iter().enumerate() {
            let args: Value = serde_json::from_str(if c.args.is_empty() { "{}" } else { &c.args }).ok().filter(Value::is_object).unwrap_or(json!({}));
            let (entry, result) = match run_tool(&ctx, &c.name, &args, &messages).await {
                Ok(r) => r,
                Err(e) if ctx.cancelled() => return Err(e),
                Err(e) => (None, format!("That didn't work: {e}")),
            };
            if let Some(entry) = entry {
                let prompt = ["prompt", "instruction", "text", "tags", "caption"].iter().map(|k| s(&args, k)).find(|v| !v.is_empty()).unwrap_or("");
                artifacts.push(json!({"kind": entry["kind"], "id": entry["id"], "outputs": entry.get("outputs").cloned().unwrap_or(json!([])),
                    "prompt": prompt, "duration": entry.get("duration").cloned().unwrap_or(Value::Null), "tool": c.name}));
                ctx.update(json!({"artifacts": artifacts}));
            }
            convo.push(json!({"role": "tool", "tool_call_id": call_id(i, c), "content": result}));
        }
        // keep what was said before the tool call on screen while the reply goes on
        let sep = if content.trim().is_empty() { "" } else { "\n\n" };
        partial.done = format!("{}{sep}", partial.content);
        partial.done_reasoning = partial.reasoning.clone();
        ctx.update(json!({"partial": partial.json()}));
        ctx.remove(&["tool"]);
    }
    let mut entry = json!({"kind": "chat", "params": {"model": model}, "sources": [], "outputs": [], "text": partial.content.trim(),
                           "reasoning": partial.reasoning.trim(), "artifacts": artifacts, "finish": finish});
    if !warnings.is_empty() {
        entry["warnings"] = json!(warnings);
    }
    Ok(entry)
}
