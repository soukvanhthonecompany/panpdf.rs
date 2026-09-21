use std::sync::Arc;

use pdf_bytes::SourceSpan;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Derived<T> {
    pub value: T,
    pub provenance: Provenance,
}

impl<T> Derived<T> {
    pub(crate) fn initial(value: T) -> Self {
        Self {
            value,
            provenance: Provenance::new(),
        }
    }

    pub(crate) fn assigned(value: T, provenance: SourceSpan) -> Self {
        Self {
            value,
            provenance: Provenance::of(provenance),
        }
    }

    pub(crate) fn assigned_from_resource(
        value: T,
        resource: SourceSpan,
        operation: SourceSpan,
    ) -> Self {
        Self {
            value,
            provenance: Provenance::of(resource).and(operation),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Provenance {
    head: Option<Arc<Node>>,
}

#[derive(Debug)]
struct Node {
    span: SourceSpan,
    len: usize,
    older: Option<Arc<Node>>,
}

impl Provenance {
    #[must_use]
    pub fn new() -> Self {
        Self { head: None }
    }

    #[must_use]
    pub fn of(span: SourceSpan) -> Self {
        Self::new().and(span)
    }

    #[must_use]
    pub fn and(&self, span: SourceSpan) -> Self {
        Self {
            head: Some(Arc::new(Node {
                span,
                len: self.len() + 1,
                older: self.head.clone(),
            })),
        }
    }

    pub fn push(&mut self, span: SourceSpan) {
        *self = self.and(span);
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.head.as_ref().map_or(0, |node| node.len)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.head.is_none()
    }

    #[must_use]
    pub fn first(&self) -> Option<&SourceSpan> {
        let mut node = self.head.as_ref()?;
        while let Some(older) = node.older.as_ref() {
            node = older;
        }
        Some(&node.span)
    }

    #[must_use]
    pub fn last(&self) -> Option<&SourceSpan> {
        self.head.as_ref().map(|node| &node.span)
    }

    #[must_use]
    pub fn to_vec(&self) -> Vec<SourceSpan> {
        let mut spans = Vec::with_capacity(self.len());
        let mut node = self.head.as_ref();
        while let Some(current) = node {
            spans.push(current.span);
            node = current.older.as_ref();
        }
        spans.reverse();
        spans
    }

    #[must_use]
    pub fn iter(&self) -> std::vec::IntoIter<SourceSpan> {
        self.to_vec().into_iter()
    }
}

impl Eq for Provenance {}

impl PartialEq for Provenance {
    fn eq(&self, other: &Self) -> bool {
        let mut left = self.head.as_ref();
        let mut right = other.head.as_ref();
        loop {
            match (left, right) {
                (None, None) => return true,
                (Some(a), Some(b)) => {
                    if Arc::ptr_eq(a, b) {
                        return true;
                    }
                    if a.len != b.len || a.span != b.span {
                        return false;
                    }
                    left = a.older.as_ref();
                    right = b.older.as_ref();
                }
                _ => return false,
            }
        }
    }
}

impl FromIterator<SourceSpan> for Provenance {
    fn from_iter<I: IntoIterator<Item = SourceSpan>>(spans: I) -> Self {
        let mut provenance = Self::new();
        for span in spans {
            provenance.push(span);
        }
        provenance
    }
}

impl From<Vec<SourceSpan>> for Provenance {
    fn from(spans: Vec<SourceSpan>) -> Self {
        spans.into_iter().collect()
    }
}

impl IntoIterator for &Provenance {
    type Item = SourceSpan;
    type IntoIter = std::vec::IntoIter<SourceSpan>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl Drop for Provenance {
    fn drop(&mut self) {
        let mut node = self.head.take();
        while let Some(current) = node {
            match Arc::try_unwrap(current) {
                Ok(mut owned) => node = owned.older.take(),
                Err(_) => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_bytes::SourceId;

    fn span(start: usize) -> SourceSpan {
        SourceSpan::new(SourceId::new(1), start, start + 1).expect("span")
    }

    #[test]
    fn a_history_reads_back_in_the_order_it_was_written() {
        let mut provenance = Provenance::new();
        assert!(provenance.is_empty());
        assert_eq!(provenance.first(), None);
        for start in 0..5 {
            provenance.push(span(start));
        }
        assert_eq!(provenance.len(), 5);
        assert_eq!(provenance.to_vec(), (0..5).map(span).collect::<Vec<_>>());
        assert_eq!(provenance.first(), Some(&span(0)));
        assert_eq!(provenance.last(), Some(&span(4)));
    }

    #[test]
    fn appending_to_a_clone_leaves_the_original_alone() {
        let first = Provenance::of(span(0)).and(span(1));
        let mut second = first.clone();
        second.push(span(2));
        assert_eq!(first.to_vec(), vec![span(0), span(1)]);
        assert_eq!(second.to_vec(), vec![span(0), span(1), span(2)]);
        assert_ne!(first, second);
    }

    #[test]
    fn two_histories_written_separately_are_equal_when_they_read_the_same() {
        let built = Provenance::of(span(7)).and(span(9));
        let collected: Provenance = vec![span(7), span(9)].into_iter().collect();
        assert_eq!(built, collected);
        assert_ne!(built, Provenance::of(span(9)).and(span(7)));
        assert_ne!(built, Provenance::of(span(7)));
    }

    #[test]
    fn a_long_history_is_shared_rather_than_copied() {
        let mut state = Provenance::new();
        for start in 0..1_000 {
            state.push(span(start));
        }
        let atoms: Vec<Provenance> = (0..1_000).map(|_| state.clone()).collect();
        let head = state.head.as_ref().expect("head");
        assert_eq!(Arc::strong_count(head), 1_001);
        assert!(atoms.iter().all(|atom| atom.len() == 1_000));
        assert_eq!(atoms[0], state);
    }

    #[test]
    fn a_history_longer_than_the_stack_can_recurse_still_drops() {
        let mut provenance = Provenance::new();
        for start in 0..200_000 {
            provenance.push(span(start));
        }
        assert_eq!(provenance.len(), 200_000);
        drop(provenance);
    }
}
