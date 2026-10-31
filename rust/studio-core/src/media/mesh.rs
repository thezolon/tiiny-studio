//! Image to 3D (Hunyuan3D, TRELLIS.2): [cut-out] -> [background] -> shape -> [simplify] -> [texture -> export],
//! chaining artifacts (media.py run_mesh). The stages, their inputs and their params come from the model's own
//! /v1/3d/capabilities.

use std::time::Duration;

use base64::Engine;
use serde_json::{json, Map, Value};

use super::{decode_uri, ensure_running, err, http, http_bytes, json_reply, make_room, multipart, put_back, py_json, py_round, save};
use crate::device::truthy;
use crate::jobs::{Ctx, JobError, JobResult};

/// Polls in a row without an answer (~5 minutes at 2 s) before a 3D stage gives up (media.py MESH_LOST_POLLS).
const LOST_POLLS: u32 = 150;

/// What each 3D stage takes when the model doesn't say (Hunyuan3D-2.1's /v1/3d/capabilities, TiinyOS 0.1.37). Each
/// model lists its own; TRELLIS.2 chains "shape_artifact" and "geometry_artifact" instead of "mesh".
fn stage_inputs(stage: &str) -> &'static [&'static str] {
    match stage {
        "background" | "shape" => &["image"],
        "simplify" => &["mesh"],
        "texture" => &["image", "mesh"],
        _ => &["material_artifact", "mesh_glb"], // export
    }
}

/// Inputs the task form takes under another field name (its fields: image, mesh, material, file, shape_artifact, ...).
fn stage_field(input: &str) -> &str {
    match input {
        "material_artifact" => "material",
        "mesh_glb" => "mesh",
        other => other,
    }
}

fn weight(stage: &str) -> f64 {
    match stage {
        "background" => 5.0,
        "shape" => 45.0,
        "simplify" => 3.0,
        "texture" => 42.0,
        _ => 5.0, // export
    }
}

/// An artifact the device returned: (file name, bytes, mime type).
type Artifact = (String, Vec<u8>, String);

/// A Python dict of artifacts: insertion order is the multipart field order.
#[derive(Default)]
struct Have(Vec<(String, Artifact)>);

impl Have {
    fn set(&mut self, key: &str, art: Artifact) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = art,
            None => self.0.push((key.to_string(), art)),
        }
    }

    fn get(&self, key: &str) -> Option<&Artifact> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, a)| a)
    }
}

/// json.dumps of the params, keys in the given order (what the device is sent as the "params" field).
fn dumps(items: &[(&str, &Value)]) -> String {
    format!("{{{}}}", items.iter().map(|(k, v)| format!("{}: {}", Value::String(k.to_string()), py_json(v))).collect::<Vec<_>>().join(", "))
}

fn str_of<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).filter(|x| truthy(x)).and_then(Value::as_str)
}

// Hunyuan3D builds the whole picture, background included, and its own background stage fails on the device (BriefNet
// missing, TiinyOS 0.1.35-0.1.37). An image model that makes transparent pictures (Qwen-Image) cuts the subject out.
const CUTOUT_PROMPT: &str = "Extract the main subject from this photo onto a transparent background. Keep the subject exactly the same: \
shape, colours, details. Remove everything else. The image has an alpha channel and a transparent background.";

