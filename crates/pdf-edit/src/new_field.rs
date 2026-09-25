use pdf_bytes::ByteStore;
use pdf_syntax::{ObjectKind, Reference};

use crate::field_look::{Look, frame, mark};
use crate::fill_field::pdf_text_string;
use crate::form::{BorderStyle, ButtonStyle, FieldKind, FieldValue, Reader};
use crate::object_edit::{ObjectEdit, reference_text};
use crate::plan::{Capability, Effect, Plan, PlannedBody, PlannedWrite};
use crate::spike_move_text::{PlannerPage, SpikeError};

const SMALLEST: f64 = 6.0;
pub const MOST_OPTIONS: usize = 1_000;
const LONGEST_NAME: usize = 128;
const MULTILINE: u32 = 1 << 12;
const RADIO_GROUP: u32 = (1 << 15) | (1 << 14);
const COMBO: u32 = 1 << 17;
const PUSHBUTTON: u32 = 1 << 16;

const DATE_FORMAT: &str = "dd/mm/yyyy";

fn caption_of<'a>(new: &'a NewField<'a>) -> &'a str {
    new.options.first().map_or("Button", String::as_str)
}

fn refused(reason: &'static str) -> SpikeError {
    SpikeError::RetypeUnsupported(reason)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NewFieldKind {
    Text,
    Paragraph,
    Checkbox,
    Radio,
    Dropdown,
    ListBox,
    Date,
    Signature,
    Button,
}

impl NewFieldKind {
    #[must_use]
    pub const fn field_kind(self) -> FieldKind {
        match self {
            Self::Text | Self::Paragraph | Self::Date => FieldKind::Text,
            Self::Checkbox => FieldKind::Checkbox,
            Self::Radio => FieldKind::Radio,
            Self::Dropdown => FieldKind::Combo,
            Self::ListBox => FieldKind::List,
            Self::Signature => FieldKind::Signature,
            Self::Button => FieldKind::Push,
        }
    }

    const fn stem(self) -> &'static str {
        match self {
            Self::Text => "Text",
            Self::Paragraph => "Paragraph",
            Self::Checkbox => "Check",
            Self::Radio => "Group",
            Self::Dropdown => "Dropdown",
            Self::ListBox => "List",
            Self::Date => "Date",
            Self::Signature => "Signature",
            Self::Button => "Button",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct NewField<'a> {
    pub(crate) rect: [f64; 4],
    pub(crate) kind: NewFieldKind,
    pub(crate) name: Option<&'a str>,
    pub(crate) options: &'a [String],
}

#[expect(
    clippy::too_many_lines,
    reason = "one plan, written in the order the objects are numbered"
)]
pub(crate) fn plan_new_field(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    new: &NewField<'_>,
) -> Result<Plan, SpikeError> {
    let credential = page.credential;
    let rect = checked_rect(new)?;
    let reader = Reader::open(source, credential)?;
    let named = crate::form::named_nodes(&reader);
    let name = match new.name {
        Some(name) => checked_name(name)?.to_owned(),
        None => next_name(new.kind, &named),
    };
    let existing = named.iter().find(|(taken, _, _)| *taken == name);
    let joining = match (new.kind, existing) {
        (NewFieldKind::Radio, Some((_, group, Some(FieldKind::Radio)))) => Some(*group),
        (_, Some(_)) => return Err(refused("a field of that name is already in this form")),
        (_, None) => None,
    };
    for option in new.options {
        if option.is_empty() || option.chars().any(char::is_control) {
            return Err(refused("an option is a line of text"));
        }
    }
    if new.kind == NewFieldKind::Button && new.options.len() > 1 {
        return Err(refused("a button has one caption"));
    }
    if new.kind == NewFieldKind::Dropdown && new.options.is_empty() {
        return Err(refused("a dropdown needs something to choose"));
    }
    if new.options.len() > MOST_OPTIONS {
        return Err(refused("this is more options than one list offers"));
    }

    let mut number = crate::block_rewrite::next_object_number(source)?;
    let mut next = || {
        let reference = Reference::new(number, 0);
        number += 1;
        reference
    };
    let widget = next();
    let group = (new.kind == NewFieldKind::Radio && joining.is_none()).then(&mut next);
    let (width, height) = (rect[2] - rect[0], rect[3] - rect[1]);
    let mut writes = Vec::new();
    let stream = |reference: Reference, content: String| PlannedWrite {
        reference,
        body: PlannedBody::NewStream {
            dictionary: format!(
                "/Type /XObject /Subtype /Form /FormType 1 /BBox [0 0 {width} {height}] \
                 /Resources << >>"
            )
            .into_bytes(),
            decoded: content.into_bytes(),
        },
    };
    let page_reference = page.program.page;
    let common = format!(
        "/Type /Annot /Subtype /Widget /Rect [{} {} {} {}] /F 4 /P {} \
         /BS << /W 1 /S /S >> /MK << /BC [0 0 0]{} >>",
        rect[0],
        rect[1],
        rect[2],
        rect[3],
        reference_text(page_reference),
        match new.kind {
            NewFieldKind::Checkbox => format!(" /CA ({})", ButtonStyle::Check.character()),
            NewFieldKind::Radio => format!(" /CA ({})", ButtonStyle::Circle.character()),
            NewFieldKind::Button => format!(
                " /BG [0.75 0.75 0.75] /CA {}",
                crate::field_settings::pdf_literal(caption_of(new))
            ),
            _ => String::new(),
        }
    );
    let title = pdf_text_string(&name);
    let body = match new.kind {
        NewFieldKind::Text
        | NewFieldKind::Paragraph
        | NewFieldKind::Date
        | NewFieldKind::Dropdown
        | NewFieldKind::ListBox => {
            let look = next();
            writes.push(stream(look, frame(&new_look(width, height, false))));
            let (flags, size) = match new.kind {
                NewFieldKind::Paragraph => (MULTILINE, 11),
                NewFieldKind::Dropdown => (COMBO, 0),
                NewFieldKind::ListBox => (0, 12),
                _ => (0, 0),
            };
            let options = if matches!(new.kind, NewFieldKind::Dropdown | NewFieldKind::ListBox) {
                let listed: Vec<String> = new.options.iter().map(|o| pdf_text_string(o)).collect();
                format!(" /Opt [{}]", listed.join(" "))
            } else {
                String::new()
            };
            let kind = if matches!(new.kind, NewFieldKind::Dropdown | NewFieldKind::ListBox) {
                "Ch"
            } else {
                "Tx"
            };
            let dated = if new.kind == NewFieldKind::Date {
                let format = DATE_FORMAT;
                format!(
                    " /AA << /F << /S /JavaScript /JS {} >> /K << /S /JavaScript /JS {} >> >>",
                    crate::field_settings::pdf_literal(&format!("AFDate_FormatEx(\"{format}\");")),
                    crate::field_settings::pdf_literal(&format!(
                        "AFDate_KeystrokeEx(\"{format}\");"
                    ))
                )
            } else {
                String::new()
            };
            format!(
                "<< {common} /FT /{kind} /Ff {flags} /T {title} /DA (/Helv {size} Tf 0 g){options}\
                 {dated} /AP << /N {} >> >>",
                reference_text(look)
            )
        }
        NewFieldKind::Signature => {
            let look = next();
            writes.push(stream(look, frame(&new_look(width, height, false))));
            format!(
                "<< {common} /FT /Sig /T {title} /AP << /N {} >> >>",
                reference_text(look)
            )
        }
        NewFieldKind::Button => {
            let look = next();
            let mut pressed = new_look(width, height, false);
            pressed.fill = Some([0.75, 0.75, 0.75]);
            pressed.style = crate::form::BorderStyle::Beveled;
            writes.push(stream(look, frame(&pressed)));
            format!(
                "<< {common} /FT /Btn /Ff {PUSHBUTTON} /T {title} /DA (/Helv 0 Tf 0 g) \
                 /AP << /N {} >> >>",
                reference_text(look)
            )
        }
        NewFieldKind::Checkbox => {
            let (on, off) = (next(), next());
            let look = new_look(width, height, false);
            writes.push(stream(
                on,
                frame(&look) + &mark(ButtonStyle::Check, &look, [0.0; 3]),
            ));
            writes.push(stream(off, frame(&look)));
            format!(
                "<< {common} /FT /Btn /T {title} /V /Off /AS /Off \
                 /AP << /N << /Yes {} /Off {} >> >> >>",
                reference_text(on),
                reference_text(off)
            )
        }
        NewFieldKind::Radio => {
            let (on, off) = (next(), next());
            let look = new_look(width, height, true);
            writes.push(stream(
                on,
                frame(&look) + &mark(ButtonStyle::Circle, &look, [0.0; 3]),
            ));
            writes.push(stream(off, frame(&look)));
            let parent = joining
                .or(group)
                .ok_or_else(|| refused("a radio button has a group"))?;
            let state = format!("Choice{}", kids_of(&reader, joining) + 1);
            format!(
                "<< {common} /Parent {} /AS /Off \
                 /AP << /N << /{state} {} /Off {} >> >> >>",
                reference_text(parent),
                reference_text(on),
                reference_text(off)
            )
        }
    };
    writes.push(PlannedWrite {
        reference: widget,
        body: PlannedBody::Direct {
            body: body.into_bytes(),
        },
    });
    if let Some(group) = group {
        writes.push(PlannedWrite {
            reference: group,
            body: PlannedBody::Direct {
                body: format!(
                    "<< /FT /Btn /Ff {RADIO_GROUP} /T {title} /V /Off /Kids [{}] >>",
                    reference_text(widget)
                )
                .into_bytes(),
            },
        });
    }

    writes.extend(listed_on_page(
        (source, credential),
        page_reference,
        &[widget],
    )?);
    if let Some(group) = joining {
        writes.push(added_kid((source, credential), group, widget)?);
    } else {
        let helvetica = next();
        writes.extend(listed_in_form(
            (source, credential),
            &reader,
            group.unwrap_or(widget),
            helvetica,
        )?);
    }
    let mut writes = one_write_each(writes)?;
    match new.kind {
        NewFieldKind::Button => {
            let caption = FieldValue::Text(caption_of(new).to_owned());
            writes = with_text_drawn(source, page, page_index, (widget, &caption), writes)?;
        }
        NewFieldKind::ListBox if !new.options.is_empty() => {
            writes = with_text_drawn(
                source,
                page,
                page_index,
                (widget, &FieldValue::Empty),
                writes,
            )?;
        }
        _ => {}
    }
    prove_added(
        (source, credential),
        &writes,
        (page_index, page_reference, widget),
        (new, &name, rect),
    )?;
    let target = page
        .program
        .streams
        .first()
        .map_or(page_reference, |stream| stream.reference);
    Ok(Plan::new(
        Capability::Exact,
        writes,
        Effect {
            page_index,
            moved: Vec::new(),
            target_stream: target,
            declared_region: Some(rect),
        },
    ))
}

