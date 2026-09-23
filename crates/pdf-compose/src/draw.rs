use crate::chart::{Chart, ChartKind};
use crate::measure::{Measure, Pen};
use crate::theme::{Colour, Rect, Theme};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    pub x: f32,
    pub y: f32,
}

impl Spot {
    #[must_use]
    pub const fn at(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stands {
    Start,
    Middle,
    End,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Fill {
        rect: Rect,
        colour: Colour,
    },
    Stroke {
        from: Spot,
        to: Spot,
        width: f32,
        colour: Colour,
    },
    Path {
        points: Vec<Spot>,
        width: f32,
        colour: Colour,
        filled: bool,
    },
    Wedge {
        centre: Spot,
        radius: f32,
        from: f32,
        to: f32,
        colour: Colour,
    },
    Words {
        at: Spot,
        text: String,
        pen: Pen,
        colour: Colour,
        stands: Stands,
    },
}

#[must_use]
pub fn draw(chart: &Chart, frame: Rect, theme: &Theme, measure: &dyn Measure) -> Vec<Shape> {
    if chart.series.is_empty() || chart.labels.is_empty() || frame.width <= 0.0 {
        return Vec::new();
    }
    if chart.kind == ChartKind::Pie {
        return pie(chart, frame, theme, measure);
    }
    let mut shapes = Vec::new();
    let body = legend(chart, frame, theme, measure, &mut shapes);
    if chart.kind == ChartKind::BarAcross {
        across(chart, body, theme, measure, &mut shapes);
    } else {
        upright(chart, body, theme, measure, &mut shapes);
    }
    shapes
}

fn legend(
    chart: &Chart,
    frame: Rect,
    theme: &Theme,
    measure: &dyn Measure,
    shapes: &mut Vec<Shape>,
) -> Rect {
    if chart.series.len() < 2 {
        return frame;
    }
    let pen = Pen::from(theme.axis);
    let swatch = theme.axis.size * 0.7;
    let gap = theme.axis.size * 0.5;
    let mut x = frame.x;
    let baseline = frame.y + measure.ascent(pen);
    for (at, series) in chart.series.iter().enumerate() {
        let width = measure.width(&series.name, pen);
        if x + swatch + gap + width > frame.right() {
            break;
        }
        shapes.push(Shape::Fill {
            rect: Rect::new(x, baseline - swatch, swatch, swatch),
            colour: theme.colour(at),
        });
        shapes.push(Shape::Words {
            at: Spot::at(x + swatch + gap * 0.5, baseline),
            text: series.name.clone(),
            pen,
            colour: theme.ink,
            stands: Stands::Start,
        });
        x += swatch + gap * 0.5 + width + gap * 1.5;
    }
    let taken = theme.axis.leading;
    Rect::new(
        frame.x,
        frame.y + taken,
        frame.width,
        (frame.height - taken).max(1.0),
    )
}

struct Scale {
    plot: Rect,
    ticks: Vec<f64>,
    low: f64,
    high: f64,
}

impl Scale {
    fn part(&self, value: f64) -> f32 {
        let span = self.high - self.low;
        if span <= 0.0 {
            return 0.0;
        }
        #[allow(clippy::cast_possible_truncation)]
        let part = ((value - self.low) / span) as f32;
        part
    }

    fn down(&self, value: f64) -> f32 {
        self.plot.bottom() - self.part(value) * self.plot.height
    }

    fn across(&self, value: f64) -> f32 {
        self.plot.x + self.part(value) * self.plot.width
    }
}

fn upright(
    chart: &Chart,
    body: Rect,
    theme: &Theme,
    measure: &dyn Measure,
    shapes: &mut Vec<Shape>,
) {
    let stacked = chart.kind == ChartKind::StackedBar;
    let (low, high) = if stacked {
        chart.stacked_reach()
    } else {
        chart.reach()
    };
    let ticks = ticks(low, high, 5);
    let pen = Pen::from(theme.axis);
    let step = ticks.get(1).zip(ticks.first()).map_or(1.0, |(a, b)| a - b);
    let widest = ticks
        .iter()
        .map(|tick| measure.width(&say(*tick, step), pen))
        .fold(0.0_f32, f32::max);
    let gutter = widest + theme.axis.size;
    let under = theme.axis.leading;
    let plot = Rect::new(
        body.x + gutter,
        body.y,
        (body.width - gutter).max(1.0),
        (body.height - under).max(1.0),
    );
    let scale = Scale {
        plot,
        low: *ticks.first().unwrap_or(&low),
        high: *ticks.last().unwrap_or(&high),
        ticks,
    };
    rules_across(&scale, step, theme, shapes);
    categories(chart, plot, theme, measure, shapes);
    match chart.kind {
        ChartKind::StackedBar => stacked_bars(chart, &scale, theme, shapes),
        ChartKind::Line | ChartKind::Area => curves(chart, &scale, theme, shapes),
        _ => grouped_bars(chart, &scale, theme, shapes),
    }
}

fn rules_across(scale: &Scale, step: f64, theme: &Theme, shapes: &mut Vec<Shape>) {
    let pen = Pen::from(theme.axis);
    for tick in &scale.ticks {
        let y = scale.down(*tick);
        let ground = tick.abs() < step * 1e-6;
        shapes.push(Shape::Stroke {
            from: Spot::at(scale.plot.x, y),
            to: Spot::at(scale.plot.right(), y),
            width: if ground { theme.grid * 2.0 } else { theme.grid },
            colour: if ground { theme.ink } else { theme.faint },
        });
        shapes.push(Shape::Words {
            at: Spot::at(scale.plot.x - theme.axis.size * 0.5, y + pen.size * 0.35),
            text: say(*tick, step),
            pen,
            colour: theme.ink,
            stands: Stands::End,
        });
    }
}

fn categories(
    chart: &Chart,
    plot: Rect,
    theme: &Theme,
    measure: &dyn Measure,
    shapes: &mut Vec<Shape>,
) {
    let pen = Pen::from(theme.axis);
    #[allow(clippy::cast_precision_loss)]
    let groups = chart.labels.len() as f32;
    let width = plot.width / groups;
    for (at, label) in chart.labels.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let middle = plot.x + (at as f32 + 0.5) * width;
        shapes.push(Shape::Words {
            at: Spot::at(
                middle,
                plot.bottom() + measure.ascent(pen) + theme.axis.size * 0.3,
            ),
            text: fit(label, width - 2.0, pen, measure),
            pen,
            colour: theme.ink,
            stands: Stands::Middle,
        });
    }
}

fn grouped_bars(chart: &Chart, scale: &Scale, theme: &Theme, shapes: &mut Vec<Shape>) {
    #[allow(clippy::cast_precision_loss)]
    let groups = chart.labels.len() as f32;
    #[allow(clippy::cast_precision_loss)]
    let many = chart.series.len().max(1) as f32;
    let group = scale.plot.width / groups;
    let width = (group * (1.0 - theme.bar_air) / many).max(0.5);
    let ground = scale.down(0.0);
    for (which, series) in chart.series.iter().enumerate() {
        for (at, value) in series.values.iter().enumerate() {
            if !value.is_finite() || at >= chart.labels.len() {
                continue;
            }
            #[allow(clippy::cast_precision_loss)]
            let start = scale.plot.x + at as f32 * group + group * theme.bar_air * 0.5;
            #[allow(clippy::cast_precision_loss)]
            let x = start + which as f32 * width;
            let y = scale.down(*value);
            shapes.push(Shape::Fill {
                rect: Rect::new(x, y.min(ground), width, (ground - y).abs()),
                colour: theme.colour(which),
            });
        }
    }
}

fn stacked_bars(chart: &Chart, scale: &Scale, theme: &Theme, shapes: &mut Vec<Shape>) {
    #[allow(clippy::cast_precision_loss)]
    let groups = chart.labels.len() as f32;
    let group = scale.plot.width / groups;
    let width = (group * (1.0 - theme.bar_air)).max(0.5);
    for at in 0..chart.labels.len() {
        #[allow(clippy::cast_precision_loss)]
        let x = scale.plot.x + at as f32 * group + group * theme.bar_air * 0.5;
        let (mut up, mut down) = (0.0_f64, 0.0_f64);
        for (which, series) in chart.series.iter().enumerate() {
            let Some(value) = series.values.get(at).copied() else {
                continue;
            };
            if !value.is_finite() {
                continue;
            }
            let (from, to) = if value >= 0.0 {
                let was = up;
                up += value;
                (was, up)
            } else {
                let was = down;
                down += value;
                (down, was)
            };
            let top = scale.down(to);
            shapes.push(Shape::Fill {
                rect: Rect::new(x, top, width, (scale.down(from) - top).abs()),
                colour: theme.colour(which),
            });
        }
    }
}

fn curves(chart: &Chart, scale: &Scale, theme: &Theme, shapes: &mut Vec<Shape>) {
    let filled = chart.kind == ChartKind::Area;
    #[allow(clippy::cast_precision_loss)]
    let groups = chart.labels.len() as f32;
    let step = scale.plot.width / groups;
    for (which, series) in chart.series.iter().enumerate() {
        let mut points: Vec<Spot> = Vec::new();
        for (at, value) in series.values.iter().enumerate() {
            if !value.is_finite() || at >= chart.labels.len() {
                continue;
            }
            #[allow(clippy::cast_precision_loss)]
            let x = scale.plot.x + (at as f32 + 0.5) * step;
            points.push(Spot::at(x, scale.down(*value)));
        }
        if points.is_empty() {
            continue;
        }
        if filled {
            let mut under = points.clone();
            let ground = scale.down(0.0);
            under.push(Spot::at(points[points.len() - 1].x, ground));
            under.push(Spot::at(points[0].x, ground));
            shapes.push(Shape::Path {
                points: under,
                width: 0.0,
                colour: theme.colour(which),
                filled: true,
            });
        }
        shapes.push(Shape::Path {
            points,
            width: 1.5,
            colour: theme.colour(which),
            filled: false,
        });
    }
}

fn across(
    chart: &Chart,
    body: Rect,
    theme: &Theme,
    measure: &dyn Measure,
    shapes: &mut Vec<Shape>,
) {
    let (low, high) = chart.reach();
    let ticks = ticks(low, high, 4);
    let pen = Pen::from(theme.axis);
    let step = ticks.get(1).zip(ticks.first()).map_or(1.0, |(a, b)| a - b);
    let gutter = chart
        .labels
        .iter()
        .map(|label| measure.width(label, pen))
        .fold(0.0_f32, f32::max)
        .min(body.width * 0.35)
        + theme.axis.size;
    let plot = Rect::new(
        body.x + gutter,
        body.y,
        (body.width - gutter).max(1.0),
        (body.height - theme.axis.leading).max(1.0),
    );
    let scale = Scale {
        plot,
        low: *ticks.first().unwrap_or(&low),
        high: *ticks.last().unwrap_or(&high),
        ticks,
    };
    for tick in &scale.ticks {
        let x = scale.across(*tick);
        let ground = tick.abs() < step * 1e-6;
        shapes.push(Shape::Stroke {
            from: Spot::at(x, plot.y),
            to: Spot::at(x, plot.bottom()),
            width: if ground { theme.grid * 2.0 } else { theme.grid },
            colour: if ground { theme.ink } else { theme.faint },
        });
        shapes.push(Shape::Words {
            at: Spot::at(
                x,
                plot.bottom() + measure.ascent(pen) + theme.axis.size * 0.3,
            ),
            text: say(*tick, step),
            pen,
            colour: theme.ink,
            stands: Stands::Middle,
        });
    }
    lying_bars(chart, &scale, gutter, theme, measure, shapes);
}

fn lying_bars(
    chart: &Chart,
    scale: &Scale,
    gutter: f32,
    theme: &Theme,
    measure: &dyn Measure,
    shapes: &mut Vec<Shape>,
) {
    let pen = Pen::from(theme.axis);
    #[allow(clippy::cast_precision_loss)]
    let rows = chart.labels.len() as f32;
    #[allow(clippy::cast_precision_loss)]
    let many = chart.series.len().max(1) as f32;
    let row = scale.plot.height / rows;
    let height = (row * (1.0 - theme.bar_air) / many).max(0.5);
    let ground = scale.across(0.0);
    for (at, label) in chart.labels.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let top = scale.plot.y + at as f32 * row + row * theme.bar_air * 0.5;
        shapes.push(Shape::Words {
            at: Spot::at(
                scale.plot.x - theme.axis.size * 0.5,
                top + row * 0.5 + pen.size * 0.35,
            ),
            text: fit(label, gutter - theme.axis.size, pen, measure),
            pen,
            colour: theme.ink,
            stands: Stands::End,
        });
        for (which, series) in chart.series.iter().enumerate() {
            let Some(value) = series.values.get(at).copied() else {
                continue;
            };
            if !value.is_finite() {
                continue;
            }
            let end = scale.across(value);
            #[allow(clippy::cast_precision_loss)]
            let y = top + which as f32 * height;
            shapes.push(Shape::Fill {
                rect: Rect::new(end.min(ground), y, (end - ground).abs(), height),
                colour: theme.colour(which),
            });
        }
    }
}

