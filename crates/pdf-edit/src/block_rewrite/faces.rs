use std::collections::BTreeMap;
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_content::{GlyphProgram, PageContentLimits, ShapedGlyph};
use pdf_paint::AppliedFont;
use pdf_syntax::Reference;

use super::{Piece, Run, Style, Token, advance_of, inner_kern, unsupported};
use crate::incremental::{ObjectBody, ObjectWrite, ProtectionPolicy};
use crate::plan::{PlannedBody, PlannedWrite};
use crate::spike_move_text::SpikeError;

pub(super) struct NewFaces<'p> {
    fonts: crate::Fonts<'p>,
    program: &'p pdf_content::PageProgram,
    restrictions: crate::Restrictions,
    faces: Vec<Face>,
}

const FACE_RUNS: usize = usize::MAX / 2;

struct Face {
    program: Arc<GlyphProgram>,
    index: u32,
    sha256: String,
    family: String,
    run: usize,
    meaning: BTreeMap<u16, String>,
    typed: bool,
}

type Named = (String, Reference);

#[derive(Clone)]
pub(super) struct Settled {
    pub writes: Vec<PlannedWrite>,
    pub fonts: Vec<pdf_content::ResourceEntry>,
}

impl<'p> NewFaces<'p> {
    pub(super) const fn new(
        fonts: crate::Fonts<'p>,
        program: &'p pdf_content::PageProgram,
        restrictions: crate::Restrictions,
    ) -> Self {
        Self {
            fonts,
            program,
            restrictions,
            faces: Vec::new(),
        }
    }

    pub(super) fn request(
        &self,
        style: &Style<'_>,
        run: usize,
    ) -> Option<pdf_content::FontRequest> {
        let applied = style.run(run).text.font.as_ref()?;
        self.program
            .resources
            .font(&applied.value.name)
            .and_then(|resource| resource.face_request().ok().flatten())
    }

    pub(super) fn face_for(
        &self,
        request: &pdf_content::FontRequest,
    ) -> Option<pdf_content::SubstitutedFace> {
        self.fonts?.primary_face(request)
    }

