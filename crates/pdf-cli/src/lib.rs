#![forbid(unsafe_code)]

pub mod census;

use std::fmt;
use std::sync::Arc;

use pdf_bytes::ByteStore;
use pdf_content::{ContentError, PageContentError};
pub use pdf_paint::{Code, Confidence, ToUnicode};
use pdf_paint::{PaintAtomKind, PaintGraph, paint_signature};
use pdf_render::{Canvas, RenderError, RenderOptions, RenderReport};

pub use pdf_session::{PageError, PageView, PaintFailure, Session};
use pdf_syntax::{
    DocumentObjects, HeaderError, Object, ObjectKind, PdfVersion, RecoverError, RecoverLimits,
    Reference, Repair, ResolveError, ResolveLimits, RevisionIndex, ScannedLocation, ScannedObjects,
    XrefEntryKind, XrefError, XrefLimits, XrefSection, open_objects_recovering,
    parse_header_recovering, parse_header_strict, parse_revision_chain_strict,
};

pub fn font_provider() -> Option<Arc<dyn pdf_content::FontProvider>> {
    static PROVIDER: std::sync::OnceLock<Option<Arc<dyn pdf_content::FontProvider>>> =
        std::sync::OnceLock::new();
    PROVIDER
        .get_or_init(|| {
            let setting = std::env::var("PANPDF_FONTS").unwrap_or_default();
            match setting.as_str() {
                "none" => None,
                "packaged" => packaged_provider(None).map(|provider| provider as Arc<_>),
                "system" => Some(Arc::new(pdf_content::SystemFontProvider::discover())),
                "" => {
                    let host: Arc<dyn pdf_content::FontProvider> =
                        Arc::new(pdf_content::SystemFontProvider::discover());
                    packaged_provider(None).map_or(Some(Arc::clone(&host)), |packaged| {
                        Some(Arc::new(FamilyFirstProvider { packaged, host }))
                    })
                }
                "packaged+system" => {
                    let host: Arc<dyn pdf_content::FontProvider> =
                        Arc::new(pdf_content::SystemFontProvider::discover());
                    packaged_provider(None).map_or(Some(Arc::clone(&host)), |packaged| {
                        Some(Arc::new(ChainProvider {
                            first: packaged,
                            then: host,
                        }))
                    })
                }
                roots => {
                    if let Some(directory) = roots.strip_prefix("packaged:") {
                        return packaged_provider(Some(std::path::Path::new(directory)))
                            .map(|provider| provider as Arc<_>);
                    }
                    let roots: Vec<std::path::PathBuf> = std::env::split_paths(roots).collect();
                    Some(Arc::new(pdf_content::SystemFontProvider::discover_in(
                        &roots,
                    )))
                }
            }
        })
        .clone()
}

#[derive(Debug)]
struct FamilyFirstProvider {
    packaged: Arc<pdf_content::SystemFontProvider>,
    host: Arc<dyn pdf_content::FontProvider>,
}

impl pdf_content::FontProvider for FamilyFirstProvider {
    fn primary_face(
        &self,
        request: &pdf_content::FontRequest,
    ) -> Option<pdf_content::SubstitutedFace> {
        self.packaged
            .primary_face(request)
            .filter(|face| pdf_content::outline_match::is_same_family(face, &request.family))
            .or_else(|| self.host.primary_face(request))
            .or_else(|| self.packaged.primary_face(request))
    }

    fn fallback_face(
        &self,
        request: &pdf_content::FontRequest,
        character: char,
    ) -> Option<pdf_content::SubstitutedFace> {
        self.host
            .fallback_face(request, character)
            .or_else(|| self.packaged.fallback_face(request, character))
    }

    fn description(&self) -> String {
        format!(
            "packaged faces of the requested family, then {}, then {}",
            self.host.description(),
            self.packaged.description()
        )
    }
}

#[derive(Debug)]
struct ChainProvider {
    first: Arc<pdf_content::SystemFontProvider>,
    then: Arc<dyn pdf_content::FontProvider>,
}

impl pdf_content::FontProvider for ChainProvider {
    fn primary_face(
        &self,
        request: &pdf_content::FontRequest,
    ) -> Option<pdf_content::SubstitutedFace> {
        self.first
            .primary_face(request)
            .or_else(|| self.then.primary_face(request))
    }

    fn fallback_face(
        &self,
        request: &pdf_content::FontRequest,
        character: char,
    ) -> Option<pdf_content::SubstitutedFace> {
        self.first
            .fallback_face(request, character)
            .or_else(|| self.then.fallback_face(request, character))
    }

    fn description(&self) -> String {
        format!(
            "{} then {}",
            self.first.description(),
            self.then.description()
        )
    }
}

#[must_use]
pub fn packaged_provider(
    directory: Option<&std::path::Path>,
) -> Option<Arc<pdf_content::SystemFontProvider>> {
    let root = match directory {
        Some(given) => given.to_path_buf(),
        None => default_package_root()?,
    };
    let manifest = std::fs::read_to_string(root.join("manifest.json")).ok()?;
    let faces = packaged_faces(&root.join("packaged"), &manifest);
    if faces.is_empty() {
        return None;
    }
    let (provider, problems) = pdf_content::SystemFontProvider::from_package(&faces);
    for problem in &problems {
        eprintln!("packaged font: {problem}");
    }
    Some(Arc::new(provider))
}

pub fn font_families() -> &'static [String] {
    static FAMILIES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    FAMILIES.get_or_init(|| {
        let mut families: Vec<String> = packaged_provider(None)
            .map(|provider| {
                provider
                    .faces()
                    .iter()
                    .map(|face| face.family.clone())
                    .collect()
            })
            .unwrap_or_default();
        families.extend(
            pdf_content::SystemFontProvider::discover()
                .faces()
                .iter()
                .map(|face| face.family.clone()),
        );
        families.retain(|family| !family.trim().is_empty());
        families.sort_by_key(|family| family.to_lowercase());
        families.dedup();
        families
    })
}

fn default_package_root() -> Option<std::path::PathBuf> {
    let mut candidates = vec![std::path::PathBuf::from("fonts")];
    if let Ok(executable) = std::env::current_exe()
        && let Some(directory) = executable.parent()
    {
        candidates.push(directory.join("fonts"));
        if let Some(up) = directory.parent() {
            candidates.push(up.join("fonts"));
        }
    }
    candidates
        .into_iter()
        .find(|candidate| candidate.join("manifest.json").is_file())
}

fn packaged_faces(directory: &std::path::Path, manifest: &str) -> Vec<pdf_content::PackagedFace> {
    let Some(list) = manifest.split("\"faces\"").nth(1) else {
        return Vec::new();
    };
    let mut faces = Vec::new();
    for chunk in list.split('{').skip(1) {
        let Some(file) = json_field(chunk, "file") else {
            continue;
        };
        let Some(sha256) = json_field(chunk, "sha256") else {
            continue;
        };
        let face_index = json_number(chunk, "face_index").unwrap_or(0);
        faces.push(pdf_content::PackagedFace {
            path: directory.join(file),
            face_index,
            sha256,
        });
    }
    faces
}

fn json_field(chunk: &str, key: &str) -> Option<String> {
    let rest = chunk
        .split(&format!("\"{key}\""))
        .nth(1)?
        .split(':')
        .nth(1)?;
    Some(rest.split('"').nth(1)?.to_owned())
}

