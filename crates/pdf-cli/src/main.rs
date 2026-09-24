#![forbid(unsafe_code)]

use std::error::Error;
use std::path::Path;

use pdf_bytes::{ByteStore, SourceId};
use pdf_cli::{
    InspectLimits, Inspection, compare_paint_graphs, inspect_page_paint_strict, inspect_recovering,
    inspect_strict, render_page_strict,
};
use pdf_content::{PageContentLimits, load_page_program_strict};
use pdf_edit::Document;
use pdf_edit::spike_move_text::move_last_text_run_with_fonts;
use pdf_render::{Canvas, DeviceTransform, RenderLimits};
use pdf_syntax::XrefLimits;

fn main() {
    if let Err(error) = run() {
        eprintln!("pdf-cli: {error}");
        std::process::exit(1);
    }
}

struct Invocation {
    command: std::ffi::OsString,
    path: std::ffi::OsString,
    output: Option<std::ffi::OsString>,
    scale: Option<f64>,
    region: Option<[f64; 4]>,
    page: Option<usize>,
    recover: bool,
}

fn parse_invocation() -> Result<Invocation, Box<dyn Error>> {
    let mut arguments = std::env::args_os();
    let _program = arguments.next();
    let Some(command) = arguments.next() else {
        return Err(usage().into());
    };

    let mut path = None;
    let mut output = None;
    let mut scale = None;
    let mut region = None;
    let mut page = None;
    let mut recover = false;
    let mut arguments = arguments.peekable();
    while let Some(argument) = arguments.next() {
        if argument == "--recover" {
            if recover {
                return Err(usage().into());
            }
            recover = true;
        } else if argument == "--scale" {
            let value = arguments.next().ok_or_else(usage)?;
            let value = value.to_str().ok_or_else(usage)?.parse::<f64>()?;
            if scale.replace(value).is_some() {
                return Err(usage().into());
            }
        } else if argument == "--page" {
            let value = arguments.next().ok_or_else(usage)?;
            let value = value.to_str().ok_or_else(usage)?.parse::<usize>()?;
            if value == 0 {
                return Err("--page counts from 1".into());
            }
            if page.replace(value - 1).is_some() {
                return Err(usage().into());
            }
        } else if argument == "--region" {
            let value = arguments.next().ok_or_else(usage)?;
            let value = value.to_str().ok_or_else(usage)?;
            let mut numbers = value.split(',').map(str::parse::<f64>);
            let (Some(Ok(x0)), Some(Ok(y0)), Some(Ok(x1)), Some(Ok(y1)), None) = (
                numbers.next(),
                numbers.next(),
                numbers.next(),
                numbers.next(),
                numbers.next(),
            ) else {
                return Err("--region takes x0,y0,x1,y1 in user space".into());
            };
            if region.replace([x0, y0, x1, y1]).is_some() {
                return Err(usage().into());
            }
        } else if path.is_none() {
            path = Some(argument);
        } else if output.is_none() {
            output = Some(argument);
        } else {
            return Err(usage().into());
        }
    }
    let Some(path) = path else {
        return Err(usage().into());
    };
    Ok(Invocation {
        command,
        path,
        output,
        scale,
        region,
        page,
        recover,
    })
}

fn run() -> Result<(), Box<dyn Error>> {
    let Invocation {
        command,
        path,
        output,
        scale,
        region,
        page,
        recover,
    } = parse_invocation()?;

    let bytes = std::fs::read(&path)?;
    let source = ByteStore::owning(SourceId::new(1), bytes);
    if command == "inspect" {
        let report = if recover {
            inspect_recovering(&source, InspectLimits::default())?
        } else {
            inspect_strict(&source, InspectLimits::default())?
        };
        print_report(Path::new(&path), &report);
    } else if command == "inspect-page" {
        if recover {
            return Err(usage().into());
        }
        let report = inspect_page_paint_strict(&source, page.unwrap_or(0))?;
        println!("file: {}", Path::new(&path).display());
        println!(
            "page: {} {} R",
            report.page.object_number(),
            report.page.generation()
        );
        report_page(&report);
    } else if command == "inspect-clusters" {
        if recover {
            return Err(usage().into());
        }
        report_clusters(Path::new(&path), &source)?;
    } else if command == "render-page" {
        if recover {
            return Err(usage().into());
        }
        let Some(output) = output else {
            return Err(usage().into());
        };
        render_command(
            &source,
            Path::new(&path),
            Path::new(&output),
            scale.unwrap_or(1.0),
            region,
            page.unwrap_or(0),
        )?;
    } else if command == "render-sweep" {
        if recover || output.is_some() || region.is_some() || page.is_some() {
            return Err(usage().into());
        }
        render_sweep_command(&source, Path::new(&path), scale.unwrap_or(1.0))?;
    } else if command == "spike-move-cluster"
        || command == "spike-move-text"
        || command == "spike-delete-cluster"
        || command == "spike-delete-selection"
    {
        if recover {
            return Err(usage().into());
        }
        let spike = if command == "spike-move-cluster" {
            spike_cluster_command
        } else if command == "spike-delete-cluster" {
            spike_delete_command
        } else if command == "spike-delete-selection" {
            spike_delete_selection_command
        } else {
            spike_command
        };
        spike(
            &source,
            Path::new(&path),
            scale.unwrap_or(10.0),
            output.as_ref().map(Path::new),
        )?;
    } else if command == "links" {
        if recover || output.is_some() || region.is_some() {
            return Err(usage().into());
        }
        links_command(&source, Path::new(&path), page)?;
    } else if command == "extract-images" {
        if recover || region.is_some() {
            return Err(usage().into());
        }
        let Some(output) = output else {
            return Err(usage().into());
        };
        extract_images_command(&source, Path::new(&path), Path::new(&output), page)?;
    } else if command == "verify-empty-round-trip" {
        if recover {
            return Err(usage().into());
        }
        verify_empty_round_trip(Path::new(&path), source)?;
    } else {
        return Err(usage().into());
    }
    Ok(())
}

