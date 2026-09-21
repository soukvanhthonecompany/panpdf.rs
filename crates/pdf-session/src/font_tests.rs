use super::{PageView, Session};
use pdf_bytes::{ByteStore, SourceId};
use pdf_content::{FontProvider, FontRequest, GlyphProgram, SubstitutedFace, SubstitutionReason};
use pdf_edit::{Command, TextRunSelection};
use std::sync::Arc;

#[derive(Debug)]
struct Triangle(Arc<GlyphProgram>);
impl FontProvider for Triangle {
    fn primary_face(&self, _: &FontRequest) -> Option<SubstitutedFace> {
        Some(SubstitutedFace {
            program: self.0.clone(),
            identity: Arc::new(pdf_content::FaceIdentity {
                family: "Test Triangle".into(),
                subfamily: "Regular".into(),
                origin: "test:triangle".into(),
                sha256: String::new(),
                face_index: 0,
                style: pdf_content::FontStyle::default(),
            }),
            reason: SubstitutionReason::ExactFamily,
        })
    }
    fn fallback_face(&self, _: &FontRequest, _: char) -> Option<SubstitutedFace> {
        None
    }
    fn description(&self) -> String {
        "test triangle".into()
    }
}

fn fixture() -> ByteStore {
    let content = "BT /F1 12 Tf 10 20 Td (A) Tj ET";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 100] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /TrueType /BaseFont /Test /Encoding /WinAnsiEncoding /FirstChar 65 /LastChar 65 /Widths [500] >>".into(),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    ByteStore::new(SourceId::new(771), Arc::<[u8]>::from(bytes))
}

fn bounds(page: &PageView) -> Option<[f64; 4]> {
    page.graph.atoms[0].kind.user_bounds()
}

#[test]
fn held_preview_resources_refer_to_the_compacted_committed_bytes() {
    let mut session = Session::new(fixture(), b"");
    for _ in 0..3 {
        let plan = session
            .plan(&Command::MoveTextRun {
                page_index: 0,
                selection: TextRunSelection::Last,
                dx: 1.0,
                dy: 0.0,
            })
            .unwrap();
        let reading = Arc::new(session.preview(&plan).unwrap());
        session.apply_holding(plan, Some(reading)).unwrap();
        let held = session.held_page(0).unwrap();
        let resource = held.program.resources.font(b"/F1").unwrap();
        assert_eq!(
            resource.source().as_bytes(),
            session.source().as_bytes(),
            "matching revision ids alone must not hide different source bytes"
        );
        assert_eq!(resource.source().id(), session.source().id());
    }
}

#[test]
fn substitute_ink_is_in_the_plan_preview_commit_and_undo() {
    let font = super::tests::hex(
        "000100000005000000000000636d6170000000000000005c00000112676c7966000000000000016e00000018686561640000\
        000000000186000000366c6f636100000000000001bc000000066d61787000000000000001c2000000060000000100030001\
        0000000c00000106000000000000000000000000000000000000000000000000000000000000000000000000000000000000\
        0000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000\
        0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000\
        0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000\
        0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000\
        0000000000000000000000000000000000010000000000640064000200003737370064000000640000000000000000000000\
        000000000000000003e80000000000000000000000000000000000000000000000000000000000000000000000000000000c\
        000000000002",
    );
    let provider: Arc<dyn FontProvider> =
        Arc::new(Triangle(Arc::new(GlyphProgram::parse(font).expect("font"))));
    let source = fixture();
    assert_eq!(
        bounds(&super::interpret_page(&source, 0).expect("no provider")),
        None
    );
    let mut session = Session::with_fonts(source.clone(), b"", Some(provider.clone()));
    assert_eq!(
        bounds(&session.page(0).expect("original")),
        Some([10.0, 20.0, 11.2, 21.2])
    );
    let command = Command::MoveTextRun {
        page_index: 0,
        selection: TextRunSelection::Last,
        dx: 10.0,
        dy: 0.0,
    };
    let standalone =
        pdf_edit::spike_move_text::plan_command_with_fonts(&source, &command, b"", Some(provider))
            .expect("plan with fonts");
    let plan = session.plan(&command).expect("session plan");
    assert_eq!(
        standalone.effect().declared_region,
        plan.effect().declared_region
    );
    assert_eq!(
        plan.effect().declared_region,
        Some([10.0, 20.0, 21.2, 21.2])
    );
    assert_eq!(
        bounds(&session.preview(&plan).expect("preview")),
        Some([20.0, 20.0, 21.2, 21.2])
    );
    session.apply(plan).expect("commit");
    assert_eq!(
        bounds(&session.page(0).expect("committed")),
        Some([20.0, 20.0, 21.2, 21.2])
    );
    assert!(session.undo().expect("undo"));
    assert!(session.source().as_bytes().starts_with(source.as_bytes()));
    assert_eq!(
        bounds(&session.page(0).expect("undone")),
        Some([10.0, 20.0, 11.2, 21.2])
    );
    assert!(session.redo().expect("redo"));
    assert_eq!(
        bounds(&session.page(0).expect("redone")),
        Some([20.0, 20.0, 21.2, 21.2])
    );
}

fn square_face(size: i16) -> Arc<GlyphProgram> {
    square_face_for(size, &[('A', 1)])
}

fn square_face_for(size: i16, characters: &[(char, u16)]) -> Arc<GlyphProgram> {
    let mut glyf = Vec::new();
    glyf.extend_from_slice(&1_i16.to_be_bytes());
    for value in [0_i16, 0, size, size] {
        glyf.extend_from_slice(&value.to_be_bytes());
    }
    glyf.extend_from_slice(&3_u16.to_be_bytes());
    glyf.extend_from_slice(&0_u16.to_be_bytes());
    glyf.extend_from_slice(&[0x37, 0x37, 0x37, 0x37]);
    glyf.extend_from_slice(&[0, size.to_be_bytes()[1], 0, 0_u8.wrapping_neg()]);
    glyf.truncate(10 + 2 * 4 + 2 + 2 + 4);
    let mut body = Vec::new();
    body.extend_from_slice(&1_i16.to_be_bytes());
    for value in [0_i16, 0, size, size] {
        body.extend_from_slice(&value.to_be_bytes());
    }
    body.extend_from_slice(&3_u16.to_be_bytes());
    body.extend_from_slice(&0_u16.to_be_bytes());
    body.extend_from_slice(&[0x01, 0x01, 0x01, 0x01]);
    for delta in [0_i16, size, 0, -size] {
        body.extend_from_slice(&delta.to_be_bytes());
    }
    for delta in [0_i16, 0, size, 0] {
        body.extend_from_slice(&delta.to_be_bytes());
    }
    if body.len() % 2 == 1 {
        body.push(0);
    }
    let mut head = vec![0_u8; 54];
    head[18..20].copy_from_slice(&1000_u16.to_be_bytes());
    head[50..52].copy_from_slice(&0_u16.to_be_bytes());
    let squares = characters
        .iter()
        .map(|(_, glyph)| *glyph)
        .max()
        .unwrap_or(1)
        .max(1);
    let mut maxp = vec![0_u8; 6];
    maxp[0..4].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
    maxp[4..6].copy_from_slice(&(squares + 1).to_be_bytes());
    let half = u16::try_from(body.len() / 2).expect("loca");
    let loca: Vec<u8> = std::iter::once(0_u16)
        .chain((0..=squares).map(|glyph| glyph * half))
        .flat_map(u16::to_be_bytes)
        .collect();
    let glyf = body.repeat(usize::from(squares));
    let tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"cmap", character_cmap(characters)),
        (b"glyf", glyf),
        (b"head", head),
        (b"loca", loca),
        (b"maxp", maxp),
    ];
    Arc::new(GlyphProgram::parse(sfnt(&tables)).expect("the fixture face parses"))
}

fn character_cmap(characters: &[(char, u16)]) -> Vec<u8> {
    let mut segments: Vec<(u16, u16, i16)> = characters
        .iter()
        .map(|(character, glyph)| {
            let code = u16::try_from(u32::from(*character)).expect("BMP");
            (code, code, glyph.wrapping_sub(code).cast_signed())
        })
        .collect();
    segments.sort_unstable();
    segments.push((0xFFFF, 0xFFFF, 1));
    let count = u16::try_from(segments.len()).expect("segments");
    let mut subtable = Vec::new();
    subtable.extend_from_slice(&4_u16.to_be_bytes());
    subtable.extend_from_slice(&(16 + count * 8).to_be_bytes());
    subtable.extend_from_slice(&0_u16.to_be_bytes());
    subtable.extend_from_slice(&(count * 2).to_be_bytes());
    subtable.extend_from_slice(&[0; 6]);
    for (_, end, _) in &segments {
        subtable.extend_from_slice(&end.to_be_bytes());
    }
    subtable.extend_from_slice(&0_u16.to_be_bytes());
    for (start, _, _) in &segments {
        subtable.extend_from_slice(&start.to_be_bytes());
    }
    for (_, _, delta) in &segments {
        subtable.extend_from_slice(&delta.to_be_bytes());
    }
    for _ in &segments {
        subtable.extend_from_slice(&0_u16.to_be_bytes());
    }
    let mut out = Vec::new();
    out.extend_from_slice(&0_u16.to_be_bytes());
    out.extend_from_slice(&1_u16.to_be_bytes());
    out.extend_from_slice(&3_u16.to_be_bytes());
    out.extend_from_slice(&1_u16.to_be_bytes());
    out.extend_from_slice(&12_u32.to_be_bytes());
    out.extend_from_slice(&subtable);
    out
}

fn sfnt(tables: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let count = u16::try_from(tables.len()).expect("table count");
    let mut out = Vec::new();
    out.extend_from_slice(&0x0001_0000_u32.to_be_bytes());
    out.extend_from_slice(&count.to_be_bytes());
    out.extend_from_slice(&[0; 6]);
    let mut records = Vec::new();
    let mut body = Vec::new();
    for (tag, bytes) in tables {
        records.push((*tag, 12 + usize::from(count) * 16 + body.len(), bytes.len()));
        body.extend_from_slice(bytes);
        while body.len() % 4 != 0 {
            body.push(0);
        }
    }
    for (tag, start, length) in records {
        out.extend_from_slice(tag);
        out.extend_from_slice(&0_u32.to_be_bytes());
        out.extend_from_slice(&u32::try_from(start).expect("offset").to_be_bytes());
        out.extend_from_slice(&u32::try_from(length).expect("length").to_be_bytes());
    }
    out.extend_from_slice(&body);
    out
}

#[derive(Debug)]
struct NamedFace {
    program: Arc<GlyphProgram>,
    family: &'static str,
    sha256: &'static str,
}

impl FontProvider for NamedFace {
    fn primary_face(&self, _: &FontRequest) -> Option<SubstitutedFace> {
        Some(SubstitutedFace {
            program: Arc::clone(&self.program),
            identity: Arc::new(pdf_content::FaceIdentity {
                family: self.family.to_owned(),
                subfamily: "Regular".to_owned(),
                origin: format!("test:{}", self.family),
                sha256: self.sha256.to_owned(),
                face_index: 0,
                style: pdf_content::FontStyle::default(),
            }),
            reason: SubstitutionReason::ExactFamily,
        })
    }
    fn fallback_face(&self, _: &FontRequest, _: char) -> Option<SubstitutedFace> {
        None
    }
    fn description(&self) -> String {
        format!("test face {}", self.family)
    }
}

fn small() -> Arc<dyn FontProvider> {
    Arc::new(NamedFace {
        program: square_face(100),
        family: "Small Square",
        sha256: "1".repeat(64).leak(),
    })
}

fn large() -> Arc<dyn FontProvider> {
    Arc::new(NamedFace {
        program: square_face(500),
        family: "Large Square",
        sha256: "2".repeat(64).leak(),
    })
}

fn page_showing(content: &str) -> ByteStore {
    page_with_resources(content, "")
}

fn page_with_resources(content: &str, extra: &str) -> ByteStore {
    page_with_font(
        content,
        extra,
        "<< /Type /Font /Subtype /TrueType /BaseFont /Test /Encoding /WinAnsiEncoding \
         /FirstChar 65 /LastChar 65 /Widths [500] >>",
    )
}

fn page_with_font(content: &str, extra: &str, font: &str) -> ByteStore {
    page_with_objects(content, extra, font, &[])
}

fn page_with_objects(content: &str, extra: &str, font: &str, more: &[&str]) -> ByteStore {
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>".into(),
        format!(
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> {extra} >> >>"
        ),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ),
        font.to_owned(),
    ];
    objects.extend(more.iter().map(|object| (*object).to_owned()));
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    let size = objects.len() + 1;
    bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    ByteStore::new(SourceId::new(772), Arc::<[u8]>::from(bytes))
}

fn faces_drawn(page: &PageView) -> Vec<String> {
    let mut found = Vec::new();
    for atom in &page.graph.atoms {
        let pdf_paint::PaintAtomKind::Text(text) = &atom.kind else {
            continue;
        };
        let Some(substitution) = text.substitution.as_ref() else {
            continue;
        };
        for glyph in &text.glyphs {
            for drawn in &glyph.substituted {
                let face = std::iter::once(&substitution.primary)
                    .chain(&substitution.fallbacks)
                    .nth(usize::from(drawn.face))
                    .expect("a face the run names");
                found.push(format!("{}/{}", face.identity.family, drawn.glyph));
            }
        }
    }
    found
}

#[test]
fn a_move_declares_the_substituted_ink_it_shows_and_keeps_the_same_face_throughout() {
    let source = page_showing("BT /F1 12 Tf 10 60 Td (A) Tj 0 -40 Td (A) Tj ET");
    let mut session = Session::with_fonts(source.clone(), b"", Some(large()));
    let before = session.page(0).expect("original");
    assert_eq!(bounds(&before), Some([10.0, 60.0, 16.0, 66.0]));
    assert_eq!(faces_drawn(&before), vec!["Large Square/1"; 2]);

    let command = Command::MoveTextRun {
        page_index: 0,
        selection: TextRunSelection::Last,
        dx: 10.0,
        dy: 0.0,
    };
    let plan = session.plan(&command).expect("a plan");
    assert_eq!(
        plan.effect().declared_region,
        Some([10.0, 20.0, 26.0, 26.0])
    );
    let preview = session.preview(&plan).expect("a preview");
    assert_eq!(second_run_bounds(&preview), Some([20.0, 20.0, 26.0, 26.0]));
    assert_eq!(faces_drawn(&preview), vec!["Large Square/1"; 2]);

    session.apply(plan).expect("commit");
    let committed = session.page(0).expect("committed");
    assert_eq!(
        bounds(&committed),
        Some([10.0, 60.0, 16.0, 66.0]),
        "the neighbour did not move"
    );
    assert_eq!(
        second_run_bounds(&committed),
        Some([20.0, 20.0, 26.0, 26.0])
    );
    assert_eq!(faces_drawn(&committed), vec!["Large Square/1"; 2]);

    let reopened = Session::with_fonts(session.source().clone(), b"", Some(large()))
        .page(0)
        .expect("reopened");
    assert_eq!(bounds(&reopened), bounds(&committed));
    assert_eq!(faces_drawn(&reopened), faces_drawn(&committed));

    assert!(session.undo().expect("undo"));
    assert_eq!(
        second_run_bounds(&session.page(0).expect("undone")),
        Some([10.0, 20.0, 16.0, 26.0])
    );
    assert!(session.redo().expect("redo"));
    assert_eq!(
        second_run_bounds(&session.page(0).expect("redone")),
        Some([20.0, 20.0, 26.0, 26.0])
    );
}

