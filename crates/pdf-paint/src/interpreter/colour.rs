use pdf_content::{
    ContentLimits, Operation, ResourceEntry, TilingPattern, parse_operations_strict,
};

use crate::color::{Color, ColorSpace};
use crate::color::{
    ColorSpaceParseContext, clamp_unit, color_components, palette_index,
    parse_color_space_definition, predefined_color_space,
};
use crate::error::{InterpretError, InterpretErrorKind};
use crate::function::clamp_to_domain;
use crate::graph::{PaintGraph, PatternInvocation, ShadingPatternPaint, TilingPatternPaint};
use crate::interpreter::{ColourState, Interpreter};
use crate::pattern::{pattern_metadata, shading_pattern_matrix};
use crate::provenance::Derived;
use crate::shading::parse_shading;
use std::sync::Arc;

use pdf_bytes::SourceSpan;
use pdf_syntax::decode_name;

impl Interpreter {
    pub(super) fn colour_state(&self) -> ColourState {
        ColourState {
            fill: self.state.fill_color.clone(),
            stroke: self.state.stroke_color.clone(),
            fill_space: self.state.fill_color_space.clone(),
            stroke_space: self.state.stroke_color_space.clone(),
        }
    }

    pub(super) fn restore_colour_state(&mut self, colour: ColourState) {
        self.state.fill_color = colour.fill;
        self.state.stroke_color = colour.stroke;
        self.state.fill_color_space = colour.fill_space;
        self.state.stroke_color_space = colour.stroke_space;
    }

    pub(super) fn default_color_space(
        &self,
        operation: &Operation,
        device: &ColorSpace,
    ) -> Result<Option<(ColorSpace, SourceSpan)>, InterpretError> {
        let key: &[u8] = match device {
            ColorSpace::DeviceGray => b"/DefaultGray",
            ColorSpace::DeviceRgb => b"/DefaultRGB",
            ColorSpace::DeviceCmyk => b"/DefaultCMYK",
            _ => return Ok(None),
        };
        let Some(resources) = self.resources.as_ref() else {
            return Ok(None);
        };
        let Some(resource) = resources.color_space(key) else {
            return Ok(None);
        };
        let load_icc = |reference, limit| resource.icc_profile(reference, limit);
        let load_indexed = |reference, limit| resource.indexed_lookup(reference, limit);
        let load_function = |reference, limit| resource.function(reference, limit);
        let load_object = |reference| resource.resolve_protected_object(reference);
        let string_plaintext = |strings, bytes| resource.string_plaintext(strings, bytes);
        let context = ColorSpaceParseContext {
            resources: Some(resources),
            load_icc: &load_icc,
            load_indexed: &load_indexed,
            load_function: &load_function,
            load_object: &load_object,
            string_plaintext: &string_plaintext,
            limits: self.limits,
        };
        let space = parse_color_space_definition(
            operation,
            resource.source(),
            resource.strings(),
            resource.value(),
            InterpretErrorKind::UnsupportedColorSpace,
            &context,
            0,
        )?;
        if matches!(
            space,
            ColorSpace::DeviceGray | ColorSpace::DeviceRgb | ColorSpace::DeviceCmyk
        ) {
            return Ok(None);
        }
        if color_components(&space) != color_components(device) {
            return Ok(None);
        }
        Ok(Some((space, resource.value().span())))
    }

    pub(super) fn set_gray(
        &mut self,
        operation: &Operation,
        stroke: bool,
    ) -> Result<(), InterpretError> {
        let [gray] = self.numbers::<1>(operation)?;
        self.assign_color_space(operation, stroke, ColorSpace::DeviceGray, None);
        self.set_color(operation, stroke, Color::DeviceGray(clamp_unit(gray)));
        Ok(())
    }

    pub(super) fn set_rgb(
        &mut self,
        operation: &Operation,
        stroke: bool,
    ) -> Result<(), InterpretError> {
        let [red, green, blue] = self.numbers::<3>(operation)?;
        self.assign_color_space(operation, stroke, ColorSpace::DeviceRgb, None);
        self.set_color(
            operation,
            stroke,
            Color::DeviceRgb(clamp_unit(red), clamp_unit(green), clamp_unit(blue)),
        );
        Ok(())
    }

