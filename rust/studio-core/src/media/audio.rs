//! Pure audio helpers (media.py split_for_speech, join_wavs, wav_seconds, to_asr_wav). The WAV reader follows
//! CPython's `wave` module (chunk walk, errors and all) so odd files behave the same as in the Python server.

use std::fmt;

/// What Python raises while reading a WAV; Display is the job error text (`f"{type(e).__name__}: {e}"`).
#[derive(Debug, PartialEq)]
pub enum WavError {
    Error(String),
    Eof,
    Runtime,
    Value(String),
}

impl fmt::Display for WavError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WavError::Error(m) => write!(f, "Error: {m}"),
            WavError::Eof => f.write_str("EOFError: "),
            WavError::Runtime => f.write_str("RuntimeError: "),
            WavError::Value(m) => write!(f, "ValueError: {m}"),
        }
    }
}

fn err(m: &str) -> WavError {
    WavError::Error(m.to_string())
}

/// str.isspace() / the regex \s of a Python str (Rust's White_Space lacks \x1c-\x1f).
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

/// str.strip() with no arguments.
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// Split text into chunks of whole sentences, each at most ~limit characters.
pub fn split_for_speech(text: &str, limit: usize) -> Vec<String> {
    let text_s = strip(text);
    let c: Vec<char> = text_s.chars().collect();
    // re.split(r"(?<=[.!?…。！？])\s+|\n{2,}", …): a sentence end plus its whitespace, or a blank line
    let mut sentences: Vec<String> = vec![];
    let (mut start, mut i) = (0, 0);
    while i < c.len() {
        let mut end = 0;
        if i > 0 && ".!?…。！？".contains(c[i - 1]) {
            let mut j = i;
            while j < c.len() && is_space(c[j]) {
                j += 1;
            }
            if j > i {
                end = j;
            }
        }
        if end == 0 {
            let mut j = i;
            while j < c.len() && c[j] == '\n' {
                j += 1;
            }
            if j - i >= 2 {
                end = j;
            }
        }
        if end > 0 {
            sentences.push(c[start..i].iter().collect());
            start = end;
            i = end;
        } else {
            i += 1;
        }
    }
    sentences.push(c[start..].iter().collect());
    let (mut chunks, mut cur): (Vec<String>, String) = (vec![], String::new());
    for s in sentences.into_iter().filter(|s| !strip(s).is_empty()) {
        let mut s: Vec<char> = s.chars().collect();
        while s.len() > limit {
            // one very long sentence: split at the last comma/space before the limit
            let rfind = |pat: &str| -> i64 {
                let p: Vec<char> = pat.chars().collect();
                (0..=limit.saturating_sub(p.len())).rev().find(|&k| s[k..k + p.len()] == p[..]).map_or(-1, |k| k as i64)
            };
            let cut = rfind(", ").max(rfind(" "));
            let cut = if cut > (limit / 3) as i64 { cut as usize } else { limit };
            let head: String = s[..cut + 1].iter().collect();
            let rest: String = s[cut + 1..].iter().collect();
            if !cur.is_empty() {
                chunks.push(std::mem::take(&mut cur));
            }
            chunks.push(strip(&head).to_string());
            s = strip(&rest).chars().collect();
        }
        let s: String = s.into_iter().collect();
        if !cur.is_empty() && cur.chars().count() + 1 + s.chars().count() > limit {
            chunks.push(std::mem::replace(&mut cur, s));
        } else {
            cur = strip(&format!("{cur} {s}")).to_string();
        }
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    if chunks.is_empty() {
        chunks.push(text.to_string());
    }
    chunks
}

struct Wav {
    channels: usize,
    sampwidth: usize,
    rate: u32,
    nframes: u64, // as declared by the data chunk
    data: Vec<u8>, // what readframes(nframes) returns: shorter when the file is cut off
}

fn le16(b: &[u8]) -> u64 {
    u16::from_le_bytes([b[0], b[1]]) as u64
}

fn le32(b: &[u8]) -> u64 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as u64
}

