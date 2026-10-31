//! The block model (headings, paragraphs, list items, tables, preformatted text) and its two renderings:
//! plain text and Markdown. Port of extract.py's `md_escape`/`md_lines`/`md_table`/`render`.

use std::collections::BTreeMap;

use super::text::{is_space, strip};

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// heading level, text
    H(i64, String),
    P(String),
    /// depth, numbered?, text
    Li(i64, bool, String),
    Table(Vec<Vec<String>>),
    /// language, text
    Pre(String, String),
}

/// Escape what would otherwise turn plain text into Markdown syntax.
pub fn md_escape(s: &str) -> String {
    let mut a = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if matches!(c, '\\' | '`' | '*' | '[' | ']' | '<') {
            a.push('\\');
        }
        a.push(c);
    }
    // a line that starts like a heading, quote, bullet or "1." item (after any whitespace)
    let ch: Vec<char> = a.chars().collect();
    let n = ch.len();
    let mut out = String::with_capacity(a.len() + 8);
    let mut i = 0;
    while i < n {
        if i == 0 || ch[i - 1] == '\n' {
            let mut e = i;
            while e < n && is_space(ch[e]) {
                e += 1;
            }
            let end = if e >= n {
                None
            } else if matches!(ch[e], '#' | '>' | '+' | '-') {
                Some(e + 1)
            } else if ch[e].is_ascii_digit() {
                let mut d = e;
                while d < n && ch[d].is_ascii_digit() {
                    d += 1;
                }
                (d < n && matches!(ch[d], '.' | ')')).then_some(d + 1)
            } else {
                None
            };
            if let Some(end) = end.filter(|&end| end == n || is_space(ch[end])) {
                out.extend(&ch[i..e]);
                if ch[e].is_ascii_digit() {
                    out.extend(&ch[e..end - 1]);
                    out.push('\\');
                    out.push(ch[end - 1]);
                } else {
                    out.push('\\');
                    out.push(ch[e]);
                }
                i = end;
                continue;
            }
        }
        out.push(ch[i]);
        i += 1;
    }
    out
}

/// Line breaks inside a paragraph become Markdown hard breaks.
pub fn md_lines(s: &str) -> String {
    s.split('\n')
        .filter(|l| !strip(l).is_empty())
        .map(|l| md_escape(strip(l)))
        .collect::<Vec<_>>()
        .join("  \n")
}

pub fn md_table(rows: &[Vec<String>]) -> String {
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    let cell = |c: &str| {
        let t = md_escape(c).replace('|', "\\|").replace('\n', "<br>");
        if strip(&t).is_empty() {
            " ".to_string()
        } else {
            strip(&t).to_string()
        }
    };
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            let mut v: Vec<String> = r.iter().map(|c| cell(c)).collect();
            v.resize(width, " ".to_string());
            v
        })
        .collect();
    let mut lines = vec![
        format!("| {} |", rows[0].join(" | ")),
        format!("|{}", " --- |".repeat(width)),
    ];
    lines.extend(rows[1..].iter().map(|r| format!("| {} |", r.join(" | "))));
    lines.join("\n")
}

/// Blocks to (plain text, Markdown).
pub fn render(blocks: &[Block]) -> (String, String) {
    let (mut text, mut md): (Vec<String>, Vec<String>) = (vec![], vec![]);
    let mut counters: BTreeMap<i64, i64> = BTreeMap::new();
    let (mut prev_li, mut prev_ordered): (bool, Option<bool>) = (false, None);
    let nl = |md: &Vec<String>| if md.is_empty() { "" } else { "\n" };
    for b in blocks {
        if !matches!(b, Block::Li(..)) {
            counters.clear();
        }
        match b {
            Block::H(level, t) => {
                text.push(t.clone());
                md.push(format!(
                    "{}{} {}",
                    nl(&md),
                    "#".repeat((*level).max(0) as usize),
                    md_escape(&t.replace('\n', " "))
                ));
            }
            Block::P(t) => {
                text.push(t.clone());
                md.push(format!("{}{}", nl(&md), md_lines(t)));
            }
            Block::Li(depth, ordered, t) => {
                counters.retain(|d, _| d <= depth);
                let n = counters.entry(*depth).or_insert(0);
                *n += 1;
                let n = *n;
                let pad = (*depth).max(0) as usize;
                let flat = t.replace('\n', " ");
                text.push(format!(
                    "{}{}{flat}",
                    "  ".repeat(pad),
                    if *ordered {
                        format!("{n}. ")
                    } else {
                        "• ".into()
                    }
                ));
                let new_list = !prev_li || (*depth == 0 && Some(*ordered) != prev_ordered); // a bullet list then a numbered one
                let lead = if !md.is_empty() && new_list { "\n" } else { "" };
                md.push(format!(
                    "{lead}{}{}{}",
                    "   ".repeat(pad),
                    if *ordered {
                        format!("{n}. ")
                    } else {
                        "- ".into()
                    },
                    md_escape(&flat)
                ));
                if *depth == 0 {
                    prev_ordered = Some(*ordered);
                }
            }
            Block::Table(rows) => {
                let rows: Vec<Vec<String>> = rows
                    .iter()
                    .filter(|r| r.iter().any(|c| !strip(c).is_empty()))
                    .cloned()
                    .collect();
                if !rows.is_empty() {
                    text.push(
                        rows.iter()
                            .map(|r| {
                                r.iter()
                                    .map(|c| c.replace('\n', " "))
                                    .collect::<Vec<_>>()
                                    .join("\t")
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                    md.push(format!("{}{}", nl(&md), md_table(&rows)));
                }
            }
            Block::Pre(lang, t) => {
                text.push(t.clone());
                md.push(format!("{}```{lang}\n{t}\n```", nl(&md)));
            }
        }
        prev_li = matches!(b, Block::Li(..));
    }
    (
        strip(&text.join("\n")).to_string(),
        strip(&md.join("\n")).to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn escapes_inline_and_line_start_syntax() {
        assert_eq!(md_escape("a *b* [c] <d> `e`"), r"a \*b\* \[c\] \<d> \`e\`");
        assert_eq!(md_escape("# not a heading"), "\\# not a heading");
        assert_eq!(
            md_escape("1. not a list\n  - nor this\n2) or this"),
            "1\\. not a list\n  \\- nor this\n2\\) or this"
        );
        assert_eq!(md_escape("-5 and #tag"), "-5 and #tag");
    }

    #[test]
    fn tables_pad_rows_and_escape_pipes() {
        let md = md_table(&[s(&["A", "B"]), s(&["x | y"])]);
        assert_eq!(md, "| A | B |\n| --- | --- |\n| x \\| y |   |");
    }

    #[test]
    fn list_numbering_restarts_and_nests() {
        let blocks = vec![
            Block::Li(0, true, "one".into()),
            Block::Li(1, true, "inner".into()),
            Block::Li(0, true, "two".into()),
            Block::P("break".into()),
            Block::Li(0, true, "again".into()),
            Block::Li(0, false, "bullet".into()),
        ];
        let (text, md) = render(&blocks);
        assert_eq!(
            text,
            "1. one\n  1. inner\n2. two\nbreak\n1. again\n• bullet"
        );
        assert_eq!(
            md,
            "1. one\n   1. inner\n2. two\n\nbreak\n\n1. again\n\n- bullet"
        );
    }

    #[test]
    fn headings_and_code() {
        let (text, md) = render(&[
            Block::H(2, "Title".into()),
            Block::Pre("json".into(), "{}".into()),
        ]);
        assert_eq!(text, "Title\n{}");
        assert_eq!(md, "## Title\n\n```json\n{}\n```");
    }
}
