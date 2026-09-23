#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ChartKind {
    #[default]
    Bar,
    StackedBar,
    BarAcross,
    Line,
    Area,
    Pie,
}

impl ChartKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bar => "bar",
            Self::StackedBar => "stacked-bar",
            Self::BarAcross => "bar-across",
            Self::Line => "line",
            Self::Area => "area",
            Self::Pie => "pie",
        }
    }

    pub const ALL: [Self; 6] = [
        Self::Bar,
        Self::StackedBar,
        Self::BarAcross,
        Self::Line,
        Self::Area,
        Self::Pie,
    ];

    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        let plain = word.trim().to_ascii_lowercase().replace([' ', '_'], "-");
        match plain.as_str() {
            "column" | "columns" | "bars" => Some(Self::Bar),
            "stacked" | "stacked-bars" | "stacked-column" => Some(Self::StackedBar),
            "horizontal-bar" | "hbar" => Some(Self::BarAcross),
            "donut" | "doughnut" => Some(Self::Pie),
            other => Self::ALL.into_iter().find(|it| it.as_str() == other),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Series {
    pub name: String,
    pub values: Vec<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chart {
    pub kind: ChartKind,
    pub title: Option<String>,
    pub labels: Vec<String>,
    pub series: Vec<Series>,
    pub unit: Option<String>,
    pub note: Option<String>,
}

impl Chart {
    #[must_use]
    pub fn reach(&self) -> (f64, f64) {
        let mut low = 0.0_f64;
        let mut high = 0.0_f64;
        for series in &self.series {
            for value in &series.values {
                if value.is_finite() {
                    low = low.min(*value);
                    high = high.max(*value);
                }
            }
        }
        (low, high)
    }

    #[must_use]
    pub fn stacked_reach(&self) -> (f64, f64) {
        let mut low = 0.0_f64;
        let mut high = 0.0_f64;
        for at in 0..self.labels.len() {
            let (mut up, mut down) = (0.0_f64, 0.0_f64);
            for series in &self.series {
                match series.values.get(at) {
                    Some(value) if value.is_finite() && *value >= 0.0 => up += value,
                    Some(value) if value.is_finite() => down += value,
                    _ => {}
                }
            }
            high = high.max(up);
            low = low.min(down);
        }
        (low, high)
    }

    pub(crate) fn words_into(&self, out: &mut String) {
        if let Some(title) = &self.title {
            out.push_str(title);
            out.push('\n');
        }
        for series in &self.series {
            out.push_str(&series.name);
            out.push('\n');
        }
        if let Some(note) = &self.note {
            out.push_str(note);
            out.push('\n');
        }
    }
}

use crate::value::{Value, parse};

pub fn read(text: &str) -> Result<Chart, String> {
    let value = parse(text)?;
    let kind = kind_of(&value)?;
    let labels = labels_of(&value);
    let series = series_of(&value, &labels)?;
    if series.is_empty() {
        return Err("this chart has no numbers in it".to_owned());
    }
    Ok(Chart {
        kind,
        title: word_at(&value, "title"),
        labels,
        series,
        unit: word_at(&value, "unit"),
        note: word_at(&value, "note"),
    })
}

fn word_at(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_text)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
}

fn kind_of(value: &Value) -> Result<ChartKind, String> {
    let Some(written) = value.get("kind").or_else(|| value.get("type")) else {
        return Err(format!("this chart does not say its kind; {}", offered()));
    };
    let word = written
        .as_text()
        .ok_or_else(|| format!("a chart's kind is a word; {}", offered()))?;
    ChartKind::parse(word).ok_or_else(|| format!("`{word}` is not a kind of chart; {}", offered()))
}

fn offered() -> String {
    let names: Vec<&str> = ChartKind::ALL.iter().map(|it| it.as_str()).collect();
    format!("it is one of {}", names.join(", "))
}

fn labels_of(value: &Value) -> Vec<String> {
    let Some(list) = value.get("labels").and_then(Value::as_list) else {
        return Vec::new();
    };
    list.iter()
        .map(|entry| match entry {
            Value::Text(text) => text.clone(),
            Value::Number(number) => trimmed(*number),
            other => format!("{other:?}"),
        })
        .collect()
}

fn trimmed(number: f64) -> String {
    if number.fract().abs() < f64::EPSILON && number.abs() < 1e15 {
        format!("{number:.0}")
    } else {
        format!("{number}")
    }
}

fn series_of(value: &Value, labels: &[String]) -> Result<Vec<Series>, String> {
    let written = match value.get("series") {
        Some(Value::List(list)) => list.clone(),
        Some(_) => return Err("`series` is a list of runs of numbers".to_owned()),
        None => vec![value.clone()],
    };
    let mut out = Vec::new();
    for (at, entry) in written.iter().enumerate() {
        let Some(values) = entry.get("values").and_then(Value::as_list) else {
            if entry.get("values").is_some() {
                return Err(format!(
                    "series {} has a `values` that is not a list",
                    at + 1
                ));
            }
            continue;
        };
        out.push(one_series(entry, values, labels, at)?);
    }
    Ok(out)
}

fn one_series(
    entry: &Value,
    values: &[Value],
    labels: &[String],
    at: usize,
) -> Result<Series, String> {
    let name = word_at(entry, "name").unwrap_or_default();
    let called = if name.is_empty() {
        format!("series {}", at + 1)
    } else {
        format!("`{name}`")
    };
    if !labels.is_empty() && values.len() > labels.len() {
        return Err(format!(
            "{called} has {} numbers but there are only {} labels",
            values.len(),
            labels.len()
        ));
    }
    let mut numbers = Vec::with_capacity(values.len());
    for (which, value) in values.iter().enumerate() {
        let number = value
            .as_number()
            .ok_or_else(|| format!("number {} of {called} is not a number", which + 1))?;
        if !number.is_finite() {
            return Err(format!("number {} of {called} is not a number", which + 1));
        }
        numbers.push(number);
    }
    Ok(Series {
        name,
        values: numbers,
    })
}

#[cfg(test)]
mod tests {
    use super::{Chart, ChartKind, read};

    #[test]
    fn a_chart_reads_back() {
        let chart = read(
            r#"{"kind":"line","title":"T","unit":"%","note":"n",
                "labels":["a","b"],
                "series":[{"name":"one","values":[1,2]},{"name":"two","values":["3",4]}]}"#,
        )
        .expect("read");
        assert_eq!(chart.kind, ChartKind::Line);
        assert_eq!(chart.title.as_deref(), Some("T"));
        assert_eq!(chart.unit.as_deref(), Some("%"));
        assert_eq!(chart.note.as_deref(), Some("n"));
        assert_eq!(chart.labels, vec!["a", "b"]);
        assert_eq!(chart.series[1].values, vec![3.0, 4.0]);
    }

    #[test]
    fn values_alone_are_one_series() {
        let chart = read(r#"{"kind":"pie","labels":["a","b"],"values":[1,2]}"#).expect("read");
        assert_eq!(chart.series.len(), 1);
        assert_eq!(chart.series[0].values, vec![1.0, 2.0]);
    }

    #[test]
    fn the_kinds_a_model_writes_are_understood() {
        for (word, kind) in [
            ("bar", ChartKind::Bar),
            ("column", ChartKind::Bar),
            ("Stacked Bar", ChartKind::StackedBar),
            ("bar_across", ChartKind::BarAcross),
            ("donut", ChartKind::Pie),
            ("area", ChartKind::Area),
        ] {
            assert_eq!(ChartKind::parse(word), Some(kind), "{word}");
        }
        assert_eq!(ChartKind::parse("spiral"), None);
        assert_eq!(ChartKind::parse(""), None);
    }

    #[test]
    fn a_chart_that_cannot_be_read_says_which_part() {
        let cases = [
            (r#"{"labels":["a"],"values":[1]}"#, "does not say its kind"),
            (
                r#"{"kind":"spiral","labels":["a"],"values":[1]}"#,
                "not a kind",
            ),
            (
                r#"{"kind":"bar","labels":["a"],"values":[1,2]}"#,
                "only 1 labels",
            ),
            (
                r#"{"kind":"bar","labels":["a"],"values":["x"]}"#,
                "not a number",
            ),
            (r#"{"kind":"bar","labels":["a"]}"#, "no numbers in it"),
        ];
        for (bad, wanted) in cases {
            let why = read(bad).expect_err(bad);
            assert!(
                why.contains(wanted),
                "{bad}\n  said: {why}\n  wanted: {wanted}"
            );
        }
        read(r#"{"kind":"bar","labels":["a"],"values":[1]}"#).expect("the control reads");
    }

    #[test]
    fn the_reach_of_a_chart_includes_zero() {
        let chart = read(
            r#"{"kind":"bar","labels":["a","b"],
            "series":[{"name":"x","values":[3,1]},{"name":"y","values":[4,1]}]}"#,
        )
        .expect("read");
        assert_eq!(chart.reach(), (0.0, 4.0));
        assert_eq!(chart.stacked_reach(), (0.0, 7.0));
        assert_eq!(Chart::default().reach(), (0.0, 0.0));
    }
}
