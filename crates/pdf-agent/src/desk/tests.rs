use std::path::{Path, PathBuf};
use std::sync::Arc;

use pdf_content::{FontProvider, FontRequest, GlyphProgram, SubstitutedFace};

use super::{BlockRange, Desk, parse_name};

#[derive(Debug)]
struct Packaged {
    latin: Arc<GlyphProgram>,
    thai: Arc<GlyphProgram>,
}

impl Packaged {
    fn face(&self, family: &str) -> Option<SubstitutedFace> {
        let program = match family {
            "DejaVu Sans" => &self.latin,
            "Noto Sans Thai" => &self.thai,
            _ => return None,
        };
        Some(SubstitutedFace {
            program: Arc::clone(program),
            identity: Arc::new(pdf_content::FaceIdentity {
                family: family.to_owned(),
                subfamily: "Regular".to_owned(),
                origin: format!("packaged:{family}"),
                sha256: family.to_owned(),
                face_index: 0,
                style: pdf_content::FontStyle::default(),
            }),
            reason: pdf_content::SubstitutionReason::ExactFamily,
        })
    }
}

impl FontProvider for Packaged {
    fn primary_face(&self, request: &FontRequest) -> Option<SubstitutedFace> {
        self.face(&request.family)
    }

    fn fallback_face(&self, _: &FontRequest, character: char) -> Option<SubstitutedFace> {
        let family = if ('\u{0E00}'..='\u{0E7F}').contains(&character) {
            "Noto Sans Thai"
        } else {
            "DejaVu Sans"
        };
        self.face(family)
    }

    fn description(&self) -> String {
        "packaged DejaVu Sans and Noto Sans Thai".to_owned()
    }
}

pub(crate) fn fonts() -> Arc<dyn FontProvider> {
    let parse = |bytes: &[u8]| Arc::new(GlyphProgram::parse(bytes.to_vec()).expect("parses"));
    Arc::new(Packaged {
        latin: parse(include_bytes!("../../../../fonts/packaged/DejaVuSans.ttf")),
        thai: parse(include_bytes!(
            "../../../../fonts/packaged/NotoSansThai-Regular.ttf"
        )),
    })
}

