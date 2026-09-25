use std::collections::HashSet;
use std::sync::Arc;

use pdf_bytes::ByteStore;
use pdf_security::AuthenticatedSecurity;
use pdf_syntax::{
    DictionaryEntry, Object, ObjectKind, Reference, ResolveLimits, RevisionIndex, XrefLimits,
    parse_revision_chain_strict,
};

use crate::spike_move_text::SpikeError;

const MOST_STRING_BYTES: usize = 1 << 20;

const MOST_SIGNATURE_BYTES: usize = 1 << 20;
const MOST_FIELD_NODES: usize = 262_144;
const MOST_PARENT_DEPTH: usize = 64;
const MOST_OPTIONS: usize = 65_536;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldKind {
    Text,
    Checkbox,
    Radio,
    Push,
    Combo,
    List,
    Signature,
}

impl FieldKind {
    #[must_use]
    pub const fn holds_a_value(self) -> bool {
        !matches!(self, Self::Push | Self::Signature)
    }

    #[must_use]
    pub const fn is_a_button(self) -> bool {
        matches!(self, Self::Checkbox | Self::Radio)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FieldValue {
    Empty,
    Text(String),
    State(String),
}

impl FieldValue {
    #[must_use]
    pub fn shown(&self) -> &str {
        match self {
            Self::Empty => "",
            Self::Text(text) | Self::State(text) => text,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Quadding {
    #[default]
    Left,
    Centre,
    Right,
}

#[derive(Clone, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is one bit of `/Ff`, named as the specification names it"
)]
pub struct FormField {
    pub field: Reference,
    pub widget: Reference,
    pub kind: FieldKind,
    pub name: String,
    pub rect: [f64; 4],
    pub value: FieldValue,
    pub read_only: bool,
    pub required: bool,
    pub multiline: bool,
    pub password: bool,
    pub max_len: Option<usize>,
    pub quadding: Quadding,
    pub appearance: Vec<u8>,
    pub states: Vec<String>,
    pub options: Vec<String>,
    pub shown_state: Option<String>,
    pub border: Option<[f64; 3]>,
    pub background: Option<[f64; 3]>,
    pub flags: u32,
    pub tooltip: String,
    pub annotation_flags: u32,
    pub border_width: f64,
    pub border_style: BorderStyle,
    pub button_style: ButtonStyle,
    pub default_value: FieldValue,
    pub option_exports: Vec<String>,
    pub text_colour: [f64; 3],
    pub rotation: u16,
    pub date_format: Option<String>,
    pub caption: String,
    pub link: Option<String>,
}

impl FormField {
    #[must_use]
    pub fn is_fillable(&self) -> bool {
        !self.read_only && self.kind.holds_a_value()
    }

    #[must_use]
    pub fn text_size(&self) -> Option<f64> {
        crate::fill_field::size_in(&self.appearance)
    }

    #[must_use]
    pub fn is_on(&self) -> bool {
        match &self.value {
            FieldValue::State(state) | FieldValue::Text(state) => {
                state != "Off" && self.states.iter().any(|offered| offered == state)
            }
            FieldValue::Empty => false,
        }
    }
}

pub mod flags {
    pub const READ_ONLY: u32 = 1;
    pub const REQUIRED: u32 = 1 << 1;
    pub const NO_EXPORT: u32 = 1 << 2;
    pub const MULTILINE: u32 = 1 << 12;
    pub const PASSWORD: u32 = 1 << 13;
    pub const NO_TOGGLE_TO_OFF: u32 = 1 << 14;
    pub const RADIO: u32 = 1 << 15;
    pub const PUSHBUTTON: u32 = 1 << 16;
    pub const COMBO: u32 = 1 << 17;
    pub const EDIT: u32 = 1 << 18;
    pub const SORT: u32 = 1 << 19;
    pub const FILE_SELECT: u32 = 1 << 20;
    pub const MULTI_SELECT: u32 = 1 << 21;
    pub const DO_NOT_SPELL_CHECK: u32 = 1 << 22;
    pub const DO_NOT_SCROLL: u32 = 1 << 23;
    pub const COMB: u32 = 1 << 24;
    pub const RICH_TEXT: u32 = 1 << 25;
    pub const RADIOS_IN_UNISON: u32 = 1 << 25;
    pub const COMMIT_ON_SEL_CHANGE: u32 = 1 << 26;
}

use flags::{COMBO, MULTILINE, PASSWORD, PUSHBUTTON, RADIO, READ_ONLY, REQUIRED};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Visibility {
    #[default]
    Visible,
    Hidden,
    VisibleNotPrinted,
    HiddenPrinted,
}

impl Visibility {
    const HIDDEN: u32 = 1 << 1;
    const PRINT: u32 = 1 << 2;
    const NO_VIEW: u32 = 1 << 5;

    #[must_use]
    pub const fn of(bits: u32) -> Self {
        if bits & Self::HIDDEN != 0 {
            Self::Hidden
        } else if bits & Self::NO_VIEW != 0 {
            if bits & Self::PRINT != 0 {
                Self::HiddenPrinted
            } else {
                Self::Hidden
            }
        } else if bits & Self::PRINT != 0 {
            Self::Visible
        } else {
            Self::VisibleNotPrinted
        }
    }

    #[must_use]
    pub const fn applied_to(self, bits: u32) -> u32 {
        let rest = bits & !(Self::HIDDEN | Self::PRINT | Self::NO_VIEW);
        rest | match self {
            Self::Visible => Self::PRINT,
            Self::Hidden => Self::HIDDEN,
            Self::VisibleNotPrinted => 0,
            Self::HiddenPrinted => Self::NO_VIEW | Self::PRINT,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BorderStyle {
    #[default]
    Solid,
    Dashed,
    Beveled,
    Inset,
    Underline,
}

impl BorderStyle {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Solid => "S",
            Self::Dashed => "D",
            Self::Beveled => "B",
            Self::Inset => "I",
            Self::Underline => "U",
        }
    }

    fn named(name: &str) -> Self {
        match name {
            "D" => Self::Dashed,
            "B" => Self::Beveled,
            "I" => Self::Inset,
            "U" => Self::Underline,
            _ => Self::Solid,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ButtonStyle {
    #[default]
    Check,
    Circle,
    Cross,
    Diamond,
    Square,
    Star,
}

impl ButtonStyle {
    pub const ALL: [Self; 6] = [
        Self::Check,
        Self::Circle,
        Self::Cross,
        Self::Diamond,
        Self::Square,
        Self::Star,
    ];

    #[must_use]
    pub const fn character(self) -> char {
        match self {
            Self::Check => '4',
            Self::Circle => 'l',
            Self::Cross => '8',
            Self::Diamond => 'u',
            Self::Square => 'n',
            Self::Star => 'H',
        }
    }

    fn of_character(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|style| text.starts_with(style.character()))
    }
}

fn refused(reason: &'static str) -> SpikeError {
    SpikeError::RetypeUnsupported(reason)
}

#[derive(Clone)]
pub struct Found {
    pub(crate) source: ByteStore,
    pub(crate) value: Object,
    strings: Option<Reference>,
}

pub struct Reader {
    pub(crate) source: ByteStore,
    pub(crate) index: RevisionIndex,
    pub(crate) security: Option<Arc<AuthenticatedSecurity>>,
}

impl Reader {
    pub fn open(source: &ByteStore, credential: &[u8]) -> Result<Self, SpikeError> {
        let (index, security) = crate::previous::readable_index(source, credential)
            .ok_or_else(|| refused("this document cannot be opened to read its form"))?;
        Ok(Self {
            source: source.clone(),
            index,
            security,
        })
    }

    pub(crate) fn at(&self, reference: Reference) -> Option<Found> {
        let resolved = self
            .index
            .resolve_object(&self.source, reference, ResolveLimits::default())
            .ok()?;
        let strings = if self.security.is_none() || resolved.is_compressed() {
            None
        } else {
            Some(reference)
        };
        Some(Found {
            source: resolved.source().clone(),
            value: resolved.value().clone(),
            strings,
        })
    }

    pub(crate) fn follow(&self, holder: &Found, value: &Object) -> Option<Found> {
        match value.kind() {
            ObjectKind::Reference(reference) => self.at(*reference),
            _ => Some(Found {
                source: holder.source.clone(),
                value: value.clone(),
                strings: holder.strings,
            }),
        }
    }

    pub(crate) fn entry(&self, holder: &Found, key: &[u8]) -> Option<Found> {
        let ObjectKind::Dictionary(entries) = holder.value.kind() else {
            return None;
        };
        let value = entries
            .iter()
            .find(|entry| entry.key_equals(&holder.source, key))?
            .value();
        self.follow(holder, value)
    }

    fn raw_entry_of(holder: &Found, key: &[u8]) -> Option<()> {
        Self::raw_entry(holder, key).map(|_| ())
    }

    pub(crate) fn raw_entry<'a>(holder: &'a Found, key: &[u8]) -> Option<&'a Object> {
        let ObjectKind::Dictionary(entries) = holder.value.kind() else {
            return None;
        };
        entries
            .iter()
            .find(|entry| entry.key_equals(&holder.source, key))
            .map(DictionaryEntry::value)
    }

    pub(crate) fn text(&self, found: &Found) -> Option<String> {
        Some(text_string(&self.bytes(found)?))
    }

    pub(crate) fn bytes(&self, found: &Found) -> Option<Vec<u8>> {
        let bytes =
            pdf_syntax::decode_string(&found.source, &found.value, MOST_STRING_BYTES).ok()?;
        match (found.strings, self.security.as_ref()) {
            (Some(reference), Some(security)) => security.decrypt_string(reference, &bytes).ok(),
            _ => Some(bytes),
        }
    }

    pub(crate) fn written_bytes(found: &Found) -> Option<Vec<u8>> {
        pdf_syntax::decode_string(&found.source, &found.value, MOST_SIGNATURE_BYTES).ok()
    }

    pub(crate) fn name(found: &Found) -> Option<String> {
        let mut name = pdf_syntax::decode_name(&found.source, &found.value).ok()?;
        if name.first() == Some(&b'/') {
            name.remove(0);
        }
        String::from_utf8(name).ok()
    }

    pub(crate) fn number(found: &Found) -> Option<f64> {
        if !matches!(found.value.kind(), ObjectKind::Number(_)) {
            return None;
        }
        std::str::from_utf8(found.source.resolve(found.value.span()).ok()?)
            .ok()?
            .parse::<f64>()
            .ok()
    }

    pub(crate) fn catalog(&self) -> Option<Found> {
        let chain = parse_revision_chain_strict(&self.source, XrefLimits::default()).ok()?;
        let root = chain.revisions().iter().find_map(|revision| {
            let ObjectKind::Dictionary(entries) = revision.trailer().kind() else {
                return None;
            };
            let value = entries
                .iter()
                .find(|entry| entry.key_equals(&self.source, b"/Root"))?
                .value();
            match value.kind() {
                ObjectKind::Reference(reference) => Some(*reference),
                _ => None,
            }
        })?;
        self.at(root)
    }

    pub(crate) fn catalog_reference(&self) -> Option<Reference> {
        let chain = parse_revision_chain_strict(&self.source, XrefLimits::default()).ok()?;
        chain.revisions().iter().find_map(|revision| {
            let ObjectKind::Dictionary(entries) = revision.trailer().kind() else {
                return None;
            };
            let value = entries
                .iter()
                .find(|entry| entry.key_equals(&self.source, b"/Root"))?
                .value();
            match value.kind() {
                ObjectKind::Reference(reference) => Some(*reference),
                _ => None,
            }
        })
    }

    pub(crate) fn acro_form(&self) -> Option<Found> {
        self.entry(&self.catalog()?, b"/AcroForm")
    }

    pub(crate) fn inherited(&self, node: &Found, key: &[u8]) -> Option<Found> {
        let mut here = node.clone();
        for _ in 0..MOST_PARENT_DEPTH {
            if let Some(found) = self.entry(&here, key) {
                return Some(found);
            }
            here = self.entry(&here, b"/Parent")?;
        }
        None
    }

    pub(crate) fn field_node(&self, widget: Reference) -> Option<(Reference, Found)> {
        let mut reference = widget;
        let mut here = self.at(widget)?;
        for _ in 0..MOST_PARENT_DEPTH {
            if Self::raw_entry(&here, b"/FT").is_some() {
                return Some((reference, here));
            }
            let parent = Self::raw_entry(&here, b"/Parent")?;
            let ObjectKind::Reference(next) = parent.kind() else {
                return None;
            };
            reference = *next;
            here = self.at(*next)?;
        }
        None
    }
}

fn text_string(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(b"\xfe\xff") {
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if let Some(rest) = bytes.strip_prefix(b"\xef\xbb\xbf") {
        return String::from_utf8_lossy(rest).into_owned();
    }
    bytes.iter().map(|byte| char::from(*byte)).collect()
}

pub fn fields_of_page(
    source: &ByteStore,
    page: Reference,
    credential: &[u8],
) -> Result<Vec<FormField>, SpikeError> {
    let reader = Reader::open(source, credential)?;
    let page = reader
        .at(page)
        .ok_or_else(|| refused("this page cannot be read to find its form fields"))?;
    let Some(annots) = reader.entry(&page, b"/Annots") else {
        return Ok(Vec::new());
    };
    let ObjectKind::Array(items) = annots.value.kind() else {
        return Ok(Vec::new());
    };
    let members = field_nodes(&reader);
    let mut fields = Vec::new();
    for item in items {
        let ObjectKind::Reference(widget) = item.kind() else {
            continue;
        };
        if !members.contains(widget) {
            continue;
        }
        if let Some(field) = read_field(&reader, *widget) {
            fields.push(field);
        }
    }
    Ok(fields)
}

pub(crate) fn named_nodes(reader: &Reader) -> Vec<(String, Reference, Option<FieldKind>)> {
    let mut named: Vec<(String, Reference, Option<FieldKind>)> = field_nodes(reader)
        .into_iter()
        .filter_map(|reference| {
            let node = reader.at(reference)?;
            Reader::raw_entry_of(&node, b"/T")?;
            Some((full_name(reader, &node), reference, kind_of(reader, &node)))
        })
        .collect();
    named.sort_by(|one, other| one.0.cmp(&other.0));
    named
}

pub(crate) fn signature_nodes(reader: &Reader) -> Vec<(String, Found)> {
    let mut found: Vec<(String, Found)> = field_nodes(reader)
        .into_iter()
        .filter_map(|reference| {
            let node = reader.at(reference)?;
            let kind = Reader::name(&reader.inherited(&node, b"/FT")?)?;
            (kind == "Sig").then(|| (full_name(reader, &node), node))
        })
        .collect();
    found.sort_by(|one, other| one.0.cmp(&other.0));
    found
}

#[must_use]
pub fn field_names(source: &ByteStore, credential: &[u8]) -> Vec<String> {
    Reader::open(source, credential).map_or_else(
        |_| Vec::new(),
        |reader| {
            named_nodes(&reader)
                .into_iter()
                .map(|(name, _, _)| name)
                .collect()
        },
    )
}

pub fn fields_of_document(
    source: &ByteStore,
    credential: &[u8],
) -> Result<Vec<(usize, FormField)>, SpikeError> {
    let pages = pdf_content::page_references_with_password(
        source,
        pdf_content::PageContentLimits::default(),
        credential,
    )
    .map_err(|_| refused("this document's pages cannot be walked to list its form"))?;
    let mut found = Vec::new();
    for (index, page) in pages.into_iter().enumerate() {
        for field in fields_of_page(source, page, credential).unwrap_or_default() {
            found.push((index, field));
        }
    }
    Ok(found)
}

pub(crate) fn field_nodes(reader: &Reader) -> HashSet<Reference> {
    let mut nodes = HashSet::new();
    let Some(form) = reader.acro_form() else {
        return nodes;
    };
    let Some(fields) = reader.entry(&form, b"/Fields") else {
        return nodes;
    };
    let mut pending = Vec::new();
    push_references(&fields, &mut pending);
    while let Some(reference) = pending.pop() {
        if !nodes.insert(reference) || nodes.len() > MOST_FIELD_NODES {
            continue;
        }
        let Some(node) = reader.at(reference) else {
            continue;
        };
        if let Some(kids) = reader.entry(&node, b"/Kids") {
            push_references(&kids, &mut pending);
        }
    }
    nodes
}

fn push_references(array: &Found, pending: &mut Vec<Reference>) {
    if let ObjectKind::Array(items) = array.value.kind() {
        pending.extend(items.iter().filter_map(|item| match item.kind() {
            ObjectKind::Reference(reference) => Some(*reference),
            _ => None,
        }));
    }
}

pub(crate) fn read_field(reader: &Reader, widget: Reference) -> Option<FormField> {
    let annotation = reader.at(widget)?;
    if reader
        .entry(&annotation, b"/Subtype")
        .and_then(|found| Reader::name(&found))
        .as_deref()
        != Some("Widget")
    {
        return None;
    }
    let (field, node) = reader.field_node(widget)?;
    let kind = kind_of(reader, &node)?;
    let flags = flags_of(reader, &node);
    let rect = rect_of(reader, &annotation)?;
    Some(FormField {
        field,
        widget,
        kind,
        name: full_name(reader, &node),
        rect,
        value: value_of(reader, &node),
        read_only: flags & READ_ONLY != 0,
        required: flags & REQUIRED != 0,
        multiline: kind == FieldKind::Text && flags & MULTILINE != 0,
        password: kind == FieldKind::Text && flags & PASSWORD != 0,
        max_len: max_len_of(reader, &node),
        quadding: quadding_of(reader, &node),
        appearance: appearance_of(reader, &node),
        states: if kind.is_a_button() {
            states_of(reader, &annotation)
        } else {
            Vec::new()
        },
        options: options_of(reader, &node),
        shown_state: reader
            .entry(&annotation, b"/AS")
            .and_then(|found| Reader::name(&found)),
        border: characteristic(reader, &annotation, b"/BC"),
        background: characteristic(reader, &annotation, b"/BG"),
        flags,
        tooltip: reader
            .inherited(&node, b"/TU")
            .and_then(|found| reader.text(&found))
            .unwrap_or_default(),
        annotation_flags: annotation_flags_of(reader, &annotation),
        border_width: border_width_of(reader, &annotation),
        border_style: reader
            .entry(&annotation, b"/BS")
            .and_then(|style| reader.entry(&style, b"/S"))
            .and_then(|found| Reader::name(&found))
            .map_or(BorderStyle::Solid, |name| BorderStyle::named(&name)),
        button_style: button_style_of(reader, &annotation, kind),
        default_value: default_of(reader, &node),
        option_exports: option_exports_of(reader, &node),
        text_colour: crate::fill_field::colour_in(&appearance_of(reader, &node)),
        date_format: date_format_of(reader, &node, &annotation),
        caption: if kind == FieldKind::Push {
            reader
                .entry(&annotation, b"/MK")
                .and_then(|looks| reader.entry(&looks, b"/CA"))
                .and_then(|found| reader.text(&found))
                .unwrap_or_default()
        } else {
            String::new()
        },
        link: link_of(reader, &annotation),
        rotation: reader
            .entry(&annotation, b"/MK")
            .and_then(|looks| reader.entry(&looks, b"/R"))
            .and_then(|found| Reader::number(&found))
            .filter(|turn| [90.0, 180.0, 270.0].contains(turn))
            .map_or(0, |turn| {
                #[allow(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "one of three whole numbers of degrees"
                )]
                let turn = turn as u16;
                turn
            }),
    })
}

fn annotation_flags_of(reader: &Reader, annotation: &Found) -> u32 {
    reader
        .entry(annotation, b"/F")
        .and_then(|found| Reader::number(&found))
        .filter(|bits| bits.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(bits))
        .map_or(0, |bits| {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "an integral value checked to lie within u32"
            )]
            let bits = bits as u32;
            bits
        })
}

fn button_style_of(reader: &Reader, annotation: &Found, kind: FieldKind) -> ButtonStyle {
    reader
        .entry(annotation, b"/MK")
        .and_then(|looks| reader.entry(&looks, b"/CA"))
        .and_then(|found| reader.text(&found))
        .and_then(|text| ButtonStyle::of_character(&text))
        .unwrap_or(if kind == FieldKind::Radio {
            ButtonStyle::Circle
        } else {
            ButtonStyle::Check
        })
}

fn link_of(reader: &Reader, annotation: &Found) -> Option<String> {
    let action = reader.entry(annotation, b"/A")?;
    if reader
        .entry(&action, b"/S")
        .and_then(|found| Reader::name(&found))
        .as_deref()
        != Some("URI")
    {
        return None;
    }
    reader.text(&reader.entry(&action, b"/URI")?)
}

fn date_format_of(reader: &Reader, node: &Found, annotation: &Found) -> Option<String> {
    let actions = reader
        .entry(annotation, b"/AA")
        .or_else(|| reader.inherited(node, b"/AA"))?;
    let format = reader.entry(&actions, b"/F")?;
    let script = reader.text(&reader.entry(&format, b"/JS")?)?;
    let start = script.find("AFDate_FormatEx(")? + "AFDate_FormatEx(".len();
    let rest = script[start..].trim_start();
    let quote = rest
        .chars()
        .next()
        .filter(|mark| *mark == '"' || *mark == '\'')?;
    let inner = &rest[1..];
    let end = inner.find(quote)?;
    Some(inner[..end].to_owned())
}

fn border_width_of(reader: &Reader, annotation: &Found) -> f64 {
    reader
        .entry(annotation, b"/BS")
        .and_then(|style| reader.entry(&style, b"/W"))
        .and_then(|found| Reader::number(&found))
        .filter(|width| width.is_finite() && (0.0..=100.0).contains(width))
        .unwrap_or(1.0)
}

fn default_of(reader: &Reader, node: &Found) -> FieldValue {
    let Some(value) = reader.inherited(node, b"/DV") else {
        return FieldValue::Empty;
    };
    match value.value.kind() {
        ObjectKind::Name => Reader::name(&value).map_or(FieldValue::Empty, FieldValue::State),
        ObjectKind::LiteralString | ObjectKind::HexString => reader
            .text(&value)
            .map_or(FieldValue::Empty, FieldValue::Text),
        _ => FieldValue::Empty,
    }
}

fn characteristic(reader: &Reader, annotation: &Found, key: &[u8]) -> Option<[f64; 3]> {
    let looks = reader.entry(annotation, b"/MK")?;
    let colour = reader.entry(&looks, key)?;
    let ObjectKind::Array(items) = colour.value.kind() else {
        return None;
    };
    let parts: Vec<f64> = items
        .iter()
        .map(|item| {
            reader
                .follow(&colour, item)
                .and_then(|found| Reader::number(&found))
        })
        .collect::<Option<Vec<f64>>>()?;
    if !parts.iter().all(|part| (0.0..=1.0).contains(part)) {
        return None;
    }
    match parts.as_slice() {
        [grey] => Some([*grey; 3]),
        [red, green, blue] => Some([*red, *green, *blue]),
        [cyan, magenta, yellow, black] => Some([
            (1.0 - cyan) * (1.0 - black),
            (1.0 - magenta) * (1.0 - black),
            (1.0 - yellow) * (1.0 - black),
        ]),
        _ => None,
    }
}

fn kind_of(reader: &Reader, node: &Found) -> Option<FieldKind> {
    let kind = Reader::name(&reader.inherited(node, b"/FT")?)?;
    let flags = flags_of(reader, node);
    Some(match kind.as_str() {
        "Tx" => FieldKind::Text,
        "Sig" => FieldKind::Signature,
        "Btn" if flags & PUSHBUTTON != 0 => FieldKind::Push,
        "Btn" if flags & RADIO != 0 => FieldKind::Radio,
        "Btn" => FieldKind::Checkbox,
        "Ch" if flags & COMBO != 0 => FieldKind::Combo,
        "Ch" => FieldKind::List,
        _ => return None,
    })
}

fn flags_of(reader: &Reader, node: &Found) -> u32 {
    reader
        .inherited(node, b"/Ff")
        .and_then(|found| Reader::number(&found))
        .filter(|value| value.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(value))
        .map_or(0, |value| {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "an integral value checked to lie within u32"
            )]
            let bits = value as u32;
            bits
        })
}

