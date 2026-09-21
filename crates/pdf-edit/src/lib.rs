#![forbid(unsafe_code)]

use std::fmt;

mod block_move;
mod block_rewrite;
mod clip_region;
pub mod destination;
mod field_group;
mod field_look;
pub mod field_settings;
mod fill_field;
pub mod form;
mod form_edit;
pub use form_edit::scope_resources;
mod group_move;
pub mod history;
pub mod image_file;
mod import_pages;
pub mod incremental;
pub mod info;
pub mod layout;
pub mod link;
pub mod new_field;
mod new_font;
mod new_image;
mod new_path;
mod new_text;
mod object_edit;
pub mod outline;
mod page_tree;
mod place_object;
mod place_text;
pub mod plan;
pub mod png;
mod previous;
mod remove_object;
pub mod reprotect;
mod retype;
pub mod signature;
mod size_text;
pub use retype::{replacement_shift, substituted_family};
pub mod spike_move_text;
mod split;
pub mod stamp;
pub mod tab_order;
pub mod text_layer;

pub type Fonts<'a> = Option<&'a std::sync::Arc<dyn pdf_content::FontProvider>>;

pub use block_rewrite::turn::{out_of as turned_out_of, rotation, text_turn};
pub use block_rewrite::{
    BlockReading, ClusterFace, LineEnd, LiveBlock, LiveLine, LivePiece, ReadLine,
    in_compatibility_form, insertion_after_marks, read_block, read_block_faces,
};
pub use history::History;
pub use incremental::Restrictions;
pub use layout::{
    Alignment, Blocked, KEEP_CLEAR, WRAP_ROW, blocked_for_block, blocked_in_frame, shapes_over,
    widest_free_run,
};
pub use plan::{
    BlockOutcome, BlockRange, Capability, ClusterRef, ClustersAfter, Command, Effect, FixedPoint,
    GlyphChange, LINE_BREAK, ObjectSelection, PageChange, ParagraphLayout, PenBlend, PenStep,
    PenStroke, Plan, PlannedCaret, RowEnds, RunRewrite, Showing, SourceAnchor, TextRange,
    TextRunSelection, TextStyle,
};
use spike_move_text::{SpikeError, plan_command};

use pdf_bytes::ByteStore;
use pdf_syntax::{
    DocumentObjects, HeaderError, PdfHeader, RecoverError, RecoverLimits, Recovered, ResolveError,
    RevisionIndex, XrefError, XrefLimits, open_objects_recovering, parse_header_recovering,
    parse_header_strict, parse_revision_chain_strict,
};

#[derive(Clone, Debug)]
pub struct Document {
    source: ByteStore,
    header: PdfHeader,
    objects: DocumentObjects,
}

impl Document {
    pub fn open_strict(source: ByteStore, limits: XrefLimits) -> Result<Self, OpenError> {
        let header = parse_header_strict(&source).map_err(OpenError::Header)?;
        let chain = parse_revision_chain_strict(&source, limits).map_err(OpenError::Revisions)?;
        let index = RevisionIndex::from_chain(&chain).map_err(OpenError::Index)?;
        Ok(Self {
            source,
            header,
            objects: DocumentObjects::Revisions {
                chain: Box::new(chain),
                index,
            },
        })
    }

    pub fn open_recovering(
        source: ByteStore,
        limits: RecoverLimits,
    ) -> Result<Recovered<Self>, OpenError> {
        let (header, mut repairs) = parse_header_recovering(&source, limits)
            .map_err(OpenError::Header)?
            .into_parts();
        let (objects, object_repairs) = open_objects_recovering(&source, limits)
            .map_err(OpenError::Recover)?
            .into_parts();
        repairs.extend(object_repairs);
        Ok(Recovered::new(
            Self {
                source,
                header,
                objects,
            },
            repairs,
        ))
    }

    #[must_use]
    pub const fn source(&self) -> &ByteStore {
        &self.source
    }

    #[must_use]
    pub const fn header(&self) -> PdfHeader {
        self.header
    }

    #[must_use]
    pub const fn objects(&self) -> &DocumentObjects {
        &self.objects
    }

    #[must_use]
    pub const fn objects_were_scanned(&self) -> bool {
        matches!(self.objects, DocumentObjects::Scanned(_))
    }

