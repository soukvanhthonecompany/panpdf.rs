use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::info::PrintAllowance;
use pdf_edit::reprotect::{Allowed, Asked, Wanted, rewrite};

fn main() -> std::process::ExitCode {
    let mut arguments = std::env::args().skip(1);
    let Some(from) = arguments.next() else {
        eprintln!("usage: reprotect <in.pdf> <out.pdf> [options]");
        return std::process::ExitCode::FAILURE;
    };
    let Some(to) = arguments.next() else {
        eprintln!("usage: reprotect <in.pdf> <out.pdf> [options]");
        return std::process::ExitCode::FAILURE;
    };

    let mut credential = Vec::new();
    let mut open = false;
    let mut asked = Asked::default();
    while let Some(argument) = arguments.next() {
        let mut word = || arguments.next().unwrap_or_default().into_bytes();
        match argument.as_str() {
            "--credential" => credential = word(),
            "--open" => open = true,
            "--user" => asked.user = word(),
            "--owner" => asked.owner = word(),
            "--no-print" => asked.allowed.print = PrintAllowance::Refused,
            "--no-modify" => asked.allowed.modify = false,
            "--no-copy" => asked.allowed.copy = false,
            other => {
                eprintln!("unexpected argument {other:?}");
                return std::process::ExitCode::FAILURE;
            }
        }
    }
    if !open
        && asked.allowed == Allowed::default()
        && asked.user.is_empty()
        && asked.owner.is_empty()
    {
        eprintln!("nothing was asked for: give --open, a password, or a restriction");
        return std::process::ExitCode::FAILURE;
    }

    let bytes = match std::fs::read(&from) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("{from}: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let was = bytes.len();
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let wanted = if open {
        Wanted::Open
    } else {
        Wanted::Protected(Box::new(asked))
    };
    let written = match rewrite(&source, &credential, &wanted) {
        Ok(written) => written,
        Err(error) => {
            eprintln!("{from}: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    if let Err(error) = std::fs::write(&to, &written) {
        eprintln!("{to}: {error}");
        return std::process::ExitCode::FAILURE;
    }
    #[expect(clippy::cast_precision_loss, reason = "a percentage shown to a person")]
    let change = (written.len() as f64 - was as f64) / was as f64 * 100.0;
    println!("{from}: {was} -> {} bytes ({change:+.1}%)", written.len());
    std::process::ExitCode::SUCCESS
}
