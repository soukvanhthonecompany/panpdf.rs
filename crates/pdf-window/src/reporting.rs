use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use pdf_app::own_files::{self, Platform};
use pdf_app::trouble::{self, Facts, Kind};

pub(crate) const REPOSITORY: &str = "soukvanhthonecompany/panpdf.rs";

const MOST_LOG: usize = 256 * 1024;

const NAMED: &str = "log.txt";

static LOG: OnceLock<Mutex<Option<Open>>> = OnceLock::new();

static RENDERER: OnceLock<String> = OnceLock::new();

struct Open {
    file: File,
    path: PathBuf,
    started: Instant,
    home: Option<String>,
}

struct Places {
    appdata: Option<PathBuf>,
    home: Option<PathBuf>,
    xdg_state: Option<PathBuf>,
}

impl Places {
    fn read() -> Self {
        let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
        Self {
            appdata: var("APPDATA"),
            home: var("HOME").or_else(|| var("USERPROFILE")),
            xdg_state: var("XDG_STATE_HOME"),
        }
    }

    fn env(&self) -> own_files::Env<'_> {
        own_files::Env {
            appdata: self.appdata.as_deref(),
            home: self.home.as_deref(),
            xdg_state: self.xdg_state.as_deref(),
        }
    }
}

pub(crate) fn begin() {
    let places = Places::read();
    let home = places
        .home
        .as_deref()
        .map(|home| home.to_string_lossy().into_owned());
    let opened = own_files::file(Platform::running(), &places.env(), NAMED)
        .and_then(|path| open(&path, home).ok());
    let _ = LOG.set(Mutex::new(opened));
    hook_up_panics();
    say(
        Kind::Session,
        &format!(
            "panpdf {} started on {} {}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
    );
}

fn open(path: &Path, home: Option<String>) -> std::io::Result<Open> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Ok(existing) = std::fs::read_to_string(path)
        && existing.len() > MOST_LOG
    {
        std::fs::write(path, trouble::trimmed(&existing, MOST_LOG))?;
    }
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    Ok(Open {
        file,
        path: path.to_path_buf(),
        started: Instant::now(),
        home,
    })
}

pub(crate) fn say(kind: Kind, said: &str) {
    let Some(log) = LOG.get() else { return };
    let Ok(mut held) = log.lock() else { return };
    let Some(open) = held.as_mut() else { return };
    let seconds = open.started.elapsed().as_secs();
    let line = trouble::log_line(
        seconds,
        kind,
        &trouble::redacted(said, open.home.as_deref()),
    );
    let _ = open.file.write_all(line.as_bytes());
    let _ = open.file.flush();
}

fn hook_up_panics() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        let at = panic
            .location()
            .map_or_else(String::new, |at| format!("{at}: "));
        say(Kind::Panicked, &format!("{at}{}", said_by(panic)));
        previous(panic);
    }));
}

fn said_by(panic: &std::panic::PanicHookInfo<'_>) -> String {
    let payload = panic.payload();
    payload.downcast_ref::<&str>().map_or_else(
        || {
            payload
                .downcast_ref::<String>()
                .cloned()
                .unwrap_or_else(|| "no message".to_owned())
        },
        |said| (*said).to_owned(),
    )
}

pub(crate) fn renderer_is(named: &str) {
    let _ = RENDERER.set(named.to_owned());
}

#[must_use]
pub(crate) fn where_it_is() -> Option<String> {
    let held = LOG.get()?.lock().ok()?;
    let open = held.as_ref()?;
    Some(trouble::redacted(
        &open.path.to_string_lossy(),
        open.home.as_deref(),
    ))
}

#[must_use]
pub(crate) fn facts() -> Facts {
    Facts {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        system: std::env::consts::OS.to_owned(),
        architecture: std::env::consts::ARCH.to_owned(),
        renderer: RENDERER
            .get()
            .cloned()
            .unwrap_or_else(|| "not recorded".to_owned()),
        fonts: crate::window_state::packaged_faces_found(),
        log: where_it_is(),
    }
}

#[must_use]
pub(crate) fn report_link() -> String {
    trouble::issue_link(REPOSITORY, "", &trouble::describe(&facts()))
}

#[must_use]
pub(crate) fn log_folder() -> Option<PathBuf> {
    let held = LOG.get()?.lock().ok()?;
    let open = held.as_ref()?;
    open.path.parent().map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrote(home: Option<&str>, lines: &[(Kind, &str)]) -> String {
        let folder = std::env::temp_dir().join(format!(
            "panpdf-log-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&folder);
        let path = folder.join(NAMED);
        let mut open = open(&path, home.map(str::to_owned)).expect("a log to write to");
        for (kind, said) in lines {
            let line = trouble::log_line(0, *kind, &trouble::redacted(said, open.home.as_deref()));
            open.file
                .write_all(line.as_bytes())
                .expect("a written line");
        }
        drop(open);
        let read = std::fs::read_to_string(&path).expect("the log back");
        let _ = std::fs::remove_dir_all(&folder);
        read
    }

    #[test]
    fn a_documents_path_is_kept_but_the_persons_name_is_not() {
        let name = "aroon";
        let home = format!("/home/{name}");
        let said = format!("opening {home}/Documents/tax-return.pdf");
        let written = wrote(Some(&home), &[(Kind::Document, &said)]);
        assert!(written.contains("tax-return.pdf"), "{written}");
        assert!(written.contains('~'), "{written}");
        assert!(!written.contains(name), "{written}");

        let control = wrote(None, &[(Kind::Document, &said)]);
        assert!(
            control.contains(name),
            "the control must keep the name, or this test proves nothing"
        );
    }

    #[test]
    fn a_page_cannot_get_in_through_a_newline() {
        let page = "refused: first line\nsecond line\r\nthird line";
        let written = wrote(None, &[(Kind::Refused, page)]);
        assert_eq!(written.lines().count(), 1, "{written}");
        assert!(written.ends_with('\n'), "{written}");
    }

    #[test]
    fn a_long_log_keeps_its_end_and_starts_at_a_line() {
        let folder = std::env::temp_dir().join(format!("panpdf-log-trim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).expect("a folder");
        let path = folder.join(NAMED);
        let mut long = String::new();
        for number in 0..40_000 {
            long.push_str(&trouble::log_line(
                number,
                Kind::Session,
                &format!("line {number}"),
            ));
        }
        assert!(long.len() > MOST_LOG, "the fixture must be over the cap");
        std::fs::write(&path, &long).expect("a long log");
        let open = open(&path, None).expect("the log reopened");
        drop(open);
        let kept = std::fs::read_to_string(&path).expect("the log back");
        assert!(kept.len() <= MOST_LOG, "{} bytes kept", kept.len());
        assert!(kept.contains("line 39999"), "the end is what matters");
        assert!(!kept.contains("line 0 "), "the beginning should be gone");
        let first = kept.lines().next().expect("a first line");
        assert!(
            first
                .trim_start()
                .chars()
                .next()
                .is_some_and(char::is_numeric),
            "a kept log begins at a line boundary, not mid-line: {first:?}"
        );
        let _ = std::fs::remove_dir_all(&folder);
    }
}
