use pdf_bytes::ByteStore;
use pdf_syntax::{ObjectKind, Reference};

use crate::field_settings::pdf_literal;
use crate::fill_field::set_entries;
use crate::form::{Found, Reader};
use crate::link::Arrival;
use crate::plan::{Capability, Effect, Plan, PlannedBody, PlannedWrite};
use crate::spike_move_text::{PlannerPage, SpikeError};

const MOST_NAMES: usize = 8_192;
const LONGEST_NAME: usize = 256;
const MOST_TREE_BYTES: usize = 1 << 22;

fn refused(reason: &'static str) -> SpikeError {
    SpikeError::RetypeUnsupported(reason)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Spot {
    pub name: String,
    pub page: usize,
    pub arrival: Arrival,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Naming {
    Name {
        name: String,
        page: usize,
        arrival: Arrival,
    },
    Rename {
        from: String,
        to: String,
    },
    Remove {
        name: String,
    },
}

pub fn spots_of(source: &ByteStore) -> Result<Vec<Spot>, SpikeError> {
    let resolver =
        pdf_content::open_link_resolver(source, pdf_content::PageContentLimits::default(), b"")
            .map_err(|_| refused("this document's names cannot be read"))?;
    Ok(resolver
        .destinations()
        .into_iter()
        .take(MOST_NAMES)
        .filter_map(|(name, destination)| {
            Some(Spot {
                name: String::from_utf8(name).ok()?,
                page: destination.page?,
                arrival: crate::link::arrival_of(destination.view),
            })
        })
        .collect())
}

pub fn spot_named(source: &ByteStore, name: &str) -> Result<Option<Spot>, SpikeError> {
    let resolver =
        pdf_content::open_link_resolver(source, pdf_content::PageContentLimits::default(), b"")
            .map_err(|_| refused("this document's names cannot be read"))?;
    Ok(resolver
        .destination_named(name.as_bytes())
        .and_then(|destination| {
            Some(Spot {
                name: name.to_owned(),
                page: destination.page?,
                arrival: crate::link::arrival_of(destination.view),
            })
        }))
}

type Pair = (Vec<u8>, String);

fn checked_name(name: &str) -> Result<Vec<u8>, SpikeError> {
    let name = name.trim();
    if name.is_empty() || name.len() > LONGEST_NAME || name.chars().any(char::is_control) {
        return Err(refused("a destination's name is one line of text"));
    }
    Ok(name.as_bytes().to_vec())
}

struct Names {
    holder: Option<Reference>,
    pairs: Vec<Pair>,
}

fn names_of(reader: &Reader) -> Result<Names, SpikeError> {
    let catalog = reader
        .catalog()
        .ok_or_else(|| refused("this document's catalog cannot be read"))?;
    let holder = crate::outline::reference_entry(&catalog, b"/Names");
    let Some(names) = reader.entry(&catalog, b"/Names") else {
        return Ok(Names {
            holder,
            pairs: Vec::new(),
        });
    };
    let Some(tree) = reader.entry(&names, b"/Dests") else {
        return Ok(Names {
            holder,
            pairs: Vec::new(),
        });
    };
    let mut pairs = Vec::new();
    walk(reader, &tree, &mut pairs, 0)?;
    Ok(Names { holder, pairs })
}

fn walk(
    reader: &Reader,
    node: &Found,
    pairs: &mut Vec<Pair>,
    depth: usize,
) -> Result<(), SpikeError> {
    if depth > 64 || pairs.len() > MOST_NAMES {
        return Err(refused("this document's names go too deep to rewrite"));
    }
    if let Some(names) = reader.entry(node, b"/Names")
        && let ObjectKind::Array(items) = names.value.kind()
    {
        for pair in items.chunks_exact(2) {
            let key = reader
                .follow(&names, &pair[0])
                .and_then(|found| match found.value.kind() {
                    ObjectKind::Name => Reader::name(&found).map(String::into_bytes),
                    _ => reader.bytes(&found),
                })
                .ok_or_else(|| refused("this document names a destination unreadably"))?;
            pairs.push((key, written(&names, &pair[1])?));
        }
    }
    if let Some(kids) = reader.entry(node, b"/Kids")
        && let ObjectKind::Array(children) = kids.value.kind()
    {
        for child in children {
            let child = reader
                .follow(&kids, child)
                .ok_or_else(|| refused("this document's name tree cannot be walked"))?;
            walk(reader, &child, pairs, depth + 1)?;
        }
    }
    Ok(())
}

fn names_beside_destinations(reader: &Reader) -> bool {
    let Some(catalog) = reader.catalog() else {
        return false;
    };
    let Some(names) = reader.entry(&catalog, b"/Names") else {
        return false;
    };
    let pdf_syntax::ObjectKind::Dictionary(entries) = names.value.kind() else {
        return false;
    };
    entries
        .iter()
        .any(|entry| !entry.key_equals(&names.source, b"/Dests"))
}

fn holds_a_string(value: &pdf_syntax::Object) -> bool {
    match value.kind() {
        pdf_syntax::ObjectKind::LiteralString | pdf_syntax::ObjectKind::HexString => true,
        pdf_syntax::ObjectKind::Array(items) => items.iter().any(holds_a_string),
        pdf_syntax::ObjectKind::Dictionary(entries) => {
            entries.iter().any(|entry| holds_a_string(entry.value()))
        }
        _ => false,
    }
}

fn written(holder: &Found, value: &pdf_syntax::Object) -> Result<String, SpikeError> {
    if let ObjectKind::Reference(reference) = value.kind() {
        return Ok(crate::object_edit::reference_text(*reference));
    }
    if holds_a_string(value) {
        return Err(refused(
            "this document writes a destination this cannot copy",
        ));
    }
    let bytes = holder
        .source
        .resolve(value.span())
        .map_err(|_| refused("this document's name tree cannot be read"))?;
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| refused("this document writes a destination this cannot copy"))
}

fn place(source: &ByteStore, page: usize, arrival: Arrival) -> Result<String, SpikeError> {
    let pages = pdf_content::page_references_with_password(
        source,
        pdf_content::PageContentLimits::default(),
        b"",
    )
    .map_err(|_| refused("this document's pages cannot be walked"))?;
    let reference = pages
        .get(page)
        .ok_or_else(|| refused("this document has no such page"))?;
    Ok(format!(
        "[{} {}]",
        crate::object_edit::reference_text(*reference),
        crate::link::destination_view(source, *reference, arrival)?
    ))
}

pub(crate) fn plan_naming(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    change: &Naming,
) -> Result<Plan, SpikeError> {
    let reader = Reader::open(source, b"")?;
    let before = spots_of(source)?;
    let mut names = names_of(&reader)?;
    let wanted = wanted_pairs(source, &mut names.pairs, change)?;

    let mut number = crate::block_rewrite::next_object_number(source)?;
    let root = Reference::new(number, 0);
    number += 1;
    let mut writes = vec![PlannedWrite {
        reference: root,
        body: PlannedBody::Direct {
            body: format!("<< /Names {} >>", leaf(&wanted)?).into_bytes(),
        },
    }];
    let pointed = (
        b"/Dests".as_slice(),
        crate::object_edit::reference_text(root),
    );
    if let Some(holder) = names.holder {
        writes.push(set_entries(source, holder, &[pointed])?);
    } else {
        let catalog = reader
            .catalog_reference()
            .ok_or_else(|| refused("this document's catalog cannot be read"))?;
        if names_beside_destinations(&reader) {
            return Err(refused(
                "this document writes its names inside its catalog, which this cannot rewrite yet",
            ));
        }
        let holder = Reference::new(number, 0);
        writes.push(PlannedWrite {
            reference: holder,
            body: PlannedBody::Direct {
                body: format!("<< /Dests {} >>", crate::object_edit::reference_text(root))
                    .into_bytes(),
            },
        });
        writes.push(set_entries(
            source,
            catalog,
            &[(b"/Names", crate::object_edit::reference_text(holder))],
        )?);
    }
    if let Naming::Remove { name } | Naming::Rename { from: name, .. } = change {
        writes.extend(out_of_the_dictionary(
            &reader,
            source,
            name.trim().as_bytes(),
        )?);
    }
    prove_names(source, &writes, (&before, change))?;
    let target = page
        .program
        .streams
        .first()
        .map_or(page.program.page, |stream| stream.reference);
    Ok(Plan::new(
        Capability::Exact,
        writes,
        Effect {
            page_index,
            moved: Vec::new(),
            target_stream: target,
            declared_region: None,
        },
    ))
}

fn wanted_pairs(
    source: &ByteStore,
    pairs: &mut Vec<Pair>,
    change: &Naming,
) -> Result<Vec<Pair>, SpikeError> {
    let mut wanted: Vec<Pair> = std::mem::take(pairs);
    match change {
        Naming::Name {
            name,
            page,
            arrival,
        } => {
            let key = checked_name(name)?;
            let value = place(source, *page, *arrival)?;
            wanted.retain(|(held, _)| held != &key);
            wanted.push((key, value));
        }
        Naming::Rename { from, to } => {
            let from = checked_name(from)?;
            let to = checked_name(to)?;
            let at = wanted
                .iter()
                .position(|(held, _)| held == &from)
                .ok_or_else(|| refused("this document does not name that destination"))?;
            let (_, value) = wanted.remove(at);
            wanted.retain(|(held, _)| held != &to);
            wanted.push((to, value));
        }
        Naming::Remove { name } => {
            let key = checked_name(name)?;
            let before = wanted.len();
            wanted.retain(|(held, _)| held != &key);
            if wanted.len() == before {
                return Err(refused("this document does not name that destination"));
            }
        }
    }
    if wanted.len() > MOST_NAMES {
        return Err(refused(
            "this document names more places than this rewrites",
        ));
    }
    wanted.sort_by(|(one, _), (other, _)| one.cmp(other));
    wanted.dedup_by(|(one, _), (other, _)| one == other);
    Ok(wanted)
}

fn leaf(pairs: &[Pair]) -> Result<String, SpikeError> {
    let mut out = String::from("[");
    for (key, value) in pairs {
        let key = std::str::from_utf8(key)
            .map_err(|_| refused("this document names a destination this cannot copy"))?;
        out.push(' ');
        out.push_str(&pdf_literal(key));
        out.push(' ');
        out.push_str(value);
        if out.len() > MOST_TREE_BYTES {
            return Err(refused(
                "this document names more places than this rewrites",
            ));
        }
    }
    out.push_str(" ]");
    Ok(out)
}

fn out_of_the_dictionary(
    reader: &Reader,
    source: &ByteStore,
    name: &[u8],
) -> Result<Vec<PlannedWrite>, SpikeError> {
    let Some(catalog) = reader.catalog() else {
        return Ok(Vec::new());
    };
    let Some(reference) = crate::outline::reference_entry(&catalog, b"/Dests") else {
        return Ok(Vec::new());
    };
    let Some(dictionary) = reader.entry(&catalog, b"/Dests") else {
        return Ok(Vec::new());
    };
    let mut key = Vec::with_capacity(name.len() + 1);
    key.push(b'/');
    key.extend_from_slice(name);
    if Reader::raw_entry(&dictionary, &key).is_none() {
        return Ok(Vec::new());
    }
    Ok(vec![set_entries(
        source,
        reference,
        &[(&key, String::new())],
    )?])
}

fn prove_names(
    source: &ByteStore,
    writes: &[PlannedWrite],
    (before, change): (&[Spot], &Naming),
) -> Result<(), SpikeError> {
    let document =
        crate::block_rewrite::commit_writes(source, writes, crate::Restrictions::SetAside)?;
    let after = spots_of(&document)?;
    let at = |spots: &[Spot], name: &str| {
        spots
            .iter()
            .find(|spot| spot.name == name)
            .map(|spot| (spot.page, spot.arrival))
    };
    let untouched: Vec<&str> = match change {
        Naming::Name {
            name,
            page,
            arrival,
        } => {
            if at(&after, name.trim()) != Some((*page, *arrival)) {
                return Err(refused(
                    "the name does not read back at the place asked for",
                ));
            }
            vec![name.trim()]
        }
        Naming::Rename { from, to } => {
            if at(&after, to.trim()) != at(before, from.trim()) {
                return Err(refused("the new name does not stand for the same place"));
            }
            if at(&after, from.trim()).is_some() {
                return Err(refused("the old name is still in the document"));
            }
            vec![from.trim(), to.trim()]
        }
        Naming::Remove { name } => {
            if at(&after, name.trim()).is_some() {
                return Err(refused("the name is still in the document"));
            }
            vec![name.trim()]
        }
    };
    for spot in before {
        if untouched.contains(&spot.name.as_str()) {
            continue;
        }
        if at(&after, &spot.name) != Some((spot.page, spot.arrival)) {
            return Err(refused("this change would move another name's destination"));
        }
    }
    for spot in &after {
        if spot_named(&document, &spot.name)?.as_ref() != Some(spot) {
            return Err(refused("a name in this document could not be followed"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use pdf_bytes::ByteStore;

    use super::{Naming, Spot, spots_of};
    use crate::link::{Arrival, Target};
    use crate::plan::Command;
    use crate::spike_move_text::{SpikeError, plan_command_with_fonts};

    fn document(catalog: &str, objects: &[&str]) -> ByteStore {
        let mut every: Vec<&str> = vec![
            catalog,
            "<< /Type /Pages /MediaBox [0 0 300 400] /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>",
            "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << >> >>",
            "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << >> >>",
            "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << >> >>",
            "<< /Length 21 >>\nstream\n0 0 m 100 100 l S    \nendstream",
        ];
        every.extend_from_slice(objects);
        crate::new_field::tests::document(&every)
    }

    fn bare() -> ByteStore {
        document("<< /Type /Catalog /Pages 2 0 R >>", &[])
    }

    fn with_names() -> ByteStore {
        document(
            "<< /Type /Catalog /Pages 2 0 R /Names 7 0 R >>",
            &[
                "<< /Dests 8 0 R >>",
                "<< /Names [(one) [3 0 R /Fit] (two) 9 0 R] >>",
                "<< /D [4 0 R /FitR 10 20 110 220] >>",
            ],
        )
    }

    fn named(source: &ByteStore, change: Naming) -> Result<ByteStore, SpikeError> {
        let command = Command::ChangeNaming {
            page_index: 0,
            change,
        };
        let plan = plan_command_with_fonts(source, &command, b"", None)?;
        crate::block_rewrite::commit_writes(source, plan.writes(), crate::Restrictions::Respect)
    }

    fn naming(name: &str, page: usize, arrival: Arrival) -> Naming {
        Naming::Name {
            name: name.to_owned(),
            page,
            arrival,
        }
    }

    #[test]
    fn the_names_a_document_holds_are_listed_with_their_places() {
        let source = document(
            "<< /Type /Catalog /Pages 2 0 R /Names 7 0 R /Dests 9 0 R >>",
            &[
                "<< /Dests 8 0 R >>",
                "<< /Names [(beta) [5 0 R /XYZ 0 400 2]] >>",
                "<< /alpha [4 0 R /Fit] >>",
            ],
        );
        assert_eq!(
            spots_of(&source).expect("the names read"),
            vec![
                Spot {
                    name: "alpha".to_owned(),
                    page: 1,
                    arrival: Arrival::FitPage,
                },
                Spot {
                    name: "beta".to_owned(),
                    page: 2,
                    arrival: Arrival::Percent(200.0),
                },
            ]
        );
    }

    #[test]
    fn a_first_name_builds_the_tree_it_needs() {
        let source =
            named(&bare(), naming("start", 2, Arrival::FitPage)).expect("the name is made");
        assert_eq!(
            spots_of(&source).expect("the names read"),
            vec![Spot {
                name: "start".to_owned(),
                page: 2,
                arrival: Arrival::FitPage,
            }]
        );
    }

    #[test]
    fn a_new_name_leaves_the_others_where_they_were() {
        let source =
            named(&with_names(), naming("three", 2, Arrival::FitWidth)).expect("the name is made");
        let spots = spots_of(&source).expect("the names read");
        assert_eq!(
            spots,
            vec![
                Spot {
                    name: "one".to_owned(),
                    page: 0,
                    arrival: Arrival::FitPage,
                },
                Spot {
                    name: "three".to_owned(),
                    page: 2,
                    arrival: Arrival::FitWidth,
                },
                Spot {
                    name: "two".to_owned(),
                    page: 1,
                    arrival: Arrival::FitVisible,
                },
            ]
        );
    }

    #[test]
    fn a_name_under_a_branch_survives_the_rebuild() {
        let source = document(
            "<< /Type /Catalog /Pages 2 0 R /Names 7 0 R >>",
            &[
                "<< /Dests 8 0 R >>",
                "<< /Kids [9 0 R 10 0 R] >>",
                "<< /Limits [(alpha) (alpha)] /Names [(alpha) [3 0 R /Fit]] >>",
                "<< /Limits [(omega) (omega)] /Names [(omega) [5 0 R /Fit]] >>",
            ],
        );
        let after =
            named(&source, naming("middle", 1, Arrival::FitPage)).expect("the name is made");
        let spots = spots_of(&after).expect("the names read");
        let names: Vec<&str> = spots.iter().map(|spot| spot.name.as_str()).collect();
        assert_eq!(names, ["alpha", "middle", "omega"]);
        for spot in &spots {
            assert_eq!(
                super::spot_named(&after, &spot.name).expect("the name reads"),
                Some(spot.clone()),
                "{}",
                spot.name
            );
        }
    }

    #[test]
    fn a_second_name_goes_into_the_tree_the_first_one_made() {
        let once = named(&bare(), naming("chapter two", 0, Arrival::FitPage))
            .expect("the first name is made");
        let twice =
            named(&once, naming("preface", 2, Arrival::FitWidth)).expect("the second name is made");
        let spots = spots_of(&twice).expect("the names read");
        let names: Vec<&str> = spots.iter().map(|spot| spot.name.as_str()).collect();
        assert_eq!(names, ["chapter two", "preface"]);
        assert_eq!((spots[1].page, spots[1].arrival), (2, Arrival::FitWidth));
    }

    #[test]
    fn naming_a_place_twice_moves_the_name() {
        let once =
            named(&with_names(), naming("one", 2, Arrival::ActualSize)).expect("the name is moved");
        let spots = spots_of(&once).expect("the names read");
        assert_eq!(spots.len(), 2);
        assert_eq!(spots[0].name, "one");
        assert_eq!((spots[0].page, spots[0].arrival), (2, Arrival::ActualSize));
    }

    #[test]
    fn a_renamed_destination_stands_for_the_same_place() {
        let source = named(
            &with_names(),
            Naming::Rename {
                from: "two".to_owned(),
                to: "appendix".to_owned(),
            },
        )
        .expect("the name is changed");
        let spots = spots_of(&source).expect("the names read");
        let names: Vec<&str> = spots.iter().map(|spot| spot.name.as_str()).collect();
        assert_eq!(names, ["appendix", "one"]);
        assert_eq!((spots[0].page, spots[0].arrival), (1, Arrival::FitVisible));
    }

    #[test]
    fn a_removed_name_is_gone_from_the_older_dictionary_too() {
        let source = document(
            "<< /Type /Catalog /Pages 2 0 R /Names 7 0 R /Dests 9 0 R >>",
            &[
                "<< /Dests 8 0 R >>",
                "<< /Names [(shared) [3 0 R /Fit]] >>",
                "<< /shared [5 0 R /Fit] /kept [4 0 R /Fit] >>",
            ],
        );
        let after = named(
            &source,
            Naming::Remove {
                name: "shared".to_owned(),
            },
        )
        .expect("the name is taken away");
        assert_eq!(
            spots_of(&after).expect("the names read"),
            vec![Spot {
                name: "kept".to_owned(),
                page: 1,
                arrival: Arrival::FitPage,
            }]
        );
    }

    #[test]
    fn what_is_refused() {
        let refusal = |change: Naming| match named(&with_names(), change) {
            Err(SpikeError::RetypeUnsupported(reason)) => reason.to_owned(),
            other => panic!("expected a refusal, got {other:?}"),
        };
        assert!(
            refusal(Naming::Remove {
                name: "nowhere".to_owned()
            })
            .contains("does not name that destination")
        );
        assert!(
            refusal(Naming::Rename {
                from: "nowhere".to_owned(),
                to: "somewhere".to_owned(),
            })
            .contains("does not name that destination")
        );
        assert!(refusal(naming("", 0, Arrival::FitPage)).contains("one line of text"));
        assert!(refusal(naming("a\nb", 0, Arrival::FitPage)).contains("one line of text"));
        assert!(refusal(naming("start", 9, Arrival::FitPage)).contains("no such page"));
    }

    #[test]
    fn a_link_to_a_name_reads_back_as_the_name() {
        let source =
            named(&bare(), naming("chapter two", 1, Arrival::FitPage)).expect("the name is made");
        let added = Command::AddLink {
            page_index: 0,
            rect: [20.0, 200.0, 180.0, 220.0],
            target: Target::Name("chapter two".to_owned()),
            look: crate::link::Look::default(),
        };
        let plan =
            plan_command_with_fonts(&source, &added, b"", None).expect("the link is planned");
        let with_link = crate::block_rewrite::commit_writes(
            &source,
            plan.writes(),
            crate::Restrictions::Respect,
        )
        .expect("the link is written");
        let links = crate::link::links_of(&with_link, 0).expect("the links read");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].2, Some(Target::Name("chapter two".to_owned())));
    }

    #[test]
    fn a_link_to_a_name_the_document_lost_reads_as_going_nowhere() {
        let source = document(
            "<< /Type /Catalog /Pages 2 0 R >>",
            &["<< /Type /Annot /Subtype /Link /Rect [20 200 180 220] /F 4 /Dest (nowhere) >>"],
        );
        let listed = crate::new_field::listed_on_page(
            &source,
            pdf_syntax::Reference::new(3, 0),
            &[pdf_syntax::Reference::new(7, 0)],
        )
        .expect("the link is listed on the page");
        let page =
            crate::block_rewrite::commit_writes(&source, &listed, crate::Restrictions::Respect)
                .expect("the page is written");
        let links = crate::link::links_of(&page, 0).expect("the links read");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].2, None);
    }

    #[test]
    fn a_link_to_a_name_that_is_not_there_is_refused() {
        let added = Command::AddLink {
            page_index: 0,
            rect: [20.0, 200.0, 180.0, 220.0],
            target: Target::Name("nowhere".to_owned()),
            look: crate::link::Look::default(),
        };
        match plan_command_with_fonts(&with_names(), &added, b"", None) {
            Err(SpikeError::RetypeUnsupported(reason)) => {
                assert!(
                    reason.contains("does not name that destination"),
                    "{reason}"
                );
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }
}
