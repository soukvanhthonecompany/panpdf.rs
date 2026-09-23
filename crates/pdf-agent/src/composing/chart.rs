use pdf_edit::PenStep;

use super::expr::Formula;
use super::theme::{Theme, darker, paler};
use super::{Colour, Composer, Style, shapes};
use crate::json::Json;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Line,
    Area,
    Bar,
    Scatter,
    Pie,
    Donut,
    Candles,
    Function,
}

#[derive(Clone, Debug, PartialEq)]
struct Series {
    name: String,
    values: Vec<f64>,
    points: Vec<(f64, f64)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Candle {
    open: f64,
    high: f64,
    low: f64,
    close: f64,
}

#[derive(Clone, Debug)]
struct Chart {
    kind: Kind,
    title: String,
    labels: Vec<String>,
    series: Vec<Series>,
    candles: Vec<Candle>,
    functions: Vec<(String, Formula)>,
    from: f64,
    to: f64,
    height: Option<f64>,
    deep: bool,
    x_title: String,
    y_title: String,
}

fn number(json: &Json) -> Option<f64> {
    match json {
        Json::Number(value) => value.is_finite().then_some(*value),
        Json::Text(text) => text.trim().replace(',', "").parse::<f64>().ok(),
        _ => None,
    }
}

fn label(json: &Json) -> String {
    match json {
        Json::Text(text) => text.clone(),
        Json::Number(value) => short(*value),
        Json::Bool(value) => value.to_string(),
        _ => String::new(),
    }
}

fn text_of(spec: &Json, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| spec.get(key).and_then(Json::as_str))
        .unwrap_or_default()
        .to_owned()
}

fn list_of<'a>(spec: &'a Json, keys: &[&str]) -> Option<&'a [Json]> {
    keys.iter()
        .find_map(|key| spec.get(key).and_then(Json::as_list))
}

fn numbers(list: &[Json], what: &str) -> Result<Vec<f64>, String> {
    list.iter()
        .enumerate()
        .map(|(at, value)| {
            number(value).ok_or_else(|| format!("{what} {} is not a number", at + 1))
        })
        .collect()
}

fn read(spec: &str) -> Result<Chart, String> {
    let json = Json::parse(spec.trim())
        .map_err(|why| format!("the chart is not JSON: {} at {}", why.reason, why.at))?;
    let kind_name = text_of(&json, &["type", "kind", "chart"]).to_ascii_lowercase();
    let kind = match kind_name.as_str() {
        "line" | "lines" => Kind::Line,
        "area" => Kind::Area,
        "bar" | "bars" | "column" | "columns" => Kind::Bar,
        "scatter" | "points" => Kind::Scatter,
        "pie" => Kind::Pie,
        "donut" | "doughnut" | "ring" => Kind::Donut,
        "candlestick" | "candles" | "ohlc" | "candle" => Kind::Candles,
        "function" | "functions" | "plot" => Kind::Function,
        "" => return Err("the chart has no \"type\"".to_owned()),
        other => {
            return Err(format!(
                "\"{other}\" is not a chart type: line, area, bar, scatter, pie, donut, candlestick or function"
            ));
        }
    };
    let labels: Vec<String> = list_of(&json, &["x", "labels", "categories"])
        .map(|list| list.iter().map(label).collect())
        .unwrap_or_default();
    let mut series = Vec::new();
    if let Some(list) = list_of(&json, &["series", "datasets"]) {
        for (at, one) in list.iter().enumerate() {
            let name = text_of(one, &["name", "label"]);
            let values = match list_of(one, &["values", "data", "y"]) {
                Some(values) if kind != Kind::Scatter => {
                    numbers(values, &format!("series {} value", at + 1))?
                }
                _ => Vec::new(),
            };
            let mut points = Vec::new();
            if let Some(list) = list_of(one, &["points", "data"]).filter(|_| kind == Kind::Scatter)
            {
                for (which, point) in list.iter().enumerate() {
                    let pair = point
                        .as_list()
                        .filter(|pair| pair.len() == 2)
                        .and_then(|pair| Some((number(&pair[0])?, number(&pair[1])?)))
                        .or_else(|| Some((number(point.get("x")?)?, number(point.get("y")?)?)))
                        .ok_or_else(|| {
                            format!("series {} point {} is not [x, y]", at + 1, which + 1)
                        })?;
                    points.push(pair);
                }
            }
            series.push(Series {
                name,
                values,
                points,
            });
        }
    } else if let Some(values) = list_of(&json, &["values", "data", "y"]) {
        if matches!(kind, Kind::Pie | Kind::Donut)
            && values.iter().all(|value| value.get("value").is_some())
        {
            let mut named = Vec::new();
            let mut pie_labels = Vec::new();
            for value in values {
                pie_labels.push(text_of(value, &["name", "label"]));
                named.push(number(value.get("value").unwrap_or(&Json::Null)).unwrap_or(0.0));
            }
            series.push(Series {
                name: String::new(),
                values: named,
                points: Vec::new(),
            });
            return finish(&json, kind, pie_labels, series);
        }
        series.push(Series {
            name: text_of(&json, &["name"]),
            values: numbers(values, "value")?,
            points: Vec::new(),
        });
    }
    finish(&json, kind, labels, series)
}

