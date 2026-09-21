use crate::color::Color;
use crate::error::InterpretErrorKind;
use crate::geometry::{FillRule, Matrix, PathSegment, Point};
use crate::graph::PaintAtomKind;
use crate::test_fixtures::{
    BLANK_GLYPH_CFF, DISAGREEING_CFF, content_only_fixture, interpret_fixture, path,
    text_clip_fixture, text_font_fixture, type3_graph, type3_text,
};
use pdf_bytes::ByteStore;

#[test]
fn a_run_that_draws_nothing_is_told_apart_from_one_that_cannot_be_measured() {
    let run_of = |source: &ByteStore| -> crate::TextShowPaint {
        let graph = interpret_fixture(source).expect("the fixture interprets");
        graph
            .atoms
            .iter()
            .find_map(|atom| match &atom.kind {
                PaintAtomKind::Text(text) => Some(text.clone()),
                _ => None,
            })
            .expect("the page paints text")
    };

    let ink = run_of(&text_font_fixture(
        "BT /F1 12 Tf 1 0 0 1 10 20 Tm <41> Tj ET",
        BLANK_GLYPH_CFF,
    ));
    assert!(
        ink.outline_points()
            .is_some_and(|points| !points.is_empty()),
        "`A` is a filled square, so it has an outline"
    );
    assert!(
        !ink.draws_no_ink(),
        "a run that draws must not be called one that does not"
    );

    let space = run_of(&text_font_fixture(
        "BT /F1 12 Tf 1 0 0 1 10 20 Tm <42> Tj ET",
        BLANK_GLYPH_CFF,
    ));
    assert!(
        space.outline_points().is_none(),
        "no contours means no points, which is what makes this confusable"
    );
    assert!(
        space.draws_no_ink(),
        "the program has the glyph and the glyph draws nothing: that is known, not missing"
    );

    let mixed = run_of(&text_font_fixture(
        "BT /F1 12 Tf 1 0 0 1 10 20 Tm <414241> Tj ET",
        BLANK_GLYPH_CFF,
    ));
    assert!(!mixed.draws_no_ink(), "two of its three glyphs draw");
    assert!(
        mixed.draws_no_ink_in(1..2),
        "the middle glyph is the space, and that is known"
    );
    assert!(mixed.outline_bounds_in(1..2).is_none());
    assert!(!mixed.draws_no_ink_in(0..2), "the first glyph draws");
    assert!(
        !mixed.draws_no_ink_in(1..1),
        "an empty range is not a run of spaces"
    );
    assert!(
        !mixed.draws_no_ink_in(0..9),
        "a range past the end is refused rather than clamped"
    );

    let unmeasurable = run_of(&content_only_fixture(
        b"BT /F1 12 Tf 1 0 0 1 10 20 Tm (A) Tj ET",
    ));
    assert!(unmeasurable.outline_points().is_none());
    assert!(
        !unmeasurable.draws_no_ink(),
        "a font this engine cannot read says nothing about what it would draw"
    );

    let graph = type3_graph(
        b"BT /F1 10 Tf 1 0 0 1 20 30 Tm (A) Tj ET",
        b"1000 0 0 1000 0 0 d1 0 0 1 1 re f",
        b"[0.001 0 0 0.001 0 0]",
    )
    .expect("a Type 3 page interprets");
    let drawn = type3_text(&graph);
    assert!(drawn.type3, "the fixture is a Type 3 run");
    assert!(
        drawn
            .outline_points()
            .is_some_and(|points| !points.is_empty()),
        "a Type 3 run is measured through the atoms its procedure painted"
    );
    assert!(
        !drawn.draws_no_ink(),
        "a Type 3 glyph draws with a procedure, and this one fills a square"
    );

    let blank = type3_graph(
        b"BT /F1 10 Tf 1 0 0 1 20 30 Tm (A) Tj ET",
        b"1000 0 d0",
        b"[0.001 0 0 0.001 0 0]",
    )
    .expect("a Type 3 page interprets");
    let empty = type3_text(&blank);
    assert!(
        empty.outline_points().is_none(),
        "a procedure that paints nothing contributes no points"
    );
    assert!(
        empty.draws_no_ink(),
        "a Type 3 procedure that paints nothing is the space case, not the unmeasurable one"
    );
}

#[test]
fn a_clipping_text_mode_adds_its_glyph_outlines_to_the_clip_at_end_of_text() {
    let graph = interpret_fixture(&text_clip_fixture(7, "41")).expect("clipping text mode");
    let text = match &graph.atoms[0].kind {
        PaintAtomKind::Text(text) => text,
        other => panic!("expected a text atom, got {other:?}"),
    };
    assert!(text.state.clip_paths.is_empty());
    assert_eq!(
        text.state.text.rendering_mode.value,
        crate::TextRenderingMode::Clip
    );

    let painted = path(&graph.atoms[1]);
    assert_eq!(
        painted.state.clip_paths.len(),
        1,
        "the clip reached the fill"
    );
    let clip = &painted.state.clip_paths[0];
    assert_eq!(clip.rule, FillRule::Nonzero);
    assert!(clip.path.segments.len() >= 4);
    assert!(matches!(clip.path.segments[0], PathSegment::MoveTo { .. }));
}

