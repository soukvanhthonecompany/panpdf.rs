use std::ops::Range;

pub const FIT_SLACK: f64 = 1e-6;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Unit {
    pub advance: f64,
    pub pitch: f64,
    pub break_before: bool,
    pub hangs: bool,
    pub spacing_after: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Paragraph<'a> {
    pub units: &'a [Unit],
    pub empty_pitch: f64,
    pub first_indent: f64,
    pub split_words: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LaidLine {
    pub paragraph: usize,
    pub units: Range<usize>,
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub pitch: f64,
    pub overflow: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub lines: Vec<LaidLine>,
    pub height: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LayoutError {
    Width(f64),
    Unit {
        paragraph: usize,
        unit: usize,
    },
    EmptyPitch {
        paragraph: usize,
    },
    Indent {
        paragraph: usize,
    },
    Backwards {
        paragraph: usize,
        unit: usize,
        ends_a_line: bool,
    },
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Width(width) => write!(formatter, "frame width {width} is not a positive number"),
            Self::Unit { paragraph, unit } => write!(
                formatter,
                "unit {unit} of paragraph {paragraph} has an advance or pitch that cannot be laid out"
            ),
            Self::EmptyPitch { paragraph } => {
                write!(formatter, "paragraph {paragraph} has no usable line pitch")
            }
            Self::Indent { paragraph } => write!(
                formatter,
                "paragraph {paragraph} has a first-line indent that leaves no line"
            ),
            Self::Backwards {
                paragraph,
                unit,
                ends_a_line: true,
            } => write!(
                formatter,
                "unit {unit} of paragraph {paragraph} moves the pen back where a line may end"
            ),
            Self::Backwards {
                paragraph, unit, ..
            } => write!(
                formatter,
                "unit {unit} of paragraph {paragraph} is in a word that moves the pen backwards"
            ),
        }
    }
}

impl std::error::Error for LayoutError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Alignment {
    Start,
    Centre,
    End,
    Justify,
}

