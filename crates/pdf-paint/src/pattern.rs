use pdf_bytes::SourceSpan;
use pdf_content::{Operation, ShadingPattern, TilingPattern};
use pdf_syntax::{NumberKind, Object, ObjectKind};

use crate::error::{InterpretError, InterpretErrorKind};
use crate::geometry::{Matrix, normalize_rectangle};
use crate::provenance::Derived;

pub(crate) struct PatternMetadata {
    pub(crate) paint_type: Derived<i64>,
    pub(crate) bbox: [f64; 4],
    pub(crate) bbox_span: SourceSpan,
    pub(crate) x_step: Derived<f64>,
    pub(crate) y_step: Derived<f64>,
    pub(crate) matrix: Derived<Matrix>,
    pub(crate) tiling_type: Derived<i64>,
}

pub(crate) fn pattern_metadata(
    pattern: &TilingPattern,
    invocation: &Operation,
) -> Result<PatternMetadata, InterpretError> {
    let paint_type_object = pattern_entry(pattern, b"/PaintType", invocation)?
        .ok_or_else(|| InterpretError::at(invocation, InterpretErrorKind::PatternMissingEntry))?;
    let paint_type = pattern_integer(pattern, paint_type_object, invocation)?;
    if !(1..=2).contains(&paint_type) {
        return Err(InterpretError::at(
            invocation,
            InterpretErrorKind::InvalidPatternEntry,
        ));
    }

    let tiling_type_object = pattern_entry(pattern, b"/TilingType", invocation)?
        .ok_or_else(|| InterpretError::at(invocation, InterpretErrorKind::PatternMissingEntry))?;
    let tiling_type = pattern_integer(pattern, tiling_type_object, invocation)?;
    if !(1..=3).contains(&tiling_type) {
        return Err(InterpretError::at(
            invocation,
            InterpretErrorKind::InvalidPatternEntry,
        ));
    }

    let bbox_object = pattern_entry(pattern, b"/BBox", invocation)?
        .ok_or_else(|| InterpretError::at(invocation, InterpretErrorKind::PatternMissingEntry))?;
    let bbox = normalize_rectangle(pattern_numbers::<4>(pattern, bbox_object, invocation)?);

    let x_step_object = pattern_entry(pattern, b"/XStep", invocation)?
        .ok_or_else(|| InterpretError::at(invocation, InterpretErrorKind::PatternMissingEntry))?;
    let x_step = pattern_number(pattern, x_step_object, invocation)?;
    let y_step_object = pattern_entry(pattern, b"/YStep", invocation)?
        .ok_or_else(|| InterpretError::at(invocation, InterpretErrorKind::PatternMissingEntry))?;
    let y_step = pattern_number(pattern, y_step_object, invocation)?;
    if x_step == 0.0 || y_step == 0.0 {
        return Err(InterpretError::at(
            invocation,
            InterpretErrorKind::InvalidPatternEntry,
        ));
    }

    let matrix = if let Some(matrix_object) = pattern_entry(pattern, b"/Matrix", invocation)? {
        let values = pattern_numbers::<6>(pattern, matrix_object, invocation)?;
        Derived::assigned(
            Matrix {
                a: values[0],
                b: values[1],
                c: values[2],
                d: values[3],
                e: values[4],
                f: values[5],
            },
            matrix_object.span(),
        )
    } else {
        Derived::initial(Matrix::IDENTITY)
    };

    Ok(PatternMetadata {
        paint_type: Derived::assigned(paint_type, paint_type_object.span()),
        bbox,
        bbox_span: bbox_object.span(),
        x_step: Derived::assigned(x_step, x_step_object.span()),
        y_step: Derived::assigned(y_step, y_step_object.span()),
        matrix,
        tiling_type: Derived::assigned(tiling_type, tiling_type_object.span()),
    })
}

fn pattern_entry<'a>(
    pattern: &'a TilingPattern,
    key: &[u8],
    invocation: &Operation,
) -> Result<Option<&'a Object>, InterpretError> {
    let ObjectKind::Dictionary(entries) = pattern.dictionary.kind() else {
        return Err(InterpretError::at(
            invocation,
            InterpretErrorKind::InvalidPatternEntry,
        ));
    };
    let mut matches = entries
        .iter()
        .filter(|entry| entry.key_equals(&pattern.source, key))
        .map(pdf_syntax::DictionaryEntry::value);
    let first = matches.next();
    if matches.next().is_some() {
        return Err(InterpretError::at(
            invocation,
            InterpretErrorKind::InvalidPatternEntry,
        ));
    }
    Ok(first)
}

