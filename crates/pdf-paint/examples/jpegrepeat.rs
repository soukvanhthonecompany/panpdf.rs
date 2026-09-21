use std::io::Cursor;
use std::sync::Arc;

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

fn adobe(bytes: &[u8]) -> bool {
    bytes.windows(5).any(|window| window == b"Adobe")
}

fn decode(encoded: &[u8], raw: bool) -> Arc<[u8]> {
    let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(encoded));
    decoder.set_max_decoding_buffer_size(1 << 30);
    let samples = decoder.decode().expect("a decoded JPEG");
    if raw {
        return Arc::from(samples);
    }
    let inverted = if adobe(encoded) {
        samples.iter().map(|byte| 255 - byte).collect::<Vec<u8>>()
    } else {
        samples
    };
    Arc::<[u8]>::from(inverted)
}

fn differing_offsets(one: &[u8], other: &[u8]) -> Vec<usize> {
    (0..one.len().max(other.len()))
        .filter(|&index| one.get(index) != other.get(index))
        .collect()
}

fn calibrate() {
    let known = [10u8, 20, 30, 40];
    assert!(differing_offsets(&known, &known).is_empty());
    for component in 0..4 {
        let mut changed = known;
        changed[component] += 1;
        assert_eq!(differing_offsets(&known, &changed), vec![component]);
    }
    assert_eq!(differing_offsets(&known, &known[..3]), vec![3]);
    assert_eq!(differing_offsets(&known[..3], &known), vec![3]);
}

fn main() {
    calibrate();
    let evidence = std::env::var_os("PANPDF_JPEG_EVIDENCE").map(std::path::PathBuf::from);
    if let Some(path) = &evidence {
        std::fs::create_dir_all(path).expect("evidence directory");
    }
    let mut paths = Vec::new();
    let mut readings = 20usize;
    let mut raw = false;
    for argument in std::env::args().skip(1) {
        if argument == "--raw" {
            raw = true;
        } else if let Ok(count) = argument.parse::<usize>() {
            readings = count;
        } else {
            let path = std::path::PathBuf::from(&argument);
            if path.is_dir() {
                let mut found: Vec<_> = std::fs::read_dir(&path)
                    .expect("the directory")
                    .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                    .filter(|path| path.extension().is_some_and(|kind| kind == "jpg"))
                    .collect();
                found.sort();
                paths.extend(found);
            } else {
                paths.push(path);
            }
        }
    }
    assert!(
        !paths.is_empty(),
        "usage: jpegrepeat <file.jpg|directory> [readings] [--raw]"
    );

    repeat(&paths, readings, raw, evidence.as_deref());
}

fn repeat(
    paths: &[std::path::PathBuf],
    readings: usize,
    raw: bool,
    evidence: Option<&std::path::Path>,
) {
    let encoded: Vec<Vec<u8>> = paths
        .iter()
        .map(|path| std::fs::read(path).expect("a JPEG"))
        .collect();
    let encoded_hashes: Vec<u64> = encoded.iter().map(|bytes| fnv1a(bytes)).collect();
    let first: Vec<Arc<[u8]>> = encoded.iter().map(|bytes| decode(bytes, raw)).collect();
    for (path, samples) in paths.iter().zip(first.iter()) {
        println!(
            "{}: {} samples {:016x}",
            path.display(),
            samples.len(),
            fnv1a(samples)
        );
    }

    let first_hashes: Vec<u64> = first.iter().map(|bytes| fnv1a(bytes)).collect();
    let mut differences = 0usize;
    for reading in 2..=readings {
        for (at, bytes) in encoded.iter().enumerate() {
            assert_eq!(
                fnv1a(bytes),
                encoded_hashes[at],
                "the encoded bytes themselves changed"
            );
        }
        for (at, bytes) in first.iter().enumerate() {
            assert_eq!(
                fnv1a(bytes),
                first_hashes[at],
                "retained reference changed before decode"
            );
        }
        let again: Vec<Arc<[u8]>> = encoded.iter().map(|bytes| decode(bytes, raw)).collect();
        for (at, bytes) in encoded.iter().enumerate() {
            assert_eq!(
                fnv1a(bytes),
                encoded_hashes[at],
                "encoded bytes changed during decode"
            );
            assert_eq!(
                fnv1a(&first[at]),
                first_hashes[at],
                "retained reference changed during decode"
            );
        }
        for (at, (one, other)) in first.iter().zip(again.iter()).enumerate() {
            if one == other {
                continue;
            }
            differences += 1;
            let offsets = differing_offsets(one, other);
            if let Some(path) = &evidence {
                let prefix = format!("reading-{reading}-image-{at}");
                std::fs::write(path.join(format!("{prefix}-reference.bin")), one)
                    .expect("save reference");
                std::fs::write(path.join(format!("{prefix}-actual.bin")), other)
                    .expect("save actual");
            }
            println!(
                "reading {reading}: {} differs at {} offsets, reference length {}",
                paths[at].display(),
                offsets.len(),
                one.len()
            );
            for index in offsets.iter().take(8) {
                println!(
                    "  sample {index} (component {}): {:#04x} against {:#04x}",
                    index % 4,
                    one.get(*index).copied().unwrap_or(0),
                    other.get(*index).copied().unwrap_or(0)
                );
            }
        }
    }
    println!("readings: {readings}, differences: {differences}");
    std::process::exit(i32::from(differences != 0));
}
