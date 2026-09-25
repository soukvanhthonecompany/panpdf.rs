use pdf_bytes::ByteStore;
use pdf_syntax::Reference;

use crate::fill_field::{
    Beside, Entry, pdf_text_string, set_entries, typed, with_colour, with_size,
};
use crate::form::{
    BorderStyle, ButtonStyle, FieldKind, FieldValue, FormField, Quadding, Reader, Visibility,
};
use crate::plan::{Capability, Effect, Plan, PlannedBody, PlannedWrite};
use crate::spike_move_text::{PlannerPage, SpikeError};

const READ_ONLY: u32 = 1;
const REQUIRED: u32 = 1 << 1;
const SAME_SIZE: f64 = 1e-6;

fn refused(reason: &'static str) -> SpikeError {
    SpikeError::RetypeUnsupported(reason)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldSettings {
    pub name: Option<String>,
    pub tooltip: Option<String>,
    pub visibility: Option<Visibility>,
    pub required: Option<bool>,
    pub read_only: Option<bool>,
    pub border: Option<Option<[f64; 3]>>,
    pub fill: Option<Option<[f64; 3]>>,
    pub border_width: Option<f64>,
    pub border_style: Option<BorderStyle>,
    pub text_colour: Option<[f64; 3]>,
    pub text_size: Option<Option<f64>>,
    pub set_flags: u32,
    pub clear_flags: u32,
    pub quadding: Option<Quadding>,
    pub default_value: Option<String>,
    pub max_len: Option<Option<usize>>,
    pub options: Option<Vec<(String, String)>>,
    pub button_style: Option<ButtonStyle>,
    pub export_value: Option<String>,
    pub checked_by_default: Option<bool>,
    pub caption: Option<String>,
    pub link: Option<Option<String>>,
    pub date_format: Option<Option<String>>,
}

impl FieldSettings {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

const fn flags_of_kind(kind: FieldKind) -> u32 {
    use crate::form::flags;
    let every = flags::READ_ONLY | flags::REQUIRED | flags::NO_EXPORT;
    every
        | match kind {
            FieldKind::Text => {
                flags::MULTILINE
                    | flags::PASSWORD
                    | flags::DO_NOT_SPELL_CHECK
                    | flags::DO_NOT_SCROLL
                    | flags::COMB
            }
            FieldKind::Combo => {
                flags::EDIT | flags::SORT | flags::DO_NOT_SPELL_CHECK | flags::COMMIT_ON_SEL_CHANGE
            }
            FieldKind::List => flags::SORT | flags::MULTI_SELECT | flags::COMMIT_ON_SEL_CHANGE,
            FieldKind::Radio => flags::NO_TOGGLE_TO_OFF | flags::RADIOS_IN_UNISON,
            FieldKind::Checkbox | FieldKind::Push | FieldKind::Signature => 0,
        }
}

fn field_on_page(
    (source, credential): (&ByteStore, &[u8]),
    page: &PlannerPage<'_>,
    widget: Reference,
) -> Result<(Reader, FormField), SpikeError> {
    let reader = Reader::open(source, credential)?;
    let field = crate::form::fields_of_page(source, page.program.page, credential)?
        .into_iter()
        .find(|field| field.widget == widget)
        .ok_or_else(|| refused("this is not a field of the document's form"))?;
    Ok((reader, field))
}

fn plan_of(
    page: &PlannerPage<'_>,
    page_index: usize,
    writes: Vec<PlannedWrite>,
    region: [f64; 4],
) -> Plan {
    let target = page
        .program
        .streams
        .first()
        .map_or(page.program.page, |stream| stream.reference);
    Plan::new(
        Capability::Exact,
        writes,
        Effect {
            page_index,
            moved: Vec::new(),
            target_stream: target,
            declared_region: Some(region),
        },
    )
}

const fn draws_its_value(kind: FieldKind) -> bool {
    matches!(kind, FieldKind::Text | FieldKind::Combo | FieldKind::List)
}

pub(crate) fn plan_set_field_box(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    (widget, rect): (Reference, [f64; 4]),
) -> Result<Plan, SpikeError> {
    let credential = page.credential;
    let rect = crate::new_field::checked_box(rect)?;
    let (_, field) = field_on_page((source, credential), &page, widget)?;
    let resized = ((field.rect[2] - field.rect[0]) - (rect[2] - rect[0])).abs() > SAME_SIZE
        || ((field.rect[3] - field.rect[1]) - (rect[3] - rect[1])).abs() > SAME_SIZE;
    let placed: Entry = (
        b"/Rect",
        format!("[{} {} {} {}]", rect[0], rect[1], rect[2], rect[3]),
    );
    let writes = if resized && draws_its_value(field.kind) {
        let mut moved = field.clone();
        moved.rect = rect;
        typed(
            source,
            page,
            page_index,
            (
                &moved,
                &Beside {
                    field: Vec::new(),
                    widget: vec![placed],
                    keep_value: false,
                },
            ),
            &field.value,
        )?
    } else {
        vec![set_entries((source, credential), widget, &[placed])?]
    };
    let document = crate::block_rewrite::commit_writes(
        source,
        &writes,
        (credential, crate::Restrictions::SetAside),
    )?;
    let back = crate::form::fields_of_page(&document, page.program.page, credential)?;
    let landed = back
        .iter()
        .find(|found| found.widget == widget)
        .is_some_and(|found| {
            found
                .rect
                .iter()
                .zip(rect)
                .all(|(one, other)| (one - other).abs() < 1e-6)
        });
    if !landed {
        return Err(refused("the field does not read back in the box asked for"));
    }
    let region = [
        field.rect[0].min(rect[0]),
        field.rect[1].min(rect[1]),
        field.rect[2].max(rect[2]),
        field.rect[3].max(rect[3]),
    ];
    Ok(plan_of(&page, page_index, writes, region))
}

pub(crate) fn plan_set_field_settings(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    (widget, settings): (Reference, &FieldSettings),
) -> Result<Plan, SpikeError> {
    let credential = page.credential;
    if settings.is_empty() {
        return Err(refused("nothing about the field was changed"));
    }
    let (reader, field) = field_on_page((source, credential), &page, widget)?;
    let mut change = Change::of(&field);
    general(&reader, &field, settings, &mut change)?;
    appearance(&field, settings, &mut change)?;
    options(&field, settings, &mut change)?;
    buttons(&field, settings, &mut change)?;
    pushbuttons(&field, settings, &mut change)?;
    dates(&field, settings, &mut change)?;

    let writes = if draws_its_value(field.kind) && change.redraw {
        typed(
            source,
            page,
            page_index,
            (
                &change.wanted,
                &Beside {
                    field: change.on_field.clone(),
                    widget: change.on_widget.clone(),
                    keep_value: false,
                },
            ),
            &field.value,
        )?
    } else if field.kind == FieldKind::Push && change.redraw {
        typed(
            source,
            page,
            page_index,
            (
                &change.wanted,
                &Beside {
                    field: change.on_field.clone(),
                    widget: change.on_widget.clone(),
                    keep_value: true,
                },
            ),
            &FieldValue::Text(change.wanted.caption.clone()),
        )?
    } else if field.kind.is_a_button() && change.redraw {
        pressed((source, credential), &field, &mut change)?
    } else {
        change.plain_writes((source, credential), &field)?
    };
    prove_settings(
        (source, credential),
        &writes,
        (page.program.page, widget),
        &change.wanted,
    )?;
    Ok(plan_of(&page, page_index, writes, field.rect))
}

struct Change {
    wanted: FormField,
    on_field: Vec<Entry>,
    on_widget: Vec<Entry>,
    redraw: bool,
    old_state: Option<String>,
}

impl Change {
    fn of(field: &FormField) -> Self {
        Self {
            wanted: field.clone(),
            on_field: Vec::new(),
            on_widget: Vec::new(),
            redraw: false,
            old_state: None,
        }
    }

    fn plain_writes(
        &self,
        (source, credential): (&ByteStore, &[u8]),
        field: &FormField,
    ) -> Result<Vec<PlannedWrite>, SpikeError> {
        if field.field == field.widget {
            let both: Vec<Entry> = self
                .on_field
                .iter()
                .chain(&self.on_widget)
                .cloned()
                .collect();
            return Ok(vec![set_entries((source, credential), field.field, &both)?]);
        }
        let mut writes = Vec::new();
        if !self.on_field.is_empty() {
            writes.push(set_entries(
                (source, credential),
                field.field,
                &self.on_field,
            )?);
        }
        if !self.on_widget.is_empty() {
            writes.push(set_entries(
                (source, credential),
                field.widget,
                &self.on_widget,
            )?);
        }
        Ok(writes)
    }
}

fn general(
    reader: &Reader,
    field: &FormField,
    settings: &FieldSettings,
    change: &mut Change,
) -> Result<(), SpikeError> {
    if let Some(name) = &settings.name {
        let name = crate::new_field::checked_name(name)?;
        let taken = crate::form::named_nodes(reader)
            .into_iter()
            .any(|(other, node, _)| other == name && node != field.field);
        if taken {
            return Err(refused("a field of that name is already in this form"));
        }
        change.on_field.push((b"/T", pdf_text_string(name)));
        name.clone_into(&mut change.wanted.name);
    }
    if let Some(tooltip) = &settings.tooltip {
        if tooltip.chars().any(char::is_control) {
            return Err(refused("a tooltip is a line of text"));
        }
        let text = if tooltip.is_empty() {
            String::new()
        } else {
            pdf_text_string(tooltip)
        };
        change.on_field.push((b"/TU", text));
        change.wanted.tooltip.clone_from(tooltip);
    }
    if let Some(visibility) = settings.visibility {
        let bits = visibility.applied_to(field.annotation_flags);
        change.on_widget.push((b"/F", bits.to_string()));
        change.wanted.annotation_flags = bits;
    }
    let mut flags = field.flags;
    for (asked, bit) in [
        (settings.required, REQUIRED),
        (settings.read_only, READ_ONLY),
    ] {
        match asked {
            Some(true) => flags |= bit,
            Some(false) => flags &= !bit,
            None => {}
        }
    }
    let allowed = flags_of_kind(field.kind);
    if (settings.set_flags | settings.clear_flags) & !allowed != 0 {
        return Err(refused("this kind of field has no such option"));
    }
    flags = (flags | settings.set_flags) & !settings.clear_flags;
    if flags != field.flags {
        use crate::form::flags as bit;
        let comb = flags & bit::COMB != 0;
        if comb && flags & (bit::MULTILINE | bit::PASSWORD) != 0 {
            return Err(refused(
                "a comb holds one character to a cell, so it is neither several lines nor a password",
            ));
        }
        let looks = bit::MULTILINE | bit::PASSWORD | bit::COMB;
        if (flags ^ field.flags) & looks != 0 {
            change.redraw = true;
        }
        change.on_field.push((b"/Ff", flags.to_string()));
        change.wanted.flags = flags;
        change.wanted.required = flags & REQUIRED != 0;
        change.wanted.read_only = flags & READ_ONLY != 0;
        change.wanted.multiline = field.kind == FieldKind::Text && flags & bit::MULTILINE != 0;
        change.wanted.password = field.kind == FieldKind::Text && flags & bit::PASSWORD != 0;
    }
    Ok(())
}

fn appearance(
    field: &FormField,
    settings: &FieldSettings,
    change: &mut Change,
) -> Result<(), SpikeError> {
    let colour_ok = |colour: &[f64; 3]| colour.iter().all(|part| (0.0..=1.0).contains(part));
    let mut looks = false;
    if let Some(border) = settings.border {
        if border.is_some_and(|colour| !colour_ok(&colour)) {
            return Err(refused("a colour is three numbers from zero to one"));
        }
        change.wanted.border = border;
        looks = true;
    }
    if let Some(fill) = settings.fill {
        if fill.is_some_and(|colour| !colour_ok(&colour)) {
            return Err(refused("a colour is three numbers from zero to one"));
        }
        change.wanted.background = fill;
        looks = true;
    }
    if let Some(width) = settings.border_width {
        if !(width.is_finite() && (0.0..=12.0).contains(&width)) {
            return Err(refused("a border is between nought and twelve points wide"));
        }
        change.wanted.border_width = width;
        looks = true;
    }
    if let Some(style) = settings.border_style {
        change.wanted.border_style = style;
        looks = true;
    }
    if looks {
        change.on_widget.push((b"/MK", looks_entry(&change.wanted)));
        change.on_widget.push((
            b"/BS",
            format!(
                "<< /W {} /S /{} >>",
                change.wanted.border_width,
                change.wanted.border_style.name()
            ),
        ));
        change.redraw = true;
    }
    let mut appearance = field.appearance.clone();
    if let Some(colour) = settings.text_colour {
        if !colour_ok(&colour) {
            return Err(refused("a colour is three numbers from zero to one"));
        }
        appearance = with_colour(&appearance, colour).into_bytes();
        change.wanted.text_colour = colour;
    }
    if let Some(size) = settings.text_size {
        if !draws_its_value(field.kind) {
            return Err(refused("only a field of text has a text size"));
        }
        if size.is_some_and(|size| !(size.is_finite() && (1.0..=300.0).contains(&size))) {
            return Err(refused("a text size is a number of points"));
        }
        appearance = with_size(&appearance, size).into_bytes();
    }
    if appearance != field.appearance {
        change
            .on_field
            .push((b"/DA", pdf_literal(&String::from_utf8_lossy(&appearance))));
        change.wanted.appearance = appearance;
        change.redraw = true;
    }
    Ok(())
}

fn looks_entry(field: &FormField) -> String {
    let colour = |[red, green, blue]: [f64; 3]| format!("[{red} {green} {blue}]");
    let mut out = String::from("<<");
    if let Some(border) = field.border {
        out.push_str(" /BC ");
        out.push_str(&colour(border));
    }
    if let Some(fill) = field.background {
        out.push_str(" /BG ");
        out.push_str(&colour(fill));
    }
    if field.kind.is_a_button() {
        out.push_str(" /CA (");
        out.push(field.button_style.character());
        out.push(')');
    } else if field.kind == FieldKind::Push && !field.caption.is_empty() {
        out.push_str(" /CA ");
        out.push_str(&pdf_literal(&field.caption));
    }
    if field.rotation != 0 {
        out.push_str(" /R ");
        out.push_str(&field.rotation.to_string());
    }
    out.push_str(" >>");
    out
}

fn options(
    field: &FormField,
    settings: &FieldSettings,
    change: &mut Change,
) -> Result<(), SpikeError> {
    use crate::form::flags as bit;
    if let Some(quadding) = settings.quadding {
        if !draws_its_value(field.kind) {
            return Err(refused("only a field of text has an alignment"));
        }
        let number = match quadding {
            Quadding::Left => 0,
            Quadding::Centre => 1,
            Quadding::Right => 2,
        };
        change.on_field.push((b"/Q", number.to_string()));
        change.wanted.quadding = quadding;
        change.redraw = true;
    }
    if let Some(most) = settings.max_len {
        if field.kind != FieldKind::Text {
            return Err(refused("only a text field has a length limit"));
        }
        if most.is_some_and(|most| most == 0 || most > 32_768) {
            return Err(refused("a length limit is a number of characters"));
        }
        change.on_field.push((
            b"/MaxLen",
            most.map_or_else(String::new, |most| most.to_string()),
        ));
        change.wanted.max_len = most;
        change.redraw = true;
    }
    if change.wanted.flags & bit::COMB != 0 && change.wanted.max_len.is_none() {
        return Err(refused(
            "a comb needs a length limit: it is how many cells there are",
        ));
    }
    if let Some(default) = &settings.default_value {
        if !draws_its_value(field.kind) {
            return Err(refused("a button's default is whether it is on"));
        }
        let text = if default.is_empty() {
            String::new()
        } else {
            pdf_text_string(default)
        };
        change.on_field.push((b"/DV", text));
        change.wanted.default_value = if default.is_empty() {
            FieldValue::Empty
        } else {
            FieldValue::Text(default.clone())
        };
    }
    if let Some(offered) = &settings.options {
        if !matches!(field.kind, FieldKind::Combo | FieldKind::List) {
            return Err(refused("only a list offers options"));
        }
        let lines = |text: &str| !text.is_empty() && !text.chars().any(char::is_control);
        if offered.is_empty()
            || !offered
                .iter()
                .all(|(export, shown)| lines(export) && lines(shown))
        {
            return Err(refused(
                "a list offers at least one option, each a line of text",
            ));
        }
        let listed: Vec<String> = offered
            .iter()
            .map(|(export, shown)| {
                if export == shown {
                    pdf_text_string(shown)
                } else {
                    format!("[{} {}]", pdf_text_string(export), pdf_text_string(shown))
                }
            })
            .collect();
        change
            .on_field
            .push((b"/Opt", format!("[{}]", listed.join(" "))));
        change.wanted.options = offered.iter().map(|(_, shown)| shown.clone()).collect();
        change.wanted.option_exports = offered.iter().map(|(export, _)| export.clone()).collect();
        change.redraw = true;
    }
    Ok(())
}

fn buttons(
    field: &FormField,
    settings: &FieldSettings,
    change: &mut Change,
) -> Result<(), SpikeError> {
    let asked = settings.button_style.is_some()
        || settings.export_value.is_some()
        || settings.checked_by_default.is_some();
    if !asked {
        return Ok(());
    }
    if !field.kind.is_a_button() {
        return Err(refused(
            "only a checkbox or radio button has a style and export value",
        ));
    }
    if let Some(style) = settings.button_style {
        change.wanted.button_style = style;
        change
            .on_widget
            .retain(|(key, _)| *key != b"/MK".as_slice());
        change.on_widget.push((b"/MK", looks_entry(&change.wanted)));
        change.redraw = true;
    }
    let state = field
        .states
        .first()
        .cloned()
        .unwrap_or_else(|| "Yes".to_owned());
    if let Some(export) = &settings.export_value {
        let named = !export.is_empty()
            && export != "Off"
            && export.len() < 128
            && export
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'));
        if !named {
            return Err(refused(
                "an export value is a name of letters, digits, stops, dashes and underscores",
            ));
        }
        if *export != state {
            change.old_state = Some(state.clone());
            change.wanted.states = vec![export.clone()];
            if field.value == FieldValue::State(state.clone()) {
                change.on_field.push((b"/V", format!("/{export}")));
                change.wanted.value = FieldValue::State(export.clone());
            }
            change.redraw = true;
        }
    }
    let on = change.wanted.states.first().cloned().unwrap_or(state);
    if let Some(checked) = settings.checked_by_default {
        let default = if checked {
            format!("/{on}")
        } else {
            String::new()
        };
        change.on_field.push((b"/DV", default));
        change.wanted.default_value = if checked {
            FieldValue::State(on)
        } else {
            FieldValue::Empty
        };
    }
    Ok(())
}

fn pushbuttons(
    field: &FormField,
    settings: &FieldSettings,
    change: &mut Change,
) -> Result<(), SpikeError> {
    if settings.caption.is_none() && settings.link.is_none() {
        return Ok(());
    }
    if field.kind != FieldKind::Push {
        return Err(refused("only a button has a caption and a link"));
    }
    if let Some(caption) = &settings.caption {
        if caption.is_empty() || caption.chars().any(char::is_control) {
            return Err(refused("a button's caption is a line of text"));
        }
        change.wanted.caption.clone_from(caption);
        change
            .on_widget
            .retain(|(key, _)| *key != b"/MK".as_slice());
        change.on_widget.push((b"/MK", looks_entry(&change.wanted)));
        change.redraw = true;
    }
    if let Some(link) = &settings.link {
        match link {
            Some(address) => {
                if address.is_empty() || address.chars().any(char::is_control) {
                    return Err(refused("a link is a web address on one line"));
                }
                change.on_widget.push((
                    b"/A",
                    format!("<< /S /URI /URI {} >>", pdf_literal(address)),
                ));
            }
            None => change.on_widget.push((b"/A", String::new())),
        }
        change.wanted.link.clone_from(link);
    }
    Ok(())
}

fn dates(
    field: &FormField,
    settings: &FieldSettings,
    change: &mut Change,
) -> Result<(), SpikeError> {
    let Some(format) = &settings.date_format else {
        return Ok(());
    };
    if field.kind != FieldKind::Text {
        return Err(refused("only a text field can be read as a date"));
    }
    match format {
        Some(format) => {
            let named = !format.is_empty()
                && format.len() < 64
                && format
                    .chars()
                    .all(|letter| letter.is_ascii_alphanumeric() || "/-. :".contains(letter));
            if !named {
                return Err(refused(
                    "a date format is written in letters, digits and the marks between them",
                ));
            }
            change.on_field.push((
                b"/AA",
                format!(
                    "<< /F << /S /JavaScript /JS {} >> /K << /S /JavaScript /JS {} >> >>",
                    pdf_literal(&format!("AFDate_FormatEx(\"{format}\");")),
                    pdf_literal(&format!("AFDate_KeystrokeEx(\"{format}\");"))
                ),
            ));
        }
        None => change.on_field.push((b"/AA", String::new())),
    }
    change.wanted.date_format.clone_from(format);
    Ok(())
}

fn pressed(
    (source, credential): (&ByteStore, &[u8]),
    field: &FormField,
    change: &mut Change,
) -> Result<Vec<PlannedWrite>, SpikeError> {
    let wanted = &change.wanted;
    let round = field.kind == FieldKind::Radio;
    let look = crate::fill_field::look_of(wanted, round);
    let first = crate::block_rewrite::next_object_number(source)?;
    let (on, off) = (Reference::new(first, 0), Reference::new(first + 1, 0));
    let stream = |reference: Reference, content: String| PlannedWrite {
        reference,
        body: PlannedBody::NewStream {
            dictionary: format!(
                "/Type /XObject /Subtype /Form /FormType 1 /BBox [0 0 {} {}] /Resources << >>",
                look.width, look.height
            )
            .into_bytes(),
            decoded: content.into_bytes(),
        },
    };
    let frame = crate::field_look::frame(&look);
    let marked =
        frame.clone() + &crate::field_look::mark(wanted.button_style, &look, wanted.text_colour);
    let state = wanted
        .states
        .first()
        .cloned()
        .unwrap_or_else(|| "Yes".to_owned());
    let mut writes = vec![stream(on, marked), stream(off, frame)];
    change
        .on_widget
        .retain(|(key, _)| *key != b"/AP".as_slice() && *key != b"/AS".as_slice());
    change.on_widget.push((
        b"/AP",
        format!(
            "<< /N << /{state} {} {} R /Off {} {} R >> >>",
            on.object_number(),
            on.generation(),
            off.object_number(),
            off.generation()
        ),
    ));
    let was_on = field
        .shown_state
        .as_ref()
        .is_some_and(|shown| shown != "Off" && field.states.contains(shown));
    change.on_widget.push((
        b"/AS",
        if was_on {
            format!("/{state}")
        } else {
            "/Off".to_owned()
        },
    ));
    change.wanted.shown_state = Some(if was_on { state } else { "Off".to_owned() });
    writes.extend(change.plain_writes((source, credential), field)?);
    Ok(writes)
}

fn prove_settings(
    (source, credential): (&ByteStore, &[u8]),
    writes: &[PlannedWrite],
    (page, widget): (Reference, Reference),
    wanted: &FormField,
) -> Result<(), SpikeError> {
    let document = crate::block_rewrite::commit_writes(
        source,
        writes,
        (credential, crate::Restrictions::SetAside),
    )?;
    let back = crate::form::fields_of_page(&document, page, credential)?;
    let found = back
        .iter()
        .find(|found| found.widget == widget)
        .ok_or_else(|| refused("the field is no longer read on the page"))?;
    let close = |one: f64, other: f64| (one - other).abs() < 1e-6;
    let colours = |one: Option<[f64; 3]>, other: Option<[f64; 3]>| match (one, other) {
        (None, None) => true,
        (Some(one), Some(other)) => one.iter().zip(other).all(|(a, b)| close(*a, b)),
        _ => false,
    };
    let same = found.name == wanted.name
        && found.tooltip == wanted.tooltip
        && found.flags == wanted.flags
        && found.annotation_flags == wanted.annotation_flags
        && found.options == wanted.options
        && found.option_exports == wanted.option_exports
        && found.quadding == wanted.quadding
        && found.text_size() == wanted.text_size()
        && colours(Some(found.text_colour), Some(wanted.text_colour))
        && found.max_len == wanted.max_len
        && found.default_value == wanted.default_value
        && colours(found.border, wanted.border)
        && colours(found.background, wanted.background)
        && close(found.border_width, wanted.border_width)
        && found.border_style == wanted.border_style
        && found.button_style == wanted.button_style
        && found.states == wanted.states
        && found.value == wanted.value
        && found.caption == wanted.caption
        && found.link == wanted.link
        && found.date_format == wanted.date_format;
    if !same {
        return Err(refused(
            "the field does not read back with the settings asked for",
        ));
    }
    Ok(())
}

pub(crate) fn pdf_literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('(');
    for letter in text.chars() {
        if matches!(letter, '(' | ')' | '\\') {
            out.push('\\');
        }
        out.push(letter);
    }
    out.push(')');
    out
}

#[cfg(test)]
#[expect(
    clippy::float_cmp,
    reason = "a box written as numbers is read back as the same numbers"
)]
mod tests {
    use pdf_bytes::{ByteStore, SourceId};
    use pdf_paint::{PaintAtomKind, PaintLimits, Point};
    use pdf_syntax::Reference;

