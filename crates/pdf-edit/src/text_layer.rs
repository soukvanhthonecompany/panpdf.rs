use std::collections::BTreeMap;

use pdf_bytes::ByteStore;
use pdf_paint::{Matrix, PaintAtomKind, PaintGraph, Point, TextRenderingMode};
use pdf_syntax::{Object, ObjectKind, Reference};

use crate::new_font::{Body, add_font_resource, entry, resolve};
use crate::plan::{Capability, Effect, MovedRun, Plan, PlannedBody, PlannedWrite, SourceAnchor};
use crate::spike_move_text::{PlannerPage, SpikeError, interpret_bytes_of};

pub mod why {
    pub const NO_WORDS: &str = "there are no recognised words to write";
    pub const EMPTY_WORD: &str = "a recognised word has no text";
    pub const CONTROL: &str = "a recognised word holds a control character";
    pub const OUTSIDE_BMP: &str =
        "a recognised word holds a character outside the Basic Multilingual Plane";
    pub const FRAME: &str = "a recognised word's box must be four numbers with width and height";
}

const FACE_NAME: &str = "GlyphLess";

#[derive(Clone, Debug, PartialEq)]
pub struct LayerWord {
    pub text: String,
    pub frame: [f64; 4],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextLayer {
    pub words: Vec<LayerWord>,
}

struct Placed {
    codes: Vec<u16>,
    matrix: Matrix,
    pens: Vec<Point>,
}

pub(crate) fn plan_text_layer(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    layer: &TextLayer,
    share_from: Option<usize>,
) -> Result<Plan, SpikeError> {
    let (_, turn) = crate::stamp::shown(&page.program.geometry)?;
    let placed = placed(layer, turn)?;

    let mut next = crate::block_rewrite::next_object_number(source)?;
    let mut writes = Vec::new();
    let found = layer_descendant(source, &page.program.resources).or_else(|| {
        let other = share_from?;
        let program = pdf_content::load_page_program_with_password(
            source,
            other,
            pdf_content::PageContentLimits::default(),
            b"",
        )
        .ok()?;
        layer_descendant(source, &program.resources)
    });
    let descendant = if let Some(descendant) = found {
        descendant
    } else {
        let objects = [0, 1, 2, 3].map(|offset| Reference::new(next + offset, 0));
        next += 4;
        writes.extend(descendant_writes(objects)?);
        objects[0]
    };
    let font = Reference::new(next, 0);
    let unicode = Reference::new(next + 1, 0);
    let mut meaning = BTreeMap::new();
    for code in placed.iter().flat_map(|word| &word.codes) {
        meaning
            .entry(*code)
            .or_insert_with(|| String::from_utf16_lossy(&[*code]));
    }
    writes.push(direct(
        font,
        format!(
            "<< /Type /Font /Subtype /Type0 /BaseFont /{FACE_NAME} /Encoding /Identity-H \
             /DescendantFonts [{} 0 R] /ToUnicode {} 0 R >>",
            descendant.object_number(),
            unicode.object_number()
        ),
    ));
    writes.push(PlannedWrite {
        reference: unicode,
        body: PlannedBody::NewStream {
            dictionary: Vec::new(),
            decoded: crate::new_font::to_unicode(&meaning).into_bytes(),
        },
    });
    let (name, holder) = add_font_resource(source, page.program.page, font)?;
    writes.push(holder);

    let document = crate::block_rewrite::commit_writes(source, &writes, page.restrictions)?;
    let carrying = crate::spike_move_text::read_page(&document, page_index, b"", page.fonts)?;
    let stream = carrying
        .program
        .streams
        .len()
        .checked_sub(1)
        .ok_or_else(|| refused("a page with no content stream cannot be written into"))?;
    let decoded = carrying.program.streams[stream].bytes.as_bytes();
    let first = candidate(decoded, &placed, &name, Matrix::IDENTITY);
    let mut graph = interpret_bytes_of(&carrying.program, stream, &first, page.fonts)?;
    let ctm = standing_ctm(&carrying.graph, &graph)?;
    let mut bytes = first;
    if ctm != Matrix::IDENTITY {
        let inverse = ctm
            .inverse()
            .ok_or_else(|| refused("this page leaves a transform text cannot be placed through"))?;
        bytes = candidate(decoded, &placed, &name, inverse);
        graph = interpret_bytes_of(&carrying.program, stream, &bytes, page.fonts)?;
    }
    prove_layer(&carrying.graph, &graph, &placed)?;

    writes.push(PlannedWrite {
        reference: carrying.program.streams[stream].reference,
        body: PlannedBody::ReplacedStream { decoded: bytes },
    });
    let ordinal = carrying.graph.atoms.len();
    let atom = graph
        .atoms
        .get(ordinal)
        .ok_or_else(|| refused("the text written does not paint"))?;
    Ok(Plan::new(
        Capability::Exact,
        writes,
        Effect {
            page_index,
            moved: vec![MovedRun {
                anchor: SourceAnchor::of(&atom.id),
                atom_ordinal: ordinal,
                original_matrix: Matrix::IDENTITY,
            }],
            target_stream: carrying.program.streams[stream].reference,
            declared_region: Some(region(&placed)),
        },
    ))
}

fn refused(reason: &'static str) -> SpikeError {
    SpikeError::BlockRewriteUnsupported(reason)
}

fn direct(reference: Reference, body: String) -> PlannedWrite {
    PlannedWrite {
        reference,
        body: PlannedBody::Direct {
            body: body.into_bytes(),
        },
    }
}

fn placed(layer: &TextLayer, turn: Matrix) -> Result<Vec<Placed>, SpikeError> {
    if layer.words.is_empty() {
        return Err(refused(why::NO_WORDS));
    }
    layer
        .words
        .iter()
        .map(|word| {
            if word.text.trim().is_empty() {
                return Err(refused(why::EMPTY_WORD));
            }
            if word.text.chars().any(|letter| {
                letter.is_control() || matches!(letter, '\u{feff}' | '\u{fffe}' | '\u{ffff}')
            }) {
                return Err(refused(why::CONTROL));
            }
            if word.text.chars().any(|letter| letter.len_utf16() != 1) {
                return Err(refused(why::OUTSIDE_BMP));
            }
            let [x0, y0, x1, y1] = word.frame;
            if !word.frame.iter().all(|edge| edge.is_finite()) || x1 <= x0 || y1 <= y0 {
                return Err(refused(why::FRAME));
            }
            let codes: Vec<u16> = word.text.encode_utf16().collect();
            let count = f64::from(u32::try_from(codes.len()).map_err(|_| refused(why::FRAME))?);
            let advance = f64::from(pdf_content::GLYPHLESS_ADVANCE) / 1000.0;
            let step = (x1 - x0) / count;
            let shown = Matrix {
                a: step / advance,
                b: 0.0,
                c: 0.0,
                d: y1 - y0,
                e: x0,
                f: y0,
            };
            let pens = (0..codes.len())
                .map(|index| {
                    let along = f64::from(u32::try_from(index).unwrap_or(u32::MAX));
                    turn.transform(Point {
                        x: x0 + step * along,
                        y: y0,
                    })
                })
                .collect();
            Ok(Placed {
                codes,
                matrix: turn.multiply(shown),
                pens,
            })
        })
        .collect()
}

fn candidate(decoded: &[u8], words: &[Placed], name: &str, inverse: Matrix) -> Vec<u8> {
    let mut out = Vec::with_capacity(decoded.len() + words.len() * 64);
    out.extend_from_slice(decoded);
    out.extend_from_slice(format!("\nq BT 3 Tr 0 Tc 0 Tw 100 Tz 0 Ts /{name} 1 Tf\n").as_bytes());
    for word in words {
        let set = inverse.multiply(word.matrix);
        out.extend_from_slice(
            format!(
                "{} {} {} {} {} {} Tm <",
                set.a, set.b, set.c, set.d, set.e, set.f
            )
            .as_bytes(),
        );
        for code in &word.codes {
            out.extend_from_slice(format!("{code:04X}").as_bytes());
        }
        out.extend_from_slice(b"> Tj\n");
    }
    out.extend_from_slice(b"ET Q\n");
    out
}

fn standing_ctm(before: &PaintGraph, after: &PaintGraph) -> Result<Matrix, SpikeError> {
    match after.atoms.get(before.atoms.len()).map(|atom| &atom.kind) {
        Some(PaintAtomKind::Text(text)) => Ok(text.state.ctm.value),
        _ => Err(refused("the text written does not paint")),
    }
}

fn prove_layer(
    before: &PaintGraph,
    after: &PaintGraph,
    words: &[Placed],
) -> Result<(), SpikeError> {
    if after.atoms.len() != before.atoms.len() + words.len() {
        return Err(SpikeError::MoveNotIsolated);
    }
    crate::new_text::prove_untouched(before, after)?;
    for (word, atom) in words.iter().zip(&after.atoms[before.atoms.len()..]) {
        let PaintAtomKind::Text(text) = &atom.kind else {
            return Err(SpikeError::MoveNotIsolated);
        };
        if text.state.text.rendering_mode.value != TextRenderingMode::Invisible
            || text.glyphs.len() != word.codes.len()
        {
            return Err(SpikeError::MoveNotIsolated);
        }
        for ((glyph, wanted), code) in text.glyphs.iter().zip(&word.pens).zip(&word.codes) {
            let at = text
                .state
                .ctm
                .value
                .multiply(glyph.matrix)
                .transform(Point { x: 0.0, y: 0.0 });
            if (at.x - wanted.x).abs() > crate::block_move::PLACEMENT_TOLERANCE
                || (at.y - wanted.y).abs() > crate::block_move::PLACEMENT_TOLERANCE
            {
                return Err(SpikeError::MoveNotIsolated);
            }
            let says = text.text.text_of(pdf_content::Code {
                value: glyph.code.value,
                byte_len: glyph.code.bytes.len(),
            });
            if glyph.code.value != u32::from(*code)
                || says.map(|meaning| meaning.text.as_str())
                    != Some(String::from_utf16_lossy(&[*code]).as_str())
            {
                return Err(SpikeError::MoveNotIsolated);
            }
        }
    }
    Ok(())
}

fn region(words: &[Placed]) -> [f64; 4] {
    let mut out = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for pen in words.iter().flat_map(|word| &word.pens) {
        out = [
            out[0].min(pen.x),
            out[1].min(pen.y),
            out[2].max(pen.x),
            out[3].max(pen.y),
        ];
    }
    out
}

fn cid_map() -> Vec<u8> {
    let mut map = vec![0_u8; 2];
    for _ in 1..=u16::MAX {
        map.extend_from_slice(&1_u16.to_be_bytes());
    }
    map
}

fn descendant_body(descriptor: Reference, map: Reference) -> String {
    format!(
        "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /{FACE_NAME} \
         /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
         /FontDescriptor {} 0 R /DW {} /CIDToGIDMap {} 0 R >>",
        descriptor.object_number(),
        pdf_content::GLYPHLESS_ADVANCE,
        map.object_number()
    )
}

fn descriptor_body(program: Reference) -> String {
    format!(
        "<< /Type /FontDescriptor /FontName /{FACE_NAME} /Flags 4 /FontBBox [0 0 {} 1000] \
         /ItalicAngle 0 /Ascent 1000 /Descent 0 /CapHeight 1000 /StemV 80 /FontFile2 {} 0 R >>",
        pdf_content::GLYPHLESS_ADVANCE,
        program.object_number()
    )
}

fn descendant_writes(
    [descendant, descriptor, program, map]: [Reference; 4],
) -> Result<Vec<PlannedWrite>, SpikeError> {
    let face = pdf_content::glyphless().map_err(|_| refused("the empty face cannot be written"))?;
    Ok(vec![
        direct(descendant, descendant_body(descriptor, map)),
        direct(descriptor, descriptor_body(program)),
        PlannedWrite {
            reference: program,
            body: PlannedBody::NewStream {
                dictionary: format!("/Length1 {}", face.len()).into_bytes(),
                decoded: face,
            },
        },
        PlannedWrite {
            reference: map,
            body: PlannedBody::NewStream {
                dictionary: b"/Filter /FlateDecode".to_vec(),
                decoded: pdf_syntax::deflate_zlib(&cid_map()),
            },
        },
    ])
}

fn layer_descendant(
    source: &ByteStore,
    resources: &pdf_content::PageResources,
) -> Option<Reference> {
    resources.fonts().iter().find_map(|named| {
        let top = resolve(source, named.reference()?).ok()?;
        if !pdf_syntax::decode_name(&top.source, entry(&top, &top.value, b"/BaseFont")?)
            .is_ok_and(|base| base == format!("/{FACE_NAME}").as_bytes())
        {
            return None;
        }
        let descendant = only_reference(&top, &top.value, b"/DescendantFonts")?;
        is_layer_descendant(source, descendant).then_some(descendant)
    })
}

fn is_layer_descendant(source: &ByteStore, descendant: Reference) -> bool {
    let check = || -> Option<bool> {
        let cid = resolve(source, descendant).ok()?;
        let descriptor = only_reference(&cid, &cid.value, b"/FontDescriptor")?;
        let map = only_reference(&cid, &cid.value, b"/CIDToGIDMap")?;
        let described = resolve(source, descriptor).ok()?;
        let program = only_reference(&described, &described.value, b"/FontFile2")?;
        let face = pdf_content::glyphless().ok()?;
        Some(
            cid.bytes == descendant_body(descriptor, map).as_bytes()
                && described.bytes == descriptor_body(program).as_bytes()
                && crate::previous::decoded_stream(source, program, b"").ok()? == face
                && crate::previous::decoded_stream(source, map, b"").ok()? == cid_map(),
        )
    };
    check().unwrap_or(false)
}

fn only_reference(body: &Body, dictionary: &Object, key: &[u8]) -> Option<Reference> {
    match entry(body, dictionary, key)?.kind() {
        ObjectKind::Reference(reference) => Some(*reference),
        ObjectKind::Array(values) => match values.as_slice() {
            [only] => match only.kind() {
                ObjectKind::Reference(reference) => Some(*reference),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use pdf_bytes::{ByteStore, SourceId};
    use pdf_paint::{PaintAtomKind, TextRenderingMode};

    use super::{LayerWord, TextLayer, why};
    use crate::history::History;
    use crate::plan::Command;
    use crate::spike_move_text::{SpikeError, plan_command_under, read_page};

    fn document(pages: usize, page: &str, content: &str) -> ByteStore {
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            format!(
                "<< /Type /Pages /MediaBox [0 0 200 300] /Kids [{}] /Count {pages} >>",
                (0..pages)
                    .map(|index| format!("{} 0 R", 3 + 2 * index))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        ];
        for index in 0..pages {
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /ProcSet [/PDF] >> {page} >>",
                4 + 2 * index
            ));
            objects.push(format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ));
        }
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
        }
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        ByteStore::new(SourceId::new(1), bytes)
    }

