use crate::ccitt::CcittError;
use crate::color::ColorSpace;
use crate::error::InterpretErrorKind;
use crate::graph::{PaintAtomKind, PaintGraph};
use crate::image::DctParameterLocation;
use crate::test_fixtures::{
    ADOBE_CMYK_JPEG, COLOR_JPEG, COLOR_JPEG_RGB, GRAY_JPEG, GRAY_JPEG_SAMPLES, PLAIN_CMYK_JPEG,
    RED_JPEG, default_space_fixture, hex_fixture, image_paint_fixture, interpret_color_page,
    resource_fixture,
};

#[test]
fn image_xobject_retains_samples_decode_mask_and_source_identity() {
    let image_data = [255, 0, 0, 0, 255, 0];
    let mask_data = [255, 128];
    let source = image_paint_fixture(
        b"/Width 2 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Decode [1 0 0 1 0 1] /Interpolate true /SMask 6 0 R",
        &image_data,
        Some((
            b"/Width 2 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Matte [1 1 1]",
            &mask_data,
        )),
        None,
    );
    let graph = interpret_color_page(&source).expect("raw image paint graph");
    assert_eq!(graph.atoms.len(), 1);
    let atom = &graph.atoms[0];
    assert_eq!(atom.id.page, pdf_syntax::Reference::new(3, 0));
    assert_eq!(atom.id.stream, pdf_syntax::Reference::new(4, 0));
    let PaintAtomKind::Image(image) = &atom.kind else {
        panic!("image atom");
    };
    assert_eq!(image.reference, pdf_syntax::Reference::new(5, 0));
    assert_eq!((image.width.value, image.height.value), (2, 1));
    assert_eq!(image.bits_per_component.value, 8);
    assert_eq!(image.components(), 3);
    assert_eq!(&*image.samples, image_data.as_slice());
    assert_eq!(
        source
            .resolve(image.encoded_data_span)
            .expect("encoded image provenance"),
        image_data
    );
    assert!((image.sample(0, 0, 0).expect("red sample") - 0.0).abs() < f64::EPSILON);
    assert!((image.sample(1, 0, 0).expect("inverted zero") - 1.0).abs() < f64::EPSILON);
    assert!((image.sample(1, 0, 1).expect("green sample") - 1.0).abs() < f64::EPSILON);
    assert_eq!(image.sample(2, 0, 0), None);
    assert_eq!(image.decode.provenance.len(), 1);
    assert!(image.interpolate.value);

    let mask = image.soft_mask.as_ref().expect("soft mask image");
    assert_eq!(mask.reference, pdf_syntax::Reference::new(6, 0));
    assert_eq!(mask.components(), 1);
    assert_eq!(&*mask.samples, mask_data.as_slice());
    assert_eq!(mask.matte.as_ref().expect("matte").value, vec![1.0; 3]);

    let source = image_paint_fixture(
        b"/Width 1 /Height 1 /ColorSpace 6 0 R /BitsPerComponent 8",
        &[1, 2, 3],
        None,
        Some(b"/DeviceRGB"),
    );
    let graph = interpret_color_page(&source).expect("indirect image colour space");
    let PaintAtomKind::Image(image) = &graph.atoms[0].kind else {
        panic!("image atom");
    };
    assert_eq!(
        image
            .color_space
            .as_ref()
            .expect("colour space")
            .provenance
            .len(),
        2
    );
}

