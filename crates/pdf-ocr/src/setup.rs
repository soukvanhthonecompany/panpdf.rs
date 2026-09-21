use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub enum SetupError {
    Unsupported,
    NoTool(String),
    Refused(String),
    NotThere,
    Cancelled,
    Io(std::io::Error),
}

impl std::fmt::Display for SetupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => {
                formatter.write_str("this system cannot unpack packages without an administrator")
            }
            Self::NoTool(name) => write!(formatter, "{name} is not on this machine"),
            Self::Refused(said) => write!(formatter, "{said}"),
            Self::NotThere => {
                formatter.write_str("the packages unpacked but no recogniser was in them")
            }
            Self::Cancelled => formatter.write_str("stopped"),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for SetupError {}

impl From<std::io::Error> for SetupError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

const PACKAGES: [&str; 3] = ["tesseract-ocr", "libtesseract5", "liblept5"];

const TIMEOUT: Duration = Duration::from_mins(20);

#[must_use]
pub fn possible() -> bool {
    cfg!(target_os = "linux")
        && Path::new("/etc/debian_version").exists()
        && ["apt-get", "dpkg"].iter().all(|tool| on_path(tool))
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(name).is_file()))
}

pub fn install(
    into: &Path,
    step: &dyn Fn(&str),
    cancel: &AtomicBool,
) -> Result<PathBuf, SetupError> {
    if !possible() {
        return Err(SetupError::Unsupported);
    }
    let packages = into.join("packages");
    let root = into.join("root");
    let _ = std::fs::remove_dir_all(&packages);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&packages)?;
    std::fs::create_dir_all(&root)?;

    step("fetching the packages");
    let mut download = Command::new("apt-get");
    download
        .arg("download")
        .args(PACKAGES)
        .current_dir(&packages);
    run(download, "apt-get", cancel)?;

    let mut debs: Vec<PathBuf> = std::fs::read_dir(&packages)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|it| it == "deb"))
        .collect();
    debs.sort();
    if debs.is_empty() {
        return Err(SetupError::Refused("apt-get downloaded nothing".to_owned()));
    }
    for deb in &debs {
        step("unpacking");
        let mut unpack = Command::new("dpkg");
        unpack.arg("-x").arg(deb).arg(&root);
        run(unpack, "dpkg", cancel)?;
    }
    let _ = std::fs::remove_dir_all(&packages);

    let program = root.join("usr").join("bin").join("tesseract");
    if !program.is_file() {
        return Err(SetupError::NotThere);
    }
    Ok(program)
}

fn run(mut command: Command, name: &str, cancel: &AtomicBool) -> Result<(), SetupError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(SetupError::Cancelled);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => SetupError::NoTool(name.to_owned()),
            _ => SetupError::Io(error),
        })?;
    let mut stderr = child.stderr.take();
    let said = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(stderr) = stderr.as_mut() {
            let _ = std::io::Read::read_to_string(stderr, &mut text);
        }
        text
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if cancel.load(Ordering::Relaxed) || started.elapsed() > TIMEOUT {
            let stopped = cancel.load(Ordering::Relaxed);
            let _ = child.kill();
            let _ = child.wait();
            let _ = said.join();
            return Err(if stopped {
                SetupError::Cancelled
            } else {
                SetupError::Refused(format!("{name} took too long"))
            });
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let said = said.join().unwrap_or_default();
    if status.success() {
        Ok(())
    } else {
        Err(SetupError::Refused(complaint(name, &said)))
    }
}

fn complaint(name: &str, said: &str) -> String {
    let line = said
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("it gave no reason");
    let cut = match line.char_indices().nth(200) {
        Some((at, _)) => &line[..at],
        None => line,
    };
    format!("{name}: {cut}")
}

#[must_use]
pub fn library_path(root: &Path) -> Option<std::ffi::OsString> {
    let lib = root.join("usr").join("lib");
    if !lib.is_dir() {
        return None;
    }
    let mut dirs = vec![lib.clone()];
    if let Ok(entries) = std::fs::read_dir(&lib) {
        let mut inside: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        inside.sort();
        dirs.extend(inside);
    }
    std::env::join_paths(dirs).ok()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{complaint, library_path};

    #[test]
    fn the_unpacked_libraries_are_named() {
        let scratch = std::env::temp_dir().join(format!("panpdf-setup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        let root = scratch.join("root");
        let arch = root.join("usr").join("lib").join("x86_64-linux-gnu");
        std::fs::create_dir_all(&arch).expect("a scratch tree");
        std::fs::write(root.join("usr").join("lib").join("a-file"), b"x").expect("written");
        let path = library_path(&root).expect("a path");
        let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert_eq!(dirs, vec![root.join("usr").join("lib"), arch]);

        assert_eq!(library_path(&scratch.join("nothing")), None);
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn a_failure_says_who_failed_and_why() {
        assert_eq!(
            complaint(
                "apt-get",
                "\nE: Unable to locate package tesseract-ocr\nmore\n"
            ),
            "apt-get: E: Unable to locate package tesseract-ocr"
        );
        assert_eq!(complaint("dpkg", "   "), "dpkg: it gave no reason");
        assert!(complaint("dpkg", &"x".repeat(500)).len() < 220);
    }
}
