use crate::color::Color;
use crate::error::InterpretErrorKind;
use crate::geometry::{FillRule, Matrix, PathSegment, Point};
use crate::graph::MarkedProperties;
use crate::interpreter::{
    PaintContext, PaintStream, interpret_operations, interpret_stream_sequence,
    interpret_stream_sequence_with_resources,
};
use crate::state::PaintLimits;
use crate::test_fixtures::{
    interpret, marked_content_fixture, optional_content_fixture,
    optional_content_fixture_with_group, path, resource_fixture,
};
use pdf_bytes::{ByteStore, SourceId};
use pdf_content::{
    ContentLimits, PageContentLimits, load_page_program_strict, parse_operations_strict,
};
use std::sync::Arc;

#[test]
fn rectangle_fill_has_stable_identity_state_and_provenance() {
    let (source, graph) = interpret(b"0.2 0.4 0.6 rg 10 20 30 40 re f").expect("valid paint");
    assert_eq!(graph.atoms.len(), 1);
    let atom = &graph.atoms[0];
    assert_eq!(atom.id.page, pdf_syntax::Reference::new(3, 0));
    assert_eq!(atom.id.stream, pdf_syntax::Reference::new(7, 0));
    assert_eq!(atom.id.ordinal, 0);
    assert_eq!(source.resolve(atom.id.operator_span), Ok(&b"f"[..]));
    let paint = path(atom);
    assert_eq!(paint.fill, Some(FillRule::Nonzero));
    assert!(!paint.stroke);
    assert_eq!(
        paint.state.fill_color.value,
        Color::DeviceRgb(0.2, 0.4, 0.6)
    );
    assert_eq!(paint.state.fill_color.provenance.len(), 1);
    assert!(matches!(
        paint.path.segments.as_slice(),
        [PathSegment::Rectangle {
            origin: Point { x: 10.0, y: 20.0 },
            width: 30.0,
            height: 40.0,
            ..
        }]
    ));
}

#[test]
fn save_and_restore_snapshot_the_graphics_state() {
    let (_, graph) = interpret(b"2 w q 9 w 0 0 1 1 re S Q 2 2 1 1 re S").expect("valid state");
    let widths: Vec<f64> = graph
        .atoms
        .iter()
        .map(|atom| path(atom).state.line_width.value)
        .collect();
    assert_eq!(widths, vec![9.0, 2.0]);
}

#[test]
fn object_scope_evidence_comes_from_the_graphics_stack_not_geometry() {
    let (_, graph) =
        interpret(b"2 0 0 3 10 20 cm q 0 0 10 10 re W n q 1 0 0 1 7 9 cm 0 0 1 1 re f Q Q")
            .expect("nested scopes");
    assert_eq!(graph.object_scopes.len(), 2);
    let inner = &graph.object_scopes[0];
    let outer = &graph.object_scopes[1];
    assert_eq!((inner.atoms.clone(), outer.atoms.clone()), (0..1, 0..1));
    assert_eq!((inner.inherited_clips, outer.inherited_clips), (1, 0));
    assert!(inner.open.start() > outer.open.start());
    assert!(inner.close.end() < outer.close.end());
    for scope in &graph.object_scopes {
        assert_eq!(
            scope.ctm.value,
            Matrix {
                a: 2.0,
                b: 0.0,
                c: 0.0,
                d: 3.0,
                e: 10.0,
                f: 20.0
            }
        );
        assert!(!scope.ctm.provenance.is_empty());
    }
    let (_, shared) = interpret(b"q 0 0 1 1 re f 3 3 1 1 re f Q").unwrap();
    assert_eq!(shared.object_scopes.len(), 1);
    assert_eq!(shared.object_scopes[0].atoms, 0..2);
}

#[test]
fn stream_sequences_keep_state_and_path_but_retain_operator_stream_identity() {
    let first = ByteStore::new(SourceId::derived(SourceId::new(12), 1), &b"q 3 w 0 0 m"[..]);
    let second = ByteStore::new(SourceId::derived(SourceId::new(12), 2), &b"1 1 l S Q"[..]);
    let first_operations =
        parse_operations_strict(&first, ContentLimits::default()).expect("first stream");
    let second_operations =
        parse_operations_strict(&second, ContentLimits::default()).expect("second stream");
    let second_reference = pdf_syntax::Reference::new(9, 0);
    let graph = interpret_stream_sequence(
        &[
            PaintStream {
                source: &first,
                reference: pdf_syntax::Reference::new(8, 0),
                operations: &first_operations,
            },
            PaintStream {
                source: &second,
                reference: second_reference,
                operations: &second_operations,
            },
        ],
        pdf_syntax::Reference::new(3, 0),
        &[],
        PaintLimits::default(),
    )
    .expect("one logical content sequence");
    assert_eq!(graph.atoms.len(), 1);
    assert_eq!(graph.atoms[0].id.stream, second_reference);
    assert!(
        graph.object_scopes.is_empty(),
        "a cross-stream scope is not writable as one span"
    );
    let paint = path(&graph.atoms[0]);
    assert!((paint.state.line_width.value - 3.0).abs() < f64::EPSILON);
    assert_eq!(paint.path.segments.len(), 2);
}

