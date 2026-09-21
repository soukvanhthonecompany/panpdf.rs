use super::{
    BTreeMap, BTreeSet, ClusterKey, Matrix, Outside, PLACEMENT_TOLERANCE, PaintAtomKind,
    PaintGraph, Piece, PlacedLine, Range, Rule, SpikeError, Style, TextShowPaint, advance_of,
    kern_between, kern_of, linear, pen_of, rules, same_setting, segments, shows, text_of,
    unsupported, written_as_run,
};

pub(super) struct Proved {
    pub(super) mapping: BTreeMap<ClusterKey, ClusterKey>,
    pub(super) cropped: bool,
    pub(super) inserted: Vec<(ClusterKey, ClusterKey)>,
    pub(super) region: Option<[f64; 4]>,
    pub(super) keys: Vec<Vec<(ClusterKey, ClusterKey)>>,
}

pub(super) fn prove(
    before: &PaintGraph,
    after: &PaintGraph,
    (named, outside, underlined): (&BTreeSet<usize>, &Outside, &[usize]),
    style: &Style<'_>,
    lines: &[PlacedLine],
    produced: usize,
    owner: ClusterKey,
) -> Result<Proved, SpikeError> {
    let first = *named.iter().next().ok_or(SpikeError::BlockNamesNoRun)?;
    let mut mapping = BTreeMap::new();
    let mut inserted = Vec::new();
    let mut keys = vec![Vec::new(); lines.len()];
    let mut cropped = false;
    let mut region: Option<[f64; 4]> = None;
    let mut bounded = true;
    let mut widen = |bounds: Option<[f64; 4]>, text: &TextShowPaint| match bounds {
        Some(extent) => {
            region = Some(region.map_or(extent, |had| union(had, extent)));
        }
        None if text.draws_no_ink() => {}
        None => bounded = false,
    };
    let mut rewritten = after.atoms.iter().enumerate();
    let mut painted: BTreeMap<usize, (pdf_paint::ColorSpace, String)> = BTreeMap::new();
    for (ordinal, one) in before.atoms.iter().enumerate() {
        if underlined.contains(&ordinal) {
            continue;
        }
        if named.contains(&ordinal) {
            let text = text_of(before, ordinal)?;
            widen(text.outline_bounds(), text);
            if ordinal != first {
                prove_kept(
                    text,
                    ordinal,
                    outside.get(&ordinal),
                    &mut rewritten,
                    &mut mapping,
                )?;
                continue;
            }
            let written = lines
                .iter()
                .enumerate()
                .filter(|(_, line)| !line.pieces.is_empty());
            check_produced(lines, produced)?;
            for (index, line) in written {
                let mut x = line.origin.x;
                let parts = segments(&line.pieces);
                let mut from = 0;
                for (number, segment) in parts.iter().copied().enumerate() {
                    let following = parts.get(number + 1).and_then(|next| next.first());
                    let gaps = line.gaps.get(from..from + segment.len()).unwrap_or(&[]);
                    from += segment.len();
                    let groups = shows(segment);
                    let atoms = written_shows(&mut rewritten, style, segment, &groups)?;
                    for (_, other) in &atoms {
                        painted
                            .entry(segment[0].style)
                            .or_insert_with(|| fill_of(other));
                        cropped |= clip_admits_moved(before, named, other)?;
                        widen(other.outline_bounds(), other);
                    }
                    let (segment_keys, end) = prove_segment(
                        style,
                        (segment, gaps, following),
                        (x, line.origin.y),
                        (&groups, &atoms),
                        owner,
                        &mut mapping,
                        &mut inserted,
                    )?;
                    keys[index].extend(segment_keys);
                    x = end;
                }
            }
            if !lines.is_empty() && lines.iter().all(|line| line.pieces.is_empty()) {
                let at = prove_empty_show(&mut rewritten, style, lines)?;
                inserted.push((owner, ClusterKey { atom: at, glyph: 0 }));
            }
            for rule in rules(style, lines) {
                let bounds = prove_rule(&rule, &mut rewritten, &painted)?;
                widen(Some(bounds), text);
            }
            prove_kept(
                text,
                ordinal,
                outside.get(&ordinal),
                &mut rewritten,
                &mut mapping,
            )?;
            continue;
        }
        prove_unchanged(ordinal, one, &mut rewritten, &mut mapping)?;
    }
    if rewritten.next().is_some() {
        return Err(unsupported(
            "the rewritten page paints more than the layout wrote",
        ));
    }
    Ok(Proved {
        mapping,
        cropped,
        inserted,
        region: region.filter(|_| bounded),
        keys,
    })
}

