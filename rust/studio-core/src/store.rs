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
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

/// Problems with the saved data that the page can show (a file moved aside as unreadable, a skipped record).
/// Process-wide: history, voices and config all report here.
static DATA_WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn warn_data(msg: String) {
    eprintln!("studio: {msg}");
    let mut w = DATA_WARNINGS.lock().unwrap_or_else(|e| e.into_inner());
    if !w.contains(&msg) {
        w.push(msg);
    }
}

/// Files that could not be read (not "missing", not "damaged"): saving to them is refused for the rest of this run, so
/// a record that is still there is never replaced by one built from "nothing".
static BLOCKED: Mutex<Vec<(PathBuf, Option<String>)>> = Mutex::new(Vec::new());

/// Refuse saves to `path`; `warning` is the data warning that goes away with the block.
fn block_saves(path: &Path, warning: Option<String>) {
    let mut b = BLOCKED.lock().unwrap_or_else(|e| e.into_inner());
    if !b.iter().any(|(p, _)| p == path) {
        b.push((path.to_path_buf(), warning));
    }
}

/// A later successful read: the block and its warning go away.
fn unblock_saves(path: &Path) {
    let mut b = BLOCKED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = b.iter().position(|(p, _)| p == path) {
        if let (_, Some(w)) = b.remove(i) {
            DATA_WARNINGS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|x| *x != w);
        }
    }
}

fn saves_blocked(path: &Path) -> bool {
    BLOCKED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|(p, _)| p == path)
}

pub fn data_warnings() -> Vec<String> {
    DATA_WARNINGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Keep an unusable file as `<name>.broken-<time>` (so the next save can't overwrite it) and report it.
pub fn set_aside(path: &Path, why: &str) {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let mut aside = path.with_file_name(format!("{name}.broken-{}", unix_now()));
    let mut n = 1;
    while aside.exists() {
        aside = path.with_file_name(format!("{name}.broken-{}-{n}", unix_now()));
        n += 1;
    }
    let kept = std::fs::rename(path, &aside).or_else(|_| std::fs::copy(path, &aside).map(|_| ()));
    let aside_name = aside.file_name().and_then(|n| n.to_str()).unwrap_or("");
    warn_data(match kept {
        Ok(()) => format!("{name} could not be read ({why}). It was kept as {aside_name} in the data folder and the studio started with an empty one."),
        Err(e) => {
            block_saves(path, None);
            format!("{name} could not be read ({why}) and could not be moved aside ({}). Fix or remove it before saving anything.", io_reason(&e))
        }
    });
}

/// Read a JSON file where "not there yet" (None) is fine but "there and unusable" is not. Only content that was read
/// and is not valid JSON counts as damaged: that file is moved aside (`set_aside`) and then counts as missing. Any
/// other read error (a sharing violation from a virus scanner or backup, no permission) leaves the file where it is,
/// says so, and refuses saves to it for this run (`write_atomic`).
pub fn read_json_or_set_aside(path: &Path) -> Option<Value> {
    read_json_with(path, |p| std::fs::read(p))
}

fn read_json_with(path: &Path, read: impl Fn(&Path) -> std::io::Result<Vec<u8>>) -> Option<Value> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    if path.is_dir() {
        // a folder where the file belongs: leave it, saves fail until someone looks at it
        warn_data(format!(
            "{name} is a folder, not a file. Fix or remove it before saving anything."
        ));
        return None;
    }
    let why = match read(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            unblock_saves(path);
            return None;
        }
        Err(e) => {
            let msg = format!(
                "{name} could not be opened ({}). It was left where it is and nothing is saved to it while it can't be read.",
                io_reason(&e)
            );
            block_saves(path, Some(msg.clone()));
            warn_data(msg);
            return None;
        }
        Ok(b) => match serde_json::from_slice(&b) {
            Ok(v) => {
                unblock_saves(path);
                return Some(v);
            }
            Err(e) => format!("not valid JSON: {e}"),
        },
    };
    unblock_saves(path);
    set_aside(path, &why);
    None
}

