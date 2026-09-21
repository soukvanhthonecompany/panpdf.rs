#![forbid(unsafe_code)]

pub mod extract;
pub mod gesture;
pub mod pictures;
pub mod select;

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use pdf_bytes::ByteStore;
use pdf_content::{
    ContentError, ContentLimits, Operation, PageContentError, PageContentLimits, PageProgram,
    parse_operation_sequence_strict,
};
use pdf_edit::spike_move_text::{PlannerPage, SpikeError, lay_out_live, plan_command_in};
use pdf_edit::{Command, History, Plan};
use pdf_paint::{InterpretError, PaintGraph, PaintLimits, PaintStream};
use pdf_semantics::{ClusterKey, Grouping, SemanticIndex};

type Correspondence = Option<BTreeMap<ClusterKey, ClusterKey>>;

type Said = (usize, Correspondence, Vec<(ClusterKey, ClusterKey)>);

type Regrouped = (usize, Option<Arc<Grouping>>, Option<Arc<Grouping>>);

pub use extract::{blank_document, extract_pages};
pub use gesture::{GestureError, Intent};
pub use pictures::pictures_into_pdf;
pub use select::{
    CandidateStack, Gesture, HitCandidate, InspectError, Inspection, ObjectRef, RevisionId,
    SelectionError, SessionId,
};

#[derive(Clone, Debug)]
pub struct PageView {
    pub program: PageProgram,
    pub repairs: Vec<pdf_content::Repair>,
    pub operations: Vec<Vec<Operation>>,
    pub graph: PaintGraph,
    pub annotations: Vec<pdf_paint::AnnotationPaint>,
    pub index: SemanticIndex,
}

impl PageView {
    #[must_use]
    pub fn layers(&self) -> Vec<&PaintGraph> {
        std::iter::once(&self.graph)
            .chain(
                self.annotations
                    .iter()
                    .filter(|annotation| !annotation.graph.atoms.is_empty())
                    .map(|annotation| &annotation.graph),
            )
            .collect()
    }

    #[must_use]
    pub fn annotations_are_faithful(&self) -> bool {
        self.annotations
            .iter()
            .all(|annotation| annotation.outcome.is_faithful())
    }

    #[must_use]
    pub fn operation_count(&self) -> usize {
        self.operations.iter().map(Vec::len).sum()
    }

    #[must_use]
    pub fn footprint(&self) -> usize {
        self.graph.footprint()
            + self
                .annotations
                .iter()
                .map(|annotation| annotation.graph.footprint())
                .sum::<usize>()
            + self.operation_count() * std::mem::size_of::<Operation>()
    }
}

pub fn interpret_page(source: &ByteStore, page_index: usize) -> Result<PageView, PageError> {
    interpret_page_with(source, page_index, b"")
}

pub fn interpret_page_with(
    source: &ByteStore,
    page_index: usize,
    credential: &[u8],
) -> Result<PageView, PageError> {
    interpret_page_grouped(source, page_index, credential, None)
}

pub fn interpret_page_grouped(
    source: &ByteStore,
    page_index: usize,
    credential: &[u8],
    grouping: Option<&Grouping>,
) -> Result<PageView, PageError> {
    interpret_page_fully(source, page_index, credential, grouping, None)
}

pub fn interpret_page_fully(
    source: &ByteStore,
    page_index: usize,
    credential: &[u8],
    grouping: Option<&Grouping>,
    fonts: Option<Arc<dyn pdf_content::FontProvider>>,
) -> Result<PageView, PageError> {
    interpret_page_on(
        source,
        page_index,
        (credential, grouping, fonts),
        pdf_content::Medium::Screen,
    )
}

fn interpret_page_on(
    source: &ByteStore,
    page_index: usize,
    (credential, grouping, fonts): (
        &[u8],
        Option<&Grouping>,
        Option<Arc<dyn pdf_content::FontProvider>>,
    ),
    medium: pdf_content::Medium,
) -> Result<PageView, PageError> {
    let (program, repairs) = pdf_content::load_page_program_recovering_for(
        source,
        page_index,
        (
            PageContentLimits::default(),
            pdf_content::RecoverLimits::default(),
        ),
        credential,
        medium,
    )
    .map_err(PageError::Page)?
    .into_parts();
    let refusal = match view_of(program, repairs, grouping, fonts.clone()) {
        Ok(view) => return Ok(view),
        Err(refusal) => refusal,
    };
    let Ok(recovered) = pdf_content::load_page_program_tolerating_damage_for(
        source,
        page_index,
        (
            PageContentLimits::default(),
            pdf_content::RecoverLimits::default(),
        ),
        credential,
        medium,
    ) else {
        return Err(refusal);
    };
    let (program, repairs) = recovered.into_parts();
    view_of(program, repairs, grouping, fonts).map_err(|_| refusal)
}

pub fn interpret_page_for_display(
    source: &ByteStore,
    page_index: usize,
    credential: &[u8],
    grouping: Option<&Grouping>,
    fonts: Option<Arc<dyn pdf_content::FontProvider>>,
) -> Result<PageView, PageError> {
    shown_on(
        source,
        page_index,
        (credential, grouping, fonts),
        pdf_content::Medium::Screen,
    )
}

pub fn interpret_page_for_print(
    source: &ByteStore,
    page_index: usize,
    credential: &[u8],
    fonts: Option<Arc<dyn pdf_content::FontProvider>>,
) -> Result<PageView, PageError> {
    shown_on(
        source,
        page_index,
        (credential, None, fonts),
        pdf_content::Medium::Print,
    )
}

