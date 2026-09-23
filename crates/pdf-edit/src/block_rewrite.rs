use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::ops::Range;

use icu_segmenter::GraphemeClusterSegmenter;
use pdf_paint::{
    Code, Matrix, PaintAtomKind, PaintGraph, Point, TextRenderingMode, TextShowElement,
    TextShowPaint, TextState,
};
use pdf_semantics::ClusterKey;

use crate::block_move::{PLACEMENT_TOLERANCE, linear};
use crate::layout::{
    Alignment, Blocked, FIT_SLACK, LayoutError, Paragraph, Unit, justify_gaps, lay_out_around,
    line_break_opportunities, widest_free_run,
};
use crate::plan::{
    BlockOutcome, BlockRange, Capability, ClusterRef, Effect, LINE_BREAK, MovedRun, Plan,
    PlannedBody, PlannedCaret, PlannedWrite, RowEnds, SourceAnchor,
};
use crate::spike_move_text::{PlannerPage, SpikeError};

pub(crate) mod faces;
mod lift;
mod live;
mod prove;
pub(crate) mod turn;
mod underline;
mod write;

pub(crate) use faces::{
    commit as commit_writes, commit_with as commit_writes_with, next_object_number,
};
pub(crate) use live::lay_out_live;
pub use live::{LiveBlock, LiveLine, LivePiece};
use prove::{Proved, Show, prove};
use underline::{Rule, adopt_underlines, rules, underline_bytes, underline_spans};
use write::{shows, write_stream, written_as_run};

const ALIGN_SLACK: f64 = 0.5;

const FIRST_INDENT_PITCHES: f64 = 4.0;

const INDENTED_ROW_FILLS: f64 = 0.75;

const PITCH_SLACK_EM: f64 = 1e-3;

const ROUNDING_SLACK_EM: f64 = 0.02;

const LINE_FLOOR_EM: f64 = 0.8;

const DEFAULT_LINE_EM: f64 = 1.2;
const SCALE_SLACK: f64 = 1e-6;

pub(crate) struct BlockEdit<'a> {
    pub rows: &'a [Vec<ClusterRef>],
    pub frame: (f64, f64),
    pub edges: (usize, usize),
    pub breaks: Option<&'a RowEnds>,
    pub range: BlockRange,
    pub text: &'a str,
    pub style: Option<&'a crate::plan::TextStyle>,
    pub typed: Option<&'a crate::plan::TextStyle>,
    pub empty: Option<&'a crate::SourceAnchor>,
    pub offset: (f64, f64),
    pub paragraph: crate::plan::ParagraphLayout,
}

impl BlockEdit<'_> {
    fn shift(&self) -> Option<(f64, f64)> {
        (self.offset.0.abs() > 0.0 || self.offset.1.abs() > 0.0).then_some(self.offset)
    }
}

fn unsupported(reason: &'static str) -> SpikeError {
    SpikeError::BlockRewriteUnsupported(reason)
}

#[derive(Clone, Debug)]
struct Piece {
    codes: Vec<pdf_content::SourceCode>,
    text: String,
    advance: f64,
    kept: Option<ClusterKey>,
    group: usize,
    style: usize,
    adjust: Vec<f64>,
    next: Option<ClusterKey>,
    break_before: bool,
    shaped: Vec<pdf_content::ShapedGlyph>,
    rise: Vec<f64>,
    underline: bool,
}

const UNREAD: &str = "\u{FFFD}";

enum Token {
    Cluster(Piece),
    Break { space: f64 },
    LineBreak,
}

struct Run<'g> {
    reference: &'g TextShowPaint,
    font: pdf_content::Font,
    text: TextState,
    fill: Option<String>,
    stroke: Option<String>,
    fill_rgb: Option<[f64; 3]>,
    line_width: f64,
    stroke_like_fill: bool,
    shear: f64,
    line: Option<(f64, f64)>,
}

struct Style<'g> {
    runs: Vec<Run<'g>>,
    run_of: BTreeMap<usize, usize>,
    to_user: Matrix,
    ctm: Matrix,
    tm_linear: Matrix,
    rule: (f64, f64),
    unspaced: BTreeMap<usize, usize>,
}

impl<'g> Style<'g> {
    fn base(&self) -> &Run<'g> {
        &self.runs[0]
    }

    fn run(&self, index: usize) -> &Run<'g> {
        self.runs.get(index).unwrap_or(&self.runs[0])
    }

    fn em(&self) -> f64 {
        self.run_em(0)
    }

    fn run_em(&self, index: usize) -> f64 {
        self.run(index).text.font_size.value * self.to_user.d
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineEnd {
    Wrap,
    WrapWithSpace,
    Line,
    Paragraph,
    Last,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReadLine {
    pub row: Option<usize>,
    pub origin: (f64, f64),
    pub em: f64,
    pub clusters: Vec<String>,
    pub end: LineEnd,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "four independent yes-or-no facts a toolbar reads, not a state machine"
)]
pub struct ClusterFace {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub blank: bool,
}

pub fn read_block_faces(
    program: &pdf_content::PageProgram,
    graph: &PaintGraph,
    rows: &[Vec<ClusterRef>],
    frame: (f64, f64),
    edges: (usize, usize),
    breaks: Option<&RowEnds>,
) -> Result<Vec<Vec<ClusterFace>>, SpikeError> {
    let angle = turn::of_rows(graph, rows)?;
    let turned;
    let graph = if angle == 0.0 {
        graph
    } else {
        turned = turn::turned(graph, angle);
        &turned
    };
    let reading = Reading::of(program, (graph, None), rows, frame, edges, breaks)?;
    let face_of = |piece: &Piece| {
        let run = reading.style.run(piece.style);
        let request = run.text.font.as_ref().and_then(|applied| {
            program
                .resources
                .font(&applied.value.name)
                .and_then(|resource| resource.face_request().ok().flatten())
        });
        ClusterFace {
            bold: run.stroke_like_fill
                || request
                    .as_ref()
                    .is_some_and(|request| request.style.is_bold()),
            italic: run.shear != 0.0
                || request.as_ref().is_some_and(|request| request.style.italic),
            underline: piece.underline,
            blank: piece.text.chars().all(char::is_whitespace),
        }
    };
    Ok(reading
        .lines
        .iter()
        .map(|line| {
            line.row.map_or_else(Vec::new, |row| {
                reading.measured[row].clusters.iter().map(face_of).collect()
            })
        })
        .collect())
}

#[derive(Clone, Debug, PartialEq)]
pub struct BlockReading {
    pub pitch: f64,
    pub alignment: Alignment,
    pub turn: f64,
    pub lines: Vec<ReadLine>,
}

impl BlockReading {
    #[must_use]
    pub fn position(&self, (line, stop): (usize, usize)) -> Option<usize> {
        let mut base = 0;
        for (index, read) in self.lines.iter().enumerate() {
            if index == line {
                return (stop <= read.clusters.len()).then_some(base + stop);
            }
            base += read.clusters.len() + usize::from(read.end != LineEnd::Wrap);
        }
        None
    }

    #[must_use]
    pub fn text_between(&self, one: (usize, usize), other: (usize, usize)) -> Option<String> {
        let (from, to) = {
            let (a, b) = (self.position(one)?, self.position(other)?);
            (a.min(b), a.max(b))
        };
        let mut text = String::new();
        let mut at = 0;
        for read in &self.lines {
            for cluster in &read.clusters {
                if (from..to).contains(&at) {
                    text.push_str(cluster);
                }
                at += 1;
            }
            let joint = match read.end {
                LineEnd::Paragraph | LineEnd::Line => Some("\n"),
                LineEnd::WrapWithSpace => Some(" "),
                LineEnd::Wrap | LineEnd::Last => None,
            };
            if let Some(joint) = joint {
                if (from..to).contains(&at) {
                    text.push_str(joint);
                }
                at += 1;
            }
        }
        Some(text)
    }
}

pub fn read_block(
    program: &pdf_content::PageProgram,
    graph: &PaintGraph,
    rows: &[Vec<ClusterRef>],
    frame: (f64, f64),
    edges: (usize, usize),
    breaks: Option<&RowEnds>,
) -> Result<BlockReading, SpikeError> {
    let angle = turn::of_rows(graph, rows)?;
    let turned;
    let graph = if angle == 0.0 {
        graph
    } else {
        turned = turn::turned(graph, angle);
        &turned
    };
    let reading = Reading::of(program, (graph, None), rows, frame, edges, breaks)?;
    let groups = line_groups(&reading.lines);
    let alignments = alignments(&reading, &groups, frame, false);
    let em = reading.style.em();
    let lines = reading
        .lines
        .iter()
        .zip(&groups)
        .map(|(line, group)| {
            let (clusters, x) = line.row.map_or_else(
                || {
                    let (alignment, edge, indent) = alignments[*group];
                    (
                        Vec::new(),
                        edge + indent + alignment.offset(frame.1 - edge - indent, 0.0),
                    )
                },
                |row| {
                    (
                        reading.measured[row]
                            .clusters
                            .iter()
                            .map(|piece| piece.text.clone())
                            .collect(),
                        reading.measured[row].start,
                    )
                },
            );
            ReadLine {
                row: line.row,
                origin: (x, line.baseline),
                em,
                clusters,
                end: line.end,
            }
        })
        .collect();
    Ok(BlockReading {
        pitch: reading.pitch,
        alignment: alignments
            .first()
            .map_or(Alignment::Start, |(alignment, _, _)| *alignment),
        turn: angle,
        lines,
    })
}

fn read_edited_block<'g>(
    page: PlannerPage<'g>,
    edit: &BlockEdit<'_>,
) -> Result<(Reading<'g>, (f64, f64)), SpikeError> {
    let graph = page.graph;
    let mut frame = edit.frame;
    if let Some(run) = edit.empty {
        return Ok((Reading::empty(page.program, graph, run, frame)?, frame));
    }
    let mut reading = Reading::of(
        page.program,
        (graph, Some(page.operations)),
        edit.rows,
        frame,
        edit.edges,
        edit.breaks,
    )?;
    if edit
        .style
        .is_some_and(|style| *style != crate::plan::TextStyle::default())
    {
        let widest = reading
            .measured
            .iter()
            .map(|row| row.end)
            .fold(f64::NEG_INFINITY, f64::max);
        if widest > frame.1 {
            frame.1 = widest;
            reading = Reading::of(
                page.program,
                (graph, Some(page.operations)),
                edit.rows,
                frame,
                edit.edges,
                edit.breaks,
            )?;
        }
    }
    Ok((reading, frame))
}

pub(crate) fn plan_block_rewrite(
    source: &pdf_bytes::ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    edit: &BlockEdit<'_>,
) -> Result<Plan, SpikeError> {
    let angle = turn::of_rows(page.graph, edit.rows)?;
    if angle == 0.0 {
        return plan_laid(source, page, page_index, edit, 0.0);
    }
    let graph = turn::turned(page.graph, angle);
    let edit = BlockEdit {
        offset: turn::into(angle, edit.offset),
        ..*edit
    };
    plan_laid(
        source,
        PlannerPage {
            graph: &graph,
            ..page
        },
        page_index,
        &edit,
        angle,
    )
}

fn plan_laid(
    source: &pdf_bytes::ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    edit: &BlockEdit<'_>,
    angle: f64,
) -> Result<Plan, SpikeError> {
    let graph = page.graph;
    let (mut reading, frame) = read_edited_block(page, edit)?;
    let stream_index = single_stream(page.program, reading.stream)?;
    let (left, right) = frame;
    let groups = line_groups(&reading.lines);
    let alignments = alignments(&reading, &groups, frame, edit.paragraph.flow_round);

    let mut faces = faces::NewFaces::new(page.fonts, page.program, page.restrictions);
    let (mut tokens, caret, selected) = edited_tokens(&reading, &groups, edit, &mut faces)?;
    let selection_start = selected.start;
    let settled = styled_and_settled(
        source,
        (page, page_index),
        edit,
        &mut reading.style,
        (&mut tokens, selected),
        &mut faces,
    )?;
    if let Some(points) = edit.style.and_then(|style| style.line_spacing) {
        reading.pitch = line_spacing_pitch(&reading.style, points)?;
    }
    let blocked = stands_in_the_frame(page, &reading, (left, right), edit.paragraph);
    let laid = place_lines(
        &tokens,
        &alignments,
        &reading,
        (left, right),
        InTheFrame {
            set: edit.paragraph.alignment,
            blocked: &blocked,
        },
    )?;
    let placed = lines_to_write(edit, &reading, graph, &laid)?;
    let owner = reading
        .rows
        .first()
        .and_then(|row| row.first())
        .map(|cluster| cluster.key)
        .or_else(|| {
            reading.named.iter().next().map(|atom| ClusterKey {
                atom: *atom,
                glyph: 0,
            })
        })
        .ok_or_else(|| unsupported("the block has no text to lay out"))?;
    let written = |lifted: &[lift::ClipEdit], settled: Option<faces::Settled>| {
        write_and_prove(
            page,
            (stream_index, graph),
            &reading,
            &placed,
            (lifted, settled),
            (owner, edit.shift(), angle),
        )
    };
    let (proved, mut writes, stream_write) = in_the_open(
        written(&[], settled.clone())?,
        &written,
        (page, stream_index, graph),
        (&reading, &placed),
        settled,
    );

    let outcome = BlockOutcome {
        cropped: proved.cropped,
        ..if edit.shift().is_some() {
            laid_outcome(&laid, &tokens, (reading.pitch, faces.typed_families()))
        } else {
            BlockOutcome {
                caret: planned_caret(&placed, &tokens, caret, &proved.keys),
                anchor: edit.style.and(planned_caret(
                    &placed,
                    &tokens,
                    selection_start,
                    &proved.keys,
                )),
                ..laid_outcome(&placed, &tokens, (reading.pitch, faces.typed_families()))
            }
        }
    };
    let moved = moved_runs(graph, &reading.named);
    writes.push(stream_write);
    Ok(Plan::new(
        Capability::Normalized,
        writes,
        Effect {
            page_index,
            moved,
            target_stream: reading.stream,
            declared_region: proved
                .region
                .map(|region| turn::bounds_out_of(angle, region)),
        },
    )
    .with_correspondence(proved.mapping)
    .with_inserted(proved.inserted)
    .with_showing(if proved.cropped {
        crate::plan::Showing::PartlyHidden
    } else {
        crate::plan::Showing::Whole
    })
    .with_block(outcome))
}