#[test]
fn black_generation_deferred_to_the_device_is_accepted_and_a_function_is_not() {
    let source = resource_fixture(
        b"/GS1 gs 0 0 1 1 re f",
        b"<< /Type /ExtGState /OPM 1 /OP false /BG2 /Default /UCR2 /Default /op false /SA false >>",
    );
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("resource-bearing page");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("content operations");
    let graph = interpret_stream_sequence_with_resources(
        &[PaintStream {
            source: &page.streams[0].bytes,
            reference: page.streams[0].reference,
            operations: &operations,
        }],
        page.page,
        &[],
        &page.resources,
        PaintLimits::default(),
    )
    .expect("a black generation deferred to the device");
    assert_eq!(graph.atoms.len(), 1);

    for entries in [
        &b"<< /Type /ExtGState /BG2 6 0 R >>"[..],
        b"<< /Type /ExtGState /BG2 /Identity >>",
        b"<< /Type /ExtGState /TR2 /Default >>",
        b"<< /Type /ExtGState /HT /Default >>",
        b"<< /Type /ExtGState /Name /Whatever >>",
    ] {
        let source = resource_fixture(b"/GS1 gs 0 0 1 1 re f", entries);
        let page = load_page_program_strict(&source, 0, PageContentLimits::default())
            .expect("resource-bearing page");
        let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
            .expect("content operations");
        let graph = interpret_stream_sequence_with_resources(
            &[PaintStream {
                source: &page.streams[0].bytes,
                reference: page.streams[0].reference,
                operations: &operations,
            }],
            page.page,
            &[],
            &page.resources,
            PaintLimits::default(),
        )
        .expect("an entry this build does not implement does not stop the page");
        assert_eq!(graph.atoms.len(), 1);
        assert!(
            graph.repairs.iter().any(|repair| matches!(
                repair.kind,
                crate::RepairKind::ExtGStateEntryIgnored { .. }
            )),
            "the skipped entry is named: {:?}",
            graph.repairs
        );
    }
}

#[test]
fn an_entry_this_build_skips_does_not_cost_the_entries_beside_it() {
    let source = resource_fixture(
        b"/GS1 gs 0 0 1 1 re f",
        b"<< /Type /ExtGState /HT /Default /ca 0.25 >>",
    );
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("resource-bearing page");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("content operations");
    let graph = interpret_stream_sequence_with_resources(
        &[PaintStream {
            source: &page.streams[0].bytes,
            reference: page.streams[0].reference,
            operations: &operations,
        }],
        page.page,
        &[],
        &page.resources,
        PaintLimits::default(),
    )
    .expect("a halftone beside an alpha");
    let alpha = crate::test_fixtures::path(&graph.atoms[0])
        .state
        .fill_alpha
        .value;
    assert!(
        (alpha - 0.25).abs() < f64::EPSILON,
        "the alpha survived the halftone: {alpha}"
    );
}

