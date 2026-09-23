use crate::layout::{
    DEFAULT_MARGIN, GUTTER, LayoutError, Order, Orientation, Paper, PerSheet, Scaling, Settings,
    Sheet, lay_out,
};

const A4: [f64; 2] = [595.275_590_551_181_1, 841.889_763_779_527_6];
const A4_WIDE: [f64; 2] = [A4[1], A4[0]];
const A3: [f64; 2] = [A4[1], 1_190.551_181_102_362_2];

fn near(one: f64, other: f64) -> bool {
    (one - other).abs() < 1e-6
}

fn near_all(one: &[f64], other: &[f64]) -> bool {
    one.len() == other.len() && one.iter().zip(other).all(|(a, b)| near(*a, *b))
}

fn bare() -> Settings {
    Settings {
        margin: 0.0,
        ..Settings::default()
    }
}

fn one(size: [f64; 2], settings: &Settings) -> Sheet {
    lay_out(&[(0, size)], settings).expect("lays out").remove(0)
}

#[test]
fn a_page_the_size_of_the_paper_is_printed_as_it_is() {
    let sheet = one(A4, &bare());
    assert!(near_all(&sheet.size, &A4));
    let placed = sheet.placements[0];
    assert!(near_all(&placed.matrix, &[1.0, 0.0, 0.0, 1.0, 0.0, 0.0]));
    assert!(near_all(&placed.clip, &[0.0, 0.0, A4[0], A4[1]]));
    assert!(!placed.turned);
}

#[test]
fn fit_shrinks_a_page_into_the_margins_and_centres_it() {
    let sheet = one(A4, &Settings::default());
    let placed = sheet.placements[0];
    let scale = 200.0 / 210.0;
    assert!(near(placed.scale, scale));
    let y = (A4[1] - A4[1] * scale) / 2.0;
    assert!(near_all(
        &placed.matrix,
        &[scale, 0.0, 0.0, scale, DEFAULT_MARGIN, y]
    ));
    assert!(
        near_all(&placed.clip, &[0.0, 0.0, A4[0], A4[1]]),
        "cut only by the paper"
    );
}

#[test]
fn each_scaling_sizes_a_small_page_as_its_name_says() {
    let a5 = [A4[1] / 2.0, A4[0]];
    let scaled = |scaling| one(a5, &Settings { scaling, ..bare() }).placements[0].scale;
    assert!(near(scaled(Scaling::Fit), A4[0] / a5[0]));
    assert!(near(scaled(Scaling::ShrinkOversized), 1.0));
    assert!(near(scaled(Scaling::ActualSize), 1.0));
    assert!(near(scaled(Scaling::Custom(50.0)), 0.5));
}

#[test]
fn a_large_page_is_shrunk_or_cut_by_the_paper() {
    for scaling in [Scaling::Fit, Scaling::ShrinkOversized] {
        let placed = one(A3, &Settings { scaling, ..bare() }).placements[0];
        assert!(near(placed.scale, A4[0] / A3[0]), "{scaling:?}");
    }
    let placed = one(
        A3,
        &Settings {
            scaling: Scaling::ActualSize,
            ..bare()
        },
    )
    .placements[0];
    let left = (A4[0] - A3[0]) / 2.0;
    let bottom = (A4[1] - A3[1]) / 2.0;
    assert!(near_all(
        &placed.matrix,
        &[1.0, 0.0, 0.0, 1.0, left, bottom]
    ));
    assert!(near_all(
        &placed.landing(),
        &[left, bottom, left + A3[0], bottom + A3[1]]
    ));
    assert!(near_all(&placed.clip, &[0.0, 0.0, A4[0], A4[1]]));
}

#[test]
fn a_landscape_page_gets_a_landscape_sheet() {
    let sheet = one(A4_WIDE, &bare());
    assert!(near_all(&sheet.size, &A4_WIDE));
    assert!(!sheet.placements[0].turned);
    assert!(near(sheet.placements[0].scale, 1.0));
}

#[test]
fn a_landscape_page_on_a_portrait_sheet_is_turned_top_to_the_left() {
    let portrait = Settings {
        orientation: Orientation::Portrait,
        ..bare()
    };
    let sheet = one(A4_WIDE, &portrait);
    let placed = sheet.placements[0];
    assert!(placed.turned);
    assert!(near(placed.scale, 1.0));
    assert!(near_all(&placed.matrix, &[0.0, 1.0, -1.0, 0.0, A4[0], 0.0]));
    let [xx, xy, yx, yy, tx, ty] = placed.matrix;
    let at = |x: f64, y: f64| [xx * x + yx * y + tx, xy * x + yy * y + ty];
    assert!(near_all(&at(0.0, A4_WIDE[1]), &[0.0, 0.0]));
    assert!(near_all(&at(A4_WIDE[0], A4_WIDE[1]), &[0.0, A4[1]]));
    assert!(near_all(&placed.landing(), &[0.0, 0.0, A4[0], A4[1]]));

    let upright = one(
        A4_WIDE,
        &Settings {
            auto_rotate: false,
            ..portrait
        },
    )
    .placements[0];
    assert!(!upright.turned);
    assert!(near(upright.scale, A4[0] / A4[1]));
}

