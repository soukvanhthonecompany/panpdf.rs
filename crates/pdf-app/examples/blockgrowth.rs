use std::sync::Arc;
use std::time::Instant;

use pdf_app::{Applied, Editor};
use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::BlockRange;

const SIZES: [usize; 4] = [100, 1_000, 10_000, 50_000];

const PER_LINE: usize = 50;

const POINTS: f64 = 10.0;

const SQUARE_CFF: &str = "0100040100010101055465737400010101131d00000030111d000000540f1d0000\
    0059100001010106616c70686100000003010102101e0e8b8b15f8888b8bf888fc888b050e8b8b15f8888b8bf888fc\
    888b050e0000220187000141";

fn hex(text: &str) -> Vec<u8> {
    text.bytes()
        .filter(u8::is_ascii_hexdigit)
        .collect::<Vec<u8>>()
        .chunks_exact(2)
        .map(|pair| {
            u8::from_str_radix(std::str::from_utf8(pair).expect("hex digits"), 16)
                .expect("hex byte")
        })
        .collect()
}

fn stream(bytes: &[u8], extra: &str) -> Vec<u8> {
    let mut result = format!("<< /Length {} {extra} >>\nstream\n", bytes.len()).into_bytes();
    result.extend_from_slice(bytes);
    result.extend_from_slice(b"\nendstream");
    result
}

#[expect(
    clippy::cast_precision_loss,
    reason = "counts here are thousands, not quadrillions"
)]
fn counted(how_many: usize) -> f64 {
    how_many as f64
}

