//! HTML into blocks: headings, paragraphs, (nested) list items, tables and preformatted text. Port of
//! extract.py's `HTMLBlocks` on top of a port of Python's `html.parser.HTMLParser` (convert_charrefs=True),
//! because the exact tokenizing quirks decide what comes out for messy pages.

use std::sync::LazyLock;

use regex::Regex;

use super::entities::HTML5;
use super::render::Block;
use super::text::{decode, is_space, strip};
use super::{Doc, Fail, R};

// ---------- html.unescape ----------

fn named(k: &str) -> Option<&'static str> {
    HTML5
        .binary_search_by(|(name, _)| (*name).cmp(k))
        .ok()
        .map(|i| HTML5[i].1)
}

/// Length in bytes of the character reference after an '&' (Python's `_charref` regex), if there is one.
fn ref_len(rest: &str) -> Option<usize> {
    let semi = |s: &str, at: usize| usize::from(s[at..].starts_with(';'));
    if let Some(r) = rest.strip_prefix('#') {
        let d = r.bytes().take_while(u8::is_ascii_digit).count();
        if d > 0 {
            return Some(1 + d + semi(r, d));
        }
        let hex = r.strip_prefix(['x', 'X'])?;
        let h = hex.bytes().take_while(u8::is_ascii_hexdigit).count();
        return (h > 0).then(|| 2 + h + semi(hex, h));
    }
    let n: usize = rest
        .chars()
        .take(32)
        .take_while(|c| !matches!(c, '\t' | '\n' | '\x0c' | ' ' | '<' | '&' | '#' | ';'))
        .map(char::len_utf8)
        .sum();
    (n > 0).then(|| n + semi(rest, n))
}

/// What a numeric reference 0x80-0x9f stands for (Windows-1252; the five unassigned ones stay as they are).
const WIN1252: [char; 32] = [
    '\u{20ac}', '\u{81}', '\u{201a}', '\u{192}', '\u{201e}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{2c6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{8d}', '\u{17d}', '\u{8f}',
    '\u{90}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{2dc}', '\u{2122}', '\u{161}', '\u{203a}', '\u{153}', '\u{9d}', '\u{17e}', '\u{178}',
];

fn replace_ref(s: &str) -> String {
    if let Some(r) = s.strip_prefix('#') {
        let (digits, radix) = match r.strip_prefix(['x', 'X']) {
            Some(h) => (h, 16),
            None => (r, 10),
        };
        let num = u64::from_str_radix(digits.trim_end_matches(';'), radix).unwrap_or(u64::MAX);
        return match num {
            // Python's _invalid_charrefs: NUL, CR and the Windows-1252 range 0x80-0x9f
            0x0 => "\u{fffd}".into(),
            0xd => "\r".into(),
            0x80..=0x9f => {
                let c = WIN1252[(num - 0x80) as usize];
                c.to_string()
            }
            0xd800..=0xdfff | 0x110000.. => "\u{fffd}".into(),
            // the code points HTML5 calls invalid are dropped
            0x1..=0x8 | 0xb | 0xe..=0x1f | 0x7f | 0xfdd0..=0xfdef => String::new(),
            n if n & 0xfffe == 0xfffe => String::new(),
            n => char::from_u32(n as u32)
                .map(String::from)
                .unwrap_or_default(),
        };
    }
    if let Some(v) = named(s) {
        return v.to_string();
    }
    // the longest matching name, as the standard defines
    let idx: Vec<usize> = s.char_indices().map(|(i, _)| i).collect();
    for x in (2..idx.len()).rev() {
        if let Some(v) = named(&s[..idx[x]]) {
            return format!("{v}{}", &s[idx[x]..]);
        }
    }
    format!("&{s}")
}

