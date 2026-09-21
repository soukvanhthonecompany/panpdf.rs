use crate::spike_move_text::{SpikeError, clip_allows_move_of};
use pdf_content::PageResources;
use pdf_paint::{Code, Matrix, PositionedGlyph, TextRenderingMode, TextShowElement, TextShowPaint};
use std::ops::Range;

pub(crate) struct Prepared {
    pub glyphs: Vec<PositionedGlyph>,
    pub pieces: Vec<Range<usize>>,
    pub retained: Vec<Option<usize>>,
    pub shift: (f64, f64),
}

pub fn replacement_shift(
    resources: &PageResources,
    text: &TextShowPaint,
    range: &Range<usize>,
    typed: &str,
) -> Result<(f64, f64), SpikeError> {
    prepare(resources, text, range, typed).map(|prepared| prepared.shift)
}

fn validate(text: &TextShowPaint, range: &Range<usize>, typed: &str) -> Result<(), SpikeError> {
    if range.start > range.end || range.end > text.glyphs.len() || text.glyphs.is_empty() {
        return Err(SpikeError::GlyphRangeOutsideRun);
    }
    if typed.is_empty() || typed.len() > 4096 || typed.chars().any(char::is_control) {
        return Err(refused(
            "type up to 4096 characters of text, without line breaks",
        ));
    }
    if text.type3 {
        return Err(refused(TYPE3));
    }
    if text.program.is_none() && text.substitution.is_none() {
        return Err(refused(NO_FACE));
    }
    if !matches!(
        text.state.text.rendering_mode.value,
        TextRenderingMode::Fill | TextRenderingMode::Stroke | TextRenderingMode::FillStroke
    ) {
        return Err(refused(INVISIBLE_OR_CLIPS));
    }
    let forward = |value: f64| matches!(value.partial_cmp(&0.0), Some(std::cmp::Ordering::Greater));
    if !forward(text.state.text.font_size.value)
        || !forward(text.state.text.horizontal_scaling.value)
    {
        return Err(refused(MIRRORED));
    }
    for glyph in &text.glyphs {
        let code = Code {
            value: glyph.code.value,
            byte_len: glyph.code.bytes.len(),
        };
        let Some(meaning) = text.text.text_of(code) else {
            return Err(refused("the existing text has no reliable Unicode mapping"));
        };
        if meaning.text.chars().count() != 1 {
            return Err(refused(
                "a code here stands for more than one character, which cannot be retyped a glyph at a time",
            ));
        }
        if !matches!(
            glyph.code.width.partial_cmp(&0.0),
            Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
        ) {
            return Err(refused(
                "the existing run has a glyph whose advance this engine cannot read",
            ));
        }
    }
    Ok(())
}

pub(crate) fn check_typed_codes(
    codes: &[pdf_content::SourceCode],
    typed: &str,
) -> Result<(), SpikeError> {
    if codes.len() != typed.chars().count()
        || codes
            .iter()
            .any(|code| !code.width.is_finite() || code.width < 0.0)
    {
        return Err(refused(
            "the encoded text is not one readable glyph per character",
        ));
    }
    if codes.iter().zip(typed.chars()).any(|(code, character)| {
        code.width <= 0.0
            && !pdf_content::is_combining_mark(character)
            && !pdf_content::is_ignorable(character)
    }) {
        return Err(refused(NO_WIDTH));
    }
    Ok(())
}

pub(crate) const TYPE3: &str =
    "this font draws its letters with procedures (Type 3), which typing cannot add to";

pub(crate) const NO_FACE: &str =
    "the file does not carry this font and no face on this machine stands in for it";

pub(crate) const MIRRORED: &str =
    "this text is drawn mirrored (a negative size or width), which typing cannot follow yet";

pub(crate) const INVISIBLE_OR_CLIPS: &str =
    "this text is invisible or used as a clipping path, so typing would change nothing you can see";

pub(crate) const NO_SUBSTITUTE_GLYPH: &str =
    "the face standing in for this font does not draw a typed character";

