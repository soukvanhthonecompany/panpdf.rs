use std::cmp::Ordering;
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceSpan};
use pdf_content::{
    IccProfileStream, IndexedLookupStream, Operation, PageContentError, PageResources,
    StringProtection,
};
use pdf_syntax::{Object, ObjectKind, Reference, decode_name, decode_string};

use crate::error::{InterpretError, InterpretErrorKind};
use crate::function::{Function, FunctionLoader, FunctionParseState, parse_function};
use crate::graph::{ShadingPatternPaint, TilingPatternPaint};
use crate::operand::{
    source_exact_integer, source_number, source_number_vector, source_numbers,
    unique_resource_entry,
};
use crate::provenance::Derived;
use crate::state::PaintLimits;

#[derive(Clone, Debug, PartialEq)]
pub enum Color {
    DeviceGray(f64),
    DeviceRgb(f64, f64, f64),
    DeviceCmyk(f64, f64, f64, f64),
    CalGray(f64),
    CalRgb(f64, f64, f64),
    Lab(f64, f64, f64),
    IccBased(Vec<f64>),
    Indexed(u8),
    Separation(f64),
    DeviceN(Vec<f64>),
    PatternUnspecified,
    TilingPattern(Arc<TilingPatternPaint>),
    ShadingPattern(Arc<ShadingPatternPaint>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ColorSpace {
    DeviceGray,
    DeviceRgb,
    DeviceCmyk,
    CalGray(Arc<CalGraySpace>),
    CalRgb(Arc<CalRgbSpace>),
    Lab(Arc<LabSpace>),
    IccBased(Arc<IccBasedSpace>),
    Indexed(Arc<IndexedSpace>),
    Separation(Arc<SeparationSpace>),
    DeviceN(Arc<DeviceNSpace>),
    Pattern(Option<Arc<ColorSpace>>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct CalGraySpace {
    pub dictionary_span: SourceSpan,
    pub white_point: Derived<[f64; 3]>,
    pub black_point: Derived<[f64; 3]>,
    pub gamma: Derived<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CalRgbSpace {
    pub dictionary_span: SourceSpan,
    pub white_point: Derived<[f64; 3]>,
    pub black_point: Derived<[f64; 3]>,
    pub gamma: Derived<[f64; 3]>,
    pub matrix: Derived<[f64; 9]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LabSpace {
    pub dictionary_span: SourceSpan,
    pub white_point: Derived<[f64; 3]>,
    pub black_point: Derived<[f64; 3]>,
    pub range: Derived<[f64; 4]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IccProfileHeader {
    pub version: [u8; 4],
    pub profile_class: [u8; 4],
    pub data_color_space: [u8; 4],
    pub connection_space: [u8; 4],
    pub tag_count: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum IccAlternate {
    DeviceGray,
    DeviceRgb,
    DeviceCmyk,
    CalGray(Arc<CalGraySpace>),
    CalRgb(Arc<CalRgbSpace>),
    Lab(Arc<LabSpace>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct IccBasedSpace {
    pub reference: Reference,
    pub dictionary_span: SourceSpan,
    pub encoded_data_span: SourceSpan,
    pub profile: Arc<[u8]>,
    pub header: IccProfileHeader,
    pub components: Derived<usize>,
    pub alternate: Derived<IccAlternate>,
    pub range: Derived<Vec<f64>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum IndexedBase {
    DeviceGray,
    DeviceRgb,
    DeviceCmyk,
    CalGray(Arc<CalGraySpace>),
    CalRgb(Arc<CalRgbSpace>),
    Lab(Arc<LabSpace>),
    IccBased(Arc<IccBasedSpace>),
    Separation(Arc<SeparationSpace>),
    DeviceN(Arc<DeviceNSpace>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndexedLookupSource {
    String(SourceSpan),
    Stream {
        reference: Reference,
        dictionary_span: SourceSpan,
        encoded_data_span: SourceSpan,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct IndexedSpace {
    pub array_span: SourceSpan,
    pub base: Derived<IndexedBase>,
    pub hival: Derived<u8>,
    pub lookup: Derived<Arc<[u8]>>,
    pub lookup_source: IndexedLookupSource,
}

impl IndexedSpace {
    #[must_use]
    pub fn base_components(&self, index: u8) -> Vec<f64> {
        let index = index.min(self.hival.value);
        let ranges = indexed_base_ranges(&self.base.value);
        let start = usize::from(index) * ranges.len();
        self.lookup.value[start..start + ranges.len()]
            .iter()
            .zip(ranges)
            .map(|(sample, [minimum, maximum])| {
                (f64::from(*sample) / 255.0).mul_add(maximum - minimum, minimum)
            })
            .collect()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Colorant {
    All,
    None,
    Named(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SeparationSpace {
    pub array_span: SourceSpan,
    pub colorant: Derived<Colorant>,
    pub alternate: Derived<Box<ColorSpace>>,
    pub tint_transform: Derived<Function>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeviceNSpace {
    pub array_span: SourceSpan,
    pub colorants: Derived<Vec<Colorant>>,
    pub alternate: Derived<Box<ColorSpace>>,
    pub tint_transform: Derived<Function>,
    pub attributes: Option<SourceSpan>,
}

impl SeparationSpace {
    #[must_use]
    pub fn alternate_components(&self, tint: f64) -> Option<Vec<f64>> {
        self.tint_transform.value.evaluate(&[tint])
    }
}

impl DeviceNSpace {
    #[must_use]
    pub fn alternate_components(&self, tints: &[f64]) -> Option<Vec<f64>> {
        self.tint_transform.value.evaluate(tints)
    }
}

pub(crate) fn clamp_unit(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

pub(crate) fn predefined_color_space(name: &[u8]) -> Option<ColorSpace> {
    match name {
        b"/DeviceGray" | b"/G" => Some(ColorSpace::DeviceGray),
        b"/DeviceRGB" | b"/RGB" => Some(ColorSpace::DeviceRgb),
        b"/DeviceCMYK" | b"/CMYK" => Some(ColorSpace::DeviceCmyk),
        b"/Pattern" => Some(ColorSpace::Pattern(None)),
        _ => None,
    }
}

type IccProfileLoader<'a> =
    dyn Fn(Reference, usize) -> Result<IccProfileStream, PageContentError> + 'a;

type IndexedLookupLoader<'a> =
    dyn Fn(Reference, usize) -> Result<IndexedLookupStream, PageContentError> + 'a;

pub(crate) type ProtectedObjectLoader<'a> =
    dyn Fn(Reference) -> Result<(ByteStore, Object, StringProtection), PageContentError> + 'a;

pub(crate) type StringDecryptor<'a> =
    dyn Fn(StringProtection, Vec<u8>) -> Result<Vec<u8>, PageContentError> + 'a;

pub(crate) struct ColorSpaceParseContext<'a> {
    pub(crate) resources: Option<&'a PageResources>,
    pub(crate) load_icc: &'a IccProfileLoader<'a>,
    pub(crate) load_indexed: &'a IndexedLookupLoader<'a>,
    pub(crate) load_function: &'a FunctionLoader<'a>,
    pub(crate) load_object: &'a ProtectedObjectLoader<'a>,
    pub(crate) string_plaintext: &'a StringDecryptor<'a>,
    pub(crate) limits: PaintLimits,
}

fn follow_color_space(
    operation: &Operation,
    reference: Reference,
    unsupported: InterpretErrorKind,
    context: &ColorSpaceParseContext<'_>,
) -> Result<(ByteStore, Object, StringProtection), InterpretError> {
    let resolved = (context.load_object)(reference).map_err(|error| {
        InterpretError::at(
            operation,
            InterpretErrorKind::ColorSpaceResource(error.kind()),
        )
    })?;
    if matches!(resolved.1.kind(), ObjectKind::Reference(_)) {
        return Err(InterpretError::at(operation, unsupported));
    }
    Ok(resolved)
}

type ArraySpaceParser = fn(
    &Operation,
    &ByteStore,
    StringProtection,
    SourceSpan,
    &[Object],
    &ColorSpaceParseContext<'_>,
    usize,
) -> Result<ColorSpace, InterpretError>;

pub(crate) fn parse_color_space_definition(
    operation: &Operation,
    source: &ByteStore,
    strings: StringProtection,
    value: &Object,
    unsupported: InterpretErrorKind,
    context: &ColorSpaceParseContext<'_>,
    depth: usize,
) -> Result<ColorSpace, InterpretError> {
    if depth >= context.limits.max_color_space_depth {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::ColorSpaceDepthLimit,
        ));
    }
    if let ObjectKind::Reference(reference) = value.kind() {
        let (source, resolved, strings) =
            follow_color_space(operation, *reference, unsupported, context)?;
        return parse_color_space_definition(
            operation,
            &source,
            strings,
            &resolved,
            unsupported,
            context,
            depth + 1,
        );
    }
    if matches!(value.kind(), ObjectKind::Name) {
        let name =
            decode_name(source, value).map_err(|_| InterpretError::at(operation, unsupported))?;
        if let Some(space) = predefined_color_space(&name) {
            return Ok(space);
        }
        let alias = context
            .resources
            .and_then(|resources| resources.color_space(&name))
            .ok_or_else(|| InterpretError::at(operation, unsupported))?;
        return parse_color_space_definition(
            operation,
            alias.source(),
            alias.strings(),
            alias.value(),
            unsupported,
            context,
            depth + 1,
        );
    }
    let ObjectKind::Array(parts) = value.kind() else {
        return Err(InterpretError::at(operation, unsupported));
    };
    let Some(family_object) = parts.first() else {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidColorSpaceEntry,
        ));
    };
    let family = decode_name(source, family_object)
        .map_err(|_| InterpretError::at(operation, InterpretErrorKind::InvalidColorSpaceEntry))?;
    if family == b"/Pattern" {
        return parse_pattern_space(
            operation,
            source,
            strings,
            parts,
            unsupported,
            context,
            depth,
        );
    }
    let array: Option<ArraySpaceParser> = match family.as_slice() {
        b"/Indexed" | b"/I" => Some(parse_indexed),
        b"/Separation" => Some(parse_separation),
        b"/DeviceN" => Some(parse_device_n),
        _ => None,
    };
    if let Some(parse) = array {
        return parse(
            operation,
            source,
            strings,
            value.span(),
            parts,
            context,
            depth,
        );
    }
    if !matches!(
        family.as_slice(),
        b"/CalGray" | b"/CalRGB" | b"/Lab" | b"/ICCBased"
    ) {
        return Err(InterpretError::at(operation, unsupported));
    }
    if parts.len() != 2 {
        return Err(InterpretError::at(
            operation,
            if family == b"/ICCBased" {
                InterpretErrorKind::InvalidIccProfileEntry
            } else {
                InterpretErrorKind::InvalidColorSpaceEntry
            },
        ));
    }
    if family == b"/ICCBased" {
        return parse_icc_based(operation, source, parts, context, depth);
    }
    parse_calibrated_color_space(operation, source, parts, &family, unsupported)
}

fn parse_calibrated_color_space(
    operation: &Operation,
    source: &ByteStore,
    parts: &[Object],
    family: &[u8],
    unsupported: InterpretErrorKind,
) -> Result<ColorSpace, InterpretError> {
    let ObjectKind::Dictionary(entries) = parts[1].kind() else {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidColorSpaceEntry,
        ));
    };
    validate_calibrated_dictionary(operation, source, entries, family)?;
    let white_point_object = calibrated_entry(operation, source, entries, b"/WhitePoint")?
        .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::ColorSpaceMissingEntry))?;
    let white_point = source_numbers::<3>(operation, source, white_point_object)?;
    if white_point[0] <= 0.0
        || white_point[1].to_bits() != 1.0_f64.to_bits()
        || white_point[2] <= 0.0
    {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidColorSpaceEntry,
        ));
    }
    let white_point = Derived::assigned(white_point, white_point_object.span());
    let black_point =
        optional_calibrated_array(operation, source, entries, b"/BlackPoint", [0.0; 3])?;
    if black_point.value.iter().any(|component| *component < 0.0) {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidColorSpaceEntry,
        ));
    }
    let dictionary_span = parts[1].span();
    match family {
        b"/CalGray" => parse_cal_gray(
            operation,
            source,
            entries,
            dictionary_span,
            white_point,
            black_point,
        ),
        b"/CalRGB" => parse_cal_rgb(
            operation,
            source,
            entries,
            dictionary_span,
            white_point,
            black_point,
        ),
        b"/Lab" => parse_lab(
            operation,
            source,
            entries,
            dictionary_span,
            white_point,
            black_point,
        ),
        _ => Err(InterpretError::at(operation, unsupported)),
    }
}

fn parse_cal_gray(
    operation: &Operation,
    source: &ByteStore,
    entries: &[pdf_syntax::DictionaryEntry],
    dictionary_span: SourceSpan,
    white_point: Derived<[f64; 3]>,
    black_point: Derived<[f64; 3]>,
) -> Result<ColorSpace, InterpretError> {
    let gamma = optional_calibrated_number(operation, source, entries, b"/Gamma", 1.0)?;
    if gamma.value <= 0.0 {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidColorSpaceEntry,
        ));
    }
    Ok(ColorSpace::CalGray(Arc::new(CalGraySpace {
        dictionary_span,
        white_point,
        black_point,
        gamma,
    })))
}

fn parse_cal_rgb(
    operation: &Operation,
    source: &ByteStore,
    entries: &[pdf_syntax::DictionaryEntry],
    dictionary_span: SourceSpan,
    white_point: Derived<[f64; 3]>,
    black_point: Derived<[f64; 3]>,
) -> Result<ColorSpace, InterpretError> {
    let gamma = optional_calibrated_array(operation, source, entries, b"/Gamma", [1.0; 3])?;
    if gamma.value.iter().any(|component| *component <= 0.0) {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidColorSpaceEntry,
        ));
    }
    let matrix = optional_calibrated_array(
        operation,
        source,
        entries,
        b"/Matrix",
        [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    )?;
    Ok(ColorSpace::CalRgb(Arc::new(CalRgbSpace {
        dictionary_span,
        white_point,
        black_point,
        gamma,
        matrix,
    })))
}

fn parse_lab(
    operation: &Operation,
    source: &ByteStore,
    entries: &[pdf_syntax::DictionaryEntry],
    dictionary_span: SourceSpan,
    white_point: Derived<[f64; 3]>,
    black_point: Derived<[f64; 3]>,
) -> Result<ColorSpace, InterpretError> {
    let range = optional_calibrated_array(
        operation,
        source,
        entries,
        b"/Range",
        [-100.0, 100.0, -100.0, 100.0],
    )?;
    if range.value[0] >= range.value[1]
        || range.value[2] >= range.value[3]
        || range.value[0] < -128.0
        || range.value[1] > 127.0
        || range.value[2] < -128.0
        || range.value[3] > 127.0
    {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidColorSpaceEntry,
        ));
    }
    Ok(ColorSpace::Lab(Arc::new(LabSpace {
        dictionary_span,
        white_point,
        black_point,
        range,
    })))
}

fn parse_icc_based(
    operation: &Operation,
    _source: &ByteStore,
    parts: &[Object],
    context: &ColorSpaceParseContext<'_>,
    depth: usize,
) -> Result<ColorSpace, InterpretError> {
    let ObjectKind::Reference(reference) = parts[1].kind() else {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidIccProfileEntry,
        ));
    };
    let profile =
        (context.load_icc)(*reference, context.limits.max_icc_profile_bytes).map_err(|error| {
            InterpretError::at(
                operation,
                InterpretErrorKind::IccProfileResource(error.kind()),
            )
        })?;
    let ObjectKind::Dictionary(entries) = profile.dictionary.kind() else {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidIccProfileEntry,
        ));
    };
    validate_icc_dictionary(operation, &profile.source, entries)?;
    let components_object = calibrated_entry(operation, &profile.source, entries, b"/N")?
        .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::IccProfileMissingEntry))?;
    let components = source_exact_integer(
        operation,
        &profile.source,
        components_object,
        InterpretErrorKind::InvalidIccProfileEntry,
    )?;
    let components = usize::try_from(components)
        .ok()
        .filter(|components| matches!(components, 1 | 3 | 4))
        .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::InvalidIccProfileEntry))?;
    let header = validate_icc_bytes(operation, &profile.bytes, components)?;
    let components = Derived::assigned(components, components_object.span());
    let alternate = if let Some(value) =
        calibrated_entry(operation, &profile.source, entries, b"/Alternate")?
    {
        let space = parse_color_space_definition(
            operation,
            &profile.source,
            StringProtection::ByObject(profile.reference),
            value,
            InterpretErrorKind::UnsupportedIccAlternate,
            context,
            depth + 1,
        )?;
        if color_components(&space) != Some(components.value) {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidIccProfileEntry,
            ));
        }
        let alternate = icc_alternate(space).ok_or_else(|| {
            InterpretError::at(operation, InterpretErrorKind::UnsupportedIccAlternate)
        })?;
        Derived::assigned(alternate, value.span())
    } else {
        Derived::initial(match components.value {
            1 => IccAlternate::DeviceGray,
            3 => IccAlternate::DeviceRgb,
            4 => IccAlternate::DeviceCmyk,
            _ => unreachable!("ICC component count checked"),
        })
    };
    let range =
        if let Some(value) = calibrated_entry(operation, &profile.source, entries, b"/Range")? {
            let values = source_number_vector(operation, &profile.source, value)?;
            if values.len() != components.value * 2
                || values.chunks_exact(2).any(|pair| pair[0] > pair[1])
            {
                return Err(InterpretError::at(
                    operation,
                    InterpretErrorKind::InvalidIccProfileEntry,
                ));
            }
            Derived::assigned(values, value.span())
        } else {
            Derived::initial([0.0, 1.0].repeat(components.value))
        };
    Ok(ColorSpace::IccBased(Arc::new(IccBasedSpace {
        reference: profile.reference,
        dictionary_span: profile.dictionary.span(),
        encoded_data_span: profile.encoded_data_span,
        profile: profile.bytes,
        header,
        components,
        alternate,
        range,
    })))
}

