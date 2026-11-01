//! CSV the way Python's `csv` module reads it: `Sniffer.sniff` to guess the delimiter and quoting, and the
//! reader's state machine (quotes, doubled quotes, newlines inside fields, a field size limit).

use std::collections::HashMap;

use fancy_regex::Regex;

use super::{Fail, R};

/// (char, how often it was seen), like the Counter items of csv.Sniffer.
type Tally = Vec<(char, usize)>;

const FIELD_LIMIT: usize = 131072;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dialect {
    pub delimiter: char,
    pub quote: char,
    pub doublequote: bool,
    pub skipinitialspace: bool,
}

impl Dialect {
    pub fn excel() -> Self {
        Dialect {
            delimiter: ',',
            quote: '"',
            doublequote: true,
            skipinitialspace: false,
        }
    }

    pub fn excel_tab() -> Self {
        Dialect {
            delimiter: '\t',
            ..Self::excel()
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    StartRecord,
    StartField,
    InField,
    InQuotedField,
    QuoteInQuotedField,
    EatCrnl,
}

/// `csv.reader(io.StringIO(text), dialect)`: lines end at "\n" only; a stray "\r" is handled by the state machine.
pub struct Reader<'a> {
    lines: std::str::SplitInclusive<'a, char>,
    d: Dialect,
    state: State,
    fields: Vec<String>,
    field: String,
    field_len: usize,
}

impl<'a> Reader<'a> {
    pub fn new(text: &'a str, d: Dialect) -> Self {
        Reader {
            lines: text.split_inclusive('\n'),
            d,
            state: State::StartRecord,
            fields: vec![],
            field: String::new(),
            field_len: 0,
        }
    }

    fn save_field(&mut self) {
        self.fields.push(std::mem::take(&mut self.field));
        self.field_len = 0;
    }

    fn add_char(&mut self, c: char) -> R<()> {
        if self.field_len >= FIELD_LIMIT {
            return Err(Fail::Other("Error")); // field larger than field limit
        }
        self.field.push(c);
        self.field_len += 1;
        Ok(())
    }

    /// One character of input (`None` is the end of a line).
    fn process(&mut self, c: Option<char>) -> R<()> {
        let nl = matches!(c, Some('\n' | '\r'));
        let end = |c: Option<char>| {
            if c.is_none() {
                State::StartRecord
            } else {
                State::EatCrnl
            }
        };
        match self.state {
            State::StartRecord => {
                if c.is_none() {
                    return Ok(());
                }
                if nl {
                    self.state = State::EatCrnl;
                    return Ok(());
                }
                self.state = State::StartField;
                self.process(c)
            }
            State::StartField => {
                if nl || c.is_none() {
                    self.save_field();
                    self.state = end(c);
                } else if c == Some(self.d.quote) {
                    self.state = State::InQuotedField;
                } else if c == Some(' ') && self.d.skipinitialspace {
                } else if c == Some(self.d.delimiter) {
                    self.save_field();
                } else {
                    if let Some(ch) = c {
                        self.add_char(ch)?;
                    }
                    self.state = State::InField;
                }
                Ok(())
            }
            State::InField => {
                if nl || c.is_none() {
                    self.save_field();
                    self.state = end(c);
                } else if c == Some(self.d.delimiter) {
                    self.save_field();
                    self.state = State::StartField;
                } else if let Some(ch) = c {
                    self.add_char(ch)?;
                }
                Ok(())
            }
            State::InQuotedField => {
                match c {
                    None => {}
                    Some(q) if q == self.d.quote => {
                        self.state = if self.d.doublequote {
                            State::QuoteInQuotedField
                        } else {
                            State::InField
                        }
                    }
                    Some(ch) => self.add_char(ch)?,
                }
                Ok(())
            }
            State::QuoteInQuotedField => {
                if c == Some(self.d.quote) {
                    self.add_char(self.d.quote)?;
                    self.state = State::InQuotedField;
                } else if c == Some(self.d.delimiter) {
                    self.save_field();
                    self.state = State::StartField;
                } else if nl || c.is_none() {
                    self.save_field();
                    self.state = end(c);
                } else {
                    if let Some(ch) = c {
                        self.add_char(ch)?;
                    }
                    self.state = State::InField;
                }
                Ok(())
            }
            State::EatCrnl => {
                if nl {
                } else if c.is_none() {
                    self.state = State::StartRecord;
                } else {
                    return Err(Fail::Other("Error")); // new-line character seen in unquoted field
                }
                Ok(())
            }
        }
    }

