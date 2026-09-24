use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};

use pdf_content::decipher::{Cipher, Features, Shapes, decipher};
use pdf_content::outline_match::Raster;
use pdf_content::{Code, FontProvider, FontRequest, ToUnicode};

use crate::graph::{PaintAtomKind, PaintGraph};
use crate::reading_order::{PlacedInk, reading_order};

pub fn read_lying_fonts(
    graph: &mut PaintGraph,
    fonts: Option<&Arc<dyn FontProvider>>,
    page: pdf_syntax::Reference,
) {
    let Some(provider) = fonts else {
        return;
    };
    let regions = provider.decipher_regions(page);
    if regions.is_empty() {
        return;
    }
    for atoms in fonts_of(graph) {
        let inside: Vec<usize> = atoms
            .iter()
            .copied()
            .filter(|at| {
                first_origin(graph, *at).is_some_and(|(x, y)| {
                    regions.iter().any(|region| {
                        region[0] <= x && x <= region[2] && region[1] <= y && y <= region[3]
                    })
                })
            })
            .collect();
        if inside.is_empty() {
            continue;
        }
        let Some(read) = verdict(graph, &atoms, provider.as_ref()) else {
            continue;
        };
        let mut rewritten: HashMap<*const ToUnicode, Arc<ToUnicode>> = HashMap::new();
        for at in inside {
            if let PaintAtomKind::Text(text) = &mut graph.atoms[at].kind {
                let table = rewritten
                    .entry(Arc::as_ptr(&text.text))
                    .or_insert_with(|| {
                        let mut table = (*text.text).clone();
                        table.add_deciphered(read.iter().cloned());
                        Arc::new(table)
                    })
                    .clone();
                text.text = table;
            }
        }
    }
}

#[must_use]
pub fn reads_by_glyphs(graph: &PaintGraph, atoms: &[usize], provider: &dyn FontProvider) -> bool {
    fonts_of(graph).iter().any(|font| {
        font.iter().any(|at| atoms.contains(at)) && verdict(graph, font, provider).is_some()
    })
}

#[must_use]
pub fn replacement_family(
    graph: &PaintGraph,
    atoms: &[usize],
    provider: &dyn FontProvider,
) -> Option<String> {
    let request = atoms.iter().find_map(|at| match &graph.atoms[*at].kind {
        PaintAtomKind::Text(text) => text.font_request.clone(),
        _ => None,
    })?;
    let face = provider.fallback_face(&request, pdf_content::decipher::LAO_LETTER)?;
    Some(face.identity.family.clone())
}

#[must_use]
pub fn region_of(graph: &PaintGraph, atoms: &[usize]) -> Option<[f64; 4]> {
    let mut region: Option<[f64; 4]> = None;
    for at in atoms {
        let Some(PaintAtomKind::Text(text)) = graph.atoms.get(*at).map(|atom| &atom.kind) else {
            continue;
        };
        for glyph in &text.glyphs {
            let matrix = text.state.ctm.value.multiply(glyph.matrix);
            for (x, y) in [(0.0, -0.4), (1.0, -0.4), (0.0, 1.3), (1.0, 1.3)] {
                let corner = matrix.transform(crate::geometry::Point { x, y });
                let held = region.get_or_insert([corner.x, corner.y, corner.x, corner.y]);
                held[0] = held[0].min(corner.x);
                held[1] = held[1].min(corner.y);
                held[2] = held[2].max(corner.x);
                held[3] = held[3].max(corner.y);
            }
        }
    }
    region
}

fn first_origin(graph: &PaintGraph, at: usize) -> Option<(f64, f64)> {
    let PaintAtomKind::Text(text) = &graph.atoms.get(at)?.kind else {
        return None;
    };
    let glyph = text.glyphs.first()?;
    let origin = text
        .state
        .ctm
        .value
        .multiply(glyph.matrix)
        .transform(crate::geometry::Point { x: 0.0, y: 0.0 });
    Some((origin.x, origin.y))
}

fn verdict(
    graph: &PaintGraph,
    atoms: &[usize],
    provider: &dyn FontProvider,
) -> Option<Vec<(Code, String)>> {
    let model = pdf_content::decipher::lao_model()?;
    let shown: usize = atoms
        .iter()
        .map(|at| match &graph.atoms[*at].kind {
            PaintAtomKind::Text(text) => text.glyphs.len(),
            _ => 0,
        })
        .sum();
    #[expect(clippy::cast_precision_loss, reason = "a count of glyphs on a page")]
    if (shown as f64) < pdf_content::decipher::LEAST_EVIDENCE {
        return None;
    }
    let request = atoms.iter().find_map(|at| match &graph.atoms[*at].kind {
        PaintAtomKind::Text(text) => text.font_request.clone(),
        _ => None,
    })?;
    let shapes = shapes_for(provider, &request, model)?;
    let (cipher, drawn) = gather(graph, atoms);
    let reading = remembered_reading(&cipher, &shapes, model);
    if !reading.outweighs_the_file() {
        return None;
    }
    Some(
        cipher
            .codes
            .iter()
            .zip(&reading.characters)
            .zip(&drawn)
            .filter(|(_, drawn)| **drawn)
            .map(|((code, text), _)| (*code, text.clone()))
            .collect(),
    )
}