impl Alignment {
    #[must_use]
    pub fn offset(self, frame_width: f64, line_width: f64) -> f64 {
        let spare = frame_width - line_width;
        if spare <= 0.0 {
            return 0.0;
        }
        match self {
            Self::Start | Self::Justify => 0.0,
            Self::Centre => spare / 2.0,
            Self::End => spare,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blocked {
    pub top: f64,
    pub bottom: f64,
    pub left: f64,
    pub right: f64,
}

pub fn lay_out(paragraphs: &[Paragraph<'_>], width: f64) -> Result<Layout, LayoutError> {
    lay_out_around(paragraphs, width, &[])
}

pub fn lay_out_around(
    paragraphs: &[Paragraph<'_>],
    width: f64,
    blocked: &[Blocked],
) -> Result<Layout, LayoutError> {
    if !width.is_finite() || width <= 0.0 {
        return Err(LayoutError::Width(width));
    }
    let mut lines = Vec::new();
    let mut y = 0.0;
    for (index, paragraph) in paragraphs.iter().enumerate() {
        check(index, paragraph, width)?;
        lay_out_paragraph(index, paragraph, width, blocked, &mut y, &mut lines);
    }
    Ok(Layout { lines, height: y })
}

fn check(index: usize, paragraph: &Paragraph<'_>, width: f64) -> Result<(), LayoutError> {
    if !paragraph.empty_pitch.is_finite() || paragraph.empty_pitch <= 0.0 {
        return Err(LayoutError::EmptyPitch { paragraph: index });
    }
    if !(paragraph.first_indent.is_finite() && paragraph.first_indent < width) {
        return Err(LayoutError::Indent { paragraph: index });
    }
    let units = paragraph.units;
    let mut glued = 0.0;
    let mut first_back = None;
    for (unit, measured) in units.iter().enumerate() {
        if !measured.advance.is_finite() || !measured.pitch.is_finite() || measured.pitch < 0.0 {
            return Err(LayoutError::Unit {
                paragraph: index,
                unit,
            });
        }
        if unit > 0 && measured.break_before {
            glued = 0.0;
            first_back = None;
        }
        glued += measured.advance;
        if measured.advance < 0.0 {
            first_back.get_or_insert(unit);
        }
        if units.get(unit + 1).is_some_and(|next| !next.break_before) {
            continue;
        }
        if glued < 0.0
            && let Some(back) = first_back
        {
            return Err(LayoutError::Backwards {
                paragraph: index,
                unit: back,
                ends_a_line: false,
            });
        }
        if measured.advance < 0.0 && unit + 1 < units.len() {
            return Err(LayoutError::Backwards {
                paragraph: index,
                unit,
                ends_a_line: true,
            });
        }
    }
    Ok(())
}

fn valid_blocked(rect: &Blocked) -> bool {
    rect.top.is_finite()
        && rect.bottom.is_finite()
        && rect.left.is_finite()
        && rect.right.is_finite()
        && rect.top < rect.bottom
        && rect.left < rect.right
}

fn overlapping(y: f64, pitch: f64, blocked: &[Blocked]) -> impl Iterator<Item = &Blocked> {
    blocked
        .iter()
        .filter(move |rect| valid_blocked(rect) && rect.top < y + pitch && rect.bottom > y)
}

fn widest_run(y: f64, pitch: f64, blocked: &[Blocked], width: f64) -> (f64, f64) {
    let mut spans: Vec<(f64, f64)> = overlapping(y, pitch, blocked)
        .map(|rect| (rect.left.max(0.0), rect.right.min(width)))
        .filter(|(left, right)| left < right)
        .collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut cursor = 0.0;
    let mut best = (0.0, 0.0);
    for (left, right) in spans.drain(..) {
        if left > cursor && left - cursor > best.1 {
            best = (cursor, left - cursor);
        }
        cursor = cursor.max(right);
    }
    if width > cursor && width - cursor > best.1 {
        best = (cursor, width - cursor);
    }
    best
}

#[must_use]
pub fn widest_free_run(
    width: f64,
    blocked: &[Blocked],
    top: f64,
    pitch: f64,
) -> Option<(f64, f64)> {
    if !(width.is_finite() && width > 0.0 && top.is_finite() && pitch.is_finite() && pitch > 0.0) {
        return None;
    }
    let (start, run) = widest_run(top, pitch, blocked, width);
    (run > 0.0).then_some((start, run))
}

fn next_bottom(y: f64, pitch: f64, blocked: &[Blocked]) -> Option<f64> {
    overlapping(y, pitch, blocked)
        .map(|rect| rect.bottom)
        .min_by(f64::total_cmp)
}

fn find_band(y: &mut f64, pitch: f64, blocked: &[Blocked], width: f64) -> (f64, f64) {
    loop {
        let (start, run_width) = widest_run(*y, pitch, blocked, width);
        if run_width > 0.0 {
            return (start, run_width);
        }
        match next_bottom(*y, pitch, blocked) {
            Some(bottom) => *y = bottom,
            None => return (0.0, width),
        }
    }
}

fn tallest_pitch(units: &[Unit], range: Range<usize>, empty_pitch: f64) -> f64 {
    let tallest = units[range]
        .iter()
        .map(|unit| unit.pitch)
        .fold(0.0, f64::max);
    if tallest > 0.0 { tallest } else { empty_pitch }
}

fn lay_out_paragraph(
    index: usize,
    paragraph: &Paragraph<'_>,
    width: f64,
    blocked: &[Blocked],
    y: &mut f64,
    lines: &mut Vec<LaidLine>,
) {
    let units = paragraph.units;
    if units.is_empty() {
        let pitch = paragraph.empty_pitch;
        let (left, _) = find_band(y, pitch, blocked, width);
        lines.push(LaidLine {
            paragraph: index,
            units: 0..0,
            left,
            top: *y,
            width: 0.0,
            pitch,
            overflow: false,
        });
        *y += pitch;
        return;
    }
    let mut start = 0;
    while start < units.len() {
        let indent = if start == 0 {
            paragraph.first_indent
        } else {
            0.0
        };
        let provisional_pitch = units[start].pitch;
        let (run_start, run_width) = find_band(y, provisional_pitch, blocked, width);
        let (end, overflow) = line_end(units, start, run_width - indent, paragraph.split_words);
        let pitch = tallest_pitch(units, start..end, paragraph.empty_pitch);
        #[allow(
            clippy::float_cmp,
            reason = "pitch is a value selected from the units' own pitches, not computed by arithmetic, so exact equality asks the right question: did filling the line pick the same value back"
        )]
        let differs = pitch != provisional_pitch;
        let (run_start, end, overflow, pitch) = if differs {
            let (run_start2, run_width2) = find_band(y, pitch, blocked, width);
            let (end2, overflow2) =
                line_end(units, start, run_width2 - indent, paragraph.split_words);
            let pitch2 = tallest_pitch(units, start..end2, paragraph.empty_pitch);
            (run_start2, end2, overflow2, pitch2)
        } else {
            (run_start, end, overflow, pitch)
        };
        let range = start..end;
        lines.push(LaidLine {
            paragraph: index,
            left: run_start,
            top: *y,
            width: visible_width(&units[range.clone()]),
            pitch,
            units: range,
            overflow,
        });
        *y += pitch;
        start = end;
    }
}

fn line_end(units: &[Unit], start: usize, width: f64, split_words: bool) -> (usize, bool) {
    let mut sum = 0.0;
    let mut hanging = 0.0;
    let mut spacing = 0.0;
    let mut last_break = None;
    for (index, unit) in units.iter().enumerate().skip(start) {
        if index > start && unit.break_before {
            last_break = Some(index);
        }
        if unit.hangs && index > start && sum - spacing > width + FIT_SLACK {
            return (index, false);
        }
        let reach = sum + unit.advance;
        let trailing = if unit.hangs {
            hanging + unit.advance
        } else {
            0.0
        };
        if !unit.hangs && reach - trailing - unit.spacing_after > width + FIT_SLACK {
            if let Some(end) = last_break {
                return (end, false);
            }
            if !split_words {
                let end = units
                    .iter()
                    .enumerate()
                    .skip(index + 1)
                    .find(|(_, unit)| unit.break_before)
                    .map_or(units.len(), |(end, _)| end);
                return (end, true);
            }
            let ends_forward = |end: usize| {
                (end == units.len() || units[end - 1].advance >= 0.0)
                    && units[start..end]
                        .iter()
                        .map(|unit| unit.advance)
                        .sum::<f64>()
                        >= 0.0
            };
            let mut end = index;
            while end > start + 1 && !ends_forward(end) {
                end -= 1;
            }
            if end > start && ends_forward(end) {
                return (end, false);
            }
            let mut end = start + 1;
            while end < units.len() && !ends_forward(end) {
                end += 1;
            }
            return (end, true);
        }
        sum = reach;
        hanging = trailing;
        spacing = unit.spacing_after;
    }
    (units.len(), false)
}

fn visible_width(units: &[Unit]) -> f64 {
    let hanging = units.iter().rev().take_while(|unit| unit.hangs).count();
    let visible = &units[..units.len() - hanging];
    visible.iter().map(|unit| unit.advance).sum::<f64>()
        - visible.last().map_or(0.0, |unit| unit.spacing_after)
}

#[must_use]
pub fn justify_gaps(units: &[Unit], run_width: f64, line_width: f64) -> Vec<f64> {
    let mut gaps = vec![0.0; units.len()];
    let spare = run_width - line_width;
    if !spare.is_finite() || spare <= 0.0 {
        return gaps;
    }
    let eligible: Vec<usize> = (0..units.len())
        .filter(|&index| {
            let next = index + 1;
            next < units.len()
                && units[next].break_before
                && !units[next..].iter().all(|unit| unit.hangs)
        })
        .collect();
    if eligible.is_empty() {
        return gaps;
    }
    #[allow(
        clippy::cast_precision_loss,
        reason = "a line's unit count never approaches where usize-to-f64 loses precision"
    )]
    let each_gap = spare / eligible.len() as f64;
    for index in eligible {
        gaps[index] = each_gap;
    }
    gaps
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    reason = "known answers are exact sums of ten-point units, which floating point represents exactly"
)]
mod tests {
    use super::*;

