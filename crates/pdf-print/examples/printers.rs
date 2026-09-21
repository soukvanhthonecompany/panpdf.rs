use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use pdf_bytes::{ByteStore, SourceId};
use pdf_print::cups;
use pdf_print::service::{self, JobSettings, Sides};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let (printers, default) = service::printers()?;
    println!("default: {default:?}");
    for printer in &printers {
        println!(
            "{} ({}) accepting={}",
            printer.name, printer.info, printer.accepting
        );
        let found = service::capabilities(&printer.name)?;
        println!(
            "  media default {:?}, colour {}, two-sided {}",
            found.media_default, found.colour, found.two_sided
        );
        println!("  media {}", found.media.join(" "));
        println!(
            "  A4 margin {:?} (hundredths of a mm)",
            found.margin_for([21000, 29700])
        );
    }
    if arguments.first().map(String::as_str) == Some("--held-job") {
        let (input, printer) = (&arguments[1], &arguments[2]);
        let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(std::fs::read(input)?));
        let geometries = pdf_content::page_geometries_with_password(
            &source,
            pdf_content::PageContentLimits::default(),
            b"",
        )?;
        let pages: Vec<(usize, [f64; 2])> = geometries
            .iter()
            .enumerate()
            .map(|(page, geometry)| {
                let (width, height) = geometry.rotated_size();
                (page, [width, height])
            })
            .collect();
        let settings = pdf_print::Settings {
            per_sheet: pdf_print::PerSheet::Pages(4),
            ..pdf_print::Settings::default()
        };
        let sheets = pdf_print::lay_out(&pages, &settings)?;
        let fonts: Arc<dyn pdf_content::FontProvider> =
            Arc::new(pdf_content::SystemFontProvider::discover());
        let job = pdf_print::job::print_sheets(
            &sheets,
            (300.0, true),
            |page, scale, region| {
                pdf_print::draw_printed(
                    &source,
                    (b"", Some(Arc::clone(&fonts))),
                    page,
                    scale,
                    region,
                )
            },
            (
                printer,
                &JobSettings {
                    title: "PanPDF held test".to_owned(),
                    copies: 1,
                    media: "iso_a4_210x297mm".to_owned(),
                    colour: true,
                    hold: true,
                    sides: Sides::One,
                },
            ),
            (
                |done| println!("  sheet {done} drawn"),
                &AtomicBool::new(false),
            ),
        )?;
        if let Some(job) = job {
            println!("job {job}: {:?}", cups::job_state(printer, job)?);
            cups::cancel(printer, job)?;
            println!(
                "job {job} after cancel: {:?}",
                cups::job_state(printer, job)?
            );
        } else {
            println!("sent, but this system does not say the job's number");
        }
    }
    Ok(())
}
