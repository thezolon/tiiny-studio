//! Text (and pictures) out of documents, in memory: Office Open XML (docx, pptx, xlsx), OpenDocument
//! (odt, odp, ods), EPUB, RTF, HTML and plain text. Port of extract.py, which is the reference: the output
//! is the same JSON, part for part. PDFs and images are read in the browser instead.
//!
//! `extract(name, data)` returns the list of parts in reading order: `{"label", "text", "md"}` or
//! `{"label", "image"}` (a data URI, for the browser to send through OCR). Documents are read into blocks
//! (headings, paragraphs, list items, tables) so the same content can be shown as plain text or Markdown.

mod csv;
mod entities;
mod epub;
mod html;
mod json;
mod odf;
mod ooxml;
mod render;
mod rtf;
mod text;
mod xml;

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{json, Value};

use render::Block;
use xml::Elem;

const MAX_MEMBER: u64 = 64 << 20; // per file inside a zip: guards against zip bombs
const MAX_TOTAL: u64 = 256 << 20;
const MAX_IMAGES: usize = 50;
const MAX_ROWS: usize = 5000; // per sheet or CSV
const MIN_IMAGE: u64 = 6 << 10; // smaller pictures are icons, bullets and logos
/// Cells kept per sheet (a sheet is cut off at MAX_ROWS rows or this many cells, whichever comes first).
const MAX_CELLS: usize = 1_000_000;
/// Columns a spreadsheet cell reference may name (Excel's own limit is 16,384).
const MAX_COLS: usize = 16_384;
/// Deepest list nesting and heading level that is shown; anything a file claims beyond that is clamped.
const MAX_LIST_DEPTH: i64 = 16;
/// Text taken from one document, in bytes, before the reading stops with an error.
const MAX_TEXT: usize = 128 << 20;
/// How long one document may take; the readers check it as they go, and `extract_isolated` also stops waiting.
const TIME_BUDGET: Duration = Duration::from_secs(45);
/// Stack of the extraction thread; the readers recurse one level per nested element (at most xml::MAX_DEPTH, a few KB of stack each in a debug build).
const STACK_SIZE: usize = 64 << 20;
const TOO_MUCH: &str = "This document has more text than the studio can read.";
const TOO_SLOW: &str = "Reading this document is taking too long, so it was stopped.";
const IMAGE_TYPES: [(&str, &str); 8] = [
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("bmp", "image/bmp"),
    ("webp", "image/webp"),
    ("tif", "image/tiff"),
    ("tiff", "image/tiff"),
];
const TEXT_EXT: [&str; 16] = [
    "txt", "md", "markdown", "csv", "tsv", "json", "xml", "log", "ini", "yaml", "yml", "srt",
    "vtt", "py", "js", "css",
];

/// What the user sees when a document can't be read. For the failures Python's ExtractError covers the text
/// is identical; for the ones server.py reports as "Couldn't read this file (…)" it is that sentence.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractError(pub String);

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ExtractError {}

/// Everything that can go wrong inside a reader, named after the Python exception it stands for.
#[derive(Debug)]
pub(crate) enum Fail {
    Extract(String),
    /// KeyError: a zip member that isn't there
    Key(String),
    /// ET.ParseError
    Parse,
    /// any other exception type (the server turns these into "Couldn't read this file (Name).")
    Other(&'static str),
}

pub(crate) type R<T> = Result<T, Fail>;

impl From<Fail> for ExtractError {
    fn from(f: Fail) -> Self {
        ExtractError(match f {
            Fail::Extract(m) => m,
            Fail::Key(name) => {
                let msg = format!(
                    "There is no item named {} in the archive",
                    text::py_repr(&name)
                );
                format!(
                    "This document is missing a part it needs ({}).",
                    text::py_repr(&msg)
                )
            }
            Fail::Parse => "This document's contents are damaged.".into(),
            Fail::Other(name) => format!("Couldn't read this file ({name})."),
        })
    }
}

struct Zip<'a> {
    ar: zip::ZipArchive<Cursor<&'a [u8]>>,
    names: Vec<String>,
    index: HashMap<String, usize>,
}