fn prove_empty_show<'a>(
    rewritten: &mut impl Iterator<Item = (usize, &'a pdf_paint::PaintAtom)>,
    style: &Style<'_>,
    lines: &[PlacedLine],
) -> Result<usize, SpikeError> {
    let misplaced = || unsupported("the emptied block's show is not where the layout put it");
    let (at, written) = rewritten.next().ok_or_else(misplaced)?;
    let PaintAtomKind::Text(text) = &written.kind else {
        return Err(misplaced());
    };
    let origin = lines.first().ok_or_else(misplaced)?.origin;
    let pen = text.state.ctm.value.transform(pdf_paint::Point {
        x: text.matrices.text.value.e,
        y: text.matrices.text.value.f,
    });
    let base = style.base();
    let same_font = text.state.text.font.as_ref().map(|font| &font.value.name)
        == base
            .reference
            .state
            .text
            .font
            .as_ref()
            .map(|font| &font.value.name);
    if !text.glyphs.is_empty()
        || !same_font
        || (text.state.text.font_size.value - base.text.font_size.value).abs() > 1e-9
        || (pen.x - origin.x).abs() > PLACEMENT_TOLERANCE
        || (pen.y - origin.y).abs() > PLACEMENT_TOLERANCE
    {
        return Err(misplaced());
    }
    Ok(at)
}

pub(super) fn check_produced(lines: &[PlacedLine], produced: usize) -> Result<(), SpikeError> {
    let laid: usize = lines
        .iter()
        .filter(|line| !line.pieces.is_empty())
        .flat_map(|line| segments(&line.pieces))
        .map(|segment| shows(segment).len())
        .sum();
    let laid = if laid == 0 && !lines.is_empty() {
        1
    } else {
        laid
    };
    if laid == produced {
        Ok(())
    } else {
        Err(unsupported(
            "the rewrite wrote a different number of show operations than it laid out",
        ))
    }
}

pub(super) fn fill_of(text: &TextShowPaint) -> (pdf_paint::ColorSpace, String) {
    (
        text.state.fill_color_space.value.clone(),
        pdf_paint::colour_signature(&text.state.fill_color.value),
    )
}

pub(super) fn prove_rule<'a>(
    rule: &Rule,
    rewritten: &mut impl Iterator<Item = (usize, &'a pdf_paint::PaintAtom)>,
    painted: &BTreeMap<usize, (pdf_paint::ColorSpace, String)>,
) -> Result<[f64; 4], SpikeError> {
    let misplaced = || unsupported("an underline is not where the layout put it");
    let (_, drawn) = rewritten.next().ok_or_else(misplaced)?;
    let PaintAtomKind::Path(path) = &drawn.kind else {
        return Err(misplaced());
    };
    let bounds = drawn.kind.user_bounds().ok_or_else(misplaced)?;
    let close =
        |one: f64, other: f64| (one - other).abs() <= PLACEMENT_TOLERANCE.max(1e-6 * one.abs());
    let colour = (
        path.state.fill_color_space.value.clone(),
        pdf_paint::colour_signature(&path.state.fill_color.value),
    );
    if path.fill.is_none()
        || path.stroke
        || !(close(bounds[0], rule.left)
            && close(bounds[1], rule.bottom)
            && close(bounds[2], rule.right)
            && close(bounds[3], rule.top))
        || painted.get(&rule.run) != Some(&colour)
    {
        return Err(misplaced());
    }
    Ok(bounds)
}

