use std::collections::BTreeMap;
use std::fmt::Write as _;

use pdf_bytes::ByteStore;
use pdf_content::TrueTypeFont;
use pdf_syntax::{Object, ObjectKind, Reference};

use crate::plan::{PlannedBody, PlannedWrite};
use crate::spike_move_text::SpikeError;

const fn refused(reason: &'static str) -> SpikeError {
    SpikeError::BlockRewriteUnsupported(reason)
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NewFont {
    pub font: Reference,
    pub writes: Vec<PlannedWrite>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FontObjects {
    pub font: Reference,
    pub descendant: Reference,
    pub descriptor: Reference,
    pub program: Reference,
    pub unicode: Reference,
}

impl FontObjects {
    pub(crate) const fn numbered_from(first: u32) -> Self {
        Self {
            font: Reference::new(first, 0),
            descendant: Reference::new(first + 1, 0),
            descriptor: Reference::new(first + 2, 0),
            program: Reference::new(first + 3, 0),
            unicode: Reference::new(first + 4, 0),
        }
    }
}

pub(crate) fn face_mark(sha256: &str, index: u32) -> String {
    let hash: String = sha256.chars().filter(char::is_ascii_alphanumeric).collect();
    format!("{hash}-{index}")
}

#[derive(Clone, Copy)]
pub(crate) enum Embeddable<'a> {
    TrueType(&'a TrueTypeFont),
    Cff {
        sfnt: &'a TrueTypeFont,
        font: &'a pdf_content::CffFont,
        table: &'a [u8],
    },
}

impl<'a> Embeddable<'a> {
    pub(crate) fn of(program: &'a pdf_content::GlyphProgram) -> Option<Self> {
        match program {
            pdf_content::GlyphProgram::TrueType(font) => Some(Self::TrueType(font)),
            pdf_content::GlyphProgram::Cff {
                font,
                data,
                sfnt: Some(sfnt),
                ..
            } => Some(Self::Cff {
                sfnt,
                font,
                table: data,
            }),
            _ => None,
        }
    }

    pub(crate) const fn metrics(self) -> &'a TrueTypeFont {
        match self {
            Self::TrueType(font) | Self::Cff { sfnt: font, .. } => font,
        }
    }

    pub(crate) fn code(self, glyph: u16) -> Option<u16> {
        match self {
            Self::Cff { font, .. } if font.is_cid() => font.cid_for_glyph(glyph),
            _ => Some(glyph),
        }
    }

    pub(crate) fn glyph(self, code: u16) -> Option<u16> {
        match self {
            Self::Cff { font, .. } if font.is_cid() => font.glyph_for_cid(code),
            _ => Some(code),
        }
    }
}

pub(crate) fn embed(
    face: Embeddable<'_>,
    text: &BTreeMap<u16, String>,
    (objects, mark, grown): (FontObjects, &str, bool),
) -> Result<NewFont, SpikeError> {
    let unsubset = || refused("the face chosen for a typed character cannot be subset");
    let glyphs = text.keys().copied().collect();
    let metrics = face.metrics();
    let (subtype, file, file_dictionary, cid_to_gid, bytes) = match face {
        Embeddable::TrueType(font) => (
            "CIDFontType2",
            "FontFile2",
            "",
            "/CIDToGIDMap /Identity ",
            pdf_content::subset_truetype(font, &glyphs)
                .map_err(|_| unsubset())?
                .program,
        ),
        Embeddable::Cff { table, .. } => (
            "CIDFontType0",
            "FontFile3",
            "/Subtype /CIDFontType0C",
            "",
            pdf_content::subset_cff(table, &glyphs)
                .map_err(|_| unsubset())?
                .0,
        ),
    };
    let mut coded = BTreeMap::new();
    for (glyph, meaning) in text {
        coded.insert(
            face.code(*glyph).ok_or_else(unsubset)?,
            (*glyph, meaning.clone()),
        );
    }
    let FontObjects {
        font,
        descendant,
        descriptor,
        program,
        unicode,
    } = objects;
    let stream = |reference: Reference, dictionary: &str, decoded: Vec<u8>| PlannedWrite {
        reference,
        body: if grown {
            PlannedBody::ReplacedStream { decoded }
        } else {
            PlannedBody::NewStream {
                dictionary: dictionary.as_bytes().to_vec(),
                decoded,
            }
        },
    };
    let name = format!(
        "{}+{}",
        subset_tag(mark, font.object_number()),
        postscript_name(metrics)
    );
    let widths: String = coded
        .iter()
        .map(|(code, (glyph, _))| format!("{code} [{}]", width(metrics, *glyph)))
        .collect::<Vec<_>>()
        .join(" ");
    let meanings: BTreeMap<u16, String> = coded
        .iter()
        .map(|(code, (_, meaning))| (*code, meaning.clone()))
        .collect();

    let writes = vec![
        direct(
            font,
            format!(
                "<< /Type /Font /Subtype /Type0 /BaseFont /{name} /Encoding /Identity-H \
                 /DescendantFonts [{} 0 R] /ToUnicode {} 0 R >>",
                descendant.object_number(),
                unicode.object_number()
            ),
        ),
        direct(
            descendant,
            format!(
                "<< /Type /Font /Subtype /{subtype} /BaseFont /{name} \
                 /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
                 /FontDescriptor {} 0 R {cid_to_gid}/DW 0 /W [{widths}] >>",
                descriptor.object_number()
            ),
        ),
        direct(descriptor, descriptor_body(metrics, &name, file, program)),
        stream(program, file_dictionary, bytes),
        stream(unicode, "", to_unicode(&meanings).into_bytes()),
    ];
    Ok(NewFont { font, writes })
}

fn descriptor_body(metrics: &TrueTypeFont, name: &str, file: &str, program: Reference) -> String {
    let scale = 1000.0 / f64::from(metrics.units_per_em());
    let units = |value: f64| (value * scale).round();
    let [x_min, y_min, x_max, y_max] = metrics.font_box().unwrap_or([0, 0, 1000, 1000]);
    let (ascent, descent) = metrics.line_metrics().unwrap_or((0.8, -0.2));
    let em = f64::from(metrics.units_per_em());
    let cap = metrics.cap_height().map_or(ascent * em, f64::from);
    let italic = metrics.italic_angle().unwrap_or(0.0);
    let flags = if italic == 0.0 { 4 } else { 4 | 64 };
    format!(
        "<< /Type /FontDescriptor /FontName /{name} /Flags {flags} \
         /FontBBox [{} {} {} {}] /ItalicAngle {italic} /Ascent {} /Descent {} \
         /CapHeight {} /StemV 80 /{file} {} 0 R >>",
        units(f64::from(x_min)),
        units(f64::from(y_min)),
        units(f64::from(x_max)),
        units(f64::from(y_max)),
        (ascent * 1000.0).round(),
        (descent * 1000.0).round(),
        units(cap),
        program.object_number()
    )
}

#[cfg(test)]
pub(crate) fn embed_truetype(
    face: &TrueTypeFont,
    text: &BTreeMap<u16, String>,
    target: (FontObjects, &str, bool),
) -> Result<NewFont, SpikeError> {
    embed(Embeddable::TrueType(face), text, target)
}

pub(crate) fn width(face: &TrueTypeFont, glyph: u16) -> f64 {
    let advance = face.advance_width(glyph).unwrap_or(0);
    (f64::from(advance) * 1000.0 / f64::from(face.units_per_em())).round()
}

fn direct(reference: Reference, body: String) -> PlannedWrite {
    PlannedWrite {
        reference,
        body: PlannedBody::Direct {
            body: body.into_bytes(),
        },
    }
}

#[cfg(test)]
pub(crate) fn show_codes(glyphs: &[u16]) -> String {
    let mut out = String::with_capacity(glyphs.len() * 4 + 2);
    out.push('<');
    for glyph in glyphs {
        let _ = write!(out, "{glyph:04X}");
    }
    out.push('>');
    out
}

fn subset_tag(mark: &str, object: u32) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in mark.as_bytes().iter().chain(&object.to_be_bytes()) {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3);
    }
    (0..6)
        .map(|index| char::from(b'A' + u8::try_from((hash >> (index * 8)) % 26).unwrap_or(0)))
        .collect()
}