#[test]
fn gs_applies_only_declared_entries_and_retains_both_provenances() {
    let source = resource_fixture(
        b".2 g 2 w /GS1 gs 0 0 1 1 re S",
        b"<< /Type /ExtGState /LW 7 /LC 1 /D [[2 3] 4] /OP true /op false /OPM 1 /FL 8 /SM .5 /SA true /BM [/Multiply /Normal] /SMask /None /CA 1.5 /ca .25 /AIS true /TK false >>",
    );
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("resource-bearing page");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("content operations");
    let graph = interpret_stream_sequence_with_resources(
        &[PaintStream {
            source: &page.streams[0].bytes,
            reference: page.streams[0].reference,
            operations: &operations,
        }],
        page.page,
        &[],
        &page.resources,
        PaintLimits::default(),
    )
    .expect("supported ExtGState");
    let paint = path(&graph.atoms[0]);
    assert!((paint.state.line_width.value - 7.0).abs() < f64::EPSILON);
    assert_eq!(paint.state.line_width.provenance.len(), 2);
    assert_eq!(paint.state.fill_color.value, Color::DeviceGray(0.2));
    assert_eq!(paint.state.fill_color.provenance.len(), 1);
    assert_eq!(paint.state.line_cap.value, crate::LineCap::Round);
    assert_eq!(paint.state.dash.value.array, vec![2.0, 3.0]);
    assert!((paint.state.dash.value.phase - 4.0).abs() < f64::EPSILON);
    assert!(paint.state.stroke_overprint.value);
    assert!(!paint.state.fill_overprint.value);
    assert_eq!(paint.state.overprint_mode.value, 1);
    assert!((paint.state.flatness.value - 8.0).abs() < f64::EPSILON);
    assert!((paint.state.smoothness.value - 0.5).abs() < f64::EPSILON);
    assert!(paint.state.stroke_adjust.value);
    assert_eq!(
        paint.state.blend_mode.value.names,
        vec![b"/Multiply".to_vec(), b"/Normal".to_vec()]
    );
    assert!((paint.state.stroke_alpha.value - 1.0).abs() < f64::EPSILON);
    assert!((paint.state.fill_alpha.value - 0.25).abs() < f64::EPSILON);
    assert!(paint.state.alpha_is_shape.value);
    assert!(!paint.state.text_knockout.value);
    assert_eq!(
        paint.state.ext_gstate.as_ref().unwrap().value.reference,
        Some(pdf_syntax::Reference::new(5, 0))
    );
}

#[test]
fn marked_content_nests_and_attaches_its_path_to_each_atom() {
    let source = marked_content_fixture(
        b"/P1 << /MCID 7 >>",
        b"/Artifact BMC 0 0 1 1 re f \
          /Span << /ActualText (x) >> BDC 0 0 1 1 re f \
          /P /P1 BDC 0 0 1 1 re f EMC \
          EMC 0 0 1 1 re f EMC 0 0 1 1 re f",
    );
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("page with marked content");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("marked-content operations");
    let graph = interpret_stream_sequence_with_resources(
        &[PaintStream {
            source: &page.streams[0].bytes,
            reference: page.streams[0].reference,
            operations: &operations,
        }],
        page.page,
        &[],
        &page.resources,
        PaintLimits::default(),
    )
    .expect("valid marked content");
    assert_eq!(graph.atoms.len(), 5);
    let tags: Vec<Vec<&[u8]>> = graph
        .atoms
        .iter()
        .map(|atom| atom.marks.iter().map(|mark| mark.tag.as_slice()).collect())
        .collect();
    assert_eq!(
        tags,
        vec![
            vec![&b"/Artifact"[..]],
            vec![&b"/Artifact"[..], &b"/Span"[..]],
            vec![&b"/Artifact"[..], &b"/Span"[..], &b"/P"[..]],
            vec![&b"/Artifact"[..]],
            vec![],
        ]
    );
    assert!(graph.atoms[0].marks[0].properties.is_none());
    assert!(matches!(
        graph.atoms[1].marks[1].properties,
        Some(MarkedProperties::Inline(_))
    ));
    let Some(MarkedProperties::Resource {
        name,
        reference,
        dictionary_span,
        ..
    }) = &graph.atoms[2].marks[2].properties
    else {
        panic!("expected a named property list")
    };
    assert_eq!(name.as_slice(), b"/P1");
    assert_eq!(*reference, None);
    assert!(!dictionary_span.is_empty());
}

