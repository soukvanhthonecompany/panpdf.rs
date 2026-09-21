use std::collections::BTreeMap;

use icu_segmenter::GraphemeClusterSegmenter;
use pdf_bytes::ByteStore;
use pdf_content::FontStyle;
use pdf_paint::{Matrix, PaintAtomKind, PaintGraph, Point};

use crate::layout::{
    Alignment, Blocked, Paragraph, Unit, justify_gaps, lay_out_around, line_break_opportunities,
};
use crate::new_font::{Embeddable, FontObjects, add_font_resource, embed, face_mark, width};
use crate::plan::{Capability, Effect, MovedRun, Plan, PlannedBody, PlannedWrite, SourceAnchor};
use crate::spike_move_text::{PlannerPage, SpikeError, interpret_bytes_of};

pub(crate) const LINE_EM: f64 = 1.2;

#[derive(Clone, Copy, Debug)]
pub struct NewText<'a> {
    pub frame: [f64; 4],
    pub text: &'a str,
    pub family: &'a str,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    pub fill: Option<[f64; 3]>,
    pub opacity: f64,
    pub turn: Matrix,
    pub share_from: Option<usize>,
    pub paragraph: crate::plan::ParagraphLayout,
}

fn refused(reason: &'static str) -> SpikeError {
    SpikeError::RetypeUnsupported(reason)
}

fn stands_in_the_frame(
    page: PlannerPage<'_>,
    new: &NewText<'_>,
    frame: [f64; 4],
) -> Vec<crate::layout::Blocked> {
    if new.paragraph.flow_round {
        crate::layout::blocked_in_frame(page.graph, frame, new.size * LINE_EM)
    } else {
        Vec::new()
    }
}

pub(crate) struct Cluster {
    pub(crate) codes: Vec<u16>,
    pub(crate) glyphs: Vec<Glyph>,
    pub(crate) text: String,
    pub(crate) meanings: Vec<String>,
    pub(crate) advance: f64,
    breaks: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Glyph {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
}

pub(crate) fn plan_new_text(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    new: &NewText<'_>,
) -> Result<Plan, SpikeError> {
    let frame = checked_frame(new)?;
    let face = face_for(&page, new)?;
    let embeddable = Embeddable::of(&face.program)
        .ok_or_else(|| refused("the face chosen cannot be embedded yet"))?;
    let clusters = shape(new.text, &face, embeddable, new.size)?;
    let blocked = stands_in_the_frame(page, new, frame);
    let alignment = new.paragraph.alignment.unwrap_or(Alignment::Start);
    let placed = turned(
        place(&clusters, frame, new.size, (alignment, &blocked))?,
        new.turn,
    );

    let meaning = meanings(&clusters, embeddable);
    let mark = face_mark(&face.identity.sha256, face.identity.face_index);
    let before = embedded_before(source, &page, new.share_from, &mark);
    let shared_with = before
        .as_ref()
        .filter(|found| !found.named_here)
        .and(new.share_from);
    let (name, mut writes) = named_face(
        source,
        page.program.page,
        (embeddable, &mark, meaning),
        before,
    )?;

    let document = crate::block_rewrite::commit_writes(source, &writes, page.restrictions)?;
    if let Some(other) = shared_with {
        let was = crate::spike_move_text::read_page(source, other, b"", page.fonts)?;
        let is = crate::spike_move_text::read_page(&document, other, b"", page.fonts)?;
        if was.graph.atoms.len() != is.graph.atoms.len() {
            return Err(SpikeError::MoveNotIsolated);
        }
        prove_untouched(&was.graph, &is.graph)?;
    }
    let mut carrying = crate::spike_move_text::read_page(&document, page_index, b"", page.fonts)?;
    let stream = carrying
        .program
        .streams
        .len()
        .checked_sub(1)
        .ok_or_else(|| refused("a page with no content stream cannot be written into"))?;
    let first = candidate(
        carrying.program.streams[stream].bytes.as_bytes(),
        &placed,
        new,
        (&name, None, Matrix::IDENTITY),
    );
    let mut graph = interpret_bytes_of(&carrying.program, stream, &first, page.fonts)?;
    let standing = standing_state(&carrying.graph, &graph)?;
    let ctm = standing.ctm.value;
    let mut state = None;
    if !drawn_as_asked(standing, new.opacity) {
        let (named, more) = see_through(source, &document, &page, new)?;
        for write in more {
            writes.retain(|was| was.reference != write.reference);
            writes.push(write);
        }
        let document = crate::block_rewrite::commit_writes(source, &writes, page.restrictions)?;
        carrying = crate::spike_move_text::read_page(&document, page_index, b"", page.fonts)?;
        state = Some(named);
    }
    let mut bytes = first;
    if ctm != Matrix::IDENTITY || state.is_some() {
        let inverse = ctm
            .inverse()
            .ok_or_else(|| refused("this page leaves a transform text cannot be placed through"))?;
        bytes = candidate(
            carrying.program.streams[stream].bytes.as_bytes(),
            &placed,
            new,
            (&name, state.as_deref(), inverse),
        );
        graph = interpret_bytes_of(&carrying.program, stream, &bytes, page.fonts)?;
    }
    prove_added(&carrying.graph, &graph, (&placed, new.opacity))?;

    writes.push(PlannedWrite {
        reference: carrying.program.streams[stream].reference,
        body: PlannedBody::ReplacedStream { decoded: bytes },
    });
    let ordinal = carrying.graph.atoms.len();
    let atom = graph
        .atoms
        .get(ordinal)
        .ok_or_else(|| refused("the text written does not paint"))?;
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
            target_stream: carrying.program.streams[stream].reference,
            declared_region: Some(region(turned_box(frame, new.turn), &placed)),
        },
    ))
}

