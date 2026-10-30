//! The job table (media.py Media.submit/_work/list/view/cancel). Each job runs as its own task so long work
//! (songs, 3D) survives page reloads; the UI polls /api/jobs. Every job waits its turn for the device lock:
//! the Tiiny serves one request at a time.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::AppState;

#[derive(Debug)]
pub struct JobError(pub String);

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub type JobResult = Result<Value, JobError>;
pub type KindFn = fn(Ctx) -> Pin<Box<dyn Future<Output = JobResult> + Send>>;

pub fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

struct Job {
    data: Map<String, Value>,
    cancel: Arc<AtomicBool>,
    remote_task: Option<String>,
}

#[derive(Default)]
pub struct Jobs {
    map: Mutex<HashMap<String, Job>>,
}

/// What a job kind gets: the app, its own job id and request, and ways to report progress.
#[derive(Clone)]
pub struct Ctx {
    pub state: AppState,
    pub id: String,
    pub model: String,
    pub req: Value,
    pub(crate) cancel: Arc<AtomicBool>,
    /// Set for a chat tool's inline job: its message/progress show on the chat job as tool_message/tool_progress.
    pub(crate) parent: Option<String>,
}

impl Ctx {
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Update display fields (progress, message, …) of the running job.
    pub fn update(&self, fields: Value) {
        match &self.parent {
            None => self.state.jobs.update(&self.id, fields),
            Some(parent) => {
                let mut out = Map::new();
                for (k, v) in fields.as_object().into_iter().flatten() {
                    if k == "message" || k == "progress" {
                        out.insert(format!("tool_{k}"), v.clone());
                    }
                }
                self.state.jobs.update(parent, Value::Object(out));
            }
        }
    }

    /// Drop fields from the job (Python's job.pop).
    pub fn remove(&self, keys: &[&str]) {
        if let Some(j) = self.state.jobs.lock().get_mut(&self.id) {
            for k in keys {
                j.data.remove(*k);
            }
        }
    }

    /// The job's current value of one field.
    pub fn get(&self, key: &str) -> Option<Value> {
        self.state.jobs.lock().get(&self.id).and_then(|j| j.data.get(key).cloned())
    }

    /// A tool's job run inline inside this one (chat): fresh id, own request, never sees this job's cancel.
    pub fn sub(&self, model: &str, req: Value) -> Ctx {
        Ctx {
            state: self.state.clone(),
            id: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(),
            model: model.to_string(),
            req,
            cancel: Arc::new(AtomicBool::new(false)),
            parent: Some(self.id.clone()),
        }
    }

    /// A remote task the device can cancel (POSTed to on cancel, or "DELETE <url>", like media.py remote_task).
    pub fn set_remote_task(&self, url: Option<String>) {
        if let Some(j) = self.state.jobs.lock().get_mut(&self.id) {
            j.remote_task = url;
        }
    }
}

fn params_of(req: &Value) -> Value {
    // small, display-only copy of the request (prompt, size, …) so the UI can show queued jobs
    let mut out = Map::new();
    for (k, v) in req.as_object().into_iter().flatten() {
        // Python's isinstance(v, (int, float)) is true for booleans too
        let keep = v.is_number() || v.is_boolean() || v.as_str().is_some_and(|s| s.chars().count() < 2000);
        if keep {
            out.insert(k.clone(), v.clone());
        }
    }
    Value::Object(out)
}

fn view(job: &Job) -> Value {
    Value::Object(job.data.clone())
}

impl Jobs {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Job>> {
        self.map.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn get(&self, id: &str) -> Option<Value> {
        self.lock().get(id).map(view)
    }

    pub fn update(&self, id: &str, fields: Value) {
        if let Some(j) = self.lock().get_mut(id) {
            for (k, v) in fields.as_object().into_iter().flatten() {
                j.data.insert(k.clone(), v.clone());
            }
        }
    }