#[test]
fn a_square_page_stays_upright() {
    let placed = one(
        [500.0, 500.0],
        &Settings {
            orientation: Orientation::Landscape,
            ..bare()
        },
    )
    .placements[0];
    assert!(!placed.turned);
}

#[test]
fn two_to_a_sheet_stand_side_by_side_on_a_landscape_sheet() {
    let sheets = lay_out(
        &[(0, A4), (1, A4), (2, A4)],
        &Settings {
            per_sheet: PerSheet::Pages(2),
            ..bare()
        },
    )
    .expect("lays out");
    assert_eq!(sheets.len(), 2);
    assert!(near_all(&sheets[0].size, &A4_WIDE));
    let cell_width = (A4_WIDE[0] - GUTTER) / 2.0;
    let scale = cell_width / A4[0];
    let [left, right] = [sheets[0].placements[0], sheets[0].placements[1]];
    assert_eq!((left.page, right.page), (0, 1));
    assert!(near(left.scale, scale) && !left.turned);
    assert!(near_all(&left.clip, &[0.0, 0.0, cell_width, A4_WIDE[1]]));
    assert!(near_all(
        &right.clip,
        &[cell_width + GUTTER, 0.0, A4_WIDE[0], A4_WIDE[1]]
    ));
    let height = A4[1] * scale;
    let bottom = (A4_WIDE[1] - height) / 2.0;
    assert!(near_all(
        &left.landing(),
        &[0.0, bottom, cell_width, bottom + height]
    ));
    assert_eq!(sheets[1].placements.len(), 1, "the odd page alone");
    assert_eq!(sheets[1].placements[0].page, 2);
}

#[test]
fn the_four_orders_fill_the_cells_as_named() {
    let pages: Vec<(usize, [f64; 2])> = (0..4).map(|page| (page, A4)).collect();
    let cells = |order| -> Vec<usize> {
        let sheet = lay_out(
            &pages,
            &Settings {
                per_sheet: PerSheet::Pages(4),
                order,
                ..bare()
            },
        )
        .expect("lays out")
        .remove(0);
        let mut by_cell = vec![usize::MAX; 4];
        for placed in &sheet.placements {
            let column = usize::from(placed.clip[0] > 1.0);
            let row = usize::from(placed.clip[3] < sheet.size[1] - 1.0);
            by_cell[row * 2 + column] = placed.page;
        }
        by_cell
    };
    assert_eq!(cells(Order::Horizontal), vec![0, 1, 2, 3]);
    assert_eq!(cells(Order::HorizontalReversed), vec![1, 0, 3, 2]);
    assert_eq!(cells(Order::Vertical), vec![0, 2, 1, 3]);
    assert_eq!(cells(Order::VerticalReversed), vec![2, 0, 3, 1]);
}

#[test]
fn four_to_a_sheet_keep_a_portrait_sheet() {
    let pages: Vec<(usize, [f64; 2])> = (0..4).map(|page| (page, A4)).collect();
    let sheet = lay_out(
        &pages,
        &Settings {
            per_sheet: PerSheet::Pages(4),
            ..bare()
        },
    )
    .expect("lays out")
    .remove(0);
    assert!(near_all(&sheet.size, &A4));
    assert!(sheet.placements.iter().all(|placed| !placed.turned));
}

#[test]
fn six_to_a_sheet_take_the_grid_that_prints_them_largest() {
    let pages: Vec<(usize, [f64; 2])> = (0..6).map(|page| (page, A4)).collect();
    let sheet = lay_out(
        &pages,
        &Settings {
            per_sheet: PerSheet::Pages(6),
            ..bare()
        },
    )
    .expect("lays out")
    .remove(0);
    assert!(near_all(&sheet.size, &A4_WIDE));
    let wide = [
        (A4_WIDE[0] - 2.0 * GUTTER) / 3.0,
        (A4_WIDE[1] - GUTTER) / 2.0,
    ];
    let scale = (wide[0] / A4[0]).min(wide[1] / A4[1]);
    let tall = [(A4[0] - GUTTER) / 2.0, (A4[1] - 2.0 * GUTTER) / 3.0];
    assert!(scale > (tall[0] / A4[0]).min(tall[1] / A4[1]));
    assert!(
        sheet
            .placements
            .iter()
            .all(|placed| near(placed.scale, scale))
    );
    let mut columns: Vec<f64> = sheet
        .placements
        .iter()
        .map(|placed| placed.clip[0])
        .collect();
    columns.sort_by(f64::total_cmp);
    columns.dedup_by(|one, other| near(*one, *other));
    assert_eq!(columns.len(), 3);
}

