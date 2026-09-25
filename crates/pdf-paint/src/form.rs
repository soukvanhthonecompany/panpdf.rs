use std::borrow::Cow;

use pdf_bytes::{ByteStore, SourceSpan};
use pdf_content::{
    FormXObject, Operation, PageResources, ResourceEntry, StringProtection, Type3Font,
};
use pdf_syntax::{Object, ObjectKind};

use crate::color::ColorSpace;
use crate::color::{
    ColorSpaceParseContext, group_blend_space_supported, parse_color_space_definition,
};
use crate::error::{InterpretError, InterpretErrorKind};
use crate::geometry::{Matrix, normalize_rectangle};
use crate::graph::GroupBackdrop;
use crate::operand::{follow_one_reference, resource_number, unique_resource_entry};
use crate::provenance::Derived;
use crate::state::{PaintLimits, SoftMaskSubtype, SoftMaskTransfer};

pub(crate) fn font_matrix(font: &Type3Font) -> Matrix {
    Matrix {
        a: font.font_matrix[0],
        b: font.font_matrix[1],
        c: font.font_matrix[2],
        d: font.font_matrix[3],
        e: font.font_matrix[4],
        f: font.font_matrix[5],
    }
}

pub(crate) fn form_matrix(
    form: &FormXObject,
    at: SourceSpan,
) -> Result<(Matrix, Option<SourceSpan>), InterpretError> {
    let Some(value) = form_entry(form, b"/Matrix") else {
        return Ok((Matrix::IDENTITY, None));
    };
    let values = form_numbers::<6>(form, value, at)?;
    Ok((
        Matrix {
            a: values[0],
            b: values[1],
            c: values[2],
            d: values[3],
            e: values[4],
            f: values[5],
        },
        Some(value.span()),
    ))
}

pub(crate) fn form_bbox(
    form: &FormXObject,
    at: SourceSpan,
) -> Result<([f64; 4], SourceSpan), InterpretError> {
    form_bbox_if_present(form, at)?
        .ok_or_else(|| InterpretError::at_span(at, InterpretErrorKind::FormMissingBBox))
}

pub(crate) fn form_bbox_if_present(
    form: &FormXObject,
    at: SourceSpan,
) -> Result<Option<([f64; 4], SourceSpan)>, InterpretError> {
    let Some(value) = form_entry(form, b"/BBox") else {
        return Ok(None);
    };
    let bounds = normalize_rectangle(form_numbers::<4>(form, value, at)?);
    Ok(Some((bounds, value.span())))
}

pub(crate) fn form_entry<'a>(form: &'a FormXObject, key: &[u8]) -> Option<&'a Object> {
    let ObjectKind::Dictionary(entries) = form.dictionary.kind() else {
        return None;
    };
    entries
        .iter()
        .find(|entry| entry.key_equals(&form.source, key))
        .map(pdf_syntax::DictionaryEntry::value)
}

fn form_numbers<const N: usize>(
    form: &FormXObject,
    value: &Object,
    at: SourceSpan,
) -> Result<[f64; N], InterpretError> {
    let (source, value) = match value.kind() {
        ObjectKind::Reference(reference) => {
            let (source, value) = form
                .resolve_object(*reference)
                .map_err(|_| InterpretError::at_span(at, InterpretErrorKind::InvalidFormEntry))?;
            (Cow::Owned(source), Cow::Owned(value))
        }
        _ => (Cow::Borrowed(&form.source), Cow::Borrowed(value)),
    };
    let ObjectKind::Array(entries) = value.kind() else {
        return Err(InterpretError::at_span(
            at,
            InterpretErrorKind::InvalidFormEntry,
        ));
    };
    let entries: &[Object; N] = entries
        .as_slice()
        .try_into()
        .map_err(|_| InterpretError::at_span(at, InterpretErrorKind::InvalidFormEntry))?;
    let mut values = [0.0; N];
    for (slot, object) in values.iter_mut().zip(entries) {
        if !matches!(object.kind(), ObjectKind::Number(_)) {
            return Err(InterpretError::at_span(
                at,
                InterpretErrorKind::InvalidFormEntry,
            ));
        }
        let bytes = source
            .resolve(object.span())
            .map_err(|_| InterpretError::at_span(at, InterpretErrorKind::ResourceSourceFailure))?;
        let text = std::str::from_utf8(bytes)
            .map_err(|_| InterpretError::at_span(at, InterpretErrorKind::InvalidNumber))?;
        *slot = text
            .parse::<f64>()
            .map_err(|_| InterpretError::at_span(at, InterpretErrorKind::InvalidNumber))?;
        if !slot.is_finite() {
            return Err(InterpretError::at_span(
                at,
                InterpretErrorKind::InvalidNumber,
            ));
        }
    }
    Ok(values)
}