fn rect_of(reader: &Reader, annotation: &Found) -> Option<[f64; 4]> {
    let rect = reader.entry(annotation, b"/Rect")?;
    let ObjectKind::Array(items) = rect.value.kind() else {
        return None;
    };
    let corners: Vec<f64> = items
        .iter()
        .filter_map(|item| Reader::number(&reader.follow(&rect, item)?))
        .collect();
    let [x0, y0, x1, y1] = <[f64; 4]>::try_from(corners).ok()?;
    if ![x0, y0, x1, y1].iter().all(|edge| edge.is_finite()) {
        return None;
    }
    Some([x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)])
}

fn full_name(reader: &Reader, node: &Found) -> String {
    let mut parts = Vec::new();
    let mut here = node.clone();
    for _ in 0..MOST_PARENT_DEPTH {
        if let Some(part) = reader
            .entry(&here, b"/T")
            .and_then(|found| reader.text(&found))
        {
            parts.push(part);
        }
        let Some(parent) = reader.entry(&here, b"/Parent") else {
            break;
        };
        here = parent;
    }
    parts.reverse();
    parts.join(".")
}

fn value_of(reader: &Reader, node: &Found) -> FieldValue {
    let Some(value) = reader.inherited(node, b"/V") else {
        return FieldValue::Empty;
    };
    match value.value.kind() {
        ObjectKind::Name => Reader::name(&value).map_or(FieldValue::Empty, FieldValue::State),
        ObjectKind::LiteralString | ObjectKind::HexString => reader
            .text(&value)
            .map_or(FieldValue::Empty, FieldValue::Text),
        ObjectKind::Array(items) => items
            .first()
            .and_then(|item| reader.follow(&value, item))
            .and_then(|first| reader.text(&first))
            .map_or(FieldValue::Empty, FieldValue::Text),
        _ => FieldValue::Empty,
    }
}