/// Hunyuan3D takes every pixel that isn't fully transparent as part of the subject, and an image model's cut-out leaves
/// faint noise (alpha 1-2) over the whole background, so the model came out as a flat card. Make every nearly see-through
/// pixel (alpha < 32) fully transparent white. Only 8-bit RGBA PNGs (what Qwen-Image returns) change; the result is
/// written uncompressed (stored deflate blocks), byte for byte the same as media.py clean_alpha.
pub fn clean_alpha(data: &[u8]) -> Vec<u8> {
    let Ok(mut reader) = png::Decoder::new(data).read_info() else {
        return data.to_vec();
    };
    let info = reader.info();
    let (w, h) = (info.width, info.height);
    if info.bit_depth != png::BitDepth::Eight || info.color_type != png::ColorType::Rgba || info.interlaced {
        return data.to_vec();
    }
    let mut px = vec![0; reader.output_buffer_size()];
    if reader.next_frame(&mut px).is_err() {
        return data.to_vec();
    }
    for p in px.chunks_exact_mut(4) {
        if p[3] < 32 {
            p.copy_from_slice(&[255, 255, 255, 0]);
        }
    }
    let stride = w as usize * 4;
    let mut rows = Vec::with_capacity(px.len() + h as usize);
    for line in px.chunks_exact(stride) {
        rows.push(0);
        rows.extend_from_slice(line);
    }
    let mut z = vec![0x78, 0x01];
    for (i, block) in rows.chunks(65535).enumerate() {
        z.push(u8::from((i + 1) * 65535 >= rows.len()));
        z.extend_from_slice(&(block.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        z.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32); // adler32
    for &x in &rows {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let chunk = |out: &mut Vec<u8>, kind: &[u8], body: &[u8]| {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        let mut crc = crc32fast::Hasher::new();
        crc.update(kind);
        crc.update(body);
        out.extend_from_slice(&crc.finalize().to_be_bytes());
    };
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = vec![];
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", b"");
    out
}

/// The subject of req["image"] on a transparent background, by req["cutout_model"]: (png bytes, saved file name).
/// Swaps the image model in when the 3D switch allows (and puts things back), else it has to be loaded.
async fn cut_out(ctx: &Ctx) -> Result<(Vec<u8>, String), JobError> {
    let req = &ctx.req;
    let model = str_of(req, "cutout_model")
        .ok_or_else(|| JobError("Cutting out the subject needs an image model that makes transparent pictures (Qwen-Image).".into()))?
        .to_string();
    ctx.update(json!({"stage": "cutout"}));
    let unloaded = if ctx.state.settings.swap_allowed("mesh") {
        make_room(ctx, &model).await?
    } else {
        ensure_running(ctx, &model).await?;
        vec![]
    };
    ctx.update(json!({"message": format!("Cutting out the subject with {}", model.rsplit('/').next().unwrap_or(&model))}));
    let size = str_of(req, "cutout_size").unwrap_or("512x512");
    let sub = ctx.sub(&model, json!({"model": model, "prompt": CUTOUT_PROMPT, "images": [req.get("image")], "size": size, "n": 1}));
    let result = super::image::run(sub).await;
    ctx.remove(&["tool_message", "tool_progress"]);
    let back = put_back(ctx, &model, &unloaded).await;
    let entry = result?;
    back?;
    let dir = &ctx.state.store.images;
    for name in entry.get("sources").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        let _ = std::fs::remove_file(dir.join(name)); // its copy of the source: the 3D entry keeps its own
    }
    let name = entry.pointer("/outputs/0").and_then(Value::as_str).unwrap_or("").to_string();
    let bytes = clean_alpha(&std::fs::read(dir.join(&name)).map_err(|e| JobError(e.to_string()))?);
    let _ = std::fs::write(dir.join(&name), &bytes); // the gallery shows the picture the model was built from
    Ok((bytes, name))
}

/// A failed or cancelled build keeps none of the files it saved (they'd have no gallery entry to delete them by).
pub async fn run(ctx: Ctx) -> JobResult {
    let mut saved = vec![];
    let result = build(&ctx, &mut saved).await;
    if result.is_err() {
        for name in &saved {
            let _ = std::fs::remove_file(ctx.state.store.images.join(name));
        }
    }
    result
}

async fn build(ctx: &Ctx, saved: &mut Vec<String>) -> JobResult {
    let ctx = ctx.clone();
    let req = &ctx.req;
    let (_, original) = decode_uri(req.get("image").and_then(Value::as_str).unwrap_or(""))?;
    // the page leaves it off for a picture that's already transparent
    let (image, cut) = if req.get("cutout").is_some_and(truthy) {
        let (bytes, name) = cut_out(&ctx).await?;
        saved.push(name.clone());
        (bytes, Some(name))
    } else {
        (original.clone(), None)
    };
    let model = ctx.model.clone();
    ensure_running(&ctx, &model).await?;
    let url = format!("{}/v1/3d", ctx.target.url); // gateway proxy: /v1/3d/tasks -> container /v1/tasks
    // With two 3D models loaded the gateway sends an unnamed request to either one: name it on every call.
    let mq = format!("?model={}", super::quote(&model));
    let (_, caps) = http(&ctx, "GET", &format!("{url}/capabilities{mq}"), None, 30, true).await?;
    let caps = caps.get("data").filter(|d| d.is_object()).cloned().unwrap_or(json!({}));
    let on = |k: &str| req.get(k).is_some_and(truthy);
    let mut plan = vec![];
    if on("remove_bg") {
        plan.push("background");
    }
    plan.push("shape");
    if on("faces") {
        plan.push("simplify");
    }
    if on("texture") {
        plan.extend(["texture", "export"]);
    }
    // A model with input_refs (TRELLIS.2) keeps each stage's artifacts: later stages point at them instead of the studio
    // uploading them again (TRELLIS's shape, geometry and material together pass the device's 200 MB request limit).
    let use_refs = caps.get("input_refs").is_some_and(truthy);
    let mut refs: Map<String, Value> = Map::new();
    if let Some(stages) = caps.get("stages").and_then(Value::as_array).filter(|s| !s.is_empty()) {
        plan.retain(|s| stages.iter().any(|x| x.as_str() == Some(s)));
    }
    let total: f64 = plan.iter().map(|s| weight(s)).sum();
    let mut done = 0.0;
    let quality: Vec<(&str, Value)> = ["output_precision", "inference_quality", "asset_quality", "seed"]
        .into_iter()
        .filter_map(|k| match req.get(k) {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if s.is_empty() => None,
            Some(v) => Some((k, v.clone())),
        })
        .collect();
    let mut have = Have::default();
    have.set("image", ("input.png".into(), image.clone(), "image/png".into()));
    let mut stage_log = vec![];
    let src = save(&ctx, &format!("{}-src.png", ctx.id), &original)?;
    saved.push(src.clone());

    let last = plan.last().copied();
    for stage in plan {
        if ctx.cancelled() {
            return Err(JobError("Cancelled".into()));
        }
        let mut params: Vec<(&str, Value)> = match stage {
            "simplify" => {
                let faces = &req["faces"];
                let n = match faces {
                    Value::String(s) => s.trim().parse::<i64>().map_err(|_| JobError(format!("ValueError: invalid literal for int() with base 10: '{s}'")))?,
                    other => other.as_f64().unwrap_or(1.0) as i64,
                };
                vec![("target_faces", json!(n))]
            }
            "background" => vec![],
            _ => quality.clone(),
        };
        if let Some(spec) = caps.pointer(&format!("/stage_specs/{stage}")).filter(|s| s.is_object()) {
            // a model that lists what each stage accepts (TRELLIS.2): send only that
            let listed = |k: &str| {
                spec.get("params").and_then(Value::as_object).is_some_and(|p| p.contains_key(k))
                    || spec.get("controls").and_then(Value::as_array).is_some_and(|c| c.iter().any(|x| x.as_str() == Some(k)))
            };
            params.retain(|(k, _)| listed(k));
        }
        let params = dumps(&params.iter().map(|(k, v)| (*k, v)).collect::<Vec<_>>());
        let names: Vec<String> = match caps.pointer(&format!("/inputs/{stage}")).and_then(Value::as_array).filter(|a| !a.is_empty()) {
            Some(a) => a.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
            None => stage_inputs(stage).iter().map(|s| s.to_string()).collect(),
        };
        let mut files: Vec<(&str, (&str, &[u8], &str))> = vec![];
        let mut inrefs = Map::new();
        for name in &names {
            let field = stage_field(name);
            if let (true, Some(r)) = (use_refs, refs.get(name.as_str())) {
                inrefs.insert(name.clone(), r.clone());
            } else if let (Some((n, d, m)), false) = (have.get(name), files.iter().any(|(f, _)| *f == field)) {
                files.push((field, (n.as_str(), d.as_slice(), m.as_str())));
            }
        }
        let mut fields = vec![("stage", stage.to_string()), ("params", params)];
        if !inrefs.is_empty() {
            fields.push(("input_refs", py_json(&Value::Object(inrefs))));
        }
        let (body, mp) = multipart(&fields, &files);
        let (code, res) = http_bytes(&ctx, "POST", &format!("{url}/tasks{mq}"), Some((body, mp)), 3600, true).await?;
        let (code, res) = json_reply(code, res)?;
        if code >= 300 {
            return Err(JobError(format!("{stage}: {}", err(&res))));
        }
        let task = res.get("data").ok_or_else(|| JobError("KeyError: 'data'".into()))?.get("task_id").ok_or_else(|| JobError("KeyError: 'task_id'".into()))?;
        let tid = match task {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        ctx.set_remote_task(Some(format!("{url}/tasks/{tid}/cancel{mq}")));
        let mut silent = 0; // polls in a row without a status: the task is gone, or the link is down (a stage can take many minutes)
        let d = loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let (code, st) = http(&ctx, "GET", &format!("{url}/tasks/{tid}{mq}"), None, 30, true).await.unwrap_or((0, json!({})));
            if code == 0 || code >= 500 {
                // no answer: ride out a dropped link
                if ctx.cancelled() {
                    return Err(JobError("Cancelled".into()));
                }
                silent += 1;
                if silent >= LOST_POLLS {
                    return Err(JobError(format!("{stage}: lost the Tiiny while it worked on this task")));
                }
                ctx.update(json!({"message": "Can't reach the Tiiny, still trying…"}));
                continue;
            }
            let d = st.get("data").filter(|d| truthy(d)).cloned().unwrap_or(json!({}));
            let p = d.get("progress").and_then(Value::as_f64).unwrap_or(0.0);
            let shown = ["message", "status"].iter().filter_map(|k| d.get(*k)).find(|v| truthy(v)).or(d.get("status")).map_or("None".to_string(), |v| match v {
                Value::String(s) => s.clone(),
                Value::Null => "None".into(),
                other => other.to_string(),
            });
            let name = stage[..1].to_uppercase() + &stage[1..];
            ctx.update(json!({"progress": py_round((done + weight(stage) * p / 100.0) / total * 100.0), "message": format!("{name}: {shown}"), "stage": stage}));
            if matches!(d.get("status").and_then(Value::as_str), Some("completed" | "failed" | "cancelled" | "error")) {
                break d;
            }
            if ctx.cancelled() {
                return Err(JobError("Cancelled".into()));
            }
            silent = if d.get("status").is_some_and(truthy) { 0 } else { silent + 1 };
            if silent >= LOST_POLLS {
                return Err(JobError(format!("{stage}: the device stopped reporting on this task")));
            }
        };
        if d.get("status").and_then(Value::as_str) != Some("completed") {
            let why = ["error", "message", "status"].iter().filter_map(|k| d.get(*k)).find(|v| truthy(v)).or(d.get("status")).cloned().unwrap_or(Value::Null);
            return Err(JobError(format!("{stage}: {}", err(&why))));
        }
        let out = d.get("result").filter(|r| truthy(r)).and_then(|r| r.get("output")).filter(|o| truthy(o)).cloned().unwrap_or(json!({}));
        stage_log.push(json!({"stage": stage, "metadata": out.get("metadata")}));
        // Stages report artifacts either as a list or (texture) as one top-level payload.
        let payload_key = ["glb", "artifact", "image", "mesh"].into_iter().find(|k| out.get(*k).is_some_and(Value::is_string)).unwrap_or("");
        let single = json!({"artifact_type": str_of(&out, "artifact_type").unwrap_or("mesh_glb"), "filename": out.get("filename"),
            "mime_type": out.get("mime_type"), "payload_key": payload_key});
        let arts: Vec<Value> = match out.get("artifacts").filter(|a| truthy(a)).and_then(Value::as_array) {
            Some(a) => a.clone(),
            None => vec![single],
        };
        for art in &arts {
            let kind = str_of(art, "artifact_type").unwrap_or("");
            if kind.ends_with("_preview") && last != Some(stage) {
                continue; // a preview only matters as the model when nothing comes after it
            }
            if let (Some(id), false) = (str_of(art, "artifact_id"), kind.is_empty()) {
                // kept on the device: later stages can point at it
                let r = json!({"task_id": tid, "artifact_id": id});
                refs.insert(kind.to_string(), r.clone());
                refs.insert(format!("{kind}_artifact"), r);
            }
            let local = !use_refs || kind == "mesh_glb" || kind.ends_with("_preview"); // only the model itself comes to the studio
            let data = match (out.get(str_of(art, "payload_key").unwrap_or("")).and_then(Value::as_str), str_of(art, "artifact_id")) {
                (Some(payload), _) => base64::engine::general_purpose::STANDARD.decode(payload).map_err(|e| JobError(format!("Error: {e}")))?,
                (None, Some(_)) if !local => continue,
                (None, Some(id)) => {
                    // not inline (TRELLIS.2): fetch it from the task
                    let mut got = (0, Err(json!("Device unreachable")));
                    for _ in 0..5 {
                        // a dropped link shouldn't lose a finished stage
                        got = match http_bytes(&ctx, "GET", &format!("{url}/tasks/{tid}/artifacts/{}{mq}", super::quote(id)), None, 600, true).await {
                            Ok(r) => r,
                            Err(e) => (0, Err(json!({"error": e.0}))),
                        };
                        if got.0 == 200 || (got.0 > 0 && got.0 < 500) || ctx.cancelled() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_secs(3)).await;
                    }
                    let (code, raw) = got;
                    match raw {
                        Ok(bytes) if code == 200 => bytes,
                        Ok(_) => return Err(JobError(format!("{stage}: couldn't fetch its {kind}: Unexpected response (HTTP {code})."))),
                        Err(v) => return Err(JobError(format!("{stage}: couldn't fetch its {kind}: {}", err(&v)))),
                    }
                }
                (None, None) => continue,
            };
            let fname = str_of(art, "filename").map(String::from).unwrap_or_else(|| format!("{kind}.bin"));
            let mime = str_of(art, "mime_type").unwrap_or("application/octet-stream");
            let item: Artifact = (fname, data, mime.to_string());
            if kind == "mesh_glb" {
                have.set("mesh", item.clone());
                have.set("mesh_glb", item);
            } else if kind.starts_with("material") {
                have.set("material", item.clone());
                have.set("material_artifact", item);
            } else if kind.starts_with("image") || mime.starts_with("image/") {
                have.set("image", item);
            } else if kind.ends_with("_preview") {
                have.set("preview", item);
            } else if !kind.is_empty() {
                // TRELLIS.2's shape/geometry: the next stage asks for "<type>_artifact"
                have.set(kind, item.clone());
                have.set(&format!("{kind}_artifact"), item);
            }
        }
        done += weight(stage);
        ctx.set_remote_task(None);
    }

    // no export (shape only): the last stage's preview GLB
    let mesh = have.get("mesh").or_else(|| have.get("preview")).ok_or_else(|| JobError("The device returned no mesh".into()))?;
    let out = save(&ctx, &format!("{}-model.glb", ctx.id), &mesh.1)?;
    let mut params = Map::new();
    params.insert("model".into(), json!(model));
    params.extend(quality.iter().map(|(k, v)| (k.to_string(), v.clone())));
    params.insert("texture".into(), json!(on("texture")));
    params.insert("faces".into(), req.get("faces").cloned().unwrap_or(Value::Null));
    params.insert("remove_bg".into(), json!(on("remove_bg")));
    params.insert("cutout".into(), if cut.is_some() { req.get("cutout_model").cloned().unwrap_or(Value::Null) } else { Value::Null });
    let sources: Vec<String> = std::iter::once(src).chain(cut).collect();
    Ok(json!({"kind": "mesh", "params": params, "sources": sources, "outputs": [out], "stages": stage_log}))
}
