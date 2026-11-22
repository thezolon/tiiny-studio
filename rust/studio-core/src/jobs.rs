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

use crate::settings::Target;
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
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

struct Job {
    data: Map<String, Value>,
    cancel: Arc<AtomicBool>,
    remote_task: Option<String>,
    /// The device this job runs on (its cancel and halt go there, whatever is active by then).
    target: Target,
    /// The task that stops the device's work after a cancel (see halt_device); the job waits for it before it lets go
    /// of the device lock, so the next job doesn't start while a model is still being reloaded.
    halt: Option<tokio::task::JoinHandle<()>>,
}

/// What a cancel has to tell the device: the job's remote task, and the kind and model to halt (a running job only).
struct CancelPlan {
    remote: Option<String>,
    halt: Option<(String, String)>,
    target: Target,
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
    /// The device address and key as they were when the job was submitted. Every request the job makes goes there,
    /// so an auto-switch mid-job never sends one device's key to another.
    pub target: Target,
    /// Every file the job has saved (media::save): all of them are shredded if it fails or is cancelled, since no
    /// gallery entry would point at them. A tool run inside a chat job keeps its own list (its result is saved at once).
    pub(crate) saved: Arc<Mutex<Vec<String>>>,
    pub(crate) cancel: Arc<AtomicBool>,
    /// Set for a chat tool's inline job: its message/progress show on the chat job as tool_message/tool_progress.
    pub(crate) parent: Option<String>,
}

/// Stamp a finished job's history entry (id, created, kind, seconds); answers the entry's kind.
pub(crate) fn stamp(e: &mut Map<String, Value>, id: &str, kind: &str, started: f64) -> String {
    let k = e
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or(kind)
        .to_string();
    e.insert("id".into(), json!(id));
    e.insert("created".into(), json!(now() as i64));
    e.insert("kind".into(), json!(k));
    e.insert(
        "seconds".into(),
        json!(((now() - started) * 10.0).round() / 10.0),
    );
    k
}