    const INK: &str = "0 0 0 rg 10 10 20 20 re f";

    fn word(text: &str, frame: [f64; 4]) -> LayerWord {
        LayerWord {
            text: text.to_owned(),
            frame,
        }
    }

    fn line() -> TextLayer {
        TextLayer {
            words: vec![
                word("Hello ", [20.0, 250.0, 80.0, 262.0]),
                word("ສະບາຍດີ", [80.0, 250.0, 150.0, 262.0]),
            ],
        }
    }

    fn written(source: ByteStore, layers: &[(usize, TextLayer)]) -> Result<History, SpikeError> {
        let first = layers.first().map_or(0, |(page, _)| *page);
        let commands: Vec<Command> = layers
            .iter()
            .map(|(page_index, layer)| Command::TextLayer {
                page_index: *page_index,
                layer: layer.clone(),
                share_from: (*page_index != first).then_some(first),
            })
            .collect();
        let mut history = History::new(source, b"");
        history.apply_together_with(commands.len(), |source, at| {
            plan_command_under(
                source,
                &commands[at],
                b"",
                (None, crate::Restrictions::Respect),
            )
        })?;
        Ok(history)
    }

    type Run = (bool, String, Vec<(f64, f64)>);

    fn runs(source: &ByteStore, page: usize) -> Vec<Run> {
        let reading = read_page(source, page, b"", None).expect("the page reads");
        reading
            .graph
            .atoms
            .iter()
            .filter_map(|atom| match &atom.kind {
                PaintAtomKind::Text(text) => Some(text),
                _ => None,
            })
            .map(|text| {
                let says = text
                    .glyphs
                    .iter()
                    .map(|glyph| {
                        text.text
                            .text_of(pdf_content::Code {
                                value: glyph.code.value,
                                byte_len: glyph.code.bytes.len(),
                            })
                            .map(|meaning| meaning.text.clone())
                            .unwrap_or_default()
                    })
                    .collect();
                let pens = text
                    .glyphs
                    .iter()
                    .map(|glyph| {
                        let at = text
                            .state
                            .ctm
                            .value
                            .multiply(glyph.matrix)
                            .transform(pdf_paint::Point { x: 0.0, y: 0.0 });
                        ((at.x * 1e6).round() / 1e6, (at.y * 1e6).round() / 1e6)
                    })
                    .collect();
                (
                    text.state.text.rendering_mode.value != TextRenderingMode::Invisible,
                    says,
                    pens,
                )
            })
            .collect()
    }

