use std::sync::OnceLock;

const PLACES: [&str; 3] = [
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/cert.pem",
];

const MOST_BYTES: usize = 8 * 1024 * 1024;

static STORE: OnceLock<Store> = OnceLock::new();

pub(crate) struct Store {
    pub(crate) place: Option<&'static str>,
    pub(crate) roots: Vec<Vec<u8>>,
}

impl Store {
    pub(crate) fn of_this_computer() -> &'static Self {
        STORE.get_or_init(|| {
            for place in PLACES {
                let Ok(text) = std::fs::read(place) else {
                    continue;
                };
                if text.len() > MOST_BYTES {
                    continue;
                }
                let roots = certificates_in(&text);
                if !roots.is_empty() {
                    return Self {
                        place: Some(place),
                        roots,
                    };
                }
            }
            Self {
                place: None,
                roots: Vec::new(),
            }
        })
    }
}

fn certificates_in(text: &[u8]) -> Vec<Vec<u8>> {
    const OPEN: &[u8] = b"-----BEGIN CERTIFICATE-----";
    const CLOSE: &[u8] = b"-----END CERTIFICATE-----";
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = position(rest, OPEN) {
        let after = &rest[start + OPEN.len()..];
        let Some(end) = position(after, CLOSE) else {
            break;
        };
        if let Some(der) = unbase64(&after[..end]) {
            found.push(der);
        }
        rest = &after[end + CLOSE.len()..];
    }
    found
}

fn position(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn unbase64(text: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut held = 0u32;
    let mut bits = 0u32;
    for &letter in text {
        if letter.is_ascii_whitespace() {
            continue;
        }
        if letter == b'=' {
            break;
        }
        let value = match letter {
            b'A'..=b'Z' => u32::from(letter - b'A'),
            b'a'..=b'z' => u32::from(letter - b'a') + 26,
            b'0'..=b'9' => u32::from(letter - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        held = (held << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            #[expect(clippy::cast_possible_truncation, reason = "one octet is taken")]
            out.push((held >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{Store, certificates_in, unbase64};

    #[test]
    fn base_sixty_four_decodes_to_what_it_encodes() {
        assert_eq!(unbase64(b"aGVsbG8=").as_deref(), Some(&b"hello"[..]));
        assert_eq!(unbase64(b"aGVsbG8h").as_deref(), Some(&b"hello!"[..]));
        assert_eq!(unbase64(b"aGVsbG8").as_deref(), Some(&b"hello"[..]));
        assert_eq!(
            unbase64(b"aGVs\nbG8h\n").as_deref(),
            Some(&b"hello!"[..]),
            "the line breaks a bundle is wrapped at are not data"
        );
        assert_eq!(unbase64(b"not base 64!").as_deref(), None);
    }

    #[test]
    fn only_what_is_between_the_markers_is_a_certificate() {
        let root = include_bytes!("../tests/data/chain-root.der");
        let mut bundle = String::from("# A comment naming an authority\n");
        bundle.push_str("-----BEGIN CERTIFICATE-----\n");
        let letters = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut encoded = String::new();
        for chunk in root.chunks(3) {
            let mut held = 0u32;
            for (index, &octet) in chunk.iter().enumerate() {
                held |= u32::from(octet) << (16 - index * 8);
            }
            for index in 0..=chunk.len() {
                let value = (held >> (18 - index * 6)) & 0x3f;
                encoded.push(letters.as_bytes()[value as usize] as char);
            }
            for _ in chunk.len()..3 {
                encoded.push('=');
            }
        }
        for line in encoded.as_bytes().chunks(64) {
            bundle.push_str(std::str::from_utf8(line).expect("base 64 is text"));
            bundle.push('\n');
        }
        bundle.push_str("-----END CERTIFICATE-----\n");
        bundle.push_str("# and a trailing comment\n");

        let found = certificates_in(bundle.as_bytes());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0], root, "what went in is what comes out");
        assert!(
            certificates_in(b"# nothing but a comment\n").is_empty(),
            "a file with no certificates holds none, which is not a failure"
        );
    }

    #[test]
    fn the_store_this_computer_keeps_reads_as_certificates() {
        let store = Store::of_this_computer();
        if let Some(place) = store.place {
            assert!(!store.roots.is_empty(), "{place} held no certificates");
            let read = store
                .roots
                .iter()
                .filter(|der| crate::x509::Certificate::read(der).is_some())
                .count();
            assert_eq!(
                read,
                store.roots.len(),
                "every certificate in {place} reads"
            );
            println!("{place}: {} roots", store.roots.len());
        } else {
            println!("this computer keeps no bundle where one is looked for");
        }
    }
}
