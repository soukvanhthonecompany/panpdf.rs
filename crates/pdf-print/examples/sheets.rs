use std::io::Write;
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_print::{Order, Orientation, PerSheet, Scaling, Settings};

fn options(
    mut arguments: impl Iterator<Item = String>,
) -> Result<(Settings, f64, bool), Box<dyn std::error::Error>> {
    let mut settings = Settings::default();
    let mut dpi = 150.0;
    let mut borders = false;
    while let Some(flag) = arguments.next() {
        match flag.as_str() {
            "--borders" => borders = true,
            "--no-rotate" => settings.auto_rotate = false,
            _ => {
                let value = arguments.next().ok_or("a flag without its value")?;
                match flag.as_str() {
                    "--paper" => {
                        settings.paper = pdf_print::papers()
                            .into_iter()
                            .find(|(name, _)| name.eq_ignore_ascii_case(&value))
                            .ok_or("no such paper")?
                            .1;
                    }
                    "--per-sheet" => {
                        settings.per_sheet = match value.split_once('x') {
                            Some((columns, rows)) => PerSheet::Grid {
                                columns: columns.parse()?,
                                rows: rows.parse()?,
                            },
                            None => PerSheet::Pages(value.parse()?),
                        };
                    }
                    "--scaling" => {
                        settings.scaling = match value.as_str() {
                            "fit" => Scaling::Fit,
                            "shrink" => Scaling::ShrinkOversized,
                            "actual" => Scaling::ActualSize,
                            percent => Scaling::Custom(percent.parse()?),
                        };
                    }
                    "--orientation" => {
                        settings.orientation = match value.as_str() {
                            "portrait" => Orientation::Portrait,
                            "landscape" => Orientation::Landscape,
                            _ => Orientation::Auto,
                        };
                    }
                    "--order" => {
                        settings.order = match value.as_str() {
                            "rltb" => Order::HorizontalReversed,
                            "tblr" => Order::Vertical,
                            "tbrl" => Order::VerticalReversed,
                            _ => Order::Horizontal,
                        };
                    }
                    "--margin" => settings.margin = value.parse::<f64>()? * 72.0 / 25.4,
                    "--dpi" => dpi = value.parse()?,
                    other => return Err(format!("unknown flag {other}").into()),
                }
            }
        }
    }
    Ok((settings, dpi, borders))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let (Some(input), Some(output)) = (arguments.next(), arguments.next()) else {
        return Err("usage: sheets IN.pdf OUT_DIR [options]".into());
    };
    let (settings, dpi, borders) = options(arguments)?;
    let bytes = std::fs::read(&input)?;
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
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
    println!("print allowance: {:?}", pdf_print::allowance(&source, b"")?);
    let fonts: Arc<dyn pdf_content::FontProvider> =
        Arc::new(pdf_content::SystemFontProvider::discover());
    let sheets = pdf_print::lay_out(&pages, &settings)?;
    std::fs::create_dir_all(&output)?;
    for (at, sheet) in sheets.iter().enumerate() {
        let image = pdf_print::draw_sheet(sheet, dpi, borders, |page, scale, region| {
            pdf_print::draw_printed(
                &source,
                (b"", Some(Arc::clone(&fonts))),
                page,
                scale,
                region,
            )
        })?;
        let path = std::path::Path::new(&output).join(format!("sheet-{:03}.ppm", at + 1));
        let mut file = std::fs::File::create(&path)?;
        write!(file, "P6\n{} {}\n255\n", image.width, image.height)?;
        file.write_all(&image.rgb)?;
        let placed: Vec<String> = sheet
            .placements
            .iter()
            .map(|placed| {
                format!(
                    "p{} x{:.3}{}",
                    placed.page + 1,
                    placed.scale,
                    if placed.turned { " turned" } else { "" }
                )
            })
            .collect();
        println!(
            "{}: {:.0} x {:.0} pt, {}",
            path.display(),
            sheet.size[0],
            sheet.size[1],
            placed.join(", ")
        );
    }
    Ok(())
}
