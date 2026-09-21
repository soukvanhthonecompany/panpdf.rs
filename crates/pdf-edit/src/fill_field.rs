use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;

use pdf_bytes::ByteStore;
use pdf_paint::{PaintAtomKind, PaintLimits, Point};
use pdf_syntax::{ObjectKind, Reference};

use crate::form::{BorderStyle, FieldKind, FieldValue, FormField, Quadding, Reader};
use crate::new_font::{Embeddable, FontObjects, embed, face_mark};
use crate::new_text::{LINE_EM, NewText, PlacedLine, meanings, place, shape};
use crate::plan::{Capability, Effect, Plan, PlannedBody, PlannedWrite};
use crate::spike_move_text::{PlannerPage, SpikeError};

const PADDING: f64 = 2.0;
const CAP_HEIGHT: f64 = 0.72;
const LADDER: [f64; 20] = [
    72.0, 60.0, 48.0, 36.0, 28.0, 24.0, 20.0, 18.0, 16.0, 14.0, 12.0, 11.0, 10.0, 9.0, 8.0, 7.0,
    6.0, 5.0, 4.0, 3.0,
];
pub const MOST_CHARACTERS: usize = 32_768;
const TOLERANCE: f64 = 0.05;

pub(crate) type Entry = (&'static [u8], String);

fn refused(reason: &'static str) -> SpikeError {
    SpikeError::RetypeUnsupported(reason)
}

#[derive(Clone, Debug)]
pub(crate) struct Filled<'a> {
    pub(crate) widget: Reference,
    pub(crate) value: &'a FieldValue,
}

pub(crate) fn plan_fill_field(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    filled: &Filled<'_>,
) -> Result<Plan, SpikeError> {
    let reader = Reader::open(source, b"")?;
    if reader.is_protected() {
        return Err(refused(
            "this document is protected, and a value written into it would not be readable",
        ));
    }
    let field = crate::form::read_field(&reader, filled.widget)
        .filter(|field| crate::form::field_nodes(&reader).contains(&field.field))
        .ok_or_else(|| refused("this is not a field of the document's form"))?;
    if field.read_only {
        return Err(refused("this field is marked as one that may be read only"));
    }
    let writes = match field.kind {
        FieldKind::Push => return Err(refused("a pushbutton holds no value to fill in")),
        FieldKind::Signature => {
            return Err(refused(
                "a signature field is signed rather than filled in, which this does not do yet",
            ));
        }
        FieldKind::Checkbox | FieldKind::Radio => turned(source, &reader, &field, filled.value)?,
        FieldKind::Text | FieldKind::Combo | FieldKind::List => typed(
            source,
            page,
            page_index,
            (&field, &Beside::default()),
            filled.value,
        )?,
    };
    let target = page
        .program
        .streams
        .first()
        .map(|stream| stream.reference)
        .ok_or_else(|| refused("a page with no content stream carries no form"))?;
    Ok(Plan::new(
        Capability::Exact,
        writes,
        Effect {
            page_index,
            moved: Vec::new(),
            target_stream: target,
            declared_region: Some(field.rect),
        },
    ))
}

fn turned(
    source: &ByteStore,
    reader: &Reader,
    field: &FormField,
    value: &FieldValue,
) -> Result<Vec<PlannedWrite>, SpikeError> {
    let state = match value {
        FieldValue::Empty => "Off".to_owned(),
        FieldValue::State(state) | FieldValue::Text(state) => state.clone(),
    };
    if state != "Off" && !field.states.contains(&state) {
        return Err(refused(
            "this box has no such state to be turned to, so turning it there would show nothing",
        ));
    }
    if !is_a_name(&state) {
        return Err(refused("a button's state is a name, not free text"));
    }
    let mut entries: BTreeMap<(u32, u16), (Reference, Vec<Entry>)> = BTreeMap::new();
    entries
        .entry(key_of(field.field))
        .or_insert_with(|| (field.field, Vec::new()))
        .1
        .push((b"/V", format!("/{state}")));
    for widget in widgets_of(reader, field) {
        let shown = if widget == field.widget {
            state.clone()
        } else {
            "Off".to_owned()
        };
        entries
            .entry(key_of(widget))
            .or_insert_with(|| (widget, Vec::new()))
            .1
            .push((b"/AS", format!("/{shown}")));
    }
    entries
        .into_values()
        .map(|(reference, pairs)| set_entries(source, reference, &pairs))
        .collect()
}

const fn key_of(reference: Reference) -> (u32, u16) {
    (reference.object_number(), reference.generation())
}

fn widgets_of(reader: &Reader, field: &FormField) -> Vec<Reference> {
    let Some(node) = reader.at(field.field) else {
        return vec![field.widget];
    };
    let Some(kids) = reader.entry(&node, b"/Kids") else {
        return vec![field.field];
    };
    let ObjectKind::Array(items) = kids.value.kind() else {
        return vec![field.widget];
    };
    let widgets: Vec<Reference> = items
        .iter()
        .filter_map(|item| match item.kind() {
            ObjectKind::Reference(reference) => Some(*reference),
            _ => None,
        })
        .collect();
    if widgets.is_empty() {
        vec![field.widget]
    } else {
        widgets
    }
}