fn pie(chart: &Chart, frame: Rect, theme: &Theme, measure: &dyn Measure) -> Vec<Shape> {
    let mut shapes = Vec::new();
    let Some(series) = chart.series.first() else {
        return shapes;
    };
    let total: f64 = series
        .values
        .iter()
        .filter(|value| value.is_finite() && **value > 0.0)
        .sum();
    if total <= 0.0 {
        return shapes;
    }
    let pen = Pen::from(theme.axis);
    let names = chart
        .labels
        .iter()
        .map(|label| measure.width(label, pen))
        .fold(0.0_f32, f32::max)
        + theme.axis.size * 2.5;
    let names = names.min(frame.width * 0.4);
    let round = (frame.width - names).min(frame.height);
    let radius = (round / 2.0).max(1.0);
    let centre = Spot::at(frame.x + radius, frame.y + frame.height / 2.0);
    let mut turned = 0.0_f32;
    for (at, value) in series.values.iter().enumerate() {
        if !value.is_finite() || *value <= 0.0 || at >= chart.labels.len() {
            continue;
        }
        #[allow(clippy::cast_possible_truncation)]
        let sweep = (value / total) as f32 * 360.0;
        shapes.push(Shape::Wedge {
            centre,
            radius,
            from: turned,
            to: turned + sweep,
            colour: theme.colour(at),
        });
        turned += sweep;
    }
    let swatch = theme.axis.size * 0.7;
    let left = frame.right() - names + theme.axis.size * 0.5;
    #[allow(clippy::cast_precision_loss)]
    let tall = chart.labels.len() as f32 * theme.axis.leading;
    let mut y = frame.y + (frame.height - tall).max(0.0) / 2.0 + measure.ascent(pen);
    for (at, label) in chart.labels.iter().enumerate() {
        shapes.push(Shape::Fill {
            rect: Rect::new(left, y - swatch, swatch, swatch),
            colour: theme.colour(at),
        });
        shapes.push(Shape::Words {
            at: Spot::at(left + swatch * 1.6, y),
            text: fit(label, names - swatch * 2.2, pen, measure),
            pen,
            colour: theme.ink,
            stands: Stands::Start,
        });
        y += theme.axis.leading;
    }
    shapes
}

