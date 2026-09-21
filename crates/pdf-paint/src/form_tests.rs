use crate::color::ColorSpace;
use crate::error::InterpretErrorKind;
use crate::geometry::{Matrix, PathSegment, Point};
use crate::graph::{GroupBackdrop, PaintAtomKind};
use crate::interpreter::{PaintStream, interpret_stream_sequence_with_resources};
use crate::state::{PaintLimits, SoftMask, SoftMaskSubtype, SoftMaskTransfer};
use crate::test_fixtures::{assert_floats, form_paint_fixture, path, soft_mask_fixture};
use pdf_bytes::ByteStore;
use pdf_content::{
    ContentLimits, PageContentLimits, load_page_program_strict, parse_operations_strict,
};

#[test]
fn a_soft_mask_backdrop_may_be_written_indirectly() {
    let direct = soft_mask_fixture(b"/Type /Mask /S /Alpha /G 6 0 R /BC [.1 .2 .3]", true);
    let indirect = soft_mask_fixture(b"/Type /Mask /S /Alpha /G 6 0 R /BC 8 0 R", true);
    let read = |source: &ByteStore| {
        let page = load_page_program_strict(source, 0, PageContentLimits::default())
            .expect("page with soft mask");
        let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
            .expect("page operations");
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
        .expect("soft mask resolves");
        let SoftMask::Dictionary(mask) = &path(&graph.atoms[0]).state.soft_mask.value else {
            panic!("expected a soft-mask dictionary")
        };
        mask.backdrop_color
            .as_ref()
            .expect("backdrop colour")
            .value
            .clone()
    };
    assert_floats(&read(&direct), &[0.1, 0.2, 0.3]);
    assert_floats(&read(&indirect), &[0.1, 0.2, 0.3]);
}

#[test]
fn soft_mask_retains_group_subtype_backdrop_and_identity_transfer() {
    let source = soft_mask_fixture(
        b"/Type /Mask /S /Alpha /G 6 0 R /BC [.1 .2 .3] /TR /Identity",
        true,
    );
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("page with soft mask");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("page operations");
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
    .expect("soft-mask paint graph");
    let paint = path(&graph.atoms[0]);
    let SoftMask::Dictionary(mask) = &paint.state.soft_mask.value else {
        panic!("soft mask dictionary was discarded")
    };
    assert_eq!(mask.subtype.value, SoftMaskSubtype::Alpha);
    assert_eq!(mask.transfer.value, SoftMaskTransfer::Identity);
    assert_eq!(mask.subtype.provenance.len(), 1);
    assert_eq!(mask.transfer.provenance.len(), 1);
    let backdrop = mask.backdrop_color.as_ref().expect("explicit backdrop");
    for (actual, expected) in backdrop.value.iter().zip([0.1, 0.2, 0.3]) {
        assert!((actual - expected).abs() < f64::EPSILON);
    }
    assert_eq!(mask.group.reference, pdf_syntax::Reference::new(6, 0));
    assert_eq!(mask.group.backdrop.value, GroupBackdrop::Transparent);
    assert_eq!(mask.group.graph.atoms.len(), 1);
    assert_eq!(mask.group.graph.atoms[0].id.invocation_path.len(), 1);
    assert_eq!(mask.group.graph.atoms[0].id.stream, mask.group.reference);
    assert!(mask.dictionary_reference_span.is_some());
    assert_eq!(paint.state.soft_mask.provenance.len(), 3);
}

