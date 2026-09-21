use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct Moment(#[cfg(not(target_arch = "wasm32"))] std::time::Instant);

impl Moment {
    pub(crate) fn now() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self(std::time::Instant::now())
        }
        #[cfg(target_arch = "wasm32")]
        {
            Self()
        }
    }

    pub(crate) fn elapsed(self) -> Duration {
        Self::now().since(self)
    }

    pub(crate) fn since(self, earlier: Self) -> Duration {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.0.saturating_duration_since(earlier.0)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = earlier;
            Duration::ZERO
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn after(self, later: Duration) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self(self.0 + later)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = later;
            self
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::Moment;

    #[test]
    fn a_span_runs_forwards() {
        let first = Moment::now();
        let second = Moment::now();
        assert!(second.since(first) <= first.elapsed());
        assert_eq!(first.since(second), std::time::Duration::ZERO);
    }

    #[test]
    fn a_moment_can_be_moved_on() {
        let now = Moment::now();
        let later = now.after(std::time::Duration::from_millis(40));
        assert_eq!(later.since(now), std::time::Duration::from_millis(40));
    }
}
