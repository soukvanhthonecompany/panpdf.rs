use std::borrow::Cow;

use pdf_bytes::ByteStore;
use pdf_content::{Operation, PageContentError, PageContentErrorKind, ResourceEntry};
use pdf_syntax::{NumberKind, Object, ObjectKind, Reference};

use crate::error::{InterpretError, InterpretErrorKind};
use crate::provenance::Derived;

pub(crate) type ObjectLoader<'a> =
    dyn Fn(Reference) -> Result<(ByteStore, Object), PageContentError> + 'a;

pub(crate) fn follow_one_reference<'a>(
    operation: &Operation,
    source: &'a ByteStore,
    value: &'a Object,
    load: &ObjectLoader<'_>,
    unresolved: fn(PageContentErrorKind) -> InterpretErrorKind,
    invalid: InterpretErrorKind,
) -> Result<(Cow<'a, ByteStore>, Cow<'a, Object>), InterpretError> {
    let ObjectKind::Reference(reference) = value.kind() else {
        return Ok((Cow::Borrowed(source), Cow::Borrowed(value)));
    };
    let (resolved_source, resolved) = load(*reference)
        .map_err(|error| InterpretError::at(operation, unresolved(error.kind())))?;
    if matches!(resolved.kind(), ObjectKind::Reference(_)) {
        return Err(InterpretError::at(operation, invalid));
    }
    Ok((Cow::Owned(resolved_source), Cow::Owned(resolved)))
}

pub(crate) fn source_numbers<const N: usize>(
    operation: &Operation,
    source: &ByteStore,
    value: &Object,
) -> Result<[f64; N], InterpretError> {
    let ObjectKind::Array(entries) = value.kind() else {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidColorSpaceEntry,
        ));
    };
    let entries: &[Object; N] = entries
        .as_slice()
        .try_into()
        .map_err(|_| InterpretError::at(operation, InterpretErrorKind::InvalidColorSpaceEntry))?;
    let mut numbers = [0.0; N];
    for (number, entry) in numbers.iter_mut().zip(entries) {
        *number = source_number(operation, source, entry)?;
    }
    Ok(numbers)
}

pub(crate) fn source_number_vector(
    operation: &Operation,
    source: &ByteStore,
    value: &Object,
) -> Result<Vec<f64>, InterpretError> {
    let ObjectKind::Array(entries) = value.kind() else {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidIccProfileEntry,
        ));
    };
    entries
        .iter()
        .map(|entry| source_number(operation, source, entry))
        .collect()
}

pub(crate) fn source_exact_integer(
    operation: &Operation,
    source: &ByteStore,
    value: &Object,
    error: InterpretErrorKind,
) -> Result<i64, InterpretError> {
    if !matches!(value.kind(), ObjectKind::Number(NumberKind::Integer)) {
        return Err(InterpretError::at(operation, error));
    }
    let bytes = source
        .resolve(value.span())
        .map_err(|_| InterpretError::at(operation, InterpretErrorKind::ResourceSourceFailure))?;
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.parse::<i64>().ok())
        .ok_or_else(|| InterpretError::at(operation, error))
}

pub(crate) fn source_number(
    operation: &Operation,
    source: &ByteStore,
    value: &Object,
) -> Result<f64, InterpretError> {
    if !matches!(value.kind(), ObjectKind::Number(_)) {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidColorSpaceEntry,
        ));
    }
    let bytes = source
        .resolve(value.span())
        .map_err(|_| InterpretError::at(operation, InterpretErrorKind::ResourceSourceFailure))?;
    let number = std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.parse::<f64>().ok())
        .filter(|number| number.is_finite())
        .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::InvalidColorSpaceEntry))?;
    Ok(number)
}

pub(crate) fn resource_numbers<const N: usize>(
    operation: &Operation,
    source: &ByteStore,
    value: &Object,
    error: InterpretErrorKind,
) -> Result<[f64; N], InterpretError> {
    let ObjectKind::Array(entries) = value.kind() else {
        return Err(InterpretError::at(operation, error));
    };
    let entries: &[Object; N] = entries
        .as_slice()
        .try_into()
        .map_err(|_| InterpretError::at(operation, error))?;
    let mut values = [0.0; N];
    for (slot, entry) in values.iter_mut().zip(entries) {
        *slot = resource_number(operation, source, entry)?;
    }
    Ok(values)
}

pub(crate) fn resource_number_vector(
    operation: &Operation,
    source: &ByteStore,
    value: &Object,
    error: InterpretErrorKind,
) -> Result<Vec<f64>, InterpretError> {
    let ObjectKind::Array(entries) = value.kind() else {
        return Err(InterpretError::at(operation, error));
    };
    entries
        .iter()
        .map(|entry| resource_number(operation, source, entry))
        .collect()
}