pub(super) fn clip_admits_moved(
    before: &PaintGraph,
    named: &BTreeSet<usize>,
    written: &TextShowPaint,
) -> Result<bool, SpikeError> {
    if written.state.clip_paths.is_empty() {
        return Ok(false);
    }
    let Ok(regions) = clip_regions(written) else {
        return Ok(false);
    };
    let originals: Vec<(&TextShowPaint, &pdf_paint::PositionedGlyph)> = named
        .iter()
        .filter_map(|ordinal| match &before.atoms.get(*ordinal)?.kind {
            PaintAtomKind::Text(text) => Some(text),
            _ => None,
        })
        .flat_map(|text| text.glyphs.iter().map(move |glyph| (text, glyph)))
        .collect();
    for (index, glyph) in written.glyphs.iter().enumerate() {
        let pen = pen_of(written.state.ctm.value, glyph.text_matrix);
        let stayed = originals.iter().any(|(text, was)| {
            let then = pen_of(text.state.ctm.value, was.text_matrix);
            was.code.bytes == glyph.code.bytes
                && (then.x - pen.x).abs() <= PLACEMENT_TOLERANCE
                && (then.y - pen.y).abs() <= PLACEMENT_TOLERANCE
                && text.state.ctm.value == written.state.ctm.value
                && linear(was.matrix) == linear(glyph.matrix)
                && clip_regions(text).is_ok_and(|then| same_regions(&then, &regions))
        });
        if stayed {
            continue;
        }
        let Some(points) = written.outline_points_in(index..index + 1) else {
            if written.draws_no_ink_in(index..index + 1) {
                continue;
            }
            return Err(SpikeError::RunExtentUnknown);
        };
        if regions.iter().any(|(region, _)| !region.admits(&points)) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn clip_regions(
    text: &TextShowPaint,
) -> Result<Vec<(crate::clip_region::ClipRegion, pdf_paint::FillRule)>, SpikeError> {
    text.state
        .clip_paths
        .iter()
        .map(|clip| {
            Ok((
                crate::clip_region::ClipRegion::of(&clip.path, clip.ctm.value)?,
                clip.rule,
            ))
        })
        .collect()
}

pub(super) fn same_regions(
    one: &[(crate::clip_region::ClipRegion, pdf_paint::FillRule)],
    other: &[(crate::clip_region::ClipRegion, pdf_paint::FillRule)],
) -> bool {
    one.len() == other.len()
        && one
            .iter()
            .zip(other)
            .all(|((a, rule_a), (b, rule_b))| rule_a == rule_b && a.parts() == b.parts())
}

pub(super) const fn outside_run_changes(change: crate::block_move::RunChange) -> &'static str {
    use crate::block_move::RunChange;
    match change {
        RunChange::GlyphCount => {
            "a text run outside the block would paint another number of glyphs"
        }
        RunChange::RenderingMode => "a text run outside the block would change its rendering mode",
        RunChange::Transform => "a text run outside the block would change its transform",
        RunChange::FillColour => "a text run outside the block would change its fill colour",
        RunChange::StrokeColour => "a text run outside the block would change its stroke colour",
        RunChange::Glyphs => "a text run outside the block would paint other glyphs",
        RunChange::Shape => "a text run outside the block would scale, turn or slant a glyph",
        RunChange::Place => "a text run outside the block would move a glyph",
    }
}

pub(super) fn prove_unchanged<'a>(
    ordinal: usize,
    one: &pdf_paint::PaintAtom,
    rewritten: &mut impl Iterator<Item = (usize, &'a pdf_paint::PaintAtom)>,
    mapping: &mut BTreeMap<ClusterKey, ClusterKey>,
) -> Result<(), SpikeError> {
    let (after_atom, other) = rewritten
        .next()
        .ok_or_else(|| unsupported("the rewritten page ends before paint outside the block"))?;
    match (&one.kind, &other.kind) {
        (PaintAtomKind::Text(one), PaintAtomKind::Text(other)) => {
            if let Some(change) = crate::block_move::run_change(one, other, 0.0, 0.0) {
                return Err(unsupported(outside_run_changes(change)));
            }
            for glyph in 0..one.glyphs.len().max(1) {
                mapping.insert(
                    ClusterKey {
                        atom: ordinal,
                        glyph,
                    },
                    ClusterKey {
                        atom: after_atom,
                        glyph,
                    },
                );
            }
        }
        (one, other) => {
            if pdf_paint::paint_signature(one) != pdf_paint::paint_signature(other) {
                return Err(unsupported("paint outside the block would change"));
            }
        }
    }
    Ok(())
}

pub(super) fn prove_kept<'a>(
    one: &TextShowPaint,
    ordinal: usize,
    ranges: Option<&Vec<Range<usize>>>,
    rewritten: &mut impl Iterator<Item = (usize, &'a pdf_paint::PaintAtom)>,
    mapping: &mut BTreeMap<ClusterKey, ClusterKey>,
) -> Result<(), SpikeError> {
    for range in ranges.into_iter().flatten() {
        let (after_atom, other) = rewritten.next().ok_or_else(|| {
            unsupported("another block's glyphs this run paints were not written back")
        })?;
        let PaintAtomKind::Text(other) = &other.kind else {
            return Err(unsupported(
                "another block's glyphs this run paints were not written back",
            ));
        };
        let kept = one
            .glyphs
            .get(range.clone())
            .ok_or(SpikeError::GlyphRangeOutsideRun)?;
        if other.glyphs.len() != kept.len()
            || !same_setting((one, &one.state.text), (other, &other.state.text), true)
        {
            return Err(unsupported(
                "another block's glyphs this run paints would change their setting",
            ));
        }
        for (index, (was, now)) in kept.iter().zip(&other.glyphs).enumerate() {
            if was.code.bytes != now.code.bytes || !same_placement(was.matrix, now.matrix) {
                return Err(unsupported(
                    "another block's glyphs this run paints would move",
                ));
            }
            mapping.insert(
                ClusterKey {
                    atom: ordinal,
                    glyph: range.start + index,
                },
                ClusterKey {
                    atom: after_atom,
                    glyph: index,
                },
            );
        }
    }
    Ok(())
}

pub(super) fn same_placement(one: Matrix, other: Matrix) -> bool {
    [
        one.a - other.a,
        one.b - other.b,
        one.c - other.c,
        one.d - other.d,
        one.e - other.e,
        one.f - other.f,
    ]
    .iter()
    .all(|difference| difference.abs() <= PLACEMENT_TOLERANCE)
}

pub(super) type Show = (f64, Vec<(usize, usize)>);

pub(super) type Written<'a> = (usize, &'a TextShowPaint);

