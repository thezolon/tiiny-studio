//! A small ElementTree-style XML tree: tags and attribute names are "{namespace}local", an element has
//! `text` (before its first child) and `tail` (after its end), comments and processing instructions are
//! dropped. Built on roxmltree; extract.py's `ET.fromstring` is the model.

use roxmltree::{Document, Node, ParsingOptions};

/// Namespace-qualified names as ElementTree writes them: `w!("p")` is "{...wordprocessingml/2006/main}p".
macro_rules! w {
    ($t:literal) => {
        concat!(
            "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}",
            $t
        )
    };
}
macro_rules! a {
    ($t:literal) => {
        concat!(
            "{http://schemas.openxmlformats.org/drawingml/2006/main}",
            $t
        )
    };
}
macro_rules! p {
    ($t:literal) => {
        concat!(
            "{http://schemas.openxmlformats.org/presentationml/2006/main}",
            $t
        )
    };
}
macro_rules! r {
    ($t:literal) => {
        concat!(
            "{http://schemas.openxmlformats.org/officeDocument/2006/relationships}",
            $t
        )
    };
}
macro_rules! s {
    ($t:literal) => {
        concat!(
            "{http://schemas.openxmlformats.org/spreadsheetml/2006/main}",
            $t
        )
    };
}
macro_rules! rel {
    ($t:literal) => {
        concat!(
            "{http://schemas.openxmlformats.org/package/2006/relationships}",
            $t
        )
    };
}
macro_rules! v {
    ($t:literal) => {
        concat!("{urn:schemas-microsoft-com:vml}", $t)
    };
}
macro_rules! text {
    ($t:literal) => {
        concat!("{urn:oasis:names:tc:opendocument:xmlns:text:1.0}", $t)
    };
}
macro_rules! table {
    ($t:literal) => {
        concat!("{urn:oasis:names:tc:opendocument:xmlns:table:1.0}", $t)
    };
}
macro_rules! draw {
    ($t:literal) => {
        concat!("{urn:oasis:names:tc:opendocument:xmlns:drawing:1.0}", $t)
    };
}
macro_rules! office {
    ($t:literal) => {
        concat!("{urn:oasis:names:tc:opendocument:xmlns:office:1.0}", $t)
    };
}
macro_rules! style {
    ($t:literal) => {
        concat!("{urn:oasis:names:tc:opendocument:xmlns:style:1.0}", $t)
    };
}
macro_rules! pres {
    ($t:literal) => {
        concat!(
            "{urn:oasis:names:tc:opendocument:xmlns:presentation:1.0}",
            $t
        )
    };
}
macro_rules! xlink {
    ($t:literal) => {
        concat!("{http://www.w3.org/1999/xlink}", $t)
    };
}
pub(crate) use {a, draw, office, p, pres, r, rel, s, style, table, text, v, w, xlink};

#[derive(Debug, Default)]
pub struct Elem {
    pub tag: String,
    pub attrs: Vec<(String, String)>,
    pub text: String,
    pub tail: String,
    pub kids: Vec<Elem>,
}

impl Elem {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// `el.find(tag)`: the first direct child with that tag.
    pub fn child(&self, tag: &str) -> Option<&Elem> {
        self.kids.iter().find(|k| k.tag == tag)
    }

    /// `el.find("a/b/c")`.
    pub fn path(&self, tags: &[&str]) -> Option<&Elem> {
        tags.iter().try_fold(self, |e, t| e.child(t))
    }

    /// `el.findall(tag)`.
    pub fn children<'a>(&'a self, tag: &'a str) -> impl Iterator<Item = &'a Elem> {
        self.kids.iter().filter(move |k| k.tag == tag)
    }

    /// `el.iter(tag)`: the element and its descendants in document order (`None` = every tag).
    pub fn iter(&self, tag: Option<&str>) -> Vec<&Elem> {
        fn walk<'a>(e: &'a Elem, tag: Option<&str>, out: &mut Vec<&'a Elem>) {
            if tag.is_none_or(|t| e.tag == t) {
                out.push(e);
            }
            for k in &e.kids {
                walk(k, tag, out);
            }
        }
        let mut out = Vec::new();
        walk(self, tag, &mut out);
        out
    }

    /// `el.find(".//tag")`: the first descendant (not the element itself).
    pub fn desc(&self, tag: &str) -> Option<&Elem> {
        self.iter(Some(tag))
            .into_iter()
            .find(|e| !std::ptr::eq(*e, self))
    }
}

fn qname(ns: Option<&str>, local: &str) -> String {
    match ns {
        Some(ns) => format!("{{{ns}}}{local}"),
        None => local.to_string(),
    }
}

fn convert(n: Node) -> Elem {
    let tn = n.tag_name();
    let mut e = Elem {
        tag: qname(tn.namespace(), tn.name()),
        ..Elem::default()
    };
    e.attrs = n
        .attributes()
        .map(|a| (qname(a.namespace(), a.name()), a.value().to_string()))
        .collect();
    for c in n.children() {
        if c.is_element() {
            e.kids.push(convert(c));
        } else if c.is_text() {
            let t = c.text().unwrap_or("");
            match e.kids.last_mut() {
                Some(last) => last.tail.push_str(t),
                None => e.text.push_str(t),
            }
        }
    }
    e
}

/// The document as text: UTF-8 (or UTF-16 with a BOM), else the encoding its XML declaration names.
fn to_text(b: &[u8]) -> Option<String> {
    if b.starts_with(&[0xff, 0xfe]) || b.starts_with(&[0xfe, 0xff]) {
        let be = b[0] == 0xfe;
        let units: Vec<u16> = b[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| {
                if be {
                    u16::from_be_bytes(c)
                } else {
                    u16::from_le_bytes(c)
                }
            })
            .collect();
        return String::from_utf16(&units).ok();
    }
    let b = b.strip_prefix(&[0xef, 0xbb, 0xbf][..]).unwrap_or(b);
    if let Ok(s) = std::str::from_utf8(b) {
        return Some(s.to_string());
    }
    let head = String::from_utf8_lossy(&b[..b.len().min(200)]).to_lowercase();
    let label = head
        .split("encoding=")
        .nth(1)?
        .trim_start_matches(['"', '\''])
        .split(['"', '\''])
        .next()?
        .to_string();
    let enc = encoding_rs::Encoding::for_label(label.as_bytes())?;
    enc.decode_without_bom_handling_and_without_replacement(b)
        .map(|c| c.into_owned())
}

/// `ET.fromstring`; Err(()) is a parse error.
pub fn parse(bytes: &[u8]) -> Result<Elem, ()> {
    let text = to_text(bytes).ok_or(())?;
    let opts = ParsingOptions {
        allow_dtd: true,
        ..ParsingOptions::default()
    };
    let doc = Document::parse_with_options(&text, opts).map_err(|_| ())?;
    Ok(convert(doc.root_element()))
}
