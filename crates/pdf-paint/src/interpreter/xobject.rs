use pdf_content::{ContentLimits, FormXObject, Operation, parse_operations_strict};

use crate::error::{InterpretError, InterpretErrorKind};
use crate::form::{TransparencyGroupMetadata, form_bbox, form_entry, form_matrix};
use crate::geometry::{FillRule, Path, PathSegment, Point};
use crate::graph::{FormInvocation, PaintAtom, PaintAtomKind, PaintId, TransparencyGroupPaint};
use crate::interpreter::Interpreter;
use crate::shading::parse_shading;
use crate::state::ClipPath;
use pdf_syntax::decode_name;

impl Interpreter {
    pub(super) fn invoke_xobject(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        Self::expect_count(operation, 1)?;
        let name = decode_name(
            self.operand_source(&operation.operands()[0]),
            &operation.operands()[0],
        )
        .map_err(|_| InterpretError::at(operation, InterpretErrorKind::OperandType))?;
        let resource = self
            .resources
            .as_ref()
            .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::ResourceScopeMissing))?
            .xobject(&name)
            .cloned()
            .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::ResourceNotFound))?;
        if self.xobject_hidden(operation, &resource)? {
            return Ok(());
        }
        let Some(form) = resource.form().cloned() else {
            if let Some(reason) = resource.form_unavailable() {
                return Err(InterpretError::at(
                    operation,
                    InterpretErrorKind::FormResource(reason),
                ));
            }
            return self.paint_image(operation, &resource);
        };
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
        if form_entry(&form, b"/Group").is_some() {
            let metadata = self.transparency_metadata(&form, operation)?;
            self.run_transparency_group(operation, &form, &operations, metadata)
        } else {
            self.run_form(operation, &form, &operations)
        }
    }

    fn xobject_hidden(
        &self,
        operation: &Operation,
        resource: &pdf_content::ResourceEntry,
    ) -> Result<bool, InterpretError> {
        let pdf_syntax::ObjectKind::Dictionary(entries) = resource.value().kind() else {
            return Ok(false);
        };
        let Some(value) = entries
            .iter()
            .find(|entry| entry.key_equals(resource.source(), b"/OC"))
            .map(pdf_syntax::DictionaryEntry::value)
        else {
            return Ok(false);
        };
        let resources = self.resources.as_ref().ok_or_else(|| {
            InterpretError::at(operation, InterpretErrorKind::ResourceScopeMissing)
        })?;
        let (source, dictionary, reference) = match value.kind() {
            pdf_syntax::ObjectKind::Reference(reference) => {
                let (source, value) = resource.resolve_object(*reference).map_err(|error| {
                    InterpretError::at(operation, InterpretErrorKind::ImageResource(error.kind()))
                })?;
                (source, value, Some(*reference))
            }
            pdf_syntax::ObjectKind::Dictionary(_) => {
                (resource.source().clone(), value.clone(), None)
            }
            _ => {
                return Err(InterpretError::at(
                    operation,
                    InterpretErrorKind::InvalidMarkedContentProperties,
                ));
            }
        };
        Self::optional_content_of(operation, resources, &source, &dictionary, reference)
    }

    pub(super) fn paint_shading(&mut self, operation: &Operation) -> Result<(), InterpretError> {
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
            .shading(&name)
            .cloned()
            .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::ResourceNotFound))?;
        let shading = parse_shading(
            operation,
            name,
            &resource,
            resources,
            self.state.clone(),
            self.limits,
        )?;
        self.record_ignored_shading_entries(operation, &shading);
        if self.paint_atoms >= self.limits.max_paint_atoms {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::PaintAtomLimit,
            ));
        }
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
            kind: PaintAtomKind::Shading(Box::new(shading)),
            marks: self.marks.clone(),
        });
        self.paint_atoms += 1;
        Ok(())
    }

    pub(super) fn record_ignored_shading_entries(
        &mut self,
        operation: &Operation,
        shading: &crate::shading::ShadingPaint,
    ) {
        for span in &shading.ignored_entries {
            self.repair(
                operation,
                crate::graph::RepairKind::ShadingEntryIgnored { key_span: *span },
            );
        }
    }

    pub(super) fn run_transparency_group(
        &mut self,
        invocation: &Operation,
        form: &FormXObject,
        operations: &[Operation],
        metadata: TransparencyGroupMetadata,
    ) -> Result<(), InterpretError> {
        if self.paint_atoms >= self.limits.max_paint_atoms {
            return Err(InterpretError::at(
                invocation,
                InterpretErrorKind::PaintAtomLimit,
            ));
        }
        let id = PaintId {
            page: self.page,
            stream: self.stream,
            operator_span: invocation.operator_span(),
            invocation_path: self.invocation_path.clone(),
            pattern_path: self.pattern_path.clone(),
            ordinal: self.graph.atoms.len(),
        };
        let group = self.capture_transparency_group(invocation, form, operations, metadata)?;
        self.emit(PaintAtom {
            id,
            kind: PaintAtomKind::TransparencyGroup(Box::new(group)),
            marks: self.marks.clone(),
        });
        self.paint_atoms += 1;
        Ok(())
    }

    pub(super) fn capture_transparency_group(
        &mut self,
        invocation: &Operation,
        form: &FormXObject,
        operations: &[Operation],
        metadata: TransparencyGroupMetadata,
    ) -> Result<TransparencyGroupPaint, InterpretError> {
        let state = self.state.clone();
        let parent_graph = std::mem::take(&mut self.graph);
        let result = self.run_form(invocation, form, operations);
        let graph = std::mem::take(&mut self.graph);
        self.graph = parent_graph;
        result?;
        Ok(TransparencyGroupPaint {
            reference: form.reference,
            dictionary_span: form.dictionary.span(),
            group_span: metadata.group_span,
            group_reference_span: metadata.group_reference_span,
            subtype_span: metadata.subtype_span,
            bbox: metadata.bbox,
            bbox_span: metadata.bbox_span,
            matrix: metadata.matrix,
            isolated: metadata.isolated,
            knockout: metadata.knockout,
            blend_space: metadata.blend_space,
            backdrop: metadata.backdrop,
            state,
            graph,
        })
    }

    pub(super) fn run_form(
        &mut self,
        invocation: &Operation,
        form: &FormXObject,
        operations: &[Operation],
    ) -> Result<(), InterpretError> {
        let (matrix, matrix_span) = form_matrix(form, invocation.operator_span())?;
        let (bbox, bbox_span) = form_bbox(form, invocation.operator_span())?;
        if self.state.clip_paths.len() >= self.limits.max_clip_paths {
            return Err(InterpretError::at(
                invocation,
                InterpretErrorKind::ClipPathLimit,
            ));
        }
        let child_resources = form.resources.clone().or_else(|| self.resources.clone());
        let frame = self.enter_nested(form.bytes.clone(), form.reference, child_resources);

        self.invocation_path.push(FormInvocation {
            form: form.reference,
            operator_span: invocation.operator_span(),
        });
        self.state.ctm.value = self.state.ctm.value.multiply(matrix);
        if let Some(matrix_span) = matrix_span {
            self.state.ctm.provenance.push(matrix_span);
        }
        self.state.ctm.provenance.push(invocation.span());
        self.pattern_base = self.state.ctm.value;
        self.state.clip_paths.push(ClipPath {
            path: Path {
                segments: vec![PathSegment::Rectangle {
                    origin: Point {
                        x: bbox[0],
                        y: bbox[1],
                    },
                    width: bbox[2] - bbox[0],
                    height: bbox[3] - bbox[1],
                    provenance: bbox_span,
                }],
            },
            rule: FillRule::Nonzero,
            ctm: self.state.ctm.clone(),
            provenance: bbox_span,
        });

        let mut last_operator = Some(invocation.operator_span());
        let mut result = Ok(());
        for operation in operations {
            if let Err(error) = self.apply(operation) {
                result = Err(error);
                break;
            }
            last_operator = Some(operation.operator_span());
        }
        if result.is_ok() {
            result = self.check_nested_balance(last_operator);
        }

        self.invocation_path.pop();
        self.leave_nested(frame);
        result
    }
}