#[test]
fn a_whole_pixel_reads_the_same_components_one_at_a_time_does() {
    for (bits, data) in [
        (1_u8, vec![0b1011_0100, 0b0100_0000]),
        (
            2_u8,
            vec![0b1100_1001, 0b0110_0000, 0b0001_1011, 0b1010_0000],
        ),
        (4_u8, vec![0x1F, 0x2E, 0x3D, 0x4C, 0x5B, 0x6A]),
        (8_u8, vec![1, 2, 3, 250, 251, 252, 7, 8, 9, 10, 11, 12]),
        (16_u8, (0..24).map(|byte| byte * 9 + 1).collect::<Vec<u8>>()),
    ] {
        let entries = format!("/Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent {bits}");
        let source = image_paint_fixture(entries.as_bytes(), &data, None, None);
        let graph = interpret_color_page(&source).expect("image with samples");
        let PaintAtomKind::Image(image) = &graph.atoms[0].kind else {
            panic!("image atom");
        };
        let mut pixel = [0.0; 3];
        for y in 0..2 {
            for x in 0..2 {
                assert!(
                    image.sample_pixel(x, y, &mut pixel),
                    "{bits} bits at {x},{y}"
                );
                for (component, whole) in pixel.iter().enumerate() {
                    let one = image
                        .sample(x, y, component)
                        .expect("component read one at a time");
                    assert!(
                        (whole - one).abs() < f64::EPSILON,
                        "{bits} bits at {x},{y} component {component}: {whole} vs {one}"
                    );
                }
            }
        }
        assert!(!image.sample_pixel(2, 0, &mut pixel), "outside the image");
        assert!(
            !image.sample_pixel(0, 0, &mut pixel[..2]),
            "buffer too short"
        );
    }
}

#[test]
fn image_codecs_and_ambiguous_samples_fail_closed() {
    let source = image_paint_fixture(
        b"/Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode",
        b"not a jpeg",
        None,
        None,
    );
    let error = interpret_color_page(&source).expect_err("malformed DCT payload");
    assert_eq!(error.kind(), InterpretErrorKind::DctDecodeFailure);

    let source = image_paint_fixture(
        b"/Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /JPXDecode",
        b"not jpx",
        None,
        None,
    );
    let error = interpret_color_page(&source).expect_err("malformed JPX payload");
    assert_eq!(
        error.kind(),
        InterpretErrorKind::JpxDecodeFailure(jpeg2000::ErrorKind::NotACodestream)
    );

    let source = image_paint_fixture(
        b"/Width 2 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8",
        &[0; 5],
        None,
        None,
    );
    let error = interpret_color_page(&source).expect_err("short image samples");
    assert_eq!(error.kind(), InterpretErrorKind::ImageSampleShortfall);

    let source = image_paint_fixture(
        b"/Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Mask [0 0]",
        &[0; 3],
        None,
        None,
    );
    let error = interpret_color_page(&source).expect_err("colour-key mask of the wrong arity");
    assert_eq!(error.kind(), InterpretErrorKind::InvalidImageEntry);

    let source = image_paint_fixture(
        b"/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Halftone 6 0 R",
        &[0; 1],
        None,
        Some(b"<< /Type /Halftone >>"),
    );
    let error = interpret_color_page(&source).expect_err("a key with no meaning here");
    assert_eq!(error.kind(), InterpretErrorKind::UnsupportedImageEntry);
    let key = error.operator_span().expect("the offending key's span");
    assert_eq!(source.resolve(key).expect("key bytes"), b"/Halftone");

    let source = image_paint_fixture(
        b"/Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Metadata 6 0 R /ImageName /ps3EE6.jpg /Name /ps3EE6.jpg",
        &[7, 8, 9],
        None,
        Some(b"<< /Type /Metadata /Subtype /XML >>"),
    );
    let graph = interpret_color_page(&source).expect("labels do not describe samples");
    let PaintAtomKind::Image(image) = &graph.atoms[0].kind else {
        panic!("image atom");
    };
    assert_eq!(&*image.samples, &[7, 8, 9]);
}

#[test]
fn an_adobe_cmyk_jpeg_has_its_samples_inverted_and_a_plain_one_does_not() {
    let read = |hex: &str| {
        let source = image_paint_fixture(
            b"/Width 1 /Height 1 /ColorSpace /DeviceCMYK /BitsPerComponent 8 /Filter /DCTDecode",
            &hex_fixture(hex),
            None,
            None,
        );
        let graph = interpret_color_page(&source).expect("CMYK JPEG");
        let PaintAtomKind::Image(image) = &graph.atoms[0].kind else {
            panic!("image atom")
        };
        image.samples.to_vec()
    };

    let plain = read(PLAIN_CMYK_JPEG);
    let adobe = read(ADOBE_CMYK_JPEG);
    assert_eq!(plain.len(), 4);
    assert_eq!(adobe.len(), 4);
    for (stored, inverted) in plain.iter().zip(&adobe) {
        assert_eq!(u16::from(*stored) + u16::from(*inverted), 255);
    }
    assert_ne!(plain, adobe);
}