fn checked_frame(new: &NewText<'_>) -> Result<[f64; 4], SpikeError> {
    if !new.frame.iter().all(|edge| edge.is_finite()) {
        return Err(refused("a frame must be four numbers"));
    }
    if !(new.size.is_finite() && new.size > 0.0) {
        return Err(refused("text has a size"));
    }
    if !(new.opacity > 0.0 && new.opacity <= 1.0) {
        return Err(refused("how much text shows runs above zero, up to one"));
    }
    if new.text.is_empty() {
        return Err(refused("there is no text to put on the page"));
    }
    if new.text.chars().any(|letter| {
        letter != '\n' && (letter.is_control() || letter == '\u{feff}' || letter == '\u{fffe}')
    }) {
        return Err(refused("a control character cannot be written as text"));
    }
    let [x0, y0, x1, y1] = new.frame;
    let frame = [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)];
    if frame[2] - frame[0] <= 0.0 {
        return Err(refused("a frame with no width holds no line"));
    }
    Ok(frame)
}

pub(crate) fn face_for(
    page: &PlannerPage<'_>,
    new: &NewText<'_>,
) -> Result<pdf_content::SubstitutedFace, SpikeError> {
    let fonts = page
        .fonts
        .ok_or_else(|| refused("no font provider was given, so no face can be chosen"))?;
    let request = pdf_content::FontRequest::for_family(
        new.family,
        FontStyle {
            weight: if new.bold { 700 } else { 400 },
            italic: new.italic,
        },
    );
    let first = new
        .text
        .chars()
        .find(|letter| *letter != '\n')
        .ok_or_else(|| refused("there is no text to put on the page"))?;
    let face = fonts
        .primary_face(&request)
        .filter(|face| draws(face, first))
        .or_else(|| fonts.fallback_face(&request, first))
        .ok_or_else(|| refused("no face on this machine draws what was typed"))?;
    let found = face.identity.style;
    if (found.is_bold() && !new.bold) || (found.italic && !new.italic) {
        return Err(refused(
            "this family has no face in the weight or slope asked for on this machine",
        ));
    }
    Ok(face)
}

fn draws(face: &pdf_content::SubstitutedFace, letter: char) -> bool {
    pdf_content::shape_cluster(&face.program, face.identity.face_index, &letter.to_string())
        .is_some_and(|shaped| shaped.iter().all(|glyph| glyph.glyph != 0))
}

pub(crate) fn shape(
    text: &str,
    face: &pdf_content::SubstitutedFace,
    embeddable: Embeddable<'_>,
    size: f64,
) -> Result<Vec<Vec<Cluster>>, SpikeError> {
    let segmenter = GraphemeClusterSegmenter::new();
    let metrics = embeddable.metrics();
    let em = f64::from(metrics.units_per_em());
    if em <= 0.0 {
        return Err(refused("the face chosen states no em"));
    }
    let mut paragraphs = Vec::new();
    for paragraph in text.split('\n') {
        let boundaries: Vec<usize> = segmenter.segment_str(paragraph).collect();
        let breaks = line_break_opportunities(paragraph, &boundaries);
        let mut clusters = Vec::new();
        for pair in boundaries.windows(2) {
            let (from, to) = (pair[0], pair[1]);
            let cluster = &paragraph[from..to];
            let shaped =
                pdf_content::shape_cluster(&face.program, face.identity.face_index, cluster)
                    .ok_or_else(|| refused("the face chosen cannot shape what was typed"))?;
            if shaped.iter().any(|glyph| glyph.glyph == 0) {
                return Err(refused("the face chosen does not draw what was typed"));
            }
            if shaped.first().is_some_and(|glyph| glyph.x != 0) {
                return Err(refused(
                    "a cluster its face places before its pen (a later slice)",
                ));
            }
            let mut codes = Vec::with_capacity(shaped.len());
            for glyph in &shaped {
                codes.push(
                    embeddable
                        .code(glyph.glyph)
                        .ok_or_else(|| refused("the face chosen cannot be embedded yet"))?,
                );
            }
            let glyphs: Vec<Glyph> = shaped
                .iter()
                .map(|glyph| Glyph {
                    x: f64::from(glyph.x) / em * size,
                    y: f64::from(glyph.y) / em * size,
                    width: width(metrics, glyph.glyph) / 1000.0 * size,
                })
                .collect();
            let advance: f64 = glyphs.iter().map(|glyph| glyph.width).sum();
            let meanings = crate::block_rewrite::faces::meanings(metrics, &shaped, cluster);
            clusters.push(Cluster {
                codes,
                glyphs,
                meanings,
                text: cluster.to_owned(),
                advance,
                breaks: breaks.binary_search(&from).is_ok(),
            });
        }
        paragraphs.push(clusters);
    }
    Ok(paragraphs)
}

pub(crate) struct PlacedLine {
    pub(crate) origin: Point,
    pub(crate) codes: Vec<u16>,
    pub(crate) glyphs: Vec<Glyph>,
    pub(crate) pens: Vec<Point>,
    pub(crate) advance: f64,
    pub(crate) turn: Matrix,
}