/// What went wrong with a file, in a few words ("No space left on device"): Rust adds " (os error N)".
pub fn io_reason(e: &std::io::Error) -> String {
    let s = e.to_string();
    s.split(" (os error")
        .next()
        .unwrap_or(&s)
        .trim_end_matches('.')
        .to_string()
}

/// Write through a unique temp file (flushed to disk before the rename, the folder too where the OS allows) so a
/// crash never leaves half a file. Err is the reason it couldn't be written.
pub fn write_atomic(path: &Path, bytes: &[u8], private: bool) -> Result<(), String> {
    use std::io::Write;
    if saves_blocked(path) {
        return Err("it could not be read at start, so it is left alone".into());
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = path.with_file_name(format!(
        "{name}.{}.tmp",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    ));
    let written = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        #[cfg(unix)]
        if private {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        #[cfg(not(unix))]
        let _ = private;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        // Windows refuses a rename onto a file that another save (or a reader) has open for a moment: retry briefly
        let mut tries = 0;
        loop {
            match std::fs::rename(&tmp, path) {
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && tries < 20 => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(5 * tries));
                }
                other => break other,
            }
        }
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(io_reason(&e));
    }
    #[cfg(unix)]
    if let Some(dir) = path.parent().and_then(|d| std::fs::File::open(d).ok()) {
        let _ = dir.sync_all();
    }
    Ok(())
}

pub fn save_json(path: &Path, v: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(v).unwrap_or_default();
    write_atomic(path, text.as_bytes(), false)
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

/// Windows opens these as devices ("NUL", "CON.txt", "COM1.png"), whatever the extension.
fn reserved_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|p| {
            stem.strip_prefix(p)
                .is_some_and(|n| n.len() == 1 && n != "0" && n.chars().all(|c| c.is_ascii_digit()))
        })
}

/// A file name inside images/ that can't climb out of it (server.py only accepts `[\w.-]+`).
pub fn safe_file(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.ends_with('.')
        && !reserved_name(name)
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '-')
}

pub fn word(id: &str) -> bool {
    !id.is_empty() && !reserved_name(id) && id.chars().all(|c| c.is_alphanumeric() || c == '_')
}

fn list_of(v: &Value, k: &str) -> Vec<String> {
    v.get(k)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|x| x.as_str().map(String::from))
        .collect()
}

