#![forbid(unsafe_code)]

use std::fmt;
use std::ops::Range;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SourceId {
    root: u64,
    derivation: u64,
}

impl SourceId {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self {
            root: value,
            derivation: 0,
        }
    }

    #[must_use]
    pub fn next_document() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1 << 32);
        Self::new(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }

    #[must_use]
    pub const fn derived(parent: Self, derivation: u64) -> Self {
        Self {
            root: parent.root,
            derivation,
        }
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.root
    }

    #[must_use]
    pub const fn derivation(self) -> u64 {
        self.derivation
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SourceSpan {
    source: SourceId,
    start: usize,
    end: usize,
}

impl SourceSpan {
    pub fn new(source: SourceId, start: usize, end: usize) -> Result<Self, SpanError> {
        if start > end {
            return Err(SpanError::Reversed { start, end });
        }

        Ok(Self { source, start, end })
    }

    #[must_use]
    pub const fn empty(source: SourceId) -> Self {
        Self {
            source,
            start: 0,
            end: 0,
        }
    }

    #[must_use]
    pub const fn source(self) -> SourceId {
        self.source
    }

    #[must_use]
    pub const fn start(self) -> usize {
        self.start
    }

    #[must_use]
    pub const fn end(self) -> usize {
        self.end
    }

    #[must_use]
    pub const fn len(self) -> usize {
        self.end - self.start
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }

    #[must_use]
    pub const fn range(self) -> Range<usize> {
        self.start..self.end
    }
}

#[derive(Clone)]
enum Held {
    Shared(Arc<[u8]>),
    Owned(Arc<Vec<u8>>),
}

impl Held {
    fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Shared(bytes) => bytes,
            Self::Owned(bytes) => bytes,
        }
    }
}

#[derive(Clone)]
pub struct ByteStore {
    id: SourceId,
    bytes: Held,
    end: usize,
}

impl PartialEq for ByteStore {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.end == other.end
    }
}

impl Eq for ByteStore {}

impl ByteStore {
    #[must_use]
    pub fn new(id: SourceId, bytes: impl Into<Arc<[u8]>>) -> Self {
        let bytes = bytes.into();
        Self {
            end: bytes.len(),
            bytes: Held::Shared(bytes),
            id,
        }
    }

    #[must_use]
    pub fn owning(id: SourceId, bytes: Vec<u8>) -> Self {
        Self {
            end: bytes.len(),
            bytes: Held::Owned(Arc::new(bytes)),
            id,
        }
    }

    #[must_use]
    pub fn prefix(&self, id: SourceId, len: usize) -> Option<Self> {
        (len <= self.end).then(|| Self {
            id,
            bytes: self.bytes.clone(),
            end: len,
        })
    }

    #[must_use]
    pub const fn id(&self) -> SourceId {
        self.id
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.end
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.end == 0
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes.as_bytes()[..self.end]
    }

    pub fn span(&self, range: Range<usize>) -> Result<SourceSpan, SpanError> {
        let span = SourceSpan::new(self.id, range.start, range.end)?;
        if span.end > self.len() {
            return Err(SpanError::OutOfBounds {
                end: span.end,
                source_len: self.len(),
            });
        }
        Ok(span)
    }

    pub fn resolve(&self, span: SourceSpan) -> Result<&[u8], SpanError> {
        if span.source != self.id {
            return Err(SpanError::WrongSource {
                expected: self.id,
                actual: span.source,
            });
        }
        if span.end > self.len() {
            return Err(SpanError::OutOfBounds {
                end: span.end,
                source_len: self.len(),
            });
        }
        Ok(&self.as_bytes()[span.range()])
    }
}

impl fmt::Debug for ByteStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ByteStore")
            .field("id", &self.id)
            .field("len", &self.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpanError {
    Reversed {
        start: usize,
        end: usize,
    },
    OutOfBounds {
        end: usize,
        source_len: usize,
    },
    WrongSource {
        expected: SourceId,
        actual: SourceId,
    },
}

impl fmt::Display for SpanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reversed { start, end } => {
                write!(
                    formatter,
                    "source span starts at {start} after ending at {end}"
                )
            }
            Self::OutOfBounds { end, source_len } => write!(
                formatter,
                "source span ends at {end}, beyond source length {source_len}"
            ),
            Self::WrongSource { expected, actual } => write!(
                formatter,
                "source span belongs to source {actual}, expected {expected}"
            ),
        }
    }
}

