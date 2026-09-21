use pdf_paint::{FillRule, GraphicsState, Matrix, PaintAtomKind, PaintGraph, Path, PathSegment};

use crate::{Cluster, Object, ObjectKind, Point, Quad, SemanticIndex};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HitEvidence {
    Ink,
    Envelope,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HitDoubt {
    CurvedClip,
    MaskedImage,
    StrokeWidth,
    GroupContents,
    UnboundedShading,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub object: usize,
    pub kind: ObjectKind,
    pub evidence: HitEvidence,
    pub doubts: Vec<HitDoubt>,
}

impl Candidate {
    #[must_use]
    pub fn certain(&self) -> bool {
        self.evidence == HitEvidence::Ink && self.doubts.is_empty()
    }
}

#[must_use]
pub fn candidates(graph: &PaintGraph, index: &SemanticIndex, point: Point) -> Vec<Candidate> {
    let mut found = Vec::new();
    for (position, object) in index.objects.iter().enumerate().rev() {
        if let Some(candidate) = test(graph, index, object, position, point) {
            found.push(candidate);
        }
    }
    found
}

#[must_use]
pub fn foremost(graph: &PaintGraph, index: &SemanticIndex, point: Point) -> Option<Candidate> {
    candidates(graph, index, point).into_iter().next()
}

fn test(
    graph: &PaintGraph,
    index: &SemanticIndex,
    object: &Object,
    position: usize,
    point: Point,
) -> Option<Candidate> {
    if object.bounds.is_some_and(|box_| !within(box_, point)) {
        return None;
    }
    let mut doubts = Vec::new();
    let mut evidence = None;
    for ordinal in object.atoms() {
        let Some(drawn) = graph.atoms.get(ordinal) else {
            continue;
        };
        if clipped_out(state_of(&drawn.kind), point, &mut doubts) == Some(true) {
            continue;
        }
        if let Some(found) = test_atom(index, object, ordinal, &drawn.kind, point, &mut doubts) {
            evidence = Some(match (evidence, found) {
                (Some(HitEvidence::Ink), _) | (_, HitEvidence::Ink) => HitEvidence::Ink,
                _ => HitEvidence::Envelope,
            });
        }
    }
    doubts.sort_unstable_by_key(|doubt| *doubt as u8);
    doubts.dedup();
    Some(Candidate {
        object: position,
        kind: object.kind,
        evidence: evidence?,
        doubts,
    })
}

fn test_atom(
    index: &SemanticIndex,
    object: &Object,
    atom: usize,
    kind: &PaintAtomKind,
    point: Point,
    doubts: &mut Vec<HitDoubt>,
) -> Option<HitEvidence> {
    match kind {
        PaintAtomKind::Text(_) => {
            let hit = clusters_of(index, object, atom).any(|cluster| {
                cluster
                    .bounds
                    .is_some_and(|bounds| within(padded(bounds), point))
            });
            if hit {
                return Some(HitEvidence::Ink);
            }
            let measurable =
                clusters_of(index, object, atom).any(|cluster| cluster.bounds.is_some());
            (!measurable && object.quad.is_some_and(|quad| quad.contains(point)))
                .then_some(HitEvidence::Envelope)
        }
        PaintAtomKind::Image(image) => {
            let quad = Quad::placed([0.0, 0.0, 1.0, 1.0], image.state.ctm.value);
            if !quad.contains(point) {
                return None;
            }
            if image.soft_mask.is_some() || image.image_mask.value {
                doubts.push(HitDoubt::MaskedImage);
            }
            Some(HitEvidence::Ink)
        }
        PaintAtomKind::Path(path) => {
            if let Some(rule) = path.fill
                && inside_path(&path.path, path.state.ctm.value, rule, point)
            {
                return Some(HitEvidence::Ink);
            }
            if path.stroke {
                doubts.push(HitDoubt::StrokeWidth);
                return Some(HitEvidence::Envelope);
            }
            None
        }
        PaintAtomKind::TransparencyGroup(group) => {
            let quad = Quad::placed(
                group.bbox,
                group.state.ctm.value.multiply(group.matrix.value),
            );
            if !quad.contains(point) {
                return None;
            }
            doubts.push(HitDoubt::GroupContents);
            Some(HitEvidence::Envelope)
        }
        PaintAtomKind::Shading(shading) => {
            if let Some(bbox) = shading.bbox.as_ref() {
                return Quad::placed(bbox.value, shading.state.ctm.value)
                    .contains(point)
                    .then_some(HitEvidence::Ink);
            }
            doubts.push(HitDoubt::UnboundedShading);
            Some(HitEvidence::Envelope)
        }
    }
}

fn clusters_of<'a>(
    index: &'a SemanticIndex,
    object: &'a Object,
    atom: usize,
) -> impl Iterator<Item = &'a Cluster> {
    index.clusters.iter().filter(move |cluster| {
        cluster.atom == atom
            && object.members.iter().any(|member| {
                member.atom == atom
                    && member.glyphs.as_ref().is_none_or(|glyphs| {
                        cluster.glyphs.start >= glyphs.start && cluster.glyphs.start < glyphs.end
                    })
            })
    })
}

