use std::sync::Arc;

use pdf_content::{
    Font, FontProvider, FontRequest, FontSubstitution, GlyphProgram, MappingCensus, MappingRoute,
    SUBSTITUTION_POLICY, SourceCode, SubstitutedFace, ToUnicode, UnresolvedReason,
    character_for_glyph_name, is_combining_mark, is_ignorable, standard_encoding_name,
};

use crate::geometry::Matrix;
use crate::graph::{PositionedGlyph, SubstitutedGlyph, TextShowElement};
use crate::state::TextState;

#[must_use]
pub fn code_advance(text: &TextState, code: &SourceCode, width_scale: f64) -> f64 {
    (code.width * width_scale * text.font_size.value
        + text.character_spacing.value
        + if code.cid.is_none() && code.value == 32 {
            text.word_spacing.value
        } else {
            0.0
        })
        * (text.horizontal_scaling.value / 100.0)
}

#[must_use]
pub fn position_text(
    text: &TextState,
    elements: &[TextShowElement],
    start: Matrix,
    program: Option<&GlyphProgram>,
    font: &Font,
    width_scale: f64,
) -> (Vec<PositionedGlyph>, Matrix) {
    let scale = text.horizontal_scaling.value / 100.0;
    let parameters = Matrix {
        a: text.font_size.value * scale,
        b: 0.0,
        c: 0.0,
        d: text.font_size.value,
        e: 0.0,
        f: text.rise.value,
    };
    let mut cursor = start;
    let mut glyphs = Vec::new();
    for element in elements {
        match element {
            TextShowElement::Codes { codes, .. } => {
                for code in codes {
                    glyphs.push(PositionedGlyph {
                        code: code.clone(),
                        text_matrix: cursor,
                        glyph: program.and_then(|program| select_glyph(program, font, code)),
                        matrix: cursor.multiply(parameters),
                        procedure: None,
                        substituted: Vec::new(),
                        silent: false,
                        unresolved: None,
                    });
                    cursor = cursor.multiply(Matrix {
                        e: code_advance(text, code, width_scale),
                        ..Matrix::IDENTITY
                    });
                }
            }
            TextShowElement::Adjustment { value, .. } => {
                cursor = cursor.multiply(Matrix {
                    e: -value / 1000.0 * text.font_size.value * scale,
                    ..Matrix::IDENTITY
                });
            }
        }
    }
    (glyphs, cursor)
}

fn select_glyph(program: &GlyphProgram, font: &Font, code: &SourceCode) -> Option<u16> {
    if let Some(cid) = code.cid {
        let cid = u16::try_from(cid).ok()?;
        if program.is_cid_keyed() {
            return program.glyph_for_cid(cid);
        }
        return Some(cid);
    }
    let code = u8::try_from(code.value).ok()?;
    let named = match font {
        Font::Simple(simple) => simple
            .glyph_name(code)
            .and_then(|name| program.glyph_for_name(name)),
        Font::Composite(_) => None,
    };
    named.or_else(|| program.glyph_for_code(code))
}

const MAX_CLUSTER: usize = 4;

pub(crate) fn covering_primary(
    glyphs: &[PositionedGlyph],
    request: &FontRequest,
    provider: &dyn FontProvider,
    font: &Font,
    text: &ToUnicode,
) -> Option<SubstitutedFace> {
    let recovered = legacy_multibyte::recover(glyphs, request);
    for (index, glyph) in glyphs.iter().enumerate() {
        let meaning = match recovered.get(index) {
            Some(Ok(character)) => Some(character.to_string()),
            Some(Err(_)) => None,
            None => meaning_of(font, text, &glyph.code, request)
                .ok()
                .map(|(meaning, _)| meaning),
        };
        for character in meaning
            .into_iter()
            .flat_map(|text| text.chars().collect::<Vec<_>>())
        {
            if !is_ignorable(character)
                && let Some(face) = provider.fallback_face(request, character)
                && face
                    .program
                    .glyph_for_char(character)
                    .is_some_and(|glyph| glyph != 0)
            {
                return Some(face);
            }
        }
    }
    None
}

