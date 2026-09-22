#![allow(
    clippy::float_cmp,
    reason = "frame snapshots restore exactly; computed glyph positions use a tolerance"
)]

use crate::{Applied, Editor};
use pdf_bytes::{ByteStore, SourceId};
use pdf_paint::glyph_placement_signature;
use std::sync::Arc;

#[test]
fn export_reopens_with_the_authenticated_nonempty_password() {
    let source = ByteStore::new(
        SourceId::new(0),
        &include_bytes!("../../pdf-edit/tests/data/modifiable-r3.pdf")[..],
    );
    assert!(
        Editor::open(source.clone()).is_err(),
        "empty password is a negative control"
    );
    let mut editor = Editor::open_with(source.clone(), b"view").unwrap();
    let view =
        pdf_session::interpret_page_fully(&source, 0, b"view", None, pdf_cli::font_provider())
            .unwrap();
    assert!(!pdf_paint::glyph_placement_signature(&view.graph).is_empty());
    editor.adopt_page(0, Arc::new(view));
    let export = editor
        .export()
        .expect("the actual password reopens the file");
    assert!(
        export.placements > 0,
        "the reopen loop must actually execute"
    );
    assert_eq!(export.bytes, source.as_bytes());
}

#[test]
fn a_restricted_document_is_edited_only_after_the_person_chooses_to() {
    let source = ByteStore::new(
        SourceId::new(0),
        &include_bytes!("../../pdf-edit/tests/data/restricted-r3.pdf")[..],
    );
    let mut editor = Editor::open_with(source.clone(), b"view").unwrap();
    assert!(editor.editing_restricted());
    let read = |editor: &mut Editor| {
        let view = pdf_session::interpret_page_grouped(
            editor.source().unwrap(),
            0,
            b"view",
            editor.grouping(0).as_deref(),
        )
        .unwrap();
        editor.adopt_page(0, Arc::new(view));
    };
    read(&mut editor);
    let anchors = editor.leaf(0).unwrap().overlay.blocks[0].anchors.clone();
    assert!(matches!(
        editor.move_block(0, &anchors, 5.0, 0.0),
        Applied::Refused(_)
    ));
    assert_eq!(
        editor.status(),
        &crate::wording::Message::Refused(crate::wording::Refusal::EditingRestricted)
    );

    assert!(editor.set_aside_restrictions());
    assert!(!editor.editing_restricted());
    assert_eq!(editor.status(), &crate::wording::Message::Quiet);
    read(&mut editor);
    let anchors = editor.leaf(0).unwrap().overlay.blocks[0].anchors.clone();
    assert!(matches!(
        editor.move_block(0, &anchors, 5.0, 0.0),
        Applied::Changed { .. }
    ));
    assert_ne!(editor.source().unwrap().as_bytes(), source.as_bytes());
    let owner = Editor::open_with(source, b"master").unwrap();
    assert!(!owner.editing_restricted());
}

const SQUARE_CFF: &str = "0100040100010101055465737400010101131d00000030111d000000540f1d00000059100001010106616c70686100000003010102101e0e8b8b15f8888b8bf888fc888b050e8b8b15f8888b8bf888fc888b050e0000220187000141";

fn stream(bytes: &[u8], extra: &str) -> Vec<u8> {
    let mut result = format!("<< /Length {} {extra} >>\nstream\n", bytes.len()).into_bytes();
    result.extend_from_slice(bytes);
    result.extend_from_slice(b"\nendstream");
    result
}

fn fixture(content: &[u8]) -> ByteStore {
    fixture_saying(content, b"<41> <0041> <42> <0042>")
}

fn fixture_saying(content: &[u8], bfchar: &[u8]) -> ByteStore {
    fixture_with(content, bfchar, "")
}

fn fixture_with(content: &[u8], bfchar: &[u8], descriptor: &str) -> ByteStore {
    fixture_font(content, bfchar, descriptor, "[600 600]")
}

fn fixture_font(content: &[u8], bfchar: &[u8], descriptor: &str, widths: &str) -> ByteStore {
    fixture_page(content, (bfchar, descriptor, widths), "")
}

fn fixture_page(
    content: &[u8],
    (bfchar, descriptor, widths): (&[u8], &str, &str),
    page: &str,
) -> ByteStore {
    let program: Vec<u8> = SQUARE_CFF
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect();
    let objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /MediaBox [0 0 200 200] /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R {page}/Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>").into_bytes(),
        stream(content, ""),
        format!("<< /Type /Font /Subtype /Type1 /BaseFont /Test /Encoding << /Differences [65 /A /alpha] >> /FirstChar 65 /LastChar 66 /Widths {widths} /FontDescriptor 6 0 R /ToUnicode 8 0 R >>").into_bytes(),
        format!("<< /Type /FontDescriptor /FontName /Test /Flags 4 {descriptor}/FontFile3 7 0 R >>")
            .into_bytes(),
        stream(&program, "/Subtype /Type1C"),
        stream(
            &[
                b"begincmap 1 begincodespacerange <00> <ff> endcodespacerange 2 beginbfchar "
                    .to_vec(),
                bfchar.to_vec(),
                b" endbfchar endcmap".to_vec(),
            ]
            .concat(),
            "",
        ),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        bytes.extend_from_slice(body);
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
    ByteStore::new(SourceId::new(818), Arc::<[u8]>::from(bytes))
}

const SPACED_CFF: &str = "0100040100010101055465737400010101131d00000030111d000000560f1d0000005d100001010106616c70686100000004010102101e1f0e8b8b15f8888b8bf888fc888b050e8b8b15f8888b8bf888fc888b050e0e000022018700010003414243";

fn spaced_fixture(content: &[u8]) -> ByteStore {
    spaced_fixture_with(content, "[600 600 600]", "")
}

fn spaced_fixture_with(content: &[u8], widths: &str, descriptor: &str) -> ByteStore {
    spaced_fixture_program(
        content,
        (widths, descriptor),
        SPACED_CFF,
        b"<41> <0041> <42> <0042> <43> <0020>",
    )
}

const NAMELESS_CFF: &str = "0100040100010101055465737400010101131d00000030111d000000560f1d0000005d100001010106673132333400000004010102101e1f0e8b8b15f8888b8bf888fc888b050e8b8b15f8888b8bf888fc888b050e0e000022018700010003414243";

fn spaced_fixture_program(
    content: &[u8],
    (widths, descriptor): (&str, &str),
    program: &str,
    bfchar: &[u8],
) -> ByteStore {
    let program: Vec<u8> = program
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect();
    let objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /MediaBox [0 0 200 200] /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream(content, ""),
        format!("<< /Type /Font /Subtype /Type1 /BaseFont /Test /Encoding << /Differences [65 /A /alpha /space] >> /FirstChar 65 /LastChar 67 /Widths {widths} /FontDescriptor 6 0 R /ToUnicode 8 0 R >>").into_bytes(),
        format!("<< /Type /FontDescriptor /FontName /Test /Flags 4 {descriptor}/FontFile3 7 0 R >>").into_bytes(),
        stream(&program, "/Subtype /Type1C"),
        stream(
            &[
                format!(
                    "begincmap 1 begincodespacerange <00> <ff> endcodespacerange {} beginbfchar ",
                    bfchar.split(|byte| *byte == b' ').count() / 2
                )
                .into_bytes(),
                bfchar.to_vec(),
                b" endbfchar endcmap".to_vec(),
            ]
            .concat(),
            "",
        ),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        bytes.extend_from_slice(body);
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
    ByteStore::new(SourceId::new(819), Arc::<[u8]>::from(bytes))
}

fn read(editor: &mut Editor) {
    let view = pdf_session::interpret_page_grouped(
        editor.source().unwrap(),
        0,
        b"",
        editor.grouping(0).as_deref(),
    )
    .unwrap();
    assert_eq!(view.index.report.clusters_outside_the_grouping, 0);
    editor.adopt_page(0, Arc::new(view));
}

fn editor() -> Editor {
    let mut editor = Editor::open(fixture(
        b"BT /F1 24 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET BT /F1 24 Tf 1 0 0 1 10 30 Tm (AA) Tj ET",
    ))
    .unwrap();
    read(&mut editor);
    assert_eq!(editor.leaf(0).unwrap().overlay.blocks.len(), 2);
    editor
}

const REFLOW: &[u8] =
    b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAACAAAA) Tj ET BT /F1 10 Tf 1 0 0 1 10 40 Tm (BB) Tj ET";

fn reflow_source() -> ByteStore {
    spaced_fixture_with(REFLOW, "[600 600 600]", "/Ascent 800 /Descent -200 ")
}

fn top_block_rows(view: &pdf_session::PageView) -> Vec<Vec<pdf_edit::ClusterRef>> {
    let baseline = |block: &pdf_semantics::Block| {
        block
            .lines
            .first()
            .and_then(|line| view.index.lines[*line].clusters.first())
            .map_or(f64::NEG_INFINITY, |cluster| {
                view.index.clusters[*cluster].baseline.y
            })
    };
    let block = view
        .index
        .blocks
        .iter()
        .max_by(|one, other| baseline(one).total_cmp(&baseline(other)))
        .unwrap();
    block
        .lines
        .iter()
        .map(|line| {
            view.index.lines[*line]
                .clusters
                .iter()
                .map(|cluster| {
                    let cluster = &view.index.clusters[*cluster];
                    pdf_edit::ClusterRef {
                        anchor: pdf_edit::SourceAnchor::of(&view.graph.atoms[cluster.atom].id),
                        glyphs: cluster.glyphs.clone(),
                    }
                })
                .collect()
        })
        .collect()
}

fn pens(source: &ByteStore) -> Vec<(u32, f64, f64)> {
    let view = pdf_session::interpret_page(source, 0).unwrap();
    let mut out = Vec::new();
    for atom in &view.graph.atoms {
        if let pdf_paint::PaintAtomKind::Text(text) = &atom.kind {
            for glyph in &text.glyphs {
                let pen = text.state.ctm.value.transform(pdf_paint::Point {
                    x: glyph.text_matrix.e,
                    y: glyph.text_matrix.f,
                });
                out.push((glyph.code.value, pen.x, pen.y));
            }
        }
    }
    out
}

fn assert_pens(got: &[(u32, f64, f64)], wanted: &[(u32, f64, f64)]) {
    assert_eq!(got.len(), wanted.len(), "{got:?}");
    for (one, other) in got.iter().zip(wanted) {
        assert_eq!(one.0, other.0, "{got:?}");
        assert!(
            (one.1 - other.1).abs() < 1e-6 && (one.2 - other.2).abs() < 1e-6,
            "{got:?}"
        );
    }
}

fn rewrite_block(
    frame: (f64, f64),
    from: (usize, usize),
    to: (usize, usize),
    text: &str,
) -> Result<(ByteStore, pdf_cli::Session), String> {
    let source = reflow_source();
    let mut session = pdf_cli::Session::new(source.clone(), b"");
    let view = session.page(0).map_err(|error| error.to_string())?;
    let rows = top_block_rows(&view);
    let plan = session
        .plan(&pdf_edit::Command::RewriteBlock {
            paragraph: pdf_edit::ParagraphLayout::default(),
            page_index: 0,
            rows,
            frame,
            edges: (0, 0),
            breaks: None,
            frame_declared: true,
            range: pdf_edit::BlockRange::Between { from, to },
            text: text.to_owned(),
        })
        .map_err(|error| error.to_string())?;
    let edited = plan
        .commit(&source, b"")
        .map_err(|error| error.to_string())?;
    session.apply(plan).map_err(|error| error.to_string())?;
    Ok((edited, session))
}

const A: u32 = 0x41;
const SPACE: u32 = 0x43;
const B: u32 = 0x42;

#[test]
fn a_block_reflows_into_a_narrower_frame_and_nothing_else_moves() {
    let before = pens(&reflow_source());
    let (edited, mut session) = rewrite_block((10.0, 40.0), (0, 9), (0, 9), "").unwrap();
    let after = pens(&edited);
    let (block, other): (Vec<_>, Vec<_>) = after.iter().partition(|pen| pen.0 != B);
    assert_pens(
        &block,
        &[
            (A, 10.0, 150.0),
            (A, 16.0, 150.0),
            (A, 22.0, 150.0),
            (A, 28.0, 150.0),
            (SPACE, 34.0, 150.0),
            (A, 10.0, 140.0),
            (A, 16.0, 140.0),
            (A, 22.0, 140.0),
            (A, 28.0, 140.0),
        ],
    );
    let untouched: Vec<_> = before.iter().filter(|pen| pen.0 == B).copied().collect();
    assert_pens(&other, &untouched);
    let view = session.page(0).unwrap();
    assert_eq!(view.index.blocks.len(), 2);
    assert_eq!(
        top_block_rows(&view)
            .iter()
            .map(Vec::len)
            .collect::<Vec<_>>(),
        [5, 4]
    );
}

#[test]
fn typing_at_the_end_wraps_onto_a_third_line() {
    let (edited, _) = rewrite_block((10.0, 40.0), (0, 9), (0, 9), " AAAA").unwrap();
    let lines: Vec<f64> = pens(&edited)
        .iter()
        .filter(|pen| pen.0 == A)
        .map(|pen| pen.2)
        .collect();
    assert_eq!(lines.len(), 12, "no letter lost");
    assert_eq!(
        lines.iter().filter(|y| (**y - 130.0).abs() < 1e-6).count(),
        4,
        "the third word is alone on the third line"
    );
}

#[test]
fn a_paragraph_break_starts_a_new_line_with_the_rest() {
    let (edited, _) = rewrite_block((10.0, 100.0), (0, 4), (0, 4), "\n").unwrap();
    let block: Vec<_> = pens(&edited).into_iter().filter(|pen| pen.0 != B).collect();
    assert_pens(
        &block,
        &[
            (A, 10.0, 150.0),
            (A, 16.0, 150.0),
            (A, 22.0, 150.0),
            (A, 28.0, 150.0),
            (SPACE, 10.0, 140.0),
            (A, 16.0, 140.0),
            (A, 22.0, 140.0),
            (A, 28.0, 140.0),
            (A, 34.0, 140.0),
        ],
    );
}

#[test]
fn a_block_rewrite_is_refused_by_name_and_writes_nothing() {
    let refusal = rewrite_block((40.0, 10.0), (0, 9), (0, 9), "")
        .err()
        .unwrap();
    assert!(refusal.contains("frame has no width"), "{refusal}");
    assert!(rewrite_block((10.0, 40.0), (0, 9), (0, 9), "X").is_err());
}

#[test]
fn a_typed_thai_vowel_joins_its_letter_and_the_caret_goes_after_the_cluster() {
    let mut written = Editor::open(fixture_font(
        b"BT /F1 24 Tf 1 0 0 1 10 150 Tm (ABAA) Tj ET",
        b"<41> <0E01> <42> <0E34>",
        "",
        "[600 0]",
    ))
    .unwrap();
    read(&mut written);
    let leaf = written.leaf(0).unwrap();
    assert_eq!(
        leaf.view.index.lines[0].clusters.len(),
        3,
        "control: the file's own กิ is one cluster"
    );
    let mut thai = Editor::open(fixture_font(
        b"BT /F1 24 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET",
        b"<41> <0E01> <42> <0E34>",
        "",
        "[600 0]",
    ))
    .unwrap();
    read(&mut thai);
    assert!(matches!(
        thai.type_text(0, 0, 1, 2, "กิ"),
        Applied::Changed { .. }
    ));
    read(&mut thai);
    let leaf = thai.leaf(0).unwrap();
    let row = &leaf.view.index.lines[0];
    assert_eq!(row.clusters.len(), 4, "the typed vowel joins its letter");
    assert_eq!(leaf.view.index.clusters[row.clusters[1]].marks, 1);
    let ungrouped = pdf_session::interpret_page(thai.source().unwrap(), 0).unwrap();
    assert_eq!(ungrouped.index.lines[0].clusters.len(), 4);
    assert_eq!(thai.landed_caret(), Some((0, 2)));
    assert_eq!(1 + "กิ".chars().count(), 3);
    assert!(matches!(
        thai.type_text(0, 0, 0, 0, "\t"),
        Applied::Refused(_)
    ));
    assert_eq!(thai.landed_caret(), None);
}

#[test]
fn a_typed_letter_with_no_width_is_set_in_a_face_that_has_one() {
    let mut latin = Editor::open(fixture_font(
        b"BT /F1 24 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET",
        b"<41> <0041> <42> <0042>",
        "",
        "[600 0]",
    ))
    .unwrap();
    read(&mut latin);
    let rect = latin.frame_boxes(0)[0];
    declare_frame(&mut latin, 0, 0, rect);
    let typed = latin.type_text(0, 0, 1, 2, "B");
    assert!(matches!(typed, Applied::Changed { .. }), "{typed:?}");
    read(&mut latin);
    let lines: Vec<usize> = latin
        .block_reading(0, 0)
        .unwrap()
        .lines
        .iter()
        .map(|line| line.clusters.len())
        .collect();
    assert_eq!(lines, [3, 1]);
    assert_eq!(
        latin.copy_text(0, 0, (0, 0), (1, 1)).as_deref(),
        Some("ABAA")
    );
    let pens = pens(latin.source().unwrap());
    let mut row: Vec<f64> = pens
        .iter()
        .filter(|pen| (pen.2 - 150.0).abs() < 1e-9)
        .map(|pen| pen.1)
        .collect();
    row.sort_by(f64::total_cmp);
    assert_eq!(row.len(), 3, "{pens:?}");
    assert!(
        row.windows(2).all(|pair| pair[1] - pair[0] > 1.0),
        "{pens:?}"
    );
    assert!(matches!(latin.undo(), Applied::Changed { .. }));
    read(&mut latin);
    assert!(matches!(
        latin.type_text(0, 0, 1, 2, "A"),
        Applied::Changed { .. }
    ));
}

#[test]
fn retyping_a_kerned_run_keeps_its_kerning() {
    let mut kerned = Editor::open(fixture(
        b"BT /F1 24 Tf 1 0 0 1 10 150 Tm [(AA) -500 (AA)] TJ ET",
    ))
    .unwrap();
    read(&mut kerned);
    let before = glyph_placement_signature(&kerned.leaf(0).unwrap().view.graph);
    assert_eq!(before.len(), 4);
    assert!(matches!(
        kerned.type_text(0, 0, 0, 1, "B"),
        Applied::Changed { .. }
    ));
    read(&mut kerned);
    let after = glyph_placement_signature(&kerned.leaf(0).unwrap().view.graph);
    assert_eq!(after.len(), 4);
    assert_ne!(after[0], before[0], "control: the first glyph did change");
    assert_eq!(after[1..], before[1..]);
    assert!(laid_out(&kerned), "{}", kerned.status());
}

fn top_block(view: &pdf_session::PageView) -> usize {
    let baseline = |block: &pdf_semantics::Block| {
        block
            .lines
            .first()
            .and_then(|line| view.index.lines[*line].clusters.first())
            .map_or(f64::NEG_INFINITY, |cluster| {
                view.index.clusters[*cluster].baseline.y
            })
    };
    (0..view.index.blocks.len())
        .max_by(|one, other| {
            baseline(&view.index.blocks[*one]).total_cmp(&baseline(&view.index.blocks[*other]))
        })
        .unwrap()
}

#[test]
fn typing_past_the_edge_wraps_and_the_frame_grows_one_line_until_undone() {
    let mut editor = Editor::open(reflow_source()).unwrap();
    read(&mut editor);
    let view = editor.leaf(0).unwrap().view.clone();
    let block = top_block(&view);
    let line = view.index.blocks[block].lines[0];
    let frame = editor.frame_boxes(0)[block];
    assert!(matches!(
        editor.type_text(0, line, 9, 9, " AAAA"),
        Applied::Changed { .. }
    ));
    assert_eq!(
        editor.landed_caret(),
        Some((1, 4)),
        "after the last A, on the new line"
    );
    read(&mut editor);
    let grown = editor.frame_boxes(0)[block];
    assert_eq!(grown[..3], frame[..3], "width and top are the person's");
    assert!(
        (grown[3] - frame[3] - 10.0).abs() < 1e-6,
        "{frame:?} -> {grown:?}"
    );
    let rows = &editor.leaf(0).unwrap().view.index.blocks[block].lines;
    assert_eq!(rows.len(), 2);

    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    read(&mut editor);
    assert_eq!(editor.frame_boxes(0)[block], frame);
    assert_eq!(
        editor.leaf(0).unwrap().view.index.blocks[block].lines.len(),
        1
    );
}

#[test]
fn a_word_typed_at_the_end_of_a_one_line_block_wraps_inside_the_frame_the_person_set() {
    let mut editor = Editor::open(reflow_source()).unwrap();
    read(&mut editor);
    let view = editor.leaf(0).unwrap().view.clone();
    let block = top_block(&view);
    let line = view.index.blocks[block].lines[0];
    let rect = editor.frame_boxes(0)[block];
    declare_frame(&mut editor, 0, block, rect);
    let typed = editor.type_text(0, line, 9, 9, " AAAA");
    assert!(
        matches!(typed, Applied::Changed { .. }),
        "{typed:?} \u{2014} {}",
        editor.status()
    );
    assert_eq!(
        editor.landed_caret(),
        Some((1, 4)),
        "after the last A, on the new line"
    );
    read(&mut editor);
    let grown = editor.frame_boxes(0)[block];
    assert_eq!(
        grown[..3],
        rect[..3],
        "the width and the top are the person's"
    );
    assert!(
        (grown[3] - rect[3] - 10.0).abs() < 1e-6,
        "one line taller, no wider: {rect:?} -> {grown:?}"
    );
    assert_eq!(
        editor.leaf(0).unwrap().view.index.blocks[block].lines.len(),
        2
    );
    for pen in pens(editor.source().unwrap()) {
        if pen.0 == A {
            assert!(pen.1 + 6.0 <= rect[2] + 1e-6, "a letter outside: {pen:?}");
        }
    }
}

#[test]
fn a_frame_dragged_narrower_than_a_word_splits_it_and_keeps_its_width() {
    let mut editor = Editor::open(reflow_source()).unwrap();
    read(&mut editor);
    let block = top_block(&editor.leaf(0).unwrap().view.clone());
    let rect = editor.frame_boxes(0)[block];
    let narrow = [rect[0], rect[1], rect[0] + 25.0, rect[3]];
    declare_frame(&mut editor, 0, block, narrow);
    read(&mut editor);
    let rows = editor.leaf(0).unwrap().view.index.blocks[block]
        .lines
        .clone();
    let last = *rows.last().unwrap();
    let end = editor.leaf(0).unwrap().view.index.lines[last]
        .clusters
        .len();
    let typed = editor.type_text(0, last, end, end, "A");
    assert!(
        matches!(typed, Applied::Changed { .. }),
        "{typed:?} \u{2014} {}",
        editor.status()
    );
    read(&mut editor);
    let after = editor.frame_boxes(0)[block];
    assert!(
        (after[2] - narrow[2]).abs() < 1e-9 && (after[0] - narrow[0]).abs() < 1e-9,
        "the frame the person dragged keeps its width: {narrow:?} -> {after:?}"
    );
    let letters: Vec<(f64, f64)> = pens(editor.source().unwrap())
        .into_iter()
        .filter(|pen| pen.0 == A)
        .map(|pen| (pen.1, pen.2))
        .collect();
    assert_eq!(letters.len(), 9, "no letter lost");
    for (x, _) in &letters {
        assert!(x + 6.0 <= narrow[2] + 1e-6, "a letter past the edge: {x}");
    }
    let mut baselines: Vec<f64> = Vec::new();
    for (_, y) in &letters {
        if !baselines.iter().any(|seen| (seen - y).abs() < 1e-6) {
            baselines.push(*y);
        }
    }
    assert_eq!(
        baselines.len(),
        3,
        "four, four and the split letter: {letters:?}"
    );
}

#[test]
fn enter_splits_the_paragraph_and_the_caret_starts_the_new_line() {
    let mut editor = Editor::open(reflow_source()).unwrap();
    read(&mut editor);
    let view = editor.leaf(0).unwrap().view.clone();
    let block = top_block(&view);
    let line = view.index.blocks[block].lines[0];
    let frame = editor.frame_boxes(0)[block];
    assert!(matches!(
        editor.type_text(0, line, 4, 4, "\n"),
        Applied::Changed { .. }
    ));
    assert_eq!(editor.landed_caret(), Some((1, 0)));
    read(&mut editor);
    assert_eq!(
        editor.leaf(0).unwrap().view.index.blocks[block].lines.len(),
        2
    );
    assert!((editor.frame_boxes(0)[block][3] - frame[3] - 10.0).abs() < 1e-6);
}

