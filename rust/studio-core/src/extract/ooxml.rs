//! Office Open XML: Word (docx), PowerPoint (pptx), Excel (xlsx). Port of extract.py's readers.

use std::collections::HashMap;

use super::render::Block;
use super::text::{dirname, is_digit, join, normpath, py_int, strip};
use super::xml::{a, p, r, rel, s, v, w, Elem};
use super::{Doc, Fail, MAX_ROWS, R};

/// Relationship id -> target path for an OOXML part (a dict: a repeated id keeps its first position).
pub struct Rels(Vec<(Option<String>, String)>);

impl Rels {
    fn get(&self, id: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k.as_deref() == Some(id)).map(|(_, v)| v.as_str())
    }

    fn insert(&mut self, id: Option<String>, target: String) {
        match self.0.iter_mut().find(|(k, _)| *k == id) {
            Some(e) => e.1 = target,
            None => self.0.push((id, target)),
        }
    }
}

fn rels(doc: &mut Doc, part: &str) -> R<Rels> {
    let d = dirname(part);
    let f = &part[part.rfind('/').map_or(0, |i| i + 1)..];
    let path = join(&join(&d, "_rels"), &format!("{f}.rels"));
    let mut out = Rels(vec![]);
    if !doc.has(&path) {
        return Ok(out);
    }
    for e in doc.xml(&path)?.iter(Some(rel!("Relationship"))) {
        if e.get("TargetMode") != Some("External") {
            out.insert(e.get("Id").map(str::to_string), normpath(&join(&d, e.get("Target").unwrap_or(""))));
        }
    }
    Ok(out)
}

/// Text of a paragraph (runs, tabs, breaks, hyperlinks); pictures found inside go to pics.
fn runs(el: &Elem, pics: &mut Vec<String>) -> String {
    let mut out = String::new();
    for node in &el.kids {
        let t = node.tag.as_str();
        if t == w!("t") || t == a!("t") {
            out.push_str(&node.text);
        } else if t == w!("tab") {
            out.push('\t');
        } else if t == w!("br") || t == w!("cr") || t == a!("br") {
            out.push('\n');
        } else if t == a!("blip") || t == v!("imagedata") {
            let rid = node.get(r!("embed")).filter(|s| !s.is_empty()).or(node.get(r!("id")));
            if let Some(rid) = rid.filter(|s| !s.is_empty()) {
                pics.push(rid.to_string());
            }
        } else if ![w!("delText"), w!("instrText"), w!("pPr"), a!("pPr")].contains(&t) {
            out.push_str(&runs(node, pics));
        }
    }
    out
}

fn images(doc: &mut Doc, rel: &Rels, pics: &[String], label: &str) -> R<()> {
    for rid in pics {
        if let Some(target) = rel.get(rid) {
            doc.image(target, label)?;
        }
    }
    Ok(())
}

// ---------- Word ----------

type Key = Option<String>;

struct Styles {
    /// paragraph style id -> heading level
    heads: HashMap<Key, i64>,
    /// paragraph style id -> list numId ("bullet"/"number" for the built-in list styles)
    lists: HashMap<Key, Option<String>>,
    /// (numId, level) -> numbered rather than bulleted
    numbering: HashMap<(Key, Key), bool>,
}

/// `re.fullmatch(r"list (bullet|number)( \d)?", name)`
fn list_style_name(name: &str) -> bool {
    let rest = name.strip_prefix("list bullet").or_else(|| name.strip_prefix("list number"));
    rest.is_some_and(|r| r.is_empty() || (r.len() == 2 && r.starts_with(' ') && r.as_bytes()[1].is_ascii_digit()))
}

type StyleMaps = (HashMap<Key, i64>, HashMap<Key, Option<String>>); // heading levels, list kinds