fn parse_separation(
    operation: &Operation,
    source: &ByteStore,
    strings: StringProtection,
    array_span: SourceSpan,
    parts: &[Object],
    context: &ColorSpaceParseContext<'_>,
    depth: usize,
) -> Result<ColorSpace, InterpretError> {
    if parts.len() != 4 {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidSeparationColorSpace,
        ));
    }
    let colorant = colorant_name(operation, source, &parts[1])?;
    let alternate = parse_tint_alternate(operation, source, strings, &parts[2], context, depth)?;
    let tint_transform =
        parse_tint_transform(operation, source, &parts[3], alternate.0, 1, context, depth)?;
    Ok(ColorSpace::Separation(Arc::new(SeparationSpace {
        array_span,
        colorant: Derived::assigned(colorant, parts[1].span()),
        alternate: alternate.1,
        tint_transform,
    })))
}

fn parse_device_n(
    operation: &Operation,
    source: &ByteStore,
    strings: StringProtection,
    array_span: SourceSpan,
    parts: &[Object],
    context: &ColorSpaceParseContext<'_>,
    depth: usize,
) -> Result<ColorSpace, InterpretError> {
    if !matches!(parts.len(), 4 | 5) {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidDeviceNColorSpace,
        ));
    }
    let ObjectKind::Array(name_objects) = parts[1].kind() else {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidDeviceNColorSpace,
        ));
    };
    if name_objects.is_empty() || name_objects.len() > context.limits.max_function_inputs {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidDeviceNColorSpace,
        ));
    }
    let mut colorants = Vec::with_capacity(name_objects.len());
    for name_object in name_objects {
        let colorant = colorant_name(operation, source, name_object)?;
        if colorant == Colorant::All {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidDeviceNColorSpace,
            ));
        }
        colorants.push(colorant);
    }
    let alternate = parse_tint_alternate(operation, source, strings, &parts[2], context, depth)?;
    let tint_transform = parse_tint_transform(
        operation,
        source,
        &parts[3],
        alternate.0,
        colorants.len(),
        context,
        depth,
    )?;
    let attributes = match parts.get(4) {
        Some(object) => {
            if !matches!(
                object.kind(),
                ObjectKind::Dictionary(_) | ObjectKind::Reference(_)
            ) {
                return Err(InterpretError::at(
                    operation,
                    InterpretErrorKind::InvalidDeviceNColorSpace,
                ));
            }
            Some(object.span())
        }
        None => None,
    };
    Ok(ColorSpace::DeviceN(Arc::new(DeviceNSpace {
        array_span,
        colorants: Derived::assigned(colorants, parts[1].span()),
        alternate: alternate.1,
        tint_transform,
        attributes,
    })))
}

