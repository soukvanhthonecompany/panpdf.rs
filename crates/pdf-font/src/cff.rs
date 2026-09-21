use std::collections::HashMap;

use crate::glyph::GlyphPath;
use crate::tables::{STANDARD_ENCODING, STANDARD_STRINGS};

#[derive(Clone, Debug, PartialEq)]
pub struct CffFont {
    charstrings: Index,
    global_subrs: Index,
    local_subrs: Index,
    font_dict_subrs: Vec<Index>,
    fd_select: Vec<u8>,
    units_per_em: u16,
    is_cid: bool,
    glyph_for_name: HashMap<Vec<u8>, u16>,
    encoding: [u16; 256],
    glyph_for_cid: Vec<(u16, u16)>,
}

const MAX_CALL_DEPTH: usize = 10;
const MAX_STACK: usize = 48;

impl CffFont {
    pub fn parse(data: &[u8]) -> Result<Self, CffError> {
        let header_size = usize::from(*data.get(2).ok_or(CffError::Malformed)?);
        let names = Index::parse(data, header_size)?;
        let top_dicts = Index::parse(data, names.end)?;
        let strings = Index::parse(data, top_dicts.end)?;
        let global_subrs = Index::parse(data, strings.end)?;
        let top = Dict::parse(top_dicts.get(data, 0).ok_or(CffError::Malformed)?)?;

        if top.offset(&[12, 6]).unwrap_or(2) != 2 {
            return Err(CffError::UnsupportedCharstringType);
        }
        let charstrings_at = top.offset(&[17]).ok_or(CffError::Malformed)?;
        let charstrings = Index::parse(data, charstrings_at)?;
        let is_cid = top.has(&[12, 30]);
        let units_per_em = font_matrix_units(&top);

        let local_subrs = top
            .private(data)
            .map(|private| private.subrs)
            .unwrap_or_default();

        let mut font_dict_subrs = Vec::new();
        if let Some(fd_array_at) = top.offset(&[12, 36]) {
            let fd_array = Index::parse(data, fd_array_at)?;
            for index in 0..fd_array.count {
                let dict = Dict::parse(fd_array.get(data, index).ok_or(CffError::Malformed)?)?;
                font_dict_subrs.push(dict.private(data).map(|p| p.subrs).unwrap_or_default());
            }
        }
        let fd_select = match top.offset(&[12, 37]) {
            Some(at) => parse_fd_select(data, at, charstrings.count)?,
            None => Vec::new(),
        };
        let (glyph_for_name, encoding, glyph_for_cid) = if is_cid {
            (
                HashMap::new(),
                [0; 256],
                charset_cids(data, &top, charstrings.count)?,
            )
        } else {
            let names = charset_names(data, &top, &strings, charstrings.count)?;
            let encoding = parse_encoding(data, &top, &strings, &names)?;
            (names, encoding, Vec::new())
        };
        Ok(Self {
            charstrings,
            global_subrs,
            local_subrs,
            font_dict_subrs,
            fd_select,
            units_per_em,
            is_cid,
            glyph_for_name,
            encoding,
            glyph_for_cid,
        })
    }

    #[must_use]
    pub fn glyph_for_cid(&self, cid: u16) -> Option<u16> {
        if !self.is_cid {
            return None;
        }
        self.glyph_for_cid
            .binary_search_by(|(known, _)| known.cmp(&cid))
            .ok()
            .map(|found| self.glyph_for_cid[found].1)
    }

    #[must_use]
    pub fn cid_for_glyph(&self, glyph: u16) -> Option<u16> {
        if !self.is_cid {
            return None;
        }
        self.glyph_for_cid
            .iter()
            .find(|(_, found)| *found == glyph)
            .map(|(cid, _)| *cid)
    }

    #[must_use]
    pub fn glyph_for_name(&self, name: &[u8]) -> Option<u16> {
        self.glyph_for_name.get(name).copied()
    }

    #[must_use]
    pub fn glyph_name(&self, glyph: u16) -> Option<&[u8]> {
        self.glyph_for_name
            .iter()
            .filter(|(_, found)| **found == glyph)
            .map(|(name, _)| name.as_slice())
            .min()
    }

    #[must_use]
    pub fn glyph_for_code(&self, code: u8) -> Option<u16> {
        match self.encoding[usize::from(code)] {
            0 => None,
            glyph => Some(glyph),
        }
    }

    #[must_use]
    pub const fn units_per_em(&self) -> u16 {
        self.units_per_em
    }

    #[must_use]
    pub const fn glyph_count(&self) -> usize {
        self.charstrings.count
    }

    #[must_use]
    pub const fn is_cid(&self) -> bool {
        self.is_cid
    }

    #[must_use]
    pub fn outline(&self, data: &[u8], glyph: u16) -> Option<GlyphPath> {
        let index = usize::from(glyph);
        let charstring = self.charstrings.get(data, index)?;
        let local = self
            .fd_select
            .get(index)
            .and_then(|fd| self.font_dict_subrs.get(usize::from(*fd)))
            .unwrap_or(&self.local_subrs);
        let mut state = Charstring {
            data,
            global: &self.global_subrs,
            local,
            path: GlyphPath::default(),
            stack: Vec::new(),
            x: 0.0,
            y: 0.0,
            stems: 0,
            width_parsed: false,
            open: false,
            global_calls: Vec::new(),
            local_calls: Vec::new(),
        };
        state.run(charstring, 0).ok()?;
        state.finish();
        Some(state.path)
    }

