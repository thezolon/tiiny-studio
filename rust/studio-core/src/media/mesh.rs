//! Hunyuan3D image-to-3D: [background] -> shape -> [simplify] -> [texture -> export], chaining artifacts
//! (media.py run_mesh).

use std::time::Duration;

use base64::Engine;
use serde_json::{json, Map, Value};

use super::{decode_uri, ensure_running, err, http, http_bytes, json_reply, multipart, py_json, py_round, save};
use crate::device::truthy;
use crate::jobs::{Ctx, JobError, JobResult};

/// Which multipart fields each Hunyuan3D stage takes (from /v1/capabilities).
fn stage_inputs(stage: &str) -> &'static [&'static str] {
    match stage {
        "background" | "shape" => &["image"],
        "simplify" => &["mesh"],
        "texture" => &["image", "mesh"],
        _ => &["material", "mesh"], // export
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

pub async fn run(ctx: Ctx) -> JobResult {
    let req = &ctx.req;
    let (_, image) = decode_uri(req.get("image").and_then(Value::as_str).unwrap_or(""))?;
    let model = ctx.model.clone();
    ensure_running(&ctx, &model).await?;
    let url = format!("{}/v1/3d", ctx.state.settings.current().url); // gateway proxy: /v1/3d/tasks -> container /v1/tasks
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
    let src = save(&ctx, &format!("{}-src.png", ctx.id), &image);

    for stage in plan {
        if ctx.cancelled() {
            return Err(JobError("Cancelled".into()));
        }
        let params = match stage {
            "simplify" => {
                let faces = &req["faces"];
                let n = match faces {
                    Value::String(s) => s.trim().parse::<i64>().map_err(|_| JobError(format!("ValueError: invalid literal for int() with base 10: '{s}'")))?,
                    other => other.as_f64().unwrap_or(1.0) as i64,
                };
                dumps(&[("target_faces", &json!(n))])
            }
            "background" => "{}".to_string(),
            _ => dumps(&quality.iter().map(|(k, v)| (*k, v)).collect::<Vec<_>>()),
        };
        let files: Vec<(&str, (&str, &[u8], &str))> =
            have.0.iter().filter(|(k, _)| stage_inputs(stage).contains(&k.as_str())).map(|(k, (n, d, m))| (k.as_str(), (n.as_str(), d.as_slice(), m.as_str()))).collect();
        let (body, mp) = multipart(&[("stage", stage.to_string()), ("params", params)], &files);
        let (code, res) = http_bytes(&ctx, "POST", &format!("{url}/tasks"), Some((body, mp)), 3600, true).await?;
        let (code, res) = json_reply(code, res)?;
        if code >= 300 {
            return Err(JobError(format!("{stage}: {}", err(&res))));
        }
        let task = res.get("data").ok_or_else(|| JobError("KeyError: 'data'".into()))?.get("task_id").ok_or_else(|| JobError("KeyError: 'task_id'".into()))?;
        let tid = match task {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        ctx.set_remote_task(Some(format!("{url}/tasks/{tid}/cancel")));
        let mut silent = 0; // polls in a row without a status: the task is gone, don't hold the device forever
        let d = loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let (_, st) = http(&ctx, "GET", &format!("{url}/tasks/{tid}"), None, 30, true).await?;
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
            if silent >= 30 {
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
            let Some(payload) = out.get(str_of(art, "payload_key").unwrap_or("")).and_then(Value::as_str) else { continue };
            let data = base64::engine::general_purpose::STANDARD.decode(payload).map_err(|e| JobError(format!("Error: {e}")))?;
            let kind = str_of(art, "artifact_type").unwrap_or("");
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
            }
        }
        done += weight(stage);
        ctx.set_remote_task(None);
    }

    let mesh = have.get("mesh").ok_or_else(|| JobError("The device returned no mesh".into()))?;
    let out = save(&ctx, &format!("{}-model.glb", ctx.id), &mesh.1);
    let mut params = Map::new();
    params.insert("model".into(), json!(model));
    params.extend(quality.iter().map(|(k, v)| (k.to_string(), v.clone())));
    params.insert("texture".into(), json!(on("texture")));
    params.insert("faces".into(), req.get("faces").cloned().unwrap_or(Value::Null));
    params.insert("remove_bg".into(), json!(on("remove_bg")));
    Ok(json!({"kind": "mesh", "params": params, "sources": [src], "outputs": [out], "stages": stage_log}))
}