    pub(super) fn set_cmyk(
        &mut self,
        operation: &Operation,
        stroke: bool,
    ) -> Result<(), InterpretError> {
        let [cyan, magenta, yellow, black] = self.numbers::<4>(operation)?;
        self.assign_color_space(operation, stroke, ColorSpace::DeviceCmyk, None);
        self.set_color(
            operation,
            stroke,
            Color::DeviceCmyk(
                clamp_unit(cyan),
                clamp_unit(magenta),
                clamp_unit(yellow),
                clamp_unit(black),
            ),
        );
        Ok(())
    }

    pub(super) fn set_color_space(
        &mut self,
        operation: &Operation,
        stroke: bool,
    ) -> Result<(), InterpretError> {
        Self::expect_count(operation, 1)?;
        let name = decode_name(
            self.operand_source(&operation.operands()[0]),
            &operation.operands()[0],
        )
        .map_err(|_| InterpretError::at(operation, InterpretErrorKind::OperandType))?;
        let (space, resource_span) = if let Some(space) = predefined_color_space(&name) {
            (space, None)
        } else {
            let resources = self.resources.as_ref().ok_or_else(|| {
                InterpretError::at(operation, InterpretErrorKind::ResourceScopeMissing)
            })?;
            let resource = resources.color_space(&name).ok_or_else(|| {
                InterpretError::at(operation, InterpretErrorKind::ResourceNotFound)
            })?;
            let load_icc = |reference, limit| resource.icc_profile(reference, limit);
            let load_indexed = |reference, limit| resource.indexed_lookup(reference, limit);
            let load_function = |reference, limit| resource.function(reference, limit);
            let load_object = |reference| resource.resolve_protected_object(reference);
            let string_plaintext = |strings, bytes| resource.string_plaintext(strings, bytes);
            let context = ColorSpaceParseContext {
                resources: Some(resources),
                load_icc: &load_icc,
                load_indexed: &load_indexed,
                load_function: &load_function,
                load_object: &load_object,
                string_plaintext: &string_plaintext,
                limits: self.limits,
            };
            let space = parse_color_space_definition(
                operation,
                resource.source(),
                resource.strings(),
                resource.value(),
                InterpretErrorKind::UnsupportedColorSpace,
                &context,
                0,
            )?;
            (space, Some(resource.value().span()))
        };
        let default = match &space {
            ColorSpace::DeviceGray => Color::DeviceGray(0.0),
            ColorSpace::DeviceRgb => Color::DeviceRgb(0.0, 0.0, 0.0),
            ColorSpace::DeviceCmyk => Color::DeviceCmyk(0.0, 0.0, 0.0, 0.0),
            ColorSpace::CalGray(_) => Color::CalGray(0.0),
            ColorSpace::CalRgb(_) => Color::CalRgb(0.0, 0.0, 0.0),
            ColorSpace::Lab(space) => Color::Lab(
                0.0,
                0.0_f64.clamp(space.range.value[0], space.range.value[1]),
                0.0_f64.clamp(space.range.value[2], space.range.value[3]),
            ),
            ColorSpace::IccBased(space) => Color::IccBased(
                space
                    .range
                    .value
                    .chunks_exact(2)
                    .map(|pair| 0.0_f64.clamp(pair[0], pair[1]))
                    .collect(),
            ),
            ColorSpace::Indexed(_) => Color::Indexed(0),
            ColorSpace::Separation(space) => {
                Color::Separation(clamp_to_domain(1.0, &space.tint_transform.value, 0))
            }
            ColorSpace::DeviceN(space) => Color::DeviceN(
                (0..space.colorants.value.len())
                    .map(|axis| clamp_to_domain(1.0, &space.tint_transform.value, axis))
                    .collect(),
            ),
            ColorSpace::Pattern(_) => Color::PatternUnspecified,
        };
        self.assign_color_space(operation, stroke, space, resource_span);
        self.assign_color(operation, stroke, default, resource_span);
        Ok(())
    }

