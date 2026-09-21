use pdf_bytes::{ByteStore, SourceId, SourceSpan};
use pdf_content::{
    ContentLimits, FormXObject, PageProgram, PageResources, parse_operations_strict,
};
use pdf_paint::{FormInvocation, PaintAtomKind, PaintGraph};
use pdf_syntax::{Object, ObjectKind, Reference};

use crate::plan::{PlannedBody, PlannedWrite};
use crate::spike_move_text::SpikeError;

pub(crate) struct FormScope {
    pub name: Vec<u8>,
    pub form: FormXObject,
    pub resources: PageResources,
    pub operations: Vec<pdf_content::Operation>,
}

impl FormScope {
    pub fn bytes(&self) -> &[u8] {
        self.form.bytes.as_bytes()
    }

    pub fn span_of(&self, operator: SourceSpan) -> Result<SourceSpan, SpikeError> {
        self.operations
            .iter()
            .find(|operation| operation.operator_span() == operator)
            .map(pdf_content::Operation::span)
            .ok_or(SpikeError::NoTextRun)
    }
}

pub(crate) fn invocation_of<'a>(
    atoms: impl IntoIterator<Item = &'a pdf_paint::PaintAtom>,
) -> Result<Option<FormInvocation>, SpikeError> {
    let mut found: Option<Option<FormInvocation>> = None;
    for atom in atoms {
        if atom.id.invocation_path.len() > 1 {
            return Err(SpikeError::RunNestedTooDeep);
        }
        let here = atom.id.invocation_path.first().copied();
        match found {
            None => found = Some(here),
            Some(had) if had == here => {}
            Some(_) => return Err(SpikeError::BlockSpansSeveralStreams),
        }
    }
    Ok(found.flatten())
}

pub fn scope_resources(
    program: &PageProgram,
    operations: &[Vec<pdf_content::Operation>],
    id: &pdf_paint::PaintId,
) -> Result<PageResources, SpikeError> {
    let Some(invocation) = id.invocation_path.first().copied() else {
        return Ok(program.resources.clone());
    };
    if id.invocation_path.len() > 1 {
        return Err(SpikeError::RunNestedTooDeep);
    }
    Ok(scope_of(program, operations, invocation)?.resources)
}

pub(crate) fn scope_of(
    program: &PageProgram,
    operations: &[Vec<pdf_content::Operation>],
    invocation: FormInvocation,
) -> Result<FormScope, SpikeError> {
    let name = invoking_name(program, operations, invocation)?;
    let entry = program
        .resources
        .xobject(&name)
        .ok_or(SpikeError::FormResourceNotFound)?;
    let form = entry.form().ok_or(SpikeError::FormResourceNotFound)?;
    if form.reference != invocation.form {
        return Err(SpikeError::FormResourceNotFound);
    }
    if invocations_naming(program, operations, &name)? > 1 {
        return Err(SpikeError::FormInvokedTwiceOnThisPage);
    }
    let form_operations = parse_operations_strict(&form.bytes, ContentLimits::default())
        .map_err(|_| SpikeError::UnsupportedContentLayout)?;
    let resources = form
        .resources
        .clone()
        .unwrap_or_else(|| program.resources.clone());
    Ok(FormScope {
        name,
        form: form.clone(),
        resources,
        operations: form_operations,
    })
}

pub(crate) struct FormWrites {
    pub writes: Vec<PlannedWrite>,
    pub target_stream: Reference,
}

pub(crate) fn writes_for(
    source: &ByteStore,
    program: &PageProgram,
    scope: &FormScope,
    edited: Vec<u8>,
) -> Result<FormWrites, SpikeError> {
    if uses_in_document(source, scope.form.reference)? <= 1 {
        return Ok(FormWrites {
            writes: vec![PlannedWrite {
                reference: scope.form.reference,
                body: PlannedBody::ReplacedStream { decoded: edited },
            }],
            target_stream: scope.form.reference,
        });
    }

    let page = crate::spike_move_text::resolve_page_object(source, program.page)?;
    let reference_span = page_form_reference_span(&page, &scope.name)?;
    let copy = Reference::new(crate::block_rewrite::next_object_number(source)?, 0);
    let dictionary = copied_form_dictionary(&scope.form.source, &scope.form.dictionary)?;
    let value_start = reference_span
        .start()
        .checked_sub(page.body_offset)
        .ok_or(SpikeError::SharedFormResources)?;
    let value_end = reference_span
        .end()
        .checked_sub(page.body_offset)
        .ok_or(SpikeError::SharedFormResources)?;
    if value_end > page.body.len() || value_start > value_end {
        return Err(SpikeError::SharedFormResources);
    }
    let mut page_body = Vec::with_capacity(page.body.len() + 16);
    page_body.extend_from_slice(&page.body[..value_start]);
    page_body
        .extend_from_slice(format!("{} {} R", copy.object_number(), copy.generation()).as_bytes());
    page_body.extend_from_slice(&page.body[value_end..]);

    Ok(FormWrites {
        writes: vec![
            PlannedWrite {
                reference: copy,
                body: PlannedBody::NewStream {
                    dictionary,
                    decoded: edited,
                },
            },
            PlannedWrite {
                reference: program.page,
                body: PlannedBody::Direct { body: page_body },
            },
        ],
        target_stream: copy,
    })
}