pub fn substitute_run(
    glyphs: &mut [PositionedGlyph],
    request: &Arc<FontRequest>,
    primary: SubstitutedFace,
    provider: &dyn FontProvider,
    font: &Font,
    text: &ToUnicode,
) -> FontSubstitution {
    let mut faces = vec![primary];
    let mut unmapped: Vec<(char, u32)> = Vec::new();
    let mut census = MappingCensus {
        source_codes: u32::try_from(glyphs.len()).unwrap_or(u32::MAX),
        ..MappingCensus::default()
    };
    let recovered = legacy_multibyte::recover(glyphs, request);
    for (index, glyph) in glyphs.iter_mut().enumerate() {
        let found = match recovered.get(index) {
            Some(Ok(character)) => Ok((
                String::from(*character),
                MappingRoute::RecoveredLegacyEncoding,
            )),
            Some(Err(reason)) => Err(*reason),
            None => meaning_of(font, text, &glyph.code, request),
        };
        let (meaning, route) = match found {
            Ok(found) => found,
            Err(reason) => {
                glyph.silent = reason == UnresolvedReason::MultibyteContinuation;
                glyph.unresolved = (!glyph.silent).then_some(reason);
                census.count_unresolved(reason);
                continue;
            }
        };
        let characters: Vec<char> = meaning
            .chars()
            .filter(|character| !is_ignorable(*character))
            .take(MAX_CLUSTER + 1)
            .collect();
        if characters.is_empty() {
            glyph.unresolved = Some(UnresolvedReason::NoEvidence);
            census.count_unresolved(UnresolvedReason::NoEvidence);
            continue;
        }
        if characters.len() > MAX_CLUSTER {
            census.count_route(route);
            glyph.unresolved = Some(UnresolvedReason::ClusterTooLong);
            census.count_unresolved(UnresolvedReason::ClusterTooLong);
            continue;
        }
        if characters.len() > 1
            && let Some(shaped) = shaped_cluster(&mut faces, provider, request, &characters)
        {
            census.count_route(route);
            census.clusters_drawn += 1;
            census.outlines_drawn += u32::try_from(shaped.len()).unwrap_or(0);
            glyph.substituted = shaped;
            continue;
        }
        if characters.len() > 1 && !characters[1..].iter().all(|c| is_combining_mark(*c)) {
            census.count_route(route);
            glyph.unresolved = Some(UnresolvedReason::MultipleBaseCharacters);
            census.count_unresolved(UnresolvedReason::MultipleBaseCharacters);
            count(&mut unmapped, characters[0]);
            continue;
        }
        census.count_route(route);
        let drawn_before = glyph.substituted.len();
        let mut complete = true;
        for character in characters {
            if let Some(drawn) = resolve(&mut faces, provider, request, character) {
                glyph.substituted.push(drawn);
            } else {
                complete = false;
                count(&mut unmapped, character);
            }
        }
        if complete {
            census.clusters_drawn += 1;
            census.outlines_drawn +=
                u32::try_from(glyph.substituted.len() - drawn_before).unwrap_or(0);
        } else {
            glyph.substituted.truncate(drawn_before);
            glyph.unresolved = Some(UnresolvedReason::NoFaceCoverage);
            census.count_unresolved(UnresolvedReason::NoFaceCoverage);
        }
    }
    let primary = faces.remove(0);
    unmapped.sort_unstable();
    let unknown_codes = census.evidence_gaps();
    FontSubstitution {
        request: Arc::clone(request),
        primary,
        fallbacks: faces,
        unmapped,
        unknown_codes,
        census,
        policy: SUBSTITUTION_POLICY,
    }
}

