use pdf_bytes::SourceSpan;
use pdf_content::{Operation, PageResources, ResourceEntry};
use pdf_syntax::{Object, ObjectKind, Reference};

use crate::color::{
    ColorSpace, ColorSpaceParseContext, color_components, parse_color_space_definition,
};
use crate::error::{InterpretError, InterpretErrorKind};
use crate::function::{Function, FunctionParseState, parse_function};
use crate::geometry::normalize_rectangle;
use crate::operand::{
    optional_boolean_pair, optional_component_array, optional_number_pair, resource_boolean,
    resource_integer,
};
use crate::operand::{resource_numbers, unique_resource_entry};
use crate::provenance::Derived;
use crate::state::{GraphicsState, PaintLimits};

#[derive(Clone, Debug, PartialEq)]
pub enum ShadingGeometry {
    Axial(Derived<[f64; 4]>),
    Radial(Derived<[f64; 6]>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShadingPaint {
    pub name: Vec<u8>,
    pub reference: Option<Reference>,
    pub dictionary_span: SourceSpan,
    pub color_space: Derived<ColorSpace>,
    pub geometry: ShadingGeometry,
    pub domain: Derived<[f64; 2]>,
    pub extend: Derived<[bool; 2]>,
    pub background: Option<Derived<Vec<f64>>>,
    pub bbox: Option<Derived<[f64; 4]>>,
    pub anti_alias: Derived<bool>,
    pub function: Function,
    pub state: GraphicsState,
    pub ignored_entries: Vec<SourceSpan>,
}

struct ShadingCommon {
    pub(crate) background: Option<Derived<Vec<f64>>>,
    pub(crate) bbox: Option<Derived<[f64; 4]>>,
    pub(crate) anti_alias: Derived<bool>,
}

pub(crate) fn parse_shading(
    operation: &Operation,
    name: Vec<u8>,
    resource: &ResourceEntry,
    resources: &PageResources,
    state: GraphicsState,
    limits: PaintLimits,
) -> Result<ShadingPaint, InterpretError> {
    let ObjectKind::Dictionary(entries) = resource.value().kind() else {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::ShadingNotDictionary,
        ));
    };
    let ignored_entries = validate_shading_dictionary(entries, resource, operation)?;
    if let Some(shading_type_name) = unique_resource_entry(
        entries,
        resource.source(),
        b"/Type",
        operation,
        InterpretErrorKind::InvalidShadingEntry,
    )? && !shading_type_name.name_equals(resource.source(), b"/Shading")
    {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidShadingEntry,
        ));
    }
    let shading_type_object =
        required_shading_entry(entries, resource, b"/ShadingType", operation)?;
    let shading_type = resource_integer(operation, resource.source(), shading_type_object)?;
    if !matches!(shading_type, 2 | 3) {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::UnsupportedShadingType,
        ));
    }
    let color_space_object = required_shading_entry(entries, resource, b"/ColorSpace", operation)?;
    let color_space =
        shading_color_space(operation, resource, resources, color_space_object, limits)?;
    let components = color_components(&color_space.value).ok_or_else(|| {
        InterpretError::at(operation, InterpretErrorKind::UnsupportedShadingColorSpace)
    })?;
    let coords_object = required_shading_entry(entries, resource, b"/Coords", operation)?;
    let geometry = if shading_type == 2 {
        ShadingGeometry::Axial(Derived::assigned(
            resource_numbers::<4>(
                operation,
                resource.source(),
                coords_object,
                InterpretErrorKind::InvalidShadingEntry,
            )?,
            coords_object.span(),
        ))
    } else {
        let coords = resource_numbers::<6>(
            operation,
            resource.source(),
            coords_object,
            InterpretErrorKind::InvalidShadingEntry,
        )?;
        if coords[2] < 0.0 || coords[5] < 0.0 {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidShadingEntry,
            ));
        }
        ShadingGeometry::Radial(Derived::assigned(coords, coords_object.span()))
    };
    let domain = optional_number_pair(entries, resource, b"/Domain", operation, [0.0, 1.0])?;
    if domain.value[0] >= domain.value[1] {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidShadingEntry,
        ));
    }
    let extend = optional_boolean_pair(entries, resource, b"/Extend", operation)?;
    let common = parse_shading_common(entries, resource, operation, components)?;
    let function_object = required_shading_entry(entries, resource, b"/Function", operation)?;
    let function =
        parse_shading_function(operation, resource, function_object, components, limits)?;
    Ok(ShadingPaint {
        ignored_entries: ignored_entries.clone(),
        name,
        reference: resource.reference(),
        dictionary_span: resource.value().span(),
        color_space,
        geometry,
        domain,
        extend,
        background: common.background,
        bbox: common.bbox,
        anti_alias: common.anti_alias,
        function,
        state,
    })
}

