pub const KIB_PER_MIB: f64 = 1024.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reading {
    pub resident_kib: u64,
    pub highest_kib: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Summary {
    pub readings: usize,
    pub at_first_kib: u64,
    pub now_kib: u64,
    pub highest_kib: u64,
    pub grew_kib: i64,
}

#[derive(Clone, Copy, Debug)]
pub struct Held {
    first: Option<u64>,
    now: u64,
    highest: u64,
    readings: usize,
}

impl Default for Held {
    fn default() -> Self {
        Self::new()
    }
}

impl Held {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            first: None,
            now: 0,
            highest: 0,
            readings: 0,
        }
    }

    pub fn saw(&mut self, reading: Reading) {
        if self.first.is_none() {
            self.first = Some(reading.resident_kib);
        }
        self.now = reading.resident_kib;
        self.highest = self
            .highest
            .max(reading.highest_kib)
            .max(reading.resident_kib);
        self.readings += 1;
    }

    #[must_use]
    pub fn readings(&self) -> usize {
        self.readings
    }

    #[must_use]
    pub fn summary(&self) -> Option<Summary> {
        let at_first_kib = self.first?;
        Some(Summary {
            readings: self.readings,
            at_first_kib,
            now_kib: self.now,
            highest_kib: self.highest,
            grew_kib: as_signed(self.now) - as_signed(at_first_kib),
        })
    }
}

fn as_signed(kib: u64) -> i64 {
    i64::try_from(kib).unwrap_or(i64::MAX)
}

#[must_use]
pub fn as_mib(kib: u64) -> f64 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "a resident size in kibibytes is well within f64's exact integers"
    )]
    let kib = kib as f64;
    kib / KIB_PER_MIB
}

#[must_use]
pub fn as_signed_mib(kib: i64) -> f64 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "a resident size in kibibytes is well within f64's exact integers"
    )]
    let kib = kib as f64;
    kib / KIB_PER_MIB
}

#[must_use]
pub fn read_status(text: &str) -> Option<Reading> {
    let resident_kib = field(text, "VmRSS:")?;
    let highest_kib = field(text, "VmHWM:").unwrap_or(resident_kib);
    Some(Reading {
        resident_kib,
        highest_kib: highest_kib.max(resident_kib),
    })
}

fn field(text: &str, named: &str) -> Option<u64> {
    let line = text.lines().find(|line| line.starts_with(named))?;
    let mut parts = line[named.len()..].split_whitespace();
    let count = parts.next()?.parse::<u64>().ok()?;
    match parts.next() {
        Some("kB") => Some(count),
        _ => None,
    }
}

impl Summary {
    #[must_use]
    pub fn logged(&self) -> String {
        format!(
            "memory: {} readings, now {:.1} MiB, peak {:.1} MiB, since first {:+.1} MiB",
            self.readings,
            as_mib(self.now_kib),
            as_mib(self.highest_kib),
            as_signed_mib(self.grew_kib)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{Held, Reading, as_mib, as_signed_mib, read_status};

    fn after(readings: &[(u64, u64)]) -> super::Summary {
        let mut held = Held::new();
        for (resident_kib, highest_kib) in readings {
            held.saw(Reading {
                resident_kib: *resident_kib,
                highest_kib: *highest_kib,
            });
        }
        held.summary().expect("readings were given")
    }

    #[test]
    fn opening_and_closing_a_document_is_counted_by_hand() {
        let summary = after(&[(102_400, 102_400), (409_600, 409_600), (153_600, 409_600)]);
        assert_eq!(summary.readings, 3);
        assert_eq!(summary.at_first_kib, 102_400);
        assert_eq!(summary.now_kib, 153_600);
        assert_eq!(summary.highest_kib, 409_600);
        assert_eq!(summary.grew_kib, 51_200);
        assert!((as_mib(summary.now_kib) - 150.0).abs() < 1e-9);
        assert!((as_signed_mib(summary.grew_kib) - 50.0).abs() < 1e-9);
    }

    #[test]
    fn growth_it_was_given_is_growth_it_reports() {
        let flat: Vec<(u64, u64)> = (0..8).map(|_| (102_400, 102_400)).collect();
        let control = after(&flat);
        assert_eq!(control.grew_kib, 0, "{control:?}");
        assert_eq!(control.highest_kib, 102_400, "{control:?}");

        let mut grown = flat;
        for reading in grown.iter_mut().skip(4) {
            reading.0 += 204_800;
            reading.1 += 204_800;
        }
        let noticed = after(&grown);
        assert_eq!(noticed.grew_kib, 204_800, "{noticed:?}");
        assert_eq!(noticed.highest_kib, 307_200, "{noticed:?}");
    }

    #[test]
    fn a_peak_no_sample_saw_is_still_reported() {
        let summary = after(&[(102_400, 102_400), (102_400, 921_600)]);
        assert_eq!(summary.highest_kib, 921_600, "{summary:?}");
        assert_eq!(summary.grew_kib, 0, "{summary:?}");
    }

    #[test]
    fn samples_alone_make_a_peak() {
        let summary = after(&[(102_400, 0), (512_000, 0), (204_800, 0)]);
        assert_eq!(summary.highest_kib, 512_000, "{summary:?}");
    }

    #[test]
    fn no_reading_means_no_answer() {
        assert!(Held::new().summary().is_none());
        assert_eq!(Held::new().readings(), 0);
    }

    #[test]
    fn the_status_file_is_read_as_the_kernel_writes_it() {
        let text = "Name:\tpdf-desktop\n\
                    State:\tS (sleeping)\n\
                    VmPeak:\t35201028 kB\n\
                    VmSize:\t35135492 kB\n\
                    VmHWM:\t  412344 kB\n\
                    VmRSS:\t  287116 kB\n\
                    RssAnon:\t  201044 kB\n\
                    Threads:\t17\n";
        let reading = read_status(text).expect("VmRSS is there");
        assert_eq!(reading.resident_kib, 287_116);
        assert_eq!(reading.highest_kib, 412_344);
    }

    #[test]
    fn the_reader_does_not_pick_up_the_virtual_size() {
        let text = "VmPeak:\t35201028 kB\nVmSize:\t35135492 kB\nVmRSS:\t  287116 kB\n";
        let reading = read_status(text).expect("VmRSS is there");
        assert_eq!(reading.resident_kib, 287_116);
        assert_eq!(
            reading.highest_kib, 287_116,
            "no VmHWM, so the resident size"
        );
    }

    #[test]
    fn text_that_cannot_be_understood_is_refused() {
        assert!(read_status("Name:\tpdf-desktop\n").is_none());
        assert!(read_status("VmRSS:\t  many kB\n").is_none());
        assert!(read_status("VmRSS:\t  287116 pages\n").is_none());
        assert!(read_status("").is_none());
    }

    #[test]
    fn the_logged_line_is_numbers_only() {
        let line = after(&[(102_400, 102_400), (409_600, 460_800)]).logged();
        assert!(line.contains("peak"), "{line}");
        assert!(line.contains("450.0"), "{line}");
        assert!(line.contains("+300.0"), "{line}");
        assert!(!line.contains('/'), "no path may be in it: {line}");
        assert_eq!(line.lines().count(), 1, "{line}");
    }
}
