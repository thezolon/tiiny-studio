//! Chat-model helpers (media.py run_assist): prompt rewrite, edit instruction, image description, lyrics,
//! loop tags. One chat-completions call through the gateway; thinking is off so short tasks stay quick.

use base64::Engine;
use regex::Regex;
use serde_json::{json, Value};

use super::{decode_uri, ensure_running, err, make_room, put_back};
use crate::jobs::{Ctx, JobError, JobResult};

fn system(task: &str) -> Option<(&'static str, u64, &'static str)> {
    Some(match task {
        "prompt" => ("You improve prompts for a text-to-image model. Rewrite the user's idea as one vivid, concrete prompt: \
subject, setting, composition, lighting, style, mood. Keep everything the user asked for and invent nothing \
that contradicts it. Under 80 words. Reply with the prompt only: no preamble, no quotes, no lists.", 200, "Rewriting the prompt"),
        "edit" => ("You improve instructions for an image-editing model that changes an existing picture. Rewrite the user's \
request as one clear, specific edit instruction: say exactly what to change and what must stay the same \
(subject, pose, framing, style). Under 60 words. Reply with the instruction only: no preamble, no quotes.", 160, "Rewriting the instruction"),
        "describe" => ("You describe images for an artist who wants to recreate or edit them. Write one paragraph (60-100 words) \
covering the subject, setting, composition, colours, lighting and style, phrased so it works as a \
text-to-image prompt. Reply with the description only.", 260, "Looking at the image"),
        "lyrics" => ("You write song lyrics for a singing model. Use only these section tags, each alone on its own line: \
[verse], [chorus], [bridge]. Under each tag write 2-4 short singable lines, one per line, then a blank line \
before the next tag. Repeat the chorus word for word. No title, no notes, no other markup, no blank lines \
inside a section. Reply with the lyrics only.", 500, "Writing lyrics"),
        "tags" => ("You write prompts for Foundation-1, a model that makes instrument loops from comma-separated tags. Turn the \
user's description into tags: first exactly one instrument, then 1-3 character tags, 1-2 phrase tags and 0-2 \
effect tags. Prefer tags from the vocabulary given; never mention drums, tempo, bars or key. Reply with one \
line of comma-separated tags only.", 60, "Picking tags"),
        _ => return None,
    })
}

/// Strip thinking, code fences and wrapping quotes from a short reply.
pub fn clean_reply(text: &str) -> String {
    let think = Regex::new(r"(?s)<think>.*?</think>").unwrap();
    let text = think.replace_all(text, "").trim().to_string();
    let fence = Regex::new(r"^```\w*\n?|\n?```$").unwrap();
    let text = fence.replace_all(&text, "").trim().to_string();
    text.trim_matches(|c| c == '"' || c == '“' || c == '”').trim().to_string()
}

/// Normalise section tags to [verse]/[chorus]/[bridge] lines; error when there are no sections at all.
pub fn clean_lyrics(text: &str) -> Result<String, JobError> {
    let tag_re = Regex::new(r"(?i)^\[?\s*(verse|chorus|bridge|intro|outro|pre-chorus|hook)(?:\s*\d+)?\s*\]?\s*:?$").unwrap();
    let mut out: Vec<String> = vec![];
    for line in clean_reply(text).lines() {
        let line = line.trim().trim_matches(|c| c == '*' || c == '_' || c == '#').trim();
        if let Some(m) = tag_re.captures(line) {
            let t = m[1].to_lowercase();
            let tag = match t.as_str() {
                "pre-chorus" => "verse".to_string(),
                "hook" => "chorus".to_string(),
                _ => t,
            };
            if !out.is_empty() {
                out.push(String::new());
            }
            out.push(format!("[{tag}]"));
        } else if !line.is_empty() && !out.is_empty() {
            out.push(line.to_string());
        }
    }
    let text = out.join("\n");
    if !Regex::new(r"\[(verse|chorus)\]").unwrap().is_match(&text) {
        return Err(JobError("The model didn't return lyrics with [verse]/[chorus] sections. Try again.".into()));
    }
    // drop empty intro/outro tags (Python: \[(intro|outro)\]\n(?=\n|$))
    let mut cleaned = text.clone();
    for tag in ["[intro]\n", "[outro]\n"] {
        let mut s = String::new();
        let mut rest = cleaned.as_str();
        while let Some(i) = rest.find(tag) {
            let after = &rest[i + tag.len()..];
            if after.is_empty() || after.starts_with('\n') {
                s.push_str(&rest[..i]);
            } else {
                s.push_str(&rest[..i + tag.len()]);
            }
            rest = after;
        }
        s.push_str(rest);
        cleaned = s;
    }
    Ok(cleaned.trim().to_string())
}

fn safe_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '-')
}

