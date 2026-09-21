use std::collections::HashMap;

use crate::glyph::{GlyphPath, PathCache};
use crate::tables::STANDARD_ENCODING;

const DEFAULT_LEN_IV: i64 = 4;
const EEXEC_KEY: u16 = 55665;
const CHARSTRING_KEY: u16 = 4330;
const MAX_SUBR_DEPTH: usize = 10;
const MAX_STACK: usize = 48;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Type1Error {
    NoEexec,
    NoCharStrings,
    ShortCharString,
}

impl std::fmt::Display for Type1Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoEexec => formatter.write_str("Type 1 program has no eexec section"),
            Self::NoCharStrings => {
                formatter.write_str("Type 1 private dictionary declares no /CharStrings")
            }
            Self::ShortCharString => {
                formatter.write_str("Type 1 charstring is shorter than its lenIV")
            }
        }
    }
}

impl std::error::Error for Type1Error {}

#[derive(Clone, Debug, PartialEq)]
pub struct Type1Font {
    charstrings: Vec<Vec<u8>>,
    names: HashMap<Vec<u8>, u16>,
    subrs: Vec<Vec<u8>>,
    encoding: [Option<u16>; 256],
    units_per_em: u16,
    paths: PathCache,
}

impl Type1Font {
    pub fn parse(bytes: &[u8]) -> Result<Self, Type1Error> {
        let bytes = strip_pfb(bytes);
        let clear_end = find(&bytes, b"eexec").ok_or(Type1Error::NoEexec)?;
        let clear = &bytes[..clear_end];
        let private = decrypt_eexec(&bytes[clear_end + b"eexec".len()..]);

        let len_iv = integer_after(&private, b"/lenIV").unwrap_or(DEFAULT_LEN_IV);
        let len_iv = usize::try_from(len_iv).unwrap_or(4);
        let subrs = read_subrs(&private, len_iv);
        let (charstrings, names) = read_charstrings(&private, len_iv)?;
        let encoding = read_encoding(clear, &names);
        let units_per_em = read_units_per_em(clear);

        Ok(Self {
            charstrings,
            names,
            subrs,
            encoding,
            units_per_em,
            paths: PathCache::default(),
        })
    }

    #[must_use]
    pub fn units_per_em(&self) -> u16 {
        self.units_per_em
    }

    #[must_use]
    pub fn glyph_count(&self) -> u16 {
        u16::try_from(self.charstrings.len()).unwrap_or(u16::MAX)
    }

    #[must_use]
    pub fn glyph_for_name(&self, name: &[u8]) -> Option<u16> {
        self.names.get(name).copied()
    }

    #[must_use]
    pub fn glyph_name(&self, glyph: u16) -> Option<&[u8]> {
        self.names
            .iter()
            .filter(|(_, found)| **found == glyph)
            .map(|(name, _)| name.as_slice())
            .min()
    }

    #[must_use]
    pub fn glyph_for_code(&self, code: u8) -> Option<u16> {
        self.encoding[usize::from(code)]
    }

    #[must_use]
    pub fn path(&self, glyph: u16) -> Option<GlyphPath> {
        self.paths.get_or(glyph, || {
            let charstring = self.charstrings.get(usize::from(glyph))?;
            let mut run = Run::new(self);
            run.execute(charstring, 0);
            Some(run.finish())
        })
    }
}

fn strip_pfb(bytes: &[u8]) -> Vec<u8> {
    if bytes.first() != Some(&0x80) {
        return bytes.to_vec();
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at + 6 <= bytes.len() && bytes[at] == 0x80 {
        let kind = bytes[at + 1];
        if kind == 3 {
            break;
        }
        let length =
            u32::from_le_bytes([bytes[at + 2], bytes[at + 3], bytes[at + 4], bytes[at + 5]])
                as usize;
        let start = at + 6;
        let end = start.saturating_add(length).min(bytes.len());
        out.extend_from_slice(&bytes[start..end]);
        at = end;
    }
    if out.is_empty() { bytes.to_vec() } else { out }
}

fn decrypt_eexec(bytes: &[u8]) -> Vec<u8> {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let body = &bytes[start..];
    let hex = body
        .iter()
        .filter(|byte| !byte.is_ascii_whitespace())
        .take(4)
        .all(u8::is_ascii_hexdigit)
        && body.len() >= 4;
    let cipher: Vec<u8> = if hex {
        let digits: Vec<u8> = body.iter().copied().filter(u8::is_ascii_hexdigit).collect();
        digits
            .chunks_exact(2)
            .map(|pair| (hex_value(pair[0]) << 4) | hex_value(pair[1]))
            .collect()
    } else {
        body.to_vec()
    };
    let mut plain = decrypt(&cipher, EEXEC_KEY);
    if plain.len() >= 4 {
        plain.drain(..4);
    }
    plain
}

const fn hex_value(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => byte - b'A' + 10,
    }
}

