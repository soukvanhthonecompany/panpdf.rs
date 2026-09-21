use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use crate::models::Model;

#[derive(Debug)]
pub enum FetchError {
    NoHome,
    NoCurl(String),
    Refused(String),
    Size { want: u64, got: u64 },
    Digest,
    Cancelled,
    Io(std::io::Error),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoHome => formatter.write_str("there is no home directory to download into"),
            Self::NoCurl(why) => write!(formatter, "curl could not be run: {why}"),
            Self::Refused(said) => write!(formatter, "the download failed: {said}"),
            Self::Size { want, got } => {
                write!(formatter, "{got} bytes arrived of {want}")
            }
            Self::Digest => formatter.write_str("the file that arrived is not the one asked for"),
            Self::Cancelled => formatter.write_str("stopped"),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for FetchError {}

impl From<std::io::Error> for FetchError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    if let Some(named) = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Some(named.join("panpdf"));
    }
    if cfg!(windows) {
        return std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("panpdf"));
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let under = if cfg!(target_os = "macos") {
        home.join("Library").join("Application Support")
    } else {
        home.join(".local").join("share")
    };
    Some(under.join("panpdf"))
}

#[must_use]
pub fn own_tessdata() -> Option<PathBuf> {
    Some(data_dir()?.join("tessdata"))
}

#[must_use]
pub fn models_dir(quality: crate::models::Quality) -> Option<PathBuf> {
    Some(own_tessdata()?.join(quality.as_str()))
}

#[must_use]
pub fn own_engine() -> Option<PathBuf> {
    Some(data_dir()?.join("ocr"))
}

#[must_use]
pub fn languages_in(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut codes: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let code = name.strip_suffix(".traineddata")?;
            (!code.is_empty() && code != "osd").then(|| code.to_owned())
        })
        .collect();
    codes.sort_unstable();
    codes.dedup();
    codes
}

#[must_use]
pub fn holds(dir: &Path, languages: &[String]) -> bool {
    languages
        .iter()
        .all(|code| dir.join(format!("{code}.traineddata")).is_file())
}

#[must_use]
pub fn tessdata_for(
    own: Option<&Path>,
    system: Option<&Path>,
    languages: &[String],
) -> Option<PathBuf> {
    if let Some(own) = own
        && !languages.is_empty()
        && holds(own, languages)
    {
        return Some(own.to_path_buf());
    }
    system.map(Path::to_path_buf)
}

#[must_use]
pub fn have(dir: &Path, model: &Model) -> bool {
    let file = dir.join(model.file_name());
    std::fs::metadata(&file).is_ok_and(|about| about.len() == model.bytes)
}

pub fn install(dir: &Path, model: &Model, data: &[u8]) -> Result<PathBuf, FetchError> {
    let got = u64::try_from(data.len()).unwrap_or(u64::MAX);
    if got != model.bytes {
        return Err(FetchError::Size {
            want: model.bytes,
            got,
        });
    }
    if crate::sha1::blob_id(data) != model.blob {
        return Err(FetchError::Digest);
    }
    std::fs::create_dir_all(dir)?;
    let file = dir.join(model.file_name());
    let part = dir.join(format!("{}.part", model.file_name()));
    match std::fs::write(&part, data).and_then(|()| std::fs::rename(&part, &file)) {
        Ok(()) => Ok(file),
        Err(error) => {
            let _ = std::fs::remove_file(&part);
            Err(FetchError::Io(error))
        }
    }
}

static COUNT: AtomicU64 = AtomicU64::new(0);

const FETCH_TIMEOUT: u64 = 1800;

pub fn fetch(
    model: &Model,
    dir: &Path,
    seen: &dyn Fn(u64),
    cancel: &AtomicBool,
) -> Result<PathBuf, FetchError> {
    if have(dir, model) {
        return Ok(dir.join(model.file_name()));
    }
    std::fs::create_dir_all(dir)?;
    let part = dir.join(format!(
        "{}.part-{}-{}",
        model.file_name(),
        std::process::id(),
        COUNT.fetch_add(1, Ordering::Relaxed)
    ));
    let outcome = fetch_into(model, &part, seen, cancel);
    let data = match outcome {
        Ok(()) => std::fs::read(&part),
        Err(error) => {
            let _ = std::fs::remove_file(&part);
            return Err(error);
        }
    };
    let _ = std::fs::remove_file(&part);
    install(dir, model, &data?)
}

fn fetch_into(
    model: &Model,
    part: &Path,
    seen: &dyn Fn(u64),
    cancel: &AtomicBool,
) -> Result<(), FetchError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(FetchError::Cancelled);
    }
    let config = config_for(&model.url(), part);
    let mut child = Command::new("curl")
        .arg("-q")
        .arg("--config")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| FetchError::NoCurl(error.to_string()))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(config.as_bytes());
    }
    let mut stderr = child.stderr.take();
    let said = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(stderr) = stderr.as_mut() {
            let _ = std::io::Read::read_to_string(stderr, &mut text);
        }
        text
    });
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = said.join();
            return Err(FetchError::Cancelled);
        }
        seen(std::fs::metadata(part).map_or(0, |about| about.len()));
        std::thread::sleep(Duration::from_millis(100));
    };
    let said = said.join().unwrap_or_default();
    if status.success() {
        seen(std::fs::metadata(part).map_or(0, |about| about.len()));
        Ok(())
    } else {
        Err(FetchError::Refused(short(&said)))
    }
}

