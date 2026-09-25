use pdf_bytes::{ByteStore, SourceSpan};
use pdf_content::{
    ContentLimits, FormXObject, Operation, PageResources, ResourceEntry, parse_operations_strict,
};
use pdf_syntax::{Object, ObjectKind, Reference, decode_name};

use crate::color::clamp_unit;
use crate::error::{InterpretError, InterpretErrorKind};
use crate::form::{
    TransparencyGroupMetadata, resolve_soft_mask_dictionary, soft_mask_backdrop_color,
    soft_mask_subtype, soft_mask_transfer, transparency_group_metadata,
    validate_soft_mask_dictionary,
};
use crate::geometry::Matrix;
use crate::graph::{
    InterpretRepair, MarkedContent, MarkedProperties, ObjectScope, RepairKind, SoftMaskPaint,
};
use crate::interpreter::{Interpreter, SavedGraphicsState};
use crate::operand::{resource_boolean, resource_integer, resource_number, unique_resource_entry};
use crate::provenance::Derived;
use crate::state::{
    AppliedExtGState, BlendMode, DashPattern, LineCap, LineJoin, SoftMask, is_standard_blend_mode,
};
use std::sync::Arc;

impl Interpreter {
    pub(super) fn begin_compatibility(
        &mut self,
        operation: &Operation,
    ) -> Result<(), InterpretError> {
        Self::expect_count(operation, 0)?;
        if self.compatibility_depth >= self.limits.max_compatibility_depth {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::CompatibilitySectionLimit,
            ));
        }
        self.compatibility_depth += 1;
        Ok(())
    }

    pub(super) fn end_compatibility(
        &mut self,
        operation: &Operation,
    ) -> Result<(), InterpretError> {
        Self::expect_count(operation, 0)?;
        self.compatibility_depth = self.compatibility_depth.checked_sub(1).ok_or_else(|| {
            InterpretError::at(operation, InterpretErrorKind::CompatibilitySectionMissing)
        })?;
        Ok(())
    }

    pub(super) fn begin_marked_content(
        &mut self,
        operation: &Operation,
        with_properties: bool,
    ) -> Result<(), InterpretError> {
        Self::expect_count(operation, if with_properties { 2 } else { 1 })?;
        let (tag, tag_span) = self.marked_tag(operation)?;
        if self.marks.len() >= self.limits.max_marked_content_depth {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::MarkedContentDepthLimit,
            ));
        }
        let properties = if with_properties {
            Some(self.marked_properties(operation)?)
        } else {
            None
        };
        let (actual_text, written_here) = if with_properties {
            self.actual_text(operation)
                .map_or((None, false), |(text, ours)| (Some(text), ours))
        } else {
            (None, false)
        };
        if tag == b"/OC" {
            let hidden = self.optional_content_hidden(operation, with_properties)?;
            if hidden {
                self.hidden_marks.push(self.marks.len());
            }
        }
        self.marks.push(MarkedContent {
            tag,
            tag_span,
            operator_span: operation.operator_span(),
            properties,
            actual_text,
            written_here,
        });
        Ok(())
    }

    fn optional_content_hidden(
        &self,
        operation: &Operation,
        with_properties: bool,
    ) -> Result<bool, InterpretError> {
        if !with_properties {
            return Ok(false);
        }
        let object = &operation.operands()[1];
        let resources = self.resources.as_ref().ok_or_else(|| {
            InterpretError::at(operation, InterpretErrorKind::ResourceScopeMissing)
        })?;
        let (source, value, reference) = match object.kind() {
            ObjectKind::Name => {
                let name = decode_name(self.operand_source(object), object)
                    .map_err(|_| InterpretError::at(operation, InterpretErrorKind::OperandType))?;
                let resource = resources.property_list(&name).ok_or_else(|| {
                    InterpretError::at(operation, InterpretErrorKind::ResourceNotFound)
                })?;
                (
                    resource.source().clone(),
                    resource.value().clone(),
                    resource.reference(),
                )
            }
            ObjectKind::Dictionary(_) => {
                (self.operand_source(object).clone(), object.clone(), None)
            }
            _ => {
                return Err(InterpretError::at(
                    operation,
                    InterpretErrorKind::InvalidMarkedContentProperties,
                ));
            }
        };
        Self::optional_content_of(operation, resources, &source, &value, reference)
    }

    pub(super) fn optional_content_of(
        operation: &Operation,
        resources: &PageResources,
        source: &ByteStore,
        value: &Object,
        reference: Option<Reference>,
    ) -> Result<bool, InterpretError> {
        let content = resources.optional_content();
        let ObjectKind::Dictionary(entries) = value.kind() else {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidMarkedContentProperties,
            ));
        };
        let subtype = entries
            .iter()
            .find(|entry| entry.key_equals(source, b"/Type"))
            .map(pdf_syntax::DictionaryEntry::value);
        let is_membership = subtype.is_some_and(|value| value.name_equals(source, b"/OCMD"));
        if !is_membership {
            return Ok(reference.is_some_and(|reference| content.group_hidden(reference)));
        }
        if entries.iter().any(|entry| entry.key_equals(source, b"/VE")) {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::UnsupportedOptionalContent,
            ));
        }
        let mut groups = Vec::new();
        if let Some(value) = entries
            .iter()
            .find(|entry| entry.key_equals(source, b"/OCGs"))
            .map(pdf_syntax::DictionaryEntry::value)
        {
            match value.kind() {
                ObjectKind::Reference(reference) => groups.push(*reference),
                ObjectKind::Array(items) => {
                    for item in items {
                        if let ObjectKind::Reference(reference) = item.kind() {
                            groups.push(*reference);
                        } else if !matches!(item.kind(), ObjectKind::Null) {
                            return Err(InterpretError::at(
                                operation,
                                InterpretErrorKind::InvalidMarkedContentProperties,
                            ));
                        }
                    }
                }
                ObjectKind::Null => {}
                _ => {
                    return Err(InterpretError::at(
                        operation,
                        InterpretErrorKind::InvalidMarkedContentProperties,
                    ));
                }
            }
        }
        if groups.is_empty() {
            return Ok(false);
        }
        let policy = entries
            .iter()
            .find(|entry| entry.key_equals(source, b"/P"))
            .map(pdf_syntax::DictionaryEntry::value);
        let visible = |reference: &Reference| !content.group_hidden(*reference);
        let shown = match policy {
            None => groups.iter().any(visible),
            Some(name) if name.name_equals(source, b"/AnyOn") => groups.iter().any(visible),
            Some(name) if name.name_equals(source, b"/AllOn") => groups.iter().all(visible),
            Some(name) if name.name_equals(source, b"/AnyOff") => {
                groups.iter().any(|group| !visible(group))
            }
            Some(name) if name.name_equals(source, b"/AllOff") => {
                groups.iter().all(|group| !visible(group))
            }
            Some(_) => {
                return Err(InterpretError::at(
                    operation,
                    InterpretErrorKind::InvalidMarkedContentProperties,
                ));
            }
        };
        Ok(!shown)
    }

    pub(super) fn end_marked_content(
        &mut self,
        operation: &Operation,
    ) -> Result<(), InterpretError> {
        Self::expect_count(operation, 0)?;
        if self.marks.len() <= self.mark_floor {
            self.repairs.push(InterpretRepair {
                kind: RepairKind::UnmatchedEndMarkedContent,
                operator_span: operation.operator_span(),
            });
            return Ok(());
        }
        self.marks.pop();
        if self.hidden_marks.last() == Some(&self.marks.len()) {
            self.hidden_marks.pop();
        }
        Ok(())
    }

    pub(super) fn marked_point(
        &mut self,
        operation: &Operation,
        with_properties: bool,
    ) -> Result<(), InterpretError> {
        Self::expect_count(operation, if with_properties { 2 } else { 1 })?;
        self.marked_tag(operation)?;
        if with_properties {
            self.marked_properties(operation)?;
        }
        Ok(())
    }

    pub(super) fn marked_tag(
        &self,
        operation: &Operation,
    ) -> Result<(Vec<u8>, SourceSpan), InterpretError> {
        let object = &operation.operands()[0];
        let tag = decode_name(self.operand_source(object), object)
            .map_err(|_| InterpretError::at(operation, InterpretErrorKind::OperandType))?;
        Ok((tag, object.span()))
    }

    fn actual_text(&self, operation: &Operation) -> Option<(String, bool)> {
        let object = &operation.operands()[1];
        let (source, value) = match object.kind() {
            ObjectKind::Dictionary(_) => (self.operand_source(object).clone(), object.clone()),
            ObjectKind::Name => {
                let name = decode_name(self.operand_source(object), object).ok()?;
                let resource = self.resources.as_ref()?.property_list(&name)?;
                (resource.source().clone(), resource.value().clone())
            }
            _ => return None,
        };
        let ObjectKind::Dictionary(entries) = value.kind() else {
            return None;
        };
        let entry = entries
            .iter()
            .find(|entry| entry.key_equals(&source, b"/ActualText"))?;
        let bytes = pdf_syntax::decode_string(&source, entry.value(), 64 * 1024).ok()?;
        let ours = entries
            .iter()
            .any(|entry| entry.key_equals(&source, b"/PanPDF"));
        Some((text_string(&bytes), ours))
    }

    pub(super) fn marked_properties(
        &self,
        operation: &Operation,
    ) -> Result<MarkedProperties, InterpretError> {
        let object = &operation.operands()[1];
        match object.kind() {
            ObjectKind::Dictionary(_) => Ok(MarkedProperties::Inline(object.span())),
            ObjectKind::Name => {
                let name = decode_name(self.operand_source(object), object)
                    .map_err(|_| InterpretError::at(operation, InterpretErrorKind::OperandType))?;
                let resource = self
                    .resources
                    .as_ref()
                    .ok_or_else(|| {
                        InterpretError::at(operation, InterpretErrorKind::ResourceScopeMissing)
                    })?
                    .property_list(&name)
                    .ok_or_else(|| {
                        InterpretError::at(operation, InterpretErrorKind::ResourceNotFound)
                    })?;
                if !matches!(resource.value().kind(), ObjectKind::Dictionary(_)) {
                    return Err(InterpretError::at(
                        operation,
                        InterpretErrorKind::InvalidMarkedContentProperties,
                    ));
                }
                Ok(MarkedProperties::Resource {
                    name,
                    name_span: object.span(),
                    reference: resource.reference(),
                    dictionary_span: resource.value().span(),
                })
            }
            _ => Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidMarkedContentProperties,
            )),
        }
    }

    pub(super) fn save(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        Self::expect_count(operation, 0)?;
        if self.stack.len() >= self.limits.max_graphics_stack_depth {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::GraphicsStackLimit,
            ));
        }
        let opening = (self.invocation_path.is_empty()
            && self.pattern_path.is_empty()
            && self.glyph_depth == 0
            && self.path.segments.is_empty()
            && self.pending_clip.is_none()
            && self.text_matrices.is_none())
        .then_some((self.stream, operation.span(), self.graph.atoms.len()));
        self.stack.push(SavedGraphicsState {
            state: self.state.clone(),
            opening,
        });
        Ok(())
    }

    pub(super) fn restore(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        Self::expect_count(operation, 0)?;
        if self.stack.len() <= self.stack_floor {
            self.repair(operation, crate::graph::RepairKind::UnmatchedRestoreState);
            return Ok(());
        }
        let saved = self.stack.pop().expect("stack length checked");
        if let Some((stream, open, first)) = saved.opening
            && stream == self.stream
            && open.source() == operation.span().source()
            && self.graph.atoms.len() > first
            && self.path.segments.is_empty()
            && self.pending_clip.is_none()
            && self.text_matrices.is_none()
        {
            self.graph.object_scopes.push(ObjectScope {
                stream,
                open,
                close: operation.span(),
                atoms: first..self.graph.atoms.len(),
                ctm: saved.state.ctm.clone(),
                inherited_clips: saved.state.clip_paths.len(),
            });
        }
        self.state = saved.state;
        Ok(())
    }

    pub(super) fn concatenate_matrix(
        &mut self,
        operation: &Operation,
    ) -> Result<(), InterpretError> {
        let values = self.numbers::<6>(operation)?;
        let additional = Matrix {
            a: values[0],
            b: values[1],
            c: values[2],
            d: values[3],
            e: values[4],
            f: values[5],
        };
        self.state.ctm.value = self.state.ctm.value.multiply(additional);
        self.state.ctm.provenance.push(operation.span());
        Ok(())
    }

    pub(super) fn set_line_width(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        let value = self.numbers::<1>(operation)?[0];
        if value < 0.0 {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidValue,
            ));
        }
        self.state.line_width = Derived::assigned(value, operation.span());
        Ok(())
    }

    pub(super) fn set_line_cap(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        let value = self.exact_integer(operation)?;
        let cap = match value {
            0 => LineCap::Butt,
            1 => LineCap::Round,
            2 => LineCap::ProjectingSquare,
            _ => {
                return Err(InterpretError::at(
                    operation,
                    InterpretErrorKind::InvalidValue,
                ));
            }
        };
        self.state.line_cap = Derived::assigned(cap, operation.span());
        Ok(())
    }

    pub(super) fn set_line_join(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        let value = self.exact_integer(operation)?;
        let join = match value {
            0 => LineJoin::Miter,
            1 => LineJoin::Round,
            2 => LineJoin::Bevel,
            _ => {
                return Err(InterpretError::at(
                    operation,
                    InterpretErrorKind::InvalidValue,
                ));
            }
        };
        self.state.line_join = Derived::assigned(join, operation.span());
        Ok(())
    }

    pub(super) fn set_miter_limit(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        let value = self.numbers::<1>(operation)?[0];
        if value < 1.0 {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidValue,
            ));
        }
        self.state.miter_limit = Derived::assigned(value, operation.span());
        Ok(())
    }

    pub(super) fn set_dash(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        Self::expect_count(operation, 2)?;
        let ObjectKind::Array(entries) = operation.operands()[0].kind() else {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::OperandType,
            ));
        };
        let mut array = Vec::with_capacity(entries.len());
        for entry in entries {
            let value = self.number(entry, operation)?;
            if value < 0.0 {
                return Err(InterpretError::at(
                    operation,
                    InterpretErrorKind::InvalidValue,
                ));
            }
            array.push(value);
        }
        if !array.is_empty() && !array.iter().any(|value| *value > 0.0) {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidValue,
            ));
        }
        let phase = self.number(&operation.operands()[1], operation)?;
        self.state.dash = Derived::assigned(DashPattern { array, phase }, operation.span());
        Ok(())
    }

    pub(super) fn set_flatness(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        let value = self.numbers::<1>(operation)?[0];
        if !(0.0..=100.0).contains(&value) {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidValue,
            ));
        }
        self.state.flatness = Derived::assigned(value, operation.span());
        Ok(())
    }

    pub(super) fn set_rendering_intent(
        &mut self,
        operation: &Operation,
    ) -> Result<(), InterpretError> {
        Self::expect_count(operation, 1)?;
        let name = decode_name(
            self.operand_source(&operation.operands()[0]),
            &operation.operands()[0],
        )
        .map_err(|_| InterpretError::at(operation, InterpretErrorKind::OperandType))?;
        self.state.rendering_intent = Derived::assigned(name, operation.span());
        Ok(())
    }

    pub(super) fn apply_ext_gstate(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        Self::expect_count(operation, 1)?;
        let name = decode_name(
            self.operand_source(&operation.operands()[0]),
            &operation.operands()[0],
        )
        .map_err(|_| InterpretError::at(operation, InterpretErrorKind::OperandType))?;
        let resources = self.resources.as_ref().ok_or_else(|| {
            InterpretError::at(operation, InterpretErrorKind::ResourceScopeMissing)
        })?;
        let resource = resources
            .ext_gstate(&name)
            .cloned()
            .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::ResourceNotFound))?;
        let ObjectKind::Dictionary(entries) = resource.value().kind() else {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::ExtGStateNotDictionary,
            ));
        };
        let mut seen = std::collections::HashSet::new();
        let mut soft_mask = None;
        for entry in entries {
            let key = entry.decoded_key(resource.source()).map_err(|_| {
                InterpretError::at(operation, InterpretErrorKind::ResourceSourceFailure)
            })?;
            if !seen.insert(key.clone()) {
                return Err(InterpretError::at(
                    operation,
                    InterpretErrorKind::DuplicateExtGStateEntry,
                ));
            }
            if key == b"/SMask" {
                soft_mask = Some(entry.value());
            } else {
                self.apply_ext_gstate_entry(operation, &resource, &key, entry.value())?;
            }
        }
        self.state.ext_gstate = Some(Derived::assigned_from_resource(
            AppliedExtGState {
                name,
                reference: resource.reference(),
            },
            resource.value().span(),
            operation.span(),
        ));
        if let Some(value) = soft_mask {
            self.apply_soft_mask(operation, &resource, value)?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn apply_ext_gstate_entry(
        &mut self,
        operation: &Operation,
        resource: &ResourceEntry,
        key: &[u8],
        value: &Object,
    ) -> Result<(), InterpretError> {
        macro_rules! assign {
            ($field:ident, $new_value:expr) => {
                self.state.$field =
                    Derived::assigned_from_resource($new_value, value.span(), operation.span())
            };
        }

        match key {
            b"/Type" => {
                let name = decode_name(resource.source(), value).map_err(|_| {
                    InterpretError::at(operation, InterpretErrorKind::InvalidExtGStateEntry)
                })?;
                if name != b"/ExtGState" {
                    return Err(InterpretError::at(
                        operation,
                        InterpretErrorKind::InvalidExtGStateEntry,
                    ));
                }
            }
            b"/LW" => {
                let number = resource_number(operation, resource.source(), value)?;
                if number < 0.0 {
                    return Err(InterpretError::at(
                        operation,
                        InterpretErrorKind::InvalidValue,
                    ));
                }
                assign!(line_width, number);
            }
            b"/LC" => {
                let cap = match resource_integer(operation, resource.source(), value)? {
                    0 => LineCap::Butt,
                    1 => LineCap::Round,
                    2 => LineCap::ProjectingSquare,
                    _ => {
                        return Err(InterpretError::at(
                            operation,
                            InterpretErrorKind::InvalidValue,
                        ));
                    }
                };
                assign!(line_cap, cap);
            }
            b"/LJ" => {
                let join = match resource_integer(operation, resource.source(), value)? {
                    0 => LineJoin::Miter,
                    1 => LineJoin::Round,
                    2 => LineJoin::Bevel,
                    _ => {
                        return Err(InterpretError::at(
                            operation,
                            InterpretErrorKind::InvalidValue,
                        ));
                    }
                };
                assign!(line_join, join);
            }
            b"/ML" => {
                let number = resource_number(operation, resource.source(), value)?;
                if number < 1.0 {
                    return Err(InterpretError::at(
                        operation,
                        InterpretErrorKind::InvalidValue,
                    ));
                }
                assign!(miter_limit, number);
            }
            b"/D" => {
                let ObjectKind::Array(parts) = value.kind() else {
                    return Err(InterpretError::at(
                        operation,
                        InterpretErrorKind::InvalidExtGStateEntry,
                    ));
                };
                if parts.len() != 2 {
                    return Err(InterpretError::at(
                        operation,
                        InterpretErrorKind::InvalidExtGStateEntry,
                    ));
                }
                let ObjectKind::Array(entries) = parts[0].kind() else {
                    return Err(InterpretError::at(
                        operation,
                        InterpretErrorKind::InvalidExtGStateEntry,
                    ));
                };
                let mut array = Vec::with_capacity(entries.len());
                for entry in entries {
                    let number = resource_number(operation, resource.source(), entry)?;
                    if number < 0.0 {
                        return Err(InterpretError::at(
                            operation,
                            InterpretErrorKind::InvalidValue,
                        ));
                    }
                    array.push(number);
                }
                if !array.is_empty() && !array.iter().any(|entry| *entry > 0.0) {
                    return Err(InterpretError::at(
                        operation,
                        InterpretErrorKind::InvalidValue,
                    ));
                }
                let phase = resource_number(operation, resource.source(), &parts[1])?;
                assign!(dash, DashPattern { array, phase });
            }
            b"/RI" => {
                let name = decode_name(resource.source(), value).map_err(|_| {
                    InterpretError::at(operation, InterpretErrorKind::InvalidExtGStateEntry)
                })?;
                assign!(rendering_intent, name);
            }
            b"/OP" => assign!(stroke_overprint, resource_boolean(operation, value)?),
            b"/op" => assign!(fill_overprint, resource_boolean(operation, value)?),
            b"/OPM" => {
                let mode = resource_integer(operation, resource.source(), value)?;
                if !matches!(mode, 0 | 1) {
                    return Err(InterpretError::at(
                        operation,
                        InterpretErrorKind::InvalidValue,
                    ));
                }
                assign!(overprint_mode, mode);
            }
            b"/FL" => {
                let number = resource_number(operation, resource.source(), value)?;
                if !(0.0..=100.0).contains(&number) {
                    return Err(InterpretError::at(
                        operation,
                        InterpretErrorKind::InvalidValue,
                    ));
                }
                assign!(flatness, number);
            }
            b"/SM" => {
                let number = resource_number(operation, resource.source(), value)?;
                if !(0.0..=1.0).contains(&number) {
                    return Err(InterpretError::at(
                        operation,
                        InterpretErrorKind::InvalidValue,
                    ));
                }
                assign!(smoothness, number);
            }
            b"/SA" => assign!(stroke_adjust, resource_boolean(operation, value)?),
            b"/BM" => assign!(
                blend_mode,
                BlendMode {
                    names: Self::resource_blend_modes(operation, resource, value)?,
                }
            ),
            b"/CA" => assign!(
                stroke_alpha,
                clamp_unit(resource_number(operation, resource.source(), value)?)
            ),
            b"/ca" => assign!(
                fill_alpha,
                clamp_unit(resource_number(operation, resource.source(), value)?)
            ),
            b"/AIS" => assign!(alpha_is_shape, resource_boolean(operation, value)?),
            b"/TK" => assign!(text_knockout, resource_boolean(operation, value)?),
            b"/BG2" | b"/UCR2" if value.name_equals(resource.source(), b"/Default") => {}
            _ => self.repair(
                operation,
                crate::graph::RepairKind::ExtGStateEntryIgnored {
                    key_span: value.span(),
                },
            ),
        }
        Ok(())
    }

    pub(super) fn apply_soft_mask(
        &mut self,
        operation: &Operation,
        resource: &ResourceEntry,
        value: &Object,
    ) -> Result<(), InterpretError> {
        if value.name_equals(resource.source(), b"/None") {
            self.state.soft_mask =
                Derived::assigned_from_resource(SoftMask::None, value.span(), operation.span());
            return Ok(());
        }

        let (mask_source, mask, dictionary_reference_span) =
            resolve_soft_mask_dictionary(operation, resource, value)?;
        let ObjectKind::Dictionary(entries) = mask.kind() else {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidSoftMaskEntry,
            ));
        };
        validate_soft_mask_dictionary(entries, &mask_source, operation)?;
        let subtype = soft_mask_subtype(entries, &mask_source, operation)?;
        let group_object = unique_resource_entry(
            entries,
            &mask_source,
            b"/G",
            operation,
            InterpretErrorKind::InvalidSoftMaskEntry,
        )?
        .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::SoftMaskMissingEntry))?;
        let ObjectKind::Reference(group_reference) = group_object.kind() else {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidSoftMaskEntry,
            ));
        };
        let form = resource.form_reference(*group_reference).map_err(|error| {
            InterpretError::at(
                operation,
                InterpretErrorKind::SoftMaskResource(error.kind()),
            )
        })?;
        if self
            .invocation_path
            .iter()
            .any(|invocation| invocation.form == form.reference)
        {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::FormInvocationCycle,
            ));
        }
        if self.invocation_path.len() >= self.limits.max_form_depth {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::FormDepthLimit,
            ));
        }
        if self.form_invocations >= self.limits.max_form_invocations {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::FormInvocationLimit,
            ));
        }
        self.form_invocations += 1;
        let operations = parse_operations_strict(&form.bytes, ContentLimits::default())
            .map_err(|_| InterpretError::at(operation, InterpretErrorKind::FormContent))?;
        let metadata = self.transparency_metadata(&form, operation)?;
        let backdrop_color =
            soft_mask_backdrop_color(operation, resource, &mask_source, entries, &metadata)?;
        let transfer = soft_mask_transfer(entries, &mask_source, operation)?;
        let group = self.capture_transparency_group(operation, &form, &operations, metadata)?;
        let dictionary_span = mask.span();
        let mask = SoftMaskPaint {
            dictionary_span,
            dictionary_reference_span,
            subtype,
            group: Box::new(group),
            backdrop_color,
            transfer,
        };
        let mut provenance = vec![dictionary_span];
        if let Some(span) = dictionary_reference_span {
            provenance.push(span);
        }
        provenance.push(operation.span());
        self.state.soft_mask = Derived {
            value: SoftMask::Dictionary(Arc::new(mask)),
            provenance: provenance.into(),
        };
        Ok(())
    }

    pub(super) fn transparency_metadata(
        &self,
        form: &FormXObject,
        operation: &Operation,
    ) -> Result<TransparencyGroupMetadata, InterpretError> {
        transparency_group_metadata(form, operation, self.resources.as_ref(), self.limits)
    }

    pub(super) fn resource_blend_modes(
        operation: &Operation,
        resource: &ResourceEntry,
        value: &Object,
    ) -> Result<Vec<Vec<u8>>, InterpretError> {
        let candidates: Vec<&Object> = match value.kind() {
            ObjectKind::Name => vec![value],
            ObjectKind::Array(entries) if !entries.is_empty() => entries.iter().collect(),
            _ => {
                return Err(InterpretError::at(
                    operation,
                    InterpretErrorKind::InvalidExtGStateEntry,
                ));
            }
        };
        let mut names = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let name = decode_name(resource.source(), candidate).map_err(|_| {
                InterpretError::at(operation, InterpretErrorKind::InvalidExtGStateEntry)
            })?;
            if !is_standard_blend_mode(&name) {
                return Err(InterpretError::at(
                    operation,
                    InterpretErrorKind::UnsupportedBlendMode,
                ));
            }
            names.push(name);
        }
        Ok(names)
    }
}

fn text_string(bytes: &[u8]) -> String {
    if let Some(units) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = units
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        return char::decode_utf16(units)
            .map(|character| character.unwrap_or('\u{FFFD}'))
            .collect();
    }
    bytes.iter().copied().map(char::from).collect()
}