/// Python's `html.unescape`.
pub fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        let after = &rest[p + 1..];
        match ref_len(after) {
            Some(n) => {
                out.push_str(&replace_ref(&after[..n]));
                rest = &after[n..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

// ---------- the tokenizer ----------

fn re(p: &str) -> Regex {
    Regex::new(p).expect("valid pattern")
}

// The patterns that look back or ahead (tag names, attributes, end tags) are scanned by hand below: run through
// a backtracking regex engine on the rest of the document they cost time proportional to everything that follows
// (quadratic over a page) and fail outright on a long attribute. These have no look-around and run on `regex`,
// which is linear.
static COMMENT_CLOSE: LazyLock<Regex> = LazyLock::new(|| re(r"--\s*>"));
static MARKED_CLOSE: LazyLock<Regex> = LazyLock::new(|| re(r"\]\s*\]\s*>"));
static MS_MARKED_CLOSE: LazyLock<Regex> = LazyLock::new(|| re(r"\]\s*>"));
static SCRIPT_CLOSE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)</\s*script\s*>"));
static STYLE_CLOSE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)</\s*style\s*>"));

fn find_from(r: &Regex, raw: &str, at: usize) -> R<Option<(usize, usize)>> {
    Ok(r.find_at(raw, at).map(|m| (m.start(), m.end())))
}

/// End of the run of characters from `at` on that satisfy `f`.
fn skip_while(raw: &str, at: usize, f: impl Fn(char) -> bool) -> usize {
    raw[at..].find(|c| !f(c)).map_or(raw.len(), |p| at + p)
}

/// A character of a tag name: `[^\t\n\r\f />\x00]`
fn name_char(c: char) -> bool {
    !matches!(c, '\t' | '\n' | '\r' | '\x0c' | ' ' | '/' | '>' | '\0')
}

/// `(?:\s|/(?!>))*`
fn skip_space_slash(raw: &str, mut k: usize) -> usize {
    while let Some(c) = raw[k..].chars().next() {
        if is_space(c) || (c == '/' && !raw[k + 1..].starts_with('>')) {
            k += c.len_utf8();
        } else {
            break;
        }
    }
    k
}

/// `([a-zA-Z][^\t\n\r\f />\x00]*)(?:\s|/(?!>))*` at `at`: where it ends and the name.
fn tag_find(raw: &str, at: usize) -> Option<(usize, &str)> {
    if !raw
        .get(at..)?
        .starts_with(|c: char| c.is_ascii_alphabetic())
    {
        return None;
    }
    let name_end = skip_while(raw, at + 1, name_char);
    Some((skip_space_slash(raw, name_end), &raw[at..name_end]))
}

/// The value part of an attribute, `\s*=+\s*('[^']*'|"[^"]*"|(?!['"])[^>\s]*)`, starting after the name; where it
/// ends. When a quote is never closed the regex backs off (fewer spaces, then fewer `=`) until the unquoted
/// alternative fits, and that is what this does too.
fn attr_value(raw: &str, at: usize) -> Option<usize> {
    let ws = skip_while(raw, at, is_space);
    let eq_end = skip_while(raw, ws, |c| c == '=');
    if eq_end == ws {
        return None;
    }
    let p = skip_while(raw, eq_end, is_space);
    let unquoted = |p: usize| skip_while(raw, p, |c| c != '>' && !is_space(c));
    let quote = raw[p..].chars().next().filter(|c| matches!(c, '\'' | '"'));
    match quote {
        None => Some(unquoted(p)),
        Some(q) => match raw[p + 1..].find(q) {
            Some(e) => Some(p + 1 + e + 1),
            None if p > eq_end => raw[..p].char_indices().next_back().map(|(i, _)| i), // an empty value
            None if eq_end - ws >= 2 => Some(unquoted(eq_end - 1)),
            None => None,
        },
    }
}

/// One attribute at `k` (Python's `attrfind_tolerant`): its end. The character before `k` has to be a quote,
/// a space or a slash.
fn attr_at(raw: &str, k: usize) -> Option<usize> {
    let prev = raw[..k].chars().next_back()?;
    if !(matches!(prev, '\'' | '"' | '/') || is_space(prev)) {
        return None;
    }
    let first = raw[k..].chars().next()?;
    if is_space(first) || first == '/' || first == '>' {
        return None;
    }
    let mut e = skip_while(raw, k + first.len_utf8(), |c| {
        !(is_space(c) || matches!(c, '/' | '=' | '>'))
    });
    if let Some(v) = attr_value(raw, e) {
        e = v;
    }
    Some(skip_space_slash(raw, e))
}

/// Python's `locatestarttagend_tolerant` at the `<` at `i` (followed by a letter): where the tag's name and
/// attributes end.
fn locate(raw: &str, i: usize) -> usize {
    let mut j = skip_while(raw, i + 2, name_char);
    j = skip_while(raw, j, |c| is_space(c) || c == '/');
    while let Some(e) = attr_at(raw, j) {
        j = e;
    }
    skip_while(raw, j, is_space)
}

/// `</\s*([a-zA-Z][-.a-zA-Z0-9:_]*)\s*>` at `i`: where it ends and the name.
fn end_tag_find(raw: &str, i: usize) -> Option<(usize, &str)> {
    raw[i..].strip_prefix("</")?;
    let s = skip_while(raw, i + 2, is_space);
    if !raw[s..].starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    let name_end = skip_while(raw, s + 1, |c| {
        c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | ':' | '_')
    });
    let t = skip_while(raw, name_end, is_space);
    raw[t..]
        .starts_with('>')
        .then(|| (t + 1, &raw[s..name_end]))
}