fn second_run_bounds(page: &PageView) -> Option<[f64; 4]> {
    page.graph.atoms.get(1)?.kind.user_bounds()
}

#[test]
fn swapping_the_provider_changes_the_declared_region_and_the_face_that_drew() {
    let source = page_showing("BT /F1 12 Tf 10 20 Td (A) Tj ET");
    let region = |provider: Arc<dyn FontProvider>| {
        let mut session = Session::with_fonts(source.clone(), b"", Some(provider));
        let page = session.page(0).expect("page");
        let plan = session
            .plan(&Command::MoveTextRun {
                page_index: 0,
                selection: TextRunSelection::Last,
                dx: 10.0,
                dy: 0.0,
            })
            .expect("a plan");
        (
            bounds(&page),
            faces_drawn(&page),
            plan.effect().declared_region,
        )
    };
    let (small_bounds, small_faces, small_region) = region(small());
    let (large_bounds, large_faces, large_region) = region(large());
    assert_eq!(small_bounds, Some([10.0, 20.0, 11.2, 21.2]));
    assert_eq!(large_bounds, Some([10.0, 20.0, 16.0, 26.0]));
    assert_ne!(small_faces, large_faces);
    assert_ne!(
        small_region, large_region,
        "if these are equal the planner is not measuring the substituted face"
    );

    let mut none = Session::new(source, b"");
    assert_eq!(bounds(&none.page(0).expect("page")), None);
    assert_eq!(
        none.plan(&Command::MoveTextRun {
            page_index: 0,
            selection: TextRunSelection::Last,
            dx: 10.0,
            dy: 0.0,
        })
        .expect("a plan")
        .effect()
        .declared_region,
        None
    );
}

#[test]
fn a_cluster_delete_proves_itself_against_a_candidate_read_with_the_same_provider() {
    let source = page_showing("BT /F1 12 Tf 10 20 Td (AA) Tj ET");
    let mut session = Session::with_fonts(source, b"", Some(large()));
    let before = session.page(0).expect("original");
    assert_eq!(faces_drawn(&before), vec!["Large Square/1"; 2]);

    let plan = session
        .plan(&Command::DeleteTextClusters {
            page_index: 0,
            selection: TextRunSelection::Last,
            glyphs: 1..2,
        })
        .expect("a cluster delete plan");
    assert_eq!(
        plan.effect().declared_region,
        Some([16.0, 20.0, 22.0, 26.0])
    );
    session.apply(plan).expect("commit");
    let after = session.page(0).expect("committed");
    assert_eq!(
        faces_drawn(&after),
        vec!["Large Square/1"],
        "one glyph gone, the other still drawn from the same face"
    );
    assert_eq!(bounds(&after), Some([10.0, 20.0, 16.0, 26.0]));
}

#[test]
fn a_text_clip_over_a_substituted_run_is_interpreted_with_the_same_provider() {
    let content = "BT 7 Tr /F1 12 Tf 10 20 Td (A) Tj ET 0 0 200 200 re f";
    let source = page_showing(content);
    let mut session = Session::with_fonts(source.clone(), b"", Some(large()));
    let page = session
        .page(0)
        .expect("the clip interprets with a provider");
    assert_eq!(page.graph.atoms.len(), 2, "the run and the rectangle");

    let refused = super::interpret_page(&source, 0);
    assert!(
        refused.is_err(),
        "a clipping run with no outlines must fail closed, not clip nothing"
    );
}

fn pens(page: &PageView) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for atom in &page.graph.atoms {
        let pdf_paint::PaintAtomKind::Text(text) = &atom.kind else {
            continue;
        };
        for glyph in &text.glyphs {
            let pen = text.state.ctm.value.transform(pdf_paint::Point {
                x: glyph.text_matrix.e,
                y: glyph.text_matrix.f,
            });
            out.push((pen.x, pen.y));
        }
    }
    out
}

fn first_block_rows(page: &PageView) -> Vec<Vec<pdf_edit::ClusterRef>> {
    page.index.blocks[0]
        .lines
        .iter()
        .map(|line| {
            page.index.lines[*line]
                .clusters
                .iter()
                .map(|cluster| {
                    let cluster = &page.index.clusters[*cluster];
                    pdf_edit::ClusterRef {
                        anchor: pdf_edit::SourceAnchor::of(&page.graph.atoms[cluster.atom].id),
                        glyphs: cluster.glyphs.clone(),
                    }
                })
                .collect()
        })
        .collect()
}

#[test]
fn a_block_in_a_substituted_font_lays_out() {
    let source = page_showing("BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET");
    let command = |session: &mut Session| {
        let page = session.page(0).expect("the page reads");
        Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            frame_declared: true,
            range: pdf_edit::BlockRange::Between {
                from: (0, 2),
                to: (0, 2),
            },
            text: "\n".to_owned(),
        }
    };

    let mut session = Session::with_fonts(source.clone(), b"", Some(large()));
    let rewrite = command(&mut session);
    let plan = session
        .plan(&rewrite)
        .expect("a substituted block lays out");
    session.apply(plan).expect("applies");
    let after = session.page(0).expect("the edited page reads");
    let found = pens(&after);
    let wanted = [(10.0, 150.0), (16.0, 150.0), (10.0, 136.0), (16.0, 136.0)];
    assert_eq!(found.len(), wanted.len(), "{found:?}");
    for (one, other) in found.iter().zip(wanted) {
        assert!(
            (one.0 - other.0).abs() < 1e-6 && (one.1 - other.1).abs() < 1e-6,
            "{found:?}"
        );
    }
    assert_eq!(
        faces_drawn(&after).len(),
        4,
        "every letter still draws from the substituted face"
    );

    let mut bare = Session::new(source, b"");
    let rewrite = command(&mut bare);
    let refused = bare
        .plan(&rewrite)
        .expect_err("control: nothing draws the run");
    assert!(
        refused.to_string().contains("no embedded outline font"),
        "control: {refused}"
    );
}

#[test]
fn text_after_a_rewritten_block_in_its_text_object_stays_where_it_was() {
    let source = page_showing("BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj 0 -60 Td (AA) Tj ET");
    let mut session = Session::with_fonts(source, b"", Some(large()));
    let page = session.page(0).expect("the page reads");
    let rewrite = Command::RewriteBlock {
        paragraph: pdf_edit::ParagraphLayout::default(),
        page_index: 0,
        rows: first_block_rows(&page),
        frame: (10.0, 190.0),
        edges: (0, 0),
        breaks: None,
        frame_declared: true,
        range: pdf_edit::BlockRange::Between {
            from: (0, 2),
            to: (0, 2),
        },
        text: "\n".to_owned(),
    };
    let plan = session
        .plan(&rewrite)
        .expect("the follower keeps its place");
    session.apply(plan).expect("applies");
    let after = session.page(0).expect("the edited page reads");
    let found = pens(&after);
    let wanted = [
        (10.0, 150.0),
        (16.0, 150.0),
        (10.0, 136.0),
        (16.0, 136.0),
        (10.0, 90.0),
        (16.0, 90.0),
    ];
    assert_eq!(found.len(), wanted.len(), "{found:?}");
    for (one, other) in found.iter().zip(wanted) {
        assert!(
            (one.0 - other.0).abs() < 1e-6 && (one.1 - other.1).abs() < 1e-6,
            "{found:?}"
        );
    }
}

fn pens_right_of(page: &PageView, x: f64) -> Vec<(f64, f64)> {
    pens(page).into_iter().filter(|pen| pen.0 > x).collect()
}

fn enter_in_first_block(content: &str, breaks: Option<Vec<usize>>) -> Result<Session, String> {
    enter_in_first_block_of(page_showing(content), breaks)
}

fn enter_in_first_block_of(
    source: ByteStore,
    breaks: Option<Vec<usize>>,
) -> Result<Session, String> {
    let mut session = Session::with_fonts(source, b"", Some(large()));
    let page = session.page(0).expect("the page reads");
    let rewrite = Command::RewriteBlock {
        paragraph: pdf_edit::ParagraphLayout::default(),
        page_index: 0,
        rows: first_block_rows(&page),
        frame: (10.0, 100.0),
        edges: (0, 0),
        breaks: breaks.map(pdf_edit::RowEnds::paragraphs),
        frame_declared: true,
        range: pdf_edit::BlockRange::Between {
            from: (0, 2),
            to: (0, 2),
        },
        text: "\n".to_owned(),
    };
    let plan = session.plan(&rewrite).map_err(|error| error.to_string())?;
    session.apply(plan).expect("applies");
    Ok(session)
}

fn assert_pens(found: &[(f64, f64)], wanted: &[(f64, f64)]) {
    assert_eq!(found.len(), wanted.len(), "{found:?}");
    for (one, other) in found.iter().zip(wanted) {
        assert!(
            (one.0 - other.0).abs() < 1e-6 && (one.1 - other.1).abs() < 1e-6,
            "{found:?}"
        );
    }
}

#[test]
fn text_drawn_between_a_blocks_shows_stays_where_it_was() {
    let mut session = enter_in_first_block(
        "BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj 140 0 Td (AA) Tj -140 -14 Td (AAAA) Tj ET",
        Some(vec![0]),
    )
    .expect("the text between keeps its place");
    let after = session.page(0).expect("the edited page reads");
    assert_pens(
        &pens_right_of(&after, 100.0),
        &[(150.0, 150.0), (156.0, 150.0)],
    );
}

#[test]
fn text_continuing_from_a_rewritten_block_stays_where_it_was() {
    let mut session = enter_in_first_block(
        "BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj [-8000 (AA)] TJ ET",
        None,
    )
    .expect("the continuing text keeps its place");
    let after = session.page(0).expect("the edited page reads");
    assert_pens(
        &pens_right_of(&after, 100.0),
        &[(130.0, 150.0), (136.0, 150.0)],
    );
}

#[test]
fn a_block_its_clip_already_cuts_takes_enter_and_says_what_the_clip_hides() {
    let enter = |clip: &str| {
        enter_in_first_block(
            &format!("q {clip} re W n BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET Q"),
            None,
        )
    };
    let laid = [(10.0, 150.0), (16.0, 150.0), (10.0, 136.0), (16.0, 136.0)];
    let mut session = enter("0 0 200 153").expect("the moved glyphs fit the clip");
    let after = session.page(0).expect("the edited page reads");
    assert_pens(&pens(&after), &laid);

    let mut session = enter("0 139 200 14").expect("a clip is a window, not a refusal");
    let after = session.page(0).expect("the edited page reads");
    assert_pens(&pens(&after), &laid);
}

#[test]
fn the_plan_for_a_clipped_enter_says_whether_a_crop_hides_part_of_it() {
    let cropped = |clip: &str| {
        let source = page_showing(&format!(
            "q {clip} re W n BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET Q"
        ));
        let mut session = Session::with_fonts(source, b"", Some(large()));
        let page = session.page(0).expect("the page reads");
        let rows = first_block_rows(&page);
        let plan = session
            .plan(&Command::RewriteBlock {
                paragraph: pdf_edit::ParagraphLayout::default(),
                page_index: 0,
                rows,
                frame: (10.0, 100.0),
                edges: (0, 0),
                breaks: None,
                frame_declared: true,
                range: pdf_edit::BlockRange::Between {
                    from: (0, 2),
                    to: (0, 2),
                },
                text: "\n".to_owned(),
            })
            .expect("a clip is a window, not a refusal");
        (
            plan.block().expect("a block rewrite").cropped,
            plan.showing(),
        )
    };
    assert_eq!(
        cropped("0 0 200 153"),
        (false, pdf_edit::Showing::Whole),
        "control: the moved pair fits, so nothing is said"
    );
    assert_eq!(
        cropped("0 139 200 14"),
        (true, pdf_edit::Showing::PartlyHidden),
        "the moved pair lands under the crop, and the plan says so"
    );
}

#[test]
fn a_row_written_under_another_rows_clip_is_asked_about_it() {
    let plan = |rule: &str| {
        let source = page_showing(&format!(
            "q 0 139 200 20 re W n {rule} BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET Q \
         BT /F1 12 Tf 14 TL 10 136 Td (AAAA) Tj ET"
        ));
        let mut session = Session::with_fonts(source, b"", Some(large()));
        let page = session.page(0).expect("the page reads");
        let rows = first_block_rows(&page);
        assert_eq!(rows.len(), 2, "one block of two rows");
        let rewrite = Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows,
            frame: (10.0, 100.0),
            edges: (0, 0),
            breaks: Some(pdf_edit::RowEnds::paragraphs(vec![0])),
            frame_declared: true,
            range: pdf_edit::BlockRange::Between {
                from: (1, 4),
                to: (1, 4),
            },
            text: "\n".to_owned(),
        };
        session
            .plan(&rewrite)
            .map(|plan| plan.block().expect("a block rewrite").cropped)
            .map_err(|error| error.to_string())
    };
    assert_eq!(
        plan(""),
        Ok(false),
        "control: a clip that hides nothing is lifted, so nothing is said"
    );
    assert_eq!(
        plan("0 100 m 200 200 l S"),
        Ok(true),
        "the second row is written under a crop, and the plan says so"
    );
}

#[test]
fn a_spot_fill_after_a_run_that_does_not_fill_is_written_in_its_own_space() {
    let source = page_with_resources(
        "BT /F1 12 Tf 14 TL 1 Tr 10 150 Td (AA) Tj 0 Tr /CS0 cs 1 scn (AA) Tj ET",
        SPOT,
    );
    let mut session =
        enter_in_first_block_of(source, None).expect("a spot fill is written in its space");
    let after = session.page(0).expect("the edited page reads");
    assert_eq!(
        inks(&after),
        [
            "10 150 Stroke fill DeviceGray(0.0) stroke DeviceGray(0.0)",
            "16 150 Stroke fill DeviceGray(0.0) stroke DeviceGray(0.0)",
            "10 136 Fill fill /Spot Separation(1.0) stroke DeviceGray(0.0)",
            "16 136 Fill fill /Spot Separation(1.0) stroke DeviceGray(0.0)",
        ]
    );
}

#[test]
fn two_spot_inks_at_one_tint_stay_two_inks() {
    let source = page_with_resources(
        "BT /F1 12 Tf 14 TL /CS0 cs 1 scn 10 150 Td (AA) Tj 1 Tc /CS1 cs 1 scn (AA) Tj ET",
        SPOT,
    );
    let mut session = enter_in_first_block_of(source, None).expect("two inks lay out");
    let after = session.page(0).expect("the edited page reads");
    assert_eq!(
        inks(&after),
        [
            "10 150 Fill fill /Spot Separation(1.0) stroke DeviceGray(0.0)",
            "16 150 Fill fill /Spot Separation(1.0) stroke DeviceGray(0.0)",
            "10 136 Fill fill /Other Separation(1.0) stroke DeviceGray(0.0)",
            "17 136 Fill fill /Other Separation(1.0) stroke DeviceGray(0.0)",
        ]
    );
}

const SPOT: &str = "/ColorSpace << /CS0 [/Separation /Spot /DeviceGray \
    << /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [0] /N 1 >>] \
    /CS1 [/Separation /Other /DeviceGray \
    << /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [0.5] /N 1 >>] >>";