impl Ctx {
    /// `device::call` on this job's device.
    pub async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        timeout: u64,
    ) -> (u16, Value) {
        crate::device::call(
            &self.state,
            method,
            path,
            body,
            timeout,
            Some((&self.target.url, &self.target.key)),
        )
        .await
    }

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

    /// Shred the files this job saved (it failed or was cancelled).
    pub fn discard_saved(&self) {
        discard(&self.state, &self.saved);
    }

    /// The same job context with a list of saved files of its own.
    pub fn own_files(mut self) -> Ctx {
        self.saved = Arc::default();
        self
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
        self.state
            .jobs
            .lock()
            .get(&self.id)
            .and_then(|j| j.data.get(key).cloned())
    }

    /// A tool's job run inline inside this one (chat): fresh id, own request, never sees this job's cancel.
    pub fn sub(&self, model: &str, req: Value) -> Ctx {
        Ctx {
            state: self.state.clone(),
            id: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(),
            model: model.to_string(),
            req,
            target: self.target.clone(),
            saved: self.saved.clone(), // mesh's cut-out is part of the build; chat gives its tools their own (own_files)
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

fn discard(state: &AppState, saved: &Mutex<Vec<String>>) {
    for f in std::mem::take(&mut *saved.lock().unwrap_or_else(|e| e.into_inner())) {
        if crate::store::safe_file(&f) {
            crate::store::shred(&state.store.images.join(f));
        }
    }
}

fn params_of(req: &Value) -> Value {
    // small, display-only copy of the request (prompt, size, …) so the UI can show queued jobs
    let mut out = Map::new();
    for (k, v) in req.as_object().into_iter().flatten() {
        // Python's isinstance(v, (int, float)) is true for booleans too
        let keep =
            v.is_number() || v.is_boolean() || v.as_str().is_some_and(|s| s.chars().count() < 2000);
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
    /// A context for the plumbing tests in media/: not in the table, no task behind it.
    #[cfg(test)]
    pub(crate) fn test_ctx(&self, state: &AppState) -> Ctx {
        Ctx {
            state: state.clone(),
            id: "t".into(),
            model: String::new(),
            req: json!({}),
            target: Target::default(),
            saved: Arc::default(),
            cancel: Arc::new(AtomicBool::new(false)),
            parent: None,
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Job>> {
        self.map.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn get(&self, id: &str) -> Option<Value> {
        let map = self.lock();
        let j = map.get(id)?;
        let mut v = view(j);
        if Self::status_of(j) == "queued" {
            let created = |j: &Job| j.data.get("created").and_then(Value::as_f64).unwrap_or(0.0);
            let ahead = map
                .values()
                .filter(|o| Self::status_of(o) == "queued" && created(o) < created(j))
                .count();
            v["queue_pos"] = json!(ahead + 1); // the same place Jobs::list shows
        }
        Some(v)
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
        self.lock()
            .values()
            .any(|j| matches!(Self::status_of(j), "queued" | "running"))
    }

    /// A queued or running job uses this model: as its own, as the tool model of a chat's inline sub-job, or (a chat
    /// still waiting, queued as "default") as whatever model that chat is about to pick.
    pub fn active_on(&self, model: &str) -> bool {
        self.lock().values().any(|j| {
            let status = Self::status_of(j);
            let own = j.data.get("model").and_then(Value::as_str);
            let tool = j
                .data
                .get("tool")
                .and_then(|t| t.get("model"))
                .and_then(Value::as_str);
            matches!(status, "queued" | "running")
                && (own == Some(model)
                    || tool == Some(model)
                    || (status == "queued" && own == Some("default")))
        })
    }

    pub fn running_on(&self, model: &str) -> bool {
        self.lock().values().any(|j| {
            j.data.get("model").and_then(Value::as_str) == Some(model)
                && Self::status_of(j) == "running"
        })
    }

    /// This job is queued or running.
    pub fn is_active(&self, id: &str) -> bool {
        self.lock()
            .get(id)
            .is_some_and(|j| matches!(Self::status_of(j), "queued" | "running"))
    }

    pub fn remove(&self, id: &str) {
        self.lock().remove(id);
    }

    pub fn clear(&self) {
        self.lock().clear();
    }

    pub fn submit(&self, state: &AppState, kind: &str, req: Value) -> Result<Value, JobError> {
        let run =
            crate::media::kind(kind).ok_or_else(|| JobError(format!("unknown job kind {kind}")))?;
        let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        let model = req
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let label = req
            .get("label")
            .and_then(Value::as_str)
            .filter(|l| !l.is_empty())
            .unwrap_or(kind);
        let data = json!({
            "id": id, "kind": kind, "model": model, "status": "queued", "progress": 0, "message": "Waiting for the model",
            "created": now(), "started": null, "label": label, "entry": null, "error": null,
            "est": req.get("est").cloned().unwrap_or(Value::Null), "params": params_of(&req),
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let target = state.settings.current();
        let job = Job {
            data: data.as_object().cloned().unwrap_or_default(),
            cancel: cancel.clone(),
            remote_task: None,
            target: target.clone(),
            halt: None,
        };
        let out = view(&job);
        {
            let mut map = self.lock();
            Self::prune(&mut map);
            map.insert(id.clone(), job);
        }
        let ctx = Ctx {
            state: state.clone(),
            id,
            model,
            req,
            target,
            saved: Arc::default(),
            cancel,
            parent: None,
        };
        tokio::spawn(work(ctx, kind.to_string(), run));
        Ok(out)
    }

    /// Forget jobs that finished more than ten minutes ago.
    fn prune(map: &mut HashMap<String, Job>) {
        let cutoff = now() - 600.0;
        map.retain(|_, j| {
            let done = matches!(Self::status_of(j), "done" | "error" | "cancelled");
            let t = j
                .data
                .get("finished")
                .or(j.data.get("created"))
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            !(done && t < cutoff)
        });
    }

    pub fn list(&self) -> Vec<Value> {
        let mut map = self.lock();
        Self::prune(&mut map);
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

    /// Mark a job cancelled and say what the device has to be told; None for an unknown id. A queued job is finished
    /// off at once; a running one is told to stop (its result is thrown away when the device answers).
    fn begin_cancel(&self, id: &str) -> Option<CancelPlan> {
        let mut map = self.lock();
        let j = map.get_mut(id)?;
        j.cancel.store(true, Ordering::Relaxed);
        let running = Self::status_of(j) == "running";
        let first = running && !j.data.contains_key("stopping");
        if running {
            j.data.insert("stopping".into(), json!(true));
        }
        let s = |k: &str| {
            j.data
                .get(k)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let halt = first.then(|| (s("kind"), s("model"))); // once, however often Stop is pressed
        if Self::status_of(j) == "queued" {
            j.data.insert("status".into(), json!("cancelled"));
            j.data.insert("message".into(), json!("Cancelled"));
        }
        Some(CancelPlan {
            remote: j.remote_task.clone(),
            halt,
            target: j.target.clone(),
        })
    }

    /// Ask a job to stop; returns false for an unknown id.
    pub async fn cancel(&self, state: &AppState, id: &str) -> bool {
        let Some(plan) = self.begin_cancel(id) else {
            return false;
        };
        cancel_remote(state, &plan).await;
        if let Some((kind, model)) = plan.halt {
            let task = tokio::spawn(halt_device(state.clone(), plan.target, kind, model));
            // kept on the job: it waits for the halt before the next job gets the device
            if let Some(j) = self.lock().get_mut(id) {
                j.halt = Some(task);
            }
        }
        true
    }

    /// The halt task a cancel started for this job, if any (taken, so it is awaited once).
    fn take_halt(&self, id: &str) -> Option<tokio::task::JoinHandle<()>> {
        self.lock().get_mut(id).and_then(|j| j.halt.take())
    }
}

/// POST to the job's remote task, or "DELETE <url>" (a video): a 3D task or video the device is working on.
async fn cancel_remote(state: &AppState, plan: &CancelPlan) {
    if let Some(task) = &plan.remote {
        let req = match task.strip_prefix("DELETE ") {
            Some(url) => state.http.delete(url),
            None => state.http.post(task).body(Vec::new()),
        };
        let _ = req
            .bearer_auth(&plan.target.key)
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await;
    }
}

impl Jobs {
    /// On shutdown: stop every job and wait (up to `wait`) until the device has been told, so a render doesn't carry
    /// on invisibly after the studio is gone (the Tiiny ignores a dropped connection) and a queued job doesn't start
    /// the moment the running one lets go. Queued jobs are finished first; the running one is stopped like Stop does it,
    /// remote task (3D, video) included.
    pub async fn cancel_running(&self, state: &AppState, wait: std::time::Duration) {
        let ids: Vec<String> = self
            .lock()
            .iter()
            .filter(|(_, j)| matches!(Self::status_of(j), "queued" | "running"))
            .map(|(id, _)| id.clone())
            .collect();
        // every flag is set before anything is awaited: a job that gets the device lock now sees its cancel
        let plans: Vec<CancelPlan> = ids.iter().filter_map(|id| self.begin_cancel(id)).collect();
        let _ = tokio::time::timeout(wait, async {
            for plan in plans {
                cancel_remote(state, &plan).await;
                if let Some((kind, model)) = plan.halt {
                    halt_device(state.clone(), plan.target, kind, model).await;
                }
            }
        })
        .await;
    }
}

/// Make the Tiiny stop a request now instead of finishing it: chat replies can be interrupted; an image only stops
/// when its model is unloaded (~10 s), so it's unloaded and loaded straight back (~15 s, tested 0.1.36).
async fn halt_device(state: AppState, target: Target, kind: String, model: String) {
    let to = Some((target.url.as_str(), target.key.as_str()));
    if model.is_empty() {
        return;
    }
    let q: String = model
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"_.-~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    match kind.as_str() {
        "chat" | "assist" => {
            crate::device::call(
                &state,
                "POST",
                &format!("/api/v1/models/{q}/interrupt"),
                Some(json!({})),
                15,
                to,
            )
            .await;
        }
        "image" => {
            crate::device::call(
                &state,
                "POST",
                &format!("/api/v1/models/{q}/stop"),
                None,
                120,
                to,
            )
            .await;
            crate::media::launch_on(&state, (&target.url, &target.key), &model, || false).await;
        }
        _ => {}
    }
}

async fn work(ctx: Ctx, kind: String, run: KindFn) {
    let state = ctx.state.clone();
    let id = ctx.id.clone();
    state
        .jobs
        .update(&id, json!({"message": "Waiting in the device queue"}));
    {
        let _device = state.device_lock.lock().await;
        if ctx.cancelled() {
            state
                .jobs
                .update(&id, json!({"status": "cancelled", "message": "Cancelled"}));
        } else {
            let started = now();
            state.jobs.update(
                &id,
                json!({"status": "running", "started": started, "message": "Working…"}),
            );
            let ephemeral = ctx.req.get("ephemeral").is_some_and(crate::device::truthy);
            let cancel = ctx.cancel.clone();
            let saved = ctx.saved.clone();
            // a panicking kind must not take the worker down: surface it as an error instead
            // kinds that make something on the device (results are written there first) need room for it
            let full = if matches!(
                kind.as_str(),
                "image" | "speech" | "transcribe" | "loop" | "song" | "mesh" | "video" | "ocr"
            ) {
                crate::device::disk_space(&state)
                    .await
                    .filter(|(free, _)| *free < 2.0)
            } else {
                None
            };
            let result = match full {
                Some((free, _)) => Ok(Err(JobError(format!(
                    "The Tiiny is almost out of space ({free:.1} GB free). Free some up (delete a model you don't use in Models → Installed, or files in the TiinyOS app) and try again."
                )))),
                None => {
                    // queued for a model that wasn't loaded (#78): load it now, unloading others only if it doesn't fit
                    let load = ctx.req.get("load").is_some_and(crate::device::truthy)
                        && state.settings.swap_allowed("queue");
                    tokio::spawn(async move {
                        if load {
                            let model = ctx.model.clone();
                            crate::media::make_room(&ctx, &model).await?;
                            ctx.update(json!({"message": "Working…"}));
                        }
                        run(ctx).await
                    })
                    .await
                }
            };
            match result {
                Ok(Ok(mut entry)) => {
                    let mut e = entry
                        .as_object_mut()
                        .map(std::mem::take)
                        .unwrap_or_default();
                    let k = stamp(&mut e, &id, &kind, started);
                    let entry = Value::Object(e);
                    if cancel.load(Ordering::Relaxed) {
                        // stopped while the device was still working: throw the result away
                        for f in crate::store::entry_files(&entry) {
                            if crate::store::safe_file(&f) {
                                crate::store::shred(&state.store.images.join(f));
                            }
                        }
                        discard(&state, &saved);
                        state
                            .jobs
                            .update(&id, json!({"status": "cancelled", "message": "Cancelled"}));
                        halted(&state, &id).await;
                        state.jobs.update(&id, json!({"finished": now()}));
                        return;
                    }
                    // helper text and chat replies live with the job, not in the gallery; throwaway jobs never touch disk
                    if !matches!(k.as_str(), "assist" | "chat") && !ephemeral {
                        crate::media::tags::embed(
                            &state.store.images,
                            &entry,
                            state.settings.tag_prompts(),
                        ); // the prompt inside the files (#58)
                        if let Err(e) = state.store.add_history(entry.clone()) {
                            let msg = format!("Couldn't save the result: {e}");
                            discard(&state, &saved);
                            state.jobs.update(
                                &id,
                                json!({"status": "error", "error": msg, "message": msg}),
                            );
                            halted(&state, &id).await;
                            state.jobs.update(&id, json!({"finished": now()}));
                            return;
                        }
                    }
                    state.jobs.update(&id, json!({"status": "done", "progress": 100, "message": "Done", "entry": entry}));
                }
                Ok(Err(JobError(msg))) => {
                    discard(&state, &saved); // a failed job keeps nothing it saved
                    if cancel.load(Ordering::Relaxed) {
                        // the device's error is just the cancel taking effect
                        state
                            .jobs
                            .update(&id, json!({"status": "cancelled", "message": "Cancelled"}));
                    } else {
                        state.jobs.update(
                            &id,
                            json!({"status": "error", "error": msg, "message": msg}),
                        );
                    }
                }
                Err(e) => {
                    discard(&state, &saved);
                    let msg = format!("{e}");
                    state.jobs.update(&id, json!({"status": "error", "error": format!("panic: {msg}"), "message": msg}));
                }
            }
            halted(&state, &id).await;
        }
    }
    state.jobs.update(&id, json!({"finished": now()}));
}

/// Wait for the halt a cancel started (an image model being unloaded and loaded back), while the job still holds the
/// device lock: the next job must not start against a model that is still loading.
async fn halted(state: &AppState, id: &str) {
    if let Some(task) = state.jobs.take_halt(id) {
        let _ = task.await;
    }
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
            data_dir: std::env::temp_dir()
                .join(format!("studio-jobs-{}", uuid::Uuid::new_v4().simple())),
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

    fn job_with(data: Value) -> Job {
        Job {
            data: data.as_object().cloned().unwrap_or_default(),
            cancel: Arc::new(AtomicBool::new(false)),
            remote_task: None,
            target: Target::default(),
            halt: None,
        }
    }

    #[test]
    fn a_chat_blocks_its_tool_model_and_a_queued_default_chat_blocks_any() {
        let jobs = Jobs::default();
        let put = |id: &str, data: Value| jobs.lock().insert(id.into(), job_with(data));
        put(
            "run",
            json!({"status": "running", "model": "org/chat", "tool": {"name": "create_image", "model": "org/img"}}),
        );
        assert!(jobs.active_on("org/chat") && jobs.active_on("org/img"));
        assert!(!jobs.active_on("org/other"));
        jobs.remove("run");
        put("wait", json!({"status": "queued", "model": "default"}));
        assert!(
            jobs.active_on("org/img"),
            "a queued default chat may pick any model"
        );
        jobs.remove("wait");
        put("done", json!({"status": "done", "model": "default"}));
        assert!(!jobs.active_on("org/img"));
    }

    #[tokio::test]
    async fn runs_in_turn_and_saves_history() {
        let s = state();
        let a = s
            .jobs
            .submit(
                &s,
                "test",
                json!({"model": "m", "ms": 100, "prompt": "duck"}),
            )
            .unwrap();
        let b = s
            .jobs
            .submit(&s, "test", json!({"model": "m", "ms": 10, "as": "assist"}))
            .unwrap();
        assert_eq!(a["status"], json!("queued"));
        assert_eq!(a["params"]["prompt"], json!("duck"));
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        let list = s.jobs.list();
        assert_eq!(list[0]["status"], json!("running"));
        assert_eq!(list[1]["queue_pos"], json!(1));
        assert!(s.jobs_active() && s.jobs.active_on("m") && s.jobs_running_on("m"));
        let a = wait_done(&s, a["id"].as_str().unwrap()).await;
        let b = wait_done(&s, b["id"].as_str().unwrap()).await;
        assert_eq!(
            (a["status"].as_str(), b["status"].as_str()),
            (Some("done"), Some("done"))
        );
        assert_eq!(a["entry"]["id"], a["id"]);
        // assist results stay with the job; others go to the gallery
        let history = s.store.load_history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0]["id"], a["id"]);
        assert!(!s.jobs_active());
    }

    #[tokio::test]
    async fn a_job_keeps_the_device_it_was_submitted_to() {
        let s = state();
        let a = s
            .settings
            .upsert(None, "A", "http://10.0.0.1", "key-a")
            .unwrap();
        let b = s
            .settings
            .upsert(None, "B", "http://10.0.0.2", "key-b")
            .unwrap();
        s.settings.activate(&a, None);
        let job = s
            .jobs
            .submit(&s, "test", json!({"model": "m", "ms": 50}))
            .unwrap();
        let id = job["id"].as_str().unwrap();
        s.settings.activate(&b, Some("A isn't answering".into()));
        assert_eq!(s.settings.current().key, "key-b");
        // the job (and so its cancel and every request it makes) stays on A with A's key
        let t = s.jobs.lock().get(id).unwrap().target.clone();
        assert_eq!(
            (t.url.as_str(), t.key.as_str()),
            ("http://10.0.0.1", "key-a")
        );
        wait_done(&s, id).await;
    }

    #[tokio::test]
    async fn a_failed_or_cancelled_job_keeps_no_files() {
        let s = state();
        let ok = s
            .jobs
            .submit(&s, "test", json!({"save": 1, "ephemeral": true}))
            .unwrap();
        let failed = s
            .jobs
            .submit(&s, "test", json!({"save": 1, "fail": 1}))
            .unwrap();
        let cancelled = s
            .jobs
            .submit(&s, "test", json!({"save": 1, "ms": 200, "ephemeral": true}))
            .unwrap();
        let (ok, failed, cancelled) = (
            ok["id"].as_str().unwrap(),
            failed["id"].as_str().unwrap(),
            cancelled["id"].as_str().unwrap(),
        );
        let has = |id: &str| {
            // results are named <date>_<time>_<id>-… (media::dated)
            let tail = format!("_{id}-x.png");
            std::fs::read_dir(&s.store.images)
                .unwrap()
                .flatten()
                .any(|f| f.file_name().to_string_lossy().ends_with(&tail))
        };
        wait_done(&s, ok).await;
        assert_eq!(wait_done(&s, failed).await["status"], json!("error"));
        assert!(has(ok) && !has(failed)); // what a finished job saved stays; a failed one's goes
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(s.jobs.cancel(&s, cancelled).await);
        assert_eq!(wait_done(&s, cancelled).await["status"], json!("cancelled"));
        assert!(!has(cancelled));
    }

    #[tokio::test]
    async fn errors_cancel_and_unknown_kinds() {
        let s = state();
        assert_eq!(
            s.jobs.submit(&s, "nope", json!({})).unwrap_err().0,
            "unknown job kind nope"
        );
        let blocker = s.jobs.submit(&s, "test", json!({"ms": 150})).unwrap();
        let queued = s.jobs.submit(&s, "test", json!({})).unwrap();
        let failing = s
            .jobs
            .submit(&s, "test", json!({"fail": 1, "ephemeral": true}))
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(s.jobs.cancel(&s, queued["id"].as_str().unwrap()).await);
        assert!(!s.jobs.cancel(&s, "missing").await);
        assert_eq!(
            s.jobs.get(queued["id"].as_str().unwrap()).unwrap()["status"],
            json!("cancelled")
        );
        assert!(s.jobs.cancel(&s, blocker["id"].as_str().unwrap()).await);
        assert_eq!(
            wait_done(&s, blocker["id"].as_str().unwrap()).await["status"],
            json!("cancelled")
        );
        let f = wait_done(&s, failing["id"].as_str().unwrap()).await;
        assert_eq!(
            (f["status"].as_str(), f["error"].as_str()),
            (Some("error"), Some("it failed"))
        );
        assert!(s.store.load_history().is_empty());
    }

    async fn wait_status(s: &AppState, id: &str, status: &str) {
        for _ in 0..500 {
            if s.jobs.get(id).unwrap()["status"] == json!(status) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("job {id} never became {status}");
    }

    #[tokio::test]
    async fn shutdown_stops_the_queue_and_the_remote_task() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/tasks/1", l.local_addr().unwrap());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = l.accept().await.unwrap();
                let mut buf = vec![0u8; 2048];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let _ = tx.send(String::from_utf8_lossy(&buf[..n]).to_string());
                let _ = sock
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                    .await;
            }
        });
        let s = state();
        let a = s.jobs.submit(&s, "test", json!({"ms": 1000})).unwrap();
        let b = s.jobs.submit(&s, "test", json!({"ms": 20})).unwrap();
        let c = s.jobs.submit(&s, "test", json!({"ms": 20})).unwrap();
        let (a, b, c) = (
            a["id"].as_str().unwrap(),
            b["id"].as_str().unwrap(),
            c["id"].as_str().unwrap(),
        );
        wait_status(&s, a, "running").await;
        s.jobs.lock().get_mut(a).unwrap().remote_task = Some(format!("DELETE {url}"));
        s.jobs
            .cancel_running(&s, std::time::Duration::from_secs(5))
            .await;
        for id in [a, b, c] {
            assert_eq!(
                wait_done(&s, id).await["status"],
                json!("cancelled"),
                "{id}"
            );
        }
        // the queued ones never reached the device
        assert!(s.jobs.get(b).unwrap()["started"].is_null());
        assert!(s.jobs.get(c).unwrap()["started"].is_null());
        // the running one's remote task (a video or 3D render) was cancelled too
        let req = rx.recv().await.unwrap();
        assert!(req.starts_with("DELETE /tasks/1 "), "{req}");
    }

    #[tokio::test]
    async fn a_single_job_lookup_shows_its_place_in_the_queue() {
        let s = state();
        let a = s.jobs.submit(&s, "test", json!({"ms": 200})).unwrap();
        let b = s.jobs.submit(&s, "test", json!({"ms": 10})).unwrap();
        let c = s.jobs.submit(&s, "test", json!({"ms": 10})).unwrap();
        let (a, b, c) = (
            a["id"].as_str().unwrap(),
            b["id"].as_str().unwrap(),
            c["id"].as_str().unwrap(),
        );
        wait_status(&s, a, "running").await;
        assert!(s.jobs.get(a).unwrap().get("queue_pos").is_none());
        assert_eq!(s.jobs.get(b).unwrap()["queue_pos"], json!(1));
        assert_eq!(s.jobs.get(c).unwrap()["queue_pos"], json!(2));
        wait_done(&s, c).await;
    }

    #[tokio::test]
    async fn a_job_waits_for_its_halt_before_it_lets_go_of_the_device() {
        let s = state();
        let job = s.jobs.submit(&s, "test", json!({"ms": 10})).unwrap();
        let id = job["id"].as_str().unwrap();
        wait_done(&s, id).await;
        let flag = Arc::new(AtomicBool::new(false));
        let f = flag.clone();
        let task = tokio::spawn(async move {
            tokio::task::yield_now().await;
            f.store(true, Ordering::SeqCst);
        });
        s.jobs.lock().get_mut(id).unwrap().halt = Some(task);
        halted(&s, id).await;
        assert!(flag.load(Ordering::SeqCst));
        assert!(s.jobs.take_halt(id).is_none()); // awaited once
    }
}
