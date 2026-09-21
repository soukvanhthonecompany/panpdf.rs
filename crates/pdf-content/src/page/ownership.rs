use super::{PageContentError, PageContentErrorKind, PageContentLimits, dictionary, entry};
use pdf_bytes::ByteStore;
use pdf_syntax::{Object, ObjectKind, Reference, RevisionIndex};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

#[derive(Debug)]
pub(super) struct ContentOwnership {
    source: ByteStore,
    index: Arc<RevisionIndex>,
    root: Reference,
    limits: PageContentLimits,
    counts: OnceLock<Result<HashMap<Reference, usize>, PageContentError>>,
}

impl ContentOwnership {
    pub(super) fn new(
        source: ByteStore,
        index: Arc<RevisionIndex>,
        root: Reference,
        limits: PageContentLimits,
    ) -> Self {
        Self {
            source,
            index,
            root,
            limits,
            counts: OnceLock::new(),
        }
    }

    pub(super) fn is_exclusive(&self, reference: Reference) -> Result<bool, PageContentError> {
        self.counts
            .get_or_init(|| self.count())
            .as_ref()
            .map(|counts| counts.get(&reference) == Some(&1))
            .map_err(Clone::clone)
    }

    fn count(&self) -> Result<HashMap<Reference, usize>, PageContentError> {
        let mut pending = vec![self.root];
        let mut seen = HashSet::new();
        let mut counts = HashMap::new();
        let mut total = 0;
        while let Some(reference) = pending.pop() {
            if !seen.insert(reference) {
                return Err(PageContentError::new(PageContentErrorKind::PageTreeCycle));
            }
            if seen.len() > self.limits.max_page_tree_nodes {
                return Err(PageContentError::new(PageContentErrorKind::PageTreeLimit));
            }
            let node = self
                .index
                .resolve_object(&self.source, reference, self.limits.resolve)
                .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
            let entries = dictionary(node.value()).ok_or_else(|| {
                PageContentError::new(PageContentErrorKind::PageTreeNodeNotDictionary)
            })?;
            let kind = entry(entries, node.source(), b"/Type")
                .ok_or_else(|| PageContentError::new(PageContentErrorKind::MissingPageTreeType))?;
            if kind.name_equals(node.source(), b"/Page") {
                if let Some(contents) = entry(entries, node.source(), b"/Contents") {
                    let references = self.references(contents)?;
                    total += references.len();
                    if total > self.limits.max_page_tree_nodes {
                        return Err(PageContentError::new(
                            PageContentErrorKind::ContentStreamLimit,
                        ));
                    }
                    for reference in references {
                        *counts.entry(reference).or_insert(0) += 1;
                    }
                }
            } else if kind.name_equals(node.source(), b"/Pages") {
                let kids = entry(entries, node.source(), b"/Kids")
                    .ok_or_else(|| PageContentError::new(PageContentErrorKind::MissingKids))?;
                let ObjectKind::Array(kids) = kids.kind() else {
                    return Err(PageContentError::new(PageContentErrorKind::KidsNotArray));
                };
                if kids.len()
                    > self
                        .limits
                        .max_page_tree_nodes
                        .saturating_sub(pending.len())
                {
                    return Err(PageContentError::new(PageContentErrorKind::PageTreeLimit));
                }
                for kid in kids {
                    let ObjectKind::Reference(reference) = kid.kind() else {
                        return Err(PageContentError::new(PageContentErrorKind::KidNotReference));
                    };
                    pending.push(*reference);
                }
            } else {
                return Err(PageContentError::new(
                    PageContentErrorKind::InvalidPageTreeType,
                ));
            }
        }
        Ok(counts)
    }

    fn references(&self, value: &Object) -> Result<Vec<Reference>, PageContentError> {
        match value.kind() {
            ObjectKind::Null => Ok(Vec::new()),
            ObjectKind::Reference(reference) => {
                let resolved = self
                    .index
                    .resolve_object(&self.source, *reference, self.limits.resolve)
                    .map_err(|error| PageContentError::new(PageContentErrorKind::Resolve(error)))?;
                if matches!(resolved.value().kind(), ObjectKind::Array(_)) {
                    self.array_references(resolved.value())
                } else if resolved.stream().is_some() {
                    Ok(vec![*reference])
                } else {
                    Err(PageContentError::new(
                        PageContentErrorKind::ContentsNotReference,
                    ))
                }
            }
            ObjectKind::Array(_) => self.array_references(value),
            _ => Err(PageContentError::new(
                PageContentErrorKind::ContentsNotReference,
            )),
        }
    }

    fn array_references(&self, value: &Object) -> Result<Vec<Reference>, PageContentError> {
        let ObjectKind::Array(values) = value.kind() else {
            unreachable!()
        };
        if values.len() > self.limits.max_content_streams {
            return Err(PageContentError::new(
                PageContentErrorKind::ContentStreamLimit,
            ));
        }
        values
            .iter()
            .map(|value| match value.kind() {
                ObjectKind::Reference(reference) => {
                    let resolved = self
                        .index
                        .resolve_object(&self.source, *reference, self.limits.resolve)
                        .map_err(|error| {
                            PageContentError::new(PageContentErrorKind::Resolve(error))
                        })?;
                    if resolved.stream().is_none() {
                        return Err(PageContentError::new(
                            PageContentErrorKind::ContentsNotReference,
                        ));
                    }
                    Ok(*reference)
                }
                _ => Err(PageContentError::new(
                    PageContentErrorKind::ContentArrayEntryNotReference,
                )),
            })
            .collect()
    }
}