#[test]
fn a_glyph_subrange_is_bounded_by_its_own_glyphs_and_not_the_whole_run() {
    let graph = interpret_fixture(&text_clip_fixture(0, "4141")).expect("two glyphs");
    let text = match &graph.atoms[0].kind {
        PaintAtomKind::Text(text) => text,
        other => panic!("expected a text atom, got {other:?}"),
    };
    assert_eq!(text.glyphs.len(), 2);

    let whole = text.outline_bounds().expect("the run has an extent");
    let first = text.outline_bounds_in(0..1).expect("the first glyph");
    let second = text.outline_bounds_in(1..2).expect("the second glyph");

    assert!(first[2] - first[0] < whole[2] - whole[0]);
    assert!(second[0] > first[0]);
    assert!((first[0] - whole[0]).abs() < 1e-9);
    assert!((second[2] - whole[2]).abs() < 1e-9);
}

#[test]
fn a_glyph_range_outside_the_run_is_refused_rather_than_clamped() {
    let graph = interpret_fixture(&text_clip_fixture(0, "4141")).expect("two glyphs");
    let text = match &graph.atoms[0].kind {
        PaintAtomKind::Text(text) => text,
        other => panic!("expected a text atom, got {other:?}"),
    };
    assert!(text.outline_points_in(0..3).is_none());
    let reversed = std::ops::Range { start: 2, end: 1 };
    assert!(text.outline_points_in(reversed).is_none());
    assert!(text.outline_points_in(2..2).is_none());
}

#[test]
fn a_clip_that_cannot_be_outlined_fails_closed() {
    let error = interpret_fixture(&text_clip_fixture(7, "42"))
        .expect_err("a glyph with no outline must not silently shrink the clip");
    assert_eq!(error.kind(), InterpretErrorKind::TextClipWithoutOutlines);
}

#[test]
fn empty_text_shows_do_not_create_or_replace_a_text_clip() {
    for show in ["<> Tj", "[] TJ", "[100 <>] TJ"] {
        let source = text_font_fixture(
            &format!("BT /F1 12 Tf 7 Tr {show} ET 0 0 5 5 re f"),
            DISAGREEING_CFF,
        );
        let graph = interpret_fixture(&source).expect("empty show contributes no clip");
        assert!(
            path(graph.atoms.last().expect("following fill"))
                .state
                .clip_paths
                .is_empty()
        );

        let source = text_font_fixture(
            &format!("BT /F1 12 Tf 7 Tr <41> Tj {show} ET 0 0 5 5 re f"),
            DISAGREEING_CFF,
        );
        let graph = interpret_fixture(&source).expect("existing glyph clip survives");
        let clips = &path(graph.atoms.last().expect("following fill"))
            .state
            .clip_paths;
        assert_eq!(clips.len(), 1);
        assert!(!clips[0].path.segments.is_empty());
    }
}

#[test]
fn type3_glyph_is_placed_by_the_font_matrix() {
    let graph = type3_graph(
        b"BT /F1 10 Tf 1 0 0 1 20 30 Tm (A) Tj ET",
        b"1000 0 0 1000 0 0 d1 0 0 1 1 re f",
        b"[0.001 0 0 0.001 0 0]",
    )
    .expect("a Type 3 page interprets");
    let text = type3_text(&graph);
    assert!(
        text.type3,
        "the run must say its font draws with procedures"
    );
    assert_eq!(text.glyphs.len(), 1);
    let procedure = text.glyphs[0]
        .procedure
        .as_ref()
        .expect("the encoded code selects a procedure");
    assert!(procedure.shape_only, "d1 declares a shape");
    assert_eq!(procedure.name, b"square");
    let PaintAtomKind::Path(paint) = &procedure.atoms[0].kind else {
        panic!("the procedure paints a path");
    };
    let PathSegment::Rectangle { origin, .. } = paint.path.segments[0] else {
        panic!("the procedure paints a rectangle");
    };
    let placed = paint.state.ctm.value.transform(origin);
    assert!((placed.x - 20.0).abs() < 1e-9, "x was {}", placed.x);
    assert!((placed.y - 30.0).abs() < 1e-9, "y was {}", placed.y);
    let far = paint.state.ctm.value.transform(Point { x: 1000.0, y: 0.0 });
    assert!((far.x - 30.0).abs() < 1e-9, "x was {}", far.x);
}

