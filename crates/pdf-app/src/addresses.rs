use pdf_cli::TextClusterBox;

const LONGEST: usize = 2_048;

#[derive(Clone, Debug, PartialEq)]
pub struct Written {
    pub address: String,
    pub shown: String,
    pub pixels: [f64; 4],
}

#[must_use]
pub fn found_in(clusters: &[TextClusterBox]) -> Vec<Written> {
    let mut rows: Vec<Vec<&TextClusterBox>> = Vec::new();
    for cluster in clusters {
        while rows.len() <= cluster.line {
            rows.push(Vec::new());
        }
        rows[cluster.line].push(cluster);
    }
    let mut found = Vec::new();
    for row in &mut rows {
        row.sort_by_key(|cluster| cluster.index_in_line);
        found.extend(in_row(row));
    }
    found
}

fn in_row(row: &[&TextClusterBox]) -> Vec<Written> {
    let mut text = String::new();
    let mut from: Vec<usize> = Vec::new();
    for (at, cluster) in row.iter().enumerate() {
        let Some(said) = cluster.text.as_ref() else {
            text.push(' ');
            from.push(at);
            continue;
        };
        for character in said.chars() {
            text.push(character);
            from.push(at);
        }
    }
    let mut found = Vec::new();
    for (start, end) in spans(&text) {
        let Some(address) = addressed(&text[start..end]) else {
            continue;
        };
        let boxes: Vec<[f64; 4]> = text
            .char_indices()
            .enumerate()
            .filter(|(_, (byte, _))| (start..end).contains(byte))
            .filter_map(|(at, _)| row.get(*from.get(at)?))
            .filter_map(|cluster| cluster.box_pixels)
            .collect();
        let Some(pixels) = around(&boxes) else {
            continue;
        };
        found.push(Written {
            address,
            shown: text[start..end].to_owned(),
            pixels,
        });
    }
    found
}

fn spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start: Option<usize> = None;
    for (at, character) in text.char_indices() {
        if character.is_whitespace() {
            if let Some(from) = start.take() {
                spans.push((from, at));
            }
        } else if start.is_none() {
            start = Some(at);
        }
    }
    if let Some(from) = start {
        spans.push((from, text.len()));
    }
    spans
}

fn addressed(word: &str) -> Option<String> {
    let word = trimmed(word);
    if word.is_empty() || word.len() > LONGEST {
        return None;
    }
    let lower = word.to_ascii_lowercase();
    let address = if lower.starts_with("http://") || lower.starts_with("https://") {
        word.to_owned()
    } else if lower.starts_with("www.") && word.len() > 4 {
        format!("https://{word}")
    } else if lower.starts_with("mailto:") {
        word.to_owned()
    } else if is_an_email(word) {
        format!("mailto:{word}")
    } else {
        return None;
    };
    if !crate::links::is_openable(&address) || !has_a_dotted_host(&address) {
        return None;
    }
    Some(address)
}

fn has_a_dotted_host(address: &str) -> bool {
    let Some((_, rest)) = address.split_once(':') else {
        return false;
    };
    let rest = rest.trim_start_matches('/');
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit('@')
        .next()
        .unwrap_or_default();
    let host = host.split(':').next().unwrap_or_default();
    host.contains('.') && !host.starts_with('.') && !host.ends_with('.')
}

fn trimmed(word: &str) -> &str {
    let word = word.trim_start_matches(['(', '[', '<', '"', '\'', '«']);
    let mut end = word.len();
    while let Some(last) = word[..end].chars().next_back() {
        let unwanted = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '"' | '\'' | '»' | '>' => true,
            ')' => word[..end].matches('(').count() < word[..end].matches(')').count(),
            ']' => word[..end].matches('[').count() < word[..end].matches(']').count(),
            _ => false,
        };
        if !unwanted {
            break;
        }
        end -= last.len_utf8();
    }
    &word[..end]
}