fn in_the_open<W>(
    plain: (Proved, Vec<PlannedWrite>, PlannedWrite),
    written: &W,
    (page, stream_index, graph): (PlannerPage<'_>, usize, &PaintGraph),
    (reading, placed): (&Reading<'_>, &[PlacedLine]),
    settled: Option<faces::Settled>,
) -> (Proved, Vec<PlannedWrite>, PlannedWrite)
where
    W: Fn(
        &[lift::ClipEdit],
        Option<faces::Settled>,
    ) -> Result<(Proved, Vec<PlannedWrite>, PlannedWrite), SpikeError>,
{
    if !plain.0.cropped {
        return plain;
    }
    let lifted = lift::inert_clips(page, stream_index, graph, &reading.named);
    if !lifted.is_empty()
        && let Ok(done) = written(&lifted, settled.clone())
        && !done.0.cropped
    {
        return done;
    }
    laid_extent(&reading.style, placed)
        .and_then(|needed| lift::grown_clips(page, stream_index, graph, &reading.named, needed))
        .and_then(|grown| written(&grown, settled).ok())
        .filter(|done| !done.0.cropped)
        .unwrap_or(plain)
}

fn write_and_prove(
    page: PlannerPage<'_>,
    (stream_index, graph): (usize, &PaintGraph),
    reading: &Reading<'_>,
    placed: &[PlacedLine],
    (lifted, settled): (&[lift::ClipEdit], Option<faces::Settled>),
    (owner, shift, angle): (ClusterKey, Option<(f64, f64)>, f64),
) -> Result<(Proved, Vec<PlannedWrite>, PlannedWrite), SpikeError> {
    let (edited, produced) = write_stream(
        page,
        stream_index,
        graph,
        (&reading.named, &reading.outside, &reading.underlines),
        &reading.style,
        placed,
        (reading.pitch, lifted),
    )?;
    let stream_write = PlannedWrite {
        reference: reading.stream,
        body: PlannedBody::ReplacedStream { decoded: edited },
    };
    let (mut rewritten, writes) = read_rewritten(page, stream_index, &stream_write, settled)?;
    if angle != 0.0 {
        rewritten = turn::turned(&rewritten, angle);
    }
    let proved = prove(
        graph,
        &rewritten,
        (&reading.named, &reading.outside, &reading.underlines),
        &reading.style,
        placed,
        produced,
        owner,
    )?;
    if let Some((dx, dy)) = shift {
        let own = |key: &ClusterKey| {
            reading.rows.iter().flatten().any(|located| {
                located.ordinal == key.atom
                    && (located.glyphs.0..located.glyphs.1).contains(&key.glyph)
            })
        };
        for (was, now) in &proved.mapping {
            if !own(was) {
                continue;
            }
            let (before, after) = (text_of(graph, was.atom)?, text_of(&rewritten, now.atom)?);
            let (Some(one), Some(other)) =
                (before.glyphs.get(was.glyph), after.glyphs.get(now.glyph))
            else {
                return Err(SpikeError::GlyphRangeOutsideRun);
            };
            let from = pen_of(before.state.ctm.value, one.text_matrix);
            let to = pen_of(after.state.ctm.value, other.text_matrix);
            if (to.x - from.x - dx).abs() > SHIFT_TOLERANCE
                || (to.y - from.y - dy).abs() > SHIFT_TOLERANCE
            {
                return Err(unsupported(
                    "the block laid out again does not stand where it stood, so a shift would rearrange it",
                ));
            }
        }
    }
    Ok((proved, writes, stream_write))
}

fn lines_to_write(
    edit: &BlockEdit<'_>,
    reading: &Reading<'_>,
    graph: &PaintGraph,
    laid: &[PlacedLine],
) -> Result<Vec<PlacedLine>, SpikeError> {
    let (dx, dy) = edit.offset;
    if !(dx.is_finite() && dy.is_finite()) {
        return Err(unsupported("a block is moved by a finite offset"));
    }
    let mut placed = if edit.shift().is_some() {
        verbatim_lines(reading, graph)?
    } else {
        rows_stay(reading, laid)?;
        laid.to_vec()
    };
    for line in &mut placed {
        line.origin.x += dx;
        line.origin.y += dy;
    }
    Ok(placed)
}

fn verbatim_lines(
    reading: &Reading<'_>,
    graph: &PaintGraph,
) -> Result<Vec<PlacedLine>, SpikeError> {
    let mut lines = Vec::new();
    for (row, measured) in reading.rows.iter().zip(&reading.measured) {
        for (located, piece) in row.iter().zip(&measured.clusters) {
            let text = text_of(graph, located.ordinal)?;
            let glyph = text
                .glyphs
                .get(located.glyphs.0)
                .ok_or(SpikeError::GlyphRangeOutsideRun)?;
            let at = pen_of(text.state.ctm.value, glyph.text_matrix);
            let mut piece = piece.clone();
            piece.next = None;
            piece.rise.clear();
            lines.push(PlacedLine {
                pieces: vec![piece],
                origin: at,
                tokens: 0..0,
                overflow: false,
                gaps: vec![0.0],
            });
        }
    }
    Ok(lines)
}

fn laid_extent(style: &Style<'_>, lines: &[PlacedLine]) -> Option<[f64; 4]> {
    let mut bounds = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for line in lines.iter().filter(|line| !line.pieces.is_empty()) {
        let width: f64 = line.pieces.iter().map(|piece| piece.advance).sum();
        let em = line
            .pieces
            .iter()
            .map(|piece| style.run_em(piece.style).abs())
            .fold(0.0, f64::max);
        let (left, right) = (
            line.origin.x.min(line.origin.x + width),
            line.origin.x.max(line.origin.x + width),
        );
        bounds = [
            bounds[0].min(left - em / 2.0),
            bounds[1].min(line.origin.y - em / 2.0),
            bounds[2].max(right + em / 2.0),
            bounds[3].max(line.origin.y + 1.5 * em),
        ];
    }
    bounds
        .iter()
        .all(|value| value.is_finite())
        .then_some(bounds)
}

const SHIFT_TOLERANCE: f64 = 0.01;

fn moved_runs(graph: &PaintGraph, named: &BTreeSet<usize>) -> Vec<MovedRun> {
    named
        .iter()
        .map(|ordinal| MovedRun {
            anchor: SourceAnchor::of(&graph.atoms[*ordinal].id),
            atom_ordinal: *ordinal,
            original_matrix: match &graph.atoms[*ordinal].kind {
                PaintAtomKind::Text(text) => text.matrices.text.value,
                _ => Matrix::IDENTITY,
            },
        })
        .collect()
}

fn read_rewritten(
    page: PlannerPage<'_>,
    stream_index: usize,
    stream_write: &PlannedWrite,
    settled: Option<faces::Settled>,
) -> Result<(PaintGraph, Vec<PlannedWrite>), SpikeError> {
    let refused = || unsupported("the rewritten stream does not read back");
    let PlannedBody::ReplacedStream { decoded } = &stream_write.body else {
        return Err(refused());
    };
    let (resources, writes) = match settled {
        None => (None, Vec::new()),
        Some(settled) => (
            Some(page.program.resources.with_fonts(settled.fonts)),
            settled.writes,
        ),
    };
    let graph = crate::split::interpret_candidate_with(
        page.program,
        resources.as_ref().unwrap_or(&page.program.resources),
        stream_index,
        decoded,
        page.fonts,
    )
    .map_err(|()| refused())?;
    Ok((graph, writes))
}

fn laid_outcome(
    placed: &[PlacedLine],
    tokens: &[Token],
    (pitch, brought_in): (f64, Vec<String>),
) -> BlockOutcome {
    BlockOutcome {
        brought_in,
        caret: None,
        anchor: None,
        pitch,
        edges: (
            placed
                .iter()
                .take_while(|line| line.pieces.is_empty())
                .count(),
            placed
                .iter()
                .rev()
                .take_while(|line| line.pieces.is_empty())
                .count(),
        ),
        breaks: row_ends(placed, tokens),
        lines: placed.len(),
        overflow: placed.iter().any(|line| line.overflow),
        empty: !placed.iter().any(|line| !line.pieces.is_empty()),
        cropped: false,
    }
}

fn single_stream(
    program: &pdf_content::PageProgram,
    reference: pdf_syntax::Reference,
) -> Result<usize, SpikeError> {
    if program
        .streams
        .iter()
        .filter(|stream| stream.reference == reference)
        .count()
        != 1
    {
        return Err(SpikeError::SharedPageContentStream);
    }
    program
        .streams
        .iter()
        .position(|stream| stream.reference == reference)
        .ok_or(SpikeError::NoTextRun)
}

#[derive(Clone, Copy, Debug)]
struct Located {
    ordinal: usize,
    key: ClusterKey,
    glyphs: (usize, usize),
}

#[derive(Clone, Copy, Debug)]
struct Line {
    row: Option<usize>,
    baseline: f64,
    end: LineEnd,
    space: f64,
}

struct Reading<'g> {
    named: BTreeSet<usize>,
    outside: Outside,
    stream: pdf_syntax::Reference,
    rows: Vec<Vec<Located>>,
    style: Style<'g>,
    pitch: f64,
    measured: Vec<MeasuredRow>,
    lines: Vec<Line>,
    underlines: Vec<usize>,
    crowded: bool,
}

impl<'g> Reading<'g> {
    fn empty(
        program: &pdf_content::PageProgram,
        graph: &'g PaintGraph,
        run: &SourceAnchor,
        (left, right): (f64, f64),
    ) -> Result<Self, SpikeError> {
        if !left.is_finite() || !right.is_finite() || right <= left {
            return Err(unsupported("the frame has no width"));
        }
        let (named, stream) = crate::block_move::resolve(graph, std::slice::from_ref(run))?;
        let ordinal = *named.iter().next().ok_or(SpikeError::BlockNamesNoRun)?;
        let text = text_of(graph, ordinal)?;
        if !text.glyphs.is_empty() {
            return Err(unsupported("the block is not empty"));
        }
        let style = style_of(program, graph, &named)?;
        let (pitch, _) = line_pitch(&style, graph, &[])?;
        let baseline = pen_of(text.state.ctm.value, text.matrices.text.value).y;
        Ok(Self {
            named,
            outside: Outside::new(),
            stream,
            rows: Vec::new(),
            style,
            pitch,
            measured: Vec::new(),
            lines: vec![Line {
                row: None,
                baseline,
                end: LineEnd::Last,
                space: 0.0,
            }],
            underlines: Vec::new(),
            crowded: false,
        })
    }

    fn of(
        program: &pdf_content::PageProgram,
        (graph, operations): (&'g PaintGraph, Option<&[Vec<pdf_content::Operation>]>),
        rows: &[Vec<ClusterRef>],
        (left, right): (f64, f64),
        edges: (usize, usize),
        breaks: Option<&RowEnds>,
    ) -> Result<Self, SpikeError> {
        if !left.is_finite() || !right.is_finite() || right <= left {
            return Err(unsupported("the frame has no width"));
        }
        if rows.is_empty() || rows.iter().all(Vec::is_empty) {
            return Err(unsupported("the block has no text to lay out"));
        }
        let anchors: Vec<SourceAnchor> = rows
            .iter()
            .flatten()
            .map(|cluster| cluster.anchor.clone())
            .collect();
        let (named, stream) = crate::block_move::resolve(graph, &anchors)?;
        let (located, outside) = resolve_rows(graph, rows, &named)?;
        let mut style = style_of(program, graph, &named)?;
        let (pitch, slack) = line_pitch(&style, graph, &located)?;
        let removable = operations.and_then(|operations| {
            let index = single_stream(program, stream).ok()?;
            Some((
                operations.get(index)?.as_slice(),
                program.streams[index].bytes.as_bytes(),
            ))
        });
        let (measured, underlines, rule) = adopt_underlines(
            &style,
            graph,
            (stream, removable),
            measure_rows(&style, graph, &located)?,
        );
        if let Some(rule) = rule {
            style.rule = rule;
        }
        if measured
            .windows(2)
            .any(|pair| pair[0].baseline - pair[1].baseline <= slack)
        {
            return Err(unsupported(
                "the block's rows are closer together than its line pitch",
            ));
        }
        let crowded = measured
            .windows(2)
            .any(|pair| pair[0].baseline - pair[1].baseline < pitch - slack);
        let lines = read_lines(&style, &measured, (pitch, slack), right, edges, breaks);
        Ok(Self {
            named,
            outside,
            stream,
            rows: located,
            style,
            pitch,
            measured,
            lines,
            underlines,
            crowded,
        })
    }
}

fn rows_stay(reading: &Reading<'_>, placed: &[PlacedLine]) -> Result<(), SpikeError> {
    if !reading.crowded {
        return Ok(());
    }
    let laid: BTreeMap<ClusterKey, f64> = placed
        .iter()
        .flat_map(|line| {
            line.pieces
                .iter()
                .filter_map(move |piece| piece.kept.map(|key| (key, line.origin.y)))
        })
        .collect();
    let moved = |row: &MeasuredRow, last: bool| {
        let mut kept = row
            .clusters
            .iter()
            .filter_map(|piece| laid.get(&piece.kept?));
        let y = if last { kept.next_back() } else { kept.next() };
        y.map(|y| y - row.baseline)
    };
    let slack = ROUNDING_SLACK_EM * reading.style.em().abs();
    let apart = reading.measured.windows(2).any(|rows| {
        rows[0].baseline - rows[1].baseline < reading.pitch - slack
            && match (moved(&rows[0], true), moved(&rows[1], false)) {
                (Some(one), Some(other)) => (one - other).abs() > PLACEMENT_TOLERANCE,
                _ => false,
            }
    });
    if apart {
        return Err(unsupported(
            "the block's rows are closer together than its line pitch",
        ));
    }
    Ok(())
}

fn resolve_rows(
    graph: &PaintGraph,
    rows: &[Vec<ClusterRef>],
    named: &BTreeSet<usize>,
) -> Result<(Vec<Vec<Located>>, Outside), SpikeError> {
    let mut covered: BTreeMap<usize, Vec<Range<usize>>> = BTreeMap::new();
    let mut resolved = Vec::with_capacity(rows.len());
    let text_atoms = crate::block_move::TextAtoms::of(graph);
    for row in rows {
        let mut out = Vec::with_capacity(row.len());
        for cluster in row {
            let ordinal = text_atoms
                .named_by(graph, &cluster.anchor)
                .ok_or(SpikeError::AnchorNamesNothing)?;
            let PaintAtomKind::Text(text) = &graph.atoms[ordinal].kind else {
                return Err(SpikeError::AnchorNamesNothing);
            };
            if cluster.glyphs.start >= cluster.glyphs.end || cluster.glyphs.end > text.glyphs.len()
            {
                return Err(SpikeError::GlyphRangeOutsideRun);
            }
            covered
                .entry(ordinal)
                .or_default()
                .push(cluster.glyphs.clone());
            out.push(Located {
                ordinal,
                key: ClusterKey {
                    atom: ordinal,
                    glyph: cluster.glyphs.start,
                },
                glyphs: (cluster.glyphs.start, cluster.glyphs.end),
            });
        }
        resolved.push(out);
    }
    let mut outside = Outside::new();
    for ordinal in named {
        let PaintAtomKind::Text(text) = &graph.atoms[*ordinal].kind else {
            continue;
        };
        let mut ranges = covered.remove(ordinal).unwrap_or_default();
        ranges.sort_by_key(|range| range.start);
        let mut next = 0;
        let mut left_out = Vec::new();
        for range in ranges {
            if range.start < next {
                return Err(unsupported("a glyph of the block is named twice"));
            }
            if range.start > next {
                left_out.push(next..range.start);
            }
            next = range.end;
        }
        if next < text.glyphs.len() {
            left_out.push(next..text.glyphs.len());
        }
        if !left_out.is_empty() {
            outside.insert(*ordinal, left_out);
        }
    }
    Ok((resolved, outside))
}

type Outside = BTreeMap<usize, Vec<Range<usize>>>;

fn text_of(graph: &PaintGraph, ordinal: usize) -> Result<&TextShowPaint, SpikeError> {
    match &graph.atoms[ordinal].kind {
        PaintAtomKind::Text(text) => Ok(text),
        _ => Err(SpikeError::AnchorNamesNothing),
    }
}

fn style_of<'g>(
    program: &pdf_content::PageProgram,
    graph: &'g PaintGraph,
    named: &BTreeSet<usize>,
) -> Result<Style<'g>, SpikeError> {
    let first = *named.iter().next().ok_or(SpikeError::BlockNamesNoRun)?;
    let reference = text_of(graph, first)?;
    let block_tm = upright(reference.matrices.text.value);
    let mut runs: Vec<Run<'g>> = Vec::new();
    let mut run_of = BTreeMap::new();
    for ordinal in named {
        let text = text_of(graph, *ordinal)?;
        if !text.type3 && text.program.is_none() && text.substitution.is_none() {
            return Err(unsupported(
                "a run in the block has no embedded outline font",
            ));
        }
        if !matches!(
            text.state.text.rendering_mode.value,
            TextRenderingMode::Fill | TextRenderingMode::Stroke | TextRenderingMode::FillStroke
        ) {
            return Err(unsupported("a run in the block is invisible or clips"));
        }
        let (scale, stretch, shear) = shape_against(block_tm, text.matrices.text.value)
            .filter(|_| linear(text.state.ctm.value) == linear(reference.state.ctm.value))
            .ok_or_else(|| {
                unsupported(
                    "the block's runs are placed under different transforms (a later slice)",
                )
            })?;
        let set = set_in(text, scale, stretch);
        let index = if let Some(index) = runs.iter().position(|run| {
            (run.shear - shear).abs() <= SCALE_SLACK
                && same_setting((run.reference, &run.text), (text, &set), true)
        }) {
            index
        } else {
            runs.push(Run {
                reference: text,
                font: font_of(program, text)?,
                text: set,
                fill: colour_operator(program, text, false),
                stroke: colour_operator(program, text, true),
                fill_rgb: None,
                line_width: text.state.line_width.value,
                stroke_like_fill: text.state.text.rendering_mode.value
                    == TextRenderingMode::FillStroke
                    && strokes_like_fill(text),
                shear,
                line: None,
            });
            runs.len() - 1
        };
        run_of.insert(*ordinal, index);
    }
    let painting = |stroke: bool| {
        runs.iter()
            .filter(move |run| paints_with(run.reference.state.text.rendering_mode.value, stroke))
    };
    let writer = |run: &Run<'_>, stroke: bool| {
        if stroke {
            run.stroke.is_some()
        } else {
            run.fill.is_some()
        }
    };
    let unwritable = |stroke: bool| {
        let base = &runs[0];
        let differs =
            painting(stroke).any(|run| !same_colour(base.reference, run.reference, stroke));
        differs && !(writer(base, stroke) && painting(stroke).all(|run| writer(run, stroke)))
    };
    if unwritable(false) || unwritable(true) {
        return Err(unsupported(
            "the block mixes colours in a colour space that cannot be written yet",
        ));
    }
    let ctm = reference.state.ctm.value;
    let tm_linear = linear(block_tm);
    let to_user = linear(ctm).multiply(tm_linear);
    if to_user.b.abs() > 1e-9 || to_user.c.abs() > 1e-9 || to_user.a <= 0.0 || to_user.d <= 0.0 {
        return Err(unsupported(
            "the block is sheared or mirrored (a later slice)",
        ));
    }
    let unspaced = unspaced_runs(&mut runs);
    Ok(Style {
        unspaced,
        runs,
        run_of,
        to_user,
        ctm,
        tm_linear,
        rule: (
            underline::UNDERLINE_OFFSET_EM,
            underline::UNDERLINE_THICKNESS_EM,
        ),
    })
}

