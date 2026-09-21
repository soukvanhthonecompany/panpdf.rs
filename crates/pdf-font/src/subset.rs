use std::collections::BTreeSet;

use crate::truetype::TrueTypeFont;

mod cff;
pub use cff::subset_cff;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubsetError {
    NoTrueTypeOutlines,
    MalformedTable([u8; 4]),
    NoSuchGlyph(u16),
    MalformedGlyph(u16),
    TooDeep(u16),
    TooLarge,
}

impl std::fmt::Display for SubsetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoTrueTypeOutlines => formatter.write_str("the face has no TrueType outlines"),
            Self::MalformedTable(tag) => write!(
                formatter,
                "the face's {} table is missing or malformed",
                String::from_utf8_lossy(tag)
            ),
            Self::NoSuchGlyph(glyph) => write!(formatter, "the face has no glyph {glyph}"),
            Self::MalformedGlyph(glyph) => write!(formatter, "glyph {glyph} is malformed"),
            Self::TooDeep(glyph) => write!(formatter, "glyph {glyph} nests too deeply"),
            Self::TooLarge => formatter.write_str("the subset is too large for an sfnt"),
        }
    }
}

impl std::error::Error for SubsetError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Subset {
    pub program: Vec<u8>,
    pub glyphs: BTreeSet<u16>,
}

const MAX_DEPTH: usize = 8;

const COPIED: [[u8; 4]; 4] = [*b"OS/2", *b"cvt ", *b"fpgm", *b"prep"];

pub fn subset_truetype(font: &TrueTypeFont, glyphs: &BTreeSet<u16>) -> Result<Subset, SubsetError> {
    if !font.has_outlines() {
        return Err(SubsetError::NoTrueTypeOutlines);
    }
    let glyf = font
        .table(*b"glyf")
        .ok_or(SubsetError::MalformedTable(*b"glyf"))?;
    let mut kept = BTreeSet::from([0_u16]);
    for &glyph in glyphs {
        keep(font, glyf, glyph, 0, &mut kept)?;
    }
    let last = *kept.last().unwrap_or(&0);
    let count = last + 1;

    let mut new_glyf = Vec::new();
    let mut loca = Vec::with_capacity((usize::from(count) + 1) * 4);
    for glyph in 0..count {
        loca.extend_from_slice(&offset(new_glyf.len())?.to_be_bytes());
        if kept.contains(&glyph) {
            new_glyf.extend_from_slice(body(font, glyf, glyph)?);
            while new_glyf.len() % 4 != 0 {
                new_glyf.push(0);
            }
        }
    }
    loca.extend_from_slice(&offset(new_glyf.len())?.to_be_bytes());

    let mut head = required(font, *b"head", 54)?.to_vec();
    head[8..12].fill(0);
    head[50..52].copy_from_slice(&1_u16.to_be_bytes());
    let mut maxp = required(font, *b"maxp", 6)?.to_vec();
    maxp[4..6].copy_from_slice(&count.to_be_bytes());
    let (hhea, hmtx) = metrics(font, count)?;

    let mut tables: Vec<([u8; 4], Vec<u8>)> = vec![
        (*b"glyf", new_glyf),
        (*b"head", head),
        (*b"hhea", hhea),
        (*b"hmtx", hmtx),
        (*b"loca", loca),
        (*b"maxp", maxp),
    ];
    for tag in COPIED {
        if let Some(bytes) = font.table(tag) {
            tables.push((tag, bytes.to_vec()));
        }
    }
    tables.sort_by_key(|table| table.0);
    Ok(Subset {
        program: assemble(&tables)?,
        glyphs: kept,
    })
}

pub const GLYPHLESS_ADVANCE: u16 = 500;