#[test]
fn distiller_encoder_settings_are_ignored_and_other_decode_parameters_are_not() {
    let jpeg = hex_fixture(RED_JPEG);
    let source = image_paint_fixture(
        b"/Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /DecodeParms << /Columns 1 /Rows 1 /Colors 3 /HSamples [2 1 1 2] /VSamples [2 1 1 2] /QFactor 0.800003 /Blend 1 /ColorTransform 1 >>",
        &jpeg,
        None,
        None,
    );
    let graph = interpret_color_page(&source).expect("Distiller encoder settings are ignored");
    let PaintAtomKind::Image(image) = &graph.atoms[0].kind else {
        panic!("DCT image atom");
    };
    assert_eq!(&*image.samples, &[254, 0, 0]);
    assert_eq!(
        image
            .dct_color_transform
            .as_ref()
            .expect("DCT ColorTransform")
            .value
            .value,
        1
    );

    let source = image_paint_fixture(
        b"/Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /DecodeParms << /Predictor 15 /ColorTransform 1 >>",
        &jpeg,
        None,
        None,
    );
    let error = interpret_color_page(&source)
        .expect_err("a decode parameter outside that set still fails closed");
    assert_eq!(
        error.kind(),
        InterpretErrorKind::UnsupportedImageCodecParameters
    );
    let key = error.operator_span().expect("the offending key's span");
    assert_eq!(source.resolve(key).expect("key bytes"), b"/Predictor");
}

#[test]
fn ccitt_decode_parameters_are_read_from_the_dictionary() {
    let fax = hex_fixture("c0040040");
    let read = |parameters: &[u8]| {
        let mut entries = b"/Width 8 /Height 2 /ColorSpace /DeviceGray \
/BitsPerComponent 1 /Filter /CCITTFaxDecode /DecodeParms "
            .to_vec();
        entries.extend_from_slice(parameters);
        let source = image_paint_fixture(&entries, &fax, None, None);
        interpret_color_page(&source)
    };

    let graph = read(b"<< /K -1 /Columns 8 /Rows 1 >>").expect("/EndOfBlock overrides /Rows");
    let PaintAtomKind::Image(image) = &graph.atoms[0].kind else {
        panic!("CCITT image atom");
    };
    assert_eq!(&*image.samples, &[0xff, 0xff]);

    let error = read(b"<< /K -1 /Columns 8 /Rows 1 /EndOfBlock false >>")
        .expect_err("a terminating /Rows short of the image is refused");
    assert_eq!(
        error.kind(),
        InterpretErrorKind::CcittDecodeFailure(CcittError::RowsShortOfImage { rows: 1, height: 2 })
    );

    let error = read(b"<< /K -1 /Columns 8 /Predictor 15 >>")
        .expect_err("an unknown decode parameter is refused");
    assert_eq!(
        error.kind(),
        InterpretErrorKind::UnsupportedImageCodecParameters
    );
}

