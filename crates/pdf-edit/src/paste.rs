use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as _;

use pdf_bytes::ByteStore;
use pdf_paint::{Matrix, PaintAtomKind, PaintGraph, Point};
use pdf_syntax::Reference;

use crate::carry::Copier;
use crate::copied::{Copied, CopiedObject, CopiedRun, CopiedTextElement, plain};
use crate::new_path::NewPath;
use crate::plan::{
    Capability, Effect, MovedRun, PenStep, Plan, PlannedBody, PlannedWrite, SourceAnchor,
};
use crate::spike_move_text::{PlannerPage, SpikeError, interpret_bytes_of};

fn refused(reason: &'static str) -> SpikeError {
    SpikeError::RetypeUnsupported(reason)
}

const PLAIN_STATE: &str = "<< /Type /ExtGState /CA 1 /ca 1 /BM /Normal /SMask /None /AIS false >>";

pub(crate) fn plan_paste(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    copied: &Copied,
    (dx, dy): (f64, f64),
    elsewhere: Option<(&ByteStore, &[u8])>,
) -> Result<Plan, SpikeError> {
    let credential = page.credential;
    if copied.objects.is_empty() {
        return Err(refused("there is nothing to paste"));
    }
    if !(dx.is_finite() && dy.is_finite()) {
        return Err(refused("a paste lands at a place"));
    }
    let mut shifted: Vec<CopiedObject> = copied
        .objects
        .iter()
        .map(|object| shift(object, dx, dy))
        .collect();
    for object in &shifted {
        checked(object)?;
    }
    let carried = carry_across(
        (source, credential),
        page.restrictions,
        copied,
        &mut shifted,
        elsewhere,
    )?;
    let with_carried;
    let carried_page;
    let (source, page) = if carried.writes.is_empty() {
        (source, page)
    } else {
        with_carried = crate::block_rewrite::commit_writes(
            source,
            &carried.writes,
            (credential, page.restrictions),
        )?;
        carried_page = read_again(&with_carried, page_index, page)?;
        (&with_carried, beside(&carried_page, page))
    };

    let stream = last_stream(&page)?;
    let standing = standing_state(&page, stream)?;
    let into =
        standing.ctm.value.inverse().ok_or_else(|| {
            refused("this page leaves a transform a paste cannot be placed through")
        })?;

    let mut naming = Naming::new(source, page);
    let state = if plain(&standing) {
        None
    } else {
        Some(naming.state(PLAIN_STATE)?)
    };
    let names = named_in_the_page(&mut naming, &shifted)?;
    let Naming {
        writes: for_resources,
        document,
        ..
    } = naming;
    let mut writes = carried.writes;
    writes.extend(for_resources);

    let named_page;
    let page = if writes.is_empty() {
        page
    } else {
        named_page =
            crate::spike_move_text::read_page(&document, page_index, page.credential, page.fonts)?;
        PlannerPage {
            program: &named_page.program,
            operations: &named_page.operations,
            graph: &named_page.graph,
            fonts: page.fonts,
            restrictions: page.restrictions,
            credential: page.credential,
        }
    };
    let stream = last_stream(&page)?;
    let decoded = page.program.streams[stream].bytes.as_bytes();
    let bytes = candidate(decoded, &shifted, &names, (state.as_deref(), into));
    let graph = interpret_bytes_of(page.program, stream, &bytes, page.fonts)?;
    prove_pasted(page.graph, &graph, &shifted)?;

    let first = page.graph.atoms.len();
    let pasted = &graph.atoms[first..];
    let region = region(pasted, &shifted)?;
    writes.push(PlannedWrite {
        reference: page.program.streams[stream].reference,
        body: PlannedBody::ReplacedStream { decoded: bytes },
    });
    Ok(Plan::new(
        Capability::Exact,
        writes,
        Effect {
            page_index,
            moved: pasted
                .iter()
                .enumerate()
                .map(|(offset, atom)| MovedRun {
                    anchor: SourceAnchor::of(&atom.id),
                    atom_ordinal: first + offset,
                    original_matrix: Matrix::IDENTITY,
                })
                .collect(),
            target_stream: page.program.streams[stream].reference,
            declared_region: Some(region),
        },
    ))
}

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

fn named_in_the_page(
    naming: &mut Naming<'_>,
    shifted: &[CopiedObject],
) -> Result<Vec<String>, SpikeError> {
    let mut names = Vec::with_capacity(shifted.len());
    for object in shifted {
        names.push(match object {
            CopiedObject::Picture { image, .. } => naming.resource((b"/XObject", "Im"), *image)?,
            CopiedObject::Text(run) => naming.resource((b"/Font", "F"), run.font)?,
            CopiedObject::Drawing { stroke, .. } => {
                match crate::new_path::gstate_entries(*stroke) {
                    Some(entries) => naming.state(&entries)?,
                    None => String::new(),
                }
            }
        });
    }
    Ok(names)
}