pub(crate) const NO_CODE: &str = "a requested character has no code in this font";

pub(crate) const NO_WIDTH: &str =
    "this font gives a typed character no width, so it would be drawn over the next letter";

pub(crate) const MANY_CODES: &str = "a requested character maps to multiple codes (G08)";

fn refused(reason: &'static str) -> SpikeError {
    SpikeError::RetypeUnsupported(reason)
}

#[derive(Debug)]
struct RunFace(pdf_content::SubstitutedFace);

impl pdf_content::FontProvider for RunFace {
    fn primary_face(
        &self,
        _request: &pdf_content::FontRequest,
    ) -> Option<pdf_content::SubstitutedFace> {
        Some(self.0.clone())
    }

    fn fallback_face(
        &self,
        _request: &pdf_content::FontRequest,
        character: char,
    ) -> Option<pdf_content::SubstitutedFace> {
        self.0
            .program
            .glyph_for_char(character)
            .filter(|glyph| *glyph != 0)
            .map(|_| self.0.clone())
    }

    fn description(&self) -> String {
        "the face this run is already drawn from".to_owned()
    }
}

fn substitute_inserted(
    text: &TextShowPaint,
    font: &pdf_content::Font,
    inserted: &mut [PositionedGlyph],
) -> Result<(), SpikeError> {
    let substitution = text.substitution.as_ref().ok_or_else(|| refused(NO_FACE))?;
    let provider = RunFace(substitution.primary.clone());
    pdf_paint::substitute_run(
        inserted,
        &substitution.request,
        substitution.primary.clone(),
        &provider,
        font,
        &text.text,
    );
    if inserted
        .iter()
        .any(|glyph| glyph.substituted.is_empty() || glyph.unresolved.is_some())
    {
        return Err(refused(NO_SUBSTITUTE_GLYPH));
    }
    Ok(())
}

#[must_use]
pub fn substituted_family(text: &TextShowPaint) -> Option<String> {
    let substitution = text.substitution.as_ref()?;
    Some(substitution.primary.identity.family.clone())
}

fn font_of(
    resources: &PageResources,
    text: &TextShowPaint,
) -> Result<pdf_content::Font, SpikeError> {
    let applied = text
        .state
        .text
        .font
        .as_ref()
        .ok_or_else(|| refused("no current font"))?;
    let resource = resources
        .font(&applied.value.name)
        .ok_or_else(|| refused("font resource unavailable"))?;
    if resource.reference() != applied.value.reference {
        return Err(refused("font resource identity differs"));
    }
    resource
        .font()
        .map_err(|_| refused("font metrics unavailable"))
}

