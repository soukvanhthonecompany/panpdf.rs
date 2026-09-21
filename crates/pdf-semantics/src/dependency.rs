use std::collections::HashMap;

use pdf_bytes::SourceSpan;
use pdf_paint::{ColorSpace, PaintAtomKind, PaintGraph, SoftMask};
use pdf_syntax::Reference;

use crate::Object;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Dependency {
    Stream(Reference),
    Form(Reference),
    Pattern(Reference),
    Font(Reference),
    Image(Reference),
    Group(Reference),
    Shading(Reference),
    ExtGState(Reference),
    ColorSpace(SourceSpan),
    SoftMask(SourceSpan),
    Unfollowed,
    Clip(SourceSpan),
}

impl Dependency {
    fn key(self) -> (u8, u64, u64, u64) {
        let reference = |tag: u8, reference: Reference| {
            (
                tag,
                u64::from(reference.object_number()),
                u64::from(reference.generation()),
                0,
            )
        };
        let span = |tag: u8, span: SourceSpan| {
            (
                tag,
                span.source().get(),
                span.start() as u64,
                span.end() as u64,
            )
        };
        match self {
            Self::Stream(at) => reference(0, at),
            Self::Form(at) => reference(1, at),
            Self::Pattern(at) => reference(2, at),
            Self::Font(at) => reference(3, at),
            Self::Image(at) => reference(4, at),
            Self::Group(at) => reference(5, at),
            Self::Shading(at) => reference(6, at),
            Self::ExtGState(at) => reference(7, at),
            Self::ColorSpace(at) => span(8, at),
            Self::SoftMask(at) => span(9, at),
            Self::Clip(at) => span(10, at),
            Self::Unfollowed => (11, 0, 0, 0),
        }
    }