fn colorant_name(
    operation: &Operation,
    source: &ByteStore,
    value: &Object,
) -> Result<Colorant, InterpretError> {
    let name = decode_name(source, value).map_err(|_| {
        InterpretError::at(operation, InterpretErrorKind::InvalidSeparationColorSpace)
    })?;
    Ok(match name.as_slice() {
        b"/All" => Colorant::All,
        b"/None" => Colorant::None,
        _ => Colorant::Named(name),
    })
}

fn parse_tint_alternate(
    operation: &Operation,
    source: &ByteStore,
    strings: StringProtection,
    value: &Object,
    context: &ColorSpaceParseContext<'_>,
    depth: usize,
) -> Result<(usize, Derived<Box<ColorSpace>>), InterpretError> {
    let space = parse_color_space_definition(
        operation,
        source,
        strings,
        value,
        InterpretErrorKind::UnsupportedTintAlternate,
        context,
        depth + 1,
    )?;
    if matches!(
        space,
        ColorSpace::Pattern(_)
            | ColorSpace::Indexed(_)
            | ColorSpace::Separation(_)
            | ColorSpace::DeviceN(_)
    ) {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::UnsupportedTintAlternate,
        ));
    }
    let components = color_components(&space).ok_or_else(|| {
        InterpretError::at(operation, InterpretErrorKind::UnsupportedTintAlternate)
    })?;
    let provenance = nested_color_space_provenance(operation, source, value, context, depth + 1)?;
    Ok((
        components,
        Derived {
            value: Box::new(space),
            provenance: provenance.into(),
        },
    ))
}