    pub(crate) fn calls(
        &self,
        data: &[u8],
        glyph: u16,
    ) -> Option<(Vec<usize>, Vec<usize>, Option<usize>)> {
        let index = usize::from(glyph);
        let charstring = self.charstrings.get(data, index)?;
        let fd = self.fd_select.get(index).map(|fd| usize::from(*fd));
        let local = fd
            .and_then(|fd| self.font_dict_subrs.get(fd))
            .unwrap_or(&self.local_subrs);
        let mut state = Charstring {
            data,
            global: &self.global_subrs,
            local,
            path: GlyphPath::default(),
            stack: Vec::new(),
            x: 0.0,
            y: 0.0,
            stems: 0,
            width_parsed: false,
            open: false,
            global_calls: Vec::new(),
            local_calls: Vec::new(),
        };
        state.run(charstring, 0).ok()?;
        Some((state.global_calls, state.local_calls, fd))
    }
}

fn font_matrix_units(top: &Dict) -> u16 {
    let Some(scale) = top.real(&[12, 7]) else {
        return 1000;
    };
    if scale <= 0.0 {
        return 1000;
    }
    let units = 1.0 / scale;
    if !units.is_finite() || units < 1.0 || units > f64::from(u16::MAX) {
        return 1000;
    }
    let mut candidate = 1_u16;
    while f64::from(candidate) < units && candidate < u16::MAX {
        candidate = candidate.saturating_add(1);
    }
    candidate
}

fn charset_names(
    data: &[u8],
    top: &Dict,
    strings: &Index,
    glyphs: usize,
) -> Result<HashMap<Vec<u8>, u16>, CffError> {
    let at = top.offset(&[15]).unwrap_or(0);
    let sids = match at {
        0 => (0..glyphs.min(229))
            .map(|glyph| u16::try_from(glyph).unwrap_or(u16::MAX))
            .collect(),
        1 | 2 => Vec::new(),
        _ => parse_charset(data, at, glyphs)?,
    };
    let mut names = HashMap::new();
    for (glyph, sid) in sids.iter().enumerate() {
        let Some(name) = sid_name(data, strings, *sid) else {
            continue;
        };
        let Ok(glyph) = u16::try_from(glyph) else {
            continue;
        };
        names.entry(name.to_vec()).or_insert(glyph);
    }
    Ok(names)
}

fn charset_cids(data: &[u8], top: &Dict, glyphs: usize) -> Result<Vec<(u16, u16)>, CffError> {
    let at = top.offset(&[15]).unwrap_or(0);
    if at <= 2 {
        return Ok(Vec::new());
    }
    let cids = parse_charset(data, at, glyphs)?;
    let mut map: Vec<(u16, u16)> = Vec::with_capacity(cids.len());
    for (glyph, cid) in cids.iter().enumerate() {
        let Ok(glyph) = u16::try_from(glyph) else {
            continue;
        };
        map.push((*cid, glyph));
    }
    map.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
    map.dedup_by_key(|(cid, _)| *cid);
    Ok(map)
}

fn sid_name<'a>(data: &'a [u8], strings: &Index, sid: u16) -> Option<&'a [u8]> {
    match usize::from(sid).checked_sub(STANDARD_STRINGS.len()) {
        None => Some(STANDARD_STRINGS[usize::from(sid)]),
        Some(custom) => strings.get(data, custom),
    }
}

fn parse_charset(data: &[u8], at: usize, glyphs: usize) -> Result<Vec<u16>, CffError> {
    let format = *data.get(at).ok_or(CffError::Malformed)?;
    let mut sids = vec![0_u16];
    let mut cursor = at + 1;
    match format {
        0 => {
            while sids.len() < glyphs {
                sids.push(read_u16(data, cursor).ok_or(CffError::Malformed)?);
                cursor += 2;
            }
        }
        1 | 2 => {
            let left_bytes = if format == 1 { 1 } else { 2 };
            while sids.len() < glyphs {
                let first = read_u16(data, cursor).ok_or(CffError::Malformed)?;
                cursor += 2;
                let left = if format == 1 {
                    u32::from(*data.get(cursor).ok_or(CffError::Malformed)?)
                } else {
                    u32::from(read_u16(data, cursor).ok_or(CffError::Malformed)?)
                };
                cursor += left_bytes;
                for step in 0..=left {
                    if sids.len() >= glyphs {
                        break;
                    }
                    let Some(sid) = u32::from(first).checked_add(step) else {
                        return Err(CffError::Malformed);
                    };
                    sids.push(u16::try_from(sid).map_err(|_| CffError::Malformed)?);
                }
            }
        }
        _ => return Err(CffError::Malformed),
    }
    Ok(sids)
}

