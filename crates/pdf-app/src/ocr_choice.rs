use pdf_ocr::Quality;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Choice {
    pub quality: Quality,
    pub skip_text: bool,
    pub languages: Vec<String>,
}

impl Default for Choice {
    fn default() -> Self {
        Self::fresh()
    }
}

impl Choice {
    #[must_use]
    pub const fn fresh() -> Self {
        Self {
            quality: Quality::Accurate,
            skip_text: true,
            languages: Vec::new(),
        }
    }
}

const FIELDS: usize = 3;

const YES: &str = "skip_text";
const NO: &str = "read_every_page";

#[must_use]
pub fn read(text: &str) -> Option<Choice> {
    let line = text.lines().next()?;
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.is_empty() || fields.len() > FIELDS {
        return None;
    }
    let quality = Quality::parse(fields[0])?;
    let skip_text = match fields.get(1) {
        None | Some(&"") => Choice::fresh().skip_text,
        Some(&YES) => true,
        Some(&NO) => false,
        Some(_) => return None,
    };
    let languages = match fields.get(2) {
        None | Some(&"") => Vec::new(),
        Some(list) => {
            let codes: Vec<String> = list.split('+').map(str::to_owned).collect();
            let is_code = |code: &String| {
                !code.is_empty()
                    && code
                        .chars()
                        .all(|letter| letter.is_ascii_alphanumeric() || letter == '_')
            };
            if !codes.iter().all(is_code) {
                return None;
            }
            codes
        }
    };
    Some(Choice {
        quality,
        skip_text,
        languages,
    })
}

#[must_use]
pub fn write(choice: &Choice) -> String {
    format!(
        "{}\t{}\t{}\n",
        choice.quality.as_str(),
        if choice.skip_text { YES } else { NO },
        choice.languages.join("+")
    )
}

#[cfg(test)]
mod tests {
    use super::{Choice, read, write};
    use pdf_ocr::Quality;

    #[test]
    fn a_choice_survives_being_written_down() {
        let lists: [&[&str]; 3] = [&[], &["lao", "eng"], &["fra", "chi_sim", "tha"]];
        for quality in Quality::ALL {
            for skip_text in [true, false] {
                for list in lists {
                    let choice = Choice {
                        quality,
                        skip_text,
                        languages: list.iter().map(|code| (*code).to_owned()).collect(),
                    };
                    assert_eq!(read(&write(&choice)), Some(choice.clone()), "{choice:?}");
                }
            }
        }
        assert_eq!(write(&Choice::fresh()), "accurate\tskip_text\t\n");
        assert_eq!(
            read("fast\tread_every_page\n"),
            Some(Choice {
                quality: Quality::Fast,
                skip_text: false,
                languages: Vec::new(),
            })
        );
    }

    #[test]
    fn a_line_from_before_the_tick_still_reads() {
        assert_eq!(
            read("fast\n"),
            Some(Choice {
                quality: Quality::Fast,
                skip_text: true,
                languages: Vec::new(),
            })
        );
        assert_eq!(read("accurate"), Some(Choice::fresh()));
        assert_eq!(read("accurate\tskip_text\n"), Some(Choice::fresh()));
    }

    #[test]
    fn the_languages_ticked_are_remembered_in_order() {
        let read_back = read("fast\tskip_text\ttha+lao+eng\n").expect("a choice");
        assert_eq!(read_back.languages, ["tha", "lao", "eng"]);
        assert_eq!(read("fast\tskip_text\tlao eng\n"), None);
        assert_eq!(read("fast\tskip_text\tscript/Lao\n"), None);
        assert_eq!(read("fast\tskip_text\tlao++eng\n"), None);
    }

    #[test]
    fn what_cannot_be_read_faithfully_is_not_read() {
        assert_eq!(read(""), None);
        assert_eq!(read("best\n"), None);
        assert_eq!(read("accurate\tmaybe\n"), None);
        assert_eq!(read("accurate\tskip_text\tlao\tsomething\n"), None);
        assert_eq!(read("\tskip_text\n"), None);
        assert_eq!(Choice::default(), Choice::fresh());
    }
}