fn parse_tint_transform(
    operation: &Operation,
    source: &ByteStore,
    value: &Object,
    outputs: usize,
    inputs: usize,
    context: &ColorSpaceParseContext<'_>,
    depth: usize,
) -> Result<Derived<Function>, InterpretError> {
    if depth >= context.limits.max_color_space_depth {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::ColorSpaceDepthLimit,
        ));
    }
    let mut state = FunctionParseState {
        count: 0,
        limits: context.limits,
        load: context.load_function,
    };
    let function = parse_function(operation, source, value, outputs, 0, &mut state)?;
    if function.inputs() != inputs {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidTintTransform,
        ));
    }
    Ok(Derived::assigned(function, value.span()))
}

fn parse_indexed(
    operation: &Operation,
    source: &ByteStore,
    strings: StringProtection,
    array_span: SourceSpan,
    parts: &[Object],
    context: &ColorSpaceParseContext<'_>,
    depth: usize,
) -> Result<ColorSpace, InterpretError> {
    if parts.len() != 4 {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidIndexedColorSpace,
        ));
    }
    let base_space = parse_color_space_definition(
        operation,
        source,
        strings,
        &parts[1],
        InterpretErrorKind::UnsupportedIndexedBase,
        context,
        depth + 1,
    )?;
    let components = color_components(&base_space)
        .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::UnsupportedIndexedBase))?;
    let base = indexed_base(base_space)
        .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::UnsupportedIndexedBase))?;
    let base = Derived {
        value: base,
        provenance: nested_color_space_provenance(
            operation,
            source,
            &parts[1],
            context,
            depth + 1,
        )?
        .into(),
    };
    let hival = source_exact_integer(
        operation,
        source,
        &parts[2],
        InterpretErrorKind::InvalidIndexedColorSpace,
    )?;
    let hival = u8::try_from(hival)
        .map_err(|_| InterpretError::at(operation, InterpretErrorKind::InvalidIndexedColorSpace))?;
    let expected = (usize::from(hival) + 1)
        .checked_mul(components)
        .filter(|length| *length <= context.limits.max_indexed_lookup_bytes)
        .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::IndexedLookupLimit))?;
    let lookup_object = &parts[3];
    let (lookup, lookup_source) = match lookup_object.kind() {
        ObjectKind::LiteralString | ObjectKind::HexString => {
            let bytes = indexed_string_lookup(operation, source, strings, lookup_object, context)?;
            (
                Derived::assigned(Arc::<[u8]>::from(bytes), lookup_object.span()),
                IndexedLookupSource::String(lookup_object.span()),
            )
        }
        ObjectKind::Reference(reference) => {
            let cap = context.limits.max_indexed_lookup_bytes;
            let stream = (context.load_indexed)(*reference, cap).map_err(|error| {
                InterpretError::at(
                    operation,
                    InterpretErrorKind::IndexedLookupResource(error.kind()),
                )
            })?;
            validate_indexed_lookup_dictionary(operation, &stream)?;
            let dictionary_span = stream.dictionary.span();
            let encoded_data_span = stream.encoded_data_span;
            (
                Derived {
                    value: stream.bytes,
                    provenance: vec![dictionary_span, encoded_data_span, lookup_object.span()]
                        .into(),
                },
                IndexedLookupSource::Stream {
                    reference: stream.reference,
                    dictionary_span,
                    encoded_data_span,
                },
            )
        }
        _ => {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidIndexedLookup,
            ));
        }
    };
    let lookup = indexed_lookup_of_declared_length(operation, lookup, expected)?;
    Ok(ColorSpace::Indexed(Arc::new(IndexedSpace {
        array_span,
        base,
        hival: Derived::assigned(hival, parts[2].span()),
        lookup,
        lookup_source,
    })))
}

