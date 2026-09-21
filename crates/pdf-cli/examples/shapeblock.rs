use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::spike_move_text::SpikeError;
use pdf_edit::{Command, FixedPoint, ObjectSelection, SourceAnchor};
use pdf_paint::{Matrix, PaintAtomKind, PaintGraph, Point};

fn named(error: &pdf_session::PlanError) -> String {
    match error {
        pdf_session::PlanError::Plan(error) => match error {
            SpikeError::BlockNotChainAligned => "a run is not at the start of its line".to_owned(),
            SpikeError::SizeAlreadySet => "already that size".to_owned(),
            SpikeError::SizeNotAsAsked => "proof: not the size that was asked for".to_owned(),
            SpikeError::RunSelectsNoFont => "shows text with no font selected".to_owned(),
            SpikeError::RunExtentUnknown => "its font resolves to no outline".to_owned(),
            SpikeError::ObjectLeavesClip => "would leave its clip".to_owned(),
            SpikeError::ObjectIsCropped => "cropped by a clip that cannot travel".to_owned(),
            SpikeError::ClipNotRectangular => "clip encloses no area".to_owned(),
            SpikeError::ClipIsCurved => "clip has a curved edge".to_owned(),
            SpikeError::ClipIsConcave => "clip is a concave shape".to_owned(),
            SpikeError::BlockRunInsideForm => "painted by a Form".to_owned(),
            SpikeError::RunNotDirectlyOnPage => "painted through a pattern".to_owned(),
            SpikeError::BlockSpansSeveralStreams => "spans several content streams".to_owned(),
            SpikeError::SharedPageContentStream => "content stream is shared".to_owned(),
            SpikeError::ObjectCtmSingular => "its own matrix has no area".to_owned(),
            SpikeError::MoveNotIsolated => "proof: something else moved".to_owned(),
            SpikeError::MoveNotProvable => "proof: could not be run".to_owned(),
            other => format!("{other}"),
        },
        pdf_session::PlanError::Page(error) => format!("{error}"),
    }
}

fn placements(graph: &PaintGraph) -> Vec<(usize, Matrix)> {
    let mut found = Vec::new();
    for (ordinal, atom) in graph.atoms.iter().enumerate() {
        let PaintAtomKind::Text(text) = &atom.kind else {
            continue;
        };
        for glyph in &text.glyphs {
            found.push((ordinal, text.state.ctm.value.multiply(glyph.matrix)));
        }
    }
    found
}

fn apart(one: Matrix, other: Matrix) -> f64 {
    [
        (one.a, other.a),
        (one.b, other.b),
        (one.c, other.c),
        (one.d, other.d),
        (one.e, other.e),
        (one.f, other.f),
    ]
    .iter()
    .map(|(had, want)| (had - want).abs() / want.abs().mul_add(1.0, 1.0))
    .fold(0.0_f64, f64::max)
}

#[derive(Clone, Copy)]
enum Gesture {
    Move(f64, f64),
    Scale(f64),
    Turn(f64),
    Size(f64),
}

impl Gesture {
    fn matrix(self) -> Matrix {
        match self {
            Self::Size(_) => Matrix::IDENTITY,
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
                let (sin, cos) = degrees.to_radians().sin_cos();
                Matrix {
                    a: cos,
                    b: sin,
                    c: -sin,
                    d: cos,
                    e: 0.0,
                    f: 0.0,
                }
            }
        }
    }

    fn about(self, middle: Option<Point>) -> FixedPoint {
        match (self, middle) {
            (Self::Move(..) | Self::Size(_), _) | (_, None) => FixedPoint::Origin,
            (_, Some(point)) => FixedPoint::At(point),
        }
    }

    fn said(self) -> String {
        match self {
            Self::Move(dx, dy) => format!("moved by ({dx}, {dy}) points"),
            Self::Scale(by) => format!("scaled by {by} about its own middle"),
            Self::Turn(degrees) => format!("turned {degrees} degrees about its own middle"),
            Self::Size(points) => format!("set to {points} points on the page"),
        }
    }
}

struct Tally {
    pages: usize,
    files: usize,
    blocks: usize,
    placed: usize,
    refused: std::collections::BTreeMap<String, usize>,
    worst_named: f64,
    worst_other: f64,
    worst_file: String,
    unproved: Vec<String>,
}

