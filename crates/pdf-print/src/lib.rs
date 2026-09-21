pub mod cups;
pub mod ipp;
pub mod job;
pub mod layout;
pub mod service;
pub mod sheet;
pub mod windows;

use std::sync::Arc;

use pdf_bytes::ByteStore;
use pdf_content::{PageContentLimits, PrintAllowance};
use pdf_render::{Canvas, RenderOptions};

pub use layout::{
    LayoutError, Order, Orientation, Paper, PerSheet, Placement, Scaling, Settings, Sheet, lay_out,
    media_name, papers,
};
pub use sheet::{SheetError, SheetImage, draw_sheet};

pub const DEGRADED_DPI: f64 = 150.0;

pub const PRINT_DPI: f64 = 300.0;

#[must_use]
pub fn job_dpi(printer: Option<i32>, allowance: PrintAllowance) -> f64 {
    if allowance == PrintAllowance::Degraded {
        return DEGRADED_DPI;
    }
    printer.map_or(PRINT_DPI, |dpi| {
        f64::from(dpi).clamp(DEGRADED_DPI, PRINT_DPI)
    })
}

pub fn allowance(
    source: &ByteStore,
    credential: &[u8],
) -> Result<PrintAllowance, pdf_content::PageContentError> {
    pdf_content::load_page_program_with_password(
        source,
        0,
        PageContentLimits::default(),
        credential,
    )
    .map(|program| program.print_allowance())
}

pub fn draw_printed(
    source: &ByteStore,
    (credential, fonts): (&[u8], Option<Arc<dyn pdf_content::FontProvider>>),
    page: usize,
    scale: f64,
    region: [u32; 4],
) -> Result<Canvas, String> {
    let view = pdf_session::interpret_page_for_print(source, page, credential, fonts)
        .map_err(|error| error.to_string())?;
    pdf_render::render_region_layers(
        &view.layers(),
        &view.program.geometry,
        RenderOptions {
            scale,
            ..RenderOptions::default()
        },
        region,
    )
    .map(|(canvas, _)| canvas)
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod cups_tests;
#[cfg(test)]
mod ipp_tests;
#[cfg(test)]
mod job_tests;
#[cfg(test)]
mod layout_tests;
#[cfg(test)]
mod print_tests;
#[cfg(test)]
mod sheet_tests;
#[cfg(test)]
mod windows_tests;
