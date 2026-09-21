pub(crate) const MIN_WIDTH: f32 = 440.0;
pub(crate) const MIN_HEIGHT: f32 = 380.0;

pub(crate) const PREFERRED: [f32; 2] = [1250.0, 900.0];

pub(crate) const SCREEN_MARGIN: f32 = 20.0;

pub(crate) const DESKTOP_KEEPS: f32 = 80.0;

pub(crate) fn opening_rect(monitor: [f32; 2]) -> ([f32; 2], [f32; 2]) {
    let [screen_width, screen_height] = monitor;
    let usable_width = (screen_width - SCREEN_MARGIN * 2.0).max(MIN_WIDTH);
    let usable_height = (screen_height - DESKTOP_KEEPS - SCREEN_MARGIN).max(MIN_HEIGHT);
    let width = PREFERRED[0].min(usable_width);
    let height = PREFERRED[1].min(usable_height);
    let left = ((screen_width - width) / 2.0).max(0.0);
    let top = ((screen_height - DESKTOP_KEEPS - height) / 2.0).clamp(0.0, SCREEN_MARGIN);
    ([width, height], [left, top])
}

pub(crate) fn wholly_on_screen(monitor: [f32; 2], size: [f32; 2], at: [f32; 2]) -> bool {
    at[0] >= 0.0
        && at[1] >= 0.0
        && at[0] + size[0] <= monitor[0]
        && at[1] + size[1] <= monitor[1] - DESKTOP_KEEPS
}

pub(crate) fn elide_middle(name: &str, room: f32, measure: impl Fn(&str) -> f32) -> String {
    if measure(name) <= room {
        return name.to_owned();
    }
    let letters: Vec<char> = name.chars().collect();
    let cut = |kept: usize| -> String {
        let head = kept.div_ceil(2);
        let tail = kept - head;
        let mut shortened: String = letters[..head].iter().collect();
        shortened.push('\u{2026}');
        shortened.extend(&letters[letters.len() - tail..]);
        shortened
    };
    let mut fits = 0;
    let mut too_many = letters.len();
    while fits + 1 < too_many {
        let middle = usize::midpoint(fits, too_many);
        if measure(&cut(middle)) <= room {
            fits = middle;
        } else {
            too_many = middle;
        }
    }
    if fits == 0 {
        return "\u{2026}".to_owned();
    }
    cut(fits)
}

pub(crate) const TOOL_WIDTH: f32 = 54.0;

pub(crate) const COMPACT_TOOL_WIDTH: f32 = 34.0;

pub(crate) const TOOL_HEIGHT: f32 = 40.0;

pub(crate) const ICON_SIDE: f32 = 20.0;

pub(crate) const GAP: f32 = 2.0;

pub(crate) const RULE_WIDTH: f32 = 4.0 + 6.0 + 4.0 + GAP;

pub(crate) const LABEL_ROOM: f32 = 24.0;

pub(crate) const EDGE: f32 = 12.0;

pub(crate) const OVERFLOW_WIDTH: f32 = COMPACT_TOOL_WIDTH + GAP;

pub(crate) const fn tool_width(labels: bool) -> f32 {
    if labels {
        TOOL_WIDTH
    } else {
        COMPACT_TOOL_WIDTH
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Bar {
    pub(crate) buttons: usize,
    pub(crate) rules: usize,
    pub(crate) choices: f32,
    pub(crate) readouts: f32,
    pub(crate) slack: f32,
}

impl Bar {
    pub(crate) fn width(&self, labels: bool) -> f32 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a toolbar has tens of buttons, not sixteen million"
        )]
        let buttons = self.buttons as f32;
        #[expect(
            clippy::cast_precision_loss,
            reason = "a toolbar has a handful of rules"
        )]
        let rules = self.rules as f32;
        buttons * (tool_width(labels) + GAP)
            + rules * RULE_WIDTH
            + self.choices
            + self.readouts
            + self.slack
            + EDGE
    }
}

