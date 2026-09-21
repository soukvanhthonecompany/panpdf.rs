use super::{
    BTreeSet, Matrix, Outside, PaintAtomKind, PaintGraph, Piece, PlacedLine, PlannerPage, Run,
    SCALE_SLACK, Show, SpikeError, Style, TextShowPaint, fills_with, kern_between, kern_of, lift,
    same_colour, same_setting, segments, set_in, shape_against, sheared, state_change,
    underline_bytes, underline_spans, unsupported,
};

pub(super) fn shows(segment: &[Piece]) -> Vec<Show> {
    let mut out: Vec<Show> = Vec::new();
    for (index, piece) in segment.iter().enumerate() {
        for position in 0..piece.codes.len() {
            let rise = piece.rise.get(position).copied().unwrap_or(0.0);
            match out.last_mut() {
                Some((last, codes)) if last.to_bits() == rise.to_bits() => {
                    codes.push((index, position));
                }
                _ => out.push((rise, vec![(index, position)])),
            }
        }
    }
    if out.is_empty() {
        out.push((0.0, Vec::new()));
    }
    out
}

pub(super) fn show_segment(
    style: &Style<'_>,
    (segment, gaps): (&[Piece], &[f64]),
    following: Option<&Piece>,
    carry: f64,
) -> (Vec<u8>, usize, f64) {
    let base = segment
        .first()
        .map_or(0.0, |piece| style.run(piece.style).text.rise.value);
    let per = segment
        .first()
        .map_or(0.0, |piece| kern_of(style, piece.style, 1.0));
    let groups = shows(segment);
    let raised = groups.iter().any(|(rise, _)| *rise != 0.0);
    let mut body = Vec::new();
    let mut carry = carry;
    for (rise, codes) in &groups {
        if raised {
            body.extend_from_slice(format!("{} Ts ", base + rise).as_bytes());
        }
        body.push(b'[');
        if carry != 0.0 && per != 0.0 {
            body.extend_from_slice(format!("{} ", carry / per).as_bytes());
        }
        carry = 0.0;
        body.push(b'<');
        for (at, &(index, position)) in codes.iter().enumerate() {
            let piece = &segment[index];
            for byte in &piece.codes[position].bytes {
                body.extend_from_slice(format!("{byte:02X}").as_bytes());
            }
            let value = if position + 1 < piece.codes.len() || !piece.shaped.is_empty() {
                piece.adjust.get(position).copied().unwrap_or(0.0)
            } else if kern_between(style, piece, segment.get(index + 1).or(following)) == 0.0 {
                0.0
            } else {
                piece.adjust.last().copied().unwrap_or(0.0)
            };
            let value = value + gap_number(style, piece, gaps.get(index).copied().unwrap_or(0.0));
            if value == 0.0 {
                continue;
            }
            if at + 1 < codes.len() {
                body.extend_from_slice(format!("> {value} <").as_bytes());
            } else {
                carry = kern_of(style, piece.style, value);
            }
        }
        body.extend_from_slice(b">] TJ ");
    }
    if raised {
        body.extend_from_slice(format!("{base} Ts ").as_bytes());
    }
    (body, groups.len(), carry)
}

fn gap_number(style: &Style<'_>, piece: &Piece, gap: f64) -> f64 {
    if gap == 0.0 {
        return 0.0;
    }
    let per = kern_of(style, piece.style, 1.0);
    if per == 0.0 { 0.0 } else { gap / per }
}

