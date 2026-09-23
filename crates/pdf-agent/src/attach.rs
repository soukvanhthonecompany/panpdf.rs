use std::fmt::Write as _;
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::image_file::ImageFile;
use pdf_session::{PageView, Session};

use crate::connect::Attachment;
use crate::desk::{MOST_CHARACTERS, picture_of, text_of_page};

pub const MOST_FILES: usize = 6;

pub const MOST_TOTAL_BYTES: usize = 20 * 1024 * 1024;

pub const LONGEST_SIDE: u32 = 1568;

pub const PAGE_DPI: f64 = 110.0;

pub const MOST_PAGE_PICTURES: usize = 4;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttachError {
    Unsupported { name: String },
    TooMany,
    TooLarge { name: String },
    Unreadable { name: String, why: String },
}

impl std::fmt::Display for AttachError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported { name } => {
                write!(
                    f,
                    "{name} cannot be read: it is not a picture, a PDF or text, and this \
                     program cannot make sense of what is inside it -- so a model could not \
                     either, over this wire"
                )
            }
            Self::TooMany => write!(
                f,
                "at most {MOST_FILES} files can be attached to a question"
            ),
            Self::TooLarge { name } => write!(f, "{name} is too large to attach"),
            Self::Unreadable { name, why } => write!(f, "{name} cannot be read: {why}"),
        }
    }
}

impl std::error::Error for AttachError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Picture,
    Pdf,
    Office,
    Text,
    Unsupported,
}

#[must_use]
pub fn kind_of(bytes: &[u8]) -> Kind {
    if picture_media_type(bytes).is_some() {
        Kind::Picture
    } else if looks_like_pdf(bytes) {
        Kind::Pdf
    } else if office_text(bytes).is_some() {
        Kind::Office
    } else if as_text(bytes).is_some() {
        Kind::Text
    } else {
        Kind::Unsupported
    }
}

pub fn fits(so_far: &[(String, usize)], more: (&str, usize)) -> Result<(), AttachError> {
    if so_far.len() >= MOST_FILES {
        return Err(AttachError::TooMany);
    }
    let total: usize = so_far.iter().map(|(_, size)| *size).sum();
    if total + more.1 > MOST_TOTAL_BYTES {
        return Err(AttachError::TooLarge {
            name: more.0.to_owned(),
        });
    }
    Ok(())
}

pub fn prepare(name: &str, bytes: &[u8]) -> Result<Vec<Attachment>, AttachError> {
    if bytes.len() > MOST_TOTAL_BYTES {
        return Err(AttachError::TooLarge {
            name: name.to_owned(),
        });
    }
    if let Some(media_type) = picture_media_type(bytes) {
        return Ok(vec![picture(name, media_type, bytes)?]);
    }
    if looks_like_pdf(bytes) {
        return from_pdf(name, bytes);
    }
    if let Some(text) = office_text(bytes) {
        return Ok(vec![Attachment::text(name, capped_text(&text))]);
    }
    if let Some(text) = as_text(bytes) {
        return Ok(vec![Attachment::text(name, capped_text(text))]);
    }
    Err(AttachError::Unsupported {
        name: name.to_owned(),
    })
}

pub fn page_picture(name: &str, view: &PageView, dpi: f64) -> Result<Attachment, AttachError> {
    let (png, _, _) = picture_of(view, dpi).map_err(|why| AttachError::Unreadable {
        name: name.to_owned(),
        why,
    })?;
    Ok(Attachment::image(name, "image/png", png))
}

fn picture_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else {
        None
    }
}

fn looks_like_pdf(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(1024)]
        .windows(4)
        .any(|window| window == b"%PDF")
}

fn picture(name: &str, media_type: &'static str, bytes: &[u8]) -> Result<Attachment, AttachError> {
    let unreadable = |why: String| AttachError::Unreadable {
        name: name.to_owned(),
        why,
    };
    let file = ImageFile::read(bytes).map_err(|error| unreadable(error.to_string()))?;
    let (across, down) = file.upright();
    if across.max(down) <= LONGEST_SIDE && file.orientation <= 1 {
        return Ok(Attachment::image(name, media_type, bytes.to_vec()));
    }
    let small = file
        .thumbnail(LONGEST_SIDE)
        .ok_or_else(|| unreadable("its pixels cannot be read".to_owned()))?;
    let png = pdf_edit::png::write((small.width, small.height), &over_white(&small.rgba), None)
        .map_err(|why| unreadable(why.to_owned()))?;
    Ok(Attachment::image(name, "image/png", png))
}

fn over_white(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len() / 4 * 3);
    for pixel in rgba.chunks_exact(4) {
        let alpha = u32::from(pixel[3]);
        for channel in &pixel[..3] {
            let over = (u32::from(*channel) * alpha + 255 * (255 - alpha)) / 255;
            out.push(u8::try_from(over.min(255)).unwrap_or(255));
        }
    }
    out
}