    fn fonts_of(source: &ByteStore, page: usize) -> Vec<pdf_syntax::Reference> {
        let reading = read_page(source, page, b"", None).expect("the page reads");
        reading
            .program
            .resources
            .fonts()
            .iter()
            .filter_map(pdf_content::ResourceEntry::reference)
            .collect()
    }

    fn descendant_of(source: &ByteStore, font: pdf_syntax::Reference) -> pdf_syntax::Reference {
        let top = crate::new_font::resolve(source, font).expect("the font reads");
        super::only_reference(&top, &top.value, b"/DescendantFonts").expect("one descendant")
    }

    #[test]
    fn recognised_words_are_found_and_not_seen() {
        let history = written(document(1, "", INK), &[(0, line())]).expect("the layer is written");
        let runs = runs(history.source(), 0);
        assert_eq!(runs.len(), 2);
        assert!(runs.iter().all(|(shows, ..)| !shows), "nothing is seen");
        assert_eq!(runs[0].1, "Hello ");
        assert_eq!(runs[1].1, "ສະບາຍດີ");
        assert_eq!(
            runs[0].2,
            vec![
                (20.0, 250.0),
                (30.0, 250.0),
                (40.0, 250.0),
                (50.0, 250.0),
                (60.0, 250.0),
                (70.0, 250.0)
            ]
        );
        assert_eq!(runs[1].2.len(), 7);
        assert_eq!(runs[1].2[0], (80.0, 250.0));
        assert_eq!(runs[1].2[6], (140.0, 250.0));
    }