fn upright(matrix: Matrix) -> Matrix {
    if matrix.b.abs() > 1e-12 || matrix.a <= 0.0 {
        return matrix;
    }
    let shear = matrix.c / matrix.a;
    if shear.abs() <= 1e-12 || shear.abs() > MAX_SHEAR {
        return matrix;
    }
    Matrix {
        c: shear.mul_add(-matrix.a, matrix.c),
        d: shear.mul_add(-matrix.b, matrix.d),
        ..matrix
    }
}

fn unspaced_runs(runs: &mut Vec<Run<'_>>) -> BTreeMap<usize, usize> {
    let mut unspaced = BTreeMap::new();
    for index in 0..runs.len() {
        let from = &runs[index];
        if from.text.word_spacing.value == 0.0 {
            continue;
        }
        let mut text = from.text.clone();
        text.word_spacing.value = 0.0;
        let run = Run {
            reference: from.reference,
            font: from.font.clone(),
            text,
            fill: from.fill.clone(),
            stroke: from.stroke.clone(),
            fill_rgb: from.fill_rgb,
            line_width: from.line_width,
            stroke_like_fill: from.stroke_like_fill,
            shear: from.shear,
            line: from.line,
        };
        runs.push(run);
        unspaced.insert(index, runs.len() - 1);
    }
    unspaced
}

fn shape_against(block: Matrix, run: Matrix) -> Option<(f64, f64, f64)> {
    uniform_against(block, run)
        .map(|(scale, shear)| (scale, 1.0, shear))
        .or_else(|| stretched_against(block, run))
}

fn uniform_against(block: Matrix, run: Matrix) -> Option<(f64, f64)> {
    let (block, run) = (linear(block), linear(run));
    let determinant = |m: Matrix| m.a * m.d - m.b * m.c;
    let scale = (determinant(run) / determinant(block)).sqrt();
    let length = block.a * block.a + block.b * block.b;
    if !(scale.is_finite() && scale > 0.0 && length > 0.0) {
        return None;
    }
    let shear = ((run.c - scale * block.c) * block.a + (run.d - scale * block.d) * block.b)
        / (scale * length);
    (shear.abs() <= MAX_SHEAR
        && close_entry(run.a, scale * block.a)
        && close_entry(run.b, scale * block.b)
        && close_entry(run.c, scale * shear.mul_add(block.a, block.c))
        && close_entry(run.d, scale * shear.mul_add(block.b, block.d)))
    .then_some((scale, shear))
}

fn stretched_against(block: Matrix, run: Matrix) -> Option<(f64, f64, f64)> {
    let (block, run) = (linear(block), linear(run));
    let within = block.inverse()?.multiply(run);
    let (along, across) = (within.a, within.d);
    if !(along.is_finite() && across.is_finite() && along > 0.0 && across > 0.0) {
        return None;
    }
    let (scale, stretch, shear) = (across, along / across, within.c / across);
    let rebuilt = block.multiply(Matrix {
        a: along,
        b: 0.0,
        c: shear * across,
        d: across,
        e: 0.0,
        f: 0.0,
    });
    (shear.abs() <= MAX_SHEAR
        && (MIN_STRETCH..=1.0 / MIN_STRETCH).contains(&stretch)
        && close_entry(run.a, rebuilt.a)
        && close_entry(run.b, rebuilt.b)
        && close_entry(run.c, rebuilt.c)
        && close_entry(run.d, rebuilt.d))
    .then_some((scale, stretch, shear))
}

fn close_entry(one: f64, other: f64) -> bool {
    (one - other).abs() <= SCALE_SLACK * one.abs().max(other.abs()).max(1.0)
}

const MIN_STRETCH: f64 = 0.1;

const MAX_SHEAR: f64 = 1.0;

const SYNTHETIC_ITALIC_SHEAR: f64 = 0.2;

fn sheared(tm: Matrix, shear: f64) -> Matrix {
    Matrix {
        c: shear.mul_add(tm.a, tm.c),
        d: shear.mul_add(tm.b, tm.d),
        ..tm
    }
}

fn set_in(text: &TextShowPaint, scale: f64, stretch: f64) -> TextState {
    let mut state = text.state.text.clone();
    state.horizontal_scaling.value *= stretch;
    state.font_size.value *= scale;
    state.character_spacing.value *= scale;
    state.word_spacing.value *= scale;
    state.rise.value *= scale;
    state
}

fn font_of(
    program: &pdf_content::PageProgram,
    text: &TextShowPaint,
) -> Result<pdf_content::Font, SpikeError> {
    let applied = text
        .state
        .text
        .font
        .as_ref()
        .ok_or_else(|| unsupported("the block has no current font"))?;
    let resource = program
        .resources
        .font(&applied.value.name)
        .ok_or_else(|| unsupported("the block's font resource is unavailable"))?;
    if resource.reference() != applied.value.reference {
        return Err(unsupported("the block's font resource identity differs"));
    }
    resource
        .font()
        .map_err(|_| unsupported("the block's font metrics are unavailable"))
}

fn device_colour(text: &TextShowPaint, stroke: bool) -> Option<String> {
    let (colour, space) = if stroke {
        (
            &text.state.stroke_color.value,
            &text.state.stroke_color_space.value,
        )
    } else {
        (
            &text.state.fill_color.value,
            &text.state.fill_color_space.value,
        )
    };
    let (operands, operator) = match (colour, space) {
        (pdf_paint::Color::DeviceGray(g), pdf_paint::ColorSpace::DeviceGray) => {
            (format!("{g}"), "g")
        }
        (pdf_paint::Color::DeviceRgb(r, g, b), pdf_paint::ColorSpace::DeviceRgb) => {
            (format!("{r} {g} {b}"), "rg")
        }
        (pdf_paint::Color::DeviceCmyk(c, m, y, k), pdf_paint::ColorSpace::DeviceCmyk) => {
            (format!("{c} {m} {y} {k}"), "k")
        }
        _ => return None,
    };
    let operator = if stroke {
        operator.to_uppercase()
    } else {
        operator.to_owned()
    };
    Some(format!("{operands} {operator} "))
}

fn name_token(name: &[u8]) -> String {
    let name = name.strip_prefix(b"/").unwrap_or(name);
    let mut out = String::from("/");
    for byte in name {
        if byte.is_ascii_graphic() && !b"#%()/<>[]{}".contains(byte) {
            out.push(char::from(*byte));
        } else {
            let _ = write!(out, "#{byte:02X}");
        }
    }
    out
}

fn state_change(from: &Run<'_>, to: &Run<'_>) -> String {
    let writers = [&to.fill, &to.stroke];
    let (a, b) = (&from.text, &to.text);
    let (from_run, to_run) = (from, to);
    let mut out = String::new();
    if (font_key(a) != font_key(b) || a.font_size.value.to_bits() != b.font_size.value.to_bits())
        && let Some(applied) = b.font.as_ref()
    {
        let _ = write!(
            out,
            "{} {} Tf ",
            name_token(&applied.value.name),
            b.font_size.value
        );
    }
    let numbers = [
        (a.character_spacing.value, b.character_spacing.value, "Tc"),
        (a.word_spacing.value, b.word_spacing.value, "Tw"),
        (a.horizontal_scaling.value, b.horizontal_scaling.value, "Tz"),
        (a.rise.value, b.rise.value, "Ts"),
    ];
    for (was, now, operator) in numbers {
        if was.to_bits() != now.to_bits() {
            let _ = write!(out, "{now} {operator} ");
        }
    }
    if from_run.line_width.to_bits() != to_run.line_width.to_bits() {
        let _ = write!(out, "{} w ", to_run.line_width);
    }
    if a.rendering_mode.value != b.rendering_mode.value {
        let mode = match b.rendering_mode.value {
            TextRenderingMode::Stroke => 1,
            TextRenderingMode::FillStroke => 2,
            _ => 0,
        };
        let _ = write!(out, "{mode} Tr ");
    }
    for (stroke, writer) in [false, true].into_iter().zip(writers) {
        if !same_run_colour(from_run, to_run, stroke)
            && let Some(operator) = writer
        {
            out.push_str(operator);
        }
    }
    out
}

