use std::collections::BTreeMap;

use eframe::egui;

pub(crate) const MIN_CELL: f32 = 128.0;

const MAX_THUMB: f32 = 240.0;

const CELL_ROOM: f32 = 12.0;

pub(crate) const COLUMN_GAP: f32 = 14.0;

pub(crate) const LABEL: f32 = 18.0;

pub(crate) const ROW_GAP: f32 = 22.0;

const GLIDE: f64 = 0.2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Grid {
    pub(crate) columns: usize,
    pub(crate) cell: f32,
    pub(crate) thumb: f32,
}

impl Grid {
    pub(crate) fn for_width(width: f32) -> Self {
        let width = width.max(MIN_CELL);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a handful of columns"
        )]
        let columns = (((width + COLUMN_GAP) / (MIN_CELL + COLUMN_GAP)).floor() as usize).max(1);
        let cell = (width - COLUMN_GAP * float(columns - 1)) / float(columns);
        Self {
            columns,
            cell,
            thumb: (cell - CELL_ROOM).min(MAX_THUMB),
        }
    }

    pub(crate) fn width(&self) -> f32 {
        self.cell * float(self.columns) + COLUMN_GAP * float(self.columns - 1)
    }
}

fn float(count: usize) -> f32 {
    #[expect(clippy::cast_precision_loss, reason = "a few columns or pages")]
    let float = count as f32;
    float
}

pub(crate) fn lay_out(grid: &Grid, heights: &[f32]) -> (Vec<egui::Rect>, f32) {
    let mut rects = Vec::with_capacity(heights.len());
    let mut top = 0.0;
    for row in heights.chunks(grid.columns) {
        let tallest = row.iter().copied().fold(0.0_f32, f32::max);
        for (column, height) in row.iter().enumerate() {
            let left = float(column) * (grid.cell + COLUMN_GAP) + (grid.cell - grid.thumb) / 2.0;
            rects.push(egui::Rect::from_min_size(
                egui::pos2(left, top),
                egui::vec2(grid.thumb, *height),
            ));
        }
        top += tallest + LABEL + ROW_GAP;
    }
    (rects, (top - ROW_GAP).max(0.0))
}

pub(crate) fn gap_at(columns: usize, pictures: &[egui::Rect], pointer: egui::Pos2) -> usize {
    pictures
        .iter()
        .filter(|picture| {
            if columns <= 1 {
                pointer.y > picture.center().y
            } else {
                pointer.y > picture.bottom() + LABEL
                    || (pointer.y >= picture.top() - ROW_GAP / 2.0
                        && pointer.x > picture.center().x)
            }
        })
        .count()
}

