use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Windows,
    MacOs,
    Xdg,
}

impl Platform {
    #[must_use]
    pub const fn running() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Xdg
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Env<'a> {
    pub appdata: Option<&'a Path>,
    pub home: Option<&'a Path>,
    pub xdg_state: Option<&'a Path>,
}

const OURS: &str = "panpdf";

#[must_use]
pub fn settled(path: &Path) -> bool {
    let text = path.to_string_lossy();
    let bytes = text.as_bytes();
    match bytes {
        [b'/', ..] | [b'\\', b'\\', ..] => true,
        [drive, b':', b'/' | b'\\', ..] => drive.is_ascii_alphabetic(),
        _ => false,
    }
}

fn absolute(path: Option<&Path>) -> Option<&Path> {
    path.filter(|path| settled(path))
}

#[must_use]
pub fn folder(platform: Platform, env: &Env) -> Option<PathBuf> {
    let base = match platform {
        Platform::Windows => absolute(env.appdata)?.to_path_buf(),
        Platform::MacOs => absolute(env.home)?
            .join("Library")
            .join("Application Support"),
        Platform::Xdg => match absolute(env.xdg_state) {
            Some(state) => state.to_path_buf(),
            None => absolute(env.home)?.join(".local").join("state"),
        },
    };
    Some(base.join(OURS))
}

#[must_use]
pub fn file(platform: Platform, env: &Env, name: &str) -> Option<PathBuf> {
    Some(folder(platform, env)?.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(appdata: Option<&'a str>, home: Option<&'a str>, xdg: Option<&'a str>) -> Env<'a> {
        Env {
            appdata: appdata.map(Path::new),
            home: home.map(Path::new),
            xdg_state: xdg.map(Path::new),
        }
    }

    #[test]
    fn each_platform_keeps_its_files_where_that_platform_keeps_files() {
        let full = env(
            Some(r"C:\Users\tester\AppData\Roaming"),
            Some("/home/someone"),
            Some("/home/someone/.state"),
        );
        assert_eq!(
            folder(Platform::Windows, &full),
            Some(PathBuf::from(r"C:\Users\tester\AppData\Roaming").join("panpdf"))
        );
        assert_eq!(
            folder(Platform::MacOs, &full),
            Some(PathBuf::from(
                "/home/someone/Library/Application Support/panpdf"
            ))
        );
        assert_eq!(
            folder(Platform::Xdg, &full),
            Some(PathBuf::from("/home/someone/.state/panpdf"))
        );
    }

    #[test]
    fn without_a_state_variable_linux_falls_back_under_home() {
        let e = env(None, Some("/home/someone"), None);
        assert_eq!(
            folder(Platform::Xdg, &e),
            Some(PathBuf::from("/home/someone/.local/state/panpdf"))
        );
    }

    #[test]
    fn the_linux_shaped_rule_is_what_left_windows_with_nowhere_to_write() {
        fn the_way_it_used_to_be_asked(env: &Env) -> Option<PathBuf> {
            let state = absolute(env.xdg_state)
                .map(Path::to_path_buf)
                .or_else(|| absolute(env.home).map(|home| home.join(".local").join("state")))?;
            Some(state.join("panpdf"))
        }
        let windows = env(Some(r"C:\Users\tester\AppData\Roaming"), None, None);
        assert_eq!(the_way_it_used_to_be_asked(&windows), None);
        assert!(folder(Platform::Windows, &windows).is_some());
    }

    #[test]
    fn a_path_that_says_where_it_is() {
        for yes in [
            "/home/someone",
            "/",
            r"C:\\Users\\tester",
            "d:/data",
            r"\\\\server\\share",
        ] {
            assert!(settled(Path::new(yes)), "{yes} says where it is");
        }
        for no in ["AppData", ".local/state", "~/state", "", r"C:", "1:/data"] {
            assert!(!settled(Path::new(no)), "{no} does not say where it is");
        }
    }

    #[test]
    fn a_relative_or_missing_variable_is_no_answer() {
        assert_eq!(
            folder(Platform::Windows, &env(Some("AppData"), None, None)),
            None
        );
        assert_eq!(folder(Platform::Windows, &env(None, None, None)), None);
        assert_eq!(
            folder(Platform::MacOs, &env(None, Some("someone"), None)),
            None
        );
        assert_eq!(
            folder(Platform::Xdg, &env(None, None, Some(".state"))),
            None
        );
        assert_eq!(
            folder(
                Platform::Xdg,
                &env(None, Some("/home/someone"), Some(".state"))
            ),
            Some(PathBuf::from("/home/someone/.local/state/panpdf")),
            "a relative XDG_STATE_HOME falls through to HOME rather than being used"
        );
    }

    #[test]
    fn a_named_file_sits_in_that_folder() {
        let e = env(None, Some("/home/someone"), None);
        assert_eq!(
            file(Platform::Xdg, &e, "recent"),
            Some(PathBuf::from("/home/someone/.local/state/panpdf/recent"))
        );
    }
}
