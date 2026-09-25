use crate::sha1;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Quality {
    #[default]
    Accurate,
    Fast,
}

impl Quality {
    pub const ALL: [Self; 2] = [Self::Accurate, Self::Fast];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accurate => "accurate",
            Self::Fast => "fast",
        }
    }

    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.as_str() == word)
    }

    #[must_use]
    pub const fn repository(self) -> (&'static str, &'static str) {
        match self {
            Self::Accurate => ("tessdata_best", "e12c65a915945e4c28e237a9b52bc4a8f39a0cec"),
            Self::Fast => ("tessdata_fast", "87416418657359cb625c412a48b6e1d6d41c29bd"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Model {
    pub code: &'static str,
    pub quality: Quality,
    pub bytes: u64,
    pub blob: &'static str,
    pub cer: Option<f32>,
}

impl Model {
    #[must_use]
    pub fn file_name(&self) -> String {
        format!("{}.traineddata", self.code)
    }

    #[must_use]
    pub fn url(&self) -> String {
        let (repository, commit) = self.quality.repository();
        format!(
            "https://raw.githubusercontent.com/tesseract-ocr/{repository}/{commit}/{}",
            self.file_name()
        )
    }

    #[must_use]
    pub fn is_the_file(&self, data: &[u8]) -> bool {
        u64::try_from(data.len()) == Ok(self.bytes) && sha1::blob_id(data) == self.blob
    }

    #[must_use]
    #[allow(clippy::unused_self)]
    pub const fn licence(&self) -> &'static str {
        "Apache-2.0"
    }

    #[must_use]
    pub const fn source(&self) -> (&'static str, &'static str) {
        self.quality.repository()
    }
}

pub const FIRST: [&str; 3] = ["lao", "tha", "eng"];

pub const HELPERS: [&str; 2] = ["equ", "osd"];

#[must_use]
pub fn is_a_language(code: &str) -> bool {
    !HELPERS.contains(&code)
}

#[must_use]
pub fn model(code: &str, quality: Quality) -> Option<&'static Model> {
    crate::catalogue::ROWS
        .iter()
        .find(|model| model.code == code && model.quality == quality)
}

#[must_use]
pub fn of_language(code: &str) -> Vec<&'static Model> {
    Quality::ALL
        .into_iter()
        .filter_map(|quality| model(code, quality))
        .collect()
}

#[must_use]
pub fn every_language() -> Vec<&'static str> {
    let mut codes: Vec<&'static str> = crate::catalogue::ROWS
        .iter()
        .map(|model| model.code)
        .collect();
    codes.sort_unstable();
    codes.dedup();
    codes
}

#[cfg(test)]
mod tests {
    use super::{FIRST, HELPERS, Quality, every_language, is_a_language, model, of_language};
    use crate::catalogue::ROWS;

    #[test]
    fn the_catalogue_says_what_it_was_asked() {
        for code in FIRST {
            for quality in Quality::ALL {
                let model = model(code, quality).expect("every language, both qualities");
                assert_eq!(model.code, code);
                assert_eq!(model.quality, quality);
                assert!(model.bytes > 1_000_000, "{code} {quality:?}");
                assert_eq!(model.blob.len(), 40);
                assert!(model.url().ends_with(&format!("/{code}.traineddata")));
            }
        }
        let best = model("lao", Quality::Accurate).expect("Lao, accurate");
        assert!(best.url().contains("tessdata_best"));
        let fast = model("lao", Quality::Fast).expect("Lao, fast");
        assert!(fast.url().contains("tessdata_fast"));
        assert!(fast.bytes < best.bytes);
        assert_eq!(model("zzz", Quality::Accurate), None);
        assert_eq!(model("", Quality::Fast), None);
        assert_eq!(model("", Quality::Accurate), None);
        assert_eq!(model("LAO", Quality::Accurate), None);
        assert_eq!(model("Lao", Quality::Fast), None);
        assert!(model("khm", Quality::Accurate).is_some());
        assert_eq!(of_language("zzz"), Vec::<&super::Model>::new());
        assert_eq!(of_language("tha").len(), 2);
    }

    #[test]
    fn only_the_measured_rows_carry_a_figure() {
        let measured = [
            ("lao", Quality::Accurate, 9.0_f32),
            ("tha", Quality::Accurate, 3.4),
            ("eng", Quality::Accurate, 0.4),
            ("lao", Quality::Fast, 11.6),
            ("tha", Quality::Fast, 3.3),
            ("eng", Quality::Fast, 0.4),
        ];
        for (code, quality, cer) in measured {
            let model = model(code, quality).expect("a measured row");
            let got = model.cer.expect("a measured row carries a figure");
            assert!((got - cer).abs() < f32::EPSILON, "{code} {quality:?}");
        }
        for quality in Quality::ALL {
            let khm = model("khm", quality).expect("Khmer is offered");
            assert_eq!(khm.cer, None, "nobody measured Khmer");
        }
    }

    #[test]
    fn a_quality_round_trips_and_an_unknown_word_is_refused() {
        for quality in Quality::ALL {
            assert_eq!(Quality::parse(quality.as_str()), Some(quality));
        }
        assert_eq!(Quality::parse("best"), None);
        assert_eq!(Quality::parse(""), None);
        assert_eq!(Quality::parse("Accurate"), None);
        assert_eq!(Quality::default(), Quality::Accurate);
    }

    #[test]
    fn every_row_is_its_own() {
        for (at, one) in ROWS.iter().enumerate() {
            for other in &ROWS[at + 1..] {
                assert!(
                    one.code != other.code || one.quality != other.quality,
                    "{} {:?} is in the table twice",
                    one.code,
                    one.quality
                );
                if one.code != other.code {
                    assert_ne!(
                        one.blob, other.blob,
                        "{} and {} share a digest",
                        one.code, other.code
                    );
                }
                assert_ne!(
                    one.url(),
                    other.url(),
                    "{} and {} share a URL",
                    one.code,
                    other.code
                );
            }
        }
    }

    #[test]
    fn every_language_is_every_code_once_sorted() {
        let all = every_language();
        assert_eq!(all.len(), 125);
        let mut sorted = all.clone();
        sorted.sort_unstable();
        assert_eq!(all, sorted, "not sorted");
        let mut deduped = all.clone();
        deduped.dedup();
        assert_eq!(all.len(), deduped.len(), "a code appears twice");
        assert!(all.contains(&"lao"));
        assert!(all.contains(&"khm"));
        assert!(!all.contains(&"frk"), "frk is a symlink, not a model");
    }

    #[test]
    fn the_helpers_are_published_and_are_not_languages() {
        let all = every_language();
        for helper in HELPERS {
            assert!(all.contains(&helper), "{helper} is published");
            assert!(!is_a_language(helper), "{helper} is not a language");
        }
        for code in FIRST.into_iter().chain(["khm"]) {
            assert!(is_a_language(code), "{code}");
        }
        assert_eq!(all.iter().filter(|code| is_a_language(code)).count(), 123);
    }

    #[test]
    fn a_model_states_its_licence_and_source() {
        for quality in Quality::ALL {
            let model = model("eng", quality).expect("English is offered");
            assert_eq!(model.licence(), "Apache-2.0");
            let (repository, commit) = model.source();
            assert_eq!((repository, commit), quality.repository());
            assert_eq!(commit.len(), 40);
        }
        let best = model("eng", Quality::Accurate).unwrap();
        assert_eq!(best.source().0, "tessdata_best");
        let fast = model("eng", Quality::Fast).unwrap();
        assert_eq!(fast.source().0, "tessdata_fast");
    }
}