pub(super) fn write_lines<'a, 'g>(
    style: &'a Style<'g>,
    lines: &[PlacedLine],
    inverse: Matrix,
    body: &mut Vec<u8>,
) -> (usize, &'a Run<'g>) {
    let tm = style.tm_linear;
    let mut produced = 0;
    let mut current = style.base();
    for line in lines.iter().filter(|line| !line.pieces.is_empty()) {
        let at = inverse.transform(line.origin);
        body.extend_from_slice(
            format!("{} {} {} {} {} {} Tm ", tm.a, tm.b, tm.c, tm.d, at.x, at.y).as_bytes(),
        );
        let mut x = line.origin.x;
        let mut slant = 0.0;
        let mut carry = 0.0;
        let parts = segments(&line.pieces);
        let mut from = 0;
        for (number, segment) in parts.iter().copied().enumerate() {
            let following = parts.get(number + 1).and_then(|next| next.first());
            let gaps = line.gaps.get(from..from + segment.len()).unwrap_or(&[]);
            from += segment.len();
            let run = style.run(segment[0].style);
            if run.shear.to_bits() != f64::to_bits(slant) {
                let at = inverse.transform(pdf_paint::Point {
                    x,
                    y: line.origin.y,
                });
                let m = sheared(tm, run.shear);
                body.extend_from_slice(
                    format!("{} {} {} {} {} {} Tm ", m.a, m.b, m.c, m.d, at.x, at.y).as_bytes(),
                );
                slant = run.shear;
                carry = 0.0;
            }
            for (index, piece) in segment.iter().enumerate() {
                x += piece.advance
                    + kern_between(style, piece, segment.get(index + 1).or(following))
                    + gaps.get(index).copied().unwrap_or(0.0);
            }
            body.extend_from_slice(state_change(current, run).as_bytes());
            current = run;
            let (shown, count, left) = show_segment(style, (segment, gaps), following, carry);
            carry = left;
            body.extend_from_slice(&shown);
            produced += count;
        }
    }
    if produced == 0
        && let Some(line) = lines.first()
    {
        let at = inverse.transform(line.origin);
        body.extend_from_slice(
            format!(
                "{} {} {} {} {} {} Tm [<>] TJ ",
                tm.a, tm.b, tm.c, tm.d, at.x, at.y
            )
            .as_bytes(),
        );
        produced = 1;
    }
    (produced, current)
}

pub(super) fn write_stream(
    page: PlannerPage<'_>,
    stream_index: usize,
    graph: &PaintGraph,
    (named, outside, underlined): (&BTreeSet<usize>, &Outside, &[usize]),
    style: &Style<'_>,
    lines: &[PlacedLine],
    (pitch, clips): (f64, &[lift::ClipEdit]),
) -> Result<(Vec<u8>, usize), SpikeError> {
    let inverse = style
        .ctm
        .inverse()
        .ok_or_else(|| unsupported("the block's transform cannot be inverted"))?;
    let base = style.base();
    let mut body = format!(" {} TL ", pitch / style.to_user.d).into_bytes();
    let (produced, current) = write_lines(style, lines, inverse, &mut body);
    body.extend_from_slice(state_change(current, base).as_bytes());
    body.extend_from_slice(format!("{} TL ", base.reference.state.text.leading.value).as_bytes());
    body.extend_from_slice(&underline_bytes(style, lines)?);
    let mut spans: Vec<(usize, usize, Vec<u8>)> =
        underline_spans(page, stream_index, graph, underlined)?
            .into_iter()
            .map(|(start, end)| (start, end, b" ".to_vec()))
            .chain(clips.iter().cloned())
            .collect();
    for (position, ordinal) in named.iter().enumerate() {
        let atom = &graph.atoms[*ordinal];
        let operation = page.operations[stream_index]
            .iter()
            .find(|operation| operation.operator_span() == atom.id.operator_span)
            .ok_or(SpikeError::SelectionNamesNoRun)?;
        let mut replacement = if position == 0 {
            body.clone()
        } else {
            b" ".to_vec()
        };
        if let Some(ranges) = outside.get(ordinal) {
            let PaintAtomKind::Text(text) = &atom.kind else {
                return Err(SpikeError::SelectionNamesNoRun);
            };
            replacement.extend_from_slice(&crate::split::kept_in_place(text, ranges)?);
        }
        if let Some(matrix) = matrix_after(
            &page.operations[stream_index],
            page.program.streams[stream_index].bytes.as_bytes(),
            (graph, named),
            style,
            *ordinal,
        )? {
            replacement.extend_from_slice(
                format!(
                    " {} {} {} {} {} {} Tm ",
                    matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f
                )
                .as_bytes(),
            );
        }
        spans.push((
            operation.span().start(),
            operation.span().end(),
            replacement,
        ));
    }
    spans.extend(actual_text_around(
        &page.operations[stream_index],
        page.program.streams[stream_index].bytes.as_bytes(),
        graph,
        named,
    ));
    spans.sort_by_key(|(start, _, _)| *start);
    let decoded = page.program.streams[stream_index].bytes.as_bytes();
    let mut edited = Vec::with_capacity(decoded.len() + body.len());
    let mut cursor = 0;
    for (start, end, replacement) in spans {
        if start < cursor || end < start || end > decoded.len() {
            return Err(SpikeError::SelectionNamesNoRun);
        }
        edited.extend_from_slice(&decoded[cursor..start]);
        edited.extend_from_slice(&replacement);
        cursor = end;
    }
    edited.extend_from_slice(&decoded[cursor..]);
    Ok((edited, produced))
}