/// `[a-zA-Z][-_.a-zA-Z0-9]*\s*` at `at`: where it ends.
fn decl_name(raw: &str, at: usize) -> Option<usize> {
    if !raw
        .get(at..)?
        .starts_with(|c: char| c.is_ascii_alphabetic())
    {
        return None;
    }
    let e = skip_while(raw, at + 1, |c| {
        c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')
    });
    Some(skip_while(raw, e, is_space))
}

fn find_char(raw: &str, c: char, from: usize) -> Option<usize> {
    raw.get(from..)?.find(c).map(|p| from + p)
}

const BLOCK: [&str; 18] = [
    "p",
    "div",
    "section",
    "article",
    "blockquote",
    "header",
    "footer",
    "main",
    "aside",
    "nav",
    "figure",
    "figcaption",
    "dd",
    "dt",
    "address",
    "hr",
    "title",
    "body",
];
const SKIP: [&str; 6] = ["script", "style", "head", "noscript", "svg", "template"];

fn is_heading(tag: &str) -> bool {
    tag.len() == 2 && tag.starts_with('h') && matches!(tag.as_bytes()[1], b'1'..=b'6')
}

/// A table being read: rows of indexes into `cells`.
#[derive(Default)]
struct Tbl {
    rows: Vec<Vec<usize>>,
    cells: Vec<Vec<String>>,
    row: Option<usize>,
    cell: Option<usize>,
}

/// What may stand in a `<head>`; any other start tag ends it, since its end tag is optional.
const HEAD_TAGS: [&str; 10] = [
    "head", "html", "title", "meta", "link", "base", "style", "script", "noscript", "template",
];

/// HTMLParser plus the HTMLBlocks handlers.
struct Parser<'d, 'z> {
    doc: &'d mut Doc<'z>,
    buf: Vec<String>,
    kind: String,
    skip: usize,
    lists: Vec<bool>,
    /// tables under construction, the innermost last (a table inside a cell is a cell of the outer one)
    tables: Vec<Tbl>,
    /// a `<head>` is open (it counts once in `skip`)
    head: bool,
    pre: usize,
    cdata: Option<&'static str>,
}

impl Parser<'_, '_> {
    fn emit(&mut self) {
        let mut text = self.buf.concat();
        if self.pre == 0 {
            text = collapse(&text);
        }
        let kind = std::mem::replace(&mut self.kind, "p".into());
        self.buf.clear();
        if kind == "pre" {
            self.doc.add(Block::Pre(
                String::new(),
                text.trim_matches('\n').to_string(),
            ));
        } else if is_heading(&kind) {
            self.doc.add(Block::H(
                i64::from(kind.as_bytes()[1] - b'0'),
                text.replace('\n', " "),
            ));
        } else if kind == "li" {
            let depth = self.lists.len().saturating_sub(1) as i64;
            self.doc.add(Block::Li(
                depth,
                self.lists.last().copied().unwrap_or(false),
                text.replace('\n', " "),
            ));
        } else {
            self.doc.add(Block::P(text));
        }
    }

    fn data(&mut self, d: &str) {
        if self.skip == 0 {
            match self.tables.last_mut().and_then(|t| t.cell.map(|c| (t, c))) {
                Some((t, c)) => t.cells[c].push(d.to_string()),
                None => self.buf.push(d.to_string()),
            }
        }
    }

    /// Inside a table cell a block element starts a new line of text there (the cell keeps it as a space).
    fn cell_break(&mut self) {
        if self.tables.last().is_some_and(|t| t.cell.is_some()) {
            self.data(" ");
        }
    }

