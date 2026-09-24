use crate::color::ColorSpace;
use crate::error::InterpretErrorKind;
use crate::function::Function;
use crate::graph::PaintAtomKind;
use crate::interpreter::{PaintStream, interpret_stream_sequence_with_resources};
use crate::shading::ShadingGeometry;
use crate::state::PaintLimits;
use crate::test_fixtures::{
    assert_floats, interpret_color_page, shading_fixture, shading_function_stream_fixture,
};
use pdf_content::{
    ContentLimits, PageContentLimits, load_page_program_strict, parse_operations_strict,
};

#[test]
fn axial_stitching_shading_retains_geometry_functions_and_provenance() {
    let source = shading_fixture(b"<< /Type /Shading /ShadingType 2 /ColorSpace /ICC /Coords [0 0 100 0] /Domain [0 1] /Extend [true false] /Background [0 0 0] /BBox [0 0 100 20] /AntiAlias true /Function << /FunctionType 3 /Domain [0 1] /Functions [<< /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 1 0] /N 1 >> << /FunctionType 2 /Domain [0 1] /C0 [0 1 0] /C1 [0 0 1] /N 2 >>] /Bounds [.4] /Encode [0 1 0 1] >> >>");
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("page with axial shading");
    assert_eq!(page.resources.shadings().len(), 1);
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("shading operation");
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
    .expect("axial stitching shading");
    let PaintAtomKind::Shading(shading) = &graph.atoms[0].kind else {
        panic!("sh operator did not emit a shading atom")
    };
    assert_eq!(shading.reference, Some(pdf_syntax::Reference::new(5, 0)));
    let ColorSpace::IccBased(definition) = &shading.color_space.value else {
        panic!("expected ICCBased RGB shading")
    };
    assert_eq!(definition.reference, pdf_syntax::Reference::new(6, 0));
    assert_eq!(definition.components.value, 3);
    assert!(matches!(shading.geometry, ShadingGeometry::Axial(_)));
    assert_eq!(shading.extend.value, [true, false]);
    assert!(shading.anti_alias.value);
    assert_eq!(shading.background.as_ref().unwrap().value.len(), 3);
    let Function::Stitching(function) = &shading.function else {
        panic!("expected a stitching function")
    };
    assert_eq!(function.functions.len(), 2);
    assert_eq!(function.bounds.value.len(), 1);
    assert!((function.bounds.value[0] - 0.4).abs() < f64::EPSILON);
    assert_eq!(function.encode.value.len(), 4);
    let Function::Exponential(first) = &function.functions[0] else {
        panic!("expected an exponential subfunction")
    };
    assert_eq!(first.c0.value.len(), 3);
    assert_eq!(first.c1.value.len(), 3);
    assert_eq!(first.exponent.provenance.len(), 1);
    assert_eq!(graph.atoms[0].id.operator_span.start(), 4);
}

#[test]
fn indexed_shading_uses_one_palette_index_component() {
    let source = shading_fixture(b"<< /ShadingType 2 /ColorSpace /IDX /Coords [0 0 10 0] /Background [1] /Function << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >> >>");
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("page with Indexed shading");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("shading operation");
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
    .expect("Indexed shading");
    let PaintAtomKind::Shading(shading) = &graph.atoms[0].kind else {
        panic!("expected shading atom")
    };
    let ColorSpace::Indexed(definition) = &shading.color_space.value else {
        panic!("expected Indexed shading colour space")
    };
    assert_eq!(definition.hival.value, 1);
    assert_eq!(shading.background.as_ref().unwrap().value, vec![1.0]);
    let Function::Exponential(function) = &shading.function else {
        panic!("expected exponential function")
    };
    assert_eq!(function.c0.value, vec![0.0]);
    assert_eq!(function.c1.value, vec![1.0]);
}