fn indexed_string_lookup(
    operation: &Operation,
    source: &ByteStore,
    strings: StringProtection,
    lookup_object: &Object,
    context: &ColorSpaceParseContext<'_>,
) -> Result<Vec<u8>, InterpretError> {
    let bytes = decode_string(
        source,
        lookup_object,
        context.limits.max_indexed_lookup_bytes,
    )
    .map_err(|_| InterpretError::at(operation, InterpretErrorKind::InvalidIndexedLookup))?;
    (context.string_plaintext)(strings, bytes).map_err(|error| {
        InterpretError::at(
            operation,
            InterpretErrorKind::IndexedLookupResource(error.kind()),
        )
    })
}

fn indexed_lookup_of_declared_length(
    operation: &Operation,
    lookup: Derived<Arc<[u8]>>,
    expected: usize,
) -> Result<Derived<Arc<[u8]>>, InterpretError> {
    match lookup.value.len().cmp(&expected) {
        Ordering::Equal => Ok(lookup),
        Ordering::Greater if lookup.value[expected..].iter().all(u8::is_ascii_whitespace) => {
            Ok(Derived {
                value: Arc::<[u8]>::from(&lookup.value[..expected]),
                provenance: lookup.provenance,
            })
        }
        Ordering::Greater | Ordering::Less => Err(InterpretError::at(
            operation,
            InterpretErrorKind::IndexedLookupLength {
                expected,
                actual: lookup.value.len(),
            },
        )),
    }
}

