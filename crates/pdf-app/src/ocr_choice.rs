use pdf_ocr::Quality;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Choice {
    pub quality: Quality,
    pub skip_text: bool,
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
        }
    }
}

const FIELDS: usize = 2;

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
    Some(Choice { quality, skip_text })
}

#[must_use]
pub fn write(choice: &Choice) -> String {
    format!(
        "{}\t{}\n",
        choice.quality.as_str(),
        if choice.skip_text { YES } else { NO }
    )
}

#[cfg(test)]
mod tests {
    use super::{Choice, read, write};
    use pdf_ocr::Quality;

    #[test]
    fn a_choice_survives_being_written_down() {
        for quality in Quality::ALL {
            for skip_text in [true, false] {
                let choice = Choice { quality, skip_text };
                assert_eq!(read(&write(&choice)), Some(choice), "{choice:?}");
            }
        }
        assert_eq!(write(&Choice::fresh()), "accurate\tskip_text\n");
        assert_eq!(
            read("fast\tread_every_page\n"),
            Some(Choice {
                quality: Quality::Fast,
                skip_text: false
            })
        );
    }

    #[test]
    fn a_line_from_before_the_tick_still_reads() {
        assert_eq!(
            read("fast\n"),
            Some(Choice {
                quality: Quality::Fast,
                skip_text: true
            })
        );
        assert_eq!(read("accurate"), Some(Choice::fresh()));
    }

    #[test]
    fn what_cannot_be_read_faithfully_is_not_read() {
        assert_eq!(read(""), None);
        assert_eq!(read("best\n"), None);
        assert_eq!(read("accurate\tmaybe\n"), None);
        assert_eq!(read("accurate\tskip_text\tsomething\n"), None);
        assert_eq!(read("\tskip_text\n"), None);
        assert_eq!(Choice::default(), Choice::fresh());
    }
}
