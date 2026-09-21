use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pdf_bytes::ByteStore;
use pdf_content::{
    ContentLimits, Font, PageContentLimits, PageResources, ResourceEntry,
    count_pages_with_password, load_page_program_with_password, parse_operations_strict,
};
use pdf_syntax::{Object, ObjectKind, Reference, decode_name, decode_string};

const MAX_DEPTH: usize = 16;

struct Found {
    page: usize,
    name: String,
    base_font: String,
    encoding: String,
    descendant_subtype: String,
    system_info: String,
    cid_to_gid: String,
    program: String,
    to_unicode: bool,
    parse: Result<(), String>,
}

impl Found {
    fn shape(&self) -> String {
        format!(
            "{} | descendant {} | {} | /CIDToGIDMap {} | {} | /ToUnicode {}",
            self.encoding,
            self.descendant_subtype,
            self.system_info,
            self.cid_to_gid,
            self.program,
            if self.to_unicode { "yes" } else { "no" },
        )
    }
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

fn entry<'a>(source: &ByteStore, object: &'a Object, key: &[u8]) -> Option<&'a Object> {
    let ObjectKind::Dictionary(entries) = object.kind() else {
        return None;
    };
    entries
        .iter()
        .find(|entry| entry.key_equals(source, key))
        .map(pdf_syntax::DictionaryEntry::value)
}

fn follow(
    resource: &ResourceEntry,
    source: &ByteStore,
    object: &Object,
) -> Option<(ByteStore, Object)> {
    let ObjectKind::Reference(reference) = object.kind() else {
        return Some((source.clone(), object.clone()));
    };
    resource.resolve_object(*reference).ok()
}

fn system_info(resource: &ResourceEntry, source: &ByteStore, descendant: &Object) -> String {
    let Some(value) = entry(source, descendant, b"/CIDSystemInfo") else {
        return "(no /CIDSystemInfo)".to_owned();
    };
    let Some((info_source, info)) = follow(resource, source, value) else {
        return "(unresolved /CIDSystemInfo)".to_owned();
    };
    let field = |key: &[u8]| {
        entry(&info_source, &info, key).map_or_else(
            || "?".to_owned(),
            |value| text(&info_source, value).replace(['\r', '\n'], " "),
        )
    };
    format!(
        "{} {} {}",
        field(b"/Registry"),
        field(b"/Ordering"),
        field(b"/Supplement")
    )
}

fn program_of(resource: &ResourceEntry, source: &ByteStore, descendant: &Object) -> String {
    let Some(value) = entry(source, descendant, b"/FontDescriptor") else {
        return "(no descriptor)".to_owned();
    };
    let Some((descriptor_source, descriptor)) = follow(resource, source, value) else {
        return "(unresolved descriptor)".to_owned();
    };
    for key in [&b"/FontFile"[..], b"/FontFile2", b"/FontFile3"] {
        if let Some(program) = entry(&descriptor_source, &descriptor, key) {
            let subtype = follow(resource, &descriptor_source, program)
                .and_then(|(stream_source, stream)| {
                    entry(&stream_source, &stream, b"/Subtype")
                        .map(|value| text(&stream_source, value))
                })
                .unwrap_or_default();
            let key = String::from_utf8_lossy(key);
            return if subtype.is_empty() {
                key.into_owned()
            } else {
                format!("{key} {subtype}")
            };
        }
    }
    "(no embedded program)".to_owned()
}