#[test]
fn marked_content_nests_independently_of_the_graphics_stack() {
    let (_, graph) = interpret(b"/Tag BMC q 0 0 1 1 re f EMC 0 0 1 1 re f Q")
        .expect("interleaved marks and graphics state");
    assert_eq!(graph.atoms.len(), 2);
    assert_eq!(graph.atoms[0].marks.len(), 1);
    assert!(graph.atoms[1].marks.is_empty());

    for (content, expected) in [
        (
            &b"/Tag BMC 0 0 1 1 re f"[..],
            InterpretErrorKind::UnbalancedMarkedContent { depth: 1 },
        ),
        (
            &b"/Tag /Missing BDC"[..],
            InterpretErrorKind::ResourceScopeMissing,
        ),
        (
            &b"/Tag 7 BDC"[..],
            InterpretErrorKind::InvalidMarkedContentProperties,
        ),
        (&b"7 BMC"[..], InterpretErrorKind::OperandType),
    ] {
        let error = interpret(content).expect_err("malformed marked content must fail");
        assert_eq!(
            error.kind(),
            expected,
            "{:?}",
            String::from_utf8_lossy(content)
        );
    }
}
#[test]
fn a_hidden_optional_content_section_paints_nothing_and_the_rest_still_paints() {
    let interpret_source = |source: &ByteStore| {
        let page = load_page_program_strict(source, 0, PageContentLimits::default())
            .expect("page with optional content");
        let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
            .expect("optional-content operations");
        interpret_stream_sequence_with_resources(
            &[PaintStream {
                source: &page.streams[0].bytes,
                reference: page.streams[0].reference,
                operations: &operations,
            }],
            page.page,
            &[],
            &page.resources,
            PaintLimits::default(),
        )
    };
    let content: &[u8] = b"0 0 1 1 re f /OC /P1 BDC 2 2 1 1 re f EMC 4 4 1 1 re f";
    let properties: &[u8] = b"/P1 5 0 R";

    let off = optional_content_fixture(b"/OFF [5 0 R]", properties, content, b"");
    let graph = interpret_source(&off).expect("a hidden section is not a failure");
    assert_eq!(graph.atoms.len(), 2, "the two outside it");

    let on = optional_content_fixture(b"/ON [5 0 R]", properties, content, b"");
    let graph = interpret_source(&on).expect("a visible section");
    assert_eq!(graph.atoms.len(), 3);
    assert_eq!(graph.atoms[1].marks.len(), 1, "and it is still structure");

    let silent = optional_content_fixture(b"", properties, content, b"");
    assert_eq!(
        interpret_source(&silent)
            .expect("no configuration")
            .atoms
            .len(),
        3
    );

    let base_off = optional_content_fixture(b"/BaseState /OFF", properties, content, b"");
    assert_eq!(
        interpret_source(&base_off).expect("base off").atoms.len(),
        2
    );
    let rescued =
        optional_content_fixture(b"/BaseState /OFF /ON [5 0 R]", properties, content, b"");
    assert_eq!(interpret_source(&rescued).expect("rescued").atoms.len(), 3);

    let membership: &[u8] = b"/P1 << /Type /OCMD /OCGs [5 0 R 6 0 R] /P /AllOn >>";
    let one_off = optional_content_fixture(b"/OFF [6 0 R]", membership, content, b"");
    assert_eq!(
        interpret_source(&one_off)
            .expect("AllOn with one off")
            .atoms
            .len(),
        2
    );
    let any_on: &[u8] = b"/P1 << /Type /OCMD /OCGs [5 0 R 6 0 R] /P /AnyOn >>";
    let one_off = optional_content_fixture(b"/OFF [6 0 R]", any_on, content, b"");
    assert_eq!(
        interpret_source(&one_off)
            .expect("AnyOn with one off")
            .atoms
            .len(),
        3
    );

    let form: &[u8] = b"0 0 1 1 re f /Fm1 Do";
    let hidden_form = optional_content_fixture(b"/OFF [5 0 R]", properties, form, b"/OC 5 0 R");
    assert_eq!(
        interpret_source(&hidden_form)
            .expect("a hidden form")
            .atoms
            .len(),
        1
    );
    let shown_form = optional_content_fixture(b"/ON [5 0 R]", properties, form, b"/OC 5 0 R");
    assert_eq!(
        interpret_source(&shown_form)
            .expect("a shown form")
            .atoms
            .len(),
        2
    );

    let expression: &[u8] = b"/P1 << /Type /OCMD /OCGs [5 0 R] /VE [/Not 5 0 R] >>";
    let error = interpret_source(&optional_content_fixture(
        b"/ON [5 0 R]",
        expression,
        content,
        b"",
    ))
    .expect_err("a visibility expression");
    assert_eq!(error.kind(), InterpretErrorKind::UnsupportedOptionalContent);
}

#[test]
fn marked_points_validate_their_operands_and_paint_nothing() {
    let source = marked_content_fixture(
        b"/P1 << /MCID 0 >>",
        b"/Tag MP /Tag << /A 1 >> DP /Tag /P1 DP 0 0 1 1 re f",
    );
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("page with marked points");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("marked-point operations");
    let graph = interpret_stream_sequence_with_resources(
        &[PaintStream {
            source: &page.streams[0].bytes,
            reference: page.streams[0].reference,
            operations: &operations,
        }],
        page.page,
        &[],
        &page.resources,
        PaintLimits::default(),
    )
    .expect("valid marked points");
    assert_eq!(graph.atoms.len(), 1);
    assert!(graph.atoms[0].marks.is_empty());

    let error = interpret(b"/Tag /Missing DP").expect_err("a dangling name must fail");
    assert_eq!(error.kind(), InterpretErrorKind::ResourceScopeMissing);
}