    use super::FieldSettings;
    use crate::form::{FieldValue, FormField, Quadding, fields_of_page};
    use crate::new_field::NewFieldKind;
    use crate::plan::Command;
    use crate::spike_move_text::{SpikeError, plan_command_with_fonts, read_page};

    fn document(objects: &[&str]) -> ByteStore {
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

    fn two_fields() -> ByteStore {
        let blank = document(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /MediaBox [0 0 300 300] /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>",
            "<< /Length 0 >>\nstream\n\nendstream",
        ]);
        let add = |source: &ByteStore, rect| {
            after(
                source,
                &Command::AddField {
                    page_index: 0,
                    rect,
                    kind: NewFieldKind::Text,
                    name: None,
                    options: Vec::new(),
                },
            )
            .expect("added")
        };
        let one = add(&blank, [20.0, 200.0, 120.0, 220.0]);
        let two = add(&one, [20.0, 150.0, 120.0, 170.0]);
        let widget = fields(&two)[0].widget;
        after(
            &two,
            &Command::FillField {
                page_index: 0,
                widget,
                value: FieldValue::Text("AB".to_owned()),
            },
        )
        .expect("filled")
    }

    fn pens(source: &ByteStore, widget: Reference) -> Vec<(f64, f64)> {
        let reading = read_page(source, 0, b"", None).expect("reads");
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
            .find(|annotation| annotation.reference == Some(widget))
            .expect("drawn");
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

    fn boxed(
        source: &ByteStore,
        widget: Reference,
        rect: [f64; 4],
    ) -> Result<ByteStore, SpikeError> {
        after(
            source,
            &Command::SetFieldBox {
                page_index: 0,
                widget,
                rect,
            },
        )
    }

    fn set(
        source: &ByteStore,
        widget: Reference,
        settings: FieldSettings,
    ) -> Result<ByteStore, SpikeError> {
        after(
            source,
            &Command::SetFieldSettings {
                page_index: 0,
                widget,
                settings,
            },
        )
    }

    #[test]
    fn a_moved_field_takes_its_value_with_it() {
        let source = two_fields();
        let field = fields(&source)[0].clone();
        let before = pens(&source, field.widget);
        let moved = boxed(&source, field.widget, [60.0, 100.0, 160.0, 120.0]).expect("moved");
        let back = fields(&moved)[0].clone();
        assert_eq!(back.rect, [60.0, 100.0, 160.0, 120.0]);
        assert_eq!(back.value, FieldValue::Text("AB".to_owned()));
        let after = pens(&moved, field.widget);
        assert_eq!(after.len(), 2);
        assert!((after[0].0 - before[0].0 - 40.0).abs() < 1e-6);
        assert!((after[0].1 - before[0].1 + 100.0).abs() < 1e-6);
    }

    #[test]
    fn a_resized_field_draws_its_value_again() {
        let source = two_fields();
        let field = fields(&source)[0].clone();
        let resized = boxed(&source, field.widget, [20.0, 200.0, 220.0, 260.0]).expect("resized");
        let after = pens(&resized, field.widget);
        assert_eq!(after.len(), 2);
        assert!(after[0].1 > 210.0, "{}", after[0].1);
        for (wide, tall) in scales(&resized, field.widget) {
            assert!(
                (wide - tall).abs() < 1e-6,
                "{wide} wide against {tall} tall"
            );
        }
    }

    fn scales(source: &ByteStore, widget: Reference) -> Vec<(f64, f64)> {
        let reading = read_page(source, 0, b"", None).expect("reads");
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
            .find(|annotation| annotation.reference == Some(widget))
            .expect("drawn");
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
                    let placed = text.state.ctm.value.multiply(glyph.matrix);
                    (placed.a.hypot(placed.b), placed.c.hypot(placed.d))
                })
            })
            .collect()
    }

    #[test]
    fn a_box_too_small_is_refused() {
        let source = two_fields();
        let field = fields(&source)[0].clone();
        assert!(boxed(&source, field.widget, [20.0, 200.0, 22.0, 202.0]).is_err());
    }

    #[test]
    fn a_field_is_renamed_but_not_onto_another() {
        let source = two_fields();
        let listed = fields(&source);
        let renamed = set(
            &source,
            listed[1].widget,
            FieldSettings {
                name: Some("Email".to_owned()),
                ..FieldSettings::default()
            },
        )
        .expect("renamed");
        assert_eq!(fields(&renamed)[1].name, "Email");
        let clash = set(
            &renamed,
            listed[1].widget,
            FieldSettings {
                name: Some("Text1".to_owned()),
                ..FieldSettings::default()
            },
        );
        assert!(clash.is_err());
    }

    #[test]
    fn required_and_read_only_change_only_their_own_bits() {
        let source = two_fields();
        let widget = fields(&source)[0].widget;
        let set_on = set(
            &source,
            widget,
            FieldSettings {
                required: Some(true),
                read_only: Some(true),
                ..FieldSettings::default()
            },
        )
        .expect("set");
        let field = fields(&set_on)[0].clone();
        assert!(field.required && field.read_only);
        let cleared = set(
            &set_on,
            widget,
            FieldSettings {
                read_only: Some(false),
                ..FieldSettings::default()
            },
        )
        .expect("cleared");
        let field = fields(&cleared)[0].clone();
        assert!(field.required && !field.read_only);
    }

    #[test]
    fn aligning_a_filled_field_redraws_it() {
        let source = two_fields();
        let field = fields(&source)[0].clone();
        let right = set(
            &source,
            field.widget,
            FieldSettings {
                quadding: Some(Quadding::Right),
                text_size: Some(Some(10.0)),
                ..FieldSettings::default()
            },
        )
        .expect("aligned");
        let back = fields(&right)[0].clone();
        assert_eq!(back.quadding, Quadding::Right);
        assert_eq!(back.text_size(), Some(10.0));
        let after = pens(&right, field.widget);
        assert!(
            (after[0].0 - (120.0 - 2.0 - 7.5)).abs() < 1e-6,
            "{}",
            after[0].0
        );
    }

    #[test]
    fn changing_nothing_is_refused() {
        let source = two_fields();
        let widget = fields(&source)[0].widget;
        assert!(set(&source, widget, FieldSettings::default()).is_err());
    }

    #[test]
    fn options_belong_to_lists() {
        let source = two_fields();
        let widget = fields(&source)[0].widget;
        let refused = set(
            &source,
            widget,
            FieldSettings {
                options: Some(vec![("A".to_owned(), "A".to_owned())]),
                ..FieldSettings::default()
            },
        );
        assert!(refused.is_err());
    }

    #[test]
    fn a_tooltip_and_visibility_are_set_and_a_tooltip_cleared() {
        use crate::form::Visibility;
        let source = two_fields();
        let widget = fields(&source)[1].widget;
        let set_on = set(
            &source,
            widget,
            FieldSettings {
                tooltip: Some("Your e-mail".to_owned()),
                visibility: Some(Visibility::VisibleNotPrinted),
                ..FieldSettings::default()
            },
        )
        .expect("set");
        let field = fields(&set_on)[1].clone();
        assert_eq!(field.tooltip, "Your e-mail");
        assert_eq!(
            Visibility::of(field.annotation_flags),
            Visibility::VisibleNotPrinted
        );
        let cleared = set(
            &set_on,
            widget,
            FieldSettings {
                tooltip: Some(String::new()),
                ..FieldSettings::default()
            },
        )
        .expect("cleared");
        assert_eq!(fields(&cleared)[1].tooltip, "");
    }

    #[test]
    fn a_border_fill_and_text_colour_are_set_and_the_value_redrawn() {
        use crate::form::BorderStyle;
        let source = two_fields();
        let field = fields(&source)[0].clone();
        let styled = set(
            &source,
            field.widget,
            FieldSettings {
                border: Some(Some([0.0, 0.0, 1.0])),
                fill: Some(Some([1.0, 1.0, 0.8])),
                border_width: Some(2.0),
                border_style: Some(BorderStyle::Dashed),
                text_colour: Some([1.0, 0.0, 0.0]),
                ..FieldSettings::default()
            },
        )
        .expect("styled");
        let back = fields(&styled)[0].clone();
        assert_eq!(back.border, Some([0.0, 0.0, 1.0]));
        assert_eq!(back.background, Some([1.0, 1.0, 0.8]));
        assert_eq!(back.border_width, 2.0);
        assert_eq!(back.border_style, BorderStyle::Dashed);
        assert_eq!(back.text_colour, [1.0, 0.0, 0.0]);
        assert_eq!(
            pens(&styled, field.widget).len(),
            2,
            "the value still draws"
        );
        let bare = set(
            &styled,
            field.widget,
            FieldSettings {
                border: Some(None),
                ..FieldSettings::default()
            },
        )
        .expect("bare");
        assert_eq!(fields(&bare)[0].border, None);
    }

    #[test]
    fn a_text_field_takes_its_own_options_and_no_others() {
        use crate::form::flags;
        let source = two_fields();
        let widget = fields(&source)[1].widget;
        let set_on = set(
            &source,
            widget,
            FieldSettings {
                set_flags: flags::MULTILINE | flags::DO_NOT_SPELL_CHECK,
                max_len: Some(Some(20)),
                default_value: Some("none".to_owned()),
                ..FieldSettings::default()
            },
        )
        .expect("set");
        let field = fields(&set_on)[1].clone();
        assert!(field.multiline);
        assert_eq!(field.max_len, Some(20));
        assert_eq!(field.default_value, FieldValue::Text("none".to_owned()));
        assert!(
            set(
                &source,
                widget,
                FieldSettings {
                    set_flags: flags::SORT,
                    ..FieldSettings::default()
                },
            )
            .is_err(),
            "sorting is a list's"
        );
        assert!(
            set(
                &source,
                widget,
                FieldSettings {
                    set_flags: flags::COMB,
                    ..FieldSettings::default()
                },
            )
            .is_err(),
            "a comb with no cells"
        );
        assert!(
            set(
                &set_on,
                widget,
                FieldSettings {
                    set_flags: flags::COMB,
                    ..FieldSettings::default()
                },
            )
            .is_err(),
            "a comb on a field of several lines"
        );
        let unlimited = set(
            &set_on,
            widget,
            FieldSettings {
                max_len: Some(None),
                ..FieldSettings::default()
            },
        )
        .expect("unlimited");
        assert_eq!(fields(&unlimited)[1].max_len, None);
    }

    #[test]
    fn a_comb_puts_one_character_in_each_cell() {
        use crate::form::flags;
        let source = two_fields();
        let field = fields(&source)[0].clone();
        let combed = set(
            &source,
            field.widget,
            FieldSettings {
                set_flags: flags::COMB,
                max_len: Some(Some(4)),
                text_size: Some(Some(10.0)),
                ..FieldSettings::default()
            },
        )
        .expect("combed");
        let pens = pens(&combed, field.widget);
        assert_eq!(pens.len(), 2);
        assert!(
            (pens[0].0 - (20.0 + 2.0 + 12.0 - 2.5)).abs() < 1e-6,
            "{}",
            pens[0].0
        );
        assert!(
            (pens[1].0 - (20.0 + 2.0 + 36.0 - 1.25)).abs() < 1e-6,
            "{}",
            pens[1].0
        );
    }

    #[test]
    fn a_list_keeps_export_values_apart_from_what_it_shows() {
        let source = two_fields();
        let dropdown = after(
            &source,
            &Command::AddField {
                page_index: 0,
                rect: [20.0, 100.0, 120.0, 120.0],
                kind: NewFieldKind::Dropdown,
                name: None,
                options: vec!["One".to_owned()],
            },
        )
        .expect("added");
        let widget = fields(&dropdown).last().expect("the list").widget;
        let offered = set(
            &dropdown,
            widget,
            FieldSettings {
                options: Some(vec![
                    ("TH".to_owned(), "Thailand".to_owned()),
                    ("Laos".to_owned(), "Laos".to_owned()),
                ]),
                ..FieldSettings::default()
            },
        )
        .expect("offered");
        let field = fields(&offered).last().expect("the list").clone();
        assert_eq!(field.options, ["Thailand", "Laos"]);
        assert_eq!(field.option_exports, ["TH", "Laos"]);
    }

    #[test]
    fn a_checkbox_changes_its_mark_and_export_value_and_stays_ticked() {
        use crate::form::ButtonStyle;
        let source = two_fields();
        let boxed = after(
            &source,
            &Command::AddField {
                page_index: 0,
                rect: [200.0, 200.0, 214.0, 214.0],
                kind: NewFieldKind::Checkbox,
                name: None,
                options: Vec::new(),
            },
        )
        .expect("added");
        let widget = fields(&boxed).last().expect("the box").widget;
        let ticked = after(
            &boxed,
            &Command::FillField {
                page_index: 0,
                widget,
                value: FieldValue::State("Yes".to_owned()),
            },
        )
        .expect("ticked");
        let changed = set(
            &ticked,
            widget,
            FieldSettings {
                button_style: Some(ButtonStyle::Star),
                export_value: Some("Agreed".to_owned()),
                checked_by_default: Some(true),
                ..FieldSettings::default()
            },
        )
        .expect("changed");
        let field = fields(&changed).last().expect("the box").clone();
        assert_eq!(field.button_style, ButtonStyle::Star);
        assert_eq!(field.states, ["Agreed"]);
        assert_eq!(field.value, FieldValue::State("Agreed".to_owned()));
        assert!(field.is_on());
        assert_eq!(field.shown_state.as_deref(), Some("Agreed"));
        assert_eq!(field.default_value, FieldValue::State("Agreed".to_owned()));
        assert!(
            set(
                &changed,
                widget,
                FieldSettings {
                    export_value: Some("Off".to_owned()),
                    ..FieldSettings::default()
                },
            )
            .is_err()
        );
    }

    #[test]
    fn a_button_takes_a_caption_and_a_link() {
        let source = two_fields();
        let placed = after(
            &source,
            &Command::AddField {
                page_index: 0,
                rect: [20.0, 60.0, 100.0, 82.0],
                kind: NewFieldKind::Button,
                name: None,
                options: vec!["AB".to_owned()],
            },
        )
        .expect("added");
        let widget = fields(&placed).last().expect("the button").widget;
        let set_on = set(
            &placed,
            widget,
            FieldSettings {
                caption: Some("BA".to_owned()),
                link: Some(Some("https://example.com/form".to_owned())),
                ..FieldSettings::default()
            },
        )
        .expect("set");
        let field = fields(&set_on).last().expect("the button").clone();
        assert_eq!(field.caption, "BA");
        assert_eq!(field.link.as_deref(), Some("https://example.com/form"));
        assert_eq!(field.value, FieldValue::Empty, "a caption is not an answer");
        let unlinked = set(
            &set_on,
            widget,
            FieldSettings {
                link: Some(None),
                ..FieldSettings::default()
            },
        )
        .expect("unlinked");
        assert_eq!(fields(&unlinked).last().expect("the button").link, None);
        let text = fields(&source)[0].widget;
        assert!(
            set(
                &source,
                text,
                FieldSettings {
                    caption: Some("no".to_owned()),
                    ..FieldSettings::default()
                },
            )
            .is_err()
        );
    }

    #[test]
    fn a_text_field_becomes_a_date_and_back() {
        let source = two_fields();
        let widget = fields(&source)[1].widget;
        let dated = set(
            &source,
            widget,
            FieldSettings {
                date_format: Some(Some("yyyy-mm-dd".to_owned())),
                ..FieldSettings::default()
            },
        )
        .expect("dated");
        assert_eq!(fields(&dated)[1].date_format.as_deref(), Some("yyyy-mm-dd"));
        let plain = set(
            &dated,
            widget,
            FieldSettings {
                date_format: Some(None),
                ..FieldSettings::default()
            },
        )
        .expect("plain");
        assert_eq!(fields(&plain)[1].date_format, None);
        assert!(
            set(
                &source,
                widget,
                FieldSettings {
                    date_format: Some(Some("dd(mm)yyyy".to_owned())),
                    ..FieldSettings::default()
                },
            )
            .is_err(),
            "a format is letters, digits and the marks between them"
        );
    }
}
