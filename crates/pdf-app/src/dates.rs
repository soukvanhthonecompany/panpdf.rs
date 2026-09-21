const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

#[must_use]
pub fn reads_as_a_date(format: &str, text: &str) -> bool {
    if format.is_empty() {
        return true;
    }
    let Some((day, month, year)) = parts(format, text) else {
        return false;
    };
    let long = matches!(month, 1 | 3 | 5 | 7 | 8 | 10 | 12);
    let february = month == 2;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = if february {
        if leap { 29 } else { 28 }
    } else if long {
        31
    } else {
        30
    };
    (1..=days).contains(&day)
}

fn parts(format: &str, text: &str) -> Option<(u32, u32, i32)> {
    let (mut day, mut month, mut year) = (1, 1, 2000);
    let mut left = text;
    let mut rest = format;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("yyyy") {
            year = number(&mut left, 4, 4)?.try_into().ok()?;
            rest = after;
        } else if let Some(after) = rest.strip_prefix("yy") {
            year = i32::try_from(number(&mut left, 2, 2)?).ok()? + 2000;
            rest = after;
        } else if let Some(after) = rest.strip_prefix("mmm") {
            let name = left.get(..3)?.to_ascii_lowercase();
            month = u32::try_from(MONTHS.iter().position(|known| *known == name)? + 1).ok()?;
            left = left.get(3..)?;
            rest = after;
        } else if let Some(after) = rest.strip_prefix("mm") {
            month = number(&mut left, 2, 2)?;
            rest = after;
        } else if let Some(after) = rest.strip_prefix('m') {
            month = number(&mut left, 1, 2)?;
            rest = after;
        } else if let Some(after) = rest.strip_prefix("dd") {
            day = number(&mut left, 2, 2)?;
            rest = after;
        } else if let Some(after) = rest.strip_prefix('d') {
            day = number(&mut left, 1, 2)?;
            rest = after;
        } else {
            let mark = rest.chars().next()?;
            left = left.strip_prefix(mark)?;
            rest = &rest[mark.len_utf8()..];
        }
    }
    left.is_empty().then_some((day, month, year))
}

fn number(left: &mut &str, least: usize, most: usize) -> Option<u32> {
    let digits = left
        .chars()
        .take(most)
        .take_while(char::is_ascii_digit)
        .count();
    if digits < least {
        return None;
    }
    let (head, tail) = left.split_at(digits);
    *left = tail;
    head.parse().ok()
}

#[must_use]
pub fn written_day(seconds: u64) -> String {
    let Moment {
        year, month, day, ..
    } = moment(seconds);
    format!("{day:02}/{month:02}/{year}")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Moment {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

#[must_use]
pub fn moment(seconds: u64) -> Moment {
    let days = i64::try_from(seconds / 86_400).unwrap_or(i64::MAX / 2) + 719_468;
    let rest = seconds % 86_400;
    let era = days.div_euclid(146_097);
    let of_era = days.rem_euclid(146_097);
    let year_of_era = (of_era - of_era / 1_460 + of_era / 36_524 - of_era / 146_096) / 365;
    let day_of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "each part is bounded by the calendar, and none of them is negative"
    )]
    Moment {
        year: year as i32,
        month: month as u8,
        day: day as u8,
        hour: (rest / 3_600) as u8,
        minute: ((rest % 3_600) / 60) as u8,
        second: (rest % 60) as u8,
    }
}

#[cfg(test)]
mod tests {
    use super::{moment, reads_as_a_date};

    #[test]
    fn a_moment_becomes_the_day_it_is() {
        let epoch = moment(0);
        assert_eq!((epoch.year, epoch.month, epoch.day), (1970, 1, 1));
        assert_eq!((epoch.hour, epoch.minute, epoch.second), (0, 0, 0));

        let leap = moment(1_709_209_800);
        assert_eq!((leap.year, leap.month, leap.day), (2024, 2, 29));
        assert_eq!((leap.hour, leap.minute), (12, 30));

        let century = moment(4_102_444_800);
        assert_eq!((century.year, century.month, century.day), (2100, 1, 1));
        let before = moment(4_102_444_800 - 86_400);
        assert_eq!((before.year, before.month, before.day), (2099, 12, 31));
    }

    #[test]
    fn each_format_reads_its_own_dates() {
        assert!(reads_as_a_date("dd/mm/yyyy", "09/09/2026"));
        assert!(reads_as_a_date("mm/dd/yyyy", "09/30/2026"));
        assert!(reads_as_a_date("yyyy-mm-dd", "2026-09-09"));
        assert!(reads_as_a_date("d mmm yyyy", "9 Sep 2026"));
        assert!(!reads_as_a_date("dd/mm/yyyy", "2026-09-09"));
        assert!(!reads_as_a_date("yyyy-mm-dd", "09/09/2026"));
        assert!(!reads_as_a_date("d mmm yyyy", "9 Sept 2026"));
    }

    #[test]
    fn a_date_has_to_be_a_day_there_is() {
        assert!(!reads_as_a_date("dd/mm/yyyy", "31/02/2026"));
        assert!(!reads_as_a_date("dd/mm/yyyy", "31/04/2026"));
        assert!(reads_as_a_date("dd/mm/yyyy", "31/03/2026"));
        assert!(reads_as_a_date("dd/mm/yyyy", "29/02/2024"));
        assert!(!reads_as_a_date("dd/mm/yyyy", "29/02/2026"));
        assert!(
            !reads_as_a_date("dd/mm/yyyy", "29/02/1900"),
            "a century is not a leap year"
        );
        assert!(
            reads_as_a_date("dd/mm/yyyy", "29/02/2000"),
            "unless it divides by four hundred"
        );
    }

    #[test]
    fn what_is_not_a_date_is_refused() {
        assert!(!reads_as_a_date("dd/mm/yyyy", ""));
        assert!(!reads_as_a_date("dd/mm/yyyy", "tomorrow"));
        assert!(!reads_as_a_date("dd/mm/yyyy", "09/09/2026 and later"));
        assert!(
            !reads_as_a_date("dd/mm/yyyy", "9/9/2026"),
            "two digits are asked for"
        );
        assert!(reads_as_a_date("d/m/yyyy", "9/9/2026"));
        assert!(reads_as_a_date("", "whatever"));
    }

    #[test]
    fn a_day_is_written_as_the_calendar_has_it() {
        use super::written_day;
        assert_eq!(written_day(0), "01/01/1970");
        assert_eq!(written_day(951_782_400), "29/02/2000");
        assert_eq!(written_day(4_107_542_400), "01/03/2100");
        assert_eq!(written_day(1_789_689_600), "18/09/2026");
        assert_eq!(written_day(1_789_689_600 + 86_399), "18/09/2026");
    }
}