fn extract_images_command(
    source: &ByteStore,
    path: &Path,
    output: &Path,
    page: Option<usize>,
) -> Result<(), Box<dyn Error>> {
    const MAX_DECODED_BYTES: usize = 256 * 1024 * 1024;

    std::fs::create_dir_all(output)?;
    let limits = PageContentLimits::default();
    let pages = match page {
        Some(index) => index..index + 1,
        None => 0..pdf_content::count_pages_strict(source, limits)?,
    };
    println!("file: {}", path.display());
    let mut written = 0usize;
    for index in pages {
        let program = match load_page_program_strict(source, index, limits) {
            Ok(program) => program,
            Err(error) => {
                println!("page {}: not loaded: {error}", index + 1);
                continue;
            }
        };
        for entry in program.resources.xobjects() {
            let Some(reference) = entry.reference() else {
                continue;
            };
            if entry.form().is_some() {
                continue;
            }
            let image = match entry.image(reference, MAX_DECODED_BYTES) {
                Ok(image) => image,
                Err(error) => {
                    println!("page {}: {reference:?}: not loaded: {error}", index + 1);
                    continue;
                }
            };
            let extension = match image.codec {
                Some(pdf_syntax::ImageCodec::Dct) => "jpg",
                Some(pdf_syntax::ImageCodec::Jpx) => "jpx",
                Some(pdf_syntax::ImageCodec::Jbig2) => "jbig2",
                Some(pdf_syntax::ImageCodec::CcittFax) => "ccitt",
                Some(pdf_syntax::ImageCodec::Crypt) => "crypt",
                None => "samples",
            };
            let name = format!(
                "page-{:05}-object-{}-{}.{extension}",
                index + 1,
                reference.object_number(),
                reference.generation()
            );
            std::fs::write(output.join(&name), image.bytes.as_ref())?;
            println!(
                "{name}: {} bytes, codec {}",
                image.bytes.len(),
                image.codec.map_or("(none)", pdf_syntax::ImageCodec::name)
            );
            written += 1;
        }
    }
    println!("wrote: {written} images to {}", output.display());
    Ok(())
}

fn links_command(
    source: &ByteStore,
    path: &Path,
    page: Option<usize>,
) -> Result<(), Box<dyn Error>> {
    let limits = PageContentLimits::default();
    let (resolver, repairs) = pdf_content::open_link_resolver_tolerating_damage(
        source,
        limits,
        pdf_content::RecoverLimits::default(),
        b"",
    )?
    .into_parts();
    println!("file: {}", path.display());
    for repair in &repairs {
        println!("recovered: {}", repair.kind());
    }
    let pages = page.map_or(0..resolver.page_count(), |index| index..index + 1);
    for index in pages {
        let found = match resolver.links(index) {
            Ok(found) => found,
            Err(error) => {
                println!("page {}: not read: {error}", index + 1);
                continue;
            }
        };
        for link in &found.links {
            println!("page {}: {}", index + 1, describe_link(link));
        }
        for unreadable in &found.unreadable {
            println!("page {}: unreadable: {}", index + 1, unreadable.error);
        }
    }
    Ok(())
}

fn describe_link(link: &pdf_content::Link) -> String {
    use pdf_content::LinkAction;
    let text = |bytes: &[u8]| {
        bytes
            .iter()
            .map(|byte| char::from(*byte))
            .collect::<String>()
    };
    let file = |file: &Option<Vec<u8>>| text(file.as_deref().unwrap_or_default());
    let mut parts = Vec::new();
    let mut destination = link.destination;
    match &link.action {
        Some(LinkAction::Uri(uri)) => parts.push(format!("action=uri uri={}", text(uri))),
        Some(LinkAction::GoTo(found)) => {
            parts.push("action=goto".to_owned());
            destination = destination.or(*found);
        }
        Some(LinkAction::RemoteGoTo {
            file: name,
            destination: found,
        }) => {
            parts.push(format!("action=remote-goto file={}", file(name)));
            destination = destination.or(*found);
        }
        Some(LinkAction::EmbeddedGoTo {
            file: name,
            destination: found,
        }) => {
            parts.push(format!("action=embedded-goto file={}", file(name)));
            destination = destination.or(*found);
        }
        Some(LinkAction::Launch { file: name }) => {
            parts.push(format!("action=launch file={}", file(name)));
        }
        Some(LinkAction::Named(name)) => parts.push(format!("action=named name={}", text(name))),
        Some(LinkAction::Other(kind)) => parts.push(format!("action=other kind={}", text(kind))),
        None => {}
    }
    if let Some(destination) = destination {
        let page = destination
            .page
            .map_or_else(|| "-1".to_owned(), |page| page.to_string());
        parts.push(format!(
            "page={page} view={}",
            describe_view(destination.view)
        ));
    }
    if link.action.is_none() && link.destination.is_none() {
        parts.push("nothing".to_owned());
    }
    let [x0, y0, x1, y1] = link.rect;
    parts.push(format!("rect={x0},{y0},{x1},{y1}"));
    parts.push(format!("quads={}", link.quad_points.len()));
    parts.join(" ")
}

