use pdf_bytes::{ByteStore, SourceId};
use pdf_content::{
    ContentLimits, PageContentError, PageContentLimits, load_page_program_with_password,
    parse_operation_sequence_strict,
};
use pdf_paint::{
    FormInvocation, InterpretError, Matrix, PaintAtomKind, PaintLimits, PaintStream, Point,
    TextShowPaint,
};
use pdf_syntax::{Object, ObjectKind, Reference};

use crate::incremental::IncrementalWriteError;
use crate::plan::{
    BlockRange, Capability, Command, Effect, GlyphChange, ObjectSelection, Plan, PlannedBody,
    PlannedWrite, RunRewrite, SourceAnchor, TextRunSelection,
};
use crate::split::ClusterEdit;

#[derive(Clone, Debug)]
pub struct MovedTextRun {
    pub source: ByteStore,
    pub original_length: usize,
    pub content_stream: Reference,
    pub atom_ordinal: usize,
    pub original_matrix: Matrix,
    pub declared_region: Option<[f64; 4]>,
    pub edited_glyphs: usize,
    pub command: Command,
}

pub fn move_last_text_run(
    source: &ByteStore,
    page_index: usize,
    dx: f64,
    dy: f64,
    credential: &[u8],
) -> Result<MovedTextRun, SpikeError> {
    move_last_text_run_with_fonts(source, page_index, dx, dy, credential, None)
}

pub fn move_last_text_run_with_fonts(
    source: &ByteStore,
    page_index: usize,
    dx: f64,
    dy: f64,
    credential: &[u8],
    fonts: Option<std::sync::Arc<dyn pdf_content::FontProvider>>,
) -> Result<MovedTextRun, SpikeError> {
    let command = Command::MoveTextRun {
        page_index,
        selection: TextRunSelection::Last,
        dx,
        dy,
    };
    let plan = plan_command_with_fonts(source, &command, credential, fonts)?;
    let effect = plan.effect().clone();
    let edited = plan.commit(source, credential)?;
    Ok(MovedTextRun {
        source: edited,
        original_length: source.len(),
        content_stream: effect.target_stream,
        atom_ordinal: effect.atom_ordinal(),
        original_matrix: effect.original_matrix(),
        declared_region: effect.declared_region,
        edited_glyphs: 0,
        command,
    })
}

pub(crate) struct PageReading {
    pub(crate) program: pdf_content::PageProgram,
    pub(crate) operations: Vec<Vec<pdf_content::Operation>>,
    pub(crate) graph: pdf_paint::PaintGraph,
}

pub(crate) fn read_page(
    source: &ByteStore,
    page_index: usize,
    credential: &[u8],
    fonts: crate::Fonts<'_>,
) -> Result<PageReading, SpikeError> {
    let program = load_page_program_with_password(
        source,
        page_index,
        PageContentLimits::default(),
        credential,
    )
    .map_err(SpikeError::Page)?;
    let bytes: Vec<&ByteStore> = program.streams.iter().map(|stream| &stream.bytes).collect();
    let operations = parse_operation_sequence_strict(&bytes, ContentLimits::default())
        .map_err(|_| SpikeError::UnsupportedContentLayout)?;
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
    let graph = pdf_paint::interpret_stream_sequence_with_fonts(
        &paint_streams,
        program.page,
        &[],
        &program.resources,
        PaintLimits::default(),
        fonts.cloned(),
    )
    .map_err(SpikeError::Interpret)?;
    drop(paint_streams);
    Ok(PageReading {
        program,
        operations,
        graph,
    })
}

pub fn move_last_cluster(
    source: &ByteStore,
    page_index: usize,
    dx: f64,
    dy: f64,
    credential: &[u8],
) -> Result<MovedTextRun, SpikeError> {
    move_last_cluster_with_fonts(source, page_index, dx, dy, credential, None)
}

pub fn move_last_cluster_with_fonts(
    source: &ByteStore,
    page_index: usize,
    dx: f64,
    dy: f64,
    credential: &[u8],
    fonts: Option<std::sync::Arc<dyn pdf_content::FontProvider>>,
) -> Result<MovedTextRun, SpikeError> {
    let glyphs = last_cluster_of_last_run(source, page_index, credential, fonts.as_ref())?;
    let glyphs_named = glyphs.len();
    let command = Command::MoveTextCluster {
        page_index,
        selection: TextRunSelection::Last,
        glyphs,
        dx,
        dy,
    };
    let plan = plan_command_with_fonts(source, &command, credential, fonts)?;
    let effect = plan.effect().clone();
    let edited = plan.commit(source, credential)?;
    Ok(MovedTextRun {
        source: edited,
        original_length: source.len(),
        content_stream: effect.target_stream,
        atom_ordinal: effect.atom_ordinal(),
        original_matrix: effect.original_matrix(),
        declared_region: effect.declared_region,
        edited_glyphs: glyphs_named,
        command,
    })
}

pub fn delete_last_cluster(
    source: &ByteStore,
    page_index: usize,
    credential: &[u8],
) -> Result<MovedTextRun, SpikeError> {
    delete_last_cluster_with_fonts(source, page_index, credential, None)
}

pub fn delete_last_cluster_with_fonts(
    source: &ByteStore,
    page_index: usize,
    credential: &[u8],
    fonts: Option<std::sync::Arc<dyn pdf_content::FontProvider>>,
) -> Result<MovedTextRun, SpikeError> {
    let glyphs = last_cluster_of_last_run(source, page_index, credential, fonts.as_ref())?;
    let glyphs_named = glyphs.len();
    let command = Command::DeleteTextClusters {
        page_index,
        selection: TextRunSelection::Last,
        glyphs,
    };
    let plan = plan_command_with_fonts(source, &command, credential, fonts)?;
    let effect = plan.effect().clone();
    let edited = plan.commit(source, credential)?;
    Ok(MovedTextRun {
        source: edited,
        original_length: source.len(),
        content_stream: effect.target_stream,
        atom_ordinal: effect.atom_ordinal(),
        original_matrix: effect.original_matrix(),
        declared_region: effect.declared_region,
        edited_glyphs: glyphs_named,
        command,
    })
}

pub fn delete_last_row_selection(
    source: &ByteStore,
    page_index: usize,
    at_most: usize,
    credential: &[u8],
) -> Result<MovedTextRun, SpikeError> {
    delete_last_row_selection_with_fonts(source, page_index, at_most, credential, None)
}

pub fn delete_last_row_selection_with_fonts(
    source: &ByteStore,
    page_index: usize,
    at_most: usize,
    credential: &[u8],
    fonts: Option<std::sync::Arc<dyn pdf_content::FontProvider>>,
) -> Result<MovedTextRun, SpikeError> {
    let graph = read_page(source, page_index, credential, fonts.as_ref())?.graph;
    let index = pdf_semantics::SemanticIndex::of(&graph);
    let line = index
        .lines
        .len()
        .checked_sub(1)
        .ok_or(SpikeError::NoTextRun)?;
    let clusters = index.lines[line].clusters.len();
    let take = at_most.min(clusters);
    if take == 0 {
        return Err(SpikeError::NoTextRun);
    }
    let spans = index
        .selection_spans(
            pdf_semantics::Caret {
                line,
                offset: clusters - take,
            },
            pdf_semantics::Caret {
                line,
                offset: clusters,
            },
        )
        .map_err(SpikeError::Selection)?;
    let glyphs_named = spans.iter().map(|span| span.glyphs.len()).sum();
    let runs = spans
        .into_iter()
        .map(|span| RunRewrite {
            anchor: SourceAnchor::of(&graph.atoms[span.atom].id),
            glyphs: Some(GlyphChange::Remove {
                glyphs: span.glyphs,
                close_gap: false,
            }),
            displace: (0.0, 0.0),
        })
        .collect();
    let command = Command::RewriteText { page_index, runs };
    let plan = plan_command_with_fonts(source, &command, credential, fonts)?;
    let effect = plan.effect().clone();
    let edited = plan.commit(source, credential)?;
    Ok(MovedTextRun {
        source: edited,
        original_length: source.len(),
        content_stream: effect.target_stream,
        atom_ordinal: effect.atom_ordinal(),
        original_matrix: effect.original_matrix(),
        declared_region: effect.declared_region,
        edited_glyphs: glyphs_named,
        command,
    })
}

fn last_cluster_of_last_run(
    source: &ByteStore,
    page_index: usize,
    credential: &[u8],
    fonts: crate::Fonts<'_>,
) -> Result<std::ops::Range<usize>, SpikeError> {
    let graph = read_page(source, page_index, credential, fonts)?.graph;
    let last = graph
        .atoms
        .iter()
        .enumerate()
        .rfind(|(_, atom)| matches!(atom.kind, PaintAtomKind::Text(_)))
        .map(|(ordinal, _)| ordinal)
        .ok_or(SpikeError::NoTextRun)?;
    pdf_semantics::SemanticIndex::of(&graph)
        .clusters
        .iter()
        .rfind(|cluster| cluster.atom == last)
        .map(|cluster| cluster.glyphs.clone())
        .ok_or(SpikeError::NoTextRun)
}

struct Intent<'a> {
    page_index: usize,
    selection: &'a TextRunSelection,
    cluster: Option<(std::ops::Range<usize>, ClusterEdit)>,
    dx: f64,
    dy: f64,
}

fn intent_of(command: &Command) -> Intent<'_> {
    match command {
        Command::AddBlankPage { .. }
        | Command::InsertPages { .. }
        | Command::RemovePages { .. }
        | Command::RotatePages { .. }
        | Command::MovePages { .. }
        | Command::SetDocumentInfo { .. } => Intent {
            page_index: command.page_index(),
            selection: &TextRunSelection::Last,
            cluster: None,
            dx: 0.0,
            dy: 0.0,
        },
        Command::MoveTextBlock { page_index, .. }
        | Command::MoveGroup { page_index, .. }
        | Command::RewriteText { page_index, .. }
        | Command::PlaceObject { page_index, .. }
        | Command::RemoveObject { page_index, .. }
        | Command::SetTextSize { page_index, .. }
        | Command::SetTextShape { page_index, .. }
        | Command::RewriteBlock { page_index, .. }
        | Command::RewriteBlockInStyle { page_index, .. }
        | Command::RewriteEmptyBlock { page_index, .. }
        | Command::PlaceNewText { page_index, .. }
        | Command::Stamp { page_index, .. }
        | Command::TextLayer { page_index, .. }
        | Command::PlaceNewImage { page_index, .. }
        | Command::DrawPath { page_index, .. }
        | Command::FillField { page_index, .. }
        | Command::AddField { page_index, .. }
        | Command::RemoveField { page_index, .. }
        | Command::SetFieldBox { page_index, .. }
        | Command::SetFieldBoxes { page_index, .. }
        | Command::CopyFields { page_index, .. }
        | Command::RemoveFields { page_index, .. }
        | Command::SetTabOrder { page_index, .. }
        | Command::ChangeOutline { page_index, .. }
        | Command::ChangeNaming { page_index, .. }
        | Command::AddLink { page_index, .. }
        | Command::AddLinks { page_index, .. }
        | Command::SetLinkProperties { page_index, .. }
        | Command::SetLinkBox { page_index, .. }
        | Command::RemoveLink { page_index, .. }
        | Command::SetLinkBoxes { page_index, .. }
        | Command::RemoveLinks { page_index, .. }
        | Command::SetFieldSettings { page_index, .. }
        | Command::StyleBlock { page_index, .. }
        | Command::ShiftBlock { page_index, .. } => Intent {
            page_index: *page_index,
            selection: &TextRunSelection::Last,
            cluster: None,
            dx: 0.0,
            dy: 0.0,
        },
        Command::MoveTextRun {
            page_index,
            selection,
            dx,
            dy,
        } => Intent {
            page_index: *page_index,
            selection,
            cluster: None,
            dx: *dx,
            dy: *dy,
        },
        Command::MoveTextCluster {
            page_index,
            selection,
            glyphs,
            dx,
            dy,
        } => Intent {
            page_index: *page_index,
            selection,
            cluster: Some((glyphs.clone(), ClusterEdit::Move)),
            dx: *dx,
            dy: *dy,
        },
        Command::DeleteTextClusters {
            page_index,
            selection,
            glyphs,
        } => Intent {
            page_index: *page_index,
            selection,
            cluster: Some((glyphs.clone(), ClusterEdit::Delete)),
            dx: 0.0,
            dy: 0.0,
        },
    }
}

#[derive(Clone, Copy)]
pub struct PlannerPage<'a> {
    pub program: &'a pdf_content::PageProgram,
    pub operations: &'a [Vec<pdf_content::Operation>],
    pub graph: &'a pdf_paint::PaintGraph,
    pub fonts: crate::Fonts<'a>,
    pub restrictions: crate::Restrictions,
    pub credential: &'a [u8],
}