fn checked_rect(new: &NewField<'_>) -> Result<[f64; 4], SpikeError> {
    checked_box(new.rect)
}

pub(crate) fn checked_box(rect: [f64; 4]) -> Result<[f64; 4], SpikeError> {
    let [x0, y0, x1, y1] = rect;
    if ![x0, y0, x1, y1].iter().all(|edge| edge.is_finite()) {
        return Err(refused("a field's box is four numbers"));
    }
    let rect = [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)];
    if rect[2] - rect[0] < SMALLEST || rect[3] - rect[1] < SMALLEST {
        return Err(refused("this box is too small for a field"));
    }
    Ok(rect)
}

pub(crate) fn checked_name(name: &str) -> Result<&str, SpikeError> {
    if name.trim().is_empty() {
        return Err(refused("a field has a name"));
    }
    if name.contains('.') {
        return Err(refused("a field's name cannot hold a full stop"));
    }
    if name.chars().count() > LONGEST_NAME || name.chars().any(char::is_control) {
        return Err(refused("a field's name is a short line of text"));
    }
    Ok(name)
}

fn next_name(kind: NewFieldKind, named: &[(String, Reference, Option<FieldKind>)]) -> String {
    (1..=named.len() + 1)
        .map(|number| format!("{}{number}", kind.stem()))
        .find(|name| !named.iter().any(|(taken, _, _)| taken == name))
        .unwrap_or_default()
}