fn max_len_of(reader: &Reader, node: &Found) -> Option<usize> {
    let value = Reader::number(&reader.inherited(node, b"/MaxLen")?)?;
    if value.fract() != 0.0 || !(1.0..=1e9).contains(&value) {
        return None;
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "an integral value checked to lie within a small range"
    )]
    Some(value as usize)
}

fn quadding_of(reader: &Reader, node: &Found) -> Quadding {
    match reader
        .inherited(node, b"/Q")
        .and_then(|found| Reader::number(&found))
    {
        Some(value) if (value - 1.0).abs() < f64::EPSILON => Quadding::Centre,
        Some(value) if (value - 2.0).abs() < f64::EPSILON => Quadding::Right,
        _ => Quadding::Left,
    }
}

fn appearance_of(reader: &Reader, node: &Found) -> Vec<u8> {
    let from = |found: &Found| reader.bytes(found);
    if let Some(own) = reader.inherited(node, b"/DA").as_ref().and_then(from) {
        return own;
    }
    reader
        .acro_form()
        .and_then(|form| reader.entry(&form, b"/DA"))
        .as_ref()
        .and_then(from)
        .unwrap_or_default()
}

fn states_of(reader: &Reader, annotation: &Found) -> Vec<String> {
    let Some(appearance) = reader.entry(annotation, b"/AP") else {
        return Vec::new();
    };
    let Some(normal) = reader.entry(&appearance, b"/N") else {
        return Vec::new();
    };
    let ObjectKind::Dictionary(entries) = normal.value.kind() else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let mut key = entry.decoded_key(&normal.source).ok()?;
            if key.first() == Some(&b'/') {
                key.remove(0);
            }
            String::from_utf8(key).ok()
        })
        .filter(|state| state != "Off")
        .collect()
}