pub fn plan_command(
    source: &ByteStore,
    command: &Command,
    credential: &[u8],
) -> Result<Plan, SpikeError> {
    plan_command_with_fonts(source, command, credential, None)
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "the owned shape `pdf_cli::font_provider` and `Session::with_fonts` both use"
)]
pub fn plan_command_with_fonts(
    source: &ByteStore,
    command: &Command,
    credential: &[u8],
    fonts: Option<std::sync::Arc<dyn pdf_content::FontProvider>>,
) -> Result<Plan, SpikeError> {
    plan_command_under(
        source,
        command,
        credential,
        (fonts.as_ref(), crate::Restrictions::Respect),
    )
}

pub fn plan_command_under(
    source: &ByteStore,
    command: &Command,
    credential: &[u8],
    (fonts, restrictions): (
        Option<&std::sync::Arc<dyn pdf_content::FontProvider>>,
        crate::Restrictions,
    ),
) -> Result<Plan, SpikeError> {
    let reading = read_page(source, command.page_index(), credential, fonts)?;
    plan_command_in(
        source,
        PlannerPage {
            program: &reading.program,
            operations: &reading.operations,
            graph: &reading.graph,
            fonts,
            restrictions,
            credential,
        },
        command,
    )
}

pub fn plan_command_in(
    source: &ByteStore,
    page: PlannerPage<'_>,
    command: &Command,
) -> Result<Plan, SpikeError> {
    if page.restrictions == crate::Restrictions::Respect && !page.program.may_modify_content() {
        return Err(crate::incremental::IncrementalWriteError::PermissionDenied.into());
    }
    let plan = plan_command_in_page(source, page, command)?;
    let mut shared = Vec::new();
    for write in plan.writes() {
        if page
            .program
            .streams
            .iter()
            .any(|stream| stream.reference == write.reference)
            && !page
                .program
                .content_stream_is_exclusive(write.reference)
                .map_err(SpikeError::Page)?
        {
            shared.push(write.reference);
        }
    }
    match shared.as_slice() {
        [] => Ok(plan),
        [stream] => copy_on_write_content(source, page.program, plan, *stream),
        _ => Err(SpikeError::SharedPageContentStream),
    }
}

pub fn lay_out_live(
    source: &ByteStore,
    page: PlannerPage<'_>,
    command: &Command,
) -> Result<crate::LiveBlock, SpikeError> {
    if page.restrictions == crate::Restrictions::Respect && !page.program.may_modify_content() {
        return Err(crate::incremental::IncrementalWriteError::PermissionDenied.into());
    }
    let Command::RewriteBlock {
        page_index,
        rows,
        frame,
        edges,
        breaks,
        frame_declared: _,
        range,
        text,
        paragraph,
    } = command
    else {
        return Err(SpikeError::BlockRewriteUnsupported(
            "only a block rewrite is laid out live",
        ));
    };
    crate::block_rewrite::lay_out_live(
        source,
        (page, *page_index),
        &crate::block_rewrite::BlockEdit {
            rows,
            frame: *frame,
            edges: *edges,
            breaks: breaks.as_ref(),
            range: *range,
            text,
            style: None,
            typed: None,
            empty: None,
            offset: (0.0, 0.0),
            paragraph: *paragraph,
        },
    )
}

fn copy_on_write_content(
    source: &ByteStore,
    program: &pdf_content::PageProgram,
    plan: Plan,
    stream: Reference,
) -> Result<Plan, SpikeError> {
    let refused = || SpikeError::SharedPageContentStream;
    let [write] = plan.writes() else {
        return Err(refused());
    };
    let PlannedBody::ReplacedStream { decoded } = &write.body else {
        return Err(refused());
    };
    if write.reference != stream
        || program
            .streams
            .iter()
            .filter(|listed| listed.reference == stream)
            .count()
            != 1
    {
        return Err(refused());
    }
    let page = resolve_page_object(source, program.page).map_err(|_| refused())?;
    let ObjectKind::Dictionary(entries) = page.value.kind() else {
        return Err(refused());
    };
    let contents = entries
        .iter()
        .find(|entry| entry.key_equals(&page.source, b"/Contents"))
        .map(pdf_syntax::DictionaryEntry::value)
        .ok_or_else(refused)?;
    let named = |value: &Object| matches!(value.kind(), ObjectKind::Reference(reference) if *reference == stream);
    let span = match contents.kind() {
        ObjectKind::Reference(_) if named(contents) => contents.span(),
        ObjectKind::Array(values) => {
            let mut naming = values.iter().filter(|value| named(value));
            match (naming.next(), naming.next()) {
                (Some(value), None) => value.span(),
                _ => return Err(refused()),
            }
        }
        _ => return Err(refused()),
    };
    let start = span
        .start()
        .checked_sub(page.body_offset)
        .ok_or_else(refused)?;
    let end = span
        .end()
        .checked_sub(page.body_offset)
        .ok_or_else(refused)?;
    if end > page.body.len() || start > end {
        return Err(refused());
    }
    let copy = Reference::new(
        crate::block_rewrite::next_object_number(source).map_err(|_| refused())?,
        0,
    );
    let mut page_body = Vec::with_capacity(page.body.len() + 16);
    page_body.extend_from_slice(&page.body[..start]);
    page_body
        .extend_from_slice(format!("{} {} R", copy.object_number(), copy.generation()).as_bytes());
    page_body.extend_from_slice(&page.body[end..]);
    let writes = vec![
        PlannedWrite {
            reference: copy,
            body: PlannedBody::NewStream {
                dictionary: Vec::new(),
                decoded: decoded.clone(),
            },
        },
        PlannedWrite {
            reference: program.page,
            body: PlannedBody::Direct { body: page_body },
        },
    ];
    Ok(plan.with_writes(writes, copy))
}

