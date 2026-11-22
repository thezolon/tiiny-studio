//! A streaming reader for the big spreadsheet parts (sheets, shared strings): quick-xml events, one element
//! built at a time, so a large sheet never becomes a whole tree in memory.
//!
//! The tree reader (xml.rs) stays the reference. Whatever this reader is not sure about (a DOCTYPE, an
//! undefined entity or prefix, a control character, unbalanced or repeated roots, nested rows or cells)
//! comes back as `Unsupported`, and the caller reads the part with the tree reader, which decides exactly
//! as before, errors included.

use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::{NsReader, XmlVersion};

use super::xml::Elem;

pub struct Unsupported;
pub type S<T> = Result<T, Unsupported>;

pub enum Ev {
    Start(Elem),
    Empty(Elem),
    End,
    Text(String),
    Eof,
}

pub struct Reader<'a> {
    rd: NsReader<&'a [u8]>,
    depth: usize,
    roots: usize,
}

/// Whether a character may appear in an XML 1.0 document.
fn xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..)
}

impl<'a> Reader<'a> {
    /// `text` is the part decoded and depth-checked by `xml::checked_text`.
    pub fn new(text: &'a str) -> S<Self> {
        // what the tree reader refuses and quick-xml would let through
        if text
            .bytes()
            .any(|b| b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r'))
            || text.contains(['\u{FFFE}', '\u{FFFF}'])
            || text.matches("]]>").count() != text.matches("<![CDATA[").count()
        {
            return Err(Unsupported);
        }
        let mut rd = NsReader::from_str(text);
        rd.config_mut().check_comments = true;
        Ok(Reader {
            rd,
            depth: 0,
            roots: 0,
        })
    }

    fn head(&self, e: &BytesStart, tag: String) -> S<Elem> {
        let mut el = Elem {
            tag,
            ..Elem::default()
        };
        for a in e.attributes() {
            let a = a.map_err(|_| Unsupported)?;
            if a.key.as_namespace_binding().is_some() {
                continue;
            }
            let (ns, local) = self.rd.resolver().resolve_attribute(a.key);
            let name = match ns {
                ResolveResult::Bound(n) => format!("{{{}}}{}", n.0, local.as_ref()),
                ResolveResult::Unbound => local.as_ref().to_string(),
                ResolveResult::Unknown(_) => return Err(Unsupported),
            };
            let value = a
                .normalized_value(XmlVersion::Implicit1_0)
                .map_err(|_| Unsupported)?;
            if value.chars().any(|c| !xml_char(c)) {
                return Err(Unsupported);
            }
            el.attrs.push((name, value.into_owned()));
        }
        Ok(el)
    }

    fn root(&mut self) -> S<()> {
        if self.depth == 0 {
            if self.roots > 0 {
                return Err(Unsupported);
            }
            self.roots += 1;
        }
        Ok(())
    }

    pub fn next(&mut self) -> S<Ev> {
        loop {
            let (ns, ev) = self.rd.read_resolved_event().map_err(|_| Unsupported)?;
            let tag = |e: &BytesStart| match &ns {
                ResolveResult::Bound(n) => Ok(format!("{{{}}}{}", n.0, e.local_name().as_ref())),
                ResolveResult::Unbound => Ok(e.local_name().as_ref().to_string()),
                ResolveResult::Unknown(_) => Err(Unsupported),
            };
            return match ev {
                Event::Start(e) => {
                    let tag = tag(&e)?;
                    self.root()?;
                    self.depth += 1;
                    Ok(Ev::Start(self.head(&e, tag)?))
                }
                Event::Empty(e) => {
                    let tag = tag(&e)?;
                    self.root()?;
                    Ok(Ev::Empty(self.head(&e, tag)?))
                }
                Event::End(_) => {
                    self.depth = self.depth.checked_sub(1).ok_or(Unsupported)?;
                    Ok(Ev::End)
                }
                Event::Text(t) => {
                    let t = t.xml10_content().into_owned();
                    if self.depth == 0 && !t.trim().is_empty() {
                        return Err(Unsupported);
                    }
                    Ok(Ev::Text(t))
                }
                Event::CData(t) if self.depth > 0 => Ok(Ev::Text(t.xml10_content().into_owned())),
                Event::GeneralRef(r) if self.depth > 0 => {
                    let c = match r.resolve_char_ref().map_err(|_| Unsupported)? {
                        Some(c) => c,
                        None => match &*r {
                            "lt" => '<',
                            "gt" => '>',
                            "amp" => '&',
                            "apos" => '\'',
                            "quot" => '"',
                            _ => return Err(Unsupported),
                        },
                    };
                    if !xml_char(c) {
                        return Err(Unsupported);
                    }
                    Ok(Ev::Text(c.to_string()))
                }
                Event::Comment(_) | Event::PI(_) | Event::Decl(_) => continue,
                Event::Eof => {
                    if self.depth != 0 || self.roots == 0 {
                        return Err(Unsupported);
                    }
                    Ok(Ev::Eof)
                }
                // a DOCTYPE (entities the tree reader resolves), or content outside the root
                _ => Err(Unsupported),
            };
        }
    }

    /// The element whose start tag `head` came from, with everything below it (text and tails as in xml.rs).
    pub fn element(&mut self, head: Elem) -> S<Elem> {
        let mut stack = vec![head];
        loop {
            match self.next()? {
                Ev::Start(e) => stack.push(e),
                Ev::Empty(e) => stack.last_mut().ok_or(Unsupported)?.kids.push(e),
                Ev::End => {
                    let done = stack.pop().ok_or(Unsupported)?;
                    match stack.last_mut() {
                        Some(parent) => parent.kids.push(done),
                        None => return Ok(done),
                    }
                }
                Ev::Text(t) => {
                    let top = stack.last_mut().ok_or(Unsupported)?;
                    match top.kids.last_mut() {
                        Some(last) => last.tail.push_str(&t),
                        None => top.text.push_str(&t),
                    }
                }
                Ev::Eof => return Err(Unsupported),
            }
        }
    }
}