/// wave.open(BytesIO(audio)): RIFF/WAVE, then chunks until "data" (which must follow "fmt ").
fn parse(buf: &[u8]) -> Result<Wav, WavError> {
    if buf.len() < 8 {
        return Err(WavError::Eof);
    }
    if &buf[..4] != b"RIFF" {
        return Err(err("file does not start with RIFF id"));
    }
    let declared = le32(&buf[4..8]); // chunks inside can't read past the RIFF chunk's own size
    let body = &buf[8..8 + declared.min((buf.len() - 8) as u64) as usize];
    if body.len() < 4 || &body[..4] != b"WAVE" {
        return Err(err("not a WAVE file"));
    }
    let mut pos: u64 = 4;
    let mut fmt: Option<(usize, usize, u32)> = None;
    while pos + 8 <= body.len() as u64 {
        let p = pos as usize;
        let sz = le32(&body[p + 4..p + 8]);
        let chunk = &body[p + 8..p + 8 + sz.min((body.len() - p - 8) as u64) as usize];
        match &body[p..p + 4] {
            b"fmt " => fmt = Some(read_fmt(chunk)?),
            b"data" => {
                let (channels, sampwidth, rate) = fmt.ok_or_else(|| err("data chunk before fmt chunk"))?;
                let framesize = (channels * sampwidth) as u64;
                let nframes = sz / framesize;
                let want = (nframes * framesize).min(chunk.len() as u64) as usize;
                return Ok(Wav { channels, sampwidth, rate, nframes, data: chunk[..want].to_vec() });
            }
            _ => {}
        }
        pos += 8 + sz + (sz & 1);
        if pos > declared {
            return Err(WavError::Runtime); // chunk.skip() seeking past the end
        }
    }
    Err(err("fmt chunk and/or data chunk missing"))
}

/// (channels, sample width in bytes, frame rate) from a "fmt " chunk.
fn read_fmt(c: &[u8]) -> Result<(usize, usize, u32), WavError> {
    if c.len() < 14 {
        return Err(WavError::Eof);
    }
    let (tag, channels, rate) = (le16(c), le16(&c[2..]) as usize, le32(&c[4..]) as u32);
    if tag != 1 && tag != 0xFFFE {
        return Err(WavError::Error(format!("unknown format: {tag}")));
    }
    if c.len() < 16 {
        return Err(WavError::Eof);
    }
    let bits = le16(&c[14..]) as usize;
    if tag == 0xFFFE {
        if c.len() < 40 {
            return Err(WavError::Eof);
        }
        let sub = &c[24..40];
        if sub != [1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71] {
            // uuid.UUID(bytes_le=sub)
            let (a, b, d) = (le32(sub), le16(&sub[4..]), le16(&sub[6..]));
            let tail: String = sub[8..].iter().map(|x| format!("{x:02x}")).collect();
            return Err(WavError::Error(format!("unknown extended format: {a:08x}-{b:04x}-{d:04x}-{}-{}", &tail[..4], &tail[4..])));
        }
    }
    let sampwidth = bits.div_ceil(8);
    if sampwidth == 0 {
        return Err(err("bad sample width"));
    }
    if channels == 0 {
        return Err(err("bad # of channels"));
    }
    Ok((channels, sampwidth, rate))
}

/// wave.open(..., "wb") with these parameters and the frames written: the canonical 44-byte header.
fn write_wav(channels: usize, sampwidth: usize, rate: u32, frames: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(44 + frames.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + frames.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(channels as u16).to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * (channels * sampwidth) as u32).to_le_bytes());
    out.extend_from_slice(&((channels * sampwidth) as u16).to_le_bytes());
    out.extend_from_slice(&((sampwidth * 8) as u16).to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(frames.len() as u32).to_le_bytes());
    out.extend_from_slice(frames);
    out
}

/// Concatenate WAVs with the same format, with a short pause between parts.
pub fn join_wavs(parts: &[Vec<u8>]) -> Result<Vec<u8>, WavError> {
    const GAP: f64 = 0.25;
    if parts.len() == 1 {
        return Ok(parts[0].clone());
    }
    let mut first: Option<(usize, usize, u32)> = None;
    let mut frames = vec![];
    for p in parts {
        let w = parse(p)?;
        first.get_or_insert((w.channels, w.sampwidth, w.rate));
        frames.push(w.data);
    }
    let (channels, sampwidth, rate) = first.ok_or_else(|| WavError::Value("no parts".into()))?;
    let silence = vec![0u8; (rate as f64 * GAP) as usize * sampwidth * channels];
    if sampwidth > 4 {
        return Err(err("bad sample width"));
    }
    if rate == 0 {
        return Err(err("bad frame rate"));
    }
    let joined = frames.join(&silence[..]);
    Ok(write_wav(channels, sampwidth, rate, &joined))
}