#[test]
fn marked_content_depth_is_bounded() {
    let mut content = Vec::new();
    for _ in 0..5 {
        content.extend_from_slice(b"/Tag BMC ");
    }
    let limits = PaintLimits {
        max_marked_content_depth: 4,
        ..PaintLimits::default()
    };
    let source = ByteStore::new(
        SourceId::derived(SourceId::new(9), 2),
        Arc::<[u8]>::from(content),
    );
    let operations =
        parse_operations_strict(&source, ContentLimits::default()).expect("operations");
    let error = interpret_operations(
        &source,
        &operations,
        &PaintContext::page_stream(
            pdf_syntax::Reference::new(3, 0),
            pdf_syntax::Reference::new(7, 0),
        ),
        limits,
    )
    .expect_err("nesting limit must be enforced");
    assert_eq!(error.kind(), InterpretErrorKind::MarkedContentDepthLimit);
}

#[test]
fn cm_concatenates_in_pdf_order() {
    let (_, graph) =
        interpret(b"2 0 0 3 0 0 cm 1 0 0 1 10 20 cm 0 0 1 1 re S").expect("valid matrix");
    let paint = path(&graph.atoms[0]);
    assert_eq!(
        paint.state.ctm.value,
        Matrix {
            a: 2.0,
            b: 0.0,
            c: 0.0,
            d: 3.0,
            e: 20.0,
            f: 60.0,
        }
    );
    assert_eq!(
        paint.state.ctm.value.transform(Point { x: 1.0, y: 1.0 }),
        Point { x: 22.0, y: 63.0 }
    );
    assert_eq!(paint.state.ctm.provenance.len(), 2);
}

#[test]
fn clipping_applies_after_the_path_ending_operation() {
    let (_, graph) =
        interpret(b"0 0 m 1 2 3 4 v W n 2 0 0 2 0 0 cm 10 10 2 2 re S").expect("valid clip");
    assert_eq!(graph.atoms.len(), 1);
    let paint = path(&graph.atoms[0]);
    assert_eq!(paint.state.clip_paths.len(), 1);
    assert_eq!(paint.state.clip_paths[0].rule, FillRule::Nonzero);
    assert_eq!(paint.state.clip_paths[0].ctm.value, Matrix::IDENTITY);
    assert_ne!(paint.state.ctm.value, Matrix::IDENTITY);
    assert!(matches!(
        paint.state.clip_paths[0].path.segments[1],
        PathSegment::CubicTo {
            control_1: Point { x: 0.0, y: 0.0 },
            control_2: Point { x: 1.0, y: 2.0 },
            end: Point { x: 3.0, y: 4.0 },
            ..
        }
    ));
}

#[test]
fn compatibility_sections_ignore_only_unknown_operators_and_balance() {
    let (_, graph) = interpret(b"BX 1 2 FutureOp BX 3 NewerOp EX 0 0 1 1 re f EX")
        .expect("balanced compatibility sections");
    assert_eq!(graph.atoms.len(), 1);

    let (_, graph) = interpret(b"BT BX 0 0 m EX ET")
        .expect("a misplaced path operator no longer refuses the page");
    assert!(
        graph
            .repairs
            .iter()
            .any(|repair| matches!(repair.kind, crate::RepairKind::OperatorInsideTextObject))
    );
}

#[test]
fn malformed_state_and_unsupported_operators_fail_closed() {
    for (bytes, expected) in [
        (
            &b"BT (not decoded yet) Tj ET"[..],
            InterpretErrorKind::TextFontMissing,
        ),
        (&b"EX"[..], InterpretErrorKind::CompatibilitySectionMissing),
        (
            &b"BX"[..],
            InterpretErrorKind::UnbalancedCompatibilitySection { depth: 1 },
        ),
        (
            &b"1 l"[..],
            InterpretErrorKind::OperandCount {
                expected: 2,
                actual: 1,
            },
        ),
    ] {
        let error = interpret(Arc::<[u8]>::from(bytes).as_ref()).expect_err("operation must fail");
        assert_eq!(error.kind(), expected);
    }
}