    fn status_of(j: &Job) -> &str {
        j.data.get("status").and_then(Value::as_str).unwrap_or("")
    }

    /// Any job queued or running (blocks unload-all, power off, wipe).
    pub fn any_active(&self) -> bool {
        self.lock().values().any(|j| matches!(Self::status_of(j), "queued" | "running"))
    }

    pub fn active_on(&self, model: &str) -> bool {
        self.lock().values().any(|j| j.data.get("model").and_then(Value::as_str) == Some(model) && matches!(Self::status_of(j), "queued" | "running"))
    }

    pub fn running_on(&self, model: &str) -> bool {
        self.lock().values().any(|j| j.data.get("model").and_then(Value::as_str) == Some(model) && Self::status_of(j) == "running")
    }

    pub fn remove(&self, id: &str) {
        self.lock().remove(id);
    }

    pub fn clear(&self) {
        self.lock().clear();
    }

    pub fn submit(&self, state: &AppState, kind: &str, req: Value) -> Result<Value, JobError> {
        let run = crate::media::kind(kind).ok_or_else(|| JobError(format!("unknown job kind {kind}")))?;
        let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        let model = req.get("model").and_then(Value::as_str).unwrap_or("").to_string();
        let label = req.get("label").and_then(Value::as_str).filter(|l| !l.is_empty()).unwrap_or(kind);
        let data = json!({
            "id": id, "kind": kind, "model": model, "status": "queued", "progress": 0, "message": "Waiting for the model",
            "created": now(), "started": null, "label": label, "entry": null, "error": null,
            "est": req.get("est").cloned().unwrap_or(Value::Null), "params": params_of(&req),
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let job = Job { data: data.as_object().cloned().unwrap_or_default(), cancel: cancel.clone(), remote_task: None };
        let out = view(&job);
        self.lock().insert(id.clone(), job);
        let ctx = Ctx { state: state.clone(), id, model, req, cancel, parent: None };
        tokio::spawn(work(ctx, kind.to_string(), run));
        Ok(out)
    }

    pub fn list(&self) -> Vec<Value> {
        let cutoff = now() - 600.0;
        let mut map = self.lock();
        map.retain(|_, j| {
            let done = matches!(Self::status_of(j), "done" | "error" | "cancelled");
            let t = j.data.get("finished").or(j.data.get("created")).and_then(Value::as_f64).unwrap_or(0.0);
            !(done && t < cutoff)
        });
        let mut jobs: Vec<&Job> = map.values().collect();
        jobs.sort_by(|a, b| {
            let c = |j: &Job| j.data.get("created").and_then(Value::as_f64).unwrap_or(0.0);
            c(a).partial_cmp(&c(b)).unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut pos = 0;
        jobs.into_iter()
            .map(|j| {
                let mut v = view(j);
                if Self::status_of(j) == "queued" {
                    pos += 1;
                    v["queue_pos"] = json!(pos);
                }
                v
            })
            .collect()
    }

    /// Ask a job to stop; returns false for an unknown id.
    pub async fn cancel(&self, state: &AppState, id: &str) -> bool {
        let (remote, halt) = {
            let mut map = self.lock();
            let Some(j) = map.get_mut(id) else { return false };
            j.cancel.store(true, Ordering::Relaxed);
            let running = Self::status_of(j) == "running";
            if running {
                j.data.insert("stopping".into(), json!(true)); // the result is thrown away when the device answers
            }
            let s = |k: &str| j.data.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let halt = running.then(|| (s("kind"), s("model")));
            if Self::status_of(j) == "queued" {
                j.data.insert("status".into(), json!("cancelled"));
                j.data.insert("message".into(), json!("Cancelled"));
            }
            (j.remote_task.clone(), halt)
        };
        if let Some(task) = remote {
            // POSTed to, or "DELETE <url>" (a video)
            let key = state.settings.current().key;
            let req = match task.strip_prefix("DELETE ") {
                Some(url) => state.http.delete(url),
                None => state.http.post(task).body(Vec::new()),
            };
            let _ = req.bearer_auth(key).timeout(std::time::Duration::from_secs(15)).send().await;
        }
        if let Some((kind, model)) = halt {
            tokio::spawn(halt_device(state.clone(), kind, model));
        }
        true
    }
}

impl Jobs {
    /// On shutdown: cancel every running job and wait (up to `wait`) until the device has been told to stop, so a
    /// render doesn't carry on invisibly after the studio is gone (the Tiiny ignores a dropped connection).
    pub async fn cancel_running(&self, state: &AppState, wait: std::time::Duration) {
        let running: Vec<(String, String, String)> = self
            .lock()
            .iter()
            .filter(|(_, j)| Self::status_of(j) == "running")
            .map(|(id, j)| {
                let s = |k: &str| j.data.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                (id.clone(), s("kind"), s("model"))
            })
            .collect();
        for (id, kind, model) in running {
            if let Some(j) = self.lock().get_mut(&id) {
                j.cancel.store(true, Ordering::Relaxed);
            }
            let _ = tokio::time::timeout(wait, halt_device(state.clone(), kind, model)).await;
        }
    }
}

/// Make the Tiiny stop a request now instead of finishing it: chat replies can be interrupted; an image only stops
/// when its model is unloaded (~10 s), so it's unloaded and loaded straight back (~15 s, tested 0.1.36).
async fn halt_device(state: AppState, kind: String, model: String) {
    if model.is_empty() {
        return;
    }
    let q: String = model.bytes().map(|b| if b.is_ascii_alphanumeric() || b"_.-~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    match kind.as_str() {
        "chat" | "assist" => {
            crate::device::call(&state, "POST", &format!("/api/v1/models/{q}/interrupt"), Some(json!({})), 15, None).await;
        }
        "image" => {
            crate::device::call(&state, "POST", &format!("/api/v1/models/{q}/stop"), None, 120, None).await;
            crate::device::call(&state, "POST", &format!("/api/v1/models/{q}/launch"), Some(json!({"model_id": model})), 120, None).await;
        }
        _ => {}
    }
}

async fn work(ctx: Ctx, kind: String, run: KindFn) {
    let state = ctx.state.clone();
    let id = ctx.id.clone();
    state.jobs.update(&id, json!({"message": "Waiting in the device queue"}));
    {
        let _device = state.device_lock.lock().await;
        if ctx.cancelled() {
            state.jobs.update(&id, json!({"status": "cancelled", "message": "Cancelled"}));
        } else {
            let started = now();
            state.jobs.update(&id, json!({"status": "running", "started": started, "message": "Working…"}));
            let ephemeral = ctx.req.get("ephemeral").is_some_and(crate::device::truthy);
            let cancel = ctx.cancel.clone();
            // a panicking kind must not take the worker down: surface it as an error instead
            let result = tokio::spawn(run(ctx)).await;
            match result {
                Ok(Ok(mut entry)) => {
                    let e = entry.as_object_mut().map(std::mem::take).unwrap_or_default();
                    let mut e = e;
                    let k = e.get("kind").and_then(Value::as_str).unwrap_or(&kind).to_string();
                    e.insert("id".into(), json!(id));
                    e.insert("created".into(), json!(now() as i64));
                    e.insert("kind".into(), json!(k));
                    e.insert("seconds".into(), json!(((now() - started) * 10.0).round() / 10.0));
                    let entry = Value::Object(e);
                    if cancel.load(Ordering::Relaxed) {
                        // stopped while the device was still working: throw the result away
                        for f in crate::store::entry_files(&entry) {
                            if crate::store::safe_file(&f) {
                                crate::store::shred(&state.store.images.join(f));
                            }
                        }
                        state.jobs.update(&id, json!({"status": "cancelled", "message": "Cancelled"}));
                        state.jobs.update(&id, json!({"finished": now()}));
                        return;
                    }
                    // helper text and chat replies live with the job, not in the gallery; throwaway jobs never touch disk
                    if !matches!(k.as_str(), "assist" | "chat") && !ephemeral {
                        state.store.add_history(entry.clone());
                    }
                    state.jobs.update(&id, json!({"status": "done", "progress": 100, "message": "Done", "entry": entry}));
                }
                Ok(Err(JobError(msg))) => {
                    if cancel.load(Ordering::Relaxed) {
                        // the device's error is just the cancel taking effect
                        state.jobs.update(&id, json!({"status": "cancelled", "message": "Cancelled"}));
                    } else {
                        state.jobs.update(&id, json!({"status": "error", "error": msg, "message": msg}));
                    }
                }
                Err(e) => {
                    let msg = format!("{e}");
                    state.jobs.update(&id, json!({"status": "error", "error": format!("panic: {msg}"), "message": msg}));
                }
            }
        }
    }
    state.jobs.update(&id, json!({"finished": now()}));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AppState {
        AppState::new(crate::Config {
            port: 0,
            host: "127.0.0.1".into(),
            password: String::new(),
            lan_access: false,
            app: false,
            data_dir: std::env::temp_dir().join(format!("studio-jobs-{}", uuid::Uuid::new_v4().simple())),
        })
    }

    async fn wait_done(s: &AppState, id: &str) -> Value {
        for _ in 0..200 {
            let j = s.jobs.get(id).unwrap();
            if j.get("finished").is_some() {
                return j;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("job {id} never finished");
    }

    #[tokio::test]
    async fn runs_in_turn_and_saves_history() {
        let s = state();
        let a = s.jobs.submit(&s, "test", json!({"model": "m", "ms": 100, "prompt": "duck"})).unwrap();
        let b = s.jobs.submit(&s, "test", json!({"model": "m", "ms": 10, "as": "assist"})).unwrap();
        assert_eq!(a["status"], json!("queued"));
        assert_eq!(a["params"]["prompt"], json!("duck"));
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        let list = s.jobs.list();
        assert_eq!(list[0]["status"], json!("running"));
        assert_eq!(list[1]["queue_pos"], json!(1));
        assert!(s.jobs_active() && s.jobs.active_on("m") && s.jobs_running_on("m"));
        let a = wait_done(&s, a["id"].as_str().unwrap()).await;
        let b = wait_done(&s, b["id"].as_str().unwrap()).await;
        assert_eq!((a["status"].as_str(), b["status"].as_str()), (Some("done"), Some("done")));
        assert_eq!(a["entry"]["id"], a["id"]);
        // assist results stay with the job; others go to the gallery
        let history = s.store.load_history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0]["id"], a["id"]);
        assert!(!s.jobs_active());
    }

    #[tokio::test]
    async fn errors_cancel_and_unknown_kinds() {
        let s = state();
        assert_eq!(s.jobs.submit(&s, "nope", json!({})).unwrap_err().0, "unknown job kind nope");
        let blocker = s.jobs.submit(&s, "test", json!({"ms": 150})).unwrap();
        let queued = s.jobs.submit(&s, "test", json!({})).unwrap();
        let failing = s.jobs.submit(&s, "test", json!({"fail": 1, "ephemeral": true})).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(s.jobs.cancel(&s, queued["id"].as_str().unwrap()).await);
        assert!(!s.jobs.cancel(&s, "missing").await);
        assert_eq!(s.jobs.get(queued["id"].as_str().unwrap()).unwrap()["status"], json!("cancelled"));
        assert!(s.jobs.cancel(&s, blocker["id"].as_str().unwrap()).await);
        assert_eq!(wait_done(&s, blocker["id"].as_str().unwrap()).await["status"], json!("cancelled"));
        let f = wait_done(&s, failing["id"].as_str().unwrap()).await;
        assert_eq!((f["status"].as_str(), f["error"].as_str()), (Some("error"), Some("it failed")));
        assert!(s.store.load_history().is_empty());
    }
}