    #[test]
    fn the_page_is_drawn_as_it_was() {
        let source = document(1, "", INK);
        let before = read_page(&source, 0, b"", None).expect("reads");
        let history = written(source, &[(0, line())]).expect("written");
        let after = read_page(history.source(), 0, b"", None).expect("reads");
        assert_eq!(
            pdf_paint::paint_signature(&before.graph.atoms[0].kind),
            pdf_paint::paint_signature(&after.graph.atoms[0].kind)
        );
        assert_eq!(after.graph.atoms.len(), 3);
    }

    #[test]
    fn a_page_that_leaves_a_transform_still_gets_its_words_where_they_are() {
        let source = document(1, "", "2 0 0 2 10 10 cm 0 0 0 rg 0 0 5 5 re f");
        let history = written(source, &[(0, line())]).expect("written");
        let runs = runs(history.source(), 0);
        assert_eq!(runs[0].2[0], (20.0, 250.0));
        assert_eq!(runs[0].2[5], (70.0, 250.0));
    }

    #[test]
    fn a_turned_page_gets_its_words_where_the_reader_saw_them() {
        let source = document(1, "/Rotate 90", INK);
        let layer = TextLayer {
            words: vec![word("Ab", [10.0, 180.0, 30.0, 190.0])],
        };
        let history = written(source, &[(0, layer)]).expect("written");
        let runs = runs(history.source(), 0);
        assert_eq!(runs[0].2, vec![(20.0, 10.0), (20.0, 20.0)]);
    }