fn is_a_name(state: &str) -> bool {
    !state.is_empty()
        && state.len() < 128
        && state
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Beside {
    pub(crate) field: Vec<Entry>,
    pub(crate) widget: Vec<Entry>,
    pub(crate) keep_value: bool,
}

pub(crate) fn typed(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    (field, beside): (&FormField, &Beside),
    value: &FieldValue,
) -> Result<Vec<PlannedWrite>, SpikeError> {
    let text = match value {
        FieldValue::Empty => String::new(),
        FieldValue::Text(text) | FieldValue::State(text) => text.clone(),
    };
    checked(field, &text)?;
    if field.kind == FieldKind::List && !text.is_empty() && !field.options.contains(&text) {
        return Err(refused(
            "a list offers what may be chosen, and this is not one of them",
        ));
    }

    let mut writes = Vec::new();
    let listing = field.kind == FieldKind::List && !field.options.is_empty();
    if text.is_empty() && !listing {
        let stream = Reference::new(crate::block_rewrite::next_object_number(source)?, 0);
        writes.push(PlannedWrite {
            reference: stream,
            body: PlannedBody::NewStream {
                dictionary: empty_dictionary(field).into_bytes(),
                decoded: [box_of(field), b"/Tx BMC EMC\n".to_vec()].concat(),
            },
        });
        writes.extend(answered(source, (field, beside), &text, stream)?);
        return merged(writes);
    }

    let shown = if listing {
        field.options.join("\n")
    } else if field.password {
        "*".repeat(text.chars().count())
    } else {
        text.clone()
    };
    let layout = if listing {
        let mut every_line = field.clone();
        every_line.multiline = true;
        every_line.quadding = Quadding::Left;
        every_line
    } else {
        field.clone()
    };
    let appearance = read_da(&field.appearance);
    let face = crate::new_text::face_for(
        &page,
        &NewText {
            paragraph: crate::plan::ParagraphLayout::default(),
            frame: field.rect,
            text: &shown,
            family: appearance.family,
            size: 1.0,
            bold: appearance.bold,
            italic: false,
            fill: Some(appearance.colour),
            opacity: 1.0,
            turn: pdf_paint::Matrix::IDENTITY,
            share_from: None,
        },
    )?;
    let embeddable = Embeddable::of(&face.program)
        .ok_or_else(|| refused("the face a form field is drawn with cannot be embedded yet"))?;
    let box_ = inner(field)?;
    let asked = if listing {
        Some(appearance.size.unwrap_or(12.0))
    } else {
        appearance.size
    };
    let (size, lines) = laid_out(&shown, &face, embeddable, (&layout, box_, asked))?;
    let chosen = highlight(listing.then_some((field, &text)), &lines, (box_, size));

    let objects = FontObjects::numbered_from(crate::block_rewrite::next_object_number(source)?);
    let clusters = shape(&shown, &face, embeddable, size)?;
    let mark = face_mark(&face.identity.sha256, face.identity.face_index);
    let font = embed(
        embeddable,
        &meanings(&clusters, embeddable),
        (objects, &mark, false),
    )?;
    writes.extend(font.writes);
    let stream = Reference::new(objects.unicode.object_number() + 1, 0);
    writes.push(PlannedWrite {
        reference: stream,
        body: PlannedBody::NewStream {
            dictionary: stream_dictionary(field, objects.font).into_bytes(),
            decoded: [
                box_of(field),
                chosen,
                drawn(&lines, (size, appearance.colour), field.rect),
            ]
            .concat(),
        },
    });
    writes.extend(answered(source, (field, beside), &text, stream)?);
    let writes = merged(writes)?;

    prove_shown(
        source,
        &writes,
        (page_index, field, page.restrictions),
        &lines,
    )?;
    Ok(writes)
}

fn answered(
    source: &ByteStore,
    (field, beside): (&FormField, &Beside),
    text: &str,
    stream: Reference,
) -> Result<Vec<PlannedWrite>, SpikeError> {
    let value = (
        b"/V".as_slice(),
        if text.is_empty() {
            String::new()
        } else {
            pdf_text_string(text)
        },
    );
    let appearance = (
        b"/AP".as_slice(),
        format!(
            "<< /N {} {} R >>",
            stream.object_number(),
            stream.generation()
        ),
    );
    let on_field: Vec<Entry> = std::iter::once(value)
        .filter(|_| !beside.keep_value)
        .chain(beside.field.clone())
        .collect();
    let on_widget: Vec<Entry> = std::iter::once(appearance)
        .chain(beside.widget.clone())
        .collect();
    if key_of(field.field) == key_of(field.widget) {
        let both: Vec<Entry> = on_field.into_iter().chain(on_widget).collect();
        return Ok(vec![set_entries(source, field.field, &both)?]);
    }
    Ok(vec![
        set_entries(source, field.field, &on_field)?,
        set_entries(source, field.widget, &on_widget)?,
    ])
}

fn checked(field: &FormField, text: &str) -> Result<(), SpikeError> {
    if text.chars().count() > MOST_CHARACTERS {
        return Err(refused("this is more text than one field holds"));
    }
    if field
        .max_len
        .is_some_and(|most| text.chars().count() > most)
    {
        return Err(refused("this field takes fewer characters than that"));
    }
    if text.contains('\n') && !field.multiline {
        return Err(refused("this field holds one line"));
    }
    if text.chars().any(|letter| {
        letter != '\n' && (letter.is_control() || letter == '\u{feff}' || letter == '\u{fffe}')
    }) {
        return Err(refused(
            "a control character cannot be written into a field",
        ));
    }
    Ok(())
}

fn inner(field: &FormField) -> Result<[f64; 4], SpikeError> {
    let rect = field.rect;
    let (width, height) = (rect[2] - rect[0], rect[3] - rect[1]);
    let border = if field.border.is_some() {
        match field.border_style {
            BorderStyle::Beveled | BorderStyle::Inset => 2.0 * field.border_width,
            _ => field.border_width,
        }
    } else {
        0.0
    };
    let pad = PADDING.max(border + 1.0);
    if !(width.is_finite() && height.is_finite()) || width <= 2.0 * pad || height <= pad {
        return Err(refused("this field's box is too small to write in"));
    }
    Ok([pad, pad, width - pad, height - pad])
}

fn laid_out(
    text: &str,
    face: &pdf_content::SubstitutedFace,
    embeddable: Embeddable<'_>,
    (field, box_, asked): (&FormField, [f64; 4], Option<f64>),
) -> Result<(f64, Vec<PlacedLine>), SpikeError> {
    if let Some(size) = asked {
        let lines = at_size(text, face, embeddable, (field, box_, size))?;
        return Ok((size, lines));
    }
    for size in LADDER {
        if let Ok(lines) = at_size(text, face, embeddable, (field, box_, size))
            && fits(&lines, box_, size)
        {
            return Ok((size, lines));
        }
    }
    Err(refused("this value does not fit in this field at any size"))
}

fn fits(lines: &[PlacedLine], box_: [f64; 4], size: f64) -> bool {
    lines.iter().all(|line| {
        line.advance <= box_[2] - box_[0] + TOLERANCE
            && line.origin.y >= box_[1]
            && line.origin.y + size <= box_[3] + TOLERANCE
    })
}

fn at_size(
    text: &str,
    face: &pdf_content::SubstitutedFace,
    embeddable: Embeddable<'_>,
    (field, box_, size): (&FormField, [f64; 4], f64),
) -> Result<Vec<PlacedLine>, SpikeError> {
    if !(size.is_finite() && size > 0.0) {
        return Err(refused("a field's text has a size"));
    }
    let clusters = shape(text, face, embeddable, size)?;
    if let (true, Some(cells)) = (
        field.flags & crate::form::flags::COMB != 0 && !field.multiline && !field.password,
        field.max_len,
    ) {
        return Ok(combed(&clusters, (box_, size), cells));
    }
    let top = if field.multiline {
        box_[3]
    } else {
        (box_[1] + box_[3] + size * CAP_HEIGHT) / 2.0 + size - size * CAP_HEIGHT
    };
    let right = if field.kind == FieldKind::List {
        box_[0] + 1e6
    } else {
        box_[2]
    };
    let mut lines = place(
        &clusters,
        [box_[0], box_[1], right, top],
        size,
        (crate::layout::Alignment::Start, &[]),
    )?;
    let width = box_[2] - box_[0];
    for line in &mut lines {
        let shift = match field.quadding {
            Quadding::Left => 0.0,
            Quadding::Centre => (width - line.advance) / 2.0,
            Quadding::Right => width - line.advance,
        };
        if shift > 0.0 {
            line.origin.x += shift;
            for pen in &mut line.pens {
                pen.x += shift;
            }
        }
    }
    Ok(lines)
}

fn highlight(
    listing: Option<(&FormField, &str)>,
    lines: &[PlacedLine],
    (box_, size): ([f64; 4], f64),
) -> Vec<u8> {
    let Some((field, text)) = listing else {
        return Vec::new();
    };
    field
        .options
        .iter()
        .position(|option| option == text)
        .and_then(|at| lines.get(at))
        .map(|line| {
            format!(
                "q 0.6 0.75 0.95 rg {} {} {} {} re f Q\n",
                box_[0] - 1.0,
                line.origin.y - size * 0.25,
                box_[2] - box_[0] + 2.0,
                size * LINE_EM
            )
            .into_bytes()
        })
        .unwrap_or_default()
}

fn combed(
    paragraphs: &[Vec<crate::new_text::Cluster>],
    (box_, size): ([f64; 4], f64),
    cells: usize,
) -> Vec<PlacedLine> {
    #[allow(clippy::cast_precision_loss, reason = "a comb is a few dozen cells")]
    let cell = (box_[2] - box_[0]) / cells as f64;
    let baseline = (box_[1] + box_[3] - size * CAP_HEIGHT) / 2.0;
    paragraphs
        .iter()
        .flatten()
        .take(cells)
        .enumerate()
        .filter(|(_, cluster)| !cluster.codes.is_empty())
        .map(|(at, cluster)| {
            #[allow(clippy::cast_precision_loss, reason = "a comb is a few dozen cells")]
            let x = box_[0] + cell * (at as f64 + 0.5) - cluster.advance / 2.0;
            let origin = Point { x, y: baseline };
            PlacedLine {
                origin,
                codes: cluster.codes.clone(),
                glyphs: cluster.glyphs.clone(),
                pens: cluster
                    .glyphs
                    .iter()
                    .map(|glyph| Point {
                        x: origin.x + glyph.x,
                        y: origin.y + glyph.y,
                    })
                    .collect(),
                advance: cluster.advance,
                turn: pdf_paint::Matrix::IDENTITY,
            }
        })
        .collect()
}

fn drawn(lines: &[PlacedLine], (size, colour): (f64, [f64; 3]), rect: [f64; 4]) -> Vec<u8> {
    let [red, green, blue] = colour;
    let (width, height) = (rect[2] - rect[0], rect[3] - rect[1]);
    let mut out = Vec::with_capacity(lines.len() * 64 + 128);
    out.extend_from_slice(b"/Tx BMC\nq\n");
    out.extend_from_slice(format!("1 1 {} {} re W n\n", width - 2.0, height - 2.0).as_bytes());
    out.extend_from_slice(
        format!(
            "BT /F1 {size} Tf {pitch} TL 0 Tc 0 Tw 100 Tz 0 Ts {red} {green} {blue} rg\n",
            pitch = size * LINE_EM,
        )
        .as_bytes(),
    );
    for line in lines {
        out.extend_from_slice(
            format!("1 0 0 1 {} {} Tm ", line.origin.x, line.origin.y).as_bytes(),
        );
        out.extend_from_slice(&crate::new_text::shows(line, size).0);
    }
    out.extend_from_slice(b"ET\nQ\nEMC\n");
    out
}

pub(crate) fn box_of(field: &FormField) -> Vec<u8> {
    crate::field_look::frame(&look_of(field, false)).into_bytes()
}

pub(crate) fn look_of(field: &FormField, round: bool) -> crate::field_look::Look {
    crate::field_look::Look {
        width: field.rect[2] - field.rect[0],
        height: field.rect[3] - field.rect[1],
        border: field.border,
        fill: field.background,
        border_width: field.border_width,
        style: field.border_style,
        round,
    }
}

fn stream_dictionary(field: &FormField, font: Reference) -> String {
    format!(
        "/Type /XObject /Subtype /Form /FormType 1 /BBox [0 0 {width} {height}] \
         /Resources << /Font << /F1 {number} {generation} R >> >>",
        width = field.rect[2] - field.rect[0],
        height = field.rect[3] - field.rect[1],
        number = font.object_number(),
        generation = font.generation(),
    )
}

fn empty_dictionary(field: &FormField) -> String {
    format!(
        "/Type /XObject /Subtype /Form /FormType 1 /BBox [0 0 {width} {height}] /Resources << >>",
        width = field.rect[2] - field.rect[0],
        height = field.rect[3] - field.rect[1],
    )
}

fn prove_shown(
    source: &ByteStore,
    writes: &[PlannedWrite],
    (page_index, field, restrictions): (usize, &FormField, crate::Restrictions),
    lines: &[PlacedLine],
) -> Result<(), SpikeError> {
    let document = crate::block_rewrite::commit_writes(source, writes, restrictions)?;
    let reading = crate::spike_move_text::read_page(&document, page_index, b"", None)?;
    let painted = pdf_paint::interpret_annotations(
        &reading.program.annotations,
        reading.program.page,
        reading.program.geometry.rotate,
        &reading.program.resources,
        PaintLimits::default(),
        None,
    );
    let shown = painted
        .iter()
        .find(|annotation| annotation.reference == Some(field.widget))
        .ok_or_else(|| refused("the field filled in is no longer on the page"))?;
    let texts: Vec<_> = shown
        .graph
        .atoms
        .iter()
        .filter_map(|atom| match &atom.kind {
            PaintAtomKind::Text(text) => Some(text),
            _ => None,
        })
        .collect();
    let counts: Vec<usize> = lines
        .iter()
        .map(|line| crate::new_text::shows(line, 1.0).1)
        .collect();
    if texts.len() != counts.iter().sum::<usize>() {
        return Err(refused("the value written does not draw in its field"));
    }
    let mut texts = texts.into_iter();
    for (line, count) in lines.iter().zip(counts) {
        let drawn: Vec<_> = texts
            .by_ref()
            .take(count)
            .flat_map(|text| text.glyphs.iter().map(move |glyph| (text, glyph)))
            .collect();
        if drawn.len() != line.pens.len() {
            return Err(refused("the value written does not draw every letter"));
        }
        for ((text, glyph), wanted) in drawn.into_iter().zip(&line.pens) {
            let at = text
                .state
                .ctm
                .value
                .multiply(glyph.matrix)
                .transform(Point { x: 0.0, y: 0.0 });
            if (at.x - wanted.x - field.rect[0]).abs() > TOLERANCE
                || (at.y - wanted.y - field.rect[1]).abs() > TOLERANCE
            {
                return Err(refused(
                    "the value written does not draw where it was laid out",
                ));
            }
        }
    }
    Ok(())
}

fn merged(writes: Vec<PlannedWrite>) -> Result<Vec<PlannedWrite>, SpikeError> {
    let mut seen = HashSet::new();
    for write in &writes {
        if !seen.insert(key_of(write.reference)) {
            return Err(refused("this field would be written twice in one revision"));
        }
    }
    Ok(writes)
}

pub(crate) fn set_entries(
    source: &ByteStore,
    reference: Reference,
    pairs: &[(&[u8], String)],
) -> Result<PlannedWrite, SpikeError> {
    let mut edit = crate::object_edit::ObjectEdit::of(source, reference)?;
    let dictionary = edit.value();
    for (key, text) in pairs {
        if text.is_empty() {
            edit.unset(&dictionary, key)?;
        } else {
            edit.set(&dictionary, key, &as_written_into(source, reference, text)?)?;
        }
    }
    edit.written()
}

fn as_written_into(
    source: &ByteStore,
    reference: Reference,
    text: &str,
) -> Result<String, SpikeError> {
    if !text.contains('(') && !text.contains('<') {
        return Ok(text.to_owned());
    }
    let reader = crate::form::Reader::open(source, b"")?;
    let Some(security) = reader.security.as_ref() else {
        return Ok(text.to_owned());
    };
    if crate::incremental::strings_are_plain(&reader.index, reference) {
        return Ok(text.to_owned());
    }
    let written = crate::incremental::with_strings_encrypted(security, reference, text.as_bytes())
        .map_err(SpikeError::Write)?;
    String::from_utf8(written).map_err(|_| refused("this value cannot be written as text"))
}

pub(crate) fn pdf_text_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 4 + 8);
    out.push('<');
    out.push_str("FEFF");
    for unit in text.encode_utf16() {
        let _ = write!(out, "{unit:04X}");
    }
    out.push('>');
    out
}

