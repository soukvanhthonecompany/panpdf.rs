use crate::error::InterpretError;
use crate::graph::{PaintAtomKind, PaintGraph};
use crate::interpreter::{
    PaintContext, PaintStream, interpret_operations, interpret_stream_sequence_with_resources,
};
use crate::state::PaintLimits;
use pdf_bytes::{ByteStore, SourceId};
use pdf_content::{
    ContentLimits, PageContentLimits, load_page_program_strict, parse_operations_strict,
};
use std::sync::Arc;

pub(crate) fn assert_floats(actual: &[f64], expected: &[f64]) {
    assert_eq!(actual.len(), expected.len());
    assert!(
        actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| (actual - expected).abs() < f64::EPSILON)
    );
}

pub(crate) fn hex_fixture(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            u8::from_str_radix(std::str::from_utf8(pair).expect("ASCII hex pair"), 16)
                .expect("hex fixture byte")
        })
        .collect()
}

pub(crate) const ADOBE_CMYK_JPEG: &str = concat!(
    "ffd8ffee000e41646f626500640000000000ffdb0043000101010101010101010101010101010101",
    "01010101010101010101010101010101010101010101010101010101010101010101010101010101",
    "01010101010101ffc000140800010001044311004d11005911004b1100ffc4001f00000105010101",
    "01010100000000000000000102030405060708090a0bffc400b51000020103030204030505040400",
    "00017d01020300041105122131410613516107227114328191a1082342b1c11552d1f02433627282",
    "090a161718191a25262728292a3435363738393a434445464748494a535455565758595a63646566",
    "6768696a737475767778797a838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3",
    "b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae1e2e3e4e5e6e7e8e9eaf1f2f3f4f5",
    "f6f7f8f9faffda000e0443004d0059004b00003f00feea2bfb58afec22bfab8affd9",
);

pub(crate) const PLAIN_CMYK_JPEG: &str = concat!(
    "ffd8ffdb004300010101010101010101010101010101010101010101010101010101010101010101",
    "01010101010101010101010101010101010101010101010101010101010101ffc000140800010001",
    "044311004d11005911004b1100ffc4001f0000010501010101010100000000000000000102030405",
    "060708090a0bffc400b5100002010303020403050504040000017d01020300041105122131410613",
    "516107227114328191a1082342b1c11552d1f02433627282090a161718191a25262728292a343536",
    "3738393a434445464748494a535455565758595a636465666768696a737475767778797a83848586",
    "8788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9ca",
    "d2d3d4d5d6d7d8d9dae1e2e3e4e5e6e7e8e9eaf1f2f3f4f5f6f7f8f9faffda000e0443004d005900",
    "4b00003f00feea2bfb58afec22bfab8affd9",
);

pub(crate) const COLOR_JPEG: &str = concat!(
    "ffd8ffe000104a46494600010100000100010000ffdb004300010101010101010101010101010101",
    "01010101010101010101010101010101010101010101010101010101010101010101010101010101",
    "010101010101010101ffdb0043010101010101010101010101010101010101010101010101010101",
    "0101010101010101010101010101010101010101010101010101010101010101010101010101ffc0",
    "0011080004000403011100021101031101ffc4001f00000105010101010101000000000000000001",
    "02030405060708090a0bffc400b5100002010303020403050504040000017d010203000411051221",
    "31410613516107227114328191a1082342b1c11552d1f02433627282090a161718191a2526272829",
    "2a3435363738393a434445464748494a535455565758595a636465666768696a737475767778797a",
    "838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6",
    "c7c8c9cad2d3d4d5d6d7d8d9dae1e2e3e4e5e6e7e8e9eaf1f2f3f4f5f6f7f8f9faffc4001f010003",
    "0101010101010101010000000000000102030405060708090a0bffc400b511000201020404030407",
    "05040400010277000102031104052131061241510761711322328108144291a1b1c109233352f015",
    "6272d10a162434e125f11718191a262728292a35363738393a434445464748494a53545556575859",
    "5a636465666768696a737475767778797a82838485868788898a92939495969798999aa2a3a4a5a6",
    "a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae2e3e4e5e6e7e8e9ea",
    "f2f3f4f5f6f7f8f9faffda000c03010002110311003f00f1cf146a9e35f0868df0ba5f0e7c5af8e1",
    "e101e33f83ff000e7e226b565f087e377c56fd9b341bad77c69a0c5ac5d36a1e14fd977c5df053c2",
    "7e26b9d16da6b2f08786fc51e32d0fc4be3cd2be1bf86bc09f0c878ba7f00fc3af017877c37fd11f",
    "470fa05fd1bbe9e79271f715f8f7c05c2d8ccc3c0ce3fc3f807c0185c8bc35f062394659c0987f0d",
    "7c39f1a7170c3657c4de1971461324c6679e25f8d7e2371867d85e13870e70e667c49c4b9cf13d6e",
    "1f5c519ff12e779dff0013fd393c6bf15bc16e2ef07b03e1571be69c1987e3af066a7881c6d530f8",
    "5ca33fcd38cb8f71be35f8d1c339af1e717f1171865bc45c49c57c79c499470ae43538df8eb88f38",
    "cd38c3c45e27c3e67e2178879e714f88dc53c5dc5d9fff00ffd9",
);