/// Collects blocks and pictures in reading order, split into parts at each picture.
pub(crate) struct Doc<'a> {
    zip: Option<Zip<'a>>,
    parts: Vec<Value>,
    blocks: Vec<Block>,
    images: usize,
    total: u64,
    /// text collected so far (bytes), against MAX_TEXT
    chars: usize,
    deadline: Instant,
    /// set once the text or time budget is spent: further blocks are dropped and the reading fails at the end
    over: Option<&'static str>,
}

impl<'a> Doc<'a> {
    fn new(zip: Option<Zip<'a>>) -> Self {
        Doc {
            zip,
            parts: vec![],
            blocks: vec![],
            images: 0,
            total: 0,
            chars: 0,
            deadline: Instant::now() + TIME_BUDGET,
            over: None,
        }
    }

    /// Fails once the text or time budget is spent, so a long loop can stop early.
    pub fn check(&mut self) -> R<()> {
        if self.over.is_none() && Instant::now() > self.deadline {
            self.over = Some(TOO_SLOW);
        }
        match self.over {
            Some(m) => Err(Fail::Extract(m.into())),
            None => Ok(()),
        }
    }

    fn zip(&self) -> &Zip<'a> {
        self.zip.as_ref().expect("zip document")
    }

    pub fn names(&self) -> &[String] {
        &self.zip().names
    }

    pub fn has(&self, name: &str) -> bool {
        self.zip().index.contains_key(name)
    }

    fn size(&mut self, name: &str) -> R<u64> {
        let i = *self
            .zip()
            .index
            .get(name)
            .ok_or_else(|| Fail::Key(name.to_string()))?;
        let z = self.zip.as_mut().expect("zip document");
        Ok(z.ar
            .by_index_raw(i)
            .map_err(|_| Fail::Other("BadZipFile"))?
            .size())
    }

    pub fn read(&mut self, name: &str) -> R<Vec<u8>> {
        let too_large = || Fail::Extract("The document is too large to read.".into());
        let size = self.size(name)?;
        let limit = MAX_MEMBER.min(MAX_TOTAL.saturating_sub(self.total));
        if size > limit {
            return Err(too_large());
        }
        let i = self.zip().index[name];
        let z = self.zip.as_mut().expect("zip document");
        let mut f = z.ar.by_index(i).map_err(|_| Fail::Other("RuntimeError"))?;
        let mut buf = Vec::new();
        // the limits count the bytes that are really read: the size in the zip directory is only a claim
        f.by_ref()
            .take(limit + 1)
            .read_to_end(&mut buf)
            .map_err(|_| Fail::Other("BadZipFile"))?;
        if buf.len() as u64 > limit {
            return Err(too_large());
        }
        self.total += buf.len() as u64;
        Ok(buf)
    }

    pub fn xml(&mut self, name: &str) -> R<Elem> {
        xml::parse(&self.read(name)?).map_err(|_| Fail::Parse)
    }

    pub fn add(&mut self, mut block: Block) {
        let _ = self.check();
        let len = match &block {
            Block::Table(rows) => rows.iter().flatten().map(String::len).sum(),
            Block::H(_, t) | Block::P(t) | Block::Li(_, _, t) | Block::Pre(_, t) => t.len(),
        };
        self.chars = self.chars.saturating_add(len);
        if self.chars > MAX_TEXT {
            self.over = Some(TOO_MUCH);
        }
        if self.over.is_some() {
            return;
        }
        match &mut block {
            Block::H(l, _) => *l = (*l).clamp(0, 6),
            Block::Li(d, _, _) => *d = (*d).clamp(0, MAX_LIST_DEPTH),
            _ => {}
        }
        let keep = match &block {
            Block::Table(_) => true,
            Block::H(_, t) | Block::P(t) | Block::Li(_, _, t) | Block::Pre(_, t) => {
                !text::strip(t).is_empty()
            }
        };
        if keep {
            self.blocks.push(block);
        }
    }

    pub fn flush(&mut self, label: &str) {
        let (text, md) = render::render(&self.blocks);
        if !text.is_empty() {
            self.parts
                .push(json!({"label": label, "text": text, "md": md}));
        }
        self.blocks.clear();
    }

    pub fn image(&mut self, name: &str, label: &str) -> R<()> {
        let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
        let Some(mime) = IMAGE_TYPES.iter().find(|(e, _)| *e == ext).map(|(_, m)| *m) else {
            return Ok(());
        };
        if self.images >= MAX_IMAGES || !self.has(name) || self.size(name)? < MIN_IMAGE {
            return Ok(());
        }
        self.flush(label);
        self.images += 1;
        let data = base64::engine::general_purpose::STANDARD.encode(self.read(name)?);
        self.parts.push(json!({"label": format!("{label}, picture {}", self.images), "image": format!("data:{mime};base64,{data}")}));
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Docx,
    Pptx,
    Xlsx,
    Odf,
    Epub,
}

