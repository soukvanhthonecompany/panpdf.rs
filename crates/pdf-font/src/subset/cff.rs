use std::collections::{BTreeMap, BTreeSet};

use super::SubsetError;
use crate::cff::CffFont;

const TABLE: [u8; 4] = *b"CFF ";

fn malformed() -> SubsetError {
    SubsetError::MalformedTable(TABLE)
}

type Private = (Vec<u8>, Vec<u8>);

pub fn subset_cff(
    data: &[u8],
    glyphs: &BTreeSet<u16>,
) -> Result<(Vec<u8>, BTreeSet<u16>), SubsetError> {
    let font = CffFont::parse(data).map_err(|_| malformed())?;
    let count = font.glyph_count();
    let mut kept = BTreeSet::from([0_u16]);
    kept.extend(glyphs.iter().copied());
    let mut global = BTreeSet::new();
    let mut local: BTreeMap<Option<usize>, BTreeSet<usize>> = BTreeMap::new();
    for &glyph in &kept {
        if usize::from(glyph) >= count {
            return Err(SubsetError::NoSuchGlyph(glyph));
        }
        let (global_calls, local_calls, fd) = font
            .calls(data, glyph)
            .ok_or(SubsetError::MalformedGlyph(glyph))?;
        global.extend(global_calls);
        local.entry(fd).or_default().extend(local_calls);
    }

    let header_size = usize::from(*data.get(2).ok_or_else(malformed)?);
    let names = Index::read(data, header_size)?;
    let tops = Index::read(data, names.end)?;
    let strings = Index::read(data, tops.end)?;
    let globals = Index::read(data, strings.end)?;
    let top = Dict::read(tops.item(data, 0).ok_or_else(malformed)?)?;
    let charstrings = Index::read(data, top.offset(&[17]).ok_or_else(malformed)?)?;

    let mut fonts = Vec::new();
    if let Some(at) = top.offset(&[12, 36]) {
        let array = Index::read(data, at)?;
        for fd in 0..array.count {
            let dict = Dict::read(array.item(data, fd).ok_or_else(malformed)?)?;
            let private = private_of(data, &dict, local.get(&Some(fd)))?;
            fonts.push((dict, private));
        }
    }
    let top_private = if top.has(&[12, 36]) {
        None
    } else {
        private_of(data, &top, local.get(&None))?
    };
    let out = assemble(Parts {
        header: data.get(..header_size).ok_or_else(malformed)?,
        names: data.get(header_size..names.end).ok_or_else(malformed)?,
        strings: data.get(tops.end..strings.end).ok_or_else(malformed)?,
        globals: globals.rewrite(data, |index| global.contains(&index), &[11]),
        charset: copied(data, &top, &[15], count, charset_length)?,
        encoding: copied(data, &top, &[16], count, encoding_length)?,
        fd_select: copied(data, &top, &[12, 37], count, fd_select_length)?,
        charstrings: charstrings.rewrite(
            data,
            |index| u16::try_from(index).is_ok_and(|glyph| kept.contains(&glyph)),
            &[14],
        ),
        top,
        fonts,
        top_private,
    })?;
    Ok((out, kept))
}

fn copied(
    data: &[u8],
    top: &Dict,
    operator: &[u8],
    count: usize,
    length: fn(&[u8], usize, usize) -> Option<usize>,
) -> Result<Option<Vec<u8>>, SubsetError> {
    match top.offset(operator) {
        Some(at) if at > 2 => {
            let size = length(data, at, count).ok_or_else(malformed)?;
            Ok(Some(
                data.get(at..at + size).ok_or_else(malformed)?.to_vec(),
            ))
        }
        _ => Ok(None),
    }
}

fn private_of(
    data: &[u8],
    dict: &Dict,
    used: Option<&BTreeSet<usize>>,
) -> Result<Option<Private>, SubsetError> {
    let Some(values) = dict.operands(&[18]) else {
        return Ok(None);
    };
    let [size, at] = values.as_slice() else {
        return Err(malformed());
    };
    let (size, at) = (whole(*size)?, whole(*at)?);
    let body = data.get(at..at + size).ok_or_else(malformed)?;
    let mut written = Dict::read(body)?;
    let subrs = match written.offset(&[19]) {
        Some(offset) => {
            let subrs = Index::read(data, at + offset)?;
            let rewritten = subrs.rewrite(
                data,
                |index| used.is_some_and(|used| used.contains(&index)),
                &[11],
            );
            written.set_offset(&[19], 0);
            let size = written.encoded_len();
            written.set_offset(&[19], size);
            rewritten
        }
        None => Vec::new(),
    };
    Ok(Some((written.encode(), subrs)))
}

