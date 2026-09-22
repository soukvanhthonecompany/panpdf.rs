pub const A_FRAME_MS: f64 = 1000.0 / 60.0;

pub const KEPT_BY_DEFAULT: usize = 240;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Summary {
    pub frames: usize,
    pub median: f64,
    pub ninety_fifth: f64,
    pub worst: f64,
    pub slow: usize,
}

#[derive(Clone, Debug)]
pub struct Speed {
    kept: Vec<f64>,
    next: usize,
    keeps: usize,
}

impl Default for Speed {
    fn default() -> Self {
        Self::keeping(KEPT_BY_DEFAULT)
    }
}

impl Speed {
    #[must_use]
    pub fn keeping(keeps: usize) -> Self {
        Self {
            kept: Vec::new(),
            next: 0,
            keeps: keeps.max(1),
        }
    }

    pub fn saw(&mut self, ms: f64) {
        if ms.is_nan() {
            return;
        }
        let ms = ms.max(0.0);
        if self.kept.len() < self.keeps {
            self.kept.push(ms);
            return;
        }
        self.kept[self.next] = ms;
        self.next = (self.next + 1) % self.keeps;
    }

    #[must_use]
    pub fn frames(&self) -> usize {
        self.kept.len()
    }

    pub fn forget(&mut self) {
        self.kept.clear();
        self.next = 0;
    }

    #[must_use]
    pub fn summary(&self) -> Option<Summary> {
        if self.kept.is_empty() {
            return None;
        }
        let mut sorted = self.kept.clone();
        sorted.sort_by(f64::total_cmp);
        let slow = self.kept.iter().filter(|ms| **ms > A_FRAME_MS).count();
        Some(Summary {
            frames: sorted.len(),
            median: at_rank(&sorted, 0.50),
            ninety_fifth: at_rank(&sorted, 0.95),
            worst: sorted[sorted.len() - 1],
            slow,
        })
    }
}

fn at_rank(sorted: &[f64], part: f64) -> f64 {
    let many = sorted.len();
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a window of frames is small; the rank is clamped into it"
    )]
    let rank = ((part * many as f64).ceil() as usize).clamp(1, many);
    sorted[rank - 1]
}

impl Summary {
    #[must_use]
    pub fn logged(&self) -> String {
        format!(
            "drawing: {} frames, median {:.1} ms, 95th {:.1} ms, worst {:.1} ms, {} over {:.1} ms",
            self.frames, self.median, self.ninety_fifth, self.worst, self.slow, A_FRAME_MS
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{A_FRAME_MS, Speed, Summary};

    fn about(measured: f64, wanted: f64) -> bool {
        (measured - wanted).abs() < 1e-9
    }

    fn after(frames: &[f64]) -> Summary {
        let mut speed = Speed::keeping(240);
        for ms in frames {
            speed.saw(*ms);
        }
        speed.summary().expect("frames were given")
    }

    #[test]
    fn nine_frames_answer_what_can_be_counted_on_paper() {
        let summary = after(&[3.0, 1.0, 4.0, 9.0, 2.0, 8.0, 5.0, 7.0, 6.0]);
        assert_eq!(summary.frames, 9);
        assert!(about(summary.median, 5.0), "{summary:?}");
        assert!(about(summary.ninety_fifth, 9.0), "{summary:?}");
        assert!(about(summary.worst, 9.0), "{summary:?}");
        assert_eq!(summary.slow, 0, "{summary:?}");
    }

    #[test]
    fn a_hundred_frames_answer_their_own_ranks() {
        let frames: Vec<f64> = (1..=100).map(f64::from).collect();
        let summary = after(&frames);
        assert!(about(summary.median, 50.0), "{summary:?}");
        assert!(about(summary.ninety_fifth, 95.0), "{summary:?}");
        assert!(about(summary.worst, 100.0), "{summary:?}");
        assert_eq!(summary.slow, 84, "{summary:?}");
    }

    #[test]
    fn one_slow_frame_among_fast_ones_is_noticed() {
        let mut frames = vec![8.0; 200];
        frames[137] = 100.0;
        let noticed = after(&frames);
        assert!(about(noticed.median, 8.0), "{noticed:?}");
        assert!(about(noticed.ninety_fifth, 8.0), "{noticed:?}");
        assert!(about(noticed.worst, 100.0), "{noticed:?}");
        assert_eq!(noticed.slow, 1, "{noticed:?}");

        let control = after(&vec![8.0; 200]);
        assert!(about(control.worst, 8.0), "{control:?}");
        assert_eq!(
            control.slow, 0,
            "the control must see no slow frame, or the one above proves nothing"
        );
    }

    #[test]
    fn the_window_rolls_and_drops_the_oldest_first() {
        let mut speed = Speed::keeping(10);
        for ms in 1..=20 {
            speed.saw(f64::from(ms));
        }
        let summary = speed.summary().expect("ten frames");
        assert_eq!(summary.frames, 10);
        assert!(about(summary.worst, 20.0), "{summary:?}");
        assert!(about(summary.median, 15.0), "{summary:?}");
        assert_eq!(summary.slow, 4, "frames 17, 18, 19 and 20: {summary:?}");
    }

    #[test]
    fn the_line_is_one_frame_of_a_sixty_hertz_screen() {
        let summary = after(&[A_FRAME_MS, A_FRAME_MS + 0.001, A_FRAME_MS - 0.001]);
        assert_eq!(summary.slow, 1, "{summary:?}");
    }

    #[test]
    fn no_frames_means_no_answer() {
        assert!(Speed::keeping(8).summary().is_none());
        let mut speed = Speed::keeping(8);
        speed.saw(4.0);
        speed.forget();
        assert!(speed.summary().is_none());
    }

    #[test]
    fn nonsense_from_a_clock_is_refused() {
        let mut speed = Speed::keeping(8);
        speed.saw(f64::NAN);
        assert_eq!(speed.frames(), 0);
        speed.saw(-3.0);
        let summary = speed.summary().expect("one frame");
        assert!(about(summary.worst, 0.0), "{summary:?}");
    }

    #[test]
    fn the_logged_line_is_numbers_only() {
        let line = after(&[8.0, 40.0, 9.0]).logged();
        assert!(line.contains("median"), "{line}");
        assert!(line.contains("40.0"), "{line}");
        assert!(!line.contains('/'), "no path may be in it: {line}");
        assert_eq!(line.lines().count(), 1, "{line}");
    }
}