#[test]
fn configured_limits_are_enforced() {
    let source = ByteStore::new(SourceId::new(11), &b"0 0 m 1 1 l"[..]);
    let operations = parse_operations_strict(&source, ContentLimits::default()).unwrap();
    let error = interpret_operations(
        &source,
        &operations,
        &PaintContext::page_stream(
            pdf_syntax::Reference::new(1, 0),
            pdf_syntax::Reference::new(2, 0),
        ),
        PaintLimits {
            max_path_segments: 1,
            ..PaintLimits::default()
        },
    )
    .expect_err("segment limit");
    assert_eq!(error.kind(), InterpretErrorKind::PathSegmentLimit);
}

#[test]
fn a_groups_own_usage_answers_ahead_of_the_configuration() {
    let interpret_source = |source: &ByteStore| {
        let page = load_page_program_strict(source, 0, PageContentLimits::default())
            .expect("page with optional content");
        let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
            .expect("optional-content operations");
        interpret_stream_sequence_with_resources(
            &[PaintStream {
                source: &page.streams[0].bytes,
                reference: page.streams[0].reference,
                operations: &operations,
            }],
            page.page,
            &[],
            &page.resources,
            PaintLimits::default(),
        )
        .expect("optional content is never a failure")
        .atoms
        .len()
    };
    let content: &[u8] = b"0 0 1 1 re f /OC /P1 BDC 2 2 1 1 re f EMC 4 4 1 1 re f";
    let properties: &[u8] = b"/P1 5 0 R";
    let case = |configuration: &[u8], group: &[u8]| {
        interpret_source(&optional_content_fixture_with_group(
            configuration,
            properties,
            content,
            b"",
            group,
        ))
    };

    let off = b"/Usage << /View << /ViewState /OFF >> >>";
    assert_eq!(case(b"/ON [5 0 R]", off), 2, "the watermark case");
    assert_eq!(case(b"", off), 2, "with no configuration at all");
    assert_eq!(
        case(b"/OFF [5 0 R]", b"/Usage << /View << /ViewState /ON >> >>"),
        3,
        "and it answers in the other direction too"
    );

    assert_eq!(
        case(
            b"/OFF [5 0 R]",
            b"/Usage << /Print << /PrintState /ON >> >>"
        ),
        2
    );
    assert_eq!(case(b"/OFF [5 0 R]", b"/Usage << >>"), 2);

    assert_eq!(case(b"/OFF [5 0 R]", b"/Intent /Design"), 3);
    assert_eq!(case(b"/OFF [5 0 R]", b"/Intent [/Design /All]"), 2);
    assert_eq!(case(b"/OFF [5 0 R]", b"/Intent [/Design]"), 3);
}

#[test]
fn the_configurations_usage_application_array_is_applied_for_view_events() {
    let interpret_source = |source: &ByteStore| {
        let page = load_page_program_strict(source, 0, PageContentLimits::default())
            .expect("page with optional content");
        let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
            .expect("optional-content operations");
        interpret_stream_sequence_with_resources(
            &[PaintStream {
                source: &page.streams[0].bytes,
                reference: page.streams[0].reference,
                operations: &operations,
            }],
            page.page,
            &[],
            &page.resources,
            PaintLimits::default(),
        )
        .expect("optional content is never a failure")
        .atoms
        .len()
    };
    let content: &[u8] = b"0 0 1 1 re f /OC /P1 BDC 2 2 1 1 re f EMC 4 4 1 1 re f";
    let properties: &[u8] = b"/P1 5 0 R";
    let case = |configuration: &[u8]| {
        interpret_source(&optional_content_fixture(
            configuration,
            properties,
            content,
            b"",
        ))
    };

    assert_eq!(
        case(b"/ON [5 0 R] /AS [<< /Event /View /OCGs [5 0 R] /View << /ViewState /OFF >> >>]"),
        2,
        "a view application overrides /ON"
    );
    assert_eq!(
        case(b"/OFF [5 0 R] /AS [<< /Event /View /OCGs [5 0 R] /View << /ViewState /ON >> >>]"),
        3,
        "and overrides /OFF"
    );
    assert_eq!(
        case(b"/ON [5 0 R] /AS [<< /Event /Print /OCGs [5 0 R] /View << /ViewState /OFF >> >>]"),
        3,
        "a print application says nothing about a screen"
    );
    assert_eq!(
        case(b"/ON [5 0 R] /AS [<< /Event /View /OCGs [6 0 R] /View << /ViewState /OFF >> >>]"),
        3,
        "and it applies only to the groups it names"
    );
    assert_eq!(
        case(b"/ON [5 0 R] /AS [<< /Event /View /OCGs [5 0 R] >>]"),
        3,
        "an entry with no /View state dictionary says nothing"
    );
    assert_eq!(
        case(b"/ON [5 0 R] /AS [<< /OCGs [5 0 R] /View << /ViewState /OFF >> >>]"),
        2,
        "an absent /Event is /View"
    );
    assert_eq!(
        case(
            b"/ON [5 0 R] /AS [<< /Event /View /OCGs [5 0 R] /View << /ViewState /OFF >> >> \
              << /Event /View /OCGs [5 0 R] /View << /ViewState /ON >> >>]"
        ),
        3,
        "a later entry overrides an earlier one"
    );
}