fn actual_text_around(
    operations: &[pdf_content::Operation],
    bytes: &[u8],
    graph: &PaintGraph,
    named: &BTreeSet<usize>,
) -> Vec<(usize, usize, Vec<u8>)> {
    let named_spans: Vec<_> = named
        .iter()
        .map(|ordinal| graph.atoms[*ordinal].id.operator_span)
        .collect();
    let text_of =
        |span: pdf_bytes::SourceSpan| bytes.get(span.start()..span.end()).unwrap_or_default();
    let mut open: Vec<(usize, bool)> = Vec::new();
    let mut rewritten = Vec::new();
    for (index, operation) in operations.iter().enumerate() {
        match text_of(operation.operator_span()) {
            b"BDC" | b"BMC" => open.push((index, false)),
            b"EMC" => {
                if let Some((opening, true)) = open.pop() {
                    let begin = &operations[opening];
                    let [tag, properties] = begin.operands() else {
                        continue;
                    };
                    if text_of(begin.operator_span()) != b"BDC"
                        || !text_of(properties.span())
                            .windows(b"/ActualText".len())
                            .any(|window| window == b"/ActualText")
                    {
                        continue;
                    }
                    let mut replacement = b" ".to_vec();
                    replacement.extend_from_slice(text_of(tag.span()));
                    replacement.extend_from_slice(b" BMC ");
                    rewritten.push((begin.span().start(), begin.span().end(), replacement));
                }
            }
            _ if named_spans.contains(&operation.operator_span()) => {
                for entry in &mut open {
                    entry.1 = true;
                }
            }
            _ => {}
        }
    }
    rewritten
}

pub(super) fn matrix_after(
    operations: &[pdf_content::Operation],
    bytes: &[u8],
    (graph, named): (&PaintGraph, &BTreeSet<usize>),
    style: &Style<'_>,
    ordinal: usize,
) -> Result<Option<Matrix>, SpikeError> {
    let atom = &graph.atoms[ordinal];
    let PaintAtomKind::Text(text) = &atom.kind else {
        return Err(SpikeError::SelectionNamesNoRun);
    };
    let position = operations
        .iter()
        .position(|operation| operation.operator_span() == atom.id.operator_span)
        .ok_or(SpikeError::SelectionNamesNoRun)?;
    let named_spans: Vec<_> = named
        .iter()
        .map(|other| graph.atoms[*other].id.operator_span)
        .collect();
    for operation in &operations[position + 1..] {
        let span = operation.operator_span();
        if named_spans.contains(&span) {
            return Ok(None);
        }
        match bytes.get(span.start()..span.end()) {
            Some(b"Td" | b"TD" | b"T*" | b"'" | b"\"") => {
                return Ok(Some(text.matrices.line.value));
            }
            Some(b"Tj" | b"TJ") => {
                let run = style.run(style.run_of.get(&ordinal).copied().unwrap_or(0));
                let (_, end) = pdf_paint::position_text(
                    &text.state.text,
                    &text.elements,
                    text.matrices.text.value,
                    text.program.as_deref(),
                    &run.font,
                    0.001,
                );
                return Ok(Some(end));
            }
            Some(b"Tm" | b"BT" | b"ET") => return Ok(None),
            _ => {}
        }
    }
    Ok(None)
}

pub(super) fn written_as_run(
    style: &Style<'_>,
    run: usize,
    written: &TextShowPaint,
    rise: f64,
) -> bool {
    let run = style.run(run);
    shape_against(style.tm_linear, written.matrices.text.value)
        .filter(|(_, _, shear)| (shear - run.shear).abs() <= SCALE_SLACK)
        .map(|(scale, stretch, _)| {
            let mut set = set_in(written, scale, stretch);
            let wanted = run.text.rise.value + rise;
            if rise != 0.0 && (set.rise.value - wanted).abs() <= 1e-6 * wanted.abs().max(1.0) {
                set.rise.value = run.text.rise.value;
            }
            set
        })
        .is_some_and(|set| {
            if run.fill_rgb.is_none() && !run.stroke_like_fill {
                return same_setting((run.reference, &run.text), (written, &set), true);
            }
            let mut reference = run.reference.clone();
            reference.state.line_width.value = run.line_width;
            same_setting((&reference, &run.text), (written, &set), false)
                && match run.fill_rgb {
                    Some(rgb) => fills_with(written, rgb),
                    None => same_colour(run.reference, written, false),
                }
                && if run.stroke_like_fill {
                    written.state.stroke_color_space.value == written.state.fill_color_space.value
                        && pdf_paint::colour_signature(&written.state.stroke_color.value)
                            == pdf_paint::colour_signature(&written.state.fill_color.value)
                } else {
                    same_colour(run.reference, written, true)
                }
        })
}
