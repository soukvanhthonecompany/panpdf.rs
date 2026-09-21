use std::fmt::Write as _;

use pdf_bytes::ByteStore;
use pdf_paint::Matrix;
use pdf_syntax::Reference;

use crate::incremental::{
    ObjectBody, ObjectWrite, ProtectionPolicy, Restrictions, append_object_writes_bounded,
};
use crate::spike_move_text::SpikeError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TextRunSelection {
    Last,
    Ordinal(usize),
    Anchored(SourceAnchor),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextRange {
    pub anchor: SourceAnchor,
    pub glyphs: std::ops::Range<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunRewrite {
    pub anchor: SourceAnchor,
    pub glyphs: Option<GlyphChange>,
    pub displace: (f64, f64),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClusterRef {
    pub anchor: SourceAnchor,
    pub glyphs: std::ops::Range<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GlyphChange {
    Replace {
        glyphs: std::ops::Range<usize>,
        text: String,
    },
    Remove {
        glyphs: std::ops::Range<usize>,
        close_gap: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceAnchor {
    pub stream: Reference,
    pub operator_offset: usize,
    pub invocation_path: Vec<(Reference, usize)>,
}

impl SourceAnchor {
    #[must_use]
    pub fn of(id: &pdf_paint::PaintId) -> Self {
        Self {
            stream: id.stream,
            operator_offset: id.operator_span.start(),
            invocation_path: id
                .invocation_path
                .iter()
                .map(|invocation| (invocation.form, invocation.operator_span.start()))
                .collect(),
        }
    }

    #[must_use]
    pub fn names(&self, id: &pdf_paint::PaintId) -> bool {
        *self == Self::of(id)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObjectSelection {
    Painted(SourceAnchor),
    Text(Vec<SourceAnchor>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FixedPoint {
    Origin,
    At(pdf_paint::Point),
}

impl FixedPoint {
    #[must_use]
    pub fn applied_to(self, transform: Matrix) -> Matrix {
        match self {
            Self::Origin => transform,
            Self::At(point) => {
                const fn translation(dx: f64, dy: f64) -> Matrix {
                    Matrix {
                        a: 1.0,
                        b: 0.0,
                        c: 0.0,
                        d: 1.0,
                        e: dx,
                        f: dy,
                    }
                }
                translation(point.x, point.y)
                    .multiply(transform)
                    .multiply(translation(-point.x, -point.y))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    MoveTextRun {
        page_index: usize,
        selection: TextRunSelection,
        dx: f64,
        dy: f64,
    },
    MoveTextCluster {
        page_index: usize,
        selection: TextRunSelection,
        glyphs: std::ops::Range<usize>,
        dx: f64,
        dy: f64,
    },
    DeleteTextClusters {
        page_index: usize,
        selection: TextRunSelection,
        glyphs: std::ops::Range<usize>,
    },
    RewriteText {
        page_index: usize,
        runs: Vec<RunRewrite>,
    },
    MoveTextBlock {
        page_index: usize,
        runs: Vec<SourceAnchor>,
        dx: f64,
        dy: f64,
    },
    MoveGroup {
        page_index: usize,
        runs: Vec<SourceAnchor>,
        objects: Vec<SourceAnchor>,
        dx: f64,
        dy: f64,
    },
    SetDocumentInfo {
        edit: crate::info::InfoEdit,
    },
    AddField {
        page_index: usize,
        rect: [f64; 4],
        kind: crate::new_field::NewFieldKind,
        name: Option<String>,
        options: Vec<String>,
    },
    SetFieldBoxes {
        page_index: usize,
        boxes: Vec<(Reference, [f64; 4])>,
    },
    CopyFields {
        page_index: usize,
        copies: Vec<(Reference, [f64; 4])>,
    },
    AddLink {
        page_index: usize,
        rect: [f64; 4],
        target: crate::link::Target,
        look: crate::link::Look,
    },
    AddLinks {
        page_index: usize,
        links: Vec<([f64; 4], crate::link::Target, crate::link::Look)>,
    },
    SetLinkProperties {
        page_index: usize,
        link: Reference,
        target: Option<crate::link::Target>,
        look: Option<crate::link::Look>,
    },
    SetLinkBox {
        page_index: usize,
        link: Reference,
        rect: [f64; 4],
    },
    SetLinkBoxes {
        page_index: usize,
        boxes: Vec<(Reference, [f64; 4])>,
    },
    RemoveLinks {
        page_index: usize,
        links: Vec<Reference>,
    },
    RemoveLink {
        page_index: usize,
        link: Reference,
    },
    ChangeOutline {
        page_index: usize,
        change: crate::outline::Change,
    },
    ChangeNaming {
        page_index: usize,
        change: crate::destination::Naming,
    },
    SetTabOrder {
        page_index: usize,
        widgets: Vec<Reference>,
        order: crate::tab_order::TabOrder,
    },
    RemoveFields {
        page_index: usize,
        widgets: Vec<Reference>,
    },
    SetFieldBox {
        page_index: usize,
        widget: Reference,
        rect: [f64; 4],
    },
    SetFieldSettings {
        page_index: usize,
        widget: Reference,
        settings: crate::field_settings::FieldSettings,
    },
    RemoveField {
        page_index: usize,
        widget: Reference,
    },
    FillField {
        page_index: usize,
        widget: Reference,
        value: crate::form::FieldValue,
    },
    SetTextShape {
        page_index: usize,
        runs: Vec<SourceAnchor>,
        turn: Option<f64>,
        slant: Option<f64>,
    },
    SetTextSize {
        page_index: usize,
        runs: Vec<SourceAnchor>,
        points: f64,
    },
    RewriteBlock {
        page_index: usize,
        rows: Vec<Vec<ClusterRef>>,
        frame: (f64, f64),
        edges: (usize, usize),
        breaks: Option<RowEnds>,
        frame_declared: bool,
        range: BlockRange,
        text: String,
        paragraph: ParagraphLayout,
    },
    RewriteEmptyBlock {
        page_index: usize,
        run: crate::SourceAnchor,
        frame: (f64, f64),
        text: String,
        style: Option<TextStyle>,
        paragraph: ParagraphLayout,
    },
    RewriteBlockInStyle {
        page_index: usize,
        rows: Vec<Vec<ClusterRef>>,
        frame: (f64, f64),
        edges: (usize, usize),
        breaks: Option<RowEnds>,
        frame_declared: bool,
        range: BlockRange,
        text: String,
        style: TextStyle,
        paragraph: ParagraphLayout,
    },
    StyleBlock {
        page_index: usize,
        rows: Vec<Vec<ClusterRef>>,
        frame: (f64, f64),
        edges: (usize, usize),
        breaks: Option<RowEnds>,
        range: BlockRange,
        style: TextStyle,
        paragraph: ParagraphLayout,
    },
    ShiftBlock {
        page_index: usize,
        rows: Vec<Vec<ClusterRef>>,
        frame: (f64, f64),
        edges: (usize, usize),
        breaks: Option<RowEnds>,
        dx: f64,
        dy: f64,
        paragraph: ParagraphLayout,
    },
    RemoveObject {
        page_index: usize,
        target: SourceAnchor,
    },
    PlaceNewText {
        page_index: usize,
        frame: [f64; 4],
        text: String,
        family: String,
        size: f64,
        bold: bool,
        italic: bool,
        fill: Option<[f64; 3]>,
        paragraph: ParagraphLayout,
    },
    Stamp {
        page_index: usize,
        stamp: crate::stamp::Stamp,
        facts: crate::stamp::Facts,
        share_from: Option<usize>,
    },
    TextLayer {
        page_index: usize,
        layer: crate::text_layer::TextLayer,
        share_from: Option<usize>,
    },
    PlaceNewImage {
        page_index: usize,
        placement: Matrix,
        file: std::sync::Arc<[u8]>,
    },
    DrawPath {
        page_index: usize,
        steps: Vec<PenStep>,
        closed: bool,
        stroke: Option<PenStroke>,
        fill: Option<[f64; 3]>,
    },
    AddBlankPage {
        beside: usize,
        before: bool,
        size: [f64; 2],
    },
    RemovePages {
        pages: Vec<usize>,
    },
    MovePages {
        pages: Vec<usize>,
        to: usize,
    },
    RotatePages {
        pages: Vec<usize>,
        quarter_turns: i32,
    },
    InsertPages {
        beside: usize,
        before: bool,
        document: std::sync::Arc<[u8]>,
        pages: Vec<usize>,
    },
    PlaceObject {
        page_index: usize,
        target: ObjectSelection,
        transform: Matrix,
        about: FixedPoint,
    },
}

impl Command {
    #[must_use]
    pub fn page_index(&self) -> usize {
        match self {
            Self::MoveTextRun { page_index, .. }
            | Self::MoveTextCluster { page_index, .. }
            | Self::DeleteTextClusters { page_index, .. }
            | Self::RewriteText { page_index, .. }
            | Self::MoveTextBlock { page_index, .. }
            | Self::MoveGroup { page_index, .. }
            | Self::PlaceObject { page_index, .. }
            | Self::PlaceNewText { page_index, .. }
            | Self::Stamp { page_index, .. }
            | Self::TextLayer { page_index, .. }
            | Self::PlaceNewImage { page_index, .. }
            | Self::DrawPath { page_index, .. }
            | Self::FillField { page_index, .. }
            | Self::AddField { page_index, .. }
            | Self::RemoveField { page_index, .. }
            | Self::SetFieldBox { page_index, .. }
            | Self::SetFieldBoxes { page_index, .. }
            | Self::CopyFields { page_index, .. }
            | Self::RemoveFields { page_index, .. }
            | Self::SetTabOrder { page_index, .. }
            | Self::ChangeOutline { page_index, .. }
            | Self::ChangeNaming { page_index, .. }
            | Self::AddLink { page_index, .. }
            | Self::AddLinks { page_index, .. }
            | Self::SetLinkProperties { page_index, .. }
            | Self::SetLinkBox { page_index, .. }
            | Self::RemoveLink { page_index, .. }
            | Self::SetLinkBoxes { page_index, .. }
            | Self::RemoveLinks { page_index, .. }
            | Self::SetFieldSettings { page_index, .. }
            | Self::RemoveObject { page_index, .. }
            | Self::SetTextSize { page_index, .. }
            | Self::SetTextShape { page_index, .. }
            | Self::RewriteBlock { page_index, .. }
            | Self::RewriteBlockInStyle { page_index, .. }
            | Self::RewriteEmptyBlock { page_index, .. }
            | Self::StyleBlock { page_index, .. }
            | Self::ShiftBlock { page_index, .. } => *page_index,
            Self::AddBlankPage { beside, .. } | Self::InsertPages { beside, .. } => *beside,
            Self::RemovePages { pages }
            | Self::MovePages { pages, .. }
            | Self::RotatePages { pages, .. } => match pages.first() {
                Some(page) => *page,
                None => 0,
            },
            Self::SetDocumentInfo { .. } => 0,
        }
    }

    #[must_use]
    pub const fn clusters_after(&self) -> ClustersAfter {
        match self {
            Self::AddLink { .. }
            | Self::AddLinks { .. }
            | Self::SetLinkProperties { .. }
            | Self::SetLinkBox { .. }
            | Self::SetLinkBoxes { .. }
            | Self::RemoveLink { .. }
            | Self::RemoveLinks { .. }
            | Self::ChangeOutline { .. }
            | Self::ChangeNaming { .. }
            | Self::SetTabOrder { .. }
            | Self::AddField { .. }
            | Self::RemoveField { .. }
            | Self::RemoveFields { .. }
            | Self::SetFieldBox { .. }
            | Self::SetFieldBoxes { .. }
            | Self::SetFieldSettings { .. }
            | Self::CopyFields { .. }
            | Self::FillField { .. }
            | Self::SetDocumentInfo { .. }
            | Self::RotatePages { .. }
            | Self::PlaceNewText { .. }
            | Self::PlaceNewImage { .. }
            | Self::DrawPath { .. }
            | Self::Stamp { .. }
            | Self::TextLayer { .. }
            | Self::PlaceObject { .. }
            | Self::MoveTextRun { .. }
            | Self::MoveTextBlock { .. }
            | Self::MoveGroup { .. }
            | Self::SetTextSize { .. }
            | Self::SetTextShape { .. } => ClustersAfter::TheSame,
            Self::RewriteBlock { .. }
            | Self::RewriteBlockInStyle { .. }
            | Self::RewriteEmptyBlock { .. }
            | Self::StyleBlock { .. }
            | Self::ShiftBlock { .. }
            | Self::RewriteText { .. }
            | Self::RemoveObject { .. } => ClustersAfter::ThePlannerSays,
            Self::AddBlankPage { .. }
            | Self::RemovePages { .. }
            | Self::MovePages { .. }
            | Self::InsertPages { .. } => ClustersAfter::ThePagesThemselves,
            Self::MoveTextCluster { .. } | Self::DeleteTextClusters { .. } => {
                ClustersAfter::NotSaid(
                    "it rewrites one run in place, and nothing reads the page back to say which cluster became which",
                )
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClustersAfter {
    TheSame,
    ThePlannerSays,
    ThePagesThemselves,
    NotSaid(&'static str),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextStyle {
    pub size: Option<f64>,
    pub fill: Option<[f64; 3]>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub family: Option<String>,
    pub underline: Option<bool>,
    pub line_spacing: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ParagraphLayout {
    pub alignment: Option<crate::layout::Alignment>,
    pub flow_round: bool,
}

impl TextStyle {
    #[must_use]
    pub const fn changes_face(&self) -> bool {
        self.bold.is_some() || self.italic.is_some() || self.family.is_some()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockRange {
    Between {
        from: (usize, usize),
        to: (usize, usize),
    },
    Units {
        at: (usize, usize),
        backwards: bool,
        count: usize,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlannedCaret {
    Beside {
        cluster: pdf_semantics::ClusterKey,
        after: bool,
    },
    EmptyLine {
        line: usize,
    },
}

pub const LINE_BREAK: char = '\u{2028}';

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RowEnds {
    pub paragraphs: Vec<usize>,
    pub lines: Vec<usize>,
}

impl RowEnds {
    #[must_use]
    pub const fn paragraphs(paragraphs: Vec<usize>) -> Self {
        Self {
            paragraphs,
            lines: Vec::new(),
        }
    }

    #[must_use]
    pub fn lists(&self, row: usize) -> bool {
        self.paragraphs.contains(&row) || self.lines.contains(&row)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Showing {
    #[default]
    Whole,
    PartlyHidden,
    OutOfSight,
}

impl Showing {
    #[must_use]
    pub(crate) fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::OutOfSight, _) | (_, Self::OutOfSight) => Self::OutOfSight,
            (Self::PartlyHidden, _) | (_, Self::PartlyHidden) => Self::PartlyHidden,
            (Self::Whole, Self::Whole) => Self::Whole,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BlockOutcome {
    pub caret: Option<PlannedCaret>,
    pub anchor: Option<PlannedCaret>,
    pub pitch: f64,
    pub edges: (usize, usize),
    pub breaks: RowEnds,
    pub lines: usize,
    pub overflow: bool,
    pub empty: bool,
    pub cropped: bool,
    pub brought_in: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Capability {
    Exact,
    Normalized,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlannedWrite {
    pub reference: Reference,
    pub body: PlannedBody,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlannedBody {
    ReplacedStream {
        decoded: Vec<u8>,
    },
    NewStream {
        dictionary: Vec<u8>,
        decoded: Vec<u8>,
    },
    Direct {
        body: Vec<u8>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PenStep {
    Move((f64, f64)),
    Line((f64, f64)),
    Curve((f64, f64), (f64, f64), (f64, f64)),
}

impl PenStep {
    #[must_use]
    pub fn points(&self) -> Vec<(f64, f64)> {
        match self {
            Self::Move(point) | Self::Line(point) => vec![*point],
            Self::Curve(one, other, end) => vec![*one, *other, *end],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PenStroke {
    pub colour: [f64; 3],
    pub width: f64,
    pub opacity: f64,
    pub blend: PenBlend,
    pub round_ends: bool,
}

impl PenStroke {
    #[must_use]
    pub const fn pen(colour: [f64; 3], width: f64) -> Self {
        Self {
            colour,
            width,
            opacity: 1.0,
            blend: PenBlend::Normal,
            round_ends: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PenBlend {
    Normal,
    Multiply,
}

impl PenBlend {
    pub(crate) const fn name(self) -> &'static [u8] {
        match self {
            Self::Normal => b"/Normal",
            Self::Multiply => b"/Multiply",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PageChange {
    Added(Vec<usize>),
    Removed(Vec<usize>),
    Reordered(Vec<usize>),
}

impl PageChange {
    #[must_use]
    pub fn undone(&self) -> Self {
        match self {
            Self::Added(pages) => Self::Removed(pages.clone()),
            Self::Removed(pages) => Self::Added(pages.clone()),
            Self::Reordered(order) => {
                let mut back = vec![0; order.len()];
                for (before, after) in order.iter().enumerate() {
                    if let Some(slot) = back.get_mut(*after) {
                        *slot = before;
                    }
                }
                Self::Reordered(back)
            }
        }
    }

    #[must_use]
    pub fn renumbered(&self, page: usize) -> Option<usize> {
        match self {
            Self::Added(pages) => Some(
                pages
                    .iter()
                    .fold(page, |at, added| if at >= *added { at + 1 } else { at }),
            ),
            Self::Removed(pages) => {
                if pages.contains(&page) {
                    None
                } else {
                    Some(page - pages.iter().filter(|gone| **gone < page).count())
                }
            }
            Self::Reordered(order) => Some(order.get(page).copied().unwrap_or(page)),
        }
    }

    #[must_use]
    pub fn first(&self) -> usize {
        match self {
            Self::Added(pages) | Self::Removed(pages) => pages.first().copied().unwrap_or(0),
            Self::Reordered(order) => order
                .iter()
                .enumerate()
                .filter(|(before, after)| before != *after)
                .map(|(_, after)| *after)
                .min()
                .unwrap_or(0),
        }
    }
}

#[cfg(test)]
mod page_change_tests {
    use super::PageChange;

    #[test]
    fn a_change_of_pages_and_its_undo_are_inverses() {
        let changes = [
            PageChange::Added(vec![1, 2]),
            PageChange::Added(vec![0, 4]),
            PageChange::Removed(vec![0, 2]),
            PageChange::Reordered(vec![2, 0, 1, 3]),
        ];
        assert_eq!(PageChange::Added(vec![1, 2]).renumbered(1), Some(3));
        assert_eq!(PageChange::Added(vec![0, 4]).renumbered(2), Some(3));
        assert_eq!(PageChange::Removed(vec![0, 2]).renumbered(3), Some(1));
        assert_eq!(PageChange::Removed(vec![0, 2]).renumbered(2), None);
        for change in changes {
            let back = change.undone();
            for page in 0..4 {
                if let Some(after) = change.renumbered(page) {
                    assert_eq!(back.renumbered(after), Some(page), "{change:?} {page}");
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    capability: Capability,
    writes: Vec<PlannedWrite>,
    effect: Effect,
    correspondence:
        Option<std::collections::BTreeMap<pdf_semantics::ClusterKey, pdf_semantics::ClusterKey>>,
    inserted: Vec<(pdf_semantics::ClusterKey, pdf_semantics::ClusterKey)>,
    block: Option<BlockOutcome>,
    pages: Option<PageChange>,
    trailer: crate::incremental::TrailerExtras,
    showing: Showing,
    undo: Option<Vec<PlannedWrite>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MovedRun {
    pub anchor: SourceAnchor,
    pub atom_ordinal: usize,
    pub original_matrix: Matrix,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Effect {
    pub page_index: usize,
    pub moved: Vec<MovedRun>,
    pub target_stream: Reference,
    pub declared_region: Option<[f64; 4]>,
}

impl Effect {
    #[must_use]
    pub fn first(&self) -> &MovedRun {
        self.moved.first().expect("a plan moves at least one run")
    }

    #[must_use]
    pub fn anchor(&self) -> &SourceAnchor {
        &self.first().anchor
    }

    #[must_use]
    pub fn atom_ordinal(&self) -> usize {
        self.first().atom_ordinal
    }

    #[must_use]
    pub fn original_matrix(&self) -> Matrix {
        self.first().original_matrix
    }
}

impl Plan {
    pub fn inverse(&self, source: &ByteStore, credential: &[u8]) -> Result<Self, SpikeError> {
        let mut writes = match &self.undo {
            Some(writes) => writes.clone(),
            None => Vec::with_capacity(self.writes.len()),
        };
        if self.undo.is_none() {
            for write in &self.writes {
                match &write.body {
                    PlannedBody::NewStream { .. } => {}
                    PlannedBody::ReplacedStream { .. } => writes.push(PlannedWrite {
                        reference: write.reference,
                        body: PlannedBody::ReplacedStream {
                            decoded: crate::previous::decoded_stream(
                                source,
                                write.reference,
                                credential,
                            )?,
                        },
                    }),
                    PlannedBody::Direct { .. }
                        if !crate::previous::defined(source, write.reference)? => {}
                    PlannedBody::Direct { .. } => writes.push(PlannedWrite {
                        reference: write.reference,
                        body: PlannedBody::Direct {
                            body: crate::previous::direct_body(
                                source,
                                write.reference,
                                credential,
                            )?,
                        },
                    }),
                }
            }
        }
        if writes.is_empty() {
            return Err(SpikeError::NothingToUndo);
        }
        Ok(Self {
            capability: self.capability,
            writes,
            effect: self.effect.clone(),
            correspondence: None,
            inserted: Vec::new(),
            block: None,
            pages: None,
            undo: None,
            trailer: crate::incremental::TrailerExtras { info: None },
            showing: Showing::Whole,
        })
    }

    pub(crate) const fn new(
        capability: Capability,
        writes: Vec<PlannedWrite>,
        effect: Effect,
    ) -> Self {
        Self {
            capability,
            writes,
            effect,
            correspondence: None,
            inserted: Vec::new(),
            block: None,
            pages: None,
            trailer: crate::incremental::TrailerExtras { info: None },
            undo: None,
            showing: Showing::Whole,
        }
    }

    pub(crate) const fn with_showing(mut self, showing: Showing) -> Self {
        self.showing = showing;
        self
    }

    #[must_use]
    pub const fn showing(&self) -> Showing {
        self.showing
    }

    pub(crate) fn with_undo(mut self, writes: Vec<PlannedWrite>) -> Self {
        self.undo = Some(writes);
        self
    }

    pub(crate) const fn with_trailer(mut self, extras: crate::incremental::TrailerExtras) -> Self {
        self.trailer = extras;
        self
    }

    pub(crate) fn with_pages(mut self, change: PageChange) -> Self {
        self.pages = Some(change);
        self
    }

    #[must_use]
    pub const fn pages(&self) -> Option<&PageChange> {
        self.pages.as_ref()
    }

    pub(crate) fn with_writes(
        mut self,
        writes: Vec<PlannedWrite>,
        target_stream: Reference,
    ) -> Self {
        self.writes = writes;
        self.effect.target_stream = target_stream;
        self
    }

    pub(crate) fn with_block(mut self, block: BlockOutcome) -> Self {
        self.block = Some(block);
        self
    }

    #[must_use]
    pub const fn block(&self) -> Option<&BlockOutcome> {
        self.block.as_ref()
    }

    pub(crate) fn with_correspondence(
        mut self,
        mapping: std::collections::BTreeMap<pdf_semantics::ClusterKey, pdf_semantics::ClusterKey>,
    ) -> Self {
        self.correspondence = Some(mapping);
        self
    }

    #[must_use]
    pub const fn correspondence(
        &self,
    ) -> Option<&std::collections::BTreeMap<pdf_semantics::ClusterKey, pdf_semantics::ClusterKey>>
    {
        self.correspondence.as_ref()
    }

    pub(crate) fn with_inserted(
        mut self,
        inserted: Vec<(pdf_semantics::ClusterKey, pdf_semantics::ClusterKey)>,
    ) -> Self {
        self.inserted = inserted;
        self
    }

    #[must_use]
    pub fn inserted(&self) -> &[(pdf_semantics::ClusterKey, pdf_semantics::ClusterKey)] {
        &self.inserted
    }

    #[must_use]
    pub const fn capability(&self) -> Capability {
        self.capability
    }

    #[must_use]
    pub const fn effect(&self) -> &Effect {
        &self.effect
    }

    #[must_use]
    pub fn writes(&self) -> &[PlannedWrite] {
        &self.writes
    }

    pub fn commit(&self, source: &ByteStore, credential: &[u8]) -> Result<ByteStore, SpikeError> {
        self.commit_bounded(
            source,
            (credential, Restrictions::Respect),
            pdf_syntax::XrefLimits::default(),
        )
    }

    pub(crate) fn commit_bounded(
        &self,
        source: &ByteStore,
        (credential, restrictions): (&[u8], Restrictions),
        limits: pdf_syntax::XrefLimits,
    ) -> Result<ByteStore, SpikeError> {
        let writes: Vec<ObjectWrite<'_>> = self
            .writes
            .iter()
            .map(|write| ObjectWrite {
                reference: write.reference,
                body: match &write.body {
                    PlannedBody::ReplacedStream { decoded } => {
                        ObjectBody::ReplacedStream { decoded }
                    }
                    PlannedBody::NewStream {
                        dictionary,
                        decoded,
                    } => ObjectBody::NewStream {
                        dictionary,
                        decoded,
                    },
                    PlannedBody::Direct { body } => ObjectBody::Direct { body },
                },
            })
            .collect();
        let bytes = append_object_writes_bounded(
            source,
            &writes,
            ProtectionPolicy::Preserve {
                credential,
                restrictions,
            },
            self.trailer,
            limits,
        )?;
        Ok(ByteStore::new(
            pdf_bytes::SourceId::new(source.id().get().wrapping_add(1)),
            bytes,
        ))
    }
}

impl SourceAnchor {
    #[must_use]
    pub fn encode(&self) -> String {
        let mut out = format!(
            "{}:{}:{}",
            self.stream.object_number(),
            self.stream.generation(),
            self.operator_offset
        );
        for (form, offset) in &self.invocation_path {
            let _ = write!(
                out,
                ",{}:{}:{}",
                form.object_number(),
                form.generation(),
                offset
            );
        }
        out
    }

    #[must_use]
    pub fn decode(text: &str) -> Option<Self> {
        let mut parts = text.split(',');
        let (stream, operator_offset) = decode_one(parts.next()?)?;
        let mut invocation_path = Vec::new();
        for part in parts {
            invocation_path.push(decode_one(part)?);
        }
        Some(Self {
            stream,
            operator_offset,
            invocation_path,
        })
    }
}

fn decode_one(text: &str) -> Option<(Reference, usize)> {
    let mut fields = text.split(':');
    let number = fields.next()?.parse::<u32>().ok()?;
    let generation = fields.next()?.parse::<u16>().ok()?;
    let offset = fields.next()?.parse::<usize>().ok()?;
    if fields.next().is_some() {
        return None;
    }
    Some((Reference::new(number, generation), offset))
}

#[cfg(test)]
mod tests {
    use pdf_syntax::Reference;

    use super::SourceAnchor;

    #[test]
    fn an_anchor_survives_the_trip_out_of_the_process_and_back() {
        for anchor in [
            SourceAnchor {
                stream: Reference::new(4, 0),
                operator_offset: 17,
                invocation_path: Vec::new(),
            },
            SourceAnchor {
                stream: Reference::new(5, 2),
                operator_offset: 0,
                invocation_path: vec![(Reference::new(9, 0), 3), (Reference::new(11, 1), 4096)],
            },
        ] {
            let encoded = anchor.encode();
            assert_eq!(SourceAnchor::decode(&encoded), Some(anchor));
        }
    }

    #[test]
    fn a_malformed_anchor_decodes_to_nothing_rather_than_to_something_else() {
        for text in ["", "4", "4:0", "4:0:x", "4:0:1:2", "a:0:1", "4:0:1,"] {
            assert_eq!(SourceAnchor::decode(text), None, "{text}");
        }
    }
}
