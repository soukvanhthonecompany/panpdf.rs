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

    const fn repository(self) -> (&'static str, &'static str) {
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
    pub cer: f32,
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
}

pub const LANGUAGES: [&str; 3] = ["lao", "tha", "eng"];

const CATALOGUE: [Model; 6] = [
    Model {
        code: "lao",
        quality: Quality::Accurate,
        bytes: 13_532_551,
        blob: "bdd2715bd1f76852b9430997eef890495e8637ef",
        cer: 9.0,
    },
    Model {
        code: "tha",
        quality: Quality::Accurate,
        bytes: 7_614_571,
        blob: "b975c85f60f82febe6f8c5a52a3eb74f3ceb01b9",
        cer: 3.4,
    },
    Model {
        code: "eng",
        quality: Quality::Accurate,
        bytes: 15_400_601,
        blob: "176dc3220de7db34d3b3aecbfa42043a6038348b",
        cer: 0.4,
    },
    Model {
        code: "lao",
        quality: Quality::Fast,
        bytes: 6_386_744,
        blob: "10bd41ae9352889f85a489eb3183eb0cbebc2516",
        cer: 11.6,
    },
    Model {
        code: "tha",
        quality: Quality::Fast,
        bytes: 1_072_600,
        blob: "ea28de3985668cac00d261c5b91f023ea66e19cd",
        cer: 3.3,
    },
    Model {
        code: "eng",
        quality: Quality::Fast,
        bytes: 4_113_088,
        blob: "bbef4675053b5b468cdb477053e28b1c698ba08e",
        cer: 0.4,
    },
];

#[must_use]
pub fn model(code: &str, quality: Quality) -> Option<&'static Model> {
    CATALOGUE
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

#[cfg(test)]
mod tests {
    use super::{CATALOGUE, LANGUAGES, Quality, model, of_language};

    #[test]
    fn the_catalogue_says_what_it_was_asked() {
        for code in LANGUAGES {
            for quality in Quality::ALL {
                let model = model(code, quality).expect("every language, both qualities");
                assert_eq!(model.code, code);
                assert_eq!(model.quality, quality);
                assert!(model.bytes > 1_000_000, "{code} {quality:?}");
                assert_eq!(model.blob.len(), 40);
                assert!(model.cer > 0.0 && model.cer < 50.0);
                assert!(model.url().ends_with(&format!("/{code}.traineddata")));
            }
        }
        let best = model("lao", Quality::Accurate).expect("Lao, accurate");
        assert!(best.url().contains("tessdata_best"));
        assert!((best.cer - 9.0).abs() < f32::EPSILON);
        let fast = model("lao", Quality::Fast).expect("Lao, fast");
        assert!(fast.url().contains("tessdata_fast"));
        assert!((fast.cer - 11.6).abs() < f32::EPSILON);
        assert!(fast.bytes < best.bytes);
        assert!(fast.cer > best.cer);
        assert_eq!(model("khm", Quality::Accurate), None);
        assert_eq!(model("", Quality::Fast), None);
        assert_eq!(model("LAO", Quality::Accurate), None);
        assert_eq!(of_language("khm"), Vec::<&super::Model>::new());
        assert_eq!(of_language("tha").len(), 2);
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
        for (at, one) in CATALOGUE.iter().enumerate() {
            for other in &CATALOGUE[at + 1..] {
                assert!(
                    one.code != other.code || one.quality != other.quality,
                    "{} {:?} is in the table twice",
                    one.code,
                    one.quality
                );
                assert_ne!(one.blob, other.blob, "two rows share a digest");
                assert_ne!(one.url(), other.url());
            }
        }
    }
}