fn parse_encoding(
    data: &[u8],
    top: &Dict,
    strings: &Index,
    names: &HashMap<Vec<u8>, u16>,
) -> Result<[u16; 256], CffError> {
    let mut encoding = [0_u16; 256];
    let at = top.offset(&[16]).unwrap_or(0);
    if at == 0 {
        for (code, name) in STANDARD_ENCODING.iter().enumerate() {
            if let Some(glyph) = name.and_then(|name| names.get(name)) {
                encoding[code] = *glyph;
            }
        }
        return Ok(encoding);
    }
    if at == 1 {
        return Ok(encoding);
    }
    let format = *data.get(at).ok_or(CffError::Malformed)?;
    let mut cursor = at + 1;
    let mut glyph = 1_u16;
    match format & 0x7f {
        0 => {
            let count = usize::from(*data.get(cursor).ok_or(CffError::Malformed)?);
            cursor += 1;
            for _ in 0..count {
                let code = *data.get(cursor).ok_or(CffError::Malformed)?;
                cursor += 1;
                encoding[usize::from(code)] = glyph;
                glyph = glyph.checked_add(1).ok_or(CffError::Malformed)?;
            }
        }
        1 => {
            let ranges = usize::from(*data.get(cursor).ok_or(CffError::Malformed)?);
            cursor += 1;
            for _ in 0..ranges {
                let first = *data.get(cursor).ok_or(CffError::Malformed)?;
                let left = *data.get(cursor + 1).ok_or(CffError::Malformed)?;
                cursor += 2;
                for step in 0..=u16::from(left) {
                    let Some(code) = u16::from(first).checked_add(step) else {
                        break;
                    };
                    if code > 255 {
                        break;
                    }
                    encoding[usize::from(code)] = glyph;
                    glyph = glyph.checked_add(1).ok_or(CffError::Malformed)?;
                }
            }
        }
        _ => return Err(CffError::Malformed),
    }
    if format & 0x80 != 0 {
        let count = usize::from(*data.get(cursor).ok_or(CffError::Malformed)?);
        cursor += 1;
        for _ in 0..count {
            let code = *data.get(cursor).ok_or(CffError::Malformed)?;
            let sid = read_u16(data, cursor + 1).ok_or(CffError::Malformed)?;
            cursor += 3;
            if let Some(glyph) = sid_name(data, strings, sid).and_then(|name| names.get(name)) {
                encoding[usize::from(code)] = *glyph;
            }
        }
    }
    Ok(encoding)
}

fn parse_fd_select(data: &[u8], at: usize, glyphs: usize) -> Result<Vec<u8>, CffError> {
    let mut out = vec![0_u8; glyphs];
    match data.get(at).ok_or(CffError::Malformed)? {
        0 => {
            let table = data
                .get(at + 1..at + 1 + glyphs)
                .ok_or(CffError::Malformed)?;
            out.copy_from_slice(table);
        }
        3 => {
            let ranges = usize::from(read_u16(data, at + 1).ok_or(CffError::Malformed)?);
            let sentinel = read_u16(data, at + 3 + ranges * 3).ok_or(CffError::Malformed)?;
            for range in 0..ranges {
                let entry = at + 3 + range * 3;
                let first = usize::from(read_u16(data, entry).ok_or(CffError::Malformed)?);
                let fd = *data.get(entry + 2).ok_or(CffError::Malformed)?;
                let next = if range + 1 == ranges {
                    usize::from(sentinel)
                } else {
                    usize::from(read_u16(data, entry + 3).ok_or(CffError::Malformed)?)
                };
                for slot in out.iter_mut().take(next.min(glyphs)).skip(first) {
                    *slot = fd;
                }
            }
        }
        _ => return Err(CffError::Malformed),
    }
    Ok(out)
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Index {
    offsets_at: usize,
    data_at: usize,
    offset_size: usize,
    count: usize,
    end: usize,
}

impl Index {
    fn parse(data: &[u8], at: usize) -> Result<Self, CffError> {
        let count = usize::from(read_u16(data, at).ok_or(CffError::Malformed)?);
        if count == 0 {
            return Ok(Self {
                end: at + 2,
                ..Self::default()
            });
        }
        let offset_size = usize::from(*data.get(at + 2).ok_or(CffError::Malformed)?);
        if !(1..=4).contains(&offset_size) {
            return Err(CffError::Malformed);
        }
        let offsets_at = at + 3;
        let data_at = offsets_at + (count + 1) * offset_size - 1;
        let index = Self {
            offsets_at,
            data_at,
            offset_size,
            count,
            end: 0,
        };
        let last = index.offset(data, count).ok_or(CffError::Malformed)?;
        let end = data_at.checked_add(last).ok_or(CffError::Malformed)?;
        if end > data.len() {
            return Err(CffError::Malformed);
        }
        Ok(Self { end, ..index })
    }

    fn offset(&self, data: &[u8], index: usize) -> Option<usize> {
        let at = self.offsets_at + index * self.offset_size;
        let bytes = data.get(at..at + self.offset_size)?;
        Some(
            bytes
                .iter()
                .fold(0_usize, |value, byte| value * 256 + usize::from(*byte)),
        )
    }

    fn get<'a>(&self, data: &'a [u8], index: usize) -> Option<&'a [u8]> {
        if index >= self.count {
            return None;
        }
        let start = self.data_at + self.offset(data, index)?;
        let end = self.data_at + self.offset(data, index + 1)?;
        (start <= end).then(|| data.get(start..end)).flatten()
    }

    const fn bias(&self) -> i32 {
        if self.count < 1240 {
            107
        } else if self.count < 33900 {
            1131
        } else {
            32768
        }
    }
}

struct PrivateDict {
    subrs: Index,
}

#[derive(Clone, Debug, Default)]
struct Dict {
    entries: Vec<(Vec<u8>, Vec<f64>)>,
}

