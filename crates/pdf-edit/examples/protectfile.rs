use std::error::Error;

use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::reprotect::{Asked, Wanted, rewrite};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let from = args
        .next()
        .ok_or("usage: protectfile <in.pdf> <out.pdf> [owner-password]")?;
    let to = args
        .next()
        .ok_or("usage: protectfile <in.pdf> <out.pdf> [owner-password]")?;
    let owner = args.next().unwrap_or_else(|| "owner".to_owned());
    let source = ByteStore::new(SourceId::new(0), std::fs::read(&from)?);
    let asked = Asked {
        user: Vec::new(),
        owner: owner.into_bytes(),
        allowed: pdf_edit::reprotect::Allowed::default(),
    };
    let written = rewrite(&source, b"", &Wanted::Protected(Box::new(asked)))
        .map_err(|error| format!("{error:?}"))?;
    std::fs::write(&to, &written)?;
    println!("{to}\t{} bytes", written.len());
    Ok(())
}