fn docx_styles(doc: &mut Doc) -> R<StyleMaps> {
    let (mut heads, mut lists) = (HashMap::new(), HashMap::new());
    if !doc.has("word/styles.xml") {
        return Ok((heads, lists));
    }
    for st in doc.xml("word/styles.xml")?.iter(Some(w!("style"))) {
        let sid = st.get(w!("styleId")).map(str::to_string);
        let name = match st.child(w!("name")) {
            Some(n) => n.get(w!("val")).ok_or(Fail::Other("AttributeError"))?.to_lowercase(),
            None => String::new(),
        };
        let lvl = st.path(&[w!("pPr"), w!("outlineLvl")]);
        let level = name.strip_prefix("heading ").filter(|d| d.len() == 1 && d.as_bytes()[0].is_ascii_digit());
        if let Some(d) = level {
            heads.insert(sid.clone(), d.parse::<i64>().unwrap());
        } else if name == "title" {
            heads.insert(sid.clone(), 1);
        } else if let Some(l) = lvl {
            let val = l.get(w!("val")).ok_or(Fail::Other("TypeError"))?;
            if let Some(n) = py_int(val).filter(|&n| is_digit(val) && n < 6) {
                heads.insert(sid.clone(), n + 1);
            }
        }
        if let Some(num) = st.path(&[w!("pPr"), w!("numPr"), w!("numId")]) {
            lists.insert(sid, num.get(w!("val")).map(str::to_string));
        } else if list_style_name(&name) {
            lists.insert(sid, Some(if name.contains("bullet") { "bullet" } else { "number" }.to_string()));
        }
    }
    Ok((heads, lists))
}

fn docx_numbering(doc: &mut Doc) -> R<HashMap<(Key, Key), bool>> {
    if !doc.has("word/numbering.xml") {
        return Ok(HashMap::new());
    }
    let root = doc.xml("word/numbering.xml")?;
    let mut abstract_: HashMap<(Key, Key), bool> = HashMap::new();
    for a in root.iter(Some(w!("abstractNum"))) {
        for lvl in a.iter(Some(w!("lvl"))) {
            let ordered = lvl.child(w!("numFmt")).is_some_and(|f| !matches!(f.get(w!("val")), Some("bullet" | "none")));
            abstract_.insert((a.get(w!("abstractNumId")).map(str::to_string), lvl.get(w!("ilvl")).map(str::to_string)), ordered);
        }
    }
    let mut out = HashMap::new();
    for n in root.iter(Some(w!("num"))) {
        if let Some(aid) = n.child(w!("abstractNumId")) {
            for ((a, ilvl), ordered) in &abstract_ {
                if a.as_deref() == aid.get(w!("val")) {
                    out.insert((n.get(w!("numId")).map(str::to_string), ilvl.clone()), *ordered);
                }
            }
        }
    }
    Ok(out)
}

fn docx_body(doc: &mut Doc, root: &Elem, rel: &Rels, label: &str, st: &Styles) -> R<()> {
    for el in &root.kids {
        let t = el.tag.as_str();
        if t == w!("p") {
            let mut pics = vec![];
            let text = runs(el, &mut pics);
            let ppr = el.child(w!("pPr"));
            let style: Key = ppr.and_then(|p| p.child(w!("pStyle"))).and_then(|s| s.get(w!("val"))).map(str::to_string);
            let num = ppr.and_then(|p| p.child(w!("numPr")));
            let olvl = ppr.and_then(|p| p.child(w!("outlineLvl")));
            let olvl_ok = match olvl {
                Some(o) if !st.heads.contains_key(&style) => {
                    let val = o.get(w!("val")).ok_or(Fail::Other("TypeError"))?;
                    is_digit(val) && py_int(val).is_some_and(|n| n < 6)
                }
                _ => false,
            };
            if st.heads.contains_key(&style) || olvl_ok {
                let level = match st.heads.get(&style).copied().filter(|&l| l != 0) {
                    Some(l) => l,
                    None => {
                        let val = olvl.ok_or(Fail::Other("AttributeError"))?.get(w!("val")).ok_or(Fail::Other("TypeError"))?;
                        py_int(val).ok_or(Fail::Other("ValueError"))? + 1
                    }
                };
                doc.add(Block::H(level, strip(&text).to_string()));
            } else if num.is_some() || st.lists.contains_key(&style) {
                let ilvl = num.and_then(|n| n.child(w!("ilvl")));
                let depth = ilvl.map_or("0", |e| e.get(w!("val")).unwrap_or("0")).to_string();
                let nid: Option<String> = match num.and_then(|n| n.child(w!("numId"))) {
                    Some(e) => e.get(w!("val")).map(str::to_string),
                    None => st.lists.get(&style).cloned().flatten(),
                };
                if nid.as_deref() == Some("0") {
                    // numbering switched off for this paragraph
                    doc.add(Block::P(text));
                } else {
                    let ordered = nid.as_deref() == Some("number") || st.numbering.get(&(nid, Some(depth.clone()))).copied().unwrap_or(false);
                    doc.add(Block::Li(if is_digit(&depth) { py_int(&depth).unwrap_or(0) } else { 0 }, ordered, strip(&text).to_string()));
                }
            } else {
                doc.add(Block::P(text));
            }
            images(doc, rel, &pics, label)?;
        } else if t == w!("tbl") {
            let (mut rows, mut pics) = (vec![], vec![]);
            for tr in el.iter(Some(w!("tr"))) {
                rows.push(
                    tr.children(w!("tc"))
                        .map(|tc| {
                            let cell: Vec<String> = tc.iter(Some(w!("p"))).into_iter().map(|p| strip(&runs(p, &mut pics)).to_string()).collect();
                            strip(&cell.join(" ")).to_string()
                        })
                        .collect::<Vec<_>>(),
                );
            }
            doc.add(Block::Table(rows));
            images(doc, rel, &pics, label)?;
        } else if [w!("sdt"), w!("sdtContent"), w!("customXml"), w!("body")].contains(&t) {
            docx_body(doc, el, rel, label, st)?;
        }
    }
    Ok(())
}