fn describe_view(view: pdf_content::View) -> String {
    use pdf_content::View;
    let value = |number: Option<f64>| number.map_or_else(|| "null".to_owned(), |n| n.to_string());
    match view {
        View::Xyz { left, top, zoom } => {
            format!("XYZ {} {} {}", value(left), value(top), value(zoom))
        }
        View::Fit => "Fit".to_owned(),
        View::FitB => "FitB".to_owned(),
        View::FitH { top } => format!("FitH {}", value(top)),
        View::FitBH { top } => format!("FitBH {}", value(top)),
        View::FitV { left } => format!("FitV {}", value(left)),
        View::FitBV { left } => format!("FitBV {}", value(left)),
        View::FitR {
            rect: [x0, y0, x1, y1],
        } => format!("FitR {x0} {y0} {x1} {y1}"),
        View::Unknown => "unknown".to_owned(),
    }
}

fn spike_command(
    source: &ByteStore,
    path: &Path,
    offset: f64,
    output: Option<&Path>,
) -> Result<(), Box<dyn Error>> {
    let moved =
        move_last_text_run_with_fonts(source, 0, offset, 0.0, b"", pdf_cli::font_provider())?;
    if let Some(output) = output {
        std::fs::write(output, moved.source.to_vec())?;
        println!("wrote: {}", output.display());
    }
    println!("file: {}", path.display());
    println!("moved atom ordinal: {}", moved.atom_ordinal);
    println!(
        "content stream: {} {} R",
        moved.content_stream.object_number(),
        moved.content_stream.generation()
    );
    println!(
        "bytes: {} -> {} (appended {})",
        moved.original_length,
        moved.source.len(),
        moved.source.len() - moved.original_length
    );

    let mut tally = Tally::default();
    let check = &mut tally;

    let prefix_intact = moved.source.get(0..moved.original_length) == Some(source.as_bytes());
    check.held(
        "original bytes unchanged",
        prefix_intact,
        &format!("first {} bytes compared", moved.original_length),
    );

    let reopened = inspect_page_paint_strict(&moved.source, 0);
    let before = inspect_page_paint_strict(source, 0)?;
    match &reopened {
        Ok(after) => {
            check.held(
                "reopens strictly",
                true,
                &format!(
                    "{} atoms, {} operations",
                    after.paint_atoms, after.operations
                ),
            );
            check.held(
                "atom count unchanged",
                after.paint_atoms == before.paint_atoms,
                &format!("{} -> {}", before.paint_atoms, after.paint_atoms),
            );
        }
        Err(error) => check.held("reopens strictly", false, &format!("{error}")),
    }

    match compare_paint_graphs(source, &moved.source, moved.atom_ordinal, offset) {
        Ok(report) => {
            let detail = report.first_difference.as_ref().map_or_else(
                || format!("{} unrelated atoms compared", report.compared),
                |(index, before, after)| {
                    format!(
                        "{} compared; atom {index} differs\n      before: {}\n      after:  {}",
                        report.compared,
                        &before[..before.len().min(220)],
                        &after[..after.len().min(220)]
                    )
                },
            );
            check.held(
                "only the selected atom changed",
                report.other_atoms_identical,
                &detail,
            );
            check.held(
                "moved by exactly the requested offset",
                report.offset_exact,
                &report.offset_detail,
            );
        }
        Err(error) => check.held("paint graph comparison", false, &error),
    }

    let (outcome, detail) = declared_region_check(source, &moved)?;
    tally.record(
        "no pixels changed outside the declared region",
        outcome,
        &detail,
    );

    let (outcome, detail) = undo_check(
        source,
        &pdf_edit::Command::MoveTextRun {
            page_index: 0,
            selection: pdf_edit::TextRunSelection::Last,
            dx: offset,
            dy: 0.0,
        },
    );
    tally.record("undo and redo walk the same command", outcome, &detail);

    verdict(&tally, "editing spike failed an invariant")
}

fn verdict(tally: &Tally, failure: &'static str) -> Result<(), Box<dyn Error>> {
    let (failures, unproven) = (tally.failures, tally.unproven);
    println!(
        "spike: {}",
        if failures > 0 {
            "at least one invariant failed"
        } else if unproven > 0 {
            "every checked invariant held, but the pixel gate proved nothing"
        } else {
            "every checked invariant held"
        }
    );
    if failures > 0 {
        return Err(failure.into());
    }
    Ok(())
}

#[derive(Debug, Default)]
struct Tally {
    failures: usize,
    unproven: usize,
}

