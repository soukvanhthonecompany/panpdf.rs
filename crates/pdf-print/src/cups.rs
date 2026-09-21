use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use crate::ipp::{self, Attribute, Message, Value, tag};
pub use crate::service::{Capabilities, JobSettings, Printer, Sides};

const TIMEOUT: Duration = Duration::from_mins(5);

const MOST_ANSWER: u64 = 16 * 1024 * 1024;

#[derive(Debug)]
pub enum CupsError {
    NoServer(std::io::Error),
    Io(std::io::Error),
    Garbled(String),
    Refused { status: u16, message: String },
}

impl std::fmt::Display for CupsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoServer(error) => write!(formatter, "no print server answers: {error}"),
            Self::Io(error) => write!(formatter, "the print server stopped answering: {error}"),
            Self::Garbled(what) => {
                write!(formatter, "the print server's answer is garbled: {what}")
            }
            Self::Refused { status, message } => {
                write!(
                    formatter,
                    "the print server refused (0x{status:04x}): {message}"
                )
            }
        }
    }
}

impl std::error::Error for CupsError {}

enum Server {
    #[cfg_attr(not(unix), allow(dead_code))]
    Socket(std::path::PathBuf),
    Tcp(String),
}

fn servers() -> Vec<Server> {
    if let Ok(named) = std::env::var("CUPS_SERVER")
        && !named.is_empty()
    {
        return vec![if named.starts_with('/') {
            Server::Socket(named.into())
        } else if named.contains(':') {
            Server::Tcp(named)
        } else {
            Server::Tcp(format!("{named}:631"))
        }];
    }
    vec![
        Server::Socket("/run/cups/cups.sock".into()),
        Server::Socket("/var/run/cups/cups.sock".into()),
        Server::Socket("/private/var/run/cupsd".into()),
        Server::Tcp("localhost:631".to_owned()),
    ]
}

trait Stream: Read + Write {}
impl<T: Read + Write> Stream for T {}

fn connect() -> Result<Box<dyn Stream>, CupsError> {
    let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, "no print server");
    for server in servers() {
        match server {
            #[cfg(unix)]
            Server::Socket(path) => match std::os::unix::net::UnixStream::connect(&path) {
                Ok(stream) => {
                    stream
                        .set_read_timeout(Some(TIMEOUT))
                        .map_err(CupsError::Io)?;
                    stream
                        .set_write_timeout(Some(TIMEOUT))
                        .map_err(CupsError::Io)?;
                    return Ok(Box::new(stream));
                }
                Err(error) => last = error,
            },
            #[cfg(not(unix))]
            Server::Socket(_) => {}
            Server::Tcp(address) => match std::net::TcpStream::connect(&address) {
                Ok(stream) => {
                    stream
                        .set_read_timeout(Some(TIMEOUT))
                        .map_err(CupsError::Io)?;
                    stream
                        .set_write_timeout(Some(TIMEOUT))
                        .map_err(CupsError::Io)?;
                    return Ok(Box::new(stream));
                }
                Err(error) => last = error,
            },
        }
    }
    Err(CupsError::NoServer(last))
}

fn path_of(printer: &str) -> String {
    let mut path = String::from("/printers/");
    for byte in printer.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            path.push(char::from(byte));
        } else {
            const HEX: &[u8; 16] = b"0123456789ABCDEF";
            path.push('%');
            path.push(char::from(HEX[usize::from(byte >> 4)]));
            path.push(char::from(HEX[usize::from(byte & 0x0F)]));
        }
    }
    path
}

fn user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "panpdf".to_owned())
}

fn exchange(path: &str, request: &Message, document: Option<&Path>) -> Result<Message, CupsError> {
    let head = request.encode();
    let document_length = match document {
        Some(file) => std::fs::metadata(file).map_err(CupsError::Io)?.len(),
        None => 0,
    };
    let length = u64::try_from(head.len()).unwrap_or(u64::MAX) + document_length;
    let mut stream = connect()?;
    let header = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/ipp\r\n\
         Content-Length: {length}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(header.as_bytes()).map_err(CupsError::Io)?;
    stream.write_all(&head).map_err(CupsError::Io)?;
    if let Some(file) = document {
        let mut reader = std::fs::File::open(file).map_err(CupsError::Io)?;
        std::io::copy(&mut reader, &mut stream).map_err(CupsError::Io)?;
    }
    stream.flush().map_err(CupsError::Io)?;
    let mut answer = Vec::new();
    stream
        .take(MOST_ANSWER)
        .read_to_end(&mut answer)
        .map_err(CupsError::Io)?;
    let body = http_body(&answer)?;
    let (message, _) =
        Message::decode(&body).map_err(|error| CupsError::Garbled(error.to_string()))?;
    if !message.succeeded() {
        let said = message
            .groups_of(tag::OPERATION)
            .find_map(|group| group.get("status-message"))
            .and_then(Attribute::value)
            .and_then(Value::text)
            .unwrap_or_default()
            .to_owned();
        return Err(CupsError::Refused {
            status: message.code,
            message: said,
        });
    }
    Ok(message)
}