    pub(super) const fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }

    pub(super) fn typed_families(&self) -> Vec<String> {
        let mut families: Vec<String> = Vec::new();
        for face in self.faces.iter().filter(|face| face.typed) {
            if !face.family.is_empty() && !families.contains(&face.family) {
                families.push(face.family.clone());
            }
        }
        families
    }

    pub(super) fn programs(&self) -> Vec<Arc<GlyphProgram>> {
        self.faces
            .iter()
            .map(|face| Arc::clone(&face.program))
            .collect()
    }

    pub(super) fn piece(
        &mut self,
        style: &Style<'_>,
        run: usize,
        cluster: &str,
        group: usize,
    ) -> Result<Piece, SpikeError> {
        let no_code = || SpikeError::RetypeUnsupported(crate::retype::NO_CODE);
        let fonts = self.fonts.ok_or_else(no_code)?;
        let request = self.request(style, run).ok_or_else(no_code)?;
        let first = cluster.chars().next().ok_or_else(no_code)?;
        if cluster.chars().any(right_to_left) {
            return Err(unsupported(
                "right-to-left text needs the bidirectional algorithm (a later slice)",
            ));
        }
        let face = fonts
            .fallback_face(&request, first)
            .ok_or_else(|| unsupported("no face on this machine shows a typed character"))?;
        self.piece_in(run, cluster, group, &face, true)
    }

    pub(super) fn piece_in(
        &mut self,
        run: usize,
        cluster: &str,
        group: usize,
        face: &pdf_content::SubstitutedFace,
        typed: bool,
    ) -> Result<Piece, SpikeError> {
        if cluster.chars().any(right_to_left) {
            return Err(unsupported(
                "right-to-left text needs the bidirectional algorithm (a later slice)",
            ));
        }
        let embeddable = crate::new_font::Embeddable::of(&face.program)
            .ok_or_else(|| unsupported("the face for a typed character cannot be embedded yet"))?;
        let shaped =
            pdf_content::shape_cluster(&face.program, face.identity.face_index, cluster)
                .ok_or_else(|| unsupported("the face chosen cannot shape a typed character"))?;
        if shaped.first().is_some_and(|glyph| glyph.x != 0) {
            return Err(unsupported(
                "a typed cluster its face places before its pen (a later slice)",
            ));
        }
        let index = if let Some(index) = self.faces.iter().position(|known| {
            known.sha256 == face.identity.sha256
                && known.index == face.identity.face_index
                && known.run == run
        }) {
            index
        } else {
            self.faces.push(Face {
                program: Arc::clone(&face.program),
                index: face.identity.face_index,
                sha256: face.identity.sha256.clone(),
                family: face.identity.family.clone(),
                run,
                meaning: BTreeMap::new(),
                typed,
            });
            self.faces.len() - 1
        };
        self.faces[index].typed |= typed;
        if shaped
            .iter()
            .any(|glyph| embeddable.code(glyph.glyph).is_none())
        {
            return Err(unsupported(
                "the face for a typed character cannot be embedded yet",
            ));
        }
        let meanings = meanings(embeddable.metrics(), &shaped, cluster);
        let known = &mut self.faces[index].meaning;
        for (glyph, meaning) in shaped.iter().zip(meanings) {
            let slot = known.entry(glyph.glyph).or_default();
            if slot.is_empty() {
                *slot = meaning;
            }
        }
        Ok(Piece {
            codes: Vec::new(),
            text: cluster.to_owned(),
            advance: 0.0,
            kept: None,
            group,
            style: FACE_RUNS + index,
            adjust: Vec::new(),
            next: None,
            break_before: false,
            shaped,
            rise: Vec::new(),
            underline: false,
        })
    }

    pub(super) fn settle(
        &self,
        source: &ByteStore,
        (page, page_index): (Reference, usize),
        style: &mut Style<'_>,
        tokens: &mut [Token],
    ) -> Result<Settled, SpikeError> {
        let (writes, document, named) = self.embed(source, page)?;
        let base = style.runs.len();
        let fonts = self.add_runs(&document, page_index, &named, style)?;
        self.fill_pieces(style, tokens, base)?;
        Ok(Settled { writes, fonts })
    }

    fn embed(
        &self,
        source: &ByteStore,
        page: Reference,
    ) -> Result<(Vec<PlannedWrite>, ByteStore, Vec<Named>), SpikeError> {
        let mark_of = |face: &Face| crate::new_font::face_mark(&face.sha256, face.index);
        let mut marks: Vec<String> = Vec::new();
        for face in &self.faces {
            let mark = mark_of(face);
            if !marks.contains(&mark) {
                marks.push(mark);
            }
        }
        let mut document = source.clone();
        let mut writes: Vec<PlannedWrite> = Vec::new();
        let mut placed = Vec::with_capacity(marks.len());
        for mark in &marks {
            let sharing: Vec<&Face> = self
                .faces
                .iter()
                .filter(|face| mark_of(face) == *mark)
                .collect();
            let (step, name, font) = self.embed_face(source, &document, page, mark, &sharing)?;
            document = commit(&document, &step, self.restrictions)?;
            writes.retain(|write| step.iter().all(|new| new.reference != write.reference));
            writes.extend(step);
            placed.push((name, font));
        }
        let named = self
            .faces
            .iter()
            .map(|face| {
                let at = marks
                    .iter()
                    .position(|known| *known == mark_of(face))
                    .unwrap_or(0);
                placed[at].clone()
            })
            .collect();
        Ok((writes, document, named))
    }

    fn embed_face(
        &self,
        source: &ByteStore,
        document: &ByteStore,
        page: Reference,
        mark: &str,
        sharing: &[&Face],
    ) -> Result<(Vec<PlannedWrite>, String, Reference), SpikeError> {
        let embeddable = crate::new_font::Embeddable::of(&sharing[0].program)
            .ok_or_else(|| unsupported("the face for a typed character cannot be embedded yet"))?;
        let existing = self.program.resources.fonts().iter().find_map(|entry| {
            let objects = crate::new_font::embedded_face(source, entry.reference()?, mark)?;
            Some((entry, objects))
        });
        let mut meaning = BTreeMap::new();
        if let Some((entry, _)) = existing {
            let had = entry
                .to_unicode()
                .map_err(|_| unsupported("a face this page embedded before cannot be read"))?;
            for (code, text) in had.entries() {
                if let Some(glyph) = u16::try_from(code.value)
                    .ok()
                    .and_then(|code| embeddable.glyph(code))
                {
                    meaning.insert(glyph, text.text.clone());
                }
            }
        }
        for face in sharing {
            for (glyph, text) in &face.meaning {
                let slot = meaning.entry(*glyph).or_insert_with(String::new);
                if slot.is_empty() {
                    slot.clone_from(text);
                }
            }
        }
        if let Some((entry, objects)) = existing {
            let font = crate::new_font::embed(embeddable, &meaning, (objects, mark, true))?;
            let name = String::from_utf8_lossy(entry.name()).into_owned();
            return Ok((font.writes, name, objects.font));
        }
        let objects = crate::new_font::FontObjects::numbered_from(next_object_number(document)?);
        let font = crate::new_font::embed(embeddable, &meaning, (objects, mark, false))?;
        let (name, holder) = crate::new_font::add_font_resource(document, page, objects.font)?;
        let mut step = font.writes;
        step.push(holder);
        Ok((step, format!("/{name}"), objects.font))
    }

    fn add_runs(
        &self,
        document: &ByteStore,
        page_index: usize,
        named: &[(String, Reference)],
        style: &mut Style<'_>,
    ) -> Result<Vec<pdf_content::ResourceEntry>, SpikeError> {
        let unread = || unsupported("the page does not read back with a typed character's face");
        let program = pdf_content::load_page_program_with_password(
            document,
            page_index,
            PageContentLimits::default(),
            b"",
        )
        .map_err(|_| unread())?;
        let mut fonts = Vec::with_capacity(named.len());
        for (face, (name, reference)) in self.faces.iter().zip(named) {
            let entry = program
                .resources
                .font(name.as_bytes())
                .filter(|entry| entry.reference() == Some(*reference))
                .ok_or_else(unread)?;
            let font = entry.font().map_err(|_| unread())?;
            fonts.push(entry.clone());
            let caret = style.run(face.run);
            let mut text = caret.text.clone();
            let applied = text
                .font
                .as_mut()
                .ok_or_else(|| unsupported("the block has no current font"))?;
            applied.value = AppliedFont {
                name: name.as_bytes().to_vec(),
                reference: Some(*reference),
            };
            let run = Run {
                reference: caret.reference,
                font,
                text,
                fill: caret.fill.clone(),
                stroke: caret.stroke.clone(),
                fill_rgb: caret.fill_rgb,
                line_width: caret.line_width,
                stroke_like_fill: caret.stroke_like_fill,
                shear: caret.shear,
                line: face
                    .typed
                    .then(|| face.program.vertical_metrics())
                    .flatten(),
            };
            style.runs.push(run);
        }
        Ok(fonts)
    }

    fn fill_pieces(
        &self,
        style: &Style<'_>,
        tokens: &mut [Token],
        base: usize,
    ) -> Result<(), SpikeError> {
        let undecoded = || unsupported("a typed character's face does not decode its codes");
        for token in tokens {
            let Token::Cluster(piece) = token else {
                continue;
            };
            if piece.shaped.is_empty() {
                continue;
            }
            let Some(index) = piece.style.checked_sub(FACE_RUNS) else {
                continue;
            };
            let face = self.faces.get(index).ok_or_else(undecoded)?;
            piece.style = base + index;
            let Some(embeddable) = crate::new_font::Embeddable::of(&face.program) else {
                return Err(undecoded());
            };
            let wanted: Vec<u16> = piece
                .shaped
                .iter()
                .map(|glyph| embeddable.code(glyph.glyph))
                .collect::<Option<_>>()
                .ok_or_else(undecoded)?;
            let bytes: Vec<u8> = wanted.iter().flat_map(|code| code.to_be_bytes()).collect();
            let codes = style
                .run(piece.style)
                .font
                .source_codes(&bytes)
                .map_err(|_| undecoded())?;
            if codes.len() != piece.shaped.len()
                || codes
                    .iter()
                    .zip(&wanted)
                    .any(|(code, wanted)| code.value != u32::from(*wanted))
            {
                return Err(undecoded());
            }
            piece.adjust = adjustments(embeddable.metrics(), &piece.shaped);
            piece.rise = if piece.shaped.iter().all(|glyph| glyph.y == 0) {
                Vec::new()
            } else {
                let size = style.run(piece.style).text.font_size.value;
                let em = f64::from(embeddable.metrics().units_per_em());
                piece
                    .shaped
                    .iter()
                    .map(|glyph| f64::from(glyph.y) * size / em)
                    .collect()
            };
            piece.advance = codes
                .iter()
                .map(|code| advance_of(style, piece.style, code))
                .sum();
            piece.codes = codes;
            piece.advance += inner_kern(style, piece);
        }
        Ok(())
    }
}

