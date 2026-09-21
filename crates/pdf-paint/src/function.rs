use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceSpan};
use pdf_content::{FunctionData, FunctionObject, Operation, PageContentError};
use pdf_syntax::{Object, ObjectKind, Reference};

use crate::error::{InterpretError, InterpretErrorKind};
use crate::operand::{
    resource_integer, resource_number, resource_number_vector, resource_numbers,
    unique_resource_entry,
};
use crate::postscript;
use crate::provenance::Derived;
use crate::state::PaintLimits;

#[derive(Clone, Debug, PartialEq)]
pub struct ExponentialFunction {
    pub dictionary_span: SourceSpan,
    pub domain: Derived<[f64; 2]>,
    pub range: Option<Derived<Vec<f64>>>,
    pub c0: Derived<Vec<f64>>,
    pub c1: Derived<Vec<f64>>,
    pub exponent: Derived<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StitchingFunction {
    pub dictionary_span: SourceSpan,
    pub domain: Derived<[f64; 2]>,
    pub range: Option<Derived<Vec<f64>>>,
    pub functions: Vec<Function>,
    pub bounds: Derived<Vec<f64>>,
    pub encode: Derived<Vec<f64>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SampledFunction {
    pub dictionary_span: SourceSpan,
    pub reference: Option<Reference>,
    pub encoded_data_span: SourceSpan,
    pub domain: Derived<Vec<f64>>,
    pub range: Derived<Vec<f64>>,
    pub size: Derived<Vec<usize>>,
    pub bits_per_sample: Derived<u32>,
    pub encode: Derived<Vec<f64>>,
    pub decode: Derived<Vec<f64>>,
    pub samples: Arc<[u8]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PostScriptFunction {
    pub dictionary_span: SourceSpan,
    pub reference: Option<Reference>,
    pub encoded_data_span: SourceSpan,
    pub domain: Derived<Vec<f64>>,
    pub range: Derived<Vec<f64>>,
    pub program: Vec<postscript::Node>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Function {
    Sampled(Box<SampledFunction>),
    Exponential(Box<ExponentialFunction>),
    Stitching(Box<StitchingFunction>),
    PostScript(Box<PostScriptFunction>),
}

impl Function {
    #[must_use]
    pub fn inputs(&self) -> usize {
        match self {
            Self::Sampled(function) => function.domain.value.len() / 2,
            Self::PostScript(function) => function.domain.value.len() / 2,
            Self::Exponential(_) | Self::Stitching(_) => 1,
        }
    }

    #[must_use]
    pub fn outputs(&self) -> usize {
        match self {
            Self::Sampled(function) => function.range.value.len() / 2,
            Self::PostScript(function) => function.range.value.len() / 2,
            Self::Exponential(function) => function.c0.value.len(),
            Self::Stitching(function) => function.range.as_ref().map_or_else(
                || function.functions.first().map_or(0, Self::outputs),
                |range| range.value.len() / 2,
            ),
        }
    }

    #[must_use]
    pub fn input_domain(&self, axis: usize) -> Option<[f64; 2]> {
        match self {
            Self::Sampled(function) => function
                .domain
                .value
                .get(axis * 2..axis * 2 + 2)
                .map(|pair| [pair[0], pair[1]]),
            Self::PostScript(function) => function
                .domain
                .value
                .get(axis * 2..axis * 2 + 2)
                .map(|pair| [pair[0], pair[1]]),
            Self::Exponential(function) => (axis == 0).then_some(function.domain.value),
            Self::Stitching(function) => (axis == 0).then_some(function.domain.value),
        }
    }

    #[must_use]
    pub fn evaluate(&self, inputs: &[f64]) -> Option<Vec<f64>> {
        if inputs.len() != self.inputs() {
            return None;
        }
        match self {
            Self::Sampled(function) => function.evaluate(inputs),
            Self::PostScript(function) => function.evaluate(inputs),
            Self::Exponential(function) => Some(function.evaluate(inputs[0])),
            Self::Stitching(function) => function.evaluate(inputs[0]),
        }
    }
}

impl PostScriptFunction {
    pub(crate) fn evaluate(&self, inputs: &[f64]) -> Option<Vec<f64>> {
        let clipped: Vec<f64> = inputs
            .iter()
            .enumerate()
            .map(|(axis, value)| {
                let low = self.domain.value.get(axis * 2).copied().unwrap_or(0.0);
                let high = self.domain.value.get(axis * 2 + 1).copied().unwrap_or(1.0);
                value.clamp(low, high)
            })
            .collect();
        let outputs = self.range.value.len() / 2;
        let produced = postscript::evaluate(&self.program, &clipped, outputs)?;
        Some(
            produced
                .into_iter()
                .enumerate()
                .map(|(component, value)| {
                    value.clamp(
                        self.range.value[component * 2],
                        self.range.value[component * 2 + 1],
                    )
                })
                .collect(),
        )
    }
}

impl ExponentialFunction {
    pub(crate) fn evaluate(&self, input: f64) -> Vec<f64> {
        let input = input.clamp(self.domain.value[0], self.domain.value[1]);
        let factor = input.powf(self.exponent.value);
        self.c0
            .value
            .iter()
            .zip(&self.c1.value)
            .enumerate()
            .map(|(index, (c0, c1))| {
                clip_to_range(factor.mul_add(c1 - c0, *c0), self.range.as_ref(), index)
            })
            .collect()
    }
}

impl StitchingFunction {
    pub(crate) fn evaluate(&self, input: f64) -> Option<Vec<f64>> {
        let [low, high] = self.domain.value;
        let input = input.clamp(low, high);
        let index = self
            .bounds
            .value
            .iter()
            .position(|bound| input < *bound)
            .unwrap_or(self.functions.len() - 1);
        let sub_low = if index == 0 {
            low
        } else {
            self.bounds.value[index - 1]
        };
        let sub_high = self.bounds.value.get(index).copied().unwrap_or(high);
        let encoded = interpolate(
            input,
            sub_low,
            sub_high,
            self.encode.value[index * 2],
            self.encode.value[index * 2 + 1],
        );
        let outputs = self.functions[index].evaluate(&[encoded])?;
        Some(
            outputs
                .into_iter()
                .enumerate()
                .map(|(component, value)| clip_to_range(value, self.range.as_ref(), component))
                .collect(),
        )
    }
}

impl SampledFunction {
    #[must_use]
    pub fn raw_sample(&self, index: usize) -> Option<f64> {
        let bits = self.bits_per_sample.value as usize;
        let start = index.checked_mul(bits)?;
        let mut value = 0_u32;
        for offset in 0..bits {
            let bit = start + offset;
            let byte = self.samples.get(bit / 8)?;
            value = (value << 1) | u32::from((byte >> (7 - bit % 8)) & 1);
        }
        Some(f64::from(value))
    }

    fn maximum_sample(&self) -> f64 {
        f64::from(u32::MAX >> (32 - self.bits_per_sample.value))
    }

    pub(crate) fn evaluate(&self, inputs: &[f64]) -> Option<Vec<f64>> {
        let outputs = self.range.value.len() / 2;
        let maximum = self.maximum_sample();
        let mut lower = Vec::with_capacity(inputs.len());
        let mut weights = Vec::with_capacity(inputs.len());
        for (axis, input) in inputs.iter().enumerate() {
            let size = self.size.value[axis];
            let clipped = input.clamp(self.domain.value[axis * 2], self.domain.value[axis * 2 + 1]);
            let encoded = interpolate(
                clipped,
                self.domain.value[axis * 2],
                self.domain.value[axis * 2 + 1],
                self.encode.value[axis * 2],
                self.encode.value[axis * 2 + 1],
            )
            .clamp(0.0, sample_axis_limit(size));
            let floor = encoded.floor();
            let index = axis_index(floor, size);
            lower.push(index);
            weights.push(encoded - floor);
        }
        let corners = 1_usize.checked_shl(u32::try_from(inputs.len()).ok()?)?;
        let mut accumulated = vec![0.0; outputs];
        for corner in 0..corners {
            let mut weight = 1.0;
            let mut offset = 0_usize;
            let mut stride = 1_usize;
            for (axis, size) in self.size.value.iter().enumerate() {
                let upper = (corner >> axis) & 1 == 1;
                let index = if upper {
                    (lower[axis] + 1).min(size - 1)
                } else {
                    lower[axis]
                };
                weight *= if upper {
                    weights[axis]
                } else {
                    1.0 - weights[axis]
                };
                offset += index * stride;
                stride *= size;
            }
            for (component, slot) in accumulated.iter_mut().enumerate() {
                let sample = self.raw_sample(offset * outputs + component)?;
                *slot += weight * sample;
            }
        }
        Some(
            accumulated
                .into_iter()
                .enumerate()
                .map(|(component, sample)| {
                    let decoded = interpolate(
                        sample,
                        0.0,
                        maximum,
                        self.decode.value[component * 2],
                        self.decode.value[component * 2 + 1],
                    );
                    decoded.clamp(
                        self.range.value[component * 2],
                        self.range.value[component * 2 + 1],
                    )
                })
                .collect(),
        )
    }
}

pub(crate) fn interpolate(
    value: f64,
    low: f64,
    high: f64,
    target_low: f64,
    target_high: f64,
) -> f64 {
    let span = high - low;
    if span <= 0.0 {
        return target_low;
    }
    ((value - low) / span).mul_add(target_high - target_low, target_low)
}

fn clip_to_range(value: f64, range: Option<&Derived<Vec<f64>>>, component: usize) -> f64 {
    range.map_or(value, |range| {
        value.clamp(range.value[component * 2], range.value[component * 2 + 1])
    })
}

fn sample_axis_limit(size: usize) -> f64 {
    axis_position(size - 1)
}

fn axis_position(index: usize) -> f64 {
    f64::from(u32::try_from(index).unwrap_or(u32::MAX))
}

fn axis_index(position: f64, size: usize) -> usize {
    let mut low = 0_usize;
    let mut high = size - 1;
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        if axis_position(middle) <= position {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    low
}

pub(crate) type FunctionLoader<'a> =
    dyn Fn(Reference, usize) -> Result<FunctionObject, PageContentError> + 'a;

pub(crate) struct FunctionParseState<'a> {
    pub(crate) count: usize,
    pub(crate) limits: PaintLimits,
    pub(crate) load: &'a FunctionLoader<'a>,
}

pub(crate) fn clamp_to_domain(value: f64, function: &Function, axis: usize) -> f64 {
    let [low, high] = function.input_domain(axis).unwrap_or([0.0, 1.0]);
    value.clamp(low, high)
}

pub(crate) fn parse_function(
    operation: &Operation,
    source: &ByteStore,
    value: &Object,
    outputs: usize,
    depth: usize,
    state: &mut FunctionParseState<'_>,
) -> Result<Function, InterpretError> {
    if depth >= state.limits.max_function_depth {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::FunctionDepthLimit,
        ));
    }
    if state.count >= state.limits.max_functions {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::FunctionCountLimit,
        ));
    }
    state.count += 1;
    if let ObjectKind::Reference(reference) = value.kind() {
        let resolved =
            (state.load)(*reference, state.limits.max_function_data_bytes).map_err(|error| {
                InterpretError::at(
                    operation,
                    InterpretErrorKind::FunctionResource(error.kind()),
                )
            })?;
        return parse_function_dictionary(
            operation,
            &resolved.source,
            &resolved.dictionary,
            Some(*reference),
            resolved.data.as_ref(),
            outputs,
            depth,
            state,
        );
    }
    parse_function_dictionary(operation, source, value, None, None, outputs, depth, state)
}

