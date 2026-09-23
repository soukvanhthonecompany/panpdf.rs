use std::io::Read;
use std::path::PathBuf;

use pdf_app::ai_key;

use super::AiState;

fn state() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    crate::own_folder::folder()
}

fn key_file() -> Option<PathBuf> {
    Some(state()?.join("ai-key"))
}

fn random(count: usize) -> Option<Vec<u8>> {
    let mut bytes = vec![0_u8; count];
    let mut source = std::fs::File::open("/dev/urandom").ok()?;
    source.read_exact(&mut bytes).ok()?;
    Some(bytes)
}

#[cfg(not(unix))]
fn write_privately(_path: &std::path::Path, _bytes: &[u8]) -> bool {
    false
}

#[cfg(unix)]
fn write_privately(path: &std::path::Path, bytes: &[u8]) -> bool {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let Some(folder) = path.parent() else {
        return false;
    };
    if std::fs::create_dir_all(folder).is_err() {
        return false;
    }
    let beside = folder.join(format!(
        "{}.writing",
        path.file_name()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("x")
    ));
    let made = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&beside);
    let Ok(mut file) = made else {
        return false;
    };
    if file.write_all(bytes).is_err() || file.sync_all().is_err() {
        drop(file);
        let _ = std::fs::remove_file(&beside);
        return false;
    }
    drop(file);
    if std::fs::rename(&beside, path).is_err() {
        let _ = std::fs::remove_file(&beside);
        return false;
    }
    true
}

fn machine(make: bool) -> Option<Vec<u8>> {
    let file = state()?.join("installation");
    let secret = match std::fs::read(&file) {
        Ok(bytes) if bytes.len() >= 32 => bytes,
        _ => {
            if !make {
                return None;
            }
            let fresh = random(32)?;
            if !write_privately(&file, &fresh) {
                return None;
            }
            fresh
        }
    };
    let mut material = secret;
    if let Some(home) = std::env::var_os("HOME") {
        material.extend_from_slice(home.as_encoded_bytes());
    }
    Some(material)
}

impl AiState {
    pub(super) fn keep_the_key(&mut self) {
        let Some(file) = key_file() else {
            return;
        };
        if !self.remember_key || self.key.is_empty() {
            let _ = std::fs::remove_file(&file);
            return;
        }
        let (Some(machine), Some(salt)) = (machine(true), random(ai_key::SALT)) else {
            self.remember_key = false;
            return;
        };
        let Some(line) = ai_key::lock(&self.key, &machine, &salt) else {
            self.remember_key = false;
            return;
        };
        if !write_privately(&file, line.as_bytes()) {
            self.remember_key = false;
        }
    }

    pub(super) fn take_the_kept_key(&mut self) {
        let Some(text) = key_file().and_then(|file| std::fs::read_to_string(file).ok()) else {
            return;
        };
        let Some(key) = machine(false).and_then(|machine| ai_key::unlock(&text, &machine)) else {
            if ai_key::looks_like_ours(&text) {
                self.notice = Some(super::Notice::plain(
                    pdf_app::wording::Message::AiKeptKeyUnreadable,
                ));
            }
            return;
        };
        self.key = key;
        self.remember_key = true;
        self.connected = false;
    }
}