fn inks(page: &PageView) -> Vec<String> {
    let described = |space: &pdf_paint::ColorSpace, colour: &pdf_paint::Color| match space {
        pdf_paint::ColorSpace::Separation(separation) => format!(
            "{} {}",
            match &separation.colorant.value {
                pdf_paint::Colorant::Named(name) => String::from_utf8_lossy(name).into_owned(),
                other => format!("{other:?}"),
            },
            pdf_paint::colour_signature(colour)
        ),
        _ => pdf_paint::colour_signature(colour),
    };
    let mut out = Vec::new();
    for atom in &page.graph.atoms {
        let pdf_paint::PaintAtomKind::Text(text) = &atom.kind else {
            continue;
        };
        let state = &text.state;
        for glyph in &text.glyphs {
            let pen = state.ctm.value.transform(pdf_paint::Point {
                x: glyph.text_matrix.e,
                y: glyph.text_matrix.f,
            });
            out.push(format!(
                "{} {} {:?} fill {} stroke {}",
                pen.x,
                pen.y,
                state.text.rendering_mode.value,
                described(&state.fill_color_space.value, &state.fill_color.value),
                described(&state.stroke_color_space.value, &state.stroke_color.value),
            ));
        }
    }
    out
}

#[test]
fn a_block_differing_only_in_a_stroke_colour_it_never_paints_lays_out() {
    let enter = |mode: u8| {
        let source = page_with_resources(
            &format!(
                "BT /F1 12 Tf 14 TL {mode} Tr /CS0 CS 1 SCN 10 150 Td (AA) Tj 1 Tc 0.5 SCN (AA) Tj ET"
            ),
            SPOT,
        );
        let mut session = Session::with_fonts(source, b"", Some(large()));
        let page = session.page(0).expect("the page reads");
        let rewrite = Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            frame_declared: true,
            range: pdf_edit::BlockRange::Between {
                from: (0, 2),
                to: (0, 2),
            },
            text: "\n".to_owned(),
        };
        let planned = session.plan(&rewrite);
        (session, planned)
    };

    let (mut session, planned) = enter(0);
    session
        .apply(planned.expect("an unpainted stroke colour is no part of the setting"))
        .expect("applies");
    let after = session.page(0).expect("the edited page reads");
    assert_pens(
        &pens(&after),
        &[(10.0, 150.0), (16.0, 150.0), (10.0, 136.0), (17.0, 136.0)],
    );

    let (mut session, planned) = enter(2);
    session
        .apply(planned.expect("a stroked spot colour is written in its space"))
        .expect("applies");
    let after = session.page(0).expect("the edited page reads");
    assert_eq!(
        inks(&after),
        [
            "10 150 FillStroke fill DeviceGray(0.0) stroke /Spot Separation(1.0)",
            "16 150 FillStroke fill DeviceGray(0.0) stroke /Spot Separation(1.0)",
            "10 136 FillStroke fill DeviceGray(0.0) stroke /Spot Separation(0.5)",
            "17 136 FillStroke fill DeviceGray(0.0) stroke /Spot Separation(0.5)",
        ]
    );
}

#[test]
fn a_block_naming_part_of_a_show_operation_lays_out_and_leaves_the_rest() {
    let source = page_showing("BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET");
    let mut session = Session::with_fonts(source, b"", Some(large()));
    let page = session.page(0).expect("the page reads");
    let mut rows = first_block_rows(&page);
    assert_eq!(rows.len(), 1, "one row");
    assert_eq!(rows[0].len(), 4, "four clusters, one a letter");
    rows[0].truncate(2);
    let plan = session
        .plan(&Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows,
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            frame_declared: true,
            range: pdf_edit::BlockRange::Between {
                from: (0, 1),
                to: (0, 1),
            },
            text: "\n".to_owned(),
        })
        .expect("a block naming part of a run lays out");
    session.apply(plan).expect("applies");
    let after = session.page(0).expect("the edited page reads");
    let found = pens(&after);
    let wanted = [(10.0, 150.0), (10.0, 136.0), (22.0, 150.0), (28.0, 150.0)];
    assert_eq!(found.len(), wanted.len(), "{found:?}");
    for (one, other) in found.iter().zip(wanted) {
        assert!(
            (one.0 - other.0).abs() < 1e-6 && (one.1 - other.1).abs() < 1e-6,
            "{found:?}"
        );
    }
}

fn lined_square_face() -> Arc<GlyphProgram> {
    let plain = square_face(500);
    let bytes = plain.program_bytes();
    let count = usize::from(u16::from_be_bytes([bytes[4], bytes[5]]));
    let mut tables: Vec<(&[u8; 4], Vec<u8>)> = Vec::with_capacity(count + 1);
    let tags: [&[u8; 4]; 5] = [b"cmap", b"glyf", b"head", b"loca", b"maxp"];
    for index in 0..count {
        let record = &bytes[12 + index * 16..28 + index * 16];
        let offset = usize::try_from(u32::from_be_bytes([
            record[8], record[9], record[10], record[11],
        ]))
        .expect("offset");
        let length = usize::try_from(u32::from_be_bytes([
            record[12], record[13], record[14], record[15],
        ]))
        .expect("length");
        let tag = tags
            .into_iter()
            .find(|tag| tag[..] == record[..4])
            .expect("a table square_face writes");
        tables.push((tag, bytes[offset..offset + length].to_vec()));
    }
    let mut hhea = vec![0_u8; 36];
    hhea[0..4].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
    hhea[4..6].copy_from_slice(&800_i16.to_be_bytes());
    hhea[6..8].copy_from_slice(&(-200_i16).to_be_bytes());
    let at = tables
        .iter()
        .position(|(tag, _)| tag[..] == b"loca"[..])
        .expect("loca");
    tables.insert(at, (b"hhea", hhea));
    Arc::new(GlyphProgram::parse(sfnt(&tables)).expect("the lined face parses"))
}

#[test]
fn a_one_row_block_with_no_stated_line_takes_the_line_of_the_face_that_draws_it() {
    let source = page_showing("BT /F1 12 Tf 10 150 Td (AAAA) Tj ET");
    let enter = |provider: Arc<dyn FontProvider>| {
        let mut session = Session::with_fonts(source.clone(), b"", Some(provider));
        let page = session.page(0).expect("the page reads");
        let command = Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            frame_declared: true,
            range: pdf_edit::BlockRange::Between {
                from: (0, 2),
                to: (0, 2),
            },
            text: "\n".to_owned(),
        };
        let planned = session.plan(&command);
        (session, planned)
    };

    let lined: Arc<dyn FontProvider> = Arc::new(NamedFace {
        program: lined_square_face(),
        family: "Lined Square",
        sha256: "3".repeat(64).leak(),
    });
    let (mut session, planned) = enter(lined);
    session
        .apply(planned.expect("the face's line lays the block out"))
        .expect("applies");
    let after = session.page(0).expect("the edited page reads");
    let found = pens(&after);
    let wanted = [(10.0, 150.0), (16.0, 150.0), (10.0, 138.0), (16.0, 138.0)];
    assert_eq!(found.len(), wanted.len(), "{found:?}");
    for (one, other) in found.iter().zip(wanted) {
        assert!(
            (one.0 - other.0).abs() < 1e-6 && (one.1 - other.1).abs() < 1e-6,
            "{found:?}"
        );
    }

    let (control_session, refused) = enter(large());
    let (mut session, planned) = (control_session, refused);
    session
        .apply(planned.expect("control: the default pitch lays the block out"))
        .expect("applies");
    let found = pens(&session.page(0).expect("the edited page reads"));
    let wanted = [(10.0, 150.0), (16.0, 150.0), (10.0, 135.6), (16.0, 135.6)];
    assert_eq!(found.len(), wanted.len(), "{found:?}");
    for (one, other) in found.iter().zip(wanted) {
        assert!(
            (one.0 - other.0).abs() < 1e-6 && (one.1 - other.1).abs() < 1e-6,
            "{found:?}"
        );
    }
}

fn embedded_page_showing(content: &str, program: &[u8]) -> ByteStore {
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
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
    );
    object(
        &mut bytes,
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
    );
    let mut stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
    stream.extend_from_slice(content.as_bytes());
    stream.extend_from_slice(b"\nendstream");
    object(&mut bytes, &stream);
    object(
        &mut bytes,
        format!(
            "<< /Type /Font /Subtype /TrueType /BaseFont /Test /Encoding /WinAnsiEncoding \
             /FirstChar 65 /LastChar 126 /Widths [{}] /FontDescriptor 6 0 R >>",
            (65..=126)
                .map(|code| if code == 124 { "10" } else { "500" })
                .collect::<Vec<_>>()
                .join(" ")
        )
        .as_bytes(),
    );
    object(
        &mut bytes,
        b"<< /Type /FontDescriptor /FontName /Test /Flags 32 /ItalicAngle 0 \
          /Ascent 0 /Descent 0 /FontFile2 7 0 R >>",
    );
    let mut stream = format!("<< /Length {} >>\nstream\n", program.len()).into_bytes();
    stream.extend_from_slice(program);
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
    ByteStore::new(SourceId::new(773), Arc::<[u8]>::from(bytes))
}

#[test]
fn an_embedded_subset_with_no_stated_line_takes_the_line_of_its_own_family() {
    let program = square_face(500).program_bytes().to_vec();
    let source = embedded_page_showing("BT /F1 12 Tf 10 150 Td (AAAA) Tj ET", &program);
    let enter = |family: &'static str| {
        let provider: Arc<dyn FontProvider> = Arc::new(NamedFace {
            program: lined_square_face(),
            family,
            sha256: "4".repeat(64).leak(),
        });
        let mut session = Session::with_fonts(source.clone(), b"", Some(provider));
        let page = session.page(0).expect("the page reads");
        let command = Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            frame_declared: true,
            range: pdf_edit::BlockRange::Between {
                from: (0, 2),
                to: (0, 2),
            },
            text: "\n".to_owned(),
        };
        let planned = session.plan(&command);
        (session, planned)
    };

    let (mut session, planned) = enter("Test");
    session
        .apply(planned.expect("the family's line lays the block out"))
        .expect("applies");
    let after = session.page(0).expect("the edited page reads");
    let found = pens(&after);
    let wanted = [(10.0, 150.0), (16.0, 150.0), (10.0, 138.0), (16.0, 138.0)];
    assert_eq!(found.len(), wanted.len(), "{found:?}");
    for (one, other) in found.iter().zip(wanted) {
        assert!(
            (one.0 - other.0).abs() < 1e-6 && (one.1 - other.1).abs() < 1e-6,
            "{found:?}"
        );
    }

    let (control_session, refused) = enter("Other");
    let (mut session, planned) = (control_session, refused);
    session
        .apply(planned.expect("control: the default pitch lays the block out"))
        .expect("applies");
    let found = pens(&session.page(0).expect("the edited page reads"));
    let wanted = [(10.0, 150.0), (16.0, 150.0), (10.0, 135.6), (16.0, 135.6)];
    assert_eq!(found.len(), wanted.len(), "{found:?}");
    for (one, other) in found.iter().zip(wanted) {
        assert!(
            (one.0 - other.0).abs() < 1e-6 && (one.1 - other.1).abs() < 1e-6,
            "{found:?}"
        );
    }
}

#[test]
fn a_letter_the_file_moves_back_under_the_next_stays_on_its_line() {
    let program = square_face_for(500, &[('A', 1), ('|', 1)])
        .program_bytes()
        .to_vec();
    let enter = |back: u32| {
        let source = embedded_page_showing(
            &format!("BT /F1 12 Tf 60 150 Td [(A|) {back} (AA)] TJ ET"),
            &program,
        );
        let provider: Arc<dyn FontProvider> = Arc::new(NamedFace {
            program: lined_square_face(),
            family: "Test",
            sha256: "5".repeat(64).leak(),
        });
        let mut session = Session::with_fonts(source, b"", Some(provider));
        let page = session.page(0).expect("the page reads");
        let command = Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (60.0, 190.0),
            edges: (0, 0),
            breaks: None,
            frame_declared: true,
            range: pdf_edit::BlockRange::Between {
                from: (0, 1),
                to: (0, 1),
            },
            text: "\n".to_owned(),
        };
        let planned = session.plan(&command);
        (session, planned)
    };

    let (mut session, planned) = enter(20);
    session
        .apply(planned.expect("the letter and its mark lay out together"))
        .expect("applies");
    let after = session.page(0).expect("the edited page reads");
    let found = pens(&after);
    let wanted = [(60.0, 150.0), (60.0, 138.0), (59.88, 138.0), (65.88, 138.0)];
    assert_eq!(found.len(), wanted.len(), "{found:?}");
    for (one, other) in found.iter().zip(wanted) {
        assert!(
            (one.0 - other.0).abs() < 1e-6 && (one.1 - other.1).abs() < 1e-6,
            "{found:?}"
        );
    }
}

fn square_face_with_advance(
    size: i16,
    characters: &[(char, u16)],
    advance: u16,
) -> Arc<GlyphProgram> {
    let plain = square_face_for(size, characters);
    let bytes = plain.program_bytes();
    let count = usize::from(u16::from_be_bytes([bytes[4], bytes[5]]));
    let mut tables: Vec<(&[u8; 4], Vec<u8>)> = Vec::with_capacity(count + 2);
    let tags: [&[u8; 4]; 5] = [b"cmap", b"glyf", b"head", b"loca", b"maxp"];
    for index in 0..count {
        let record = &bytes[12 + index * 16..28 + index * 16];
        let offset = usize::try_from(u32::from_be_bytes([
            record[8], record[9], record[10], record[11],
        ]))
        .expect("offset");
        let length = usize::try_from(u32::from_be_bytes([
            record[12], record[13], record[14], record[15],
        ]))
        .expect("length");
        let tag = tags
            .into_iter()
            .find(|tag| tag[..] == record[..4])
            .expect("a table square_face_for writes");
        tables.push((tag, bytes[offset..offset + length].to_vec()));
    }
    let mut hhea = vec![0_u8; 36];
    hhea[0..4].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
    hhea[4..6].copy_from_slice(&800_i16.to_be_bytes());
    hhea[6..8].copy_from_slice(&(-200_i16).to_be_bytes());
    let glyphs = tables
        .iter()
        .find(|(tag, _)| tag[..] == b"maxp"[..])
        .map(|(_, maxp)| u16::from_be_bytes([maxp[4], maxp[5]]))
        .expect("maxp");
    hhea[34..36].copy_from_slice(&glyphs.to_be_bytes());
    let hmtx: Vec<u8> = [0_u16, 0]
        .into_iter()
        .chain((1..glyphs).flat_map(|_| [advance, 0]))
        .flat_map(u16::to_be_bytes)
        .collect();
    let at = tables
        .iter()
        .position(|(tag, _)| tag[..] == b"loca"[..])
        .expect("loca");
    tables.insert(at, (b"hmtx", hmtx));
    tables.insert(at, (b"hhea", hhea));
    Arc::new(GlyphProgram::parse(sfnt(&tables)).expect("the face parses"))
}

#[derive(Debug)]
struct Covering {
    covering: Option<Arc<GlyphProgram>>,
}

impl FontProvider for Covering {
    fn primary_face(&self, request: &FontRequest) -> Option<SubstitutedFace> {
        large().primary_face(request)
    }
    fn fallback_face(&self, _: &FontRequest, _: char) -> Option<SubstitutedFace> {
        Some(SubstitutedFace {
            program: Arc::clone(self.covering.as_ref()?),
            identity: Arc::new(pdf_content::FaceIdentity {
                family: "Covering".to_owned(),
                subfamily: "Regular".to_owned(),
                origin: "test:covering".to_owned(),
                sha256: "3".repeat(64),
                face_index: 0,
                style: pdf_content::FontStyle::default(),
            }),
            reason: SubstitutionReason::ScriptCoverage,
        })
    }
    fn description(&self) -> String {
        "test covering face".to_owned()
    }
}