fn read_again(
    source: &ByteStore,
    page_index: usize,
    had: PlannerPage<'_>,
) -> Result<crate::spike_move_text::PageReading, SpikeError> {
    crate::spike_move_text::read_page(source, page_index, had.credential, had.fonts)
}

fn beside<'a>(
    read: &'a crate::spike_move_text::PageReading,
    had: PlannerPage<'a>,
) -> PlannerPage<'a> {
    PlannerPage {
        program: &read.program,
        operations: &read.operations,
        graph: &read.graph,
        fonts: had.fonts,
        restrictions: had.restrictions,
        credential: had.credential,
    }
}

struct Carried {
    writes: Vec<PlannedWrite>,
}

fn carry_across(
    (source, credential): (&ByteStore, &[u8]),
    restrictions: crate::Restrictions,
    copied: &Copied,
    shifted: &mut [CopiedObject],
    elsewhere: Option<(&ByteStore, &[u8])>,
) -> Result<Carried, SpikeError> {
    if copied.from == source.id() {
        return Ok(Carried { writes: Vec::new() });
    }
    let Some((other, password)) = elsewhere else {
        return Err(refused(
            "this was copied from another document, whose objects are not here to be pasted",
        ));
    };
    let _ = restrictions;
    if other.id().get() == source.id().get() {
        return Err(refused(
            "the document this was copied from is named as this one, so a span of either would resolve in the other",
        ));
    }
    let (index, security) = crate::previous::readable_index(other, password)
        .ok_or_else(|| refused("the document this was copied from cannot be read"))?;
    let mut carrier = Copier {
        other,
        index: &index,
        tree: HashSet::new(),
        numbers: HashMap::new(),
        queue: VecDeque::new(),
        next: crate::block_rewrite::next_object_number(source)?,
        writes: Vec::new(),
        security,
        into: crate::previous::readable_index(source, credential).and_then(|(_, into)| into),
    };
    for object in shifted.iter_mut() {
        match object {
            CopiedObject::Picture { image, .. } => *image = carrier.number_of(*image)?,
            CopiedObject::Text(run) => run.font = carrier.number_of(run.font)?,
            CopiedObject::Drawing { .. } => {}
        }
    }
    carrier.copy_reached()?;
    Ok(Carried {
        writes: carrier.writes,
    })
}

fn shift(object: &CopiedObject, dx: f64, dy: f64) -> CopiedObject {
    let by = translation(dx, dy);
    match object {
        CopiedObject::Picture { image, placement } => CopiedObject::Picture {
            image: *image,
            placement: by.multiply(*placement),
        },
        CopiedObject::Drawing {
            steps,
            closed,
            stroke,
            fill,
        } => CopiedObject::Drawing {
            steps: steps
                .iter()
                .map(|step| {
                    let moved = |(x, y): (f64, f64)| (x + dx, y + dy);
                    match step {
                        PenStep::Move(point) => PenStep::Move(moved(*point)),
                        PenStep::Line(point) => PenStep::Line(moved(*point)),
                        PenStep::Curve(one, other, end) => {
                            PenStep::Curve(moved(*one), moved(*other), moved(*end))
                        }
                    }
                })
                .collect(),
            closed: *closed,
            stroke: *stroke,
            fill: *fill,
        },
        CopiedObject::Text(run) => CopiedObject::Text(CopiedRun {
            matrix: by.multiply(run.matrix),
            glyphs: run
                .glyphs
                .iter()
                .map(|glyph| crate::copied::CopiedGlyph {
                    placed: by.multiply(glyph.placed),
                    ..glyph.clone()
                })
                .collect(),
            elements: run.elements.clone(),
            ..*run
        }),
    }
}

fn checked(object: &CopiedObject) -> Result<(), SpikeError> {
    let finite = |m: Matrix| [m.a, m.b, m.c, m.d, m.e, m.f].iter().all(|v| v.is_finite());
    match object {
        CopiedObject::Picture { placement, .. } => {
            if !finite(*placement) || placement.inverse().is_none() {
                return Err(refused("a pasted picture has to be given some room"));
            }
        }
        CopiedObject::Drawing {
            steps,
            closed,
            stroke,
            fill,
        } => crate::new_path::checked(&NewPath {
            steps,
            closed: *closed,
            stroke: *stroke,
            fill: *fill,
        })?,
        CopiedObject::Text(run) => {
            let numbers = [
                run.size,
                run.horizontal_scaling,
                run.character_spacing,
                run.word_spacing,
                run.rise,
            ];
            if !finite(run.matrix) || !numbers.iter().all(|v| v.is_finite()) {
                return Err(refused("a pasted run is made of numbers"));
            }
            if run.glyphs.is_empty() || run.elements.is_empty() {
                return Err(refused("a pasted run shows something"));
            }
            if !run.fill.iter().all(|part| (0.0..=1.0).contains(part)) {
                return Err(refused("a colour is three numbers from zero to one"));
            }
        }
    }
    Ok(())
}

