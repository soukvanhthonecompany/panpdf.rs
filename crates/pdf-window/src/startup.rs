use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

static BEGAN: OnceLock<Instant> = OnceLock::new();

static STAGES: Mutex<Vec<(&'static str, f64)>> = Mutex::new(Vec::new());

static BUILT: AtomicBool = AtomicBool::new(false);
static WRITTEN: AtomicBool = AtomicBool::new(false);

pub fn began() {
    let _ = BEGAN.set(Instant::now());
}

#[must_use]
fn since_start() -> Duration {
    BEGAN.get().map_or(Duration::ZERO, Instant::elapsed)
}

pub(crate) fn stage(named: &'static str, took: Duration) {
    if let Ok(mut stages) = STAGES.lock() {
        stages.push((named, took.as_secs_f64() * 1e3));
    }
}

pub(crate) fn reached(named: &'static str) {
    stage(named, since_start());
}

pub(crate) fn a_frame_was_built() -> bool {
    let first = !BUILT.swap(true, Ordering::Relaxed);
    if first {
        reached("built");
    }
    first
}

pub(crate) fn a_frame_began() {
    if !BUILT.load(Ordering::Relaxed) || WRITTEN.swap(true, Ordering::Relaxed) {
        return;
    }
    reached("shown");
    let Ok(stages) = STAGES.lock() else { return };
    let said = stages
        .iter()
        .map(|(named, ms)| format!("{named} {ms:.0} ms"))
        .collect::<Vec<_>>()
        .join(", ");
    crate::reporting::say(pdf_app::trouble::Kind::Session, &format!("startup: {said}"));
}

#[cfg(test)]
mod tests {
    use super::{Duration, STAGES, stage};

    #[test]
    fn a_stage_is_written_down_in_milliseconds() {
        stage("test-stage", Duration::from_millis(40));
        let stages = STAGES.lock().expect("the stages");
        let (named, ms) = stages
            .iter()
            .find(|(named, _)| *named == "test-stage")
            .expect("the stage just written");
        assert_eq!(*named, "test-stage");
        assert!((ms - 40.0).abs() < 1.0, "{ms}");
    }
}
