use std::sync::atomic::AtomicBool;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [input, output, rest @ ..] = arguments.as_slice() else {
        return Err("usage: ocr IN OUT [--languages lao+tha+eng] [--pages 1-3]".into());
    };
    let mut languages = vec!["eng".to_owned()];
    let mut pages = None;
    let mut rest = rest.iter();
    while let Some(flag) = rest.next() {
        let value = rest.next().ok_or("a flag needs a value")?;
        match flag.as_str() {
            "--languages" => languages = value.split('+').map(str::to_owned).collect(),
            "--pages" => pages = Some(value.clone()),
            _ => return Err(format!("unknown flag {flag}").into()),
        }
    }
    let engine = pdf_ocr::Tesseract::locate()?;
    let source = pdf_bytes::ByteStore::new(pdf_bytes::SourceId::new(1), std::fs::read(input)?);
    let mut session = pdf_session::Session::new(source, b"");
    let count = session.page_count()?;
    let chosen = match &pages {
        Some(spec) => pdf_edit::stamp::pages_of(spec, count, pdf_edit::stamp::Only::Every)?,
        None => (0..count).collect(),
    };
    let never = AtomicBool::new(false);
    let mut commands = Vec::new();
    let mut first = None;
    for &index in &chosen {
        let view = session.page(index)?;
        let started = std::time::Instant::now();
        let read = pdf_ocr::read_page(
            &view.layers(),
            &view.program.geometry,
            &engine,
            &languages,
            &never,
        )?;
        eprintln!(
            "page {}: {} words, confidence {:.1}, {:.1} s",
            index + 1,
            read.layer.words.len(),
            read.confidence.unwrap_or(0.0),
            started.elapsed().as_secs_f64()
        );
        if read.layer.words.is_empty() {
            continue;
        }
        commands.push(pdf_edit::plan::Command::TextLayer {
            page_index: index,
            layer: read.layer,
            share_from: first,
        });
        first.get_or_insert(index);
    }
    session.apply_each(&commands)?;
    std::fs::write(output, session.source().as_bytes())?;
    Ok(())
}
