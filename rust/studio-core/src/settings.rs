//! Saved Tiiny devices (name, address and that device's own API key) in data/config.json, plus which one
//! is active. TIINY_URL/TIINY_KEY seed a first device when nothing is saved. Port of server.py Settings.
//!
//! With auto-switch on, a monitor probes every saved device (the unauthenticated status page at "/", so
//! no key is sent) and keeps the studio on the first one in the list that answers: it moves away when the
//! active device stops responding and back to a higher-priority one when that returns. A key is only ever
//! sent to the device it was saved with.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub key: String,
}

impl Device {
    /// The saved address passes `device_url` (checked every time, so an address saved by an older version or
    /// edited in config.json is covered too).
    pub fn usable(&self) -> bool {
        device_url(&self.url).is_ok()
    }
}

/// Said once at start-up for each saved address that isn't usable (see `Device::usable`).
fn warn_unusable(devices: &[Device]) {
    for d in devices.iter().filter(|d| !d.usable()) {
        eprintln!(
            "Device \"{}\" has an address that isn't usable (backslash, user name, query or odd scheme): it gets no key and no requests until it is corrected in Settings.",
            d.name
        );
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Probe {
    pub ok: bool,
    pub ms: Option<u64>,
}

/// Model swapping: may helpers and chat tools load what they need (and put the user's models back)? `on` is the
/// global switch; each section's switch only counts while it's on. All on by default.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Swap {
    #[serde(default = "yes")]
    pub on: bool,
    #[serde(default = "yes")]
    pub chat: bool,
    #[serde(default = "yes")]
    pub image: bool,
    #[serde(default = "yes")]
    pub audio: bool,
    #[serde(default = "yes")]
    pub mesh: bool,
    /// Queued jobs load their own model when their turn comes (#78).
    #[serde(default = "yes")]
    pub queue: bool,
}

fn new_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

fn yes() -> bool {
    true
}

impl Default for Swap {
    fn default() -> Self {
        Swap {
            on: true,
            chat: true,
            image: true,
            audio: true,
            mesh: true,
            queue: true,
        }
    }
}

/// The built-in MCP server (mcp.rs): off by default; its own bearer token, made the first time it is turned on.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct McpSettings {
    #[serde(default)]
    pub on: bool,
    #[serde(default)]
    pub token: String,
}

#[derive(Default)]
struct State {
    swap: Swap,
    mcp: McpSettings,
    helper_model: String,       // the text model helpers use ("" = automatic)
    update_check: Option<bool>, // "Check for updates": None until the user answers (counts as off)
    tag_prompts: Option<bool>,  // "Write prompts into files" (#58): None = on
    devices: Vec<Device>,
    active: Option<String>,
    auto: bool,
    source: &'static str,
    status: HashMap<String, Probe>,
    misses: HashMap<String, u32>, // probes in a row a device didn't answer (see LEAVE_AFTER_MISSES)
    pinned: Option<String>,       // a device picked by hand stays in use while it answers
    last_switch: Option<Value>,
    save_error: Option<String>, // why the last write of config.json failed
}

pub struct Settings {
    path: PathBuf,
    lan_access: bool,
    password: bool,
    app: bool,
    state: Mutex<State>,
}

/// Snapshot of the device requests go to.
#[derive(Clone, Debug, Default)]
pub struct Target {
    pub id: Option<String>,
    pub name: String,
    pub url: String,
    pub key: String,
}

impl Target {
    pub fn host(&self) -> String {
        hostname(&self.url)
    }
}

pub fn normalize_url(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    let lower = url.to_ascii_lowercase();
    if !url.is_empty() && !(lower.starts_with("http://") || lower.starts_with("https://")) {
        format!("http://{url}")
    } else {
        url.to_string()
    }
}

