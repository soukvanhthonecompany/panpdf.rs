use crate::color::{
    Color, ColorSpace, Colorant, IccAlternate, IndexedBase, IndexedLookupSource, palette_index,
};
use crate::error::InterpretErrorKind;
use crate::function::Function;
use crate::graph::PaintAtomKind;
use crate::interpreter::{PaintStream, interpret_stream_sequence_with_resources};
use crate::state::PaintLimits;
use crate::test_fixtures::{
    assert_floats, calibrated_color_fixture, color_space_fixture, icc_color_fixture,
    icc_group_fixture, icc_profile_bytes, indexed_stream_fixture, interpret_color_page, path,
    tint_stream_fixture,
};
use pdf_content::{
    ContentLimits, PageContentErrorKind, PageContentLimits, load_page_program_strict,
    parse_operations_strict,
};

#[test]
fn named_and_direct_device_color_spaces_drive_component_operators() {
    let source = color_space_fixture();
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("page with colour-space resource");
    assert!(page.resources.color_space(b"/Cs1").is_some());
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("colour operations");
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
    .expect("device colour spaces");
    let paint = path(&graph.atoms[0]);
    assert_eq!(paint.state.fill_color_space.value, ColorSpace::DeviceRgb);
    assert_eq!(paint.state.fill_color_space.provenance.len(), 2);
    assert_eq!(
        paint.state.fill_color.value,
        Color::DeviceRgb(0.2, 0.4, 0.6)
    );
    assert_eq!(paint.state.stroke_color_space.value, ColorSpace::DeviceCmyk);
    assert_eq!(paint.state.stroke_color_space.provenance.len(), 1);
    assert_eq!(
        paint.state.stroke_color.value,
        Color::DeviceCmyk(0.1, 0.2, 0.3, 0.4)
    );
}

#[test]
fn calibrated_color_spaces_retain_parameters_and_clamp_components() {
    let source = calibrated_color_fixture(
        b"/CG [/CalGray << /WhitePoint [.9505 1 1.089] /Gamma 2.2 >>] /CR [/CalRGB << /WhitePoint [.9505 1 1.089] /BlackPoint [.01 .02 .03] /Gamma [2.2 2.3 2.4] /Matrix [1 .1 .2 .3 1 .4 .5 .6 1] >>] /LAB [/Lab << /WhitePoint [.9505 1 1.089] /Range [-80 90 -70 75] >>] /LABD [/Lab << /WhitePoint [.9505 1 1.089] >>]",
        b"/CG cs 1.4 sc 0 0 1 1 re f /CR CS -.1 .2 1.3 SC 2 0 1 1 re S /LAB cs 50 -100 100 sc 4 0 1 1 re f /LABD cs 50 0 0 sc 6 0 1 1 re f",
    );
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("page with calibrated colour spaces");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("calibrated colour operations");
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
    .expect("supported calibrated colour spaces");
    assert_eq!(graph.atoms.len(), 4);

    let gray = path(&graph.atoms[0]);
    let ColorSpace::CalGray(definition) = &gray.state.fill_color_space.value else {
        panic!("expected CalGray")
    };
    assert_floats(&definition.white_point.value, &[0.9505, 1.0, 1.089]);
    assert_floats(&definition.black_point.value, &[0.0; 3]);
    assert_eq!(definition.black_point.provenance.len(), 0);
    assert!((definition.gamma.value - 2.2).abs() < f64::EPSILON);
    assert_eq!(definition.gamma.provenance.len(), 1);
    assert_eq!(gray.state.fill_color.value, Color::CalGray(1.0));
    assert_eq!(gray.state.fill_color_space.provenance.len(), 2);

    let rgb = path(&graph.atoms[1]);
    let ColorSpace::CalRgb(definition) = &rgb.state.stroke_color_space.value else {
        panic!("expected CalRGB")
    };
    assert_floats(&definition.black_point.value, &[0.01, 0.02, 0.03]);
    assert_floats(&definition.gamma.value, &[2.2, 2.3, 2.4]);
    assert!((definition.matrix.value[1] - 0.1).abs() < f64::EPSILON);
    assert_eq!(rgb.state.stroke_color.value, Color::CalRgb(0.0, 0.2, 1.0));

    let lab = path(&graph.atoms[2]);
    let ColorSpace::Lab(definition) = &lab.state.fill_color_space.value else {
        panic!("expected Lab")
    };
    assert_floats(&definition.range.value, &[-80.0, 90.0, -70.0, 75.0]);
    assert_eq!(lab.state.fill_color.value, Color::Lab(50.0, -80.0, 75.0));

    let default_lab = path(&graph.atoms[3]);
    let ColorSpace::Lab(definition) = &default_lab.state.fill_color_space.value else {
        panic!("expected default Lab")
    };
    assert_floats(&definition.range.value, &[-100.0, 100.0, -100.0, 100.0]);
    assert!(definition.range.provenance.is_empty());
}

