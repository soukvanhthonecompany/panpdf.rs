use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::tsv::{Line, ReadingError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grey {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

#[derive(Debug)]
pub enum OcrError {
    NotInstalled,
    LanguageMissing(String),
    BadLanguage(String),
    BadImage,
    Render(String),
    Cancelled,
    TimedOut,
    Failed(String),
    Unreadable(ReadingError),
    Io(std::io::Error),
}

impl std::fmt::Display for OcrError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInstalled => {
                formatter.write_str("the text recogniser (Tesseract) is not installed")
            }
            Self::LanguageMissing(code) => {
                write!(formatter, "the recogniser has no language file for {code}")
            }
            Self::BadLanguage(code) => write!(formatter, "{code:?} is not a language code"),
            Self::BadImage => formatter.write_str("the page image is not the size it says"),
            Self::Render(why) => write!(formatter, "the page could not be drawn to be read: {why}"),
            Self::Cancelled => formatter.write_str("stopped"),
            Self::TimedOut => formatter.write_str("the recogniser took too long over one page"),
            Self::Failed(said) => write!(formatter, "the recogniser failed: {said}"),
            Self::Unreadable(error) => write!(formatter, "{error}"),
            Self::Io(error) => write!(formatter, "the recogniser could not be run: {error}"),
        }
    }
}

impl std::error::Error for OcrError {}

impl From<std::io::Error> for OcrError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

pub const PAGE_TIMEOUT: Duration = Duration::from_mins(10);

const PROGRAM: &str = if cfg!(windows) {
    "tesseract.exe"
} else {
    "tesseract"
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tesseract {
    pub program: PathBuf,
    pub languages: Option<PathBuf>,
    pub own: Option<PathBuf>,
    pub library_path: Option<std::ffi::OsString>,
}

impl Tesseract {
    pub fn locate() -> Result<Self, OcrError> {
        let beside = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("ocr")));
        let named = std::env::var_os("PANPDF_TESSERACT").map(PathBuf::from);
        let bundled = beside.as_ref().map(|dir| dir.join(PROGRAM));
        let unpacked = crate::store::own_engine();
        let ours = unpacked
            .as_ref()
            .map(|dir| dir.join("root").join("usr").join("bin").join(PROGRAM));
        let on_path = std::env::var_os("PATH").and_then(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join(PROGRAM))
                .find(|candidate| candidate.is_file())
        });
        let program = named
            .into_iter()
            .chain(bundled)
            .chain(ours.clone())
            .chain(on_path)
            .find(|candidate| candidate.is_file())
            .ok_or(OcrError::NotInstalled)?;
        let languages = std::env::var_os("PANPDF_TESSDATA")
            .map(PathBuf::from)
            .or_else(|| {
                beside
                    .map(|dir| dir.join("tessdata"))
                    .filter(|dir| dir.is_dir())
            });
        let library_path = match (&unpacked, ours.as_ref() == Some(&program)) {
            (Some(dir), true) => crate::setup::library_path(&dir.join("root")),
            _ => None,
        };
        Ok(Self {
            program,
            languages,
            own: crate::store::models_dir(crate::models::Quality::Accurate),
            library_path,
        })
    }

    fn command(&self) -> Command {
        self.command_for(&[])
    }

    fn command_for(&self, languages: &[String]) -> Command {
        let mut command = Command::new(&self.program);
        let dir =
            crate::store::tessdata_for(self.own.as_deref(), self.languages.as_deref(), languages);
        if let Some(dir) = dir {
            command.arg("--tessdata-dir").arg(dir);
        }
        if let Some(path) = &self.library_path {
            command.env("LD_LIBRARY_PATH", path);
        }
        command
    }

    #[must_use]
    pub fn every_language(&self) -> Vec<String> {
        let mut codes: Vec<String> = self
            .own
            .as_deref()
            .map(crate::store::languages_in)
            .unwrap_or_default();
        for code in self.installed_languages().unwrap_or_default() {
            if !codes.contains(&code) {
                codes.push(code);
            }
        }
        codes.sort_unstable();
        codes.dedup();
        codes
    }

    pub fn installed_languages(&self) -> Result<Vec<String>, OcrError> {
        let output = self
            .command()
            .arg("--list-langs")
            .stdin(Stdio::null())
            .output()?;
        if !output.status.success() {
            return Err(OcrError::Failed(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .skip(1)
            .map(str::trim)
            .filter(|code| !code.is_empty() && *code != "osd" && is_code(code))
            .map(str::to_owned)
            .collect())
    }

    pub fn recognize(
        &self,
        image: &Grey,
        languages: &[String],
        dpi: u32,
        cancel: &AtomicBool,
    ) -> Result<Vec<Line>, OcrError> {
        let expected = usize::try_from(u64::from(image.width) * u64::from(image.height))
            .map_err(|_| OcrError::BadImage)?;
        if expected == 0 || image.pixels.len() != expected {
            return Err(OcrError::BadImage);
        }
        if let Some(code) = languages.iter().find(|code| !is_code(code)) {
            return Err(OcrError::BadLanguage(code.clone()));
        }
        let joined = if languages.is_empty() {
            "eng".to_owned()
        } else {
            languages.join("+")
        };
        let dir = PrivateDir::new()?;
        let base = dir.path.join("page");
        let mut child = self
            .command_for(languages)
            .arg("stdin")
            .arg(&base)
            .args(["-l", &joined, "--dpi", &dpi.to_string(), "--psm", "3"])
            .args(["-c", "tessedit_create_tsv=1", "-c", "tessedit_create_txt=1"])
            .env("OMP_THREAD_LIMIT", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stderr = child.stderr.take();
        let said = std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(stderr) = stderr.as_mut() {
                let _ = stderr.read_to_string(&mut text);
            }
            text
        });
        if let Some(mut stdin) = child.stdin.take() {
            let written = stdin
                .write_all(format!("P5\n{} {}\n255\n", image.width, image.height).as_bytes())
                .and_then(|()| stdin.write_all(&image.pixels));
            drop(stdin);
            if let Err(error) = written {
                let _ = child.kill();
                let _ = child.wait();
                let said = said.join().unwrap_or_default();
                return Err(failure(&said, &joined).unwrap_or(OcrError::Io(error)));
            }
        }
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(OcrError::Cancelled);
            }
            if started.elapsed() > PAGE_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                return Err(OcrError::TimedOut);
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let said = said.join().unwrap_or_default();
        if !status.success() {
            return Err(
                failure(&said, &joined).unwrap_or_else(|| OcrError::Failed(said.trim().to_owned()))
            );
        }
        let tsv = std::fs::read_to_string(base.with_extension("tsv"))?;
        let text = std::fs::read_to_string(base.with_extension("txt"))?;
        crate::tsv::read(&tsv, &text).map_err(OcrError::Unreadable)
    }
}

