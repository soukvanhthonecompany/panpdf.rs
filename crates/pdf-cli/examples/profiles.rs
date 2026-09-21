use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_paint::{ColorSpace, IccBasedSpace, PaintAtomKind};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Shape {
    MatrixTrc,
    GrayTrc,
    Lut,
    Unreadable,
}

impl Shape {
    fn name(self) -> &'static str {
        match self {
            Self::MatrixTrc => "matrix and tone curves",
            Self::GrayTrc => "one tone curve (gray)",
            Self::Lut => "a lookup table (A2B0)",
            Self::Unreadable => "unreadable or neither",
        }
    }
}

fn tags(profile: &[u8]) -> Option<Vec<[u8; 4]>> {
    let count = u32::from_be_bytes(profile.get(128..132)?.try_into().ok()?) as usize;
    if count > (profile.len().saturating_sub(132)) / 12 {
        return None;
    }
    (0..count)
        .map(|index| {
            let at = 132 + index * 12;
            profile.get(at..at + 4)?.try_into().ok()
        })
        .collect()
}

fn shape_of(profile: &[u8]) -> Shape {
    let Some(tags) = tags(profile) else {
        return Shape::Unreadable;
    };
    let has = |signature: &[u8; 4]| tags.iter().any(|tag| tag == signature);
    if has(b"rXYZ") && has(b"gXYZ") && has(b"bXYZ") && has(b"rTRC") && has(b"gTRC") && has(b"bTRC")
    {
        Shape::MatrixTrc
    } else if has(b"kTRC") {
        Shape::GrayTrc
    } else if has(b"A2B0") {
        Shape::Lut
    } else {
        Shape::Unreadable
    }
}

#[derive(Default)]
struct Tally {
    pages: usize,
    atoms: usize,
    selections: BTreeMap<&'static str, usize>,
    profiles: HashMap<Vec<u8>, (Shape, [u8; 4], usize)>,
    calibrated_identity: usize,
    calibrated_real: usize,
}

fn is_identity_cal(space: &ColorSpace) -> bool {
    const IDENTITY: [f64; 9] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    let one = |value: f64| (value - 1.0).abs() < 1e-9;
    match space {
        ColorSpace::CalGray(definition) => one(definition.gamma.value),
        ColorSpace::CalRgb(definition) => {
            definition.gamma.value.iter().copied().all(one)
                && definition
                    .matrix
                    .value
                    .iter()
                    .zip(IDENTITY)
                    .all(|(held, want)| (held - want).abs() < 1e-9)
        }
        _ => false,
    }
}

fn name_of(space: &ColorSpace) -> &'static str {
    match space {
        ColorSpace::DeviceGray => "DeviceGray",
        ColorSpace::DeviceRgb => "DeviceRGB",
        ColorSpace::DeviceCmyk => "DeviceCMYK",
        ColorSpace::CalGray(_) => "CalGray",
        ColorSpace::CalRgb(_) => "CalRGB",
        ColorSpace::Lab(_) => "Lab",
        ColorSpace::IccBased(_) => "ICCBased",
        ColorSpace::Indexed(_) => "Indexed",
        ColorSpace::Separation(_) => "Separation",
        ColorSpace::DeviceN(_) => "DeviceN",
        ColorSpace::Pattern(_) => "Pattern",
    }
}

fn spaces_of(space: &ColorSpace, into: &mut Vec<ColorSpace>) {
    into.push(space.clone());
    match space {
        ColorSpace::Indexed(definition) => match &definition.base.value {
            pdf_paint::IndexedBase::IccBased(inner) => {
                spaces_of(&ColorSpace::IccBased(inner.clone()), into);
            }
            pdf_paint::IndexedBase::Separation(inner) => {
                spaces_of(&ColorSpace::Separation(inner.clone()), into);
            }
            pdf_paint::IndexedBase::DeviceN(inner) => {
                spaces_of(&ColorSpace::DeviceN(inner.clone()), into);
            }
            pdf_paint::IndexedBase::CalGray(inner) => {
                spaces_of(&ColorSpace::CalGray(inner.clone()), into);
            }
            pdf_paint::IndexedBase::CalRgb(inner) => {
                spaces_of(&ColorSpace::CalRgb(inner.clone()), into);
            }
            _ => {}
        },
        ColorSpace::Separation(definition) => {
            spaces_of(&definition.alternate.value, into);
        }
        ColorSpace::DeviceN(definition) => {
            spaces_of(&definition.alternate.value, into);
        }
        _ => {}
    }
}