    /// The next row, or None at the end of the text.
    pub fn next_row(&mut self) -> R<Option<Vec<String>>> {
        loop {
            let Some(line) = self.lines.next() else {
                if self.field_len != 0 || self.state == State::InQuotedField {
                    self.save_field();
                    break;
                }
                return Ok(None);
            };
            for c in line.chars() {
                self.process(Some(c))?;
            }
            self.process(None)?;
            if self.state == State::StartRecord {
                break;
            }
        }
        Ok(Some(std::mem::take(&mut self.fields)))
    }
}

// ---------- csv.Sniffer ----------

fn re(p: &str) -> R<Regex> {
    Regex::new(p).map_err(|_| Fail::Other("error"))
}

fn rx<T>(r: Result<T, fancy_regex::Error>) -> R<T> {
    r.map_err(|_| Fail::Other("error"))
}

/// Python's `max(d, key=d.get)` over an insertion-ordered dict: the first key with the highest count.
fn max_key<K: Copy>(d: &[(K, usize)]) -> K {
    let mut best = &d[0];
    for e in d {
        if e.1 > best.1 {
            best = e;
        }
    }
    best.0
}

fn bump<K: PartialEq>(d: &mut Vec<(K, usize)>, k: K) {
    match d.iter_mut().find(|e| e.0 == k) {
        Some(e) => e.1 += 1,
        None => d.push((k, 1)),
    }
}

/// (quote char, double quote?, delimiter, skip initial space?)
fn guess_quote_and_delimiter(
    data: &str,
    delimiters: &str,
) -> R<(Option<char>, bool, Option<char>, bool)> {
    let pats = [
        r#"(?sm)(?P<delim>[^\w\n"'])(?P<space> ?)(?P<quote>["']).*?(?P=quote)(?P=delim)"#,
        r#"(?sm)(?:^|\n)(?P<quote>["']).*?(?P=quote)(?P<delim>[^\w\n"'])(?P<space> ?)"#,
        r#"(?sm)(?P<delim>[^\w\n"'])(?P<space> ?)(?P<quote>["']).*?(?P=quote)(?:$|\n)"#,
        r#"(?sm)(?:^|\n)(?P<quote>["']).*?(?P=quote)(?:$|\n)"#,
    ];
    let mut found: Option<(Regex, Vec<fancy_regex::Captures>)> = None;
    for p in pats {
        let r = re(p)?;
        let caps: Vec<_> = r
            .captures_iter(data)
            .collect::<Result<_, _>>()
            .map_err(|_| Fail::Other("error"))?;
        if !caps.is_empty() {
            found = Some((r, caps));
            break;
        }
    }
    let Some((r, matches)) = found else {
        return Ok((None, false, None, false));
    };
    let has = |n: &str| r.capture_names().flatten().any(|c| c == n);
    let (mut quotes, mut delims, mut spaces): (Tally, Tally, usize) = (vec![], vec![], 0);
    for m in &matches {
        if let Some(q) = m.name("quote").and_then(|g| g.as_str().chars().next()) {
            bump(&mut quotes, q);
        }
        if !has("delim") {
            continue;
        }
        if let Some(d) = m.name("delim").and_then(|g| g.as_str().chars().next()) {
            if delimiters.contains(d) {
                bump(&mut delims, d);
            }
        }
        if has("space") && m.name("space").is_some_and(|g| !g.as_str().is_empty()) {
            spaces += 1;
        }
    }
    let quotechar = max_key(&quotes);
    let (delim, skip) = if delims.is_empty() {
        (None, false)
    } else {
        let d = max_key(&delims);
        let n = delims.iter().find(|e| e.0 == d).unwrap().1;
        (Some(d).filter(|&d| d != '\n'), n == spaces)
    };
    // an extra quote between delimiters means a double-quoted format
    let d = delim
        .map(|d| fancy_regex::escape(&d.to_string()).into_owned())
        .unwrap_or_default();
    let q = quotechar;
    let dq = re(&format!(
        r"(?m)(({d})|^)\W*{q}[^{d}\n]*{q}[^{d}\n]*{q}\W*(({d})|$)"
    ))?;
    let doublequote = rx(dq.is_match(data))?;
    Ok((Some(quotechar), doublequote, delim, skip))
}

fn count_sub(line: &str, sub: &str) -> usize {
    line.matches(sub).count()
}

fn guess_delimiter(data: &str, delimiters: &str) -> (Option<char>, bool) {
    let data: Vec<&str> = data.split('\n').filter(|l| !l.is_empty()).collect();
    let chunk = data.len().min(10);
    let (mut iteration, mut start, mut end) = (0usize, 0usize, chunk);
    let mut freq: Vec<Vec<(usize, i64)>> = vec![vec![]; 127];
    let mut modes: HashMap<char, (i64, i64)> = HashMap::new();
    let mut delims: HashMap<char, (i64, i64)> = HashMap::new();
    let skip_of =
        |d: char| count_sub(data[0], &d.to_string()) == count_sub(data[0], &format!("{d} "));
    while start < data.len() {
        iteration += 1;
        for line in &data[start..end.min(data.len())] {
            for (code, meta) in freq.iter_mut().enumerate() {
                let n = count_sub(line, &(code as u8 as char).to_string());
                match meta.iter_mut().find(|e| e.0 == n) {
                    Some(e) => e.1 += 1,
                    None => meta.push((n, 1)),
                }
            }
        }
        for (code, items) in freq.iter().enumerate() {
            if items.len() == 1 && items[0].0 == 0 {
                continue;
            }
            let ch = code as u8 as char;
            if items.len() > 1 {
                let mut best = 0;
                for (n, e) in items.iter().enumerate() {
                    if e.1 > items[best].1 {
                        best = n;
                    }
                }
                let others: i64 = items
                    .iter()
                    .enumerate()
                    .filter(|(n, _)| *n != best)
                    .map(|(_, e)| e.1)
                    .sum();
                modes.insert(ch, (items[best].0 as i64, items[best].1 - others));
            } else {
                modes.insert(ch, (items[0].0 as i64, items[0].1));
            }
        }
        let total = (chunk * iteration).min(data.len()) as f64;
        let mut consistency = 1.0f64;
        while delims.is_empty() && consistency >= 0.9 {
            for (&k, &v) in &modes {
                if v.0 > 0
                    && v.1 > 0
                    && (v.1 as f64 / total) >= consistency
                    && delimiters.contains(k)
                {
                    delims.insert(k, v);
                }
            }
            consistency -= 0.01;
        }
        if delims.len() == 1 {
            let d = *delims.keys().next().unwrap();
            return (Some(d), skip_of(d));
        }
        start = end;
        end += chunk;
    }
    if delims.is_empty() {
        return (None, false);
    }
    if delims.len() > 1 {
        for d in [',', '\t', ';', ' ', ':'] {
            if delims.contains_key(&d) {
                return (Some(d), skip_of(d));
            }
        }
    }
    let mut items: Vec<((i64, i64), char)> = delims.iter().map(|(&k, &v)| (v, k)).collect();
    items.sort();
    let d = items.last().unwrap().1;
    (Some(d), skip_of(d))
}

/// `csv.Sniffer().sniff(sample, delimiters)`; None where Python raises "Could not determine delimiter".
pub fn sniff(sample: &str, delimiters: &str) -> R<Option<Dialect>> {
    let (quote, doublequote, mut delimiter, mut skip) =
        guess_quote_and_delimiter(sample, delimiters)?;
    if delimiter.is_none() {
        (delimiter, skip) = guess_delimiter(sample, delimiters);
    }
    Ok(delimiter.map(|delimiter| Dialect {
        delimiter,
        quote: quote.unwrap_or('"'),
        doublequote,
        skipinitialspace: skip,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(text: &str, d: Dialect) -> Vec<Vec<String>> {
        let mut r = Reader::new(text, d);
        std::iter::from_fn(|| r.next_row().unwrap()).collect()
    }

    #[test]
    fn reads_quotes_and_embedded_newlines() {
        let got = rows("a,\"b,\"\"c\"\"\",d\n\"x\ny\",2\n\nlast", Dialect::excel());
        assert_eq!(
            got,
            vec![
                vec!["a", "b,\"c\"", "d"],
                vec!["x\ny", "2"],
                vec![],
                vec!["last"]
            ]
        );
    }

    #[test]
    fn sniffs_delimiters() {
        let d = sniff("Name;Qty\nApples;3\nPears;5\n", ",;\t|")
            .unwrap()
            .unwrap();
        assert_eq!(d.delimiter, ';');
        let d = sniff("Name,Qty\nApples,3\n\"Pears, green\",5\n", ",;\t|")
            .unwrap()
            .unwrap();
        assert_eq!((d.delimiter, d.quote), (',', '"'));
    }
}