struct Parts<'a> {
    header: &'a [u8],
    names: &'a [u8],
    strings: &'a [u8],
    globals: Vec<u8>,
    charset: Option<Vec<u8>>,
    encoding: Option<Vec<u8>>,
    fd_select: Option<Vec<u8>>,
    charstrings: Vec<u8>,
    top: Dict,
    fonts: Vec<(Dict, Option<Private>)>,
    top_private: Option<Private>,
}

fn assemble(parts: Parts<'_>) -> Result<Vec<u8>, SubsetError> {
    let Parts {
        header,
        names,
        strings,
        globals,
        charset,
        encoding,
        fd_select,
        charstrings,
        mut top,
        mut fonts,
        top_private,
    } = parts;
    for operator in [&[15][..], &[16], &[17], &[12, 36], &[12, 37]] {
        if top.has(operator) {
            top.set_offset(operator, 0);
        }
    }
    if top_private.is_some() {
        top.set_private(0, 0);
    }
    for (dict, private) in &mut fonts {
        if private.is_some() {
            dict.set_private(0, 0);
        }
    }
    let top_len = index(&[&top.encode()]).len();
    let mut at = header.len() + names.len() + top_len + strings.len() + globals.len();
    let mut place = |length: usize| {
        let here = at;
        at += length;
        here
    };
    let charset_at = charset.as_ref().map(|block| place(block.len()));
    let encoding_at = encoding.as_ref().map(|block| place(block.len()));
    let fd_select_at = fd_select.as_ref().map(|block| place(block.len()));
    let charstrings_at = place(charstrings.len());
    let fd_dicts: Vec<Vec<u8>> = fonts.iter().map(|(dict, _)| dict.encode()).collect();
    let fd_array_at = (!fonts.is_empty())
        .then(|| place(index(&fd_dicts.iter().map(Vec::as_slice).collect::<Vec<_>>()).len()));
    let mut privates = Vec::new();
    for (dict, private) in &mut fonts {
        if let Some((body, subrs)) = private {
            let here = place(body.len() + subrs.len());
            dict.set_private(body.len(), here);
            privates.push((body.clone(), subrs.clone()));
        }
    }
    if let Some((body, subrs)) = &top_private {
        let here = place(body.len() + subrs.len());
        top.set_private(body.len(), here);
        privates.push((body.clone(), subrs.clone()));
    }
    for (operator, offset) in [
        (&[15][..], charset_at),
        (&[16], encoding_at),
        (&[12, 37], fd_select_at),
        (&[12, 36], fd_array_at),
        (&[17], Some(charstrings_at)),
    ] {
        if let Some(offset) = offset {
            top.set_offset(operator, offset);
        }
    }

    let mut out = Vec::with_capacity(at);
    out.extend_from_slice(header);
    out.extend_from_slice(names);
    out.extend_from_slice(&index(&[&top.encode()]));
    out.extend_from_slice(strings);
    out.extend_from_slice(&globals);
    for block in [&charset, &encoding, &fd_select].into_iter().flatten() {
        out.extend_from_slice(block);
    }
    out.extend_from_slice(&charstrings);
    if !fonts.is_empty() {
        let dicts: Vec<Vec<u8>> = fonts.iter().map(|(dict, _)| dict.encode()).collect();
        out.extend_from_slice(&index(&dicts.iter().map(Vec::as_slice).collect::<Vec<_>>()));
    }
    for (body, subrs) in privates {
        out.extend_from_slice(&body);
        out.extend_from_slice(&subrs);
    }
    if out.len() != at {
        return Err(malformed());
    }
    Ok(out)
}

fn whole(value: f64) -> Result<usize, SubsetError> {
    if value.is_finite() && value >= 0.0 && value.fract() == 0.0 && value <= f64::from(u32::MAX) {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "checked"
        )]
        Ok(value as usize)
    } else {
        Err(malformed())
    }
}