#[test]
fn dct_color_transform_is_read_only_from_decode_parameters() {
    const HEAD: &[u8] =
        b"/Width 4 /Height 4 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode";
    let colour = hex_fixture(COLOR_JPEG);
    let rgb = |entries: &[u8]| {
        let source = image_paint_fixture(entries, &colour, None, None);
        interpret_color_page(&source)
    };
    let samples = |graph: &PaintGraph| {
        let PaintAtomKind::Image(image) = &graph.atoms[0].kind else {
            panic!("DCT image atom");
        };
        image.samples.to_vec()
    };

    let graph = rgb(&[HEAD, b" /ColorTransform 0"].concat())
        .expect("an image-dictionary /ColorTransform is not a decode parameter");
    assert_eq!(samples(&graph), COLOR_JPEG_RGB);
    let PaintAtomKind::Image(image) = &graph.atoms[0].kind else {
        panic!("DCT image atom");
    };
    let transform = image
        .dct_color_transform
        .as_ref()
        .expect("the entry is kept as provenance");
    assert_eq!(transform.value.value, 0);
    assert_eq!(
        transform.location,
        DctParameterLocation::ImageDictionaryExtension
    );

    let bare = rgb(HEAD).expect("no /ColorTransform anywhere");
    assert_eq!(samples(&bare), COLOR_JPEG_RGB);

    let error = rgb(&[HEAD, b" /DecodeParms << /ColorTransform 0 >>"].concat())
        .expect_err("no transformation on three components is not implemented");
    assert_eq!(
        error.kind(),
        InterpretErrorKind::UnsupportedImageCodecParameters
    );

    let graph = rgb(&[
        HEAD,
        b" /ColorTransform 0 /DecodeParms << /ColorTransform 1 >>",
    ]
    .concat())
    .expect("/DecodeParms is the parameter");
    assert_eq!(samples(&graph), COLOR_JPEG_RGB);
    let PaintAtomKind::Image(image) = &graph.atoms[0].kind else {
        panic!("DCT image atom");
    };
    let transform = image
        .dct_color_transform
        .as_ref()
        .expect("the decode parameter");
    assert_eq!(transform.value.value, 1);
    assert_eq!(transform.location, DctParameterLocation::DecodeParameters);

    let error =
        rgb(&[HEAD, b" /ColorTransform 7"].concat()).expect_err("an out-of-range /ColorTransform");
    assert_eq!(
        error.kind(),
        InterpretErrorKind::InvalidImageCodecParameters
    );

    let source = image_paint_fixture(
        &[HEAD, b" /ColorTransform 0"].concat(),
        &colour[..colour.len() / 2],
        None,
        None,
    );
    let error = interpret_color_page(&source).expect_err("truncated JPEG");
    assert_eq!(error.kind(), InterpretErrorKind::DctDecodeFailure);

    let grey = hex_fixture(GRAY_JPEG);
    let source = image_paint_fixture(
        b"/Width 4 /Height 4 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /DCTDecode /DecodeParms << /ColorTransform 0 >>",
        &grey,
        None,
        None,
    );
    let graph = interpret_color_page(&source).expect("greyscale /ColorTransform 0");
    assert_eq!(samples(&graph), GRAY_JPEG_SAMPLES);
}

#[test]
fn dct_image_matches_independent_pixel_answer_and_dictionary() {
    let jpeg = hex_fixture(RED_JPEG);
    let source = image_paint_fixture(
        b"/Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode",
        &jpeg,
        None,
        None,
    );
    let graph = interpret_color_page(&source).expect("DCT image paint graph");
    let PaintAtomKind::Image(image) = &graph.atoms[0].kind else {
        panic!("DCT image atom");
    };
    assert_eq!(&*image.samples, &[254, 0, 0]);
    assert_eq!(image.sample(0, 0, 0), Some(254.0 / 255.0));
    let codec = image.codec.as_ref().expect("DCT codec provenance");
    assert_eq!(codec.value, pdf_syntax::ImageCodec::Dct);
    assert_eq!(
        source
            .resolve(*codec.provenance.first().expect("filter provenance"))
            .expect("DCT filter token"),
        b"/DCTDecode"
    );
    assert_eq!(image.dct_color_transform, None);

    let source = image_paint_fixture(
        b"/Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /ColorTransform 1 /Intent /Perceptual",
        &jpeg,
        None,
        None,
    );
    let graph = interpret_color_page(&source).expect("Skia-style direct ColorTransform");
    let PaintAtomKind::Image(image) = &graph.atoms[0].kind else {
        panic!("DCT image atom");
    };
    let transform = image
        .dct_color_transform
        .as_ref()
        .expect("direct DCT ColorTransform");
    assert_eq!(transform.value.value, 1);
    assert_eq!(
        transform.location,
        DctParameterLocation::ImageDictionaryExtension
    );
    assert_eq!(&*image.samples, &[254, 0, 0]);
    assert_eq!(image.state.rendering_intent.value, b"/Perceptual");
    assert_eq!(image.state.rendering_intent.provenance.len(), 1);

    let source = image_paint_fixture(
        b"/Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /ColorTransform 0",
        &jpeg,
        None,
        None,
    );
    let graph = interpret_color_page(&source).expect("Skia-style /ColorTransform 0");
    let PaintAtomKind::Image(image) = &graph.atoms[0].kind else {
        panic!("DCT image atom");
    };
    assert_eq!(&*image.samples, &[254, 0, 0]);

    let source = image_paint_fixture(
        b"/Width 2 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode",
        &jpeg,
        None,
        None,
    );
    let error = interpret_color_page(&source).expect_err("PDF width disagrees with JPEG");
    assert_eq!(error.kind(), InterpretErrorKind::DctMetadataMismatch);

    let source = image_paint_fixture(
        b"/Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /DecodeParms << /ColorTransform 2 >>",
        &jpeg,
        None,
        None,
    );
    let error = interpret_color_page(&source).expect_err("invalid DCT ColorTransform");
    assert_eq!(
        error.kind(),
        InterpretErrorKind::InvalidImageCodecParameters
    );
}