fn kids_of(reader: &Reader, group: Option<Reference>) -> usize {
    group
        .and_then(|group| reader.at(group))
        .and_then(|node| reader.entry(&node, b"/Kids"))
        .map_or(0, |kids| match kids.value.kind() {
            ObjectKind::Array(items) => items.len(),
            _ => 0,
        })
}

fn new_look(width: f64, height: f64, round: bool) -> Look {
    Look {
        width,
        height,
        border: Some([0.0, 0.0, 0.0]),
        fill: None,
        border_width: 1.0,
        style: BorderStyle::Solid,
        round,
    }
}

pub(crate) fn listed_on_page(
    (source, credential): (&ByteStore, &[u8]),
    page: Reference,
    widgets: &[Reference],
) -> Result<Vec<PlannedWrite>, SpikeError> {
    if widgets.is_empty() {
        return Ok(Vec::new());
    }
    let written: Vec<String> = widgets.iter().copied().map(reference_text).collect();
    let mut edit = ObjectEdit::of(source, page, credential)?;
    let dictionary = edit.value();
    let annots = crate::object_edit::entry(&edit.body, &dictionary, b"/Annots").cloned();
    match annots.as_ref().map(pdf_syntax::Object::kind) {
        None => edit.set(&dictionary, b"/Annots", &format!("[{}]", written.join(" ")))?,
        Some(ObjectKind::Array(_)) => {
            let annots = annots.ok_or_else(|| refused("the page's annotations cannot be read"))?;
            edit.append(&annots, &written.join(" "))?;
        }
        Some(ObjectKind::Reference(list)) => {
            let mut held = ObjectEdit::of(source, *list, credential)?;
            let array = held.value();
            held.append(&array, &written.join(" "))?;
            return Ok(vec![held.written()?]);
        }
        Some(_) => return Err(refused("the page's annotations cannot be read")),
    }
    Ok(vec![edit.written()?])
}