impl Tally {
    fn record(&mut self, name: &str, outcome: Outcome, detail: &str) {
        println!("  {} {name}: {detail}", outcome.label());
        match outcome {
            Outcome::Held => {}
            Outcome::Unproven => self.unproven += 1,
            Outcome::Violated => self.failures += 1,
        }
    }

    fn held(&mut self, name: &str, passed: bool, detail: &str) {
        self.record(
            name,
            if passed {
                Outcome::Held
            } else {
                Outcome::Violated
            },
            detail,
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Outcome {
    Held,
    Unproven,
    Violated,
}

impl Outcome {
    const fn label(self) -> &'static str {
        match self {
            Self::Held => "PASS",
            Self::Unproven => "NOTE",
            Self::Violated => "FAIL",
        }
    }
}

fn spike_cluster_command(
    source: &ByteStore,
    path: &Path,
    offset: f64,
    output: Option<&Path>,
) -> Result<(), Box<dyn Error>> {
    let moved = pdf_edit::spike_move_text::move_last_cluster_with_fonts(
        source,
        0,
        offset,
        0.0,
        b"",
        pdf_cli::font_provider(),
    )?;
    check_cluster_edit(source, path, &moved, output)
}

fn spike_delete_command(
    source: &ByteStore,
    path: &Path,
    _offset: f64,
    output: Option<&Path>,
) -> Result<(), Box<dyn Error>> {
    let moved = pdf_edit::spike_move_text::delete_last_cluster_with_fonts(
        source,
        0,
        b"",
        pdf_cli::font_provider(),
    )?;
    check_cluster_edit(source, path, &moved, output)
}

const SELECTION_CLUSTERS: usize = 3;

fn spike_delete_selection_command(
    source: &ByteStore,
    path: &Path,
    _offset: f64,
    output: Option<&Path>,
) -> Result<(), Box<dyn Error>> {
    let deleted = pdf_edit::spike_move_text::delete_last_row_selection_with_fonts(
        source,
        0,
        SELECTION_CLUSTERS,
        b"",
        pdf_cli::font_provider(),
    )?;
    check_cluster_edit(source, path, &deleted, output)
}

fn check_cluster_edit(
    source: &ByteStore,
    path: &Path,
    moved: &pdf_edit::spike_move_text::MovedTextRun,
    output: Option<&Path>,
) -> Result<(), Box<dyn Error>> {
    if let Some(output) = output {
        std::fs::write(output, moved.source.to_vec())?;
        println!("wrote: {}", output.display());
    }
    println!("file: {}", path.display());
    println!(
        "bytes: {} -> {} (appended {})",
        moved.original_length,
        moved.source.len(),
        moved.source.len() - moved.original_length
    );

    let mut tally = Tally::default();
    let check = &mut tally;
    check.held(
        "original bytes unchanged",
        moved.source.get(0..moved.original_length) == Some(source.as_bytes()),
        &format!("first {} bytes compared", moved.original_length),
    );

    let before = page_glyph_placements(source);
    let after = page_glyph_placements(&moved.source);
    match (before, after) {
        (Ok(before), Ok(after)) => {
            let survivors = before.iter().filter(|one| after.contains(one)).count();
            let (left, arrived) = (before.len() - survivors, after.len() - survivors);
            let named = moved.edited_glyphs;
            check.held(
                "exactly the named glyphs left their old places",
                left == named,
                &format!(
                    "{left} of {} placements changed, {named} named",
                    before.len()
                ),
            );
            check.held(
                "nothing arrived that the edit did not name",
                arrived == named || arrived == 0,
                &format!("{arrived} new placements, {named} named"),
            );
            check.held(
                "every other glyph is exactly where it was",
                survivors == before.len() - named,
                &format!("{survivors} of {} placements untouched", before.len()),
            );
        }
        (Err(error), _) | (_, Err(error)) => {
            check.held("the page reopens and interprets", false, &error);
        }
    }

    let (outcome, detail) = declared_region_check(source, moved)?;
    tally.record(
        "no pixels changed outside the declared region",
        outcome,
        &detail,
    );

    let (outcome, detail) = undo_check(source, &moved.command);
    tally.record("undo and redo walk the same command", outcome, &detail);

    verdict(&tally, "cluster editing spike failed an invariant")
}

fn page_glyph_placements(source: &ByteStore) -> Result<Vec<String>, String> {
    pdf_cli::page_glyph_placements_strict(source, 0).map_err(|error| error.to_string())
}

fn first_difference(actual: &[String], expected: &[String]) -> String {
    let Some(at) = actual
        .iter()
        .zip(expected.iter())
        .position(|(one, other)| one != other)
    else {
        return format!(
            "{} atoms against the {} it had",
            actual.len(),
            expected.len()
        );
    };
    let shorten = |text: &String| text.chars().take(220).collect::<String>();
    format!(
        "atom {at} of {} differs\n      was:  {}\n      now:  {}",
        actual.len(),
        shorten(&expected[at]),
        shorten(&actual[at])
    )
}

fn undo_check(source: &ByteStore, command: &pdf_edit::Command) -> (Outcome, String) {
    let document = match pdf_edit::Document::open_strict(source.clone(), XrefLimits::default()) {
        Ok(document) => document,
        Err(error) => {
            return (
                Outcome::Violated,
                format!("document does not reopen: {error}"),
            );
        }
    };
    let plan =
        match document
            .begin_transaction()
            .plan_with_fonts(command, b"", pdf_cli::font_provider())
        {
            Ok(plan) => plan,
            Err(error) => {
                return (
                    Outcome::Violated,
                    format!("the edit no longer plans: {error}"),
                );
            }
        };

    let signatures = |bytes: &ByteStore| {
        inspect_page_paint_strict(bytes, 0)
            .map(|page| page.paint_signatures)
            .map_err(|error| format!("{error}"))
    };
    let at_rest = match signatures(source) {
        Ok(signatures) => signatures,
        Err(error) => return (Outcome::Violated, format!("page does not reopen: {error}")),
    };

    let mut history = pdf_edit::History::new(source.clone(), b"");
    if let Err(error) = history.apply(plan) {
        return (
            Outcome::Violated,
            format!("the plan does not apply: {error}"),
        );
    }
    let moved = match signatures(history.source()) {
        Ok(signatures) => signatures,
        Err(error) => {
            return (
                Outcome::Violated,
                format!("edited page does not reopen: {error}"),
            );
        }
    };
    if moved == at_rest {
        return (
            Outcome::Unproven,
            "the edit changed no paint, so walking a history over it proves nothing".to_owned(),
        );
    }

    for (step, expected) in [("undo", &at_rest), ("redo", &moved), ("undo", &at_rest)] {
        let walked = if step == "undo" {
            history.undo()
        } else {
            history.redo()
        };
        match walked {
            Ok(true) => {}
            Ok(false) => return (Outcome::Violated, format!("{step} had nothing to walk")),
            Err(error) => return (Outcome::Violated, format!("{step} failed: {error}")),
        }
        match signatures(history.source()) {
            Ok(actual) if actual == *expected => {}
            Ok(actual) => {
                return (
                    Outcome::Violated,
                    format!(
                        "after {step} the page paints something else: {}",
                        first_difference(&actual, expected)
                    ),
                );
            }
            Err(error) => {
                return (
                    Outcome::Violated,
                    format!("after {step} the page does not reopen: {error}"),
                );
            }
        }
    }
    (
        Outcome::Held,
        format!(
            "{} atoms identical through apply, undo, redo, undo; the file grew {} bytes rather than shrinking",
            at_rest.len(),
            history.source().len() - source.len()
        ),
    )
}

fn declared_region_check(
    source: &ByteStore,
    moved: &pdf_edit::spike_move_text::MovedTextRun,
) -> Result<(Outcome, String), Box<dyn Error>> {
    let Some(changed) = changed_region(source, &moved.source)? else {
        return Ok((
            Outcome::Unproven,
            "no pixel changed at all, so this gate saw nothing to bound; the edit \
             is real in the paint graph but has no visible effect on this page"
                .to_owned(),
        ));
    };
    let Some(declared) = moved
        .declared_region
        .and_then(|region| device_region(source, region))
    else {
        return Ok((
            Outcome::Violated,
            format!("pixels changed at {changed:?} but the edit declared no region"),
        ));
    };
    if region_contains(declared, changed) {
        Ok((
            Outcome::Held,
            format!("changed {changed:?} within declared {declared:?}"),
        ))
    } else {
        Ok((
            Outcome::Violated,
            format!("changed {changed:?} outside declared {declared:?}"),
        ))
    }
}

fn changed_region(
    before: &ByteStore,
    after: &ByteStore,
) -> Result<Option<[u32; 4]>, Box<dyn Error>> {
    let (original, _) = render_page_strict(before, 0, 1.0)?;
    let (edited, _) = render_page_strict(after, 0, 1.0)?;
    changed_canvas_region(&original, &edited).map_err(Into::into)
}

fn changed_canvas_region(
    original: &Canvas,
    edited: &Canvas,
) -> Result<Option<[u32; 4]>, &'static str> {
    if original.width != edited.width || original.height != edited.height {
        return Err("page raster dimensions changed");
    }
    let mut region: Option<[u32; 4]> = None;
    for y in 0..original.height {
        for x in 0..original.width {
            let (left, right) = (original.pixel(x, y), edited.pixel(x, y));
            if left
                .iter()
                .zip(right.iter())
                .all(|(left, right)| left.total_cmp(right).is_eq())
            {
                continue;
            }
            region = Some(match region {
                None => [x, y, x + 1, y + 1],
                Some([x0, y0, x1, y1]) => [x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1)],
            });
        }
    }
    Ok(region)
}

