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

use base64::Engine;
use serde_json::{json, Value};

use render::Block;
use xml::Elem;

const MAX_MEMBER: u64 = 64 << 20; // per file inside a zip: guards against zip bombs
const MAX_TOTAL: u64 = 256 << 20;
const MAX_IMAGES: usize = 50;
const MAX_ROWS: usize = 5000; // per sheet or CSV
const MIN_IMAGE: u64 = 6 << 10; // smaller pictures are icons, bullets and logos
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
}

impl<'a> Doc<'a> {
    fn new(zip: Option<Zip<'a>>) -> Self {
        Doc {
            zip,
            parts: vec![],
            blocks: vec![],
            images: 0,
            total: 0,
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
        let size = self.size(name)?;
        if size > MAX_MEMBER || self.total + size > MAX_TOTAL {
            return Err(Fail::Extract("The document is too large to read.".into()));
        }
        self.total += size;
        let i = self.zip().index[name];
        let z = self.zip.as_mut().expect("zip document");
        let mut f = z.ar.by_index(i).map_err(|_| Fail::Other("RuntimeError"))?;
        let mut buf = Vec::new();
        f.by_ref()
            .take(MAX_MEMBER + 1)
            .read_to_end(&mut buf)
            .map_err(|_| Fail::Other("BadZipFile"))?;
        buf.truncate(MAX_MEMBER as usize);
        Ok(buf)
    }

    pub fn xml(&mut self, name: &str) -> R<Elem> {
        xml::parse(&self.read(name)?).map_err(|_| Fail::Parse)
    }

    pub fn add(&mut self, block: Block) {
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
    Ok(doc.parts)
}
