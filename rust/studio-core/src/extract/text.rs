//! Plain-text readers and the string helpers that match Python's behaviour (strip, isspace, posixpath,
//! decode): text blocks, the Markdown/CSV/JSON files, and byte decoding.

use super::render::Block;
use super::{Doc, R};

/// Python's `str.isspace` (and regex `\s`): Unicode White_Space plus the C0 separators.
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Python's `str.strip()`.
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// Python's `str.isdigit()` for ASCII digits (non-empty).
pub fn is_digit(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Python's `int(s)` for plain decimal text (surrounding whitespace and a sign allowed).
pub fn py_int(s: &str) -> Option<i64> {
    let t = strip(s);
    let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
    if !is_digit(digits) {
        return None;
    }
    Some(t.parse::<i64>().unwrap_or(if t.starts_with('-') { i64::MIN } else { i64::MAX }))
}

/// File extension, lower-cased ("" when the name has no dot).
pub fn ext(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((_, e)) => e.to_lowercase(),
        None => String::new(),
    }
}

/// `posixpath.dirname`.
pub fn dirname(p: &str) -> String {
    let (head, _) = p.split_at(p.rfind('/').map_or(0, |i| i + 1));
    if !head.is_empty() && !head.bytes().all(|b| b == b'/') { head.trim_end_matches('/').to_string() } else { head.to_string() }
}

/// `posixpath.join` for two parts.
pub fn join(a: &str, b: &str) -> String {
    if b.starts_with('/') {
        b.to_string()
    } else if a.is_empty() || a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

/// `posixpath.normpath`.
pub fn normpath(path: &str) -> String {
    if path.is_empty() {
        return ".".into();
    }
    let initial = if path.starts_with("//") && !path.starts_with("///") { 2 } else { path.starts_with('/') as usize };
    let mut new: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp != ".." || (initial == 0 && new.is_empty()) || new.last() == Some(&"..") {
            new.push(comp);
        } else if !new.is_empty() {
            new.pop();
        }
    }
    let out = format!("{}{}", "/".repeat(initial), new.join("/"));
    if out.is_empty() { ".".into() } else { out }
}

/// `repr()` of a Python str (only what KeyError messages need).
pub fn py_repr(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::from(q);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == q => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(q);
    out
}

const CP1252: [char; 32] = [
    '\u{20ac}', '\u{fffd}', '\u{201a}', '\u{192}', '\u{201e}', '\u{2026}', '\u{2020}', '\u{2021}', '\u{2c6}', '\u{2030}', '\u{160}',
    '\u{2039}', '\u{152}', '\u{fffd}', '\u{17d}', '\u{fffd}', '\u{fffd}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}',
    '\u{2013}', '\u{2014}', '\u{2dc}', '\u{2122}', '\u{161}', '\u{203a}', '\u{153}', '\u{fffd}', '\u{17e}', '\u{178}',
];

/// One cp1252 byte as Python decodes it with errors="replace" (the five holes become U+FFFD).
pub fn cp1252(b: u8) -> char {
    if (0x80..0xa0).contains(&b) { CP1252[(b - 0x80) as usize] } else { b as char }
}

/// extract.py's decode(): UTF-8 (BOM dropped), then UTF-16, then cp1252.
pub fn decode(raw: &[u8]) -> String {
    let bom16 = raw.starts_with(&[0xff, 0xfe]) || raw.starts_with(&[0xfe, 0xff]);
    if !bom16 {
        if let Ok(s) = std::str::from_utf8(raw) {
            return s.strip_prefix('\u{feff}').unwrap_or(s).to_string();
        }
    }
    if raw.len() % 2 == 0 {
        let (be, body) = if raw.starts_with(&[0xfe, 0xff]) {
            (true, &raw[2..])
        } else if raw.starts_with(&[0xff, 0xfe]) {
            (false, &raw[2..])
        } else {
            (false, raw)
        };
        let units: Vec<u16> =
            body.chunks(2).map(|c| if be { u16::from_be_bytes([c[0], c[1]]) } else { u16::from_le_bytes([c[0], c[1]]) }).collect();
        if let Ok(s) = String::from_utf16(&units) {
            return s;
        }
    }
    raw.iter().map(|&b| cp1252(b)).collect()
}

/// Plain text: paragraphs split at blank lines (`\n\s*\n`), line breaks kept.
pub fn text_blocks(doc: &mut Doc, text: &str) {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let ch: Vec<(usize, char)> = text.char_indices().collect();
    let (mut start, mut i) = (0, 0);
    let mut paras = Vec::new();
    while i < ch.len() {
        if ch[i].1 == '\n' {
            let mut j = i + 1;
            while j < ch.len() && is_space(ch[j].1) {
                j += 1;
            }
            if let Some(k) = (i + 1..j).rev().find(|&k| ch[k].1 == '\n') {
                paras.push(&text[start..ch[i].0]);
                start = ch[k].0 + 1;
                i = k + 1;
                continue;
            }
        }
        i += 1;
    }
    paras.push(&text[start..]);
    for p in paras {
        doc.add(Block::P(p.trim_matches('\n').to_string()));
    }
}

/// A JSON file pretty-printed the way Python's `json.dumps(indent=2, ensure_ascii=False)` does; text that
/// doesn't parse is kept as it is.
pub fn json_text(data: &[u8]) -> String {
    let text = strip(&decode(data)).to_string();
    super::json::pretty(&text).unwrap_or(text)
}

/// CSV/TSV into one table block (at most MAX_ROWS rows).
pub fn csv_table(doc: &mut Doc, data: &[u8], tab: bool) -> R<()> {
    use super::csv::{sniff, Dialect, Reader};
    let text = decode(data);
    let dialect = if tab {
        Dialect::excel_tab()
    } else {
        let sample: String = text.chars().take(4096).collect();
        sniff(&sample, ",;\t|")?.unwrap_or_else(Dialect::excel)
    };
    let mut reader = Reader::new(&text, dialect);
    let mut rows = Vec::new();
    while rows.len() < super::MAX_ROWS {
        match reader.next_row()? {
            Some(r) => rows.push(r),
            None => break,
        }
    }
    if rows.len() == super::MAX_ROWS {
        reader.next_row()?; // Python's zip() pulls one more row before it stops, and can fail on it
    }
    doc.add(Block::Table(rows));
    Ok(())
}