fn zip_kind(ext: &str) -> Option<Kind> {
    Some(match ext {
        "docx" | "docm" | "dotx" => Kind::Docx,
        "pptx" | "pptm" | "ppsx" => Kind::Pptx,
        "xlsx" | "xlsm" => Kind::Xlsx,
        "odt" | "ott" | "odp" | "ods" => Kind::Odf,
        "epub" => Kind::Epub,
        _ => return None,
    })
}

/// Text (and pictures) out of a document, as the list of parts the UI reads.
pub fn extract(filename: &str, data: &[u8]) -> Result<Value, ExtractError> {
    let mut parts = read_parts(filename, data)?;
    for p in &mut parts {
        for k in ["text", "md"] {
            if let Some(Value::String(s)) = p.get_mut(k) {
                *s = s.replace("\r\n", "\n").replace('\r', "\n");
            }
        }
    }
    let keep = |p: &Value| {
        ["image", "text"].iter().any(|k| {
            p.get(*k)
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
        })
    };
    parts.retain(keep);
    Ok(Value::Array(parts))
}

/// `extract` for the callers that serve requests: runs it on its own thread with a big stack (so a
/// pathological file can't overflow a worker's), turns a panic into an error and stops waiting after a
/// while. The readers also check the time themselves, so the thread ends soon after.
pub async fn extract_isolated<D>(filename: String, data: D) -> Result<Value, ExtractError>
where
    D: AsRef<[u8]> + Send + 'static,
{
    const FAILED: &str = "Couldn't read this file (the reader stopped unexpectedly).";
    let waited = tokio::task::spawn_blocking(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("extract".into())
            .stack_size(STACK_SIZE)
            .spawn(move || {
                let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    extract(&filename, data.as_ref())
                }));
                let _ = tx.send(done);
            });
        if spawned.is_err() {
            return Err(ExtractError(FAILED.into()));
        }
        match rx.recv_timeout(TIME_BUDGET + Duration::from_secs(15)) {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err(ExtractError(FAILED.into())),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(ExtractError(TOO_SLOW.into())),
            Err(_) => Err(ExtractError(FAILED.into())),
        }
    })
    .await;
    waited.unwrap_or_else(|_| Err(ExtractError(FAILED.into())))
}

/// `<!doctype html` or `<html` at the start (after whitespace), any case.
fn looks_like_html(data: &[u8]) -> bool {
    let head = &data[..data.len().min(200)];
    let rest = &head[head
        .iter()
        .take_while(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0c | 0x0b))
        .count()..];
    let Some(rest) = rest.strip_prefix(b"<") else {
        return false;
    };
    let starts =
        |p: &str| rest.len() >= p.len() && rest[..p.len()].eq_ignore_ascii_case(p.as_bytes());
    starts("!doctype html") || starts("html")
}

