use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceSpan};
use pdf_security::AuthenticatedSecurity;
use pdf_syntax::{DictionaryEntry, Object, ObjectKind, Reference, RevisionIndex};

use super::{
    FormXObject, OptionalContent, PageContentError, PageContentErrorKind, PageContentLimits,
    ResourceLoadState, dictionary, entry, load_form_stream, page_rectangle, resolve_indirect,
    resolve_number,
};

const MAX_ANNOTATIONS: usize = 65_536;
const MAX_FIELD_NODES: usize = 262_144;
const MAX_PARENT_DEPTH: usize = 64;

#[derive(Clone, Debug, Default)]
pub struct Annotations {
    pub entries: Vec<Annotation>,
    pub unreadable: Vec<UnreadableAnnotation>,
    pub need_appearances: bool,
    pub form_unreadable: Option<PageContentError>,
}

#[derive(Clone, Debug)]
pub struct UnreadableAnnotation {
    pub reference: Option<Reference>,
    pub error: PageContentError,
}

#[derive(Clone, Debug)]
pub struct Annotation {
    pub reference: Option<Reference>,
    pub source: ByteStore,
    pub dictionary: Object,
    pub subtype: Option<Vec<u8>>,
    pub rect: [f64; 4],
    pub rect_span: SourceSpan,
    pub flags: AnnotationFlags,
    pub field: bool,
    pub appearance: Appearance,
}

impl Annotation {
    #[must_use]
    pub fn is(&self, name: &[u8]) -> bool {
        self.subtype.as_deref() == Some(name)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AnnotationFlags {
    pub bits: u32,
    pub malformed: bool,
}

impl AnnotationFlags {
    pub const INVISIBLE: u32 = 1;
    pub const HIDDEN: u32 = 1 << 1;
    pub const PRINT: u32 = 1 << 2;
    pub const NO_ZOOM: u32 = 1 << 3;
    pub const NO_ROTATE: u32 = 1 << 4;
    pub const NO_VIEW: u32 = 1 << 5;

    #[must_use]
    pub const fn has(self, flag: u32) -> bool {
        self.bits & flag != 0
    }
}

#[derive(Clone, Debug)]
pub enum Appearance {
    Absent,
    MissingState {
        state: Vec<u8>,
    },
    Stream {
        form: Arc<FormXObject>,
        span: SourceSpan,
    },
    Unreadable(PageContentError),
}

pub(super) struct AnnotationContext<'a> {
    pub(super) document: &'a ByteStore,
    pub(super) index: &'a Arc<RevisionIndex>,
    pub(super) security: Option<&'a Arc<AuthenticatedSecurity>>,
    pub(super) catalog: Reference,
    pub(super) optional_content: Arc<OptionalContent>,
    pub(super) limits: PageContentLimits,
}

pub(super) fn load_annotations(
    context: &AnnotationContext<'_>,
    page_source: &ByteStore,
    page_entries: &[DictionaryEntry],
) -> Annotations {
    let mut annotations = Annotations::default();
    let Some(value) = entry(page_entries, page_source, b"/Annots") else {
        return annotations;
    };
    let (source, list) = match resolve_indirect(
        context.document,
        context.index,
        page_source,
        value,
        context.limits,
    ) {
        Ok(found) => found,
        Err(error) => {
            annotations.unreadable.push(UnreadableAnnotation {
                reference: None,
                error,
            });
            return annotations;
        }
    };
    let items = match list.kind() {
        ObjectKind::Array(items) => items,
        ObjectKind::Null => return annotations,
        _ => {
            annotations.unreadable.push(UnreadableAnnotation {
                reference: None,
                error: PageContentError::new(PageContentErrorKind::AnnotsNotArray),
            });
            return annotations;
        }
    };
    let mut form = FormMembership::default();
    for item in items.iter().take(MAX_ANNOTATIONS) {
        let reference = match item.kind() {
            ObjectKind::Reference(reference) => Some(*reference),
            _ => None,
        };
        match load_annotation(context, &source, item, reference, &mut form) {
            Ok(Some(annotation)) => annotations.entries.push(annotation),
            Ok(None) => {}
            Err(error) => annotations
                .unreadable
                .push(UnreadableAnnotation { reference, error }),
        }
    }
    if items.len() > MAX_ANNOTATIONS {
        annotations.unreadable.push(UnreadableAnnotation {
            reference: None,
            error: PageContentError::new(PageContentErrorKind::AnnotationLimit),
        });
    }
    if let Some(Err(error)) = form.nodes {
        annotations.form_unreadable = Some(error);
    }
    annotations.need_appearances = need_appearances(context);
    annotations
}

fn load_annotation(
    context: &AnnotationContext<'_>,
    list_source: &ByteStore,
    item: &Object,
    reference: Option<Reference>,
    form: &mut FormMembership,
) -> Result<Option<Annotation>, PageContentError> {
    let (source, value) = resolve_indirect(
        context.document,
        context.index,
        list_source,
        item,
        context.limits,
    )?;
    let Some(entries) = dictionary(&value) else {
        if matches!(value.kind(), ObjectKind::Null) {
            return Ok(None);
        }
        return Err(PageContentError::new(
            PageContentErrorKind::AnnotationNotDictionary,
        ));
    };
    let rect_value = entry(entries, &source, b"/Rect")
        .ok_or_else(|| PageContentError::new(PageContentErrorKind::InvalidAnnotationRect))?;
    let rect = page_rectangle(
        context.document,
        context.index,
        &source,
        rect_value,
        context.limits,
        PageContentErrorKind::InvalidAnnotationRect,
    )?;
    let subtype = entry(entries, &source, b"/Subtype")
        .and_then(|object| pdf_syntax::decode_name(&source, object).ok())
        .map(without_solidus);
    let flags = flags(context, &source, entries);
    let field = subtype.as_deref() == Some(b"Widget".as_slice())
        && reference.is_some_and(|reference| form.contains(context, reference))
        && has_field_type(context, &source, entries);
    let appearance = appearance(context, &source, entries);
    Ok(Some(Annotation {
        reference,
        source: source.clone(),
        dictionary: value.clone(),
        subtype,
        rect,
        rect_span: rect_value.span(),
        flags,
        field,
        appearance,
    }))
}

fn without_solidus(mut name: Vec<u8>) -> Vec<u8> {
    if name.first() == Some(&b'/') {
        name.remove(0);
    }
    name
}

fn flags(
    context: &AnnotationContext<'_>,
    source: &ByteStore,
    entries: &[DictionaryEntry],
) -> AnnotationFlags {
    let Some(value) = entry(entries, source, b"/F") else {
        return AnnotationFlags::default();
    };
    match resolve_number(
        context.document,
        context.index,
        source,
        value,
        context.limits,
    ) {
        Ok(number) if number.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(&number) => {
            AnnotationFlags {
                #[allow(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "an integral value checked to lie within u32"
                )]
                bits: number as u32,
                malformed: false,
            }
        }
        _ => AnnotationFlags {
            bits: 0,
            malformed: true,
        },
    }
}

