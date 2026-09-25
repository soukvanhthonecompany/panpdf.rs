use super::{
    BTreeSet, BlockEdit, ClusterKey, Matrix, PaintAtomKind, PaintGraph, Piece, PlacedLine,
    SpikeError, Style, TextShowElement, advance_of, alignments, edited_tokens, kern_between,
    kern_of, laid_extent, line_groups, lines_to_write, place_lines, read_edited_block, rules,
    single_stream, turn, unsupported,
};
use crate::spike_move_text::PlannerPage;

#[derive(Clone, Debug)]
pub struct LiveBlock {
    pub hidden: BTreeSet<usize>,
    pub atoms: Vec<pdf_paint::PaintAtom>,
    pub lines: Vec<LiveLine>,
    pub extent: Option<[f64; 4]>,
    pub before: Vec<Vec<ClusterKey>>,
}

#[derive(Clone, Debug)]
pub struct LiveLine {
    pub origin: (f64, f64),
    pub em: f64,
    pub pieces: Vec<LivePiece>,
}

#[derive(Clone, Debug)]
pub struct LivePiece {
    pub text: String,
    pub kept: Option<ClusterKey>,
    pub from: f64,
    pub to: f64,
}

pub(crate) fn lay_out_live(
    source: &pdf_bytes::ByteStore,
    (page, page_index): (PlannerPage<'_>, usize),
    edit: &BlockEdit<'_>,
) -> Result<LiveBlock, SpikeError> {
    if turn::of_rows(page.graph, edit.rows)? != 0.0 {
        return Err(unsupported("a live block is upright"));
    }
    if edit.style.is_some() || edit.typed.is_some() || edit.shift().is_some() {
        return Err(unsupported(
            "a live block is typed into, in the style it has",
        ));
    }
    let graph = page.graph;
    let (mut reading, frame) = read_edited_block(page, edit)?;
    single_stream(page.program, reading.stream)?;
    if !reading.outside.is_empty() {
        return Err(unsupported("a live block paints no other block's glyphs"));
    }
    if !reading.underlines.is_empty() {
        return Err(unsupported("a live block draws no underline"));
    }
    let groups = line_groups(&reading.lines);
    let aligned = alignments(&reading, &groups, frame, edit.paragraph.flow_round);
    let mut faces = super::faces::NewFaces::new(&page);
    let (mut tokens, _, selected) = edited_tokens(&reading, &groups, edit, &mut faces)?;
    let base = reading.style.runs.len();
    let programs = faces.programs();
    super::styled_and_settled(
        source,
        (page, page_index),
        edit,
        &mut reading.style,
        (&mut tokens, selected),
        &mut faces,
    )?;
    let blocked = super::stands_in_the_frame(page, &reading, frame, edit.paragraph);
    let laid = place_lines(
        &tokens,
        &aligned,
        &reading,
        frame,
        super::InTheFrame {
            set: edit.paragraph.alignment,
            blocked: &blocked,
        },
    )?;
    let placed = lines_to_write(edit, &reading, graph, &laid)?;
    if !rules(&reading.style, &placed).is_empty() {
        return Err(unsupported("a live block draws no underline"));
    }
    let owners = run_owners(&reading.style, graph);
    let mut templates = run_templates(&reading.style, &owners, graph);
    for (index, program) in programs.into_iter().enumerate() {
        if let Some(Some(pdf_paint::PaintAtom {
            kind: PaintAtomKind::Text(paint),
            ..
        })) = templates.get_mut(base + index)
        {
            paint.units_per_em = program.units_per_em();
            paint.program = Some(program);
            paint.substitution = None;
            paint.font_request = None;
        }
    }
    let mut atoms = Vec::new();
    let mut lines = Vec::with_capacity(placed.len());
    for line in &placed {
        lines.push(live_line(
            &reading.style,
            line,
            (&templates, page.fonts),
            &mut atoms,
        )?);
    }
    let before = reading
        .lines
        .iter()
        .map(|line| {
            line.row.map_or_else(Vec::new, |row| {
                reading.rows[row]
                    .iter()
                    .map(|cluster| cluster.key)
                    .collect()
            })
        })
        .collect();
    Ok(LiveBlock {
        hidden: reading.named.clone(),
        atoms,
        lines,
        extent: laid_extent(&reading.style, &placed),
        before,
    })
}

fn run_owners(style: &Style<'_>, graph: &PaintGraph) -> Vec<Option<usize>> {
    let mut owners = vec![None; style.runs.len()];
    for (ordinal, run) in &style.run_of {
        if let Some(slot) = owners.get_mut(*run)
            && slot.is_none()
            && graph.atoms.get(*ordinal).is_some()
        {
            *slot = Some(*ordinal);
        }
    }
    let first = style.run_of.keys().next().copied();
    for slot in &mut owners {
        if slot.is_none() {
            *slot = first;
        }
    }
    owners
}

fn run_templates(
    style: &Style<'_>,
    owners: &[Option<usize>],
    graph: &PaintGraph,
) -> Vec<Option<pdf_paint::PaintAtom>> {
    style
        .runs
        .iter()
        .zip(owners)
        .map(|(run, owner)| {
            let atom = &graph.atoms[(*owner)?];
            let reference = run.reference;
            let mut paint = reference.clone_without_glyphs();
            paint.state.text = run.text.clone();
            if let Some([red, green, blue]) = run.fill_rgb {
                paint.state.fill_color_space.value = pdf_paint::ColorSpace::DeviceRgb;
                paint.state.fill_color.value = pdf_paint::Color::DeviceRgb(red, green, blue);
            }
            if run.stroke_like_fill {
                paint.state.stroke_color_space.value = paint.state.fill_color_space.value.clone();
                paint.state.stroke_color.value = paint.state.fill_color.value.clone();
                paint.state.line_width.value = run.line_width;
            }
            paint.state.clip_paths.clear();
            Some(pdf_paint::PaintAtom {
                id: atom.id.clone(),
                kind: PaintAtomKind::Text(paint),
                marks: atom.marks.clone(),
            })
        })
        .collect()
}

fn live_line(
    style: &Style<'_>,
    line: &PlacedLine,
    (templates, fonts): (&[Option<pdf_paint::PaintAtom>], crate::Fonts<'_>),
    atoms: &mut Vec<pdf_paint::PaintAtom>,
) -> Result<LiveLine, SpikeError> {
    let inverse = style
        .ctm
        .inverse()
        .ok_or_else(|| unsupported("the block's transform cannot be inverted"))?;
    let mut pieces = Vec::with_capacity(line.pieces.len());
    let mut x = line.origin.x;
    let mut open: Option<(usize, pdf_paint::PaintAtom)> = None;
    for (index, piece) in line.pieces.iter().enumerate() {
        if !piece.codes.is_empty() {
            if open.as_ref().is_some_and(|(run, _)| *run != piece.style) {
                atoms.extend(open.take().map(|(_, atom)| substituted(atom, style, fonts)));
            }
            if open.is_none() {
                let template = templates
                    .get(piece.style)
                    .cloned()
                    .flatten()
                    .ok_or_else(|| unsupported("a live block's run paints on the page"))?;
                open = Some((piece.style, template));
            }
            if let Some((
                _,
                pdf_paint::PaintAtom {
                    kind: PaintAtomKind::Text(paint),
                    ..
                },
            )) = open.as_mut()
            {
                live_glyphs(style, piece, (x, line.origin.y), inverse, paint)?;
            }
        }
        let to = x + piece.advance;
        pieces.push(LivePiece {
            text: piece.text.clone(),
            kept: piece.kept,
            from: x,
            to,
        });
        x = to + kern_between(style, piece, line.pieces.get(index + 1));
    }
    atoms.extend(open.map(|(_, atom)| substituted(atom, style, fonts)));
    let em = line
        .pieces
        .iter()
        .map(|piece| style.run_em(piece.style).abs())
        .fold(0.0, f64::max);
    Ok(LiveLine {
        origin: (line.origin.x, line.origin.y),
        em: if em > 0.0 { em } else { style.em().abs() },
        pieces,
    })
}

fn live_glyphs(
    style: &Style<'_>,
    piece: &Piece,
    (start, y): (f64, f64),
    inverse: Matrix,
    paint: &mut pdf_paint::TextShowPaint,
) -> Result<(), SpikeError> {
    let run = style.run(piece.style);
    let reference = run.reference;
    if reference.type3 || (paint.program.is_none() && paint.substitution.is_none()) {
        return Err(unsupported(
            "a live block's fonts carry their own outlines or are drawn in a face",
        ));
    }
    let program = paint.program.clone();
    let source_span = match reference.elements.first() {
        Some(
            TextShowElement::Codes { source_span, .. }
            | TextShowElement::Adjustment { source_span, .. },
        ) => *source_span,
        None => return Err(unsupported("a live block's run shows something")),
    };
    let tm = if run.shear == 0.0 {
        style.tm_linear
    } else {
        super::sheared(style.tm_linear, run.shear)
    };
    let mut pen_x = start;
    for (position, code) in piece.codes.iter().enumerate() {
        let at = inverse.transform(pdf_paint::Point { x: pen_x, y });
        let matrix = Matrix {
            e: at.x,
            f: at.y,
            ..tm
        };
        if paint.glyphs.is_empty() {
            paint.matrices.text.value = matrix;
            paint.matrices.line.value = matrix;
        }
        let mut text = run.text.clone();
        text.rise.value = piece.rise.get(position).copied().unwrap_or(0.0);
        let element = TextShowElement::Codes {
            source_span,
            decoded_bytes: code.bytes.clone(),
            codes: vec![code.clone()],
        };
        let (mut placed, _) = pdf_paint::position_text(
            &text,
            std::slice::from_ref(&element),
            matrix,
            program.as_deref(),
            &run.font,
            0.001,
        );
        if let Some(shaped) = piece.shaped.get(position) {
            for glyph in &mut placed {
                glyph.glyph = Some(shaped.glyph);
            }
        }
        paint.glyphs.append(&mut placed);
        paint.elements.push(element);
        pen_x += advance_of(style, piece.style, code);
        if position + 1 < piece.codes.len() {
            pen_x += kern_of(
                style,
                piece.style,
                piece.adjust.get(position).copied().unwrap_or(0.0),
            );
        }
    }
    Ok(())
}

fn substituted(
    mut atom: pdf_paint::PaintAtom,
    style: &Style<'_>,
    fonts: crate::Fonts<'_>,
) -> pdf_paint::PaintAtom {
    let PaintAtomKind::Text(paint) = &mut atom.kind else {
        return atom;
    };
    let (None, Some(substitution), Some(provider)) =
        (paint.program.as_ref(), paint.substitution.clone(), fonts)
    else {
        return atom;
    };
    let Some(font) = style
        .runs
        .iter()
        .find(|run| {
            run.reference
                .substitution
                .as_ref()
                .is_some_and(|other| std::sync::Arc::ptr_eq(other, &substitution))
        })
        .map(|run| run.font.clone())
    else {
        return atom;
    };
    let text = std::sync::Arc::clone(&paint.text);
    let found = pdf_paint::substitute_run(
        &mut paint.glyphs,
        &substitution.request,
        substitution.primary.clone(),
        provider.as_ref(),
        &font,
        &text,
    );
    paint.substitution = Some(std::sync::Arc::new(found));
    atom
}