fn option_exports_of(reader: &Reader, node: &Found) -> Vec<String> {
    let Some(options) = reader.inherited(node, b"/Opt") else {
        return Vec::new();
    };
    let ObjectKind::Array(items) = options.value.kind() else {
        return Vec::new();
    };
    items
        .iter()
        .take(MOST_OPTIONS)
        .filter_map(|item| {
            let found = reader.follow(&options, item)?;
            match found.value.kind() {
                ObjectKind::Array(pair) => reader.text(&reader.follow(&found, pair.first()?)?),
                _ => reader.text(&found),
            }
        })
        .collect()
}

fn options_of(reader: &Reader, node: &Found) -> Vec<String> {
    let Some(options) = reader.inherited(node, b"/Opt") else {
        return Vec::new();
    };
    let ObjectKind::Array(items) = options.value.kind() else {
        return Vec::new();
    };
    items
        .iter()
        .take(MOST_OPTIONS)
        .filter_map(|item| {
            let found = reader.follow(&options, item)?;
            match found.value.kind() {
                ObjectKind::Array(pair) => {
                    let shown = pair.get(1).or_else(|| pair.first())?;
                    reader.text(&reader.follow(&found, shown)?)
                }
                _ => reader.text(&found),
            }
        })
        .collect()
}

#[cfg(test)]
#[expect(
    clippy::float_cmp,
    reason = "a rectangle written as numbers is read back as the same numbers"
)]
mod tests {
    use pdf_bytes::{ByteStore, SourceId};
    use pdf_syntax::Reference;

