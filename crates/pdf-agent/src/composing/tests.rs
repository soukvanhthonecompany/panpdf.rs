use std::fmt::Write as _;

use super::theme::{self, Theme};
use super::{Composed, Mark, Measure, Room, Setting, Sheet, Style, compose};
use crate::markup::laying_out::parts;

struct Even;

impl Measure for Even {
    fn room(&self, text: &str, style: &Style, width: f64) -> Result<Room, String> {
        if text.chars().any(|c| u32::from(c) > 0xFFFF) {
            return Err("no face on this machine draws what was typed".to_owned());
        }
        let per = style.size * 0.5;
        let mut lines = 0_u32;
        let mut widest: f64 = 0.0;
        for paragraph in text.split('\n') {
            let wide = f64::from(u32::try_from(paragraph.chars().count()).unwrap_or(0)) * per;
            let count = (wide / width).ceil().max(1.0);
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a small count"
            )]
            let count = count as u32;
            lines += count;
            widest = widest.max(wide.min(width));
        }
        Ok(Room {
            height: f64::from(lines) * style.size * 1.2,
            widest,
            lines: lines as usize,
        })
    }
}

const A4: Sheet = Sheet {
    wide: 595.0,
    high: 842.0,
    margin: 50.0,
};

fn composed(markdown: &str, theme: &Theme) -> Composed {
    let setting = Setting {
        sheet: A4,
        from_page: 0,
        start: None,
        family: "Noto Sans",
        theme,
        body: 10.0,
    };
    compose(&parts(markdown, 10.0), &setting, &Even).expect("it composes")
}

fn same(one: &[f64; 3], other: &[f64; 3]) -> bool {
    one.map(f64::to_bits) == other.map(f64::to_bits)
}