fn shown_on(
    source: &ByteStore,
    page_index: usize,
    (credential, grouping, fonts): (
        &[u8],
        Option<&Grouping>,
        Option<Arc<dyn pdf_content::FontProvider>>,
    ),
    medium: pdf_content::Medium,
) -> Result<PageView, PageError> {
    let strict = match interpret_page_on(
        source,
        page_index,
        (credential, grouping, fonts.clone()),
        medium,
    ) {
        Ok(view) => return Ok(view),
        Err(refusal) => refusal,
    };
    let Ok(recovered) = pdf_content::load_page_program_tolerating_damage_for(
        source,
        page_index,
        (
            PageContentLimits::default(),
            pdf_content::RecoverLimits::default(),
        ),
        credential,
        medium,
    ) else {
        return Err(strict);
    };
    let (program, repairs) = recovered.into_parts();
    view_of_tolerating_unsupported(program, repairs, grouping, fonts).map_err(|_| strict)
}

fn view_of(
    program: pdf_content::PageProgram,
    repairs: Vec<pdf_content::Repair>,
    grouping: Option<&Grouping>,
    fonts: Option<Arc<dyn pdf_content::FontProvider>>,
) -> Result<PageView, PageError> {
    view_of_inner(program, repairs, grouping, fonts, false)
}

fn view_of_tolerating_unsupported(
    program: pdf_content::PageProgram,
    repairs: Vec<pdf_content::Repair>,
    grouping: Option<&Grouping>,
    fonts: Option<Arc<dyn pdf_content::FontProvider>>,
) -> Result<PageView, PageError> {
    view_of_inner(program, repairs, grouping, fonts, true)
}