fn json_number(chunk: &str, key: &str) -> Option<u32> {
    let rest = chunk
        .split(&format!("\"{key}\""))
        .nth(1)?
        .split(':')
        .nth(1)?;
    rest.trim_start()
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InspectLimits {
    pub xref: XrefLimits,
    pub resolve: ResolveLimits,
    pub recover: RecoverLimits,
    pub max_objects_to_inspect: usize,
    pub max_object_diagnostics: usize,
}

impl Default for InspectLimits {
    fn default() -> Self {
        Self {
            xref: XrefLimits::default(),
            resolve: ResolveLimits::default(),
            recover: RecoverLimits::default(),
            max_objects_to_inspect: 100_000,
            max_object_diagnostics: 100,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RevisionKind {
    Classic,
    Stream,
    Hybrid,
}

impl fmt::Display for RevisionKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Classic => "classic",
            Self::Stream => "stream",
            Self::Hybrid => "hybrid",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectSource {
    Revisions,
    Scan,
}

impl fmt::Display for ObjectSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Revisions => "cross-reference revisions",
            Self::Scan => "scan (no revision provenance)",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RevisionSummary {
    pub kind: RevisionKind,
    pub byte_offset: usize,
    pub entries: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ObjectSummary {
    pub direct: usize,
    pub compressed: usize,
    pub free: usize,
}

impl ObjectSummary {
    #[must_use]
    pub const fn active(self) -> usize {
        self.direct + self.compressed
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectResolutionError {
    Revision(ResolveError),
    Scan(RecoverError),
}

impl fmt::Display for ObjectResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Revision(error) => error.fmt(formatter),
            Self::Scan(error) => error.fmt(formatter),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectDiagnostic {
    pub reference: Reference,
    pub error: ObjectResolutionError,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Inspection {
    pub version: PdfVersion,
    pub byte_len: usize,
    pub object_source: ObjectSource,
    pub startxref: Option<usize>,
    pub revisions: Vec<RevisionSummary>,
    pub objects: ObjectSummary,
    pub repairs: Vec<Repair>,
    pub encrypted: Option<bool>,
    pub signatures_found: usize,
    pub signature_objects_inspected: usize,
    pub signature_scan_complete: bool,
    pub unresolved_objects: usize,
    pub object_diagnostics: Vec<ObjectDiagnostic>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PagePaintInspection {
    pub page: Reference,
    pub content_streams: usize,
    pub decoded_bytes: usize,
    pub operations: usize,
    pub paint_atoms: usize,
    pub paint_signatures: Vec<String>,
}

pub fn render_page_strict(
    source: &ByteStore,
    page_index: usize,
    scale: f64,
) -> Result<(Canvas, RenderReport), PagePaintInspectError> {
    let page = page_paint_graph_strict(source, page_index)?;
    render_page_view(&page, scale)
}

pub fn render_region_view(
    page: &PageView,
    scale: f64,
    region: [f64; 4],
) -> Result<Option<(Canvas, RenderReport)>, PagePaintInspectError> {
    let device = pdf_render::DeviceTransform::for_page(
        &page.program.geometry,
        scale,
        pdf_render::RenderLimits::default(),
    )
    .map_err(PagePaintInspectError::Render)?;
    let Some(pixels) = device.pixel_box(region) else {
        return Ok(None);
    };
    let options = RenderOptions {
        scale,
        ..RenderOptions::default()
    };
    pdf_render::render_region_layers(&page.layers(), &page.program.geometry, options, pixels)
        .map(Some)
        .map_err(PagePaintInspectError::Render)
}

pub fn render_region_pixels_view(
    page: &PageView,
    scale: f64,
    region: [u32; 4],
) -> Result<(Canvas, RenderReport), PagePaintInspectError> {
    let options = RenderOptions {
        scale,
        ..RenderOptions::default()
    };
    pdf_render::render_region_layers(&page.layers(), &page.program.geometry, options, region)
        .map_err(PagePaintInspectError::Render)
}

pub fn render_page_view(
    page: &PageView,
    scale: f64,
) -> Result<(Canvas, RenderReport), PagePaintInspectError> {
    let options = RenderOptions {
        scale,
        ..RenderOptions::default()
    };
    pdf_render::render_page_layers(&page.layers(), &page.program.geometry, options)
        .map_err(PagePaintInspectError::Render)
}

fn page_paint_graph_strict(
    source: &ByteStore,
    page_index: usize,
) -> Result<PageView, PagePaintInspectError> {
    pdf_session::interpret_page_fully(source, page_index, b"", None, font_provider())
        .map_err(PagePaintInspectError::from)
}

pub struct GraphComparison {
    pub compared: usize,
    pub other_atoms_identical: bool,
    pub first_difference: Option<(usize, String, String)>,
    pub offset_exact: bool,
    pub offset_detail: String,
}

pub fn compare_paint_graphs(
    before: &ByteStore,
    after: &ByteStore,
    edited_ordinal: usize,
    offset: f64,
) -> Result<GraphComparison, String> {
    let original = page_paint_graph_strict(before, 0).map_err(|error| error.to_string())?;
    let edited = page_paint_graph_strict(after, 0).map_err(|error| error.to_string())?;
    if original.graph.atoms.len() != edited.graph.atoms.len() {
        return Err(format!(
            "atom count changed: {} -> {}",
            original.graph.atoms.len(),
            edited.graph.atoms.len()
        ));
    }
    let mut compared = 0_usize;
    let mut other_atoms_identical = true;
    let mut first_difference = None;
    for (index, (left, right)) in original
        .graph
        .atoms
        .iter()
        .zip(&edited.graph.atoms)
        .enumerate()
    {
        if index == edited_ordinal {
            continue;
        }
        compared += 1;
        let (before, after) = (paint_signature(&left.kind), paint_signature(&right.kind));
        if before != after {
            other_atoms_identical = false;
            if first_difference.is_none() {
                first_difference = Some((index, before, after));
            }
        }
    }
    let (offset_exact, offset_detail) = match (
        &original.graph.atoms[edited_ordinal].kind,
        &edited.graph.atoms[edited_ordinal].kind,
    ) {
        (PaintAtomKind::Text(left), PaintAtomKind::Text(right)) => {
            let original = left.matrices.text.value;
            let expected_x = original.a.mul_add(offset, 0.0);
            let expected_y = original.b.mul_add(offset, 0.0);
            let moved_x = right.matrices.text.value.e - original.e;
            let moved_y = right.matrices.text.value.f - original.f;
            let exact = (moved_x - expected_x).abs() < 1e-6 && (moved_y - expected_y).abs() < 1e-6;
            let to_user = left.state.ctm.value.multiply(original);
            let user_x = to_user.a.mul_add(offset, 0.0);
            let user_y = to_user.b.mul_add(offset, 0.0);
            (
                exact,
                format!(
                    "moved by ({moved_x}, {moved_y}) in the text matrix's target space, which is {offset} in text space scaled by {}; that is ({user_x:.3}, {user_y:.3}) in default user space",
                    original.a
                ),
            )
        }
        _ => (false, "edited atom is not a text run".to_owned()),
    };
    Ok(GraphComparison {
        compared,
        other_atoms_identical,
        first_difference,
        offset_exact,
        offset_detail,
    })
}

pub fn inspect_page_paint_strict(
    source: &ByteStore,
    page_index: usize,
) -> Result<PagePaintInspection, PagePaintInspectError> {
    let page = page_paint_graph_strict(source, page_index)?;
    Ok(PagePaintInspection {
        page: page.program.page,
        content_streams: page.program.streams.len(),
        decoded_bytes: page
            .program
            .streams
            .iter()
            .map(|stream| stream.bytes.len())
            .sum(),
        operations: page.operation_count(),
        paint_atoms: page.graph.atoms.len(),
        paint_signatures: page
            .graph
            .atoms
            .iter()
            .map(|atom| paint_signature(&atom.kind))
            .collect(),
    })
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PageClusterInspection {
    pub text_runs: usize,
    pub clusters: usize,
    pub stacked_clusters: usize,
    pub marks: usize,
    pub runs_without_glyphs: usize,
    pub marks_by_width: usize,
    pub marks_by_position: usize,
    pub clusters_reordered_by_paint: usize,
    pub lines: usize,
    pub lines_split_by_gap: usize,
    pub longest_line: usize,
    pub blocks: usize,
    pub blocks_of_one_line: usize,
    pub lines_split_by_column: usize,
    pub largest_block: usize,
}

pub fn inspect_page_clusters_strict(
    source: &ByteStore,
    page_index: usize,
) -> Result<PageClusterInspection, PagePaintInspectError> {
    let page = page_paint_graph_strict(source, page_index)?;
    let index = pdf_semantics::SemanticIndex::of(&page.graph);
    Ok(PageClusterInspection {
        text_runs: index.report.runs_clustered + index.report.runs_without_glyphs,
        clusters: index.clusters.len(),
        stacked_clusters: index
            .clusters
            .iter()
            .filter(|cluster| cluster.is_stacked())
            .count(),
        marks: index.report.marks,
        runs_without_glyphs: index.report.runs_without_glyphs,
        marks_by_width: index.report.marks_by_width,
        marks_by_position: index.report.marks_by_position,
        clusters_reordered_by_paint: index.report.marks_reordered_by_paint,
        lines: index.lines.len(),
        lines_split_by_gap: index.report.lines_split_by_gap,
        blocks: index.blocks.len(),
        blocks_of_one_line: index.report.blocks_of_one_line,
        lines_split_by_column: index.report.lines_split_by_column,
        largest_block: index
            .blocks
            .iter()
            .map(|block| block.lines.len())
            .max()
            .unwrap_or(0),
        longest_line: index
            .lines
            .iter()
            .map(|line| line.clusters.len())
            .max()
            .unwrap_or(0),
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PagePaintInspectError {
    Page(PageContentError),
    Content(ContentError),
    Paint(Box<PaintFailure>),
    Render(RenderError),
}

impl From<PageError> for PagePaintInspectError {
    fn from(error: PageError) -> Self {
        match error {
            PageError::Page(error) => Self::Page(error),
            PageError::Content(error) => Self::Content(error),
            PageError::Paint(failure) => Self::Paint(failure),
        }
    }
}

impl fmt::Display for PagePaintInspectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Page(error) => write!(formatter, "page content: {error}"),
            Self::Content(error) => write!(formatter, "content syntax: {error}"),
            Self::Paint(failure) => match &failure.operator {
                Some(operator) => write!(
                    formatter,
                    "paint interpretation at operator {operator:?}: {}",
                    failure.error
                ),
                None => write!(formatter, "paint interpretation: {}", failure.error),
            },
            Self::Render(error) => write!(formatter, "rasterisation: {error}"),
        }
    }
}

impl std::error::Error for PagePaintInspectError {}

pub fn inspect_strict(
    source: &ByteStore,
    limits: InspectLimits,
) -> Result<Inspection, InspectError> {
    let header = parse_header_strict(source).map_err(InspectError::Header)?;
    let chain =
        parse_revision_chain_strict(source, limits.xref).map_err(InspectError::Revisions)?;
    let index = RevisionIndex::from_chain(&chain).map_err(InspectError::Index)?;

    let mut inspection = inspect_revisions(source, &chain, &index, &limits);
    inspection.version = header.version();
    inspection.byte_len = source.len();
    Ok(inspection)
}

pub fn inspect_recovering(
    source: &ByteStore,
    limits: InspectLimits,
) -> Result<Inspection, InspectError> {
    let (header, mut repairs) = parse_header_recovering(source, limits.recover)
        .map_err(InspectError::Header)?
        .into_parts();
    let (objects, object_repairs) = open_objects_recovering(source, limits.recover)
        .map_err(InspectError::Recover)?
        .into_parts();
    repairs.extend(object_repairs);

    let mut inspection = match &objects {
        DocumentObjects::Revisions { chain, index } => {
            inspect_revisions(source, chain, index, &limits)
        }
        DocumentObjects::Scanned(scanned) => inspect_scanned(source, scanned, &limits),
    };
    inspection.version = header.version();
    inspection.byte_len = source.len();
    inspection.repairs = repairs;
    Ok(inspection)
}

fn inspect_revisions(
    source: &ByteStore,
    chain: &pdf_syntax::RevisionChain,
    index: &RevisionIndex,
    limits: &InspectLimits,
) -> Inspection {
    let revisions = chain
        .revisions()
        .iter()
        .map(|section| RevisionSummary {
            kind: match section {
                XrefSection::Classic(_) => RevisionKind::Classic,
                XrefSection::Stream(_) => RevisionKind::Stream,
                XrefSection::Hybrid(_) => RevisionKind::Hybrid,
            },
            byte_offset: section.byte_offset(),
            entries: section.entries().len(),
        })
        .collect();
    let encrypted = chain
        .revisions()
        .iter()
        .any(|section| dictionary_has_key(source, section.trailer(), b"/Encrypt"));

    let mut selected: Vec<_> = index.selected_entries().collect();
    selected.sort_unstable_by_key(|entry| entry.entry().object_number());
    let mut objects = ObjectSummary::default();
    for selected_entry in &selected {
        match selected_entry.entry().kind() {
            XrefEntryKind::Free { .. } => objects.free += 1,
            XrefEntryKind::InUse { .. } => objects.direct += 1,
            XrefEntryKind::Compressed { .. } => objects.compressed += 1,
        }
    }

    let mut scan = SignatureScan::default();
    if !encrypted {
        for selected_entry in selected
            .iter()
            .filter(|entry| !matches!(entry.entry().kind(), XrefEntryKind::Free { .. }))
            .take(limits.max_objects_to_inspect)
        {
            let entry = selected_entry.entry();
            let reference = Reference::new(entry.object_number(), entry.generation());
            scan.inspected += 1;
            match index.resolve_object(source, reference, limits.resolve) {
                Ok(object) => scan.count_signature(object.source(), object.value()),
                Err(error) => scan.record_failure(
                    reference,
                    ObjectResolutionError::Revision(error),
                    limits.max_object_diagnostics,
                ),
            }
        }
    }

    Inspection {
        version: PdfVersion::new(0, 0),
        byte_len: source.len(),
        object_source: ObjectSource::Revisions,
        startxref: Some(chain.startxref()),
        revisions,
        objects,
        repairs: Vec::new(),
        encrypted: Some(encrypted),
        signatures_found: scan.signatures_found,
        signature_objects_inspected: scan.inspected,
        signature_scan_complete: !encrypted
            && scan.inspected == objects.active()
            && scan.unresolved == 0,
        unresolved_objects: scan.unresolved,
        object_diagnostics: scan.diagnostics,
    }
}

fn inspect_scanned(
    source: &ByteStore,
    scanned: &ScannedObjects,
    limits: &InspectLimits,
) -> Inspection {
    let mut entries: Vec<_> = scanned.entries().collect();
    entries.sort_unstable_by_key(|entry| entry.reference().object_number());

    let mut objects = ObjectSummary::default();
    for entry in &entries {
        match entry.location() {
            ScannedLocation::Direct { .. } => objects.direct += 1,
            ScannedLocation::InObjectStream { .. } => objects.compressed += 1,
        }
    }

    let mut scan = SignatureScan::default();
    for entry in entries.iter().take(limits.max_objects_to_inspect) {
        scan.inspected += 1;
        if let Err(error) = scanned.resolve_object(source, entry.reference(), limits.recover) {
            scan.record_failure(
                entry.reference(),
                ObjectResolutionError::Scan(error),
                limits.max_object_diagnostics,
            );
        }
    }

    Inspection {
        version: PdfVersion::new(0, 0),
        byte_len: source.len(),
        object_source: ObjectSource::Scan,
        startxref: None,
        revisions: Vec::new(),
        objects,
        repairs: Vec::new(),
        encrypted: None,
        signatures_found: 0,
        signature_objects_inspected: 0,
        signature_scan_complete: false,
        unresolved_objects: scan.unresolved,
        object_diagnostics: scan.diagnostics,
    }
}

#[derive(Default)]
struct SignatureScan {
    signatures_found: usize,
    inspected: usize,
    unresolved: usize,
    diagnostics: Vec<ObjectDiagnostic>,
}

impl SignatureScan {
    fn count_signature(&mut self, source: &ByteStore, object: &Object) {
        if is_signature_dictionary(source, object) {
            self.signatures_found += 1;
        }
    }

    fn record_failure(
        &mut self,
        reference: Reference,
        error: ObjectResolutionError,
        max_diagnostics: usize,
    ) {
        self.unresolved += 1;
        if self.diagnostics.len() < max_diagnostics {
            self.diagnostics.push(ObjectDiagnostic { reference, error });
        }
    }
}

fn dictionary_has_key(source: &ByteStore, object: &Object, key: &[u8]) -> bool {
    let ObjectKind::Dictionary(entries) = object.kind() else {
        return false;
    };
    entries.iter().any(|entry| entry.key_equals(source, key))
}

fn is_signature_dictionary(source: &ByteStore, object: &Object) -> bool {
    let ObjectKind::Dictionary(entries) = object.kind() else {
        return false;
    };
    entries.iter().any(|entry| {
        (entry.key_equals(source, b"/Type") || entry.key_equals(source, b"/FT"))
            && entry.value().name_equals(source, b"/Sig")
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InspectError {
    Header(HeaderError),
    Revisions(XrefError),
    Index(ResolveError),
    Recover(RecoverError),
}

impl fmt::Display for InspectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Header(error) => write!(formatter, "header: {error}"),
            Self::Revisions(error) => write!(formatter, "revision chain: {error}"),
            Self::Index(error) => write!(formatter, "active object index: {error}"),
            Self::Recover(error) => write!(formatter, "recovery: {error}"),
        }
    }
}

impl std::error::Error for InspectError {}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};

    use super::{
        InspectLimits, ObjectSource, RevisionKind, atoms_at, device_box, inspect_page_paint_strict,
        inspect_recovering, inspect_strict, page_atom_boxes, page_clusters_between, page_overlay,
        page_run_offset,
    };

    fn assert_box(got: [f64; 4], want: [f64; 4]) {
        for (index, (left, right)) in got.iter().zip(&want).enumerate() {
            assert!(
                (left - right).abs() < 1e-9,
                "corner {index}: got {got:?}, want {want:?}"
            );
        }
    }

    fn rotating_device(matrix: pdf_paint::Matrix) -> pdf_render::DeviceTransform {
        pdf_render::DeviceTransform {
            matrix,
            width: 100,
            height: 100,
            scale: 1.0,
        }
    }

    #[test]
    fn a_device_box_is_rebuilt_from_corners_rather_than_from_transformed_extremes() {
        let quarter_turn = pdf_paint::Matrix {
            a: 0.0,
            b: 1.0,
            c: -1.0,
            d: 0.0,
            e: 0.0,
            f: 0.0,
        };
        let placed = device_box(&rotating_device(quarter_turn), [1.0, 2.0, 5.0, 4.0]);
        assert_box(placed, [-4.0, 1.0, -2.0, 5.0]);
        assert!(
            placed[0] < placed[2] && placed[1] < placed[3],
            "still a box"
        );
    }

    #[test]
    fn a_y_flip_places_the_box_without_inverting_it() {
        let flip = pdf_paint::Matrix {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: -1.0,
            e: 0.0,
            f: 100.0,
        };
        let placed = device_box(&rotating_device(flip), [10.0, 20.0, 30.0, 40.0]);
        assert_box(placed, [10.0, 60.0, 30.0, 80.0]);
    }

    fn fixture() -> ByteStore {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let signature = bytes.len();
        bytes.extend_from_slice(b"1 0 obj\n<< /Ty#70e /Sig >>\nendobj\n");
        let encryption = bytes.len();
        bytes.extend_from_slice(b"2 0 obj\n<< /Filter /Standard >>\nendobj\n");
        let xref = bytes.len();
        bytes.extend_from_slice(b"xref\n0 3\n0000000000 65535 f \n");
        bytes.extend_from_slice(format!("{signature:010} 00000 n \n").as_bytes());
        bytes.extend_from_slice(format!("{encryption:010} 00000 n \n").as_bytes());
        bytes.extend_from_slice(b"trailer\n<< /Size 3 /Enc#72ypt 2 0 R >>\nstartxref\n");
        bytes.extend_from_slice(xref.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        ByteStore::new(SourceId::new(41), Arc::<[u8]>::from(bytes))
    }

    #[test]
    fn atom_boxes_place_a_rectangle_where_it_is_painted() {
        let boxes = page_atom_boxes(&page_fixture(), 0, 1.0).expect("page places its atoms");
        assert_eq!(boxes.len(), 1, "one `re f` is one atom: {boxes:?}");
        assert_eq!(boxes[0].kind, "path");
        assert_eq!(boxes[0].ordinal, 0);
        assert_eq!(boxes[0].depth, 0);
        assert_box(
            boxes[0].box_pixels.expect("a filled rect has an extent"),
            [10.0, 40.0, 40.0, 80.0],
        );
    }

    #[test]
    fn invisible_words_are_boxed_where_they_stand() {
        use pdf_edit::text_layer::{LayerWord, TextLayer};
        let content = b"0.25 g 10 20 30 40 re f";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>".to_owned(),
            format!(
                "<< /Length {} >>\nstream\n{}\nendstream",
                content.len(),
                String::from_utf8_lossy(content)
            ),
        ];
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
        }
        let xref = bytes.len();
        bytes.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        let mut session = pdf_session::Session::new(
            ByteStore::new(SourceId::new(56), Arc::<[u8]>::from(bytes)),
            b"",
        );
        session
            .apply_each(&[pdf_edit::plan::Command::TextLayer {
                page_index: 0,
                layer: TextLayer {
                    words: vec![LayerWord {
                        text: "Hello ".to_owned(),
                        frame: [20.0, 50.0, 80.0, 62.0],
                    }],
                },
                share_from: None,
            }])
            .expect("the layer is written");
        let overlay = page_overlay(session.source(), 0, 1.0).expect("the page interprets");
        let boxes: Vec<[f64; 4]> = overlay
            .clusters
            .iter()
            .map(|cluster| cluster.box_pixels.expect("an invisible cluster has a box"))
            .collect();
        assert_eq!(boxes.len(), 6, "{boxes:?}");
        assert_box(boxes[0], [20.0, 38.0, 30.0, 50.0]);
        assert_box(boxes[4], [60.0, 38.0, 70.0, 50.0]);
    }

    #[test]
    fn atom_boxes_follow_the_scale_they_were_asked_for() {
        let boxes = page_atom_boxes(&page_fixture(), 0, 2.0).expect("page places its atoms");
        assert_box(
            boxes[0].box_pixels.expect("a filled rect has an extent"),
            [20.0, 80.0, 80.0, 160.0],
        );
    }

    #[test]
    fn a_hit_test_answers_both_ways() {
        let boxes = page_atom_boxes(&page_fixture(), 0, 1.0).expect("page places its atoms");
        assert_eq!(atoms_at(&boxes, 25.0, 60.0).len(), 1, "inside the rect");
        assert!(atoms_at(&boxes, 5.0, 60.0).is_empty(), "left of the rect");
        assert!(atoms_at(&boxes, 25.0, 20.0).is_empty(), "above the rect");
        assert!(
            atoms_at(&boxes, 199.0, 99.0).is_empty(),
            "the far corner of the page paints nothing"
        );
    }

    #[test]
    fn the_box_list_is_never_shorter_than_what_the_renderer_visited() {
        let source = page_fixture();
        let boxes = page_atom_boxes(&source, 0, 1.0).expect("page places its atoms");
        let (_, report) = super::render_page_strict(&source, 0, 1.0).expect("page rasterises");
        assert!(
            boxes.len() >= report.visited,
            "{} boxes against {} visited",
            boxes.len(),
            report.visited
        );
        assert_eq!(
            boxes.len(),
            report.visited,
            "this page has no group to refuse"
        );
    }

    #[test]
    fn an_atom_painted_by_the_page_has_no_enclosing_group() {
        let boxes = page_atom_boxes(&page_fixture(), 0, 1.0).expect("page places its atoms");
        assert_eq!(boxes[0].parent, None);
        assert_eq!(boxes[0].depth, 0);
    }

    const SQUARE_CFF: &str = "0100040100010101055465737400010101131d00000030111d000000540f1d0000\
        0059100001010106616c70686100000003010102101e0e8b8b15f8888b8bf888fc888b050e8b8b15f8888b8bf888fc\
        888b050e0000220187000141";

    const NAMELESS_CFF: &str = "0100040100010101055465737400010101131d00000030111d000000540f1d0000\
        0059100001010106673132333400000003010102101e0e8b8b15f8888b8bf888fc888b050e8b8b15f8888b8bf888fc\
        888b050e0001870022000141";

    fn hex(text: &str) -> Vec<u8> {
        text.bytes()
            .filter(u8::is_ascii_hexdigit)
            .collect::<Vec<u8>>()
            .chunks_exact(2)
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).expect("hex digits"), 16)
                    .expect("hex byte")
            })
            .collect()
    }

    fn object(bytes: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]) {
        offsets.push(bytes.len());
        bytes.extend_from_slice(body);
    }

    fn text_page_that_says_what_it_means(content: &[u8]) -> ByteStore {
        let cmap = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap                      /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def                      /CMapName /Adobe-Identity-UCS def /CMapType 2 def                      1 begincodespacerange <00> <ff> endcodespacerange                      4 beginbfchar <41> <0041> <42> <0042> <43> <0031> <44> <0031> endbfchar                      endcmap end end"
            .to_vec();
        text_page_with(content, Some(&cmap), SQUARE_CFF)
    }

    fn text_page(content: &[u8]) -> ByteStore {
        text_page_with(content, None, NAMELESS_CFF)
    }

    fn text_page_with(content: &[u8], to_unicode: Option<&[u8]>, program: &str) -> ByteStore {
        let program = hex(program);
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        object(
            &mut bytes,
            &mut offsets,
            b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        );
        object(
            &mut bytes,
            &mut offsets,
            b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
        );
        object(
            &mut bytes,
            &mut offsets,
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        );
        bytes.extend_from_slice(content);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        object(
            &mut bytes,
            &mut offsets,
            format!(
                "5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Test /FirstChar 65 \
                 /LastChar 68 /Widths [600 600 600 600] /FontDescriptor 6 0 R{} >>\nendobj\n",
                if to_unicode.is_some() {
                    " /ToUnicode 8 0 R"
                } else {
                    ""
                }
            )
            .as_bytes(),
        );
        object(
            &mut bytes,
            &mut offsets,
            b"6 0 obj\n<< /Type /FontDescriptor /FontName /Test /Flags 4 /FontFile3 7 0 R >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!(
                "7 0 obj\n<< /Subtype /Type1C /Length {} >>\nstream\n",
                program.len()
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(&program);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        if let Some(cmap) = to_unicode {
            offsets.push(bytes.len());
            bytes.extend_from_slice(
                format!("8 0 obj\n<< /Length {} >>\nstream\n", cmap.len()).as_bytes(),
            );
            bytes.extend_from_slice(cmap);
            bytes.extend_from_slice(b"\nendstream\nendobj\n");
        }
        let size = offsets.len() + 1;
        let xref = bytes.len();
        bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n").as_bytes(),
        );
        bytes.extend_from_slice(xref.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        ByteStore::new(SourceId::new(613), Arc::<[u8]>::from(bytes))
    }

    fn two_pictures_in_one_place() -> ByteStore {
        let content = b"q 80 0 0 80 20 10 cm /Lower Do Q\nq 80 0 0 80 20 10 cm /Upper Do Q\n";
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        object(
            &mut bytes,
            &mut offsets,
            b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        );
        object(
            &mut bytes,
            &mut offsets,
            b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
        );
        object(
            &mut bytes,
            &mut offsets,
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject \
              << /Lower 5 0 R /Upper 6 0 R >> >> >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        );
        bytes.extend_from_slice(content);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        for number in 5..=6 {
            offsets.push(bytes.len());
            bytes.extend_from_slice(
                format!(
                    "{number} 0 obj\n<< /Type /XObject /Subtype /Image /Width 1 /Height 1 \
                     /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length 3 >>\nstream\n"
                )
                .as_bytes(),
            );
            bytes.extend_from_slice(&[10, 20, 30]);
            bytes.extend_from_slice(b"\nendstream\nendobj\n");
        }
        let size = offsets.len() + 1;
        let xref = bytes.len();
        bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n").as_bytes(),
        );
        bytes.extend_from_slice(xref.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        ByteStore::new(SourceId::new(614), Arc::<[u8]>::from(bytes))
    }

    fn drawn_page(content: &[u8]) -> ByteStore {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for body in [
            b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".as_slice(),
            b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 200] /Kids [3 0 R] /Count 1 >>\nendobj\n",
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n",
        ] {
            offsets.push(bytes.len());
            bytes.extend_from_slice(body);
        }
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        );
        bytes.extend_from_slice(content);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        let size = offsets.len() + 1;
        let xref = bytes.len();
        bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n")
                .as_bytes(),
        );
        ByteStore::new(SourceId::new(615), Arc::<[u8]>::from(bytes))
    }

    #[test]
    fn a_turned_drawing_is_framed_along_its_own_axes() {
        let turn = std::f64::consts::FRAC_1_SQRT_2;
        let content = format!("q {turn} {turn} -{turn} {turn} 100 100 cm -40 -5 80 10 re f Q");
        let mut session = crate::Session::new(drawn_page(content.as_bytes()), b"");
        let view = session.page(0).expect("the fixture is a page");
        let overlay = crate::page_overlay_view(&view, 1.0).expect("it has an overlay");
        assert_eq!(overlay.objects.len(), 1, "{:?}", overlay.objects);
        let [origin, along, _, up] = overlay.objects[0].quad;
        let (dx, dy) = (along[0] - origin[0], along[1] - origin[1]);
        assert!(
            (dx.hypot(dy) - 80.0).abs() < 1e-6,
            "{:?}",
            overlay.objects[0].quad
        );
        assert!(
            (dx - 80.0 * turn).abs() < 1e-6 && (dy + 80.0 * turn).abs() < 1e-6,
            "{dx} {dy}"
        );
        assert!(up[1] < origin[1], "its own up runs up the screen");

        let flipped = b"q 1 0 0 -1 0 200 cm 20 20 80 10 re f Q";
        let mut session = crate::Session::new(drawn_page(flipped), b"");
        let view = session.page(0).expect("the fixture is a page");
        let overlay = crate::page_overlay_view(&view, 1.0).expect("it has an overlay");
        let [origin, along, _, up] = overlay.objects[0].quad;
        assert!(
            (along[0] - origin[0] - 80.0).abs() < 1e-6,
            "{:?}",
            overlay.objects[0].quad
        );
        assert!(
            up[1] < origin[1],
            "flipped content still has its up upwards: {:?}",
            overlay.objects[0].quad
        );
    }

    #[test]
    fn a_turned_drawing_is_offered_by_its_own_area() {
        let turn = std::f64::consts::FRAC_1_SQRT_2;
        let offered = |content: &str| {
            let mut session = crate::Session::new(drawn_page(content.as_bytes()), b"");
            let view = session.page(0).expect("the fixture is a page");
            crate::page_overlay_view(&view, 1.0)
                .expect("it has an overlay")
                .objects
                .len()
        };
        assert_eq!(offered("20 90 160 20 re f"), 1, "control: upright");
        assert_eq!(
            offered(&format!(
                "q {turn} {turn} -{turn} {turn} 100 100 cm -80 -10 160 20 re f Q"
            )),
            1,
            "turned"
        );
        assert_eq!(offered("25 25 150 150 re f"), 0, "the paper");
    }

    #[test]
    fn two_pictures_in_the_same_place_are_two_rows_and_two_targets() {
        let mut session = crate::Session::new(two_pictures_in_one_place(), b"");
        let view = session.page(0).expect("the fixture is a page");
        let overlay = crate::page_overlay_view(&view, 1.0).expect("it has an overlay");

        assert_eq!(overlay.objects.len(), 2, "the fixture paints two pictures");
        assert_eq!(
            overlay.objects[0].quad, overlay.objects[1].quad,
            "the fixture is only a test of identity if the two coincide"
        );
        assert_ne!(
            overlay.objects[0].anchor, overlay.objects[1].anchor,
            "and they are painted by two different pieces of the source"
        );

        let rows: Vec<&crate::PanelRow> = overlay
            .rows
            .iter()
            .filter(|row| row.kind == pdf_semantics::ObjectKind::Image)
            .collect();
        assert_eq!(rows.len(), 2, "one row each");
        let targets: Vec<Option<usize>> = rows.iter().map(|row| row.movable).collect();
        assert_ne!(
            targets[0], targets[1],
            "both rows selected the same picture"
        );
        for row in rows {
            let target = row.movable.expect("a picture is a canvas target");
            assert_eq!(
                overlay.objects[target].object, row.object,
                "the row selected an object other than the one it names"
            );
        }
    }

    #[test]
    fn typing_says_which_of_the_five_answers_it_is() {
        let source = text_page_that_says_what_it_means(b"BT /F1 12 Tf 10 50 Td (ABCD) Tj ET");
        let mut session = crate::Session::new(source, b"");
        let view = session.page(0).expect("the fixture is a page");
        let overlay = crate::page_overlay_view(&view, 1.0).expect("it has an overlay");
        let ask = |offset: usize, typed: &str| {
            let Some(anchor) = crate::anchor_under_caret(&overlay.clusters, 0, offset) else {
                return crate::Typing::Nowhere;
            };
            let Some(run) = overlay.runs.iter().find(|run| run.anchor == anchor) else {
                return crate::Typing::Nowhere;
            };
            crate::typing_verdict(&run.text, typed)
        };
        assert_eq!(
            overlay.clusters[0].text.as_deref(),
            Some("A"),
            "a cluster says what the file says it says"
        );

        assert_eq!(ask(0, "AB"), crate::Typing::WithinTheFont { codes: 2 });
        assert_eq!(
            ask(4, "A"),
            crate::Typing::WithinTheFont { codes: 1 },
            "a caret at the end of the row types into the run the pen is in"
        );
        assert_eq!(
            ask(0, "Z"),
            crate::Typing::NeedsANewGlyph { character: 'Z' },
            "a character this font has no code for is Class C"
        );
        assert_eq!(
            ask(0, "1"),
            crate::Typing::Ambiguous {
                character: '1',
                codes: 2
            },
            "two codes say `1`, and G08 forbids picking one of them silently"
        );
        assert_eq!(
            crate::selection_text(&overlay.clusters, 0, 0, 3).as_deref(),
            Some("AB1"),
            "a selection says what its clusters say, in row order"
        );
        assert_eq!(ask(0, ""), crate::Typing::Nowhere);
        assert_eq!(ask(9, "A"), crate::Typing::Nowhere, "no cluster is there");

        let plain = text_page(b"BT /F1 12 Tf 10 50 Td (ABCD) Tj ET");
        let mut session = crate::Session::new(plain, b"");
        let view = session.page(0).expect("the fixture is a page");
        let overlay = crate::page_overlay_view(&view, 1.0).expect("it has an overlay");
        assert_eq!(
            crate::typing_verdict(&overlay.runs[0].text, "A"),
            crate::Typing::NoMeaning { refused: false },
            "a file that declared nothing is not a file whose CMap we rejected"
        );
        assert_eq!(
            overlay.clusters[0].text, None,
            "and the cluster says nothing rather than guessing at `A`"
        );
        let named = text_page_with(b"BT /F1 12 Tf 10 50 Td (ABCD) Tj ET", None, SQUARE_CFF);
        let mut named_session = crate::Session::new(named, b"");
        let named_view = named_session.page(0).expect("the fixture is a page");
        let named_overlay = crate::page_overlay_view(&named_view, 1.0).expect("it has an overlay");
        assert_eq!(named_overlay.clusters[0].text.as_deref(), Some("A"));
        assert_eq!(
            crate::selection_text(&overlay.clusters, 0, 0, 3),
            None,
            "one unknown cluster makes the whole selection unusable to a command"
        );
        assert_eq!(
            crate::read_selection(&overlay.clusters, 0, 0, 3),
            Some(crate::SelectionReading {
                shown: "\u{fffd}\u{fffd}\u{fffd}".to_owned(),
                unknown: 3
            }),
            "and a person is still told how much of it there is"
        );
    }

    #[test]
    fn the_overlay_and_the_index_agree_about_every_selection() {
        let source = text_page(b"BT /F1 12 Tf 10 50 Td (AB) Tj (AB) Tj ET");
        let mut session = crate::Session::new(source, b"");
        let view = session.page(0).expect("the fixture is a page");
        let overlay = crate::page_overlay_view(&view, 1.0).expect("it has an overlay");
        let rows = view.index.lines.len();
        assert!(rows > 0, "the fixture paints a row");

        let (mut yes, mut no) = (0_usize, 0_usize);
        for line in 0..rows {
            let width = view.index.lines[line].clusters.len();
            for from in 0..=width {
                for to in 0..=width {
                    let overlay_says =
                        crate::selection_is_actionable(&overlay.clusters, line, from, to);
                    let index_says =
                        crate::page_selection_between_view(&view, line, from, to, true).is_ok();
                    assert_eq!(
                        overlay_says, index_says,
                        "row {line}, {from}..{to}: the overlay says {overlay_says} and the index says {index_says}"
                    );
                    if overlay_says { yes += 1 } else { no += 1 }
                }
            }
        }
        assert!(yes > 0, "the fixture must have selections that resolve");
        assert!(
            no > 0,
            "and selections that do not, or agreeing proves nothing"
        );
    }

    fn page_fixture() -> ByteStore {
        let content = b"0.25 g 10 20 30 40 re f";
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
        bytes.extend_from_slice(content.len().to_string().as_bytes());
        bytes.extend_from_slice(b" >>\nstream\n");
        bytes.extend_from_slice(content);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        let xref = bytes.len();
        bytes.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(b"trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n");
        bytes.extend_from_slice(xref.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        ByteStore::new(SourceId::new(55), Arc::<[u8]>::from(bytes))
    }

    #[test]
    fn first_page_flows_from_pdf_bytes_to_sourced_paint_atoms() {
        let report = inspect_page_paint_strict(&page_fixture(), 0).expect("M2 page inspection");
        assert_eq!(report.page, pdf_syntax::Reference::new(3, 0));
        assert_eq!(report.content_streams, 1);
        assert_eq!(report.decoded_bytes, 23);
        assert_eq!(report.operations, 3);
        assert_eq!(report.paint_atoms, 1);
    }

    #[test]
    fn reports_encryption_without_parsing_ciphertext_as_objects() {
        let report = inspect_strict(&fixture(), InspectLimits::default()).expect("strict report");

        assert_eq!(report.version.to_string(), "1.7");
        assert_eq!(report.revisions.len(), 1);
        assert_eq!(report.revisions[0].kind, RevisionKind::Classic);
        assert_eq!(report.objects.active(), 2);
        assert_eq!(report.objects.free, 1);
        assert_eq!(report.encrypted, Some(true));
        assert_eq!(report.signature_objects_inspected, 0);
        assert!(!report.signature_scan_complete);
        assert!(report.object_diagnostics.is_empty());
    }

    #[test]
    fn strict_mode_reports_no_repairs_because_it_performs_none() {
        let report = inspect_strict(&fixture(), InspectLimits::default()).expect("strict report");
        assert!(report.repairs.is_empty());
        assert_eq!(report.object_source, ObjectSource::Revisions);
    }

    #[test]
    fn recovery_of_a_healthy_file_matches_the_strict_report_exactly() {
        let source = fixture();
        let strict = inspect_strict(&source, InspectLimits::default()).expect("strict report");
        let recovered =
            inspect_recovering(&source, InspectLimits::default()).expect("recovering report");
        assert_eq!(strict, recovered);
        assert!(recovered.repairs.is_empty());
    }

    #[test]
    fn recovery_reports_a_scanned_map_without_claiming_revision_facts() {
        let source = fixture();
        let mut bytes = source.as_bytes().to_vec();
        let keyword = b"startxref\n";
        let position = bytes
            .windows(keyword.len())
            .rposition(|window| window == keyword)
            .expect("fixture has a startxref");
        let digits_start = position + keyword.len();
        let digits_end = digits_start
            + bytes[digits_start..]
                .iter()
                .position(|byte| *byte == b'\n')
                .expect("startxref value ends with a newline");
        bytes.splice(digits_start..digits_end, b"999999".iter().copied());
        let source = ByteStore::new(SourceId::new(43), Arc::<[u8]>::from(bytes));

        assert!(inspect_strict(&source, InspectLimits::default()).is_err());

        let report = inspect_recovering(&source, InspectLimits::default()).expect("recoverable");
        assert_eq!(report.object_source, ObjectSource::Scan);
        assert!(!report.repairs.is_empty());
        assert!(report.startxref.is_none());
        assert!(report.revisions.is_empty());
        assert_eq!(
            report.encrypted, None,
            "a scanned map has no trailer, so encryption cannot be claimed either way"
        );
        assert!(!report.signature_scan_complete);
        assert_eq!(report.objects.direct, 2);
    }

    #[test]
    fn finds_signature_dictionaries_and_decodes_name_escapes() {
        let source = fixture();
        let mut bytes = source.as_bytes().to_vec();
        let marker = b" /Enc#72ypt 2 0 R";
        let start = bytes
            .windows(marker.len())
            .position(|window| window == marker)
            .expect("encryption trailer entry");
        bytes.drain(start..start + marker.len());
        let xref = bytes
            .windows(b"xref\n".len())
            .position(|window| window == b"xref\n")
            .expect("xref offset");
        let startxref = bytes
            .windows(b"startxref\n".len())
            .position(|window| window == b"startxref\n")
            .expect("startxref");
        let number_start = startxref + b"startxref\n".len();
        let number_end = bytes[number_start..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|relative| number_start + relative)
            .expect("startxref line ending");
        bytes.splice(number_start..number_end, xref.to_string().bytes());
        let source = ByteStore::new(SourceId::new(42), Arc::<[u8]>::from(bytes));

        let report = inspect_strict(&source, InspectLimits::default()).expect("strict report");
        assert_eq!(report.encrypted, Some(false));
        assert_eq!(report.signatures_found, 1);
        assert_eq!(report.signature_objects_inspected, 2);
        assert!(report.signature_scan_complete);
    }

    fn marked_row() -> ByteStore {
        let content = b"BT /F1 12 Tf 1 0 0 1 10 20 Tm <414243> Tj ET";
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /FirstChar 65 /LastChar 67 /Widths [600 0 600] >> >> >> >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        );
        bytes.extend_from_slice(content);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
        );
        for offset in &offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                offsets.len() + 1
            )
            .as_bytes(),
        );
        ByteStore::new(SourceId::new(56), Arc::<[u8]>::from(bytes))
    }

    #[test]
    fn a_drag_in_pixels_becomes_the_offset_that_run_actually_needs() {
        let source = scaled_matrix_row();
        let anchor = first_run_anchor(&source);

        let (dx, dy) =
            page_run_offset(&source, 0, &anchor, 1.0, 12.0, 0.0).expect("the run is on the page");
        assert!(
            (dx - 1.0).abs() < 1e-9,
            "12 pixels across a 12-fold matrix is one unit: {dx}"
        );
        assert!(dy.abs() < 1e-9, "{dy}");

        let (dx, dy) =
            page_run_offset(&source, 0, &anchor, 1.0, 0.0, 24.0).expect("the run is on the page");
        assert!(dx.abs() < 1e-9, "{dx}");
        assert!((dy + 2.0).abs() < 1e-9, "{dy}");

        let (dx, _) =
            page_run_offset(&source, 0, &anchor, 2.0, 12.0, 0.0).expect("the run is on the page");
        assert!((dx - 0.5).abs() < 1e-9, "{dx}");

        assert!(page_run_offset(&source, 0, "1:0:9999", 1.0, 12.0, 0.0).is_err());
        assert!(page_run_offset(&source, 0, "not an anchor", 1.0, 12.0, 0.0).is_err());
    }

    #[test]
    fn the_overlay_reports_the_size_the_text_is_drawn_at() {
        let overlay = page_overlay(&scaled_matrix_row(), 0, 1.0).expect("the page interprets");
        let run = &overlay.runs[0];
        assert!(
            (run.size - 1.0).abs() < 1e-9,
            "the file said Tf 1: {}",
            run.size
        );
        assert!(
            (run.em - 12.0).abs() < 1e-9,
            "and the matrix makes it 12 on the page: {}",
            run.em
        );
    }

    fn scaled_matrix_row() -> ByteStore {
        const TINY_CFF: &str = "0100040100010101055465737400010101131d00000030111d0000004c0f1d0000\
            0051100001010106616c706861000000030101020c160e8b8b158c8b058b8c050e8b8b158c8b058b8c050e00002201\
            87000141";
        let program: Vec<u8> = {
            let digits: Vec<u8> = TINY_CFF.bytes().filter(u8::is_ascii_hexdigit).collect();
            digits
                .chunks_exact(2)
                .map(|pair| {
                    u8::from_str_radix(std::str::from_utf8(pair).expect("hex digits"), 16)
                        .expect("hex byte")
                })
                .collect()
        };
        let content = b"BT /F1 1 Tf 12 0 0 12 10 40 Tm (AAA) Tj ET";
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        );
        bytes.extend_from_slice(content);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Test /FirstChar 65 /LastChar 65 /Widths [600] /FontDescriptor 6 0 R >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"6 0 obj\n<< /Type /FontDescriptor /FontName /Test /Flags 4 /FontFile3 7 0 R >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!(
                "7 0 obj\n<< /Subtype /Type1C /Length {} >>\nstream\n",
                program.len()
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(&program);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        let size = offsets.len() + 1;
        let xref = bytes.len();
        bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
        for offset in &offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n")
                .as_bytes(),
        );
        ByteStore::new(SourceId::new(58), Arc::<[u8]>::from(bytes))
    }

    fn first_run_anchor(source: &ByteStore) -> String {
        page_overlay(source, 0, 1.0)
            .expect("the page interprets")
            .runs[0]
            .anchor
            .clone()
    }

    #[test]
    fn caret_positions_name_the_clusters_between_them_rather_than_glyphs() {
        let source = marked_row();

        let first = page_clusters_between(&source, 0, 0, 0, 1).expect("the row holds two clusters");
        assert_eq!(first.glyphs, 0..2, "the mark travels with its base");
        assert_eq!(first.clusters, 1);
        let second = page_clusters_between(&source, 0, 0, 1, 2).expect("the row holds two");
        assert_eq!(second.glyphs, 2..3);
        assert_eq!(
            first.anchor, second.anchor,
            "one show operation paints both, so both name the same run"
        );

        let both = page_clusters_between(&source, 0, 0, 0, 2).expect("both clusters");
        assert_eq!(both.glyphs, 0..3);
        assert_eq!(both.clusters, 2);
        assert_eq!(both.anchor, first.anchor);

        let refused =
            page_clusters_between(&source, 0, 0, 0, 3).expect_err("the row holds two clusters");
        assert!(refused.contains("2 clusters"), "{refused}");
        let refused = page_clusters_between(&source, 0, 1, 0, 1).expect_err("one row");
        assert!(refused.contains("no row 1"), "{refused}");
        let refused = page_clusters_between(&source, 0, 0, 1, 1).expect_err("an empty drag");
        assert!(refused.contains("nothing is selected"), "{refused}");
    }
}