fn uses_in_document(source: &ByteStore, reference: Reference) -> Result<usize, SpikeError> {
    let chain = pdf_syntax::parse_revision_chain_strict(source, pdf_syntax::XrefLimits::default())
        .map_err(|_| SpikeError::SharedFormResources)?;
    let index = pdf_syntax::RevisionIndex::from_chain(&chain)
        .map_err(|_| SpikeError::SharedFormResources)?;
    let mut found = 0_usize;
    for entry in index.selected_entries() {
        let entry = entry.entry();
        if matches!(entry.kind(), pdf_syntax::XrefEntryKind::Free { .. }) {
            continue;
        }
        let number = entry.object_number();
        if number == reference.object_number() {
            continue;
        }
        let Ok(resolved) = index.resolve_object(
            source,
            Reference::new(number, entry.generation()),
            pdf_syntax::ResolveLimits::default(),
        ) else {
            return Ok(2);
        };
        found += names_reference(resolved.value(), reference, 0);
        if found >= 2 {
            return Ok(found);
        }
    }
    Ok(found)
}

fn names_reference(value: &Object, reference: Reference, depth: usize) -> usize {
    const DEEPEST: usize = 32;
    if depth > DEEPEST {
        return 0;
    }
    match value.kind() {
        ObjectKind::Reference(found) => usize::from(*found == reference),
        ObjectKind::Array(items) => items
            .iter()
            .map(|item| names_reference(item, reference, depth + 1))
            .sum(),
        ObjectKind::Dictionary(entries) => entries
            .iter()
            .map(|entry| names_reference(entry.value(), reference, depth + 1))
            .sum(),
        _ => 0,
    }
}

pub(crate) fn prove_isolated(
    scope: &FormScope,
    edited: &[u8],
    edited_spans: &[SourceSpan],
    fonts: crate::Fonts<'_>,
) -> Result<Edited, SpikeError> {
    let form = &scope.form;
    let before = crate::split::interpret_form_for_proof(
        &form.bytes,
        form.reference,
        &scope.resources,
        fonts,
    )?;
    let candidate = ByteStore::new(
        SourceId::new(form.bytes.id().get().wrapping_add(1)),
        std::sync::Arc::<[u8]>::from(edited.to_vec()),
    );
    let after = crate::split::interpret_form_for_proof(
        &candidate,
        form.reference,
        &scope.resources,
        fonts,
    )?;
    if before.atoms.len() != after.atoms.len() {
        return Err(SpikeError::MoveNotIsolated);
    }
    let mut windows = Vec::new();
    for span in edited_spans {
        let at = before
            .atoms
            .iter()
            .position(|atom| atom.id.operator_span == *span)
            .ok_or(SpikeError::MoveNotIsolated)?;
        windows.push(at);
    }
    let glyphs_in = |graph: &PaintGraph, at: usize| -> usize {
        match graph.atoms.get(at).map(|atom| &atom.kind) {
            Some(PaintAtomKind::Text(text)) => text.glyphs.len(),
            _ => 0,
        }
    };
    let mut ranges = Vec::with_capacity(windows.len());
    for at in &windows {
        let start: usize = (0..*at).map(|index| glyphs_in(&before, index)).sum();
        ranges.push(start..start + glyphs_in(&before, *at));
    }
    let one = pdf_paint::glyph_placement_signature(&before);
    let other = pdf_paint::glyph_placement_signature(&after);
    if one.len() != other.len() {
        return Err(SpikeError::MoveNotIsolated);
    }
    for (index, (a, b)) in one.iter().zip(&other).enumerate() {
        if a != b && !ranges.iter().any(|range| range.contains(&index)) {
            return Err(SpikeError::MoveNotIsolated);
        }
    }
    let signatures = |graph: &PaintGraph| -> Vec<String> {
        graph
            .atoms
            .iter()
            .map(|atom| pdf_paint::paint_signature(&atom.kind))
            .collect()
    };
    let one = signatures(&before);
    let other = signatures(&after);
    for (index, (a, b)) in one.iter().zip(&other).enumerate() {
        if a != b && !windows.contains(&index) {
            return Err(SpikeError::MoveNotIsolated);
        }
    }
    Ok(Edited {
        before,
        after,
        windows,
    })
}

