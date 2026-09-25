use std::collections::HashMap;

use pdf_bytes::ByteStore;
use pdf_syntax::{ObjectKind, Reference};

use crate::fill_field::{Entry, pdf_text_string, set_entries};
use crate::form::{Found, Reader};
use crate::plan::{Capability, Effect, Plan, PlannedBody, PlannedWrite};
use crate::spike_move_text::{PlannerPage, SpikeError};

const MOST_ITEMS: usize = 65_536;
const MOST_DEPTH: usize = 64;

fn refused(reason: &'static str) -> SpikeError {
    SpikeError::RetypeUnsupported(reason)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bookmark {
    pub reference: Reference,
    pub title: String,
    pub page: Option<usize>,
    pub depth: usize,
    pub open: bool,
    pub children: usize,
    pub previous: Option<Reference>,
    pub parent: Option<Reference>,
}

pub fn read_outline(source: &ByteStore, credential: &[u8]) -> Result<Vec<Bookmark>, SpikeError> {
    let reader = Reader::open(source, credential)?;
    let pages = page_numbers(source, credential);
    let Some(root) = root_of(&reader) else {
        return Ok(Vec::new());
    };
    let mut found = Vec::new();
    walk(&reader, &pages, (&root, None), 0, &mut found);
    Ok(found)
}

fn root_of(reader: &Reader) -> Option<Found> {
    reader.entry(&reader.catalog()?, b"/Outlines")
}

pub(crate) fn root_reference(reader: &Reader) -> Option<Reference> {
    reference_entry(&reader.catalog()?, b"/Outlines")
}

fn page_numbers(source: &ByteStore, credential: &[u8]) -> HashMap<(u32, u16), usize> {
    pdf_content::page_references_with_password(
        source,
        pdf_content::PageContentLimits::default(),
        credential,
    )
    .unwrap_or_default()
    .into_iter()
    .enumerate()
    .map(|(at, page)| ((page.object_number(), page.generation()), at))
    .collect()
}

pub(crate) fn reference_entry(holder: &Found, key: &[u8]) -> Option<Reference> {
    match Reader::raw_entry(holder, key)?.kind() {
        ObjectKind::Reference(reference) => Some(*reference),
        _ => None,
    }
}

fn walk(
    reader: &Reader,
    pages: &HashMap<(u32, u16), usize>,
    (node, parent): (&Found, Option<Reference>),
    depth: usize,
    found: &mut Vec<Bookmark>,
) {
    if depth > MOST_DEPTH || found.len() > MOST_ITEMS {
        return;
    }
    let mut item = reference_entry(node, b"/First");
    let mut previous = None;
    while let Some(reference) = item {
        let Some(here) = reader.at(reference) else {
            return;
        };
        let count = reader
            .entry(&here, b"/Count")
            .and_then(|found| Reader::number(&found))
            .unwrap_or(0.0);
        let children = child_count(reader, &here);
        found.push(Bookmark {
            reference,
            title: reader
                .entry(&here, b"/Title")
                .and_then(|found| reader.text(&found))
                .unwrap_or_default(),
            page: page_of(reader, pages, &here),
            depth,
            open: count > 0.0,
            children,
            previous,
            parent,
        });
        previous = Some(reference);
        if children > 0 {
            walk(reader, pages, (&here, Some(reference)), depth + 1, found);
        }
        if found.len() > MOST_ITEMS {
            return;
        }
        item = reference_entry(&here, b"/Next");
    }
}

fn child_count(reader: &Reader, item: &Found) -> usize {
    let mut count = 0;
    let mut child = reference_entry(item, b"/First");
    while let Some(reference) = child {
        count += 1;
        if count > MOST_ITEMS {
            break;
        }
        let Some(here) = reader.at(reference) else {
            break;
        };
        child = reference_entry(&here, b"/Next");
    }
    count
}

fn page_of(reader: &Reader, pages: &HashMap<(u32, u16), usize>, item: &Found) -> Option<usize> {
    let destination = reader.entry(item, b"/Dest").or_else(|| {
        let action = reader.entry(item, b"/A")?;
        let kind = reader
            .entry(&action, b"/S")
            .and_then(|found| Reader::name(&found))?;
        (kind == "GoTo").then(|| reader.entry(&action, b"/D"))?
    })?;
    let array = match destination.value.kind() {
        ObjectKind::Array(_) => destination,
        ObjectKind::Name | ObjectKind::LiteralString | ObjectKind::HexString => {
            named_destination(reader, &destination)?
        }
        _ => return None,
    };
    let ObjectKind::Array(items) = array.value.kind() else {
        return None;
    };
    match items.first()?.kind() {
        ObjectKind::Reference(page) => pages
            .get(&(page.object_number(), page.generation()))
            .copied(),
        ObjectKind::Number(_) => Reader::number(&reader.follow(&array, items.first()?)?)
            .filter(|number| number.fract() == 0.0 && *number >= 0.0)
            .map(|number| {
                #[allow(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "a whole page number, checked to be positive"
                )]
                let page = number as usize;
                page
            }),
        _ => None,
    }
}