pub fn glyphless() -> Result<Vec<u8>, SubsetError> {
    const UNITS: u16 = 1000;
    let be = |values: &[u16]| {
        values
            .iter()
            .flat_map(|value| value.to_be_bytes())
            .collect::<Vec<u8>>()
    };
    let mut head = Vec::with_capacity(54);
    head.extend_from_slice(&0x0001_0000_u32.to_be_bytes());
    head.extend_from_slice(&0x0001_0000_u32.to_be_bytes());
    head.extend_from_slice(&0_u32.to_be_bytes());
    head.extend_from_slice(&0x5F0F_3CF5_u32.to_be_bytes());
    head.extend(be(&[0x000B, UNITS]));
    head.extend_from_slice(&[0; 16]);
    head.extend(be(&[0, 0, GLYPHLESS_ADVANCE, UNITS, 0, 8, 2, 0, 0]));
    let mut hhea = Vec::with_capacity(36);
    hhea.extend_from_slice(&0x0001_0000_u32.to_be_bytes());
    hhea.extend(be(&[
        UNITS,
        0,
        0,
        GLYPHLESS_ADVANCE,
        0,
        0,
        0,
        1,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        2,
    ]));
    let mut maxp = Vec::with_capacity(32);
    maxp.extend_from_slice(&0x0001_0000_u32.to_be_bytes());
    maxp.extend(be(&[2, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0]));
    let hmtx = be(&[GLYPHLESS_ADVANCE, 0, GLYPHLESS_ADVANCE, 0]);
    let loca = be(&[0, 0, 0]);
    let mut post = Vec::with_capacity(32);
    post.extend_from_slice(&0x0003_0000_u32.to_be_bytes());
    post.extend_from_slice(&[0; 28]);
    assemble(&[
        (*b"glyf", Vec::new()),
        (*b"head", head),
        (*b"hhea", hhea),
        (*b"hmtx", hmtx),
        (*b"loca", loca),
        (*b"maxp", maxp),
        (*b"post", post),
    ])
}

fn keep(
    font: &TrueTypeFont,
    glyf: &[u8],
    glyph: u16,
    depth: usize,
    kept: &mut BTreeSet<u16>,
) -> Result<(), SubsetError> {
    if depth > MAX_DEPTH {
        return Err(SubsetError::TooDeep(glyph));
    }
    if glyph >= font.glyph_count() {
        return Err(SubsetError::NoSuchGlyph(glyph));
    }
    if !kept.insert(glyph) && depth > 0 {
        return Ok(());
    }
    let bytes = body(font, glyf, glyph)?;
    if bytes.is_empty() || read_i16(bytes, 0).ok_or(SubsetError::MalformedGlyph(glyph))? >= 0 {
        return Ok(());
    }
    for component in components(bytes).ok_or(SubsetError::MalformedGlyph(glyph))? {
        keep(font, glyf, component, depth + 1, kept)?;
    }
    Ok(())
}

fn components(body: &[u8]) -> Option<Vec<u16>> {
    const ARGS_ARE_WORDS: u16 = 0x0001;
    const HAVE_SCALE: u16 = 0x0008;
    const MORE_COMPONENTS: u16 = 0x0020;
    const HAVE_XY_SCALE: u16 = 0x0040;
    const HAVE_TWO_BY_TWO: u16 = 0x0080;
    let mut found = Vec::new();
    let mut cursor = 10_usize;
    loop {
        let flags = read_u16(body, cursor)?;
        found.push(read_u16(body, cursor + 2)?);
        cursor += 4;
        cursor += if flags & ARGS_ARE_WORDS == 0 { 2 } else { 4 };
        cursor += if flags & HAVE_SCALE != 0 {
            2
        } else if flags & HAVE_XY_SCALE != 0 {
            4
        } else if flags & HAVE_TWO_BY_TWO != 0 {
            8
        } else {
            0
        };
        if cursor > body.len() {
            return None;
        }
        if flags & MORE_COMPONENTS == 0 || found.len() > usize::from(u16::MAX) {
            return Some(found);
        }
    }
}