#[test]
fn type3_glyph_bounds_come_from_what_its_procedure_paints() {
    let graph = type3_graph(
        b"BT /F1 10 Tf 1 0 0 1 20 30 Tm (A) Tj ET",
        b"1000 0 0 1000 0 0 d1 0 0 1000 1000 re f",
        b"[0.001 0 0 0.001 0 0]",
    )
    .expect("a Type 3 page interprets");
    let bounds = type3_text(&graph)
        .outline_bounds()
        .expect("the painted CharProc has an extent");
    for (actual, expected) in bounds.into_iter().zip([20.0, 30.0, 30.0, 40.0]) {
        assert!((actual - expected).abs() < 1e-9, "{bounds:?}");
    }
}

#[test]
fn type3_widths_scale_by_the_font_matrix() {
    let advance_at = |font_matrix: &[u8]| {
        let graph = type3_graph(
            b"BT /F1 10 Tf 1 0 0 1 0 0 Tm (AA) Tj ET",
            b"1000 0 0 1000 0 0 d1 0 0 1 1 re f",
            font_matrix,
        )
        .expect("a Type 3 page interprets");
        type3_text(&graph).glyphs[1].text_matrix.e
    };
    let ordinary = advance_at(b"[0.001 0 0 0.001 0 0]");
    let tenfold = advance_at(b"[0.01 0 0 0.01 0 0]");
    assert!((ordinary - 5.0).abs() < 1e-9, "advance was {ordinary}");
    assert!((tenfold - 50.0).abs() < 1e-9, "advance was {tenfold}");
}

#[test]
fn type3_code_without_a_procedure_paints_nothing() {
    let graph = type3_graph(
        b"BT /F1 10 Tf 1 0 0 1 0 0 Tm (AB) Tj ET",
        b"1000 0 0 1000 0 0 d1 0 0 1 1 re f",
        b"[0.001 0 0 0.001 0 0]",
    )
    .expect("a code with no procedure is not an error");
    let text = type3_text(&graph);
    assert!(text.glyphs[0].procedure.is_some());
    assert!(
        text.glyphs[1].procedure.is_none(),
        "/absent has no /CharProcs entry"
    );
    assert!((text.glyphs[1].text_matrix.e - 5.0).abs() < 1e-9);
}

#[test]
fn shape_only_glyph_keeps_the_runs_colour() {
    let graph = type3_graph(
        b"BT 1 0 0 rg /F1 10 Tf 1 0 0 1 0 0 Tm (A) Tj ET",
        b"1000 0 0 1000 0 0 d1 0 1 0 rg 0 0 1 1 re f",
        b"[0.001 0 0 0.001 0 0]",
    )
    .expect("a Type 3 page interprets");
    let procedure = type3_text(&graph).glyphs[0]
        .procedure
        .as_ref()
        .expect("a procedure ran");
    let PaintAtomKind::Path(paint) = &procedure.atoms[0].kind else {
        panic!("the procedure paints a path");
    };
    assert_eq!(
        paint.state.fill_color.value,
        Color::DeviceRgb(1.0, 0.0, 0.0)
    );
}

#[test]
fn coloured_glyph_keeps_its_own_colour() {
    let graph = type3_graph(
        b"BT 1 0 0 rg /F1 10 Tf 1 0 0 1 0 0 Tm (A) Tj ET",
        b"1000 0 d0 0 1 0 rg 0 0 1 1 re f",
        b"[0.001 0 0 0.001 0 0]",
    )
    .expect("a Type 3 page interprets");
    let procedure = type3_text(&graph).glyphs[0]
        .procedure
        .as_ref()
        .expect("a procedure ran");
    assert!(!procedure.shape_only, "d0 declares a coloured glyph");
    let PaintAtomKind::Path(paint) = &procedure.atoms[0].kind else {
        panic!("the procedure paints a path");
    };
    assert_eq!(
        paint.state.fill_color.value,
        Color::DeviceRgb(0.0, 1.0, 0.0)
    );
}

#[test]
fn glyph_metrics_outside_a_procedure_are_refused() {
    for content in [&b"1000 0 d0"[..], &b"1000 0 0 0 1000 1000 d1"[..]] {
        let error = type3_graph(content, b"1000 0 d0", b"[0.001 0 0 0.001 0 0]")
            .expect_err("d0 and d1 belong to a glyph procedure");
        assert_eq!(
            error.kind(),
            InterpretErrorKind::GlyphMetricOutsideProcedure
        );
    }
}