pub fn docx(doc: &mut Doc) -> R<()> {
    let (heads, lists) = docx_styles(doc)?;
    let st = Styles { heads, lists, numbering: docx_numbering(doc)? };
    let root = doc.xml("word/document.xml")?;
    let body = root.child(w!("body"));
    let rel = rels(doc, "word/document.xml")?;
    docx_body(doc, body.ok_or(Fail::Other("TypeError"))?, &rel, "Document", &st)?;
    doc.flush("Document");
    let mut notes: Vec<String> = doc.names().iter().filter(|n| *n == "word/footnotes.xml" || *n == "word/endnotes.xml").cloned().collect();
    notes.sort();
    for name in notes {
        let root = doc.xml(&name)?;
        for note in &root.kids {
            if matches!(note.get(w!("type")), None | Some("normal")) {
                let rel = rels(doc, &name)?;
                docx_body(doc, note, &rel, "Notes", &st)?;
            }
        }
        doc.flush("Notes");
    }
    Ok(())
}

// ---------- PowerPoint ----------

/// Names matching `<prefix><digits>.xml`, in numeric order.
fn numbered(names: &[String], prefix: &str) -> Vec<String> {
    let num = |n: &str| n.strip_prefix(prefix).and_then(|r| r.strip_suffix(".xml")).filter(|d| is_digit(d)).map(|d| d.parse::<u128>().unwrap_or(u128::MAX));
    let mut v: Vec<&String> = names.iter().filter(|n| num(n).is_some()).collect();
    v.sort_by_key(|n| num(n));
    v.into_iter().cloned().collect()
}

fn pptx_shapes(doc: &mut Doc, tree: &Elem, rel: &Rels, label: &str) -> R<()> {
    for el in &tree.kids {
        let t = el.tag.as_str();
        if t == p!("grpSp") {
            pptx_shapes(doc, el, rel, label)?;
        } else if t == p!("sp") {
            let ph = el.path(&[p!("nvSpPr"), p!("nvPr"), p!("ph")]);
            let kind = ph.map(|ph| ph.get("type").unwrap_or("body"));
            let Some(body) = el.child(p!("txBody")) else { continue };
            for para in body.children(a!("p")) {
                let text = strip(&runs(para, &mut vec![])).to_string();
                let ppr = para.child(a!("pPr"));
                let depth = match ppr {
                    Some(pp) => py_int(pp.get("lvl").unwrap_or("0")).ok_or(Fail::Other("ValueError"))?,
                    None => 0,
                };
                let has = |tag: &str| ppr.is_some_and(|pp| pp.child(tag).is_some());
                let (bullet_off, auto, char_) = (has(a!("buNone")), has(a!("buAutoNum")), has(a!("buChar")));
                if matches!(kind, Some("title" | "ctrTitle")) {
                    doc.add(Block::H(1, text));
                } else if !bullet_off && (auto || char_ || matches!(kind, Some("body" | "obj"))) {
                    doc.add(Block::Li(depth, auto, text));
                } else {
                    doc.add(Block::P(text));
                }
            }
        } else if t == p!("graphicFrame") {
            for tbl in el.iter(Some(a!("tbl"))) {
                let rows = tbl
                    .children(a!("tr"))
                    .map(|tr| tr.children(a!("tc")).map(|tc| strip(&runs(tc, &mut vec![])).to_string()).collect::<Vec<_>>())
                    .collect();
                doc.add(Block::Table(rows));
            }
        } else if t == p!("pic") {
            let rid = el.desc(a!("blip")).and_then(|b| b.get(r!("embed")));
            if let Some(target) = rid.and_then(|id| rel.get(id)) {
                doc.image(target, label)?;
            }
        }
    }
    Ok(())
}

