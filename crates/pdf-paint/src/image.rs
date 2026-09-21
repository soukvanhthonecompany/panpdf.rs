use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceSpan};
use pdf_content::{ImageXObject, Operation};
use pdf_syntax::{Object, ObjectKind, Reference, decode_name};

use crate::ccitt;
use crate::color::{ColorSpace, color_components};
use crate::error::{InterpretError, InterpretErrorKind};
use crate::operand::{
    resource_boolean, resource_integer, resource_number_vector, unique_resource_entry,
};
use crate::provenance::Derived;
use crate::state::GraphicsState;

#[derive(Clone, Debug, PartialEq)]
pub struct DctColorTransform {
    pub value: Derived<u8>,
    pub location: DctParameterLocation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DctParameterLocation {
    DecodeParameters,
    ImageDictionaryExtension,
}

#[derive(Clone, Copy)]
pub(crate) struct ImageSampleShape {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) components: usize,
    pub(crate) bits_per_component: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImagePaint {
    pub reference: Reference,
    pub dictionary_span: SourceSpan,
    pub encoded_data_span: SourceSpan,
    pub width: Derived<u32>,
    pub height: Derived<u32>,
    pub bits_per_component: Derived<u8>,
    pub codec: Option<Derived<pdf_syntax::ImageCodec>>,
    pub dct_color_transform: Option<DctColorTransform>,
    pub color_space: Option<Derived<ColorSpace>>,
    pub image_mask: Derived<bool>,
    pub decode: Derived<Vec<f64>>,
    pub interpolate: Derived<bool>,
    pub soft_mask: Option<Box<ImagePaint>>,
    pub mask: Option<ImageMask>,
    pub matte: Option<Derived<Vec<f64>>>,
    pub samples: Arc<[u8]>,
    pub state: GraphicsState,
}

fn scaled_index(fraction: f64, extent: u32) -> Option<u32> {
    if extent == 0 || !(0.0..1.0).contains(&fraction) {
        return None;
    }
    let scaled = fraction * f64::from(extent);
    let mut low = 0_u32;
    let mut high = extent - 1;
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        if f64::from(middle) <= scaled {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    Some(low)
}

#[derive(Clone, Debug, PartialEq)]
pub enum ImageMask {
    Stencil(Box<ImagePaint>),
    ColorKey(Derived<Vec<u32>>),
}

impl ImagePaint {
    #[must_use]
    pub fn masked_out(&self, x: u32, y: u32) -> bool {
        match self.mask.as_ref() {
            None => false,
            Some(ImageMask::Stencil(stencil)) => {
                let Some(unit) = self.unit_of(x, y) else {
                    return false;
                };
                let Some((column, row)) = stencil.sample_at_unit(unit) else {
                    return false;
                };
                stencil
                    .sample(column, row, 0)
                    .is_some_and(|value| value >= 0.5)
            }
            Some(ImageMask::ColorKey(ranges)) => {
                let components = self.components();
                if ranges.value.len() != components * 2 {
                    return false;
                }
                let bits = usize::from(self.bits_per_component.value);
                let Some(start) = self.sample_offset(x, y, bits, components) else {
                    return false;
                };
                (0..components).all(|component| {
                    self.raw_sample(start + component * bits, bits)
                        .is_some_and(|raw| {
                            raw >= ranges.value[component * 2]
                                && raw <= ranges.value[component * 2 + 1]
                        })
                })
            }
        }
    }

    fn unit_of(&self, x: u32, y: u32) -> Option<[f64; 2]> {
        if self.width.value == 0 || self.height.value == 0 {
            return None;
        }
        Some([
            (f64::from(x) + 0.5) / f64::from(self.width.value),
            (f64::from(y) + 0.5) / f64::from(self.height.value),
        ])
    }

    fn sample_at_unit(&self, unit: [f64; 2]) -> Option<(u32, u32)> {
        Some((
            scaled_index(unit[0], self.width.value)?,
            scaled_index(unit[1], self.height.value)?,
        ))
    }

    #[must_use]
    pub fn components(&self) -> usize {
        if self.image_mask.value {
            1
        } else {
            self.color_space
                .as_ref()
                .and_then(|space| color_components(&space.value))
                .unwrap_or(0)
        }
    }

    #[must_use]
    pub fn sample(&self, x: u32, y: u32, component: usize) -> Option<f64> {
        let components = self.components();
        if x >= self.width.value || y >= self.height.value || component >= components {
            return None;
        }
        let bits = usize::from(self.bits_per_component.value);
        let start = self
            .sample_offset(x, y, bits, components)?
            .checked_add(component.checked_mul(bits)?)?;
        let raw = self.raw_sample(start, bits)?;
        self.decoded(raw, component)
    }

    #[must_use]
    pub fn sample_pixel(&self, x: u32, y: u32, out: &mut [f64]) -> bool {
        let components = self.components();
        if x >= self.width.value || y >= self.height.value || out.len() < components {
            return false;
        }
        let bits = usize::from(self.bits_per_component.value);
        let Some(start) = self.sample_offset(x, y, bits, components) else {
            return false;
        };
        for (component, slot) in out.iter_mut().enumerate().take(components) {
            let Some(raw) = self.raw_sample(start + component * bits, bits) else {
                return false;
            };
            let Some(value) = self.decoded(raw, component) else {
                return false;
            };
            *slot = value;
        }
        true
    }

    fn sample_offset(&self, x: u32, y: u32, bits: usize, components: usize) -> Option<usize> {
        let row_bits = (self.width.value as usize)
            .checked_mul(components)?
            .checked_mul(bits)?;
        let row_bytes = row_bits.div_ceil(8);
        (y as usize)
            .checked_mul(row_bytes)?
            .checked_mul(8)?
            .checked_add((x as usize).checked_mul(components)?.checked_mul(bits)?)
    }

    pub(crate) fn raw_sample(&self, start: usize, bits: usize) -> Option<u32> {
        let mut raw = 0_u32;
        if bits == 8 || bits == 16 {
            for offset in 0..bits / 8 {
                raw = (raw << 8) | u32::from(*self.samples.get(start / 8 + offset)?);
            }
        } else {
            for offset in 0..bits {
                let bit = start + offset;
                let byte = self.samples.get(bit / 8)?;
                raw = (raw << 1) | u32::from((byte >> (7 - bit % 8)) & 1);
            }
        }
        Some(raw)
    }

    pub(crate) fn decoded(&self, raw: u32, component: usize) -> Option<f64> {
        let maximum = f64::from(u32::MAX >> (32 - u32::from(self.bits_per_component.value)));
        let low = *self.decode.value.get(component * 2)?;
        let high = *self.decode.value.get(component * 2 + 1)?;
        Some((f64::from(raw) / maximum).mul_add(high - low, low))
    }
}

pub(crate) fn image_matte(
    entries: &[pdf_syntax::DictionaryEntry],
    source: &ByteStore,
    operation: &Operation,
    depth: usize,
    components: usize,
) -> Result<Option<Derived<Vec<f64>>>, InterpretError> {
    let Some(object) = unique_resource_entry(
        entries,
        source,
        b"/Matte",
        operation,
        InterpretErrorKind::InvalidImageEntry,
    )?
    else {
        return Ok(None);
    };
    if depth == 0 || components != 1 {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidImageEntry,
        ));
    }
    let values = resource_number_vector(
        operation,
        source,
        object,
        InterpretErrorKind::InvalidImageEntry,
    )?;
    if values.is_empty() {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidImageEntry,
        ));
    }
    Ok(Some(Derived::assigned(values, object.span())))
}

