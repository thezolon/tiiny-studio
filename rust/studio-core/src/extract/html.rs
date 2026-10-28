//! HTML into blocks: headings, paragraphs, (nested) list items, tables and preformatted text. Port of
//! extract.py's `HTMLBlocks` on top of a port of Python's `html.parser.HTMLParser` (convert_charrefs=True),
//! because the exact tokenizing quirks decide what comes out for messy pages.

use std::sync::LazyLock;

use fancy_regex::Regex;

use super::entities::HTML5;
use super::render::Block;
use super::text::{decode, is_space, strip};
use super::{Doc, Fail, R};

// ---------- html.unescape ----------

fn named(k: &str) -> Option<&'static str> {
    HTML5.binary_search_by(|(name, _)| (*name).cmp(k)).ok().map(|i| HTML5[i].1)
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
    let n: usize = rest.chars().take(32).take_while(|c| !matches!(c, '\t' | '\n' | '\x0c' | ' ' | '<' | '&' | '#' | ';')).map(char::len_utf8).sum();
    (n > 0).then(|| n + semi(rest, n))
}

fn replace_ref(s: &str) -> String {
    if let Some(r) = s.strip_prefix('#') {
        let (digits, radix) = match r.strip_prefix(['x', 'X']) {
            Some(h) => (h, 16),
            None => (r, 10),
        };
        let num = u64::from_str_radix(digits.trim_end_matches(';'), radix).unwrap_or(u64::MAX);
        return match num {
            0x9b => "\u{203a}".into(),
            0x9c => "\u{153}".into(),
            0x9d => "\u{9d}".into(),
            0x9e => "\u{17e}".into(),
            0x9f => "\u{178}".into(),
            0xd800..=0xdfff | 0x110000.. => "\u{fffd}".into(),
            // the code points HTML5 calls invalid are dropped (NUL is not among them, as in Python)
            0x1..=0x8 | 0xb | 0xe..=0x1f | 0x7f..=0x9f | 0xfdd0..=0xfdef => String::new(),
            n if n & 0xfffe == 0xfffe => String::new(),
            n => char::from_u32(n as u32).map(String::from).unwrap_or_default(),
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

// Each pattern is anchored (\A) and run on a slice that starts where Python's `.match(rawdata, pos)` starts.
static TAGFIND: LazyLock<Regex> = LazyLock::new(|| re(r"\A([a-zA-Z][^\t\n\r\f />\x00]*)(?:\s|/(?!>))*"));
static ATTRFIND: LazyLock<Regex> = LazyLock::new(|| {
    // starts one character early so the look-behind sees what precedes the position
    re(r#"(?s)\A.((?<=['"\s/])[^\s/>][^\s/=>]*)(\s*=+\s*('[^']*'|"[^"]*"|(?!['"])[^>\s]*))?(?:\s|/(?!>))*"#)
});
static LOCATE: LazyLock<Regex> = LazyLock::new(|| {
    re(r#"\A<[a-zA-Z][^\t\n\r\f />\x00]*(?:[\s/]*(?:(?<=['"\s/])[^\s/>][^\s/=>]*(?:\s*=+\s*(?:'[^']*'|"[^"]*"|(?!['"])[^>\s]*)\s*)?(?:\s|/(?!>))*)*)?\s*"#)
});
static ENDTAGFIND: LazyLock<Regex> = LazyLock::new(|| re(r"\A</\s*([a-zA-Z][-.a-zA-Z0-9:_]*)\s*>"));
static DECLNAME: LazyLock<Regex> = LazyLock::new(|| re(r"\A[a-zA-Z][-_.a-zA-Z0-9]*\s*"));
static COMMENT_CLOSE: LazyLock<Regex> = LazyLock::new(|| re(r"--\s*>"));
static MARKED_CLOSE: LazyLock<Regex> = LazyLock::new(|| re(r"\]\s*\]\s*>"));
static MS_MARKED_CLOSE: LazyLock<Regex> = LazyLock::new(|| re(r"\]\s*>"));
static SCRIPT_CLOSE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)</\s*script\s*>"));
static STYLE_CLOSE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)</\s*style\s*>"));

fn bad(_: fancy_regex::Error) -> Fail {
    Fail::Other("error")
}

/// End offset of the anchored match on `raw[at..]`, with the first capture group.
fn anchored<'a>(r: &Regex, raw: &'a str, at: usize) -> R<Option<(usize, &'a str)>> {
    Ok(r.captures(&raw[at..]).map_err(bad)?.map(|c| (at + c.get(0).unwrap().end(), c.get(1).map_or("", |g| g.as_str()))))
}

fn find_from(r: &Regex, raw: &str, at: usize) -> R<Option<(usize, usize)>> {
    Ok(r.find_from_pos(raw, at).map_err(bad)?.map(|m| (m.start(), m.end())))
}

fn find_char(raw: &str, c: char, from: usize) -> Option<usize> {
    raw.get(from..)?.find(c).map(|p| from + p)
}

const BLOCK: [&str; 18] = [
    "p", "div", "section", "article", "blockquote", "header", "footer", "main", "aside", "nav", "figure", "figcaption", "dd", "dt",
    "address", "hr", "title", "body",
];
const SKIP: [&str; 6] = ["script", "style", "head", "noscript", "svg", "template"];

fn is_heading(tag: &str) -> bool {
    tag.len() == 2 && tag.starts_with('h') && matches!(tag.as_bytes()[1], b'1'..=b'6')
}

/// HTMLParser plus the HTMLBlocks handlers.
struct Parser<'d, 'z> {
    doc: &'d mut Doc<'z>,
    buf: Vec<String>,
    kind: String,
    skip: usize,
    lists: Vec<bool>,
    /// table under construction: rows of indexes into `cells`
    table: Option<Vec<Vec<usize>>>,
    cells: Vec<Vec<String>>,
    row: Option<usize>,
    cell: Option<usize>,
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
            self.doc.add(Block::Pre(String::new(), text.trim_matches('\n').to_string()));
        } else if is_heading(&kind) {
            self.doc.add(Block::H(i64::from(kind.as_bytes()[1] - b'0'), text.replace('\n', " ")));
        } else if kind == "li" {
            let depth = self.lists.len().saturating_sub(1) as i64;
            self.doc.add(Block::Li(depth, self.lists.last().copied().unwrap_or(false), text.replace('\n', " ")));
        } else {
            self.doc.add(Block::P(text));
        }
    }

    fn data(&mut self, d: &str) {
        if self.skip == 0 {
            match self.cell {
                Some(c) => self.cells[c].push(d.to_string()),
                None => self.buf.push(d.to_string()),
            }
        }
    }

    fn start_tag(&mut self, tag: &str) {
        if SKIP.contains(&tag) {
            self.skip += 1;
        } else if self.skip > 0 {
        } else if tag == "br" {
            self.data("\n");
        } else if self.table.is_some() && matches!(tag, "tr" | "td" | "th") {
            let table = self.table.as_mut().unwrap();
            if tag == "tr" {
                table.push(vec![]);
                self.row = Some(table.len() - 1);
            } else {
                if self.row.is_none() {
                    table.push(vec![]);
                    self.row = Some(table.len() - 1);
                }
                self.cells.push(vec![]);
                self.cell = Some(self.cells.len() - 1);
                table[self.row.unwrap()].push(self.cells.len() - 1);
            }
        } else if tag == "table" {
            self.emit();
            self.table = Some(vec![]);
            self.cells.clear();
            self.row = None;
            self.cell = None;
        } else if tag == "ul" || tag == "ol" {
            self.emit();
            self.lists.push(tag == "ol");
        } else if tag == "li" || tag == "pre" || is_heading(tag) {
            self.emit();
            self.kind = tag.to_string();
            self.pre += usize::from(tag == "pre");
        } else if BLOCK.contains(&tag) {
            self.emit();
        }
    }

    fn end_tag(&mut self, tag: &str) {
        if SKIP.contains(&tag) {
            self.skip = self.skip.saturating_sub(1);
        } else if self.skip > 0 {
        } else if tag == "td" || tag == "th" {
            self.cell = None;
        } else if tag == "table" && self.table.is_some() {
            let table = self.table.take().unwrap();
            let rows: Vec<Vec<String>> = table
                .iter()
                .map(|r| r.iter().map(|&c| strip(&collapse_ws(&self.cells[c].concat())).to_string()).collect::<Vec<_>>())
                .filter(|r| !r.is_empty())
                .collect();
            self.row = None;
            self.cell = None;
            self.doc.add(Block::Table(rows));
        } else if tag == "ul" || tag == "ol" {
            self.emit();
            self.lists.pop();
        } else if tag == "pre" {
            self.emit();
            self.pre = self.pre.saturating_sub(1);
        } else if tag == "li" || is_heading(tag) || BLOCK.contains(&tag) {
            self.emit();
        }
    }

    /// Everything from `<` at `i`: returns where parsing goes on, None when the construct isn't terminated.
    fn parse_starttag(&mut self, raw: &str, i: usize) -> R<Option<usize>> {
        let Some(endpos) = check_for_whole_start_tag(raw, i)? else { return Ok(None) };
        let (mut k, name) = anchored(&TAGFIND, raw, i + 1)?.expect("a tag name follows <letter");
        let tag = name.to_lowercase();
        while k < endpos {
            let prev = raw[..k].char_indices().next_back().map_or(0, |(p, _)| p);
            match ATTRFIND.find(&raw[prev..]).map_err(bad)? {
                Some(m) => k = prev + m.end(),
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
        let Some(gt) = find_char(raw, '>', i + 1) else { return Ok(None) };
        let gtpos = gt + 1;
        if let Some((_, name)) = anchored(&ENDTAGFIND, raw, i)? {
            self.end_tag(&name.to_lowercase());
            return Ok(Some(gtpos));
        }
        let Some((name_end, name)) = anchored(&TAGFIND, raw, i + 2)? else {
            return if raw[i..].starts_with("</>") { Ok(Some(i + 3)) } else { Ok(parse_bogus_comment(raw, i)) };
        };
        let gtpos = find_char(raw, '>', name_end).map_or(0, |p| p + 1);
        self.end_tag(&name.to_lowercase());
        Ok(Some(gtpos))
    }

    fn run(&mut self, raw: &str) -> R<()> {
        let n = raw.len();
        let mut i = 0;
        while i < n {
            if let Some(elem) = self.cdata {
                // script/style contents are skipped; without a closing tag the rest is dropped
                let close = if elem == "script" { &SCRIPT_CLOSE } else { &STYLE_CLOSE };
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
    let (j, _) = anchored(&LOCATE, raw, i)?.expect("LOCATE matches at <letter");
    let Some(next) = raw[j..].chars().next() else { return Ok(None) };
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
    } else if rest.get(..9).is_some_and(|s| s.eq_ignore_ascii_case("<!doctype")) {
        Ok(find_char(raw, '>', i + 9).map(|p| p + 1))
    } else {
        Ok(parse_bogus_comment(raw, i))
    }
}

fn parse_marked_section(raw: &str, i: usize) -> R<Option<usize>> {
    if i + 3 == raw.len() {
        return Ok(None);
    }
    let Some(m) = DECLNAME.find(&raw[i + 3..]).map_err(bad)? else { return Err(Fail::Other("AssertionError")) };
    if i + 3 + m.end() == raw.len() {
        return Ok(None); // end of buffer
    }
    let name = strip(m.as_str()).to_lowercase();
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
            let l = if n > 0 { l.trim_start_matches([' ', '\t']) } else { l };
            if n < last { l.trim_end_matches([' ', '\t']) } else { l }
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

pub fn html_blocks(doc: &mut Doc, raw: &[u8]) -> R<()> {
    let text = decode(raw);
    let mut p = Parser {
        doc,
        buf: vec![],
        kind: "p".into(),
        skip: 0,
        lists: vec![],
        table: None,
        cells: vec![],
        row: None,
        cell: None,
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
        assert_eq!(unescape("a &amp; b &lt;c&gt; &nbsp;|&copy 2020|&#65;&#x42;|&bogus;|&amp"), "a & b <c> \u{a0}|\u{a9} 2020|AB|&bogus;|&");
        assert_eq!(unescape("&#0;&#x80;&#xD800;&#x110000;"), "\0\u{fffd}\u{fffd}");
        assert_eq!(unescape("5 & 6 &notit; &ampx"), "5 & 6 \u{ac}it; &x");
    }

    #[test]
    fn whitespace_collapses_around_line_breaks() {
        assert_eq!(collapse("  a \t b\u{a0} \n  c \n"), "a b\u{a0}\nc");
    }
}