    fn start_tag(&mut self, tag: &str) {
        if self.head && self.skip == 1 && !HEAD_TAGS.contains(&tag) {
            self.head = false; // an implied </head>
            self.skip = 0;
        }
        if tag == "head" && self.skip == 0 {
            self.head = true;
            self.skip += 1;
        } else if SKIP.contains(&tag) {
            self.skip += 1;
        } else if self.skip > 0 {
        } else if tag == "br" {
            self.data("\n");
        } else if self.tables.last().is_some() && matches!(tag, "tr" | "td" | "th") {
            let t = self.tables.last_mut().unwrap();
            if tag == "tr" {
                t.rows.push(vec![]);
                t.row = Some(t.rows.len() - 1);
            } else {
                if t.row.is_none() {
                    t.rows.push(vec![]);
                    t.row = Some(t.rows.len() - 1);
                }
                t.cells.push(vec![]);
                t.cell = Some(t.cells.len() - 1);
                let n = t.cells.len() - 1;
                t.rows[t.row.unwrap()].push(n);
            }
        } else if tag == "table" {
            if self.tables.is_empty() {
                self.emit();
            }
            self.tables.push(Tbl::default());
        } else if tag == "ul" || tag == "ol" {
            self.cell_break();
            self.emit();
            self.lists.push(tag == "ol");
        } else if tag == "li" || tag == "pre" || is_heading(tag) {
            self.cell_break();
            self.emit();
            self.kind = tag.to_string();
            self.pre += usize::from(tag == "pre");
        } else if BLOCK.contains(&tag) {
            self.cell_break();
            self.emit();
        }
    }

    fn end_tag(&mut self, tag: &str) {
        if tag == "head" {
            if self.head {
                self.head = false;
                self.skip = self.skip.saturating_sub(1);
            }
        } else if SKIP.contains(&tag) {
            self.skip = self.skip.saturating_sub(1);
        } else if self.skip > 0 {
        } else if tag == "td" || tag == "th" {
            if let Some(t) = self.tables.last_mut() {
                t.cell = None;
            }
        } else if tag == "table" && !self.tables.is_empty() {
            let t = self.tables.pop().unwrap();
            let rows: Vec<Vec<String>> = t
                .rows
                .iter()
                .map(|r| {
                    r.iter()
                        .map(|&c| strip(&collapse_ws(&t.cells[c].concat())).to_string())
                        .collect::<Vec<_>>()
                })
                .filter(|r| !r.is_empty())
                .collect();
            if self.tables.is_empty() {
                self.doc.add(Block::Table(rows));
            } else {
                // a table inside a cell: its text becomes part of that cell
                let flat: Vec<&str> = rows
                    .iter()
                    .flatten()
                    .map(String::as_str)
                    .filter(|c| !c.is_empty())
                    .collect();
                self.data(&format!(" {} ", flat.join(" ")));
            }
        } else if tag == "ul" || tag == "ol" {
            self.cell_break();
            self.emit();
            self.lists.pop();
        } else if tag == "pre" {
            self.emit();
            self.pre = self.pre.saturating_sub(1);
        } else if tag == "li" || is_heading(tag) || BLOCK.contains(&tag) {
            self.cell_break();
            self.emit();
        }
    }

    /// Everything from `<` at `i`: returns where parsing goes on, None when the construct isn't terminated.
    fn parse_starttag(&mut self, raw: &str, i: usize) -> R<Option<usize>> {
        let Some(endpos) = check_for_whole_start_tag(raw, i)? else {
            return Ok(None);
        };
        let (mut k, name) = tag_find(raw, i + 1).expect("a tag name follows <letter");
        let tag = name.to_lowercase();
        while k < endpos {
            match attr_at(raw, k) {
                Some(e) => k = e,
                None => break,
            }
        }
        let end = strip(raw.get(k..endpos).unwrap_or(""));
        if end != ">" && end != "/>" {
            self.data(&raw[i..endpos]);
            return Ok(Some(endpos));
        }
        if end.ends_with("/>") {
            self.start_tag(&tag);
            self.end_tag(&tag);
        } else {
            self.start_tag(&tag);
            if tag == "script" || tag == "style" {
                self.cdata = Some(if tag == "script" { "script" } else { "style" });
            }
        }
        Ok(Some(endpos))
    }