impl std::error::Error for SpanError {}

impl fmt::Display for SourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.root, self.derivation)
    }
}

#[cfg(test)]
mod tests {
    use super::{ByteStore, SourceId, SourceSpan, SpanError};

    #[test]
    fn resolves_a_valid_span_without_copying() {
        let store = ByteStore::new(SourceId::new(7), &b"%PDF-2.0\n"[..]);
        let span = store.span(1..4).expect("valid source range");

        assert_eq!(store.resolve(span), Ok(&b"PDF"[..]));
        assert_eq!(span.source(), SourceId::new(7));
    }

    #[test]
    fn rejects_a_span_from_another_revision() {
        let first = ByteStore::new(SourceId::new(1), &b"first"[..]);
        let second = ByteStore::new(SourceId::new(2), &b"second"[..]);
        let span = first.span(0..5).expect("valid source range");

        assert_eq!(
            second.resolve(span),
            Err(SpanError::WrongSource {
                expected: SourceId::new(2),
                actual: SourceId::new(1),
            })
        );
    }

    #[test]
    fn rejects_reversed_and_out_of_bounds_spans() {
        let store = ByteStore::new(SourceId::new(1), &b"abc"[..]);

        assert_eq!(
            SourceSpan::new(store.id(), 2, 1),
            Err(SpanError::Reversed { start: 2, end: 1 })
        );
        assert_eq!(
            store.span(0..4),
            Err(SpanError::OutOfBounds {
                end: 4,
                source_len: 3,
            })
        );
    }

    #[test]
    fn an_owned_vector_is_moved_rather_than_copied() {
        let bytes = vec![b'a'; 4096];
        let at = bytes.as_ptr().addr();
        let store = ByteStore::owning(SourceId::new(5), bytes);

        assert_eq!(store.as_bytes().as_ptr().addr(), at);
        assert_eq!(store.len(), 4096);
        assert_eq!(store.as_bytes(), &[b'a'; 4096][..]);
    }

    #[test]
    fn a_prefix_shares_one_allocation_and_stops_where_it_says() {
        let store = ByteStore::owning(SourceId::new(1), b"%PDF-1.7\nbody\nappended".to_vec());
        let prefix = store
            .prefix(SourceId::new(2), 14)
            .expect("a prefix no longer than its source");

        assert_eq!(prefix.as_bytes(), &b"%PDF-1.7\nbody\n"[..]);
        assert_eq!(prefix.len(), 14);
        assert_eq!(
            prefix.as_bytes().as_ptr().addr(),
            store.as_bytes().as_ptr().addr(),
            "a prefix must not copy the document"
        );
        assert!(store.as_bytes().starts_with(prefix.as_bytes()));
    }

    #[test]
    fn a_prefix_longer_than_its_source_is_refused() {
        let store = ByteStore::new(SourceId::new(1), &b"abc"[..]);

        assert!(store.prefix(SourceId::new(2), 4).is_none());
        assert!(store.prefix(SourceId::new(2), 3).is_some());
    }

    #[test]
    fn a_prefix_is_a_revision_of_its_own() {
        let store = ByteStore::owning(SourceId::new(1), b"abcdef".to_vec());
        let prefix = store.prefix(SourceId::new(2), 3).expect("a prefix");
        let span = store.span(0..6).expect("a span of the whole source");

        assert!(matches!(
            prefix.resolve(span),
            Err(SpanError::WrongSource { .. })
        ));
        let own = prefix.span(0..3).expect("a span of the prefix");
        assert_eq!(prefix.resolve(own), Ok(&b"abc"[..]));
        assert_eq!(
            prefix.span(0..4),
            Err(SpanError::OutOfBounds {
                end: 4,
                source_len: 3,
            })
        );
    }

    #[test]
    fn derived_sources_do_not_alias_their_parent() {
        let parent = SourceId::new(9);
        let derived = SourceId::derived(parent, 42);
        let parent_store = ByteStore::new(parent, &b"same"[..]);
        let derived_store = ByteStore::new(derived, &b"same"[..]);
        let parent_span = parent_store.span(0..4).expect("parent span");

        assert_eq!(derived.get(), parent.get());
        assert_eq!(derived.derivation(), 42);
        assert!(matches!(
            derived_store.resolve(parent_span),
            Err(SpanError::WrongSource { .. })
        ));
    }
}
