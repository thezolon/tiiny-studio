//! JSON files, re-printed the way Python's `json.dumps(json.loads(text), indent=2, ensure_ascii=False)` does:
//! keys in file order (a repeated key keeps its first position and last value), integers of any size,
//! floats as Python's repr, NaN/Infinity accepted.

enum J {
    Null,
    Bool(bool),
    /// the integer's digits, normalised as `str(int(...))`
    Int(String),
    Float(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

struct P<'a> {
    b: &'a [u8],
    s: &'a str,
    i: usize,
}

const MAX_DEPTH: usize = 400;

impl P<'_> {
    fn ws(&mut self) {
        while matches!(self.b.get(self.i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    fn lit(&mut self, word: &str) -> bool {
        let ok = self.b[self.i..].starts_with(word.as_bytes());
        if ok {
            self.i += word.len();
        }
        ok
    }

    fn value(&mut self, depth: usize) -> Option<J> {
        if depth > MAX_DEPTH {
            return None;
        }
        match *self.b.get(self.i)? {
            b'"' => self.string().map(J::Str),
            b'{' => {
                self.i += 1;
                let mut items: Vec<(String, J)> = vec![];
                let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
                self.ws();
                if self.b.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Some(J::Obj(items));
                }
                loop {
                    self.ws();
                    if self.b.get(self.i) != Some(&b'"') {
                        return None;
                    }
                    let key = self.string()?;
                    self.ws();
                    if self.b.get(self.i) != Some(&b':') {
                        return None;
                    }
                    self.i += 1;
                    self.ws();
                    let v = self.value(depth + 1)?;
                    match index.get(&key) {
                        Some(&n) => items[n].1 = v,
                        None => {
                            index.insert(key.clone(), items.len());
                            items.push((key, v));
                        }
                    }
                    self.ws();
                    match self.b.get(self.i)? {
                        b',' => self.i += 1,
                        b'}' => {
                            self.i += 1;
                            return Some(J::Obj(items));
                        }
                        _ => return None,
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut items = vec![];
                self.ws();
                if self.b.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Some(J::Arr(items));
                }
                loop {
                    self.ws();
                    items.push(self.value(depth + 1)?);
                    self.ws();
                    match self.b.get(self.i)? {
                        b',' => self.i += 1,
                        b']' => {
                            self.i += 1;
                            return Some(J::Arr(items));
                        }
                        _ => return None,
                    }
                }
            }
            b'n' if self.lit("null") => Some(J::Null),
            b't' if self.lit("true") => Some(J::Bool(true)),
            b'f' if self.lit("false") => Some(J::Bool(false)),
            b'N' if self.lit("NaN") => Some(J::Float(f64::NAN)),
            b'I' if self.lit("Infinity") => Some(J::Float(f64::INFINITY)),
            b'-' if self.lit("-Infinity") => Some(J::Float(f64::NEG_INFINITY)),
            b'-' | b'0'..=b'9' => self.number(),
            _ => None,
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.i;
        while self.b.get(self.i).is_some_and(u8::is_ascii_digit) {
            self.i += 1;
        }
        self.i - start
    }

    fn number(&mut self) -> Option<J> {
        let start = self.i;
        if self.b[self.i] == b'-' {
            self.i += 1;
        }
        match self.b.get(self.i)? {
            b'0' => self.i += 1,
            b'1'..=b'9' => {
                self.digits();
            }
            _ => return None,
        }
        let int_end = self.i;
        let mut float = false;
        if self.b.get(self.i) == Some(&b'.') && self.b.get(self.i + 1).is_some_and(u8::is_ascii_digit) {
            self.i += 1;
            self.digits();
            float = true;
        }
        if matches!(self.b.get(self.i), Some(b'e' | b'E')) {
            let save = self.i;
            self.i += 1;
            if matches!(self.b.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if self.digits() == 0 {
                self.i = save;
            } else {
                float = true;
            }
        }
        let tok = &self.s[start..self.i];
        if float {
            return tok.parse::<f64>().ok().map(J::Float);
        }
        let int = &self.s[start..int_end];
        if int.trim_start_matches('-').len() > 4300 {
            return None; // Python refuses to turn longer integers into numbers
        }
        Some(J::Int(if int == "-0" { "0".into() } else { int.to_string() }))
    }

    fn hex4(&self, at: usize) -> Option<u32> {
        u32::from_str_radix(self.s.get(at..at + 4)?, 16).ok().filter(|_| self.b[at..at + 4].iter().all(u8::is_ascii_hexdigit))
    }

    fn string(&mut self) -> Option<String> {
        self.i += 1;
        let mut out = String::new();
        loop {
            let rest = &self.s[self.i..];
            let stop = rest.bytes().position(|c| c == b'"' || c == b'\\' || c < 0x20)?;
            out.push_str(&rest[..stop]);
            self.i += stop;
            match self.b[self.i] {
                b'"' => {
                    self.i += 1;
                    return Some(out);
                }
                b'\\' => {
                    let e = *self.b.get(self.i + 1)?;
                    self.i += 2;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let mut c = self.hex4(self.i)?;
                            self.i += 4;
                            if (0xd800..0xdc00).contains(&c) && self.b[self.i..].starts_with(b"\\u") {
                                if let Some(lo) = self.hex4(self.i + 2).filter(|lo| (0xdc00..0xe000).contains(lo)) {
                                    c = 0x10000 + ((c - 0xd800) << 10) + (lo - 0xdc00);
                                    self.i += 6;
                                }
                            }
                            out.push(char::from_u32(c).unwrap_or('\u{fffd}')); // a lone surrogate can't live in a Rust string
                        }
                        _ => return None,
                    }
                }
                _ => return None, // a raw control character inside a string
            }
        }
    }
}

/// Python's `repr(float)`.
fn float_repr(f: f64) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    let sci = format!("{:e}", f.abs());
    let (mant, exp) = sci.split_once('e').unwrap();
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let decpt = exp.parse::<i32>().unwrap() + 1;
    let n = digits.len() as i32;
    let body = if decpt > 16 || decpt <= -4 {
        let e = decpt - 1;
        let m = if n > 1 { format!("{}.{}", &digits[..1], &digits[1..]) } else { digits.clone() };
        format!("{m}e{}{:02}", if e < 0 { '-' } else { '+' }, e.abs())
    } else if decpt <= 0 {
        format!("0.{}{digits}", "0".repeat((-decpt) as usize))
    } else if decpt >= n {
        format!("{digits}{}.0", "0".repeat((decpt - n) as usize))
    } else {
        format!("{}.{}", &digits[..decpt as usize], &digits[decpt as usize..])
    };
    if f.is_sign_negative() { format!("-{body}") } else { body }
}

fn quote(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn dump(j: &J, level: usize, out: &mut String) {
    let pad = |n: usize| "  ".repeat(n);
    match j {
        J::Null => out.push_str("null"),
        J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        J::Int(s) => out.push_str(s),
        J::Float(f) => out.push_str(&float_repr(*f)),
        J::Str(s) => quote(s, out),
        J::Arr(a) if a.is_empty() => out.push_str("[]"),
        J::Obj(o) if o.is_empty() => out.push_str("{}"),
        J::Arr(a) => {
            out.push_str("[\n");
            for (n, v) in a.iter().enumerate() {
                out.push_str(&pad(level + 1));
                dump(v, level + 1, out);
                out.push_str(if n + 1 < a.len() { ",\n" } else { "\n" });
            }
            out.push_str(&pad(level));
            out.push(']');
        }
        J::Obj(o) => {
            out.push_str("{\n");
            for (n, (k, v)) in o.iter().enumerate() {
                out.push_str(&pad(level + 1));
                quote(k, out);
                out.push_str(": ");
                dump(v, level + 1, out);
                out.push_str(if n + 1 < o.len() { ",\n" } else { "\n" });
            }
            out.push_str(&pad(level));
            out.push('}');
        }
    }
}

/// The text re-printed with two-space indents, or None when it isn't JSON (Python's ValueError).
pub fn pretty(text: &str) -> Option<String> {
    let mut p = P { b: text.as_bytes(), s: text, i: 0 };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.i != text.len() {
        return None;
    }
    let mut out = String::new();
    dump(&v, 0, &mut out);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prints_like_python() {
        assert_eq!(pretty(r#"{"b":[1,2.50,{}],"a":"xé\n","b":null}"#).unwrap(), "{\n  \"b\": null,\n  \"a\": \"x\u{e9}\\n\"\n}");
        assert_eq!(pretty("[1e22, 1e-7, 0.1, 100000000000000000000, -0, -0.0]").unwrap(), "[\n  1e+22,\n  1e-07,\n  0.1,\n  100000000000000000000,\n  0,\n  -0.0\n]");
        assert_eq!(pretty("[1e15, 1.5e16, 12345.678]").unwrap(), "[\n  1000000000000000.0,\n  1.5e+16,\n  12345.678\n]");
        assert!(pretty("{\"a\": 1,}").is_none() && pretty("[1] x").is_none() && pretty("tru").is_none());
    }
}
