use pdf_bytes::ByteStore;
use pdf_paint::{Matrix, PaintAtomKind, PaintGraph, PathSegment, Point};
use pdf_syntax::Reference;

use crate::plan::{
    Capability, Effect, MovedRun, PenBlend, PenStep, PenStroke, Plan, PlannedBody, PlannedWrite,
    SourceAnchor,
};
use crate::spike_move_text::{PlannerPage, SpikeError, interpret_bytes_of};

pub const MOST_STEPS: usize = 10_000;

fn refused(reason: &'static str) -> SpikeError {
    SpikeError::RetypeUnsupported(reason)
}

#[derive(Clone, Copy, Debug)]
pub struct NewPath<'a> {
    pub steps: &'a [PenStep],
    pub closed: bool,
    pub stroke: Option<PenStroke>,
    pub fill: Option<[f64; 3]>,
}

pub(crate) fn plan_new_path(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    new: &NewPath<'_>,
) -> Result<Plan, SpikeError> {
    checked(new)?;
    let mut writes = Vec::new();
    let mut state = None;
    if let Some(entries) = state_entries(new) {
        let object = Reference::new(crate::block_rewrite::next_object_number(source)?, 0);
        writes.push(PlannedWrite {
            reference: object,
            body: PlannedBody::Direct {
                body: entries.into_bytes(),
            },
        });
        let (name, holder) = crate::new_font::add_resource(
            source,
            page.program.page,
            (b"/ExtGState", "GS"),
            object,
        )?;
        writes.push(holder);
        state = Some(name);
    }
    let carried;
    let page = if writes.is_empty() {
        page
    } else {
        let document = crate::block_rewrite::commit_writes(source, &writes, page.restrictions)?;
        carried = crate::spike_move_text::read_page(&document, page_index, b"", page.fonts)?;
        PlannerPage {
            program: &carried.program,
            operations: &carried.operations,
            graph: &carried.graph,
            fonts: page.fonts,
            restrictions: page.restrictions,
            credential: page.credential,
        }
    };
    let stream = page
        .program
        .streams
        .len()
        .checked_sub(1)
        .ok_or_else(|| refused("a page with no content stream cannot be drawn on"))?;
    let decoded = page.program.streams[stream].bytes.as_bytes();

    let measuring = candidate(decoded, new, state.as_deref(), None);
    let measured = interpret_bytes_of(page.program, stream, &measuring, page.fonts)?;
    let standing = drawn(page.graph, &measured)?.state.ctm.value;
    let into = standing
        .inverse()
        .ok_or_else(|| refused("this page leaves a transform a line cannot be drawn through"))?;
    let bytes = candidate(decoded, new, state.as_deref(), Some(into));
    let graph = interpret_bytes_of(page.program, stream, &bytes, page.fonts)?;
    prove_drawn(page.graph, &graph, new)?;

    let ordinal = page.graph.atoms.len();
    let atom = &graph.atoms[ordinal];
    writes.push(PlannedWrite {
        reference: page.program.streams[stream].reference,
        body: PlannedBody::ReplacedStream { decoded: bytes },
    });
    Ok(Plan::new(
        Capability::Exact,
        writes,
        Effect {
            page_index,
            moved: vec![MovedRun {
                anchor: SourceAnchor::of(&atom.id),
                atom_ordinal: ordinal,
                original_matrix: Matrix::IDENTITY,
            }],
            target_stream: page.program.streams[stream].reference,
            declared_region: Some(region(new)),
        },
    ))
}

fn state_entries(new: &NewPath<'_>) -> Option<String> {
    gstate_entries(new.stroke)
}

pub(crate) fn gstate_entries(stroke: Option<PenStroke>) -> Option<String> {
    let pen = stroke?;
    if pen.opacity >= 1.0 && pen.blend == PenBlend::Normal {
        return None;
    }
    let blend = String::from_utf8_lossy(pen.blend.name()).into_owned();
    Some(format!(
        "<< /Type /ExtGState /CA {opacity} /ca {opacity} /BM {blend} >>",
        opacity = pen.opacity,
    ))
}