fn plan_command_in_page(
    source: &ByteStore,
    page: PlannerPage<'_>,
    command: &Command,
) -> Result<Plan, SpikeError> {
    let plan = plan_one_command_in_page(source, page, command)?;
    match holding(command.clusters_after(), plan.correspondence().is_some()) {
        Holding::AsItWas => Ok(plan.with_correspondence(identity_correspondence(page.graph))),
        Holding::AsItIs => Ok(plan),
        Holding::Refuse => Err(SpikeError::ClustersNotNamed(
            "the planner wrote the page's text without saying which cluster became which",
        )),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Holding {
    AsItWas,
    AsItIs,
    Refuse,
}

const fn holding(after: crate::ClustersAfter, said: bool) -> Holding {
    match after {
        _ if said => Holding::AsItIs,
        crate::ClustersAfter::TheSame | crate::ClustersAfter::ThePagesThemselves => {
            Holding::AsItWas
        }
        crate::ClustersAfter::ThePlannerSays => Holding::Refuse,
        crate::ClustersAfter::NotSaid(_) => Holding::AsItIs,
    }
}

#[expect(clippy::too_many_lines, reason = "a dispatch, one arm per command")]
fn plan_one_command_in_page(
    source: &ByteStore,
    page: PlannerPage<'_>,
    command: &Command,
) -> Result<Plan, SpikeError> {
    if let Command::RewriteBlock {
        page_index,
        rows,
        frame,
        edges,
        breaks,
        frame_declared: _,
        range,
        text,
        paragraph,
    } = command
    {
        return crate::block_rewrite::plan_block_rewrite(
            source,
            page,
            *page_index,
            &crate::block_rewrite::BlockEdit {
                rows,
                frame: *frame,
                edges: *edges,
                breaks: breaks.as_ref(),
                range: *range,
                text,
                style: None,
                typed: None,
                empty: None,
                offset: (0.0, 0.0),
                paragraph: *paragraph,
            },
        );
    }
    if let Command::RotatePages {
        pages,
        quarter_turns,
    } = command
    {
        return crate::page_tree::plan_rotate_pages(source, page, (pages, *quarter_turns));
    }
    if let Command::RemovePages { pages } = command {
        return crate::page_tree::plan_remove_pages(source, page, pages);
    }
    if let Command::MovePages { pages, to } = command {
        return crate::page_tree::plan_move_pages(source, page, (pages, *to));
    }
    if let Command::InsertPages {
        beside,
        before,
        document,
        pages,
    } = command
    {
        return crate::import_pages::plan_insert_pages(
            source,
            page,
            &crate::import_pages::Imported {
                beside: *beside,
                before: *before,
                document,
                pages,
            },
        );
    }
    if let Command::SetDocumentInfo { edit } = command {
        return crate::info::plan_set_document_info(source, page, edit);
    }
    if let Command::AddBlankPage {
        beside,
        before,
        size,
    } = command
    {
        return crate::page_tree::plan_blank_page(
            source,
            page,
            &crate::page_tree::BlankPage {
                beside: *beside,
                before: *before,
                size: *size,
            },
        );
    }
    if let Command::AddLink {
        page_index,
        rect,
        target,
        look,
    } = command
    {
        return crate::link::plan_add_link(source, page, *page_index, (*rect, target, *look));
    }
    if let Command::AddLinks { page_index, links } = command {
        return crate::link::plan_add_links(source, page, *page_index, links);
    }
    if let Command::SetLinkProperties {
        page_index,
        link,
        target,
        look,
    } = command
    {
        return crate::link::plan_set_link_properties(
            source,
            page,
            *page_index,
            (*link, target.as_ref(), *look),
        );
    }
    if let Command::SetLinkBox {
        page_index,
        link,
        rect,
    } = command
    {
        return crate::link::plan_set_link_box(source, page, *page_index, (*link, *rect));
    }
    if let Command::SetLinkBoxes { page_index, boxes } = command {
        return crate::link::plan_set_link_boxes(source, page, *page_index, boxes);
    }
    if let Command::RemoveLinks { page_index, links } = command {
        return crate::link::plan_remove_links(source, page, *page_index, links);
    }
    if let Command::RemoveLink { page_index, link } = command {
        return crate::link::plan_remove_link(source, page, *page_index, *link);
    }
    if let Command::ChangeOutline { page_index, change } = command {
        return crate::outline::plan_outline_change(source, page, *page_index, change);
    }
    if let Command::ChangeNaming { page_index, change } = command {
        return crate::destination::plan_naming(source, page, *page_index, change);
    }
    if let Command::SetTabOrder {
        page_index,
        widgets,
        order,
    } = command
    {
        return crate::tab_order::plan_set_tab_order(source, page, *page_index, (widgets, *order));
    }
    if let Command::SetFieldBoxes { page_index, boxes } = command {
        return crate::field_group::plan_set_field_boxes(source, page, *page_index, boxes);
    }
    if let Command::CopyFields { page_index, copies } = command {
        return crate::field_group::plan_copy_fields(source, page, *page_index, copies);
    }
    if let Command::RemoveFields {
        page_index,
        widgets,
    } = command
    {
        return crate::field_group::plan_remove_fields(source, page, *page_index, widgets);
    }
    if let Command::SetFieldBox {
        page_index,
        widget,
        rect,
    } = command
    {
        return crate::field_settings::plan_set_field_box(
            source,
            page,
            *page_index,
            (*widget, *rect),
        );
    }
    if let Command::SetFieldSettings {
        page_index,
        widget,
        settings,
    } = command
    {
        return crate::field_settings::plan_set_field_settings(
            source,
            page,
            *page_index,
            (*widget, settings),
        );
    }
    if let Command::RemoveField { page_index, widget } = command {
        return crate::new_field::plan_remove_field(source, page, *page_index, *widget);
    }
    if let Command::AddField {
        page_index,
        rect,
        kind,
        name,
        options,
    } = command
    {
        return crate::new_field::plan_new_field(
            source,
            page,
            *page_index,
            &crate::new_field::NewField {
                rect: *rect,
                kind: *kind,
                name: name.as_deref(),
                options,
            },
        );
    }
    if let Command::FillField {
        page_index,
        widget,
        value,
    } = command
    {
        return crate::fill_field::plan_fill_field(
            source,
            page,
            *page_index,
            &crate::fill_field::Filled {
                widget: *widget,
                value,
            },
        );
    }
    if let Command::DrawPath {
        page_index,
        steps,
        closed,
        stroke,
        fill,
    } = command
    {
        return crate::new_path::plan_new_path(
            source,
            page,
            *page_index,
            &crate::new_path::NewPath {
                steps,
                closed: *closed,
                stroke: *stroke,
                fill: *fill,
            },
        );
    }
    if let Command::PlaceNewImage {
        page_index,
        placement,
        file,
    } = command
    {
        return crate::new_image::plan_new_image(
            source,
            page,
            *page_index,
            &crate::new_image::NewImage {
                placement: *placement,
                file,
            },
        );
    }
    if let Command::PlaceNewText {
        page_index,
        frame,
        text,
        family,
        size,
        bold,
        italic,
        fill,
        paragraph,
    } = command
    {
        return crate::new_text::plan_new_text(
            source,
            page,
            *page_index,
            &crate::new_text::NewText {
                frame: *frame,
                text,
                family,
                size: *size,
                bold: *bold,
                italic: *italic,
                fill: *fill,
                opacity: 1.0,
                turn: pdf_paint::Matrix::IDENTITY,
                share_from: None,
                paragraph: *paragraph,
            },
        );
    }
    if let Command::Stamp {
        page_index,
        stamp,
        facts,
        share_from,
    } = command
    {
        return crate::stamp::plan_stamp(source, page, *page_index, stamp, (facts, *share_from));
    }
    if let Command::TextLayer {
        page_index,
        layer,
        share_from,
    } = command
    {
        return crate::text_layer::plan_text_layer(source, page, *page_index, layer, *share_from);
    }
    if let Command::RewriteEmptyBlock {
        page_index,
        run,
        frame,
        text,
        style,
        paragraph,
    } = command
    {
        return crate::block_rewrite::plan_block_rewrite(
            source,
            page,
            *page_index,
            &crate::block_rewrite::BlockEdit {
                rows: &[],
                frame: *frame,
                edges: (0, 0),
                breaks: None,
                range: BlockRange::Between {
                    from: (0, 0),
                    to: (0, 0),
                },
                text,
                style: None,
                typed: style.as_ref(),
                empty: Some(run),
                offset: (0.0, 0.0),
                paragraph: *paragraph,
            },
        );
    }
    if let Command::RewriteBlockInStyle {
        page_index,
        rows,
        frame,
        edges,
        breaks,
        frame_declared: _,
        range,
        text,
        style,
        paragraph,
    } = command
    {
        return crate::block_rewrite::plan_block_rewrite(
            source,
            page,
            *page_index,
            &crate::block_rewrite::BlockEdit {
                rows,
                frame: *frame,
                edges: *edges,
                breaks: breaks.as_ref(),
                range: *range,
                text,
                style: None,
                typed: Some(style),
                empty: None,
                offset: (0.0, 0.0),
                paragraph: *paragraph,
            },
        );
    }
    if let Command::StyleBlock {
        page_index,
        rows,
        frame,
        edges,
        breaks,
        range,
        style,
        paragraph,
    } = command
    {
        return crate::block_rewrite::plan_block_rewrite(
            source,
            page,
            *page_index,
            &crate::block_rewrite::BlockEdit {
                rows,
                frame: *frame,
                edges: *edges,
                breaks: breaks.as_ref(),
                range: *range,
                text: "",
                style: Some(style),
                typed: None,
                empty: None,
                offset: (0.0, 0.0),
                paragraph: *paragraph,
            },
        );
    }
    if let Command::ShiftBlock {
        page_index,
        rows,
        frame,
        edges,
        breaks,
        dx,
        dy,
        paragraph,
    } = command
    {
        return crate::block_rewrite::plan_block_rewrite(
            source,
            page,
            *page_index,
            &crate::block_rewrite::BlockEdit {
                rows,
                frame: *frame,
                edges: *edges,
                breaks: breaks.as_ref(),
                range: BlockRange::Between {
                    from: (0, 0),
                    to: (0, 0),
                },
                text: "",
                style: None,
                typed: None,
                empty: None,
                offset: (*dx, *dy),
                paragraph: *paragraph,
            },
        );
    }
    if let Command::RewriteText { page_index, runs } = command {
        return crate::split::rewrite_runs(
            source,
            page.program,
            page.operations,
            page.graph,
            *page_index,
            runs,
            page.fonts,
        );
    }
    if let Command::MoveTextBlock {
        page_index,
        runs,
        dx,
        dy,
    } = command
    {
        return crate::block_move::plan_block_move(
            source,
            page.program,
            page.operations,
            page.graph,
            *page_index,
            runs,
            *dx,
            *dy,
            page.fonts,
        );
    }
    if let Command::MoveGroup {
        page_index,
        runs,
        objects,
        dx,
        dy,
    } = command
    {
        return crate::group_move::plan_group_move(
            (page.program, page.operations, page.graph),
            *page_index,
            (runs, objects),
            (*dx, *dy),
            page.fonts,
        );
    }
    if let Command::SetTextSize {
        page_index,
        runs,
        points,
    } = command
    {
        return crate::size_text::plan_set_text_size(
            page.program,
            page.operations,
            page.graph,
            *page_index,
            runs,
            *points,
            page.fonts,
        );
    }
    if let Command::SetTextShape {
        page_index,
        runs,
        turn,
        slant,
    } = command
    {
        return crate::place_text::plan_set_text_shape(
            page.program,
            page.operations,
            page.graph,
            *page_index,
            runs,
            *turn,
            *slant,
            page.fonts,
        );
    }
    if let Command::RemoveObject { page_index, target } = command {
        return plan_removal_in(page, *page_index, target);
    }
    if let Command::PlaceObject {
        page_index,
        target,
        transform,
        about,
    } = command
    {
        let plan = match target {
            ObjectSelection::Painted(anchor) => crate::place_object::plan_place_object(
                source,
                page.program,
                page.operations,
                page.graph,
                *page_index,
                anchor,
                *transform,
                *about,
                page.fonts,
            ),
            ObjectSelection::Text(runs) => crate::place_text::plan_place_text(
                page.program,
                page.operations,
                page.graph,
                *page_index,
                runs,
                *transform,
                *about,
                page.fonts,
            ),
        }?;
        return Ok(plan.with_correspondence(identity_correspondence(page.graph)));
    }
    let identity =
        matches!(command, Command::MoveTextRun { .. }).then(|| identity_correspondence(page.graph));
    let plan = plan_one_run_in(source, page, command)?;
    Ok(match identity {
        Some(mapping) => plan.with_correspondence(mapping),
        None => plan,
    })
}

fn identity_correspondence(
    graph: &pdf_paint::PaintGraph,
) -> std::collections::BTreeMap<pdf_semantics::ClusterKey, pdf_semantics::ClusterKey> {
    graph
        .atoms
        .iter()
        .enumerate()
        .flat_map(|(atom, paint)| {
            let count = match &paint.kind {
                pdf_paint::PaintAtomKind::Text(text) => text.glyphs.len().max(1),
                _ => 0,
            };
            (0..count).map(move |glyph| {
                let key = pdf_semantics::ClusterKey { atom, glyph };
                (key, key)
            })
        })
        .collect()
}

fn plan_removal_in(
    page: PlannerPage<'_>,
    page_index: usize,
    target: &SourceAnchor,
) -> Result<Plan, SpikeError> {
    let plan = crate::remove_object::plan_remove_object(
        page.program,
        page.operations,
        page.graph,
        page_index,
        target,
        page.fonts,
    )?;
    let gone = plan.effect().moved.first().map(|run| run.atom_ordinal);
    let mapping = gone.map_or_else(
        || identity_correspondence(page.graph),
        |gone| correspondence_without(page.graph, gone),
    );
    Ok(plan.with_correspondence(mapping))
}

fn correspondence_without(
    graph: &pdf_paint::PaintGraph,
    gone: usize,
) -> std::collections::BTreeMap<pdf_semantics::ClusterKey, pdf_semantics::ClusterKey> {
    graph
        .atoms
        .iter()
        .enumerate()
        .filter(|(atom, _)| *atom != gone)
        .flat_map(|(atom, paint)| {
            let count = match &paint.kind {
                pdf_paint::PaintAtomKind::Text(text) => text.glyphs.len().max(1),
                _ => 0,
            };
            let after = if atom > gone { atom - 1 } else { atom };
            (0..count).map(move |glyph| {
                (
                    pdf_semantics::ClusterKey { atom, glyph },
                    pdf_semantics::ClusterKey { atom: after, glyph },
                )
            })
        })
        .collect()
}

#[expect(
    clippy::too_many_lines,
    reason = "one run's preconditions, each with the failure it was written for"
)]
fn plan_one_run_in(
    source: &ByteStore,
    page: PlannerPage<'_>,
    command: &Command,
) -> Result<Plan, SpikeError> {
    let Intent {
        page_index,
        selection,
        cluster,
        dx,
        dy,
    } = intent_of(command);
    let PlannerPage {
        program,
        operations,
        graph,
        fonts,
        restrictions: _,
        credential: _,
    } = page;

    let mut runs = graph
        .atoms
        .iter()
        .enumerate()
        .filter_map(|(ordinal, atom)| match &atom.kind {
            PaintAtomKind::Text(text) => Some((ordinal, atom, text)),
            _ => None,
        });
    let (ordinal, atom, text) = match selection {
        TextRunSelection::Last => runs.next_back().ok_or(SpikeError::NoTextRun)?,
        TextRunSelection::Ordinal(wanted) => runs
            .find(|(ordinal, _, _)| ordinal == wanted)
            .ok_or(SpikeError::NoTextRun)?,
        TextRunSelection::Anchored(anchor) => runs
            .find(|(_, atom, _)| anchor.names(&atom.id))
            .ok_or(SpikeError::AnchorNamesNothing)?,
    };
    let normalization = normalization_for(graph, ordinal);

    if !atom.id.pattern_path.is_empty() {
        return Err(SpikeError::RunNotDirectlyOnPage);
    }
    if let Some(invocation) = atom.id.invocation_path.first() {
        if atom.id.invocation_path.len() != 1 {
            return Err(SpikeError::RunNestedTooDeep);
        }
        let showing = clip_allows_move_of(text, dx, dy)?;
        let content = match &cluster {
            Some((glyphs, edit)) => FormContent::Cluster {
                glyphs,
                edit: *edit,
            },
            None => FormContent::Run,
        };
        return copy_on_write_form(
            source,
            program,
            operations,
            graph,
            &normalization,
            *invocation,
            atom,
            text,
            ordinal,
            page_index,
            content,
            dx,
            dy,
            fonts,
        )
        .map(|plan| plan.with_showing(showing));
    }
    if program
        .streams
        .iter()
        .filter(|stream| stream.reference == atom.id.stream)
        .count()
        != 1
    {
        return Err(SpikeError::SharedPageContentStream);
    }
    let showing = clip_allows_move_of(text, dx, dy)?;
    if let Some((glyphs, edit)) = cluster {
        return crate::split::rewrite_cluster(
            program,
            operations,
            graph,
            &normalization,
            atom,
            text,
            ordinal,
            page_index,
            &glyphs,
            edit,
            dx,
            dy,
            fonts,
        )
        .map(|plan| plan.with_showing(showing));
    }
    rewrite_page_stream(
        program,
        operations,
        graph,
        &normalization,
        atom,
        text,
        ordinal,
        page_index,
        dx,
        dy,
        fonts,
    )
    .map(|plan| plan.with_showing(showing))
}

#[expect(
    clippy::too_many_arguments,
    reason = "a spike's one edit, threaded rather than given a struct it would not outlive"
)]
fn rewrite_page_stream(
    program: &pdf_content::PageProgram,
    operations: &[Vec<pdf_content::Operation>],
    graph: &pdf_paint::PaintGraph,
    normalization: &[(usize, Matrix)],
    atom: &pdf_paint::PaintAtom,
    text: &TextShowPaint,
    ordinal: usize,
    page_index: usize,
    dx: f64,
    dy: f64,
    fonts: crate::Fonts<'_>,
) -> Result<Plan, SpikeError> {
    let stream_index = program
        .streams
        .iter()
        .position(|stream| stream.reference == atom.id.stream)
        .ok_or(SpikeError::NoTextRun)?;
    let stream = &program.streams[stream_index];
    let instruction = operations[stream_index]
        .iter()
        .find(|operation| operation.operator_span() == atom.id.operator_span)
        .ok_or(SpikeError::NoTextRun)?;
    let insert_at = instruction.span().start();

    let mut insertions = vec![(insert_at, move_instruction(text, dx, dy))];
    for (pinned_ordinal, matrix) in normalization {
        let pinned = graph
            .atoms
            .get(*pinned_ordinal)
            .ok_or(SpikeError::NoTextRun)?;
        if pinned.id.stream != atom.id.stream {
            return Err(SpikeError::NormalizationNotNeutral);
        }
        let operation = operations[stream_index]
            .iter()
            .find(|operation| operation.operator_span() == pinned.id.operator_span)
            .ok_or(SpikeError::NoTextRun)?;
        insertions.push((operation.span().start(), absolute_matrix(*matrix)));
    }
    insertions.sort_by_key(|(at, _)| *at);

    let decoded = stream.bytes.as_bytes();
    let mut edited = Vec::with_capacity(decoded.len() + 64 * insertions.len());
    let mut cursor = 0_usize;
    for (at, bytes) in &insertions {
        edited.extend_from_slice(&decoded[cursor..*at]);
        edited.extend_from_slice(bytes);
        cursor = *at;
    }
    edited.extend_from_slice(&decoded[cursor..]);

    prove_move_isolated(program, stream_index, &insertions, graph, ordinal, fonts)?;
    let capability = if normalization.is_empty() {
        Capability::Exact
    } else {
        Capability::Normalized
    };

    Ok(Plan::new(
        capability,
        vec![PlannedWrite {
            reference: stream.reference,
            body: PlannedBody::ReplacedStream { decoded: edited },
        }],
        Effect {
            page_index,
            moved: vec![crate::plan::MovedRun {
                anchor: SourceAnchor::of(&atom.id),
                atom_ordinal: ordinal,
                original_matrix: text.matrices.text.value,
            }],
            target_stream: stream.reference,
            declared_region: declared_region(text, dx, dy),
        },
    ))
}