#[test]
fn a_line_typed_in_a_taller_face_is_as_tall_as_that_faces_line() {
    let source = page_showing("BT /F1 12 Tf 10 TL 10 150 Td (AA) Tj ET");
    let provider: Arc<dyn FontProvider> = Arc::new(Covering {
        covering: Some(square_face_with_advance(300, &[('B', 1)], 600)),
    });
    let mut session = Session::with_fonts(source, b"", Some(provider));
    let page = session.page(0).expect("the page reads");
    let plan = session
        .plan(&Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            frame_declared: true,
            range: pdf_edit::BlockRange::Between {
                from: (0, 2),
                to: (0, 2),
            },
            text: "B\nB".to_owned(),
        })
        .expect("typed with a paragraph break");
    session.apply(plan).expect("applies");
    assert_pens(
        &pens(&session.page(0).expect("the page reads")),
        &[(10.0, 150.0), (16.0, 150.0), (22.0, 150.0), (10.0, 138.0)],
    );
}

#[test]
fn a_vowel_sign_typed_after_its_letter_is_shaped_with_it() {
    let source = page_showing("BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET");
    let provider: Arc<dyn FontProvider> = Arc::new(Covering {
        covering: Some(square_face_with_advance(
            300,
            &[('ક', 1), ('\u{0AC1}', 2), ('\u{25CC}', 3)],
            600,
        )),
    });
    let mut session = Session::with_fonts(source, b"", Some(provider));
    for (stop, typed) in [(2, "ક"), (3, "\u{0AC1}")] {
        let page = session.page(0).expect("the page reads");
        let plan = session
            .plan(&Command::RewriteBlock {
                paragraph: pdf_edit::ParagraphLayout::default(),
                page_index: 0,
                rows: first_block_rows(&page),
                frame: (10.0, 190.0),
                edges: (0, 0),
                breaks: None,
                frame_declared: true,
                range: pdf_edit::BlockRange::Between {
                    from: (0, stop),
                    to: (0, stop),
                },
                text: typed.to_owned(),
            })
            .expect("typed");
        session.apply(plan).expect("applies");
    }
    let after = session.page(0).expect("the edited page reads");
    let (read, embedded) = read_and_embedded(&after);
    assert_eq!(read, "AAક\u{0AC1}AA");
    assert_eq!(embedded.len(), 2, "{embedded:?}");
    assert_one_block_holds_every_glyph(&after);
}

fn assert_one_block_holds_every_glyph(page: &PageView) {
    let glyphs: usize = page
        .graph
        .atoms
        .iter()
        .filter_map(|atom| match &atom.kind {
            pdf_paint::PaintAtomKind::Text(text) => Some(text.glyphs.len()),
            _ => None,
        })
        .sum();
    let held: usize = page.index.blocks[0]
        .lines
        .iter()
        .flat_map(|line| &page.index.lines[*line].clusters)
        .map(|cluster| page.index.clusters[*cluster].glyphs.len())
        .sum();
    assert_eq!(page.index.blocks.len(), 1);
    assert_eq!(held, glyphs);
}

#[test]
fn a_second_sign_typed_after_a_letter_and_its_first_is_shaped_with_both() {
    let source = page_showing("BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET");
    let provider: Arc<dyn FontProvider> = Arc::new(Covering {
        covering: Some(square_face_with_advance(
            300,
            &[('ક', 1), ('\u{0ABC}', 2), ('\u{0AC1}', 3), ('\u{25CC}', 4)],
            600,
        )),
    });
    let mut session = Session::with_fonts(source, b"", Some(provider));
    for (stop, typed) in [(2, "ક"), (3, "\u{0ABC}"), (3, "\u{0AC1}")] {
        let page = session.page(0).expect("the page reads");
        let plan = session
            .plan(&Command::RewriteBlock {
                paragraph: pdf_edit::ParagraphLayout::default(),
                page_index: 0,
                rows: first_block_rows(&page),
                frame: (10.0, 190.0),
                edges: (0, 0),
                breaks: None,
                frame_declared: true,
                range: pdf_edit::BlockRange::Between {
                    from: (0, stop),
                    to: (0, stop),
                },
                text: typed.to_owned(),
            })
            .unwrap_or_else(|error| panic!("{typed:?} typed: {error:?}"));
        session.apply(plan).expect("applies");
    }
    let after = session.page(0).expect("the edited page reads");
    let (read, _) = read_and_embedded(&after);
    assert_eq!(read, "AAક\u{0ABC}\u{0AC1}AA");
    assert_one_block_holds_every_glyph(&after);
}

#[test]
fn a_character_typed_after_one_a_face_wrote_is_written_in_the_blocks_font_when_it_has_it() {
    let source = page_showing("BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET");
    let provider: Arc<dyn FontProvider> = Arc::new(Covering {
        covering: Some(square_face_with_advance(300, &[('ລ', 1), ('A', 1)], 600)),
    });
    let mut session = Session::with_fonts(source, b"", Some(provider));
    for (stop, typed) in [(2, "ລ"), (3, "A")] {
        let page = session.page(0).expect("the page reads");
        let plan = session
            .plan(&Command::RewriteBlock {
                paragraph: pdf_edit::ParagraphLayout::default(),
                page_index: 0,
                rows: first_block_rows(&page),
                frame: (10.0, 190.0),
                edges: (0, 0),
                breaks: None,
                frame_declared: true,
                range: pdf_edit::BlockRange::Between {
                    from: (0, stop),
                    to: (0, stop),
                },
                text: typed.to_owned(),
            })
            .expect("typed");
        session.apply(plan).expect("applies");
    }
    let after = session.page(0).expect("the edited page reads");
    assert_pens(
        &pens(&after),
        &[
            (10.0, 150.0),
            (16.0, 150.0),
            (22.0, 150.0),
            (29.2, 150.0),
            (35.2, 150.0),
            (41.2, 150.0),
        ],
    );
    let (read, embedded) = read_and_embedded(&after);
    assert_eq!(read, "AAລAAA");
    assert_eq!(embedded, [("/F2".to_owned(), Some(1))]);
}

#[test]
fn a_letter_typed_in_a_taller_face_takes_that_faces_own_line_unscaled() {
    let source = page_with_objects(
        "BT /F1 12 Tf 12 TL 10 150 Td (AA) Tj T* (AA) Tj ET",
        "",
        "<< /Type /Font /Subtype /TrueType /BaseFont /Test /Encoding /WinAnsiEncoding \
         /FirstChar 65 /LastChar 65 /Widths [500] /FontDescriptor 6 0 R >>",
        &[
            "<< /Type /FontDescriptor /FontName /Test /Flags 32 /Ascent 500 /Descent -100 \
           /ItalicAngle 0 /StemV 80 /CapHeight 500 /FontBBox [0 -100 500 500] >>",
        ],
    );
    let provider: Arc<dyn FontProvider> = Arc::new(Covering {
        covering: Some(square_face_with_advance(300, &[('B', 1)], 600)),
    });
    let mut session = Session::with_fonts(source, b"", Some(provider));
    let page = session.page(0).expect("the page reads");
    let plan = session
        .plan(&Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            frame_declared: true,
            range: pdf_edit::BlockRange::Between {
                from: (1, 0),
                to: (1, 0),
            },
            text: "B".to_owned(),
        })
        .expect("typed");
    session.apply(plan).expect("applies");
    assert_pens(
        &pens(&session.page(0).expect("the page reads")),
        &[
            (10.0, 150.0),
            (16.0, 150.0),
            (10.0, 138.0),
            (17.2, 138.0),
            (23.2, 138.0),
        ],
    );
}

#[test]
fn a_block_rewritten_inside_an_actual_text_span_leaves_the_span_without_it() {
    let source = page_showing(
        "BT /F1 12 Tf 14 TL 10 150 Td /Span <</ActualText (X)>> BDC (AA) Tj EMC (AA) Tj ET",
    );
    let provider: Arc<dyn FontProvider> = Arc::new(Covering { covering: None });
    let mut session = Session::with_fonts(source, b"", Some(provider));
    let page = session.page(0).expect("the page reads");
    let plan = session
        .plan(&Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            frame_declared: true,
            range: pdf_edit::BlockRange::Between {
                from: (0, 4),
                to: (0, 4),
            },
            text: "A".to_owned(),
        })
        .expect("typed");
    session.apply(plan).expect("applies");
    let after = session.page(0).expect("the edited page reads");
    let (read, _) = read_and_embedded(&after);
    assert_eq!(read, "AAAAA");
    for atom in &after.graph.atoms {
        if matches!(atom.kind, pdf_paint::PaintAtomKind::Text(_)) {
            assert!(
                atom.marks.iter().all(|mark| mark.properties.is_none()),
                "{:?}",
                atom.marks
            );
        }
    }
}

#[test]
fn a_character_the_blocks_font_gives_no_width_is_typed_in_an_embedded_face() {
    let source = page_showing("BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET");
    let provider: Arc<dyn FontProvider> = Arc::new(Covering {
        covering: Some(square_face_with_advance(300, &[('B', 1)], 600)),
    });
    let mut session = Session::with_fonts(source, b"", Some(provider));
    let page = session.page(0).expect("the page reads");
    let plan = session
        .plan(&Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            frame_declared: true,
            range: pdf_edit::BlockRange::Between {
                from: (0, 2),
                to: (0, 2),
            },
            text: "B".to_owned(),
        })
        .expect("the character is typed");
    session.apply(plan).expect("applies");
    let after = session.page(0).expect("the edited page reads");
    assert_pens(
        &pens(&after),
        &[
            (10.0, 150.0),
            (16.0, 150.0),
            (22.0, 150.0),
            (29.2, 150.0),
            (35.2, 150.0),
        ],
    );
    let (read, _) = read_and_embedded(&after);
    assert_eq!(read, "AABAA");
}

#[test]
fn a_block_whose_font_has_no_space_keeps_its_rows_and_is_typed_into() {
    let source = page_showing("BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj T* (AA) Tj ET");
    let mut session = Session::with_fonts(source, b"", Some(large()));
    let page = session.page(0).expect("the page reads");
    let plan = session
        .plan(&Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            frame_declared: true,
            range: pdf_edit::BlockRange::Between {
                from: (0, 4),
                to: (0, 4),
            },
            text: "A".to_owned(),
        })
        .expect("typed at the first row's end");
    session.apply(plan).expect("applies");
    assert_pens(
        &pens(&session.page(0).expect("the page reads")),
        &[
            (10.0, 150.0),
            (16.0, 150.0),
            (22.0, 150.0),
            (28.0, 150.0),
            (34.0, 150.0),
            (10.0, 136.0),
            (16.0, 136.0),
        ],
    );
}

fn type3_page(content: &str) -> ByteStore {
    type3_page_described(content, "")
}

fn type3_page_described(content: &str, extra: &str) -> ByteStore {
    let procedure = "500 0 0 0 500 700 d1 0 0 500 700 re f";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>"
            .into(),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ),
        "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 500 700] \
         /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /A 6 0 R >> \
         /Encoding << /Type /Encoding /Differences [65 /A] >> \
         /FirstChar 65 /LastChar 65 /Widths [500] /Resources << >> {extra} >>"
            .replace("{extra}", extra),
        format!(
            "<< /Length {} >>\nstream\n{procedure}\nendstream",
            procedure.len()
        ),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 7\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size 7 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    ByteStore::new(SourceId::new(773), Arc::<[u8]>::from(bytes))
}

#[test]
fn a_block_in_a_type3_font_is_styled_and_typed_into() {
    let source = type3_page("BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET");
    let mut session = Session::with_fonts(source, b"", Some(large()));
    let page = session.page(0).expect("the page reads");
    let plan = session
        .plan(&Command::StyleBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            range: pdf_edit::BlockRange::Between {
                from: (0, 0),
                to: (0, 4),
            },
            style: pdf_edit::TextStyle {
                fill: Some([1.0, 0.0, 0.0]),
                ..pdf_edit::TextStyle::default()
            },
        })
        .expect("a Type 3 block is set red");
    session.apply(plan).expect("applies");
    let after = session.page(0).expect("the page reads");
    assert_pens(
        &pens(&after),
        &[(10.0, 150.0), (16.0, 150.0), (22.0, 150.0), (28.0, 150.0)],
    );
    let plan = session
        .plan(&Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&after),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            frame_declared: true,
            range: pdf_edit::BlockRange::Between {
                from: (0, 4),
                to: (0, 4),
            },
            text: "A".to_owned(),
        })
        .expect("typed into a Type 3 block");
    session.apply(plan).expect("applies");
    assert_pens(
        &pens(&session.page(0).expect("the page reads")),
        &[
            (10.0, 150.0),
            (16.0, 150.0),
            (22.0, 150.0),
            (28.0, 150.0),
            (34.0, 150.0),
        ],
    );
}

#[test]
fn a_turned_block_in_a_type3_font_inside_a_clip_is_typed_into() {
    let source = type3_page(
        "0 0 200 200 re W n BT /F1 12 Tf 14 TL \
         0.9396926 0.3420201 -0.3420201 0.9396926 20 150 Tm (AAAA) Tj ET",
    );
    let mut session = Session::with_fonts(source, b"", Some(large()));
    let page = session.page(0).expect("the page reads");
    let plan = session
        .plan(&Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (0.0, 180.0),
            edges: (0, 0),
            breaks: None,
            frame_declared: true,
            range: pdf_edit::BlockRange::Between {
                from: (0, 4),
                to: (0, 4),
            },
            text: "A".to_owned(),
        })
        .expect("typed into a turned Type 3 block");
    session.apply(plan).expect("applies");
    let after = pens(&session.page(0).expect("the page reads"));
    let last = after.last().expect("a glyph");
    assert!(
        (last.0 - 42.553).abs() < 1e-3 && (last.1 - 158.209).abs() < 1e-3,
        "{after:?}"
    );
}

#[test]
fn a_character_a_type3_font_lacks_is_typed_in_a_face_its_descriptor_describes() {
    let source = type3_page_described(
        "BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET",
        "/FontDescriptor << /Type /FontDescriptor /FontName /BAAAAA+NotoSansThai-Regular \
         /FontFamily (Noto Sans Thai) /FontWeight 600 /Flags 4 >>",
    );
    let provider: Arc<dyn FontProvider> = Arc::new(Covering {
        covering: Some(square_face_with_advance(300, &[('ລ', 1)], 600)),
    });
    let mut session = Session::with_fonts(source, b"", Some(provider));
    let page = session.page(0).expect("the page reads");
    let resource = page
        .program
        .resources
        .fonts()
        .first()
        .expect("the page has its font");
    assert!(resource.font_request().expect("reads").is_none());
    let request = resource
        .face_request()
        .expect("reads")
        .expect("a Type 3 font with a descriptor asks for a face");
    assert_eq!(request.family, "Noto Sans Thai");
    assert_eq!(request.style.weight, 600);
    let plan = session
        .plan(&Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            frame_declared: true,
            range: pdf_edit::BlockRange::Between {
                from: (0, 2),
                to: (0, 2),
            },
            text: "ລ".to_owned(),
        })
        .expect("typed in a face from the machine");
    session.apply(plan).expect("applies");
    assert_pens(
        &pens(&session.page(0).expect("the page reads")),
        &[
            (10.0, 150.0),
            (16.0, 150.0),
            (22.0, 150.0),
            (29.2, 150.0),
            (35.2, 150.0),
        ],
    );
}