    fn parse_endtag(&mut self, raw: &str, i: usize) -> R<Option<usize>> {
        let Some(gt) = find_char(raw, '>', i + 1) else {
            return Ok(None);
        };
        let gtpos = gt + 1;
        if let Some((_, name)) = end_tag_find(raw, i) {
            self.end_tag(&name.to_lowercase());
            return Ok(Some(gtpos));
        }
        let Some((name_end, name)) = tag_find(raw, i + 2) else {
            return if raw[i..].starts_with("</>") {
                Ok(Some(i + 3))
            } else {
                Ok(parse_bogus_comment(raw, i))
            };
        };
        let gtpos = find_char(raw, '>', name_end).map_or(0, |p| p + 1);
        self.end_tag(&name.to_lowercase());
        Ok(Some(gtpos))
    }

    fn run(&mut self, raw: &str) -> R<()> {
        let n = raw.len();
        let mut i = 0;
        while i < n {
            self.doc.check()?;
            if let Some(elem) = self.cdata {
                // script/style contents are skipped; without a closing tag the rest is dropped
                let close = if elem == "script" {
                    &SCRIPT_CLOSE
                } else {
                    &STYLE_CLOSE
                };
                match find_from(close, raw, i)? {
                    Some((_, e)) => {
                        self.end_tag(elem);
                        self.cdata = None;
                        i = e;
                        continue;
                    }
                    None => break,
                }
            }
            let j = find_char(raw, '<', i).unwrap_or(n);
            if i < j {
                self.data(&unescape(&raw[i..j]));
            }
            i = j;
            if i == n {
                break;
            }
            let rest = &raw[i..];
            let k = if rest[1..].starts_with(|c: char| c.is_ascii_alphabetic()) {
                self.parse_starttag(raw, i)?
            } else if rest.starts_with("</") {
                self.parse_endtag(raw, i)?
            } else if rest.starts_with("<!--") {
                parse_comment(raw, i)?
            } else if rest.starts_with("<?") {
                find_char(raw, '>', i + 2).map(|p| p + 1)
            } else if rest.starts_with("<!") {
                parse_html_declaration(raw, i)?
            } else if i + 1 < n {
                self.data("<");
                Some(i + 1)
            } else {
                break;
            };
            i = match k {
                Some(k) => k,
                None => {
                    // not terminated, and no more input: like html.parser since its 2025 fix (3.12.13 has
                    // it, 3.12.10 doesn't), an unfinished tag, comment or declaration at the end is dropped; only a bare
                    // "</" stays as text
                    if &raw[i..] == "</" {
                        self.data("</");
                    }
                    n
                }
            };
        }
        if i < n && self.cdata.is_none() {
            self.data(&unescape(&raw[i..n]));
        }
        Ok(())
    }
}

fn check_for_whole_start_tag(raw: &str, i: usize) -> R<Option<usize>> {
    let j = locate(raw, i);
    let Some(next) = raw[j..].chars().next() else {
        return Ok(None);
    };
    Ok(match next {
        '>' => Some(j + 1),
        '/' if raw[j..].starts_with("/>") => Some(j + 2),
        '/' => None,
        c if c.is_ascii_alphabetic() || c == '=' => None,
        _ => Some(if j > i { j } else { i + 1 }),
    })
}

fn parse_comment(raw: &str, i: usize) -> R<Option<usize>> {
    Ok(find_from(&COMMENT_CLOSE, raw, i + 4)?.map(|(_, e)| e))
}

fn parse_bogus_comment(raw: &str, i: usize) -> Option<usize> {
    find_char(raw, '>', i + 2).map(|p| p + 1)
}

fn parse_html_declaration(raw: &str, i: usize) -> R<Option<usize>> {
    let rest = &raw[i..];
    if rest.starts_with("<!--") {
        parse_comment(raw, i)
    } else if rest.starts_with("<![") {
        parse_marked_section(raw, i)
    } else if rest
        .get(..9)
        .is_some_and(|s| s.eq_ignore_ascii_case("<!doctype"))
    {
        Ok(find_char(raw, '>', i + 9).map(|p| p + 1))
    } else {
        Ok(parse_bogus_comment(raw, i))
    }
}