fn record(tally: &mut Tally, space: &ColorSpace, seen: &mut Vec<Shape>) {
    let mut found = Vec::new();
    spaces_of(space, &mut found);
    for space in &found {
        *tally.selections.entry(name_of(space)).or_default() += 1;
        match space {
            ColorSpace::CalGray(_) | ColorSpace::CalRgb(_) => {
                if is_identity_cal(space) {
                    tally.calibrated_identity += 1;
                } else {
                    tally.calibrated_real += 1;
                }
            }
            ColorSpace::IccBased(definition) => seen.push(note_profile(tally, definition)),
            _ => {}
        }
    }
}

fn note_profile(tally: &mut Tally, definition: &Arc<IccBasedSpace>) -> Shape {
    let bytes = definition.profile.to_vec();
    let entry = tally.profiles.entry(bytes).or_insert_with(|| {
        (
            shape_of(&definition.profile),
            definition.header.data_color_space,
            0,
        )
    });
    entry.2 += 1;
    entry.0
}

fn measure(path: &PathBuf, tally: &mut Tally, verbose: bool) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let Ok(view) = pdf_session::interpret_page(&source, 0) else {
        return;
    };
    tally.pages += 1;
    let before = tally.atoms;
    let mut seen: Vec<Shape> = Vec::new();
    for atom in &view.graph.atoms {
        let state = match &atom.kind {
            PaintAtomKind::Image(paint) => &paint.state,
            PaintAtomKind::Path(paint) => &paint.state,
            PaintAtomKind::Text(paint) => &paint.state,
            PaintAtomKind::Shading(paint) => &paint.state,
            PaintAtomKind::TransparencyGroup(paint) => &paint.state,
        };
        tally.atoms += 1;
        record(tally, &state.fill_color_space.value, &mut seen);
        record(tally, &state.stroke_color_space.value, &mut seen);
        if let PaintAtomKind::Shading(paint) = &atom.kind {
            record(tally, &paint.color_space.value, &mut seen);
        }
    }
    if verbose {
        let mut shapes: Vec<&'static str> = seen.iter().map(|shape| shape.name()).collect();
        shapes.sort_unstable();
        shapes.dedup();
        println!(
            "{}: {} atoms{}{}",
            path.display(),
            tally.atoms - before,
            if shapes.is_empty() { "" } else { "; " },
            shapes.join(", ")
        );
    }
}

fn furthest_from_its_alternate(profile: &[u8]) -> Option<u8> {
    let transform = pdf_render::icc::IccTransform::of(profile)?;
    let mut worst = 0_u8;
    for red in 0..9 {
        for green in 0..9 {
            for blue in 0..9 {
                let components = [red, green, blue].map(|step| f64::from(step) / 8.0);
                let applied = transform.to_srgb(&components)?;
                for (managed, plain) in applied.iter().zip(components) {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let level = |value: f64| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
                    worst = worst.max(level(*managed).abs_diff(level(plain)));
                }
            }
        }
    }
    Some(worst)
}

