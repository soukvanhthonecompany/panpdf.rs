use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_content::PrintAllowance;

use crate::layout::{Orientation, Paper, Settings, lay_out};
use crate::{DEGRADED_DPI, PRINT_DPI, allowance, draw_printed, draw_sheet, job_dpi};

fn document() -> ByteStore {
    let content = b"0 0 0 rg 0 25 50 25 re f";
    let appearance = b"1 0 0 rg 0 0 20 20 re f";
    let bodies: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 50] /Resources << >> /Contents 4 0 R /Annots [5 0 R] >>"
            .to_vec(),
        stream(b"", content),
        b"<< /Type /Annot /Subtype /Square /Rect [80 0 100 20] /F 36 /AP << /N 6 0 R >> >>".to_vec(),
        stream(b"/Type /XObject /Subtype /Form /BBox [0 0 20 20]", appearance),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (at, body) in bodies.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", at + 1).as_bytes());
        bytes.extend_from_slice(body);
        bytes.extend_from_slice(b"\nendobj\n");
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
    ByteStore::new(SourceId::new(7), Arc::<[u8]>::from(bytes))
}

fn stream(entries: &[u8], data: &[u8]) -> Vec<u8> {
    let mut body = b"<< ".to_vec();
    body.extend_from_slice(entries);
    body.extend_from_slice(format!(" /Length {} >>\nstream\n", data.len()).as_bytes());
    body.extend_from_slice(data);
    body.extend_from_slice(b"\nendstream");
    body
}

#[test]
fn a_page_prints_turned_with_its_paper_only_annotation() {
    let source = document();
    assert_eq!(allowance(&source, b""), Ok(PrintAllowance::Faithful));
    let sheet = lay_out(
        &[(0, [100.0, 50.0])],
        &Settings {
            paper: Paper {
                width: 50.0,
                height: 100.0,
            },
            orientation: Orientation::Portrait,
            margin: 0.0,
            ..Settings::default()
        },
    )
    .expect("lays out")
    .remove(0);
    let image = draw_sheet(&sheet, 72.0, false, |page, scale, region| {
        draw_printed(&source, (b"", None), page, scale, region)
    })
    .expect("draws");
    assert_eq!((image.width, image.height), (50, 100));
    assert_eq!(image.pixel(10, 90), [0, 0, 0]);
    assert_eq!(image.pixel(10, 40), [255, 255, 255]);
    assert_eq!(image.pixel(35, 90), [255, 255, 255]);
    assert_eq!(image.pixel(40, 10), [255, 0, 0]);
}

#[test]
fn a_job_is_drawn_at_the_printers_resolution_within_bounds() {
    assert!((job_dpi(None, PrintAllowance::Faithful) - PRINT_DPI).abs() < f64::EPSILON);
    assert!((job_dpi(Some(600), PrintAllowance::Faithful) - PRINT_DPI).abs() < f64::EPSILON);
    assert!((job_dpi(Some(200), PrintAllowance::Faithful) - 200.0).abs() < f64::EPSILON);
    assert!((job_dpi(Some(72), PrintAllowance::Faithful) - DEGRADED_DPI).abs() < f64::EPSILON);
    assert!((job_dpi(Some(600), PrintAllowance::Degraded) - DEGRADED_DPI).abs() < f64::EPSILON);
}
