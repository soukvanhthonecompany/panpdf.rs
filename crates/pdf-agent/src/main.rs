use std::io::{BufRead, Write};

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("panpdf-mcp {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let mut server = pdf_agent::protocol::Server::default();
    let input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    for line in input.lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                eprintln!("panpdf-mcp: standard input: {error}");
                break;
            }
        };
        if let Some(reply) = server.answer_line(&line)
            && (writeln!(output, "{reply}").is_err() || output.flush().is_err())
        {
            break;
        }
    }
}