#[test]
fn a_space_typed_into_a_run_with_word_spacing_takes_its_own_width() {
    let mut widths = vec!["250".to_owned()];
    widths.extend(std::iter::repeat_n("0".to_owned(), 32));
    widths.push("500".to_owned());
    let font = format!(
        "<< /Type /Font /Subtype /TrueType /BaseFont /Test /Encoding /WinAnsiEncoding \
         /FirstChar 32 /LastChar 65 /Widths [{}] >>",
        widths.join(" ")
    );
    let source = page_with_font("BT /F1 12 Tf 14 TL 5 Tw 10 150 Td (AAAA) Tj ET", "", &font);
    let mut session = Session::with_fonts(source, b"", Some(large()));
    let page = session.page(0).expect("the page reads");
    let plan = session
        .plan(&Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            frame_declared: true,
            range: pdf_edit::BlockRange::Between {
                from: (0, 4),
                to: (0, 4),
            },
            text: " A".to_owned(),
        })
        .expect("typed");
    session.apply(plan).expect("applies");
    let after = session.page(0).expect("the page reads");
    let letters: Vec<(f64, f64)> = pens(&after)
        .into_iter()
        .filter(|pen| (pen.1 - 150.0).abs() < 1e-6)
        .collect();
    assert_pens(
        &letters,
        &[
            (10.0, 150.0),
            (16.0, 150.0),
            (22.0, 150.0),
            (28.0, 150.0),
            (34.0, 150.0),
            (37.0, 150.0),
        ],
    );
}

#[test]
fn a_character_the_blocks_font_lacks_is_typed_in_an_embedded_face() {
    let source = page_with_resources(
        "BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET /CS0 cs 1 scn BT /F1 12 Tf 10 40 Td (AA) Tj ET",
        SPOT,
    );
    let face = square_face_with_advance(300, &[('ລ', 1)], 600);
    let command = |session: &mut Session| {
        let page = session.page(0).expect("the page reads");
        Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            frame_declared: true,
            range: pdf_edit::BlockRange::Between {
                from: (0, 2),
                to: (0, 2),
            },
            text: "ລ".to_owned(),
        }
    };

    let provider: Arc<dyn FontProvider> = Arc::new(Covering {
        covering: Some(face),
    });
    let mut session = Session::with_fonts(source.clone(), b"", Some(provider));
    let rewrite = command(&mut session);
    let plan = session.plan(&rewrite).expect("the character is typed");
    session.apply(plan).expect("applies");
    let after = session.page(0).expect("the edited page reads");
    assert_pens(
        &pens(&after),
        &[
            (10.0, 150.0),
            (16.0, 150.0),
            (22.0, 150.0),
            (29.2, 150.0),
            (35.2, 150.0),
            (10.0, 40.0),
            (16.0, 40.0),
        ],
    );
    let mut read = String::new();
    let mut embedded = Vec::new();
    for atom in &after.graph.atoms {
        let pdf_paint::PaintAtomKind::Text(text) = &atom.kind else {
            continue;
        };
        let font = text.state.text.font.as_ref().expect("a font");
        for glyph in &text.glyphs {
            let code = pdf_content::Code {
                value: glyph.code.value,
                byte_len: glyph.code.bytes.len(),
            };
            read.push_str(&text.text.text_of(code).expect("the code reads").text);
            if text.program.is_some() {
                embedded.push((
                    String::from_utf8_lossy(&font.value.name).into_owned(),
                    glyph.glyph,
                ));
            }
        }
    }
    assert_eq!(read, "AAລAAAA");
    assert_eq!(embedded, [("/F2".to_owned(), Some(1))]);
    let written = session.source().as_bytes();
    for ours in [&b"PanPDF"[..], b"/Pan"] {
        assert!(
            !written.windows(ours.len()).any(|window| window == ours),
            "{}",
            String::from_utf8_lossy(ours)
        );
    }
    session.undo().expect("undoes");
    let undone = session.page(0).expect("the page reads");
    assert_pens(
        &pens(&undone),
        &[
            (10.0, 150.0),
            (16.0, 150.0),
            (22.0, 150.0),
            (28.0, 150.0),
            (10.0, 40.0),
            (16.0, 40.0),
        ],
    );

    let provider: Arc<dyn FontProvider> = Arc::new(Covering { covering: None });
    let mut bare = Session::with_fonts(source, b"", Some(provider));
    let rewrite = command(&mut bare);
    let refused = bare
        .plan(&rewrite)
        .expect_err("control: no face is offered");
    assert!(
        refused.to_string().contains("no face on this machine"),
        "control: {refused}"
    );
}

#[test]
fn a_mark_the_shaper_stacks_is_shown_at_its_own_rise() {
    let thai = Arc::new(
        GlyphProgram::parse(
            include_bytes!("../../../fonts/packaged/NotoSansThai-Regular.ttf").to_vec(),
        )
        .expect("the packaged face parses"),
    );
    let source = page_showing("BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET");
    let provider: Arc<dyn FontProvider> = Arc::new(Covering {
        covering: Some(thai),
    });
    let mut session = Session::with_fonts(source, b"", Some(provider));
    let page = session.page(0).expect("the page reads");
    let command = Command::RewriteBlock {
        paragraph: pdf_edit::ParagraphLayout::default(),
        page_index: 0,
        rows: first_block_rows(&page),
        frame: (10.0, 190.0),
        edges: (0, 0),
        breaks: None,
        frame_declared: true,
        range: pdf_edit::BlockRange::Between {
            from: (0, 2),
            to: (0, 2),
        },
        text: "ปั๊".to_owned(),
    };
    let plan = session
        .plan(&command)
        .expect("the stacked cluster is typed");
    session.apply(plan).expect("applies");
    let after = session.page(0).expect("the edited page reads");
    let mut glyphs = Vec::new();
    for atom in &after.graph.atoms {
        let pdf_paint::PaintAtomKind::Text(text) = &atom.kind else {
            continue;
        };
        for glyph in &text.glyphs {
            let pen = text.state.ctm.value.transform(pdf_paint::Point {
                x: glyph.text_matrix.e,
                y: glyph.text_matrix.f,
            });
            let code = pdf_content::Code {
                value: glyph.code.value,
                byte_len: glyph.code.bytes.len(),
            };
            let read = text
                .text
                .text_of(code)
                .map(|meaning| meaning.text.clone())
                .unwrap_or_default();
            glyphs.push((read, pen.x, text.state.text.rise.value));
        }
    }
    let wanted = [
        ("A", 10.0, 0.0),
        ("A", 16.0, 0.0),
        ("ป", 22.0, 0.0),
        ("ั", 29.248, 0.0),
        ("๊", 27.004, -0.48),
        ("A", 29.26, 0.0),
        ("A", 35.26, 0.0),
    ];
    assert_eq!(glyphs.len(), wanted.len(), "{glyphs:?}");
    for ((read, x, rise), (text, at, lifted)) in glyphs.iter().zip(wanted) {
        assert!(
            read == text && (x - at).abs() < 1e-6 && (rise - lifted).abs() < 1e-9,
            "{glyphs:?}"
        );
    }
}

#[test]
fn a_typed_cluster_written_across_two_shows_is_named_in_its_block_by_both() {
    let thai = Arc::new(
        GlyphProgram::parse(
            include_bytes!("../../../fonts/packaged/NotoSansThai-Regular.ttf").to_vec(),
        )
        .expect("the packaged face parses"),
    );
    let source = page_showing("BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET");
    let provider: Arc<dyn FontProvider> = Arc::new(Covering {
        covering: Some(thai),
    });
    let mut session = Session::with_fonts(source, b"", Some(provider));
    let page = session.page(0).expect("the page reads");
    let plan = session
        .plan(&Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            frame_declared: true,
            range: pdf_edit::BlockRange::Between {
                from: (0, 2),
                to: (0, 2),
            },
            text: "ปั๊".to_owned(),
        })
        .expect("the stacked cluster is typed");
    let preview = session.preview(&plan).expect("the candidate reads");
    let raised = preview
        .graph
        .atoms
        .iter()
        .position(|atom| {
            matches!(&atom.kind, pdf_paint::PaintAtomKind::Text(text)
                if text.state.text.rise.value != 0.0)
        })
        .expect("a show at a rise");
    assert!(
        plan.inserted().iter().any(|(_, key)| key.atom == raised),
        "{:?}",
        plan.inserted()
    );
    session.apply(plan).expect("applies");
    assert_one_block_holds_every_glyph(&session.page(0).expect("the edited page reads"));
}

#[test]
fn a_caret_after_a_cluster_written_across_two_shows_is_named_by_the_last() {
    let thai = Arc::new(
        GlyphProgram::parse(
            include_bytes!("../../../fonts/packaged/NotoSansThai-Regular.ttf").to_vec(),
        )
        .expect("the packaged face parses"),
    );
    let source = page_showing("BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET");
    let provider: Arc<dyn FontProvider> = Arc::new(Covering {
        covering: Some(thai),
    });
    let mut session = Session::with_fonts(source, b"", Some(provider));
    let page = session.page(0).expect("the page reads");
    let plan = session
        .plan(&Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            frame_declared: true,
            range: pdf_edit::BlockRange::Between {
                from: (0, 2),
                to: (0, 2),
            },
            text: "ปั๊".to_owned(),
        })
        .expect("the stacked cluster is typed");
    let preview = session.preview(&plan).expect("the candidate reads");
    let raised = preview
        .graph
        .atoms
        .iter()
        .position(|atom| {
            matches!(&atom.kind, pdf_paint::PaintAtomKind::Text(text)
                if text.state.text.rise.value != 0.0)
        })
        .expect("a show at a rise");
    let caret = plan.block().and_then(|outcome| outcome.caret);
    assert!(
        matches!(
            caret,
            Some(pdf_edit::PlannedCaret::Beside { cluster, after: true }) if cluster.atom == raised
        ),
        "{caret:?}, raised show {raised}"
    );
}

#[test]
fn a_letter_outlined_by_a_second_show_stays_one_letter_and_takes_the_new_colour() {
    let source = page_showing(
        "BT /F1 12 Tf 14 TL 1 0 0 1 10 150 Tm (A) Tj 1 Tr 1 0 0 1 10 150 Tm (A) Tj \
         0 Tr 1 0 0 1 16 150 Tm (AA) Tj ET",
    );
    let mut session = Session::with_fonts(source, b"", Some(large()));
    let style_all = |session: &mut Session, style: pdf_edit::TextStyle| {
        let page = session.page(0).expect("the page reads");
        let command = Command::StyleBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            range: pdf_edit::BlockRange::Between {
                from: (0, 0),
                to: (0, 4),
            },
            style,
        };
        session.plan(&command)
    };
    let red = pdf_edit::TextStyle {
        fill: Some([1.0, 0.0, 0.0]),
        ..pdf_edit::TextStyle::default()
    };
    let plan = style_all(&mut session, red).expect("the block is set red");
    session.apply(plan).expect("applies");
    let coloured = session.page(0).expect("the page reads");
    let set_red = [(10.0, 150.0), (10.0, 150.0), (16.0, 150.0), (22.0, 150.0)];
    assert_pens(&pens(&coloured), &set_red);
    let signature = pdf_paint::colour_signature(&pdf_paint::Color::DeviceRgb(1.0, 0.0, 0.0));
    let painted = inks(&coloured);
    assert!(
        painted
            .iter()
            .all(|ink| ink.contains(&format!("fill {signature} "))),
        "{painted:?}"
    );
    let outline = painted
        .iter()
        .find(|ink| ink.contains(" Stroke "))
        .expect("the outline is still stroked");
    assert!(
        outline.ends_with(&format!("stroke {signature}")),
        "{outline}"
    );

    let larger = pdf_edit::TextStyle {
        size: Some(24.0),
        ..pdf_edit::TextStyle::default()
    };
    let plan = style_all(&mut session, larger).expect("the block is set larger");
    session.apply(plan).expect("applies");
    let grown = session.page(0).expect("the page reads");
    assert_pens(
        &pens(&grown),
        &[(10.0, 150.0), (10.0, 150.0), (22.0, 150.0), (34.0, 150.0)],
    );
    session.undo().expect("undoes");
    assert_pens(&pens(&session.page(0).expect("the page reads")), &set_red);
}

#[test]
fn a_glyph_the_file_draws_nearer_than_the_width_before_it_stays_where_it_was() {
    let source = page_showing(
        "BT /F1 12 Tf 14 TL 1 Tc 1 0 0 1 10 150 Tm (AA) Tj 0 Tc 1 0 0 1 18 153 Tm (A) Tj \
         1 0 0 1 26 150 Tm (A) Tj ET",
    );
    let mut session = Session::with_fonts(source, b"", Some(large()));
    let style_all = |session: &mut Session, style: pdf_edit::TextStyle| {
        let page = session.page(0).expect("the page reads");
        let rows = first_block_rows(&page);
        let end = rows[0].len();
        let command = Command::StyleBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows,
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            range: pdf_edit::BlockRange::Between {
                from: (0, 0),
                to: (0, end),
            },
            style,
        };
        session.plan(&command)
    };
    let drawn = |page: &PageView| -> Vec<(f64, f64)> {
        let mut out = Vec::new();
        for atom in &page.graph.atoms {
            let pdf_paint::PaintAtomKind::Text(text) = &atom.kind else {
                continue;
            };
            for glyph in &text.glyphs {
                let pen = text.state.ctm.value.transform(pdf_paint::Point {
                    x: glyph.text_matrix.e,
                    y: glyph.text_matrix.f + text.state.text.rise.value * glyph.text_matrix.d,
                });
                out.push((pen.x, pen.y));
            }
        }
        out
    };
    let red = pdf_edit::TextStyle {
        fill: Some([1.0, 0.0, 0.0]),
        ..pdf_edit::TextStyle::default()
    };
    let plan = style_all(&mut session, red).expect("the block is set red");
    session.apply(plan).expect("applies");
    let coloured = session.page(0).expect("the page reads");
    assert_pens(
        &drawn(&coloured),
        &[(10.0, 150.0), (17.0, 150.0), (18.0, 153.0), (26.0, 150.0)],
    );
    let written = String::from_utf8_lossy(coloured.program.streams[0].bytes.as_bytes());
    assert!(!written.contains(" <>] TJ"), "{written}");
    let larger = pdf_edit::TextStyle {
        size: Some(24.0),
        ..pdf_edit::TextStyle::default()
    };
    let plan = style_all(&mut session, larger).expect("the block is set larger");
    session.apply(plan).expect("applies");
    assert_pens(
        &drawn(&session.page(0).expect("the page reads")),
        &[(10.0, 150.0), (24.0, 150.0), (26.0, 156.0), (42.0, 150.0)],
    );
}

#[test]
fn a_word_the_file_shows_a_tab_or_a_justified_space_on_stays_where_it_was() {
    let source = page_showing(
        "BT /F1 12 Tf 14 TL 1 0 0 1 10 150 Tm (AA) Tj [-1000 (A)] TJ \
         1 0 0 1 60 150 Tm (A) Tj ET",
    );
    let mut session = Session::with_fonts(source, b"", Some(large()));
    let style_all = |session: &mut Session, style: pdf_edit::TextStyle| {
        let page = session.page(0).expect("the page reads");
        let rows = first_block_rows(&page);
        let end = rows[0].len();
        let command = Command::StyleBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows,
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            range: pdf_edit::BlockRange::Between {
                from: (0, 0),
                to: (0, end),
            },
            style,
        };
        session.plan(&command)
    };
    let red = pdf_edit::TextStyle {
        fill: Some([1.0, 0.0, 0.0]),
        ..pdf_edit::TextStyle::default()
    };
    let plan = style_all(&mut session, red).expect("the block is set red");
    session.apply(plan).expect("applies");
    assert_pens(
        &pens(&session.page(0).expect("the page reads")),
        &[(10.0, 150.0), (16.0, 150.0), (34.0, 150.0), (60.0, 150.0)],
    );
    let larger = pdf_edit::TextStyle {
        size: Some(24.0),
        ..pdf_edit::TextStyle::default()
    };
    let plan = style_all(&mut session, larger).expect("the block is set larger");
    session.apply(plan).expect("applies");
    assert_pens(
        &pens(&session.page(0).expect("the page reads")),
        &[(10.0, 150.0), (22.0, 150.0), (58.0, 150.0), (110.0, 150.0)],
    );
}