fn decrypt(cipher: &[u8], key: u16) -> Vec<u8> {
    const C1: u16 = 52845;
    const C2: u16 = 22719;
    let mut r = key;
    let mut plain = Vec::with_capacity(cipher.len());
    for byte in cipher {
        plain.push(byte ^ (r >> 8) as u8);
        r = (u16::from(*byte).wrapping_add(r))
            .wrapping_mul(C1)
            .wrapping_add(C2);
    }
    plain
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn integer_after(bytes: &[u8], keyword: &[u8]) -> Option<i64> {
    let at = find(bytes, keyword)? + keyword.len();
    let rest = &bytes[at..];
    let start = rest.iter().position(|byte| !byte.is_ascii_whitespace())?;
    let rest = &rest[start..];
    let end = rest
        .iter()
        .position(|byte| !byte.is_ascii_digit() && *byte != b'-')
        .unwrap_or(rest.len());
    std::str::from_utf8(&rest[..end]).ok()?.parse().ok()
}

fn read_subrs(private: &[u8], len_iv: usize) -> Vec<Vec<u8>> {
    let Some(at) = find(private, b"/Subrs") else {
        return Vec::new();
    };
    let count = integer_after(&private[at..], b"/Subrs").unwrap_or(0).max(0);
    let count = usize::try_from(count).unwrap_or(0);
    let mut subrs = vec![Vec::new(); count];
    let mut cursor = at;
    for _ in 0..count {
        let Some(dup) = find(&private[cursor..], b"dup ") else {
            break;
        };
        let at = cursor + dup + b"dup ".len();
        let Some((index, rest)) = take_integer(&private[at..]) else {
            break;
        };
        let Some((length, rest)) = take_integer(rest) else {
            break;
        };
        let Some(data) = take_binary(private, rest, length) else {
            break;
        };
        if let Ok(index) = usize::try_from(index)
            && index < subrs.len()
        {
            subrs[index] = discard_iv(decrypt(data, CHARSTRING_KEY), len_iv);
        }
        cursor = offset_of(private, data) + data.len();
    }
    subrs
}

type CharStrings = (Vec<Vec<u8>>, HashMap<Vec<u8>, u16>);

fn read_charstrings(private: &[u8], len_iv: usize) -> Result<CharStrings, Type1Error> {
    let at = find(private, b"/CharStrings").ok_or(Type1Error::NoCharStrings)?;
    let mut cursor = at + b"/CharStrings".len();
    if let Some(begin) = find(&private[cursor..], b"begin") {
        cursor += begin + b"begin".len();
    }
    let mut charstrings = Vec::new();
    let mut names = HashMap::new();
    while cursor < private.len() {
        let Some(slash) = private[cursor..].iter().position(|byte| *byte == b'/') else {
            break;
        };
        let at = cursor + slash + 1;
        let end = private[at..]
            .iter()
            .position(|byte| byte.is_ascii_whitespace() || *byte == b'(' || *byte == b'/')
            .map_or(private.len(), |offset| at + offset);
        let name = private[at..end].to_vec();
        let Some((length, rest)) = take_integer(&private[end..]) else {
            break;
        };
        let Some(data) = take_binary(private, rest, length) else {
            break;
        };
        let index = u16::try_from(charstrings.len()).unwrap_or(u16::MAX);
        charstrings.push(discard_iv(decrypt(data, CHARSTRING_KEY), len_iv));
        names.entry(name).or_insert(index);
        cursor = offset_of(private, data) + data.len();
    }
    if charstrings.is_empty() {
        return Err(Type1Error::NoCharStrings);
    }
    Ok((charstrings, names))
}

fn discard_iv(mut plain: Vec<u8>, len_iv: usize) -> Vec<u8> {
    if plain.len() >= len_iv {
        plain.drain(..len_iv);
    }
    plain
}

fn offset_of(whole: &[u8], part: &[u8]) -> usize {
    (part.as_ptr() as usize) - (whole.as_ptr() as usize)
}

fn take_integer(bytes: &[u8]) -> Option<(i64, &[u8])> {
    let start = bytes.iter().position(|byte| !byte.is_ascii_whitespace())?;
    let rest = &bytes[start..];
    let end = rest
        .iter()
        .position(|byte| !byte.is_ascii_digit() && *byte != b'-')?;
    if end == 0 {
        return None;
    }
    let value = std::str::from_utf8(&rest[..end]).ok()?.parse().ok()?;
    Some((value, &rest[end..]))
}

fn take_binary<'a>(whole: &'a [u8], after_length: &'a [u8], length: i64) -> Option<&'a [u8]> {
    let length = usize::try_from(length).ok()?;
    let start = after_length
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())?;
    let keyword_end = after_length[start..]
        .iter()
        .position(u8::is_ascii_whitespace)?;
    let data_start = offset_of(whole, after_length) + start + keyword_end + 1;
    whole.get(data_start..data_start.checked_add(length)?)
}

