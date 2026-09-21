use std::path::{Path, PathBuf};
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::spike_move_text::SpikeError;
use pdf_edit::{Command, FixedPoint, ObjectSelection, SourceAnchor};
use pdf_paint::{Matrix, PaintAtomKind};
use pdf_semantics::ObjectKind;

fn named(error: &pdf_session::PlanError) -> String {
    match error {
        pdf_session::PlanError::Plan(error) => match error {
            SpikeError::ObjectLeavesClip => "would leave its clip".to_owned(),
            SpikeError::ObjectIsCropped => "cropped by a clip that cannot travel".to_owned(),
            SpikeError::ClipNotRectangular => "clip encloses no area".to_owned(),
            SpikeError::ClipIsCurved => "clip has a curved edge".to_owned(),
            SpikeError::ClipIsConcave => "clip is a concave shape".to_owned(),
            SpikeError::ObjectInsideForm => "painted by a Form".to_owned(),
            SpikeError::RunNotDirectlyOnPage => "painted through a pattern".to_owned(),
            SpikeError::SharedPageContentStream => "content stream is shared".to_owned(),
            SpikeError::ObjectCtmSingular => "its own matrix has no area".to_owned(),
            SpikeError::ObjectNotInPageContent => "not in a rewritable stream".to_owned(),
            SpikeError::MoveNotIsolated => "proof: something else moved".to_owned(),
            SpikeError::MoveNotProvable => "proof: could not be run".to_owned(),
            other => format!("{other}"),
        },
        pdf_session::PlanError::Page(error) => format!("{error}"),
    }
}

fn origin(graph: &pdf_paint::PaintGraph, ordinal: usize) -> Option<(f64, f64)> {
    match &graph.atoms.get(ordinal)?.kind {
        PaintAtomKind::Image(image) => {
            let ctm = image.state.ctm.value;
            Some((ctm.e, ctm.f))
        }
        _ => None,
    }
}

struct Tally {
    pages: usize,
    files: usize,
    pictures: usize,
    moved: usize,
    refused: std::collections::BTreeMap<String, usize>,
    worst: f64,
    worst_file: String,
    unproved: Vec<String>,
}

#[derive(Clone, Copy)]
enum Gesture {
    Move(f64, f64),
    Scale(f64),
    Turn(f64),
    Remove,
}

impl Gesture {
    fn matrix(self) -> Matrix {
        match self {
            Self::Move(dx, dy) => Matrix {
                e: dx,
                f: dy,
                ..Matrix::IDENTITY
            },
            Self::Scale(by) => Matrix {
                a: by,
                d: by,
                ..Matrix::IDENTITY
            },
            Self::Turn(degrees) => {
                let angle = degrees.to_radians();
                Matrix {
                    a: angle.cos(),
                    b: angle.sin(),
                    c: -angle.sin(),
                    d: angle.cos(),
                    e: 0.0,
                    f: 0.0,
                }
            }
            Self::Remove => Matrix::IDENTITY,
        }
    }

    fn command(
        self,
        graph: &pdf_paint::PaintGraph,
        ordinal: usize,
        anchor: SourceAnchor,
    ) -> Command {
        match self {
            Self::Remove => Command::RemoveObject {
                page_index: 0,
                target: anchor,
            },
            _ => Command::PlaceObject {
                page_index: 0,
                target: ObjectSelection::Painted(anchor),
                transform: self.matrix(),
                about: self.about(graph, ordinal),
            },
        }
    }

    fn about(self, graph: &pdf_paint::PaintGraph, ordinal: usize) -> FixedPoint {
        match self {
            Self::Move(..) => FixedPoint::Origin,
            _ => graph
                .atoms
                .get(ordinal)
                .and_then(|atom| pdf_semantics::placed_quad(&atom.kind))
                .map_or(FixedPoint::Origin, |quad| FixedPoint::At(quad.center())),
        }
    }

    fn puts(self, point: (f64, f64), about: FixedPoint) -> (f64, f64) {
        let held = match about {
            FixedPoint::At(held) => (held.x, held.y),
            FixedPoint::Origin => (0.0, 0.0),
        };
        let moved = self.matrix().transform(pdf_paint::Point {
            x: point.0 - held.0,
            y: point.1 - held.1,
        });
        (moved.x + held.0, moved.y + held.1)
    }

