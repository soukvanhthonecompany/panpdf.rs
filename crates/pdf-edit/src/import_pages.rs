use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_content::{
    PageContentLimits, RecoverLimits, page_geometries_recovering, page_references_recovering,
};
use pdf_syntax::{Object, ObjectKind, Reference, ResolveLimits, RevisionIndex};

use crate::page_tree::{TreeEdit, order, prove_order, refused};
use crate::plan::{Capability, Effect, PageChange, Plan, PlannedBody, PlannedWrite};
use crate::spike_move_text::{PlannerPage, SpikeError};

const MOST_OBJECTS: usize = 200_000;

const PAGE_KEYS_LEFT: &[&[u8]] = &[b"/Parent", b"/B", b"/StructParents"];

pub(crate) struct Imported<'a> {
    pub beside: usize,
    pub before: bool,
    pub document: &'a Arc<[u8]>,
    pub pages: &'a [usize],
}

pub(crate) fn plan_insert_pages(
    source: &ByteStore,
    page: PlannerPage<'_>,
    imported: &Imported<'_>,
) -> Result<Plan, SpikeError> {
    let other = ByteStore::new(
        SourceId::new(source.id().get().wrapping_add(0x5157)),
        Arc::clone(imported.document),
    );
    let (index, protected) = crate::previous::readable_index(&other, b"")
        .ok_or_else(|| refused("the other document cannot be read"))?;
    if protected.is_some() {
        return Err(refused(
            "pages of a protected document cannot be put into another yet",
        ));
    }
    let theirs: Vec<Reference> = page_references_recovering(
        &other,
        PageContentLimits::default(),
        RecoverLimits::default(),
        b"",
    )
    .map_err(|_| refused("the other document's pages cannot be listed"))?
    .into_parts()
    .0;
    let geometries = page_geometries_recovering(
        &other,
        PageContentLimits::default(),
        RecoverLimits::default(),
        b"",
    )
    .map_err(|_| refused("the other document's pages cannot be laid out"))?
    .into_parts()
    .0;
    if imported.pages.is_empty() || imported.pages.iter().any(|at| *at >= theirs.len()) {
        return Err(refused("the other document does not have a page named"));
    }
    let before = order(source)?;
    if before.get(imported.beside) != Some(&page.program.page) {
        return Err(refused("the page to put pages beside is not the page read"));
    }

    let mut copier = Copier {
        other: &other,
        index: &index,
        tree: tree_objects(&other, &index, &theirs)?,
        numbers: HashMap::new(),
        queue: VecDeque::new(),
        next: crate::block_rewrite::next_object_number(source)?,
        writes: Vec::new(),
    };
    let mut tree = TreeEdit::new(source);
    let neighbour = page.program.page;
    let node = tree.parent_of(neighbour)?;
    let mut made = Vec::with_capacity(imported.pages.len());
    for at in imported.pages {
        let copy = Reference::new(copier.take_number()?, 0);
        let body = copier.page_body(theirs[*at], &geometries[*at], node)?;
        copier.writes.push(PlannedWrite {
            reference: copy,
            body: PlannedBody::Direct { body },
        });
        made.push(copy);
    }
    copier.copy_reached()?;

    let mut previous: Option<Reference> = None;
    for copy in &made {
        match previous {
            None => tree.insert_new(*copy, neighbour, imported.before, node)?,
            Some(earlier) => tree.insert_new(*copy, earlier, false, node)?,
        }
        previous = Some(*copy);
    }
    let mut writes = copier.writes;
    writes.extend(tree.writes()?);

    let first = if imported.before {
        imported.beside
    } else {
        imported.beside + 1
    };
    let change = PageChange::Added((first..first + made.len()).collect());
    let document = crate::block_rewrite::commit_writes(source, &writes, page.restrictions)?;
    prove_order(&before, &document, &change, &made)?;
    for (offset, at) in imported.pages.iter().enumerate() {
        prove_same_paint(&other, *at, &document, first + offset)?;
    }
    Ok(Plan::new(
        Capability::Exact,
        writes,
        Effect {
            page_index: first,
            moved: Vec::new(),
            target_stream: made[0],
            declared_region: None,
        },
    )
    .with_pages(change))
}

fn tree_objects(
    other: &ByteStore,
    index: &RevisionIndex,
    pages: &[Reference],
) -> Result<HashSet<Reference>, SpikeError> {
    let mut objects: HashSet<Reference> = pages.iter().copied().collect();
    for page in pages {
        let mut at = parent(other, index, *page)?;
        let mut steps = 0;
        while let Some(node) = at {
            if !objects.insert(node) || steps > 64 {
                break;
            }
            steps += 1;
            at = parent(other, index, node)?;
        }
    }
    Ok(objects)
}