fn folder(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("panpdf-agent-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("a folder");
    path
}

fn document(folder: &Path, name: &str, text: &str) -> PathBuf {
    let path = folder.join(name);
    std::fs::write(
        &path,
        pdf_session::blank_document([595.0, 842.0]).expect("a blank page"),
    )
    .expect("written");
    let mut desk = Desk::with_fonts(Some(fonts()));
    let opened = desk.open(&path, "", false).expect("opens");
    desk.place_text(
        &opened.handle,
        0,
        [72.0, 72.0, 400.0, 200.0],
        text,
        ("DejaVu Sans", 12.0, false, false, None),
    )
    .expect("the text is placed");
    desk.save(&opened.handle, &path, true)
        .expect("saved over the blank");
    path
}

#[test]
fn a_page_reads_as_named_blocks() {
    let folder = folder("reads");
    let path = document(&folder, "one.pdf", "Hello world, from an agent.");
    let mut desk = Desk::with_fonts(Some(fonts()));
    let handle = desk.open(&path, "", false).expect("opens").handle;
    let blocks = desk.blocks(&handle, 0).expect("the page reads");
    assert_eq!(blocks.len(), 1, "{blocks:?}");
    let block = &blocks[0];
    assert_eq!(block.name(), "p1-b1");
    assert_eq!(block.text, "Hello world, from an agent.");
    assert!((block.size - 12.0).abs() < 0.5, "{}", block.size);
    assert!((block.area[0] - 72.0).abs() < 1.0, "{:?}", block.area);
    assert!(
        block.area[1] > 72.0 && block.area[1] < 90.0,
        "{:?}",
        block.area
    );
    assert!(block.fixed.is_none());
}

#[test]
fn a_word_or_a_whole_block_is_replaced() {
    let folder = folder("replaces");
    let path = document(&folder, "one.pdf", "Hello world, from an agent.");
    let mut desk = Desk::with_fonts(Some(fonts()));
    let handle = desk.open(&path, "", false).expect("opens").handle;
    desk.blocks(&handle, 0).expect("read");
    let now = desk
        .rewrite(&handle, "p1-b1", Some("world"), "there")
        .expect("one word is replaced");
    assert_eq!(now.text, "Hello there, from an agent.");
    let now = desk
        .rewrite(&handle, &now.name(), None, "สวัสดีครับ")
        .expect("the whole block is replaced, in Thai");
    assert_eq!(now.text, "สวัสดีครับ");
}

#[test]
fn a_name_finds_its_block_again_or_is_refused() {
    let folder = folder("names");
    let path = document(&folder, "one.pdf", "First paragraph.");
    let mut desk = Desk::with_fonts(Some(fonts()));
    let handle = desk.open(&path, "", false).expect("opens").handle;
    desk.place_text(
        &handle,
        0,
        [72.0, 400.0, 400.0, 500.0],
        "Second paragraph.",
        ("DejaVu Sans", 12.0, false, false, None),
    )
    .expect("a second paragraph, far below the first");
    let blocks = desk.blocks(&handle, 0).expect("read");
    assert_eq!(blocks.len(), 2, "{blocks:?}");
    let named = |text: &str| {
        blocks
            .iter()
            .find(|block| block.text.starts_with(text))
            .expect("the paragraph")
            .name()
    };
    let (first, second) = (named("First"), named("Second"));

    desk.rewrite(&handle, &first, Some("First"), "Opening")
        .expect("the first is edited");
    let again = desk
        .rewrite(&handle, &second, Some("Second"), "Closing")
        .expect("the second is found by the name read before the edit");
    assert_eq!(again.text, "Closing paragraph.");

    desk.walk(&handle, true).expect("undo the second");
    desk.walk(&handle, true).expect("undo the first");
    let refused = desk
        .rewrite(&handle, &first, None, "x")
        .expect_err("the block changed since its name was given");
    assert!(refused.contains("read_text page 1 again"), "{refused}");

    let never = desk
        .rewrite(&handle, "p1-b9", None, "x")
        .expect_err("never read");
    assert!(never.contains("has not been read"), "{never}");
}

#[test]
fn a_find_that_is_not_one_place_is_refused() {
    let folder = folder("find");
    let path = document(&folder, "one.pdf", "one two one");
    let mut desk = Desk::with_fonts(Some(fonts()));
    let handle = desk.open(&path, "", false).expect("opens").handle;
    desk.blocks(&handle, 0).expect("read");
    let absent = desk
        .rewrite(&handle, "p1-b1", Some("three"), "x")
        .expect_err("absent");
    assert!(absent.contains("is not in the block"), "{absent}");
    let twice = desk
        .rewrite(&handle, "p1-b1", Some("one"), "x")
        .expect_err("twice");
    assert!(twice.contains("2 times"), "{twice}");
    let empty = desk
        .rewrite(&handle, "p1-b1", Some(""), "x")
        .expect_err("empty");
    assert!(empty.contains("empty"), "{empty}");
}

#[test]
fn undo_and_redo_walk_the_edits() {
    let folder = folder("undo");
    let path = document(&folder, "one.pdf", "Keep this.");
    let mut desk = Desk::with_fonts(Some(fonts()));
    let handle = desk.open(&path, "", false).expect("opens").handle;
    assert!(
        !desk.walk(&handle, true).expect("undo"),
        "nothing to undo yet"
    );
    desk.blocks(&handle, 0).expect("read");
    desk.rewrite(&handle, "p1-b1", None, "Changed.")
        .expect("edited");
    assert!(desk.walk(&handle, true).expect("undo"));
    assert_eq!(desk.blocks(&handle, 0).expect("read")[0].text, "Keep this.");
    assert!(desk.walk(&handle, false).expect("redo"));
    assert_eq!(desk.blocks(&handle, 0).expect("read")[0].text, "Changed.");
}

#[test]
fn saving_keeps_what_it_was_not_told_to_replace() {
    let folder = folder("save");
    let path = document(&folder, "one.pdf", "Before.");
    let mut desk = Desk::with_fonts(Some(fonts()));
    let handle = desk.open(&path, "", false).expect("opens").handle;
    desk.blocks(&handle, 0).expect("read");
    desk.rewrite(&handle, "p1-b1", None, "After.")
        .expect("edited");

    let refused = desk.save(&handle, &path, false).expect_err("exists");
    assert!(refused.contains("already exists"), "{refused}");

    let copy = folder.join("copy.pdf");
    desk.save(&handle, &copy, false)
        .expect("a new file is written");
    let mut other = Desk::with_fonts(Some(fonts()));
    let reopened = other.open(&copy, "", false).expect("reopens").handle;
    assert_eq!(other.blocks(&reopened, 0).expect("read")[0].text, "After.");

    let mut changed = std::fs::read(&path).expect("read");
    changed.extend_from_slice(b"\n% changed by someone else\n");
    std::fs::write(&path, &changed).expect("written");
    let refused = desk
        .save(&handle, &path, true)
        .expect_err("changed on disk");
    assert!(refused.contains("changed on disk"), "{refused}");
    assert_eq!(
        std::fs::read(&path).expect("read"),
        changed,
        "left as it was"
    );

    let left: Vec<_> = std::fs::read_dir(&folder)
        .expect("listed")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".tmp"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn block_names_read_as_page_and_block() {
    assert_eq!(parse_name("p3-b12"), Ok((2, 11)));
    for bad in ["p0-b1", "p1-b0", "3-12", "p3b12", "p-b", "pX-b1", ""] {
        assert!(parse_name(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn a_find_across_a_break_is_a_range_across_it() {
    let folder = folder("across");
    let path = document(&folder, "one.pdf", "ab\ncd efgh ijkl mnop");
    let mut desk = Desk::with_fonts(Some(fonts()));
    let handle = desk.open(&path, "", false).expect("opens").handle;
    let blocks = desk.blocks(&handle, 0).expect("read");
    assert_eq!(blocks.len(), 1, "{blocks:?}");
    assert_eq!(blocks[0].text, "ab\ncd efgh ijkl mnop");
    let open = desk.open.get_mut(&handle).expect("open");
    let view = super::view_of(&mut open.session, 0).expect("page");
    let (_, parts) = super::read(&view, 0, 0).expect("the block");
    assert_eq!(
        super::range_within(&parts.reading, "b\nc"),
        Ok(BlockRange::Between {
            from: (0, 1),
            to: (1, 1)
        })
    );
}

#[test]
fn a_name_from_before_the_pages_moved_is_refused() {
    let folder = folder("moved");
    let path = folder.join("three.pdf");
    std::fs::write(
        &path,
        pdf_session::blank_document([595.0, 842.0]).expect("a blank page"),
    )
    .expect("written");
    let mut desk = Desk::with_fonts(Some(fonts()));
    let handle = desk.open(&path, "", false).expect("opens").handle;
    for _ in 0..2 {
        desk.command(
            &handle,
            &pdf_edit::Command::AddBlankPage {
                beside: 0,
                before: false,
                size: [595.0, 842.0],
            },
        )
        .expect("another page");
    }
    for page in 0..3 {
        desk.place_text(
            &handle,
            page,
            [72.0, 700.0, 400.0, 60.0],
            "Company confidential.",
            ("DejaVu Sans", 10.0, false, false, None),
        )
        .expect("the same footer on every page");
        desk.blocks(&handle, page).expect("the page reads");
    }

    desk.command(&handle, &pdf_edit::Command::RemovePages { pages: vec![0] })
        .expect("the first page goes");

    let refused = desk
        .rewrite(&handle, "p2-b1", None, "Public.")
        .expect_err("a name from before the pages moved means nothing now");
    assert!(
        refused.contains("before the pages were moved") && refused.contains("read_text"),
        "{refused}"
    );
    for page in 0..2 {
        let footer = &desk.blocks(&handle, page).expect("what is left reads")[0];
        assert_eq!(
            footer.text,
            "Company confidential.",
            "the footer of page {} says what it said",
            page + 1
        );
    }
}