    #[must_use]
    pub const fn kind(self) -> &'static str {
        match self {
            Self::Stream(_) => "stream",
            Self::Form(_) => "form",
            Self::Pattern(_) => "pattern",
            Self::Font(_) => "font",
            Self::Image(_) => "image",
            Self::Group(_) => "group",
            Self::Shading(_) => "shading",
            Self::ExtGState(_) => "graphics state",
            Self::ColorSpace(_) => "colour space",
            Self::SoftMask(_) => "soft mask",
            Self::Clip(_) => "clip",
            Self::Unfollowed => "unfollowed nesting",
        }
    }

    #[must_use]
    pub const fn is_shareable_definition(self) -> bool {
        !matches!(self, Self::Stream(_) | Self::Clip(_))
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Dependencies {
    pub of: Vec<Dependency>,
}

impl Dependencies {
    #[must_use]
    pub fn on(&self, dependency: Dependency) -> bool {
        self.of.contains(&dependency)
    }

    pub fn forms(&self) -> impl Iterator<Item = Reference> + '_ {
        self.of.iter().filter_map(|dependency| match dependency {
            Dependency::Form(reference) => Some(*reference),
            _ => None,
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct DependencyIndex {
    per_object: Vec<Dependencies>,
    dependents: HashMap<Dependency, Vec<usize>>,
}

impl DependencyIndex {
    #[must_use]
    pub fn of(graph: &PaintGraph, objects: &[Object]) -> Self {
        let mut per_object = Vec::with_capacity(objects.len());
        let mut dependents: HashMap<Dependency, Vec<usize>> = HashMap::new();
        for (position, object) in objects.iter().enumerate() {
            let mut found = Vec::new();
            for atom in object.atoms() {
                if let Some(atom) = graph.atoms.get(atom) {
                    collect(atom, &mut found);
                }
            }
            found.sort_unstable_by_key(|dependency| dependency.key());
            found.dedup();
            for dependency in &found {
                let entry = dependents.entry(*dependency).or_default();
                if entry.last() != Some(&position) {
                    entry.push(position);
                }
            }
            per_object.push(Dependencies { of: found });
        }
        Self {
            per_object,
            dependents,
        }
    }

    #[must_use]
    pub fn of_object(&self, object: usize) -> Option<&Dependencies> {
        self.per_object.get(object)
    }

    #[must_use]
    pub fn dependents_of(&self, dependency: Dependency) -> &[usize] {
        self.dependents
            .get(&dependency)
            .map_or(&[], std::vec::Vec::as_slice)
    }

    #[must_use]
    pub fn shared_by(&self, object: usize) -> Vec<(Dependency, Vec<usize>)> {
        let Some(mine) = self.per_object.get(object) else {
            return Vec::new();
        };
        mine.of
            .iter()
            .filter(|dependency| dependency.is_shareable_definition())
            .filter_map(|dependency| {
                let others: Vec<usize> = self
                    .dependents_of(*dependency)
                    .iter()
                    .copied()
                    .filter(|other| *other != object)
                    .collect();
                if others.is_empty() {
                    None
                } else {
                    Some((*dependency, others))
                }
            })
            .collect()
    }

    #[must_use]
    pub fn reaches_only_its_own(&self, object: usize) -> bool {
        let Some(mine) = self.per_object.get(object) else {
            return false;
        };
        !mine.on(Dependency::Unfollowed) && self.shared_by(object).is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.per_object.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.per_object.is_empty()
    }
}

const MAX_NESTING: usize = 16;

fn collect(atom: &pdf_paint::PaintAtom, found: &mut Vec<Dependency>) {
    found.push(Dependency::Stream(atom.id.stream));
    found.extend(
        atom.id
            .invocation_path
            .iter()
            .map(|invocation| Dependency::Form(invocation.form)),
    );
    found.extend(
        atom.id
            .pattern_path
            .iter()
            .map(|invocation| Dependency::Pattern(invocation.pattern)),
    );
    collect_kind(&atom.kind, found, MAX_NESTING);
}

fn collect_kind(kind: &PaintAtomKind, found: &mut Vec<Dependency>, depth: usize) {
    let state = match kind {
        PaintAtomKind::Path(path) => &path.state,
        PaintAtomKind::Text(text) => &text.state,
        PaintAtomKind::Image(image) => &image.state,
        PaintAtomKind::Shading(shading) => &shading.state,
        PaintAtomKind::TransparencyGroup(group) => &group.state,
    };
    collect_state(state, found, depth);

    match kind {
        PaintAtomKind::Text(text) => {
            if let Some(font) = text.state.text.font.as_ref()
                && let Some(reference) = font.value.reference
            {
                found.push(Dependency::Font(reference));
            }
            for glyph in &text.glyphs {
                if let Some(procedure) = glyph.procedure.as_ref() {
                    for atom in &procedure.atoms {
                        collect_nested(&atom.kind, found, depth);
                    }
                }
            }
        }
        PaintAtomKind::Image(image) => collect_image(image, found, depth),
        PaintAtomKind::TransparencyGroup(group) => {
            found.push(Dependency::Group(group.reference));
            for atom in &group.graph.atoms {
                found.push(Dependency::Stream(atom.id.stream));
                collect_nested(&atom.kind, found, depth);
            }
        }
        PaintAtomKind::Shading(shading) => {
            if let Some(reference) = shading.reference {
                found.push(Dependency::Shading(reference));
            }
            if let Some(span) = color_space_span(&shading.color_space.value) {
                found.push(Dependency::ColorSpace(span));
            }
        }
        PaintAtomKind::Path(_) => {}
    }
}

fn collect_nested(kind: &PaintAtomKind, found: &mut Vec<Dependency>, depth: usize) {
    match depth.checked_sub(1) {
        Some(remaining) => collect_kind(kind, found, remaining),
        None => found.push(Dependency::Unfollowed),
    }
}

fn collect_image(image: &pdf_paint::ImagePaint, found: &mut Vec<Dependency>, depth: usize) {
    found.push(Dependency::Image(image.reference));
    if let Some(space) = image.color_space.as_ref()
        && let Some(span) = color_space_span(&space.value)
    {
        found.push(Dependency::ColorSpace(span));
    }
    if let Some(mask) = image.soft_mask.as_ref() {
        match depth.checked_sub(1) {
            Some(remaining) => collect_image(mask, found, remaining),
            None => found.push(Dependency::Unfollowed),
        }
    }
}

fn collect_state(state: &pdf_paint::GraphicsState, found: &mut Vec<Dependency>, depth: usize) {
    if let Some(applied) = state.ext_gstate.as_ref()
        && let Some(reference) = applied.value.reference
    {
        found.push(Dependency::ExtGState(reference));
    }
    if let SoftMask::Dictionary(mask) = &state.soft_mask.value {
        found.push(Dependency::SoftMask(mask.dictionary_span));
        found.push(Dependency::Group(mask.group.reference));
        match depth.checked_sub(1) {
            Some(remaining) => {
                for atom in &mask.group.graph.atoms {
                    found.push(Dependency::Stream(atom.id.stream));
                    collect_nested(&atom.kind, found, remaining);
                }
            }
            None => found.push(Dependency::Unfollowed),
        }
    }
    found.extend(
        state
            .clip_paths
            .iter()
            .map(|clip| Dependency::Clip(clip.provenance)),
    );
    for space in [
        &state.fill_color_space.value,
        &state.stroke_color_space.value,
    ] {
        if let Some(span) = color_space_span(space) {
            found.push(Dependency::ColorSpace(span));
        }
    }
}

fn color_space_span(space: &ColorSpace) -> Option<SourceSpan> {
    match space {
        ColorSpace::IccBased(icc) => Some(icc.dictionary_span),
        ColorSpace::Indexed(indexed) => Some(indexed.array_span),
        ColorSpace::Separation(separation) => Some(separation.array_span),
        ColorSpace::DeviceN(device_n) => Some(device_n.array_span),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdf_bytes::{SourceId, SourceSpan};
    use pdf_paint::{
        Derived, FormInvocation, GraphicsState, GroupBackdrop, ImagePaint, Matrix, PaintAtom,
        PaintAtomKind, PaintGraph, PaintId, SoftMask, SoftMaskPaint, SoftMaskSubtype,
        SoftMaskTransfer, TransparencyGroupPaint,
    };
    use pdf_syntax::Reference;

    use super::{Dependency, DependencyIndex};
    use crate::{Member, Object, ObjectKind};

    fn span(start: usize) -> SourceSpan {
        SourceSpan::new(SourceId::new(1), start, start + 1).expect("a forward span")
    }

    fn placement(ordinal: usize, image: Reference, through: &[Reference]) -> PaintAtom {
        PaintAtom {
            id: PaintId {
                page: Reference::new(1, 0),
                stream: Reference::new(2, 0),
                operator_span: span(ordinal),
                invocation_path: through
                    .iter()
                    .enumerate()
                    .map(|(at, form)| FormInvocation {
                        form: *form,
                        operator_span: span(100 + ordinal * 10 + at),
                    })
                    .collect(),
                pattern_path: Vec::new(),
                ordinal,
            },
            kind: PaintAtomKind::Image(Box::new(ImagePaint {
                reference: image,
                dictionary_span: span(200),
                encoded_data_span: span(201),
                width: Derived {
                    value: 1,
                    provenance: pdf_paint::Provenance::new(),
                },
                height: Derived {
                    value: 1,
                    provenance: pdf_paint::Provenance::new(),
                },
                bits_per_component: Derived {
                    value: 8,
                    provenance: pdf_paint::Provenance::new(),
                },
                codec: None,
                dct_color_transform: None,
                color_space: None,
                image_mask: Derived {
                    value: false,
                    provenance: pdf_paint::Provenance::new(),
                },
                decode: Derived {
                    value: Vec::new(),
                    provenance: pdf_paint::Provenance::new(),
                },
                interpolate: Derived {
                    value: false,
                    provenance: pdf_paint::Provenance::new(),
                },
                soft_mask: None,
                mask: None,
                matte: None,
                samples: Arc::from(&[0_u8][..]),
                state: GraphicsState {
                    ctm: Derived {
                        value: Matrix::IDENTITY,
                        provenance: pdf_paint::Provenance::new(),
                    },
                    ..GraphicsState::default()
                },
            })),
            marks: Vec::new(),
        }
    }

    fn transparency_group(reference: Reference, atoms: Vec<PaintAtom>) -> TransparencyGroupPaint {
        TransparencyGroupPaint {
            reference,
            dictionary_span: span(400),
            group_span: span(401),
            group_reference_span: None,
            subtype_span: span(402),
            bbox: [0.0, 0.0, 1.0, 1.0],
            bbox_span: span(403),
            matrix: Derived {
                value: Matrix::IDENTITY,
                provenance: pdf_paint::Provenance::new(),
            },
            isolated: Derived {
                value: true,
                provenance: pdf_paint::Provenance::new(),
            },
            knockout: Derived {
                value: false,
                provenance: pdf_paint::Provenance::new(),
            },
            blend_space: None,
            backdrop: Derived {
                value: GroupBackdrop::Transparent,
                provenance: pdf_paint::Provenance::new(),
            },
            state: GraphicsState::default(),
            graph: PaintGraph {
                object_scopes: Vec::new(),
                repairs: Vec::new(),
                skipped: Vec::new(),
                atoms,
            },
        }
    }

    fn group_atom(ordinal: usize, reference: Reference, atoms: Vec<PaintAtom>) -> PaintAtom {
        PaintAtom {
            id: PaintId {
                page: Reference::new(1, 0),
                stream: Reference::new(2, 0),
                operator_span: span(ordinal),
                invocation_path: Vec::new(),
                pattern_path: Vec::new(),
                ordinal,
            },
            kind: PaintAtomKind::TransparencyGroup(Box::new(transparency_group(reference, atoms))),
            marks: Vec::new(),
        }
    }

    fn object(atom: usize) -> Object {
        Object {
            kind: ObjectKind::Image,
            members: vec![Member::whole(atom)],
            quad: None,
            bounds: None,
        }
    }

    #[test]
    fn a_form_invoked_twice_is_reported_as_shared_by_both_occurrences() {
        let form = Reference::new(12, 0);
        let graph = PaintGraph {
            object_scopes: Vec::new(),
            repairs: Vec::new(),
            skipped: Vec::new(),
            atoms: vec![
                placement(0, Reference::new(10, 0), &[form]),
                placement(1, Reference::new(11, 0), &[form]),
            ],
        };
        let index = DependencyIndex::of(&graph, &[object(0), object(1)]);

        assert_eq!(index.dependents_of(Dependency::Form(form)), [0, 1]);
        let shared = index.shared_by(0);
        assert_eq!(shared.len(), 1, "{shared:?}");
        assert_eq!(shared[0].0, Dependency::Form(form));
        assert_eq!(shared[0].1, vec![1]);
        assert!(!index.reaches_only_its_own(0));
        assert!(!index.reaches_only_its_own(1));
    }

    #[test]
    fn an_image_used_as_another_image_s_mask_is_a_user_of_it() {
        let masked = Reference::new(10, 0);
        let mask = Reference::new(20, 0);
        let mut first = placement(0, masked, &[]);
        if let PaintAtomKind::Image(image) = &mut first.kind {
            let mut inner = placement(9, mask, &[]);
            let PaintAtomKind::Image(inner) = &mut inner.kind else {
                unreachable!()
            };
            image.soft_mask = Some(inner.clone());
        }
        let graph = PaintGraph {
            object_scopes: Vec::new(),
            repairs: Vec::new(),
            skipped: Vec::new(),
            atoms: vec![first, placement(1, mask, &[])],
        };
        let index = DependencyIndex::of(&graph, &[object(0), object(1)]);

        assert_eq!(
            index.dependents_of(Dependency::Image(mask)),
            [0, 1],
            "the masked picture is a user of its mask"
        );
        assert!(!index.reaches_only_its_own(0));
        assert!(!index.reaches_only_its_own(1));
    }

    #[test]
    fn one_image_resource_placed_twice_is_reported_as_shared() {
        let image = Reference::new(10, 0);
        let graph = PaintGraph {
            object_scopes: Vec::new(),
            repairs: Vec::new(),
            skipped: Vec::new(),
            atoms: vec![placement(0, image, &[]), placement(1, image, &[])],
        };
        let index = DependencyIndex::of(&graph, &[object(0), object(1)]);

        assert_eq!(index.dependents_of(Dependency::Image(image)), [0, 1]);
        assert_eq!(index.shared_by(1)[0].0, Dependency::Image(image));
    }

    #[test]
    fn sharing_the_page_stream_is_not_sharing_a_definition() {
        let graph = PaintGraph {
            object_scopes: Vec::new(),
            repairs: Vec::new(),
            skipped: Vec::new(),
            atoms: vec![
                placement(0, Reference::new(10, 0), &[]),
                placement(1, Reference::new(11, 0), &[]),
            ],
        };
        let index = DependencyIndex::of(&graph, &[object(0), object(1)]);

        assert_eq!(
            index.dependents_of(Dependency::Stream(Reference::new(2, 0))),
            [0, 1]
        );
        assert!(
            index
                .of_object(0)
                .expect("the object")
                .on(Dependency::Stream(Reference::new(2, 0)))
        );
        assert!(index.reaches_only_its_own(0), "{:?}", index.shared_by(0));
        assert!(index.reaches_only_its_own(1));
    }

    #[test]
    fn a_form_invoked_once_is_not_shared() {
        let graph = PaintGraph {
            object_scopes: Vec::new(),
            repairs: Vec::new(),
            skipped: Vec::new(),
            atoms: vec![
                placement(0, Reference::new(10, 0), &[Reference::new(12, 0)]),
                placement(1, Reference::new(11, 0), &[Reference::new(13, 0)]),
            ],
        };
        let index = DependencyIndex::of(&graph, &[object(0), object(1)]);

        assert!(index.reaches_only_its_own(0));
        assert_eq!(
            index
                .of_object(0)
                .expect("the object")
                .forms()
                .collect::<Vec<_>>(),
            vec![Reference::new(12, 0)]
        );
    }

    #[test]
    fn one_object_using_a_form_twice_shares_it_with_nobody() {
        let form = Reference::new(12, 0);
        let graph = PaintGraph {
            object_scopes: Vec::new(),
            repairs: Vec::new(),
            skipped: Vec::new(),
            atoms: vec![
                placement(0, Reference::new(10, 0), &[form]),
                placement(1, Reference::new(11, 0), &[form]),
            ],
        };
        let both = Object {
            kind: ObjectKind::Image,
            members: vec![Member::whole(0), Member::whole(1)],
            quad: None,
            bounds: None,
        };
        let index = DependencyIndex::of(&graph, &[both]);

        assert_eq!(index.dependents_of(Dependency::Form(form)), [0]);
        assert!(index.reaches_only_its_own(0));
    }

    #[test]
    fn a_resource_inside_a_transparency_group_is_reached_by_the_group() {
        let inner = Reference::new(20, 0);
        let group = group_atom(0, Reference::new(30, 0), vec![placement(9, inner, &[])]);
        let graph = PaintGraph {
            object_scopes: Vec::new(),
            repairs: Vec::new(),
            skipped: Vec::new(),
            atoms: vec![group, placement(1, inner, &[])],
        };
        let index = DependencyIndex::of(&graph, &[object(0), object(1)]);

        assert_eq!(index.dependents_of(Dependency::Image(inner)), [0, 1]);
        assert!(!index.reaches_only_its_own(0));
    }

    #[test]
    fn a_state_soft_mask_reaches_the_group_it_paints_and_its_resources() {
        let inner = Reference::new(20, 0);
        let mask_group = Reference::new(40, 0);
        let mut masked = placement(0, Reference::new(10, 0), &[]);
        if let PaintAtomKind::Image(image) = &mut masked.kind {
            image.state.soft_mask = Derived {
                value: SoftMask::Dictionary(Arc::new(SoftMaskPaint {
                    dictionary_span: span(300),
                    dictionary_reference_span: None,
                    subtype: Derived {
                        value: SoftMaskSubtype::Alpha,
                        provenance: pdf_paint::Provenance::new(),
                    },
                    group: Box::new(transparency_group(
                        mask_group,
                        vec![placement(9, inner, &[])],
                    )),
                    backdrop_color: None,
                    transfer: Derived {
                        value: SoftMaskTransfer::Identity,
                        provenance: pdf_paint::Provenance::new(),
                    },
                })),
                provenance: pdf_paint::Provenance::new(),
            };
        }
        let graph = PaintGraph {
            object_scopes: Vec::new(),
            repairs: Vec::new(),
            skipped: Vec::new(),
            atoms: vec![masked, placement(1, inner, &[])],
        };
        let index = DependencyIndex::of(&graph, &[object(0), object(1)]);

        assert!(
            index
                .of_object(0)
                .expect("the object")
                .on(Dependency::Group(mask_group))
        );
        assert_eq!(index.dependents_of(Dependency::Image(inner)), [0, 1]);
    }

    #[test]
    fn an_object_whose_nesting_was_not_followed_never_claims_isolation() {
        let mut atom = placement(0, Reference::new(10, 0), &[]);
        for step in 0..=u32::try_from(super::MAX_NESTING).expect("a small bound") {
            let PaintAtomKind::Image(image) = &mut atom.kind else {
                unreachable!()
            };
            let mut outer = placement(step as usize + 1, Reference::new(100 + step, 0), &[]);
            let PaintAtomKind::Image(outer_image) = &mut outer.kind else {
                unreachable!()
            };
            outer_image.soft_mask = Some(image.clone());
            atom = outer;
        }
        let graph = PaintGraph {
            object_scopes: Vec::new(),
            repairs: Vec::new(),
            skipped: Vec::new(),
            atoms: vec![atom],
        };
        let index = DependencyIndex::of(&graph, &[object(0)]);

        assert!(
            index
                .of_object(0)
                .expect("the object")
                .on(Dependency::Unfollowed)
        );
        assert!(
            !index.reaches_only_its_own(0),
            "an unfinished walk reported isolation"
        );
    }

    #[test]
    fn the_dependency_list_is_in_a_stable_order() {
        let graph = PaintGraph {
            object_scopes: Vec::new(),
            repairs: Vec::new(),
            skipped: Vec::new(),
            atoms: vec![placement(
                0,
                Reference::new(10, 0),
                &[Reference::new(30, 0), Reference::new(20, 0)],
            )],
        };
        let index = DependencyIndex::of(&graph, &[object(0)]);
        let listed = &index.of_object(0).expect("the object").of;

        assert_eq!(
            listed,
            &vec![
                Dependency::Stream(Reference::new(2, 0)),
                Dependency::Form(Reference::new(20, 0)),
                Dependency::Form(Reference::new(30, 0)),
                Dependency::Image(Reference::new(10, 0)),
            ]
        );
    }
}
