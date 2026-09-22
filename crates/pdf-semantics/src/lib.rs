#![forbid(unsafe_code)]

pub mod dependency;
pub mod hit;
pub mod membership;

use std::collections::BTreeMap;
use std::fmt;

use pdf_paint::{Matrix, PaintAtomKind, PaintGraph, Point, TextRenderingMode, TextShowPaint};

pub use dependency::{Dependencies, Dependency, DependencyIndex};
pub use hit::{Candidate, HitDoubt, HitEvidence, candidates, foremost};
pub use membership::{Contested, Dangling, InkKind, MembershipAudit, Orphan};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClusterEvidence {
    Shaped,
    Inferred,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is one independent kind of evidence, counted apart"
)]
pub struct MarkEvidence {
    pub declared_zero_width: bool,
    pub origin_did_not_advance: bool,
    pub continues_a_source_code: bool,
    pub extends_the_cluster_before: bool,
}

fn meaning_of(text: &TextShowPaint, glyph: &pdf_paint::PositionedGlyph) -> Option<String> {
    text.text
        .text_of(pdf_font::Code {
            value: glyph.code.value,
            byte_len: glyph.code.bytes.len(),
        })
        .map(|meaning| meaning.text.clone())
}

fn extends_the_cluster_before(
    text: &TextShowPaint,
    previous: &pdf_paint::PositionedGlyph,
    glyph: &pdf_paint::PositionedGlyph,
) -> bool {
    let (Some(before), Some(this)) = (meaning_of(text, previous), meaning_of(text, glyph)) else {
        return false;
    };
    if before.is_empty() || this.chars().next().is_none_or(|first| first.is_ascii()) {
        return false;
    }
    let joined = format!("{before}{this}");
    !icu_segmenter::GraphemeClusterSegmenter::new()
        .segment_str(&joined)
        .any(|offset| offset == before.len())
}

impl MarkEvidence {
    #[must_use]
    pub const fn joins_previous(self) -> bool {
        self.declared_zero_width
            || self.origin_did_not_advance
            || self.continues_a_source_code
            || self.extends_the_cluster_before
    }