pub(crate) struct Edited {
    pub before: PaintGraph,
    pub after: PaintGraph,
    pub windows: Vec<usize>,
}

fn invocations_naming(
    program: &PageProgram,
    operations: &[Vec<pdf_content::Operation>],
    name: &[u8],
) -> Result<usize, SpikeError> {
    let mut found = 0_usize;
    for (stream, stream_operations) in program.streams.iter().zip(operations) {
        for operation in stream_operations {
            if operation.operator_bytes(&stream.bytes) != Ok(b"Do") {
                continue;
            }
            let [operand] = operation.operands() else {
                continue;
            };
            if pdf_syntax::decode_name(&stream.bytes, operand)
                .map_err(|_| SpikeError::FormResourceNotFound)?
                == name
            {
                found += 1;
            }
        }
    }
    Ok(found)
}

fn invoking_name(
    program: &PageProgram,
    operations: &[Vec<pdf_content::Operation>],
    invocation: FormInvocation,
) -> Result<Vec<u8>, SpikeError> {
    for (stream, stream_operations) in program.streams.iter().zip(operations) {
        for operation in stream_operations {
            if operation.operator_span() != invocation.operator_span {
                continue;
            }
            let [operand] = operation.operands() else {
                return Err(SpikeError::FormResourceNotFound);
            };
            return pdf_syntax::decode_name(&stream.bytes, operand)
                .map_err(|_| SpikeError::FormResourceNotFound);
        }
    }
    Err(SpikeError::FormResourceNotFound)
}

fn page_form_reference_span(
    page: &crate::spike_move_text::PageObjectBody,
    name: &[u8],
) -> Result<SourceSpan, SpikeError> {
    let mut value = &page.value;
    for key in [&b"/Resources"[..], b"/XObject", name] {
        let ObjectKind::Dictionary(entries) = value.kind() else {
            return Err(SpikeError::SharedFormResources);
        };
        value = entries
            .iter()
            .find(|entry| entry.key_equals(&page.source, key))
            .map(pdf_syntax::DictionaryEntry::value)
            .ok_or(SpikeError::SharedFormResources)?;
    }
    if !matches!(value.kind(), ObjectKind::Reference(_)) {
        return Err(SpikeError::SharedFormResources);
    }
    Ok(value.span())
}

