use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::layout::Sheet;
use crate::service::{Capabilities, JobSettings, Printer, Sides};
use crate::sheet::{SheetImage, draw_sheet};

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const SCRIPT: &str = include_str!("windows.ps1");

#[derive(Debug)]
pub enum WindowsError {
    NoPowerShell(std::io::Error),
    Io(std::io::Error),
    Refused(String),
    Garbled(String),
    TimedOut,
}

impl std::fmt::Display for WindowsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoPowerShell(error) => {
                write!(
                    formatter,
                    "Windows PowerShell could not be started: {error}"
                )
            }
            Self::Io(error) => write!(formatter, "the print service could not be asked: {error}"),
            Self::Refused(said) => write!(formatter, "Windows refused: {said}"),
            Self::Garbled(what) => {
                write!(formatter, "the print service's answer is garbled: {what}")
            }
            Self::TimedOut => formatter.write_str("the print service did not answer in time"),
        }
    }
}

impl std::error::Error for WindowsError {}

#[cfg_attr(not(windows), allow(dead_code))]
#[expect(
    clippy::cast_possible_truncation,
    reason = "a paper is a few thousand hundredths of an inch"
)]
fn hundredths_of_an_inch(points: f64) -> i32 {
    (points * 100.0 / 72.0).round() as i32
}

#[cfg_attr(not(windows), allow(dead_code))]
fn hundredths_of_a_millimetre(hundredths_of_an_inch: i32) -> i32 {
    hundredths_of_an_inch.saturating_mul(254) / 10
}

#[cfg_attr(not(windows), allow(dead_code))]
fn offered_sizes() -> String {
    crate::papers()
        .iter()
        .map(|(_, paper)| {
            format!(
                "{}x{}",
                hundredths_of_an_inch(paper.width),
                hundredths_of_an_inch(paper.height)
            )
        })
        .collect::<Vec<_>>()
        .join(";")
}

#[cfg_attr(not(windows), allow(dead_code))]
fn media_of(size: [i32; 2]) -> Option<&'static str> {
    crate::papers().iter().find_map(|(name, paper)| {
        let same =
            |points: f64, hundredths: i32| (hundredths_of_an_inch(points) - hundredths).abs() <= 4;
        (same(paper.width, size[0]) && same(paper.height, size[1]))
            .then(|| crate::media_name(name))
            .flatten()
    })
}

#[cfg_attr(not(windows), allow(dead_code))]
fn size_of(media: &str) -> Option<[i32; 2]> {
    crate::papers().iter().find_map(|(name, paper)| {
        (crate::media_name(name) == Some(media)).then(|| {
            [
                hundredths_of_an_inch(paper.width),
                hundredths_of_an_inch(paper.height),
            ]
        })
    })
}

#[cfg_attr(not(windows), allow(dead_code))]
fn from_base64(text: &str) -> Option<String> {
    let mut bits = 0_u32;
    let mut held = 0;
    let mut bytes = Vec::new();
    for byte in text.bytes().filter(|byte| *byte != b'=') {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(value);
        held += 6;
        if held >= 8 {
            held -= 8;
            bytes.push(u8::try_from((bits >> held) & 0xFF).ok()?);
        }
    }
    String::from_utf8(bytes).ok()
}