#[must_use]
pub fn font_evidence(graph: &PaintGraph) -> Vec<(Vec<usize>, Cipher)> {
    fonts_of(graph)
        .into_iter()
        .map(|atoms| {
            let (cipher, _) = gather(graph, &atoms);
            (atoms, cipher)
        })
        .collect()
}

#[must_use]
pub fn reading_lines(graph: &PaintGraph) -> Vec<String> {
    let mut lines = Vec::new();
    for atoms in fonts_of(graph) {
        let (cipher, _, glyphs) = gather_placed(graph, &atoms);
        for line in glyphs {
            lines.push(
                line.iter()
                    .filter_map(|(at, code)| match &graph.atoms[*at].kind {
                        PaintAtomKind::Text(text) => text
                            .text
                            .text_of(cipher.codes[*code])
                            .map(|meaning| meaning.text.clone()),
                        _ => None,
                    })
                    .collect(),
            );
        }
    }
    lines
}

fn fonts_of(graph: &PaintGraph) -> Vec<Vec<usize>> {
    let mut by_font: HashMap<*const pdf_content::GlyphProgram, Vec<usize>> = HashMap::new();
    for (index, atom) in graph.atoms.iter().enumerate() {
        if let PaintAtomKind::Text(text) = &atom.kind
            && let Some(program) = &text.program
            && !text.glyphs.is_empty()
        {
            by_font.entry(Arc::as_ptr(program)).or_default().push(index);
        }
    }
    let mut fonts: Vec<Vec<usize>> = by_font.into_values().collect();
    fonts.sort();
    fonts
}

#[must_use]
pub fn references(
    graph: &PaintGraph,
    atoms: &[usize],
    provider: &dyn FontProvider,
) -> Option<(Arc<Shapes>, &'static pdf_content::decipher::CharModel)> {
    let model = pdf_content::decipher::lao_model()?;
    let request = atoms.iter().find_map(|at| match &graph.atoms[*at].kind {
        PaintAtomKind::Text(text) => text.font_request.clone(),
        _ => None,
    })?;
    Some((shapes_for(provider, &request, model)?, model))
}

fn gather(graph: &PaintGraph, atoms: &[usize]) -> (Cipher, Vec<bool>) {
    let (cipher, drawn, _) = gather_placed(graph, atoms);
    (cipher, drawn)
}

type GlyphLine = Vec<(usize, usize)>;

fn gather_placed(graph: &PaintGraph, atoms: &[usize]) -> (Cipher, Vec<bool>, Vec<GlyphLine>) {
    let mut cipher = Cipher::default();
    let mut index_of: BTreeMap<Code, usize> = BTreeMap::new();
    let mut boxes: Vec<Option<[f64; 4]>> = Vec::new();
    let mut placed: Vec<(usize, usize, PlacedInk)> = Vec::new();
    let mut digests: HashMap<*const pdf_content::GlyphProgram, u128> = HashMap::new();
    for at in atoms {
        let PaintAtomKind::Text(text) = &graph.atoms[*at].kind else {
            continue;
        };
        let Some(program) = &text.program else {
            continue;
        };
        let digest = *digests
            .entry(Arc::as_ptr(program))
            .or_insert_with(|| program_digest(program));
        for glyph in &text.glyphs {
            let code = Code {
                value: glyph.code.value,
                byte_len: glyph.code.bytes.len(),
            };
            let index = *index_of.entry(code).or_insert_with(|| {
                let drawing = glyph.glyph.and_then(|id| drawing_of(program, digest, id));
                boxes.push(drawing.as_ref().map(|(ink, _)| *ink));
                cipher.codes.push(code);
                cipher.drawings.push(drawing.map(|(_, features)| features));
                cipher
                    .claims
                    .push(text.text.declared_text_of(code).and_then(|meaning| {
                        let mut characters = meaning.text.chars();
                        match (characters.next(), characters.next()) {
                            (Some(character), None) => Some(character),
                            _ => None,
                        }
                    }));
                cipher.codes.len() - 1
            });
            let matrix = text.state.ctm.value.multiply(glyph.matrix);
            let origin = matrix.transform(crate::geometry::Point { x: 0.0, y: 0.0 });
            placed.push((
                index,
                *at,
                PlacedInk {
                    origin: (origin.x, origin.y),
                    turn: matrix.b.atan2(matrix.a),
                    em: matrix.a.hypot(matrix.b),
                    ink: boxes[index],
                },
            ));
        }
    }
    let inks: Vec<PlacedInk> = placed.iter().map(|(_, _, ink)| *ink).collect();
    let order = reading_order(&inks);
    cipher.lines = order
        .iter()
        .map(|line| line.iter().map(|at| placed[*at].0).collect())
        .collect();
    let glyphs = order
        .iter()
        .map(|line| {
            line.iter()
                .map(|at| (placed[*at].1, placed[*at].0))
                .collect()
        })
        .collect();
    let drawn = cipher.drawings.iter().map(Option::is_some).collect();
    (cipher, drawn, glyphs)
}