fn parse_pattern_space(
    operation: &Operation,
    source: &ByteStore,
    strings: StringProtection,
    parts: &[Object],
    unsupported: InterpretErrorKind,
    context: &ColorSpaceParseContext<'_>,
    depth: usize,
) -> Result<ColorSpace, InterpretError> {
    let Some(base_object) = parts.get(1) else {
        return Ok(ColorSpace::Pattern(None));
    };
    let base = parse_color_space_definition(
        operation,
        source,
        strings,
        base_object,
        unsupported,
        context,
        depth + 1,
    )?;
    if matches!(base, ColorSpace::Pattern(_)) {
        return Err(InterpretError::at(operation, unsupported));
    }
    Ok(ColorSpace::Pattern(Some(Arc::new(base))))
}

fn icc_alternate(space: ColorSpace) -> Option<IccAlternate> {
    match space {
        ColorSpace::DeviceGray => Some(IccAlternate::DeviceGray),
        ColorSpace::DeviceRgb => Some(IccAlternate::DeviceRgb),
        ColorSpace::DeviceCmyk => Some(IccAlternate::DeviceCmyk),
        ColorSpace::CalGray(space) => Some(IccAlternate::CalGray(space)),
        ColorSpace::CalRgb(space) => Some(IccAlternate::CalRgb(space)),
        ColorSpace::Lab(space) => Some(IccAlternate::Lab(space)),
        ColorSpace::Pattern(_)
        | ColorSpace::IccBased(_)
        | ColorSpace::Indexed(_)
        | ColorSpace::Separation(_)
        | ColorSpace::DeviceN(_) => None,
    }
}

fn indexed_base(space: ColorSpace) -> Option<IndexedBase> {
    match space {
        ColorSpace::DeviceGray => Some(IndexedBase::DeviceGray),
        ColorSpace::DeviceRgb => Some(IndexedBase::DeviceRgb),
        ColorSpace::DeviceCmyk => Some(IndexedBase::DeviceCmyk),
        ColorSpace::CalGray(space) => Some(IndexedBase::CalGray(space)),
        ColorSpace::CalRgb(space) => Some(IndexedBase::CalRgb(space)),
        ColorSpace::Lab(space) => Some(IndexedBase::Lab(space)),
        ColorSpace::IccBased(space) => Some(IndexedBase::IccBased(space)),
        ColorSpace::Separation(space) => Some(IndexedBase::Separation(space)),
        ColorSpace::DeviceN(space) => Some(IndexedBase::DeviceN(space)),
        ColorSpace::Indexed(_) | ColorSpace::Pattern(_) => None,
    }
}