#[allow(clippy::too_many_arguments)]
fn parse_function_dictionary(
    operation: &Operation,
    source: &ByteStore,
    value: &Object,
    reference: Option<Reference>,
    data: Option<&FunctionData>,
    outputs: usize,
    depth: usize,
    state: &mut FunctionParseState<'_>,
) -> Result<Function, InterpretError> {
    let ObjectKind::Dictionary(entries) = value.kind() else {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::UnsupportedFunction,
        ));
    };
    let function_type_object = unique_resource_entry(
        entries,
        source,
        b"/FunctionType",
        operation,
        InterpretErrorKind::InvalidFunction,
    )?
    .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::FunctionMissingEntry))?;
    match resource_integer(operation, source, function_type_object)? {
        0 => parse_sampled_function(
            operation,
            source,
            value,
            entries,
            reference,
            data,
            outputs,
            state.limits,
        ),
        4 => parse_postscript_function(
            operation,
            source,
            value,
            entries,
            reference,
            data,
            outputs,
            state.limits,
        ),
        2 => parse_exponential_function(operation, source, value, entries, outputs),
        3 => parse_stitching_function(operation, source, value, entries, outputs, depth, state),
        _ => Err(InterpretError::at(
            operation,
            InterpretErrorKind::UnsupportedFunction,
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn parse_sampled_function(
    operation: &Operation,
    source: &ByteStore,
    dictionary: &Object,
    entries: &[pdf_syntax::DictionaryEntry],
    reference: Option<Reference>,
    data: Option<&FunctionData>,
    outputs: usize,
    limits: PaintLimits,
) -> Result<Function, InterpretError> {
    validate_function_dictionary(
        entries,
        source,
        operation,
        &[
            b"/FunctionType",
            b"/Domain",
            b"/Range",
            b"/Size",
            b"/BitsPerSample",
            b"/Order",
            b"/Encode",
            b"/Decode",
            b"/Length",
            b"/Filter",
            b"/DecodeParms",
            b"/DL",
        ],
    )?;
    let data =
        data.ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::FunctionMissingData))?;
    let domain = function_domain_vector(entries, source, operation)?;
    let inputs = domain.value.len() / 2;
    if inputs > limits.max_function_inputs {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::FunctionInputLimit,
        ));
    }
    let range = function_range(entries, source, operation, outputs)?
        .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::FunctionMissingEntry))?;
    if let Some(order) = optional_function_entry(entries, source, b"/Order", operation)? {
        let order = resource_integer(operation, source, order)
            .map_err(|_| InterpretError::at(operation, InterpretErrorKind::InvalidFunction))?;
        if order != 1 {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::UnsupportedFunctionEntry,
            ));
        }
    }
    let size_object = required_function_entry(entries, source, b"/Size", operation)?;
    let size = sampled_function_size(operation, source, size_object, inputs)?;
    let bits_object = required_function_entry(entries, source, b"/BitsPerSample", operation)?;
    let bits = resource_integer(operation, source, bits_object)?;
    let bits_per_sample = u32::try_from(bits)
        .ok()
        .filter(|bits| matches!(bits, 1 | 2 | 4 | 8 | 12 | 16 | 24 | 32))
        .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::InvalidFunction))?;
    let encode = sampled_function_pairs(entries, source, b"/Encode", operation, inputs, || {
        size.iter()
            .flat_map(|count| [0.0, sample_axis_limit(*count)])
            .collect()
    })?;
    let decode = sampled_function_pairs(entries, source, b"/Decode", operation, outputs, || {
        range.value.clone()
    })?;
    let samples = size
        .iter()
        .try_fold(1_usize, |total, count| total.checked_mul(*count))
        .and_then(|total| total.checked_mul(outputs))
        .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::InvalidFunction))?;
    let required_bits = samples
        .checked_mul(bits_per_sample as usize)
        .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::InvalidFunction))?;
    if data.bytes.len() < required_bits.div_ceil(8) {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidFunction,
        ));
    }
    Ok(Function::Sampled(Box::new(SampledFunction {
        dictionary_span: dictionary.span(),
        reference,
        encoded_data_span: data.encoded_data_span,
        domain,
        range,
        size: Derived::assigned(size, size_object.span()),
        bits_per_sample: Derived::assigned(bits_per_sample, bits_object.span()),
        encode,
        decode,
        samples: Arc::clone(&data.bytes),
    })))
}

