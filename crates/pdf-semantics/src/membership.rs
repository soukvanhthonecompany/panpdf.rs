use std::collections::BTreeMap;
use std::ops::Range;

use pdf_paint::{PaintAtomKind, PaintGraph};

use crate::{ClusterReport, Object, SemanticIndex};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum InkKind {
    Path,
    Text,
    Image,
    Shading,
    TransparencyGroup,
}

impl InkKind {
    #[must_use]
    pub const fn of(kind: &PaintAtomKind) -> Self {
        match kind {
            PaintAtomKind::Path(_) => Self::Path,
            PaintAtomKind::Text(_) => Self::Text,
            PaintAtomKind::Image(_) => Self::Image,
            PaintAtomKind::Shading(_) => Self::Shading,
            PaintAtomKind::TransparencyGroup(_) => Self::TransparencyGroup,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::Text => "text",
            Self::Image => "image",
            Self::Shading => "shading",
            Self::TransparencyGroup => "transparency group",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Orphan {
    pub atom: usize,
    pub kind: InkKind,
    pub unowned: usize,
    pub units: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Contested {
    pub atom: usize,
    pub kind: InkKind,
    pub owners: Vec<usize>,
    pub overlapping: usize,
    pub units: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Dangling {
    pub object: usize,
    pub atom: usize,
    pub past_glyph: Option<usize>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MembershipAudit {
    pub atoms: usize,
    pub owned: usize,
    pub orphans: Vec<Orphan>,
    pub contested: Vec<Contested>,
    pub dangling: Vec<Dangling>,
    pub nested: usize,
}

fn units(kind: &PaintAtomKind) -> usize {
    match kind {
        PaintAtomKind::Text(text) if !text.glyphs.is_empty() => text.glyphs.len(),
        _ => 1,
    }
}

impl MembershipAudit {
    #[must_use]
    pub fn of(graph: &PaintGraph, objects: &[Object]) -> Self {
        let mut claims: BTreeMap<usize, BTreeMap<usize, Vec<Range<usize>>>> = BTreeMap::new();
        let mut dangling = Vec::new();
        for (position, object) in objects.iter().enumerate() {
            for member in &object.members {
                let Some(atom) = graph.atoms.get(member.atom) else {
                    dangling.push(Dangling {
                        object: position,
                        atom: member.atom,
                        past_glyph: None,
                    });
                    continue;
                };
                let units = units(&atom.kind);
                let range = match &member.glyphs {
                    None => 0..units,
                    Some(range) => range.clone(),
                };
                if range.end > units {
                    dangling.push(Dangling {
                        object: position,
                        atom: member.atom,
                        past_glyph: Some(range.end),
                    });
                    continue;
                }
                if range.is_empty() {
                    continue;
                }
                claims
                    .entry(member.atom)
                    .or_default()
                    .entry(position)
                    .or_default()
                    .push(range);
            }
        }

        let (mut owned, mut orphans, mut contested) = (0, Vec::new(), Vec::new());
        for (index, atom) in graph.atoms.iter().enumerate() {
            let kind = InkKind::of(&atom.kind);
            let units = units(&atom.kind);
            let mut cover = vec![0_u32; units];
            let mut owners = Vec::new();
            if let Some(by_owner) = claims.get(&index) {
                for (owner, ranges) in by_owner {
                    owners.push(*owner);
                    for unit in merged(ranges).flat_map(std::iter::IntoIterator::into_iter) {
                        cover[unit] += 1;
                    }
                }
            }
            let unowned = cover.iter().filter(|count| **count == 0).count();
            let overlapping = cover.iter().filter(|count| **count > 1).count();
            if unowned == 0 && overlapping == 0 {
                owned += 1;
            }
            if unowned > 0 {
                orphans.push(Orphan {
                    atom: index,
                    kind,
                    unowned,
                    units,
                });
            }
            if overlapping > 0 {
                contested.push(Contested {
                    atom: index,
                    kind,
                    owners,
                    overlapping,
                    units,
                });
            }
        }

        Self {
            atoms: graph.atoms.len(),
            owned,
            orphans,
            contested,
            dangling,
            nested: nested_atoms(graph),
        }
    }

    #[must_use]
    pub fn of_index(graph: &PaintGraph, index: &SemanticIndex) -> Self {
        Self::of(graph, &index.objects)
    }

    #[must_use]
    pub fn accounted(&self) -> bool {
        self.orphans.is_empty() && self.contested.is_empty() && self.dangling.is_empty()
    }

    #[must_use]
    pub fn orphans_by_kind(&self) -> BTreeMap<InkKind, usize> {
        let mut counts = BTreeMap::new();
        for orphan in &self.orphans {
            *counts.entry(orphan.kind).or_insert(0) += 1;
        }
        counts
    }

    #[must_use]
    pub fn unowned_units(&self) -> usize {
        self.orphans.iter().map(|orphan| orphan.unowned).sum()
    }

    #[must_use]
    pub fn contested_units(&self) -> usize {
        self.contested
            .iter()
            .map(|contested| contested.overlapping)
            .sum()
    }

    #[must_use]
    pub fn agrees_with_report(&self, report: &ClusterReport) -> bool {
        self.orphans_by_kind()
            .get(&InkKind::Path)
            .copied()
            .unwrap_or(0)
            == report.paths_outside_any_object
    }
}

fn merged(ranges: &[Range<usize>]) -> impl Iterator<Item = Range<usize>> + use<> {
    let mut sorted: Vec<Range<usize>> = ranges.to_vec();
    sorted.sort_by_key(|range| (range.start, range.end));
    let mut out: Vec<Range<usize>> = Vec::new();
    for range in sorted {
        match out.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => out.push(range),
        }
    }
    out.into_iter()
}

fn nested_atoms(graph: &PaintGraph) -> usize {
    fn count(kind: &PaintAtomKind) -> usize {
        match kind {
            PaintAtomKind::TransparencyGroup(group) => {
                group.graph.atoms.len()
                    + group
                        .graph
                        .atoms
                        .iter()
                        .map(|atom| count(&atom.kind))
                        .sum::<usize>()
            }
            PaintAtomKind::Text(text) => text
                .glyphs
                .iter()
                .filter_map(|glyph| glyph.procedure.as_ref())
                .map(|procedure| {
                    procedure.atoms.len()
                        + procedure
                            .atoms
                            .iter()
                            .map(|atom| count(&atom.kind))
                            .sum::<usize>()
                })
                .sum(),
            PaintAtomKind::Path(_) | PaintAtomKind::Image(_) | PaintAtomKind::Shading(_) => 0,
        }
    }
    graph.atoms.iter().map(|atom| count(&atom.kind)).sum()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdf_bytes::{SourceId, SourceSpan};
    use pdf_font::SourceCode;
    use pdf_paint::{
        Derived, FillRule, GraphicsState, Matrix, PaintAtom, PaintAtomKind, PaintGraph, PaintId,
        Path, PathPaint, PathSegment, Point, PositionedGlyph, TextMatrices, TextShowPaint,
        Type3Glyph,
    };
    use pdf_syntax::Reference;

    use super::{InkKind, MembershipAudit};
    use crate::{Member, Object, ObjectKind};

    fn span() -> SourceSpan {
        SourceSpan::new(SourceId::new(1), 0, 1).expect("a forward span")
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

    fn path(at: f64) -> PaintAtomKind {
        PaintAtomKind::Path(PathPaint {
            path: Path {
                segments: vec![
                    PathSegment::MoveTo {
                        point: Point { x: at, y: 0.0 },
                        provenance: span(),
                    },
                    PathSegment::LineTo {
                        point: Point {
                            x: at + 10.0,
                            y: 0.0,
                        },
                        provenance: span(),
                    },
                ],
            },
            stroke: false,
            fill: Some(FillRule::Nonzero),
            state: GraphicsState::default(),
        })
    }

    fn five_paths() -> PaintGraph {
        PaintGraph {
            object_scopes: Vec::new(),
            repairs: Vec::new(),
            skipped: Vec::new(),
            atoms: (0..5)
                .map(|index| {
                    #[allow(clippy::cast_precision_loss)]
                    atom(index, path(index as f64 * 20.0))
                })
                .collect(),
        }
    }

    fn one_each(count: usize) -> Vec<Object> {
        (0..count)
            .map(|index| Object {
                kind: ObjectKind::Image,
                members: vec![Member::whole(index)],
                quad: None,
                bounds: None,
            })
            .collect()
    }

    #[test]
    fn a_complete_membership_accounts_for_every_atom_once() {
        let graph = five_paths();
        let audit = MembershipAudit::of(&graph, &one_each(5));
        assert!(audit.accounted(), "{audit:?}");
        assert_eq!(audit.atoms, 5);
        assert_eq!(audit.owned, 5);
        assert_eq!(audit.nested, 0);
    }

    #[test]
    fn one_atom_dropped_on_purpose_is_reported_as_orphan_ink() {
        let graph = five_paths();
        let mut objects = one_each(5);
        objects.remove(3);

        let audit = MembershipAudit::of(&graph, &objects);
        assert!(!audit.accounted(), "the dropped atom went unnoticed");
        assert_eq!(audit.owned, 4);
        assert_eq!(
            audit
                .orphans
                .iter()
                .map(|orphan| orphan.atom)
                .collect::<Vec<_>>(),
            vec![3]
        );
        assert_eq!(audit.orphans_by_kind().get(&InkKind::Path), Some(&1));
    }

    #[test]
    fn an_atom_claimed_twice_is_reported_against_both_claimants() {
        let graph = five_paths();
        let mut objects = one_each(5);
        objects[1].members.push(Member::whole(2));

        let audit = MembershipAudit::of(&graph, &objects);
        assert!(!audit.accounted());
        assert_eq!(audit.owned, 4);
        assert!(audit.orphans.is_empty());
        assert_eq!(audit.contested.len(), 1);
        assert_eq!(audit.contested[0].atom, 2);
        assert_eq!(audit.contested[0].owners, vec![1, 2]);
    }

    #[test]
    fn a_selection_that_swept_up_its_neighbours_is_reported() {
        let graph = five_paths();
        let objects = vec![
            Object {
                kind: ObjectKind::Image,
                members: vec![Member::whole(0), Member::whole(1), Member::whole(2)],
                quad: None,
                bounds: None,
            },
            Object {
                kind: ObjectKind::Image,
                members: vec![Member::whole(2), Member::whole(3), Member::whole(4)],
                quad: None,
                bounds: None,
            },
        ];

        let audit = MembershipAudit::of(&graph, &objects);
        assert!(!audit.accounted());
        assert_eq!(audit.contested.len(), 1);
        assert_eq!(audit.contested[0].atom, 2);
        assert_eq!(audit.owned, 4);
    }

    #[test]
    fn a_claim_on_an_atom_the_graph_does_not_have_is_reported() {
        let graph = five_paths();
        let mut objects = one_each(5);
        objects[0].members.push(Member::whole(9));

        let audit = MembershipAudit::of(&graph, &objects);
        assert!(!audit.accounted());
        assert_eq!(audit.dangling.len(), 1);
        assert_eq!(audit.dangling[0].object, 0);
        assert_eq!(audit.dangling[0].atom, 9);
        assert_eq!(audit.owned, 5);
        assert!(audit.orphans.is_empty());
    }

    #[test]
    fn one_object_naming_an_atom_twice_is_one_claim_not_a_conflict() {
        let graph = five_paths();
        let mut objects = one_each(5);
        objects[4].members.push(Member::whole(4));

        let audit = MembershipAudit::of(&graph, &objects);
        assert!(audit.accounted(), "{audit:?}");
        assert_eq!(audit.owned, 5);
    }

    fn type3_text(procedure_atoms: usize) -> PaintAtomKind {
        let procedure = Arc::new(Type3Glyph {
            name: b"alpha".to_vec(),
            reference: Reference::new(7, 0),
            atoms: (0..procedure_atoms)
                .map(|index| atom(index, path(0.0)))
                .collect(),
            shape_only: true,
        });
        PaintAtomKind::Text(TextShowPaint {
            elements: Vec::new(),
            state: GraphicsState::default(),
            matrices: TextMatrices {
                text: Derived {
                    value: Matrix::IDENTITY,
                    provenance: pdf_paint::Provenance::new(),
                },
                line: Derived {
                    value: Matrix::IDENTITY,
                    provenance: pdf_paint::Provenance::new(),
                },
            },
            glyphs: vec![PositionedGlyph {
                code: SourceCode {
                    bytes: vec![65],
                    value: 65,
                    byte_offset: 0,
                    cid: None,
                    mapping_span: None,
                    completed_bytes: 0,
                    width: 600.0,
                },
                glyph: None,
                matrix: Matrix::IDENTITY,
                text_matrix: Matrix::IDENTITY,
                procedure: Some(procedure),
                substituted: Vec::new(),
                silent: false,
                unresolved: None,
            }],
            program: None,
            units_per_em: 1000,
            text: Arc::default(),
            substitution: None,
            font_request: None,
            type3: true,
            family_line: None,
        })
    }

    #[test]
    fn ink_inside_a_procedure_is_counted_apart_from_what_is_owned() {
        let graph = PaintGraph {
            object_scopes: Vec::new(),
            repairs: Vec::new(),
            skipped: Vec::new(),
            atoms: vec![atom(0, type3_text(3))],
        };
        let audit = MembershipAudit::of(&graph, &one_each(1));

        assert!(audit.accounted());
        assert_eq!(audit.atoms, 1);
        assert_eq!(audit.owned, 1);
        assert_eq!(audit.nested, 3, "the procedure's own ink was not counted");
    }

    #[test]
    fn a_page_with_no_ink_is_accounted_for() {
        let audit = MembershipAudit::of(&PaintGraph::default(), &[]);
        assert!(audit.accounted());
        assert_eq!(audit.atoms, 0);
        assert_eq!(audit.owned, 0);
    }
}
