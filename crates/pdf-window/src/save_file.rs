use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub(crate) fn unused_copy(original: &Path) -> PathBuf {
    let first = pdf_app::files::copy_beside(original);
    let mut candidate = first.clone();
    let stem = first.file_stem().unwrap_or_default();
    for number in 2.. {
        if fs::symlink_metadata(&candidate).is_err_and(|e| e.kind() == io::ErrorKind::NotFound) {
            return candidate;
        }
        if fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
        let mut name = stem.to_os_string();
        name.push(format!("-{number}.pdf"));
        candidate.set_file_name(name);
    }
    unreachable!()
}

pub fn save(
    original: &Path,
    destination: &Path,
    bytes: &[u8],
    previous_digest: Option<&str>,
) -> io::Result<String> {
    save_with(original, destination, bytes, previous_digest, |file| {
        file.write_all(bytes)
    })
}

fn save_with(
    original: &Path,
    destination: &Path,
    bytes: &[u8],
    previous_digest: Option<&str>,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<String> {
    if original == destination
        || fs::canonicalize(original)
            .ok()
            .zip(fs::canonicalize(destination).ok())
            .is_some_and(|(a, b)| a == b)
    {
        return Err(io::Error::other(
            "the original document cannot be overwritten",
        ));
    }
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let (temporary, mut file) = loop {
        let number = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(".panpdf-{}-{number}.tmp", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => break (Temporary(path), file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    };
    write(&mut file)?;
    file.sync_all()?;
    drop(file);
    match previous_digest {
        None => fs::hard_link(&temporary.0, destination)?,
        Some(expected) => {
            let metadata = fs::symlink_metadata(destination)?;
            if !metadata.file_type().is_file()
                || pdf_content::sha256_hex(&fs::read(destination)?) != expected
            {
                return Err(io::Error::other(
                    "the saved copy was changed outside this session",
                ));
            }
            fs::rename(&temporary.0, destination)?;
        }
    }
    Ok(pdf_content::sha256_hex(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory() -> Temporary {
        let path = std::env::temp_dir().join(format!(
            "panpdf-save-test-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Temporary(path)
    }

    #[test]
    fn a_failed_write_and_a_collision_preserve_existing_bytes() {
        let dir = directory();
        let original = dir.0.join("original.pdf");
        let destination = dir.0.join("original-edited.pdf");
        fs::write(&original, b"original").unwrap();
        let digest = save(&original, &destination, b"first", None).unwrap();
        assert!(save(&original, &destination, b"collision", None).is_err());
        assert!(
            save_with(&original, &destination, b"second", Some(&digest), |file| {
                file.write_all(b"partial")?;
                Err(io::Error::other("injected disk failure"))
            })
            .is_err()
        );
        assert_eq!(fs::read(&destination).unwrap(), b"first");
        let digest = save(&original, &destination, b"second", Some(&digest)).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"second");
        fs::write(&destination, b"someone else's work").unwrap();
        assert!(save(&original, &destination, b"third", Some(&digest)).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"someone else's work");
        assert_eq!(fs::read(&original).unwrap(), b"original");
        assert_eq!(
            fs::read_dir(&dir.0).unwrap().count(),
            2,
            "temporary files removed"
        );
        fs::remove_dir_all(&dir.0).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_cannot_turn_save_into_an_original_overwrite() {
        let dir = directory();
        let original = dir.0.join("original.pdf");
        let destination = dir.0.join("original-edited.pdf");
        fs::write(&original, b"original").unwrap();
        std::os::unix::fs::symlink(&original, &destination).unwrap();
        assert!(save(&original, &destination, b"edit", None).is_err());
        assert_ne!(unused_copy(&original), destination);
        assert_eq!(fs::read(&original).unwrap(), b"original");
        fs::remove_dir_all(&dir.0).unwrap();
    }
}