impl Dict {
    fn parse(data: &[u8]) -> Result<Self, CffError> {
        let mut entries = Vec::new();
        let mut operands: Vec<f64> = Vec::new();
        let mut cursor = 0_usize;
        while cursor < data.len() {
            let byte = data[cursor];
            match byte {
                0..=21 => {
                    let operator = if byte == 12 {
                        let second = *data.get(cursor + 1).ok_or(CffError::Malformed)?;
                        cursor += 2;
                        vec![12, second]
                    } else {
                        cursor += 1;
                        vec![byte]
                    };
                    entries.push((operator, std::mem::take(&mut operands)));
                }
                28 => {
                    operands.push(f64::from(
                        read_u16(data, cursor + 1)
                            .ok_or(CffError::Malformed)?
                            .cast_signed(),
                    ));
                    cursor += 3;
                }
                29 => {
                    let bytes = data
                        .get(cursor + 1..cursor + 5)
                        .ok_or(CffError::Malformed)?;
                    operands.push(f64::from(i32::from_be_bytes([
                        bytes[0], bytes[1], bytes[2], bytes[3],
                    ])));
                    cursor += 5;
                }
                30 => {
                    let (value, used) = parse_real(data, cursor + 1)?;
                    operands.push(value);
                    cursor += 1 + used;
                }
                32..=246 => {
                    operands.push(f64::from(i32::from(byte) - 139));
                    cursor += 1;
                }
                247..=250 => {
                    let next = i32::from(*data.get(cursor + 1).ok_or(CffError::Malformed)?);
                    operands.push(f64::from((i32::from(byte) - 247) * 256 + next + 108));
                    cursor += 2;
                }
                251..=254 => {
                    let next = i32::from(*data.get(cursor + 1).ok_or(CffError::Malformed)?);
                    operands.push(f64::from(-(i32::from(byte) - 251) * 256 - next - 108));
                    cursor += 2;
                }
                _ => return Err(CffError::Malformed),
            }
            if operands.len() > MAX_STACK {
                return Err(CffError::Malformed);
            }
        }
        Ok(Self { entries })
    }

    fn operands(&self, operator: &[u8]) -> Option<&[f64]> {
        self.entries
            .iter()
            .find(|(key, _)| key == operator)
            .map(|(_, values)| values.as_slice())
    }

    fn has(&self, operator: &[u8]) -> bool {
        self.operands(operator).is_some()
    }

    fn offset(&self, operator: &[u8]) -> Option<usize> {
        offset_operand(self.real(operator)?)
    }

    fn real(&self, operator: &[u8]) -> Option<f64> {
        self.operands(operator)?.last().copied()
    }

    fn private(&self, data: &[u8]) -> Option<PrivateDict> {
        let values = self.operands(&[18])?;
        let [size, at] = values else { return None };
        let size = offset_operand(*size)?;
        let at = offset_operand(*at)?;
        let dict = Dict::parse(data.get(at..at.checked_add(size)?)?).ok()?;
        let subrs = match dict.offset(&[19]) {
            Some(offset) => Index::parse(data, at.checked_add(offset)?).ok()?,
            None => Index::default(),
        };
        Some(PrivateDict { subrs })
    }
}

fn parse_real(data: &[u8], at: usize) -> Result<(f64, usize), CffError> {
    let mut text = String::new();
    let mut used = 0_usize;
    loop {
        let byte = *data.get(at + used).ok_or(CffError::Malformed)?;
        used += 1;
        for nibble in [byte >> 4, byte & 0x0f] {
            match nibble {
                0..=9 => text.push(char::from(b'0' + nibble)),
                0x0a => text.push('.'),
                0x0b => text.push('E'),
                0x0c => text.push_str("E-"),
                0x0e => text.push('-'),
                0x0f => {
                    return text
                        .parse::<f64>()
                        .map(|value| (value, used))
                        .map_err(|_| CffError::Malformed);
                }
                _ => return Err(CffError::Malformed),
            }
        }
        if used > 32 {
            return Err(CffError::Malformed);
        }
    }
}

struct Charstring<'a> {
    data: &'a [u8],
    global: &'a Index,
    local: &'a Index,
    path: GlyphPath,
    stack: Vec<f64>,
    x: f64,
    y: f64,
    stems: usize,
    width_parsed: bool,
    open: bool,
    global_calls: Vec<usize>,
    local_calls: Vec<usize>,
}