pub(crate) const COLOR_JPEG_RGB: [u8; 48] = [
    254, 0, 0, 0, 255, 1, 0, 0, 254, 254, 255, 0, 0, 255, 255, 255, 0, 254, 255, 255, 253, 0, 0, 0,
    128, 63, 31, 33, 64, 128, 199, 10, 90, 10, 200, 90, 70, 70, 70, 180, 180, 180, 255, 128, 0, 1,
    129, 255,
];

pub(crate) const GRAY_JPEG: &str = concat!(
    "ffd8ffe000104a46494600010100000100010000ffdb004300030202030202030303030403030405",
    "0805050404050a070706080c0a0c0c0b0a0b0b0d0e12100d0e110e0b0b1016101113141515150c0f",
    "171816141812141514ffc0000b080004000401011100ffc4001f0000010501010101010100000000",
    "000000000102030405060708090a0bffc400b5100002010303020403050504040000017d01020300",
    "041105122131410613516107227114328191a1082342b1c11552d1f02433627282090a161718191a",
    "25262728292a3435363738393a434445464748494a535455565758595a636465666768696a737475",
    "767778797a838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9ba",
    "c2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae1e2e3e4e5e6e7e8e9eaf1f2f3f4f5f6f7f8f9faffda",
    "0008010100003f00f92f56fdb17c49e19d467d3b40f05f80343d2e172b1595b786e29513e8d31773",
    "f8b1afffd9",
);

pub(crate) const GRAY_JPEG_SAMPLES: [u8; 16] = [
    1, 28, 67, 93, 129, 157, 198, 224, 255, 201, 140, 105, 49, 30, 14, 7,
];

pub(crate) const RED_JPEG: &str = concat!(
    "ffd8ffe000104a46494600010100000000000000ffdb004300010101010101010101010101010101",
    "01010101010101010101010101010101010101010101010101010101010101010101010101010101",
    "010101010101010101ffdb0043010101010101010101010101010101010101010101010101010101",
    "0101010101010101010101010101010101010101010101010101010101010101010101010101ffc0",
    "0011080001000103011100021101031101ffc40014000100000000000000000000000000000009ff",
    "c40014100100000000000000000000000000000000ffc40015010101000000000000000000000000",
    "0000090affc40014110100000000000000000000000000000000ffda000c03010002110311003f00",
    "17c53afe1fffd9",
);

pub(crate) fn resource_fixture(content: &[u8], ext_gstate: &[u8]) -> ByteStore {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ExtGState << /GS1 5 0 R >> >> >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
    bytes.extend_from_slice(content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"5 0 obj\n");
    bytes.extend_from_slice(ext_gstate);
    bytes.extend_from_slice(b"\nendobj\n");
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(b"trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n");
    bytes.extend_from_slice(xref.to_string().as_bytes());
    bytes.extend_from_slice(b"\n%%EOF\n");
    ByteStore::new(SourceId::new(61), Arc::<[u8]>::from(bytes))
}

