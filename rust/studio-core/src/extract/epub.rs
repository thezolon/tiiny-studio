//! EPUB: the spine's HTML files in reading order, one part per section. Port of extract.py's `epub`.

use std::collections::HashMap;

use super::text::{dirname, join, normpath};
use super::{html, Doc, Fail, R};

pub fn epub(doc: &mut Doc) -> R<()> {
    let container = doc.xml("META-INF/container.xml")?;
    let rootfile = container
        .iter(None)
        .into_iter()
        .find(|e| e.tag.ends_with("rootfile"))
        .and_then(|e| e.get("full-path"))
        .map(str::to_string);
    let Some(rootfile) = rootfile.filter(|r| !r.is_empty() && doc.has(r)) else {
        return Err(Fail::Extract(
            "This EPUB has no readable contents list.".into(),
        ));
    };
    let opf = doc.xml(&rootfile)?;
    let base = dirname(&rootfile);
    let mut items: HashMap<Option<&str>, Option<&str>> = HashMap::new();
    for e in opf
        .iter(None)
        .into_iter()
        .filter(|e| e.tag.ends_with("}item"))
    {
        items.insert(e.get("id"), e.get("href"));
    }
    let spine = opf
        .iter(None)
        .into_iter()
        .filter(|e| e.tag.ends_with("}itemref"))
        .map(|e| e.get("idref"));
    for (i, idref) in spine.enumerate() {
        let path = items
            .get(&idref)
            .copied()
            .flatten()
            .filter(|h| !h.is_empty())
            .map(|h| normpath(&join(&base, h)));
        if let Some(path) = path.filter(|p| doc.has(p)) {
            let raw = doc.read(&path)?;
            html::html_blocks(doc, &raw)?;
            doc.flush(&format!("Section {}", i + 1));
        }
    }
    Ok(())
}
