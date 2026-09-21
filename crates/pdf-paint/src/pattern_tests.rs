use crate::color::Color;
use crate::error::InterpretErrorKind;
use crate::interpreter::{PaintStream, interpret_stream_sequence_with_resources};
use crate::state::PaintLimits;
use crate::test_fixtures::{
    interpret, interpret_fixture, path, pattern_paint_fixture, shading_pattern_fixture,
    uncoloured_pattern_paint_fixture,
};
use pdf_content::{
    ContentLimits, PageContentLimits, load_page_program_strict, parse_operations_strict,
};

#[test]
fn coloured_tiling_pattern_is_a_reusable_sourced_paint_graph() {
    let source = pattern_paint_fixture(1);
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("page with tiling pattern");
    assert_eq!(page.resources.patterns().len(), 1);
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
    .expect("coloured tiling pattern");
    let outer = path(&graph.atoms[0]);
    let Color::TilingPattern(pattern) = &outer.state.fill_color.value else {
        panic!("selected fill is not a tiling pattern")
    };
    assert_eq!(pattern.reference, pdf_syntax::Reference::new(5, 0));
    assert_eq!(pattern.paint_type.value, 1);
    assert_eq!(pattern.tiling_type.value, 2);
    for (actual, expected) in pattern.bbox.iter().zip([0.0, 0.0, 2.0, 2.0]) {
        assert!((actual - expected).abs() < f64::EPSILON);
    }
    assert!((pattern.x_step.value - 2.0).abs() < f64::EPSILON);
    assert!((pattern.y_step.value - 2.0).abs() < f64::EPSILON);
    assert!((pattern.matrix.value.e - 3.0).abs() < f64::EPSILON);
    assert!((pattern.matrix.value.f - 4.0).abs() < f64::EPSILON);
    assert_eq!(outer.state.fill_color.provenance.len(), 2);
    assert_eq!(pattern.graph.atoms.len(), 1);
    assert_eq!(pattern.graph.atoms[0].id.stream, pattern.reference);
    assert_eq!(pattern.graph.atoms[0].id.pattern_path.len(), 1);
    assert_eq!(
        pattern.graph.atoms[0].id.pattern_path[0].pattern,
        pattern.reference
    );
    assert_eq!(
        path(&pattern.graph.atoms[0]).state.fill_color.value,
        Color::DeviceGray(0.25)
    );
}

#[test]
fn uncoloured_pattern_and_pattern_limits_fail_closed() {
    let source = pattern_paint_fixture(2);
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("page with uncoloured tiling pattern");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("page operations");
    let stream = PaintStream {
        source: &page.streams[0].bytes,
        reference: page.streams[0].reference,
        operations: &operations,
    };
    let error = interpret_stream_sequence_with_resources(
        &[stream],
        page.page,
        &[],
        &page.resources,
        PaintLimits::default(),
    )
    .expect_err("an uncoloured pattern with no base colour has no colour");
    assert_eq!(
        error.kind(),
        InterpretErrorKind::UncoloredPatternUnsupported
    );

    let source = pattern_paint_fixture(1);
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("page with coloured tiling pattern");
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
        PaintLimits {
            max_pattern_invocations: 0,
            ..PaintLimits::default()
        },
    )
    .expect_err("zero invocation limit must be measured");
    assert_eq!(error.kind(), InterpretErrorKind::PatternInvocationLimit);
}

#[test]
fn pattern_space_requires_a_selected_pattern_before_painting() {
    let error =
        interpret(b"/Pattern cs 0 0 1 1 re f").expect_err("painting without a pattern must fail");
    assert_eq!(error.kind(), InterpretErrorKind::PatternColorMissing);
}

#[test]
fn a_shading_pattern_is_selected_as_a_colour_with_its_own_matrix() {
    let source = shading_pattern_fixture(b"/Matrix [2 0 0 2 3 4]", false);
    let graph = interpret_fixture(&source).expect("page with a shading pattern");
    let outer = path(&graph.atoms[0]);
    let Color::ShadingPattern(pattern) = &outer.state.fill_color.value else {
        panic!("selected fill is not a shading pattern")
    };
    assert_eq!(pattern.reference, Some(pdf_syntax::Reference::new(5, 0)));
    assert!((pattern.matrix.value.a - 2.0).abs() < f64::EPSILON);
    assert!((pattern.matrix.value.e - 3.0).abs() < f64::EPSILON);
    assert!((pattern.matrix.value.f - 4.0).abs() < f64::EPSILON);
    assert_eq!(pattern.base, crate::Matrix::IDENTITY);
    let crate::ShadingGeometry::Axial(coords) = &pattern.shading.geometry else {
        panic!("axial shading")
    };
    crate::test_fixtures::assert_floats(&coords.value, &[0.0, 0.0, 10.0, 0.0]);
    assert_eq!(pattern.shading.extend.value, [true, true]);
    assert_eq!(
        pattern.shading.function.evaluate(&[0.5]),
        Some(vec![0.5]),
        "the shading's own function, not a copy of it"
    );
}