pub fn pptx(doc: &mut Doc) -> R<()> {
    for (i, name) in numbered(doc.names(), "ppt/slides/slide").iter().enumerate() {
        let i = i + 1;
        let rel = rels(doc, name)?;
        let root = doc.xml(name)?;
        if let Some(tree) = root.path(&[p!("cSld"), p!("spTree")]) {
            pptx_shapes(doc, tree, &rel, &format!("Slide {i}"))?;
        }
        doc.flush(&format!("Slide {i}"));
        for (_, note) in &rel.0 {
            let is_note = note.strip_prefix("ppt/notesSlides/notesSlide").and_then(|r| r.strip_suffix(".xml")).is_some_and(is_digit);
            if is_note && doc.has(note) {
                for sp in doc.xml(note)?.iter(Some(p!("sp"))) {
                    let ph = sp.path(&[p!("nvSpPr"), p!("nvPr"), p!("ph")]);
                    if ph.is_some_and(|ph| ph.get("type") == Some("body")) {
                        for para in sp.iter(Some(a!("p"))) {
                            doc.add(Block::P(runs(para, &mut vec![])));
                        }
                    }
                }
                doc.flush(&format!("Slide {i} notes"));
            }
        }
    }
    Ok(())
}

// ---------- Excel ----------

/// Column number (0-based) of a cell reference like "AB12".
fn col_index(cell: &str) -> R<usize> {
    let letters = cell.bytes().take_while(u8::is_ascii_uppercase);
    let mut n = 0usize;
    let mut any = false;
    for b in letters {
        any = true;
        n = n.saturating_mul(26).saturating_add((b - 64) as usize);
    }
    if !any {
        return Err(Fail::Other("AttributeError"));
    }
    if n > 1 << 20 {
        return Err(Fail::Other("MemoryError"));
    }
    Ok(n - 1)
}

pub fn xlsx(doc: &mut Doc) -> R<()> {
    let mut shared: Vec<String> = vec![];
    if doc.has("xl/sharedStrings.xml") {
        for si in doc.xml("xl/sharedStrings.xml")?.iter(Some(s!("si"))) {
            shared.push(si.iter(Some(s!("t"))).iter().map(|t| t.text.as_str()).collect());
        }
    }
    let rel = rels(doc, "xl/workbook.xml")?;
    let book = doc.xml("xl/workbook.xml")?;
    for sheet in book.iter(Some(s!("sheet"))) {
        let Some(target) = sheet.get(r!("id")).and_then(|id| rel.get(id)).filter(|t| !t.is_empty() && doc.has(t)) else { continue };
        let mut rows: Vec<Vec<String>> = vec![];
        for row in doc.xml(target)?.iter(Some(s!("row"))) {
            let mut cells: HashMap<usize, String> = HashMap::new();
            for c in row.iter(Some(s!("c"))) {
                let (kind, vel) = (c.get("t"), c.child(s!("v")));
                let vtext = vel.map(|v| v.text.as_str()).filter(|t| !t.is_empty());
                let shared_idx = vtext.filter(|t| is_digit(t) && kind == Some("s")).and_then(|t| t.parse::<usize>().ok()).filter(|&n| n < shared.len());
                let val = if let Some(n) = shared_idx {
                    shared[n].clone()
                } else if kind == Some("inlineStr") {
                    c.iter(Some(s!("t"))).iter().map(|t| t.text.as_str()).collect()
                } else {
                    vtext.unwrap_or("").to_string()
                };
                if let (false, Some(cell)) = (val.is_empty(), c.get("r").filter(|r| !r.is_empty())) {
                    cells.insert(col_index(cell)?, val.replace('\t', " "));
                }
            }
            if let Some(&max) = cells.keys().max() {
                rows.push((0..=max).map(|i| cells.get(&i).cloned().unwrap_or_default()).collect());
            }
            if rows.len() >= MAX_ROWS {
                break;
            }
        }
        if !rows.is_empty() {
            doc.add(Block::Table(rows));
        }
        doc.flush(sheet.get("name").filter(|n| !n.is_empty()).unwrap_or("Sheet"));
    }
    Ok(())
}