fn strokes_like_fill(text: &TextShowPaint) -> bool {
    matches!(
        text.state.text.rendering_mode.value,
        TextRenderingMode::Stroke | TextRenderingMode::FillStroke
    ) && text.state.stroke_color_space.value == text.state.fill_color_space.value
        && pdf_paint::colour_signature(&text.state.stroke_color.value)
            == pdf_paint::colour_signature(&text.state.fill_color.value)
}

fn same_run_colour(one: &Run<'_>, other: &Run<'_>, stroke: bool) -> bool {
    if stroke && (one.stroke_like_fill || other.stroke_like_fill) {
        return one.stroke_like_fill == other.stroke_like_fill
            && same_run_colour(one, other, false);
    }
    match (stroke, one.fill_rgb, other.fill_rgb) {
        (true, _, _) | (false, None, None) => same_colour(one.reference, other.reference, stroke),
        (false, Some(one), Some(other)) => one
            .iter()
            .zip(other)
            .all(|(one, other)| one.to_bits() == other.to_bits()),
        _ => false,
    }
}

fn fills_with(text: &TextShowPaint, rgb: [f64; 3]) -> bool {
    matches!(
        text.state.fill_color_space.value,
        pdf_paint::ColorSpace::DeviceRgb
    ) && match text.state.fill_color.value {
        pdf_paint::Color::DeviceRgb(red, green, blue) => [red, green, blue]
            .iter()
            .zip(rgb)
            .all(|(had, wanted)| (had - wanted).abs() < 1e-9),
        _ => false,
    }
}

fn same_colour(one: &TextShowPaint, other: &TextShowPaint, stroke: bool) -> bool {
    let (a, b) = (&one.state, &other.state);
    if stroke {
        a.stroke_color_space.value == b.stroke_color_space.value
            && pdf_paint::colour_signature(&a.stroke_color.value)
                == pdf_paint::colour_signature(&b.stroke_color.value)
    } else {
        a.fill_color_space.value == b.fill_color_space.value
            && pdf_paint::colour_signature(&a.fill_color.value)
                == pdf_paint::colour_signature(&b.fill_color.value)
    }
}

fn colour_operator(
    program: &pdf_content::PageProgram,
    text: &TextShowPaint,
    stroke: bool,
) -> Option<String> {
    if let Some(device) = device_colour(text, stroke) {
        return Some(device);
    }
    let (colour, space) = if stroke {
        (
            &text.state.stroke_color.value,
            &text.state.stroke_color_space,
        )
    } else {
        (&text.state.fill_color.value, &text.state.fill_color_space)
    };
    let components: Vec<f64> = match colour {
        pdf_paint::Color::CalGray(g) | pdf_paint::Color::Separation(g) => vec![*g],
        pdf_paint::Color::CalRgb(r, g, b) | pdf_paint::Color::Lab(r, g, b) => vec![*r, *g, *b],
        pdf_paint::Color::IccBased(values) | pdf_paint::Color::DeviceN(values) => values.clone(),
        pdf_paint::Color::Indexed(index) => vec![f64::from(*index)],
        _ => return None,
    };
    let selected = space.provenance.last()?;
    let stream = program
        .streams
        .iter()
        .find(|stream| stream.bytes.id() == selected.source())?;
    let written = stream
        .bytes
        .as_bytes()
        .get(selected.start()..selected.end())?;
    let written = std::str::from_utf8(written).ok()?;
    let mut tokens = written.split_ascii_whitespace();
    let (name, operator) = (tokens.next()?, tokens.next()?);
    let wanted = if stroke { "CS" } else { "cs" };
    if tokens.next().is_some()
        || operator != wanted
        || !name.starts_with('/')
        || name == "/Pattern"
        || name.len() < 2
    {
        return None;
    }
    let values: Vec<String> = components.iter().map(f64::to_string).collect();
    let set = if stroke { "SCN" } else { "scn" };
    Some(format!("{name} {wanted} {} {set} ", values.join(" ")))
}

fn segments(pieces: &[Piece]) -> Vec<&[Piece]> {
    let mut out = Vec::new();
    let mut start = 0;
    for index in 1..=pieces.len() {
        if index == pieces.len() || pieces[index].style != pieces[start].style {
            if start < index {
                out.push(&pieces[start..index]);
            }
            start = index;
        }
    }
    out
}

fn same_setting(
    (one, one_text): (&TextShowPaint, &TextState),
    (other, other_text): (&TextShowPaint, &TextState),
    fill: bool,
) -> bool {
    let (a, b) = (&one.state, &other.state);
    let (at, bt) = (one_text, other_text);
    font_key(at) == font_key(bt)
        && at.font_size.value.to_bits() == bt.font_size.value.to_bits()
        && at.character_spacing.value.to_bits() == bt.character_spacing.value.to_bits()
        && at.word_spacing.value.to_bits() == bt.word_spacing.value.to_bits()
        && at.horizontal_scaling.value.to_bits() == bt.horizontal_scaling.value.to_bits()
        && at.rise.value.to_bits() == bt.rise.value.to_bits()
        && at.rendering_mode.value == bt.rendering_mode.value
        && linear(a.ctm.value) == linear(b.ctm.value)
        && (!fill || !paints_with(at.rendering_mode.value, false) || same_colour(one, other, false))
        && (!paints_with(at.rendering_mode.value, true)
            || (a.line_width.value.to_bits() == b.line_width.value.to_bits()
                && (!fill || same_colour(one, other, true))))
}

fn font_key(text: &TextState) -> Option<(&[u8], Option<pdf_syntax::Reference>)> {
    text.font.as_ref().map(|applied| {
        let name = &applied.value.name;
        (
            name.strip_prefix(b"/").unwrap_or(name),
            applied.value.reference,
        )
    })
}

const fn paints_with(mode: TextRenderingMode, stroke: bool) -> bool {
    if stroke {
        matches!(
            mode,
            TextRenderingMode::Stroke
                | TextRenderingMode::FillStroke
                | TextRenderingMode::StrokeClip
                | TextRenderingMode::FillStrokeClip
        )
    } else {
        matches!(
            mode,
            TextRenderingMode::Fill
                | TextRenderingMode::FillStroke
                | TextRenderingMode::FillClip
                | TextRenderingMode::FillStrokeClip
        )
    }
}

fn advance_of(style: &Style<'_>, run: usize, code: &pdf_content::SourceCode) -> f64 {
    let run = style.run(run);
    let source_span = match run.reference.elements.first() {
        Some(
            TextShowElement::Codes { source_span, .. }
            | TextShowElement::Adjustment { source_span, .. },
        ) => *source_span,
        None => return 0.0,
    };
    let (_, pen) = pdf_paint::position_text(
        &run.text,
        &[TextShowElement::Codes {
            source_span,
            decoded_bytes: code.bytes.clone(),
            codes: vec![code.clone()],
        }],
        Matrix::IDENTITY,
        run.reference.program.as_deref(),
        &run.font,
        0.001,
    );
    style.to_user.a * pen.e
}

fn kern_of(style: &Style<'_>, run: usize, value: f64) -> f64 {
    let text = &style.run(run).text;
    style.to_user.a
        * (-value / 1000.0 * text.font_size.value * text.horizontal_scaling.value / 100.0)
}

fn adjustments_after(text: &TextShowPaint) -> Vec<f64> {
    let mut after = vec![0.0; text.glyphs.len()];
    let mut seen = 0_usize;
    for element in &text.elements {
        match element {
            TextShowElement::Codes { codes, .. } => seen += codes.len(),
            TextShowElement::Adjustment { value, .. } => {
                if let Some(slot) = seen.checked_sub(1).and_then(|last| after.get_mut(last)) {
                    *slot += value;
                }
            }
        }
    }
    after
}

fn inner_kern(style: &Style<'_>, piece: &Piece) -> f64 {
    let count = if piece.shaped.is_empty() {
        piece.adjust.len().saturating_sub(1)
    } else {
        piece.adjust.len()
    };
    piece
        .adjust
        .iter()
        .take(count)
        .map(|value| kern_of(style, piece.style, *value))
        .sum()
}

fn kern_between(style: &Style<'_>, piece: &Piece, next: Option<&Piece>) -> f64 {
    match (
        piece.next,
        next.and_then(|after| after.kept),
        piece.adjust.last(),
    ) {
        (Some(was), Some(now), Some(value)) if was == now => kern_of(style, piece.style, *value),
        (None, _, Some(value))
            if next.is_some() && piece.kept.is_some() && piece.shaped.is_empty() =>
        {
            kern_of(style, piece.style, *value)
        }
        _ => 0.0,
    }
}

struct MeasuredRow {
    clusters: Vec<Piece>,
    start: f64,
    end: f64,
    baseline: f64,
}

fn measure_rows(
    style: &Style<'_>,
    graph: &PaintGraph,
    rows: &[Vec<Located>],
) -> Result<Vec<MeasuredRow>, SpikeError> {
    let mut measured = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(first) = row.first() else {
            return Err(unsupported("the block has an empty row"));
        };
        let first_text = text_of(graph, first.ordinal)?;
        let pen = pen_of(
            first_text.state.ctm.value,
            first_text.glyphs[first.glyphs.0].text_matrix,
        );
        let mut clusters = Vec::with_capacity(row.len());
        let mut x = pen.x;
        let mut end = pen.x;
        for (position, located) in row.iter().enumerate() {
            let text = text_of(graph, located.ordinal)?;
            let adjustments = adjustments_after(text);
            let run = style.run_of.get(&located.ordinal).copied().unwrap_or(0);
            let mut piece = Piece {
                codes: Vec::new(),
                text: String::new(),
                advance: 0.0,
                kept: Some(located.key),
                group: 0,
                style: run,
                adjust: adjustments
                    .get(located.glyphs.0..located.glyphs.1)
                    .map(<[f64]>::to_vec)
                    .unwrap_or_default(),
                next: row
                    .get(position + 1)
                    .filter(|after| {
                        after.ordinal == located.ordinal && after.glyphs.0 == located.glyphs.1
                    })
                    .map(|after| after.key),
                break_before: false,
                shaped: Vec::new(),
                rise: Vec::new(),
                underline: false,
            };
            for glyph in &text.glyphs[located.glyphs.0..located.glyphs.1] {
                let meaning = text.text.text_of(Code {
                    value: glyph.code.value,
                    byte_len: glyph.code.bytes.len(),
                });
                match meaning {
                    Some(meaning) if !meaning.text.is_empty() => {
                        piece.text.push_str(&meaning.text);
                    }
                    _ => piece.text.push_str(UNREAD),
                }
                piece.advance += advance_of(style, run, &glyph.code);
                piece.codes.push(glyph.code.clone());
            }
            piece.advance += inner_kern(style, &piece);
            let at = pen_of(
                text.state.ctm.value,
                text.glyphs[located.glyphs.0].text_matrix,
            );
            let em = style.run_em(run).abs();
            let lifted = at.y - pen.y;
            if lifted.abs() > 1e-3 * em && style.to_user.d != 0.0 {
                piece.rise = vec![lifted / style.to_user.d; piece.codes.len()];
            }
            if piece.next.is_none()
                && let Some(after) = row.get(position + 1)
                && after.ordinal != located.ordinal
            {
                let after_text = text_of(graph, after.ordinal)?;
                let distance = pen_of(
                    after_text.state.ctm.value,
                    after_text.glyphs[after.glyphs.0].text_matrix,
                )
                .x - at.x;
                if let Some(value) = kept_distance(style, run, piece.advance, distance)
                    && let Some(last) = piece.adjust.last_mut()
                {
                    *last = value;
                    piece.next = Some(after.key);
                }
            }
            x += piece.advance;
            if !is_space(&piece.text) {
                end = x;
            }
            if piece.next.is_some()
                && let Some(value) = piece.adjust.last()
            {
                x += kern_of(style, run, *value);
            }
            clusters.push(piece);
        }
        measured.push(MeasuredRow {
            clusters,
            start: pen.x,
            end,
            baseline: pen.y,
        });
    }
    Ok(measured)
}

fn kept_distance(style: &Style<'_>, run: usize, advance: f64, distance: f64) -> Option<f64> {
    let per = kern_of(style, run, 1.0);
    let em = style.run_em(run).abs();
    (per != 0.0 && (distance - advance).abs() > 1e-3 * em && distance >= -em)
        .then(|| (distance - advance) / per)
}

fn pen_of(ctm: Matrix, text_matrix: Matrix) -> Point {
    ctm.transform(Point {
        x: text_matrix.e,
        y: text_matrix.f,
    })
}

fn is_space(text: &str) -> bool {
    !text.is_empty() && text.chars().all(char::is_whitespace)
}