fn sampled_function_size(
    operation: &Operation,
    source: &ByteStore,
    value: &Object,
    inputs: usize,
) -> Result<Vec<usize>, InterpretError> {
    let ObjectKind::Array(entries) = value.kind() else {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidFunction,
        ));
    };
    if entries.len() != inputs {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidFunction,
        ));
    }
    entries
        .iter()
        .map(|entry| {
            let count = resource_integer(operation, source, entry)?;
            u32::try_from(count)
                .ok()
                .filter(|count| *count >= 1)
                .map(|count| count as usize)
                .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::InvalidFunction))
        })
        .collect()
}

fn sampled_function_pairs(
    entries: &[pdf_syntax::DictionaryEntry],
    source: &ByteStore,
    key: &[u8],
    operation: &Operation,
    pairs: usize,
    default: impl FnOnce() -> Vec<f64>,
) -> Result<Derived<Vec<f64>>, InterpretError> {
    let Some(object) = optional_function_entry(entries, source, key, operation)? else {
        return Ok(Derived::initial(default()));
    };
    let values = resource_number_vector(
        operation,
        source,
        object,
        InterpretErrorKind::InvalidFunction,
    )?;
    if values.len() != pairs * 2 {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidFunction,
        ));
    }
    Ok(Derived::assigned(values, object.span()))
}