#[test]
fn repeated_stitching_bounds_describe_an_unreachable_subfunction() {
    let source = shading_fixture(b"<< /ShadingType 2 /ColorSpace /DeviceGray /Coords [0 0 100 0] /Function << /FunctionType 3 /Domain [0 1] /Bounds [.5 .5] /Encode [0 1 0 1 0 1] /Functions [<< /FunctionType 2 /Domain [0 1] /C0 [.2] /C1 [.2] /N 1 >> << /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [1] /N 1 >> << /FunctionType 2 /Domain [0 1] /C0 [.2] /C1 [0] /N 1 >>] >> >>");
    let graph = interpret_color_page(&source).expect("repeated bounds are unambiguous");
    let PaintAtomKind::Shading(shading) = &graph.atoms[0].kind else {
        panic!("expected a shading atom")
    };
    let Function::Stitching(function) = &shading.function else {
        panic!("expected a stitching function")
    };
    assert_floats(&function.bounds.value, &[0.5, 0.5]);
    for (input, expected) in [(0.0, 0.2), (0.25, 0.2), (0.5, 0.2), (0.75, 0.1), (1.0, 0.0)] {
        assert_floats(
            &shading.function.evaluate(&[input]).expect("stitching"),
            &[expected],
        );
    }

    let source = shading_fixture(b"<< /ShadingType 2 /ColorSpace /DeviceGray /Coords [0 0 100 0] /Function << /FunctionType 3 /Domain [0 1] /Bounds [.6 .5] /Encode [0 1 0 1 0 1] /Functions [<< /FunctionType 2 /Domain [0 1] /C0 [.2] /C1 [.2] /N 1 >> << /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [1] /N 1 >> << /FunctionType 2 /Domain [0 1] /C0 [.2] /C1 [0] /N 1 >>] >> >>");
    let error = interpret_color_page(&source).expect_err("decreasing bounds must fail");
    assert_eq!(error.kind(), InterpretErrorKind::InvalidFunction);
}

#[test]
fn radial_shading_and_function_limits_are_measured() {
    let source = shading_fixture(b"<< /ShadingType 3 /ColorSpace /DeviceGray /Coords [0 0 0 10 10 20] /Function << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >> >>");
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("page with radial shading");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("shading operation");
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
    .expect("radial shading");
    let PaintAtomKind::Shading(shading) = &graph.atoms[0].kind else {
        panic!("expected shading atom")
    };
    assert!(matches!(shading.geometry, ShadingGeometry::Radial(_)));
    assert!(
        shading
            .domain
            .value
            .iter()
            .zip([0.0, 1.0])
            .all(|(actual, expected)| (actual - expected).abs() < f64::EPSILON)
    );
    assert_eq!(shading.extend.value, [false, false]);

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
            max_functions: 0,
            ..PaintLimits::default()
        },
    )
    .expect_err("zero function limit must be enforced");
    assert_eq!(error.kind(), InterpretErrorKind::FunctionCountLimit);
}

#[test]
fn unsupported_and_malformed_shadings_fail_closed() {
    for (dictionary, expected) in [
        (
            &b"<< /ShadingType 4 /ColorSpace /DeviceRGB >>"[..],
            InterpretErrorKind::UnsupportedShadingType,
        ),
        (
            &b"<< /ShadingType 3 /ColorSpace /DeviceGray /Coords [0 0 -1 1 1 2] /Function << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >> >>"[..],
            InterpretErrorKind::InvalidShadingEntry,
        ),
        (
            &b"<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 1 0] /Function << /FunctionType 3 /Domain [0 1] /Functions [<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0] /C1 [1 1 1] /N 1 >>] /Bounds [.5] /Encode [0 1] >> >>"[..],
            InterpretErrorKind::InvalidFunction,
        ),
    ] {
        let source = shading_fixture(dictionary);
        let page = load_page_program_strict(&source, 0, PageContentLimits::default())
            .expect("page with shading boundary");
        let operations =
            parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
                .expect("shading operation");
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
        .expect_err("unsupported shading semantics must be refused");
        assert_eq!(error.kind(), expected);
    }
}