    use super::{FieldKind, FieldValue, Quadding, fields_of_page};

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

    fn blank() -> String {
        "<< /Type /XObject /Subtype /Form /BBox [0 0 100 20] /Length 0 >>\nstream\n\nendstream"
            .to_owned()
    }

    fn a_form() -> ByteStore {
        document(&[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R 7 0 R 10 0 R] \
             /DA (/Helv 0 Tf 0 g) >> >>"
                .to_owned(),
            "<< /Type /Pages /MediaBox [0 0 300 300] /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /Contents 11 0 R /Resources << >> \
             /Annots [4 0 R 5 0 R 8 0 R 9 0 R 10 0 R 12 0 R] >>"
                .to_owned(),
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /V (Phan) /Rect [10 260 200 280] \
             /Q 1 /MaxLen 40 /DA (/Helv 11 Tf 0 g) /AP << /N 13 0 R >> >>"
                .to_owned(),
            "<< /Type /Annot /Subtype /Widget /FT /Btn /T (Agree) /V /Yes /AS /Yes \
             /Rect [10 230 26 246] /AP << /N << /Yes 13 0 R /Off 13 0 R >> >> >>"
                .to_owned(),
            "<< /Type /Null >>".to_owned(),
            "<< /FT /Btn /Ff 32768 /T (Size) /V /Large /Kids [8 0 R 9 0 R] >>".to_owned(),
            "<< /Type /Annot /Subtype /Widget /Parent 7 0 R /Rect [10 200 26 216] \
             /AS /Large /AP << /N << /Large 13 0 R /Off 13 0 R >> >> >>"
                .to_owned(),
            "<< /Type /Annot /Subtype /Widget /Parent 7 0 R /Rect [40 200 56 216] \
             /AS /Off /AP << /N << /Small 13 0 R /Off 13 0 R >> >> >>"
                .to_owned(),
            "<< /Type /Annot /Subtype /Widget /FT /Ch /Ff 131073 /T (Country) /V (Thailand) \
             /Opt [(Thailand) [(LA) (Laos)]] /Rect [10 170 200 190] /AP << /N 13 0 R >> >>"
                .to_owned(),
            "<< /Length 0 >>\nstream\n\nendstream".to_owned(),
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Stray) /Rect [10 140 200 160] >>"
                .to_owned(),
            blank(),
        ])
    }

    fn read(source: &ByteStore) -> Vec<super::FormField> {
        fields_of_page(source, Reference::new(3, 0), b"").expect("the page's fields")
    }

    #[test]
    fn a_page_lists_the_boxes_that_can_be_filled() {
        let fields = read(&a_form());
        let names: Vec<&str> = fields.iter().map(|field| field.name.as_str()).collect();
        assert_eq!(names, ["Name", "Agree", "Size", "Size", "Country"]);
        let kinds: Vec<FieldKind> = fields.iter().map(|field| field.kind).collect();
        assert_eq!(
            kinds,
            [
                FieldKind::Text,
                FieldKind::Checkbox,
                FieldKind::Radio,
                FieldKind::Radio,
                FieldKind::Combo
            ]
        );
    }

    #[test]
    fn a_text_field_says_what_it_holds() {
        let fields = read(&a_form());
        let text = &fields[0];
        assert_eq!(text.value, FieldValue::Text("Phan".to_owned()));
        assert_eq!(text.rect, [10.0, 260.0, 200.0, 280.0]);
        assert_eq!(text.quadding, Quadding::Centre);
        assert_eq!(text.max_len, Some(40));
        assert_eq!(text.appearance, b"/Helv 11 Tf 0 g");
        assert!(text.is_fillable());
        assert!(!text.multiline);
    }

    #[test]
    fn a_ticked_checkbox_is_on() {
        let fields = read(&a_form());
        let box_ = &fields[1];
        assert_eq!(box_.states, ["Yes"]);
        assert_eq!(box_.value, FieldValue::State("Yes".to_owned()));
        assert!(box_.is_on());
    }

    #[test]
    fn a_radio_group_turns_on_the_button_the_value_names() {
        let fields = read(&a_form());
        let (large, small) = (&fields[2], &fields[3]);
        assert_eq!(large.field, Reference::new(7, 0));
        assert_eq!(small.field, Reference::new(7, 0));
        assert_eq!(large.states, ["Large"]);
        assert_eq!(small.states, ["Small"]);
        assert!(large.is_on());
        assert!(!small.is_on());
    }

    #[test]
    fn a_read_only_choice_offers_its_options_and_refuses_to_be_filled() {
        let fields = read(&a_form());
        let choice = &fields[4];
        assert_eq!(choice.options, ["Thailand", "Laos"]);
        assert_eq!(choice.value, FieldValue::Text("Thailand".to_owned()));
        assert!(choice.read_only);
        assert!(!choice.is_fillable());
    }

    #[test]
    fn a_field_falls_back_to_the_form_s_appearance() {
        let fields = read(&a_form());
        assert_eq!(fields[1].appearance, b"/Helv 0 Tf 0 g");
    }

    #[test]
    fn a_value_written_as_utf_16_reads_as_its_characters() {
        let mut objects: Vec<String> = Vec::new();
        objects
            .push("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] >> >>".to_owned());
        objects
            .push("<< /Type /Pages /MediaBox [0 0 300 300] /Kids [3 0 R] /Count 1 >>".to_owned());
        objects.push(
            "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << >> /Annots [4 0 R] >>"
                .to_owned(),
        );
        let hex: String = std::iter::once("FEFF".to_owned())
            .chain("ผาน".encode_utf16().map(|unit| format!("{unit:04X}")))
            .collect();
        objects.push(format!(
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /V <{hex}> /Rect [0 0 10 10] >>"
        ));
        objects.push("<< /Length 0 >>\nstream\n\nendstream".to_owned());
        let fields = read(&document(&objects));
        assert_eq!(fields[0].value, FieldValue::Text("ผาน".to_owned()));
    }

    #[test]
    fn a_visibility_changes_only_its_own_bits() {
        use super::Visibility;
        let locked_no_zoom = (1 << 7) | (1 << 3);
        for choice in [
            Visibility::Visible,
            Visibility::Hidden,
            Visibility::VisibleNotPrinted,
            Visibility::HiddenPrinted,
        ] {
            let bits = choice.applied_to(locked_no_zoom | 4);
            assert_eq!(Visibility::of(bits), choice);
            assert_eq!(bits & locked_no_zoom, locked_no_zoom, "{choice:?}");
        }
    }

    #[test]
    fn a_page_without_a_form_lists_nothing() {
        let source = document(&[
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /MediaBox [0 0 300 300] /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>".to_owned(),
            "<< /Length 0 >>\nstream\n\nendstream".to_owned(),
        ]);
        assert!(read(&source).is_empty());
    }
}
