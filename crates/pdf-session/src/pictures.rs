use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::Command;
use pdf_edit::image_file::ImageFile;
use pdf_edit::spike_move_text::SpikeError;
use pdf_paint::Matrix;

use crate::Session;

pub const SCREEN_DPI: f64 = 96.0;

pub(crate) const PAGE_POINTS: (f64, f64) = (3.0, 14_400.0);

const BELIEVABLE_DPI: (f64, f64) = (1.0, 10_000.0);

fn refused(reason: &'static str) -> SpikeError {
    SpikeError::RetypeUnsupported(reason)
}

pub fn page_for(picture: &[u8]) -> Result<[f64; 2], SpikeError> {
    let file = ImageFile::read(picture).map_err(|error| refused(error.reason()))?;
    let (pixels_across, pixels_down) = file.upright();
    let believable = |dpi: f64| (BELIEVABLE_DPI.0..=BELIEVABLE_DPI.1).contains(&dpi);
    let (across, down) = match file.dpi() {
        Some((across, down)) if believable(across) && believable(down) => {
            if file.orientation >= 5 {
                (down, across)
            } else {
                (across, down)
            }
        }
        _ => (SCREEN_DPI, SCREEN_DPI),
    };
    let size = [
        f64::from(pixels_across) * 72.0 / across,
        f64::from(pixels_down) * 72.0 / down,
    ];
    if size
        .iter()
        .any(|side| !(PAGE_POINTS.0..=PAGE_POINTS.1).contains(side))
    {
        return Err(refused(
            "that picture would make a page larger or smaller than PDF allows",
        ));
    }
    Ok(size)
}

pub fn pictures_into_pdf(pictures: &[Arc<[u8]>]) -> Result<Vec<u8>, SpikeError> {
    if pictures.is_empty() {
        return Err(refused("no picture was named to make pages of"));
    }
    let mut commands = Vec::with_capacity(pictures.len() * 2 + 1);
    for (at, picture) in pictures.iter().enumerate() {
        let [width, height] = page_for(picture)?;
        commands.push(Command::AddBlankPage {
            beside: at,
            before: false,
            size: [width, height],
        });
        commands.push(Command::PlaceNewImage {
            page_index: at + 1,
            placement: Matrix {
                a: width,
                b: 0.0,
                c: 0.0,
                d: height,
                e: 0.0,
                f: 0.0,
            },
            file: Arc::clone(picture),
        });
    }
    commands.push(Command::RemovePages { pages: vec![0] });
    let starter = ByteStore::new(SourceId::new(0), crate::extract::empty_document());
    let mut session = Session::new(starter, b"");
    session.apply_each(&commands)?;
    Ok(session.source().to_vec())
}

