use pdf_bytes::ByteStore;
use pdf_content::{PageContentLimits, page_references_with_password};
use pdf_syntax::{Object, ObjectKind, Reference};

use crate::new_font::{Body, entry, resolve};
use crate::plan::{Capability, Effect, PageChange, Plan, PlannedBody, PlannedWrite};
use crate::spike_move_text::{PlannerPage, SpikeError};

const MOST_ANCESTORS: usize = 64;

#[derive(Clone, Copy, Debug)]
pub struct BlankPage {
    pub beside: usize,
    pub before: bool,
    pub size: [f64; 2],
}

impl BlankPage {
    const fn at(&self) -> usize {
        if self.before {
            self.beside
        } else {
            self.beside + 1
        }
    }
}

pub(crate) fn refused(reason: &'static str) -> SpikeError {
    SpikeError::RetypeUnsupported(reason)
}

pub(crate) fn order(source: &ByteStore, credential: &[u8]) -> Result<Vec<Reference>, SpikeError> {
    page_references_with_password(source, PageContentLimits::default(), credential)
        .map_err(|_| refused("this document's pages cannot be listed"))
}

pub(crate) fn plan_blank_page(
    source: &ByteStore,
    page: PlannerPage<'_>,
    blank: &BlankPage,
) -> Result<Plan, SpikeError> {
    let [width, height] = blank.size;
    if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
        return Err(refused("a page has a width and a height"));
    }
    let before = order(source, page.credential)?;
    if blank.beside >= before.len() {
        return Err(refused("a page cannot go beside a page that is not there"));
    }
    let neighbour = page.program.page;
    let mut tree = TreeEdit::new(source, page.credential);
    let parent = tree.parent_of(neighbour)?;

    let leaf = crate::block_rewrite::next_object_number(source)?;
    let (added, stream) = (Reference::new(leaf, 0), Reference::new(leaf + 1, 0));
    tree.insert_new(added, neighbour, blank.before, parent)?;
    let mut writes = vec![
        PlannedWrite {
            reference: added,
            body: PlannedBody::Direct {
                body: format!(
                    "<< /Type /Page /Parent {} {} R /MediaBox [0 0 {width} {height}] \
                     /Resources << >> /Contents {} {} R >>",
                    parent.object_number(),
                    parent.generation(),
                    stream.object_number(),
                    stream.generation(),
                )
                .into_bytes(),
            },
        },
        PlannedWrite {
            reference: stream,
            body: PlannedBody::NewStream {
                dictionary: Vec::new(),
                decoded: Vec::new(),
            },
        },
    ];
    writes.extend(tree.writes()?);

    let change = PageChange::Added(vec![blank.at()]);
    let document =
        crate::block_rewrite::commit_writes(source, &writes, (page.credential, page.restrictions))?;
    prove_order(&before, (&document, page.credential), &change, &[added])?;
    let put = pdf_content::load_page_program_with_password(
        &document,
        blank.at(),
        PageContentLimits::default(),
        page.credential,
    )
    .map_err(|_| refused("the page added does not read"))?;
    #[expect(clippy::float_cmp, reason = "a written number read back is the number")]
    let sized = put.geometry.media_box == [0.0, 0.0, width, height];
    if !sized {
        return Err(refused("the page added is not the size asked for"));
    }
    Ok(structural(writes, change, stream))
}

fn named(pages: &[usize], count: usize) -> Result<Vec<usize>, SpikeError> {
    let mut sorted = pages.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.is_empty() || sorted.len() != pages.len() {
        return Err(refused("name each page once"));
    }
    if sorted.last().is_some_and(|last| *last >= count) {
        return Err(refused("a page named is not in the document"));
    }
    Ok(sorted)
}

pub(crate) fn plan_remove_pages(
    source: &ByteStore,
    page: PlannerPage<'_>,
    pages: &[usize],
) -> Result<Plan, SpikeError> {
    let before = order(source, page.credential)?;
    let gone = named(pages, before.len())?;
    if gone.len() >= before.len() {
        return Err(refused("a document keeps at least one page"));
    }
    if before.get(gone[0]) != Some(&page.program.page) {
        return Err(refused("the page to take out is not the page read"));
    }
    let mut tree = TreeEdit::new(source, page.credential);
    for index in &gone {
        tree.remove(before[*index])?;
    }
    let writes = tree.writes()?;
    let change = PageChange::Removed(gone);
    let document =
        crate::block_rewrite::commit_writes(source, &writes, (page.credential, page.restrictions))?;
    prove_order(&before, (&document, page.credential), &change, &[])?;
    Ok(structural(writes, change, first_stream(&page)))
}