pub(crate) fn colour_in(appearance: &[u8]) -> [f64; 3] {
    read_da(appearance).colour
}

pub(crate) fn size_in(appearance: &[u8]) -> Option<f64> {
    read_da(appearance).size
}

pub(crate) fn with_colour(appearance: &[u8], [red, green, blue]: [f64; 3]) -> String {
    let text = String::from_utf8_lossy(appearance);
    let mut kept: Vec<&str> = Vec::new();
    let mut operands: Vec<&str> = Vec::new();
    for token in text.split_whitespace() {
        match token {
            "g" | "rg" | "k" | "G" | "RG" | "K" => operands.clear(),
            _ if token.starts_with('/') || token.bytes().all(is_number_byte) => {
                operands.push(token);
            }
            _ => {
                kept.append(&mut operands);
                kept.push(token);
            }
        }
    }
    kept.append(&mut operands);
    let mut out = kept.join(" ");
    if !out.is_empty() {
        out.push(' ');
    }
    let _ = write!(out, "{red} {green} {blue} rg");
    out
}

pub(crate) fn with_size(appearance: &[u8], size: Option<f64>) -> String {
    let written = size.map_or_else(|| "0".to_owned(), |size| format!("{size}"));
    let text = String::from_utf8_lossy(appearance);
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if let Some(at) = tokens.iter().position(|token| *token == "Tf")
        && at >= 2
    {
        let mut out: Vec<String> = tokens.iter().map(|token| (*token).to_owned()).collect();
        out[at - 1] = written;
        return out.join(" ");
    }
    format!("/Helv {written} Tf {}", tokens.join(" "))
        .trim_end()
        .to_owned()
}

