use std::io::Cursor;
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_content::{ImageXObject, Operation, ResourceEntry, StringProtection};
use pdf_syntax::{ObjectKind, ObjectParser, ParseLimits, Reference};

use crate::ccitt;
use crate::color::{
    ColorSpace, ColorSpaceParseContext, color_components, parse_color_space_definition,
};
use crate::error::{InterpretError, InterpretErrorKind};
use crate::graph::{PaintAtom, PaintAtomKind, PaintId};
use crate::image::{
    DctColorTransform, DctParameterLocation, ImageSampleShape, ccitt_parameters,
    dct_color_transform, dct_decoder_transform, default_image_decode, image_bits_per_component,
    image_dimension, image_flag, image_matte, image_rendering_intent, jpeg_has_adobe_app14,
    require_addressable_samples, subtype_is, validate_image_dictionary,
};
use crate::image::{ImageMask, ImagePaint};
use crate::interpreter::Interpreter;
use crate::operand::{resource_number_vector, source_exact_integer, unique_resource_entry};
use crate::provenance::Derived;
use crate::state::GraphicsState;

impl Interpreter {
    pub(super) fn paint_image(
        &mut self,
        operation: &Operation,
        resource: &ResourceEntry,
    ) -> Result<(), InterpretError> {
        let reference = resource
            .reference()
            .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::UnsupportedXObject))?;
        let image = resource
            .image(reference, self.limits.max_image_bytes)
            .map_err(|error| {
                InterpretError::at(operation, InterpretErrorKind::ImageResource(error.kind()))
            })?;
        if !subtype_is(&image, b"/Image") {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::UnsupportedXObject,
            ));
        }
        self.record_stream_repairs(operation, &image.repairs);
        let paint = self.image_paint(operation, resource, &image, 0)?;
        let ordinal = self.graph.atoms.len();
        self.emit(PaintAtom {
            id: PaintId {
                page: self.page,
                stream: self.stream,
                operator_span: operation.operator_span(),
                invocation_path: self.invocation_path.clone(),
                pattern_path: self.pattern_path.clone(),
                ordinal,
            },
            kind: PaintAtomKind::Image(Box::new(paint)),
            marks: self.marks.clone(),
        });
        self.paint_atoms += 1;
        if self.paint_atoms > self.limits.max_paint_atoms {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::PaintAtomLimit,
            ));
        }
        Ok(())
    }

    pub(super) fn paint_inline_image(
        &mut self,
        operation: &Operation,
    ) -> Result<(), InterpretError> {
        let Some(inline) = operation.inline_image() else {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::UnsupportedOperator,
            ));
        };
        let source = self.source.clone();
        let resources = self
            .resources
            .as_ref()
            .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::ImageMissingEntry))?;

        let expanded = expand_inline_dictionary(&source, inline, operation)?;
        let derivation = 0x4249_0000_0000_0000_u64 | operation.operator_span().start() as u64;
        let dictionary_source =
            ByteStore::new(SourceId::derived(source.id(), derivation), expanded);
        let dictionary = ObjectParser::new(&dictionary_source, 0, ParseLimits::default())
            .parse_next()
            .ok()
            .flatten()
            .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::InvalidImageEntry))?;
        let ObjectKind::Dictionary(entries) = dictionary.kind() else {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidImageEntry,
            ));
        };
        let encoded = source
            .resolve(inline.data)
            .map_err(|_| InterpretError::at(operation, InterpretErrorKind::InvalidImageEntry))?;
        let decoded = pdf_syntax::decode_image_stream_bytes_recovering(
            &dictionary_source,
            entries.as_slice(),
            encoded,
            inline.data.start(),
            self.limits.max_image_bytes,
        )
        .map_err(|_| InterpretError::at(operation, InterpretErrorKind::InvalidImageEntry))?;

        let entry = resources
            .inline_entry(dictionary_source.clone(), dictionary.clone())
            .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::ImageMissingEntry))?;
        let image = ImageXObject {
            reference: Reference::new(1, 0),
            source: dictionary_source,
            dictionary,
            encoded_data_span: inline.data,
            bytes: decoded.bytes.into(),
            codec: decoded.codec,
            codec_span: decoded.codec_span,
            codec_parameters: decoded.codec_parameters,
            repairs: decoded.repairs,
            strings: StringProtection::Plain,
        };
        self.record_stream_repairs(operation, &image.repairs);
        let paint = self.image_paint(operation, &entry, &image, 0)?;
        let ordinal = self.graph.atoms.len();
        self.emit(PaintAtom {
            id: PaintId {
                page: self.page,
                stream: self.stream,
                operator_span: operation.operator_span(),
                invocation_path: self.invocation_path.clone(),
                pattern_path: self.pattern_path.clone(),
                ordinal,
            },
            kind: PaintAtomKind::Image(Box::new(paint)),
            marks: self.marks.clone(),
        });
        self.paint_atoms += 1;
        if self.paint_atoms > self.limits.max_paint_atoms {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::PaintAtomLimit,
            ));
        }
        Ok(())
    }

    pub(super) fn image_paint(
        &mut self,
        operation: &Operation,
        resource: &ResourceEntry,
        image: &ImageXObject,
        depth: usize,
    ) -> Result<ImagePaint, InterpretError> {
        let source = &image.source;
        let ObjectKind::Dictionary(entries) = image.dictionary.kind() else {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidImageEntry,
            ));
        };
        validate_image_dictionary(entries, source, operation)?;
        let width = image_dimension(entries, source, operation, b"/Width")?;
        let height = image_dimension(entries, source, operation, b"/Height")?;
        if (width.value as usize)
            .checked_mul(height.value as usize)
            .is_none_or(|pixels| pixels > self.limits.max_image_pixels)
        {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::ImageLimit,
            ));
        }
        let image_mask = image_flag(entries, source, operation, b"/ImageMask")?;
        let bits_per_component =
            image_bits_per_component(entries, source, operation, image_mask.value)?;
        let color_space = if image_mask.value {
            None
        } else {
            Some(self.image_color_space(operation, resource, source, image.strings, entries)?)
        };
        let components = color_space
            .as_ref()
            .and_then(|space| color_components(&space.value))
            .unwrap_or(1);
        if components == 0 {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidImageEntry,
            ));
        }
        let (samples, dct_color_transform) = self.image_samples(
            operation,
            resource,
            image,
            entries,
            ImageSampleShape {
                width: width.value,
                height: height.value,
                components,
                bits_per_component: bits_per_component.value,
            },
        )?;
        let decode = Self::image_decode(
            operation,
            source,
            entries,
            color_space.as_ref(),
            image_mask.value,
            bits_per_component.value,
            components,
        )?;
        let interpolate = image_flag(entries, source, operation, b"/Interpolate")?;
        let codec = Self::image_codec(operation, image)?;
        let state = self.image_state(operation, source, entries)?;
        require_addressable_samples(
            operation,
            samples.len(),
            width.value,
            height.value,
            components,
            bits_per_component.value,
        )?;
        let (soft_mask, mask, matte) = self.image_masks(
            operation,
            resource,
            source,
            entries,
            depth,
            components,
            bits_per_component.value,
        )?;
        Ok(ImagePaint {
            reference: image.reference,
            dictionary_span: image.dictionary.span(),
            encoded_data_span: image.encoded_data_span,
            width,
            height,
            bits_per_component,
            codec,
            dct_color_transform,
            color_space,
            image_mask,
            decode,
            interpolate,
            soft_mask,
            mask,
            matte,
            samples,
            state,
        })
    }

    pub(super) fn image_state(
        &self,
        operation: &Operation,
        source: &ByteStore,
        entries: &[pdf_syntax::DictionaryEntry],
    ) -> Result<GraphicsState, InterpretError> {
        let mut state = self.state.clone();
        if let Some(intent) = image_rendering_intent(entries, source, operation)? {
            state.rendering_intent = intent;
        }
        Ok(state)
    }

    fn jbig2_globals(
        &self,
        operation: &Operation,
        resource: &ResourceEntry,
        image: &ImageXObject,
    ) -> Result<Option<Arc<[u8]>>, InterpretError> {
        let Some(parameters) = image.codec_parameters.as_ref() else {
            return Ok(None);
        };
        let ObjectKind::Dictionary(entries) = parameters.kind() else {
            return Ok(None);
        };
        let Some(object) = unique_resource_entry(
            entries,
            &image.source,
            b"/JBIG2Globals",
            operation,
            InterpretErrorKind::InvalidImageEntry,
        )?
        else {
            return Ok(None);
        };
        let ObjectKind::Reference(reference) = object.kind() else {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidImageEntry,
            ));
        };
        let globals = resource
            .image(*reference, self.limits.max_image_bytes)
            .map_err(|error| {
                InterpretError::at(operation, InterpretErrorKind::ImageResource(error.kind()))
            })?;
        Ok(Some(globals.bytes))
    }

    pub(super) fn image_samples(
        &mut self,
        operation: &Operation,
        resource: &ResourceEntry,
        image: &ImageXObject,
        entries: &[pdf_syntax::DictionaryEntry],
        shape: ImageSampleShape,
    ) -> Result<(Arc<[u8]>, Option<DctColorTransform>), InterpretError> {
        match image.codec {
            None => {
                if entries
                    .iter()
                    .any(|entry| entry.key_equals(&image.source, b"/ColorTransform"))
                {
                    return Err(InterpretError::at(
                        operation,
                        InterpretErrorKind::UnsupportedImageEntry,
                    ));
                }
                Ok((Arc::clone(&image.bytes), None))
            }
            Some(pdf_syntax::ImageCodec::Dct) => self.dct_samples(operation, image, entries, shape),
            Some(pdf_syntax::ImageCodec::CcittFax) => {
                let parameters =
                    ccitt_parameters(operation, &image.source, image.codec_parameters.as_ref())?;
                let samples = ccitt::decode(
                    &image.bytes,
                    &parameters,
                    shape.width,
                    shape.height,
                    self.limits.max_image_bytes,
                )
                .map_err(|error| {
                    InterpretError::at(operation, InterpretErrorKind::CcittDecodeFailure(error))
                })?;
                if shape.components != 1 || shape.bits_per_component != 1 {
                    return Err(InterpretError::at(
                        operation,
                        InterpretErrorKind::DctMetadataMismatch,
                    ));
                }
                Ok((Arc::<[u8]>::from(samples), None))
            }
            Some(pdf_syntax::ImageCodec::Jbig2) => {
                let globals = self.jbig2_globals(operation, resource, image)?;
                let samples = crate::jbig2::decode(
                    &image.bytes,
                    globals.as_deref(),
                    shape.width,
                    shape.height,
                    self.limits.max_image_bytes,
                )
                .map_err(|error| {
                    InterpretError::at(operation, InterpretErrorKind::Jbig2DecodeFailure(error))
                })?;
                if shape.components != 1 || shape.bits_per_component != 1 {
                    return Err(InterpretError::at(
                        operation,
                        InterpretErrorKind::DctMetadataMismatch,
                    ));
                }
                Ok((Arc::<[u8]>::from(samples), None))
            }
            Some(pdf_syntax::ImageCodec::Jpx) => {
                Ok((self.jpx_samples(operation, image, shape)?, None))
            }
            Some(codec) => Err(InterpretError::at(
                operation,
                InterpretErrorKind::UnsupportedImageCodec(codec),
            )),
        }
    }

    fn jpx_samples(
        &mut self,
        operation: &Operation,
        image: &ImageXObject,
        shape: ImageSampleShape,
    ) -> Result<Arc<[u8]>, InterpretError> {
        let decoded = jpeg2000::decode_recovering(&image.bytes).map_err(|error| {
            InterpretError::at(operation, InterpretErrorKind::JpxDecodeFailure(error.kind))
        })?;
        if decoded.width != shape.width
            || decoded.height != shape.height
            || decoded.components.len() < shape.components
        {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::JpxMetadataMismatch,
            ));
        }
        let bits = u32::from(shape.bits_per_component);
        if bits == 0 || bits > 16 {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::JpxMetadataMismatch,
            ));
        }
        let maximum = (1u64 << bits) - 1;
        let row_bits = u64::from(shape.width) * shape.components as u64 * u64::from(bits);
        let row_bytes = usize::try_from(row_bits.div_ceil(8))
            .map_err(|_| InterpretError::at(operation, InterpretErrorKind::ImageLimit))?;
        let total = row_bytes
            .checked_mul(shape.height as usize)
            .filter(|bytes| *bytes <= self.limits.max_image_bytes)
            .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::ImageLimit))?;

        let mut samples = vec![0u8; total];
        for y in 0..shape.height as usize {
            let mut bit = 0u64;
            let row = y * row_bytes;
            for x in 0..shape.width as usize {
                for component in decoded.components.iter().take(shape.components) {
                    let cx = x * component.width as usize / (shape.width as usize).max(1);
                    let cy = y * component.height as usize / (shape.height as usize).max(1);
                    let raw = component
                        .samples
                        .get(cy * component.width as usize + cx)
                        .copied()
                        .unwrap_or(0);
                    let depth_maximum = (1i64 << component.depth) - 1;
                    let shifted = if component.signed {
                        i64::from(raw) + (1i64 << (component.depth - 1))
                    } else {
                        i64::from(raw)
                    };
                    let clamped = u64::try_from(shifted.clamp(0, depth_maximum)).unwrap_or(0);
                    let depth_maximum = u64::try_from(depth_maximum).unwrap_or(1);
                    let value = if depth_maximum == maximum {
                        clamped
                    } else {
                        (clamped * maximum + depth_maximum / 2) / depth_maximum
                    };
                    write_bits(&mut samples[row..], bit, bits, value);
                    bit += u64::from(bits);
                }
            }
        }
        for repair in &decoded.repairs {
            self.repairs.push(crate::InterpretRepair {
                kind: crate::graph::RepairKind::JpxCodestream { repair: *repair },
                operator_span: operation.operator_span(),
            });
        }
        Ok(Arc::<[u8]>::from(samples))
    }

    fn dct_samples(
        &self,
        operation: &Operation,
        image: &ImageXObject,
        entries: &[pdf_syntax::DictionaryEntry],
        shape: ImageSampleShape,
    ) -> Result<(Arc<[u8]>, Option<DctColorTransform>), InterpretError> {
        let transform = dct_color_transform(
            operation,
            &image.source,
            image.codec_parameters.as_ref(),
            entries,
            shape.components,
        )?;
        let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(image.bytes.as_ref()));
        decoder.set_max_decoding_buffer_size(self.limits.max_image_bytes);
        if !jpeg_has_adobe_app14(&image.bytes)
            && let Some(transform) = transform.as_ref()
            && transform.location == DctParameterLocation::DecodeParameters
        {
            decoder.set_color_transform(dct_decoder_transform(
                transform.value.value,
                shape.components,
                operation,
            )?);
        }
        let samples = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| decoder.decode()))
            .map_err(|_| InterpretError::at(operation, InterpretErrorKind::DctDecodeFailure))?
            .map_err(|_| InterpretError::at(operation, InterpretErrorKind::DctDecodeFailure))?;
        let info = decoder
            .info()
            .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::DctDecodeFailure))?;
        let (decoded_components, decoded_bits) = match info.pixel_format {
            jpeg_decoder::PixelFormat::L8 => (1, 8),
            jpeg_decoder::PixelFormat::RGB24 => (3, 8),
            jpeg_decoder::PixelFormat::CMYK32 => (4, 8),
            jpeg_decoder::PixelFormat::L16 => {
                return Err(InterpretError::at(
                    operation,
                    InterpretErrorKind::UnsupportedDctPrecision,
                ));
            }
        };
        if u32::from(info.width) != shape.width
            || u32::from(info.height) != shape.height
            || decoded_components != shape.components
            || decoded_bits != shape.bits_per_component
        {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::DctMetadataMismatch,
            ));
        }
        let samples = if decoded_components == 4 && jpeg_has_adobe_app14(&image.bytes) {
            samples.iter().map(|byte| 255 - byte).collect::<Vec<u8>>()
        } else {
            samples
        };
        Ok((Arc::<[u8]>::from(samples), transform))
    }

    pub(super) fn image_color_space(
        &self,
        operation: &Operation,
        resource: &ResourceEntry,
        source: &ByteStore,
        strings: StringProtection,
        entries: &[pdf_syntax::DictionaryEntry],
    ) -> Result<Derived<ColorSpace>, InterpretError> {
        let object = unique_resource_entry(
            entries,
            source,
            b"/ColorSpace",
            operation,
            InterpretErrorKind::InvalidImageEntry,
        )?
        .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::ImageMissingEntry))?;
        let resolved = match object.kind() {
            ObjectKind::Reference(reference) => Some(
                resource
                    .resolve_protected_object(*reference)
                    .map_err(|error| {
                        InterpretError::at(
                            operation,
                            InterpretErrorKind::ImageResource(error.kind()),
                        )
                    })?,
            ),
            _ => None,
        };
        let (definition_source, definition_strings, definition, provenance) =
            match resolved.as_ref() {
                Some((definition_source, definition, definition_strings)) => (
                    definition_source,
                    *definition_strings,
                    definition,
                    vec![object.span(), definition.span()],
                ),
                None => (source, strings, object, vec![object.span()]),
            };
        let load_icc = |reference, limit| resource.icc_profile(reference, limit);
        let load_indexed = |reference, limit| resource.indexed_lookup(reference, limit);
        let load_function = |reference, limit| resource.function(reference, limit);
        let load_object = |reference| resource.resolve_protected_object(reference);
        let string_plaintext = |strings, bytes| resource.string_plaintext(strings, bytes);
        let context = ColorSpaceParseContext {
            resources: self.resources.as_ref(),
            load_icc: &load_icc,
            load_indexed: &load_indexed,
            load_function: &load_function,
            load_object: &load_object,
            string_plaintext: &string_plaintext,
            limits: self.limits,
        };
        let space = parse_color_space_definition(
            operation,
            definition_source,
            definition_strings,
            definition,
            InterpretErrorKind::UnsupportedColorSpace,
            &context,
            0,
        )?;
        if matches!(space, ColorSpace::Pattern(_)) {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::UnsupportedColorSpace,
            ));
        }
        if let Some((substitute, span)) = self.default_color_space(operation, &space)? {
            let mut provenance = provenance;
            provenance.push(span);
            return Ok(Derived {
                value: substitute,
                provenance: provenance.into(),
            });
        }
        Ok(Derived {
            value: space,
            provenance: provenance.into(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn image_decode(
        operation: &Operation,
        source: &ByteStore,
        entries: &[pdf_syntax::DictionaryEntry],
        color_space: Option<&Derived<ColorSpace>>,
        image_mask: bool,
        bits: u8,
        components: usize,
    ) -> Result<Derived<Vec<f64>>, InterpretError> {
        let object = unique_resource_entry(
            entries,
            source,
            b"/Decode",
            operation,
            InterpretErrorKind::InvalidImageEntry,
        )?;
        let Some(object) = object else {
            return Ok(Derived::initial(default_image_decode(
                color_space.map(|space| &space.value),
                image_mask,
                bits,
                components,
            )));
        };
        let values = resource_number_vector(
            operation,
            source,
            object,
            InterpretErrorKind::InvalidImageEntry,
        )?;
        if values.len() != components * 2 {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidImageEntry,
            ));
        }
        Ok(Derived::assigned(values, object.span()))
    }

    pub(super) fn image_soft_mask(
        &mut self,
        operation: &Operation,
        resource: &ResourceEntry,
        source: &ByteStore,
        entries: &[pdf_syntax::DictionaryEntry],
        depth: usize,
        base_components: usize,
    ) -> Result<Option<Box<ImagePaint>>, InterpretError> {
        let Some(object) = unique_resource_entry(
            entries,
            source,
            b"/SMask",
            operation,
            InterpretErrorKind::InvalidImageEntry,
        )?
        else {
            return Ok(None);
        };
        if depth > 0 {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::UnsupportedImageEntry,
            ));
        }
        let ObjectKind::Reference(reference) = object.kind() else {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidImageEntry,
            ));
        };
        let mask = resource
            .image(*reference, self.limits.max_image_bytes)
            .map_err(|error| {
                InterpretError::at(operation, InterpretErrorKind::ImageResource(error.kind()))
            })?;
        let paint = self.image_paint(operation, resource, &mask, depth + 1)?;
        if paint.image_mask.value || paint.components() != 1 {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidImageEntry,
            ));
        }
        if paint
            .matte
            .as_ref()
            .is_some_and(|matte| matte.value.len() != base_components)
        {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidImageEntry,
            ));
        }
        Ok(Some(Box::new(paint)))
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn image_stencil_mask(
        &mut self,
        operation: &Operation,
        resource: &ResourceEntry,
        source: &ByteStore,
        entries: &[pdf_syntax::DictionaryEntry],
        depth: usize,
        components: usize,
        bits_per_component: u8,
    ) -> Result<Option<ImageMask>, InterpretError> {
        let Some(object) = unique_resource_entry(
            entries,
            source,
            b"/Mask",
            operation,
            InterpretErrorKind::InvalidImageEntry,
        )?
        else {
            return Ok(None);
        };
        if depth > 0 {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::UnsupportedImageEntry,
            ));
        }
        match object.kind() {
            ObjectKind::Reference(reference) => {
                let mask = resource
                    .image(*reference, self.limits.max_image_bytes)
                    .map_err(|error| {
                        InterpretError::at(
                            operation,
                            InterpretErrorKind::ImageResource(error.kind()),
                        )
                    })?;
                let paint = self.image_paint(operation, resource, &mask, depth + 1)?;
                if !paint.image_mask.value {
                    return Err(InterpretError::at(
                        operation,
                        InterpretErrorKind::InvalidImageEntry,
                    ));
                }
                Ok(Some(ImageMask::Stencil(Box::new(paint))))
            }
            ObjectKind::Array(items) => {
                if items.len() != components * 2 {
                    return Err(InterpretError::at(
                        operation,
                        InterpretErrorKind::InvalidImageEntry,
                    ));
                }
                let ceiling = u32::MAX >> (32 - u32::from(bits_per_component));
                let mut ranges = Vec::with_capacity(items.len());
                for item in items {
                    let value = source_exact_integer(
                        operation,
                        source,
                        item,
                        InterpretErrorKind::InvalidImageEntry,
                    )?;
                    let value = u32::try_from(value)
                        .ok()
                        .filter(|raw| *raw <= ceiling)
                        .ok_or_else(|| {
                            InterpretError::at(operation, InterpretErrorKind::InvalidImageEntry)
                        })?;
                    ranges.push(value);
                }
                if ranges.chunks_exact(2).any(|pair| pair[0] > pair[1]) {
                    return Err(InterpretError::at(
                        operation,
                        InterpretErrorKind::InvalidImageEntry,
                    ));
                }
                Ok(Some(ImageMask::ColorKey(Derived::assigned(
                    ranges,
                    object.span(),
                ))))
            }
            _ => Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidImageEntry,
            )),
        }
    }

    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    fn image_masks(
        &mut self,
        operation: &Operation,
        resource: &ResourceEntry,
        source: &ByteStore,
        entries: &[pdf_syntax::DictionaryEntry],
        depth: usize,
        components: usize,
        bits_per_component: u8,
    ) -> Result<
        (
            Option<Box<ImagePaint>>,
            Option<ImageMask>,
            Option<Derived<Vec<f64>>>,
        ),
        InterpretError,
    > {
        let soft_mask =
            self.image_soft_mask(operation, resource, source, entries, depth, components)?;
        let mask = self.image_stencil_mask(
            operation,
            resource,
            source,
            entries,
            depth,
            components,
            bits_per_component,
        )?;
        if soft_mask.is_some() && mask.is_some() {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidImageEntry,
            ));
        }
        let matte = image_matte(entries, source, operation, depth, components)?;
        Ok((soft_mask, mask, matte))
    }

    fn image_codec(
        operation: &Operation,
        image: &ImageXObject,
    ) -> Result<Option<Derived<pdf_syntax::ImageCodec>>, InterpretError> {
        match (image.codec, image.codec_span) {
            (Some(codec), Some(span)) => Ok(Some(Derived::assigned(codec, span))),
            (None, None) => Ok(None),
            _ => Err(InterpretError::at(
                operation,
                InterpretErrorKind::SourceSpanFailure,
            )),
        }
    }
}

