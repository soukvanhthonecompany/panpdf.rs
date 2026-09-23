use std::sync::atomic::AtomicBool;

use pdf_agent::connect::{Connection, Provider, Turn};

fn main() {
    let mut args = std::env::args().skip(1);
    let base_url = args
        .next()
        .unwrap_or_else(|| "http://127.0.0.1:11434/v1".to_owned());
    let model = args.next().unwrap_or_else(|| "qwen3.5:latest".to_owned());
    let asked = args
        .next()
        .unwrap_or_else(|| "say hello in three words".to_owned());
    let connection = Connection {
        provider: Provider::Ollama,
        base_url,
        api_key: String::new(),
        model,
        effort: pdf_agent::connect::Effort::None,
    };
    let turns = [Turn::person(asked)];
    let stop = AtomicBool::new(false);

    let started = std::time::Instant::now();
    let whole = connection.converse_with(&turns, None, &[], None, &stop);
    println!("not streamed, {:?}: {whole:?}", started.elapsed());

    let started = std::time::Instant::now();
    let mut first = None;
    let mut grew = 0_usize;
    let streamed = connection.converse_streaming(&turns, None, &[], None, &stop, &mut |far| {
        grew += 1;
        if first.is_none() && !far.said.is_empty() {
            first = Some(started.elapsed());
        }
    });
    println!(
        "streamed, {:?}, {grew} steps, first word after {first:?}: {streamed:?}",
        started.elapsed()
    );
}