/// Length in seconds, to 2 decimals; None when it isn't a readable WAV.
pub fn wav_seconds(data: &[u8]) -> Result<Option<f64>, WavError> {
    match parse(data) {
        Ok(w) if w.rate != 0 => Ok(format!("{:.2}", w.nframes as f64 / w.rate as f64).parse().ok()),
        Ok(_) | Err(WavError::Error(_) | WavError::Eof) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Downmix/resample WAV input to 16-bit 16 kHz mono; None when the input isn't a WAV (callers pass it through).
pub fn to_asr_wav(audio: &[u8]) -> Result<Option<Vec<u8>>, WavError> {
    let w = match parse(audio) {
        Ok(w) => w,
        Err(WavError::Error(_) | WavError::Eof) => return Ok(None),
        Err(e) => return Err(e),
    };
    let (channels, sw, rate) = (w.channels, w.sampwidth, w.rate);
    if channels == 1 && rate == 16000 && sw == 2 {
        return Ok(Some(audio.to_vec()));
    }
    let raw = &w.data;
    let not_multiple = || WavError::Value("bytes length not a multiple of item size".into());
    let mut a: Vec<i32> = match sw {
        2 if raw.len() % 2 != 0 => return Err(not_multiple()),
        2 => raw.chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as i32).collect(),
        4 if raw.len() % 4 != 0 => return Err(not_multiple()),
        4 => raw.chunks(4).map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]) >> 16).collect(),
        1 => raw.iter().map(|&b| (b as i32 - 128) << 8).collect(),
        3 => (0..raw.len().saturating_sub(2)).step_by(3).map(|i| i16::from_le_bytes([raw[i + 1], raw[i + 2]]) as i32).collect(),
        _ => return Ok(Some(audio.to_vec())),
    };
    if channels > 1 {
        let n = a.len() / channels;
        a = (0..n).map(|i| a[i * channels..(i + 1) * channels].iter().sum::<i32>().div_euclid(channels as i32)).collect();
    }
    if rate != 16000 {
        let step = rate as f64 / 16000.0;
        if step.fract() == 0.0 {
            // 32/48 kHz: average each group of samples (a simple low-pass)
            let k = step as usize;
            a = if k == 0 { vec![] } else { a.chunks_exact(k).map(|g| g.iter().sum::<i32>().div_euclid(k as i32)).collect() };
        } else {
            // linear interpolation
            let n = (a.len() as f64 / step) as usize;
            let last = a.len() as i64 - 1;
            a = (0..n)
                .map(|i| {
                    let x = i as f64 * step;
                    let j = x as i64;
                    let (lo, hi) = (a[j as usize] as f64, a[(j + 1).min(last) as usize] as f64);
                    (lo + (hi - lo) * (x - j as f64)) as i32
                })
                .collect();
        }
    }
    let frames: Vec<u8> = a.iter().flat_map(|v| (*v as i16).to_le_bytes()).collect();
    Ok(Some(write_wav(1, 2, 16000, &frames)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A canonical WAV whose sample bytes are (i*7919*seed) % 251; the expected values below come from running
    /// media.py's functions on the same input.
    fn wav(ch: usize, sw: usize, rate: u32, n: usize, seed: usize) -> Vec<u8> {
        let data: Vec<u8> = (0..n * ch * sw).map(|i| ((i * 7919 * seed) % 251) as u8).collect();
        write_wav(ch, sw, rate, &data)
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn splits_like_python() {
        let s = |t: &str, l: usize| split_for_speech(t, l);
        assert_eq!(s("Hello world. How are you? Fine!", 320), ["Hello world. How are you? Fine!"]);
        assert_eq!(s("One. Two! Three? Four… Five。 Six！ Seven？ Eight.", 20), ["One. Two! Three?", "Four… Five。 Six！", "Seven？ Eight."]);
        assert_eq!(
            s("A fairly long sentence, with several commas, that must be cut somewhere, around the limit value.", 30),
            ["A fairly long sentence, with", "several commas, that must be", "cut somewhere, around the", "limit value."]
        );
        // no comma or space to cut at: a piece of limit + 1 characters (Python's s[:cut + 1])
        assert_eq!(s(&"x".repeat(50), 20), ["x".repeat(21), "x".repeat(21), "x".repeat(8)]);
        assert_eq!(s("para one\n\npara two\n\n\npara three. And more.\nsame line", 15), ["para one", "para two", "para three.", "And more.", "same line"]);
        assert_eq!(s(" a. \n\n b ", 320), ["a. b"]);
        assert_eq!(s("   ", 320), ["   "]);
        assert_eq!(s("", 320), [""]);
        let long = format!("Short. {}end. Tail.", "word ".repeat(30));
        assert_eq!(
            s(&long, 40),
            ["Short.", "word word word word word word word word", "word word word word word word word word", "word word word word word word word word", "word word word word word word end. Tail."]
        );
        assert_eq!(s("ab, cd ef gh ij kl mn op qr st uv wx yz", 12), ["ab, cd ef", "gh ij kl mn", "op qr st uv", "wx yz"]);
        assert_eq!(s("Ünï çödé. Naïve café. 日本語。English again.", 16), ["Ünï çödé.", "Naïve café.", "日本語。English", "again."]);
    }

    #[test]
    fn asr_conversion_like_python() {
        let asr = |ch, sw, rate, n| hex(&to_asr_wav(&wav(ch, sw, rate, n, 1)).unwrap().unwrap()[44..]);
        assert_eq!(asr(1, 2, 16000, 6), "008a19a332bc4bd564ee7d0c"); // already 16 kHz mono: passed through
        assert_eq!(asr(2, 2, 16000, 5), "8c96bec870fd22325464");
        assert_eq!(asr(1, 2, 48000, 9), "19a30ef0af3e");
        assert_eq!(asr(1, 2, 22050, 12), "008a97ac2dcf70f25919f03b865e84db");
        assert_eq!(asr(1, 1, 16000, 5), "0080000a0099002300b2");
        assert_eq!(asr(2, 3, 24000, 6), "deea0f1e44ceb502");
        assert_eq!(asr(1, 4, 32000, 6), "32bc96257c09");
        assert_eq!(asr(2, 2, 8000, 4), "8c96a5afbec817e370fdc91722322232");
        assert_eq!(asr(3, 2, 16000, 4), "19a30ef0af3efbde");
        let out = to_asr_wav(&wav(2, 2, 8000, 4, 1)).unwrap().unwrap();
        assert_eq!(hex(&out[..44]), "524946463400000057415645666d74201000000001000100803e0000007d0000020010006461746110000000");
        // not a WAV: callers send it unchanged
        assert_eq!(to_asr_wav(b"not a wav at all"), Ok(None));
        assert_eq!(to_asr_wav(b""), Ok(None));
        // a cut-off file: odd byte counts fail where Python's array() does, 8-bit survives
        let mut cut = wav(2, 2, 8000, 4, 1);
        cut.truncate(cut.len() - 3);
        assert_eq!(to_asr_wav(&cut), Err(WavError::Value("bytes length not a multiple of item size".into())));
        let mut cut = wav(2, 1, 8000, 4, 1);
        cut.truncate(cut.len() - 3);
        assert_eq!(hex(&to_asr_wav(&cut).unwrap().unwrap()[44..]), "00c580d100de00de");
    }

    #[test]
    fn joins_like_python() {
        let j = join_wavs(&[wav(1, 2, 16000, 3, 1), wav(1, 2, 16000, 2, 3)]).unwrap();
        assert_eq!(j.len(), 8054);
        assert_eq!(hex(&j[..44]), "524946466e1f000057415645666d74201000000001000100803e0000007d000002001000646174614a1f0000");
        assert_eq!((hex(&j[44..50]), hex(&j[j.len() - 4..])), ("008a19a332bc".to_string(), "00a34bee".to_string()));
        assert!(j[50..j.len() - 4].iter().all(|b| *b == 0));
        let j = join_wavs(&[wav(2, 1, 8000, 2, 1), wav(2, 1, 8000, 1, 2), wav(2, 1, 8000, 1, 4)]).unwrap();
        assert_eq!(j.len(), 8052);
        assert_eq!(hex(&j[..44]), "524946466c1f000057415645666d74201000000001000200401f0000803e00000200080064617461481f0000");
        assert_eq!((hex(&j[44..48]), hex(&j[j.len() - 4..])), ("008a19a3".to_string(), "00000032".to_string()));
        let one = wav(1, 2, 16000, 3, 1);
        assert_eq!(join_wavs(std::slice::from_ref(&one)).unwrap(), one);
    }

    #[test]
    fn seconds_like_python() {
        let s = |ch, rate, n| wav_seconds(&wav(ch, 2, rate, n, 1)).unwrap();
        assert_eq!(s(1, 16000, 8000), Some(0.5));
        assert_eq!(s(1, 8000, 1000), Some(0.12)); // 0.125: ties go to even, as Python's round()
        assert_eq!(s(1, 16000, 3), Some(0.0));
        assert_eq!(s(1, 3, 1), Some(0.33));
        assert_eq!(s(1, 24000, 36000), Some(1.5));
        assert_eq!(s(1, 0, 5), None); // ZeroDivisionError
        assert_eq!(wav_seconds(b"junk"), Ok(None));
        // RIFF size smaller than the file: the data chunk is never reached
        let mut short = wav(1, 2, 8000, 4, 1);
        short[4..8].copy_from_slice(&30u32.to_le_bytes());
        assert_eq!(wav_seconds(&short), Ok(None));
    }
}