#[allow(clippy::too_many_arguments)]
fn parse_postscript_function(
    operation: &Operation,
    source: &ByteStore,
    dictionary: &Object,
    entries: &[pdf_syntax::DictionaryEntry],
    reference: Option<Reference>,
    data: Option<&FunctionData>,
    outputs: usize,
    limits: PaintLimits,
) -> Result<Function, InterpretError> {
    validate_function_dictionary(
        entries,
        source,
        operation,
        &[
            b"/FunctionType",
            b"/Domain",
            b"/Range",
            b"/Length",
            b"/Filter",
            b"/DecodeParms",
            b"/DL",
        ],
    )?;
    let data =
        data.ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::FunctionMissingData))?;
    let domain = function_domain_vector(entries, source, operation)?;
    if domain.value.len() / 2 > limits.max_function_inputs {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::FunctionInputLimit,
        ));
    }
    let range = function_range(entries, source, operation, outputs)?
        .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::FunctionMissingEntry))?;
    let program = postscript::parse(&data.bytes).map_err(|error| {
        InterpretError::at(operation, InterpretErrorKind::PostScriptFunction(error))
    })?;
    Ok(Function::PostScript(Box::new(PostScriptFunction {
        dictionary_span: dictionary.span(),
        reference,
        encoded_data_span: data.encoded_data_span,
        domain,
        range,
        program,
    })))
}