fn parse_shading_function(
    operation: &Operation,
    resource: &ResourceEntry,
    value: &Object,
    components: usize,
    limits: PaintLimits,
) -> Result<Function, InterpretError> {
    let load_function = |reference, limit| resource.function(reference, limit);
    let mut state = FunctionParseState {
        count: 0,
        limits,
        load: &load_function,
    };
    let function = parse_function(
        operation,
        resource.source(),
        value,
        components,
        0,
        &mut state,
    )?;
    if function.inputs() == 1 {
        Ok(function)
    } else {
        Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidFunction,
        ))
    }
}

fn parse_shading_common(
    entries: &[pdf_syntax::DictionaryEntry],
    resource: &ResourceEntry,
    operation: &Operation,
    components: usize,
) -> Result<ShadingCommon, InterpretError> {
    let background =
        optional_component_array(entries, resource, b"/Background", operation, components)?;
    let bbox = unique_resource_entry(
        entries,
        resource.source(),
        b"/BBox",
        operation,
        InterpretErrorKind::InvalidShadingEntry,
    )?
    .map(|value| {
        let span = value.span();
        let (source, value) = match value.kind() {
            ObjectKind::Reference(reference) => {
                let (source, resolved) = resource.resolve_object(*reference).map_err(|_| {
                    InterpretError::at(operation, InterpretErrorKind::InvalidShadingEntry)
                })?;
                (
                    std::borrow::Cow::Owned(source),
                    std::borrow::Cow::Owned(resolved),
                )
            }
            _ => (
                std::borrow::Cow::Borrowed(resource.source()),
                std::borrow::Cow::Borrowed(value),
            ),
        };
        let bounds = normalize_rectangle(resource_numbers::<4>(
            operation,
            &source,
            &value,
            InterpretErrorKind::InvalidShadingEntry,
        )?);
        Ok(Derived::assigned(bounds, span))
    })
    .transpose()?;
    let anti_alias = if let Some(value) = unique_resource_entry(
        entries,
        resource.source(),
        b"/AntiAlias",
        operation,
        InterpretErrorKind::InvalidShadingEntry,
    )? {
        Derived::assigned(resource_boolean(operation, value)?, value.span())
    } else {
        Derived::initial(false)
    };
    Ok(ShadingCommon {
        background,
        bbox,
        anti_alias,
    })
}

fn validate_shading_dictionary(
    entries: &[pdf_syntax::DictionaryEntry],
    resource: &ResourceEntry,
    operation: &Operation,
) -> Result<Vec<SourceSpan>, InterpretError> {
    let mut seen = std::collections::HashSet::new();
    let mut ignored = Vec::new();
    for entry in entries {
        let key = entry.decoded_key(resource.source()).map_err(|_| {
            InterpretError::at(operation, InterpretErrorKind::ResourceSourceFailure)
        })?;
        if !seen.insert(key.clone()) {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidShadingEntry,
            ));
        }
        if !matches!(
            key.as_slice(),
            b"/Type"
                | b"/ShadingType"
                | b"/ColorSpace"
                | b"/Background"
                | b"/BBox"
                | b"/AntiAlias"
                | b"/Coords"
                | b"/Domain"
                | b"/Function"
                | b"/Extend"
        ) {
            ignored.push(entry.value().span());
        }
    }
    Ok(ignored)
}

fn required_shading_entry<'a>(
    entries: &'a [pdf_syntax::DictionaryEntry],
    resource: &ResourceEntry,
    key: &[u8],
    operation: &Operation,
) -> Result<&'a Object, InterpretError> {
    unique_resource_entry(
        entries,
        resource.source(),
        key,
        operation,
        InterpretErrorKind::InvalidShadingEntry,
    )?
    .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::ShadingMissingEntry))
}

fn shading_color_space(
    operation: &Operation,
    resource: &ResourceEntry,
    resources: &PageResources,
    value: &Object,
    limits: PaintLimits,
) -> Result<Derived<ColorSpace>, InterpretError> {
    let load_icc = |reference, limit| resource.icc_profile(reference, limit);
    let load_indexed = |reference, limit| resource.indexed_lookup(reference, limit);
    let load_function = |reference, limit| resource.function(reference, limit);
    let load_object = |reference| resource.resolve_object(reference);
    let context = ColorSpaceParseContext {
        resources: Some(resources),
        load_icc: &load_icc,
        load_indexed: &load_indexed,
        load_function: &load_function,
        load_object: &load_object,
        limits,
    };
    let space = parse_color_space_definition(
        operation,
        resource.source(),
        value,
        InterpretErrorKind::UnsupportedShadingColorSpace,
        &context,
        0,
    )?;
    if matches!(space, ColorSpace::Pattern(_)) {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::UnsupportedShadingColorSpace,
        ));
    }
    Ok(Derived::assigned(space, value.span()))
}
