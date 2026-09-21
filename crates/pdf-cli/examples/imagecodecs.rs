use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pdf_bytes::ByteStore;
use pdf_content::{
    ImageXObject, PageContentLimits, PageResources, count_pages_with_password,
    load_page_program_with_password,
};
use pdf_syntax::{Object, ObjectKind, Reference};

const MAX_DEPTH: usize = 16;

struct Found {
    decoded: Option<Result<usize, String>>,
    page: usize,
    name: String,
    codec: String,
    width: Option<i64>,
    height: Option<i64>,
    bits: Option<i64>,
    mask: bool,
    parameters: Vec<(String, String)>,
    dictionary_transform: Option<String>,
    encoded: usize,
}

fn text(source: &ByteStore, object: &Object) -> String {
    String::from_utf8_lossy(
        source
            .as_bytes()
            .get(object.span().start()..object.span().end())
            .unwrap_or_default(),
    )
    .into_owned()
}

fn number(source: &ByteStore, object: &Object) -> Option<i64> {
    match object.kind() {
        ObjectKind::Number(_) => text(source, object).trim().parse().ok(),
        _ => None,
    }
}

fn describe(source: &ByteStore, object: &Object) -> String {
    match object.kind() {
        ObjectKind::Boolean(_) | ObjectKind::Number(_) | ObjectKind::Name | ObjectKind::Null => {
            text(source, object)
        }
        ObjectKind::Array(items) => format!("[{} items]", items.len()),
        ObjectKind::Dictionary(entries) => format!("<<{} entries>>", entries.len()),
        ObjectKind::Reference(reference) => {
            format!("{} {} R", reference.object_number(), reference.generation())
        }
        ObjectKind::LiteralString | ObjectKind::HexString => "(string)".to_owned(),
    }
}

fn entry<'a>(source: &ByteStore, object: &'a Object, key: &[u8]) -> Option<&'a Object> {
    let ObjectKind::Dictionary(entries) = object.kind() else {
        return None;
    };
    entries
        .iter()
        .find(|entry| entry.key_equals(source, key))
        .map(pdf_syntax::DictionaryEntry::value)
}

fn is_image(source: &ByteStore, object: &Object) -> bool {
    entry(source, object, b"/Subtype").is_some_and(|value| value.name_equals(source, b"/Image"))
}

fn is_form(source: &ByteStore, object: &Object) -> bool {
    entry(source, object, b"/Subtype").is_some_and(|value| value.name_equals(source, b"/Form"))
}

fn parameters(image: &ImageXObject) -> Vec<(String, String)> {
    let Some(object) = image.codec_parameters.as_ref() else {
        return Vec::new();
    };
    let source = &image.source;
    let ObjectKind::Dictionary(entries) = object.kind() else {
        return vec![("(parms)".to_owned(), describe(source, object))];
    };
    entries
        .iter()
        .map(|entry| {
            let key = String::from_utf8_lossy(
                source
                    .as_bytes()
                    .get(entry.key().span().start()..entry.key().span().end())
                    .unwrap_or_default(),
            )
            .into_owned();
            (key, describe(source, entry.value()))
        })
        .collect()
}

fn ccitt_of(image: &ImageXObject) -> Result<usize, String> {
    let source = &image.source;
    let mut held = pdf_paint::ccitt::CcittParameters::default();
    if let Some(object) = image.codec_parameters.as_ref()
        && let ObjectKind::Dictionary(entries) = object.kind()
    {
        for entry in entries {
            let value = entry.value();
            let integer = || number(source, value).unwrap_or(0);
            let flag = matches!(value.kind(), ObjectKind::Boolean(true));
            if entry.key_equals(source, b"/K") {
                held.k = integer();
            } else if entry.key_equals(source, b"/Columns") {
                held.columns = u32::try_from(integer()).unwrap_or(0);
            } else if entry.key_equals(source, b"/Rows") {
                held.rows = u32::try_from(integer()).unwrap_or(0);
            } else if entry.key_equals(source, b"/BlackIs1") {
                held.black_is_1 = flag;
            } else if entry.key_equals(source, b"/EncodedByteAlign") {
                held.byte_align = flag;
            } else if entry.key_equals(source, b"/EndOfLine") {
                held.end_of_line = flag;
            }
        }
    }
    let width = entry(source, &image.dictionary, b"/Width")
        .and_then(|value| number(source, value))
        .unwrap_or(0);
    let height = entry(source, &image.dictionary, b"/Height")
        .and_then(|value| number(source, value))
        .unwrap_or(0);
    let (Ok(width), Ok(height)) = (u32::try_from(width), u32::try_from(height)) else {
        return Err("the image declares no usable size".to_owned());
    };
    pdf_paint::ccitt::decode(&image.bytes, &held, width, height, 1 << 28)
        .map(|samples| samples.len())
        .map_err(|error| error.to_string())
}