pub(crate) fn clip_allows_move_of(
    text: &TextShowPaint,
    dx: f64,
    dy: f64,
) -> Result<crate::plan::Showing, SpikeError> {
    let Some(points) = text.outline_points() else {
        return if text.state.clip_paths.is_empty() || text.draws_no_ink() {
            Ok(crate::plan::Showing::Whole)
        } else {
            Err(SpikeError::RunExtentUnknown)
        };
    };
    let shift = user_space_shift(text, dx, dy);
    let moved: Vec<Point> = points
        .iter()
        .map(|point| Point {
            x: point.x + shift[0],
            y: point.y + shift[1],
        })
        .collect();
    let mut showing = crate::plan::Showing::Whole;
    for clip in &text.state.clip_paths {
        showing = showing.and(clip_shows(clip, &moved));
    }
    Ok(showing)
}

pub(crate) fn clip_shows(clip: &pdf_paint::ClipPath, extent: &[Point]) -> crate::plan::Showing {
    if crate::clip_region::ClipCover::of(&clip.path, clip.ctm.value).hides(extent) {
        return crate::plan::Showing::OutOfSight;
    }
    match crate::clip_region::ClipRegion::of(&clip.path, clip.ctm.value) {
        Ok(region) if !region.admits(extent) => crate::plan::Showing::PartlyHidden,
        _ => crate::plan::Showing::Whole,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "a spike's one edit, threaded rather than given a struct it would not outlive"
)]
fn copy_on_write_form(
    source: &ByteStore,
    program: &pdf_content::PageProgram,
    operations: &[Vec<pdf_content::Operation>],
    graph: &pdf_paint::PaintGraph,
    normalization: &[(usize, Matrix)],
    invocation: FormInvocation,
    atom: &pdf_paint::PaintAtom,
    text: &TextShowPaint,
    ordinal: usize,
    page_index: usize,
    content: FormContent<'_>,
    dx: f64,
    dy: f64,
    fonts: crate::Fonts<'_>,
) -> Result<Plan, SpikeError> {
    let scope = crate::form_edit::scope_of(program, operations, invocation)?;
    let form = &scope.form;
    let form_operations = &scope.operations;
    let (edited, capability, region) = match content {
        FormContent::Run => {
            let instruction = form_operations
                .iter()
                .find(|operation| operation.operator_span() == atom.id.operator_span)
                .ok_or(SpikeError::NoTextRun)?;
            let insert_at = instruction.span().start();
            let decoded = form.bytes.as_bytes();
            let mut edited = Vec::with_capacity(decoded.len() + 64);
            edited.extend_from_slice(&decoded[..insert_at]);
            edited.extend_from_slice(&move_instruction(text, dx, dy));
            edited.extend_from_slice(&decoded[insert_at..]);
            crate::form_edit::prove_isolated(&scope, &edited, &[atom.id.operator_span], fonts)?;
            (edited, Capability::Exact, declared_region(text, dx, dy))
        }
        FormContent::Cluster { glyphs, edit } => {
            let (edited, region) = crate::split::cluster_edit_in_form(
                form,
                &scope.resources,
                form_operations,
                graph,
                normalization,
                atom,
                text,
                glyphs,
                edit,
                dx,
                dy,
                fonts,
            )?;
            (edited, Capability::Normalized, region)
        }
    };

    let written = crate::form_edit::writes_for(source, program, &scope, edited)?;

    Ok(Plan::new(
        capability,
        written.writes,
        Effect {
            page_index,
            moved: vec![crate::plan::MovedRun {
                anchor: SourceAnchor::of(&atom.id),
                atom_ordinal: ordinal,
                original_matrix: text.matrices.text.value,
            }],
            target_stream: written.target_stream,
            declared_region: region,
        },
    ))
}

#[derive(Clone, Copy)]
enum FormContent<'a> {
    Run,
    Cluster {
        glyphs: &'a std::ops::Range<usize>,
        edit: ClusterEdit,
    },
}

pub(crate) struct PageObjectBody {
    pub(crate) source: ByteStore,
    pub(crate) value: Object,
    pub(crate) body: Vec<u8>,
    pub(crate) body_offset: usize,
}

pub(crate) fn resolve_page_object(
    source: &ByteStore,
    page: Reference,
) -> Result<PageObjectBody, SpikeError> {
    let chain = pdf_syntax::parse_revision_chain_strict(source, pdf_syntax::XrefLimits::default())
        .map_err(|_| SpikeError::SharedFormResources)?;
    let index = pdf_syntax::RevisionIndex::from_chain(&chain)
        .map_err(|_| SpikeError::SharedFormResources)?;
    let resolved = index
        .resolve_object(source, page, pdf_syntax::ResolveLimits::default())
        .map_err(|_| SpikeError::SharedFormResources)?;
    if resolved.is_compressed() {
        return Err(SpikeError::SharedFormResources);
    }
    let span = resolved.value().span();
    let body = resolved
        .source()
        .resolve(span)
        .map_err(|_| SpikeError::SharedFormResources)?
        .to_vec();
    Ok(PageObjectBody {
        source: resolved.source().clone(),
        value: resolved.value().clone(),
        body,
        body_offset: span.start(),
    })
}

fn prove_move_isolated(
    program: &pdf_content::PageProgram,
    stream_index: usize,
    insertions: &[(usize, Vec<u8>)],
    original: &pdf_paint::PaintGraph,
    ordinal: usize,
    fonts: crate::Fonts<'_>,
) -> Result<(), SpikeError> {
    let rewritten = interpret_with_insertions_of(program, stream_index, insertions, fonts)?;
    if rewritten.atoms.len() != original.atoms.len() {
        return Err(SpikeError::MoveNotIsolated);
    }
    let before = pdf_paint::glyph_placement_signature(original);
    let after = pdf_paint::glyph_placement_signature(&rewritten);
    if before.len() != after.len() {
        return Err(SpikeError::MoveNotIsolated);
    }
    let glyphs_in = |graph: &pdf_paint::PaintGraph, at: usize| -> usize {
        match graph.atoms.get(at).map(|atom| &atom.kind) {
            Some(PaintAtomKind::Text(text)) => text.glyphs.len(),
            _ => 0,
        }
    };
    let start: usize = (0..ordinal).map(|at| glyphs_in(original, at)).sum();
    let moved = start..start + glyphs_in(original, ordinal);
    for (index, (one, other)) in before.iter().zip(&after).enumerate() {
        if !moved.contains(&index) && one != other {
            return Err(SpikeError::MoveNotIsolated);
        }
    }
    Ok(())
}

pub(crate) fn interpret_with_insertions_of(
    program: &pdf_content::PageProgram,
    stream_index: usize,
    insertions: &[(usize, Vec<u8>)],
    fonts: crate::Fonts<'_>,
) -> Result<pdf_paint::PaintGraph, SpikeError> {
    let decoded = program.streams[stream_index].bytes.as_bytes();
    let mut candidate_bytes = Vec::with_capacity(decoded.len() + 64 * insertions.len());
    let mut cursor = 0_usize;
    for (at, bytes) in insertions {
        candidate_bytes.extend_from_slice(&decoded[cursor..*at]);
        candidate_bytes.extend_from_slice(bytes);
        cursor = *at;
    }
    candidate_bytes.extend_from_slice(&decoded[cursor..]);
    interpret_bytes_of(program, stream_index, &candidate_bytes, fonts)
}

pub(crate) fn interpret_bytes_of(
    program: &pdf_content::PageProgram,
    stream_index: usize,
    candidate_bytes: &[u8],
    fonts: crate::Fonts<'_>,
) -> Result<pdf_paint::PaintGraph, SpikeError> {
    let candidate = ByteStore::new(
        SourceId::new(
            program.streams[stream_index]
                .bytes
                .id()
                .get()
                .wrapping_add(1),
        ),
        std::sync::Arc::<[u8]>::from(candidate_bytes.to_vec()),
    );
    let mut sources: Vec<&ByteStore> = program.streams.iter().map(|stream| &stream.bytes).collect();
    sources[stream_index] = &candidate;
    let operations = parse_operation_sequence_strict(&sources, ContentLimits::default())
        .map_err(|_| SpikeError::NormalizationNotNeutral)?;
    let paint_streams: Vec<_> = program
        .streams
        .iter()
        .zip(&sources)
        .zip(&operations)
        .map(|((stream, source), operations)| PaintStream {
            source,
            reference: stream.reference,
            operations,
        })
        .collect();
    pdf_paint::interpret_stream_sequence_with_fonts(
        &paint_streams,
        program.page,
        &[],
        &program.resources,
        PaintLimits::default(),
        fonts.cloned(),
    )
    .map_err(|_| SpikeError::NormalizationNotNeutral)
}

pub(crate) fn normalization_for(
    graph: &pdf_paint::PaintGraph,
    selected: usize,
) -> Vec<(usize, Matrix)> {
    let Some(PaintAtomKind::Text(chosen)) = graph.atoms.get(selected).map(|atom| &atom.kind) else {
        return Vec::new();
    };
    let mut pinned = Vec::new();
    for (ordinal, atom) in graph.atoms.iter().enumerate().skip(selected + 1) {
        let PaintAtomKind::Text(text) = &atom.kind else {
            continue;
        };
        if text.matrices.text.value == text.matrices.line.value {
            break;
        }
        if text.matrices.line.value != chosen.matrices.line.value {
            break;
        }
        pinned.push((ordinal, text.matrices.text.value));
    }
    pinned
}

pub(crate) fn absolute_matrix(matrix: Matrix) -> Vec<u8> {
    format!(
        " {} {} {} {} {} {} Tm ",
        matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f
    )
    .into_bytes()
}

fn move_instruction(text: &TextShowPaint, dx: f64, dy: f64) -> Vec<u8> {
    let matrix = text.matrices.text.value;
    if matrix == text.matrices.line.value {
        return format!(" {dx} {dy} Td ").into_bytes();
    }
    let e = matrix.a.mul_add(dx, matrix.c * dy) + matrix.e;
    let f = matrix.b.mul_add(dx, matrix.d * dy) + matrix.f;
    format!(
        " {} {} {} {} {e} {f} Tm ",
        matrix.a, matrix.b, matrix.c, matrix.d
    )
    .into_bytes()
}

fn declared_region(text: &TextShowPaint, dx: f64, dy: f64) -> Option<[f64; 4]> {
    let shift = user_space_shift(text, dx, dy);
    text.outline_bounds().map(|before| {
        [
            before[0].min(before[0] + shift[0]),
            before[1].min(before[1] + shift[1]),
            before[2].max(before[2] + shift[0]),
            before[3].max(before[3] + shift[1]),
        ]
    })
}

fn user_space_shift(text: &TextShowPaint, dx: f64, dy: f64) -> [f64; 2] {
    let to_user = text.state.ctm.value.multiply(text.matrices.text.value);
    [
        to_user.a.mul_add(dx, to_user.c * dy),
        to_user.b.mul_add(dx, to_user.d * dy),
    ]
}