fn parent(
    other: &ByteStore,
    index: &RevisionIndex,
    object: Reference,
) -> Result<Option<Reference>, SpikeError> {
    let resolved = index
        .resolve_object(other, object, ResolveLimits::default())
        .map_err(|_| refused("the other document's page tree cannot be read"))?;
    Ok(
        entry_of(resolved.source(), resolved.value(), b"/Parent").and_then(|value| {
            match value.kind() {
                ObjectKind::Reference(reference) => Some(*reference),
                _ => None,
            }
        }),
    )
}

fn entry_of<'a>(source: &ByteStore, value: &'a Object, key: &[u8]) -> Option<&'a Object> {
    let ObjectKind::Dictionary(entries) = value.kind() else {
        return None;
    };
    entries
        .iter()
        .find(|entry| entry.key_equals(source, key))
        .map(pdf_syntax::DictionaryEntry::value)
}

struct Copier<'a> {
    other: &'a ByteStore,
    index: &'a RevisionIndex,
    tree: HashSet<Reference>,
    numbers: HashMap<Reference, Reference>,
    queue: VecDeque<Reference>,
    next: u32,
    writes: Vec<PlannedWrite>,
}

impl Copier<'_> {
    fn take_number(&mut self) -> Result<u32, SpikeError> {
        let number = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| refused("this document has no object numbers left"))?;
        Ok(number)
    }

    fn number_of(&mut self, reference: Reference) -> Result<Reference, SpikeError> {
        if let Some(copy) = self.numbers.get(&reference) {
            return Ok(*copy);
        }
        if self.numbers.len() >= MOST_OBJECTS {
            return Err(refused("the pages reach more objects than can be copied"));
        }
        let copy = Reference::new(self.take_number()?, 0);
        self.numbers.insert(reference, copy);
        self.queue.push_back(reference);
        Ok(copy)
    }

    fn page_body(
        &mut self,
        page: Reference,
        geometry: &pdf_content::PageGeometry,
        node: Reference,
    ) -> Result<Vec<u8>, SpikeError> {
        let resolved = self
            .index
            .resolve_object(self.other, page, ResolveLimits::default())
            .map_err(|_| refused("a page of the other document cannot be read"))?;
        let source = resolved.source().clone();
        let value = resolved.value().clone();
        let ObjectKind::Dictionary(entries) = value.kind() else {
            return Err(refused("a page of the other document is not a dictionary"));
        };
        let mut out = b"<<".to_vec();
        let has = |key: &[u8]| entries.iter().any(|entry| entry.key_equals(&source, key));
        let (resources, media, crop, rotate) = (
            has(b"/Resources"),
            has(b"/MediaBox"),
            has(b"/CropBox"),
            has(b"/Rotate"),
        );
        for entry in entries {
            if PAGE_KEYS_LEFT
                .iter()
                .any(|key| entry.key_equals(&source, key))
            {
                continue;
            }
            out.push(b' ');
            out.extend_from_slice(raw(&source, entry.key().span())?);
            out.push(b' ');
            self.value(&source, entry.value(), &mut out)?;
        }
        out.extend_from_slice(
            format!(" /Parent {} {} R", node.object_number(), node.generation()).as_bytes(),
        );
        if !resources {
            out.extend_from_slice(b" /Resources ");
            match self.inherited_resources(page)? {
                Some((from, inherited)) => self.value(&from, &inherited, &mut out)?,
                None => out.extend_from_slice(b"<< >>"),
            }
        }
        let rectangle = |[x0, y0, x1, y1]: [f64; 4]| format!("[{x0} {y0} {x1} {y1}]");
        if !media {
            out.extend_from_slice(
                format!(" /MediaBox {}", rectangle(geometry.media_box)).as_bytes(),
            );
        }
        #[expect(clippy::float_cmp, reason = "two readings of one file's numbers")]
        let cropped = geometry.crop_box != geometry.media_box;
        if !crop && cropped {
            out.extend_from_slice(format!(" /CropBox {}", rectangle(geometry.crop_box)).as_bytes());
        }
        if !rotate && geometry.rotate != 0 {
            out.extend_from_slice(format!(" /Rotate {}", geometry.rotate).as_bytes());
        }
        out.extend_from_slice(b" >>");
        Ok(out)
    }

    fn inherited_resources(
        &self,
        page: Reference,
    ) -> Result<Option<(ByteStore, Object)>, SpikeError> {
        let mut at = parent(self.other, self.index, page)?;
        let mut steps = 0;
        while let Some(node) = at {
            if steps > 64 {
                break;
            }
            steps += 1;
            let resolved = self
                .index
                .resolve_object(self.other, node, ResolveLimits::default())
                .map_err(|_| refused("the other document's page tree cannot be read"))?;
            if let Some(resources) = entry_of(resolved.source(), resolved.value(), b"/Resources") {
                return Ok(Some((resolved.source().clone(), resources.clone())));
            }
            at = parent(self.other, self.index, node)?;
        }
        Ok(None)
    }

    fn copy_reached(&mut self) -> Result<(), SpikeError> {
        while let Some(reference) = self.queue.pop_front() {
            let copy = self.numbers[&reference];
            let resolved = self
                .index
                .resolve_object(self.other, reference, ResolveLimits::default())
                .map_err(|_| refused("an object a copied page uses cannot be read"))?;
            let source = resolved.source().clone();
            let value = resolved.value().clone();
            if let Some(stream) = resolved.stream() {
                let ObjectKind::Dictionary(entries) = value.kind() else {
                    return Err(refused("a stream a copied page uses has no dictionary"));
                };
                let mut dictionary = Vec::new();
                for entry in entries {
                    if entry.key_equals(&source, b"/Length") {
                        continue;
                    }
                    dictionary.push(b' ');
                    dictionary.extend_from_slice(raw(&source, entry.key().span())?);
                    dictionary.push(b' ');
                    self.value(&source, entry.value(), &mut dictionary)?;
                }
                let data = raw(&source, stream.data_span())?.to_vec();
                self.writes.push(PlannedWrite {
                    reference: copy,
                    body: PlannedBody::NewStream {
                        dictionary,
                        decoded: data,
                    },
                });
            } else {
                let mut body = Vec::new();
                self.value(&source, &value, &mut body)?;
                self.writes.push(PlannedWrite {
                    reference: copy,
                    body: PlannedBody::Direct { body },
                });
            }
        }
        Ok(())
    }

    fn value(
        &mut self,
        source: &ByteStore,
        value: &Object,
        out: &mut Vec<u8>,
    ) -> Result<(), SpikeError> {
        match value.kind() {
            ObjectKind::Reference(reference) => {
                if self.tree.contains(reference) {
                    out.extend_from_slice(b"null");
                } else {
                    let copy = self.number_of(*reference)?;
                    out.extend_from_slice(
                        format!("{} {} R", copy.object_number(), copy.generation()).as_bytes(),
                    );
                }
            }
            ObjectKind::Array(items) => {
                out.push(b'[');
                for (at, item) in items.iter().enumerate() {
                    if at > 0 {
                        out.push(b' ');
                    }
                    self.value(source, item, out)?;
                }
                out.push(b']');
            }
            ObjectKind::Dictionary(entries) => {
                out.extend_from_slice(b"<<");
                for entry in entries {
                    out.push(b' ');
                    out.extend_from_slice(raw(source, entry.key().span())?);
                    out.push(b' ');
                    self.value(source, entry.value(), out)?;
                }
                out.extend_from_slice(b" >>");
            }
            _ => out.extend_from_slice(raw(source, value.span())?),
        }
        Ok(())
    }
}