fn line_pitch(
    style: &Style<'_>,
    graph: &PaintGraph,
    rows: &[Vec<Located>],
) -> Result<(f64, f64), SpikeError> {
    let slack = PITCH_SLACK_EM * style.em().abs();
    let baselines: Vec<f64> = rows
        .iter()
        .filter_map(|row| row.first())
        .map(|first| {
            text_of(graph, first.ordinal).map(|text| {
                pen_of(
                    text.state.ctm.value,
                    text.glyphs[first.glyphs.0].text_matrix,
                )
                .y
            })
        })
        .collect::<Result<_, _>>()?;
    let base = style.em().abs();
    let grown = |row: &Vec<Located>| {
        let tallest = row
            .iter()
            .filter_map(|located| style.run_of.get(&located.ordinal))
            .map(|run| style.run_em(*run).abs())
            .fold(0.0, f64::max);
        if base > 0.0 {
            (tallest / base).max(1.0)
        } else {
            1.0
        }
    };
    let gaps: Vec<f64> = baselines
        .windows(2)
        .zip(rows.iter().filter(|row| !row.is_empty()).skip(1))
        .map(|(pair, below)| (pair[0] - pair[1]) / grown(below))
        .collect();
    let whole_multiples_of = |unit: f64| {
        unit > slack
            && gaps.iter().all(|gap| {
                let steps = (gap / unit).round();
                steps >= 1.0 && (gap - steps * unit).abs() <= slack
            })
    };
    let floor = LINE_FLOOR_EM * style.em().abs();
    let leading = Some(style.base().reference.state.text.leading.value * style.to_user.d)
        .filter(|leading| *leading >= floor)
        .unwrap_or(0.0);
    if !gaps.is_empty() && whole_multiples_of(leading) {
        return Ok((leading, slack));
    }
    if let Some(smallest) = gaps.iter().copied().reduce(f64::min)
        && whole_multiples_of(smallest)
    {
        return Ok((smallest, slack));
    }
    let stated = if leading > 0.0 {
        Some(leading)
    } else {
        style
            .base()
            .reference
            .line_metrics()
            .map(|(ascent, descent)| (ascent - descent) * style.em())
            .filter(|pitch| *pitch >= floor)
    };
    let stepped_at = |pitch: f64| gaps.is_empty() || gaps.iter().any(|gap| *gap < 1.5 * pitch);
    if let Some(pitch) = stated
        && gaps.iter().all(|gap| *gap >= pitch - slack)
        && stepped_at(pitch)
    {
        return Ok((pitch, slack));
    }
    let rounding = ROUNDING_SLACK_EM * style.em().abs();
    if let Some(pitch) = rounded_pitch(&gaps, style.em().abs()) {
        return Ok((pitch, rounding));
    }
    if let Some(pitch) = stated
        && !stepped_at(pitch)
        && let Some(smallest) = gaps.iter().copied().reduce(f64::min)
        && smallest >= LINE_FLOOR_EM * style.em().abs()
    {
        return Ok((smallest, rounding));
    }
    if let Some(pitch) = stated {
        return Ok((pitch, slack));
    }
    if let Some(smallest) = gaps.iter().copied().reduce(f64::min)
        && smallest >= floor
    {
        return Ok((smallest, rounding));
    }
    Ok((DEFAULT_LINE_EM * style.em().abs(), slack))
}

fn rounded_pitch(gaps: &[f64], em: f64) -> Option<f64> {
    let tolerance = ROUNDING_SLACK_EM * em;
    let smallest = gaps.iter().copied().reduce(f64::min)?;
    if !smallest.is_finite() || smallest <= 0.0 {
        return None;
    }
    let steps: Vec<f64> = gaps.iter().map(|gap| (gap / smallest).round()).collect();
    let total: f64 = steps.iter().sum();
    if steps.iter().any(|step| *step < 1.0) {
        return None;
    }
    let pitch = gaps.iter().sum::<f64>() / total;
    if pitch < LINE_FLOOR_EM * em {
        return None;
    }
    let (mut down, mut counted) = (0.0, 0.0);
    for (gap, step) in gaps.iter().zip(&steps) {
        down += gap;
        counted += step;
        if (down - counted * pitch).abs() > tolerance {
            return None;
        }
    }
    Some(pitch)
}

fn read_lines(
    style: &Style<'_>,
    rows: &[MeasuredRow],
    (pitch, short): (f64, f64),
    right: f64,
    (leading, trailing): (usize, usize),
    breaks: Option<&RowEnds>,
) -> Vec<Line> {
    let slack = PITCH_SLACK_EM * style.em().abs();
    let known = breaks.filter(|listed| {
        listed
            .paragraphs
            .iter()
            .chain(&listed.lines)
            .all(|row| *row < rows.len())
    });
    let first = rows.first().map_or(0.0, |row| row.baseline);
    let mut lines: Vec<Line> = (0..leading)
        .map(|index| Line {
            row: None,
            baseline: first + pitch * steps_f64(leading - index),
            end: LineEnd::Paragraph,
            space: 0.0,
        })
        .collect();
    for (index, row) in rows.iter().enumerate() {
        let opens = lines
            .last()
            .is_none_or(|line| !matches!(line.end, LineEnd::Wrap | LineEnd::WrapWithSpace));
        lines.push(Line {
            row: Some(index),
            baseline: row.baseline,
            end: LineEnd::Last,
            space: 0.0,
        });
        let Some(next) = rows.get(index + 1) else {
            break;
        };
        let gap = row.baseline - next.baseline;
        let (steps, leftover) = lines_down(gap, row_pitch(style, next, pitch), (pitch, short));
        if steps >= 2.0 {
            set_end(&mut lines, LineEnd::Paragraph, 0.0);
            let empty = count_of(steps) - 1;
            for step in 1..=empty {
                lines.push(Line {
                    row: None,
                    baseline: row.baseline - pitch * steps_f64(step),
                    end: LineEnd::Paragraph,
                    space: 0.0,
                });
            }
            if leftover > slack {
                set_end(&mut lines, LineEnd::Paragraph, leftover);
            }
            continue;
        }
        let end = match known {
            Some(listed) if listed.paragraphs.contains(&index) => LineEnd::Paragraph,
            Some(listed) if listed.lines.contains(&index) => LineEnd::Line,
            Some(_) => LineEnd::Wrap,
            None => {
                let run = row.clusters.last().map_or(0, |piece| piece.style);
                let end = row_end(
                    (row, opens),
                    next,
                    pitch,
                    right,
                    space_piece(style, run).as_ref(),
                );
                let apart =
                    gap - row_pitch(style, next, pitch) > ROUNDING_SLACK_EM * style.em().abs();
                let close = crowded_by(style, row, next, pitch);
                if (apart || close) && matches!(end, LineEnd::Wrap | LineEnd::WrapWithSpace) {
                    LineEnd::Paragraph
                } else {
                    end
                }
            }
        };
        let listed = known.is_some_and(|listed| listed.lists(index));
        let leftover =
            if listed || (end == LineEnd::Paragraph && crowded_by(style, row, next, pitch)) {
                gap - row_pitch(style, next, pitch)
            } else if end == LineEnd::Paragraph && gap - pitch > slack {
                gap - pitch
            } else {
                0.0
            };
        set_end(&mut lines, end, leftover);
    }
    if trailing > 0 {
        let last = rows.last().map_or(first, |row| row.baseline);
        set_end(&mut lines, LineEnd::Paragraph, 0.0);
        for step in 1..=trailing {
            lines.push(Line {
                row: None,
                baseline: last - pitch * steps_f64(step),
                end: if step == trailing {
                    LineEnd::Last
                } else {
                    LineEnd::Paragraph
                },
                space: 0.0,
            });
        }
    }
    lines
}

fn crowded_by(style: &Style<'_>, row: &MeasuredRow, next: &MeasuredRow, pitch: f64) -> bool {
    row_pitch(style, next, pitch) - (row.baseline - next.baseline)
        > ROUNDING_SLACK_EM * style.em().abs()
}

fn lines_down(gap: f64, own: f64, (pitch, short): (f64, f64)) -> (f64, f64) {
    let steps = ((gap - own + short) / pitch).floor() + 1.0;
    (steps, gap - own - (steps - 1.0) * pitch)
}

fn row_pitch(style: &Style<'_>, row: &MeasuredRow, pitch: f64) -> f64 {
    row.clusters
        .iter()
        .map(|piece| pitch * (style.run_em(piece.style) / style.em()).max(1.0))
        .fold(pitch, f64::max)
}

fn set_end(lines: &mut [Line], end: LineEnd, space: f64) {
    if let Some(last) = lines.last_mut() {
        last.end = end;
        last.space = space;
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "a count of lines in one block, far below 2^52"
)]
fn steps_f64(count: usize) -> f64 {
    count as f64
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a whole, positive number of lines, checked by the caller"
)]
fn count_of(steps: f64) -> usize {
    steps as usize
}

fn row_end(
    (row, opens): (&MeasuredRow, bool),
    next: &MeasuredRow,
    pitch: f64,
    right: f64,
    space: Option<&Piece>,
) -> LineEnd {
    let first_word = first_word_width(next);
    let ends_in_space = row
        .clusters
        .last()
        .is_some_and(|piece| is_space(&piece.text));
    let starts_with_space = next
        .clusters
        .first()
        .is_some_and(|piece| is_space(&piece.text));
    let joined_text = format!("{}{}", row_text(row), row_text(next));
    let at = row_text(row).len();
    let boundaries: Vec<usize> = piece_starts(row.clusters.iter().chain(&next.clusters));
    let breaks_there = unread_joint(row, next)
        || line_break_opportunities(&joined_text, &boundaries).contains(&at);
    let needs_space = !ends_in_space && !starts_with_space && !breaks_there;
    let space_width = if needs_space {
        space.map_or(f64::INFINITY, |piece| piece.advance)
    } else {
        0.0
    };
    let indent = row.start - next.start;
    let first_line_indent = opens
        && indent.abs() > ALIGN_SLACK
        && indent.abs() <= FIRST_INDENT_PITCHES * pitch
        && row.end - next.start >= INDENTED_ROW_FILLS * (right - next.start)
        && (indent > 0.0 || starts_with_list_marker(&row_text(row)));
    let hard = ((next.start - row.start).abs() > ALIGN_SLACK && !first_line_indent)
        || row.baseline - next.baseline > 1.5 * pitch
        || row.end + space_width + first_word <= right + FIT_SLACK;
    if hard {
        return LineEnd::Paragraph;
    }
    let first_cluster = next
        .clusters
        .first()
        .map_or(f64::INFINITY, |piece| piece.advance);
    let split_word = needs_space
        && row.end + first_cluster > right + FIT_SLACK
        && trailing_word_width(row) >= row.end - row.start - FIT_SLACK
        && trailing_word_width(row) + first_word > right - next.start + FIT_SLACK;
    if !needs_space || split_word {
        return LineEnd::Wrap;
    }
    if space.is_none() {
        return LineEnd::Paragraph;
    }
    LineEnd::WrapWithSpace
}

fn unread_joint(row: &MeasuredRow, next: &MeasuredRow) -> bool {
    let unread = |piece: Option<&Piece>| piece.is_some_and(|piece| piece.text.contains(UNREAD));
    unread(row.clusters.last()) || unread(next.clusters.first())
}

fn row_text(row: &MeasuredRow) -> String {
    row.clusters
        .iter()
        .map(|piece| piece.text.as_str())
        .collect()
}

fn piece_starts<'a>(pieces: impl Iterator<Item = &'a Piece>) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut offset = 0;
    for piece in pieces {
        starts.push(offset);
        offset += piece.text.len();
    }
    starts
}

fn row_ends(placed: &[PlacedLine], tokens: &[Token]) -> RowEnds {
    let mut ends = RowEnds::default();
    let mut row = 0;
    for (index, line) in placed.iter().enumerate() {
        if line.pieces.is_empty() {
            continue;
        }
        if let Some(next) = placed.get(index + 1)
            && (next.pieces.is_empty() || next.tokens.start > line.tokens.end)
        {
            let after = tokens.get(line.tokens.end..).unwrap_or_default();
            match after
                .iter()
                .find(|token| !matches!(token, Token::Cluster(_)))
            {
                Some(Token::LineBreak) => ends.lines.push(row),
                _ => ends.paragraphs.push(row),
            }
        }
        row += 1;
    }
    ends
}

fn trailing_word_width(row: &MeasuredRow) -> f64 {
    let text = row_text(row);
    let starts = piece_starts(row.clusters.iter());
    let last_break = line_break_opportunities(&text, &starts)
        .into_iter()
        .filter(|at| *at < text.len())
        .max()
        .unwrap_or(0);
    row.clusters
        .iter()
        .zip(&starts)
        .filter(|(_, start)| **start >= last_break)
        .filter(|(piece, _)| !is_space(&piece.text))
        .map(|(piece, _)| piece.advance)
        .sum()
}

fn first_word_width(row: &MeasuredRow) -> f64 {
    let text = row_text(row);
    let starts = piece_starts(row.clusters.iter());
    let first_break = line_break_opportunities(&text, &starts)
        .first()
        .copied()
        .unwrap_or(text.len());
    row.clusters
        .iter()
        .zip(&starts)
        .take_while(|(_, start)| **start < first_break)
        .filter(|(piece, _)| !is_space(&piece.text))
        .map(|(piece, _)| piece.advance)
        .sum()
}

#[must_use]
pub fn in_compatibility_form(text: &str) -> String {
    compatibility_form(text).unwrap_or_else(|| text.to_owned())
}

fn compatibility_form(cluster: &str) -> Option<String> {
    if !cluster
        .chars()
        .any(|character| matches!(character, '\u{0E33}' | '\u{0EB3}' | '\u{0EDC}' | '\u{0EDD}'))
    {
        return None;
    }
    Some(
        cluster
            .chars()
            .flat_map(|character| match character {
                '\u{0E33}' => vec!['\u{0E4D}', '\u{0E32}'],
                '\u{0EB3}' => vec!['\u{0ECD}', '\u{0EB2}'],
                '\u{0EDC}' => vec!['\u{0EAB}', '\u{0E99}'],
                '\u{0EDD}' => vec!['\u{0EAB}', '\u{0EA1}'],
                other => vec![other],
            })
            .collect(),
    )
}

fn same_look(style: &Style<'_>, one: usize, other: usize) -> bool {
    let (a, b) = (style.run(one), style.run(other));
    let state = |run: &Run<'_>| {
        (
            run.text.font_size.value.to_bits(),
            run.text.character_spacing.value.to_bits(),
            run.text.word_spacing.value.to_bits(),
            run.text.horizontal_scaling.value.to_bits(),
            run.text.rise.value.to_bits(),
            run.text.rendering_mode.value,
        )
    };
    state(a) == state(b)
        && a.fill == b.fill
        && a.stroke == b.stroke
        && a.fill_rgb == b.fill_rgb
        && a.stroke_like_fill == b.stroke_like_fill
        && a.shear.to_bits() == b.shear.to_bits()
        && a.line_width.to_bits() == b.line_width.to_bits()
}

fn space_piece(style: &Style<'_>, run: usize) -> Option<Piece> {
    typed_pieces(style, None, " ", 0, (run, &[]))
        .ok()
        .or_else(|| typed_pieces(style, None, " ", 0, (0, &[])).ok())?
        .into_iter()
        .next()
}