fn body<'a>(font: &TrueTypeFont, glyf: &'a [u8], glyph: u16) -> Result<&'a [u8], SubsetError> {
    let (start, end) = font
        .glyph_range(glyph)
        .ok_or(SubsetError::MalformedGlyph(glyph))?;
    glyf.get(start..end)
        .ok_or(SubsetError::MalformedGlyph(glyph))
}

fn required(font: &TrueTypeFont, tag: [u8; 4], length: usize) -> Result<&[u8], SubsetError> {
    font.table(tag)
        .filter(|bytes| bytes.len() >= length)
        .ok_or(SubsetError::MalformedTable(tag))
}

fn metrics(font: &TrueTypeFont, count: u16) -> Result<(Vec<u8>, Vec<u8>), SubsetError> {
    let mut hhea = required(font, *b"hhea", 36)?.to_vec();
    let old = read_u16(&hhea, 34).ok_or(SubsetError::MalformedTable(*b"hhea"))?;
    let hmtx = font
        .table(*b"hmtx")
        .ok_or(SubsetError::MalformedTable(*b"hmtx"))?;
    if old == 0 {
        return Err(SubsetError::MalformedTable(*b"hhea"));
    }
    let pairs = old.min(count);
    let mut out = hmtx
        .get(..usize::from(pairs) * 4)
        .ok_or(SubsetError::MalformedTable(*b"hmtx"))?
        .to_vec();
    for glyph in pairs..count {
        let at = usize::from(old) * 4 + usize::from(glyph - old) * 2;
        out.extend_from_slice(
            hmtx.get(at..at + 2)
                .ok_or(SubsetError::MalformedTable(*b"hmtx"))?,
        );
    }
    hhea[34..36].copy_from_slice(&pairs.to_be_bytes());
    Ok((hhea, out))
}

fn assemble(tables: &[([u8; 4], Vec<u8>)]) -> Result<Vec<u8>, SubsetError> {
    let count = u16::try_from(tables.len()).map_err(|_| SubsetError::TooLarge)?;
    let power = 1_u16 << (u16::BITS - 1 - count.leading_zeros());
    let search_range = power * 16;
    let mut out = Vec::new();
    out.extend_from_slice(&0x0001_0000_u32.to_be_bytes());
    out.extend_from_slice(&count.to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(
        &u16::try_from(power.trailing_zeros())
            .unwrap_or(0)
            .to_be_bytes(),
    );
    out.extend_from_slice(&(count * 16 - search_range).to_be_bytes());
    let mut at = 12 + tables.len() * 16;
    let mut head_at = None;
    for (tag, bytes) in tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum(bytes).to_be_bytes());
        out.extend_from_slice(&offset(at)?.to_be_bytes());
        out.extend_from_slice(&offset(bytes.len())?.to_be_bytes());
        if tag == b"head" {
            head_at = Some(at);
        }
        at += bytes.len().next_multiple_of(4);
    }
    for (_, bytes) in tables {
        out.extend_from_slice(bytes);
        out.resize(out.len().next_multiple_of(4), 0);
    }
    if let Some(head) = head_at {
        let adjustment = 0xB1B0_AFBA_u32.wrapping_sub(checksum(&out));
        out[head + 8..head + 12].copy_from_slice(&adjustment.to_be_bytes());
    }
    Ok(out)
}

