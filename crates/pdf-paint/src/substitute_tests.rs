use std::fmt::Write as _;
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_content::{
    ContentLimits, FontProvider, FontRequest, GlyphProgram, PageContentLimits, SubstitutedFace,
    SubstitutionReason, load_page_program_strict, parse_operations_strict,
};

use crate::InterpretError;
use crate::graph::{PaintAtomKind, PaintGraph, TextShowPaint};
use crate::interpreter::{PaintStream, interpret_stream_sequence_with_fonts};
use crate::state::PaintLimits;

fn triangle() -> Vec<u8> {
    triangle_of(100)
}

fn triangle_of(side: u8) -> Vec<u8> {
    let mut glyf = Vec::new();
    glyf.extend_from_slice(&1_i16.to_be_bytes());
    for value in [0_i16, 0, i16::from(side), i16::from(side)] {
        glyf.extend_from_slice(&value.to_be_bytes());
    }
    glyf.extend_from_slice(&2_u16.to_be_bytes());
    glyf.extend_from_slice(&0_u16.to_be_bytes());
    glyf.extend_from_slice(&[0x37, 0x37, 0x37]);
    glyf.extend_from_slice(&[0, side, 0]);
    glyf.extend_from_slice(&[0, 0, side]);
    if glyf.len() % 2 == 1 {
        glyf.push(0);
    }
    glyf
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

fn face_covering_with_notdef(characters: &[char], notdef: &[char]) -> Arc<GlyphProgram> {
    face_built(characters, notdef)
}

fn face_covering(characters: &[char]) -> Arc<GlyphProgram> {
    face_built(characters, &[])
}

fn face_built(characters: &[char], notdef: &[char]) -> Arc<GlyphProgram> {
    let outlines: Vec<Vec<u8>> = characters.iter().map(|_| triangle()).collect();
    let mut pairs: Vec<(u16, u16)> = characters
        .iter()
        .enumerate()
        .map(|(index, character)| {
            (
                u16::try_from(u32::from(*character)).expect("bmp character"),
                u16::try_from(index + 1).expect("glyph"),
            )
        })
        .collect();
    pairs.extend(
        notdef
            .iter()
            .map(|character| (u16::try_from(u32::from(*character)).expect("bmp"), 0_u16)),
    );
    sfnt_face(&outlines, 1, &pairs)
}

fn sfnt_face(outlines: &[Vec<u8>], encoding: u16, pairs: &[(u16, u16)]) -> Arc<GlyphProgram> {
    let glyph_count = u16::try_from(outlines.len() + 1).expect("glyph count");
    let mut glyf = Vec::new();
    let mut loca = vec![0_u16];
    loca.push(0);
    for outline in outlines {
        glyf.extend_from_slice(outline);
        loca.push(u16::try_from(glyf.len() / 2).expect("loca"));
    }
    let subtable = format4(pairs);
    let mut cmap = 0_u16.to_be_bytes().to_vec();
    cmap.extend_from_slice(&1_u16.to_be_bytes());
    cmap.extend_from_slice(&3_u16.to_be_bytes());
    cmap.extend_from_slice(&encoding.to_be_bytes());
    cmap.extend_from_slice(&12_u32.to_be_bytes());
    cmap.extend_from_slice(&subtable);

    let mut head = vec![0_u8; 54];
    head[18..20].copy_from_slice(&1000_u16.to_be_bytes());
    head[50..52].copy_from_slice(&0_u16.to_be_bytes());
    let mut maxp = vec![0_u8; 6];
    maxp[4..6].copy_from_slice(&glyph_count.to_be_bytes());
    let loca_bytes: Vec<u8> = loca.iter().flat_map(|value| value.to_be_bytes()).collect();
    let tables: Vec<(&[u8; 4], &[u8])> = vec![
        (b"cmap", &cmap),
        (b"glyf", &glyf),
        (b"head", &head),
        (b"loca", &loca_bytes),
        (b"maxp", &maxp),
    ];
    let mut out = Vec::new();
    out.extend_from_slice(&0x0001_0000_u32.to_be_bytes());
    out.extend_from_slice(&u16::try_from(tables.len()).expect("count").to_be_bytes());
    out.extend_from_slice(&[0; 6]);
    let mut offset = 12 + tables.len() * 16;
    let mut directory = Vec::new();
    let mut body = Vec::new();
    for (tag, data) in &tables {
        directory.extend_from_slice(*tag);
        directory.extend_from_slice(&[0; 4]);
        directory.extend_from_slice(&u32::try_from(offset).expect("offset").to_be_bytes());
        directory.extend_from_slice(&u32::try_from(data.len()).expect("length").to_be_bytes());
        body.extend_from_slice(data);
        offset += data.len();
    }
    out.extend_from_slice(&directory);
    out.extend_from_slice(&body);
    Arc::new(GlyphProgram::parse(out).expect("synthetic face parses"))
}

fn sfnt_bytes(outlines: &[Vec<u8>], encoding: u16, pairs: &[(u16, u16)]) -> Vec<u8> {
    sfnt_face(outlines, encoding, pairs)
        .program_bytes()
        .to_vec()
}

fn identity(name: &str) -> Arc<pdf_content::FaceIdentity> {
    Arc::new(pdf_content::FaceIdentity {
        family: name.to_owned(),
        subfamily: "Regular".to_owned(),
        origin: format!("test:{name}"),
        sha256: String::new(),
        face_index: 0,
        style: pdf_content::FontStyle::default(),
    })
}

#[derive(Debug)]
struct TestProvider {
    primary: Option<(String, Arc<GlyphProgram>)>,
    coverage: Option<(Vec<char>, String, Arc<GlyphProgram>)>,
    asked: std::sync::Mutex<Vec<String>>,
}

impl TestProvider {
    fn with_primary(characters: &[char]) -> Self {
        Self {
            primary: Some(("Test Primary".to_owned(), face_covering(characters))),
            coverage: None,
            asked: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn and_coverage(mut self, characters: &[char]) -> Self {
        self.coverage = Some((
            characters.to_vec(),
            "Test Coverage".to_owned(),
            face_covering(characters),
        ));
        self
    }

    fn empty() -> Self {
        Self {
            primary: None,
            coverage: None,
            asked: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn primary_requests(&self) -> usize {
        self.asked.lock().expect("lock").len()
    }
}

impl FontProvider for TestProvider {
    fn primary_face(&self, request: &FontRequest) -> Option<SubstitutedFace> {
        self.asked
            .lock()
            .expect("lock")
            .push(String::from_utf8_lossy(&request.base_font).into_owned());
        let (name, program) = self.primary.as_ref()?;
        Some(SubstitutedFace {
            program: Arc::clone(program),
            identity: identity(name),
            reason: SubstitutionReason::ExactFamily,
        })
    }

    fn fallback_face(&self, _request: &FontRequest, character: char) -> Option<SubstitutedFace> {
        let (characters, name, program) = self.coverage.as_ref()?;
        if !characters.contains(&character) {
            return None;
        }
        Some(SubstitutedFace {
            program: Arc::clone(program),
            identity: identity(name),
            reason: SubstitutionReason::ScriptCoverage,
        })
    }

    fn description(&self) -> String {
        "test provider".to_owned()
    }
}

fn page_with(content: &str, font_dictionary: &str, to_unicode: Option<&str>) -> ByteStore {
    page_with_flags(content, font_dictionary, to_unicode, 32)
}

fn page_with_flags(
    content: &str,
    font_dictionary: &str,
    to_unicode: Option<&str>,
    flags: u32,
) -> ByteStore {
    let content = content.as_bytes().to_vec();
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    let push = |bytes: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]| {
        offsets.push(bytes.len());
        bytes.extend_from_slice(body);
    };
    push(
        &mut bytes,
        &mut offsets,
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
    );
    push(
        &mut bytes,
        &mut offsets,
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    push(&mut bytes, &mut offsets, b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes(),
    );
    bytes.extend_from_slice(&content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(format!("5 0 obj\n{font_dictionary}\nendobj\n").as_bytes());
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        format!(
            "6 0 obj\n<< /Type /FontDescriptor /FontName /Nowhere /Flags {flags} /ItalicAngle 0 >>\nendobj\n"
        )
        .as_bytes(),
    );
    if let Some(to_unicode) = to_unicode {
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!("7 0 obj\n<< /Length {} >>\nstream\n", to_unicode.len()).as_bytes(),
        );
        bytes.extend_from_slice(to_unicode.as_bytes());
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
    }
    let size = offsets.len() + 1;
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    ByteStore::new(SourceId::new(77), Arc::<[u8]>::from(bytes))
}

fn interpret(
    source: &ByteStore,
    provider: Option<Arc<dyn FontProvider>>,
) -> Result<PaintGraph, InterpretError> {
    let page =
        load_page_program_strict(source, 0, PageContentLimits::default()).expect("fixture page");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("fixture operations");
    interpret_stream_sequence_with_fonts(
        &[PaintStream {
            source: &page.streams[0].bytes,
            reference: page.streams[0].reference,
            operations: &operations,
        }],
        page.page,
        &[],
        &page.resources,
        PaintLimits::default(),
        provider,
    )
}

fn runs(graph: &PaintGraph) -> Vec<&TextShowPaint> {
    graph
        .atoms
        .iter()
        .filter_map(|atom| match &atom.kind {
            PaintAtomKind::Text(text) => Some(text),
            _ => None,
        })
        .collect()
}

const WIN_ANSI_FONT: &str = "<< /Type /Font /Subtype /TrueType /BaseFont /Nowhere \
/Encoding /WinAnsiEncoding /FirstChar 65 /LastChar 67 /Widths [500 500 500] \
/FontDescriptor 6 0 R >>";

#[test]
fn a_substituted_run_draws_at_exactly_the_positions_the_document_declared() {
    let source = page_with("BT /F1 12 Tf 10 20 Td (ABC) Tj ET", WIN_ANSI_FONT, None);
    let without = interpret(&source, None).expect("interprets");
    let provider = Arc::new(TestProvider::with_primary(&['A', 'B', 'C']));
    let with = interpret(&source, Some(provider)).expect("interprets");

    let before = runs(&without);
    let after = runs(&with);
    assert_eq!(before.len(), 1);
    assert_eq!(after.len(), 1);
    assert!(
        before[0].glyphs.iter().all(|g| g.substituted.is_empty()),
        "nothing may be substituted without a provider"
    );
    assert!(
        after[0]
            .glyphs
            .iter()
            .all(|glyph| glyph.substituted.len() == 1),
        "every code should have drawn one outline"
    );
    for (before, after) in before[0].glyphs.iter().zip(&after[0].glyphs) {
        assert_eq!(
            before.matrix, after.matrix,
            "a substituted glyph moved: the face's advance leaked into placement"
        );
        assert_eq!(before.text_matrix, after.text_matrix);
        assert!((before.code.width - after.code.width).abs() < f64::EPSILON);
    }
    let substitution = after[0].substitution.as_ref().expect("a substitution");
    assert_eq!(substitution.primary.identity.family, "Test Primary");
    assert!(substitution.unmapped.is_empty());
    assert_eq!(substitution.unknown_codes, 0);
}

#[test]
fn tj_adjustments_and_a_scaled_matrix_survive_substitution() {
    let content = "BT /F1 12 Tf 2 0 0 3 10 20 Tm [(A) -500 (B) 250 (C)] TJ ET";
    let source = page_with(content, WIN_ANSI_FONT, None);
    let without = interpret(&source, None).expect("interprets");
    let provider = Arc::new(TestProvider::with_primary(&['A', 'B', 'C']));
    let with = interpret(&source, Some(provider)).expect("interprets");
    let before = runs(&without);
    let after = runs(&with);
    for (before, after) in before[0].glyphs.iter().zip(&after[0].glyphs) {
        assert_eq!(before.matrix, after.matrix);
    }
    assert_eq!(after[0].glyphs.len(), 3);
}

#[test]
fn a_differences_entry_decides_the_character_not_the_code() {
    let font = "<< /Type /Font /Subtype /TrueType /BaseFont /Nowhere \
/Encoding << /Type /Encoding /BaseEncoding /WinAnsiEncoding /Differences [65 /omega] >> \
/FirstChar 65 /LastChar 65 /Widths [500] /FontDescriptor 6 0 R >>";
    let source = page_with("BT /F1 12 Tf (A) Tj ET", font, None);
    let provider = Arc::new(TestProvider::with_primary(&['\u{03C9}']));
    let graph = interpret(&source, Some(provider)).expect("interprets");
    let after = runs(&graph);
    assert_eq!(
        after[0].glyphs[0].substituted.len(),
        1,
        "the /Differences name did not decide the glyph"
    );
}

#[test]
fn a_cluster_of_a_base_and_a_combining_mark_draws_both_at_one_origin() {
    let to_unicode = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n\
1 begincodespacerange <00> <FF> endcodespacerange\n\
1 beginbfchar <41> <0EC20EC9> endbfchar\n\
endcmap CMapName currentdict /CMap defineresource pop end end";
    let font = "<< /Type /Font /Subtype /TrueType /BaseFont /Nowhere \
/FirstChar 65 /LastChar 65 /Widths [500] /FontDescriptor 6 0 R /ToUnicode 7 0 R >>";
    let source = page_with("BT /F1 12 Tf (A) Tj ET", font, Some(to_unicode));
    let provider = Arc::new(TestProvider::with_primary(&['\u{0EC2}', '\u{0EC9}']));
    let graph = interpret(&source, Some(provider)).expect("interprets");
    let after = runs(&graph);
    let drawn = &after[0].glyphs[0].substituted;
    assert_eq!(drawn.len(), 2, "the tone mark was dropped");
    assert_ne!(drawn[0].glyph, drawn[1].glyph);
    let substitution = after[0].substitution.as_ref().expect("substitution");
    assert!(substitution.unmapped.is_empty());
}

#[track_caller]
fn assert_at(found: f64, wanted: f64) {
    assert!(
        (found - wanted).abs() < 1e-9,
        "expected {wanted}, found {found}"
    );
}

#[test]
fn review_a_marked_cluster_is_placed_by_the_document_and_reported_as_unshaped() {
    let to_unicode = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n\
1 begincodespacerange <00> <FF> endcodespacerange\n\
2 beginbfchar <41> <0EC20EC9> <42> <0041> endbfchar\n\
endcmap CMapName currentdict /CMap defineresource pop end end";
    let font = "<< /Type /Font /Subtype /TrueType /BaseFont /Nowhere \
/FirstChar 65 /LastChar 66 /Widths [500 500] /FontDescriptor 6 0 R /ToUnicode 7 0 R >>";
    let source = page_with("BT /F1 12 Tf 10 20 Td (AB) Tj ET", font, Some(to_unicode));
    let provider = Arc::new(TestProvider::with_primary(&['\u{0EC2}', '\u{0EC9}', 'A']));
    let graph = interpret(&source, Some(provider)).expect("interprets");
    let run = runs(&graph)[0];

    assert_eq!(run.glyphs.len(), 2, "two source codes, two glyph entries");
    assert_eq!(run.glyphs[0].substituted.len(), 2);
    assert_at(run.glyphs[0].matrix.e, 10.0);
    assert_at(run.glyphs[0].matrix.f, 20.0);
    assert_eq!(run.glyphs[1].substituted.len(), 1);
    assert_at(run.glyphs[1].matrix.e, 16.0);
    assert_at(run.glyphs[1].matrix.f, 20.0);

    assert!(
        run.has_unshaped_cluster(),
        "a base and its marks at one origin must be reported, not passed off"
    );

    let substitution = run.substitution.as_ref().expect("a substitution");
    assert_eq!(substitution.census.clusters_drawn, 2, "two clusters");
    assert_eq!(substitution.census.outlines_drawn, 3, "three outlines");
    assert_eq!(substitution.census.source_codes, 2);
    assert_eq!(
        substitution.census.routes,
        vec![(pdf_content::MappingRoute::ToUnicode, 2)]
    );
    assert!(substitution.census.unresolved.is_empty());
}

#[test]
fn review_a_cluster_of_several_base_characters_is_counted_not_spread_out() {
    let to_unicode = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n\
1 begincodespacerange <00> <FF> endcodespacerange\n\
1 beginbfchar <41> <00660069> endbfchar\n\
endcmap CMapName currentdict /CMap defineresource pop end end";
    let font = "<< /Type /Font /Subtype /TrueType /BaseFont /Nowhere \
/FirstChar 65 /LastChar 65 /Widths [500] /FontDescriptor 6 0 R /ToUnicode 7 0 R >>";
    let source = page_with("BT /F1 12 Tf (A) Tj ET", font, Some(to_unicode));
    let provider = Arc::new(TestProvider::with_primary(&['f', 'i']));
    let graph = interpret(&source, Some(provider)).expect("interprets");
    let run = runs(&graph)[0];
    assert!(
        run.glyphs[0].substituted.is_empty(),
        "the `fi` ligature must not be drawn as two letters in one cell"
    );
    let substitution = run.substitution.as_ref().expect("a substitution");
    assert_eq!(
        substitution.census.unresolved,
        vec![(pdf_content::UnresolvedReason::MultipleBaseCharacters, 1)]
    );
    assert_eq!(substitution.unmapped, vec![('f', 1)]);
}

#[test]
fn review_a_transformed_and_a_negative_size_run_place_clusters_from_the_document() {
    let to_unicode = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n\
1 begincodespacerange <00> <FF> endcodespacerange\n\
1 beginbfchar <41> <0EC20EC9> endbfchar\n\
endcmap CMapName currentdict /CMap defineresource pop end end";
    let font = "<< /Type /Font /Subtype /TrueType /BaseFont /Nowhere \
/FirstChar 65 /LastChar 65 /Widths [500] /FontDescriptor 6 0 R /ToUnicode 7 0 R >>";
    let faces = || Arc::new(TestProvider::with_primary(&['\u{0EC2}', '\u{0EC9}']));

    let scaled = page_with(
        "BT /F1 12 Tf 2 0 0 3 10 20 Tm (AA) Tj ET",
        font,
        Some(to_unicode),
    );
    let graph = interpret(&scaled, Some(faces())).expect("interprets");
    let run = runs(&graph)[0];
    assert_at(run.glyphs[0].matrix.e, 10.0);
    assert_at(run.glyphs[1].matrix.e, 22.0);
    assert_eq!(run.glyphs[0].substituted.len(), 2);
    assert_eq!(run.glyphs[1].substituted.len(), 2);

    let flipped = page_with("BT /F1 -12 Tf 60 20 Td (AA) Tj ET", font, Some(to_unicode));
    let graph = interpret(&flipped, Some(faces())).expect("interprets");
    let run = runs(&graph)[0];
    assert_at(run.glyphs[0].matrix.e, 60.0);
    assert_at(run.glyphs[1].matrix.e, 54.0);
    assert_eq!(run.glyphs[1].substituted.len(), 2);
}

#[test]
fn coverage_can_supply_a_face_when_no_primary_family_exists() {
    let font = "<< /Type /Font /Subtype /TrueType /BaseFont /Nowhere /Encoding /WinAnsiEncoding /FirstChar 65 /LastChar 65 /Widths [500] >>";
    let source = page_with("BT /F1 12 Tf (A) Tj ET", font, None);
    let provider = Arc::new(TestProvider::empty().and_coverage(&['A']));
    let graph = interpret(&source, Some(provider)).expect("interprets");
    let text = runs(&graph)[0];
    assert_eq!(text.glyphs[0].substituted.len(), 1);
    assert_eq!(
        text.substitution.as_ref().unwrap().primary.reason,
        SubstitutionReason::ScriptCoverage
    );
}

#[test]
fn a_character_the_primary_face_lacks_comes_from_a_covering_face() {
    let to_unicode = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n\
1 begincodespacerange <00> <FF> endcodespacerange\n\
1 beginbfchar <42> <0E01> endbfchar\n\
endcmap CMapName currentdict /CMap defineresource pop end end";
    let font = "<< /Type /Font /Subtype /TrueType /BaseFont /Nowhere \
/FirstChar 65 /LastChar 66 /Widths [500 500] /FontDescriptor 6 0 R /ToUnicode 7 0 R >>";
    let source = page_with("BT /F1 12 Tf (B) Tj ET", font, Some(to_unicode));
    let provider = Arc::new(TestProvider::with_primary(&['A']).and_coverage(&['\u{0E01}']));
    let graph = interpret(&source, Some(provider)).expect("interprets");
    let after = runs(&graph);
    let drawn = &after[0].glyphs[0].substituted;
    assert_eq!(drawn.len(), 1);
    assert!(
        !after[0].has_unshaped_cluster(),
        "one outline per code is not a cluster and must not be reported as one"
    );
    assert_eq!(drawn[0].face, 1, "the fallback face was not recorded");
    let substitution = after[0].substitution.as_ref().expect("substitution");
    assert_eq!(substitution.fallbacks.len(), 1);
    assert_eq!(substitution.fallbacks[0].identity.family, "Test Coverage");
    assert_eq!(
        substitution.fallbacks[0].reason,
        SubstitutionReason::ScriptCoverage
    );
}

#[test]
fn a_character_no_face_carries_is_reported_and_not_drawn() {
    let to_unicode = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n\
1 begincodespacerange <00> <FF> endcodespacerange\n\
1 beginbfchar <42> <0E01> endbfchar\n\
endcmap CMapName currentdict /CMap defineresource pop end end";
    let font = "<< /Type /Font /Subtype /TrueType /BaseFont /Nowhere \
/FirstChar 65 /LastChar 66 /Widths [500 500] /FontDescriptor 6 0 R /ToUnicode 7 0 R >>";
    let source = page_with("BT /F1 12 Tf (B) Tj ET", font, Some(to_unicode));
    let provider = Arc::new(TestProvider::with_primary(&['A']));
    let graph = interpret(&source, Some(provider)).expect("interprets");
    let after = runs(&graph);
    assert!(after[0].glyphs[0].substituted.is_empty());
    let substitution = after[0].substitution.as_ref().expect("substitution");
    assert_eq!(substitution.unmapped, vec![('\u{0E01}', 1)]);
}

#[test]
fn a_symbolic_font_with_no_evidence_draws_nothing_and_says_so() {
    let font = "<< /Type /Font /Subtype /TrueType /BaseFont /Nowhere \
/Encoding << /Type /Encoding /Differences [65 /g3 /g4] >> \
/FirstChar 65 /LastChar 66 /Widths [500 500] /FontDescriptor 8 0 R >>";
    let font = font.replace("8 0 R", "6 0 R");
    let source = page_with_flags("BT /F1 12 Tf (AB) Tj ET", &font, None, 4);
    let provider = Arc::new(TestProvider::with_primary(&['A', 'B']));
    let graph = interpret(&source, Some(provider)).expect("interprets");
    let after = runs(&graph);
    assert!(
        after[0]
            .glyphs
            .iter()
            .all(|glyph| glyph.substituted.is_empty()),
        "a symbolic font's invented names were read as characters"
    );
    let substitution = after[0].substitution.as_ref().expect("substitution");
    assert_eq!(substitution.unknown_codes, 2);
}

#[test]
fn one_font_resource_is_searched_once_however_many_runs_use_it() {
    let content = "BT /F1 12 Tf (A) Tj (B) Tj (C) Tj (A) Tj ET";
    let source = page_with(content, WIN_ANSI_FONT, None);
    let provider = Arc::new(TestProvider::with_primary(&['A', 'B', 'C']));
    let graph = interpret(
        &source,
        Some(Arc::clone(&provider) as Arc<dyn FontProvider>),
    )
    .expect("interprets");
    assert_eq!(runs(&graph).len(), 4);
    assert_eq!(
        provider.primary_requests(),
        1,
        "the provider was asked once per run instead of once per resource"
    );
}

#[test]
fn a_provider_with_no_face_leaves_the_run_undrawn_and_names_the_font() {
    let source = page_with("BT /F1 12 Tf (ABC) Tj ET", WIN_ANSI_FONT, None);
    let provider = Arc::new(TestProvider::empty());
    let graph = interpret(&source, Some(provider)).expect("interprets");
    let after = runs(&graph);
    assert!(after[0].substitution.is_none());
    let request = after[0].font_request.as_ref().expect("a request");
    assert_eq!(request.base_font, b"/Nowhere");
    assert_eq!(request.family, "Nowhere");
}

#[test]
fn a_substituted_face_supplies_the_outlines_a_text_clip_needs() {
    let content = "BT /F1 12 Tf 7 Tr 10 20 Td (ABC) Tj ET 0 0 200 100 re f";
    let source = page_with(content, WIN_ANSI_FONT, None);
    let refused = interpret(&source, None);
    assert!(
        refused.is_err(),
        "without a face the clip has no outlines and must refuse"
    );
    let provider = Arc::new(TestProvider::with_primary(&['A', 'B', 'C']));
    let graph = interpret(&source, Some(provider)).expect("the clip now has outlines");
    assert!(
        graph
            .atoms
            .iter()
            .any(|atom| matches!(atom.kind, PaintAtomKind::Path(_))),
        "the rectangle drawn through the text clip is missing"
    );
}

#[test]
fn a_face_carrying_none_of_the_characters_draws_nothing() {
    let source = page_with("BT /F1 12 Tf (ABC) Tj ET", WIN_ANSI_FONT, None);
    let provider = Arc::new(TestProvider::with_primary(&['\u{0E01}']));
    let graph = interpret(&source, Some(provider)).expect("interprets");
    let after = runs(&graph);
    assert!(
        after[0]
            .glyphs
            .iter()
            .all(|glyph| glyph.substituted.is_empty())
    );
    let substitution = after[0].substitution.as_ref().expect("substitution");
    assert_eq!(
        substitution.unmapped,
        vec![('A', 1), ('B', 1), ('C', 1)],
        "the characters that could not be drawn were not reported"
    );
}

#[test]
fn a_type3_font_is_never_substituted() {
    let font = "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 100 100] \
/FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << >> \
/Encoding << /Type /Encoding /Differences [65 /square] >> \
/FirstChar 65 /LastChar 65 /Widths [500] >>";
    let source = page_with("BT /F1 12 Tf (A) Tj ET", font, None);
    let provider = Arc::new(TestProvider::with_primary(&['A']));
    let graph = interpret(
        &source,
        Some(Arc::clone(&provider) as Arc<dyn FontProvider>),
    )
    .expect("interprets");
    let after = runs(&graph);
    assert!(after[0].type3);
    assert!(after[0].substitution.is_none());
    assert!(after[0].font_request.is_none());
    assert_eq!(provider.primary_requests(), 0);
}

#[test]
fn a_character_mapped_to_notdef_is_not_a_glyph() {
    let source = page_with("BT /F1 12 Tf (ABC) Tj ET", WIN_ANSI_FONT, None);
    let provider = Arc::new(TestProvider {
        primary: Some((
            "Test Primary".to_owned(),
            face_covering_with_notdef(&['A'], &['B', 'C']),
        )),
        coverage: None,
        asked: std::sync::Mutex::new(Vec::new()),
    });
    let graph = interpret(&source, Some(provider)).expect("interprets");
    let after = runs(&graph);
    assert_eq!(after[0].glyphs[0].substituted.len(), 1, "A is carried");
    assert!(
        after[0].glyphs[1].substituted.is_empty(),
        "a character mapped to .notdef was drawn as glyph 0"
    );
    assert!(after[0].glyphs[2].substituted.is_empty());
    let substitution = after[0].substitution.as_ref().expect("substitution");
    assert_eq!(substitution.unmapped, vec![('B', 1), ('C', 1)]);
}

fn unicode_fixture(destination: &str) -> ByteStore {
    let cmap = format!(
        "begincmap 1 begincodespacerange <00> <FF> endcodespacerange \
         1 beginbfchar <41> <{destination}> endbfchar endcmap"
    );
    page_with_flags(
        "BT /F1 12 Tf (A) Tj ET",
        "<< /Type /Font /Subtype /TrueType /BaseFont /Nowhere \
         /FirstChar 65 /LastChar 65 /Widths [500] /FontDescriptor 6 0 R /ToUnicode 7 0 R >>",
        Some(&cmap),
        4,
    )
}

#[test]
fn review_private_use_is_not_a_license_to_draw_latin() {
    let source = unicode_fixture("F041");
    let provider = Arc::new(TestProvider::with_primary(&['A']));
    let graph = interpret(&source, Some(provider)).expect("interprets");
    assert!(runs(&graph)[0].glyphs[0].substituted.is_empty());
    assert_eq!(
        runs(&graph)[0].substitution.as_ref().unwrap().unmapped,
        vec![('\u{f041}', 1)]
    );
}

fn every_byte_font(base_font: &str) -> String {
    let widths: Vec<&str> = std::iter::repeat_n("500", 256).collect();
    format!(
        "<< /Type /Font /Subtype /TrueType /BaseFont {base_font} \
         /Encoding /WinAnsiEncoding /FirstChar 0 /LastChar 255 /Widths [{}] \
         /FontDescriptor 6 0 R >>",
        widths.join(" ")
    )
}

#[test]
fn review_legacy_cjk_winansi_conflict_recovers_the_characters_its_name_declares() {
    let source = page_with_flags(
        "BT /F1 12 Tf <CBCECCE5> Tj ET",
        &every_byte_font("/#CB#CE#CC#E5"),
        None,
        4,
    );
    let provider = Arc::new(TestProvider::with_primary(&['宋', '体']));
    let graph = interpret(&source, Some(provider)).expect("interprets");
    let run = runs(&graph)[0];

    assert_eq!(
        run.glyphs.len(),
        4,
        "one code per byte, as the file wrote it"
    );
    assert_eq!(run.glyphs[0].substituted.len(), 1, "宋 drawn by its lead");
    assert!(run.glyphs[1].substituted.is_empty(), "its trailing byte");
    assert_eq!(run.glyphs[2].substituted.len(), 1, "体 drawn by its lead");
    assert!(run.glyphs[3].substituted.is_empty());

    let substitution = run.substitution.as_ref().expect("a substitution");
    assert_eq!(
        substitution.census.routes,
        vec![(pdf_content::MappingRoute::RecoveredLegacyEncoding, 2)]
    );
    assert_eq!(
        substitution.census.unresolved,
        vec![(pdf_content::UnresolvedReason::MultibyteContinuation, 2)]
    );
    assert_eq!(substitution.census.source_codes, 4);
    assert_eq!(substitution.census.clusters_drawn, 2);
    assert_eq!(substitution.census.outlines_drawn, 2);
    assert_eq!(substitution.unknown_codes, 0);
    assert!(substitution.unmapped.is_empty());

    for (glyph, wanted) in run.glyphs.iter().zip([0.0, 6.0, 12.0, 18.0]) {
        assert_at(glyph.matrix.e, wanted);
    }
}

#[test]
fn review_a_font_name_outside_the_recovered_families_is_not_decoded_as_gbk() {
    let source = page_with_flags(
        "BT /F1 12 Tf <CBCE> Tj ET",
        &every_byte_font("/Nowhere"),
        None,
        4,
    );
    let provider = Arc::new(TestProvider::with_primary(&['宋', '体']));
    let graph = interpret(&source, Some(provider)).expect("interprets");
    let run = runs(&graph)[0];
    assert!(
        run.glyphs.iter().all(|glyph| glyph.substituted.is_empty()),
        "no glyph may be drawn: this font's codes still mean what WinAnsi says"
    );
    let substitution = run.substitution.as_ref().expect("a substitution");
    assert!(
        substitution.census.routes.is_empty()
            || substitution
                .census
                .routes
                .iter()
                .all(|(route, _)| *route != pdf_content::MappingRoute::RecoveredLegacyEncoding),
        "the recovery must not run for a name outside the list"
    );
    assert_eq!(substitution.unmapped, vec![('Ë', 1), ('Î', 1)]);
}

#[test]
fn review_a_truncated_legacy_multibyte_code_is_refused_not_completed() {
    let source = page_with_flags(
        "BT /F1 12 Tf <41CB> Tj ET",
        &every_byte_font("/#CB#CE#CC#E5"),
        None,
        4,
    );
    let provider = Arc::new(TestProvider::with_primary(&['A', '宋', '体']));
    let graph = interpret(&source, Some(provider)).expect("interprets");
    let run = runs(&graph)[0];
    assert_eq!(
        run.glyphs[0].substituted.len(),
        1,
        "the ASCII byte is a code"
    );
    assert!(run.glyphs[1].substituted.is_empty());
    let substitution = run.substitution.as_ref().expect("a substitution");
    assert_eq!(
        substitution.census.unresolved,
        vec![(pdf_content::UnresolvedReason::TruncatedMultibyte, 1)]
    );
    assert_eq!(substitution.unknown_codes, 1);
}

#[test]
fn review_a_legacy_code_is_not_assembled_across_two_show_operands() {
    let source = page_with_flags(
        "BT /F1 12 Tf [<CB> 0 <CE>] TJ ET",
        &every_byte_font("/#CB#CE#CC#E5"),
        None,
        4,
    );
    let provider = Arc::new(TestProvider::with_primary(&['宋', '体']));
    let graph = interpret(&source, Some(provider)).expect("interprets");
    let run = runs(&graph)[0];
    assert!(
        run.glyphs.iter().all(|glyph| glyph.substituted.is_empty()),
        "neither half is a code on its own"
    );
    let substitution = run.substitution.as_ref().expect("a substitution");
    assert_eq!(
        substitution.census.unresolved,
        vec![(pdf_content::UnresolvedReason::TruncatedMultibyte, 2)]
    );
}

#[test]
fn review_a_recovered_character_no_face_covers_is_reported_as_the_character() {
    let source = page_with_flags(
        "BT /F1 12 Tf <CBCE> Tj ET",
        &every_byte_font("/#CB#CE#CC#E5"),
        None,
        4,
    );
    let provider = Arc::new(TestProvider::with_primary(&['A']));
    let graph = interpret(&source, Some(provider)).expect("interprets");
    let run = runs(&graph)[0];
    assert!(run.glyphs.iter().all(|glyph| glyph.substituted.is_empty()));
    let substitution = run.substitution.as_ref().expect("a substitution");
    assert_eq!(substitution.unmapped, vec![('宋', 1)]);
    assert_eq!(
        substitution.census.routes,
        vec![(pdf_content::MappingRoute::RecoveredLegacyEncoding, 1)]
    );
    assert_eq!(
        substitution.census.unresolved,
        vec![
            (pdf_content::UnresolvedReason::MultibyteContinuation, 1),
            (pdf_content::UnresolvedReason::NoFaceCoverage, 1),
        ]
    );
    assert_eq!(substitution.unknown_codes, 0);
}

#[test]
fn review_legacy_recovery_draws_nothing_without_a_provider() {
    let source = page_with_flags(
        "BT /F1 12 Tf <CBCE> Tj ET",
        &every_byte_font("/#CB#CE#CC#E5"),
        None,
        4,
    );
    let graph = interpret(&source, None).expect("interprets");
    let run = runs(&graph)[0];
    assert!(run.glyphs.iter().all(|glyph| glyph.substituted.is_empty()));
    assert!(run.substitution.is_none());
}

fn standard_face_font(base_font: &str, extra: &str) -> String {
    let widths: Vec<&str> = std::iter::repeat_n("500", 256).collect();
    format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont {base_font} \
         /FirstChar 0 /LastChar 255 /Widths [{}] /FontDescriptor 6 0 R{extra} >>",
        widths.join(" ")
    )
}

#[test]
fn review_a_symbol_face_reads_its_own_encoding_ahead_of_a_private_use_to_unicode() {
    let cmap = "begincmap 1 begincodespacerange <00> <FF> endcodespacerange \
                1 beginbfchar <B7> <F0B7> endbfchar endcmap";
    let source = page_with_flags(
        "BT /F1 12 Tf <B7> Tj ET",
        &standard_face_font("/Symbol", " /ToUnicode 7 0 R"),
        Some(cmap),
        4,
    );
    let provider = Arc::new(TestProvider::with_primary(&['\u{2022}']));
    let graph = interpret(&source, Some(provider)).expect("interprets");
    let run = runs(&graph)[0];
    assert_eq!(run.glyphs.len(), 1);
    assert_eq!(run.glyphs[0].substituted.len(), 1, "the bullet is drawn");
    let substitution = run.substitution.as_ref().expect("a substitution");
    assert_eq!(
        substitution.census.routes,
        vec![(pdf_content::MappingRoute::StandardFaceEncoding, 1)]
    );
    assert!(substitution.unmapped.is_empty());
}

#[test]
fn review_a_dingbat_check_mark_draws_whether_or_not_the_font_states_widths() {
    for font in [
        "<< /Type /Font /Subtype /Type1 /BaseFont /ZapfDingbats >>".to_owned(),
        standard_face_font("/ZapfDingbats", ""),
    ] {
        let source = page_with_flags("BT /F1 12 Tf (4) Tj ET", &font, None, 4);
        let provider = Arc::new(TestProvider::with_primary(&['\u{2714}']));
        let graph = interpret(&source, Some(provider)).expect("interprets");
        let run = runs(&graph)[0];
        assert_eq!(run.glyphs.len(), 1);
        assert_eq!(
            run.glyphs[0].substituted.len(),
            1,
            "the tick is drawn: {font}"
        );
        let substitution = run.substitution.as_ref().expect("a substitution");
        assert!(substitution.unmapped.is_empty(), "{font}");
    }
}

#[test]
fn review_a_symbol_faces_differences_still_outrank_its_built_in_encoding() {
    let source = page_with_flags(
        "BT /F1 12 Tf <B7> Tj ET",
        &standard_face_font("/Symbol", " /Encoding << /Differences [183 /alpha] >>"),
        None,
        4,
    );
    let provider = Arc::new(TestProvider::with_primary(&['\u{2022}', '\u{3B1}']));
    let graph = interpret(&source, Some(provider)).expect("interprets");
    let substitution = runs(&graph)[0]
        .substitution
        .as_ref()
        .expect("a substitution");
    assert_eq!(
        substitution.census.routes,
        vec![(pdf_content::MappingRoute::EncodingGlyphName, 1)],
        "`/Differences` named this code, so the face's own table must not answer"
    );
}

#[test]
fn review_a_non_symbolic_standard_face_gets_no_built_in_table() {
    let cmap = "begincmap 1 begincodespacerange <00> <FF> endcodespacerange \
                1 beginbfchar <B7> <F0B7> endbfchar endcmap";
    let source = page_with_flags(
        "BT /F1 12 Tf <B7> Tj ET",
        &standard_face_font("/Helvetica", " /ToUnicode 7 0 R"),
        Some(cmap),
        4,
    );
    let provider = Arc::new(TestProvider::with_primary(&['\u{2022}']));
    let graph = interpret(&source, Some(provider)).expect("interprets");
    let run = runs(&graph)[0];
    assert!(run.glyphs[0].substituted.is_empty());
    let substitution = run.substitution.as_ref().expect("a substitution");
    assert_eq!(substitution.unmapped, vec![('\u{f0b7}', 1)]);
}

#[test]
fn review_explicit_unknown_name_does_not_fall_back_to_standard_encoding() {
    let font = "<< /Type /Font /Subtype /TrueType /BaseFont /Nowhere \
        /Encoding << /Differences [65 /producerSpecificGlyph] >> \
        /FirstChar 65 /LastChar 65 /Widths [500] /FontDescriptor 6 0 R >>";
    let source = page_with("BT /F1 12 Tf (A) Tj ET", font, None);
    let provider = Arc::new(TestProvider::with_primary(&['A']));
    let graph = interpret(&source, Some(provider)).expect("interprets");
    assert!(runs(&graph)[0].glyphs[0].substituted.is_empty());
    assert_eq!(
        runs(&graph)[0].substitution.as_ref().unwrap().unknown_codes,
        1
    );
}

#[test]
fn review_overlong_cluster_is_reported_instead_of_truncated() {
    let source = unicode_fixture("00410301030203030304");
    let provider = Arc::new(TestProvider::with_primary(&[
        'A', '\u{301}', '\u{302}', '\u{303}', '\u{304}',
    ]));
    let graph = interpret(&source, Some(provider)).expect("interprets");
    let run = runs(&graph)[0];
    assert!(
        run.glyphs[0].substituted.is_empty(),
        "four outlines are not a complete five-character cluster"
    );
    assert_eq!(run.substitution.as_ref().unwrap().unknown_codes, 1);
}

#[test]
fn review_missing_mark_does_not_make_a_partial_cluster_look_complete() {
    let source = unicode_fixture("00410301");
    let provider = Arc::new(TestProvider::with_primary(&['A']));
    let graph = interpret(&source, Some(provider)).expect("interprets");
    let run = runs(&graph)[0];
    assert!(
        run.glyphs[0].substituted.is_empty(),
        "a missing accent must not pass the outline-completeness check"
    );
    assert_eq!(
        run.substitution.as_ref().unwrap().unmapped,
        vec![('\u{301}', 1)]
    );
}

#[test]
fn shaped_design_offsets_reach_canonical_ink_bounds_and_signatures() {
    let font = "<< /Type /Font /Subtype /TrueType /BaseFont /Nowhere /Encoding /WinAnsiEncoding /FirstChar 65 /LastChar 65 /Widths [500] >>";
    let source = page_with("BT /F1 12 Tf 10 20 Td (A) Tj ET", font, None);
    let provider = Arc::new(TestProvider::with_primary(&['A']));
    let mut graph = interpret(&source, Some(provider)).expect("graph");
    let before = crate::glyph_placement_signature(&graph);
    let PaintAtomKind::Text(text) = &mut graph.atoms[0].kind else {
        panic!("text")
    };
    text.glyphs[0].substituted[0].offset = [100, 200];
    text.glyphs[0].substituted[0].shaped = true;
    let bounds = text.outline_bounds().expect("ink");
    for (actual, expected) in bounds.into_iter().zip([11.2, 22.4, 12.4, 23.6]) {
        assert_at(actual, expected);
    }
    assert_ne!(before, crate::glyph_placement_signature(&graph));
}

#[derive(Debug)]
struct ReferenceProvider {
    family: String,
    program: Arc<GlyphProgram>,
}

impl FontProvider for ReferenceProvider {
    fn primary_face(&self, _request: &FontRequest) -> Option<SubstitutedFace> {
        Some(SubstitutedFace {
            program: Arc::clone(&self.program),
            identity: identity(&self.family),
            reason: SubstitutionReason::ExactFamily,
        })
    }

    fn fallback_face(&self, _request: &FontRequest, _character: char) -> Option<SubstitutedFace> {
        None
    }

    fn description(&self) -> String {
        format!("reference {}", self.family)
    }
}

fn unread_subset_page(content: &[u8]) -> ByteStore {
    let program = sfnt_bytes(
        &[triangle_of(200), triangle_of(100)],
        0,
        &[(0xF041, 1), (0xF042, 2)],
    );
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    let mut object = |bytes: &mut Vec<u8>, body: &[u8]| {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
        bytes.extend_from_slice(body);
        bytes.extend_from_slice(b"\nendobj\n");
    };
    object(&mut bytes, b"<< /Type /Catalog /Pages 2 0 R >>");
    object(
        &mut bytes,
        b"<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>",
    );
    object(
        &mut bytes,
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
    );
    let mut stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
    stream.extend_from_slice(content);
    stream.extend_from_slice(b"\nendstream");
    object(&mut bytes, &stream);
    object(&mut bytes, b"<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+LaoTest /FirstChar 65 /LastChar 66 /Widths [500 500] /FontDescriptor 6 0 R >>");
    object(&mut bytes, b"<< /Type /FontDescriptor /FontName /ABCDEF+LaoTest /Flags 4 /ItalicAngle 0 /FontFile2 7 0 R >>");
    let mut stream = format!("<< /Length {} >>\nstream\n", program.len()).into_bytes();
    stream.extend_from_slice(&program);
    stream.extend_from_slice(b"\nendstream");
    object(&mut bytes, &stream);
    let size = offsets.len() + 1;
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in &offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    ByteStore::new(SourceId::new(78), Arc::<[u8]>::from(bytes))
}

fn lao_reference(family: &str) -> Arc<ReferenceProvider> {
    Arc::new(ReferenceProvider {
        family: family.to_owned(),
        program: sfnt_face(
            &[triangle_of(100), triangle_of(200)],
            1,
            &[(0x0E99, 1), (0x0E81, 2)],
        ),
    })
}

fn said(graph: &PaintGraph) -> Vec<Option<String>> {
    let run = runs(graph)[0];
    run.glyphs
        .iter()
        .map(|glyph| {
            run.text
                .text_of(pdf_content::Code {
                    value: glyph.code.value,
                    byte_len: glyph.code.bytes.len(),
                })
                .map(|meaning| meaning.text.clone())
        })
        .collect()
}

#[test]
fn an_unread_glyph_reads_as_the_reference_glyph_with_its_outline() {
    let source = unread_subset_page(b"BT /F1 12 Tf 10 50 Td (AB) Tj ET");
    let graph = interpret(&source, Some(lao_reference("LaoTest"))).expect("interprets");
    assert_eq!(said(&graph), [Some("ກ".to_owned()), Some("ນ".to_owned())]);
    let run = runs(&graph)[0];
    assert_eq!(
        run.text.by_confidence(pdf_content::Confidence::Matched),
        2,
        "and says the reading came from outlines"
    );

    let graph = interpret(&source, None).expect("interprets");
    assert_eq!(said(&graph), [None, None], "control: no reference");
    let graph = interpret(&source, Some(lao_reference("Other Family"))).expect("interprets");
    assert_eq!(
        said(&graph),
        [None, None],
        "control: another family's outlines"
    );
}

#[test]
fn every_run_of_a_font_holds_what_later_runs_matched() {
    let source = unread_subset_page(b"BT /F1 12 Tf 10 50 Td (A) Tj (B) Tj ET");
    let graph = interpret(&source, Some(lao_reference("LaoTest"))).expect("interprets");
    let first = runs(&graph)[0];
    assert_eq!(
        first
            .text
            .codes_for("ນ")
            .iter()
            .map(|code| code.value)
            .collect::<Vec<_>>(),
        [0x42],
        "the first run shows only A, and still knows what B is"
    );
}

#[test]
fn a_winansi_runs_second_space_is_a_non_breaking_space_and_a_space_has_one_code() {
    let font = "<< /Type /Font /Subtype /TrueType /BaseFont /Nowhere \
/Encoding /WinAnsiEncoding /FirstChar 32 /LastChar 32 /Widths [250] \
/FontDescriptor 6 0 R >>";
    let source = page_with("BT /F1 12 Tf ( ) Tj ET", font, None);
    let graph = interpret(&source, None).expect("interprets");
    let text = &runs(&graph)[0].text;
    let one = |value| pdf_content::Code { value, byte_len: 1 };
    assert_eq!(
        text.text_of(one(0xA0)).map(|meaning| meaning.text.as_str()),
        Some("\u{a0}")
    );
    assert_eq!(
        text.text_of(one(0xAD)).map(|meaning| meaning.text.as_str()),
        Some("\u{ad}")
    );
    assert_eq!(text.codes_to_write(" "), vec![one(0x20)]);
    assert_eq!(text.codes_to_write("-"), vec![one(0x2D)]);
}

fn polygon(points: &[(i16, i16)]) -> Vec<u8> {
    polygons(&[points])
}

fn polygons(contours: &[&[(i16, i16)]]) -> Vec<u8> {
    let points: Vec<(i16, i16)> = contours
        .iter()
        .flat_map(|contour| contour.iter().copied())
        .collect();
    let xs = points.iter().map(|point| point.0);
    let ys = points.iter().map(|point| point.1);
    let mut glyf = Vec::new();
    glyf.extend_from_slice(
        &i16::try_from(contours.len())
            .expect("contours")
            .to_be_bytes(),
    );
    for value in [
        xs.clone().min().unwrap_or(0),
        ys.clone().min().unwrap_or(0),
        xs.max().unwrap_or(0),
        ys.max().unwrap_or(0),
    ] {
        glyf.extend_from_slice(&value.to_be_bytes());
    }
    let mut end = 0_usize;
    for contour in contours {
        end += contour.len();
        glyf.extend_from_slice(&u16::try_from(end - 1).expect("points").to_be_bytes());
    }
    glyf.extend_from_slice(&0_u16.to_be_bytes());
    glyf.extend(std::iter::repeat_n(0x01_u8, points.len()));
    let mut previous = (0_i16, 0_i16);
    for point in &points {
        glyf.extend_from_slice(&(point.0 - previous.0).to_be_bytes());
        previous.0 = point.0;
    }
    for point in &points {
        glyf.extend_from_slice(&(point.1 - previous.1).to_be_bytes());
        previous.1 = point.1;
    }
    if glyf.len() % 2 == 1 {
        glyf.push(0);
    }
    glyf
}

fn distinct_shape(index: i16, family: i16) -> Vec<u8> {
    let key = index * 7 + family * 31;
    let width = 150 + (key * 37) % 350;
    let height = 250 + (key * 53) % 500;
    let lean = (key * 71) % 200 - 100;
    let notch = (key * 97) % 150;
    polygon(&[
        (40, 0),
        (40 + width, notch / 3),
        (40 + width + lean, height),
        (40 + width / 2, height - notch),
        (40 + lean / 2, height + notch / 2),
    ])
}

fn latin_shape(index: i16) -> Vec<u8> {
    let foot = 60 + index * 25;
    polygon(&[
        (foot, 0),
        (foot + 70, 0),
        (foot + 70 + index * 15, 720),
        (foot + index * 15, 720),
    ])
}

const LAO_LETTERS: [char; 17] = [
    'ກ', 'າ', 'ນ', 'ທ', 'ຳ', 'ຄ', 'ວ', 'ມ', 'ສ', 'ະ', 'ອ', 'ດ', 'ແ', '່', 'ຊ', 'ັ', '້',
];

const LAO_LINE: &str = "ການທຳຄວາມສະອາດແມ່ນຊັ້ນ";

const LEGACY_GLYPHS: usize = 16;

fn legacy_text(k: usize) -> String {
    if k == 15 {
        "ັ້".to_owned()
    } else {
        LAO_LETTERS[k].to_string()
    }
}

const VOWEL: [(i16, i16); 4] = [(60, 600), (380, 600), (380, 690), (60, 690)];
const TONE: [(i16, i16); 4] = [(200, 700), (260, 700), (260, 880), (200, 880)];

fn lao_shape(index: i16) -> Vec<u8> {
    match index {
        13 => tone_mark(),
        15 => polygon(&VOWEL),
        16 => polygon(&TONE),
        _ => distinct_shape(index, 0),
    }
}

fn tone_mark() -> Vec<u8> {
    polygon(&[(-450, 820), (-310, 820), (-330, 960), (-430, 940)])
}

fn legacy_shape(index: i16) -> Vec<u8> {
    let back = |points: &[(i16, i16)], rise: i16| -> Vec<(i16, i16)> {
        points.iter().map(|(x, y)| (x - 490, y + rise)).collect()
    };
    if index == 15 {
        let vowel = back(&VOWEL, 0);
        let tone: Vec<(i16, i16)> = back(&TONE, 53)
            .into_iter()
            .map(|(x, y)| (x + 120, y))
            .collect();
        polygons(&[&vowel, &tone])
    } else {
        lao_shape(index)
    }
}

#[derive(Debug)]
struct ScriptProvider {
    lao: Arc<GlyphProgram>,
    latin: Arc<GlyphProgram>,
    regions: Vec<[f64; 4]>,
}

impl ScriptProvider {
    fn new(regions: &[[f64; 4]]) -> Arc<Self> {
        let lao_pairs: Vec<(u16, u16)> = LAO_LETTERS
            .iter()
            .enumerate()
            .map(|(k, letter)| {
                (
                    u16::try_from(u32::from(*letter)).expect("bmp"),
                    u16::try_from(k + 1).expect("glyph"),
                )
            })
            .collect();
        let latin_pairs: Vec<(u16, u16)> =
            (0..16_u16).map(|k| (u16::from(b'a') + k, k + 1)).collect();
        Arc::new(Self {
            lao: sfnt_face(&(0..17).map(lao_shape).collect::<Vec<_>>(), 1, &lao_pairs),
            regions: regions.to_vec(),
            latin: sfnt_face(
                &(0..16).map(latin_shape).collect::<Vec<_>>(),
                1,
                &latin_pairs,
            ),
        })
    }

    fn face(program: &Arc<GlyphProgram>, family: &str) -> SubstitutedFace {
        SubstitutedFace {
            program: Arc::clone(program),
            identity: Arc::new(pdf_content::FaceIdentity {
                family: family.to_owned(),
                subfamily: "Regular".to_owned(),
                origin: format!("test:{family}"),
                sha256: family.to_owned(),
                face_index: 0,
                style: pdf_content::FontStyle::default(),
            }),
            reason: SubstitutionReason::ScriptCoverage,
        }
    }
}

impl FontProvider for ScriptProvider {
    fn primary_face(&self, _request: &FontRequest) -> Option<SubstitutedFace> {
        None
    }

    fn fallback_face(&self, _request: &FontRequest, character: char) -> Option<SubstitutedFace> {
        Some(if ('\u{0E80}'..='\u{0EFF}').contains(&character) {
            Self::face(&self.lao, "Lao Reference")
        } else {
            Self::face(&self.latin, "Latin Reference")
        })
    }

    fn description(&self) -> String {
        "script references".to_owned()
    }

    fn decipher_regions(&self, _page: pdf_syntax::Reference) -> Vec<[f64; 4]> {
        self.regions.clone()
    }
}

fn legacy_lao_page(destinations: impl Fn(usize) -> String) -> ByteStore {
    let glyphs = u16::try_from(LEGACY_GLYPHS).expect("glyphs");
    let pairs: Vec<(u16, u16)> = (0..glyphs).map(|k| (0xF061 + k, k + 1)).collect();
    let program = sfnt_bytes(&(0..16).map(legacy_shape).collect::<Vec<_>>(), 0, &pairs);
    let code_of = |text: &str| {
        let k = (0..LEGACY_GLYPHS)
            .position(|k| legacy_text(k) == text)
            .expect("a fixture glyph");
        char::from(b'a' + u8::try_from(k).expect("sixteen"))
    };
    let letters: String = "ການທຳຄວາມສະອາດແມນຊນ"
        .chars()
        .map(|letter| code_of(&letter.to_string()))
        .collect();
    let (tone, stacked) = (code_of("່"), code_of("ັ້"));
    let mut content = String::new();
    for line in 0..4 {
        let y = 90 - line * 20;
        let _ = write!(
            content,
            "BT /F1 12 Tf 10 {y} Td ({letters}) Tj ET \
             BT /F1 12 Tf 163.6 {y} Td ({tone}) Tj ET \
             BT /F1 12 Tf 182.8 {y} Td ({stacked}) Tj ET "
        );
    }
    let mut entries = String::new();
    for k in 0..LEGACY_GLYPHS {
        let _ = write!(entries, "<{:02X}> <{}> ", 0x61 + k, destinations(k));
    }
    let cmap = format!(
        "begincmap 1 begincodespacerange <00> <FF> endcodespacerange \
         {LEGACY_GLYPHS} beginbfchar {entries}endbfchar endcmap"
    );
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    let mut object = |bytes: &mut Vec<u8>, body: &[u8]| {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
        bytes.extend_from_slice(body);
        bytes.extend_from_slice(b"\nendobj\n");
    };
    let stream = |body: &[u8]| {
        let mut out = format!("<< /Length {} >>\nstream\n", body.len()).into_bytes();
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendstream");
        out
    };
    object(&mut bytes, b"<< /Type /Catalog /Pages 2 0 R >>");
    object(
        &mut bytes,
        b"<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>",
    );
    object(
        &mut bytes,
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
    );
    object(&mut bytes, &stream(content.as_bytes()));
    object(&mut bytes, b"<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+LAOFONTTEST /FirstChar 97 /LastChar 112 /Widths [800 800 800 800 800 800 800 800 800 800 800 800 800 800 800 800] /FontDescriptor 6 0 R /ToUnicode 8 0 R >>");
    object(&mut bytes, b"<< /Type /FontDescriptor /FontName /ABCDEF+LAOFONTTEST /Flags 4 /ItalicAngle 0 /FontFile2 7 0 R >>");
    object(&mut bytes, &stream(&program));
    object(&mut bytes, &stream(cmap.as_bytes()));
    let size = offsets.len() + 1;
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in &offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    ByteStore::new(SourceId::new(79), Arc::<[u8]>::from(bytes))
}

fn page_text(graph: &PaintGraph) -> (String, usize) {
    let mut text = String::new();
    let mut deciphered = 0;
    for run in runs(graph) {
        for glyph in &run.glyphs {
            if let Some(meaning) = run.text.text_of(pdf_content::Code {
                value: glyph.code.value,
                byte_len: glyph.code.bytes.len(),
            }) {
                text.push_str(&meaning.text);
                deciphered +=
                    usize::from(meaning.confidence == pdf_content::Confidence::Deciphered);
            }
        }
        text.push('\n');
    }
    (text, deciphered)
}

const WHOLE_PAGE: [f64; 4] = [0.0, 0.0, 200.0, 100.0];

#[test]
fn a_font_that_draws_lao_at_latin_codes_reads_as_lao() {
    let lying = legacy_lao_page(|k| format!("{:04X}", 0x61 + k));
    let graph = interpret(&lying, Some(ScriptProvider::new(&[WHOLE_PAGE]))).expect("interprets");
    assert_eq!(
        crate::decipher_fonts::reading_lines(&graph),
        vec![LAO_LINE.to_owned(); 4]
    );
    assert_eq!(page_text(&graph).1, 21 * 4);

    let latin = vec!["abcdefgbhijkblmhncopc".to_owned(); 4];
    let graph = interpret(&lying, None).expect("interprets");
    assert_eq!(page_text(&graph).1, 0);
    assert_eq!(crate::decipher_fonts::reading_lines(&graph), latin);
    let graph = interpret(&lying, Some(ScriptProvider::new(&[]))).expect("interprets");
    assert_eq!(crate::decipher_fonts::reading_lines(&graph), latin);
    let honest = legacy_lao_page(|k| {
        legacy_text(k)
            .chars()
            .fold(String::new(), |mut hex, character| {
                let _ = write!(hex, "{:04X}", u32::from(character));
                hex
            })
    });
    let graph = interpret(&honest, Some(ScriptProvider::new(&[WHOLE_PAGE]))).expect("interprets");
    assert_eq!(page_text(&graph).1, 0);
    assert_eq!(
        crate::decipher_fonts::reading_lines(&graph),
        vec![LAO_LINE.to_owned(); 4]
    );
}

#[test]
fn only_the_lines_being_edited_are_read_by_their_glyphs() {
    let lying = legacy_lao_page(|k| format!("{:04X}", 0x61 + k));
    let graph = interpret(
        &lying,
        Some(ScriptProvider::new(&[[0.0, 65.0, 200.0, 80.0]])),
    )
    .expect("interprets");
    let latin = "abcdefgbhijkblmhncopc".to_owned();
    assert_eq!(
        crate::decipher_fonts::reading_lines(&graph),
        vec![latin.clone(), LAO_LINE.to_owned(), latin.clone(), latin]
    );
    let graph = interpret(
        &lying,
        Some(ScriptProvider::new(&[[0.0, 85.0, 200.0, 100.0]])),
    )
    .expect("interprets");
    assert_eq!(crate::decipher_fonts::reading_lines(&graph)[0], LAO_LINE);
    let first_line: Vec<usize> = graph
        .atoms
        .iter()
        .enumerate()
        .filter(|(_, atom)| match &atom.kind {
            PaintAtomKind::Text(text) => {
                text.state.ctm.value.multiply(text.glyphs[0].matrix).f > 85.0
            }
            _ => false,
        })
        .map(|(at, _)| at)
        .collect();
    assert!(!first_line.is_empty());
    let provider = ScriptProvider::new(&[]);
    assert!(crate::decipher_fonts::reads_by_glyphs(
        &graph,
        &first_line,
        provider.as_ref()
    ));
    let second_line: Vec<usize> = graph
        .atoms
        .iter()
        .enumerate()
        .filter(|(_, atom)| match &atom.kind {
            PaintAtomKind::Text(text) => {
                let y = text.state.ctm.value.multiply(text.glyphs[0].matrix).f;
                (65.0..80.0).contains(&y)
            }
            _ => false,
        })
        .map(|(at, _)| at)
        .collect();
    assert!(crate::decipher_fonts::reads_by_glyphs(
        &graph,
        &second_line,
        provider.as_ref()
    ));
}

fn origins(graph: &PaintGraph) -> Vec<f64> {
    runs(graph)
        .iter()
        .flat_map(|run| run.glyphs.iter().map(|glyph| glyph.matrix.e))
        .collect()
}

#[test]
fn a_font_written_again_under_its_number_is_not_the_font_kept() {
    let content = "BT /F1 10 Tf 0 0 Td (A) Tj /F2 10 Tf (A) Tj /F1 10 Tf (A) Tj (A) Tj ET";
    let old = page_with(content, WIN_ANSI_FONT, None);
    let rewritten = old
        .as_bytes()
        .to_vec()
        .replace_all(b"/F1 5 0 R", b"/F2 5 0 R")
        .replace_all(b"[500 500 500]", b"[900 900 900]");
    let rewritten = ByteStore::new(SourceId::new(77), Arc::<[u8]>::from(rewritten));
    let limits = PageContentLimits::default();
    let page = load_page_program_strict(&old, 0, limits).expect("fixture page");
    let theirs = load_page_program_strict(&rewritten, 0, limits).expect("rewritten page");
    let written = theirs
        .resources
        .font(b"/F2")
        .expect("the rewritten font")
        .clone();
    let kept = page.resources.font(b"/F1").expect("the page's font");
    assert_eq!(written.reference(), kept.reference());
    assert!(!written.is_same_object(kept));
    assert!(kept.is_same_object(&kept.clone()));
    let again = load_page_program_strict(&page_with(content, WIN_ANSI_FONT, None), 0, limits)
        .expect("the same bytes, read again");
    assert!(
        !kept.is_same_object(again.resources.font(b"/F1").expect("font")),
        "equal bytes in another allocation are not proved the same"
    );

    let resources = page.resources.with_fonts([written]);
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("fixture operations");
    let graph = interpret_stream_sequence_with_fonts(
        &[PaintStream {
            source: &page.streams[0].bytes,
            reference: page.streams[0].reference,
            operations: &operations,
        }],
        page.page,
        &[],
        &resources,
        PaintLimits::default(),
        None,
    )
    .expect("interprets");

    let placed = origins(&graph);
    assert_eq!(placed.len(), 4);
    for (got, want) in placed.iter().zip([0.0, 5.0, 14.0, 19.0]) {
        assert!((got - want).abs() < 1e-9, "{placed:?}");
    }
}

trait ReplaceAll {
    fn replace_all(self, from: &[u8], to: &[u8]) -> Vec<u8>;
}

impl ReplaceAll for Vec<u8> {
    fn replace_all(self, from: &[u8], to: &[u8]) -> Vec<u8> {
        assert_eq!(from.len(), to.len(), "a replacement must keep every offset");
        let mut out = self;
        let mut at = 0;
        let mut found = false;
        while let Some(position) = out[at..]
            .windows(from.len())
            .position(|window| window == from)
        {
            let start = at + position;
            out[start..start + from.len()].copy_from_slice(to);
            at = start + from.len();
            found = true;
        }
        assert!(found, "the fixture holds what is replaced");
        out
    }
}

#[test]
fn a_remembered_glyph_drawing_belongs_to_its_program() {
    use pdf_content::decipher::Features;
    use pdf_content::outline_match::Raster;

    let large = sfnt_face(&[triangle_of(200)], 1, &[(0x41, 1)]);
    let small = sfnt_face(&[triangle_of(40)], 1, &[(0x41, 1)]);
    let direct = |program: &GlyphProgram| {
        Raster::of(program, 1).map(|raster| (raster.ink_box(), Features::of(&raster)))
    };
    let (large_digest, small_digest) = (
        crate::decipher_fonts::program_digest(&large),
        crate::decipher_fonts::program_digest(&small),
    );
    assert_ne!(large_digest, small_digest);
    for _ in 0..2 {
        let large_drawing = crate::decipher_fonts::drawing_of(&large, large_digest, 1);
        let small_drawing = crate::decipher_fonts::drawing_of(&small, small_digest, 1);
        assert!(large_drawing.is_some() && small_drawing.is_some());
        assert_eq!(large_drawing, direct(&large));
        assert_eq!(small_drawing, direct(&small));
        assert_ne!(large_drawing, small_drawing);
    }
}
