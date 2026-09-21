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
                    "{name} is not a picture or a PDF, and cannot be attached"
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

#[cfg(test)]
mod tests;
