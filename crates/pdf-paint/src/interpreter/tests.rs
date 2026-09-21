use crate::graph::PaintAtomKind;
use crate::interpreter::Interpreter;
use crate::state::PaintLimits;
use crate::test_fixtures::{text_state_fixture, type0_text_fixture};
use pdf_content::{
    ContentLimits, PageContentLimits, load_page_program_strict, parse_operations_strict,
};

#[test]
fn text_state_outside_bt_and_line_matrices_follow_pdf_semantics() {
    let source = text_state_fixture();
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("page with font resource");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("text-state operations");
    let et = operations.last().unwrap();
    let mut interpreter = Interpreter::new(
        &page.streams[0].bytes,
        page.page,
        page.streams[0].reference,
        &[],
        Some(&page.resources),
        PaintLimits::default(),
        None,
    );
    for operation in &operations[..operations.len() - 1] {
        interpreter.apply(operation).expect("supported text state");
    }
    let state = &interpreter.state.text;
    assert!((state.character_spacing.value - 1.0).abs() < f64::EPSILON);
    assert!((state.word_spacing.value - 2.0).abs() < f64::EPSILON);
    assert!((state.horizontal_scaling.value - 80.0).abs() < f64::EPSILON);
    assert!((state.leading.value - 6.0).abs() < f64::EPSILON);
    assert!((state.font_size.value - 12.0).abs() < f64::EPSILON);
    assert_eq!(
        state.rendering_mode.value,
        crate::TextRenderingMode::FillStroke
    );
    assert!((state.rise.value - 3.0).abs() < f64::EPSILON);
    assert_eq!(state.font.as_ref().unwrap().value.name, b"/F1");
    assert_eq!(state.font.as_ref().unwrap().provenance.len(), 2);
    let matrices = interpreter.text_matrices.as_ref().unwrap();
    assert_eq!(matrices.text.provenance.len(), 4);
    assert_eq!(interpreter.graph.atoms.len(), 1);
    let PaintAtomKind::Text(text_show) = &interpreter.graph.atoms[0].kind else {
        panic!("expected text-show atom")
    };
    assert_eq!(text_show.elements.len(), 3);
    assert!((text_show.matrices.text.value.e - 15.0).abs() < f64::EPSILON);
    assert_eq!(text_show.matrices.text.provenance.len(), 3);
    let crate::TextShowElement::Codes { codes, .. } = &text_show.elements[0] else {
        panic!("expected source codes")
    };
    assert_eq!(
        codes.iter().map(|code| code.value).collect::<Vec<_>>(),
        vec![32, 34]
    );
    assert_eq!(
        codes.iter().map(|code| code.width).collect::<Vec<_>>(),
        vec![250.0, 750.0]
    );
    assert!((matrices.text.value.e - 32.44).abs() < 1.0e-10);
    interpreter.apply(et).expect("ET closes the object");
    assert!(interpreter.text_matrices.is_none());
}

#[test]
fn type0_cmap_preserves_codes_cids_metrics_and_mapping_provenance() {
    let source = type0_text_fixture();
    let page = load_page_program_strict(&source, 0, PageContentLimits::default())
        .expect("page with Type0 font");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("text operations");
    let mut interpreter = Interpreter::new(
        &page.streams[0].bytes,
        page.page,
        page.streams[0].reference,
        &[],
        Some(&page.resources),
        PaintLimits::default(),
        None,
    );
    for operation in &operations[..operations.len() - 1] {
        interpreter.apply(operation).expect("Type0 text");
    }
    let PaintAtomKind::Text(text) = &interpreter.graph.atoms[0].kind else {
        panic!("expected text atom")
    };
    let crate::TextShowElement::Codes { codes, .. } = &text.elements[0] else {
        panic!("expected codes")
    };
    assert_eq!(
        codes
            .iter()
            .map(|code| code.bytes.clone())
            .collect::<Vec<_>>(),
        vec![vec![0x20], vec![0x21], vec![0x81, 0x02]]
    );
    assert_eq!(
        codes.iter().map(|code| code.cid).collect::<Vec<_>>(),
        vec![Some(3), Some(0), Some(42)]
    );
    assert_eq!(
        codes.iter().map(|code| code.width).collect::<Vec<_>>(),
        vec![250.0, 900.0, 700.0]
    );
    assert!(codes[0].mapping_span.is_some());
    assert!(codes[1].mapping_span.is_none());
    assert!(codes[2].mapping_span.is_some());
    assert!((interpreter.text_matrices.as_ref().unwrap().text.value.e - 21.5).abs() < 1.0e-10);
}