fn read_encoding(clear: &[u8], names: &HashMap<Vec<u8>, u16>) -> [Option<u16>; 256] {
    let mut encoding = [None; 256];
    let Some(at) = find(clear, b"/Encoding") else {
        return encoding;
    };
    let rest = &clear[at..];
    if find(&rest[..rest.len().min(64)], b"StandardEncoding").is_some() {
        for (code, name) in STANDARD_ENCODING.iter().enumerate() {
            encoding[code] = name.and_then(|name| names.get(name).copied());
        }
        return encoding;
    }
    let mut cursor = 0;
    while let Some(dup) = find(&rest[cursor..], b"dup ") {
        let at = cursor + dup + b"dup ".len();
        let Some((code, after)) = take_integer(&rest[at..]) else {
            break;
        };
        let Some(slash) = after.iter().position(|byte| *byte == b'/') else {
            break;
        };
        let name_start = slash + 1;
        let end = after[name_start..]
            .iter()
            .position(u8::is_ascii_whitespace)
            .map_or(after.len(), |offset| name_start + offset);
        if let Ok(code) = usize::try_from(code)
            && code < 256
        {
            encoding[code] = names.get(&after[name_start..end]).copied();
        }
        cursor = at + (offset_of(rest, after) - offset_of(rest, &rest[at..])) + end;
        if cursor >= rest.len() {
            break;
        }
    }
    encoding
}

fn read_units_per_em(clear: &[u8]) -> u16 {
    const DEFAULT: u16 = 1000;
    let Some(at) = find(clear, b"/FontMatrix") else {
        return DEFAULT;
    };
    let rest = &clear[at..];
    let Some(open) = rest.iter().position(|byte| *byte == b'[') else {
        return DEFAULT;
    };
    let Some(close) = rest.iter().position(|byte| *byte == b']') else {
        return DEFAULT;
    };
    if close <= open {
        return DEFAULT;
    }
    let first = std::str::from_utf8(&rest[open + 1..close])
        .ok()
        .and_then(|text| text.split_whitespace().next().map(str::to_owned))
        .and_then(|text| text.parse::<f64>().ok());
    match first {
        Some(scale) if scale.is_finite() && scale > 0.0 => {
            let em = (1.0 / scale).round();
            if (1.0..=16384.0).contains(&em) {
                #[allow(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "bounded to 1..=16384 on the line above"
                )]
                {
                    em as u16
                }
            } else {
                DEFAULT
            }
        }
        _ => DEFAULT,
    }
}

struct Run<'font> {
    font: &'font Type1Font,
    path: GlyphPath,
    stack: Vec<f64>,
    postscript: Vec<f64>,
    x: f64,
    y: f64,
    open: bool,
    flex: Option<Vec<(f64, f64)>>,
    done: bool,
}

impl<'font> Run<'font> {
    fn new(font: &'font Type1Font) -> Self {
        Self {
            font,
            path: GlyphPath::default(),
            stack: Vec::new(),
            postscript: Vec::new(),
            x: 0.0,
            y: 0.0,
            open: false,
            flex: None,
            done: false,
        }
    }