    #[must_use]
    pub const fn both(self) -> bool {
        self.declared_zero_width && self.origin_did_not_advance
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cluster {
    pub atom: usize,
    pub glyphs: std::ops::Range<usize>,
    pub origin: Point,
    pub baseline: Point,
    pub em: f64,
    pub direction: Point,
    pub evidence: ClusterEvidence,
    pub marks: usize,
    pub continuations: usize,
    pub bounds: Option<[f64; 4]>,
    pub layout: Option<[f64; 4]>,
    pub advance: f64,
    pub code: u32,
    pub blank: bool,
}

impl Cluster {
    #[must_use]
    pub const fn is_stacked(&self) -> bool {
        self.marks > 0
    }

    #[must_use]
    pub const fn name(&self) -> ClusterKey {
        ClusterKey {
            atom: self.atom,
            glyph: self.glyphs.start,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClusterReport {
    pub runs_clustered: usize,
    pub runs_without_glyphs: usize,
    pub marks: usize,
    pub marks_by_width: usize,
    pub marks_by_position: usize,
    pub continuations: usize,
    pub marks_reordered_by_paint: usize,
    pub marks_lifted_onto_their_row: usize,
    pub rows_joined_on_one_baseline: usize,
    pub lines_split_by_gap: usize,
    pub blocks_of_one_line: usize,
    pub clusters_outside_the_grouping: usize,
    pub lines_split_by_column: usize,
    pub lines_split_by_edge: usize,
    pub lines_split_by_heading: usize,
    pub lines_split_as_cells: usize,
    pub paths_outside_any_object: usize,
    pub clusters_stranded_by_geometry: usize,
    pub lines_layered_apart: usize,
    pub lines_refused_by_paint_order: usize,
    pub lines_refused_as_covering: usize,
    pub lines_split_by_content: usize,
    pub lines_split_by_text_between: usize,
    pub lines_split_by_delimiters: usize,
    pub drawings_not_grouped: usize,
    pub objects_without_extent: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub clusters: Vec<usize>,
    pub widest_gap: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub lines: Vec<usize>,
    pub evidence: BlockEvidence,
    pub bounds: Option<[f64; 4]>,
    pub layout: Option<[f64; 4]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockEvidence {
    Alone,
    RunsOn,
    Kept,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuadEvidence {
    Placement,
    Ink,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad {
    pub corners: [Point; 4],
    pub evidence: QuadEvidence,
}

impl Quad {
    #[must_use]
    pub fn placed(rect: [f64; 4], matrix: Matrix) -> Self {
        Self {
            corners: [
                matrix.transform(Point {
                    x: rect[0],
                    y: rect[1],
                }),
                matrix.transform(Point {
                    x: rect[2],
                    y: rect[1],
                }),
                matrix.transform(Point {
                    x: rect[2],
                    y: rect[3],
                }),
                matrix.transform(Point {
                    x: rect[0],
                    y: rect[3],
                }),
            ],
            evidence: QuadEvidence::Placement,
        }
    }

    #[must_use]
    pub fn bounds(&self) -> [f64; 4] {
        let mut box_ = [
            self.corners[0].x,
            self.corners[0].y,
            self.corners[0].x,
            self.corners[0].y,
        ];
        for corner in &self.corners[1..] {
            box_ = union(box_, [corner.x, corner.y, corner.x, corner.y]);
        }
        box_
    }

    #[must_use]
    pub fn center(&self) -> Point {
        Point {
            x: self.corners.iter().map(|corner| corner.x).sum::<f64>() / 4.0,
            y: self.corners.iter().map(|corner| corner.y).sum::<f64>() / 4.0,
        }
    }

    #[must_use]
    pub fn contains(&self, point: Point) -> bool {
        let mut positive = false;
        let mut negative = false;
        for index in 0..4 {
            let from = self.corners[index];
            let to = self.corners[(index + 1) % 4];
            let side =
                (to.x - from.x).mul_add(point.y - from.y, -((to.y - from.y) * (point.x - from.x)));
            positive |= side > 0.0;
            negative |= side < 0.0;
        }
        if positive && negative {
            return false;
        }
        if positive || negative {
            return true;
        }
        let box_ = self.bounds();
        (box_[0]..=box_[2]).contains(&point.x) && (box_[1]..=box_[3]).contains(&point.y)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectKind {
    Text(usize),
    Image,
    Form,
    Shading,
    TextRun,
    Path,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Member {
    pub atom: usize,
    pub glyphs: Option<std::ops::Range<usize>>,
}

impl Member {
    #[must_use]
    pub const fn whole(atom: usize) -> Self {
        Self { atom, glyphs: None }
    }

    #[must_use]
    pub const fn glyphs(atom: usize, glyphs: std::ops::Range<usize>) -> Self {
        Self {
            atom,
            glyphs: Some(glyphs),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    pub kind: ObjectKind,
    pub members: Vec<Member>,
    pub quad: Option<Quad>,
    pub bounds: Option<[f64; 4]>,
}

impl Object {
    pub fn atoms(&self) -> impl Iterator<Item = usize> + '_ {
        let mut last = None;
        self.members.iter().filter_map(move |member| {
            if last == Some(member.atom) {
                None
            } else {
                last = Some(member.atom);
                Some(member.atom)
            }
        })
    }

    #[must_use]
    pub fn first_atom(&self) -> Option<usize> {
        self.members.iter().map(|member| member.atom).min()
    }
}

#[must_use]
pub fn placed_quad(kind: &PaintAtomKind) -> Option<Quad> {
    match kind {
        PaintAtomKind::Image(image) => {
            Some(Quad::placed([0.0, 0.0, 1.0, 1.0], image.state.ctm.value))
        }
        PaintAtomKind::TransparencyGroup(group) => Some(Quad::placed(
            group.bbox,
            group.state.ctm.value.multiply(group.matrix.value),
        )),
        PaintAtomKind::Shading(shading) => shading
            .bbox
            .as_ref()
            .map(|bbox| Quad::placed(bbox.value, shading.state.ctm.value)),
        PaintAtomKind::Text(_) | PaintAtomKind::Path(_) => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Caret {
    pub line: usize,
    pub offset: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectionSpan {
    pub atom: usize,
    pub glyphs: std::ops::Range<usize>,
    pub clusters: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionError {
    Empty,
    CrossesRuns(usize),
    NotContiguous,
}

impl fmt::Display for SelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(formatter, "nothing is selected there"),
            Self::CrossesRuns(runs) => write!(
                formatter,
                "the selection is painted by {runs} separate text runs, and one edit acts inside one"
            ),
            Self::NotContiguous => write!(
                formatter,
                "the selected clusters are not consecutive glyphs of the run that paints them"
            ),
        }
    }
}

impl std::error::Error for SelectionError {}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ClusterKey {
    pub atom: usize,
    pub glyph: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Grouping {
    blocks: Vec<Vec<ClusterKey>>,
}

impl Grouping {
    #[must_use]
    pub fn remapped(&self, mapping: &BTreeMap<ClusterKey, ClusterKey>) -> Self {
        self.remapped_with_insertions(mapping, &[])
    }

    #[must_use]
    pub fn remapped_with_insertions(
        &self,
        mapping: &BTreeMap<ClusterKey, ClusterKey>,
        inserted: &[(ClusterKey, ClusterKey)],
    ) -> Self {
        let owners: Vec<Option<usize>> = inserted
            .iter()
            .map(|(site, _)| self.block_holding(*site))
            .collect();
        Self {
            blocks: self
                .blocks
                .iter()
                .enumerate()
                .map(|(index, block)| {
                    let mut keys: Vec<_> = block
                        .iter()
                        .filter_map(|key| mapping.get(key).copied())
                        .chain(
                            inserted
                                .iter()
                                .zip(&owners)
                                .filter(|(_, owner)| **owner == Some(index))
                                .map(|((_, key), _)| *key),
                        )
                        .collect();
                    keys.sort_unstable();
                    keys.dedup();
                    keys
                })
                .collect(),
        }
    }

    fn block_holding(&self, site: ClusterKey) -> Option<usize> {
        self.blocks
            .iter()
            .enumerate()
            .flat_map(|(index, block)| block.iter().map(move |key| (*key, index)))
            .filter(|(key, _)| key.atom == site.atom && key.glyph <= site.glyph)
            .max_by_key(|(key, _)| key.glyph)
            .map(|(_, index)| index)
    }

    #[must_use]
    pub fn of(index: &SemanticIndex) -> Self {
        let blocks = index
            .blocks
            .iter()
            .map(|block| {
                block
                    .lines
                    .iter()
                    .filter_map(|line| index.lines.get(*line))
                    .flat_map(|line| line.clusters.iter())
                    .filter_map(|cluster| Some(index.clusters.get(*cluster)?.name()))
                    .collect()
            })
            .collect();
        Self { blocks }
    }

    #[must_use]
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    #[must_use]
    pub fn block(&self, block: usize) -> Option<&[ClusterKey]> {
        self.blocks.get(block).map(Vec::as_slice)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SemanticIndex {
    pub clusters: Vec<Cluster>,
    pub lines: Vec<Line>,
    pub blocks: Vec<Block>,
    pub objects: Vec<Object>,
    pub report: ClusterReport,
    faces: Vec<u32>,
    scopes: Vec<u32>,
    delimiters: Vec<u8>,
}

const OPENS_A_DELIMITER: u8 = 1;

const CLOSES_A_DELIMITER: u8 = 2;

fn delimiter_pieces(text: &str) -> u8 {
    text.chars().fold(0, |pieces, character| {
        pieces
            | match character {
                '\u{239B}' | '\u{239E}' | '\u{23A1}' | '\u{23A4}' | '\u{23A7}' | '\u{23AB}' => {
                    OPENS_A_DELIMITER
                }
                '\u{239D}' | '\u{23A0}' | '\u{23A3}' | '\u{23A6}' | '\u{23A9}' | '\u{23AD}' => {
                    CLOSES_A_DELIMITER
                }
                _ => 0,
            }
    })
}

const ADVANCE_EPSILON: f64 = 0.001;

const BASELINE_TOLERANCE_EM: f64 = 0.25;

const MARK_LIFT_EM: f64 = 1.0;

const COLUMN_GAP_EM: f64 = 2.5;

const SAME_PEN_EM: f64 = 0.02;

const DIRECTION_TOLERANCE: f64 = 0.999_998;

const BLOCK_LEADING_EM: f64 = 2.0;

const BLOCK_MINIMUM_STEP_EM: f64 = 0.1;

const BLOCK_SIZE_TOLERANCE: f64 = 0.8;

const BLOCK_EDGE_EM: f64 = 4.0;

const BLOCK_OVERLAP: f64 = 0.5;

const COVERED_LETTER: f64 = 0.15;

const COLLISION_OVERLAP: f64 = 0.5;

const FAKE_BOLD_DISTANCE_EM: f64 = 0.1;

const FAKE_BOLD_ATOM_WINDOW: usize = 5;

const SUPERSCRIPT_SIZE: f64 = 0.9;

const PITCH_TOLERANCE: f64 = 0.25;

const HEADING_SHORTER_EM: f64 = 2.0;

const CELL_FILLS: f64 = 0.75;

const ROW_MATE_EM: f64 = 0.25;

fn face_name(text: &TextShowPaint) -> (Vec<u8>, bool) {
    let name = text.font_request.as_ref().map_or_else(
        || {
            text.state.text.font.as_ref().map_or_else(Vec::new, |font| {
                let mut name = font.value.name.clone();
                name.extend_from_slice(format!("{:?}", font.value.reference).as_bytes());
                name
            })
        },
        |request| without_subset_tag(&request.base_font).to_vec(),
    );
    let stroked = matches!(
        text.state.text.rendering_mode.value,
        TextRenderingMode::FillStroke | TextRenderingMode::FillStrokeClip
    );
    (name, stroked)
}

fn without_subset_tag(base: &[u8]) -> &[u8] {
    match base.get(..7) {
        Some([tag @ .., b'+']) if tag.iter().all(u8::is_ascii_uppercase) => &base[7..],
        _ => base,
    }
}

fn dot(one: Point, other: Point) -> f64 {
    one.x.mul_add(other.x, one.y * other.y)
}

fn union(one: [f64; 4], other: [f64; 4]) -> [f64; 4] {
    [
        one[0].min(other[0]),
        one[1].min(other[1]),
        one[2].max(other[2]),
        one[3].max(other[3]),
    ]
}

fn corners_of(bounds: [f64; 4]) -> [Point; 4] {
    [
        Point {
            x: bounds[0],
            y: bounds[1],
        },
        Point {
            x: bounds[2],
            y: bounds[1],
        },
        Point {
            x: bounds[2],
            y: bounds[3],
        },
        Point {
            x: bounds[0],
            y: bounds[3],
        },
    ]
}

fn overlap(one: (f64, f64), other: (f64, f64)) -> f64 {
    let shared = one.1.min(other.1) - one.0.max(other.0);
    if shared < 0.0 {
        return 0.0;
    }
    let narrower = (one.1 - one.0).min(other.1 - other.0);
    if narrower <= 0.0 {
        return if shared >= 0.0 { 1.0 } else { 0.0 };
    }
    shared / narrower
}

fn reading_order(a: &Cluster, b: &Cluster) -> std::cmp::Ordering {
    let key = |cluster: &Cluster| {
        let Point {
            x: along_x,
            y: along_y,
        } = cluster.direction;
        let upright = along_y.abs() <= UPRIGHT_DIRECTION && along_x > 0.0;
        if upright {
            return (0_i64, cluster.baseline.y, cluster.baseline.x);
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "an angle in radians over a small step, far inside i64"
        )]
        let bucket = (along_y.atan2(along_x) / DIRECTION_BUCKET).round() as i64;
        let up = cluster
            .baseline
            .y
            .mul_add(along_x, -(cluster.baseline.x * along_y));
        let along = cluster
            .baseline
            .x
            .mul_add(along_x, cluster.baseline.y * along_y);
        (1 + bucket.rem_euclid(ANGLE_BUCKETS), up, along)
    };
    let ((group_a, up_a, along_a), (group_b, up_b, along_b)) = (key(a), key(b));
    group_a
        .cmp(&group_b)
        .then_with(|| up_b.total_cmp(&up_a))
        .then_with(|| along_a.total_cmp(&along_b))
}

const UPRIGHT_DIRECTION: f64 = 1e-9;

const DIRECTION_BUCKET: f64 = 1e-4;

const ANGLE_BUCKETS: i64 = 62_832;

const WINDOW_SLACK: f64 = 1e-3;

struct AcrossBaselines {
    sorted: Vec<(f64, usize)>,
    widest_em: f64,
    direction: Point,
}

impl AcrossBaselines {
    fn of(clusters: &[Cluster], members: &[usize]) -> Option<Self> {
        let first = clusters.get(*members.first()?)?;
        let direction = first.direction;
        if !direction.x.is_finite() || !direction.y.is_finite() {
            return None;
        }
        let mut sorted: Vec<(f64, usize)> = Vec::with_capacity(members.len());
        let mut widest_em = 0.0_f64;
        for member in members {
            let cluster = clusters.get(*member)?;
            if cluster.direction != direction
                || !cluster.baseline.x.is_finite()
                || !cluster.baseline.y.is_finite()
                || !cluster.em.is_finite()
            {
                return None;
            }
            widest_em = widest_em.max(cluster.em.abs());
            sorted.push((
                cluster
                    .baseline
                    .y
                    .mul_add(direction.x, -(cluster.baseline.x * direction.y)),
                *member,
            ));
        }
        sorted.sort_by(|one, other| one.0.total_cmp(&other.0));
        Some(Self {
            sorted,
            widest_em,
            direction,
        })
    }

    fn near(&self, seed: &Cluster, ems: f64) -> impl Iterator<Item = usize> + '_ {
        let at = seed
            .baseline
            .y
            .mul_add(self.direction.x, -(seed.baseline.x * self.direction.y));
        let window = ems.mul_add(self.widest_em.max(seed.em.abs()), WINDOW_SLACK);
        let from = self
            .sorted
            .partition_point(|(across, _)| *across < at - window);
        let to = self
            .sorted
            .partition_point(|(across, _)| *across <= at + window);
        self.sorted[from..to].iter().map(|(_, member)| *member)
    }
}

struct AcrossRows {
    from: usize,
    sorted: Vec<(f64, usize)>,
    by_band: Vec<(f64, f64, usize)>,
    band: Vec<(f64, f64)>,
    widest_band: f64,
    widest_em: f64,
    direction: Point,
}

impl AcrossRows {
    fn of(index: &SemanticIndex, from: usize) -> Option<Self> {
        let direction = index.line_seed(from)?.direction;
        if !direction.x.is_finite() || !direction.y.is_finite() {
            return None;
        }
        let across = |point: Point| point.y.mul_add(direction.x, -(point.x * direction.y));
        let count = index.lines.len().checked_sub(from)?;
        let mut sorted = Vec::with_capacity(count);
        let mut band = Vec::with_capacity(count);
        let mut widest_band = 0.0_f64;
        let mut widest_em = 0.0_f64;
        for line in from..index.lines.len() {
            let seed = index.line_seed(line)?;
            if seed.direction != direction
                || !seed.baseline.x.is_finite()
                || !seed.baseline.y.is_finite()
                || !seed.em.is_finite()
            {
                return None;
            }
            widest_em = widest_em.max(seed.em.abs());
            sorted.push((across(seed.baseline), line));
            let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
            for cluster in &index.lines[line].clusters {
                let cluster = index.clusters.get(*cluster)?;
                if !cluster.baseline.x.is_finite()
                    || !cluster.baseline.y.is_finite()
                    || !cluster.em.is_finite()
                {
                    return None;
                }
                let reach = 0.7 * cluster.em.abs();
                let at = across(cluster.baseline);
                low = low.min(at - reach);
                high = high.max(at + reach);
                if let Some(bounds) = cluster.bounds {
                    if !bounds.iter().all(|edge| edge.is_finite()) {
                        return None;
                    }
                    for corner in [
                        Point {
                            x: bounds[0],
                            y: bounds[1],
                        },
                        Point {
                            x: bounds[0],
                            y: bounds[3],
                        },
                        Point {
                            x: bounds[2],
                            y: bounds[1],
                        },
                        Point {
                            x: bounds[2],
                            y: bounds[3],
                        },
                    ] {
                        let at = across(corner);
                        low = low.min(at);
                        high = high.max(at);
                    }
                }
            }
            if !low.is_finite() || !high.is_finite() {
                return None;
            }
            widest_band = widest_band.max(high - low);
            band.push((low, high));
        }
        sorted.sort_by(|one, other| one.0.total_cmp(&other.0));
        let mut by_band: Vec<(f64, f64, usize)> = band
            .iter()
            .enumerate()
            .map(|(offset, (low, high))| (*low, *high, from + offset))
            .collect();
        by_band.sort_by(|one, other| one.0.total_cmp(&other.0));
        Some(Self {
            from,
            sorted,
            by_band,
            band,
            widest_band,
            widest_em,
            direction,
        })
    }

    fn near(&self, seed: &Cluster, ems: f64) -> impl Iterator<Item = usize> + '_ {
        let at = seed
            .baseline
            .y
            .mul_add(self.direction.x, -(seed.baseline.x * self.direction.y));
        let window = ems.mul_add(self.widest_em.max(seed.em.abs()), WINDOW_SLACK);
        let from = self
            .sorted
            .partition_point(|(across, _)| *across < at - window);
        let to = self
            .sorted
            .partition_point(|(across, _)| *across <= at + window);
        self.sorted[from..to].iter().map(|(_, line)| *line)
    }

    fn overlapping(&self, line: usize) -> impl Iterator<Item = usize> + '_ {
        let (low, high) = self
            .band
            .get(line.wrapping_sub(self.from))
            .copied()
            .unwrap_or((f64::NEG_INFINITY, f64::INFINITY));
        let start = self
            .by_band
            .partition_point(|(at, _, _)| *at < low - self.widest_band - WINDOW_SLACK);
        let end = self
            .by_band
            .partition_point(|(at, _, _)| *at <= high + WINDOW_SLACK);
        self.by_band[start..end]
            .iter()
            .filter(move |(_, reaches, _)| *reaches >= low - WINDOW_SLACK)
            .map(|(_, _, line)| *line)
    }
}

struct EdgeRun {
    em: f64,
    unmeasured: bool,
    spread: [(f64, f64); 3],
}

impl Default for EdgeRun {
    fn default() -> Self {
        Self {
            em: 0.0,
            unmeasured: false,
            spread: [(f64::INFINITY, f64::NEG_INFINITY); 3],
        }
    }
}

impl EdgeRun {
    fn take(&mut self, index: &SemanticIndex, line: usize) {
        if let Some(seed) = index.line_seed(line) {
            self.em = self.em.max(seed.em);
        }
        let Some((start, end)) = index.line_reach(line) else {
            self.unmeasured = true;
            return;
        };
        for (slot, at) in [start, end, f64::midpoint(start, end)]
            .into_iter()
            .enumerate()
        {
            self.spread[slot].0 = self.spread[slot].0.min(at);
            self.spread[slot].1 = self.spread[slot].1.max(at);
        }
    }

    fn set_to_an_edge(&self) -> bool {
        if self.em <= 0.0 || self.unmeasured {
            return true;
        }
        self.spread
            .iter()
            .map(|(low, high)| high - low)
            .fold(f64::INFINITY, f64::min)
            <= BLOCK_EDGE_EM * self.em
    }
}

impl SemanticIndex {
    #[must_use]
    pub fn of(graph: &PaintGraph) -> Self {
        let mut index = Self::clusters_of(graph);
        let every: Vec<usize> = (0..index.clusters.len()).collect();
        index.group_lines_over(&every);
        index.group_blocks_over(0);
        index.join_rows_on_one_baseline();
        index.split_blocks_not_set_to_an_edge();
        let grouping = Grouping::of(&index);
        let report = index.report.clone();
        let evidence: Vec<BlockEvidence> =
            index.blocks.iter().map(|block| block.evidence).collect();
        index.lines.clear();
        index.blocks.clear();
        index.apply(&grouping);
        index.join_rows_on_one_baseline();
        for (block, found) in index.blocks.iter_mut().zip(evidence) {
            block.evidence = found;
        }
        index.report = report;
        index.renumber_lines_in_reading_order();
        index.index_objects(graph);
        index
    }

    fn renumber_lines_in_reading_order(&mut self) {
        let mut order: Vec<usize> = (0..self.lines.len()).collect();
        order.sort_by(
            |one, other| match (self.line_seed(*one), self.line_seed(*other)) {
                (Some(a), Some(b)) => reading_order(a, b),
                (None, None) => one.cmp(other),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
            },
        );
        let mut position = vec![0; order.len()];
        for (new, old) in order.iter().enumerate() {
            position[*old] = new;
        }
        let mut taken: Vec<Option<Line>> = std::mem::take(&mut self.lines)
            .into_iter()
            .map(Some)
            .collect();
        self.lines = order
            .iter()
            .map(|old| taken[*old].take().expect("each row is placed once"))
            .collect();
        for block in &mut self.blocks {
            for line in &mut block.lines {
                *line = position[*line];
            }
            block.lines.sort_unstable();
        }
    }

    #[must_use]
    pub fn of_grouped(graph: &PaintGraph, grouping: &Grouping) -> Self {
        let mut index = Self::clusters_of(graph);
        index.apply(grouping);
        index.join_rows_on_one_baseline();
        index.index_objects(graph);
        index
    }

    fn clusters_of(graph: &PaintGraph) -> Self {
        let mut index = Self::default();
        let mut named: BTreeMap<(Vec<u8>, bool), u32> = BTreeMap::new();
        let mut contents: Vec<(
            &[pdf_paint::FormInvocation],
            &[pdf_paint::PatternInvocation],
        )> = Vec::new();
        for (position, atom) in graph.atoms.iter().enumerate() {
            let PaintAtomKind::Text(text) = &atom.kind else {
                continue;
            };
            let before = index.clusters.len();
            index.extend_from_run(position, text);
            if index.clusters.len() == before {
                index.report.runs_without_glyphs += 1;
            } else {
                index.report.runs_clustered += 1;
            }
            let next = u32::try_from(named.len()).unwrap_or(u32::MAX);
            let face = *named.entry(face_name(text)).or_insert(next);
            index.faces.resize(index.clusters.len(), face);
            let content = (&atom.id.invocation_path[..], &atom.id.pattern_path[..]);
            let scope = contents
                .iter()
                .position(|known| *known == content)
                .unwrap_or_else(|| {
                    contents.push(content);
                    contents.len() - 1
                });
            index.scopes.resize(
                index.clusters.len(),
                u32::try_from(scope).unwrap_or(u32::MAX),
            );
            for cluster in before..index.clusters.len() {
                let glyphs = index.clusters[cluster].glyphs.clone();
                let pieces = text.glyphs.get(glyphs).map_or(0, |glyphs| {
                    glyphs.iter().fold(0, |pieces, glyph| {
                        pieces | meaning_of(text, glyph).map_or(0, |read| delimiter_pieces(&read))
                    })
                });
                index.delimiters.push(pieces);
            }
        }
        index
    }

    fn apply(&mut self, grouping: &Grouping) {
        let mut where_it_is: BTreeMap<ClusterKey, usize> = BTreeMap::new();
        for (position, cluster) in self.clusters.iter().enumerate() {
            where_it_is.insert(cluster.name(), position);
        }
        let mut placed = vec![false; self.clusters.len()];
        for block in &grouping.blocks {
            let mut members = Vec::new();
            for key in block {
                let Some(&cluster) = where_it_is.get(key) else {
                    continue;
                };
                if placed[cluster] {
                    continue;
                }
                placed[cluster] = true;
                members.push(cluster);
            }
            let first = self.lines.len();
            self.group_lines_over(&members);
            let lines: Vec<usize> = (first..self.lines.len()).collect();
            let bounds = lines
                .iter()
                .filter_map(|line| self.line_bounds(*line))
                .reduce(union);
            let layout = lines
                .iter()
                .filter_map(|line| self.line_layout(*line))
                .reduce(union);
            self.blocks.push(Block {
                lines,
                evidence: BlockEvidence::Kept,
                bounds,
                layout,
            });
        }
        let strangers: Vec<usize> = (0..self.clusters.len())
            .filter(|cluster| !placed[*cluster])
            .collect();
        self.report.clusters_outside_the_grouping = strangers.len();
        if !strangers.is_empty() {
            let first_line = self.lines.len();
            self.group_lines_over(&strangers);
            self.group_blocks_over(first_line);
        }
    }

    fn index_objects(&mut self, graph: &PaintGraph) {
        let mut objects: Vec<(usize, Object)> = Vec::new();
        for (block_index, block) in self.blocks.iter().enumerate() {
            let members = self.members_of_block(block);
            let first = members
                .iter()
                .map(|member| member.atom)
                .min()
                .unwrap_or(usize::MAX);
            objects.push((
                first,
                Object {
                    kind: ObjectKind::Text(block_index),
                    members,
                    quad: self.text_quad(graph, block),
                    bounds: block.bounds,
                },
            ));
        }
        let claimed: std::collections::BTreeSet<usize> = objects
            .iter()
            .flat_map(|(_, object)| object.members.iter().map(|member| member.atom))
            .collect();
        let drawing_of = Self::drawings_of(graph);
        let mut drawn: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (position, atom) in graph.atoms.iter().enumerate() {
            let kind = match &atom.kind {
                PaintAtomKind::Text(_) if claimed.contains(&position) => continue,
                PaintAtomKind::Text(_) => ObjectKind::TextRun,
                PaintAtomKind::Path(_) => {
                    if let Some(drawing) = drawing_of.get(&position) {
                        drawn.entry(*drawing).or_default().push(position);
                    } else {
                        self.report.drawings_not_grouped += 1;
                    }
                    continue;
                }
                PaintAtomKind::Image(_) => ObjectKind::Image,
                PaintAtomKind::TransparencyGroup(_) => ObjectKind::Form,
                PaintAtomKind::Shading(_) => ObjectKind::Shading,
            };
            let quad = placed_quad(&atom.kind);
            objects.push((
                position,
                Object {
                    kind,
                    members: vec![Member::whole(position)],
                    quad,
                    bounds: atom.kind.user_bounds(),
                },
            ));
        }
        for (_, atoms) in drawn {
            let Some(first) = atoms.first().copied() else {
                continue;
            };
            let bounds = atoms
                .iter()
                .filter_map(|atom| graph.atoms.get(*atom)?.kind.user_bounds())
                .reduce(union);
            objects.push((
                first,
                Object {
                    kind: ObjectKind::Path,
                    members: atoms.into_iter().map(Member::whole).collect(),
                    quad: None,
                    bounds,
                },
            ));
        }
        objects.sort_by_key(|(first, _)| *first);
        self.report.objects_without_extent = objects
            .iter()
            .filter(|(_, object)| object.quad.is_none())
            .count();
        self.objects = objects.into_iter().map(|(_, object)| object).collect();
    }

    fn drawings_of(graph: &PaintGraph) -> BTreeMap<usize, usize> {
        graph
            .atoms
            .iter()
            .enumerate()
            .filter(|(_, atom)| matches!(atom.kind, PaintAtomKind::Path(_)))
            .filter(|(_, atom)| atom.kind.user_bounds().is_some())
            .map(|(position, _)| (position, position))
            .collect()
    }

    fn members_of_block(&self, block: &Block) -> Vec<Member> {
        let mut spans: BTreeMap<usize, Vec<std::ops::Range<usize>>> = BTreeMap::new();
        for cluster in self.clusters_of_block(block) {
            spans
                .entry(cluster.atom)
                .or_default()
                .push(cluster.glyphs.clone());
        }
        let mut members = Vec::new();
        for (atom, mut ranges) in spans {
            ranges.sort_by_key(|range| (range.start, range.end));
            let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
            for range in ranges {
                match merged.last_mut() {
                    Some(last) if range.start <= last.end => {
                        last.end = last.end.max(range.end);
                    }
                    _ => merged.push(range),
                }
            }
            members.extend(merged.into_iter().map(|range| Member::glyphs(atom, range)));
        }
        members
    }

    fn clusters_of_block<'a>(&'a self, block: &'a Block) -> impl Iterator<Item = &'a Cluster> {
        block
            .lines
            .iter()
            .filter_map(|line| self.lines.get(*line))
            .flat_map(|line| line.clusters.iter())
            .filter_map(|cluster| self.clusters.get(*cluster))
    }

    fn text_quad(&self, graph: &PaintGraph, block: &Block) -> Option<Quad> {
        let direction = self.clusters_of_block(block).next()?.direction;
        let along_an_axis = direction.x.abs().max(direction.y.abs()) >= DIRECTION_TOLERANCE;
        let mut points: Vec<Point> = Vec::new();
        for cluster in self.clusters_of_block(block) {
            if along_an_axis {
                if let Some(bounds) = cluster.bounds {
                    points.extend(corners_of(bounds));
                }
            } else if let Some(PaintAtomKind::Text(text)) =
                graph.atoms.get(cluster.atom).map(|atom| &atom.kind)
                && let Some(outline) = text.outline_points_in(cluster.glyphs.clone())
            {
                points.extend(outline);
            }
        }
        let across = Point {
            x: -direction.y,
            y: direction.x,
        };
        let (mut along_low, mut along_high) = (f64::INFINITY, f64::NEG_INFINITY);
        let (mut across_low, mut across_high) = (f64::INFINITY, f64::NEG_INFINITY);
        for point in &points {
            let along = dot(*point, direction);
            let sideways = dot(*point, across);
            along_low = along_low.min(along);
            along_high = along_high.max(along);
            across_low = across_low.min(sideways);
            across_high = across_high.max(sideways);
        }
        if points.is_empty() {
            return None;
        }
        let corner = |along: f64, sideways: f64| Point {
            x: direction.x.mul_add(along, across.x * sideways),
            y: direction.y.mul_add(along, across.y * sideways),
        };
        Some(Quad {
            corners: [
                corner(along_low, across_low),
                corner(along_high, across_low),
                corner(along_high, across_high),
                corner(along_low, across_high),
            ],
            evidence: QuadEvidence::Ink,
        })
    }

    #[must_use]
    pub fn line_bounds(&self, line: usize) -> Option<[f64; 4]> {
        let line = self.lines.get(line)?;
        line.clusters
            .iter()
            .filter_map(|index| self.clusters.get(*index)?.bounds)
            .reduce(union)
    }

    #[must_use]
    pub fn line_layout(&self, line: usize) -> Option<[f64; 4]> {
        let line = self.lines.get(line)?;
        line.clusters
            .iter()
            .filter_map(|index| {
                let cluster = self.clusters.get(*index)?;
                cluster.layout.or_else(|| {
                    let ink = cluster.bounds?;
                    let end = Point {
                        x: cluster.baseline.x + cluster.direction.x * cluster.advance,
                        y: cluster.baseline.y + cluster.direction.y * cluster.advance,
                    };
                    let pen = [
                        cluster.baseline.x.min(end.x),
                        cluster.baseline.y.min(end.y),
                        cluster.baseline.x.max(end.x),
                        cluster.baseline.y.max(end.y),
                    ];
                    Some(union(ink, pen))
                })
            })
            .reduce(union)
    }

    fn group_blocks_over(&mut self, from_line: usize) {
        let mut order: Vec<usize> = (from_line..self.lines.len()).collect();
        order.sort_by(|one, other| {
            let (a, b) = (self.line_seed(*one), self.line_seed(*other));
            match (a, b) {
                (Some(a), Some(b)) => reading_order(a, b),
                (None, None) => one.cmp(other),
                (None, _) => std::cmp::Ordering::Greater,
                (_, None) => std::cmp::Ordering::Less,
            }
        });

        let mut taken = vec![true; self.lines.len()];
        for claimed in taken.iter_mut().skip(from_line) {
            *claimed = false;
        }
        let mut line_of = vec![usize::MAX; self.clusters.len()];
        for (position, line) in self.lines.iter().enumerate() {
            for cluster in &line.clusters {
                if let Some(slot) = line_of.get_mut(*cluster) {
                    *slot = position;
                }
            }
        }
        let slots = self.row_slots();
        let across = AcrossRows::of(self, from_line);
        let mut held = vec![false; self.lines.len()];
        let mut open: Vec<usize> = Vec::new();
        for seed in order {
            if taken[seed] {
                continue;
            }
            taken[seed] = true;
            held[seed] = true;
            open.push(seed);
            let mut refused = std::collections::BTreeSet::new();
            loop {
                let last = *open.last().expect("the block has its seed");
                let mut candidates: Vec<(usize, f64)> = Vec::new();
                for line in self.rows_within_reach(last, &taken, across.as_ref()) {
                    if !self.lines_run_on(last, line) {
                        continue;
                    }
                    if self.sets_off_a_heading(last, line) {
                        self.report.lines_split_by_heading += 1;
                        continue;
                    }
                    if Self::are_cells(last, line, &slots) {
                        self.report.lines_split_as_cells += 1;
                        continue;
                    }
                    if self.passes_over_another_blocks_row(last, line, &open, &taken) {
                        self.report.lines_split_by_text_between += 1;
                        continue;
                    }
                    if self.closes_a_delimiter_over_one_that_opens(last, line) {
                        self.report.lines_split_by_delimiters += 1;
                        continue;
                    }
                    if self.a_row_of_the_block_covers(line, &open, &held, across.as_ref()) {
                        self.report.lines_refused_as_covering += 1;
                        continue;
                    }
                    candidates.push((line, self.step_between(last, line).unwrap_or(f64::INFINITY)));
                }
                let mut best: Option<(usize, f64)> = None;
                for &(line, step) in &candidates {
                    if self.position_is_ambiguous(&open, line, step, &candidates)
                        && !self.painted_on_from(last, line, &open, &line_of)
                    {
                        refused.insert(line);
                        continue;
                    }
                    if best.is_none_or(|(_, shortest)| step < shortest) {
                        best = Some((line, step));
                    }
                }
                let Some((next, _)) = best else { break };
                refused.remove(&next);
                taken[next] = true;
                held[next] = true;
                open.push(next);
            }
            self.report.lines_refused_by_paint_order += refused.len();
            let last = *open.last().expect("the block has its seed");
            for line in self.rows_within_reach(last, &taken, across.as_ref()) {
                if self.only_the_columns_disagree(last, line) {
                    self.report.lines_split_by_column += 1;
                }
            }
            for line in &open {
                held[*line] = false;
            }
            self.close_block(&mut open);
        }
    }

    fn rows_within_reach(
        &self,
        last: usize,
        taken: &[bool],
        across: Option<&AcrossRows>,
    ) -> Vec<usize> {
        let Some(across) = across else {
            return (0..taken.len()).filter(|line| !taken[*line]).collect();
        };
        let Some(seed) = self.line_seed(last) else {
            return Vec::new();
        };
        let mut near: Vec<usize> = across
            .near(seed, BLOCK_LEADING_EM)
            .filter(|line| !taken[*line])
            .collect();
        near.sort_unstable();
        near
    }

    fn a_row_of_the_block_covers(
        &self,
        line: usize,
        open: &[usize],
        held: &[bool],
        across: Option<&AcrossRows>,
    ) -> bool {
        match across {
            Some(across) => across
                .overlapping(line)
                .any(|other| held[other] && self.rows_cover(other, line)),
            None => open.iter().any(|other| self.rows_cover(*other, line)),
        }
    }

    fn position_is_ambiguous(
        &self,
        open: &[usize],
        line: usize,
        step: f64,
        candidates: &[(usize, f64)],
    ) -> bool {
        let last = *open.last().expect("a block has a line");
        let (Some(above), Some(below)) = (self.line_seed(last), self.line_seed(line)) else {
            return true;
        };
        let em = above.em.max(below.em);
        if step < em || self.lies_below(last, line) != Some(true) {
            return true;
        }
        if candidates
            .iter()
            .any(|&(other, other_step)| other != line && (other_step - step).abs() < em)
        {
            return true;
        }
        if let [first, second, ..] = open
            && let Some(pitch) = self.step_between(*first, *second)
            && pitch > 0.0
        {
            let multiple = step / pitch;
            if multiple.round() < 1.0 || (multiple - multiple.round()).abs() > PITCH_TOLERANCE {
                return true;
            }
        }
        false
    }

    fn rows_cover(&self, one: usize, other: usize) -> bool {
        let Some(&seed) = self.lines[one].clusters.first() else {
            return false;
        };
        let area = |b: [f64; 4]| (b[2] - b[0]) * (b[3] - b[1]);
        self.lines[one].clusters.iter().any(|&first| {
            self.lines[other].clusters.iter().any(|&second| {
                let Some((a, b)) = self.letter_boxes(seed, first, second) else {
                    return false;
                };
                let width = a[2].min(b[2]) - a[0].max(b[0]);
                let height = a[3].min(b[3]) - a[1].max(b[1]);
                let smaller = area(a).min(area(b));
                width > 0.0
                    && height > 0.0
                    && smaller > 0.0
                    && width * height >= COVERED_LETTER * smaller
                    && !self.repeats(first.min(second), first.max(second))
            })
        })
    }

    fn same_scope(&self, one: usize, other: usize) -> bool {
        self.scopes.get(one) == self.scopes.get(other)
    }

    fn letter_boxes(&self, seed: usize, one: usize, other: usize) -> Option<([f64; 4], [f64; 4])> {
        let from = &self.clusters[seed];
        let (a, b) = (&self.clusters[one], &self.clusters[other]);
        if a.blank || b.blank || a.advance <= 0.0 || b.advance <= 0.0 {
            return None;
        }
        let upright = from.direction.x.abs() >= DIRECTION_TOLERANCE
            || from.direction.y.abs() >= DIRECTION_TOLERANCE;
        if upright && let (Some(first), Some(second)) = (a.bounds, b.bounds) {
            return Some((first, second));
        }
        let band = |cluster: usize| {
            let this = &self.clusters[cluster];
            let start = self.along(seed, cluster);
            let across = (this.baseline.x - from.baseline.x).mul_add(
                -from.direction.y,
                (this.baseline.y - from.baseline.y) * from.direction.x,
            );
            [
                start,
                0.15f64.mul_add(-this.em, across),
                start + this.advance,
                0.7f64.mul_add(this.em, across),
            ]
        };
        Some((band(one), band(other)))
    }

    fn lies_below(&self, upper: usize, lower: usize) -> Option<bool> {
        let (above, below) = (self.line_seed(upper)?, self.line_seed(lower)?);
        let across = Point {
            x: -above.direction.y,
            y: above.direction.x,
        };
        Some(
            dot(
                Point {
                    x: below.baseline.x - above.baseline.x,
                    y: below.baseline.y - above.baseline.y,
                },
                across,
            ) < 0.0,
        )
    }

    fn painted_on_from(&self, last: usize, line: usize, open: &[usize], line_of: &[usize]) -> bool {
        let span = |line: usize| {
            let clusters = &self.lines[line].clusters;
            let low = clusters.iter().copied().min();
            let high = clusters.iter().copied().max();
            low.zip(high)
        };
        let (Some((above_low, above_high)), Some((below_low, below_high))) =
            (span(last), span(line))
        else {
            return false;
        };
        let Some(below_on_page) = self.lies_below(last, line) else {
            return false;
        };
        let between = if above_high < below_low {
            if !below_on_page {
                return false;
            }
            above_high + 1..below_low
        } else if below_high < above_low {
            if below_on_page {
                return false;
            }
            below_high + 1..above_low
        } else {
            return true;
        };
        let (Some(from), Some(to)) = (self.line_seed(last), self.line_seed(line)) else {
            return false;
        };
        let across = Point {
            x: -from.direction.y,
            y: from.direction.x,
        };
        let offset = |row: usize| {
            self.line_seed(row).map(|seed| {
                dot(
                    Point {
                        x: seed.baseline.x - from.baseline.x,
                        y: seed.baseline.y - from.baseline.y,
                    },
                    across,
                )
            })
        };
        let Some(end) = offset(line) else {
            return false;
        };
        let em = from.em.max(to.em);
        let inside = |row: usize| {
            let (Some(at), Some(seed)) = (offset(row), self.line_seed(row)) else {
                return false;
            };
            let margin = if seed.em < SUPERSCRIPT_SIZE * em {
                em / 2.0
            } else {
                BASELINE_TOLERANCE_EM * em
            };
            (end.min(0.0) - margin..=end.max(0.0) + margin).contains(&at)
        };
        between.into_iter().all(|cluster| {
            let owner = line_of.get(cluster).copied().unwrap_or(usize::MAX);
            self.clusters[cluster].advance <= 0.0
                || owner == line
                || open.contains(&owner)
                || inside(owner)
        })
    }

    fn passes_over_another_blocks_row(
        &self,
        last: usize,
        line: usize,
        open: &[usize],
        taken: &[bool],
    ) -> bool {
        let Some(above) = self.line_seed(last) else {
            return false;
        };
        let across = Point {
            x: -above.direction.y,
            y: above.direction.x,
        };
        let height = |row: usize| {
            self.line_seed(row).map(|seed| {
                dot(
                    Point {
                        x: seed.baseline.x - above.baseline.x,
                        y: seed.baseline.y - above.baseline.y,
                    },
                    across,
                )
            })
        };
        let (Some(to), Some(one), Some(other)) = (
            height(line),
            self.line_extent(last, above.direction),
            self.line_extent(line, above.direction),
        ) else {
            return false;
        };
        let (low, high) = if to < 0.0 { (to, 0.0) } else { (0.0, to) };
        let margin = above.em * BLOCK_MINIMUM_STEP_EM;
        let another_size = |row: usize| {
            self.line_seed(row).is_some_and(|seed| {
                let (larger, smaller) = if seed.em >= above.em {
                    (seed.em, above.em)
                } else {
                    (above.em, seed.em)
                };
                larger > 0.0 && smaller / larger < BLOCK_SIZE_TOLERANCE
            })
        };
        taken.iter().enumerate().any(|(row, claimed)| {
            *claimed
                && row != last
                && row != line
                && !open.contains(&row)
                && another_size(row)
                && height(row).is_some_and(|at| at > low + margin && at < high - margin)
                && self
                    .line_extent(row, above.direction)
                    .is_some_and(|between| {
                        overlap(between, one) >= BLOCK_OVERLAP
                            && overlap(between, other) >= BLOCK_OVERLAP
                    })
        })
    }

    fn closes_a_delimiter_over_one_that_opens(&self, last: usize, line: usize) -> bool {
        let (Some(&seed), Some(below)) = (
            self.lines[last].clusters.first(),
            self.lies_below(last, line),
        ) else {
            return false;
        };
        let (upper, lower) = if below { (last, line) } else { (line, last) };
        let pieces = |row: usize, bit: u8| -> Vec<(f64, f64)> {
            self.lines[row]
                .clusters
                .iter()
                .filter(|cluster| self.delimiters.get(**cluster).is_some_and(|p| p & bit != 0))
                .map(|cluster| (self.along(seed, *cluster), self.clusters[*cluster].em))
                .collect()
        };
        let closing = pieces(upper, CLOSES_A_DELIMITER);
        if closing.is_empty() {
            return false;
        }
        pieces(lower, OPENS_A_DELIMITER).iter().any(|(at, em)| {
            closing
                .iter()
                .any(|(other, other_em)| (at - other).abs() <= em.max(*other_em))
        })
    }

    fn step_between(&self, one: usize, other: usize) -> Option<f64> {
        let (above, below) = (self.line_seed(one)?, self.line_seed(other)?);
        let across = Point {
            x: -above.direction.y,
            y: above.direction.x,
        };
        Some(
            dot(
                Point {
                    x: below.baseline.x - above.baseline.x,
                    y: below.baseline.y - above.baseline.y,
                },
                across,
            )
            .abs(),
        )
    }

    fn join_rows_on_one_baseline(&mut self) {
        let mut gone = vec![false; self.lines.len()];
        for block in 0..self.blocks.len() {
            let lines = self.blocks[block].lines.clone();
            for (position, &into) in lines.iter().enumerate() {
                if gone[into] {
                    continue;
                }
                for &from in &lines[position + 1..] {
                    if gone[from] || !self.rows_share_a_baseline(into, from) {
                        continue;
                    }
                    let seed = self.line_seed(into).map(|_| self.lines[into].clusters[0]);
                    let Some(seed) = seed else { continue };
                    let span = |line: usize| {
                        self.lines[line].clusters.iter().fold(
                            (f64::INFINITY, f64::NEG_INFINITY),
                            |(low, high), cluster| {
                                let at = self.along(seed, *cluster);
                                (low.min(at), high.max(at + self.clusters[*cluster].advance))
                            },
                        )
                    };
                    let ((one_low, one_high), (other_low, other_high)) = (span(into), span(from));
                    if one_low < other_high && other_low < one_high {
                        continue;
                    }
                    let moved = std::mem::take(&mut self.lines[from].clusters);
                    let kept = std::mem::take(&mut self.lines[into].clusters);
                    let clusters: Vec<usize> = if other_low < one_low {
                        moved.into_iter().chain(kept).collect()
                    } else {
                        kept.into_iter().chain(moved).collect()
                    };
                    let widest = clusters
                        .windows(2)
                        .map(|pair| {
                            (self.along(seed, pair[1]) - self.along(seed, pair[0])).abs()
                                / self.clusters[pair[0]].em.max(f64::MIN_POSITIVE)
                        })
                        .fold(0.0_f64, f64::max);
                    self.lines[into] = Line {
                        clusters,
                        widest_gap: widest,
                    };
                    gone[from] = true;
                    self.report.rows_joined_on_one_baseline += 1;
                }
            }
        }
        if !gone.iter().any(|removed| *removed) {
            return;
        }
        let mut renumbered = vec![usize::MAX; self.lines.len()];
        let mut kept = Vec::with_capacity(self.lines.len());
        for (index, line) in std::mem::take(&mut self.lines).into_iter().enumerate() {
            if !gone[index] {
                renumbered[index] = kept.len();
                kept.push(line);
            }
        }
        self.lines = kept;
        for block in &mut self.blocks {
            block.lines = block
                .lines
                .iter()
                .filter(|line| !gone[**line])
                .map(|line| renumbered[*line])
                .collect();
        }
    }

    fn split_blocks_not_set_to_an_edge(&mut self) {
        let mut split = Vec::with_capacity(self.blocks.len());
        for block in std::mem::take(&mut self.blocks) {
            if block.lines.len() < 2 {
                split.push(block);
                continue;
            }
            let mut run: Vec<usize> = Vec::new();
            let mut runs: Vec<Vec<usize>> = Vec::new();
            let mut edges = EdgeRun::default();
            for line in block.lines {
                run.push(line);
                edges.take(self, line);
                if run.len() >= 2 && !edges.set_to_an_edge() {
                    let last = run.pop().expect("the row just pushed");
                    self.report.lines_split_by_edge += 1;
                    runs.push(std::mem::take(&mut run));
                    run.push(last);
                    edges = EdgeRun::default();
                    edges.take(self, last);
                }
            }
            if !run.is_empty() {
                runs.push(run);
            }
            for lines in runs {
                let evidence = if lines.len() == 1 {
                    self.report.blocks_of_one_line += 1;
                    BlockEvidence::Alone
                } else {
                    BlockEvidence::RunsOn
                };
                let bounds = lines
                    .iter()
                    .filter_map(|line| self.line_bounds(*line))
                    .reduce(union);
                let layout = lines
                    .iter()
                    .filter_map(|line| self.line_layout(*line))
                    .reduce(union);
                split.push(Block {
                    lines,
                    evidence,
                    bounds,
                    layout,
                });
            }
        }
        self.blocks = split;
    }

    fn rows_share_a_baseline(&self, one: usize, other: usize) -> bool {
        match (
            self.lines[one].clusters.first(),
            self.lines[other].clusters.first(),
        ) {
            (Some(a), Some(b)) => {
                let (left, right) = (&self.clusters[*a], &self.clusters[*b]);
                left.advance > 0.0 && right.advance > 0.0 && self.shares_baseline(*a, *b)
            }
            _ => false,
        }
    }

    fn close_block(&mut self, open: &mut Vec<usize>) {
        let lines = std::mem::take(open);
        let bounds = lines
            .iter()
            .filter_map(|line| self.line_bounds(*line))
            .reduce(union);
        let layout = lines
            .iter()
            .filter_map(|line| self.line_layout(*line))
            .reduce(union);
        let evidence = if lines.len() == 1 {
            self.report.blocks_of_one_line += 1;
            BlockEvidence::Alone
        } else {
            BlockEvidence::RunsOn
        };
        self.blocks.push(Block {
            lines,
            evidence,
            bounds,
            layout,
        });
    }

    fn only_the_columns_disagree(&self, previous: usize, line: usize) -> bool {
        let (Some(above), Some(below)) = (self.line_seed(previous), self.line_seed(line)) else {
            return false;
        };
        if dot(above.direction, below.direction) < DIRECTION_TOLERANCE {
            return false;
        }
        let larger = above.em.max(below.em);
        let smaller = above.em.min(below.em);
        if larger <= 0.0 || smaller / larger < BLOCK_SIZE_TOLERANCE {
            return false;
        }
        let across = Point {
            x: -above.direction.y,
            y: above.direction.x,
        };
        let step = dot(
            Point {
                x: below.baseline.x - above.baseline.x,
                y: below.baseline.y - above.baseline.y,
            },
            across,
        )
        .abs();
        if !(larger * BLOCK_MINIMUM_STEP_EM..=larger * BLOCK_LEADING_EM).contains(&step) {
            return false;
        }
        let (Some(one), Some(other)) = (
            self.line_extent(previous, above.direction),
            self.line_extent(line, above.direction),
        ) else {
            return false;
        };
        overlap(one, other) < BLOCK_OVERLAP
    }

    fn line_seed(&self, line: usize) -> Option<&Cluster> {
        self.clusters.get(*self.lines.get(line)?.clusters.first()?)
    }

    fn lines_run_on(&self, previous: usize, line: usize) -> bool {
        let (Some(above), Some(below)) = (self.line_seed(previous), self.line_seed(line)) else {
            return false;
        };
        if dot(above.direction, below.direction) < DIRECTION_TOLERANCE {
            return false;
        }
        let (Some(&one), Some(&other)) = (
            self.lines[previous].clusters.first(),
            self.lines[line].clusters.first(),
        ) else {
            return false;
        };
        if !self.same_scope(one, other) {
            return false;
        }
        let (larger, smaller) = if above.em >= below.em {
            (above.em, below.em)
        } else {
            (below.em, above.em)
        };
        if larger <= 0.0 || smaller / larger < BLOCK_SIZE_TOLERANCE {
            return false;
        }
        let across = Point {
            x: -above.direction.y,
            y: above.direction.x,
        };
        let step = dot(
            Point {
                x: below.baseline.x - above.baseline.x,
                y: below.baseline.y - above.baseline.y,
            },
            across,
        )
        .abs();
        if step > larger * BLOCK_LEADING_EM {
            return false;
        }
        if step < larger * BLOCK_MINIMUM_STEP_EM {
            return false;
        }
        let (Some(one), Some(other)) = (
            self.line_extent(previous, above.direction),
            self.line_extent(line, above.direction),
        ) else {
            return false;
        };
        overlap(one, other) >= BLOCK_OVERLAP
    }

    fn sets_off_a_heading(&self, previous: usize, line: usize) -> bool {
        let Some(below) = self.lies_below(previous, line) else {
            return false;
        };
        let (upper, lower) = if below {
            (previous, line)
        } else {
            (line, previous)
        };
        let face = |cluster: usize| self.faces.get(cluster).copied();
        let upper_clusters = &self.lines[upper].clusters;
        let Some(heading) = upper_clusters.first().and_then(|first| face(*first)) else {
            return false;
        };
        if upper_clusters
            .iter()
            .any(|cluster| face(*cluster) != Some(heading))
        {
            return false;
        }
        if self.lines[lower]
            .clusters
            .first()
            .and_then(|first| face(*first))
            == Some(heading)
        {
            return false;
        }
        let (Some(above), Some(beneath)) = (self.line_seed(upper), self.line_seed(lower)) else {
            return false;
        };
        let (Some((_, upper_end)), Some((_, lower_end))) = (
            self.line_extent(upper, above.direction),
            self.line_extent(lower, above.direction),
        ) else {
            return false;
        };
        upper_end + HEADING_SHORTER_EM * above.em.max(beneath.em) <= lower_end
    }

    fn are_cells(previous: usize, line: usize, slots: &[Option<(f64, f64)>]) -> bool {
        let short = |line: usize| {
            slots
                .get(line)
                .copied()
                .flatten()
                .is_some_and(|(width, room)| room > 0.0 && width < CELL_FILLS * room)
        };
        short(previous) && short(line)
    }

    fn row_slots(&self) -> Vec<Option<(f64, f64)>> {
        let reach: Vec<Option<(f64, f64)>> = (0..self.lines.len())
            .map(|line| self.line_reach(line))
            .collect();
        (0..self.lines.len())
            .map(|line| {
                let seed = self.line_seed(line)?;
                let (start, end) = reach[line]?;
                let across = Point {
                    x: -seed.direction.y,
                    y: seed.direction.x,
                };
                let mut after: Option<f64> = None;
                let mut before: Option<f64> = None;
                for (other, other_reach) in reach.iter().enumerate() {
                    if other == line {
                        continue;
                    }
                    let (Some(mate), Some((mate_start, mate_end))) =
                        (self.line_seed(other), *other_reach)
                    else {
                        continue;
                    };
                    if dot(seed.direction, mate.direction) < DIRECTION_TOLERANCE {
                        continue;
                    }
                    let offset = dot(
                        Point {
                            x: mate.baseline.x - seed.baseline.x,
                            y: mate.baseline.y - seed.baseline.y,
                        },
                        across,
                    );
                    if offset.abs() >= ROW_MATE_EM * seed.em.max(mate.em) {
                        continue;
                    }
                    if mate_start >= end {
                        let room = mate_start - start;
                        after = Some(after.map_or(room, |nearest| nearest.min(room)));
                    } else if mate_end <= start {
                        let room = start - mate_start;
                        before = Some(before.map_or(room, |nearest| nearest.min(room)));
                    }
                }
                after.or(before).map(|room| (end - start, room))
            })
            .collect()
    }

    fn line_reach(&self, line: usize) -> Option<(f64, f64)> {
        let direction = self.line_seed(line)?.direction;
        let mut span: Option<(f64, f64)> = None;
        for index in &self.lines.get(line)?.clusters {
            let cluster = self.clusters.get(*index)?;
            let along = dot(cluster.baseline, direction);
            let end = along + cluster.advance.max(0.0);
            span = Some(span.map_or((along, end), |(low, high)| (low.min(along), high.max(end))));
        }
        span
    }

    fn line_extent(&self, line: usize, direction: Point) -> Option<(f64, f64)> {
        let line = self.lines.get(line)?;
        let mut span: Option<(f64, f64)> = None;
        for index in &line.clusters {
            let cluster = self.clusters.get(*index)?;
            let along = dot(
                Point {
                    x: cluster.baseline.x,
                    y: cluster.baseline.y,
                },
                direction,
            );
            span = Some(match span {
                None => (along, along),
                Some((low, high)) => (low.min(along), high.max(along)),
            });
        }
        span
    }

    fn group_lines_over(&mut self, members: &[usize]) {
        let mut remaining: Vec<usize> = members.to_vec();
        let across = AcrossBaselines::of(&self.clusters, members);
        let reading_order = |this: &Self, left: usize, right: usize| {
            reading_order(&this.clusters[left], &this.clusters[right])
        };
        remaining.sort_by(|left, right| {
            let marks = |cluster: usize| self.clusters[cluster].advance <= 0.0;
            marks(*left)
                .cmp(&marks(*right))
                .then_with(|| reading_order(self, *left, *right))
        });
        let first_line = self.lines.len();
        let mut seeds: Vec<usize> = Vec::new();

        let mut taken = vec![true; self.clusters.len()];
        for member in members {
            taken[*member] = false;
        }
        for seed in remaining {
            if taken[seed] {
                continue;
            }
            let mut on_baseline: Vec<usize> = match &across {
                Some(across) => across
                    .near(&self.clusters[seed], BASELINE_TOLERANCE_EM)
                    .filter(|candidate| !taken[*candidate])
                    .filter(|candidate| self.shares_baseline(seed, *candidate))
                    .collect(),
                None => (0..self.clusters.len())
                    .filter(|candidate| !taken[*candidate])
                    .filter(|candidate| self.shares_baseline(seed, *candidate))
                    .collect(),
            };
            on_baseline.sort_unstable();
            let on_baseline = on_baseline;
            let others = on_baseline.len();
            let on_baseline: Vec<usize> = on_baseline
                .into_iter()
                .filter(|candidate| self.same_scope(seed, *candidate))
                .collect();
            if on_baseline.len() < others {
                self.report.lines_split_by_content += 1;
            }
            let (mut members, layers) = self.layer_holding(seed, on_baseline);
            if layers > 1 {
                self.report.lines_layered_apart += 1;
            }
            let mut lifted = self.marks_lifted_off(seed, &members, &taken, across.as_ref());
            lifted.retain(|mark| self.same_scope(seed, *mark));
            self.report.marks_lifted_onto_their_row += lifted.len();
            members.extend(lifted);
            members.sort_by(|left, right| {
                self.along(seed, *left).total_cmp(&self.along(seed, *right))
            });
            self.report.marks_reordered_by_paint +=
                self.order_shared_pen_positions_by_paint(seed, &mut members);
            self.report.marks_reordered_by_paint +=
                self.attach_marks_to_their_letters(seed, &mut members);

            let mut line = Vec::new();
            let mut widest = 0.0_f64;
            let mut previous: Option<usize> = None;
            for member in members {
                if let Some(before) = previous {
                    let gap = (self.along(seed, member) - self.along(seed, before))
                        / self.clusters[before].em.max(f64::MIN_POSITIVE);
                    if gap > COLUMN_GAP_EM {
                        self.report.lines_split_by_gap += 1;
                        break;
                    }
                    widest = widest.max(gap);
                }
                taken[member] = true;
                line.push(member);
                previous = Some(member);
            }
            if !line.is_empty() {
                self.lines.push(Line {
                    clusters: line,
                    widest_gap: widest,
                });
                seeds.push(seed);
            }
        }
        let mut ordered: Vec<(usize, Line)> = seeds
            .into_iter()
            .zip(self.lines.drain(first_line..))
            .collect();
        ordered.sort_by(|(one, _), (other, _)| reading_order(self, *one, *other));
        self.lines.extend(ordered.into_iter().map(|(_, line)| line));
        let stranded: Vec<usize> = members
            .iter()
            .copied()
            .filter(|member| !taken[*member])
            .collect();
        self.report.clusters_stranded_by_geometry += stranded.len();
        for cluster in stranded {
            self.lines.push(Line {
                clusters: vec![cluster],
                widest_gap: 0.0,
            });
        }
    }

    fn order_shared_pen_positions_by_paint(&self, seed: usize, members: &mut [usize]) -> usize {
        let mut reordered = 0;
        let mut start = 0;
        while start < members.len() {
            let first = self.along(seed, members[start]);
            let em = self.clusters[members[start]].em.max(f64::MIN_POSITIVE);
            let mut end = start + 1;
            while end < members.len()
                && (self.along(seed, members[end]) - first).abs() <= SAME_PEN_EM * em
            {
                end += 1;
            }
            if end - start > 1 {
                let was = members[start..end].to_vec();
                members[start..end].sort_unstable();
                if members[start..end] != was[..] {
                    reordered += end - start - 1;
                }
            }
            start = end;
        }
        reordered
    }

    fn layer_holding(&self, seed: usize, mut members: Vec<usize>) -> (Vec<usize>, usize) {
        if !members.contains(&seed) {
            return (members, 1);
        }
        members.sort_unstable();
        let mut runs: Vec<Vec<usize>> = Vec::new();
        for member in members {
            match runs.last_mut() {
                Some(run)
                    if run.last().is_some_and(|last| {
                        last + 1 == member && self.same_size(*last, member)
                    }) =>
                {
                    run.push(member);
                }
                _ => runs.push(vec![member]),
            }
        }
        let mut layers: Vec<Vec<usize>> = Vec::new();
        for run in runs {
            match layers
                .iter_mut()
                .find(|layer| !self.runs_collide(seed, layer, &run))
            {
                Some(layer) => layer.extend(run),
                None => layers.push(run),
            }
        }
        let count = layers.len();
        let held = layers
            .into_iter()
            .find(|layer| layer.contains(&seed))
            .unwrap_or_default();
        (held, count)
    }

    fn runs_collide(&self, seed: usize, layer: &[usize], run: &[usize]) -> bool {
        run.iter().any(|later| {
            layer.iter().any(|earlier| {
                self.letters_collide(seed, *earlier, *later) && !self.repeats(*earlier, *later)
            })
        })
    }

    fn letters_collide(&self, seed: usize, one: usize, other: usize) -> bool {
        let (Some(a), Some(b)) = (self.extent_along(seed, one), self.extent_along(seed, other))
        else {
            return false;
        };
        if !(a.1 > a.0 && b.1 > b.0) {
            return false;
        }
        let shared = a.1.min(b.1) - a.0.max(b.0);
        let measure = if self.same_size(one, other) {
            (a.1 - a.0).max(b.1 - b.0)
        } else {
            (a.1 - a.0).min(b.1 - b.0)
        };
        shared >= COLLISION_OVERLAP * measure
    }

    fn same_size(&self, one: usize, other: usize) -> bool {
        let (a, b) = (self.clusters[one].em.abs(), self.clusters[other].em.abs());
        let (larger, smaller) = if a >= b { (a, b) } else { (b, a) };
        larger <= 0.0 || smaller / larger >= BLOCK_SIZE_TOLERANCE
    }

    fn extent_along(&self, seed: usize, cluster: usize) -> Option<(f64, f64)> {
        let (origin, direction) = (self.clusters[seed].baseline, self.clusters[seed].direction);
        let this = &self.clusters[cluster];
        if this.blank || this.advance <= 0.0 {
            return None;
        }
        if let Some([x0, y0, x1, y1]) = this.bounds {
            let project =
                |x: f64, y: f64| (x - origin.x).mul_add(direction.x, (y - origin.y) * direction.y);
            let corners = [
                project(x0, y0),
                project(x1, y0),
                project(x1, y1),
                project(x0, y1),
            ];
            let low = corners.iter().copied().fold(f64::INFINITY, f64::min);
            let high = corners.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            return Some((low, high));
        }
        let from = self.along(seed, cluster);
        Some((from, from + this.advance))
    }

    fn repeats(&self, earlier: usize, later: usize) -> bool {
        let (a, b) = (&self.clusters[earlier], &self.clusters[later]);
        let em = a.em.max(b.em);
        a.code == b.code
            && a.glyphs.len() == b.glyphs.len()
            && (a.em - b.em).abs() <= em * 0.01
            && (b.baseline.x - a.baseline.x).hypot(b.baseline.y - a.baseline.y)
                <= FAKE_BOLD_DISTANCE_EM * em
            && a.atom.abs_diff(b.atom) <= FAKE_BOLD_ATOM_WINDOW
    }

    fn marks_lifted_off(
        &self,
        seed: usize,
        members: &[usize],
        taken: &[bool],
        across: Option<&AcrossBaselines>,
    ) -> Vec<usize> {
        let is_mark = |cluster: usize| self.clusters[cluster].advance <= 0.0;
        let em = self.clusters[seed].em;
        let candidates: Vec<usize> = match across {
            Some(across) => {
                let mut near: Vec<usize> =
                    across.near(&self.clusters[seed], MARK_LIFT_EM).collect();
                near.sort_unstable();
                near
            }
            None => (0..self.clusters.len()).collect(),
        };
        candidates
            .into_iter()
            .filter(|mark| !taken[*mark] && is_mark(*mark) && !members.contains(mark))
            .filter(|mark| {
                let (a, b) = (&self.clusters[seed], &self.clusters[*mark]);
                if a.direction
                    .x
                    .mul_add(b.direction.x, a.direction.y * b.direction.y)
                    < DIRECTION_TOLERANCE
                {
                    return false;
                }
                let across = (b.baseline.x - a.baseline.x).mul_add(
                    -a.direction.y,
                    (b.baseline.y - a.baseline.y) * a.direction.x,
                );
                if across.abs() > MARK_LIFT_EM * em.max(b.em) {
                    return false;
                }
                let Some(letter) = (0..*mark).rev().find(|before| !is_mark(*before)) else {
                    return false;
                };
                if !members.contains(&letter) {
                    return false;
                }
                let start = self.along(seed, letter);
                let end = start + self.clusters[letter].advance;
                let reach = self.clusters[letter].em / 2.0;
                (start - reach..=end + reach).contains(&self.along(seed, *mark))
            })
            .collect()
    }

    fn attach_marks_to_their_letters(&self, seed: usize, members: &mut Vec<usize>) -> usize {
        let is_mark = |cluster: usize| self.clusters[cluster].advance <= 0.0;
        let marks: Vec<usize> = members.iter().copied().filter(|m| is_mark(*m)).collect();
        if marks.is_empty() {
            return 0;
        }
        let mut moved = 0;
        for mark in marks {
            let Some(letter) = members
                .iter()
                .copied()
                .filter(|other| *other < mark && !is_mark(*other))
                .max()
            else {
                continue;
            };
            if (letter + 1..mark).any(|between| !is_mark(between)) {
                continue;
            }
            let start = self.along(seed, letter);
            let end = start + self.clusters[letter].advance;
            let at = self.along(seed, mark);
            let reach = self.clusters[letter].em / 2.0;
            if !(start - reach..=end + reach).contains(&at) {
                continue;
            }
            let Some(from) = members.iter().position(|m| *m == mark) else {
                continue;
            };
            members.remove(from);
            let Some(after) = members.iter().position(|m| *m == letter) else {
                members.insert(from, mark);
                continue;
            };
            let mut to = after + 1;
            while to < members.len() && is_mark(members[to]) && members[to] < mark {
                to += 1;
            }
            if to != from {
                moved += 1;
            }
            members.insert(to, mark);
        }
        moved
    }

    fn shares_baseline(&self, seed: usize, candidate: usize) -> bool {
        let (a, b) = (&self.clusters[seed], &self.clusters[candidate]);
        if a.direction
            .x
            .mul_add(b.direction.x, a.direction.y * b.direction.y)
            < DIRECTION_TOLERANCE
        {
            return false;
        }
        let across = (b.baseline.x - a.baseline.x).mul_add(
            -a.direction.y,
            (b.baseline.y - a.baseline.y) * a.direction.x,
        );
        across.abs() <= BASELINE_TOLERANCE_EM * a.em.max(b.em)
    }

    fn along(&self, seed: usize, cluster: usize) -> f64 {
        let (a, b) = (&self.clusters[seed], &self.clusters[cluster]);
        (b.baseline.x - a.baseline.x)
            .mul_add(a.direction.x, (b.baseline.y - a.baseline.y) * a.direction.y)
    }

    #[must_use]
    pub fn caret_at(&self, point: Point) -> Option<Caret> {
        let line = (0..self.lines.len()).min_by(|left, right| {
            self.distance_to_line(*left, point)
                .total_cmp(&self.distance_to_line(*right, point))
        })?;
        Some(self.caret_on_line(line, point))
    }

    fn caret_on_line(&self, line: usize, point: Point) -> Caret {
        let clusters = &self.lines[line].clusters;
        let offset = (0..=clusters.len())
            .min_by(|left, right| {
                let (a, b) = (
                    self.caret_point(line, *left),
                    self.caret_point(line, *right),
                );
                (a.x - point.x)
                    .hypot(a.y - point.y)
                    .total_cmp(&(b.x - point.x).hypot(b.y - point.y))
            })
            .unwrap_or(0);
        Caret { line, offset }
    }

    fn distance_to_line(&self, line: usize, point: Point) -> f64 {
        self.lines[line]
            .clusters
            .first()
            .map_or(f64::INFINITY, |first| {
                let cluster = &self.clusters[*first];
                (point.x - cluster.baseline.x)
                    .mul_add(
                        -cluster.direction.y,
                        (point.y - cluster.baseline.y) * cluster.direction.x,
                    )
                    .abs()
            })
    }

    #[must_use]
    pub fn caret_point(&self, line: usize, offset: usize) -> Point {
        let clusters = &self.lines[line].clusters;
        if clusters.is_empty() {
            return Point { x: 0.0, y: 0.0 };
        }
        clusters.get(offset).map_or_else(
            || {
                let last = &self.clusters[*clusters.last().unwrap_or(&0)];
                let advance = self.cluster_advance(line, clusters.len() - 1);
                Point {
                    x: last.direction.x.mul_add(advance, last.baseline.x),
                    y: last.direction.y.mul_add(advance, last.baseline.y),
                }
            },
            |cluster| self.clusters[*cluster].baseline,
        )
    }

    fn cluster_advance(&self, line: usize, position: usize) -> f64 {
        let clusters = &self.lines[line].clusters;
        let Some(current) = clusters.get(position) else {
            return 0.0;
        };
        clusters.get(position + 1).map_or_else(
            || self.clusters[*current].em * 0.5,
            |next| {
                let (a, b) = (&self.clusters[*current], &self.clusters[*next]);
                (b.baseline.x - a.baseline.x)
                    .mul_add(a.direction.x, (b.baseline.y - a.baseline.y) * a.direction.y)
            },
        )
    }

    #[must_use]
    pub fn caret_left(&self, caret: Caret) -> Caret {
        if caret.offset > 0 {
            return Caret {
                line: caret.line,
                offset: caret.offset - 1,
            };
        }
        if caret.line == 0 {
            return caret;
        }
        let line = caret.line - 1;
        Caret {
            line,
            offset: self.lines[line].clusters.len(),
        }
    }

    #[must_use]
    pub fn caret_right(&self, caret: Caret) -> Caret {
        if caret.offset < self.lines[caret.line].clusters.len() {
            return Caret {
                line: caret.line,
                offset: caret.offset + 1,
            };
        }
        if caret.line + 1 >= self.lines.len() {
            return caret;
        }
        Caret {
            line: caret.line + 1,
            offset: 0,
        }
    }

    #[must_use]
    pub fn caret_up(&self, caret: Caret) -> Caret {
        if caret.line == 0 {
            return caret;
        }
        self.caret_on_line(caret.line - 1, self.caret_point(caret.line, caret.offset))
    }

    #[must_use]
    pub fn caret_down(&self, caret: Caret) -> Caret {
        if caret.line + 1 >= self.lines.len() {
            return caret;
        }
        self.caret_on_line(caret.line + 1, self.caret_point(caret.line, caret.offset))
    }

    #[must_use]
    pub fn selection(&self, from: Caret, to: Caret) -> Vec<usize> {
        if from.line != to.line {
            return Vec::new();
        }
        let (start, end) = if from.offset <= to.offset {
            (from.offset, to.offset)
        } else {
            (to.offset, from.offset)
        };
        self.lines[from.line]
            .clusters
            .get(start..end)
            .unwrap_or_default()
            .to_vec()
    }

    pub fn selection_span(&self, from: Caret, to: Caret) -> Result<SelectionSpan, SelectionError> {
        let spans = self.selection_spans(from, to)?;
        let [span] = spans.as_slice() else {
            return Err(SelectionError::CrossesRuns(spans.len()));
        };
        Ok(span.clone())
    }

    pub fn selection_spans(
        &self,
        from: Caret,
        to: Caret,
    ) -> Result<Vec<SelectionSpan>, SelectionError> {
        let selected = self.selection(from, to);
        if selected.is_empty() {
            return Err(SelectionError::Empty);
        }
        let mut atoms = Vec::new();
        for cluster in &selected {
            let atom = self.clusters[*cluster].atom;
            if !atoms.contains(&atom) {
                atoms.push(atom);
            }
        }
        let mut spans = Vec::with_capacity(atoms.len());
        for atom in atoms {
            let clusters: Vec<&Cluster> = selected
                .iter()
                .map(|cluster| &self.clusters[*cluster])
                .filter(|cluster| cluster.atom == atom)
                .collect();
            let start = clusters
                .iter()
                .map(|cluster| cluster.glyphs.start)
                .min()
                .unwrap_or(0);
            let end = clusters
                .iter()
                .map(|cluster| cluster.glyphs.end)
                .max()
                .unwrap_or(0);
            let covered: usize = clusters.iter().map(|cluster| cluster.glyphs.len()).sum();
            if end - start != covered {
                return Err(SelectionError::NotContiguous);
            }
            spans.push(SelectionSpan {
                atom,
                glyphs: start..end,
                clusters: clusters.len(),
            });
        }
        Ok(spans)
    }

    fn extend_from_run(&mut self, atom: usize, text: &TextShowPaint) {
        let Some(direction) = advance_direction(text) else {
            return;
        };
        let ctm = text.state.ctm.value;
        let page_direction = {
            let placed = ctm.multiply(text.matrices.text.value);
            let length = placed.a.hypot(placed.b);
            if length > 0.0 {
                Point {
                    x: placed.a / length,
                    y: placed.b / length,
                }
            } else {
                return;
            }
        };
        let mut open: Option<Cluster> = None;
        for (position, glyph) in text.glyphs.iter().enumerate() {
            let origin = Point {
                x: glyph.matrix.e,
                y: glyph.matrix.f,
            };
            let evidence = if open.is_none() {
                MarkEvidence::default()
            } else {
                let previous = &text.glyphs[position - 1];
                MarkEvidence {
                    declared_zero_width: glyph.code.width == 0.0,
                    continues_a_source_code: glyph.silent,
                    origin_did_not_advance: previous.code.width != 0.0
                        && !advanced(
                            Point {
                                x: previous.matrix.e,
                                y: previous.matrix.f,
                            },
                            origin,
                            direction,
                        ),
                    extends_the_cluster_before: extends_the_cluster_before(text, previous, glyph),
                }
            };
            match open.as_mut() {
                Some(current) if evidence.joins_previous() => {
                    current.glyphs.end = position + 1;
                    current.advance += glyph_advance(ctm, glyph);
                    current.bounds = text.outline_bounds_in(current.glyphs.clone());
                    current.layout = text.layout_bounds_in(current.glyphs.clone());
                    current.blank = text.program.is_some() && current.bounds.is_none();
                    if evidence.continues_a_source_code {
                        current.continuations += 1;
                        self.report.continuations += 1;
                    } else {
                        current.marks += 1;
                        self.report.marks += 1;
                        if evidence.declared_zero_width {
                            self.report.marks_by_width += 1;
                        }
                        if evidence.origin_did_not_advance {
                            self.report.marks_by_position += 1;
                        }
                    }
                }
                _ => {
                    if let Some(finished) = open.replace(Cluster {
                        atom,
                        glyphs: position..position + 1,
                        origin,
                        baseline: ctm.transform(origin),
                        em: {
                            let placed = ctm.multiply(glyph.matrix);
                            placed.c.hypot(placed.d)
                        },
                        direction: page_direction,
                        evidence: ClusterEvidence::Inferred,
                        marks: 0,
                        continuations: 0,
                        bounds: text.outline_bounds_in(position..position + 1),
                        layout: text.layout_bounds_in(position..position + 1),
                        advance: glyph_advance(ctm, glyph),
                        code: glyph.code.value,
                        blank: text.program.is_some()
                            && text.outline_bounds_in(position..position + 1).is_none(),
                    }) {
                        self.clusters.push(finished);
                    }
                }
            }
        }
        if let Some(last) = open {
            self.clusters.push(last);
        }
    }
}

fn advance_direction(text: &TextShowPaint) -> Option<Point> {
    let matrix = text.matrices.text.value;
    let length = matrix.a.hypot(matrix.b);
    (length > 0.0).then(|| Point {
        x: matrix.a / length,
        y: matrix.b / length,
    })
}

fn glyph_advance(ctm: Matrix, glyph: &pdf_paint::PositionedGlyph) -> f64 {
    let placed = ctm.multiply(glyph.matrix);
    placed.a.hypot(placed.b) * glyph.code.width * 0.001
}

fn advanced(previous: Point, current: Point, direction: Point) -> bool {
    let along =
        (current.x - previous.x).mul_add(direction.x, (current.y - previous.y) * direction.y);
    along.abs() > ADVANCE_EPSILON
}

#[cfg(test)]
mod tests {
    use super::{
        ADVANCE_EPSILON, AcrossBaselines, AcrossRows, BlockEvidence, Caret, Cluster,
        ClusterEvidence, MarkEvidence, Member, MembershipAudit, ObjectKind, Quad, QuadEvidence,
        SelectionError, SemanticIndex, advanced, overlap,
    };
    use pdf_bytes::{SourceId, SourceSpan};
    use pdf_font::SourceCode;
    use pdf_paint::{
        ColorSpace, Derived, FillRule, GraphicsState, ImagePaint, Matrix, PaintAtom, PaintAtomKind,
        PaintGraph, PaintId, Path, PathPaint, PathSegment, Point, PositionedGlyph, TextMatrices,
        TextShowPaint, Type3Glyph,
    };
    use pdf_syntax::Reference;

    fn span() -> SourceSpan {
        SourceSpan::new(SourceId::new(1), 0, 1).expect("a forward span")
    }

    fn code(width: f64) -> SourceCode {
        SourceCode {
            bytes: vec![0],
            value: 0,
            byte_offset: 0,
            cid: None,
            mapping_span: None,
            completed_bytes: 0,
            width,
        }
    }

    fn run(origins: &[(f64, f64)], widths: &[f64], text_matrix: Matrix) -> TextShowPaint {
        TextShowPaint {
            elements: Vec::new(),
            state: GraphicsState::default(),
            matrices: TextMatrices {
                text: Derived {
                    value: text_matrix,
                    provenance: pdf_paint::Provenance::new(),
                },
                line: Derived {
                    value: text_matrix,
                    provenance: pdf_paint::Provenance::new(),
                },
            },
            glyphs: origins
                .iter()
                .zip(widths)
                .map(|(&(x, y), &width)| PositionedGlyph {
                    code: code(width),
                    glyph: Some(1),
                    text_matrix: Matrix {
                        a: 1.0,
                        b: 0.0,
                        c: 0.0,
                        d: 1.0,
                        e: x,
                        f: y,
                    },
                    matrix: Matrix {
                        a: text_matrix.a,
                        b: text_matrix.b,
                        c: text_matrix.c,
                        d: text_matrix.d,
                        e: x,
                        f: y,
                    },
                    procedure: None,
                    substituted: Vec::new(),
                    silent: false,
                    unresolved: None,
                })
                .collect(),
            program: None,
            text: std::sync::Arc::default(),
            substitution: None,
            font_request: None,
            units_per_em: 1000,
            type3: false,
            family_line: None,
        }
    }

    fn graph_of(text: TextShowPaint) -> PaintGraph {
        PaintGraph {
            object_scopes: Vec::new(),
            repairs: Vec::new(),
            skipped: Vec::new(),
            atoms: vec![PaintAtom {
                id: PaintId {
                    page: Reference::new(1, 0),
                    stream: Reference::new(2, 0),
                    operator_span: span(),
                    invocation_path: Vec::new(),
                    pattern_path: Vec::new(),
                    ordinal: 0,
                },
                kind: PaintAtomKind::Text(text),
                marks: Vec::new(),
            }],
        }
    }

    fn clusters(text: TextShowPaint) -> SemanticIndex {
        SemanticIndex::of(&graph_of(text))
    }

    struct Cell {
        origins: Vec<(f64, f64)>,
        widths: Vec<f64>,
        matrix: Matrix,
    }

    fn page(runs: &[Cell]) -> PaintGraph {
        PaintGraph {
            object_scopes: Vec::new(),
            repairs: Vec::new(),
            skipped: Vec::new(),
            atoms: runs
                .iter()
                .enumerate()
                .map(|(ordinal, cell)| PaintAtom {
                    id: PaintId {
                        page: Reference::new(1, 0),
                        stream: Reference::new(2, 0),
                        operator_span: span(),
                        invocation_path: Vec::new(),
                        pattern_path: Vec::new(),
                        ordinal,
                    },
                    kind: PaintAtomKind::Text(run(&cell.origins, &cell.widths, cell.matrix)),
                    marks: Vec::new(),
                })
                .collect(),
        }
    }

    const EM: f64 = 12.0;

    fn at(x: f64, y: f64) -> Cell {
        Cell {
            origins: vec![(x, y)],
            widths: vec![600.0],
            matrix: Matrix {
                a: EM,
                b: 0.0,
                c: 0.0,
                d: EM,
                e: x,
                f: y,
            },
        }
    }

    #[test]
    fn two_clusters_at_one_pen_position_keep_the_order_the_file_painted() {
        let index = SemanticIndex::of(&page(&[at(0.0, 100.0), at(7.01, 100.0), at(7.0, 100.0)]));
        let [line] = index.lines.as_slice() else {
            panic!("one row, got {:?}", index.lines.len());
        };
        assert_eq!(
            line.clusters,
            vec![0, 1, 2],
            "the pen did not move between the last two, so the file decides"
        );
        assert_eq!(index.report.marks_reordered_by_paint, 1);

        let index = SemanticIndex::of(&page(&[at(0.0, 100.0), at(14.0, 100.0), at(7.0, 100.0)]));
        let [line] = index.lines.as_slice() else {
            panic!("one row");
        };
        assert_eq!(
            line.clusters,
            vec![0, 2, 1],
            "paint order is not reading order when the pen actually advanced"
        );
        assert_eq!(index.report.marks_reordered_by_paint, 0);
    }

    #[test]
    fn the_shortlist_and_the_scan_group_the_same_rows() {
        let upright = || -> Vec<Cell> {
            (0..6)
                .flat_map(|row| {
                    (0..5).map(move |column| {
                        at(f64::from(column) * 8.0, 200.0 - f64::from(row) * 16.0)
                    })
                })
                .collect()
        };
        let shortlisted = SemanticIndex::of(&page(&upright()));
        assert!(
            AcrossBaselines::of(
                &shortlisted.clusters,
                &(0..shortlisted.clusters.len()).collect::<Vec<usize>>()
            )
            .is_some(),
            "a control: every cluster of this fixture travels one way"
        );

        let mut mixed = upright();
        mixed.push(Cell {
            origins: vec![(400.0, 400.0)],
            widths: vec![600.0],
            matrix: Matrix {
                a: 0.0,
                b: EM,
                c: -EM,
                d: 0.0,
                e: 400.0,
                f: 400.0,
            },
        });
        let scanned = SemanticIndex::of(&page(&mixed));
        assert!(
            AcrossBaselines::of(
                &scanned.clusters,
                &(0..scanned.clusters.len()).collect::<Vec<usize>>()
            )
            .is_none(),
            "the turned run is what sends this reading down the scan"
        );

        let shared = shortlisted.clusters.len();
        let rows = |index: &SemanticIndex| -> Vec<Vec<usize>> {
            index
                .lines
                .iter()
                .map(|line| line.clusters.clone())
                .filter(|clusters| clusters.iter().all(|cluster| *cluster < shared))
                .collect()
        };
        assert_eq!(rows(&shortlisted), rows(&scanned));
        assert_eq!(rows(&shortlisted).len(), 6, "six rows of five");
    }

    #[test]
    fn the_shortlist_and_the_scan_group_the_same_blocks() {
        let upright = || -> Vec<Cell> {
            let mut cells = Vec::new();
            for line in 0..3 {
                let y = 300.0 - 1.5 * EM * f64::from(line);
                cells.extend(row(0.0, y, 36));
                cells.extend(row(40.0 * EM, y, 36));
            }
            for offset in [0.0, 0.4 * EM, 0.8 * EM] {
                for line in 0..3 {
                    cells.extend(row(0.0, 100.0 - offset - 1.2 * EM * f64::from(line), 5));
                }
            }
            cells.extend(row(0.0, 40.0, 10));
            cells.extend(row(0.0, 40.0 - 1.5 * EM, 50));
            cells.extend(row(40.0 * EM, 40.0 - 3.0 * EM, 5));
            cells
        };
        let shortlisted = SemanticIndex::of(&page(&upright()));
        assert!(
            AcrossRows::of(&shortlisted, 0).is_some(),
            "a control: every row of this fixture travels one way"
        );

        let mut mixed = upright();
        mixed.push(Cell {
            origins: vec![(400.0, 400.0)],
            widths: vec![600.0],
            matrix: Matrix {
                a: 0.0,
                b: EM,
                c: -EM,
                d: 0.0,
                e: 400.0,
                f: 400.0,
            },
        });
        let scanned = SemanticIndex::of(&page(&mixed));
        assert!(
            AcrossRows::of(&scanned, 0).is_none(),
            "the turned run is what sends this reading down the scan"
        );

        let shared = shortlisted.clusters.len();
        let blocks = |index: &SemanticIndex| -> Vec<Vec<Vec<usize>>> {
            index
                .blocks
                .iter()
                .map(|block| {
                    block
                        .lines
                        .iter()
                        .map(|line| index.lines[*line].clusters.clone())
                        .collect::<Vec<Vec<usize>>>()
                })
                .filter(|rows| {
                    rows.iter()
                        .all(|clusters| clusters.iter().all(|cluster| *cluster < shared))
                })
                .collect()
        };
        assert_eq!(blocks(&shortlisted), blocks(&scanned));
        assert_eq!(
            blocks(&shortlisted).len(),
            6,
            "two columns, three paragraphs over each other, and a split run: {:?}",
            blocks(&shortlisted)
        );
        assert_eq!(
            shortlisted.report.lines_refused_as_covering,
            scanned.report.lines_refused_as_covering
        );
        assert!(
            shortlisted.report.lines_refused_as_covering > 0,
            "the covering test has to decide something here"
        );
        assert_eq!(
            shortlisted.report.lines_split_by_column,
            scanned.report.lines_split_by_column
        );
        assert!(shortlisted.report.lines_split_by_column > 0);
        assert_eq!(
            shortlisted.report.lines_split_by_edge,
            scanned.report.lines_split_by_edge
        );
        assert!(shortlisted.report.lines_split_by_edge > 0);
    }

    fn at_size(x: f64, y: f64, em: f64) -> Cell {
        Cell {
            origins: vec![(x, y)],
            widths: vec![600.0],
            matrix: Matrix {
                a: em,
                b: 0.0,
                c: 0.0,
                d: em,
                e: x,
                f: y,
            },
        }
    }

    fn row(x: f64, y: f64, count: usize) -> Vec<Cell> {
        (0..count)
            .map(|step| {
                at(
                    EM.mul_add(f64::from(u32::try_from(step).expect("a small row")), x),
                    y,
                )
            })
            .collect()
    }

    #[test]
    fn a_centred_block_is_set_to_its_middle_and_stays_one_block() {
        let centred = |count: usize| {
            let width = EM * f64::from(u32::try_from(count).expect("a small row"));
            25.0f64.mul_add(EM, -(width / 2.0))
        };
        let mut cells = Vec::new();
        for (step, count) in [20_usize, 10, 16].into_iter().enumerate() {
            let y = 1.5f64.mul_add(-EM * f64::from(u32::try_from(step).expect("three")), 100.0);
            cells.extend(row(centred(count), y, count));
        }
        let index = SemanticIndex::of(&page(&cells));
        assert_eq!(index.lines.len(), 3, "{:?}", index.lines);
        assert_eq!(
            index.blocks.len(),
            1,
            "a centred block is set to its middle: {:?}",
            index.blocks
        );
        assert_eq!(index.report.lines_split_by_edge, 0);

        let mut flush = Vec::new();
        for (step, count) in [20_usize, 10, 16].into_iter().enumerate() {
            let y = 1.5f64.mul_add(-EM * f64::from(u32::try_from(step).expect("three")), 100.0);
            flush.extend(row(0.0, y, count));
        }
        let flush = SemanticIndex::of(&page(&flush));
        assert_eq!(flush.blocks.len(), 1, "{:?}", flush.blocks);

        let mut scattered = Vec::new();
        for (step, count) in [20_usize, 10, 16].into_iter().enumerate() {
            let step = f64::from(u32::try_from(step).expect("three"));
            let y = 1.5f64.mul_add(-EM * step, 100.0);
            scattered.extend(row(20.0 * EM * step, y, count));
        }
        let scattered = SemanticIndex::of(&page(&scattered));
        assert!(
            scattered.blocks.len() > 1,
            "rows set to no edge are not one block: {:?}",
            scattered.blocks
        );
    }

    #[test]
    fn lines_that_run_on_are_one_block_and_a_gap_ends_it() {
        let mut cells = Vec::new();
        for line in 0..3 {
            cells.extend(row(0.0, 100.0 - 1.5 * EM * f64::from(line), 5));
        }
        cells.extend(row(0.0, 100.0 - 1.5 * EM * 2.0 - 2.5 * EM, 5));
        let index = SemanticIndex::of(&page(&cells));

        assert_eq!(index.lines.len(), 4, "four rows");
        assert_eq!(
            index.blocks.len(),
            2,
            "three rows run on, the fourth does not"
        );
        assert_eq!(index.blocks[0].lines, vec![0, 1, 2]);
        assert_eq!(index.blocks[0].evidence, BlockEvidence::RunsOn);
        assert_eq!(index.blocks[1].lines, vec![3]);
        assert_eq!(index.blocks[1].evidence, BlockEvidence::Alone);
        assert_eq!(index.report.blocks_of_one_line, 1);
    }

    #[test]
    fn a_heading_does_not_join_the_body_under_it() {
        let mut cells = vec![at_size(0.0, 100.0, EM * 2.0)];
        cells.extend(row(0.0, 100.0 - EM * 2.0, 5));
        cells.extend(row(0.0, 100.0 - EM * 3.5, 5));
        let index = SemanticIndex::of(&page(&cells));

        assert_eq!(index.lines.len(), 3);
        assert_eq!(index.blocks.len(), 2, "the heading stands alone");
        assert_eq!(index.blocks[0].lines, vec![0]);
        assert_eq!(index.blocks[1].lines, vec![1, 2]);
    }

    fn in_face(graph: &mut PaintGraph, atoms: std::ops::Range<usize>, face: &str) {
        for atom in &mut graph.atoms[atoms] {
            if let PaintAtomKind::Text(text) = &mut atom.kind {
                text.state.text.font = Some(Derived {
                    value: pdf_paint::AppliedFont {
                        name: face.as_bytes().to_vec(),
                        reference: None,
                    },
                    provenance: pdf_paint::Provenance::new(),
                });
            }
        }
    }

    #[test]
    fn a_short_bold_heading_at_the_body_size_stands_alone() {
        let mut cells = row(0.0, 100.0, 3);
        cells.extend(row(0.0, 100.0 - 1.5 * EM, 12));
        cells.extend(row(0.0, 100.0 - 3.0 * EM, 12));
        let mut graph = page(&cells);
        in_face(&mut graph, 0..3, "Bold");
        in_face(&mut graph, 3..27, "Regular");
        let index = SemanticIndex::of(&graph);

        assert_eq!(index.lines.len(), 3);
        assert_eq!(index.blocks.len(), 2, "{:?}", index.blocks);
        assert_eq!(index.blocks[0].lines, vec![0]);
        assert_eq!(index.blocks[1].lines, vec![1, 2]);
        assert!(index.report.lines_split_by_heading > 0, "and counted");
    }

    #[test]
    fn bold_inside_a_paragraph_does_not_make_a_heading() {
        let mut full = row(0.0, 100.0, 12);
        full.extend(row(0.0, 100.0 - 1.5 * EM, 12));
        let mut graph = page(&full);
        in_face(&mut graph, 0..12, "Bold");
        in_face(&mut graph, 12..24, "Regular");
        assert_eq!(SemanticIndex::of(&graph).blocks.len(), 1, "a bold sentence");

        let mut opens = row(0.0, 100.0, 3);
        opens.extend(row(0.0, 100.0 - 1.5 * EM, 12));
        let mut graph = page(&opens);
        in_face(&mut graph, 0..4, "Bold");
        in_face(&mut graph, 4..15, "Regular");
        assert_eq!(
            SemanticIndex::of(&graph).blocks.len(),
            1,
            "a bold first word"
        );

        let mut mixed = row(0.0, 100.0, 3);
        mixed.extend(row(0.0, 100.0 - 1.5 * EM, 12));
        let mut graph = page(&mixed);
        in_face(&mut graph, 0..2, "Bold");
        in_face(&mut graph, 2..15, "Regular");
        assert_eq!(
            SemanticIndex::of(&graph).blocks.len(),
            1,
            "a short line that opens bold and ends regular"
        );

        let mut short = row(0.0, 100.0, 3);
        short.extend(row(0.0, 100.0 - 1.5 * EM, 12));
        let mut graph = page(&short);
        in_face(&mut graph, 0..15, "Regular");
        let index = SemanticIndex::of(&graph);
        assert_eq!(index.blocks.len(), 1, "one face");
        assert_eq!(index.report.lines_split_by_heading, 0);
    }

    #[test]
    fn a_subset_tag_does_not_change_the_face() {
        use super::without_subset_tag as face;
        assert_eq!(face(b"ABCDEF+Times-Bold"), face(b"GHIJKL+Times-Bold"));
        assert_eq!(face(b"ABCDEF+Times-Bold"), b"Times-Bold");
        assert_ne!(face(b"ABCDEF+Times-Bold"), face(b"ABCDEF+Times-Roman"));
        assert_eq!(face(b"Times-Bold"), b"Times-Bold");
        assert_eq!(face(b"abcdef+Times"), b"abcdef+Times", "not a tag");
        assert_eq!(face(b"ABC+Times"), b"ABC+Times", "too short to be a tag");
    }

    #[test]
    fn two_columns_do_not_run_into_each_other() {
        let mut cells = Vec::new();
        for line in 0..3 {
            let y = 100.0 - 1.5 * EM * f64::from(line);
            cells.extend(row(0.0, y, 36));
            cells.extend(row(40.0 * EM, y, 36));
        }
        let index = SemanticIndex::of(&page(&cells));

        assert_eq!(index.lines.len(), 6, "three rows in each of two columns");
        assert_eq!(index.blocks.len(), 2, "a column each");
        for block in &index.blocks {
            assert_eq!(block.lines.len(), 3, "{block:?}");
        }
        assert!(
            index.report.lines_split_by_column > 0,
            "the column rejection must be counted, not silent"
        );
    }

    #[test]
    fn a_short_last_line_still_belongs_to_its_paragraph() {
        let mut cells = row(0.0, 100.0, 12);
        cells.extend(row(0.0, 100.0 - 1.5 * EM, 12));
        cells.extend(row(0.0, 100.0 - 3.0 * EM, 2));
        let index = SemanticIndex::of(&page(&cells));

        assert_eq!(index.lines.len(), 3);
        assert_eq!(index.blocks.len(), 1, "{:?}", index.blocks);
        assert_eq!(index.blocks[0].lines, vec![0, 1, 2]);
    }

    #[test]
    fn cells_on_one_row_do_not_stack_into_a_block() {
        let mut cells = row(0.0, 100.0, 3);
        cells.extend(row(EM * 7.0, 100.0, 3));
        let index = SemanticIndex::of(&page(&cells));

        assert_eq!(index.lines.len(), 2, "a gap made two rows");
        assert_eq!(index.blocks.len(), 2, "and they are two blocks");
    }

    fn with_codes(mut graph: PaintGraph, codes: &[u32]) -> PaintGraph {
        for (atom, code) in graph.atoms.iter_mut().zip(codes) {
            if let PaintAtomKind::Text(text) = &mut atom.kind {
                for glyph in &mut text.glyphs {
                    glyph.code.value = *code;
                }
            }
        }
        graph
    }

    fn block_atoms(index: &SemanticIndex) -> Vec<Vec<usize>> {
        let mut blocks = atoms_by_block(index);
        blocks.sort();
        blocks
    }

    fn paragraph_with_word_between(
        word_painted_between: bool,
        pitch: f64,
        offset: f64,
    ) -> PaintGraph {
        let pitch = pitch * EM;
        let rows: Vec<Vec<Cell>> = (0..3)
            .map(|line| row(0.0, 100.0 - pitch * f64::from(line), 5))
            .collect();
        let word = row(0.0, 100.0 - offset * EM, 7);
        let mut cells = Vec::new();
        let mut rows = rows.into_iter();
        cells.extend(rows.next().expect("row 0"));
        if word_painted_between {
            cells.extend(word);
            cells.extend(rows.next().expect("row 1"));
            cells.extend(rows.next().expect("row 2"));
        } else {
            cells.extend(rows.next().expect("row 1"));
            cells.extend(rows.next().expect("row 2"));
            cells.extend(word);
        }
        page(&cells)
    }

    #[test]
    fn a_word_set_down_inside_a_paragraph_is_told_apart_by_the_order_it_was_painted() {
        let apart = SemanticIndex::of(&paragraph_with_word_between(false, 1.9, 0.95));
        assert_eq!(apart.lines.len(), 4);
        let mut sizes: Vec<usize> = apart.blocks.iter().map(|block| block.lines.len()).collect();
        sizes.sort_unstable();
        assert_eq!(sizes, vec![1, 3], "the paragraph, and the word alone");
        assert_eq!(
            block_atoms(&apart),
            vec![(0..15).collect::<Vec<_>>(), (15..22).collect::<Vec<_>>()]
        );
        assert!(
            apart.report.lines_refused_by_paint_order > 0,
            "and the refusal is counted"
        );
        assert_eq!(
            apart.report.lines_refused_as_covering, 0,
            "nothing covers anything"
        );

        let together = SemanticIndex::of(&paragraph_with_word_between(true, 1.9, 0.95));
        assert_eq!(together.blocks.len(), 1, "{:?}", together.blocks);
        assert_eq!(together.blocks[0].lines.len(), 4);

        for painted_between in [false, true] {
            let over = SemanticIndex::of(&paragraph_with_word_between(painted_between, 1.2, 1.77));
            assert_eq!(over.lines.len(), 4);
            for block in &over.blocks {
                for one in &block.lines {
                    for other in &block.lines {
                        assert!(
                            one == other || !over.rows_cover(*one, *other),
                            "{painted_between}: rows {one} and {other} cover each other in {:?}",
                            over.blocks
                        );
                    }
                }
            }
            assert!(
                over.report.lines_refused_as_covering > 0,
                "{painted_between}"
            );
        }
        let after = SemanticIndex::of(&paragraph_with_word_between(false, 1.2, 1.77));
        assert_eq!(
            block_atoms(&after),
            vec![(0..15).collect::<Vec<_>>(), (15..22).collect::<Vec<_>>()]
        );
    }

    #[test]
    fn two_paragraphs_drawn_on_the_same_baselines_are_two_rows_each_and_two_blocks() {
        let pitch = 1.2 * EM;
        let mut cells = row(0.0, 100.0, 4);
        cells.extend(row(0.0, 100.0 - pitch, 4));
        cells.extend(row(0.0, 100.0, 4));
        cells.extend(row(0.0, 100.0 - pitch, 4));
        let codes: Vec<u32> = (0..16).map(|atom| if atom < 8 { 1 } else { 2 }).collect();
        let index = SemanticIndex::of(&with_codes(page(&cells), &codes));
        assert_eq!(index.lines.len(), 4, "a row per paragraph per baseline");
        assert_eq!(index.report.lines_layered_apart, 2);
        assert_eq!(
            block_atoms(&index),
            vec![(0..8).collect::<Vec<_>>(), (8..16).collect::<Vec<_>>()]
        );

        let mut clear = row(0.0, 100.0, 4);
        clear.extend(row(0.0, 100.0 - pitch, 4));
        clear.extend(row(4.0 * EM, 100.0, 4));
        clear.extend(row(4.0 * EM, 100.0 - pitch, 4));
        let clear = SemanticIndex::of(&with_codes(page(&clear), &codes));
        assert_eq!(clear.lines.len(), 2);
        assert_eq!(clear.report.lines_layered_apart, 0);
        assert_eq!(clear.blocks.len(), 1);
    }

    #[test]
    fn a_line_painted_over_a_watermark_at_another_size_is_its_own_row() {
        let sized = |x: f64, y: f64, size: f64| Cell {
            origins: vec![(x, y)],
            widths: vec![600.0],
            matrix: Matrix {
                a: size,
                b: 0.0,
                c: 0.0,
                d: size,
                e: x,
                f: y,
            },
        };
        let big: Vec<Cell> = (0..4)
            .map(|step| sized(36.0 * f64::from(step), 100.0, 36.0))
            .collect();
        let mut over = big;
        over.extend((0..8).map(|step| sized(3.0 + EM * f64::from(step), 98.0, EM)));
        let index = SemanticIndex::of(&page(&over));
        let mut row_lengths: Vec<usize> =
            index.lines.iter().map(|line| line.clusters.len()).collect();
        row_lengths.sort_unstable();
        assert_eq!(row_lengths, vec![4, 8], "{:?}", index.lines);

        let mut clear: Vec<Cell> = (0..4)
            .map(|step| sized(36.0 * f64::from(step), 100.0, 36.0))
            .collect();
        clear.extend((0..8).map(|step| sized(160.0 + EM * f64::from(step), 98.0, EM)));
        let clear = SemanticIndex::of(&page(&clear));
        assert_eq!(clear.lines.len(), 1, "{:?}", clear.lines);
    }

    #[test]
    fn rows_drawn_over_each_other_are_never_one_block() {
        let turned = |x: f64, y: f64, angle: f64| {
            let (sin, cos) = angle.to_radians().sin_cos();
            Cell {
                origins: vec![(x, y)],
                widths: vec![600.0],
                matrix: Matrix {
                    a: EM * cos,
                    b: EM * sin,
                    c: -EM * sin,
                    d: EM * cos,
                    e: x,
                    f: y,
                },
            }
        };
        let rows = |pitch: f64, angle: f64| {
            let (sin, cos) = angle.to_radians().sin_cos();
            let cells: Vec<Cell> = (0..2)
                .flat_map(|line| {
                    (0..5).map(move |step| {
                        let along = EM * f64::from(step);
                        let down = pitch * f64::from(line);
                        (
                            along.mul_add(cos, down * sin),
                            along.mul_add(sin, -down * cos),
                        )
                    })
                })
                .map(|(dx, dy)| turned(100.0 + dx, 300.0 + dy, angle))
                .collect();
            SemanticIndex::of(&page(&cells))
        };
        for angle in [0.0, 30.0] {
            let over = rows(0.5 * EM, angle);
            assert_eq!(over.lines.len(), 2, "{angle}: {:?}", over.lines);
            assert_eq!(over.blocks.len(), 2, "{angle}: {:?}", over.blocks);
            assert!(
                over.report.lines_refused_as_covering > 0,
                "{angle}: and counted"
            );

            let apart = rows(1.2 * EM, angle);
            assert_eq!(apart.blocks.len(), 1, "{angle}: {:?}", apart.blocks);
            assert_eq!(apart.report.lines_refused_as_covering, 0);
        }
    }

    #[test]
    fn text_a_form_paints_is_never_in_a_row_or_block_with_the_pages_own() {
        let pitch = 1.2 * EM;
        let cells = |formed: bool| {
            let mut cells = row(0.0, 100.0, 4);
            cells.extend(row(4.5 * EM, 100.0, 2));
            cells.extend(row(0.0, 100.0 - pitch, 4));
            cells.extend(row(4.5 * EM, 100.0 - pitch, 2));
            let mut graph = page(&cells);
            if formed {
                for atom in graph
                    .atoms
                    .iter_mut()
                    .filter(|atom| matches!(atom.id.ordinal, 4 | 5 | 10 | 11))
                {
                    atom.id.invocation_path = vec![pdf_paint::FormInvocation {
                        form: Reference::new(9, 0),
                        operator_span: span(),
                    }];
                }
            }
            SemanticIndex::of(&graph)
        };
        let formed = cells(true);
        assert_eq!(formed.lines.len(), 4, "{:?}", formed.lines);
        assert!(formed.report.lines_split_by_content > 0);
        assert_eq!(
            block_atoms(&formed),
            vec![vec![0, 1, 2, 3, 6, 7, 8, 9], vec![4, 5], vec![10, 11]]
        );

        let plain = cells(false);
        assert_eq!(plain.lines.len(), 2, "{:?}", plain.lines);
        assert_eq!(plain.blocks.len(), 1);
        assert_eq!(plain.report.lines_split_by_content, 0);

        let stacked = |formed: bool| {
            let mut cells = row(0.0, 100.0, 4);
            cells.extend(row(0.0, 100.0 - pitch, 4));
            let mut graph = page(&cells);
            if formed {
                for atom in &mut graph.atoms[4..] {
                    atom.id.invocation_path = vec![pdf_paint::FormInvocation {
                        form: Reference::new(9, 0),
                        operator_span: span(),
                    }];
                }
            }
            SemanticIndex::of(&graph)
        };
        assert_eq!(stacked(true).blocks.len(), 2);
        assert_eq!(stacked(false).blocks.len(), 1);
    }

    #[test]
    fn a_mark_with_its_own_advance_drawn_back_over_its_base_is_not_a_second_layer() {
        let base = at(0.0, 100.0);
        let mark = Cell {
            origins: vec![(2.0, 100.0)],
            widths: vec![200.0],
            matrix: Matrix {
                a: EM,
                b: 0.0,
                c: 0.0,
                d: EM,
                e: 2.0,
                f: 100.0,
            },
        };
        let mut cells = vec![base, at(EM, 100.0)];
        cells.push(at(0.0, 400.0));
        cells.push(mark);
        let index = SemanticIndex::of(&page(&cells));
        let on_the_row = index
            .lines
            .iter()
            .filter(|line| index.clusters[line.clusters[0]].baseline.y < 200.0)
            .count();
        assert_eq!(on_the_row, 1, "the mark stays on its base's row");
        assert_eq!(index.report.lines_layered_apart, 0);
    }

    #[test]
    fn a_word_painted_again_at_once_is_bold_and_painted_again_later_is_a_copy() {
        let word = |dx: f64| row(dx, 100.0, 3);
        let mut bold = word(0.0);
        bold.push(at(0.0, 400.0));
        bold.extend(word(0.05 * EM));
        let bold = SemanticIndex::of(&with_codes(page(&bold), &[1, 2, 3, 9, 1, 2, 3]));
        assert_eq!(
            bold.lines.len(),
            2,
            "fake bold is one row, the letter above another"
        );
        assert_eq!(bold.report.lines_layered_apart, 0);

        let mut copy = word(0.0);
        copy.extend(row(0.0, 400.0, 6));
        copy.extend(word(0.05 * EM));
        let copy = SemanticIndex::of(&with_codes(
            page(&copy),
            &[1, 2, 3, 9, 9, 9, 9, 9, 9, 1, 2, 3],
        ));
        let on_the_word: Vec<&super::Line> = copy
            .lines
            .iter()
            .filter(|line| copy.clusters[line.clusters[0]].baseline.y < 200.0)
            .collect();
        assert_eq!(
            on_the_word.len(),
            2,
            "a copy painted later is a row of its own"
        );
        assert_eq!(copy.report.lines_layered_apart, 1);
    }

    #[test]
    fn a_citation_painted_between_rows_does_not_break_the_paragraph() {
        let pitch = 0.95 * EM;
        let mut cells = row(0.0, 100.0, 5);
        cells.push(at_size(2.0 * EM, 100.0 + EM / 3.0, 0.65 * EM));
        cells.extend(row(0.0, 100.0 - pitch, 5));
        cells.extend(row(0.0, 100.0 - 2.0 * pitch, 5));
        let index = SemanticIndex::of(&page(&cells));
        assert!(
            index.blocks.iter().any(|b| b.lines.len() == 3),
            "the three rows are one paragraph: {:?}",
            index.blocks
        );
    }

    #[test]
    fn a_mark_far_above_its_row_does_not_put_the_rows_out_of_order() {
        let pitch = 1.5 * EM;
        let mut cells = row(0.0, 100.0, 5);
        cells.extend(row(0.0, 100.0 - pitch, 5));
        cells.extend(row(0.0, 100.0 - 2.0 * pitch, 5));
        let raised = 100.0 - pitch + 1.8 * EM;
        cells.push(Cell {
            origins: vec![(EM, raised)],
            widths: vec![0.0],
            matrix: Matrix {
                a: EM,
                b: 0.0,
                c: 0.0,
                d: EM,
                e: EM,
                f: raised,
            },
        });
        let index = SemanticIndex::of(&page(&cells));
        assert_eq!(index.lines.len(), 4, "{:?}", index.lines);
        for block in &index.blocks {
            assert!(
                block.lines.windows(2).all(|pair| pair[0] < pair[1]),
                "a block's rows are listed top to bottom: {:?}",
                index.blocks
            );
        }
        assert_eq!(index.blocks[0].lines, vec![0, 1, 2], "{:?}", index.blocks);
        assert_eq!(index.blocks[1].lines, vec![3], "{:?}", index.blocks);
        let again = SemanticIndex::of_grouped(&page(&cells), &super::Grouping::of(&index));
        let rows = |index: &SemanticIndex| -> Vec<Vec<Vec<usize>>> {
            index
                .blocks
                .iter()
                .map(|block| {
                    block
                        .lines
                        .iter()
                        .map(|line| index.lines[*line].clusters.clone())
                        .collect()
                })
                .collect()
        };
        assert_eq!(rows(&index), rows(&again));
    }

    #[test]
    fn dots_of_a_row_painted_after_the_next_row_stay_in_the_paragraph() {
        let pitch = 1.2 * EM;
        let dot = |x: f64, y: f64| Cell {
            origins: vec![(x, y)],
            widths: vec![0.0],
            matrix: Matrix {
                a: EM,
                b: 0.0,
                c: 0.0,
                d: EM,
                e: x,
                f: y,
            },
        };
        let mut cells = row(0.0, 100.0, 5);
        cells.extend(row(0.0, 100.0 - pitch, 5));
        cells.push(dot(EM, 100.0 - pitch - 0.3 * EM));
        cells.push(dot(EM, 100.0 - 0.4 * EM));
        cells.extend(row(0.0, 100.0 - pitch - 1.3 * pitch, 5));
        let index = SemanticIndex::of(&page(&cells));
        assert_eq!(index.lines.len(), 5);
        let biggest = index.blocks.iter().map(|b| b.lines.len()).max();
        assert!(
            index
                .blocks
                .iter()
                .any(|b| b.lines.contains(&0) && b.lines.contains(&4)),
            "the first and third rows are one paragraph: {:?} (largest {biggest:?})",
            index.blocks
        );
    }

    #[test]
    fn labels_and_values_on_tight_rows_are_a_block_each() {
        let mut cells = Vec::new();
        for line in 0..3 {
            let y = 100.0 - 0.9 * EM * f64::from(line);
            cells.extend(row(0.0, y, 3));
            cells.extend(row(8.0 * EM, y, 4));
        }
        let index = SemanticIndex::of(&page(&cells));
        assert_eq!(index.lines.len(), 6, "every row whole");
        let sizes: Vec<usize> = index.blocks.iter().map(|b| b.lines.len()).collect();
        assert_eq!(sizes, vec![1; 6], "a label or a value each");
    }

    #[test]
    fn a_column_of_short_cells_is_a_block_each() {
        let mut cells = Vec::new();
        for line in 0..3 {
            let y = 100.0 - 1.5 * EM * f64::from(line);
            for column in 0..3 {
                cells.extend(row(10.0 * EM * f64::from(column), y, 3));
            }
        }
        let index = SemanticIndex::of(&page(&cells));
        assert_eq!(index.lines.len(), 9);
        assert_eq!(index.blocks.len(), 9, "{:?}", index.blocks);
        assert!(index.report.lines_split_as_cells > 0, "and counted");

        let mut cells = Vec::new();
        for line in 0..2 {
            let y = 100.0 - 1.5 * EM * f64::from(line);
            cells.extend(row(0.0, y, 3));
            cells.extend(row(8.0 * EM, y, 12));
            cells.extend(row(40.0 * EM, y, 3));
        }
        let index = SemanticIndex::of(&page(&cells));
        assert_eq!(index.lines.len(), 6);
        let sizes: Vec<usize> = index.blocks.iter().map(|b| b.lines.len()).collect();
        assert_eq!(
            sizes,
            vec![1; 6],
            "the wide middle cells too: {:?}",
            index.blocks
        );
    }

    #[test]
    fn a_short_line_is_a_cell_only_beside_another_short_line() {
        let mut cells = Vec::new();
        for line in 0..3 {
            let y = 100.0 - 1.5 * EM * f64::from(line);
            let count = if line == 2 { 4 } else { 36 };
            cells.extend(row(0.0, y, count));
            cells.extend(row(40.0 * EM, y, 36));
        }
        let index = SemanticIndex::of(&page(&cells));
        let sizes: Vec<usize> = index.blocks.iter().map(|b| b.lines.len()).collect();
        assert_eq!(sizes, vec![3, 3], "{:?}", index.blocks);

        let mut alone = row(0.0, 100.0, 3);
        alone.extend(row(0.0, 100.0 - 1.5 * EM, 3));
        let index = SemanticIndex::of(&page(&alone));
        assert_eq!(index.blocks.len(), 1, "short rows with nothing beside them");
    }

    #[test]
    fn marks_nudged_above_and_below_a_row_stay_on_it() {
        let mark = |x: f64, y: f64| Cell {
            origins: vec![(x, y)],
            widths: vec![0.0],
            matrix: Matrix {
                a: EM,
                b: 0.0,
                c: 0.0,
                d: EM,
                e: x,
                f: y,
            },
        };
        let mut cells = row(0.0, 100.0, 2);
        cells.push(mark(EM, 102.0));
        cells.extend(row(2.0 * EM, 100.0, 2));
        cells.push(mark(3.0 * EM, 97.5));
        cells.extend(row(4.0 * EM, 100.0, 2));
        let index = SemanticIndex::of(&page(&cells));
        assert_eq!(index.lines.len(), 1, "one row: {:?}", index.lines);
        assert_eq!(index.blocks.len(), 1);
    }

    #[test]
    fn a_mark_lifted_off_its_row_joins_the_letter_it_was_painted_on() {
        let mark = |x: f64, y: f64| Cell {
            origins: vec![(x, y)],
            widths: vec![0.0],
            matrix: Matrix {
                a: EM,
                b: 0.0,
                c: 0.0,
                d: EM,
                e: x,
                f: y,
            },
        };
        let arranged = |elsewhere: bool, at: (f64, f64)| {
            let mut cells = row(0.0, 100.0, 2);
            if elsewhere {
                cells.extend(row(0.0, 70.0, 2));
            }
            cells.push(mark(at.0, at.1));
            cells.extend(row(24.0, 100.0, 2));
            SemanticIndex::of(&page(&cells))
        };
        let joined = arranged(false, (14.0, 104.0));
        assert_eq!(joined.lines.len(), 1, "{:?}", joined.lines);
        assert_eq!(joined.blocks.len(), 1);
        assert_eq!(joined.report.marks_lifted_onto_their_row, 1);
        for (elsewhere, at, rows) in [
            (true, (14.0, 104.0), 3),
            (false, (14.0, 113.0), 2),
            (false, (60.0, 104.0), 2),
        ] {
            let apart = arranged(elsewhere, at);
            assert_eq!(
                apart.lines.len(),
                rows,
                "control {elsewhere} {at:?}: {:?}",
                apart.lines
            );
            assert_eq!(apart.report.marks_lifted_onto_their_row, 0);
        }
    }

    #[test]
    fn rows_a_gap_ended_on_one_baseline_of_one_block_are_one_row() {
        let mut cells = row(0.0, 100.0, 10);
        cells.push(Cell {
            origins: vec![(107.0, 100.0)],
            widths: vec![0.0],
            matrix: Matrix {
                a: EM,
                b: 0.0,
                c: 0.0,
                d: EM,
                e: 107.0,
                f: 100.0,
            },
        });
        cells.extend(row(40.0 * EM, 100.0, 5));
        cells.extend(row(0.0, 100.0 - 1.5 * EM, 50));
        let index = SemanticIndex::of(&page(&cells));
        assert_eq!(index.blocks.len(), 1, "{:?}", index.blocks);
        assert_eq!(index.lines.len(), 2, "{:?}", index.lines);
        assert_eq!(index.lines[0].clusters.len(), 16);
        assert_eq!(
            index.lines[0].clusters[..12],
            (0..12).collect::<Vec<_>>()[..]
        );
        assert_eq!(index.report.rows_joined_on_one_baseline, 1);

        let mut stepped = row(0.0, 100.0, 10);
        stepped.extend(row(0.0, 100.0 - 1.5 * EM, 50));
        stepped.extend(row(40.0 * EM, 100.0 - 3.0 * EM, 5));
        let stepped = SemanticIndex::of(&page(&stepped));
        assert_eq!(stepped.lines.len(), 3);
        assert_eq!(stepped.report.rows_joined_on_one_baseline, 0);
        assert_eq!(
            stepped
                .blocks
                .iter()
                .map(|block| block.lines.clone())
                .collect::<Vec<_>>(),
            vec![vec![0, 1], vec![2]],
            "the row set 40 ems to the right is not part of the paragraph"
        );
        assert_eq!(stepped.report.lines_split_by_edge, 1);
        let pitch = 1.2 * EM;
        let codes: Vec<u32> = (0..16).map(|atom| if atom < 8 { 1 } else { 2 }).collect();
        let mut clear = row(0.0, 100.0, 4);
        clear.extend(row(0.0, 100.0 - pitch, 4));
        clear.extend(row(4.0 * EM, 100.0, 4));
        clear.extend(row(4.0 * EM, 100.0 - pitch, 4));
        let grouping = super::Grouping::of(&SemanticIndex::of(&with_codes(page(&clear), &codes)));
        let mut over = row(0.0, 100.0, 4);
        over.extend(row(0.0, 100.0 - pitch, 4));
        over.extend(row(0.0, 100.0, 4));
        over.extend(row(0.0, 100.0 - pitch, 4));
        let over = SemanticIndex::of_grouped(&with_codes(page(&over), &codes), &grouping);
        assert_eq!(over.blocks.len(), 1);
        assert_eq!(over.lines.len(), 4, "{:?}", over.lines);
        assert_eq!(over.report.rows_joined_on_one_baseline, 0);

        let mut columns = row(0.0, 100.0, 10);
        columns.extend(row(40.0 * EM, 100.0, 5));
        let apart = SemanticIndex::of(&page(&columns));
        assert_eq!(apart.lines.len(), 2);
        assert_eq!(apart.blocks.len(), 2);
        assert_eq!(apart.report.rows_joined_on_one_baseline, 0);
    }

    #[test]
    fn rows_set_tighter_than_an_em_are_still_one_paragraph() {
        let mut cells = Vec::new();
        for line in 0..3 {
            cells.extend(row(0.0, 100.0 - 0.9 * EM * f64::from(line), 5));
        }
        let index = SemanticIndex::of(&page(&cells));
        assert_eq!(index.lines.len(), 3);
        assert_eq!(index.blocks.len(), 1, "{:?}", index.blocks);
        assert_eq!(index.report.lines_refused_by_paint_order, 0);
    }

    #[test]
    fn a_superscript_painted_in_its_row_stays_in_that_paragraph() {
        let cells = vec![
            at(0.0, 100.0),
            at(EM, 100.0),
            at(2.0 * EM, 100.0 + EM / 3.0),
            at(3.0 * EM, 100.0),
            at(0.0, 100.0 - 1.2 * EM),
            at(EM, 100.0 - 1.2 * EM),
            at(2.0 * EM, 100.0 - 1.2 * EM),
            at(3.0 * EM, 100.0 - 1.2 * EM),
        ];
        let index = SemanticIndex::of(&page(&cells));
        assert_eq!(index.lines.len(), 3, "the superscript is its own row");
        assert_eq!(index.blocks.len(), 1, "{:?}", index.blocks);
    }

    #[test]
    fn three_paragraphs_laid_over_each_other_are_three_blocks() {
        let pitch = 1.2 * EM;
        let mut cells = Vec::new();
        for offset in [0.0, 0.4 * EM, 0.8 * EM] {
            for line in 0..3 {
                cells.extend(row(0.0, 100.0 - offset - pitch * f64::from(line), 5));
            }
        }
        let index = SemanticIndex::of(&page(&cells));
        assert_eq!(index.lines.len(), 9);
        assert_eq!(
            block_atoms(&index),
            vec![
                (0..15).collect::<Vec<_>>(),
                (15..30).collect::<Vec<_>>(),
                (30..45).collect::<Vec<_>>()
            ]
        );
    }

    #[test]
    fn rows_with_another_blocks_row_between_them_are_not_one_paragraph() {
        let text_em = 16.0;
        let pitch = 1.2 * text_em;
        let build = |text_x: f64, size: f64| {
            let mut cells = Vec::new();
            for line in 0..3 {
                cells.extend((0..30).map(|step| {
                    at_size(
                        text_x + size * f64::from(step),
                        300.0 - pitch * f64::from(line),
                        size,
                    )
                }));
            }
            cells.extend(row(12.0 * EM, 300.0 - pitch + pitch / 2.0, 3));
            cells.extend(row(12.0 * EM, 300.0 - pitch - pitch / 2.0, 3));
            SemanticIndex::of(&page(&cells))
        };
        let between = build(0.0, text_em);
        assert_eq!(between.lines.len(), 5);
        assert_eq!(
            block_atoms(&between),
            vec![
                (0..90).collect::<Vec<_>>(),
                (90..93).collect::<Vec<_>>(),
                (93..96).collect::<Vec<_>>()
            ],
            "{:?}",
            between.blocks
        );
        assert!(between.report.lines_split_by_text_between > 0);

        let aside = build(40.0 * EM, text_em);
        assert!(
            block_atoms(&aside).contains(&(90..96).collect::<Vec<_>>()),
            "{:?}",
            aside.blocks
        );
        assert_eq!(aside.report.lines_split_by_text_between, 0);

        let one_size = build(0.0, EM);
        assert_eq!(one_size.report.lines_split_by_text_between, 0);
    }

    #[test]
    fn a_row_where_a_bracket_closes_over_one_where_another_opens_is_another_formula() {
        let meanings = std::sync::Arc::new(
            pdf_font::ToUnicode::from_cmap(
                &pdf_bytes::ByteStore::new(
                    SourceId::new(10),
                    std::sync::Arc::<[u8]>::from(
                        &b"1 begincodespacerange <00> <ff> endcodespacerange \
                           3 beginbfchar <01> <23A3> <02> <23A1> <03> <0061> endbfchar"[..],
                    ),
                ),
                pdf_font::CMapLimits::default(),
            )
            .expect("the map reads"),
        );
        let two_rows = |upper: [u32; 2], lower: [u32; 2]| {
            let mut cells = row(0.0, 300.0, 2);
            cells.extend(row(0.0, 300.0 - 1.2 * EM, 2));
            let mut graph = page(&cells);
            for (atom, value) in graph.atoms.iter_mut().zip(upper.into_iter().chain(lower)) {
                if let PaintAtomKind::Text(text) = &mut atom.kind {
                    text.text = std::sync::Arc::clone(&meanings);
                    for glyph in &mut text.glyphs {
                        glyph.code.value = value;
                        glyph.code.bytes = vec![u8::try_from(value).expect("a byte")];
                    }
                }
            }
            SemanticIndex::of(&graph)
        };
        let apart = two_rows([3, 1], [2, 3]);
        assert_eq!(apart.lines.len(), 2);
        assert_eq!(apart.blocks.len(), 2, "{:?}", apart.blocks);
        assert_eq!(apart.report.lines_split_by_delimiters, 1);

        let one = two_rows([3, 2], [1, 3]);
        assert_eq!(one.blocks.len(), 1, "{:?}", one.blocks);
        assert_eq!(one.report.lines_split_by_delimiters, 0);
    }

    #[test]
    fn a_row_off_the_paragraphs_pitch_is_asked_how_it_was_painted() {
        let paragraph_then = |below: f64| {
            let mut cells = row(0.0, 300.0, 5);
            cells.extend(row(0.0, 300.0 - EM, 5));
            cells.extend(row(0.0, 300.0 - 2.0 * EM, 5));
            cells.extend(row(400.0, 700.0, 5));
            cells.extend(row(0.0, 300.0 - 2.0 * EM - below, 5));
            SemanticIndex::of(&page(&cells))
        };
        let on = paragraph_then(2.0 * EM);
        assert_eq!(on.blocks.iter().map(|b| b.lines.len()).max(), Some(4));
        assert_eq!(on.report.lines_refused_by_paint_order, 0);
        let off = paragraph_then(1.5 * EM);
        assert_eq!(off.blocks.iter().map(|b| b.lines.len()).max(), Some(3));
        assert_eq!(off.report.lines_refused_by_paint_order, 1);
    }

    #[test]
    fn an_interval_inside_another_overlaps_it_completely() {
        assert!((overlap((0.0, 10.0), (0.0, 10.0)) - 1.0).abs() < 1e-9);
        assert!((overlap((0.0, 10.0), (2.0, 4.0)) - 1.0).abs() < 1e-9);
        assert!((overlap((0.0, 10.0), (5.0, 15.0)) - 0.5).abs() < 1e-9);
        assert!(
            overlap((0.0, 10.0), (20.0, 30.0)).abs() < 1e-9,
            "no overlap at all"
        );
        assert!(
            (overlap((0.0, 10.0), (5.0, 5.0)) - 1.0).abs() < 1e-9,
            "a point inside an interval is inside it"
        );
    }

    #[test]
    fn characters_painted_one_show_operation_at_a_time_become_one_line() {
        let cells = [at(0.0, 100.0), at(10.0, 100.0), at(20.0, 100.0)];
        let graph = page(&cells);
        let index = SemanticIndex::of(&graph);
        assert_eq!(index.clusters.len(), 3);
        assert_eq!(index.lines.len(), 1, "three fragments, one row");
        assert_eq!(index.lines[0].clusters, vec![0, 1, 2]);
    }

    #[test]
    fn two_baselines_are_two_lines() {
        let cells = [at(0.0, 100.0), at(10.0, 100.0), at(0.0, 80.0)];
        let graph = page(&cells);
        let index = SemanticIndex::of(&graph);
        assert_eq!(index.lines.len(), 2);
        assert_eq!(index.lines[0].clusters.len(), 2, "the upper row first");
        assert_eq!(index.lines[1].clusters.len(), 1);
    }

    #[test]
    fn a_column_gutter_ends_the_line_and_the_split_is_counted() {
        let cells = [at(0.0, 100.0), at(10.0, 100.0), at(300.0, 100.0)];
        let graph = page(&cells);
        let index = SemanticIndex::of(&graph);
        assert_eq!(index.lines.len(), 2, "the gutter ends the row");
        assert_eq!(index.report.lines_split_by_gap, 1);
        let close = [at(0.0, 100.0), at(10.0, 100.0), at(22.0, 100.0)];
        let tight = SemanticIndex::of(&page(&close));
        assert_eq!(tight.lines.len(), 1);
        assert_eq!(tight.report.lines_split_by_gap, 0);
    }

    #[test]
    fn a_line_is_ordered_by_where_the_pen_goes_not_by_paint_order() {
        let cells = [at(20.0, 100.0), at(0.0, 100.0), at(10.0, 100.0)];
        let graph = page(&cells);
        let index = SemanticIndex::of(&graph);
        assert_eq!(index.lines.len(), 1);
        assert_eq!(index.lines[0].clusters, vec![1, 2, 0]);
    }

    #[test]
    fn a_rotated_run_does_not_join_an_upright_one_it_crosses() {
        let rotate = Matrix {
            a: 0.0,
            b: 1.0,
            c: -1.0,
            d: 0.0,
            e: 0.0,
            f: 100.0,
        };
        let graph = PaintGraph {
            object_scopes: Vec::new(),
            repairs: Vec::new(),
            skipped: Vec::new(),
            atoms: vec![
                PaintAtom {
                    id: PaintId {
                        page: Reference::new(1, 0),
                        stream: Reference::new(2, 0),
                        operator_span: span(),
                        invocation_path: Vec::new(),
                        pattern_path: Vec::new(),
                        ordinal: 0,
                    },
                    kind: PaintAtomKind::Text(run(
                        &[(0.0, 100.0)],
                        &[600.0],
                        Matrix {
                            a: 1.0,
                            b: 0.0,
                            c: 0.0,
                            d: 1.0,
                            e: 0.0,
                            f: 100.0,
                        },
                    )),
                    marks: Vec::new(),
                },
                PaintAtom {
                    id: PaintId {
                        page: Reference::new(1, 0),
                        stream: Reference::new(2, 0),
                        operator_span: span(),
                        invocation_path: Vec::new(),
                        pattern_path: Vec::new(),
                        ordinal: 1,
                    },
                    kind: PaintAtomKind::Text(run(&[(0.0, 100.0)], &[600.0], rotate)),
                    marks: Vec::new(),
                },
            ],
        };
        let index = SemanticIndex::of(&graph);
        assert_eq!(
            index.lines.len(),
            2,
            "different directions are different rows"
        );
    }

    #[test]
    fn a_superscript_is_not_on_the_line_it_sits_above() {
        let cells = [at(0.0, 100.0), at(10.0, 100.0 + EM / 3.0), at(20.0, 100.0)];
        let graph = page(&cells);
        let index = SemanticIndex::of(&graph);
        assert_eq!(index.lines.len(), 2);
        let noisy = [
            at(0.0, 100.0),
            at(10.0, 100.0 + EM / 100.0),
            at(20.0, 100.0),
        ];
        let steady = SemanticIndex::of(&page(&noisy));
        assert_eq!(steady.lines.len(), 1);
    }

    #[test]
    fn every_cluster_belongs_to_exactly_one_line() {
        let cells = [
            at(0.0, 100.0),
            at(10.0, 100.0),
            at(300.0, 100.0),
            at(0.0, 80.0),
        ];
        let graph = page(&cells);
        let index = SemanticIndex::of(&graph);
        let mut seen: Vec<usize> = index
            .lines
            .iter()
            .flat_map(|line| line.clusters.iter().copied())
            .collect();
        seen.sort_unstable();
        assert_eq!(seen, (0..index.clusters.len()).collect::<Vec<_>>());
    }

    fn one_run_row(origins: &[(f64, f64)]) -> PaintGraph {
        page(&[Cell {
            origins: origins.to_vec(),
            widths: vec![600.0; origins.len()],
            matrix: Matrix {
                a: EM,
                b: 0.0,
                c: 0.0,
                d: EM,
                e: origins[0].0,
                f: origins[0].1,
            },
        }])
    }

    fn two_paragraphs(apart: f64) -> PaintGraph {
        page(&[
            at(0.0, 300.0),
            at(10.0, 300.0),
            at(0.0, 280.0),
            at(10.0, 280.0),
            at(0.0, 280.0 - apart),
            at(10.0, 280.0 - apart),
            at(0.0, 260.0 - apart),
            at(10.0, 260.0 - apart),
        ])
    }

    fn atoms_by_block(index: &SemanticIndex) -> Vec<Vec<usize>> {
        index
            .blocks
            .iter()
            .map(|block| {
                let mut atoms: Vec<usize> = block
                    .lines
                    .iter()
                    .flat_map(|line| index.lines[*line].clusters.iter())
                    .map(|cluster| index.clusters[*cluster].atom)
                    .collect();
                atoms.sort_unstable();
                atoms.dedup();
                atoms
            })
            .collect()
    }

    #[test]
    fn the_geometry_alone_merges_a_paragraph_dropped_onto_another() {
        let apart = SemanticIndex::of(&two_paragraphs(200.0));
        assert_eq!(apart.blocks.len(), 2, "two paragraphs, far apart");
        let together = SemanticIndex::of(&two_paragraphs(20.0));
        assert_eq!(
            together.blocks.len(),
            1,
            "the same eight runs, one paragraph's leading apart, inferred as one block"
        );
    }

    #[test]
    fn a_grouping_keeps_a_paragraph_dropped_onto_another_apart_from_it() {
        let before = SemanticIndex::of(&two_paragraphs(200.0));
        let grouping = super::Grouping::of(&before);
        assert_eq!(grouping.block_count(), 2);

        let after = SemanticIndex::of_grouped(&two_paragraphs(20.0), &grouping);
        assert_eq!(after.blocks.len(), 2, "still two paragraphs");
        assert_eq!(
            atoms_by_block(&after),
            atoms_by_block(&before),
            "each block holds the runs it went in with"
        );
        assert_eq!(after.report.clusters_outside_the_grouping, 0);
        assert!(
            after
                .blocks
                .iter()
                .all(|block| block.evidence == BlockEvidence::Kept),
            "and says that is why"
        );
    }

    #[test]
    fn a_grouping_keeps_the_rows_of_two_paragraphs_apart_as_well() {
        let before = SemanticIndex::of(&two_paragraphs(200.0));
        let grouping = super::Grouping::of(&before);
        let after = SemanticIndex::of_grouped(&two_paragraphs(20.0), &grouping);
        for block in &after.blocks {
            assert_eq!(block.lines.len(), 2, "two rows each, as they were");
        }
        let mut lines: Vec<usize> = after.blocks.iter().flat_map(|b| b.lines.clone()).collect();
        lines.sort_unstable();
        lines.dedup();
        assert_eq!(lines.len(), 4, "and no row belongs to two blocks");
    }

    #[test]
    fn text_the_grouping_never_saw_is_grouped_and_counted_rather_than_dropped() {
        let before = SemanticIndex::of(&two_paragraphs(200.0));
        let grouping = super::Grouping::of(&before);
        let mut atoms = two_paragraphs(200.0);
        let more = page(&[at(0.0, 20.0), at(10.0, 20.0)]);
        atoms.atoms.extend(more.atoms);
        let after = SemanticIndex::of_grouped(&atoms, &grouping);
        assert_eq!(
            after.report.clusters_outside_the_grouping, 2,
            "the two new clusters are named as strangers"
        );
        assert_eq!(
            after.blocks.len(),
            3,
            "and grouped into a block of their own"
        );
        assert_eq!(after.blocks[2].evidence, BlockEvidence::Alone);
        assert_eq!(atoms_by_block(&after)[2], vec![8, 9]);
    }

    #[test]
    fn a_block_whose_text_is_gone_keeps_its_place_in_the_numbering() {
        let before = SemanticIndex::of(&two_paragraphs(200.0));
        let grouping = super::Grouping::of(&before);
        let mut left = two_paragraphs(200.0);
        left.atoms.truncate(4);
        let after = SemanticIndex::of_grouped(&left, &grouping);
        assert_eq!(after.blocks.len(), 2, "the empty block still counts");
        assert_eq!(atoms_by_block(&after)[0], vec![0, 1, 2, 3]);
        assert!(after.blocks[1].lines.is_empty());
        assert_eq!(after.blocks[1].bounds, None);
        assert_eq!(after.report.clusters_outside_the_grouping, 0);
    }

    fn two_rows() -> PaintGraph {
        page(&[
            at(0.0, 100.0),
            at(10.0, 100.0),
            at(20.0, 100.0),
            at(0.0, 80.0),
            at(10.0, 80.0),
            at(20.0, 80.0),
        ])
    }

    #[test]
    fn a_caret_walks_the_row_one_cluster_at_a_time() {
        let index = SemanticIndex::of(&two_rows());
        let mut caret = Caret { line: 0, offset: 0 };
        for expected in 1..=3 {
            caret = index.caret_right(caret);
            assert_eq!(caret.offset, expected);
            assert_eq!(caret.line, 0, "still on the first row");
        }
    }

    #[test]
    fn a_caret_crosses_to_the_next_row_at_the_end_and_back_at_the_start() {
        let index = SemanticIndex::of(&two_rows());
        let end = Caret { line: 0, offset: 3 };
        let crossed = index.caret_right(end);
        assert_eq!(crossed, Caret { line: 1, offset: 0 });
        assert_eq!(index.caret_left(crossed), Caret { line: 0, offset: 3 });
    }

    #[test]
    fn a_caret_stops_at_the_ends_of_the_page_rather_than_wrapping() {
        let index = SemanticIndex::of(&two_rows());
        let first = Caret { line: 0, offset: 0 };
        assert_eq!(index.caret_left(first), first);
        assert_eq!(index.caret_up(first), first);
        let last = Caret { line: 1, offset: 3 };
        assert_eq!(index.caret_right(last), last);
        assert_eq!(index.caret_down(last), last);
    }

    #[test]
    fn moving_up_keeps_the_caret_where_it_was_on_the_page() {
        let index = SemanticIndex::of(&page(&[
            at(0.0, 100.0),
            at(10.0, 100.0),
            at(20.0, 100.0),
            at(20.0, 80.0),
        ]));
        let below = Caret { line: 1, offset: 1 };
        let above = index.caret_up(below);
        assert_eq!(above.line, 0);
        assert_eq!(
            above.offset, 3,
            "carrying the index would have given offset 1, at x = 10, sliding \
             the caret two characters left for the crime of moving up"
        );
        let from = index.caret_point(below.line, below.offset);
        let to = index.caret_point(above.line, above.offset);
        assert!((from.x - to.x).abs() < 1e-9, "{from:?} vs {to:?}");
    }

    #[test]
    fn a_click_lands_on_the_nearest_gap_of_the_nearest_row() {
        let index = SemanticIndex::of(&two_rows());
        let caret = index
            .caret_at(Point { x: 9.0, y: 100.0 })
            .expect("the page has rows");
        assert_eq!(caret, Caret { line: 0, offset: 1 });
        let lower = index
            .caret_at(Point { x: 0.0, y: 81.0 })
            .expect("the page has rows");
        assert_eq!(lower.line, 1);
    }

    #[test]
    fn a_click_past_the_end_of_a_short_row_stays_on_that_row() {
        let index = SemanticIndex::of(&page(&[
            at(0.0, 100.0),
            at(0.0, 80.0),
            at(10.0, 80.0),
            at(20.0, 80.0),
        ]));
        let caret = index
            .caret_at(Point { x: 40.0, y: 100.0 })
            .expect("the page has rows");
        assert_eq!(caret.line, 0, "the short upper row keeps its own click");
    }

    #[test]
    fn a_caret_cannot_name_a_position_inside_a_cluster() {
        let graph = page(&[Cell {
            origins: vec![(0.0, 100.0), (7.2, 100.0), (7.2, 100.0)],
            widths: vec![600.0, 0.0, 0.0],
            matrix: Matrix {
                a: EM,
                b: 0.0,
                c: 0.0,
                d: EM,
                e: 0.0,
                f: 100.0,
            },
        }]);
        let index = SemanticIndex::of(&graph);
        assert_eq!(index.clusters.len(), 1, "three glyphs, one cluster");
        assert_eq!(index.lines[0].clusters.len(), 1);
        let start = Caret { line: 0, offset: 0 };
        assert_eq!(index.caret_right(start), Caret { line: 0, offset: 1 });
    }

    #[test]
    fn a_selection_between_two_carets_is_the_clusters_between_them() {
        let index = SemanticIndex::of(&two_rows());
        let from = Caret { line: 0, offset: 0 };
        let to = Caret { line: 0, offset: 2 };
        assert_eq!(index.selection(from, to), vec![0, 1]);
        assert_eq!(index.selection(to, from), vec![0, 1]);
        assert!(index.selection(from, from).is_empty());
    }

    #[test]
    fn a_selection_inside_one_run_is_the_glyphs_it_covers() {
        let graph = one_run_row(&[(0.0, 100.0), (10.0, 100.0), (20.0, 100.0)]);
        let index = SemanticIndex::of(&graph);
        assert_eq!(index.lines.len(), 1);
        let span = index
            .selection_span(Caret { line: 0, offset: 0 }, Caret { line: 0, offset: 2 })
            .expect("one run paints the whole row");
        assert_eq!(span.atom, 0);
        assert_eq!(span.glyphs, 0..2);
        assert_eq!(span.clusters, 2);
        assert_eq!(
            index.selection_span(Caret { line: 0, offset: 2 }, Caret { line: 0, offset: 0 }),
            Ok(span)
        );
        assert_eq!(
            index.selection_span(Caret { line: 0, offset: 1 }, Caret { line: 0, offset: 1 }),
            Err(SelectionError::Empty)
        );
    }

    #[test]
    fn a_selection_across_two_runs_says_so_rather_than_editing_one_of_them() {
        let index = SemanticIndex::of(&two_rows());
        assert_eq!(
            index.selection_span(Caret { line: 0, offset: 0 }, Caret { line: 0, offset: 2 }),
            Err(SelectionError::CrossesRuns(2))
        );
        let spans = index
            .selection_spans(Caret { line: 0, offset: 0 }, Caret { line: 0, offset: 2 })
            .expect("both runs are exact parts of one selection");
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].atom, 0);
        assert_eq!(spans[0].glyphs, 0..1);
        assert_eq!(spans[1].atom, 1);
        assert_eq!(spans[1].glyphs, 0..1);
        let span = index
            .selection_span(Caret { line: 0, offset: 1 }, Caret { line: 0, offset: 2 })
            .expect("one cluster is painted by one run");
        assert_eq!(span.clusters, 1);
        assert_eq!(span.glyphs, 0..1, "each run here paints one glyph");
    }

    #[test]
    fn a_selection_whose_glyphs_are_not_consecutive_is_refused() {
        let graph = one_run_row(&[(0.0, 100.0), (20.0, 100.0), (10.0, 100.0)]);
        let index = SemanticIndex::of(&graph);
        assert_eq!(index.lines[0].clusters, vec![0, 2, 1], "read left to right");
        assert_eq!(
            index.selection_span(Caret { line: 0, offset: 0 }, Caret { line: 0, offset: 2 }),
            Err(SelectionError::NotContiguous)
        );
        let span = index
            .selection_span(Caret { line: 0, offset: 0 }, Caret { line: 0, offset: 3 })
            .expect("the whole run is one range");
        assert_eq!(span.glyphs, 0..3);
    }

    #[test]
    fn a_selection_across_two_rows_is_refused_rather_than_guessed() {
        let index = SemanticIndex::of(&two_rows());
        assert!(
            index
                .selection(Caret { line: 0, offset: 1 }, Caret { line: 1, offset: 2 })
                .is_empty()
        );
    }

    #[test]
    fn each_advancing_glyph_opens_its_own_cluster() {
        let index = clusters(run(
            &[(0.0, 0.0), (10.0, 0.0), (20.0, 0.0)],
            &[10.0, 10.0, 10.0],
            Matrix::IDENTITY,
        ));
        assert_eq!(index.clusters.len(), 3);
        assert!(index.clusters.iter().all(|cluster| !cluster.is_stacked()));
        assert_eq!(index.report.marks, 0);
    }

    #[test]
    fn a_thai_base_with_two_stacked_marks_is_one_cluster() {
        let index = clusters(run(
            &[(0.0, 0.0), (600.0, 0.0), (600.0, 0.0), (600.0, 0.0)],
            &[600.0, 0.0, 0.0, 500.0],
            Matrix::IDENTITY,
        ));
        assert_eq!(index.clusters.len(), 2, "base+marks, then the next base");
        assert_eq!(index.clusters[0].glyphs, 0..3);
        assert_eq!(index.clusters[0].marks, 2);
        assert!(index.clusters[0].is_stacked());
        assert_eq!(
            index.clusters[1].glyphs,
            3..4,
            "the next base shares an origin with the last mark and must still \
             open its own cluster"
        );
        assert_eq!(index.report.marks_by_width, 2);
    }

    #[test]
    fn a_producer_that_backs_the_pen_up_is_clustered_by_position() {
        let index = clusters(run(
            &[(0.0, 0.0), (0.0, 0.0)],
            &[600.0, 600.0],
            Matrix::IDENTITY,
        ));
        assert_eq!(index.clusters.len(), 1);
        assert_eq!(index.report.marks_by_position, 1);
        assert_eq!(index.report.marks_by_width, 0);
    }

    #[test]
    fn a_mark_above_its_base_has_moved_without_advancing() {
        let direction = Point { x: 1.0, y: 0.0 };
        assert!(!advanced(
            Point { x: 0.0, y: 0.0 },
            Point { x: 0.0, y: 7.0 },
            direction
        ));
        assert!(advanced(
            Point { x: 0.0, y: 0.0 },
            Point { x: 7.0, y: 0.0 },
            direction
        ));
    }

    #[test]
    fn a_rotated_run_advances_along_its_own_text_space() {
        let rotate = Matrix {
            a: 0.0,
            b: 1.0,
            c: -1.0,
            d: 0.0,
            e: 0.0,
            f: 0.0,
        };
        let index = clusters(run(
            &[(0.0, 0.0), (0.0, 0.0), (0.0, 10.0)],
            &[10.0, 10.0, 10.0],
            rotate,
        ));
        assert_eq!(index.clusters.len(), 2, "the mark joins its base");
        assert_eq!(index.clusters[0].glyphs, 0..2);
        assert_eq!(index.clusters[1].glyphs, 2..3);
        assert_eq!(index.report.marks_by_position, 1);
    }

    #[test]
    fn a_zero_width_glyph_joins_even_where_character_spacing_moved_the_pen() {
        let index = clusters(run(
            &[(0.0, 0.0), (602.0, 0.0), (604.0, 0.0)],
            &[600.0, 0.0, 0.0],
            Matrix::IDENTITY,
        ));
        assert_eq!(index.clusters.len(), 1);
        assert_eq!(index.clusters[0].marks, 2);
        assert_eq!(index.report.marks_by_width, 2);
        assert_eq!(
            index.report.marks_by_position, 0,
            "the origins moved; only the widths carried this"
        );
    }

    #[test]
    fn both_techniques_on_one_glyph_are_counted_as_both() {
        let index = clusters(run(
            &[(0.0, 0.0), (0.0, 0.0)],
            &[600.0, 0.0],
            Matrix::IDENTITY,
        ));
        assert_eq!(index.clusters.len(), 1);
        assert_eq!(index.report.marks_by_width, 1);
        assert_eq!(index.report.marks_by_position, 1);
        assert_eq!(index.report.marks, 1, "one mark, found twice, counted once");
    }

    #[test]
    fn a_run_that_paints_no_glyph_is_not_a_clustering_failure() {
        let index = clusters(run(&[], &[], Matrix::IDENTITY));
        assert!(index.clusters.is_empty());
        assert_eq!(index.report.runs_without_glyphs, 1);
        assert_eq!(index.report.runs_clustered, 0);
    }

    #[test]
    fn a_degenerate_text_matrix_yields_no_clusters_rather_than_a_guess() {
        let degenerate = Matrix {
            a: 0.0,
            b: 0.0,
            c: 0.0,
            d: 0.0,
            e: 0.0,
            f: 0.0,
        };
        let index = clusters(run(&[(0.0, 0.0), (10.0, 0.0)], &[10.0, 10.0], degenerate));
        assert!(index.clusters.is_empty());
        assert_eq!(index.report.runs_without_glyphs, 1);
    }

    #[test]
    fn every_cluster_is_marked_inferred() {
        let index = clusters(run(
            &[(0.0, 0.0), (10.0, 0.0)],
            &[10.0, 0.0],
            Matrix::IDENTITY,
        ));
        assert!(
            index
                .clusters
                .iter()
                .all(|cluster| cluster.evidence == ClusterEvidence::Inferred)
        );
    }

    #[test]
    fn an_advance_just_over_the_epsilon_separates_and_just_under_joins() {
        let direction = Point { x: 1.0, y: 0.0 };
        let origin = Point { x: 0.0, y: 0.0 };
        assert!(advanced(
            origin,
            Point {
                x: ADVANCE_EPSILON * 2.0,
                y: 0.0
            },
            direction
        ));
        assert!(!advanced(
            origin,
            Point {
                x: ADVANCE_EPSILON / 2.0,
                y: 0.0
            },
            direction
        ));
    }

    #[test]
    fn clusters_carry_the_atom_that_paints_them_and_never_a_copy_of_it() {
        let index = clusters(run(&[(0.0, 0.0)], &[10.0], Matrix::IDENTITY));
        let cluster: &Cluster = &index.clusters[0];
        assert_eq!(cluster.atom, 0);
        assert_eq!(cluster.glyphs, 0..1);
        assert_eq!(cluster.origin, Point { x: 0.0, y: 0.0 });
    }

    #[test]
    fn a_recovered_multibyte_pair_is_one_cluster_though_both_bytes_advance() {
        let mut paint = run(
            &[(0.0, 0.0), (6.0, 0.0), (12.0, 0.0)],
            &[500.0, 500.0, 500.0],
            Matrix::IDENTITY,
        );
        paint.glyphs[1].silent = true;
        let index = clusters(paint);
        assert_eq!(index.clusters.len(), 2, "the pair must not be split");
        assert_eq!(index.clusters[0].glyphs, 0..2);
        assert_eq!(index.clusters[0].continuations, 1);
        assert_eq!(
            index.clusters[0].marks, 0,
            "a continuation is not a mark and must not make the cluster stacked"
        );
        assert!(!index.clusters[0].is_stacked());
        assert_eq!(index.clusters[1].glyphs, 2..3);
        assert_eq!(index.report.continuations, 1);
        assert_eq!(index.report.marks, 0);

        let plain = clusters(run(
            &[(0.0, 0.0), (6.0, 0.0), (12.0, 0.0)],
            &[500.0, 500.0, 500.0],
            Matrix::IDENTITY,
        ));
        assert_eq!(plain.clusters.len(), 3);
        assert_eq!(plain.report.continuations, 0);
    }

    #[test]
    fn a_sign_the_file_says_continues_its_letter_is_one_cluster_with_it() {
        let meanings = pdf_font::ToUnicode::from_cmap(
            &pdf_bytes::ByteStore::new(
                SourceId::new(9),
                std::sync::Arc::<[u8]>::from(
                    &b"1 begincodespacerange <00> <ff> endcodespacerange \
                       3 beginbfchar <01> <0CA8> <02> <0CC1> <03> <0C95> endbfchar"[..],
                ),
            ),
            pdf_font::CMapLimits::default(),
        )
        .expect("the map reads");
        let placed = || {
            let mut paint = run(
                &[(0.0, 0.0), (6.0, 0.0), (12.0, 0.0)],
                &[500.0, 500.0, 500.0],
                Matrix::IDENTITY,
            );
            for (value, glyph) in (1..).zip(&mut paint.glyphs) {
                glyph.code.value = value;
                glyph.code.bytes = vec![u8::try_from(value).expect("a byte")];
            }
            paint
        };
        let mut read = placed();
        read.text = std::sync::Arc::new(meanings);
        let index = clusters(read);
        assert_eq!(index.clusters.len(), 2);
        assert_eq!(index.clusters[0].glyphs, 0..2);
        assert_eq!(index.clusters[1].glyphs, 2..3);

        let unread = clusters(placed());
        assert_eq!(unread.clusters.len(), 3, "no text, no joining");
    }

    #[test]
    fn either_technique_joins_and_neither_is_required() {
        let by_width = MarkEvidence {
            declared_zero_width: true,
            origin_did_not_advance: false,
            continues_a_source_code: false,
            extends_the_cluster_before: false,
        };
        assert!(by_width.joins_previous());
        assert!(!by_width.both());

        let by_position = MarkEvidence {
            declared_zero_width: false,
            origin_did_not_advance: true,
            continues_a_source_code: false,
            extends_the_cluster_before: false,
        };
        assert!(by_position.joins_previous());
        assert!(!by_position.both());

        let continuation = MarkEvidence {
            declared_zero_width: false,
            origin_did_not_advance: false,
            continues_a_source_code: true,
            extends_the_cluster_before: false,
        };
        assert!(continuation.joins_previous());
        assert!(
            !continuation.both(),
            "`both` is about the two mark techniques and must not include this"
        );

        let base = MarkEvidence {
            declared_zero_width: false,
            origin_did_not_advance: false,
            continues_a_source_code: false,
            extends_the_cluster_before: false,
        };
        assert!(!base.joins_previous());

        let both = MarkEvidence {
            declared_zero_width: true,
            origin_did_not_advance: true,
            continues_a_source_code: false,
            extends_the_cluster_before: false,
        };
        assert!(both.joins_previous());
        assert!(both.both());
    }

    fn atom(kind: PaintAtomKind, ordinal: usize) -> PaintAtom {
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

    fn image(ctm: Matrix) -> PaintAtomKind {
        let state = GraphicsState {
            ctm: Derived {
                value: ctm,
                provenance: pdf_paint::Provenance::new(),
            },
            ..Default::default()
        };
        PaintAtomKind::Image(Box::new(ImagePaint {
            reference: Reference::new(7, 0),
            dictionary_span: span(),
            encoded_data_span: span(),
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
            color_space: Some(Derived {
                value: ColorSpace::DeviceGray,
                provenance: pdf_paint::Provenance::new(),
            }),
            image_mask: Derived {
                value: false,
                provenance: pdf_paint::Provenance::new(),
            },
            decode: Derived {
                value: vec![0.0, 1.0],
                provenance: pdf_paint::Provenance::new(),
            },
            interpolate: Derived {
                value: false,
                provenance: pdf_paint::Provenance::new(),
            },
            soft_mask: None,
            mask: None,
            matte: None,
            samples: std::sync::Arc::from(vec![0_u8].as_slice()),
            state,
        }))
    }

    fn square(x: f64, y: f64, side: f64) -> PaintAtomKind {
        PaintAtomKind::Path(PathPaint {
            path: Path {
                segments: vec![PathSegment::Rectangle {
                    origin: Point { x, y },
                    width: side,
                    height: side,
                    provenance: span(),
                }],
            },
            stroke: false,
            fill: Some(FillRule::Nonzero),
            state: GraphicsState::default(),
        })
    }

    fn type3_run(origins: &[(f64, f64)], text_matrix: Matrix) -> TextShowPaint {
        let widths = vec![500.0; origins.len()];
        let mut text = run(origins, &widths, text_matrix);
        text.type3 = true;
        for (glyph, &(x, y)) in text.glyphs.iter_mut().zip(origins) {
            glyph.procedure = Some(std::sync::Arc::new(Type3Glyph {
                name: b"a".to_vec(),
                reference: Reference::new(8, 0),
                atoms: vec![atom(square(x - 2.0, y - 2.0, 4.0), 0)],
                shape_only: false,
            }));
        }
        text
    }

    fn twelve_point() -> Matrix {
        Matrix {
            a: 12.0,
            b: 0.0,
            c: 0.0,
            d: 12.0,
            e: 0.0,
            f: 0.0,
        }
    }

    fn same_box(one: [f64; 4], other: [f64; 4]) {
        for (found, wanted) in one.iter().zip(&other) {
            assert!((found - wanted).abs() < 1e-9, "{one:?} is not {other:?}");
        }
    }

    fn area(quad: &Quad) -> f64 {
        let mut twice = 0.0;
        for index in 0..4 {
            let one = quad.corners[index];
            let other = quad.corners[(index + 1) % 4];
            twice += one.x.mul_add(other.y, -(other.x * one.y));
        }
        (twice / 2.0).abs()
    }

    #[test]
    fn an_image_is_an_object_placed_exactly_where_its_matrix_puts_it() {
        let graph = PaintGraph {
            object_scopes: Vec::new(),
            repairs: Vec::new(),
            skipped: Vec::new(),
            atoms: vec![atom(
                image(Matrix {
                    a: 120.0,
                    b: 0.0,
                    c: 0.0,
                    d: 90.0,
                    e: 380.0,
                    f: 600.0,
                }),
                0,
            )],
        };
        let index = SemanticIndex::of(&graph);
        assert_eq!(index.objects.len(), 1);
        let object = &index.objects[0];
        assert_eq!(object.kind, ObjectKind::Image);
        assert_eq!(object.members, vec![Member::whole(0)]);
        let quad = object.quad.expect("an image is always placed");
        assert_eq!(quad.evidence, QuadEvidence::Placement);
        assert_eq!(
            quad.corners,
            [
                Point { x: 380.0, y: 600.0 },
                Point { x: 500.0, y: 600.0 },
                Point { x: 500.0, y: 690.0 },
                Point { x: 380.0, y: 690.0 },
            ]
        );
    }

    #[test]
    fn a_turned_image_is_framed_by_a_quad_and_not_by_the_box_round_it() {
        let turn = std::f64::consts::FRAC_PI_4;
        let graph = PaintGraph {
            object_scopes: Vec::new(),
            repairs: Vec::new(),
            skipped: Vec::new(),
            atoms: vec![atom(
                image(Matrix {
                    a: turn.cos() * 100.0,
                    b: turn.sin() * 100.0,
                    c: -turn.sin() * 100.0,
                    d: turn.cos() * 100.0,
                    e: 0.0,
                    f: 0.0,
                }),
                0,
            )],
        };
        let index = SemanticIndex::of(&graph);
        let quad = index.objects[0].quad.expect("placed");
        let box_ = quad.bounds();
        let boxed = (box_[2] - box_[0]) * (box_[3] - box_[1]);
        assert!((area(&quad) / boxed - 0.5).abs() < 1e-9, "{quad:?}");
        assert!(quad.contains(quad.center()));
        assert!(!quad.contains(Point {
            x: box_[0] + 1.0,
            y: box_[1] + 1.0
        }));
    }

    #[test]
    fn a_rotated_line_is_framed_by_its_ink_and_not_by_its_bounding_box() {
        let turn = std::f64::consts::FRAC_PI_6;
        let matrix = Matrix {
            a: turn.cos() * 12.0,
            b: turn.sin() * 12.0,
            c: -turn.sin() * 12.0,
            d: turn.cos() * 12.0,
            e: 0.0,
            f: 0.0,
        };
        let origins: Vec<(f64, f64)> = (0..6)
            .map(|step| {
                let along = f64::from(step) * 10.0;
                (along * turn.cos(), along * turn.sin())
            })
            .collect();
        let graph = graph_of(type3_run(&origins, matrix));
        let index = SemanticIndex::of(&graph);
        assert_eq!(index.objects.len(), 1);
        assert_eq!(index.objects[0].kind, ObjectKind::Text(0));
        let quad = index.objects[0].quad.expect("ink to measure");
        assert_eq!(quad.evidence, QuadEvidence::Ink);

        let box_ = index.objects[0].bounds.expect("ink to measure");
        let boxed = (box_[2] - box_[0]) * (box_[3] - box_[1]);
        assert!(
            area(&quad) < boxed / 4.0,
            "a frame turned to the text is {} against a box of {boxed}",
            area(&quad)
        );
        for &(x, y) in &origins {
            assert!(
                quad.contains(Point { x, y }),
                "({x}, {y}) is off its own line"
            );
        }
    }

    #[test]
    fn an_upright_line_is_framed_by_the_box_it_already_had() {
        let origins: Vec<(f64, f64)> = (0..4).map(|step| (f64::from(step) * 10.0, 0.0)).collect();
        let graph = graph_of(type3_run(&origins, twelve_point()));
        let index = SemanticIndex::of(&graph);
        let quad = index.objects[0].quad.expect("ink to measure");
        let box_ = index.objects[0].bounds.expect("ink to measure");
        same_box(quad.bounds(), box_);
    }

    #[test]
    fn strokes_that_touch_are_still_drawings_of_their_own() {
        let graph = PaintGraph {
            object_scopes: Vec::new(),
            repairs: Vec::new(),
            skipped: Vec::new(),
            atoms: vec![
                atom(square(0.0, 0.0, 10.0), 0),
                atom(square(10.5, 0.0, 10.0), 1),
                atom(square(21.0, 0.0, 10.0), 2),
            ],
        };
        let index = SemanticIndex::of(&graph);
        assert_eq!(index.objects.len(), 3, "{:?}", index.objects);
        for (step, object) in index.objects.iter().enumerate() {
            assert_eq!(object.kind, ObjectKind::Path);
            assert_eq!(object.members, vec![Member::whole(step)]);
        }
        assert_eq!(index.objects[1].bounds, Some([10.5, 0.0, 20.5, 10.0]));
        assert_eq!(
            index.report.paths_outside_any_object, 0,
            "ink with no owner"
        );
        assert!(MembershipAudit::of_index(&graph, &index).accounted());
    }

    #[test]
    fn a_run_that_cannot_be_clustered_is_still_one_object() {
        let degenerate = Matrix {
            a: 0.0,
            b: 0.0,
            c: 0.0,
            d: 12.0,
            e: 0.0,
            f: 0.0,
        };
        let graph = graph_of(run(&[(0.0, 0.0)], &[10.0], degenerate));
        let index = SemanticIndex::of(&graph);

        assert_eq!(index.report.runs_without_glyphs, 1);
        assert_eq!(index.objects.len(), 1);
        assert_eq!(index.objects[0].kind, ObjectKind::TextRun);
        assert_eq!(index.objects[0].members, vec![Member::whole(0)]);
        assert!(MembershipAudit::of_index(&graph, &index).accounted());
    }

    #[test]
    fn a_cluster_whose_geometry_is_not_a_number_still_owns_its_glyph() {
        let unreadable = Matrix {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: f64::NAN,
            e: 0.0,
            f: 0.0,
        };
        let graph = graph_of(run(&[(0.0, 0.0)], &[10.0], unreadable));
        let index = SemanticIndex::of(&graph);

        assert_eq!(index.clusters.len(), 1);
        assert_eq!(index.report.clusters_stranded_by_geometry, 1);
        assert_eq!(index.lines.len(), 1, "the stranded cluster is its own row");
        assert!(
            MembershipAudit::of_index(&graph, &index).accounted(),
            "a glyph the row rule could not place is still ink somebody owns"
        );
    }

    #[test]
    fn one_show_operation_split_between_two_blocks_is_owned_by_glyph_not_by_atom() {
        let text = run(
            &[(0.0, 0.0), (10.0, 0.0), (0.0, -40.0), (10.0, -40.0)],
            &[10.0, 10.0, 10.0, 10.0],
            twelve_point(),
        );
        let graph = graph_of(text);
        let index = SemanticIndex::of(&graph);

        assert_eq!(index.blocks.len(), 2, "one run, two blocks");
        let audit = MembershipAudit::of_index(&graph, &index);
        assert!(
            audit.accounted(),
            "one atom claimed twice or left half-owned: {audit:?}"
        );
        let owned: Vec<Vec<Member>> = index
            .objects
            .iter()
            .map(|object| object.members.clone())
            .collect();
        assert_eq!(
            owned,
            vec![vec![Member::glyphs(0, 0..2)], vec![Member::glyphs(0, 2..4)]]
        );
    }

    #[test]
    fn objects_are_in_paint_order_so_the_last_painted_is_the_one_on_top() {
        let graph = PaintGraph {
            object_scopes: Vec::new(),
            repairs: Vec::new(),
            skipped: Vec::new(),
            atoms: vec![
                atom(image(Matrix::IDENTITY), 0),
                atom(
                    PaintAtomKind::Text(run(&[(0.0, 0.0)], &[10.0], Matrix::IDENTITY)),
                    1,
                ),
                atom(image(Matrix::IDENTITY), 2),
            ],
        };
        let index = SemanticIndex::of(&graph);
        let kinds: Vec<ObjectKind> = index.objects.iter().map(|object| object.kind).collect();
        assert_eq!(
            kinds,
            vec![ObjectKind::Image, ObjectKind::Text(0), ObjectKind::Image]
        );
        assert_eq!(index.objects[1].atoms().collect::<Vec<_>>(), vec![1]);
    }

    #[test]
    fn a_block_whose_font_embeds_no_program_is_an_object_with_no_frame() {
        let index = clusters(run(
            &[(0.0, 0.0), (10.0, 0.0)],
            &[10.0, 10.0],
            twelve_point(),
        ));
        assert_eq!(index.objects.len(), 1);
        assert!(index.objects[0].quad.is_none());
        assert_eq!(index.report.objects_without_extent, 1);
    }

    #[test]
    fn a_quad_with_no_area_contains_its_own_box_and_not_the_page() {
        let flat = Quad {
            corners: [
                Point { x: 0.0, y: 0.0 },
                Point { x: 10.0, y: 0.0 },
                Point { x: 10.0, y: 0.0 },
                Point { x: 0.0, y: 0.0 },
            ],
            evidence: QuadEvidence::Placement,
        };
        assert!(flat.contains(Point { x: 5.0, y: 0.0 }));
        assert!(!flat.contains(Point { x: 5.0, y: 5.0 }));

        let point = Quad {
            corners: [Point { x: 3.0, y: 4.0 }; 4],
            evidence: QuadEvidence::Placement,
        };
        assert!(point.contains(Point { x: 3.0, y: 4.0 }));
        assert!(!point.contains(Point { x: 3.0, y: 5.0 }));
    }

    #[test]
    fn a_mirrored_placement_is_contained_the_same_way_a_plain_one_is() {
        let mirrored = Quad::placed(
            [0.0, 0.0, 1.0, 1.0],
            Matrix {
                a: -10.0,
                b: 0.0,
                c: 0.0,
                d: 10.0,
                e: 0.0,
                f: 0.0,
            },
        );
        assert!(mirrored.contains(Point { x: -5.0, y: 5.0 }));
        assert!(!mirrored.contains(Point { x: 5.0, y: 5.0 }));
        same_box(mirrored.bounds(), [-10.0, 0.0, 0.0, 10.0]);
        assert_eq!(mirrored.center(), Point { x: -5.0, y: 5.0 });
    }
}

#[cfg(test)]
mod insertion_owner_tests {
    use super::{ClusterKey, Grouping};
    use std::collections::BTreeMap;

    const fn key(atom: usize, glyph: usize) -> ClusterKey {
        ClusterKey { atom, glyph }
    }

    #[test]
    fn text_inserted_at_a_mark_joins_the_block_of_the_cluster_it_is_in() {
        let grouping = Grouping {
            blocks: vec![vec![key(3, 0), key(3, 2), key(3, 4)], vec![key(3, 6)]],
        };
        let mapping: BTreeMap<ClusterKey, ClusterKey> = [(0, 0), (2, 2), (4, 4), (6, 8)]
            .iter()
            .map(|(was, now)| (key(3, *was), key(3, *now)))
            .collect();
        let typed = [(key(3, 5), key(3, 6)), (key(3, 5), key(3, 7))];
        let after = grouping.remapped_with_insertions(&mapping, &typed);
        assert_eq!(
            after.blocks,
            vec![
                vec![key(3, 0), key(3, 2), key(3, 4), key(3, 6), key(3, 7)],
                vec![key(3, 8)]
            ]
        );
    }

    #[test]
    fn text_inserted_in_a_run_no_block_holds_is_owned_by_none() {
        let grouping = Grouping {
            blocks: vec![vec![key(3, 2)]],
        };
        let mapping: BTreeMap<ClusterKey, ClusterKey> = [(key(3, 2), key(3, 2))].into();
        let after = grouping
            .remapped_with_insertions(&mapping, &[(key(3, 1), key(3, 9)), (key(4, 5), key(4, 6))]);
        assert_eq!(after.blocks, vec![vec![key(3, 2)]]);
    }
}
