use pdf_ocr::models::{FIRST, is_a_language};

use crate::wording::Message;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Language {
    pub code: &'static str,
    pub english: &'static str,
    pub autonym: Option<&'static str>,
    pub reads: bool,
}

impl Language {
    #[must_use]
    pub fn of(code: &str) -> Option<Self> {
        let english = pdf_ocr::names::name_of(code)?;
        let code = pdf_ocr::models::of_language(code).first()?.code;
        Some(Self {
            code,
            english,
            autonym: pdf_ocr::autonyms::autonym_of(code),
            reads: is_a_language(code),
        })
    }

    #[must_use]
    pub fn shown_autonym(&self) -> Option<&'static str> {
        self.autonym.filter(|own| {
            own.to_lowercase() != self.english.to_lowercase() && own.chars().all(draws_unshaped)
        })
    }
}

#[must_use]
pub const fn draws_unshaped(letter: char) -> bool {
    matches!(letter as u32,
        0x0020..=0x024F
        | 0x02B0..=0x036F
        | 0x0370..=0x03FF
        | 0x0400..=0x052F
        | 0x0530..=0x058F
        | 0x0E00..=0x0EFF
        | 0x10A0..=0x10FF
        | 0x1200..=0x137F
        | 0x1E00..=0x1FFF
        | 0x2010..=0x201F
        | 0x3000..=0x30FF
        | 0x4E00..=0x9FFF
        | 0xAC00..=0xD7AF
    )
}

#[must_use]
pub fn catalogue() -> Vec<Language> {
    let mut all: Vec<Language> = pdf_ocr::models::every_language()
        .into_iter()
        .filter_map(Language::of)
        .collect();
    all.sort_by(|one, other| {
        (!one.reads, one.english.to_lowercase(), one.code).cmp(&(
            !other.reads,
            other.english.to_lowercase(),
            other.code,
        ))
    });
    all
}

#[must_use]
pub fn search<'a>(list: &'a [Language], query: &str) -> Vec<&'a Language> {
    let query = query.trim().to_lowercase();
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.is_empty() {
        return list.iter().collect();
    }
    let mut found: Vec<(u8, usize, &Language)> = list
        .iter()
        .enumerate()
        .filter_map(|(at, language)| {
            let names: Vec<String> = [Some(language.english), language.autonym]
                .into_iter()
                .flatten()
                .map(str::to_lowercase)
                .collect();
            let every_word = words.iter().all(|word| {
                language.code.contains(word) || names.iter().any(|name| name.contains(word))
            });
            if !every_word {
                return None;
            }
            let rank = if language.code == query {
                0
            } else if names.iter().any(|name| name.starts_with(&query)) {
                1
            } else if names.iter().any(|name| {
                name.split(|letter: char| !letter.is_alphanumeric())
                    .any(|part| part.starts_with(words[0]))
            }) {
                2
            } else {
                3
            };
            Some((rank, at, language))
        })
        .collect();
    found.sort_by_key(|&(rank, at, _)| (rank, at));
    found.into_iter().map(|(_, _, language)| language).collect()
}

#[must_use]
pub fn yours(here: &[String], ticked: &[String]) -> Vec<String> {
    let mut rest: Vec<&String> = here
        .iter()
        .chain(ticked)
        .filter(|code| !FIRST.contains(&code.as_str()) && is_a_language(code))
        .collect();
    rest.sort_by_key(|code| {
        (
            pdf_ocr::names::name_of(code).map(str::to_lowercase),
            code.as_str(),
        )
    });
    rest.dedup();
    FIRST
        .iter()
        .map(|code| (*code).to_owned())
        .chain(rest.into_iter().cloned())
        .collect()
}

#[must_use]
pub fn ticks(remembered: &[String], here: &[String]) -> Vec<String> {
    let has = |code: &String| here.contains(code) && is_a_language(code);
    let mut ticked: Vec<String> = remembered
        .iter()
        .filter(|code| has(code))
        .cloned()
        .collect();
    if ticked.is_empty() {
        ticked = FIRST
            .iter()
            .map(|code| (*code).to_owned())
            .filter(|code| has(code))
            .collect();
    }
    if ticked.is_empty() {
        ticked = here.iter().filter(|code| has(code)).cloned().collect();
    }
    in_order(&ticked, here)
}