pub(crate) fn plan_move_pages(
    source: &ByteStore,
    page: PlannerPage<'_>,
    (pages, to): (&[usize], usize),
) -> Result<Plan, SpikeError> {
    let before = order(source, page.credential)?;
    named(pages, before.len())?;
    if to + pages.len() > before.len() {
        return Err(refused("pages cannot move past the end of the document"));
    }
    if before.get(pages[0]) != Some(&page.program.page) {
        return Err(refused("the page to move is not the page read"));
    }
    let mut after: Vec<usize> = (0..before.len())
        .filter(|index| !pages.contains(index))
        .collect();
    for (offset, moved) in pages.iter().enumerate() {
        after.insert(to + offset, *moved);
    }
    let mut reorder = vec![0; before.len()];
    for (new_index, old_index) in after.iter().enumerate() {
        reorder[*old_index] = new_index;
    }
    if reorder.iter().enumerate().all(|(old, new)| old == *new) {
        return Err(refused("the pages are already there"));
    }
    let mut tree = TreeEdit::new(source, page.credential);
    for moved in pages {
        tree.remove(before[*moved])?;
    }
    let mut previous: Option<Reference> = to.checked_sub(1).map(|at| before[after[at]]);
    for moved in pages {
        let reference = before[*moved];
        if let Some(neighbour) = previous {
            tree.insert(reference, neighbour, false)?;
        } else {
            let first_left = after
                .iter()
                .find(|index| !pages.contains(index))
                .ok_or_else(|| refused("there is no page left to move beside"))?;
            tree.insert(reference, before[*first_left], true)?;
        }
        previous = Some(reference);
    }
    let writes = tree.writes()?;
    let change = PageChange::Reordered(reorder);
    let document =
        crate::block_rewrite::commit_writes(source, &writes, (page.credential, page.restrictions))?;
    prove_order(&before, (&document, page.credential), &change, &[])?;
    Ok(structural(writes, change, first_stream(&page)))
}

fn first_stream(page: &PlannerPage<'_>) -> Reference {
    page.program
        .streams
        .first()
        .map_or(page.program.page, |stream| stream.reference)
}

pub(crate) fn plan_rotate_pages(
    source: &ByteStore,
    page: PlannerPage<'_>,
    (pages, quarter_turns): (&[usize], i32),
) -> Result<Plan, SpikeError> {
    if quarter_turns.rem_euclid(4) == 0 {
        return Err(refused(
            "a page turned all the way round is the page it was",
        ));
    }
    let before = order(source, page.credential)?;
    let turned = named(pages, before.len())?;
    let geometries = pdf_content::page_geometries_with_password(
        source,
        PageContentLimits::default(),
        page.credential,
    )
    .map_err(|_| refused("this document's pages cannot be laid out"))?;
    let mut writes = Vec::new();
    let mut wanted = Vec::new();
    for index in &turned {
        let now = i32::from(geometries[*index].rotate);
        let turn = (now + 90 * quarter_turns).rem_euclid(360);
        let body = resolve(source, before[*index], page.credential)?;
        writes.push(match entry(&body, &body.value, b"/Rotate") {
            Some(value) => rewritten(&body, &[(value, turn.to_string())])?,
            None => with_entry(&body, &format!("/Rotate {turn}"))?,
        });
        wanted.push(turn);
    }
    let document =
        crate::block_rewrite::commit_writes(source, &writes, (page.credential, page.restrictions))?;
    if order(&document, page.credential)? != before {
        return Err(refused("turning a page moved the pages"));
    }
    let shown = pdf_content::page_geometries_with_password(
        &document,
        PageContentLimits::default(),
        page.credential,
    )
    .map_err(|_| refused("the document with the page turned does not read"))?;
    for (index, turn) in turned.iter().zip(&wanted) {
        if shown.get(*index).map(|geometry| i32::from(geometry.rotate)) != Some(*turn) {
            return Err(refused("a page is not at the turn asked for"));
        }
    }
    Ok(Plan::new(
        Capability::Exact,
        writes,
        Effect {
            page_index: turned[0],
            moved: Vec::new(),
            target_stream: first_stream(&page),
            declared_region: None,
        },
    ))
}