const MOST_DRAWINGS: usize = 16_384;

const MOST_READINGS: usize = 8;

pub(crate) fn program_digest(program: &pdf_content::GlyphProgram) -> u128 {
    use std::hash::{Hash, Hasher};
    let half = |salt: u8| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        salt.hash(&mut hasher);
        program.program_bytes().hash(&mut hasher);
        hasher.finish()
    };
    (u128::from(half(1)) << 64) | u128::from(half(2))
}

pub(crate) fn drawing_of(
    program: &pdf_content::GlyphProgram,
    digest: u128,
    glyph: u16,
) -> Option<([f64; 4], Features)> {
    type Drawings = HashMap<(u128, u16), Option<([f64; 4], Features)>>;
    static KEPT: OnceLock<Mutex<Drawings>> = OnceLock::new();
    let kept = KEPT.get_or_init(Mutex::default);
    if let Some(found) = kept
        .lock()
        .ok()
        .and_then(|held| held.get(&(digest, glyph)).cloned())
    {
        return found;
    }
    let drawing =
        Raster::of(program, glyph).map(|raster| (raster.ink_box(), Features::of(&raster)));
    if let Ok(mut held) = kept.lock() {
        if held.len() >= MOST_DRAWINGS {
            held.clear();
        }
        held.insert((digest, glyph), drawing.clone());
    }
    drawing
}

fn remembered_reading(
    cipher: &Cipher,
    shapes: &Arc<Shapes>,
    model: &'static pdf_content::decipher::CharModel,
) -> pdf_content::decipher::Reading {
    type Readings = Vec<(Arc<Shapes>, Cipher, pdf_content::decipher::Reading)>;
    static KEPT: OnceLock<Mutex<Readings>> = OnceLock::new();
    let kept = KEPT.get_or_init(Mutex::default);
    if let Some(found) = kept.lock().ok().and_then(|held| {
        held.iter()
            .find(|(against, read, _)| Arc::ptr_eq(against, shapes) && read == cipher)
            .map(|(_, _, reading)| reading.clone())
    }) {
        return found;
    }
    let reading = decipher(cipher, shapes, model);
    if let Ok(mut held) = kept.lock() {
        if held.len() >= MOST_READINGS {
            held.remove(0);
        }
        held.push((Arc::clone(shapes), cipher.clone(), reading.clone()));
    }
    reading
}

fn shapes_for(
    provider: &dyn FontProvider,
    request: &FontRequest,
    model: &pdf_content::decipher::CharModel,
) -> Option<Arc<Shapes>> {
    static BUILT: OnceLock<Mutex<HashMap<Vec<String>, Arc<Shapes>>>> = OnceLock::new();
    let script = provider.fallback_face(request, pdf_content::decipher::LAO_LETTER)?;
    let mut named = request.clone();
    "DejaVu Sans".clone_into(&mut named.family);
    let mut faces = vec![script];
    faces.extend(provider.fallback_face(request, 'a'));
    faces.extend(provider.primary_face(&named));
    faces.dedup_by(|one, other| one.identity.sha256 == other.identity.sha256);
    let mut key: Vec<String> = faces
        .iter()
        .map(|face| face.identity.sha256.clone())
        .collect();
    key.sort();
    key.dedup();
    let built = BUILT.get_or_init(Mutex::default);
    if let Some(found) = built.lock().ok().and_then(|held| held.get(&key).cloned()) {
        return Some(found);
    }
    let characters = model.characters();
    let mut shapes = Shapes::default();
    let mut added: Vec<&str> = Vec::new();
    for face in &faces {
        if added.contains(&face.identity.sha256.as_str()) {
            continue;
        }
        added.push(&face.identity.sha256);
        shapes.add_face(&face.program, characters.iter().copied());
        shapes.add_stacks(
            &face.program,
            &pdf_content::decipher::LAO_VOWELS_ABOVE,
            &pdf_content::decipher::LAO_TONES,
        );
    }
    if shapes.is_empty() {
        return None;
    }
    let shapes = Arc::new(shapes);
    if let Ok(mut held) = built.lock() {
        held.insert(key, Arc::clone(&shapes));
    }
    Some(shapes)
}