#[must_use]
pub fn ticks(low: f64, high: f64, most: usize) -> Vec<f64> {
    let most = most.max(1);
    if !low.is_finite() || !high.is_finite() || high <= low {
        return vec![low.min(0.0), low.min(0.0) + 1.0];
    }
    #[allow(clippy::cast_precision_loss)]
    let raw = (high - low) / most as f64;
    let power = 10.0_f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0]
        .iter()
        .map(|rung| rung * power)
        .find(|rung| *rung >= raw)
        .unwrap_or(power * 10.0);
    let first = (low / step).floor() * step;
    let last = (high / step).ceil() * step;
    let mut out = Vec::new();
    let mut at = first;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let many = ((last - first) / step).round() as usize;
    for _ in 0..=many.min(64) {
        out.push(at);
        at += step;
    }
    out
}

#[must_use]
pub fn say(value: f64, step: f64) -> String {
    let places = if step.abs() >= 1.0 || step == 0.0 {
        0
    } else {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let places = (-step.abs().log10()).ceil().clamp(0.0, 3.0) as usize;
        places
    };
    let said = format!("{value:.places$}");
    if said
        .trim_start_matches('-')
        .trim_matches('0')
        .trim_matches('.')
        .is_empty()
    {
        return said.trim_start_matches('-').to_owned();
    }
    said
}