pub(crate) fn image_flag(
    entries: &[pdf_syntax::DictionaryEntry],
    source: &ByteStore,
    operation: &Operation,
    key: &[u8],
) -> Result<Derived<bool>, InterpretError> {
    let Some(object) = unique_resource_entry(
        entries,
        source,
        key,
        operation,
        InterpretErrorKind::InvalidImageEntry,
    )?
    else {
        return Ok(Derived::initial(false));
    };
    Ok(Derived::assigned(
        resource_boolean(operation, object)?,
        object.span(),
    ))
}

fn is_dct_decode_parameter(entry: &pdf_syntax::DictionaryEntry, source: &ByteStore) -> bool {
    [
        &b"/ColorTransform"[..],
        b"/Columns",
        b"/Rows",
        b"/Colors",
        b"/HSamples",
        b"/VSamples",
        b"/QFactor",
        b"/Blend",
    ]
    .iter()
    .any(|key| entry.key_equals(source, key))
}

pub(crate) fn ccitt_parameters(
    operation: &Operation,
    source: &ByteStore,
    parameters: Option<&Object>,
) -> Result<ccitt::CcittParameters, InterpretError> {
    let mut held = ccitt::CcittParameters::default();
    let Some(parameters) = parameters else {
        return Ok(held);
    };
    let ObjectKind::Dictionary(entries) = parameters.kind() else {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidImageCodecParameters,
        ));
    };
    for entry in entries {
        let integer = || resource_integer(operation, source, entry.value());
        let boolean = || resource_boolean(operation, entry.value());
        if entry.key_equals(source, b"/K") {
            held.k = integer()?;
        } else if entry.key_equals(source, b"/Columns") {
            held.columns = u32::try_from(integer()?).map_err(|_| {
                InterpretError::at(operation, InterpretErrorKind::InvalidImageCodecParameters)
            })?;
        } else if entry.key_equals(source, b"/Rows") {
            held.rows = u32::try_from(integer()?).map_err(|_| {
                InterpretError::at(operation, InterpretErrorKind::InvalidImageCodecParameters)
            })?;
        } else if entry.key_equals(source, b"/BlackIs1") {
            held.black_is_1 = boolean()?;
        } else if entry.key_equals(source, b"/EncodedByteAlign") {
            held.byte_align = boolean()?;
        } else if entry.key_equals(source, b"/EndOfLine") {
            held.end_of_line = boolean()?;
        } else if entry.key_equals(source, b"/EndOfBlock") {
            held.end_of_block = boolean()?;
        } else if entry.key_equals(source, b"/DamagedRowsBeforeError") {
            if integer()? != 0 {
                return Err(InterpretError::at_span(
                    entry.key().span(),
                    InterpretErrorKind::UnsupportedImageCodecParameters,
                ));
            }
        } else {
            return Err(InterpretError::at_span(
                entry.key().span(),
                InterpretErrorKind::UnsupportedImageCodecParameters,
            ));
        }
    }
    Ok(held)
}

