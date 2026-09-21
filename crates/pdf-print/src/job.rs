use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::layout::Sheet;
use crate::service::{self, JobSettings};
use crate::sheet::SheetImage;
#[cfg(not(windows))]
use crate::sheet::draw_sheet;

pub struct SheetsPdf {
    file: BufWriter<std::fs::File>,
    written: u64,
    offsets: Vec<u64>,
    pages: Vec<usize>,
}

impl SheetsPdf {
    pub fn create(path: &Path) -> std::io::Result<Self> {
        let mut pdf = Self {
            file: BufWriter::new(std::fs::File::create(path)?),
            written: 0,
            offsets: vec![0, 0],
            pages: Vec::new(),
        };
        pdf.put(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")?;
        Ok(pdf)
    }

    fn put(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.file.write_all(bytes)?;
        self.written += u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        Ok(())
    }

    fn object(&mut self, number: usize, body: &[u8]) -> std::io::Result<()> {
        if self.offsets.len() < number {
            self.offsets.resize(number, 0);
        }
        self.offsets[number - 1] = self.written;
        self.put(format!("{number} 0 obj\n").as_bytes())?;
        self.put(body)?;
        self.put(b"\nendobj\n")
    }

    pub fn add(&mut self, size: [f64; 2], image: &SheetImage) -> std::io::Result<()> {
        let number = self.offsets.len() + 1;
        let grey = image
            .rgb
            .chunks_exact(3)
            .all(|pixel| pixel[0] == pixel[1] && pixel[1] == pixel[2]);
        let (space, samples) = if grey {
            (
                "/DeviceGray",
                image
                    .rgb
                    .chunks_exact(3)
                    .map(|pixel| pixel[0])
                    .collect::<Vec<u8>>(),
            )
        } else {
            ("/DeviceRGB", image.rgb.clone())
        };
        let packed = pdf_syntax::deflate_zlib(&samples);
        let mut picture = format!(
            "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace {space} \
             /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
            image.width,
            image.height,
            packed.len()
        )
        .into_bytes();
        picture.extend_from_slice(&packed);
        picture.extend_from_slice(b"\nendstream");
        self.object(number, &picture)?;
        let [width, height] = size;
        let draw = format!("q {width:.4} 0 0 {height:.4} 0 0 cm /Sheet Do Q");
        let mut content = format!("<< /Length {} >>\nstream\n", draw.len()).into_bytes();
        content.extend_from_slice(draw.as_bytes());
        content.extend_from_slice(b"\nendstream");
        self.object(number + 1, &content)?;
        let page = format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width:.4} {height:.4}] \
             /Resources << /XObject << /Sheet {number} 0 R >> >> /Contents {} 0 R >>",
            number + 1
        );
        self.object(number + 2, page.as_bytes())?;
        self.pages.push(number + 2);
        Ok(())
    }

    pub fn finish(mut self) -> std::io::Result<()> {
        self.object(1, b"<< /Type /Catalog /Pages 2 0 R >>")?;
        let kids: Vec<String> = self
            .pages
            .iter()
            .map(|page| format!("{page} 0 R"))
            .collect();
        let tree = format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            self.pages.len()
        );
        self.object(2, tree.as_bytes())?;
        let table = self.written;
        let size = self.offsets.len() + 1;
        let mut xref = format!("xref\n0 {size}\n0000000000 65535 f \n").into_bytes();
        for offset in &self.offsets.clone() {
            xref.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        xref.extend_from_slice(
            format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{table}\n%%EOF\n")
                .as_bytes(),
        );
        self.put(&xref)?;
        self.file.flush()
    }
}

pub(crate) struct PrivateDir {
    pub(crate) path: PathBuf,
}

impl PrivateDir {
    pub(crate) fn new() -> std::io::Result<Self> {
        static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "panpdf-print-{}-{}-{nanos}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::Relaxed)
        ));
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(&path)?;
        Ok(Self { path })
    }
}

impl Drop for PrivateDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[derive(Debug)]
pub enum JobError {
    Sheet(crate::SheetError),
    Io(std::io::Error),
    Server(service::ServiceError),
    Stopped,
}

impl std::fmt::Display for JobError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sheet(error) => error.fmt(formatter),
            Self::Io(error) => write!(formatter, "the job could not be written: {error}"),
            Self::Server(error) => error.fmt(formatter),
            Self::Stopped => formatter.write_str("stopped"),
        }
    }
}

impl std::error::Error for JobError {}

#[cfg(not(windows))]
fn print_sheets_cups(
    sheets: &[Sheet],
    (dpi, borders): (f64, bool),
    mut draw: impl FnMut(usize, f64, [u32; 4]) -> Result<pdf_render::Canvas, String>,
    (printer, settings): (&str, &JobSettings),
    (mut drawn, stop): (impl FnMut(usize), &AtomicBool),
) -> Result<i32, JobError> {
    let directory = PrivateDir::new().map_err(JobError::Io)?;
    let path = directory.path.join("sheets.pdf");
    let mut pdf = SheetsPdf::create(&path).map_err(JobError::Io)?;
    for (at, sheet) in sheets.iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            return Err(JobError::Stopped);
        }
        let image = draw_sheet(sheet, dpi, borders, &mut draw).map_err(JobError::Sheet)?;
        pdf.add(sheet.size, &image).map_err(JobError::Io)?;
        drawn(at + 1);
    }
    pdf.finish().map_err(JobError::Io)?;
    if stop.load(Ordering::Relaxed) {
        return Err(JobError::Stopped);
    }
    crate::cups::print_file(printer, &path, settings)
        .map_err(|error| JobError::Server(service::ServiceError::Cups(error)))
}

pub fn print_sheets(
    sheets: &[Sheet],
    drawing: (f64, bool),
    draw: impl FnMut(usize, f64, [u32; 4]) -> Result<pdf_render::Canvas, String>,
    target: (&str, &JobSettings),
    progress: (impl FnMut(usize), &AtomicBool),
) -> Result<Option<i32>, JobError> {
    #[cfg(windows)]
    return crate::windows::print_sheets(sheets, drawing, draw, target, progress);
    #[cfg(not(windows))]
    return print_sheets_cups(sheets, drawing, draw, target, progress).map(Some);
}