pub(crate) fn seams(columns: usize, pictures: &[egui::Rect]) -> Vec<egui::Pos2> {
    let count = pictures.len();
    (0..=count)
        .map(|at| {
            if columns <= 1 {
                let x = pictures.first().map_or(0.0, |first| first.center().x);
                let y = match (
                    at.checked_sub(1).map(|before| pictures[before]),
                    pictures.get(at),
                ) {
                    (Some(above), Some(below)) => {
                        f32::midpoint(above.bottom() + LABEL, below.top())
                    }
                    (Some(above), None) => above.bottom() + LABEL + ROW_GAP / 2.0,
                    (None, Some(below)) => below.top() - ROW_GAP / 2.0,
                    (None, None) => 0.0,
                };
                return egui::pos2(x, y);
            }
            match (
                at.checked_sub(1).map(|before| pictures[before]),
                pictures.get(at),
            ) {
                (Some(left), Some(right)) if (left.top() - right.top()).abs() < 0.5 => {
                    egui::pos2(f32::midpoint(left.right(), right.left()), right.center().y)
                }
                (_, Some(right)) => egui::pos2(right.left() - COLUMN_GAP / 2.0, right.center().y),
                (Some(left), None) => egui::pos2(left.right() + COLUMN_GAP / 2.0, left.center().y),
                (None, None) => egui::pos2(0.0, 0.0),
            }
        })
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Renumber {
    pub(crate) was: Vec<Option<usize>>,
    pub(crate) turned: Vec<usize>,
    pub(crate) quarter_turns: i32,
}

impl Renumber {
    pub(crate) fn moved(count: usize, pages: &[usize], to: usize) -> Self {
        let mut rest: Vec<Option<usize>> = (0..count)
            .filter(|page| !pages.contains(page))
            .map(Some)
            .collect();
        let to = to.min(rest.len());
        rest.splice(to..to, pages.iter().copied().map(Some));
        Self {
            was: rest,
            ..Self::default()
        }
    }

    pub(crate) fn removed(count: usize, pages: &[usize]) -> Self {
        Self {
            was: (0..count)
                .filter(|page| !pages.contains(page))
                .map(Some)
                .collect(),
            ..Self::default()
        }
    }

    pub(crate) fn inserted(count: usize, at: usize, added: usize) -> Self {
        let mut was: Vec<Option<usize>> = (0..count).map(Some).collect();
        let at = at.min(count);
        was.splice(at..at, std::iter::repeat_n(None, added));
        Self {
            was,
            ..Self::default()
        }
    }

    pub(crate) fn copied(count: usize, pages: &[usize], at: usize) -> Self {
        let mut was: Vec<Option<usize>> = (0..count).map(Some).collect();
        let at = at.min(count);
        was.splice(at..at, pages.iter().copied().map(Some));
        Self {
            was,
            ..Self::default()
        }
    }

    pub(crate) fn turned(count: usize, pages: &[usize], quarter_turns: i32) -> Self {
        Self {
            was: (0..count).map(Some).collect(),
            turned: pages.to_vec(),
            quarter_turns,
        }
    }

    pub(crate) fn follow<T: Clone>(&self, kept: &BTreeMap<usize, T>) -> BTreeMap<usize, T> {
        self.was
            .iter()
            .enumerate()
            .filter_map(|(now, was)| Some((now, kept.get(&(*was)?)?.clone())))
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Glide {
    from: egui::Vec2,
    to: egui::Vec2,
    started: f64,
}

impl Glide {
    fn at(&self, now: f64) -> egui::Vec2 {
        let share = ((now - self.started) / GLIDE).clamp(0.0, 1.0);
        let eased = 1.0 - (1.0 - share).powi(3);
        #[expect(clippy::cast_possible_truncation, reason = "a share between 0 and 1")]
        let eased = eased as f32;
        self.from + (self.to - self.from) * eased
    }

    fn arrived(&self, now: f64) -> bool {
        self.from == self.to || now - self.started >= GLIDE
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Motion {
    glides: BTreeMap<usize, Glide>,
    panel: Option<crate::room::Flow>,
}

impl Motion {
    pub(crate) fn panel_at(&mut self, settled: f32, now: f64) -> (f32, bool) {
        if let Some(flow) = self.panel
            && (flow.to() - settled).abs() <= crate::room::SAME_WIDTH
            && flow.running(now)
        {
            return (flow.at(now), true);
        }
        self.panel = None;
        (settled, false)
    }

    pub(crate) fn flow_panel(&mut self, from: f32, to: f32, now: f64) {
        self.panel = Some(crate::room::Flow::new(from, to, now));
    }

    pub(crate) fn settle_panel(&mut self) {
        self.panel = None;
    }

    pub(crate) fn at(&mut self, page: usize, target: egui::Vec2, now: f64) -> (egui::Vec2, bool) {
        let glide = self.glides.entry(page).or_insert(Glide {
            from: target,
            to: target,
            started: now,
        });
        if glide.to != target {
            *glide = Glide {
                from: glide.at(now),
                to: target,
                started: now,
            };
        }
        (glide.at(now), !glide.arrived(now))
    }

    pub(crate) fn set_off_from(&mut self, page: usize, from: egui::Vec2, now: f64) {
        self.glides.insert(
            page,
            Glide {
                from,
                to: from,
                started: now,
            },
        );
    }

    pub(crate) fn renumber(&mut self, renumber: &Renumber) {
        self.glides = renumber.follow(&self.glides);
    }

    pub(crate) fn forget(&mut self) {
        self.glides.clear();
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PanelShape {
    pub(crate) rect: egui::Rect,
    pub(crate) pictures: Vec<egui::Rect>,
    pub(crate) columns: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Arriving {
    Pages(std::path::PathBuf),
    Pictures(Vec<std::path::PathBuf>),
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    reason = "the layouts here are sums of whole numbers, which arrive exactly"
)]
mod tests {
    use std::collections::BTreeMap;

    use eframe::egui;

    use super::{COLUMN_GAP, Grid, LABEL, Motion, ROW_GAP, Renumber, gap_at, lay_out, seams};

    fn column() -> Grid {
        Grid {
            columns: 1,
            cell: 140.0,
            thumb: 100.0,
        }
    }

    #[test]
    fn a_narrow_panel_has_one_column_and_a_wide_one_several() {
        assert_eq!(Grid::for_width(150.0).columns, 1);
        let wide = Grid::for_width(3.0 * 128.0 + 2.0 * COLUMN_GAP);
        assert_eq!(wide.columns, 3);
        assert_eq!(wide.width(), 3.0 * 128.0 + 2.0 * COLUMN_GAP);
    }

    #[test]
    fn pictures_stack_under_their_numbers() {
        let (rects, height) = lay_out(&column(), &[100.0, 50.0]);
        assert_eq!(rects[0].min, egui::pos2(20.0, 0.0));
        assert_eq!(rects[1].min.y, 100.0 + LABEL + ROW_GAP);
        assert_eq!(height, 100.0 + LABEL + ROW_GAP + 50.0 + LABEL);
    }

    #[test]
    fn a_row_is_as_tall_as_its_tallest_picture() {
        let grid = Grid {
            columns: 2,
            cell: 100.0,
            thumb: 100.0,
        };
        let (rects, _) = lay_out(&grid, &[80.0, 120.0, 60.0]);
        assert_eq!(rects[1].min, egui::pos2(100.0 + COLUMN_GAP, 0.0));
        assert_eq!(rects[2].min, egui::pos2(0.0, 120.0 + LABEL + ROW_GAP));
    }

    #[test]
    fn the_gap_follows_the_pointer_in_reading_order() {
        let (rects, _) = lay_out(&column(), &[100.0, 100.0, 100.0]);
        assert_eq!(gap_at(1, &rects, egui::pos2(0.0, 10.0)), 0);
        assert_eq!(gap_at(1, &rects, egui::pos2(0.0, 60.0)), 1);
        assert_eq!(gap_at(1, &rects, egui::pos2(0.0, 1000.0)), 3);
        let grid = Grid {
            columns: 2,
            cell: 100.0,
            thumb: 100.0,
        };
        let (rects, _) = lay_out(&grid, &[100.0; 4]);
        assert_eq!(gap_at(2, &rects, egui::pos2(10.0, 50.0)), 0);
        assert_eq!(gap_at(2, &rects, egui::pos2(80.0, 50.0)), 1);
        assert_eq!(gap_at(2, &rects, egui::pos2(200.0, 50.0)), 2);
        assert_eq!(gap_at(2, &rects, egui::pos2(10.0, 190.0)), 2);
    }

    #[test]
    fn a_seam_lies_between_two_pictures() {
        let (rects, _) = lay_out(&column(), &[100.0, 100.0]);
        let seams = seams(1, &rects);
        assert_eq!(seams.len(), 3);
        assert!(seams[1].y > rects[0].bottom() && seams[1].y < rects[1].top());
        assert!(seams[0].y < rects[0].top());
        assert!(seams[2].y > rects[1].bottom());
    }

    #[test]
    fn a_page_moved_forward_leaves_the_others_in_order() {
        assert_eq!(
            Renumber::moved(4, &[0], 2).was,
            vec![Some(1), Some(2), Some(0), Some(3)]
        );
        assert_eq!(
            Renumber::moved(4, &[1, 3], 0).was,
            vec![Some(1), Some(3), Some(0), Some(2)]
        );
    }

    #[test]
    fn pages_taken_out_close_up_and_pages_put_in_open_a_gap() {
        assert_eq!(
            Renumber::removed(4, &[1]).was,
            vec![Some(0), Some(2), Some(3)]
        );
        assert_eq!(
            Renumber::inserted(2, 1, 2).was,
            vec![Some(0), None, None, Some(1)]
        );
        assert_eq!(
            Renumber::copied(2, &[0, 1], 2).was,
            vec![Some(0), Some(1), Some(0), Some(1)]
        );
    }

    #[test]
    fn what_a_page_had_follows_it_to_its_new_number() {
        let had = BTreeMap::from([(0, 'a'), (1, 'b'), (2, 'c')]);
        let followed = Renumber::moved(3, &[2], 0).follow(&had);
        assert_eq!(followed, BTreeMap::from([(0, 'c'), (1, 'a'), (2, 'b')]));
    }

    #[test]
    fn a_picture_glides_to_its_new_place_and_stops_there() {
        let mut motion = Motion::default();
        let (first, moving) = motion.at(0, egui::vec2(0.0, 0.0), 0.0);
        assert_eq!((first, moving), (egui::vec2(0.0, 0.0), false));
        let (partway, moving) = motion.at(0, egui::vec2(0.0, 100.0), 1.0);
        assert!(moving && partway.y == 0.0, "it sets off from where it was");
        let (later, _) = motion.at(0, egui::vec2(0.0, 100.0), 1.1);
        assert!(
            later.y > 50.0 && later.y < 100.0,
            "eased: past halfway at half time"
        );
        let (there, moving) = motion.at(0, egui::vec2(0.0, 100.0), 2.0);
        assert_eq!((there, moving), (egui::vec2(0.0, 100.0), false));
    }
}
