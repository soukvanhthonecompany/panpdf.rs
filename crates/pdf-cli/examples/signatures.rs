use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::signature::{Covers, signatures};

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: signatures <file.pdf>");
    let bytes = std::fs::read(&path).expect("read");
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    match signatures(&source, b"") {
        Err(error) => println!("{path}: {error}"),
        Ok(found) if found.is_empty() => println!("{path}: no signatures"),
        Ok(found) => {
            println!("{path}: {} signature(s)", found.len());
            for one in found {
                println!("  field    {}", one.field);
                println!("  kind     {:?}", one.kind);
                println!("  name     {:?}", one.name);
                println!("  signed   {:?}", one.signed.map(|stamp| stamp.write()));
                println!("  reason   {:?}", one.reason);
                println!("  encoding {}", one.encoding);
                match &one.checked {
                    None => println!("  checked  nothing to open"),
                    Some(checked) => {
                        println!("  verdict  {:?}", checked.integrity);
                        println!("  signer   {:?}", checked.signer);
                        println!("  issuer   {:?}", checked.issuer);
                        println!("  trust    {:?} ({} links)", checked.trust, checked.links);
                        println!(
                            "  crypto   {:?} {:?} bits, sound: {}",
                            checked.hash, checked.key_bits, checked.hash_is_sound
                        );
                        println!(
                            "  when     {:?}, certificate current: {}",
                            checked.signed_at.map(pdf_edit::signature::Moment::write),
                            checked.certificate_is_current
                        );
                    }
                }
                match one.covers {
                    Covers::WholeDocument => println!("  covers   the whole document"),
                    Covers::UpTo { signed_through, of } => println!(
                        "  covers   {signed_through} of {of} bytes ({} added after)",
                        of - signed_through
                    ),
                    Covers::Unstated => println!("  covers   not stated"),
                }
                println!();
            }
        }
    }
}