    fn finish(mut self) -> GlyphPath {
        if self.open {
            self.path.close();
        }
        self.path
    }

    fn move_to(&mut self, dx: f64, dy: f64) {
        self.x += dx;
        self.y += dy;
        if let Some(points) = self.flex.as_mut() {
            points.push((self.x, self.y));
            return;
        }
        if self.open {
            self.path.close();
        }
        self.path.move_to(self.x, self.y);
        self.open = true;
    }

    fn line_to(&mut self, dx: f64, dy: f64) {
        self.x += dx;
        self.y += dy;
        self.path.line_to(self.x, self.y);
    }

    fn curve_to(&mut self, d: [f64; 6]) {
        let x1 = self.x + d[0];
        let y1 = self.y + d[1];
        let x2 = x1 + d[2];
        let y2 = y1 + d[3];
        self.x = x2 + d[4];
        self.y = y2 + d[5];
        self.path.curve_to(x1, y1, x2, y2, self.x, self.y);
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one arm per Type 1 operator; splitting it would hide the language"
    )]
    fn execute(&mut self, code: &[u8], depth: usize) {
        if depth > MAX_SUBR_DEPTH {
            return;
        }
        let mut at = 0;
        while at < code.len() && !self.done {
            let byte = code[at];
            at += 1;
            match byte {
                32..=246 => self.push(f64::from(i16::from(byte) - 139)),
                247..=250 => {
                    let Some(next) = code.get(at) else { return };
                    at += 1;
                    self.push(f64::from(
                        (i16::from(byte) - 247) * 256 + i16::from(*next) + 108,
                    ));
                }
                251..=254 => {
                    let Some(next) = code.get(at) else { return };
                    at += 1;
                    self.push(f64::from(
                        -(i16::from(byte) - 251) * 256 - i16::from(*next) - 108,
                    ));
                }
                255 => {
                    let Some(bytes) = code.get(at..at + 4) else {
                        return;
                    };
                    at += 4;
                    let value = i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                    self.push(f64::from(value));
                }
                4 => {
                    let dy = self.last(1);
                    self.move_to(0.0, dy[0]);
                    self.stack.clear();
                }
                5 => {
                    let d = self.last(2);
                    self.line_to(d[0], d[1]);
                    self.stack.clear();
                }
                6 => {
                    let d = self.last(1);
                    self.line_to(d[0], 0.0);
                    self.stack.clear();
                }
                7 => {
                    let d = self.last(1);
                    self.line_to(0.0, d[0]);
                    self.stack.clear();
                }
                8 => {
                    let d = self.last(6);
                    self.curve_to([d[0], d[1], d[2], d[3], d[4], d[5]]);
                    self.stack.clear();
                }
                9 => {
                    if self.open {
                        self.path.close();
                        self.open = false;
                    }
                    self.stack.clear();
                }
                10 => {
                    let Some(index) = self.stack.pop() else {
                        continue;
                    };
                    #[allow(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "a subroutine index is a small non-negative integer"
                    )]
                    let index = index as usize;
                    if let Some(subr) = self.font.subrs.get(index) {
                        let subr = subr.clone();
                        self.execute(&subr, depth + 1);
                    }
                }
                11 => return,
                13 => {
                    let d = self.last(2);
                    self.x = d[0];
                    self.y = 0.0;
                    self.stack.clear();
                }
                14 => {
                    self.done = true;
                    return;
                }
                21 => {
                    let d = self.last(2);
                    self.move_to(d[0], d[1]);
                    self.stack.clear();
                }
                22 => {
                    let d = self.last(1);
                    self.move_to(d[0], 0.0);
                    self.stack.clear();
                }
                30 => {
                    let d = self.last(4);
                    self.curve_to([0.0, d[0], d[1], d[2], d[3], 0.0]);
                    self.stack.clear();
                }
                31 => {
                    let d = self.last(4);
                    self.curve_to([d[0], 0.0, d[1], d[2], 0.0, d[3]]);
                    self.stack.clear();
                }
                12 => {
                    let Some(second) = code.get(at) else { return };
                    at += 1;
                    self.escape(*second, depth);
                }
                _ => self.stack.clear(),
            }
        }
    }

    fn escape(&mut self, operator: u8, depth: usize) {
        match operator {
            6 => {
                let d = self.last(5);
                self.stack.clear();
                self.done = true;
                self.compose(d[1], d[2], d[3], d[4], depth);
            }
            7 => {
                let d = self.last(4);
                self.x = d[0];
                self.y = d[1];
                self.stack.clear();
            }
            12 => {
                let b = self.stack.pop().unwrap_or(1.0);
                let a = self.stack.pop().unwrap_or(0.0);
                self.push(if b == 0.0 { 0.0 } else { a / b });
            }
            16 => self.other_subr(),
            17 => {
                let value = self.postscript.pop().unwrap_or(0.0);
                self.push(value);
            }
            33 => {
                let d = self.last(2);
                self.x = d[0];
                self.y = d[1];
                self.stack.clear();
            }
            _ => self.stack.clear(),
        }
    }

    fn other_subr(&mut self) {
        let Some(number) = self.stack.pop() else {
            return;
        };
        let count = self.stack.pop().unwrap_or(0.0);
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "an argument count is a small non-negative integer"
        )]
        let count = (count.max(0.0) as usize).min(self.stack.len());
        let arguments: Vec<f64> = self.stack.split_off(self.stack.len() - count);
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "an othersubr number is a small non-negative integer"
        )]
        let number = number.max(0.0) as usize;
        match number {
            0 => {
                if let Some(points) = self.flex.take()
                    && points.len() >= 7
                {
                    self.path.curve_to(
                        points[1].0,
                        points[1].1,
                        points[2].0,
                        points[2].1,
                        points[3].0,
                        points[3].1,
                    );
                    self.path.curve_to(
                        points[4].0,
                        points[4].1,
                        points[5].0,
                        points[5].1,
                        points[6].0,
                        points[6].1,
                    );
                    self.x = points[6].0;
                    self.y = points[6].1;
                }
                self.postscript.push(self.y);
                self.postscript.push(self.x);
            }
            1 => self.flex = Some(Vec::with_capacity(7)),
            2 => {}
            3 => self
                .postscript
                .push(arguments.first().copied().unwrap_or(3.0)),
            _ => self.postscript.extend(arguments.iter().rev()),
        }
    }

    fn compose(&mut self, adx: f64, ady: f64, base: f64, accent: f64, depth: usize) {
        let glyph = |code: f64| -> Option<u16> {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a standard-encoding code is 0..=255"
            )]
            let code = code.clamp(0.0, 255.0) as usize;
            STANDARD_ENCODING
                .get(code)
                .copied()
                .flatten()
                .and_then(|name| self.font.names.get(name).copied())
        };
        for (glyph, dx, dy) in [(glyph(base), 0.0, 0.0), (glyph(accent), adx, ady)] {
            let Some(index) = glyph else { continue };
            let Some(charstring) = self.font.charstrings.get(usize::from(index)) else {
                continue;
            };
            let mut part = Run::new(self.font);
            part.execute(&charstring.clone(), depth + 1);
            let mut path = part.finish();
            path.translate(dx, dy);
            self.path.segments.extend(path.segments);
        }
        self.open = false;
    }

    fn push(&mut self, value: f64) {
        if self.stack.len() < MAX_STACK {
            self.stack.push(value);
        }
    }

    fn last(&self, n: usize) -> Vec<f64> {
        let mut taken = vec![0.0; n];
        let have = self.stack.len().min(n);
        taken[n - have..].copy_from_slice(&self.stack[self.stack.len() - have..]);
        taken
    }
}

