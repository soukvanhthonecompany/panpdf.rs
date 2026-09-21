use crate::color::ColorSpace;
use crate::error::InterpretErrorKind;
use crate::function::Function;
use crate::test_fixtures::{assert_floats, interpret_color_page, path, tint_stream_fixture};

#[test]
fn an_order_of_one_is_the_interpolation_this_reader_does_and_three_is_not() {
    let build = |order: &[u8]| {
        let mut entries =
            b"/FunctionType 0 /Domain [0 1] /Range [0 1] /Size [4] /BitsPerSample 4".to_vec();
        entries.extend_from_slice(order);
        tint_stream_fixture(
            b"/SEP [/Separation /Spot /DeviceGray 5 0 R]",
            &entries,
            &[0x05, 0xAF],
            b"/SEP cs 1 scn 0 0 1 1 re f",
        )
    };
    for order in [&b""[..], b" /Order 1"] {
        interpret_color_page(&build(order)).expect("linear interpolation is what this does");
    }
    let error = interpret_color_page(&build(b" /Order 3"))
        .expect_err("a cubic spline is not the curve this reader draws");
    assert_eq!(error.kind(), InterpretErrorKind::UnsupportedFunctionEntry);
}

#[test]
fn sampled_function_addresses_sub_byte_samples_and_interpolates() {
    let source = tint_stream_fixture(
        b"/SEP [/Separation /Spot /DeviceGray 5 0 R]",
        b"/FunctionType 0 /Domain [0 1] /Range [0 1] /Size [4] /BitsPerSample 4",
        &[0x05, 0xAF],
        b"/SEP cs 1 scn 0 0 1 1 re f",
    );
    let graph = interpret_color_page(&source).expect("valid sampled tint transform");
    let ColorSpace::Separation(definition) = &path(&graph.atoms[0]).state.fill_color_space.value
    else {
        panic!("expected Separation")
    };
    let Function::Sampled(function) = &definition.tint_transform.value else {
        panic!("expected a sampled tint transform")
    };
    let raw: Vec<_> = (0..4)
        .map(|index| function.raw_sample(index).expect("addressable sample"))
        .collect();
    assert_floats(&raw, &[0.0, 5.0, 10.0, 15.0]);
    assert!(function.raw_sample(4).is_none());
    assert_floats(&definition.alternate_components(0.0).expect("tint"), &[0.0]);
    assert_floats(&definition.alternate_components(1.0).expect("tint"), &[1.0]);
    assert_floats(
        &definition.alternate_components(1.0 / 3.0).expect("tint"),
        &[1.0 / 3.0],
    );
    assert_floats(&definition.alternate_components(0.5).expect("tint"), &[0.5]);
}

#[test]
fn malformed_sampled_functions_and_input_limits_fail_closed() {
    for (entries, data, expected) in [
        (
            &b"/FunctionType 0 /Domain [0 1] /Range [0 1] /Size [4] /BitsPerSample 8"[..],
            &[0_u8, 255][..],
            InterpretErrorKind::InvalidFunction,
        ),
        (
            &b"/FunctionType 0 /Domain [0 1] /Range [0 1] /Size [2] /BitsPerSample 5"[..],
            &[0_u8, 255][..],
            InterpretErrorKind::InvalidFunction,
        ),
        (
            &b"/FunctionType 0 /Domain [0 1] /Range [0 1] /Size [0] /BitsPerSample 8"[..],
            &[0_u8, 255][..],
            InterpretErrorKind::InvalidFunction,
        ),
        (
            &b"/FunctionType 0 /Domain [0 1] /Range [0 1] /Size [2 2] /BitsPerSample 8"[..],
            &[0_u8, 255][..],
            InterpretErrorKind::InvalidFunction,
        ),
        (
            &b"/FunctionType 0 /Domain [0 1] /Size [2] /BitsPerSample 8"[..],
            &[0_u8, 255][..],
            InterpretErrorKind::FunctionMissingEntry,
        ),
        (
            &b"/FunctionType 0 /Domain [0 1] /Range [0 1] /Size [2] /BitsPerSample 8 /Order 3"[..],
            &[0_u8, 255][..],
            InterpretErrorKind::UnsupportedFunctionEntry,
        ),
    ] {
        let source = tint_stream_fixture(
            b"/SEP [/Separation /Spot /DeviceGray 5 0 R]",
            entries,
            data,
            b"/SEP cs 1 scn 0 0 1 1 re f",
        );
        let error =
            interpret_color_page(&source).expect_err("malformed sampled function must be refused");
        assert_eq!(
            error.kind(),
            expected,
            "{:?}",
            String::from_utf8_lossy(entries)
        );
    }

    let domain = b"/FunctionType 0 /Domain [0 1 0 1 0 1 0 1 0 1 0 1 0 1 0 1 0 1] /Range [0 1] /Size [1 1 1 1 1 1 1 1 1] /BitsPerSample 8";
    let colorants = b"/DN [/DeviceN [/A /B /C /D /E /F /G /H /I] /DeviceGray 5 0 R]";
    let source = tint_stream_fixture(
        colorants,
        domain,
        &[255_u8],
        b"/DN cs 1 1 1 1 1 1 1 1 1 scn 0 0 1 1 re f",
    );
    let error = interpret_color_page(&source).expect_err("input limit must be enforced");
    assert_eq!(error.kind(), InterpretErrorKind::InvalidDeviceNColorSpace);
}