fn with_entry(body: &Body, text: &str) -> Result<PlannedWrite, SpikeError> {
    let malformed = || refused("this page's dictionary cannot be rewritten");
    let end = body
        .value
        .span()
        .end()
        .checked_sub(body.offset)
        .ok_or_else(malformed)?;
    let close = end.checked_sub(2).ok_or_else(malformed)?;
    if body.bytes.get(close..end) != Some(b">>".as_slice()) {
        return Err(malformed());
    }
    let mut bytes = Vec::with_capacity(body.bytes.len() + text.len() + 2);
    bytes.extend_from_slice(&body.bytes[..close]);
    if !bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes.push(b' ');
    }
    bytes.extend_from_slice(text.as_bytes());
    bytes.push(b' ');
    bytes.extend_from_slice(&body.bytes[close..]);
    Ok(PlannedWrite {
        reference: body.reference,
        body: PlannedBody::Direct { body: bytes },
    })
}

fn structural(writes: Vec<PlannedWrite>, change: PageChange, target: Reference) -> Plan {
    Plan::new(
        Capability::Exact,
        writes,
        Effect {
            page_index: change.first(),
            moved: Vec::new(),
            target_stream: target,
            declared_region: None,
        },
    )
    .with_pages(change)
}

pub(crate) fn prove_order(
    before: &[Reference],
    (document, credential): (&ByteStore, &[u8]),
    change: &PageChange,
    added: &[Reference],
) -> Result<(), SpikeError> {
    let after = order(document, credential)
        .map_err(|_| refused("the document with the pages changed does not read"))?;
    let mut wanted: Vec<Option<Reference>> = vec![None; after.len()];
    for (index, page) in before.iter().enumerate() {
        if let Some(to) = change.renumbered(index) {
            let slot = wanted
                .get_mut(to)
                .ok_or_else(|| refused("the document does not have the pages it should"))?;
            *slot = Some(*page);
        }
    }
    let expected = match change {
        PageChange::Added(pages) => {
            if pages.len() != added.len() {
                return Err(refused("the pages added are not the pages asked for"));
            }
            for (at, page) in pages.iter().zip(added) {
                let slot = wanted
                    .get_mut(*at)
                    .ok_or_else(|| refused("a page added is not where it was asked for"))?;
                *slot = Some(*page);
            }
            before.len() + pages.len()
        }
        PageChange::Removed(pages) => before.len() - pages.len(),
        PageChange::Reordered(_) => before.len(),
    };
    if after.len() != expected
        || wanted
            .iter()
            .zip(&after)
            .any(|(wanted, got)| *wanted != Some(*got))
    {
        return Err(refused("the pages are not in the order asked for"));
    }
    Ok(())
}

pub(crate) struct TreeEdit<'s> {
    source: &'s ByteStore,
    credential: &'s [u8],
    nodes: Vec<(Reference, Option<Vec<Reference>>, i64)>,
    parents: Vec<(Reference, Reference)>,
    made: Vec<(Reference, Reference)>,
}

impl<'s> TreeEdit<'s> {
    pub(crate) const fn new(source: &'s ByteStore, credential: &'s [u8]) -> Self {
        Self {
            source,
            credential,
            nodes: Vec::new(),
            parents: Vec::new(),
            made: Vec::new(),
        }
    }

    pub(crate) fn parent_of(&self, object: Reference) -> Result<Reference, SpikeError> {
        if let Some((_, node)) = self
            .parents
            .iter()
            .chain(&self.made)
            .rev()
            .find(|(page, _)| *page == object)
        {
            return Ok(*node);
        }
        let body = resolve(self.source, object, self.credential)?;
        match entry(&body, &body.value, b"/Parent").map(Object::kind) {
            Some(ObjectKind::Reference(reference)) => Ok(*reference),
            _ => Err(refused("a page does not say which node holds it")),
        }
    }