fn postscript_name(face: &TrueTypeFont) -> String {
    let name: String = face
        .postscript_name()
        .unwrap_or_default()
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .take(63)
        .collect();
    if name.is_empty() {
        "Embedded".to_owned()
    } else {
        name
    }
}

pub(crate) fn to_unicode(text: &BTreeMap<u16, String>) -> String {
    let entries: Vec<String> = text
        .iter()
        .filter(|(_, meaning)| !meaning.is_empty())
        .map(|(glyph, meaning)| {
            let mut entry = format!("<{glyph:04X}> <");
            for unit in meaning.encode_utf16() {
                let _ = write!(entry, "{unit:04X}");
            }
            entry.push('>');
            entry
        })
        .collect();
    let mut out = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n\
         1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    for chunk in entries.chunks(100) {
        let _ = writeln!(out, "{} beginbfchar", chunk.len());
        for entry in chunk {
            out.push_str(entry);
            out.push('\n');
        }
        out.push_str("endbfchar\n");
    }
    out.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    out
}

pub(crate) struct Body {
    pub(crate) source: ByteStore,
    pub(crate) value: Object,
    pub(crate) reference: Reference,
    pub(crate) bytes: Vec<u8>,
    pub(crate) offset: usize,
}

pub(crate) fn resolve(source: &ByteStore, reference: Reference) -> Result<Body, SpikeError> {
    let unreadable = || refused("the page's font resources cannot be read");
    let (index, _) = crate::previous::readable_index(source, b"").ok_or_else(unreadable)?;
    let resolved = index
        .resolve_object(source, reference, pdf_syntax::ResolveLimits::default())
        .map_err(|_| unreadable())?;
    let span = resolved.value().span();
    let bytes = resolved
        .source()
        .resolve(span)
        .map_err(|_| unreadable())?
        .to_vec();
    Ok(Body {
        source: resolved.source().clone(),
        value: resolved.value().clone(),
        reference,
        bytes,
        offset: span.start(),
    })
}

