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
    Some(t.parse::<i64>().unwrap_or(if t.starts_with('-') {
        i64::MIN
    } else {
        i64::MAX
    }))
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
    if !head.is_empty() && !head.bytes().all(|b| b == b'/') {
        head.trim_end_matches('/').to_string()
    } else {
        head.to_string()
    }
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
    let initial = if path.starts_with("//") && !path.starts_with("///") {
        2
    } else {
        path.starts_with('/') as usize
    };
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
    if out.is_empty() {
        ".".into()
    } else {
        out
    }
}

/// `repr()` of a Python str (only what KeyError messages need).
pub fn py_repr(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
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
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push(q);
    out
}

const CP1252: [char; 32] = [
    '\u{20ac}', '\u{fffd}', '\u{201a}', '\u{192}', '\u{201e}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{2c6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{fffd}', '\u{17d}', '\u{fffd}',
    '\u{fffd}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{2dc}', '\u{2122}', '\u{161}', '\u{203a}', '\u{153}', '\u{fffd}', '\u{17e}', '\u{178}',
];

/// One cp1252 byte as Python decodes it with errors="replace" (the five holes become U+FFFD).
pub fn cp1252(b: u8) -> char {
    if (0x80..0xa0).contains(&b) {
        CP1252[(b - 0x80) as usize]
    } else {
        b as char
    }
}

/// Whether BOM-less bytes look like UTF-16: a good share of the sampled bytes are NUL, nearly all of them in
/// the high byte of each pair. Returns true for big-endian. Latin-1/cp1252 text has no NULs, however its
/// bytes happen to pair up.
fn utf16_without_bom(raw: &[u8]) -> Option<bool> {
    let sample = &raw[..raw.len().min(4096) & !1];
    let (mut even, mut odd) = (0usize, 0usize);
    for (i, &b) in sample.iter().enumerate() {
        if b == 0 {
            if i % 2 == 0 {
                even += 1;
            } else {
                odd += 1;
            }
        }
    }
    let pairs = sample.len() / 2;
    if pairs == 0 || even + odd < pairs / 4 {
        return None;
    }
    if odd >= even * 4 {
        Some(false)
    } else if even >= odd * 4 {
        Some(true)
    } else {
        None
    }
}

/// extract.py's decode(): UTF-8 (BOM dropped), then UTF-16 (with a BOM, or BOM-less when it clearly is),
/// then cp1252.
pub fn decode(raw: &[u8]) -> String {
    let bom16 = raw.starts_with(&[0xff, 0xfe]) || raw.starts_with(&[0xfe, 0xff]);
    if !bom16 {
        if let Ok(s) = std::str::from_utf8(raw) {
            return s.strip_prefix('\u{feff}').unwrap_or(s).to_string();
        }
    }
    let guess = if bom16 { None } else { utf16_without_bom(raw) };
    if raw.len().is_multiple_of(2) && (bom16 || guess.is_some()) {
        let (be, body) = if raw.starts_with(&[0xfe, 0xff]) {
            (true, &raw[2..])
        } else if raw.starts_with(&[0xff, 0xfe]) {
            (false, &raw[2..])
        } else {
            (guess == Some(true), raw)
        };
        let units: Vec<u16> = body
            .chunks(2)
            .map(|c| {
                if be {
                    u16::from_be_bytes([c[0], c[1]])
                } else {
                    u16::from_le_bytes([c[0], c[1]])
                }
            })
            .collect();
        if let Ok(s) = String::from_utf16(&units) {
            return s;
        }
    }
    raw.iter().map(|&b| cp1252(b)).collect()
}

/// Plain text: paragraphs split at blank lines (`\n\s*\n`), line breaks kept.
pub fn text_blocks(doc: &mut Doc, text: &str) {
    let text = if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text.to_string()
    };
    let (mut start, mut paras) = (0, Vec::new());
    let mut it = text.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if c != '\n' {
            continue;
        }
        // the whitespace after this newline: a paragraph break when it holds another newline (the last one wins)
        let mut last_nl = None;
        while let Some(&(k, w)) = it.peek() {
            if !is_space(w) {
                break;
            }
            if w == '\n' {
                last_nl = Some(k);
            }
            it.next();
        }
        if let Some(k) = last_nl {
            paras.push(&text[start..i]);
            start = k + 1;
        }
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

/// The rows of a CSV/TSV text (at most MAX_ROWS), by Python's csv rules.
fn csv_rows(text: &str, tab: bool) -> R<Vec<Vec<String>>> {
    use super::csv::{sniff, Dialect, Reader};
    let dialect = if tab {
        Dialect::excel_tab()
    } else {
        let sample: String = text.chars().take(4096).collect();
        sniff(&sample, ",;\t|")?.unwrap_or_else(Dialect::excel)
    };
    let mut reader = Reader::new(text, dialect);
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
    Ok(rows)
}

/// CSV/TSV into one table block (at most MAX_ROWS rows). Line ends are normalised first (a file with bare
/// CRs is one line to Python's reader); a file the reader refuses (an unclosed quote swallows the rest, or a
/// cell is over the field limit) is shown as plain text instead of failing.
pub fn csv_table(doc: &mut Doc, data: &[u8], tab: bool) -> R<()> {
    let text = decode(data).replace("\r\n", "\n").replace('\r', "\n");
    match csv_rows(&text, tab) {
        Ok(rows) => doc.add(Block::Table(rows)),
        Err(_) => text_blocks(doc, strip(&text)),
    }
    Ok(())
}