fn shaped_cluster(
    faces: &mut Vec<SubstitutedFace>,
    provider: &dyn FontProvider,
    request: &Arc<FontRequest>,
    characters: &[char],
) -> Option<Vec<SubstitutedGlyph>> {
    for character in characters {
        let _ = resolve(faces, provider, request, *character);
    }
    let text: String = characters.iter().collect();
    faces.iter().enumerate().find_map(|(index, face)| {
        let glyphs = pdf_content::shape_cluster(&face.program, face.identity.face_index, &text)?;
        let index = u16::try_from(index).ok()?;
        Some(
            glyphs
                .into_iter()
                .map(|item| SubstitutedGlyph {
                    face: index,
                    glyph: item.glyph,
                    offset: [item.x, item.y],
                    shaped: true,
                })
                .collect::<Vec<_>>(),
        )
    })
}

mod legacy_multibyte {
    use pdf_content::{FontRequest, LegacyByte, UnresolvedReason, decode_legacy_bytes};

    use crate::graph::PositionedGlyph;

    pub(super) fn recover(
        glyphs: &[PositionedGlyph],
        request: &FontRequest,
    ) -> Vec<Result<char, UnresolvedReason>> {
        let Some(encoding) = request.legacy_multibyte_encoding() else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(glyphs.len());
        for operand in operands(glyphs) {
            let bytes: Vec<u8> = operand
                .iter()
                .map(|glyph| u8::try_from(glyph.code.value).unwrap_or(0))
                .collect();
            for answer in decode_legacy_bytes(encoding, &bytes) {
                out.push(match answer {
                    LegacyByte::Meaning(character) => Ok(character),
                    LegacyByte::Unresolved(reason) => Err(reason),
                });
            }
        }
        debug_assert_eq!(out.len(), glyphs.len());
        out
    }

    fn operands(glyphs: &[PositionedGlyph]) -> Vec<&[PositionedGlyph]> {
        let mut spans = Vec::new();
        let mut start = 0;
        for index in 1..glyphs.len() {
            let previous = &glyphs[index - 1].code;
            let current = &glyphs[index].code;
            let continues = previous.bytes.len() == 1
                && current.bytes.len() == 1
                && current.byte_offset == previous.byte_offset + 1;
            if !continues {
                spans.push(&glyphs[start..index]);
                start = index;
            }
        }
        if start < glyphs.len() {
            spans.push(&glyphs[start..]);
        }
        spans
    }
}

fn resolve(
    faces: &mut Vec<SubstitutedFace>,
    provider: &dyn FontProvider,
    request: &Arc<FontRequest>,
    character: char,
) -> Option<SubstitutedGlyph> {
    for (index, face) in faces.iter().enumerate() {
        if let Some(glyph) = face.program.glyph_for_char(character) {
            return Some(SubstitutedGlyph {
                face: u16::try_from(index).ok()?,
                glyph,
                offset: [0, 0],
                shaped: false,
            });
        }
    }
    if let Some(found) = provider.fallback_face(request, character)
        && let Some(glyph) = found.program.glyph_for_char(character)
        && let Ok(index) = u16::try_from(faces.len())
    {
        faces.push(found);
        return Some(SubstitutedGlyph {
            face: index,
            glyph,
            offset: [0, 0],
            shaped: false,
        });
    }
    None
}

fn count(unmapped: &mut Vec<(char, u32)>, character: char) {
    if let Some(slot) = unmapped.iter_mut().find(|(found, _)| *found == character) {
        slot.1 += 1;
        return;
    }
    unmapped.push((character, 1));
}