fn named_destination(reader: &Reader, name: &Found) -> Option<Found> {
    let wanted = match name.value.kind() {
        ObjectKind::Name => Reader::name(name)?.into_bytes(),
        _ => reader.text(name)?.into_bytes(),
    };
    let catalog = reader.catalog()?;
    if let Some(names) = reader.entry(&catalog, b"/Names")
        && let Some(tree) = reader.entry(&names, b"/Dests")
        && let Some(found) = in_name_tree(reader, &tree, &wanted, 0)
    {
        return destination_array(reader, &found);
    }
    let dests = reader.entry(&catalog, b"/Dests")?;
    let mut key = Vec::with_capacity(wanted.len() + 1);
    key.push(b'/');
    key.extend_from_slice(&wanted);
    destination_array(reader, &reader.entry(&dests, &key)?)
}

fn destination_array(reader: &Reader, found: &Found) -> Option<Found> {
    match found.value.kind() {
        ObjectKind::Array(_) => Some(found.clone()),
        ObjectKind::Dictionary(_) => reader.entry(found, b"/D"),
        _ => None,
    }
}

fn in_name_tree(reader: &Reader, node: &Found, wanted: &[u8], depth: usize) -> Option<Found> {
    if depth > MOST_DEPTH {
        return None;
    }
    if let Some(names) = reader.entry(node, b"/Names")
        && let ObjectKind::Array(items) = names.value.kind()
    {
        for pair in items.chunks(2) {
            let [key, value] = pair else { continue };
            let key = reader.text(&reader.follow(&names, key)?)?;
            if key.as_bytes() == wanted {
                return reader.follow(&names, value);
            }
        }
    }
    let kids = reader.entry(node, b"/Kids")?;
    let ObjectKind::Array(items) = kids.value.kind() else {
        return None;
    };
    for kid in items {
        let kid = reader.follow(&kids, kid)?;
        if within(reader, &kid, wanted)
            && let Some(found) = in_name_tree(reader, &kid, wanted, depth + 1)
        {
            return Some(found);
        }
    }
    None
}