#[test]
fn unbalanced_glyph_procedure_is_closed_where_it_ends() {
    let graph = type3_graph(
        b"BT /F1 10 Tf 1 0 0 1 0 0 Tm (A) Tj ET",
        b"1000 0 0 1000 0 0 d1 q 0 0 1 1 re f",
        b"[0.001 0 0 0.001 0 0]",
    )
    .expect("an unbalanced procedure is drawn, and what was wrong with it is said");
    assert!(
        graph.repairs.iter().any(|repair| matches!(
            repair.kind,
            crate::RepairKind::UnclosedSaveState { depth: 1 }
        )),
        "{:?}",
        graph.repairs
    );
    assert_eq!(graph.atoms.len(), 1, "the glyph is still drawn");
}

#[test]
fn glyph_procedure_does_not_leak_its_state() {
    let graph = type3_graph(
        b"BT /F1 10 Tf 1 0 0 1 0 0 Tm (A) Tj ET 0 0 1 1 re f",
        b"1000 0 d0 0 1 0 rg 2 0 0 2 0 0 cm 0 0 1 1 re f",
        b"[0.001 0 0 0.001 0 0]",
    )
    .expect("a Type 3 page interprets");
    let PaintAtomKind::Path(after) = &graph.atoms[1].kind else {
        panic!("the page paints a path after the text");
    };
    assert_eq!(after.state.fill_color.value, Color::DeviceGray(0.0));
    assert_eq!(after.state.ctm.value, Matrix::IDENTITY);
}

#[test]
fn the_quote_operators_move_a_line_and_show_a_string() {
    let source = text_font_fixture(
        "BT /F1 10 Tf 12 TL 1 0 0 1 10 100 Tm <41> Tj (\u{41}) ' 3 4 (\u{41}) \" ET",
        BLANK_GLYPH_CFF,
    );
    let graph = interpret_fixture(&source).expect("a page using ' and \"");
    let runs: Vec<&crate::TextShowPaint> = graph
        .atoms
        .iter()
        .filter_map(|atom| match &atom.kind {
            PaintAtomKind::Text(text) => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(runs.len(), 3, "Tj, ' and \" each show one run");
    assert!((runs[0].matrices.line.value.f - 100.0).abs() < f64::EPSILON);
    assert!(
        (runs[1].matrices.line.value.f - 88.0).abs() < f64::EPSILON,
        "' drops one leading"
    );
    assert!(
        (runs[2].matrices.line.value.f - 76.0).abs() < f64::EPSILON,
        "\" drops another"
    );
    assert!((runs[1].matrices.text.value.e - 10.0).abs() < f64::EPSILON);
    assert!((runs[0].state.text.word_spacing.value - 0.0).abs() < f64::EPSILON);
    assert!((runs[2].state.text.word_spacing.value - 3.0).abs() < f64::EPSILON);
    assert!((runs[2].state.text.character_spacing.value - 4.0).abs() < f64::EPSILON);
}

#[test]
fn a_quote_operator_with_the_wrong_operands_is_refused() {
    let source = text_font_fixture(
        "BT /F1 10 Tf 12 TL 1 0 0 1 10 100 Tm 3 (\u{41}) \" ET",
        BLANK_GLYPH_CFF,
    );
    let error = interpret_fixture(&source).expect_err("\" with two operands");
    assert_eq!(
        error.kind(),
        InterpretErrorKind::OperandCount {
            expected: 3,
            actual: 2
        }
    );
}

#[test]
fn a_stray_emc_is_recorded_and_the_page_is_still_drawn() {
    let source = content_only_fixture(b"EMC 10 20 30 40 re f");
    let page = pdf_content::load_page_program_strict(
        &source,
        0,
        pdf_content::PageContentLimits::default(),
    )
    .expect("fixture page");
    let graph = interpret_fixture(&source).expect("a page with one stray EMC");
    assert_eq!(graph.atoms.len(), 1, "the rectangle is still painted");
    assert!(graph.atoms[0].marks.is_empty(), "it is inside no section");
    assert_eq!(graph.repairs.len(), 1);
    assert_eq!(
        graph.repairs[0].kind,
        crate::RepairKind::UnmatchedEndMarkedContent
    );
    assert_eq!(
        page.streams[0]
            .bytes
            .resolve(graph.repairs[0].operator_span)
            .expect("the repair's span is in the content stream"),
        b"EMC",
        "the repair names the operator that broke the rule"
    );
}

#[test]
fn balanced_marks_repair_nothing_and_an_unclosed_section_still_fails() {
    let source = content_only_fixture(b"/Tag BMC 10 20 30 40 re f EMC");
    let graph = interpret_fixture(&source).expect("a balanced section");
    assert!(graph.repairs.is_empty());
    assert_eq!(graph.atoms[0].marks.len(), 1);

    let source = content_only_fixture(b"/Tag BMC 10 20 30 40 re f");
    let error = interpret_fixture(&source).expect_err("a section left open");
    assert_eq!(
        error.kind(),
        InterpretErrorKind::UnbalancedMarkedContent { depth: 1 }
    );
}