fn appearance(
    context: &AnnotationContext<'_>,
    source: &ByteStore,
    entries: &[DictionaryEntry],
) -> Appearance {
    let Some(value) = entry(entries, source, b"/AP") else {
        return Appearance::Absent;
    };
    let (ap_source, ap) = match resolve_indirect(
        context.document,
        context.index,
        source,
        value,
        context.limits,
    ) {
        Ok(found) => found,
        Err(error) => return Appearance::Unreadable(error),
    };
    let Some(ap_entries) = dictionary(&ap) else {
        return Appearance::Absent;
    };
    let Some(normal) = entry(ap_entries, &ap_source, b"/N") else {
        return Appearance::Absent;
    };
    let state = || {
        entry(entries, source, b"/AS")
            .and_then(|object| pdf_syntax::decode_name(source, object).ok())
            .map_or_else(|| b"Off".to_vec(), without_solidus)
    };
    match normal.kind() {
        ObjectKind::Reference(reference) => {
            let resolved = match context.index.resolve_object(
                context.document,
                *reference,
                context.limits.resolve,
            ) {
                Ok(resolved) => resolved,
                Err(error) => {
                    return Appearance::Unreadable(PageContentError::new(
                        PageContentErrorKind::Resolve(error),
                    ));
                }
            };
            if resolved.stream().is_some() {
                return stream(context, *reference, normal.span());
            }
            match dictionary(resolved.value()) {
                Some(states) => select_state(context, resolved.source(), states, &state()),
                None => Appearance::Unreadable(PageContentError::new(
                    PageContentErrorKind::AppearanceNotStream,
                )),
            }
        }
        ObjectKind::Dictionary(states) => select_state(context, &ap_source, states, &state()),
        _ => Appearance::Unreadable(PageContentError::new(
            PageContentErrorKind::AppearanceNotStream,
        )),
    }
}

fn select_state(
    context: &AnnotationContext<'_>,
    source: &ByteStore,
    states: &[DictionaryEntry],
    state: &[u8],
) -> Appearance {
    let mut key = Vec::with_capacity(state.len() + 1);
    key.push(b'/');
    key.extend_from_slice(state);
    let Some(value) = entry(states, source, &key) else {
        return Appearance::MissingState {
            state: state.to_vec(),
        };
    };
    let ObjectKind::Reference(reference) = value.kind() else {
        return Appearance::Unreadable(PageContentError::new(
            PageContentErrorKind::AppearanceNotStream,
        ));
    };
    stream(context, *reference, value.span())
}