#[expect(
    clippy::too_many_lines,
    reason = "every kind's fields, read in one place"
)]
fn finish(
    json: &Json,
    kind: Kind,
    labels: Vec<String>,
    series: Vec<Series>,
) -> Result<Chart, String> {
    let mut candles = Vec::new();
    let mut labels = labels;
    if kind == Kind::Candles {
        if let Some(list) = list_of(json, &["candles", "ohlc", "data"]) {
            for (at, candle) in list.iter().enumerate() {
                let get = |key: &str, short: &str, index: usize| {
                    candle
                        .get(key)
                        .or_else(|| candle.get(short))
                        .or_else(|| candle.as_list().and_then(|list| list.get(index)))
                        .and_then(number)
                        .ok_or_else(|| format!("candle {} has no \"{key}\"", at + 1))
                };
                let offset = usize::from(candle.as_list().is_some_and(|list| list.len() >= 5));
                let one = Candle {
                    open: get("open", "o", offset)?,
                    high: get("high", "h", offset + 1)?,
                    low: get("low", "l", offset + 2)?,
                    close: get("close", "c", offset + 3)?,
                };
                if one.high < one.low {
                    return Err(format!("candle {} has its high below its low", at + 1));
                }
                if labels.len() <= at {
                    let x = candle
                        .get("x")
                        .or_else(|| candle.get("date"))
                        .or_else(|| candle.get("t"))
                        .or_else(|| {
                            candle
                                .as_list()
                                .filter(|list| list.len() >= 5)
                                .and_then(|list| list.first())
                        })
                        .map(label)
                        .unwrap_or_default();
                    labels.push(x);
                }
                candles.push(one);
            }
        } else {
            let column = |key: &str| {
                list_of(json, &[key])
                    .map(|list| numbers(list, key))
                    .transpose()
            };
            let (open, high, low, close) = (
                column("open")?,
                column("high")?,
                column("low")?,
                column("close")?,
            );
            let (Some(open), Some(high), Some(low), Some(close)) = (open, high, low, close) else {
                return Err("a candlestick chart needs \"candles\": [{\"open\", \"high\", \"low\", \"close\"}]".to_owned());
            };
            for at in 0..open.len().min(high.len()).min(low.len()).min(close.len()) {
                candles.push(Candle {
                    open: open[at],
                    high: high[at],
                    low: low[at],
                    close: close[at],
                });
            }
        }
        if candles.is_empty() {
            return Err("the candlestick chart has no candles".to_owned());
        }
    }
    let mut functions = Vec::new();
    if kind == Kind::Function {
        let written: Vec<(String, String)> =
            if let Some(list) = list_of(json, &["functions", "expressions", "f"]) {
                list.iter()
                    .map(|one| match one {
                        Json::Text(text) => (text.clone(), text.clone()),
                        other => (
                            text_of(other, &["name", "label"]),
                            text_of(other, &["expr", "expression", "f", "formula"]),
                        ),
                    })
                    .collect()
            } else {
                let one = text_of(json, &["expr", "expression", "function", "formula", "f"]);
                vec![(one.clone(), one)]
            };
        for (name, text) in written {
            if text.trim().is_empty() {
                continue;
            }
            let formula = Formula::read(&text).map_err(|why| format!("in \"{text}\": {why}"))?;
            let name = if name.trim().is_empty() {
                text.clone()
            } else {
                name
            };
            functions.push((name, formula));
        }
        if functions.is_empty() {
            return Err("a function chart needs \"functions\": [\"x^2\", ...]".to_owned());
        }
    }
    let from = json.get("from").and_then(number).unwrap_or(-5.0);
    let to = json.get("to").and_then(number).unwrap_or(5.0);
    if matches!(kind, Kind::Function) && to.partial_cmp(&from) != Some(std::cmp::Ordering::Greater)
    {
        return Err("\"to\" must be greater than \"from\"".to_owned());
    }
    let wanted = matches!(
        kind,
        Kind::Line | Kind::Area | Kind::Bar | Kind::Pie | Kind::Donut
    );
    if wanted && series.iter().all(|one| one.values.is_empty()) {
        return Err(
            "the chart has no values: \"series\": [{\"name\": ..., \"values\": [...]}]".to_owned(),
        );
    }
    if kind == Kind::Scatter && series.iter().all(|one| one.points.is_empty()) {
        return Err("a scatter chart needs \"series\": [{\"points\": [[x, y], ...]}]".to_owned());
    }
    let style = text_of(json, &["style", "look"]).to_ascii_lowercase();
    Ok(Chart {
        kind,
        title: text_of(json, &["title", "name"]),
        labels,
        series,
        candles,
        functions,
        from,
        to,
        height: json
            .get("height")
            .and_then(number)
            .map(|height| height.clamp(100.0, 640.0)),
        deep: style.contains("3d") || json.get("3d") == Some(&Json::Bool(true)),
        x_title: text_of(json, &["x_title", "x_label", "xlabel"]),
        y_title: text_of(json, &["y_title", "y_label", "ylabel"]),
    })
}