pub(crate) fn place(
    paragraphs: &[Vec<Cluster>],
    frame: [f64; 4],
    size: f64,
    (alignment, blocked): (Alignment, &[Blocked]),
) -> Result<Vec<PlacedLine>, SpikeError> {
    let pitch = size * LINE_EM;
    let measured: Vec<Vec<Unit>> = paragraphs
        .iter()
        .map(|clusters| {
            clusters
                .iter()
                .map(|cluster| Unit {
                    advance: cluster.advance,
                    pitch,
                    break_before: cluster.breaks,
                    hangs: cluster.text == " ",
                    spacing_after: 0.0,
                })
                .collect()
        })
        .collect();
    let laid: Vec<Paragraph<'_>> = measured
        .iter()
        .map(|units| Paragraph {
            units,
            empty_pitch: pitch,
            first_indent: 0.0,
            split_words: true,
        })
        .collect();
    let width = frame[2] - frame[0];
    let layout = lay_out_around(&laid, width, blocked)
        .map_err(|_| refused("the text cannot be laid out in the frame drawn"))?;
    let mut lines = Vec::new();
    let last_of = |line: &crate::layout::LaidLine| {
        layout
            .lines
            .iter()
            .rfind(|other| other.paragraph == line.paragraph)
            .is_none_or(|other| other.units.end == line.units.end)
    };
    for line in &layout.lines {
        let clusters = &paragraphs[line.paragraph][line.units.clone()];
        let run = crate::layout::widest_free_run(width, blocked, line.top, line.pitch)
            .map_or(width, |(_, run)| run);
        let gaps = if alignment == Alignment::Justify && !last_of(line) && !line.overflow {
            justify_gaps(
                &measured[line.paragraph][line.units.clone()],
                run,
                line.width,
            )
        } else {
            Vec::new()
        };
        let start = frame[0] + line.left + alignment.offset(run, line.width);
        let baseline = frame[3] - size - line.top;
        let mut codes = Vec::new();
        let mut glyphs = Vec::new();
        let mut pens = Vec::new();
        let mut pen = start;
        for (at, cluster) in clusters.iter().enumerate() {
            for (code, glyph) in cluster.codes.iter().zip(&cluster.glyphs) {
                codes.push(*code);
                glyphs.push(Glyph {
                    x: pen - start + glyph.x,
                    ..*glyph
                });
                pens.push(Point {
                    x: pen + glyph.x,
                    y: baseline + glyph.y,
                });
            }
            pen += cluster.advance + gaps.get(at).copied().unwrap_or(0.0);
        }
        if !codes.is_empty() {
            lines.push(PlacedLine {
                origin: Point {
                    x: start,
                    y: baseline,
                },
                codes,
                glyphs,
                pens,
                advance: pen - start,
                turn: Matrix::IDENTITY,
            });
        }
    }
    if lines.is_empty() {
        return Err(refused("there is no text to put on the page"));
    }
    Ok(lines)
}

pub(crate) fn meanings(
    paragraphs: &[Vec<Cluster>],
    embeddable: Embeddable<'_>,
) -> BTreeMap<u16, String> {
    let mut meaning = BTreeMap::new();
    for cluster in paragraphs.iter().flatten() {
        for (code, text) in cluster.codes.iter().zip(&cluster.meanings) {
            let Some(glyph) = embeddable.glyph(*code) else {
                continue;
            };
            let slot = meaning.entry(glyph).or_insert_with(String::new);
            if slot.is_empty() {
                slot.clone_from(text);
            }
        }
    }
    meaning
}