#[test]
fn a_stencil_mask_hides_exactly_the_samples_it_marks() {
    let image_data = [255, 0, 0, 0, 255, 0];
    let mask_data = [0b1000_0000];
    let source = image_paint_fixture(
        b"/Width 2 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Mask 6 0 R",
        &image_data,
        Some((
            b"/Width 2 /Height 1 /ImageMask true /BitsPerComponent 1",
            &mask_data,
        )),
        None,
    );
    let graph = interpret_color_page(&source).expect("stencil-masked image");
    let PaintAtomKind::Image(image) = &graph.atoms[0].kind else {
        panic!("image atom");
    };
    let Some(crate::ImageMask::Stencil(stencil)) = image.mask.as_ref() else {
        panic!("stencil mask");
    };
    assert_eq!(stencil.reference, pdf_syntax::Reference::new(6, 0));
    assert!(stencil.image_mask.value);
    assert!(image.masked_out(0, 0), "the sample the mask sets is hidden");
    assert!(
        !image.masked_out(1, 0),
        "the sample the mask clears is drawn"
    );
    assert!((image.sample(0, 0, 0).expect("red") - 1.0).abs() < f64::EPSILON);
}

#[test]
fn a_stencil_mask_of_another_size_is_resampled_not_indexed() {
    let image_data = [0, 0, 0, 0];
    let mask_data = [0b1000_0000];
    let source = image_paint_fixture(
        b"/Width 4 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Mask 6 0 R",
        &image_data,
        Some((
            b"/Width 2 /Height 1 /ImageMask true /BitsPerComponent 1",
            &mask_data,
        )),
        None,
    );
    let graph = interpret_color_page(&source).expect("coarse stencil mask");
    let PaintAtomKind::Image(image) = &graph.atoms[0].kind else {
        panic!("image atom");
    };
    assert!(image.masked_out(0, 0));
    assert!(image.masked_out(1, 0));
    assert!(!image.masked_out(2, 0));
    assert!(!image.masked_out(3, 0));
}

#[test]
fn a_colour_key_range_hides_only_the_samples_inside_every_range() {
    let image_data = [255, 0, 0, 0, 255, 0, 250, 4, 4];
    let source = image_paint_fixture(
        b"/Width 3 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Mask [200 255 0 10 0 10]",
        &image_data,
        None,
        None,
    );
    let graph = interpret_color_page(&source).expect("colour-key masked image");
    let PaintAtomKind::Image(image) = &graph.atoms[0].kind else {
        panic!("image atom");
    };
    let Some(crate::ImageMask::ColorKey(ranges)) = image.mask.as_ref() else {
        panic!("colour-key mask");
    };
    assert_eq!(ranges.value, vec![200, 255, 0, 10, 0, 10]);
    assert!(image.masked_out(0, 0), "255,0,0 is inside every range");
    assert!(
        !image.masked_out(1, 0),
        "0,255,0 is outside the first range"
    );
    assert!(image.masked_out(2, 0), "250,4,4 is inside every range");
}