fn short(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    let magnitude = value.abs();
    if !(1e-3..1e6).contains(&magnitude) {
        return format!("{value:.2e}");
    }
    let text = format!("{value:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    text.replace('-', "−")
}

fn ticks(low: f64, high: f64, count: f64) -> Vec<f64> {
    let (low, high) = if (high - low).abs() < 1e-12 {
        (low - 1.0, high + 1.0)
    } else {
        (low, high)
    };
    let rough = (high - low) / count.max(1.0);
    let power = 10_f64.powf(rough.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0]
        .iter()
        .map(|m| m * power)
        .find(|step| *step >= rough)
        .unwrap_or(power * 10.0);
    let first = (low / step).floor() * step;
    let mut out = Vec::new();
    let mut at = first;
    while at <= high + step * 1e-9 || out.len() < 2 {
        out.push(if at.abs() < step * 1e-9 { 0.0 } else { at });
        at += step;
        if out.len() > 40 {
            break;
        }
    }
    if out.last().is_some_and(|last| *last < high - step * 1e-9) {
        out.push(at);
    }
    out
}

fn mix(a: Colour, b: Colour, t: f64) -> Colour {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

pub(crate) fn set_out(
    composer: &mut Composer<'_>,
    spec: &str,
    space_before: f64,
    indent: f64,
) -> Result<(), String> {
    let chart = read(spec)?;
    let theme = *composer.theme();
    let body = composer.setting.body;
    let left = composer.left() + indent;
    let right = composer.right();
    let width = right - left;
    let height = chart.height.unwrap_or((width * 0.58).min(300.0)).max(120.0);
    let raise = if theme.dressed { 3.0 } else { 0.0 };
    let top = composer.place(space_before, height + raise);
    let card = if theme.paper.is_some() {
        theme.soft
    } else {
        [1.0, 1.0, 1.0]
    };
    if theme.dressed {
        composer.shape(
            shapes::rounded(
                left + raise,
                top + raise,
                right + raise,
                top + height + raise,
                8.0,
            ),
            None,
            Some(theme.shadow),
        );
    }
    composer.shape(
        shapes::rounded(left, top, right, top + height, 8.0),
        Some((theme.line, 0.7)),
        Some(card),
    );
    let muted = mix(theme.ink, card, 0.35);
    let pad = 12.0;
    let mut drawing = Drawing {
        composer,
        theme,
        card,
        muted,
        small: (body * 0.78).max(6.5),
        family: String::new(),
    };
    drawing.family = drawing.composer.setting.family.to_owned();
    let mut inner_top = top + pad;
    if !chart.title.trim().is_empty() {
        let style = Style {
            family: drawing.family.clone(),
            size: body * 1.05,
            bold: true,
            italic: false,
            colour: Some(if theme.dressed {
                theme.accent
            } else {
                theme.ink
            }),
        };
        let fitted = drawing
            .composer
            .fit(&chart.title, &style, width - 2.0 * pad)?;
        let h = fitted.room.height;
        drawing
            .composer
            .text((left + pad, inner_top), width - 2.0 * pad, fitted);
        inner_top += h + 6.0;
    }
    let area = [left + pad, inner_top, right - pad, top + height - pad];
    match chart.kind {
        Kind::Pie | Kind::Donut => drawing.pie(&chart, area)?,
        _ => drawing.plot(&chart, area)?,
    }
    drawing.composer.top = top + height + raise;
    Ok(())
}

struct Drawing<'a, 'b> {
    composer: &'a mut Composer<'b>,
    theme: Theme,
    card: Colour,
    muted: Colour,
    small: f64,
    family: String,
}

impl Drawing<'_, '_> {
    fn colour(&self, at: usize) -> Colour {
        self.theme.palette[at % self.theme.palette.len()]
    }

    fn style(&self, size: f64, colour: Colour, bold: bool) -> Style {
        Style {
            family: self.family.clone(),
            size,
            bold,
            italic: false,
            colour: Some(colour),
        }
    }

    fn label(
        &mut self,
        text: &str,
        (x, top): (f64, f64),
        anchor: f64,
        colour: Colour,
        size: f64,
    ) -> Result<f64, String> {
        if text.trim().is_empty() {
            return Ok(0.0);
        }
        let fitted = self
            .composer
            .fit(text, &self.style(size, colour, false), 400.0)?;
        let wide = fitted.room.widest;
        self.composer.text((x - wide * anchor, top), wide, fitted);
        Ok(wide)
    }

    fn measure(&self, text: &str, size: f64) -> Result<f64, String> {
        if text.trim().is_empty() {
            return Ok(0.0);
        }
        Ok(self
            .composer
            .fit(text, &self.style(size, self.muted, false), 400.0)?
            .room
            .widest)
    }

    fn legend(&mut self, names: &[(String, Colour)], area: [f64; 4]) -> Result<f64, String> {
        let names: Vec<&(String, Colour)> = names
            .iter()
            .filter(|(name, _)| !name.trim().is_empty())
            .collect();
        if names.len() < 2 {
            return Ok(0.0);
        }
        let size = self.small;
        let line = size * 1.4;
        let mut widths = Vec::new();
        for (name, _) in &names {
            widths.push(self.measure(name, size)?);
        }
        let mut rows: Vec<Vec<usize>> = vec![Vec::new()];
        let mut used = 0.0;
        let room = area[2] - area[0];
        for (at, wide) in widths.iter().enumerate() {
            let entry = size + 4.0 + wide + 14.0;
            if used + entry > room && !rows.last().is_none_or(Vec::is_empty) {
                rows.push(Vec::new());
                used = 0.0;
            }
            used += entry;
            if let Some(row) = rows.last_mut() {
                row.push(at);
            }
        }
        let height = line * f64::from(u32::try_from(rows.len()).unwrap_or(1));
        let mut y = area[3] - height;
        for row in rows {
            let total: f64 = row
                .iter()
                .map(|&at| size + 4.0 + widths[at] + 14.0)
                .sum::<f64>()
                - 14.0;
            let mut x = area[0] + (room - total) / 2.0;
            for at in row {
                let (name, colour) = names[at];
                self.composer.shape(
                    shapes::rounded(x, y + size * 0.2, x + size, y + size * 1.2, 2.0),
                    None,
                    Some(*colour),
                );
                x += size + 4.0;
                self.label(name, (x, y), 0.0, self.muted, size)?;
                x += widths[at] + 14.0;
            }
            y += line;
        }
        Ok(height + 4.0)
    }

    #[expect(clippy::too_many_lines, reason = "one chart's frame, drawn in order")]
    fn plot(&mut self, chart: &Chart, area: [f64; 4]) -> Result<(), String> {
        let size = self.small;
        let mut samples: Vec<Vec<(f64, f64)>> = Vec::new();
        let (x_low, x_high, mut y_low, mut y_high);
        match chart.kind {
            Kind::Function => {
                x_low = chart.from;
                x_high = chart.to;
                let mut all = Vec::new();
                for (_, formula) in &chart.functions {
                    let mut points = Vec::new();
                    for step in 0..=300 {
                        let x = x_low + (x_high - x_low) * f64::from(step) / 300.0;
                        let y = formula.at(x);
                        points.push((x, y));
                        if y.is_finite() {
                            all.push(y);
                        }
                    }
                    samples.push(points);
                }
                if all.is_empty() {
                    return Err(
                        "the function has no value anywhere from \"from\" to \"to\"".to_owned()
                    );
                }
                all.sort_by(f64::total_cmp);
                let at = |share: f64| {
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        clippy::cast_precision_loss,
                        reason = "an index into a short list"
                    )]
                    let index = ((all.len() - 1) as f64 * share).round() as usize;
                    all[index]
                };
                let (low, high) = (at(0.02), at(0.98));
                let spare = (high - low).abs().max(1e-9) * 0.08;
                y_low = low - spare;
                y_high = high + spare;
            }
            Kind::Scatter => {
                let every: Vec<(f64, f64)> = chart
                    .series
                    .iter()
                    .flat_map(|one| one.points.iter().copied())
                    .collect();
                x_low = every.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
                x_high = every.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
                y_low = every.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
                y_high = every.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
            }
            Kind::Candles => {
                x_low = 0.0;
                x_high = 0.0;
                y_low = chart
                    .candles
                    .iter()
                    .map(|c| c.low)
                    .fold(f64::INFINITY, f64::min);
                y_high = chart
                    .candles
                    .iter()
                    .map(|c| c.high)
                    .fold(f64::NEG_INFINITY, f64::max);
                for one in &chart.series {
                    for value in &one.values {
                        y_low = y_low.min(*value);
                        y_high = y_high.max(*value);
                    }
                }
            }
            _ => {
                x_low = 0.0;
                x_high = 0.0;
                let every: Vec<f64> = chart
                    .series
                    .iter()
                    .flat_map(|one| one.values.iter().copied())
                    .collect();
                y_low = every.iter().copied().fold(f64::INFINITY, f64::min);
                y_high = every.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                if matches!(chart.kind, Kind::Bar | Kind::Area) {
                    y_low = y_low.min(0.0);
                    y_high = y_high.max(0.0);
                }
            }
        }
        let y_ticks = ticks(y_low, y_high, 5.0);
        let (y_low, y_high) = (y_ticks[0], y_ticks[y_ticks.len() - 1]);
        let continuous = matches!(chart.kind, Kind::Function | Kind::Scatter);
        let x_ticks = if continuous {
            ticks(x_low, x_high, 6.0)
        } else {
            Vec::new()
        };
        let (x_low, x_high) = if continuous {
            (x_ticks[0], x_ticks[x_ticks.len() - 1])
        } else {
            (x_low, x_high)
        };

        let names: Vec<(String, Colour)> = match chart.kind {
            Kind::Function => chart
                .functions
                .iter()
                .enumerate()
                .map(|(at, (name, _))| (name.clone(), self.colour(at)))
                .collect(),
            _ => chart
                .series
                .iter()
                .enumerate()
                .map(|(at, one)| {
                    (
                        one.name.clone(),
                        self.colour(at + usize::from(chart.kind == Kind::Candles)),
                    )
                })
                .collect(),
        };
        let legend = self.legend(&names, area)?;
        let mut label_width: f64 = 0.0;
        let y_labels: Vec<String> = y_ticks.iter().map(|tick| short(*tick)).collect();
        for text in &y_labels {
            label_width = label_width.max(self.measure(text, size)?);
        }
        let y_title_room = if chart.y_title.trim().is_empty() {
            0.0
        } else {
            size * 1.5
        };
        let x_title_room = if chart.x_title.trim().is_empty() {
            0.0
        } else {
            size * 1.5
        };
        let plot = [
            area[0] + label_width + 6.0 + y_title_room,
            area[1] + size * 0.6,
            area[2] - 4.0,
            area[3] - legend - size * 1.6 - x_title_room,
        ];
        if plot[2] - plot[0] < 40.0 || plot[3] - plot[1] < 30.0 {
            return Err(
                "the chart is too small for its labels: give it more \"height\"".to_owned(),
            );
        }
        let y_of = |value: f64| plot[3] - (value - y_low) / (y_high - y_low) * (plot[3] - plot[1]);
        let x_of = |value: f64| plot[0] + (value - x_low) / (x_high - x_low) * (plot[2] - plot[0]);

        let grid_colour = mix(self.theme.line, self.card, 0.35);
        let mut grid = Vec::new();
        for tick in &y_ticks {
            let y = y_of(*tick);
            grid.extend(shapes::line(plot[0], y, plot[2], y));
        }
        for tick in &x_ticks {
            let x = x_of(*tick);
            grid.extend(shapes::line(x, plot[1], x, plot[3]));
        }
        self.composer.shape(grid, Some((grid_colour, 0.5)), None);
        let axis_colour = mix(self.theme.ink, self.card, 0.55);
        let zero_y = if (y_low..=y_high).contains(&0.0) {
            y_of(0.0)
        } else {
            plot[3]
        };
        let mut axes = shapes::line(plot[0], zero_y, plot[2], zero_y);
        if continuous && (x_low..=x_high).contains(&0.0) {
            let x = x_of(0.0);
            axes.extend(shapes::line(x, plot[1], x, plot[3]));
        } else {
            axes.extend(shapes::line(plot[0], plot[1], plot[0], plot[3]));
        }
        self.composer.shape(axes, Some((axis_colour, 0.9)), None);
        for (tick, text) in y_ticks.iter().zip(&y_labels) {
            let y = y_of(*tick) - size * 0.6;
            self.label(text, (plot[0] - 5.0, y), 1.0, self.muted, size)?;
        }
        for tick in &x_ticks {
            self.label(
                &short(*tick),
                (x_of(*tick), plot[3] + 3.0),
                0.5,
                self.muted,
                size,
            )?;
        }
        if !chart.y_title.trim().is_empty() {
            self.label(
                &chart.y_title,
                (area[0], plot[1] - size * 1.5),
                0.0,
                self.muted,
                size,
            )?;
        }
        if !chart.x_title.trim().is_empty() {
            self.label(
                &chart.x_title,
                (f64::midpoint(plot[0], plot[2]), plot[3] + size * 1.6),
                0.5,
                self.muted,
                size,
            )?;
        }

        let count = match chart.kind {
            Kind::Candles => chart.candles.len(),
            _ => chart
                .series
                .iter()
                .map(|one| one.values.len())
                .max()
                .unwrap_or(0),
        };
        let slots = f64::from(u32::try_from(count.max(1)).unwrap_or(1));
        let slot = (plot[2] - plot[0]) / slots;
        let centre = |at: usize| plot[0] + slot * (f64::from(u32::try_from(at).unwrap_or(0)) + 0.5);
        if !continuous && count > 0 {
            let mut widest: f64 = 0.0;
            for text in chart.labels.iter().take(count) {
                widest = widest.max(self.measure(text, size)?);
            }
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a small step count"
            )]
            let every = ((widest + 6.0) / slot).ceil().max(1.0) as usize;
            for (at, text) in chart.labels.iter().take(count).enumerate() {
                if at % every == 0 {
                    self.label(text, (centre(at), plot[3] + 3.0), 0.5, self.muted, size)?;
                }
            }
        }

        match chart.kind {
            Kind::Function => {
                for (at, points) in samples.iter().enumerate() {
                    let mut steps = Vec::new();
                    let mut last: Option<(f64, f64)> = None;
                    for &(x, y) in points {
                        let inside = y.is_finite()
                            && y >= y_low - (y_high - y_low)
                            && y <= y_high + (y_high - y_low);
                        if !inside {
                            last = None;
                            continue;
                        }
                        let point = (x_of(x), y_of(y.clamp(y_low, y_high)));
                        let jump =
                            last.is_some_and(|(_, was)| (was - y).abs() > (y_high - y_low) * 0.9);
                        if last.is_none() || jump {
                            steps.push(PenStep::Move(point));
                        } else {
                            steps.push(PenStep::Line(point));
                        }
                        last = Some((x, y));
                    }
                    self.composer
                        .shape(steps, Some((self.colour(at), 1.8)), None);
                }
            }
            Kind::Scatter => {
                for (at, one) in chart.series.iter().enumerate() {
                    let mut dots = Vec::new();
                    for &(x, y) in &one.points {
                        dots.extend(shapes::circle(x_of(x), y_of(y), 2.8));
                    }
                    self.composer
                        .shape(dots, Some((self.card, 0.8)), Some(self.colour(at)));
                }
            }
            Kind::Line | Kind::Area => {
                for (at, one) in chart.series.iter().enumerate() {
                    let colour = self.colour(at);
                    let points: Vec<(f64, f64)> = one
                        .values
                        .iter()
                        .enumerate()
                        .map(|(i, v)| (centre(i), y_of(*v)))
                        .collect();
                    if chart.kind == Kind::Area && points.len() > 1 {
                        let mut region = points.clone();
                        region.push((points[points.len() - 1].0, zero_y));
                        region.push((points[0].0, zero_y));
                        let fill = if chart.series.len() > 1 {
                            paler(colour, 0.7)
                        } else {
                            paler(colour, 0.6)
                        };
                        self.composer
                            .shape(shapes::polygon(&region), None, Some(fill));
                    }
                    self.composer
                        .shape(shapes::polyline(&points), Some((colour, 2.0)), None);
                    if points.len() <= 40 {
                        let mut dots = Vec::new();
                        for &(x, y) in &points {
                            dots.extend(shapes::circle(x, y, 2.6));
                        }
                        self.composer
                            .shape(dots, Some((self.card, 1.0)), Some(colour));
                    }
                }
            }
            Kind::Bar => self.bars(chart, &plot, slot, zero_y, &y_of)?,
            Kind::Candles => {
                let body = (slot * 0.6).min(18.0);
                let mut wicks_up = Vec::new();
                let mut wicks_down = Vec::new();
                let mut up = Vec::new();
                let mut down = Vec::new();
                for (at, candle) in chart.candles.iter().enumerate() {
                    let x = centre(at);
                    let rising = candle.close >= candle.open;
                    let wick = shapes::line(x, y_of(candle.high), x, y_of(candle.low));
                    let (a, b) = (y_of(candle.open), y_of(candle.close));
                    let (top, bottom) = (a.min(b), a.max(b).max(a.min(b) + 0.8));
                    let rect = shapes::rect(x - body / 2.0, top, x + body / 2.0, bottom);
                    if rising {
                        wicks_up.extend(wick);
                        up.extend(rect);
                    } else {
                        wicks_down.extend(wick);
                        down.extend(rect);
                    }
                }
                let (rise, fall) = (self.theme.rise, self.theme.fall);
                self.composer
                    .shape(wicks_up, Some((darker(rise, 0.15), 1.0)), None);
                self.composer
                    .shape(wicks_down, Some((darker(fall, 0.15), 1.0)), None);
                self.composer
                    .shape(up, Some((darker(rise, 0.2), 0.6)), Some(rise));
                self.composer
                    .shape(down, Some((darker(fall, 0.2), 0.6)), Some(fall));
                for (at, one) in chart.series.iter().enumerate() {
                    let points: Vec<(f64, f64)> = one
                        .values
                        .iter()
                        .enumerate()
                        .map(|(i, v)| (centre(i), y_of(*v)))
                        .collect();
                    self.composer.shape(
                        shapes::polyline(&points),
                        Some((self.colour(at + 1), 1.6)),
                        None,
                    );
                }
            }
            Kind::Pie | Kind::Donut => {}
        }
        Ok(())
    }

    fn bars(
        &mut self,
        chart: &Chart,
        plot: &[f64; 4],
        slot: f64,
        zero_y: f64,
        y_of: &dyn Fn(f64) -> f64,
    ) -> Result<(), String> {
        let groups = chart.series.len().max(1);
        let group = slot * 0.72;
        let wide = group / f64::from(u32::try_from(groups).unwrap_or(1));
        let depth = if chart.deep {
            (wide * 0.35).clamp(3.0, 10.0)
        } else {
            0.0
        };
        let count: usize = chart.series.iter().map(|one| one.values.len()).sum();
        for (which, one) in chart.series.iter().enumerate() {
            let colour = self.colour(which);
            let (mut fronts, mut tops, mut sides) = (Vec::new(), Vec::new(), Vec::new());
            for (at, value) in one.values.iter().enumerate() {
                let slot_left = plot[0]
                    + slot * f64::from(u32::try_from(at).unwrap_or(0))
                    + (slot - group) / 2.0;
                let x0 = slot_left + wide * f64::from(u32::try_from(which).unwrap_or(0));
                let x1 = x0 + wide - depth - 1.0;
                let y = y_of(*value);
                let (top, bottom) = (y.min(zero_y), y.max(zero_y));
                fronts.extend(shapes::rounded(
                    x0,
                    top,
                    x1,
                    bottom,
                    if depth > 0.0 { 0.0 } else { 1.5 },
                ));
                if depth > 0.0 {
                    let d = depth;
                    tops.extend(shapes::polygon(&[
                        (x0, top),
                        (x0 + d, top - d * 0.7),
                        (x1 + d, top - d * 0.7),
                        (x1, top),
                    ]));
                    sides.extend(shapes::polygon(&[
                        (x1, top),
                        (x1 + d, top - d * 0.7),
                        (x1 + d, bottom - d * 0.7),
                        (x1, bottom),
                    ]));
                }
                if count <= 16 {
                    let text = short(*value);
                    let y_label = if *value >= 0.0 {
                        top - depth * 0.7 - self.small * 1.35
                    } else {
                        bottom + 2.0
                    };
                    self.label(
                        &text,
                        (f64::midpoint(x0, x1), y_label),
                        0.5,
                        self.muted,
                        self.small * 0.92,
                    )?;
                }
            }
            if depth > 0.0 {
                self.composer.shape(sides, None, Some(darker(colour, 0.28)));
                self.composer.shape(tops, None, Some(paler(colour, 0.35)));
            }
            self.composer.shape(fronts, None, Some(colour));
        }
        Ok(())
    }

    #[expect(clippy::too_many_lines, reason = "one pie, drawn back to front")]
    fn pie(&mut self, chart: &Chart, area: [f64; 4]) -> Result<(), String> {
        let Some(one) = chart.series.first() else {
            return Ok(());
        };
        let values: Vec<f64> = one.values.iter().map(|v| v.max(0.0)).collect();
        let total: f64 = values.iter().sum();
        if total <= 0.0 {
            return Err("a pie's values add up to nothing".to_owned());
        }
        let size = self.small;
        let names: Vec<String> = (0..values.len())
            .map(|at| {
                let name = chart
                    .labels
                    .get(at)
                    .cloned()
                    .unwrap_or_else(|| format!("{}", at + 1));
                format!(
                    "{name}  {}%",
                    short((values[at] / total * 1000.0).round() / 10.0)
                )
            })
            .collect();
        let mut widest: f64 = 0.0;
        for name in &names {
            widest = widest.max(self.measure(name, size)?);
        }
        let legend_wide = (widest + size + 10.0).min((area[2] - area[0]) * 0.45);
        let pie_area = [area[0], area[1], area[2] - legend_wide - 10.0, area[3]];
        let squash = if chart.deep { 0.62 } else { 1.0 };
        let depth = if chart.deep {
            (pie_area[3] - pie_area[1]) * 0.1
        } else {
            0.0
        };
        let r = ((pie_area[2] - pie_area[0]) / 2.0)
            .min((pie_area[3] - pie_area[1] - depth) / 2.0 / squash)
            .max(10.0);
        let (cx, cy) = (
            f64::midpoint(pie_area[0], pie_area[2]),
            pie_area[1] + (pie_area[3] - pie_area[1] - depth) / 2.0,
        );
        let start = -std::f64::consts::FRAC_PI_2;
        let mut angle = start;
        let mut slices = Vec::new();
        for (at, value) in values.iter().enumerate() {
            let sweep = value / total * std::f64::consts::TAU;
            slices.push((at, angle, angle + sweep));
            angle += sweep;
        }
        let inner = if chart.kind == Kind::Donut {
            r * 0.55
        } else {
            0.0
        };
        if chart.deep {
            for &(at, from, to) in &slices {
                let (a, b) = (
                    from.clamp(0.0, std::f64::consts::PI),
                    to.clamp(0.0, std::f64::consts::PI),
                );
                if b - a < 1e-6 {
                    continue;
                }
                let mut steps = vec![PenStep::Move((cx + r * a.cos(), cy + r * squash * a.sin()))];
                steps.extend(shapes::arc_steps(cx, cy, (r, r * squash), a, b));
                steps.push(PenStep::Line((
                    cx + r * b.cos(),
                    cy + r * squash * b.sin() + depth,
                )));
                steps.extend(shapes::arc_steps(cx, cy + depth, (r, r * squash), b, a));
                steps.push(PenStep::Line((cx + r * a.cos(), cy + r * squash * a.sin())));
                self.composer
                    .shape(steps, None, Some(darker(self.colour(at), 0.3)));
            }
        }
        for &(at, from, to) in &slices {
            let mut steps = if inner > 0.0 {
                let mut steps = vec![PenStep::Move((
                    cx + r * from.cos(),
                    cy + r * squash * from.sin(),
                ))];
                steps.extend(shapes::arc_steps(cx, cy, (r, r * squash), from, to));
                steps.push(PenStep::Line((
                    cx + inner * to.cos(),
                    cy + inner * squash * to.sin(),
                )));
                steps.extend(shapes::arc_steps(cx, cy, (inner, inner * squash), to, from));
                steps
            } else {
                let mut steps = vec![
                    PenStep::Move((cx, cy)),
                    PenStep::Line((cx + r * from.cos(), cy + r * squash * from.sin())),
                ];
                steps.extend(shapes::arc_steps(cx, cy, (r, r * squash), from, to));
                steps
            };
            steps.push(match steps[0] {
                PenStep::Move(point) => PenStep::Line(point),
                other => other,
            });
            self.composer
                .shape(steps, Some((self.card, 1.2)), Some(self.colour(at)));
        }
        for &(_, from, to) in &slices {
            let share = (to - from) / std::f64::consts::TAU;
            if share < 0.06 {
                continue;
            }
            let mid = f64::midpoint(from, to);
            let reach = if inner > 0.0 {
                f64::midpoint(r, inner)
            } else {
                r * 0.62
            };
            let (x, y) = (cx + reach * mid.cos(), cy + reach * squash * mid.sin());
            let text = format!("{}%", short((share * 1000.0).round() / 10.0));
            let white = if self.theme.paper.is_some() {
                self.theme.on_accent
            } else {
                [1.0, 1.0, 1.0]
            };
            self.label(&text, (x, y - size * 0.7), 0.5, white, size)?;
        }
        if inner > 0.0 && !one.name.trim().is_empty() {
            self.label(&one.name, (cx, cy - size * 0.7), 0.5, self.theme.ink, size)?;
        }
        let line = size * 1.6;
        let count = f64::from(u32::try_from(names.len()).unwrap_or(1));
        let mut y = f64::midpoint(area[1], area[3]) - line * count / 2.0;
        let x = area[2] - legend_wide;
        for (at, name) in names.iter().enumerate() {
            self.composer.shape(
                shapes::rounded(x, y + size * 0.2, x + size, y + size * 1.2, 2.0),
                None,
                Some(self.colour(at)),
            );
            self.label(name, (x + size + 6.0, y), 0.0, self.theme.ink, size)?;
            y += line;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axes_get_round_ticks() {
        assert_eq!(ticks(0.0, 6.3, 5.0), vec![0.0, 2.0, 4.0, 6.0, 8.0]);
        assert_eq!(
            ticks(-1.2, 1.2, 5.0),
            vec![-1.5, -1.0, -0.5, 0.0, 0.5, 1.0, 1.5]
        );
        assert_eq!(short(2.5), "2.5");
        assert_eq!(short(-3.0), "−3");
        assert_eq!(short(100.0), "100");
    }

    #[test]
    fn charts_are_read_or_refused_by_name() {
        let bar = read(
            r#"{"type":"bar","x":["a","b"],"series":[{"name":"s","values":[1,2]}],"style":"3d"}"#,
        )
        .expect("a bar chart");
        assert_eq!(bar.kind, Kind::Bar);
        assert!(bar.deep);
        assert_eq!(bar.series[0].values, vec![1.0, 2.0]);
        let candles = read(r#"{"type":"candlestick","candles":[{"x":"Mon","open":1,"high":3,"low":0.5,"close":2}]}"#).expect("candles");
        assert_eq!(candles.candles.len(), 1);
        assert_eq!(candles.labels, vec!["Mon".to_owned()]);
        let pie =
            read(r#"{"type":"pie","values":[{"name":"A","value":3},{"name":"B","value":1}]}"#)
                .expect("a pie");
        assert_eq!(pie.labels, vec!["A".to_owned(), "B".to_owned()]);
        let function = read(r#"{"type":"function","functions":["exp(-x^2)"],"from":-3,"to":3}"#)
            .expect("a function");
        assert_eq!(function.functions.len(), 1);
        assert!(read(r#"{"type":"radar"}"#).unwrap_err().contains("radar"));
        assert!(
            read(r#"{"type":"bar","series":[{"values":[1,"x"]}]}"#)
                .unwrap_err()
                .contains("value 2")
        );
        assert!(
            read(r#"{"type":"function","functions":["foo(x)"]}"#)
                .unwrap_err()
                .contains("foo")
        );
        assert!(read("not json").unwrap_err().contains("JSON"));
    }
}