#[must_use]
pub fn fit(text: &str, room: f32, pen: Pen, measure: &dyn Measure) -> String {
    if room <= 0.0 {
        return String::new();
    }
    if measure.width(text, pen) <= room {
        return text.to_owned();
    }
    let mark = '\u{2026}';
    let mut kept = String::new();
    for letter in text.chars() {
        let mut tried = kept.clone();
        tried.push(letter);
        tried.push(mark);
        if measure.width(&tried, pen) > room {
            break;
        }
        kept.push(letter);
    }
    if kept.is_empty() {
        return String::new();
    }
    kept.push(mark);
    kept
}

#[cfg(test)]
mod tests {
    use super::{Shape, Spot, draw, fit, say, ticks};
    use crate::chart::{Chart, ChartKind, Series};
    use crate::measure::{Even, Pen};
    use crate::theme::{Face, Rect, Theme};

    fn chart(kind: ChartKind, values: &[&[f64]]) -> Chart {
        Chart {
            kind,
            title: None,
            labels: (0..values[0].len()).map(|at| format!("L{at}")).collect(),
            series: values
                .iter()
                .enumerate()
                .map(|(at, row)| Series {
                    name: format!("s{at}"),
                    values: (*row).to_vec(),
                })
                .collect(),
            unit: None,
            note: None,
        }
    }