#[test]
fn a_stencil_mask_reads_its_own_decode_array() {
    let image_data = [0, 0];
    let mask_data = [0b1000_0000];
    let source = image_paint_fixture(
        b"/Width 2 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Mask 6 0 R",
        &image_data,
        Some((
            b"/Width 2 /Height 1 /ImageMask true /BitsPerComponent 1 /Decode [1 0]",
            &mask_data,
        )),
        None,
    );
    let graph = interpret_color_page(&source).expect("inverted stencil mask");
    let PaintAtomKind::Image(image) = &graph.atoms[0].kind else {
        panic!("image atom");
    };
    assert!(!image.masked_out(0, 0), "/Decode [1 0] reverses the sense");
    assert!(image.masked_out(1, 0));
}

#[test]
fn a_mask_this_engine_cannot_read_is_refused_by_name() {
    let source = image_paint_fixture(
        b"/Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Mask [0 0 0 0]",
        &[0, 0, 0],
        None,
        None,
    );
    assert_eq!(
        interpret_color_page(&source)
            .expect_err("wrong arity")
            .kind(),
        InterpretErrorKind::InvalidImageEntry
    );

    let source = image_paint_fixture(
        b"/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Mask [200 100]",
        &[0],
        None,
        None,
    );
    assert_eq!(
        interpret_color_page(&source)
            .expect_err("reversed range")
            .kind(),
        InterpretErrorKind::InvalidImageEntry
    );

    let source = image_paint_fixture(
        b"/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Mask 6 0 R",
        &[0],
        Some((
            b"/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8",
            &[0],
        )),
        None,
    );
    assert_eq!(
        interpret_color_page(&source)
            .expect_err("not a stencil")
            .kind(),
        InterpretErrorKind::InvalidImageEntry
    );
}

#[test]
fn an_image_with_no_mask_hides_nothing() {
    let source = image_paint_fixture(
        b"/Width 2 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8",
        &[255, 0, 0, 0, 255, 0],
        None,
        None,
    );
    let graph = interpret_color_page(&source).expect("unmasked image");
    let PaintAtomKind::Image(image) = &graph.atoms[0].kind else {
        panic!("image atom");
    };
    assert!(image.mask.is_none());
    assert!(!image.masked_out(0, 0));
    assert!(!image.masked_out(1, 0));
}

#[test]
fn an_inline_image_is_read_through_its_abbreviated_dictionary() {
    let content: &[u8] = b"q 20 0 0 20 5 5 cm BI /W 2 /H 2 /BPC 8 /CS /G ID \x00\x40\x80\xff EI Q";
    let source = resource_fixture(content, b"<< >>");
    let graph = interpret_color_page(&source).expect("the page interprets");
    let images: Vec<&crate::ImagePaint> = graph
        .atoms
        .iter()
        .filter_map(|atom| match &atom.kind {
            crate::PaintAtomKind::Image(image) => Some(image.as_ref()),
            _ => None,
        })
        .collect();
    assert_eq!(images.len(), 1, "one inline image");
    let image = images[0];
    assert_eq!((image.width.value, image.height.value), (2, 2));
    assert_eq!(image.bits_per_component.value, 8);
    assert_eq!(
        image.samples.as_ref(),
        &[0x00, 0x40, 0x80, 0xff],
        "four greyscale samples, as the stream wrote them"
    );
    assert!(!image.image_mask.value);
    assert!(matches!(
        image.color_space.as_ref().map(|space| &space.value),
        Some(crate::ColorSpace::DeviceGray)
    ));
    assert!((image.state.ctm.value.a - 20.0).abs() < 1e-9);
}

