use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};

use crate::job::SheetsPdf;
use crate::sheet::SheetImage;

fn image(width: u32, height: u32, colour: impl Fn(u32, u32) -> [u8; 3]) -> SheetImage {
    let mut rgb = Vec::new();
    for y in 0..height {
        for x in 0..width {
            rgb.extend_from_slice(&colour(x, y));
        }
    }
    SheetImage { width, height, rgb }
}

#[test]
fn the_sheets_come_back_as_they_went_in() {
    let directory = std::env::temp_dir().join(format!("panpdf-job-test-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("a directory");
    let path = directory.join("sheets.pdf");
    let grey = image(
        30,
        20,
        |x, _| if x < 15 { [0, 0, 0] } else { [200, 200, 200] },
    );
    let colour = image(20, 30, |_, y| match y {
        y if y < 10 => [255, 0, 0],
        y if y < 12 => [128, 128, 128],
        _ => [0, 0, 255],
    });
    let mut pdf = SheetsPdf::create(&path).expect("created");
    pdf.add([30.0, 20.0], &grey).expect("added");
    pdf.add([20.0, 30.0], &colour).expect("added");
    pdf.finish().expect("finished");
    let bytes = std::fs::read(&path).expect("written");
    let _ = std::fs::remove_dir_all(&directory);
    let source = ByteStore::new(SourceId::new(3), Arc::<[u8]>::from(bytes));
    let geometries = pdf_content::page_geometries_with_password(
        &source,
        pdf_content::PageContentLimits::default(),
        b"",
    )
    .expect("the job's PDF reads strictly");
    assert_eq!(geometries.len(), 2);
    assert_eq!(geometries[0].rotated_size(), (30.0, 20.0));
    assert_eq!(geometries[1].rotated_size(), (20.0, 30.0));
    for (page, picture) in [(0, &grey), (1, &colour)] {
        let drawn = crate::draw_printed(
            &source,
            (b"", None),
            page,
            1.0,
            [0, 0, picture.width, picture.height],
        )
        .expect("draws");
        for y in 0..picture.height {
            for x in 0..picture.width {
                let [red, green, blue] = drawn.pixel(x, y);
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let got = [red, green, blue].map(|part| (part * 255.0).round() as u8);
                assert_eq!(got, picture.pixel(x, y), "page {page} at {x},{y}");
            }
        }
    }
}
