use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_paint::Point;
use pdf_semantics::{HitEvidence, ObjectKind};
use pdf_session::Session;

fn name(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Text(_) => "text",
        ObjectKind::TextRun => "text run",
        ObjectKind::Image => "picture",
        ObjectKind::Form => "group",
        ObjectKind::Shading => "shading",
        ObjectKind::Path => "drawing",
    }
}

fn point_of(text: &str) -> Option<Point> {
    let (x, y) = text.split_once(',')?;
    Some(Point {
        x: x.trim().parse().ok()?,
        y: y.trim().parse().ok()?,
    })
}

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let path = PathBuf::from(arguments.next().expect("a pdf file"));
    let flags: Vec<String> = arguments.collect();
    let page = flags
        .iter()
        .position(|flag| flag == "--page")
        .and_then(|at| flags.get(at + 1))
        .and_then(|value| value.parse::<usize>().ok())
        .map_or(0, |number| number.saturating_sub(1));
    let at = flags
        .iter()
        .position(|flag| flag == "--at")
        .and_then(|at| flags.get(at + 1))
        .map(|value| point_of(value).expect("--at takes x,y"));

    let bytes = std::fs::read(&path).expect("the file reads");
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let mut session = Session::new(source, b"");

    let rows = match session.page_objects(page) {
        Ok(rows) => rows,
        Err(error) => {
            eprintln!("{}: {error}", path.display());
            return ExitCode::from(2);
        }
    };
    let inspected = session.inspect(&rows).expect("the rows were just resolved");
    println!(
        "{} page {}: {} objects at {}",
        path.display(),
        page + 1,
        rows.len(),
        session.revision()
    );

    if let Some(point) = at {
        report_stack(&mut session, page, point, rows.len());
    }
    if flags.iter().any(|flag| flag == "--panel") {
        report_panel(&inspected);
    }
    ExitCode::SUCCESS
}

fn doubt_name(doubt: pdf_semantics::HitDoubt) -> &'static str {
    match doubt {
        pdf_semantics::HitDoubt::CurvedClip => "curved clip",
        pdf_semantics::HitDoubt::MaskedImage => "undecoded mask",
        pdf_semantics::HitDoubt::StrokeWidth => "stroke outline not built",
        pdf_semantics::HitDoubt::GroupContents => "group contents not tested",
        pdf_semantics::HitDoubt::UnboundedShading => "shading has no bbox",
    }
}

fn report_stack(session: &mut Session, page: usize, point: Point, objects: usize) {
    let stack = session.hit_candidates(page, point).expect("the page");
    println!("under ({:.1}, {:.1}), front to back:", point.x, point.y);
    if stack.is_empty() {
        println!("  nothing");
    }
    for (depth, candidate) in stack.all().iter().enumerate() {
        let doubts: Vec<&str> = candidate.doubts.iter().copied().map(doubt_name).collect();
        println!(
            "  {depth:>2} object {:<4} {:<9} {:<8} {}",
            candidate.reference.object,
            name(candidate.kind),
            match candidate.evidence {
                HitEvidence::Ink => "ink",
                HitEvidence::Envelope => "envelope",
            },
            if doubts.is_empty() {
                "certain".to_string()
            } else {
                doubts.join(", ")
            }
        );
    }
    println!(
        "  {} of {objects} objects are not under this point",
        objects - stack.all().len()
    );
}

fn report_panel(inspected: &[pdf_session::Inspection]) {
    println!("panel, front to back:");
    for (row, found) in inspected.iter().enumerate() {
        println!(
            "  {row:>3} object {:<4} {:<9} {:<8} {:>2} contributions, {} shared definitions{}",
            found.reference.object,
            name(found.kind),
            if found.quad.is_some() {
                "framed"
            } else {
                "no frame"
            },
            found.members.len(),
            found.shared.len(),
            if found.dependencies_complete {
                ""
            } else {
                " (dependency walk stopped early)"
            }
        );
    }
}

#[cfg(test)]
mod tests {
    use super::point_of;

    #[test]
    fn a_point_is_two_numbers_separated_by_a_comma() {
        let point = point_of("12.5, -3").expect("a point");
        assert!((point.x - 12.5).abs() < 1e-9);
        assert!((point.y + 3.0).abs() < 1e-9);
    }

    #[test]
    fn anything_else_is_not_a_point() {
        assert!(point_of("12.5").is_none());
        assert!(point_of("a,b").is_none());
        assert!(point_of("").is_none());
    }
}
