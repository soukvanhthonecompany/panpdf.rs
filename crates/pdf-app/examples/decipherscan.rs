use std::sync::Arc;
use std::time::Instant;

use pdf_bytes::{ByteStore, SourceId};
use pdf_paint::PaintAtomKind;

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = arguments.first() else {
        eprintln!("usage: decipherscan file.pdf page...");
        return;
    };
    let Ok(bytes) = std::fs::read(path) else {
        eprintln!("{path}: unreadable");
        return;
    };
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let Some(provider) = pdf_cli::font_provider() else {
        eprintln!("no font provider");
        return;
    };
    for page in arguments[1..]
        .iter()
        .filter_map(|page| page.parse::<usize>().ok())
    {
        let started = Instant::now();
        let Ok(view) = pdf_session::interpret_page_fully(
            &source,
            page,
            b"",
            None,
            Some(Arc::clone(&provider)),
        ) else {
            println!("{path}\t{page}\tdoes not read");
            continue;
        };
        let elapsed = started.elapsed();
        for (atoms, cipher) in pdf_paint::decipher_fonts::font_evidence(&view.graph) {
            let family = atoms
                .iter()
                .find_map(|at| match &view.graph.atoms[*at].kind {
                    PaintAtomKind::Text(text) => text
                        .font_request
                        .as_ref()
                        .map(|request| request.family.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            let Some((shapes, model)) =
                pdf_paint::decipher_fonts::references(&view.graph, &atoms, provider.as_ref())
            else {
                continue;
            };
            let reading = pdf_content::decipher::decipher(&cipher, &shapes, model);
            let (mut agree, mut shown) = (0, 0);
            for line in &cipher.lines {
                for &code in line {
                    if cipher.drawings[code].is_some() {
                        shown += 1;
                        agree += usize::from(
                            cipher.claims[code].map(String::from).as_ref()
                                == Some(&reading.characters[code]),
                        );
                    }
                }
            }
            print_asked(&cipher, &reading, &shapes, model);
            let deciphered =
                pdf_paint::decipher_fonts::reads_by_glyphs(&view.graph, &atoms, provider.as_ref());
            let first_line = |read: &dyn Fn(usize) -> String| -> String {
                cipher
                    .lines
                    .iter()
                    .find(|line| line.len() > 8)
                    .map(|line| line.iter().map(|code| read(*code)).collect::<String>())
                    .unwrap_or_default()
                    .chars()
                    .take(60)
                    .collect()
            };
            let file = first_line(&|code| {
                cipher.claims[code].map_or_else(|| "?".to_owned(), String::from)
            });
            let read = first_line(&|code| reading.characters[code].clone());
            println!(
                "{path}\t{page}\t{family}\tgain {:.3}\tchanged {:.2}\tevidence {}\tfluency {:.2}\t{}\tagree {agree}/{shown}\t{:.0} ms\tfile: {file}\tread: {read}",
                reading.gain,
                reading.changed,
                reading.evidence,
                reading.fluency,
                if deciphered { "DECIPHERED" } else { "kept" },
                elapsed.as_secs_f64() * 1000.0,
            );
        }
    }
}

fn print_disagreements(
    cipher: &pdf_content::decipher::Cipher,
    reading: &pdf_content::decipher::Reading,
) {
    let mut wrong: std::collections::BTreeMap<(String, String), usize> =
        std::collections::BTreeMap::new();
    for line in &cipher.lines {
        for &code in line {
            if cipher.drawings[code].is_some()
                && cipher.claims[code].map(String::from).as_ref() != Some(&reading.characters[code])
            {
                let claim = cipher.claims[code].map_or_else(|| "?".to_owned(), String::from);
                *wrong
                    .entry((claim, reading.characters[code].clone()))
                    .or_default() += 1;
            }
        }
    }
    let mut wrong: Vec<_> = wrong.into_iter().collect();
    wrong.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    println!("  wrong: {:?}", &wrong[..wrong.len().min(15)]);
}

fn print_shared(cipher: &pdf_content::decipher::Cipher, reading: &pdf_content::decipher::Reading) {
    let mut by_character: std::collections::BTreeMap<&str, Vec<usize>> =
        std::collections::BTreeMap::new();
    for (code, character) in reading.characters.iter().enumerate() {
        if cipher.drawings[code].is_some() {
            by_character.entry(character).or_default().push(code);
        }
    }
    for (character, codes) in by_character
        .into_iter()
        .filter(|(_, codes)| codes.len() > 1)
    {
        let shown: Vec<String> = codes
            .iter()
            .map(|code| {
                let times = cipher
                    .lines
                    .iter()
                    .flatten()
                    .filter(|at| **at == *code)
                    .count();
                format!(
                    "{:#04x} file {:?} x{times}",
                    cipher.codes[*code].value, cipher.claims[*code]
                )
            })
            .collect();
        println!("  shared {character:?}: {}", shown.join(", "));
    }
}

fn print_asked(
    cipher: &pdf_content::decipher::Cipher,
    reading: &pdf_content::decipher::Reading,
    shapes: &pdf_content::decipher::Shapes,
    model: &pdf_content::decipher::CharModel,
) {
    if std::env::var_os("DECIPHERSCAN_DIFF").is_some() {
        print_disagreements(cipher, reading);
    }
    if let Some(codes) = std::env::var_os("DECIPHERSCAN_CANDIDATES") {
        for (at, code) in cipher.codes.iter().enumerate() {
            let wanted = format!("{:02x}", code.value);
            if !codes
                .to_string_lossy()
                .split(',')
                .any(|each| each == wanted)
            {
                continue;
            }
            if let Some(features) = &cipher.drawings[at] {
                let ranked: Vec<String> = shapes
                    .ranked(features, |text| model.knows_text(text))
                    .into_iter()
                    .take(6)
                    .map(|(text, likeness)| format!("{text:?} {likeness:.3}"))
                    .collect();
                println!(
                    "  {wanted} read {:?}: {}",
                    reading.characters[at],
                    ranked.join(", ")
                );
            }
        }
    }
    if std::env::var_os("DECIPHERSCAN_LINES").is_some() {
        for line in &cipher.lines {
            let text: String = line
                .iter()
                .map(|code| reading.characters[*code].as_str())
                .collect();
            println!("  | {text}");
        }
    }
    if std::env::var_os("DECIPHERSCAN_SHARED").is_some() {
        print_shared(cipher, reading);
    }
}
