#![forbid(unsafe_code)]

use std::fmt;
use std::ops::Range;
use std::sync::{Arc, OnceLock};

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

    fn address(&self) -> *const u8 {
        self.as_bytes().as_ptr()
    }
}

#[derive(Clone)]
enum Body {
    One(Held),
    Pieces(Arc<Pieces>),
}

#[derive(Clone)]
struct Piece {
    at: usize,
    held: Held,
    len: usize,
}

impl Piece {
    fn bytes(&self) -> &[u8] {
        &self.held.as_bytes()[..self.len]
    }

    const fn end(&self) -> usize {
        self.at + self.len
    }
}

struct Pieces {
    pieces: Vec<Piece>,
    joined: OnceLock<Vec<u8>>,
}

const MOST_PIECES: usize = 16;

thread_local! {
    static WHOLE_COPIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[must_use]
pub fn whole_copies() -> usize {
    WHOLE_COPIES.with(std::cell::Cell::get)
}

impl Pieces {
    fn joined(&self) -> &[u8] {
        self.joined.get_or_init(|| {
            WHOLE_COPIES.with(|copies| copies.set(copies.get() + 1));
            let len = self.pieces.last().map_or(0, Piece::end);
            let mut joined = Vec::with_capacity(len);
            for piece in &self.pieces {
                joined.extend_from_slice(piece.bytes());
            }
            joined
        })
    }

    fn at(&self, offset: usize) -> (usize, &[u8]) {
        let found = self
            .pieces
            .partition_point(|piece| piece.at <= offset)
            .saturating_sub(1);
        let piece = &self.pieces[found];
        (piece.at, piece.bytes())
    }
}

#[derive(Clone)]
pub struct ByteStore {
    id: SourceId,
    body: Body,
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
            body: Body::One(Held::Shared(bytes)),
            id,
        }
    }

    #[must_use]
    pub fn owning(id: SourceId, bytes: Vec<u8>) -> Self {
        Self {
            end: bytes.len(),
            body: Body::One(Held::Owned(Arc::new(bytes))),
            id,
        }
    }

    #[must_use]
    pub fn followed_by(&self, id: SourceId, tail: Vec<u8>) -> Self {
        let mut pieces = self.pieces();
        if !tail.is_empty() {
            pieces.push(Piece {
                at: self.end,
                len: tail.len(),
                held: Held::Owned(Arc::new(tail)),
            });
        }
        if pieces.len() > MOST_PIECES {
            let after = pieces.split_off(1);
            let mut rest = Vec::new();
            for piece in &after {
                rest.extend_from_slice(piece.bytes());
            }
            pieces.push(Piece {
                at: after[0].at,
                len: rest.len(),
                held: Held::Owned(Arc::new(rest)),
            });
        }
        let end = pieces.last().map_or(0, Piece::end);
        let body = match <[Piece; 1]>::try_from(pieces) {
            Ok([piece]) => Body::One(piece.held),
            Err(pieces) if pieces.is_empty() => Body::One(Held::Shared(Arc::from(&[][..]))),
            Err(pieces) => Body::Pieces(Arc::new(Pieces {
                pieces,
                joined: OnceLock::new(),
            })),
        };
        Self { id, body, end }
    }

    fn pieces(&self) -> Vec<Piece> {
        let whole = match &self.body {
            Body::One(held) => vec![Piece {
                at: 0,
                held: held.clone(),
                len: self.end,
            }],
            Body::Pieces(pieces) => pieces.pieces.clone(),
        };
        let mut cut = Vec::with_capacity(whole.len() + 1);
        for mut piece in whole {
            if piece.at >= self.end {
                break;
            }
            piece.len = piece.len.min(self.end - piece.at);
            cut.push(piece);
        }
        cut
    }

    #[must_use]
    pub fn prefix(&self, id: SourceId, len: usize) -> Option<Self> {
        if len > self.end {
            return None;
        }
        let body = match &self.body {
            Body::Pieces(pieces) if pieces.pieces[0].len >= len => {
                Body::One(pieces.pieces[0].held.clone())
            }
            Body::One(_) | Body::Pieces(_) => self.body.clone(),
        };
        Some(Self { id, body, end: len })
    }

    #[must_use]
    pub const fn id(&self) -> SourceId {
        self.id
    }

    #[must_use]
    pub fn is_same(&self, other: &Self) -> bool {
        self == other && self.address() == other.address()
    }

    fn address(&self) -> *const u8 {
        match &self.body {
            Body::One(held) => held.address(),
            Body::Pieces(pieces) => Arc::as_ptr(pieces).cast(),
        }
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
        match &self.body {
            Body::One(held) => &held.as_bytes()[..self.end],
            Body::Pieces(pieces) => &pieces.joined()[..self.end],
        }
    }

    #[must_use]
    pub fn run_at(&self, offset: usize) -> (usize, &[u8]) {
        match &self.body {
            Body::One(held) => (0, &held.as_bytes()[..self.end]),
            Body::Pieces(pieces) => {
                if let Some(joined) = pieces.joined.get() {
                    return (0, &joined[..self.end]);
                }
                let (at, bytes) = pieces.at(offset.min(self.end.saturating_sub(1)));
                (at, &bytes[..bytes.len().min(self.end - at)])
            }
        }
    }

    pub fn runs(&self) -> impl Iterator<Item = &[u8]> {
        let mut at = 0;
        std::iter::from_fn(move || {
            if at >= self.end {
                return None;
            }
            let (start, bytes) = self.run_at(at);
            let run = &bytes[at - start..];
            at = start + bytes.len();
            Some(run)
        })
    }

    #[must_use]
    pub fn ahead(&self, start: usize, want: usize) -> &[u8] {
        let run = self.bytes_from(start);
        if run.len() >= want || start + run.len() >= self.end {
            run
        } else {
            &self.as_bytes()[start..]
        }
    }

    #[must_use]
    pub fn to_vec(&self) -> Vec<u8> {
        self.copy_of(0..self.end).unwrap_or_default()
    }

    #[must_use]
    pub fn copy_of(&self, range: Range<usize>) -> Option<Vec<u8>> {
        if range.start > range.end || range.end > self.end {
            return None;
        }
        let mut bytes = Vec::with_capacity(range.len());
        let mut at = range.start;
        while at < range.end {
            let (start, run) = self.run_at(at);
            let upto = (start + run.len()).min(range.end);
            bytes.extend_from_slice(&run[at - start..upto - start]);
            at = upto;
        }
        Some(bytes)
    }

    #[must_use]
    pub fn same_bytes_as(&self, bytes: &[u8]) -> bool {
        let mut at = 0;
        self.end == bytes.len()
            && self.runs().all(|run| {
                let same = bytes.get(at..at + run.len()) == Some(run);
                at += run.len();
                same
            })
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
        Ok(self.bytes_in(span.range()))
    }

    #[must_use]
    pub fn get(&self, range: Range<usize>) -> Option<&[u8]> {
        (range.start <= range.end && range.end <= self.end).then(|| self.bytes_in(range))
    }

    fn bytes_in(&self, range: Range<usize>) -> &[u8] {
        let (at, run) = self.run_at(range.start);
        if range.start >= at && range.end <= at + run.len() {
            &run[range.start - at..range.end - at]
        } else {
            &self.as_bytes()[range]
        }
    }

    #[must_use]
    pub fn bytes_from(&self, start: usize) -> &[u8] {
        if start > self.end {
            return &[];
        }
        let (at, run) = self.run_at(start);
        run.get(start - at..).unwrap_or(&[])
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
    use super::{Body, ByteStore, MOST_PIECES, SourceId, SourceSpan, SpanError};

    fn unjoined(store: &ByteStore) -> bool {
        matches!(&store.body, Body::Pieces(pieces) if pieces.joined.get().is_none())
    }

    fn in_pieces(bytes: &[u8], cuts: &[usize]) -> ByteStore {
        let mut from = 0;
        let mut store: Option<ByteStore> = None;
        for &cut in cuts.iter().chain(std::iter::once(&bytes.len())) {
            let piece = bytes[from..cut].to_vec();
            store = Some(match store {
                None => ByteStore::owning(SourceId::new(1), piece),
                Some(store) => store.followed_by(SourceId::new(1), piece),
            });
            from = cut;
        }
        store.expect("at least one piece")
    }

    #[test]
    fn a_source_in_pieces_reads_as_the_same_bytes_in_one() {
        let bytes = b"%PDF-1.7\n1 0 obj\n(one)\nendobj\n2 0 obj\n(two)\nendobj\n";
        let one = ByteStore::new(SourceId::new(1), &bytes[..]);
        for cuts in [&[9_usize][..], &[9, 25], &[1, 2, 3, 40], &[30]] {
            let pieces = in_pieces(bytes, cuts);
            assert_eq!(pieces, one);
            for start in 0..=bytes.len() {
                let (at, run) = pieces.run_at(start);
                assert!(at <= start.min(bytes.len() - 1), "{cuts:?} {start}");
                assert_eq!(run, &bytes[at..at + run.len()]);
                assert!(bytes[start..].starts_with(pieces.bytes_from(start)));
                let next = cuts.iter().copied().find(|cut| *cut > start);
                assert_eq!(
                    pieces.bytes_from(start).len(),
                    next.unwrap_or(bytes.len()) - start
                );
                for end in start..=next.unwrap_or(bytes.len()) {
                    let span = pieces.span(start..end).expect("inside");
                    assert_eq!(pieces.resolve(span), Ok(&bytes[start..end]));
                }
            }
            assert_eq!(pieces.runs().collect::<Vec<_>>().concat(), bytes.to_vec());
            assert_eq!(pieces.to_vec(), bytes.to_vec());
            for start in 0..=bytes.len() {
                for end in start..=bytes.len() {
                    assert_eq!(
                        pieces.copy_of(start..end).as_deref(),
                        Some(&bytes[start..end])
                    );
                }
            }
            let reversed = std::ops::Range { start: 3, end: 2 };
            assert_eq!(pieces.copy_of(reversed), None);
            assert_eq!(pieces.copy_of(0..bytes.len() + 1), None);
            assert!(pieces.same_bytes_as(bytes));
            assert!(!pieces.same_bytes_as(&bytes[1..]));
            let mut changed = bytes.to_vec();
            changed[start_of_last(cuts)] ^= 1;
            assert!(!pieces.same_bytes_as(&changed));
            assert!(
                unjoined(&pieces),
                "{cuts:?}: reading inside pieces joined them"
            );
            let copies = super::whole_copies();
            for start in 0..=bytes.len() {
                for end in start..=bytes.len() {
                    let span = pieces.span(start..end).expect("inside");
                    assert_eq!(
                        pieces.resolve(span),
                        one.resolve(one.span(start..end).expect("inside"))
                    );
                }
            }
            assert_eq!(pieces.as_bytes(), &bytes[..]);
            assert_eq!(super::whole_copies(), copies + 1, "joined once, then kept");
            assert!(
                !unjoined(&pieces),
                "as_bytes joins the pieces and keeps the join"
            );
            assert_eq!(pieces.run_at(start_of_last(cuts)), (0, &bytes[..]));
        }
    }

    fn start_of_last(cuts: &[usize]) -> usize {
        cuts.last().copied().unwrap_or(0)
    }

    #[test]
    fn only_a_span_across_two_pieces_joins_them() {
        let pieces = in_pieces(b"abcdef", &[3]);
        let inside = pieces.span(3..6).expect("inside");
        assert_eq!(pieces.resolve(inside), Ok(&b"def"[..]));
        assert!(unjoined(&pieces));
        let across = pieces.span(2..4).expect("inside");
        assert_eq!(pieces.resolve(across), Ok(&b"cd"[..]));
        assert!(!unjoined(&pieces));
    }

    #[test]
    fn a_revision_shares_the_file_it_follows() {
        let opened = ByteStore::owning(SourceId::new(1), b"%PDF-1.7\nbody".to_vec());
        let next = opened.followed_by(SourceId::new(2), b"\nmore".to_vec());
        let after = next.followed_by(SourceId::new(3), b"\nagain".to_vec());

        assert_eq!(after.len(), opened.len() + 11);
        assert_eq!(after.bytes_from(0), opened.as_bytes());
        let back = after
            .prefix(SourceId::new(1), opened.len())
            .expect("shorter");
        assert!(back.is_same(&opened), "the prefix is the opened allocation");
        assert!(!after.is_same(&after.followed_by(SourceId::new(3), Vec::new())));
        assert!(after.is_same(&after.clone()));
        assert!(unjoined(&after));
        assert_eq!(
            after
                .prefix(SourceId::new(9), 16)
                .expect("shorter")
                .as_bytes(),
            b"%PDF-1.7\nbody\nmo"
        );
    }

    #[test]
    fn pieces_past_the_most_are_joined_behind_the_first() {
        let opened = ByteStore::owning(SourceId::new(1), b"opened".to_vec());
        let mut store = opened.clone();
        let mut expected = b"opened".to_vec();
        for step in 0..3 * MOST_PIECES {
            let tail = format!("<{step}>").into_bytes();
            expected.extend_from_slice(&tail);
            store = store.followed_by(SourceId::new(1), tail);
            let Body::Pieces(pieces) = &store.body else {
                panic!("in pieces");
            };
            assert!(pieces.pieces.len() <= MOST_PIECES);
        }
        assert_eq!(store.runs().collect::<Vec<_>>().concat(), expected);
        assert!(
            store
                .prefix(SourceId::new(1), 6)
                .expect("shorter")
                .is_same(&opened)
        );
    }

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
    fn the_same_source_is_one_allocation_not_one_name() {
        let store = ByteStore::new(SourceId::new(1), &b"%PDF-1.7\nbody"[..]);
        let twin = ByteStore::new(SourceId::new(1), &b"%PDF-1.7\nbody"[..]);
        let other = ByteStore::new(SourceId::new(1), &b"%PDF-1.7\nBODY"[..]);

        assert!(store.is_same(&store.clone()));
        assert_eq!(store, other);
        assert!(!store.is_same(&other));
        assert!(!store.is_same(&twin));
        let prefix = store.prefix(SourceId::new(1), 8).expect("shorter");
        assert!(!store.is_same(&prefix));
        assert!(!store.is_same(&store.prefix(SourceId::new(2), store.len()).expect("whole")));
        assert!(store.is_same(&store.prefix(SourceId::new(1), store.len()).expect("whole")));
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
