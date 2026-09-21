use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let path = PathBuf::from(arguments.next().ok_or("usage: iccprobe <profile.icc>")?);
    let bytes = std::fs::read(&path)?;
    let transform = pdf_render::icc::IccTransform::of(&bytes).ok_or("profile refused")?;
    let components: usize = arguments.next().ok_or("component count")?.parse()?;
    let mut line = String::new();
    for input in std::io::stdin().lines() {
        line.clear();
        line.push_str(&input?);
        let values: Vec<f64> = line
            .split_whitespace()
            .map(|value| value.parse::<f64>().unwrap_or(0.0) / 255.0)
            .collect();
        if values.len() != components {
            continue;
        }
        match transform.to_srgb(&values) {
            Some(rgb) => {
                let level = |v: f64| {
                    #[allow(clippy::cast_possible_truncation)]
                    let rounded = (v.clamp(0.0, 1.0) * 255.0).round() as i64;
                    rounded
                };
                println!("{} {} {}", level(rgb[0]), level(rgb[1]), level(rgb[2]));
            }
            None => println!("refused"),
        }
    }
    Ok(())
}
