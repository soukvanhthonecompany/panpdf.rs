use std::fmt::Write as _;

use pdf_bytes::ByteStore;
use pdf_paint::{Matrix, PaintAtomKind, PaintGraph, Point};
use pdf_syntax::Reference;

use crate::image_file::{Colour, ImageFile, Stored};
use crate::plan::{Capability, Effect, MovedRun, Plan, PlannedBody, PlannedWrite, SourceAnchor};
use crate::spike_move_text::{PlannerPage, SpikeError, interpret_bytes_of};

fn refused(reason: &'static str) -> SpikeError {
    SpikeError::RetypeUnsupported(reason)
}

#[derive(Clone, Copy, Debug)]
pub struct NewImage<'a> {
    pub placement: Matrix,
    pub file: &'a [u8],
}

pub(crate) fn plan_new_image(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    new: &NewImage<'_>,
) -> Result<Plan, SpikeError> {
    let region = checked_region(new.placement)?;
    let image = ImageFile::read(new.file).map_err(|error| refused(error.reason()))?;
    let placement = placement(new.placement, &image);

    let first = crate::block_rewrite::next_object_number(source)?;
    let object = Reference::new(first, 0);
    let mut writes = image_writes(&image, first);
    let (name, holder) =
        crate::new_font::add_resource(source, page.program.page, (b"/XObject", "Im"), object)?;
    writes.push(holder);

    let document = crate::block_rewrite::commit_writes(source, &writes, page.restrictions)?;
    let carrying = crate::spike_move_text::read_page(&document, page_index, b"", page.fonts)?;
    let stream = carrying
        .program
        .streams
        .len()
        .checked_sub(1)
        .ok_or_else(|| refused("a page with no content stream cannot be written into"))?;
    let decoded = carrying.program.streams[stream].bytes.as_bytes();

    let measuring = candidate(decoded, &name, None);
    let measured = interpret_bytes_of(&carrying.program, stream, &measuring, page.fonts)?;
    let standing = painted(&carrying.graph, &measured)?.state.ctm.value;
    let into = standing
        .inverse()
        .ok_or_else(|| refused("this page leaves a transform a picture cannot be placed through"))?
        .multiply(placement);
    let bytes = candidate(decoded, &name, Some(into));
    let graph = interpret_bytes_of(&carrying.program, stream, &bytes, page.fonts)?;
    prove_placed(&carrying.graph, &graph, &image, placement)?;

    writes.push(PlannedWrite {
        reference: carrying.program.streams[stream].reference,
        body: PlannedBody::ReplacedStream { decoded: bytes },
    });
    let ordinal = carrying.graph.atoms.len();
    let atom = &graph.atoms[ordinal];
    Ok(Plan::new(
        Capability::Exact,
        writes,
        Effect {
            page_index,
            moved: vec![MovedRun {
                anchor: SourceAnchor::of(&atom.id),
                atom_ordinal: ordinal,
                original_matrix: Matrix::IDENTITY,
            }],
            target_stream: carrying.program.streams[stream].reference,
            declared_region: Some(region),
        },
    ))
}

fn checked_region(placement: Matrix) -> Result<[f64; 4], SpikeError> {
    let m = placement;
    if ![m.a, m.b, m.c, m.d, m.e, m.f]
        .iter()
        .all(|value| value.is_finite())
    {
        return Err(refused("a placement must be six numbers"));
    }
    if placement.inverse().is_none() {
        return Err(refused("a picture has to be given some room"));
    }
    let corners = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)]
        .map(|(x, y)| placement.transform(Point { x, y }));
    Ok(corners.iter().fold(
        [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ],
        |[x0, y0, x1, y1], at| [x0.min(at.x), y0.min(at.y), x1.max(at.x), y1.max(at.y)],
    ))
}