#[test]
fn shading_resolves_an_indirect_function_and_refuses_a_multi_input_one() {
    let source = shading_function_stream_fixture(
        b"<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 100 0] /Function 5 0 R >>",
        b"/FunctionType 0 /Domain [0 1] /Range [0 1 0 1 0 1] /Size [2] /BitsPerSample 8",
        &[0, 0, 0, 255, 128, 0],
    );
    let graph = interpret_color_page(&source).expect("indirect shading function");
    let PaintAtomKind::Shading(shading) = &graph.atoms[0].kind else {
        panic!("expected a shading atom")
    };
    let Function::Sampled(function) = &shading.function else {
        panic!("expected a sampled shading function")
    };
    assert_eq!(function.reference, Some(pdf_syntax::Reference::new(5, 0)));
    assert_floats(
        &shading.function.evaluate(&[1.0]).expect("shading function"),
        &[1.0, 128.0 / 255.0, 0.0],
    );

    let source = shading_function_stream_fixture(
        b"<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 100 0] /Function 5 0 R >>",
        b"/FunctionType 0 /Domain [0 1 0 1] /Range [0 1 0 1 0 1] /Size [2 1] /BitsPerSample 8",
        &[0, 0, 0, 255, 128, 0],
    );
    let error = interpret_color_page(&source).expect_err("multi-input shading function");
    assert_eq!(error.kind(), InterpretErrorKind::InvalidFunction);
}

#[test]
fn a_function_shading_keeps_its_rectangle_matrix_and_two_input_function() {
    let source = shading_function_stream_fixture(
        b"<< /ShadingType 1 /ColorSpace /DeviceRGB /Domain [-1 1 -1 1] /Matrix [10 0 0 10 50 50] /Function 5 0 R >>",
        b"/FunctionType 4 /Domain [-1 1 -1 1] /Range [0 1 0 1 0 1]",
        b"{ 0 exch }",
    );
    let graph = interpret_color_page(&source).expect("function shading");
    let PaintAtomKind::Shading(shading) = &graph.atoms[0].kind else {
        panic!("expected a shading atom")
    };
    let ShadingGeometry::Function { domain, matrix } = &shading.geometry else {
        panic!("expected a function shading")
    };
    assert_floats(&domain.value, &[-1.0, 1.0, -1.0, 1.0]);
    assert_eq!(domain.provenance.len(), 1);
    assert_floats(
        &[
            matrix.value.a,
            matrix.value.d,
            matrix.value.e,
            matrix.value.f,
        ],
        &[10.0, 10.0, 50.0, 50.0],
    );
    assert_floats(
        &shading
            .function
            .evaluate(&[0.25, 0.75])
            .expect("two inputs"),
        &[0.25, 0.0, 0.75],
    );

    let source = shading_function_stream_fixture(
        b"<< /ShadingType 1 /ColorSpace /DeviceRGB /Function 5 0 R >>",
        b"/FunctionType 4 /Domain [0 1 0 1] /Range [0 1 0 1 0 1]",
        b"{ 0 exch }",
    );
    let graph = interpret_color_page(&source).expect("function shading with defaults");
    let PaintAtomKind::Shading(shading) = &graph.atoms[0].kind else {
        panic!("expected a shading atom")
    };
    let ShadingGeometry::Function { domain, matrix } = &shading.geometry else {
        panic!("expected a function shading")
    };
    assert_floats(&domain.value, &[0.0, 1.0, 0.0, 1.0]);
    assert!(domain.provenance.is_empty());
    assert_eq!(matrix.value, crate::geometry::Matrix::IDENTITY);
}

#[test]
fn a_function_shading_refuses_a_one_input_function_and_an_empty_rectangle() {
    for (shading, entries, expected) in [
        (
            &b"<< /ShadingType 1 /ColorSpace /DeviceRGB /Function 5 0 R >>"[..],
            &b"/FunctionType 0 /Domain [0 1] /Range [0 1 0 1 0 1] /Size [2] /BitsPerSample 8"[..],
            InterpretErrorKind::InvalidFunction,
        ),
        (
            &b"<< /ShadingType 1 /ColorSpace /DeviceRGB /Domain [1 1 0 1] /Function 5 0 R >>"[..],
            &b"/FunctionType 0 /Domain [0 1 0 1] /Range [0 1 0 1 0 1] /Size [2 1] /BitsPerSample 8"
                [..],
            InterpretErrorKind::InvalidShadingEntry,
        ),
    ] {
        let source = shading_function_stream_fixture(shading, entries, &[0, 0, 0, 255, 128, 0]);
        let error = interpret_color_page(&source).expect_err("refused function shading");
        assert_eq!(error.kind(), expected);
    }
}