pub(crate) fn entry<'a>(body: &Body, dictionary: &'a Object, key: &[u8]) -> Option<&'a Object> {
    let ObjectKind::Dictionary(entries) = dictionary.kind() else {
        return None;
    };
    entries
        .iter()
        .find(|entry| entry.key_equals(&body.source, key))
        .map(pdf_syntax::DictionaryEntry::value)
}

fn inserted(body: &Body, dictionary: &Object, text: &str) -> Result<PlannedWrite, SpikeError> {
    let malformed = || refused("the page's font resources cannot be read");
    let end = dictionary
        .span()
        .end()
        .checked_sub(body.offset)
        .ok_or_else(malformed)?;
    let close = end.checked_sub(2).ok_or_else(malformed)?;
    if body.bytes.get(close..end) != Some(b">>".as_slice()) {
        return Err(malformed());
    }
    let mut bytes = Vec::with_capacity(body.bytes.len() + text.len() + 2);
    bytes.extend_from_slice(&body.bytes[..close]);
    if !bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes.push(b' ');
    }
    bytes.extend_from_slice(text.as_bytes());
    bytes.push(b' ');
    bytes.extend_from_slice(&body.bytes[close..]);
    Ok(PlannedWrite {
        reference: body.reference,
        body: PlannedBody::Direct { body: bytes },
    })
}

fn inherited_resources(
    source: &ByteStore,
    page: &Body,
) -> Result<Option<(Body, Object)>, SpikeError> {
    const DEEPEST: usize = 100;
    let named = |body: &Body| match entry(body, &body.value, b"/Parent").map(Object::kind) {
        Some(ObjectKind::Reference(reference)) => Some(*reference),
        _ => None,
    };
    let Some(mut parent) = named(page) else {
        return Ok(None);
    };
    for _ in 0..DEEPEST {
        let body = resolve(source, parent)?;
        if let Some(resources) = entry(&body, &body.value, b"/Resources").cloned() {
            return Ok(Some((body, resources)));
        }
        let Some(next) = named(&body) else {
            return Ok(None);
        };
        parent = next;
    }
    Err(refused("the page's parents are nested too deeply to read"))
}