fn is_an_email(word: &str) -> bool {
    let Some((who, host)) = word.split_once('@') else {
        return false;
    };
    !who.is_empty()
        && !host.is_empty()
        && !host.contains('@')
        && host.contains('.')
        && !host.starts_with('.')
        && !host.ends_with('.')
        && !who.contains(|character: char| character.is_whitespace())
}

fn around(boxes: &[[f64; 4]]) -> Option<[f64; 4]> {
    let first = *boxes.first()?;
    Some(boxes.iter().fold(first, |held, one| {
        [
            held[0].min(one[0]),
            held[1].min(one[1]),
            held[2].max(one[2]),
            held[3].max(one[3]),
        ]
    }))
}

#[cfg(test)]
#[expect(
    clippy::float_cmp,
    reason = "a box of numbers taken from clusters is the same numbers"
)]
mod tests {
    use super::{Written, found_in};
    use pdf_cli::TextClusterBox;

    fn row(line: usize, said: &str, left: f64) -> Vec<TextClusterBox> {
        said.chars()
            .enumerate()
            .map(|(at, character)| {
                #[expect(clippy::cast_precision_loss, reason = "a few dozen characters")]
                let x = left + at as f64 * 10.0;
                TextClusterBox {
                    anchor: String::new(),
                    glyphs: 0..1,
                    box_pixels: Some([x, 100.0, x + 10.0, 112.0]),
                    stacked: false,
                    line,
                    index_in_line: at,
                    text: Some(character.to_string()),
                }
            })
            .collect()
    }

    fn addresses(said: &str) -> Vec<String> {
        found_in(&row(0, said, 0.0))
            .into_iter()
            .map(|written| written.address)
            .collect()
    }

    #[test]
    fn the_shapes_people_write() {
        assert_eq!(
            addresses("See https://example.org/a and www.example.org too"),
            ["https://example.org/a", "https://www.example.org"]
        );
        assert_eq!(
            addresses("Write to someone@example.org please"),
            ["mailto:someone@example.org"]
        );
        assert_eq!(addresses("http://example.org"), ["http://example.org"]);
    }

    #[test]
    fn punctuation_round_an_address_is_not_part_of_it() {
        assert_eq!(
            addresses("(see https://example.org/a.)"),
            ["https://example.org/a"]
        );
        assert_eq!(
            addresses("https://example.org/a_(b)"),
            ["https://example.org/a_(b)"]
        );
        assert_eq!(
            addresses("https://example.org/a,"),
            ["https://example.org/a"]
        );
    }

    #[test]
    fn what_is_not_offered() {
        for said in [
            "a sentence with no address in it",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "example.org",
            "@example.org",
            "someone@",
            "someone@example",
            "ftp://example.org/a",
        ] {
            assert!(addresses(said).is_empty(), "this is not a link: {said}");
        }
    }

    #[test]
    fn the_box_is_round_the_address() {
        let found = found_in(&row(0, "a https://b.example c", 0.0));
        assert_eq!(found.len(), 1);
        let Written { pixels, shown, .. } = &found[0];
        assert_eq!(shown, "https://b.example");
        assert_eq!(pixels[0], 20.0, "after \"a \"");
        assert_eq!(pixels[2], 190.0, "seventeen characters of ten points");
        assert_eq!([pixels[1], pixels[3]], [100.0, 112.0]);
    }

    #[test]
    fn an_address_is_not_read_across_two_rows() {
        let mut clusters = row(0, "htt", 0.0);
        clusters.extend(row(1, "ps://example.org/a", 0.0));
        assert!(
            found_in(&clusters).is_empty(),
            "neither half is an address on its own"
        );
    }

    #[test]
    fn an_unreadable_cluster_breaks_an_address() {
        let mut clusters = row(0, "https://example.org", 0.0);
        clusters[10].text = None;
        assert!(found_in(&clusters).is_empty());
    }
}