fn named_face(
    source: &ByteStore,
    page: pdf_syntax::Reference,
    (embeddable, mark, mut meaning): (Embeddable<'_>, &str, BTreeMap<u16, String>),
    before: Option<Before>,
) -> Result<(String, Vec<PlannedWrite>), SpikeError> {
    let Some(Before {
        entry,
        objects,
        named_here,
    }) = before
    else {
        let objects = FontObjects::numbered_from(crate::block_rewrite::next_object_number(source)?);
        let mut writes = embed(embeddable, &meaning, (objects, mark, false))?.writes;
        let (name, holder) = add_font_resource(source, page, objects.font)?;
        writes.push(holder);
        return Ok((name, writes));
    };
    let had = entry
        .to_unicode()
        .map_err(|_| refused("a face embedded before cannot be read"))?;
    for (code, text) in had.entries() {
        if let Some(glyph) = u16::try_from(code.value)
            .ok()
            .and_then(|code| embeddable.glyph(code))
        {
            let slot: &mut String = meaning.entry(glyph).or_default();
            if slot.is_empty() {
                slot.clone_from(&text.text);
            }
        }
    }
    let mut writes = embed(embeddable, &meaning, (objects, mark, true))?.writes;
    if named_here {
        let name = String::from_utf8_lossy(entry.name())
            .trim_start_matches('/')
            .to_owned();
        return Ok((name, writes));
    }
    let (name, holder) = add_font_resource(source, page, objects.font)?;
    writes.push(holder);
    Ok((name, writes))
}

struct Before {
    entry: pdf_content::ResourceEntry,
    objects: FontObjects,
    named_here: bool,
}

fn embedded_before(
    source: &ByteStore,
    page: &PlannerPage<'_>,
    share_from: Option<usize>,
    mark: &str,
) -> Option<Before> {
    let found = |fonts: &[pdf_content::ResourceEntry], named_here| {
        fonts.iter().find_map(|entry| {
            let objects = crate::new_font::embedded_face(source, entry.reference()?, mark)?;
            Some(Before {
                entry: entry.clone(),
                objects,
                named_here,
            })
        })
    };
    found(page.program.resources.fonts(), true).or_else(|| {
        let other = pdf_content::load_page_program_with_password(
            source,
            share_from?,
            pdf_content::PageContentLimits::default(),
            b"",
        )
        .ok()?;
        found(other.resources.fonts(), false)
    })
}

fn turned(mut lines: Vec<PlacedLine>, turn: Matrix) -> Vec<PlacedLine> {
    for line in &mut lines {
        line.origin = turn.transform(line.origin);
        for pen in &mut line.pens {
            *pen = turn.transform(*pen);
        }
        line.turn = turn;
    }
    lines
}

fn turned_box([x0, y0, x1, y1]: [f64; 4], turn: Matrix) -> [f64; 4] {
    let corners =
        [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| turn.transform(Point { x, y }));
    [
        corners.iter().map(|at| at.x).fold(f64::INFINITY, f64::min),
        corners.iter().map(|at| at.y).fold(f64::INFINITY, f64::min),
        corners
            .iter()
            .map(|at| at.x)
            .fold(f64::NEG_INFINITY, f64::max),
        corners
            .iter()
            .map(|at| at.y)
            .fold(f64::NEG_INFINITY, f64::max),
    ]
}

fn candidate(
    decoded: &[u8],
    lines: &[PlacedLine],
    new: &NewText<'_>,
    (name, state, inverse): (&str, Option<&str>, Matrix),
) -> Vec<u8> {
    let [red, green, blue] = new.fill.unwrap_or([0.0, 0.0, 0.0]);
    let mut out = Vec::with_capacity(decoded.len() + lines.len() * 64);
    out.extend_from_slice(decoded);
    out.extend_from_slice(b"\nq ");
    if let Some(state) = state {
        out.extend_from_slice(format!("/{state} gs ").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "BT /{name} {size} Tf {pitch} TL 0 Tc 0 Tw 100 Tz 0 Ts {red} {green} {blue} rg\n",
            size = new.size,
            pitch = new.size * LINE_EM,
        )
        .as_bytes(),
    );
    for line in lines {
        let set = inverse.multiply(Matrix {
            e: line.origin.x,
            f: line.origin.y,
            ..line.turn
        });
        out.extend_from_slice(
            format!(
                "{} {} {} {} {} {} Tm ",
                set.a, set.b, set.c, set.d, set.e, set.f
            )
            .as_bytes(),
        );
        out.extend_from_slice(&shows(line, new.size).0);
    }
    out.extend_from_slice(b"ET Q\n");
    out
}

const SAME_PLACE: f64 = 1e-6;

pub(crate) fn shows(line: &PlacedLine, size: f64) -> (Vec<u8>, usize) {
    let glyphs = &line.glyphs;
    let gap = |at: usize| {
        glyphs
            .get(at + 1)
            .map_or(0.0, |next| next.x - (glyphs[at].x + glyphs[at].width))
    };
    let moves = |at: usize| gap(at).abs() > SAME_PLACE;
    let raised = glyphs.iter().any(|glyph| glyph.y.abs() > SAME_PLACE);
    let first_moves = glyphs
        .first()
        .is_some_and(|glyph| glyph.x.abs() > SAME_PLACE);
    let mut out = Vec::new();
    if !raised && !first_moves && !(0..glyphs.len()).any(moves) {
        out.push(b'<');
        for code in &line.codes {
            out.extend_from_slice(format!("{code:04X}").as_bytes());
        }
        out.extend_from_slice(b"> Tj\n");
        return (out, 1);
    }
    let number = |points: f64| -points * 1000.0 / size;
    let mut count = 0;
    let mut carry = glyphs.first().map_or(0.0, |glyph| glyph.x);
    let mut at = 0;
    while at < glyphs.len() {
        let rise = glyphs[at].y;
        let mut end = at;
        while end + 1 < glyphs.len() && (glyphs[end + 1].y - rise).abs() <= SAME_PLACE {
            end += 1;
        }
        if raised {
            out.extend_from_slice(format!("{rise} Ts ").as_bytes());
        }
        out.push(b'[');
        if carry.abs() > SAME_PLACE {
            out.extend_from_slice(format!("{} ", number(carry)).as_bytes());
        }
        out.push(b'<');
        for index in at..=end {
            out.extend_from_slice(format!("{:04X}", line.codes[index]).as_bytes());
            if index < end && moves(index) {
                out.extend_from_slice(format!("> {} <", number(gap(index))).as_bytes());
            }
        }
        out.extend_from_slice(b">] TJ ");
        carry = gap(end);
        count += 1;
        at = end + 1;
    }
    if raised {
        out.extend_from_slice(b"0 Ts");
    }
    out.push(b'\n');
    (out, count)
}

fn standing_state<'a>(
    before: &PaintGraph,
    after: &'a PaintGraph,
) -> Result<&'a pdf_paint::GraphicsState, SpikeError> {
    let atom = after
        .atoms
        .get(before.atoms.len())
        .ok_or_else(|| refused("the text written does not paint"))?;
    match &atom.kind {
        PaintAtomKind::Text(text) => Ok(&text.state),
        _ => Err(refused("the text written does not paint")),
    }
}

fn drawn_as_asked(state: &pdf_paint::GraphicsState, opacity: f64) -> bool {
    (state.fill_alpha.value - opacity).abs() <= 1e-9
        && (state.stroke_alpha.value - opacity).abs() <= 1e-9
        && state.blend_mode.value.names == [b"/Normal".to_vec()]
        && matches!(state.soft_mask.value, pdf_paint::SoftMask::None)
        && !state.alpha_is_shape.value
}

fn state_entries(opacity: f64) -> String {
    format!(
        "<< /Type /ExtGState /CA {opacity} /ca {opacity} /BM /Normal /SMask /None /AIS false >>"
    )
}

fn see_through(
    source: &ByteStore,
    document: &ByteStore,
    page: &PlannerPage<'_>,
    new: &NewText<'_>,
) -> Result<(String, Vec<PlannedWrite>), SpikeError> {
    let wanted = state_entries(new.opacity);
    let written = |entry: &pdf_content::ResourceEntry| {
        let reference = entry.reference()?;
        let body = crate::new_font::resolve(source, reference).ok()?;
        (body.bytes == wanted.as_bytes()).then_some(reference)
    };
    if let Some(entry) = page
        .program
        .resources
        .ext_gstates()
        .iter()
        .find(|entry| written(entry).is_some())
    {
        let name = String::from_utf8_lossy(entry.name())
            .trim_start_matches('/')
            .to_owned();
        return Ok((name, Vec::new()));
    }
    let shared = new.share_from.and_then(|other| {
        let program = pdf_content::load_page_program_with_password(
            source,
            other,
            pdf_content::PageContentLimits::default(),
            b"",
        )
        .ok()?;
        program.resources.ext_gstates().iter().find_map(written)
    });
    let mut writes = Vec::new();
    let object = if let Some(object) = shared {
        object
    } else {
        let object =
            pdf_syntax::Reference::new(crate::block_rewrite::next_object_number(document)?, 0);
        writes.push(PlannedWrite {
            reference: object,
            body: PlannedBody::Direct {
                body: wanted.into_bytes(),
            },
        });
        object
    };
    let (name, holder) =
        crate::new_font::add_resource(document, page.program.page, (b"/ExtGState", "GS"), object)?;
    writes.push(holder);
    Ok((name, writes))
}