pub(crate) struct TransparencyGroupMetadata {
    pub(crate) group_span: SourceSpan,
    pub(crate) group_reference_span: Option<SourceSpan>,
    pub(crate) subtype_span: SourceSpan,
    pub(crate) bbox: [f64; 4],
    pub(crate) bbox_span: SourceSpan,
    pub(crate) matrix: Derived<Matrix>,
    pub(crate) isolated: Derived<bool>,
    pub(crate) knockout: Derived<bool>,
    pub(crate) blend_space: Option<Derived<ColorSpace>>,
    pub(crate) backdrop: Derived<GroupBackdrop>,
}

pub(crate) fn transparency_group_metadata(
    form: &FormXObject,
    invocation: &Operation,
    invoking_resources: Option<&PageResources>,
    limits: PaintLimits,
) -> Result<TransparencyGroupMetadata, InterpretError> {
    let group_entry = unique_form_entry(form, b"/Group", invocation)?
        .ok_or_else(|| InterpretError::at(invocation, InterpretErrorKind::GroupMissingEntry))?;
    let (group_source, group, group_strings, group_reference_span) = match group_entry.kind() {
        ObjectKind::Reference(reference) => {
            let (source, value, strings) =
                form.resolve_protected_object(*reference).map_err(|error| {
                    InterpretError::at(invocation, InterpretErrorKind::GroupResource(error.kind()))
                })?;
            (source, value, strings, Some(group_entry.span()))
        }
        _ => (
            form.source.clone(),
            group_entry.clone(),
            form.strings(),
            None,
        ),
    };
    let ObjectKind::Dictionary(entries) = group.kind() else {
        return Err(InterpretError::at(
            invocation,
            InterpretErrorKind::InvalidGroupEntry,
        ));
    };
    if let Some(group_type) = unique_group_entry(entries, &group_source, b"/Type", invocation)?
        && !group_type.name_equals(&group_source, b"/Group")
    {
        return Err(InterpretError::at(
            invocation,
            InterpretErrorKind::InvalidGroupEntry,
        ));
    }
    let subtype = unique_group_entry(entries, &group_source, b"/S", invocation)?
        .ok_or_else(|| InterpretError::at(invocation, InterpretErrorKind::GroupMissingEntry))?;
    if !subtype.name_equals(&group_source, b"/Transparency") {
        return Err(InterpretError::at(
            invocation,
            InterpretErrorKind::UnsupportedGroupSubtype,
        ));
    }

    let isolated =
        if let Some(value) = unique_group_entry(entries, &group_source, b"/I", invocation)? {
            Derived::assigned(group_boolean(value, invocation)?, value.span())
        } else {
            Derived::initial(false)
        };
    let knockout =
        if let Some(value) = unique_group_entry(entries, &group_source, b"/K", invocation)? {
            Derived::assigned(group_boolean(value, invocation)?, value.span())
        } else {
            Derived::initial(false)
        };
    let effective_resources = form.resources.as_ref().or(invoking_resources);
    let blend_space = unique_group_entry(entries, &group_source, b"/CS", invocation)?
        .map(|value| {
            group_color_space(
                form,
                &group_source,
                group_strings,
                value,
                invocation,
                effective_resources,
                limits,
            )
        })
        .transpose()?;
    let backdrop_value = if isolated.value {
        GroupBackdrop::Transparent
    } else {
        GroupBackdrop::Inherited
    };
    let backdrop = Derived {
        value: backdrop_value,
        provenance: isolated.provenance.clone(),
    };
    let (bbox, bbox_span) = form_bbox(form, invocation.operator_span())?;
    let (matrix, matrix_span) = form_matrix(form, invocation.operator_span())?;
    let matrix = matrix_span.map_or_else(
        || Derived::initial(matrix),
        |span| Derived::assigned(matrix, span),
    );
    Ok(TransparencyGroupMetadata {
        group_span: group.span(),
        group_reference_span,
        subtype_span: subtype.span(),
        bbox,
        bbox_span,
        matrix,
        isolated,
        knockout,
        blend_space,
        backdrop,
    })
}