#[derive(Clone, Copy, Debug)]
struct DefaultAppearance {
    size: Option<f64>,
    colour: [f64; 3],
    family: &'static str,
    bold: bool,
}

fn read_da(bytes: &[u8]) -> DefaultAppearance {
    let mut appearance = DefaultAppearance {
        size: None,
        colour: [0.0, 0.0, 0.0],
        family: "Helvetica",
        bold: false,
    };
    let mut operands: Vec<&[u8]> = Vec::new();
    for token in bytes
        .split(u8::is_ascii_whitespace)
        .filter(|token| !token.is_empty())
    {
        let number = |at: usize| -> Option<f64> {
            let operand = operands.get(operands.len().checked_sub(at)?)?;
            std::str::from_utf8(operand).ok()?.parse::<f64>().ok()
        };
        match token {
            b"Tf" => {
                if let Some(size) = number(1) {
                    appearance.size = (size.is_finite() && size > 0.0).then_some(size);
                }
                if let Some(name) = operands.get(operands.len().wrapping_sub(2)) {
                    let (family, bold) = family_of(name);
                    appearance.family = family;
                    appearance.bold = bold;
                }
                operands.clear();
            }
            b"g" => {
                if let Some(grey) = number(1).filter(|grey| (0.0..=1.0).contains(grey)) {
                    appearance.colour = [grey; 3];
                }
                operands.clear();
            }
            b"rg" => {
                if let (Some(red), Some(green), Some(blue)) = (number(3), number(2), number(1))
                    && [red, green, blue]
                        .iter()
                        .all(|part| (0.0..=1.0).contains(part))
                {
                    appearance.colour = [red, green, blue];
                }
                operands.clear();
            }
            b"k" => {
                if let (Some(cyan), Some(magenta), Some(yellow), Some(black)) =
                    (number(4), number(3), number(2), number(1))
                {
                    let parts = [cyan, magenta, yellow, black];
                    if parts.iter().all(|part| (0.0..=1.0).contains(part)) {
                        appearance.colour = [
                            (1.0 - cyan) * (1.0 - black),
                            (1.0 - magenta) * (1.0 - black),
                            (1.0 - yellow) * (1.0 - black),
                        ];
                    }
                }
                operands.clear();
            }
            _ if token.first() == Some(&b'/') || token.iter().all(|byte| is_number_byte(*byte)) => {
                operands.push(token);
            }
            _ => operands.clear(),
        }
    }
    appearance
}