impl Charstring<'_> {
    fn finish(&mut self) {
        if self.open {
            self.path.close();
            self.open = false;
        }
    }

    fn start_contour(&mut self) {
        self.finish();
        self.path.move_to(self.x, self.y);
        self.open = true;
    }

    fn take_width(&mut self, even: bool) {
        if !self.width_parsed {
            self.width_parsed = true;
            if (self.stack.len() % 2 == 1) == even {
                self.stack.remove(0);
            }
        }
    }

    fn run(&mut self, code: &[u8], depth: usize) -> Result<bool, CffError> {
        if depth > MAX_CALL_DEPTH {
            return Err(CffError::Malformed);
        }
        let mut cursor = 0_usize;
        while cursor < code.len() {
            let byte = code[cursor];
            cursor += 1;
            match byte {
                28 => {
                    self.push(f64::from(
                        read_u16(code, cursor)
                            .ok_or(CffError::Malformed)?
                            .cast_signed(),
                    ))?;
                    cursor += 2;
                }
                32..=246 => self.push(f64::from(i32::from(byte) - 139))?,
                247..=250 => {
                    let next = i32::from(*code.get(cursor).ok_or(CffError::Malformed)?);
                    cursor += 1;
                    self.push(f64::from((i32::from(byte) - 247) * 256 + next + 108))?;
                }
                251..=254 => {
                    let next = i32::from(*code.get(cursor).ok_or(CffError::Malformed)?);
                    cursor += 1;
                    self.push(f64::from(-(i32::from(byte) - 251) * 256 - next - 108))?;
                }
                255 => {
                    let bytes = code.get(cursor..cursor + 4).ok_or(CffError::Malformed)?;
                    cursor += 4;
                    self.push(
                        f64::from(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
                            / 65536.0,
                    )?;
                }
                1 | 3 | 18 | 23 => {
                    self.take_width(true);
                    self.stems += self.stack.len() / 2;
                    self.stack.clear();
                }
                19 | 20 => {
                    self.take_width(true);
                    self.stems += self.stack.len() / 2;
                    self.stack.clear();
                    cursor += self.stems.div_ceil(8);
                }
                4 | 5 | 6 | 7 | 8 | 21 | 22 | 24 | 25 | 26 | 27 | 30 | 31 => self.draw(byte),
                10 => {
                    let index = self.pop_subr(self.local.bias())?;
                    self.local_calls.push(index);
                    let body = self
                        .local
                        .get(self.data, index)
                        .ok_or(CffError::Malformed)?;
                    if self.run(body, depth + 1)? {
                        return Ok(true);
                    }
                }
                29 => {
                    let index = self.pop_subr(self.global.bias())?;
                    self.global_calls.push(index);
                    let body = self
                        .global
                        .get(self.data, index)
                        .ok_or(CffError::Malformed)?;
                    if self.run(body, depth + 1)? {
                        return Ok(true);
                    }
                }
                11 => return Ok(false),
                14 => {
                    self.take_width(true);
                    self.finish();
                    return Ok(true);
                }
                12 => {
                    let second = *code.get(cursor).ok_or(CffError::Malformed)?;
                    cursor += 1;
                    self.escape(second);
                }
                _ => return Err(CffError::Malformed),
            }
        }
        Ok(false)
    }

    fn draw(&mut self, operator: u8) {
        match operator {
            21 => {
                self.take_width(true);
                let [dx, dy] = self.last::<2>();
                self.x += dx;
                self.y += dy;
                self.start_contour();
                self.stack.clear();
            }
            22 | 4 => {
                self.take_width(false);
                let [delta] = self.last::<1>();
                if operator == 22 {
                    self.x += delta;
                } else {
                    self.y += delta;
                }
                self.start_contour();
                self.stack.clear();
            }
            5 => {
                for pair in std::mem::take(&mut self.stack).chunks_exact(2) {
                    self.x += pair[0];
                    self.y += pair[1];
                    self.path.line_to(self.x, self.y);
                }
            }
            6 | 7 => {
                let mut horizontal = operator == 6;
                for value in std::mem::take(&mut self.stack) {
                    if horizontal {
                        self.x += value;
                    } else {
                        self.y += value;
                    }
                    self.path.line_to(self.x, self.y);
                    horizontal = !horizontal;
                }
            }
            8 => {
                for values in std::mem::take(&mut self.stack).chunks_exact(6) {
                    self.curve(
                        values[0], values[1], values[2], values[3], values[4], values[5],
                    );
                }
            }
            24 => {
                let values = std::mem::take(&mut self.stack);
                let curves = values.len().saturating_sub(2) / 6;
                for chunk in values[..curves * 6].chunks_exact(6) {
                    self.curve(chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5]);
                }
                if let Some(pair) = values.get(curves * 6..curves * 6 + 2) {
                    self.x += pair[0];
                    self.y += pair[1];
                    self.path.line_to(self.x, self.y);
                }
            }
            25 => {
                let values = std::mem::take(&mut self.stack);
                let lines = values.len().saturating_sub(6) / 2;
                for pair in values[..lines * 2].chunks_exact(2) {
                    self.x += pair[0];
                    self.y += pair[1];
                    self.path.line_to(self.x, self.y);
                }
                if let Some(chunk) = values.get(lines * 2..lines * 2 + 6) {
                    self.curve(chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5]);
                }
            }
            26 | 27 => self.axis_curves(operator == 26),
            _ => self.alternating_curves(operator == 31),
        }
    }

    fn escape(&mut self, operator: u8) {
        let values = std::mem::take(&mut self.stack);
        match operator {
            35 if values.len() >= 13 => {
                self.curve(
                    values[0], values[1], values[2], values[3], values[4], values[5],
                );
                self.curve(
                    values[6], values[7], values[8], values[9], values[10], values[11],
                );
            }
            34 if values.len() >= 7 => {
                let start_y = self.y;
                self.curve(values[0], 0.0, values[1], values[2], values[3], 0.0);
                self.curve(values[4], 0.0, values[5], start_y - self.y, values[6], 0.0);
            }
            36 if values.len() >= 9 => {
                let start_y = self.y;
                self.curve(values[0], values[1], values[2], values[3], values[4], 0.0);
                self.curve(
                    values[5],
                    0.0,
                    values[6],
                    values[7],
                    values[8],
                    start_y - self.y - values[7],
                );
            }
            37 if values.len() >= 11 => {
                let (start_x, start_y) = (self.x, self.y);
                self.curve(
                    values[0], values[1], values[2], values[3], values[4], values[5],
                );
                let dx = values[6];
                let dy = values[7];
                let dx2 = values[8];
                let dy2 = values[9];
                let total_x = self.x + dx + dx2 - start_x;
                let total_y = self.y + dy + dy2 - start_y;
                if total_x.abs() > total_y.abs() {
                    self.curve(dx, dy, dx2, dy2, values[10], start_y - (self.y + dy + dy2));
                } else {
                    self.curve(dx, dy, dx2, dy2, start_x - (self.x + dx + dx2), values[10]);
                }
            }
            _ => {}
        }
    }

    fn axis_curves(&mut self, vertical: bool) {
        let mut values = std::mem::take(&mut self.stack);
        let mut lead = 0.0;
        if values.len() % 4 == 1 {
            lead = values.remove(0);
        }
        for chunk in values.chunks_exact(4) {
            if vertical {
                self.curve(lead, chunk[0], chunk[1], chunk[2], 0.0, chunk[3]);
            } else {
                self.curve(chunk[0], lead, chunk[1], chunk[2], chunk[3], 0.0);
            }
            lead = 0.0;
        }
    }

    fn alternating_curves(&mut self, mut horizontal: bool) {
        let values = std::mem::take(&mut self.stack);
        let mut cursor = 0_usize;
        while values.len() - cursor >= 4 {
            let remaining = values.len() - cursor;
            let last = if remaining == 5 {
                values[cursor + 4]
            } else {
                0.0
            };
            let chunk = &values[cursor..cursor + 4];
            if horizontal {
                self.curve(chunk[0], 0.0, chunk[1], chunk[2], last, chunk[3]);
            } else {
                self.curve(0.0, chunk[0], chunk[1], chunk[2], chunk[3], last);
            }
            horizontal = !horizontal;
            cursor += 4;
        }
    }

    fn curve(&mut self, dx1: f64, dy1: f64, dx2: f64, dy2: f64, dx3: f64, dy3: f64) {
        if !self.open {
            self.start_contour();
        }
        let x1 = self.x + dx1;
        let y1 = self.y + dy1;
        let x2 = x1 + dx2;
        let y2 = y1 + dy2;
        self.x = x2 + dx3;
        self.y = y2 + dy3;
        self.path.curve_to(x1, y1, x2, y2, self.x, self.y);
    }

    fn push(&mut self, value: f64) -> Result<(), CffError> {
        if self.stack.len() >= MAX_STACK {
            return Err(CffError::Malformed);
        }
        self.stack.push(value);
        Ok(())
    }

    fn pop_subr(&mut self, bias: i32) -> Result<usize, CffError> {
        let value = self.stack.pop().ok_or(CffError::Malformed)?;
        offset_operand(value + f64::from(bias)).ok_or(CffError::Malformed)
    }

    fn last<const N: usize>(&self) -> [f64; N] {
        let mut out = [0.0; N];
        let start = self.stack.len().saturating_sub(N);
        for (slot, value) in out.iter_mut().zip(&self.stack[start..]) {
            *slot = *value;
        }
        out
    }
}

