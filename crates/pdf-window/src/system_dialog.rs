use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Kind {
    Pdfs,
    Pictures,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Question {
    pub(crate) title: String,
    pub(crate) folder: PathBuf,
    pub(crate) kind: Kind,
    pub(crate) naming: Option<String>,
    pub(crate) several: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Answer {
    Chose(Vec<PathBuf>),
    Cancelled,
    Unavailable,
}

pub(crate) struct Asking {
    answer: Receiver<Answer>,
    stop: Arc<AtomicBool>,
}

impl Asking {
    pub(crate) fn answer(&self) -> Option<Answer> {
        match self.answer.try_recv() {
            Ok(answer) => Some(answer),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Answer::Unavailable),
        }
    }
}

impl Drop for Asking {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub(crate) fn ask(question: &Question) -> Option<Asking> {
    if std::env::var_os("PANPDF_OWN_CHOOSER").is_some() {
        return None;
    }
    let programs = programs(question);
    if programs.is_empty() {
        return None;
    }
    let (send, answer) = channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = Arc::clone(&stop);
    std::thread::Builder::new()
        .name("file-window".to_owned())
        .spawn(move || {
            let mut said = Answer::Unavailable;
            for (program, arguments) in programs {
                said = run(&program, &arguments, &stopping);
                if said != Answer::Unavailable {
                    break;
                }
            }
            let _ = send.send(said);
        })
        .ok()?;
    Some(Asking { answer, stop })
}

#[cfg(all(unix, not(target_os = "macos")))]
fn programs(question: &Question) -> Vec<(String, Vec<String>)> {
    let on_kde = std::env::var("XDG_CURRENT_DESKTOP")
        .is_ok_and(|desktop| desktop.to_ascii_uppercase().contains("KDE"));
    let both = [
        ("zenity".to_owned(), zenity(question)),
        ("kdialog".to_owned(), kdialog(question)),
    ];
    let mut order: Vec<(String, Vec<String>)> = both.into_iter().collect();
    if on_kde {
        order.reverse();
    }
    order
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn programs(_question: &Question) -> Vec<(String, Vec<String>)> {
    Vec::new()
}

#[cfg(not(target_arch = "wasm32"))]
fn patterns(kind: Kind) -> &'static str {
    match kind {
        Kind::Pdfs => "*.pdf *.PDF",
        Kind::Pictures => "*.png *.jpg *.jpeg *.PNG *.JPG *.JPEG",
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn start(question: &Question) -> String {
    match &question.naming {
        Some(name) => question.folder.join(name).display().to_string(),
        None => format!("{}/", question.folder.display()),
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn zenity(question: &Question) -> Vec<String> {
    let mut arguments = vec![
        "--file-selection".to_owned(),
        format!("--title={}", question.title),
        format!("--filename={}", start(question)),
    ];
    let name = match question.kind {
        Kind::Pdfs => "PDF",
        Kind::Pictures => "PNG, JPEG",
    };
    arguments.push(format!(
        "--file-filter={name} | {}",
        patterns(question.kind)
    ));
    if question.naming.is_some() {
        arguments.push("--save".to_owned());
    }
    if question.several {
        arguments.push("--multiple".to_owned());
        arguments.push("--separator=\n".to_owned());
    }
    arguments
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn kdialog(question: &Question) -> Vec<String> {
    let filter = format!("{}|{}", patterns(question.kind), question.title);
    let mut arguments = vec![format!("--title={}", question.title)];
    if question.naming.is_some() {
        arguments.push("--getsavefilename".to_owned());
    } else {
        arguments.push("--getopenfilename".to_owned());
    }
    arguments.push(start(question));
    arguments.push(filter);
    if question.several {
        arguments.push("--multiple".to_owned());
        arguments.push("--separate-output".to_owned());
    }
    arguments
}

fn run(program: &str, arguments: &[String], stop: &AtomicBool) -> Answer {
    let child = std::process::Command::new(program)
        .args(arguments)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return Answer::Unavailable;
    };
    let reading = child.stdout.take().map(|mut out| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = out.read_to_end(&mut bytes);
            bytes
        })
    });
    let status = loop {
        if stop.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Answer::Cancelled;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(40)),
            Err(_) => return Answer::Unavailable,
        }
    };
    let bytes = reading
        .and_then(|reading| reading.join().ok())
        .unwrap_or_default();
    match status.code() {
        Some(0) => {
            let paths = read_paths(&bytes);
            if paths.is_empty() {
                Answer::Cancelled
            } else {
                Answer::Chose(paths)
            }
        }
        Some(1) => Answer::Cancelled,
        _ => Answer::Unavailable,
    }
}

pub(crate) fn read_paths(bytes: &[u8]) -> Vec<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| PathBuf::from(std::ffi::OsStr::from_bytes(line)))
            .collect()
    }
    #[cfg(not(unix))]
    {
        String::from_utf8_lossy(bytes)
            .lines()
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .collect()
    }
}

pub(crate) fn folder_and_name(path: &Path) -> Option<(PathBuf, String)> {
    let folder = path.parent()?.to_path_buf();
    let name = path.file_name()?.to_string_lossy().into_owned();
    Some((folder, name))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Kind, Question, kdialog, read_paths, zenity};

    fn question(naming: Option<&str>, several: bool) -> Question {
        Question {
            title: "Open a file".to_owned(),
            folder: PathBuf::from("/home/someone/Documents"),
            kind: Kind::Pdfs,
            naming: naming.map(str::to_owned),
            several,
        }
    }

    #[test]
    fn zenity_is_told_where_to_start_and_what_to_show() {
        let open = zenity(&question(None, false));
        assert!(open.contains(&"--filename=/home/someone/Documents/".to_owned()));
        assert!(open.contains(&"--file-filter=PDF | *.pdf *.PDF".to_owned()));
        assert!(!open.contains(&"--save".to_owned()));
        let save = zenity(&question(Some("report-copy.pdf"), false));
        assert!(save.contains(&"--filename=/home/someone/Documents/report-copy.pdf".to_owned()));
        assert!(save.contains(&"--save".to_owned()));
        assert!(!save.iter().any(|argument| argument.contains("overwrite")));
        let several = zenity(&question(None, true));
        assert!(several.contains(&"--multiple".to_owned()));
    }

    #[test]
    fn kdialog_is_told_the_same() {
        let open = kdialog(&question(None, false));
        assert!(open.contains(&"--getopenfilename".to_owned()));
        assert!(open.contains(&"/home/someone/Documents/".to_owned()));
        let save = kdialog(&question(Some("a.pdf"), false));
        assert!(save.contains(&"--getsavefilename".to_owned()));
        assert!(kdialog(&question(None, true)).contains(&"--separate-output".to_owned()));
    }

    #[test]
    fn the_paths_printed_are_read_one_a_line() {
        assert_eq!(
            read_paths(b"/a/one file.png\n/b/two.jpg\n"),
            vec![
                PathBuf::from("/a/one file.png"),
                PathBuf::from("/b/two.jpg")
            ]
        );
        assert!(read_paths(b"").is_empty());
    }
}
