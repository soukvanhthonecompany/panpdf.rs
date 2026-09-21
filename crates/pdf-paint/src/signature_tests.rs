use crate::interpreter::{PaintStream, interpret_stream_sequence_with_resources};
use crate::state::PaintLimits;
use pdf_bytes::{ByteStore, SourceId};
use pdf_content::{
    ContentLimits, PageContentLimits, load_page_program_strict, parse_operations_strict,
};
use std::sync::Arc;

#[test]
fn shading_pattern_signature_ignores_revision_but_detects_gradient_changes() {
    let source = crate::test_fixtures::shading_pattern_fixture(b"", false);
    let other = ByteStore::new(SourceId::new(999), Arc::<[u8]>::from(source.as_bytes()));
    let graph = crate::test_fixtures::interpret_fixture(&source).expect("pattern");
    let other_graph = crate::test_fixtures::interpret_fixture(&other).expect("same pattern");
    let signature = crate::paint_signature(&graph.atoms[0].kind);
    assert_eq!(
        signature,
        crate::paint_signature(&other_graph.atoms[0].kind)
    );
    let mut changed = graph.atoms[0].kind.clone();
    let crate::PaintAtomKind::Path(path) = &mut changed else {
        panic!("path")
    };
    let crate::Color::ShadingPattern(pattern) = &mut path.state.fill_color.value else {
        panic!("shading pattern")
    };
    let crate::function::Function::Exponential(function) =
        &mut Arc::make_mut(pattern).shading.function
    else {
        panic!("function")
    };
    function.c1.value[0] = 0.5;
    assert_ne!(signature, crate::paint_signature(&changed));
}

#[test]
fn a_signature_says_nothing_about_which_revision_the_bytes_came_from() {
    let fixture = |id: u64| -> ByteStore {
        let content = b"q 40 0 0 30 10 10 cm /Im1 Do Q";
        let samples: [u8; 4] = [0, 1, 1, 0];
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Im1 5 0 R >> >> >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
        bytes.extend_from_slice(content.len().to_string().as_bytes());
        bytes.extend_from_slice(b" >>\nstream\n");
        bytes.extend_from_slice(content);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"5 0 obj\n<< /Type /XObject /Subtype /Image /Width 2 /Height 2 /BitsPerComponent 8 /ColorSpace [/Indexed /DeviceRGB 1 <000000 FFFFFF>] /Length 4 >>\nstream\n");
        bytes.extend_from_slice(&samples);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        let xref = bytes.len();
        bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(b"trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n");
        bytes.extend_from_slice(xref.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        ByteStore::new(SourceId::new(id), Arc::<[u8]>::from(bytes))
    };
    let signature_of = |id: u64| -> String {
        let source = fixture(id);
        let page = load_page_program_strict(&source, 0, PageContentLimits::default())
            .expect("a page with an indexed image");
        let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
            .expect("its operations");
        let graph = interpret_stream_sequence_with_resources(
            &[PaintStream {
                source: &page.streams[0].bytes,
                reference: page.streams[0].reference,
                operations: &operations,
            }],
            page.page,
            &[],
            &page.resources,
            PaintLimits::default(),
        )
        .expect("it interprets");
        crate::paint_signature(&graph.atoms[0].kind)
    };
    assert_eq!(signature_of(1), signature_of(2));
    assert!(signature_of(1).contains("palette="), "{}", signature_of(1));
}