fn walk(
    resources: &PageResources,
    page: usize,
    depth: usize,
    decode: bool,
    seen: &mut Vec<Reference>,
    into: &mut Vec<Found>,
) {
    if depth > MAX_DEPTH {
        return;
    }
    for resource in resources.xobjects() {
        let Some(reference) = resource.reference() else {
            continue;
        };
        if seen.contains(&reference) {
            continue;
        }
        let source = resource.source();
        if is_image(source, resource.value()) {
            seen.push(reference);
            let Ok(image) = resource.image(reference, 64 * 1024 * 1024) else {
                continue;
            };
            let is_ccitt = image.codec == Some(pdf_syntax::ImageCodec::CcittFax);
            let decoded = (decode && is_ccitt).then(|| ccitt_of(&image));
            into.push(Found {
                decoded,
                page,
                name: String::from_utf8_lossy(resource.name()).into_owned(),
                codec: image
                    .codec
                    .map_or("(none)", pdf_syntax::ImageCodec::name)
                    .to_owned(),
                width: entry(&image.source, &image.dictionary, b"/Width")
                    .and_then(|value| number(&image.source, value)),
                height: entry(&image.source, &image.dictionary, b"/Height")
                    .and_then(|value| number(&image.source, value)),
                bits: entry(&image.source, &image.dictionary, b"/BitsPerComponent")
                    .and_then(|value| number(&image.source, value)),
                mask: entry(&image.source, &image.dictionary, b"/ImageMask")
                    .is_some_and(|value| matches!(value.kind(), ObjectKind::Boolean(true))),
                parameters: parameters(&image),
                dictionary_transform: entry(&image.source, &image.dictionary, b"/ColorTransform")
                    .map(|value| describe(&image.source, value)),
                encoded: image.bytes.len(),
            });
        } else if is_form(source, resource.value())
            && let Some(form) = resource.form()
            && let Some(inner) = form.resources.as_ref()
        {
            seen.push(reference);
            walk(inner, page, depth + 1, decode, seen, into);
        }
    }
}

fn images_of(path: &Path, pages: &[usize], decode: bool) -> Vec<Found> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let source = ByteStore::new(pdf_bytes::SourceId::new(1), Arc::<[u8]>::from(bytes));
    let limits = PageContentLimits::default();
    let mut found = Vec::new();
    for page in pages {
        let Ok(program) = load_page_program_with_password(&source, *page, limits, b"") else {
            continue;
        };
        let mut seen = Vec::new();
        walk(
            &program.resources,
            page + 1,
            0,
            decode,
            &mut seen,
            &mut found,
        );
    }
    found
}

fn pages_of(path: &Path, all: bool, one: Option<usize>) -> Vec<usize> {
    if let Some(page) = one {
        return vec![page - 1];
    }
    if !all {
        return vec![0];
    }
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let source = ByteStore::new(pdf_bytes::SourceId::new(1), Arc::<[u8]>::from(bytes));
    match count_pages_with_password(&source, PageContentLimits::default(), b"") {
        Ok(count) => (0..count).collect(),
        Err(_) => vec![0],
    }
}

struct Options {
    target: PathBuf,
    all: bool,
    summary: bool,
    decode: bool,
    page: Option<usize>,
}

fn options() -> Options {
    let mut arguments = std::env::args().skip(1);
    let Some(target) = arguments.next().map(PathBuf::from) else {
        eprintln!("usage: imagecodecs <file.pdf|directory> [--page N] [--all] [--summary]");
        std::process::exit(2);
    };
    let mut held = Options {
        target,
        all: false,
        summary: false,
        decode: false,
        page: None,
    };
    let rest: Vec<String> = arguments.collect();
    let mut index = 0;
    while index < rest.len() {
        match rest[index].as_str() {
            "--all" => held.all = true,
            "--summary" => held.summary = true,
            "--decode" => held.decode = true,
            "--page" => {
                index += 1;
                held.page = rest.get(index).and_then(|value| value.parse().ok());
            }
            other => eprintln!("ignoring {other}"),
        }
        index += 1;
    }
    held
}