const fn region_contains(declared: [u32; 4], changed: [u32; 4]) -> bool {
    changed[0] >= declared[0]
        && changed[1] >= declared[1]
        && changed[2] <= declared[2]
        && changed[3] <= declared[3]
}

fn device_region(source: &ByteStore, region: [f64; 4]) -> Option<[u32; 4]> {
    let program = load_page_program_strict(source, 0, PageContentLimits::default()).ok()?;
    let device = DeviceTransform::for_page(&program.geometry, 1.0, RenderLimits::default()).ok()?;
    let corners = [
        apply(device.matrix, [region[0], region[1]]),
        apply(device.matrix, [region[2], region[1]]),
        apply(device.matrix, [region[0], region[3]]),
        apply(device.matrix, [region[2], region[3]]),
    ];
    let low_x = corners
        .iter()
        .map(|point| point[0])
        .fold(f64::INFINITY, f64::min);
    let high_x = corners
        .iter()
        .map(|point| point[0])
        .fold(f64::NEG_INFINITY, f64::max);
    let low_y = corners
        .iter()
        .map(|point| point[1])
        .fold(f64::INFINITY, f64::min);
    let high_y = corners
        .iter()
        .map(|point| point[1])
        .fold(f64::NEG_INFINITY, f64::max);
    Some([
        pixel_floor(low_x - 1.0),
        pixel_floor(low_y - 1.0),
        pixel_ceil(high_x + 1.0, device.width),
        pixel_ceil(high_y + 1.0, device.height),
    ])
}

