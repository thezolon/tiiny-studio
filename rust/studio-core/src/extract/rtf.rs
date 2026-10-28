//! RTF to plain text: paragraphs, tabs, \'hh and \uN characters; skips font tables, pictures and other
//! destinations. Port of extract.py's `rtf_text`, token for token (same regex alternatives, same order).

use super::text::{cp1252, strip};

/// Output text; UTF-16 surrogates from `\uN` are paired up (Python keeps them as separate str items).
#[derive(Default)]
struct Out {
    s: String,
    high: Option<u16>,
}

impl Out {
    fn flush_high(&mut self) {
        if self.high.take().is_some() {
            self.s.push('\u{fffd}');
        }
    }

    fn push(&mut self, c: char) {
        self.flush_high();
        self.s.push(c);
    }

    fn unit(&mut self, u: u16) {
        match (self.high, u) {
            (Some(h), 0xdc00..=0xdfff) => {
                self.high = None;
                self.s.push(char::from_u32(0x10000 + (((h as u32) - 0xd800) << 10) + (u as u32 - 0xdc00)).unwrap_or('\u{fffd}'));
            }
            (_, 0xd800..=0xdbff) => {
                self.flush_high();
                self.high = Some(u);
            }
            (_, 0xdc00..=0xdfff) => {
                self.flush_high();
                self.s.push('\u{fffd}');
            }
            _ => self.push(char::from_u32(u as u32).unwrap_or('\u{fffd}')),
        }
    }
}

/// One byte in the document's code page (what `bytes([b]).decode(codepage)` gives; cp1252 when that fails).
fn decode_byte(codepage: &str, b: u8) -> char {
    if b < 0x80 || codepage == "cp1252" {
        return cp1252(b);
    }
    let n = codepage.trim_start_matches("cp");
    let label = match n {
        "932" => "ms932".to_string(),
        "950" => "big5".to_string(),
        "874" => "windows-874".to_string(),
        _ => codepage.to_string(),
    };
    let one = encoding_rs::Encoding::for_label(label.as_bytes())
        .filter(|e| *e != encoding_rs::UTF_8 && *e != encoding_rs::UTF_16LE && *e != encoding_rs::UTF_16BE)
        .and_then(|e| e.decode_without_bom_handling_and_without_replacement(&[b]).map(|c| c.into_owned()));
    if let Some(s) = one {
        let mut it = s.chars();
        if let (Some(c), None) = (it.next(), it.next()) {
            return c;
        }
    }
    cp1252(b)
}

const DESTINATIONS: [&str; 14] = [
    "fonttbl", "colortbl", "stylesheet", "info", "pict", "header", "footer", "object", "themedata", "datastore", "listtable",
    "listoverridetable", "rsidtbl", "generator",
];

