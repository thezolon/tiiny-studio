//! The prompt and settings inside a finished result's own files (#58), so they travel with the file and show in the
//! file manager: PNG text chunks, an ID3 tag on MP3s (Explorer's tooltip and Details pane, Finder's Get Info), and
//! MP4 title/comment. Best effort: a file that can't be tagged is left exactly as it was.

use std::path::Path;

use serde_json::Value;

const SOFTWARE: &str = concat!("Tiiny Studio ", env!("CARGO_PKG_VERSION"));

/// What goes into the files: a short title (the prompt's first line) and a description (the whole prompt plus the
/// model and the settings that reproduce it).
fn text(entry: &Value) -> Option<(String, String)> {
    let p = entry.get("params")?;
    let s = |k: &str| {
        p.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let prompt = ["prompt", "caption", "text", "tags"]
        .iter()
        .map(|k| s(k))
        .find(|x| !x.is_empty())
        .unwrap_or_default();
    let model = s("model");
    if prompt.is_empty() && model.is_empty() {
        return None;
    }
    let title: String = prompt
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(120)
        .collect();
    let mut desc = prompt.clone();
    let mut extra = vec![];
    if !model.is_empty() {
        extra.push(format!("Model: {model}"));
    }
    for k in ["seed", "steps", "size", "seconds"] {
        if let Some(v) = p.get(k).filter(|v| !v.is_null()) {
            extra.push(format!(
                "{k}: {}",
                v.as_str()
                    .map(String::from)
                    .unwrap_or_else(|| v.to_string())
            ));
        }
    }
    if !extra.is_empty() {
        if !desc.is_empty() {
            desc.push_str("\n\n");
        }
        desc.push_str(&extra.join(" · "));
    }
    Some((title, desc))
}

/// Tag every output and download of a finished entry (in `dir`, the results folder).
pub fn embed(dir: &Path, entry: &Value) {
    let Some((title, desc)) = text(entry) else {
        return;
    };
    let files = ["outputs", "downloads"]
        .iter()
        .filter_map(|k| entry.get(*k).and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
        .filter(|n| crate::store::safe_file(n));
    for name in files {
        let path = dir.join(name);
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".png") {
            rewrite(&path, |b| png_with_text(b, &title, &desc));
        } else if lower.ends_with(".mp3") {
            rewrite(&path, |b| Some(mp3_with_id3(b, &title, &desc)));
        } else if lower.ends_with(".mp4") {
            if let Ok(mut tag) = mp4ameta::Tag::read_from_path(&path) {
                tag.set_title(title.clone());
                tag.set_comment(desc.clone());
                tag.set_encoder(SOFTWARE);
                let _ = tag.write_to_path(&path);
            }
        }
    }
}

/// Replace a file's bytes with `f(bytes)` through a temporary file, so a failure never leaves half a file.
fn rewrite(path: &Path, f: impl FnOnce(&[u8]) -> Option<Vec<u8>>) {
    let Ok(b) = std::fs::read(path) else { return };
    let Some(out) = f(&b) else { return };
    let tmp = path.with_extension("tagging");
    if std::fs::write(&tmp, out).is_ok() && std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// The PNG with iTXt chunks (UTF-8) for Title, Description and Software right after IHDR.
fn png_with_text(b: &[u8], title: &str, desc: &str) -> Option<Vec<u8>> {
    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if !b.starts_with(SIG) || b.len() < 33 || &b[12..16] != b"IHDR" {
        return None;
    }
    let ihdr_end = 8 + 8 + u32::from_be_bytes(b[8..12].try_into().ok()?) as usize + 4;
    let mut out = b[..ihdr_end].to_vec();
    for (key, val) in [
        ("Title", title),
        ("Description", desc),
        ("Software", SOFTWARE),
    ] {
        if val.is_empty() {
            continue;
        }
        // keyword \0 compression-flag compression-method language \0 translated-keyword \0 text
        let mut data = key.as_bytes().to_vec();
        data.extend_from_slice(&[0, 0, 0, 0, 0]);
        data.extend_from_slice(val.as_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut crc = crc32fast::Hasher::new();
        crc.update(b"iTXt");
        crc.update(&data);
        out.extend_from_slice(b"iTXt");
        out.extend_from_slice(&data);
        out.extend_from_slice(&crc.finalize().to_be_bytes());
    }
    out.extend_from_slice(&b[ihdr_end..]);
    Some(out)
}

/// UTF-16 with a byte-order mark and a terminator, as ID3v2.3 text frames want it (encoding 1).
fn utf16(s: &str) -> Vec<u8> {
    let mut v = vec![0xFF, 0xFE];
    for u in s.encode_utf16() {
        v.extend_from_slice(&u.to_le_bytes());
    }
    v.extend_from_slice(&[0, 0]);
    v
}

/// The MP3 with an ID3v2.3 tag (title, comment, encoder) in front, replacing one that was there.
/// v2.3 with UTF-16 because that's what Windows Explorer reads reliably.
fn mp3_with_id3(b: &[u8], title: &str, desc: &str) -> Vec<u8> {
    let synch = |s: &[u8]| {
        s.iter()
            .fold(0usize, |a, &x| (a << 7) | (x & 0x7f) as usize)
    };
    let body = if b.len() >= 10 && &b[..3] == b"ID3" {
        let footer = if b[5] & 0x10 != 0 { 10 } else { 0 };
        &b[(10 + synch(&b[6..10]) + footer).min(b.len())..]
    } else {
        b
    };
    let frame = |id: &[u8; 4], data: Vec<u8>| {
        let mut f = id.to_vec();
        f.extend_from_slice(&(data.len() as u32).to_be_bytes());
        f.extend_from_slice(&[0, 0]);
        f.extend(data);
        f
    };
    let mut frames = vec![];
    if !title.is_empty() {
        frames.extend(frame(b"TIT2", [vec![1], utf16(title)].concat()));
    }
    if !desc.is_empty() {
        // encoding, language, short description (empty), text
        frames.extend(frame(
            b"COMM",
            [vec![1], b"eng".to_vec(), utf16(""), utf16(desc)].concat(),
        ));
    }
    frames.extend(frame(b"TSSE", [vec![1], utf16(SOFTWARE)].concat()));
    let n = frames.len();
    let mut out = b"ID3\x03\x00\x00".to_vec();
    out.extend_from_slice(&[
        (n >> 21) as u8 & 0x7f,
        (n >> 14) as u8 & 0x7f,
        (n >> 7) as u8 & 0x7f,
        n as u8 & 0x7f,
    ]);
    out.extend(frames);
    out.extend_from_slice(body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn title_and_description_from_the_entry() {
        let e = json!({"params": {"prompt": "A steampunk robot\nwalking", "model": "Qwen/Qwen-Image-2.1", "seed": 7, "steps": 4}});
        let (t, d) = text(&e).unwrap();
        assert_eq!(t, "A steampunk robot");
        assert!(d.starts_with(
            "A steampunk robot\nwalking\n\nModel: Qwen/Qwen-Image-2.1 · seed: 7 · steps: 4"
        ));
        assert!(text(&json!({"params": {}})).is_none());
    }

    #[test]
    fn png_gets_text_chunks_after_ihdr_and_still_decodes() {
        let mut png = vec![];
        {
            let mut enc = png::Encoder::new(&mut png, 1, 1);
            enc.set_color(png::ColorType::Rgba);
            enc.write_header()
                .unwrap()
                .write_image_data(&[1, 2, 3, 4])
                .unwrap();
        }
        let out = png_with_text(&png, "TEST title", "TEST — prompt ✓").unwrap();
        let dec = png::Decoder::new(std::io::Cursor::new(&out))
            .read_info()
            .unwrap();
        let texts: Vec<_> = dec
            .info()
            .utf8_text
            .iter()
            .map(|t| (t.keyword.clone(), t.get_text().unwrap()))
            .collect();
        assert!(texts.contains(&("Title".into(), "TEST title".into())));
        assert!(texts.contains(&("Description".into(), "TEST — prompt ✓".into())));
        assert!(png_with_text(b"not a png", "a", "b").is_none());
    }

    #[test]
    fn mp3_tag_replaces_an_old_one_and_keeps_the_audio() {
        let audio = b"\xff\xfb\x90\x00AUDIO";
        let first = mp3_with_id3(audio, "One", "first");
        let second = mp3_with_id3(&first, "Two", "second");
        assert!(second.starts_with(b"ID3\x03\x00"));
        assert!(second.ends_with(audio));
        assert_eq!(second.windows(4).filter(|w| w == b"TIT2").count(), 1); // the old tag is gone
        let size = second[6..10]
            .iter()
            .fold(0usize, |a, &x| (a << 7) | x as usize);
        assert_eq!(10 + size + audio.len(), second.len());
    }
}