#[test]
fn a_default_colour_space_replaces_the_device_space_an_image_names() {
    let space_of = |source: &pdf_bytes::ByteStore| {
        let graph = crate::test_fixtures::interpret_fixture(source).expect("the page draws");
        let PaintAtomKind::Image(image) = &graph.atoms[0].kind else {
            panic!("the page draws one image");
        };
        image.color_space.as_ref().map(|space| space.value.clone())
    };
    let cal: &[u8] =
        b"/DefaultRGB [/CalRGB << /WhitePoint [0.9505 1.0 1.089] /Gamma [2.2 2.2 2.2] >>]";

    let plain = default_space_fixture(b"", b"/DeviceRGB", &[10, 20, 30]);
    assert!(matches!(space_of(&plain), Some(ColorSpace::DeviceRgb)));

    let defaulted = default_space_fixture(cal, b"/DeviceRGB", &[10, 20, 30]);
    assert!(matches!(space_of(&defaulted), Some(ColorSpace::CalRgb(_))));

    let gray = default_space_fixture(cal, b"/DeviceGray", &[10]);
    assert!(matches!(space_of(&gray), Some(ColorSpace::DeviceGray)));

    let mismatched = default_space_fixture(
        b"/DefaultRGB [/CalGray << /WhitePoint [0.9505 1.0 1.089] >>]",
        b"/DeviceRGB",
        &[10, 20, 30],
    );
    assert!(matches!(space_of(&mismatched), Some(ColorSpace::DeviceRgb)));

    let circular = default_space_fixture(b"/DefaultRGB /DeviceRGB", b"/DeviceRGB", &[10, 20, 30]);
    assert!(matches!(space_of(&circular), Some(ColorSpace::DeviceRgb)));
}

#[test]
fn the_device_colour_operators_deliberately_ignore_the_default_spaces() {
    let cal: &[u8] =
        b"/DefaultRGB [/CalRGB << /WhitePoint [0.9505 1.0 1.089] /Gamma [2.2 2.2 2.2] >>]";
    let source = crate::test_fixtures::default_space_content_fixture(
        cal,
        b"0.8 0.35 0.15 rg 0 0 40 40 re f",
    );
    let graph = crate::test_fixtures::interpret_fixture(&source).expect("the page draws");
    let PaintAtomKind::Path(path) = &graph.atoms[0].kind else {
        panic!("the page draws one path");
    };
    assert!(matches!(
        path.state.fill_color_space.value,
        ColorSpace::DeviceRgb
    ));
}

mod jpx_fixture {
    pub const GREY_TILED_J2K: &[u8] = include_bytes!("fixtures/grey-tiled.j2k");
    include!("fixtures/grey-tiled.rs");
}

#[test]
fn a_jpeg_2000_image_arrives_as_samples_the_dictionary_can_address() {
    let source = image_paint_fixture(
        b"/Width 20 /Height 14 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /JPXDecode",
        jpx_fixture::GREY_TILED_J2K,
        None,
        None,
    );
    let graph = interpret_color_page(&source).expect("a JPEG 2000 image");
    let PaintAtomKind::Image(image) = &graph.atoms[0].kind else {
        panic!("image atom");
    };
    assert_eq!(image.components(), 1);
    assert_eq!(&*image.samples, jpx_fixture::GREY_TILED);
}

#[test]
fn a_jpeg_2000_image_whose_dictionary_disagrees_with_it_is_refused() {
    let source = image_paint_fixture(
        b"/Width 21 /Height 14 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /JPXDecode",
        jpx_fixture::GREY_TILED_J2K,
        None,
        None,
    );
    let error = interpret_color_page(&source).expect_err("the dictionary disagrees");
    assert_eq!(error.kind(), InterpretErrorKind::JpxMetadataMismatch);
}

#[test]
fn a_jpeg_2000_image_that_miscounts_its_tile_parts_is_drawn_and_reported() {
    let mut damaged = jpx_fixture::GREY_TILED_J2K.to_vec();
    let mut offset = 0;
    while offset + 12 < damaged.len() {
        if damaged[offset] == 0xFF && damaged[offset + 1] == 0x90 {
            damaged[offset + 11] = 7;
        }
        offset += 1;
    }
    let source = image_paint_fixture(
        b"/Width 20 /Height 14 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /JPXDecode",
        &damaged,
        None,
        None,
    );
    let graph = interpret_color_page(&source).expect("a repairable JPEG 2000 image");
    let PaintAtomKind::Image(image) = &graph.atoms[0].kind else {
        panic!("image atom");
    };
    assert_eq!(&*image.samples, jpx_fixture::GREY_TILED);
    assert!(
        graph.repairs.iter().any(|repair| matches!(
            repair.kind,
            crate::RepairKind::JpxCodestream {
                repair: jpeg2000::Repair::TilePartCountDisagrees { declared: 7, .. }
            }
        )),
        "the codec's repair reaches the graph: {:?}",
        graph.repairs
    );
}
