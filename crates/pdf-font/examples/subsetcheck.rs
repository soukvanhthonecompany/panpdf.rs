use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use pdf_font::subset::{SubsetError, subset_cff, subset_truetype};
use pdf_font::truetype::TrueTypeFont;
use rustybuzz::ttf_parser;

const SAMPLES: &[&str] = &[
    "The quick brown fox, café naïve Å ß",
    "ສະບາຍດີ ພາສາລາວ ກ່ຽວກັບ",
    "สวัสดีครับ ภาษาไทย ที่นี่",
    "Привет мир Ελληνικά",
    "مرحبا بالعالم",
    "नमस्ते दुनिया",
    "ភាសាខ្មែរ မြန်မာ",
    "שלום עולם",
    "中文字体 日本語のテキスト 한국어 텍스트",
];

struct Bounds;
impl ttf_parser::OutlineBuilder for Bounds {
    fn move_to(&mut self, _: f32, _: f32) {}
    fn line_to(&mut self, _: f32, _: f32) {}
    fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {}
    fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {}
    fn close(&mut self) {}
}

type Checked = (usize, usize, Vec<String>);

fn check_cff(font: &TrueTypeFont, asked: &BTreeSet<u16>) -> Option<Result<Checked, String>> {
    let table = font.cff_table()?;
    let (program, kept) = match subset_cff(table, asked) {
        Ok(found) => found,
        Err(error) => return Some(Err(error.to_string())),
    };
    let mut problems = Vec::new();
    let ours = pdf_font::cff::CffFont::parse(&program);
    let original = pdf_font::cff::CffFont::parse(table).ok()?;
    let theirs = ttf_parser::cff::Table::parse(&program);
    let before = ttf_parser::cff::Table::parse(table)?;
    match (ours, theirs) {
        (Ok(ours), Some(theirs)) => {
            if ours.glyph_count() != original.glyph_count() {
                problems.push("glyph count differs".to_owned());
            }
            let count = u16::try_from(original.glyph_count()).unwrap_or(u16::MAX);
            for glyph in (0..count).filter(|glyph| kept.contains(glyph) || glyph % 97 == 1) {
                let id = ttf_parser::GlyphId(glyph);
                if kept.contains(&glyph) {
                    if ours.outline(&program, glyph) != original.outline(table, glyph) {
                        problems.push(format!("glyph {glyph}: outline differs"));
                    }
                    if theirs.outline(id, &mut Bounds).ok() != before.outline(id, &mut Bounds).ok()
                    {
                        problems.push(format!("glyph {glyph}: ttf-parser box differs"));
                    }
                } else if ours
                    .outline(&program, glyph)
                    .is_some_and(|path| path != pdf_font::glyph::GlyphPath::default())
                {
                    problems.push(format!("glyph {glyph}: not emptied"));
                }
                if theirs.glyph_cid(id) != before.glyph_cid(id) {
                    problems.push(format!("glyph {glyph}: CID differs"));
                }
            }
        }
        (ours, theirs) => problems.push(format!(
            "does not parse: ours {:?}, ttf-parser {}",
            ours.err(),
            theirs.is_some()
        )),
    }
    Some(Ok((table.len(), program.len(), problems)))
}

fn faces(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            faces(&path, out);
        } else if path.extension().is_some_and(|extension| {
            ["ttf", "otf", "ttc"]
                .iter()
                .any(|wanted| extension.eq_ignore_ascii_case(wanted))
        }) {
            out.push(path);
        }
    }
}