    fn said(self) -> String {
        match self {
            Self::Move(dx, dy) => format!("moved by ({dx}, {dy}) points"),
            Self::Scale(by) => format!("scaled by {by} about its own centre"),
            Self::Turn(degrees) => format!("turned {degrees} degrees about its own centre"),
            Self::Remove => "taken off the page".to_owned(),
        }
    }
}

fn removed(
    before: &pdf_paint::PaintGraph,
    after: &pdf_paint::PaintGraph,
    ordinal: usize,
    path: &Path,
    tally: &mut Tally,
    verbose: bool,
) -> bool {
    let lost = before.atoms.len().saturating_sub(after.atoms.len());
    let residue = f64::from(u32::try_from(lost.abs_diff(1)).unwrap_or(u32::MAX));
    if residue > tally.worst {
        tally.worst = residue;
        tally.worst_file = path.display().to_string();
    }
    if lost != 1 {
        *tally
            .refused
            .entry(format!("the page lost {lost} atoms, not one"))
            .or_default() += 1;
        return false;
    }
    if verbose {
        println!("  {ordinal:>4}  gone; the page is one atom lighter");
    }
    true
}

fn pictures_of(view: &pdf_session::PageView) -> Vec<(usize, SourceAnchor)> {
    view.index
        .objects
        .iter()
        .filter(|object| object.kind == ObjectKind::Image)
        .filter_map(|object| {
            let ordinal = object.first_atom()?;
            Some((
                ordinal,
                SourceAnchor::of(&view.graph.atoms.get(ordinal)?.id),
            ))
        })
        .collect()
}

fn write_once(output: &Path, bytes: &[u8]) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .expect("--output must name a new file");
    file.write_all(bytes).expect("write placed fixture");
}