#[test]
fn a_style_leaves_a_row_of_larger_text_on_its_baseline() {
    let source = page_showing(
        "BT /F1 12 Tf 14 TL 1 0 0 1 10 150 Tm (AA) Tj /F1 14 Tf 1 0 0 1 10 134 Tm (AA) Tj \
         /F1 12 Tf 1 0 0 1 10 120 Tm (AA) Tj ET",
    );
    let mut session = Session::with_fonts(source, b"", Some(large()));
    let page = session.page(0).expect("the page reads");
    let command = Command::StyleBlock {
        paragraph: pdf_edit::ParagraphLayout::default(),
        page_index: 0,
        rows: first_block_rows(&page),
        frame: (10.0, 190.0),
        edges: (0, 0),
        breaks: Some(pdf_edit::RowEnds::paragraphs(vec![0, 1, 2])),
        range: pdf_edit::BlockRange::Between {
            from: (0, 0),
            to: (2, 2),
        },
        style: pdf_edit::TextStyle {
            fill: Some([1.0, 0.0, 0.0]),
            ..pdf_edit::TextStyle::default()
        },
    };
    let plan = session.plan(&command).expect("the block is set red");
    session.apply(plan).expect("applies");
    let after = session.page(0).expect("the page reads");
    let baselines: Vec<f64> = pens(&after).iter().map(|pen| pen.1).collect();
    assert_eq!(baselines.len(), 6, "{baselines:?}");
    for (baseline, expected) in baselines
        .iter()
        .zip([150.0, 150.0, 134.0, 134.0, 120.0, 120.0])
    {
        assert!((baseline - expected).abs() < 1e-3, "{baselines:?}");
    }
}

#[test]
fn a_block_with_rows_nearer_than_its_pitch_is_styled_while_no_row_moves() {
    let source = page_showing(
        "BT /F1 12 Tf 14 TL 1 0 0 1 10 150 Tm (AA) Tj 1 0 0 1 10 140 Tm (AA) Tj \
         1 0 0 1 10 126 Tm (AA) Tj ET",
    );
    let mut session = Session::with_fonts(source, b"", Some(large()));
    let style_all = |session: &mut Session, style: pdf_edit::TextStyle| {
        let page = session.page(0).expect("the page reads");
        session.plan(&Command::StyleBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: Some(pdf_edit::RowEnds::paragraphs(vec![0, 1, 2])),
            range: pdf_edit::BlockRange::Between {
                from: (0, 0),
                to: (2, 2),
            },
            style,
        })
    };
    let plan = style_all(
        &mut session,
        pdf_edit::TextStyle {
            fill: Some([1.0, 0.0, 0.0]),
            ..pdf_edit::TextStyle::default()
        },
    )
    .expect("the block is set red");
    session.apply(plan).expect("applies");
    let after = session.page(0).expect("the page reads");
    let baselines: Vec<f64> = pens(&after).iter().map(|pen| pen.1).collect();
    assert_eq!(baselines.len(), 6, "{baselines:?}");
    for (baseline, expected) in baselines
        .iter()
        .zip([150.0, 150.0, 140.0, 140.0, 126.0, 126.0])
    {
        assert!((baseline - expected).abs() < 1e-3, "{baselines:?}");
    }
    let refused = style_all(
        &mut session,
        pdf_edit::TextStyle {
            size: Some(24.0),
            ..pdf_edit::TextStyle::default()
        },
    )
    .expect_err("a size that moves a row is refused");
    assert!(refused.to_string().contains("closer together"), "{refused}");
}

#[test]
fn a_style_keeps_a_row_a_little_past_its_frame_on_its_line() {
    let source = page_showing("BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj T* (AA) Tj ET");
    let mut session = Session::with_fonts(source, b"", Some(large()));
    let page = session.page(0).expect("the page reads");
    let command = Command::StyleBlock {
        paragraph: pdf_edit::ParagraphLayout::default(),
        page_index: 0,
        rows: first_block_rows(&page),
        frame: (10.0, 33.0),
        edges: (0, 0),
        breaks: Some(pdf_edit::RowEnds::paragraphs(vec![0, 1])),
        range: pdf_edit::BlockRange::Between {
            from: (0, 0),
            to: (1, 2),
        },
        style: pdf_edit::TextStyle {
            fill: Some([1.0, 0.0, 0.0]),
            ..pdf_edit::TextStyle::default()
        },
    };
    let plan = session.plan(&command).expect("the block is set red");
    session.apply(plan).expect("applies");
    let after = session.page(0).expect("the page reads");
    let on = |y: f64| {
        pens(&after)
            .iter()
            .filter(|pen| (pen.1 - y).abs() < 1e-6)
            .count()
    };
    assert_eq!((on(150.0), on(136.0)), (4, 2), "{:?}", pens(&after));
}

#[test]
fn right_to_left_text_is_refused_until_it_can_be_ordered() {
    let source = page_showing("BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET");
    let provider: Arc<dyn FontProvider> = Arc::new(Covering {
        covering: Some(square_face_with_advance(300, &[('\u{0627}', 1)], 600)),
    });
    let mut session = Session::with_fonts(source, b"", Some(provider));
    let page = session.page(0).expect("the page reads");
    let rewrite = Command::RewriteBlock {
        paragraph: pdf_edit::ParagraphLayout::default(),
        page_index: 0,
        rows: first_block_rows(&page),
        frame: (10.0, 190.0),
        edges: (0, 0),
        breaks: None,
        frame_declared: true,
        range: pdf_edit::BlockRange::Between {
            from: (0, 2),
            to: (0, 2),
        },
        text: "\u{0627}".to_owned(),
    };
    let refused = session.plan(&rewrite).expect_err("right to left");
    assert!(refused.to_string().contains("right-to-left"), "{refused}");
}

fn two_square_face(first: char, second: char) -> Arc<GlyphProgram> {
    let plain = square_face_with_advance(300, &[(first, 1), (second, 2)], 600);
    let bytes = plain.program_bytes();
    let count = usize::from(u16::from_be_bytes([bytes[4], bytes[5]]));
    let table = |wanted: &[u8; 4]| {
        (0..count)
            .map(|index| &bytes[12 + index * 16..28 + index * 16])
            .find(|record| record[..4] == wanted[..])
            .map(|record| {
                let at = |offset: usize| {
                    usize::try_from(u32::from_be_bytes([
                        record[offset],
                        record[offset + 1],
                        record[offset + 2],
                        record[offset + 3],
                    ]))
                    .expect("offset")
                };
                bytes[at(8)..at(8) + at(12)].to_vec()
            })
            .expect("a table the face has")
    };
    let loca = table(b"loca");
    let square_end = usize::from(u16::from_be_bytes([loca[4], loca[5]])) * 2;
    let square = table(b"glyf")[..square_end].to_vec();
    let mut glyf = square.clone();
    glyf.extend_from_slice(&square);
    let half = u16::try_from(square_end / 2).expect("loca");
    let loca: Vec<u8> = [0, 0, half, half * 2]
        .iter()
        .flat_map(|value: &u16| value.to_be_bytes())
        .collect();
    let mut maxp = table(b"maxp");
    maxp[4..6].copy_from_slice(&3_u16.to_be_bytes());
    let mut hhea = table(b"hhea");
    hhea[34..36].copy_from_slice(&3_u16.to_be_bytes());
    let hmtx: Vec<u8> = [0_u16, 0, 600, 0, 600, 0]
        .iter()
        .flat_map(|value| value.to_be_bytes())
        .collect();
    let tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"cmap", table(b"cmap")),
        (b"glyf", glyf),
        (b"head", table(b"head")),
        (b"hhea", hhea),
        (b"hmtx", hmtx),
        (b"loca", loca),
        (b"maxp", maxp),
    ];
    Arc::new(GlyphProgram::parse(sfnt(&tables)).expect("the face parses"))
}

#[test]
fn a_face_the_page_already_embedded_grows_instead_of_being_embedded_again() {
    let source = page_showing("BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET");
    let provider: Arc<dyn FontProvider> = Arc::new(Covering {
        covering: Some(two_square_face('ລ', 'ວ')),
    });
    let mut session = Session::with_fonts(source, b"", Some(provider));
    let type_at = |session: &mut Session, stop: usize, text: &str| {
        let page = session.page(0).expect("the page reads");
        let rewrite = Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            frame_declared: true,
            range: pdf_edit::BlockRange::Between {
                from: (0, stop),
                to: (0, stop),
            },
            text: text.to_owned(),
        };
        let plan = session.plan(&rewrite).expect("typed");
        session.apply(plan).expect("applies");
    };
    type_at(&mut session, 2, "ລ");
    type_at(&mut session, 3, "ວ");

    let after = session.page(0).expect("the page reads");
    let names: Vec<String> = after
        .program
        .resources
        .fonts()
        .iter()
        .map(|font| String::from_utf8_lossy(font.name()).into_owned())
        .collect();
    assert_eq!(names, ["/F1", "/F2"]);
    assert_pens(
        &pens(&after),
        &[
            (10.0, 150.0),
            (16.0, 150.0),
            (22.0, 150.0),
            (29.2, 150.0),
            (36.4, 150.0),
            (42.4, 150.0),
        ],
    );
    let (read, embedded) = read_and_embedded(&after);
    assert_eq!(read, "AAລວAA");
    assert_eq!(
        embedded,
        [("/F2".to_owned(), Some(1)), ("/F2".to_owned(), Some(2))]
    );

    session.undo().expect("undoes");
    let undone = session.page(0).expect("the page reads");
    let (read, embedded) = read_and_embedded(&undone);
    assert_eq!(read, "AAລAA");
    assert_eq!(embedded, [("/F2".to_owned(), Some(1))]);
}

fn read_and_embedded(page: &PageView) -> (String, Vec<(String, Option<u16>)>) {
    let mut read = String::new();
    let mut embedded = Vec::new();
    for atom in &page.graph.atoms {
        let pdf_paint::PaintAtomKind::Text(text) = &atom.kind else {
            continue;
        };
        let font = text.state.text.font.as_ref().expect("a font");
        for glyph in &text.glyphs {
            let code = pdf_content::Code {
                value: glyph.code.value,
                byte_len: glyph.code.bytes.len(),
            };
            read.push_str(&text.text.text_of(code).expect("the code reads").text);
            if text.program.is_some() {
                embedded.push((
                    String::from_utf8_lossy(&font.value.name).into_owned(),
                    glyph.glyph,
                ));
            }
        }
    }
    (read, embedded)
}

#[test]
fn a_selection_is_set_in_another_size_and_colour() {
    let source = page_showing("BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET");
    let mut session = Session::with_fonts(source, b"", Some(large()));
    let style_between = |session: &mut Session, from: usize, to: usize| {
        let page = session.page(0).expect("the page reads");
        let command = Command::StyleBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            range: pdf_edit::BlockRange::Between {
                from: (0, from),
                to: (0, to),
            },
            style: pdf_edit::TextStyle {
                size: Some(24.0),
                fill: Some([1.0, 0.0, 0.0]),
                ..pdf_edit::TextStyle::default()
            },
        };
        session.plan(&command)
    };
    let plan = style_between(&mut session, 1, 3).expect("the selection is styled");
    session.apply(plan).expect("applies");
    let after = session.page(0).expect("the page reads");
    assert_pens(
        &pens(&after),
        &[(10.0, 150.0), (16.0, 150.0), (28.0, 150.0), (40.0, 150.0)],
    );
    let fills: Vec<String> = inks(&after)
        .iter()
        .map(|ink| {
            ink.split(" fill ")
                .nth(1)
                .unwrap_or_default()
                .split(" stroke ")
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    assert_eq!(fills[1], fills[2]);
    assert_ne!(fills[0], fills[1], "{fills:?}");
    assert_eq!(fills[0], fills[3], "{fills:?}");
    assert_eq!(
        fills[1],
        pdf_paint::colour_signature(&pdf_paint::Color::DeviceRgb(1.0, 0.0, 0.0))
    );
    let sizes: Vec<f64> = after
        .graph
        .atoms
        .iter()
        .filter_map(|atom| match &atom.kind {
            pdf_paint::PaintAtomKind::Text(text) => Some(text),
            _ => None,
        })
        .flat_map(|text| text.glyphs.iter().map(|_| text.state.text.font_size.value))
        .collect();
    assert_eq!(sizes, [12.0, 24.0, 24.0, 12.0]);

    session.undo().expect("undoes");
    let undone = session.page(0).expect("the page reads");
    assert_pens(
        &pens(&undone),
        &[(10.0, 150.0), (16.0, 150.0), (22.0, 150.0), (28.0, 150.0)],
    );

    let refused = style_between(&mut session, 2, 2).expect_err("control: nothing selected");
    assert!(
        refused.to_string().contains("nothing selected"),
        "{refused}"
    );
}

#[derive(Debug)]
struct Weights {
    bold: Option<Arc<GlyphProgram>>,
}

impl FontProvider for Weights {
    fn primary_face(&self, request: &FontRequest) -> Option<SubstitutedFace> {
        match (&self.bold, request.style.is_bold()) {
            (Some(bold), true) => Some(SubstitutedFace {
                program: Arc::clone(bold),
                identity: Arc::new(pdf_content::FaceIdentity {
                    family: "Test".to_owned(),
                    subfamily: "Bold".to_owned(),
                    origin: "test:bold".to_owned(),
                    sha256: "4".repeat(64),
                    face_index: 0,
                    style: pdf_content::FontStyle {
                        weight: 700,
                        italic: false,
                    },
                }),
                reason: SubstitutionReason::ExactFamily,
            }),
            _ => large().primary_face(request),
        }
    }
    fn fallback_face(&self, _: &FontRequest, _: char) -> Option<SubstitutedFace> {
        None
    }
    fn description(&self) -> String {
        "test weights".to_owned()
    }
}

#[test]
fn a_selection_is_set_bold_in_its_familys_bold_face() {
    let source =
        page_showing("1 0 0 rg BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET 1 w 0 0 m 50 50 l S");
    let styled = |session: &mut Session, style: pdf_edit::TextStyle| {
        let page = session.page(0).expect("the page reads");
        session.plan(&Command::StyleBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            range: pdf_edit::BlockRange::Between {
                from: (0, 1),
                to: (0, 3),
            },
            style,
        })
    };
    let bold = pdf_edit::TextStyle {
        bold: Some(true),
        ..pdf_edit::TextStyle::default()
    };
    let provider: Arc<dyn FontProvider> = Arc::new(Weights {
        bold: Some(square_face_with_advance(300, &[('A', 1)], 800)),
    });
    let mut session = Session::with_fonts(source.clone(), b"", Some(provider));
    let plan = styled(&mut session, bold.clone()).expect("set bold");
    session.apply(plan).expect("applies");
    let after = session.page(0).expect("the page reads");
    assert_pens(
        &pens(&after),
        &[(10.0, 150.0), (16.0, 150.0), (25.6, 150.0), (35.2, 150.0)],
    );
    let (read, embedded) = read_and_embedded(&after);
    assert_eq!(read, "AAAA");
    assert_eq!(
        embedded,
        [("/F2".to_owned(), Some(1)), ("/F2".to_owned(), Some(1))]
    );

    let provider: Arc<dyn FontProvider> = Arc::new(Weights { bold: None });
    let mut regular_only = Session::with_fonts(source, b"", Some(provider));
    let plan = styled(&mut regular_only, bold).expect("bold is synthesised");
    regular_only.apply(plan).expect("applies");
    let after = regular_only.page(0).expect("the page reads");
    assert_pens(
        &pens(&after),
        &[(10.0, 150.0), (16.0, 150.0), (22.0, 150.0), (28.0, 150.0)],
    );
    let mut strokes = Vec::new();
    let mut path_width = None;
    for atom in &after.graph.atoms {
        match &atom.kind {
            pdf_paint::PaintAtomKind::Text(text) => {
                let state = &text.state;
                let like_fill = pdf_paint::colour_signature(&state.stroke_color.value)
                    == pdf_paint::colour_signature(&state.fill_color.value);
                for _ in &text.glyphs {
                    strokes.push((
                        state.text.rendering_mode.value,
                        state.line_width.value,
                        like_fill,
                    ));
                }
            }
            pdf_paint::PaintAtomKind::Path(path) => path_width = Some(path.state.line_width.value),
            _ => {}
        }
    }
    let (fill, bold) = (
        pdf_paint::TextRenderingMode::Fill,
        pdf_paint::TextRenderingMode::FillStroke,
    );
    assert_eq!((strokes[0].0, strokes[3].0), (fill, fill), "{strokes:?}");
    for middle in &strokes[1..3] {
        assert_eq!(middle.0, bold, "{strokes:?}");
        assert!((middle.1 - 0.36).abs() < 1e-9 && middle.2, "{strokes:?}");
    }
    assert_eq!(path_width, Some(1.0));

    let refused = styled(
        &mut regular_only,
        pdf_edit::TextStyle {
            bold: Some(true),
            family: Some("Test".to_owned()),
            ..pdf_edit::TextStyle::default()
        },
    )
    .expect_err("control: synthesised bold in another family");
    assert!(refused.to_string().contains("weight or slope"), "{refused}");
}