#[derive(Default)]
struct Census {
    counts: BTreeMap<String, (usize, usize)>,
    shapes: BTreeMap<String, usize>,
    transforms: BTreeMap<String, (usize, usize)>,
    decoded: (usize, usize),
    refusals: BTreeMap<String, usize>,
    refusing_files: BTreeMap<String, usize>,
}

fn count_transforms(image: &Found, into: &mut BTreeMap<String, usize>) {
    if let Some(value) = image.dictionary_transform.as_ref() {
        *into
            .entry(format!("image dictionary /ColorTransform {value}"))
            .or_default() += 1;
    }
    for (key, value) in &image.parameters {
        if key == "/ColorTransform" {
            *into
                .entry(format!("/DecodeParms /ColorTransform {value}"))
                .or_default() += 1;
        }
    }
}

fn report(census: &Census, files: usize, decode: bool) {
    println!("\n{files} files");
    for (codec, (images, count)) in &census.counts {
        println!("  {codec:20} {images:6} images in {count} files");
    }
    if decode {
        println!(
            "\nCCITT decoding: {} decoded, {} refused, over {} files that refused any",
            census.decoded.0,
            census.decoded.1,
            census.refusing_files.len()
        );
        for (reason, count) in &census.refusals {
            println!("  {count:5} x {reason}");
        }
    }
    if !census.transforms.is_empty() {
        println!("\n/ColorTransform, by where it is written:");
        for (place, (images, count)) in &census.transforms {
            println!("  {images:5} images in {count} files -- {place}");
        }
    }
    if !census.shapes.is_empty() {
        println!("\n/CCITTFaxDecode parameter shapes:");
        for (shape, count) in &census.shapes {
            println!("  {count:5} x {shape}");
        }
    }
}

fn main() {
    let Options {
        target,
        all,
        summary,
        decode,
        page,
    } = options();

    let paths: Vec<PathBuf> = if target.is_dir() {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&target)
            .expect("the directory reads")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|kind| kind == "pdf"))
            .collect();
        paths.sort();
        paths
    } else {
        vec![target]
    };

    let mut census = Census::default();
    for path in &paths {
        let found = images_of(path, &pages_of(path, all, page), decode);
        let mut here: BTreeMap<String, usize> = BTreeMap::new();
        let mut transforms_here: BTreeMap<String, usize> = BTreeMap::new();
        for image in &found {
            *here.entry(image.codec.clone()).or_default() += 1;
            count_transforms(image, &mut transforms_here);
            if image.codec == "/CCITTFaxDecode" {
                let shape = image
                    .parameters
                    .iter()
                    .map(|(key, value)| format!("{key} {value}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                *census
                    .shapes
                    .entry(if shape.is_empty() {
                        "(no /DecodeParms)".to_owned()
                    } else {
                        shape
                    })
                    .or_default() += 1;
            }
            match &image.decoded {
                Some(Ok(_)) => census.decoded.0 += 1,
                Some(Err(reason)) => {
                    census.decoded.1 += 1;
                    let kind: String = reason
                        .split_whitespace()
                        .filter(|word| !word.chars().next().is_some_and(char::is_numeric))
                        .collect::<Vec<_>>()
                        .join(" ");
                    *census.refusals.entry(kind).or_default() += 1;
                    *census
                        .refusing_files
                        .entry(path.display().to_string())
                        .or_default() += 1;
                    if !summary {
                        println!(
                            "  REFUSED {}: page {} {} -- {reason}",
                            path.display(),
                            image.page,
                            image.name
                        );
                    }
                }
                None => {}
            }
            if !summary {
                println!(
                    "{}: page {} {} {} {}x{} bpc {} mask {} -- {} encoded bytes",
                    path.display(),
                    image.page,
                    image.name,
                    image.codec,
                    image.width.unwrap_or(-1),
                    image.height.unwrap_or(-1),
                    image.bits.unwrap_or(-1),
                    image.mask,
                    image.encoded,
                );
                for (key, value) in &image.parameters {
                    println!("    {key} {value}");
                }
            }
        }
        for (codec, count) in here {
            let entry = census.counts.entry(codec).or_default();
            entry.0 += count;
            entry.1 += 1;
        }
        for (place, count) in transforms_here {
            let entry = census.transforms.entry(place).or_default();
            entry.0 += count;
            entry.1 += 1;
        }
    }

    report(&census, paths.len(), decode);
}