fn apply(matrix: pdf_paint::Matrix, point: [f64; 2]) -> [f64; 2] {
    [
        matrix
            .a
            .mul_add(point[0], matrix.c.mul_add(point[1], matrix.e)),
        matrix
            .b
            .mul_add(point[0], matrix.d.mul_add(point[1], matrix.f)),
    ]
}

fn pixel_floor(value: f64) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    let mut low = 0_u32;
    let mut high = u32::MAX;
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        if f64::from(middle) <= value {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    low
}

fn pixel_ceil(value: f64, limit: u32) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    let floor = pixel_floor(value);
    if f64::from(floor) < value {
        floor.saturating_add(1).min(limit)
    } else {
        floor.min(limit)
    }
}

fn render_command(
    source: &ByteStore,
    path: &Path,
    output: &Path,
    scale: f64,
    region: Option<[f64; 4]>,
    page_index: usize,
) -> Result<(), Box<dyn Error>> {
    let read = std::time::Instant::now();
    let page = pdf_session::interpret_page_for_display(
        source,
        page_index,
        b"",
        None,
        pdf_cli::font_provider(),
    )?;
    let read = read.elapsed();
    let drawn = std::time::Instant::now();
    let (canvas, report) = match region {
        Some(region) => pdf_cli::render_region_view(&page, scale, region)?
            .ok_or("that region touches no pixel of this page")?,
        None => pdf_cli::render_page_view(&page, scale)?,
    };
    let drawn = drawn.elapsed();
    let extension = output
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_ascii_lowercase);
    let image = match extension.as_deref() {
        Some("bmp") => canvas.to_bmp(),
        Some("ppm") => canvas.to_ppm(),
        _ => return Err("render output must end in .bmp or .ppm".into()),
    };
    std::fs::write(output, image)?;
    println!("file: {}", path.display());
    println!("page: {}", page_index + 1);
    println!("output: {}", output.display());
    if let Some([x0, y0, x1, y1]) = region {
        println!("region: {x0},{y0},{x1},{y1} in user space");
    }
    println!(
        "pixels: {}x{} at ({}, {}) at scale {scale}",
        canvas.width, canvas.height, canvas.origin.0, canvas.origin.1
    );
    println!(
        "read: {:.3} s   drew: {:.3} s",
        read.as_secs_f64(),
        drawn.as_secs_f64()
    );
    println!("atoms visited: {}", report.visited);
    println!("atoms drawn: {}", report.drawn);
    let empty = report
        .visited
        .saturating_sub(report.drawn)
        .saturating_sub(report.skipped.len());
    if empty > 0 {
        println!("atoms that painted nothing: {empty}");
    }
    println!("ink fraction: {:.4}", canvas.ink_fraction());
    if report.skipped.is_empty() {
        println!("not drawn: none");
    } else {
        for (reason, count) in report.skipped_counts() {
            println!("not drawn: {count} x {reason}");
        }
    }
    if report.approximations.is_empty() {
        println!("approximations: none");
    } else {
        for approximation in &report.approximations {
            println!("approximation: {approximation}");
        }
    }
    print_what_the_page_lost(&page);
    println!(
        "faithful: {}",
        if report.is_faithful() && page.annotations_are_faithful() {
            "yes"
        } else {
            "no"
        }
    );
    Ok(())
}

fn print_what_the_page_lost(page: &pdf_session::PageView) {
    let mut annotations: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();
    for annotation in &page.annotations {
        *annotations
            .entry(annotation.outcome.to_string())
            .or_default() += 1;
    }
    for (outcome, count) in annotations {
        println!("annotations: {count} x {outcome}");
    }
    let mut dropped: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let annotation_graphs = page.annotations.iter().map(|annotation| &annotation.graph);
    for error in std::iter::once(&page.graph)
        .chain(annotation_graphs.clone())
        .flat_map(|graph| &graph.skipped)
    {
        *dropped.entry(error.kind().to_string()).or_default() += 1;
    }
    for (error, count) in dropped {
        println!("skipped: {count} x {error}");
    }
    let mut repairs: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for repair in &page.repairs {
        *repairs.entry(repair.kind().to_string()).or_default() += 1;
    }
    for repair in std::iter::once(&page.graph)
        .chain(annotation_graphs)
        .flat_map(|graph| &graph.repairs)
    {
        *repairs.entry(repair.kind.to_string()).or_default() += 1;
    }
    for (repair, count) in repairs {
        println!("recovered: {count} x {repair}");
    }
}