fn indexed_base_ranges(base: &IndexedBase) -> Vec<[f64; 2]> {
    match base {
        IndexedBase::DeviceGray | IndexedBase::CalGray(_) => vec![[0.0, 1.0]],
        IndexedBase::DeviceRgb | IndexedBase::CalRgb(_) => vec![[0.0, 1.0]; 3],
        IndexedBase::DeviceCmyk => vec![[0.0, 1.0]; 4],
        IndexedBase::Lab(space) => vec![
            [0.0, 100.0],
            [space.range.value[0], space.range.value[1]],
            [space.range.value[2], space.range.value[3]],
        ],
        IndexedBase::IccBased(space) => space
            .range
            .value
            .chunks_exact(2)
            .map(|pair| [pair[0], pair[1]])
            .collect(),
        IndexedBase::Separation(space) => {
            vec![
                space
                    .tint_transform
                    .value
                    .input_domain(0)
                    .unwrap_or([0.0, 1.0]),
            ]
        }
        IndexedBase::DeviceN(space) => (0..space.colorants.value.len())
            .map(|axis| {
                space
                    .tint_transform
                    .value
                    .input_domain(axis)
                    .unwrap_or([0.0, 1.0])
            })
            .collect(),
    }
}

fn nested_color_space_provenance(
    operation: &Operation,
    source: &ByteStore,
    value: &Object,
    context: &ColorSpaceParseContext<'_>,
    depth: usize,
) -> Result<Vec<SourceSpan>, InterpretError> {
    if depth >= context.limits.max_color_space_depth {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::ColorSpaceDepthLimit,
        ));
    }
    if matches!(value.kind(), ObjectKind::Name) {
        let name = decode_name(source, value).map_err(|_| {
            InterpretError::at(operation, InterpretErrorKind::InvalidColorSpaceEntry)
        })?;
        if predefined_color_space(&name).is_none() {
            let alias = context
                .resources
                .and_then(|resources| resources.color_space(&name))
                .ok_or_else(|| {
                    InterpretError::at(operation, InterpretErrorKind::UnsupportedColorSpace)
                })?;
            let mut provenance = nested_color_space_provenance(
                operation,
                alias.source(),
                alias.value(),
                context,
                depth + 1,
            )?;
            provenance.push(value.span());
            return Ok(provenance);
        }
    }
    Ok(vec![value.span()])
}

fn validate_indexed_lookup_dictionary(
    operation: &Operation,
    stream: &IndexedLookupStream,
) -> Result<(), InterpretError> {
    let ObjectKind::Dictionary(entries) = stream.dictionary.kind() else {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidIndexedLookup,
        ));
    };
    let mut seen = std::collections::HashSet::new();
    for entry in entries {
        let key = entry.decoded_key(&stream.source).map_err(|_| {
            InterpretError::at(operation, InterpretErrorKind::ResourceSourceFailure)
        })?;
        if !seen.insert(key.clone()) {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidIndexedLookup,
            ));
        }
        if !matches!(
            key.as_slice(),
            b"/Length" | b"/Filter" | b"/DecodeParms" | b"/DL"
        ) {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::UnsupportedIndexedLookupEntry,
            ));
        }
    }
    Ok(())
}

fn validate_icc_dictionary(
    operation: &Operation,
    source: &ByteStore,
    entries: &[pdf_syntax::DictionaryEntry],
) -> Result<(), InterpretError> {
    let mut seen = std::collections::HashSet::new();
    for entry in entries {
        let key = entry.decoded_key(source).map_err(|_| {
            InterpretError::at(operation, InterpretErrorKind::ResourceSourceFailure)
        })?;
        if !seen.insert(key.clone()) {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidIccProfileEntry,
            ));
        }
        if !matches!(
            key.as_slice(),
            b"/Length"
                | b"/Filter"
                | b"/DecodeParms"
                | b"/N"
                | b"/Alternate"
                | b"/Range"
                | b"/Metadata"
        ) {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::UnsupportedIccProfileEntry,
            ));
        }
    }
    Ok(())
}

fn validate_icc_bytes(
    operation: &Operation,
    bytes: &[u8],
    components: usize,
) -> Result<IccProfileHeader, InterpretError> {
    if bytes.len() < 132
        || u32::from_be_bytes(bytes[0..4].try_into().unwrap()) as usize != bytes.len()
    {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidIccProfile,
        ));
    }
    let version = <[u8; 4]>::try_from(&bytes[8..12]).unwrap();
    let profile_class = <[u8; 4]>::try_from(&bytes[12..16]).unwrap();
    let data_color_space = <[u8; 4]>::try_from(&bytes[16..20]).unwrap();
    let valid_data_space = matches!(
        (components, &data_color_space),
        (1, b"GRAY") | (3, b"RGB " | b"Lab ") | (4, b"CMYK")
    );
    if bytes[36..40] != *b"acsp"
        || !(2..=4).contains(&version[0])
        || !matches!(&profile_class, b"scnr" | b"mntr" | b"prtr" | b"spac")
        || !valid_data_space
        || !matches!(&bytes[20..24], b"XYZ " | b"Lab ")
    {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidIccProfile,
        ));
    }
    let tag_count = u32::from_be_bytes(bytes[128..132].try_into().unwrap());
    let table_bytes = usize::try_from(tag_count)
        .ok()
        .and_then(|count| count.checked_mul(12))
        .and_then(|size| 132_usize.checked_add(size))
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::InvalidIccProfile))?;
    let mut tags = std::collections::HashSet::new();
    for record in bytes[132..table_bytes].chunks_exact(12) {
        if !tags.insert(<[u8; 4]>::try_from(&record[0..4]).unwrap()) {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidIccProfile,
            ));
        }
        let offset = u32::from_be_bytes(record[4..8].try_into().unwrap()) as usize;
        let size = u32::from_be_bytes(record[8..12].try_into().unwrap()) as usize;
        if offset < table_bytes
            || !offset.is_multiple_of(4)
            || size < 8
            || offset.checked_add(size).is_none_or(|end| end > bytes.len())
            || bytes[offset + 4..offset + 8] != [0; 4]
        {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidIccProfile,
            ));
        }
    }
    Ok(IccProfileHeader {
        version,
        profile_class,
        data_color_space,
        connection_space: bytes[20..24].try_into().unwrap(),
        tag_count,
    })
}