    fn slot(&mut self, node: Reference) -> &mut (Reference, Option<Vec<Reference>>, i64) {
        let at = if let Some(at) = self.nodes.iter().position(|held| held.0 == node) {
            at
        } else {
            self.nodes.push((node, None, 0));
            self.nodes.len() - 1
        };
        &mut self.nodes[at]
    }

    pub(crate) fn made(&mut self, page: Reference, node: Reference) {
        self.made.push((page, node));
    }

    fn kids(&mut self, node: Reference) -> Result<Vec<Reference>, SpikeError> {
        if let Some(kids) = self.slot(node).1.clone() {
            return Ok(kids);
        }
        let body = resolve(self.source, node, self.credential)?;
        let unreadable = || refused("this document's page tree cannot be read");
        let kids = entry(&body, &body.value, b"/Kids").ok_or_else(unreadable)?;
        let ObjectKind::Array(values) = kids.kind() else {
            return Err(unreadable());
        };
        values
            .iter()
            .map(|kid| match kid.kind() {
                ObjectKind::Reference(reference) => Ok(*reference),
                _ => Err(unreadable()),
            })
            .collect()
    }

    fn place_of(kids: &[Reference], page: Reference) -> Result<usize, SpikeError> {
        if kids.iter().filter(|kid| **kid == page).count() != 1 {
            return Err(refused(
                "this page is named more than once by the node that holds it",
            ));
        }
        kids.iter()
            .position(|kid| *kid == page)
            .ok_or_else(|| refused("this document's page tree cannot be read"))
    }

    fn remove(&mut self, page: Reference) -> Result<(), SpikeError> {
        let node = self.parent_of(page)?;
        let mut kids = self.kids(node)?;
        let at = Self::place_of(&kids, page)?;
        kids.remove(at);
        self.slot(node).1 = Some(kids);
        self.gain(node, -1)
    }

    pub(crate) fn insert(
        &mut self,
        page: Reference,
        neighbour: Reference,
        before: bool,
    ) -> Result<(), SpikeError> {
        let node = self.parent_of(neighbour)?;
        let mut kids = self.kids(node)?;
        let at = Self::place_of(&kids, neighbour)?;
        kids.insert(if before { at } else { at + 1 }, page);
        self.slot(node).1 = Some(kids);
        self.gain(node, 1)?;
        if !self.made.iter().any(|(made, _)| *made == page)
            && let Ok(held) = self.parent_of(page)
            && held != node
        {
            self.parents.retain(|(pending, _)| *pending != page);
            self.parents.push((page, node));
        }
        Ok(())
    }

    fn gain(&mut self, node: Reference, pages: i64) -> Result<(), SpikeError> {
        let mut seen: Vec<Reference> = Vec::new();
        let mut at = Some(node);
        while let Some(reference) = at {
            if seen.contains(&reference) {
                return Err(refused("this document's page tree holds a cycle"));
            }
            if seen.len() >= MOST_ANCESTORS {
                return Err(refused("this document's page tree is too deep to edit"));
            }
            seen.push(reference);
            self.slot(reference).2 += pages;
            at = self.parent_of(reference).ok();
        }
        Ok(())
    }

    pub(crate) fn writes(&self) -> Result<Vec<PlannedWrite>, SpikeError> {
        let mut writes = Vec::new();
        for (node, kids, gained) in &self.nodes {
            if kids.is_none() && *gained == 0 {
                continue;
            }
            let body = resolve(self.source, *node, self.credential)?;
            let mut edits: Vec<(&Object, String)> = Vec::new();
            if let Some(kids) = kids {
                let value = entry(&body, &body.value, b"/Kids")
                    .ok_or_else(|| refused("this document's page tree cannot be read"))?;
                let written: Vec<String> = kids
                    .iter()
                    .map(|kid| format!("{} {} R", kid.object_number(), kid.generation()))
                    .collect();
                edits.push((value, format!("[{}]", written.join(" "))));
            }
            if *gained != 0 {
                let value = entry(&body, &body.value, b"/Count").ok_or_else(|| {
                    refused("a node of this page tree does not say how many pages it holds")
                })?;
                edits.push((value, counted(&body, value, *gained)?.to_string()));
            }
            writes.push(rewritten(&body, &edits)?);
        }
        for (page, node) in &self.parents {
            let body = resolve(self.source, *page, self.credential)?;
            let value = entry(&body, &body.value, b"/Parent")
                .ok_or_else(|| refused("a page does not say which node holds it"))?;
            writes.push(rewritten(
                &body,
                &[(
                    value,
                    format!("{} {} R", node.object_number(), node.generation()),
                )],
            )?);
        }
        Ok(writes)
    }
}

