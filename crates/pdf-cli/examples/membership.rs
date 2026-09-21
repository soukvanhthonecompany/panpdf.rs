use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_semantics::{ClusterReport, DependencyIndex, InkKind, MembershipAudit, Object, ObjectKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Break {
    Drop,
    Duplicate,
    Steal,
    Widen,
}

impl Break {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "drop" => Some(Self::Drop),
            "duplicate" => Some(Self::Duplicate),
            "steal" => Some(Self::Steal),
            "widen" => Some(Self::Widen),
            _ => None,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Drop => "drop",
            Self::Duplicate => "duplicate",
            Self::Steal => "steal",
            Self::Widen => "widen",
        }
    }
}

fn overlaps(one: [f64; 4], other: [f64; 4]) -> bool {
    one[0] < other[2] && other[0] < one[2] && one[1] < other[3] && other[1] < one[3]
}

fn broken(objects: &mut Vec<Object>, how: Break) -> bool {
    match how {
        Break::Drop => {
            let Some(object) = objects.iter_mut().find(|object| !object.members.is_empty()) else {
                return false;
            };
            object.members.pop();
            if object.members.is_empty() {
                objects.retain(|object| !object.members.is_empty());
            }
            true
        }
        Break::Duplicate => {
            let Some(first) = objects
                .iter()
                .find(|object| !object.members.is_empty())
                .and_then(|object| object.members.first().cloned())
            else {
                return false;
            };
            let Some(other) = objects
                .iter_mut()
                .find(|object| !object.members.contains(&first))
            else {
                return false;
            };
            other.members.push(first);
            true
        }
        Break::Steal => {
            let boxes: Vec<_> = objects
                .iter()
                .map(|object| (object.bounds, object.members.clone()))
                .collect();
            let mut stolen = false;
            for (position, object) in objects.iter_mut().enumerate() {
                let Some(mine) = object.bounds else { continue };
                for (other, (theirs, members)) in boxes.iter().enumerate() {
                    if other == position {
                        continue;
                    }
                    if theirs.is_some_and(|theirs| overlaps(mine, theirs)) {
                        for member in members {
                            if !object.members.contains(member) {
                                object.members.push(member.clone());
                                stolen = true;
                            }
                        }
                    }
                }
            }
            stolen
        }
        Break::Widen => {
            let mut boundaries: Vec<(usize, usize, usize)> = Vec::new();
            for (position, object) in objects.iter().enumerate() {
                for (at, member) in object.members.iter().enumerate() {
                    if let Some(glyphs) = &member.glyphs {
                        boundaries.push((member.atom, glyphs.end, position << 32 | at));
                    }
                }
            }
            let starts: Vec<(usize, usize)> = objects
                .iter()
                .flat_map(|object| object.members.iter())
                .filter_map(|member| {
                    member
                        .glyphs
                        .as_ref()
                        .map(|glyphs| (member.atom, glyphs.start))
                })
                .collect();
            let Some(&(_, _, packed)) = boundaries
                .iter()
                .find(|(atom, end, _)| starts.contains(&(*atom, *end)))
            else {
                return false;
            };
            let (position, at) = (packed >> 32, packed & 0xffff_ffff);
            if let Some(glyphs) = objects[position].members[at].glyphs.as_mut() {
                glyphs.end += 1;
            }
            true
        }
    }
}

fn kinds(objects: &[Object]) -> String {
    let (mut text, mut image, mut form, mut shading, mut path, mut run) = (0, 0, 0, 0, 0, 0);
    for object in objects {
        match object.kind {
            ObjectKind::Text(_) => text += 1,
            ObjectKind::Image => image += 1,
            ObjectKind::Form => form += 1,
            ObjectKind::Shading => shading += 1,
            ObjectKind::Path => path += 1,
            ObjectKind::TextRun => run += 1,
        }
    }
    format!(
        "{text} text, {image} image, {form} group, {shading} shading, {path} drawing, \
         {run} unclustered run"
    )
}

fn report_sharing(dependencies: &DependencyIndex) {
    let mut shared: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    let mut objects_sharing = 0_usize;
    for object in 0..dependencies.len() {
        let sharing = dependencies.shared_by(object);
        if !sharing.is_empty() {
            objects_sharing += 1;
        }
        for (dependency, _) in sharing {
            *shared.entry(dependency.kind()).or_insert(0) += 1;
        }
    }
    if objects_sharing == 0 {
        return;
    }
    let named: Vec<String> = shared
        .iter()
        .map(|(kind, count)| format!("{count} {kind}"))
        .collect();
    println!(
        "  {objects_sharing} of {} objects reach a definition another object reaches: {}",
        dependencies.len(),
        named.join(", ")
    );
}