fn validate_calibrated_dictionary(
    operation: &Operation,
    source: &ByteStore,
    entries: &[pdf_syntax::DictionaryEntry],
    family: &[u8],
) -> Result<(), InterpretError> {
    let mut seen = std::collections::HashSet::new();
    for entry in entries {
        let key = entry.decoded_key(source).map_err(|_| {
            InterpretError::at(operation, InterpretErrorKind::ResourceSourceFailure)
        })?;
        if !seen.insert(key.clone()) {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidColorSpaceEntry,
            ));
        }
        let supported = match family {
            b"/CalGray" => matches!(key.as_slice(), b"/WhitePoint" | b"/BlackPoint" | b"/Gamma"),
            b"/CalRGB" => matches!(
                key.as_slice(),
                b"/WhitePoint" | b"/BlackPoint" | b"/Gamma" | b"/Matrix"
            ),
            b"/Lab" => matches!(key.as_slice(), b"/WhitePoint" | b"/BlackPoint" | b"/Range"),
            _ => true,
        };
        if !supported {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidColorSpaceEntry,
            ));
        }
    }
    Ok(())
}

fn calibrated_entry<'a>(
    operation: &Operation,
    source: &ByteStore,
    entries: &'a [pdf_syntax::DictionaryEntry],
    key: &[u8],
) -> Result<Option<&'a Object>, InterpretError> {
    unique_resource_entry(
        entries,
        source,
        key,
        operation,
        InterpretErrorKind::InvalidColorSpaceEntry,
    )
}

fn optional_calibrated_number(
    operation: &Operation,
    source: &ByteStore,
    entries: &[pdf_syntax::DictionaryEntry],
    key: &[u8],
    default: f64,
) -> Result<Derived<f64>, InterpretError> {
    calibrated_entry(operation, source, entries, key)?.map_or_else(
        || Ok(Derived::initial(default)),
        |value| {
            source_number(operation, source, value)
                .map(|number| Derived::assigned(number, value.span()))
        },
    )
}

fn optional_calibrated_array<const N: usize>(
    operation: &Operation,
    source: &ByteStore,
    entries: &[pdf_syntax::DictionaryEntry],
    key: &[u8],
    default: [f64; N],
) -> Result<Derived<[f64; N]>, InterpretError> {
    calibrated_entry(operation, source, entries, key)?.map_or_else(
        || Ok(Derived::initial(default)),
        |value| {
            source_numbers(operation, source, value)
                .map(|numbers| Derived::assigned(numbers, value.span()))
        },
    )
}

pub(crate) fn group_blend_space_supported(space: &ColorSpace) -> bool {
    match space {
        ColorSpace::DeviceGray
        | ColorSpace::DeviceRgb
        | ColorSpace::DeviceCmyk
        | ColorSpace::CalGray(_)
        | ColorSpace::CalRgb(_) => true,
        ColorSpace::IccBased(space) => {
            space.header.data_color_space != *b"Lab "
                && space.range.value.chunks_exact(2).all(|pair| {
                    pair[0].to_bits() == 0.0_f64.to_bits() && pair[1].to_bits() == 1.0_f64.to_bits()
                })
        }
        ColorSpace::Lab(_)
        | ColorSpace::Indexed(_)
        | ColorSpace::Separation(_)
        | ColorSpace::DeviceN(_)
        | ColorSpace::Pattern(_) => false,
    }
}

pub(crate) fn color_components(space: &ColorSpace) -> Option<usize> {
    match space {
        ColorSpace::DeviceGray
        | ColorSpace::CalGray(_)
        | ColorSpace::Indexed(_)
        | ColorSpace::Separation(_) => Some(1),
        ColorSpace::DeviceRgb | ColorSpace::CalRgb(_) | ColorSpace::Lab(_) => Some(3),
        ColorSpace::DeviceCmyk => Some(4),
        ColorSpace::IccBased(space) => Some(space.components.value),
        ColorSpace::DeviceN(space) => Some(space.colorants.value.len()),
        ColorSpace::Pattern(_) => None,
    }
}

pub(crate) fn palette_index(value: f64, hival: u8) -> u8 {
    if value.is_nan() || value < 0.5 {
        return 0;
    }
    (0..hival)
        .find(|candidate| value < f64::from(*candidate) + 0.5)
        .unwrap_or(hival)
}
