use std::process::{Command, Stdio};

use pdf_app::wording::{Lang, Message};

const TITLE: &str = "PanPDF";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Desk {
    Windows,
    Mac,
    Unix,
    Elsewhere,
}

impl Desk {
    pub(crate) const fn here() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Mac
        } else if cfg!(unix) {
            Self::Unix
        } else {
            Self::Elsewhere
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Teller {
    PowerShell,
    OsaScript,
    Zenity,
    KDialog,
}

pub(crate) fn order(desk: Desk, kde: bool) -> Vec<Teller> {
    match desk {
        Desk::Windows => vec![Teller::PowerShell],
        Desk::Mac => vec![Teller::OsaScript],
        Desk::Unix if kde => vec![Teller::KDialog, Teller::Zenity],
        Desk::Unix => vec![Teller::Zenity, Teller::KDialog],
        Desk::Elsewhere => Vec::new(),
    }
}

fn on_kde() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .is_ok_and(|desktop| desktop.to_ascii_uppercase().contains("KDE"))
}

pub(crate) const fn powershell() -> &'static str {
    "Add-Type -AssemblyName System.Windows.Forms | Out-Null; \
     [System.Windows.Forms.MessageBox]::Show($env:PANPDF_SAID, $env:PANPDF_TITLE, 'OK', 'Error') \
     | Out-Null"
}

pub(crate) const SAID: &str = "PANPDF_SAID";

pub(crate) fn zenity(said: &str) -> Vec<String> {
    vec![
        "--error".to_owned(),
        format!("--title={TITLE}"),
        format!("--text={said}"),
    ]
}

pub(crate) fn kdialog(said: &str) -> Vec<String> {
    vec![
        format!("--title={TITLE}"),
        "--error".to_owned(),
        said.to_owned(),
    ]
}

pub(crate) fn applescript(said: &str) -> String {
    let lines: Vec<String> = said
        .split('\n')
        .map(|line| format!("\"{}\"", line.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect();
    format!(
        "display dialog {} with title \"{TITLE}\" buttons {{\"OK\"}} default button 1 with icon stop",
        lines.join(" & return & ")
    )
}

pub(crate) fn tell(reason: &str) {
    let said = Message::NoGraphics(reason.to_owned()).say(Lang::English);
    eprintln!("{said}");
    if std::env::var_os("PANPDF_NO_DIALOG").is_some() {
        return;
    }
    for teller in order(Desk::here(), on_kde()) {
        if shown(teller, &said) {
            return;
        }
    }
}

fn shown(teller: Teller, said: &str) -> bool {
    let mut command = match teller {
        Teller::PowerShell => {
            let mut command = Command::new(windows_powershell());
            command
                .args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-Sta",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-Command",
                    powershell(),
                ])
                .env(SAID, said)
                .env("PANPDF_TITLE", TITLE);
            no_console(&mut command);
            command
        }
        Teller::OsaScript => {
            let mut command = Command::new("osascript");
            command.args(["-e", &applescript(said)]);
            command
        }
        Teller::Zenity => {
            let mut command = Command::new("zenity");
            command.args(zenity(said));
            command
        }
        Teller::KDialog => {
            let mut command = Command::new("kdialog");
            command.args(kdialog(said));
            command
        }
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match command.spawn() {
        Ok(mut child) => child.wait().is_ok_and(|status| status.success()),
        Err(_) => false,
    }
}

fn windows_powershell() -> std::path::PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    std::path::PathBuf::from(root).join("System32\\WindowsPowerShell\\v1.0\\powershell.exe")
}

#[cfg(target_os = "windows")]
fn no_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000);
}

#[cfg(not(target_os = "windows"))]
#[expect(
    clippy::missing_const_for_fn,
    reason = "the Windows arm is not const, and the two must have one signature"
)]
fn no_console(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use pdf_app::wording::{Lang, Message};

    use super::{Desk, Teller, applescript, kdialog, order, powershell, zenity};

    #[test]
    fn the_sentence_names_the_driver_and_not_opengl() {
        let said =
            Message::NoGraphics("egui_glow requires opengl 2.0+.".to_owned()).say(Lang::English);
        assert!(said.contains("graphics driver"), "{said}");
        assert!(said.contains("PanPDF"), "{said}");
        assert!(
            !said.to_ascii_lowercase().contains("opengl 2.0+.\n"),
            "the technical line must not be the sentence itself: {said}"
        );
        assert!(said.ends_with("egui_glow requires opengl 2.0+."), "{said}");
    }

    #[test]
    fn each_machine_is_told_which_program_to_try() {
        assert_eq!(order(Desk::Windows, false), vec![Teller::PowerShell]);
        assert_eq!(order(Desk::Windows, true), vec![Teller::PowerShell]);
        assert_eq!(order(Desk::Mac, false), vec![Teller::OsaScript]);
        assert_eq!(
            order(Desk::Unix, false),
            vec![Teller::Zenity, Teller::KDialog]
        );
        assert_eq!(
            order(Desk::Unix, true),
            vec![Teller::KDialog, Teller::Zenity]
        );
        assert!(order(Desk::Elsewhere, false).is_empty());
    }

    #[test]
    fn the_powershell_reads_the_sentence_from_the_environment() {
        let script = powershell();
        assert!(script.contains("$env:PANPDF_SAID"), "{script}");
        assert!(script.contains("MessageBox"), "{script}");
        assert!(!script.contains("graphics"), "{script}");
    }

    #[test]
    fn applescript_escapes_what_would_end_its_own_string() {
        let one = applescript("plain");
        assert!(one.contains("\"plain\""), "{one}");
        assert!(!one.contains(" & return & "), "{one}");
        let two = applescript("a \"quoted\" C:\\path\nsecond line");
        assert!(
            two.contains("\"a \\\"quoted\\\" C:\\\\path\" & return & \"second line\""),
            "{two}"
        );
        assert!(!two.contains('\n'), "{two}");
    }

    #[test]
    fn the_linux_programs_are_told_the_title_and_the_sentence() {
        assert_eq!(
            zenity("no driver"),
            vec!["--error", "--title=PanPDF", "--text=no driver"]
        );
        assert_eq!(
            kdialog("no driver"),
            vec!["--title=PanPDF", "--error", "no driver"]
        );
    }
}