    fn units(count: usize) -> Vec<Unit> {
        vec![
            Unit {
                advance: 10.0,
                pitch: 12.0,
                break_before: true,
                hangs: false,
                spacing_after: 0.0,
            };
            count
        ]
    }

    fn one(units: &[Unit]) -> [Paragraph<'_>; 1] {
        [Paragraph {
            units,
            empty_pitch: 12.0,
            first_indent: 0.0,
            split_words: true,
        }]
    }

    fn assert_every_unit_once(paragraphs: &[Paragraph<'_>], layout: &Layout) {
        for (index, paragraph) in paragraphs.iter().enumerate() {
            let mut next = 0;
            for line in layout.lines.iter().filter(|line| line.paragraph == index) {
                assert_eq!(line.units.start, next, "paragraph {index} skips or repeats");
                next = line.units.end;
            }
            assert_eq!(
                next,
                paragraph.units.len(),
                "paragraph {index} loses its tail"
            );
        }
    }

    #[test]
    fn spacing_after_a_lines_last_glyph_does_not_count_toward_its_width() {
        let mut text = marked(&[4]);
        text[2].advance = 10.5;
        text[2].spacing_after = 0.5;
        text[3].hangs = true;
        let paragraphs = one(&text);
        let layout = lay_out(&paragraphs, 30.0).unwrap();
        assert_eq!(
            layout
                .lines
                .iter()
                .map(|line| (line.units.clone(), line.width, line.overflow))
                .collect::<Vec<_>>(),
            vec![(0..4, 30.0, false), (4..5, 10.0, false)]
        );
    }

    #[test]
    fn white_space_hangs_past_the_frame_but_never_begins_past_it() {
        let mut text = units(4);
        text.extend(units(30).into_iter().map(|unit| Unit {
            hangs: true,
            ..unit
        }));
        let paragraphs = one(&text);
        let layout = lay_out(&paragraphs, 100.0).unwrap();
        assert!(
            layout.lines.len() > 1,
            "thirty spaces stayed on one line: {:?}",
            layout.lines
        );
        assert_every_unit_once(&paragraphs, &layout);
        for line in &layout.lines {
            let mut pen = 0.0;
            for (offset, unit) in text[line.units.clone()].iter().enumerate() {
                assert!(
                    pen <= 100.0 + FIT_SLACK,
                    "unit {offset} of line {:?} begins at {pen}, outside the frame",
                    line.units
                );
                pen += unit.advance;
            }
            assert!(
                pen <= 110.0 + FIT_SLACK,
                "line {:?} reaches {pen}, more than one space past the frame",
                line.units
            );
        }
    }

    #[test]
    fn the_contract_known_answers_twenty_thirty_thirty_one() {
        for (count, lines, height) in [(20, 2, 24.0), (30, 3, 36.0), (31, 4, 48.0)] {
            let text = units(count);
            let paragraphs = one(&text);
            let layout = lay_out(&paragraphs, 100.0).unwrap();
            assert_eq!(layout.lines.len(), lines, "{count} units");
            assert_eq!(layout.height, height, "{count} units");
            assert_every_unit_once(&paragraphs, &layout);
            for line in &layout.lines {
                assert!(line.width <= 100.0 + FIT_SLACK);
                assert!(!line.overflow);
            }
        }
        let text = units(31);
        let layout = lay_out(&one(&text), 100.0).unwrap();
        assert_eq!(layout.lines[3].units, 30..31);
        assert_eq!(layout.lines[3].width, 10.0);
    }

    #[test]
    fn a_word_wider_than_the_line_is_split_between_clusters_at_the_edge() {
        let mut text = units(15);
        for unit in &mut text {
            unit.break_before = false;
        }
        text[12].break_before = true;
        let paragraphs = one(&text);
        let layout = lay_out(&paragraphs, 100.0).unwrap();
        assert_eq!(
            layout
                .lines
                .iter()
                .map(|line| line.units.clone())
                .collect::<Vec<_>>(),
            [0..10, 10..15]
        );
        assert!(layout.lines.iter().all(|line| !line.overflow));
        assert!(
            layout
                .lines
                .iter()
                .all(|line| line.width <= 100.0 + FIT_SLACK)
        );
        assert_every_unit_once(&paragraphs, &layout);
    }

    #[test]
    fn a_word_wider_than_the_line_runs_on_whole_where_words_may_not_split() {
        let mut text = units(15);
        for unit in &mut text {
            unit.break_before = false;
        }
        text[12].break_before = true;
        let paragraphs = [Paragraph {
            units: &text,
            empty_pitch: 12.0,
            first_indent: 0.0,
            split_words: false,
        }];
        let layout = lay_out(&paragraphs, 100.0).unwrap();
        assert_eq!(
            layout
                .lines
                .iter()
                .map(|line| (line.units.clone(), line.overflow))
                .collect::<Vec<_>>(),
            [(0..12, true), (12..15, false)]
        );
        assert_eq!(layout.lines[0].width, 120.0);
        assert_every_unit_once(&paragraphs, &layout);
    }

    #[test]
    fn a_word_that_fits_on_a_line_of_its_own_moves_down_whole() {
        let mut text = units(13);
        for (index, unit) in text.iter_mut().enumerate() {
            unit.break_before = index == 4;
            unit.hangs = index == 3;
        }
        let layout = lay_out(&one(&text), 100.0).unwrap();
        assert_eq!(
            layout
                .lines
                .iter()
                .map(|line| line.units.clone())
                .collect::<Vec<_>>(),
            [0..4, 4..13]
        );
    }

    #[test]
    fn a_single_cluster_wider_than_the_frame_overflows_on_a_line_of_its_own() {
        let mut text = units(3);
        text[1].advance = 150.0;
        for unit in &mut text {
            unit.break_before = false;
        }
        let paragraphs = one(&text);
        let layout = lay_out(&paragraphs, 100.0).unwrap();
        assert_eq!(
            layout
                .lines
                .iter()
                .map(|line| (line.units.clone(), line.overflow))
                .collect::<Vec<_>>(),
            [(0..1, false), (1..2, true), (2..3, false)]
        );
        assert_eq!(layout.lines[1].width, 150.0, "never shrunk");
        assert_every_unit_once(&paragraphs, &layout);
    }

    #[test]
    fn white_space_at_the_edge_hangs_and_does_not_push_a_word_down() {
        let mut text = units(14);
        for (index, unit) in text.iter_mut().enumerate() {
            unit.break_before = index == 11;
            unit.hangs = index == 10;
        }
        let paragraphs = one(&text);
        let layout = lay_out(&paragraphs, 100.0).unwrap();
        assert_eq!(layout.lines[0].units, 0..11);
        assert_eq!(
            layout.lines[0].width, 100.0,
            "the hanging space is not counted"
        );
        assert!(!layout.lines[0].overflow);
        assert_eq!(layout.lines[1].units, 11..14);
        text[10].hangs = false;
        let layout = lay_out(&one(&text), 100.0).unwrap();
        assert_eq!(layout.lines[0].units, 0..10);
        assert!(!layout.lines[0].overflow);
    }

    #[test]
    fn a_first_line_indent_narrows_only_the_first_line() {
        let many = units(10);
        let indented = [Paragraph {
            units: &many,
            empty_pitch: 12.0,
            first_indent: 25.0,
            split_words: true,
        }];
        let layout = lay_out(&indented, 100.0).unwrap();
        let ranges: Vec<_> = layout.lines.iter().map(|line| line.units.clone()).collect();
        assert_eq!(ranges, vec![0..7, 7..10]);
        assert_eq!(lay_out(&one(&many), 100.0).unwrap().lines.len(), 1);

        let wide = [Paragraph {
            units: &many,
            empty_pitch: 12.0,
            first_indent: 100.0,
            split_words: true,
        }];
        assert_eq!(
            lay_out(&wide, 100.0),
            Err(LayoutError::Indent { paragraph: 0 })
        );

        let more = units(14);
        let hanging = [Paragraph {
            units: &more,
            empty_pitch: 12.0,
            first_indent: -25.0,
            split_words: true,
        }];
        let layout = lay_out(&hanging, 100.0).unwrap();
        let ranges: Vec<_> = layout.lines.iter().map(|line| line.units.clone()).collect();
        assert_eq!(ranges, vec![0..12, 12..14]);
    }

    #[test]
    fn hard_breaks_start_lines_and_an_empty_paragraph_keeps_its_line() {
        let first = units(3);
        let last = units(2);
        let paragraphs = [
            Paragraph {
                units: &first,
                empty_pitch: 12.0,
                first_indent: 0.0,
                split_words: true,
            },
            Paragraph {
                units: &[],
                empty_pitch: 12.0,
                first_indent: 0.0,
                split_words: true,
            },
            Paragraph {
                units: &last,
                empty_pitch: 12.0,
                first_indent: 0.0,
                split_words: true,
            },
        ];
        let layout = lay_out(&paragraphs, 100.0).unwrap();
        assert_eq!(layout.lines.len(), 3);
        assert_eq!(
            layout
                .lines
                .iter()
                .map(|line| line.paragraph)
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );
        assert_eq!(layout.lines[1].units, 0..0);
        assert_eq!(layout.height, 36.0);
        assert_every_unit_once(&paragraphs, &layout);
    }

    #[test]
    fn a_line_is_as_tall_as_its_tallest_unit() {
        let mut text = units(12);
        text[3].pitch = 30.0;
        let layout = lay_out(&one(&text), 100.0).unwrap();
        assert_eq!(layout.lines[0].pitch, 30.0);
        assert_eq!(layout.lines[1].pitch, 12.0);
        assert_eq!(layout.height, 42.0);
    }

    #[test]
    fn alignment_places_a_line_and_never_pushes_an_overflow_out_of_the_frame() {
        assert_eq!(Alignment::Start.offset(100.0, 40.0), 0.0);
        assert_eq!(Alignment::Centre.offset(100.0, 40.0), 30.0);
        assert_eq!(Alignment::End.offset(100.0, 40.0), 60.0);
        assert_eq!(Alignment::Justify.offset(100.0, 40.0), 0.0);
        for alignment in [
            Alignment::Start,
            Alignment::Centre,
            Alignment::End,
            Alignment::Justify,
        ] {
            assert_eq!(alignment.offset(100.0, 120.0), 0.0);
        }
    }

    #[test]
    fn input_that_cannot_be_laid_out_is_refused_by_name() {
        let text = units(2);
        assert_eq!(lay_out(&one(&text), 0.0), Err(LayoutError::Width(0.0)));
        assert!(matches!(
            lay_out(&one(&text), f64::NAN),
            Err(LayoutError::Width(_))
        ));
        let mut bad = units(2);
        bad[1].advance = f64::NAN;
        assert_eq!(
            lay_out(&one(&bad), 100.0),
            Err(LayoutError::Unit {
                paragraph: 0,
                unit: 1
            })
        );
        let empty = [Paragraph {
            units: &[],
            empty_pitch: 0.0,
            first_indent: 0.0,
            split_words: true,
        }];
        assert_eq!(
            lay_out(&empty, 100.0),
            Err(LayoutError::EmptyPitch { paragraph: 0 })
        );
    }

    fn marked(breaks: &[usize]) -> Vec<Unit> {
        let mut text: Vec<Unit> = (0..5)
            .map(|index| Unit {
                advance: 10.0,
                pitch: 12.0,
                break_before: breaks.contains(&index),
                hangs: false,
                spacing_after: 0.0,
            })
            .collect();
        text[2].advance = -4.0;
        text
    }

    #[test]
    fn a_glyph_moved_back_under_its_mark_lays_out_glued_to_it() {
        let text = marked(&[0, 4]);
        let paragraphs = one(&text);
        let layout = lay_out(&paragraphs, 100.0).expect("glued, it lays out");
        assert_eq!(layout.lines.len(), 1);
        assert_eq!(layout.lines[0].width, 36.0);
        assert_every_unit_once(&paragraphs, &layout);

        let narrow = lay_out(&paragraphs, 25.0).expect("glued, it lays out");
        let ends: Vec<usize> = narrow.lines.iter().map(|line| line.units.end).collect();
        assert_eq!(ends, [2, 5]);
        assert_every_unit_once(&paragraphs, &narrow);
        for line in &narrow.lines {
            assert!(text[line.units.end - 1].advance >= 0.0, "{ends:?}");
        }

        let starting = marked(&[0, 2, 4]);
        let paragraphs = one(&starting);
        let layout = lay_out(&paragraphs, 100.0).expect("its word moves forward");
        assert_eq!(layout.lines.len(), 1);
        assert_eq!(layout.lines[0].width, 36.0);
        assert_every_unit_once(&paragraphs, &layout);

        let mut ending = marked(&[0]);
        ending.truncate(3);
        let paragraphs = one(&ending);
        let layout = lay_out(&paragraphs, 100.0).expect("nothing follows the mark");
        assert_eq!(layout.lines.len(), 1);
        assert_eq!(layout.lines[0].width, 16.0);
        assert_every_unit_once(&paragraphs, &layout);
    }

    #[test]
    fn a_negative_advance_a_line_could_end_on_or_that_runs_backwards_is_refused() {
        assert_eq!(
            lay_out(&one(&marked(&[0, 3])), 100.0),
            Err(LayoutError::Backwards {
                paragraph: 0,
                unit: 2,
                ends_a_line: true
            })
        );

        let mut behind = marked(&[0, 2, 4]);
        behind[3].advance = 2.0;
        assert_eq!(
            lay_out(&one(&behind), 100.0),
            Err(LayoutError::Backwards {
                paragraph: 0,
                unit: 2,
                ends_a_line: false
            })
        );

        let backwards = vec![
            Unit {
                advance: -10.0,
                pitch: 12.0,
                break_before: false,
                hangs: false,
                spacing_after: 0.0,
            };
            3
        ];
        assert_eq!(
            lay_out(&one(&backwards), 100.0),
            Err(LayoutError::Backwards {
                paragraph: 0,
                unit: 0,
                ends_a_line: false
            })
        );
    }

    #[test]
    fn a_word_split_at_the_edge_never_leaves_a_line_moving_backwards() {
        let mut text = marked(&[0]);
        text.truncate(4);
        text[1].advance = -6.0;
        text[2].advance = 2.0;
        let paragraphs = one(&text);
        let layout = lay_out(&paragraphs, 5.0).expect("its word moves forward");
        let ends: Vec<usize> = layout.lines.iter().map(|line| line.units.end).collect();
        assert_eq!(ends, [1, 4]);
        assert_every_unit_once(&paragraphs, &layout);
        for line in &layout.lines {
            assert!(line.width >= 0.0, "{ends:?}");
        }
    }

    #[test]
    fn every_unit_lands_on_one_line_in_order_when_obstacles_are_present() {
        let text = units(40);
        let paragraphs = one(&text);
        let configurations: Vec<Vec<Blocked>> = vec![
            vec![],
            vec![Blocked {
                top: 0.0,
                bottom: 24.0,
                left: 0.0,
                right: 50.0,
            }],
            vec![Blocked {
                top: 0.0,
                bottom: 12.0,
                left: 0.0,
                right: 100.0,
            }],
            vec![
                Blocked {
                    top: 0.0,
                    bottom: 12.0,
                    left: 30.0,
                    right: 100.0,
                },
                Blocked {
                    top: 12.0,
                    bottom: 24.0,
                    left: 60.0,
                    right: 100.0,
                },
                Blocked {
                    top: 24.0,
                    bottom: 36.0,
                    left: 90.0,
                    right: 100.0,
                },
            ],
            vec![
                Blocked {
                    top: 5.0,
                    bottom: 5.0,
                    left: 0.0,
                    right: 50.0,
                },
                Blocked {
                    top: 0.0,
                    bottom: 10.0,
                    left: 60.0,
                    right: 60.0,
                },
                Blocked {
                    top: f64::NAN,
                    bottom: 10.0,
                    left: 0.0,
                    right: 10.0,
                },
                Blocked {
                    top: 0.0,
                    bottom: 24.0,
                    left: 20.0,
                    right: 40.0,
                },
            ],
        ];
        for blocked in &configurations {
            let layout = lay_out_around(&paragraphs, 100.0, blocked).unwrap();
            assert_every_unit_once(&paragraphs, &layout);
            for line in &layout.lines {
                assert!(line.left >= 0.0, "{blocked:?}: {line:?}");
                assert!(
                    line.left + line.width <= 100.0 + FIT_SLACK,
                    "{blocked:?}: {line:?}"
                );
            }
        }
        let unobstructed = lay_out_around(&paragraphs, 100.0, &configurations[0]).unwrap();
        assert!(unobstructed.lines.iter().all(|line| line.left == 0.0));
        assert_eq!(unobstructed, lay_out(&paragraphs, 100.0).unwrap());
    }

    #[test]
    fn an_obstacle_narrows_the_lines_it_blocks_and_frees_the_lines_below_it() {
        let text = units(30);
        let paragraphs = one(&text);
        let blocked = [Blocked {
            top: 0.0,
            bottom: 24.0,
            left: 0.0,
            right: 50.0,
        }];
        let layout = lay_out_around(&paragraphs, 100.0, &blocked).unwrap();
        assert_every_unit_once(&paragraphs, &layout);
        let summary: Vec<(Range<usize>, f64, f64)> = layout
            .lines
            .iter()
            .map(|line| (line.units.clone(), line.left, line.width))
            .collect();
        assert_eq!(
            summary,
            vec![
                (0..5, 50.0, 50.0),
                (5..10, 50.0, 50.0),
                (10..20, 0.0, 100.0),
                (20..30, 0.0, 100.0),
            ]
        );
    }

    #[test]
    fn a_band_blocked_edge_to_edge_is_skipped_and_the_text_resumes_below_it() {
        let text = units(10);
        let paragraphs = one(&text);
        let blocked = [Blocked {
            top: 0.0,
            bottom: 12.0,
            left: 0.0,
            right: 100.0,
        }];
        let layout = lay_out_around(&paragraphs, 100.0, &blocked).unwrap();
        assert_every_unit_once(&paragraphs, &layout);
        assert_eq!(layout.lines.len(), 1, "{:?}", layout.lines);
        assert_eq!(layout.lines[0].units, 0..10);
        assert_eq!(layout.lines[0].left, 0.0);
        assert_eq!(layout.lines[0].width, 100.0);
        assert!(!layout.lines[0].overflow);
        assert_eq!(layout.lines[0].top, 12.0);
        assert_eq!(layout.height, 24.0);
    }

    #[test]
    fn a_frame_with_nothing_in_the_way_is_as_tall_as_its_lines() {
        let text = units(10);
        let paragraphs = one(&text);
        let layout = lay_out(&paragraphs, 100.0).unwrap();
        assert_eq!(layout.lines.len(), 1);
        assert_eq!(layout.lines[0].top, 0.0);
        assert_eq!(layout.height, 12.0);
    }

    #[test]
    fn a_line_takes_the_widest_run_of_its_band_and_not_the_first() {
        let text = units(7);
        let paragraphs = one(&text);
        let blocked = [Blocked {
            top: 0.0,
            bottom: 12.0,
            left: 20.0,
            right: 50.0,
        }];
        let layout = lay_out_around(&paragraphs, 100.0, &blocked).unwrap();
        assert_every_unit_once(&paragraphs, &layout);
        assert_eq!(layout.lines[0].left, 50.0, "{:?}", layout.lines);
        assert_eq!(layout.lines[0].units, 0..5, "{:?}", layout.lines);
        assert_eq!(layout.lines[0].width, 50.0, "{:?}", layout.lines);
    }

    #[test]
    fn a_staircase_of_blocked_rows_gives_lines_of_increasing_width() {
        let text = units(18);
        let paragraphs = one(&text);
        let blocked = [
            Blocked {
                top: 0.0,
                bottom: 12.0,
                left: 30.0,
                right: 100.0,
            },
            Blocked {
                top: 12.0,
                bottom: 24.0,
                left: 60.0,
                right: 100.0,
            },
            Blocked {
                top: 24.0,
                bottom: 36.0,
                left: 90.0,
                right: 100.0,
            },
        ];
        let layout = lay_out_around(&paragraphs, 100.0, &blocked).unwrap();
        assert_every_unit_once(&paragraphs, &layout);
        let widths: Vec<f64> = layout.lines.iter().map(|line| line.width).collect();
        assert_eq!(widths, vec![30.0, 60.0, 90.0]);
        assert!(layout.lines.iter().all(|line| line.left == 0.0));
        assert_eq!(
            layout
                .lines
                .iter()
                .map(|line| line.units.clone())
                .collect::<Vec<_>>(),
            vec![0..3, 3..9, 9..18]
        );
    }

    #[test]
    fn justify_gaps_divides_the_spare_among_eligible_gaps() {
        let mut text = units(4);
        for (index, unit) in text.iter_mut().enumerate() {
            unit.break_before = index != 0;
        }
        let gaps = justify_gaps(&text, 46.0, 40.0);
        assert_eq!(gaps, vec![2.0, 2.0, 2.0, 0.0]);
    }

    #[test]
    fn justify_gaps_gives_nothing_with_no_break_or_no_spare() {
        let mut text = units(4);
        for unit in &mut text {
            unit.break_before = false;
        }
        assert_eq!(justify_gaps(&text, 46.0, 40.0), vec![0.0; 4]);

        let mut broken = units(4);
        for (index, unit) in broken.iter_mut().enumerate() {
            unit.break_before = index != 0;
        }
        assert_eq!(justify_gaps(&broken, 40.0, 40.0), vec![0.0; 4]);
        assert_eq!(justify_gaps(&broken, 30.0, 40.0), vec![0.0; 4]);
    }
}