fn checksum(bytes: &[u8]) -> u32 {
    bytes.chunks(4).fold(0_u32, |sum, chunk| {
        let mut word = [0_u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

fn offset(value: usize) -> Result<u32, SubsetError> {
    u32::try_from(value).map_err(|_| SubsetError::TooLarge)
}

fn read_u16(data: &[u8], at: usize) -> Option<u16> {
    let bytes = data.get(at..at + 2)?;
    Some(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_i16(data: &[u8], at: usize) -> Option<i16> {
    read_u16(data, at).map(u16::cast_signed)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    #[test]
    fn the_glyphless_face_is_a_face_that_draws_nothing() {
        let program = super::glyphless().expect("a few dozen bytes fit");
        let face = TrueTypeFont::parse(program.clone()).expect("it parses");
        assert_eq!(face.units_per_em(), 1000);
        assert_eq!(face.glyph_count(), 2);
        assert!(face.has_outlines());
        for glyph in 0..2 {
            assert_eq!(face.advance_width(glyph), Some(super::GLYPHLESS_ADVANCE));
        }
        assert_eq!(face.table(*b"glyf").map(<[u8]>::len), Some(0));
        assert_eq!(super::checksum(&program), 0xB1B0_AFBA);
    }

    use super::{SubsetError, checksum, subset_truetype};
    use crate::truetype::TrueTypeFont;

    fn triangle(size: u8) -> Vec<u8> {
        let mut glyph = Vec::new();
        glyph.extend_from_slice(&1_i16.to_be_bytes());
        for value in [0_i16, 0, i16::from(size), i16::from(size)] {
            glyph.extend_from_slice(&value.to_be_bytes());
        }
        glyph.extend_from_slice(&2_u16.to_be_bytes());
        glyph.extend_from_slice(&0_u16.to_be_bytes());
        glyph.extend_from_slice(&[0x37; 3]);
        glyph.extend_from_slice(&[0, size, 0]);
        glyph.extend_from_slice(&[0, 0, size]);
        glyph
    }

    fn composite(component: u16, dx: i8, dy: i8) -> Vec<u8> {
        let mut glyph = Vec::new();
        glyph.extend_from_slice(&(-1_i16).to_be_bytes());
        glyph.extend_from_slice(&[0; 8]);
        glyph.extend_from_slice(&0x000A_u16.to_be_bytes());
        glyph.extend_from_slice(&component.to_be_bytes());
        glyph.extend_from_slice(&[dx.cast_unsigned(), dy.cast_unsigned()]);
        glyph.extend_from_slice(&0x4000_u16.to_be_bytes());
        glyph
    }

    fn face(glyphs: &[Vec<u8>]) -> Vec<u8> {
        let mut glyf = Vec::new();
        let mut loca = Vec::new();
        for glyph in glyphs {
            loca.extend_from_slice(&u16::try_from(glyf.len() / 2).expect("offset").to_be_bytes());
            glyf.extend_from_slice(glyph);
            if glyf.len() % 2 == 1 {
                glyf.push(0);
            }
        }
        loca.extend_from_slice(&u16::try_from(glyf.len() / 2).expect("offset").to_be_bytes());
        let count = u16::try_from(glyphs.len()).expect("count");
        let mut head = vec![0_u8; 54];
        head[18..20].copy_from_slice(&1000_u16.to_be_bytes());
        let mut maxp = vec![0_u8; 32];
        maxp[..4].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
        maxp[4..6].copy_from_slice(&count.to_be_bytes());
        let mut hhea = vec![0_u8; 36];
        hhea[4..6].copy_from_slice(&800_i16.to_be_bytes());
        hhea[34..36].copy_from_slice(&2_u16.to_be_bytes());
        let mut hmtx = Vec::new();
        for (advance, bearing) in [(500_u16, 0_i16), (600, 10)] {
            hmtx.extend_from_slice(&advance.to_be_bytes());
            hmtx.extend_from_slice(&bearing.to_be_bytes());
        }
        for glyph in 2..count {
            hmtx.extend_from_slice(&i16::try_from(glyph).expect("bearing").to_be_bytes());
        }
        let tables: [(&[u8; 4], Vec<u8>); 9] = [
            (b"cmap", vec![0; 4]),
            (b"fpgm", vec![0xB0, 0x01]),
            (b"glyf", glyf),
            (b"head", head),
            (b"hhea", hhea),
            (b"hmtx", hmtx),
            (b"loca", loca),
            (b"maxp", maxp),
            (b"post", vec![0; 32]),
        ];
        let mut out = Vec::new();
        out.extend_from_slice(&0x0001_0000_u32.to_be_bytes());
        out.extend_from_slice(&9_u16.to_be_bytes());
        out.extend_from_slice(&[0; 6]);
        let mut at = 12 + tables.len() * 16;
        let mut bodies = Vec::new();
        for (tag, bytes) in &tables {
            out.extend_from_slice(*tag);
            out.extend_from_slice(&[0; 4]);
            out.extend_from_slice(&u32::try_from(at).expect("at").to_be_bytes());
            out.extend_from_slice(&u32::try_from(bytes.len()).expect("length").to_be_bytes());
            bodies.extend_from_slice(bytes);
            at += bytes.len();
        }
        out.extend_from_slice(&bodies);
        out
    }

    fn tags(program: &[u8]) -> Vec<String> {
        let count = usize::from(u16::from_be_bytes([program[4], program[5]]));
        (0..count)
            .map(|index| {
                String::from_utf8_lossy(&program[12 + index * 16..16 + index * 16]).into_owned()
            })
            .collect()
    }

    #[test]
    fn a_subset_keeps_the_asked_glyphs_their_components_and_their_ids() {
        let original = TrueTypeFont::parse(face(&[
            triangle(50),
            triangle(100),
            composite(1, 20, -10),
            triangle(120),
            triangle(90),
        ]))
        .expect("face");

        let subset = subset_truetype(&original, &BTreeSet::from([2])).expect("subset");
        assert_eq!(subset.glyphs, BTreeSet::from([0, 1, 2]));
        let font = TrueTypeFont::parse(subset.program.clone()).expect("the subset parses");
        assert_eq!(font.glyph_count(), 3);
        assert!(!font.outline(2).expect("composite").is_empty());
        for glyph in 0..3 {
            assert_eq!(
                font.outline(glyph),
                original.outline(glyph),
                "glyph {glyph}"
            );
        }
        assert_eq!(font.outline(3), None);
        assert_eq!(
            tags(&subset.program),
            ["fpgm", "glyf", "head", "hhea", "hmtx", "loca", "maxp"]
        );
        assert!(subset.program.len() < original.program_bytes().len());
        assert_eq!(checksum(&subset.program), 0xB1B0_AFBA);

        let subset = subset_truetype(&original, &BTreeSet::from([3])).expect("subset");
        assert_eq!(subset.glyphs, BTreeSet::from([0, 3]));
        let font = TrueTypeFont::parse(subset.program).expect("the subset parses");
        assert_eq!(font.glyph_count(), 4);
        assert_eq!(font.outline(3), original.outline(3));
        assert!(font.outline(1).expect("emptied").is_empty());
        assert!(font.outline(2).expect("emptied").is_empty());
        assert_eq!(font.advance_width(0), Some(500));
        assert_eq!(font.advance_width(3), Some(600));
    }

    #[test]
    fn a_subset_shorter_than_the_face_metrics_says_so_in_hhea() {
        let original = TrueTypeFont::parse(face(&[triangle(50), triangle(100)])).expect("face");
        let subset = subset_truetype(&original, &BTreeSet::new()).expect("subset");
        let font = TrueTypeFont::parse(subset.program).expect("the subset parses");
        assert_eq!(font.glyph_count(), 1);
        let hhea = font.table(*b"hhea").expect("hhea");
        assert_eq!(u16::from_be_bytes([hhea[34], hhea[35]]), 1);
        assert_eq!(font.table(*b"hmtx").expect("hmtx").len(), 4);
        assert_eq!(font.advance_width(0), Some(500));
        assert_eq!(font.outline(0), original.outline(0));
    }

    #[test]
    fn a_glyph_the_face_lacks_or_a_component_it_lacks_is_refused() {
        let original =
            TrueTypeFont::parse(face(&[triangle(50), composite(7, 0, 0)])).expect("face");
        assert_eq!(
            subset_truetype(&original, &BTreeSet::from([2])),
            Err(SubsetError::NoSuchGlyph(2))
        );
        assert_eq!(
            subset_truetype(&original, &BTreeSet::from([1])),
            Err(SubsetError::NoSuchGlyph(7))
        );
    }
}
