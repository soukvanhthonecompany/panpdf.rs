use pdf_render::Canvas;

use crate::layout::{Orientation, PerSheet, Placement, Scaling, Settings, Sheet, lay_out};
use crate::sheet::{SheetError, draw_sheet};

const RED: [f32; 3] = [1.0, 0.0, 0.0];
const BLUE: [f32; 3] = [0.0, 0.0, 1.0];
const GREEN: [f32; 3] = [0.0, 1.0, 0.0];

fn halves(size: [f64; 2]) -> impl FnMut(usize, f64, [u32; 4]) -> Result<Canvas, String> {
    move |_, scale, [x0, y0, x1, y1]| {
        let across = (size[0] * scale).ceil();
        let down = (size[1] * scale).ceil();
        let mut canvas = Canvas::window(x0, y0, x1 - x0, y1 - y0);
        for y in y0..y1 {
            for x in x0..x1 {
                let colour = if f64::from(y) >= down - 1.0 {
                    GREEN
                } else if f64::from(x) < across / 2.0 {
                    RED
                } else {
                    BLUE
                };
                let at = ((y - y0) * (x1 - x0) + (x - x0)) as usize;
                canvas.pixels[at] = colour;
            }
        }
        Ok(canvas)
    }
}

fn bare() -> Settings {
    Settings {
        margin: 0.0,
        paper: crate::layout::Paper {
            width: 40.0,
            height: 60.0,
        },
        ..Settings::default()
    }
}

#[test]
fn an_upright_page_lands_pixel_for_pixel() {
    let sheet = lay_out(&[(0, [40.0, 60.0])], &bare())
        .expect("lays out")
        .remove(0);
    let image = draw_sheet(&sheet, 72.0, false, halves([40.0, 60.0])).expect("draws");
    assert_eq!((image.width, image.height), (40, 60));
    assert_eq!(image.pixel(0, 0), [255, 0, 0]);
    assert_eq!(image.pixel(19, 30), [255, 0, 0]);
    assert_eq!(image.pixel(20, 30), [0, 0, 255]);
    assert_eq!(image.pixel(39, 0), [0, 0, 255]);
    assert_eq!(image.pixel(5, 59), [0, 255, 0]);
}

#[test]
fn a_turned_page_lands_turned_anticlockwise() {
    let sheet = lay_out(
        &[(0, [60.0, 40.0])],
        &Settings {
            orientation: Orientation::Portrait,
            ..bare()
        },
    )
    .expect("lays out")
    .remove(0);
    assert!(sheet.placements[0].turned);
    let image = draw_sheet(&sheet, 72.0, false, halves([60.0, 40.0])).expect("draws");
    assert_eq!((image.width, image.height), (40, 60));
    assert_eq!(image.pixel(39, 10), [0, 255, 0], "the foot, on the right");
    assert_eq!(image.pixel(10, 45), [255, 0, 0], "the left half, below");
    assert_eq!(image.pixel(10, 15), [0, 0, 255], "the right half, above");
    assert_eq!(image.pixel(38, 10), [0, 0, 255]);
}

#[test]
fn only_what_the_paper_shows_is_drawn() {
    let sheet = lay_out(
        &[(0, [80.0, 100.0])],
        &Settings {
            scaling: Scaling::ActualSize,
            ..bare()
        },
    )
    .expect("lays out")
    .remove(0);
    let mut asked = Vec::new();
    let mut inner = halves([80.0, 100.0]);
    let image = draw_sheet(&sheet, 72.0, false, |page, scale, region| {
        asked.push(region);
        inner(page, scale, region)
    })
    .expect("draws");
    assert_eq!(asked, vec![[20, 20, 60, 80]]);
    assert_eq!(image.pixel(19, 0), [255, 0, 0]);
    assert_eq!(image.pixel(20, 59), [0, 0, 255]);
}

#[test]
fn pages_stay_in_their_cells() {
    let settings = Settings {
        paper: crate::layout::Paper {
            width: 100.0,
            height: 200.0,
        },
        per_sheet: PerSheet::Pages(2),
        ..bare()
    };
    let sheet = lay_out(&[(0, [100.0, 200.0]), (1, [100.0, 200.0])], &settings)
        .expect("lays out")
        .remove(0);
    assert!((sheet.size[0] - 200.0).abs() < 1e-9 && (sheet.size[1] - 100.0).abs() < 1e-9);
    let image = draw_sheet(&sheet, 72.0, false, halves([100.0, 200.0])).expect("draws");
    let gutter = crate::layout::GUTTER;
    let cell = (200.0 - gutter) / 2.0;
    let [first, second] = [sheet.placements[0].landing(), sheet.placements[1].landing()];
    assert!(first[2] <= cell + 1e-9 && second[0] >= cell + gutter - 1e-9);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let middle = (cell + gutter / 2.0) as u32;
    assert_eq!(image.pixel(middle, 50), [255, 255, 255]);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let inside = (first[0] + 1.0) as u32;
    assert_eq!(image.pixel(inside, 50), [255, 0, 0]);
}

#[test]
fn a_border_is_drawn_round_the_page() {
    let sheet = Sheet {
        size: [40.0, 60.0],
        placements: vec![Placement {
            page: 0,
            size: [20.0, 20.0],
            matrix: [1.0, 0.0, 0.0, 1.0, 10.0, 20.0],
            clip: [0.0, 0.0, 40.0, 60.0],
            scale: 1.0,
            turned: false,
        }],
    };
    let white =
        |_: usize, _: f64, [x0, y0, x1, y1]: [u32; 4]| Ok(Canvas::window(x0, y0, x1 - x0, y1 - y0));
    let image = draw_sheet(&sheet, 144.0, true, white).expect("draws");
    assert_eq!(image.pixel(20, 50), [0, 0, 0]);
    assert_eq!(image.pixel(59, 50), [0, 0, 0]);
    assert_eq!(image.pixel(30, 40), [0, 0, 0]);
    assert_eq!(image.pixel(30, 79), [0, 0, 0]);
    assert_eq!(image.pixel(30, 50), [255, 255, 255]);
    assert_eq!(image.pixel(19, 50), [255, 255, 255]);
}

#[test]
fn a_page_that_cannot_be_drawn_stops_the_sheet() {
    let sheet = lay_out(&[(3, [40.0, 60.0])], &bare())
        .expect("lays out")
        .remove(0);
    let failing = |_: usize, _: f64, _: [u32; 4]| Err("broken".to_owned());
    assert_eq!(
        draw_sheet(&sheet, 72.0, false, failing),
        Err(SheetError::Page {
            page: 3,
            why: "broken".to_owned()
        })
    );
    assert_eq!(
        draw_sheet(&sheet, 0.0, false, halves([40.0, 60.0])),
        Err(SheetError::BadResolution)
    );
}
