use std::sync::Arc;

use pdf_content::{Font, Operation, ToUnicode, Type3Font, Type3Procedure};
use pdf_syntax::{Object, ObjectKind, decode_name, decode_string};

use crate::error::{InterpretError, InterpretErrorKind};
use crate::form::font_matrix;
use crate::geometry::{FillRule, Matrix, Path, PathSegment, Point};
use crate::graph::{
    PaintAtom, PaintAtomKind, PaintId, PositionedGlyph, TextShowElement, TextShowPaint, Type3Glyph,
};
use crate::interpreter::{FontRequest, Interpreter, ResolvedFont};
use crate::provenance::Derived;
use crate::state::{AppliedFont, ClipPath, TextMatrices, TextRenderingMode};
use crate::text::{code_advance, position_text};
use pdf_content::{
    ContentLimits, FontSubstitution, GlyphProgram, GlyphSegment, ResourceFontError,
    parse_operations_strict,
};

impl Interpreter {
    pub(super) fn begin_text(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        Self::expect_count(operation, 0)?;
        if self.text_matrices.is_some() {
            self.repair(operation, crate::graph::RepairKind::NestedTextObject);
        }
        self.text_matrices = Some(TextMatrices {
            text: Derived::assigned(Matrix::IDENTITY, operation.span()),
            line: Derived::assigned(Matrix::IDENTITY, operation.span()),
        });
        self.text_clip = None;
        Ok(())
    }