pub(crate) fn http_body(answer: &[u8]) -> Result<Vec<u8>, CupsError> {
    let split = answer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| CupsError::Garbled("no end of headers".to_owned()))?;
    let head = String::from_utf8_lossy(&answer[..split]);
    let body = &answer[split + 4..];
    let mut lines = head.lines();
    let status = lines.next().unwrap_or_default();
    if status.split_whitespace().nth(1) != Some("200") {
        return Err(CupsError::Garbled(status.to_owned()));
    }
    let chunked = lines.any(|line| {
        let lower = line.to_ascii_lowercase();
        lower.starts_with("transfer-encoding:") && lower.contains("chunked")
    });
    if !chunked {
        return Ok(body.to_vec());
    }
    let mut out = Vec::new();
    let mut at = 0;
    loop {
        let line_end = body[at..]
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| CupsError::Garbled("a chunk without its size".to_owned()))?;
        let size_text = String::from_utf8_lossy(&body[at..at + line_end]);
        let size_text = size_text.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| CupsError::Garbled("a chunk size that is not hex".to_owned()))?;
        at += line_end + 2;
        if size == 0 {
            return Ok(out);
        }
        let chunk = body
            .get(at..at + size)
            .ok_or_else(|| CupsError::Garbled("a chunk that ends early".to_owned()))?;
        out.extend_from_slice(chunk);
        at += size + 2;
    }
}

fn about(operation: u16, printer: &str) -> Message {
    let mut request = Message::request(operation, 1);
    request.add(
        tag::OPERATION,
        ipp::text(
            "printer-uri",
            tag::URI,
            &format!("ipp://localhost{}", path_of(printer)),
        ),
    );
    request.add(
        tag::OPERATION,
        ipp::text("requesting-user-name", tag::NAME, &user()),
    );
    request
}

fn asking(request: &mut Message, names: &[&str]) {
    request.add(
        tag::OPERATION,
        Attribute {
            name: "requested-attributes".to_owned(),
            values: names
                .iter()
                .map(|name| Value::Text(tag::KEYWORD, (*name).to_owned()))
                .collect(),
        },
    );
}

pub fn printers() -> Result<(Vec<Printer>, Option<String>), CupsError> {
    let mut request = Message::request(ipp::operation::CUPS_GET_PRINTERS, 1);
    asking(
        &mut request,
        &["printer-name", "printer-info", "printer-is-accepting-jobs"],
    );
    let answer = match exchange("/", &request, None) {
        Ok(answer) => answer,
        Err(CupsError::Refused { status: 0x0406, .. }) => return Ok((Vec::new(), None)),
        Err(error) => return Err(error),
    };
    let printers: Vec<Printer> = answer
        .groups_of(tag::PRINTER)
        .filter_map(|group| {
            let name = group.get("printer-name")?.value()?.text()?.to_owned();
            let info = group
                .get("printer-info")
                .and_then(Attribute::value)
                .and_then(Value::text)
                .filter(|info| !info.is_empty())
                .unwrap_or(&name)
                .to_owned();
            let accepting = group
                .get("printer-is-accepting-jobs")
                .and_then(Attribute::value)
                .is_none_or(|value| *value == Value::Boolean(true));
            Some(Printer {
                name,
                info,
                accepting,
            })
        })
        .collect();
    let mut request = Message::request(ipp::operation::CUPS_GET_DEFAULT, 2);
    asking(&mut request, &["printer-name"]);
    let default = exchange("/", &request, None).ok().and_then(|answer| {
        answer
            .groups_of(tag::PRINTER)
            .find_map(|group| group.get("printer-name"))
            .and_then(Attribute::value)
            .and_then(Value::text)
            .map(str::to_owned)
    });
    Ok((printers, default))
}