#[test]
fn delete_split_empty_block_and_history_keep_ownership_and_frame() {
    let mut editor = editor();
    let frames = editor.frame_boxes(0).to_vec();
    let original = glyph_placement_signature(&editor.leaf(0).unwrap().view.graph);
    assert!(matches!(editor.delete(0, 0, 1, 3), Applied::Changed { .. }));
    read(&mut editor);
    assert_eq!(editor.frame_boxes(0), frames);
    assert_eq!(editor.leaf(0).unwrap().view.index.blocks.len(), 2);
    assert_eq!(
        editor.leaf(0).unwrap().view.index.lines[0].clusters.len(),
        2
    );
    let anchors = editor.leaf(0).unwrap().overlay.blocks[0].anchors.clone();
    assert!(matches!(
        editor.delete_block(0, &anchors),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    let leaf = editor.leaf(0).unwrap();
    assert_eq!(leaf.overlay.blocks.len(), 2);
    assert!(leaf.overlay.blocks[0].anchors.is_empty());
    assert_eq!(leaf.overlay.blocks[0].box_pixels, frames[0]);
    assert_eq!(
        leaf.view.index.lines[0].clusters.len(),
        2,
        "only the second paragraph remains"
    );
    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    read(&mut editor);
    assert_eq!(
        editor.leaf(0).unwrap().view.index.lines[0].clusters.len(),
        2
    );
    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    read(&mut editor);
    assert_eq!(
        glyph_placement_signature(&editor.leaf(0).unwrap().view.graph),
        original
    );
    assert_eq!(editor.frame_boxes(0), frames);
    assert!(matches!(editor.redo(), Applied::Changed { .. }));
    read(&mut editor);
    assert_eq!(editor.frame_boxes(0), frames);
    assert_eq!(
        editor.leaf(0).unwrap().view.index.lines[0].clusters.len(),
        2
    );
}

#[test]
fn a_deleted_block_leaves_nothing_to_point_at_once_the_document_is_reopened() {
    let mut editor = editor();
    assert_eq!(editor.leaf(0).unwrap().overlay.blocks.len(), 2);
    let anchors = editor.leaf(0).unwrap().overlay.blocks[0].anchors.clone();
    assert!(matches!(
        editor.delete_block(0, &anchors),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    let leaf = editor.leaf(0).unwrap();
    assert!(leaf.overlay.blocks[0].anchors.is_empty());
    assert!(!pdf_app_stands(&leaf.overlay, 0));

    let mut reopened = Editor::open(editor.source().unwrap().clone()).unwrap();
    read(&mut reopened);
    let blocks = &reopened.leaf(0).unwrap().overlay.blocks;
    assert_eq!(blocks.len(), 1, "no empty block in the saved document");
    assert!(!blocks[0].anchors.is_empty());
}

fn pdf_app_stands(overlay: &crate::Overlay, block: usize) -> bool {
    crate::view::stands(!overlay.blocks[block].anchors.is_empty(), block, None)
}

#[test]
fn moving_over_another_paragraph_and_undoing_never_adopts_its_text() {
    let mut editor = editor();
    let frames = editor.frame_boxes(0).to_vec();
    let anchors = editor.leaf(0).unwrap().overlay.blocks[0].anchors.clone();
    assert!(matches!(
        editor.move_block(0, &anchors, 0.0, 120.0),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    let leaf = editor.leaf(0).unwrap();
    assert_eq!(leaf.overlay.blocks.len(), 2);
    assert_eq!(
        leaf.view.index.lines[leaf.overlay.blocks[0].lines[0]]
            .clusters
            .len(),
        4
    );
    assert_eq!(
        leaf.view.index.lines[leaf.overlay.blocks[1].lines[0]]
            .clusters
            .len(),
        2
    );
    assert_eq!(
        editor.frame_boxes(0)[0],
        crate::view::box_shifted(frames[0], 0.0, 120.0)
    );
    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    read(&mut editor);
    assert_eq!(editor.frame_boxes(0), frames);
}

#[test]
fn refused_edit_keeps_frames_and_a_resize_undo_does_not_edit_pdf_bytes() {
    let mut editor = editor();
    let frames = editor.frame_boxes(0).to_vec();
    let source = editor.source().unwrap().as_bytes().to_vec();
    assert!(matches!(editor.delete(0, 999, 0, 1), Applied::Refused(_)));
    assert_eq!(editor.frame_boxes(0), frames);
    assert!(!editor.can_undo());
    let mut larger = frames[0];
    larger[2] += 30.0;
    editor.preview_frame(0, 0, larger);
    editor.finish_frame_resize(0, 0, frames[0]);
    assert!(editor.can_undo());
    assert!(matches!(editor.undo(), Applied::Unchanged));
    assert_eq!(editor.frame_boxes(0), frames);
    assert_eq!(editor.source().unwrap().as_bytes(), source);
    assert!(editor.can_redo());
    assert!(matches!(editor.redo(), Applied::Unchanged));
    assert_eq!(editor.frame_boxes(0)[0], larger);
}

#[test]
fn type_replace_insert_and_undo_preserve_known_positions_and_fixed_frame() {
    let mut editor = editor();
    let frames = editor.frame_boxes(0).to_vec();
    let original = glyph_placement_signature(&editor.leaf(0).unwrap().view.graph);
    assert!(
        matches!(editor.type_text(0, 0, 1, 3, "B"), Applied::Changed { .. }),
        "{}",
        editor.status()
    );
    read(&mut editor);
    assert_eq!(editor.frame_boxes(0), frames);
    assert_eq!(
        editor.leaf(0).unwrap().view.index.lines[0].clusters.len(),
        3
    );
    assert!(
        matches!(editor.type_text(0, 0, 2, 2, "A"), Applied::Changed { .. }),
        "{}",
        editor.status()
    );
    read(&mut editor);
    let graph = &editor.leaf(0).unwrap().view.graph;
    let glyphs: Vec<_> = graph
        .atoms
        .iter()
        .filter_map(|atom| match &atom.kind {
            pdf_paint::PaintAtomKind::Text(text) => Some(&text.glyphs),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(
        glyphs
            .iter()
            .map(|glyph| glyph.code.value)
            .collect::<Vec<_>>(),
        [65, 66, 65, 65, 65, 65]
    );
    for (glyph, x) in glyphs.iter().zip([10.0, 24.4, 38.8, 53.2, 10.0, 24.4]) {
        assert!(
            (glyph.matrix.e - x).abs() < 1e-8,
            "{} != {x}",
            glyph.matrix.e
        );
    }
    assert_eq!(editor.frame_boxes(0), frames);
    assert_eq!(editor.export().unwrap().placements, 6);
    editor.undo();
    editor.undo();
    read(&mut editor);
    assert_eq!(
        glyph_placement_signature(&editor.leaf(0).unwrap().view.graph),
        original
    );
    editor.redo();
    editor.redo();
    read(&mut editor);
    assert_eq!(editor.frame_boxes(0), frames);
}

#[test]
fn a_font_whose_codes_are_thai_can_be_typed_in() {
    let mut editor = Editor::open(fixture_saying(
        b"BT /F1 24 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET",
        b"<41> <0E01> <42> <0E02>",
    ))
    .unwrap();
    read(&mut editor);
    let frames = editor.frame_boxes(0).to_vec();

    assert!(
        matches!(editor.type_text(0, 0, 1, 2, "ข"), Applied::Changed { .. }),
        "{}",
        editor.status()
    );
    read(&mut editor);
    let graph = &editor.leaf(0).unwrap().view.graph;
    let codes: Vec<u32> = graph
        .atoms
        .iter()
        .filter_map(|atom| match &atom.kind {
            pdf_paint::PaintAtomKind::Text(text) => Some(&text.glyphs),
            _ => None,
        })
        .flatten()
        .map(|glyph| glyph.code.value)
        .collect();
    assert_eq!(codes, [65, 66, 65, 65], "the second glyph became ข");
    assert_eq!(editor.frame_boxes(0), frames, "the frame did not move");

    assert!(
        matches!(editor.type_text(0, 0, 1, 2, "\t"), Applied::Refused(_)),
        "{}",
        editor.status()
    );
}

#[test]
fn a_refused_typing_changes_neither_page_frame_nor_history() {
    let mut editor = editor();
    let bytes = editor.source().unwrap().as_bytes().to_vec();
    let frames = editor.frame_boxes(0).to_vec();
    for typed in ["\t", "A\tA"] {
        assert!(
            matches!(editor.type_text(0, 0, 4, 4, typed), Applied::Refused(_)),
            "{typed}: {}",
            editor.status()
        );
        assert_eq!(editor.source().unwrap().as_bytes(), bytes);
        assert_eq!(editor.frame_boxes(0), frames);
        assert!(!editor.can_undo());
    }
    let mut larger = frames[0];
    larger[2] += 30.0;
    editor.preview_frame(0, 0, larger);
    editor.finish_frame_resize(0, 0, frames[0]);
    assert!(
        matches!(editor.type_text(0, 0, 4, 4, "A"), Applied::Changed { .. }),
        "{}",
        editor.status()
    );
    read(&mut editor);
    assert_eq!(editor.frame_boxes(0)[0], larger);
}

#[test]
#[ignore = "writes fixtures only into an explicitly supplied probe directory"]
fn external_text_probe_artifacts() {
    let directory = std::path::PathBuf::from(
        std::env::var("PANPDF_TEXT_PROBE_DIR")
            .expect("set PANPDF_TEXT_PROBE_DIR to a scratch directory"),
    );
    std::fs::create_dir_all(&directory).unwrap();
    let mut editor = editor();
    std::fs::write(
        directory.join("before.pdf"),
        editor.source().unwrap().as_bytes(),
    )
    .unwrap();
    assert!(matches!(
        editor.type_text(0, 0, 1, 3, "B"),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    std::fs::write(
        directory.join("replaced.pdf"),
        editor.export().unwrap().bytes,
    )
    .unwrap();
    assert!(matches!(
        editor.type_text(0, 0, 2, 2, "A"),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    std::fs::write(
        directory.join("inserted.pdf"),
        editor.export().unwrap().bytes,
    )
    .unwrap();
    editor.undo();
    editor.undo();
    read(&mut editor);
    std::fs::write(directory.join("undone.pdf"), editor.export().unwrap().bytes).unwrap();
}

#[test]
fn a_second_character_typed_where_the_caret_lands_on_a_space_is_accepted() {
    let mut editor = Editor::open(spaced_fixture(
        b"BT /F1 24 Tf 1 0 0 1 10 150 Tm (ACA) Tj ET",
    ))
    .unwrap();
    read(&mut editor);
    assert_eq!(
        editor.leaf(0).unwrap().view.index.lines[0].clusters.len(),
        3,
        "the fixture is A, a space, A"
    );
    widen_the_frame(&mut editor);
    let opened = [
        "code=65 glyph=Some(1) says=\"A\" at=10.0,150.0",
        "code=67 glyph=Some(3) says=\" \" at=24.4,150.0",
        "code=65 glyph=Some(1) says=\"A\" at=38.8,150.0",
    ];
    let both_typed = [
        "code=66 glyph=Some(2) says=\"B\" at=10.0,150.0",
        "code=66 glyph=Some(2) says=\"B\" at=24.4,150.0",
        "code=67 glyph=Some(3) says=\" \" at=38.8,150.0",
        "code=65 glyph=Some(1) says=\"A\" at=53.2,150.0",
    ];
    assert_eq!(said(&mut editor), opened);

    assert!(
        matches!(editor.type_text(0, 0, 0, 1, "B"), Applied::Changed { .. }),
        "the first character: {}",
        editor.status()
    );
    read(&mut editor);

    let second = editor.type_text(0, 0, 1, 1, "B");
    assert!(
        matches!(second, Applied::Changed { .. }),
        "typing where the caret lands on a space: {second:?} · {}",
        editor.status()
    );
    read(&mut editor);
    assert_eq!(
        said(&mut editor),
        both_typed,
        "a B was inserted at the caret, and the space and the A moved along"
    );

    assert!(editor.can_undo());
    assert!(!editor.can_redo());
    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    read(&mut editor);
    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    read(&mut editor);
    assert_eq!(said(&mut editor), opened, "two undos, the opened page");
}

#[test]
fn a_space_still_advances_the_pen_after_it_is_typed_over() {
    let mut editor = Editor::open(spaced_fixture(
        b"BT /F1 24 Tf 1 0 0 1 10 150 Tm (ACA) Tj ET",
    ))
    .unwrap();
    read(&mut editor);
    widen_the_frame(&mut editor);

    assert!(
        matches!(editor.type_text(0, 0, 2, 3, "B"), Applied::Changed { .. }),
        "after the space: {}",
        editor.status()
    );
    read(&mut editor);
    assert_eq!(
        said(&mut editor),
        [
            "code=65 glyph=Some(1) says=\"A\" at=10.0,150.0",
            "code=67 glyph=Some(3) says=\" \" at=24.4,150.0",
            "code=66 glyph=Some(2) says=\"B\" at=38.8,150.0",
        ],
        "the space is untouched and still advances 14.4 pt"
    );

    assert!(
        matches!(editor.type_text(0, 0, 0, 1, " "), Applied::Changed { .. }),
        "typing a space: {}",
        editor.status()
    );
    read(&mut editor);
    assert_eq!(
        said(&mut editor),
        [
            "code=67 glyph=Some(3) says=\" \" at=10.0,150.0",
            "code=67 glyph=Some(3) says=\" \" at=24.4,150.0",
            "code=66 glyph=Some(2) says=\"B\" at=38.8,150.0",
        ],
        "two spaces now, and the B is where it was"
    );
}

#[test]
fn a_second_character_typed_where_the_caret_lands_on_ink_is_accepted() {
    let mut editor = Editor::open(spaced_fixture(
        b"BT /F1 24 Tf 1 0 0 1 10 150 Tm (AAC) Tj ET",
    ))
    .unwrap();
    read(&mut editor);
    widen_the_frame(&mut editor);
    assert!(
        matches!(editor.type_text(0, 0, 0, 1, "A"), Applied::Changed { .. }),
        "the first character: {}",
        editor.status()
    );
    read(&mut editor);
    assert!(
        matches!(editor.type_text(0, 0, 1, 1, "A"), Applied::Changed { .. }),
        "the second character: {}",
        editor.status()
    );
}

#[test]
fn a_refusal_between_an_edit_and_its_undo_leaves_both_steps_saying_what_they_said() {
    let mut editor = Editor::open(spaced_fixture(
        b"BT /F1 24 Tf 1 0 0 1 10 150 Tm (AAA) Tj ET",
    ))
    .unwrap();
    editor.keep_a_ledger(true);
    read(&mut editor);
    widen_the_frame(&mut editor);

    let opened = [
        "code=65 glyph=Some(1) says=\"A\" at=10.0,150.0",
        "code=65 glyph=Some(1) says=\"A\" at=24.4,150.0",
        "code=65 glyph=Some(1) says=\"A\" at=38.8,150.0",
    ];
    let edited = [
        "code=66 glyph=Some(2) says=\"B\" at=10.0,150.0",
        "code=65 glyph=Some(1) says=\"A\" at=24.4,150.0",
        "code=65 glyph=Some(1) says=\"A\" at=38.8,150.0",
    ];
    assert_eq!(
        said(&mut editor),
        opened,
        "the fixture does not say what it should"
    );

    assert!(
        matches!(editor.type_text(0, 0, 0, 1, "B"), Applied::Changed { .. }),
        "the accepted edit: {}",
        editor.status()
    );
    read(&mut editor);
    assert_eq!(said(&mut editor), edited);
    let bytes_before_the_refusal = editor.source().unwrap().as_bytes().to_vec();
    let revision_before_the_refusal = editor.revision();

    let refusal = editor.type_text(0, 0, 1, 2, "\t");
    assert!(
        matches!(refusal, Applied::Refused(_)),
        "the deliberately unsupported input was accepted: {refusal:?}"
    );
    read(&mut editor);
    assert_eq!(
        editor.source().unwrap().as_bytes(),
        bytes_before_the_refusal,
        "the refused command rewrote the document"
    );
    assert_eq!(editor.revision(), revision_before_the_refusal);
    assert_eq!(
        said(&mut editor),
        edited,
        "the refused command changed the page"
    );

    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    read(&mut editor);
    assert_eq!(
        said(&mut editor),
        opened,
        "undo across a refusal landed on different text"
    );
    assert!(editor.can_redo());

    assert!(matches!(editor.redo(), Applied::Changed { .. }));
    read(&mut editor);
    assert_eq!(
        said(&mut editor),
        edited,
        "redo across a refusal landed on different text"
    );

    let records = editor.take_records();
    let names: Vec<(&str, &str)> = records
        .iter()
        .map(|record| (record.command.as_str(), record.outcome.name()))
        .collect();
    assert_eq!(
        names,
        [
            ("Type", "changed"),
            ("Type", "refused"),
            ("Undo", "changed"),
            ("Redo", "changed"),
        ],
        "the sequence the ledger recorded is not the one that was driven"
    );
    assert!(records[1].measured_state_unchanged());
    assert!(records[0].after.can_undo && !records[0].after.can_redo);
    assert!(
        records
            .iter()
            .filter(|record| record.outcome.name() == "changed")
            .all(|record| !record.measured_state_unchanged()),
        "a command that changed the document reported its measured state unmoved"
    );
    assert_eq!(records[1].before.can_undo, records[1].after.can_undo);
}

#[test]
fn what_the_page_says_is_read_from_the_characters_and_not_from_the_grouping() {
    let page = |content: &[u8]| {
        let mut editor = Editor::open(spaced_fixture(content)).unwrap();
        read(&mut editor);
        (said(&mut editor), atoms_behind_the_line(&mut editor))
    };
    let three_a = |content: &[u8]| page(content).0;
    let (original, original_atoms) = page(b"BT /F1 24 Tf 1 0 0 1 10 150 Tm (AAA) Tj ET");

    let other_text = three_a(b"BT /F1 24 Tf 1 0 0 1 10 150 Tm (ABA) Tj ET");
    assert_ne!(
        original, other_text,
        "a wrong character read as the right one"
    );
    assert_eq!(original.len(), other_text.len());
    assert_eq!(
        original
            .iter()
            .map(|line| line.split(" at=").nth(1).unwrap())
            .collect::<Vec<_>>(),
        other_text
            .iter()
            .map(|line| line.split(" at=").nth(1).unwrap())
            .collect::<Vec<_>>(),
        "the control is meant to differ only in the characters"
    );

    let moved = three_a(b"BT /F1 24 Tf 1 0 0 1 40 150 Tm (AAA) Tj ET");
    assert_ne!(original, moved, "text at the wrong place read as unmoved");

    let (regrouped, regrouped_atoms) = page(
        b"BT /F1 24 Tf 1 0 0 1 10 150 Tm <41> Tj 1 0 0 1 24.4 150 Tm <41> Tj \
          1 0 0 1 38.8 150 Tm <41> Tj ET",
    );
    assert_eq!(
        (original_atoms, regrouped_atoms),
        (1, 3),
        "the control does not actually regroup the line, so it proves nothing"
    );
    assert_eq!(
        regrouped, original,
        "the same text in the same places read as different because it was grouped differently"
    );
}

fn atoms_behind_the_line(editor: &mut Editor) -> usize {
    let leaf = editor.leaf(0).unwrap();
    let index = &leaf.view.index;
    index.lines[0]
        .clusters
        .iter()
        .map(|&cluster| index.clusters[cluster].atom)
        .collect::<std::collections::BTreeSet<_>>()
        .len()
}

fn said(editor: &mut Editor) -> Vec<String> {
    said_line(editor, 0)
}

fn said_line(editor: &mut Editor, line: usize) -> Vec<String> {
    let leaf = editor.leaf(0).unwrap();
    let index = &leaf.view.index;
    index.lines[line]
        .clusters
        .iter()
        .map(|&cluster| {
            let cluster = &index.clusters[cluster];
            let pdf_paint::PaintAtomKind::Text(text) = &leaf.view.graph.atoms[cluster.atom].kind
            else {
                panic!("a cluster on a text line belongs to an atom that is not text");
            };
            let glyph = &text.glyphs[cluster.glyphs.start];
            let code = pdf_content::Code {
                value: glyph.code.value,
                byte_len: glyph.code.bytes.len() + glyph.code.completed_bytes,
            };
            let says = text
                .text
                .text_of(code)
                .map_or_else(|| "?".to_owned(), |meaning| meaning.text.clone());
            format!(
                "code={} glyph={:?} says={:?} at={:.1},{:.1}",
                glyph.code.value, glyph.glyph, says, cluster.baseline.x, cluster.baseline.y
            )
        })
        .collect()
}

#[test]
fn the_whole_gesture_from_selecting_text_to_reopening_the_saved_file() {
    let document = || {
        spaced_fixture(
            b"BT /F1 24 Tf 1 0 0 1 10 150 Tm (AACAA) Tj ET \
              BT /F1 24 Tf 1 0 0 1 10 50 Tm (AA) Tj ET",
        )
    };
    let row = |code: u32, glyph: u16, says: &str, x: f64, y: f64| {
        format!(
            "code={code} glyph={:?} says={:?} at={x:.1},{y:.1}",
            Some(glyph),
            says
        )
    };
    let a = |x: f64| row(65, 1, "A", x, 150.0);
    let b = |x: f64| row(66, 2, "B", x, 150.0);
    let space = |x: f64| row(67, 3, " ", x, 150.0);

    let opened = vec![a(10.0), a(24.4), space(38.8), a(53.2), a(67.6)];
    let edited = vec![b(10.0), b(24.4), a(38.8), b(53.2), a(67.6)];
    let control = vec![row(65, 1, "A", 10.0, 50.0), row(65, 1, "A", 24.4, 50.0)];

    let mut editor = Editor::open(document()).unwrap();
    read(&mut editor);
    assert_eq!(said(&mut editor), opened);
    assert_eq!(
        said_line(&mut editor, 1),
        control,
        "the row nothing touches"
    );
    let tight = editor.frame_boxes(0)[0];
    widen_the_frame(&mut editor);

    changed(
        &mut editor.type_text(0, 0, 0, 1, "B"),
        "replace the selection",
    );
    read(&mut editor);
    changed(
        &mut editor.type_text(0, 0, 1, 1, "B"),
        "keep typing at the caret",
    );
    read(&mut editor);
    assert_eq!(
        said(&mut editor),
        vec![b(10.0), b(24.4), a(38.8), space(53.2), a(67.6), a(82.0)],
        "an insertion moved the space and the two As along by one advance"
    );
    changed(
        &mut editor.type_text(0, 0, 3, 4, "B"),
        "type over the space",
    );
    read(&mut editor);
    changed(&mut editor.delete(0, 0, 5, 6), "delete the last cluster");
    read(&mut editor);
    assert_eq!(said(&mut editor), edited);
    assert_eq!(
        said_line(&mut editor, 1),
        control,
        "the other row did not move"
    );

    let bytes = editor.source().unwrap().as_bytes().to_vec();
    let steps = (editor.can_undo(), editor.can_redo());
    let refused = editor.type_text(0, 0, 0, 1, "\t");
    assert!(matches!(refused, Applied::Refused(_)), "{refused:?}");
    assert_eq!(editor.source().unwrap().as_bytes(), bytes);
    assert_eq!((editor.can_undo(), editor.can_redo()), steps);
    read(&mut editor);
    assert_eq!(said(&mut editor), edited, "a refusal changed the page");

    for _ in 0..4 {
        changed(&mut editor.undo(), "undo");
        read(&mut editor);
    }
    assert_eq!(said(&mut editor), opened);
    assert_eq!(said_line(&mut editor, 1), control);
    assert!(matches!(editor.undo(), Applied::Unchanged));
    assert_eq!(editor.frame_boxes(0)[0], tight);
    assert!(!editor.can_undo(), "four commands and a resize, five undos");

    assert!(matches!(editor.redo(), Applied::Unchanged));
    for _ in 0..4 {
        changed(&mut editor.redo(), "redo");
        read(&mut editor);
    }
    assert!(!editor.can_redo());
    assert_eq!(said(&mut editor), edited, "redo put back what undo took");

    let saved = editor.export().expect("the edited document exports").bytes;
    reopen_and_edit_again(&saved, &edited, &control, &b(38.8));
}

fn reopen_and_edit_again(saved: &[u8], edited: &[String], control: &[String], third_typed: &str) {
    let open = |id: u64| {
        let mut editor =
            Editor::open(ByteStore::new(SourceId::new(id), Arc::<[u8]>::from(saved))).unwrap();
        read(&mut editor);
        editor
    };
    let mut reopened = open(820);
    assert_eq!(
        said(&mut reopened),
        edited,
        "the saved file says what was typed"
    );
    assert_eq!(said_line(&mut reopened, 1), control);
    assert!(
        !reopened.can_undo(),
        "a reopened file has no history of its own"
    );

    changed(
        &mut reopened.type_text(0, 0, 2, 3, "B"),
        "edit the reopened file",
    );
    read(&mut reopened);
    let mut again = edited.to_vec();
    again[2] = third_typed.to_owned();
    assert_eq!(
        said(&mut reopened),
        again,
        "the third cluster became a B in the reopened document"
    );

    assert_eq!(said(&mut open(821)), edited);
}

fn changed(applied: &mut Applied, what: &str) {
    assert!(
        matches!(applied, Applied::Changed { .. }),
        "{what}: {applied:?}"
    );
}

#[test]
fn deleting_around_a_space_and_pasting_a_word_the_font_cannot_finish() {
    let mut editor = Editor::open(spaced_fixture(
        b"BT /F1 24 Tf 1 0 0 1 10 150 Tm (AACAA) Tj ET",
    ))
    .unwrap();
    read(&mut editor);
    let a = |x: f64| format!("code=65 glyph=Some(1) says=\"A\" at={x:.1},150.0");
    let b = |x: f64| format!("code=66 glyph=Some(2) says=\"B\" at={x:.1},150.0");
    let space = |x: f64| format!("code=67 glyph=Some(3) says=\" \" at={x:.1},150.0");
    assert_eq!(
        said(&mut editor),
        vec![a(10.0), a(24.4), space(38.8), a(53.2), a(67.6)]
    );

    let bytes = editor.source().unwrap().as_bytes().to_vec();
    let refused = editor.type_text(0, 0, 0, 1, "BB\tBB");
    assert!(matches!(refused, Applied::Refused(_)), "{refused:?}");
    assert_eq!(
        editor.source().unwrap().as_bytes(),
        bytes,
        "part of a refused paste was committed"
    );
    assert!(!editor.can_undo());

    widen_the_frame(&mut editor);
    assert!(
        matches!(
            editor.type_text(0, 0, 0, 1, "BBBB"),
            Applied::Changed { .. }
        ),
        "{}",
        editor.status()
    );
    read(&mut editor);
    assert_eq!(
        said(&mut editor),
        vec![
            b(10.0),
            b(24.4),
            b(38.8),
            b(53.2),
            a(67.6),
            space(82.0),
            a(96.4),
            a(110.8)
        ],
        "four glyphs replaced one, and everything after it moved by three advances"
    );

    assert!(
        matches!(editor.delete(0, 0, 5, 6), Applied::Changed { .. }),
        "{}",
        editor.status()
    );
    read(&mut editor);
    assert_eq!(
        said(&mut editor),
        vec![
            b(10.0),
            b(24.4),
            b(38.8),
            b(53.2),
            a(67.6),
            a(82.0),
            a(96.4)
        ],
        "the space is gone and the two As closed up by one advance"
    );

    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    read(&mut editor);
    assert!(
        matches!(editor.delete(0, 0, 4, 5), Applied::Changed { .. }),
        "{}",
        editor.status()
    );
    read(&mut editor);
    assert_eq!(
        said(&mut editor),
        vec![
            b(10.0),
            b(24.4),
            b(38.8),
            b(53.2),
            space(67.6),
            a(82.0),
            a(96.4)
        ],
        "the A before the space went and the space took its place"
    );
}

#[test]
fn a_frame_says_which_side_a_candidate_leaves_by_and_how_far() {
    use crate::document::{FrameFit, frame_fit};
    use crate::wording::{Lang, Side};
    let frame = [10.0, 20.0, 110.0, 60.0];

    let inside = frame_fit(frame, [12.0, 25.0, 100.0, 55.0], 1e-6);
    assert_eq!(inside.over, [-2.0, -5.0, -10.0, -5.0]);
    assert_eq!(inside.worst(), (Side::Left, -2.0));
    assert!(inside.fits());

    for (bounds, side, named, over) in [
        ([9.0, 25.0, 100.0, 55.0], Side::Left, "left", 1.0),
        ([12.0, 18.0, 100.0, 55.0], Side::Top, "top", 2.0),
        ([12.0, 25.0, 113.0, 55.0], Side::Right, "right", 3.0),
        ([12.0, 25.0, 100.0, 64.0], Side::Bottom, "bottom", 4.0),
    ] {
        let fit = frame_fit(frame, bounds, 1e-6);
        assert_eq!(fit.worst(), (side, over), "{bounds:?}");
        assert!(!fit.fits());
        let why = crate::wording::Refusal::Engine("a reason of the block's own".to_owned());
        let said = fit.refusal_because(why.clone()).say(Lang::English);
        assert!(said.contains(named), "{said}");
        assert!(
            said.contains(&format!("{over:.2}")),
            "the refusal says the number: {}",
            fit.refusal_because(why.clone())
        );
        assert!(
            said.contains("a reason of the block's own"),
            "the refusal says why the line was not wrapped: {said}"
        );
    }

    let fit = FrameFit {
        over: [-1.0, -1.0, 5.0, -1.0],
        allowed: 24.0,
    };
    assert!(fit.fits());
}

#[test]
fn an_inferred_and_a_declared_frame_hold_a_line_to_the_same_edge() {
    let fixture = || {
        let mut editor = Editor::open(spaced_fixture(
            b"BT /F1 24 Tf 1 0 0 1 10 150 Tm (AAA) Tj ET",
        ))
        .unwrap();
        read(&mut editor);
        editor
    };
    let inferred = fixture();
    let rect = inferred.frame_boxes(0)[0];
    let mut declared = fixture();
    declare_frame(&mut declared, 0, 0, rect);
    for (name, mut editor) in [("inferred", inferred), ("declared", declared)] {
        assert_eq!(editor.frame_is_declared(0, 0), name == "declared");
        let typed = editor.type_text(0, 0, 3, 3, "A");
        assert!(
            matches!(typed, Applied::Changed { .. }),
            "{name}: {typed:?}"
        );
        let got: Vec<(f64, f64)> = pens(editor.source().unwrap())
            .into_iter()
            .map(|pen| (pen.1, pen.2))
            .collect();
        let frame = editor.frame_boxes(0)[0];
        assert_placed(
            &got,
            &[(10.0, 150.0), (24.4, 150.0), (38.8, 150.0), (10.0, 121.2)],
        );
        assert!(
            (frame[0] - rect[0]).abs() < 1e-9 && (frame[2] - rect[2]).abs() < 1e-9,
            "{name}: the frame keeps its width: {frame:?}"
        );
    }
}

fn lined(ascent: i32, descent: i32) -> Editor {
    let mut editor = Editor::open(fixture_with(
        b"BT /F1 24 Tf 1 0 0 1 10 150 Tm (AAA) Tj ET",
        b"<41> <0041> <42> <0042>",
        &format!("/Ascent {ascent} /Descent {descent} "),
    ))
    .unwrap();
    read(&mut editor);
    editor
}

fn close(one: [f64; 4], other: [f64; 4]) -> bool {
    one.iter().zip(other).all(|(a, b)| (a - b).abs() < 1e-9)
}

#[test]
fn a_block_is_laid_out_by_its_advances_and_its_fonts_declared_line() {
    let laid = lined(800, -200);
    let leaf = laid.leaf(0).unwrap();
    let block = &leaf.view.index.blocks[0];
    assert!(
        close(block.layout.unwrap(), [10.0, 145.2, 53.2, 169.2]),
        "{:?}",
        block.layout
    );
    assert!(
        close(
            leaf.overlay.blocks[0].layout_pixels,
            [10.0, 30.8, 53.2, 54.8]
        ),
        "{:?}",
        leaf.overlay.blocks[0].layout_pixels
    );
    assert!(close(laid.frame_boxes(0)[0], [10.0, 30.8, 53.2, 54.8]));

    let control = editor();
    let index = &control.leaf(0).unwrap().view.index;
    assert!(
        index
            .clusters
            .iter()
            .all(|cluster| cluster.layout.is_none())
    );
    let bounds = index.blocks[0].bounds.unwrap();
    assert!(
        close(
            index.blocks[0].layout.unwrap(),
            [bounds[0], bounds[1], 67.6, bounds[3]]
        ),
        "{:?}",
        index.blocks[0].layout
    );
}

#[test]
fn ink_that_overshoots_its_line_fits_an_inferred_frame_and_a_widened_one() {
    use crate::document::{FRAME_SLACK, frame_fit};

    let ink_over = |editor: &Editor| {
        let leaf = editor.leaf(0).unwrap();
        let ink = pdf_cli::page_overlay_view(&leaf.view, 1.0).unwrap().blocks[0].box_pixels;
        frame_fit(editor.frame_boxes(0)[0], ink, FRAME_SLACK)
    };

    let mut inferred = lined(400, -200);
    let fit = ink_over(&inferred);
    assert!(!fit.fits(), "the fixture must overshoot: {fit:?}");
    assert_eq!(fit.worst().0, crate::wording::Side::Top);
    assert!((fit.worst().1 - 2.4).abs() < 1e-9, "{fit:?}");
    let typed = inferred.type_text(0, 0, 1, 2, "B");
    assert!(
        matches!(typed, Applied::Changed { .. }),
        "an inferred frame refused ink overshoot: {typed:?} · {}",
        inferred.status()
    );

    let mut widened = lined(400, -200);
    let rect = widened.frame_boxes(0)[0];
    declare_frame(
        &mut widened,
        0,
        0,
        [rect[0], rect[1], rect[2] + 20.0, rect[3]],
    );
    assert!(!ink_over(&widened).fits());
    let typed = widened.type_text(0, 0, 1, 2, "B");
    assert!(
        matches!(typed, Applied::Changed { .. }),
        "a widened frame refused ink overshoot: {typed:?} · {}",
        widened.status()
    );
    read(&mut widened);
    let says: Vec<bool> = said(&mut widened)
        .iter()
        .map(|row| row.contains("says=\"B\""))
        .collect();
    assert_eq!(
        says,
        [false, true, false],
        "only the middle cluster became a B"
    );
}

#[test]
fn a_word_past_the_frame_wraps_at_its_edge_and_an_unlaid_block_is_still_refused() {
    let inferred = lined(800, -200);
    let rect = inferred.frame_boxes(0)[0];
    let mut declared = lined(800, -200);
    declare_frame(&mut declared, 0, 0, rect);
    for (name, mut editor) in [("inferred", inferred), ("declared", declared)] {
        let changed = editor.type_text(0, 0, 3, 3, "A");
        assert!(
            matches!(changed, Applied::Changed { .. }),
            "{name}: {changed:?}"
        );
        assert_eq!(editor.landed_caret(), Some((1, 1)), "{name}");
        read(&mut editor);
        let frame = editor.frame_boxes(0)[0];
        let leaf = editor.leaf(0).unwrap();
        let rows: Vec<usize> = leaf
            .view
            .index
            .lines
            .iter()
            .map(|line| line.clusters.len())
            .collect();
        let laid = leaf.overlay.blocks[0].layout_pixels;
        assert!(
            (frame[0] - rect[0]).abs() < 1e-9 && (frame[2] - rect[2]).abs() < 1e-9,
            "{name}: the frame keeps its width: {frame:?}"
        );
        assert!(frame[3] > rect[3], "{name}: and grows down: {frame:?}");
        assert_eq!(
            rows,
            [3, 1],
            "{name}: three letters fill the line, the fourth wraps"
        );
        assert!(
            laid[2] <= rect[2] + 1e-6,
            "{name}: nothing past the edge: {laid:?}"
        );
    }

    let mut unlined =
        Editor::open(fixture(b"BT /F1 24 Tf 1 0 0 -1 10 150 Tm (AAA) Tj ET")).unwrap();
    read(&mut unlined);
    let before = unlined.source().unwrap().as_bytes().to_vec();
    let refused = unlined.type_text(0, 0, 3, 3, "A");
    assert!(
        matches!(refused, Applied::Refused(_)),
        "control: {refused:?}"
    );
    assert_eq!(unlined.source().unwrap().as_bytes(), before);
}

fn spaced_block(content: &[u8]) -> (Editor, usize) {
    let mut editor = Editor::open(spaced_fixture_with(
        content,
        "[600 600 600]",
        "/Ascent 800 /Descent -200 ",
    ))
    .unwrap();
    read(&mut editor);
    let block = top_block(&editor.leaf(0).unwrap().view);
    (editor, block)
}

fn declared_block(content: &[u8]) -> (Editor, usize) {
    let (mut editor, block) = spaced_block(content);
    let rect = editor.frame_boxes(0)[block];
    declare_frame(&mut editor, 0, block, rect);
    (editor, block)
}

fn placed(editor: &Editor, code: u32) -> Vec<(f64, f64)> {
    pens(editor.source().unwrap())
        .into_iter()
        .filter(|pen| pen.0 == code)
        .map(|pen| (pen.1, pen.2))
        .collect()
}

fn assert_placed(got: &[(f64, f64)], wanted: &[(f64, f64)]) {
    assert_eq!(got.len(), wanted.len(), "{got:?}");
    for (one, other) in got.iter().zip(wanted) {
        assert!(
            (one.0 - other.0).abs() < 1e-6 && (one.1 - other.1).abs() < 1e-6,
            "{got:?} against {wanted:?}"
        );
    }
}

fn row(x: f64, y: f64, count: usize) -> Vec<(f64, f64)> {
    (0..count)
        .map(|index| {
            (
                6.0f64.mul_add(f64::from(u32::try_from(index).unwrap()), x),
                y,
            )
        })
        .collect()
}

#[test]
fn a_long_word_typed_into_a_narrow_frame_wraps_and_stays_one_word() {
    let (mut editor, block) = declared_block(b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET");
    let mut caret = (0, 4);
    for _ in 0..7 {
        let changed = editor.edit(
            0,
            block,
            pdf_edit::BlockRange::Between {
                from: caret,
                to: caret,
            },
            "A",
        );
        assert!(matches!(changed, Applied::Changed { .. }), "{changed:?}");
        caret = editor.landed_caret().expect("a caret after each letter");
    }
    assert_eq!(caret, (2, 3), "eleven letters, four to a line");
    assert!(placed(&editor, SPACE).is_empty(), "no space was ever added");
    assert_placed(
        &placed(&editor, A),
        &[
            row(10.0, 150.0, 4),
            row(10.0, 140.0, 4),
            row(10.0, 130.0, 3),
        ]
        .concat(),
    );
    read(&mut editor);
    assert_eq!(
        editor.copy_text(0, block, (0, 0), (2, 3)).as_deref(),
        Some("AAAAAAAAAAA")
    );
}

#[test]
fn spaces_held_at_the_end_of_a_line_never_leave_the_frame() {
    let (mut editor, block) = declared_block(b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET");
    let mut caret = (0, 4);
    for press in 0..10 {
        let changed = editor.edit(
            0,
            block,
            pdf_edit::BlockRange::Between {
                from: caret,
                to: caret,
            },
            " ",
        );
        assert!(
            matches!(changed, Applied::Changed { .. }),
            "space {press}: {changed:?}"
        );
        caret = editor.landed_caret().expect("a caret after each space");
    }
    let spaces = placed(&editor, SPACE);
    assert_eq!(spaces.len(), 10, "every space pressed is in the file");
    for (x, y) in &spaces {
        assert!(
            *x <= 34.0 + 1e-6,
            "a space starts at ({x}, {y}), right of the frame's edge at 34: {spaces:?}"
        );
    }
    assert!(
        caret.0 > 0,
        "the spaces wrapped: the caret is on row {}",
        caret.0
    );
    let deleted = editor.edit(
        0,
        block,
        pdf_edit::BlockRange::Units {
            at: caret,
            backwards: true,
            count: 1,
        },
        "",
    );
    assert!(matches!(deleted, Applied::Changed { .. }), "{deleted:?}");
    let left = placed(&editor, SPACE);
    assert_eq!(left.len(), 9, "Backspace took one space");
    assert_placed(&left, &spaces[..9]);
    assert_placed(&placed(&editor, A), &row(10.0, 150.0, 4));
}

#[test]
fn a_long_word_typed_into_a_frame_nobody_set_wraps_and_the_frame_keeps_its_width() {
    let (mut editor, block) = spaced_block(b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET");
    assert!(!editor.frame_is_declared(0, block));
    let frame = editor.frame_boxes(0)[block];
    let mut caret = (0, 4);
    for _ in 0..7 {
        let changed = editor.edit(
            0,
            block,
            pdf_edit::BlockRange::Between {
                from: caret,
                to: caret,
            },
            "A",
        );
        assert!(matches!(changed, Applied::Changed { .. }), "{changed:?}");
        caret = editor.landed_caret().expect("a caret after each letter");
    }
    assert_eq!(caret, (2, 3), "eleven letters, four to a line");
    assert_placed(
        &placed(&editor, A),
        &[
            row(10.0, 150.0, 4),
            row(10.0, 140.0, 4),
            row(10.0, 130.0, 3),
        ]
        .concat(),
    );
    read(&mut editor);
    let kept = editor.frame_boxes(0)[block];
    assert!(
        (kept[0] - frame[0]).abs() < 1e-9 && (kept[2] - frame[2]).abs() < 1e-9,
        "the width stays: {frame:?} -> {kept:?}"
    );
}

#[test]
fn a_break_after_a_full_row_is_held_by_the_session_and_backspace_joins_it() {
    let (mut editor, block) = declared_block(b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET");
    let type_at = |editor: &mut Editor, at: (usize, usize), text: &str| {
        let changed = editor.edit(
            0,
            block,
            pdf_edit::BlockRange::Between { from: at, to: at },
            text,
        );
        assert!(
            matches!(changed, Applied::Changed { .. }),
            "{text:?}: {changed:?}"
        );
        editor.landed_caret().expect("a caret")
    };
    let copy = |editor: &mut Editor| {
        read(editor);
        let leaf = editor.leaf(0).unwrap();
        let lines = &leaf.overlay.blocks[block].lines;
        let last = *lines.last().expect("a row");
        let end = (lines.len() - 1, leaf.view.index.lines[last].clusters.len());
        editor.copy_text(0, block, (0, 0), end)
    };
    let caret = type_at(&mut editor, (0, 4), "AAAA");
    assert_eq!(caret, (1, 4), "eight letters, four to a line");
    let caret = type_at(&mut editor, (1, 0), "\n");
    assert_eq!(caret, (1, 0));
    let caret = type_at(&mut editor, caret, "B");
    assert_eq!(caret, (1, 1));
    assert_eq!(copy(&mut editor).as_deref(), Some("AAAA\nBAAAA"));

    let back = |editor: &mut Editor, at: (usize, usize)| {
        let changed = editor.edit(
            0,
            block,
            pdf_edit::BlockRange::Units {
                at,
                backwards: true,
                count: 1,
            },
            "",
        );
        assert!(matches!(changed, Applied::Changed { .. }), "{changed:?}");
        editor.landed_caret().expect("a caret")
    };
    let caret = back(&mut editor, caret);
    assert_eq!(
        copy(&mut editor).as_deref(),
        Some("AAAA\nAAAA"),
        "the letter, not the break"
    );
    let caret = back(&mut editor, caret);
    assert_eq!(caret, (0, 4));
    assert_eq!(
        copy(&mut editor).as_deref(),
        Some("AAAAAAAA"),
        "then the break, and no letter of the row above"
    );
    assert_eq!(placed(&editor, A).len(), 8);

    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    assert_eq!(
        copy(&mut editor).as_deref(),
        Some("AAAA\nAAAA"),
        "undo restores the break"
    );
    let caret = back(&mut editor, (1, 0));
    assert_eq!(caret, (0, 4));
    assert_eq!(copy(&mut editor).as_deref(), Some("AAAAAAAA"));
}

#[test]
fn enter_inside_a_word_and_after_a_wrapped_last_row_read_back_as_pressed() {
    let whole = |editor: &mut Editor, block: usize| {
        read(editor);
        let mut last = (0, 0);
        for line in 0..32 {
            let mut stop = 0;
            while editor.copy_text(0, block, (0, 0), (line, stop)).is_some()
                || (line, stop) == (0, 0)
            {
                last = (line, stop);
                stop += 1;
            }
            if stop == 0 {
                break;
            }
        }
        editor.copy_text(0, block, (0, 0), last)
    };
    let (mut editor, block) = spaced_block(b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (ABAB) Tj ET");
    let changed = editor.edit(
        0,
        block,
        pdf_edit::BlockRange::Between {
            from: (0, 2),
            to: (0, 2),
        },
        "\n",
    );
    assert!(matches!(changed, Applied::Changed { .. }), "{changed:?}");
    assert_eq!(whole(&mut editor, block).as_deref(), Some("AB\nAB"));
    assert_eq!(editor.landed_caret(), Some((1, 0)));

    let (mut wrapped, block) = spaced_block(b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET");
    let changed = wrapped.edit(
        0,
        block,
        pdf_edit::BlockRange::Between {
            from: (0, 4),
            to: (0, 4),
        },
        " AA",
    );
    assert!(matches!(changed, Applied::Changed { .. }), "{changed:?}");
    assert_eq!(whole(&mut wrapped, block).as_deref(), Some("AAAA AA"));
    let end = wrapped.landed_caret().expect("a caret");
    let changed = wrapped.edit(
        0,
        block,
        pdf_edit::BlockRange::Between { from: end, to: end },
        "\n",
    );
    assert!(matches!(changed, Applied::Changed { .. }), "{changed:?}");
    assert_eq!(
        whole(&mut wrapped, block).as_deref(),
        Some("AAAA AA\n"),
        "the wrap above stays a wrap"
    );
}

#[test]
fn a_wrapped_centred_title_stays_one_paragraph_because_the_session_says_so() {
    let (mut editor, block) = spaced_block(b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET");
    let frame = editor.frame_boxes(0)[block];
    declare_frame(&mut editor, 0, block, [0.0, frame[1], 44.0, frame[3]]);
    let whole = |editor: &mut Editor| {
        read(editor);
        let mut last = (0, 0);
        for line in 0..32 {
            let mut stop = 0;
            while editor.copy_text(0, block, (0, 0), (line, stop)).is_some()
                || (line, stop) == (0, 0)
            {
                last = (line, stop);
                stop += 1;
            }
            if stop == 0 {
                break;
            }
        }
        editor.copy_text(0, block, (0, 0), last)
    };
    let changed = editor.edit(
        0,
        block,
        pdf_edit::BlockRange::Between {
            from: (0, 4),
            to: (0, 4),
        },
        " AAA",
    );
    assert!(matches!(changed, Applied::Changed { .. }), "{changed:?}");
    let starts: Vec<f64> = {
        let mut rows: Vec<(f64, f64)> = placed(&editor, A);
        rows.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.total_cmp(&b.0)));
        let mut firsts = Vec::new();
        let mut last_y = f64::NAN;
        for (x, y) in rows {
            if (y - last_y).abs() > 1e-6 || last_y.is_nan() {
                firsts.push(x);
                last_y = y;
            }
        }
        firsts
    };
    assert_eq!(starts.len(), 2, "two rows");
    assert!(
        (starts[0] - starts[1]).abs() > 1.0,
        "centred rows start apart: {starts:?}"
    );
    assert_eq!(whole(&mut editor).as_deref(), Some("AAAA AAA"));

    let end = editor.landed_caret().expect("a caret");
    let changed = editor.edit(
        0,
        block,
        pdf_edit::BlockRange::Between { from: end, to: end },
        "\n",
    );
    assert!(matches!(changed, Applied::Changed { .. }), "{changed:?}");
    assert_eq!(whole(&mut editor).as_deref(), Some("AAAA AAA\n"));
}

#[test]
fn a_kerned_row_keeps_its_kerning_where_its_pairs_survive() {
    let content: &[u8] = b"BT /F1 10 Tf 1 0 0 1 10 150 Tm [(AA) 500 (AA)] TJ ET";
    let (mut end, block) = spaced_block(content);
    assert_placed(
        &placed(&end, A),
        &[(10.0, 150.0), (16.0, 150.0), (17.0, 150.0), (23.0, 150.0)],
    );
    let frame = end.frame_boxes(0)[block];
    declare_frame(&mut end, 0, block, [10.0, frame[1], 150.0, frame[3]]);
    let changed = end.edit(
        0,
        block,
        pdf_edit::BlockRange::Between {
            from: (0, 4),
            to: (0, 4),
        },
        "B",
    );
    assert!(matches!(changed, Applied::Changed { .. }), "{changed:?}");
    assert!(laid_out(&end), "laid out as a block: {}", end.status());
    assert_placed(
        &placed(&end, A),
        &[(10.0, 150.0), (16.0, 150.0), (17.0, 150.0), (23.0, 150.0)],
    );
    assert_placed(&placed(&end, B), &[(29.0, 150.0)]);

    let (mut between, block) = spaced_block(content);
    let frame = between.frame_boxes(0)[block];
    declare_frame(&mut between, 0, block, [10.0, frame[1], 150.0, frame[3]]);
    let changed = between.edit(
        0,
        block,
        pdf_edit::BlockRange::Between {
            from: (0, 2),
            to: (0, 2),
        },
        "B",
    );
    assert!(matches!(changed, Applied::Changed { .. }), "{changed:?}");
    assert!(
        laid_out(&between),
        "laid out as a block: {}",
        between.status()
    );
    assert_placed(
        &placed(&between, A),
        &[(10.0, 150.0), (16.0, 150.0), (28.0, 150.0), (34.0, 150.0)],
    );
    assert_placed(&placed(&between, B), &[(22.0, 150.0)]);

    let (mut tight, block) = spaced_block(content);
    let frame = tight.frame_boxes(0)[block];
    declare_frame(&mut tight, 0, block, [10.0, frame[1], 29.0, frame[3]]);
    let changed = tight.edit(
        0,
        block,
        pdf_edit::BlockRange::Between {
            from: (0, 4),
            to: (0, 4),
        },
        "\n",
    );
    assert!(matches!(changed, Applied::Changed { .. }), "{changed:?}");
    assert!(laid_out(&tight), "laid out as a block: {}", tight.status());
    assert_placed(
        &placed(&tight, A),
        &[(10.0, 150.0), (16.0, 150.0), (17.0, 150.0), (23.0, 150.0)],
    );
}

#[test]
fn a_ligature_is_one_cluster_and_the_block_still_lays_out() {
    let mut editor = Editor::open(fixture_with(
        b"BT /F1 24 Tf 1 0 0 1 10 150 Tm (ABA) Tj ET",
        b"<41> <0041> <42> <00660069>",
        "/Ascent 800 /Descent -200 ",
    ))
    .unwrap();
    read(&mut editor);
    let changed = editor.edit(
        0,
        0,
        pdf_edit::BlockRange::Between {
            from: (0, 1),
            to: (0, 1),
        },
        "\n",
    );
    assert!(matches!(changed, Applied::Changed { .. }), "{changed:?}");
    assert!(
        laid_out(&editor),
        "laid out as a block: {}",
        editor.status()
    );
    read(&mut editor);
    assert_eq!(
        editor.copy_text(0, 0, (0, 0), (1, 2)).as_deref(),
        Some("A\nfiA")
    );
}

fn laid_out(editor: &Editor) -> bool {
    use crate::wording::{Done, Layout, Message};
    matches!(
        editor.status(),
        Message::Done(
            Done::Styled
                | Done::Typed {
                    layout: Layout::InFrame,
                    ..
                }
        )
    )
}

#[test]
fn a_mark_with_its_own_advance_lays_out_with_its_base() {
    let mut editor = Editor::open(spaced_fixture_with(
        b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (ABA) Tj ET",
        "[600 0 600]",
        "/Ascent 800 /Descent -200 ",
    ))
    .unwrap();
    read(&mut editor);
    let rows = &editor.leaf(0).unwrap().view.index.lines;
    assert_eq!(
        rows[0].clusters.len(),
        2,
        "B declares no width: a mark on the first A"
    );
    let changed = editor.edit(
        0,
        0,
        pdf_edit::BlockRange::Between {
            from: (0, 2),
            to: (0, 2),
        },
        "\n",
    );
    assert!(matches!(changed, Applied::Changed { .. }), "{changed:?}");
    assert!(
        laid_out(&editor),
        "laid out as a block: {}",
        editor.status()
    );
}

fn styled(editor: &Editor) -> Vec<(u32, f64, String)> {
    let view = pdf_session::interpret_page(editor.source().unwrap(), 0).unwrap();
    let mut out = Vec::new();
    for atom in &view.graph.atoms {
        if let pdf_paint::PaintAtomKind::Text(text) = &atom.kind {
            for glyph in &text.glyphs {
                out.push((
                    glyph.code.value,
                    text.state.text.font_size.value,
                    format!("{:?}", text.state.fill_color.value),
                ));
            }
        }
    }
    out
}

#[test]
fn a_block_mixing_sizes_and_colours_lays_out_run_by_run() {
    let content: &[u8] =
        b"0 g BT /F1 10 Tf 1 0 0 1 10 150 Tm (AA) Tj 1 0 0 rg /F1 20 Tf (AA) Tj ET \
        BT 1 0 0 1 10 40 Tm (AB) Tj ET";
    let (mut editor, block) = spaced_block(content);
    let frame = editor.frame_boxes(0)[block];
    declare_frame(&mut editor, 0, block, [10.0, frame[1], 190.0, frame[3]]);
    let in_black = "DeviceGray(0.0)".to_owned();
    let red = "DeviceRgb(1.0, 0.0, 0.0)".to_owned();

    let changed = editor.edit(
        0,
        block,
        pdf_edit::BlockRange::Between {
            from: (0, 4),
            to: (0, 4),
        },
        "B",
    );
    assert!(matches!(changed, Applied::Changed { .. }), "{changed:?}");
    assert!(
        laid_out(&editor),
        "laid out as a block: {}",
        editor.status()
    );
    assert_placed(
        &placed(&editor, A)[..4],
        &[(10.0, 150.0), (16.0, 150.0), (22.0, 150.0), (34.0, 150.0)],
    );
    assert_placed(&placed(&editor, B)[..1], &[(46.0, 150.0)]);
    let styles = styled(&editor);
    assert_eq!(
        styles[..5],
        [
            (A, 10.0, in_black.clone()),
            (A, 10.0, in_black.clone()),
            (A, 20.0, red.clone()),
            (A, 20.0, red.clone()),
            (B, 20.0, red.clone()),
        ]
    );
    assert_eq!(
        styles[5].1, 20.0,
        "text after the block keeps the size it had"
    );
    assert_eq!(styles[5].2, red, "and the colour");

    read(&mut editor);
    let changed = editor.edit(
        0,
        block,
        pdf_edit::BlockRange::Between {
            from: (0, 1),
            to: (0, 1),
        },
        "B",
    );
    assert!(matches!(changed, Applied::Changed { .. }), "{changed:?}");
    assert!(
        laid_out(&editor),
        "laid out as a block: {}",
        editor.status()
    );
    assert_pens(
        &pens(editor.source().unwrap())[..6],
        &[
            (A, 10.0, 150.0),
            (B, 16.0, 150.0),
            (A, 22.0, 150.0),
            (A, 28.0, 150.0),
            (A, 40.0, 150.0),
            (B, 52.0, 150.0),
        ],
    );
    assert_eq!(styled(&editor)[1], (B, 10.0, in_black));

    let (mut tall, block) =
        spaced_block(b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AA) Tj /F1 20 Tf (AAA) Tj ET");
    let frame = tall.frame_boxes(0)[block];
    declare_frame(&mut tall, 0, block, [10.0, frame[1], 60.0, frame[3]]);
    let changed = tall.edit(
        0,
        block,
        pdf_edit::BlockRange::Between {
            from: (0, 5),
            to: (0, 5),
        },
        "B",
    );
    assert!(matches!(changed, Applied::Changed { .. }), "{changed:?}");
    assert!(laid_out(&tall), "laid out as a block: {}", tall.status());
    assert_placed(&placed(&tall, B), &[(10.0, 130.0)]);
}

const WRAPPED: &[u8] = b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj 1 0 0 1 10 140 Tm (AAAAA) Tj ET";

const INDENTED: &[u8] = b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj 1 0 0 1 16 140 Tm (AA) Tj ET";

#[test]
fn an_empty_line_made_by_enter_survives_the_next_letter_and_undo() {
    let (mut editor, block) = spaced_block(
        b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj 1 0 0 1 10 140 Tm (AAAAAAAA) Tj ET \
          BT /F1 10 Tf 1 0 0 1 10 40 Tm (BB) Tj ET",
    );
    let original = placed(&editor, A);
    let lines = editor.leaf(0).unwrap().overlay.blocks[block].lines.clone();

    assert!(matches!(
        editor.type_text(0, lines[0], 2, 2, "\n"),
        Applied::Changed { .. }
    ));
    assert_eq!(editor.landed_caret(), Some((1, 0)));
    read(&mut editor);
    let rows = editor.leaf(0).unwrap().overlay.blocks[block].lines.clone();
    assert_eq!(rows.len(), 3);

    assert!(matches!(
        editor.type_text(0, rows[1], 0, 0, "\n"),
        Applied::Changed { .. }
    ));
    assert_eq!(
        editor.landed_caret(),
        Some((2, 0)),
        "before `AA`, which is the block's third line now that an empty one is above it"
    );
    read(&mut editor);
    let expected = [
        row(10.0, 150.0, 2),
        row(10.0, 130.0, 2),
        row(10.0, 120.0, 8),
    ]
    .concat();
    assert_placed(&placed(&editor, A), &expected);
    let overlay = &editor.leaf(0).unwrap().overlay;
    let lines = &overlay.blocks[block].lines;
    assert_eq!(lines.len(), 4, "the empty line is one of the block's lines");
    let empty: Vec<_> = overlay
        .carets
        .iter()
        .filter(|stop| stop.line == lines[1])
        .collect();
    assert_eq!(empty.len(), 1, "and it has one caret stop");
    assert!(
        (empty[0].at[0] - 10.0).abs() < 1e-6 && (empty[0].at[1] - 60.0).abs() < 1e-6,
        "at the start of a baseline 140 pt up a 200 pt page: {:?}",
        empty[0]
    );

    assert!(matches!(
        editor.edit(
            0,
            block,
            pdf_edit::BlockRange::Between {
                from: (2, 0),
                to: (2, 0)
            },
            "A"
        ),
        Applied::Changed { .. }
    ));
    assert_eq!(editor.landed_caret(), Some((2, 1)));
    read(&mut editor);
    let typed = [
        row(10.0, 150.0, 2),
        row(10.0, 130.0, 3),
        row(10.0, 120.0, 8),
    ]
    .concat();
    assert_placed(&placed(&editor, A), &typed);
    assert_eq!(editor.leaf(0).unwrap().overlay.blocks[block].lines.len(), 4);
    assert_eq!(
        editor.leaf(0).unwrap().view.index.blocks.len(),
        2,
        "still two blocks"
    );

    for _ in 0..3 {
        assert!(matches!(editor.undo(), Applied::Changed { .. }));
        read(&mut editor);
    }
    assert_placed(&placed(&editor, A), &original);
}

#[test]
fn enter_at_the_end_of_a_block_leaves_an_empty_line_to_type_on() {
    let mut editor = Editor::open(reflow_source()).unwrap();
    read(&mut editor);
    let block = top_block(&editor.leaf(0).unwrap().view);
    let line = editor.leaf(0).unwrap().overlay.blocks[block].lines[0];
    let frame = editor.frame_boxes(0)[block];

    assert!(matches!(
        editor.type_text(0, line, 9, 9, "\n"),
        Applied::Changed { .. }
    ));
    assert_eq!(
        editor.landed_caret(),
        Some((1, 0)),
        "on the new, empty line"
    );
    assert_eq!(editor.block_edges(0, block), (0, 1));
    read(&mut editor);
    assert!(
        (editor.frame_boxes(0)[block][3] - frame[3] - 10.0).abs() < 1e-6,
        "the frame is one pitch taller"
    );
    let overlay = &editor.leaf(0).unwrap().overlay;
    let lines = overlay.blocks[block].lines.clone();
    assert_eq!(lines.len(), 2);
    let stop = overlay
        .carets
        .iter()
        .find(|stop| stop.line == lines[1])
        .expect("the empty line has a stop");
    assert!((stop.at[1] - 60.0).abs() < 1e-6, "{stop:?}");

    assert!(matches!(
        editor.edit(
            0,
            block,
            pdf_edit::BlockRange::Between {
                from: (1, 0),
                to: (1, 0)
            },
            "A"
        ),
        Applied::Changed { .. }
    ));
    assert_eq!(editor.landed_caret(), Some((1, 1)));
    assert_eq!(editor.block_edges(0, block), (0, 0));
    read(&mut editor);
    let placed_a = placed(&editor, A);
    assert_eq!(placed_a.last().copied(), Some((10.0, 140.0)));

    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    read(&mut editor);
    assert_eq!(
        editor.block_edges(0, block),
        (0, 1),
        "the empty line is back"
    );
    assert_eq!(editor.leaf(0).unwrap().overlay.blocks[block].lines.len(), 2);
    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    read(&mut editor);
    assert_eq!(editor.block_edges(0, block), (0, 0));
    assert_eq!(editor.frame_boxes(0)[block], frame);
}

fn insertion_at(line: usize, stop: usize) -> pdf_edit::BlockRange {
    pdf_edit::BlockRange::Between {
        from: (line, stop),
        to: (line, stop),
    }
}

#[test]
fn shift_enter_breaks_the_line_and_the_session_keeps_the_paragraph() {
    let (mut editor, block) = spaced_block(b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET");
    let line_break = pdf_edit::LINE_BREAK.to_string();
    assert!(matches!(
        editor.edit(0, block, insertion_at(0, 2), &line_break),
        Applied::Changed { .. }
    ));
    assert_eq!(editor.landed_caret(), Some((1, 0)));
    read(&mut editor);
    assert_placed(
        &placed(&editor, A),
        &[row(10.0, 150.0, 2), row(10.0, 140.0, 2)].concat(),
    );
    let held = pdf_edit::RowEnds {
        paragraphs: Vec::new(),
        lines: vec![0],
    };
    assert_eq!(editor.block_breaks(0, block), Some(held.clone()));
    assert_eq!(
        editor.copy_text(0, block, (0, 0), (1, 2)).as_deref(),
        Some("AA\nAA")
    );

    assert!(matches!(
        editor.edit(0, block, insertion_at(1, 2), "A"),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    assert_placed(
        &placed(&editor, A),
        &[row(10.0, 150.0, 2), row(10.0, 140.0, 3)].concat(),
    );
    assert_eq!(editor.block_breaks(0, block), Some(held.clone()));

    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    read(&mut editor);
    assert_eq!(editor.block_breaks(0, block), Some(held));

    assert!(matches!(
        editor.edit(
            0,
            block,
            pdf_edit::BlockRange::Units {
                at: (1, 0),
                backwards: true,
                count: 1
            },
            ""
        ),
        Applied::Changed { .. }
    ));
    assert_eq!(editor.landed_caret(), Some((0, 2)));
    read(&mut editor);
    assert_placed(&placed(&editor, A), &row(10.0, 150.0, 4));
    assert_eq!(
        editor.block_breaks(0, block),
        Some(pdf_edit::RowEnds::default())
    );
}

#[test]
fn enter_in_an_indented_paragraph_keeps_the_indent_on_both_halves() {
    let (mut editor, block) = spaced_block(INDENTED);
    assert!(matches!(
        editor.edit(0, block, insertion_at(1, 1), "\n"),
        Applied::Changed { .. }
    ));
    assert_eq!(editor.landed_caret(), Some((2, 0)));
    read(&mut editor);
    assert_placed(
        &placed(&editor, A),
        &[
            row(10.0, 150.0, 4),
            row(16.0, 140.0, 1),
            row(16.0, 130.0, 1),
        ]
        .concat(),
    );
    assert_eq!(
        editor.block_breaks(0, block),
        Some(pdf_edit::RowEnds::paragraphs(vec![0, 1]))
    );
}

#[test]
fn a_paragraph_started_after_a_centred_title_is_centred() {
    let (mut editor, block) = spaced_block(b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET");
    let frame = editor.frame_boxes(0)[block];
    declare_frame(&mut editor, 0, block, [0.0, frame[1], 44.0, frame[3]]);
    assert!(matches!(
        editor.edit(0, block, insertion_at(0, 4), "\n"),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    let overlay = &editor.leaf(0).unwrap().overlay;
    let empty = overlay.blocks[block].lines[1];
    let stop = overlay
        .carets
        .iter()
        .find(|stop| stop.line == empty)
        .expect("the empty line has a stop");
    assert!((stop.at[0] - 22.0).abs() < 1e-6, "{stop:?}");
    assert!(matches!(
        editor.edit(0, block, insertion_at(1, 0), "AA"),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    assert_placed(
        &placed(&editor, A),
        &[row(10.0, 150.0, 4), row(16.0, 140.0, 2)].concat(),
    );
}

#[test]
fn a_letter_typed_after_enter_takes_the_style_before_the_break() {
    let content: &[u8] =
        b"0 g BT /F1 10 Tf 1 0 0 1 10 150 Tm (AA) Tj 1 0 0 rg /F1 20 Tf (AA) Tj ET";
    let (mut editor, block) = spaced_block(content);
    let red = styled(&editor).last().expect("a glyph").2.clone();
    assert!(matches!(
        editor.edit(0, block, insertion_at(0, 4), "\n"),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    assert!(matches!(
        editor.edit(0, block, insertion_at(1, 0), "B"),
        Applied::Changed { .. }
    ));
    let typed: Vec<_> = styled(&editor)
        .into_iter()
        .filter(|glyph| glyph.0 == B)
        .collect();
    assert_eq!(typed, vec![(B, 20.0, red)]);
}

#[test]
fn line_spacing_set_from_a_caret_lays_the_block_at_that_pitch() {
    let (mut editor, block) = spaced_block(
        b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj 1 0 0 1 10 140 Tm (AAAAAAAA) Tj ET \
          BT /F1 10 Tf 1 0 0 1 10 40 Tm (BB) Tj ET",
    );
    let original = placed(&editor, A);
    let others = placed(&editor, B);
    assert_eq!(editor.block_pitch(0, block), Some(10.0));

    let spacing = pdf_edit::TextStyle {
        line_spacing: Some(15.0),
        ..pdf_edit::TextStyle::default()
    };
    let styled = editor.style(0, block, insertion_at(0, 2), spacing);
    assert!(matches!(styled, Applied::Changed { .. }), "{styled:?}");
    read(&mut editor);
    assert_placed(
        &placed(&editor, A),
        &[row(10.0, 150.0, 4), row(10.0, 135.0, 8)].concat(),
    );
    assert_placed(&placed(&editor, B), &others);
    assert_eq!(editor.block_pitch(0, block), Some(15.0));

    assert!(matches!(
        editor.edit(0, block, insertion_at(0, 4), "A"),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    assert_placed(
        &placed(&editor, A),
        &[row(10.0, 150.0, 5), row(10.0, 135.0, 8)].concat(),
    );

    for _ in 0..2 {
        assert!(matches!(editor.undo(), Applied::Changed { .. }));
        read(&mut editor);
    }
    assert_placed(&placed(&editor, A), &original);
    assert_eq!(editor.block_pitch(0, block), Some(10.0));
}

#[test]
fn a_line_spacing_too_tight_to_read_back_is_refused() {
    let (mut editor, block) = spaced_block(
        b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj 1 0 0 1 10 140 Tm (AAAAAAAA) Tj ET",
    );
    let before = editor.source().unwrap().as_bytes().to_vec();
    let tight = pdf_edit::TextStyle {
        line_spacing: Some(5.0),
        ..pdf_edit::TextStyle::default()
    };
    let refused = editor.style(0, block, insertion_at(0, 0), tight);
    assert!(matches!(refused, Applied::Refused(_)), "{refused:?}");
    assert_eq!(editor.source().unwrap().as_bytes(), before);
}

#[test]
fn an_indented_first_line_is_its_paragraphs_and_enter_keeps_the_indent() {
    let (mut editor, block) =
        spaced_block(b"BT /F1 10 Tf 1 0 0 1 16 150 Tm (AAA) Tj 1 0 0 1 10 140 Tm (AAAAA) Tj ET");
    assert_eq!(
        editor.copy_text(0, block, (0, 0), (1, 5)).as_deref(),
        Some("AAA AAAAA")
    );
    assert!(matches!(
        editor.edit(0, block, insertion_at(1, 5), "\n"),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    assert!(matches!(
        editor.edit(0, block, insertion_at(2, 0), "B"),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    assert_placed(
        &placed(&editor, A),
        &[row(16.0, 150.0, 3), row(10.0, 140.0, 5)].concat(),
    );
    assert_placed(&placed(&editor, B), &[(16.0, 130.0)]);
}

#[test]
fn only_a_long_first_row_of_a_paragraph_is_an_indent() {
    let (short, block) =
        spaced_block(b"BT /F1 10 Tf 1 0 0 1 16 150 Tm (AA) Tj 1 0 0 1 10 140 Tm (AAAAAAAAA) Tj ET");
    assert_eq!(
        short.copy_text(0, block, (0, 0), (1, 9)).as_deref(),
        Some("AA\nAAAAAAAAA")
    );

    let (middle, block) = spaced_block(
        b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAAA) Tj 1 0 0 1 10 140 Tm (AAAAA) Tj \
          1 0 0 1 4 130 Tm (AAAAAA) Tj ET",
    );
    assert_eq!(
        middle.copy_text(0, block, (0, 0), (2, 6)).as_deref(),
        Some("AAAAAAAAAA\nAAAAAA"),
        "the first two rows are one word split at the frame's edge"
    );
}

#[test]
fn a_stated_spacing_no_row_steps_at_is_not_the_pitch() {
    let (mut editor, block) = spaced_block(
        b"BT /F1 10 Tf 4 TL 1 0 0 1 10 150 Tm (AAAA) Tj 1 0 0 1 10 135 Tm (AAAAA) Tj \
          1 0 0 1 10 118 Tm (AAAAA) Tj ET",
    );
    assert_eq!(editor.block_pitch(0, block), Some(15.0));
    assert_eq!(
        editor.copy_text(0, block, (0, 0), (2, 5)).as_deref(),
        Some("AAAA AAAAA\nAAAAA")
    );
    assert!(matches!(
        editor.edit(0, block, insertion_at(0, 4), "A"),
        Applied::Changed { .. }
    ));
    assert_placed(
        &placed(&editor, A),
        &[
            row(10.0, 150.0, 5),
            row(10.0, 135.0, 5),
            row(10.0, 118.0, 5),
        ]
        .concat(),
    );
}

#[test]
fn backspace_and_delete_cross_a_wrap_and_join_paragraphs() {
    let units = |at, backwards, count| pdf_edit::BlockRange::Units {
        at,
        backwards,
        count,
    };

    let (mut once, block) = declared_block(WRAPPED);
    assert!(matches!(
        once.edit(0, block, units((1, 0), true, 1), ""),
        Applied::Changed { .. }
    ));
    assert_eq!(once.landed_caret(), Some((0, 4)));
    assert_placed(
        &placed(&once, A),
        &[row(10.0, 150.0, 5), row(10.0, 140.0, 4)].concat(),
    );
    assert!(placed(&once, SPACE).is_empty(), "the joining space is gone");

    let (mut twice, block) = declared_block(WRAPPED);
    assert!(matches!(
        twice.edit(0, block, units((1, 0), true, 2), ""),
        Applied::Changed { .. }
    ));
    assert_placed(
        &placed(&twice, A),
        &[row(10.0, 150.0, 5), row(10.0, 140.0, 3)].concat(),
    );

    let (mut joined, block) = declared_block(INDENTED);
    assert!(matches!(
        joined.edit(0, block, units((1, 0), true, 1), ""),
        Applied::Changed { .. }
    ));
    assert_placed(
        &placed(&joined, A),
        &[row(10.0, 150.0, 4), row(10.0, 140.0, 2)].concat(),
    );

    let (mut forward, block) = declared_block(INDENTED);
    assert!(matches!(
        forward.edit(0, block, units((0, 4), false, 1), ""),
        Applied::Changed { .. }
    ));
    assert_placed(
        &placed(&forward, A),
        &[row(10.0, 150.0, 4), row(10.0, 140.0, 2)].concat(),
    );

    let (mut start, block) = spaced_block(INDENTED);
    let before = start.source().unwrap().as_bytes().to_vec();
    assert_eq!(
        start.edit(0, block, units((0, 0), true, 1), ""),
        Applied::Unchanged
    );
    assert_eq!(start.source().unwrap().as_bytes(), before);

    let (mut last, block) = spaced_block(b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AA) Tj ET");
    let emptied = last.edit(0, block, units((0, 2), true, 2), "");
    assert!(matches!(emptied, Applied::Changed { .. }), "{emptied:?}");
    assert_eq!(last.landed_caret(), Some((0, 0)));
    read(&mut last);
    assert!(placed(&last, A).is_empty());
    assert_eq!(
        last.edit(0, block, units((0, 0), true, 1), ""),
        Applied::Unchanged
    );
    assert!(matches!(
        last.edit(0, block, insertion_at(0, 0), "A"),
        Applied::Changed { .. }
    ));
    assert_placed(&placed(&last, A), &[(10.0, 150.0)]);
}

#[test]
fn typing_over_a_selection_across_rows_replaces_all_of_it() {
    let (mut editor, block) = declared_block(WRAPPED);
    assert!(matches!(
        editor.edit(
            0,
            block,
            pdf_edit::BlockRange::Between {
                from: (0, 2),
                to: (1, 2)
            },
            "B"
        ),
        Applied::Changed { .. }
    ));
    assert_eq!(editor.landed_caret(), Some((0, 3)));
    let codes: Vec<(u32, f64)> = pens(editor.source().unwrap())
        .into_iter()
        .map(|pen| (pen.0, pen.1))
        .collect();
    assert_eq!(
        codes,
        vec![
            (A, 10.0),
            (A, 16.0),
            (B, 22.0),
            (A, 28.0),
            (A, 34.0),
            (A, 10.0)
        ]
    );
}

#[test]
fn copying_across_rows_keeps_spaces_and_paragraph_breaks() {
    let (wrapped, block) = spaced_block(WRAPPED);
    assert_eq!(
        wrapped.copy_text(0, block, (0, 1), (1, 3)).as_deref(),
        Some("AAA AAA")
    );
    assert_eq!(
        wrapped.copy_text(0, block, (1, 3), (0, 1)).as_deref(),
        Some("AAA AAA"),
        "in either order"
    );
    let (indented, block) = spaced_block(INDENTED);
    assert_eq!(
        indented.copy_text(0, block, (0, 2), (1, 1)).as_deref(),
        Some("AA\nA")
    );
    let (spaced, block) =
        spaced_block(b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AACA) Tj 1 0 0 1 10 140 Tm (AAAA) Tj ET");
    assert_eq!(
        spaced.copy_text(0, block, (0, 0), (1, 4)).as_deref(),
        Some("AA A AAAA")
    );
    let (split, block) =
        spaced_block(b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj 1 0 0 1 10 140 Tm (AA) Tj ET");
    assert_eq!(
        split.copy_text(0, block, (0, 0), (1, 2)).as_deref(),
        Some("AAAAAA")
    );
}

#[test]
fn a_row_that_nearly_fills_its_frame_keeps_its_start() {
    let (mut editor, block) = spaced_block(b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET");
    let frame = editor.frame_boxes(0)[block];
    declare_frame(&mut editor, 0, block, [7.0, frame[1], 37.0, frame[3]]);
    let line = editor.leaf(0).unwrap().overlay.blocks[block].lines[0];
    assert!(matches!(
        editor.type_text(0, line, 4, 4, "A"),
        Applied::Changed { .. }
    ));
    assert_eq!(placed(&editor, A).first().copied(), Some((10.0, 150.0)));
}

#[test]
fn a_later_line_step_still_reads_the_line_spacing_the_file_set() {
    let (mut editor, block) = spaced_block(
        b"BT /F1 10 Tf 12 TL 1 0 0 1 10 150 Tm (AAAA) Tj 1 0 0 1 10 140 Tm (AAAA) Tj ET \
          BT 1 0 0 1 10 40 Tm (BB) Tj T* (BB) Tj ET",
    );
    assert!(matches!(
        editor.edit(
            0,
            block,
            pdf_edit::BlockRange::Between {
                from: (0, 4),
                to: (0, 4)
            },
            "A"
        ),
        Applied::Changed { .. }
    ));
    assert_placed(
        &placed(&editor, B),
        &[row(10.0, 40.0, 2), row(10.0, 28.0, 2)].concat(),
    );
}

#[test]
fn a_blank_glyph_under_letters_does_not_cut_their_row() {
    let (editor, _) = spaced_block(
        b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (C) Tj 1 0 0 1 10 50 Tm (A) Tj \
          1 0 0 1 10 150 Tm (AB) Tj ET",
    );
    let index = &editor.leaf(0).unwrap().view.index;
    let rows = index
        .lines
        .iter()
        .filter(|line| (index.clusters[line.clusters[0]].baseline.y - 150.0).abs() < 1e-6)
        .count();
    assert_eq!(rows, 1, "the space and the letters over it are one row");
    assert_eq!(index.report.lines_layered_apart, 0);
}

#[test]
fn a_mark_painted_on_its_own_over_its_letter_does_not_cut_the_row() {
    let mut editor = Editor::open(spaced_fixture_with(
        b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AA) Tj 1 0 0 1 10 50 Tm (A) Tj \
          1 0 0 1 14 150 Tm (B) Tj 1 0 0 1 22 150 Tm (AA) Tj ET",
        "[600 0 600]",
        "/Ascent 800 /Descent -200 ",
    ))
    .unwrap();
    read(&mut editor);
    let index = &editor.leaf(0).unwrap().view.index;
    let rows = index
        .lines
        .iter()
        .filter(|line| (index.clusters[line.clusters[0]].baseline.y - 150.0).abs() < 1e-6)
        .count();
    assert_eq!(rows, 1, "the mark stays on its letter's row");
    assert_eq!(index.report.lines_layered_apart, 0);
}

#[test]
fn an_object_drawn_between_a_paragraphs_rows_is_not_grouped_into_it() {
    let (mut editor, block) = spaced_block(
        b"BT /F1 10 Tf 10 TL 1 0 0 1 10 150 Tm (AAAA) Tj 1 0 0 1 10 140 Tm (AAAA) Tj ET \
          BT /F1 10 Tf 1 0 0 1 10 144 Tm (BB) Tj ET",
    );
    let lines = &editor.leaf(0).unwrap().overlay.blocks[block].lines;
    assert_eq!(lines.len(), 2, "the paragraph's two rows, and not BB");
    assert_placed(&placed(&editor, B), &row(10.0, 144.0, 2));

    let changed = editor.edit(
        0,
        block,
        pdf_edit::BlockRange::Between {
            from: (0, 0),
            to: (0, 0),
        },
        "A",
    );
    assert!(matches!(changed, Applied::Changed { .. }), "{changed:?}");
    assert_placed(&placed(&editor, B), &row(10.0, 144.0, 2));
    assert_eq!(
        placed(&editor, A).len(),
        9,
        "one A typed into the paragraph"
    );
}

#[test]
fn a_block_whose_rows_sit_closer_than_its_pitch_keeps_their_gaps() {
    let (mut editor, block) = spaced_block(
        b"BT /F1 10 Tf 10 TL 1 0 0 1 10 150 Tm (AAAA) Tj 1 0 0 1 10 140.5 Tm (BB) Tj \
          1 0 0 1 10 132 Tm (AAAA) Tj ET",
    );
    let lines = &editor.leaf(0).unwrap().overlay.blocks[block].lines;
    assert_eq!(
        lines.len(),
        3,
        "painted in its flow, BB is in the paragraph"
    );
    let before = placed(&editor, B);
    assert_placed(&before, &row(10.0, 140.5, 2));

    let changed = editor.edit(
        0,
        block,
        pdf_edit::BlockRange::Between {
            from: (0, 0),
            to: (0, 1),
        },
        "B",
    );
    assert!(matches!(changed, Applied::Changed { .. }), "{changed:?}");
    assert!(laid_out(&editor), "{}", editor.status());
    let after = placed(&editor, B);
    assert_eq!(after.len(), 3, "one B typed");
    assert!(
        after.contains(&(10.0, 150.0)),
        "typed on its own row: {after:?}"
    );
    assert!(
        after.contains(&(10.0, 140.5)) && after.contains(&(16.0, 140.5)),
        "and BB where it was: {after:?}"
    );
    assert_placed(
        &placed(&editor, A),
        &[row(16.0, 150.0, 3), row(10.0, 132.0, 4)].concat(),
    );

    let (mut enter, block) = spaced_block(
        b"BT /F1 10 Tf 10 TL 1 0 0 1 10 150 Tm (AAAA) Tj 1 0 0 1 10 140.5 Tm (BB) Tj \
          1 0 0 1 10 132 Tm (AAAA) Tj ET",
    );
    let changed = enter.edit(0, block, insertion_at(0, 2), "\n");
    assert!(matches!(changed, Applied::Changed { .. }), "{changed:?}");
    assert_placed(&placed(&enter, B), &row(10.0, 130.5, 2));
    assert_placed(
        &placed(&enter, A),
        &[
            row(10.0, 150.0, 2),
            row(10.0, 140.0, 2),
            row(10.0, 122.0, 4),
        ]
        .concat(),
    );
}

#[test]
fn backspace_after_a_letter_with_a_mark_takes_the_mark_and_leaves_the_letter() {
    let source = spaced_fixture_program(
        b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAB) Tj ET",
        ("[600 600 600]", "/Ascent 800 /Descent -200 "),
        SPACED_CFF,
        b"<41> <0041> <42> <00410301> <43> <0020>",
    );
    let mut editor = Editor::open(source).unwrap();
    read(&mut editor);
    let backspace = |editor: &mut Editor, at: usize| {
        let block = block_of_a(editor);
        editor.edit(
            0,
            block,
            pdf_edit::BlockRange::Units {
                at: (0, at),
                backwards: true,
                count: 1,
            },
            "",
        )
    };
    let applied = backspace(&mut editor, 3);
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    read(&mut editor);
    assert_placed(&placed(&editor, B), &[]);
    assert_placed(
        &placed(&editor, A),
        &[(10.0, 150.0), (16.0, 150.0), (22.0, 150.0)],
    );
    let applied = backspace(&mut editor, 3);
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    read(&mut editor);
    assert_placed(&placed(&editor, A), &[(10.0, 150.0), (16.0, 150.0)]);
}

#[test]
fn a_mark_the_page_reads_joined_to_its_letter_leaves_the_caret_after_both() {
    let source = spaced_fixture_program(
        b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (CAAA) Tj ET",
        ("[600 0 600]", "/Ascent 800 /Descent -200 "),
        SPACED_CFF,
        b"<41> <0041> <42> <0301> <43> <00410042>",
    );
    let mut editor = Editor::open(source).unwrap();
    read(&mut editor);
    let block = block_of_a(&editor);
    let typed = editor.edit(
        0,
        block,
        pdf_edit::BlockRange::Between {
            from: (0, 4),
            to: (0, 4),
        },
        "\u{0301}",
    );
    assert!(matches!(typed, Applied::Changed { .. }), "{typed:?}");
    assert_eq!(editor.landed_caret(), Some((0, 4)));
}

#[test]
fn sara_am_typed_into_a_font_that_writes_it_as_two_is_written_as_the_two() {
    let source = spaced_fixture_program(
        b"BT /F1 10 Tf 10 TL 1 0 0 1 10 150 Tm (AAAA) Tj T* (AA) Tj ET",
        ("[600 600 0]", "/Ascent 800 /Descent -200 "),
        SPACED_CFF,
        b"<41> <0041> <42> <0E32> <43> <0E4D>",
    );
    let mut editor = Editor::open(source).unwrap();
    read(&mut editor);
    let block = block_of_a(&editor);
    let rect = editor.frame_boxes(0)[block];
    declare_frame(&mut editor, 0, block, rect);
    let typed = editor.edit(
        0,
        block,
        pdf_edit::BlockRange::Between {
            from: (1, 2),
            to: (1, 2),
        },
        "\u{0E33}",
    );
    assert!(matches!(typed, Applied::Changed { .. }), "{typed:?}");
    read(&mut editor);
    assert_placed(&placed(&editor, SPACE), &[(22.0, 140.0)]);
    assert_placed(&placed(&editor, B), &[(22.0, 140.0)]);
}

#[test]
fn a_size_set_from_the_toolbar_lays_the_paragraph_out_in_its_frame() {
    let mut editor = Editor::open(reflow_source()).unwrap();
    read(&mut editor);
    let block = top_block(&editor.leaf(0).unwrap().view);
    let anchors = editor.leaf(0).unwrap().overlay.blocks[block]
        .anchors
        .clone();
    let job = editor.begin_set_size(0, &anchors, 20.0).unwrap();
    let applied = editor.adopt(job.run());
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    read(&mut editor);
    assert_placed(
        &placed(&editor, A),
        &[
            (10.0, 150.0),
            (22.0, 150.0),
            (34.0, 150.0),
            (46.0, 150.0),
            (10.0, 130.0),
            (22.0, 130.0),
            (34.0, 130.0),
            (46.0, 130.0),
        ],
    );
}

#[test]
fn a_block_set_to_justify_fills_its_frame_except_on_its_last_line() {
    let mut editor = Editor::open(reflow_source()).unwrap();
    read(&mut editor);
    let view = editor.leaf(0).unwrap().view.clone();
    let block = top_block(&view);
    let line = view.index.blocks[block].lines[0];
    let frame = editor.frame_boxes(0)[block];
    declare_frame(
        &mut editor,
        0,
        block,
        [frame[0], frame[1], frame[2] + 20.0, frame[3]],
    );
    editor.set_alignment(0, block, pdf_edit::Alignment::Justify);
    assert!(matches!(
        editor.type_text(0, line, 9, 9, " AA AA"),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    assert_placed(
        &placed(&editor, A),
        &[
            (10.0, 150.0),
            (16.0, 150.0),
            (22.0, 150.0),
            (28.0, 150.0),
            (41.0, 150.0),
            (47.0, 150.0),
            (53.0, 150.0),
            (59.0, 150.0),
            (72.0, 150.0),
            (78.0, 150.0),
            (10.0, 140.0),
            (16.0, 140.0),
        ],
    );
}

#[test]
fn a_block_told_to_flow_round_a_drawing_keeps_its_lines_out_of_it() {
    let source = spaced_fixture_with(
        b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAACAAAA) Tj ET 0 0 0 rg 10 140 30 20 re f",
        "[600 600 600]",
        "/Ascent 800 /Descent -200 ",
    );
    let mut editor = Editor::open(source).unwrap();
    read(&mut editor);
    let view = editor.leaf(0).unwrap().view.clone();
    let block = top_block(&view);
    let line = view.index.blocks[block].lines[0];
    let frame = editor.frame_boxes(0)[block];
    declare_frame(
        &mut editor,
        0,
        block,
        [frame[0], frame[1], frame[2] + 20.0, frame[3]],
    );
    editor.set_flow_round(0, block, true);
    assert!(matches!(
        editor.type_text(0, line, 9, 9, " AA AA AA AA AA AA"),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    let mut starts: Vec<(f64, f64)> = Vec::new();
    for (x, y) in placed(&editor, A) {
        if starts
            .last()
            .is_none_or(|(_, last)| (last - y).abs() > 1e-6)
        {
            starts.push((x, y));
        }
    }
    assert!(starts.len() >= 4, "{starts:?}");
    assert!(
        starts[..3].iter().all(|(x, _)| (x - 43.0).abs() < 1e-6),
        "the lines beside the drawing: {starts:?}"
    );
    assert!(
        (starts[3].0 - 10.0).abs() < 1e-6,
        "the line below it: {starts:?}"
    );
    let bands = editor
        .free_bands(0, block)
        .expect("the frame gives up bands");
    assert!(bands.len() >= 2, "{bands:?}");
    assert!((bands[0][0] - 43.0).abs() < 1e-6, "{bands:?}");
    assert!((bands[1][0] - 10.0).abs() < 1e-6, "{bands:?}");
}

#[test]
fn the_bands_drawn_are_the_bands_the_lines_were_laid_in() {
    let source = spaced_fixture_with(
        b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAACAAAA) Tj ET 0 0 0 rg 10 134 30 1 re f",
        "[600 600 600]",
        "/Ascent 800 /Descent -200 ",
    );
    let mut editor = Editor::open(source).unwrap();
    read(&mut editor);
    let view = editor.leaf(0).unwrap().view.clone();
    let block = top_block(&view);
    let line = view.index.blocks[block].lines[0];
    let frame = editor.frame_boxes(0)[block];
    declare_frame(
        &mut editor,
        0,
        block,
        [frame[0], frame[1], frame[2] + 20.0, frame[3]],
    );
    editor.set_flow_round(0, block, true);
    assert!(matches!(
        editor.type_text(0, line, 9, 9, " AA AA AA AA AA AA AA AA AA AA"),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    let starts = line_starts(&editor);
    let xs: Vec<f64> = starts
        .iter()
        .map(|(x, _)| (x * 10.0).round() / 10.0)
        .collect();
    assert_eq!(&xs[..4], &[10.0, 10.0, 43.0, 10.0], "{starts:?}");
    let block = top_block(&editor.leaf(0).unwrap().view);
    let bands = editor
        .free_bands(0, block)
        .expect("the frame gives up bands");
    let narrowed: Vec<[f64; 4]> = bands
        .iter()
        .copied()
        .filter(|band| (band[0] - 43.0).abs() < 1e-6)
        .collect();
    assert_eq!(narrowed.len(), 1, "one band is narrowed: {bands:?}");
    assert!(
        (narrowed[0][1] - 60.0).abs() < 1e-6 && (narrowed[0][3] - 70.0).abs() < 1e-6,
        "the third line's own band: {bands:?}"
    );
}

#[test]
fn text_beside_a_turned_picture_keeps_clear_of_its_upright_box() {
    let mut editor = Editor::open(reflow_source()).unwrap();
    read(&mut editor);
    assert!(matches!(
        editor.place_images(0, vec![([10.0, 40.0, 40.0, 70.0], Arc::from(TWO_PIXELS))]),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    let anchor = editor.leaf(0).unwrap().overlay.objects[0].anchor.clone();
    let angle = 45.0_f64.to_radians();
    let turn = pdf_paint::Matrix {
        a: angle.cos(),
        b: angle.sin(),
        c: -angle.sin(),
        d: angle.cos(),
        e: 0.0,
        f: 0.0,
    };
    let job = editor
        .begin_shape(0, &anchor, turn, (25.0, 55.0))
        .expect("nothing else is running");
    assert!(matches!(editor.adopt(job.run()), Applied::Changed { .. }));
    read(&mut editor);
    let leaf = editor.leaf(0).unwrap();
    let quad = leaf.overlay.objects[0].quad;
    assert!(
        (quad[0][0] - quad[1][0]).abs() > 1.0 && (quad[0][1] - quad[1][1]).abs() > 1.0,
        "the picture is turned: {quad:?}"
    );
    let picture = editor
        .rect_in_user_space(0, leaf.overlay.objects[0].box_pixels)
        .expect("the picture has a box");
    let view = leaf.view.clone();
    let block = top_block(&view);
    let line = view.index.blocks[block].lines[0];
    let frame = editor.frame_boxes(0)[block];
    declare_frame(
        &mut editor,
        0,
        block,
        [frame[0], frame[1], frame[2] + 40.0, frame[3]],
    );
    editor.set_flow_round(0, block, true);
    assert!(matches!(
        editor.type_text(0, line, 9, 9, " AA AA AA AA AA AA AA AA AA AA"),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    let starts = line_starts(&editor);
    let beside: Vec<f64> = starts
        .iter()
        .filter(|(_, y)| {
            *y + 10.0 > picture[1] - pdf_edit::KEEP_CLEAR && *y < picture[3] + pdf_edit::KEEP_CLEAR
        })
        .map(|(x, _)| *x)
        .collect();
    assert!(
        beside.len() >= 3,
        "several lines stand beside it: {starts:?}"
    );
    let wanted = picture[2] + pdf_edit::KEEP_CLEAR;
    assert!(
        beside.iter().all(|x| (x - wanted).abs() < 1e-3),
        "every line beside the box starts at {wanted:.2}: {beside:?} (picture {picture:?})"
    );
    let below = starts
        .iter()
        .find(|(_, y)| *y + 10.0 <= picture[1] - pdf_edit::KEEP_CLEAR)
        .expect("a line below the picture");
    assert!(
        (below.0 - 10.0).abs() < 1e-6,
        "below it, the frame's own edge: {starts:?}"
    );
}

#[test]
fn a_letter_typed_after_a_spaced_out_space_lands_where_the_caret_stood() {
    let source = spaced_fixture_with(
        b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AA) Tj 80 Tc [(C) 8000] TJ ET",
        "[600 600 600]",
        "/Ascent 800 /Descent -200 ",
    );
    let mut editor = Editor::open(source).unwrap();
    read(&mut editor);
    let view = editor.leaf(0).unwrap().view.clone();
    let block = top_block(&view);
    let line = view.index.blocks[block].lines[0];
    let frame = editor.frame_boxes(0)[block];
    declare_frame(&mut editor, 0, block, [frame[0], frame[1], 190.0, frame[3]]);
    assert!(matches!(
        editor.type_text(0, line, 3, 3, "AA"),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    assert_placed(
        &placed(&editor, A),
        &[(10.0, 150.0), (16.0, 150.0), (28.0, 150.0), (34.0, 150.0)],
    );
    assert_placed(&placed(&editor, SPACE), &[(22.0, 150.0)]);
}

#[test]
fn a_letter_typed_into_a_tracked_run_keeps_its_tracking() {
    let source = spaced_fixture_with(
        b"BT /F1 10 Tf 2 Tc 1 0 0 1 10 150 Tm (AA) Tj ET",
        "[600 600 600]",
        "/Ascent 800 /Descent -200 ",
    );
    let mut editor = Editor::open(source).unwrap();
    read(&mut editor);
    let view = editor.leaf(0).unwrap().view.clone();
    let block = top_block(&view);
    let line = view.index.blocks[block].lines[0];
    let frame = editor.frame_boxes(0)[block];
    declare_frame(&mut editor, 0, block, [frame[0], frame[1], 190.0, frame[3]]);
    assert!(matches!(
        editor.type_text(0, line, 2, 2, "A"),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    assert_placed(
        &placed(&editor, A),
        &[(10.0, 150.0), (18.0, 150.0), (26.0, 150.0)],
    );
}

fn flowing_editor() -> Editor {
    let source = spaced_fixture_with(
        b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAACAAAA) Tj ET 0 0 0 rg 10 140 30 20 re f",
        "[600 600 600]",
        "/Ascent 800 /Descent -200 ",
    );
    let mut editor = Editor::open(source).unwrap();
    read(&mut editor);
    let view = editor.leaf(0).unwrap().view.clone();
    let block = top_block(&view);
    let line = view.index.blocks[block].lines[0];
    let frame = editor.frame_boxes(0)[block];
    declare_frame(
        &mut editor,
        0,
        block,
        [frame[0], frame[1], frame[2] + 20.0, frame[3]],
    );
    editor.set_flow_round(0, block, true);
    assert!(matches!(
        editor.type_text(0, line, 9, 9, " AA AA AA AA AA AA"),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    let starts = line_starts(&editor);
    assert!(
        starts[..3].iter().all(|(x, _)| (x - 43.0).abs() < 1e-6),
        "it flows round the drawing to begin with: {starts:?}"
    );
    editor
}

fn line_starts(editor: &Editor) -> Vec<(f64, f64)> {
    let mut starts: Vec<(f64, f64)> = Vec::new();
    for (x, y) in placed(editor, A) {
        if starts
            .last()
            .is_none_or(|(_, last)| (last - y).abs() > 1e-6)
        {
            starts.push((x, y));
        }
    }
    starts
}

fn the_drawing(editor: &Editor) -> String {
    let view = &editor.leaf(0).unwrap().view;
    let atom = view
        .graph
        .atoms
        .iter()
        .find(|atom| matches!(atom.kind, pdf_paint::PaintAtomKind::Path(_)))
        .expect("the page has a drawing");
    pdf_edit::SourceAnchor::of(&atom.id).encode()
}

#[test]
fn a_block_that_makes_way_keeps_a_frame_that_holds_its_lines() {
    let source = spaced_fixture_with(
        b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAACAAAA) Tj ET 0 0 0 rg 10 140 30 20 re f",
        "[600 600 600]",
        "/Ascent 800 /Descent -200 ",
    );
    let mut editor = Editor::open(source).unwrap();
    read(&mut editor);
    let view = editor.leaf(0).unwrap().view.clone();
    let block = top_block(&view);
    let line = view.index.blocks[block].lines[0];
    let frame = editor.frame_boxes(0)[block];
    declare_frame(
        &mut editor,
        0,
        block,
        [frame[0], frame[1], frame[2] + 20.0, frame[3]],
    );
    assert!(matches!(
        editor.type_text(0, line, 9, 9, " AA AA AA AA AA AA"),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    let straight = line_starts(&editor).len();
    let job = editor
        .begin_flow_round(0, [0.0, 0.0, 200.0, 200.0])
        .expect("the drawing reaches a block");
    let applied = editor.adopt(job.run());
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    read(&mut editor);
    let block = top_block(&editor.leaf(0).unwrap().view);
    let starts = line_starts(&editor);
    assert!(
        starts.len() > straight,
        "making way costs lines: {starts:?}"
    );
    let held = editor
        .frame_in_user_space(0, editor.frame_boxes(0)[block])
        .expect("the frame is on the page");
    assert!(
        starts.iter().all(|(_, y)| *y >= held[1] - 1e-6),
        "every line is inside {held:?}: {starts:?}"
    );
}

#[test]
fn a_block_that_flows_round_is_laid_out_again_when_it_moves_away() {
    let mut editor = flowing_editor();
    let block = top_block(&editor.leaf(0).unwrap().view);
    let moved = editor.move_text_block(0, block, 0.0, 60.0);
    assert!(matches!(moved, Applied::Changed { .. }), "{moved:?}");
    assert_eq!(
        *editor.status(),
        crate::wording::Message::TextMadeWay { blocks: 1 }
    );
    read(&mut editor);
    let starts = line_starts(&editor);
    assert!(
        starts.iter().all(|(x, _)| (x - 10.0).abs() < 1e-6),
        "moved clear of the drawing: {starts:?}"
    );
    assert_eq!(editor.leaf(0).unwrap().view.index.blocks.len(), 1);
}

#[test]
fn a_move_that_makes_text_make_way_declares_the_whole_page() {
    let mut editor = flowing_editor();
    let anchor = the_drawing(&editor);
    let moved = editor.place(0, &anchor, 0.0, 60.0);
    assert_eq!(
        *editor.status(),
        crate::wording::Message::TextMadeWay { blocks: 1 }
    );
    assert!(
        matches!(
            moved,
            Applied::Changed {
                page: 0,
                region: None
            }
        ),
        "{moved:?}"
    );
    let source = spaced_fixture_with(
        b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAACAAAA) Tj ET 0 0 0 rg 10 140 30 20 re f",
        "[600 600 600]",
        "/Ascent 800 /Descent -200 ",
    );
    let mut plain = Editor::open(source).unwrap();
    read(&mut plain);
    let anchor = the_drawing(&plain);
    let moved = plain.place(0, &anchor, 0.0, 60.0);
    assert!(
        matches!(
            moved,
            Applied::Changed {
                page: 0,
                region: Some(_)
            }
        ),
        "{moved:?}"
    );
}

#[test]
fn telling_the_text_to_make_way_twice_lays_it_in_the_same_place() {
    let mut editor = flowing_editor();
    let before = line_starts(&editor);
    let job = editor
        .begin_flow_round(0, [0.0, 0.0, 200.0, 200.0])
        .expect("the drawing reaches a block");
    let applied = editor.adopt(job.run());
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    read(&mut editor);
    assert_eq!(line_starts(&editor), before);
}

#[test]
fn undoing_a_move_of_a_flowing_block_takes_back_the_wrap_with_it() {
    let mut editor = flowing_editor();
    let before = line_starts(&editor);
    let block = top_block(&editor.leaf(0).unwrap().view);
    assert!(matches!(
        editor.move_text_block(0, block, 0.0, 60.0),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    read(&mut editor);
    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    read(&mut editor);
    assert_eq!(line_starts(&editor), before);
    assert!(matches!(editor.redo(), Applied::Changed { .. }));
    assert!(matches!(editor.redo(), Applied::Changed { .. }));
    read(&mut editor);
    let after = line_starts(&editor);
    assert!(
        after.iter().all(|(x, _)| (x - 10.0).abs() < 1e-6),
        "redone: {after:?}"
    );
}

#[test]
fn a_cropped_drawing_slides_under_its_crop_and_the_window_says_so() {
    let source = spaced_fixture_with(
        b"q 10 100 20 30 re W n 0 0 0 rg 10 100 40 30 re f 60 100 40 30 re f Q",
        "[600 600 600]",
        "/Ascent 800 /Descent -200 ",
    );
    let mut editor = Editor::open(source).unwrap();
    read(&mut editor);
    let anchor = the_drawing(&editor);
    let moved = editor.place(0, &anchor, 5.0, 0.0);
    assert!(matches!(moved, Applied::Changed { .. }), "{moved:?}");
    let said = editor.status().say(crate::wording::Lang::English);
    assert!(said.contains("under its crop"), "{said}");
    read(&mut editor);
    let anchor = the_drawing(&editor);
    let gone = editor.place(0, &anchor, 400.0, 0.0);
    assert!(matches!(gone, Applied::Changed { .. }), "{gone:?}");
    let said = editor.status().say(crate::wording::Lang::English);
    assert!(said.contains("out of sight"), "{said}");
}

#[test]
fn a_drawing_standing_in_two_paragraphs_makes_both_of_them_give_way() {
    let source = spaced_fixture_with(
        b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAACAAAA) Tj ET           BT /F1 10 Tf 1 0 0 1 10 60 Tm (AAAACAAAA) Tj ET 0 0 0 rg 10 40 30 130 re f",
        "[600 600 600]",
        "/Ascent 800 /Descent -200 ",
    );
    let mut editor = Editor::open(source).unwrap();
    read(&mut editor);
    assert_eq!(editor.leaf(0).unwrap().view.index.blocks.len(), 2);
    for block in 0..2 {
        let frame = editor.frame_boxes(0)[block];
        declare_frame(
            &mut editor,
            0,
            block,
            [frame[0], frame[1], frame[2] + 20.0, frame[3]],
        );
    }
    let job = editor
        .begin_flow_round(0, [0.0, 0.0, 200.0, 200.0])
        .expect("the drawing reaches both blocks");
    let applied = editor.adopt(job.run());
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    assert_eq!(
        *editor.status(),
        crate::wording::Message::TextMadeWay { blocks: 2 }
    );
    read(&mut editor);
    assert_eq!(editor.leaf(0).unwrap().view.index.blocks.len(), 2);
    let starts = line_starts(&editor);
    assert!(
        starts.iter().all(|(x, _)| (x - 43.0).abs() < 1e-6),
        "both paragraphs give way: {starts:?}"
    );
    let anchor = the_drawing(&editor);
    let moved = editor.place(0, &anchor, 120.0, 0.0);
    assert!(matches!(moved, Applied::Changed { .. }), "{moved:?}");
    assert_eq!(
        *editor.status(),
        crate::wording::Message::TextMadeWay { blocks: 2 }
    );
    read(&mut editor);
    let starts = line_starts(&editor);
    assert!(
        starts.iter().all(|(x, _)| (x - 10.0).abs() < 1e-6),
        "the drawing has left them both: {starts:?}"
    );
}

#[test]
fn a_flowing_block_laid_out_again_is_still_one_block() {
    let mut editor = flowing_editor();
    let block = top_block(&editor.leaf(0).unwrap().view);
    let moved = editor.move_text_block(0, block, 0.0, 5.0);
    assert!(matches!(moved, Applied::Changed { .. }), "{moved:?}");
    assert!(
        editor.grouping(0).is_some(),
        "the page's blocks are still named"
    );
    read(&mut editor);
    assert_eq!(editor.leaf(0).unwrap().view.index.blocks.len(), 1);
    let starts = line_starts(&editor);
    assert!(
        starts[..2].iter().all(|(x, _)| (x - 43.0).abs() < 1e-6),
        "it still flows round the drawing: {starts:?}"
    );
}

#[test]
fn a_block_that_flows_round_is_laid_out_again_when_the_drawing_moves_away() {
    let mut editor = flowing_editor();
    let anchor = the_drawing(&editor);
    let moved = editor.place(0, &anchor, 120.0, 0.0);
    assert!(matches!(moved, Applied::Changed { .. }), "{moved:?}");
    read(&mut editor);
    let starts = line_starts(&editor);
    assert!(
        starts.iter().all(|(x, _)| (x - 10.0).abs() < 1e-6),
        "the drawing has left the frame: {starts:?}"
    );
    let anchor = the_drawing(&editor);
    let back = editor.place(0, &anchor, -120.0, 0.0);
    assert!(matches!(back, Applied::Changed { .. }), "{back:?}");
    read(&mut editor);
    let starts = line_starts(&editor);
    assert!(
        starts[..3].iter().all(|(x, _)| (x - 43.0).abs() < 1e-6),
        "the drawing is back in the frame: {starts:?}"
    );
}

#[test]
fn a_selection_and_the_caret_keys_follow_the_rows_of_one_block() {
    use crate::view::{Step, block_position, caret_step_in, selection_rows, stop_at};
    let stop = |line, offset, y| pdf_cli::CaretStop {
        line,
        offset,
        at: [10.0 + 6.0 * f64::from(u32::try_from(offset).unwrap()), y],
        up: [0.0, -10.0],
    };
    let mut stops: Vec<_> = (0..=4).map(|offset| stop(3, offset, 50.0)).collect();
    stops.push(stop(7, 0, 60.0));
    stops.extend((0..=2).map(|offset| stop(9, offset, 70.0)));
    let rows = [3, 7, 9];
    let at = |line, offset| stop_at(&stops, line, offset).unwrap();

    assert_eq!(block_position(&stops, &rows, at(9, 1)), Some((2, 1)));
    assert_eq!(
        selection_rows(&stops, &rows, at(3, 2), at(9, 1)),
        vec![(3, 2, 4), (9, 0, 1)]
    );
    assert_eq!(
        selection_rows(&stops, &rows, at(9, 1), at(3, 2)),
        vec![(3, 2, 4), (9, 0, 1)],
        "the same range dragged the other way"
    );
    assert_eq!(
        selection_rows(&stops, &rows, at(3, 1), at(3, 3)),
        vec![(3, 1, 3)]
    );
    assert!(selection_rows(&stops, &rows, at(3, 1), at(3, 1)).is_empty());

    assert_eq!(caret_step_in(&stops, &rows, at(9, 0), Step::Left), at(7, 0));
    assert_eq!(caret_step_in(&stops, &rows, at(7, 0), Step::Left), at(3, 4));
    assert_eq!(
        caret_step_in(&stops, &rows, at(3, 4), Step::Right),
        at(7, 0)
    );
    assert_eq!(
        caret_step_in(&stops, &rows, at(3, 0), Step::Left),
        at(3, 0),
        "the block's start"
    );
    assert_eq!(
        caret_step_in(&stops, &rows, at(3, 2), Step::RowStart),
        at(3, 0)
    );
    assert_eq!(
        caret_step_in(&stops, &rows, at(3, 2), Step::RowEnd),
        at(3, 4)
    );
    assert_eq!(
        caret_step_in(&stops, &rows, at(9, 2), Step::Right),
        at(9, 2),
        "the block's end"
    );
}

fn declare_frame(editor: &mut Editor, page: usize, block: usize, rect: [f64; 4]) {
    let first = editor.frame_boxes(page)[block];
    let nudged = [first[0], first[1], first[2] + 1.0, first[3]];
    editor.preview_frame(page, block, nudged);
    editor.finish_frame_resize(page, block, first);
    editor.preview_frame(page, block, rect);
    editor.finish_frame_resize(page, block, nudged);
    assert_eq!(editor.frame_boxes(page)[block], rect);
    assert!(editor.frame_is_declared(page, block));
}

fn widen_the_frame(editor: &mut Editor) {
    let frames = editor.frame_boxes(0).to_vec();
    let mut larger = frames[0];
    larger[0] -= 30.0;
    larger[2] += 60.0;
    editor.preview_frame(0, 0, larger);
    editor.finish_frame_resize(0, 0, frames[0]);
    assert_eq!(editor.frame_boxes(0)[0], larger);
}

fn linked_fixture() -> ByteStore {
    let objects: [&[u8]; 6] = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /MediaBox [0 0 200 200] /Kids [3 0 R 4 0 R] /Count 2 >>",
        b"<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << >> /Annots [5 0 R] >>",
        b"<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << >> >>",
        b"<< /Type /Annot /Subtype /Link /Rect [10 20 110 40] /Dest [4 0 R /Fit] >>",
        b"<< /Length 0 >>\nstream\n\nendstream",
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        bytes.extend_from_slice(body);
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
    ByteStore::new(SourceId::new(77), Arc::<[u8]>::from(bytes))
}

#[test]
fn the_link_under_a_pixel_is_found_through_the_page_transform() {
    let mut editor = Editor::open(linked_fixture()).expect("opens");
    let link = editor
        .link_at(0, (60.0, 170.0))
        .expect("a link at user (60, 30)");
    assert_eq!(
        link.destination.and_then(|destination| destination.page),
        Some(1)
    );
    assert!(
        editor.link_at(0, (60.0, 30.0)).is_none(),
        "user (60, 170) is paper"
    );
    assert!(
        editor.link_at(1, (60.0, 170.0)).is_none(),
        "the second page has no link"
    );
}

#[test]
fn a_cluster_part_of_which_says_nothing_keeps_the_rest_and_stays_unknown_to_a_command() {
    let mut editor = Editor::open(spaced_fixture_program(
        b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AB) Tj ET",
        ("[600 0 600]", "/Ascent 800 /Descent -200 "),
        NAMELESS_CFF,
        b"<43> <0020>",
    ))
    .unwrap();
    read(&mut editor);
    let overlay = &editor.leaf(0).unwrap().overlay;
    let line = overlay.clusters[0].line;
    let in_row: Vec<_> = overlay
        .clusters
        .iter()
        .filter(|cluster| cluster.line == line)
        .collect();
    assert_eq!(in_row.len(), 1, "the unmapped sign joins its letter");
    assert_eq!(in_row[0].text.as_deref(), Some("A\u{fffd}"));
    let reading = pdf_cli::read_selection(&overlay.clusters, line, 0, 1).expect("a selection");
    assert_eq!(reading.shown, "A\u{fffd}");
    assert_eq!(reading.unknown, 1);
    assert_eq!(pdf_cli::selection_text(&overlay.clusters, line, 0, 1), None);
}

#[test]
fn a_block_whose_letters_have_no_unicode_meaning_takes_enter_and_breaks_where_the_file_did() {
    let mut editor = Editor::open(spaced_fixture_program(
        b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (BBB) Tj 1 0 0 1 10 140 Tm (BBBB) Tj ET \
          BT /F1 10 Tf 1 0 0 1 10 40 Tm (AA) Tj ET",
        ("[600 600 600]", "/Ascent 800 /Descent -200 "),
        NAMELESS_CFF,
        b"<43> <0020>",
    ))
    .unwrap();
    read(&mut editor);
    let block = top_block(&editor.leaf(0).unwrap().view);
    assert_eq!(
        editor.copy_text(0, block, (0, 0), (1, 4)).as_deref(),
        Some("\u{fffd}".repeat(7).as_str()),
        "a glyph nothing names reads as unknown, never as the alpha it was not drawn as"
    );
    let other = 1 - block;
    assert_eq!(
        editor.copy_text(0, other, (0, 0), (0, 2)).as_deref(),
        Some("AA"),
        "control: a name the Adobe Glyph List knows is read"
    );
    let changed = editor.edit(
        0,
        block,
        pdf_edit::BlockRange::Between {
            from: (0, 2),
            to: (0, 2),
        },
        "\n",
    );
    assert!(matches!(changed, Applied::Changed { .. }), "{changed:?}");
    assert!(
        laid_out(&editor),
        "laid out as a block: {}",
        editor.status()
    );
    assert_placed(
        &placed(&editor, B),
        &[
            row(10.0, 150.0, 2),
            row(10.0, 140.0, 1),
            row(10.0, 130.0, 4),
        ]
        .concat(),
    );
    assert!(placed(&editor, SPACE).is_empty(), "no space was added");
}

#[test]
fn evenly_spaced_rows_two_pitches_apart_keep_their_empty_line_in_the_window() {
    let (editor, block) = spaced_block(
        b"BT /F1 10 Tf 10 TL 1 0 0 1 10 150 Tm (AAAA) Tj 1 0 0 1 10 130 Tm (AAAA) Tj ET \
          BT /F1 10 Tf 1 0 0 1 10 40 Tm (BB) Tj ET",
    );
    let leaf = editor.leaf(0).unwrap();
    assert_eq!(
        leaf.view.index.blocks[block].lines.len(),
        2,
        "the page has two rows in the block"
    );
    assert_eq!(
        leaf.overlay.blocks[block].lines.len(),
        3,
        "and the window has a line for the empty one between them"
    );
    assert_eq!(
        editor.copy_text(0, block, (0, 0), (2, 4)).as_deref(),
        Some("AAAA\n\nAAAA")
    );
}

#[test]
fn a_block_sized_by_its_text_matrix_lays_out_and_keeps_its_subscript() {
    let content: &[u8] = b"BT /F1 1 Tf 10 0 0 10 10 150 Tm (AA) Tj 5 0 0 5 22 150 Tm (A) Tj \
        10 0 0 10 25 150 Tm (A) Tj ET BT /F1 1 Tf 10 0 0 10 10 40 Tm (B) Tj ET";
    let (mut editor, block) = spaced_block(content);
    assert_eq!(
        editor.leaf(0).unwrap().view.index.blocks[block].lines.len(),
        1,
        "one row"
    );
    let frame = editor.frame_boxes(0)[block];
    declare_frame(&mut editor, 0, block, [10.0, frame[1], 190.0, frame[3]]);
    let changed = editor.edit(
        0,
        block,
        pdf_edit::BlockRange::Between {
            from: (0, 4),
            to: (0, 4),
        },
        "B",
    );
    assert!(matches!(changed, Applied::Changed { .. }), "{changed:?}");
    assert!(
        laid_out(&editor),
        "laid out as a block: {}",
        editor.status()
    );
    assert_pens(
        &pens(editor.source().unwrap())[..5],
        &[
            (A, 10.0, 150.0),
            (A, 16.0, 150.0),
            (A, 22.0, 150.0),
            (A, 25.0, 150.0),
            (B, 31.0, 150.0),
        ],
    );
    let sizes: Vec<f64> = styled(&editor).iter().map(|style| style.1).collect();
    assert_eq!(sizes[..5], [1.0, 1.0, 0.5, 1.0, 1.0]);
    assert_eq!(sizes[5], 1.0, "text after the block keeps its size");

    read(&mut editor);
    let changed = editor.edit(
        0,
        block,
        pdf_edit::BlockRange::Between {
            from: (0, 3),
            to: (0, 3),
        },
        "\n",
    );
    assert!(matches!(changed, Applied::Changed { .. }), "{changed:?}");
    assert!(
        laid_out(&editor),
        "laid out as a block: {}",
        editor.status()
    );
    assert_pens(
        &pens(editor.source().unwrap())[..5],
        &[
            (A, 10.0, 150.0),
            (A, 16.0, 150.0),
            (A, 22.0, 150.0),
            (A, 10.0, 140.0),
            (B, 16.0, 140.0),
        ],
    );
}

#[test]
fn a_style_keeps_the_row_ends_of_a_block_never_laid_out() {
    let mut editor = Editor::open(spaced_fixture_with(
        b"BT /F1 10 Tf 12 TL 1 0 0 1 10 150 Tm (AAAACAAAA) Tj T* (AAAA) Tj T* (AAAACAAAACAA) Tj ET",
        "[600 600 600]",
        "/Ascent 800 /Descent -200 ",
    ))
    .unwrap();
    read(&mut editor);
    assert_eq!(editor.leaf(0).unwrap().overlay.blocks.len(), 1);
    let red = pdf_edit::TextStyle {
        fill: Some([1.0, 0.0, 0.0]),
        ..pdf_edit::TextStyle::default()
    };
    let styled = editor.style(
        0,
        0,
        pdf_edit::BlockRange::Between {
            from: (0, 0),
            to: (2, 12),
        },
        red,
    );
    assert!(matches!(styled, Applied::Changed { .. }), "{styled:?}");
    read(&mut editor);
    let typed = editor.edit(
        0,
        0,
        pdf_edit::BlockRange::Between {
            from: (0, 9),
            to: (0, 9),
        },
        " AAAA",
    );
    assert!(matches!(typed, Applied::Changed { .. }), "{typed:?}");
    let mut letters: Vec<(i64, usize)> = Vec::new();
    for (code, _, y) in pens(editor.source().unwrap()) {
        if code != 0x41 {
            continue;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "baselines a few hundred points up"
        )]
        let baseline = y.round() as i64;
        match letters.iter_mut().find(|(at, _)| *at == baseline) {
            Some((_, count)) => *count += 1,
            None => letters.push((baseline, 1)),
        }
    }
    letters.sort_by_key(|(at, _)| -at);
    assert_eq!(letters, [(150, 8), (138, 4), (126, 4), (114, 10)]);
}

const SHARED_ROW: &[u8] =
    b"BT /F1 10 Tf 1 0 0 1 10 150 Tm [(AAAA) -10000 (BB)] TJ ET BT /F1 10 Tf 1 0 0 1 10 40 Tm (AA) Tj ET";

#[test]
fn moving_a_block_that_shares_its_show_operation_leaves_the_other_block() {
    let source = spaced_fixture_with(SHARED_ROW, "[600 600 600]", "/Ascent 800 /Descent -200 ");
    let before = pens(&source);
    let mut editor = Editor::open(source).unwrap();
    read(&mut editor);
    let leaf = editor.leaf(0).unwrap();
    let blocks = leaf.overlay.blocks.len();
    assert_eq!(blocks, 3, "the row reads as two cells over one block");
    let first = leaf
        .overlay
        .blocks
        .iter()
        .position(|block| {
            block.lines.first().is_some_and(|line| {
                let line = &leaf.view.index.lines[*line];
                line.clusters.len() == 4
                    && leaf.view.index.clusters[line.clusters[0]].baseline.y > 100.0
            })
        })
        .expect("the AAAA cell");
    let anchors = leaf.overlay.blocks[first].anchors.clone();
    let applied = editor.move_block(0, &anchors, 7.0, -5.0);
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    let after = pens(editor.source().unwrap());
    let mut wanted = before.clone();
    for pen in wanted.iter_mut().filter(|pen| pen.2 > 100.0 && pen.0 == A) {
        pen.1 += 7.0;
        pen.2 += 5.0;
    }
    let sorted = |pens: &[(u32, f64, f64)]| {
        let mut pens = pens.to_vec();
        pens.sort_by(|one, other| one.1.total_cmp(&other.1).then(one.2.total_cmp(&other.2)));
        pens
    };
    assert_pens(&sorted(&after), &sorted(&wanted));
    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    assert_pens(&pens(editor.source().unwrap()), &before);
}

fn drawings(source: &ByteStore) -> Vec<[f64; 4]> {
    let view = pdf_session::interpret_page(source, 0).unwrap();
    view.graph
        .atoms
        .iter()
        .filter(|atom| matches!(atom.kind, pdf_paint::PaintAtomKind::Path(_)))
        .filter_map(|atom| atom.kind.user_bounds())
        .collect()
}

#[test]
fn text_and_the_rule_between_it_move_together_and_undo_together() {
    let source = spaced_fixture_with(
        b"0 0 10000 10000 re f BT /F1 10 Tf 1 0 0 1 10 150 Tm (AA) Tj ET 10 140 12 1 re f BT /F1 10 Tf 1 0 0 1 10 128 Tm (BB) Tj ET",
        "[600 600 600]",
        "/Ascent 800 /Descent -200 ",
    );
    let (pens_before, rules_before) = (pens(&source), drawings(&source));
    assert_eq!(rules_before.len(), 2);
    let mut editor = Editor::open(source).unwrap();
    read(&mut editor);
    let leaf = editor.leaf(0).unwrap();
    let anchors: Vec<String> = leaf
        .overlay
        .blocks
        .iter()
        .flat_map(|block| block.anchors.iter().cloned())
        .collect();
    let objects: Vec<String> = leaf
        .overlay
        .objects
        .iter()
        .filter(|object| object.kind == pdf_semantics::ObjectKind::Path)
        .map(|object| object.anchor.clone())
        .collect();
    assert_eq!(objects.len(), 1, "the rule is a target the window serves");

    let frames = editor.frame_boxes(0).to_vec();
    assert_eq!(frames.len(), 2);
    let applied = editor.move_group(0, &anchors, &objects, (7.0, -5.0));
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    read(&mut editor);
    let shifted: Vec<[f64; 4]> = frames
        .iter()
        .map(|frame| crate::view::box_shifted(*frame, 7.0, -5.0))
        .collect();
    assert_eq!(editor.frame_boxes(0), shifted.as_slice());
    let leaf = editor.leaf(0).unwrap();
    for (block, frame) in shifted.iter().enumerate() {
        let ink = leaf.overlay.blocks[block].layout_pixels;
        assert!(
            frame
                .iter()
                .zip(ink.iter())
                .all(|(a, b)| (a - b).abs() < 1e-6),
            "frame {frame:?} sits on its block {ink:?}"
        );
    }
    let moved = editor.source().unwrap().clone();
    let wanted: Vec<(u32, f64, f64)> = pens_before
        .iter()
        .map(|(code, x, y)| (*code, x + 7.0, y + 5.0))
        .collect();
    assert_pens(&pens(&moved), &wanted);
    assert_eq!(drawings(&moved)[0], rules_before[0], "the background stays");
    let rule = drawings(&moved)[1];
    let was = rules_before[1];
    for (now, then, by) in [
        (rule[0], was[0], 7.0),
        (rule[1], was[1], 5.0),
        (rule[2], was[2], 7.0),
        (rule[3], was[3], 5.0),
    ] {
        assert!((now - (then + by)).abs() < 1e-6, "{rule:?} from {was:?}");
    }

    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    assert_pens(&pens(editor.source().unwrap()), &pens_before);
    assert_eq!(drawings(editor.source().unwrap()), rules_before);
    assert_eq!(editor.frame_boxes(0), frames.as_slice());
}

#[test]
fn a_band_of_two_paragraphs_moved_together_carries_both_frames() {
    let mut editor = editor();
    let frames = editor.frame_boxes(0).to_vec();
    let boxes: Vec<[f64; 4]> = editor
        .leaf(0)
        .unwrap()
        .overlay
        .blocks
        .iter()
        .map(|block| block.layout_pixels)
        .collect();
    assert_eq!(frames, boxes, "at rest a frame is its block's box");
    let anchors: Vec<String> = editor
        .leaf(0)
        .unwrap()
        .overlay
        .blocks
        .iter()
        .flat_map(|block| block.anchors.iter().cloned())
        .collect();
    assert_eq!(anchors.len(), 2);
    assert!(matches!(
        editor.move_block(0, &anchors, 9.0, 11.0),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    let moved: Vec<[f64; 4]> = editor
        .leaf(0)
        .unwrap()
        .overlay
        .blocks
        .iter()
        .map(|block| block.layout_pixels)
        .collect();
    for (was, now) in boxes.iter().zip(moved.iter()) {
        assert!(
            (now[0] - was[0] - 9.0).abs() < 1e-6 && (now[1] - was[1] - 11.0).abs() < 1e-6,
            "the block moved: {was:?} -> {now:?}"
        );
    }
    assert_eq!(
        editor.frame_boxes(0).to_vec(),
        frames
            .iter()
            .map(|frame| crate::view::box_shifted(*frame, 9.0, 11.0))
            .collect::<Vec<_>>()
    );
    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    read(&mut editor);
    assert_eq!(editor.frame_boxes(0), frames.as_slice());
}

#[test]
fn a_stated_spacing_under_the_line_floor_is_not_the_pitch() {
    let (mut editor, block) = spaced_block(b"BT /F1 10 Tf 0.5 TL 1 0 0 1 10 150 Tm (AAAA) Tj ET");
    assert_eq!(editor.block_pitch(0, block), Some(10.0));
    assert!(matches!(
        editor.edit(0, block, insertion_at(0, 4), "\nA"),
        Applied::Changed { .. }
    ));
    assert_placed(
        &placed(&editor, A),
        &[row(10.0, 150.0, 4), row(10.0, 140.0, 1)].concat(),
    );
}

#[test]
fn typing_on_a_row_alone_never_pushes_another_blocks_glyphs() {
    let source = spaced_fixture_with(
        b"BT /F1 10 Tf 1 0 0 1 10 150 Tm [(AAAA) -10000 (BB)] TJ ET \
          q 2 0 0 2 0 0 cm BT /F1 5 Tf 1 0 0 1 5 70 Tm (AAAA) Tj ET Q",
        "[600 600 600]",
        "/Ascent 800 /Descent -200 ",
    );
    let bytes = source.as_bytes().to_vec();
    let mut editor = Editor::open(source).unwrap();
    read(&mut editor);
    let leaf = editor.leaf(0).unwrap();
    let rows = |block: &pdf_cli::TextBlockBox| -> Vec<usize> {
        block
            .lines
            .iter()
            .map(|line| leaf.view.index.lines[*line].clusters.len())
            .collect()
    };
    let all: Vec<Vec<usize>> = leaf.overlay.blocks.iter().map(rows).collect();
    let block = all
        .iter()
        .position(|counts| counts == &[4, 4])
        .unwrap_or_else(|| panic!("the cell and the row under it: {all:?}"));
    let applied = editor.edit(0, block, insertion_at(0, 4), "A");
    assert!(matches!(applied, Applied::Refused(_)), "{applied:?}");
    assert_eq!(editor.source().unwrap().as_bytes(), bytes);
}

#[test]
fn a_block_stating_no_line_height_is_laid_out_at_the_default_pitch() {
    let mut editor = Editor::open(fixture(b"BT /F1 24 Tf 1 0 0 1 10 150 Tm (AAA) Tj ET")).unwrap();
    read(&mut editor);
    let applied = editor.edit(0, 0, insertion_at(0, 3), "\nA");
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    let wanted = [(10.0, 150.0), (24.4, 150.0), (38.8, 150.0), (10.0, 121.2)];
    let got: Vec<(f64, f64)> = pens(editor.source().unwrap())
        .into_iter()
        .map(|pen| (pen.1, pen.2))
        .collect();
    assert_placed(&got, &wanted);
}

#[test]
fn rows_under_transforms_that_differ_only_in_place_are_laid_out_together() {
    let (mut editor, block) = declared_block(
        b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET \
          q 1 0 0 1 20 5 cm BT /F1 10 Tf 1 0 0 1 -10 135 Tm (AAAA) Tj ET Q",
    );
    let applied = editor.edit(0, block, insertion_at(0, 2), "\n");
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    assert_placed(
        &placed(&editor, A),
        &[
            row(10.0, 150.0, 2),
            row(10.0, 140.0, 4),
            row(10.0, 130.0, 2),
        ]
        .concat(),
    );
}

#[test]
fn a_row_squeezed_along_its_baseline_is_laid_out_with_its_block() {
    let (mut editor, block) = spaced_block(
        b"BT /F1 1 Tf 10 0 0 10 10 150 Tm (AA) Tj 5 0 0 10 10 140 Tm (AAAAAAAA) Tj ET",
    );
    let applied = editor.edit(0, block, insertion_at(0, 1), "\n");
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    let squeezed: Vec<(f64, f64)> = (0..8)
        .map(|index| (10.0 + 3.0 * f64::from(index), 130.0))
        .collect();
    assert_placed(
        &placed(&editor, A),
        &[row(10.0, 150.0, 1), row(10.0, 140.0, 1), squeezed].concat(),
    );
}

#[test]
fn a_clip_that_hides_nothing_is_lifted_for_a_line_that_grows_past_it() {
    let (mut editor, block) =
        spaced_block(b"q 5 145 50 15 re W n BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET Q");
    let applied = editor.edit(0, block, insertion_at(0, 2), "\n");
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    assert_placed(
        &placed(&editor, A),
        &[row(10.0, 150.0, 2), row(10.0, 140.0, 2)].concat(),
    );
    let view = pdf_session::interpret_page(editor.source().unwrap(), 0).unwrap();
    assert!(
        view.graph.atoms.iter().all(|atom| match &atom.kind {
            pdf_paint::PaintAtomKind::Text(text) => text.state.clip_paths.is_empty(),
            _ => true,
        }),
        "the clip is lifted"
    );

    assert!(
        !editor
            .status()
            .say(crate::wording::Lang::English)
            .contains("behind a crop"),
        "control: a lifted clip hides nothing, so nothing is said"
    );

    for content in [
        &b"q 5 145 50 15 re W n 0 100 m 100 200 l S \
          BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET Q"[..],
        &b"q 5 145 50 15 re W n 4 w 6 146 m 54 146 l S \
          BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET Q"[..],
    ] {
        let (mut cut, block) = spaced_block(content);
        let applied = cut.edit(0, block, insertion_at(0, 2), "\n");
        assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
        assert_placed(
            &placed(&cut, A),
            &[row(10.0, 150.0, 2), row(10.0, 140.0, 2)].concat(),
        );
        let said = cut.status().say(crate::wording::Lang::English);
        assert!(said.contains("behind a crop"), "{said}");
    }
}

#[test]
fn a_clip_that_hides_only_what_nobody_sees_is_lifted() {
    let clipped = |editor: &Editor| {
        let view = pdf_session::interpret_page(editor.source().unwrap(), 0).unwrap();
        view.graph.atoms.iter().any(|atom| match &atom.kind {
            pdf_paint::PaintAtomKind::Text(text) => !text.state.clip_paths.is_empty(),
            _ => false,
        })
    };
    for (drawn, lifted) in [
        ("250 150 m 300 150 l S", true),
        ("-100 150 m -50 150 l S", true),
        ("10 147 45.03 1 re f", true),
        ("150 150 m 190 150 l S", false),
        ("180 150 m 300 150 l S", false),
        ("10 147 45.2 1 re f", false),
    ] {
        let content =
            format!("q 5 145 50 15 re W n {drawn} BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET Q");
        let (mut editor, block) = spaced_block(content.as_bytes());
        let applied = editor.edit(0, block, insertion_at(0, 2), "\n");
        assert!(
            matches!(applied, Applied::Changed { .. }),
            "{drawn}: {applied:?}"
        );
        assert_placed(
            &placed(&editor, A),
            &[row(10.0, 150.0, 2), row(10.0, 140.0, 2)].concat(),
        );
        assert_eq!(clipped(&editor), !lifted, "{drawn}");
    }
}

#[test]
fn a_clip_with_rounded_corners_that_hides_nothing_is_lifted() {
    let cell = "7 145 m 53 145 l 54.1 145 55 145.9 55 147 c 55 158 l 55 159.1 54.1 160 53 160 c \
                7 160 l 5.9 160 5 159.1 5 158 c 5 147 l 5 145.9 5.9 145 7 145 c h W n";
    let content = format!("q {cell} BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET Q");
    let (mut editor, block) = spaced_block(content.as_bytes());
    let applied = editor.edit(0, block, insertion_at(0, 2), "\n");
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    assert_placed(
        &placed(&editor, A),
        &[row(10.0, 150.0, 2), row(10.0, 140.0, 2)].concat(),
    );

    assert!(
        !editor
            .status()
            .say(crate::wording::Lang::English)
            .contains("behind a crop"),
        "control: a lifted clip hides nothing, so nothing is said"
    );

    let content =
        format!("q {cell} 0 100 m 100 200 l S BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET Q");
    let (mut cut, block) = spaced_block(content.as_bytes());
    let applied = cut.edit(0, block, insertion_at(0, 2), "\n");
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    let said = cut.status().say(crate::wording::Lang::English);
    assert!(said.contains("behind a crop"), "{said}");
}

#[test]
fn a_clip_that_hides_something_grows_over_a_new_line_when_that_shows_nothing_more() {
    let (mut editor, block) = spaced_block(
        b"q 5 145 50 15 re W n 0 152 m 40 152 l S BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET Q",
    );
    let applied = editor.edit(0, block, insertion_at(0, 2), "\n");
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    assert_placed(
        &placed(&editor, A),
        &[row(10.0, 150.0, 2), row(10.0, 140.0, 2)].concat(),
    );
    let view = pdf_session::interpret_page(editor.source().unwrap(), 0).unwrap();
    let clip = view
        .graph
        .atoms
        .iter()
        .find_map(|atom| match &atom.kind {
            pdf_paint::PaintAtomKind::Text(text) => text.state.clip_paths.first().cloned(),
            _ => None,
        })
        .expect("the text is still clipped");
    let bounds = pdf_paint::PaintAtomKind::Path(pdf_paint::PathPaint {
        path: clip.path.clone(),
        stroke: false,
        fill: None,
        state: pdf_paint::GraphicsState::default(),
    })
    .user_bounds()
    .unwrap();
    assert!(
        bounds
            .iter()
            .zip([5.0, 135.0, 55.0, 165.0])
            .all(|(one, other)| (one - other).abs() < 1e-6),
        "{bounds:?}"
    );

    let (mut shown, block) = spaced_block(
        b"q 5 145 50 15 re W n 10 141 m 40 141 l S BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET Q",
    );
    let applied = shown.edit(0, block, insertion_at(0, 2), "\n");
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    let said = shown.status().say(crate::wording::Lang::English);
    assert!(said.contains("behind a crop"), "{said}");
}

#[test]
fn moving_a_block_out_of_a_clip_that_hides_nothing_lifts_it() {
    let (mut editor, block) =
        spaced_block(b"q 5 145 50 15 re W n BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET Q");
    let anchors = editor.leaf(0).unwrap().overlay.blocks[block]
        .anchors
        .clone();
    let applied = editor.move_block(0, &anchors, 0.0, 30.0);
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    assert_placed(&placed(&editor, A), &row(10.0, 120.0, 4));
    assert!(
        !editor
            .status()
            .say(crate::wording::Lang::English)
            .contains("behind a crop"),
        "control: a lifted clip hides nothing, so nothing is said"
    );

    let (mut cut, block) = spaced_block(
        b"q 5 145 50 15 re W n 0 100 m 100 200 l S \
          BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET Q",
    );
    let anchors = cut.leaf(0).unwrap().overlay.blocks[block].anchors.clone();
    let applied = cut.move_block(0, &anchors, 0.0, 30.0);
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    let said = cut.status().say(crate::wording::Lang::English);
    assert!(said.contains("behind a crop"), "{said}");
}

#[test]
fn moving_the_second_cell_of_a_shared_show_moves_that_cell() {
    let source = spaced_fixture_with(SHARED_ROW, "[600 600 600]", "/Ascent 800 /Descent -200 ");
    let before = pens(&source);
    let mut editor = Editor::open(source).unwrap();
    read(&mut editor);
    let leaf = editor.leaf(0).unwrap();
    let cells: Vec<usize> = (0..leaf.overlay.blocks.len())
        .filter(|block| {
            leaf.overlay.blocks[*block]
                .lines
                .first()
                .is_some_and(|line| {
                    let line = &leaf.view.index.lines[*line];
                    leaf.view.index.clusters[line.clusters[0]].baseline.y > 100.0
                })
        })
        .collect();
    assert_eq!(cells.len(), 2, "two cells on the row");
    assert_eq!(
        leaf.overlay.blocks[cells[0]].anchors, leaf.overlay.blocks[cells[1]].anchors,
        "control: the cells cannot be told apart by their anchors"
    );
    let second = cells
        .into_iter()
        .find(|block| {
            let line = &leaf.view.index.lines[leaf.overlay.blocks[*block].lines[0]];
            line.clusters.len() == 2
        })
        .unwrap();
    let applied = editor.move_text_block(0, second, 7.0, -5.0);
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    let mut wanted = before.clone();
    for pen in wanted.iter_mut().filter(|pen| pen.2 > 100.0 && pen.0 == B) {
        pen.1 += 7.0;
        pen.2 += 5.0;
    }
    let sorted = |pens: &[(u32, f64, f64)]| {
        let mut pens = pens.to_vec();
        pens.sort_by(|one, other| one.1.total_cmp(&other.1).then(one.2.total_cmp(&other.2)));
        pens
    };
    assert_pens(&sorted(&pens(editor.source().unwrap())), &sorted(&wanted));
}

#[test]
fn a_block_whose_first_run_is_part_way_along_its_line_moves() {
    let source = spaced_fixture_with(
        b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj 60 Tc (C) Tj 0 Tc (BB) Tj ET",
        "[600 600 600]",
        "/Ascent 800 /Descent -200 ",
    );
    let before = pens(&source);
    let mut editor = Editor::open(source).unwrap();
    read(&mut editor);
    let leaf = editor.leaf(0).unwrap();
    let block = (0..leaf.overlay.blocks.len())
        .find(|block| {
            let line = &leaf.view.index.lines[leaf.overlay.blocks[*block].lines[0]];
            let cluster = &leaf.view.index.clusters[line.clusters[0]];
            cluster.baseline.x > 90.0
        })
        .expect("BB is a block of its own");
    let applied = editor.move_text_block(0, block, 7.0, -5.0);
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    let mut wanted = before.clone();
    for pen in wanted.iter_mut().filter(|pen| pen.0 == B) {
        pen.1 += 7.0;
        pen.2 += 5.0;
    }
    assert_placed(&placed(&editor, B), &[(107.0, 155.0), (113.0, 155.0)]);
    let sorted = |pens: &[(u32, f64, f64)]| {
        let mut pens = pens.to_vec();
        pens.sort_by(|one, other| one.1.total_cmp(&other.1).then(one.2.total_cmp(&other.2)));
        pens
    };
    assert_pens(&sorted(&pens(editor.source().unwrap())), &sorted(&wanted));
}

const TURNED: &[u8] =
    b"BT /F1 10 Tf 0 1 -1 0 100 20 Tm (AAAACAAAA) Tj ET BT /F1 10 Tf 1 0 0 1 10 180 Tm (BB) Tj ET";

fn block_of_a(editor: &Editor) -> usize {
    let view = &editor.leaf(0).unwrap().view;
    (0..view.index.blocks.len())
        .find(|block| {
            view.index.blocks[*block].lines.iter().any(|line| {
                view.index.lines[*line].clusters.iter().any(|cluster| {
                    let cluster = &view.index.clusters[*cluster];
                    matches!(&view.graph.atoms[cluster.atom].kind,
                        pdf_paint::PaintAtomKind::Text(text) if text.glyphs[cluster.glyphs.start].code.value == A)
                })
            })
        })
        .unwrap()
}

#[test]
fn a_turned_paragraph_wraps_and_breaks_along_its_own_baseline() {
    let mut editor = Editor::open(spaced_fixture_with(
        TURNED,
        "[600 600 600]",
        "/Ascent 800 /Descent -200 ",
    ))
    .unwrap();
    read(&mut editor);
    let block = block_of_a(&editor);
    let line = editor.leaf(0).unwrap().view.index.blocks[block].lines[0];
    let frame = editor.frame_boxes(0)[block];
    let quad = editor.leaf(0).unwrap().overlay.blocks[block].quad;
    assert!(
        (quad[1][0] - quad[0][0]).abs() < 1e-6 && (quad[1][1] - quad[0][1] + 54.0).abs() < 1e-6,
        "{quad:?}"
    );
    let typed = editor.type_text(0, line, 9, 9, " AAAA");
    let Applied::Changed {
        region: Some(region),
        ..
    } = typed
    else {
        panic!("{typed:?}");
    };
    assert!(region[0] <= 100.0 && region[2] >= 110.0, "{region:?}");
    let column = |x: f64, ys: &[f64]| ys.iter().map(|y| (x, *y)).collect::<Vec<_>>();
    let mut wanted = column(100.0, &[20.0, 26.0, 32.0, 38.0, 50.0, 56.0, 62.0, 68.0]);
    wanted.extend(column(110.0, &[20.0, 26.0, 32.0, 38.0]));
    assert_placed(&placed(&editor, A), &wanted);
    assert_placed(&placed(&editor, B), &[(10.0, 180.0), (16.0, 180.0)]);
    assert_eq!(editor.landed_caret(), Some((1, 4)));
    read(&mut editor);
    let grown = editor.frame_boxes(0)[block_of_a(&editor)];
    assert!(
        (grown[3] - frame[3] - 10.0).abs() < 1e-6 && (grown[2] - frame[2]).abs() < 1e-6,
        "{frame:?} -> {grown:?}"
    );

    let mut editor = Editor::open(spaced_fixture_with(
        TURNED,
        "[600 600 600]",
        "/Ascent 800 /Descent -200 ",
    ))
    .unwrap();
    read(&mut editor);
    let block = block_of_a(&editor);
    let line = editor.leaf(0).unwrap().view.index.blocks[block].lines[0];
    assert!(matches!(
        editor.type_text(0, line, 5, 5, "\n"),
        Applied::Changed { .. }
    ));
    let mut wanted = column(100.0, &[20.0, 26.0, 32.0, 38.0]);
    wanted.extend(column(110.0, &[20.0, 26.0, 32.0, 38.0]));
    assert_placed(&placed(&editor, A), &wanted);
}

#[test]
fn the_empty_line_after_a_turned_paragraph_is_placed_along_it() {
    let mut editor = Editor::open(spaced_fixture_with(
        TURNED,
        "[600 600 600]",
        "/Ascent 800 /Descent -200 ",
    ))
    .unwrap();
    read(&mut editor);
    let block = block_of_a(&editor);
    let line = editor.leaf(0).unwrap().view.index.blocks[block].lines[0];
    assert!(matches!(
        editor.type_text(0, line, 9, 9, "\n"),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    let leaf = editor.leaf(0).unwrap();
    let block = block_of_a(&editor);
    let lines = &leaf.overlay.blocks[block].lines;
    assert_eq!(lines.len(), 2, "the row and the empty line");
    let stop = leaf
        .overlay
        .carets
        .iter()
        .find(|stop| stop.line == lines[1])
        .expect("the empty line has a stop");
    assert!(
        (stop.at[0] - 110.0).abs() < 1e-6 && (stop.at[1] - 180.0).abs() < 1e-6,
        "{stop:?}"
    );
    assert!(
        (stop.up[0] + 10.0).abs() < 1e-6 && stop.up[1].abs() < 1e-6,
        "{stop:?}"
    );
}

#[test]
fn a_paragraph_turned_from_the_toolbar_takes_typing_along_its_angle() {
    let mut editor = Editor::open(reflow_source()).unwrap();
    read(&mut editor);
    let block = top_block(&editor.leaf(0).unwrap().view);
    let anchors = editor.leaf(0).unwrap().overlay.blocks[block]
        .anchors
        .clone();
    let job = editor
        .begin_set_angles(0, &anchors, Some(30.0), None)
        .unwrap();
    assert!(matches!(editor.adopt(job.run()), Applied::Changed { .. }));
    read(&mut editor);
    let turned = placed(&editor, A);
    let blocks_before = placed(&editor, B);
    assert_eq!(turned.len(), 8);
    let block = block_of_a(&editor);
    assert!(editor.leaf(0).unwrap().overlay.blocks[block].turn > 0.5);
    let line = editor.leaf(0).unwrap().view.index.blocks[block].lines[0];
    let typed = editor.type_text(0, line, 9, 9, " AAAA");
    assert!(matches!(typed, Applied::Changed { .. }), "{typed:?}");
    let (down_x, down_y) = (
        30f64.to_radians().sin() * 10.0,
        -30f64.to_radians().cos() * 10.0,
    );
    let mut wanted = turned.clone();
    wanted.extend(turned[..4].iter().map(|(x, y)| (x + down_x, y + down_y)));
    assert_placed(&placed(&editor, A), &wanted);
    assert_placed(&placed(&editor, B), &blocks_before);
    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    assert_placed(&placed(&editor, A), &turned);
}

#[test]
fn a_flipped_paragraph_turned_from_the_toolbar_keeps_its_pitch_while_typed_into() {
    let mut editor = Editor::open(spaced_fixture_with(
        b"0.75 0 0 -0.75 0 200 cm BT /F1 16 Tf 20 TL 1 0 0 -1 20 60 Tm (AAAACAAAA) Tj \
          1 0 0 -1 20 80 Tm (AAAA) Tj ET",
        "[600 600 600]",
        "/Ascent 800 /Descent -200 ",
    ))
    .unwrap();
    read(&mut editor);
    let pitch = |editor: &Editor| {
        let view = &editor.leaf(0).unwrap().view;
        let block = &view.index.blocks[block_of_a(editor)];
        let seed = |row: usize| {
            let line = &view.index.lines[block.lines[row]];
            view.index.clusters[line.clusters[0]].baseline
        };
        let (one, other) = (seed(0), seed(1));
        (one.x - other.x).hypot(one.y - other.y)
    };
    assert!((pitch(&editor) - 15.0).abs() < 1e-6, "{}", pitch(&editor));
    let block = block_of_a(&editor);
    let anchors = editor.leaf(0).unwrap().overlay.blocks[block]
        .anchors
        .clone();
    let job = editor
        .begin_set_angles(0, &anchors, Some(20.0), None)
        .unwrap();
    assert!(matches!(editor.adopt(job.run()), Applied::Changed { .. }));
    read(&mut editor);
    assert!(
        (pitch(&editor) - 15.0).abs() < 1e-6,
        "turned: {}",
        pitch(&editor)
    );
    for step in 0..3 {
        let block = block_of_a(&editor);
        let stops = editor
            .leaf(0)
            .unwrap()
            .overlay
            .carets
            .iter()
            .filter(|stop| stop.line == editor.leaf(0).unwrap().overlay.blocks[block].lines[0])
            .count();
        let typed = editor.edit(
            0,
            block,
            pdf_edit::BlockRange::Between {
                from: (0, stops - 1),
                to: (0, stops - 1),
            },
            "A",
        );
        assert!(
            matches!(typed, Applied::Changed { .. }),
            "{step}: {typed:?}"
        );
        read(&mut editor);
        assert!(
            (pitch(&editor) - 15.0).abs() < 1e-6,
            "after letter {step}: {}",
            pitch(&editor)
        );
    }
}

#[test]
fn a_moved_turned_paragraph_keeps_its_frame_along_it() {
    let mut editor = Editor::open(spaced_fixture_with(
        TURNED,
        "[600 600 600]",
        "/Ascent 800 /Descent -200 ",
    ))
    .unwrap();
    read(&mut editor);
    let block = block_of_a(&editor);
    let frame = editor.frame_boxes(0)[block];
    let moved = editor.move_text_block(0, block, 5.0, 0.0);
    assert!(matches!(moved, Applied::Changed { .. }), "{moved:?}");
    read(&mut editor);
    let block = block_of_a(&editor);
    let shifted = editor.frame_boxes(0)[block];
    assert!(
        shifted
            .iter()
            .zip([frame[0], frame[1] + 5.0, frame[2], frame[3] + 5.0])
            .all(|(one, other)| (one - other).abs() < 1e-6),
        "{frame:?} -> {shifted:?}"
    );
    let line = editor.leaf(0).unwrap().view.index.blocks[block].lines[0];
    let typed = editor.type_text(0, line, 9, 9, " AAAA");
    assert!(matches!(typed, Applied::Changed { .. }), "{typed:?}");
    let column = |x: f64, ys: &[f64]| ys.iter().map(|y| (x, *y)).collect::<Vec<_>>();
    let mut wanted = column(105.0, &[20.0, 26.0, 32.0, 38.0, 50.0, 56.0, 62.0, 68.0]);
    wanted.extend(column(115.0, &[20.0, 26.0, 32.0, 38.0]));
    assert_placed(&placed(&editor, A), &wanted);
}

#[test]
fn enter_in_a_paragraph_at_any_quarter_turn_breaks_along_it() {
    let cases: [(&str, [(f64, f64); 8]); 3] = [
        (
            "0 -1 1 0 100 180",
            [
                (100.0, 180.0),
                (100.0, 174.0),
                (90.0, 180.0),
                (90.0, 174.0),
                (90.0, 162.0),
                (90.0, 156.0),
                (90.0, 150.0),
                (90.0, 144.0),
            ],
        ),
        (
            "-1 0 0 -1 180 100",
            [
                (180.0, 100.0),
                (174.0, 100.0),
                (180.0, 110.0),
                (174.0, 110.0),
                (162.0, 110.0),
                (156.0, 110.0),
                (150.0, 110.0),
                (144.0, 110.0),
            ],
        ),
        (
            "0 1 -1 0 100 20",
            [
                (100.0, 20.0),
                (100.0, 26.0),
                (110.0, 20.0),
                (110.0, 26.0),
                (110.0, 38.0),
                (110.0, 44.0),
                (110.0, 50.0),
                (110.0, 56.0),
            ],
        ),
    ];
    for (tm, wanted) in cases {
        let content = format!(
            "BT /F1 10 Tf {tm} Tm (AAAACAAAA) Tj ET BT /F1 10 Tf 1 0 0 1 10 180 Tm (BB) Tj ET"
        );
        let mut editor = Editor::open(spaced_fixture_with(
            content.as_bytes(),
            "[600 600 600]",
            "/Ascent 800 /Descent -200 ",
        ))
        .unwrap();
        read(&mut editor);
        let block = block_of_a(&editor);
        let line = editor.leaf(0).unwrap().view.index.blocks[block].lines[0];
        let entered = editor.type_text(0, line, 2, 2, "\n");
        assert!(
            matches!(entered, Applied::Changed { .. }),
            "{tm}: {entered:?}"
        );
        assert_placed(&placed(&editor, A), &wanted);
        assert_eq!(editor.landed_caret(), Some((1, 0)), "{tm}");
    }
}

#[test]
fn a_sign_typed_between_a_letter_and_its_nukta_goes_after_the_nukta() {
    let source = spaced_fixture_program(
        b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (A) Tj (B) Tj (AA) Tj ET",
        ("[600 0 600]", "/Ascent 800 /Descent -200 "),
        SPACED_CFF,
        b"<41> <0041> <42> <0B3C> <43> <0B3F>",
    );
    let mut editor = Editor::open(source).unwrap();
    read(&mut editor);
    let block = block_of_a(&editor);
    let rect = editor.frame_boxes(0)[block];
    declare_frame(&mut editor, 0, block, rect);
    let line = editor.leaf(0).unwrap().view.index.blocks[block].lines[0];
    let typed = editor.type_text(0, line, 1, 1, "\u{0B3F}");
    assert!(matches!(typed, Applied::Changed { .. }), "{typed:?}");
    read(&mut editor);
    assert_placed(&placed(&editor, B), &[(16.0, 150.0)]);
    assert_placed(&placed(&editor, SPACE), &[(16.0, 150.0)]);
    assert_placed(
        &placed(&editor, A),
        &[(10.0, 150.0), (22.0, 150.0), (10.0, 140.0)],
    );
    let block = block_of_a(&editor);
    let text = editor.copy_text(0, block, (0, 0), (1, 1));
    assert_eq!(text.as_deref(), Some("A\u{0B3C}\u{0B3F}AA"));
}

#[test]
fn enter_between_a_letter_and_its_mark_breaks_after_the_mark() {
    let source = spaced_fixture_program(
        b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (A) Tj (B) Tj (ACA) Tj ET",
        ("[600 0 600]", "/Ascent 800 /Descent -200 "),
        SPACED_CFF,
        b"<41> <0041> <42> <0E48> <43> <0020>",
    );
    let mut editor = Editor::open(source).unwrap();
    read(&mut editor);
    let block = block_of_a(&editor);
    let line = editor.leaf(0).unwrap().view.index.blocks[block].lines[0];
    let entered = editor.type_text(0, line, 1, 1, "\n");
    assert!(matches!(entered, Applied::Changed { .. }), "{entered:?}");
    assert_placed(&placed(&editor, B), &[(16.0, 150.0)]);
    assert_placed(
        &placed(&editor, A),
        &[(10.0, 150.0), (10.0, 140.0), (22.0, 140.0)],
    );

    let source = spaced_fixture_program(
        b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (B) Tj (B) Tj (AA) Tj ET",
        ("[600 0 600]", "/Ascent 800 /Descent -200 "),
        SPACED_CFF,
        b"<41> <0041> <42> <0E48> <43> <0020>",
    );
    let mut editor = Editor::open(source).unwrap();
    read(&mut editor);
    let block = block_of_a(&editor);
    let line = editor.leaf(0).unwrap().view.index.blocks[block].lines[0];
    let entered = editor.type_text(0, line, 1, 1, "\n");
    assert!(matches!(entered, Applied::Changed { .. }), "{entered:?}");
    assert_placed(&placed(&editor, B), &[(10.0, 150.0), (10.0, 140.0)]);
}

#[test]
fn an_edit_hands_back_the_page_it_proved_and_it_is_the_committed_page() {
    let mut editor = editor();
    let block = 0;
    let line = editor.leaf(0).unwrap().view.index.blocks[block].lines[0];
    let typed = editor.type_text(0, line, 1, 1, "B");
    assert!(matches!(typed, Applied::Changed { .. }), "{typed:?}");

    let kept = editor
        .leaf(0)
        .expect("the edit hands its reading back rather than dropping the page")
        .clone();

    let source = editor.source().unwrap().clone();
    let fresh = pdf_session::interpret_page_grouped(&source, 0, b"", editor.grouping(0).as_deref())
        .expect("the committed page reads");

    assert_eq!(
        kept.view.index.blocks.len(),
        fresh.index.blocks.len(),
        "the kept reading has the blocks the committed page has"
    );
    assert_eq!(
        kept.view.index.lines.len(),
        fresh.index.lines.len(),
        "and its rows"
    );
    assert_eq!(
        kept.view.index.clusters.len(),
        fresh.index.clusters.len(),
        "and its clusters"
    );
    for (row, line) in kept.view.index.lines.iter().enumerate() {
        assert_eq!(
            line.clusters.len(),
            fresh.index.lines[row].clusters.len(),
            "row {row} holds the same clusters in both readings"
        );
    }
    let origin = |index: &pdf_semantics::SemanticIndex| -> Vec<(f64, f64)> {
        index
            .clusters
            .iter()
            .map(|cluster| {
                let [x0, y0, ..] = cluster.bounds.unwrap_or([f64::NAN; 4]);
                (
                    (x0 * 1000.0).round() / 1000.0,
                    (y0 * 1000.0).round() / 1000.0,
                )
            })
            .collect()
    };
    assert_eq!(
        origin(&kept.view.index),
        origin(&fresh.index),
        "every cluster of the kept reading sits where the committed page puts it"
    );
    assert_eq!(
        kept.view.index.clusters.len(),
        7,
        "AAAA with a B typed into it, and the second block's AA"
    );
}

#[test]
fn the_stand_in_is_one_blank_a4_page() {
    let editor = Editor::stand_in().expect("the stand-in opens");
    assert_eq!(editor.page_count(), 1);
    assert_eq!(editor.strip().page_size(0), Some((595.0, 842.0)));
}

#[test]
fn text_typed_in_a_chosen_style_is_set_in_it() {
    let (mut editor, block) = spaced_block(b"0 g BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET");
    let rect = editor.frame_boxes(0)[block];
    declare_frame(
        &mut editor,
        0,
        block,
        [rect[0], rect[1], rect[2] + 60.0, rect[3]],
    );
    let plain = styled(&editor)[0].2.clone();
    let chosen = pdf_edit::TextStyle {
        size: Some(20.0),
        fill: Some([1.0, 0.0, 0.0]),
        ..pdf_edit::TextStyle::default()
    };
    let before = styled(&editor);
    let changed = editor.edit_in_style(0, block, insertion_at(0, 4), "BB", chosen);
    assert!(
        matches!(changed, Applied::Changed { .. }),
        "{changed:?} {}",
        editor.status()
    );
    let glyphs = styled(&editor);
    let red = glyphs
        .iter()
        .find(|glyph| glyph.0 == B)
        .expect("a B")
        .2
        .clone();
    assert_ne!(red, plain, "the typed text is not in the block's colour");
    assert_eq!(
        glyphs,
        vec![
            (A, 10.0, plain.clone()),
            (A, 10.0, plain.clone()),
            (A, 10.0, plain.clone()),
            (A, 10.0, plain),
            (B, 20.0, red.clone()),
            (B, 20.0, red.clone()),
        ]
    );
    assert_placed(&placed(&editor, A), &row(10.0, 150.0, 4));
    assert_placed(&placed(&editor, B), &[(34.0, 150.0), (46.0, 150.0)]);

    read(&mut editor);
    let caret = editor.landed_caret().expect("a caret after the typing");
    assert_eq!(caret, (0, 6));
    assert!(matches!(
        editor.edit(0, block, insertion_at(0, 6), "B"),
        Applied::Changed { .. }
    ));
    let last = styled(&editor).last().cloned().expect("a glyph");
    assert_eq!(last, (B, 20.0, red));

    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    assert_eq!(styled(&editor), before, "undo paints the page as it was");
    assert_placed(&placed(&editor, A), &row(10.0, 150.0, 4));
}

#[test]
#[expect(
    clippy::cast_possible_truncation,
    reason = "a baseline in hundredths of a point on a 200 pt page"
)]
fn letters_typed_on_a_row_of_larger_text_do_not_move_it_down() {
    for (size, below) in [(20.0_f64, 20.0_f64), (15.0, 15.0)] {
        let (mut editor, block) =
            declared_block(b"0 g BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET");
        let larger = pdf_edit::TextStyle {
            size: Some(size),
            ..pdf_edit::TextStyle::default()
        };
        let changed = editor.edit_in_style(0, block, insertion_at(0, 4), "A", larger);
        assert!(
            matches!(changed, Applied::Changed { .. }),
            "{changed:?} {}",
            editor.status()
        );
        let mut caret = editor.landed_caret().expect("a caret");
        read(&mut editor);
        let rows = |editor: &Editor| -> Vec<i64> {
            let set: std::collections::BTreeSet<i64> = placed(editor, A)
                .iter()
                .map(|(_, y)| (y * 100.0).round() as i64)
                .collect();
            set.into_iter().collect()
        };
        let at = |steps: f64| (below.mul_add(-steps, 150.0) * 100.0).round() as i64;
        assert_eq!(rows(&editor), vec![at(1.0), at(0.0)], "{size} pt");
        for _ in 0..3 {
            let changed = editor.edit(0, block, insertion_at(caret.0, caret.1), "A");
            assert!(
                matches!(changed, Applied::Changed { .. }),
                "{changed:?} {}",
                editor.status()
            );
            caret = editor.landed_caret().expect("a caret after each letter");
            read(&mut editor);
            assert!(
                rows(&editor)
                    .iter()
                    .all(|y| [at(2.0), at(1.0), at(0.0)].contains(y)),
                "{size} pt: {:?}",
                rows(&editor)
            );
            assert_eq!(editor.block_pitch(0, block), Some(10.0), "{size} pt");
        }
        assert_eq!(rows(&editor), vec![at(2.0), at(1.0), at(0.0)], "{size} pt");
        assert_eq!(placed(&editor, A).len(), 8, "{size} pt");
    }
}

#[test]
fn text_in_a_chosen_style_is_not_typed_a_row_at_a_time() {
    let content: &[u8] = b"BT /F1 24 Tf 1 0 0 -1 10 150 Tm (AAA) Tj ET";
    let mut plain = Editor::open(fixture(content)).unwrap();
    read(&mut plain);
    let control = plain.edit(
        0,
        0,
        pdf_edit::BlockRange::Between {
            from: (0, 1),
            to: (0, 2),
        },
        "A",
    );
    assert!(
        matches!(control, Applied::Changed { .. }),
        "control: {control:?}"
    );

    let mut styled = Editor::open(fixture(content)).unwrap();
    read(&mut styled);
    let before = styled.source().unwrap().as_bytes().to_vec();
    let refused = styled.edit_in_style(
        0,
        0,
        pdf_edit::BlockRange::Between {
            from: (0, 1),
            to: (0, 2),
        },
        "A",
        pdf_edit::TextStyle {
            size: Some(20.0),
            fill: Some([1.0, 0.0, 0.0]),
            ..pdf_edit::TextStyle::default()
        },
    );
    assert!(matches!(refused, Applied::Refused(_)), "{refused:?}");
    assert_eq!(styled.source().unwrap().as_bytes(), before);
}

#[test]
fn text_is_stroked_in_the_modes_that_stroke_it() {
    let pixel = |mode: u8, (x, y): (usize, usize)| -> [u8; 3] {
        let content =
            format!("0 g 1 0 0 RG 6 w BT /F1 40 Tf {mode} Tr 1 0 0 1 10 100 Tm (A) Tj ET");
        let source = fixture(content.as_bytes());
        let view = pdf_session::interpret_page(&source, 0).expect("the page reads");
        let (canvas, _) = crate::painter::draw_page(&view, 2.0).expect("it draws");
        let rgb = canvas.to_rgb8();
        let width = canvas.width as usize;
        let at = (y * width + x) * 3;
        [rgb[at], rgb[at + 1], rgb[at + 2]]
    };
    let middle = (40, 180);
    let just_outside = (40, 156);
    let far_outside = (40, 150);
    let (black, white, red) = ([0, 0, 0], [255, 255, 255], [255, 0, 0]);
    assert_eq!(pixel(0, middle), black, "fill: the middle is ink");
    assert_eq!(pixel(0, far_outside), white, "fill: no pen outside");
    assert_eq!(pixel(2, middle), black, "fill and stroke: filled");
    assert_eq!(pixel(2, just_outside), red, "fill and stroke: the pen");
    assert_eq!(pixel(1, middle), white, "stroke: hollow");
    assert_eq!(pixel(1, just_outside), red, "stroke: the pen");
    assert_eq!(pixel(3, middle), white, "invisible");
}

#[test]
fn a_paragraph_emptied_a_letter_at_a_time_takes_typing_in_its_own_style() {
    let (mut editor, block) = spaced_block(b"0 1 0 rg BT /F1 10 Tf 1 0 0 1 10 150 Tm (AA) Tj ET");
    let green = styled(&editor)[0].2.clone();
    let backspace = |editor: &mut Editor, at| {
        editor.edit(
            0,
            block,
            pdf_edit::BlockRange::Units {
                at,
                backwards: true,
                count: 1,
            },
            "",
        )
    };
    assert!(matches!(
        backspace(&mut editor, (0, 2)),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    let last = backspace(&mut editor, (0, 1));
    assert!(
        matches!(last, Applied::Changed { .. }),
        "{last:?} {}",
        editor.status()
    );
    assert_eq!(editor.landed_caret(), Some((0, 0)));
    read(&mut editor);
    assert!(styled(&editor).is_empty(), "no glyph is painted");
    let leaf = editor.leaf(0).unwrap();
    let shows: Vec<_> = leaf
        .view
        .graph
        .atoms
        .iter()
        .filter_map(|atom| match &atom.kind {
            pdf_paint::PaintAtomKind::Text(text) => Some((
                text.glyphs.len(),
                text.state.text.font_size.value,
                text.matrices.text.value.e,
                text.matrices.text.value.f,
            )),
            _ => None,
        })
        .collect();
    assert_eq!(shows, vec![(0, 10.0, 10.0, 150.0)]);
    let lines = leaf.overlay.blocks[block].lines.clone();
    assert_eq!(lines.len(), 1, "one empty line");
    let stops: Vec<_> = leaf
        .overlay
        .carets
        .iter()
        .filter(|stop| stop.line == lines[0])
        .map(|stop| (stop.offset, stop.at, stop.up))
        .collect();
    assert_eq!(stops, vec![(0, [10.0, 50.0], [0.0, -10.0])]);
    assert_eq!(
        leaf.overlay.blocks[block].shape.map(|shape| shape.size),
        Some(10.0),
        "the font box shows the size the next letter is typed in"
    );
    assert!(matches!(backspace(&mut editor, (0, 0)), Applied::Unchanged));

    let typed = editor.edit(0, block, insertion_at(0, 0), "BA");
    assert!(
        matches!(typed, Applied::Changed { .. }),
        "{typed:?} {}",
        editor.status()
    );
    assert_eq!(editor.landed_caret(), Some((0, 2)));
    read(&mut editor);
    assert_eq!(
        styled(&editor),
        vec![(B, 10.0, green.clone()), (A, 10.0, green)]
    );
    assert_placed(&placed(&editor, B), &[(10.0, 150.0)]);
    assert_eq!(
        editor.copy_text(0, block, (0, 0), (0, 2)).as_deref(),
        Some("BA")
    );

    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    read(&mut editor);
    assert!(styled(&editor).is_empty(), "undo: empty again");
    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    read(&mut editor);
    assert_eq!(placed(&editor, A), vec![(10.0, 150.0)], "undo: the A");
}

#[test]
fn a_page_number_is_stamped_and_taken_back_with_one_undo() {
    let mut editor = editor();
    let atoms = |editor: &Editor| {
        pdf_session::interpret_page(editor.source().unwrap(), 0)
            .unwrap()
            .graph
            .atoms
            .len()
    };
    let before = atoms(&editor);
    let stamp = pdf_edit::stamp::Stamp {
        wording: "{page} / {pages}".to_owned(),
        spot: pdf_edit::stamp::Spot::Along {
            edge: pdf_edit::stamp::Edge::Footer,
            side: pdf_edit::stamp::Side::Centre,
        },
        family: "DejaVu Sans".to_owned(),
        size: 10.0,
        bold: false,
        italic: false,
        fill: None,
        opacity: 1.0,
        margin: 20.0,
    };
    let applied = editor.stamp(
        vec![0],
        stamp,
        (1, "a.pdf".to_owned(), "18/09/2026".to_owned()),
    );
    assert!(
        matches!(applied, Applied::Changed { page: 0, .. }),
        "{applied:?}"
    );
    assert_eq!(
        editor.status(),
        &crate::wording::Message::Done(crate::wording::Done::Stamped { count: 1 })
    );
    assert_eq!(atoms(&editor), before + 1, "one line, one text object");
    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    assert_eq!(atoms(&editor), before);
}

#[test]
fn words_read_from_a_page_are_written_under_it_and_taken_back_with_one_undo() {
    use pdf_edit::text_layer::{LayerWord, TextLayer};
    let mut editor = editor();
    let atoms = |editor: &Editor| {
        pdf_session::interpret_page(editor.source().unwrap(), 0)
            .unwrap()
            .graph
            .atoms
            .len()
    };
    let before = atoms(&editor);
    let layer = |text: &str| TextLayer {
        words: vec![LayerWord {
            text: text.to_owned(),
            frame: [20.0, 20.0, 80.0, 32.0],
        }],
    };
    let applied = editor.text_layers(
        vec![(0, layer("ສະບາຍດີ")), (0, layer("\u{1F600}"))],
        (87, 2, 1),
    );
    assert!(
        matches!(applied, Applied::Changed { page: 0, .. }),
        "{applied:?}"
    );
    assert_eq!(
        editor.status(),
        &crate::wording::Message::Done(crate::wording::Done::Recognized {
            pages: 1,
            confidence: 87,
            had_text: 2,
            refused: 2,
        })
    );
    assert_eq!(atoms(&editor), before + 1, "one word, one text object");
    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    assert_eq!(atoms(&editor), before);
    let applied = editor.text_layers(vec![(0, layer("\u{1F600}"))], (87, 0, 0));
    assert!(matches!(applied, Applied::Refused(_)), "{applied:?}");
    assert_eq!(atoms(&editor), before);
}

#[test]
fn a_stamp_on_a_turned_page_reads_left_to_right_along_the_bottom_it_is_shown_with() {
    let source = fixture_page(
        b"BT /F1 24 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET",
        (b"<41> <0041> <42> <0042>", "", "[600 600]"),
        "/Rotate 90 ",
    );
    let mut editor = Editor::open(source).unwrap();
    read(&mut editor);
    let stamp = pdf_edit::stamp::Stamp {
        wording: "12".to_owned(),
        spot: pdf_edit::stamp::Spot::Along {
            edge: pdf_edit::stamp::Edge::Footer,
            side: pdf_edit::stamp::Side::Left,
        },
        family: "DejaVu Sans".to_owned(),
        size: 10.0,
        bold: false,
        italic: false,
        fill: None,
        opacity: 1.0,
        margin: 20.0,
    };
    let applied = editor.stamp(vec![0], stamp, (1, String::new(), String::new()));
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    let view = pdf_session::interpret_page(editor.source().unwrap(), 0).unwrap();
    let text = view
        .graph
        .atoms
        .iter()
        .rev()
        .find_map(|atom| match &atom.kind {
            pdf_paint::PaintAtomKind::Text(text) => Some(text),
            _ => None,
        })
        .unwrap();
    let device = pdf_render::DeviceTransform::for_page(
        &view.program.geometry,
        1.0,
        pdf_render::RenderLimits::default(),
    )
    .unwrap();
    let (width, height) = (f64::from(device.width), f64::from(device.height));
    let pens: Vec<pdf_paint::Point> = text
        .glyphs
        .iter()
        .map(|glyph| {
            let at = text
                .state
                .ctm
                .value
                .multiply(glyph.matrix)
                .transform(pdf_paint::Point { x: 0.0, y: 0.0 });
            device.matrix.transform(at)
        })
        .collect();
    assert_eq!(pens.len(), 2, "two digits");
    assert!((pens[0].x - 20.0).abs() < 0.5, "{pens:?} of {width}");
    assert!(
        (pens[0].y - (height - 20.0)).abs() < 0.5,
        "{pens:?} of {height}"
    );
    assert!(pens[1].x > pens[0].x + 1.0, "{pens:?}");
    assert!((pens[1].y - pens[0].y).abs() < 0.01, "{pens:?}");
    let up = text.state.ctm.value.multiply(text.glyphs[0].matrix);
    let shown = device.matrix.multiply(up);
    assert!(shown.d < 0.0 && shown.a > 0.0, "{shown:?}");
    assert!(shown.b.abs() < 1e-9 && shown.c.abs() < 1e-9, "{shown:?}");
}

#[test]
fn an_empty_show_is_not_read_as_a_block() {
    let mut editor = Editor::open(fixture(
        b"BT /F1 24 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET BT /F1 24 Tf 1 0 0 1 10 30 Tm () Tj ET",
    ))
    .unwrap();
    read(&mut editor);
    let leaf = editor.leaf(0).unwrap();
    let shows = leaf
        .view
        .graph
        .atoms
        .iter()
        .filter(|atom| matches!(atom.kind, pdf_paint::PaintAtomKind::Text(_)))
        .count();
    assert_eq!(shows, 2, "the page holds two shows, one of them empty");
    assert_eq!(
        leaf.overlay.blocks.len(),
        1,
        "only the show with glyphs in it is a block"
    );
}

#[test]
fn text_placed_in_a_drawn_frame_is_written_in_the_style_that_was_chosen() {
    let mut editor = Editor::open(fixture(b"BT /F1 24 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET")).unwrap();
    read(&mut editor);
    let style = crate::NewTextStyle {
        family: "Helvetica".to_owned(),
        size: 18.0,
        bold: true,
        italic: false,
        fill: Some([1.0, 0.0, 0.0]),
        paragraph: pdf_edit::ParagraphLayout::default(),
    };
    let applied = editor.place_text(0, [20.0, 40.0, 200.0, 80.0], "Hi", &style);
    assert!(
        matches!(applied, Applied::Changed { .. }),
        "the text goes on the page: {applied:?}"
    );
    let view = pdf_session::interpret_page_grouped(
        editor.source().unwrap(),
        0,
        b"",
        editor.grouping(0).as_deref(),
    )
    .unwrap();
    editor.adopt_page(0, Arc::new(view));
    let leaf = editor.leaf(0).unwrap();
    let written = leaf
        .overlay
        .runs
        .iter()
        .find(|run| run.fill == Some([1.0, 0.0, 0.0]))
        .expect("a run in the colour that was chosen");
    assert!(
        (written.em.abs() - 18.0).abs() < 0.01,
        "and at the size that was chosen, not the default: {}",
        written.em
    );
}

fn live_against_committed(
    mut editor: Editor,
    block: usize,
    range: pdf_edit::BlockRange,
    text: &str,
) -> (pdf_edit::LiveBlock, f32) {
    let before = editor.leaf(0).expect("the page is read").view.clone();
    let live = editor
        .live_block(0, block, range, text)
        .expect("the block lays out live");
    let options = pdf_render::RenderOptions {
        scale: 2.0,
        ..pdf_render::RenderOptions::default()
    };
    let device = pdf_render::DeviceTransform::for_page(
        &before.program.geometry,
        options.scale,
        options.limits,
    )
    .unwrap();
    let whole = [0, 0, device.width, device.height];
    let typed = pdf_paint::PaintGraph {
        atoms: live.atoms.clone(),
        ..pdf_paint::PaintGraph::default()
    };
    let (shown, _) = pdf_render::render_region_replacing(
        (&before.graph, &live.hidden, &typed),
        &[],
        &before.program.geometry,
        options,
        whole,
    )
    .unwrap();
    let applied = editor.edit(0, block, range, text);
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    read(&mut editor);
    let after = editor.leaf(0).expect("the page is read again").view.clone();
    let (committed, _) =
        pdf_render::render_page(&after.graph, &after.program.geometry, options).unwrap();
    assert_eq!(
        (shown.width, shown.height),
        (committed.width, committed.height)
    );
    let worst = shown
        .pixels
        .iter()
        .zip(&committed.pixels)
        .flat_map(|(one, other)| (0..3).map(move |channel| (one[channel] - other[channel]).abs()))
        .fold(0.0_f32, f32::max);
    (live, worst)
}

#[test]
fn a_block_drawn_live_is_the_block_the_edit_writes() {
    for (text, lines) in [("AB", 2), (" ABABAB", 3)] {
        let (editor, block) = declared_block(b"BT /F1 10 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET");
        let (live, worst) = live_against_committed(
            editor,
            block,
            pdf_edit::BlockRange::Between {
                from: (0, 4),
                to: (0, 4),
            },
            text,
        );
        assert_eq!(live.lines.len(), lines, "{text:?}: {:?}", live.lines);
        assert!(
            worst == 0.0,
            "{text:?}: the live page differs from the committed one by {worst}"
        );
    }
}

#[test]
fn a_block_in_a_face_the_file_does_not_carry_is_drawn_live_as_written() {
    let mut editor = Editor::open(fixture(b"BT /F1 24 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET")).unwrap();
    read(&mut editor);
    let block = top_block(&editor.leaf(0).unwrap().view);
    let (live, worst) = live_against_committed(
        editor,
        block,
        pdf_edit::BlockRange::Between {
            from: (0, 4),
            to: (0, 4),
        },
        "AA",
    );
    assert!(!live.atoms.is_empty());
    assert!(
        worst == 0.0,
        "the live page differs from the committed one by {worst}"
    );
}

const TWO_PIXELS: &[u8] = b"\x89\x50\x4e\x47\x0d\x0a\x1a\x0a\x00\x00\x00\x0d\x49\x48\x44\x52\x00\x00\x00\x02\x00\x00\x00\x01\x08\x02\x00\x00\x00\x7b\x40\xe8\xdd\x00\x00\x00\x0d\x49\x44\x41\x54\x78\x9c\x63\xf8\xcf\x00\x04\xff\x01\x07\x00\x01\xff\xe2\x23\x9e\x59\x00\x00\x00\x00\x49\x45\x4e\x44\xae\x42\x60\x82";

#[test]
fn pictures_put_down_together_are_one_step() {
    let images = |editor: &Editor| {
        let view =
            pdf_session::interpret_page_for_display(editor.source().unwrap(), 0, b"", None, None)
                .unwrap();
        view.graph
            .atoms
            .iter()
            .filter(|atom| matches!(atom.kind, pdf_paint::PaintAtomKind::Image(_)))
            .count()
    };
    let mut editor = editor();
    let file: Arc<[u8]> = Arc::from(TWO_PIXELS);
    let pictures = vec![
        ([10.0, 10.0, 30.0, 20.0], Arc::clone(&file)),
        ([40.0, 10.0, 60.0, 20.0], Arc::clone(&file)),
        ([70.0, 10.0, 90.0, 20.0], file),
    ];
    assert!(matches!(
        editor.place_images(0, pictures),
        Applied::Changed { page: 0, .. }
    ));
    assert_eq!(images(&editor), 3);
    editor.undo();
    assert_eq!(images(&editor), 0, "one undo takes all three back");
}

#[test]
fn a_size_set_on_part_of_a_typed_block_leaves_it_one_block() {
    let mut editor = Editor::open(fixture(b"BT /F1 24 Tf 1 0 0 1 10 180 Tm (AAAA) Tj ET")).unwrap();
    read(&mut editor);
    let style = crate::NewTextStyle {
        family: "Helvetica".to_owned(),
        size: 14.0,
        bold: false,
        italic: false,
        fill: None,
        paragraph: pdf_edit::ParagraphLayout::default(),
    };
    let applied = editor.place_text(
        0,
        [20.0, 20.0, 120.0, 160.0],
        &vec!["AAAA"; 20].join(" "),
        &style,
    );
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    let view = pdf_session::interpret_page_grouped(
        editor.source().unwrap(),
        0,
        b"",
        editor.grouping(0).as_deref(),
    )
    .unwrap();
    editor.adopt_page(0, Arc::new(view));
    let blocks = |editor: &Editor| editor.leaf(0).unwrap().overlay.blocks.len();
    assert_eq!(blocks(&editor), 2, "the old block and the typed one");
    let typed = editor
        .leaf(0)
        .unwrap()
        .overlay
        .blocks
        .iter()
        .position(|block| block.lines.len() > 2)
        .unwrap();
    let rows = editor.leaf(0).unwrap().overlay.blocks[typed].lines.len();
    let job = editor
        .begin_style(
            0,
            typed,
            pdf_edit::BlockRange::Between {
                from: (rows / 2, 0),
                to: (rows - 1, 1),
            },
            pdf_edit::TextStyle {
                size: Some(9.0),
                ..pdf_edit::TextStyle::default()
            },
        )
        .unwrap();
    let applied = editor.adopt(job.run());
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    read(&mut editor);
    assert_eq!(blocks(&editor), 2, "setting a size on part of it split it");
}

#[test]
fn a_typed_block_moved_onto_another_never_adopts_its_text() {
    let mut editor = Editor::open(fixture(b"BT /F1 24 Tf 1 0 0 1 10 180 Tm (A) Tj ET")).unwrap();
    read(&mut editor);
    let style = crate::NewTextStyle {
        family: "Helvetica".to_owned(),
        size: 24.0,
        bold: false,
        italic: false,
        fill: None,
        paragraph: pdf_edit::ParagraphLayout::default(),
    };
    let place = |editor: &mut Editor, frame: [f64; 4]| {
        let applied = editor.place_text(0, frame, "AA AA", &style);
        assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
        let view = pdf_session::interpret_page_grouped(
            editor.source().unwrap(),
            0,
            b"",
            editor.grouping(0).as_deref(),
        )
        .unwrap();
        editor.adopt_page(0, Arc::new(view));
    };
    place(&mut editor, [10.0, 100.0, 190.0, 140.0]);
    place(&mut editor, [10.0, 20.0, 190.0, 60.0]);
    let clusters = |editor: &Editor| -> Vec<usize> {
        let leaf = editor.leaf(0).unwrap();
        leaf.overlay
            .blocks
            .iter()
            .map(|block| {
                block
                    .lines
                    .iter()
                    .map(|line| leaf.view.index.lines[*line].clusters.len())
                    .sum()
            })
            .collect()
    };
    assert_eq!(
        clusters(&editor),
        vec![1, 5, 5],
        "the opened block and the two typed ones"
    );
    let anchors = editor.leaf(0).unwrap().overlay.blocks[2].anchors.clone();
    assert!(matches!(
        editor.move_block(0, &anchors, 0.0, 56.0),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    assert_eq!(
        clusters(&editor),
        vec![1, 5, 5],
        "a block adopted the other's text"
    );
}

#[test]
fn typing_takes_the_font_size_and_colour_of_the_text_it_follows() {
    let content = b"BT /F1 24 Tf 1 0 0 rg 1 0 0 1 10 150 Tm (AAAA) Tj ET";
    let fixture = || {
        Editor::open(spaced_fixture_with(
            content,
            "[600 600 600]",
            "/Ascent 800 /Descent -200 ",
        ))
        .unwrap()
    };
    let mut editor = fixture();
    read(&mut editor);
    let block = top_block(&editor.leaf(0).unwrap().view);
    let line = editor.leaf(0).unwrap().view.index.blocks[block].lines[0];
    assert!(matches!(
        editor.type_text(0, line, 4, 4, "B"),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    let painted = painted_runs(&editor.leaf(0).unwrap().view);
    assert_eq!(painted.len(), 2, "{painted:?}");
    assert_eq!(painted[0].1, painted[1].1, "the same size: {painted:?}");
    assert_eq!(painted[0].2, painted[1].2, "the same colour: {painted:?}");
    assert_eq!(painted[0].3, painted[1].3, "the same font: {painted:?}");

    let mut editor = fixture();
    read(&mut editor);
    assert!(matches!(
        editor.type_text(0, line, 4, 4, "ก"),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    let painted = painted_runs(&editor.leaf(0).unwrap().view);
    assert_eq!(painted.len(), 2, "{painted:?}");
    assert_eq!(painted[0].1, painted[1].1, "the same size: {painted:?}");
    assert_eq!(painted[0].2, painted[1].2, "the same colour: {painted:?}");
    assert_ne!(
        painted[0].3, painted[1].3,
        "a font with no Thai cannot have drawn it: {painted:?}"
    );
    assert!(
        painted[1].3.contains("Thai"),
        "a face that has Thai: {painted:?}"
    );
}

fn painted_runs(
    view: &pdf_session::PageView,
) -> Vec<(Vec<u32>, f64, Option<pdf_paint::Color>, String)> {
    view.graph
        .atoms
        .iter()
        .filter_map(|atom| match &atom.kind {
            pdf_paint::PaintAtomKind::Text(text) => Some((
                text.glyphs.iter().map(|glyph| glyph.code.value).collect(),
                text.state.text.font_size.value,
                Some(text.state.fill_color.value.clone()),
                text.font_request
                    .as_ref()
                    .map(|request| request.base_font.clone())
                    .map(|name| String::from_utf8_lossy(&name).into_owned())
                    .unwrap_or_default(),
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn the_status_line_names_the_font_a_typed_letter_needed() {
    let content = b"BT /F1 24 Tf 1 0 0 rg 1 0 0 1 10 150 Tm (AAAA) Tj ET";
    let mut editor = Editor::open(spaced_fixture_with(
        content,
        "[600 600 600]",
        "/Ascent 800 /Descent -200 ",
    ))
    .unwrap();
    read(&mut editor);
    let block = top_block(&editor.leaf(0).unwrap().view);
    let line = editor.leaf(0).unwrap().view.index.blocks[block].lines[0];
    assert!(matches!(
        editor.type_text(0, line, 4, 4, "ก"),
        Applied::Changed { .. }
    ));
    let said = editor.status().say(crate::wording::Lang::English);
    assert!(said.contains("Noto Sans Thai"), "{said}");
    assert!(said.contains("size and colour"), "{said}");
}

#[test]
fn whole_block_spans_the_last_stop() {
    let mut editor = Editor::open(fixture(b"BT /F1 24 Tf 1 0 0 1 10 150 Tm (AAAA) Tj ET")).unwrap();
    read(&mut editor);
    let block = top_block(&editor.leaf(0).unwrap().view);
    let range = editor.whole_block(0, block).expect("the block reads");
    let pdf_edit::BlockRange::Between { from, to } = range else {
        panic!("the whole of a block is a range between two stops: {range:?}");
    };
    assert_eq!(from, (0, 0));
    assert_eq!(to, (0, 4));
    assert_eq!(
        editor.copy_text(0, block, from, to).as_deref(),
        Some("AAAA")
    );
    assert_eq!(editor.whole_block(0, 99), None);
}

#[test]
fn a_frame_drawn_taller_than_its_text_keeps_its_height_when_text_is_committed() {
    let mut editor = Editor::open(reflow_source()).unwrap();
    read(&mut editor);
    let view = editor.leaf(0).unwrap().view.clone();
    let block = top_block(&view);
    let line = view.index.blocks[block].lines[0];
    let inferred = editor.frame_boxes(0)[block];
    let drawn = [inferred[0], inferred[1], inferred[2], inferred[3] + 30.0];
    declare_frame(&mut editor, 0, block, drawn);
    let typed = editor.type_text(0, line, 1, 1, "A");
    assert!(
        matches!(typed, Applied::Changed { .. }),
        "{typed:?} \u{2014} {}",
        editor.status()
    );
    read(&mut editor);
    let after = editor.frame_boxes(0)[block];
    assert_eq!(
        after, drawn,
        "the box the person drew is the box the block keeps: {drawn:?} -> {after:?}"
    );

    let mut plain = Editor::open(reflow_source()).unwrap();
    read(&mut plain);
    let line = plain.leaf(0).unwrap().view.index.blocks[block].lines[0];
    plain.preview_frame(0, block, drawn);
    assert_eq!(plain.frame_boxes(0)[block], drawn);
    assert!(!plain.frame_is_declared(0, block));
    assert!(matches!(
        plain.type_text(0, line, 1, 1, "A"),
        Applied::Changed { .. }
    ));
    read(&mut plain);
    let after = plain.frame_boxes(0)[block];
    assert!(
        after[3] < drawn[3] - 1e-6,
        "a frame nobody declared is still its text's: {drawn:?} -> {after:?}"
    );
}

#[test]
fn a_block_copied_and_pasted_stands_beside_itself() {
    let mut editor = Editor::open(reflow_source()).unwrap();
    read(&mut editor);
    let block = top_block(&editor.leaf(0).unwrap().view);
    let anchors = editor.leaf(0).unwrap().overlay.blocks[block]
        .anchors
        .clone();
    let before = pens(editor.source().unwrap());
    let fonts = |source: &ByteStore| {
        source
            .as_bytes()
            .windows(b"/Type /Font".len())
            .filter(|window| *window == b"/Type /Font")
            .count()
    };
    let fonts_before = fonts(editor.source().unwrap());
    let copied = editor
        .copy_objects(0, &anchors)
        .expect("the block is copied");
    let applied = editor.paste_objects(0, copied, (12.0, 20.0));
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    let after = pens(editor.source().unwrap());
    assert_eq!(
        after.len(),
        before.len() + 9,
        "the nine glyphs of the block are painted again: {after:?}"
    );
    assert_eq!(
        fonts(editor.source().unwrap()),
        fonts_before,
        "no second font"
    );
    let pasted = &after[before.len()..];
    for (one, other) in before.iter().take(9).zip(pasted) {
        assert_eq!(one.0, other.0, "the same code: {pasted:?}");
        assert!(
            (one.1 + 12.0 - other.1).abs() < 1e-6 && (one.2 - 20.0 - other.2).abs() < 1e-6,
            "{pasted:?}"
        );
    }
}

#[test]
fn a_paste_is_one_step_and_undoes_whole() {
    let mut editor = Editor::open(reflow_source()).unwrap();
    read(&mut editor);
    let block = top_block(&editor.leaf(0).unwrap().view);
    let anchors = editor.leaf(0).unwrap().overlay.blocks[block]
        .anchors
        .clone();
    let before = pens(editor.source().unwrap());
    let copied = editor
        .copy_objects(0, &anchors)
        .expect("the block is copied");
    assert!(matches!(
        editor.paste_objects(0, copied, (12.0, 20.0)),
        Applied::Changed { .. }
    ));
    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    assert_pens(&pens(editor.source().unwrap()), &before);
    assert!(!editor.can_undo(), "one step, and it is gone");
}

#[test]
fn a_block_and_a_picture_paste_together_and_undo_together() {
    let mut editor = Editor::open(reflow_source()).unwrap();
    read(&mut editor);
    let picture: Arc<[u8]> = include_bytes!("../../pdf-edit/tests/data/picture-rgb.jpg")
        .as_slice()
        .into();
    assert!(matches!(
        editor.place_images(0, vec![([100.0, 20.0, 140.0, 50.0], picture)]),
        Applied::Changed { .. }
    ));
    read(&mut editor);
    let leaf = editor.leaf(0).unwrap();
    assert_eq!(leaf.overlay.objects.len(), 1, "the picture is on the page");
    let block = top_block(&leaf.view);
    let mut anchors = leaf.overlay.blocks[block].anchors.clone();
    anchors.push(leaf.overlay.objects[0].anchor.clone());
    let pens_before = pens(editor.source().unwrap());
    let copied = editor
        .copy_objects(0, &anchors)
        .expect("the group is copied");
    assert_eq!(copied.objects.len(), anchors.len());
    assert!(matches!(
        editor.paste_objects(0, copied, (12.0, 20.0)),
        Applied::Changed { .. }
    ));
    let read_pasted = |editor: &mut Editor| {
        let view = pdf_session::interpret_page_grouped(
            editor.source().unwrap(),
            0,
            b"",
            editor.grouping(0).as_deref(),
        )
        .unwrap();
        editor.adopt_page(0, Arc::new(view));
    };
    read_pasted(&mut editor);
    let leaf = editor.leaf(0).unwrap();
    assert_eq!(leaf.overlay.objects.len(), 2, "the picture is pasted");
    assert_eq!(
        pens(editor.source().unwrap()).len(),
        pens_before.len() + 9,
        "so is the block"
    );
    let images = |source: &ByteStore| {
        source
            .as_bytes()
            .windows(b"/Subtype /Image".len())
            .filter(|window| *window == b"/Subtype /Image")
            .count()
    };
    assert_eq!(
        images(editor.source().unwrap()),
        1,
        "one image object, invoked twice"
    );
    assert!(matches!(editor.undo(), Applied::Changed { .. }));
    read(&mut editor);
    assert_eq!(
        editor.leaf(0).unwrap().overlay.objects.len(),
        1,
        "one undo took the picture's copy"
    );
    assert_pens(&pens(editor.source().unwrap()), &pens_before);
}