fn last_stream(page: &PlannerPage<'_>) -> Result<usize, SpikeError> {
    page.program
        .streams
        .len()
        .checked_sub(1)
        .ok_or_else(|| refused("a page with no content stream cannot be pasted onto"))
}

fn standing_state(
    page: &PlannerPage<'_>,
    stream: usize,
) -> Result<pdf_paint::GraphicsState, SpikeError> {
    let decoded = page.program.streams[stream].bytes.as_bytes();
    let mut probe = Vec::with_capacity(decoded.len() + 24);
    probe.extend_from_slice(decoded);
    probe.extend_from_slice(b"\nq 0 0 m 1 1 l S Q\n");
    let measured = interpret_bytes_of(page.program, stream, &probe, page.fonts)?;
    match measured
        .atoms
        .get(page.graph.atoms.len())
        .map(|atom| &atom.kind)
    {
        Some(PaintAtomKind::Path(path)) => Ok(path.state.clone()),
        _ => Err(refused("what this page leaves in force cannot be measured")),
    }
}

struct Naming<'a> {
    source: &'a ByteStore,
    page: Reference,
    resources: &'a pdf_content::PageResources,
    restrictions: crate::Restrictions,
    credential: &'a [u8],
    document: ByteStore,
    writes: Vec<PlannedWrite>,
    named: Vec<(Vec<u8>, Reference, String)>,
    states: Vec<(String, Reference)>,
}

impl<'a> Naming<'a> {
    fn new(source: &'a ByteStore, page: PlannerPage<'a>) -> Self {
        Self {
            source,
            page: page.program.page,
            resources: &page.program.resources,
            restrictions: page.restrictions,
            credential: page.credential,
            document: source.clone(),
            writes: Vec::new(),
            named: Vec::new(),
            states: Vec::new(),
        }
    }

    fn entries(&self, category: &[u8]) -> &'a [pdf_content::ResourceEntry] {
        match category {
            b"/XObject" => self.resources.xobjects(),
            b"/Font" => self.resources.fonts(),
            _ => self.resources.ext_gstates(),
        }
    }

    fn write(&mut self, write: PlannedWrite) -> Result<(), SpikeError> {
        self.writes.retain(|had| had.reference != write.reference);
        self.writes.push(write);
        self.document = crate::block_rewrite::commit_writes(
            self.source,
            &self.writes,
            (self.credential, self.restrictions),
        )?;
        Ok(())
    }

    fn resource(
        &mut self,
        (category, prefix): (&[u8], &str),
        object: Reference,
    ) -> Result<String, SpikeError> {
        if let Some(entry) = self
            .entries(category)
            .iter()
            .find(|entry| entry.reference() == Some(object))
        {
            return Ok(name_of(entry));
        }
        if let Some((_, _, name)) = self
            .named
            .iter()
            .find(|(had, reference, _)| had == category && *reference == object)
        {
            return Ok(name.clone());
        }
        let (name, holder) = crate::new_font::add_resource(
            &self.document,
            self.page,
            (category, prefix),
            object,
            self.credential,
        )?;
        self.write(holder)?;
        self.named.push((category.to_vec(), object, name.clone()));
        Ok(name)
    }

    fn state(&mut self, entries: &str) -> Result<String, SpikeError> {
        let says = |entry: &pdf_content::ResourceEntry| {
            let reference = entry.reference()?;
            let body = crate::new_font::resolve(self.source, reference, self.credential).ok()?;
            (body.bytes == entries.as_bytes()).then_some(reference)
        };
        if let Some(entry) = self
            .entries(b"/ExtGState")
            .iter()
            .find(|entry| says(entry).is_some())
        {
            return Ok(name_of(entry));
        }
        let object = if let Some((_, object)) = self.states.iter().find(|(had, _)| had == entries) {
            *object
        } else {
            let object =
                Reference::new(crate::block_rewrite::next_object_number(&self.document)?, 0);
            self.write(PlannedWrite {
                reference: object,
                body: PlannedBody::Direct {
                    body: entries.as_bytes().to_vec(),
                },
            })?;
            self.states.push((entries.to_owned(), object));
            object
        };
        self.resource((b"/ExtGState", "GS"), object)
    }
}