    #[test]
    fn a_range_shares_one_face_and_undoes_as_one() {
        let source = document(2, "", INK);
        let other = TextLayer {
            words: vec![word("ทดสอบ", [20.0, 200.0, 70.0, 212.0])],
        };
        let mut history =
            written(source, &[(0, line()), (1, other)]).expect("the range is written");
        let first = fonts_of(history.source(), 0);
        let second = fonts_of(history.source(), 1);
        assert_eq!((first.len(), second.len()), (1, 1));
        assert_ne!(first, second, "each page has its own map of what it says");
        assert_eq!(
            descendant_of(history.source(), first[0]),
            descendant_of(history.source(), second[0]),
            "one face for the range"
        );
        assert_eq!(runs(history.source(), 1)[0].1, "ทดสอบ");
        assert!(history.undo().expect("undo walks"));
        assert!(fonts_of(history.source(), 0).is_empty());
        assert!(fonts_of(history.source(), 1).is_empty());
    }

    #[test]
    fn a_page_with_a_layer_uses_its_face_again() {
        let history = written(document(1, "", INK), &[(0, line())]).expect("written");
        let face = descendant_of(history.source(), fonts_of(history.source(), 0)[0]);
        let again = written(history.source().clone(), &[(0, line())]).expect("written again");
        let fonts = fonts_of(again.source(), 0);
        assert_eq!(fonts.len(), 2);
        assert!(
            fonts
                .iter()
                .all(|font| descendant_of(again.source(), *font) == face)
        );
    }