fn counted(body: &Body, count: &Object, gained: i64) -> Result<i64, SpikeError> {
    let odd = || refused("a node of this page tree counts its pages oddly");
    if !matches!(
        count.kind(),
        ObjectKind::Number(pdf_syntax::NumberKind::Integer)
    ) {
        return Err(odd());
    }
    let held: i64 = body
        .source
        .resolve(count.span())
        .ok()
        .and_then(|bytes| std::str::from_utf8(bytes).ok()?.trim().parse().ok())
        .ok_or_else(odd)?;
    held.checked_add(gained)
        .filter(|count| *count >= 0)
        .ok_or_else(odd)
}

fn rewritten(body: &Body, edits: &[(&Object, String)]) -> Result<PlannedWrite, SpikeError> {
    let malformed = || refused("this document's page tree cannot be rewritten");
    let mut spans: Vec<(usize, usize, &str)> = Vec::with_capacity(edits.len());
    for (value, text) in edits {
        let span = value.span();
        let start = span
            .start()
            .checked_sub(body.offset)
            .ok_or_else(malformed)?;
        let end = span.end().checked_sub(body.offset).ok_or_else(malformed)?;
        if end > body.bytes.len() || start > end {
            return Err(malformed());
        }
        spans.push((start, end, text));
    }
    spans.sort_unstable_by_key(|(start, _, _)| *start);
    if spans.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err(malformed());
    }
    let mut bytes = Vec::with_capacity(body.bytes.len());
    let mut at = 0;
    for (start, end, text) in spans {
        bytes.extend_from_slice(&body.bytes[at..start]);
        bytes.extend_from_slice(text.as_bytes());
        at = end;
    }
    bytes.extend_from_slice(&body.bytes[at..]);
    Ok(PlannedWrite {
        reference: body.reference,
        body: PlannedBody::Direct { body: bytes },
    })
}

#[cfg(test)]
#[expect(
    clippy::float_cmp,
    reason = "a size written as a number is read back as the same number"
)]
mod tests {
    use pdf_bytes::{ByteStore, SourceId};
    use pdf_content::{PageContentLimits, count_pages_strict, load_page_program_strict};

    use crate::plan::Command;
    use crate::spike_move_text::plan_command;

    const A4: [f64; 2] = [595.28, 841.89];
    const LETTER: [f64; 2] = [612.0, 792.0];