#[test]
fn malformed_and_unsupported_calibrated_color_spaces_fail_closed() {
    for (definition, expected) in [
        (
            &b"/Bad [/CalGray << /Gamma 2 >>]"[..],
            InterpretErrorKind::ColorSpaceMissingEntry,
        ),
        (
            &b"/Bad [/CalRGB << /WhitePoint [1 1 1] /Gamma [1 0 1] >>]"[..],
            InterpretErrorKind::InvalidColorSpaceEntry,
        ),
        (
            &b"/Bad [/Lab << /WhitePoint [1 1 1] /Range [-129 1 -1 1] >>]"[..],
            InterpretErrorKind::InvalidColorSpaceEntry,
        ),
        (
            &b"/Bad [/CalGray << /WhitePoint [1 1 1] /Gamma 1 /Gamma 2 >>]"[..],
            InterpretErrorKind::InvalidColorSpaceEntry,
        ),
        (
            &b"/Bad [/ICCBased << >>]"[..],
            InterpretErrorKind::InvalidIccProfileEntry,
        ),
    ] {
        let source = calibrated_color_fixture(definition, b"/Bad cs 0 sc 0 0 1 1 re f");
        let page = load_page_program_strict(&source, 0, PageContentLimits::default())
            .expect("page with colour-space boundary");
        let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
            .expect("colour-space operation");
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
        .expect_err("unsafe colour-space semantics must be refused");
        assert_eq!(error.kind(), expected);
    }
}

#[test]
fn indexed_color_rounds_clamps_and_expands_a_calibrated_palette() {
    let source = calibrated_color_fixture(
        b"/BASE [/Lab << /WhitePoint [.9505 1 1.089] /Range [-80 90 -70 75] >>] /IDX [/Indexed /BASE 2 <000000 804080 FFFFFF>]",
        b"/IDX cs -1 sc 0 0 1 1 re f .49 sc 0 0 1 1 re f .5 sc 0 0 1 1 re f 1.5 sc 0 0 1 1 re f 99 sc 0 0 1 1 re f",
    );
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("page with Indexed colour space");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("Indexed colour operations");
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
    .expect("valid Indexed colour space");
    let indices: Vec<_> = graph
        .atoms
        .iter()
        .map(|atom| path(atom).state.fill_color.value.clone())
        .collect();
    assert_eq!(
        indices,
        vec![
            Color::Indexed(0),
            Color::Indexed(0),
            Color::Indexed(1),
            Color::Indexed(2),
            Color::Indexed(2),
        ]
    );
    let first = path(&graph.atoms[0]);
    let ColorSpace::Indexed(definition) = &first.state.fill_color_space.value else {
        panic!("expected Indexed")
    };
    assert_eq!(definition.hival.value, 2);
    assert_eq!(
        definition.lookup.value.as_ref(),
        &[0, 0, 0, 128, 64, 128, 255, 255, 255]
    );
    assert!(matches!(
        definition.lookup_source,
        IndexedLookupSource::String(_)
    ));
    assert!(matches!(definition.base.value, IndexedBase::Lab(_)));
    assert_eq!(definition.base.provenance.len(), 2);
    assert_eq!(definition.lookup.provenance.len(), 1);
    assert_eq!(first.state.fill_color_space.provenance.len(), 2);
    assert_floats(
        &definition.base_components(1),
        &[
            (128.0_f64 / 255.0).mul_add(100.0, 0.0),
            (64.0_f64 / 255.0).mul_add(170.0, -80.0),
            (128.0_f64 / 255.0).mul_add(145.0, -70.0),
        ],
    );
    assert_floats(&definition.base_components(255), &[100.0, 90.0, 75.0]);
}