fn prove_added(
    before: &PaintGraph,
    after: &PaintGraph,
    (lines, opacity): (&[PlacedLine], f64),
) -> Result<(), SpikeError> {
    let counts: Vec<usize> = lines.iter().map(|line| shows(line, 1.0).1).collect();
    if after.atoms.len() != before.atoms.len() + counts.iter().sum::<usize>() {
        return Err(SpikeError::MoveNotIsolated);
    }
    prove_untouched(before, after)?;
    let mut added = after.atoms[before.atoms.len()..].iter();
    for (line, count) in lines.iter().zip(counts) {
        let mut drawn = Vec::new();
        for atom in added.by_ref().take(count) {
            let PaintAtomKind::Text(text) = &atom.kind else {
                return Err(SpikeError::MoveNotIsolated);
            };
            if !drawn_as_asked(&text.state, opacity) {
                return Err(SpikeError::MoveNotIsolated);
            }
            drawn.extend(text.glyphs.iter().map(|glyph| (text, glyph)));
        }
        if drawn.len() != line.pens.len() {
            return Err(SpikeError::MoveNotIsolated);
        }
        for ((text, glyph), wanted) in drawn.into_iter().zip(&line.pens) {
            let at = text
                .state
                .ctm
                .value
                .multiply(glyph.matrix)
                .transform(Point { x: 0.0, y: 0.0 });
            if (at.x - wanted.x).abs() > crate::block_move::PLACEMENT_TOLERANCE
                || (at.y - wanted.y).abs() > crate::block_move::PLACEMENT_TOLERANCE
            {
                return Err(SpikeError::MoveNotIsolated);
            }
        }
    }
    Ok(())
}

pub(crate) fn prove_untouched(before: &PaintGraph, after: &PaintGraph) -> Result<(), SpikeError> {
    for (one, other) in before.atoms.iter().zip(&after.atoms) {
        match (&one.kind, &other.kind) {
            (PaintAtomKind::Text(one), PaintAtomKind::Text(other)) => {
                crate::place_text::prove_run_placed(one, other, Matrix::IDENTITY)?;
            }
            _ => {
                if pdf_paint::paint_signature(&one.kind) != pdf_paint::paint_signature(&other.kind)
                {
                    return Err(SpikeError::MoveNotIsolated);
                }
            }
        }
    }
    Ok(())
}

