use std::error::Error;

use pdf_bytes::{ByteStore, SourceId};

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

type Picture = (usize, std::sync::Arc<[u8]>);

fn images(source: &ByteStore) -> Result<Vec<Picture>, Box<dyn Error>> {
    let view = pdf_session::interpret_page_fully(source, 0, b"", None, pdf_cli::font_provider())?;
    Ok(view
        .graph
        .atoms
        .iter()
        .enumerate()
        .filter_map(|(ordinal, atom)| match &atom.kind {
            pdf_paint::PaintAtomKind::Image(image) => Some((ordinal, image.samples.clone())),
            _ => None,
        })
        .collect())
}

fn report(first: &[u8], again: &[u8]) {
    if first.len() != again.len() {
        println!(
            "    lengths differ: {} against {}",
            first.len(),
            again.len()
        );
        return;
    }
    let offsets: Vec<usize> = first
        .iter()
        .zip(again.iter())
        .enumerate()
        .filter_map(|(at, (one, other))| (one != other).then_some(at))
        .collect();
    println!(
        "    {} of {} bytes differ, first at {:?}, last at {:?}",
        offsets.len(),
        first.len(),
        offsets.first(),
        offsets.last()
    );
    for at in offsets.iter().take(8) {
        println!(
            "      byte {at}: {:#04x} against {:#04x}",
            first[*at], again[*at]
        );
    }
    if let (Some(&start), Some(&end)) = (offsets.first(), offsets.last()) {
        let contiguous = offsets.len() == end - start + 1;
        println!("    contiguous: {contiguous}; span {start}..={end}");
        let tail_zero = again[start..=end].iter().all(|byte| *byte == again[start]);
        println!("    the second reading holds one repeated value there: {tail_zero}");
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    let path = arguments
        .next()
        .ok_or("usage: imagerepeat <file.pdf> [readings]")?;
    let readings: usize = arguments.next().map_or(Ok(10), |text| text.parse())?;

    let source = ByteStore::new(SourceId::new(0), std::fs::read(&path)?);
    let first = images(&source)?;
    println!("page 0 holds {} image atoms", first.len());
    for (ordinal, samples) in &first {
        println!(
            "  atom {ordinal}: {} bytes, {:016x}",
            samples.len(),
            fnv1a(samples)
        );
    }

    let mut disagreements = 0usize;
    for reading in 2..=readings {
        let again = images(&source)?;
        if again.len() != first.len() {
            println!(
                "reading {reading}: {} image atoms, not {}",
                again.len(),
                first.len()
            );
            disagreements += 1;
            continue;
        }
        for ((ordinal, one), (_, other)) in first.iter().zip(again.iter()) {
            if one == other {
                continue;
            }
            disagreements += 1;
            println!(
                "reading {reading}: atom {ordinal} decoded differently ({:016x} against {:016x})",
                fnv1a(one),
                fnv1a(other)
            );
            report(one, other);
        }
    }
    println!("readings: {readings}, disagreements: {disagreements}");
    std::process::exit(i32::from(disagreements != 0));
}