fn middle_of(graph: &PaintGraph, named: &BTreeSet<usize>) -> Option<Point> {
    let mut box_of: Option<[f64; 4]> = None;
    for ordinal in named {
        let PaintAtomKind::Text(text) = &graph.atoms.get(*ordinal)?.kind else {
            continue;
        };
        let Some(bounds) = text.outline_bounds() else {
            continue;
        };
        box_of = Some(match box_of {
            None => bounds,
            Some(had) => [
                had[0].min(bounds[0]),
                had[1].min(bounds[1]),
                had[2].max(bounds[2]),
                had[3].max(bounds[3]),
            ],
        });
    }
    box_of.map(|at| Point {
        x: f64::midpoint(at[0], at[2]),
        y: f64::midpoint(at[1], at[3]),
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "one file measured end to end: read, name its blocks, plan, commit, read back"
)]
fn measure(path: &Path, gesture: Gesture, tally: &mut Tally, verbose: bool) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let Ok(view) = pdf_session::interpret_page(&source, 0) else {
        return;
    };
    tally.pages += 1;
    let before = placements(&view.graph);

    let blocks: Vec<BTreeSet<usize>> = view
        .index
        .blocks
        .iter()
        .map(|block| {
            let mut named = BTreeSet::new();
            for line in &block.lines {
                for cluster in &view.index.lines[*line].clusters {
                    named.insert(view.index.clusters[*cluster].atom);
                }
            }
            named
        })
        .filter(|named| !named.is_empty())
        .collect();
    if blocks.is_empty() {
        return;
    }
    tally.files += 1;
    if verbose {
        println!("{}: {} blocks", path.display(), blocks.len());
    }

    for (index, named) in blocks.iter().enumerate() {
        tally.blocks += 1;
        let runs: Vec<SourceAnchor> = named
            .iter()
            .filter_map(|ordinal| Some(SourceAnchor::of(&view.graph.atoms.get(*ordinal)?.id)))
            .collect();
        let about = gesture.about(middle_of(&view.graph, named));
        let mut session = pdf_session::Session::new(source.clone(), b"");
        let planned = session.plan(&match gesture {
            Gesture::Size(points) => Command::SetTextSize {
                page_index: 0,
                runs,
                points,
            },
            _ => Command::PlaceObject {
                page_index: 0,
                target: ObjectSelection::Text(runs),
                transform: gesture.matrix(),
                about,
            },
        });
        let plan = match planned {
            Ok(plan) => plan,
            Err(error) => {
                let reason = named_or_proof(&error, path, tally);
                if verbose {
                    println!("  {index:>4}  refused: {reason}");
                }
                continue;
            }
        };
        if let Err(error) = session.apply(plan) {
            *tally.refused.entry(format!("commit: {error}")).or_default() += 1;
            continue;
        }
        let Ok(after) = pdf_session::interpret_page(session.source(), 0) else {
            *tally
                .refused
                .entry("could not be read back".to_owned())
                .or_default() += 1;
            continue;
        };
        let now = placements(&after.graph);
        if now.len() != before.len() {
            *tally
                .refused
                .entry("the page paints a different number of glyphs".to_owned())
                .or_default() += 1;
            continue;
        }
        let wanted = about.applied_to(gesture.matrix());
        let (mut worst_named, mut worst_other) = (0.0_f64, 0.0_f64);
        for ((ordinal, was), (_, is)) in before.iter().zip(&now) {
            if named.contains(ordinal) {
                if !matches!(gesture, Gesture::Size(_)) {
                    worst_named = worst_named.max(apart(*is, wanted.multiply(*was)));
                }
            } else {
                worst_other = worst_other.max(apart(*is, *was));
            }
        }
        if let Gesture::Size(points) = gesture {
            for ordinal in named {
                let Some(PaintAtomKind::Text(text)) =
                    after.graph.atoms.get(*ordinal).map(|atom| &atom.kind)
                else {
                    continue;
                };
                let Some(size) = text.size_on_page() else {
                    continue;
                };
                worst_named = worst_named.max((size - points).abs());
            }
        }
        tally.placed += 1;
        if worst_named.max(worst_other) > tally.worst_named.max(tally.worst_other) {
            tally.worst_file = path.display().to_string();
        }
        tally.worst_named = tally.worst_named.max(worst_named);
        tally.worst_other = tally.worst_other.max(worst_other);
        if verbose {
            println!(
                "  {index:>4}  {} runs, off by {worst_named:.3e} named / {worst_other:.3e} other",
                named.len()
            );
        }
    }
}

fn named_or_proof(error: &pdf_session::PlanError, path: &Path, tally: &mut Tally) -> String {
    let reason = named(error);
    if reason.starts_with("proof:") {
        let here = format!("{} ({reason})", path.display());
        if !tally.unproved.contains(&here) {
            tally.unproved.push(here);
        }
    }
    *tally.refused.entry(reason.clone()).or_default() += 1;
    reason
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let target = PathBuf::from(arguments.next().expect("a pdf file or a directory"));
    let rest: Vec<String> = arguments.collect();
    let summary = rest.iter().any(|flag| flag == "--summary");
    let number = |flag: &str, fallback: f64| {
        rest.iter()
            .position(|argument| argument == flag)
            .and_then(|at| rest.get(at + 1))
            .and_then(|value| value.parse().ok())
            .unwrap_or(fallback)
    };
    let gesture = if rest.iter().any(|flag| flag == "--points") {
        Gesture::Size(number("--points", 14.0))
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
        blocks: 0,
        placed: 0,
        refused: std::collections::BTreeMap::new(),
        worst_named: 0.0,
        worst_other: 0.0,
        worst_file: String::new(),
        unproved: Vec::new(),
    };
    let verbose = !summary && paths.len() == 1;
    for path in &paths {
        measure(path, gesture, &mut tally, verbose);
    }

    println!(
        "\n{}: {} of {} blocks, on {} of {} pages that interpret",
        gesture.said(),
        tally.placed,
        tally.blocks,
        tally.files,
        tally.pages
    );
    if tally.placed > 0 {
        let unit = if matches!(gesture, Gesture::Size(_)) {
            "points"
        } else {
            "relative"
        };
        println!(
            "  furthest from what was asked: {:.3e} named ({unit}), \
             {:.3e} for everything else  ({})",
            tally.worst_named, tally.worst_other, tally.worst_file
        );
    }
    for (reason, count) in &tally.refused {
        println!("  {count:>5} refused: {reason}");
    }
    for file in &tally.unproved {
        println!("  the proof refused in {file}");
    }
}