pub fn capabilities(printer: &str) -> Result<Capabilities, CupsError> {
    let mut request = about(ipp::operation::GET_PRINTER_ATTRIBUTES, printer);
    asking(
        &mut request,
        &[
            "media-supported",
            "media-default",
            "media-col-database",
            "color-supported",
            "sides-supported",
            "printer-resolution-default",
        ],
    );
    let answer = exchange(&path_of(printer), &request, None)?;
    let mut found = Capabilities::default();
    for group in answer.groups_of(tag::PRINTER) {
        for attribute in &group.attributes {
            match attribute.name.as_str() {
                "media-supported" => {
                    found.media = attribute
                        .values
                        .iter()
                        .filter_map(Value::text)
                        .map(str::to_owned)
                        .collect();
                }
                "media-default" => {
                    found.media_default =
                        attribute.value().and_then(Value::text).map(str::to_owned);
                }
                "color-supported" => {
                    found.colour = attribute.value() == Some(&Value::Boolean(true));
                }
                "sides-supported" => {
                    found.two_sided = attribute
                        .values
                        .iter()
                        .filter_map(Value::text)
                        .any(|sides| sides.starts_with("two-sided"));
                }
                "media-col-database" => {
                    found.margins = attribute.values.iter().filter_map(margins_of).collect();
                }
                "printer-resolution-default" => {
                    found.resolution = match attribute.value() {
                        Some(Value::Resolution(across, _, 3)) => Some(*across),
                        Some(Value::Resolution(across, _, 4)) => Some(across * 254 / 100),
                        _ => None,
                    };
                }
                _ => {}
            }
        }
    }
    Ok(found)
}

fn margins_of(value: &Value) -> Option<([i32; 2], [i32; 4])> {
    let Value::Collection(members) = value else {
        return None;
    };
    let Value::Collection(size) = Attribute::member(members, "media-size")? else {
        return None;
    };
    let across = Attribute::member(size, "x-dimension")?.integer()?;
    let down = Attribute::member(size, "y-dimension")?.integer()?;
    let edge = |name: &str| {
        Attribute::member(members, name)
            .and_then(Value::integer)
            .unwrap_or(0)
    };
    Some((
        [across, down],
        [
            edge("media-bottom-margin"),
            edge("media-left-margin"),
            edge("media-right-margin"),
            edge("media-top-margin"),
        ],
    ))
}

pub fn print_file(printer: &str, document: &Path, job: &JobSettings) -> Result<i32, CupsError> {
    let mut request = about(ipp::operation::PRINT_JOB, printer);
    request.add(tag::OPERATION, ipp::text("job-name", tag::NAME, &job.title));
    request.add(
        tag::OPERATION,
        ipp::text("document-format", tag::MIME_TYPE, "application/pdf"),
    );
    request.add(
        tag::JOB,
        ipp::integer("copies", i32::from(job.copies.max(1))),
    );
    request.add(tag::JOB, ipp::text("media", tag::KEYWORD, &job.media));
    request.add(tag::JOB, ipp::text("print-scaling", tag::KEYWORD, "none"));
    request.add(tag::JOB, ipp::integer("number-up", 1));
    request.add(
        tag::JOB,
        ipp::text(
            "print-color-mode",
            tag::KEYWORD,
            if job.colour { "color" } else { "monochrome" },
        ),
    );
    request.add(
        tag::JOB,
        ipp::text(
            "multiple-document-handling",
            tag::KEYWORD,
            "separate-documents-collated-copies",
        ),
    );
    request.add(
        tag::JOB,
        ipp::text("sides", tag::KEYWORD, sides_keyword(job.sides)),
    );
    if job.hold {
        request.add(
            tag::JOB,
            ipp::text("job-hold-until", tag::KEYWORD, "indefinite"),
        );
    }
    let answer = exchange(&path_of(printer), &request, Some(document))?;
    answer
        .groups_of(tag::JOB)
        .find_map(|group| group.get("job-id"))
        .and_then(Attribute::value)
        .and_then(Value::integer)
        .ok_or_else(|| CupsError::Garbled("no job number".to_owned()))
}

pub(crate) const fn sides_keyword(sides: Sides) -> &'static str {
    match sides {
        Sides::One => "one-sided",
        Sides::TwoLongEdge => "two-sided-long-edge",
        Sides::TwoShortEdge => "two-sided-short-edge",
    }
}

pub fn job_state(printer: &str, job: i32) -> Result<(i32, String), CupsError> {
    let mut request = about(ipp::operation::GET_JOB_ATTRIBUTES, printer);
    request.add(tag::OPERATION, ipp::integer("job-id", job));
    asking(
        &mut request,
        &[
            "job-state",
            "job-state-reasons",
            "job-printer-state-message",
        ],
    );
    let answer = exchange(&path_of(printer), &request, None)?;
    let group = answer
        .groups_of(tag::JOB)
        .next()
        .ok_or_else(|| CupsError::Garbled("no job attributes".to_owned()))?;
    let state = group
        .get("job-state")
        .and_then(Attribute::value)
        .and_then(Value::integer)
        .unwrap_or(0);
    let reasons: Vec<&str> = group
        .get("job-state-reasons")
        .map(|reasons| reasons.values.iter().filter_map(Value::text).collect())
        .unwrap_or_default();
    Ok((state, reasons.join(", ")))
}

pub fn cancel(printer: &str, job: i32) -> Result<(), CupsError> {
    let mut request = about(ipp::operation::CANCEL_JOB, printer);
    request.add(tag::OPERATION, ipp::integer("job-id", job));
    exchange(&path_of(printer), &request, None).map(|_| ())
}