fn description(profile: &[u8]) -> Option<String> {
    let count = u32::from_be_bytes(profile.get(128..132)?.try_into().ok()?) as usize;
    if count > profile.len().saturating_sub(132) / 12 {
        return None;
    }
    for index in 0..count {
        let at = 132 + index * 12;
        let entry = profile.get(at..at + 12)?;
        if &entry[0..4] != b"desc" {
            continue;
        }
        let offset = u32::from_be_bytes(entry[4..8].try_into().ok()?) as usize;
        let size = u32::from_be_bytes(entry[8..12].try_into().ok()?) as usize;
        let tag = profile.get(offset..offset.checked_add(size)?)?;
        let text = match tag.get(0..4)? {
            b"desc" => {
                let length = u32::from_be_bytes(tag.get(8..12)?.try_into().ok()?) as usize;
                tag.get(12..12 + length.min(128))?
            }
            b"mluc" => {
                let length = u32::from_be_bytes(tag.get(20..24)?.try_into().ok()?) as usize;
                let start = u32::from_be_bytes(tag.get(24..28)?.try_into().ok()?) as usize;
                tag.get(start..start + length.min(256))?
            }
            _ => return None,
        };
        let readable: String = text
            .iter()
            .filter(|byte| byte.is_ascii_graphic() || **byte == b' ')
            .map(|byte| char::from(*byte))
            .collect();
        return Some(readable.trim().to_owned());
    }
    None
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let target = PathBuf::from(arguments.next().expect("a pdf file or a directory"));
    let flags: Vec<String> = arguments.collect();
    let summary = flags.iter().any(|flag| flag == "--summary");
    let dump = flags
        .iter()
        .position(|flag| flag == "--dump")
        .and_then(|at| flags.get(at + 1))
        .map(PathBuf::from);
    let mut paths: Vec<PathBuf> = if target.is_dir() {
        let mut found: Vec<PathBuf> = std::fs::read_dir(&target)
            .expect("readdir")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
            })
            .collect();
        found.sort();
        found
    } else {
        vec![target]
    };
    paths.dedup();

    let mut tally = Tally::default();
    for path in &paths {
        measure(path, &mut tally, !summary);
    }

    println!(
        "\n{} pages, {} atoms; a space is counted once per atom that selects it, \
         fill and stroke separately",
        tally.pages, tally.atoms
    );
    for (name, count) in &tally.selections {
        println!("  {count:>7}  {name}");
    }

    println!(
        "\n  calibrated spaces whose parameters are the identity: {} \
         (already exact; the rest need the arithmetic: {})",
        tally.calibrated_identity, tally.calibrated_real
    );

    if let Some(directory) = dump.as_ref() {
        std::fs::create_dir_all(directory).expect("a directory to write profiles into");
        for (bytes, (shape, space, _)) in &tally.profiles {
            let digest = pdf_content::sha256_hex(bytes);
            let space = String::from_utf8_lossy(space).trim().to_string();
            let name = format!(
                "{}-{space}-{}.icc",
                match shape {
                    Shape::MatrixTrc => "matrix",
                    Shape::GrayTrc => "gray",
                    Shape::Lut => "lut",
                    Shape::Unreadable => "unreadable",
                },
                &digest[..16]
            );
            std::fs::write(directory.join(&name), bytes).expect("a profile to write");
            println!("  wrote {name} ({} bytes)", bytes.len());
        }
    }

    let mut by_shape: BTreeMap<Shape, (usize, usize)> = BTreeMap::new();
    let mut by_space: BTreeMap<String, usize> = BTreeMap::new();
    for (shape, data_space, selections) in tally.profiles.values() {
        let entry = by_shape.entry(*shape).or_default();
        entry.0 += 1;
        entry.1 += selections;
        *by_space
            .entry(String::from_utf8_lossy(data_space).trim().to_owned())
            .or_default() += 1;
    }
    println!(
        "\n  {} distinct ICC profiles, by what honouring one would take:",
        tally.profiles.len()
    );
    for (bytes, (shape, _, _)) in &tally.profiles {
        if *shape != Shape::MatrixTrc {
            continue;
        }
        let Some(worst) = furthest_from_its_alternate(bytes) else {
            continue;
        };
        println!(
            "  {:>7}  {}  the applied profile differs from DeviceRGB by at most {worst} levels",
            "",
            description(bytes).unwrap_or_else(|| "(no description)".to_owned())
        );
    }
    for (shape, (profiles, selections)) in &by_shape {
        println!(
            "  {profiles:>7} profiles, {selections:>6} selections  {}",
            shape.name()
        );
    }
    print!("  their data colour spaces:");
    for (space, count) in &by_space {
        print!(" {space} x{count}");
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::{Shape, shape_of, tags};

    fn profile(signatures: &[&[u8; 4]]) -> Vec<u8> {
        let mut bytes = vec![0_u8; 128];
        bytes.extend(u32::try_from(signatures.len()).unwrap().to_be_bytes());
        for signature in signatures {
            bytes.extend_from_slice(*signature);
            bytes.extend(0_u32.to_be_bytes());
            bytes.extend(0_u32.to_be_bytes());
        }
        bytes
    }

    #[test]
    fn each_shape_is_read_off_the_tag_table() {
        assert_eq!(
            shape_of(&profile(&[
                b"rXYZ", b"gXYZ", b"bXYZ", b"rTRC", b"gTRC", b"bTRC"
            ])),
            Shape::MatrixTrc
        );
        assert_eq!(
            shape_of(&profile(&[b"rXYZ", b"gXYZ", b"rTRC", b"gTRC", b"bTRC"])),
            Shape::Unreadable
        );
        assert_eq!(shape_of(&profile(&[b"kTRC"])), Shape::GrayTrc);
        assert_eq!(shape_of(&profile(&[b"A2B0", b"B2A0"])), Shape::Lut);
        assert_eq!(shape_of(&profile(&[b"desc", b"cprt"])), Shape::Unreadable);
    }

    #[test]
    fn a_profile_claiming_more_tags_than_it_holds_is_refused_not_read_past() {
        let mut lying = profile(&[b"rXYZ"]);
        lying[128..132].copy_from_slice(&1_000_000_u32.to_be_bytes());
        assert!(tags(&lying).is_none());
        assert_eq!(shape_of(&lying), Shape::Unreadable);

        assert!(tags(&[0_u8; 100]).is_none());
        assert_eq!(shape_of(&[0_u8; 100]), Shape::Unreadable);

        assert_eq!(tags(&profile(&[b"rXYZ", b"gXYZ"])).unwrap().len(), 2);
    }
}