/// A device address as the HTTP client will read it, or why it is refused. The check uses the `url` crate, the
/// parser reqwest sends requests with, so the key can't go to a different host than the one this code sees
/// (a hand-written parser once read `http://a:80\@b` as host b while the request went to a). Refused: a
/// backslash, a user name or password in the address, a query or fragment, a scheme other than http(s).
/// Answers the address in its normal form (no trailing slash).
pub fn device_url(url: &str) -> Result<String, String> {
    let bad = |why: &str| Err(format!("That isn't a usable device address: {why}."));
    let url = normalize_url(url);
    if url.contains('\\') {
        return bad("it has a backslash");
    }
    if url.matches("://").count() != 1 || url.ends_with(':') {
        return bad("it must start with http:// or https://");
    }
    let Ok(u) = url::Url::parse(&url) else {
        return bad("it can't be read as a web address");
    };
    if !matches!(u.scheme(), "http" | "https") || u.host_str().is_none() {
        return bad("it must start with http:// or https:// and name a host");
    }
    if !u.username().is_empty() || u.password().is_some() {
        return bad("it must not contain a user name or password");
    }
    if u.query().is_some() || u.fragment().is_some() {
        return bad("it must not contain a ? or # part");
    }
    Ok(u.as_str().trim_end_matches('/').to_string())
}

/// scheme://host:port of an address as the HTTP client reads it; None when it isn't a usable address.
pub fn origin_of(url: &str) -> Option<String> {
    device_url(url)
        .ok()
        .and_then(|u| url::Url::parse(&u).ok())
        .map(|u| u.origin().ascii_serialization())
}

/// Two addresses are the same device origin (scheme, host and port); a key is only ever sent to its own origin.
pub fn same_origin(a: &str, b: &str) -> bool {
    matches!((origin_of(a), origin_of(b)), (Some(x), Some(y)) if x == y)
}

/// The request address for `path` on a device, built from the parsed address instead of by joining strings.
pub fn device_endpoint(url: &str, path: &str) -> Result<url::Url, String> {
    let base = device_url(url)?;
    url::Url::parse(&format!("{base}{path}"))
        .map_err(|_| "That isn't a usable device address.".to_string())
}

/// The host part of a URL ("" when there is none), lower-cased like urlparse().hostname.
pub fn hostname(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or("");
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(authority);
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    host.to_ascii_lowercase()
}

/// A device is only given up (auto-switch) after this many missed probes in a row, 20 s apart: one slow answer
/// from a busy Tiiny must not switch the studio away and the download queue with it.
const LEAVE_AFTER_MISSES: u32 = 3;

/// Running in the Docker image (its Dockerfile sets STUDIO_DOCKER=1).
pub fn in_docker() -> bool {
    std::env::var("STUDIO_DOCKER").is_ok_and(|v| v == "1")
}

pub fn default_device_name(url: &str) -> String {
    let host = hostname(url);
    if host.starts_with("172.17.") {
        "USB cable".into()
    } else if host.is_empty() {
        "Tiiny".into()
    } else {
        format!("Tiiny ({host})")
    }
}

fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn hint(key: &str) -> String {
    if key.chars().count() >= 8 {
        let tail: String = key
            .chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("…{tail}")
    } else {
        String::new()
    }
}

impl State {
    fn current(&self) -> Option<&Device> {
        self.devices
            .iter()
            .find(|d| Some(&d.id) == self.active.as_ref())
            .or(self.devices.first())
    }
}