fn within(reader: &Reader, node: &Found, wanted: &[u8]) -> bool {
    let Some(limits) = reader.entry(node, b"/Limits") else {
        return true;
    };
    let ObjectKind::Array(items) = limits.value.kind() else {
        return true;
    };
    let bound = |at: usize| {
        items
            .get(at)
            .and_then(|item| reader.follow(&limits, item))
            .and_then(|found| reader.text(&found))
    };
    match (bound(0), bound(1)) {
        (Some(low), Some(high)) => wanted >= low.as_bytes() && wanted <= high.as_bytes(),
        _ => true,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Add {
        title: String,
        page: usize,
        after: Option<Reference>,
        inside: bool,
    },
    Rename {
        bookmark: Reference,
        title: String,
    },
    Retarget {
        bookmark: Reference,
        page: usize,
    },
    Remove {
        bookmark: Reference,
    },
    Move {
        bookmark: Reference,
        after: Option<Reference>,
        inside: bool,
        before: bool,
    },
    Fold {
        bookmark: Reference,
        open: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
struct Node {
    reference: Reference,
    open: bool,
    children: Vec<Node>,
}

impl Node {
    fn shown(&self) -> usize {
        self.children
            .iter()
            .map(|child| 1 + if child.open { child.shown() } else { 0 })
            .sum()
    }

    fn find(&self, wanted: Reference) -> Option<&Self> {
        if self.reference == wanted {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find(wanted))
    }

    fn take(&mut self, wanted: Reference) -> Option<Self> {
        if let Some(at) = self
            .children
            .iter()
            .position(|child| child.reference == wanted)
        {
            return Some(self.children.remove(at));
        }
        self.children
            .iter_mut()
            .find_map(|child| child.take(wanted))
    }

    fn put(&mut self, node: Self, after: Option<Reference>, inside: bool, before: bool) -> bool {
        let Some(after) = after else {
            self.children.push(node);
            return true;
        };
        if inside {
            if let Some(parent) = self.child_mut(after) {
                parent.children.push(node);
                return true;
            }
            return false;
        }
        if let Some(at) = self
            .children
            .iter()
            .position(|child| child.reference == after)
        {
            self.children.insert(if before { at } else { at + 1 }, node);
            return true;
        }
        for child in &mut self.children {
            if child.put(node.clone(), Some(after), inside, before) {
                return true;
            }
        }
        false
    }

    fn child_mut(&mut self, wanted: Reference) -> Option<&mut Self> {
        if self.reference == wanted {
            return Some(self);
        }
        self.children
            .iter_mut()
            .find_map(|child| child.child_mut(wanted))
    }
}

pub(crate) fn plan_outline_change(
    source: &ByteStore,
    page: PlannerPage<'_>,
    page_index: usize,
    change: &Change,
) -> Result<Plan, SpikeError> {
    let credential = page.credential;
    let reader = Reader::open(source, credential)?;
    let pages = pdf_content::page_references_with_password(
        source,
        pdf_content::PageContentLimits::default(),
        credential,
    )
    .map_err(|_| refused("this document's pages cannot be walked"))?;
    let before = read_outline(source, credential)?;

    let mut number = crate::block_rewrite::next_object_number(source)?;
    let mut writes: Vec<PlannedWrite> = Vec::new();
    let mut fresh: HashMap<(u32, u16), String> = HashMap::new();
    let root = root_reference(&reader).unwrap_or_else(|| {
        let root = Reference::new(number, 0);
        number += 1;
        fresh.insert(
            (root.object_number(), root.generation()),
            "/Type /Outlines".to_owned(),
        );
        root
    });
    let root_is_new = fresh.contains_key(&(root.object_number(), root.generation()));
    let mut tree = Node {
        reference: root,
        open: true,
        children: tree_of(&before),
    };

    apply(
        (source, credential),
        change,
        (&mut tree, &mut writes, &mut fresh),
        (&pages, number),
    )?;

    writes.extend(linked((source, credential), &tree, &fresh)?);
    if root_is_new {
        let catalog = reader
            .catalog_reference()
            .ok_or_else(|| refused("this document's catalog cannot be read"))?;
        writes.push(set_entries(
            (source, credential),
            catalog,
            &[(b"/Outlines", crate::object_edit::reference_text(root))],
        )?);
    }
    let writes = one_write_each(writes)?;
    prove_outline((source, credential), &writes, (&tree, change))?;
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

fn apply(
    (source, credential): (&ByteStore, &[u8]),
    change: &Change,
    (tree, writes, fresh): (
        &mut Node,
        &mut Vec<PlannedWrite>,
        &mut HashMap<(u32, u16), String>,
    ),
    (pages, number): (&[Reference], u32),
) -> Result<(), SpikeError> {
    match change {
        Change::Add {
            title,
            page: to,
            after,
            inside,
        } => {
            let title = checked_title(title)?;
            let destination = destination_of(pages, *to)?;
            let added = Reference::new(number, 0);
            fresh.insert(
                (added.object_number(), added.generation()),
                format!("/Title {} /Dest {destination}", pdf_text_string(title)),
            );
            if let Some(after) = after
                && tree.find(*after).is_none()
            {
                return Err(refused("this is not a bookmark of this document"));
            }
            let node = Node {
                reference: added,
                open: false,
                children: Vec::new(),
            };
            if !tree.put(node, *after, *inside, false) {
                return Err(refused("this is not a bookmark of this document"));
            }
            if *inside && let Some(after) = after {
                open_up(tree, *after);
            }
        }
        Change::Rename { bookmark, title } => {
            let title = checked_title(title)?;
            found(tree, *bookmark)?;
            writes.push(set_entries(
                (source, credential),
                *bookmark,
                &[(b"/Title", pdf_text_string(title))],
            )?);
        }
        Change::Retarget { bookmark, page: to } => {
            found(tree, *bookmark)?;
            let destination = destination_of(pages, *to)?;
            writes.push(set_entries(
                (source, credential),
                *bookmark,
                &[(b"/Dest", destination), (b"/A", String::new())],
            )?);
        }
        Change::Remove { bookmark } => {
            found(tree, *bookmark)?;
            tree.take(*bookmark)
                .ok_or_else(|| refused("this is not a bookmark of this document"))?;
        }
        Change::Move {
            bookmark,
            after,
            inside,
            before,
        } => {
            found(tree, *bookmark)?;
            if let Some(after) = after {
                let held = tree
                    .find(*bookmark)
                    .ok_or_else(|| refused("this is not a bookmark of this document"))?;
                if held.find(*after).is_some() {
                    return Err(refused(
                        "a bookmark cannot be put inside itself or under one of its own",
                    ));
                }
                if tree.find(*after).is_none() {
                    return Err(refused("this is not a bookmark of this document"));
                }
            }
            let node = tree
                .take(*bookmark)
                .ok_or_else(|| refused("this is not a bookmark of this document"))?;
            if !tree.put(node, *after, *inside, *before) {
                return Err(refused("this is not a bookmark of this document"));
            }
            if *inside && let Some(after) = after {
                open_up(tree, *after);
            }
        }
        Change::Fold { bookmark, open } => {
            found(tree, *bookmark)?;
            if let Some(node) = tree.child_mut(*bookmark) {
                node.open = *open;
            }
        }
    }
    Ok(())
}

fn tree_of(bookmarks: &[Bookmark]) -> Vec<Node> {
    let mut roots: Vec<Node> = Vec::new();
    let mut path: Vec<Reference> = Vec::new();
    for book in bookmarks {
        let node = Node {
            reference: book.reference,
            open: book.open,
            children: Vec::new(),
        };
        path.truncate(book.depth);
        match path.last().copied() {
            None => roots.push(node),
            Some(parent) => {
                let mut holder = Node {
                    reference: Reference::new(0, 0),
                    open: true,
                    children: std::mem::take(&mut roots),
                };
                if let Some(under) = holder.child_mut(parent) {
                    under.children.push(node);
                }
                roots = holder.children;
            }
        }
        path.push(book.reference);
    }
    roots
}

fn open_up(tree: &mut Node, bookmark: Reference) {
    if let Some(node) = tree.child_mut(bookmark) {
        node.open = true;
    }
}

fn found(tree: &Node, bookmark: Reference) -> Result<(), SpikeError> {
    tree.find(bookmark)
        .map(|_| ())
        .ok_or_else(|| refused("this is not a bookmark of this document"))
}

fn checked_title(title: &str) -> Result<&str, SpikeError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(refused("a bookmark has a title"));
    }
    if title.chars().any(char::is_control) || title.chars().count() > 512 {
        return Err(refused("a bookmark's title is a line of text"));
    }
    Ok(title)
}

fn destination_of(pages: &[Reference], page: usize) -> Result<String, SpikeError> {
    let reference = pages
        .get(page)
        .ok_or_else(|| refused("this document has no such page"))?;
    Ok(format!(
        "[{} /XYZ null null null]",
        crate::object_edit::reference_text(*reference)
    ))
}

fn linked(
    (source, credential): (&ByteStore, &[u8]),
    tree: &Node,
    fresh: &HashMap<(u32, u16), String>,
) -> Result<Vec<PlannedWrite>, SpikeError> {
    let mut writes = Vec::new();
    let mut wanted: Vec<(Reference, Vec<Entry>)> = Vec::new();
    collect(tree, None, &mut wanted);
    let reader = Reader::open(source, credential)?;
    for (reference, entries) in wanted {
        if let Some(base) = fresh.get(&(reference.object_number(), reference.generation())) {
            let listed: Vec<String> = entries
                .iter()
                .filter(|(_, value)| !value.is_empty())
                .map(|(key, value)| format!("{} {value}", String::from_utf8_lossy(key)))
                .collect();
            writes.push(PlannedWrite {
                reference,
                body: PlannedBody::Direct {
                    body: format!("<< {base} {} >>", listed.join(" ")).into_bytes(),
                },
            });
            continue;
        }
        if unchanged(&reader, reference, &entries) {
            continue;
        }
        writes.push(set_entries((source, credential), reference, &entries)?);
    }
    Ok(writes)
}

fn collect(node: &Node, parent: Option<Reference>, wanted: &mut Vec<(Reference, Vec<Entry>)>) {
    let text = |reference: Option<Reference>| {
        reference.map_or_else(String::new, crate::object_edit::reference_text)
    };
    let count = if node.children.is_empty() {
        String::new()
    } else if node.open || parent.is_none() {
        node.shown().to_string()
    } else {
        format!("-{}", node.shown())
    };
    let mut entries: Vec<Entry> = vec![
        (
            b"/First",
            text(node.children.first().map(|one| one.reference)),
        ),
        (
            b"/Last",
            text(node.children.last().map(|one| one.reference)),
        ),
        (b"/Count", count),
    ];
    if parent.is_some() {
        entries.push((b"/Parent", text(parent)));
    }
    wanted.push((node.reference, entries));
    for (at, child) in node.children.iter().enumerate() {
        collect(child, Some(node.reference), wanted);
        let previous = at
            .checked_sub(1)
            .and_then(|before| node.children.get(before));
        let next = node.children.get(at + 1);
        if let Some((_, entries)) = wanted
            .iter_mut()
            .find(|(reference, _)| *reference == child.reference)
        {
            entries.push((b"/Prev", text(previous.map(|one| one.reference))));
            entries.push((b"/Next", text(next.map(|one| one.reference))));
        }
    }
}

fn unchanged(reader: &Reader, reference: Reference, entries: &[Entry]) -> bool {
    let Some(node) = reader.at(reference) else {
        return false;
    };
    entries.iter().all(|(key, value)| {
        let held = Reader::raw_entry(&node, key);
        match (held, value.is_empty()) {
            (None, true) => true,
            (None, false) | (Some(_), true) => false,
            (Some(object), false) => node
                .source
                .resolve(object.span())
                .is_ok_and(|bytes| String::from_utf8_lossy(bytes).trim() == value.trim()),
        }
    })
}

fn prove_outline(
    (source, credential): (&ByteStore, &[u8]),
    writes: &[PlannedWrite],
    (tree, change): (&Node, &Change),
) -> Result<(), SpikeError> {
    let document = crate::block_rewrite::commit_writes(
        source,
        writes,
        (credential, crate::Restrictions::SetAside),
    )?;
    let after = read_outline(&document, credential)?;
    let mut wanted: Vec<(Reference, usize)> = Vec::new();
    flatten(tree, 0, &mut wanted);
    let read: Vec<(Reference, usize)> = after
        .iter()
        .map(|book| (book.reference, book.depth))
        .collect();
    if read != wanted {
        return Err(refused(
            "the bookmarks do not read back in the order asked for",
        ));
    }
    match change {
        Change::Rename { bookmark, title } => {
            let named = after
                .iter()
                .find(|book| book.reference == *bookmark)
                .is_some_and(|book| book.title == title.trim());
            if !named {
                return Err(refused(
                    "the bookmark does not read back under its new name",
                ));
            }
        }
        Change::Add { page, .. } | Change::Retarget { page, .. } => {
            let bookmark = match change {
                Change::Add { .. } => after.iter().find(|book| {
                    !wanted.is_empty() && book.reference == wanted_added(tree, &after)
                }),
                Change::Retarget { bookmark, .. } => {
                    after.iter().find(|book| book.reference == *bookmark)
                }
                _ => None,
            };
            if bookmark.is_some_and(|book| book.page != Some(*page)) {
                return Err(refused(
                    "the bookmark does not read back on the page asked for",
                ));
            }
        }
        Change::Fold { bookmark, open } => {
            let folded = after
                .iter()
                .find(|book| book.reference == *bookmark)
                .is_some_and(|book| book.open == *open || book.children == 0);
            if !folded {
                return Err(refused(
                    "the bookmark does not read back open or closed as asked",
                ));
            }
        }
        Change::Move { .. } | Change::Remove { .. } => {}
    }
    Ok(())
}

fn wanted_added(tree: &Node, after: &[Bookmark]) -> Reference {
    let mut every: Vec<(Reference, usize)> = Vec::new();
    flatten(tree, 0, &mut every);
    every
        .iter()
        .map(|(reference, _)| *reference)
        .find(|reference| !after.iter().any(|book| book.reference == *reference))
        .unwrap_or_else(|| Reference::new(0, 0))
}

fn flatten(node: &Node, depth: usize, into: &mut Vec<(Reference, usize)>) {
    for child in &node.children {
        into.push((child.reference, depth));
        flatten(child, depth + 1, into);
    }
}

fn one_write_each(writes: Vec<PlannedWrite>) -> Result<Vec<PlannedWrite>, SpikeError> {
    let mut seen = std::collections::HashSet::new();
    for write in &writes {
        if !seen.insert((
            write.reference.object_number(),
            write.reference.generation(),
        )) {
            return Err(refused("this change would write one bookmark twice"));
        }
    }
    Ok(writes)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{Bookmark, read_outline};
    use pdf_bytes::ByteStore;

    pub(crate) fn with_outline(objects: &[&str]) -> ByteStore {
        let mut every: Vec<&str> = vec![
            "<< /Type /Catalog /Pages 2 0 R /Outlines 7 0 R /Names << /Dests 8 0 R >> >>",
            "<< /Type /Pages /MediaBox [0 0 300 300] /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>",
            "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << >> >>",
            "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << >> >>",
            "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << >> >>",
            "<< /Length 0 >>\nstream\n\nendstream",
        ];
        every.extend_from_slice(objects);
        crate::new_field::tests::document(&every)
    }

    #[test]
    fn an_outline_reads_as_the_tree_a_table_of_contents_shows() {
        let source = with_outline(&[
            "<< /Type /Outlines /First 9 0 R /Last 12 0 R /Count 4 >>",
            "<< /Names [(chapter) [5 0 R /Fit]] >>",
            "<< /Title (One) /Parent 7 0 R /Next 12 0 R /First 10 0 R /Last 11 0 R /Count 2 /Dest [3 0 R /Fit] >>",
            "<< /Title (One a) /Parent 9 0 R /Next 11 0 R /Dest [4 0 R /XYZ null 700 null] >>",
            "<< /Title (One b) /Parent 9 0 R /Prev 10 0 R /A << /S /GoTo /D [5 0 R /Fit] >> >>",
            "<< /Title (Two) /Parent 7 0 R /Prev 9 0 R /First 13 0 R /Last 13 0 R /Count -1 /Dest (chapter) >>",
            "<< /Title (Two a) /Parent 12 0 R /Dest [3 0 R /Fit] >>",
        ]);
        let outline = read_outline(&source, b"").expect("an outline");
        let shown: Vec<(&str, Option<usize>, usize, bool)> = outline
            .iter()
            .map(|book: &Bookmark| (book.title.as_str(), book.page, book.depth, book.open))
            .collect();
        assert_eq!(
            shown,
            [
                ("One", Some(0), 0, true),
                ("One a", Some(1), 1, false),
                ("One b", Some(2), 1, false),
                ("Two", Some(2), 0, false),
                ("Two a", Some(0), 1, false),
            ]
        );
        assert_eq!(outline[0].children, 2);
        assert_eq!(outline[3].children, 1, "a closed chapter still has its own");
        assert_eq!(outline[1].parent, Some(outline[0].reference));
        assert_eq!(outline[2].previous, Some(outline[1].reference));
    }

    #[test]
    fn a_document_without_an_outline_has_no_bookmarks() {
        let source = crate::new_field::tests::document(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /MediaBox [0 0 300 300] /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>",
            "<< /Length 0 >>\nstream\n\nendstream",
        ]);
        assert!(read_outline(&source, b"").expect("no outline").is_empty());
    }

    #[test]
    fn a_bookmark_that_goes_nowhere_is_still_read() {
        let source = with_outline(&[
            "<< /Type /Outlines /First 9 0 R /Last 9 0 R /Count 1 >>",
            "<< /Names [(chapter) [5 0 R /Fit]] >>",
            "<< /Title (Nowhere) /Parent 7 0 R /Dest (missing) >>",
        ]);
        let outline = read_outline(&source, b"").expect("an outline");
        assert_eq!(outline.len(), 1);
        assert_eq!(outline[0].title, "Nowhere");
        assert_eq!(outline[0].page, None);
    }

    use super::Change;
    use crate::form::Reader;
    use crate::plan::Command;
    use crate::spike_move_text::{SpikeError, plan_command_with_fonts};
    use pdf_syntax::Reference;

    fn after(source: &ByteStore, change: Change) -> Result<ByteStore, SpikeError> {
        let command = Command::ChangeOutline {
            page_index: 0,
            change,
        };
        let plan = plan_command_with_fonts(source, &command, b"", None)?;
        crate::block_rewrite::commit_writes(
            source,
            plan.writes(),
            (b"", crate::Restrictions::Respect),
        )
    }

    fn shown(source: &ByteStore) -> Vec<(String, usize, Option<usize>)> {
        read_outline(source, b"")
            .expect("an outline")
            .into_iter()
            .map(|book| (book.title, book.depth, book.page))
            .collect()
    }

    fn a_book() -> ByteStore {
        with_outline(&[
            "<< /Type /Outlines /First 9 0 R /Last 11 0 R /Count 3 >>",
            "<< /Names [] >>",
            "<< /Title (One) /Parent 7 0 R /Next 11 0 R /First 10 0 R /Last 10 0 R /Count 1 /Dest [3 0 R /Fit] >>",
            "<< /Title (One a) /Parent 9 0 R /Dest [4 0 R /Fit] >>",
            "<< /Title (Two) /Parent 7 0 R /Prev 9 0 R /Dest [5 0 R /Fit] >>",
        ])
    }

    #[test]
    fn a_bookmark_is_added_at_the_end() {
        let added = after(
            &a_book(),
            Change::Add {
                title: "Three".to_owned(),
                page: 2,
                after: None,
                inside: false,
            },
        )
        .expect("added");
        assert_eq!(
            shown(&added),
            [
                ("One".to_owned(), 0, Some(0)),
                ("One a".to_owned(), 1, Some(1)),
                ("Two".to_owned(), 0, Some(2)),
                ("Three".to_owned(), 0, Some(2)),
            ]
        );
    }

    #[test]
    fn a_bookmark_is_added_inside_another() {
        let book = a_book();
        let two = read_outline(&book, b"").expect("an outline")[2].reference;
        let added = after(
            &book,
            Change::Add {
                title: "Two a".to_owned(),
                page: 1,
                after: Some(two),
                inside: true,
            },
        )
        .expect("added");
        assert_eq!(
            shown(&added),
            [
                ("One".to_owned(), 0, Some(0)),
                ("One a".to_owned(), 1, Some(1)),
                ("Two".to_owned(), 0, Some(2)),
                ("Two a".to_owned(), 1, Some(1)),
            ]
        );
        let outline = read_outline(&added, b"").expect("an outline");
        assert!(outline[2].open, "a parent opens to show what was put in it");
    }

    #[test]
    fn a_document_without_bookmarks_gains_them() {
        let bare = crate::new_field::tests::document(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /MediaBox [0 0 300 300] /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>",
            "<< /Length 0 >>\nstream\n\nendstream",
        ]);
        let added = after(
            &bare,
            Change::Add {
                title: "Start".to_owned(),
                page: 0,
                after: None,
                inside: false,
            },
        )
        .expect("added");
        assert_eq!(shown(&added), [("Start".to_owned(), 0, Some(0))]);
    }

    #[test]
    fn a_bookmark_is_renamed_and_sent_elsewhere() {
        let book = a_book();
        let section = read_outline(&book, b"").expect("an outline")[1].reference;
        let renamed = after(
            &book,
            Change::Rename {
                bookmark: section,
                title: "Section one".to_owned(),
            },
        )
        .expect("renamed");
        let sent = after(
            &renamed,
            Change::Retarget {
                bookmark: section,
                page: 2,
            },
        )
        .expect("sent");
        assert_eq!(
            shown(&sent),
            [
                ("One".to_owned(), 0, Some(0)),
                ("Section one".to_owned(), 1, Some(2)),
                ("Two".to_owned(), 0, Some(2)),
            ]
        );
        assert!(
            after(
                &book,
                Change::Rename {
                    bookmark: section,
                    title: "  ".to_owned(),
                },
            )
            .is_err(),
            "a bookmark has a title"
        );
        assert!(
            after(
                &book,
                Change::Retarget {
                    bookmark: section,
                    page: 9,
                },
            )
            .is_err(),
            "this document has no page nine"
        );
    }

    #[test]
    fn a_bookmark_is_removed_with_its_children() {
        let book = a_book();
        let one = read_outline(&book, b"").expect("an outline")[0].reference;
        let removed = after(&book, Change::Remove { bookmark: one }).expect("removed");
        assert_eq!(shown(&removed), [("Two".to_owned(), 0, Some(2))]);
    }

    #[test]
    fn a_bookmark_is_moved_but_never_inside_itself() {
        let book = a_book();
        let outline = read_outline(&book, b"").expect("an outline");
        let (one, section, two) = (
            outline[0].reference,
            outline[1].reference,
            outline[2].reference,
        );
        let moved = after(
            &book,
            Change::Move {
                bookmark: section,
                after: Some(two),
                inside: true,
                before: false,
            },
        )
        .expect("moved");
        assert_eq!(
            shown(&moved),
            [
                ("One".to_owned(), 0, Some(0)),
                ("Two".to_owned(), 0, Some(2)),
                ("One a".to_owned(), 1, Some(1)),
            ]
        );
        assert!(
            after(
                &book,
                Change::Move {
                    bookmark: one,
                    after: Some(section),
                    inside: true,
                    before: false,
                },
            )
            .is_err(),
            "a bookmark cannot be put under its own child"
        );
        assert!(
            after(
                &book,
                Change::Move {
                    bookmark: Reference::new(999, 0),
                    after: None,
                    inside: false,
                    before: false,
                },
            )
            .is_err()
        );
    }

    #[test]
    fn a_bookmark_is_folded_and_opened() {
        let book = a_book();
        let one = read_outline(&book, b"").expect("an outline")[0].reference;
        let folded = after(
            &book,
            Change::Fold {
                bookmark: one,
                open: false,
            },
        )
        .expect("folded");
        let outline = read_outline(&folded, b"").expect("an outline");
        assert!(!outline[0].open);
        assert_eq!(outline[0].children, 1, "its child is still its child");
        let opened = after(
            &folded,
            Change::Fold {
                bookmark: one,
                open: true,
            },
        )
        .expect("opened");
        assert!(read_outline(&opened, b"").expect("an outline")[0].open);
    }

    fn body_of(source: &ByteStore, reference: Reference) -> String {
        let reader = Reader::open(source, b"").expect("opens");
        let found = reader.at(reference).expect("the object");
        String::from_utf8_lossy(found.source.resolve(found.value.span()).expect("its bytes"))
            .into_owned()
    }

    #[test]
    fn the_count_written_is_what_a_viewer_shows() {
        let book = a_book();
        let outline = read_outline(&book, b"").expect("an outline");
        let (one, two) = (outline[0].reference, outline[2].reference);
        let root = crate::outline::root_reference(&Reader::open(&book, b"").expect("opens"))
            .expect("a root");
        let opened = after(
            &book,
            Change::Add {
                title: "Two a".to_owned(),
                page: 0,
                after: Some(two),
                inside: true,
            },
        )
        .expect("added");
        assert!(
            body_of(&opened, root).contains("/Count 4"),
            "{}",
            body_of(&opened, root)
        );
        let folded = after(
            &opened,
            Change::Fold {
                bookmark: one,
                open: false,
            },
        )
        .expect("folded");
        assert!(
            body_of(&folded, root).contains("/Count 3"),
            "{}",
            body_of(&folded, root)
        );
        assert!(
            body_of(&folded, one).contains("/Count -1"),
            "{}",
            body_of(&folded, one)
        );
    }

    #[test]
    fn a_bookmark_lands_after_the_one_it_was_put_after() {
        let book = a_book();
        let one = read_outline(&book, b"").expect("an outline")[0].reference;
        let added = after(
            &book,
            Change::Add {
                title: "Between".to_owned(),
                page: 1,
                after: Some(one),
                inside: false,
            },
        )
        .expect("added");
        let titles: Vec<String> = read_outline(&added, b"")
            .expect("an outline")
            .into_iter()
            .map(|book| book.title)
            .collect();
        assert_eq!(titles, ["One", "One a", "Between", "Two"]);
    }

    #[test]
    fn a_title_of_two_lines_is_refused() {
        let book = a_book();
        let one = read_outline(&book, b"").expect("an outline")[0].reference;
        assert!(
            after(
                &book,
                Change::Rename {
                    bookmark: one,
                    title: "One\nTwo".to_owned(),
                },
            )
            .is_err()
        );
    }

    #[test]
    fn retargeting_takes_the_old_action_off() {
        let source = with_outline(&[
            "<< /Type /Outlines /First 9 0 R /Last 9 0 R /Count 1 >>",
            "<< /Names [] >>",
            "<< /Title (Go) /Parent 7 0 R /A << /S /GoTo /D [5 0 R /Fit] >> >>",
        ]);
        let bookmark = read_outline(&source, b"").expect("an outline")[0].reference;
        assert_eq!(
            read_outline(&source, b"").expect("an outline")[0].page,
            Some(2)
        );
        let sent = after(&source, Change::Retarget { bookmark, page: 0 }).expect("sent");
        assert_eq!(
            read_outline(&sent, b"").expect("an outline")[0].page,
            Some(0)
        );
        let body = body_of(&sent, bookmark);
        assert!(!body.contains("/A "), "{body}");
    }

    #[test]
    fn a_bookmark_moves_up_by_landing_before_another() {
        let book = a_book();
        let outline = read_outline(&book, b"").expect("an outline");
        let (one, section, two) = (
            outline[0].reference,
            outline[1].reference,
            outline[2].reference,
        );
        let moved = after(
            &book,
            Change::Move {
                bookmark: two,
                after: Some(one),
                inside: false,
                before: true,
            },
        )
        .expect("moved");
        let titles: Vec<String> = read_outline(&moved, b"")
            .expect("an outline")
            .into_iter()
            .map(|book| book.title)
            .collect();
        assert_eq!(titles, ["Two", "One", "One a"]);
        let out = after(
            &book,
            Change::Move {
                bookmark: section,
                after: Some(one),
                inside: false,
                before: true,
            },
        )
        .expect("moved out");
        let depths: Vec<(String, usize)> = read_outline(&out, b"")
            .expect("an outline")
            .into_iter()
            .map(|book| (book.title, book.depth))
            .collect();
        assert_eq!(
            depths,
            [
                ("One a".to_owned(), 0),
                ("One".to_owned(), 0),
                ("Two".to_owned(), 0)
            ]
        );
    }
}