const fn right_to_left(character: char) -> bool {
    matches!(
        character as u32,
        0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x1_0800..=0x1_0FFF | 0x1_E800..=0x1_EFFF
    )
}

pub(crate) fn meanings(
    face: &pdf_content::TrueTypeFont,
    shaped: &[ShapedGlyph],
    cluster: &str,
) -> Vec<String> {
    meanings_by(|character| face.glyph_for_char(character), shaped, cluster)
}

fn meanings_by(
    glyph_for: impl Fn(char) -> Option<u16>,
    shaped: &[ShapedGlyph],
    cluster: &str,
) -> Vec<String> {
    let mut out = vec![String::new(); shaped.len()];
    let mut left = Vec::new();
    let reach = |out: &[String], character: char| {
        let reached = glyph_for(character);
        shaped
            .iter()
            .zip(out)
            .position(|(glyph, text)| Some(glyph.glyph) == reached && text.is_empty())
    };
    for character in cluster.chars() {
        if let Some(position) = reach(&out, character) {
            out[position].push(character);
            continue;
        }
        if let Some([first, second]) = sara_am(character) {
            if let Some(one) = reach(&out, first)
                && let Some(other) = reach(&out, second).filter(|other| *other != one)
            {
                out[one].push(first);
                out[other].push(second);
                continue;
            }
            if let Some(other) = reach(&out, second) {
                out[other].push(second);
                left.push(first);
                continue;
            }
        }
        left.push(character);
    }
    let empty: Vec<usize> = (0..out.len())
        .filter(|index| out[*index].is_empty())
        .collect();
    for (index, character) in left.into_iter().enumerate() {
        let position = empty.get(index).or(empty.last()).copied().unwrap_or(0);
        out[position].push(character);
    }
    out
}