fn parse_marked_section(raw: &str, i: usize) -> R<Option<usize>> {
    if i + 3 == raw.len() {
        return Ok(None);
    }
    let Some(end) = decl_name(raw, i + 3) else {
        return Err(Fail::Other("AssertionError"));
    };
    if end == raw.len() {
        return Ok(None); // end of buffer
    }
    let name = strip(&raw[i + 3..end]).to_lowercase();
    let close = match name.as_str() {
        "temp" | "cdata" | "ignore" | "include" | "rcdata" => &MARKED_CLOSE,
        "if" | "else" | "endif" => &MS_MARKED_CLOSE,
        _ => return Err(Fail::Other("AssertionError")),
    };
    Ok(find_from(close, raw, i + 3)?.map(|(_, e)| e))
}

// ---------- text clean-up ----------

/// Runs of `[ \t\r\f\v]` become one space, then spaces next to a line break go, then strip.
fn collapse(text: &str) -> String {
    let mut one = String::with_capacity(text.len());
    let mut in_run = false;
    for c in text.chars() {
        if matches!(c, ' ' | '\t' | '\r' | '\x0c' | '\x0b') {
            if !in_run {
                one.push(' ');
            }
            in_run = true;
        } else {
            one.push(c);
            in_run = false;
        }
    }
    let lines: Vec<&str> = one.split('\n').collect();
    let last = lines.len() - 1;
    let joined: Vec<&str> = lines
        .iter()
        .enumerate()
        .map(|(n, l)| {
            let l = if n > 0 {
                l.trim_start_matches([' ', '\t'])
            } else {
                l
            };
            if n < last {
                l.trim_end_matches([' ', '\t'])
            } else {
                l
            }
        })
        .collect();
    strip(&joined.join("\n")).to_string()
}

/// `re.sub(r"\s+", " ", s)`
fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_run = false;
    for c in s.chars() {
        if is_space(c) {
            if !in_run {
                out.push(' ');
            }
            in_run = true;
        } else {
            out.push(c);
            in_run = false;
        }
    }
    out
}

/// The page's text: like `decode`, but a page that isn't UTF-8 and names its charset (`<meta charset=...>`)
/// is read in that one.
fn page_text(raw: &[u8]) -> String {
    if std::str::from_utf8(raw).is_err()
        && !raw.starts_with(&[0xff, 0xfe])
        && !raw.starts_with(&[0xfe, 0xff])
    {
        let head = String::from_utf8_lossy(&raw[..raw.len().min(2048)]).to_lowercase();
        let label = head.find("charset").and_then(|p| {
            let rest = head[p + 7..].trim_start().strip_prefix('=')?;
            let rest = rest.trim_start().trim_start_matches(['"', '\'']);
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.')))
                .unwrap_or(rest.len());
            Some(rest[..end].to_string())
        });
        let enc = label
            .and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes()))
            .filter(|e| {
                *e != encoding_rs::UTF_8
                    && *e != encoding_rs::UTF_16LE
                    && *e != encoding_rs::UTF_16BE
            });
        if let Some(e) = enc {
            return e.decode_without_bom_handling(raw).0.into_owned();
        }
    }
    decode(raw)
}

pub fn html_blocks(doc: &mut Doc, raw: &[u8]) -> R<()> {
    let text = page_text(raw);
    let mut p = Parser {
        doc,
        buf: vec![],
        kind: "p".into(),
        skip: 0,
        lists: vec![],
        tables: vec![],
        head: false,
        pre: 0,
        cdata: None,
    };
    p.run(&text)?;
    p.emit();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescape_matches_python() {
        assert_eq!(
            unescape("a &amp; b &lt;c&gt; &nbsp;|&copy 2020|&#65;&#x42;|&bogus;|&amp"),
            "a & b <c> \u{a0}|\u{a9} 2020|AB|&bogus;|&"
        );
        assert_eq!(
            unescape("&#0;&#x80;&#xD800;&#x110000;"),
            "\u{fffd}\u{20ac}\u{fffd}\u{fffd}"
        );
        assert_eq!(unescape("5 & 6 &notit; &ampx"), "5 & 6 \u{ac}it; &x");
    }

    #[test]
    fn whitespace_collapses_around_line_breaks() {
        assert_eq!(collapse("  a \t b\u{a0} \n  c \n"), "a b\u{a0}\nc");
    }
}