#[test]
fn a_grid_keeps_its_columns_and_rows() {
    let pages: Vec<(usize, [f64; 2])> = (0..3).map(|page| (page, A4)).collect();
    let sheet = lay_out(
        &pages,
        &Settings {
            per_sheet: PerSheet::Grid {
                columns: 3,
                rows: 1,
            },
            orientation: Orientation::Portrait,
            auto_rotate: false,
            ..bare()
        },
    )
    .expect("lays out")
    .remove(0);
    assert!(near_all(&sheet.size, &A4));
    let width = (A4[0] - 2.0 * GUTTER) / 3.0;
    for (at, placed) in sheet.placements.iter().enumerate() {
        let x0 = f64::from(u8::try_from(at).expect("small")) * (width + GUTTER);
        assert!(near(placed.clip[0], x0));
        assert!(near(placed.scale, width / A4[0]));
    }
}

#[test]
fn what_cannot_be_laid_out_says_why() {
    assert_eq!(lay_out(&[], &bare()), Err(LayoutError::NoPages));
    let page = [(0, A4)];
    let with = |settings: Settings| lay_out(&page, &settings);
    assert_eq!(
        with(Settings {
            per_sheet: PerSheet::Pages(3),
            ..bare()
        }),
        Err(LayoutError::BadGrid)
    );
    assert_eq!(
        with(Settings {
            per_sheet: PerSheet::Grid {
                columns: 0,
                rows: 2
            },
            ..bare()
        }),
        Err(LayoutError::BadGrid)
    );
    assert_eq!(
        with(Settings {
            margin: 300.0,
            ..bare()
        }),
        Err(LayoutError::NoRoom)
    );
    assert_eq!(
        with(Settings {
            scaling: Scaling::Custom(0.0),
            ..bare()
        }),
        Err(LayoutError::BadSize)
    );
    assert_eq!(
        lay_out(&[(0, [0.0, 10.0])], &bare()),
        Err(LayoutError::BadSize)
    );
    assert_eq!(
        with(Settings {
            paper: Paper {
                width: f64::NAN,
                height: 10.0
            },
            ..bare()
        }),
        Err(LayoutError::BadSize)
    );
}

#[test]
fn each_papers_printer_name_says_its_size() {
    for (name, paper) in crate::layout::papers() {
        let media = crate::layout::media_name(name).unwrap_or_else(|| panic!("{name} has a name"));
        let (size, unit) = media
            .rsplit_once('_')
            .map(|(_, size)| size.split_at(size.len() - 2))
            .expect("a PWG name ends in its size and unit");
        let (across, down) = size.split_once('x').expect("width by height");
        let (across, down): (f64, f64) = (
            across.parse().expect("a number"),
            down.parse().expect("a number"),
        );
        let said = match unit {
            "mm" => Paper::millimetres(across, down),
            "in" => Paper::inches(across, down),
            other => panic!("{name} is measured in {other}"),
        };
        assert!(
            near(said.width, paper.width) && near(said.height, paper.height),
            "{name} is {} x {} points, and {media} says {} x {}",
            paper.width,
            paper.height,
            said.width,
            said.height
        );
    }
    assert_eq!(crate::layout::media_name("Papyrus"), None);
}

#[test]
fn a_nudge_moves_the_page_by_what_it_says_and_nothing_else() {
    let still = one(A4, &bare()).placements[0];
    let moved = one(
        A4,
        &Settings {
            nudge: [30.0, -12.0],
            ..bare()
        },
    )
    .placements[0];
    assert!(near_all(&moved.matrix, &[1.0, 0.0, 0.0, 1.0, 30.0, -12.0]));
    assert!(near(moved.scale, still.scale), "the size did not change");
    assert_eq!(moved.turned, still.turned, "the turn did not change");
    assert!(
        near_all(&moved.clip, &still.clip),
        "a moved page is still cut by the paper, not by where it was moved to"
    );
    assert!(
        near_all(&still.matrix, &[1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
        "the unnudged page is where it has always been"
    );
}

#[test]
fn a_custom_scale_and_a_nudge_do_not_interfere() {
    let placed = one(
        A4,
        &Settings {
            scaling: Scaling::Custom(50.0),
            nudge: [10.0, 0.0],
            ..bare()
        },
    )
    .placements[0];
    assert!(near(placed.scale, 0.5));
    assert!(near(placed.matrix[4], A4[0] / 4.0 + 10.0));
    assert!(near(placed.matrix[5], A4[1] / 4.0));
}

#[test]
fn a_grid_is_not_moved_by_a_nudge() {
    let settings = Settings {
        per_sheet: PerSheet::Pages(4),
        ..bare()
    };
    let still = one(A4, &settings);
    let nudged = one(
        A4,
        &Settings {
            nudge: [25.0, 25.0],
            ..settings
        },
    );
    assert_eq!(still, nudged, "a grid places its cells by the grid alone");
}