fn name_of(entry: &pdf_content::ResourceEntry) -> String {
    String::from_utf8_lossy(entry.name())
        .trim_start_matches('/')
        .to_owned()
}

fn candidate(
    decoded: &[u8],
    objects: &[CopiedObject],
    names: &[String],
    (state, into): (Option<&str>, Matrix),
) -> Vec<u8> {
    let mut out = Vec::with_capacity(decoded.len() + objects.len() * 96);
    out.extend_from_slice(decoded);
    let mut wrap = String::from("\nq ");
    if let Some(name) = state {
        let _ = write!(wrap, "/{name} gs ");
    }
    let m = [into.a, into.b, into.c, into.d, into.e, into.f].map(|value| value + 0.0);
    let _ = writeln!(
        wrap,
        "{} {} {} {} {} {} cm",
        m[0], m[1], m[2], m[3], m[4], m[5]
    );
    out.extend_from_slice(wrap.as_bytes());
    for (object, name) in objects.iter().zip(names) {
        match object {
            CopiedObject::Picture { placement, .. } => {
                out.extend_from_slice(&crate::new_image::candidate(&[], name, Some(*placement)));
            }
            CopiedObject::Drawing {
                steps,
                closed,
                stroke,
                fill,
            } => {
                let drawing = NewPath {
                    steps,
                    closed: *closed,
                    stroke: *stroke,
                    fill: *fill,
                };
                let state = (!name.is_empty()).then_some(name.as_str());
                out.extend_from_slice(&crate::new_path::candidate(&[], &drawing, state, None));
            }
            CopiedObject::Text(run) => show(&mut out, run, name),
        }
    }
    out.extend_from_slice(b"Q\n");
    out
}

fn show(out: &mut Vec<u8>, run: &CopiedRun, font: &str) {
    let [red, green, blue] = run.fill;
    let m = [
        run.matrix.a,
        run.matrix.b,
        run.matrix.c,
        run.matrix.d,
        run.matrix.e,
        run.matrix.f,
    ]
    .map(|value| value + 0.0);
    let mut text = String::new();
    let _ = write!(
        text,
        "BT /{font} {} Tf {} Tz {} Tc {} Tw {} Ts 0 Tr {red} {green} {blue} rg {} {} {} {} {} {} Tm ",
        run.size,
        run.horizontal_scaling,
        run.character_spacing,
        run.word_spacing,
        run.rise,
        m[0],
        m[1],
        m[2],
        m[3],
        m[4],
        m[5],
    );
    let hex = |codes: &[u8]| {
        let mut text = String::with_capacity(codes.len() * 2 + 2);
        text.push('<');
        for byte in codes {
            let _ = write!(text, "{byte:02X}");
        }
        text.push('>');
        text
    };
    match run.elements.as_slice() {
        [CopiedTextElement::Codes(codes)] => {
            let _ = write!(text, "{} Tj", hex(codes));
        }
        elements => {
            text.push('[');
            for element in elements {
                match element {
                    CopiedTextElement::Codes(codes) => text.push_str(&hex(codes)),
                    CopiedTextElement::Adjustment(value) => {
                        let _ = write!(text, " {} ", value + 0.0);
                    }
                }
            }
            text.push_str("] TJ");
        }
    }
    text.push_str(" ET\n");
    out.extend_from_slice(text.as_bytes());
}

fn prove_pasted(
    before: &PaintGraph,
    after: &PaintGraph,
    objects: &[CopiedObject],
) -> Result<(), SpikeError> {
    if after.atoms.len() != before.atoms.len() + objects.len() {
        return Err(SpikeError::PasteNotIsolated);
    }
    crate::new_text::prove_untouched(before, after)?;
    for (object, atom) in objects.iter().zip(&after.atoms[before.atoms.len()..]) {
        prove_one(object, &atom.kind)?;
    }
    Ok(())
}

fn prove_one(object: &CopiedObject, kind: &PaintAtomKind) -> Result<(), SpikeError> {
    let wrong = || SpikeError::PasteNotIsolated;
    match (object, kind) {
        (CopiedObject::Picture { image, placement }, PaintAtomKind::Image(paint)) => {
            if paint.reference != *image || !plain(&paint.state) {
                return Err(wrong());
            }
            for (x, y) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                let corner = Point { x, y };
                let (got, wanted) = (
                    paint.state.ctm.value.transform(corner),
                    placement.transform(corner),
                );
                if (got.x - wanted.x).abs() > crate::block_move::PLACEMENT_TOLERANCE
                    || (got.y - wanted.y).abs() > crate::block_move::PLACEMENT_TOLERANCE
                {
                    return Err(wrong());
                }
            }
            Ok(())
        }
        (
            CopiedObject::Drawing {
                steps,
                closed,
                stroke,
                fill,
            },
            PaintAtomKind::Path(paint),
        ) => {
            let drawing = NewPath {
                steps,
                closed: *closed,
                stroke: *stroke,
                fill: *fill,
            };
            crate::new_path::drawn_as(paint, &drawing).map_err(|_| wrong())?;
            let state = &paint.state;
            if (state.fill_alpha.value - 1.0).abs() > 1e-9
                || !matches!(state.soft_mask.value, pdf_paint::SoftMask::None)
                || state.alpha_is_shape.value
            {
                return Err(wrong());
            }
            Ok(())
        }
        (CopiedObject::Text(run), PaintAtomKind::Text(paint)) => prove_run(run, paint),
        _ => Err(wrong()),
    }
}