pub(crate) fn prepare(
    resources: &PageResources,
    text: &TextShowPaint,
    range: &Range<usize>,
    typed: &str,
) -> Result<Prepared, SpikeError> {
    validate(text, range, typed)?;
    let font = font_of(resources, text)?;
    let bytes = encode(text, typed)?;
    let codes = font
        .source_codes(&bytes)
        .map_err(|_| refused("the encoded text does not decode in this font"))?;
    check_typed_codes(&codes, typed)?;
    let program = text.program.as_deref();
    let end = pdf_paint::position_text(
        &text.state.text,
        &text.elements,
        text.matrices.text.value,
        program,
        &font,
        0.001,
    )
    .1;
    let start = text
        .glyphs
        .get(range.start)
        .map_or(end, |glyph| glyph.text_matrix);
    let old_end = text
        .glyphs
        .get(range.end)
        .map_or(end, |glyph| glyph.text_matrix);
    let source_span = match text
        .elements
        .first()
        .ok_or_else(|| refused("no source text operation"))?
    {
        TextShowElement::Codes { source_span, .. }
        | TextShowElement::Adjustment { source_span, .. } => *source_span,
    };
    let elements = [TextShowElement::Codes {
        source_span,
        decoded_bytes: bytes,
        codes,
    }];
    let (mut inserted, new_end) =
        pdf_paint::position_text(&text.state.text, &elements, start, program, &font, 0.001);
    if inserted
        .iter()
        .any(|glyph| !glyph.matrix.e.is_finite() || !glyph.matrix.f.is_finite())
    {
        return Err(refused(
            "the replacement contains an unresolved glyph or invalid position",
        ));
    }
    if program.is_some() {
        if inserted.iter().any(|glyph| glyph.glyph.is_none()) {
            return Err(refused(
                "the replacement contains an unresolved glyph or invalid position",
            ));
        }
    } else {
        substitute_inserted(text, &font, &mut inserted)?;
    }
    let shift = (new_end.e - old_end.e, new_end.f - old_end.f);
    let mut glyphs = text.glyphs[..range.start].to_vec();
    let mut retained: Vec<_> = (0..range.start).map(Some).collect();
    retained.extend(std::iter::repeat_n(None, inserted.len()));
    glyphs.extend(inserted);
    for (index, glyph) in text.glyphs.iter().enumerate().skip(range.end) {
        let mut glyph = glyph.clone();
        glyph.matrix.e += shift.0;
        glyph.matrix.f += shift.1;
        glyph.text_matrix.e += shift.0;
        glyph.text_matrix.f += shift.1;
        glyphs.push(glyph);
        retained.push(Some(index));
    }
    let mut expected = text.clone();
    expected.glyphs.clone_from(&glyphs);
    if expected.outline_bounds().is_none() && !expected.draws_no_ink() {
        return Err(refused("replacement ink bounds cannot be established"));
    }
    clip_allows_move_of(&expected, 0.0, 0.0)?;
    let pieces = continuous_pieces(text, &font, &glyphs);
    let ctm = text.state.ctm.value;
    Ok(Prepared {
        glyphs,
        pieces,
        retained,
        shift: (
            ctm.a.mul_add(shift.0, ctm.c * shift.1),
            ctm.b.mul_add(shift.0, ctm.d * shift.1),
        ),
    })
}

fn continuous_pieces(
    text: &TextShowPaint,
    font: &pdf_content::Font,
    glyphs: &[PositionedGlyph],
) -> Vec<Range<usize>> {
    let source_span = match text.elements.first() {
        Some(
            TextShowElement::Codes { source_span, .. }
            | TextShowElement::Adjustment { source_span, .. },
        ) => *source_span,
        None => return (0..glyphs.len()).map(|glyph| glyph..glyph + 1).collect(),
    };
    let mut pieces = Vec::new();
    let mut piece_start = 0;
    for index in 1..glyphs.len() {
        let before = &glyphs[index - 1];
        let (_, pen) = pdf_paint::position_text(
            &text.state.text,
            &[TextShowElement::Codes {
                source_span,
                decoded_bytes: before.code.bytes.clone(),
                codes: vec![before.code.clone()],
            }],
            before.text_matrix,
            text.program.as_deref(),
            font,
            0.001,
        );
        if !same_matrix(pen, glyphs[index].text_matrix) {
            pieces.push(piece_start..index);
            piece_start = index;
        }
    }
    if !glyphs.is_empty() {
        pieces.push(piece_start..glyphs.len());
    }
    pieces
}

fn same_matrix(one: Matrix, other: Matrix) -> bool {
    [
        one.a - other.a,
        one.b - other.b,
        one.c - other.c,
        one.d - other.d,
        one.e - other.e,
        one.f - other.f,
    ]
    .iter()
    .all(|difference| difference.abs() <= 1e-9)
}

pub(crate) fn encode(text: &TextShowPaint, typed: &str) -> Result<Vec<u8>, SpikeError> {
    let mut bytes = Vec::new();
    for character in typed.chars() {
        let choices = text.text.codes_to_write(&character.to_string());
        match choices.as_slice() {
            [code] => bytes.extend(code.bytes()),
            [] => return Err(refused(NO_CODE)),
            _ => return Err(refused(MANY_CODES)),
        }
    }
    Ok(bytes)
}