fn unique_form_entry<'a>(
    form: &'a FormXObject,
    key: &[u8],
    invocation: &Operation,
) -> Result<Option<&'a Object>, InterpretError> {
    let ObjectKind::Dictionary(entries) = form.dictionary.kind() else {
        return Err(InterpretError::at(
            invocation,
            InterpretErrorKind::InvalidFormEntry,
        ));
    };
    unique_dictionary_entry(entries, &form.source, key, invocation)
}

fn unique_group_entry<'a>(
    entries: &'a [pdf_syntax::DictionaryEntry],
    source: &ByteStore,
    key: &[u8],
    invocation: &Operation,
) -> Result<Option<&'a Object>, InterpretError> {
    unique_dictionary_entry(entries, source, key, invocation)
}

fn unique_dictionary_entry<'a>(
    entries: &'a [pdf_syntax::DictionaryEntry],
    source: &ByteStore,
    key: &[u8],
    invocation: &Operation,
) -> Result<Option<&'a Object>, InterpretError> {
    let mut matches = entries
        .iter()
        .filter(|entry| entry.key_equals(source, key))
        .map(pdf_syntax::DictionaryEntry::value);
    let first = matches.next();
    if matches.next().is_some() {
        return Err(InterpretError::at(
            invocation,
            InterpretErrorKind::InvalidGroupEntry,
        ));
    }
    Ok(first)
}

fn group_boolean(value: &Object, invocation: &Operation) -> Result<bool, InterpretError> {
    match value.kind() {
        ObjectKind::Boolean(true) => Ok(true),
        ObjectKind::Boolean(false) => Ok(false),
        _ => Err(InterpretError::at(
            invocation,
            InterpretErrorKind::InvalidGroupEntry,
        )),
    }
}

fn group_color_space(
    form: &FormXObject,
    source: &ByteStore,
    strings: StringProtection,
    value: &Object,
    invocation: &Operation,
    resources: Option<&PageResources>,
    limits: PaintLimits,
) -> Result<Derived<ColorSpace>, InterpretError> {
    let load_icc = |reference, limit| form.icc_profile(reference, limit);
    let load_indexed = |reference, limit| form.indexed_lookup(reference, limit);
    let load_function = |reference, limit| form.function(reference, limit);
    let load_object = |reference| form.resolve_protected_object(reference);
    let string_plaintext = |strings, bytes| form.string_plaintext(strings, bytes);
    let context = ColorSpaceParseContext {
        load_function: &load_function,
        load_object: &load_object,
        string_plaintext: &string_plaintext,
        resources,
        load_icc: &load_icc,
        load_indexed: &load_indexed,
        limits,
    };
    let space = parse_color_space_definition(
        invocation,
        source,
        strings,
        value,
        InterpretErrorKind::UnsupportedGroupColorSpace,
        &context,
        0,
    )?;
    if !group_blend_space_supported(&space) {
        return Err(InterpretError::at(
            invocation,
            InterpretErrorKind::UnsupportedGroupColorSpace,
        ));
    }
    Ok(Derived::assigned(space, value.span()))
}

pub(crate) fn resolve_soft_mask_dictionary(
    operation: &Operation,
    resource: &ResourceEntry,
    value: &Object,
) -> Result<(ByteStore, Object, Option<SourceSpan>), InterpretError> {
    let ObjectKind::Reference(reference) = value.kind() else {
        return Ok((resource.source().clone(), value.clone(), None));
    };
    let (source, resolved) = resource.resolve_object(*reference).map_err(|error| {
        InterpretError::at(
            operation,
            InterpretErrorKind::SoftMaskResource(error.kind()),
        )
    })?;
    Ok((source, resolved, Some(value.span())))
}

pub(crate) fn soft_mask_subtype(
    entries: &[pdf_syntax::DictionaryEntry],
    source: &ByteStore,
    operation: &Operation,
) -> Result<Derived<SoftMaskSubtype>, InterpretError> {
    let value = unique_resource_entry(
        entries,
        source,
        b"/S",
        operation,
        InterpretErrorKind::InvalidSoftMaskEntry,
    )?
    .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::SoftMaskMissingEntry))?;
    let subtype = if value.name_equals(source, b"/Alpha") {
        SoftMaskSubtype::Alpha
    } else if value.name_equals(source, b"/Luminosity") {
        SoftMaskSubtype::Luminosity
    } else {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidSoftMaskEntry,
        ));
    };
    Ok(Derived::assigned(subtype, value.span()))
}