const fn is_number_byte(byte: u8) -> bool {
    byte.is_ascii_digit() || matches!(byte, b'.' | b'-' | b'+')
}

fn family_of(name: &[u8]) -> (&'static str, bool) {
    match name {
        b"/Cour" | b"/CoOb" => ("Courier New", false),
        b"/CoBo" | b"/CoBO" => ("Courier New", true),
        b"/TiRo" | b"/TiIt" => ("Times New Roman", false),
        b"/TiBo" | b"/TiBI" => ("Times New Roman", true),
        b"/HeBo" | b"/HeBO" => ("Helvetica", true),
        _ => ("Helvetica", false),
    }
}

#[cfg(test)]
#[expect(
    clippy::float_cmp,
    reason = "a coordinate written as a number is read back as the same number"
)]
mod tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};
    use pdf_content::FontProvider;
    use pdf_paint::{PaintAtomKind, PaintLimits, Point};
    use pdf_syntax::Reference;

    use crate::form::{FieldValue, fields_of_page};
    use crate::plan::Command;
    use crate::spike_move_text::{SpikeError, plan_command_with_fonts, read_page};

    fn provider() -> Arc<dyn FontProvider> {
        crate::new_text::tests::provider()
    }

    fn document(objects: &[String]) -> ByteStore {
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

    fn a_form() -> ByteStore {
        document(&a_form_objects())
    }

    fn filled(widget: u32, value: FieldValue) -> Command {
        Command::FillField {
            page_index: 0,
            widget: Reference::new(widget, 0),
            value,
        }
    }

    fn after(source: &ByteStore, command: &Command) -> Result<ByteStore, SpikeError> {
        let plan = plan_command_with_fonts(source, command, b"", Some(provider()))?;
        crate::block_rewrite::commit_writes(source, plan.writes(), crate::Restrictions::Respect)
    }

    fn painted(source: &ByteStore, widget: u32) -> Vec<(f64, f64)> {
        let reading = read_page(source, 0, b"", None).expect("the page reads");
        let painted = pdf_paint::interpret_annotations(
            &reading.program.annotations,
            reading.program.page,
            reading.program.geometry.rotate,
            &reading.program.resources,
            PaintLimits::default(),
            None,
        );
        let shown = painted
            .iter()
            .find(|annotation| annotation.reference == Some(Reference::new(widget, 0)))
            .expect("the widget is on the page");
        shown
            .graph
            .atoms
            .iter()
            .filter_map(|atom| match &atom.kind {
                PaintAtomKind::Text(text) => Some(text),
                _ => None,
            })
            .flat_map(|text| {
                text.glyphs.iter().map(|glyph| {
                    let at = text
                        .state
                        .ctm
                        .value
                        .multiply(glyph.matrix)
                        .transform(Point { x: 0.0, y: 0.0 });
                    (at.x, at.y)
                })
            })
            .collect()
    }

    #[test]
    fn a_value_typed_into_a_field_is_what_the_field_says_and_what_it_shows() {
        let source = a_form();
        let filled_in =
            after(&source, &filled(4, FieldValue::Text("AB".to_owned()))).expect("the field fills");
        let fields = fields_of_page(&filled_in, Reference::new(3, 0), b"").expect("fields");
        assert_eq!(fields[0].value, FieldValue::Text("AB".to_owned()));
        let pens = painted(&filled_in, 4);
        assert_eq!(pens.len(), 2, "two letters draw");
        assert_eq!(pens[0].0, 20.0 + super::PADDING);
        assert_eq!(pens[1].0, 20.0 + super::PADDING + 5.0);
        let baseline = (200.0 + 220.0 - 10.0 * super::CAP_HEIGHT) / 2.0;
        assert!((pens[0].1 - baseline).abs() < 0.001, "{}", pens[0].1);
    }

    #[test]
    fn ticking_a_box_turns_it_on() {
        let source = a_form();
        let ticked =
            after(&source, &filled(5, FieldValue::State("Yes".to_owned()))).expect("the box ticks");
        let fields = fields_of_page(&ticked, Reference::new(3, 0), b"").expect("fields");
        assert_eq!(fields[1].value, FieldValue::State("Yes".to_owned()));
        assert!(fields[1].is_on());
    }

    #[test]
    fn clearing_a_box_turns_it_off() {
        let source = a_form();
        let ticked =
            after(&source, &filled(5, FieldValue::State("Yes".to_owned()))).expect("the box ticks");
        let cleared = after(&ticked, &filled(5, FieldValue::Empty)).expect("the box clears");
        let fields = fields_of_page(&cleared, Reference::new(3, 0), b"").expect("fields");
        assert!(!fields[1].is_on());
    }

    #[test]
    fn choosing_one_radio_button_turns_the_others_off() {
        let source = a_form();
        let large = after(&source, &filled(7, FieldValue::State("Large".to_owned())))
            .expect("the button turns on");
        let fields = fields_of_page(&large, Reference::new(3, 0), b"").expect("fields");
        assert!(fields[2].is_on(), "the button chosen is on");
        assert!(!fields[3].is_on(), "the other is off");
        assert_eq!(fields[2].shown_state.as_deref(), Some("Large"));
        assert_eq!(fields[3].shown_state.as_deref(), Some("Off"));
        let small = after(&large, &filled(8, FieldValue::State("Small".to_owned())))
            .expect("the other turns on");
        let fields = fields_of_page(&small, Reference::new(3, 0), b"").expect("fields");
        assert!(!fields[2].is_on());
        assert!(fields[3].is_on());
        assert_eq!(
            fields[2].shown_state.as_deref(),
            Some("Off"),
            "turned off again"
        );
        assert_eq!(fields[3].shown_state.as_deref(), Some("Small"));
    }

    #[test]
    fn a_state_the_box_does_not_have_is_refused() {
        let source = a_form();
        let refusal = after(&source, &filled(5, FieldValue::State("Maybe".to_owned())));
        assert!(refusal.is_err());
    }

    #[test]
    fn a_bordered_field_keeps_its_border_when_filled() {
        let mut objects = a_form_objects();
        objects[3] = "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /Rect [20 200 120 220] \
                      /MK << /BC [1 0 0] >> /DA (/Helv 10 Tf 0 g) >>"
            .to_owned();
        let source = document(&objects);
        let filled_in =
            after(&source, &filled(4, FieldValue::Text("AB".to_owned()))).expect("the field fills");
        let reading = read_page(&filled_in, 0, b"", None).expect("reads");
        let painted = pdf_paint::interpret_annotations(
            &reading.program.annotations,
            reading.program.page,
            0,
            &reading.program.resources,
            PaintLimits::default(),
            None,
        );
        let shown = painted
            .iter()
            .find(|annotation| annotation.reference == Some(Reference::new(4, 0)))
            .expect("drawn");
        let red_border = shown.graph.atoms.iter().any(|atom| match &atom.kind {
            PaintAtomKind::Path(path) => path.stroke,
            _ => false,
        });
        assert!(red_border, "the border is still drawn");
        assert_eq!(painted_count_of_text(&filled_in, 4), 2);
    }

    fn painted_count_of_text(source: &ByteStore, widget: u32) -> usize {
        painted(source, widget).len()
    }

    #[test]
    fn emptying_a_field_leaves_it_blank() {
        let source = a_form();
        let filled_in =
            after(&source, &filled(4, FieldValue::Text("AB".to_owned()))).expect("the field fills");
        let emptied = after(&filled_in, &filled(4, FieldValue::Empty)).expect("the field empties");
        let fields = fields_of_page(&emptied, Reference::new(3, 0), b"").expect("fields");
        assert_eq!(fields[0].value, FieldValue::Empty);
        assert!(painted(&emptied, 4).is_empty());
    }

    #[test]
    fn a_value_longer_than_the_field_takes_is_refused() {
        let mut objects: Vec<String> = a_form_objects();
        objects[3] = "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /Rect [20 200 120 220] \
                      /MaxLen 2 /DA (/Helv 10 Tf 0 g) >>"
            .to_owned();
        let source = document(&objects);
        assert!(after(&source, &filled(4, FieldValue::Text("AB".to_owned()))).is_ok());
        assert!(after(&source, &filled(4, FieldValue::Text("ABA".to_owned()))).is_err());
    }

    #[test]
    fn a_second_line_needs_a_field_that_holds_one() {
        let source = a_form();
        assert!(after(&source, &filled(4, FieldValue::Text("A\nB".to_owned()))).is_err());
        let mut objects = a_form_objects();
        objects[3] = "<< /Type /Annot /Subtype /Widget /FT /Tx /Ff 4096 /T (Name) \
                      /Rect [20 160 120 220] /DA (/Helv 10 Tf 0 g) >>"
            .to_owned();
        let two_lines = document(&objects);
        let filled_in = after(&two_lines, &filled(4, FieldValue::Text("A\nB".to_owned())))
            .expect("two lines fit");
        let pens = painted(&filled_in, 4);
        assert_eq!(pens.len(), 2);
        assert!(pens[0].1 > pens[1].1, "the second line is under the first");
    }

    #[test]
    fn a_right_aligned_field_draws_against_its_right_edge() {
        let mut objects = a_form_objects();
        objects[3] = "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /Q 2 \
                      /Rect [20 200 120 220] /DA (/Helv 10 Tf 0 g) >>"
            .to_owned();
        let source = document(&objects);
        let filled_in =
            after(&source, &filled(4, FieldValue::Text("AB".to_owned()))).expect("the field fills");
        let pens = painted(&filled_in, 4);
        assert!(
            (pens[0].0 - (120.0 - super::PADDING - 7.5)).abs() < 0.001,
            "{}",
            pens[0].0
        );
    }

    #[test]
    fn a_field_that_sizes_itself_picks_a_size_that_fits() {
        let mut objects = a_form_objects();
        objects[3] = "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) \
                      /Rect [20 200 120 220] /DA (/Helv 0 Tf 0 g) >>"
            .to_owned();
        let source = document(&objects);
        let filled_in =
            after(&source, &filled(4, FieldValue::Text("AB".to_owned()))).expect("the field fills");
        let pens = painted(&filled_in, 4);
        assert_eq!(pens.len(), 2);
        for (x, y) in pens {
            assert!((20.0..=120.0).contains(&x), "{x}");
            assert!((200.0..=220.0).contains(&y), "{y}");
        }
    }

    #[test]
    fn a_widget_outside_the_form_is_not_a_field() {
        let mut objects = a_form_objects();
        objects[0] = "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 6 0 R] \
                      /DA (/Helv 0 Tf 0 g) >> >>"
            .to_owned();
        let source = document(&objects);
        assert!(after(&source, &filled(5, FieldValue::State("Yes".to_owned()))).is_err());
    }

    #[test]
    fn a_read_only_field_is_refused() {
        let mut objects = a_form_objects();
        objects[3] = "<< /Type /Annot /Subtype /Widget /FT /Tx /Ff 1 /T (Name) \
                      /Rect [20 200 120 220] /DA (/Helv 10 Tf 0 g) >>"
            .to_owned();
        let source = document(&objects);
        assert!(after(&source, &filled(4, FieldValue::Text("AB".to_owned()))).is_err());
    }

    #[test]
    fn filling_a_field_does_not_touch_the_page() {
        let source = a_form();
        let before = read_page(&source, 0, b"", None).expect("reads");
        let filled_in =
            after(&source, &filled(4, FieldValue::Text("AB".to_owned()))).expect("the field fills");
        let after_page = read_page(&filled_in, 0, b"", None).expect("reads");
        assert_eq!(
            before.program.streams[0].bytes.as_bytes(),
            after_page.program.streams[0].bytes.as_bytes()
        );
    }

    fn a_form_objects() -> Vec<String> {
        vec![
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R 6 0 R] \
             /DA (/Helv 0 Tf 0 g) >> >>"
                .to_owned(),
            "<< /Type /Pages /MediaBox [0 0 300 300] /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /Contents 9 0 R /Resources << /ProcSet [/PDF] >> \
             /Annots [4 0 R 5 0 R 7 0 R 8 0 R] >>"
                .to_owned(),
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /Rect [20 200 120 220] \
             /DA (/Helv 10 Tf 0 g) >>"
                .to_owned(),
            "<< /Type /Annot /Subtype /Widget /FT /Btn /T (Agree) /V /Off /AS /Off \
             /Rect [20 160 36 176] /AP << /N << /Yes 10 0 R /Off 10 0 R >> >> >>"
                .to_owned(),
            "<< /FT /Btn /Ff 32768 /T (Size) /V /Off /Kids [7 0 R 8 0 R] >>".to_owned(),
            "<< /Type /Annot /Subtype /Widget /Parent 6 0 R /Rect [20 120 36 136] /AS /Off \
             /AP << /N << /Large 10 0 R /Off 10 0 R >> >> >>"
                .to_owned(),
            "<< /Type /Annot /Subtype /Widget /Parent 6 0 R /Rect [50 120 66 136] /AS /Off \
             /AP << /N << /Small 10 0 R /Off 10 0 R >> >> >>"
                .to_owned(),
            "<< /Length 0 >>\nstream\n\nendstream".to_owned(),
            "<< /Type /XObject /Subtype /Form /BBox [0 0 16 16] /Length 0 >>\nstream\n\nendstream"
                .to_owned(),
        ]
    }

    #[test]
    fn a_string_typed_into_an_object_the_file_holds_is_written_as_ciphertext() {
        let source = crate::incremental::tests::protected_pdf();
        let catalog = Reference::new(1, 0);
        let write = super::set_entries(
            &source,
            catalog,
            &[(b"/Foo", crate::field_settings::pdf_literal("hello"))],
        )
        .expect("the entry is set");
        let crate::plan::PlannedBody::Direct { body } = &write.body else {
            panic!("a dictionary is written directly");
        };
        assert!(
            !body.windows(5).any(|window| window == b"hello"),
            "the plaintext must not be written into a protected document: {}",
            String::from_utf8_lossy(body)
        );
        let document = crate::block_rewrite::commit_writes(
            &source,
            std::slice::from_ref(&write),
            crate::Restrictions::Respect,
        )
        .expect("the revision is written");
        let reader = crate::form::Reader::open(&document, b"").expect("the document opens");
        let found = reader
            .entry(&reader.catalog().expect("a catalog"), b"/Foo")
            .expect("the entry reads");
        assert_eq!(reader.text(&found).as_deref(), Some("hello"));
    }

    #[test]
    fn a_string_typed_into_a_plain_document_is_written_as_it_was_typed() {
        let source = crate::new_field::tests::document(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /MediaBox [0 0 300 300] /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>",
            "<< /Length 0 >>\nstream\n\nendstream",
        ]);
        let write = super::set_entries(
            &source,
            Reference::new(1, 0),
            &[(b"/Foo", crate::field_settings::pdf_literal("hello"))],
        )
        .expect("the entry is set");
        let crate::plan::PlannedBody::Direct { body } = &write.body else {
            panic!("a dictionary is written directly");
        };
        assert!(body.windows(5).any(|window| window == b"hello"));
    }
}