#[cfg(test)]
#[expect(
    clippy::float_cmp,
    reason = "a page size written as a number is read back as the same number"
)]
mod tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};

    use super::{page_for, pictures_into_pdf};
    use crate::interpret_page;

    fn picture(width: u32, height: u32, dpi: Option<f64>) -> Arc<[u8]> {
        let mut pixels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let across = u8::try_from(x % 256).unwrap_or(0);
                let down = u8::try_from(y % 256).unwrap_or(0);
                pixels.extend_from_slice(&[across, down, 255 - across]);
            }
        }
        let density = dpi.map(|dpi| {
            let metre = pdf_edit::png::per_metre(dpi);
            (metre, metre)
        });
        Arc::from(pdf_edit::png::write((width, height), &pixels, density).expect("written"))
    }

    fn store(bytes: &[u8]) -> ByteStore {
        let held: Arc<[u8]> = Arc::from(bytes.to_vec());
        ByteStore::new(SourceId::new(7), held)
    }

    fn sizes(bytes: &[u8]) -> Vec<[f64; 4]> {
        pdf_content::page_geometries_with_password(
            &store(bytes),
            pdf_content::PageContentLimits::default(),
            b"",
        )
        .expect("the pages lay out")
        .iter()
        .map(|page| page.media_box)
        .collect()
    }

    #[test]
    fn a_picture_makes_a_page_of_the_size_it_says_it_is() {
        assert_eq!(
            page_for(&picture(192, 96, None)).expect("read"),
            [144.0, 72.0]
        );
        let [width, height] = page_for(&picture(600, 300, Some(300.0))).expect("read");
        assert!((width - 144.0).abs() < 0.05 && (height - 72.0).abs() < 0.05);
        let mad =
            pdf_edit::png::write((192, 96), &vec![0; 192 * 96 * 3], Some((1, 1))).expect("written");
        assert_eq!(page_for(&mad).expect("read"), [144.0, 72.0]);
    }

    #[test]
    fn a_sideways_photograph_is_a_page_the_way_it_is_seen() {
        let jpeg = |orientation: u8| {
            let mut out = vec![0xFF_u8, 0xD8];
            let mut jfif = b"JFIF\0".to_vec();
            jfif.extend_from_slice(&[1, 2, 1]);
            jfif.extend_from_slice(&300_u16.to_be_bytes());
            jfif.extend_from_slice(&150_u16.to_be_bytes());
            jfif.extend_from_slice(&[0, 0]);
            out.extend_from_slice(&[0xFF, 0xE0]);
            out.extend_from_slice(&u16::try_from(jfif.len() + 2).expect("length").to_be_bytes());
            out.extend_from_slice(&jfif);
            let mut exif = b"Exif\0\0II*\0".to_vec();
            exif.extend_from_slice(&8_u32.to_le_bytes());
            exif.extend_from_slice(&1_u16.to_le_bytes());
            exif.extend_from_slice(&0x0112_u16.to_le_bytes());
            exif.extend_from_slice(&3_u16.to_le_bytes());
            exif.extend_from_slice(&1_u32.to_le_bytes());
            exif.extend_from_slice(&[orientation, 0, 0, 0]);
            out.extend_from_slice(&[0xFF, 0xE1]);
            out.extend_from_slice(&u16::try_from(exif.len() + 2).expect("length").to_be_bytes());
            out.extend_from_slice(&exif);
            out.extend_from_slice(&[0xFF, 0xC0, 0, 17, 8]);
            out.extend_from_slice(&300_u16.to_be_bytes());
            out.extend_from_slice(&600_u16.to_be_bytes());
            out.push(3);
            for component in 1..=3_u8 {
                out.extend_from_slice(&[component, 0x11, 0]);
            }
            out.extend_from_slice(&[0xFF, 0xD9]);
            out
        };
        for orientation in [1, 6, 8] {
            let [width, height] = page_for(&jpeg(orientation)).expect("read");
            assert!(
                (width - 144.0).abs() < 0.05 && (height - 144.0).abs() < 0.05,
                "orientation {orientation}: {width} by {height}"
            );
        }
    }

    #[test]
    fn a_page_no_pdf_could_hold_is_refused() {
        let error = page_for(&picture(3, 3, None)).expect_err("refused");
        assert!(error.to_string().contains("larger or smaller"), "{error}");
        assert!(pictures_into_pdf(&[picture(3, 3, None)]).is_err());
    }

    #[test]
    fn what_is_not_a_picture_is_refused() {
        assert!(page_for(b"GIF89a").is_err());
        assert!(pictures_into_pdf(&[Arc::from(b"%PDF-1.7".to_vec())]).is_err());
        let nothing = pictures_into_pdf(&[]).expect_err("refused");
        assert!(
            nothing.to_string().contains("no picture was named"),
            "{nothing}"
        );
    }

    #[test]
    fn every_picture_becomes_a_page_of_its_own_in_order() {
        let pictures = [
            picture(192, 96, None),
            picture(96, 192, None),
            picture(600, 300, Some(300.0)),
        ];
        let made = pictures_into_pdf(&pictures).expect("made");
        let sizes = sizes(&made);
        assert_eq!(sizes.len(), 3);
        assert_eq!(sizes[0], [0.0, 0.0, 144.0, 72.0]);
        assert_eq!(sizes[1], [0.0, 0.0, 72.0, 144.0]);
        assert!((sizes[2][2] - 144.0).abs() < 0.05);
        for page in 0..3 {
            let atoms = interpret_page(&store(&made), page)
                .expect("the page reads")
                .graph
                .atoms
                .len();
            assert_eq!(atoms, 1, "a page of one picture");
        }
    }

    #[test]
    fn the_page_paints_the_picture_it_was_given() {
        let made = pictures_into_pdf(&[picture(8, 4, None)]).expect("made");
        let view = interpret_page(&store(&made), 0).expect("the page reads");
        let atom = view.graph.atoms.first().expect("one atom");
        let pdf_paint::PaintAtomKind::Image(image) = &atom.kind else {
            panic!("a picture is what a picture page paints");
        };
        assert_eq!((image.width.value, image.height.value), (8, 4));
        let ctm = image.state.ctm.value;
        assert_eq!((ctm.a, ctm.d), (6.0, 3.0));
        assert_eq!((ctm.e, ctm.f), (0.0, 0.0));
        assert_eq!((ctm.b, ctm.c), (0.0, 0.0));
    }
}