fn offset_operand(value: f64) -> Option<usize> {
    if !value.is_finite() || value < 0.0 || value > f64::from(u32::MAX) {
        return None;
    }
    let mut low = 0_u32;
    let mut high = u32::MAX;
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        if f64::from(middle) <= value {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    f64::from(low)
        .total_cmp(&value)
        .is_eq()
        .then_some(low as usize)
}

fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    let bytes = data.get(offset..offset + 2)?;
    Some(u16::from_be_bytes([bytes[0], bytes[1]]))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CffError {
    Malformed,
    UnsupportedCharstringType,
}

impl std::fmt::Display for CffError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Malformed => "CFF font program is malformed",
            Self::UnsupportedCharstringType => "CFF font uses unsupported charstrings",
        })
    }
}

impl std::error::Error for CffError {}

#[cfg(test)]
mod tests {
    use super::{CffError, CffFont};
    use crate::glyph::GlyphSegment;

    fn font_with_charstrings(charstrings: &[&[u8]]) -> Vec<u8> {
        fn index(items: &[&[u8]]) -> Vec<u8> {
            if items.is_empty() {
                return vec![0, 0];
            }
            let mut out = Vec::new();
            out.extend_from_slice(&u16::try_from(items.len()).expect("count").to_be_bytes());
            out.push(1);
            let mut offset = 1_u8;
            out.push(offset);
            for item in items {
                offset += u8::try_from(item.len()).expect("item length");
                out.push(offset);
            }
            for item in items {
                out.extend_from_slice(item);
            }
            out
        }

        let names = index(&[b"Test"]);
        let strings = index(&[]);
        let global_subrs = index(&[]);
        let charstring_index = index(charstrings);
        let header_len = 4_usize;
        let top_dict_len = 6_usize;
        let dict_index_len = 2 + 1 + 2 + top_dict_len;
        let charstrings_at =
            header_len + names.len() + dict_index_len + strings.len() + global_subrs.len();
        let mut top_dict = vec![29];
        top_dict.extend_from_slice(&u32::try_from(charstrings_at).expect("offset").to_be_bytes());
        top_dict.push(17);
        assert_eq!(top_dict.len(), top_dict_len);
        let top_dicts = index(&[&top_dict]);
        assert_eq!(top_dicts.len(), dict_index_len);

        let mut out = vec![1, 0, 4, 1];
        out.extend_from_slice(&names);
        out.extend_from_slice(&top_dicts);
        out.extend_from_slice(&strings);
        out.extend_from_slice(&global_subrs);
        out.extend_from_slice(&charstring_index);
        out
    }

    fn font_with_tables(
        charstrings: &[&[u8]],
        strings: &[&[u8]],
        charset: &[u8],
        encoding: &[u8],
    ) -> Vec<u8> {
        fn index(items: &[&[u8]]) -> Vec<u8> {
            if items.is_empty() {
                return vec![0, 0];
            }
            let mut out = Vec::new();
            out.extend_from_slice(&u16::try_from(items.len()).expect("count").to_be_bytes());
            out.push(1);
            let mut offset = 1_u8;
            out.push(offset);
            for item in items {
                offset += u8::try_from(item.len()).expect("item length");
                out.push(offset);
            }
            for item in items {
                out.extend_from_slice(item);
            }
            out
        }
        fn operand(value: usize, operator: u8) -> Vec<u8> {
            let mut out = vec![29];
            out.extend_from_slice(&u32::try_from(value).expect("offset").to_be_bytes());
            out.push(operator);
            out
        }

        let names = index(&[b"Test"]);
        let string_index = index(strings);
        let global_subrs = index(&[]);
        let charstring_index = index(charstrings);
        let top_dict_len = 18_usize;
        let dict_index_len = 2 + 1 + 2 + top_dict_len;
        let before_charstrings =
            4 + names.len() + dict_index_len + string_index.len() + global_subrs.len();
        let charset_at = before_charstrings + charstring_index.len();
        let encoding_at = charset_at + charset.len();

        let mut top_dict = operand(before_charstrings, 17);
        top_dict.extend_from_slice(&operand(charset_at, 15));
        top_dict.extend_from_slice(&operand(encoding_at, 16));
        assert_eq!(top_dict.len(), top_dict_len);
        let top_dicts = index(&[&top_dict]);
        assert_eq!(top_dicts.len(), dict_index_len);

        let mut out = vec![1, 0, 4, 1];
        out.extend_from_slice(&names);
        out.extend_from_slice(&top_dicts);
        out.extend_from_slice(&string_index);
        out.extend_from_slice(&global_subrs);
        out.extend_from_slice(&charstring_index);
        assert_eq!(out.len(), charset_at);
        out.extend_from_slice(charset);
        out.extend_from_slice(encoding);
        out
    }