pub(crate) fn checked(new: &NewPath<'_>) -> Result<(), SpikeError> {
    let Some(PenStep::Move(_)) = new.steps.first() else {
        return Err(refused("a drawing starts where the pen went down"));
    };
    if new.steps.len() < 2 {
        return Err(refused("a drawing is more than one point"));
    }
    if new.steps.len() > MOST_STEPS {
        return Err(refused("this drawing has more points than one line holds"));
    }
    if !new
        .steps
        .iter()
        .flat_map(PenStep::points)
        .all(|(x, y)| x.is_finite() && y.is_finite())
    {
        return Err(refused("a drawing is made of numbers"));
    }
    match new.stroke {
        Some(pen) if !(pen.width.is_finite() && pen.width > 0.0) => {
            return Err(refused("a pen has a width"));
        }
        _ => {}
    }
    match new.stroke {
        Some(pen) if !(0.0..=1.0).contains(&pen.opacity) => {
            return Err(refused("how see-through a stroke is runs from zero to one"));
        }
        _ => {}
    }
    if new.stroke.is_none() && new.fill.is_none() {
        return Err(refused(
            "a drawing that is neither drawn nor filled would not show",
        ));
    }
    let colours = new.stroke.map(|pen| pen.colour).into_iter().chain(new.fill);
    if !colours.flatten().all(|part| (0.0..=1.0).contains(&part)) {
        return Err(refused("a colour is three numbers from zero to one"));
    }
    Ok(())
}

pub(crate) fn candidate(
    decoded: &[u8],
    new: &NewPath<'_>,
    state: Option<&str>,
    into: Option<Matrix>,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(decoded.len() + new.steps.len() * 24);
    out.extend_from_slice(decoded);
    out.extend_from_slice(b"\nq ");
    if let Some(name) = state {
        out.extend_from_slice(format!("/{name} gs ").as_bytes());
    }
    if let Some(m) = into {
        let m = [m.a, m.b, m.c, m.d, m.e, m.f].map(|value| value + 0.0);
        out.extend_from_slice(
            format!("{} {} {} {} {} {} cm ", m[0], m[1], m[2], m[3], m[4], m[5]).as_bytes(),
        );
    }
    if let Some(pen) = new.stroke {
        let [red, green, blue] = pen.colour;
        let cap = u8::from(pen.round_ends);
        out.extend_from_slice(
            format!("{red} {green} {blue} RG {} w {cap} J 1 j ", pen.width).as_bytes(),
        );
    }
    if let Some([red, green, blue]) = new.fill {
        out.extend_from_slice(format!("{red} {green} {blue} rg ").as_bytes());
    }
    for step in new.steps {
        match step {
            PenStep::Move((x, y)) => out.extend_from_slice(format!("{x} {y} m ").as_bytes()),
            PenStep::Line((x, y)) => out.extend_from_slice(format!("{x} {y} l ").as_bytes()),
            PenStep::Curve((x1, y1), (x2, y2), (x, y)) => {
                out.extend_from_slice(format!("{x1} {y1} {x2} {y2} {x} {y} c ").as_bytes());
            }
        }
    }
    if new.closed {
        out.extend_from_slice(b"h ");
    }
    out.extend_from_slice(match (new.stroke.is_some(), new.fill.is_some()) {
        (true, true) => b"B Q\n",
        (true, false) => b"S Q\n",
        (false, _) => b"f Q\n",
    });
    out
}

fn drawn<'a>(
    before: &PaintGraph,
    after: &'a PaintGraph,
) -> Result<&'a pdf_paint::PathPaint, SpikeError> {
    match after.atoms.get(before.atoms.len()).map(|atom| &atom.kind) {
        Some(PaintAtomKind::Path(path)) => Ok(path),
        _ => Err(refused("the line drawn does not paint")),
    }
}

fn prove_drawn(
    before: &PaintGraph,
    after: &PaintGraph,
    new: &NewPath<'_>,
) -> Result<(), SpikeError> {
    if after.atoms.len() != before.atoms.len() + 1 {
        return Err(SpikeError::MoveNotIsolated);
    }
    crate::new_text::prove_untouched(before, after)?;
    drawn_as(drawn(before, after)?, new)
}

pub(crate) fn drawn_as(paint: &pdf_paint::PathPaint, new: &NewPath<'_>) -> Result<(), SpikeError> {
    let wrong = || refused("the line drawn does not paint as it was drawn");
    if paint.stroke != new.stroke.is_some() || paint.fill.is_some() != new.fill.is_some() {
        return Err(wrong());
    }
    let ctm = paint.state.ctm.value;
    let wanted: Vec<(f64, f64)> = new.steps.iter().flat_map(PenStep::points).collect();
    let painted: Vec<Point> = paint
        .path
        .segments
        .iter()
        .flat_map(|segment| match segment {
            PathSegment::MoveTo { point, .. } | PathSegment::LineTo { point, .. } => {
                vec![*point]
            }
            PathSegment::CubicTo {
                control_1,
                control_2,
                end,
                ..
            } => vec![*control_1, *control_2, *end],
            PathSegment::ClosePath { .. } => Vec::new(),
            PathSegment::Rectangle { origin, .. } => vec![*origin],
        })
        .collect();
    if painted.len() != wanted.len() {
        return Err(wrong());
    }
    for (point, (x, y)) in painted.iter().zip(wanted) {
        let at = ctm.transform(*point);
        if (at.x - x).abs() > crate::block_move::PLACEMENT_TOLERANCE
            || (at.y - y).abs() > crate::block_move::PLACEMENT_TOLERANCE
        {
            return Err(wrong());
        }
    }
    if let Some(pen) = new.stroke {
        let width = paint.state.line_width.value;
        if (width - pen.width).abs() > crate::block_move::PLACEMENT_TOLERANCE
            || !paints(&paint.state.stroke_color.value, pen.colour)
        {
            return Err(wrong());
        }
        if (paint.state.stroke_alpha.value - pen.opacity).abs() > 1e-9
            || paint.state.blend_mode.value.names != vec![pen.blend.name().to_vec()]
        {
            return Err(wrong());
        }
    }
    match new.fill {
        Some(colour) if !paints(&paint.state.fill_color.value, colour) => Err(wrong()),
        _ => Ok(()),
    }
}