fn added_kid(
    (source, credential): (&ByteStore, &[u8]),
    group: Reference,
    widget: Reference,
) -> Result<PlannedWrite, SpikeError> {
    let mut edit = ObjectEdit::of(source, group, credential)?;
    let dictionary = edit.value();
    let kids = crate::object_edit::entry(&edit.body, &dictionary, b"/Kids")
        .cloned()
        .ok_or_else(|| refused("this radio group lists no buttons"))?;
    match kids.kind() {
        ObjectKind::Array(_) => edit.append(&kids, &reference_text(widget))?,
        ObjectKind::Reference(list) => {
            let mut held = ObjectEdit::of(source, *list, credential)?;
            let array = held.value();
            held.append(&array, &reference_text(widget))?;
            return held.written();
        }
        _ => return Err(refused("this radio group lists no buttons")),
    }
    edit.written()
}

fn listed_in_form(
    (source, credential): (&ByteStore, &[u8]),
    reader: &Reader,
    field: Reference,
    helvetica: Reference,
) -> Result<Vec<PlannedWrite>, SpikeError> {
    let catalog = reader
        .catalog_reference()
        .ok_or_else(|| refused("this document's catalog cannot be read"))?;
    let mut edit = ObjectEdit::of(source, catalog, credential)?;
    let dictionary = edit.value();
    let font = || PlannedWrite {
        reference: helvetica,
        body: PlannedBody::Direct {
            body:
                b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                    .to_vec(),
        },
    };
    let resources = format!("<< /Font << /Helv {} >> >>", reference_text(helvetica));
    let form = crate::object_edit::entry(&edit.body, &dictionary, b"/AcroForm").cloned();
    let mut extra = Vec::new();
    let (mut edit, form) = match form.as_ref().map(pdf_syntax::Object::kind) {
        None => {
            edit.set(
                &dictionary,
                b"/AcroForm",
                &format!(
                    "<< /Fields [{}] /DA (/Helv 0 Tf 0 g) /DR {resources} >>",
                    reference_text(field)
                ),
            )?;
            return Ok(vec![edit.written()?, font()]);
        }
        Some(ObjectKind::Dictionary(_)) => {
            let form = form.ok_or_else(|| refused("this document's form cannot be read"))?;
            (edit, form)
        }
        Some(ObjectKind::Reference(held)) => {
            let edit = ObjectEdit::of(source, *held, credential)?;
            let form = edit.value();
            (edit, form)
        }
        Some(_) => return Err(refused("this document's form cannot be read")),
    };
    if crate::object_edit::entry(&edit.body, &form, b"/DR").is_none() {
        edit.set(&form, b"/DR", &resources)?;
        extra.push(font());
    }
    let fields = crate::object_edit::entry(&edit.body, &form, b"/Fields").cloned();
    match fields.as_ref().map(pdf_syntax::Object::kind) {
        None => edit.set(&form, b"/Fields", &format!("[{}]", reference_text(field)))?,
        Some(ObjectKind::Array(_)) => {
            let fields = fields.ok_or_else(|| refused("this document's form cannot be read"))?;
            edit.append(&fields, &reference_text(field))?;
        }
        Some(ObjectKind::Reference(list)) => {
            let mut held = ObjectEdit::of(source, *list, credential)?;
            let array = held.value();
            held.append(&array, &reference_text(field))?;
            extra.push(held.written()?);
        }
        Some(_) => return Err(refused("this document's form cannot be read")),
    }
    if !edit.is_empty() {
        extra.insert(0, edit.written()?);
    }
    Ok(extra)
}

fn with_text_drawn(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    (widget, value): (Reference, &FieldValue),
    writes: Vec<PlannedWrite>,
) -> Result<Vec<PlannedWrite>, SpikeError> {
    let credential = page.credential;
    let document = crate::block_rewrite::commit_writes(
        source,
        &writes,
        (credential, crate::Restrictions::SetAside),
    )?;
    let field = crate::form::fields_of_page(&document, page.program.page, credential)?
        .into_iter()
        .find(|field| field.widget == widget)
        .ok_or_else(|| refused("the button added is not read back as a field of the form"))?;
    let drawn = crate::fill_field::typed(
        &document,
        page,
        page_index,
        (
            &field,
            &crate::fill_field::Beside {
                field: Vec::new(),
                widget: Vec::new(),
                keep_value: true,
            },
        ),
        value,
    )?;
    let mut folded = writes;
    for write in drawn {
        match folded
            .iter_mut()
            .find(|held| held.reference == write.reference)
        {
            Some(held) => *held = write,
            None => folded.push(write),
        }
    }
    Ok(folded)
}

pub(crate) fn one_write_each(writes: Vec<PlannedWrite>) -> Result<Vec<PlannedWrite>, SpikeError> {
    let mut seen = std::collections::HashSet::new();
    for write in &writes {
        if !seen.insert((
            write.reference.object_number(),
            write.reference.generation(),
        )) {
            return Err(refused("this field would write one object twice"));
        }
    }
    Ok(writes)
}