fn config_for(url: &str, part: &Path) -> String {
    format!(
        "url = \"{}\"\noutput = \"{}\"\nsilent\nshow-error\nfail\ngloboff\n\
         location\nmax-redirs = 5\nproto = \"https\"\nproto-redir = \"https\"\n\
         connect-timeout = 20\nmax-time = {FETCH_TIMEOUT}\n",
        curl_escape(url),
        curl_escape(&part.to_string_lossy())
    )
}

fn curl_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for letter in value.chars() {
        match letter {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\r' | '\n' | '\t' => out.push(' '),
            other => out.push(other),
        }
    }
    out
}

fn short(said: &str) -> String {
    let line = said
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let trimmed = line.trim();
    match trimmed.char_indices().nth(200) {
        Some((at, _)) => trimmed[..at].to_owned(),
        None => trimmed.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{curl_escape, holds, install, languages_in, tessdata_for};
    use crate::models::{Model, Quality};
    use crate::sha1;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("panpdf-store-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("a scratch directory");
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn pretend(data: &[u8]) -> Model {
        let blob: &'static str = Box::leak(sha1::blob_id(data).into_boxed_str());
        Model {
            code: "lao",
            quality: Quality::Fast,
            bytes: u64::try_from(data.len()).expect("a small test file"),
            blob,
            cer: 11.6,
        }
    }

    #[test]
    fn a_short_download_is_rejected_and_removed() {
        let scratch = Scratch::new("install");
        let dir = scratch.path();
        let whole = b"a model, as far as this test is concerned".to_vec();
        let model = pretend(&whole);

        let short = &whole[..whole.len() - 1];
        let refused = install(dir, &model, short);
        assert!(
            matches!(refused, Err(super::FetchError::Size { want, got })
                if want == model.bytes && got == model.bytes - 1),
            "{refused:?}"
        );
        assert_eq!(std::fs::read_dir(dir).expect("readable").count(), 0);

        let mut wrong = whole.clone();
        wrong[0] = b'b';
        assert!(matches!(
            install(dir, &model, &wrong),
            Err(super::FetchError::Digest)
        ));
        assert_eq!(std::fs::read_dir(dir).expect("readable").count(), 0);

        let kept = install(dir, &model, &whole).expect("the right file is kept");
        assert_eq!(kept, dir.join("lao.traineddata"));
        assert_eq!(std::fs::read(&kept).expect("readable"), whole);
        assert!(super::have(dir, &model));
        assert_eq!(std::fs::read_dir(dir).expect("readable").count(), 1);
    }

    #[test]
    fn the_programs_own_models_come_first() {
        let scratch = Scratch::new("locate");
        let own = scratch.path().join("own");
        let system = scratch.path().join("system");
        std::fs::create_dir_all(&own).expect("own");
        std::fs::create_dir_all(&system).expect("system");
        for code in ["lao", "tha", "eng", "osd"] {
            std::fs::write(system.join(format!("{code}.traineddata")), b"system").expect("written");
        }
        let want: Vec<String> = ["lao", "tha"].iter().map(|it| (*it).to_owned()).collect();

        assert_eq!(
            tessdata_for(Some(&own), Some(&system), &want),
            Some(system.clone()),
            "an empty directory of our own is not used"
        );
        std::fs::write(own.join("lao.traineddata"), b"ours").expect("written");
        assert_eq!(
            tessdata_for(Some(&own), Some(&system), &want),
            Some(system.clone()),
            "half the languages is not enough"
        );
        std::fs::write(own.join("tha.traineddata"), b"ours").expect("written");
        assert_eq!(
            tessdata_for(Some(&own), Some(&system), &want),
            Some(own.clone()),
            "ours holds both, so ours is read"
        );
        let empty = scratch.path().join("empty");
        std::fs::create_dir_all(&empty).expect("empty");
        assert_eq!(tessdata_for(Some(&empty), None, &want), None);
        assert_eq!(
            tessdata_for(None, Some(&system), &want),
            Some(system.clone())
        );
        assert!(!holds(&empty, &want));

        assert_eq!(languages_in(&own), vec!["lao".to_owned(), "tha".to_owned()]);
        assert_eq!(
            languages_in(scratch.path().join("nowhere").as_path()),
            Vec::<String>::new()
        );
        assert_eq!(
            languages_in(&system),
            vec!["eng".to_owned(), "lao".to_owned(), "tha".to_owned()]
        );
    }

    #[test]
    fn a_url_cannot_become_another_option() {
        assert_eq!(curl_escape("https://x/a\"b"), "https://x/a\\\"b");
        assert_eq!(curl_escape("a\nheader = \"x\""), "a header = \\\"x\\\"");
        assert_eq!(curl_escape("C:\\panpdf"), "C:\\\\panpdf");
        let config = super::config_for("https://x/lao.traineddata", Path::new("/tmp/a b.part"));
        assert!(config.contains("url = \"https://x/lao.traineddata\""));
        assert!(config.contains("output = \"/tmp/a b.part\""));
        assert!(config.contains("proto = \"https\""));
        assert_eq!(
            config
                .lines()
                .filter(|line| line.starts_with("url"))
                .count(),
            1
        );
    }
}