pub(crate) fn dct_color_transform(
    operation: &Operation,
    source: &ByteStore,
    parameters: Option<&Object>,
    image_entries: &[pdf_syntax::DictionaryEntry],
    _components: usize,
) -> Result<Option<DctColorTransform>, InterpretError> {
    let from_parameters = match parameters {
        Some(parameters) => {
            let ObjectKind::Dictionary(entries) = parameters.kind() else {
                return Err(InterpretError::at(
                    operation,
                    InterpretErrorKind::InvalidImageCodecParameters,
                ));
            };
            for entry in entries {
                if !is_dct_decode_parameter(entry, source) {
                    return Err(InterpretError::at_span(
                        entry.key().span(),
                        InterpretErrorKind::UnsupportedImageCodecParameters,
                    ));
                }
            }
            unique_resource_entry(
                entries,
                source,
                b"/ColorTransform",
                operation,
                InterpretErrorKind::InvalidImageCodecParameters,
            )?
        }
        None => None,
    };
    let from_image = unique_resource_entry(
        image_entries,
        source,
        b"/ColorTransform",
        operation,
        InterpretErrorKind::InvalidImageCodecParameters,
    )?;
    let (value, location) = match (from_parameters, from_image) {
        (Some(value), _) => (value, DctParameterLocation::DecodeParameters),
        (None, Some(value)) => (value, DctParameterLocation::ImageDictionaryExtension),
        (None, None) => return Ok(None),
    };
    let parsed = resource_integer(operation, source, value).map_err(|_| {
        InterpretError::at(operation, InterpretErrorKind::InvalidImageCodecParameters)
    })?;
    let parsed = u8::try_from(parsed)
        .ok()
        .filter(|value| matches!(value, 0 | 1))
        .ok_or_else(|| {
            InterpretError::at(operation, InterpretErrorKind::InvalidImageCodecParameters)
        })?;
    Ok(Some(DctColorTransform {
        value: Derived::assigned(parsed, value.span()),
        location,
    }))
}