#[test]
fn text_set_bold_in_a_taller_face_keeps_the_blocks_line_spacing() {
    let source = page_showing("BT /F1 12 Tf 10 TL 10 150 Td (AA) Tj T* (AA) Tj ET");
    let provider: Arc<dyn FontProvider> = Arc::new(Weights {
        bold: Some(square_face_with_advance(300, &[('A', 1)], 500)),
    });
    let mut session = Session::with_fonts(source, b"", Some(provider));
    let page = session.page(0).expect("the page reads");
    let plan = session
        .plan(&Command::StyleBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: Some(pdf_edit::RowEnds::paragraphs(vec![0, 1])),
            range: pdf_edit::BlockRange::Between {
                from: (0, 0),
                to: (1, 2),
            },
            style: pdf_edit::TextStyle {
                bold: Some(true),
                ..pdf_edit::TextStyle::default()
            },
        })
        .expect("set bold");
    session.apply(plan).expect("applies");
    let baselines: Vec<f64> = pens(&session.page(0).expect("the page reads"))
        .iter()
        .map(|pen| pen.1)
        .collect();
    assert_eq!(baselines.len(), 4, "{baselines:?}");
    for (baseline, expected) in baselines.iter().zip([150.0, 150.0, 140.0, 140.0]) {
        assert!((baseline - expected).abs() < 1e-3, "{baselines:?}");
    }
}

#[test]
fn a_selection_is_emboldened_when_its_familys_bold_face_cannot_draw_it() {
    let source = page_showing("1 0 0 rg BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET");
    let provider: Arc<dyn FontProvider> = Arc::new(Weights {
        bold: Some(square_face_with_advance(300, &[('B', 1)], 800)),
    });
    let mut session = Session::with_fonts(source, b"", Some(provider));
    let page = session.page(0).expect("the page reads");
    let plan = session
        .plan(&Command::StyleBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            range: pdf_edit::BlockRange::Between {
                from: (0, 1),
                to: (0, 3),
            },
            style: pdf_edit::TextStyle {
                bold: Some(true),
                ..pdf_edit::TextStyle::default()
            },
        })
        .expect("bold is drawn from the text's own glyphs");
    session.apply(plan).expect("applies");
    let after = session.page(0).expect("the page reads");
    assert_pens(
        &pens(&after),
        &[(10.0, 150.0), (16.0, 150.0), (22.0, 150.0), (28.0, 150.0)],
    );
    let modes: Vec<String> = after
        .graph
        .atoms
        .iter()
        .filter_map(|atom| match &atom.kind {
            pdf_paint::PaintAtomKind::Text(text) => Some(text),
            _ => None,
        })
        .flat_map(|text| {
            let mode = format!("{:?}", text.state.text.rendering_mode.value);
            text.glyphs.iter().map(move |_| mode.clone())
        })
        .collect();
    let stroked: Vec<bool> = modes.iter().map(|mode| mode.contains("Stroke")).collect();
    assert_eq!(stroked, [false, true, true, false], "{modes:?}");
    let (read, _) = read_and_embedded(&after);
    assert_eq!(read, "AAAA");
}

#[derive(Debug)]
struct OtherFamilyBold;

impl FontProvider for OtherFamilyBold {
    fn primary_face(&self, request: &FontRequest) -> Option<SubstitutedFace> {
        if !request.style.is_bold() {
            return large().primary_face(request);
        }
        Some(SubstitutedFace {
            program: square_face_with_advance(300, &[('A', 1)], 800),
            identity: Arc::new(pdf_content::FaceIdentity {
                family: "Other".to_owned(),
                subfamily: "Bold".to_owned(),
                origin: "test:other-bold".to_owned(),
                sha256: "5".repeat(64),
                face_index: 0,
                style: pdf_content::FontStyle {
                    weight: 700,
                    italic: false,
                },
            }),
            reason: SubstitutionReason::ExactFamily,
        })
    }
    fn fallback_face(&self, _: &FontRequest, _: char) -> Option<SubstitutedFace> {
        None
    }
    fn description(&self) -> String {
        "test other-family bold".to_owned()
    }
}

#[test]
fn a_selection_is_emboldened_when_the_bold_face_is_another_familys() {
    let source = page_showing("1 0 0 rg BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET");
    let mut session = Session::with_fonts(source, b"", Some(Arc::new(OtherFamilyBold)));
    let page = session.page(0).expect("the page reads");
    let plan = session
        .plan(&Command::StyleBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            range: pdf_edit::BlockRange::Between {
                from: (0, 1),
                to: (0, 3),
            },
            style: pdf_edit::TextStyle {
                bold: Some(true),
                ..pdf_edit::TextStyle::default()
            },
        })
        .expect("bold is drawn from the text's own glyphs");
    session.apply(plan).expect("applies");
    let after = session.page(0).expect("the page reads");
    assert_pens(
        &pens(&after),
        &[(10.0, 150.0), (16.0, 150.0), (22.0, 150.0), (28.0, 150.0)],
    );
    let stroked: Vec<bool> = after
        .graph
        .atoms
        .iter()
        .filter_map(|atom| match &atom.kind {
            pdf_paint::PaintAtomKind::Text(text) => Some(text),
            _ => None,
        })
        .flat_map(|text| {
            let stroke = format!("{:?}", text.state.text.rendering_mode.value).contains("Stroke");
            text.glyphs.iter().map(move |_| stroke)
        })
        .collect();
    assert_eq!(stroked, [false, true, true, false]);
}

#[test]
fn a_selection_is_slanted_when_its_family_has_no_italic_face() {
    let source = page_showing("BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET");
    let provider: Arc<dyn FontProvider> = Arc::new(Weights { bold: None });
    let mut session = Session::with_fonts(source, b"", Some(provider));
    let command = |session: &mut Session, style: pdf_edit::TextStyle| {
        let page = session.page(0).expect("the page reads");
        Command::StyleBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            range: pdf_edit::BlockRange::Between {
                from: (0, 1),
                to: (0, 3),
            },
            style,
        }
    };
    let slants = |page: &PageView| -> Vec<f64> {
        page.graph
            .atoms
            .iter()
            .filter_map(|atom| match &atom.kind {
                pdf_paint::PaintAtomKind::Text(text) => Some(text),
                _ => None,
            })
            .flat_map(|text| text.glyphs.iter().map(|glyph| glyph.text_matrix.c))
            .collect()
    };
    let italic = command(
        &mut session,
        pdf_edit::TextStyle {
            italic: Some(true),
            ..pdf_edit::TextStyle::default()
        },
    );
    let plan = session.plan(&italic).expect("italic is synthesised");
    session.apply(plan).expect("applies");
    let after = session.page(0).expect("the page reads");
    assert_pens(
        &pens(&after),
        &[(10.0, 150.0), (16.0, 150.0), (22.0, 150.0), (28.0, 150.0)],
    );
    let found = slants(&after);
    assert_eq!(found.len(), 4);
    for (slant, wanted) in found.iter().zip([0.0, 0.2, 0.2, 0.0]) {
        assert!((slant - wanted).abs() < 1e-9, "{found:?}");
    }

    let rows = first_block_rows(&after);
    let plan = session
        .plan(&Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows,
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            frame_declared: true,
            range: pdf_edit::BlockRange::Between {
                from: (0, 4),
                to: (0, 4),
            },
            text: "A".to_owned(),
        })
        .expect("a slanted block is typed into");
    session.apply(plan).expect("applies");
    let typed = session.page(0).expect("the page reads");
    assert_pens(
        &pens(&typed),
        &[
            (10.0, 150.0),
            (16.0, 150.0),
            (22.0, 150.0),
            (28.0, 150.0),
            (34.0, 150.0),
        ],
    );
    let found = slants(&typed);
    for (slant, wanted) in found.iter().zip([0.0, 0.2, 0.2, 0.0, 0.0]) {
        assert!((slant - wanted).abs() < 1e-9, "{found:?}");
    }

    let both = command(
        &mut session,
        pdf_edit::TextStyle {
            italic: Some(true),
            family: Some("Test".to_owned()),
            ..pdf_edit::TextStyle::default()
        },
    );
    let refused = session.plan(&both).expect_err("control: slant and family");
    assert!(refused.to_string().contains("weight or slope"), "{refused}");
}

#[test]
fn a_whole_block_set_in_synthesised_italic_or_bold_is_set_back() {
    let source = page_showing("BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET");
    let provider: Arc<dyn FontProvider> = Arc::new(Weights { bold: None });
    let mut session = Session::with_fonts(source, b"", Some(provider));
    let style = |session: &mut Session, style: pdf_edit::TextStyle| {
        let page = session.page(0).expect("the page reads");
        let plan = session
            .plan(&Command::StyleBlock {
                paragraph: pdf_edit::ParagraphLayout::default(),
                page_index: 0,
                rows: first_block_rows(&page),
                frame: (10.0, 190.0),
                edges: (0, 0),
                breaks: None,
                range: pdf_edit::BlockRange::Between {
                    from: (0, 0),
                    to: (0, 4),
                },
                style,
            })
            .expect("the whole block is styled");
        session.apply(plan).expect("applies");
        session.page(0).expect("the page reads")
    };
    let slants = |page: &PageView| -> Vec<f64> {
        page.graph
            .atoms
            .iter()
            .filter_map(|atom| match &atom.kind {
                pdf_paint::PaintAtomKind::Text(text) => Some(text),
                _ => None,
            })
            .flat_map(|text| text.glyphs.iter().map(|glyph| glyph.text_matrix.c))
            .collect()
    };
    let modes = |page: &PageView| -> Vec<pdf_paint::TextRenderingMode> {
        page.graph
            .atoms
            .iter()
            .filter_map(|atom| match &atom.kind {
                pdf_paint::PaintAtomKind::Text(text) => Some(text.state.text.rendering_mode.value),
                _ => None,
            })
            .collect()
    };
    let pens_now = [(10.0, 150.0), (16.0, 150.0), (22.0, 150.0), (28.0, 150.0)];

    let slanted = style(
        &mut session,
        pdf_edit::TextStyle {
            italic: Some(true),
            ..pdf_edit::TextStyle::default()
        },
    );
    assert_pens(&pens(&slanted), &pens_now);
    assert!(
        slants(&slanted)
            .iter()
            .all(|slant| (slant - 0.2).abs() < 1e-9)
    );
    let upright = style(
        &mut session,
        pdf_edit::TextStyle {
            italic: Some(false),
            ..pdf_edit::TextStyle::default()
        },
    );
    assert_pens(&pens(&upright), &pens_now);
    let found = slants(&upright);
    assert!(found.iter().all(|slant| slant.abs() < 1e-9), "{found:?}");

    let bold = style(
        &mut session,
        pdf_edit::TextStyle {
            bold: Some(true),
            ..pdf_edit::TextStyle::default()
        },
    );
    assert!(
        modes(&bold)
            .iter()
            .all(|mode| *mode == pdf_paint::TextRenderingMode::FillStroke),
        "{:?}",
        modes(&bold)
    );
    let plain = style(
        &mut session,
        pdf_edit::TextStyle {
            bold: Some(false),
            ..pdf_edit::TextStyle::default()
        },
    );
    assert!(
        modes(&plain)
            .iter()
            .all(|mode| *mode == pdf_paint::TextRenderingMode::Fill),
        "{:?}",
        modes(&plain)
    );
    assert_pens(&pens(&plain), &pens_now);
}

fn underlines(page: &PageView) -> Vec<[f64; 4]> {
    page.graph
        .atoms
        .iter()
        .filter(|atom| matches!(&atom.kind, pdf_paint::PaintAtomKind::Path(path) if path.fill.is_some()))
        .filter_map(|atom| atom.kind.user_bounds())
        .filter(|bounds| bounds[3] - bounds[1] <= 1.0)
        .collect()
}

fn assert_boxes(found: &[[f64; 4]], wanted: &[[f64; 4]]) {
    assert_eq!(found.len(), wanted.len(), "{found:?}");
    for (one, other) in found.iter().zip(wanted) {
        assert!(
            one.iter()
                .zip(other)
                .all(|(one, other)| (one - other).abs() < 1e-6),
            "{found:?}"
        );
    }
}

#[test]
fn a_cluster_set_back_over_the_one_before_is_underlined_under_its_span() {
    let source = page_showing("BT /F1 12 Tf 14 TL 10 150 Td (A) Tj -8 Tc (A) Tj ET");
    let mut session = Session::with_fonts(source, b"", Some(large()));
    let page = session.page(0).expect("the page reads");
    let plan = session
        .plan(&Command::StyleBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            range: pdf_edit::BlockRange::Between {
                from: (0, 1),
                to: (0, 2),
            },
            style: pdf_edit::TextStyle {
                underline: Some(true),
                ..pdf_edit::TextStyle::default()
            },
        })
        .expect("underlined");
    session.apply(plan).expect("applies");
    let after = session.page(0).expect("the page reads");
    assert_pens(&pens(&after), &[(10.0, 150.0), (16.0, 150.0)]);
    assert_boxes(&underlines(&after), &[[14.0, 147.84, 16.0, 148.56]]);
}

