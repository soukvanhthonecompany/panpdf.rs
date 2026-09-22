#[must_use]
pub(crate) fn matching(families: &[String], query: &str) -> Vec<usize> {
    let whole = query.trim().to_lowercase();
    if whole.is_empty() {
        return (0..families.len()).collect();
    }
    let words: Vec<&str> = whole.split_whitespace().collect();
    let lowered: Vec<String> = families.iter().map(|name| name.to_lowercase()).collect();
    let holds_every_word = |name: &str| words.iter().all(|word| name.contains(word));
    let begins: Vec<usize> = lowered
        .iter()
        .enumerate()
        .filter(|(_, name)| name.starts_with(&whole))
        .map(|(at, _)| at)
        .collect();
    let contains: Vec<usize> = lowered
        .iter()
        .enumerate()
        .filter(|(at, name)| !begins.contains(at) && holds_every_word(name))
        .map(|(at, _)| at)
        .collect();
    begins.into_iter().chain(contains).collect()
}

#[cfg(test)]
mod tests {
    use super::matching;

    fn families() -> Vec<String> {
        [
            "C059",
            "DejaVu Math TeX Gyre",
            "DejaVu Sans",
            "DejaVu Sans Mono",
            "DejaVu Serif",
            "FreeMono",
            "FreeSans",
            "Liberation Mono",
            "Noto Sans Lao",
        ]
        .map(str::to_owned)
        .to_vec()
    }

    fn names(query: &str) -> Vec<String> {
        let families = families();
        matching(&families, query)
            .into_iter()
            .map(|at| families[at].clone())
            .collect()
    }

    fn prefix_only(query: &str) -> Vec<String> {
        families()
            .into_iter()
            .filter(|name| name.to_lowercase().starts_with(query))
            .collect()
    }

    #[test]
    fn a_word_anywhere_in_the_name_finds_it() {
        assert_eq!(
            names("mono"),
            ["DejaVu Sans Mono", "FreeMono", "Liberation Mono"]
        );
        assert!(
            prefix_only("mono").is_empty(),
            "the control must disagree, or the test is not measuring anything"
        );
        assert_eq!(
            prefix_only("free"),
            names("free"),
            "and agree where both are right"
        );
    }

    #[test]
    fn the_words_may_come_in_any_order() {
        assert_eq!(names("mono dejavu"), ["DejaVu Sans Mono"]);
        assert_eq!(names("DEJAVU sans"), ["DejaVu Sans", "DejaVu Sans Mono"]);
        assert_eq!(names("lao"), ["Noto Sans Lao"]);
    }

    #[test]
    fn a_name_that_begins_with_the_query_comes_first() {
        assert_eq!(names("free"), ["FreeMono", "FreeSans"]);
        assert_eq!(
            names("de"),
            [
                "DejaVu Math TeX Gyre",
                "DejaVu Sans",
                "DejaVu Sans Mono",
                "DejaVu Serif"
            ]
        );
        assert_eq!(
            names("s"),
            [
                "DejaVu Sans",
                "DejaVu Sans Mono",
                "DejaVu Serif",
                "FreeSans",
                "Noto Sans Lao"
            ]
        );
    }

    #[test]
    fn nothing_typed_is_everything() {
        assert_eq!(matching(&families(), ""), (0..9).collect::<Vec<_>>());
        assert_eq!(matching(&families(), "   "), (0..9).collect::<Vec<_>>());
        assert!(names("zzz").is_empty());
        assert!(matching(&[], "a").is_empty());
    }
}