#[derive(Clone, Debug)]
pub enum SpikeError {
    Page(PageContentError),
    Interpret(InterpretError),
    UnsupportedContentLayout,
    NoTextRun,
    RunNotDirectlyOnPage,
    RunNestedTooDeep,
    FormResourceNotFound,
    SharedFormResources,
    SharedPageContentStream,
    SizeNotUsable,
    SizeAlreadySet,
    SizeNotAsAsked,
    RunSelectsNoFont,
    ClipNotRectangular,
    RunExtentUnknown,
    ProtectedDocument,
    PreviousObjectUnreadable,
    NothingToUndo,
    AnchorNamesNothing,
    NormalizationNotNeutral,
    MoveNotIsolated,
    MoveNotProvable,
    BlockNamesNoRun,
    BlockNotChainAligned,
    BlockRunInsideForm,
    BlockSpansSeveralStreams,
    BlockRunNotInTargetStream,
    BlockOffsetNotRepresentable,
    GlyphRangeOutsideRun,
    GlyphRangeIsNotACluster,
    Selection(pdf_semantics::SelectionError),
    SelectionNamesNoRun,
    SelectionSpansSeveralStreams,
    SelectionNotDirectlyOnPage,
    DeleteNotIsolated,
    RetypeUnsupported(&'static str),
    ClustersNotNamed(&'static str),
    BlockRewriteUnsupported(&'static str),
    FormInvokedTwiceOnThisPage,
    SplitNotNeutral,
    Write(IncrementalWriteError),
    ObjectNamedMoreThanOnce,
    ObjectIsText,
    ObjectIsDrawing,
    ObjectInsideForm,
    ObjectNotInPageContent,
    ObjectInsideTextObject,
    ObjectCtmSingular,
    PlacementNotInvertible,
    ObjectExtentUnknown,
    ObjectLeavesClip,
    ObjectIsCropped,
    ClipIsCurved,
    ClipIsConcave,
}

impl From<crate::clip_region::ClipUnreadable> for SpikeError {
    fn from(why: crate::clip_region::ClipUnreadable) -> Self {
        match why {
            crate::clip_region::ClipUnreadable::Curved => Self::ClipIsCurved,
            crate::clip_region::ClipUnreadable::Concave => Self::ClipIsConcave,
            crate::clip_region::ClipUnreadable::Degenerate => Self::ClipNotRectangular,
        }
    }
}

impl From<IncrementalWriteError> for SpikeError {
    fn from(error: IncrementalWriteError) -> Self {
        match error {
            IncrementalWriteError::ProtectedDocument => Self::ProtectedDocument,
            other => Self::Write(other),
        }
    }
}

impl std::fmt::Display for SpikeError {
    #[expect(
        clippy::too_many_lines,
        reason = "one sentence per refusal, and a refusal that cannot say which rule it was is what this project exists not to have"
    )]
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Page(error) => write!(formatter, "page content: {error}"),
            Self::Interpret(error) => write!(formatter, "paint interpretation: {error}"),
            Self::UnsupportedContentLayout => {
                formatter.write_str("page content is not one rewritable stream")
            }
            Self::NoTextRun => formatter.write_str("page paints no text run"),
            Self::RunNotDirectlyOnPage => {
                formatter.write_str("last text run is painted through a pattern cell")
            }
            Self::RunNestedTooDeep => formatter.write_str(
                "this is painted by a Form inside another Form, and only the outer one can be copied for this page",
            ),
            Self::FormResourceNotFound => formatter
                .write_str("the Do that reached the run names no Form in this page's resources"),
            Self::SharedFormResources => formatter.write_str(
                "the resources naming the Form are not written in this page's own dictionary",
            ),
            Self::SharedPageContentStream => formatter.write_str(
                "the selected page content stream is used more than once and needs copy-on-write",
            ),
            Self::SizeNotUsable => {
                formatter.write_str("that size cannot be written into this run's own space")
            }
            Self::SizeAlreadySet => {
                formatter.write_str("this text already renders at that size")
            }
            Self::SizeNotAsAsked => formatter
                .write_str("the rewritten page does not render at the size that was asked for"),
            Self::RunSelectsNoFont => {
                formatter.write_str("this run shows text with no font selected")
            }
            Self::ClipNotRectangular => {
                formatter.write_str("a clip in effect encloses no area at all")
            }
            Self::ClipIsCurved => formatter.write_str("a clip in effect has a curved edge"),
            Self::ClipIsConcave => formatter.write_str("a clip in effect is a concave shape"),
            Self::RunExtentUnknown => formatter
                .write_str("the run's glyphs resolve to no outline, so its extent is unknown"),
            Self::ProtectedDocument => formatter.write_str(
                "the document is encrypted, and writing into it needs an encryption policy",
            ),
            Self::PreviousObjectUnreadable => formatter
                .write_str("an object this plan writes cannot be read back out of the document"),
            Self::NothingToUndo => {
                formatter.write_str("the plan created every object it wrote, so nothing is undone")
            }
            Self::AnchorNamesNothing => {
                formatter.write_str("no text run is written where the selection anchors")
            }
            Self::NormalizationNotNeutral => formatter
                .write_str("the rewrite that would isolate this run changes what the page paints"),
            Self::MoveNotIsolated => formatter
                .write_str("moving this run would move text after it, which this edit cannot state"),
            Self::BlockNamesNoRun => {
                formatter.write_str("the block names no run this page paints")
            }
            Self::BlockNotChainAligned => formatter.write_str(
                "a run in the block is not at the start of its line, so the block cannot be moved as one",
            ),
            Self::BlockRunInsideForm => {
                formatter.write_str("a run in the block is painted by a Form")
            }
            Self::BlockSpansSeveralStreams => {
                formatter.write_str("the block's runs are written in more than one content stream")
            }
            Self::BlockRunNotInTargetStream => {
                formatter.write_str("a run in the block is outside the stream this plan writes")
            }
            Self::BlockOffsetNotRepresentable => {
                formatter.write_str("the offset cannot be expressed in a run's own text space")
            }
            Self::MoveNotProvable => formatter
                .write_str("this move cannot be checked, so it is refused rather than guessed at"),
            Self::GlyphRangeOutsideRun => {
                formatter.write_str("the glyph range names glyphs this run does not paint")
            }
            Self::GlyphRangeIsNotACluster => formatter.write_str(
                "the glyph range does not cover whole clusters, and a selection may not split one",
            ),
            Self::Selection(error) => write!(formatter, "{error}"),
            Self::SelectionNamesNoRun => formatter
                .write_str("the selection names no unique text run this page paints"),
            Self::SelectionSpansSeveralStreams => formatter.write_str(
                "the selection is written in more than one page content stream",
            ),
            Self::SelectionNotDirectlyOnPage => formatter.write_str(
                "part of the selection is painted through a Form or pattern",
            ),
            Self::DeleteNotIsolated => formatter.write_str(
                "deleting this selection would move or change paint outside it",
            ),
            Self::RetypeUnsupported(reason) => write!(formatter, "cannot type here: {reason}"),
            Self::ClustersNotNamed(reason) => write!(
                formatter,
                "the edit does not say where the page's text went: {reason}"
            ),
            Self::BlockRewriteUnsupported(reason) => {
                write!(formatter, "cannot lay this block out again: {reason}")
            }
            Self::FormInvokedTwiceOnThisPage => formatter.write_str(
                "this page invokes the Form more than once under one name, so only its shared definition could be repointed",
            ),
            Self::SplitNotNeutral => formatter
                .write_str("splitting the run changed what the page paints, so it was refused"),
            Self::ObjectNamedMoreThanOnce => {
                formatter.write_str("this selection names more than one object on the page")
            }
            Self::ObjectIsText => formatter.write_str(
                "this object is text, whose placement is written into its own text matrix",
            ),
            Self::ObjectIsDrawing => formatter
                .write_str("this object is a path, and paths are not yet grouped into drawings"),
            Self::ObjectInsideForm => formatter
                .write_str("this object is painted by a Form, which may be shared with other pages"),
            Self::ObjectNotInPageContent => formatter
                .write_str("this object's operator is not in a content stream this plan writes"),
            Self::ObjectInsideTextObject => formatter.write_str(
                "this object is painted inside a text object, where no transform may be written",
            ),
            Self::ObjectCtmSingular => {
                formatter.write_str("this object's own transform has no area")
            }
            Self::PlacementNotInvertible => {
                formatter.write_str("this placement would leave the object with no area")
            }
            Self::ObjectExtentUnknown => formatter.write_str(
                "this object has no extent, so it cannot be shown to stay inside its clip",
            ),
            Self::ObjectLeavesClip => {
                formatter.write_str("the placement would put part of the object outside its clip")
            }
            Self::ObjectIsCropped => formatter.write_str(
                "a clip crops this object and cannot travel with it, so moving it alone \
                 would change what shows",
            ),
            Self::Write(error) => write!(formatter, "incremental write: {error}"),
        }
    }
}

