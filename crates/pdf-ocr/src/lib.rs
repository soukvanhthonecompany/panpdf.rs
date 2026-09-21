pub mod models;
pub mod setup;
pub mod sha1;
pub mod store;
pub mod tesseract;
pub mod tsv;

use pdf_edit::text_layer::{LayerWord, TextLayer};

pub use models::{Model, Quality};
pub use store::FetchError;
pub use tesseract::{Grey, OcrError, Tesseract};
pub use tsv::{Line, Word};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    pub layer: TextLayer,
    pub confidence: Option<f32>,
}

pub const DPI: u32 = 300;

pub fn read_page(
    layers: &[&pdf_paint::PaintGraph],
    geometry: &pdf_content::PageGeometry,
    engine: &Tesseract,
    languages: &[String],
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Reading, OcrError> {
    let scale = f64::from(DPI) / 72.0;
    let options = pdf_render::RenderOptions {
        scale,
        ..pdf_render::RenderOptions::default()
    };
    let (canvas, _) = pdf_render::render_page_layers(layers, geometry, options)
        .map_err(|error| OcrError::Render(error.to_string()))?;
    let grey = Grey {
        width: canvas.width,
        height: canvas.height,
        pixels: canvas
            .to_rgb8()
            .chunks_exact(3)
            .map(|pixel| {
                let sum = 299 * u32::from(pixel[0])
                    + 587 * u32::from(pixel[1])
                    + 114 * u32::from(pixel[2]);
                u8::try_from(sum / 1000).unwrap_or(u8::MAX)
            })
            .collect(),
    };
    let lines = recognize_choosing(engine, &grey, languages, cancel)?;
    let (_, shown_height) = geometry.rotated_size();
    Ok(reading(&lines, scale, shown_height))
}

const LAO: std::ops::RangeInclusive<char> = '\u{0e80}'..='\u{0eff}';

pub const LAO_FLOOR: f32 = 25.0;

pub fn recognize_choosing(
    engine: &Tesseract,
    image: &Grey,
    languages: &[String],
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Vec<Line>, OcrError> {
    let has = |code: &str| languages.iter().any(|language| language == code);
    if !(has("lao") && has("tha")) {
        return engine.recognize(image, languages, DPI, cancel);
    }
    let without = |code: &str| -> Vec<String> {
        languages
            .iter()
            .filter(|language| *language != code)
            .cloned()
            .collect()
    };
    let as_lao = engine.recognize(image, &without("tha"), DPI, cancel)?;
    if is_not_lao(&as_lao) {
        engine.recognize(image, &without("lao"), DPI, cancel)
    } else {
        Ok(as_lao)
    }
}

#[must_use]
pub fn is_not_lao(as_lao: &[Line]) -> bool {
    script_confidence(as_lao, &LAO).is_some_and(|sure| sure < LAO_FLOOR)
}

#[must_use]
pub fn script_confidence(lines: &[Line], script: &std::ops::RangeInclusive<char>) -> Option<f32> {
    let (mut weight, mut sum) = (0_usize, 0.0_f64);
    for word in lines.iter().flat_map(|line| &line.words) {
        let count = word
            .text
            .chars()
            .filter(|letter| script.contains(letter))
            .count();
        if count == 0 {
            continue;
        }
        weight += count;
        #[expect(
            clippy::cast_precision_loss,
            reason = "a page has far fewer letters than 2^52"
        )]
        let counted = count as f64;
        sum += counted * f64::from(word.confidence);
    }
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "a percentage"
    )]
    let mean = (weight > 0).then(|| (sum / weight as f64) as f32);
    mean
}