#[expect(clippy::too_many_lines, reason = "one check, face by face")]
fn main() {
    let mut paths = Vec::new();
    for dir in std::env::args().skip(1) {
        faces(Path::new(&dir), &mut paths);
    }
    paths.sort();
    let (mut checked, mut refused, mut wrong) = (0, 0, 0);
    let (mut cff_checked, mut cff_refused, mut cff_wrong) = (0, 0, 0);
    let (mut cff_before, mut cff_after) = (0_usize, 0_usize);
    let (mut before, mut after) = (0_usize, 0_usize);
    for path in &paths {
        let Ok(data) = std::fs::read(path) else {
            continue;
        };
        let faces = TrueTypeFont::face_count(&data).unwrap_or(1).min(4);
        for face in 0..faces {
            let Ok(font) = TrueTypeFont::parse_face(data.clone(), face) else {
                continue;
            };
            let Some(full) = rustybuzz::Face::from_slice(&data, face) else {
                continue;
            };
            let mut asked = BTreeSet::new();
            for text in SAMPLES {
                let mut buffer = rustybuzz::UnicodeBuffer::new();
                buffer.push_str(text);
                buffer.guess_segment_properties();
                let shaped = rustybuzz::shape(&full, &[], buffer);
                asked.extend(
                    shaped
                        .glyph_infos()
                        .iter()
                        .filter_map(|info| u16::try_from(info.glyph_id).ok()),
                );
            }
            asked.extend((0..font.glyph_count()).step_by(7));
            if font.cff_table().is_some() {
                match check_cff(&font, &asked) {
                    Some(Ok((before, after, problems))) => {
                        cff_checked += 1;
                        cff_before += before;
                        cff_after += after;
                        if !problems.is_empty() {
                            cff_wrong += 1;
                            println!(
                                "WRONG CFF {} #{face} {}",
                                path.display(),
                                problems[..problems.len().min(3)].join("; ")
                            );
                        }
                    }
                    Some(Err(error)) => {
                        cff_refused += 1;
                        println!("REFUSED CFF {} #{face} {error}", path.display());
                    }
                    None => {}
                }
                continue;
            }
            let subset = match subset_truetype(&font, &asked) {
                Ok(subset) => subset,
                Err(SubsetError::NoTrueTypeOutlines) => continue,
                Err(error) => {
                    refused += 1;
                    println!("REFUSED {} {error}", path.display());
                    continue;
                }
            };
            checked += 1;
            before += data.len();
            after += subset.program.len();
            let mut problems = Vec::new();
            let ours = TrueTypeFont::parse(subset.program.clone());
            let theirs = ttf_parser::Face::parse(&subset.program, 0);
            match (ours, theirs) {
                (Ok(ours), Ok(theirs)) => {
                    for glyph in 0..ours.glyph_count() {
                        let id = ttf_parser::GlyphId(glyph);
                        if subset.glyphs.contains(&glyph) {
                            if ours.outline(glyph) != font.outline(glyph) {
                                problems.push(format!("glyph {glyph}: outline differs"));
                            }
                            if theirs.glyph_bounding_box(id) != full.glyph_bounding_box(id) {
                                problems.push(format!("glyph {glyph}: ttf-parser box differs"));
                            }
                        } else if ours
                            .outline(glyph)
                            .is_some_and(|outline| !outline.is_empty())
                        {
                            problems.push(format!("glyph {glyph}: not emptied"));
                        }
                        if ours.advance_width(glyph) != font.advance_width(glyph)
                            || theirs.glyph_hor_advance(id) != full.glyph_hor_advance(id)
                        {
                            problems.push(format!("glyph {glyph}: advance differs"));
                        }
                    }
                }
                (ours, theirs) => problems.push(format!(
                    "does not parse: ours {:?}, ttf-parser {:?}",
                    ours.err(),
                    theirs.err()
                )),
            }
            if !problems.is_empty() {
                wrong += 1;
                println!(
                    "WRONG {} {}",
                    path.display(),
                    problems[..problems.len().min(3)].join("; ")
                );
            }
        }
    }
    println!(
        "faces checked {checked}, refused {refused}, wrong {wrong}; bytes {before} -> {after}"
    );
    println!(
        "CFF faces checked {cff_checked}, refused {cff_refused}, wrong {cff_wrong}; bytes {cff_before} -> {cff_after}"
    );
}
