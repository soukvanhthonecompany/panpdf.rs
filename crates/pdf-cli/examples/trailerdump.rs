use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_syntax::{ObjectKind, XrefLimits, parse_revision_chain_strict};

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: trailerdump <file.pdf>");
    let bytes = std::fs::read(&path).expect("read");
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let chain = match parse_revision_chain_strict(&source, XrefLimits::default()) {
        Ok(chain) => chain,
        Err(error) => {
            println!("the revision chain does not parse: {error}");
            return;
        }
    };
    println!("{} revision(s)", chain.revisions().len());
    for (index, revision) in chain.revisions().iter().enumerate() {
        let trailer = revision.trailer();
        print!("  revision {index}: trailer ");
        match trailer.kind() {
            ObjectKind::Dictionary(entries) => {
                println!("with {} entries", entries.len());
                for entry in entries {
                    let key = String::from_utf8_lossy(
                        &source.as_bytes()[entry.key().span().start()..entry.key().span().end()],
                    )
                    .into_owned();
                    println!("    {key} -> {:?}", entry.value().kind());
                }
            }
            other => println!("is not a dictionary: {other:?}"),
        }
    }
}