#[test]
fn palette_index_rounds_half_up_and_clips_at_hival() {
    assert_eq!(palette_index(-1.0, 3), 0);
    assert_eq!(palette_index(0.49, 3), 0);
    assert_eq!(palette_index(0.5, 3), 1);
    assert_eq!(palette_index(2.49, 3), 2);
    assert_eq!(palette_index(2.5, 3), 3);
    assert_eq!(palette_index(3.5, 3), 3);
    assert_eq!(palette_index(0.0, 0), 0);
    assert_eq!(palette_index(7.0, 0), 0);
    assert_eq!(palette_index(254.49, 255), 254);
    assert_eq!(palette_index(254.5, 255), 255);
    assert_eq!(palette_index(f64::MAX, 255), 255);
}

#[test]
fn indexed_lookup_stream_retains_resource_provenance() {
    let source = indexed_stream_fixture(
        b"",
        &[0, 64, 128, 255, 128, 0],
        b"/IDX cs .5 sc 0 0 1 1 re f",
    );
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("page with Indexed lookup stream");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("Indexed operations");
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
    .expect("valid Indexed lookup stream");
    let paint = path(&graph.atoms[0]);
    assert_eq!(paint.state.fill_color.value, Color::Indexed(1));
    let ColorSpace::Indexed(definition) = &paint.state.fill_color_space.value else {
        panic!("expected Indexed")
    };
    assert!(matches!(
        definition.lookup_source,
        IndexedLookupSource::Stream { reference, .. }
            if reference == pdf_syntax::Reference::new(5, 0)
    ));
    assert_eq!(definition.lookup.provenance.len(), 3);
    assert_floats(&definition.base_components(1), &[1.0, 128.0 / 255.0, 0.0]);
}

#[test]
fn indexed_lookup_forgives_a_trailing_end_of_line_and_nothing_else() {
    for excess in [&b"\n"[..], &b" \r\n"[..]] {
        let mut lookup = vec![0, 64, 128, 255, 128, 0];
        lookup.extend_from_slice(excess);
        let source = indexed_stream_fixture(b"", &lookup, b"/IDX cs .5 sc 0 0 1 1 re f");
        let page = load_page_program_strict(&source, 0, PageContentLimits::default())
            .expect("page with an over-long Indexed lookup stream");
        let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
            .expect("Indexed operations");
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
        .expect("a palette padded with whitespace still resolves");
        let ColorSpace::Indexed(definition) = &path(&graph.atoms[0]).state.fill_color_space.value
        else {
            panic!("expected Indexed")
        };
        assert_eq!(definition.lookup.value.len(), 6);
        assert_floats(&definition.base_components(1), &[1.0, 128.0 / 255.0, 0.0]);
    }

    let source = indexed_stream_fixture(b"", &[0, 64, 128, 255, 128, 0, 7], b"/IDX cs");
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("page with a disagreeing Indexed lookup stream");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("Indexed operations");
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
    .expect_err("a palette carrying a real extra byte must fail closed");
    assert_eq!(
        error.kind(),
        InterpretErrorKind::IndexedLookupLength {
            expected: 6,
            actual: 7,
        }
    );
}