#[must_use]
pub fn in_order(ticked: &[String], here: &[String]) -> Vec<String> {
    yours(here, ticked)
        .into_iter()
        .filter(|code| ticked.contains(code))
        .collect()
}

pub fn one_place(own: &[String], system: &[String], chosen: &[String]) -> Result<(), Vec<String>> {
    let all_in = |dir: &[String]| chosen.iter().all(|code| dir.contains(code));
    if chosen.is_empty() || all_in(own) || all_in(system) {
        return Ok(());
    }
    Err(chosen
        .iter()
        .filter(|code| !own.contains(code))
        .cloned()
        .collect())
}

#[must_use]
pub fn how_well(chosen: &[String], quality: pdf_ocr::Quality) -> Vec<Message> {
    let first: Vec<String>;
    let codes = if chosen.is_empty() {
        first = FIRST.iter().map(|code| (*code).to_owned()).collect();
        &first
    } else {
        chosen
    };
    codes
        .iter()
        .map(
            |code| match pdf_ocr::models::model(code, quality).and_then(|model| model.cer) {
                Some(cer) => Message::OcrErrorRate {
                    code: code.clone(),
                    cer,
                },
                None => Message::OcrNotMeasured(code.clone()),
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        Language, Message, catalogue, draws_unshaped, how_well, in_order, one_place, search, ticks,
        yours,
    };
    use pdf_ocr::Quality;

    fn codes(list: &[&Language]) -> Vec<&'static str> {
        list.iter().map(|language| language.code).collect()
    }

    fn owned(codes: &[&str]) -> Vec<String> {
        codes.iter().map(|code| (*code).to_owned()).collect()
    }

    #[test]
    fn the_list_is_the_whole_catalogue_by_name() {
        let all = catalogue();
        assert_eq!(all.len(), 125);
        let mut unique = codes(&all.iter().collect::<Vec<_>>());
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), 125);
        assert_eq!(all[0].code, "afr");
        assert_eq!(all[122].code, "yor");
        assert_eq!(
            all[123..].iter().map(|l| l.code).collect::<Vec<_>>(),
            ["equ", "osd"]
        );
        assert!(all[..123].iter().all(|language| language.reads));
        assert!(all[123..].iter().all(|language| !language.reads));
        let at = |code: &str| all.iter().position(|l| l.code == code).expect(code);
        assert!(at("chi_sim") < at("ces"), "Chinese before Czech, by name");
    }

    #[test]
    fn a_search_finds_a_language_every_way_it_is_named() {
        let all = catalogue();
        let first = |query: &str| search(&all, query).first().map(|l| l.code);
        assert_eq!(first("lao"), Some("lao"));
        assert_eq!(first("\u{0ea5}\u{0eb2}\u{0ea7}"), Some("lao"));
        assert_eq!(first("\u{0e44}\u{0e17}\u{0e22}"), Some("tha"));
        assert_eq!(first("THAI"), Some("tha"));
        assert_eq!(first("fra"), Some("fra"));
        assert_eq!(first("fran\u{e7}ais"), Some("fra"));
        assert_eq!(first("French"), Some("fra"));
        assert_eq!(first("\u{65e5}\u{672c}\u{8a9e}"), Some("jpn"));
        assert_eq!(first("  deutsch "), Some("deu"));
        assert!(!codes(&search(&all, "lao")).contains(&"tha"));
        assert!(search(&all, "qqqq").is_empty());
        assert_eq!(search(&all, "").len(), 125);
        assert_eq!(search(&all, "   ").len(), 125);
    }

    #[test]
    fn every_word_counts_and_a_start_ranks_first() {
        let all = catalogue();
        let found = codes(&search(&all, "chinese trad"));
        assert_eq!(found, ["chi_tra", "chi_tra_vert"]);
        let german = codes(&search(&all, "ger"));
        assert_eq!(german.first(), Some(&"deu"));
        let bian = codes(&search(&all, "bian"));
        assert!(bian.contains(&"srp"));
        let arabic = codes(&search(&all, "arabic"));
        assert_eq!(arabic, ["ara"]);
    }

    #[test]
    fn an_own_name_is_shown_only_where_it_draws_right() {
        let of = |code: &str| Language::of(code).expect(code);
        assert_eq!(of("lao").shown_autonym(), Some("\u{0ea5}\u{0eb2}\u{0ea7}"));
        assert_eq!(of("tha").shown_autonym(), Some("\u{0e44}\u{0e17}\u{0e22}"));
        assert_eq!(of("fra").shown_autonym(), Some("fran\u{e7}ais"));
        assert_eq!(of("eng").shown_autonym(), None);
        assert_eq!(of("ara").shown_autonym(), None);
        assert!(of("ara").autonym.is_some());
        assert_eq!(of("hin").shown_autonym(), None);
        let all = catalogue();
        assert_eq!(
            search(
                &all,
                "\u{0627}\u{0644}\u{0639}\u{0631}\u{0628}\u{064a}\u{0629}"
            )[0]
            .code,
            "ara"
        );
        assert!(!draws_unshaped('\u{0627}'));
        assert!(!draws_unshaped('\u{094d}'));
        assert!(draws_unshaped('a'));
        assert!(draws_unshaped('\u{0e81}'));
        assert_eq!(Language::of("zzz"), None);
    }

    #[test]
    fn yours_is_the_first_three_then_the_rest_by_name() {
        let here = owned(&["eng", "fra", "khm", "osd", "fra"]);
        assert_eq!(
            yours(&here, &owned(&["deu"])),
            owned(&["lao", "tha", "eng", "khm", "fra", "deu"])
        );
        assert_eq!(yours(&[], &[]), owned(&["lao", "tha", "eng"]));
    }

    #[test]
    fn ticks_follow_what_was_remembered_and_what_is_here() {
        let here = owned(&["eng", "fra", "lao", "osd"]);
        assert_eq!(
            ticks(&owned(&["fra", "lao"]), &here),
            owned(&["lao", "fra"])
        );
        assert_eq!(ticks(&owned(&["deu"]), &here), owned(&["lao", "eng"]));
        assert_eq!(ticks(&[], &here), owned(&["lao", "eng"]));
        assert_eq!(ticks(&[], &owned(&["fra", "osd"])), owned(&["fra"]));
        assert_eq!(
            ticks(&owned(&["osd"]), &owned(&["osd"])),
            Vec::<String>::new()
        );
        assert_eq!(ticks(&[], &[]), Vec::<String>::new());
        assert_eq!(
            in_order(&owned(&["fra", "tha"]), &owned(&["fra", "tha"])),
            owned(&["tha", "fra"])
        );
    }

    #[test]
    fn a_rate_is_given_only_where_one_was_measured() {
        let chosen = owned(&["lao", "fra"]);
        assert_eq!(
            how_well(&chosen, Quality::Accurate),
            [
                Message::OcrErrorRate {
                    code: "lao".to_owned(),
                    cer: 9.0
                },
                Message::OcrNotMeasured("fra".to_owned()),
            ]
        );
        assert_eq!(
            how_well(&chosen, Quality::Fast)[0],
            Message::OcrErrorRate {
                code: "lao".to_owned(),
                cer: 11.6
            }
        );
        let first = how_well(&[], Quality::Accurate);
        assert_eq!(first.len(), 3);
        assert!(
            first
                .iter()
                .all(|said| matches!(said, Message::OcrErrorRate { .. }))
        );
        assert_eq!(
            how_well(&owned(&["frk"]), Quality::Fast),
            [Message::OcrNotMeasured("frk".to_owned())]
        );
    }

    #[test]
    fn a_choice_split_between_two_places_names_what_to_download() {
        let own = owned(&["fra"]);
        let system = owned(&["lao", "tha", "eng"]);
        assert_eq!(one_place(&own, &system, &owned(&["lao", "eng"])), Ok(()));
        assert_eq!(one_place(&own, &system, &owned(&["fra"])), Ok(()));
        assert_eq!(
            one_place(&own, &system, &owned(&["lao", "fra", "eng"])),
            Err(owned(&["lao", "eng"]))
        );
        assert_eq!(one_place(&own, &system, &[]), Ok(()));
    }
}