impl Settings {
    pub fn load(data_dir: &std::path::Path, lan_access: bool, password: bool, app: bool) -> Self {
        let path = data_dir.join("config.json");
        let saved: Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(json!({}));
        let mut st = State::default();
        if let Some(devices) = saved
            .get("devices")
            .and_then(|d| serde_json::from_value::<Vec<Device>>(d.clone()).ok())
        {
            st.devices = devices;
            st.active = saved
                .get("active")
                .and_then(Value::as_str)
                .map(String::from);
            st.auto = saved.get("auto").and_then(Value::as_bool).unwrap_or(true);
            st.source = "file";
        } else {
            // older single-device config, or env vars
            let env = |k: &str| std::env::var(k).unwrap_or_default();
            let s = |k: &str| {
                saved
                    .get(k)
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
                    .map(String::from)
            };
            let url = normalize_url(&s("url").unwrap_or_else(|| env("TIINY_URL")));
            let key = s("key").unwrap_or_else(|| env("TIINY_KEY"));
            if !url.is_empty() {
                st.devices.push(Device {
                    id: new_id(),
                    name: default_device_name(&url),
                    url: url.clone(),
                    key,
                });
            }
            st.active = st.devices.first().map(|d| d.id.clone());
            st.auto = true;
            let file = saved.as_object().is_some_and(|o| !o.is_empty());
            st.source = if file {
                "file"
            } else if !url.is_empty() {
                "env"
            } else {
                "none"
            };
        }
        st.swap = saved
            .get("swap")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        st.mcp = saved
            .get("mcp")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        st.helper_model = saved
            .get("helper_model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        st.update_check = saved.get("update_check").and_then(Value::as_bool);
        st.tag_prompts = saved.get("tag_prompts").and_then(Value::as_bool);
        warn_unusable(&st.devices);
        Settings {
            path,
            lan_access,
            password,
            app,
            state: Mutex::new(st),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn current(&self) -> Target {
        let st = self.lock();
        st.current()
            .map(|d| Target {
                id: Some(d.id.clone()),
                name: d.name.clone(),
                url: d.url.clone(),
                // an address the HTTP client would read differently from us is kept (so it can be corrected in
                // Settings) but is never sent a key
                key: if d.usable() {
                    d.key.clone()
                } else {
                    String::new()
                },
            })
            .unwrap_or_default()
    }

    pub fn active(&self) -> Option<String> {
        self.lock().active.clone()
    }

    pub fn last_switch(&self) -> Option<Value> {
        self.lock().last_switch.clone()
    }

    pub fn devices(&self) -> Vec<Device> {
        self.lock().devices.clone()
    }

    /// "Check for updates": off until the user answers the one-time question (always off in Docker, where updates come with the image).
    pub fn update_check(&self) -> bool {
        self.lock().update_check.unwrap_or(false)
    }

    /// Not decided yet and not Docker: the page asks "Check for updates automatically?" once (S14).
    pub fn update_ask(&self) -> bool {
        !in_docker() && self.lock().update_check.is_none()
    }

    /// "Write prompts into files": on by default (#58); off keeps the prompt and spoken text out of result files.
    pub fn tag_prompts(&self) -> bool {
        self.lock().tag_prompts.unwrap_or(true)
    }

    pub fn set_tag_prompts(&self, on: bool) {
        let mut st = self.lock();
        st.tag_prompts = Some(on);
        self.save(&mut st);
    }

    pub fn set_update_check(&self, on: bool) {
        let mut st = self.lock();
        st.update_check = Some(on);
        self.save(&mut st);
    }

    /// Everything the settings page shows; the routes take the MCP token out except where it is asked for.
    pub fn public(&self) -> Value {
        let update_check = self.update_check();
        let update_ask = self.update_ask();
        let st = self.lock();
        let cur = st.current().cloned();
        let (url, key) = cur
            .as_ref()
            .map(|d| (d.url.clone(), d.key.clone()))
            .unwrap_or_default();
        json!({
            "devices": st.devices.iter().map(|d| {
                let mut row = json!({
                    "id": d.id, "name": d.name, "url": d.url, "host": hostname(&d.url),
                    "key_set": !d.key.is_empty(), "key_hint": hint(&d.key), "status": st.status.get(&d.id),
                });
                if !d.usable() {
                    row["usable"] = json!(false); // only said when it is not
                }
                row
            }).collect::<Vec<_>>(),
            "active": cur.as_ref().map(|d| d.id.clone()), "name": cur.as_ref().map(|d| d.name.clone()).unwrap_or_default(),
            "url": url, "host": hostname(&url), "auto": st.auto,
            "key_set": !key.is_empty(), "key_hint": hint(&key), "configured": !url.is_empty() && !key.is_empty() && cur.as_ref().is_some_and(Device::usable),
            "source": st.source, "lan_access": self.lan_access, "password": self.password,
            "last_switch": st.last_switch, "version": crate::version(),
            "swap": st.swap, "mcp": st.mcp, "helper_model": st.helper_model, "docker": in_docker(),
            "update_check": update_check, "update_ask": update_ask, "tag_prompts": st.tag_prompts.unwrap_or(true),
            "app": self.app, // the desktop app: no .env to edit, so the UI words the network notes differently
        })
    }

    fn save(&self, st: &mut State) {
        let mut body = json!({"devices": st.devices, "active": st.active, "auto": st.auto, "swap": st.swap, "mcp": st.mcp, "helper_model": st.helper_model});
        if let Some(on) = st.update_check {
            body["update_check"] = json!(on);
        }
        if let Some(on) = st.tag_prompts {
            body["tag_prompts"] = json!(on);
        }
        let tmp = self.path.with_extension("tmp");
        let written = std::fs::write(
            &tmp,
            serde_json::to_string_pretty(&body).unwrap_or_default(),
        )
        .and_then(|_| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
            }
            std::fs::rename(&tmp, &self.path)
        });
        st.save_error = written.err().map(|e| crate::store::io_reason(&e));
        st.source = "file";
    }

    /// Why the last change couldn't be written to config.json (the change itself is in effect until the studio stops);
    /// reading it clears it, so a route calls it before and after the change it makes.
    pub fn take_save_error(&self) -> Option<String> {
        self.lock().save_error.take()
    }

    pub fn upsert(
        &self,
        did: Option<&str>,
        name: &str,
        url: &str,
        key: &str,
    ) -> Result<String, String> {
        if normalize_url(url).is_empty() {
            return Err("Enter the device address, e.g. http://192.168.1.50".into());
        }
        let url = device_url(url)?;
        let name: String = name.trim().chars().take(40).collect();
        let mut st = self.lock();
        let id = match did.and_then(|did| st.devices.iter_mut().find(|d| d.id == did)) {
            Some(d) => {
                if !key.is_empty() {
                    d.key = key.into();
                } else if !same_origin(&url, &d.url) {
                    d.key.clear(); // a key is never sent to a different origin: a new address needs it re-entered
                }
                d.url = url;
                if !name.is_empty() {
                    d.name = name;
                }
                d.id.clone()
            }
            None => {
                let d = Device {
                    id: new_id(),
                    name: if name.is_empty() {
                        default_device_name(&url)
                    } else {
                        name
                    },
                    url,
                    key: key.into(),
                };
                let id = d.id.clone();
                st.devices.push(d);
                id
            }
        };
        if st.active.is_none() || st.current().is_none() {
            st.active = Some(id.clone());
        }
        st.status.remove(&id);
        self.save(&mut st);
        Ok(id)
    }

    pub fn set_swap(&self, req: &Value) {
        let mut st = self.lock();
        if let Some(sw) = req.get("swap").and_then(Value::as_object) {
            for (k, v) in sw {
                let on = crate::device::truthy(v);
                match k.as_str() {
                    "on" => st.swap.on = on,
                    "chat" => st.swap.chat = on,
                    "image" => st.swap.image = on,
                    "audio" => st.swap.audio = on,
                    "mesh" => st.swap.mesh = on,
                    "queue" => st.swap.queue = on,
                    _ => {}
                }
            }
        }
        if let Some(m) = req.get("helper_model").and_then(Value::as_str) {
            st.helper_model = m.trim().chars().take(200).collect();
        }
        self.save(&mut st);
    }

    pub fn mcp(&self) -> McpSettings {
        self.lock().mcp.clone()
    }

    /// `{on}` switches the MCP server (its token is made the first time it goes on); `{new_token: true}` replaces
    /// the token, which locks out every agent that has the old one.
    pub fn set_mcp(&self, req: &Value) {
        let mut st = self.lock();
        if let Some(on) = req.get("on").map(crate::device::truthy) {
            st.mcp.on = on;
            if on && st.mcp.token.is_empty() {
                st.mcp.token = new_token();
            }
        }
        if req.get("new_token").is_some_and(crate::device::truthy) {
            st.mcp.token = new_token();
        }
        self.save(&mut st);
    }

    pub fn swap_allowed(&self, section: &str) -> bool {
        let sw = &self.lock().swap;
        sw.on
            && match section {
                "chat" => sw.chat,
                "image" => sw.image,
                "audio" => sw.audio,
                "mesh" => sw.mesh,
                "queue" => sw.queue,
                _ => false,
            }
    }

    pub fn remove(&self, did: &str) {
        let mut st = self.lock();
        st.devices.retain(|d| d.id != did);
        if st.active.as_deref() == Some(did) {
            st.active = st.devices.first().map(|d| d.id.clone());
        }
        st.status.remove(did);
        self.save(&mut st);
    }

    pub fn move_device(&self, did: &str, step: i64) {
        let mut st = self.lock();
        if let Some(i) = st.devices.iter().position(|d| d.id == did) {
            let j = i as i64 + step;
            if (0..st.devices.len() as i64).contains(&j) {
                st.devices.swap(i, j as usize);
                self.save(&mut st);
            }
        }
    }

    pub fn activate(&self, did: &str, reason: Option<String>) {
        let mut st = self.lock();
        if reason.is_none() {
            st.pinned = Some(did.into()); // chosen by hand: keep it while it answers
        }
        let Some(target) = st.devices.iter().find(|d| d.id == did).cloned() else {
            return;
        };
        if st.active.as_deref() == Some(did) {
            return;
        }
        let before = st.current().map(|d| d.name.clone());
        st.active = Some(did.into());
        if let Some(r) = &reason {
            st.last_switch =
                Some(json!({"to": target.name, "from": before, "t": now(), "reason": r}));
        }
        self.save(&mut st);
        println!(
            "Device: {} ({}){}",
            target.name,
            target.url,
            reason.map(|r| format!(" — {r}")).unwrap_or_default()
        );
    }

    pub fn set_auto(&self, on: bool) {
        let mut st = self.lock();
        st.auto = on;
        if on {
            st.pinned = None; // turning auto-switch (back) on releases a hand-picked device
        }
        self.save(&mut st);
    }

    pub async fn probe(client: reqwest::Client, url: String) -> Probe {
        let t0 = Instant::now();
        let Ok(endpoint) = device_endpoint(&url, "/") else {
            return Probe {
                ok: false,
                ms: None,
            };
        };
        match client
            .get(endpoint)
            .timeout(Duration::from_secs(2))
            .send()
            .await
        {
            Ok(_) => Probe {
                ok: true,
                ms: Some(t0.elapsed().as_millis() as u64),
            },
            Err(_) => Probe {
                ok: false,
                ms: None,
            },
        }
    }

    /// Probe every saved device in parallel; with auto-switch on, use the first one that answers.
    pub async fn check(&self, client: &reqwest::Client) {
        let devices = self.devices();
        let probes = futures_join(
            devices
                .iter()
                .map(|d| Self::probe(client.clone(), d.url.clone())),
        )
        .await;
        let results: HashMap<String, Probe> =
            devices.iter().map(|d| d.id.clone()).zip(probes).collect();
        let switch = {
            let mut st = self.lock();
            st.status.extend(results.clone());
            for (id, p) in &results {
                let n = st.misses.entry(id.clone()).or_default();
                *n = if p.ok { 0 } else { n.saturating_add(1) };
            }
            if !(st.auto && devices.len() > 1) {
                return;
            }
            let ok = |id: &str| results.get(id).is_some_and(|p| p.ok);
            let cur = st.current().cloned();
            if let Some(c) = &cur {
                let missed = st.misses.get(&c.id).copied().unwrap_or(0);
                if !ok(&c.id) && missed < LEAVE_AFTER_MISSES {
                    return; // one or two missed probes: it may just be busy, stay
                }
            }
            if let (Some(p), Some(c)) = (&st.pinned, &cur) {
                if p == &c.id && ok(&c.id) {
                    return; // the hand-picked device still answers: leave it
                }
            }
            st.pinned = None;
            let best = devices.iter().find(|d| ok(&d.id) && !d.key.is_empty());
            match (best, cur) {
                (Some(b), Some(c)) if b.id != c.id => {
                    let reason = if !ok(&c.id) {
                        format!("{} isn't answering", c.name)
                    } else {
                        format!("{} is back", b.name)
                    };
                    Some((b.id.clone(), reason))
                }
                _ => None,
            }
        };
        if let Some((id, reason)) = switch {
            self.activate(&id, Some(reason));
        }
    }
}

/// Run futures concurrently and collect their results in order (no extra crate needed).
async fn futures_join<F: std::future::Future + Send + 'static>(
    futs: impl Iterator<Item = F>,
) -> Vec<F::Output>
where
    F::Output: Send + 'static,
{
    let handles: Vec<_> = futs.map(tokio::spawn).collect();
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        out.push(h.await.expect("probe task"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(normalize_url(" 192.168.1.50/ "), "http://192.168.1.50");
        assert_eq!(normalize_url("https://x.local"), "https://x.local");
        assert_eq!(normalize_url(""), "");
        assert_eq!(hostname("http://User@Tiiny.Local:8080/x"), "tiiny.local");
        assert_eq!(hostname("http://[::1]:80"), "::1");
        assert_eq!(default_device_name("http://172.17.0.9"), "USB cable");
        assert_eq!(
            default_device_name("http://192.168.1.50"),
            "Tiiny (192.168.1.50)"
        );
        assert_eq!(hint("abcdefgh1234"), "…1234");
        assert_eq!(hint("short"), "");
    }

    #[test]
    fn update_check_is_off_until_answered_and_prompt_tags_default_on() {
        let dir = std::env::temp_dir().join(format!("studio-settings-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = Settings::load(&dir, false, false, false);
        assert!(!s.update_check()); // never contacts GitHub before the question is answered
        assert_eq!(s.update_ask(), !in_docker());
        assert_eq!(s.public()["update_ask"], json!(!in_docker()));
        assert!(s.tag_prompts());
        assert_eq!(s.public()["tag_prompts"], json!(true));
        s.set_update_check(true);
        s.set_tag_prompts(false);
        let again = Settings::load(&dir, false, false, false);
        assert!(again.update_check());
        assert!(!again.update_ask());
        assert!(!again.tag_prompts());
        assert_eq!(again.public()["tag_prompts"], json!(false));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn device_addresses_are_read_like_the_http_client_does() {
        assert_eq!(device_url("192.168.1.50/").unwrap(), "http://192.168.1.50");
        assert_eq!(
            device_url("HTTP://Tiiny.Local:8080").unwrap(),
            "http://tiiny.local:8080"
        );
        for bad in [
            "http://attacker:8080\\@192.168.1.50", // the parser reads this as host attacker
            "http://192.168.1.50@attacker:8080",
            "http://user:pw@192.168.1.50",
            "http://192.168.1.50?x=1",
            "http://192.168.1.50#x",
            "ftp://192.168.1.50",
            "http://",
        ] {
            assert!(device_url(bad).is_err(), "{bad}");
        }
        // scheme, host and port all count: another port on the same host is another origin
        assert!(same_origin("192.168.1.50", "http://192.168.1.50:80/"));
        assert!(!same_origin(
            "http://192.168.1.50",
            "http://192.168.1.50:8080"
        ));
        assert!(!same_origin("http://192.168.1.50", "https://192.168.1.50"));
        assert!(!same_origin(
            "http://127.0.0.1:9\\@192.168.1.50",
            "http://192.168.1.50"
        ));
        let u = device_endpoint("http://192.168.1.50:8080/", "/v1/models?a=1").unwrap();
        assert_eq!(u.as_str(), "http://192.168.1.50:8080/v1/models?a=1");
    }

    #[test]
    fn a_key_stays_only_with_its_own_origin() {
        let dir = std::env::temp_dir().join(format!("studio-settings-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = Settings::load(&dir, false, false, false);
        let a = s.upsert(None, "", "192.168.1.50", "key-aaaa-1111").unwrap();
        // other port, other scheme, backslash trick: refused or key dropped, never kept
        s.upsert(Some(&a), "", "http://192.168.1.50:8080", "")
            .unwrap();
        assert_eq!(s.devices()[0].key, "");
        s.upsert(Some(&a), "", "http://192.168.1.50", "key-aaaa-1111")
            .unwrap();
        assert!(s
            .upsert(Some(&a), "", "http://127.0.0.1:9\\@192.168.1.50", "")
            .is_err());
        assert_eq!(s.devices()[0].url, "http://192.168.1.50");
        assert_eq!(s.devices()[0].key, "key-aaaa-1111");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_bad_saved_address_is_kept_but_gets_no_key() {
        let dir = std::env::temp_dir().join(format!("studio-settings-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = json!({"devices": [{"id": "d1", "name": "Old", "url": "http://127.0.0.1:9\\@192.168.1.50", "key": "secret-key-1"}], "active": "d1"});
        std::fs::write(dir.join("config.json"), cfg.to_string()).unwrap();
        let s = Settings::load(&dir, false, false, false);
        assert_eq!(s.devices()[0].url, "http://127.0.0.1:9\\@192.168.1.50"); // kept, so it can be corrected
        assert_eq!(s.current().key, ""); // but nothing is sent a key
        let p = s.public();
        assert_eq!(p["devices"][0]["usable"], json!(false));
        assert_eq!(p["configured"], json!(false));
        // correcting it makes it usable again (the old key is dropped with the origin change)
        s.upsert(Some("d1"), "", "http://192.168.1.50", "new-key-2222")
            .unwrap();
        assert_eq!(s.current().key, "new-key-2222");
        assert!(s.public()["devices"][0].get("usable").is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Answers every request on 127.0.0.1 with an empty 200 (what a probe needs); returns the address.
    async fn live_server() -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut c, _)) = l.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = c.read(&mut buf).await;
                    let _ = c
                        .write_all(
                            b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                        )
                        .await;
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn auto_switch_waits_for_three_missed_probes() {
        let dir = std::env::temp_dir().join(format!("studio-settings-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = Settings::load(&dir, false, false, false);
        let live = live_server().await;
        // the first (preferred, active) device is dead: nothing listens on port 1
        let a = s
            .upsert(None, "dead", "http://127.0.0.1:1", "key-aaaa-1111")
            .unwrap();
        let b = s.upsert(None, "live", &live, "key-bbbb-2222").unwrap();
        s.set_auto(true);
        let client = reqwest::Client::new();
        s.check(&client).await;
        s.check(&client).await;
        assert_eq!(s.active().as_deref(), Some(a.as_str()), "two misses: stay");
        s.check(&client).await;
        assert_eq!(s.active().as_deref(), Some(b.as_str()), "third miss: leave");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn devices_and_keys() {
        let dir = std::env::temp_dir().join(format!("studio-settings-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = Settings::load(&dir, false, false, false);
        let a = s.upsert(None, "", "192.168.1.50", "key-aaaa-1111").unwrap();
        let b = s
            .upsert(None, "Hotspot", "http://192.168.1.60", "key-bbbb-2222")
            .unwrap();
        assert_eq!(s.active().as_deref(), Some(a.as_str()));
        // a new host without a key drops the old key
        s.upsert(Some(&a), "", "http://192.168.1.70", "").unwrap();
        assert_eq!(s.devices()[0].key, "");
        s.move_device(&b, -1);
        assert_eq!(s.devices()[0].id, b);
        s.activate(&b, None);
        let reloaded = Settings::load(&dir, false, false, false);
        assert_eq!(reloaded.public()["active"], json!(b));
        assert_eq!(reloaded.public()["source"], json!("file"));
        s.remove(&b);
        assert_eq!(s.active().as_deref(), Some(a.as_str()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