fn stream(context: &AnnotationContext<'_>, reference: Reference, span: SourceSpan) -> Appearance {
    let mut state = ResourceLoadState {
        forms: HashMap::new(),
        visiting: Vec::new(),
        loaded_forms: 0,
        security: context.security.cloned(),
        optional_content: Arc::clone(&context.optional_content),
    };
    match load_form_stream(
        context.document,
        context.index,
        context.security.map(AsRef::as_ref),
        reference,
        context.limits,
        &mut state,
        false,
    ) {
        Ok(form) => Appearance::Stream { form, span },
        Err(error) => Appearance::Unreadable(error),
    }
}

fn need_appearances(context: &AnnotationContext<'_>) -> bool {
    let Some((source, entries)) = acro_form(context) else {
        return false;
    };
    entry(&entries, &source, b"/NeedAppearances")
        .is_some_and(|value| matches!(value.kind(), ObjectKind::Boolean(true)))
}

fn acro_form(context: &AnnotationContext<'_>) -> Option<(ByteStore, Vec<DictionaryEntry>)> {
    let catalog = context
        .index
        .resolve_object(context.document, context.catalog, context.limits.resolve)
        .ok()?;
    let value = entry(dictionary(catalog.value())?, catalog.source(), b"/AcroForm")?;
    let (source, form) = resolve_indirect(
        context.document,
        context.index,
        catalog.source(),
        value,
        context.limits,
    )
    .ok()?;
    Some((source, dictionary(&form)?.to_vec()))
}

#[derive(Default)]
struct FormMembership {
    nodes: Option<Result<HashSet<Reference>, PageContentError>>,
}

impl FormMembership {
    fn contains(&mut self, context: &AnnotationContext<'_>, reference: Reference) -> bool {
        self.nodes
            .get_or_insert_with(|| field_nodes(context))
            .as_ref()
            .is_ok_and(|nodes| nodes.contains(&reference))
    }
}

fn field_nodes(context: &AnnotationContext<'_>) -> Result<HashSet<Reference>, PageContentError> {
    let mut nodes = HashSet::new();
    let Some((source, form)) = acro_form(context) else {
        return Ok(nodes);
    };
    let Some(fields) = entry(&form, &source, b"/Fields") else {
        return Ok(nodes);
    };
    let mut pending = Vec::new();
    push_references(context, &source, fields, &mut pending)?;
    while let Some(reference) = pending.pop() {
        if !nodes.insert(reference) {
            continue;
        }
        if nodes.len() > MAX_FIELD_NODES {
            return Err(PageContentError::new(PageContentErrorKind::AnnotationLimit));
        }
        let resolved = context
            .index
            .resolve_object(context.document, reference, context.limits.resolve)
            .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
        if let Some(kids) = dictionary(resolved.value())
            .and_then(|entries| entry(entries, resolved.source(), b"/Kids"))
        {
            push_references(context, resolved.source(), kids, &mut pending)?;
        }
    }
    Ok(nodes)
}

fn push_references(
    context: &AnnotationContext<'_>,
    source: &ByteStore,
    value: &Object,
    pending: &mut Vec<Reference>,
) -> Result<(), PageContentError> {
    let (_, array) = resolve_indirect(
        context.document,
        context.index,
        source,
        value,
        context.limits,
    )?;
    if let ObjectKind::Array(items) = array.kind() {
        pending.extend(items.iter().filter_map(|item| match item.kind() {
            ObjectKind::Reference(reference) => Some(*reference),
            _ => None,
        }));
    }
    Ok(())
}

fn has_field_type(
    context: &AnnotationContext<'_>,
    source: &ByteStore,
    entries: &[DictionaryEntry],
) -> bool {
    if entry(entries, source, b"/FT").is_some() {
        return true;
    }
    let mut parent = entry(entries, source, b"/Parent").cloned();
    let mut parent_source = source.clone();
    for _ in 0..MAX_PARENT_DEPTH {
        let Some(value) = parent.take() else {
            return false;
        };
        let Ok((next_source, node)) = resolve_indirect(
            context.document,
            context.index,
            &parent_source,
            &value,
            context.limits,
        ) else {
            return false;
        };
        let Some(node_entries) = dictionary(&node) else {
            return false;
        };
        if entry(node_entries, &next_source, b"/FT").is_some() {
            return true;
        }
        parent = entry(node_entries, &next_source, b"/Parent").cloned();
        parent_source = next_source;
    }
    false
}