fn measure(path: &Path, gesture: Gesture, tally: &mut Tally, verbose: bool, output: Option<&Path>) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let Ok(view) = pdf_session::interpret_page(&source, 0) else {
        return;
    };
    tally.pages += 1;

    let pictures = pictures_of(&view);
    if pictures.is_empty() {
        return;
    }
    if output.is_some() && pictures.len() != 1 {
        eprintln!("--output requires a fixture with exactly one picture");
        std::process::exit(2);
    }
    tally.files += 1;
    if verbose {
        println!("{}: {} pictures", path.display(), pictures.len());
    }

    for (ordinal, anchor) in pictures {
        tally.pictures += 1;
        let mut session = pdf_session::Session::new(source.clone(), b"");
        let planned = session.plan(&gesture.command(&view.graph, ordinal, anchor));
        let plan = match planned {
            Ok(plan) => plan,
            Err(error) => {
                let reason = named(&error);
                if verbose {
                    println!("  {ordinal:>4}  refused: {reason}");
                }
                if reason.starts_with("proof:") {
                    let named_here = format!("{} ({reason})", path.display());
                    if !tally.unproved.contains(&named_here) {
                        tally.unproved.push(named_here);
                    }
                }
                *tally.refused.entry(reason).or_default() += 1;
                continue;
            }
        };
        if let Err(error) = session.apply(plan) {
            *tally.refused.entry(format!("commit: {error}")).or_default() += 1;
            continue;
        }
        if let Some(output) = output {
            write_once(output, session.source().as_bytes());
        }
        let Ok(after) = pdf_session::interpret_page(session.source(), 0) else {
            *tally
                .refused
                .entry("could not be read back".to_owned())
                .or_default() += 1;
            continue;
        };
        if matches!(gesture, Gesture::Remove) {
            if !removed(&view.graph, &after.graph, ordinal, path, tally, verbose) {
                continue;
            }
            tally.moved += 1;
            continue;
        }
        let (Some(was), Some(now)) = (origin(&view.graph, ordinal), origin(&after.graph, ordinal))
        else {
            *tally
                .refused
                .entry("no picture where one was".to_owned())
                .or_default() += 1;
            continue;
        };
        let was_quad = pdf_semantics::placed_quad(&view.graph.atoms[ordinal].kind).unwrap();
        let now_quad = pdf_semantics::placed_quad(&after.graph.atoms[ordinal].kind).unwrap();
        let about = gesture.about(&view.graph, ordinal);
        let expected = was_quad.corners.map(|point| {
            let (x, y) = gesture.puts((point.x, point.y), about);
            pdf_paint::Point { x, y }
        });
        let residue = corner_residue(expected, now_quad.corners);
        tally.moved += 1;
        if residue > tally.worst {
            tally.worst = residue;
            tally.worst_file = path.display().to_string();
        }
        if verbose {
            println!(
                "  {ordinal:>4}  ({:.2}, {:.2}) -> ({:.2}, {:.2})  off by {residue:.3e}",
                was.0, was.1, now.0, now.1
            );
        }
    }
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let target = PathBuf::from(arguments.next().expect("a pdf file or a directory"));
    let rest: Vec<String> = arguments.collect();
    let summary = rest.iter().any(|flag| flag == "--summary");
    let output = rest.iter().position(|flag| flag == "--output").map(|at| {
        assert!(target.is_file(), "--output requires one fixture file");
        PathBuf::from(rest.get(at + 1).expect("--output needs a new file path"))
    });
    let number = |flag: &str, fallback: f64| {
        rest.iter()
            .position(|argument| argument == flag)
            .and_then(|at| rest.get(at + 1))
            .and_then(|value| value.parse().ok())
            .unwrap_or(fallback)
    };
    let gesture = if rest.iter().any(|flag| flag == "--remove") {
        Gesture::Remove
    } else if rest.iter().any(|flag| flag == "--scale") {
        Gesture::Scale(number("--scale", 1.2))
    } else if rest.iter().any(|flag| flag == "--turn") {
        Gesture::Turn(number("--turn", 15.0))
    } else {
        Gesture::Move(number("--dx", 12.0), number("--dy", -7.0))
    };

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

    let mut tally = Tally {
        pages: 0,
        files: 0,
        pictures: 0,
        moved: 0,
        refused: std::collections::BTreeMap::new(),
        worst: 0.0,
        worst_file: String::new(),
        unproved: Vec::new(),
    };
    let verbose = !summary && paths.len() == 1;
    for path in &paths {
        measure(path, gesture, &mut tally, verbose, output.as_deref());
    }

    println!(
        "\n{}: {} of {} pictures, on {} of {} pages that interpret",
        gesture.said(),
        tally.moved,
        tally.pictures,
        tally.files,
        tally.pages
    );
    if tally.moved > 0 {
        if matches!(gesture, Gesture::Remove) {
            println!(
                "  worst discrepancy in what the page lost: {:.3e} atoms  ({})",
                tally.worst, tally.worst_file
            );
        } else {
            println!(
                "  furthest any picture landed from where it was asked to: {:.3e} points  ({})",
                tally.worst, tally.worst_file
            );
        }
    }
    for (reason, count) in &tally.refused {
        println!("  {count:>5} refused: {reason}");
    }
    for file in &tally.unproved {
        println!("  the proof refused in {file}");
    }
}

fn corner_residue(expected: [pdf_paint::Point; 4], actual: [pdf_paint::Point; 4]) -> f64 {
    expected
        .iter()
        .zip(actual)
        .fold(0.0_f64, |worst, (want, got)| {
            let distance = (want.x - got.x).hypot(want.y - got.y);
            if distance.is_finite() {
                worst.max(distance)
            } else {
                f64::INFINITY
            }
        })
}

#[cfg(test)]
mod tests {
    use super::corner_residue;
    use pdf_paint::Point;
    #[test]
    fn measuring_only_the_origin_would_miss_the_wrong_scale() {
        let square = [
            Point { x: 0.0, y: 0.0 },
            Point { x: 3.0, y: 0.0 },
            Point { x: 3.0, y: 4.0 },
            Point { x: 0.0, y: 4.0 },
        ];
        assert!(corner_residue(square, square).abs() < 1e-12);
        let doubled = square.map(|p| Point {
            x: 2.0 * p.x,
            y: 2.0 * p.y,
        });
        assert!((corner_residue(square, doubled) - 5.0).abs() < 1e-12);
        let mut broken = square;
        broken[1].x = f64::NAN;
        assert!(corner_residue(square, broken).is_infinite());
    }
}