fn type3_fixture(content: &[u8], procedure: &[u8], font_matrix: &[u8]) -> ByteStore {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
    bytes.extend_from_slice(content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"5 0 obj\n<< /Type /Font /Subtype /Type3 /FontMatrix ");
    bytes.extend_from_slice(font_matrix);
    bytes.extend_from_slice(
        b" /FontBBox [0 0 1000 1000] /FirstChar 65 /LastChar 66 /Widths [500 500]\
 /Encoding << /Type /Encoding /Differences [65 /square /absent] >>\
 /CharProcs << /square 6 0 R >> >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"6 0 obj\n<< /Length ");
    bytes.extend_from_slice(procedure.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(procedure);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    let size = offsets.len() + 1;
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n").as_bytes(),
    );
    bytes.extend_from_slice(xref.to_string().as_bytes());
    bytes.extend_from_slice(b"\n%%EOF\n");
    ByteStore::new(SourceId::new(73), Arc::<[u8]>::from(bytes))
}

pub(crate) fn type3_graph(
    content: &[u8],
    procedure: &[u8],
    font_matrix: &[u8],
) -> Result<PaintGraph, InterpretError> {
    let source = type3_fixture(content, procedure, font_matrix);
    let page =
        load_page_program_strict(&source, 0, PageContentLimits::default()).expect("Type 3 page");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("content operations");
    interpret_stream_sequence_with_resources(
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
}

pub(crate) fn type3_text(graph: &PaintGraph) -> &crate::TextShowPaint {
    match &graph.atoms[0].kind {
        PaintAtomKind::Text(text) => text,
        other => panic!("expected one text atom, found {other:?}"),
    }
}

pub(crate) fn soft_mask_fixture(mask_entries: &[u8], indirect: bool) -> ByteStore {
    let page_content = b"/GS1 gs 10 20 30 40 re f";
    let group_content = b".5 g 0 0 2 2 re f";
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ExtGState << /GS1 5 0 R >> >> >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
    bytes.extend_from_slice(page_content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(page_content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"5 0 obj\n<< /Type /ExtGState /SMask ");
    if indirect {
        bytes.extend_from_slice(b"7 0 R");
    } else {
        bytes.extend_from_slice(b"<< ");
        bytes.extend_from_slice(mask_entries);
        bytes.extend_from_slice(b" >>");
    }
    bytes.extend_from_slice(b" >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"6 0 obj\n<< /Type /XObject /Subtype /Form /BBox [0 0 2 2] /Group << /Type /Group /S /Transparency /I true /CS /DeviceRGB >> /Resources << >> /Length ");
    bytes.extend_from_slice(group_content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(group_content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    if indirect {
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"7 0 obj\n<< ");
        bytes.extend_from_slice(mask_entries);
        bytes.extend_from_slice(b" >>\nendobj\n");
    }
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"8 0 obj\n[.1 .2 .3]\nendobj\n");
    let size = offsets.len() + 1;
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n").as_bytes(),
    );
    bytes.extend_from_slice(xref.to_string().as_bytes());
    bytes.extend_from_slice(b"\n%%EOF\n");
    ByteStore::new(SourceId::new(67), Arc::<[u8]>::from(bytes))
}

pub(crate) fn shading_fixture(shading: &[u8]) -> ByteStore {
    let content = b"/S1 sh";
    let profile = icc_profile_bytes(*b"RGB ");
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Shading << /S1 5 0 R >> /ColorSpace << /ICC [/ICCBased 6 0 R] /IDX [/Indexed /DeviceRGB 1 <000000 FFFFFF>] >> >> >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
    bytes.extend_from_slice(content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"5 0 obj\n");
    bytes.extend_from_slice(shading);
    bytes.extend_from_slice(b"\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"6 0 obj\n<< /N 3 /Length ");
    bytes.extend_from_slice(profile.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(&profile);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 7\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(b"trailer\n<< /Size 7 /Root 1 0 R >>\nstartxref\n");
    bytes.extend_from_slice(xref.to_string().as_bytes());
    bytes.extend_from_slice(b"\n%%EOF\n");
    ByteStore::new(SourceId::new(68), Arc::<[u8]>::from(bytes))
}

pub(crate) fn color_space_fixture() -> ByteStore {
    let content = b"/Cs1 cs .2 .4 .6 sc /DeviceCMYK CS .1 .2 .3 .4 SC 0 0 2 3 re B";
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ColorSpace << /Cs1 /DeviceRGB >> >> >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
    bytes.extend_from_slice(content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(b"trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n");
    bytes.extend_from_slice(xref.to_string().as_bytes());
    bytes.extend_from_slice(b"\n%%EOF\n");
    ByteStore::new(SourceId::new(65), Arc::<[u8]>::from(bytes))
}

pub(crate) fn calibrated_color_fixture(color_spaces: &[u8], content: &[u8]) -> ByteStore {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ColorSpace << ",
    );
    bytes.extend_from_slice(color_spaces);
    bytes.extend_from_slice(b" >> >> >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
    bytes.extend_from_slice(content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(b"trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n");
    bytes.extend_from_slice(xref.to_string().as_bytes());
    bytes.extend_from_slice(b"\n%%EOF\n");
    ByteStore::new(SourceId::new(69), Arc::<[u8]>::from(bytes))
}

pub(crate) fn tint_stream_fixture(
    color_spaces: &[u8],
    stream_entries: &[u8],
    stream_data: &[u8],
    content: &[u8],
) -> ByteStore {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ColorSpace << ",
    );
    bytes.extend_from_slice(color_spaces);
    bytes.extend_from_slice(b" >> >> >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
    bytes.extend_from_slice(content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"5 0 obj\n<< ");
    bytes.extend_from_slice(stream_entries);
    bytes.extend_from_slice(b" /Length ");
    bytes.extend_from_slice(stream_data.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(stream_data);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(b"trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n");
    bytes.extend_from_slice(xref.to_string().as_bytes());
    bytes.extend_from_slice(b"\n%%EOF\n");
    ByteStore::new(SourceId::new(71), Arc::<[u8]>::from(bytes))
}

pub(crate) fn shading_function_stream_fixture(
    shading: &[u8],
    function_entries: &[u8],
    function_data: &[u8],
) -> ByteStore {
    let content = b"/S1 sh";
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Shading << /S1 6 0 R >> >> >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
    bytes.extend_from_slice(content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"5 0 obj\n<< ");
    bytes.extend_from_slice(function_entries);
    bytes.extend_from_slice(b" /Length ");
    bytes.extend_from_slice(function_data.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(function_data);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"6 0 obj\n");
    bytes.extend_from_slice(shading);
    bytes.extend_from_slice(b"\nendobj\n");
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 7\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(b"trailer\n<< /Size 7 /Root 1 0 R >>\nstartxref\n");
    bytes.extend_from_slice(xref.to_string().as_bytes());
    bytes.extend_from_slice(b"\n%%EOF\n");
    ByteStore::new(SourceId::new(72), Arc::<[u8]>::from(bytes))
}

pub(crate) fn interpret_color_page(source: &ByteStore) -> Result<PaintGraph, InterpretError> {
    let page = load_page_program_strict(source, 0, PageContentLimits::default())
        .expect("colour-space fixture page");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("colour-space fixture operations");
    interpret_stream_sequence_with_resources(
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
}

pub(crate) fn icc_profile_bytes(data_color_space: [u8; 4]) -> Vec<u8> {
    let mut profile = vec![0_u8; 152];
    profile[0..4].copy_from_slice(&152_u32.to_be_bytes());
    profile[8..12].copy_from_slice(&[4, 3, 0, 0]);
    profile[12..16].copy_from_slice(b"mntr");
    profile[16..20].copy_from_slice(&data_color_space);
    profile[20..24].copy_from_slice(b"XYZ ");
    profile[36..40].copy_from_slice(b"acsp");
    profile[128..132].copy_from_slice(&1_u32.to_be_bytes());
    profile[132..136].copy_from_slice(b"tst0");
    profile[136..140].copy_from_slice(&144_u32.to_be_bytes());
    profile[140..144].copy_from_slice(&8_u32.to_be_bytes());
    profile[144..148].copy_from_slice(b"test");
    profile
}

pub(crate) fn icc_color_fixture(
    profile_entries: &[u8],
    profile: &[u8],
    content: &[u8],
) -> ByteStore {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ColorSpace << /ICC [/ICCBased 5 0 R] >> >> >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
    bytes.extend_from_slice(content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"5 0 obj\n<< ");
    bytes.extend_from_slice(profile_entries);
    bytes.extend_from_slice(b" /Length ");
    bytes.extend_from_slice(profile.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(profile);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(b"trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n");
    bytes.extend_from_slice(xref.to_string().as_bytes());
    bytes.extend_from_slice(b"\n%%EOF\n");
    ByteStore::new(SourceId::new(70), Arc::<[u8]>::from(bytes))
}

pub(crate) fn indexed_stream_fixture(
    lookup_entries: &[u8],
    lookup: &[u8],
    content: &[u8],
) -> ByteStore {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ColorSpace << /IDX [/Indexed /DeviceRGB 1 5 0 R] >> >> >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
    bytes.extend_from_slice(content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"5 0 obj\n<< ");
    bytes.extend_from_slice(lookup_entries);
    bytes.extend_from_slice(b" /Length ");
    bytes.extend_from_slice(lookup.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(lookup);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(b"trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n");
    bytes.extend_from_slice(xref.to_string().as_bytes());
    bytes.extend_from_slice(b"\n%%EOF\n");
    ByteStore::new(SourceId::new(72), Arc::<[u8]>::from(bytes))
}

pub(crate) fn icc_group_fixture(profile: &[u8]) -> ByteStore {
    let page_content = b"/F1 Do";
    let form_content = b"0 0 1 1 re f";
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /F1 5 0 R >> >> >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
    bytes.extend_from_slice(page_content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(page_content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"5 0 obj\n<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Group << /S /Transparency /CS [/ICCBased 6 0 R] >> /Resources << >> /Length ");
    bytes.extend_from_slice(form_content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(form_content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"6 0 obj\n<< /N 3 /Length ");
    bytes.extend_from_slice(profile.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(profile);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 7\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(b"trailer\n<< /Size 7 /Root 1 0 R >>\nstartxref\n");
    bytes.extend_from_slice(xref.to_string().as_bytes());
    bytes.extend_from_slice(b"\n%%EOF\n");
    ByteStore::new(SourceId::new(71), Arc::<[u8]>::from(bytes))
}

pub(crate) fn failing_form_fixture() -> ByteStore {
    let page_content = b"/F1 Do 0 0 1 rg 0 0 5 5 re f";
    let form_content = b"/Nope gs 1 1 1 1 re f";
    let objects: [Vec<u8>; 5] = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /F1 5 0 R >> >> >>"
            .to_vec(),
        [
            format!("<< /Length {} >>\nstream\n", page_content.len()).as_bytes(),
            page_content,
            b"\nendstream",
        ]
        .concat(),
        [
            format!(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << >> /Length {} >>\nstream\n",
                form_content.len()
            )
            .as_bytes(),
            form_content,
            b"\nendstream",
        ]
        .concat(),
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
    bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    ByteStore::new(SourceId::new(63), Arc::<[u8]>::from(bytes))
}

pub(crate) fn form_paint_fixture(group: &[u8], indirect_group: Option<&[u8]>) -> ByteStore {
    let page_content = b"2 0 0 2 10 20 cm /F1 Do 0 0 1 1 re S /F1 Do";
    let form_content = b"/GS1 gs 1 2 3 4 re f";
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /F1 5 0 R >> >> >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
    bytes.extend_from_slice(page_content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(page_content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"5 0 obj\n<< /Type /XObject /Subtype /Form /BBox [5 6 0 0] /Matrix [1 0 0 1 3 4] ",
    );
    bytes.extend_from_slice(group);
    bytes.extend_from_slice(
        b" /Resources << /ExtGState << /GS1 << /Type /ExtGState /ca .5 >> >> >> /Length ",
    );
    bytes.extend_from_slice(form_content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(form_content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    if let Some(dictionary) = indirect_group {
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"6 0 obj\n");
        bytes.extend_from_slice(dictionary);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let size = offsets.len() + 1;
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n").as_bytes(),
    );
    bytes.extend_from_slice(xref.to_string().as_bytes());
    bytes.extend_from_slice(b"\n%%EOF\n");
    ByteStore::new(SourceId::new(62), Arc::<[u8]>::from(bytes))
}

pub(crate) fn pattern_paint_fixture(paint_type: i64) -> ByteStore {
    uncoloured_pattern_paint_fixture(paint_type, b"/Pattern", b"")
}

pub(crate) fn uncoloured_pattern_paint_fixture(
    paint_type: i64,
    space: &[u8],
    select: &[u8],
) -> ByteStore {
    let mut page_content = b"/CS0 cs ".to_vec();
    page_content.extend_from_slice(select);
    page_content.extend_from_slice(b" /P1 scn 10 20 30 40 re f");
    let page_content = page_content.as_slice();
    let pattern_content = b".25 g 0 0 2 2 re f";
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Pattern << /P1 5 0 R >> /ColorSpace << /CS0 ");
    bytes.extend_from_slice(space);
    bytes.extend_from_slice(b" >> >> >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
    bytes.extend_from_slice(page_content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(page_content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"5 0 obj\n<< /Type /Pattern /PatternType 1 /PaintType ");
    bytes.extend_from_slice(paint_type.to_string().as_bytes());
    bytes.extend_from_slice(b" /TilingType 2 /BBox [0 0 2 2] /XStep 2 /YStep 2 /Matrix [1 0 0 1 3 4] /Resources << >> /Length ");
    bytes.extend_from_slice(pattern_content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(pattern_content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(b"trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n");
    bytes.extend_from_slice(xref.to_string().as_bytes());
    bytes.extend_from_slice(b"\n%%EOF\n");
    ByteStore::new(SourceId::new(66), Arc::<[u8]>::from(bytes))
}

pub(crate) fn shading_pattern_fixture(matrix: &[u8], inside_form: bool) -> ByteStore {
    let page_content: &[u8] = if inside_form {
        b"1 0 0 1 5 7 cm /Fm1 Do"
    } else {
        b"/Pattern cs /P1 scn 0 0 20 10 re f"
    };
    let form_content = b"/Pattern cs /P1 scn 0 0 20 10 re f";
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 40 20] /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Pattern << /P1 5 0 R >> /XObject << /Fm1 6 0 R >> >> >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
    bytes.extend_from_slice(page_content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(page_content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"5 0 obj\n<< /Type /Pattern /PatternType 2 ");
    bytes.extend_from_slice(matrix);
    bytes.extend_from_slice(
        b" /Shading << /ShadingType 2 /ColorSpace /DeviceGray /Coords [0 0 10 0] \
/Function << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >> /Extend [true true] >> >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"6 0 obj\n<< /Type /XObject /Subtype /Form /BBox [0 0 20 10] /Matrix [2 0 0 2 0 0] \
/Resources << /Pattern << /P1 5 0 R >> >> /Length ",
    );
    bytes.extend_from_slice(form_content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(form_content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    let size = offsets.len() + 1;
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n").as_bytes(),
    );
    bytes.extend_from_slice(xref.to_string().as_bytes());
    bytes.extend_from_slice(b"\n%%EOF\n");
    ByteStore::new(SourceId::new(67), Arc::<[u8]>::from(bytes))
}

pub(crate) fn image_paint_fixture(
    image_entries: &[u8],
    image_data: &[u8],
    soft_mask: Option<(&[u8], &[u8])>,
    indirect_object: Option<&[u8]>,
) -> ByteStore {
    assert!(soft_mask.is_none() || indirect_object.is_none());
    let content = b"2 0 0 1 10 20 cm /Im1 Do";
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 40 40] /Kids [3 0 R] /Count 1 >>\nendobj\n",
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
    bytes.extend_from_slice(b"5 0 obj\n<< /Type /XObject /Subtype /Image ");
    bytes.extend_from_slice(image_entries);
    bytes.extend_from_slice(b" /Length ");
    bytes.extend_from_slice(image_data.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(image_data);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    if let Some((mask_entries, mask_data)) = soft_mask {
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"6 0 obj\n<< /Type /XObject /Subtype /Image ");
        bytes.extend_from_slice(mask_entries);
        bytes.extend_from_slice(b" /Length ");
        bytes.extend_from_slice(mask_data.len().to_string().as_bytes());
        bytes.extend_from_slice(b" >>\nstream\n");
        bytes.extend_from_slice(mask_data);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
    } else if let Some(value) = indirect_object {
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"6 0 obj\n");
        bytes.extend_from_slice(value);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let size = offsets.len() + 1;
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n").as_bytes(),
    );
    bytes.extend_from_slice(xref.to_string().as_bytes());
    bytes.extend_from_slice(b"\n%%EOF\n");
    ByteStore::new(SourceId::new(73), Arc::<[u8]>::from(bytes))
}

pub(crate) const BLANK_GLYPH_CFF: &str = "0100040100010101055465737400010101131d00000030111d000000470f\
    1d0000004c100001010106616c7068610000000301010210110e8b8b15f8888b8bf888fc888b050e0e0000220187\
    00024142";

pub(crate) fn text_clip_fixture(mode: u8, code: &str) -> ByteStore {
    text_font_fixture(
        &format!("BT /F1 12 Tf {mode} Tr 1 0 0 1 10 20 Tm <{code}> Tj ET 0 0 5 5 re f"),
        DISAGREEING_CFF,
    )
}

pub(crate) fn content_only_fixture(content: &[u8]) -> ByteStore {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes(),
    );
    bytes.extend_from_slice(content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Nowhere /FirstChar 65 /LastChar 66 /Widths [500 500] >>\nendobj\n");
    let size = offsets.len() + 1;
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    ByteStore::new(SourceId::new(98), Arc::<[u8]>::from(bytes))
}

pub(crate) fn text_font_fixture(content: &str, cff: &str) -> ByteStore {
    let content = content.as_bytes().to_vec();
    let program = hex_fixture(cff);
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
    bytes.extend_from_slice(content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(&content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Test /FirstChar 65 /LastChar 66 /Widths [500 500] /FontDescriptor 6 0 R >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"6 0 obj\n<< /Type /FontDescriptor /FontName /Test /Flags 4 /FontFile3 7 0 R >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"7 0 obj\n<< /Subtype /Type1C /Length ");
    bytes.extend_from_slice(program.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(&program);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    let size = offsets.len() + 1;
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    ByteStore::new(SourceId::new(97), Arc::<[u8]>::from(bytes))
}

pub(crate) fn interpret_fixture(source: &ByteStore) -> Result<PaintGraph, InterpretError> {
    let page =
        load_page_program_strict(source, 0, PageContentLimits::default()).expect("fixture page");
    let operations = parse_operations_strict(&page.streams[0].bytes, ContentLimits::default())
        .expect("fixture operations");
    interpret_stream_sequence_with_resources(
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
}

pub(crate) fn text_state_fixture() -> ByteStore {
    let content = b"1 Tc 2 Tw 80 Tz 14 TL /F1 12 Tf 2 Tr 3 Ts BT 1 0 0 1 10 20 Tm 5 -6 TD T* [<2022> 100 <21>] TJ ET";
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /FirstChar 32 /LastChar 34 /Widths [250 500 750] >> >> >> >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
    bytes.extend_from_slice(content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(b"trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n");
    bytes.extend_from_slice(xref.to_string().as_bytes());
    bytes.extend_from_slice(b"\n%%EOF\n");
    ByteStore::new(SourceId::new(63), Arc::<[u8]>::from(bytes))
}

pub(crate) fn type0_text_fixture() -> ByteStore {
    let content = b"BT 1 Tc 2 Tw /F1 10 Tf <20218102> Tj ET";
    let cmap = b"2 begincodespacerange <00> <7f> <8100> <81ff> endcodespacerange 1 begincidchar <20> 3 endcidchar 1 begincidrange <8100> <8102> 40 endcidrange";
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 << /Type /Font /Subtype /Type0 /Encoding 6 0 R /DescendantFonts [5 0 R] >> >> >> >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
    bytes.extend_from_slice(content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"5 0 obj\n<< /Type /Font /Subtype /CIDFontType2 /DW 900 /W [3 [250] 40 42 700] >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"6 0 obj\n<< /Length ");
    bytes.extend_from_slice(cmap.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(cmap);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 7\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(b"trailer\n<< /Size 7 /Root 1 0 R >>\nstartxref\n");
    bytes.extend_from_slice(xref.to_string().as_bytes());
    bytes.extend_from_slice(b"\n%%EOF\n");
    ByteStore::new(SourceId::new(64), Arc::<[u8]>::from(bytes))
}

pub(crate) fn interpret(
    bytes: &[u8],
) -> Result<(ByteStore, crate::PaintGraph), crate::InterpretError> {
    let source = ByteStore::new(
        SourceId::derived(SourceId::new(9), 1),
        Arc::<[u8]>::from(bytes),
    );
    let operations = parse_operations_strict(&source, ContentLimits::default())
        .expect("content syntax should parse");
    let graph = interpret_operations(
        &source,
        &operations,
        &PaintContext::page_stream(
            pdf_syntax::Reference::new(3, 0),
            pdf_syntax::Reference::new(7, 0),
        ),
        PaintLimits::default(),
    )?;
    Ok((source, graph))
}

pub(crate) fn path(atom: &crate::PaintAtom) -> &crate::PathPaint {
    match &atom.kind {
        PaintAtomKind::Path(paint) => paint,
        PaintAtomKind::Text(_)
        | PaintAtomKind::TransparencyGroup(_)
        | PaintAtomKind::Shading(_)
        | PaintAtomKind::Image(_) => {
            panic!("expected path atom")
        }
    }
}

pub(crate) const DISAGREEING_CFF: &str = "0100040100010101055465737400010101131d00000030111d0000004c0f1d0000\
    0051100001010106616c706861000000030101020c160e8b8b158c8b058b8c050e8b8b158c8b058b8c050e00002201\
    87000141";

pub(crate) fn marked_content_fixture(properties: &[u8], content: &[u8]) -> ByteStore {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Properties << ",
    );
    bytes.extend_from_slice(properties);
    bytes.extend_from_slice(b" >> >> >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
    bytes.extend_from_slice(content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(b"trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n");
    bytes.extend_from_slice(xref.to_string().as_bytes());
    bytes.extend_from_slice(b"\n%%EOF\n");
    ByteStore::new(SourceId::new(73), Arc::<[u8]>::from(bytes))
}

pub(crate) fn optional_content_fixture(
    configuration: &[u8],
    properties: &[u8],
    content: &[u8],
    form_entries: &[u8],
) -> ByteStore {
    optional_content_fixture_with_group(configuration, properties, content, form_entries, b"")
}

pub(crate) fn optional_content_fixture_with_group(
    configuration: &[u8],
    properties: &[u8],
    content: &[u8],
    form_entries: &[u8],
    group_entries: &[u8],
) -> ByteStore {
    let form_content = b"0 0 4 4 re f";
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R 6 0 R] /D << ",
    );
    bytes.extend_from_slice(configuration);
    bytes.extend_from_slice(b" >> >> >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Fm1 7 0 R >> /Properties << ",
    );
    bytes.extend_from_slice(properties);
    bytes.extend_from_slice(b" >> >> >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
    bytes.extend_from_slice(content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"5 0 obj\n<< /Type /OCG /Name (A) ");
    bytes.extend_from_slice(group_entries);
    bytes.extend_from_slice(b" >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"6 0 obj\n<< /Type /OCG /Name (B) >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"7 0 obj\n<< /Type /XObject /Subtype /Form /BBox [0 0 4 4] /Resources << >> ",
    );
    bytes.extend_from_slice(form_entries);
    bytes.extend_from_slice(b" /Length ");
    bytes.extend_from_slice(form_content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(form_content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    let size = offsets.len() + 1;
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n").as_bytes(),
    );
    bytes.extend_from_slice(xref.to_string().as_bytes());
    bytes.extend_from_slice(b"\n%%EOF\n");
    ByteStore::new(SourceId::new(75), Arc::<[u8]>::from(bytes))
}

pub(crate) fn default_space_fixture(
    page_color_spaces: &[u8],
    image_color_space: &[u8],
    samples: &[u8],
) -> ByteStore {
    let content = b"40 0 0 40 0 0 cm /Im1 Do";
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 40 40] /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Im1 5 0 R >> /ColorSpace << ",
    );
    bytes.extend_from_slice(page_color_spaces);
    bytes.extend_from_slice(b" >> >> >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
    bytes.extend_from_slice(content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"5 0 obj\n<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace ",
    );
    bytes.extend_from_slice(image_color_space);
    bytes.extend_from_slice(b" /Length ");
    bytes.extend_from_slice(samples.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(samples);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    let size = offsets.len() + 1;
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n").as_bytes(),
    );
    bytes.extend_from_slice(xref.to_string().as_bytes());
    bytes.extend_from_slice(b"\n%%EOF\n");
    ByteStore::new(SourceId::new(73), Arc::<[u8]>::from(bytes))
}

pub(crate) fn default_space_content_fixture(page_color_spaces: &[u8], content: &[u8]) -> ByteStore {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 40 40] /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    offsets.push(bytes.len());
    bytes.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ColorSpace << ",
    );
    bytes.extend_from_slice(page_color_spaces);
    bytes.extend_from_slice(b" >> >> >>\nendobj\n");
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
    bytes.extend_from_slice(content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    let size = offsets.len() + 1;
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n").as_bytes(),
    );
    bytes.extend_from_slice(xref.to_string().as_bytes());
    bytes.extend_from_slice(b"\n%%EOF\n");
    ByteStore::new(SourceId::new(74), Arc::<[u8]>::from(bytes))
}
