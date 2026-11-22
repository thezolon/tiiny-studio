//! OpenDocument: text (odt), presentations (odp), spreadsheets (ods). Port of extract.py's `odf*`.

use std::collections::HashMap;

use super::render::Block;
use super::text::{py_int, strip};
use super::xml::{draw, office, pres, style, table, text, xlink, Elem};
use super::{rows_note, Doc, Fail, MAX_CELLS, MAX_COLS, MAX_ROWS, R};
use std::cell::Cell;

thread_local! {
    /// Set when a space run or paragraph was cut by a limit while reading; `odf` turns it into a visible note.
    static CUT: Cell<bool> = const { Cell::new(false) };
}

fn cut_note(doc: &mut Doc) {
    if CUT.replace(false) {
        doc.add(Block::P(
            "[… long runs of spaces or very long paragraphs were shortened]".into(),
        ));
    }
}

/// Longest run of spaces one `text:s` stands for, and the most text one paragraph keeps; a file can claim
/// any count, and 100 billion spaces would be one allocation.
const MAX_SPACES: i64 = 1000;
const MAX_INLINE: usize = 16 << 20;

/// List style name -> {level: numbered?}
type Styles = HashMap<Option<String>, HashMap<i64, bool>>;

fn int(v: Option<&str>, default: &str) -> R<i64> {
    py_int(v.unwrap_or(default)).ok_or(Fail::Other("ValueError"))
}

/// Text of an ODF paragraph, with its spaces, tabs and line breaks (notes and frames left out).
fn inline(el: &Elem) -> R<String> {
    let mut out = el.text.clone();
    for c in &el.kids {
        if out.len() > MAX_INLINE {
            CUT.set(true);
            break;
        }
        let t = c.tag.as_str();
        if t == text!("s") {
            let n = int(c.get(text!("c")), "1")?;
            if n > MAX_SPACES {
                CUT.set(true);
            }
            out.push_str(&" ".repeat(n.clamp(0, MAX_SPACES) as usize));
        } else if t == text!("tab") {
            out.push('\t');
        } else if t == text!("line-break") {
            out.push('\n');
        } else if ![text!("note"), draw!("frame"), draw!("a")].contains(&t) {
            out.push_str(&inline(c)?);
        }
        out.push_str(&c.tail);
    }
    Ok(out)
}

/// Text of every paragraph under `el`, each trimmed, joined by spaces.
fn paragraphs(el: &Elem, trim: bool) -> R<String> {
    let mut parts = vec![];
    for p in el.iter(Some(text!("p"))) {
        let t = inline(p)?;
        parts.push(if trim { strip(&t).to_string() } else { t });
    }
    Ok(parts.join(" "))
}

fn list_styles(doc: &mut Doc) -> R<Styles> {
    let mut out = Styles::new();
    for name in ["styles.xml", "content.xml"] {
        if doc.has(name) {
            for ls in doc.xml(name)?.iter(Some(text!("list-style"))) {
                let mut levels = HashMap::new();
                for l in &ls.kids {
                    levels.insert(
                        int(l.get(text!("level")), "1")? - 1,
                        l.tag == text!("list-level-style-number"),
                    );
                }
                out.insert(ls.get(style!("name")).map(str::to_string), levels);
            }
        }
    }
    Ok(out)
}

fn images(doc: &mut Doc, el: &Elem, label: &str) -> R<()> {
    for img in el.iter(Some(draw!("image"))) {
        doc.image(img.get(xlink!("href")).unwrap_or(""), label)?;
    }
    Ok(())
}

fn walk(
    doc: &mut Doc,
    items: &[&Elem],
    label: &str,
    styles: &Styles,
    depth: i64,
    style: Option<&str>,
) -> R<()> {
    for c in items {
        let t = c.tag.as_str();
        let kids: Vec<&Elem> = c.kids.iter().collect();
        if t == text!("h") {
            let level = int(c.get(text!("outline-level")), "1")?.min(6);
            doc.add(Block::H(level, strip(&inline(c)?).to_string()));
            images(doc, c, label)?;
        } else if t == text!("p") {
            doc.add(Block::P(inline(c)?));
            // a text box anchored in the paragraph (`inline` leaves frames out): its text follows
            let boxes = c.iter(Some(draw!("text-box")));
            if !boxes.is_empty() {
                walk(doc, &boxes, label, styles, depth, style)?;
            }
            images(doc, c, label)?;
        } else if t == text!("list") {
            let st = c
                .get(text!("style-name"))
                .filter(|s| !s.is_empty())
                .or(style);
            walk(doc, &kids, label, styles, depth + 1, st)?;
        } else if t == text!("list-item") || t == text!("list-header") {
            for item in &c.kids {
                if item.tag == text!("p") || item.tag == text!("h") {
                    let ordered = styles
                        .get(&style.map(str::to_string))
                        .and_then(|m| m.get(&depth.max(0)))
                        .copied()
                        .unwrap_or(false);
                    doc.add(Block::Li(
                        depth.max(0),
                        ordered,
                        strip(&inline(item)?).to_string(),
                    ));
                    images(doc, item, label)?;
                } else {
                    walk(doc, &[item], label, styles, depth, style)?;
                }
            }
        } else if t == table!("table") {
            let mut rows = vec![];
            for row in c.iter(Some(table!("table-row"))) {
                let mut cells = vec![];
                for cell in row.kids.iter().filter(|k| {
                    k.tag == table!("table-cell") || k.tag == table!("covered-table-cell")
                }) {
                    cells.push(paragraphs(cell, true)?);
                }
                rows.push(cells);
            }
            doc.add(Block::Table(rows));
            images(doc, c, label)?;
        } else if t == draw!("frame") || t == draw!("image") {
            if c.desc(draw!("text-box")).is_some() {
                walk(
                    doc,
                    &c.iter(Some(draw!("text-box"))),
                    label,
                    styles,
                    depth,
                    style,
                )?;
            }
            images(doc, c, label)?;
        } else {
            walk(doc, &kids, label, styles, depth, style)?;
        }
    }
    Ok(())
}