fn line_groups(lines: &[Line]) -> Vec<usize> {
    let mut group = 0;
    lines
        .iter()
        .map(|line| {
            let mine = group;
            if line.end == LineEnd::Paragraph {
                group += 1;
            }
            mine
        })
        .collect()
}

fn starts_with_list_marker(text: &str) -> bool {
    let text = text.trim_start();
    let mut characters = text.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    if matches!(
        first,
        '-' | '\u{2013}'
            | '\u{2014}'
            | '\u{2022}'
            | '\u{25CF}'
            | '\u{25CB}'
            | '\u{25AA}'
            | '*'
            | '\u{F0B7}'
            | '\u{F0A7}'
            | '\u{F076}'
    ) {
        return true;
    }
    let label = |character: char| {
        character.is_ascii_alphanumeric()
            || ('\u{0ED0}'..='\u{0ED9}').contains(&character)
            || ('\u{0E50}'..='\u{0E59}').contains(&character)
    };
    let body = text.strip_prefix('(').unwrap_or(text);
    let digits = body
        .chars()
        .take_while(|character| label(*character))
        .count();
    let mut after = body.chars().skip(digits);
    (1..=3).contains(&digits)
        && after
            .next()
            .is_some_and(|close| close == '.' || close == ')')
        && after.next().is_none_or(char::is_whitespace)
}

fn alignments(
    reading: &Reading<'_>,
    groups: &[usize],
    (left, right): (f64, f64),
    flowing: bool,
) -> Vec<(Alignment, f64, f64)> {
    let count = groups.last().map_or(0, |last| last + 1);
    if flowing {
        return vec![(Alignment::Start, left, 0.0); count];
    }
    let em = reading.style.base().reference.state.text.font_size.value * reading.style.to_user.a;
    let mut out: Vec<(Alignment, f64, f64)> = Vec::with_capacity(count);
    for group in 0..count {
        let members: Vec<&MeasuredRow> = reading
            .lines
            .iter()
            .zip(groups)
            .filter(|(_, owner)| **owner == group)
            .filter_map(|(line, _)| line.row.map(|row| &reading.measured[row]))
            .collect();
        if members.is_empty() {
            out.push(out.last().copied().unwrap_or((Alignment::Start, left, 0.0)));
            continue;
        }
        let near = |one: f64, other: f64| (one - other).abs() <= ALIGN_SLACK;
        let first = members.first().map_or(left, |row| row.start);
        let centre = f64::midpoint(left, right);
        let all = |test: &dyn Fn(&MeasuredRow) -> bool| members.iter().all(|row| test(row));
        let roomy = |row: &MeasuredRow| row.start - left >= em;
        let rest = members.get(1).map(|row| row.start);
        if let Some(rest) = rest
            && !near(first, rest)
            && members[1..].iter().all(|row| near(row.start, rest))
        {
            out.push((Alignment::Start, rest, first - rest));
            continue;
        }
        out.push(
            if (members.len() > 1 && all(&|row| near(row.start, first))) || near(first, left) {
                (Alignment::Start, first, 0.0)
            } else if all(&|row| {
                roomy(row)
                    && right - row.end >= em
                    && near(f64::midpoint(row.start, row.end), centre)
            }) {
                (Alignment::Centre, left, 0.0)
            } else if all(&|row| roomy(row) && near(row.end, right)) {
                (Alignment::End, left, 0.0)
            } else {
                (Alignment::Start, first, 0.0)
            },
        );
    }
    out
}

fn reopens_a_break(reading: &Reading<'_>, index: usize, row: usize) -> bool {
    index > 0
        && reading.lines[index - 1].end == LineEnd::Wrap
        && reading.lines[index - 1]
            .row
            .is_some_and(|before| unread_joint(&reading.measured[before], &reading.measured[row]))
}

fn reading_tokens(reading: &Reading<'_>, groups: &[usize]) -> (Vec<Token>, Vec<Vec<usize>>) {
    let mut tokens = Vec::new();
    let mut stops: Vec<Vec<usize>> = Vec::with_capacity(reading.lines.len());
    for (index, line) in reading.lines.iter().enumerate() {
        match line.row {
            Some(row) => {
                let clusters = &reading.measured[row].clusters;
                let reopens = reopens_a_break(reading, index, row);
                let mut line_stops = Vec::with_capacity(clusters.len() + 1);
                for (position, piece) in clusters.iter().enumerate() {
                    line_stops.push(tokens.len());
                    let mut piece = piece.clone();
                    piece.group = groups[index];
                    piece.break_before = reopens && position == 0;
                    tokens.push(Token::Cluster(piece));
                }
                line_stops.push(tokens.len());
                stops.push(line_stops);
            }
            None => stops.push(vec![tokens.len()]),
        }
        match line.end {
            LineEnd::Paragraph => tokens.push(Token::Break { space: line.space }),
            LineEnd::Line => tokens.push(Token::LineBreak),
            LineEnd::WrapWithSpace => {
                let run = line
                    .row
                    .and_then(|row| reading.measured[row].clusters.last())
                    .map_or(0, |piece| piece.style);
                if let Some(mut piece) = space_piece(&reading.style, run) {
                    piece.group = groups[index];
                    tokens.push(Token::Cluster(piece));
                }
            }
            LineEnd::Wrap | LineEnd::Last => {}
        }
    }
    (tokens, stops)
}

fn last_character_off(text: &str) -> Option<String> {
    if text.contains(UNREAD) {
        return None;
    }
    let mut characters: Vec<char> = text.chars().collect();
    let last = characters.pop()?;
    let joins = |character: char| matches!(u32::from(character), 0x200C | 0x200D | 0xFE00..=0xFE0F | 0xE0100..=0xE01EF);
    if characters.is_empty() || joins(last) || characters.last().copied().is_some_and(joins) {
        return None;
    }
    Some(characters.into_iter().collect())
}

fn joins_cluster_before(letter: &str, typed: &str) -> bool {
    if letter.is_empty() || typed.is_empty() || letter.contains(UNREAD) {
        return false;
    }
    let joined = format!("{letter}{typed}");
    GraphemeClusterSegmenter::new()
        .segment_str(&joined)
        .filter(|offset| *offset > 0)
        .all(|offset| offset != letter.len())
}

fn past_marks(tokens: &[Token], mut position: usize) -> usize {
    let starts_with_mark = |token: &Token| match token {
        Token::Cluster(piece) => joins_cluster_before("a", &piece.text),
        Token::Break { .. } | Token::LineBreak => false,
    };
    let mut letter = position;
    while letter > 0 && tokens.get(letter - 1).is_some_and(starts_with_mark) {
        letter -= 1;
    }
    if letter == 0 || !matches!(tokens.get(letter - 1), Some(Token::Cluster(_))) {
        return position;
    }
    while tokens.get(position).is_some_and(starts_with_mark) {
        position += 1;
    }
    position
}

fn shape_with_the_letter_before(
    tokens: &[Token],
    from: usize,
    typed: &mut String,
    style: &Style<'_>,
    (group, run, around): (usize, usize, &[usize]),
) -> usize {
    let mut start = from;
    while start > 0
        && let Some(Token::Cluster(before)) = tokens.get(start - 1)
        && before.group == group
    {
        start -= 1;
        if !joins_cluster_before("a", &before.text) {
            break;
        }
    }
    let letter: String = tokens[start..from]
        .iter()
        .filter_map(|token| match token {
            Token::Cluster(piece) => Some(piece.text.as_str()),
            Token::Break { .. } | Token::LineBreak => None,
        })
        .collect();
    if start == from || !joins_cluster_before(&letter, typed) {
        return from;
    }
    let joined = format!("{letter}{typed}");
    let cluster_end = GraphemeClusterSegmenter::new()
        .segment_str(&joined)
        .find(|offset| *offset > 0)
        .unwrap_or(joined.len());
    let alone = &joined[letter.len()..cluster_end.max(letter.len())];
    if alone.is_empty() || typed_pieces(style, None, alone, group, (run, around)).is_err() {
        typed.insert_str(0, &letter);
        return start;
    }
    from
}

#[must_use]
pub fn insertion_after_marks(before: &str, after: &str) -> usize {
    let mark = |character: char| joins_cluster_before("a", character.encode_utf8(&mut [0; 4]));
    let letter = before.trim_end_matches(mark);
    if letter.is_empty() || letter.ends_with(['\n', LINE_BREAK]) {
        return 0;
    }
    after
        .chars()
        .take_while(|character| mark(*character))
        .count()
}

fn edited_range(
    tokens: &[Token],
    stops: &[Vec<usize>],
    edit: &BlockEdit<'_>,
) -> Result<(usize, usize), SpikeError> {
    let stop = |(line, offset): (usize, usize)| {
        stops
            .get(line)
            .and_then(|line| line.get(offset))
            .copied()
            .ok_or(SpikeError::GlyphRangeOutsideRun)
    };
    match edit.range {
        BlockRange::Between { from, to } => {
            let (one, other) = (stop(from)?, stop(to)?);
            Ok((
                past_marks(tokens, one.min(other)),
                past_marks(tokens, one.max(other)),
            ))
        }
        BlockRange::Units {
            at,
            backwards,
            count,
        } => {
            if !edit.text.is_empty() {
                return Err(unsupported("a delete carries no text"));
            }
            let at = stop(at)?;
            let (from, to) = if backwards {
                (at.saturating_sub(count), at)
            } else {
                (at, at.saturating_add(count).min(tokens.len()))
            };
            if from == to {
                return Err(unsupported("there is nothing there to delete"));
            }
            Ok((from, to))
        }
    }
}

fn edited_tokens(
    reading: &Reading<'_>,
    groups: &[usize],
    edit: &BlockEdit<'_>,
    faces: &mut faces::NewFaces<'_>,
) -> Result<(Vec<Token>, usize, Range<usize>), SpikeError> {
    let (mut tokens, stops) = reading_tokens(reading, groups);
    let (mut from, to) = edited_range(&tokens, &stops, edit)?;
    let retyped = match (edit.range, tokens.get(from)) {
        (
            BlockRange::Units {
                backwards: true,
                count: 1,
                ..
            },
            Some(Token::Cluster(piece)),
        ) if to == from + 1 && edit.style.is_none() => last_character_off(&piece.text)
            .map(|rest| (rest, piece.group, piece.style, piece.underline)),
        _ => None,
    };
    if edit.style.is_some() {
        styled_selection(edit, from..to)?;
        return Ok((tokens, to, from..to));
    }
    let (group, run, underline) = retyped.as_ref().map_or_else(
        || {
            let beside = || {
                tokens[..from]
                    .iter()
                    .rev()
                    .chain(tokens[to..].iter())
                    .filter_map(|token| match token {
                        Token::Cluster(piece) => Some(piece),
                        Token::Break { .. } | Token::LineBreak => None,
                    })
            };
            beside()
                .find(|piece| !piece.text.chars().all(char::is_whitespace))
                .or_else(|| beside().next())
                .map_or((0, 0, false), |piece| {
                    (piece.group, piece.style, piece.underline)
                })
        },
        |(_, group, run, underline)| (*group, *run, *underline),
    );
    let mut typed_text = retyped
        .as_ref()
        .map_or(edit.text, |(rest, ..)| rest.as_str())
        .to_owned();
    let mut around: Vec<usize> = Vec::new();
    for token in tokens[..from].iter().rev().chain(tokens[to..].iter()) {
        if let Token::Cluster(piece) = token
            && !around.contains(&piece.style)
        {
            around.push(piece.style);
        }
    }
    if retyped.is_none() {
        from = shape_with_the_letter_before(
            &tokens,
            from,
            &mut typed_text,
            &reading.style,
            (group, run, &around),
        );
    }
    let mut inserted = Vec::new();
    for piece in typed_text.split_inclusive(['\n', LINE_BREAK]) {
        let (segment, end) = if let Some(segment) = piece.strip_suffix('\n') {
            (segment, Some(Token::Break { space: 0.0 }))
        } else if let Some(segment) = piece.strip_suffix(LINE_BREAK) {
            (segment, Some(Token::LineBreak))
        } else {
            (piece, None)
        };
        if !segment.is_empty() {
            inserted.extend(
                typed_pieces(
                    &reading.style,
                    Some(&mut *faces),
                    segment,
                    group,
                    (run, &around),
                )?
                .into_iter()
                .map(|mut piece| {
                    piece.underline = underline;
                    Token::Cluster(piece)
                }),
            );
        }
        inserted.extend(end);
    }
    let caret = from + inserted.len();
    let selected = from..caret;
    tokens.splice(from..to, inserted);
    if !tokens
        .iter()
        .any(|token| matches!(token, Token::Cluster(_)))
    {
        return Ok((Vec::new(), 0, 0..0));
    }
    Ok((tokens, caret, selected))
}

fn apply_style(
    style: &mut Style<'_>,
    tokens: &mut [Token],
    wanted: &crate::plan::TextStyle,
    faces: &mut faces::NewFaces<'_>,
) -> Result<(), SpikeError> {
    restyle(style, tokens, wanted)?;
    if wanted.changes_face() {
        restyle_face(style, tokens, wanted, faces)?;
    }
    Ok(())
}