#[derive(Clone, Debug)]
pub struct TextRunBox {
    pub anchor: String,
    pub box_pixels: [f64; 4],
    pub font: String,
    pub size: f64,
    pub em: f64,
    pub text: Arc<pdf_paint::ToUnicode>,
    pub fill: Option<[f64; 3]>,
    pub family: Option<String>,
}

impl TextRunBox {
    #[must_use]
    pub fn anchor_safe_font(&self) -> String {
        self.font
            .chars()
            .map(|character| {
                if character.is_whitespace() {
                    '_'
                } else {
                    character
                }
            })
            .collect()
    }
}

pub fn page_glyph_placements_strict(
    source: &ByteStore,
    page_index: usize,
) -> Result<Vec<String>, PagePaintInspectError> {
    let page = page_paint_graph_strict(source, page_index)?;
    Ok(pdf_paint::glyph_placement_signature(&page.graph))
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextBlockBox {
    pub box_pixels: [f64; 4],
    pub layout_pixels: [f64; 4],
    pub turn: f64,
    pub quad: QuadPixels,
    pub lines: Vec<usize>,
    pub anchors: Vec<String>,
    pub runs_on: bool,
    pub shape: Option<BlockShape>,
    pub font: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockShape {
    pub size: f64,
    pub turn: f64,
    pub slant: f64,
}

pub type QuadPixels = [[f64; 2]; 4];

#[derive(Clone, Debug, PartialEq)]
pub struct ObjectBox {
    pub object: usize,
    pub anchor: String,
    pub kind: pdf_semantics::ObjectKind,
    pub box_pixels: [f64; 4],
    pub quad: QuadPixels,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextClusterBox {
    pub anchor: String,
    pub glyphs: std::ops::Range<usize>,
    pub box_pixels: Option<[f64; 4]>,
    pub stacked: bool,
    pub line: usize,
    pub index_in_line: usize,
    pub text: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CaretStop {
    pub line: usize,
    pub offset: usize,
    pub at: [f64; 2],
    pub up: [f64; 2],
}

impl CaretStop {
    #[must_use]
    pub fn height(&self) -> f64 {
        self.up[0].hypot(self.up[1])
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PanelRow {
    pub object: usize,
    pub kind: pdf_semantics::ObjectKind,
    pub movable: Option<usize>,
    pub contributions: usize,
    pub box_pixels: Option<[f64; 4]>,
    pub quad: Option<QuadPixels>,
}

#[derive(Clone, Debug)]
pub struct PageOverlay {
    pub runs: Vec<TextRunBox>,
    pub clusters: Vec<TextClusterBox>,
    pub carets: Vec<CaretStop>,
    pub blocks: Vec<TextBlockBox>,
    pub objects: Vec<ObjectBox>,
    pub rows: Vec<PanelRow>,
}

pub fn page_overlay(
    source: &ByteStore,
    page_index: usize,
    scale: f64,
) -> Result<PageOverlay, String> {
    let page = page_paint_graph_strict(source, page_index).map_err(|error| error.to_string())?;
    page_overlay_view(&page, scale)
}

pub fn page_overlay_view(page: &PageView, scale: f64) -> Result<PageOverlay, String> {
    let device = pdf_render::DeviceTransform::for_page(
        &page.program.geometry,
        scale,
        pdf_render::RenderLimits::default(),
    )
    .map_err(|error| format!("{error:?}"))?;
    let index = &page.index;

    let mut runs = Vec::new();
    for atom in &page.graph.atoms {
        let PaintAtomKind::Text(text) = &atom.kind else {
            continue;
        };
        let Some(bounds) = text.outline_bounds() else {
            continue;
        };
        runs.push(run_box(&device, atom, text, bounds));
    }

    let mut placement: Vec<Option<(usize, usize)>> = vec![None; index.clusters.len()];
    for (row, line) in index.lines.iter().enumerate() {
        for (position, cluster) in line.clusters.iter().enumerate() {
            placement[*cluster] = Some((row, position));
        }
    }
    let mut clusters = Vec::new();
    for (ordinal, cluster) in index.clusters.iter().enumerate() {
        let Some((line, index_in_line)) = placement[ordinal] else {
            continue;
        };
        let atom = &page.graph.atoms[cluster.atom];
        let box_pixels = match &atom.kind {
            PaintAtomKind::Text(text) => text
                .outline_bounds_in(cluster.glyphs.clone())
                .or_else(|| {
                    matches!(
                        text.state.text.rendering_mode.value,
                        pdf_paint::TextRenderingMode::Invisible
                            | pdf_paint::TextRenderingMode::Clip
                    )
                    .then(|| text.advance_bounds_in(cluster.glyphs.clone()))
                    .flatten()
                })
                .map(|bounds| device_box(&device, bounds)),
            _ => None,
        };
        clusters.push(TextClusterBox {
            anchor: pdf_edit::SourceAnchor::of(&atom.id).encode(),
            glyphs: cluster.glyphs.clone(),
            box_pixels,
            stacked: cluster.is_stacked(),
            line,
            index_in_line,
            text: cluster_text(&atom.kind, cluster.glyphs.clone()),
        });
    }

    let mut carets = Vec::new();
    for (line, row) in index.lines.iter().enumerate() {
        let (em, direction) =
            row.clusters
                .first()
                .map_or((0.0, pdf_paint::Point { x: 1.0, y: 0.0 }), |cluster| {
                    let cluster = &index.clusters[*cluster];
                    (cluster.em, cluster.direction)
                });
        let rise = pdf_paint::Point {
            x: -direction.y * em,
            y: direction.x * em,
        };
        for offset in 0..=row.clusters.len() {
            let point = index.caret_point(line, offset);
            let placed = device.matrix.transform(point);
            let top = device.matrix.transform(pdf_paint::Point {
                x: point.x + rise.x,
                y: point.y + rise.y,
            });
            carets.push(CaretStop {
                line,
                offset,
                at: [placed.x, placed.y],
                up: [top.x - placed.x, top.y - placed.y],
            });
        }
    }

    let objects = place_objects(page, index, &device);
    Ok(PageOverlay {
        runs,
        clusters,
        carets,
        blocks: place_blocks(page, index, &device),
        rows: panel_rows(index, &device, &objects),
        objects,
    })
}

fn run_box(
    device: &pdf_render::DeviceTransform,
    atom: &pdf_paint::PaintAtom,
    text: &pdf_paint::TextShowPaint,
    bounds: [f64; 4],
) -> TextRunBox {
    TextRunBox {
        anchor: pdf_edit::SourceAnchor::of(&atom.id).encode(),
        box_pixels: device_box(device, bounds),
        text: Arc::clone(&text.text),
        font: text.state.text.font.as_ref().map_or_else(
            || "(none)".to_owned(),
            |font| String::from_utf8_lossy(&font.value.name).into_owned(),
        ),
        size: text.state.text.font_size.value,
        em: {
            let to_user = text.state.ctm.value.multiply(text.matrices.text.value);
            text.state.text.font_size.value * to_user.c.hypot(to_user.d)
        },
        fill: pdf_render::color_to_rgb(
            &text.state.fill_color_space.value,
            &text.state.fill_color.value,
        )
        .map(|(rgb, _)| rgb),
        family: text
            .font_request
            .as_ref()
            .map(|request| request.family.trim().to_owned())
            .filter(|family| !family.is_empty()),
    }
}

fn panel_rows(
    index: &pdf_semantics::SemanticIndex,
    device: &pdf_render::DeviceTransform,
    movable: &[ObjectBox],
) -> Vec<PanelRow> {
    let mut rows = Vec::with_capacity(index.objects.len());
    for (position, object) in index.objects.iter().enumerate().rev() {
        let quad = object.quad.map(|quad| device_quad(device, &quad));
        let target = movable
            .iter()
            .position(|candidate| candidate.object == position);
        rows.push(PanelRow {
            object: position,
            kind: object.kind,
            movable: target,
            contributions: object.members.len(),
            box_pixels: quad
                .as_ref()
                .map(|_| device_box(device, object.quad.map_or([0.0; 4], |quad| quad.bounds()))),
            quad,
        });
    }
    rows
}

fn place_objects(
    page: &PageView,
    index: &pdf_semantics::SemanticIndex,
    device: &pdf_render::DeviceTransform,
) -> Vec<ObjectBox> {
    let paper = f64::from(device.width) * f64::from(device.height);
    let mut objects = Vec::new();
    for (position, object) in index.objects.iter().enumerate() {
        let Some(atom) = object
            .first_atom()
            .and_then(|ordinal| page.graph.atoms.get(ordinal))
        else {
            continue;
        };
        let quad = match object.kind {
            pdf_semantics::ObjectKind::Image => object.quad,
            pdf_semantics::ObjectKind::Path => drawing_quad(page, object)
                .or_else(|| {
                    object.bounds.map(|bounds| {
                        pdf_semantics::Quad::placed(bounds, pdf_paint::Matrix::IDENTITY)
                    })
                })
                .filter(|quad| quad_area(&device_quad(device, quad)) * 4.0 <= paper),
            _ => None,
        };
        let Some(quad) = quad else {
            continue;
        };
        objects.push(ObjectBox {
            object: position,
            anchor: pdf_edit::SourceAnchor::of(&atom.id).encode(),
            kind: object.kind,
            box_pixels: device_box(device, quad.bounds()),
            quad: device_quad(device, &quad),
        });
    }
    objects
}

fn drawing_quad(page: &PageView, object: &pdf_semantics::Object) -> Option<pdf_semantics::Quad> {
    let mut shared: Option<pdf_paint::Matrix> = None;
    let mut own: Option<[f64; 4]> = None;
    for member in &object.members {
        let pdf_paint::PaintAtomKind::Path(path) = &page.graph.atoms.get(member.atom)?.kind else {
            return None;
        };
        let matrix = path.state.ctm.value;
        if let Some(first) = shared {
            let same = [
                (first.a, matrix.a),
                (first.b, matrix.b),
                (first.c, matrix.c),
                (first.d, matrix.d),
                (first.e, matrix.e),
                (first.f, matrix.f),
            ]
            .iter()
            .all(|(one, other)| (one - other).abs() <= 1e-9 * one.abs().max(other.abs()).max(1.0));
            if !same {
                return None;
            }
        } else {
            shared = Some(matrix);
        }
        if let Some(bounds) = path.own_bounds() {
            own = Some(own.map_or(bounds, |held| {
                [
                    held[0].min(bounds[0]),
                    held[1].min(bounds[1]),
                    held[2].max(bounds[2]),
                    held[3].max(bounds[3]),
                ]
            }));
        }
    }
    let matrix = shared?;
    let [x0, y0, x1, y1] = own?;
    let area = matrix.a.mul_add(matrix.d, -(matrix.b * matrix.c));
    if !area.is_finite() || area.abs() <= f64::EPSILON {
        return None;
    }
    let rect = if area < 0.0 {
        [x0, y1, x1, y0]
    } else {
        [x0, y0, x1, y1]
    };
    Some(pdf_semantics::Quad::placed(rect, matrix))
}

fn cluster_text(kind: &PaintAtomKind, glyphs: std::ops::Range<usize>) -> Option<String> {
    let PaintAtomKind::Text(run) = kind else {
        return None;
    };
    let mut text = String::new();
    let mut known = false;
    for glyph in run.glyphs.get(glyphs)? {
        match run.text.text_of(Code {
            value: glyph.code.value,
            byte_len: glyph.code.bytes.len(),
        }) {
            Some(meaning) if !meaning.text.is_empty() => {
                known = true;
                text.push_str(&meaning.text);
            }
            _ => text.push('\u{fffd}'),
        }
    }
    known.then_some(text)
}

fn place_blocks(
    page: &PageView,
    index: &pdf_semantics::SemanticIndex,
    device: &pdf_render::DeviceTransform,
) -> Vec<TextBlockBox> {
    let quads: std::collections::BTreeMap<usize, pdf_semantics::Quad> = index
        .objects
        .iter()
        .filter_map(|object| match object.kind {
            pdf_semantics::ObjectKind::Text(block) => Some((block, object.quad?)),
            _ => None,
        })
        .collect();
    let mut blocks = Vec::new();
    for (index_of, block) in index.blocks.iter().enumerate() {
        let bounds = block.bounds.unwrap_or([0.0; 4]);
        let mut anchors: Vec<String> = Vec::new();
        for line in &block.lines {
            for cluster in &index.lines[*line].clusters {
                let atom = &page.graph.atoms[index.clusters[*cluster].atom];
                let anchor = pdf_edit::SourceAnchor::of(&atom.id).encode();
                if anchors.last() != Some(&anchor) && !anchors.contains(&anchor) {
                    anchors.push(anchor);
                }
            }
        }
        let box_pixels = device_box(device, bounds);
        let turn = block_turn(page, index, block);
        let layout = if turn == 0.0 {
            block.layout
        } else {
            turned_layout(page, index, block, turn)
        };
        blocks.push(TextBlockBox {
            box_pixels,
            layout_pixels: layout.map_or(box_pixels, |layout| device_box(device, layout)),
            turn,
            quad: quads
                .get(&index_of)
                .map_or_else(|| quad_of_box(box_pixels), |quad| device_quad(device, quad)),
            lines: block.lines.clone(),
            shape: block_shape(page, index, block),
            font: block_font(page, index, block),
            anchors,
            runs_on: block.lines.len() > 1,
        });
    }
    blocks
}

fn block_turn(
    page: &pdf_session::PageView,
    index: &pdf_semantics::SemanticIndex,
    block: &pdf_semantics::Block,
) -> f64 {
    let Some(cluster) = block
        .lines
        .first()
        .and_then(|line| index.lines.get(*line))
        .and_then(|line| line.clusters.first())
        .and_then(|cluster| index.clusters.get(*cluster))
    else {
        return 0.0;
    };
    match page.graph.atoms.get(cluster.atom).map(|atom| &atom.kind) {
        Some(PaintAtomKind::Text(text)) => pdf_edit::text_turn(text),
        _ => 0.0,
    }
}

fn turned_layout(
    page: &pdf_session::PageView,
    index: &pdf_semantics::SemanticIndex,
    block: &pdf_semantics::Block,
    turn: f64,
) -> Option<[f64; 4]> {
    let back = pdf_edit::rotation(-turn);
    let mut bounds = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for line in &block.lines {
        for cluster in &index.lines.get(*line)?.clusters {
            let cluster = index.clusters.get(*cluster)?;
            let Some(PaintAtomKind::Text(text)) =
                page.graph.atoms.get(cluster.atom).map(|atom| &atom.kind)
            else {
                continue;
            };
            let points = text
                .layout_points_in(cluster.glyphs.clone())
                .or_else(|| {
                    let mut ink = text
                        .outline_points_in(cluster.glyphs.clone())
                        .filter(|points| !points.is_empty())?;
                    ink.push(cluster.baseline);
                    ink.push(pdf_paint::Point {
                        x: cluster.baseline.x + cluster.direction.x * cluster.advance,
                        y: cluster.baseline.y + cluster.direction.y * cluster.advance,
                    });
                    Some(ink)
                })
                .unwrap_or_default();
            for point in points {
                let turned = back.transform(point);
                bounds = [
                    bounds[0].min(turned.x),
                    bounds[1].min(turned.y),
                    bounds[2].max(turned.x),
                    bounds[3].max(turned.y),
                ];
            }
        }
    }
    bounds
        .iter()
        .all(|value| value.is_finite())
        .then_some(bounds)
}

fn block_font(
    page: &pdf_session::PageView,
    index: &pdf_semantics::SemanticIndex,
    block: &pdf_semantics::Block,
) -> Option<String> {
    let line = index.lines.get(*block.lines.first()?)?;
    let cluster = index.clusters.get(*line.clusters.first()?)?;
    let pdf_paint::PaintAtomKind::Text(text) = &page.graph.atoms.get(cluster.atom)?.kind else {
        return None;
    };
    let family = text.font_request.as_ref()?.family.trim();
    (!family.is_empty()).then(|| family.to_owned())
}

fn block_shape(
    page: &pdf_session::PageView,
    index: &pdf_semantics::SemanticIndex,
    block: &pdf_semantics::Block,
) -> Option<BlockShape> {
    let line = index.lines.get(*block.lines.first()?)?;
    let cluster = index.clusters.get(*line.clusters.first()?)?;
    let pdf_paint::PaintAtomKind::Text(text) = &page.graph.atoms.get(cluster.atom)?.kind else {
        return None;
    };
    let shape = text.placed_shape()?;
    Some(BlockShape {
        size: text.size_on_page()?,
        turn: shape.turn.to_degrees(),
        slant: shape.slant.to_degrees(),
    })
}

#[must_use]
pub fn selection_is_actionable(
    clusters: &[TextClusterBox],
    line: usize,
    from: usize,
    to: usize,
) -> bool {
    let (from, to) = (from.min(to), from.max(to));
    let width = clusters
        .iter()
        .filter(|cluster| cluster.line == line)
        .count();
    if to > width {
        return false;
    }
    let selected: Vec<&TextClusterBox> = clusters
        .iter()
        .filter(|cluster| {
            cluster.line == line && cluster.index_in_line >= from && cluster.index_in_line < to
        })
        .collect();
    if selected.is_empty() {
        return false;
    }
    let mut ranges = std::collections::BTreeMap::<&str, (usize, usize, usize)>::new();
    for cluster in selected {
        let range = ranges.entry(cluster.anchor.as_str()).or_insert((
            cluster.glyphs.start,
            cluster.glyphs.end,
            0,
        ));
        range.0 = range.0.min(cluster.glyphs.start);
        range.1 = range.1.max(cluster.glyphs.end);
        range.2 += cluster.glyphs.len();
    }
    ranges
        .values()
        .all(|(start, end, covered)| end - start == *covered)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Typing {
    WithinTheFont { codes: usize },
    NoMeaning { refused: bool },
    NeedsANewGlyph { character: char },
    Ambiguous { character: char, codes: usize },
    Nowhere,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectionReading {
    pub shown: String,
    pub unknown: usize,
}

#[must_use]
pub fn read_selection(
    clusters: &[TextClusterBox],
    line: usize,
    from: usize,
    to: usize,
) -> Option<SelectionReading> {
    let (from, to) = (from.min(to), from.max(to));
    if from == to {
        return None;
    }
    let mut selected: Vec<&TextClusterBox> = clusters
        .iter()
        .filter(|cluster| {
            cluster.line == line && cluster.index_in_line >= from && cluster.index_in_line < to
        })
        .collect();
    if selected.len() != to - from {
        return None;
    }
    selected.sort_by_key(|cluster| cluster.index_in_line);
    let mut reading = SelectionReading {
        shown: String::new(),
        unknown: 0,
    };
    for cluster in selected {
        if let Some(text) = &cluster.text {
            if text.contains('\u{fffd}') {
                reading.unknown += 1;
            }
            reading.shown.push_str(text);
        } else {
            reading.unknown += 1;
            reading.shown.push('\u{fffd}');
        }
    }
    Some(reading)
}

#[must_use]
pub fn selection_text(
    clusters: &[TextClusterBox],
    line: usize,
    from: usize,
    to: usize,
) -> Option<String> {
    read_selection(clusters, line, from, to)
        .filter(|reading| reading.unknown == 0)
        .map(|reading| reading.shown)
}

#[must_use]
pub fn anchor_under_caret(clusters: &[TextClusterBox], line: usize, offset: usize) -> Option<&str> {
    let row = clusters.iter().filter(|cluster| cluster.line == line);
    let beside = row
        .clone()
        .find(|cluster| cluster.index_in_line == offset)
        .or_else(|| {
            row.clone()
                .find(|cluster| cluster.index_in_line + 1 == offset)
        })?;
    Some(&beside.anchor)
}

#[must_use]
pub fn typing_verdict(font: &ToUnicode, typed: &str) -> Typing {
    if typed.is_empty() {
        return Typing::Nowhere;
    }
    if font.is_empty() {
        return Typing::NoMeaning {
            refused: font.refused().is_some(),
        };
    }
    let mut codes = 0;
    for character in typed.chars() {
        match font.codes_to_write(&character.to_string()).len() {
            0 => return Typing::NeedsANewGlyph { character },
            1 => codes += 1,
            many => {
                return Typing::Ambiguous {
                    character,
                    codes: many,
                };
            }
        }
    }
    Typing::WithinTheFont { codes }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClusterTarget {
    pub anchor: pdf_edit::SourceAnchor,
    pub glyphs: std::ops::Range<usize>,
    pub clusters: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SelectionTarget {
    pub runs: Vec<pdf_edit::RunRewrite>,
    pub clusters: usize,
    pub closes_gap: bool,
}

pub fn page_block_removal_view(
    page: &PageView,
    anchors: &[pdf_edit::SourceAnchor],
) -> Result<Vec<pdf_edit::RunRewrite>, String> {
    let mut runs = Vec::with_capacity(anchors.len());
    for anchor in anchors {
        let atom = page
            .graph
            .atoms
            .iter()
            .find(|atom| anchor.names(&atom.id))
            .ok_or_else(|| "this block names a run the page does not have".to_owned())?;
        let pdf_paint::PaintAtomKind::Text(text) = &atom.kind else {
            return Err("this block names something that is not text".to_owned());
        };
        if text.glyphs.is_empty() {
            continue;
        }
        runs.push(pdf_edit::RunRewrite {
            anchor: anchor.clone(),
            glyphs: Some(pdf_edit::GlyphChange::Remove {
                glyphs: 0..text.glyphs.len(),
                close_gap: false,
            }),
            displace: (0.0, 0.0),
        });
    }
    if runs.is_empty() {
        return Err("this block paints no glyphs, so there is nothing to delete".to_owned());
    }
    Ok(runs)
}

pub fn page_run_offset(
    source: &ByteStore,
    page_index: usize,
    anchor: &str,
    scale: f64,
    dx: f64,
    dy: f64,
) -> Result<(f64, f64), String> {
    let page = page_paint_graph_strict(source, page_index).map_err(|error| error.to_string())?;
    page_run_offset_view(&page, anchor, scale, dx, dy)
}

pub fn page_run_offset_view(
    page: &PageView,
    anchor: &str,
    scale: f64,
    dx: f64,
    dy: f64,
) -> Result<(f64, f64), String> {
    let anchor = pdf_edit::SourceAnchor::decode(anchor).ok_or("malformed anchor")?;
    let device = pdf_render::DeviceTransform::for_page(
        &page.program.geometry,
        scale,
        pdf_render::RenderLimits::default(),
    )
    .map_err(|error| format!("{error:?}"))?;
    let text = page
        .graph
        .atoms
        .iter()
        .find_map(|atom| match &atom.kind {
            PaintAtomKind::Text(text) if anchor.names(&atom.id) => Some(text),
            _ => None,
        })
        .ok_or("no text run is written where the selection anchors")?;

    let to_user = text.state.ctm.value.multiply(text.matrices.text.value);
    let placed = |dx: f64, dy: f64| {
        let ux = to_user.a.mul_add(dx, to_user.c * dy);
        let uy = to_user.b.mul_add(dx, to_user.d * dy);
        (
            device.matrix.a.mul_add(ux, device.matrix.c * uy),
            device.matrix.b.mul_add(ux, device.matrix.d * uy),
        )
    };
    let (ax, ay) = placed(1.0, 0.0);
    let (bx, by) = placed(0.0, 1.0);
    let determinant = ax.mul_add(by, -(ay * bx));
    if !determinant.is_finite() || determinant.abs() < 1e-12 {
        return Err(
            "this run's transform has no area, so a movement in pixels says nothing about it"
                .to_owned(),
        );
    }
    Ok((
        dx.mul_add(by, -(dy * bx)) / determinant,
        ax.mul_add(dy, -(ay * dx)) / determinant,
    ))
}

pub fn page_offset_view(
    page: &PageView,
    scale: f64,
    dx: f64,
    dy: f64,
) -> Result<(f64, f64), String> {
    let device = pdf_render::DeviceTransform::for_page(
        &page.program.geometry,
        scale,
        pdf_render::RenderLimits::default(),
    )
    .map_err(|error| format!("{error:?}"))?;
    let matrix = device.matrix;
    let determinant = matrix.a.mul_add(matrix.d, -(matrix.b * matrix.c));
    if !determinant.is_finite() || determinant.abs() < 1e-12 {
        return Err(
            "this page's transform has no area, so a movement in pixels says nothing about it"
                .to_owned(),
        );
    }
    Ok((
        matrix.d.mul_add(dx, -(matrix.c * dy)) / determinant,
        matrix.a.mul_add(dy, -(matrix.b * dx)) / determinant,
    ))
}

pub fn page_transform_view(
    page: &PageView,
    scale: f64,
    matrix: pdf_paint::Matrix,
    fixed: (f64, f64),
) -> Result<(pdf_paint::Matrix, pdf_paint::Point), String> {
    let device = pdf_render::DeviceTransform::for_page(
        &page.program.geometry,
        scale,
        pdf_render::RenderLimits::default(),
    )
    .map_err(|error| format!("{error:?}"))?;
    let back = device.matrix.inverse().ok_or_else(|| {
        "this page's transform has no area, so a gesture in pixels says nothing about it".to_owned()
    })?;
    let straight = |matrix: pdf_paint::Matrix| pdf_paint::Matrix {
        e: 0.0,
        f: 0.0,
        ..matrix
    };
    Ok((
        straight(back)
            .multiply(straight(matrix))
            .multiply(straight(device.matrix)),
        back.transform(pdf_paint::Point {
            x: fixed.0,
            y: fixed.1,
        }),
    ))
}

pub fn page_clusters_between(
    source: &ByteStore,
    page_index: usize,
    line: usize,
    from: usize,
    to: usize,
) -> Result<ClusterTarget, String> {
    let page = page_paint_graph_strict(source, page_index).map_err(|error| error.to_string())?;
    page_clusters_between_view(&page, line, from, to)
}

pub fn page_clusters_between_view(
    page: &PageView,
    line: usize,
    from: usize,
    to: usize,
) -> Result<ClusterTarget, String> {
    let index = &page.index;
    let row = index.lines.get(line).ok_or_else(|| {
        format!(
            "this page has {} rows, so there is no row {line}",
            index.lines.len()
        )
    })?;
    if from.max(to) > row.clusters.len() {
        return Err(format!(
            "row {line} holds {} clusters, so it has no position {}",
            row.clusters.len(),
            from.max(to)
        ));
    }
    let span = index
        .selection_span(
            pdf_semantics::Caret { line, offset: from },
            pdf_semantics::Caret { line, offset: to },
        )
        .map_err(|error| error.to_string())?;
    let atom = page
        .graph
        .atoms
        .get(span.atom)
        .ok_or("the selection names a paint atom this page does not have")?;
    Ok(ClusterTarget {
        anchor: pdf_edit::SourceAnchor::of(&atom.id),
        glyphs: span.glyphs,
        clusters: span.clusters,
    })
}

pub fn page_selection_between_view(
    page: &PageView,
    line: usize,
    from: usize,
    to: usize,
    close_gap: bool,
) -> Result<SelectionTarget, String> {
    let index = &page.index;
    let row = index.lines.get(line).ok_or_else(|| {
        format!(
            "this page has {} rows, so there is no row {line}",
            index.lines.len()
        )
    })?;
    if from.max(to) > row.clusters.len() {
        return Err(format!(
            "row {line} holds {} clusters, so it has no position {}",
            row.clusters.len(),
            from.max(to)
        ));
    }
    let spans = index
        .selection_spans(
            pdf_semantics::Caret { line, offset: from },
            pdf_semantics::Caret { line, offset: to },
        )
        .map_err(|error| error.to_string())?;
    let clusters = spans.iter().map(|span| span.clusters).sum();
    let anchor_of = |atom: usize| -> Result<pdf_edit::SourceAnchor, String> {
        page.graph
            .atoms
            .get(atom)
            .map(|atom| pdf_edit::SourceAnchor::of(&atom.id))
            .ok_or_else(|| "the selection names a paint atom this page does not have".to_owned())
    };
    let mut runs = Vec::with_capacity(spans.len());
    let mut selected_atoms = Vec::with_capacity(spans.len());
    for span in spans {
        selected_atoms.push(span.atom);
        runs.push(pdf_edit::RunRewrite {
            anchor: anchor_of(span.atom)?,
            glyphs: Some(pdf_edit::GlyphChange::Remove {
                glyphs: span.glyphs,
                close_gap,
            }),
            displace: (0.0, 0.0),
        });
    }
    let (from_at, to_at) = (
        index.caret_point(line, from.min(to)),
        index.caret_point(line, from.max(to)),
    );
    let gap = pdf_paint::Point {
        x: from_at.x - to_at.x,
        y: from_at.y - to_at.y,
    };
    let after_selection = runs_on_row_after(page, line, from.max(to));
    for atom in runs_following(page, &selected_atoms) {
        if selected_atoms.contains(&atom) {
            continue;
        }
        let displace = if close_gap && after_selection.contains(&atom) {
            text_space_of(page, atom, gap)
                .ok_or("a run on this row paints into a space nothing can be carried into")?
        } else {
            (0.0, 0.0)
        };
        runs.push(pdf_edit::RunRewrite {
            anchor: anchor_of(atom)?,
            glyphs: None,
            displace,
        });
    }
    Ok(SelectionTarget {
        runs,
        clusters,
        closes_gap: close_gap,
    })
}

fn runs_following(page: &PageView, edited: &[usize]) -> Vec<usize> {
    let Some(last) = edited.iter().copied().max() else {
        return Vec::new();
    };
    let Some(stream) = page.graph.atoms.get(last).map(|atom| atom.id.stream) else {
        return Vec::new();
    };
    page.graph
        .atoms
        .iter()
        .enumerate()
        .skip(last + 1)
        .filter(|(_, atom)| match &atom.kind {
            PaintAtomKind::Text(text) => !text.glyphs.is_empty(),
            _ => false,
        })
        .filter(|(_, atom)| {
            atom.id.stream == stream
                && atom.id.invocation_path.is_empty()
                && atom.id.pattern_path.is_empty()
        })
        .map(|(ordinal, _)| ordinal)
        .collect()
}

#[must_use]
pub fn substituted_family_on_row(page: &PageView, line: usize, offset: usize) -> Option<String> {
    let row = page.index.lines.get(line)?;
    let cluster = row.clusters.get(offset).or_else(|| row.clusters.last())?;
    let atom = page.index.clusters[*cluster].atom;
    let PaintAtomKind::Text(text) = &page.graph.atoms[atom].kind else {
        return None;
    };
    pdf_edit::substituted_family(text)
}

pub fn page_replacement_view(
    page: &PageView,
    line: usize,
    from: usize,
    to: usize,
    typed: &str,
) -> Result<Vec<pdf_edit::RunRewrite>, String> {
    let row = page.index.lines.get(line).ok_or("no such row")?;
    if from.max(to) > row.clusters.len() {
        return Err("caret outside row".to_owned());
    }
    let (atom, glyphs) = if from == to {
        let cluster = row
            .clusters
            .get(from)
            .or_else(|| row.clusters.last())
            .ok_or("an empty block needs an explicit font and insertion point")?;
        let cluster = &page.index.clusters[*cluster];
        let at = if from == row.clusters.len() {
            cluster.glyphs.end
        } else {
            cluster.glyphs.start
        };
        (cluster.atom, at..at)
    } else {
        let spans = page
            .index
            .selection_spans(
                pdf_semantics::Caret { line, offset: from },
                pdf_semantics::Caret { line, offset: to },
            )
            .map_err(|error| error.to_string())?;
        let [span] = spans.as_slice() else {
            return Err("replacement across source runs is not supported yet".to_owned());
        };
        (span.atom, span.glyphs.clone())
    };
    let PaintAtomKind::Text(text) = &page.graph.atoms[atom].kind else {
        return Err("not a text run".to_owned());
    };
    let resources =
        pdf_edit::scope_resources(&page.program, &page.operations, &page.graph.atoms[atom].id)
            .map_err(|error| error.to_string())?;
    let (x, y) = pdf_edit::replacement_shift(&resources, text, &glyphs, typed)
        .map_err(|error| error.to_string())?;
    let mut runs = vec![pdf_edit::RunRewrite {
        anchor: pdf_edit::SourceAnchor::of(&page.graph.atoms[atom].id),
        glyphs: Some(pdf_edit::GlyphChange::Replace {
            glyphs,
            text: typed.to_owned(),
        }),
        displace: (0.0, 0.0),
    }];
    let after = runs_on_row_after(page, line, from.max(to));
    for following in runs_following(page, &[atom]) {
        let displace = if after.contains(&following) {
            text_space_of(page, following, pdf_paint::Point { x, y })
                .ok_or("singular text placement")?
        } else {
            (0.0, 0.0)
        };
        runs.push(pdf_edit::RunRewrite {
            anchor: pdf_edit::SourceAnchor::of(&page.graph.atoms[following].id),
            glyphs: None,
            displace,
        });
    }
    Ok(runs)
}

fn runs_on_row_after(page: &PageView, line: usize, offset: usize) -> Vec<usize> {
    let index = &page.index;
    let Some(row) = index.lines.get(line) else {
        return Vec::new();
    };
    let mut after = Vec::new();
    let mut before = Vec::new();
    for (position, cluster) in row.clusters.iter().enumerate() {
        let atom = index.clusters[*cluster].atom;
        if position < offset {
            before.push(atom);
        } else {
            after.push(atom);
        }
    }
    after.sort_unstable();
    after.dedup();
    after.retain(|atom| !before.contains(atom));
    after
}

fn text_space_of(page: &PageView, atom: usize, gap: pdf_paint::Point) -> Option<(f64, f64)> {
    let PaintAtomKind::Text(text) = &page.graph.atoms.get(atom)?.kind else {
        return None;
    };
    let to_user = text.state.ctm.value.multiply(text.matrices.text.value);
    let determinant = to_user.a.mul_add(to_user.d, -(to_user.b * to_user.c));
    if !determinant.is_finite() || determinant.abs() < 1e-12 {
        return None;
    }
    Some((
        to_user.d.mul_add(gap.x, -(to_user.c * gap.y)) / determinant,
        to_user.a.mul_add(gap.y, -(to_user.b * gap.x)) / determinant,
    ))
}

#[derive(Clone, Debug, PartialEq)]
pub struct AtomBox {
    pub ordinal: usize,
    pub parent: Option<usize>,
    pub depth: usize,
    pub kind: &'static str,
    pub box_pixels: Option<[f64; 4]>,
    pub source: String,
    pub id: pdf_paint::PaintId,
}

pub fn page_atom_boxes(
    source: &ByteStore,
    page_index: usize,
    scale: f64,
) -> Result<Vec<AtomBox>, PagePaintInspectError> {
    let page = page_paint_graph_strict(source, page_index)?;
    page_atom_boxes_view(&page, scale)
}

pub fn page_atom_boxes_view(
    page: &PageView,
    scale: f64,
) -> Result<Vec<AtomBox>, PagePaintInspectError> {
    let device = pdf_render::DeviceTransform::for_page(
        &page.program.geometry,
        scale,
        pdf_render::RenderLimits::default(),
    )
    .map_err(PagePaintInspectError::Render)?;
    let mut boxes = Vec::new();
    let mut ordinal = 0_usize;
    collect_atom_boxes(&page.graph, &device, 0, None, &mut ordinal, &mut boxes);
    Ok(boxes)
}

fn collect_atom_boxes(
    graph: &PaintGraph,
    device: &pdf_render::DeviceTransform,
    depth: usize,
    parent: Option<usize>,
    ordinal: &mut usize,
    out: &mut Vec<AtomBox>,
) {
    for atom in &graph.atoms {
        let here = *ordinal;
        *ordinal += 1;
        out.push(AtomBox {
            ordinal: here,
            parent,
            depth,
            kind: atom_kind_name(&atom.kind),
            box_pixels: atom_user_bounds(&atom.kind).map(|bounds| device_box(device, bounds)),
            source: format!(
                "{} {} R @ {}..{}",
                atom.id.stream.object_number(),
                atom.id.stream.generation(),
                atom.id.operator_span.start(),
                atom.id.operator_span.end()
            ),
            id: atom.id.clone(),
        });
        if let PaintAtomKind::TransparencyGroup(group) = &atom.kind {
            collect_atom_boxes(&group.graph, device, depth + 1, Some(here), ordinal, out);
        }
    }
}

const fn atom_kind_name(kind: &PaintAtomKind) -> &'static str {
    match kind {
        PaintAtomKind::Path(_) => "path",
        PaintAtomKind::Text(_) => "text",
        PaintAtomKind::TransparencyGroup(_) => "group",
        PaintAtomKind::Shading(_) => "shading",
        PaintAtomKind::Image(_) => "image",
    }
}

fn atom_user_bounds(kind: &PaintAtomKind) -> Option<[f64; 4]> {
    kind.user_bounds()
}

#[must_use]
pub fn atoms_at(boxes: &[AtomBox], x: f64, y: f64) -> Vec<&AtomBox> {
    let mut hits: Vec<&AtomBox> = boxes
        .iter()
        .filter(|atom| {
            atom.box_pixels
                .is_some_and(|[x0, y0, x1, y1]| x >= x0 && x <= x1 && y >= y0 && y <= y1)
        })
        .collect();
    hits.reverse();
    hits
}

fn device_box(device: &pdf_render::DeviceTransform, bounds: [f64; 4]) -> [f64; 4] {
    device.device_box(bounds)
}

fn device_quad(device: &pdf_render::DeviceTransform, quad: &pdf_semantics::Quad) -> QuadPixels {
    quad.corners.map(|corner| {
        let placed = device.matrix.transform(corner);
        [placed.x, placed.y]
    })
}

fn quad_area(quad: &QuadPixels) -> f64 {
    let mut twice = 0.0;
    for index in 0..4 {
        let [x0, y0] = quad[index];
        let [x1, y1] = quad[(index + 1) % 4];
        twice += x0.mul_add(y1, -(x1 * y0));
    }
    (twice / 2.0).abs()
}

fn quad_of_box(bounds: [f64; 4]) -> QuadPixels {
    [
        [bounds[0], bounds[1]],
        [bounds[2], bounds[1]],
        [bounds[2], bounds[3]],
        [bounds[0], bounds[3]],
    ]
}
