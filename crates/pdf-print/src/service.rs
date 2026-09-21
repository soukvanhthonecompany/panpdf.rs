use crate::cups::CupsError;
#[cfg(windows)]
use crate::windows::WindowsError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Printer {
    pub name: String,
    pub info: String,
    pub accepting: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub media: Vec<String>,
    pub media_default: Option<String>,
    pub margins: Vec<([i32; 2], [i32; 4])>,
    pub colour: bool,
    pub two_sided: bool,
    pub resolution: Option<i32>,
}

impl Capabilities {
    #[must_use]
    pub fn margin_for(&self, size: [i32; 2]) -> Option<i32> {
        self.margins
            .iter()
            .filter(|(known, _)| {
                (known[0] - size[0]).abs() <= 100 && (known[1] - size[1]).abs() <= 100
            })
            .map(|(_, edges)| edges.iter().copied().max().unwrap_or(0))
            .max()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sides {
    #[default]
    One,
    TwoLongEdge,
    TwoShortEdge,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobSettings {
    pub title: String,
    pub copies: u16,
    pub media: String,
    pub colour: bool,
    pub sides: Sides,
    pub hold: bool,
}

#[derive(Debug)]
pub enum ServiceError {
    Cups(CupsError),
    #[cfg(windows)]
    Windows(WindowsError),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cups(error) => error.fmt(formatter),
            #[cfg(windows)]
            Self::Windows(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ServiceError {}

pub fn printers() -> Result<(Vec<Printer>, Option<String>), ServiceError> {
    #[cfg(windows)]
    return crate::windows::printers().map_err(ServiceError::Windows);
    #[cfg(not(windows))]
    return crate::cups::printers().map_err(ServiceError::Cups);
}

pub fn capabilities(printer: &str) -> Result<Capabilities, ServiceError> {
    #[cfg(windows)]
    return crate::windows::capabilities(printer).map_err(ServiceError::Windows);
    #[cfg(not(windows))]
    return crate::cups::capabilities(printer).map_err(ServiceError::Cups);
}