fn paragraph(characters: usize) -> ByteStore {
    let rows = characters.div_ceil(PER_LINE);
    let leading = POINTS * 1.2;
    let width = 40.0 + POINTS * 0.6 * counted(PER_LINE);
    let height = 40.0 + leading * counted(rows);
    let mut content = Vec::new();
    let mut left = characters;
    for row in 0..rows {
        let on_this_row = left.min(PER_LINE);
        left -= on_this_row;
        let y = height - 20.0 - leading * counted(row + 1);
        content.extend_from_slice(format!("BT /F1 {POINTS} Tf 1 0 0 1 20 {y:.2} Tm (").as_bytes());
        content.extend(std::iter::repeat_n(b'A', on_this_row));
        content.extend_from_slice(b") Tj ET\n");
    }
    let program = hex(SQUARE_CFF);
    let objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /MediaBox [0 0 {width:.2} {height:.2}] /Kids [3 0 R] /Count 1 >>")
            .into_bytes(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_vec(),
        stream(&content, ""),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Test /Encoding << /Differences [65 /A /alpha] \
          >> /FirstChar 65 /LastChar 66 /Widths [600 600] /FontDescriptor 6 0 R /ToUnicode 8 0 R \
          >>"
        .to_vec(),
        b"<< /Type /FontDescriptor /FontName /Test /Flags 4 /Ascent 800 /Descent -200 \
          /FontFile3 7 0 R >>"
            .to_vec(),
        stream(&program, "/Subtype /Type1C"),
        stream(
            b"begincmap 1 begincodespacerange <00> <ff> endcodespacerange 2 beginbfchar <41> \
              <0041> <42> <0042> endbfchar endcmap",
            "",
        ),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        bytes.extend_from_slice(body);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let xref = bytes.len();
    bytes.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    ByteStore::new(SourceId::new(864), Arc::<[u8]>::from(bytes))
}

fn read(editor: &mut Editor) -> bool {
    let Some(source) = editor.source().cloned() else {
        return false;
    };
    match pdf_session::interpret_page_fully(
        &source,
        0,
        b"",
        editor.grouping(0).as_deref(),
        pdf_cli::font_provider(),
    ) {
        Ok(view) => {
            editor.adopt_page(0, Arc::new(view));
            true
        }
        Err(_) => false,
    }
}

fn timed<T>(work: impl FnOnce() -> T) -> (T, f64) {
    let start = Instant::now();
    let answer = work();
    (answer, start.elapsed().as_secs_f64() * 1000.0)
}

mod growth {
    pub fn steps(points: &[(f64, f64)]) -> Vec<Option<f64>> {
        points.windows(2).map(exponent).collect()
    }

    pub fn exponent(points: &[(f64, f64)]) -> Option<f64> {
        let usable: Vec<(f64, f64)> = points
            .iter()
            .filter(|(size, cost)| *size > 0.0 && *cost > 0.0)
            .map(|(size, cost)| (size.ln(), cost.ln()))
            .collect();
        if usable.len() < 2 {
            return None;
        }
        let count = super::counted(usable.len());
        let mean_x = usable.iter().map(|(x, _)| x).sum::<f64>() / count;
        let mean_y = usable.iter().map(|(_, y)| y).sum::<f64>() / count;
        let top: f64 = usable
            .iter()
            .map(|(x, y)| (x - mean_x) * (y - mean_y))
            .sum();
        let bottom: f64 = usable.iter().map(|(x, _)| (x - mean_x).powi(2)).sum();
        (bottom > f64::EPSILON).then(|| top / bottom)
    }

    #[cfg(test)]
    mod tests {
        use super::exponent;

        #[test]
        fn a_known_linear_and_a_known_quadratic_series_come_back_as_one_and_two() {
            let sizes = [100.0, 1_000.0, 10_000.0, 50_000.0];
            let linear: Vec<(f64, f64)> = sizes.iter().map(|n| (*n, 0.004 * n)).collect();
            let square: Vec<(f64, f64)> = sizes.iter().map(|n| (*n, 1e-6 * n * n)).collect();
            let root: Vec<(f64, f64)> = sizes.iter().map(|n| (*n, n.sqrt())).collect();
            assert!((exponent(&linear).unwrap() - 1.0).abs() < 1e-9);
            assert!((exponent(&square).unwrap() - 2.0).abs() < 1e-9);
            assert!((exponent(&root).unwrap() - 0.5).abs() < 1e-9);
            assert_eq!(exponent(&[(100.0, 1.0)]), None);
            assert_eq!(exponent(&[(100.0, 0.0), (1_000.0, -1.0)]), None);
        }
    }
}

fn quadratic_busywork(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut seen = 0_usize;
    for (index, byte) in bytes.iter().enumerate().flat_map(|pair| [pair; 4]) {
        for other in &bytes[..index] {
            if std::hint::black_box(*other) == *byte {
                seen = seen.wrapping_add(1);
            }
        }
    }
    seen
}

struct Row {
    characters: usize,
    rows: usize,
    first: f64,
    reading: f64,
    planning: f64,
    typing: f64,
    reread: f64,
    moving: f64,
}

fn measure(characters: usize, prove: bool) -> Option<Row> {
    let source = paragraph(characters);
    let mut editor = Editor::open(source).expect("open");
    let (opened, first) = timed(|| read(&mut editor));
    if !opened {
        println!("{characters}: the page could not be read");
        return None;
    }
    let leaf = editor.leaf(0).cloned()?;
    let (block, rows) = leaf
        .overlay
        .blocks
        .iter()
        .enumerate()
        .max_by_key(|(_, block)| block.lines.len())
        .map(|(index, block)| (index, block.lines.len()))?;
    let anchors = leaf.overlay.blocks[block].anchors.clone();

    let middle = rows / 2;
    let stops = leaf.overlay.blocks[block]
        .lines
        .get(middle)
        .and_then(|line| leaf.view.index.lines.get(*line))
        .map_or(0, |line| line.clusters.len());
    let at = (middle, stops / 2);
    let (read_again, reading) = timed(|| editor.block_reading(0, block));
    assert!(read_again.is_some(), "the block could not be read");
    let (refused, planning) =
        timed(|| editor.block_refusal(0, block, BlockRange::Between { from: at, to: at }, "x"));
    assert!(refused.is_none(), "planning answered {refused:?}");

    let charge = prove
        .then(|| editor.copy_text(0, block, (0, 0), (rows - 1, 0)))
        .flatten();
    let (applied, typing) = timed(|| {
        if let Some(text) = charge.as_deref() {
            std::hint::black_box(quadratic_busywork(text));
        }
        editor.edit(0, block, BlockRange::Between { from: at, to: at }, "x")
    });
    if !matches!(applied, Applied::Changed { .. }) {
        println!("{characters}: typing answered {applied:?}");
        return None;
    }
    let (fresh, reread) = timed(|| read(&mut editor));
    assert!(fresh, "the page became unreadable after typing");

    let anchors = if editor
        .leaf(0)
        .and_then(|leaf| leaf.overlay.blocks.get(block))
        .is_some_and(|block| block.anchors == anchors)
    {
        anchors
    } else {
        editor.leaf(0)?.overlay.blocks.get(block)?.anchors.clone()
    };
    let (moved, moving) = timed(|| editor.move_block(0, &anchors, 1.0, 0.0));
    if !matches!(moved, Applied::Changed { .. }) {
        println!("{characters}: moving answered {moved:?}");
        return None;
    }
    Some(Row {
        characters,
        rows,
        first,
        reading,
        planning,
        typing,
        reread,
        moving,
    })
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let prove = arguments.iter().any(|value| value == "--prove");
    let sizes: Vec<usize> = arguments
        .iter()
        .filter_map(|value| value.parse().ok())
        .collect();
    let sizes = if sizes.is_empty() {
        SIZES.to_vec()
    } else {
        sizes
    };
    if prove {
        println!("--prove: the typing column is charged quadratic busywork on purpose");
    }
    println!(
        "  chars   rows      open ms      read ms      plan ms      type ms    reread ms   \
         move ms"
    );
    let mut measured = Vec::new();
    for size in sizes {
        let Some(row) = measure(size, prove) else {
            continue;
        };
        println!(
            "{:7} {:6} {:12.1} {:12.1} {:12.1} {:12.1} {:12.1} {:12.1}",
            row.characters,
            row.rows,
            row.first,
            row.reading,
            row.planning,
            row.typing,
            row.reread,
            row.moving
        );
        measured.push(row);
    }
    let shape = |pick: fn(&Row) -> f64| {
        let points: Vec<(f64, f64)> = measured
            .iter()
            .map(|row| (counted(row.characters), pick(row)))
            .collect();
        growth::exponent(&points)
            .map_or_else(|| "     ?".to_owned(), |slope| format!("{slope:6.2}"))
    };
    println!(
        "exponent       {} {} {} {} {} {}",
        shape(|row| row.first),
        shape(|row| row.reading),
        shape(|row| row.planning),
        shape(|row| row.typing),
        shape(|row| row.reread),
        shape(|row| row.moving),
    );
    let step = |pick: fn(&Row) -> f64| {
        let points: Vec<(f64, f64)> = measured
            .iter()
            .map(|row| (counted(row.characters), pick(row)))
            .collect();
        growth::steps(&points)
            .into_iter()
            .map(|slope| slope.map_or_else(|| "     ?".to_owned(), |slope| format!("{slope:6.2}")))
            .collect::<Vec<String>>()
    };
    for (index, pair) in measured.windows(2).enumerate() {
        println!(
            "{:6} to {:6} {} {} {} {} {} {}",
            pair[0].characters,
            pair[1].characters,
            step(|row| row.first)[index],
            step(|row| row.reading)[index],
            step(|row| row.planning)[index],
            step(|row| row.typing)[index],
            step(|row| row.reread)[index],
            step(|row| row.moving)[index],
        );
    }
    println!("(1.00 is linear in the characters of the block; 2.00 is quadratic)");
}
