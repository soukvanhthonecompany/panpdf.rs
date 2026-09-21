use crate::Session;
use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::{Command, FixedPoint, ObjectSelection, SourceAnchor};
use pdf_paint::{Matrix, PaintAtomKind};
use std::sync::Arc;
fn source(second: &str) -> ByteStore {
    source_with_kids(second, "3 0 R 4 0 R")
}

fn source_with_kids(second: &str, kids: &str) -> ByteStore {
    let content = b"q 20 0 0 20 10 10 cm /Im Do Q BT /F1 10 Tf 1 0 0 1 10 80 Tm (A) Tj ET";
    document_with_content(second, kids, content)
}

fn document_with_content(second: &str, kids: &str, content: &[u8]) -> ByteStore {
    let mut stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
    stream.extend(content);
    stream.extend(b"\nendstream");
    let mut picture = b"<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length 3 >>\nstream\n".to_vec();
    picture.extend([255, 0, 0]);
    picture.extend(b"\nendstream");
    let page = b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 5 0 R /Resources << /XObject << /Im 6 0 R >> /Font << /F1 9 0 R >> >> >>".to_vec();
    let other = format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] {second} /Resources << /XObject << /Im 6 0 R >> /Font << /F1 9 0 R >> >> >>").into_bytes();
    let objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [{kids}] /Count 2 >>").into_bytes(),
        page,
        other,
        stream.clone(),
        picture,
        stream,
        b"[5 0 R]".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_vec(),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = vec![];
    for (i, body) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend(format!("{} 0 obj\n", i + 1).bytes());
        bytes.extend(body);
        bytes.extend(b"\nendobj\n");
    }
    let xref = bytes.len();
    bytes.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
    for offset in offsets {
        bytes.extend(format!("{offset:010} 00000 n \n").bytes());
    }
    bytes.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .bytes(),
    );
    ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes))
}
fn location(session: &mut Session, page: usize) -> f64 {
    let view = session.page(page).unwrap();
    match &view.graph.atoms[0].kind {
        PaintAtomKind::Image(image) => image.state.ctm.value.e,
        _ => panic!(),
    }
}

#[test]
fn shared_content_is_copied_for_the_edited_page_alone() {
    for second in ["/Contents 5 0 R", "/Contents [5 0 R]", "/Contents 8 0 R"] {
        let original = source(second);
        let mut session = Session::new(original.clone(), b"");
        let first = session.page(0).unwrap();
        let anchor = SourceAnchor::of(&first.graph.atoms[0].id);
        let plan = session
            .plan(&Command::PlaceObject {
                page_index: 0,
                target: ObjectSelection::Painted(anchor),
                transform: Matrix {
                    e: 12.0,
                    ..Matrix::IDENTITY
                },
                about: FixedPoint::Origin,
            })
            .unwrap_or_else(|error| panic!("{second}: {error:?}"));
        session.apply(plan).unwrap();
        assert!((location(&mut session, 0) - 22.0).abs() < 1e-9, "{second}");
        let mut reopened = Session::new(session.source().clone(), b"");
        assert!((location(&mut reopened, 0) - 22.0).abs() < 1e-9, "{second}");
        assert!((location(&mut reopened, 1) - 10.0).abs() < 1e-9, "{second}");
        assert!(session.source().as_bytes().starts_with(original.as_bytes()));
        assert!(session.undo().unwrap());
        assert!((location(&mut session, 0) - 10.0).abs() < 1e-9, "{second}");
        let mut undone = Session::new(session.source().clone(), b"");
        assert!((location(&mut undone, 1) - 10.0).abs() < 1e-9, "{second}");
    }
}

#[test]
fn distinct_streams_may_share_image_samples_without_sharing_placement() {
    let mut session = Session::new(source("/Contents 7 0 R"), b"");
    let first = session.page(0).unwrap();
    let second = session.page(1).unwrap();
    assert!((location(&mut session, 0) - 10.0).abs() < 1e-9);
    assert!((location(&mut session, 1) - 10.0).abs() < 1e-9);
    let anchor = SourceAnchor::of(&first.graph.atoms[0].id);
    let plan = session
        .plan(&Command::PlaceObject {
            page_index: 0,
            target: ObjectSelection::Painted(anchor),
            transform: Matrix {
                e: 12.0,
                ..Matrix::IDENTITY
            },
            about: FixedPoint::Origin,
        })
        .unwrap();
    session.apply(plan).unwrap();
    assert!((location(&mut session, 0) - 22.0).abs() < 1e-9);
    assert!(Arc::ptr_eq(&second, &session.page(1).unwrap()));
    let mut reopened = Session::new(session.source().clone(), b"");
    assert!((location(&mut reopened, 1) - 10.0).abs() < 1e-9);
    assert!(session.undo().unwrap());
    assert!((location(&mut session, 0) - 10.0).abs() < 1e-9);
    assert!(session.redo().unwrap());
    assert!((location(&mut session, 0) - 22.0).abs() < 1e-9);
}

