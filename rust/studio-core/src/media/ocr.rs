//! Text from an image via the gateway's /v1/ocr (Tiiny SDK docs), with GLM-OCR reached on its own port
//! (media.py run_ocr). Sent as a throwaway job by the UI.

use serde_json::{json, Value};

use super::{decode_uri, err, http, port_open, running_instance, short};
use crate::jobs::{Ctx, JobError, JobResult};

/// urllib.parse.quote(s, safe="").
fn quote(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"_.-~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

pub async fn run(ctx: Ctx) -> JobResult {
    let image = ctx.req.get("image").and_then(Value::as_str).unwrap_or("").to_string();
    decode_uri(&image)?; // validates the data URI
    let model = ctx.model.clone();
    let inst = running_instance(&ctx, &model).await?;
    ctx.update(json!({"message": "Reading text"}));
    let cur = &ctx.target;
    let url = format!("{}/v1/ocr?model={}", cur.url, quote(&model));
    let (code, mut res) = http(&ctx, "POST", &url, Some(json!({"input": image, "response_format": "json"})), 300, true).await?;
    if code != 200 && err(&res).to_lowercase().contains("endpoint not found") {
        // GLM-OCR only serves OpenAI-style chat on its own port; the gateway's /v1/ocr has no route for it and
        // its chat route refuses it ("does not support chat", TiinyOS 0.1.35). Reach it directly when possible.
        let host = cur.host();
        let port = inst.get("port").and_then(Value::as_u64);
        if !port_open(&host, port).await {
            return Err(JobError(format!("{} only works over Wi-Fi/LAN (the gateway can't route it). Use PP-OCRv6 on this connection.", short(&model))));
        }
        let base = format!("http://{host}:{}", port.unwrap_or(0));
        let (_, info) = http(&ctx, "GET", &format!("{base}/v1/models"), None, 10, false).await?;
        let name = info.pointer("/data/0/id").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(short(&model)).to_string();
        let body = json!({"model": name, "temperature": 0, "max_tokens": 4096, "messages": [{"role": "user", "content": [
            {"type": "image_url", "image_url": {"url": image}}, {"type": "text", "text": "Text Recognition:"}]}]});
        let (code, chat) = http(&ctx, "POST", &format!("{base}/v1/chat/completions"), Some(body), 300, false).await?;
        let content = chat.pointer("/choices/0/message/content").and_then(Value::as_str);
        let Some(text) = content.filter(|_| code == 200) else { return Err(JobError(err(&chat))) };
        let text = text.trim().to_string();
        let lines: Vec<Value> = text.lines().filter(|l| !l.trim().is_empty()).map(|l| json!({"text": l})).collect();
        res = json!({"data": [{"text": text, "lines": lines}]});
    } else if code != 200 || res.get("data").and_then(Value::as_array).is_none_or(|d| d.is_empty()) {
        return Err(JobError(err(&res)));
    }
    let item = &res["data"][0];
    let lines: Vec<Value> = item.get("lines").and_then(Value::as_array).into_iter().flatten()
        .map(|l| json!({"text": l.get("text"), "score": l.get("score")})).collect();
    Ok(json!({"kind": "ocr", "params": {"model": model}, "sources": [], "outputs": [],
              "text": item.get("text").cloned().unwrap_or(json!("")), "lines": lines}))
}