#[test]
fn a_selection_is_underlined_and_its_underline_moves_with_it() {
    let source = page_showing("BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET");
    let mut session = Session::with_fonts(source, b"", Some(large()));
    let page = session.page(0).expect("the page reads");
    let plan = session
        .plan(&Command::StyleBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            range: pdf_edit::BlockRange::Between {
                from: (0, 1),
                to: (0, 3),
            },
            style: pdf_edit::TextStyle {
                underline: Some(true),
                ..pdf_edit::TextStyle::default()
            },
        })
        .expect("underlined");
    session.apply(plan).expect("applies");
    let after = session.page(0).expect("the page reads");
    assert_boxes(&underlines(&after), &[[16.0, 147.84, 28.0, 148.56]]);

    let plan = session
        .plan(&Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&after),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            frame_declared: true,
            range: pdf_edit::BlockRange::Between {
                from: (0, 0),
                to: (0, 0),
            },
            text: "A".to_owned(),
        })
        .expect("typed before the underline");
    session.apply(plan).expect("applies");
    let typed = session.page(0).expect("the page reads");
    assert_eq!(pens(&typed).len(), 5);
    assert_boxes(&underlines(&typed), &[[22.0, 147.84, 34.0, 148.56]]);

    session.undo().expect("undoes");
    let undone = session.page(0).expect("the page reads");
    assert_boxes(&underlines(&undone), &[[16.0, 147.84, 28.0, 148.56]]);
}

fn type_before_block(content: &str) -> (std::sync::Arc<PageView>, Vec<u8>) {
    let source = page_showing(content);
    let mut session = Session::with_fonts(source.clone(), b"", Some(large()));
    let page = session.page(0).expect("the page reads");
    let plan = session
        .plan(&Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            frame_declared: true,
            range: pdf_edit::BlockRange::Between {
                from: (0, 0),
                to: (0, 0),
            },
            text: "A".to_owned(),
        })
        .expect("typed before the block");
    let written = plan.commit(&source, b"").expect("commits");
    session.apply(plan).expect("applies");
    (
        session.page(0).expect("the page reads"),
        written.as_bytes().to_vec(),
    )
}

#[test]
fn an_underline_is_written_with_no_name_of_ours_and_found_by_its_shape() {
    let source = page_showing("BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET");
    let mut session = Session::with_fonts(source.clone(), b"", Some(large()));
    let page = session.page(0).expect("the page reads");
    let plan = session
        .plan(&Command::StyleBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            range: pdf_edit::BlockRange::Between {
                from: (0, 1),
                to: (0, 3),
            },
            style: pdf_edit::TextStyle {
                underline: Some(true),
                ..pdf_edit::TextStyle::default()
            },
        })
        .expect("underlined");
    let written = plan.commit(&source, b"").expect("commits");
    assert!(
        !written
            .as_bytes()
            .windows(6)
            .any(|window| window == b"PanPDF"),
        "the file names this editor"
    );
    session.apply(plan).expect("applies");
    let after = session.page(0).expect("the page reads");
    let plan = session
        .plan(&Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&after),
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: None,
            frame_declared: true,
            range: pdf_edit::BlockRange::Between {
                from: (0, 0),
                to: (0, 0),
            },
            text: "A".to_owned(),
        })
        .expect("typed");
    session.apply(plan).expect("applies");
    let typed = session.page(0).expect("the page reads");
    assert_boxes(&underlines(&typed), &[[22.0, 147.84, 34.0, 148.56]]);
}

#[test]
fn another_writers_underline_goes_with_its_text_and_other_rules_stay() {
    let text = "BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET";
    let (typed, _) = type_before_block(&format!("{text} 0 g 16 147.5 12 0.6 re f"));
    assert_eq!(pens(&typed).len(), 5);
    assert_boxes(&underlines(&typed), &[[22.0, 147.5, 34.0, 148.1]]);

    let (typed, _) = type_before_block(&format!("{text} 0 g 10 147.5 80 0.6 re f"));
    assert_boxes(&underlines(&typed), &[[10.0, 147.5, 90.0, 148.1]]);
    let (typed, _) = type_before_block(&format!("{text} q 1 0 0 rg 16 147.5 12 0.6 re f Q"));
    assert_boxes(&underlines(&typed), &[[16.0, 147.5, 28.0, 148.1]]);
    let thick: Vec<[f64; 4]> = type_before_block(&format!("{text} 0 g 16 145 12 3 re f"))
        .0
        .graph
        .atoms
        .iter()
        .filter(|atom| matches!(atom.kind, pdf_paint::PaintAtomKind::Path(_)))
        .filter_map(|atom| atom.kind.user_bounds())
        .collect();
    assert_boxes(&thick, &[[16.0, 145.0, 28.0, 148.0]]);
}

#[test]
fn an_underline_under_the_old_tag_loses_the_tag_when_edited() {
    let (typed, written) = type_before_block(
        "BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET \
         /PanPDFUnderline BMC q 0 g 16 147.84 12 0.72 re f Q EMC",
    );
    assert_boxes(&underlines(&typed), &[[22.0, 147.84, 34.0, 148.56]]);
    let content = typed.program.streams[0].bytes.as_bytes();
    assert!(
        !content.windows(6).any(|window| window == b"PanPDF"),
        "{}",
        String::from_utf8_lossy(content)
    );
    let _ = written;
}

#[test]
fn a_turned_blocks_underline_follows_it_along_its_baseline() {
    let source = page_showing("BT /F1 12 Tf 14 TL 0 1 -1 0 150 10 Tm (AAAA) Tj ET");
    let mut session = Session::with_fonts(source, b"", Some(large()));
    let page = session.page(0).expect("the page reads");
    let turned_frame = (10.0, 190.0);
    let plan = session
        .plan(&Command::StyleBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: turned_frame,
            edges: (0, 0),
            breaks: None,
            range: pdf_edit::BlockRange::Between {
                from: (0, 1),
                to: (0, 3),
            },
            style: pdf_edit::TextStyle {
                underline: Some(true),
                ..pdf_edit::TextStyle::default()
            },
        })
        .expect("underlined");
    session.apply(plan).expect("applies");
    let rules = |page: &PageView| -> Vec<[f64; 4]> {
        page.graph
            .atoms
            .iter()
            .filter(|atom| matches!(&atom.kind, pdf_paint::PaintAtomKind::Path(path) if path.fill.is_some()))
            .filter_map(|atom| atom.kind.user_bounds())
            .collect()
    };
    let after = session.page(0).expect("the page reads");
    assert_boxes(&rules(&after), &[[151.44, 16.0, 152.16, 28.0]]);
    let plan = session
        .plan(&Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&after),
            frame: turned_frame,
            edges: (0, 0),
            breaks: None,
            frame_declared: true,
            range: pdf_edit::BlockRange::Between {
                from: (0, 0),
                to: (0, 0),
            },
            text: "A".to_owned(),
        })
        .expect("typed");
    session.apply(plan).expect("applies");
    let typed = session.page(0).expect("the page reads");
    assert_boxes(&rules(&typed), &[[151.44, 22.0, 152.16, 34.0]]);
    assert_pens(
        &pens(&typed),
        &[
            (150.0, 10.0),
            (150.0, 16.0),
            (150.0, 22.0),
            (150.0, 28.0),
            (150.0, 34.0),
        ],
    );

    let source =
        page_showing("BT /F1 12 Tf 14 TL 0.8660254 0.5 -0.5 0.8660254 50 50 Tm (AAAA) Tj ET");
    let mut session = Session::with_fonts(source, b"", Some(large()));
    let page = session.page(0).expect("the page reads");
    let plan = session
        .plan(&Command::StyleBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows: first_block_rows(&page),
            frame: (-100.0, 190.0),
            edges: (0, 0),
            breaks: None,
            range: pdf_edit::BlockRange::Between {
                from: (0, 1),
                to: (0, 3),
            },
            style: pdf_edit::TextStyle {
                underline: Some(true),
                ..pdf_edit::TextStyle::default()
            },
        })
        .expect("underlined at 30 degrees");
    session.apply(plan).expect("applies");
    assert_eq!(rules(&session.page(0).expect("the page reads")).len(), 1);
}

#[test]
fn a_shift_moves_every_glyph_exactly_however_the_row_was_set() {
    let shift = |second: &str| {
        let source = page_showing(&format!(
            "BT /F1 12 Tf 14 TL 10 150 Td (AAAA) Tj ET BT /F1 12 Tf {second} ET"
        ));
        let mut session = Session::with_fonts(source, b"", Some(large()));
        let page = session.page(0).expect("the page reads");
        let rows: Vec<Vec<pdf_edit::ClusterRef>> = page
            .index
            .blocks
            .iter()
            .flat_map(|block| block.lines.iter())
            .map(|line| {
                page.index.lines[*line]
                    .clusters
                    .iter()
                    .map(|cluster| {
                        let cluster = &page.index.clusters[*cluster];
                        pdf_edit::ClusterRef {
                            anchor: pdf_edit::SourceAnchor::of(&page.graph.atoms[cluster.atom].id),
                            glyphs: cluster.glyphs.clone(),
                        }
                    })
                    .collect()
            })
            .collect();
        assert_eq!(rows.len(), 2);
        let command = Command::ShiftBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows,
            frame: (10.0, 190.0),
            edges: (0, 0),
            breaks: Some(pdf_edit::RowEnds::paragraphs(vec![0, 1])),
            dx: 5.0,
            dy: 0.0,
        };
        let planned = session.plan(&command).map_err(|error| error.to_string());
        planned.map(|plan| {
            session.apply(plan).expect("applies");
            pens(&session.page(0).expect("the shifted page reads"))
        })
    };
    let mut stepped = shift("10 136 Td [(A) 1200 (AAA)] TJ").expect("a stepped row shifts");
    stepped.sort_by(|one, other| (-one.1, one.0).partial_cmp(&(-other.1, other.0)).unwrap());
    let wanted = [
        (15.0, 150.0),
        (21.0, 150.0),
        (27.0, 150.0),
        (33.0, 150.0),
        (6.6, 136.0),
        (12.6, 136.0),
        (15.0, 136.0),
        (18.6, 136.0),
    ];
    assert_eq!(stepped.len(), wanted.len(), "{stepped:?}");
    for (one, other) in stepped.iter().zip(&wanted) {
        assert!(
            (one.0 - other.0).abs() < 1e-6 && (one.1 - other.1).abs() < 1e-6,
            "{stepped:?}"
        );
    }
    let mut lifted =
        shift("10 136 Td (AAA) Tj 1 0 0 1 28 139 Tm (A) Tj").expect("a lifted cluster shifts");
    lifted.sort_by(|one, other| (-one.1, one.0).partial_cmp(&(-other.1, other.0)).unwrap());
    let wanted = [
        (15.0, 150.0),
        (21.0, 150.0),
        (27.0, 150.0),
        (33.0, 150.0),
        (33.0, 139.0),
        (15.0, 136.0),
        (21.0, 136.0),
        (27.0, 136.0),
    ];
    assert_eq!(lifted.len(), wanted.len(), "{lifted:?}");
    for (one, other) in lifted.iter().zip(&wanted) {
        assert!(
            (one.0 - other.0).abs() < 1e-6 && (one.1 - other.1).abs() < 1e-6,
            "{lifted:?}"
        );
    }
    let found = shift("10 136 Td (AAAA) Tj").expect("rows at the edge shift");
    let wanted: Vec<(f64, f64)> = [150.0, 136.0]
        .iter()
        .flat_map(|y| [15.0, 21.0, 27.0, 33.0].map(|x| (x, *y)))
        .collect();
    assert_eq!(found.len(), wanted.len(), "{found:?}");
    for (one, other) in found.iter().zip(&wanted) {
        assert!(
            (one.0 - other.0).abs() < 1e-6 && (one.1 - other.1).abs() < 1e-6,
            "{found:?}"
        );
    }
}

fn run_of(page: &PageView, atom: usize) -> &pdf_paint::TextShowPaint {
    let pdf_paint::PaintAtomKind::Text(text) = &page.graph.atoms[atom].kind else {
        panic!("atom {atom} is not text");
    };
    text
}

fn typing_into(
    source: &ByteStore,
    provider: Option<Arc<dyn FontProvider>>,
    typed: &str,
) -> Result<(f64, f64), String> {
    let mut session = Session::with_fonts(source.clone(), b"", provider);
    let page = session.page(0).expect("the page reads");
    let text = run_of(&page, 0);
    pdf_edit::replacement_shift(&page.program.resources, text, &(1..1), typed)
        .map_err(|error| error.to_string())
}

#[test]
fn a_font_the_file_only_names_can_be_typed_in_through_its_face() {
    let source = page_showing("BT /F1 12 Tf 10 20 Td (A) Tj ET");
    let shift = typing_into(&source, Some(large()), "A").expect("typed");
    assert!(
        (shift.0 - 6.0).abs() < 1e-9 && shift.1.abs() < 1e-9,
        "{shift:?}"
    );
    assert_eq!(
        typing_into(&source, None, "A"),
        Err(
            "cannot type here: the file does not carry this font and no face on this machine \
             stands in for it"
                .to_owned()
        )
    );
    let mut session = Session::with_fonts(source, b"", Some(large()));
    let page = session.page(0).expect("the page reads");
    assert_eq!(
        pdf_edit::substituted_family(run_of(&page, 0)).as_deref(),
        Some("Large Square")
    );
    let plan = session
        .plan(&Command::RewriteText {
            page_index: 0,
            runs: vec![pdf_edit::RunRewrite {
                anchor: pdf_edit::SourceAnchor::of(&page.graph.atoms[0].id),
                glyphs: Some(pdf_edit::GlyphChange::Replace {
                    glyphs: 1..1,
                    text: "A".to_owned(),
                }),
                displace: (0.0, 0.0),
            }],
        })
        .expect("a plan to type in a substituted run");
    let candidate = session.preview(&plan).expect("preview");
    assert_eq!(run_of(&candidate, 0).glyphs.len(), 2);
    let ink = candidate.graph.atoms[0].kind.user_bounds().expect("ink");
    for (one, other) in ink.iter().zip([10.0, 20.0, 22.0, 26.0]) {
        assert!((one - other).abs() < 1e-6, "{ink:?}");
    }
}

#[test]
fn an_embedded_font_is_still_written_in_its_own_outlines() {
    let source = embedded_page_showing(
        "BT /F1 12 Tf 10 20 Td (A) Tj ET",
        square_face(500).program_bytes(),
    );
    let shift = typing_into(&source, Some(large()), "A").expect("typed");
    assert!(
        (shift.0 - 6.0).abs() < 1e-9 && shift.1.abs() < 1e-9,
        "{shift:?}"
    );
    let mut session = Session::with_fonts(source, b"", Some(large()));
    let page = session.page(0).expect("the page reads");
    assert_eq!(pdf_edit::substituted_family(run_of(&page, 0)), None);
}

#[test]
fn invisible_text_is_refused_for_being_invisible() {
    let source = embedded_page_showing(
        "BT 3 Tr /F1 12 Tf 10 20 Td (A) Tj ET",
        square_face(500).program_bytes(),
    );
    assert_eq!(
        typing_into(&source, Some(large()), "A"),
        Err(
            "cannot type here: this text is invisible or used as a clipping path, so typing \
             would change nothing you can see"
                .to_owned()
        )
    );
}

#[test]
fn mirrored_text_is_refused_for_being_mirrored() {
    let upright = page_showing("BT /F1 12 Tf 10 20 Td (A) Tj ET");
    typing_into(&upright, Some(large()), "A").expect("upright text types");
    let mirrored = page_showing("BT /F1 -12 Tf 10 20 Td (A) Tj ET");
    assert_eq!(
        typing_into(&mirrored, Some(large()), "A"),
        Err(
            "cannot type here: this text is drawn mirrored (a negative size or width), which \
             typing cannot follow yet"
                .to_owned()
        )
    );
    let flipped = page_showing("BT /F1 12 Tf -100 Tz 10 20 Td (A) Tj ET");
    assert_eq!(
        typing_into(&flipped, Some(large()), "A"),
        Err(
            "cannot type here: this text is drawn mirrored (a negative size or width), which \
             typing cannot follow yet"
                .to_owned()
        )
    );
}