    fn frame() -> Rect {
        Rect::new(50.0, 100.0, 400.0, 200.0)
    }

    fn fills(shapes: &[Shape]) -> Vec<Rect> {
        shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Fill { rect, .. } => Some(*rect),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn ticks_are_round_numbers_off_the_ladder_of_one_two_and_five() {
        assert_eq!(ticks(0.0, 100.0, 5), [0.0, 20.0, 40.0, 60.0, 80.0, 100.0]);
        assert_eq!(ticks(0.0, 9.0, 5), [0.0, 2.0, 4.0, 6.0, 8.0, 10.0]);
        assert_eq!(ticks(0.0, 1.0, 4), [0.0, 0.5, 1.0]);
        let even = ticks(0.0, 9.0, 5);
        assert!(!even.iter().any(|tick| (tick - 1.8).abs() < 0.001));
    }

    #[test]
    fn an_axis_with_nothing_on_it_still_answers() {
        assert_eq!(ticks(0.0, 0.0, 5).len(), 2);
        assert_eq!(ticks(f64::NAN, 3.0, 5).len(), 2);
    }

    #[test]
    fn a_number_is_written_with_the_decimals_its_step_needs() {
        assert_eq!(say(20.0, 20.0), "20");
        assert_eq!(say(0.5, 0.5), "0.5");
        assert_eq!(say(0.25, 0.05), "0.25");
        assert_eq!(say(-0.004, 1.0), "0");
    }

    #[test]
    fn the_value_axis_reaches_the_ground_so_a_small_difference_looks_small() {
        let shapes = draw(
            &chart(ChartKind::Bar, &[&[98.0, 100.0]]),
            frame(),
            &Theme::house(),
            &Even::half(),
        );
        let bars = fills(&shapes);
        assert_eq!(bars.len(), 2);
        let ratio = bars[0].height / bars[1].height;
        assert!(
            (ratio - 0.98).abs() < 0.02,
            "a 2% difference drew as {ratio}"
        );
    }

    #[test]
    fn a_bar_is_as_tall_as_its_number() {
        let shapes = draw(
            &chart(ChartKind::Bar, &[&[50.0, 100.0]]),
            frame(),
            &Theme::house(),
            &Even::half(),
        );
        let bars = fills(&shapes);
        assert!((bars[1].height / bars[0].height - 2.0).abs() < 0.01);
        assert!((bars[0].bottom() - bars[1].bottom()).abs() < 0.01);
    }

    #[test]
    fn a_stacked_bar_is_its_pieces_end_to_end() {
        let theme = Theme::house();
        let stacked = draw(
            &chart(ChartKind::StackedBar, &[&[30.0], &[70.0]]),
            frame(),
            &theme,
            &Even::half(),
        );
        let bars: Vec<Rect> = fills(&stacked)
            .into_iter()
            .filter(|rect| rect.width > theme.axis.size)
            .collect();
        assert_eq!(bars.len(), 2);
        let ratio = bars[1].height / bars[0].height;
        assert!((ratio - 70.0 / 30.0).abs() < 0.02, "piled in {ratio}");
        let highest = stacked
            .iter()
            .filter_map(|shape| match shape {
                Shape::Stroke { from, .. } => Some(from.y),
                _ => None,
            })
            .fold(f32::MAX, f32::min);
        assert!(
            (bars[1].y - highest).abs() < 0.5,
            "{} to {highest}",
            bars[1].y
        );
        assert!(bars[1].bottom() <= bars[0].y + 0.01);
    }

    #[test]
    fn a_line_chart_draws_one_line_a_series_and_no_bars() {
        let shapes = draw(
            &chart(ChartKind::Line, &[&[1.0, 2.0, 3.0], &[3.0, 2.0, 1.0]]),
            frame(),
            &Theme::house(),
            &Even::half(),
        );
        let paths: Vec<&Shape> = shapes
            .iter()
            .filter(|shape| matches!(shape, Shape::Path { filled: false, .. }))
            .collect();
        assert_eq!(paths.len(), 2);
        let Shape::Path { points, .. } = paths[0] else {
            panic!()
        };
        assert_eq!(points.len(), 3);
        let area = draw(
            &chart(ChartKind::Area, &[&[1.0, 2.0, 3.0]]),
            frame(),
            &Theme::house(),
            &Even::half(),
        );
        assert!(
            area.iter()
                .any(|shape| matches!(shape, Shape::Path { filled: true, .. }))
        );
    }

    #[test]
    fn a_pie_turns_once_all_the_way_round() {
        let shapes = draw(
            &chart(ChartKind::Pie, &[&[1.0, 2.0, 1.0]]),
            frame(),
            &Theme::house(),
            &Even::half(),
        );
        let wedges: Vec<(f32, f32)> = shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Wedge { from, to, .. } => Some((*from, *to)),
                _ => None,
            })
            .collect();
        assert_eq!(wedges.len(), 3);
        let swept: f32 = wedges.iter().map(|(from, to)| to - from).sum();
        assert!((swept - 360.0).abs() < 0.01, "swept {swept}");
        assert!((wedges[0].1 - wedges[0].0 - 90.0).abs() < 0.01);
        assert!((wedges[1].0 - wedges[0].1).abs() < 0.01);
    }

    #[test]
    fn a_pie_of_nothing_draws_nothing() {
        let shapes = draw(
            &chart(ChartKind::Pie, &[&[0.0, 0.0]]),
            frame(),
            &Theme::house(),
            &Even::half(),
        );
        assert!(shapes.is_empty());
        assert!(
            !draw(
                &chart(ChartKind::Pie, &[&[0.0, 1.0]]),
                frame(),
                &Theme::house(),
                &Even::half(),
            )
            .is_empty()
        );
    }

    #[test]
    fn a_chart_with_no_numbers_at_all_draws_nothing() {
        let bare = Chart {
            kind: ChartKind::Bar,
            ..Chart::default()
        };
        assert!(draw(&bare, frame(), &Theme::house(), &Even::half()).is_empty());
    }

    #[test]
    fn nothing_drawn_leaves_the_frame_the_page_gave_it() {
        let theme = Theme::house();
        let ruler = Even::half();
        for kind in ChartKind::ALL {
            let drawn = chart(kind, &[&[3.0, -1.0, 4.0], &[1.0, 5.0, 9.0]]);
            for shape in draw(&drawn, frame(), &theme, &ruler) {
                let inside = |spot: Spot| {
                    spot.x >= frame().x - 0.5
                        && spot.x <= frame().right() + 0.5
                        && spot.y >= frame().y - 0.5
                        && spot.y <= frame().bottom() + 0.5
                };
                match shape {
                    Shape::Fill { rect, .. } => {
                        assert!(inside(Spot::at(rect.x, rect.y)), "{kind:?} {rect:?}");
                        assert!(
                            inside(Spot::at(rect.right(), rect.bottom())),
                            "{kind:?} {rect:?}"
                        );
                    }
                    Shape::Stroke { from, to, .. } => {
                        assert!(inside(from) && inside(to), "{kind:?} {from:?} {to:?}");
                    }
                    Shape::Path { points, .. } => {
                        for point in points {
                            assert!(inside(point), "{kind:?} {point:?}");
                        }
                    }
                    Shape::Wedge { centre, radius, .. } => {
                        assert!(inside(Spot::at(centre.x - radius, centre.y - radius)));
                        assert!(inside(Spot::at(centre.x + radius, centre.y + radius)));
                    }
                    Shape::Words { .. } => {}
                }
            }
        }
    }

    #[test]
    fn two_series_get_two_colours_and_a_legend() {
        let theme = Theme::house();
        let shapes = draw(
            &chart(ChartKind::Bar, &[&[1.0], &[2.0]]),
            frame(),
            &theme,
            &Even::half(),
        );
        let names: Vec<String> = shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Words { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(names.contains(&"s0".to_owned()) && names.contains(&"s1".to_owned()));
        let bars: Vec<Rect> = fills(&shapes)
            .into_iter()
            .filter(|rect| rect.width > theme.axis.size)
            .collect();
        assert_eq!(bars.len(), 2);
        assert!(bars[0].x < bars[1].x, "the two bars stand side by side");
        let alone = draw(
            &chart(ChartKind::Bar, &[&[1.0]]),
            frame(),
            &theme,
            &Even::half(),
        );
        let said: Vec<String> = alone
            .iter()
            .filter_map(|shape| match shape {
                Shape::Words { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(!said.contains(&"s0".to_owned()));
    }

    #[test]
    fn a_label_too_wide_is_shortened_and_never_dropped_in_silence() {
        let ruler = Even::half();
        let pen = Pen::new(Face::Body, 10.0);
        assert_eq!(fit("abcdefghij", 30.0, pen, &ruler), "abcde\u{2026}");
        assert_eq!(fit("abc", 30.0, pen, &ruler), "abc");
        assert_eq!(fit("abc", 0.0, pen, &ruler), "");
    }
}
