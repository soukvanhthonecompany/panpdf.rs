fn main() {
    for path in std::env::args().skip(1) {
        let Ok(bytes) = std::fs::read(&path) else {
            println!("{path}: cannot be read from disk");
            continue;
        };
        match pdf_agent::attach::prepare(&path, &bytes) {
            Ok(parts) => {
                for part in &parts {
                    println!(
                        "{} -> {:?}, {} bytes",
                        part.name,
                        part.kind,
                        part.bytes.len()
                    );
                    if let Ok(text) = std::str::from_utf8(&part.bytes) {
                        let head: String = text.chars().take(300).collect();
                        println!("    {head}");
                    }
                }
            }
            Err(why) => println!("{path} refused: {why}"),
        }
    }
}