#[test]
fn malformed_indexed_spaces_and_lookup_limits_fail_closed() {
    for (definition, expected) in [
        (
            &b"/Bad [/Indexed /DeviceRGB 1]"[..],
            InterpretErrorKind::InvalidIndexedColorSpace,
        ),
        (
            &b"/Bad [/Indexed /DeviceRGB 1.5 <000000 FFFFFF>]"[..],
            InterpretErrorKind::InvalidIndexedColorSpace,
        ),
        (
            &b"/Bad [/Indexed /Pattern 0 <000000>]"[..],
            InterpretErrorKind::UnsupportedIndexedBase,
        ),
        (
            &b"/Bad [/Indexed [/Indexed /DeviceGray 0 <00>] 0 <00>]"[..],
            InterpretErrorKind::UnsupportedIndexedBase,
        ),
        (
            &b"/Bad [/Indexed /DeviceRGB 1 <000000>]"[..],
            InterpretErrorKind::IndexedLookupLength {
                expected: 6,
                actual: 3,
            },
        ),
        (
            &b"/Bad [/Indexed /DeviceGray 1 <0080FF>]"[..],
            InterpretErrorKind::IndexedLookupLength {
                expected: 2,
                actual: 3,
            },
        ),
    ] {
        let source = calibrated_color_fixture(definition, b"/Bad cs 0 sc");
        let page = load_page_program_strict(&source, 0, PageContentLimits::default())
            .expect("page with Indexed boundary");
        let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
            .expect("Indexed operation");
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
        .expect_err("unsafe Indexed semantics must be refused");
        assert_eq!(error.kind(), expected);
    }

    let source = indexed_stream_fixture(
        b"/Intent /Perceptual",
        &[0, 0, 0, 255, 255, 255],
        b"/IDX cs",
    );
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("page with unsupported lookup entry");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("Indexed operation");
    let paint_stream = PaintStream {
        source: &page.streams[0].bytes,
        reference: page.streams[0].reference,
        operations: &operations,
    };
    let error = interpret_stream_sequence_with_resources(
        &[paint_stream],
        page.page,
        &[],
        &page.resources,
        PaintLimits::default(),
    )
    .expect_err("unsupported lookup dictionary entry must fail closed");
    assert_eq!(
        error.kind(),
        InterpretErrorKind::UnsupportedIndexedLookupEntry
    );

    let error = interpret_stream_sequence_with_resources(
        &[paint_stream],
        page.page,
        &[],
        &page.resources,
        PaintLimits {
            max_indexed_lookup_bytes: 5,
            ..PaintLimits::default()
        },
    )
    .expect_err("lookup byte limit must be enforced");
    assert_eq!(error.kind(), InterpretErrorKind::IndexedLookupLimit);
}

#[test]
fn separation_retains_colorant_alternate_and_tint_transform() {
    let source = calibrated_color_fixture(
        b"/SEP [/Separation /Spot /DeviceCMYK << /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 .2 .8 .1] /N 1 >>]",
        b"/SEP cs 0 0 1 1 re f .5 scn 0 0 1 1 re f -1 scn 0 0 1 1 re f 2 scn 0 0 1 1 re f",
    );
    let graph = interpret_color_page(&source).expect("valid Separation colour space");
    let tints: Vec<_> = graph
        .atoms
        .iter()
        .map(|atom| path(atom).state.fill_color.value.clone())
        .collect();
    assert_eq!(
        tints,
        vec![
            Color::Separation(1.0),
            Color::Separation(0.5),
            Color::Separation(0.0),
            Color::Separation(1.0),
        ]
    );
    let ColorSpace::Separation(definition) = &path(&graph.atoms[0]).state.fill_color_space.value
    else {
        panic!("expected Separation")
    };
    assert_eq!(
        definition.colorant.value,
        Colorant::Named(b"/Spot".to_vec())
    );
    assert_eq!(definition.colorant.provenance.len(), 1);
    assert_eq!(*definition.alternate.value, ColorSpace::DeviceCmyk);
    assert_eq!(definition.alternate.provenance.len(), 1);
    assert_eq!(definition.tint_transform.value.inputs(), 1);
    assert_eq!(definition.tint_transform.value.outputs(), 4);
    assert_eq!(definition.tint_transform.provenance.len(), 1);
    assert_floats(
        &definition
            .alternate_components(0.5)
            .expect("tint transform"),
        &[0.0, 0.1, 0.4, 0.05],
    );
    assert_floats(
        &definition
            .alternate_components(1.0)
            .expect("tint transform"),
        &[0.0, 0.2, 0.8, 0.1],
    );
}