fn styled_and_settled(
    source: &pdf_bytes::ByteStore,
    (page, page_index): (PlannerPage<'_>, usize),
    edit: &BlockEdit<'_>,
    style: &mut Style<'_>,
    (tokens, selected): (&mut [Token], Range<usize>),
    faces: &mut faces::NewFaces<'_>,
) -> Result<Option<faces::Settled>, SpikeError> {
    let typed = edit.typed.filter(|_| !selected.is_empty());
    if let Some(wanted) = edit.style {
        apply_style(style, &mut tokens[selected.clone()], wanted, faces)?;
    } else if let Some(wanted) = typed
        && wanted.changes_face()
    {
        typed_face(style, &mut tokens[selected.clone()], wanted, faces)?;
    }
    let settled = if faces.is_empty() {
        None
    } else {
        Some(faces.settle(source, (page.program.page, page_index), style, tokens)?)
    };
    if let Some(wanted) = typed {
        restyle(
            style,
            &mut tokens[selected],
            &crate::plan::TextStyle {
                size: wanted.size,
                fill: wanted.fill,
                underline: wanted.underline,
                ..crate::plan::TextStyle::default()
            },
        )?;
    }
    Ok(settled)
}

fn typed_face(
    style: &mut Style<'_>,
    tokens: &mut [Token],
    wanted: &crate::plan::TextStyle,
    faces: &mut faces::NewFaces<'_>,
) -> Result<(), SpikeError> {
    if tokens
        .iter()
        .any(|token| matches!(token, Token::Cluster(piece) if !piece.shaped.is_empty()))
    {
        return Err(unsupported(
            "text in a font the page does not have cannot be typed in another font yet",
        ));
    }
    restyle_face(style, tokens, wanted, faces)
}

fn styled_selection(edit: &BlockEdit<'_>, selected: Range<usize>) -> Result<(), SpikeError> {
    if !matches!(edit.range, BlockRange::Between { .. }) || !edit.text.is_empty() {
        return Err(unsupported("a style is set on a selection of the block"));
    }
    let spacing_only = edit.style.is_some_and(|style| {
        style.line_spacing.is_some()
            && crate::plan::TextStyle {
                line_spacing: None,
                ..style.clone()
            } == crate::plan::TextStyle::default()
    });
    let nothing_set = edit
        .style
        .is_some_and(|style| *style == crate::plan::TextStyle::default());
    if selected.is_empty() && !(spacing_only || nothing_set) {
        return Err(unsupported("there is nothing selected to set a style on"));
    }
    Ok(())
}

fn line_spacing_pitch(style: &Style<'_>, points: f64) -> Result<f64, SpikeError> {
    if !(points.is_finite() && points <= 1000.0) {
        return Err(unsupported(
            "a line spacing must be a number of points, at most 1000",
        ));
    }
    if points < LINE_FLOOR_EM * style.em().abs() {
        return Err(unsupported(
            "a line spacing must be at least 0.8 of the text size",
        ));
    }
    Ok(points)
}

fn restyle(
    style: &mut Style<'_>,
    tokens: &mut [Token],
    wanted: &crate::plan::TextStyle,
) -> Result<(), SpikeError> {
    if wanted
        .size
        .is_some_and(|points| !(points.is_finite() && points > 0.0 && points <= 1000.0))
    {
        return Err(unsupported(
            "a size must be more than 0 and at most 1000 points",
        ));
    }
    if wanted
        .fill
        .is_some_and(|rgb| rgb.iter().any(|value| !(0.0..=1.0).contains(value)))
    {
        return Err(unsupported("a colour's components must be between 0 and 1"));
    }
    if wanted.fill.is_some() && style.base().fill.is_none() {
        return Err(unsupported(
            "the block's own colour cannot be written back after a coloured selection",
        ));
    }
    let mut made: BTreeMap<usize, usize> = BTreeMap::new();
    for token in tokens {
        let Token::Cluster(piece) = token else {
            continue;
        };
        if let Some(underline) = wanted.underline {
            piece.underline = underline;
        }
        if wanted.size.is_none() && wanted.fill.is_none() {
            continue;
        }
        let run = if let Some(run) = made.get(&piece.style) {
            *run
        } else {
            let from = style.run(piece.style);
            let mut text = from.text.clone();
            if let Some(points) = wanted.size {
                let size = points / style.to_user.d;
                if text.font_size.value != 0.0 {
                    let ratio = size / text.font_size.value;
                    text.rise.value *= ratio;
                    text.character_spacing.value *= ratio;
                    text.word_spacing.value *= ratio;
                }
                text.font_size.value = size;
            }
            let (fill, fill_rgb) = match wanted.fill {
                Some([r, g, b]) => (Some(format!("{r} {g} {b} rg ")), Some([r, g, b])),
                None => (from.fill.clone(), from.fill_rgb),
            };
            let follows = from.stroke_like_fill
                || (wanted.fill.is_some() && strokes_like_fill(from.reference));
            let run = Run {
                reference: from.reference,
                font: from.font.clone(),
                text,
                stroke: if follows {
                    fill.as_deref().map(stroking)
                } else {
                    from.stroke.clone()
                },
                fill,
                fill_rgb,
                line_width: from.line_width,
                line: from.line,
                stroke_like_fill: follows,
                shear: from.shear,
            };
            style.runs.push(run);
            made.insert(piece.style, style.runs.len() - 1);
            style.runs.len() - 1
        };
        let (was, now) = (
            style.run(piece.style).text.font_size.value,
            style.run(run).text.font_size.value,
        );
        if was != 0.0 {
            for rise in &mut piece.rise {
                *rise *= now / was;
            }
        }
        piece.style = run;
        piece.advance = piece
            .codes
            .iter()
            .map(|code| advance_of(style, run, code))
            .sum();
        piece.advance += inner_kern(style, piece);
    }
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "one cluster at a time through one decision: keep, synthesise, or shape again"
)]
fn restyle_face(
    style: &mut Style<'_>,
    tokens: &mut [Token],
    wanted: &crate::plan::TextStyle,
    faces: &mut faces::NewFaces<'_>,
) -> Result<(), SpikeError> {
    let mut synthesised: BTreeMap<(usize, bool, bool), usize> = BTreeMap::new();
    let mut plain: BTreeMap<usize, usize> = BTreeMap::new();
    for token in tokens {
        let Token::Cluster(piece) = token else {
            continue;
        };
        let mut request = faces
            .request(style, piece.style)
            .ok_or_else(|| unsupported("the selection's font cannot be named"))?;
        let was = request.style;
        let own_family = request.family.clone();
        if let Some(family) = &wanted.family {
            request.family.clone_from(family);
            request.base_font = family.replace(' ', "").into_bytes();
            request.standard_face = None;
        }
        if let Some(bold) = wanted.bold {
            request.style.weight = if bold { 700 } else { 400 };
        }
        if let Some(italic) = wanted.italic {
            request.style.italic = italic;
        }
        if wanted.family.is_none()
            && request.style.is_bold() == was.is_bold()
            && request.style.italic == was.italic
        {
            if wanted.bold == Some(false) || wanted.italic == Some(false) {
                piece.style = if let Some(run) = plain.get(&piece.style) {
                    *run
                } else {
                    let run = unsynthesised(style, piece.style, wanted);
                    plain.insert(piece.style, run);
                    run
                };
            }
            continue;
        }
        let refused = || {
            unsupported("this family has no face in the weight or slope asked for on this machine")
        };
        let unread = piece.text.contains(UNREAD);
        let face = faces.face_for(&request);
        let drawable = !unread
            && face
                .as_ref()
                .is_some_and(|face| draws(face, &piece.text, wanted.family.is_some(), &own_family));
        if !drawable
            && wanted.family.is_none()
            && wanted.bold != Some(false)
            && wanted.italic != Some(false)
        {
            let bold = request.style.is_bold() && !was.is_bold();
            let italic = request.style.italic && !was.italic;
            if bold && italic {
                return Err(refused());
            }
            if bold || italic {
                synthesise(style, &mut synthesised, piece, (bold, italic))?;
            }
            continue;
        }
        if unread {
            return Err(unsupported(
                "text the file gives no meaning cannot be set in another font",
            ));
        }
        let face =
            face.ok_or_else(|| unsupported("no face on this machine is the one asked for"))?;
        let found = face.identity.style;
        if (found.is_bold() && !request.style.is_bold()) || (found.italic && !request.style.italic)
        {
            return Err(refused());
        }
        let (bold, italic) = (
            request.style.is_bold() && !found.is_bold(),
            request.style.italic && !found.italic,
        );
        if bold || italic {
            if wanted.family.is_some() || (wanted.bold.is_some() && wanted.italic.is_some()) {
                return Err(refused());
            }
            synthesise(style, &mut synthesised, piece, (bold, italic))?;
            continue;
        }
        if wanted.bold == Some(false) || wanted.italic == Some(false) {
            piece.style = if let Some(run) = plain.get(&piece.style) {
                *run
            } else {
                let run = unsynthesised(style, piece.style, wanted);
                plain.insert(piece.style, run);
                run
            };
        }
        if found.is_bold() != request.style.is_bold() || found.italic != request.style.italic {
            return Err(unsupported(
                "this family has no face in the weight or slope asked for on this machine",
            ));
        }
        if wanted
            .family
            .as_ref()
            .is_some_and(|family| !pdf_content::outline_match::is_same_family(&face, family))
        {
            return Err(unsupported("the font asked for is not on this machine"));
        }
        let mut shown = faces.piece_in(piece.style, &piece.text, piece.group, &face, false)?;
        shown.break_before = piece.break_before;
        shown.underline = piece.underline;
        *piece = shown;
    }
    Ok(())
}

fn draws(face: &pdf_content::SubstitutedFace, text: &str, named: bool, own_family: &str) -> bool {
    (named || pdf_content::outline_match::is_same_family(face, own_family))
        && pdf_content::shape_cluster(&face.program, face.identity.face_index, text).is_some_and(
            |shaped| {
                text.contains('\u{25CC}')
                    || face
                        .program
                        .glyph_for_char('\u{25CC}')
                        .is_none_or(|circle| shaped.iter().all(|glyph| glyph.glyph != circle))
            },
        )
}

fn synthesise(
    style: &mut Style<'_>,
    synthesised: &mut BTreeMap<(usize, bool, bool), usize>,
    piece: &mut Piece,
    (bold, italic): (bool, bool),
) -> Result<(), SpikeError> {
    let key = (piece.style, bold, italic);
    let run = if let Some(run) = synthesised.get(&key) {
        *run
    } else {
        let mut run = piece.style;
        if bold && !style.run(run).stroke_like_fill {
            run = embolden(style, run)?;
        }
        if italic && style.run(run).shear == 0.0 {
            run = slant(style, run);
        }
        synthesised.insert(key, run);
        run
    };
    piece.style = run;
    Ok(())
}

fn embolden(style: &mut Style<'_>, run: usize) -> Result<usize, SpikeError> {
    let ctm = linear(style.ctm);
    let scale = (ctm.a * ctm.d - ctm.b * ctm.c).abs().sqrt();
    if !(scale.is_finite() && scale > 0.0) {
        return Err(unsupported("the block's transform cannot be inverted"));
    }
    let line_width = style.run_em(run) * SYNTHETIC_BOLD_EM / scale;
    let from = style.run(run);
    let fill = from
        .fill
        .clone()
        .ok_or_else(|| unsupported("a colour that cannot be written cannot be made bold"))?;
    let mut text = from.text.clone();
    text.rendering_mode.value = TextRenderingMode::FillStroke;
    let bold = Run {
        reference: from.reference,
        font: from.font.clone(),
        line_width,
        text,
        stroke: Some(stroking(&fill)),
        fill: Some(fill),
        fill_rgb: from.fill_rgb,
        stroke_like_fill: true,
        shear: from.shear,
        line: from.line,
    };
    style.runs.push(bold);
    Ok(style.runs.len() - 1)
}

fn slant(style: &mut Style<'_>, run: usize) -> usize {
    let from = style.run(run);
    let slanted = Run {
        reference: from.reference,
        font: from.font.clone(),
        text: from.text.clone(),
        fill: from.fill.clone(),
        stroke: from.stroke.clone(),
        fill_rgb: from.fill_rgb,
        line_width: from.line_width,
        stroke_like_fill: from.stroke_like_fill,
        shear: SYNTHETIC_ITALIC_SHEAR,
        line: from.line,
    };
    style.runs.push(slanted);
    style.runs.len() - 1
}

fn unsynthesised(style: &mut Style<'_>, run: usize, wanted: &crate::plan::TextStyle) -> usize {
    let from = style.run(run);
    let unbold = wanted.bold == Some(false) && from.stroke_like_fill;
    let upright = wanted.italic == Some(false) && from.shear != 0.0;
    if !unbold && !upright {
        return run;
    }
    let mut text = from.text.clone();
    if unbold {
        text.rendering_mode.value = TextRenderingMode::Fill;
    }
    let plain = Run {
        reference: from.reference,
        font: from.font.clone(),
        text,
        fill: from.fill.clone(),
        stroke: if unbold { None } else { from.stroke.clone() },
        fill_rgb: from.fill_rgb,
        line_width: from.line_width,
        stroke_like_fill: from.stroke_like_fill && !unbold,
        shear: if upright { 0.0 } else { from.shear },
        line: from.line,
    };
    style.runs.push(plain);
    style.runs.len() - 1
}

const SYNTHETIC_BOLD_EM: f64 = 0.03;

fn stroking(fill: &str) -> String {
    let mut out = String::with_capacity(fill.len());
    for token in fill.split_whitespace() {
        out.push_str(match token {
            "g" => "G",
            "rg" => "RG",
            "k" => "K",
            "cs" => "CS",
            "sc" => "SC",
            "scn" => "SCN",
            other => other,
        });
        out.push(' ');
    }
    out
}