const fn sara_am(character: char) -> Option<[char; 2]> {
    match character {
        '\u{0E33}' => Some(['\u{0E4D}', '\u{0E32}']),
        '\u{0EB3}' => Some(['\u{0ECD}', '\u{0EB2}']),
        _ => None,
    }
}

fn adjustments(face: &pdf_content::TrueTypeFont, shaped: &[ShapedGlyph]) -> Vec<f64> {
    let em = f64::from(face.units_per_em());
    let thousandths = |units: i32| f64::from(units) * 1000.0 / em;
    let mut out = vec![0.0; shaped.len()];
    for index in 1..shaped.len() {
        let before = &shaped[index - 1];
        let width = crate::new_font::width(face, before.glyph);
        out[index - 1] = thousandths(before.x) + width - thousandths(shaped[index].x);
    }
    if let Some(last) = shaped.last() {
        let total: i32 = shaped.iter().map(|glyph| glyph.advance).sum();
        let width = crate::new_font::width(face, last.glyph);
        out[shaped.len() - 1] = thousandths(last.x) + width - thousandths(total);
    }
    out
}

pub(crate) fn next_object_number(source: &ByteStore) -> Result<u32, SpikeError> {
    let chain = pdf_syntax::parse_revision_chain_strict(source, pdf_syntax::XrefLimits::default())
        .map_err(|_| unsupported("the document's objects cannot be counted"))?;
    Ok(chain
        .revisions()
        .iter()
        .flat_map(pdf_syntax::XrefSection::entries)
        .map(|entry| entry.object_number())
        .max()
        .unwrap_or(0)
        + 1)
}