fn clipped_out(state: &GraphicsState, point: Point, doubts: &mut Vec<HitDoubt>) -> Option<bool> {
    if state.clip_paths.is_empty() {
        return None;
    }
    for clip in &state.clip_paths {
        if has_curve(&clip.path) {
            doubts.push(HitDoubt::CurvedClip);
        }
        if !inside_path(&clip.path, clip.ctm.value, clip.rule, point) {
            return Some(true);
        }
    }
    Some(false)
}

const fn state_of(kind: &PaintAtomKind) -> &GraphicsState {
    match kind {
        PaintAtomKind::Path(path) => &path.state,
        PaintAtomKind::Text(text) => &text.state,
        PaintAtomKind::Image(image) => &image.state,
        PaintAtomKind::Shading(shading) => &shading.state,
        PaintAtomKind::TransparencyGroup(group) => &group.state,
    }
}

const TOUCH: f64 = 0.5;

fn padded(bounds: [f64; 4]) -> [f64; 4] {
    [
        bounds[0] - TOUCH,
        bounds[1] - TOUCH,
        bounds[2] + TOUCH,
        bounds[3] + TOUCH,
    ]
}

fn within(bounds: [f64; 4], point: Point) -> bool {
    point.x >= bounds[0] && point.x <= bounds[2] && point.y >= bounds[1] && point.y <= bounds[3]
}

fn has_curve(path: &Path) -> bool {
    path.segments
        .iter()
        .any(|segment| matches!(segment, PathSegment::CubicTo { .. }))
}

const FLATTEN: usize = 16;

#[must_use]
pub fn inside_path(path: &Path, matrix: Matrix, rule: FillRule, point: Point) -> bool {
    let mut ray = Ray {
        point,
        winding: 0,
        crossings: 0,
    };
    let mut start: Option<Point> = None;
    let mut current: Option<Point> = None;
    for segment in &path.segments {
        match segment {
            PathSegment::MoveTo { point: at, .. } => {
                ray.close(current, start);
                let at = matrix.transform(*at);
                start = Some(at);
                current = Some(at);
            }
            PathSegment::LineTo { point: at, .. } => {
                let at = matrix.transform(*at);
                if let Some(from) = current {
                    ray.cross(from, at);
                }
                current = Some(at);
            }
            PathSegment::CubicTo {
                control_1,
                control_2,
                end,
                ..
            } => {
                let (Some(from), one, two, to) = (
                    current,
                    matrix.transform(*control_1),
                    matrix.transform(*control_2),
                    matrix.transform(*end),
                ) else {
                    continue;
                };
                let mut previous = from;
                for step in 1..=FLATTEN {
                    #[allow(clippy::cast_precision_loss)]
                    let t = step as f64 / FLATTEN as f64;
                    let next = cubic(from, one, two, to, t);
                    ray.cross(previous, next);
                    previous = next;
                }
                current = Some(to);
            }
            PathSegment::ClosePath { .. } => {
                if let (Some(from), Some(to)) = (current, start) {
                    ray.cross(from, to);
                }
                current = start;
            }
            PathSegment::Rectangle {
                origin,
                width,
                height,
                ..
            } => {
                ray.close(current, start);
                let corners = [
                    matrix.transform(*origin),
                    matrix.transform(Point {
                        x: origin.x + width,
                        y: origin.y,
                    }),
                    matrix.transform(Point {
                        x: origin.x + width,
                        y: origin.y + height,
                    }),
                    matrix.transform(Point {
                        x: origin.x,
                        y: origin.y + height,
                    }),
                ];
                for index in 0..4 {
                    ray.cross(corners[index], corners[(index + 1) % 4]);
                }
                start = Some(corners[0]);
                current = Some(corners[0]);
            }
        }
    }
    ray.close(current, start);
    match rule {
        FillRule::Nonzero => ray.winding != 0,
        FillRule::EvenOdd => ray.crossings % 2 == 1,
    }
}

