use std::fmt::Write as _;
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_content::{ContentLimits, PageContentLimits, parse_operation_sequence_strict};
use pdf_paint::{
    ColorSpace, PaintAtomKind, PaintGraph, PaintLimits, PaintStream,
    interpret_stream_sequence_with_resources,
};
use pdf_security::AuthenticatedSecurity;
use pdf_syntax::{
    Reference, ResolveLimits, RevisionIndex, XrefLimits, parse_revision_chain_strict,
};

enum Part {
    Raw(&'static str),
    Text(&'static [u8]),
}

struct Written {
    parts: Vec<Part>,
    stream: Option<&'static [u8]>,
}

fn raw(text: &'static str) -> Written {
    Written {
        parts: vec![Part::Raw(text)],
        stream: None,
    }
}

type Encrypt<'a> = dyn Fn(Reference, &[u8], bool) -> Vec<u8> + 'a;

struct Lock<'a> {
    dictionary: &'a [u8],
    identifier: &'a str,
    encrypt: &'a Encrypt<'a>,
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn write_document(objects: &[Written], lock: Option<&Lock<'_>>) -> ByteStore {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        let number = u32::try_from(index + 1).expect("few objects");
        let at = Reference::new(number, 0);
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        for part in &object.parts {
            match part {
                Part::Raw(text) => bytes.extend_from_slice(text.as_bytes()),
                Part::Text(text) => {
                    let written = match lock {
                        Some(lock) => (lock.encrypt)(at, text, false),
                        None => text.to_vec(),
                    };
                    bytes.extend_from_slice(format!("<{}>", hex(&written)).as_bytes());
                }
            }
        }
        if let Some(data) = object.stream {
            let written = match lock {
                Some(lock) => (lock.encrypt)(at, data, true),
                None => data.to_vec(),
            };
            bytes.extend_from_slice(format!(" /Length {} >>\nstream\n", written.len()).as_bytes());
            bytes.extend_from_slice(&written);
            bytes.extend_from_slice(b"\nendstream");
        }
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let mut trailer = format!("/Size {} /Root 1 0 R", objects.len() + 1);
    if let Some(lock) = lock {
        let number = objects.len() + 1;
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        bytes.extend_from_slice(lock.dictionary);
        bytes.extend_from_slice(b"\nendobj\n");
        trailer = format!(
            "/Size {} /Root 1 0 R /Encrypt {number} 0 R /ID [<{id}> <{id}>]",
            number + 1,
            id = lock.identifier
        );
    }
    let xref = bytes.len();
    bytes.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
    );
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< {trailer} >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    ByteStore::new(SourceId::new(77), Arc::<[u8]>::from(bytes))
}

const IN_PAGE_RESOURCES: &[u8] = b"\xff\x00\x00\x00\xff\x00";
const IN_ITS_OWN_OBJECT: &[u8] = b"\x00\x00\xff\xff\xff\x00";
const IN_AN_IMAGE: &[u8] = b"\x10\x20\x30\x40\x50\x60";
const INLINE: &[u8] = b"\xff\x00\xff\x00\xff\xff";

fn page_objects() -> Vec<Written> {
    const CONTENT: &[u8] = b"/Pal cs 1 sc 0 0 10 10 re f\n\
/Ref cs 0 sc 10 0 10 10 re f\n\
q 20 0 0 10 20 0 cm /Im Do Q\n\
q 20 0 0 10 40 0 cm BI /W 2 /H 1 /BPC 8 /CS [/I /RGB 1 <ff00ff00ffff>] ID \x00\x01 EI Q\n";
    const CMAP: &[u8] = b"1 begincodespacerange <00> <ff> endcodespacerange \
1 begincidrange <00> <ff> 0 endcidrange";
    vec![
        raw("<< /Type /Catalog /Pages 2 0 R >>"),
        raw("<< /Type /Pages /MediaBox [0 0 100 20] /Kids [3 0 R] /Count 1 >>"),
        Written {
            parts: vec![
                Part::Raw(
                    "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << \
                     /ColorSpace << /Ref 6 0 R /Pal [/Indexed /DeviceRGB 1 ",
                ),
                Part::Text(IN_PAGE_RESOURCES),
                Part::Raw("] >> /XObject << /Im 5 0 R >> /Font << /F1 7 0 R /F3 10 0 R >> >> >>"),
            ],
            stream: None,
        },
        Written {
            parts: vec![Part::Raw("<<")],
            stream: Some(CONTENT),
        },
        Written {
            parts: vec![
                Part::Raw(
                    "<< /Type /XObject /Subtype /Image /Width 2 /Height 1 /BitsPerComponent 8 \
                     /ColorSpace [/Indexed /DeviceRGB 1 ",
                ),
                Part::Text(IN_AN_IMAGE),
                Part::Raw("]"),
            ],
            stream: Some(b"\x00\x01"),
        },
        Written {
            parts: vec![
                Part::Raw("[/Indexed /DeviceRGB 1 "),
                Part::Text(IN_ITS_OWN_OBJECT),
                Part::Raw("]"),
            ],
            stream: None,
        },
        raw(
            "<< /Type /Font /Subtype /Type0 /BaseFont /Unembedded /Encoding 9 0 R \
             /DescendantFonts [8 0 R] >>",
        ),
        Written {
            parts: vec![
                Part::Raw(
                    "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Unembedded \
                     /CIDSystemInfo << /Registry ",
                ),
                Part::Text(b"Adobe"),
                Part::Raw(" /Ordering "),
                Part::Text(b"Japan1"),
                Part::Raw(" /Supplement 6 >> /DW 1000 >>"),
            ],
            stream: None,
        },
        Written {
            parts: vec![Part::Raw("<< /Type /CMap /CMapName /Embedded")],
            stream: Some(CMAP),
        },
        raw("<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] \
             /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << >> /Encoding << /Differences [] >> \
             /FirstChar 32 /LastChar 32 /Widths [250] /FontDescriptor 11 0 R >>"),
        Written {
            parts: vec![
                Part::Raw(
                    "<< /Type /FontDescriptor /FontName /BAAAAA+NotoSansThai-Regular /Flags 4 \
                     /FontFamily ",
                ),
                Part::Text(b"Noto Sans Thai"),
                Part::Raw(" >>"),
            ],
            stream: None,
        },
    ]
}

fn three_copies() -> [(&'static str, ByteStore); 3] {
    const IDENTIFIER: &str = "000102030405060708090a0b0c0d0e0f";
    let objects = page_objects();
    let open = write_document(&objects, None);
    let rc4_dictionary = b"<< /Filter /Standard /V 1 /R 2 /Length 40 /P -4 \
/O <2055c756c72e1ad702608e8196acad447ad32d17cff583235f6dd15fed7dab67> \
/U <b6271bb74f4fd4bf931172dcde8682912edc27b84ad0dc7cb83dc19fb91734d5> >>";
    let unchanged = |_: Reference, bytes: &[u8], _: bool| bytes.to_vec();
    let skeleton = write_document(
        &objects,
        Some(&Lock {
            dictionary: rc4_dictionary,
            identifier: IDENTIFIER,
            encrypt: &unchanged,
        }),
    );
    let rc4 = handler(&skeleton);
    let rc4_encrypt = |at: Reference, bytes: &[u8], stream: bool| encrypt(&rc4, at, bytes, stream);
    let rc4_copy = write_document(
        &objects,
        Some(&Lock {
            dictionary: rc4_dictionary,
            identifier: IDENTIFIER,
            encrypt: &rc4_encrypt,
        }),
    );

    let made = pdf_security::make_protection(&pdf_security::Wanted {
        user: Vec::new(),
        owner: b"owner".to_vec(),
        allowed: pdf_security::Allowed::default(),
    })
    .expect("AES-256 protection is made");
    let aes_encrypt =
        |at: Reference, bytes: &[u8], stream: bool| encrypt(&made.security, at, bytes, stream);
    let aes_copy = write_document(
        &objects,
        Some(&Lock {
            dictionary: &made.dictionary,
            identifier: IDENTIFIER,
            encrypt: &aes_encrypt,
        }),
    );
    [("open", open), ("RC4", rc4_copy), ("AES-256", aes_copy)]
}

fn handler(source: &ByteStore) -> AuthenticatedSecurity {
    let chain = parse_revision_chain_strict(source, XrefLimits::default()).expect("revisions");
    let index = RevisionIndex::from_chain(&chain).expect("index");
    pdf_security::authenticate_standard_password(
        source,
        &chain,
        &index,
        b"",
        ResolveLimits::default(),
    )
    .expect("the empty user password opens the fixture")
}

fn encrypt(security: &AuthenticatedSecurity, at: Reference, bytes: &[u8], stream: bool) -> Vec<u8> {
    if stream {
        security.encrypt_stream(at, bytes)
    } else {
        security.encrypt_string(at, bytes)
    }
    .expect("the fixture encrypts")
}

fn paint(source: &ByteStore) -> (pdf_content::PageProgram, PaintGraph) {
    let program =
        pdf_content::load_page_program_with_password(source, 0, PageContentLimits::default(), b"")
            .expect("the page loads with the empty user password");
    let sources: Vec<&ByteStore> = program.streams.iter().map(|stream| &stream.bytes).collect();
    let operations = parse_operation_sequence_strict(&sources, ContentLimits::default())
        .expect("the content parses");
    let streams: Vec<_> = program
        .streams
        .iter()
        .zip(&sources)
        .zip(&operations)
        .map(|((stream, source), operations)| PaintStream {
            source,
            reference: stream.reference,
            operations,
        })
        .collect();
    let graph = interpret_stream_sequence_with_resources(
        &streams,
        program.page,
        &[],
        &program.resources,
        PaintLimits::default(),
    )
    .expect("the page interprets");
    (program, graph)
}

fn palettes(graph: &PaintGraph) -> Vec<Vec<u8>> {
    graph
        .atoms
        .iter()
        .filter_map(|atom| match &atom.kind {
            PaintAtomKind::Path(path) => Some(path.state.fill_color_space.value.clone()),
            PaintAtomKind::Image(image) => {
                image.color_space.as_ref().map(|space| space.value.clone())
            }
            _ => None,
        })
        .filter_map(|space| match space {
            ColorSpace::Indexed(indexed) => Some(indexed.lookup.value.to_vec()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_protected_page_paints_through_the_palettes_it_was_written_with() {
    let expected = vec![
        IN_PAGE_RESOURCES.to_vec(),
        IN_ITS_OWN_OBJECT.to_vec(),
        IN_AN_IMAGE.to_vec(),
        INLINE.to_vec(),
    ];
    for (name, source) in three_copies() {
        let (_, graph) = paint(&source);
        assert_eq!(
            palettes(&graph),
            expected,
            "{name}: the palettes painted through"
        );
        let written = source.as_bytes();
        let hidden = !written
            .windows(IN_PAGE_RESOURCES.len() * 2)
            .any(|window| window == hex(IN_PAGE_RESOURCES).as_bytes());
        assert_eq!(
            hidden,
            name != "open",
            "{name}: the palette's bytes as written"
        );
    }
}

#[test]
fn a_protected_fonts_cmap_and_names_are_read_as_written() {
    for (name, source) in three_copies() {
        let (program, _) = paint(&source);
        let composite = program.resources.font(b"/F1").expect("the composite font");
        assert!(
            matches!(composite.font(), Ok(pdf_content::Font::Composite(_))),
            "{name}: the embedded CMap parses"
        );
        let request = composite
            .font_request()
            .expect("the request reads")
            .expect("an unembedded font asks for a face");
        assert_eq!(request.registry.as_deref(), Some("Adobe"), "{name}");
        assert_eq!(request.ordering.as_deref(), Some("Japan1"), "{name}");

        let type3 = program.resources.font(b"/F3").expect("the Type 3 font");
        let face = type3
            .face_request()
            .expect("the request reads")
            .expect("a Type 3 font asks for a face to type in");
        assert_eq!(face.family, "Noto Sans Thai", "{name}: the family name");
    }
}