#[test]
fn ownership_errors_on_other_pages_do_not_silently_grant_edit_permission() {
    for second in ["/Contents 999 0 R", "/Contents [8 0 R]", "/Contents true"] {
        let mut session = Session::new(source(second), b"");
        let first = session.page(0).unwrap();
        let anchor = SourceAnchor::of(&first.graph.atoms[0].id);
        assert!(
            session
                .plan(&Command::PlaceObject {
                    page_index: 0,
                    target: ObjectSelection::Painted(anchor),
                    transform: Matrix {
                        e: 12.0,
                        ..Matrix::IDENTITY
                    },
                    about: FixedPoint::Origin
                })
                .is_err()
        );
        assert!(!session.undo().unwrap());
    }
}

#[test]
fn text_commands_copy_a_shared_page_stream_too() {
    for (second, shared) in [("/Contents 7 0 R", false), ("/Contents 5 0 R", true)] {
        let mut session = Session::new(source(second), b"");
        let result = session.plan(&Command::MoveTextRun {
            page_index: 0,
            selection: pdf_edit::TextRunSelection::Last,
            dx: 12.0,
            dy: 0.0,
        });
        session
            .apply(result.unwrap_or_else(|error| panic!("{second}: {error:?}")))
            .unwrap();
        let mut reopened = Session::new(session.source().clone(), b"");
        let text_x = |session: &mut Session, page| {
            let view = session.page(page).unwrap();
            view.graph
                .atoms
                .iter()
                .find_map(|atom| match &atom.kind {
                    PaintAtomKind::Text(text) => Some(text.matrices.text.value.e),
                    _ => None,
                })
                .unwrap()
        };
        assert!(
            (text_x(&mut reopened, 0) - 22.0).abs() < 1e-9,
            "{second} {shared}"
        );
        assert!(
            (text_x(&mut reopened, 1) - 10.0).abs() < 1e-9,
            "{second} {shared}"
        );
    }
}

#[test]
fn ownership_census_honours_limits_and_rejects_cycles_beyond_the_selected_page() {
    use pdf_content::{PageContentErrorKind, PageContentLimits, load_page_program_strict};
    let limits = PageContentLimits {
        max_page_tree_nodes: 2,
        ..PageContentLimits::default()
    };
    let program = load_page_program_strict(&source("/Contents 7 0 R"), 0, limits).unwrap();
    assert_eq!(
        program
            .content_stream_is_exclusive(program.streams[0].reference)
            .unwrap_err()
            .kind(),
        PageContentErrorKind::PageTreeLimit
    );
    let document = source_with_kids("/Contents 7 0 R", "3 0 R 4 0 R 2 0 R");
    let program = load_page_program_strict(&document, 0, PageContentLimits::default()).unwrap();
    assert_eq!(
        program
            .content_stream_is_exclusive(program.streams[0].reference)
            .unwrap_err()
            .kind(),
        PageContentErrorKind::PageTreeCycle
    );
}

#[test]
fn cropped_placement_is_one_history_step_and_never_edits_a_shared_page_stream() {
    let content = b"q 10 10 10 20 re W n 20 0 0 20 10 10 cm /Im Do Q";
    for (second, shared) in [("/Contents 7 0 R", false), ("/Contents 5 0 R", true)] {
        let source = document_with_content(second, "3 0 R 4 0 R", content);
        let mut session = Session::new(source.clone(), b"");
        let first = session.page(0).unwrap();
        let plan = session.plan(&Command::PlaceObject {
            page_index: 0,
            target: ObjectSelection::Painted(SourceAnchor::of(&first.graph.atoms[0].id)),
            transform: Matrix {
                e: 12.0,
                ..Matrix::IDENTITY
            },
            about: FixedPoint::Origin,
        });
        let _ = shared;
        session.apply(plan.unwrap()).unwrap();
        let clip_x = |session: &mut Session, page| {
            let view = session.page(page).unwrap();
            let PaintAtomKind::Image(image) = &view.graph.atoms[0].kind else {
                panic!()
            };
            image.state.clip_paths[0].ctm.value.e
        };
        assert!((clip_x(&mut session, 0) - 12.0).abs() < 1e-9);
        let mut reopened = Session::new(session.source().clone(), b"");
        assert!((clip_x(&mut reopened, 0) - 12.0).abs() < 1e-9);
        assert!(clip_x(&mut reopened, 1).abs() < 1e-9);
        assert!(session.undo().unwrap());
        assert!(clip_x(&mut session, 0).abs() < 1e-9);
        assert!(!session.undo().unwrap());
        assert!(session.redo().unwrap());
        assert!((clip_x(&mut session, 0) - 12.0).abs() < 1e-9);
    }
}