fn prove_added(
    (source, credential): (&ByteStore, &[u8]),
    writes: &[PlannedWrite],
    (page_index, page, widget): (usize, Reference, Reference),
    (new, name, rect): (&NewField<'_>, &str, [f64; 4]),
) -> Result<(), SpikeError> {
    let document = crate::block_rewrite::commit_writes(
        source,
        writes,
        (credential, crate::Restrictions::SetAside),
    )?;
    let fields = crate::form::fields_of_page(&document, page, credential)?;
    let found = fields
        .iter()
        .find(|field| field.widget == widget)
        .ok_or_else(|| refused("the field added is not read back as a field of the form"))?;
    let close = |one: f64, other: f64| (one - other).abs() < 1e-6;
    if found.kind != new.kind.field_kind()
        || found.name != name
        || !found
            .rect
            .iter()
            .zip(rect)
            .all(|(one, other)| close(*one, other))
        || (new.kind == NewFieldKind::Paragraph) != found.multiline
        || (matches!(new.kind, NewFieldKind::Dropdown | NewFieldKind::ListBox)
            && found.options != new.options)
        || (new.kind == NewFieldKind::Button && found.caption != caption_of(new))
    {
        return Err(refused(
            "the field added does not read back as the field asked for",
        ));
    }
    let reading = crate::spike_move_text::read_page(&document, page_index, credential, None)?;
    let painted = pdf_paint::interpret_annotations(
        &reading.program.annotations,
        reading.program.page,
        reading.program.geometry.rotate,
        &reading.program.resources,
        pdf_paint::PaintLimits::default(),
        None,
    );
    let drawn = painted.iter().any(|annotation| {
        annotation.reference == Some(widget) && !annotation.graph.atoms.is_empty()
    });
    if !drawn {
        return Err(refused("the field added does not draw"));
    }
    Ok(())
}

pub(crate) fn plan_remove_field(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    widget: Reference,
) -> Result<Plan, SpikeError> {
    let credential = page.credential;
    let reader = Reader::open(source, credential)?;
    let page_reference = page.program.page;
    let before = crate::form::fields_of_page(source, page_reference, credential)?;
    let field = before
        .iter()
        .find(|field| field.widget == widget)
        .ok_or_else(|| refused("this is not a field of the document's form"))?;
    let mut writes = vec![unlisted_on_page(
        (source, credential),
        page_reference,
        widget,
    )?];
    if field.field == widget {
        writes.push(unlisted_in_form((source, credential), &reader, widget)?);
    } else {
        let buttons = kids_of(&reader, Some(field.field));
        if buttons <= 1 {
            writes.push(unlisted_in_form(
                (source, credential),
                &reader,
                field.field,
            )?);
        } else {
            let mut edit = ObjectEdit::of(source, field.field, credential)?;
            let dictionary = edit.value();
            let kids = crate::object_edit::entry(&edit.body, &dictionary, b"/Kids")
                .cloned()
                .ok_or_else(|| refused("this radio group lists no buttons"))?;
            match kids.kind() {
                ObjectKind::Array(_) => {
                    edit.remove(&kids, widget)?;
                    writes.push(edit.written()?);
                }
                ObjectKind::Reference(list) => {
                    let mut held = ObjectEdit::of(source, *list, credential)?;
                    let array = held.value();
                    held.remove(&array, widget)?;
                    writes.push(held.written()?);
                }
                _ => return Err(refused("this radio group lists no buttons")),
            }
        }
    }
    let writes = one_write_each(writes)?;
    let document = crate::block_rewrite::commit_writes(
        source,
        &writes,
        (credential, crate::Restrictions::SetAside),
    )?;
    let after = crate::form::fields_of_page(&document, page_reference, credential)?;
    if after.iter().any(|left| left.widget == widget) || after.len() + 1 != before.len() {
        return Err(refused("the field is still read on the page"));
    }
    let target = page
        .program
        .streams
        .first()
        .map_or(page_reference, |stream| stream.reference);
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

pub(crate) fn unlisted_on_page(
    (source, credential): (&ByteStore, &[u8]),
    page: Reference,
    widget: Reference,
) -> Result<PlannedWrite, SpikeError> {
    let edit = ObjectEdit::of(source, page, credential)?;
    let dictionary = edit.value();
    let annots = crate::object_edit::entry(&edit.body, &dictionary, b"/Annots")
        .cloned()
        .ok_or_else(|| refused("the page's annotations cannot be read"))?;
    let (mut edit, array) = match annots.kind() {
        ObjectKind::Array(_) => (edit, annots),
        ObjectKind::Reference(list) => {
            let held = ObjectEdit::of(source, *list, credential)?;
            let array = held.value();
            (held, array)
        }
        _ => return Err(refused("the page's annotations cannot be read")),
    };
    if edit.remove(&array, widget)? == 0 {
        return Err(refused("the page does not list this field"));
    }
    edit.written()
}

fn unlisted_in_form(
    (source, credential): (&ByteStore, &[u8]),
    reader: &Reader,
    field: Reference,
) -> Result<PlannedWrite, SpikeError> {
    let unreadable = || refused("this document's form cannot be read");
    let catalog = reader.catalog_reference().ok_or_else(unreadable)?;
    let edit = ObjectEdit::of(source, catalog, credential)?;
    let dictionary = edit.value();
    let form = crate::object_edit::entry(&edit.body, &dictionary, b"/AcroForm")
        .cloned()
        .ok_or_else(unreadable)?;
    let (edit, form) = match form.kind() {
        ObjectKind::Dictionary(_) => (edit, form),
        ObjectKind::Reference(held) => {
            let edit = ObjectEdit::of(source, *held, credential)?;
            let form = edit.value();
            (edit, form)
        }
        _ => return Err(unreadable()),
    };
    let fields = crate::object_edit::entry(&edit.body, &form, b"/Fields")
        .cloned()
        .ok_or_else(unreadable)?;
    let (mut edit, array) = match fields.kind() {
        ObjectKind::Array(_) => (edit, fields),
        ObjectKind::Reference(list) => {
            let held = ObjectEdit::of(source, *list, credential)?;
            let array = held.value();
            (held, array)
        }
        _ => return Err(unreadable()),
    };
    if edit.remove(&array, field)? == 0 {
        return Err(refused("this field is not listed at the top of the form"));
    }
    edit.written()
}

#[cfg(test)]
#[expect(
    clippy::float_cmp,
    reason = "a box written as numbers is read back as the same numbers"
)]
pub(crate) mod tests {
    use pdf_bytes::{ByteStore, SourceId};
    use pdf_syntax::Reference;