pub(crate) fn labels_fit(room: f32, bar: &Bar, showing: bool) -> bool {
    let needed = bar.width(true);
    if showing {
        room >= needed
    } else {
        room >= needed + LABEL_ROOM
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Piece {
    pub(crate) width: f32,
    pub(crate) rank: u8,
}

pub(crate) const ALL: u8 = u8::MAX;

pub(crate) fn shed_above(room: f32, pieces: &[Piece], overflow: f32) -> u8 {
    let total: f32 = pieces.iter().map(|piece| piece.width).sum();
    if total <= room {
        return ALL;
    }
    let highest = pieces.iter().map(|piece| piece.rank).max().unwrap_or(0);
    for cut in (1..=highest).rev() {
        let used: f32 = pieces
            .iter()
            .filter(|piece| piece.rank < cut)
            .map(|piece| piece.width)
            .sum();
        if used + overflow <= room {
            return cut;
        }
    }
    1
}

pub(crate) const PANEL_WIDTH: f32 = 164.0;

pub(crate) const PANEL_NARROWEST: f32 = 140.0;

pub(crate) const PANEL_WIDEST: f32 = 360.0;

pub(crate) const PANEL_SHARE: f32 = 0.25;

pub(crate) const PANEL_FOLD_FLOOR: f32 = 96.0;

pub(crate) const PANEL_FOLDS_BELOW: f32 = 600.0;

pub(crate) const GRID_INNER: f32 =
    2.0 * crate::page_motion::MIN_CELL + crate::page_motion::COLUMN_GAP;

pub(crate) const PANEL_CHROME: f32 = 26.0;

pub(crate) const GRID_BEGINS: f32 = GRID_INNER + PANEL_CHROME;

pub(crate) fn panel_bounds(window_width: f32) -> (f32, f32) {
    (PANEL_NARROWEST, panel_widest(window_width))
}

pub(crate) fn panel_widest(window_width: f32) -> f32 {
    window_width.max(PANEL_NARROWEST)
}

pub(crate) fn sidebar_widest(window_width: f32) -> f32 {
    let share = (window_width * PANEL_SHARE).min(PANEL_WIDEST);
    share.clamp(PANEL_NARROWEST, GRID_BEGINS)
}

pub(crate) const PANEL_GONE: f32 = 1.0;

pub(crate) const SAME_WIDTH: f32 = 0.5;

pub(crate) fn panel_now(folded: bool, held: bool, remembered: f32, window_width: f32) -> f32 {
    if folded {
        return 0.0;
    }
    if held {
        return remembered.clamp(PANEL_FOLD_FLOOR, panel_widest(window_width));
    }
    panel_width(window_width, remembered)
}

pub(crate) const THUMBS_SPARE: usize = 24;

pub(crate) fn thumbs_kept(
    shown: Option<(usize, usize)>,
) -> Option<std::ops::RangeInclusive<usize>> {
    let (first, last) = shown?;
    Some(first.saturating_sub(THUMBS_SPARE)..=last.saturating_add(THUMBS_SPARE))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Panel {
    pub(crate) width: f32,
    pub(crate) reserved: f32,
    pub(crate) floating: bool,
}

pub(crate) fn panel_layout(width: f32, window_width: f32) -> Panel {
    let width = width.clamp(0.0, panel_widest(window_width));
    Panel {
        width,
        reserved: width.min(sidebar_widest(window_width)),
        floating: width >= GRID_BEGINS,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum FoldPress {
    BackToSidebar(f32),
    FoldAway,
}

pub(crate) fn fold_press(width: f32, window_width: f32) -> FoldPress {
    if panel_layout(width, window_width).floating {
        FoldPress::BackToSidebar(panel_width(window_width, PANEL_WIDTH))
    } else {
        FoldPress::FoldAway
    }
}

pub(crate) const FLOW: f64 = 0.18;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Flow {
    from: f32,
    to: f32,
    started: f64,
}

impl Flow {
    pub(crate) fn new(from: f32, to: f32, now: f64) -> Self {
        Self {
            from,
            to,
            started: now,
        }
    }

    pub(crate) fn at(self, now: f64) -> f32 {
        let share = ((now - self.started) / FLOW).clamp(0.0, 1.0);
        let eased = 1.0 - (1.0 - share).powi(3);
        #[expect(clippy::cast_possible_truncation, reason = "a share between 0 and 1")]
        let eased = eased as f32;
        self.from + (self.to - self.from) * eased
    }

    #[expect(
        clippy::float_cmp,
        reason = "the two ends are copied from one another, so a flow that goes \
                  nowhere holds the very same number in both; a margin here would \
                  call a one-point fold finished before it began"
    )]
    pub(crate) fn running(self, now: f64) -> bool {
        self.from != self.to && now - self.started < FLOW
    }

    pub(crate) fn to(self) -> f32 {
        self.to
    }
}

pub(crate) fn panel_width(window_width: f32, remembered: f32) -> f32 {
    let (narrowest, widest) = panel_bounds(window_width);
    match dragged_to(remembered.clamp(narrowest, widest), window_width) {
        Dragged::Wide(width) => width,
        Dragged::Fold => narrowest,
    }
}

pub(crate) fn panel_starts_folded(window_width: f32) -> bool {
    window_width < PANEL_FOLDS_BELOW
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Dragged {
    Fold,
    Wide(f32),
}

pub(crate) fn dragged_to(width: f32, window_width: f32) -> Dragged {
    let (narrowest, widest) = panel_bounds(window_width);
    if width < narrowest {
        return Dragged::Fold;
    }
    let width = width.min(widest);
    let sidebar = sidebar_widest(window_width);
    if width <= sidebar || width >= GRID_BEGINS {
        return Dragged::Wide(width);
    }
    Dragged::Wide(if width - sidebar < GRID_BEGINS - width {
        sidebar
    } else {
        GRID_BEGINS
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct View {
    pub(crate) pages_folded: bool,
    pub(crate) pages_width: f32,
}

impl Default for View {
    fn default() -> Self {
        Self {
            pages_folded: false,
            pages_width: PANEL_WIDTH,
        }
    }
}

impl View {
    pub(crate) fn write(self) -> String {
        format!(
            "pages-folded {}\npages-width {:.0}\n",
            u8::from(self.pages_folded),
            self.pages_width
        )
    }

    pub(crate) fn read(text: &str) -> Self {
        let mut view = Self::default();
        for line in text.lines() {
            let mut words = line.split_whitespace();
            match (words.next(), words.next()) {
                (Some("pages-folded"), Some(value)) => view.pages_folded = value == "1",
                (Some("pages-width"), Some(value)) => {
                    if let Ok(width) = value.parse::<f32>()
                        && width.is_finite()
                        && width > 0.0
                    {
                        view.pages_width = width;
                    }
                }
                _ => {}
            }
        }
        view
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ALL, Bar, DESKTOP_KEEPS, Dragged, FLOW, Flow, FoldPress, GRID_BEGINS, GRID_INNER,
        LABEL_ROOM, MIN_HEIGHT, MIN_WIDTH, PANEL_CHROME, PANEL_FOLD_FLOOR, PANEL_NARROWEST,
        PANEL_WIDEST, PANEL_WIDTH, PREFERRED, Piece, THUMBS_SPARE, View, dragged_to, elide_middle,
        fold_press, labels_fit, opening_rect, panel_bounds, panel_layout, panel_now,
        panel_starts_folded, panel_width, shed_above, sidebar_widest, thumbs_kept,
        wholly_on_screen,
    };

    #[track_caller]
    fn close(measured: f32, wanted: f32) {
        assert!(
            (measured - wanted).abs() < 0.01,
            "{measured} is not {wanted}"
        );
    }

    fn plain_bar() -> Bar {
        Bar {
            buttons: 20,
            rules: 5,
            choices: 0.0,
            readouts: 90.0,
            slack: 8.0,
        }
    }

    #[test]
    fn the_words_are_under_the_icons_while_the_bar_fits_with_them() {
        let bar = plain_bar();
        let with = bar.width(true);
        let without = bar.width(false);
        assert!(
            (1290.0..1330.0).contains(&with),
            "twenty buttons with words need about 1310 points, not {with}"
        );
        assert!(
            (890.0..930.0).contains(&without),
            "and about 910 without them, not {without}"
        );
        assert!(
            labels_fit(1680.0, &bar, true),
            "a maximised window keeps them"
        );
        assert!(!labels_fit(1250.0, &bar, true), "the opening size cannot");
        assert!(!labels_fit(683.0, &bar, true), "half a laptop is not close");
    }

    #[test]
    fn a_width_just_inside_the_boundary_stays_on_its_own_side_of_it() {
        let bar = plain_bar();
        let needed = bar.width(true);
        assert!(labels_fit(needed, &bar, true));
        assert!(!labels_fit(needed - 0.1, &bar, true));
        assert!(!labels_fit(needed, &bar, false), "hysteresis, not a knife");
        assert!(!labels_fit(needed + LABEL_ROOM - 1.0, &bar, false));
        assert!(labels_fit(needed + LABEL_ROOM, &bar, false));
    }

    #[test]
    fn putting_a_tool_down_brings_the_words_back_at_the_same_window_size() {
        let room = 1680.0;
        let bare = plain_bar();
        assert!(labels_fit(room, &bare, true), "a maximised window has room");
        let with_shape = Bar {
            choices: 620.0,
            ..bare
        };
        assert!(
            !labels_fit(room, &with_shape, true),
            "the Shape tool's choices take the words' room"
        );
        assert!(
            labels_fit(room, &bare, false),
            "the words come back once the choices go"
        );
    }

    #[test]
    fn what_does_not_fit_leaves_the_bar_highest_rank_first() {
        let pieces = [
            Piece {
                width: 100.0,
                rank: 0,
            },
            Piece {
                width: 100.0,
                rank: 1,
            },
            Piece {
                width: 100.0,
                rank: 2,
            },
            Piece {
                width: 100.0,
                rank: 3,
            },
        ];
        assert_eq!(shed_above(400.0, &pieces, 40.0), ALL, "it all fits");
        assert_eq!(shed_above(399.0, &pieces, 40.0), 3);
        assert_eq!(shed_above(340.0, &pieces, 40.0), 3);
        assert_eq!(shed_above(339.0, &pieces, 40.0), 2);
        assert_eq!(shed_above(140.0, &pieces, 40.0), 1);
        assert_eq!(shed_above(10.0, &pieces, 40.0), 1);
    }

    #[test]
    fn a_wider_window_never_sheds_more_than_a_narrower_one() {
        let pieces: Vec<Piece> = (0..8u8)
            .map(|rank| Piece {
                width: 34.0 + f32::from(rank),
                rank,
            })
            .collect();
        let mut last = 0;
        for room in 0..600 {
            let cut = shed_above(
                f32::from(u16::try_from(room).expect("small")),
                &pieces,
                36.0,
            );
            assert!(cut >= last, "room {room} shed more than a narrower window");
            last = cut;
        }
    }

    #[test]
    fn a_sidebar_may_take_a_quarter_of_the_window_and_never_two_columns() {
        for (window, widest) in [
            (1680.0, GRID_BEGINS),
            (3840.0, GRID_BEGINS),
            (1400.0, GRID_BEGINS),
            (960.0, 240.0),
            (683.0, 170.75),
            (455.0, PANEL_NARROWEST),
        ] {
            let (narrowest, whole) = panel_bounds(window);
            close(narrowest, PANEL_NARROWEST);
            close(whole, window);
            close(sidebar_widest(window), widest);
        }
        for width in [MIN_WIDTH, 455.0, 683.0, 960.0, 1280.0, 1680.0, 3840.0] {
            assert!(
                sidebar_widest(width) <= (width * 0.5).max(PANEL_NARROWEST),
                "a sidebar taking half the window at {width}"
            );
            assert!(
                sidebar_widest(width) <= GRID_BEGINS,
                "a sidebar with room for two columns at {width}"
            );
        }
        assert!(sidebar_widest(960.0) < GRID_BEGINS);
        const { assert!(GRID_BEGINS < PANEL_WIDEST) };
    }

    #[test]
    fn the_grid_begins_where_a_second_column_first_fits() {
        close(GRID_INNER, 270.0);
        close(GRID_BEGINS, 296.0);
        assert_eq!(crate::page_motion::Grid::for_width(GRID_INNER).columns, 2);
        assert_eq!(
            crate::page_motion::Grid::for_width(GRID_INNER - 1.0).columns,
            1,
            "a point short of the threshold is still one column"
        );
        assert_eq!(
            crate::page_motion::Grid::for_width(GRID_BEGINS - PANEL_CHROME).columns,
            2
        );
        assert_eq!(
            crate::page_motion::Grid::for_width(GRID_BEGINS - 1.0 - PANEL_CHROME).columns,
            1
        );
    }

    #[test]
    fn the_page_view_stops_shrinking_where_the_grid_begins() {
        let sidebar = panel_layout(200.0, 1680.0);
        assert!(!sidebar.floating);
        close(sidebar.width, 200.0);
        close(sidebar.reserved, 200.0);
        let edge = panel_layout(GRID_BEGINS, 1680.0);
        assert!(edge.floating, "the threshold belongs to the grid");
        close(edge.reserved, edge.width);
        for pulled in [GRID_BEGINS, 400.0, 840.0, 1200.0, 1680.0, 4000.0] {
            let grid = panel_layout(pulled, 1680.0);
            assert!(grid.floating);
            close(grid.reserved, GRID_BEGINS);
            close(grid.width, pulled.min(1680.0));
        }
        for pulled in [GRID_BEGINS, 500.0, 960.0] {
            let grid = panel_layout(pulled, 960.0);
            assert!(grid.floating);
            close(grid.reserved, 240.0);
        }
        close(panel_layout(240.0, 960.0).reserved, 240.0);
        let left = |pulled: f32| 1680.0 - panel_layout(pulled, 1680.0).reserved;
        close(left(1200.0), left(GRID_BEGINS));
        close(left(1680.0), left(GRID_BEGINS));
        assert!(left(200.0) > left(GRID_BEGINS), "a sidebar still pushes");
    }

    #[test]
    fn the_corner_button_leaves_the_grid_first_and_folds_second() {
        assert_eq!(
            fold_press(900.0, 1680.0),
            FoldPress::BackToSidebar(PANEL_WIDTH)
        );
        assert_eq!(
            fold_press(GRID_BEGINS, 1680.0),
            FoldPress::BackToSidebar(PANEL_WIDTH)
        );
        assert_eq!(fold_press(GRID_BEGINS - 1.0, 1680.0), FoldPress::FoldAway);
        assert_eq!(fold_press(PANEL_WIDTH, 1680.0), FoldPress::FoldAway);
        assert_eq!(
            fold_press(600.0, 683.0),
            FoldPress::BackToSidebar(PANEL_WIDTH)
        );
    }

    #[test]
    fn a_panel_remembered_wider_than_this_window_allows_comes_back_inside() {
        close(panel_width(1680.0, 2400.0), 1680.0);
        close(panel_width(1680.0, 640.0), 640.0);
        close(panel_width(683.0, 900.0), 683.0);
        close(panel_width(1680.0, 200.0), 200.0);
        close(panel_width(1680.0, 10.0), PANEL_NARROWEST);
        close(panel_width(960.0, 260.0), 240.0);
        close(panel_width(960.0, 290.0), GRID_BEGINS);
    }

    #[test]
    fn the_narrow_snapped_windows_start_with_the_panel_folded() {
        assert!(panel_starts_folded(455.0), "a third of a 1366 laptop");
        assert!(panel_starts_folded(MIN_WIDTH));
        assert!(!panel_starts_folded(683.0), "a half of a 1366 laptop");
        assert!(!panel_starts_folded(960.0));
        assert!(!panel_starts_folded(1280.0));
    }

    #[test]
    fn dragging_the_edge_in_far_enough_folds_the_panel_rather_than_slivering_it() {
        assert_eq!(dragged_to(100.0, 1680.0), Dragged::Fold);
        assert_eq!(dragged_to(139.0, 1680.0), Dragged::Fold);
        for (dragged, window, left) in [
            (140.0, 1680.0, 140.0),
            (800.0, 1680.0, 800.0),
            (2000.0, 1680.0, 1680.0),
            (400.0, 683.0, 400.0),
        ] {
            let Dragged::Wide(width) = dragged_to(dragged, window) else {
                panic!("{dragged} on a {window} window should not fold");
            };
            close(width, left);
        }
    }

    #[test]
    fn a_width_that_is_neither_a_sidebar_nor_a_grid_goes_to_the_nearer_one() {
        let settled = |width: f32, window: f32| match dragged_to(width, window) {
            Dragged::Wide(width) => width,
            Dragged::Fold => panic!("{width} on a {window} window should not fold"),
        };
        close(settled(250.0, 960.0), 240.0);
        close(settled(290.0, 960.0), GRID_BEGINS);
        close(settled(267.0, 960.0), 240.0);
        close(settled(269.0, 960.0), GRID_BEGINS);
        close(settled(240.0, 960.0), 240.0);
        close(settled(800.0, 960.0), 800.0);
        for width in [250.0_f32, 270.0, 290.0] {
            close(settled(width, 1680.0), width);
        }
    }

    #[test]
    fn a_folded_panel_a_held_edge_and_a_remembered_width_are_three_answers() {
        close(panel_now(true, false, 240.0, 1680.0), 0.0);
        close(panel_now(true, true, 240.0, 1680.0), 0.0);
        close(panel_now(false, true, 100.0, 1680.0), 100.0);
        close(panel_now(false, true, 2000.0, 1680.0), 1680.0);
        close(panel_now(false, true, 10.0, 1680.0), PANEL_FOLD_FLOOR);
        close(panel_now(false, false, 100.0, 1680.0), PANEL_NARROWEST);
        close(panel_now(false, false, 900.0, 1680.0), 900.0);
        close(panel_now(false, false, 260.0, 960.0), 240.0);
    }

    #[test]
    fn the_panel_keeps_a_screenful_of_pictures_either_side_of_what_it_shows() {
        let kept = thumbs_kept(Some((100, 123))).expect("something is shown");
        assert_eq!(kept, 76..=147);
        assert_eq!(kept.count(), 24 + 2 * THUMBS_SPARE);
        assert!(!thumbs_kept(Some((100, 123))).expect("shown").contains(&75));
        assert_eq!(thumbs_kept(Some((0, 5))).expect("shown"), 0..=29);
        assert!(
            thumbs_kept(None).is_none(),
            "a folded panel forgets nothing"
        );
        let small = thumbs_kept(Some((0, 9))).expect("shown").count();
        let large = thumbs_kept(Some((900, 909))).expect("shown").count();
        assert_eq!(small, 10 + THUMBS_SPARE, "the front is clipped at zero");
        assert_eq!(large, 10 + 2 * THUMBS_SPARE);
    }

    #[test]
    fn a_panel_flows_to_its_new_width_in_its_own_time() {
        let flow = Flow::new(164.0, 964.0, 10.0);
        close(flow.at(10.0), 164.0);
        close(flow.at(20.0), 964.0);
        assert!(flow.running(10.0));
        assert!(flow.running(10.0 + FLOW / 2.0));
        assert!(!flow.running(10.0 + FLOW + 0.000_01));
        assert!(!flow.running(11.0));
        let halfway = flow.at(10.0 + FLOW / 2.0);
        close(halfway, 164.0 + 800.0 * 0.875);
        assert!(
            halfway > f32::midpoint(164.0, 964.0),
            "eased out, not a straight line"
        );
        close(flow.to(), 964.0);
        let still = Flow::new(300.0, 300.0, 0.0);
        assert!(
            !still.running(0.0),
            "a panel going nowhere asks for no frames"
        );
    }

    #[test]
    fn what_the_window_remembers_about_itself_is_written_and_read_back() {
        let view = View {
            pages_folded: true,
            pages_width: 231.0,
        };
        assert_eq!(View::read(&view.write()), view);
        let open = View {
            pages_folded: false,
            pages_width: 164.0,
        };
        assert_eq!(View::read(&open.write()), open);
        assert_eq!(View::read(""), View::default());
        assert_eq!(View::read("moon-phase waxing\n"), View::default());
        assert_eq!(View::read("pages-width nonsense\n"), View::default());
        assert_eq!(View::read("pages-width -5\n"), View::default());
    }

    #[test]
    fn the_smallest_window_allowed_is_under_every_size_real_snapping_makes() {
        let screens = [
            [1366.0_f32, 768.0_f32],
            [1920.0, 1080.0],
            [2560.0, 1440.0],
            [2560.0, 1440.0],
        ];
        let mut narrowest = f32::INFINITY;
        let mut shortest = f32::INFINITY;
        for [width, height] in screens {
            for across in [width / 2.0, width / 3.0] {
                narrowest = narrowest.min(across);
            }
            shortest = shortest.min(height / 2.0);
        }
        close(narrowest, 455.333_34);
        close(shortest, 384.0);
        assert!(
            MIN_WIDTH <= narrowest,
            "the narrowest snap is {narrowest} and the floor is {MIN_WIDTH}"
        );
        assert!(
            MIN_HEIGHT <= shortest,
            "the shortest snap is {shortest} and the floor is {MIN_HEIGHT}"
        );
    }

    #[test]
    fn a_new_window_opens_wholly_on_a_1280_by_800_laptop() {
        let monitor = [1280.0, 800.0];
        let (size, at) = opening_rect(monitor);
        close(size[0], 1240.0);
        close(size[1], 700.0);
        close(at[0], 20.0);
        close(at[1], 10.0);
        assert!(wholly_on_screen(monitor, size, at));
        assert!(!wholly_on_screen(monitor, [1266.0, 839.0], [78.0, 78.0]));
    }

    #[test]
    fn a_new_window_is_wholly_on_every_screen_it_can_open_on() {
        let (size, at) = opening_rect([2560.0, 1440.0]);
        close(size[0], PREFERRED[0]);
        close(size[1], PREFERRED[1]);
        assert!(wholly_on_screen([2560.0, 1440.0], size, at));
        for monitor in [
            [1280.0, 800.0],
            [1366.0, 768.0],
            [1920.0, 1080.0],
            [1024.0, 768.0],
            [800.0, 600.0],
            [3840.0, 2160.0],
        ] {
            let (size, at) = opening_rect(monitor);
            assert!(
                wholly_on_screen(monitor, size, at),
                "{size:?} at {at:?} runs off a {monitor:?} screen"
            );
            assert!(size[0] >= MIN_WIDTH && size[1] >= MIN_HEIGHT);
            assert!(size[1] <= monitor[1] - DESKTOP_KEEPS);
        }
    }

    #[test]
    fn a_name_too_long_for_its_place_loses_its_middle_and_not_its_end() {
        let seven = |text: &str| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "a file name is not four billion letters"
            )]
            let width = text.chars().count() as f32 * 7.0;
            width
        };
        let long = "quarterly-report-final-final-with-the-corrections.pdf";
        let short = elide_middle(long, 140.0, seven);
        assert!(seven(&short) <= 140.0, "{short} is still too wide");
        assert!(short.contains('\u{2026}'), "{short} says nothing was cut");
        let last_four: String = short.chars().rev().take(4).collect();
        assert_eq!(last_four, "fdp.", "{short} lost the extension");
        assert!(short.starts_with("quarterly"), "{short} lost the start");
        assert_eq!(short.chars().count(), 20, "nineteen letters and the dots");
    }

    #[test]
    fn a_name_that_fits_is_left_alone() {
        let seven = |text: &str| {
            #[expect(clippy::cast_precision_loss, reason = "a short name")]
            let width = text.chars().count() as f32 * 7.0;
            width
        };
        let name = "report.pdf";
        assert_eq!(
            elide_middle(name, 70.0, seven),
            name,
            "ten letters, 70 wide"
        );
        assert_ne!(elide_middle(name, 69.0, seven), name, "one point short");
        assert_eq!(elide_middle(name, 0.0, seven), "\u{2026}");
        assert_eq!(elide_middle("", 0.0, seven), "");
    }
}