#[cfg_attr(not(windows), allow(dead_code))]
fn read_answer(out: &str) -> Result<Vec<Vec<&str>>, WindowsError> {
    let lines: Vec<Vec<&str>> = out
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.is_empty())
        .map(|line| line.split('\t').collect())
        .collect();
    if let Some(error) = lines.iter().find(|fields| fields[0] == "error") {
        let said = error.get(1).and_then(|said| from_base64(said));
        return Err(WindowsError::Refused(said.unwrap_or_else(|| {
            "an error it could not put into words".to_owned()
        })));
    }
    Ok(lines)
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn printers_from(out: &str) -> Result<(Vec<Printer>, Option<String>), WindowsError> {
    let mut printers = Vec::new();
    let mut default = None;
    for fields in read_answer(out)? {
        let name = || {
            fields
                .get(1)
                .and_then(|text| from_base64(text))
                .ok_or_else(|| WindowsError::Garbled(fields.join(" ")))
        };
        match fields[0] {
            "printer" => {
                let name = name()?;
                printers.push(Printer {
                    info: name.clone(),
                    name,
                    accepting: true,
                });
            }
            "default" => default = Some(name()?),
            _ => {}
        }
    }
    if printers.is_empty() {
        return Err(WindowsError::Garbled(
            "the print service listed no printers".to_owned(),
        ));
    }
    Ok((printers, default))
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn capabilities_from(out: &str) -> Result<Capabilities, WindowsError> {
    let mut found = Capabilities::default();
    for fields in read_answer(out)? {
        let numbers: Option<Vec<i32>> = fields[1..]
            .iter()
            .map(|number| number.parse().ok())
            .collect();
        let numbers = numbers.ok_or_else(|| WindowsError::Garbled(fields.join(" ")))?;
        match (fields[0], numbers.as_slice()) {
            ("colour", [said]) => found.colour = *said == 1,
            ("two-sided", [said]) => found.two_sided = *said == 1,
            ("resolution", [dpi]) => found.resolution = Some(*dpi),
            ("default-paper", [width, height]) => {
                found.media_default = media_of([*width, *height]).map(str::to_owned);
            }
            ("paper", [width, height]) => {
                if let Some(media) = media_of([*width, *height])
                    && !found.media.iter().any(|known| known == media)
                {
                    found.media.push(media.to_owned());
                }
            }
            ("margin", [width, height, left, top, right, bottom]) => {
                let size = [*width, *height].map(hundredths_of_a_millimetre);
                let edges = [*bottom, *left, *right, *top]
                    .map(|edge| hundredths_of_a_millimetre(edge.max(0)));
                found.margins.push((size, edges));
            }
            _ => return Err(WindowsError::Garbled(fields.join(" "))),
        }
    }
    if found.media.is_empty() {
        return Err(WindowsError::Garbled(
            "the print service listed no supported paper".to_owned(),
        ));
    }
    Ok(found)
}

#[cfg_attr(not(windows), allow(dead_code))]
const fn duplex(sides: Sides) -> u8 {
    match sides {
        Sides::One => 1,
        Sides::TwoLongEdge => 2,
        Sides::TwoShortEdge => 3,
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
fn job_environment(printer: &str, job: &JobSettings) -> Vec<(&'static str, String)> {
    let [width, height] = size_of(&job.media).unwrap_or([0, 0]);
    vec![
        ("PANPDF_MODE", "print".to_owned()),
        ("PANPDF_PRINTER", printer.to_owned()),
        ("PANPDF_TITLE", job.title.clone()),
        ("PANPDF_COPIES", job.copies.max(1).to_string()),
        ("PANPDF_COLOUR", u8::from(job.colour).to_string()),
        ("PANPDF_SIDES", duplex(job.sides).to_string()),
        ("PANPDF_PAPER", format!("{width}x{height}")),
    ]
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn sheet_bytes(size: [f64; 2], image: &SheetImage) -> Vec<u8> {
    let width = image.width as usize;
    let row = (width * 3 + 3) & !3;
    let mut bytes = Vec::with_capacity(24 + row * image.height as usize);
    bytes.extend_from_slice(&image.width.to_le_bytes());
    bytes.extend_from_slice(&image.height.to_le_bytes());
    bytes.extend_from_slice(&size[0].to_le_bytes());
    bytes.extend_from_slice(&size[1].to_le_bytes());
    for line in image.rgb.chunks_exact(width * 3) {
        let start = bytes.len();
        for pixel in line.chunks_exact(3) {
            bytes.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
        }
        bytes.resize(start + row, 0);
    }
    bytes
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn stream_sheets(
    out: &mut impl Write,
    sheets: &[Sheet],
    (dpi, borders): (f64, bool),
    mut draw: impl FnMut(usize, f64, [u32; 4]) -> Result<pdf_render::Canvas, String>,
    (mut drawn, stop): (impl FnMut(usize), &AtomicBool),
) -> Result<(), crate::job::JobError> {
    use crate::job::JobError;
    let count = u32::try_from(sheets.len())
        .map_err(|_| JobError::Io(std::io::Error::other("too many sheets for one job")))?;
    out.write_all(&count.to_le_bytes()).map_err(JobError::Io)?;
    for (at, sheet) in sheets.iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            return Err(JobError::Stopped);
        }
        let image = draw_sheet(sheet, dpi, borders, &mut draw).map_err(JobError::Sheet)?;
        out.write_all(&sheet_bytes(sheet.size, &image))
            .map_err(JobError::Io)?;
        drawn(at + 1);
    }
    out.flush().map_err(JobError::Io)
}

#[cfg(windows)]
pub(crate) use running::{capabilities, print_sheets, printers};

#[cfg(windows)]
mod running {
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use super::{WindowsError, capabilities_from, job_environment, printers_from};
    use crate::job::{JobError, PrivateDir};
    use crate::layout::Sheet;
    use crate::service::{Capabilities, JobSettings, Printer, ServiceError};

    const ASKING: Duration = Duration::from_secs(30);

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn powershell() -> std::path::PathBuf {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        std::path::PathBuf::from(root).join("System32\\WindowsPowerShell\\v1.0\\powershell.exe")
    }

    fn start(
        directory: &PrivateDir,
        environment: &[(&'static str, String)],
        fed: bool,
    ) -> Result<Child, WindowsError> {
        let script = directory.path.join("panpdf-print.ps1");
        std::fs::write(&script, super::SCRIPT).map_err(WindowsError::Io)?;
        let mut command = Command::new(powershell());
        command
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&script)
            .envs(environment.iter().map(|(name, value)| (*name, value)))
            .stdin(if fed { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW);
        command.spawn().map_err(WindowsError::NoPowerShell)
    }

    fn answer(child: Child, within: Option<Duration>) -> Result<String, WindowsError> {
        answer_with_cancel(child, within, None, None)
    }

    fn answer_with_cancel(
        mut child: Child,
        within: Option<Duration>,
        stop: Option<&AtomicBool>,
        abort: Option<&AtomicBool>,
    ) -> Result<String, WindowsError> {
        let mut out = child.stdout.take();
        let reading = std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(out) = out.as_mut() {
                let _ = out.read_to_string(&mut text);
            }
            text
        });
        let mut errors = child.stderr.take();
        let complaining = std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(errors) = errors.as_mut() {
                let _ = errors.read_to_string(&mut text);
            }
            text
        });
        let started = Instant::now();
        let status = loop {
            if stop.is_some_and(|flag| flag.load(Ordering::Relaxed))
                || abort.is_some_and(|flag| flag.load(Ordering::Relaxed))
            {
                let _ = child.kill();
                let _ = child.wait();
                return Err(WindowsError::TimedOut);
            }
            if let Some(status) = child.try_wait().map_err(WindowsError::Io)? {
                break status;
            }
            if within.is_some_and(|within| started.elapsed() > within) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(WindowsError::TimedOut);
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let text = reading.join().unwrap_or_default();
        let complaint = complaining.join().unwrap_or_default();
        if !status.success() {
            return Err(WindowsError::Refused(if complaint.trim().is_empty() {
                format!("PowerShell exited with {status}")
            } else {
                complaint.trim().to_owned()
            }));
        }
        if text.trim().is_empty() && !complaint.trim().is_empty() {
            return Err(WindowsError::Refused(complaint.trim().to_owned()));
        }
        Ok(text)
    }

    fn ask(environment: &[(&'static str, String)]) -> Result<String, WindowsError> {
        let directory = PrivateDir::new().map_err(WindowsError::Io)?;
        let child = start(&directory, environment, false)?;
        answer(child, Some(ASKING))
    }

    pub(crate) fn printers() -> Result<(Vec<Printer>, Option<String>), WindowsError> {
        printers_from(&ask(&[("PANPDF_MODE", "list".to_owned())])?)
    }

    pub(crate) fn capabilities(printer: &str) -> Result<Capabilities, WindowsError> {
        capabilities_from(&ask(&[
            ("PANPDF_MODE", "ask".to_owned()),
            ("PANPDF_PRINTER", printer.to_owned()),
            ("PANPDF_SIZES", super::offered_sizes()),
        ])?)
    }

    pub(crate) fn print_sheets(
        sheets: &[Sheet],
        drawing: (f64, bool),
        draw: impl FnMut(usize, f64, [u32; 4]) -> Result<pdf_render::Canvas, String>,
        (printer, settings): (&str, &JobSettings),
        (drawn, stop): (impl FnMut(usize), &AtomicBool),
    ) -> Result<Option<i32>, JobError> {
        let server = |error| JobError::Server(ServiceError::Windows(error));
        let directory = PrivateDir::new().map_err(JobError::Io)?;
        let mut child =
            start(&directory, &job_environment(printer, settings), true).map_err(server)?;
        let mut input = child
            .stdin
            .take()
            .ok_or_else(|| JobError::Io(std::io::Error::other("no pipe to the print script")))?;
        let abort = AtomicBool::new(false);
        let (fed, said) = std::thread::scope(|scope| {
            let supervisor =
                scope.spawn(|| answer_with_cancel(child, Some(ASKING), Some(stop), Some(&abort)));
            let fed = {
                let mut input = std::io::BufWriter::with_capacity(1 << 20, &mut input);
                super::stream_sheets(&mut input, sheets, drawing, draw, (drawn, stop))
            };
            if fed.is_err() {
                abort.store(true, Ordering::Relaxed);
            }
            drop(input);
            let said = supervisor.join().unwrap_or_else(|_| {
                Err(WindowsError::Refused(
                    "print supervisor panicked".to_owned(),
                ))
            });
            (fed, said)
        });
        match fed {
            Err(JobError::Stopped) => return Err(JobError::Stopped),
            Err(JobError::Io(_)) if said.is_ok() => {}
            Err(error) => return Err(error),
            Ok(()) => {}
        }
        let answer = said.map_err(server)?;
        let lines = super::read_answer(&answer).map_err(server)?;
        if stop.load(Ordering::Relaxed) || lines.iter().any(|fields| fields[0] == "stopped") {
            return Err(JobError::Stopped);
        }
        if lines.iter().any(|fields| fields[0] == "sent") {
            Ok(None)
        } else {
            Err(server(WindowsError::Garbled(
                "the print script ended without saying the job was sent".to_owned(),
            )))
        }
    }
}