fn from_pdf(name: &str, bytes: &[u8]) -> Result<Vec<Attachment>, AttachError> {
    let unreadable = |why: String| AttachError::Unreadable {
        name: name.to_owned(),
        why,
    };
    let held: Arc<[u8]> = Arc::from(bytes.to_vec());
    let source = ByteStore::new(SourceId::new(0), held);
    let mut session = Session::new(source, b"");
    let pages = session
        .page_count()
        .map_err(|error| unreadable(error.to_string()))?;
    if pages == 0 {
        return Err(unreadable("it has no pages".to_owned()));
    }
    let mut words = String::new();
    let mut pictures = Vec::new();
    let mut left_out = 0_usize;
    for page in 0..pages {
        if words.chars().count() >= MOST_CHARACTERS {
            left_out = pages - page;
            break;
        }
        let Ok(view) = session.page_for_display(page) else {
            let _ = writeln!(&mut words, "\n-- page {} cannot be read --", page + 1);
            continue;
        };
        let room = MOST_CHARACTERS.saturating_sub(words.chars().count());
        let said = text_of_page(&view, page, room);
        if said.trim().is_empty() {
            if pictures.len() < MOST_PAGE_PICTURES
                && let Ok(picture) =
                    page_picture(&format!("{name} page {}", page + 1), &view, PAGE_DPI)
            {
                pictures.push(picture);
                let _ = writeln!(
                    &mut words,
                    "\n-- page {} has no text; it is attached as a picture --",
                    page + 1
                );
            } else {
                let _ = writeln!(&mut words, "\n-- page {} has no text --", page + 1);
            }
            continue;
        }
        let _ = writeln!(&mut words, "\n-- page {} --\n{said}", page + 1);
    }
    if left_out > 0 {
        let _ = writeln!(
            &mut words,
            "\n-- {left_out} more page(s) were not read: this document is longer than one \
             question can carry --"
        );
    }
    let mut out = Vec::with_capacity(pictures.len() + 1);
    if !words.trim().is_empty() {
        out.push(Attachment::text(name, words));
    }
    out.extend(pictures);
    if out.is_empty() {
        return Err(unreadable(
            "none of its pages could be read or drawn".to_owned(),
        ));
    }
    Ok(out)
}

fn as_text(bytes: &[u8]) -> Option<&str> {
    if bytes.contains(&0) {
        return None;
    }
    std::str::from_utf8(bytes).ok()
}

fn capped_text(text: &str) -> String {
    if text.chars().count() <= MOST_CHARACTERS {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(MOST_CHARACTERS).collect();
    let _ = write!(
        &mut out,
        "\n\n-- cut short at {MOST_CHARACTERS} characters: this file is longer than one \
         question can carry --"
    );
    out
}

fn office_text(bytes: &[u8]) -> Option<String> {
    let mut out = String::new();
    for entry in central_directory(bytes)?
        .into_iter()
        .filter(|entry| wanted_office_member(&entry.name))
    {
        if let Some(xml) = zip_member_text(bytes, &entry) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&strip_tags(&xml));
        }
    }
    (!out.trim().is_empty()).then_some(out)
}

struct ZipEntry {
    name: String,
    method: u16,
    compressed_size: usize,
    local_header_offset: usize,
}

fn wanted_office_member(name: &str) -> bool {
    name == "word/document.xml"
        || name == "xl/sharedStrings.xml"
        || (name.starts_with("ppt/slides/slide")
            && std::path::Path::new(name)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("xml")))
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    bytes
        .get(at..at + 2)
        .map(|two| u16::from_le_bytes([two[0], two[1]]))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    bytes
        .get(at..at + 4)
        .map(|four| u32::from_le_bytes([four[0], four[1], four[2], four[3]]))
}

fn central_directory(bytes: &[u8]) -> Option<Vec<ZipEntry>> {
    let tail_from = bytes.len().saturating_sub(22 + 65_535);
    let eocd = bytes[tail_from..]
        .windows(4)
        .rposition(|four| four == b"PK\x05\x06")?
        + tail_from;
    let count = usize::from(u16_at(bytes, eocd + 10)?);
    let mut at = usize::try_from(u32_at(bytes, eocd + 16)?).ok()?;
    let mut entries = Vec::with_capacity(count.min(MOST_FILES * 8));
    for _ in 0..count {
        if bytes.get(at..at + 4) != Some(&b"PK\x01\x02"[..]) {
            return None;
        }
        let method = u16_at(bytes, at + 10)?;
        let compressed_size = usize::try_from(u32_at(bytes, at + 20)?).ok()?;
        let name_len = usize::from(u16_at(bytes, at + 28)?);
        let extra_len = usize::from(u16_at(bytes, at + 30)?);
        let comment_len = usize::from(u16_at(bytes, at + 32)?);
        let local_header_offset = usize::try_from(u32_at(bytes, at + 42)?).ok()?;
        let name = bytes
            .get(at + 46..at + 46 + name_len)
            .and_then(|raw| std::str::from_utf8(raw).ok())?
            .to_owned();
        entries.push(ZipEntry {
            name,
            method,
            compressed_size,
            local_header_offset,
        });
        at = at + 46 + name_len + extra_len + comment_len;
    }
    Some(entries)
}

fn zip_member_text(bytes: &[u8], entry: &ZipEntry) -> Option<String> {
    let at = entry.local_header_offset;
    if bytes.get(at..at + 4) != Some(&b"PK\x03\x04"[..]) {
        return None;
    }
    let name_len = usize::from(u16_at(bytes, at + 26)?);
    let extra_len = usize::from(u16_at(bytes, at + 28)?);
    let data_at = at + 30 + name_len + extra_len;
    let data = bytes.get(data_at..data_at + entry.compressed_size)?;
    let raw = match entry.method {
        0 => data.to_vec(),
        8 => {
            let mut zlib = Vec::with_capacity(data.len() + 2);
            zlib.extend_from_slice(&[0x78, 0x9c]);
            zlib.extend_from_slice(data);
            pdf_session::inflate_zlib(&zlib, MOST_TOTAL_BYTES * 4).ok()?
        }
        _ => return None,
    };
    String::from_utf8(raw).ok()
}

fn strip_tags(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut in_tag = false;
    for ch in xml.chars() {
        match ch {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' => in_tag = false,
            _ if in_tag => {}
            _ => out.push(ch),
        }
    }
    let out = out
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests;