    pub(super) fn set_selected_color(
        &mut self,
        operation: &Operation,
        stroke: bool,
    ) -> Result<(), InterpretError> {
        let space = if stroke {
            self.state.stroke_color_space.value.clone()
        } else {
            self.state.fill_color_space.value.clone()
        };
        if let ColorSpace::Pattern(base) = &space {
            return self.set_pattern_color(operation, stroke, base.as_deref());
        }
        let Some(expected) = color_components(&space) else {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::UnsupportedColorSpace,
            ));
        };
        let actual = operation.operands().len();
        if actual != expected {
            self.repair(
                operation,
                crate::graph::RepairKind::ColourOperandCount { expected, actual },
            );
        }
        if actual < expected {
            return Ok(());
        }
        let colour = self.colour_in_space(operation, &space, &operation.operands()[..expected])?;
        self.set_color(operation, stroke, colour);
        Ok(())
    }

    fn colour_in_space(
        &self,
        operation: &Operation,
        base: &ColorSpace,
        operands: &[pdf_syntax::Object],
    ) -> Result<Color, InterpretError> {
        let component = |index: usize| -> Result<f64, InterpretError> {
            match operands.get(index) {
                Some(operand) => self.number(operand, operation),
                None => Ok(0.0),
            }
        };
        Ok(match base {
            ColorSpace::DeviceGray => Color::DeviceGray(clamp_unit(component(0)?)),
            ColorSpace::CalGray(_) => Color::CalGray(clamp_unit(component(0)?)),
            ColorSpace::DeviceRgb => Color::DeviceRgb(
                clamp_unit(component(0)?),
                clamp_unit(component(1)?),
                clamp_unit(component(2)?),
            ),
            ColorSpace::CalRgb(_) => Color::CalRgb(
                clamp_unit(component(0)?),
                clamp_unit(component(1)?),
                clamp_unit(component(2)?),
            ),
            ColorSpace::DeviceCmyk => Color::DeviceCmyk(
                clamp_unit(component(0)?),
                clamp_unit(component(1)?),
                clamp_unit(component(2)?),
                clamp_unit(component(3)?),
            ),
            ColorSpace::Lab(definition) => Color::Lab(
                component(0)?.clamp(0.0, 100.0),
                component(1)?.clamp(definition.range.value[0], definition.range.value[1]),
                component(2)?.clamp(definition.range.value[2], definition.range.value[3]),
            ),
            ColorSpace::IccBased(definition) => {
                let mut components = Vec::with_capacity(definition.components.value);
                for index in 0..definition.components.value {
                    let value = component(index)?;
                    components.push(value.clamp(
                        definition.range.value[index * 2],
                        definition.range.value[index * 2 + 1],
                    ));
                }
                Color::IccBased(components)
            }
            ColorSpace::Indexed(definition) => {
                Color::Indexed(palette_index(component(0)?, definition.hival.value))
            }
            ColorSpace::Separation(definition) => Color::Separation(clamp_to_domain(
                component(0)?,
                &definition.tint_transform.value,
                0,
            )),
            ColorSpace::DeviceN(definition) => {
                let mut tints = Vec::with_capacity(definition.colorants.value.len());
                for axis in 0..definition.colorants.value.len() {
                    let value = component(axis)?;
                    tints.push(clamp_to_domain(
                        value,
                        &definition.tint_transform.value,
                        axis,
                    ));
                }
                Color::DeviceN(tints)
            }
            ColorSpace::Pattern(_) => Color::PatternUnspecified,
        })
    }

    pub(super) fn set_pattern_color(
        &mut self,
        operation: &Operation,
        stroke: bool,
        base: Option<&ColorSpace>,
    ) -> Result<(), InterpretError> {
        let Some((name_operand, colour_operands)) = operation.operands().split_last() else {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::OperandCount {
                    expected: 1,
                    actual: 0,
                },
            ));
        };
        if base.is_none() && !colour_operands.is_empty() {
            self.repair(
                operation,
                crate::graph::RepairKind::UncolouredPatternOperandCount {
                    expected: 0,
                    actual: colour_operands.len(),
                },
            );
        }
        if let Some(base) = base
            && let Some(expected) = color_components(base)
            && colour_operands.len() != expected
        {
            self.repair(
                operation,
                crate::graph::RepairKind::UncolouredPatternOperandCount {
                    expected,
                    actual: colour_operands.len(),
                },
            );
        }
        let name = decode_name(self.operand_source(name_operand), name_operand)
            .map_err(|_| InterpretError::at(operation, InterpretErrorKind::OperandType))?;
        let uncoloured = match base {
            Some(base) => Some((
                base.clone(),
                self.colour_in_space(operation, base, colour_operands)?,
            )),
            None => None,
        };
        let resource = self
            .resources
            .as_ref()
            .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::ResourceScopeMissing))?
            .pattern(&name)
            .cloned()
            .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::ResourceNotFound))?;
        let kind = resource.pattern_type().map_err(|error| {
            InterpretError::at(operation, InterpretErrorKind::PatternResource(error.kind()))
        })?;
        if kind == 2 {
            return self.set_shading_pattern_color(operation, stroke, &resource);
        }
        let pattern = resource.tiling_pattern().map_err(|error| {
            InterpretError::at(operation, InterpretErrorKind::PatternResource(error.kind()))
        })?;
        let metadata = pattern_metadata(&pattern, operation)?;
        let uncoloured = if metadata.paint_type.value == 2 {
            let Some(uncoloured) = uncoloured else {
                return Err(InterpretError::at(
                    operation,
                    InterpretErrorKind::UncoloredPatternUnsupported,
                ));
            };
            Some(uncoloured)
        } else {
            None
        };
        let graph = self.interpret_pattern(operation, &pattern, uncoloured)?;
        let paint = TilingPatternPaint {
            reference: pattern.reference,
            dictionary_span: pattern.dictionary.span(),
            paint_type: metadata.paint_type,
            bbox: metadata.bbox,
            bbox_span: metadata.bbox_span,
            x_step: metadata.x_step,
            y_step: metadata.y_step,
            matrix: metadata.matrix,
            tiling_type: metadata.tiling_type,
            graph,
        };
        self.assign_color(
            operation,
            stroke,
            Color::TilingPattern(Arc::new(paint)),
            Some(pattern.dictionary.span()),
        );
        Ok(())
    }

    pub(super) fn interpret_pattern(
        &mut self,
        invocation: &Operation,
        pattern: &TilingPattern,
        uncoloured: Option<(ColorSpace, Color)>,
    ) -> Result<PaintGraph, InterpretError> {
        if self
            .pattern_path
            .iter()
            .any(|entry| entry.pattern == pattern.reference)
        {
            return Err(InterpretError::at(
                invocation,
                InterpretErrorKind::PatternInvocationCycle,
            ));
        }
        if self.pattern_path.len() >= self.limits.max_pattern_depth {
            return Err(InterpretError::at(
                invocation,
                InterpretErrorKind::PatternDepthLimit,
            ));
        }
        if self.pattern_invocations >= self.limits.max_pattern_invocations {
            return Err(InterpretError::at(
                invocation,
                InterpretErrorKind::PatternInvocationLimit,
            ));
        }
        self.pattern_invocations += 1;
        let operations = parse_operations_strict(&pattern.bytes, ContentLimits::default())
            .map_err(|_| InterpretError::at(invocation, InterpretErrorKind::PatternContent))?;

        let parent_source = std::mem::replace(&mut self.source, pattern.bytes.clone());
        let parent_stream = std::mem::replace(&mut self.stream, pattern.reference);
        let parent_resources = self.resources.replace(pattern.resources.clone());
        let parent_state = std::mem::take(&mut self.state);
        let parent_colour_fixed = self.colour_is_fixed;
        if let Some((space, colour)) = uncoloured {
            let space = Derived::assigned(space, invocation.span());
            let colour = Derived::assigned(colour, invocation.span());
            self.state.fill_color_space = space.clone();
            self.state.stroke_color_space = space;
            self.state.fill_color = colour.clone();
            self.state.stroke_color = colour;
            self.colour_is_fixed = true;
        }
        let parent_path = std::mem::take(&mut self.path);
        let parent_current_point = self.current_point.take();
        let parent_subpath_start = self.subpath_start.take();
        let parent_clips = self.take_clip_accumulators();
        let parent_text_matrices = self.text_matrices.take();
        let parent_stack_floor = self.stack_floor;
        let parent_stack_len = self.stack.len();
        let parent_mark_floor = self.mark_floor;
        let parent_graph = std::mem::take(&mut self.graph);
        let parent_compatibility_depth = std::mem::take(&mut self.compatibility_depth);
        self.stack_floor = parent_stack_len;
        self.mark_floor = self.marks.len();
        self.pattern_path.push(PatternInvocation {
            pattern: pattern.reference,
            operator_span: invocation.operator_span(),
        });

        let result = (|| {
            let mut last_operator = Some(invocation.operator_span());
            for operation in &operations {
                self.apply(operation)?;
                last_operator = Some(operation.operator_span());
            }
            self.close_unbalanced_state(last_operator);
            if self.marks.len() != self.mark_floor {
                return Err(InterpretError::new(
                    last_operator,
                    InterpretErrorKind::UnbalancedMarkedContent {
                        depth: self.marks.len() - self.mark_floor,
                    },
                ));
            }
            if self.compatibility_depth != 0 {
                return Err(InterpretError::new(
                    last_operator,
                    InterpretErrorKind::UnbalancedCompatibilitySection {
                        depth: self.compatibility_depth,
                    },
                ));
            }
            Ok(std::mem::take(&mut self.graph))
        })();

        self.pattern_path.pop();
        self.stack.truncate(parent_stack_len);
        self.marks.truncate(parent_mark_floor);
        self.stack_floor = parent_stack_floor;
        self.mark_floor = parent_mark_floor;
        self.compatibility_depth = parent_compatibility_depth;
        self.colour_is_fixed = parent_colour_fixed;
        self.source = parent_source;
        self.stream = parent_stream;
        self.resources = parent_resources;
        self.state = parent_state;
        self.path = parent_path;
        self.current_point = parent_current_point;
        self.subpath_start = parent_subpath_start;
        self.restore_clip_accumulators(parent_clips);
        self.text_matrices = parent_text_matrices;
        self.graph = parent_graph;
        result
    }

    pub(super) fn assign_color_space(
        &mut self,
        operation: &Operation,
        stroke: bool,
        space: ColorSpace,
        resource_span: Option<SourceSpan>,
    ) {
        let derived = if let Some(span) = resource_span {
            Derived::assigned_from_resource(space, span, operation.span())
        } else {
            Derived::assigned(space, operation.span())
        };
        if stroke {
            self.state.stroke_color_space = derived;
        } else {
            self.state.fill_color_space = derived;
        }
    }

    pub(super) fn set_color(&mut self, operation: &Operation, stroke: bool, color: Color) {
        self.assign_color(operation, stroke, color, None);
    }

    pub(super) fn assign_color(
        &mut self,
        operation: &Operation,
        stroke: bool,
        color: Color,
        resource_span: Option<SourceSpan>,
    ) {
        let derived = if let Some(span) = resource_span {
            Derived::assigned_from_resource(color, span, operation.span())
        } else {
            Derived::assigned(color, operation.span())
        };
        if stroke {
            self.state.stroke_color = derived;
        } else {
            self.state.fill_color = derived;
        }
    }

    pub(super) fn set_shading_pattern_color(
        &mut self,
        operation: &Operation,
        stroke: bool,
        resource: &ResourceEntry,
    ) -> Result<(), InterpretError> {
        let pattern = resource.shading_pattern().map_err(|error| {
            InterpretError::at(operation, InterpretErrorKind::PatternResource(error.kind()))
        })?;
        let resources = self
            .resources
            .as_ref()
            .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::ResourceScopeMissing))?
            .clone();
        let shading = parse_shading(
            operation,
            resource.name().to_vec(),
            &pattern.shading,
            &resources,
            self.state.clone(),
            self.limits,
        )?;
        self.record_ignored_shading_entries(operation, &shading);
        let matrix = shading_pattern_matrix(&pattern, operation)?;
        let paint = ShadingPatternPaint {
            reference: pattern.reference,
            dictionary_span: pattern.dictionary.span(),
            matrix,
            base: self.pattern_base,
            shading,
        };
        self.assign_color(
            operation,
            stroke,
            Color::ShadingPattern(Arc::new(paint)),
            Some(pattern.dictionary.span()),
        );
        Ok(())
    }
}
