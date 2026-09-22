use std::sync::Mutex;

use pdf_app::footprint::{Held, Reading};

static HELD: Mutex<Held> = Mutex::new(Held::new());

static WRITTEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(target_os = "linux")]
fn reading() -> Option<Reading> {
    let text = std::fs::read_to_string("/proc/self/status").ok()?;
    pdf_app::footprint::read_status(&text)
}

#[cfg(not(target_os = "linux"))]
fn reading() -> Option<Reading> {
    None
}

pub(crate) fn sample() {
    let Some(reading) = reading() else { return };
    if let Ok(mut held) = HELD.lock() {
        held.saw(reading);
    }
}

pub(crate) fn log_what_was_held() {
    sample();
    if WRITTEN.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let Ok(held) = HELD.lock() else { return };
    if let Some(summary) = held.summary() {
        crate::reporting::say(pdf_app::trouble::Kind::Session, &summary.logged());
    }
}
