const AUTONYMS: [(&str, &str); 108] = [
    ("afr", "Afrikaans"),
    ("amh", "አማርኛ"),
    ("ara", "العربية"),
    ("asm", "অসমীয়া"),
    ("aze", "azərbaycan"),
    ("bel", "беларуская"),
    ("ben", "বাংলা"),
    ("bod", "བོད་སྐད་"),
    ("bos", "bosanski"),
    ("bre", "brezhoneg"),
    ("bul", "български"),
    ("cat", "català"),
    ("ces", "čeština"),
    ("chi_sim", "简体中文"),
    ("chi_sim_vert", "简体中文"),
    ("chi_tra", "繁體中文"),
    ("chi_tra_vert", "繁體中文"),
    ("chr", "ᏣᎳᎩ"),
    ("cym", "Cymraeg"),
    ("dan", "dansk"),
    ("deu", "Deutsch"),
    ("deu_latf", "Deutsch"),
    ("div", "ދިވެހި"),
    ("dzo", "རྫོང་ཁ"),
    ("ell", "Ελληνικά"),
    ("eng", "English"),
    ("epo", "esperanto"),
    ("est", "eesti"),
    ("eus", "euskara"),
    ("fao", "føroyskt"),
    ("fas", "فارسی"),
    ("fil", "Filipino"),
    ("fin", "suomi"),
    ("fra", "français"),
    ("fry", "West-Frysk"),
    ("gla", "Gàidhlig"),
    ("gle", "Gaeilge"),
    ("glg", "galego"),
    ("guj", "ગુજરાતી"),
    ("hat", "Kreyòl ayisyen"),
    ("heb", "עברית"),
    ("hin", "हिन्दी"),
    ("hrv", "hrvatski"),
    ("hun", "magyar"),
    ("hye", "հայերեն"),
    ("iku", "ᐃᓄᒃᑎᑐᑦ"),
    ("ind", "Indonesia"),
    ("isl", "íslenska"),
    ("ita", "italiano"),
    ("jpn", "日本語"),
    ("jpn_vert", "日本語"),
    ("kan", "ಕನ್ನಡ"),
    ("kat", "ქართული"),
    ("kaz", "қазақ тілі"),
    ("khm", "ខ្មែរ"),
    ("kir", "кыргызча"),
    ("kor", "한국어"),
    ("kor_vert", "한국어"),
    ("lao", "ລາວ"),
    ("lav", "latviešu"),
    ("lit", "lietuvių"),
    ("ltz", "Lëtzebuergesch"),
    ("mal", "മലയാളം"),
    ("mar", "मराठी"),
    ("mkd", "македонски"),
    ("mlt", "Malti"),
    ("mon", "монгол"),
    ("mri", "Māori"),
    ("msa", "Melayu"),
    ("mya", "မြန်မာ"),
    ("nep", "नेपाली"),
    ("nld", "Nederlands"),
    ("oci", "occitan"),
    ("ori", "ଓଡ଼ିଆ"),
    ("pol", "polski"),
    ("por", "português"),
    ("pus", "پښتو"),
    ("ron", "română"),
    ("rus", "русский"),
    ("san", "संस्कृतम्"),
    ("sin", "සිංහල"),
    ("slk", "slovenčina"),
    ("slv", "slovenščina"),
    ("snd", "سنڌي"),
    ("spa", "español"),
    ("sqi", "shqip"),
    ("srp", "српски"),
    ("srp_latn", "srpski"),
    ("sun", "Basa Sunda"),
    ("swa", "Kiswahili"),
    ("swe", "svenska"),
    ("syr", "ܣܘܪܝܝܐ"),
    ("tam", "தமிழ்"),
    ("tat", "татар"),
    ("tel", "తెలుగు"),
    ("tgk", "тоҷикӣ"),
    ("tha", "ไทย"),
    ("tir", "ትግርኛ"),
    ("ton", "lea fakatonga"),
    ("tur", "Türkçe"),
    ("uig", "ئۇيغۇرچە"),
    ("ukr", "українська"),
    ("urd", "اردو"),
    ("uzb", "o‘zbek"),
    ("uzb_cyrl", "Ўзбекча"),
    ("vie", "Tiếng Việt"),
    ("yid", "ייִדיש"),
    ("yor", "Èdè Yorùbá"),
];

#[must_use]
pub fn autonym_of(code: &str) -> Option<&'static str> {
    AUTONYMS
        .binary_search_by_key(&code, |&(c, _)| c)
        .ok()
        .map(|index| AUTONYMS[index].1)
}

#[cfg(test)]
mod tests {
    use super::{AUTONYMS, autonym_of};

    #[test]
    fn known_answers() {
        assert_eq!(autonym_of("lao"), Some("\u{0ea5}\u{0eb2}\u{0ea7}"));
        assert_eq!(autonym_of("tha"), Some("\u{0e44}\u{0e17}\u{0e22}"));
        assert_eq!(autonym_of("eng"), Some("English"));
        assert_eq!(autonym_of("enm"), None);
        assert_eq!(autonym_of("zzz"), None);
    }

    #[test]
    fn every_code_is_offered_and_the_table_is_sorted() {
        let mut sorted = AUTONYMS;
        sorted.sort_unstable_by_key(|&(c, _)| c);
        assert_eq!(AUTONYMS, sorted);
        for (code, name) in AUTONYMS {
            assert!(crate::names::name_of(code).is_some(), "{code}");
            assert!(!name.trim().is_empty(), "{code}");
        }
    }
}
