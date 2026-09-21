use std::error::Error;
use std::path::PathBuf;

use pdf_bytes::{ByteStore, SourceId};
use pdf_content::{PageContentLimits, load_page_program_with_password};
use pdf_syntax::{Object, ObjectKind};

fn is_image(source: &ByteStore, value: &Object) -> bool {
    let ObjectKind::Dictionary(entries) = value.kind() else {
        return false;
    };
    entries.iter().any(|entry| {
        entry.key_equals(source, b"/Subtype")
            && matches!(entry.value().kind(), ObjectKind::Name)
            && entry.value().name_equals(source, b"/Image")
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    let path = PathBuf::from(
        arguments
            .next()
            .ok_or("usage: dctdump <file.pdf> <directory>")?,
    );
    let into = PathBuf::from(
        arguments
            .next()
            .ok_or("usage: dctdump <file.pdf> <directory> [--page N]")?,
    );
    let mut page_index = 0usize;
    while let Some(argument) = arguments.next() {
        if argument == "--page" {
            let value = arguments
                .next()
                .ok_or("--page needs a page number")?
                .parse::<usize>()?;
            page_index = value.checked_sub(1).ok_or("--page counts from 1")?;
        } else {
            return Err("usage: dctdump <file.pdf> <directory> [--page N]".into());
        }
    }
    std::fs::create_dir_all(&into)?;

    let source = ByteStore::new(SourceId::new(0), std::fs::read(&path)?);
    let program =
        load_page_program_with_password(&source, page_index, PageContentLimits::default(), b"")?;
    let resources = &program.resources;
    let mut written = 0usize;
    for resource in resources.xobjects() {
        let Some(reference) = resource.reference() else {
            continue;
        };
        if !is_image(resource.source(), resource.value()) {
            continue;
        }
        let Ok(image) = resource.image(reference, 256 * 1024 * 1024) else {
            continue;
        };
        if image.codec != Some(pdf_syntax::ImageCodec::Dct) {
            continue;
        }
        let name = format!(
            "object-{}-{}.jpg",
            reference.object_number(),
            reference.generation()
        );
        std::fs::write(into.join(&name), image.bytes.as_ref())?;
        println!("{name}: {} encoded bytes", image.bytes.len());
        written += 1;
    }
    println!("wrote {written} DCT streams to {}", into.display());
    Ok(())
}