pub async fn run(ctx: Ctx) -> JobResult {
    let req = &ctx.req;
    let task = req.get("task").and_then(Value::as_str).unwrap_or("");
    let Some((prompt, tokens, message)) = system(task) else {
        let shown = match req.get("task") {
            Some(Value::String(s)) => s.clone(),
            None | Some(Value::Null) => "None".into(),
            Some(v) => v.to_string(),
        };
        return Err(JobError(format!("unknown helper {shown}")));
    };
    let text = req.get("text").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let user: Value = if task == "describe" {
        let uri = match req.get("file").and_then(Value::as_str).filter(|f| !f.is_empty()) {
            Some(name) => {
                let path = ctx.state.store.images.join(name);
                if !safe_name(name) || !path.is_file() {
                    return Err(JobError("That image isn't in the gallery".into()));
                }
                let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
                let bytes = std::fs::read(&path).map_err(|e| JobError(e.to_string()))?;
                format!("data:image/{};base64,{}", if ext == "jpg" { "jpeg" } else { &ext }, base64::engine::general_purpose::STANDARD.encode(bytes))
            }
            None => {
                let image = req.get("image").and_then(Value::as_str).unwrap_or("");
                decode_uri(image)?;
                image.to_string()
            }
        };
        let t = if text.is_empty() { "Describe this image." } else { &text };
        json!([{"type": "text", "text": t}, {"type": "image_url", "image_url": {"url": uri}}])
    } else if text.is_empty() {
        return Err(JobError("Nothing to work from: type something first".into()));
    } else {
        match req.get("context").and_then(Value::as_str).filter(|c| !c.is_empty()) {
            Some(c) => json!(format!("{}\n\n{text}", c.trim())),
            None => json!(text),
        }
    };
    let model = ctx.model.clone();
    // With swapping allowed for this section, load the helper's model (making room) and put things back after;
    // otherwise just check it's loaded, for a clear "isn't loaded" error instead of a gateway 4xx.
    let unloaded = if ctx.state.settings.swap_allowed(if matches!(task, "lyrics" | "tags") { "audio" } else { "image" }) {
        make_room(&ctx, &model).await?
    } else {
        ensure_running(&ctx, &model).await?;
        vec![]
    };
    let result = call(&ctx, task, &model, prompt, tokens, message, user).await;
    let back = put_back(&ctx, &model, &unloaded).await;
    let entry = result?;
    back?;
    Ok(entry)
}

async fn call(ctx: &Ctx, task: &str, model: &str, prompt: &str, tokens: u64, message: &str, user: Value) -> JobResult {
    let mut body = json!({"model": model, "messages": [{"role": "system", "content": prompt}, {"role": "user", "content": user}],
        "max_tokens": tokens, "temperature": if task == "lyrics" { 0.8 } else { 0.6 }, "stream": false});
    if model.contains("gpt-oss") {
        // always reasons and rejects enable_thinking; give it room to think
        body["reasoning_effort"] = json!("low");
        body["max_tokens"] = json!(tokens + 1500);
    } else {
        body["chat_template_kwargs"] = json!({"enable_thinking": false});
    }
    ctx.update(json!({"message": message}));
    let (code, res) = crate::device::call(&ctx.state, "POST", "/v1/chat/completions", Some(body), 600, None).await;
    if code != 200 || res.get("choices").and_then(Value::as_array).is_none_or(|c| c.is_empty()) {
        return Err(JobError(err(&res)));
    }
    let reply = res.pointer("/choices/0/message/content").and_then(Value::as_str).unwrap_or("");
    let mut out = if task == "lyrics" { clean_lyrics(reply)? } else { clean_reply(reply) };
    if task == "tags" {
        out = Regex::new(r"\s*\n\s*").unwrap().replace_all(&out, ", ").trim_matches(|c| c == ',' || c == ' ').to_string();
    }
    if out.is_empty() {
        return Err(JobError("The model returned nothing. Try again.".into()));
    }
    Ok(json!({"kind": "assist", "params": {"model": model, "task": task}, "sources": [], "outputs": [], "text": out,
              "usage": res.get("usage").cloned().unwrap_or(Value::Null)}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies() {
        assert_eq!(clean_reply("<think>hmm</think>\n```text\n\"A fox\"\n```"), "A fox");
        assert_eq!(clean_reply("“quoted”"), "quoted");
    }

    #[test]
    fn lyrics() {
        let raw = "**Verse 1:**\nline one\nline two\n\n## Chorus\nsing it\n[Intro]\n\n[Hook]\nhey";
        // same as Python: the dropped empty [intro] leaves its blank line behind
        assert_eq!(clean_lyrics(raw).unwrap(), "[verse]\nline one\nline two\n\n[chorus]\nsing it\n\n\n[chorus]\nhey");
        assert!(clean_lyrics("no sections here").is_err());
    }
}
