use std::collections::HashMap;

use crate::glyph::{GlyphPath, PathCache};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutlinePoint {
    pub x: f64,
    pub y: f64,
    pub on_curve: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlyphOutline {
    pub contours: Vec<Vec<OutlinePoint>>,
}

impl GlyphOutline {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.contours.iter().all(|contour| contour.len() < 2)
    }
}

const UNICODE_SUBTABLES: [(u16, u16); 6] = [(3, 1), (3, 10), (0, 3), (0, 4), (0, 6), (0, 0)];

const CHARACTER_BUDGET: u32 = 1 << 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CmapSubtable {
    WindowsSymbol,
    MacintoshRoman,
    WindowsUnicode,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrueTypeFont {
    data: Vec<u8>,
    tables: HashMap<[u8; 4], (usize, usize)>,
    units_per_em: u16,
    long_loca: bool,
    glyph_count: u16,
    paths: PathCache,
}

const MAX_COMPONENT_DEPTH: usize = 8;

impl TrueTypeFont {
    #[must_use]
    pub fn program_bytes(&self) -> &[u8] {
        &self.data
    }

    pub fn parse(data: Vec<u8>) -> Result<Self, TrueTypeError> {
        Self::parse_face(data, 0)
    }

    pub fn face_count(data: &[u8]) -> Result<u32, TrueTypeError> {
        let tag = read_u32(data, 0).ok_or(TrueTypeError::NotSfnt)?;
        match tag {
            0x0001_0000 | 0x7472_7565 | 0x4f54_544f => Ok(1),
            0x7474_6366 => read_u32(data, 8).ok_or(TrueTypeError::NotSfnt),
            _ => Err(TrueTypeError::NotSfnt),
        }
    }

    pub fn parse_face(data: Vec<u8>, face: u32) -> Result<Self, TrueTypeError> {
        let tag = read_u32(&data, 0).ok_or(TrueTypeError::NotSfnt)?;
        let offset = match tag {
            0x0001_0000 | 0x7472_7565 | 0x4f54_544f => {
                if face != 0 {
                    return Err(TrueTypeError::NoSuchFace);
                }
                0
            }
            0x7474_6366 => {
                let count = read_u32(&data, 8).ok_or(TrueTypeError::NotSfnt)?;
                if face >= count {
                    return Err(TrueTypeError::NoSuchFace);
                }
                let record = 12 + usize::try_from(face).map_err(|_| TrueTypeError::NoSuchFace)? * 4;
                usize::try_from(read_u32(&data, record).ok_or(TrueTypeError::NotSfnt)?)
                    .map_err(|_| TrueTypeError::NotSfnt)?
            }
            _ => return Err(TrueTypeError::NotSfnt),
        };
        let table_count = read_u16(&data, offset + 4).ok_or(TrueTypeError::NotSfnt)?;
        let mut tables = HashMap::new();
        for index in 0..usize::from(table_count) {
            let record = offset + 12 + index * 16;
            let mut tag = [0_u8; 4];
            tag.copy_from_slice(data.get(record..record + 4).ok_or(TrueTypeError::NotSfnt)?);
            let start = read_u32(&data, record + 8).ok_or(TrueTypeError::NotSfnt)? as usize;
            let length = read_u32(&data, record + 12).ok_or(TrueTypeError::NotSfnt)? as usize;
            if start.checked_add(length).is_none_or(|end| end > data.len()) {
                return Err(TrueTypeError::TableOutOfRange);
            }
            tables.insert(tag, (start, length));
        }
        let (head, _) = *tables.get(b"head").ok_or(TrueTypeError::MissingTable)?;
        let units_per_em = read_u16(&data, head + 18).ok_or(TrueTypeError::MissingTable)?;
        if units_per_em == 0 {
            return Err(TrueTypeError::InvalidHead);
        }
        let long_loca = match read_u16(&data, head + 50) {
            Some(0) => false,
            Some(1) => true,
            _ => return Err(TrueTypeError::InvalidHead),
        };
        let (maxp, _) = *tables.get(b"maxp").ok_or(TrueTypeError::MissingTable)?;
        let glyph_count = read_u16(&data, maxp + 4).ok_or(TrueTypeError::MissingTable)?;
        Ok(Self {
            data,
            tables,
            units_per_em,
            long_loca,
            glyph_count,
            paths: PathCache::default(),
        })
    }

    #[must_use]
    pub const fn units_per_em(&self) -> u16 {
        self.units_per_em
    }

    #[must_use]
    pub const fn glyph_count(&self) -> u16 {
        self.glyph_count
    }

    #[must_use]
    pub fn has_outlines(&self) -> bool {
        self.tables.contains_key(b"loca") && self.tables.contains_key(b"glyf")
    }

    #[must_use]
    pub fn cff_table(&self) -> Option<&[u8]> {
        let (start, length) = *self.tables.get(b"CFF ")?;
        self.data.get(start..start + length)
    }

    #[must_use]
    pub fn path(&self, glyph: u16) -> Option<GlyphPath> {
        self.paths.get_or(glyph, || self.decode_path(glyph))
    }

    fn decode_path(&self, glyph: u16) -> Option<GlyphPath> {
        let outline = self.outline(glyph)?;
        let mut path = GlyphPath::default();
        for contour in &outline.contours {
            if contour.len() < 2 {
                continue;
            }
            let rotate = contour.iter().position(|point| point.on_curve);
            let start = rotate.map_or_else(
                || {
                    let last = contour[contour.len() - 1];
                    let first = contour[0];
                    (
                        f64::midpoint(last.x, first.x),
                        f64::midpoint(last.y, first.y),
                    )
                },
                |index| (contour[index].x, contour[index].y),
            );
            let offset = rotate.unwrap_or(0);
            path.move_to(start.0, start.1);
            let mut cursor = start;
            let mut pending: Option<(f64, f64)> = None;
            for step in 1..=contour.len() {
                let point = contour[(offset + step) % contour.len()];
                let position = (point.x, point.y);
                if point.on_curve {
                    match pending.take() {
                        Some(control) => {
                            path.quadratic_to(cursor, control, position);
                        }
                        None => path.line_to(position.0, position.1),
                    }
                    cursor = position;
                } else if let Some(control) = pending.replace(position) {
                    let implied = (
                        f64::midpoint(control.0, position.0),
                        f64::midpoint(control.1, position.1),
                    );
                    path.quadratic_to(cursor, control, implied);
                    cursor = implied;
                }
            }
            if let Some(control) = pending {
                path.quadratic_to(cursor, control, start);
            }
            path.close();
        }
        Some(path)
    }

    #[must_use]
    pub fn glyph_for_code(&self, code: u8) -> Option<(u16, CmapSubtable)> {
        let value = u32::from(code);
        for (platform, encoding, kind) in [
            (3_u16, 0_u16, CmapSubtable::WindowsSymbol),
            (1, 0, CmapSubtable::MacintoshRoman),
            (3, 1, CmapSubtable::WindowsUnicode),
        ] {
            let Some(table) = self.subtable(platform, encoding) else {
                continue;
            };
            for candidate in if kind == CmapSubtable::WindowsSymbol {
                vec![value, 0xF000 | value]
            } else {
                vec![value]
            } {
                if let Some(glyph) = self.lookup(table, candidate)
                    && glyph != 0
                {
                    return Some((glyph, kind));
                }
            }
        }
        None
    }

    #[must_use]
    pub fn glyph_for_name(&self, name: &[u8]) -> Option<(u16, CmapSubtable)> {
        if let Some(character) = crate::tables::unicode_for_glyph_name(name) {
            for (platform, encoding) in UNICODE_SUBTABLES {
                let Some(table) = self.subtable(platform, encoding) else {
                    continue;
                };
                if let Some(glyph) = self.lookup(table, character)
                    && glyph != 0
                {
                    return Some((glyph, CmapSubtable::WindowsUnicode));
                }
            }
            if character <= 0xFF
                && let Some(table) = self.subtable(3, 0)
            {
                for candidate in [character, 0xF000 | character] {
                    if let Some(glyph) = self.lookup(table, candidate)
                        && glyph != 0
                    {
                        return Some((glyph, CmapSubtable::WindowsSymbol));
                    }
                }
            }
        }
        let code = crate::tables::mac_roman_code_for_glyph_name(name)?;
        let table = self.subtable(1, 0)?;
        let glyph = self.lookup(table, u32::from(code))?;
        (glyph != 0).then_some((glyph, CmapSubtable::MacintoshRoman))
    }

    #[must_use]
    pub fn glyph_for_char(&self, character: char) -> Option<u16> {
        let value = u32::from(character);
        for (platform, encoding) in UNICODE_SUBTABLES {
            let Some(table) = self.subtable(platform, encoding) else {
                continue;
            };
            if let Some(glyph) = self.lookup(table, value)
                && glyph != 0
            {
                return Some(glyph);
            }
        }
        if value <= 0xFF
            && let Some(table) = self.subtable(3, 0)
        {
            for candidate in [value, 0xF000 | value] {
                if let Some(glyph) = self.lookup(table, candidate)
                    && glyph != 0
                {
                    return Some(glyph);
                }
            }
        }
        None
    }

    #[must_use]
    pub fn names(&self) -> (Option<String>, Option<String>) {
        let family = self.name_record(16).or_else(|| self.name_record(1));
        let subfamily = self.name_record(17).or_else(|| self.name_record(2));
        (family, subfamily)
    }

    fn name_record(&self, wanted: u16) -> Option<String> {
        let (name, length) = *self.tables.get(b"name")?;
        let count = read_u16(&self.data, name + 2)?;
        let storage = name + usize::from(read_u16(&self.data, name + 4)?);
        let mut best: Option<String> = None;
        for index in 0..usize::from(count) {
            let record = name + 6 + index * 12;
            if record + 12 > name + length {
                break;
            }
            let platform = read_u16(&self.data, record)?;
            if read_u16(&self.data, record + 6)? != wanted {
                continue;
            }
            let bytes = usize::from(read_u16(&self.data, record + 8)?);
            let offset = usize::from(read_u16(&self.data, record + 10)?);
            let start = storage.checked_add(offset)?;
            let text = self.data.get(start..start.checked_add(bytes)?)?;
            let decoded = match platform {
                0 | 3 => text
                    .chunks_exact(2)
                    .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                    .map(|unit| u8::try_from(unit).ok().map(char::from))
                    .collect::<Option<String>>(),
                1 => text
                    .iter()
                    .map(|byte| byte.is_ascii().then_some(char::from(*byte)))
                    .collect::<Option<String>>(),
                _ => None,
            };
            let Some(decoded) = decoded else { continue };
            if decoded.is_empty() {
                continue;
            }
            if platform == 3 {
                return Some(decoded);
            }
            best = best.or(Some(decoded));
        }
        best
    }

    #[must_use]
    pub fn postscript_name(&self) -> Option<String> {
        self.name_record(6)
    }

    #[must_use]
    pub fn font_box(&self) -> Option<[i16; 4]> {
        let head = self.table(*b"head")?;
        Some([
            read_i16(head, 36)?,
            read_i16(head, 38)?,
            read_i16(head, 40)?,
            read_i16(head, 42)?,
        ])
    }

    #[must_use]
    pub fn italic_angle(&self) -> Option<f64> {
        let post = self.table(*b"post")?;
        let whole = read_i16(post, 4)?;
        let fraction = read_u16(post, 6)?;
        Some(f64::from(whole) + f64::from(fraction) / 65536.0)
    }

    #[must_use]
    pub fn cap_height(&self) -> Option<i16> {
        let os2 = self.table(*b"OS/2")?;
        (read_u16(os2, 0)? >= 2)
            .then(|| read_i16(os2, 88))
            .flatten()
    }

    #[must_use]
    pub fn os2_style(&self) -> Option<(u16, bool, bool)> {
        let (os2, length) = *self.tables.get(b"OS/2")?;
        if length < 64 {
            return None;
        }
        let weight = read_u16(&self.data, os2 + 4)?;
        let selection = read_u16(&self.data, os2 + 62)?;
        Some((weight, selection & 0x0001 != 0, selection & 0x0020 != 0))
    }

    #[must_use]
    pub fn line_metrics(&self) -> Option<(f64, f64)> {
        let (hhea, length) = *self.tables.get(b"hhea")?;
        if length < 8 {
            return None;
        }
        let ascender = read_i16(&self.data, hhea + 4)?;
        let descender = read_i16(&self.data, hhea + 6)?;
        let units = f64::from(self.units_per_em);
        (ascender > descender).then(|| (f64::from(ascender) / units, f64::from(descender) / units))
    }

    #[must_use]
    pub fn characters(&self) -> Vec<(u16, char)> {
        let mut found = Vec::new();
        for (platform, encoding) in UNICODE_SUBTABLES {
            let Some(table) = self.subtable(platform, encoding) else {
                continue;
            };
            self.walk(table, &mut found);
        }
        found
    }

    fn walk(&self, table: usize, out: &mut Vec<(u16, char)>) {
        let mut add = |value: u32, glyph: u16| {
            if glyph != 0
                && let Some(character) = char::from_u32(value)
            {
                out.push((glyph, character));
            }
        };
        match read_u16(&self.data, table) {
            Some(0 | 6) => {
                for value in 0..=u32::from(u16::MAX) {
                    if let Some(glyph) = self.lookup(table, value) {
                        add(value, glyph);
                    }
                }
            }
            Some(4) => self.walk_format4(table, &mut add),
            Some(12) => {
                let Some(groups) = read_u32(&self.data, table + 12) else {
                    return;
                };
                let mut budget = CHARACTER_BUDGET;
                for index in 0..groups as usize {
                    let group = table + 16 + index * 12;
                    let (Some(start), Some(end), Some(first)) = (
                        read_u32(&self.data, group),
                        read_u32(&self.data, group + 4),
                        read_u32(&self.data, group + 8),
                    ) else {
                        return;
                    };
                    for value in start..=end.min(start.saturating_add(budget)) {
                        let Some(glyph) = u16::try_from(first + (value - start)).ok() else {
                            break;
                        };
                        add(value, glyph);
                        budget -= 1;
                        if budget == 0 {
                            return;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn walk_format4(&self, table: usize, add: &mut impl FnMut(u32, u16)) {
        let Some(count) = read_u16(&self.data, table + 6) else {
            return;
        };
        let segments = usize::from(count) / 2;
        let ends = table + 14;
        let starts = ends + segments * 2 + 2;
        let mut budget = CHARACTER_BUDGET;
        for index in 0..segments {
            let (Some(end), Some(start)) = (
                read_u16(&self.data, ends + index * 2),
                read_u16(&self.data, starts + index * 2),
            ) else {
                return;
            };
            for value in u32::from(start)..=u32::from(end) {
                if let Some(glyph) = self.lookup_format4(table, value) {
                    add(value, glyph);
                }
                budget -= 1;
                if budget == 0 {
                    return;
                }
            }
        }
    }

    fn subtable(&self, platform: u16, encoding: u16) -> Option<usize> {
        let (cmap, _) = *self.tables.get(b"cmap")?;
        let count = read_u16(&self.data, cmap + 2)?;
        for index in 0..usize::from(count) {
            let record = cmap + 4 + index * 8;
            if read_u16(&self.data, record)? == platform
                && read_u16(&self.data, record + 2)? == encoding
            {
                return Some(cmap + read_u32(&self.data, record + 4)? as usize);
            }
        }
        None
    }

    fn lookup(&self, table: usize, value: u32) -> Option<u16> {
        match read_u16(&self.data, table)? {
            0 => {
                let index = usize::try_from(value).ok()?;
                (value < 256)
                    .then(|| self.data.get(table + 6 + index).copied())
                    .flatten()
                    .map(u16::from)
            }
            4 => self.lookup_format4(table, value),
            6 => {
                let first = u32::from(read_u16(&self.data, table + 6)?);
                let count = u32::from(read_u16(&self.data, table + 8)?);
                let offset = value.checked_sub(first)?;
                (offset < count)
                    .then(|| read_u16(&self.data, table + 10 + (offset as usize) * 2))
                    .flatten()
            }
            12 => {
                let groups = read_u32(&self.data, table + 12)?;
                for index in 0..groups as usize {
                    let group = table + 16 + index * 12;
                    let start = read_u32(&self.data, group)?;
                    let end = read_u32(&self.data, group + 4)?;
                    if (start..=end).contains(&value) {
                        let glyph = read_u32(&self.data, group + 8)? + (value - start);
                        return u16::try_from(glyph).ok();
                    }
                }
                None
            }
            _ => None,
        }
    }

    fn lookup_format4(&self, table: usize, value: u32) -> Option<u16> {
        let value = u16::try_from(value).ok()?;
        let segments = usize::from(read_u16(&self.data, table + 6)?) / 2;
        let ends = table + 14;
        let starts = ends + segments * 2 + 2;
        let deltas = starts + segments * 2;
        let ranges = deltas + segments * 2;
        for index in 0..segments {
            let end = read_u16(&self.data, ends + index * 2)?;
            if value > end {
                continue;
            }
            let start = read_u16(&self.data, starts + index * 2)?;
            if value < start {
                return None;
            }
            let delta = read_u16(&self.data, deltas + index * 2)?;
            let range_offset = read_u16(&self.data, ranges + index * 2)?;
            if range_offset == 0 {
                return Some(value.wrapping_add(delta));
            }
            let at =
                ranges + index * 2 + usize::from(range_offset) + usize::from(value - start) * 2;
            let glyph = read_u16(&self.data, at)?;
            return Some(if glyph == 0 {
                0
            } else {
                glyph.wrapping_add(delta)
            });
        }
        None
    }

    #[must_use]
    pub fn outline(&self, glyph: u16) -> Option<GlyphOutline> {
        self.outline_at(glyph, 0)
    }

    fn outline_at(&self, glyph: u16, depth: usize) -> Option<GlyphOutline> {
        if depth > MAX_COMPONENT_DEPTH || glyph >= self.glyph_count || !self.has_outlines() {
            return None;
        }
        let (start, end) = self.glyph_range(glyph)?;
        if start == end {
            return Some(GlyphOutline::default());
        }
        let (glyf, glyf_length) = *self.tables.get(b"glyf")?;
        if end > glyf_length {
            return None;
        }
        let body = self.data.get(glyf + start..glyf + end)?;
        let contour_count = read_i16(body, 0)?;
        if contour_count >= 0 {
            simple_outline(body, usize::try_from(contour_count).ok()?)
        } else {
            self.composite_outline(body, depth)
        }
    }

    pub(crate) fn table(&self, tag: [u8; 4]) -> Option<&[u8]> {
        let (start, length) = *self.tables.get(&tag)?;
        self.data.get(start..start + length)
    }

    #[must_use]
    pub fn glyph_name(&self, glyph: u16) -> Option<&[u8]> {
        let post = self.table(*b"post")?;
        if read_u32(post, 0)? != 0x0002_0000 {
            return None;
        }
        let count = read_u16(post, 32)?;
        if glyph >= count {
            return None;
        }
        let index = read_u16(post, 34 + usize::from(glyph) * 2)?;
        let wanted = usize::from(index).checked_sub(258)?;
        let mut at = 34 + usize::from(count) * 2;
        for _ in 0..wanted {
            let length = usize::from(*post.get(at)?);
            at = at.checked_add(length + 1)?;
        }
        let length = usize::from(*post.get(at)?);
        post.get(at + 1..at + 1 + length)
    }

    #[must_use]
    pub fn advance_width(&self, glyph: u16) -> Option<u16> {
        if glyph >= self.glyph_count {
            return None;
        }
        let hhea = self.table(*b"hhea")?;
        let hmtx = self.table(*b"hmtx")?;
        let metrics = read_u16(hhea, 34)?;
        if metrics == 0 {
            return None;
        }
        read_u16(hmtx, usize::from(glyph.min(metrics - 1)) * 4)
    }

    pub(crate) fn glyph_range(&self, glyph: u16) -> Option<(usize, usize)> {
        let (loca, length) = *self.tables.get(b"loca")?;
        let index = usize::from(glyph);
        let (start, end) = if self.long_loca {
            if (index + 2) * 4 > length {
                return None;
            }
            (
                read_u32(&self.data, loca + index * 4)? as usize,
                read_u32(&self.data, loca + (index + 1) * 4)? as usize,
            )
        } else {
            if (index + 2) * 2 > length {
                return None;
            }
            (
                usize::from(read_u16(&self.data, loca + index * 2)?) * 2,
                usize::from(read_u16(&self.data, loca + (index + 1) * 2)?) * 2,
            )
        };
        (start <= end).then_some((start, end))
    }

    fn composite_outline(&self, body: &[u8], depth: usize) -> Option<GlyphOutline> {
        let mut combined = GlyphOutline::default();
        let mut cursor = 10_usize;
        loop {
            let flags = read_u16(body, cursor)?;
            let component = read_u16(body, cursor + 2)?;
            cursor += 4;
            let (dx, dy) = if flags & ARGS_ARE_WORDS == 0 {
                let first = (*body.get(cursor)?).cast_signed();
                let second = (*body.get(cursor + 1)?).cast_signed();
                cursor += 2;
                (f64::from(first), f64::from(second))
            } else {
                let first = read_i16(body, cursor)?;
                let second = read_i16(body, cursor + 2)?;
                cursor += 4;
                (f64::from(first), f64::from(second))
            };
            if flags & ARGS_ARE_XY == 0 {
                return None;
            }
            let [a, b, c, d] = if flags & HAVE_SCALE != 0 {
                let scale = read_f2dot14(body, cursor)?;
                cursor += 2;
                [scale, 0.0, 0.0, scale]
            } else if flags & HAVE_XY_SCALE != 0 {
                let x = read_f2dot14(body, cursor)?;
                let y = read_f2dot14(body, cursor + 2)?;
                cursor += 4;
                [x, 0.0, 0.0, y]
            } else if flags & HAVE_TWO_BY_TWO != 0 {
                let values = [
                    read_f2dot14(body, cursor)?,
                    read_f2dot14(body, cursor + 2)?,
                    read_f2dot14(body, cursor + 4)?,
                    read_f2dot14(body, cursor + 6)?,
                ];
                cursor += 8;
                values
            } else {
                [1.0, 0.0, 0.0, 1.0]
            };
            let part = self.outline_at(component, depth + 1)?;
            for contour in part.contours {
                combined.contours.push(
                    contour
                        .into_iter()
                        .map(|point| OutlinePoint {
                            x: a.mul_add(point.x, c * point.y) + dx,
                            y: b.mul_add(point.x, d * point.y) + dy,
                            on_curve: point.on_curve,
                        })
                        .collect(),
                );
            }
            if flags & MORE_COMPONENTS == 0 {
                break;
            }
        }
        Some(combined)
    }
}

const ARGS_ARE_WORDS: u16 = 0x0001;
const ARGS_ARE_XY: u16 = 0x0002;
const HAVE_SCALE: u16 = 0x0008;
const MORE_COMPONENTS: u16 = 0x0020;
const HAVE_XY_SCALE: u16 = 0x0040;
const HAVE_TWO_BY_TWO: u16 = 0x0080;

const ON_CURVE: u8 = 0x01;
const X_SHORT: u8 = 0x02;
const Y_SHORT: u8 = 0x04;
const REPEAT: u8 = 0x08;
const X_SAME_OR_POSITIVE: u8 = 0x10;
const Y_SAME_OR_POSITIVE: u8 = 0x20;

fn simple_outline(body: &[u8], contour_count: usize) -> Option<GlyphOutline> {
    let mut ends = Vec::with_capacity(contour_count);
    for index in 0..contour_count {
        ends.push(usize::from(read_u16(body, 10 + index * 2)?));
    }
    let point_count = ends.last().map_or(0, |last| last + 1);
    if point_count == 0 {
        return Some(GlyphOutline::default());
    }
    if ends.windows(2).any(|pair| pair[0] >= pair[1]) {
        return None;
    }
    let instruction_length = usize::from(read_u16(body, 10 + contour_count * 2)?);
    let mut cursor = 10 + contour_count * 2 + 2 + instruction_length;

    let mut flags = Vec::with_capacity(point_count);
    while flags.len() < point_count {
        let flag = *body.get(cursor)?;
        cursor += 1;
        flags.push(flag);
        if flag & REPEAT != 0 {
            let repeats = *body.get(cursor)?;
            cursor += 1;
            for _ in 0..repeats {
                if flags.len() >= point_count {
                    break;
                }
                flags.push(flag);
            }
        }
    }

    let mut xs = Vec::with_capacity(point_count);
    let mut x = 0_i32;
    for flag in &flags {
        if flag & X_SHORT != 0 {
            let delta = i32::from(*body.get(cursor)?);
            cursor += 1;
            x += if flag & X_SAME_OR_POSITIVE != 0 {
                delta
            } else {
                -delta
            };
        } else if flag & X_SAME_OR_POSITIVE == 0 {
            x += i32::from(read_i16(body, cursor)?);
            cursor += 2;
        }
        xs.push(x);
    }
    let mut ys = Vec::with_capacity(point_count);
    let mut y = 0_i32;
    for flag in &flags {
        if flag & Y_SHORT != 0 {
            let delta = i32::from(*body.get(cursor)?);
            cursor += 1;
            y += if flag & Y_SAME_OR_POSITIVE != 0 {
                delta
            } else {
                -delta
            };
        } else if flag & Y_SAME_OR_POSITIVE == 0 {
            y += i32::from(read_i16(body, cursor)?);
            cursor += 2;
        }
        ys.push(y);
    }

    let mut outline = GlyphOutline::default();
    let mut start = 0_usize;
    for end in ends {
        let mut contour = Vec::with_capacity(end + 1 - start);
        for index in start..=end {
            contour.push(OutlinePoint {
                x: f64::from(*xs.get(index)?),
                y: f64::from(*ys.get(index)?),
                on_curve: flags.get(index)? & ON_CURVE != 0,
            });
        }
        outline.contours.push(contour);
        start = end + 1;
    }
    Some(outline)
}

fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    let bytes = data.get(offset..offset + 2)?;
    Some(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_i16(data: &[u8], offset: usize) -> Option<i16> {
    read_u16(data, offset).map(u16::cast_signed)
}

fn read_u32(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset + 4)?;
    Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_f2dot14(data: &[u8], offset: usize) -> Option<f64> {
    read_i16(data, offset).map(|value| f64::from(value) / 16384.0)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrueTypeError {
    NotSfnt,
    MissingTable,
    TableOutOfRange,
    InvalidHead,
    NoSuchFace,
}

impl std::fmt::Display for TrueTypeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::NotSfnt => "font program is not a TrueType sfnt",
            Self::MissingTable => "font program is missing a required table",
            Self::TableOutOfRange => "font table extends past the font program",
            Self::InvalidHead => "font program has an invalid head table",
            Self::NoSuchFace => "font program has no face at that index",
        })
    }
}

impl std::error::Error for TrueTypeError {}

#[cfg(test)]
mod tests {
    use super::{OutlinePoint, TrueTypeError, TrueTypeFont};

    fn font_with_glyf(glyf: &[u8], loca: &[u16], glyph_count: u16) -> Vec<u8> {
        font_with_cmap(glyf, loca, glyph_count, &[])
    }

    fn post_v2(names: &[&str]) -> Vec<u8> {
        let count = u16::try_from(names.len() + 1).expect("a small font");
        let mut out = Vec::new();
        out.extend_from_slice(&0x0002_0000_u32.to_be_bytes());
        out.extend_from_slice(&[0; 28]);
        out.extend_from_slice(&count.to_be_bytes());
        out.extend_from_slice(&0_u16.to_be_bytes());
        for (step, _) in names.iter().enumerate() {
            let index = 258 + u16::try_from(step).expect("a small font");
            out.extend_from_slice(&index.to_be_bytes());
        }
        for name in names {
            out.push(u8::try_from(name.len()).expect("a short name"));
            out.extend_from_slice(name.as_bytes());
        }
        out
    }

    #[test]
    fn a_truetype_post_table_names_the_glyphs_the_font_named_itself() {
        let names = ["dotbelow", "uni0E81", "laoKo"];
        let post = post_v2(&names);
        let data = font_with_tables(&[], &[0, 0, 0, 0, 0], 4, &[], &[(b"post", &post)]);
        let font = TrueTypeFont::parse(data).expect("the font parses");
        assert_eq!(font.glyph_name(0), None, "a standard name says nothing new");
        assert_eq!(font.glyph_name(1), Some(&b"dotbelow"[..]));
        assert_eq!(font.glyph_name(2), Some(&b"uni0E81"[..]));
        assert_eq!(font.glyph_name(3), Some(&b"laoKo"[..]));
        assert_eq!(font.glyph_name(9), None, "past the table");

        let sneak = {
            let mut out = Vec::new();
            out.extend_from_slice(&0x0002_0000_u32.to_be_bytes());
            out.extend_from_slice(&[0; 28]);
            out.extend_from_slice(&2_u16.to_be_bytes());
            out.extend_from_slice(&0_u16.to_be_bytes());
            out.extend_from_slice(&258_u16.to_be_bytes());
            out.extend_from_slice(&[4, b'w', 0x01, 0x02, b't']);
            out
        };
        let sneak = font_with_tables(&[], &[0, 0, 0, 0, 0], 4, &[], &[(b"post", &sneak)]);
        let sneak = TrueTypeFont::parse(sneak).expect("the font parses");
        assert_eq!(sneak.glyph_name(1), Some(&b"w\x01\x02t"[..]));
        assert_eq!(
            sneak.glyph_name(3),
            None,
            "a glyph past the table is refused, not read on into the names"
        );

        let bare = font_with_tables(&[], &[0, 0, 0, 0, 0], 4, &[], &[]);
        let bare = TrueTypeFont::parse(bare).expect("the font parses");
        assert_eq!(bare.glyph_name(1), None, "no post table names nothing");

        let mut three = post_v2(&names);
        three[..4].copy_from_slice(&0x0003_0000_u32.to_be_bytes());
        let three = font_with_tables(&[], &[0, 0, 0, 0, 0], 4, &[], &[(b"post", &three)]);
        let three = TrueTypeFont::parse(three).expect("the font parses");
        assert_eq!(
            three.glyph_name(1),
            None,
            "version 3.0 declares that it has no names"
        );
    }

    fn font_with_cmap(glyf: &[u8], loca: &[u16], glyph_count: u16, cmap: &[u8]) -> Vec<u8> {
        font_with_tables(glyf, loca, glyph_count, cmap, &[])
    }

    fn font_with_tables(
        glyf: &[u8],
        loca: &[u16],
        glyph_count: u16,
        cmap: &[u8],
        extra: &[(&[u8; 4], &[u8])],
    ) -> Vec<u8> {
        let mut head = vec![0_u8; 54];
        head[18..20].copy_from_slice(&1000_u16.to_be_bytes());
        head[50..52].copy_from_slice(&0_u16.to_be_bytes());
        let mut maxp = vec![0_u8; 6];
        maxp[4..6].copy_from_slice(&glyph_count.to_be_bytes());
        let loca_bytes: Vec<u8> = loca.iter().flat_map(|value| value.to_be_bytes()).collect();

        let mut tables: Vec<(&[u8; 4], &[u8])> = vec![
            (b"glyf", glyf),
            (b"head", &head),
            (b"loca", &loca_bytes),
            (b"maxp", &maxp),
        ];
        if !cmap.is_empty() {
            tables.push((b"cmap", cmap));
        }
        tables.extend_from_slice(extra);
        let mut out = Vec::new();
        out.extend_from_slice(&0x0001_0000_u32.to_be_bytes());
        out.extend_from_slice(
            &u16::try_from(tables.len())
                .expect("table count")
                .to_be_bytes(),
        );
        out.extend_from_slice(&[0; 6]);
        let mut offset = 12 + tables.len() * 16;
        let mut directory = Vec::new();
        let mut body = Vec::new();
        for (tag, data) in &tables {
            directory.extend_from_slice(*tag);
            directory.extend_from_slice(&[0; 4]);
            directory.extend_from_slice(&u32::try_from(offset).expect("offset").to_be_bytes());
            directory.extend_from_slice(&u32::try_from(data.len()).expect("length").to_be_bytes());
            body.extend_from_slice(data);
            offset += data.len();
        }
        out.extend_from_slice(&directory);
        out.extend_from_slice(&body);
        out
    }

    fn pad(mut glyf: Vec<u8>) -> Vec<u8> {
        if glyf.len() % 2 == 1 {
            glyf.push(0);
        }
        glyf
    }

    fn triangle() -> Vec<u8> {
        let mut glyf = Vec::new();
        glyf.extend_from_slice(&1_i16.to_be_bytes());
        for value in [0_i16, 0, 100, 100] {
            glyf.extend_from_slice(&value.to_be_bytes());
        }
        glyf.extend_from_slice(&2_u16.to_be_bytes());
        glyf.extend_from_slice(&0_u16.to_be_bytes());
        glyf.extend_from_slice(&[0x37, 0x37, 0x37]);
        glyf.extend_from_slice(&[0, 100, 0]);
        glyf.extend_from_slice(&[0, 0, 100]);
        pad(glyf)
    }

    fn format4(segments: &[(u16, u16, i16)]) -> Vec<u8> {
        let count = u16::try_from(segments.len()).expect("segments");
        let mut out = Vec::new();
        out.extend_from_slice(&4_u16.to_be_bytes());
        out.extend_from_slice(&(16 + count * 8).to_be_bytes());
        out.extend_from_slice(&0_u16.to_be_bytes());
        out.extend_from_slice(&(count * 2).to_be_bytes());
        out.extend_from_slice(&[0; 6]);
        for (_, end, _) in segments {
            out.extend_from_slice(&end.to_be_bytes());
        }
        out.extend_from_slice(&0_u16.to_be_bytes());
        for (start, _, _) in segments {
            out.extend_from_slice(&start.to_be_bytes());
        }
        for (_, _, delta) in segments {
            out.extend_from_slice(&delta.to_be_bytes());
        }
        for _ in segments {
            out.extend_from_slice(&0_u16.to_be_bytes());
        }
        out
    }

    fn delta(character: u16, glyph: u16) -> i16 {
        glyph.wrapping_sub(character).cast_signed()
    }

    fn cmap(subtables: &[(u16, u16, Vec<u8>)]) -> Vec<u8> {
        let count = u16::try_from(subtables.len()).expect("subtables");
        let mut out = 0_u16.to_be_bytes().to_vec();
        out.extend_from_slice(&count.to_be_bytes());
        let mut offset = 4 + u32::from(count) * 8;
        let mut body = Vec::new();
        for (platform, encoding, data) in subtables {
            out.extend_from_slice(&platform.to_be_bytes());
            out.extend_from_slice(&encoding.to_be_bytes());
            out.extend_from_slice(&offset.to_be_bytes());
            offset += u32::try_from(data.len()).expect("subtable length");
            body.extend_from_slice(data);
        }
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn the_cmap_read_backwards_gives_unicode_and_never_the_private_use_area() {
        let glyf = triangle();
        let loca = [0, 0, u16::try_from(glyf.len() / 2).expect("loca")];
        let unicode = format4(&[
            (0x0041, 0x0041, 1 - 0x0041),
            (0x0E01, 0x0E01, 2 - 0x0E01),
            (0xFFFF, 0xFFFF, 1),
        ]);
        let symbol = format4(&[(0xF041, 0xF041, delta(0xF041, 3)), (0xFFFF, 0xFFFF, 1)]);
        let font = TrueTypeFont::parse(font_with_cmap(
            &glyf,
            &loca,
            4,
            &cmap(&[(3, 1, unicode), (3, 0, symbol)]),
        ))
        .expect("valid sfnt");

        let found = font.characters();
        assert!(found.contains(&(1, 'A')), "{found:?}");
        assert!(found.contains(&(2, 'ก')), "{found:?}");
        assert!(
            found.iter().all(|(glyph, _)| *glyph != 3),
            "the symbolic table was walked: {found:?}"
        );
        assert!(
            found
                .iter()
                .all(|(_, character)| !('\u{e000}'..='\u{f8ff}').contains(character)),
            "a private-use code point came back as text: {found:?}"
        );
        assert_eq!(font.glyph_for_code(0x41).map(|(glyph, _)| glyph), Some(3));
    }

    #[test]
    fn reads_a_simple_contour_and_its_units_per_em() {
        let glyf = triangle();
        let font = TrueTypeFont::parse(font_with_glyf(
            &glyf,
            &[0, 0, u16::try_from(glyf.len() / 2).expect("loca")],
            2,
        ))
        .expect("valid sfnt");
        assert_eq!(font.units_per_em(), 1000);
        assert_eq!(font.glyph_count(), 2);
        let outline = font.outline(1).expect("glyph 1");
        assert_eq!(outline.contours.len(), 1);
        assert_eq!(
            outline.contours[0],
            vec![
                OutlinePoint {
                    x: 0.0,
                    y: 0.0,
                    on_curve: true
                },
                OutlinePoint {
                    x: 100.0,
                    y: 0.0,
                    on_curve: true
                },
                OutlinePoint {
                    x: 100.0,
                    y: 100.0,
                    on_curve: true
                },
            ]
        );
    }

    #[test]
    fn a_remembered_outline_is_the_outline_decoded() {
        let glyf = triangle();
        let font = TrueTypeFont::parse(font_with_glyf(
            &glyf,
            &[0, 0, u16::try_from(glyf.len() / 2).expect("loca")],
            2,
        ))
        .expect("valid sfnt");
        let decoded = font.decode_path(1).expect("glyph 1");
        assert!(!decoded.is_empty());
        assert_eq!(font.path(1).as_ref(), Some(&decoded), "first ask");
        assert_eq!(font.path(1).as_ref(), Some(&decoded), "remembered");
        assert_eq!(font.path(7), font.decode_path(7), "a glyph past the end");
        assert_eq!(font.path(7), font.decode_path(7), "and again");
        let copy = font.clone();
        assert_eq!(copy.path(1).as_ref(), Some(&decoded), "a clone");
        assert_eq!(copy, font, "the memo is not part of what a font is");
    }

    #[test]
    fn the_line_is_read_from_hhea_and_a_line_with_no_height_is_not_one() {
        let glyf = triangle();
        let loca = [0, 0, u16::try_from(glyf.len() / 2).expect("loca")];
        let hhea = |ascender: i16, descender: i16| {
            let mut table = vec![0_u8; 36];
            table[4..6].copy_from_slice(&ascender.to_be_bytes());
            table[6..8].copy_from_slice(&descender.to_be_bytes());
            table
        };
        let with = |table: &[u8]| {
            TrueTypeFont::parse(font_with_tables(&glyf, &loca, 2, &[], &[(b"hhea", table)]))
                .expect("valid sfnt")
                .line_metrics()
        };
        let (ascent, descent) = with(&hhea(880, -120)).expect("a line");
        assert!((ascent - 0.88).abs() < 1e-12, "{ascent}");
        assert!((descent + 0.12).abs() < 1e-12, "{descent}");
        assert_eq!(with(&hhea(0, 0)), None);
        assert_eq!(with(&hhea(-120, 880)), None);
        assert_eq!(with(&hhea(880, -120)[..6]), None);
        let none = TrueTypeFont::parse(font_with_glyf(&glyf, &loca, 2)).expect("valid sfnt");
        assert_eq!(none.line_metrics(), None);
    }

    #[test]
    fn an_empty_loca_entry_is_a_glyph_with_no_marks() {
        let glyf = triangle();
        let font = TrueTypeFont::parse(font_with_glyf(
            &glyf,
            &[0, 0, u16::try_from(glyf.len() / 2).expect("loca")],
            2,
        ))
        .expect("valid sfnt");
        let outline = font.outline(0).expect("glyph 0 resolves");
        assert!(outline.is_empty());
        assert!(font.outline(2).is_none(), "out of range glyph");
    }

    #[test]
    fn a_composite_glyph_places_its_component() {
        let mut glyf = triangle();
        let component_end = glyf.len();
        glyf.extend_from_slice(&(-1_i16).to_be_bytes());
        for value in [0_i16, 0, 200, 200] {
            glyf.extend_from_slice(&value.to_be_bytes());
        }
        glyf.extend_from_slice(&0x0003_u16.to_be_bytes());
        glyf.extend_from_slice(&1_u16.to_be_bytes());
        glyf.extend_from_slice(&10_i16.to_be_bytes());
        glyf.extend_from_slice(&20_i16.to_be_bytes());
        let glyf = pad(glyf);
        let font = TrueTypeFont::parse(font_with_glyf(
            &glyf,
            &[
                0,
                0,
                u16::try_from(component_end / 2).expect("loca"),
                u16::try_from(glyf.len() / 2).expect("loca"),
            ],
            3,
        ))
        .expect("valid sfnt");
        let outline = font.outline(2).expect("composite glyph");
        assert_eq!(outline.contours.len(), 1);
        assert_eq!(
            outline.contours[0][0],
            OutlinePoint {
                x: 10.0,
                y: 20.0,
                on_curve: true
            }
        );
        assert_eq!(
            outline.contours[0][2],
            OutlinePoint {
                x: 110.0,
                y: 120.0,
                on_curve: true
            }
        );
    }

    #[test]
    fn malformed_font_programs_fail_closed() {
        assert_eq!(
            TrueTypeFont::parse(b"not a font at all".to_vec()).unwrap_err(),
            TrueTypeError::NotSfnt
        );
        let glyf = triangle();
        let mut truncated = font_with_glyf(
            &glyf,
            &[0, 0, u16::try_from(glyf.len() / 2).expect("loca")],
            2,
        );
        truncated.truncate(truncated.len() - glyf.len() - 1);
        assert_eq!(
            TrueTypeFont::parse(truncated).unwrap_err(),
            TrueTypeError::TableOutOfRange
        );
        let font = TrueTypeFont::parse(font_with_glyf(&glyf[..12], &[0, 0, 6], 2))
            .expect("directory still parses");
        assert!(font.outline(1).is_none());
    }
}