fn failure(said: &str, joined: &str) -> Option<OcrError> {
    said.contains("Failed loading language").then(|| {
        let missing = said
            .lines()
            .find_map(|line| {
                let start =
                    line.find("Failed loading language '")? + "Failed loading language '".len();
                let rest = &line[start..];
                Some(rest[..rest.find('\'')?].to_owned())
            })
            .unwrap_or_else(|| joined.to_owned());
        OcrError::LanguageMissing(missing)
    })
}

fn is_code(code: &str) -> bool {
    !code.is_empty()
        && code
            .chars()
            .all(|letter| letter.is_ascii_alphanumeric() || letter == '_')
}

struct PrivateDir {
    path: PathBuf,
}

impl PrivateDir {
    fn new() -> std::io::Result<Self> {
        static COUNT: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "panpdf-ocr-{}-{}-{nanos}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::Relaxed)
        ));
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(&path)?;
        Ok(Self { path })
    }
}

impl Drop for PrivateDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

impl AsRef<Path> for PrivateDir {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::{Grey, OcrError, Tesseract, failure, is_code};

    #[test]
    fn a_language_code_cannot_be_an_argument() {
        assert!(is_code("lao"));
        assert!(is_code("chi_sim"));
        assert!(!is_code(""));
        assert!(!is_code("-c"));
        assert!(!is_code("../eng"));
        assert!(!is_code("eng+tha"));
    }

    #[test]
    fn a_missing_language_is_named() {
        let said = "Error opening data file /x/khm.traineddata\n\
                    Failed loading language 'khm'\nTesseract couldn't load any languages!";
        assert!(
            matches!(failure(said, "khm+eng"), Some(OcrError::LanguageMissing(code)) if code == "khm")
        );
        assert!(failure("Segmentation fault", "eng").is_none());
    }

    #[test]
    fn what_is_asked_is_checked_before_anything_runs() {
        let engine = Tesseract {
            program: "/nonexistent/tesseract".into(),
            languages: None,
            own: None,
            library_path: None,
        };
        let never = AtomicBool::new(false);
        let short = Grey {
            width: 4,
            height: 4,
            pixels: vec![255; 15],
        };
        assert!(matches!(
            engine.recognize(&short, &["eng".to_owned()], 300, &never),
            Err(OcrError::BadImage)
        ));
        let right = Grey {
            pixels: vec![255; 16],
            ..short
        };
        assert!(matches!(
            engine.recognize(&right, &["-c".to_owned()], 300, &never),
            Err(OcrError::BadLanguage(_))
        ));
        assert!(matches!(
            engine.recognize(&right, &["eng".to_owned()], 300, &never),
            Err(OcrError::Io(_))
        ));
    }
}