struct Index {
    count: usize,
    offset_size: usize,
    offsets_at: usize,
    data_at: usize,
    end: usize,
}

impl Index {
    fn read(data: &[u8], at: usize) -> Result<Self, SubsetError> {
        let count = usize::from(u16::from_be_bytes([
            *data.get(at).ok_or_else(malformed)?,
            *data.get(at + 1).ok_or_else(malformed)?,
        ]));
        if count == 0 {
            return Ok(Self {
                count,
                offset_size: 1,
                offsets_at: at + 2,
                data_at: at + 2,
                end: at + 2,
            });
        }
        let offset_size = usize::from(*data.get(at + 2).ok_or_else(malformed)?);
        if !(1..=4).contains(&offset_size) {
            return Err(malformed());
        }
        let offsets_at = at + 3;
        let data_at = offsets_at + (count + 1) * offset_size - 1;
        let mut index = Self {
            count,
            offset_size,
            offsets_at,
            data_at,
            end: 0,
        };
        index.end = data_at + index.offset(data, count).ok_or_else(malformed)?;
        if index.end > data.len() {
            return Err(malformed());
        }
        Ok(index)
    }

    fn offset(&self, data: &[u8], item: usize) -> Option<usize> {
        let at = self.offsets_at + item * self.offset_size;
        Some(
            data.get(at..at + self.offset_size)?
                .iter()
                .fold(0, |value, byte| value * 256 + usize::from(*byte)),
        )
    }

    fn item<'a>(&self, data: &'a [u8], item: usize) -> Option<&'a [u8]> {
        if item >= self.count {
            return None;
        }
        let start = self.data_at + self.offset(data, item)?;
        let end = self.data_at + self.offset(data, item + 1)?;
        data.get(start..end)
    }

    fn rewrite(&self, data: &[u8], keep: impl Fn(usize) -> bool, empty: &[u8]) -> Vec<u8> {
        let items: Vec<&[u8]> = (0..self.count)
            .map(|item| {
                if keep(item) {
                    self.item(data, item).unwrap_or(empty)
                } else {
                    empty
                }
            })
            .collect();
        index(&items)
    }
}

fn index(items: &[&[u8]]) -> Vec<u8> {
    let count = u16::try_from(items.len()).unwrap_or(u16::MAX);
    let mut out = count.to_be_bytes().to_vec();
    if items.is_empty() {
        return out;
    }
    let total: usize = items.iter().map(|item| item.len()).sum::<usize>() + 1;
    let size = match total {
        0..=0xFF => 1,
        0x100..=0xFFFF => 2,
        0x1_0000..=0xFF_FFFF => 3,
        _ => 4,
    };
    out.push(u8::try_from(size).unwrap_or(4));
    let mut offset = 1_usize;
    let write = |out: &mut Vec<u8>, value: usize| {
        let bytes = value.to_be_bytes();
        out.extend_from_slice(&bytes[bytes.len() - size..]);
    };
    write(&mut out, offset);
    for item in items {
        offset += item.len();
        write(&mut out, offset);
    }
    for item in items {
        out.extend_from_slice(item);
    }
    out
}

#[derive(Clone)]
struct Dict {
    entries: Vec<(Vec<u8>, Vec<u8>, Vec<f64>)>,
}

