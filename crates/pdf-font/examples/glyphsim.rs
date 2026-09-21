use std::collections::BTreeMap;

use pdf_font::glyph::GlyphProgram;
use pdf_font::outline_match::Raster;

fn main() {
    let faces: Vec<String> = std::env::args().skip(1).collect();
    assert!(faces.len() >= 2, "two or more faces to compare");
    let mut loaded: Vec<(String, BTreeMap<char, Raster>)> = Vec::new();
    for path in &faces {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let Ok(program) = GlyphProgram::parse(bytes) else {
            continue;
        };
        let mut by_character = BTreeMap::new();
        let sfnt = match &program {
            GlyphProgram::TrueType(font) => Some(font),
            GlyphProgram::Cff { sfnt, .. } => sfnt.as_deref(),
            GlyphProgram::Type1 { .. } => None,
        };
        let Some(sfnt) = sfnt else { continue };
        for (glyph, character) in sfnt.characters() {
            if let Some(raster) = Raster::of(&program, glyph) {
                by_character.entry(character).or_insert(raster);
            }
        }
        println!("{path}: {} characters", by_character.len());
        loaded.push((path.clone(), by_character));
    }

    let mut same: Vec<f64> = Vec::new();
    let mut other: Vec<f64> = Vec::new();
    for (index, (_, one)) in loaded.iter().enumerate() {
        for (_, two) in loaded.iter().skip(index + 1) {
            let shared: Vec<char> = one
                .keys()
                .filter(|key| two.contains_key(key))
                .copied()
                .collect();
            for character in &shared {
                same.push(one[character].disagreement(&two[character]));
            }
            for pair in shared.windows(2) {
                other.push(one[&pair[0]].disagreement(&two[&pair[1]]));
            }
        }
    }
    let report = |name: &str, values: &mut Vec<f64>| {
        if values.is_empty() {
            println!("{name}: none");
            return;
        }
        values.sort_by(f64::total_cmp);
        let at = |per_mille: usize| {
            let last = values.len() - 1;
            values[last * per_mille / 1000]
        };
        println!(
            "{name}: n={} min {:.2} p10 {:.2} median {:.2} p90 {:.2} max {:.2}",
            values.len(),
            values[0],
            at(100),
            at(500),
            at(900),
            values[values.len() - 1],
        );
    };
    report("same character, different design", &mut same);
    report("different characters           ", &mut other);
}
