use std::sync::Arc;

use pdf_edit::image_file::ImageFile;

use super::{
    AttachError, LONGEST_SIDE, MOST_FILES, MOST_TOTAL_BYTES, fits, looks_like_pdf, prepare,
};
use crate::connect::{Attachment, AttachmentKind};
use crate::desk::{Desk, MOST_CHARACTERS, text_of_page};

fn png(across: u32, down: u32) -> Vec<u8> {
    let pixels = vec![0x40_u8; (across as usize) * (down as usize) * 3];
    pdf_edit::png::write((across, down), &pixels, None).expect("a PNG is written")
}

fn document(folder_name: &str, text: &str) -> Vec<u8> {
    let folder = std::env::temp_dir().join(format!(
        "panpdf-attach-{}-{folder_name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).expect("a folder");
    let path = folder.join("attached.pdf");
    std::fs::write(
        &path,
        pdf_session::blank_document([595.0, 842.0]).expect("a blank page"),
    )
    .expect("written");
    let mut desk = Desk::with_fonts(Some(crate::desk::tests::fonts()));
    let opened = desk.open(&path, "", false).expect("opens");
    desk.place_text(
        &opened.handle,
        0,
        [72.0, 72.0, 500.0, 300.0],
        text,
        ("DejaVu Sans", 12.0, false, false, None),
    )
    .expect("the text is placed");
    desk.save(&opened.handle, &path, true).expect("saved");
    let bytes = std::fs::read(&path).expect("read back");
    let _ = std::fs::remove_dir_all(&folder);
    bytes
}

#[test]
fn a_picture_is_sent_as_it_is_unless_it_is_too_large() {
    let small = png(10, 10);
    let made = prepare("small.png", &small).expect("a picture attaches");
    assert_eq!(made.len(), 1);
    assert_eq!(made[0].bytes, small, "the small picture is untouched");
    assert_eq!(
        made[0].kind,
        AttachmentKind::Image {
            media_type: "image/png".to_owned()
        }
    );

    let wide = png(2_000, 40);
    let made = prepare("wide.png", &wide).expect("a picture attaches");
    assert_eq!(made.len(), 1);
    assert_ne!(made[0].bytes, wide, "a large picture is written again");
    let read = ImageFile::read(&made[0].bytes).expect("a PNG comes back");
    assert_eq!(read.upright(), (LONGEST_SIDE, 31), "{:?}", read.upright());
}

#[test]
fn a_file_of_another_kind_is_refused_by_name() {
    let refused = prepare("notes.docx", b"PK\x03\x04rest of a zip").unwrap_err();
    assert_eq!(
        refused,
        AttachError::Unsupported {
            name: "notes.docx".to_owned()
        }
    );
    assert!(!looks_like_pdf(b"PK\x03\x04rest of a zip"));
    assert!(looks_like_pdf(b"%PDF-1.7\n"));
    assert!(looks_like_pdf(b"junk junk junk\n%PDF-1.4\n"));
}

#[test]
fn another_pdf_is_attached_as_the_words_of_its_pages() {
    let bytes = document("words", "Hello from another document.");
    let made = prepare("other.pdf", &bytes).expect("a PDF attaches");
    assert_eq!(made.len(), 1, "{made:?}");
    assert_eq!(made[0].name, "other.pdf");
    assert_eq!(made[0].kind, AttachmentKind::Text);
    let words = made[0].as_text().into_owned();
    assert!(words.contains("Hello from another document."), "{words}");
    assert!(words.contains("page 1"), "{words}");
}

#[test]
fn a_pages_words_are_cut_at_the_cap() {
    let bytes = document("cap", "Hello from another document.");
    let held: Arc<[u8]> = Arc::from(bytes);
    let source = pdf_bytes::ByteStore::new(pdf_bytes::SourceId::new(0), held);
    let mut session = pdf_session::Session::new(source, b"");
    let view = session.page_for_display(0).expect("the page reads");
    assert_eq!(text_of_page(&view, 0, 5), "Hello");
    assert_eq!(
        text_of_page(&view, 0, MOST_CHARACTERS),
        "Hello from another document."
    );
}

#[test]
fn a_page_with_no_text_is_attached_as_a_picture() {
    let picture: Arc<[u8]> = Arc::from(png(200, 280));
    let scanned = pdf_session::pictures_into_pdf(&[picture]).expect("a page of a picture");
    let made = prepare("scan.pdf", &scanned).expect("a PDF attaches");
    let pictures: Vec<&Attachment> = made
        .iter()
        .filter(|item| matches!(item.kind, AttachmentKind::Image { .. }))
        .collect();
    assert_eq!(pictures.len(), 1, "{made:?}");
    assert_eq!(pictures[0].name, "scan.pdf page 1");
    assert_eq!(
        pictures[0].kind,
        AttachmentKind::Image {
            media_type: "image/png".to_owned()
        }
    );
    assert!(
        ImageFile::read(&pictures[0].bytes).is_ok(),
        "the page is a picture anything reads"
    );
    let words: String = made
        .iter()
        .filter(|item| item.kind == AttachmentKind::Text)
        .map(|item| item.as_text().into_owned())
        .collect();
    assert!(words.contains("page 1 has no text"), "{words}");
}

#[test]
fn six_files_and_twenty_megabytes_are_the_allowance() {
    let six: Vec<(String, usize)> = (0..MOST_FILES)
        .map(|at| (format!("{at}.png"), 10))
        .collect();
    assert_eq!(fits(&six, ("one more.png", 10)), Err(AttachError::TooMany));
    assert_eq!(fits(&six[..2], ("small.png", 10)), Ok(()));
    let half = vec![("big.pdf".to_owned(), MOST_TOTAL_BYTES - 10)];
    assert_eq!(
        fits(&half, ("another.png", 100)),
        Err(AttachError::TooLarge {
            name: "another.png".to_owned()
        })
    );
    let huge = vec![0_u8; MOST_TOTAL_BYTES + 1];
    assert_eq!(
        prepare("huge.bin", &huge).unwrap_err(),
        AttachError::TooLarge {
            name: "huge.bin".to_owned()
        }
    );
}