pub fn rtf_text(raw: &[u8]) -> String {
    let b = raw; // latin-1: one byte, one character
    let n = b.len();
    let (mut out, mut stack) = (Out::default(), Vec::<(bool, i64)>::new());
    let (mut skip, mut uc, mut pending_skip) = (false, 1i64, 0i64);
    let mut codepage = "cp1252".to_string();
    let hex = |i: usize| b.get(i).is_some_and(u8::is_ascii_hexdigit);
    let mut i = 0;
    while i < n {
        let c = b[i];
        if c == b'\\' {
            let q = i + 1;
            let letters = b[q.min(n)..].iter().take(32).take_while(|c| c.is_ascii_alphabetic()).count();
            if letters > 0 {
                // \word, optional number, one optional trailing space
                let word = String::from_utf8_lossy(&b[q..q + letters]).to_lowercase();
                let mut j = q + letters;
                let a = j + (b.get(j) == Some(&b'-')) as usize;
                let digits = b[a.min(n)..].iter().take(10).take_while(|c| c.is_ascii_digit()).count();
                let arg: Option<&str> = (digits > 0).then(|| {
                    let s = std::str::from_utf8(&b[j..a + digits]).unwrap();
                    j = a + digits;
                    s
                });
                if b.get(j) == Some(&b' ') {
                    j += 1;
                }
                i = j;
                let num = || arg.and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
                if DESTINATIONS.contains(&word.as_str()) {
                    skip = true;
                } else if word == "ansicpg" && arg.is_some() {
                    codepage = format!("cp{}", arg.unwrap());
                } else if word == "uc" && arg.is_some() {
                    uc = num();
                } else if skip {
                } else if matches!(word.as_str(), "par" | "line" | "sect" | "page" | "row") {
                    out.push('\n');
                } else if matches!(word.as_str(), "tab" | "cell") {
                    out.push('\t');
                } else if word == "u" && arg.is_some() {
                    out.unit(num().rem_euclid(65536) as u16);
                    pending_skip = uc;
                }
            } else if b.get(q) == Some(&b'\'') && hex(q + 1) && hex(q + 2) {
                let byte = u8::from_str_radix(std::str::from_utf8(&b[q + 1..q + 3]).unwrap(), 16).unwrap();
                i = q + 3;
                if pending_skip != 0 {
                    pending_skip -= 1;
                } else if !skip {
                    out.push(decode_byte(&codepage, byte));
                }
            } else if q < n {
                // \X for any one other character
                let sym = b[q];
                i = q + 1;
                if sym == b'*' {
                    skip = true;
                } else if !skip && matches!(sym, b'\\' | b'{' | b'}') {
                    out.push(sym as char);
                } else if !skip && sym == b'~' {
                    out.push('\u{a0}');
                }
            } else {
                i += 1; // a lone backslash at the very end matches nothing
            }
        } else if c == b'{' {
            stack.push((skip, uc));
            i += 1;
        } else if c == b'}' {
            (skip, uc) = stack.pop().unwrap_or((false, 1));
            i += 1;
        } else if c == b'\r' || c == b'\n' {
            while i < n && (b[i] == b'\r' || b[i] == b'\n') {
                i += 1;
            }
        } else {
            let end = b[i..].iter().position(|c| matches!(c, b'\\' | b'{' | b'}' | b'\r' | b'\n')).map_or(n, |p| i + p);
            let mut plain = &b[i..end];
            i = end;
            if !skip {
                if pending_skip != 0 {
                    // Python slices plain[drop:] with drop = min(pending_skip, len); a negative drop counts from the end
                    let len = plain.len() as i64;
                    let drop = pending_skip.min(len);
                    plain = &plain[(if drop < 0 { (len + drop).max(0) } else { drop }) as usize..];
                    pending_skip -= drop;
                }
                for &ch in plain {
                    out.push(ch as char);
                }
            }
        }
    }
    out.flush_high();
    strip(&collapse_newlines(&out.s)).to_string()
}

/// `re.sub(r"\n{3,}", "\n\n", s)`
fn collapse_newlines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut run = 0;
    for c in s.chars() {
        if c == '\n' {
            run += 1;
            if run <= 2 {
                out.push(c);
            }
        } else {
            run = 0;
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_hex_unicode_and_paragraphs() {
        let rtf = br"{\rtf1\ansi\ansicpg1252{\fonttbl{\f0 Arial;}}\f0 Caf\'e9 costs 4\u8364? today.\par\tab Next\~line}";
        assert_eq!(rtf_text(rtf), "Caf\u{e9} costs 4\u{20ac} today.\n\tNext\u{a0}line");
    }

    #[test]
    fn skips_destinations_and_pairs_surrogates() {
        let rtf = br"{\rtf1{\*\generator Foo;}{\colortbl;\red0;}Hi \u-10179?\u-8704? there}";
        assert_eq!(rtf_text(rtf), "Hi \u{1f600} there");
    }

    #[test]
    fn collapses_blank_lines() {
        assert_eq!(rtf_text(br"{\rtf1 a\par\par\par\par b}"), "a\n\nb");
    }
}