pub(crate) fn dct_decoder_transform(
    value: u8,
    components: usize,
    operation: &Operation,
) -> Result<jpeg_decoder::ColorTransform, InterpretError> {
    if value == 0 {
        if components == 1 {
            return Ok(jpeg_decoder::ColorTransform::None);
        }
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::UnsupportedImageCodecParameters,
        ));
    }
    match components {
        1 => Ok(jpeg_decoder::ColorTransform::Grayscale),
        3 => Ok(jpeg_decoder::ColorTransform::YCbCr),
        4 => Ok(jpeg_decoder::ColorTransform::YCCK),
        _ => Err(InterpretError::at(
            operation,
            InterpretErrorKind::InvalidImageCodecParameters,
        )),
    }
}

pub(crate) fn jpeg_has_adobe_app14(bytes: &[u8]) -> bool {
    if !bytes.starts_with(&[0xff, 0xd8]) {
        return false;
    }
    let mut cursor = 2_usize;
    while cursor < bytes.len() {
        while bytes.get(cursor) == Some(&0xff) {
            cursor += 1;
        }
        let Some(&marker) = bytes.get(cursor) else {
            return false;
        };
        cursor += 1;
        if matches!(marker, 0xd9 | 0xda) {
            return false;
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        let Some(length_bytes) = bytes.get(cursor..cursor.saturating_add(2)) else {
            return false;
        };
        let length = usize::from(u16::from_be_bytes([length_bytes[0], length_bytes[1]]));
        if length < 2 {
            return false;
        }
        let Some(end) = cursor.checked_add(length) else {
            return false;
        };
        let Some(segment) = bytes.get(cursor + 2..end) else {
            return false;
        };
        if marker == 0xee && segment.len() >= 12 && segment.starts_with(b"Adobe") {
            return true;
        }
        cursor = end;
    }
    false
}

pub(crate) fn image_rendering_intent(
    entries: &[pdf_syntax::DictionaryEntry],
    source: &ByteStore,
    operation: &Operation,
) -> Result<Option<Derived<Vec<u8>>>, InterpretError> {
    let intent = unique_resource_entry(
        entries,
        source,
        b"/Intent",
        operation,
        InterpretErrorKind::InvalidImageEntry,
    )?;
    let Some(intent) = intent else {
        return Ok(None);
    };
    let name = decode_name(source, intent)
        .map_err(|_| InterpretError::at(operation, InterpretErrorKind::InvalidImageEntry))?;
    Ok(Some(Derived::assigned(name, intent.span())))
}

pub(crate) fn image_bits_per_component(
    entries: &[pdf_syntax::DictionaryEntry],
    source: &ByteStore,
    operation: &Operation,
    image_mask: bool,
) -> Result<Derived<u8>, InterpretError> {
    let object = unique_resource_entry(
        entries,
        source,
        b"/BitsPerComponent",
        operation,
        InterpretErrorKind::InvalidImageEntry,
    )?;
    match (image_mask, object) {
        (true, Some(object)) => {
            if resource_integer(operation, source, object)? != 1 {
                return Err(InterpretError::at(
                    operation,
                    InterpretErrorKind::InvalidImageEntry,
                ));
            }
            Ok(Derived::assigned(1, object.span()))
        }
        (true, None) => Ok(Derived::initial(1)),
        (false, None) => Err(InterpretError::at(
            operation,
            InterpretErrorKind::ImageMissingEntry,
        )),
        (false, Some(object)) => {
            let bits = resource_integer(operation, source, object)?;
            let bits = u8::try_from(bits)
                .ok()
                .filter(|bits| matches!(bits, 1 | 2 | 4 | 8 | 16))
                .ok_or_else(|| {
                    InterpretError::at(operation, InterpretErrorKind::InvalidImageEntry)
                })?;
            Ok(Derived::assigned(bits, object.span()))
        }
    }
}

pub(crate) fn require_addressable_samples(
    operation: &Operation,
    available: usize,
    width: u32,
    height: u32,
    components: usize,
    bits: u8,
) -> Result<(), InterpretError> {
    let row_bytes = (width as usize)
        .checked_mul(components)
        .and_then(|samples| samples.checked_mul(usize::from(bits)))
        .map(|bits| bits.div_ceil(8))
        .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::ImageLimit))?;
    let required = row_bytes
        .checked_mul(height as usize)
        .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::ImageLimit))?;
    if available < required {
        return Err(InterpretError::at(
            operation,
            InterpretErrorKind::ImageSampleShortfall,
        ));
    }
    Ok(())
}

