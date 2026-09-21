use std::io::Write;

fn main() {
    let mut arguments = std::env::args().skip(1);
    let (Some(path), Some(width), Some(height), Some(out)) = (
        arguments.next(),
        arguments.next().and_then(|value| value.parse::<u32>().ok()),
        arguments.next().and_then(|value| value.parse::<u32>().ok()),
        arguments.next(),
    ) else {
        eprintln!("usage: ccittraw <data> <width> <height> <out.pbm> [flags]");
        std::process::exit(2);
    };
    let mut parameters = pdf_paint::ccitt::CcittParameters {
        k: -1,
        columns: width,
        ..pdf_paint::ccitt::CcittParameters::default()
    };
    let rest: Vec<String> = arguments.collect();
    let mut index = 0;
    while index < rest.len() {
        match rest[index].as_str() {
            "--k" => {
                index += 1;
                parameters.k = rest
                    .get(index)
                    .and_then(|text| text.parse::<i64>().ok())
                    .unwrap_or(-1);
            }
            "--columns" => {
                index += 1;
                parameters.columns = rest
                    .get(index)
                    .and_then(|text| text.parse::<u32>().ok())
                    .unwrap_or(width);
            }
            "--rows" => {
                index += 1;
                parameters.rows = rest
                    .get(index)
                    .and_then(|text| text.parse::<u32>().ok())
                    .unwrap_or(0);
            }
            "--black-is-1" => parameters.black_is_1 = true,
            "--byte-align" => parameters.byte_align = true,
            "--end-of-line" => parameters.end_of_line = true,
            "--no-end-of-block" => parameters.end_of_block = false,
            other => {
                eprintln!("unknown flag {other}");
                std::process::exit(2);
            }
        }
        index += 1;
    }

    let data = std::fs::read(&path).expect("the encoded data reads");
    let samples = match pdf_paint::ccitt::decode(&data, &parameters, width, height, 1 << 28) {
        Ok(samples) => samples,
        Err(error) => {
            println!("refused: {error}");
            std::process::exit(3);
        }
    };
    let mut file = std::fs::File::create(&out).expect("the output opens");
    write!(file, "P4\n{width} {height}\n").expect("the header writes");
    let inverted: Vec<u8> = samples.iter().map(|byte| !byte).collect();
    file.write_all(&inverted).expect("the samples write");
    println!("decoded {width}x{height}, {} bytes", samples.len());
}