fn render_sweep_command(source: &ByteStore, path: &Path, scale: f64) -> Result<(), Box<dyn Error>> {
    use std::collections::BTreeMap;

    let session = pdf_session::Session::with_fonts(source.clone(), b"", pdf_cli::font_provider());
    let count = session.page_count()?;
    println!("file: {}", path.display());
    println!("pages: {count}");
    println!("scale: {scale}");

    let started = std::time::Instant::now();
    let mut interpreted = 0_usize;
    let mut complete = 0_usize;
    let mut exact = 0_usize;
    let mut visited = 0_usize;
    let mut drawn = 0_usize;
    let mut skipped_atoms = 0_usize;
    let mut skipped_by_reason: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut approximated_pages: BTreeMap<String, usize> = BTreeMap::new();
    let mut refusals: BTreeMap<String, usize> = BTreeMap::new();

    for index in 0..count {
        let (_, report) = match pdf_cli::render_page_strict(source, index, scale) {
            Ok(rendered) => rendered,
            Err(error) => {
                let reason = error.to_string();
                println!("page {}: refused: {reason}", index + 1);
                *refusals.entry(reason).or_default() += 1;
                continue;
            }
        };
        interpreted += 1;
        visited += report.visited;
        drawn += report.drawn;
        skipped_atoms += report.skipped.len();

        let counts = report.skipped_counts();
        if counts.is_empty() {
            complete += 1;
            if report.approximations.is_empty() {
                exact += 1;
            }
        }
        for (reason, atoms) in &counts {
            let entry = skipped_by_reason
                .entry(reason.to_string())
                .or_insert((0, 0));
            entry.0 += 1;
            entry.1 += atoms;
        }
        for approximation in &report.approximations {
            *approximated_pages
                .entry(approximation.to_string())
                .or_default() += 1;
        }

        if !counts.is_empty() {
            let named = counts
                .iter()
                .map(|(reason, atoms)| format!("{atoms} x {reason}"))
                .collect::<Vec<_>>()
                .join("; ");
            println!(
                "page {}: not drawn: {named} (of {} atoms)",
                index + 1,
                report.visited
            );
        }
    }

    let elapsed = started.elapsed();
    println!("--- document ---");
    println!("pages: {count}");
    println!("pages rasterised: {interpreted}");
    println!("pages refused: {}", count - interpreted);
    println!("pages that drew every atom: {complete}");
    println!("pages that drew every atom and approximated nothing: {exact}");
    println!("atoms visited: {visited}");
    println!("atoms drawn: {drawn}");
    println!("atoms not drawn: {skipped_atoms}");
    if skipped_by_reason.is_empty() {
        println!("not drawn: none");
    } else {
        for (reason, (pages, atoms)) in &skipped_by_reason {
            println!("not drawn: {atoms} atoms on {pages} pages x {reason}");
        }
    }
    if approximated_pages.is_empty() {
        println!("approximations: none");
    } else {
        for (approximation, pages) in &approximated_pages {
            println!("approximation: {pages} pages x {approximation}");
        }
    }
    for (reason, pages) in &refusals {
        println!("refused: {pages} pages x {reason}");
    }
    println!("swept in: {:.3} s", elapsed.as_secs_f64());
    Ok(())
}

const fn usage() -> &'static str {
    "usage: pdf-cli inspect [--recover] <file.pdf>\n       pdf-cli inspect-page [--page N] <file.pdf>\n       pdf-cli inspect-clusters <file.pdf>\n       pdf-cli render-page [--scale N] [--page N] [--region x0,y0,x1,y1] <file.pdf> <out.bmp|out.ppm>\n       pdf-cli render-sweep [--scale N] <file.pdf>\n       pdf-cli spike-move-text [--scale DX] <file.pdf>\n       pdf-cli spike-move-cluster [--scale DX] <file.pdf>\n       pdf-cli spike-delete-cluster <file.pdf>\n       pdf-cli spike-delete-selection <file.pdf>\n       pdf-cli links [--page N] <file.pdf>\n       pdf-cli extract-images [--page N] <file.pdf> <out-dir>\n       pdf-cli verify-empty-round-trip <file.pdf>"
}

fn report_page(report: &pdf_cli::PagePaintInspection) {
    println!("content streams: {}", report.content_streams);
    println!("decoded bytes: {}", report.decoded_bytes);
    println!("operations: {}", report.operations);
    println!("paint atoms: {}", report.paint_atoms);
}

fn report_clusters(path: &Path, source: &ByteStore) -> Result<(), Box<dyn Error>> {
    let report = pdf_cli::inspect_page_clusters_strict(source, 0)?;
    println!("file: {}", path.display());
    println!("text runs: {}", report.text_runs);
    println!("clusters: {}", report.clusters);
    println!("stacked clusters: {}", report.stacked_clusters);
    println!("marks: {}", report.marks);
    println!(
        "runs with no glyph positions: {}",
        report.runs_without_glyphs
    );
    println!("marks by declared width: {}", report.marks_by_width);
    println!("marks by position: {}", report.marks_by_position);
    println!(
        "clusters reordered by paint: {}",
        report.clusters_reordered_by_paint
    );
    println!("lines: {}", report.lines);
    println!("longest line: {}", report.longest_line);
    println!("lines ended by a gap: {}", report.lines_split_by_gap);
    println!("blocks: {}", report.blocks);
    println!("largest block: {} lines", report.largest_block);
    println!("blocks of one line: {}", report.blocks_of_one_line);
    println!(
        "lines kept apart by column: {}",
        report.lines_split_by_column
    );
    Ok(())
}