    fn document(objects: &[String]) -> ByteStore {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
        }
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        ByteStore::new(SourceId::new(1), bytes)
    }

    fn two_pages() -> ByteStore {
        let content = "BT /F1 12 Tf 10 10 Td (A) Tj ET";
        let stream = format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        );
        document(&[
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /MediaBox [0 0 200 200] /Kids [3 0 R 4 0 R] /Count 2 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << >> >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << >> >>".to_owned(),
            stream.clone(),
            stream,
        ])
    }

    fn pages(source: &ByteStore) -> Vec<u32> {
        let count = count_pages_strict(source, PageContentLimits::default()).expect("counted");
        (0..count)
            .map(|page| {
                load_page_program_strict(source, page, PageContentLimits::default())
                    .expect("a page")
                    .page
                    .object_number()
            })
            .collect()
    }

    #[test]
    fn the_pages_of_a_document_that_asks_for_a_password_are_changed() {
        let password = b"panpdf";
        let locked = ByteStore::new(
            SourceId::new(7),
            crate::reprotect::rewrite(
                &two_pages(),
                b"",
                &crate::reprotect::Wanted::Protected(Box::new(pdf_security::Wanted {
                    user: password.to_vec(),
                    owner: b"owner".to_vec(),
                    allowed: pdf_security::Allowed::default(),
                })),
            )
            .expect("the document is protected"),
        );
        let count = |source: &ByteStore| {
            pdf_content::page_references_with_password(
                source,
                PageContentLimits::default(),
                password,
            )
            .expect("the pages are listed")
            .len()
        };
        for (command, pages) in [
            (
                Command::RotatePages {
                    pages: vec![0],
                    quarter_turns: 1,
                },
                2,
            ),
            (added(0, false, A4), 3),
            (Command::RemovePages { pages: vec![1] }, 1),
        ] {
            let plan = plan_command(&locked, &command, password).expect("the change is planned");
            let after = plan.commit(&locked, password).expect("commits");
            assert_eq!(count(&after), pages, "{command:?}");
            assert!(
                pdf_content::page_references_with_password(
                    &after,
                    PageContentLimits::default(),
                    b""
                )
                .is_err(),
                "and nothing of it reads without the password"
            );
        }
    }

    fn added(beside: usize, before: bool, size: [f64; 2]) -> Command {
        Command::AddBlankPage {
            beside,
            before,
            size,
        }
    }

    #[test]
    fn a_blank_page_goes_in_at_the_size_it_was_asked_for() {
        let source = two_pages();
        assert_eq!(pages(&source), vec![3, 4]);
        for (beside, before, size, wanted) in [
            (0, true, A4, vec![7, 3, 4]),
            (0, false, LETTER, vec![3, 7, 4]),
            (1, false, A4, vec![3, 4, 7]),
        ] {
            let plan = plan_command(&source, &added(beside, before, size), b"")
                .expect("the page is planned");
            let after = plan.commit(&source, b"").expect("the plan commits");
            assert_eq!(pages(&after), wanted, "{beside} before={before}");
            let at = wanted
                .iter()
                .position(|page| *page == 7)
                .expect("the new page");
            let put = load_page_program_strict(&after, at, PageContentLimits::default())
                .expect("the new page reads");
            assert_eq!(put.geometry.media_box, [0.0, 0.0, size[0], size[1]]);
            assert_eq!(put.streams.len(), 1);
            assert!(put.streams[0].bytes.as_bytes().is_empty());
        }
    }

    #[test]
    fn the_count_above_a_new_page_says_how_many_pages_there_are() {
        let source = two_pages();
        let plan = plan_command(&source, &added(0, true, A4), b"").expect("planned");
        let after = plan.commit(&source, b"").expect("commits");
        let written = String::from_utf8_lossy(after.as_bytes()).into_owned();
        let last = written
            .rfind("/Type /Pages")
            .expect("the node is written again");
        assert!(
            written[last..].contains("/Count 3"),
            "{}",
            &written[last..last + 120]
        );
    }

    #[test]
    fn every_node_above_a_new_page_counts_it() {
        let content = "BT /F1 12 Tf 10 10 Td (A) Tj ET";
        let stream = format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        );
        let source = document(&[
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /MediaBox [0 0 200 200] /Kids [3 0 R 6 0 R] /Count 3 >>".to_owned(),
            "<< /Type /Pages /Parent 2 0 R /Kids [4 0 R 5 0 R] /Count 2 >>".to_owned(),
            "<< /Type /Page /Parent 3 0 R /Contents 8 0 R /Resources << >> >>".to_owned(),
            "<< /Type /Page /Parent 3 0 R /Contents 8 0 R /Resources << >> >>".to_owned(),
            "<< /Type /Pages /Parent 2 0 R /Kids [7 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 6 0 R /Contents 8 0 R /Resources << >> >>".to_owned(),
            stream,
        ]);
        let plan = plan_command(&source, &added(1, false, A4), b"").expect("planned");
        let after = plan.commit(&source, b"").expect("commits");
        assert_eq!(pages(&after), vec![4, 5, 9, 7]);
        let written = String::from_utf8_lossy(after.as_bytes()).into_owned();
        let tail = &written[source.as_bytes().len()..];
        assert!(
            tail.contains("/Kids [4 0 R 5 0 R 9 0 R] /Count 3"),
            "{tail}"
        );
        assert!(tail.contains("/Kids [3 0 R 6 0 R] /Count 4"), "{tail}");
        assert!(!tail.contains("/Kids [7 0 R]"), "{tail}");
    }

    fn nested() -> ByteStore {
        let content = "BT /F1 12 Tf 10 10 Td (A) Tj ET";
        let stream = format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        );
        document(&[
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /MediaBox [0 0 200 200] /Kids [3 0 R 6 0 R] /Count 3 >>".to_owned(),
            "<< /Type /Pages /Parent 2 0 R /Kids [4 0 R 5 0 R] /Count 2 >>".to_owned(),
            "<< /Type /Page /Parent 3 0 R /Contents 8 0 R /Resources << >> >>".to_owned(),
            "<< /Type /Page /Parent 3 0 R /Contents 8 0 R /Resources << >> >>".to_owned(),
            "<< /Type /Pages /Parent 2 0 R /Kids [7 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 6 0 R /Contents 8 0 R /Resources << >> >>".to_owned(),
            stream,
        ])
    }

    #[test]
    fn a_page_taken_out_leaves_the_others_in_order() {
        let source = nested();
        for (page, wanted) in [(0, vec![5, 7]), (1, vec![4, 7]), (2, vec![4, 5])] {
            let plan = plan_command(&source, &Command::RemovePages { pages: vec![page] }, b"")
                .expect("the page is planned out");
            let after = plan.commit(&source, b"").expect("commits");
            assert_eq!(pages(&after), wanted, "page {page}");
        }
        let plan =
            plan_command(&source, &Command::RemovePages { pages: vec![2] }, b"").expect("planned");
        let after = plan.commit(&source, b"").expect("commits");
        let tail =
            String::from_utf8_lossy(&after.as_bytes()[source.as_bytes().len()..]).into_owned();
        assert!(tail.contains("/Kids [] /Count 0"), "{tail}");
        assert!(tail.contains("/Kids [3 0 R 6 0 R] /Count 2"), "{tail}");

        let one = document(&[
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /MediaBox [0 0 200 200] /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>".to_owned(),
            "<< /Length 0 >>\nstream\n\nendstream".to_owned(),
        ]);
        assert!(plan_command(&one, &Command::RemovePages { pages: vec![0] }, b"").is_err());
    }

    #[test]
    fn a_page_moved_lands_where_it_was_asked_to() {
        let source = nested();
        for (from, to, wanted) in [
            (0, 1, vec![5, 4, 7]),
            (0, 2, vec![5, 7, 4]),
            (2, 0, vec![7, 4, 5]),
            (1, 2, vec![4, 7, 5]),
        ] {
            let plan = plan_command(
                &source,
                &Command::MovePages {
                    pages: vec![from],
                    to,
                },
                b"",
            )
            .expect("the move is planned");
            let after = plan.commit(&source, b"").expect("commits");
            assert_eq!(pages(&after), wanted, "{from} -> {to}");
        }
        let plan = plan_command(
            &source,
            &Command::MovePages {
                pages: vec![2],
                to: 0,
            },
            b"",
        )
        .expect("planned");
        let after = plan.commit(&source, b"").expect("commits");
        let tail =
            String::from_utf8_lossy(&after.as_bytes()[source.as_bytes().len()..]).into_owned();
        assert!(
            tail.contains("/Kids [7 0 R 4 0 R 5 0 R] /Count 3"),
            "{tail}"
        );
        assert!(tail.contains("/Kids [] /Count 0"), "{tail}");
        assert!(tail.contains("/Type /Page /Parent 3 0 R"), "{tail}");
        assert!(
            !tail.contains("/Kids [3 0 R 6 0 R]"),
            "the root keeps its count: {tail}"
        );
        assert!(
            plan_command(
                &source,
                &Command::MovePages {
                    pages: vec![1],
                    to: 1
                },
                b""
            )
            .is_err()
        );
        assert!(
            plan_command(
                &source,
                &Command::MovePages {
                    pages: vec![1],
                    to: 3
                },
                b""
            )
            .is_err()
        );
    }

    #[test]
    fn a_page_turns_from_the_turn_it_had() {
        let content = "BT /F1 12 Tf 10 10 Td (A) Tj ET";
        let stream = format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        );
        let source = document(&[
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /MediaBox [0 0 200 100] /Rotate 90 /Kids [3 0 R 4 0 R] /Count 2 >>"
                .to_owned(),
            "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << >> >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /Rotate 270 /Contents 5 0 R /Resources << >> >>"
                .to_owned(),
            stream,
        ]);
        let turns = |source: &ByteStore| -> Vec<u16> {
            pdf_content::page_geometries_with_password(source, PageContentLimits::default(), b"")
                .expect("laid out")
                .iter()
                .map(|geometry| geometry.rotate)
                .collect()
        };
        assert_eq!(turns(&source), vec![90, 270]);
        for (page, quarter_turns, wanted) in [
            (0, 1, vec![180, 270]),
            (0, -1, vec![0, 270]),
            (1, 1, vec![90, 0]),
            (1, 2, vec![90, 90]),
        ] {
            let plan = plan_command(
                &source,
                &Command::RotatePages {
                    pages: vec![page],
                    quarter_turns,
                },
                b"",
            )
            .expect("the turn is planned");
            let after = plan.commit(&source, b"").expect("commits");
            assert_eq!(turns(&after), wanted, "page {page} by {quarter_turns}");
            assert_eq!(pages(&after), vec![3, 4]);
        }
        assert!(
            plan_command(
                &source,
                &Command::RotatePages {
                    pages: vec![0],
                    quarter_turns: 4
                },
                b""
            )
            .is_err()
        );
    }

    #[test]
    fn several_pages_are_one_edit() {
        let source = nested();
        let removed = plan_command(&source, &Command::RemovePages { pages: vec![0, 2] }, b"")
            .expect("planned");
        assert_eq!(
            removed.pages(),
            Some(&crate::plan::PageChange::Removed(vec![0, 2]))
        );
        assert_eq!(
            pages(&removed.commit(&source, b"").expect("commits")),
            vec![5]
        );
        assert!(
            plan_command(
                &source,
                &Command::RemovePages {
                    pages: vec![0, 1, 2]
                },
                b""
            )
            .is_err(),
            "every page"
        );
        assert!(
            plan_command(&source, &Command::RemovePages { pages: vec![1, 1] }, b"").is_err(),
            "a page twice"
        );

        for (moved, to, wanted) in [
            (vec![0, 2], 1, vec![5, 4, 7]),
            (vec![2, 0], 0, vec![7, 4, 5]),
            (vec![1, 2], 0, vec![5, 7, 4]),
        ] {
            let plan = plan_command(
                &source,
                &Command::MovePages {
                    pages: moved.clone(),
                    to,
                },
                b"",
            )
            .expect("planned");
            let after = plan.commit(&source, b"").expect("commits");
            assert_eq!(pages(&after), wanted, "{moved:?} to {to}");
            let crate::plan::PageChange::Reordered(order) =
                plan.pages().cloned().expect("a reorder")
            else {
                panic!("a move is a reorder");
            };
            for (old, new) in order.iter().enumerate() {
                assert_eq!(pages(&after)[*new], pages(&source)[old]);
            }
        }

        let plan = plan_command(
            &source,
            &Command::RotatePages {
                pages: vec![0, 2],
                quarter_turns: 1,
            },
            b"",
        )
        .expect("planned");
        let after = plan.commit(&source, b"").expect("commits");
        let turns: Vec<u16> =
            pdf_content::page_geometries_with_password(&after, PageContentLimits::default(), b"")
                .expect("laid out")
                .iter()
                .map(|geometry| geometry.rotate)
                .collect();
        assert_eq!(turns, vec![90, 0, 90]);
    }

    #[test]
    fn a_page_named_twice_by_its_node_is_refused() {
        let content = "BT /F1 12 Tf 10 10 Td (A) Tj ET";
        let stream = format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        );
        let source = document(&[
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /MediaBox [0 0 200 200] /Kids [3 0 R 3 0 R] /Count 2 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>".to_owned(),
            stream,
        ]);
        assert!(plan_command(&source, &added(0, false, A4), b"").is_err());
    }

    #[test]
    fn a_page_with_no_size_and_a_page_beside_nothing_are_refused() {
        let source = two_pages();
        for command in [
            added(0, true, [0.0, 841.89]),
            added(0, true, [595.28, f64::NAN]),
            added(2, true, A4),
        ] {
            assert!(
                plan_command(&source, &command, b"").is_err(),
                "{command:?} is written"
            );
        }
    }
}