/// Every file an entry owns: outputs, sources and downloads.
pub fn entry_files(e: &Value) -> Vec<String> {
    [
        list_of(e, "outputs"),
        list_of(e, "sources"),
        list_of(e, "downloads"),
    ]
    .concat()
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

    /// A saved list. A file that exists but is unusable is set aside (`set_aside`) and never saved over.
    fn load_list(path: &Path) -> Vec<Value> {
        match read_json_or_set_aside(path) {
            Some(Value::Array(a)) => a,
            Some(_) => {
                set_aside(path, "not a list");
                vec![]
            }
            None => vec![],
        }
    }

    pub fn load_history(&self) -> Vec<Value> {
        Self::load_list(&self.history)
    }

    pub fn save_history(&self, items: &[Value]) -> Result<(), String> {
        save_json(&self.history, &Value::Array(items.to_vec()))
    }

    pub fn add_history(&self, entry: Value) -> Result<(), String> {
        let _g = self.lock();
        let mut items = self.load_history();
        items.insert(0, entry);
        self.save_history(&items)
    }

    /// Apply `f` to one saved entry (e.g. clear a preview's resume token) and save.
    pub fn edit_history(
        &self,
        id: &str,
        f: impl Fn(&mut serde_json::Map<String, Value>),
    ) -> Result<(), String> {
        let _g = self.lock();
        let mut items = self.load_history();
        for e in items.iter_mut() {
            if e.get("id").and_then(Value::as_str) == Some(id) {
                if let Some(o) = e.as_object_mut() {
                    f(o);
                }
            }
        }
        self.save_history(&items)
    }

    /// Remove entries and shred their files (callers drop jobs/search cache themselves). The list is saved first, so a
    /// failed save (disk full) leaves entries that still have their files; the shredding (slow: it writes zeros and
    /// syncs) happens after the store lock is let go. Blocking: call from `spawn_blocking` in async code.
    pub fn delete_entries(&self, ids: &[String]) -> Result<(), String> {
        let gone = {
            let _g = self.lock();
            let items = self.load_history();
            let (gone, keep): (Vec<Value>, Vec<Value>) = items.into_iter().partition(|e| {
                e.get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|i| ids.iter().any(|x| x == i))
            });
            self.save_history(&keep)?;
            gone
        };
        for e in &gone {
            for f in entry_files(e) {
                if safe_file(&f) {
                    shred(&self.images.join(f));
                }
            }
        }
        Ok(())
    }

    /// The device ids already handled; None without a (usable) file: the next look at the device is a baseline.
    pub fn seen_ids(&self) -> Option<std::collections::BTreeSet<String>> {
        let v = read_json_file(&self.seen)?;
        Some(
            v.as_array()?
                .iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect(),
        )
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
        let _ = save_json(&self.seen, &json!(all)); // best effort: a clip or song deleted here could be pulled back from the device at the next look
    }

    pub fn load_voices(&self) -> Vec<Value> {
        Self::load_list(&self.voices)
    }

    pub fn save_voices(&self, voices: &[Value]) -> Result<(), String> {
        save_json(&self.voices, &Value::Array(voices.to_vec()))
    }

    /// A saved voice as the speech model wants it: name, transcript and the clip as a data URI.
    pub fn voice_for(&self, vid: &str) -> Value {
        use base64::Engine;
        let v = self
            .load_voices()
            .into_iter()
            .find(|v| v.get("id").and_then(Value::as_str) == Some(vid));
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
        for entry in std::fs::read_dir(&self.chats)
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(c) = read_json_file(&path) else {
                continue;
            };
            let title = c
                .get("title")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
                .unwrap_or("Chat");
            out.push(json!({
                "id": path.file_stem().and_then(|s| s.to_str()).unwrap_or(""), "title": title,
                "updated": c.get("updated").cloned().unwrap_or(json!(0)),
                "count": c.get("messages").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
            }));
        }
        let key = |c: &Value| c["updated"].as_f64().unwrap_or(0.0);
        out.sort_by(|a, b| {
            key(b)
                .partial_cmp(&key(a))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        out
    }

    /// For "wipe everything": temp files and `.broken-*` copies of history/voices/chats hold content too.
    pub fn shred_leftovers(&self) {
        let data = self.history.parent().unwrap_or(Path::new("."));
        for dir in [data, self.chats.as_path()] {
            for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                let ours = n.ends_with(".tmp") && !n.starts_with("config.json")
                    || (n.starts_with("history.json.") || n.starts_with("voices.json."))
                        && n.contains(".broken-")
                    || dir == self.chats && n.contains(".json.broken-");
                if ours && e.path().is_file() {
                    shred(&e.path());
                }
            }
        }
    }

    pub fn chat_path(&self, id: &str) -> PathBuf {
        self.chats.join(format!("{id}.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_error_leaves_the_file_and_blocks_saves_but_bad_content_is_set_aside() {
        let dir =
            std::env::temp_dir().join(format!("studio-store-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        // a sharing violation / no permission: not damaged, so it stays and is never overwritten
        let busy = dir.join("history.json");
        std::fs::write(&busy, b"[{\"id\": \"keep\"}]").unwrap();
        let denied = |_: &Path| Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert!(read_json_with(&busy, denied).is_none());
        assert!(busy.exists(), "left in place");
        assert!(
            !std::fs::read_dir(&dir).unwrap().any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".broken-")),
            "not moved aside"
        );
        assert!(
            write_atomic(&busy, b"[]", false).is_err(),
            "saves are refused"
        );
        assert_eq!(std::fs::read(&busy).unwrap(), b"[{\"id\": \"keep\"}]");
        // a later successful read clears the block and its warning
        let warned = || {
            data_warnings()
                .iter()
                .any(|w| w.starts_with("history.json could not be opened ("))
        };
        assert!(warned());
        assert!(read_json_or_set_aside(&busy).is_some());
        assert!(!warned());
        assert!(
            write_atomic(&busy, b"[{\"id\": \"keep\"}]", false).is_ok(),
            "saves work again"
        );
        // content that was read and is not JSON is damaged: moved aside, then the path is free again
        let bad = dir.join("voices.json");
        std::fs::write(&bad, b"{not json").unwrap();
        assert!(read_json_or_set_aside(&bad).is_none());
        assert!(!bad.exists(), "moved aside");
        assert!(write_atomic(&bad, b"[]", false).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn history_round_trip_and_shred() {
        let dir =
            std::env::temp_dir().join(format!("studio-store-{}", uuid::Uuid::new_v4().simple()));
        let s = Store::new(&dir);
        std::fs::write(s.images.join("a.png"), b"png").unwrap();
        s.add_history(json!({"id": "e1", "outputs": ["a.png"]}))
            .unwrap();
        s.add_history(json!({"id": "e2"})).unwrap();
        s.edit_history("e1", |e| {
            e.insert("fav".into(), json!(true));
        })
        .unwrap();
        assert_eq!(s.load_history()[1]["fav"], json!(true));
        s.delete_entries(&["e1".into()]).unwrap();
        assert_eq!(s.load_history().len(), 1);
        assert!(!s.images.join("a.png").exists());
        assert!(!safe_file("../x") && !safe_file("a/b") && safe_file("voice-1.wav"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_unreadable_history_is_kept_aside_and_a_missing_one_is_empty() {
        let dir =
            std::env::temp_dir().join(format!("studio-store-{}", uuid::Uuid::new_v4().simple()));
        let s = Store::new(&dir);
        assert!(s.load_history().is_empty() && s.load_voices().is_empty());
        assert!(!dir.join("history.json").exists());
        std::fs::write(dir.join("history.json"), vec![0u8; 64]).unwrap();
        assert!(s.load_history().is_empty());
        s.add_history(json!({"id": "e1"})).unwrap();
        let kept: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("history.json.broken-")
            })
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(std::fs::read(kept[0].path()).unwrap(), vec![0u8; 64]);
        assert_eq!(s.load_history().len(), 1);
        // wipe takes the kept copy too, and no temp file is left behind by saves
        s.shred_leftovers();
        assert!(!kept[0].path().exists());
        assert!(std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .all(|e| !e.file_name().to_string_lossy().ends_with(".tmp")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reserved_windows_names_are_refused() {
        for n in [
            "NUL", "nul", "CON.png", "com1", "LPT9.wav", "aux.txt", "a.", "PRN",
        ] {
            assert!(!safe_file(n), "{n}");
        }
        assert!(safe_file("console.png") && safe_file("com10.png") && safe_file("a.b-c_1.png"));
        assert!(!word("NUL") && word("nul1") && word("chat_1"));
    }

    #[test]
    fn concurrent_saves_of_one_file_all_succeed() {
        let dir =
            std::env::temp_dir().join(format!("studio-store-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("c.json");
        let hs: Vec<_> = (0..4)
            .map(|t| {
                let p = p.clone();
                std::thread::spawn(move || {
                    (0..100)
                        .filter(|i| save_json(&p, &json!({"t": t, "i": i})).is_err())
                        .count()
                })
            })
            .collect();
        let failed: usize = hs.into_iter().map(|h| h.join().unwrap()).sum();
        // Windows may refuse a rename onto a file another thread has open for reading; none here reads
        assert_eq!(failed, 0);
        assert!(read_json_file(&p).is_some());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_write_is_an_error_with_a_reason() {
        let dir =
            std::env::temp_dir().join(format!("studio-store-{}", uuid::Uuid::new_v4().simple()));
        let s = Store::new(&dir);
        // the folder is gone (a plain file in its place): nothing can be written, and the caller is told
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::write(&dir, b"").unwrap();
        assert!(!s.add_history(json!({"id": "e1"})).unwrap_err().is_empty());
        assert!(!s.save_voices(&[]).unwrap_err().is_empty());
        std::fs::remove_file(&dir).unwrap();
        assert!(!io_reason(&std::io::Error::from_raw_os_error(2)).contains("(os error"));
    }
}