pub(crate) fn embedded_face(
    source: &ByteStore,
    font: Reference,
    mark: &str,
) -> Option<FontObjects> {
    let referenced =
        |body: &Body, dictionary: &Object, key: &[u8]| match entry(body, dictionary, key)?.kind() {
            ObjectKind::Reference(reference) => Some(*reference),
            ObjectKind::Array(values) => match values.as_slice() {
                [only] => match only.kind() {
                    ObjectKind::Reference(reference) => Some(*reference),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        };
    let top = resolve(source, font).ok()?;
    let base = entry(&top, &top.value, b"/BaseFont")?;
    let base = pdf_syntax::decode_name(&top.source, base).ok()?;
    let tag = format!("/{}+", subset_tag(mark, font.object_number()));
    if !base.starts_with(tag.as_bytes()) {
        return None;
    }
    let descendant = referenced(&top, &top.value, b"/DescendantFonts")?;
    let unicode = referenced(&top, &top.value, b"/ToUnicode")?;
    let cid = resolve(source, descendant).ok()?;
    let descriptor = referenced(&cid, &cid.value, b"/FontDescriptor")?;
    let described = resolve(source, descriptor).ok()?;
    let program = referenced(&described, &described.value, b"/FontFile2")
        .or_else(|| referenced(&described, &described.value, b"/FontFile3"))?;
    Some(FontObjects {
        font,
        descendant,
        descriptor,
        program,
        unicode,
    })
}

pub(crate) fn add_font_resource(
    source: &ByteStore,
    page: Reference,
    font: Reference,
) -> Result<(String, PlannedWrite), SpikeError> {
    add_resource(source, page, (b"/Font", "F"), font)
}

pub(crate) fn add_resource(
    source: &ByteStore,
    page: Reference,
    (category, prefix): (&[u8], &str),
    object: Reference,
) -> Result<(String, PlannedWrite), SpikeError> {
    let page_body = resolve(source, page)?;
    let reference = format!("{} {} R", object.object_number(), object.generation());
    let category_text = String::from_utf8_lossy(category);
    let own = entry(&page_body, &page_body.value, b"/Resources").cloned();
    let found = match own {
        Some(value) => Some((None, value)),
        None => inherited_resources(source, &page_body)?
            .map(|(ancestor, value)| (Some(ancestor), value)),
    };
    let Some((ancestor, resources)) = found else {
        let name = format!("{prefix}1");
        let write = inserted(
            &page_body,
            &page_body.value,
            &format!("/Resources << {category_text} << /{name} {reference} >> >>"),
        )?;
        return Ok((name, write));
    };
    let holder = ancestor.unwrap_or(page_body);
    let (holder, resources) = match resources.kind() {
        ObjectKind::Dictionary(_) => (holder, resources),
        ObjectKind::Reference(reference) => {
            let body = resolve(source, *reference)?;
            let value = body.value.clone();
            (body, value)
        }
        _ => return Err(refused("the page's resources cannot be read")),
    };
    if !matches!(resources.kind(), ObjectKind::Dictionary(_)) {
        return Err(refused("the page's resources cannot be read"));
    }
    match entry(&holder, &resources, category).cloned() {
        None => {
            let name = format!("{prefix}1");
            let write = inserted(
                &holder,
                &resources,
                &format!("{category_text} << /{name} {reference} >>"),
            )?;
            Ok((name, write))
        }
        Some(fonts) => {
            let (holder, fonts) = match fonts.kind() {
                ObjectKind::Dictionary(_) => (holder, fonts),
                ObjectKind::Reference(reference) => {
                    let body = resolve(source, *reference)?;
                    let value = body.value.clone();
                    (body, value)
                }
                _ => return Err(refused("the page's resources cannot be read")),
            };
            let ObjectKind::Dictionary(entries) = fonts.kind() else {
                return Err(refused("the page's resources cannot be read"));
            };
            let taken: Vec<Vec<u8>> = entries
                .iter()
                .filter_map(|entry| entry.decoded_key(&holder.source).ok())
                .collect();
            let name = (1..=taken.len() + 1)
                .map(|number| format!("{prefix}{number}"))
                .find(|name| {
                    !taken
                        .iter()
                        .any(|key| key.get(1..) == Some(name.as_bytes()))
                })
                .unwrap_or_default();
            let write = inserted(&holder, &fonts, &format!("/{name} {reference}"))?;
            Ok((name, write))
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::BTreeMap;
    use std::fmt::Write as _;

    use pdf_bytes::{ByteStore, SourceId};
    use pdf_content::{
        Code, ContentLimits, PageContentLimits, TrueTypeFont, load_page_program_strict,
        parse_operation_sequence_strict,
    };
    use pdf_paint::{
        PaintAtomKind, PaintLimits, PaintStream, interpret_stream_sequence_with_resources,
    };
    use pdf_syntax::Reference;

    use super::{add_font_resource, embed_truetype, show_codes, to_unicode};
    use crate::incremental::{ObjectWrite, ProtectionPolicy, append_object_writes};
    use crate::plan::{PlannedBody, PlannedWrite};

    fn document(page: &str, extra: &[&str]) -> ByteStore {
        tree(
            "<< /Type /Pages /MediaBox [0 0 200 200] /Kids [3 0 R] /Count 1 >>",
            page,
            extra,
        )
    }

    fn tree(pages: &str, page: &str, extra: &[&str]) -> ByteStore {
        let content = "BT /F1 12 Tf 10 10 Td (A) Tj ET";
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            pages.to_owned(),
            page.to_owned(),
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ),
        ];
        objects.extend(extra.iter().map(|&object| object.to_owned()));
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
        }
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        ByteStore::new(SourceId::new(1), bytes)
    }

    fn written(source: &ByteStore, font: u32) -> (String, u32, String) {
        let (name, write) =
            add_font_resource(source, Reference::new(3, 0), Reference::new(font, 0))
                .expect("named");
        let PlannedBody::Direct { body } = write.body else {
            panic!("a dictionary is written directly");
        };
        (
            name,
            write.reference.object_number(),
            String::from_utf8(body).expect("text"),
        )
    }

    const PAGE: &str = "/Type /Page /Parent 2 0 R /Contents 4 0 R";

    #[test]
    fn a_font_is_named_in_whichever_dictionary_holds_the_page_fonts() {
        let source = document(
            &format!("<< {PAGE} /Resources << /Font << /F1 5 0 R /PanF1 5 0 R >> >> >>"),
            &[],
        );
        let (name, object, body) = written(&source, 9);
        assert_eq!((name.as_str(), object), ("F2", 3));
        assert!(
            body.contains("/Font << /F1 5 0 R /PanF1 5 0 R /F2 9 0 R >>"),
            "{body}"
        );

        let source = document(
            &format!("<< {PAGE} /Resources << /ProcSet [/PDF] >> >>"),
            &[],
        );
        let (name, object, body) = written(&source, 9);
        assert_eq!((name.as_str(), object), ("F1", 3));
        assert!(
            body.contains("/ProcSet [/PDF] /Font << /F1 9 0 R >> >>"),
            "{body}"
        );

        let source = document(
            &format!("<< {PAGE} /Resources 5 0 R >>"),
            &["<< /Font 6 0 R >>", "<< /F1 7 0 R >>"],
        );
        let (name, object, body) = written(&source, 9);
        assert_eq!((name.as_str(), object), ("F2", 6));
        assert_eq!(body, "<< /F1 7 0 R /F2 9 0 R >>");
    }

    #[test]
    fn a_page_is_named_where_it_inherits_its_resources_from() {
        let source = tree(
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /Resources << /Font << /F1 5 0 R >> >> >>",
            &format!("<< {PAGE} >>"),
            &[],
        );
        let (name, object, body) = written(&source, 9);
        assert_eq!((name.as_str(), object), ("F2", 2));
        assert!(body.contains("/F1 5 0 R /F2 9 0 R"), "{body}");

        let source = tree(
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /Resources 5 0 R >>",
            &format!("<< {PAGE} >>"),
            &["<< /Font << /F1 6 0 R >> >>"],
        );
        let (name, object, body) = written(&source, 9);
        assert_eq!((name.as_str(), object), ("F2", 5));
        assert_eq!(body, "<< /Font << /F1 6 0 R /F2 9 0 R >> >>");

        let source = tree(
            "<< /Type /Pages /Kids [5 0 R] /Count 1 /Resources << /Font << /F1 6 0 R >> >> >>",
            "<< /Type /Page /Parent 5 0 R /Contents 4 0 R >>",
            &["<< /Type /Pages /Parent 2 0 R /Kids [3 0 R] /Count 1 >>"],
        );
        let (_, object, _) = written(&source, 9);
        assert_eq!(object, 2, "the nearest ancestor that has any");

        let source = document(&format!("<< {PAGE} >>"), &[]);
        let (name, object, body) = written(&source, 9);
        assert_eq!((name.as_str(), object), ("F1", 3));
        assert!(
            body.contains("/Resources << /Font << /F1 9 0 R >> >>"),
            "{body}"
        );
    }

    #[test]
    fn a_page_tree_that_loops_is_refused() {
        let source = tree(
            "<< /Type /Pages /Parent 2 0 R /Kids [3 0 R] /Count 1 >>",
            &format!("<< {PAGE} >>"),
            &[],
        );
        assert!(add_font_resource(&source, Reference::new(3, 0), Reference::new(9, 0)).is_err());
    }

    fn rectangle(width: i16, height: i16) -> Vec<u8> {
        let mut glyph = Vec::new();
        for value in [1, 0, 0, width, height] {
            glyph.extend_from_slice(&value.to_be_bytes());
        }
        glyph.extend_from_slice(&3_u16.to_be_bytes());
        glyph.extend_from_slice(&0_u16.to_be_bytes());
        glyph.extend_from_slice(&[0x01; 4]);
        for delta in [0, width, 0, -width, 0, 0, height, 0] {
            glyph.extend_from_slice(&delta.to_be_bytes());
        }
        glyph
    }

    pub(crate) fn face() -> TrueTypeFont {
        let glyphs = [Vec::new(), rectangle(1000, 1000), rectangle(600, 1400)];
        let (mut glyf, mut loca) = (Vec::new(), Vec::new());
        for glyph in &glyphs {
            loca.extend_from_slice(&u16::try_from(glyf.len() / 2).expect("loca").to_be_bytes());
            glyf.extend_from_slice(glyph);
        }
        loca.extend_from_slice(&u16::try_from(glyf.len() / 2).expect("loca").to_be_bytes());
        let mut head = vec![0_u8; 54];
        head[18..20].copy_from_slice(&2000_u16.to_be_bytes());
        head[40..42].copy_from_slice(&1000_i16.to_be_bytes());
        head[42..44].copy_from_slice(&1400_i16.to_be_bytes());
        let mut maxp = vec![0_u8; 6];
        maxp[..4].copy_from_slice(&0x0000_5000_u32.to_be_bytes());
        maxp[4..6].copy_from_slice(&3_u16.to_be_bytes());
        let mut hhea = vec![0_u8; 36];
        hhea[4..6].copy_from_slice(&800_i16.to_be_bytes());
        hhea[6..8].copy_from_slice(&(-200_i16).to_be_bytes());
        hhea[34..36].copy_from_slice(&3_u16.to_be_bytes());
        let hmtx: Vec<u8> = [0_u16, 0, 1200, 0, 800, 0]
            .iter()
            .flat_map(|value| value.to_be_bytes())
            .collect();
        let tables: [(&[u8; 4], Vec<u8>); 6] = [
            (b"glyf", glyf),
            (b"head", head),
            (b"hhea", hhea),
            (b"hmtx", hmtx),
            (b"loca", loca),
            (b"maxp", maxp),
        ];
        let mut out = 0x0001_0000_u32.to_be_bytes().to_vec();
        out.extend_from_slice(&6_u16.to_be_bytes());
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
        TrueTypeFont::parse(out).expect("the face parses")
    }

    fn text_runs(source: &ByteStore) -> Vec<pdf_paint::TextShowPaint> {
        let program =
            load_page_program_strict(source, 0, PageContentLimits::default()).expect("a page");
        text_runs_in(&program, &program.resources)
    }

    fn text_runs_in(
        program: &pdf_content::PageProgram,
        resources: &pdf_content::PageResources,
    ) -> Vec<pdf_paint::TextShowPaint> {
        let sources: Vec<&ByteStore> = program.streams.iter().map(|stream| &stream.bytes).collect();
        let operations =
            parse_operation_sequence_strict(&sources, ContentLimits::default()).expect("parses");
        let streams: Vec<_> = program
            .streams
            .iter()
            .zip(&sources)
            .zip(&operations)
            .map(|((stream, source), operations)| PaintStream {
                source,
                reference: stream.reference,
                operations,
            })
            .collect();
        interpret_stream_sequence_with_resources(
            &streams,
            program.page,
            &[],
            resources,
            PaintLimits::default(),
        )
        .expect("paints")
        .atoms
        .into_iter()
        .filter_map(|atom| match atom.kind {
            PaintAtomKind::Text(text) => Some(text),
            _ => None,
        })
        .collect()
    }

    #[test]
    fn an_embedded_face_draws_its_glyphs_where_its_widths_put_them_and_reads_back() {
        let source = document(
            &format!(
                "<< {PAGE} /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 \
                 /BaseFont /Helvetica >> >> >> >>"
            ),
            &[],
        );
        let text = BTreeMap::from([(1, "ລ".to_owned()), (2, "A".to_owned())]);
        let font = embed_truetype(
            &face(),
            &text,
            (super::FontObjects::numbered_from(5), "test-0", false),
        )
        .expect("embedded");
        let (name, page) =
            add_font_resource(&source, Reference::new(3, 0), font.font).expect("named");
        let content = format!("BT /{name} 20 Tf 10 50 Td {} Tj ET", show_codes(&[1, 2]));
        let mut writes = font.writes.clone();
        writes.push(page);
        writes.push(PlannedWrite {
            reference: Reference::new(4, 0),
            body: PlannedBody::ReplacedStream {
                decoded: content.into_bytes(),
            },
        });
        let objects: Vec<ObjectWrite<'_>> = writes.iter().map(PlannedWrite::object_write).collect();
        let bytes = append_object_writes(
            &source,
            &objects,
            ProtectionPolicy::Preserve {
                credential: b"",
                restrictions: crate::incremental::Restrictions::Respect,
            },
        )
        .expect("commits");
        let committed = ByteStore::new(SourceId::new(2), bytes);

        let runs = text_runs(&committed);
        let [run] = runs.as_slice() else {
            panic!("one run, found {}", runs.len());
        };
        assert!(run.program.is_some(), "drawn from the embedded program");
        assert_eq!(
            run.glyphs
                .iter()
                .map(|glyph| glyph.glyph)
                .collect::<Vec<_>>(),
            [Some(1), Some(2)]
        );
        let read: String = run
            .glyphs
            .iter()
            .map(|glyph| {
                let code = Code {
                    value: glyph.code.value,
                    byte_len: 2,
                };
                run.text
                    .text_of(code)
                    .expect("the code has text")
                    .text
                    .clone()
            })
            .collect();
        assert_eq!(read, "ລA");
        let close = |one: [f64; 4], other: [f64; 4]| {
            one.iter()
                .zip(other)
                .all(|(one, other)| (one - other).abs() < 1e-6)
        };
        let first = run.outline_bounds_in(0..1).expect("a square");
        let second = run.outline_bounds_in(1..2).expect("a rectangle");
        assert!(close(first, [10.0, 50.0, 20.0, 60.0]), "{first:?}");
        assert!(close(second, [22.0, 50.0, 28.0, 64.0]), "{second:?}");
    }

    fn objects(writes: &[PlannedWrite]) -> Vec<ObjectWrite<'_>> {
        writes.iter().map(PlannedWrite::object_write).collect()
    }

    #[test]
    fn a_face_read_from_its_own_objects_paints_as_the_committed_one() {
        let source = document(
            &format!(
                "<< {PAGE} /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 \
                 /BaseFont /Helvetica >> >> >> >>"
            ),
            &[],
        );
        let text = BTreeMap::from([(1, "ລ".to_owned()), (2, "A".to_owned())]);
        let font = embed_truetype(
            &face(),
            &text,
            (super::FontObjects::numbered_from(5), "test-0", false),
        )
        .expect("embedded");
        let (name, page) =
            add_font_resource(&source, Reference::new(3, 0), font.font).expect("named");
        let content = format!("BT /{name} 20 Tf 10 50 Td {} Tj ET", show_codes(&[1, 2]));
        let mut face_writes = font.writes.clone();
        face_writes.push(page);
        let mut writes = face_writes.clone();
        writes.push(PlannedWrite {
            reference: Reference::new(4, 0),
            body: PlannedBody::ReplacedStream {
                decoded: content.into_bytes(),
            },
        });
        let policy = ProtectionPolicy::Preserve {
            credential: b"",
            restrictions: crate::incremental::Restrictions::Respect,
        };
        let committed = ByteStore::new(
            SourceId::new(2),
            append_object_writes(&source, &objects(&writes), policy).expect("commits"),
        );
        let alone = crate::incremental::objects_alone(&source, &objects(&face_writes), policy)
            .expect("written alone");
        assert!(alone.len() < committed.len() - source.len() + 64);

        let limits = PageContentLimits::default();
        let before = load_page_program_strict(&source, 0, limits).expect("the page");
        let entry = before
            .resources
            .font_written_in(format!("/{name}").as_bytes(), font.font, &alone, limits)
            .expect("the font reads from its own objects");
        let after = load_page_program_strict(&committed, 0, limits).expect("the page");
        let read_alone = text_runs_in(&after, &after.resources.with_fonts([entry]));
        let read_whole = text_runs(&committed);
        let [run] = read_alone.as_slice() else {
            panic!("one run, found {}", read_alone.len());
        };
        let [whole] = read_whole.as_slice() else {
            panic!("one run, found {}", read_whole.len());
        };
        assert_eq!(run.glyphs.len(), whole.glyphs.len());
        for (one, other) in run.glyphs.iter().zip(&whole.glyphs) {
            assert_eq!(one.code, other.code);
            assert_eq!(one.glyph, other.glyph);
            assert_eq!(one.matrix, other.matrix);
            let code = Code {
                value: one.code.value,
                byte_len: 2,
            };
            assert_eq!(run.text.text_of(code), whole.text.text_of(code));
        }
        assert_eq!(
            run.program.as_ref().map(|program| program.units_per_em()),
            whole.program.as_ref().map(|program| program.units_per_em())
        );
        let second = run.outline_bounds_in(1..2).expect("a rectangle");
        assert!((second[0] - 22.0).abs() < 1e-6, "{second:?}");
    }

    #[test]
    #[ignore = "writes a file for an outside check"]
    fn embeds_real_faces_for_an_outside_engine() {
        let out = std::env::var("PANPDF_EMBED_OUT").expect("PANPDF_EMBED_OUT");
        let dir =
            std::path::PathBuf::from(std::env::var("PANPDF_FONTS_DIR").expect("PANPDF_FONTS_DIR"));
        let lines = [
            ("NotoSansLao-Regular.ttf", "ສະບາຍດີ ພາສາລາວ ກ່ຽວກັບ"),
            ("SaysetthaOT-Regular.ttf", "ປະເທດລາວ ນ້ຳ ຂ້ອຍ"),
            ("NotoSansThai-Regular.ttf", "สวัสดีครับ ภาษาไทย ที่นี่"),
            ("LiberationSerif-Regular.ttf", "Hello, café naïve"),
            ("DejaVuSans.ttf", "Привет мир Ελληνικά"),
        ];
        let mut source = document(
            &format!(
                "<< {PAGE} /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> >>"
            ),
            &[],
        );
        let mut content = String::new();
        for (row, (file, text)) in lines.iter().enumerate() {
            let data = std::fs::read(dir.join(file)).expect("the face is packaged");
            let face = TrueTypeFont::parse(data.clone()).expect("parses");
            let program = pdf_content::GlyphProgram::TrueType(face.clone());
            let characters = face.characters();
            let mut meaning = BTreeMap::new();
            let mut placed = Vec::new();
            let mut pen = 0_i32;
            for word in text.split(' ') {
                let shaped = pdf_content::shape_cluster(&program, 0, word).expect("shapes");
                for glyph in &shaped {
                    let own = characters
                        .iter()
                        .find(|(id, character)| *id == glyph.glyph && word.contains(*character));
                    meaning.entry(glyph.glyph).or_insert_with(|| {
                        own.map(|(_, character)| character.to_string())
                            .unwrap_or_default()
                    });
                    placed.push((glyph.glyph, pen + glyph.x, glyph.y));
                }
                let right = shaped
                    .iter()
                    .map(|glyph| glyph.x + i32::from(face.advance_width(glyph.glyph).unwrap_or(0)))
                    .max()
                    .expect("a glyph");
                pen += right + 250;
            }
            let first = pdf_syntax_next(&source);
            let font = embed_truetype(
                &face,
                &meaning,
                (super::FontObjects::numbered_from(first), "test-0", false),
            )
            .expect("embedded");
            let (name, page) =
                add_font_resource(&source, Reference::new(3, 0), font.font).expect("named");
            let em = f64::from(face.units_per_em());
            for (glyph, x, y) in placed {
                let x = 10.0 + 12.0 * f64::from(x) / em;
                let y = 30.0f64.mul_add(-f64::from(u8::try_from(row).expect("row")), 170.0)
                    + 12.0 * f64::from(y) / em;
                let _ = writeln!(
                    content,
                    "BT /{name} 12 Tf 1 0 0 1 {x:.3} {y:.3} Tm {} Tj ET",
                    show_codes(&[glyph])
                );
            }
            let mut writes = font.writes;
            writes.push(page);
            source = commit(&source, &writes);
        }
        let stream = PlannedWrite {
            reference: Reference::new(4, 0),
            body: PlannedBody::ReplacedStream {
                decoded: content.into_bytes(),
            },
        };
        source = commit(&source, &[stream]);
        std::fs::write(out, source.as_bytes()).expect("written");
    }

    fn pdf_syntax_next(source: &ByteStore) -> u32 {
        let chain =
            pdf_syntax::parse_revision_chain_strict(source, pdf_syntax::XrefLimits::default())
                .expect("chain");
        chain
            .revisions()
            .iter()
            .flat_map(pdf_syntax::XrefSection::entries)
            .map(|entry| entry.object_number())
            .max()
            .unwrap_or(0)
            + 1
    }

    fn commit(source: &ByteStore, writes: &[PlannedWrite]) -> ByteStore {
        let objects: Vec<ObjectWrite<'_>> = writes.iter().map(PlannedWrite::object_write).collect();
        let bytes = append_object_writes(
            source,
            &objects,
            ProtectionPolicy::Preserve {
                credential: b"",
                restrictions: crate::incremental::Restrictions::Respect,
            },
        )
        .expect("commits");
        ByteStore::new(SourceId::new(source.id().get() + 1), bytes)
    }

    #[test]
    fn codes_are_glyph_ids_and_the_cmap_states_each_glyphs_text() {
        assert_eq!(show_codes(&[0x12, 0x1A2B]), "<00121A2B>");
        let cmap = to_unicode(&BTreeMap::from([
            (0x12, "ລ".to_owned()),
            (0x13, String::new()),
            (0x14, "ffi".to_owned()),
            (0x15, "😀".to_owned()),
        ]));
        assert!(
            cmap.contains(
                "3 beginbfchar\n<0012> <0EA5>\n<0014> <006600660069>\n<0015> <D83DDE00>\nendbfchar"
            ),
            "{cmap}"
        );
    }
}