fn parse_exponential_function(
    operation: &Operation,
    source: &ByteStore,
    dictionary: &Object,
    entries: &[pdf_syntax::DictionaryEntry],
    outputs: usize,
) -> Result<Function, InterpretError> {
    validate_function_dictionary(
        entries,
        source,
        operation,
        &[
            b"/FunctionType",
            b"/Domain",
            b"/Range",
            b"/C0",
            b"/C1",
            b"/N",
        ],
    )?;
    let domain = function_domain(entries, source, operation)?;
    let range = function_range(entries, source, operation, outputs)?;
    let c0 = function_components_entry(entries, source, b"/C0", operation, vec![0.0])?;
    let c1 = function_components_entry(entries, source, b"/C1", operation, vec![1.0])?;
    if c0.value.len() != outputs || c1.value.len() != outputs {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidFunction,
        ));
    }
    let exponent_object = required_function_entry(entries, source, b"/N", operation)?;
    let exponent = resource_number(operation, source, exponent_object)?;
    if exponent <= 0.0 {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidFunction,
        ));
    }
    Ok(Function::Exponential(Box::new(ExponentialFunction {
        dictionary_span: dictionary.span(),
        domain,
        range,
        c0,
        c1,
        exponent: Derived::assigned(exponent, exponent_object.span()),
    })))
}

#[allow(clippy::too_many_arguments)]
fn parse_stitching_function(
    operation: &Operation,
    source: &ByteStore,
    dictionary: &Object,
    entries: &[pdf_syntax::DictionaryEntry],
    outputs: usize,
    depth: usize,
    state: &mut FunctionParseState<'_>,
) -> Result<Function, InterpretError> {
    validate_function_dictionary(
        entries,
        source,
        operation,
        &[
            b"/FunctionType",
            b"/Domain",
            b"/Range",
            b"/Functions",
            b"/Bounds",
            b"/Encode",
        ],
    )?;
    let domain = function_domain(entries, source, operation)?;
    let range = function_range(entries, source, operation, outputs)?;
    let functions_object = required_function_entry(entries, source, b"/Functions", operation)?;
    let ObjectKind::Array(function_objects) = functions_object.kind() else {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidFunction,
        ));
    };
    if function_objects.is_empty() {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidFunction,
        ));
    }
    let functions = function_objects
        .iter()
        .map(|function| parse_function(operation, source, function, outputs, depth + 1, state))
        .collect::<Result<Vec<_>, _>>()?;
    if functions.iter().any(|function| function.inputs() != 1) {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidFunction,
        ));
    }
    let bounds_object = required_function_entry(entries, source, b"/Bounds", operation)?;
    let bounds = resource_number_vector(
        operation,
        source,
        bounds_object,
        InterpretErrorKind::InvalidFunction,
    )?;
    if bounds.len() + 1 != functions.len()
        || bounds
            .iter()
            .copied()
            .try_fold(domain.value[0], |previous, bound| {
                (bound >= previous && bound <= domain.value[1]).then_some(bound)
            })
            .is_none()
    {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidFunction,
        ));
    }
    let encode_object = required_function_entry(entries, source, b"/Encode", operation)?;
    let encode = resource_number_vector(
        operation,
        source,
        encode_object,
        InterpretErrorKind::InvalidFunction,
    )?;
    if encode.len() != functions.len() * 2 {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidFunction,
        ));
    }
    Ok(Function::Stitching(Box::new(StitchingFunction {
        dictionary_span: dictionary.span(),
        domain,
        range,
        functions,
        bounds: Derived::assigned(bounds, bounds_object.span()),
        encode: Derived::assigned(encode, encode_object.span()),
    })))
}