fn abbreviation(name: &[u8]) -> Option<&'static [u8]> {
    Some(match name {
        b"/BPC" => b"/BitsPerComponent",
        b"/CS" => b"/ColorSpace",
        b"/D" => b"/Decode",
        b"/DP" => b"/DecodeParms",
        b"/F" => b"/Filter",
        b"/H" => b"/Height",
        b"/IM" => b"/ImageMask",
        b"/I" => b"/Interpolate",
        b"/L" => b"/Length",
        b"/W" => b"/Width",
        _ => return None,
    })
}

fn value_abbreviation(name: &[u8]) -> Option<&'static [u8]> {
    Some(match name {
        b"/G" => b"/DeviceGray",
        b"/RGB" => b"/DeviceRGB",
        b"/CMYK" => b"/DeviceCMYK",
        b"/I" => b"/Indexed",
        b"/AHx" => b"/ASCIIHexDecode",
        b"/A85" => b"/ASCII85Decode",
        b"/LZW" => b"/LZWDecode",
        b"/Fl" => b"/FlateDecode",
        b"/RL" => b"/RunLengthDecode",
        b"/CCF" => b"/CCITTFaxDecode",
        b"/DCT" => b"/DCTDecode",
        _ => return None,
    })
}

fn expand_inline_dictionary(
    source: &ByteStore,
    inline: &pdf_content::InlineImage,
    operation: &Operation,
) -> Result<Vec<u8>, InterpretError> {
    let mut out = b"<<".to_vec();
    for (key, value) in &inline.entries {
        let key_bytes = source
            .resolve(key.span())
            .map_err(|_| InterpretError::at(operation, InterpretErrorKind::InvalidImageEntry))?;
        let value_bytes = source
            .resolve(value.span())
            .map_err(|_| InterpretError::at(operation, InterpretErrorKind::InvalidImageEntry))?;
        out.push(b' ');
        out.extend_from_slice(abbreviation(key_bytes).unwrap_or(key_bytes));
        out.push(b' ');
        out.extend_from_slice(value_abbreviation(value_bytes).unwrap_or(value_bytes));
    }
    out.extend_from_slice(b" >>");
    Ok(out)
}

fn write_bits(row: &mut [u8], offset: u64, count: u32, value: u64) {
    for index in 0..u64::from(count) {
        let bit = (value >> (u64::from(count) - 1 - index)) & 1;
        if bit == 0 {
            continue;
        }
        let position = offset + index;
        let byte = (position / 8) as usize;
        if let Some(slot) = row.get_mut(byte) {
            *slot |= 0x80 >> (position % 8);
        }
    }
}
