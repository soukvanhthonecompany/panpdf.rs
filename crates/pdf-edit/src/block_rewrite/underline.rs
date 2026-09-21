use super::{
    MeasuredRow, PaintAtomKind, PaintGraph, PlacedLine, PlannerPage, Point, SpikeError, Style,
    is_space, kern_between, kern_of, linear, unsupported,
};

pub(super) const UNDERLINE_OFFSET_EM: f64 = 0.12;

pub(super) const UNDERLINE_THICKNESS_EM: f64 = 0.06;

pub(super) const UNDERLINE_REACH_EM: f64 = 0.4;

pub(super) struct Rule {
    pub(super) run: usize,
    pub(super) left: f64,
    pub(super) right: f64,
    pub(super) bottom: f64,
    pub(super) top: f64,
}

pub(super) fn rules(style: &Style<'_>, lines: &[PlacedLine]) -> Vec<Rule> {
    let mut out = Vec::new();
    for line in lines {
        let end = line
            .pieces
            .iter()
            .rposition(|piece| !is_space(&piece.text))
            .map_or(0, |last| last + 1);
        let mut x = line.origin.x;
        let mut open: Option<Rule> = None;
        for (index, piece) in line.pieces.iter().enumerate() {
            let start = x;
            x += piece.advance + kern_between(style, piece, line.pieces.get(index + 1));
            if !(piece.underline && index < end) {
                out.extend(open.take());
                continue;
            }
            let end = start + piece.advance;
            if let Some(rule) = open.as_mut().filter(|rule| rule.run == piece.style) {
                rule.left = rule.left.min(end);
                rule.right = rule.right.max(end);
                continue;
            }
            out.extend(open.take());
            let em = style.run_em(piece.style);
            let top = line.origin.y - style.rule.0 * em;
            open = Some(Rule {
                run: piece.style,
                left: start.min(end),
                right: start.max(end),
                bottom: top - style.rule.1 * em,
                top,
            });
        }
        out.extend(open.take());
    }
    out
}

pub(super) fn underline_bytes(
    style: &Style<'_>,
    lines: &[PlacedLine],
) -> Result<Vec<u8>, SpikeError> {
    let rules = rules(style, lines);
    if rules.is_empty() {
        return Ok(Vec::new());
    }
    let inverse = style
        .ctm
        .inverse()
        .ok_or_else(|| unsupported("the block's transform cannot be inverted"))?;
    let mut body = b" ET q ".to_vec();
    for rule in &rules {
        let fill = style
            .run(rule.run)
            .fill
            .as_deref()
            .ok_or_else(|| unsupported("an underline in a colour that cannot be written"))?;
        let corner = |x, y| inverse.transform(Point { x, y });
        let upright = linear(style.ctm);
        if upright.b.abs() <= 1e-9 && upright.c.abs() <= 1e-9 {
            let (low, high) = (corner(rule.left, rule.bottom), corner(rule.right, rule.top));
            body.extend_from_slice(
                format!(
                    "{fill} {} {} {} {} re f ",
                    low.x,
                    low.y,
                    high.x - low.x,
                    high.y - low.y
                )
                .as_bytes(),
            );
        } else {
            let [one, two, three, four] = [
                corner(rule.left, rule.bottom),
                corner(rule.right, rule.bottom),
                corner(rule.right, rule.top),
                corner(rule.left, rule.top),
            ];
            body.extend_from_slice(
                format!(
                    "{fill} {} {} m {} {} l {} {} l {} {} l h f ",
                    one.x, one.y, two.x, two.y, three.x, three.y, four.x, four.y
                )
                .as_bytes(),
            );
        }
    }
    body.extend_from_slice(b"Q BT ");
    Ok(body)
}

pub(super) type StreamOperations<'a> = (&'a [pdf_content::Operation], &'a [u8]);