#[test]
fn paper_asks_the_print_states() {
    let count = |source: &ByteStore, medium: pdf_content::Medium| {
        let page = pdf_content::load_page_program_for(
            source,
            0,
            PageContentLimits::default(),
            b"",
            medium,
        )
        .expect("page with optional content");
        assert_eq!(page.resources.optional_content().medium(), medium);
        let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
            .expect("optional-content operations");
        interpret_stream_sequence_with_resources(
            &[PaintStream {
                source: &page.streams[0].bytes,
                reference: page.streams[0].reference,
                operations: &operations,
            }],
            page.page,
            &[],
            &page.resources,
            PaintLimits::default(),
        )
        .expect("optional content is never a failure")
        .atoms
        .len()
    };
    let content: &[u8] = b"0 0 1 1 re f /OC /P1 BDC 2 2 1 1 re f EMC 4 4 1 1 re f";
    let properties: &[u8] = b"/P1 5 0 R";
    let both = |configuration: &[u8], group: &[u8]| {
        let source =
            optional_content_fixture_with_group(configuration, properties, content, b"", group);
        (
            count(&source, pdf_content::Medium::Screen),
            count(&source, pdf_content::Medium::Print),
        )
    };
    assert_eq!(
        both(
            b"/ON [5 0 R]",
            b"/Usage << /View << /ViewState /OFF >> /Print << /PrintState /ON >> >>"
        ),
        (2, 3),
        "a watermark for paper only"
    );
    assert_eq!(
        both(
            b"/ON [5 0 R]",
            b"/Usage << /Print << /PrintState /OFF >> >>"
        ),
        (3, 2),
        "a note for the screen only"
    );
    assert_eq!(
        both(b"/ON [5 0 R]", b"/Usage << /View << /ViewState /OFF >> >>"),
        (2, 2),
        "silent about paper, the group answers with its screen state"
    );
    assert_eq!(
        both(
            b"/ON [5 0 R] /AS [<< /Event /Print /OCGs [5 0 R] /Print << /PrintState /OFF >> >>]",
            b""
        ),
        (3, 2),
        "a print application applies to paper and only to paper"
    );
    assert_eq!(
        both(
            b"/ON [5 0 R] /AS [<< /OCGs [5 0 R] /View << /ViewState /OFF >> >>]",
            b""
        ),
        (2, 3),
        "an absent /Event is /View, which paper does not answer to"
    );
    assert_eq!(
        both(
            b"/ON [5 0 R] /AS [<< /OCGs [5 0 R] /Print << /PrintState /OFF >> >>]",
            b""
        ),
        (3, 3),
        "nor does its print state: an entry is for paper only when /Event says so"
    );
    assert_eq!(both(b"/OFF [5 0 R]", b""), (2, 2), "/OFF hides on both");
}

#[test]
fn an_unsupported_operator_is_dropped_only_on_the_tolerant_path() {
    let read = |source: &ByteStore| {
        let page = load_page_program_strict(source, 0, PageContentLimits::default())
            .expect("the page opens");
        let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
            .expect("the operations parse");
        (page, operations)
    };
    let content: &[u8] = b"0 0 1 1 re f /Nope cs 2 2 1 1 re f";
    let source = resource_fixture(content, b"<< /Type /ExtGState /LW 2 >>");
    let (page, operations) = read(&source);
    let streams = [PaintStream {
        source: &page.streams[0].bytes,
        reference: page.streams[0].reference,
        operations: &operations,
    }];

    let strict = interpret_stream_sequence_with_resources(
        &streams,
        page.page,
        &[],
        &page.resources,
        PaintLimits::default(),
    )
    .expect_err("the strict door still refuses the whole page");
    assert!(
        strict.kind().is_skippable(),
        "and it refuses something it could have skipped"
    );

    let graph = crate::interpret_stream_sequence_tolerating_unsupported(
        &streams,
        page.page,
        &[],
        &page.resources,
        PaintLimits::default(),
        None,
    )
    .expect("the tolerant door draws the page");
    assert_eq!(graph.atoms.len(), 2, "both fills, one on each side of it");
    assert_eq!(
        graph.skipped.len(),
        1,
        "and the loss is reported, not hidden"
    );
    assert_eq!(graph.skipped[0].kind(), strict.kind());
}