fn placement(placement: Matrix, image: &ImageFile<'_>) -> Matrix {
    let turn = image.orientation_matrix();
    placement.multiply(Matrix {
        a: turn[0],
        b: turn[1],
        c: turn[2],
        d: turn[3],
        e: turn[4],
        f: turn[5],
    })
}

fn image_writes(image: &ImageFile<'_>, first: u32) -> Vec<PlannedWrite> {
    let head = |colour: &str, bits: u8, filter: &str| {
        format!(
            "/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace {colour} \
             /BitsPerComponent {bits} /Filter {filter}",
            image.width, image.height,
        )
    };
    match &image.stored {
        Stored::Jpeg(bytes) => vec![PlannedWrite {
            reference: Reference::new(first, 0),
            body: PlannedBody::NewStream {
                dictionary: {
                    let mut dictionary = head(image.colour.name(), 8, "/DCTDecode");
                    if image.colour == Colour::Cmyk && image.inverted {
                        dictionary.push_str(" /Decode [1 0 1 0 1 0 1 0]");
                    }
                    dictionary.into_bytes()
                },
                decoded: bytes.to_vec(),
            },
        }],
        Stored::Samples {
            bits,
            colour,
            alpha,
        } => {
            let mut dictionary = head(image.colour.name(), *bits, "/FlateDecode");
            let mut writes = Vec::new();
            if let Some(alpha) = alpha {
                let mask = first + 1;
                let _ = write!(dictionary, " /SMask {mask} 0 R");
                writes.push(PlannedWrite {
                    reference: Reference::new(mask, 0),
                    body: PlannedBody::NewStream {
                        dictionary: head(Colour::Gray.name(), *bits, "/FlateDecode").into_bytes(),
                        decoded: pdf_syntax::deflate_zlib(alpha),
                    },
                });
            }
            writes.insert(
                0,
                PlannedWrite {
                    reference: Reference::new(first, 0),
                    body: PlannedBody::NewStream {
                        dictionary: dictionary.into_bytes(),
                        decoded: pdf_syntax::deflate_zlib(colour),
                    },
                },
            );
            writes
        }
    }
}

fn candidate(decoded: &[u8], name: &str, into: Option<Matrix>) -> Vec<u8> {
    let mut out = Vec::with_capacity(decoded.len() + 96);
    out.extend_from_slice(decoded);
    out.extend_from_slice(b"\nq ");
    if let Some(m) = into {
        out.extend_from_slice(
            format!("{} {} {} {} {} {} cm ", m.a, m.b, m.c, m.d, m.e, m.f).as_bytes(),
        );
    }
    out.extend_from_slice(format!("/{name} Do Q\n").as_bytes());
    out
}

fn painted<'a>(
    before: &PaintGraph,
    after: &'a PaintGraph,
) -> Result<&'a pdf_paint::ImagePaint, SpikeError> {
    match after.atoms.get(before.atoms.len()).map(|atom| &atom.kind) {
        Some(PaintAtomKind::Image(image)) => Ok(image),
        _ => Err(refused("the picture written does not paint")),
    }
}