#[test]
fn a_shading_pattern_without_a_matrix_uses_the_identity() {
    let source = shading_pattern_fixture(b"", false);
    let graph = interpret_fixture(&source).expect("shading pattern with no matrix");
    let Color::ShadingPattern(pattern) = &path(&graph.atoms[0]).state.fill_color.value else {
        panic!("shading pattern")
    };
    assert_eq!(pattern.matrix.value, crate::Matrix::IDENTITY);
    assert!(pattern.matrix.provenance.is_empty());
}

#[test]
fn a_shading_pattern_inside_a_form_measures_against_the_form_space() {
    let source = shading_pattern_fixture(b"/Matrix [1 0 0 1 0 0]", true);
    let graph = interpret_fixture(&source).expect("shading pattern inside a form");
    let Color::ShadingPattern(pattern) = &path(&graph.atoms[0]).state.fill_color.value else {
        panic!("shading pattern")
    };
    assert_eq!(
        pattern.base,
        crate::Matrix {
            a: 2.0,
            b: 0.0,
            c: 0.0,
            d: 2.0,
            e: 5.0,
            f: 7.0
        }
    );
}

#[test]
fn a_pattern_of_an_unknown_type_is_refused() {
    let source = shading_pattern_fixture(b"", false);
    let text = String::from_utf8(source.as_bytes().to_vec()).expect("fixture is ASCII");
    let broken = text.replace("/PatternType 2", "/PatternType 7");
    let broken = pdf_bytes::ByteStore::new(
        pdf_bytes::SourceId::new(68),
        std::sync::Arc::<[u8]>::from(broken.into_bytes()),
    );
    let error = interpret_fixture(&broken).expect_err("an unknown pattern type");
    assert_eq!(
        error.kind(),
        InterpretErrorKind::PatternResource(pdf_content::PageContentErrorKind::PatternWrongType)
    );
}

fn uncoloured_graph(space: &[u8], select: &[u8]) -> crate::PaintGraph {
    let source = uncoloured_pattern_paint_fixture(2, space, select);
    interpret_fixture(&source).expect("page with an uncoloured tiling pattern")
}

fn cell_colour(graph: &crate::PaintGraph) -> Color {
    let Color::TilingPattern(pattern) = &path(&graph.atoms[0]).state.fill_color.value else {
        panic!("selected fill is not a tiling pattern")
    };
    assert_eq!(pattern.paint_type.value, 2);
    assert_eq!(pattern.graph.atoms.len(), 1);
    path(&pattern.graph.atoms[0]).state.fill_color.value.clone()
}

#[test]
fn an_uncoloured_pattern_is_painted_in_the_colour_that_chose_it() {
    assert_eq!(
        cell_colour(&uncoloured_graph(b"[/Pattern /DeviceRGB]", b"1 0 0")),
        Color::DeviceRgb(1.0, 0.0, 0.0)
    );
    assert_eq!(
        cell_colour(&uncoloured_graph(b"[/Pattern /DeviceGray]", b"0.5")),
        Color::DeviceGray(0.5)
    );
    assert_eq!(
        cell_colour(&uncoloured_graph(b"[/Pattern /DeviceCMYK]", b"0 1 1 0")),
        Color::DeviceCmyk(0.0, 1.0, 1.0, 0.0)
    );
}

#[test]
fn a_colour_operator_inside_an_uncoloured_cell_has_no_effect() {
    assert_eq!(
        cell_colour(&uncoloured_graph(b"[/Pattern /DeviceRGB]", b"1 0 0")),
        Color::DeviceRgb(1.0, 0.0, 0.0)
    );
}

#[test]
fn a_coloured_pattern_reached_through_a_base_space_keeps_its_own_colour() {
    let source = uncoloured_pattern_paint_fixture(1, b"[/Pattern /DeviceRGB]", b"1 0 0");
    let graph = interpret_fixture(&source).expect("page with a coloured tiling pattern");
    let Color::TilingPattern(pattern) = &path(&graph.atoms[0]).state.fill_color.value else {
        panic!("selected fill is not a tiling pattern")
    };
    assert_eq!(
        path(&pattern.graph.atoms[0]).state.fill_color.value,
        Color::DeviceGray(0.25)
    );
}

#[test]
fn an_scn_with_the_wrong_operand_count_is_repaired_and_reported() {
    for (select, actual) in [(&b"1"[..], 1usize), (&b"1 0 0 0"[..], 4)] {
        let graph = uncoloured_graph(b"[/Pattern /DeviceRGB]", select);
        assert_eq!(cell_colour(&graph), Color::DeviceRgb(1.0, 0.0, 0.0));
        assert!(
            graph.repairs.iter().any(|repair| matches!(
                repair.kind,
                crate::RepairKind::UncolouredPatternOperandCount { expected: 3, actual: reported }
                    if reported == actual
            )),
            "the wrong count is said out loud: {:?}",
            graph.repairs
        );
    }
}

#[test]
fn a_pattern_space_is_refused_as_its_own_base() {
    let source = uncoloured_pattern_paint_fixture(2, b"[/Pattern /Pattern]", b"");
    let error = interpret_fixture(&source).expect_err("a pattern base is not a colour");
    assert_eq!(error.kind(), InterpretErrorKind::UnsupportedColorSpace);
}
