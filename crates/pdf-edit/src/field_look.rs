use std::fmt::Write as _;

use crate::form::{BorderStyle, ButtonStyle};

const KAPPA: f64 = 0.552_284_749_830_793_4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Look {
    pub(crate) width: f64,
    pub(crate) height: f64,
    pub(crate) border: Option<[f64; 3]>,
    pub(crate) fill: Option<[f64; 3]>,
    pub(crate) border_width: f64,
    pub(crate) style: BorderStyle,
    pub(crate) round: bool,
}

fn rgb([red, green, blue]: [f64; 3]) -> String {
    format!("{red} {green} {blue}")
}

pub(crate) fn circle(x: f64, y: f64, r: f64) -> String {
    let k = r * KAPPA;
    format!(
        "{} {y} m {} {} {} {} {x} {} c {} {} {} {} {} {y} c {} {} {} {} {x} {} c {} {} {} {} {} {y} c ",
        x + r,
        x + r,
        y + k,
        x + k,
        y + r,
        y + r,
        x - k,
        y + r,
        x - r,
        y + k,
        x - r,
        x - r,
        y - k,
        x - k,
        y - r,
        y - r,
        x + k,
        y - r,
        x + r,
        y - k,
        x + r,
    )
}

pub(crate) fn frame(look: &Look) -> String {
    let (w, h) = (look.width, look.height);
    let bw = look.border_width.max(0.0);
    let mut out = String::new();
    if let Some(fill) = look.fill {
        if look.round {
            let r = w.min(h) / 2.0;
            _ = writeln!(out, "q {} rg {}f Q", rgb(fill), circle(w / 2.0, h / 2.0, r));
        } else {
            _ = writeln!(out, "q {} rg 0 0 {w} {h} re f Q", rgb(fill));
        }
    }
    let Some(border) = look.border else {
        return out;
    };
    if bw <= 0.0 {
        return out;
    }
    let dash = if look.style == BorderStyle::Dashed {
        "[3] 0 d "
    } else {
        ""
    };
    if look.round {
        let r = w.min(h) / 2.0 - bw / 2.0;
        _ = writeln!(
            out,
            "q {} RG {bw} w {dash}{}S Q",
            rgb(border),
            circle(w / 2.0, h / 2.0, r)
        );
        return out;
    }
    let half = bw / 2.0;
    if look.style == BorderStyle::Underline {
        _ = writeln!(
            out,
            "q {} RG {bw} w 0 {half} m {w} {half} l S Q",
            rgb(border)
        );
        return out;
    }
    _ = writeln!(
        out,
        "q {} RG {bw} w {dash}{half} {half} {} {} re S Q",
        rgb(border),
        w - bw,
        h - bw
    );
    let (light, dark) = match look.style {
        BorderStyle::Beveled => (
            [1.0, 1.0, 1.0],
            look.fill
                .map_or([0.5; 3], |fill| fill.map(|part| part * 0.5)),
        ),
        BorderStyle::Inset => ([0.5; 3], [0.75; 3]),
        _ => return out,
    };
    let (top_left, bottom_right) = bands(w, h, bw);
    for (colour, band) in [(light, top_left), (dark, bottom_right)] {
        let mut path = String::new();
        for (at, (x, y)) in band.iter().enumerate() {
            _ = write!(path, "{x} {y} {} ", if at == 0 { "m" } else { "l" });
        }
        _ = writeln!(out, "q {} rg {path}h f Q", rgb(colour));
    }
    out
}

type Band = [(f64, f64); 6];

fn bands(w: f64, h: f64, bw: f64) -> (Band, Band) {
    let (a, b) = (bw, 2.0 * bw);
    (
        [
            (a, a),
            (a, h - a),
            (w - a, h - a),
            (w - b, h - b),
            (b, h - b),
            (b, b),
        ],
        [
            (a, a),
            (w - a, a),
            (w - a, h - a),
            (w - b, h - b),
            (w - b, b),
            (b, b),
        ],
    )
}