    use super::NewFieldKind;
    use crate::form::{FieldKind, FieldValue, FormField, fields_of_page};
    use crate::plan::Command;
    use crate::spike_move_text::{SpikeError, plan_command_with_fonts, read_page};

    pub(crate) fn document(objects: &[&str]) -> ByteStore {
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

    fn no_form() -> ByteStore {
        document(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /MediaBox [0 0 300 300] /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>",
            "<< /Length 21 >>\nstream\n0 0 m 100 100 l S    \nendstream",
        ])
    }

    fn add(kind: NewFieldKind, rect: [f64; 4], name: Option<&str>, options: &[&str]) -> Command {
        Command::AddField {
            page_index: 0,
            rect,
            kind,
            name: name.map(str::to_owned),
            options: options.iter().map(|option| (*option).to_owned()).collect(),
        }
    }

    fn after(source: &ByteStore, command: &Command) -> Result<ByteStore, SpikeError> {
        let plan = plan_command_with_fonts(
            source,
            command,
            b"",
            Some(crate::new_text::tests::provider()),
        )?;
        crate::block_rewrite::commit_writes(
            source,
            plan.writes(),
            (b"", crate::Restrictions::Respect),
        )
    }

    fn fields(source: &ByteStore) -> Vec<FormField> {
        let page = read_page(source, 0, b"", None).expect("reads").program.page;
        fields_of_page(source, page, b"").expect("fields")
    }

