use pdf_paint::{GraphicsState, Matrix, PaintAtomKind, PaintGraph, Point, TextShowPaint};

use super::{ClusterRef, SpikeError, linear};

#[must_use]
pub fn text_turn(text: &TextShowPaint) -> f64 {
    let to_user = linear(text.state.ctm.value).multiply(linear(text.matrices.text.value));
    if to_user.b.abs() <= UPRIGHT && to_user.a > 0.0 {
        return 0.0;
    }
    let turn = to_user.b.atan2(to_user.a);
    if turn.is_finite() { turn } else { 0.0 }
}

const UPRIGHT: f64 = 1e-9;

pub(super) fn of_rows(graph: &PaintGraph, rows: &[Vec<ClusterRef>]) -> Result<f64, SpikeError> {
    let Some(first) = rows.iter().flatten().next() else {
        return Ok(0.0);
    };
    let text = graph
        .atoms
        .iter()
        .find_map(|atom| match &atom.kind {
            PaintAtomKind::Text(text) if first.anchor.names(&atom.id) => Some(text),
            _ => None,
        })
        .ok_or(SpikeError::AnchorNamesNothing)?;
    Ok(text_turn(text))
}

#[must_use]
pub fn rotation(turn: f64) -> Matrix {
    let (sin, cos) = turn.sin_cos();
    Matrix {
        a: cos,
        b: sin,
        c: -sin,
        d: cos,
        e: 0.0,
        f: 0.0,
    }
}

pub(super) fn turned(graph: &PaintGraph, turn: f64) -> PaintGraph {
    let back = rotation(-turn);
    let mut out = graph.clone();
    turn_atoms(&mut out.atoms, back);
    for scope in &mut out.object_scopes {
        scope.ctm.value = back.multiply(scope.ctm.value);
    }
    out
}

fn turn_atoms(atoms: &mut [pdf_paint::PaintAtom], back: Matrix) {
    let state = |state: &mut GraphicsState| {
        state.ctm.value = back.multiply(state.ctm.value);
        for clip in &mut state.clip_paths {
            clip.ctm.value = back.multiply(clip.ctm.value);
        }
    };
    for atom in atoms {
        match &mut atom.kind {
            PaintAtomKind::Path(paint) => state(&mut paint.state),
            PaintAtomKind::Text(text) => {
                state(&mut text.state);
                for glyph in &mut text.glyphs {
                    if let Some(procedure) = &mut glyph.procedure {
                        turn_atoms(&mut std::sync::Arc::make_mut(procedure).atoms, back);
                    }
                }
            }
            PaintAtomKind::TransparencyGroup(group) => state(&mut group.state),
            PaintAtomKind::Shading(shading) => state(&mut shading.state),
            PaintAtomKind::Image(image) => state(&mut image.state),
        }
    }
}

pub(super) fn into(turn: f64, (dx, dy): (f64, f64)) -> (f64, f64) {
    let at = rotation(-turn).transform(Point { x: dx, y: dy });
    (at.x, at.y)
}

#[must_use]
pub fn out_of(turn: f64, point: Point) -> Point {
    rotation(turn).transform(point)
}

pub(super) fn bounds_out_of(turn: f64, bounds: [f64; 4]) -> [f64; 4] {
    let corners = [
        (bounds[0], bounds[1]),
        (bounds[2], bounds[1]),
        (bounds[2], bounds[3]),
        (bounds[0], bounds[3]),
    ]
    .map(|(x, y)| out_of(turn, Point { x, y }));
    corners.iter().fold(
        [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ],
        |had, point| {
            [
                had[0].min(point.x),
                had[1].min(point.y),
                had[2].max(point.x),
                had[3].max(point.y),
            ]
        },
    )
}
