use pdf_content::{Followed, Link, LinkAction};

use crate::wording::{LinkKind, Message};

#[must_use]
pub fn contains(link: &Link, x: f64, y: f64) -> bool {
    let [x0, y0, x1, y1] = link.rect;
    (x0..=x1).contains(&x) && (y0..=y1).contains(&y)
}

#[must_use]
pub fn is_openable(uri: &str) -> bool {
    let Some((scheme, rest)) = uri.split_once(':') else {
        return false;
    };
    let known = ["http", "https", "mailto"]
        .iter()
        .any(|allowed| scheme.eq_ignore_ascii_case(allowed));
    known
        && !rest.is_empty()
        && !uri
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
}

#[must_use]
pub fn describe(link: &Link, page_count: usize) -> Message {
    match link.followed() {
        Followed::Page(destination) => match destination.page {
            Some(page) if page < page_count => Message::LinkToPage(page + 1),
            _ => Message::LinkToMissingPage,
        },
        Followed::Uri(uri) => {
            let uri = String::from_utf8_lossy(uri).into_owned();
            if is_openable(&uri) {
                Message::LinkToAddress(uri)
            } else {
                Message::LinkToUnopenableAddress(uri)
            }
        }
        Followed::Other(action) => Message::LinkDoesSomethingElse(match action {
            LinkAction::RemoteGoTo { .. } => LinkKind::AnotherFile,
            LinkAction::EmbeddedGoTo { .. } => LinkKind::AttachedFile,
            LinkAction::Launch { .. } => LinkKind::LaunchProgram,
            LinkAction::Named(_) => LinkKind::ReaderCommand,
            LinkAction::Other(_) | LinkAction::Uri(_) | LinkAction::GoTo(_) => LinkKind::SomeAction,
        }),
        Followed::Nothing => Message::LinkPointsNowhere,
    }
}

#[cfg(test)]
mod tests {
    use pdf_content::{Destination, Link, LinkAction, View};

    use super::{contains, describe, is_openable};
    use crate::wording::Message;

    fn link(action: Option<LinkAction>, destination: Option<Destination>) -> Link {
        Link {
            reference: None,
            rect: [10.0, 20.0, 110.0, 40.0],
            quad_points: Vec::new(),
            flags: 0,
            action,
            destination,
        }
    }

    #[test]
    fn the_web_and_mail_are_opened() {
        assert!(is_openable("https://example.com/a?b=c"));
        assert!(is_openable("HTTP://example.com"));
        assert!(is_openable("mailto:someone@example.com"));
    }

    #[test]
    fn nothing_else_is_opened() {
        assert!(!is_openable("file:///etc/passwd"));
        assert!(!is_openable("javascript:alert(1)"));
        assert!(!is_openable("smb://server/share"));
        assert!(!is_openable("https:"));
        assert!(!is_openable("relative/page.html"));
        assert!(!is_openable("https://example.com/a b"));
        assert!(!is_openable("https://example.com/\u{7}"));
    }

    #[test]
    fn a_rectangle_holds_its_edges_and_nothing_outside() {
        let found = link(None, None);
        assert!(contains(&found, 10.0, 20.0));
        assert!(contains(&found, 60.0, 30.0));
        assert!(!contains(&found, 9.99, 30.0));
        assert!(!contains(&found, 60.0, 40.01));
    }

    #[test]
    fn a_hint_names_the_page_counting_from_one_and_refuses_a_missing_one() {
        let to = |page| {
            link(
                None,
                Some(Destination {
                    page: Some(page),
                    view: View::Fit,
                }),
            )
        };
        assert_eq!(describe(&to(0), 3), Message::LinkToPage(1));
        assert_eq!(describe(&to(3), 3), Message::LinkToMissingPage);
    }

    #[test]
    fn a_hint_says_when_an_address_will_not_be_opened() {
        let web = link(Some(LinkAction::Uri(b"https://example.com".to_vec())), None);
        assert!(matches!(describe(&web, 1), Message::LinkToAddress(_)));
        let local = link(Some(LinkAction::Uri(b"file:///tmp/x".to_vec())), None);
        assert!(matches!(
            describe(&local, 1),
            Message::LinkToUnopenableAddress(_)
        ));
    }
}