fn prove_run(run: &CopiedRun, paint: &pdf_paint::TextShowPaint) -> Result<(), SpikeError> {
    let wrong = || SpikeError::PasteNotIsolated;
    let font = paint
        .state
        .text
        .font
        .as_ref()
        .and_then(|font| font.value.reference);
    if font != Some(run.font)
        || paint.state.text.rendering_mode.value != pdf_paint::TextRenderingMode::Fill
        || !plain(&paint.state)
        || !crate::new_path::paints(&paint.state.fill_color.value, run.fill)
        || paint.glyphs.len() != run.glyphs.len()
    {
        return Err(wrong());
    }
    let ctm = paint.state.ctm.value;
    for (got, wanted) in paint.glyphs.iter().zip(&run.glyphs) {
        if got.code.value != wanted.code || got.code.cid != wanted.cid || got.glyph != wanted.glyph
        {
            return Err(wrong());
        }
        if !crate::place_object::alike(ctm.multiply(got.matrix), wanted.placed) {
            return Err(wrong());
        }
    }
    Ok(())
}

fn region(
    pasted: &[pdf_paint::PaintAtom],
    objects: &[CopiedObject],
) -> Result<[f64; 4], SpikeError> {
    let mut region = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    let mut take = |x: f64, y: f64, reach: f64| {
        region = [
            region[0].min(x - reach),
            region[1].min(y - reach),
            region[2].max(x + reach),
            region[3].max(y + reach),
        ];
    };
    for object in objects {
        match object {
            CopiedObject::Picture { placement, .. } => {
                for (x, y) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                    let corner = placement.transform(Point { x, y });
                    take(corner.x, corner.y, 0.0);
                }
            }
            CopiedObject::Drawing { steps, stroke, .. } => {
                let reach = stroke.map_or(0.0, |pen| pen.width / 2.0);
                for (x, y) in steps.iter().flat_map(PenStep::points) {
                    take(x, y, reach);
                }
            }
            CopiedObject::Text(run) => {
                for glyph in &run.glyphs {
                    take(glyph.placed.e, glyph.placed.f, 0.0);
                }
            }
        }
    }
    for atom in pasted {
        if let Some([x0, y0, x1, y1]) = atom.kind.user_bounds() {
            take(x0, y0, 0.0);
            take(x1, y1, 0.0);
        }
    }
    if region.iter().all(|value| value.is_finite()) {
        Ok(region)
    } else {
        Err(refused("where the paste landed cannot be said"))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};
    use pdf_paint::{Matrix, PaintAtomKind, PaintGraph};

    use super::{candidate, prove_pasted, shift};
    use crate::block_move::tests::{SQUARE_CFF, hex};
    use crate::copied::{Copied, CopiedObject, copy_from};
    use crate::image_file::tests::png;
    use crate::plan::{Command, PenStep, PenStroke, SourceAnchor};
    use crate::spike_move_text::{interpret_bytes_of, plan_command_with_fonts, read_page};

    fn document(pages: &[&str]) -> ByteStore {
        let program = hex(SQUARE_CFF);
        let mut font_file =
            format!("<< /Subtype /Type1C /Length {} >>\nstream\n", program.len()).into_bytes();
        font_file.extend_from_slice(&program);
        font_file.extend_from_slice(b"\nendstream");
        let mut objects: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            Vec::new(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Test /FirstChar 65 /LastChar 67 /Widths [600 600 600] /FontDescriptor 4 0 R >>".to_vec(),
            b"<< /Type /FontDescriptor /FontName /Test /Flags 4 /FontFile3 5 0 R >>".to_vec(),
            font_file,
        ];
        let mut kids = Vec::new();
        for (index, content) in pages.iter().enumerate() {
            let page = objects.len() + 1;
            kids.push(format!("{page} 0 R"));
            let resources = if index == 0 {
                "/Resources << /Font << /F1 3 0 R >> >>"
            } else {
                "/Resources << >>"
            };
            objects.push(
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] {resources} /Contents {} 0 R >>",
                    page + 1
                )
                .into_bytes(),
            );
            objects.push(
                format!(
                    "<< /Length {} >>\nstream\n{content}\nendstream",
                    content.len()
                )
                .into_bytes(),
            );
        }
        objects[1] = format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            pages.len()
        )
        .into_bytes();
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
            bytes.extend_from_slice(object);
            bytes.extend_from_slice(b"\nendobj\n");
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
        ByteStore::new(SourceId::new(1201), Arc::<[u8]>::from(bytes))
    }

    fn done(source: &ByteStore, command: &Command) -> ByteStore {
        let plan = plan_command_with_fonts(source, command, b"", None).expect("plans");
        plan.commit(source, b"").expect("commits")
    }

    pub(crate) fn three_kinds() -> ByteStore {
        let source = document(&[
            "BT /F1 12 Tf 1 0 0 1 20 100 Tm (ABC) Tj ET",
            "0 0 1 rg 0 0 5 5 re f",
        ]);
        let drawn = done(
            &source,
            &Command::DrawPath {
                page_index: 0,
                steps: vec![PenStep::Move((20.0, 30.0)), PenStep::Line((60.0, 90.0))],
                closed: false,
                stroke: Some(PenStroke::pen([1.0, 0.0, 0.0], 2.0)),
                fill: None,
            },
        );
        done(
            &drawn,
            &Command::PlaceNewImage {
                page_index: 0,
                placement: Matrix {
                    a: 40.0,
                    b: 0.0,
                    c: 0.0,
                    d: 30.0,
                    e: 100.0,
                    f: 120.0,
                },
                file: png((2, 1, 8, 2, 0), &[], &[0, 1, 2, 3, 4, 5, 6]).into(),
            },
        )
    }

    pub(crate) fn anchors(graph: &PaintGraph) -> Vec<SourceAnchor> {
        graph
            .atoms
            .iter()
            .map(|atom| SourceAnchor::of(&atom.id))
            .collect()
    }

    fn glyph_origins(graph: &PaintGraph) -> Vec<(u32, f64, f64)> {
        let mut out = Vec::new();
        for atom in &graph.atoms {
            if let PaintAtomKind::Text(text) = &atom.kind {
                for glyph in &text.glyphs {
                    let at = crate::block_move::glyph_origin(glyph.matrix, text.state.ctm.value);
                    out.push((glyph.code.value, at.x, at.y));
                }
            }
        }
        out
    }

    fn images(source: &ByteStore) -> usize {
        source
            .as_bytes()
            .windows(b"/Subtype /Image".len())
            .filter(|window| *window == b"/Subtype /Image")
            .count()
    }

    pub(crate) fn a_bare_page() -> ByteStore {
        let objects: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Contents 4 0 R >>".to_vec(),
            b"<< /Length 0 >>\nstream\n\nendstream".to_vec(),
        ];
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
            bytes.extend_from_slice(object);
            bytes.extend_from_slice(b"\nendobj\n");
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
        ByteStore::new(SourceId::new(1301), Arc::<[u8]>::from(bytes))
    }

    #[test]
    fn a_copy_is_pasted_into_another_document_carrying_what_it_names() {
        let from = three_kinds();
        let read = read_page(&from, 0, b"", None).expect("reads");
        let copied = copy_from(&read.graph, &anchors(&read.graph), from.id()).expect("copies");
        assert_eq!(copied.objects.len(), 3, "a run, a line and a picture");

        let into = a_bare_page();
        assert_eq!(images(&into), 0, "the second document starts with no image");
        let bare = read_page(&into, 0, b"", None).expect("reads");
        assert!(bare.graph.atoms.is_empty(), "and paints nothing");

        let refused = plan_command_with_fonts(
            &into,
            &Command::PasteObjects {
                page_index: 0,
                copied: copied.clone(),
                dx: 0.0,
                dy: 0.0,
                elsewhere: None,
            },
            b"",
            None,
        );
        assert!(
            refused.is_err(),
            "a copy from elsewhere must not be pasted blind: {refused:?}"
        );

        let after = done(
            &into,
            &Command::PasteObjects {
                page_index: 0,
                copied,
                dx: 0.0,
                dy: 0.0,
                elsewhere: Some((from.clone(), crate::Password::default())),
            },
        );
        let now = read_page(&after, 0, b"", None).expect("reads");
        assert_eq!(
            now.graph.atoms.len(),
            3,
            "the run, the line and the picture are painted in the second document"
        );
        assert_eq!(
            glyph_origins(&now.graph),
            glyph_origins(&read.graph),
            "the same glyphs stand where they stood"
        );
        assert_eq!(
            images(&after),
            1,
            "the image was carried across, once, into a file that had none"
        );
        assert!(
            after
                .as_bytes()
                .windows(b"/FontFile3".len())
                .any(|w| w == b"/FontFile3"),
            "the font program came with it, so the second file stands on its own"
        );
    }

    #[test]
    fn a_copy_pasted_on_its_own_page_names_what_the_page_already_has() {
        let source = three_kinds();
        let before = read_page(&source, 0, b"", None).expect("reads");
        assert_eq!(before.graph.atoms.len(), 3, "a run, a line and a picture");
        let copied =
            copy_from(&before.graph, &anchors(&before.graph), source.id()).expect("copies");
        assert_eq!(copied.objects.len(), 3);
        let plan = plan_command_with_fonts(
            &source,
            &Command::PasteObjects {
                page_index: 0,
                copied,
                dx: 15.0,
                dy: -20.0,
                elsewhere: None,
            },
            b"",
            None,
        )
        .expect("plans");
        assert_eq!(
            plan.writes().len(),
            1,
            "the content stream, and nothing else"
        );
        let after_source = plan.commit(&source, b"").expect("commits");
        assert_eq!(images(&after_source), 1, "the samples are in the file once");
        let after = read_page(&after_source, 0, b"", None).expect("reads");
        assert_eq!(after.graph.atoms.len(), 6);
        let (was, now) = (glyph_origins(&before.graph), glyph_origins(&after.graph));
        assert_eq!(now.len(), was.len() * 2);
        for (one, other) in was.iter().zip(&now[was.len()..]) {
            assert_eq!(one.0, other.0);
            assert!(
                (one.1 + 15.0 - other.1).abs() < 1e-6 && (one.2 - 20.0 - other.2).abs() < 1e-6,
                "{now:?}"
            );
        }
        let (PaintAtomKind::Image(was), PaintAtomKind::Image(now)) =
            (&before.graph.atoms[2].kind, &after.graph.atoms[5].kind)
        else {
            panic!("the picture is third and sixth");
        };
        assert_eq!(was.reference, now.reference, "the same image object");
        let (a, b) = (was.state.ctm.value, now.state.ctm.value);
        assert!(
            (b.e - a.e - 15.0).abs() < 1e-6 && (b.f - a.f + 20.0).abs() < 1e-6,
            "{b:?}"
        );
        assert_eq!((a.a, a.d), (b.a, b.d), "the same size");
        let (PaintAtomKind::Path(was), PaintAtomKind::Path(now)) =
            (&before.graph.atoms[1].kind, &after.graph.atoms[4].kind)
        else {
            panic!("the line is second and fifth");
        };
        let point = |path: &pdf_paint::PathPaint| {
            let pdf_paint::PathSegment::MoveTo { point, .. } = path.path.segments[0] else {
                panic!("a move")
            };
            path.state.ctm.value.transform(point)
        };
        let (a, b) = (point(was), point(now));
        assert!((b.x - a.x - 15.0).abs() < 1e-6 && (b.y - a.y + 20.0).abs() < 1e-6);
        assert!((now.state.line_width.value - 2.0).abs() < 1e-9);
    }

    #[test]
    fn a_copy_pasted_on_another_page_is_named_there_and_copied_nowhere() {
        let source = three_kinds();
        let before = read_page(&source, 0, b"", None).expect("reads");
        let copied =
            copy_from(&before.graph, &anchors(&before.graph), source.id()).expect("copies");
        let plan = plan_command_with_fonts(
            &source,
            &Command::PasteObjects {
                page_index: 1,
                copied,
                dx: 0.0,
                dy: 0.0,
                elsewhere: None,
            },
            b"",
            None,
        )
        .expect("plans");
        assert!(
            plan.writes().len() >= 2,
            "the page's resources and its content stream: {}",
            plan.writes().len()
        );
        let after_source = plan.commit(&source, b"").expect("commits");
        assert_eq!(images(&after_source), 1);
        let after = read_page(&after_source, 1, b"", None).expect("reads");
        assert_eq!(
            after.graph.atoms.len(),
            4,
            "the square and the three pasted"
        );
        assert_eq!(
            glyph_origins(&after.graph),
            glyph_origins(&before.graph),
            "in place"
        );
        let PaintAtomKind::Image(now) = &after.graph.atoms[3].kind else {
            panic!("the picture is last");
        };
        let PaintAtomKind::Image(was) = &before.graph.atoms[2].kind else {
            panic!("the picture was last");
        };
        assert_eq!(was.reference, now.reference);
        assert_eq!(was.samples, now.samples);
        assert_eq!(
            read_page(&after_source, 0, b"", None)
                .expect("reads")
                .graph
                .atoms
                .len(),
            3,
            "the page it came from is untouched"
        );
    }

    #[test]
    fn a_paste_is_one_plan_however_many_objects_it_holds() {
        let source = three_kinds();
        let before = read_page(&source, 0, b"", None).expect("reads");
        let copied =
            copy_from(&before.graph, &anchors(&before.graph), source.id()).expect("copies");
        let plan = plan_command_with_fonts(
            &source,
            &Command::PasteObjects {
                page_index: 0,
                copied,
                dx: 1.0,
                dy: 1.0,
                elsewhere: None,
            },
            b"",
            None,
        )
        .expect("plans");
        assert_eq!(plan.effect().moved.len(), 3, "three atoms in one effect");
        assert_eq!(plan.effect().page_index, 0);
    }

    #[test]
    fn the_proof_refuses_a_paste_that_landed_elsewhere_or_lost_an_atom() {
        let source = three_kinds();
        let reading = read_page(&source, 0, b"", None).expect("reads");
        let copied =
            copy_from(&reading.graph, &anchors(&reading.graph), source.id()).expect("copies");
        let named_as_written = |objects: &[CopiedObject]| -> Vec<String> {
            objects
                .iter()
                .map(|object| match object {
                    CopiedObject::Picture { .. } => "Im1".to_owned(),
                    CopiedObject::Drawing { .. } => String::new(),
                    CopiedObject::Text(_) => "F1".to_owned(),
                })
                .collect()
        };
        let decoded = reading.program.streams[0].bytes.as_bytes();
        let wanted: Vec<CopiedObject> = copied
            .objects
            .iter()
            .map(|o| shift(o, 15.0, -20.0))
            .collect();
        let bytes = candidate(
            decoded,
            &wanted,
            &named_as_written(&wanted),
            (None, Matrix::IDENTITY),
        );
        let graph = interpret_bytes_of(&reading.program, 0, &bytes, None).expect("interprets");
        prove_pasted(&reading.graph, &graph, &wanted).expect("the true paste proves");

        let elsewhere: Vec<CopiedObject> = copied
            .objects
            .iter()
            .map(|o| shift(o, 16.0, -20.0))
            .collect();
        let bytes = candidate(
            decoded,
            &elsewhere,
            &named_as_written(&elsewhere),
            (None, Matrix::IDENTITY),
        );
        let graph = interpret_bytes_of(&reading.program, 0, &bytes, None).expect("interprets");
        assert!(
            prove_pasted(&reading.graph, &graph, &wanted).is_err(),
            "a point off is refused"
        );

        let bytes = candidate(
            b"",
            &wanted,
            &named_as_written(&wanted),
            (None, Matrix::IDENTITY),
        );
        let graph = interpret_bytes_of(&reading.program, 0, &bytes, None).expect("interprets");
        assert!(
            prove_pasted(&reading.graph, &graph, &wanted).is_err(),
            "the page's own atoms gone is refused"
        );
    }

    #[test]
    fn a_page_that_leaves_a_transform_still_pastes_in_default_user_space() {
        let source = document(&["BT /F1 12 Tf 1 0 0 1 20 100 Tm (AB) Tj ET 2 0 0 2 30 40 cm"]);
        let before = read_page(&source, 0, b"", None).expect("reads");
        let copied =
            copy_from(&before.graph, &anchors(&before.graph), source.id()).expect("copies");
        let after_source = done(
            &source,
            &Command::PasteObjects {
                page_index: 0,
                copied,
                dx: 5.0,
                dy: 5.0,
                elsewhere: None,
            },
        );
        let after = read_page(&after_source, 0, b"", None).expect("reads");
        let (was, now) = (glyph_origins(&before.graph), glyph_origins(&after.graph));
        for (one, other) in was.iter().zip(&now[was.len()..]) {
            assert!(
                (one.1 + 5.0 - other.1).abs() < 1e-6 && (one.2 + 5.0 - other.2).abs() < 1e-6,
                "{now:?}"
            );
        }
    }

    #[test]
    fn what_cannot_be_copied_or_pasted_is_refused() {
        let source = document(&["BT /F1 12 Tf 1 0 0 1 20 100 Tm 0.5 0 0 0 k (AB) Tj ET"]);
        let before = read_page(&source, 0, b"", None).expect("reads");
        let refused = copy_from(&before.graph, &anchors(&before.graph), source.id());
        assert!(
            matches!(
                refused,
                Err(crate::spike_move_text::SpikeError::CopyUnsupported(_))
            ),
            "{refused:?}"
        );
        let empty = plan_command_with_fonts(
            &source,
            &Command::PasteObjects {
                page_index: 0,
                copied: Copied {
                    objects: Vec::new(),
                    from: source.id(),
                },
                dx: 0.0,
                dy: 0.0,
                elsewhere: None,
            },
            b"",
            None,
        );
        assert!(empty.is_err());
    }
}