pub(crate) fn optional_number_pair(
    entries: &[pdf_syntax::DictionaryEntry],
    resource: &ResourceEntry,
    key: &[u8],
    operation: &Operation,
    default: [f64; 2],
) -> Result<Derived<[f64; 2]>, InterpretError> {
    let Some(value) = unique_resource_entry(
        entries,
        resource.source(),
        key,
        operation,
        InterpretErrorKind::InvalidShadingEntry,
    )?
    else {
        return Ok(Derived::initial(default));
    };
    Ok(Derived::assigned(
        resource_numbers::<2>(
            operation,
            resource.source(),
            value,
            InterpretErrorKind::InvalidShadingEntry,
        )?,
        value.span(),
    ))
}

pub(crate) fn optional_boolean_pair(
    entries: &[pdf_syntax::DictionaryEntry],
    resource: &ResourceEntry,
    key: &[u8],
    operation: &Operation,
) -> Result<Derived<[bool; 2]>, InterpretError> {
    let Some(value) = unique_resource_entry(
        entries,
        resource.source(),
        key,
        operation,
        InterpretErrorKind::InvalidShadingEntry,
    )?
    else {
        return Ok(Derived::initial([false, false]));
    };
    let ObjectKind::Array(values) = value.kind() else {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidShadingEntry,
        ));
    };
    let [first, second] = values.as_slice() else {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidShadingEntry,
        ));
    };
    Ok(Derived::assigned(
        [
            resource_boolean(operation, first)?,
            resource_boolean(operation, second)?,
        ],
        value.span(),
    ))
}

pub(crate) fn optional_component_array(
    entries: &[pdf_syntax::DictionaryEntry],
    resource: &ResourceEntry,
    key: &[u8],
    operation: &Operation,
    components: usize,
) -> Result<Option<Derived<Vec<f64>>>, InterpretError> {
    let Some(value) = unique_resource_entry(
        entries,
        resource.source(),
        key,
        operation,
        InterpretErrorKind::InvalidShadingEntry,
    )?
    else {
        return Ok(None);
    };
    let values = resource_number_vector(
        operation,
        resource.source(),
        value,
        InterpretErrorKind::InvalidShadingEntry,
    )?;
    if values.len() != components {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidShadingEntry,
        ));
    }
    Ok(Some(Derived::assigned(values, value.span())))
}

pub(crate) fn resource_number(
    operation: &Operation,
    source: &ByteStore,
    value: &Object,
) -> Result<f64, InterpretError> {
    if !matches!(value.kind(), ObjectKind::Number(_)) {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidExtGStateEntry,
        ));
    }
    let bytes = source
        .resolve(value.span())
        .map_err(|_| InterpretError::at(operation, InterpretErrorKind::ResourceSourceFailure))?;
    let text = std::str::from_utf8(bytes)
        .map_err(|_| InterpretError::at(operation, InterpretErrorKind::InvalidNumber))?;
    let number = text
        .parse::<f64>()
        .map_err(|_| InterpretError::at(operation, InterpretErrorKind::InvalidNumber))?;
    if !number.is_finite() {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidNumber,
        ));
    }
    Ok(number)
}

pub(crate) fn resource_integer(
    operation: &Operation,
    source: &ByteStore,
    value: &Object,
) -> Result<i64, InterpretError> {
    if !matches!(value.kind(), ObjectKind::Number(NumberKind::Integer)) {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidExtGStateEntry,
        ));
    }
    let bytes = source
        .resolve(value.span())
        .map_err(|_| InterpretError::at(operation, InterpretErrorKind::ResourceSourceFailure))?;
    let text = std::str::from_utf8(bytes)
        .map_err(|_| InterpretError::at(operation, InterpretErrorKind::InvalidNumber))?;
    text.parse::<i64>()
        .map_err(|_| InterpretError::at(operation, InterpretErrorKind::InvalidNumber))
}

pub(crate) fn resource_boolean(
    operation: &Operation,
    value: &Object,
) -> Result<bool, InterpretError> {
    let ObjectKind::Boolean(enabled) = value.kind() else {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidExtGStateEntry,
        ));
    };
    Ok(*enabled)
}

pub(crate) fn unique_resource_entry<'a>(
    entries: &'a [pdf_syntax::DictionaryEntry],
    source: &ByteStore,
    key: &[u8],
    operation: &Operation,
    error: InterpretErrorKind,
) -> Result<Option<&'a Object>, InterpretError> {
    let mut matches = entries
        .iter()
        .filter(|entry| entry.key_equals(source, key))
        .map(pdf_syntax::DictionaryEntry::value);
    let first = matches.next();
    if matches.next().is_some() {
        return Err(InterpretError::at(operation, error));
    }
    Ok(first)
}