#[test]
fn device_n_resolves_an_indirect_sampled_tint_transform() {
    let samples = [0, 0, 0, 255, 0, 0, 0, 255, 0, 255, 255, 255];
    let source = tint_stream_fixture(
        b"/DN [/DeviceN [/Cyan /Magenta] /DeviceRGB 5 0 R]",
        b"/FunctionType 0 /Domain [0 1 0 1] /Range [0 1 0 1 0 1] /Size [2 2] /BitsPerSample 8",
        &samples,
        b"/DN cs 1 0 scn 0 0 1 1 re f",
    );
    let graph = interpret_color_page(&source).expect("valid DeviceN colour space");
    let paint = path(&graph.atoms[0]);
    assert_eq!(paint.state.fill_color.value, Color::DeviceN(vec![1.0, 0.0]));
    let ColorSpace::DeviceN(definition) = &paint.state.fill_color_space.value else {
        panic!("expected DeviceN")
    };
    assert_eq!(
        definition.colorants.value,
        vec![
            Colorant::Named(b"/Cyan".to_vec()),
            Colorant::Named(b"/Magenta".to_vec())
        ]
    );
    assert_eq!(*definition.alternate.value, ColorSpace::DeviceRgb);
    assert!(definition.attributes.is_none());
    let Function::Sampled(function) = &definition.tint_transform.value else {
        panic!("expected a sampled tint transform")
    };
    assert_eq!(function.reference, Some(pdf_syntax::Reference::new(5, 0)));
    assert_eq!(function.size.value, vec![2, 2]);
    assert_eq!(function.bits_per_sample.value, 8);
    assert_eq!(function.samples.as_ref(), &samples);
    assert!(function.encode.provenance.is_empty());
    assert_floats(&function.encode.value, &[0.0, 1.0, 0.0, 1.0]);
    assert_eq!(function.decode.value, function.range.value);
    assert_floats(
        &definition.alternate_components(&[1.0, 0.0]).expect("tint"),
        &[1.0, 0.0, 0.0],
    );
    assert_floats(
        &definition.alternate_components(&[0.5, 0.0]).expect("tint"),
        &[0.5, 0.0, 0.0],
    );
    assert_floats(
        &definition.alternate_components(&[0.5, 0.5]).expect("tint"),
        &[0.5, 0.5, 0.25],
    );
}

#[test]
fn device_n_retains_attributes_without_interpreting_them() {
    let source = calibrated_color_fixture(
        b"/DN [/DeviceN [/Spot /None] /DeviceGray << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >> << /Subtype /DeviceN >>]",
        b"/DN cs 1 1 scn 0 0 1 1 re f",
    );
    let error = interpret_color_page(&source).expect_err("tint arity must be checked");
    assert_eq!(error.kind(), InterpretErrorKind::InvalidTintTransform);

    let samples = [0_u8, 255];
    let source = tint_stream_fixture(
        b"/DN [/DeviceN [/Spot /None] /DeviceGray 5 0 R << /Subtype /DeviceN >>]",
        b"/FunctionType 0 /Domain [0 1 0 1] /Range [0 1] /Size [2 1] /BitsPerSample 8",
        &samples,
        b"/DN cs 1 1 scn 0 0 1 1 re f",
    );
    let graph = interpret_color_page(&source).expect("valid DeviceN with attributes");
    let ColorSpace::DeviceN(definition) = &path(&graph.atoms[0]).state.fill_color_space.value
    else {
        panic!("expected DeviceN")
    };
    assert_eq!(definition.colorants.value[1], Colorant::None);
    assert!(definition.attributes.is_some());
    assert_floats(
        &definition.alternate_components(&[1.0, 1.0]).expect("tint"),
        &[1.0],
    );
}