#[must_use]
pub fn reading(lines: &[Line], scale: f64, shown_height: f64) -> Reading {
    let mut words = Vec::new();
    let (mut weight, mut sum) = (0.0_f64, 0.0_f64);
    let point = |pixel: u32| f64::from(pixel) / scale;
    for line in lines {
        let top = shown_height - point(line.pixels[1]);
        let bottom = shown_height - point(line.pixels[3]);
        let mut run: Vec<&Word> = Vec::new();
        for (index, word) in line.words.iter().enumerate() {
            run.push(word);
            let last = index + 1 == line.words.len();
            if !(word.space_after || last) {
                continue;
            }
            let mut text: String = run
                .iter()
                .flat_map(|word| word.text.chars())
                .filter(|letter| {
                    !letter.is_control()
                        && letter.len_utf16() == 1
                        && !matches!(letter, '\u{feff}' | '\u{fffe}' | '\u{ffff}')
                })
                .collect();
            let left = point(run.iter().map(|word| word.pixels[0]).min().unwrap_or(0));
            let own_right = point(run.iter().map(|word| word.pixels[2]).max().unwrap_or(0));
            let right = match line.words.get(index + 1) {
                Some(next) if word.space_after && point(next.pixels[0]) > own_right => {
                    text.push(' ');
                    point(next.pixels[0])
                }
                _ => own_right,
            };
            for word in &run {
                let length =
                    f64::from(u32::try_from(word.text.chars().count()).unwrap_or(u32::MAX));
                weight += length;
                sum += length * f64::from(word.confidence);
            }
            run.clear();
            if text.trim().is_empty() || right <= left || top <= bottom {
                continue;
            }
            words.push(LayerWord {
                text,
                frame: [left, bottom, right, top],
            });
        }
    }
    Reading {
        layer: TextLayer { words },
        #[expect(clippy::cast_possible_truncation, reason = "a percentage fits an f32")]
        confidence: (weight > 0.0).then(|| (sum / weight) as f32),
    }
}

#[cfg(test)]
mod tests {
    use super::{Line, Word, reading};

    fn word(text: &str, left: u32, right: u32, space_after: bool) -> Word {
        Word {
            text: text.to_owned(),
            pixels: [left, 110, right, 130],
            confidence: 90.0,
            space_after,
        }
    }

    #[test]
    fn words_are_in_points_on_the_page_as_shown() {
        let lines = [Line {
            pixels: [20, 100, 400, 140],
            words: vec![word("Hello", 20, 120, true), word("world", 140, 240, false)],
        }];
        let read = reading(&lines, 2.0, 400.0);
        let words: Vec<(&str, [f64; 4])> = read
            .layer
            .words
            .iter()
            .map(|word| (word.text.as_str(), word.frame))
            .collect();
        assert_eq!(
            words,
            vec![
                ("Hello ", [10.0, 330.0, 70.0, 350.0]),
                ("world", [70.0, 330.0, 120.0, 350.0])
            ]
        );
        assert_eq!(read.confidence, Some(90.0));
    }

    #[test]
    fn a_run_between_spaces_is_one_word_in_the_order_read() {
        let lines = [Line {
            pixels: [0, 100, 300, 140],
            words: vec![
                word("ก", 10, 30, false),
                word("ุ", 12, 28, false),
                word("ง", 34, 50, true),
                word("ข", 80, 100, false),
            ],
        }];
        let read = reading(&lines, 1.0, 200.0);
        let words: Vec<(&str, [f64; 4])> = read
            .layer
            .words
            .iter()
            .map(|word| (word.text.as_str(), word.frame))
            .collect();
        assert_eq!(
            words,
            vec![
                ("กุง ", [10.0, 60.0, 80.0, 100.0]),
                ("ข", [80.0, 60.0, 100.0, 100.0])
            ]
        );
    }

    #[test]
    fn a_scripts_confidence_is_of_its_own_words() {
        let sure = |text: &str, confidence| Word {
            confidence,
            ..word(text, 0, 10, true)
        };
        let lines = [Line {
            pixels: [0, 100, 300, 140],
            words: vec![
                sure("Hello", 99.0),
                sure("\u{0e81}\u{0eb2}", 10.0),
                sure("\u{0e82}", 40.0),
            ],
        }];
        assert_eq!(super::script_confidence(&lines, &super::LAO), Some(20.0));
        let english = [Line {
            pixels: [0, 100, 300, 140],
            words: vec![sure("Hello", 99.0)],
        }];
        assert_eq!(super::script_confidence(&english, &super::LAO), None);
        let at = |confidence| {
            [Line {
                pixels: [0, 100, 300, 140],
                words: vec![sure("\u{0e81}", confidence)],
            }]
        };
        assert!(super::is_not_lao(&at(13.0)));
        assert!(!super::is_not_lao(&at(38.0)));
        assert!(!super::is_not_lao(&english));
    }

    #[test]
    fn what_cannot_be_written_is_left_out() {
        let lines = [Line {
            pixels: [0, 100, 300, 140],
            words: vec![
                word("a\u{1F600}b", 10, 50, true),
                word("\u{7}", 60, 70, false),
            ],
        }];
        let read = reading(&lines, 1.0, 200.0);
        assert_eq!(read.layer.words.len(), 1);
        assert_eq!(read.layer.words[0].text, "ab ");
        assert_eq!(reading(&[], 1.0, 200.0).confidence, None);
    }
}