fn meaning_of(
    font: &Font,
    text: &ToUnicode,
    code: &SourceCode,
    request: &FontRequest,
) -> Result<(String, MappingRoute), UnresolvedReason> {
    if request.has_ambiguous_cjk_encoding() {
        return Err(UnresolvedReason::AmbiguousEncoding);
    }
    if let Font::Simple(simple) = font
        && let Ok(byte) = u8::try_from(code.value)
        && let Some(name) = simple.glyph_name(byte)
        && let Some(character) = request.standard_face.map_or_else(
            || character_for_glyph_name(name),
            |face| face.character_for_name(name),
        )
    {
        return Ok((String::from(character), MappingRoute::EncodingGlyphName));
    }
    if let Font::Simple(simple) = font
        && let Some(face) = request.standard_face
        && face.is_symbolic()
        && let Ok(byte) = u8::try_from(code.value)
        && simple.glyph_name(byte).is_none()
        && let Some(table) = face.built_in_encoding()
        && let Some(name) = table[usize::from(byte)]
        && let Some(character) = face.character_for_name(name)
    {
        return Ok((String::from(character), MappingRoute::StandardFaceEncoding));
    }
    let key = pdf_content::Code {
        value: code.value,
        byte_len: code.bytes.len() + code.completed_bytes,
    };
    if let Some(meaning) = text.text_of(key) {
        return Ok((meaning.text.clone(), MappingRoute::ToUnicode));
    }
    if request.code_is_utf16()
        && code.completed_bytes == 0
        && let Some(character) = char::from_u32(code.value)
    {
        return Ok((String::from(character), MappingRoute::Utf16Code));
    }
    if let Font::Simple(simple) = font
        && !request.flags.symbolic()
        && let Ok(byte) = u8::try_from(code.value)
        && simple.glyph_name(byte).is_none()
        && let Some(name) = standard_encoding_name(byte)
        && let Some(character) = character_for_glyph_name(name)
    {
        return Ok((String::from(character), MappingRoute::StandardEncoding));
    }
    Err(UnresolvedReason::NoEvidence)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};
    use pdf_content::{Font, GlyphProgram, SourceCode};

    use super::select_glyph;
    use crate::test_fixtures::{DISAGREEING_CFF, hex_fixture};

    #[test]
    fn a_font_dictionary_encoding_outranks_the_program_it_selects_from() {
        let program = GlyphProgram::parse(hex_fixture(DISAGREEING_CFF)).expect("CFF fixture");
        assert_eq!(program.glyph_for_name(b"A"), Some(1));
        assert_eq!(program.glyph_for_name(b"alpha"), Some(2));
        assert_eq!(program.glyph_for_code(0x41), Some(1));

        let code = |value: u32| SourceCode {
            bytes: vec![u8::try_from(value).expect("one-byte code")],
            value,
            byte_offset: 0,
            cid: None,
            mapping_span: None,
            completed_bytes: 0,
            width: 0.0,
        };
        let simple = |dictionary: &[u8]| {
            let mut bytes = b"<< /Type /Font /Subtype /Type1 /FirstChar 65 /LastChar 65\
                              /Widths [500] "
                .to_vec();
            bytes.extend_from_slice(dictionary);
            bytes.extend_from_slice(b" >>");
            let source = ByteStore::new(SourceId::new(83), Arc::<[u8]>::from(bytes));
            let object =
                pdf_syntax::ObjectParser::new(&source, 0, pdf_syntax::ParseLimits::default())
                    .parse_next()
                    .expect("font dictionary parses")
                    .expect("font dictionary present");
            Font::Simple(
                pdf_content::parse_simple_font(&source, &object, &|_| {
                    panic!("the fixture has no indirect entries")
                })
                .expect("simple font"),
            )
        };

        assert_eq!(
            select_glyph(
                &program,
                &simple(b"/Encoding << /Differences [65 /alpha] >>"),
                &code(0x41)
            ),
            Some(2)
        );
        assert_eq!(
            select_glyph(
                &program,
                &simple(b"/Encoding << /Differences [66 /alpha] >>"),
                &code(0x41)
            ),
            Some(1)
        );
        assert_eq!(
            select_glyph(
                &program,
                &simple(b"/Encoding << /Differences [65 /nosuchglyph] >>"),
                &code(0x41)
            ),
            Some(1)
        );
        assert_eq!(select_glyph(&program, &simple(b""), &code(0x41)), Some(1));
    }

    #[test]
    fn a_cid_keyed_program_selects_through_its_charset() {
        let program = GlyphProgram::parse(cid_keyed_cff(&[302, 17, 65_535])).expect("a CFF");
        assert!(program.is_cid_keyed());

        assert_eq!(select_glyph(&program, &any_font(), &cid_code(302)), Some(1));
        assert_eq!(select_glyph(&program, &any_font(), &cid_code(17)), Some(2));
        assert_eq!(
            select_glyph(&program, &any_font(), &cid_code(65_535)),
            Some(3)
        );
    }

    #[test]
    fn a_cid_the_subset_does_not_carry_selects_nothing() {
        let program = GlyphProgram::parse(cid_keyed_cff(&[302, 17, 65_535])).expect("a CFF");

        assert_eq!(select_glyph(&program, &any_font(), &cid_code(1)), None);
        assert_eq!(select_glyph(&program, &any_font(), &cid_code(303)), None);
    }

    #[test]
    fn a_program_that_is_not_cid_keyed_still_uses_the_cid_as_an_index() {
        let program = GlyphProgram::parse(hex_fixture(DISAGREEING_CFF)).expect("CFF fixture");
        assert!(!program.is_cid_keyed());

        assert_eq!(select_glyph(&program, &any_font(), &cid_code(1)), Some(1));
        assert_eq!(
            select_glyph(&program, &any_font(), &cid_code(302)),
            Some(302)
        );
    }

    fn any_font() -> Font {
        let mut bytes = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec();
        bytes.push(b'\n');
        let source = ByteStore::new(SourceId::new(7), Arc::<[u8]>::from(bytes));
        let object = pdf_syntax::ObjectParser::new(&source, 0, pdf_syntax::ParseLimits::default())
            .parse_next()
            .expect("font dictionary parses")
            .expect("font dictionary present");
        Font::Simple(
            pdf_content::parse_simple_font(&source, &object, &|_| {
                panic!("the fixture has no indirect entries")
            })
            .expect("simple font"),
        )
    }

    fn cid_keyed_cff(cids: &[u16]) -> Vec<u8> {
        const SQUARE: &[u8] = &[139, 139, 21, 140, 139, 5, 139, 140, 5, 14];

        fn index(items: &[&[u8]]) -> Vec<u8> {
            if items.is_empty() {
                return vec![0, 0];
            }
            let mut out = u16::try_from(items.len())
                .expect("count")
                .to_be_bytes()
                .to_vec();
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

        let charstrings: Vec<&[u8]> = std::iter::repeat_n(SQUARE, cids.len() + 1).collect();
        let names = index(&[b"Test"]);
        let strings = index(&[b"Adobe", b"Identity"]);
        let global_subrs = index(&[]);
        let charstring_index = index(&charstrings);

        let ros_len = 3 * 5 + 2;
        let top_dict_len = ros_len + 6 + 6;
        let dict_index_len = 2 + 1 + 2 + top_dict_len;
        let before_charstrings =
            4 + names.len() + dict_index_len + strings.len() + global_subrs.len();
        let charset_at = before_charstrings + charstring_index.len();

        let mut top_dict = number(391);
        top_dict.extend_from_slice(&number(392));
        top_dict.extend_from_slice(&number(0));
        top_dict.extend_from_slice(&[12, 30]);
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
        out.extend_from_slice(&strings);
        out.extend_from_slice(&global_subrs);
        out.extend_from_slice(&charstring_index);
        assert_eq!(out.len(), charset_at);
        out.push(0);
        for cid in cids {
            out.extend_from_slice(&cid.to_be_bytes());
        }
        out
    }

    fn cid_code(cid: u32) -> SourceCode {
        SourceCode {
            bytes: cid.to_be_bytes()[2..].to_vec(),
            value: cid,
            byte_offset: 0,
            cid: Some(cid),
            mapping_span: None,
            completed_bytes: 0,
            width: 0.0,
        }
    }
}