impl Dict {
    fn read(data: &[u8]) -> Result<Self, SubsetError> {
        let mut entries = Vec::new();
        let mut start = 0;
        let mut operands = Vec::new();
        let mut cursor = 0;
        while cursor < data.len() {
            let byte = data[cursor];
            let value = |at: usize| data.get(at).copied().ok_or_else(malformed);
            match byte {
                0..=21 => {
                    let operator = if byte == 12 {
                        cursor += 2;
                        vec![12, value(cursor - 1)?]
                    } else {
                        cursor += 1;
                        vec![byte]
                    };
                    let length = cursor - operator.len();
                    entries.push((
                        operator,
                        data[start..length].to_vec(),
                        std::mem::take(&mut operands),
                    ));
                    start = cursor;
                }
                28 => {
                    operands.push(f64::from(i16::from_be_bytes([
                        value(cursor + 1)?,
                        value(cursor + 2)?,
                    ])));
                    cursor += 3;
                }
                29 => {
                    operands.push(f64::from(i32::from_be_bytes([
                        value(cursor + 1)?,
                        value(cursor + 2)?,
                        value(cursor + 3)?,
                        value(cursor + 4)?,
                    ])));
                    cursor += 5;
                }
                30 => {
                    cursor += 1;
                    loop {
                        let packed = value(cursor)?;
                        cursor += 1;
                        if packed & 0x0F == 0x0F || packed >> 4 == 0x0F {
                            break;
                        }
                    }
                    operands.push(f64::NAN);
                }
                32..=246 => {
                    operands.push(f64::from(i32::from(byte) - 139));
                    cursor += 1;
                }
                247..=250 => {
                    operands.push(f64::from(
                        (i32::from(byte) - 247) * 256 + i32::from(value(cursor + 1)?) + 108,
                    ));
                    cursor += 2;
                }
                251..=254 => {
                    operands.push(f64::from(
                        -(i32::from(byte) - 251) * 256 - i32::from(value(cursor + 1)?) - 108,
                    ));
                    cursor += 2;
                }
                _ => return Err(malformed()),
            }
        }
        Ok(Self { entries })
    }

    fn operands(&self, operator: &[u8]) -> Option<&Vec<f64>> {
        self.entries
            .iter()
            .find(|(key, _, _)| key == operator)
            .map(|(_, _, values)| values)
    }

    fn has(&self, operator: &[u8]) -> bool {
        self.operands(operator).is_some()
    }

    fn offset(&self, operator: &[u8]) -> Option<usize> {
        whole(*self.operands(operator)?.last()?).ok()
    }

    fn set(&mut self, operator: &[u8], values: &[usize]) {
        let mut bytes = Vec::with_capacity(values.len() * 5);
        for value in values {
            bytes.push(29);
            bytes.extend_from_slice(&u32::try_from(*value).unwrap_or(u32::MAX).to_be_bytes());
        }
        let floats = values
            .iter()
            .map(|value| f64::from(u32::try_from(*value).unwrap_or(u32::MAX)))
            .collect();
        match self.entries.iter_mut().find(|(key, _, _)| key == operator) {
            Some(entry) => {
                entry.1 = bytes;
                entry.2 = floats;
            }
            None => self.entries.push((operator.to_vec(), bytes, floats)),
        }
    }

    fn set_offset(&mut self, operator: &[u8], value: usize) {
        self.set(operator, &[value]);
    }

    fn set_private(&mut self, size: usize, at: usize) {
        self.set(&[18], &[size, at]);
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (operator, operands, _) in &self.entries {
            out.extend_from_slice(operands);
            out.extend_from_slice(operator);
        }
        out
    }

    fn encoded_len(&self) -> usize {
        self.encode().len()
    }
}

fn charset_length(data: &[u8], at: usize, glyphs: usize) -> Option<usize> {
    let covered = glyphs.checked_sub(1)?;
    match data.get(at)? {
        0 => Some(1 + covered * 2),
        format @ (1 | 2) => {
            let step = if *format == 1 { 3 } else { 4 };
            let mut cursor = at + 1;
            let mut seen = 0;
            while seen < covered {
                let left = if *format == 1 {
                    usize::from(*data.get(cursor + 2)?)
                } else {
                    usize::from(u16::from_be_bytes([
                        *data.get(cursor + 2)?,
                        *data.get(cursor + 3)?,
                    ]))
                };
                seen += left + 1;
                cursor += step;
            }
            Some(cursor - at)
        }
        _ => None,
    }
}

fn encoding_length(data: &[u8], at: usize, _: usize) -> Option<usize> {
    let format = *data.get(at)?;
    let count = usize::from(*data.get(at + 1)?);
    let mut length = match format & 0x7F {
        0 => 2 + count,
        1 => 2 + count * 2,
        _ => return None,
    };
    if format & 0x80 != 0 {
        let supplements = usize::from(*data.get(at + length)?);
        length += 1 + supplements * 3;
    }
    Some(length)
}