pub(crate) fn subtype_is(image: &ImageXObject, expected: &[u8]) -> bool {
    let ObjectKind::Dictionary(entries) = image.dictionary.kind() else {
        return false;
    };
    entries
        .iter()
        .find(|entry| entry.key_equals(&image.source, b"/Subtype"))
        .is_some_and(|entry| entry.value().name_equals(&image.source, expected))
}

pub(crate) fn image_dimension(
    entries: &[pdf_syntax::DictionaryEntry],
    source: &ByteStore,
    operation: &Operation,
    key: &[u8],
) -> Result<Derived<u32>, InterpretError> {
    let object = unique_resource_entry(
        entries,
        source,
        key,
        operation,
        InterpretErrorKind::InvalidImageEntry,
    )?
    .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::ImageMissingEntry))?;
    let value = resource_integer(operation, source, object)?;
    let value = u32::try_from(value)
        .ok()
        .filter(|value| *value >= 1)
        .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::InvalidImageEntry))?;
    Ok(Derived::assigned(value, object.span()))
}

pub(crate) fn default_image_decode(
    space: Option<&ColorSpace>,
    image_mask: bool,
    bits: u8,
    components: usize,
) -> Vec<f64> {
    if image_mask {
        return vec![0.0, 1.0];
    }
    match space {
        Some(ColorSpace::Lab(definition)) => vec![
            0.0,
            100.0,
            definition.range.value[0],
            definition.range.value[1],
            definition.range.value[2],
            definition.range.value[3],
        ],
        Some(ColorSpace::Indexed(_)) => {
            vec![0.0, f64::from(u32::MAX >> (32 - u32::from(bits)))]
        }
        Some(ColorSpace::IccBased(definition)) => definition.range.value.clone(),
        _ => [0.0, 1.0].repeat(components),
    }
}

pub(crate) fn validate_image_dictionary(
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
                InterpretErrorKind::InvalidImageEntry,
            ));
        }
        if !matches!(
            key.as_slice(),
            b"/Type"
                | b"/Subtype"
                | b"/Width"
                | b"/Height"
                | b"/BitsPerComponent"
                | b"/ColorSpace"
                | b"/Decode"
                | b"/ImageMask"
                | b"/Interpolate"
                | b"/SMask"
                | b"/Mask"
                | b"/Matte"
                | b"/Length"
                | b"/Filter"
                | b"/DecodeParms"
                | b"/DL"
                | b"/Name"
                | b"/Intent"
                | b"/ColorTransform"
                | b"/Metadata"
                | b"/ImageName"
        ) {
            return Err(InterpretError::at_span(
                entry.key().span(),
                InterpretErrorKind::UnsupportedImageEntry,
            ));
        }
    }
    Ok(())
}