fn prove_placed(
    before: &PaintGraph,
    after: &PaintGraph,
    image: &ImageFile<'_>,
    placement: Matrix,
) -> Result<(), SpikeError> {
    if after.atoms.len() != before.atoms.len() + 1 {
        return Err(SpikeError::MoveNotIsolated);
    }
    crate::new_text::prove_untouched(before, after)?;
    let paint = painted(before, after)?;
    for (x, y) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        let corner = Point { x, y };
        let (got, wanted) = (
            paint.state.ctm.value.transform(corner),
            placement.transform(corner),
        );
        if (got.x - wanted.x).abs() > crate::block_move::PLACEMENT_TOLERANCE
            || (got.y - wanted.y).abs() > crate::block_move::PLACEMENT_TOLERANCE
        {
            return Err(SpikeError::MoveNotIsolated);
        }
    }
    let wrong = || refused("the picture written does not paint as its file says");
    if paint.width.value != image.width || paint.height.value != image.height {
        return Err(wrong());
    }
    match &image.stored {
        Stored::Jpeg(_) => {
            let pixels = usize::try_from(u64::from(image.width) * u64::from(image.height))
                .map_err(|_| wrong())?;
            if paint.soft_mask.is_some()
                || paint.samples.len() != pixels * image.colour.components()
            {
                return Err(wrong());
            }
        }
        Stored::Samples { colour, alpha, .. } => {
            if paint.samples.as_ref() != colour.as_slice() {
                return Err(wrong());
            }
            match (alpha, &paint.soft_mask) {
                (None, None) => {}
                (Some(alpha), Some(mask)) if mask.samples.as_ref() == alpha.as_slice() => {}
                _ => return Err(wrong()),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    reason = "a placement of whole points arrives as the whole points it was"
)]
mod tests {
    use pdf_bytes::{ByteStore, SourceId};
    use pdf_paint::{Matrix, PaintAtomKind};

    use crate::image_file::tests::png;
    use crate::plan::Command;
    use crate::spike_move_text::{plan_command_with_fonts, read_page};

    fn document(leaves: &str) -> ByteStore {
        let content = format!("0 0 1 rg 0 0 10 10 re f {leaves}");
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Resources << >> /Contents 4 0 R >>"
                .to_owned(),
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ),
        ];
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
        }
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        ByteStore::new(SourceId::new(1), bytes)
    }

    fn placed(
        source: &ByteStore,
        frame: [f64; 4],
        file: &[u8],
    ) -> (ByteStore, pdf_paint::PaintGraph) {
        let plan = plan_command_with_fonts(
            source,
            &Command::PlaceNewImage {
                page_index: 0,
                placement: framed(frame),
                file: file.into(),
            },
            b"",
            None,
        )
        .expect("plans");
        let written = plan.commit(source, b"").expect("writes");
        let page = read_page(&written, 0, b"", None).expect("reads");
        (written, page.graph)
    }

    fn framed([x0, y0, x1, y1]: [f64; 4]) -> Matrix {
        Matrix {
            a: x1 - x0,
            b: 0.0,
            c: 0.0,
            d: y1 - y0,
            e: x0,
            f: y0,
        }
    }

    fn two_pixels() -> Vec<u8> {
        png((2, 1, 8, 6, 0), &[], &[0, 255, 0, 0, 255, 0, 255, 0, 128])
    }

    fn image_of(graph: &pdf_paint::PaintGraph) -> &pdf_paint::ImagePaint {
        match &graph.atoms.last().expect("an atom").kind {
            PaintAtomKind::Image(image) => image,
            other => panic!("the last atom is the picture, not {other:?}"),
        }
    }

    #[test]
    fn a_png_lands_on_its_frame_with_its_colour_and_alpha() {
        let source = document("");
        let (_, graph) = placed(&source, [50.0, 60.0, 150.0, 110.0], &two_pixels());
        assert_eq!(graph.atoms.len(), 2, "the square and the picture");
        let image = image_of(&graph);
        let m = image.state.ctm.value;
        assert_eq!(
            [m.a, m.b, m.c, m.d, m.e, m.f],
            [100.0, 0.0, 0.0, 50.0, 50.0, 60.0]
        );
        assert_eq!(image.samples.as_ref(), [255, 0, 0, 0, 255, 0]);
        assert_eq!(
            image.soft_mask.as_ref().expect("a mask").samples.as_ref(),
            [255, 128]
        );
    }

    #[test]
    fn a_page_that_leaves_a_transform_still_gets_the_frame_asked_for() {
        let source = document("2 0 0 2 30 40 cm");
        let (_, graph) = placed(&source, [10.0, 20.0, 110.0, 70.0], &two_pixels());
        let m = image_of(&graph).state.ctm.value;
        for (got, wanted) in [m.a, m.b, m.c, m.d, m.e, m.f]
            .iter()
            .zip([100.0, 0.0, 0.0, 50.0, 10.0, 20.0])
        {
            assert!((got - wanted).abs() < 1e-9, "{m:?}");
        }
    }

    #[test]
    fn an_opaque_picture_carries_no_mask_and_a_second_is_named_apart() {
        let opaque = png((1, 1, 8, 2, 0), &[], &[0, 1, 2, 3]);
        let source = document("");
        let (once, _) = placed(&source, [0.0, 0.0, 10.0, 10.0], &opaque);
        let (_, graph) = placed(&once, [20.0, 0.0, 30.0, 10.0], &opaque);
        assert_eq!(graph.atoms.len(), 3, "the square and two pictures");
        for atom in &graph.atoms[1..] {
            let PaintAtomKind::Image(image) = &atom.kind else {
                panic!("a picture");
            };
            assert!(image.soft_mask.is_none());
            assert_eq!(image.samples.as_ref(), [1, 2, 3]);
        }
        let es: Vec<f64> = graph.atoms[1..]
            .iter()
            .map(|atom| match &atom.kind {
                PaintAtomKind::Image(image) => image.state.ctm.value.e,
                _ => f64::NAN,
            })
            .collect();
        assert_eq!(es, [0.0, 20.0], "each is its own picture");
    }

    #[test]
    fn a_jpeg_goes_in_as_its_own_bytes_and_paints_what_it_decodes_to() {
        let source = document("");
        for (file, components, colour) in [
            (
                &include_bytes!("../tests/data/picture-rgb.jpg")[..],
                3,
                [200, 30, 40, 0],
            ),
            (
                &include_bytes!("../tests/data/picture-progressive.jpg")[..],
                3,
                [1, 2, 3, 0],
            ),
            (
                &include_bytes!("../tests/data/picture-cmyk.jpg")[..],
                4,
                [245, 55, 225, 255],
            ),
        ] {
            let (written, graph) = placed(&source, [0.0, 0.0, 40.0, 30.0], file);
            let image = image_of(&graph);
            assert_eq!((image.width.value, image.height.value), (4, 3));
            assert_eq!(image.samples.len(), 12 * components);
            for pixel in image.samples.chunks(components) {
                for (got, wanted) in pixel.iter().zip(colour) {
                    assert!(got.abs_diff(wanted) <= 3, "{pixel:?} for {colour:?}");
                }
            }
            let inverted = components == 4;
            assert_eq!(
                image.decode.value.first() == Some(&1.0),
                inverted,
                "{:?}",
                image.decode.value
            );
            let bytes = written.as_bytes();
            assert!(
                bytes.windows(file.len()).any(|window| window == file),
                "the file is in the document byte for byte"
            );
        }
    }

    #[test]
    fn a_photograph_stored_sideways_is_placed_the_way_up_it_says() {
        let source = document("");
        let file = include_bytes!("../tests/data/picture-turned.jpg");
        let (_, graph) = placed(&source, [0.0, 0.0, 30.0, 40.0], file);
        let m = image_of(&graph).state.ctm.value;
        assert_eq!(
            [m.a, m.b, m.c, m.d, m.e, m.f],
            [0.0, -40.0, 30.0, 0.0, 0.0, 40.0]
        );
    }

    #[test]
    fn what_cannot_be_placed_is_refused() {
        let source = document("");
        for (frame, file) in [
            ([0.0, 0.0, 0.0, 10.0], two_pixels()),
            ([0.0, 0.0, 10.0, 10.0], b"not a picture".to_vec()),
        ] {
            assert!(
                plan_command_with_fonts(
                    &source,
                    &Command::PlaceNewImage {
                        page_index: 0,
                        placement: framed(frame),
                        file: file.into(),
                    },
                    b"",
                    None,
                )
                .is_err()
            );
        }
    }
}
