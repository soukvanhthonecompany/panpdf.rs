use pdf_bytes::ByteStore;
use pdf_paint::Matrix;

use crate::new_text::NewText;
use crate::plan::Plan;
use crate::spike_move_text::{PlannerPage, SpikeError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Edge {
    Header,
    Footer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Side {
    Left,
    Centre,
    Right,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Spot {
    Along { edge: Edge, side: Side },
    Middle,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stamp {
    pub wording: String,
    pub spot: Spot,
    pub family: String,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    pub fill: Option<[f64; 3]>,
    pub opacity: f64,
    pub margin: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Facts {
    pub number: i64,
    pub count: i64,
    pub name: String,
    pub today: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Only {
    #[default]
    Every,
    Odd,
    Even,
}

pub mod why {
    pub const LONE_CLOSING_BRACE: &str = "a closing brace stands alone in the wording";
    pub const UNCLOSED_BRACE: &str = "a brace is opened and never closed in the wording";
    pub const UNKNOWN_TOKEN: &str = "the wording names something this does not know";
    pub const NO_PAGES: &str = "this document has no pages";
    pub const RANGE_BACKWARDS: &str = "a range that ends before it starts";
    pub const RANGE_NAMES_NOTHING: &str = "this names no page of the document";
    pub const RANGE_NOT_NUMBERS: &str = "a page range is written in numbers";
    pub const RANGE_PAGE_ZERO: &str = "a reader counts pages from one";
    pub const RANGE_PAST_THE_END: &str = "this document has no page of that number";
    pub const BAD_MARGIN: &str = "a margin is a distance in from the edge";
    pub const BAD_SIZE: &str = "text has a size";
    pub const NO_ROOM: &str = "this page has no room between its margins";
    pub const WIDER_THAN_THE_PAGE: &str = "this line is wider than the page between its margins";
    pub const ONE_LINE: &str = "a stamp is one line";
    pub const NO_TEXT: &str = "there is no text to put on the page";
    pub const PAGE_IS_TURNED: &str = "a page turned by other than a quarter turn cannot be stamped";

    pub const ALL: [&str; 16] = [
        LONE_CLOSING_BRACE,
        UNCLOSED_BRACE,
        UNKNOWN_TOKEN,
        NO_PAGES,
        RANGE_BACKWARDS,
        RANGE_NAMES_NOTHING,
        RANGE_NOT_NUMBERS,
        RANGE_PAGE_ZERO,
        RANGE_PAST_THE_END,
        BAD_MARGIN,
        BAD_SIZE,
        NO_ROOM,
        WIDER_THAN_THE_PAGE,
        ONE_LINE,
        NO_TEXT,
        PAGE_IS_TURNED,
    ];
}

fn refused(reason: &'static str) -> SpikeError {
    SpikeError::RetypeUnsupported(reason)
}

pub fn worded(wording: &str, facts: &Facts) -> Result<String, SpikeError> {
    let mut out = String::with_capacity(wording.len());
    let mut rest = wording;
    while let Some(at) = rest.find(['{', '}']) {
        out.push_str(&rest[..at]);
        let opener = rest.as_bytes()[at];
        rest = &rest[at + 1..];
        if rest.as_bytes().first() == Some(&opener) {
            out.push(char::from(opener));
            rest = &rest[1..];
            continue;
        }
        if opener == b'}' {
            return Err(refused(why::LONE_CLOSING_BRACE));
        }
        let end = rest.find('}').ok_or_else(|| refused(why::UNCLOSED_BRACE))?;
        match &rest[..end] {
            "page" => out.push_str(&facts.number.to_string()),
            "pages" => out.push_str(&facts.count.to_string()),
            "file" => out.push_str(&facts.name),
            "date" => out.push_str(&facts.today),
            _ => return Err(refused(why::UNKNOWN_TOKEN)),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

pub fn pages_of(spec: &str, count: usize, only: Only) -> Result<Vec<usize>, SpikeError> {
    if count == 0 {
        return Err(refused(why::NO_PAGES));
    }
    let asked = spec.trim();
    let mut numbers: Vec<usize> = Vec::new();
    if asked.is_empty() || asked.eq_ignore_ascii_case("all") {
        numbers.extend(1..=count);
    } else {
        for piece in asked.split([',', ' ']).filter(|piece| !piece.is_empty()) {
            let (from, to) = match piece.split_once('-') {
                None => {
                    let one = number(piece, count)?;
                    (one, one)
                }
                Some((start, end)) => (
                    if start.trim().is_empty() {
                        1
                    } else {
                        number(start, count)?
                    },
                    if end.trim().is_empty() {
                        count
                    } else {
                        number(end, count)?
                    },
                ),
            };
            if from > to {
                return Err(refused(why::RANGE_BACKWARDS));
            }
            numbers.extend(from..=to);
        }
    }
    numbers.retain(|number| match only {
        Only::Every => true,
        Only::Odd => number % 2 == 1,
        Only::Even => number % 2 == 0,
    });
    numbers.sort_unstable();
    numbers.dedup();
    if numbers.is_empty() {
        return Err(refused(why::RANGE_NAMES_NOTHING));
    }
    Ok(numbers.into_iter().map(|number| number - 1).collect())
}

fn number(written: &str, count: usize) -> Result<usize, SpikeError> {
    let number: usize = written
        .trim()
        .parse()
        .map_err(|_| refused(why::RANGE_NOT_NUMBERS))?;
    if number == 0 {
        return Err(refused(why::RANGE_PAGE_ZERO));
    }
    if number > count {
        return Err(refused(why::RANGE_PAST_THE_END));
    }
    Ok(number)
}

fn frame_for(crop: [f64; 4], stamp: &Stamp, width: f64) -> Result<[f64; 4], SpikeError> {
    if !(stamp.margin.is_finite() && stamp.margin >= 0.0) {
        return Err(refused(why::BAD_MARGIN));
    }
    if !(stamp.size.is_finite() && stamp.size > 0.0) {
        return Err(refused(why::BAD_SIZE));
    }
    let [left, bottom, right, top] = crop;
    let (holds, high) = (right - left - 2.0 * stamp.margin, top - bottom);
    if width > holds {
        return Err(refused(why::WIDER_THAN_THE_PAGE));
    }
    if stamp.size > high - 2.0 * stamp.margin {
        return Err(refused(why::NO_ROOM));
    }
    let baseline = match stamp.spot {
        Spot::Along {
            edge: Edge::Footer, ..
        } => bottom + stamp.margin,
        Spot::Along {
            edge: Edge::Header, ..
        } => top - stamp.margin - stamp.size,
        Spot::Middle => f64::midpoint(bottom, top) - stamp.size / 2.0,
    };
    let start = match stamp.spot {
        Spot::Along {
            side: Side::Left, ..
        } => left + stamp.margin,
        Spot::Along {
            side: Side::Right, ..
        } => right - stamp.margin - width,
        Spot::Along {
            side: Side::Centre, ..
        }
        | Spot::Middle => f64::midpoint(left, right) - width / 2.0,
    };
    Ok([start, baseline, start + width + 0.25, baseline + stamp.size])
}

fn width_of(page: &PlannerPage<'_>, new: &NewText<'_>) -> Result<f64, SpikeError> {
    let face = crate::new_text::face_for(page, new)?;
    let embeddable = crate::new_font::Embeddable::of(&face.program)
        .ok_or_else(|| refused("the face chosen cannot be embedded yet"))?;
    let paragraphs = crate::new_text::shape(new.text, &face, embeddable, new.size)?;
    Ok(paragraphs
        .iter()
        .flatten()
        .map(|cluster| cluster.advance)
        .sum())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Landing {
    pub line: String,
    pub frame: [f64; 4],
    pub turn: Matrix,
}

pub fn landing_on(
    source: &ByteStore,
    page_index: usize,
    credential: &[u8],
    fonts: Option<&std::sync::Arc<dyn pdf_content::FontProvider>>,
    stamp: &Stamp,
    facts: &Facts,
) -> Result<Landing, SpikeError> {
    let reading = crate::spike_move_text::read_page(source, page_index, credential, fonts)?;
    let page = PlannerPage {
        program: &reading.program,
        operations: &reading.operations,
        graph: &reading.graph,
        fonts,
        restrictions: crate::Restrictions::Respect,
        credential: b"",
    };
    landing(&page, stamp, facts)
}

pub fn landing(
    page: &PlannerPage<'_>,
    stamp: &Stamp,
    facts: &Facts,
) -> Result<Landing, SpikeError> {
    let (line, frame, turn) = laid_out(page, stamp, facts)?;
    Ok(Landing {
        line,
        frame: in_user_space(frame, turn),
        turn,
    })
}

fn laid_out(
    page: &PlannerPage<'_>,
    stamp: &Stamp,
    facts: &Facts,
) -> Result<(String, [f64; 4], Matrix), SpikeError> {
    let line = worded(&stamp.wording, facts)?;
    if line.contains('\n') {
        return Err(refused(why::ONE_LINE));
    }
    if line.trim().is_empty() {
        return Err(refused(why::NO_TEXT));
    }
    let (sheet, turn) = shown(&page.program.geometry)?;
    let width = width_of(page, &as_typed(stamp, &line))?;
    let frame = frame_for(sheet, stamp, width)?;
    Ok((line, frame, turn))
}

pub(crate) fn shown(
    geometry: &pdf_content::PageGeometry,
) -> Result<([f64; 4], Matrix), SpikeError> {
    let [x0, y0, x1, y1] = geometry.crop_box;
    let turn = match geometry.rotate {
        0 => Matrix {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: x0,
            f: y0,
        },
        90 => Matrix {
            a: 0.0,
            b: 1.0,
            c: -1.0,
            d: 0.0,
            e: x1,
            f: y0,
        },
        180 => Matrix {
            a: -1.0,
            b: 0.0,
            c: 0.0,
            d: -1.0,
            e: x1,
            f: y1,
        },
        270 => Matrix {
            a: 0.0,
            b: -1.0,
            c: 1.0,
            d: 0.0,
            e: x0,
            f: y1,
        },
        _ => return Err(refused(why::PAGE_IS_TURNED)),
    };
    let (width, height) = geometry.rotated_size();
    Ok(([0.0, 0.0, width, height], turn))
}

fn in_user_space([x0, y0, x1, y1]: [f64; 4], turn: Matrix) -> [f64; 4] {
    let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
        .map(|(x, y)| turn.transform(pdf_paint::Point { x, y }));
    [
        corners.iter().map(|at| at.x).fold(f64::INFINITY, f64::min),
        corners.iter().map(|at| at.y).fold(f64::INFINITY, f64::min),
        corners
            .iter()
            .map(|at| at.x)
            .fold(f64::NEG_INFINITY, f64::max),
        corners
            .iter()
            .map(|at| at.y)
            .fold(f64::NEG_INFINITY, f64::max),
    ]
}

fn as_typed<'a>(stamp: &'a Stamp, line: &'a str) -> NewText<'a> {
    NewText {
        paragraph: crate::plan::ParagraphLayout::default(),
        frame: [0.0, 0.0, 1.0, 1.0],
        text: line,
        family: &stamp.family,
        size: stamp.size,
        bold: stamp.bold,
        italic: stamp.italic,
        fill: stamp.fill,
        opacity: stamp.opacity,
        turn: pdf_paint::Matrix::IDENTITY,
        share_from: None,
    }
}

pub(crate) fn plan_stamp(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    stamp: &Stamp,
    (facts, share_from): (&Facts, Option<usize>),
) -> Result<Plan, SpikeError> {
    let (line, frame, turn) = laid_out(&page, stamp, facts)?;
    let new = NewText {
        frame,
        turn,
        share_from,
        ..as_typed(stamp, &line)
    };
    crate::new_text::plan_new_text(source, page, page_index, &new)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};
    use pdf_content::FontProvider;
    use pdf_paint::PaintAtomKind;

    use super::{Edge, Facts, Only, Side, Spot, Stamp, landing_on, pages_of, worded};
    use crate::plan::Command;
    use crate::spike_move_text::{plan_command_with_fonts, read_page};

    fn document(boxes: &str) -> ByteStore {
        document_drawing(boxes, "", "0 0 0 rg 10 10 20 20 re f")
    }

    fn document_drawing(boxes: &str, resources: &str, content: &str) -> ByteStore {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /MediaBox [0 0 200 300] /Kids [3 0 R] /Count 1 >>".to_owned(),
            format!(
                "<< /Type /Page /Parent 2 0 R /Contents 4 0 R {boxes} /Resources << /ProcSet [/PDF] {resources} >> >>"
            ),
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ),
        ];
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

    pub(crate) fn two_pages() -> ByteStore {
        let content = "0 0 0 rg 10 10 20 20 re f";
        let stream = format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        );
        let page = |contents: usize| {
            format!(
                "<< /Type /Page /Parent 2 0 R /Contents {contents} 0 R /Resources << /ProcSet [/PDF] >> >>"
            )
        };
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /MediaBox [0 0 200 300] /Kids [3 0 R 5 0 R] /Count 2 >>".to_owned(),
            page(4),
            stream.clone(),
            page(6),
            stream,
        ];
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

    fn pens(source: &ByteStore, fonts: &Arc<dyn FontProvider>) -> Vec<(f64, f64)> {
        let reading = read_page(source, 0, b"", Some(fonts)).expect("the page reads");
        reading
            .graph
            .atoms
            .iter()
            .filter_map(|atom| match &atom.kind {
                PaintAtomKind::Text(text) => Some(text),
                _ => None,
            })
            .flat_map(|text| {
                text.glyphs.iter().map(|glyph| {
                    let at = text
                        .state
                        .ctm
                        .value
                        .multiply(glyph.matrix)
                        .transform(pdf_paint::Point { x: 0.0, y: 0.0 });
                    (at.x, at.y)
                })
            })
            .collect()
    }

    fn facts() -> Facts {
        Facts {
            number: 3,
            count: 12,
            name: "book.pdf".to_owned(),
            today: "18/09/2026".to_owned(),
        }
    }

    const WIDTH: f64 = 7.5;

    fn stamp(spot: Spot) -> Stamp {
        Stamp {
            wording: "AB".to_owned(),
            spot,
            family: "Test Face".to_owned(),
            size: 10.0,
            bold: false,
            italic: false,
            fill: None,
            opacity: 1.0,
            margin: 20.0,
        }
    }

    fn stamped(boxes: &str, stamp: &Stamp) -> Result<Vec<(f64, f64)>, crate::SpikeError> {
        let source = document(boxes);
        let fonts = crate::new_text::tests::provider();
        let plan = plan_command_with_fonts(
            &source,
            &Command::Stamp {
                page_index: 0,
                stamp: stamp.clone(),
                facts: facts(),
                share_from: None,
            },
            b"",
            Some(Arc::clone(&fonts)),
        )?;
        let after = plan.commit(&source, b"").expect("the plan commits");
        Ok(pens(&after, &fonts))
    }

    #[test]
    fn a_wording_says_what_this_page_is() {
        assert_eq!(
            worded("{file} {page} / {pages} -- {date} {{sic}}", &facts()).expect("it words"),
            "book.pdf 3 / 12 -- 18/09/2026 {sic}"
        );
        assert_eq!(worded("Draft", &facts()).expect("it words"), "Draft");
    }

    #[test]
    fn a_wording_this_cannot_answer_is_refused() {
        for wording in ["{pages }", "{page", "page}", "{}"] {
            assert!(
                worded(wording, &facts()).is_err(),
                "{wording} should be refused"
            );
        }
    }

    #[test]
    fn a_footer_sits_a_margin_in_from_the_bottom_left() {
        let places = stamped(
            "",
            &stamp(Spot::Along {
                edge: Edge::Footer,
                side: Side::Left,
            }),
        )
        .expect("the footer is planned");
        assert_eq!(places, vec![(20.0, 20.0), (25.0, 20.0)]);
    }

    #[test]
    fn a_header_on_the_right_ends_at_the_right_margin() {
        let places = stamped(
            "",
            &stamp(Spot::Along {
                edge: Edge::Header,
                side: Side::Right,
            }),
        )
        .expect("the header is planned");
        let start = 200.0 - 20.0 - WIDTH;
        assert_eq!(places, vec![(start, 270.0), (start + 5.0, 270.0)]);
    }

    #[test]
    fn a_watermark_is_centred_on_the_page() {
        let places = stamped("", &stamp(Spot::Middle)).expect("the watermark is planned");
        let start = 100.0 - WIDTH / 2.0;
        assert_eq!(places, vec![(start, 145.0), (start + 5.0, 145.0)]);
    }

    #[test]
    fn the_line_a_page_shows_is_the_worded_one() {
        let source = document("");
        let fonts = crate::new_text::tests::provider();
        let mut asked = stamp(Spot::Middle);
        asked.wording = "{file}".to_owned();
        let placed = |name: &str| {
            let plan = plan_command_with_fonts(
                &source,
                &Command::Stamp {
                    page_index: 0,
                    stamp: asked.clone(),
                    facts: Facts {
                        name: name.to_owned(),
                        ..facts()
                    },
                    share_from: None,
                },
                b"",
                Some(Arc::clone(&fonts)),
            )
            .expect("the stamp is planned");
            let after = plan.commit(&source, b"").expect("the plan commits");
            pens(&after, &fonts)
        };
        assert_eq!(
            placed("AB"),
            vec![
                (100.0 - WIDTH / 2.0, 145.0),
                (100.0 - WIDTH / 2.0 + 5.0, 145.0)
            ]
        );
        assert_eq!(placed("A"), vec![(97.5, 145.0)]);
    }

    #[test]
    fn a_stamp_follows_the_crop_box_and_not_the_media_box() {
        let places = stamped(
            "/CropBox [40 60 160 240]",
            &stamp(Spot::Along {
                edge: Edge::Footer,
                side: Side::Left,
            }),
        )
        .expect("the footer is planned");
        assert_eq!(places, vec![(60.0, 80.0), (65.0, 80.0)]);
    }

    #[test]
    fn what_is_refused() {
        let wide = Stamp {
            size: 400.0,
            ..stamp(Spot::Middle)
        };
        assert!(stamped("", &wide).is_err(), "a line wider than the page");
        let margin = Stamp {
            margin: 149.0,
            ..stamp(Spot::Middle)
        };
        assert!(stamped("", &margin).is_err(), "no room between the margins");
        let empty = Stamp {
            wording: "   ".to_owned(),
            ..stamp(Spot::Along {
                edge: Edge::Footer,
                side: Side::Left,
            })
        };
        assert!(stamped("", &empty).is_err(), "a wording that says nothing");
    }

    #[test]
    fn a_range_names_the_pages_a_person_wrote() {
        let every: Vec<usize> = (0..12).collect();
        assert_eq!(pages_of("", 12, Only::Every).expect("all"), every);
        assert_eq!(pages_of("  All ", 12, Only::Every).expect("all"), every);
        assert_eq!(
            pages_of("1-3, 8 5", 12, Only::Every).expect("a range"),
            vec![0, 1, 2, 4, 7]
        );
        assert_eq!(
            pages_of("-2, 11-, 2", 12, Only::Every).expect("a range"),
            vec![0, 1, 10, 11]
        );
    }

    #[test]
    fn odd_and_even_count_what_a_reader_sees() {
        assert_eq!(
            pages_of("1-6", 12, Only::Odd).expect("odd"),
            vec![0, 2, 4],
            "pages 1, 3 and 5"
        );
        assert_eq!(
            pages_of("1-6", 12, Only::Even).expect("even"),
            vec![1, 3, 5],
            "pages 2, 4 and 6"
        );
    }

    #[test]
    fn a_range_this_document_cannot_answer_is_refused() {
        for spec in ["0", "13", "5-2", "one", "3-x", "1-3, 99"] {
            assert!(
                pages_of(spec, 12, Only::Every).is_err(),
                "{spec} should be refused"
            );
        }
        assert!(pages_of("2", 12, Only::Odd).is_err(), "no page is left");
        assert!(pages_of("", 0, Only::Every).is_err(), "no pages at all");
    }

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "the preview and the write compute the frame by the same arithmetic"
    )]
    fn a_preview_says_where_the_line_will_land() {
        let source = document("");
        let fonts = crate::new_text::tests::provider();
        let asked = stamp(Spot::Along {
            edge: Edge::Header,
            side: Side::Right,
        });
        let landing =
            landing_on(&source, 0, b"", Some(&fonts), &asked, &facts()).expect("the preview lands");
        assert_eq!(landing.line, "AB");
        let start = 200.0 - 20.0 - WIDTH;
        assert_eq!(landing.frame, [start, 270.0, start + WIDTH + 0.25, 280.0]);
        assert_eq!(
            stamped("", &asked).expect("the header is planned")[0],
            (landing.frame[0], landing.frame[1]),
            "the glyph lands on the corner the preview drew"
        );
    }

    #[test]
    fn a_preview_refuses_what_the_writing_would_refuse() {
        let source = document("");
        let fonts = crate::new_text::tests::provider();
        let asked = Stamp {
            size: 400.0,
            ..stamp(Spot::Middle)
        };
        assert!(landing_on(&source, 0, b"", Some(&fonts), &asked, &facts()).is_err());
    }

    #[test]
    fn a_range_is_stamped_page_by_page_as_one_step() {
        let source = two_pages();
        let fonts = crate::new_text::tests::provider();
        let mut asked = stamp(Spot::Along {
            edge: Edge::Footer,
            side: Side::Left,
        });
        asked.wording = "{file}".to_owned();
        let commands: Vec<Command> = pages_of("", 2, Only::Every)
            .expect("both pages")
            .into_iter()
            .map(|page_index| Command::Stamp {
                page_index,
                stamp: asked.clone(),
                facts: Facts {
                    name: ["BA", "A"][page_index].to_owned(),
                    ..facts()
                },
                share_from: (page_index > 0).then_some(0),
            })
            .collect();
        let mut history = crate::history::History::new(source, b"");
        history
            .apply_together_with(commands.len(), |source, at| {
                crate::spike_move_text::plan_command_under(
                    source,
                    &commands[at],
                    b"",
                    (Some(&fonts), crate::Restrictions::Respect),
                )
            })
            .expect("the range is stamped");
        let on = |source: &ByteStore, page: usize| {
            let reading = read_page(source, page, b"", Some(&fonts)).expect("reads");
            let mut out = Vec::new();
            for atom in &reading.graph.atoms {
                if let PaintAtomKind::Text(text) = &atom.kind {
                    for glyph in &text.glyphs {
                        let at = text
                            .state
                            .ctm
                            .value
                            .multiply(glyph.matrix)
                            .transform(pdf_paint::Point { x: 0.0, y: 0.0 });
                        out.push((at.x, at.y));
                    }
                }
            }
            out
        };
        assert_eq!(on(history.source(), 0), vec![(20.0, 20.0), (22.5, 20.0)]);
        assert_eq!(on(history.source(), 1), vec![(20.0, 20.0)]);
        let fonts_of = |page: usize| -> Vec<pdf_syntax::Reference> {
            pdf_content::load_page_program_strict(
                history.source(),
                page,
                pdf_content::PageContentLimits::default(),
            )
            .expect("the page loads")
            .resources
            .fonts()
            .iter()
            .filter_map(pdf_content::ResourceEntry::reference)
            .collect()
        };
        assert_eq!(fonts_of(0).len(), 1);
        assert_eq!(
            fonts_of(0),
            fonts_of(1),
            "the second page shares the first's face"
        );
        assert_eq!(history.undo_depth(), 1);
        assert!(history.undo().expect("undo walks"));
        assert!(on(history.source(), 0).is_empty(), "page 0 is back");
        assert!(on(history.source(), 1).is_empty(), "page 1 is back");
    }

    #[test]
    fn a_turned_page_is_stamped_the_right_way_up_for_its_reader() {
        let footer = stamp(Spot::Along {
            edge: Edge::Footer,
            side: Side::Left,
        });
        for (turn, first, second) in [
            ("/Rotate 90", (180.0, 20.0), (180.0, 25.0)),
            ("/Rotate 180", (180.0, 280.0), (175.0, 280.0)),
            ("/Rotate 270", (20.0, 280.0), (20.0, 275.0)),
        ] {
            assert_eq!(
                stamped(turn, &footer).expect("the turned page is stamped"),
                vec![first, second],
                "{turn}"
            );
        }
        let header = stamp(Spot::Along {
            edge: Edge::Header,
            side: Side::Left,
        });
        assert_eq!(
            stamped("/Rotate 90", &header).expect("the header is stamped"),
            vec![(30.0, 20.0), (30.0, 25.0)],
            "shown 200 high: the baseline is 200 - 20 - 10 up, the file's x = 200 - 170"
        );
    }

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "a quarter turn moves whole numbers to whole numbers"
    )]
    fn a_preview_on_a_turned_page_covers_where_the_line_lands() {
        let source = document("/Rotate 90");
        let fonts = crate::new_text::tests::provider();
        let asked = stamp(Spot::Along {
            edge: Edge::Footer,
            side: Side::Left,
        });
        let landing =
            landing_on(&source, 0, b"", Some(&fonts), &asked, &facts()).expect("the preview lands");
        assert_eq!(landing.frame, [170.0, 20.0, 180.0, 20.0 + WIDTH + 0.25]);
        assert_eq!((landing.turn.b, landing.turn.c), (1.0, -1.0));
    }

    fn alphas(
        source: &ByteStore,
        page: usize,
        fonts: &Arc<dyn FontProvider>,
    ) -> Vec<(bool, f64, f64)> {
        let reading = read_page(source, page, b"", Some(fonts)).expect("the page reads");
        reading
            .graph
            .atoms
            .iter()
            .map(|atom| match &atom.kind {
                PaintAtomKind::Text(text) => (
                    true,
                    text.state.fill_alpha.value,
                    text.state.stroke_alpha.value,
                ),
                PaintAtomKind::Path(path) => (
                    false,
                    path.state.fill_alpha.value,
                    path.state.stroke_alpha.value,
                ),
                _ => (false, f64::NAN, f64::NAN),
            })
            .collect()
    }

    fn states_of(source: &ByteStore, page: usize) -> Vec<pdf_syntax::Reference> {
        pdf_content::load_page_program_strict(
            source,
            page,
            pdf_content::PageContentLimits::default(),
        )
        .expect("the page loads")
        .resources
        .ext_gstates()
        .iter()
        .filter_map(pdf_content::ResourceEntry::reference)
        .collect()
    }

    #[test]
    fn a_watermark_lets_the_page_show_through() {
        let source = two_pages();
        let fonts = crate::new_text::tests::provider();
        let asked = Stamp {
            opacity: 0.3,
            ..stamp(Spot::Middle)
        };
        let commands: Vec<Command> = (0..2)
            .map(|page_index| Command::Stamp {
                page_index,
                stamp: asked.clone(),
                facts: facts(),
                share_from: (page_index > 0).then_some(0),
            })
            .collect();
        let mut history = crate::history::History::new(source, b"");
        history
            .apply_together_with(commands.len(), |source, at| {
                crate::spike_move_text::plan_command_under(
                    source,
                    &commands[at],
                    b"",
                    (Some(&fonts), crate::Restrictions::Respect),
                )
            })
            .expect("the range is stamped");
        for page in 0..2 {
            assert_eq!(
                alphas(history.source(), page, &fonts),
                vec![(false, 1.0, 1.0), (true, 0.3, 0.3)],
                "page {page}: the rectangle is ink, the watermark is see-through"
            );
        }
        assert_eq!(states_of(history.source(), 0).len(), 1);
        assert_eq!(
            states_of(history.source(), 0),
            states_of(history.source(), 1),
            "the second page names the first page's graphics state"
        );
        assert!(history.undo().expect("undo walks"));
        assert!(states_of(history.source(), 0).is_empty());
        assert!(states_of(history.source(), 1).is_empty());
    }

    #[test]
    fn an_opaque_stamp_brings_no_graphics_state() {
        let source = document("");
        let fonts = crate::new_text::tests::provider();
        let plan = plan_command_with_fonts(
            &source,
            &Command::Stamp {
                page_index: 0,
                stamp: stamp(Spot::Middle),
                facts: facts(),
                share_from: None,
            },
            b"",
            Some(Arc::clone(&fonts)),
        )
        .expect("the stamp is planned");
        let after = plan.commit(&source, b"").expect("the plan commits");
        assert!(states_of(&after, 0).is_empty());
    }

    #[test]
    fn a_page_left_see_through_does_not_make_the_stamp_faint() {
        let source = document_drawing(
            "",
            "/ExtGState << /GS1 << /Type /ExtGState /ca 0.5 /CA 0.5 >> >>",
            "/GS1 gs 0 0 0 rg 10 10 20 20 re f",
        );
        let fonts = crate::new_text::tests::provider();
        for opacity in [1.0, 0.25] {
            let plan = plan_command_with_fonts(
                &source,
                &Command::Stamp {
                    page_index: 0,
                    stamp: Stamp {
                        opacity,
                        ..stamp(Spot::Middle)
                    },
                    facts: facts(),
                    share_from: None,
                },
                b"",
                Some(Arc::clone(&fonts)),
            )
            .expect("the stamp is planned");
            let after = plan.commit(&source, b"").expect("the plan commits");
            assert_eq!(
                alphas(&after, 0, &fonts),
                vec![(false, 0.5, 0.5), (true, opacity, opacity)],
                "asked for {opacity}"
            );
            let named = pdf_content::load_page_program_strict(
                &after,
                0,
                pdf_content::PageContentLimits::default(),
            )
            .expect("the page loads")
            .resources
            .ext_gstates()
            .len();
            assert_eq!(
                named, 2,
                "the page's own, and the stamp's: the page's says too little"
            );
        }
    }
}