#[test]
fn malformed_separation_and_device_n_spaces_fail_closed() {
    for (definition, expected) in [
        (
            &b"/Bad [/Separation /Spot /DeviceGray]"[..],
            InterpretErrorKind::InvalidSeparationColorSpace,
        ),
        (
            &b"/Bad [/Separation 1 /DeviceGray << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>]"[..],
            InterpretErrorKind::InvalidSeparationColorSpace,
        ),
        (
            &b"/Bad [/Separation /Spot /Pattern << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>]"[..],
            InterpretErrorKind::UnsupportedTintAlternate,
        ),
        (
            &b"/Bad [/Separation /Spot [/Indexed /DeviceGray 0 <00>] << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>]"[..],
            InterpretErrorKind::UnsupportedTintAlternate,
        ),
        (
            &b"/Bad [/Separation /Spot /DeviceRGB << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>]"[..],
            InterpretErrorKind::InvalidFunction,
        ),
        (
            &b"/Bad [/Separation /Spot /DeviceGray << /FunctionType 4 /Domain [0 1] /Range [0 1] >>]"[..],
            InterpretErrorKind::FunctionMissingData,
        ),
        (
            &b"/Bad [/Separation /Spot /DeviceGray << /FunctionType 7 /Domain [0 1] /Range [0 1] >>]"[..],
            InterpretErrorKind::UnsupportedFunction,
        ),
        (
            &b"/Bad [/Separation /Spot /DeviceGray << /FunctionType 0 /Domain [0 1] /Range [0 1] /Size [2] /BitsPerSample 8 >>]"[..],
            InterpretErrorKind::FunctionMissingData,
        ),
        (
            &b"/Bad [/DeviceN [] /DeviceGray << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>]"[..],
            InterpretErrorKind::InvalidDeviceNColorSpace,
        ),
        (
            &b"/Bad [/DeviceN [/All] /DeviceGray << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>]"[..],
            InterpretErrorKind::InvalidDeviceNColorSpace,
        ),
        (
            &b"/Bad [/DeviceN /Spot /DeviceGray << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>]"[..],
            InterpretErrorKind::InvalidDeviceNColorSpace,
        ),
        (
            &b"/Bad [/DeviceN [/Spot] /DeviceGray << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >> 7]"[..],
            InterpretErrorKind::InvalidDeviceNColorSpace,
        ),
    ] {
        let source = calibrated_color_fixture(definition, b"/Bad cs 0 sc");
        let error = interpret_color_page(&source)
            .expect_err("unsafe tint colour-space semantics must be refused");
        assert_eq!(error.kind(), expected, "{:?}", String::from_utf8_lossy(definition));
    }
}

#[test]
fn indexed_palette_over_a_separation_base_uses_tint_domains() {
    let source = calibrated_color_fixture(
        b"/SEP [/Separation /Spot /DeviceGray << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>] /IDX [/Indexed /SEP 1 <0080>]",
        b"/IDX cs 1 sc 0 0 1 1 re f",
    );
    let graph = interpret_color_page(&source).expect("Indexed over a Separation base");
    let ColorSpace::Indexed(definition) = &path(&graph.atoms[0]).state.fill_color_space.value
    else {
        panic!("expected Indexed")
    };
    assert!(matches!(definition.base.value, IndexedBase::Separation(_)));
    assert_floats(&definition.base_components(1), &[128.0 / 255.0]);
}

#[test]
fn icc_based_color_retains_profile_header_parameters_and_provenance() {
    let profile = icc_profile_bytes(*b"RGB ");
    let source = icc_color_fixture(
        b"/N 3 /Alternate [/CalRGB << /WhitePoint [.9505 1 1.089] >>] /Range [-1 2 0 .8 .2 1]",
        &profile,
        b"/ICC cs -2 .9 .1 sc 0 0 1 1 re f",
    );
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("page with ICCBased colour space");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("ICC colour operations");
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
    .expect("structurally valid ICCBased colour space");
    let paint = path(&graph.atoms[0]);
    let ColorSpace::IccBased(definition) = &paint.state.fill_color_space.value else {
        panic!("expected ICCBased")
    };
    assert_eq!(definition.reference, pdf_syntax::Reference::new(5, 0));
    assert_eq!(definition.profile.as_ref(), profile);
    assert_eq!(definition.header.version, [4, 3, 0, 0]);
    assert_eq!(definition.header.profile_class, *b"mntr");
    assert_eq!(definition.header.data_color_space, *b"RGB ");
    assert_eq!(definition.header.connection_space, *b"XYZ ");
    assert_eq!(definition.header.tag_count, 1);
    assert_eq!(definition.components.value, 3);
    assert_eq!(definition.components.provenance.len(), 1);
    assert_floats(&definition.range.value, &[-1.0, 2.0, 0.0, 0.8, 0.2, 1.0]);
    assert_eq!(definition.range.provenance.len(), 1);
    assert!(matches!(
        definition.alternate.value,
        IccAlternate::CalRgb(_)
    ));
    assert_eq!(definition.alternate.provenance.len(), 1);
    assert_eq!(
        paint.state.fill_color.value,
        Color::IccBased(vec![-1.0, 0.8, 0.2])
    );
    assert_eq!(paint.state.fill_color_space.provenance.len(), 2);
}