fn typed_pieces(
    style: &Style<'_>,
    mut faces: Option<&mut faces::NewFaces<'_>>,
    typed: &str,
    group: usize,
    (run, around): (usize, &[usize]),
) -> Result<Vec<Piece>, SpikeError> {
    if typed.chars().any(char::is_control) {
        return Err(unsupported("tabs and control characters cannot be typed"));
    }
    let brought = |index: usize| style.run(index).line.is_some();
    let mut order: Vec<usize> = Vec::new();
    if !brought(run) {
        order.push(run);
    }
    order.extend(
        around
            .iter()
            .copied()
            .filter(|other| *other != run && !brought(*other) && same_look(style, run, *other)),
    );
    if brought(run) {
        order.push(run);
    }
    let mut boundaries: Vec<usize> = GraphemeClusterSegmenter::new()
        .segment_str(typed)
        .filter(|offset| *offset > 0)
        .collect();
    boundaries.dedup();
    let mut pieces = Vec::with_capacity(boundaries.len());
    let mut start = 0;
    for end in boundaries {
        let cluster = &typed[start..end];
        start = end;
        let encoded = |index: usize, written: &str| {
            let candidate = style.run(index);
            crate::retype::encode(candidate.reference, written).and_then(|bytes| {
                let codes = candidate.font.source_codes(&bytes).map_err(|_| {
                    unsupported("the typed text does not decode in the block's font")
                })?;
                crate::retype::check_typed_codes(&codes, written)?;
                Ok(codes)
            })
        };
        let mut own = encoded(order[0], cluster).map(|codes| (order[0], codes));
        for other in order.iter().copied().skip(1) {
            if own.is_ok() {
                break;
            }
            if let Ok(codes) = encoded(other, cluster) {
                own = Ok((other, codes));
            }
        }
        if own.is_err()
            && let Some(form) = compatibility_form(cluster)
            && let Ok(typed) = typed_pieces(style, None, &form, group, (run, around))
        {
            pieces.extend(typed);
            continue;
        }
        let (run, codes) = match own {
            Ok(found) => found,
            Err(SpikeError::RetypeUnsupported(
                crate::retype::NO_CODE | crate::retype::NO_WIDTH | crate::retype::MANY_CODES,
            )) if faces.is_some() => {
                if let Some(faces) = faces.as_deref_mut() {
                    pieces.push(faces.piece(style, run, cluster, group)?);
                }
                continue;
            }
            Err(error) => return Err(error),
        };
        let set = match style.unspaced.get(&run) {
            Some(unspaced) if codes.iter().any(|code| code.bytes == [32]) => *unspaced,
            _ => run,
        };
        pieces.push(Piece {
            advance: codes.iter().map(|code| advance_of(style, set, code)).sum(),
            adjust: vec![0.0; codes.len()],
            codes,
            text: cluster.to_owned(),
            kept: None,
            group,
            style: set,
            next: None,
            break_before: false,
            shaped: Vec::new(),
            rise: Vec::new(),
            underline: false,
        });
    }
    Ok(pieces)
}

#[derive(Clone)]
struct PlacedLine {
    pieces: Vec<Piece>,
    origin: Point,
    tokens: Range<usize>,
    overflow: bool,
    gaps: Vec<f64>,
}

struct Laid {
    members: Vec<(usize, Piece)>,
    start: usize,
    space: f64,
    opens: bool,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct InTheFrame<'a> {
    pub(crate) set: Option<Alignment>,
    pub(crate) blocked: &'a [Blocked],
}

fn paragraphs_of(tokens: &[Token]) -> Vec<Laid> {
    let mut paragraphs = vec![Laid {
        members: Vec::new(),
        start: 0,
        space: 0.0,
        opens: true,
    }];
    for (index, token) in tokens.iter().enumerate() {
        match token {
            Token::Break { space } => paragraphs.push(Laid {
                members: Vec::new(),
                start: index + 1,
                space: *space,
                opens: true,
            }),
            Token::LineBreak => paragraphs.push(Laid {
                members: Vec::new(),
                start: index + 1,
                space: 0.0,
                opens: false,
            }),
            Token::Cluster(piece) => {
                if let Some(last) = paragraphs.last_mut() {
                    last.members.push((index, piece.clone()));
                }
            }
        }
    }
    paragraphs
}

fn rows_for_paragraph(blocked: &[Blocked], down: f64, in_from: f64) -> Vec<Blocked> {
    blocked
        .iter()
        .map(|rect| Blocked {
            top: rect.top - down,
            bottom: rect.bottom - down,
            left: rect.left - in_from,
            right: rect.right - in_from,
        })
        .collect()
}

fn place_lines(
    tokens: &[Token],
    alignments: &[(Alignment, f64, f64)],
    reading: &Reading<'_>,
    (left, right): (f64, f64),
    frame: InTheFrame<'_>,
) -> Result<Vec<PlacedLine>, SpikeError> {
    let pitch = reading.pitch;
    let first_baseline = reading.lines.first().map_or(0.0, |line| line.baseline);
    let paragraphs = paragraphs_of(tokens);
    let mut lines = Vec::new();
    let mut baseline = first_baseline;
    let mut down = 0.0;
    let mut first = true;
    let mut previous_group = 0;
    for paragraph in &paragraphs {
        let group = paragraph
            .members
            .first()
            .map_or(previous_group, |(_, piece)| piece.group);
        previous_group = group;
        let (read_as, edge, indent) =
            alignments
                .get(group)
                .copied()
                .unwrap_or((Alignment::Start, left, 0.0));
        let (alignment, edge, indent) = match frame.set {
            Some(set) => (set, left, 0.0),
            None => (read_as, edge, indent),
        };
        let indent = if paragraph.opens { indent } else { 0.0 };
        let available = right - edge;
        if available <= 0.0 {
            return Err(unsupported(
                "a paragraph starts at or past the frame's right edge",
            ));
        }
        if !first {
            down += paragraph.space;
        }
        let blocked = rows_for_paragraph(frame.blocked, down, edge - left);
        let pieces: Vec<&Piece> = paragraph.members.iter().map(|(_, piece)| piece).collect();
        let units = units_of(&reading.style, &pieces, pitch);
        let layout = lay_out_around(
            &[Paragraph {
                units: &units,
                empty_pitch: pitch,
                first_indent: indent,
                split_words: true,
            }],
            available,
            &blocked,
        )
        .map_err(|error| unsupported(layout_refusal(&error)))?;
        let count = layout.lines.len();
        let mut unskipped = 0.0;
        for (index, line) in layout.lines.into_iter().enumerate() {
            let skipped = (line.top - unskipped).max(0.0);
            unskipped = line.top + line.pitch;
            if !first {
                baseline -= line.pitch;
                if index == 0 {
                    baseline -= paragraph.space;
                }
            }
            baseline -= skipped;
            first = false;
            let run = widest_free_run(available, &blocked, line.top, line.pitch)
                .map_or(available, |(_, run)| run);
            let run = if index == 0 { run - indent } else { run };
            let start = if index == 0 { indent } else { 0.0 };
            let x = edge + start + line.left + alignment.offset(run, line.width);
            let tokens = if line.units.is_empty() {
                paragraph.start..paragraph.start
            } else {
                paragraph.members[line.units.start].0..paragraph.members[line.units.end - 1].0 + 1
            };
            let gaps = if alignment == Alignment::Justify && index + 1 < count && !line.overflow {
                justify_gaps(&units[line.units.clone()], run, line.width)
            } else {
                vec![0.0; line.units.len()]
            };
            lines.push(PlacedLine {
                pieces: pieces[line.units.clone()]
                    .iter()
                    .map(|piece| (*piece).clone())
                    .collect(),
                origin: Point { x, y: baseline },
                tokens,
                overflow: line.overflow,
                gaps,
            });
        }
        down += layout.height;
    }
    Ok(lines)
}

fn stands_in_the_frame(
    page: PlannerPage<'_>,
    reading: &Reading<'_>,
    (left, right): (f64, f64),
    paragraph: crate::plan::ParagraphLayout,
) -> Vec<Blocked> {
    if !paragraph.flow_round {
        return Vec::new();
    }
    let pitch = reading.pitch;
    let first_baseline = reading.lines.first().map_or(0.0, |line| line.baseline);
    crate::layout::blocked_for_block(
        page.graph,
        page.program.geometry.crop_box[1],
        (left, right),
        (first_baseline, pitch),
    )
}

fn units_of(style: &Style<'_>, pieces: &[&Piece], pitch: f64) -> Vec<Unit> {
    let text: String = pieces.iter().map(|piece| piece.text.as_str()).collect();
    let starts = piece_starts(pieces.iter().copied());
    let breaks = line_break_opportunities(&text, &starts);
    let mut units: Vec<Unit> = pieces
        .iter()
        .zip(&starts)
        .enumerate()
        .map(|(index, (piece, start))| {
            let kern = kern_between(style, piece, pieces.get(index + 1).copied());
            Unit {
                advance: piece.advance + kern,
                pitch: unit_pitch(style, piece, pitch),
                break_before: piece.break_before || breaks.binary_search(start).is_ok(),
                hangs: is_space(&piece.text),
                spacing_after: kern,
            }
        })
        .collect();
    let backwards: Vec<usize> = units
        .iter()
        .enumerate()
        .filter(|(_, unit)| unit.advance < 0.0)
        .map(|(index, _)| index)
        .collect();
    for index in backwards {
        for glued in units.iter_mut().skip(index).take(2) {
            glued.break_before = false;
        }
    }
    units
}

fn unit_pitch(style: &Style<'_>, piece: &Piece, pitch: f64) -> f64 {
    let run = piece.style;
    let grown = pitch * (style.run_em(run) / style.em()).max(1.0);
    let Some((ascent, descent)) = style.run(run).line else {
        return grown;
    };
    grown.max((ascent - descent) * style.run_em(run).abs())
}

const fn layout_refusal(error: &LayoutError) -> &'static str {
    match error {
        LayoutError::Backwards {
            ends_a_line: true, ..
        } => "a mark the file moves the pen back from is where a line may end",
        LayoutError::Backwards { .. } => "a word the file writes moves the pen backwards",
        _ => "the block's measurements cannot be laid out",
    }
}

fn planned_caret(
    placed: &[PlacedLine],
    tokens: &[Token],
    caret: usize,
    keys: &[Vec<(ClusterKey, ClusterKey)>],
) -> Option<PlannedCaret> {
    let is_cluster = |index: usize| matches!(tokens.get(index), Some(Token::Cluster(_)));
    let line_of = |index: usize| placed.iter().position(|line| line.tokens.contains(&index));
    let (line, offset) = if caret > 0 && is_cluster(caret - 1) {
        let line = line_of(caret - 1)?;
        (line, caret - placed[line].tokens.start)
    } else if is_cluster(caret) {
        let line = line_of(caret)?;
        (line, caret - placed[line].tokens.start)
    } else {
        let line = placed
            .iter()
            .position(|line| line.pieces.is_empty() && line.tokens.start == caret)?;
        return Some(PlannedCaret::EmptyLine { line });
    };
    let keys = keys.get(line)?;
    Some(match offset {
        0 => PlannedCaret::Beside {
            cluster: keys.first()?.0,
            after: false,
        },
        after => PlannedCaret::Beside {
            cluster: keys.get(after - 1)?.1,
            after: true,
        },
    })
}

#[cfg(test)]
mod scale_tests {
    use super::*;

    fn matrix(a: f64, b: f64, c: f64, d: f64) -> Matrix {
        Matrix {
            a,
            b,
            c,
            d,
            e: 0.0,
            f: 0.0,
        }
    }

    #[test]
    fn a_scale_a_stretch_and_a_slant_are_read_and_a_turn_is_not() {
        let block = matrix(10.0, 0.0, 0.0, 10.0);
        assert_eq!(
            shape_against(block, matrix(5.0, 0.0, 0.0, 5.0)),
            Some((0.5, 1.0, 0.0))
        );
        assert_eq!(
            shape_against(block, matrix(5.0, 0.0, 0.0, 8.0)),
            Some((0.8, 0.625, 0.0)),
            "squeezed along the baseline"
        );
        assert_eq!(
            shape_against(block, matrix(0.5, 0.0, 0.0, 8.0)),
            None,
            "squeezed past a tenth"
        );
        assert_eq!(
            shape_against(block, matrix(-5.0, 0.0, 0.0, -5.0)),
            None,
            "turned half"
        );
        assert_eq!(
            shape_against(block, matrix(5.0, 0.0, 1.0, 5.0)),
            Some((0.5, 1.0, 0.2)),
            "slanted"
        );
        assert_eq!(
            shape_against(block, matrix(5.0, 0.0, 6.0, 5.0)),
            None,
            "slanted past 45 degrees"
        );
        assert_eq!(sheared(block, 0.2), matrix(10.0, 0.0, 2.0, 10.0));
        let turned = matrix(0.0, 10.0, -10.0, 0.0);
        assert_eq!(
            shape_against(turned, matrix(0.0, 5.0, -5.0, 0.0)),
            Some((0.5, 1.0, 0.0))
        );
        assert_eq!(
            shape_against(block, matrix(5.0, 1.0, 0.0, 5.0)),
            None,
            "turned a little"
        );
        assert_eq!(shape_against(turned, matrix(0.0, -5.0, 5.0, 0.0)), None);
    }
}

#[cfg(test)]
mod marker_tests {
    use super::starts_with_list_marker;

    #[test]
    fn a_list_item_starts_with_a_bullet_a_dash_or_a_short_label() {
        for marked in [
            "- ຜູ້ເວົ້າ",
            "• item",
            "\u{F0B7} ອຸປະກອນ",
            "1. first",
            "12) twelfth",
            "(a) option",
            "໑. ລາວ",
            "๓) ไทย",
            "  – indented dash",
        ] {
            assert!(starts_with_list_marker(marked), "{marked:?}");
        }
        for unmarked in [
            "2.3 ການ",
            "1234. year",
            "Introduction",
            "ຈາກການ",
            "",
            "12 apples",
            "(abcd) long",
        ] {
            assert!(!starts_with_list_marker(unmarked), "{unmarked:?}");
        }
    }
}

#[cfg(test)]
mod pitch_tests {
    use super::*;

    fn close(value: Option<f64>, wanted: f64) -> bool {
        value.is_some_and(|value| (value - wanted).abs() < 1e-9)
    }

    #[test]
    fn rows_a_producer_rounded_lie_on_the_pitch_of_their_height() {
        assert!(close(rounded_pitch(&[18.80, 18.69], 10.87), 18.745));
        assert!(close(
            rounded_pitch(&[38.34, 19.2, 19.2, 19.2, 19.2, 19.2], 20.0),
            134.34 / 7.0
        ));
    }

    #[test]
    fn rows_off_their_grid_marks_and_objects_between_rows_have_no_rounded_pitch() {
        assert_eq!(rounded_pitch(&[18.8, 18.2], 10.87), None);
        assert_eq!(rounded_pitch(&[3.6, 18.02], 12.0), None);
        assert_eq!(rounded_pitch(&[6.0, 4.0], 10.0), None);
        assert_eq!(rounded_pitch(&[], 10.0), None);
    }
}