pub(crate) fn paints(painted: &pdf_paint::Color, [red, green, blue]: [f64; 3]) -> bool {
    match painted {
        pdf_paint::Color::DeviceRgb(r, g, b) => [(*r, red), (*g, green), (*b, blue)]
            .iter()
            .all(|(painted, wanted)| (painted - wanted).abs() <= 1e-9),
        _ => false,
    }
}

fn region(new: &NewPath<'_>) -> [f64; 4] {
    let reach = new.stroke.map_or(0.0, |pen| pen.width / 2.0);
    new.steps.iter().flat_map(PenStep::points).fold(
        [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ],
        |[x0, y0, x1, y1], (x, y)| {
            [
                x0.min(x - reach),
                y0.min(y - reach),
                x1.max(x + reach),
                y1.max(y + reach),
            ]
        },
    )
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    reason = "a drawing of whole points arrives as the whole points it was"
)]
mod tests {
    use pdf_bytes::{ByteStore, SourceId};
    use pdf_paint::{PaintAtomKind, PathSegment};

    use crate::plan::{Command, PenBlend, PenStep, PenStroke};
    use crate::spike_move_text::{plan_command_with_fonts, read_page};

    fn document(leaves: &str) -> ByteStore {
        let content = format!("0 0 1 rg 0 0 10 10 re f {leaves}");
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Resources << >> /Contents 4 0 R >>"
                .to_owned(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        ];
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
        }
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        ByteStore::new(SourceId::new(1), bytes)
    }

    fn drew(source: &ByteStore, command: &Command) -> Option<pdf_paint::PathPaint> {
        let plan = plan_command_with_fonts(source, command, b"", None).ok()?;
        let after = plan.commit(source, b"").expect("the plan commits");
        let page = read_page(&after, 0, b"", None).expect("reads");
        match &page.graph.atoms.last().expect("an atom").kind {
            PaintAtomKind::Path(path) => Some(path.clone()),
            other => panic!("the last atom is the drawing, not {other:?}"),
        }
    }

    fn pen_stroke(steps: Vec<PenStep>, closed: bool) -> Command {
        Command::DrawPath {
            page_index: 0,
            steps,
            closed,
            stroke: Some(PenStroke::pen([1.0, 0.0, 0.0], 2.0)),
            fill: None,
        }
    }

    fn points(path: &pdf_paint::PathPaint) -> Vec<(f64, f64)> {
        let ctm = path.state.ctm.value;
        path.path
            .segments
            .iter()
            .filter_map(|segment| match segment {
                PathSegment::MoveTo { point, .. } | PathSegment::LineTo { point, .. } => {
                    let at = ctm.transform(*point);
                    Some((at.x, at.y))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_line_is_drawn_through_the_points_it_was_drawn_through() {
        let source = document("");
        let path = drew(
            &source,
            &pen_stroke(
                vec![
                    PenStep::Move((20.0, 30.0)),
                    PenStep::Line((60.0, 90.0)),
                    PenStep::Line((100.0, 30.0)),
                ],
                false,
            ),
        )
        .expect("the line is drawn");
        assert_eq!(points(&path), [(20.0, 30.0), (60.0, 90.0), (100.0, 30.0)]);
        assert!(path.stroke && path.fill.is_none());
        assert_eq!(path.state.line_width.value, 2.0);
        assert_eq!(
            path.state.stroke_color.value,
            pdf_paint::Color::DeviceRgb(1.0, 0.0, 0.0)
        );
    }

    #[test]
    fn a_page_that_leaves_a_transform_draws_the_same_line_at_the_same_width() {
        let source = document("2 0 0 2 30 40 cm");
        let path = drew(
            &source,
            &pen_stroke(
                vec![PenStep::Move((20.0, 30.0)), PenStep::Line((60.0, 90.0))],
                false,
            ),
        )
        .expect("the line is drawn");
        for (got, wanted) in points(&path).iter().zip([(20.0, 30.0), (60.0, 90.0)]) {
            assert!((got.0 - wanted.0).abs() < 1e-9 && (got.1 - wanted.1).abs() < 1e-9);
        }
        let scale = path.state.ctm.value;
        assert!(
            (path.state.line_width.value * scale.a - 2.0).abs() < 1e-9,
            "two points wide on the page: {} at {}",
            path.state.line_width.value,
            scale.a
        );
    }

    #[test]
    fn a_closed_shape_is_filled_and_drawn_and_its_curve_is_kept() {
        let source = document("");
        let path = drew(
            &source,
            &Command::DrawPath {
                page_index: 0,
                steps: vec![
                    PenStep::Move((10.0, 10.0)),
                    PenStep::Curve((10.0, 40.0), (70.0, 40.0), (70.0, 10.0)),
                ],
                closed: true,
                stroke: Some(PenStroke::pen([0.0, 0.0, 1.0], 1.5)),
                fill: Some([0.0, 0.5, 0.0]),
            },
        )
        .expect("the shape is drawn");
        assert!(path.stroke && path.fill.is_some(), "drawn and filled");
        assert!(
            matches!(path.path.segments.get(1), Some(PathSegment::CubicTo { .. })),
            "the curve is written as a curve: {:?}",
            path.path.segments
        );
        assert!(matches!(
            path.path.segments.get(2),
            Some(PathSegment::ClosePath { .. })
        ));
        assert_eq!(
            path.state.fill_color.value,
            pdf_paint::Color::DeviceRgb(0.0, 0.5, 0.0)
        );
    }

    #[test]
    fn a_highlighter_is_see_through_and_multiplied_into_the_page() {
        let source = document("");
        let path = drew(
            &source,
            &Command::DrawPath {
                page_index: 0,
                steps: vec![PenStep::Move((10.0, 20.0)), PenStep::Line((90.0, 20.0))],
                closed: false,
                stroke: Some(PenStroke {
                    colour: [1.0, 0.95, 0.2],
                    width: 14.0,
                    opacity: 0.4,
                    blend: PenBlend::Multiply,
                    round_ends: false,
                }),
                fill: None,
            },
        )
        .expect("the highlight is drawn");
        assert_eq!(path.state.stroke_alpha.value, 0.4);
        assert_eq!(
            path.state.blend_mode.value.names,
            vec![b"/Multiply".to_vec()]
        );
        assert_eq!(path.state.line_cap.value, pdf_paint::LineCap::Butt);
        assert_eq!(path.state.line_width.value, 14.0);
    }

    #[test]
    fn an_opaque_pen_brings_no_graphics_state_with_it() {
        let source = document("");
        let plan = plan_command_with_fonts(
            &source,
            &pen_stroke(
                vec![PenStep::Move((0.0, 0.0)), PenStep::Line((10.0, 10.0))],
                false,
            ),
            b"",
            None,
        )
        .expect("plans");
        assert_eq!(
            plan.writes().len(),
            1,
            "the content stream, and nothing else"
        );
        let marked = plan_command_with_fonts(
            &source,
            &Command::DrawPath {
                page_index: 0,
                steps: vec![PenStep::Move((0.0, 0.0)), PenStep::Line((10.0, 10.0))],
                closed: false,
                stroke: Some(PenStroke {
                    colour: [1.0, 1.0, 0.0],
                    width: 10.0,
                    opacity: 0.5,
                    blend: PenBlend::Multiply,
                    round_ends: false,
                }),
                fill: None,
            },
            b"",
            None,
        )
        .expect("plans");
        assert_eq!(
            marked.writes().len(),
            3,
            "the state, the page that names it, and the content stream"
        );
    }

    #[test]
    fn what_cannot_be_drawn_is_refused() {
        let source = document("");
        let one_point = pen_stroke(vec![PenStep::Move((10.0, 10.0))], false);
        let no_move = pen_stroke(
            vec![PenStep::Line((10.0, 10.0)), PenStep::Line((20.0, 20.0))],
            false,
        );
        let no_width = Command::DrawPath {
            page_index: 0,
            steps: vec![PenStep::Move((0.0, 0.0)), PenStep::Line((10.0, 10.0))],
            closed: false,
            stroke: Some(PenStroke::pen([0.0, 0.0, 0.0], 0.0)),
            fill: None,
        };
        let neither = Command::DrawPath {
            page_index: 0,
            steps: vec![PenStep::Move((0.0, 0.0)), PenStep::Line((10.0, 10.0))],
            closed: false,
            stroke: None,
            fill: None,
        };
        for command in [one_point, no_move, no_width, neither] {
            assert!(drew(&source, &command).is_none(), "{command:?}");
        }
    }
}