fn raw(source: &ByteStore, span: pdf_bytes::SourceSpan) -> Result<&[u8], SpikeError> {
    source
        .resolve(span)
        .map_err(|_| refused("an object a copied page uses cannot be read"))
}

impl TreeEdit<'_> {
    pub(crate) fn insert_new(
        &mut self,
        page: Reference,
        neighbour: Reference,
        before: bool,
        node: Reference,
    ) -> Result<(), SpikeError> {
        self.made(page, node);
        self.insert(page, neighbour, before)
    }
}

fn prove_same_paint(
    other: &ByteStore,
    theirs: usize,
    document: &ByteStore,
    ours: usize,
) -> Result<(), SpikeError> {
    let Ok(original) = crate::spike_move_text::read_page(other, theirs, b"", None) else {
        return Ok(());
    };
    let copy = crate::spike_move_text::read_page(document, ours, b"", None)
        .map_err(|_| refused("a copied page does not read where its original does"))?;
    if pdf_paint::glyph_placement_signature(&original.graph)
        != pdf_paint::glyph_placement_signature(&copy.graph)
    {
        return Err(refused(
            "a copied page does not paint what its original paints",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[expect(
    clippy::float_cmp,
    reason = "a size written as a number is read back as the same number"
)]
mod tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};
    use pdf_content::{
        PageContentLimits, page_geometries_with_password, page_references_with_password,
    };

    use crate::plan::{Command, PageChange};
    use crate::spike_move_text::{plan_command, read_page};

    fn document(objects: &[String]) -> Vec<u8> {
        document_headed(b"%PDF-1.7\n", objects)
    }

    fn document_headed(header: &[u8], objects: &[String]) -> Vec<u8> {
        let mut bytes = header.to_vec();
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
        bytes
    }

    fn stream(content: &str) -> String {
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        )
    }

    fn ours() -> ByteStore {
        ByteStore::new(
            SourceId::new(1),
            document(&[
                "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
                "<< /Type /Pages /MediaBox [0 0 200 200] /Kids [3 0 R 4 0 R] /Count 2 >>"
                    .to_owned(),
                "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << >> >>".to_owned(),
                "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << >> >>".to_owned(),
                stream("0 0 0 rg 1 1 5 5 re f"),
            ]),
        )
    }

    fn theirs() -> Arc<[u8]> {
        Arc::from(document(&[
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /MediaBox [0 0 300 400] /Rotate 90 \
             /Resources << /Font << /F1 5 0 R >> >> /Kids [3 0 R 4 0 R] /Count 2 >>"
                .to_owned(),
            "<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /Contents 7 0 R /Annots [8 0 R] >>".to_owned(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
            stream("BT /F1 12 Tf 10 20 Td (Copied) Tj ET"),
            stream("BT /F1 9 Tf 30 40 Td (Second) Tj ET"),
            "<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /Dest [3 0 R /Fit] /P 4 0 R >>"
                .to_owned(),
        ]))
    }

    fn order(source: &ByteStore) -> Vec<u32> {
        page_references_with_password(source, PageContentLimits::default(), b"")
            .expect("listed")
            .iter()
            .map(|page| page.object_number())
            .collect()
    }

    fn text(source: &ByteStore, page: usize) -> Vec<(String, [i64; 2])> {
        let reading = read_page(source, page, b"", None).expect("the page reads");
        pdf_paint::glyph_placement_signature(&reading.graph)
            .iter()
            .map(|placed| (format!("{placed:?}"), [0, 0]))
            .collect()
    }

    #[test]
    fn pages_of_another_document_go_in_painting_what_they_painted() {
        let source = ours();
        let other = theirs();
        let plan = plan_command(
            &source,
            &Command::InsertPages {
                beside: 0,
                before: false,
                document: Arc::clone(&other),
                pages: vec![1, 0],
            },
            b"",
        )
        .expect("the pages are planned in");
        assert_eq!(plan.pages(), Some(&PageChange::Added(vec![1, 2])));
        let after = plan.commit(&source, b"").expect("commits");

        let pages = order(&after);
        assert_eq!(pages.len(), 4);
        assert_eq!(
            (pages[0], pages[3]),
            (3, 4),
            "our pages are where they were"
        );
        let geometries = page_geometries_with_password(&after, PageContentLimits::default(), b"")
            .expect("laid out");
        for copy in [1, 2] {
            assert_eq!(geometries[copy].media_box, [0.0, 0.0, 300.0, 400.0]);
            assert_eq!(geometries[copy].rotate, 90);
        }
        let their_source = ByteStore::new(SourceId::new(7), Arc::clone(&other));
        assert_eq!(
            text(&after, 1),
            text(&their_source, 1),
            "the second page first"
        );
        assert_eq!(text(&after, 2), text(&their_source, 0));

        let tail =
            String::from_utf8_lossy(&after.as_bytes()[source.as_bytes().len()..]).into_owned();
        assert_eq!(tail.matches("/BaseFont /Helvetica").count(), 1, "{tail}");
        assert!(tail.contains("/Dest [null /Fit] /P null"), "{tail}");
    }

    #[test]
    fn pages_go_in_from_a_document_whose_header_a_strict_read_refuses() {
        let other: Arc<[u8]> = Arc::from(document_headed(
            b"%PDF-1.3 \n",
            &[
                "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
                "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R \
                 /Resources << >> >>"
                    .to_owned(),
                stream("0 0 1 rg 10 10 50 50 re f"),
            ],
        ));
        let their_source = ByteStore::new(SourceId::new(7), Arc::clone(&other));
        assert!(
            page_references_with_password(&their_source, PageContentLimits::default(), b"")
                .is_err(),
            "a strict read refuses it"
        );
        let source = ours();
        let plan = plan_command(
            &source,
            &Command::InsertPages {
                beside: 0,
                before: true,
                document: other,
                pages: vec![0],
            },
            b"",
        )
        .expect("its page is planned in");
        assert_eq!(plan.pages(), Some(&PageChange::Added(vec![0])));
        let after = plan.commit(&source, b"").expect("commits");
        assert_eq!(order(&after).len(), 3);
    }

    #[test]
    fn a_page_that_is_not_there_is_refused() {
        let source = ours();
        for (beside, pages) in [(0, vec![2]), (0, vec![]), (5, vec![0])] {
            assert!(
                plan_command(
                    &source,
                    &Command::InsertPages {
                        beside,
                        before: true,
                        document: theirs(),
                        pages: pages.clone(),
                    },
                    b"",
                )
                .is_err(),
                "{beside} {pages:?}"
            );
        }
    }
}
