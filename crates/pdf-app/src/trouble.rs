use std::fmt::Write as _;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub version: String,
    pub system: String,
    pub architecture: String,
    pub renderer: String,
    pub fonts: bool,
    pub log: Option<String>,
}

const ASKED: &str = "\
## What happened

## What I expected instead

## How to make it happen again

1.
2.
3.
";

#[must_use]
pub fn describe(facts: &Facts) -> String {
    let mut said = String::from(ASKED);
    said.push_str("\n## This machine\n\n");
    let _ = writeln!(said, "- PanPDF {}", facts.version);
    let _ = writeln!(said, "- {} on {}", facts.system, facts.architecture);
    let _ = writeln!(said, "- Drawing with {}", facts.renderer);
    let _ = writeln!(
        said,
        "- Packaged fonts: {}",
        if facts.fonts { "found" } else { "not found" }
    );
    match &facts.log {
        Some(where_it_is) => {
            let _ = writeln!(
                said,
                "\nThere is a log of this session at `{where_it_is}`. It stays on \
                 your computer until you attach it yourself. It names the documents \
                 you opened, so read it before you do."
            );
        }
        None => said.push_str("\nThis session kept no log.\n"),
    }
    said
}

#[must_use]
pub fn issue_link(repository: &str, title: &str, body: &str) -> String {
    let full = format!(
        "https://github.com/{repository}/issues/new?title={}&body={}",
        encoded(title),
        encoded(body)
    );
    if full.len() <= MOST_ADDRESS {
        full
    } else {
        format!(
            "https://github.com/{repository}/issues/new?title={}",
            encoded(title)
        )
    }
}

pub const MOST_ADDRESS: usize = 6000;

fn encoded(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Session,
    Document,
    Refused,
    Failed,
    Panicked,
}

impl Kind {
    const fn word(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Document => "document",
            Self::Refused => "refused",
            Self::Failed => "failed",
            Self::Panicked => "panicked",
        }
    }
}

#[must_use]
pub fn log_line(seconds: u64, kind: Kind, said: &str) -> String {
    let flattened: String = said
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    format!("{seconds:>6} {:<9} {}\n", kind.word(), flattened.trim())
}

#[must_use]
pub fn redacted(text: &str, home: Option<&str>) -> String {
    match home.map(|home| home.trim_end_matches(['/', '\\'])) {
        Some(home) if !home.is_empty() => text.replace(home, "~"),
        _ => text.to_owned(),
    }
}

#[must_use]
pub fn trimmed(existing: &str, most: usize) -> &str {
    if existing.len() <= most {
        return existing;
    }
    let from = existing.len() - most;
    match existing[from..].find('\n') {
        Some(newline) => &existing[from + newline + 1..],
        None => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            version: "0.1.0".to_owned(),
            system: "windows".to_owned(),
            architecture: "x86_64".to_owned(),
            renderer: "wgpu Dx12".to_owned(),
            fonts: true,
            log: Some("~/AppData/Roaming/panpdf/log".to_owned()),
        }
    }

    #[test]
    fn a_report_carries_the_machine_and_nothing_else() {
        let private = [
            "tax-return.pdf",
            "/home/someone",
            "someone",
            r"C:\Users\someone",
        ];
        let said = describe(&facts());
        for secret in private {
            assert!(
                !said.contains(secret),
                "a report must not carry {secret}, and this one does:\n{said}"
            );
        }
        let helpful = format!("{said}\n- Open document: /home/someone/tax-return.pdf");
        assert!(
            private.iter().any(|secret| helpful.contains(secret)),
            "the control must disagree, or it is not measuring anything"
        );
        assert!(said.contains("PanPDF 0.1.0"));
        assert!(said.contains("What happened"));
    }

    #[test]
    fn the_report_points_at_the_log_without_sending_it() {
        let said = describe(&facts());
        assert!(said.contains("AppData/Roaming/panpdf/log"));
        assert!(said.contains("stays on your computer"));
        let mut without = facts();
        without.log = None;
        assert!(describe(&without).contains("kept no log"));
    }

    #[test]
    fn the_address_is_encoded_as_a_query_string() {
        assert_eq!(encoded("a b"), "a%20b");
        assert_eq!(encoded("#1 & 2"), "%231%20%26%202");
        assert_eq!(encoded("one\ntwo"), "one%0Atwo");
        assert_eq!(encoded("~-_.aZ9"), "~-_.aZ9");
        assert_eq!(encoded("ลาว"), "%E0%B8%A5%E0%B8%B2%E0%B8%A7");
    }

    #[test]
    fn a_body_that_will_not_fit_is_left_out_whole() {
        let short = issue_link("someone/panpdf", "It crashed", "small");
        assert!(short.contains("&body=small"));
        let long = issue_link("someone/panpdf", "It crashed", &"x".repeat(MOST_ADDRESS));
        assert!(!long.contains("&body="));
        assert!(long.len() < MOST_ADDRESS);
        assert!(long.starts_with("https://github.com/someone/panpdf/issues/new?title="));
    }

    #[test]
    fn a_log_line_is_one_line() {
        let line = log_line(12, Kind::Refused, "a colour change\nunder a clip\r\n");
        assert_eq!(line.matches('\n').count(), 1);
        assert!(line.ends_with('\n'));
        assert!(line.contains("refused"));
        assert!(line.contains("a colour change under a clip"));
        assert_eq!(
            log_line(0, Kind::Session, "started"),
            "     0 session   started\n"
        );
    }

    #[test]
    fn a_path_loses_the_home_folder() {
        assert_eq!(
            redacted(
                "read /home/someone/tax.pdf, wrote /home/someone/out.pdf",
                Some("/home/someone")
            ),
            "read ~/tax.pdf, wrote ~/out.pdf"
        );
        assert_eq!(
            redacted(r"C:\Users\someone\tax.pdf", Some(r"C:\Users\someone")),
            r"~\tax.pdf"
        );
        assert_eq!(
            redacted("/home/someone/tax.pdf", None),
            "/home/someone/tax.pdf"
        );
        assert_eq!(
            redacted("/home/someone/tax.pdf", Some("")),
            "/home/someone/tax.pdf"
        );
    }

    #[test]
    fn trimming_keeps_the_end_and_never_half_a_line() {
        let log = "one\ntwo\nthree\nfour\n";
        assert_eq!(trimmed(log, 100), log);
        let kept = trimmed(log, 10);
        assert!(log.ends_with(kept));
        assert!(kept.len() <= 10);
        assert!(kept.is_empty() || log[..log.len() - kept.len()].ends_with('\n'));
        assert_eq!(trimmed("no newline at all", 5), "");
    }
}