fn view_of_inner(
    program: pdf_content::PageProgram,
    mut repairs: Vec<pdf_content::Repair>,
    grouping: Option<&Grouping>,
    fonts: Option<Arc<dyn pdf_content::FontProvider>>,
    tolerate_unsupported: bool,
) -> Result<PageView, PageError> {
    repairs.extend(program.streams.iter().flat_map(|stream| {
        stream.repairs.iter().map(|repair| {
            let pdf_syntax::StreamRepair::FilterEndedWithoutEod { byte_offset, .. } = repair;
            pdf_content::Repair::stream_decoding(*byte_offset, *repair)
        })
    }));
    let bytes: Vec<&ByteStore> = program.streams.iter().map(|stream| &stream.bytes).collect();
    let operations = parse_operation_sequence_strict(&bytes, ContentLimits::default())
        .map_err(PageError::Content)?;
    let paint_streams: Vec<_> = program
        .streams
        .iter()
        .zip(&operations)
        .map(|(stream, operations)| PaintStream {
            source: &stream.bytes,
            reference: stream.reference,
            operations,
        })
        .collect();
    let annotation_fonts = fonts.clone();
    let interpret = if tolerate_unsupported {
        pdf_paint::interpret_stream_sequence_tolerating_unsupported
    } else {
        pdf_paint::interpret_stream_sequence_with_fonts
    };
    let graph = interpret(
        &paint_streams,
        program.page,
        &[],
        &program.resources,
        PaintLimits::default(),
        fonts,
    )
    .map_err(|error| {
        let operator = error.operator_span().and_then(|span| {
            program
                .streams
                .iter()
                .find(|stream| stream.bytes.id() == span.source())
                .and_then(|stream| stream.bytes.resolve(span).ok())
                .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        });
        PageError::Paint(Box::new(PaintFailure { error, operator }))
    })?;
    drop(paint_streams);
    let annotations = pdf_paint::interpret_annotations(
        &program.annotations,
        program.page,
        program.geometry.rotate,
        &program.resources,
        PaintLimits::default(),
        annotation_fonts.as_ref(),
    );
    let index = grouping.map_or_else(
        || SemanticIndex::of(&graph),
        |grouping| SemanticIndex::of_grouped(&graph, grouping),
    );
    Ok(PageView {
        program,
        repairs,
        operations,
        graph,
        annotations,
        index,
    })
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Door {
    Editing,
    Display,
}

pub const PAGE_CACHE_BYTES: usize = 192 * 1024 * 1024;

pub const PAGE_CACHE_PAGES: usize = 24;

#[derive(Debug)]
pub struct Session {
    id: SessionId,
    credential: Vec<u8>,
    history: History,
    pages: BTreeMap<(usize, Door), Arc<PageView>>,
    order: Vec<(usize, Door)>,
    bytes: usize,
    groupings: BTreeMap<usize, Arc<Grouping>>,
    grouping_done: Vec<GroupingStep>,
    grouping_undone: Vec<GroupingStep>,
    fonts: Option<Arc<dyn pdf_content::FontProvider>>,
    scope: Option<Arc<pdf_content::DecipherScope>>,
    revision: RevisionId,
    last_pages: Option<pdf_edit::PageChange>,
    last_spread: Vec<usize>,
}

fn spread_of(pages: &[usize]) -> Vec<usize> {
    let mut named: Vec<usize> = pages.to_vec();
    named.sort_unstable();
    named.dedup();
    if named.len() < 2 {
        return Vec::new();
    }
    named
}

impl Clone for Session {
    fn clone(&self) -> Self {
        Self {
            id: SessionId::next(),
            credential: self.credential.clone(),
            history: self.history.clone(),
            pages: self.pages.clone(),
            order: self.order.clone(),
            bytes: self.bytes,
            groupings: self.groupings.clone(),
            grouping_done: self.grouping_done.clone(),
            grouping_undone: self.grouping_undone.clone(),
            fonts: self.fonts.clone(),
            scope: self.scope.clone(),
            revision: self.revision,
            last_pages: self.last_pages.clone(),
            last_spread: self.last_spread.clone(),
        }
    }
}

#[derive(Clone, Debug)]
struct GroupingStep {
    page: usize,
    before: Option<Arc<Grouping>>,
    after: Option<Arc<Grouping>>,
    pages: Option<pdf_edit::PageChange>,
    taken: Vec<(usize, Arc<Grouping>)>,
    parked: bool,
    spread: Vec<usize>,
    regrouped: Vec<Regrouped>,
}

impl Session {
    #[must_use]
    pub fn new(source: ByteStore, credential: &[u8]) -> Self {
        Self {
            id: SessionId::next(),
            credential: credential.to_vec(),
            history: History::new(source, credential),
            pages: BTreeMap::new(),
            order: Vec::new(),
            bytes: 0,
            groupings: BTreeMap::new(),
            grouping_done: Vec::new(),
            grouping_undone: Vec::new(),
            fonts: None,
            scope: None,
            revision: RevisionId::first(),
            last_pages: None,
            last_spread: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_fonts(
        source: ByteStore,
        credential: &[u8],
        fonts: Option<Arc<dyn pdf_content::FontProvider>>,
    ) -> Self {
        let scope = fonts.map(|inner| Arc::new(pdf_content::DecipherScope::new(inner)));
        Self {
            fonts: scope
                .clone()
                .map(|scope| -> Arc<dyn pdf_content::FontProvider> { scope }),
            scope,
            ..Self::new(source, credential)
        }
    }

    pub fn reads_by_glyphs(&mut self, page: usize, atoms: &[usize]) -> Result<bool, PageError> {
        let Some(provider) = self.fonts.clone() else {
            return Ok(false);
        };
        let view = self.page(page)?;
        Ok(pdf_paint::decipher_fonts::reads_by_glyphs(
            &view.graph,
            atoms,
            provider.as_ref(),
        ))
    }

    pub fn replacement_family(
        &mut self,
        page: usize,
        atoms: &[usize],
    ) -> Result<Option<String>, PageError> {
        let Some(provider) = self.fonts.clone() else {
            return Ok(None);
        };
        let view = self.page(page)?;
        Ok(pdf_paint::decipher_fonts::replacement_family(
            &view.graph,
            atoms,
            provider.as_ref(),
        ))
    }

    pub fn read_by_glyphs(&mut self, page: usize, atoms: &[usize]) -> Result<bool, PageError> {
        let Some(scope) = self.scope.clone() else {
            return Ok(false);
        };
        let view = self.page(page)?;
        let Some(region) = pdf_paint::decipher_fonts::region_of(&view.graph, atoms) else {
            return Ok(false);
        };
        let changed = scope.allow(view.program.page, region);
        if changed {
            self.release_page(page);
        }
        Ok(changed)
    }

    #[must_use]
    pub fn font_provider(&self) -> Option<&Arc<dyn pdf_content::FontProvider>> {
        self.fonts.as_ref()
    }

    #[must_use]
    pub const fn source(&self) -> &ByteStore {
        self.history.source()
    }

    #[must_use]
    pub const fn history(&self) -> &History {
        &self.history
    }

    pub fn page_count(&self) -> Result<usize, PageError> {
        pdf_content::count_pages_recovering(
            self.history.source(),
            PageContentLimits::default(),
            pdf_content::RecoverLimits::default(),
            &self.credential,
        )
        .map(|recovered| recovered.into_parts().0)
        .map_err(PageError::Page)
    }

    pub fn page_geometries(&self) -> Result<Vec<pdf_content::PageGeometry>, PageError> {
        pdf_content::page_geometries_recovering(
            self.history.source(),
            PageContentLimits::default(),
            pdf_content::RecoverLimits::default(),
            &self.credential,
        )
        .map(|recovered| recovered.into_parts().0)
        .map_err(PageError::Page)
    }

    #[must_use]
    pub fn held_pages(&self) -> usize {
        self.pages.len()
    }

    pub fn page(&mut self, index: usize) -> Result<Arc<PageView>, PageError> {
        if let Some(held) = self.recall((index, Door::Editing)) {
            return Ok(held);
        }
        let view = Arc::new(interpret_page_fully(
            self.history.source(),
            index,
            &self.credential,
            self.groupings.get(&index).map(AsRef::as_ref),
            self.fonts.clone(),
        )?);
        self.name_new_blocks(index, &view);
        self.hold((index, Door::Editing), &view);
        Ok(view)
    }

    fn recall(&mut self, key: (usize, Door)) -> Option<Arc<PageView>> {
        let held = self.pages.get(&key).map(Arc::clone)?;
        if let Some(at) = self.order.iter().position(|slot| *slot == key) {
            let key = self.order.remove(at);
            self.order.push(key);
        }
        Some(held)
    }

    fn hold(&mut self, key: (usize, Door), view: &Arc<PageView>) {
        self.release(key);
        self.bytes += view.footprint();
        self.pages.insert(key, Arc::clone(view));
        self.order.push(key);
        while (self.bytes > PAGE_CACHE_BYTES || self.order.len() > PAGE_CACHE_PAGES)
            && self.order.len() > 1
        {
            let oldest = self.order.remove(0);
            self.release(oldest);
        }
    }

    fn release(&mut self, key: (usize, Door)) {
        if let Some(gone) = self.pages.remove(&key) {
            self.bytes = self.bytes.saturating_sub(gone.footprint());
        }
        self.order.retain(|slot| *slot != key);
    }

    fn release_page(&mut self, index: usize) {
        self.release((index, Door::Editing));
        self.release((index, Door::Display));
    }

    pub fn page_for_display(&mut self, index: usize) -> Result<Arc<PageView>, PageError> {
        if let Some(held) = self.recall((index, Door::Display)) {
            return Ok(held);
        }
        if let Ok(view) = self.page(index) {
            return Ok(view);
        }
        let view = Arc::new(interpret_page_for_display(
            self.history.source(),
            index,
            &self.credential,
            self.groupings.get(&index).map(AsRef::as_ref),
            self.fonts.clone(),
        )?);
        self.name_new_blocks(index, &view);
        self.hold((index, Door::Display), &view);
        Ok(view)
    }

    pub fn forget_pages(&mut self) {
        self.pages.clear();
        self.order.clear();
        self.bytes = 0;
    }

    #[must_use]
    pub const fn last_pages(&self) -> Option<&pdf_edit::PageChange> {
        self.last_pages.as_ref()
    }

    #[must_use]
    pub fn last_spread(&self) -> &[usize] {
        &self.last_spread
    }

    #[must_use]
    pub const fn groupings(&self) -> &BTreeMap<usize, Arc<Grouping>> {
        &self.groupings
    }

    #[must_use]
    pub fn grouping(&self, page: usize) -> Option<Arc<Grouping>> {
        self.groupings.get(&page).cloned()
    }

    pub fn establish_grouping(&mut self, page: usize, grouping: Arc<Grouping>) {
        if let std::collections::btree_map::Entry::Vacant(entry) = self.groupings.entry(page) {
            entry.insert(grouping);
            self.release_page(page);
        }
    }

    pub fn plan(&mut self, command: &Command) -> Result<Plan, PlanError> {
        let view = self.page(command.page_index()).map_err(PlanError::Page)?;
        plan_command_in(
            self.history.source(),
            PlannerPage {
                program: &view.program,
                operations: &view.operations,
                graph: &view.graph,
                fonts: self.fonts.as_ref(),
                restrictions: self.history.restrictions(),
                credential: &self.credential,
            },
            command,
        )
        .map_err(PlanError::Plan)
    }

    #[must_use]
    pub fn credential(&self) -> &[u8] {
        &self.credential
    }

    pub fn lay_out_live(&mut self, command: &Command) -> Result<pdf_edit::LiveBlock, PlanError> {
        let view = self.page(command.page_index()).map_err(PlanError::Page)?;
        lay_out_live(
            self.history.source(),
            PlannerPage {
                program: &view.program,
                operations: &view.operations,
                graph: &view.graph,
                fonts: self.fonts.as_ref(),
                restrictions: self.history.restrictions(),
                credential: &self.credential,
            },
            command,
        )
        .map_err(PlanError::Plan)
    }

    pub fn restricts_editing(&self) -> Result<bool, pdf_content::PageContentError> {
        let program = pdf_content::load_page_program_with_password(
            self.history.source(),
            0,
            pdf_content::PageContentLimits::default(),
            &self.credential,
        )?;
        Ok(!program.may_modify_content())
    }

    pub fn set_aside_restrictions(&mut self) {
        self.history.set_aside_restrictions();
    }

    #[must_use]
    pub const fn restrictions(&self) -> pdf_edit::Restrictions {
        self.history.restrictions()
    }

    pub fn apply(&mut self, plan: Plan) -> Result<(), SpikeError> {
        self.apply_holding(plan, None)
    }

    pub fn apply_each(&mut self, commands: &[Command]) -> Result<(), SpikeError> {
        let spread: Vec<usize> = commands.iter().map(Command::page_index).collect();
        let page = spread.first().copied().unwrap_or(0);
        let (credential, fonts) = (self.credential.clone(), self.fonts.clone());
        let restrictions = self.history.restrictions();
        let mut said: Vec<Said> = Vec::new();
        self.history
            .apply_together_with(commands.len(), |source, at| {
                let plan = pdf_edit::spike_move_text::plan_command_under(
                    source,
                    &commands[at],
                    &credential,
                    (fonts.as_ref(), restrictions),
                )?;
                said.push((
                    plan.effect().page_index,
                    plan.correspondence().cloned(),
                    plan.inserted().to_vec(),
                ));
                Ok(plan)
            })?;
        let regrouped = self.carry_groupings(&said);
        self.last_pages = None;
        self.last_spread = spread_of(&spread);
        self.grouping_done.push(GroupingStep {
            page,
            before: None,
            after: None,
            pages: None,
            taken: Vec::new(),
            parked: false,
            spread,
            regrouped,
        });
        self.grouping_undone.clear();
        self.forget_pages();
        self.revision = self.revision.next();
        Ok(())
    }

    fn carry_groupings(&mut self, said: &[Said]) -> Vec<Regrouped> {
        let mut walked: Vec<Regrouped> = Vec::new();
        for (page, mapping, inserted) in said {
            let before = self.groupings.get(page).cloned();
            let after = before
                .as_ref()
                .zip(mapping.as_ref())
                .map(|(grouping, mapping)| {
                    Arc::new(grouping.remapped_with_insertions(mapping, inserted))
                });
            self.set_grouping(*page, after.clone());
            match walked.iter_mut().find(|(at, _, _)| at == page) {
                Some(entry) => entry.2 = after,
                None => walked.push((*page, before, after)),
            }
        }
        walked
    }

    fn walk_spread(&mut self, spread: &[usize], regrouped: &[Regrouped], back: bool) {
        for page in spread {
            match regrouped.iter().find(|(at, _, _)| at == page) {
                Some((_, before, after)) => {
                    self.set_grouping(*page, if back { before.clone() } else { after.clone() });
                }
                None => {
                    self.groupings.remove(page);
                }
            }
        }
        self.forget_pages();
    }

    pub fn apply_holding(
        &mut self,
        plan: Plan,
        reading: Option<Arc<PageView>>,
    ) -> Result<(), SpikeError> {
        let page = plan.effect().page_index;
        let before = self.groupings.get(&page).cloned();
        let after = before
            .as_ref()
            .zip(plan.correspondence())
            .map(|(grouping, mapping)| {
                Arc::new(grouping.remapped_with_insertions(mapping, plan.inserted()))
            });
        let pages = plan.pages().cloned();
        self.history.apply(plan)?;
        self.last_pages.clone_from(&pages);
        self.last_spread.clear();
        if let Some(change) = pages {
            let taken = match &change {
                pdf_edit::PageChange::Removed(gone) => gone
                    .iter()
                    .filter_map(|at| Some((*at, Arc::clone(self.groupings.get(at)?))))
                    .collect(),
                _ => Vec::new(),
            };
            self.renumber_pages(&change, false);
            self.grouping_done.push(GroupingStep {
                page: change.first(),
                before: None,
                after: None,
                pages: Some(change),
                taken,
                parked: false,
                spread: Vec::new(),
                regrouped: Vec::new(),
            });
            self.grouping_undone.clear();
            self.forget_pages();
            self.revision = self.revision.next();
            return Ok(());
        }
        self.set_grouping(page, after.clone());
        self.grouping_done.push(GroupingStep {
            page,
            before,
            after,
            pages: None,
            taken: Vec::new(),
            parked: false,
            spread: Vec::new(),
            regrouped: Vec::new(),
        });
        self.grouping_undone.clear();
        self.forget_last_step();
        self.revision = self.revision.next();
        if let Some(reading) = reading {
            self.hold((page, Door::Editing), &reading);
        }
        Ok(())
    }

    #[must_use]
    pub fn held_page(&self, index: usize) -> Option<Arc<PageView>> {
        self.pages.get(&(index, Door::Editing)).map(Arc::clone)
    }

    pub fn preview(&self, plan: &Plan) -> Result<PageView, String> {
        let page = plan.effect().page_index;
        let bytes = self
            .history
            .preview(plan)
            .map_err(|error| error.to_string())?;
        let grouping = self
            .grouping(page)
            .zip(plan.correspondence())
            .map(|(grouping, mapping)| grouping.remapped_with_insertions(mapping, plan.inserted()));
        interpret_page_fully(
            &bytes,
            page,
            &self.credential,
            grouping.as_ref(),
            self.fonts.clone(),
        )
        .map_err(|error| error.to_string())
    }

    pub fn undo(&mut self) -> Result<bool, SpikeError> {
        let walked = self.history.undo()?;
        if walked {
            self.last_pages = None;
            self.last_spread.clear();
            if let Some(step) = self.grouping_done.pop() {
                if let Some(change) = &step.pages {
                    let back = change.undone();
                    self.renumber_pages(&back, true);
                    self.last_pages = Some(back);
                    for (at, grouping) in &step.taken {
                        self.groupings.insert(*at, Arc::clone(grouping));
                    }
                    self.forget_pages();
                } else if step.spread.is_empty() {
                    self.set_grouping(step.page, step.before.clone());
                } else {
                    let spread = step.spread.clone();
                    self.walk_spread(&spread, &step.regrouped, true);
                    self.last_spread = spread_of(&spread);
                }
                self.grouping_undone.push(step);
            }
            self.forget_last_step();
            self.revision = self.revision.next();
        }
        Ok(walked)
    }

    pub fn redo(&mut self) -> Result<bool, SpikeError> {
        let walked = self.history.redo()?;
        if walked {
            self.last_pages = None;
            self.last_spread.clear();
            if let Some(step) = self.grouping_undone.pop() {
                if let Some(change) = &step.pages {
                    self.last_pages = Some(change.clone());
                    self.renumber_pages(change, false);
                    self.forget_pages();
                } else if step.spread.is_empty() {
                    self.set_grouping(step.page, step.after.clone());
                } else {
                    let spread = step.spread.clone();
                    self.walk_spread(&spread, &step.regrouped, false);
                    self.last_spread = spread_of(&spread);
                }
                self.grouping_done.push(step);
            }
            self.forget_last_step();
            self.revision = self.revision.next();
        }
        Ok(walked)
    }

    fn forget_last_step(&mut self) {
        match self.history.last_page() {
            Some(page) => self.release_page(page),
            None => self.forget_pages(),
        }
    }

    fn renumber_pages(&mut self, change: &pdf_edit::PageChange, walking_back: bool) {
        self.groupings = std::mem::take(&mut self.groupings)
            .into_iter()
            .filter_map(|(page, grouping)| change.renumbered(page).map(|page| (page, grouping)))
            .collect();
        let mut unparking = walking_back;
        for step in self.grouping_done.iter_mut().rev() {
            if step.pages.is_some() {
                if matches!(step.pages, Some(pdf_edit::PageChange::Removed(_))) {
                    unparking = false;
                }
                continue;
            }
            if step.parked {
                if unparking && matches!(change, pdf_edit::PageChange::Added(_)) {
                    step.parked = false;
                }
                continue;
            }
            match change.renumbered(step.page) {
                Some(page) => step.page = page,
                None => step.parked = true,
            }
        }
    }

    fn name_new_blocks(&mut self, index: usize, view: &PageView) {
        if self
            .groupings
            .get(&index)
            .is_none_or(|_| view.index.report.clusters_outside_the_grouping > 0)
        {
            self.groupings
                .insert(index, Arc::new(Grouping::of(&view.index)));
        }
    }

    fn set_grouping(&mut self, page: usize, grouping: Option<Arc<Grouping>>) {
        if let Some(grouping) = grouping {
            self.groupings.insert(page, grouping);
        } else {
            self.groupings.remove(&page);
        }
    }
}

#[derive(Debug)]
pub enum PlanError {
    Page(PageError),
    Plan(SpikeError),
}

impl fmt::Display for PlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Page(error) => error.fmt(formatter),
            Self::Plan(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for PlanError {}

#[derive(Clone, Debug, PartialEq)]
pub enum PageError {
    Page(PageContentError),
    Content(ContentError),
    Paint(Box<PaintFailure>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaintFailure {
    pub error: InterpretError,
    pub operator: Option<String>,
}

impl fmt::Display for PageError {
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
        }
    }
}

impl std::error::Error for PageError {}

#[cfg(test)]
mod ownership_tests;

#[cfg(test)]
mod gesture_tests;

#[cfg(test)]
mod select_tests;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};
    use pdf_edit::{Command, Document, TextRunSelection};
    use pdf_paint::glyph_placement_signature;
    use pdf_syntax::XrefLimits;

    use super::{
        Door, PAGE_CACHE_BYTES, PageError, PageView, Session, interpret_page,
        interpret_page_for_display, interpret_page_for_print,
    };

    pub(crate) const TINY_CFF: &str = "0100040100010101055465737400010101131d00000030111d0000004c0f1d0000\
        0051100001010106616c706861000000030101020c160e8b8b158c8b058b8c050e8b8b158c8b058b8c050e00002201\
        87000141";

    pub(crate) fn hex(text: &str) -> Vec<u8> {
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

    fn flagged_annotations_fixture(flags: &[u32]) -> ByteStore {
        let mut bodies: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        ];
        let first = 4;
        let annots: Vec<String> = (0..flags.len())
            .map(|at| format!("{} 0 R", first + at * 2))
            .collect();
        bodies.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Resources << >> /Annots [{}] >>",
                annots.join(" ")
            )
            .into_bytes(),
        );
        for (at, flag) in flags.iter().enumerate() {
            let x = 10 + at * 30;
            bodies.push(
                format!(
                    "<< /Type /Annot /Subtype /Square /Rect [{x} 10 {} 30] /F {flag} /AP << /N {} 0 R >> >>",
                    x + 20,
                    first + at * 2 + 1
                )
                .into_bytes(),
            );
            let paint = b"0 0 20 20 re f";
            let mut stream = format!(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length {} >>\nstream\n",
                paint.len()
            )
            .into_bytes();
            stream.extend_from_slice(paint);
            stream.extend_from_slice(b"\nendstream");
            bodies.push(stream);
        }
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (at, body) in bodies.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n", at + 1).as_bytes());
            bytes.extend_from_slice(body);
            bytes.extend_from_slice(b"\nendobj\n");
        }
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
        ByteStore::new(SourceId::new(92), Arc::<[u8]>::from(bytes))
    }

    #[test]
    fn paper_draws_the_annotations_that_ask_to_be_printed() {
        let source = flagged_annotations_fixture(&[0, 4, 36, 32, 6]);
        let drawn = |view: &PageView| -> Vec<bool> {
            view.annotations
                .iter()
                .map(|annotation| {
                    annotation.outcome == pdf_paint::AnnotationOutcome::Drawn
                        && !annotation.graph.atoms.is_empty()
                })
                .collect()
        };
        let screen = interpret_page_for_display(&source, 0, b"", None, None).expect("the page");
        let paper = interpret_page_for_print(&source, 0, b"", None).expect("the page");
        assert_eq!(drawn(&screen), vec![true, true, false, false, false]);
        assert_eq!(drawn(&paper), vec![false, true, true, false, false]);
    }

    fn two_page_fixture() -> ByteStore {
        let first = b"BT /F1 12 Tf 5 5 Td (A) Tj ET";
        let second = b"BT /F1 12 Tf 5 50 Td (A) Tj ET";
        let program = hex(TINY_CFF);
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        let object = |bytes: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]| {
            offsets.push(bytes.len());
            bytes.extend_from_slice(body);
        };
        object(
            &mut bytes,
            &mut offsets,
            b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        );
        object(
            &mut bytes,
            &mut offsets,
            b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 100 100] /Kids [3 0 R 8 0 R] /Count 2 >>\nendobj\n",
        );
        object(
            &mut bytes,
            &mut offsets,
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", first.len()).as_bytes(),
        );
        bytes.extend_from_slice(first);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        object(
            &mut bytes,
            &mut offsets,
            b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Test /FirstChar 65 /LastChar 65 /Widths [600] /FontDescriptor 6 0 R >>\nendobj\n",
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
        object(
            &mut bytes,
            &mut offsets,
            b"8 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 9 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!("9 0 obj\n<< /Length {} >>\nstream\n", second.len()).as_bytes(),
        );
        bytes.extend_from_slice(second);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
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
        ByteStore::new(SourceId::new(91), Arc::<[u8]>::from(bytes))
    }

    fn move_last_run(session: &Session, page_index: usize, dx: f64) -> pdf_edit::Plan {
        let document = Document::open_strict(session.source().clone(), XrefLimits::default())
            .expect("the fixture opens");
        document
            .begin_transaction()
            .plan(
                &Command::MoveTextRun {
                    page_index,
                    selection: TextRunSelection::Last,
                    dx,
                    dy: 0.0,
                },
                b"",
            )
            .expect("the page has a run to move")
    }

    #[test]
    fn commands_over_several_pages_are_one_undo() {
        let pages = |source: &ByteStore| -> Vec<String> {
            (0..2)
                .map(|page| {
                    let program = pdf_content::load_page_program_strict(
                        source,
                        page,
                        pdf_content::PageContentLimits::default(),
                    )
                    .expect("the page loads");
                    String::from_utf8_lossy(program.streams[0].bytes.as_bytes()).into_owned()
                })
                .collect()
        };
        let at_rest = two_page_fixture();
        let mut session = Session::new(at_rest.clone(), b"");
        let moved = |page_index| Command::MoveTextRun {
            page_index,
            selection: TextRunSelection::Last,
            dx: 4.0,
            dy: 0.0,
        };
        session
            .apply_each(&[moved(0), moved(1)])
            .expect("both pages move");
        let after = pages(session.source());
        let before = pages(&at_rest);
        assert!(
            after.iter().zip(&before).all(|(now, was)| now != was),
            "both pages changed"
        );
        assert_eq!(session.history().undo_depth(), 1, "two pages, one step");
        assert_eq!(session.last_spread(), &[0, 1]);

        assert!(session.undo().expect("undo walks"));
        assert!(
            pages(session.source()) == before,
            "one undo, both pages back"
        );
        assert!(!session.history().can_undo());
        assert!(session.redo().expect("redo walks"));
        assert!(pages(session.source()) == after, "one redo, both forward");
        assert_eq!(session.last_spread(), &[0, 1]);
        let one = session
            .plan(&moved(0))
            .expect("a step of one page is planned");
        session.apply(one).expect("it commits");
        assert!(session.last_spread().is_empty(), "one page names itself");

        let group = Session::new(at_rest.clone(), b"")
            .apply_each(&[moved(0), moved(0)])
            .err();
        assert!(group.is_none(), "two commands on one page commit");
        let mut together = Session::new(at_rest.clone(), b"");
        together
            .apply_each(&[moved(0), moved(0)])
            .expect("two commands on one page");
        assert!(
            together.last_spread().is_empty(),
            "one page named twice is still one page"
        );
        assert!(together.undo().expect("undo walks"));
        assert!(
            together.last_spread().is_empty(),
            "and the undo of it names one page too"
        );
        assert!(together.redo().expect("redo walks"));
        assert!(
            together.last_spread().is_empty(),
            "and so does the redo of it"
        );

        let mut refused = Session::new(at_rest.clone(), b"");
        let wrong = Command::MoveTextRun {
            page_index: 1,
            selection: TextRunSelection::Ordinal(99),
            dx: 4.0,
            dy: 0.0,
        };
        assert!(refused.apply_each(&[moved(0), wrong]).is_err());
        assert_eq!(refused.source().as_bytes(), at_rest.as_bytes());
        assert_eq!(refused.history().undo_depth(), 0);
        assert!(refused.apply_each(&[]).is_err(), "no command is no step");
    }

    #[test]
    fn a_document_knows_how_many_pages_it_has() {
        let mut session = Session::new(two_page_fixture(), b"");
        assert_eq!(session.page_count().expect("the tree walks"), 2);
        let plan = move_last_run(&session, 0, 4.0);
        session.apply(plan).expect("the run moves");
        assert_eq!(
            session.page_count().expect("the tree still walks"),
            2,
            "an edit that moves a run must not change how many pages there are"
        );
    }

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "a size written as a number is read back as the same number"
    )]
    fn a_blank_page_goes_in_comes_out_and_goes_in_again() {
        let mut session = Session::new(two_page_fixture(), b"");
        let moved = move_last_run(&session, 1, 4.0);
        session
            .apply(moved)
            .expect("the run on the second page moves");
        let added = session
            .plan(&Command::AddBlankPage {
                beside: 0,
                before: false,
                size: [612.0, 792.0],
            })
            .expect("the page is planned");
        assert_eq!(added.pages(), Some(&pdf_edit::PageChange::Added(vec![1])));
        session.apply(added).expect("the page goes in");
        let sizes = |session: &Session| -> Vec<[f64; 4]> {
            session
                .page_geometries()
                .expect("laid out")
                .iter()
                .map(|geometry| geometry.media_box)
                .collect()
        };
        assert_eq!(sizes(&session).len(), 3);
        assert_eq!(sizes(&session)[1], [0.0, 0.0, 612.0, 792.0]);
        assert_eq!(session.history().last_page(), Some(1));

        assert!(session.undo().expect("undoes"), "the page comes out");
        assert_eq!(sizes(&session).len(), 2);
        assert!(session.undo().expect("undoes"), "the move is undone");
        assert_eq!(session.page_count().expect("walks"), 2);

        assert!(session.redo().expect("redoes"), "the move again");
        assert!(session.redo().expect("redoes"), "the page again");
        assert_eq!(sizes(&session)[1], [0.0, 0.0, 612.0, 792.0]);
    }

    #[test]
    fn an_edit_on_a_page_taken_out_is_undone_on_it_once_it_is_back() {
        let mut session = Session::new(two_page_fixture(), b"");
        let order = |session: &Session| -> Vec<u32> {
            pdf_content::page_references_with_password(
                session.source(),
                pdf_content::PageContentLimits::default(),
                b"",
            )
            .expect("listed")
            .iter()
            .map(|page| page.object_number())
            .collect()
        };
        let pages = order(&session);
        let view = session.page(1).expect("the second page reads");
        session.establish_grouping(1, Arc::new(super::Grouping::of(&view.index)));
        let moved = move_last_run(&session, 1, 4.0);
        session
            .apply(moved)
            .expect("the run on the second page moves");
        assert_eq!(session.history().last_page(), Some(1));

        let plan = session
            .plan(&Command::MovePages {
                pages: vec![1],
                to: 0,
            })
            .expect("the move is planned");
        session.apply(plan).expect("the second page goes first");
        assert_eq!(order(&session), vec![pages[1], pages[0]]);
        let plan = session
            .plan(&Command::RemovePages { pages: vec![0] })
            .expect("the removal is planned");
        session.apply(plan).expect("it goes");
        assert_eq!(order(&session), vec![pages[0]]);

        assert!(session.undo().expect("undoes"), "the page comes back");
        assert!(session.undo().expect("undoes"), "and goes back to second");
        assert_eq!(order(&session), pages);
        assert!(session.undo().expect("undoes"), "the run on it moves back");
        assert_eq!(session.history().last_page(), Some(1));
        assert!(
            session.grouping(1).is_some(),
            "the second page keeps its blocks"
        );
        assert!(
            session.grouping(0).is_none(),
            "and they were not put back on the first"
        );
        assert!(!session.undo().expect("walks"), "nothing is left to undo");

        for _ in 0..3 {
            assert!(session.redo().expect("redoes"));
        }
        assert_eq!(order(&session), vec![pages[0]]);
    }

    #[test]
    fn a_page_is_read_once_and_then_handed_back() {
        let mut session = Session::new(two_page_fixture(), b"");
        let first = session.page(0).expect("the page interprets");
        let again = session.page(0).expect("the page is still there");
        assert!(
            Arc::ptr_eq(&first, &again),
            "the second ask must not have read the page again"
        );
        assert_eq!(session.held_pages(), 1);
    }

    #[test]
    fn a_session_gives_up_its_least_recently_used_page_rather_than_grow() {
        let mut session = Session::new(two_page_fixture(), b"");
        session.page(0).expect("page 0 interprets");
        session.page(1).expect("page 1 interprets");
        assert_eq!(session.held_pages(), 2, "both fit under the real budget");

        session.bytes = PAGE_CACHE_BYTES + 1;
        let third = session.page(0).expect("page 0 is still held");
        assert!(
            session.held_pages() >= 1,
            "a cache that holds nothing re-reads every page on every ask"
        );
        assert!(
            session.pages.contains_key(&(0, Door::Editing)),
            "the page just asked for is never the one given up"
        );
        drop(third);
    }

    #[test]
    fn asking_for_a_page_again_moves_it_out_of_the_way_of_eviction() {
        let mut session = Session::new(two_page_fixture(), b"");
        session.page(0).expect("page 0 interprets");
        session.page(1).expect("page 1 interprets");
        assert_eq!(session.order, vec![(0, Door::Editing), (1, Door::Editing)]);
        session.page(0).expect("page 0 is held");
        assert_eq!(
            session.order,
            vec![(1, Door::Editing), (0, Door::Editing)],
            "the page just used is the last to be given up"
        );
    }

    #[test]
    fn giving_up_a_page_gives_up_the_bytes_it_was_counted_for() {
        let mut session = Session::new(two_page_fixture(), b"");
        session.page(0).expect("page 0 interprets");
        let one = session.bytes;
        session.page(1).expect("page 1 interprets");
        assert!(session.bytes > one, "the second page cost nothing");
        session.forget_pages();
        assert_eq!(session.bytes, 0);
        assert_eq!(session.held_pages(), 0);
        assert!(session.order.is_empty());
        session.page(0).expect("page 0 re-reads");
        assert_eq!(session.bytes, one, "the same page cost a different amount");
    }

    #[test]
    fn an_edit_forgets_both_doors_onto_the_page_it_named() {
        let mut session = Session::new(two_page_fixture(), b"");
        let view = session.page(0).expect("page 0 interprets");
        session.pages.insert((0, Door::Display), view);
        session.order.push((0, Door::Display));
        let plan = move_last_run(&session, 0, 7.0);
        session.apply(plan).expect("the run moves");
        assert!(!session.pages.contains_key(&(0, Door::Editing)));
        assert!(
            !session.pages.contains_key(&(0, Door::Display)),
            "the display reading survived an edit to its own page"
        );
    }

    #[test]
    fn a_held_page_says_what_a_fresh_read_says() {
        let mut session = Session::new(two_page_fixture(), b"");
        let held = session.page(1).expect("the page interprets");
        let fresh = interpret_page(session.source(), 1).expect("the page interprets again");
        assert_eq!(
            glyph_placement_signature(&held.graph),
            glyph_placement_signature(&fresh.graph)
        );
        assert!(!held.graph.atoms.is_empty(), "the fixture paints something");
    }

    #[test]
    fn an_edit_forgets_its_own_page_and_keeps_the_rest_correct() {
        let mut session = Session::new(two_page_fixture(), b"");
        let before_0 = session.page(0).expect("page 0 interprets");
        let before_1 = session.page(1).expect("page 1 interprets");
        assert_eq!(session.held_pages(), 2);

        let plan = move_last_run(&session, 0, 7.0);
        session.apply(plan).expect("the run moves");

        assert_eq!(
            session.held_pages(),
            1,
            "exactly one page should have been forgotten"
        );
        let after_1 = session.page(1).expect("page 1 is still held");
        assert!(
            Arc::ptr_eq(&before_1, &after_1),
            "the untouched page was read again"
        );

        let fresh_1 = interpret_page(session.source(), 1).expect("page 1 reopens");
        assert_eq!(
            glyph_placement_signature(&after_1.graph),
            glyph_placement_signature(&fresh_1.graph),
            "the page the session kept is not what the new revision paints"
        );

        let after_0 = session.page(0).expect("page 0 reopens");
        assert!(
            !Arc::ptr_eq(&before_0, &after_0),
            "the edited page was served from before the edit"
        );
        assert_ne!(
            glyph_placement_signature(&before_0.graph),
            glyph_placement_signature(&after_0.graph),
            "the edit put no glyph anywhere new"
        );
    }

    #[test]
    fn a_restricted_document_is_edited_only_once_its_restrictions_are_set_aside() {
        let source = ByteStore::new(
            pdf_bytes::SourceId::new(0),
            &include_bytes!("../../pdf-edit/tests/data/restricted-r3.pdf")[..],
        );
        let command = pdf_edit::Command::MoveTextRun {
            page_index: 0,
            selection: pdf_edit::TextRunSelection::Last,
            dx: 1.0,
            dy: 0.0,
        };
        let mut session = Session::new(source.clone(), b"view");
        assert!(session.restricts_editing().unwrap());
        assert_eq!(session.restrictions(), pdf_edit::Restrictions::Respect);
        assert!(session.plan(&command).is_err());
        session.set_aside_restrictions();
        let plan = session.plan(&command).expect("planned once set aside");
        session.apply(plan).expect("committed once set aside");
        assert!(session.restricts_editing().unwrap(), "the file keeps them");

        let owner = Session::new(source, b"master");
        assert!(!owner.restricts_editing().unwrap());
    }

    #[test]
    fn walking_the_history_forgets_the_page_each_step_lands_on() {
        let mut session = Session::new(two_page_fixture(), b"");
        let original = glyph_placement_signature(&session.page(0).expect("page 0").graph);
        let plan = move_last_run(&session, 0, 7.0);
        session.apply(plan).expect("the run moves");
        let moved = glyph_placement_signature(&session.page(0).expect("page 0").graph);

        session.page(1).expect("page 1 interprets");
        assert!(session.undo().expect("the move undoes"));
        assert_eq!(session.held_pages(), 1, "page 1 should have been kept");
        assert_eq!(
            glyph_placement_signature(&session.page(0).expect("page 0").graph),
            original,
            "undo left the moved page on show"
        );

        assert!(session.redo().expect("the move redoes"));
        assert_eq!(
            glyph_placement_signature(&session.page(0).expect("page 0").graph),
            moved,
            "redo left the unmoved page on show"
        );
    }

    #[test]
    fn a_refused_page_is_not_held() {
        let mut session = Session::new(two_page_fixture(), b"");
        let error = session.page(7).expect_err("the fixture has two pages");
        assert!(matches!(error, PageError::Page(_)));
        assert_eq!(session.held_pages(), 0);
    }
}

#[cfg(test)]
mod font_tests;