#[expect(
    clippy::many_single_char_names,
    reason = "a drawing's width, height, side and centre, named as geometry names them"
)]
pub(crate) fn mark(style: ButtonStyle, look: &Look, colour: [f64; 3]) -> String {
    let (w, h) = (look.width, look.height);
    let side = w.min(h);
    let (cx, cy) = (w / 2.0, h / 2.0);
    let ink = rgb(colour);
    match style {
        ButtonStyle::Check => format!(
            "q {ink} RG {} w 1 J 1 j {} {} m {} {} l {} {} l S Q\n",
            side * 0.12,
            cx - side * 0.28,
            cy + side * 0.02,
            cx - side * 0.08,
            cy - side * 0.22,
            cx + side * 0.28,
            cy + side * 0.24,
        ),
        ButtonStyle::Cross => format!(
            "q {ink} RG {} w 1 J {} {} m {} {} l {} {} m {} {} l S Q\n",
            side * 0.12,
            cx - side * 0.25,
            cy - side * 0.25,
            cx + side * 0.25,
            cy + side * 0.25,
            cx - side * 0.25,
            cy + side * 0.25,
            cx + side * 0.25,
            cy - side * 0.25,
        ),
        ButtonStyle::Circle => format!("q {ink} rg {}f Q\n", circle(cx, cy, side * 0.22)),
        ButtonStyle::Square => {
            let half = side * 0.22;
            format!(
                "q {ink} rg {} {} {} {} re f Q\n",
                cx - half,
                cy - half,
                2.0 * half,
                2.0 * half
            )
        }
        ButtonStyle::Diamond => {
            let r = side * 0.3;
            format!(
                "q {ink} rg {cx} {} m {} {cy} l {cx} {} l {} {cy} l h f Q\n",
                cy + r,
                cx + r,
                cy - r,
                cx - r
            )
        }
        ButtonStyle::Star => {
            let (outer, inner) = (side * 0.32, side * 0.13);
            let mut path = String::new();
            for point in 0..10 {
                let r = if point % 2 == 0 { outer } else { inner };
                let angle =
                    std::f64::consts::FRAC_PI_2 + f64::from(point) * std::f64::consts::PI / 5.0;
                let (x, y) = (cx + r * angle.cos(), cy + r * angle.sin());
                _ = write!(path, "{x} {y} {} ", if point == 0 { "m" } else { "l" });
            }
            format!("q {ink} rg {path}h f Q\n")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Look, frame, mark};
    use crate::form::{BorderStyle, ButtonStyle};

    fn look(style: BorderStyle) -> Look {
        Look {
            width: 100.0,
            height: 20.0,
            border: Some([0.0, 0.0, 0.0]),
            fill: Some([1.0, 1.0, 0.0]),
            border_width: 2.0,
            style,
            round: false,
        }
    }

    #[test]
    fn each_border_style_draws_what_it_names() {
        let solid = frame(&look(BorderStyle::Solid));
        assert!(solid.contains(" re S") && !solid.contains(" d "));
        assert!(frame(&look(BorderStyle::Dashed)).contains("[3] 0 d"));
        let under = frame(&look(BorderStyle::Underline));
        assert!(under.contains(" l S") && !under.contains(" re S"));
        assert_eq!(
            frame(&look(BorderStyle::Beveled)).matches(" f Q").count(),
            3,
            "fill and two bands"
        );
        assert_eq!(frame(&look(BorderStyle::Inset)).matches(" f Q").count(), 3);
        assert_eq!(solid.matches(" f Q").count(), 1, "the fill only");
    }

    #[test]
    fn the_bands_stay_in_the_border_strip() {
        let (w, h, bw) = (100.0, 20.0, 2.0);
        let (top_left, bottom_right) = super::bands(w, h, bw);
        let within = |value: f64, low: f64, high: f64| (low..=high).contains(&value);
        for (x, y) in top_left {
            assert!(
                within(x, bw, 2.0 * bw) || within(y, h - 2.0 * bw, h - bw),
                "{x} {y}"
            );
        }
        for (x, y) in bottom_right {
            assert!(
                within(x, w - 2.0 * bw, w - bw) || within(y, bw, 2.0 * bw),
                "{x} {y}"
            );
        }
        let area = |band: super::Band| {
            let twice: f64 = (0..6)
                .map(|at| {
                    let (x0, y0) = band[at];
                    let (x1, y1) = band[(at + 1) % 6];
                    x0 * y1 - x1 * y0
                })
                .sum();
            (twice / 2.0).abs()
        };
        for band in [top_left, bottom_right] {
            let covered = area(band);
            assert!(
                (bw * (w + h) * 0.5..=bw * (w + h) * 1.5).contains(&covered),
                "{covered}"
            );
        }
    }

    #[test]
    fn a_box_without_a_border_draws_none() {
        let mut bare = look(BorderStyle::Solid);
        bare.border = None;
        assert!(!frame(&bare).contains(" S Q"));
        let mut thin = look(BorderStyle::Solid);
        thin.border_width = 0.0;
        assert!(!frame(&thin).contains(" S Q"));
    }

    #[test]
    fn the_six_marks_are_six_drawings() {
        let square = look(BorderStyle::Solid);
        let drawn: Vec<String> = ButtonStyle::ALL
            .iter()
            .map(|style| mark(*style, &square, [0.0, 0.0, 0.0]))
            .collect();
        assert!(drawn[0].ends_with("S Q\n"), "a check is a stroke");
        assert!(drawn[1].ends_with("f Q\n"), "a circle is filled");
        for (at, one) in drawn.iter().enumerate() {
            assert!(!drawn[at + 1..].contains(one));
        }
    }
}