impl std::error::Error for SpikeError {}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};
    use pdf_content::{PageContentLimits, load_page_program_strict};

    use pdf_content::{ContentLimits, parse_operation_sequence_strict};
    use pdf_paint::{
        Matrix, PaintAtomKind, PaintLimits, PaintStream, Path, PathSegment, Point,
        interpret_stream_sequence_with_resources,
    };
    use pdf_syntax::Reference;

    use super::{SpikeError, move_last_text_run, plan_command};
    use crate::plan::{Capability, Command, TextRunSelection};

    pub(super) fn content_array_fixture(repeat_first: bool) -> ByteStore {
        content_fixture(repeat_first, b"BT /F1 12 Tf 10 20 Td (A) Tj ET")
    }

    fn content_fixture(repeat_first: bool, first: &[u8]) -> ByteStore {
        let second = b"0 0 m 5 5 l S";
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 100 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!(
                "3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents [4 0 R {} 0 R] /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /FirstChar 65 /LastChar 67 /Widths [600 600 600] >> >> >> >>\nendobj\n",
                if repeat_first { 4 } else { 5 }
            )
            .as_bytes(),
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", first.len()).as_bytes(),
        );
        bytes.extend_from_slice(first);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!("5 0 obj\n<< /Length {} >>\nstream\n", second.len()).as_bytes(),
        );
        bytes.extend_from_slice(second);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        let xref = bytes.len();
        bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(b"trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n");
        bytes.extend_from_slice(xref.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        ByteStore::new(SourceId::new(401), Arc::<[u8]>::from(bytes))
    }

    #[test]
    fn a_content_array_rewrites_only_the_stream_that_owns_the_text_atom() {
        let source = content_array_fixture(false);
        let moved = move_last_text_run(&source, 0, 10.0, 0.0, b"").expect("array is patchable");
        assert_eq!(&moved.source.as_bytes()[..source.len()], source.as_bytes());
        let page = load_page_program_strict(&moved.source, 0, PageContentLimits::default())
            .expect("edited array reopens");
        assert_eq!(page.streams.len(), 2);
        assert!(
            page.streams[0]
                .bytes
                .as_bytes()
                .windows(b" 10 0 Td (A) Tj".len())
                .any(|window| window == b" 10 0 Td (A) Tj")
        );
        assert_eq!(page.streams[1].bytes.as_bytes(), b"0 0 m 5 5 l S");
    }

    #[test]
    fn an_anchor_names_the_same_run_that_an_ordinal_only_happens_to() {
        use crate::Document;
        use crate::plan::{Command, TextRunSelection};

        let source = content_fixture(false, b"BT /F1 12 Tf 10 20 Td (A) Tj (B) Tj (C) Tj ET");
        let document = Document::open_strict(source.clone(), pdf_syntax::XrefLimits::default())
            .expect("fixture opens");
        let ask = |selection: TextRunSelection| {
            document
                .begin_transaction()
                .plan(
                    &Command::MoveTextRun {
                        page_index: 0,
                        selection,
                        dx: 10.0,
                        dy: 0.0,
                    },
                    b"",
                )
                .map(|plan| plan.effect().clone())
        };

        let asked = ask(TextRunSelection::Last).expect("the last run is movable");
        let anchor = asked.anchor().clone();
        let replayed =
            ask(TextRunSelection::Anchored(anchor.clone())).expect("the anchor still names a run");
        assert_eq!(replayed, asked);

        let mut offsets = Vec::new();
        for ordinal in 0..3 {
            let effect = ask(TextRunSelection::Ordinal(ordinal)).expect("run is movable");
            assert_eq!(effect.atom_ordinal(), ordinal);
            let replayed = ask(TextRunSelection::Anchored(effect.anchor().clone()))
                .expect("the anchor names the run it came from");
            assert_eq!(replayed.atom_ordinal(), ordinal);
            offsets.push(effect.anchor().operator_offset);
        }
        offsets.sort_unstable();
        offsets.dedup();
        assert_eq!(offsets.len(), 3, "three runs, three places in the source");

        assert_eq!(anchor.operator_offset, offsets[2]);
        assert!(anchor.invocation_path.is_empty());

        let nowhere = crate::plan::SourceAnchor {
            operator_offset: anchor.operator_offset + 1,
            ..anchor
        };
        assert!(matches!(
            ask(TextRunSelection::Anchored(nowhere)),
            Err(SpikeError::AnchorNamesNothing)
        ));
    }

    #[test]
    fn an_anchor_distinguishes_two_uses_of_one_shared_form() {
        use crate::plan::SourceAnchor;

        let source = content_fixture(false, b"/Fm0 Do /Fm0 Do");
        let page = load_page_program_strict(&source, 0, PageContentLimits::default());
        drop(page);

        let source = form_text_fixture();
        let page = load_page_program_strict(&source, 0, PageContentLimits::default())
            .expect("form fixture opens");
        let operations =
            parse_operation_sequence_strict(&[&page.streams[0].bytes], ContentLimits::default())
                .expect("operations");
        let graph = interpret_stream_sequence_with_resources(
            &[PaintStream {
                source: &page.streams[0].bytes,
                reference: page.streams[0].reference,
                operations: &operations[0],
            }],
            page.page,
            &[],
            &page.resources,
            PaintLimits::default(),
        )
        .expect("form fixture interprets");
        let atom = graph
            .atoms
            .iter()
            .find(|atom| matches!(atom.kind, PaintAtomKind::Text(_)))
            .expect("the Form paints text");
        let anchor = SourceAnchor::of(&atom.id);
        assert_eq!(anchor.stream, Reference::new(5, 0));
        assert_eq!(anchor.invocation_path.len(), 1);
        assert_eq!(anchor.invocation_path[0].0, Reference::new(5, 0));
        assert!(anchor.names(&atom.id));
    }

    #[test]
    fn moving_a_run_that_is_not_last_pins_the_rest_of_its_line_first() {
        use crate::Document;
        use crate::plan::{Capability, Command, PlannedBody, TextRunSelection};

        let source = content_fixture(false, b"BT /F1 12 Tf 10 20 Td (A) Tj (B) Tj (C) Tj ET");
        let document = Document::open_strict(source.clone(), pdf_syntax::XrefLimits::default())
            .expect("fixture opens");
        let plan = document
            .begin_transaction()
            .plan(
                &Command::MoveTextRun {
                    page_index: 0,
                    selection: TextRunSelection::Ordinal(0),
                    dx: 10.0,
                    dy: 0.0,
                },
                b"",
            )
            .expect("the first run is movable once the line is pinned");

        assert_eq!(plan.capability(), Capability::Normalized);
        let PlannedBody::ReplacedStream { decoded } = &plan.writes()[0].body else {
            panic!("expected a replaced stream")
        };
        let text = String::from_utf8_lossy(decoded).into_owned();
        assert!(text.contains(" 10 0 Td (A) Tj"), "{text}");
        assert!(text.contains("1 0 0 1 17.2 20 Tm (B) Tj"), "{text}");
        assert!(text.contains("1 0 0 1 24.4 20 Tm (C) Tj"), "{text}");

        let edited = plan.commit(&source, b"").expect("the plan commits");
        let matrices = |bytes: &ByteStore| {
            let page = load_page_program_strict(bytes, 0, PageContentLimits::default())
                .expect("page reopens");
            let operations = parse_operation_sequence_strict(
                &[&page.streams[0].bytes],
                ContentLimits::default(),
            )
            .expect("operations");
            let graph = interpret_stream_sequence_with_resources(
                &[PaintStream {
                    source: &page.streams[0].bytes,
                    reference: page.streams[0].reference,
                    operations: &operations[0],
                }],
                page.page,
                &[],
                &page.resources,
                PaintLimits::default(),
            )
            .expect("page interprets");
            graph
                .atoms
                .iter()
                .filter_map(|atom| match &atom.kind {
                    PaintAtomKind::Text(text) => Some(text.matrices.text.value.e),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let before = matrices(&source);
        let after = matrices(&edited);
        assert_eq!(before.len(), 3);
        assert!((after[0] - (before[0] + 10.0)).abs() < 1e-9, "{after:?}");
        assert!((after[1] - before[1]).abs() < 1e-9, "{after:?}");
        assert!((after[2] - before[2]).abs() < 1e-9, "{after:?}");
    }

    #[test]
    fn a_run_already_last_on_its_line_needs_no_normalization() {
        use crate::Document;
        use crate::plan::{Capability, Command, TextRunSelection};

        let source = content_fixture(false, b"BT /F1 12 Tf 10 20 Td (A) Tj (B) Tj ET");
        let document = Document::open_strict(source, pdf_syntax::XrefLimits::default())
            .expect("fixture opens");
        let plan = document
            .begin_transaction()
            .plan(
                &Command::MoveTextRun {
                    page_index: 0,
                    selection: TextRunSelection::Ordinal(1),
                    dx: 10.0,
                    dy: 0.0,
                },
                b"",
            )
            .expect("the last run is movable");
        assert_eq!(plan.capability(), Capability::Exact);
    }

    #[test]
    fn plans_committed_together_are_one_undo() {
        use crate::Document;
        use crate::history::History;
        use crate::plan::{Command, TextRunSelection};

        let source = content_array_fixture(false);
        let text = |bytes: &ByteStore| {
            let page = load_page_program_strict(bytes, 0, PageContentLimits::default())
                .expect("page reopens");
            String::from_utf8_lossy(page.streams[0].bytes.as_bytes()).into_owned()
        };
        let at_rest = text(&source);

        let mut history = History::new(source.clone(), b"");
        let mut plans = Vec::new();
        let mut standing = source.clone();
        for _ in 0..2 {
            let document =
                Document::open_strict(standing.clone(), pdf_syntax::XrefLimits::default())
                    .expect("fixture opens");
            let plan = document
                .begin_transaction()
                .plan(
                    &Command::MoveTextRun {
                        page_index: 0,
                        selection: TextRunSelection::Last,
                        dx: 10.0,
                        dy: 0.0,
                    },
                    b"",
                )
                .expect("the run is movable");
            standing = plan.commit(&standing, b"").expect("the plan commits");
            plans.push(plan);
        }

        history
            .apply_together(plans)
            .expect("both plans commit as one step");
        let moved = text(history.source());
        assert_ne!(moved, at_rest);
        assert_eq!(history.undo_depth(), 1);
        assert!(history.undo().expect("undo commits"));
        assert_eq!(text(history.source()), at_rest);
        assert!(!history.can_undo());
        assert!(history.redo().expect("redo commits"));
        assert_eq!(text(history.source()), moved);

        let mut empty = History::new(source, b"");
        assert!(empty.apply_together(Vec::new()).is_err());
        assert_eq!(empty.undo_depth(), 0);
    }

    #[test]
    fn a_history_walks_undo_and_redo_over_the_same_committed_command() {
        use crate::Document;
        use crate::history::History;
        use crate::plan::{Command, TextRunSelection};

        let source = content_array_fixture(false);
        let document = Document::open_strict(source.clone(), pdf_syntax::XrefLimits::default())
            .expect("fixture opens");
        let command = Command::MoveTextRun {
            page_index: 0,
            selection: TextRunSelection::Last,
            dx: 10.0,
            dy: 0.0,
        };
        let plan = document
            .begin_transaction()
            .plan(&command, b"")
            .expect("the run is movable");

        let text = |bytes: &ByteStore| {
            let page = load_page_program_strict(bytes, 0, PageContentLimits::default())
                .expect("page reopens");
            String::from_utf8_lossy(page.streams[0].bytes.as_bytes()).into_owned()
        };
        let at_rest = text(&source);

        let mut history = History::new(source.clone(), b"");
        assert!(!history.can_undo() && !history.can_redo());

        history.apply(plan).expect("the plan applies");
        let moved = text(history.source());
        assert_ne!(moved, at_rest);
        assert!(history.can_undo() && !history.can_redo());
        assert_eq!(history.undo_depth(), 1);

        for _ in 0..1_100 {
            assert!(history.undo().expect("undo commits"));
            assert_eq!(text(history.source()), at_rest);
            assert!(!history.can_undo() && history.can_redo());

            assert!(history.redo().expect("redo commits"));
            assert_eq!(text(history.source()), moved);
            assert!(history.can_undo() && !history.can_redo());
        }

        assert_eq!(
            pdf_syntax::parse_revision_chain_strict(
                history.source(),
                pdf_syntax::XrefLimits::default()
            )
            .unwrap()
            .revisions()
            .len(),
            2
        );
        assert!(history.source().len() > source.len());
        assert_eq!(
            &history.source().as_bytes()[..source.len()],
            source.as_bytes()
        );

        assert!(history.undo().expect("undo commits"));
        assert!(!history.undo().expect("nothing left to undo"));
        assert!(history.redo().expect("redo commits"));
        assert!(!history.redo().expect("nothing left to redo"));
    }

    #[test]
    fn content_permissions_are_checked_before_planning_and_at_the_writer() {
        use crate::incremental::{
            IncrementalWriteError, ProtectionPolicy, StreamReplacement, append_stream_replacement,
        };
        use crate::plan::{Command, TextRunSelection};
        let source = ByteStore::new(
            pdf_bytes::SourceId::new(0),
            &include_bytes!("../tests/data/restricted-r3.pdf")[..],
        );
        let command = Command::MoveTextRun {
            page_index: 0,
            selection: TextRunSelection::Last,
            dx: 1.0,
            dy: 0.0,
        };
        assert!(matches!(
            super::plan_command(&source, &command, b"view"),
            Err(super::SpikeError::Write(
                IncrementalWriteError::PermissionDenied
            ))
        ));
        let replacement = StreamReplacement {
            reference: pdf_syntax::Reference::new(4, 0),
            decoded: b"BT /F1 12 Tf 21 50 Td (Hello) Tj ET",
        };
        assert!(matches!(
            append_stream_replacement(
                &source,
                replacement,
                ProtectionPolicy::Preserve {
                    credential: b"view",
                    restrictions: crate::Restrictions::Respect,
                }
            ),
            Err(IncrementalWriteError::PermissionDenied)
        ));
        let plan = super::plan_command(&source, &command, b"master")
            .expect("owner is the positive control");
        let mut history = crate::History::new(source.clone(), b"master");
        history.apply(plan).unwrap();
        for _ in 0..3 {
            history.undo().unwrap();
            history.redo().unwrap();
        }
        let reopened = pdf_content::load_page_program_with_password(
            history.source(),
            0,
            PageContentLimits::default(),
            b"view",
        );
        assert!(
            reopened.is_ok(),
            "encrypted objects survive session compaction: {reopened:?}"
        );
        assert!(history.source().as_bytes().starts_with(source.as_bytes()));
    }

    #[test]
    fn restrictions_set_aside_edit_the_document_and_keep_its_protection() {
        use crate::incremental::{ProtectionPolicy, StreamReplacement, append_stream_replacement};
        use crate::plan::{Command, TextRunSelection};
        let source = ByteStore::new(
            pdf_bytes::SourceId::new(0),
            &include_bytes!("../tests/data/restricted-r3.pdf")[..],
        );
        let command = Command::MoveTextRun {
            page_index: 0,
            selection: TextRunSelection::Last,
            dx: 1.0,
            dy: 0.0,
        };
        assert!(
            append_stream_replacement(
                &source,
                StreamReplacement {
                    reference: pdf_syntax::Reference::new(4, 0),
                    decoded: b"BT /F1 12 Tf 21 50 Td (Hello) Tj ET",
                },
                ProtectionPolicy::Preserve {
                    credential: b"view",
                    restrictions: crate::Restrictions::SetAside,
                }
            )
            .is_ok()
        );
        let reading = super::read_page(&source, 0, b"view", None).unwrap();
        let page = |restrictions| super::PlannerPage {
            program: &reading.program,
            operations: &reading.operations,
            graph: &reading.graph,
            fonts: None,
            restrictions,
            credential: b"view",
        };
        assert!(matches!(
            super::plan_command_in(&source, page(crate::Restrictions::Respect), &command),
            Err(super::SpikeError::Write(
                crate::incremental::IncrementalWriteError::PermissionDenied
            ))
        ));
        let plan = super::plan_command_in(&source, page(crate::Restrictions::SetAside), &command)
            .expect("set aside, the edit is planned");

        let mut respecting = crate::History::new(source.clone(), b"view");
        assert!(respecting.apply(plan.clone()).is_err());
        let mut history = crate::History::new(source.clone(), b"view");
        history.set_aside_restrictions();
        history.apply(plan).unwrap();
        history.undo().unwrap();
        history.redo().unwrap();

        let reopened = pdf_content::load_page_program_with_password(
            history.source(),
            0,
            PageContentLimits::default(),
            b"view",
        )
        .expect("still opens with the same password");
        assert!(
            !reopened.may_modify_content(),
            "the author's permissions are kept in the file"
        );
        assert_ne!(history.source().as_bytes(), source.as_bytes());
    }

    #[test]
    fn thousands_of_fresh_edits_keep_one_session_update() {
        let source = content_fixture(false, b"BT /F1 12 Tf 10 20 Td (A) Tj ET");
        let mut history = crate::History::new(source.clone(), b"");
        for step in 0..1_100 {
            let command = crate::Command::MoveTextRun {
                page_index: 0,
                selection: crate::TextRunSelection::Last,
                dx: if step % 2 == 0 { 1.0 } else { -1.0 },
                dy: 0.0,
            };
            let plan = super::plan_command(history.source(), &command, b"").unwrap();
            history.apply(plan).unwrap();
        }
        assert_eq!(history.undo_depth(), 1_100);
        assert_eq!(
            pdf_syntax::parse_revision_chain_strict(
                history.source(),
                pdf_syntax::XrefLimits::default()
            )
            .unwrap()
            .revisions()
            .len(),
            2
        );
        assert!(history.source().as_bytes().starts_with(source.as_bytes()));
    }

    #[test]
    fn applying_a_command_after_undoing_one_discards_the_undone_branch() {
        use crate::Document;
        use crate::history::History;
        use crate::plan::{Command, TextRunSelection};

        let source = content_array_fixture(false);
        let document = Document::open_strict(source.clone(), pdf_syntax::XrefLimits::default())
            .expect("fixture opens");
        let command = Command::MoveTextRun {
            page_index: 0,
            selection: TextRunSelection::Last,
            dx: 10.0,
            dy: 0.0,
        };
        let plan = document
            .begin_transaction()
            .plan(&command, b"")
            .expect("the run is movable");

        let mut history = History::new(source.clone(), b"");
        history.apply(plan.clone()).expect("the plan applies");
        assert!(history.undo().expect("undo commits"));
        assert!(history.can_redo());

        history.apply(plan).expect("a second command applies");
        assert!(!history.can_redo());
        assert_eq!(history.undo_depth(), 1);
    }

    #[test]
    fn undoing_an_edit_restores_what_the_page_paints() {
        use crate::Document;
        use crate::plan::{Command, TextRunSelection};

        for source in [content_array_fixture(false), form_text_fixture()] {
            let document = Document::open_strict(source.clone(), pdf_syntax::XrefLimits::default())
                .expect("fixture opens");
            let command = Command::MoveTextRun {
                page_index: 0,
                selection: TextRunSelection::Last,
                dx: 10.0,
                dy: 0.0,
            };
            let plan = document
                .begin_transaction()
                .plan(&command, b"")
                .expect("the run is movable");
            let inverse = plan
                .inverse(&source, b"")
                .expect("the plan can be inverted");

            let edited = plan.commit(&source, b"").expect("the plan commits");
            let undone = inverse.commit(&edited, b"").expect("the inverse commits");

            assert_eq!(&edited.as_bytes()[..source.len()], source.as_bytes());
            assert_eq!(&undone.as_bytes()[..edited.len()], edited.as_bytes());
            assert!(undone.len() > edited.len());

            let text_of = |bytes: &ByteStore| {
                let page = load_page_program_strict(bytes, 0, PageContentLimits::default())
                    .expect("page reopens");
                page.streams
                    .iter()
                    .map(|stream| String::from_utf8_lossy(stream.bytes.as_bytes()).into_owned())
                    .collect::<Vec<_>>()
                    .join("|")
            };
            let form_of = |bytes: &ByteStore| {
                let page = load_page_program_strict(bytes, 0, PageContentLimits::default())
                    .expect("page reopens");
                page.resources.xobject(b"/Fm0").and_then(|entry| {
                    entry
                        .form()
                        .map(|form| String::from_utf8_lossy(form.bytes.as_bytes()).into_owned())
                })
            };
            let before = (text_of(&source), form_of(&source));
            let during = (text_of(&edited), form_of(&edited));
            let after = (text_of(&undone), form_of(&undone));
            assert_ne!(
                before, during,
                "the edit must actually have changed something"
            );
            assert_eq!(before, after, "undo must put back what the page paints");
        }
    }

    #[test]
    fn planning_says_what_would_happen_and_changes_nothing() {
        use crate::plan::{Capability, Command, PlannedBody, TextRunSelection};
        use crate::{Document, plan::Plan};

        let source = content_array_fixture(false);
        let document = Document::open_strict(source.clone(), pdf_syntax::XrefLimits::default())
            .expect("fixture opens");
        let command = Command::MoveTextRun {
            page_index: 0,
            selection: TextRunSelection::Last,
            dx: 10.0,
            dy: 0.0,
        };

        let transaction = document.begin_transaction();
        let plan = transaction.plan(&command, b"").expect("the run is movable");
        assert_eq!(plan.capability(), Capability::Exact);
        assert_eq!(plan.effect().atom_ordinal(), 0);
        assert_eq!(plan.effect().target_stream, Reference::new(4, 0));
        assert!(plan.effect().declared_region.is_none(), "no embedded font");
        assert_eq!(plan.writes().len(), 1);
        let PlannedBody::ReplacedStream { decoded } = &plan.writes()[0].body else {
            panic!("expected a replaced stream")
        };
        assert!(
            String::from_utf8_lossy(decoded).contains(" 10 0 Td (A) Tj"),
            "{}",
            String::from_utf8_lossy(decoded)
        );

        assert!(transaction.is_empty());
        let unchanged = transaction
            .commit(b"")
            .expect("an unused transaction commits nothing");
        assert!(!unchanged.revision_created());
        assert_eq!(unchanged.source().as_bytes(), source.as_bytes());

        let again: Plan = document
            .begin_transaction()
            .plan(&command, b"")
            .expect("the run is still movable");
        assert_eq!(again, plan);

        let mut transaction = document.begin_transaction();
        transaction.push(plan);
        assert!(!transaction.is_empty());
        let outcome = transaction.commit(b"").expect("the plan commits");
        assert!(outcome.revision_created());
        assert_eq!(
            &outcome.source().as_bytes()[..source.len()],
            source.as_bytes()
        );
    }

    #[test]
    fn a_run_that_is_not_first_on_its_line_is_placed_with_tm_not_td() {
        let source = content_fixture(false, b"BT /F1 12 Tf 10 20 Td (A) Tj (B) Tj ET");
        let moved = move_last_text_run(&source, 0, 10.0, 0.0, b"").expect("mid-line run");
        let page = load_page_program_strict(&moved.source, 0, PageContentLimits::default())
            .expect("edited page reopens");
        let edited = page.streams[0].bytes.as_bytes();
        let text = String::from_utf8_lossy(edited);
        assert!(text.contains(" Tm "), "expected a Tm, got {text}");
        assert!(text.contains("10 20 Td (A) Tj"), "{text}");
        assert!(text.contains("1 0 0 1 27.2 20 Tm (B) Tj"), "{text}");
    }

    #[test]
    fn a_run_that_is_first_on_its_line_keeps_the_smaller_td_edit() {
        let source = content_array_fixture(false);
        let moved = move_last_text_run(&source, 0, 10.0, 0.0, b"").expect("first on line");
        let page = load_page_program_strict(&moved.source, 0, PageContentLimits::default())
            .expect("edited page reopens");
        let text = String::from_utf8_lossy(page.streams[0].bytes.as_bytes()).into_owned();
        assert!(text.contains(" 10 0 Td (A) Tj"), "{text}");
        assert!(!text.contains(" Tm "), "{text}");
    }

    const TINY_CFF: &str = "0100040100010101055465737400010101131d00000030111d0000004c0f1d0000\
        0051100001010106616c706861000000030101020c160e8b8b158c8b058b8c050e8b8b158c8b058b8c050e00002201\
        87000141";

    fn hex(text: &str) -> Vec<u8> {
        let digits: Vec<u8> = text.bytes().filter(u8::is_ascii_hexdigit).collect();
        digits
            .chunks_exact(2)
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).expect("hex digits"), 16)
                    .expect("hex byte")
            })
            .collect()
    }

    fn form_text_fixture() -> ByteStore {
        let page_content = b"/Fm0 Do";
        let form_content = b"BT /F1 12 Tf 5 5 Td (A) Tj ET";
        let program = hex(TINY_CFF);
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 100 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Fm0 5 0 R >> >> >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", page_content.len()).as_bytes(),
        );
        bytes.extend_from_slice(page_content);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!(
                "5 0 obj\n<< /Type /XObject /Subtype /Form /BBox [0 0 100 100] /Intent /View /Resources << /Font << /F1 6 0 R >> >> /Length {} >>\nstream\n",
                form_content.len()
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(form_content);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"6 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Test /FirstChar 65 /LastChar 65 /Widths [600] /FontDescriptor 7 0 R >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"7 0 obj\n<< /Type /FontDescriptor /FontName /Test /Flags 4 /FontFile3 8 0 R >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!(
                "8 0 obj\n<< /Subtype /Type1C /Length {} >>\nstream\n",
                program.len()
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(&program);
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
        ByteStore::new(SourceId::new(88), Arc::<[u8]>::from(bytes))
    }

    #[test]
    fn a_run_inside_a_form_used_once_is_moved_in_the_form_itself() {
        let source = form_text_fixture();
        let moved = move_last_text_run(&source, 0, 10.0, 0.0, b"").expect("form run is editable");
        assert_eq!(&moved.source.as_bytes()[..source.len()], source.as_bytes());

        let page = load_page_program_strict(&moved.source, 0, PageContentLimits::default())
            .expect("edited document reopens");
        assert_eq!(page.streams[0].bytes.as_bytes(), b"/Fm0 Do");
        let form = page
            .resources
            .xobject(b"/Fm0")
            .and_then(pdf_content::ResourceEntry::form)
            .expect("the page still names a Form");
        assert_eq!(form.reference, Reference::new(5, 0));
        assert_eq!(form.reference, moved.content_stream);
        let edited = String::from_utf8_lossy(form.bytes.as_bytes()).into_owned();
        assert!(edited.contains("10 0 Td (A) Tj"), "{edited}");
        let dictionary = String::from_utf8_lossy(
            form.source
                .resolve(form.dictionary.span())
                .expect("the Form's dictionary"),
        )
        .into_owned();
        assert!(dictionary.contains("/BBox [0 0 100 100]"), "{dictionary}");
        assert!(dictionary.contains("/Intent /View"), "{dictionary}");
    }

    pub(crate) fn shared_form_fixture() -> ByteStore {
        shared_form_document(b"/Fm0 Do")
    }

    pub(crate) fn form_document(
        first_content: &[u8],
        form_content: &[u8],
        shared: bool,
    ) -> ByteStore {
        form_document_in(first_content, form_content, shared)
    }

    fn shared_form_document(first_content: &[u8]) -> ByteStore {
        form_document_in(first_content, b"BT /F1 12 Tf 5 5 Td (AAA) Tj ET", true)
    }

    fn form_document_in(first_content: &[u8], form_content: &[u8], shared: bool) -> ByteStore {
        let second_content: &[u8] = if shared {
            b"1 0 0 1 20 20 cm /Fm0 Do"
        } else {
            b""
        };
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
            b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 100 100] /Kids [3 0 R 9 0 R] /Count 2 >>\nendobj\n",
        );
        object(
            &mut bytes,
            &mut offsets,
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Fm0 5 0 R >> >> >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", first_content.len()).as_bytes(),
        );
        bytes.extend_from_slice(first_content);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!(
                "5 0 obj\n<< /Type /XObject /Subtype /Form /BBox [0 0 100 100] /Resources << /Font << /F1 6 0 R >> >> /Length {} >>\nstream\n",
                form_content.len()
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(form_content);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        object(
            &mut bytes,
            &mut offsets,
            b"6 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Test /FirstChar 65 /LastChar 65 /Widths [600] /FontDescriptor 7 0 R >>\nendobj\n",
        );
        object(
            &mut bytes,
            &mut offsets,
            b"7 0 obj\n<< /Type /FontDescriptor /FontName /Test /Flags 4 /FontFile3 8 0 R >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!(
                "8 0 obj\n<< /Subtype /Type1C /Length {} >>\nstream\n",
                program.len()
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(&program);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        object(
            &mut bytes,
            &mut offsets,
            if shared {
                &b"9 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 10 0 R /Resources << /XObject << /Fm0 5 0 R >> >> >>\nendobj\n"[..]
            } else {
                &b"9 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 10 0 R /Resources << >> >>\nendobj\n"[..]
            },
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!("10 0 obj\n<< /Length {} >>\nstream\n", second_content.len()).as_bytes(),
        );
        bytes.extend_from_slice(second_content);
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
        ByteStore::new(SourceId::new(89), Arc::<[u8]>::from(bytes))
    }

    pub(crate) fn nested_form_fixture() -> ByteStore {
        let program = hex(TINY_CFF);
        let outer = b"/Fm1 Do";
        let inner = b"BT /F1 12 Tf 5 5 Td (AAA) Tj ET";
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
            b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 100 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
        );
        object(
            &mut bytes,
            &mut offsets,
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Fm0 5 0 R >> >> >>\nendobj\n",
        );
        let page_content = b"/Fm0 Do";
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", page_content.len()).as_bytes(),
        );
        bytes.extend_from_slice(page_content);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!(
                "5 0 obj\n<< /Type /XObject /Subtype /Form /BBox [0 0 100 100] /Resources << /XObject << /Fm1 9 0 R >> >> /Length {} >>\nstream\n",
                outer.len()
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(outer);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        object(
            &mut bytes,
            &mut offsets,
            b"6 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Test /FirstChar 65 /LastChar 65 /Widths [600] /FontDescriptor 7 0 R >>\nendobj\n",
        );
        object(
            &mut bytes,
            &mut offsets,
            b"7 0 obj\n<< /Type /FontDescriptor /FontName /Test /Flags 4 /FontFile3 8 0 R >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!(
                "8 0 obj\n<< /Subtype /Type1C /Length {} >>\nstream\n",
                program.len()
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(&program);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!(
                "9 0 obj\n<< /Type /XObject /Subtype /Form /BBox [0 0 100 100] /Resources << /Font << /F1 6 0 R >> >> /Length {} >>\nstream\n",
                inner.len()
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(inner);
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
        ByteStore::new(SourceId::new(91), Arc::<[u8]>::from(bytes))
    }

    pub(crate) fn page_glyphs(source: &ByteStore, page_index: usize) -> Vec<String> {
        let page = load_page_program_strict(source, page_index, PageContentLimits::default())
            .expect("the page opens");
        let sources: Vec<&ByteStore> = page.streams.iter().map(|stream| &stream.bytes).collect();
        let operations = parse_operation_sequence_strict(&sources, ContentLimits::default())
            .expect("the page parses");
        let streams: Vec<_> = page
            .streams
            .iter()
            .zip(&operations)
            .map(|(stream, operations)| PaintStream {
                source: &stream.bytes,
                reference: stream.reference,
                operations,
            })
            .collect();
        let graph = interpret_stream_sequence_with_resources(
            &streams,
            page.page,
            &[],
            &page.resources,
            PaintLimits::default(),
        )
        .expect("the page interprets");
        pdf_paint::glyph_placement_signature(&graph)
    }

    #[test]
    fn a_cluster_inside_a_form_is_deleted_by_copying_the_form_not_editing_it() {
        let source = shared_form_fixture();
        let before_first = page_glyphs(&source, 0);
        let before_second = page_glyphs(&source, 1);
        assert_eq!(before_first.len(), 3, "the Form paints three glyphs");

        let plan = plan_command(
            &source,
            &Command::DeleteTextClusters {
                page_index: 0,
                selection: TextRunSelection::Last,
                glyphs: 1..2,
            },
            b"",
        )
        .expect("a cluster inside a Form is deletable");
        assert_eq!(plan.capability(), Capability::Normalized);
        let edited = plan.commit(&source, b"").expect("the plan commits");
        assert_eq!(&edited.as_bytes()[..source.len()], source.as_bytes());

        let after_first = page_glyphs(&edited, 0);
        assert_eq!(
            after_first,
            vec![before_first[0].clone(), before_first[2].clone()]
        );

        assert_eq!(page_glyphs(&edited, 1), before_second);
        let page = load_page_program_strict(&edited, 1, PageContentLimits::default())
            .expect("the second page opens");
        let form = page
            .resources
            .xobject(b"/Fm0")
            .and_then(pdf_content::ResourceEntry::form)
            .expect("the second page still names a Form");
        assert_eq!(
            form.reference,
            Reference::new(5, 0),
            "the untouched page still names the shared definition"
        );
        assert_eq!(
            form.bytes.as_bytes(),
            b"BT /F1 12 Tf 5 5 Td (AAA) Tj ET",
            "the shared definition keeps its bytes exactly"
        );

        let page = load_page_program_strict(&edited, 0, PageContentLimits::default())
            .expect("the first page opens");
        assert_eq!(page.streams[0].bytes.as_bytes(), b"/Fm0 Do");
        let copy = page
            .resources
            .xobject(b"/Fm0")
            .and_then(pdf_content::ResourceEntry::form)
            .expect("the first page names a Form");
        assert_ne!(copy.reference, Reference::new(5, 0));
        let content = String::from_utf8_lossy(copy.bytes.as_bytes()).into_owned();
        assert_eq!(content.matches("TJ").count(), 2, "{content}");
        assert!(content.contains("1 0 0 1 5 5 Tm"), "{content}");
    }

    #[test]
    fn undo_over_a_form_cluster_edit_points_the_page_back_at_the_definition() {
        let source = shared_form_fixture();
        let before = page_glyphs(&source, 0);
        let plan = plan_command(
            &source,
            &Command::DeleteTextClusters {
                page_index: 0,
                selection: TextRunSelection::Last,
                glyphs: 1..2,
            },
            b"",
        )
        .expect("a cluster inside a Form is deletable");

        let mut history = crate::History::new(source.clone(), b"");
        history.apply(plan).expect("the plan applies");
        assert_eq!(page_glyphs(history.source(), 0).len(), 2);

        assert!(history.undo().expect("the delete is undoable"));
        assert_eq!(
            page_glyphs(history.source(), 0),
            before,
            "the page paints what it painted before the edit"
        );
        assert!(history.source().len() > source.len());
        let page = load_page_program_strict(history.source(), 0, PageContentLimits::default())
            .expect("the undone page opens");
        let form = page
            .resources
            .xobject(b"/Fm0")
            .and_then(pdf_content::ResourceEntry::form)
            .expect("the page names a Form again");
        assert_eq!(
            form.reference,
            Reference::new(5, 0),
            "the page selects the shared definition once more"
        );

        assert!(history.redo().expect("the delete is redoable"));
        assert_eq!(page_glyphs(history.source(), 0).len(), 2);
    }

    #[test]
    fn a_form_invoked_twice_on_one_page_is_refused_rather_than_edited_twice() {
        let twice = shared_form_document(b"/Fm0 Do 1 0 0 1 30 0 cm /Fm0 Do");
        assert_eq!(page_glyphs(&twice, 0).len(), 6);
        assert!(matches!(
            plan_command(
                &twice,
                &Command::DeleteTextClusters {
                    page_index: 0,
                    selection: TextRunSelection::Last,
                    glyphs: 1..2,
                },
                b"",
            ),
            Err(SpikeError::FormInvokedTwiceOnThisPage)
        ));
    }

    #[test]
    fn a_repeated_page_stream_requires_copy_on_write() {
        let source = content_array_fixture(true);
        assert!(matches!(
            move_last_text_run(&source, 0, 10.0, 0.0, b""),
            Err(SpikeError::SharedPageContentStream)
        ));
    }

    fn span() -> pdf_bytes::SourceSpan {
        pdf_bytes::SourceSpan::new(SourceId::new(1), 0, 1).expect("a forward span")
    }

    fn rectangle(x: f64, y: f64, width: f64, height: f64) -> Path {
        Path {
            segments: vec![PathSegment::Rectangle {
                origin: Point { x, y },
                width,
                height,
                provenance: span(),
            }],
        }
    }

    fn rotation(degrees: f64) -> Matrix {
        let (sin, cos) = degrees.to_radians().sin_cos();
        Matrix {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            e: 0.0,
            f: 0.0,
        }
    }

    fn refused_by(path: &Path) -> SpikeError {
        SpikeError::from(
            crate::clip_region::ClipRegion::of(path, Matrix::IDENTITY)
                .expect_err("this shape is not readable"),
        )
    }

    fn admits_point(path: &Path, ctm: Matrix, x: f64, y: f64) -> bool {
        crate::clip_region::ClipRegion::of(path, ctm)
            .is_ok_and(|region| region.admits(&[Point { x, y }]))
    }

    #[test]
    fn a_point_inside_an_axis_aligned_clip_is_inside_and_one_outside_is_not() {
        let clip = rectangle(0.0, 0.0, 100.0, 50.0);
        assert!(admits_point(&clip, Matrix::IDENTITY, 50.0, 25.0));
        assert!(!admits_point(&clip, Matrix::IDENTITY, 150.0, 25.0));
        assert!(!admits_point(&clip, Matrix::IDENTITY, 50.0, -1.0));
    }

    #[test]
    fn a_rotated_clip_is_decided_exactly_rather_than_refused() {
        let clip = rectangle(0.0, 0.0, 100.0, 20.0);

        let centre = rotation(45.0).transform(Point { x: 50.0, y: 10.0 });
        assert!(admits_point(&clip, rotation(45.0), centre.x, centre.y));

        assert!(!admits_point(&clip, rotation(45.0), 60.0, 5.0));
    }

    #[test]
    fn a_curved_clip_that_turns_both_ways_is_refused_rather_than_approximated() {
        let curved = Path {
            segments: vec![
                PathSegment::MoveTo {
                    point: Point { x: 0.0, y: 0.0 },
                    provenance: span(),
                },
                PathSegment::CubicTo {
                    control_1: Point { x: 1.0, y: 1.0 },
                    control_2: Point { x: 2.0, y: -1.0 },
                    end: Point { x: 3.0, y: 0.0 },
                    provenance: span(),
                },
            ],
        };
        assert!(matches!(refused_by(&curved), SpikeError::ClipIsCurved));
    }

    #[test]
    fn two_subpaths_are_two_windows_now_rather_than_a_refusal() {
        let two_subpaths = Path {
            segments: vec![
                PathSegment::Rectangle {
                    origin: Point { x: 0.0, y: 0.0 },
                    width: 10.0,
                    height: 10.0,
                    provenance: span(),
                },
                PathSegment::Rectangle {
                    origin: Point { x: 20.0, y: 0.0 },
                    width: 10.0,
                    height: 10.0,
                    provenance: span(),
                },
            ],
        };
        assert!(admits_point(&two_subpaths, Matrix::IDENTITY, 5.0, 5.0));
        assert!(admits_point(&two_subpaths, Matrix::IDENTITY, 25.0, 5.0));
        assert!(
            !admits_point(&two_subpaths, Matrix::IDENTITY, 15.0, 5.0),
            "the gap between two windows is not inside either"
        );
    }

    #[test]
    fn a_concave_outline_is_refused_because_the_predicate_cannot_decide_it() {
        let concave = Path {
            segments: vec![
                PathSegment::MoveTo {
                    point: Point { x: 0.0, y: 0.0 },
                    provenance: span(),
                },
                PathSegment::LineTo {
                    point: Point { x: 10.0, y: 0.0 },
                    provenance: span(),
                },
                PathSegment::LineTo {
                    point: Point { x: 5.0, y: 5.0 },
                    provenance: span(),
                },
                PathSegment::LineTo {
                    point: Point { x: 10.0, y: 10.0 },
                    provenance: span(),
                },
                PathSegment::LineTo {
                    point: Point { x: 0.0, y: 10.0 },
                    provenance: span(),
                },
            ],
        };
        assert!(matches!(refused_by(&concave), SpikeError::ClipIsConcave));
    }

    #[test]
    fn a_closed_rectangle_that_repeats_its_first_point_is_still_convex() {
        let closed = Path {
            segments: vec![
                PathSegment::MoveTo {
                    point: Point { x: 0.0, y: 0.0 },
                    provenance: span(),
                },
                PathSegment::LineTo {
                    point: Point { x: 10.0, y: 0.0 },
                    provenance: span(),
                },
                PathSegment::LineTo {
                    point: Point { x: 10.0, y: 10.0 },
                    provenance: span(),
                },
                PathSegment::LineTo {
                    point: Point { x: 0.0, y: 10.0 },
                    provenance: span(),
                },
                PathSegment::LineTo {
                    point: Point { x: 0.0, y: 0.0 },
                    provenance: span(),
                },
                PathSegment::ClosePath { provenance: span() },
            ],
        };
        assert!(admits_point(&closed, Matrix::IDENTITY, 5.0, 5.0));
        assert!(!admits_point(&closed, Matrix::IDENTITY, 15.0, 5.0));
    }
}

#[cfg(test)]
mod clusters_after_tests {
    use pdf_semantics::ClusterKey;

    use super::tests::content_array_fixture;
    use super::{Holding, holding, plan_command};
    use crate::plan::{Command, TextRunSelection};
    use crate::{ClustersAfter, info::InfoEdit};

    #[test]
    fn a_plan_is_held_to_what_its_command_says_of_its_clusters() {
        let cannot = ClustersAfter::NotSaid("for a reason");
        for after in [
            ClustersAfter::TheSame,
            ClustersAfter::ThePlannerSays,
            ClustersAfter::ThePagesThemselves,
            cannot,
        ] {
            assert_eq!(holding(after, true), Holding::AsItIs, "{after:?}");
        }
        assert_eq!(holding(ClustersAfter::TheSame, false), Holding::AsItWas);
        assert_eq!(
            holding(ClustersAfter::ThePagesThemselves, false),
            Holding::AsItWas
        );
        assert_eq!(
            holding(ClustersAfter::ThePlannerSays, false),
            Holding::Refuse
        );
        assert_eq!(holding(cannot, false), Holding::AsItIs);
    }

    #[test]
    fn each_kind_of_command_says_what_becomes_of_its_clusters() {
        assert_eq!(
            Command::SetDocumentInfo {
                edit: InfoEdit::default()
            }
            .clusters_after(),
            ClustersAfter::TheSame
        );
        assert_eq!(
            Command::SetTextSize {
                page_index: 0,
                runs: Vec::new(),
                points: 12.0,
            }
            .clusters_after(),
            ClustersAfter::TheSame
        );
        assert_eq!(
            Command::RewriteText {
                page_index: 0,
                runs: Vec::new(),
            }
            .clusters_after(),
            ClustersAfter::ThePlannerSays
        );
        assert_eq!(
            Command::RemovePages { pages: vec![1] }.clusters_after(),
            ClustersAfter::ThePagesThemselves
        );
        for command in [
            Command::MoveTextCluster {
                page_index: 0,
                selection: TextRunSelection::Last,
                glyphs: 0..1,
                dx: 0.0,
                dy: 0.0,
            },
            Command::DeleteTextClusters {
                page_index: 0,
                selection: TextRunSelection::Last,
                glyphs: 0..1,
            },
        ] {
            assert!(
                matches!(command.clusters_after(), ClustersAfter::NotSaid(_)),
                "{command:?}"
            );
        }
    }

    #[test]
    fn describing_the_document_keeps_every_cluster_where_it_was() {
        let source = content_array_fixture(false);
        let plan = plan_command(
            &source,
            &Command::SetDocumentInfo {
                edit: InfoEdit {
                    title: Some("Named".to_owned()),
                    ..InfoEdit::default()
                },
            },
            b"",
        )
        .expect("the description is planned");
        let mapping = plan
            .correspondence()
            .expect("a command that changes no glyph says so");
        let first = ClusterKey { atom: 0, glyph: 0 };
        assert_eq!(mapping.get(&first), Some(&first));
        assert!(
            mapping.iter().all(|(one, other)| one == other),
            "every cluster is the cluster it was"
        );
    }
}