#[test]
fn malformed_icc_profiles_and_parameters_fail_closed() {
    let rgb = icc_profile_bytes(*b"RGB ");
    let mut bad_signature = rgb.clone();
    bad_signature[36..40].copy_from_slice(b"nope");
    for (entries, profile, expected) in [
        (
            &b"/Alternate /DeviceRGB"[..],
            rgb.as_slice(),
            InterpretErrorKind::IccProfileMissingEntry,
        ),
        (
            &b"/N 4"[..],
            rgb.as_slice(),
            InterpretErrorKind::InvalidIccProfile,
        ),
        (
            &b"/N 3 /Range [0 1 0 1]"[..],
            rgb.as_slice(),
            InterpretErrorKind::InvalidIccProfileEntry,
        ),
        (
            &b"/N 3 /Alternate /DeviceGray"[..],
            rgb.as_slice(),
            InterpretErrorKind::InvalidIccProfileEntry,
        ),
        (
            &b"/N 3"[..],
            bad_signature.as_slice(),
            InterpretErrorKind::InvalidIccProfile,
        ),
    ] {
        let source = icc_color_fixture(entries, profile, b"/ICC cs 0 0 0 sc");
        let page = load_page_program_strict(&source, 0, PageContentLimits::default())
            .expect("page with ICC boundary");
        let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
            .expect("ICC operation");
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
        .expect_err("unsafe ICC semantics must be refused");
        assert_eq!(error.kind(), expected);
    }
}

#[test]
fn icc_based_profile_is_a_bounded_transparency_blend_space() {
    let profile = icc_profile_bytes(*b"RGB ");
    let source = icc_group_fixture(&profile);
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("page with ICC transparency group");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("Form invocation");
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
    .expect("RGB ICC group blend space");
    let PaintAtomKind::TransparencyGroup(group) = &graph.atoms[0].kind else {
        panic!("expected transparency group")
    };
    let Some(ColorSpace::IccBased(space)) = group.blend_space.as_ref().map(|space| &space.value)
    else {
        panic!("expected ICCBased blend space")
    };
    assert_eq!(space.reference, pdf_syntax::Reference::new(6, 0));
    assert_floats(&space.range.value, &[0.0, 1.0, 0.0, 1.0, 0.0, 1.0]);
    assert!(space.range.provenance.is_empty());
}

#[test]
fn color_space_depth_and_icc_profile_limits_are_enforced() {
    let profile = icc_profile_bytes(*b"RGB ");
    let source = icc_color_fixture(b"/N 3", &profile, b"/ICC cs 0 0 0 sc");
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("page with ICC resource limit");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("ICC operation");
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
            max_icc_profile_bytes: profile.len() - 1,
            ..PaintLimits::default()
        },
    )
    .expect_err("ICC profile decode limit");
    assert!(matches!(
        error.kind(),
        InterpretErrorKind::IccProfileResource(PageContentErrorKind::Decode(_))
    ));

    let source = calibrated_color_fixture(b"/A /B /B /A", b"/A cs");
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("page with cyclic colour aliases");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("colour alias operation");
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
            max_color_space_depth: 2,
            ..PaintLimits::default()
        },
    )
    .expect_err("cyclic colour aliases");
    assert_eq!(error.kind(), InterpretErrorKind::ColorSpaceDepthLimit);
}