struct Ray {
    point: Point,
    winding: i32,
    crossings: u32,
}

impl Ray {
    fn cross(&mut self, from: Point, to: Point) {
        let (low, high) = (from.y.min(to.y), from.y.max(to.y));
        if self.point.y < low || self.point.y >= high {
            return;
        }
        let t = (self.point.y - from.y) / (to.y - from.y);
        if t.mul_add(to.x - from.x, from.x) <= self.point.x {
            return;
        }
        self.crossings += 1;
        self.winding += if to.y > from.y { 1 } else { -1 };
    }

    fn close(&mut self, current: Option<Point>, start: Option<Point>) {
        if let (Some(from), Some(to)) = (current, start)
            && (from.x - to.x).abs() + (from.y - to.y).abs() > 0.0
        {
            self.cross(from, to);
        }
    }
}

fn cubic(from: Point, one: Point, two: Point, to: Point, at: f64) -> Point {
    let rest = 1.0 - at;
    let start = rest * rest * rest;
    let first = 3.0 * rest * rest * at;
    let second = 3.0 * rest * at * at;
    let end = at * at * at;
    Point {
        x: start.mul_add(
            from.x,
            first.mul_add(one.x, second.mul_add(two.x, end * to.x)),
        ),
        y: start.mul_add(
            from.y,
            first.mul_add(one.y, second.mul_add(two.y, end * to.y)),
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdf_bytes::{SourceId, SourceSpan};
    use pdf_paint::{
        ClipPath, Derived, FillRule, GraphicsState, ImagePaint, Matrix, PaintAtom, PaintAtomKind,
        PaintGraph, PaintId, Path, PathPaint, PathSegment, Point,
    };
    use pdf_syntax::Reference;

    use super::{HitDoubt, HitEvidence, candidates, foremost, inside_path};
    use crate::SemanticIndex;

    fn span() -> SourceSpan {
        SourceSpan::new(SourceId::new(1), 0, 1).expect("a forward span")
    }

    fn derived<T>(value: T) -> Derived<T> {
        Derived {
            value,
            provenance: pdf_paint::Provenance::new(),
        }
    }

    fn placed(x: f64, y: f64, width: f64, height: f64) -> Matrix {
        Matrix {
            a: width,
            b: 0.0,
            c: 0.0,
            d: height,
            e: x,
            f: y,
        }
    }

    fn atom(ordinal: usize, kind: PaintAtomKind) -> PaintAtom {
        PaintAtom {
            id: PaintId {
                page: Reference::new(1, 0),
                stream: Reference::new(2, 0),
                operator_span: span(),
                invocation_path: Vec::new(),
                pattern_path: Vec::new(),
                ordinal,
            },
            kind,
            marks: Vec::new(),
        }
    }

    fn image(matrix: Matrix, masked: bool) -> PaintAtomKind {
        let one = ImagePaint {
            reference: Reference::new(9, 0),
            dictionary_span: span(),
            encoded_data_span: span(),
            width: derived(1),
            height: derived(1),
            bits_per_component: derived(8),
            codec: None,
            dct_color_transform: None,
            color_space: None,
            image_mask: derived(false),
            decode: derived(Vec::new()),
            interpolate: derived(false),
            soft_mask: None,
            mask: None,
            matte: None,
            samples: Arc::from(&[0_u8][..]),
            state: GraphicsState {
                ctm: derived(matrix),
                ..GraphicsState::default()
            },
        };
        let mut outer = one.clone();
        outer.reference = Reference::new(10, 0);
        if masked {
            outer.soft_mask = Some(Box::new(one));
        }
        PaintAtomKind::Image(Box::new(outer))
    }

    fn triangle(matrix: Matrix) -> PaintAtomKind {
        PaintAtomKind::Path(PathPaint {
            path: Path {
                segments: vec![
                    PathSegment::MoveTo {
                        point: Point { x: 0.0, y: 0.0 },
                        provenance: span(),
                    },
                    PathSegment::LineTo {
                        point: Point { x: 100.0, y: 0.0 },
                        provenance: span(),
                    },
                    PathSegment::LineTo {
                        point: Point { x: 0.0, y: 100.0 },
                        provenance: span(),
                    },
                    PathSegment::ClosePath { provenance: span() },
                ],
            },
            stroke: false,
            fill: Some(FillRule::Nonzero),
            state: GraphicsState {
                ctm: derived(matrix),
                ..GraphicsState::default()
            },
        })
    }

    fn graph_of(atoms: Vec<PaintAtom>) -> (PaintGraph, SemanticIndex) {
        let graph = PaintGraph {
            object_scopes: Vec::new(),
            repairs: Vec::new(),
            skipped: Vec::new(),
            atoms,
        };
        let index = SemanticIndex::of(&graph);
        (graph, index)
    }

    #[test]
    fn candidates_come_back_front_to_back_and_the_covered_one_is_among_them() {
        let (graph, index) = graph_of(vec![
            atom(0, image(placed(0.0, 0.0, 100.0, 100.0), false)),
            atom(1, image(placed(20.0, 20.0, 40.0, 40.0), false)),
        ]);
        let under = Point { x: 30.0, y: 30.0 };

        let found = candidates(&graph, &index, under);
        assert_eq!(found.len(), 2, "the covered object was dropped");
        assert_eq!(found[0].object, 1, "the one on top is not first");
        assert_eq!(found[1].object, 0);
        assert_eq!(
            foremost(&graph, &index, under).expect("a hit").object,
            1,
            "a plain click must take the front one"
        );
    }

    #[test]
    fn a_point_in_the_box_but_not_in_the_shape_is_not_a_hit() {
        let (graph, index) = graph_of(vec![atom(0, triangle(Matrix::IDENTITY))]);

        assert_eq!(
            candidates(&graph, &index, Point { x: 10.0, y: 10.0 }).len(),
            1
        );
        assert!(
            candidates(&graph, &index, Point { x: 90.0, y: 90.0 }).is_empty(),
            "the bounding box answered for the shape"
        );
    }

    #[test]
    fn a_point_outside_the_clip_is_not_a_hit_however_the_object_is_placed() {
        let mut kind = image(placed(0.0, 0.0, 100.0, 100.0), false);
        let PaintAtomKind::Image(picture) = &mut kind else {
            unreachable!()
        };
        picture.state.clip_paths = vec![ClipPath {
            path: Path {
                segments: vec![PathSegment::Rectangle {
                    origin: Point { x: 0.0, y: 0.0 },
                    width: 50.0,
                    height: 100.0,
                    provenance: span(),
                }],
            },
            rule: FillRule::Nonzero,
            ctm: derived(Matrix::IDENTITY),
            provenance: span(),
        }];
        let (graph, index) = graph_of(vec![atom(0, kind)]);

        assert_eq!(
            candidates(&graph, &index, Point { x: 25.0, y: 50.0 }).len(),
            1
        );
        assert!(
            candidates(&graph, &index, Point { x: 75.0, y: 50.0 }).is_empty(),
            "the clipped-away half answered"
        );
    }

    #[test]
    fn a_masked_image_is_a_hit_that_names_its_own_doubt() {
        let (graph, index) = graph_of(vec![atom(0, image(placed(0.0, 0.0, 10.0, 10.0), true))]);
        let found = candidates(&graph, &index, Point { x: 5.0, y: 5.0 });

        assert_eq!(found[0].evidence, HitEvidence::Ink);
        assert_eq!(found[0].doubts, vec![HitDoubt::MaskedImage]);
        assert!(
            !found[0].certain(),
            "an undecoded mask was reported certain"
        );
    }

    #[test]
    fn a_stroke_answers_with_its_envelope_and_names_the_reason() {
        let mut kind = triangle(Matrix::IDENTITY);
        let PaintAtomKind::Path(path) = &mut kind else {
            unreachable!()
        };
        path.fill = None;
        path.stroke = true;
        let (graph, index) = graph_of(vec![atom(0, kind)]);

        let found = candidates(&graph, &index, Point { x: 90.0, y: 90.0 });
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].evidence, HitEvidence::Envelope);
        assert_eq!(found[0].doubts, vec![HitDoubt::StrokeWidth]);
    }

    #[test]
    fn a_point_on_nothing_hits_nothing() {
        let (graph, index) = graph_of(vec![atom(0, image(placed(0.0, 0.0, 10.0, 10.0), false))]);

        assert!(candidates(&graph, &index, Point { x: 50.0, y: 50.0 }).is_empty());
        assert!(foremost(&graph, &index, Point { x: 50.0, y: 50.0 }).is_none());
    }

    #[test]
    fn the_two_fill_rules_disagree_where_a_shape_overlaps_itself() {
        let path = Path {
            segments: vec![
                PathSegment::Rectangle {
                    origin: Point { x: 0.0, y: 0.0 },
                    width: 100.0,
                    height: 100.0,
                    provenance: span(),
                },
                PathSegment::Rectangle {
                    origin: Point { x: 25.0, y: 25.0 },
                    width: 50.0,
                    height: 50.0,
                    provenance: span(),
                },
            ],
        };
        let middle = Point { x: 50.0, y: 50.0 };
        let ring = Point { x: 10.0, y: 50.0 };

        assert!(inside_path(
            &path,
            Matrix::IDENTITY,
            FillRule::Nonzero,
            middle
        ));
        assert!(!inside_path(
            &path,
            Matrix::IDENTITY,
            FillRule::EvenOdd,
            middle
        ));
        assert!(inside_path(
            &path,
            Matrix::IDENTITY,
            FillRule::Nonzero,
            ring
        ));
        assert!(inside_path(
            &path,
            Matrix::IDENTITY,
            FillRule::EvenOdd,
            ring
        ));
    }

    #[test]
    fn containment_follows_the_matrix_the_path_is_placed_by() {
        let (graph, index) = graph_of(vec![atom(0, triangle(placed(200.0, 200.0, 1.0, 1.0)))]);

        assert_eq!(
            candidates(&graph, &index, Point { x: 210.0, y: 210.0 }).len(),
            1
        );
        assert!(candidates(&graph, &index, Point { x: 10.0, y: 10.0 }).is_empty());
    }

    #[test]
    fn an_open_subpath_is_closed_where_it_ends_not_where_the_path_does() {
        let apart = |closed: bool| {
            let mut segments = vec![
                PathSegment::MoveTo {
                    point: Point { x: 0.0, y: 0.0 },
                    provenance: span(),
                },
                PathSegment::LineTo {
                    point: Point { x: 10.0, y: 0.0 },
                    provenance: span(),
                },
                PathSegment::LineTo {
                    point: Point { x: 10.0, y: 10.0 },
                    provenance: span(),
                },
            ];
            if closed {
                segments.push(PathSegment::ClosePath { provenance: span() });
            }
            segments.extend([
                PathSegment::MoveTo {
                    point: Point { x: 20.0, y: 20.0 },
                    provenance: span(),
                },
                PathSegment::LineTo {
                    point: Point { x: 21.0, y: 20.0 },
                    provenance: span(),
                },
                PathSegment::LineTo {
                    point: Point { x: 21.0, y: 21.0 },
                    provenance: span(),
                },
                PathSegment::ClosePath { provenance: span() },
            ]);
            Path { segments }
        };
        let air = Point { x: 1.0, y: 8.0 };

        for rule in [FillRule::Nonzero, FillRule::EvenOdd] {
            assert!(
                !inside_path(&apart(true), Matrix::IDENTITY, rule, air),
                "the known answer: {rule:?} does not fill the air outside two triangles"
            );
            assert!(
                !inside_path(&apart(false), Matrix::IDENTITY, rule, air),
                "leaving the first subpath open must not change what {rule:?} fills"
            );
        }
        for rule in [FillRule::Nonzero, FillRule::EvenOdd] {
            assert!(inside_path(
                &apart(false),
                Matrix::IDENTITY,
                rule,
                Point { x: 8.0, y: 1.0 }
            ));
        }
    }

    #[test]
    fn a_clip_of_two_subpaths_does_not_admit_the_air_between_them() {
        let clip = Path {
            segments: vec![
                PathSegment::MoveTo {
                    point: Point { x: 0.0, y: 0.0 },
                    provenance: span(),
                },
                PathSegment::LineTo {
                    point: Point { x: 10.0, y: 0.0 },
                    provenance: span(),
                },
                PathSegment::LineTo {
                    point: Point { x: 10.0, y: 10.0 },
                    provenance: span(),
                },
                PathSegment::MoveTo {
                    point: Point { x: 60.0, y: 60.0 },
                    provenance: span(),
                },
                PathSegment::LineTo {
                    point: Point { x: 70.0, y: 60.0 },
                    provenance: span(),
                },
                PathSegment::LineTo {
                    point: Point { x: 70.0, y: 70.0 },
                    provenance: span(),
                },
            ],
        };
        let mut kind = image(placed(0.0, 0.0, 100.0, 100.0), false);
        let PaintAtomKind::Image(picture) = &mut kind else {
            unreachable!()
        };
        picture.state.clip_paths = vec![ClipPath {
            path: clip,
            rule: FillRule::Nonzero,
            ctm: derived(Matrix::IDENTITY),
            provenance: span(),
        }];
        let (graph, index) = graph_of(vec![atom(0, kind)]);

        assert_eq!(
            candidates(&graph, &index, Point { x: 8.0, y: 1.0 }).len(),
            1
        );
        assert!(candidates(&graph, &index, Point { x: 1.0, y: 8.0 }).is_empty());
    }
}