fn validate_function_dictionary(
    entries: &[pdf_syntax::DictionaryEntry],
    source: &ByteStore,
    operation: &Operation,
    allowed: &[&[u8]],
) -> Result<(), InterpretError> {
    let mut seen = std::collections::HashSet::new();
    for entry in entries {
        let key = entry.decoded_key(source).map_err(|_| {
            InterpretError::at(operation, InterpretErrorKind::ResourceSourceFailure)
        })?;
        if !seen.insert(key.clone()) {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidFunction,
            ));
        }
        if !allowed.iter().any(|allowed| *allowed == key) {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::UnsupportedFunctionEntry,
            ));
        }
    }
    Ok(())
}

fn optional_function_entry<'a>(
    entries: &'a [pdf_syntax::DictionaryEntry],
    source: &ByteStore,
    key: &[u8],
    operation: &Operation,
) -> Result<Option<&'a Object>, InterpretError> {
    unique_resource_entry(
        entries,
        source,
        key,
        operation,
        InterpretErrorKind::InvalidFunction,
    )
}

fn required_function_entry<'a>(
    entries: &'a [pdf_syntax::DictionaryEntry],
    source: &ByteStore,
    key: &[u8],
    operation: &Operation,
) -> Result<&'a Object, InterpretError> {
    optional_function_entry(entries, source, key, operation)?
        .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::FunctionMissingEntry))
}

fn function_domain(
    entries: &[pdf_syntax::DictionaryEntry],
    source: &ByteStore,
    operation: &Operation,
) -> Result<Derived<[f64; 2]>, InterpretError> {
    let object = required_function_entry(entries, source, b"/Domain", operation)?;
    let domain = resource_numbers::<2>(
        operation,
        source,
        object,
        InterpretErrorKind::InvalidFunction,
    )?;
    if domain[0] >= domain[1] {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidFunction,
        ));
    }
    Ok(Derived::assigned(domain, object.span()))
}

fn function_domain_vector(
    entries: &[pdf_syntax::DictionaryEntry],
    source: &ByteStore,
    operation: &Operation,
) -> Result<Derived<Vec<f64>>, InterpretError> {
    let object = required_function_entry(entries, source, b"/Domain", operation)?;
    let domain = resource_number_vector(
        operation,
        source,
        object,
        InterpretErrorKind::InvalidFunction,
    )?;
    if domain.is_empty()
        || domain.len() % 2 != 0
        || domain.chunks_exact(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidFunction,
        ));
    }
    Ok(Derived::assigned(domain, object.span()))
}

fn function_range(
    entries: &[pdf_syntax::DictionaryEntry],
    source: &ByteStore,
    operation: &Operation,
    outputs: usize,
) -> Result<Option<Derived<Vec<f64>>>, InterpretError> {
    let Some(object) = optional_function_entry(entries, source, b"/Range", operation)? else {
        return Ok(None);
    };
    let range = resource_number_vector(
        operation,
        source,
        object,
        InterpretErrorKind::InvalidFunction,
    )?;
    if range.len() != outputs * 2 || range.chunks_exact(2).any(|pair| pair[0] > pair[1]) {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidFunction,
        ));
    }
    Ok(Some(Derived::assigned(range, object.span())))
}

fn function_components_entry(
    entries: &[pdf_syntax::DictionaryEntry],
    source: &ByteStore,
    key: &[u8],
    operation: &Operation,
    default: Vec<f64>,
) -> Result<Derived<Vec<f64>>, InterpretError> {
    let Some(object) = optional_function_entry(entries, source, key, operation)? else {
        return Ok(Derived::initial(default));
    };
    Ok(Derived::assigned(
        resource_number_vector(
            operation,
            source,
            object,
            InterpretErrorKind::InvalidFunction,
        )?,
        object.span(),
    ))
}