#[test]
fn malformed_soft_mask_and_transfer_functions_fail_closed() {
    for (entries, expected) in [
        (
            &b"/S /Alpha /G 6 0 R /BC [0 0]"[..],
            InterpretErrorKind::InvalidSoftMaskEntry,
        ),
        (
            &b"/S /Alpha /G 6 0 R /TR /NotIdentity"[..],
            InterpretErrorKind::UnsupportedSoftMaskTransfer,
        ),
        (
            &b"/S /Unknown /G 6 0 R"[..],
            InterpretErrorKind::InvalidSoftMaskEntry,
        ),
    ] {
        let source = soft_mask_fixture(entries, false);
        let page = load_page_program_strict(&source, 0, PageContentLimits::default())
            .expect("page with soft-mask boundary");
        let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
            .expect("page operations");
        let error = interpret_stream_sequence_with_resources(
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
        .expect_err("unsafe soft-mask semantics must be refused");
        assert_eq!(error.kind(), expected);
    }
}

#[test]
fn form_invocations_share_definition_but_have_distinct_identity_and_scoped_state() {
    let source = form_paint_fixture(b"", None);
    let page =
        load_page_program_strict(&source, 0, PageContentLimits::default()).expect("page with Form");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("page operations");
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
    .expect("Form paint");
    assert_eq!(graph.atoms.len(), 3);
    let first_form = path(&graph.atoms[0]);
    let page_path = path(&graph.atoms[1]);
    let second_form = path(&graph.atoms[2]);
    assert_eq!(graph.atoms[0].id.stream, pdf_syntax::Reference::new(5, 0));
    assert_eq!(graph.atoms[0].id.invocation_path.len(), 1);
    assert_eq!(graph.atoms[2].id.invocation_path.len(), 1);
    assert_eq!(
        graph.atoms[0].id.invocation_path[0].form,
        graph.atoms[2].id.invocation_path[0].form
    );
    assert_ne!(
        graph.atoms[0].id.invocation_path[0].operator_span,
        graph.atoms[2].id.invocation_path[0].operator_span
    );
    assert!((first_form.state.fill_alpha.value - 0.5).abs() < f64::EPSILON);
    assert_eq!(first_form.state.clip_paths.len(), 1);
    assert_eq!(
        first_form.state.clip_paths[0].ctm.value,
        first_form.state.ctm.value
    );
    assert_eq!(
        first_form.state.ctm.value,
        Matrix {
            a: 2.0,
            b: 0.0,
            c: 0.0,
            d: 2.0,
            e: 16.0,
            f: 28.0,
        }
    );
    assert!(matches!(
        first_form.state.clip_paths[0].path.segments.as_slice(),
        [PathSegment::Rectangle {
            origin: Point { x: 0.0, y: 0.0 },
            width: 5.0,
            height: 6.0,
            ..
        }]
    ));
    assert_eq!(first_form.state.ctm.value, second_form.state.ctm.value);
    assert!((page_path.state.fill_alpha.value - 1.0).abs() < f64::EPSILON);
    assert!(page_path.state.clip_paths.is_empty());
}

#[test]
fn transparency_group_preserves_compositing_boundary_and_metadata() {
    let source = form_paint_fixture(
        b"/Group << /Type /Group /S /Transparency /I true /K true /CS [/CalGray << /WhitePoint [.9505 1 1.089] >>] >>",
        None,
    );
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("page with transparency group");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("page operations");
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
    .expect("supported transparency group");
    assert_eq!(graph.atoms.len(), 3);
    let PaintAtomKind::TransparencyGroup(group) = &graph.atoms[0].kind else {
        panic!("Form group was flattened into the parent graph")
    };
    assert_eq!(group.reference, pdf_syntax::Reference::new(5, 0));
    assert_floats(&group.bbox, &[0.0, 0.0, 5.0, 6.0]);
    assert!(group.isolated.value);
    assert!(group.knockout.value);
    assert_eq!(group.isolated.provenance.len(), 1);
    assert_eq!(group.knockout.provenance.len(), 1);
    assert_eq!(group.backdrop.value, GroupBackdrop::Transparent);
    assert_eq!(group.backdrop.provenance, group.isolated.provenance);
    let Some(ColorSpace::CalGray(definition)) =
        group.blend_space.as_ref().map(|space| &space.value)
    else {
        panic!("expected calibrated group blend space")
    };
    assert!((definition.gamma.value - 1.0).abs() < f64::EPSILON);
    assert_eq!(group.graph.atoms.len(), 1);
    assert_eq!(group.graph.atoms[0].id.invocation_path.len(), 1);
    assert_eq!(group.graph.atoms[0].id.stream, group.reference);
    assert!(matches!(graph.atoms[1].kind, PaintAtomKind::Path(_)));
    assert!(matches!(
        graph.atoms[2].kind,
        PaintAtomKind::TransparencyGroup(_)
    ));
}

#[test]
fn a_group_blending_space_may_itself_be_written_indirectly() {
    for (group, referenced) in [
        (
            &b"/Group << /Type /Group /S /Transparency /CS 6 0 R >>"[..],
            &b"/DeviceCMYK"[..],
        ),
        (
            b"/Group << /Type /Group /S /Transparency /CS 6 0 R >>",
            b"[/CalRGB << /WhitePoint [0.9505 1.0 1.089] >>]",
        ),
    ] {
        let source = form_paint_fixture(group, Some(referenced));
        let page = load_page_program_strict(&source, 0, PageContentLimits::default())
            .expect("page with an indirect group colour space");
        let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
            .expect("page operations");
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
        .expect("an indirect group blending space resolves");
        let PaintAtomKind::TransparencyGroup(resolved) = &graph.atoms[0].kind else {
            panic!("expected transparency group")
        };
        assert!(resolved.blend_space.is_some());
    }

    let source = form_paint_fixture(
        b"/Group << /Type /Group /S /Transparency /CS 6 0 R >>",
        Some(b"[/Separation /All /DeviceGray << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>]"),
    );
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("page with a Separation group space");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("page operations");
    let error = interpret_stream_sequence_with_resources(
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
    .expect_err("a Separation space cannot be a blending space");
    assert_eq!(error.kind(), InterpretErrorKind::UnsupportedGroupColorSpace);
}

#[test]
fn transparency_group_resolves_one_indirect_dictionary_with_provenance() {
    let source = form_paint_fixture(
        b"/Group 6 0 R",
        Some(b"<< /Type /Group /S /Transparency /I false /K false /CS /DeviceCMYK >>"),
    );
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("page with indirect transparency group");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("page operations");
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
    .expect("indirect group dictionary");
    let PaintAtomKind::TransparencyGroup(group) = &graph.atoms[0].kind else {
        panic!("expected transparency group")
    };
    assert_eq!(
        group.blend_space.as_ref().map(|space| &space.value),
        Some(&ColorSpace::DeviceCmyk)
    );
    let reference_span = group
        .group_reference_span
        .expect("Form dictionary reference provenance");
    assert_ne!(reference_span, group.group_span);
    assert_eq!(group.group_span.source(), group.subtype_span.source());
    assert!(group.group_span.start() <= group.subtype_span.start());
    assert!(group.group_span.end() >= group.subtype_span.end());
}

#[test]
fn malformed_or_unsupported_form_groups_fail_closed() {
    for (group, expected) in [
        (
            &b"/Group << /S /NotTransparency >>"[..],
            InterpretErrorKind::UnsupportedGroupSubtype,
        ),
        (
            &b"/Group << /S /Transparency /I 1 >>"[..],
            InterpretErrorKind::InvalidGroupEntry,
        ),
        (
            &b"/Group << /S /Transparency /CS [/Lab << /WhitePoint [1 1 1] >>] >>"[..],
            InterpretErrorKind::UnsupportedGroupColorSpace,
        ),
        (
            &b"/Group << /S /Transparency /CS [/Indexed /DeviceRGB 0 <000000>] >>"[..],
            InterpretErrorKind::UnsupportedGroupColorSpace,
        ),
    ] {
        let source = form_paint_fixture(group, None);
        let page = load_page_program_strict(&source, 0, PageContentLimits::default())
            .expect("page with malformed group metadata");
        let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
            .expect("page operations");
        let error = interpret_stream_sequence_with_resources(
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
        .expect_err("ambiguous group semantics must be refused");
        assert_eq!(error.kind(), expected);
    }
}