fn region(frame: [f64; 4], lines: &[PlacedLine]) -> [f64; 4] {
    let mut box_of = frame;
    for pen in lines.iter().flat_map(|line| &line.pens) {
        box_of[0] = box_of[0].min(pen.x);
        box_of[1] = box_of[1].min(pen.y);
        box_of[2] = box_of[2].max(pen.x);
        box_of[3] = box_of[3].max(pen.y);
    }
    box_of
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};
    use pdf_content::{FontProvider, FontRequest, GlyphProgram, SubstitutedFace};
    use pdf_paint::PaintAtomKind;

    use crate::plan::Command;
    use crate::spike_move_text::{plan_command_with_fonts, read_page};

    fn square(side: i16) -> Vec<u8> {
        let mut glyph = Vec::new();
        for value in [1_i16, 0, 0, side, side] {
            glyph.extend_from_slice(&value.to_be_bytes());
        }
        glyph.extend_from_slice(&3_u16.to_be_bytes());
        glyph.extend_from_slice(&0_u16.to_be_bytes());
        glyph.extend_from_slice(&[0x01; 4]);
        for delta in [0_i16, side, 0, -side, 0, 0, side, 0] {
            glyph.extend_from_slice(&delta.to_be_bytes());
        }
        glyph
    }

    fn format4(pairs: &[(u16, u16)]) -> Vec<u8> {
        let mut segments: Vec<(u16, u16, i16)> = pairs
            .iter()
            .map(|(character, glyph)| {
                (
                    *character,
                    *character,
                    glyph.wrapping_sub(*character).cast_signed(),
                )
            })
            .collect();
        segments.sort_unstable();
        segments.push((0xFFFF, 0xFFFF, 1));
        let count = u16::try_from(segments.len()).expect("segments");
        let mut out = Vec::new();
        out.extend_from_slice(&4_u16.to_be_bytes());
        out.extend_from_slice(&(16 + count * 8).to_be_bytes());
        out.extend_from_slice(&0_u16.to_be_bytes());
        out.extend_from_slice(&(count * 2).to_be_bytes());
        out.extend_from_slice(&[0; 6]);
        for (_, end, _) in &segments {
            out.extend_from_slice(&end.to_be_bytes());
        }
        out.extend_from_slice(&0_u16.to_be_bytes());
        for (start, _, _) in &segments {
            out.extend_from_slice(&start.to_be_bytes());
        }
        for (_, _, delta) in &segments {
            out.extend_from_slice(&delta.to_be_bytes());
        }
        for _ in &segments {
            out.extend_from_slice(&0_u16.to_be_bytes());
        }
        out
    }

    fn face_of(entries: &[(u8, i16, bool)]) -> Arc<GlyphProgram> {
        let mut glyf = Vec::new();
        let mut loca = Vec::new();
        let mut widths: Vec<u16> = vec![0, 0];
        let mut pairs: Vec<(u16, u16)> = Vec::new();
        loca.extend_from_slice(&0_u16.to_be_bytes());
        loca.extend_from_slice(&0_u16.to_be_bytes());
        for (at, (letter, advance, ink)) in entries.iter().enumerate() {
            if *ink {
                glyf.extend_from_slice(&square(*advance));
            }
            loca.extend_from_slice(&u16::try_from(glyf.len() / 2).expect("loca").to_be_bytes());
            widths.push(advance.cast_unsigned());
            widths.push(0);
            pairs.push((u16::from(*letter), u16::try_from(at + 1).expect("glyph")));
        }
        let count = u16::try_from(entries.len() + 1).expect("glyph count");
        let mut head = vec![0_u8; 54];
        head[18..20].copy_from_slice(&1000_u16.to_be_bytes());
        head[40..42].copy_from_slice(&500_i16.to_be_bytes());
        head[42..44].copy_from_slice(&500_i16.to_be_bytes());
        let mut maxp = vec![0_u8; 6];
        maxp[..4].copy_from_slice(&0x0000_5000_u32.to_be_bytes());
        maxp[4..6].copy_from_slice(&count.to_be_bytes());
        let mut hhea = vec![0_u8; 36];
        hhea[4..6].copy_from_slice(&800_i16.to_be_bytes());
        hhea[6..8].copy_from_slice(&(-200_i16).to_be_bytes());
        hhea[34..36].copy_from_slice(&count.to_be_bytes());
        let hmtx: Vec<u8> = widths
            .iter()
            .flat_map(|value| value.to_be_bytes())
            .collect();
        let subtable = format4(&pairs);
        let mut cmap = 0_u16.to_be_bytes().to_vec();
        cmap.extend_from_slice(&1_u16.to_be_bytes());
        cmap.extend_from_slice(&3_u16.to_be_bytes());
        cmap.extend_from_slice(&1_u16.to_be_bytes());
        cmap.extend_from_slice(&12_u32.to_be_bytes());
        cmap.extend_from_slice(&subtable);
        let tables: [(&[u8; 4], Vec<u8>); 7] = [
            (b"cmap", cmap),
            (b"glyf", glyf),
            (b"head", head),
            (b"hhea", hhea),
            (b"hmtx", hmtx),
            (b"loca", loca),
            (b"maxp", maxp),
        ];
        let mut out = 0x0001_0000_u32.to_be_bytes().to_vec();
        out.extend_from_slice(&u16::try_from(tables.len()).expect("count").to_be_bytes());
        out.extend_from_slice(&[0; 6]);
        let mut at = 12 + tables.len() * 16;
        let mut bodies = Vec::new();
        for (tag, bytes) in &tables {
            out.extend_from_slice(*tag);
            out.extend_from_slice(&[0; 4]);
            out.extend_from_slice(&u32::try_from(at).expect("at").to_be_bytes());
            out.extend_from_slice(&u32::try_from(bytes.len()).expect("length").to_be_bytes());
            bodies.extend_from_slice(bytes);
            at += bytes.len();
        }
        out.extend_from_slice(&bodies);
        Arc::new(GlyphProgram::parse(out).expect("the face parses"))
    }

    fn face() -> Arc<GlyphProgram> {
        face_of(&[(b'A', 500, true), (b'B', 250, true), (b' ', 250, false)])
    }

    fn other_face() -> Arc<GlyphProgram> {
        face_of(&[(b'Z', 500, true)])
    }

    #[derive(Debug)]
    struct OneFace {
        program: Arc<GlyphProgram>,
        other: Arc<GlyphProgram>,
    }

    impl FontProvider for OneFace {
        fn primary_face(&self, request: &FontRequest) -> Option<SubstitutedFace> {
            if request.family == "Other Face" {
                return Some(SubstitutedFace {
                    program: Arc::clone(&self.other),
                    identity: Arc::new(pdf_content::FaceIdentity {
                        family: "Other Face".to_owned(),
                        subfamily: "Regular".to_owned(),
                        origin: "test:other".to_owned(),
                        sha256: "fedcba9876543210".to_owned(),
                        face_index: 0,
                        style: pdf_content::FontStyle::default(),
                    }),
                    reason: pdf_content::SubstitutionReason::ExactFamily,
                });
            }
            if request.family != "Test Face" {
                return None;
            }
            Some(SubstitutedFace {
                program: Arc::clone(&self.program),
                identity: Arc::new(pdf_content::FaceIdentity {
                    family: "Test Face".to_owned(),
                    subfamily: "Regular".to_owned(),
                    origin: "test:one".to_owned(),
                    sha256: "0123456789abcdef".to_owned(),
                    face_index: 0,
                    style: pdf_content::FontStyle::default(),
                }),
                reason: pdf_content::SubstitutionReason::ExactFamily,
            })
        }

        fn fallback_face(
            &self,
            _request: &FontRequest,
            character: char,
        ) -> Option<SubstitutedFace> {
            self.primary_face(&FontRequest::for_family(
                "Test Face",
                pdf_content::FontStyle::default(),
            ))
            .filter(|face| super::draws(face, character))
        }

        fn description(&self) -> String {
            "one synthetic face".to_owned()
        }
    }

    pub(crate) fn provider() -> Arc<dyn FontProvider> {
        Arc::new(OneFace {
            program: face(),
            other: other_face(),
        })
    }

    fn document(content: &str) -> ByteStore {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /MediaBox [0 0 200 200] /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ProcSet [/PDF] >> >>"
                .to_owned(),
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ),
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

    fn pens(source: &ByteStore, fonts: &Arc<dyn FontProvider>) -> Vec<(f64, f64)> {
        let reading = read_page(source, 0, b"", Some(fonts)).expect("the page reads");
        reading
            .graph
            .atoms
            .iter()
            .filter_map(|atom| match &atom.kind {
                PaintAtomKind::Text(text) => Some(text),
                _ => None,
            })
            .flat_map(|text| {
                text.glyphs.iter().map(|glyph| {
                    let at = text
                        .state
                        .ctm
                        .value
                        .multiply(glyph.matrix)
                        .transform(pdf_paint::Point { x: 0.0, y: 0.0 });
                    (at.x, at.y)
                })
            })
            .collect()
    }

    fn command(text: &str, frame: [f64; 4]) -> Command {
        Command::PlaceNewText {
            paragraph: crate::plan::ParagraphLayout::default(),
            page_index: 0,
            frame,
            text: text.to_owned(),
            family: "Test Face".to_owned(),
            size: 10.0,
            bold: false,
            italic: false,
            fill: None,
        }
    }

    #[test]
    fn a_justified_line_fills_its_frame_and_the_last_line_does_not() {
        let fonts = provider();
        let source = document("");
        let text = "AA AA AA AA AA AA AA AA AA AA";
        let frame = [20.0, 60.0, 120.0, 140.0];
        let placed = |alignment| {
            let mut command = command(text, frame);
            if let Command::PlaceNewText { paragraph, .. } = &mut command {
                paragraph.alignment = Some(alignment);
            }
            let plan = plan_command_with_fonts(&source, &command, b"", Some(Arc::clone(&fonts)))
                .expect("the text is planned");
            let after = plan.commit(&source, b"").expect("the plan commits");
            pens(&after, &fonts)
        };
        let plain = placed(crate::layout::Alignment::Start);
        let justified = placed(crate::layout::Alignment::Justify);
        assert_eq!(plain.len(), justified.len(), "{plain:?} {justified:?}");
        let gap = 2.5 / 7.0;
        for word in 0..8_u8 {
            let at = justified[usize::from(word) * 3].0;
            let wanted = 20.0 + f64::from(word) * (12.5 + gap);
            assert!((at - wanted).abs() < 1e-6, "word {word}: {justified:?}");
        }
        assert!(
            (justified[22].0 + 5.0 - 120.0).abs() < 1e-6,
            "{justified:?}"
        );
        assert!((plain[22].0 + 5.0 - 117.5).abs() < 1e-6, "{plain:?}");
        for glyph in 24..justified.len() {
            assert!(
                (justified[glyph].0 - plain[glyph].0).abs() < 1e-6,
                "the last line moved: {justified:?} {plain:?}"
            );
        }
    }

    #[test]
    fn text_flows_round_a_drawing_standing_in_its_frame() {
        let fonts = provider();
        let source = document("0 0 0 rg 20 120 60 20 re f");
        let text = vec!["AA"; 40].join(" ");
        let text = text.as_str();
        let frame = [20.0, 60.0, 180.0, 140.0];
        let placed = |flow_round| {
            let mut command = command(text, frame);
            if let Command::PlaceNewText { paragraph, .. } = &mut command {
                paragraph.flow_round = flow_round;
            }
            let plan = plan_command_with_fonts(&source, &command, b"", Some(Arc::clone(&fonts)))
                .expect("the text is planned");
            let after = plan.commit(&source, b"").expect("the plan commits");
            let mut lines: std::collections::BTreeMap<i64, f64> = std::collections::BTreeMap::new();
            for (x, y) in pens(&after, &fonts) {
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "a baseline rounded to a thousandth, only to group the glyphs of one line"
                )]
                let key = (y * 1000.0).round() as i64;
                lines
                    .entry(key)
                    .and_modify(|at| *at = at.min(x))
                    .or_insert(x);
            }
            let mut starts: Vec<f64> = lines.into_iter().rev().map(|(_, x)| x).collect();
            starts.truncate(3);
            starts
        };
        assert_eq!(placed(false), vec![20.0, 20.0, 20.0]);
        assert_eq!(placed(true), vec![83.0, 83.0, 20.0]);
    }

    #[derive(Debug)]
    struct Thai(Arc<GlyphProgram>);

    impl FontProvider for Thai {
        fn primary_face(&self, request: &FontRequest) -> Option<SubstitutedFace> {
            (request.family == "Noto Sans Thai").then(|| SubstitutedFace {
                program: Arc::clone(&self.0),
                identity: Arc::new(pdf_content::FaceIdentity {
                    family: "Noto Sans Thai".to_owned(),
                    subfamily: "Regular".to_owned(),
                    origin: "fonts/packaged/NotoSansThai-Regular.ttf".to_owned(),
                    sha256: "packaged".to_owned(),
                    face_index: 0,
                    style: pdf_content::FontStyle::default(),
                }),
                reason: pdf_content::SubstitutionReason::ExactFamily,
            })
        }

        fn fallback_face(&self, request: &FontRequest, _: char) -> Option<SubstitutedFace> {
            self.primary_face(request)
        }

        fn description(&self) -> String {
            "the packaged Thai face".to_owned()
        }
    }

    #[test]
    fn thai_marks_stand_where_the_shaper_puts_them() {
        let thai = Arc::new(
            GlyphProgram::parse(
                include_bytes!("../../../fonts/packaged/NotoSansThai-Regular.ttf").to_vec(),
            )
            .expect("the packaged face parses"),
        );
        let fonts: Arc<dyn FontProvider> = Arc::new(Thai(thai));
        let source = document("0 0 0 rg 10 10 20 20 re f");
        let command = Command::PlaceNewText {
            paragraph: crate::plan::ParagraphLayout::default(),
            page_index: 0,
            frame: [20.0, 100.0, 180.0, 140.0],
            text: "ปั๊".to_owned(),
            family: "Noto Sans Thai".to_owned(),
            size: 12.0,
            bold: false,
            italic: false,
            fill: None,
        };
        let plan = plan_command_with_fonts(&source, &command, b"", Some(Arc::clone(&fonts)))
            .expect("Thai is planned");
        let after = plan.commit(&source, b"").expect("the plan commits");
        let reading = read_page(&after, 0, b"", Some(&fonts)).expect("the page reads");
        let mut glyphs = Vec::new();
        for atom in &reading.graph.atoms {
            let PaintAtomKind::Text(text) = &atom.kind else {
                continue;
            };
            for glyph in &text.glyphs {
                let at = text
                    .state
                    .ctm
                    .value
                    .multiply(glyph.matrix)
                    .transform(pdf_paint::Point { x: 0.0, y: 0.0 });
                let code = pdf_content::Code {
                    value: glyph.code.value,
                    byte_len: glyph.code.bytes.len(),
                };
                let read = text
                    .text
                    .text_of(code)
                    .map(|meaning| meaning.text.clone())
                    .unwrap_or_default();
                glyphs.push((read, at.x, at.y - 128.0));
            }
        }
        let wanted = [("ป", 20.0, 0.0), ("ั", 27.248, 0.0), ("๊", 25.004, -0.48)];
        assert_eq!(glyphs.len(), wanted.len(), "{glyphs:?}");
        for ((read, x, rise), (text, at, lifted)) in glyphs.iter().zip(wanted) {
            assert!(
                read == text && (x - at).abs() < 1e-6 && (rise - lifted).abs() < 1e-6,
                "{glyphs:?}"
            );
        }
    }

    #[test]
    fn thai_text_copies_out_as_typed() {
        let thai = Arc::new(
            GlyphProgram::parse(
                include_bytes!("../../../fonts/packaged/NotoSansThai-Regular.ttf").to_vec(),
            )
            .expect("the packaged face parses"),
        );
        let fonts: Arc<dyn FontProvider> = Arc::new(Thai(thai));
        let source = document("0 0 0 rg 10 10 20 20 re f");
        let typed = "ที่นี่ น้ำ ภาษาไทย ปั๊ม";
        let command = Command::PlaceNewText {
            paragraph: crate::plan::ParagraphLayout::default(),
            page_index: 0,
            frame: [10.0, 100.0, 190.0, 140.0],
            text: typed.to_owned(),
            family: "Noto Sans Thai".to_owned(),
            size: 10.0,
            bold: false,
            italic: false,
            fill: None,
        };
        let plan = plan_command_with_fonts(&source, &command, b"", Some(Arc::clone(&fonts)))
            .expect("Thai is planned");
        let after = plan.commit(&source, b"").expect("the plan commits");
        let reading = read_page(&after, 0, b"", Some(&fonts)).expect("the page reads");
        let mut read = String::new();
        for atom in &reading.graph.atoms {
            let PaintAtomKind::Text(text) = &atom.kind else {
                continue;
            };
            for glyph in &text.glyphs {
                let code = pdf_content::Code {
                    value: glyph.code.value,
                    byte_len: glyph.code.bytes.len(),
                };
                if let Some(meaning) = text.text.text_of(code) {
                    read.push_str(&meaning.text);
                }
            }
        }
        assert_eq!(read, typed.replace("น้ำ", "น\u{e4d}\u{e49}า"));
    }

    #[test]
    fn text_added_to_a_page_stands_where_the_frame_puts_it() {
        let source = document("0 0 0 rg 10 10 20 20 re f");
        let fonts = provider();
        let plan = plan_command_with_fonts(
            &source,
            &command("AB", [20.0, 100.0, 180.0, 140.0]),
            b"",
            Some(Arc::clone(&fonts)),
        )
        .expect("the text is planned");
        let after = plan.commit(&source, b"").expect("the plan commits");
        assert_eq!(pens(&after, &fonts), vec![(20.0, 130.0), (25.0, 130.0)]);
    }

    #[test]
    fn text_wider_than_its_frame_is_laid_out_on_more_lines() {
        let source = document("0 0 0 rg 10 10 20 20 re f");
        let fonts = provider();
        let plan = plan_command_with_fonts(
            &source,
            &command("AAAA", [20.0, 100.0, 32.0, 140.0]),
            b"",
            Some(Arc::clone(&fonts)),
        )
        .expect("the text is planned");
        let after = plan.commit(&source, b"").expect("the plan commits");
        let places = pens(&after, &fonts);
        assert_eq!(places.len(), 4);
        assert_eq!(places[0], (20.0, 130.0));
        assert_eq!(places[2], (20.0, 118.0));
    }

    #[test]
    fn a_line_ends_at_a_break_the_text_offers() {
        let source = document("0 0 0 rg 10 10 20 20 re f");
        let fonts = provider();
        let plan = plan_command_with_fonts(
            &source,
            &command("AB AB", [20.0, 100.0, 35.5, 140.0]),
            b"",
            Some(Arc::clone(&fonts)),
        )
        .expect("the text is planned");
        let after = plan.commit(&source, b"").expect("the plan commits");
        let places = pens(&after, &fonts);
        assert_eq!(places.len(), 5);
        assert_eq!(
            places,
            vec![
                (20.0, 130.0),
                (25.0, 130.0),
                (27.5, 130.0),
                (20.0, 118.0),
                (25.0, 118.0),
            ]
        );
    }

    #[test]
    fn a_page_that_leaves_a_transform_still_places_the_text_by_the_frame() {
        let source = document("q 2 0 0 2 0 0 cm 0 0 0 rg 5 5 10 10 re f");
        let fonts = provider();
        let plan = plan_command_with_fonts(
            &source,
            &command("A", [20.0, 100.0, 180.0, 140.0]),
            b"",
            Some(Arc::clone(&fonts)),
        )
        .expect("the text is planned");
        let after = plan.commit(&source, b"").expect("the plan commits");
        assert_eq!(pens(&after, &fonts), vec![(20.0, 130.0)]);
    }

    #[test]
    fn text_the_family_cannot_draw_is_set_in_a_face_that_can() {
        let source = document("0 0 0 rg 10 10 20 20 re f");
        let fonts = provider();
        let mut asked = command("A", [20.0, 100.0, 180.0, 140.0]);
        if let Command::PlaceNewText { family, .. } = &mut asked {
            "Other Face".clone_into(family);
        }
        let plan = plan_command_with_fonts(&source, &asked, b"", Some(Arc::clone(&fonts)))
            .expect("the text is planned");
        let after = plan.commit(&source, b"").expect("the plan commits");
        assert_eq!(pens(&after, &fonts), vec![(20.0, 130.0)]);
    }

    #[test]
    fn text_with_no_face_and_a_frame_with_no_width_are_refused() {
        let source = document("0 0 0 rg 10 10 20 20 re f");
        let fonts = provider();
        for (text, frame) in [
            ("A", [20.0, 100.0, 20.0, 140.0]),
            ("Z", [20.0, 100.0, 180.0, 140.0]),
        ] {
            assert!(
                plan_command_with_fonts(
                    &source,
                    &command(text, frame),
                    b"",
                    Some(Arc::clone(&fonts))
                )
                .is_err(),
                "{text} in {frame:?} is written"
            );
        }
    }
}