    fn font_with_charstrings_and_charset(charstrings: &[&[u8]], charset: &[u8]) -> Vec<u8> {
        let mut data = font_with_tables(charstrings, &[], charset, &[]);
        let encoding_operator = data
            .windows(6)
            .position(|window| window[0] == 29 && window[5] == 16)
            .expect("Encoding operand");
        data[encoding_operator + 1..encoding_operator + 5].copy_from_slice(&0_u32.to_be_bytes());
        data
    }

    const SQUARE: &[u8] = &[139, 139, 21, 140, 139, 5, 139, 140, 5, 14];

    fn cid_font_with_charset(charstrings: &[&[u8]], cids: &[u16]) -> Vec<u8> {
        fn index(items: &[&[u8]]) -> Vec<u8> {
            if items.is_empty() {
                return vec![0, 0];
            }
            let mut out = Vec::new();
            out.extend_from_slice(&u16::try_from(items.len()).expect("count").to_be_bytes());
            out.push(1);
            let mut offset = 1_u8;
            out.push(offset);
            for item in items {
                offset += u8::try_from(item.len()).expect("item length");
                out.push(offset);
            }
            for item in items {
                out.extend_from_slice(item);
            }
            out
        }
        fn number(value: usize) -> Vec<u8> {
            let mut out = vec![29];
            out.extend_from_slice(&u32::try_from(value).expect("operand").to_be_bytes());
            out
        }

        let names = index(&[b"Test"]);
        let string_index = index(&[b"Adobe", b"Identity"]);
        let global_subrs = index(&[]);
        let charstring_index = index(charstrings);

        let ros_len = 3 * 5 + 2;
        let top_dict_len = ros_len + 6 + 6;
        let dict_index_len = 2 + 1 + 2 + top_dict_len;
        let before_charstrings =
            4 + names.len() + dict_index_len + string_index.len() + global_subrs.len();
        let charset_at = before_charstrings + charstring_index.len();

        let mut top_dict = number(391);
        top_dict.extend_from_slice(&number(392));
        top_dict.extend_from_slice(&number(0));
        top_dict.extend_from_slice(&[12, 30]);
        assert_eq!(top_dict.len(), ros_len);
        top_dict.extend_from_slice(&number(before_charstrings));
        top_dict.push(17);
        top_dict.extend_from_slice(&number(charset_at));
        top_dict.push(15);
        assert_eq!(top_dict.len(), top_dict_len);
        let top_dicts = index(&[&top_dict]);
        assert_eq!(top_dicts.len(), dict_index_len);

        let mut out = vec![1, 0, 4, 1];
        out.extend_from_slice(&names);
        out.extend_from_slice(&top_dicts);
        out.extend_from_slice(&string_index);
        out.extend_from_slice(&global_subrs);
        out.extend_from_slice(&charstring_index);
        assert_eq!(out.len(), charset_at);
        out.push(0);
        for cid in cids {
            out.extend_from_slice(&cid.to_be_bytes());
        }
        out
    }

    #[test]
    fn a_cid_keyed_charset_maps_cids_to_glyphs_that_are_not_the_cids() {
        let data = cid_font_with_charset(&[SQUARE, SQUARE, SQUARE, SQUARE], &[302, 17, 65_535]);
        let font = CffFont::parse(&data).expect("a CID-keyed CFF");

        assert!(
            font.is_cid(),
            "the ROS operator did not make this CID-keyed"
        );
        assert_eq!(font.glyph_for_cid(302), Some(1));
        assert_eq!(font.glyph_for_cid(17), Some(2));
        assert_eq!(font.glyph_for_cid(65_535), Some(3));
    }

    #[test]
    fn a_cid_the_subset_does_not_carry_is_absent_rather_than_guessed() {
        let data = cid_font_with_charset(&[SQUARE, SQUARE], &[302]);
        let font = CffFont::parse(&data).expect("a CID-keyed CFF");

        assert_eq!(font.glyph_for_cid(302), Some(1));
        assert_eq!(font.glyph_for_cid(1), None, "a CID it does not hold");
        assert_eq!(font.glyph_for_cid(303), None, "one past the only CID");
    }

    #[test]
    fn a_font_without_a_ros_operator_answers_no_cid() {
        let data = font_with_charstrings_and_charset(&[SQUARE, SQUARE], &[0, 0, 34]);
        let font = CffFont::parse(&data).expect("a name-keyed CFF");

        assert!(!font.is_cid());
        assert_eq!(font.glyph_for_cid(1), None);
        assert_eq!(font.glyph_for_cid(302), None);
    }

    #[test]
    fn a_charset_names_glyphs_through_standard_and_custom_strings() {
        let charset = &[0, 0, 34, 1, 135][..];
        let data = font_with_tables(&[&[14], SQUARE, SQUARE], &[b"uniABCD"], charset, &[0, 0]);
        let font = CffFont::parse(&data).expect("valid CFF");
        assert_eq!(font.glyph_count(), 3);
        assert_eq!(font.glyph_for_name(b"A"), Some(1));
        assert_eq!(font.glyph_for_name(b"uniABCD"), Some(2));
        assert_eq!(font.glyph_for_name(b".notdef"), Some(0));
        assert_eq!(font.glyph_for_name(b"B"), None);
        assert_eq!(font.glyph_for_code(b'A'), None);
    }