fn report(
    path: &Path,
    page: usize,
    audit: &MembershipAudit,
    objects: &[Object],
    report: &ClusterReport,
    graph: &pdf_paint::PaintGraph,
    dependencies: &DependencyIndex,
) {
    println!(
        "{} page {}: {} atoms -- {} owned, {} unowned, {} contested, {} dangling",
        path.display(),
        page + 1,
        audit.atoms,
        audit.owned,
        audit.orphans.len(),
        audit.contested.len(),
        audit.dangling.len(),
    );
    println!("  objects: {}", kinds(objects));
    let unowned: Vec<String> = audit
        .orphans_by_kind()
        .into_iter()
        .map(|(kind, count)| format!("{count} {}", kind.name()))
        .collect();
    if !unowned.is_empty() {
        println!("  unowned: {}", unowned.join(", "));
    }
    for contested in &audit.contested {
        println!(
            "  contested: atom {} ({}) claimed by objects {:?}",
            contested.atom,
            contested.kind.name(),
            contested.owners
        );
    }
    if audit.nested > 0 {
        println!(
            "  not reached: {} atoms inside a transparency group or a Type 3 procedure",
            audit.nested
        );
    }
    if report.runs_without_glyphs > 0 {
        println!(
            "  runs with no glyph positions: {}",
            report.runs_without_glyphs
        );
    }
    if report.clusters_stranded_by_geometry > 0 {
        println!(
            "  clusters given a row of their own because their geometry is unreadable: {}",
            report.clusters_stranded_by_geometry
        );
    }
    report_sharing(dependencies);
    for orphan in &audit.orphans {
        if orphan.kind != InkKind::Text {
            continue;
        }
        let glyphs = match graph.atoms.get(orphan.atom).map(|atom| &atom.kind) {
            Some(pdf_paint::PaintAtomKind::Text(text)) => text.glyphs.len(),
            _ => 0,
        };
        println!(
            "  unowned text atom {}: {glyphs} glyph positions, {} of {} units unowned",
            orphan.atom, orphan.unowned, orphan.units
        );
    }
}

fn pdfs(target: &Path) -> Vec<PathBuf> {
    if !target.is_dir() {
        return vec![target.to_path_buf()];
    }
    let mut found: Vec<PathBuf> = std::fs::read_dir(target)
        .expect("readdir")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
        })
        .collect();
    found.sort();
    found
}

#[derive(Default)]
struct Totals {
    pages: usize,
    atoms: usize,
    owned: usize,
    unowned: usize,
    contested: usize,
    nested: usize,
    incomplete: usize,
    runs_without_glyphs: usize,
    stranded: usize,
    objects_sharing: usize,
    objects_total: usize,
    shared_by_kind: std::collections::BTreeMap<&'static str, usize>,
}

impl Totals {
    fn add_sharing(&mut self, dependencies: &DependencyIndex) {
        for object in 0..dependencies.len() {
            let shared = dependencies.shared_by(object);
            if shared.is_empty() {
                continue;
            }
            self.objects_sharing += 1;
            for (dependency, _) in shared {
                *self.shared_by_kind.entry(dependency.kind()).or_insert(0) += 1;
            }
        }
        self.objects_total += dependencies.len();
    }
}

