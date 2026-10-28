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

#[derive(Clone, Debug, Serialize)]
pub struct Probe {
    pub ok: bool,
    pub ms: Option<u64>,
}

#[derive(Default)]
struct State {
    devices: Vec<Device>,
    active: Option<String>,
    auto: bool,
    source: &'static str,
    status: HashMap<String, Probe>,
    pinned: Option<String>, // a device picked by hand stays in use while it answers
    last_switch: Option<Value>,
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
    if !url.is_empty() && !(url.starts_with("http://") || url.starts_with("https://")) {
        format!("http://{url}")
    } else {
        url.to_string()
    }
}

/// The host part of a URL ("" when there is none), lower-cased like urlparse().hostname.
pub fn hostname(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or("");
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    host.to_ascii_lowercase()
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
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn hint(key: &str) -> String {
    if key.chars().count() >= 8 {
        let tail: String = key.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
        format!("…{tail}")
    } else {
        String::new()
    }
}

impl State {
    fn current(&self) -> Option<&Device> {
        self.devices.iter().find(|d| Some(&d.id) == self.active.as_ref()).or(self.devices.first())
    }
}

impl Settings {
    pub fn load(data_dir: &std::path::Path, lan_access: bool, password: bool, app: bool) -> Self {
        let path = data_dir.join("config.json");
        let saved: Value = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}));
        let mut st = State::default();
        if let Some(devices) = saved.get("devices").and_then(|d| serde_json::from_value::<Vec<Device>>(d.clone()).ok()) {
            st.devices = devices;
            st.active = saved.get("active").and_then(Value::as_str).map(String::from);
            st.auto = saved.get("auto").and_then(Value::as_bool).unwrap_or(true);
            st.source = "file";
        } else {
            // older single-device config, or env vars
            let env = |k: &str| std::env::var(k).unwrap_or_default();
            let s = |k: &str| saved.get(k).and_then(Value::as_str).filter(|v| !v.is_empty()).map(String::from);
            let url = normalize_url(&s("url").unwrap_or_else(|| env("TIINY_URL")));
            let key = s("key").unwrap_or_else(|| env("TIINY_KEY"));
            if !url.is_empty() {
                st.devices.push(Device { id: new_id(), name: default_device_name(&url), url: url.clone(), key });
            }
            st.active = st.devices.first().map(|d| d.id.clone());
            st.auto = true;
            let file = saved.as_object().is_some_and(|o| !o.is_empty());
            st.source = if file { "file" } else if !url.is_empty() { "env" } else { "none" };
        }
        Settings { path, lan_access, password, app, state: Mutex::new(st) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn current(&self) -> Target {
        let st = self.lock();
        st.current().map(|d| Target { id: Some(d.id.clone()), name: d.name.clone(), url: d.url.clone(), key: d.key.clone() }).unwrap_or_default()
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

    pub fn public(&self) -> Value {
        let st = self.lock();
        let cur = st.current().cloned();
        let (url, key) = cur.as_ref().map(|d| (d.url.clone(), d.key.clone())).unwrap_or_default();
        json!({
            "devices": st.devices.iter().map(|d| json!({
                "id": d.id, "name": d.name, "url": d.url, "host": hostname(&d.url),
                "key_set": !d.key.is_empty(), "key_hint": hint(&d.key), "status": st.status.get(&d.id),
            })).collect::<Vec<_>>(),
            "active": cur.as_ref().map(|d| d.id.clone()), "name": cur.as_ref().map(|d| d.name.clone()).unwrap_or_default(),
            "url": url, "host": hostname(&url), "auto": st.auto,
            "key_set": !key.is_empty(), "key_hint": hint(&key), "configured": !url.is_empty() && !key.is_empty(),
            "source": st.source, "lan_access": self.lan_access, "password": self.password,
            "last_switch": st.last_switch, "version": crate::version(),
            "app": self.app, // the desktop app: no .env to edit, so the UI words the network notes differently
        })
    }

    fn save(&self, st: &mut State) {
        let body = json!({"devices": st.devices, "active": st.active, "auto": st.auto});
        let tmp = self.path.with_extension("tmp");
        if std::fs::write(&tmp, serde_json::to_string_pretty(&body).unwrap_or_default()).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
            }
            let _ = std::fs::rename(&tmp, &self.path);
        }
        st.source = "file";
    }

    pub fn upsert(&self, did: Option<&str>, name: &str, url: &str, key: &str) -> Result<String, String> {
        let url = normalize_url(url);
        if url.is_empty() {
            return Err("Enter the device address, e.g. http://192.168.1.50".into());
        }
        let name: String = name.trim().chars().take(40).collect();
        let mut st = self.lock();
        let id = match did.and_then(|did| st.devices.iter_mut().find(|d| d.id == did)) {
            Some(d) => {
                if !key.is_empty() {
                    d.key = key.into();
                } else if hostname(&url) != hostname(&d.url) {
                    d.key.clear(); // a key is never sent to a different host: a new address needs it re-entered
                }
                d.url = url;
                if !name.is_empty() {
                    d.name = name;
                }
                d.id.clone()
            }
            None => {
                let d = Device { id: new_id(), name: if name.is_empty() { default_device_name(&url) } else { name }, url, key: key.into() };
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
        let Some(target) = st.devices.iter().find(|d| d.id == did).cloned() else { return };
        if st.active.as_deref() == Some(did) {
            return;
        }
        let before = st.current().map(|d| d.name.clone());
        st.active = Some(did.into());
        if let Some(r) = &reason {
            st.last_switch = Some(json!({"to": target.name, "from": before, "t": now(), "reason": r}));
        }
        self.save(&mut st);
        println!("Device: {} ({}){}", target.name, target.url, reason.map(|r| format!(" — {r}")).unwrap_or_default());
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
        match client.get(format!("{url}/")).timeout(Duration::from_secs(2)).send().await {
            Ok(_) => Probe { ok: true, ms: Some(t0.elapsed().as_millis() as u64) },
            Err(_) => Probe { ok: false, ms: None },
        }
    }

    /// Probe every saved device in parallel; with auto-switch on, use the first one that answers.
    pub async fn check(&self, client: &reqwest::Client) {
        let devices = self.devices();
        let probes = futures_join(devices.iter().map(|d| Self::probe(client.clone(), d.url.clone()))).await;
        let results: HashMap<String, Probe> = devices.iter().map(|d| d.id.clone()).zip(probes).collect();
        let switch = {
            let mut st = self.lock();
            st.status.extend(results.clone());
            if !(st.auto && devices.len() > 1) {
                return;
            }
            let ok = |id: &str| results.get(id).is_some_and(|p| p.ok);
            let cur = st.current().cloned();
            if let (Some(p), Some(c)) = (&st.pinned, &cur) {
                if p == &c.id && ok(&c.id) {
                    return; // the hand-picked device still answers: leave it
                }
            }
            st.pinned = None;
            let best = devices.iter().find(|d| ok(&d.id) && !d.key.is_empty());
            match (best, cur) {
                (Some(b), Some(c)) if b.id != c.id => {
                    let reason = if !ok(&c.id) { format!("{} isn't answering", c.name) } else { format!("{} is back", b.name) };
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
async fn futures_join<F: std::future::Future>(futs: impl Iterator<Item = F>) -> Vec<F::Output>
where
    F: Send + 'static,
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
        assert_eq!(default_device_name("http://192.168.1.50"), "Tiiny (192.168.1.50)");
        assert_eq!(hint("abcdefgh1234"), "…1234");
        assert_eq!(hint("short"), "");
    }

    #[test]
    fn devices_and_keys() {
        let dir = std::env::temp_dir().join(format!("studio-settings-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = Settings::load(&dir, false, false, false);
        let a = s.upsert(None, "", "192.168.1.50", "key-aaaa-1111").unwrap();
        let b = s.upsert(None, "Hotspot", "http://192.168.1.60", "key-bbbb-2222").unwrap();
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