fn texts(composed: &Composed) -> Vec<(&str, [f64; 4], &Style, usize)> {
    composed
        .marks
        .iter()
        .filter_map(|mark| match mark {
            Mark::Text {
                page,
                area,
                text,
                style,
            } => Some((text.as_str(), *area, style, *page)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_theme_dresses_the_page() {
    let classic = theme::named("classic").expect("classic");
    let out = composed("# Title\n\n## Part\n\n- item\n\nBody.\n", classic);
    let first_text = out
        .marks
        .iter()
        .position(|mark| matches!(mark, Mark::Text { .. }))
        .expect("text");
    let bands: Vec<&Mark> = out.marks[..first_text].iter().collect();
    assert_eq!(
        bands.len(),
        2,
        "shadow and band before the title's words: {bands:#?}"
    );
    assert!(
        matches!(bands[1], Mark::Shape { fill: Some(fill), .. } if same(fill, &classic.accent))
    );
    let words = texts(&out);
    let (title, area, style, _) = words[0];
    assert_eq!(title, "Title");
    assert_eq!(style.colour, Some(classic.on_accent));
    assert!((area[0] - (50.0 + 18.0 * 0.6)).abs() < 1e-9, "{area:?}");
    let (part, _, style, _) = words[1];
    assert_eq!(part, "Part");
    assert_eq!(style.colour, Some(classic.accent));
    let (item, _, _, _) = words[2];
    assert_eq!(item, "item");
    let dot = out.marks.iter().any(|mark| {
        matches!(mark, Mark::Shape { fill: Some(fill), steps, .. } if same(fill, &classic.accent) && steps.len() == 5)
    });
    assert!(dot, "a round bullet in the accent");
    assert_eq!(words[3].2.colour, Some(classic.ink));

    let plain = theme::named("plain").expect("plain");
    let bare = composed("# Title\n\n## Part\n\n- item\n", plain);
    for mark in &bare.marks {
        if let Mark::Shape {
            fill: Some(fill), ..
        } = mark
        {
            assert!(
                same(fill, &plain.ink) || same(fill, &plain.line),
                "{fill:?}"
            );
        }
    }
}

#[test]
fn a_table_is_dressed_and_carried_over() {
    let classic = theme::named("classic").expect("classic");
    let mut markdown = "| Name | Score |\n|---|---|\n".to_owned();
    for at in 0..40 {
        let _ = writeln!(markdown, "| Row {at} | {at} |");
    }
    let out = composed(&markdown, classic);
    let first_text = out
        .marks
        .iter()
        .position(|mark| matches!(mark, Mark::Text { .. }))
        .expect("text");
    let before: Vec<&Mark> = out.marks[..first_text].iter().collect();
    assert!(
        before.iter().any(
            |mark| matches!(mark, Mark::Shape { fill: Some(fill), .. } if same(fill, &classic.accent))
        ),
        "the head's band goes first"
    );
    assert!(
        before.iter().any(
            |mark| matches!(mark, Mark::Shape { fill: Some(fill), .. } if same(fill, &classic.stripe))
        ),
        "the stripes go first"
    );
    assert!(
        out.marks
            .iter()
            .any(|mark| matches!(mark, Mark::NewPage { after: 0 }))
    );
    let heads: Vec<usize> = texts(&out)
        .into_iter()
        .filter(|(text, _, _, _)| *text == "Name")
        .map(|(_, _, _, page)| page)
        .collect();
    assert_eq!(heads, vec![0, 1]);
    assert_eq!(out.pages, 2);
}

#[test]
fn a_long_document_goes_on_to_new_pages() {
    let classic = theme::named("slate").expect("slate");
    let markdown = "A paragraph of words.\n\n".repeat(80);
    let out = composed(&markdown, classic);
    assert!(out.pages >= 2);
    for (_, area, _, _) in texts(&out) {
        assert!(area[3] <= 842.0 - 50.0 + 1e-9, "{area:?}");
    }
}

#[test]
fn a_dark_theme_paints_the_paper() {
    let midnight = theme::named("midnight").expect("midnight");
    let out = composed(&"Words.\n\n".repeat(120), midnight);
    let Mark::Shape { fill, .. } = &out.marks[0] else {
        panic!("the paper first");
    };
    assert_eq!(*fill, midnight.paper);
    let papers = out
        .marks
        .iter()
        .filter(|mark| matches!(mark, Mark::Shape { fill, .. } if *fill == midnight.paper))
        .count();
    assert_eq!(papers, out.pages);
}

#[test]
fn pictographs_are_left_out() {
    let classic = theme::named("classic").expect("classic");
    let out = composed("Well done! \u{1F31F}\n", classic);
    assert_eq!(texts(&out)[0].0, "Well done! ");
    assert_eq!(out.left_out, "\u{1F31F}", "and the model is told what went");
}

#[test]
fn charts_are_drawn_or_refused_whole() {
    let classic = theme::named("classic").expect("classic");
    let out = composed(
        "```chart\n{\"type\":\"bar\",\"title\":\"Sales\",\"x\":[\"Q1\",\"Q2\"],\"series\":[{\"name\":\"A\",\"values\":[3,5]},{\"name\":\"B\",\"values\":[4,2]}],\"style\":\"3d\"}\n```\n",
        classic,
    );
    let fills: Vec<[f64; 3]> = out
        .marks
        .iter()
        .filter_map(|mark| match mark {
            Mark::Shape {
                fill: Some(fill), ..
            } => Some(*fill),
            _ => None,
        })
        .collect();
    assert!(fills.contains(&classic.palette[0]) && fills.contains(&classic.palette[1]));
    assert!(
        fills.contains(&theme::darker(classic.palette[0], 0.28)),
        "a shaded side"
    );
    assert!(texts(&out).iter().any(|(text, _, _, _)| *text == "Sales"));

    let setting = Setting {
        sheet: A4,
        from_page: 0,
        start: None,
        family: "Noto Sans",
        theme: classic,
        body: 10.0,
    };
    let refused = compose(
        &parts("Before.\n\n```chart\n{\"type\":\"radar\"}\n```\n", 10.0),
        &setting,
        &Even,
    );
    assert!(refused.unwrap_err().contains("radar"));
}

#[test]
fn an_equation_is_stacked() {
    let classic = theme::named("classic").expect("classic");
    let out = composed("$$\\frac{a+b}{2}$$\n", classic);
    let words = texts(&out);
    assert_eq!(words.len(), 2, "{words:?}");
    assert!(
        words[0].1[1] < words[1].1[1],
        "the numerator above the denominator"
    );
    let bars = out
        .marks
        .iter()
        .filter(|mark| {
            matches!(
                mark,
                Mark::Shape {
                    stroke: Some(_),
                    ..
                }
            )
        })
        .count();
    assert_eq!(bars, 1);
}

#[test]
fn a_heading_keeps_with_what_it_heads() {
    let classic = theme::named("classic").expect("classic");
    let filler = "Words.\n\n".repeat(42);
    let chart = format!(
        "{filler}## Results\n\n```chart\n{{\"type\":\"bar\",\"x\":[\"a\"],\"series\":[{{\"values\":[1]}}],\"height\":200}}\n```\n"
    );
    let page_of = |out: &Composed, wanted: &str| {
        texts(out)
            .into_iter()
            .filter(|(text, _, _, _)| *text == wanted)
            .map(|(_, _, _, page)| page)
            .next_back()
            .expect("found")
    };
    let out = composed(&chart, classic);
    assert_eq!(page_of(&out, "Words."), 0, "the filler fits page 0");
    assert_eq!(
        page_of(&out, "Results"),
        1,
        "the heading moved to the chart's page"
    );
    let control = composed(&format!("{filler}## Results\n\nMore.\n"), classic);
    assert_eq!(page_of(&control, "Results"), 0);
}