pub(super) fn written_shows<'a>(
    rewritten: &mut impl Iterator<Item = (usize, &'a pdf_paint::PaintAtom)>,
    style: &Style<'_>,
    segment: &[Piece],
    groups: &[Show],
) -> Result<Vec<Written<'a>>, SpikeError> {
    let mut atoms = Vec::with_capacity(groups.len());
    for (rise, _) in groups {
        let (after_atom, other) = rewritten
            .next()
            .ok_or_else(|| unsupported("the rewritten page ends before a line the layout wrote"))?;
        let PaintAtomKind::Text(other) = &other.kind else {
            return Err(unsupported(
                "the rewritten page paints something else where a line was written",
            ));
        };
        if !written_as_run(style, segment[0].style, other, *rise) {
            return Err(unsupported("a written line is not set the way its run was"));
        }
        atoms.push((after_atom, other));
    }
    Ok(atoms)
}

pub(super) fn prove_segment(
    style: &Style<'_>,
    (pieces, gaps, following): (&[Piece], &[f64], Option<&Piece>),
    (start, y): (f64, f64),
    (groups, atoms): (&[Show], &[Written<'_>]),
    owner: ClusterKey,
    mapping: &mut BTreeMap<ClusterKey, ClusterKey>,
    inserted: &mut Vec<(ClusterKey, ClusterKey)>,
) -> Result<(Vec<(ClusterKey, ClusterKey)>, f64), SpikeError> {
    let mut shown: Vec<Vec<(usize, usize)>> = vec![Vec::new(); pieces.len()];
    for (show, (_, codes)) in groups.iter().enumerate() {
        for (glyph, (index, _)) in codes.iter().enumerate() {
            shown[*index].push((show, glyph));
        }
    }
    for ((_, codes), (_, other)) in groups.iter().zip(atoms) {
        if codes.len() != other.glyphs.len() {
            return Err(unsupported(
                "a written line paints a different number of glyphs than the layout gave it",
            ));
        }
    }
    let mut x = start;
    let mut keys = Vec::with_capacity(pieces.len());
    let mut next = (0, 0);
    for (index, piece) in pieces.iter().enumerate() {
        let (show, glyph) = shown[index].first().copied().unwrap_or(next);
        let key = ClusterKey {
            atom: atoms.get(show).map_or(atoms[0].0, |(atom, _)| *atom),
            glyph,
        };
        let last = shown[index]
            .last()
            .and_then(|(show, glyph)| {
                atoms.get(*show).map(|(atom, _)| ClusterKey {
                    atom: *atom,
                    glyph: *glyph,
                })
            })
            .unwrap_or(key);
        keys.push((key, last));
        match piece.kept {
            Some(old) => {
                mapping.insert(old, key);
            }
            None => inserted.push((owner, key)),
        }
        for (other, at) in shown[index].iter().copied().skip(1) {
            if let Some((atom, _)) = atoms.get(other) {
                inserted.push((
                    owner,
                    ClusterKey {
                        atom: *atom,
                        glyph: at,
                    },
                ));
            }
        }
        let mut pen_x = x;
        for (position, code) in piece.codes.iter().enumerate() {
            let (show, glyph) = shown[index][position];
            let other = atoms[show].1;
            let placed = other.glyphs.get(glyph).ok_or_else(|| {
                unsupported("a written line paints fewer glyphs than the layout gave it")
            })?;
            next = (show, glyph + 1);
            let pen = pen_of(other.state.ctm.value, placed.text_matrix);
            if placed.code.value != code.value {
                return Err(unsupported(
                    "a written glyph is not the code the layout gave it",
                ));
            }
            if (pen.x - pen_x).abs() > PLACEMENT_TOLERANCE.max(1e-6 * pen_x.abs())
                || (pen.y - y).abs() > PLACEMENT_TOLERANCE.max(1e-6 * pen.y.abs())
            {
                return Err(unsupported(
                    "a written glyph is not where the layout put it",
                ));
            }
            pen_x += advance_of(style, piece.style, code);
            if position + 1 < piece.codes.len() {
                pen_x += kern_of(
                    style,
                    piece.style,
                    piece.adjust.get(position).copied().unwrap_or(0.0),
                );
            }
        }
        x += piece.advance
            + kern_between(style, piece, pieces.get(index + 1).or(following))
            + gaps.get(index).copied().unwrap_or(0.0);
    }
    Ok((keys, x))
}

pub(super) fn union(one: [f64; 4], other: [f64; 4]) -> [f64; 4] {
    [
        one[0].min(other[0]),
        one[1].min(other[1]),
        one[2].max(other[2]),
        one[3].max(other[3]),
    ]
}