#[test]
fn a_form_that_fails_part_way_leaves_the_page_able_to_go_on() {
    let source = crate::test_fixtures::failing_form_fixture();
    let page =
        load_page_program_strict(&source, 0, PageContentLimits::default()).expect("the page opens");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("the operations parse");
    let streams = [crate::PaintStream {
        source: &page.streams[0].bytes,
        reference: page.streams[0].reference,
        operations: &operations,
    }];
    let graph = crate::interpret_stream_sequence_tolerating_unsupported(
        &streams,
        page.page,
        &[],
        &page.resources,
        PaintLimits::default(),
        None,
    )
    .expect("the page after the form is still drawn");
    assert_eq!(
        graph.atoms.len(),
        1,
        "the page's own fill, and nothing of the form"
    );
    assert_eq!(graph.skipped.len(), 1, "the form's failure is reported");
}

#[test]
fn losing_track_of_the_program_still_refuses_on_the_tolerant_path() {
    let read = |source: &ByteStore| {
        let page = load_page_program_strict(source, 0, PageContentLimits::default())
            .expect("the page opens");
        let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
            .expect("the operations parse");
        (page, operations)
    };
    for (content, expected) in [
        (
            &b"EX 0 0 1 1 re f"[..],
            InterpretErrorKind::CompatibilitySectionMissing,
        ),
        (
            &b"BX 0 0 1 1 re f"[..],
            InterpretErrorKind::UnbalancedCompatibilitySection { depth: 1 },
        ),
    ] {
        let source = resource_fixture(content, b"<< /Type /ExtGState >>");
        let (page, operations) = read(&source);
        let streams = [PaintStream {
            source: &page.streams[0].bytes,
            reference: page.streams[0].reference,
            operations: &operations,
        }];
        let error = crate::interpret_stream_sequence_tolerating_unsupported(
            &streams,
            page.page,
            &[],
            &page.resources,
            PaintLimits::default(),
            None,
        )
        .expect_err("this is not an operator to drop");
        assert_eq!(error.kind(), expected);
        assert!(!expected.is_skippable());
    }
}

#[test]
fn a_tf_naming_no_font_binds_the_standard_font_and_says_so() {
    for content in [
        &b"BT /Nope 12 Tf (H) Tj ET"[..],
        b"BT (F1) 12 Tf (H) Tj ET",
        b"BT 1 12 Tf (H) Tj ET",
    ] {
        let source = resource_fixture(content, b"<< /Type /ExtGState /LW 2 >>");
        let page = load_page_program_strict(&source, 0, PageContentLimits::default())
            .expect("resource-bearing page");
        let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
            .expect("content operations");
        let graph = interpret_stream_sequence_with_resources(
            &[PaintStream {
                source: &page.streams[0].bytes,
                reference: page.streams[0].reference,
                operations: &operations,
            }],
            page.page,
            &[],
            &page.resources,
            PaintLimits::default(),
        )
        .expect("a font the page does not have does not stop the text");
        assert_eq!(graph.atoms.len(), 1, "the run was shown");
        assert!(
            graph.repairs.iter().any(|repair| matches!(
                repair.kind,
                crate::RepairKind::FontNameBoundToStandardFont
            )),
            "the binding is reported: {:?}",
            graph.repairs
        );
    }
}

#[test]
fn a_skipped_painting_operator_does_not_leave_its_path_behind() {
    let source = resource_fixture(
        b"/Pattern cs 0 0 1 1 re f 0 g 4 4 1 1 re f",
        b"<< /Type /ExtGState /LW 2 >>",
    );
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("resource-bearing page");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("content operations");
    let graph = crate::interpret_stream_sequence_tolerating_unsupported(
        &[PaintStream {
            source: &page.streams[0].bytes,
            reference: page.streams[0].reference,
            operations: &operations,
        }],
        page.page,
        &[],
        &page.resources,
        PaintLimits::default(),
        None,
    )
    .expect("the tolerant door keeps the page");
    assert_eq!(graph.skipped.len(), 1, "the pattern fill was skipped");
    assert_eq!(graph.atoms.len(), 1, "and only the second fill was painted");
    let painted = crate::test_fixtures::path(&graph.atoms[0]);
    assert_eq!(
        painted.path.segments.len(),
        1,
        "with one rectangle, not two: {:?}",
        painted.path.segments
    );
}