    #[test]
    fn a_text_field_on_a_page_without_a_form_can_be_filled_in() {
        let added = after(
            &no_form(),
            &add(NewFieldKind::Text, [20.0, 200.0, 120.0, 220.0], None, &[]),
        )
        .expect("the field is added");
        let listed = fields(&added);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].kind, FieldKind::Text);
        assert_eq!(listed[0].name, "Text1");
        assert_eq!(listed[0].rect, [20.0, 200.0, 120.0, 220.0]);
        assert_eq!(listed[0].border, Some([0.0, 0.0, 0.0]));
        let filled = after(
            &added,
            &Command::FillField {
                page_index: 0,
                widget: listed[0].widget,
                value: FieldValue::Text("AB".to_owned()),
            },
        )
        .expect("the new field fills");
        assert_eq!(fields(&filled)[0].value, FieldValue::Text("AB".to_owned()));
    }

    #[test]
    fn names_are_never_shared() {
        let one = after(
            &no_form(),
            &add(NewFieldKind::Text, [20.0, 200.0, 120.0, 220.0], None, &[]),
        )
        .expect("one");
        let two = after(
            &one,
            &add(NewFieldKind::Text, [120.0, 170.0, 20.0, 150.0], None, &[]),
        )
        .expect("two");
        let listed = fields(&two);
        assert_eq!(listed[1].name, "Text2");
        assert_eq!(listed[1].rect, [20.0, 150.0, 120.0, 170.0]);
        assert!(
            after(
                &two,
                &add(
                    NewFieldKind::Checkbox,
                    [0.0, 0.0, 12.0, 12.0],
                    Some("Text1"),
                    &[]
                )
            )
            .is_err()
        );
        assert!(
            after(
                &two,
                &add(NewFieldKind::Text, [0.0, 0.0, 50.0, 12.0], Some("a.b"), &[])
            )
            .is_err()
        );
    }

    #[test]
    fn a_new_checkbox_ticks() {
        let added = after(
            &no_form(),
            &add(NewFieldKind::Checkbox, [20.0, 20.0, 34.0, 34.0], None, &[]),
        )
        .expect("added");
        let listed = fields(&added);
        assert_eq!(listed[0].states, ["Yes"]);
        assert!(!listed[0].is_on());
        let ticked = after(
            &added,
            &Command::FillField {
                page_index: 0,
                widget: listed[0].widget,
                value: FieldValue::State("Yes".to_owned()),
            },
        )
        .expect("ticks");
        assert!(fields(&ticked)[0].is_on());
    }

    #[test]
    fn radio_buttons_of_one_name_are_one_group() {
        let one = after(
            &no_form(),
            &add(
                NewFieldKind::Radio,
                [20.0, 20.0, 34.0, 34.0],
                Some("Size"),
                &[],
            ),
        )
        .expect("one");
        let two = after(
            &one,
            &add(
                NewFieldKind::Radio,
                [50.0, 20.0, 64.0, 34.0],
                Some("Size"),
                &[],
            ),
        )
        .expect("two");
        let listed = fields(&two);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].field, listed[1].field, "one field between them");
        assert_eq!(listed[0].states, ["Choice1"]);
        assert_eq!(listed[1].states, ["Choice2"]);
        let chosen = after(
            &two,
            &Command::FillField {
                page_index: 0,
                widget: listed[1].widget,
                value: FieldValue::State("Choice2".to_owned()),
            },
        )
        .expect("chosen");
        let listed = fields(&chosen);
        assert!(!listed[0].is_on());
        assert!(listed[1].is_on());
    }

    #[test]
    fn a_dropdown_offers_its_options() {
        let rect = [20.0, 20.0, 120.0, 40.0];
        assert!(after(&no_form(), &add(NewFieldKind::Dropdown, rect, None, &[])).is_err());
        let added = after(
            &no_form(),
            &add(NewFieldKind::Dropdown, rect, None, &["Thailand", "ລາວ"]),
        )
        .expect("added");
        let listed = fields(&added);
        assert_eq!(listed[0].kind, FieldKind::Combo);
        assert_eq!(listed[0].options, ["Thailand", "ລາວ"]);
    }

    #[test]
    fn a_paragraph_holds_several_lines() {
        let added = after(
            &no_form(),
            &add(
                NewFieldKind::Paragraph,
                [20.0, 20.0, 200.0, 120.0],
                None,
                &[],
            ),
        )
        .expect("added");
        assert!(fields(&added)[0].multiline);
    }

    #[test]
    fn a_form_written_in_objects_of_its_own_gains_the_field() {
        let source = document(&[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>",
            "<< /Type /Pages /MediaBox [0 0 300 300] /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> /Annots 7 0 R >>",
            "<< /Length 0 >>\nstream\n\nendstream",
            "<< /Fields 6 0 R /DA (/Helv 0 Tf 0 g) >>",
            "[8 0 R]",
            "[8 0 R]",
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Old) /Rect [0 0 50 20] >>",
        ]);
        let added = after(
            &source,
            &add(NewFieldKind::Text, [20.0, 200.0, 120.0, 220.0], None, &[]),
        )
        .expect("added");
        let names: Vec<String> = fields(&added).into_iter().map(|field| field.name).collect();
        assert_eq!(names, ["Old", "Text1"]);
    }

    #[test]
    fn a_box_too_small_is_refused() {
        assert!(
            after(
                &no_form(),
                &add(NewFieldKind::Text, [20.0, 20.0, 22.0, 40.0], None, &[])
            )
            .is_err()
        );
    }

    #[test]
    fn undoing_a_field_takes_it_away() {
        let source = no_form();
        let command = add(NewFieldKind::Checkbox, [20.0, 20.0, 34.0, 34.0], None, &[]);
        let plan = plan_command_with_fonts(&source, &command, b"", None).expect("planned");
        let added = crate::block_rewrite::commit_writes(
            &source,
            plan.writes(),
            (b"", crate::Restrictions::Respect),
        )
        .expect("added");
        assert_eq!(
            read_page(&added, 0, b"", None)
                .expect("reads")
                .program
                .streams[0]
                .bytes
                .as_bytes(),
            read_page(&source, 0, b"", None)
                .expect("reads")
                .program
                .streams[0]
                .bytes
                .as_bytes()
        );
        let back = plan.inverse(&source, b"").expect("an inverse");
        let undone = crate::block_rewrite::commit_writes(
            &added,
            back.writes(),
            (b"", crate::Restrictions::Respect),
        )
        .expect("undone");
        assert!(fields(&undone).is_empty());
        let _ = Reference::new(1, 0);
    }

    fn remove(source: &ByteStore, widget: Reference) -> Result<ByteStore, SpikeError> {
        after(
            source,
            &Command::RemoveField {
                page_index: 0,
                widget,
            },
        )
    }

    #[test]
    fn a_removed_field_leaves_the_page_and_the_form() {
        let one = after(
            &no_form(),
            &add(NewFieldKind::Text, [20.0, 200.0, 120.0, 220.0], None, &[]),
        )
        .expect("one");
        let two = after(
            &one,
            &add(NewFieldKind::Checkbox, [20.0, 20.0, 34.0, 34.0], None, &[]),
        )
        .expect("two");
        let listed = fields(&two);
        let removed = remove(&two, listed[0].widget).expect("removed");
        let left = fields(&removed);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].name, "Check1");
        let again = after(
            &removed,
            &add(NewFieldKind::Text, [20.0, 200.0, 120.0, 220.0], None, &[]),
        )
        .expect("again");
        assert!(fields(&again).iter().any(|field| field.name == "Text1"));
    }

    #[test]
    fn the_last_button_of_a_group_takes_the_group_with_it() {
        let one = after(
            &no_form(),
            &add(
                NewFieldKind::Radio,
                [20.0, 20.0, 34.0, 34.0],
                Some("Size"),
                &[],
            ),
        )
        .expect("one");
        let two = after(
            &one,
            &add(
                NewFieldKind::Radio,
                [50.0, 20.0, 64.0, 34.0],
                Some("Size"),
                &[],
            ),
        )
        .expect("two");
        let listed = fields(&two);
        let fewer = remove(&two, listed[0].widget).expect("one button out");
        let left = fields(&fewer);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].states, ["Choice2"]);
        let none = remove(&fewer, left[0].widget).expect("the last out");
        assert!(fields(&none).is_empty());
        let reader = crate::form::Reader::open(&none, b"").expect("opens");
        assert!(crate::form::named_nodes(&reader).is_empty());
    }

    #[test]
    fn a_list_box_shows_its_items() {
        let added = after(
            &no_form(),
            &add(
                NewFieldKind::ListBox,
                [20.0, 200.0, 140.0, 260.0],
                None,
                &["A", "B", "AB"],
            ),
        )
        .expect("added");
        let field = fields(&added)[0].clone();
        assert_eq!(field.kind, FieldKind::List);
        assert_eq!(field.options, ["A", "B", "AB"]);
        assert_eq!(field.name, "List1");
        let listed = drawn_lines(&added, field.widget);
        assert_eq!(listed, 3);
        let chosen = after(
            &added,
            &Command::FillField {
                page_index: 0,
                widget: field.widget,
                value: FieldValue::Text("B".to_owned()),
            },
        )
        .expect("chosen");
        assert_eq!(fields(&chosen)[0].value, FieldValue::Text("B".to_owned()));
        assert_eq!(
            drawn_lines(&chosen, field.widget),
            3,
            "the items are still all there"
        );
    }

    fn drawn_lines(source: &ByteStore, widget: Reference) -> usize {
        let reading = read_page(source, 0, b"", None).expect("reads");
        let painted = pdf_paint::interpret_annotations(
            &reading.program.annotations,
            reading.program.page,
            0,
            &reading.program.resources,
            pdf_paint::PaintLimits::default(),
            None,
        );
        painted
            .iter()
            .find(|annotation| annotation.reference == Some(widget))
            .map_or(0, |shown| {
                shown
                    .graph
                    .atoms
                    .iter()
                    .filter(|atom| matches!(atom.kind, pdf_paint::PaintAtomKind::Text(_)))
                    .count()
            })
    }

    #[test]
    fn a_date_field_says_what_a_date_looks_like() {
        let added = after(
            &no_form(),
            &add(NewFieldKind::Date, [20.0, 200.0, 120.0, 222.0], None, &[]),
        )
        .expect("added");
        let field = fields(&added)[0].clone();
        assert_eq!(field.kind, FieldKind::Text);
        assert_eq!(field.date_format.as_deref(), Some("dd/mm/yyyy"));
        assert_eq!(field.name, "Date1");
    }

    #[test]
    fn a_signature_field_is_not_filled_in() {
        let added = after(
            &no_form(),
            &add(
                NewFieldKind::Signature,
                [20.0, 200.0, 200.0, 240.0],
                None,
                &[],
            ),
        )
        .expect("added");
        let field = fields(&added)[0].clone();
        assert_eq!(field.kind, FieldKind::Signature);
        assert!(!field.is_fillable());
        assert!(
            after(
                &added,
                &Command::FillField {
                    page_index: 0,
                    widget: field.widget,
                    value: FieldValue::Text("me".to_owned()),
                },
            )
            .is_err()
        );
    }

    #[test]
    fn a_button_shows_its_caption() {
        let added = after(
            &no_form(),
            &add(
                NewFieldKind::Button,
                [20.0, 200.0, 100.0, 222.0],
                None,
                &["AB"],
            ),
        )
        .expect("added");
        let field = fields(&added)[0].clone();
        assert_eq!(field.kind, FieldKind::Push);
        assert_eq!(field.caption, "AB");
        assert_eq!(field.value, FieldValue::Empty, "a caption is not an answer");
        assert_eq!(drawn_lines(&added, field.widget), 1);
    }
}