    #[test]
    fn a_ranged_charset_expands_the_same_way_in_both_formats() {
        for charset in [&[1, 0, 34, 2][..], &[2, 0, 34, 0, 2][..]] {
            let data = font_with_tables(&[&[14], SQUARE, SQUARE, SQUARE], &[], charset, &[0, 0]);
            let font = CffFont::parse(&data).expect("valid CFF");
            assert_eq!(font.glyph_for_name(b"A"), Some(1));
            assert_eq!(font.glyph_for_name(b"B"), Some(2));
            assert_eq!(font.glyph_for_name(b"C"), Some(3));
        }
    }

    #[test]
    fn the_built_in_encoding_reaches_glyphs_by_code() {
        let charset = &[0, 0, 34, 0, 66][..];

        let data = font_with_tables(&[&[14], SQUARE, SQUARE], &[], charset, &[0, 2, 0x41, 0x7a]);
        let font = CffFont::parse(&data).expect("valid CFF");
        assert_eq!(font.glyph_for_code(0x41), Some(1));
        assert_eq!(font.glyph_for_code(0x7a), Some(2));
        assert_eq!(font.glyph_for_code(0x61), None);

        let data = font_with_tables(&[&[14], SQUARE, SQUARE], &[], charset, &[1, 1, 0x41, 1]);
        let font = CffFont::parse(&data).expect("valid CFF");
        assert_eq!(font.glyph_for_code(0x41), Some(1));
        assert_eq!(font.glyph_for_code(0x42), Some(2));

        let data = font_with_tables(
            &[&[14], SQUARE, SQUARE],
            &[],
            charset,
            &[0x80, 1, 0x41, 1, 0x61, 0, 34],
        );
        let font = CffFont::parse(&data).expect("valid CFF");
        assert_eq!(font.glyph_for_code(0x41), Some(1));
        assert_eq!(font.glyph_for_code(0x61), Some(1), "supplement maps a to A");
    }

    #[test]
    fn an_absent_encoding_is_the_standard_one_resolved_through_the_charset() {
        let data = font_with_charstrings_and_charset(&[&[14], SQUARE, SQUARE], &[0, 0, 66, 0, 34]);
        let font = CffFont::parse(&data).expect("valid CFF");
        assert_eq!(font.glyph_for_name(b"a"), Some(1));
        assert_eq!(font.glyph_for_name(b"A"), Some(2));
        assert_eq!(font.glyph_for_code(b'A'), Some(2));
        assert_eq!(font.glyph_for_code(b'a'), Some(1));
        assert_eq!(font.glyph_for_code(0x80), None);
    }

    #[test]
    #[ignore = "prints a fixture rather than checking anything"]
    fn print_precedence_fixture() {
        let data = font_with_tables(
            &[&[14], SQUARE, SQUARE],
            &[b"alpha"],
            &[0, 0, 34, 1, 135],
            &[0, 1, 0x41],
        );
        println!(
            "{}",
            data.iter().fold(String::new(), |mut text, byte| {
                use std::fmt::Write as _;
                let _ = write!(text, "{byte:02x}");
                text
            })
        );
    }

    #[test]
    fn runs_a_charstring_into_lines() {
        let charstring: &[u8] = &[139, 139, 21, 239, 139, 5, 139, 239, 5, 14];
        let data = font_with_charstrings(&[&[14], charstring]);
        let font = CffFont::parse(&data).expect("valid CFF");
        assert_eq!(font.glyph_count(), 2);
        assert_eq!(font.units_per_em(), 1000);
        let path = font.outline(&data, 1).expect("glyph 1");
        assert_eq!(
            path.segments,
            vec![
                GlyphSegment::MoveTo { x: 0.0, y: 0.0 },
                GlyphSegment::LineTo { x: 100.0, y: 0.0 },
                GlyphSegment::LineTo { x: 100.0, y: 100.0 },
                GlyphSegment::Close,
            ]
        );
    }

    #[test]
    fn runs_a_cubic_and_leaves_an_empty_glyph_empty() {
        let charstring: &[u8] = &[139, 139, 21, 149, 139, 149, 149, 139, 149, 8, 14];
        let data = font_with_charstrings(&[&[14], charstring]);
        let font = CffFont::parse(&data).expect("valid CFF");
        let path = font.outline(&data, 1).expect("glyph 1");
        assert_eq!(
            path.segments,
            vec![
                GlyphSegment::MoveTo { x: 0.0, y: 0.0 },
                GlyphSegment::CurveTo {
                    x1: 10.0,
                    y1: 0.0,
                    x2: 20.0,
                    y2: 10.0,
                    x: 20.0,
                    y: 20.0,
                },
                GlyphSegment::Close,
            ]
        );
        assert!(font.outline(&data, 0).expect("glyph 0").is_empty());
        assert!(font.outline(&data, 2).is_none(), "out of range glyph");
    }

    #[test]
    fn malformed_programs_fail_closed() {
        assert_eq!(CffFont::parse(&[]).unwrap_err(), CffError::Malformed);
        assert_eq!(
            CffFont::parse(b"not a font program at all").unwrap_err(),
            CffError::Malformed
        );
        let data = font_with_charstrings(&[&[14]]);
        assert_eq!(
            CffFont::parse(&data[..data.len() - 2]).unwrap_err(),
            CffError::Malformed
        );
    }
}
