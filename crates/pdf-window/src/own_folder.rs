use std::path::PathBuf;

use pdf_app::own_files::{self, Platform};

pub(crate) struct Places {
    pub(crate) appdata: Option<PathBuf>,
    pub(crate) home: Option<PathBuf>,
    pub(crate) xdg_state: Option<PathBuf>,
}

impl Places {
    pub(crate) fn read() -> Self {
        let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
        Self {
            appdata: var("APPDATA"),
            home: var("HOME").or_else(|| var("USERPROFILE")),
            xdg_state: var("XDG_STATE_HOME"),
        }
    }

    pub(crate) fn env(&self) -> own_files::Env<'_> {
        own_files::Env {
            appdata: self.appdata.as_deref(),
            home: self.home.as_deref(),
            xdg_state: self.xdg_state.as_deref(),
        }
    }
}

#[cfg_attr(
    target_arch = "wasm32",
    expect(
        dead_code,
        reason = "the kept key, its one caller, is not built for the browser"
    )
)]
pub(crate) fn folder() -> Option<PathBuf> {
    own_files::folder(Platform::running(), &Places::read().env())
}

pub(crate) fn own_file(name: &str) -> Option<PathBuf> {
    own_files::file(Platform::running(), &Places::read().env(), name)
}