pub(crate) fn commit(
    source: &ByteStore,
    writes: &[PlannedWrite],
    restrictions: crate::Restrictions,
) -> Result<ByteStore, SpikeError> {
    commit_with(
        source,
        writes,
        (b"", restrictions),
        crate::incremental::TrailerExtras::default(),
    )
}

pub(crate) fn commit_with(
    source: &ByteStore,
    writes: &[PlannedWrite],
    (credential, restrictions): (&[u8], crate::Restrictions),
    extras: crate::incremental::TrailerExtras,
) -> Result<ByteStore, SpikeError> {
    let objects: Vec<ObjectWrite<'_>> = writes
        .iter()
        .map(|write| ObjectWrite {
            reference: write.reference,
            body: match &write.body {
                PlannedBody::ReplacedStream { decoded } => ObjectBody::ReplacedStream { decoded },
                PlannedBody::NewStream {
                    dictionary,
                    decoded,
                } => ObjectBody::NewStream {
                    dictionary,
                    decoded,
                },
                PlannedBody::Direct { body } => ObjectBody::Direct { body },
            },
        })
        .collect();
    let bytes = crate::incremental::append_object_writes_bounded(
        source,
        &objects,
        ProtectionPolicy::Preserve {
            credential,
            restrictions,
        },
        extras,
        pdf_syntax::XrefLimits::default(),
    )
    .map_err(|_| unsupported("a typed character's face cannot be added to this document"))?;
    Ok(ByteStore::owning(
        SourceId::new(source.id().get().wrapping_add(1)),
        bytes,
    ))
}

#[cfg(test)]
mod tests {
    use pdf_content::ShapedGlyph;

    use super::{adjustments, meanings, meanings_by, right_to_left};
    use crate::new_font::tests::face;

    const fn glyph(glyph: u16, x: i32) -> ShapedGlyph {
        advancing(glyph, x, 0)
    }

    const fn advancing(glyph: u16, x: i32, advance: i32) -> ShapedGlyph {
        ShapedGlyph {
            glyph,
            x,
            y: 0,
            advance,
        }
    }

    #[test]
    fn the_number_after_a_glyph_puts_the_next_where_the_shaper_placed_it() {
        let shaped = [
            advancing(1, 0, 1000),
            advancing(2, 1000, 900),
            advancing(1, 1900, 1200),
        ];
        assert_eq!(adjustments(&face(), &shaped), [100.0, -50.0, 0.0]);
        let marked = [advancing(1, 0, 1200), glyph(2, 1100)];
        assert_eq!(adjustments(&face(), &marked), [50.0, 350.0]);
    }

    #[test]
    fn characters_no_glyph_is_reached_for_go_one_each_to_the_glyphs_with_none() {
        let two = [glyph(1, 0), glyph(2, 1000)];
        assert_eq!(meanings(&face(), &two, "ab"), ["a", "b"]);
        assert_eq!(meanings(&face(), &two, "abc"), ["a", "bc"]);
        assert_eq!(meanings(&face(), &two[..1], "ab"), ["ab"]);
    }

    #[test]
    fn sara_am_leaves_the_aa_glyph_only_the_aa_where_the_nikhahit_is_joined_to_a_tone() {
        let lao = |character| match character {
            'ນ' => Some(15),
            '້' => Some(68),
            'າ' => Some(34),
            _ => None,
        };
        let joined = [glyph(15, 0), glyph(99, 614), glyph(34, 614)];
        assert_eq!(meanings_by(lao, &joined, "ນ້ຳ"), ["ນ", "້ໍ", "າ"]);
        let thai = |character| match character {
            'น' => Some(1),
            '้' => Some(2),
            'ํ' => Some(3),
            'า' => Some(4),
            _ => None,
        };
        let apart = [glyph(1, 0), glyph(2, 600), glyph(3, 600), glyph(4, 600)];
        assert_eq!(meanings_by(thai, &apart, "น้ำ"), ["น", "้", "ํ", "า"]);
    }

    #[test]
    fn arabic_and_hebrew_are_right_to_left_and_lao_thai_and_latin_are_not() {
        assert!(right_to_left('\u{0627}') && right_to_left('\u{05D0}'));
        assert!(!right_to_left('\u{0EA5}') && !right_to_left('\u{0E01}') && !right_to_left('a'));
    }
}