    pub(super) fn end_text(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        Self::expect_count(operation, 0)?;
        let Some(matrices) = self.text_matrices.take() else {
            self.repair(operation, crate::graph::RepairKind::UnmatchedEndText);
            return Ok(());
        };
        self.retained_text_matrices = matrices;
        let Some((path, provenance)) = self.text_clip.take() else {
            return Ok(());
        };
        if self.state.clip_paths.len() >= self.limits.max_clip_paths {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::ClipPathLimit,
            ));
        }
        self.state.clip_paths.push(ClipPath {
            path,
            rule: FillRule::Nonzero,
            ctm: self.state.ctm.clone(),
            provenance,
        });
        Ok(())
    }

    pub(super) fn accumulate_text_clip(
        &mut self,
        operation: &Operation,
        glyphs: &[PositionedGlyph],
        program: Option<&GlyphProgram>,
        units_per_em: u16,
        substitution: Option<&FontSubstitution>,
    ) -> Result<(), InterpretError> {
        if glyphs.is_empty() {
            return Ok(());
        }
        let provenance = operation.span();
        let (path, _) = self
            .text_clip
            .get_or_insert_with(|| (Path::default(), provenance));
        if program.is_none() && substitution.is_none() {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::TextClipWithoutOutlines,
            ));
        }
        for glyph in glyphs {
            let outlines = outlines_for(glyph, program, units_per_em, substitution);
            if outlines.is_empty() {
                return Err(InterpretError::at(
                    operation,
                    InterpretErrorKind::TextClipWithoutOutlines,
                ));
            }
            for (outline, units_per_em) in outlines {
                let scale = 1.0 / f64::from(units_per_em);
                let place = |x: f64, y: f64| {
                    let point = glyph.matrix.transform(Point {
                        x: x * scale,
                        y: y * scale,
                    });
                    Point {
                        x: point.x,
                        y: point.y,
                    }
                };
                for segment in &outline.segments {
                    path.segments.push(match *segment {
                        GlyphSegment::MoveTo { x, y } => PathSegment::MoveTo {
                            point: place(x, y),
                            provenance,
                        },
                        GlyphSegment::LineTo { x, y } => PathSegment::LineTo {
                            point: place(x, y),
                            provenance,
                        },
                        GlyphSegment::CurveTo {
                            x1,
                            y1,
                            x2,
                            y2,
                            x,
                            y,
                        } => PathSegment::CubicTo {
                            control_1: place(x1, y1),
                            control_2: place(x2, y2),
                            end: place(x, y),
                            provenance,
                        },
                        GlyphSegment::Close => PathSegment::ClosePath { provenance },
                    });
                }
            }
        }
        Ok(())
    }

    pub(super) fn set_character_spacing(
        &mut self,
        operation: &Operation,
    ) -> Result<(), InterpretError> {
        let value = self.numbers::<1>(operation)?[0];
        self.state.text.character_spacing = Derived::assigned(value, operation.span());
        Ok(())
    }

    pub(super) fn set_word_spacing(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        let value = self.numbers::<1>(operation)?[0];
        self.state.text.word_spacing = Derived::assigned(value, operation.span());
        Ok(())
    }

    pub(super) fn set_horizontal_scaling(
        &mut self,
        operation: &Operation,
    ) -> Result<(), InterpretError> {
        let value = self.numbers::<1>(operation)?[0];
        self.state.text.horizontal_scaling = Derived::assigned(value, operation.span());
        Ok(())
    }

    pub(super) fn set_text_leading(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        let value = self.numbers::<1>(operation)?[0];
        self.state.text.leading = Derived::assigned(value, operation.span());
        Ok(())
    }

    pub(super) fn set_text_font(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        Self::expect_count(operation, 2)?;
        let name = decode_name(
            self.operand_source(&operation.operands()[0]),
            &operation.operands()[0],
        )
        .unwrap_or_else(|_| b"\x00no name".to_vec());
        let size = self.number(&operation.operands()[1], operation)?;
        let usable = self
            .resources
            .as_ref()
            .and_then(|resources| resources.font(&name))
            .is_some_and(|resource| matches!(resource.value().kind(), ObjectKind::Dictionary(_)));
        if !usable {
            self.bind_standard_font(operation, &name)?;
        }
        let resource = self
            .resources
            .as_ref()
            .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::ResourceScopeMissing))?
            .font(&name)
            .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::ResourceNotFound))?;
        if !matches!(resource.value().kind(), ObjectKind::Dictionary(_)) {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::FontNotDictionary,
            ));
        }
        self.state.text.font = Some(Derived::assigned_from_resource(
            AppliedFont {
                name,
                reference: resource.reference(),
            },
            resource.value().span(),
            operation.span(),
        ));
        self.state.text.font_size = Derived::assigned(size, operation.span());
        Ok(())
    }

    fn bind_standard_font(
        &mut self,
        operation: &Operation,
        name: &[u8],
    ) -> Result<(), InterpretError> {
        const DICTIONARY: &[u8] = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";
        let derivation = 0x5466_0000_0000_0000_u64 | operation.operator_span().start() as u64;
        let source = pdf_bytes::ByteStore::new(
            pdf_bytes::SourceId::derived(self.source.id(), derivation),
            DICTIONARY.to_vec(),
        );
        let dictionary =
            pdf_syntax::ObjectParser::new(&source, 0, pdf_syntax::ParseLimits::default())
                .parse_next()
                .ok()
                .flatten()
                .ok_or_else(|| {
                    InterpretError::at(operation, InterpretErrorKind::FontNotDictionary)
                })?;
        let resources = self.resources.as_mut().ok_or_else(|| {
            InterpretError::at(operation, InterpretErrorKind::ResourceScopeMissing)
        })?;
        if !resources.bind_font(name.to_vec(), source, dictionary) {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::ResourceNotFound,
            ));
        }
        self.repair(
            operation,
            crate::graph::RepairKind::FontNameBoundToStandardFont,
        );
        Ok(())
    }

    pub(super) fn set_text_rendering_mode(
        &mut self,
        operation: &Operation,
    ) -> Result<(), InterpretError> {
        let mode = match self.exact_integer(operation)? {
            0 => TextRenderingMode::Fill,
            1 => TextRenderingMode::Stroke,
            2 => TextRenderingMode::FillStroke,
            3 => TextRenderingMode::Invisible,
            4 => TextRenderingMode::FillClip,
            5 => TextRenderingMode::StrokeClip,
            6 => TextRenderingMode::FillStrokeClip,
            7 => TextRenderingMode::Clip,
            _ => {
                return Err(InterpretError::at(
                    operation,
                    InterpretErrorKind::InvalidValue,
                ));
            }
        };
        self.state.text.rendering_mode = Derived::assigned(mode, operation.span());
        Ok(())
    }

    pub(super) fn set_text_rise(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        let value = self.numbers::<1>(operation)?[0];
        self.state.text.rise = Derived::assigned(value, operation.span());
        Ok(())
    }

    pub(super) fn move_text_line(
        &mut self,
        operation: &Operation,
        set_leading: bool,
    ) -> Result<(), InterpretError> {
        let [tx, ty] = self.numbers::<2>(operation)?;
        if set_leading {
            self.state.text.leading = Derived::assigned(-ty, operation.span());
        }
        let matrices = self.text_matrices_in_hand(operation);
        let translation = Matrix {
            e: tx,
            f: ty,
            ..Matrix::IDENTITY
        };
        matrices.line.value = matrices.line.value.multiply(translation);
        matrices.line.provenance.push(operation.span());
        matrices.text = matrices.line.clone();
        Ok(())
    }

    pub(super) fn set_text_matrix(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        let values = self.numbers::<6>(operation)?;
        let matrices = self.text_matrices_in_hand(operation);
        let matrix = Matrix {
            a: values[0],
            b: values[1],
            c: values[2],
            d: values[3],
            e: values[4],
            f: values[5],
        };
        matrices.text = Derived::assigned(matrix, operation.span());
        matrices.line = matrices.text.clone();
        Ok(())
    }

    pub(super) fn next_text_line(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        Self::expect_count(operation, 0)?;
        self.require_text_object(operation);
        self.advance_text_line(operation);
        Ok(())
    }

    fn advance_text_line(&mut self, operation: &Operation) {
        let translation = Matrix {
            e: 0.0,
            f: -self.state.text.leading.value,
            ..Matrix::IDENTITY
        };
        let matrices = self.text_matrices.as_mut().expect("text object checked");
        matrices.line.value = matrices.line.value.multiply(translation);
        matrices.line.provenance.push(operation.span());
        matrices.text = matrices.line.clone();
    }

    pub(super) fn show_next_line(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        self.require_text_object(operation);
        Self::expect_count(operation, 1)?;
        self.advance_text_line(operation);
        self.show_string(operation, 0)
    }

    pub(super) fn show_next_line_spaced(
        &mut self,
        operation: &Operation,
    ) -> Result<(), InterpretError> {
        self.require_text_object(operation);
        Self::expect_count(operation, 3)?;
        let word = self.number(&operation.operands()[0], operation)?;
        let character = self.number(&operation.operands()[1], operation)?;
        self.state.text.word_spacing = Derived::assigned(word, operation.span());
        self.state.text.character_spacing = Derived::assigned(character, operation.span());
        self.advance_text_line(operation);
        self.show_string(operation, 2)
    }

    pub(super) fn require_text_object(&mut self, operation: &Operation) {
        let _ = self.text_matrices_in_hand(operation);
    }

    pub(super) fn show_text(
        &mut self,
        operation: &Operation,
        array_form: bool,
    ) -> Result<(), InterpretError> {
        self.require_text_object(operation);
        Self::expect_count(operation, 1)?;
        self.show(operation, array_form, 0)
    }

    fn show_string(&mut self, operation: &Operation, operand: usize) -> Result<(), InterpretError> {
        self.show(operation, false, operand)
    }

    fn show(
        &mut self,
        operation: &Operation,
        array_form: bool,
        operand: usize,
    ) -> Result<(), InterpretError> {
        let clipping = matches!(
            self.state.text.rendering_mode.value,
            TextRenderingMode::FillClip
                | TextRenderingMode::StrokeClip
                | TextRenderingMode::FillStrokeClip
                | TextRenderingMode::Clip
        );
        let font = self.current_font(operation)?;
        let elements = if array_form {
            self.decode_text_array(operation, &font)?
        } else {
            vec![self.decode_text_string(operation, &font, &operation.operands()[operand])?]
        };
        if self.paint_atoms >= self.limits.max_paint_atoms {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::PaintAtomLimit,
            ));
        }
        let matrices = self
            .text_matrices
            .as_ref()
            .expect("text object checked")
            .clone();
        let type3 = self.current_type3(operation)?;
        let width_scale = type3
            .as_ref()
            .map_or(1.0 / 1000.0, |font| font.advance_scale());
        let advance = self.text_advance(&elements, width_scale);
        let program = self.font_program(operation)?;
        let text = self.font_text_map();
        let units_per_em = program.as_ref().map_or(1000, |font| font.units_per_em());
        let mut glyphs =
            self.position_glyphs(&elements, &matrices, program.as_deref(), &font, width_scale);
        let type3_font = type3.is_some();
        let text = match program.as_deref() {
            Some(program) if !type3_font => {
                self.match_outlines(operation, &glyphs, program, text)?
            }
            _ => text,
        };
        let (substitution, font_request) = if type3_font {
            (None, None)
        } else if program.is_none() {
            self.substitute(operation, &mut glyphs, &font, &text)?
        } else {
            (None, self.font_request_of(operation)?)
        };
        if let Some(type3) = type3 {
            self.run_type3_glyphs(operation, &mut glyphs, &type3, &font)?;
        }
        if clipping {
            self.accumulate_text_clip(
                operation,
                &glyphs,
                program.as_deref(),
                units_per_em,
                substitution.as_deref(),
            )?;
        }
        let family_line = self.line_of(operation, program.as_deref(), font_request.as_deref())?;
        let ordinal = self.graph.atoms.len();
        self.note_matched_run(ordinal);
        self.emit(PaintAtom {
            id: PaintId {
                page: self.page,
                stream: self.stream,
                operator_span: operation.operator_span(),
                invocation_path: self.invocation_path.clone(),
                pattern_path: self.pattern_path.clone(),
                ordinal,
            },
            kind: PaintAtomKind::Text(TextShowPaint {
                elements,
                state: self.state.clone(),
                matrices,
                glyphs,
                program,
                text,
                substitution,
                font_request,
                units_per_em,
                type3: type3_font,
                family_line,
            }),
            marks: self.marks.clone(),
        });
        self.paint_atoms += 1;
        let translation = Matrix {
            e: advance,
            f: 0.0,
            ..Matrix::IDENTITY
        };
        let matrices = self.text_matrices.as_mut().expect("text object checked");
        matrices.text.value = matrices.text.value.multiply(translation);
        matrices.text.provenance.push(operation.span());
        Ok(())
    }

    pub(super) fn current_font(
        &mut self,
        operation: &Operation,
    ) -> Result<Arc<Font>, InterpretError> {
        let applied =
            self.state.text.font.as_ref().ok_or_else(|| {
                InterpretError::at(operation, InterpretErrorKind::TextFontMissing)
            })?;
        let resource = self
            .resources
            .as_ref()
            .and_then(|resources| resources.font(&applied.value.name))
            .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::ResourceNotFound))?;
        if let Some(reference) = resource.reference()
            && let Some((parsed_from, font)) = self.parsed_fonts.get(&reference)
            && parsed_from.is_same_object(resource)
        {
            return Ok(Arc::clone(font));
        }
        let font = Arc::new(resource.font().map_err(|error| {
            InterpretError::at(operation, InterpretErrorKind::FontDecode(error))
        })?);
        if let Some(reference) = resource.reference() {
            self.parsed_fonts
                .insert(reference, (resource.clone(), Arc::clone(&font)));
        }
        Ok(font)
    }

    pub(super) fn decode_text_array(
        &self,
        operation: &Operation,
        font: &Font,
    ) -> Result<Vec<TextShowElement>, InterpretError> {
        let ObjectKind::Array(entries) = operation.operands()[0].kind() else {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::OperandType,
            ));
        };
        let mut elements = Vec::with_capacity(entries.len());
        let mut decoded_bytes = 0_usize;
        for entry in entries {
            match entry.kind() {
                ObjectKind::LiteralString | ObjectKind::HexString => {
                    let element = self.decode_text_string(operation, font, entry)?;
                    let TextShowElement::Codes {
                        decoded_bytes: bytes,
                        ..
                    } = &element
                    else {
                        unreachable!("string decoder returns codes")
                    };
                    decoded_bytes = decoded_bytes.checked_add(bytes.len()).ok_or_else(|| {
                        InterpretError::at(operation, InterpretErrorKind::TextStringLimit)
                    })?;
                    if decoded_bytes > self.limits.max_decoded_text_bytes {
                        return Err(InterpretError::at(
                            operation,
                            InterpretErrorKind::TextStringLimit,
                        ));
                    }
                    elements.push(element);
                }
                ObjectKind::Number(_) => elements.push(TextShowElement::Adjustment {
                    source_span: entry.span(),
                    value: self.number(entry, operation)?,
                }),
                _ => {
                    return Err(InterpretError::at(
                        operation,
                        InterpretErrorKind::OperandType,
                    ));
                }
            }
        }
        Ok(elements)
    }

    pub(super) fn decode_text_string(
        &self,
        operation: &Operation,
        font: &Font,
        object: &Object,
    ) -> Result<TextShowElement, InterpretError> {
        let decoded_bytes = decode_string(
            self.operand_source(object),
            object,
            self.limits.max_decoded_text_bytes,
        )
        .map_err(|_| InterpretError::at(operation, InterpretErrorKind::InvalidTextString))?;
        let codes = font.source_codes(&decoded_bytes).map_err(|error| {
            InterpretError::at(
                operation,
                InterpretErrorKind::FontDecode(ResourceFontError::CMap(error)),
            )
        })?;
        Ok(TextShowElement::Codes {
            source_span: object.span(),
            decoded_bytes,
            codes,
        })
    }

    pub(super) fn font_program(
        &mut self,
        operation: &Operation,
    ) -> Result<Option<Arc<GlyphProgram>>, InterpretError> {
        let Some(applied) = self.state.text.font.as_ref() else {
            return Ok(None);
        };
        let Some(resource) = self
            .resources
            .as_ref()
            .and_then(|resources| resources.font(&applied.value.name))
        else {
            return Ok(None);
        };
        let reference = resource.reference();
        if let Some(reference) = reference
            && let Some(cached) = self.font_programs.get(&reference)
        {
            return Ok(cached.clone());
        }
        let program = resource.glyph_program().map_err(|error| {
            InterpretError::at(operation, InterpretErrorKind::FontProgram(error.kind()))
        })?;
        if let Some(reference) = reference {
            self.font_programs.insert(reference, program.clone());
        }
        Ok(program)
    }

    fn match_outlines(
        &mut self,
        operation: &Operation,
        glyphs: &[PositionedGlyph],
        program: &GlyphProgram,
        text: Arc<ToUnicode>,
    ) -> Result<Arc<ToUnicode>, InterpretError> {
        let unread = |glyph: &&PositionedGlyph| {
            glyph.glyph.is_some()
                && text
                    .text_of(pdf_content::Code {
                        value: glyph.code.value,
                        byte_len: glyph.code.bytes.len(),
                    })
                    .is_none_or(|meaning| meaning.text.is_empty())
        };
        if self.fonts.is_none() || !glyphs.iter().any(|glyph| unread(&glyph)) {
            return Ok(text);
        }
        let reference = self
            .state
            .text
            .font
            .as_ref()
            .and_then(|applied| self.resources.as_ref()?.font(&applied.value.name))
            .and_then(pdf_content::ResourceEntry::reference);
        let Some(face) = self.same_family_face(operation)? else {
            return Ok(text);
        };
        let faces = pdf_content::outline_match::reference_of(&face);
        let tried = reference.map(|reference| self.outlines_tried.entry(reference).or_default());
        let mut tried = tried;
        let mut matched = Vec::new();
        for glyph in glyphs.iter().filter(unread) {
            let code = (glyph.code.value, glyph.code.bytes.len());
            if let Some(tried) = tried.as_mut()
                && !tried.insert(code)
            {
                continue;
            }
            let Some(id) = glyph.glyph else { continue };
            let Some(raster) = pdf_content::outline_match::Raster::of(program, id) else {
                continue;
            };
            let hint = program
                .glyph_name(id)
                .and_then(pdf_content::outline_match::index_in_name)
                .or_else(|| glyph.code.cid.and_then(|cid| u16::try_from(cid).ok()));
            if let Some(character) = faces.character_of(&raster, hint) {
                matched.push((
                    pdf_content::Code {
                        value: code.0,
                        byte_len: code.1,
                    },
                    character,
                ));
            }
        }
        if matched.is_empty() {
            return Ok(text);
        }
        let mut extended = (*text).clone();
        extended.add_matched(matched);
        let extended = Arc::new(extended);
        if let Some(reference) = reference {
            self.text_maps.insert(reference, Arc::clone(&extended));
        }
        Ok(extended)
    }

    fn same_family_face(
        &mut self,
        operation: &Operation,
    ) -> Result<Option<pdf_content::SubstitutedFace>, InterpretError> {
        let Some(request) = self.font_request_of(operation)? else {
            return Ok(None);
        };
        let mut named = (*request).clone();
        pdf_content::outline_match::font_family(&request.family).clone_into(&mut named.family);
        Ok(self
            .fonts
            .as_ref()
            .and_then(|provider| provider.primary_face(&named))
            .filter(|face| pdf_content::outline_match::is_same_family(face, &request.family)))
    }

    fn line_of(
        &mut self,
        operation: &Operation,
        program: Option<&GlyphProgram>,
        request: Option<&FontRequest>,
    ) -> Result<Option<(f64, f64)>, InterpretError> {
        let (Some(program), Some(request)) = (program, request) else {
            return Ok(None);
        };
        let stated = Some(request)
            .and_then(|request| Some((request.ascent? / 1000.0, request.descent? / 1000.0)))
            .and_then(crate::graph::usable_line)
            .or_else(|| {
                program
                    .vertical_metrics()
                    .and_then(crate::graph::usable_line)
            });
        if self.fonts.is_none() || stated.is_some() {
            return Ok(None);
        }
        Ok(self
            .same_family_face(operation)?
            .and_then(|face| face.program.vertical_metrics())
            .and_then(crate::graph::usable_line))
    }

    fn note_matched_run(&mut self, ordinal: usize) {
        if let Some(reference) = self
            .state
            .text
            .font
            .as_ref()
            .and_then(|applied| self.resources.as_ref()?.font(&applied.value.name))
            .and_then(pdf_content::ResourceEntry::reference)
            .filter(|reference| {
                self.hidden_marks.is_empty() && self.outlines_tried.contains_key(reference)
            })
        {
            self.matched_runs.push((ordinal, reference));
        }
    }

    fn substitute(
        &mut self,
        operation: &Operation,
        glyphs: &mut [PositionedGlyph],
        font: &Font,
        text: &ToUnicode,
    ) -> Result<Substituted, InterpretError> {
        let Some(resolved) = self.resolved_font(operation)? else {
            return Ok((None, None));
        };
        let Some(provider) = self.fonts.as_ref() else {
            return Ok((None, Some(Arc::clone(&resolved.request))));
        };
        let face = resolved.face.clone().or_else(|| {
            crate::text::covering_primary(glyphs, &resolved.request, provider.as_ref(), font, text)
        });
        let Some(face) = face else {
            return Ok((None, Some(Arc::clone(&resolved.request))));
        };
        let substitution = crate::text::substitute_run(
            glyphs,
            &resolved.request,
            face.clone(),
            provider.as_ref(),
            font,
            text,
        );
        Ok((Some(Arc::new(substitution)), None))
    }

    fn font_request_of(
        &mut self,
        operation: &Operation,
    ) -> Result<Option<Arc<FontRequest>>, InterpretError> {
        let Some(applied) = self.state.text.font.as_ref() else {
            return Ok(None);
        };
        let name = applied.value.name.clone();
        let Some(resource) = self
            .resources
            .as_ref()
            .and_then(|resources| resources.font(&name))
        else {
            return Ok(None);
        };
        let reference = resource.reference();
        if let Some(reference) = reference
            && let Some(cached) = self.font_requests.get(&reference)
        {
            return Ok(cached.clone());
        }
        let request = resource
            .font_request()
            .map_err(|error| {
                InterpretError::at(operation, InterpretErrorKind::FontProgram(error.kind()))
            })?
            .map(Arc::new);
        if let Some(reference) = reference {
            self.font_requests.insert(reference, request.clone());
        }
        Ok(request)
    }

    fn resolved_font(
        &mut self,
        operation: &Operation,
    ) -> Result<Option<Arc<ResolvedFont>>, InterpretError> {
        let Some(applied) = self.state.text.font.as_ref() else {
            return Ok(None);
        };
        let name = applied.value.name.clone();
        let Some(resource) = self
            .resources
            .as_ref()
            .and_then(|resources| resources.font(&name))
        else {
            return Ok(None);
        };
        let reference = resource.reference();
        if let Some(reference) = reference
            && let Some(cached) = self.font_faces.get(&reference)
        {
            return Ok(cached.clone());
        }
        let request = self.font_request_of(operation)?;
        let resolved = request.map(|request| {
            let face = self
                .fonts
                .as_ref()
                .and_then(|provider| provider.primary_face(&request));
            Arc::new(ResolvedFont { request, face })
        });
        if let Some(reference) = reference {
            self.font_faces.insert(reference, resolved.clone());
        }
        Ok(resolved)
    }

    pub(super) fn font_text_map(&mut self) -> Arc<ToUnicode> {
        let Some(applied) = self.state.text.font.as_ref() else {
            return Arc::default();
        };
        let Some(resource) = self
            .resources
            .as_ref()
            .and_then(|resources| resources.font(&applied.value.name))
        else {
            return Arc::default();
        };
        let reference = resource.reference();
        if let Some(reference) = reference
            && let Some(cached) = self.text_maps.get(&reference)
        {
            return Arc::clone(cached);
        }
        let map = resource.to_unicode().unwrap_or_default();
        if let Some(reference) = reference {
            self.text_maps.insert(reference, Arc::clone(&map));
        }
        map
    }

    pub(super) fn current_type3(
        &mut self,
        operation: &Operation,
    ) -> Result<Option<Arc<Type3Font>>, InterpretError> {
        let Some(applied) = self.state.text.font.as_ref() else {
            return Ok(None);
        };
        let Some(resource) = self
            .resources
            .as_ref()
            .and_then(|resources| resources.font(&applied.value.name))
        else {
            return Ok(None);
        };
        let reference = resource.reference();
        if let Some(reference) = reference
            && let Some(cached) = self.type3_fonts.get(&reference)
        {
            return Ok(cached.clone());
        }
        let font = resource
            .type3_font()
            .map_err(|error| {
                InterpretError::at(operation, InterpretErrorKind::Type3Resource(error.kind()))
            })?
            .map(Arc::new);
        if let Some(reference) = reference {
            self.type3_fonts.insert(reference, font.clone());
        }
        Ok(font)
    }

    pub(super) fn run_type3_glyphs(
        &mut self,
        operation: &Operation,
        glyphs: &mut [PositionedGlyph],
        type3: &Type3Font,
        font: &Font,
    ) -> Result<(), InterpretError> {
        let Font::Simple(simple) = font else {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::Type3NotSimpleFont,
            ));
        };
        for glyph in glyphs.iter_mut() {
            let Ok(code) = u8::try_from(glyph.code.value) else {
                continue;
            };
            let Some(name) = simple.glyph_name(code).map(<[u8]>::to_vec) else {
                continue;
            };
            let Some(procedure) = type3.procedure(&name).cloned() else {
                continue;
            };
            let placement = glyph.matrix;
            let (atoms, shape_only) =
                self.capture_type3_glyph(operation, &procedure, type3, placement)?;
            glyph.procedure = Some(Arc::new(Type3Glyph {
                name,
                reference: procedure.reference,
                atoms,
                shape_only,
            }));
        }
        Ok(())
    }

    pub(super) fn capture_type3_glyph(
        &mut self,
        operation: &Operation,
        procedure: &Type3Procedure,
        type3: &Type3Font,
        placement: Matrix,
    ) -> Result<(Vec<PaintAtom>, bool), InterpretError> {
        if self.glyph_depth >= self.limits.max_form_depth {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::FormDepthLimit,
            ));
        }
        let operations = if let Some(cached) = self.type3_procedures.get(&procedure.reference) {
            Arc::clone(cached)
        } else {
            let parsed = Arc::new(
                parse_operations_strict(&procedure.bytes, ContentLimits::default()).map_err(
                    |_| InterpretError::at(operation, InterpretErrorKind::Type3ProcedureContent),
                )?,
            );
            self.type3_procedures
                .insert(procedure.reference, Arc::clone(&parsed));
            parsed
        };
        let resources = type3.resources.clone().or_else(|| self.resources.clone());
        let frame = self.enter_nested(procedure.bytes.clone(), procedure.reference, resources);
        self.glyph_depth += 1;
        let parent_shape_only = std::mem::take(&mut self.glyph_shape_only);
        let parent_graph = std::mem::take(&mut self.graph);
        self.state.ctm.value = self
            .state
            .ctm
            .value
            .multiply(placement)
            .multiply(font_matrix(type3));
        self.state.ctm.provenance.push(type3.font_matrix_span);
        self.state.ctm.provenance.push(operation.span());

        let result = self.run_glyph_operations(operation, &operations);
        let graph = std::mem::take(&mut self.graph);
        self.graph = parent_graph;
        let shape_only = std::mem::replace(&mut self.glyph_shape_only, parent_shape_only);
        self.glyph_depth -= 1;
        self.leave_nested(frame);
        result?;
        Ok((graph.atoms, shape_only))
    }

    pub(super) fn run_glyph_operations(
        &mut self,
        invocation: &Operation,
        operations: &[Operation],
    ) -> Result<(), InterpretError> {
        let mut last_operator = Some(invocation.operator_span());
        for operation in operations {
            self.apply(operation)?;
            last_operator = Some(operation.operator_span());
        }
        self.check_nested_balance(last_operator)
    }

    pub(super) fn set_glyph_width(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        self.require_glyph_procedure(operation)?;
        let _ = self.numbers::<2>(operation)?;
        Ok(())
    }

    pub(super) fn set_glyph_shape(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        self.require_glyph_procedure(operation)?;
        let _ = self.numbers::<6>(operation)?;
        self.glyph_shape_only = true;
        Ok(())
    }

    pub(super) fn require_glyph_procedure(
        &self,
        operation: &Operation,
    ) -> Result<(), InterpretError> {
        if self.glyph_depth == 0 {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::GlyphMetricOutsideProcedure,
            ));
        }
        Ok(())
    }

    pub(super) fn position_glyphs(
        &self,
        elements: &[TextShowElement],
        matrices: &TextMatrices,
        program: Option<&GlyphProgram>,
        font: &Font,
        width_scale: f64,
    ) -> Vec<PositionedGlyph> {
        position_text(
            &self.state.text,
            elements,
            matrices.text.value,
            program,
            font,
            width_scale,
        )
        .0
    }

    pub(super) fn text_advance(&self, elements: &[TextShowElement], width_scale: f64) -> f64 {
        let text = &self.state.text;
        let scale = text.horizontal_scaling.value / 100.0;
        elements.iter().fold(0.0, |advance, element| match element {
            TextShowElement::Codes { codes, .. } => {
                advance
                    + codes.iter().fold(0.0, |width, code| {
                        width + code_advance(text, code, width_scale)
                    })
            }
            TextShowElement::Adjustment { value, .. } => {
                advance - value / 1000.0 * text.font_size.value * scale
            }
        })
    }
}

type Substituted = (Option<Arc<FontSubstitution>>, Option<Arc<FontRequest>>);

fn outlines_for(
    glyph: &PositionedGlyph,
    program: Option<&GlyphProgram>,
    units_per_em: u16,
    substitution: Option<&FontSubstitution>,
) -> Vec<(pdf_content::GlyphPath, u16)> {
    if let Some(substitution) = substitution
        && !glyph.substituted.is_empty()
    {
        return glyph
            .substituted
            .iter()
            .filter_map(|drawn| {
                let face = if drawn.face == 0 {
                    &substitution.primary
                } else {
                    substitution.fallbacks.get(usize::from(drawn.face) - 1)?
                };
                let outline = face.program.path(drawn.glyph)?;
                Some((outline, face.program.units_per_em()))
            })
            .collect();
    }
    let Some(program) = program else {
        return Vec::new();
    };
    let Some(index) = glyph.glyph else {
        return Vec::new();
    };
    program
        .path(index)
        .map(|outline| vec![(outline, units_per_em)])
        .unwrap_or_default()
}
