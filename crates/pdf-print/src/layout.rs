#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Paper {
    pub width: f64,
    pub height: f64,
}

const POINTS_PER_MM: f64 = 72.0 / 25.4;

impl Paper {
    #[must_use]
    pub fn millimetres(width: f64, height: f64) -> Self {
        Self {
            width: width * POINTS_PER_MM,
            height: height * POINTS_PER_MM,
        }
    }

    #[must_use]
    pub fn inches(width: f64, height: f64) -> Self {
        Self {
            width: width * 72.0,
            height: height * 72.0,
        }
    }
}

#[must_use]
pub fn papers() -> Vec<(&'static str, Paper)> {
    PAPERS
        .iter()
        .map(|(name, paper, _)| (*name, paper()))
        .collect()
}

#[must_use]
pub fn media_name(paper: &str) -> Option<&'static str> {
    PAPERS
        .iter()
        .find(|(name, _, _)| *name == paper)
        .map(|(_, _, media)| *media)
}

type Known = (&'static str, fn() -> Paper, &'static str);

const PAPERS: [Known; 9] = [
    (
        "A4",
        || Paper::millimetres(210.0, 297.0),
        "iso_a4_210x297mm",
    ),
    (
        "A3",
        || Paper::millimetres(297.0, 420.0),
        "iso_a3_297x420mm",
    ),
    (
        "A5",
        || Paper::millimetres(148.0, 210.0),
        "iso_a5_148x210mm",
    ),
    (
        "B5",
        || Paper::millimetres(176.0, 250.0),
        "iso_b5_176x250mm",
    ),
    (
        "JIS B5",
        || Paper::millimetres(182.0, 257.0),
        "jis_b5_182x257mm",
    ),
    ("Letter", || Paper::inches(8.5, 11.0), "na_letter_8.5x11in"),
    ("Legal", || Paper::inches(8.5, 14.0), "na_legal_8.5x14in"),
    (
        "Folio (F4)",
        || Paper::inches(8.5, 13.0),
        "na_foolscap_8.5x13in",
    ),
    ("Tabloid", || Paper::inches(11.0, 17.0), "na_ledger_11x17in"),
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Orientation {
    #[default]
    Auto,
    Portrait,
    Landscape,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Scaling {
    #[default]
    Fit,
    ShrinkOversized,
    ActualSize,
    Custom(f64),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PerSheet {
    Pages(u8),
    Grid { columns: u8, rows: u8 },
}

impl Default for PerSheet {
    fn default() -> Self {
        Self::Pages(1)
    }
}

impl PerSheet {
    pub const COUNTS: [u8; 6] = [1, 2, 4, 6, 9, 16];

    #[must_use]
    pub const fn sides(self) -> Option<(u8, u8)> {
        match self {
            Self::Pages(1) => Some((1, 1)),
            Self::Pages(2) => Some((2, 1)),
            Self::Pages(4) => Some((2, 2)),
            Self::Pages(6) => Some((3, 2)),
            Self::Pages(9) => Some((3, 3)),
            Self::Pages(16) => Some((4, 4)),
            Self::Pages(_) => None,
            Self::Grid { columns, rows } => {
                if columns == 0 || rows == 0 {
                    None
                } else {
                    Some((columns, rows))
                }
            }
        }
    }

    #[must_use]
    pub fn cells(self) -> usize {
        self.sides()
            .map_or(0, |(one, other)| usize::from(one) * usize::from(other))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Order {
    #[default]
    Horizontal,
    HorizontalReversed,
    Vertical,
    VerticalReversed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub paper: Paper,
    pub orientation: Orientation,
    pub scaling: Scaling,
    pub per_sheet: PerSheet,
    pub order: Order,
    pub auto_rotate: bool,
    pub margin: f64,
    pub nudge: [f64; 2],
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            paper: Paper::millimetres(210.0, 297.0),
            orientation: Orientation::Auto,
            scaling: Scaling::Fit,
            per_sheet: PerSheet::Pages(1),
            order: Order::Horizontal,
            auto_rotate: true,
            margin: DEFAULT_MARGIN,
            nudge: [0.0, 0.0],
        }
    }
}

pub const DEFAULT_MARGIN: f64 = 5.0 * POINTS_PER_MM;

pub const GUTTER: f64 = 9.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub page: usize,
    pub size: [f64; 2],
    pub matrix: [f64; 6],
    pub clip: [f64; 4],
    pub scale: f64,
    pub turned: bool,
}

impl Placement {
    #[must_use]
    pub fn landing(&self) -> [f64; 4] {
        let [xx, xy, yx, yy, tx, ty] = self.matrix;
        let [width, height] = self.size;
        let corners = [(0.0, 0.0), (width, 0.0), (width, height), (0.0, height)]
            .map(|(x, y)| (xx * x + yx * y + tx, xy * x + yy * y + ty));
        let xs = corners.map(|corner| corner.0);
        let ys = corners.map(|corner| corner.1);
        [
            xs.iter().copied().fold(f64::INFINITY, f64::min),
            ys.iter().copied().fold(f64::INFINITY, f64::min),
            xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        ]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sheet {
    pub size: [f64; 2],
    pub placements: Vec<Placement>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutError {
    NoPages,
    BadGrid,
    BadSize,
    NoRoom,
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::NoPages => "no pages to print",
            Self::BadGrid => "the pages to a sheet are not a grid",
            Self::BadSize => "a size is not a positive number",
            Self::NoRoom => "the margins leave no room on the paper",
        })
    }
}

impl std::error::Error for LayoutError {}

fn positive(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

fn fitted(size: [f64; 2], room: [f64; 2], may_turn: bool) -> (f64, bool) {
    let upright = (room[0] / size[0]).min(room[1] / size[1]);
    let turned = (room[0] / size[1]).min(room[1] / size[0]);
    if may_turn && turned > upright * (1.0 + 1e-9) {
        (turned, true)
    } else {
        (upright, false)
    }
}

fn centred(page: usize, size: [f64; 2], (scale, turned): (f64, bool), cell: [f64; 4]) -> Placement {
    let [width, height] = if turned {
        [size[1] * scale, size[0] * scale]
    } else {
        [size[0] * scale, size[1] * scale]
    };
    let x = f64::midpoint(cell[0], cell[2]) - width / 2.0;
    let y = f64::midpoint(cell[1], cell[3]) - height / 2.0;
    let matrix = if turned {
        [0.0, scale, -scale, 0.0, x + width, y]
    } else {
        [scale, 0.0, 0.0, scale, x, y]
    };
    Placement {
        page,
        size,
        matrix,
        clip: cell,
        scale,
        turned,
    }
}

fn standing(paper: Paper, landscape: bool) -> [f64; 2] {
    let (short, long) = (paper.width.min(paper.height), paper.width.max(paper.height));
    if landscape {
        [long, short]
    } else {
        [short, long]
    }
}

pub fn lay_out(
    pages: &[(usize, [f64; 2])],
    settings: &Settings,
) -> Result<Vec<Sheet>, LayoutError> {
    if pages.is_empty() {
        return Err(LayoutError::NoPages);
    }
    let paper = settings.paper;
    let margin = settings.margin;
    let sizes_positive = positive(paper.width)
        && positive(paper.height)
        && margin.is_finite()
        && margin >= 0.0
        && pages
            .iter()
            .all(|(_, size)| positive(size[0]) && positive(size[1]));
    if !sizes_positive {
        return Err(LayoutError::BadSize);
    }
    if let Scaling::Custom(percent) = settings.scaling
        && !positive(percent)
    {
        return Err(LayoutError::BadSize);
    }
    let (long_side, short_side) = settings.per_sheet.sides().ok_or(LayoutError::BadGrid)?;
    let short = paper.width.min(paper.height);
    if short - 2.0 * margin <= 0.0 {
        return Err(LayoutError::NoRoom);
    }
    if (long_side, short_side) == (1, 1) {
        return Ok(pages
            .iter()
            .map(|(page, size)| single(*page, *size, settings))
            .collect());
    }
    Ok(grid(pages, settings, (long_side, short_side)))
}

fn single(page: usize, size: [f64; 2], settings: &Settings) -> Sheet {
    let landscape = match settings.orientation {
        Orientation::Auto => size[0] > size[1],
        Orientation::Portrait => false,
        Orientation::Landscape => true,
    };
    let sheet = standing(settings.paper, landscape);
    let margin = settings.margin;
    let room = [sheet[0] - 2.0 * margin, sheet[1] - 2.0 * margin];
    let (fit, turned) = fitted(size, room, settings.auto_rotate);
    let scale = match settings.scaling {
        Scaling::Fit => fit,
        Scaling::ShrinkOversized => fit.min(1.0),
        Scaling::ActualSize => 1.0,
        Scaling::Custom(percent) => percent / 100.0,
    };
    let mut placement = centred(
        page,
        size,
        (scale, turned),
        [margin, margin, sheet[0] - margin, sheet[1] - margin],
    );
    placement.matrix[4] += settings.nudge[0];
    placement.matrix[5] += settings.nudge[1];
    placement.clip = [0.0, 0.0, sheet[0], sheet[1]];
    Sheet {
        size: sheet,
        placements: vec![placement],
    }
}

type Arrangement = ((f64, bool), bool, (u8, u8));

fn grid(
    pages: &[(usize, [f64; 2])],
    settings: &Settings,
    (long_side, short_side): (u8, u8),
) -> Vec<Sheet> {
    let landscapes: &[bool] = match settings.orientation {
        Orientation::Auto => &[false, true],
        Orientation::Portrait => &[false],
        Orientation::Landscape => &[true],
    };
    let arrangements: Vec<(u8, u8)> = match settings.per_sheet {
        PerSheet::Grid { columns, rows } => vec![(columns, rows)],
        PerSheet::Pages(_) if long_side == short_side => vec![(long_side, short_side)],
        PerSheet::Pages(_) => vec![(long_side, short_side), (short_side, long_side)],
    };
    let first = pages[0].1;
    let mut best: Option<Arrangement> = None;
    for &landscape in landscapes {
        for &(columns, rows) in &arrangements {
            let sheet = standing(settings.paper, landscape);
            let cell = cell_size(sheet, settings.margin, (columns, rows));
            if !(positive(cell[0]) && positive(cell[1])) {
                continue;
            }
            let (scale, turned) = fitted(first, cell, settings.auto_rotate);
            let better = best.is_none_or(|((held, held_turned), _, _)| {
                scale > held * (1.0 + 1e-9)
                    || (scale >= held * (1.0 - 1e-9) && held_turned && !turned)
            });
            if better {
                best = Some(((scale, turned), landscape, (columns, rows)));
            }
        }
    }
    let (landscape, (columns, rows)) =
        best.map_or((false, (1, 1)), |(_, landscape, grid)| (landscape, grid));
    let sheet = standing(settings.paper, landscape);
    let size = cell_size(sheet, settings.margin, (columns, rows));
    let cells = usize::from(columns) * usize::from(rows);
    pages
        .chunks(cells)
        .map(|chunk| Sheet {
            size: sheet,
            placements: chunk
                .iter()
                .enumerate()
                .map(|(at, (page, page_size))| {
                    let (column, row) = cell_of(at, (columns, rows), settings.order);
                    let x0 = settings.margin + f64::from(column) * (size[0] + GUTTER);
                    let y1 = sheet[1] - settings.margin - f64::from(row) * (size[1] + GUTTER);
                    let cell = [x0, y1 - size[1], x0 + size[0], y1];
                    let fit = fitted(*page_size, size, settings.auto_rotate);
                    centred(*page, *page_size, fit, cell)
                })
                .collect(),
        })
        .collect()
}

fn cell_size(sheet: [f64; 2], margin: f64, (columns, rows): (u8, u8)) -> [f64; 2] {
    let (columns, rows) = (f64::from(columns), f64::from(rows));
    [
        (sheet[0] - 2.0 * margin - (columns - 1.0) * GUTTER) / columns,
        (sheet[1] - 2.0 * margin - (rows - 1.0) * GUTTER) / rows,
    ]
}

fn cell_of(at: usize, (columns, rows): (u8, u8), order: Order) -> (u8, u8) {
    let (columns_n, rows_n) = (usize::from(columns), usize::from(rows));
    let (column, row) = match order {
        Order::Horizontal => (at % columns_n, at / columns_n),
        Order::HorizontalReversed => (columns_n - 1 - at % columns_n, at / columns_n),
        Order::Vertical => (at / rows_n, at % rows_n),
        Order::VerticalReversed => (columns_n - 1 - at / rows_n, at % rows_n),
    };
    (
        u8::try_from(column).unwrap_or(u8::MAX),
        u8::try_from(row).unwrap_or(u8::MAX),
    )
}