fn report_totals(totals: &Totals) {
    let Totals {
        pages,
        atoms,
        owned,
        unowned,
        contested,
        nested,
        incomplete,
        runs_without_glyphs,
        stranded,
        objects_sharing,
        objects_total,
        ref shared_by_kind,
    } = *totals;
    println!(
        "\n{pages} pages: {atoms} atoms -- {owned} owned, {unowned} unowned, {contested} contested"
    );
    println!("  pages whose ink is not fully accounted for: {incomplete} of {pages}");
    println!("  atoms inside groups and procedures, not reached: {nested}");
    println!("  text runs with no glyph positions to cluster: {runs_without_glyphs}");
    println!("  clusters stranded by unreadable geometry: {stranded}");
    let named: Vec<String> = shared_by_kind
        .iter()
        .map(|(kind, count)| format!("{count} {kind}"))
        .collect();
    println!(
        "  objects that reach a definition another object reaches: \
         {objects_sharing} of {objects_total}"
    );
    println!("    by what they share: {}", named.join(", "));
}

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let target = PathBuf::from(arguments.next().expect("a pdf file or a directory"));
    let flags: Vec<String> = arguments.collect();
    let summary = flags.iter().any(|flag| flag == "--summary");
    let expect_accounted = flags.iter().any(|flag| flag == "--expect-accounted");
    let how = flags.iter().position(|flag| flag == "--break").map(|at| {
        let name = flags
            .get(at + 1)
            .expect("--break needs drop|duplicate|steal");
        Break::parse(name).expect("--break takes drop, duplicate or steal")
    });

    let paths = pdfs(&target);
    let one_file = paths.len() == 1;
    let mut totals = Totals::default();
    let (mut broke, mut unnoticed) = (0_usize, 0_usize);

    for path in &paths {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if one_file => panic!("{}: {error}", path.display()),
            Err(_) => continue,
        };
        let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
        let Ok(view) = pdf_session::interpret_page(&source, 0) else {
            continue;
        };
        totals.pages += 1;

        let mut objects = view.index.objects.clone();
        let corrupted = how.is_some_and(|how| broken(&mut objects, how));
        if corrupted {
            broke += 1;
        }
        let audit = MembershipAudit::of(&view.graph, &objects);
        let dependencies = DependencyIndex::of(&view.graph, &objects);
        totals.add_sharing(&dependencies);
        if corrupted && audit.accounted() {
            unnoticed += 1;
            println!(
                "{}: the audit did not notice a `{}` membership",
                path.display(),
                how.expect("a break was applied").name()
            );
        }

        totals.atoms += audit.atoms;
        totals.owned += audit.owned;
        totals.unowned += audit.orphans.len();
        totals.contested += audit.contested.len();
        totals.nested += audit.nested;
        if !audit.accounted() {
            totals.incomplete += 1;
        }
        assert!(
            how.is_some() || audit.agrees_with_report(&view.index.report),
            "{}: the audit and the index disagree about unowned paths",
            path.display()
        );
        totals.runs_without_glyphs += view.index.report.runs_without_glyphs;
        totals.stranded += view.index.report.clusters_stranded_by_geometry;
        if !summary {
            report(
                path,
                0,
                &audit,
                &objects,
                &view.index.report,
                &view.graph,
                &dependencies,
            );
        }
    }

    if summary || paths.len() > 1 {
        report_totals(&totals);
    }

    if let Some(how) = how {
        println!(
            "\nbroke `{}` on {broke} of {} pages; the audit missed {unnoticed}",
            how.name(),
            totals.pages
        );
        if broke == 0 {
            eprintln!("nothing here could be broken, so the instrument was not tested");
            return ExitCode::from(4);
        }
        if unnoticed > 0 {
            return ExitCode::from(4);
        }
        return ExitCode::SUCCESS;
    }
    if expect_accounted && totals.incomplete > 0 {
        eprintln!(
            "{} of {} pages leave ink with no owner",
            totals.incomplete, totals.pages
        );
        return ExitCode::from(3);
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use pdf_semantics::{Member, MembershipAudit, Object, ObjectKind};

    use super::{Break, broken, overlaps};

    fn objects(boxes: &[Option<[f64; 4]>]) -> Vec<Object> {
        boxes
            .iter()
            .enumerate()
            .map(|(index, bounds)| Object {
                kind: ObjectKind::Image,
                members: vec![Member::whole(index)],
                quad: None,
                bounds: *bounds,
            })
            .collect()
    }

    #[test]
    fn boxes_that_touch_at_an_edge_do_not_overlap() {
        assert!(!overlaps([0.0, 0.0, 10.0, 10.0], [10.0, 0.0, 20.0, 10.0]));
        assert!(overlaps([0.0, 0.0, 10.0, 10.0], [9.0, 0.0, 20.0, 10.0]));
    }

    #[test]
    fn dropping_takes_one_atom_out_of_the_membership() {
        let mut membership = objects(&[None, None, None]);
        assert!(broken(&mut membership, Break::Drop));
        let claimed: usize = membership.iter().map(|object| object.members.len()).sum();
        assert_eq!(claimed, 2);
    }

    #[test]
    fn duplicating_gives_one_atom_two_owners() {
        let mut membership = objects(&[None, None]);
        assert!(broken(&mut membership, Break::Duplicate));
        assert_eq!(
            membership[1].members,
            vec![Member::whole(1), Member::whole(0)]
        );
    }

    #[test]
    fn stealing_takes_only_the_neighbours_whose_boxes_overlap() {
        let mut membership = objects(&[
            Some([0.0, 0.0, 10.0, 10.0]),
            Some([5.0, 5.0, 15.0, 15.0]),
            Some([100.0, 100.0, 110.0, 110.0]),
        ]);
        assert!(broken(&mut membership, Break::Steal));
        assert_eq!(
            membership[0].members,
            vec![Member::whole(0), Member::whole(1)]
        );
        assert_eq!(
            membership[1].members,
            vec![Member::whole(1), Member::whole(0)]
        );
        assert_eq!(
            membership[2].members,
            vec![Member::whole(2)],
            "a distant object was swept up"
        );
    }

    #[test]
    fn a_page_with_one_object_cannot_have_an_atom_claimed_twice() {
        let mut membership = objects(&[None]);
        assert!(
            !broken(&mut membership, Break::Duplicate),
            "a break that changed nothing must say so"
        );
    }

    #[test]
    fn every_break_is_one_the_audit_reports() {
        use pdf_paint::PaintGraph;

        let graph = PaintGraph {
            object_scopes: Vec::new(),
            repairs: Vec::new(),
            skipped: Vec::new(),
            atoms: Vec::new(),
        };
        for how in [Break::Drop, Break::Duplicate, Break::Steal] {
            let mut membership = objects(&[
                Some([0.0, 0.0, 10.0, 10.0]),
                Some([5.0, 5.0, 15.0, 15.0]),
                Some([100.0, 100.0, 110.0, 110.0]),
            ]);
            let changed = broken(&mut membership, how);
            assert!(changed, "{how:?} broke nothing");
            let audit = MembershipAudit::of(&graph, &membership);
            assert!(
                !audit.accounted(),
                "{how:?} produced a membership the audit calls clean"
            );
        }
    }
}