fn copied_form_dictionary(source: &ByteStore, dictionary: &Object) -> Result<Vec<u8>, SpikeError> {
    let ObjectKind::Dictionary(entries) = dictionary.kind() else {
        return Err(SpikeError::FormResourceNotFound);
    };
    let mut out = Vec::new();
    for entry in entries {
        if entry.key_equals(source, b"/Length")
            || entry.key_equals(source, b"/Filter")
            || entry.key_equals(source, b"/DecodeParms")
            || entry.key_equals(source, b"/DL")
        {
            continue;
        }
        let key = source
            .resolve(entry.key().span())
            .map_err(|_| SpikeError::FormResourceNotFound)?;
        let value = source
            .resolve(entry.value().span())
            .map_err(|_| SpikeError::FormResourceNotFound)?;
        out.extend_from_slice(key);
        out.push(b' ');
        out.extend_from_slice(value);
        out.push(b' ');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use pdf_bytes::ByteStore;
    use pdf_content::{
        ContentLimits, PageContentLimits, load_page_program_strict, parse_operation_sequence_strict,
    };
    use pdf_paint::{
        Matrix, PaintAtomKind, PaintLimits, PaintStream, interpret_stream_sequence_with_resources,
    };
    use pdf_syntax::Reference;

    use crate::plan::{
        Command, FixedPoint, GlyphChange, ObjectSelection, PlannedBody, RunRewrite, SourceAnchor,
    };
    use crate::spike_move_text::plan_command;
    use crate::spike_move_text::tests::{form_document, nested_form_fixture, page_glyphs};

    fn read(
        source: &ByteStore,
        page_index: usize,
    ) -> (
        pdf_content::PageProgram,
        Vec<Vec<pdf_content::Operation>>,
        pdf_paint::PaintGraph,
    ) {
        let page = load_page_program_strict(source, page_index, PageContentLimits::default())
            .expect("the page opens");
        let sources: Vec<&ByteStore> = page.streams.iter().map(|stream| &stream.bytes).collect();
        let operations = parse_operation_sequence_strict(&sources, ContentLimits::default())
            .expect("the page parses");
        let streams: Vec<_> = page
            .streams
            .iter()
            .zip(&operations)
            .map(|(stream, operations)| PaintStream {
                source: &stream.bytes,
                reference: stream.reference,
                operations,
            })
            .collect();
        let graph = interpret_stream_sequence_with_resources(
            &streams,
            page.page,
            &[],
            &page.resources,
            PaintLimits::default(),
        )
        .expect("the page interprets");
        drop(streams);
        (page, operations, graph)
    }

    fn inside_the_form(source: &ByteStore, text: bool) -> SourceAnchor {
        let (_, _, graph) = read(source, 0);
        let atom = graph
            .atoms
            .iter()
            .find(|atom| {
                !atom.id.invocation_path.is_empty()
                    && matches!(atom.kind, PaintAtomKind::Text(_)) == text
            })
            .expect("the Form paints one of these");
        SourceAnchor::of(&atom.id)
    }

    fn page_paint(source: &ByteStore, page_index: usize) -> Vec<String> {
        let (_, _, graph) = read(source, page_index);
        graph
            .atoms
            .iter()
            .map(|atom| pdf_paint::paint_signature(&atom.kind))
            .collect()
    }

    #[test]
    fn a_block_inside_a_shared_form_moves_on_one_page_and_leaves_the_other_alone() {
        let source = form_document(b"/Fm0 Do", b"BT /F1 12 Tf 5 5 Td (AAA) Tj ET", true);
        let before_first = page_glyphs(&source, 0);
        let before_second = page_glyphs(&source, 1);
        assert_eq!(before_first.len(), 3, "the Form paints three glyphs");

        let anchor = inside_the_form(&source, true);
        let plan = plan_command(
            &source,
            &Command::MoveTextBlock {
                page_index: 0,
                runs: vec![anchor],
                dx: 10.0,
                dy: 0.0,
            },
            b"",
        )
        .expect("a block inside a shared Form moves");
        assert_eq!(plan.writes().len(), 2, "the copy and the repointed page");
        assert!(matches!(
            plan.writes()[0].body,
            PlannedBody::NewStream { .. }
        ));
        assert_eq!(
            plan.writes()[1].reference,
            Reference::new(3, 0),
            "only page one's dictionary is rewritten"
        );

        let edited = plan.commit(&source, b"").expect("the plan commits");
        assert_eq!(
            page_glyphs(&edited, 1),
            before_second,
            "the page that shares the Form paints exactly what it painted"
        );
        let moved = page_glyphs(&edited, 0);
        assert_eq!(moved.len(), before_first.len(), "no glyph was lost");
        for (was, now) in before_first.iter().zip(&moved) {
            assert_ne!(was, now, "every glyph of the block moved");
        }
    }

    #[test]
    fn a_form_the_document_uses_once_is_edited_in_place_with_no_copy() {
        let source = form_document(b"/Fm0 Do", b"BT /F1 12 Tf 5 5 Td (AAA) Tj ET", false);
        let anchor = inside_the_form(&source, true);
        let plan = plan_command(
            &source,
            &Command::MoveTextBlock {
                page_index: 0,
                runs: vec![anchor],
                dx: 10.0,
                dy: 0.0,
            },
            b"",
        )
        .expect("a block inside a Form nobody shares moves");
        assert_eq!(plan.writes().len(), 1, "one write, and it is the Form");
        assert_eq!(plan.writes()[0].reference, Reference::new(5, 0));
        assert!(matches!(
            plan.writes()[0].body,
            PlannedBody::ReplacedStream { .. }
        ));
        assert_eq!(plan.effect().target_stream, Reference::new(5, 0));
        let edited = plan.commit(&source, b"").expect("the plan commits");
        let page = load_page_program_strict(&edited, 0, PageContentLimits::default())
            .expect("the page still opens");
        assert_eq!(
            page.resources
                .xobject(b"/Fm0")
                .and_then(pdf_content::ResourceEntry::form)
                .expect("the page still names a Form")
                .reference,
            Reference::new(5, 0),
            "the page still names the definition it always named"
        );
    }

    #[test]
    fn typing_into_a_form_run_resolves_the_font_in_the_forms_own_resources() {
        let source = form_document(b"/Fm0 Do", b"BT /F1 12 Tf 5 5 Td (AAA) Tj ET", true);
        let before_second = page_glyphs(&source, 1);
        let anchor = inside_the_form(&source, true);
        let plan = plan_command(
            &source,
            &Command::RewriteText {
                page_index: 0,
                runs: vec![RunRewrite {
                    anchor,
                    glyphs: Some(GlyphChange::Replace {
                        glyphs: 1..2,
                        text: "A".to_owned(),
                    }),
                    displace: (0.0, 0.0),
                }],
            },
            b"",
        )
        .expect("a run inside a Form is typed in");
        assert_eq!(plan.writes().len(), 2, "the copy and the repointed page");
        let edited = plan.commit(&source, b"").expect("the plan commits");
        assert_eq!(
            page_glyphs(&edited, 1),
            before_second,
            "the page that shares the Form is untouched"
        );
        assert_eq!(
            page_glyphs(&edited, 0).len(),
            3,
            "the typed run still paints three glyphs"
        );
    }

    #[test]
    fn a_runs_resource_scope_is_the_forms_when_a_form_paints_it() {
        let source = form_document(b"/Fm0 Do", b"BT /F1 12 Tf 5 5 Td (AAA) Tj ET", true);
        let (page, operations, graph) = read(&source, 0);
        assert!(
            page.resources.font(b"/F1").is_none(),
            "the page itself names no /F1, so a page lookup could only fail"
        );
        let atom = graph
            .atoms
            .iter()
            .find(|atom| matches!(atom.kind, PaintAtomKind::Text(_)))
            .expect("the Form paints text");
        let resources = super::super::scope_resources(&page, &operations, &atom.id)
            .expect("the run's scope resolves");
        assert!(
            resources.font(b"/F1").is_some(),
            "the Form's own /F1 is what the run selected"
        );
    }

    #[test]
    fn an_object_inside_a_shared_form_moves_and_leaves_the_other_page_alone() {
        let source = form_document(b"/Fm0 Do", b"1 0 0 1 10 10 cm 0 0 20 20 re f", true);
        let before_first = page_paint(&source, 0);
        let before_second = page_paint(&source, 1);
        let anchor = inside_the_form(&source, false);
        let plan = plan_command(
            &source,
            &Command::PlaceObject {
                page_index: 0,
                target: ObjectSelection::Painted(anchor),
                transform: Matrix {
                    a: 1.0,
                    b: 0.0,
                    c: 0.0,
                    d: 1.0,
                    e: 7.0,
                    f: 0.0,
                },
                about: FixedPoint::Origin,
            },
            b"",
        )
        .expect("an object inside a shared Form is placed");
        assert_eq!(plan.writes().len(), 2, "the copy and the repointed page");
        let PlannedBody::NewStream { decoded, .. } = &plan.writes()[0].body else {
            panic!("the copy is a new stream")
        };
        let written = String::from_utf8_lossy(decoded).into_owned();
        assert!(written.contains("1 0 0 1 7 0 cm "), "{written}");
        assert!(written.trim_end().ends_with(" Q"), "{written}");

        let edited = plan.commit(&source, b"").expect("the plan commits");
        assert_eq!(
            page_paint(&edited, 1),
            before_second,
            "the page that shares the Form paints exactly what it painted"
        );
        assert_ne!(
            page_paint(&edited, 0),
            before_first,
            "the page the object was moved on did change"
        );
    }

    #[test]
    fn a_form_inside_a_form_is_refused_and_says_which_form_is_in_the_way() {
        let source = nested_form_fixture();
        let anchor = inside_the_form(&source, true);
        assert_eq!(anchor.invocation_path.len(), 2, "two Forms deep");
        let refused = plan_command(
            &source,
            &Command::MoveTextBlock {
                page_index: 0,
                runs: vec![anchor],
                dx: 10.0,
                dy: 0.0,
            },
            b"",
        )
        .expect_err("a run two Forms deep is refused");
        let said = refused.to_string();
        assert!(said.contains("Form inside another Form"), "{said}");
    }
}