fn pattern_numbers<const N: usize>(
    pattern: &TilingPattern,
    value: &Object,
    invocation: &Operation,
) -> Result<[f64; N], InterpretError> {
    let ObjectKind::Array(entries) = value.kind() else {
        return Err(InterpretError::at(
            invocation,
            InterpretErrorKind::InvalidPatternEntry,
        ));
    };
    let entries: &[Object; N] = entries
        .as_slice()
        .try_into()
        .map_err(|_| InterpretError::at(invocation, InterpretErrorKind::InvalidPatternEntry))?;
    let mut values = [0.0; N];
    for (slot, object) in values.iter_mut().zip(entries) {
        *slot = pattern_number(pattern, object, invocation)?;
    }
    Ok(values)
}

fn pattern_number(
    pattern: &TilingPattern,
    object: &Object,
    invocation: &Operation,
) -> Result<f64, InterpretError> {
    if !matches!(object.kind(), ObjectKind::Number(_)) {
        return Err(InterpretError::at(
            invocation,
            InterpretErrorKind::InvalidPatternEntry,
        ));
    }
    let bytes = pattern
        .source
        .resolve(object.span())
        .map_err(|_| InterpretError::at(invocation, InterpretErrorKind::ResourceSourceFailure))?;
    let value = std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .ok_or_else(|| InterpretError::at(invocation, InterpretErrorKind::InvalidPatternEntry))?;
    Ok(value)
}

fn pattern_integer(
    pattern: &TilingPattern,
    object: &Object,
    invocation: &Operation,
) -> Result<i64, InterpretError> {
    if !matches!(object.kind(), ObjectKind::Number(NumberKind::Integer)) {
        return Err(InterpretError::at(
            invocation,
            InterpretErrorKind::InvalidPatternEntry,
        ));
    }
    let bytes = pattern
        .source
        .resolve(object.span())
        .map_err(|_| InterpretError::at(invocation, InterpretErrorKind::ResourceSourceFailure))?;
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.parse::<i64>().ok())
        .ok_or_else(|| InterpretError::at(invocation, InterpretErrorKind::InvalidPatternEntry))
}

pub(crate) fn shading_pattern_matrix(
    pattern: &ShadingPattern,
    invocation: &Operation,
) -> Result<Derived<Matrix>, InterpretError> {
    let ObjectKind::Dictionary(entries) = pattern.dictionary.kind() else {
        return Err(InterpretError::at(
            invocation,
            InterpretErrorKind::InvalidPatternEntry,
        ));
    };
    let mut matches = entries
        .iter()
        .filter(|entry| entry.key_equals(&pattern.source, b"/Matrix"))
        .map(pdf_syntax::DictionaryEntry::value);
    let Some(object) = matches.next() else {
        return Ok(Derived::initial(Matrix::IDENTITY));
    };
    if matches.next().is_some() {
        return Err(InterpretError::at(
            invocation,
            InterpretErrorKind::InvalidPatternEntry,
        ));
    }
    let ObjectKind::Array(items) = object.kind() else {
        return Err(InterpretError::at(
            invocation,
            InterpretErrorKind::InvalidPatternEntry,
        ));
    };
    let items: &[pdf_syntax::Object; 6] = items
        .as_slice()
        .try_into()
        .map_err(|_| InterpretError::at(invocation, InterpretErrorKind::InvalidPatternEntry))?;
    let mut values = [0.0; 6];
    for (slot, item) in values.iter_mut().zip(items) {
        *slot = shading_pattern_number(pattern, item, invocation)?;
    }
    Ok(Derived::assigned(
        Matrix {
            a: values[0],
            b: values[1],
            c: values[2],
            d: values[3],
            e: values[4],
            f: values[5],
        },
        object.span(),
    ))
}

fn shading_pattern_number(
    pattern: &ShadingPattern,
    object: &Object,
    invocation: &Operation,
) -> Result<f64, InterpretError> {
    if !matches!(object.kind(), ObjectKind::Number(_)) {
        return Err(InterpretError::at(
            invocation,
            InterpretErrorKind::InvalidPatternEntry,
        ));
    }
    let bytes = pattern
        .source
        .resolve(object.span())
        .map_err(|_| InterpretError::at(invocation, InterpretErrorKind::ResourceSourceFailure))?;
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .ok_or_else(|| InterpretError::at(invocation, InterpretErrorKind::InvalidPatternEntry))
}
