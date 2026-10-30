//! The data folder: history.json (results), voices.json, chats/, images/ (every output and source file).
//! Same files and formats as server.py, so an existing data folder keeps working.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde_json::{json, Value};

pub struct Store {
    pub images: PathBuf,
    pub chats: PathBuf,
    history: PathBuf,
    voices: PathBuf,
    /// Device ids the studio saved, deleted or skipped (device_seen.json); see media/recover.rs.
    seen: PathBuf,
    /// Held while reading-modifying-writing history or voices (server.py LOCK).
    lock: Mutex<()>,
    seen_lock: Mutex<()>,
}

pub fn read_json_file(path: &Path) -> Option<Value> {
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok())
}

/// Write through a temp file so a crash never leaves half a file.
pub fn save_json(path: &Path, v: &Value) {
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, serde_json::to_string_pretty(v).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// Overwrite a file with zeros, then delete it. Best effort: SSDs and copy-on-write filesystems may still
/// keep old blocks, but the data is no longer reachable through the file system.
pub fn shred(path: &Path) {
    use std::io::Write;
    if let Ok(meta) = std::fs::metadata(path) {
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(path) {
            let zeros = vec![0u8; 1 << 20];
            let mut left = meta.len();
            while left > 0 {
                let n = left.min(zeros.len() as u64) as usize;
                if f.write_all(&zeros[..n]).is_err() {
                    break;
                }
                left -= n as u64;
            }
            let _ = f.flush();
            let _ = f.sync_all();
        }
    }
    let _ = std::fs::remove_file(path);
}

/// A file name inside images/ that can't climb out of it (server.py only accepts `[\w.-]+`).
pub fn safe_file(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '-')
}

pub fn word(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_alphanumeric() || c == '_')
}

fn list_of(v: &Value, k: &str) -> Vec<String> {
    v.get(k).and_then(Value::as_array).into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect()
}

/// Every file an entry owns: outputs, sources and downloads.
pub fn entry_files(e: &Value) -> Vec<String> {
    [list_of(e, "outputs"), list_of(e, "sources"), list_of(e, "downloads")].concat()
}

impl Store {
    pub fn new(data: &Path) -> Self {
        let s = Store {
            images: data.join("images"),
            chats: data.join("chats"),
            history: data.join("history.json"),
            voices: data.join("voices.json"),
            seen: data.join("device_seen.json"),
            lock: Mutex::new(()),
            seen_lock: Mutex::new(()),
        };
        let _ = std::fs::create_dir_all(&s.images);
        let _ = std::fs::create_dir_all(&s.chats);
        let _ = std::fs::create_dir_all(data.join("icons"));
        s
    }

    pub fn lock(&self) -> MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn load_history(&self) -> Vec<Value> {
        read_json_file(&self.history).and_then(|v| v.as_array().cloned()).unwrap_or_default()
    }

    pub fn save_history(&self, items: &[Value]) {
        save_json(&self.history, &Value::Array(items.to_vec()));
    }

    pub fn add_history(&self, entry: Value) {
        let _g = self.lock();
        let mut items = self.load_history();
        items.insert(0, entry);
        self.save_history(&items);
    }

    /// Apply `f` to one saved entry (e.g. clear a preview's resume token) and save.
    pub fn edit_history(&self, id: &str, f: impl Fn(&mut serde_json::Map<String, Value>)) {
        let _g = self.lock();
        let mut items = self.load_history();
        for e in items.iter_mut() {
            if e.get("id").and_then(Value::as_str) == Some(id) {
                if let Some(o) = e.as_object_mut() {
                    f(o);
                }
            }
        }
        self.save_history(&items);
    }

    /// Remove entries and shred their files; returns nothing (callers drop jobs/search cache themselves).
    pub fn delete_entries(&self, ids: &[String]) {
        let _g = self.lock();
        let items = self.load_history();
        let (gone, keep): (Vec<Value>, Vec<Value>) =
            items.into_iter().partition(|e| e.get("id").and_then(Value::as_str).is_some_and(|i| ids.iter().any(|x| x == i)));
        for e in &gone {
            for f in entry_files(e) {
                if safe_file(&f) {
                    shred(&self.images.join(f));
                }
            }
        }
        self.save_history(&keep);
    }

