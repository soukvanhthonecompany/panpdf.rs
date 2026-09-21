use pdf_cli::census;
use std::io::{self, Write};
use std::path::Path;

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err(
            "usage: census_page FILE.pdf PAGE_INDEX (batch: probes/font-census/measure.py)".into(),
        );
    }
    let page: usize = args[1].parse()?;
    let end = page.checked_add(1).ok_or("page index overflow")?;
    let path = Path::new(&args[0]);
    let bytes = std::fs::read(path)?;
    let result = census::census_range(path, &bytes, page..end, b"", pdf_cli::font_provider());
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    for line in census::json_lines(&result) {
        writeln!(stdout, "{line}")?;
    }
    stdout.flush()?;
    Ok(())
}
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("census_page: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