#[cfg(test)]
mod tests {
    use super::{CHARSTRING_KEY, EEXEC_KEY, Type1Error, Type1Font};
    use crate::glyph::GlyphSegment;

    fn encrypt(plain: &[u8], key: u16) -> Vec<u8> {
        const C1: u16 = 52845;
        const C2: u16 = 22719;
        let mut r = key;
        let mut cipher = Vec::with_capacity(plain.len());
        for byte in plain {
            let out = byte ^ (r >> 8) as u8;
            cipher.push(out);
            r = (u16::from(out).wrapping_add(r))
                .wrapping_mul(C1)
                .wrapping_add(C2);
        }
        cipher
    }

    fn number(value: i32) -> Vec<u8> {
        match value {
            -107..=107 => vec![u8::try_from(value + 139).expect("in range")],
            108..=1131 => {
                let value = value - 108;
                vec![
                    u8::try_from((value >> 8) + 247).expect("in range"),
                    u8::try_from(value & 0xff).expect("in range"),
                ]
            }
            -1131..=-108 => {
                let value = -value - 108;
                vec![
                    u8::try_from((value >> 8) + 251).expect("in range"),
                    u8::try_from(value & 0xff).expect("in range"),
                ]
            }
            _ => {
                let mut out = vec![255];
                out.extend_from_slice(&value.to_be_bytes());
                out
            }
        }
    }