fn fd_select_length(data: &[u8], at: usize, glyphs: usize) -> Option<usize> {
    match data.get(at)? {
        0 => Some(1 + glyphs),
        3 => {
            let ranges = usize::from(u16::from_be_bytes([*data.get(at + 1)?, *data.get(at + 2)?]));
            Some(5 + ranges * 3)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::subset_cff;
    use crate::cff::CffFont;
    use crate::glyph::GlyphPath;

    fn index(items: &[&[u8]]) -> Vec<u8> {
        super::index(items)
    }

    fn int(value: usize) -> Vec<u8> {
        let mut out = vec![29];
        out.extend_from_slice(&u32::try_from(value).expect("offset").to_be_bytes());
        out
    }

    fn font() -> Vec<u8> {
        let square = |side: u8| vec![139, 139, 21, 139 + side, 139, 5, 139, 139 + side, 5, 11];
        let globals = index(&[&square(100)]);
        let charstrings = index(&[&[14], &[32, 29, 14], &[32, 10, 14]]);
        let local = index(&[&square(50), &[11]]);
        let charset = [0, 0, 10, 0, 20];
        let fd_select = [3, 0, 1, 0, 0, 0, 0, 3];
        let names = index(&[b"Test"]);
        let strings = index(&[b"Adobe", b"Identity"]);
        let private_len = int(0).len() + 1;
        let fd = |private_at: usize| {
            let mut dict = int(private_len);
            dict.extend(int(private_at));
            dict.push(18);
            dict
        };
        let top =
            |charset_at: usize, fd_select_at: usize, charstrings_at: usize, fd_array_at: usize| {
                let mut dict = vec![0xF8, 0x1B, 0xF8, 0x1C, 139, 12, 30];
                dict.extend(int(charset_at));
                dict.push(15);
                dict.extend(int(fd_select_at));
                dict.extend([12, 37]);
                dict.extend(int(charstrings_at));
                dict.push(17);
                dict.extend(int(fd_array_at));
                dict.extend([12, 36]);
                dict
            };
        let header = [1, 0, 4, 4];
        let top_len = index(&[&top(0, 0, 0, 0)]).len();
        let fd_array_len = index(&[&fd(0)]).len();
        let charset_at = header.len() + names.len() + top_len + strings.len() + globals.len();
        let fd_select_at = charset_at + charset.len();
        let charstrings_at = fd_select_at + fd_select.len();
        let fd_array_at = charstrings_at + charstrings.len();
        let private_at = fd_array_at + fd_array_len;
        let mut private = int(private_len);
        private.push(19);
        let mut out = header.to_vec();
        out.extend(names);
        out.extend(index(&[&top(
            charset_at,
            fd_select_at,
            charstrings_at,
            fd_array_at,
        )]));
        out.extend(strings);
        out.extend(globals);
        out.extend(charset);
        out.extend(fd_select);
        out.extend(charstrings);
        out.extend(index(&[&fd(private_at)]));
        out.extend(private);
        out.extend(local);
        out
    }

    #[test]
    fn a_cff_subset_keeps_the_glyphs_their_subroutines_their_cids_and_nothing_else() {
        let data = font();
        let original = CffFont::parse(&data).expect("the fixture parses");
        let square = original.outline(&data, 1).expect("glyph 1 draws");
        assert_ne!(square, GlyphPath::default());
        assert_ne!(original.outline(&data, 2), Some(GlyphPath::default()));

        let (program, kept) = subset_cff(&data, &BTreeSet::from([1])).expect("subset");
        assert_eq!(kept, BTreeSet::from([0, 1]));
        let subset = CffFont::parse(&program).expect("the subset parses");
        assert_eq!(subset.glyph_count(), 3);
        assert_eq!(subset.outline(&program, 1), Some(square));
        assert_eq!(subset.outline(&program, 2), Some(GlyphPath::default()));
        assert_eq!(
            [1, 2].map(|glyph| subset.cid_for_glyph(glyph)),
            [Some(10), Some(20)]
        );
        assert!(program.len() < data.len());

        let (program, _) = subset_cff(&data, &BTreeSet::from([2])).expect("subset");
        let subset = CffFont::parse(&program).expect("the subset parses");
        assert_eq!(subset.outline(&program, 2), original.outline(&data, 2));
        assert_eq!(subset.outline(&program, 1), Some(GlyphPath::default()));
    }

    #[test]
    fn a_glyph_the_cff_lacks_is_refused() {
        assert_eq!(
            subset_cff(&font(), &BTreeSet::from([3])),
            Err(super::SubsetError::NoSuchGlyph(3))
        );
    }
}
