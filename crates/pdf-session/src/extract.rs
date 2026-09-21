use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::Command;
use pdf_edit::spike_move_text::SpikeError;

use crate::Session;
use crate::pictures::PAGE_POINTS;

pub fn extract_pages(document: &Arc<[u8]>, pages: &[usize]) -> Result<Vec<u8>, SpikeError> {
    if pages.is_empty() {
        return Err(SpikeError::RetypeUnsupported(
            "no page was named to copy out",
        ));
    }
    let starter = ByteStore::new(SourceId::new(0), empty_document());
    let mut session = Session::new(starter, b"");
    session.apply_each(&[
        Command::InsertPages {
            beside: 0,
            before: false,
            document: Arc::clone(document),
            pages: pages.to_vec(),
        },
        Command::RemovePages { pages: vec![0] },
    ])?;
    Ok(session.source().as_bytes().to_vec())
}

pub fn blank_document(size: [f64; 2]) -> Result<Vec<u8>, SpikeError> {
    if size
        .iter()
        .any(|side| !(PAGE_POINTS.0..=PAGE_POINTS.1).contains(side))
    {
        return Err(SpikeError::RetypeUnsupported(
            "a page that size is larger or smaller than PDF allows",
        ));
    }
    let starter = ByteStore::new(SourceId::new(0), empty_document());
    let mut session = Session::new(starter, b"");
    session.apply_each(&[
        Command::AddBlankPage {
            beside: 0,
            before: false,
            size,
        },
        Command::RemovePages { pages: vec![0] },
    ])?;
    Ok(session.source().as_bytes().to_vec())
}

pub(crate) fn empty_document() -> Arc<[u8]> {
    let bodies: [&[u8]; 4] = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595.28 841.89] /Resources << >> \
          /Contents 4 0 R >>",
        b"<< /Length 0 >>\nstream\n\nendstream",
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::with_capacity(bodies.len());
    for (at, body) in bodies.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", at + 1).as_bytes());
        bytes.extend_from_slice(body);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let size = bodies.len() + 1;
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    Arc::from(bytes)
}