    fn program(encoding: &[u8]) -> Vec<u8> {
        let mut charstring = Vec::new();
        charstring.extend(number(0));
        charstring.extend(number(600));
        charstring.push(13);
        charstring.extend(number(100));
        charstring.extend(number(100));
        charstring.push(21);
        charstring.extend(number(400));
        charstring.push(6);
        charstring.extend(number(400));
        charstring.push(7);
        charstring.extend(number(-400));
        charstring.push(6);
        charstring.push(9);
        charstring.push(14);
        let mut padded = vec![0x41, 0x42, 0x43, 0x44];
        padded.extend_from_slice(&charstring);
        let enciphered = encrypt(&padded, CHARSTRING_KEY);

        let mut private = b"XXXX dup /Private 8 dict dup begin\n/lenIV 4 def\n".to_vec();
        private.extend_from_slice(b"/CharStrings 2 dict dup begin\n");
        private.extend_from_slice(b"/.notdef 5 RD ");
        private.extend_from_slice(&encrypt(&[0x41, 0x42, 0x43, 0x44, 14], CHARSTRING_KEY));
        private.extend_from_slice(b" ND\n/square ");
        private.extend_from_slice(enciphered.len().to_string().as_bytes());
        private.extend_from_slice(b" RD ");
        private.extend_from_slice(&enciphered);
        private.extend_from_slice(b" ND\nend end\n");

        let mut whole =
            b"%!PS-AdobeFont-1.0: Test 001.001\n/FontMatrix [0.001 0 0 0.001 0 0] readonly def\n"
                .to_vec();
        whole.extend_from_slice(encoding);
        whole.extend_from_slice(b"currentfile eexec\n");
        whole.extend_from_slice(&encrypt(&private, EEXEC_KEY));
        whole
    }

    #[test]
    fn a_type_one_program_decrypts_to_the_outline_its_charstring_draws() {
        let font = Type1Font::parse(&program(
            b"/Encoding 256 array\ndup 65 /square put\nreadonly def\n",
        ))
        .expect("a Type 1 program");
        assert_eq!(font.units_per_em(), 1000);
        assert_eq!(font.glyph_count(), 2, ".notdef and square");

        let glyph = font.glyph_for_name(b"square").expect("the named glyph");
        assert_eq!(
            font.glyph_for_code(b'A'),
            Some(glyph),
            "the program's own /Encoding puts it at code 65"
        );
        assert_eq!(font.glyph_for_code(b'B'), None, "and at no other code");

        assert_eq!(
            font.path(glyph).expect("an outline").segments,
            vec![
                GlyphSegment::MoveTo { x: 100.0, y: 100.0 },
                GlyphSegment::LineTo { x: 500.0, y: 100.0 },
                GlyphSegment::LineTo { x: 500.0, y: 500.0 },
                GlyphSegment::LineTo { x: 100.0, y: 500.0 },
                GlyphSegment::Close,
            ]
        );
    }

    #[test]
    fn the_short_form_encoding_is_the_standard_one() {
        let font = Type1Font::parse(&program(b"/Encoding StandardEncoding def\n"))
            .expect("a Type 1 program");
        assert_eq!(font.glyph_for_code(b'A'), None);
        assert!(font.glyph_for_name(b"square").is_some());
    }

    #[test]
    fn bytes_that_are_not_a_type_one_program_are_refused_by_name() {
        assert_eq!(
            Type1Font::parse(b"%!PS-AdobeFont-1.0: Test\n").expect_err("no eexec"),
            Type1Error::NoEexec
        );
        let mut no_charstrings = b"%!PS\ncurrentfile eexec\n".to_vec();
        no_charstrings.extend_from_slice(&encrypt(b"XXXX /Private 1 dict\n", EEXEC_KEY));
        assert_eq!(
            Type1Font::parse(&no_charstrings).expect_err("no charstrings"),
            Type1Error::NoCharStrings
        );
    }
}