fn composite(resource: &ResourceEntry, page: usize) -> Option<Found> {
    let source = resource.source();
    let value = resource.value();
    let subtype = entry(source, value, b"/Subtype")?;
    if !subtype.name_equals(source, b"/Type0") {
        return None;
    }
    let encoding = entry(source, value, b"/Encoding").map_or_else(
        || "(no /Encoding)".to_owned(),
        |object| match object.kind() {
            ObjectKind::Name => text(source, object),
            ObjectKind::Reference(_) => follow(resource, source, object).map_or_else(
                || "(unresolved stream)".to_owned(),
                |(stream_source, stream)| {
                    let name = entry(&stream_source, &stream, b"/CMapName")
                        .map(|value| text(&stream_source, value));
                    let used = entry(&stream_source, &stream, b"/UseCMap")
                        .map(|value| text(&stream_source, value));
                    match (name, used) {
                        (Some(name), Some(used)) => {
                            format!("(stream {name} usecmap {used})")
                        }
                        (Some(name), None) => format!("(stream {name})"),
                        (None, _) => "(stream)".to_owned(),
                    }
                },
            ),
            _ => "(malformed /Encoding)".to_owned(),
        },
    );
    let base_font = entry(source, value, b"/BaseFont")
        .map_or_else(|| "(none)".to_owned(), |object| text(source, object));
    let descendants = entry(source, value, b"/DescendantFonts")
        .and_then(|object| follow(resource, source, object));
    let descendant = descendants.and_then(|(array_source, array)| {
        let ObjectKind::Array(items) = array.kind() else {
            return None;
        };
        follow(resource, &array_source, items.first()?)
    });
    let (descendant_subtype, system, cid_to_gid, program) = descendant.map_or_else(
        || {
            (
                "(no descendant)".to_owned(),
                String::new(),
                String::new(),
                String::new(),
            )
        },
        |(descendant_source, descendant)| {
            (
                entry(&descendant_source, &descendant, b"/Subtype")
                    .map_or_else(|| "(none)".to_owned(), |v| text(&descendant_source, v)),
                system_info(resource, &descendant_source, &descendant),
                entry(&descendant_source, &descendant, b"/CIDToGIDMap").map_or_else(
                    || "(absent)".to_owned(),
                    |v| match v.kind() {
                        ObjectKind::Name => text(&descendant_source, v),
                        _ => "(stream)".to_owned(),
                    },
                ),
                program_of(resource, &descendant_source, &descendant),
            )
        },
    );
    Some(Found {
        page,
        name: String::from_utf8_lossy(resource.name()).into_owned(),
        base_font,
        encoding,
        descendant_subtype,
        system_info: system,
        cid_to_gid,
        program,
        to_unicode: entry(source, value, b"/ToUnicode").is_some(),
        parse: resource
            .font()
            .map(|_| ())
            .map_err(|error| error.to_string()),
    })
}

fn decode_page(
    resources: &PageResources,
    streams: &[pdf_content::DecodedContentStream],
    into: &mut Decoded,
) {
    let bytes: Vec<ByteStore> = streams.iter().map(|stream| stream.bytes.clone()).collect();
    let mut refused_here = false;
    decode_content(
        resources,
        &bytes,
        0,
        &mut Vec::new(),
        into,
        &mut refused_here,
    );
    if refused_here {
        into.pages_refused += 1;
    }
}

fn decode_content(
    resources: &PageResources,
    streams: &[ByteStore],
    depth: usize,
    seen: &mut Vec<Reference>,
    into: &mut Decoded,
    refused_here: &mut bool,
) {
    if depth > MAX_DEPTH {
        return;
    }
    let mut selected: Option<Font> = None;
    for stream in streams {
        let Ok(operations) = parse_operations_strict(stream, ContentLimits::default()) else {
            continue;
        };
        for operation in &operations {
            let operator = stream
                .as_bytes()
                .get(operation.operator_span().start()..operation.operator_span().end())
                .unwrap_or_default();
            let operand_name = |index: usize| -> Option<Vec<u8>> {
                decode_name(stream, operation.operands().get(index)?).ok()
            };
            if operator == b"Tf" {
                selected = operand_name(0)
                    .and_then(|name| resources.font(&name))
                    .and_then(|resource| resource.font().ok());
                continue;
            }
            if operator == b"Do" {
                let Some(resource) = operand_name(0).and_then(|name| resources.xobject(&name))
                else {
                    continue;
                };
                let Some(reference) = resource.reference() else {
                    continue;
                };
                if seen.contains(&reference) {
                    continue;
                }
                seen.push(reference);
                if let Some(form) = resource.form() {
                    let inner = form.resources.as_ref().unwrap_or(resources);
                    decode_content(
                        inner,
                        std::slice::from_ref(&form.bytes),
                        depth + 1,
                        seen,
                        into,
                        refused_here,
                    );
                }
                continue;
            }
            let shown: Vec<&Object> = match operator {
                b"Tj" | b"'" | b"\"" => operation.operands().last().into_iter().collect(),
                b"TJ" => match operation.operands().first().map(Object::kind) {
                    Some(ObjectKind::Array(items)) => items.iter().collect(),
                    _ => Vec::new(),
                },
                _ => continue,
            };
            let Some(Font::Composite(font)) = selected.as_ref() else {
                continue;
            };
            for item in shown {
                if !matches!(
                    item.kind(),
                    ObjectKind::LiteralString | ObjectKind::HexString
                ) {
                    continue;
                }
                let Ok(bytes) = decode_string(stream, item, 1 << 20) else {
                    continue;
                };
                into.strings += 1;
                match font.source_codes(&bytes) {
                    Ok(codes) => {
                        into.codes += codes.len();
                        into.mapped += codes
                            .iter()
                            .filter(|code| code.cid.is_some_and(|cid| cid != 0))
                            .count();
                    }
                    Err(error) => {
                        let shape = format!(
                            "{error} -- {} bytes: {}",
                            bytes.len(),
                            bytes
                                .iter()
                                .take(24)
                                .fold(String::new(), |mut spelled, byte| {
                                    use std::fmt::Write;
                                    let _ = write!(spelled, "{byte:02x}");
                                    spelled
                                },)
                        );
                        *into.refusals.entry(shape).or_default() += 1;
                        *refused_here = true;
                    }
                }
            }
        }
    }
}