pub(crate) fn validate_soft_mask_dictionary(
    entries: &[pdf_syntax::DictionaryEntry],
    source: &ByteStore,
    operation: &Operation,
) -> Result<(), InterpretError> {
    let mut seen = std::collections::HashSet::new();
    for entry in entries {
        let key = entry.decoded_key(source).map_err(|_| {
            InterpretError::at(operation, InterpretErrorKind::ResourceSourceFailure)
        })?;
        if !seen.insert(key.clone()) {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidSoftMaskEntry,
            ));
        }
        if !matches!(key.as_slice(), b"/Type" | b"/S" | b"/G" | b"/BC" | b"/TR") {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::UnsupportedSoftMaskEntry,
            ));
        }
    }
    if let Some(mask_type) = unique_resource_entry(
        entries,
        source,
        b"/Type",
        operation,
        InterpretErrorKind::InvalidSoftMaskEntry,
    )? && !mask_type.name_equals(source, b"/Mask")
    {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidSoftMaskEntry,
        ));
    }
    Ok(())
}

pub(crate) fn soft_mask_transfer(
    entries: &[pdf_syntax::DictionaryEntry],
    source: &ByteStore,
    operation: &Operation,
) -> Result<Derived<SoftMaskTransfer>, InterpretError> {
    let Some(transfer) = unique_resource_entry(
        entries,
        source,
        b"/TR",
        operation,
        InterpretErrorKind::InvalidSoftMaskEntry,
    )?
    else {
        return Ok(Derived::initial(SoftMaskTransfer::Identity));
    };
    if !transfer.name_equals(source, b"/Identity") {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::UnsupportedSoftMaskTransfer,
        ));
    }
    Ok(Derived::assigned(
        SoftMaskTransfer::Identity,
        transfer.span(),
    ))
}

pub(crate) fn soft_mask_backdrop_color(
    operation: &Operation,
    resource: &ResourceEntry,
    mask_source: &ByteStore,
    entries: &[pdf_syntax::DictionaryEntry],
    metadata: &TransparencyGroupMetadata,
) -> Result<Option<Derived<Vec<f64>>>, InterpretError> {
    let Some(backdrop) = unique_resource_entry(
        entries,
        mask_source,
        b"/BC",
        operation,
        InterpretErrorKind::InvalidSoftMaskEntry,
    )?
    else {
        return Ok(None);
    };
    let load = |reference| resource.resolve_object(reference);
    let (backdrop_source, backdrop) = follow_one_reference(
        operation,
        mask_source,
        backdrop,
        &load,
        InterpretErrorKind::SoftMaskResource,
        InterpretErrorKind::InvalidSoftMaskEntry,
    )?;
    soft_mask_backdrop(operation, &backdrop_source, &backdrop, metadata).map(Some)
}

fn soft_mask_backdrop(
    operation: &Operation,
    source: &ByteStore,
    value: &Object,
    group: &TransparencyGroupMetadata,
) -> Result<Derived<Vec<f64>>, InterpretError> {
    let expected = match group.blend_space.as_ref().map(|space| &space.value) {
        Some(ColorSpace::DeviceGray | ColorSpace::CalGray(_)) => 1,
        Some(ColorSpace::DeviceRgb | ColorSpace::CalRgb(_) | ColorSpace::Lab(_)) => 3,
        Some(ColorSpace::DeviceCmyk) => 4,
        Some(ColorSpace::IccBased(space)) => space.components.value,
        Some(
            ColorSpace::Indexed(_)
            | ColorSpace::Separation(_)
            | ColorSpace::DeviceN(_)
            | ColorSpace::Pattern(_),
        )
        | None => {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidSoftMaskEntry,
            ));
        }
    };
    let ObjectKind::Array(entries) = value.kind() else {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidSoftMaskEntry,
        ));
    };
    if entries.len() != expected {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidSoftMaskEntry,
        ));
    }
    let mut components = Vec::with_capacity(entries.len());
    for entry in entries {
        components.push(resource_number(operation, source, entry)?);
    }
    Ok(Derived::assigned(components, value.span()))
}