    #[must_use]
    pub const fn begin_transaction(&self) -> Transaction<'_> {
        Transaction {
            document: self,
            plans: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub struct Transaction<'document> {
    document: &'document Document,
    plans: Vec<Plan>,
}

impl Transaction<'_> {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plans.is_empty()
    }

    pub fn plan(&self, command: &Command, credential: &[u8]) -> Result<Plan, SpikeError> {
        plan_command(&self.document.source, command, credential)
    }

    pub fn plan_with_fonts(
        &self,
        command: &Command,
        credential: &[u8],
        fonts: Option<std::sync::Arc<dyn pdf_content::FontProvider>>,
    ) -> Result<Plan, SpikeError> {
        spike_move_text::plan_command_with_fonts(&self.document.source, command, credential, fonts)
    }

    pub fn push(&mut self, plan: Plan) {
        self.plans.push(plan);
    }

    pub fn commit(self, credential: &[u8]) -> Result<CommitOutcome, SpikeError> {
        if self.plans.is_empty() {
            return Ok(CommitOutcome {
                source: self.document.source.clone(),
                revision_created: false,
            });
        }
        let mut source = self.document.source.clone();
        for plan in &self.plans {
            source = plan.commit(&source, credential)?;
        }
        Ok(CommitOutcome {
            source,
            revision_created: true,
        })
    }
}

#[derive(Clone, Debug)]
pub struct CommitOutcome {
    source: ByteStore,
    revision_created: bool,
}

impl CommitOutcome {
    #[must_use]
    pub const fn source(&self) -> &ByteStore {
        &self.source
    }

    #[must_use]
    pub const fn revision_created(&self) -> bool {
        self.revision_created
    }

    #[must_use]
    pub fn into_source(self) -> ByteStore {
        self.source
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenError {
    Header(HeaderError),
    Revisions(XrefError),
    Index(ResolveError),
    Recover(RecoverError),
}

impl fmt::Display for OpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Header(error) => write!(formatter, "header: {error}"),
            Self::Revisions(error) => write!(formatter, "revision chain: {error}"),
            Self::Index(error) => write!(formatter, "active object index: {error}"),
            Self::Recover(error) => write!(formatter, "recovery: {error}"),
        }
    }
}

impl std::error::Error for OpenError {}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};
    use pdf_syntax::{RecoverLimits, XrefLimits};

    use super::Document;

    fn binary_pdf() -> ByteStore {
        let mut bytes = b"%PDF-1.7\n%\x80\x81\x9e\x9f\n".to_vec();
        let catalog = bytes.len();
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog >>\nendobj\n");
        let xref = bytes.len();
        bytes.extend_from_slice(b"xref\n0 2\n0000000000 65535 f \n");
        bytes.extend_from_slice(format!("{catalog:010} 00000 n \n").as_bytes());
        bytes.extend_from_slice(b"trailer\n<< /Size 2 /Root 1 0 R >>\nstartxref\n");
        bytes.extend_from_slice(xref.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        ByteStore::new(SourceId::new(91), Arc::<[u8]>::from(bytes))
    }

    #[test]
    fn empty_commit_returns_the_identical_byte_store_without_serialization() {
        let source = binary_pdf();
        let original_pointer = source.as_bytes().as_ptr();
        let original_bytes = source.as_bytes().to_vec();
        let original_id = source.id();
        let document =
            Document::open_strict(source, XrefLimits::default()).expect("known valid document");
        let transaction = document.begin_transaction();
        assert!(transaction.is_empty());

        let outcome = transaction
            .commit(b"")
            .expect("an empty transaction serialises nothing");
        assert!(!outcome.revision_created());
        assert_eq!(outcome.source().id(), original_id);
        assert_eq!(outcome.source().as_bytes(), original_bytes);
        assert_eq!(outcome.source().as_bytes().as_ptr(), original_pointer);
    }

    #[test]
    fn a_recovered_document_still_returns_its_original_bytes_unchanged() {
        let source = binary_pdf();
        let mut bytes = source.as_bytes().to_vec();
        let keyword = b"startxref\n";
        let position = bytes
            .windows(keyword.len())
            .rposition(|window| window == keyword)
            .expect("the control document has a startxref");
        let digits_start = position + keyword.len();
        let digits_end = digits_start
            + bytes[digits_start..]
                .iter()
                .position(|byte| *byte == b'\n')
                .expect("the startxref value ends with a newline");
        bytes.splice(digits_start..digits_end, b"999999".iter().copied());

        let source = ByteStore::new(SourceId::new(92), Arc::<[u8]>::from(bytes));
        let original_pointer = source.as_bytes().as_ptr();
        let original_bytes = source.as_bytes().to_vec();
        let original_id = source.id();

        assert!(
            Document::open_strict(source.clone(), XrefLimits::default()).is_err(),
            "the control must be unopenable strictly, or it proves nothing"
        );

        let (document, repairs) = Document::open_recovering(source, RecoverLimits::default())
            .expect("the objects are intact")
            .into_parts();
        assert!(!repairs.is_empty());
        assert!(document.objects_were_scanned());

        let outcome = document
            .begin_transaction()
            .commit(b"")
            .expect("an empty transaction serialises nothing");
        assert!(!outcome.revision_created());
        assert_eq!(outcome.source().id(), original_id);
        assert_eq!(outcome.source().as_bytes(), original_bytes);
        assert_eq!(outcome.source().as_bytes().as_ptr(), original_pointer);
    }
}