fn walk(
    resources: &PageResources,
    page: usize,
    depth: usize,
    seen: &mut Vec<Reference>,
    into: &mut Vec<Found>,
) {
    if depth > MAX_DEPTH {
        return;
    }
    for resource in resources.fonts() {
        if let Some(reference) = resource.reference() {
            if seen.contains(&reference) {
                continue;
            }
            seen.push(reference);
        }
        if let Some(found) = composite(resource, page) {
            into.push(found);
        }
    }
    for resource in resources.xobjects() {
        let Some(reference) = resource.reference() else {
            continue;
        };
        if seen.contains(&reference) {
            continue;
        }
        let source = resource.source();
        if entry(source, resource.value(), b"/Subtype")
            .is_some_and(|value| value.name_equals(source, b"/Form"))
            && let Some(form) = resource.form()
            && let Some(inner) = form.resources.as_ref()
        {
            seen.push(reference);
            walk(inner, page, depth + 1, seen, into);
        }
    }
}

fn fonts_of(path: &Path, pages: &[usize], decoded: Option<&mut Decoded>) -> Vec<Found> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let source = ByteStore::new(pdf_bytes::SourceId::new(1), Arc::<[u8]>::from(bytes));
    let limits = PageContentLimits::default();
    let mut found = Vec::new();
    let mut decoded = decoded;
    for page in pages {
        let Ok(program) = load_page_program_with_password(&source, *page, limits, b"") else {
            continue;
        };
        let mut seen = Vec::new();
        walk(&program.resources, page + 1, 0, &mut seen, &mut found);
        if let Some(into) = decoded.as_deref_mut() {
            decode_page(&program.resources, &program.streams, into);
        }
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
        eprintln!("usage: cmaps <file.pdf|directory> [--page N] [--all] [--summary] [--decode]");
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
struct Decoded {
    strings: usize,
    codes: usize,
    mapped: usize,
    refusals: BTreeMap<String, usize>,
    pages_refused: usize,
}

#[derive(Default)]
struct Census {
    encodings: BTreeMap<String, (usize, usize)>,
    shapes: BTreeMap<String, usize>,
    refusals: BTreeMap<String, usize>,
    decoded: Decoded,
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
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
            })
            .collect();
        paths.sort();
        paths
    } else {
        vec![target]
    };

    let mut census = Census::default();
    let mut files_with_composites = 0;
    for path in &paths {
        let pages = pages_of(path, all, page);
        let found = fonts_of(path, &pages, decode.then_some(&mut census.decoded));
        if found.is_empty() {
            continue;
        }
        files_with_composites += 1;
        let mut file_encodings: BTreeMap<String, usize> = BTreeMap::new();
        for item in &found {
            *file_encodings.entry(item.encoding.clone()).or_default() += 1;
            *census.shapes.entry(item.shape()).or_default() += 1;
            if let Err(message) = &item.parse {
                *census.refusals.entry(message.clone()).or_default() += 1;
            }
        }
        for (encoding, count) in file_encodings {
            let slot = census.encodings.entry(encoding).or_default();
            slot.0 += count;
            slot.1 += 1;
        }
        if !summary {
            println!("\n{}", path.display());
            for item in &found {
                println!(
                    "  page {:>4} {:<12} {:<40} {}",
                    item.page, item.name, item.base_font, item.encoding
                );
                println!(
                    "                          descendant {} | {} | /CIDToGIDMap {} | {} | /ToUnicode {}",
                    item.descendant_subtype,
                    item.system_info,
                    item.cid_to_gid,
                    item.program,
                    if item.to_unicode { "yes" } else { "no" }
                );
                if let Err(message) = &item.parse {
                    println!("                          refused: {message}");
                }
            }
        }
    }

    println!(
        "\n{} files, {} with composite fonts",
        paths.len(),
        files_with_composites
    );
    println!("\ncomposite /Encoding, as written:");
    for (encoding, (uses, files)) in &census.encodings {
        println!("  {uses:6} uses in {files:3} files -- {encoding}");
    }
    println!("\nfull shapes:");
    for (shape, count) in &census.shapes {
        println!("  {count:6} x {shape}");
    }
    if !census.refusals.is_empty() {
        println!("\nfont() refusals:");
        for (message, count) in &census.refusals {
            println!("  {count:6} x {message}");
        }
    }
    if decode {
        let decoded = &census.decoded;
        println!(
            "\ndecoding composite show strings: {} strings, {} codes, {} selected a CID",
            decoded.strings, decoded.codes, decoded.mapped
        );
        println!(
            "  pages with at least one refusal: {}",
            decoded.pages_refused
        );
        for (message, count) in &decoded.refusals {
            println!("  {count:6} x {message}");
        }
    }
}