#[cfg(test)]
#[expect(
    clippy::float_cmp,
    reason = "a page size written as a number is read back as the same number"
)]
mod tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};

    use super::{blank_document, empty_document, extract_pages};
    use crate::interpret_page;

    const MARKERS: [&str; 3] = ["One", "Two", "Three"];

    fn three_pages() -> Arc<[u8]> {
        let mut bodies: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R 6 0 R 9 0 R] /Count 3 >>".to_vec(),
        ];
        for (at, (width, height)) in [(200, 100), (300, 300), (400, 200)].into_iter().enumerate() {
            let page = 3 + at * 3;
            let marker = MARKERS[at];
            bodies.push(
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] \
                     /Resources << /XObject << /Mark{} {} 0 R >> >> /Contents {} 0 R >>",
                    marker,
                    page + 2,
                    page + 1,
                )
                .into_bytes(),
            );
            bodies.push(stream(
                b"<< >>",
                format!(
                    "q {} 0 0 {} 10 10 cm /Mark{marker} Do Q",
                    20 + at * 10,
                    20 + at * 10
                )
                .as_bytes(),
            ));
            bodies.push(stream(
                format!(
                    "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace \
                     /DeviceGray /BitsPerComponent 8 /Decode [{at} 1] >>"
                )
                .as_bytes(),
                &[b'A' + u8::try_from(at).expect("three pages")],
            ));
        }
        write_document(&bodies)
    }

    fn stream(dictionary: &[u8], data: &[u8]) -> Vec<u8> {
        let mut out = dictionary[..dictionary.len() - 2].to_vec();
        out.extend_from_slice(format!(" /Length {} >>\nstream\n", data.len()).as_bytes());
        out.extend_from_slice(data);
        out.extend_from_slice(b"\nendstream");
        out
    }

    fn write_document(bodies: &[Vec<u8>]) -> Arc<[u8]> {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (at, body) in bodies.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n", at + 1).as_bytes());
            bytes.extend_from_slice(body);
            bytes.extend_from_slice(b"\nendobj\n");
        }
        let size = bodies.len() + 1;
        let xref = bytes.len();
        bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n")
                .as_bytes(),
        );
        Arc::from(bytes)
    }

    fn store(bytes: &Arc<[u8]>) -> ByteStore {
        ByteStore::new(SourceId::new(7), Arc::clone(bytes))
    }

    fn painting(bytes: &Arc<[u8]>, page: usize) -> Vec<String> {
        interpret_page(&store(bytes), page)
            .expect("the page reads")
            .graph
            .atoms
            .iter()
            .map(|atom| pdf_paint::paint_signature(&atom.kind))
            .collect()
    }

    fn size(bytes: &Arc<[u8]>, page: usize) -> [f64; 4] {
        pdf_content::page_geometries_with_password(
            &store(bytes),
            pdf_content::PageContentLimits::default(),
            b"",
        )
        .expect("the pages lay out")[page]
            .media_box
    }

    #[test]
    fn the_pages_named_come_out_in_that_order() {
        let document = three_pages();
        let copied: Arc<[u8]> = Arc::from(extract_pages(&document, &[2, 0]).expect("extracted"));
        assert_eq!(
            pdf_content::count_pages_strict(
                &store(&copied),
                pdf_content::PageContentLimits::default()
            )
            .expect("the copy's pages count"),
            2
        );
        for (there, here) in [(2, 0), (0, 1)] {
            assert_eq!(painting(&document, there), painting(&copied, here));
            assert_eq!(size(&document, there), size(&copied, here));
        }
        assert!(!painting(&copied, 0).is_empty(), "a page that paints");
    }

    #[test]
    fn a_page_carries_its_own_objects_and_no_others() {
        let document = three_pages();
        let copied = extract_pages(&document, &[0]).expect("extracted");
        let holds = |name: &str| {
            let name = name.as_bytes();
            copied.windows(name.len()).any(|window| window == name)
        };
        assert!(holds("/MarkOne"), "the copied page's own picture");
        for other in ["/MarkTwo", "/MarkThree"] {
            assert!(!holds(other), "a page not copied leaves nothing behind");
        }
        assert!(
            copied.len() < document.len(),
            "one page of three is smaller than three"
        );
    }

    #[test]
    fn a_page_asked_for_twice_comes_out_twice() {
        let document = three_pages();
        let copied: Arc<[u8]> = Arc::from(extract_pages(&document, &[1, 1]).expect("extracted"));
        assert_eq!(painting(&copied, 0), painting(&copied, 1));
        assert_eq!(painting(&copied, 0), painting(&document, 1));
    }

    #[test]
    fn nothing_and_no_such_page_are_refused() {
        let document = three_pages();
        let nothing = extract_pages(&document, &[]).expect_err("refused");
        assert!(
            nothing.to_string().contains("no page was named"),
            "{nothing}"
        );
        assert!(extract_pages(&document, &[3]).is_err());
        assert!(extract_pages(&document, &[0, 9]).is_err());
    }

    #[test]
    fn a_new_document_is_one_blank_page_of_the_size_asked_for() {
        let letter: Arc<[u8]> = Arc::from(blank_document([612.0, 792.0]).expect("a new document"));
        let store = store(&letter);
        let pages = pdf_content::page_geometries_with_password(
            &store,
            pdf_content::PageContentLimits::default(),
            b"",
        )
        .expect("the page lays out");
        assert_eq!(pages.len(), 1, "one page");
        assert_eq!(pages[0].media_box, [0.0, 0.0, 612.0, 792.0]);
        assert!(painting(&letter, 0).is_empty(), "a new page paints nothing");
        assert!(blank_document([612.0, 0.5]).is_err(), "too small");
        assert!(blank_document([612.0, 20_000.0]).is_err(), "too large");
    }

    #[test]
    fn the_empty_document_is_one_blank_page() {
        let blank = empty_document();
        assert_eq!(
            pdf_content::count_pages_strict(
                &store(&blank),
                pdf_content::PageContentLimits::default()
            )
            .expect("the blank document's pages count"),
            1
        );
        assert!(
            painting(&blank, 0).is_empty(),
            "a blank page paints nothing"
        );
    }
}
