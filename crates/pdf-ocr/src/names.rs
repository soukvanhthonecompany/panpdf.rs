const NAMES: [(&str, &str); 125] = [
    ("afr", "Afrikaans"),
    ("amh", "Amharic"),
    ("ara", "Arabic"),
    ("asm", "Assamese"),
    ("aze", "Azerbaijani"),
    ("aze_cyrl", "Azerbaijani (Cyrillic)"),
    ("bel", "Belarusian"),
    ("ben", "Bengali"),
    ("bod", "Tibetan"),
    ("bos", "Bosnian"),
    ("bre", "Breton"),
    ("bul", "Bulgarian"),
    ("cat", "Catalan; Valencian"),
    ("ceb", "Cebuano"),
    ("ces", "Czech"),
    ("chi_sim", "Chinese (Simplified)"),
    ("chi_sim_vert", "Chinese (Simplified, vertical)"),
    ("chi_tra", "Chinese (Traditional)"),
    ("chi_tra_vert", "Chinese (Traditional, vertical)"),
    ("chr", "Cherokee"),
    ("cos", "Corsican"),
    ("cym", "Welsh"),
    ("dan", "Danish"),
    ("deu", "German"),
    ("deu_latf", "German (Fraktur Latin)"),
    ("div", "Divehi; Dhivehi; Maldivian"),
    ("dzo", "Dzongkha"),
    ("ell", "Greek, Modern (1453-)"),
    ("eng", "English"),
    ("enm", "English, Middle (1100-1500)"),
    ("epo", "Esperanto"),
    ("equ", "Math / equations"),
    ("est", "Estonian"),
    ("eus", "Basque"),
    ("fao", "Faroese"),
    ("fas", "Persian"),
    ("fil", "Filipino (old - Tagalog)"),
    ("fin", "Finnish"),
    ("fra", "French"),
    ("frm", "French, Middle (ca.1400-1600)"),
    ("fry", "Western Frisian"),
    ("gla", "Scottish Gaelic"),
    ("gle", "Irish"),
    ("glg", "Galician"),
    ("grc", "Greek, Ancient (to 1453) (contrib)"),
    ("guj", "Gujarati"),
    ("hat", "Haitian; Haitian Creole"),
    ("heb", "Hebrew"),
    ("hin", "Hindi"),
    ("hrv", "Croatian"),
    ("hun", "Hungarian"),
    ("hye", "Armenian"),
    ("iku", "Inuktitut"),
    ("ind", "Indonesian"),
    ("isl", "Icelandic"),
    ("ita", "Italian"),
    ("ita_old", "Italian (Old)"),
    ("jav", "Javanese"),
    ("jpn", "Japanese"),
    ("jpn_vert", "Japanese (vertical)"),
    ("kan", "Kannada"),
    ("kat", "Georgian"),
    ("kat_old", "Georgian (Old)"),
    ("kaz", "Kazakh"),
    ("khm", "Central Khmer"),
    ("kir", "Kirghiz; Kyrgyz"),
    ("kmr", "Kurmanji (Kurdish - Latin Script)"),
    ("kor", "Korean"),
    ("kor_vert", "Korean (vertical)"),
    ("lao", "Lao"),
    ("lat", "Latin"),
    ("lav", "Latvian"),
    ("lit", "Lithuanian"),
    ("ltz", "Luxembourgish"),
    ("mal", "Malayalam"),
    ("mar", "Marathi"),
    ("mkd", "Macedonian"),
    ("mlt", "Maltese"),
    ("mon", "Mongolian"),
    ("mri", "Maori"),
    ("msa", "Malay"),
    ("mya", "Burmese"),
    ("nep", "Nepali"),
    ("nld", "Dutch; Flemish"),
    ("nor", "Norwegian"),
    ("oci", "Occitan (post 1500)"),
    ("ori", "Oriya"),
    ("osd", "Orientation and script detection"),
    ("pan", "Panjabi; Punjabi"),
    ("pol", "Polish"),
    ("por", "Portuguese"),
    ("pus", "Pushto; Pashto"),
    ("que", "Quechua"),
    ("ron", "Romanian; Moldavian; Moldovan"),
    ("rus", "Russian"),
    ("san", "Sanskrit"),
    ("sin", "Sinhala; Sinhalese"),
    ("slk", "Slovak"),
    ("slv", "Slovenian"),
    ("snd", "Sindhi"),
    ("spa", "Spanish; Castilian"),
    ("spa_old", "Spanish; Castilian (Old)"),
    ("sqi", "Albanian"),
    ("srp", "Serbian"),
    ("srp_latn", "Serbian (Latin)"),
    ("sun", "Sundanese"),
    ("swa", "Swahili"),
    ("swe", "Swedish"),
    ("syr", "Syriac"),
    ("tam", "Tamil"),
    ("tat", "Tatar"),
    ("tel", "Telugu"),
    ("tgk", "Tajik"),
    ("tha", "Thai"),
    ("tir", "Tigrinya"),
    ("ton", "Tonga"),
    ("tur", "Turkish"),
    ("uig", "Uighur; Uyghur"),
    ("ukr", "Ukrainian"),
    ("urd", "Urdu"),
    ("uzb", "Uzbek"),
    ("uzb_cyrl", "Uzbek (Cyrillic)"),
    ("vie", "Vietnamese"),
    ("yid", "Yiddish"),
    ("yor", "Yoruba"),
];

#[must_use]
pub fn name_of(code: &str) -> Option<&'static str> {
    NAMES
        .binary_search_by_key(&code, |&(c, _)| c)
        .ok()
        .map(|index| NAMES[index].1)
}

#[cfg(test)]
mod tests {
    use super::{NAMES, name_of};
    use crate::catalogue::ROWS;

    #[test]
    fn known_answers() {
        assert_eq!(name_of("chi_sim"), Some("Chinese (Simplified)"));
        assert_eq!(name_of("aze_cyrl"), Some("Azerbaijani (Cyrillic)"));
        assert_eq!(name_of("lao"), Some("Lao"));
        assert_eq!(name_of("tha"), Some("Thai"));
        assert_eq!(name_of("eng"), Some("English"));
        assert_eq!(
            name_of("frk"),
            None,
            "frk is not in the catalogue: see catalogue.rs"
        );
        assert_eq!(name_of("osd"), Some("Orientation and script detection"));
        assert_eq!(name_of("div"), Some("Divehi; Dhivehi; Maldivian"));
        assert_eq!(name_of("jpn_vert"), Some("Japanese (vertical)"));
    }

    #[test]
    fn an_unknown_code_is_refused() {
        assert_eq!(name_of("zzz"), None);
        assert_eq!(name_of(""), None);
        assert_eq!(name_of("LAO"), None);
        assert_eq!(name_of("Lao"), None);
    }

    #[test]
    fn every_catalogue_code_has_a_name() {
        for model in &ROWS {
            let name = name_of(model.code);
            assert!(name.is_some(), "{} has no name", model.code);
            assert_ne!(
                name,
                Some(model.code),
                "{} is named by its code",
                model.code
            );
        }
    }

    #[test]
    fn the_table_is_sorted_by_code() {
        let mut sorted = NAMES;
        sorted.sort_unstable_by_key(|&(c, _)| c);
        assert_eq!(NAMES, sorted);
    }
}