    #[test]
    fn a_lookalike_face_is_not_used() {
        let history = written(document(1, "", INK), &[(0, line())]).expect("written");
        let face = descendant_of(history.source(), fonts_of(history.source(), 0)[0]);
        let mut bytes = history.source().as_bytes().to_vec();
        let at = bytes
            .windows(7)
            .rposition(|window| window == b"/DW 500")
            .expect("the descendant says its width");
        bytes[at + 4..at + 7].copy_from_slice(b"600");
        let tampered = ByteStore::new(SourceId::new(2), bytes);
        let again = written(tampered, &[(0, line())]).expect("written");
        let fonts = fonts_of(again.source(), 0);
        assert_eq!(fonts.len(), 2);
        assert_eq!(descendant_of(again.source(), fonts[0]), face);
        assert_ne!(descendant_of(again.source(), fonts[1]), face);
    }

    fn refusal(layer: TextLayer) -> &'static str {
        match written(document(1, "", INK), &[(0, layer)]) {
            Err(SpikeError::BlockRewriteUnsupported(reason)) => reason,
            Err(other) => panic!("refused for another reason: {other:?}"),
            Ok(_) => panic!("written"),
        }
    }

    #[test]
    fn what_cannot_be_written_is_refused_by_name() {
        assert_eq!(refusal(TextLayer::default()), why::NO_WORDS);
        let one = |text: &str, frame| TextLayer {
            words: vec![word(text, frame)],
        };
        let frame = [10.0, 10.0, 50.0, 20.0];
        assert_eq!(refusal(one(" ", frame)), why::EMPTY_WORD);
        assert_eq!(refusal(one("a\u{7}", frame)), why::CONTROL);
        assert_eq!(refusal(one("a\u{1F600}", frame)), why::OUTSIDE_BMP);
        assert_eq!(refusal(one("a", [10.0, 10.0, 10.0, 20.0])), why::FRAME);
        assert_eq!(refusal(one("a", [10.0, 20.0, 50.0, 10.0])), why::FRAME);
        assert_eq!(refusal(one("a", [f64::NAN, 10.0, 50.0, 20.0])), why::FRAME);
    }
}