pub(super) fn adopt_underlines(
    style: &Style<'_>,
    graph: &PaintGraph,
    (stream, operations): (pdf_syntax::Reference, Option<StreamOperations<'_>>),
    mut measured: Vec<MeasuredRow>,
) -> (Vec<MeasuredRow>, Vec<usize>, Option<(f64, f64)>) {
    let mut owned = Vec::new();
    let mut shape = None;
    for (ordinal, atom) in graph.atoms.iter().enumerate() {
        let PaintAtomKind::Path(path) = &atom.kind else {
            continue;
        };
        if atom.id.stream != stream
            || !atom.id.invocation_path.is_empty()
            || !atom.id.pattern_path.is_empty()
            || path.fill.is_none()
            || path.stroke
            || !is_rectangle(path)
            || operations.is_some_and(|(operations, bytes)| {
                removable_span(operations, bytes, atom).is_none()
            })
        {
            continue;
        }
        let Some([left, bottom, right, top]) = atom.kind.user_bounds() else {
            continue;
        };
        let mut adopted = false;
        for row in &mut measured {
            let mut x = row.start;
            let mut covered: Vec<(usize, f64, f64)> = Vec::new();
            for index in 0..row.clusters.len() {
                let piece = &row.clusters[index];
                let (start, advance) = (x, piece.advance);
                x += advance;
                if piece.next.is_some()
                    && let Some(value) = piece.adjust.last()
                {
                    x += kern_of(style, piece.style, *value);
                }
                let middle = start + advance / 2.0;
                if (left..=right).contains(&middle) {
                    covered.push((
                        index,
                        start.min(start + advance),
                        start.max(start + advance),
                    ));
                }
            }
            let Some(first) = covered.first() else {
                continue;
            };
            let em = style.run_em(row.clusters[first.0].style);
            let from = covered
                .iter()
                .map(|one| one.1)
                .fold(f64::INFINITY, f64::min);
            let to = covered
                .iter()
                .map(|one| one.2)
                .fold(f64::NEG_INFINITY, f64::max);
            let thickness = top - bottom;
            let underneath = top <= row.baseline && top >= row.baseline - UNDERLINE_REACH_EM * em;
            let thin = thickness > 0.0 && thickness <= UNDERLINE_MAX_THICKNESS_EM * em;
            let ends = (left - from).abs() <= UNDERLINE_END_SLACK_EM * em
                && (right - to).abs() <= UNDERLINE_END_SLACK_EM * em;
            let coloured = covered
                .iter()
                .all(|one| same_fill(style.run(row.clusters[one.0].style).reference, path));
            if underneath && thin && ends && coloured {
                for (index, _, _) in covered {
                    row.clusters[index].underline = true;
                }
                adopted = true;
                shape.get_or_insert(((row.baseline - top) / em, thickness / em));
            }
        }
        if adopted {
            owned.push(ordinal);
        }
    }
    (measured, owned, shape)
}

const UNDERLINE_MAX_THICKNESS_EM: f64 = 0.15;

const UNDERLINE_END_SLACK_EM: f64 = 0.3;

fn same_fill(text: &pdf_paint::TextShowPaint, path: &pdf_paint::PathPaint) -> bool {
    text.state.fill_color_space.value == path.state.fill_color_space.value
        && pdf_paint::colour_signature(&text.state.fill_color.value)
            == pdf_paint::colour_signature(&path.state.fill_color.value)
}

fn is_rectangle(path: &pdf_paint::PathPaint) -> bool {
    use pdf_paint::PathSegment;
    let ctm = path.state.ctm.value;
    let points: Vec<pdf_paint::Point> = match path.path.segments.as_slice() {
        [
            PathSegment::Rectangle {
                origin,
                width,
                height,
                ..
            },
        ] => [
            (origin.x, origin.y),
            (origin.x + width, origin.y),
            (origin.x + width, origin.y + height),
            (origin.x, origin.y + height),
        ]
        .iter()
        .map(|(x, y)| pdf_paint::Point { x: *x, y: *y })
        .collect(),
        [
            PathSegment::MoveTo { point: p0, .. },
            PathSegment::LineTo { point: p1, .. },
            PathSegment::LineTo { point: p2, .. },
            PathSegment::LineTo { point: p3, .. },
            rest @ ..,
        ] => {
            let closes = match rest {
                [] | [PathSegment::ClosePath { .. }] => true,
                [PathSegment::LineTo { point, .. }]
                | [
                    PathSegment::LineTo { point, .. },
                    PathSegment::ClosePath { .. },
                ] => (point.x - p0.x).abs() <= 1e-9 && (point.y - p0.y).abs() <= 1e-9,
                _ => false,
            };
            if !closes {
                return false;
            }
            vec![*p0, *p1, *p2, *p3]
        }
        _ => return false,
    };
    let placed: Vec<pdf_paint::Point> = points.iter().map(|point| ctm.transform(*point)).collect();
    let scale = placed
        .iter()
        .map(|point| point.x.abs().max(point.y.abs()))
        .fold(1.0, f64::max);
    let upright = |a: &pdf_paint::Point, b: &pdf_paint::Point| {
        (a.x - b.x).abs() <= 1e-9 * scale || (a.y - b.y).abs() <= 1e-9 * scale
    };
    (0..4).all(|index| upright(&placed[index], &placed[(index + 1) % 4]))
}

const PATH_CONSTRUCTION: [&[u8]; 7] = [b"re", b"m", b"l", b"c", b"v", b"y", b"h"];

fn removable_span(
    operations: &[pdf_content::Operation],
    bytes: &[u8],
    atom: &pdf_paint::PaintAtom,
) -> Option<(usize, usize)> {
    let PaintAtomKind::Path(path) = &atom.kind else {
        return None;
    };
    let painted = operations
        .iter()
        .position(|operation| operation.operator_span() == atom.id.operator_span)?;
    let operator = |operation: &pdf_content::Operation| {
        let span = operation.operator_span();
        bytes.get(span.start()..span.end())
    };
    let mut first = painted;
    while first > 0
        && operator(&operations[first - 1]).is_some_and(|op| PATH_CONSTRUCTION.contains(&op))
    {
        first -= 1;
    }
    (painted - first == path.path.segments.len()).then(|| {
        (
            operations[first].span().start(),
            operations[painted].span().end(),
        )
    })
}

pub(super) fn underline_spans(
    page: PlannerPage<'_>,
    stream_index: usize,
    graph: &PaintGraph,
    owned: &[usize],
) -> Result<Vec<(usize, usize)>, SpikeError> {
    let unremovable = || unsupported("an underline's operators cannot be taken out of the stream");
    let operations = &page.operations[stream_index];
    let bytes = page.program.streams[stream_index].bytes.as_bytes();
    let mut spans = Vec::with_capacity(owned.len());
    for ordinal in owned {
        let span =
            removable_span(operations, bytes, &graph.atoms[*ordinal]).ok_or_else(unremovable)?;
        spans.push(span);
    }
    spans.sort_unstable();
    Ok(widen_to_emptied_groups(operations, bytes, spans))
}

const COLOUR_OPERATORS: [&[u8]; 12] = [
    b"g", b"G", b"rg", b"RG", b"k", b"K", b"cs", b"CS", b"sc", b"SC", b"scn", b"SCN",
];

fn widen_to_emptied_groups(
    operations: &[pdf_content::Operation],
    bytes: &[u8],
    mut spans: Vec<(usize, usize)>,
) -> Vec<(usize, usize)> {
    let operator = |operation: &pdf_content::Operation| {
        let span = operation.operator_span();
        bytes.get(span.start()..span.end()).unwrap_or_default()
    };
    let removed = |spans: &[(usize, usize)], operation: &pdf_content::Operation| {
        let span = operation.span();
        spans
            .iter()
            .any(|(start, end)| *start <= span.start() && span.end() <= *end)
    };
    loop {
        let mut widened = None;
        'groups: for (open, opening) in operations.iter().enumerate() {
            let closer: &[u8] = match operator(opening) {
                b"q" => b"Q",
                b"BMC" | b"BDC" => b"EMC",
                _ => continue,
            };
            if removed(&spans, opening) {
                continue;
            }
            let mut holds_removed = false;
            for (offset, inner) in operations[open + 1..].iter().enumerate() {
                let name = operator(inner);
                if name == closer {
                    if holds_removed {
                        widened = Some((
                            opening.span().start(),
                            inner.span().end(),
                            open,
                            open + 1 + offset,
                        ));
                    }
                    if widened.is_some() {
                        break 'groups;
                    }
                    continue 'groups;
                }
                if removed(&spans, inner) {
                    holds_removed = true;
                } else if !COLOUR_OPERATORS.contains(&name) {
                    continue 'groups;
                }
            }
        }
        let Some((start, end, _, _)) = widened else {
            return spans;
        };
        spans.retain(|(one, other)| !(start <= *one && *other <= end));
        spans.push((start, end));
        spans.sort_unstable();
    }
}