fn verify_empty_round_trip(path: &Path, source: ByteStore) -> Result<(), Box<dyn Error>> {
    let source_id = source.id();
    let source_pointer = source.as_bytes().as_ptr();
    let source_len = source.len();
    let document = Document::open_strict(source, XrefLimits::default())?;
    let outcome = document.begin_transaction().commit(b"")?;
    if outcome.revision_created()
        || outcome.source().id() != source_id
        || outcome.source().as_bytes().as_ptr() != source_pointer
        || outcome.source().len() != source_len
    {
        return Err("empty transaction did not retain the original byte store".into());
    }
    println!("file: {}", path.display());
    println!("empty round trip: identical byte store ({source_len} bytes)");
    Ok(())
}

fn print_report(path: &Path, report: &Inspection) {
    println!("file: {}", path.display());
    println!("bytes: {}", report.byte_len);
    println!("version: {}", report.version);
    println!("objects from: {}", report.object_source);
    match report.startxref {
        Some(startxref) => println!("startxref: {startxref}"),
        None => println!("startxref: none (no trustworthy revision chain)"),
    }
    println!("revisions: {}", report.revisions.len());
    for (index, revision) in report.revisions.iter().enumerate() {
        println!(
            "  {index}: {} at {} ({} entries)",
            revision.kind, revision.byte_offset, revision.entries
        );
    }
    println!(
        "objects: {} active ({} direct, {} compressed), {} free",
        report.objects.active(),
        report.objects.direct,
        report.objects.compressed,
        report.objects.free
    );
    println!("repairs: {}", report.repairs.len());
    for repair in &report.repairs {
        println!("  {repair}");
    }
    println!(
        "encryption: {}",
        match report.encrypted {
            Some(true) => "present",
            Some(false) => "none",
            None => "unknown (no trailer to read /Encrypt from)",
        }
    );
    if report.signature_scan_complete {
        println!("signatures: {}", report.signatures_found);
    } else {
        println!(
            "signatures: at least {} (scan incomplete after {} objects)",
            report.signatures_found, report.signature_objects_inspected
        );
    }
    println!("unresolved objects: {}", report.unresolved_objects);
    for diagnostic in &report.object_diagnostics {
        println!(
            "  {} {} R: {}",
            diagnostic.reference.object_number(),
            diagnostic.reference.generation(),
            diagnostic.error
        );
    }
}

#[cfg(test)]
mod tests {
    use pdf_render::Canvas;

    use super::{changed_canvas_region, region_contains};

    #[test]
    fn dirty_region_has_a_known_answer_and_catches_an_outside_pixel() {
        let original = Canvas::blank(6, 5);
        let mut edited = original.clone();
        edited.pixels[usize::from(2_u8) * 6 + usize::from(3_u8)] = [0.0, 0.0, 0.0];
        edited.pixels[usize::from(3_u8) * 6 + usize::from(4_u8)] = [0.0, 0.0, 0.0];

        let changed = changed_canvas_region(&original, &edited)
            .expect("same-size canvases")
            .expect("two pixels changed");
        assert_eq!(changed, [3, 2, 5, 4]);
        assert!(region_contains([2, 1, 5, 4], changed));
        assert!(
            !region_contains([3, 2, 4, 4], changed),
            "the second changed pixel is the negative control"
        );
    }

    #[test]
    fn identical_canvases_are_neutral_and_size_changes_are_not_ignored() {
        let original = Canvas::blank(2, 2);
        assert_eq!(
            changed_canvas_region(&original, &original),
            Ok(None),
            "normalization control"
        );
        assert_eq!(
            changed_canvas_region(&original, &Canvas::blank(3, 2)),
            Err("page raster dimensions changed")
        );
    }
}

#[cfg(test)]
mod usage_tests {
    use super::usage;

    #[test]
    fn usage_names_every_dispatched_command() {
        for command in [
            "inspect",
            "inspect-page",
            "inspect-clusters",
            "render-page",
            "spike-move-text",
            "spike-move-cluster",
            "spike-delete-cluster",
            "spike-delete-selection",
            "links",
            "extract-images",
            "verify-empty-round-trip",
        ] {
            assert!(
                usage().contains(&format!("pdf-cli {command}")),
                "usage does not mention {command}"
            );
        }
    }

    #[test]
    fn usage_does_not_mention_a_command_that_does_not_exist() {
        assert!(!usage().contains("pdf-cli redact"));
        for line in usage().lines() {
            let line = line.trim();
            assert!(
                line.starts_with("usage: pdf-cli ") || line.starts_with("pdf-cli "),
                "usage line is not an invocation: {line:?}"
            );
        }
    }
}