fn read_parts(name: &str, data: &[u8]) -> Result<Vec<Value>, ExtractError> {
    let ext = text::ext(name);
    let legacy = match ext.as_str() {
        "doc" => Some("Word 97-2003 (.doc)"),
        "xls" => Some("Excel 97-2003 (.xls)"),
        "ppt" => Some("PowerPoint 97-2003 (.ppt)"),
        _ => None,
    };
    if let Some(l) = legacy {
        return Err(ExtractError(format!("{l} is an old binary format the studio can't read. Save it as the newer format (.{ext}x) or as PDF first.")));
    }
    if zip_kind(&ext).is_some() || data.starts_with(b"PK\x03\x04") {
        let ar = zip::ZipArchive::new(Cursor::new(data))
            .map_err(|_| ExtractError("This file looks damaged (not a valid document).".into()))?;
        let mut names = Vec::with_capacity(ar.len());
        let mut index = HashMap::new();
        let mut ar = ar;
        for i in 0..ar.len() {
            let n = ar
                .by_index_raw(i)
                .map_err(|_| ExtractError("Couldn't read this file (BadZipFile).".into()))?
                .name()
                .to_string();
            index.insert(n.clone(), i); // a repeated name resolves to the last entry, as in Python
            names.push(n);
        }
        let mut doc = Doc::new(Some(Zip { ar, names, index }));
        let kind = match zip_kind(&ext) {
            Some(k) => k,
            None => {
                // guess from the contents
                let has = |n: &str| doc.has(n);
                if has("word/document.xml") {
                    Kind::Docx
                } else if has("ppt/presentation.xml") {
                    Kind::Pptx
                } else if has("xl/workbook.xml") {
                    Kind::Xlsx
                } else if has("content.xml") {
                    Kind::Odf
                } else if has("META-INF/container.xml") {
                    Kind::Epub
                } else {
                    return Err(ExtractError(
                        "That zip file isn't a document the studio can read.".into(),
                    ));
                }
            }
        };
        match kind {
            Kind::Docx => ooxml::docx(&mut doc),
            Kind::Pptx => ooxml::pptx(&mut doc),
            Kind::Xlsx => ooxml::xlsx(&mut doc),
            Kind::Odf => odf::odf(&mut doc),
            Kind::Epub => epub::epub(&mut doc),
        }?;
        doc.check()?;
        return Ok(doc.parts);
    }
    let mut doc = Doc::new(None);
    if ext == "rtf" || data.starts_with(b"{\\rtf") {
        text::text_blocks(&mut doc, &rtf::rtf_text(data));
        doc.flush("Document");
    } else if matches!(ext.as_str(), "html" | "htm" | "xhtml") || looks_like_html(data) {
        html::html_blocks(&mut doc, data)?;
        doc.flush("Page");
    } else if ext == "md" || ext == "markdown" {
        let t = text::strip(&text::decode(data)).to_string();
        return Ok(vec![json!({"label": "Text", "text": t, "md": t})]);
    } else if ext == "csv" || ext == "tsv" {
        text::csv_table(&mut doc, data, ext == "tsv")?;
        doc.flush("Table");
    } else if ext == "json" {
        doc.add(Block::Pre("json".into(), text::json_text(data)));
        doc.flush("Text");
    } else if TEXT_EXT.contains(&ext.as_str()) || !data[..data.len().min(4096)].contains(&0) {
        text::text_blocks(&mut doc, text::strip(&text::decode(data)));
        doc.flush("Text");
    } else {
        return Err(ExtractError(
            "The studio can't read this kind of file.".into(),
        ));
    }
    doc.check()?;
    Ok(doc.parts)
}

#[cfg(test)]
mod tests;