    /// The device ids already handled; None without a (usable) file: the next look at the device is a baseline.
    pub fn seen_ids(&self) -> Option<std::collections::BTreeSet<String>> {
        let v = read_json_file(&self.seen)?;
        Some(v.as_array()?.iter().filter_map(|x| x.as_str().map(String::from)).collect())
    }

    /// Add handled device ids. Only once the baseline exists (or when making it): until then everything on the device
    /// counts as history anyway.
    pub fn mark_seen(&self, ids: &[String], baseline: bool) {
        let _g = self.seen_lock.lock().unwrap_or_else(|e| e.into_inner());
        let have = self.seen_ids();
        if have.is_none() && !baseline {
            return;
        }
        let mut all = have.unwrap_or_default();
        all.extend(ids.iter().filter(|i| !i.is_empty()).cloned());
        save_json(&self.seen, &json!(all));
    }

    pub fn load_voices(&self) -> Vec<Value> {
        read_json_file(&self.voices).and_then(|v| v.as_array().cloned()).unwrap_or_default()
    }

    pub fn save_voices(&self, voices: &[Value]) {
        save_json(&self.voices, &Value::Array(voices.to_vec()));
    }

    /// A saved voice as the speech model wants it: name, transcript and the clip as a data URI.
    pub fn voice_for(&self, vid: &str) -> Value {
        use base64::Engine;
        let v = self.load_voices().into_iter().find(|v| v.get("id").and_then(Value::as_str) == Some(vid));
        let Some(v) = v else { return Value::Null };
        let file = v.get("file").and_then(Value::as_str).unwrap_or("");
        match std::fs::read(self.images.join(file)) {
            Ok(b) if safe_file(file) => json!({
                "name": v.get("name"), "text": v.get("text").and_then(Value::as_str).unwrap_or(""),
                "uri": format!("data:audio/wav;base64,{}", base64::engine::general_purpose::STANDARD.encode(b)),
            }),
            _ => Value::Null,
        }
    }

    pub fn list_chats(&self) -> Vec<Value> {
        let mut out = vec![];
        for entry in std::fs::read_dir(&self.chats).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(c) = read_json_file(&path) else { continue };
            let title = c.get("title").and_then(Value::as_str).filter(|t| !t.is_empty()).unwrap_or("Chat");
            out.push(json!({
                "id": path.file_stem().and_then(|s| s.to_str()).unwrap_or(""), "title": title,
                "updated": c.get("updated").cloned().unwrap_or(json!(0)),
                "count": c.get("messages").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
            }));
        }
        let key = |c: &Value| c["updated"].as_f64().unwrap_or(0.0);
        out.sort_by(|a, b| key(b).partial_cmp(&key(a)).unwrap_or(std::cmp::Ordering::Equal));
        out
    }

    pub fn chat_path(&self, id: &str) -> PathBuf {
        self.chats.join(format!("{id}.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_round_trip_and_shred() {
        let dir = std::env::temp_dir().join(format!("studio-store-{}", uuid::Uuid::new_v4().simple()));
        let s = Store::new(&dir);
        std::fs::write(s.images.join("a.png"), b"png").unwrap();
        s.add_history(json!({"id": "e1", "outputs": ["a.png"]}));
        s.add_history(json!({"id": "e2"}));
        s.edit_history("e1", |e| {
            e.insert("fav".into(), json!(true));
        });
        assert_eq!(s.load_history()[1]["fav"], json!(true));
        s.delete_entries(&["e1".into()]);
        assert_eq!(s.load_history().len(), 1);
        assert!(!s.images.join("a.png").exists());
        assert!(!safe_file("../x") && !safe_file("a/b") && safe_file("voice-1.wav"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