pub fn odf(doc: &mut Doc) -> R<()> {
    CUT.set(false);
    let content = doc.xml("content.xml")?;
    let Some(body) = content.child(office!("body")) else {
        return Ok(());
    };
    let styles = list_styles(doc)?;
    if let Some(pres_) = body.child(office!("presentation")) {
        for (i, page) in pres_.iter(Some(draw!("page"))).into_iter().enumerate() {
            let label = format!("Slide {}", i + 1);
            for frame in page.iter(Some(draw!("frame"))) {
                match frame.get(pres!("class")) {
                    Some("title") => doc.add(Block::H(1, paragraphs(frame, true)?)),
                    Some("outline") => {
                        for b in frame.iter(Some(draw!("text-box"))) {
                            walk(
                                doc,
                                &b.kids.iter().collect::<Vec<_>>(),
                                &label,
                                &styles,
                                -1,
                                None,
                            )?;
                        }
                    }
                    _ => walk(doc, &[frame], &label, &styles, -1, None)?,
                }
            }
            cut_note(doc);
            doc.flush(&label);
        }
    } else if let Some(sheets) = body.child(office!("spreadsheet")) {
        for t in sheets.iter(Some(table!("table"))) {
            let mut rows: Vec<Vec<String>> = vec![];
            let mut cell_count = 0usize;
            let (mut rows_left, mut repeat_cut, mut cols_cut) = (0usize, false, false);
            doc.check()?;
            let row_els = t.iter(Some(table!("table-row")));
            for (ri, row) in row_els.iter().enumerate() {
                let mut cells: Vec<String> = vec![];
                let mut gap_cut = false;
                for c in row.kids.iter().filter(|k| {
                    k.tag == table!("table-cell") || k.tag == table!("covered-table-cell")
                }) {
                    let val = paragraphs(c, false)?;
                    // a gap of empty cells is one cell repeated: keep it, or what follows lands in the wrong column
                    let wanted = int(c.get(table!("number-columns-repeated")), "1")?;
                    let n = wanted.min(if val.is_empty() { 256 } else { 50 });
                    if wanted > n {
                        if val.is_empty() {
                            gap_cut = true;
                        } else {
                            repeat_cut = true;
                        }
                    }
                    if !val.is_empty() && gap_cut {
                        repeat_cut = true; // something follows a gap that was shortened
                    }
                    for _ in 0..n.max(0) {
                        if cells.len() >= MAX_COLS {
                            cols_cut |= !val.is_empty();
                            break;
                        }
                        cells.push(val.clone());
                    }
                }
                while cells.last().is_some_and(String::is_empty) {
                    cells.pop();
                }
                if !cells.is_empty() {
                    cell_count += cells.len();
                    rows.push(cells);
                }
                if rows.len() >= MAX_ROWS || cell_count >= MAX_CELLS {
                    rows_left = row_els.len() - ri - 1;
                    break;
                }
            }
            if !rows.is_empty() {
                doc.add(Block::Table(rows));
            }
            if rows_left > 0 {
                doc.add(Block::P(rows_note(rows_left)));
            }
            if repeat_cut {
                doc.add(Block::P("[… some repeated cells were shortened]".into()));
            }
            if cols_cut {
                doc.add(Block::P(format!(
                    "[… cells beyond column {MAX_COLS} not shown]"
                )));
            }
            cut_note(doc);
            doc.flush(
                t.get(table!("name"))
                    .filter(|n| !n.is_empty())
                    .unwrap_or("Sheet"),
            );
        }
    } else {
        walk(
            doc,
            &body.kids.iter().collect::<Vec<_>>(),
            "Document",
            &styles,
            -1,
            None,
        )?;
        cut_note(doc);
        doc.flush("Document");
    }
    Ok(())
}
