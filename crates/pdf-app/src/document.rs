use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use pdf_bytes::ByteStore;
use pdf_cli::{
    CaretStop, ObjectBox, TextBlockBox, TextClusterBox, page_offset_view, page_overlay_view,
    page_run_offset_view, page_selection_between_view, page_transform_view,
};
use pdf_content::PageGeometry;
use pdf_edit::PageChange;
use pdf_edit::spike_move_text::SpikeError;
use pdf_edit::{
    BlockOutcome, BlockRange, ClusterRef, Command, Copied, FixedPoint, ObjectSelection,
    PlannedCaret, SourceAnchor, TextRunSelection,
};

use crate::frames::{Breaks, Edges};
use crate::wording::{
    BlockMove, Done, Hidden, Lang, Layout, LayoutWhy, Message, PictureMove, Refusal, Side,
};
use pdf_paint::Matrix;
use pdf_semantics::Grouping;
use pdf_session::{PageView, Session};

use crate::strip::Strip;

pub const OVERLAY_SCALE: f64 = 1.0;

#[derive(Clone, Debug, PartialEq)]
pub struct RunBox {
    pub anchor: String,
    pub bounds: [f64; 4],
    pub text: Arc<pdf_paint::ToUnicode>,
    pub em: f64,
    pub fill: Option<[f64; 3]>,
    pub family: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Export {
    pub bytes: Vec<u8>,
    pub placements: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Overlay {
    pub runs: Vec<RunBox>,
    pub clusters: Vec<TextClusterBox>,
    pub carets: Vec<CaretStop>,
    pub blocks: Vec<TextBlockBox>,
    pub objects: Vec<ObjectBox>,
    pub rows: Vec<pdf_cli::PanelRow>,
}

#[derive(Clone, Debug)]
pub struct Leaf {
    pub view: Arc<PageView>,
    pub overlay: Overlay,
    pub epoch: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Applied {
    Changed {
        page: usize,
        region: Option<[f64; 4]>,
    },
    Unchanged,
    Refused(Refusal),
}

#[derive(Debug)]
enum Step {
    Type {
        page: usize,
        block: usize,
        range: BlockRange,
        text: String,
        style: Option<pdf_edit::TextStyle>,
    },
    Style {
        page: usize,
        block: usize,
        range: BlockRange,
        style: pdf_edit::TextStyle,
    },
    FlowRound {
        page: usize,
        blocks: Vec<usize>,
    },
    ResizeFrame {
        page: usize,
        block: usize,
        started: [f64; 4],
    },
    AddPage {
        beside: usize,
        before: bool,
        size: [f64; 2],
    },
    RemovePages {
        pages: Vec<usize>,
    },
    MovePages {
        pages: Vec<usize>,
        to: usize,
    },
    RotatePages {
        pages: Vec<usize>,
        quarter_turns: i32,
    },
    Describe {
        edit: pdf_edit::info::InfoEdit,
    },
    Stamp {
        pages: Vec<usize>,
        stamp: pdf_edit::stamp::Stamp,
        start: i64,
        name: String,
        today: String,
    },
    TextLayers {
        layers: Vec<(usize, pdf_edit::text_layer::TextLayer)>,
        confidence: u8,
        had_text: usize,
        unread: usize,
    },
    InsertPages {
        beside: usize,
        before: bool,
        document: Arc<[u8]>,
        pages: Vec<usize>,
    },
    PlaceText {
        page: usize,
        frame: [f64; 4],
        text: String,
        family: String,
        size: f64,
        bold: bool,
        italic: bool,
        fill: Option<[f64; 3]>,
        paragraph: pdf_edit::ParagraphLayout,
    },
    PlaceImage {
        page: usize,
        pictures: Vec<(pdf_paint::Matrix, Arc<[u8]>)>,
    },
    DrawLine {
        page: usize,
        steps: Vec<pdf_edit::PenStep>,
        stroke: Option<pdf_edit::PenStroke>,
        fill: Option<[f64; 3]>,
        closed: bool,
        drew: Drew,
    },
    Move {
        page: usize,
        anchor: String,
        dx: f64,
        dy: f64,
    },
    MoveBlock {
        page: usize,
        anchors: Vec<String>,
        block: Option<usize>,
        group: bool,
        dx: f64,
        dy: f64,
    },
    MoveGroup {
        page: usize,
        anchors: Vec<String>,
        objects: Vec<String>,
        dx: f64,
        dy: f64,
    },
    Paste {
        page: usize,
        copied: Copied,
        dx: f64,
        dy: f64,
        elsewhere: Option<pdf_bytes::ByteStore>,
    },
    Place {
        page: usize,
        anchor: String,
        dx: f64,
        dy: f64,
    },
    Shape {
        page: usize,
        anchor: String,
        matrix: Matrix,
        about: (f64, f64),
    },
    ShapeBlock {
        page: usize,
        anchors: Vec<String>,
        matrix: Matrix,
        about: (f64, f64),
    },
    SetSize {
        page: usize,
        anchors: Vec<String>,
        points: f64,
    },
    SetAngles {
        page: usize,
        anchors: Vec<String>,
        turn: Option<f64>,
        slant: Option<f64>,
    },
    DeleteBlock {
        page: usize,
        anchors: Vec<String>,
    },
    DeleteGroup {
        page: usize,
        anchors: Vec<String>,
        objects: Vec<String>,
    },
    RemoveObject {
        page: usize,
        anchor: String,
        rubbing: bool,
    },
    ReorderObjects {
        page: usize,
        anchors: Vec<String>,
        order: pdf_edit::Stacking,
    },
    AddField {
        page: usize,
        rect: [f64; 4],
        kind: pdf_edit::new_field::NewFieldKind,
        name: Option<String>,
        options: Vec<String>,
    },
    SetFieldBoxes {
        page: usize,
        boxes: Vec<(pdf_syntax::Reference, [f64; 4])>,
    },
    CopyFields {
        page: usize,
        copies: Vec<(pdf_syntax::Reference, [f64; 4])>,
    },
    AddLink {
        page: usize,
        rect: [f64; 4],
        target: pdf_edit::link::Target,
        look: pdf_edit::link::Look,
    },
    AddLinks {
        page: usize,
        links: Vec<([f64; 4], pdf_edit::link::Target, pdf_edit::link::Look)>,
    },
    SetLinkProperties {
        page: usize,
        link: pdf_syntax::Reference,
        target: Option<pdf_edit::link::Target>,
        look: Option<pdf_edit::link::Look>,
    },
    ChangeNaming {
        page: usize,
        change: pdf_edit::destination::Naming,
    },
    SetLinkBox {
        page: usize,
        link: pdf_syntax::Reference,
        rect: [f64; 4],
    },
    SetLinkBoxes {
        page: usize,
        boxes: Vec<(pdf_syntax::Reference, [f64; 4])>,
    },
    RemoveLinks {
        page: usize,
        links: Vec<pdf_syntax::Reference>,
    },
    RemoveLink {
        page: usize,
        link: pdf_syntax::Reference,
    },
    ChangeOutline {
        page: usize,
        change: pdf_edit::outline::Change,
    },
    SetTabOrder {
        page: usize,
        widgets: Vec<pdf_syntax::Reference>,
        order: pdf_edit::tab_order::TabOrder,
    },
    RemoveFields {
        page: usize,
        widgets: Vec<pdf_syntax::Reference>,
    },
    SetFieldBox {
        page: usize,
        widget: pdf_syntax::Reference,
        rect: [f64; 4],
    },
    SetFieldSettings {
        page: usize,
        widget: pdf_syntax::Reference,
        settings: pdf_edit::field_settings::FieldSettings,
    },
    RemoveField {
        page: usize,
        widget: pdf_syntax::Reference,
    },
    FillField {
        page: usize,
        widget: pdf_syntax::Reference,
        value: pdf_edit::form::FieldValue,
    },
    Undo,
    Redo,
    #[cfg(test)]
    Panics,
}

#[derive(Clone, Debug)]
pub struct FieldBox {
    pub field: pdf_edit::form::FormField,
    pub pixels: [f64; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct LinkBox {
    pub link: pdf_syntax::Reference,
    pub pixels: [f64; 4],
    pub look: pdf_edit::link::Look,
    pub target: Option<pdf_edit::link::Target>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Drew {
    Line,
    Mark,
    Shape,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Drawn {
    pub steps: Vec<pdf_edit::PenStep>,
    pub stroke: Option<pdf_edit::PenStroke>,
    pub fill: Option<[f64; 3]>,
    pub closed: bool,
    pub drew: Drew,
}

impl Step {
    const fn name(&self) -> &'static str {
        match self {
            Self::Type { .. } => "Type",
            Self::Style { .. } => "Style",
            Self::FlowRound { .. } => "FlowRound",
            Self::ResizeFrame { .. } => "ResizeFrame",
            Self::PlaceText { .. } => "PlaceText",
            Self::PlaceImage { .. } => "PlaceImage",
            Self::DrawLine { .. } => "DrawLine",
            Self::AddPage { .. } => "AddPage",
            Self::RemovePages { .. } => "RemovePages",
            Self::MovePages { .. } => "MovePages",
            Self::RotatePages { .. } => "RotatePages",
            Self::Describe { .. } => "Describe",
            Self::InsertPages { .. } => "InsertPages",
            Self::Stamp { .. } => "Stamp",
            Self::TextLayers { .. } => "TextLayers",
            Self::Move { .. } => "Move",
            Self::MoveBlock { .. } => "MoveBlock",
            Self::MoveGroup { .. } => "MoveGroup",
            Self::Paste { .. } => "Paste",
            Self::Place { .. } => "Place",
            Self::Shape { .. } => "Shape",
            Self::ShapeBlock { .. } => "ShapeBlock",
            Self::SetSize { .. } => "SetSize",
            Self::SetAngles { .. } => "SetAngles",
            Self::DeleteBlock { .. } => "DeleteBlock",
            Self::DeleteGroup { .. } => "DeleteGroup",
            Self::RemoveObject { .. } => "RemoveObject",
            Self::ReorderObjects { .. } => "ReorderObjects",
            Self::FillField { .. } => "FillField",
            Self::AddField { .. } => "AddField",
            Self::RemoveField { .. } => "RemoveField",
            Self::SetFieldBox { .. } => "SetFieldBox",
            Self::SetFieldBoxes { .. } => "SetFieldBoxes",
            Self::CopyFields { .. } => "CopyFields",
            Self::RemoveFields { .. } => "RemoveFields",
            Self::SetTabOrder { .. } => "SetTabOrder",
            Self::ChangeOutline { .. } => "ChangeOutline",
            Self::AddLink { .. } => "AddLink",
            Self::AddLinks { .. } => "AddLinks",
            Self::SetLinkProperties { .. } => "SetLinkProperties",
            Self::ChangeNaming { .. } => "ChangeNaming",
            Self::SetLinkBox { .. } => "SetLinkBox",
            Self::RemoveLink { .. } => "RemoveLink",
            Self::SetLinkBoxes { .. } => "SetLinkBoxes",
            Self::RemoveLinks { .. } => "RemoveLinks",
            Self::SetFieldSettings { .. } => "SetFieldSettings",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            #[cfg(test)]
            Self::Panics => "Panics",
        }
    }

    fn page_arguments(&self) -> String {
        match self {
            Self::RemovePages { pages } => format!("pages={pages:?}"),
            Self::MovePages { pages, to } => format!("pages={pages:?} to={to}"),
            Self::RotatePages {
                pages,
                quarter_turns,
            } => format!("pages={pages:?} quarter_turns={quarter_turns}"),
            Self::InsertPages {
                beside,
                before,
                document,
                pages,
            } => format!(
                "beside={beside} before={before} bytes={} pages={pages:?}",
                document.len()
            ),
            _ => String::new(),
        }
    }

    #[expect(clippy::too_many_lines, reason = "one record, one arm per step")]
    fn arguments(&self) -> String {
        match self {
            Self::Type {
                page,
                block,
                range,
                text,
                style: None,
            } => format!("page={page} block={block} range={range:?} text={text:?}"),
            Self::Type {
                page,
                block,
                range,
                text,
                style: Some(style),
            } => format!("page={page} block={block} range={range:?} text={text:?} style={style:?}"),
            Self::Style {
                page,
                block,
                range,
                style,
            } => format!("page={page} block={block} range={range:?} style={style:?}"),
            Self::FlowRound { page, blocks } => format!("page={page} blocks={blocks:?}"),
            Self::ResizeFrame {
                page,
                block,
                started,
            } => format!("page={page} block={block} started={started:?}"),
            Self::PlaceText {
                page,
                frame,
                text,
                family,
                size,
                bold,
                italic,
                fill,
                paragraph,
            } => format!(
                "page={page} frame={frame:?} text={text:?} family={family} size={size} \
                 bold={bold} italic={italic} fill={fill:?} paragraph={paragraph:?}"
            ),
            Self::PlaceImage { page, pictures } => format!(
                "page={page} placements={:?} bytes={:?}",
                pictures
                    .iter()
                    .map(|(placement, _)| placement)
                    .collect::<Vec<_>>(),
                pictures
                    .iter()
                    .map(|(_, file)| file.len())
                    .collect::<Vec<_>>()
            ),
            Self::DrawLine {
                page,
                steps,
                stroke,
                fill,
                closed,
                drew,
            } => format!(
                "page={page} points={} stroke={stroke:?} fill={fill:?} closed={closed} drew={drew:?}",
                steps.len()
            ),
            Self::AddPage {
                beside,
                before,
                size,
            } => format!("beside={beside} before={before} size={size:?}"),
            Self::RemovePages { .. }
            | Self::MovePages { .. }
            | Self::RotatePages { .. }
            | Self::InsertPages { .. } => self.page_arguments(),
            Self::Describe { edit } => format!("{edit:?}"),
            Self::Move {
                page,
                anchor,
                dx,
                dy,
            }
            | Self::Place {
                page,
                anchor,
                dx,
                dy,
            } => format!("page={page} anchor={anchor} dx={dx} dy={dy}"),
            Self::MoveBlock {
                page,
                anchors,
                group,
                dx,
                dy,
                ..
            } => format!(
                "page={page} anchors={} dx={dx} dy={dy}{}",
                anchors.len(),
                if *group { " group" } else { "" }
            ),
            Self::MoveGroup {
                page,
                anchors,
                objects,
                dx,
                dy,
            } => format!(
                "page={page} anchors={} objects={} dx={dx} dy={dy}",
                anchors.len(),
                objects.len()
            ),
            Self::Paste {
                page,
                copied,
                dx,
                dy,
                elsewhere,
            } => format!(
                "page={page} objects={} dx={dx} dy={dy} from={}",
                copied.objects.len(),
                if elsewhere.is_some() {
                    "another document"
                } else {
                    "this document"
                }
            ),
            Self::Shape { page, anchor, .. } => format!("page={page} anchor={anchor} handle"),
            Self::ShapeBlock { page, anchors, .. } => {
                format!("page={page} anchors={} handle", anchors.len())
            }
            Self::SetSize {
                page,
                anchors,
                points,
            } => format!("page={page} anchors={} points={points}", anchors.len()),
            Self::SetAngles {
                page,
                anchors,
                turn,
                slant,
            } => format!(
                "page={page} anchors={} turn={turn:?} slant={slant:?}",
                anchors.len()
            ),
            Self::DeleteBlock { page, anchors } => {
                format!("page={page} anchors={}", anchors.len())
            }
            Self::DeleteGroup {
                page,
                anchors,
                objects,
            } => format!(
                "page={page} anchors={} objects={}",
                anchors.len(),
                objects.len()
            ),
            Self::RemoveObject {
                page,
                anchor,
                rubbing,
            } => format!("page={page} anchor={anchor} rubbing={rubbing}"),
            Self::ReorderObjects {
                page,
                anchors,
                order,
            } => format!("page={page} anchors={} order={order:?}", anchors.len()),
            Self::AddField {
                page,
                rect,
                kind,
                name,
                options,
            } => {
                format!("page={page} rect={rect:?} kind={kind:?} name={name:?} options={options:?}")
            }
            Self::SetFieldBoxes { page, boxes } => format!(
                "page={page} boxes={:?}",
                boxes
                    .iter()
                    .map(|(widget, rect)| (widget.object_number(), *rect))
                    .collect::<Vec<_>>()
            ),
            Self::CopyFields { page, copies } => format!(
                "page={page} copies={:?}",
                copies
                    .iter()
                    .map(|(widget, rect)| (widget.object_number(), *rect))
                    .collect::<Vec<_>>()
            ),
            Self::ChangeOutline { page, change } => format!("page={page} change={change:?}"),
            Self::AddLink {
                page,
                rect,
                target,
                look,
            } => format!("page={page} rect={rect:?} target={target:?} look={look:?}"),
            Self::AddLinks { page, links } => {
                format!("page={page} links={}", links.len())
            }
            Self::SetLinkProperties {
                page,
                link,
                target,
                look,
            } => format!(
                "page={page} link={} target={target:?} look={look:?}",
                link.object_number()
            ),
            Self::ChangeNaming { page, change } => format!("page={page} change={change:?}"),
            Self::Stamp {
                pages,
                stamp,
                start,
                ..
            } => format!("pages={pages:?} start={start} stamp={stamp:?}"),
            Self::TextLayers { layers, .. } => format!(
                "pages={:?} words={:?}",
                layers.iter().map(|(page, _)| page).collect::<Vec<_>>(),
                layers
                    .iter()
                    .map(|(_, layer)| layer.words.len())
                    .collect::<Vec<_>>()
            ),
            Self::SetLinkBox { page, link, rect } => {
                format!("page={page} link={} rect={rect:?}", link.object_number())
            }
            Self::RemoveLink { page, link } => {
                format!("page={page} link={}", link.object_number())
            }
            Self::SetLinkBoxes { page, boxes } => format!(
                "page={page} boxes={:?}",
                boxes
                    .iter()
                    .map(|(link, rect)| (link.object_number(), *rect))
                    .collect::<Vec<_>>()
            ),
            Self::RemoveLinks { page, links } => format!(
                "page={page} links={:?}",
                links
                    .iter()
                    .copied()
                    .map(pdf_syntax::Reference::object_number)
                    .collect::<Vec<_>>()
            ),
            Self::SetTabOrder {
                page,
                widgets,
                order,
            } => format!(
                "page={page} order={order:?} widgets={:?}",
                widgets
                    .iter()
                    .copied()
                    .map(pdf_syntax::Reference::object_number)
                    .collect::<Vec<_>>()
            ),
            Self::RemoveFields { page, widgets } => format!(
                "page={page} widgets={:?}",
                widgets
                    .iter()
                    .copied()
                    .map(pdf_syntax::Reference::object_number)
                    .collect::<Vec<_>>()
            ),
            Self::SetFieldBox { page, widget, rect } => format!(
                "page={page} widget={} {} rect={rect:?}",
                widget.object_number(),
                widget.generation()
            ),
            Self::SetFieldSettings {
                page,
                widget,
                settings,
            } => format!(
                "page={page} widget={} {} settings={settings:?}",
                widget.object_number(),
                widget.generation()
            ),
            Self::RemoveField { page, widget } => format!(
                "page={page} widget={} {}",
                widget.object_number(),
                widget.generation()
            ),
            Self::FillField {
                page,
                widget,
                value,
            } => format!(
                "page={page} widget={} {} value={value:?}",
                widget.object_number(),
                widget.generation()
            ),
            Self::Undo | Self::Redo => String::new(),
            #[cfg(test)]
            Self::Panics => String::new(),
        }
    }
}

pub struct EditJob {
    session: Session,
    step: Step,
    frames: crate::frames::Frames,
    paragraphs: Paragraphs,
    flowed: Flowed,
    move_frame: Option<(usize, f64, f64)>,
    group_frames: Vec<(usize, f64, f64)>,
    resize_frame: Option<(usize, [f64; 4], Edges, Breaks)>,
    whole_block: Option<(usize, (usize, usize))>,
    wrote_text: Option<String>,
    laid: Option<(usize, Overlay)>,
}

type Paragraphs = BTreeMap<(usize, usize), pdf_edit::ParagraphLayout>;

type Flowed = BTreeMap<(usize, usize), Vec<pdf_edit::Blocked>>;

fn paragraph_of(paragraphs: &Paragraphs, page: usize, block: usize) -> pdf_edit::ParagraphLayout {
    paragraphs.get(&(page, block)).copied().unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq)]
pub struct NewTextStyle {
    pub family: String,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    pub fill: Option<[f64; 3]>,
    pub paragraph: pdf_edit::ParagraphLayout,
}

struct Typed {
    block: usize,
    row: usize,
    offset: usize,
    anchor: Option<(usize, usize)>,
    frame: Option<[f64; 4]>,
    edges: Edges,
    breaks: Breaks,
    overflow: bool,
    brought_in: Option<String>,
    drawn_from: Option<String>,
    cropped: bool,
    note: Option<Refusal>,
    status: Option<Done>,
}

enum Edited {
    Nothing,
    Refused(Refusal),
}

impl From<Refusal> for Edited {
    fn from(reason: Refusal) -> Self {
        Self::Refused(reason)
    }
}

impl From<String> for Edited {
    fn from(reason: String) -> Self {
        Self::Refused(reason.into())
    }
}

impl From<&str> for Edited {
    fn from(reason: &str) -> Self {
        Self::Refused(reason.into())
    }
}

const NOTHING_TO_DELETE: &str = "there is nothing there to delete";

const EMPTYING_THE_BLOCK: &str = "every character deleted";

pub struct EditOutcome {
    session: Session,
    applied: Applied,
    status: Message,
    frames: crate::frames::Frames,
    paragraphs: Paragraphs,
    flowed: Flowed,
    caret: Option<(usize, usize)>,
    anchor: Option<(usize, usize)>,
    wrote_text: Option<String>,
    laid: Option<(usize, Overlay)>,
    timed: Option<pdf_session::stages::Stages>,
}

impl EditOutcome {
    #[must_use]
    pub const fn applied(&self) -> &Applied {
        &self.applied
    }
}

pub struct Editor {
    frames: crate::frames::Frames,
    paragraphs: Paragraphs,
    flowed: Flowed,
    session: Option<Session>,
    last_edit: Option<pdf_session::stages::Stages>,
    fonts: Option<Arc<dyn pdf_content::FontProvider>>,
    credential: Vec<u8>,
    restricted: bool,
    geometries: Vec<PageGeometry>,
    strip: Strip,
    links: Option<Arc<pdf_content::LinkResolver>>,
    page_links: BTreeMap<usize, Arc<pdf_content::PageLinks>>,
    named_places: Option<Arc<Vec<pdf_edit::destination::Spot>>>,
    page_fields: BTreeMap<usize, Arc<Vec<FieldBox>>>,
    page_link_boxes: BTreeMap<usize, Arc<Vec<LinkBox>>>,
    document_fields: Option<Arc<Vec<(usize, pdf_edit::form::FormField)>>>,
    outline: Option<Arc<Vec<pdf_edit::outline::Bookmark>>>,
    read: BTreeMap<usize, Arc<Leaf>>,
    wanted_at: BTreeMap<usize, u64>,
    asks: u64,
    seen: BTreeSet<usize>,
    groupings: BTreeMap<usize, Arc<Grouping>>,
    epoch: u64,
    status: Message,
    landed_caret: Option<(usize, usize)>,
    landed_anchor: Option<(usize, usize)>,
    wrote_text: Option<String>,
    pages_changed: Option<PageChange>,
    pages_redrawn: bool,
    redraw_reason: Option<&'static str>,
    ledger: crate::ledger::Ledger,
    tracing: bool,
    pending: Option<Pending>,
}

struct Pending {
    request: u64,
    command: &'static str,
    arguments: String,
    before: crate::ledger::DocumentState,
}

impl Editor {
    pub fn open(source: ByteStore) -> Result<Self, String> {
        Self::open_with(source, b"")
    }

    pub fn stand_in() -> Result<Self, String> {
        Self::open(ByteStore::new(
            pdf_bytes::SourceId::next_document(),
            Arc::<[u8]>::from(blank_pdf()),
        ))
    }

    pub fn blank(size: [f64; 2]) -> Result<Self, String> {
        let bytes = pdf_session::blank_document(size).map_err(|error| error.to_string())?;
        Self::open(ByteStore::new(
            pdf_bytes::SourceId::next_document(),
            Arc::<[u8]>::from(bytes),
        ))
    }

    pub fn open_with(source: ByteStore, credential: &[u8]) -> Result<Self, String> {
        let session = Session::with_fonts(source, credential, pdf_cli::font_provider());
        let geometries = session
            .page_geometries()
            .map_err(|error| format!("this document cannot be laid out: {error}"))?;
        if geometries.is_empty() {
            return Err("this document has no pages".to_owned());
        }
        let strip = Strip::of(&geometries);
        let links = links_of(session.source(), credential);
        let restricted = session.restricts_editing().unwrap_or(false);
        Ok(Self {
            fonts: session.font_provider().cloned(),
            session: Some(session),
            last_edit: None,
            credential: credential.to_vec(),
            restricted,
            geometries,
            strip,
            links,
            page_links: BTreeMap::new(),
            named_places: None,
            page_fields: BTreeMap::new(),
            page_link_boxes: BTreeMap::new(),
            document_fields: None,
            outline: None,
            read: BTreeMap::new(),
            wanted_at: BTreeMap::new(),
            asks: 0,
            seen: BTreeSet::new(),
            groupings: BTreeMap::new(),
            frames: crate::frames::Frames::default(),
            paragraphs: BTreeMap::new(),
            flowed: BTreeMap::new(),
            epoch: 0,
            status: Message::Quiet,
            landed_caret: None,
            landed_anchor: None,
            wrote_text: None,
            pages_changed: None,
            pages_redrawn: false,
            redraw_reason: None,
            ledger: crate::ledger::Ledger::default(),
            tracing: false,
            pending: None,
        })
    }

    #[must_use]
    pub const fn strip(&self) -> &Strip {
        &self.strip
    }

    #[must_use]
    pub fn page_count(&self) -> usize {
        self.geometries.len()
    }

    #[must_use]
    pub fn geometry(&self, page: usize) -> Option<&PageGeometry> {
        self.geometries.get(page)
    }

    #[must_use]
    pub fn page_pixels(&self, page: usize, scale: f64) -> Option<(u32, u32)> {
        let device = pdf_render::DeviceTransform::for_page(
            self.geometry(page)?,
            scale,
            pdf_render::RenderLimits::default(),
        )
        .ok()?;
        Some((device.width, device.height))
    }

    #[must_use]
    pub fn region_in_pixels(&self, page: usize, scale: f64, region: [f64; 4]) -> Option<[u32; 4]> {
        pdf_render::DeviceTransform::for_page(
            self.geometry(page)?,
            scale,
            pdf_render::RenderLimits::default(),
        )
        .ok()?
        .pixel_box(region)
    }

    pub fn fields_on(&mut self, page: usize) -> Arc<Vec<FieldBox>> {
        if let Some(fields) = self.page_fields.get(&page) {
            return Arc::clone(fields);
        }
        let (Some(session), Some(leaf)) = (self.session.as_ref(), self.read.get(&page)) else {
            return Arc::new(Vec::new());
        };
        let Some(device) = overlay_device(&leaf.view) else {
            return Arc::new(Vec::new());
        };
        let read = pdf_edit::form::fields_of_page(
            session.source(),
            leaf.view.program.page,
            &self.credential,
        )
        .unwrap_or_default();
        let boxes: Vec<FieldBox> = read
            .into_iter()
            .map(|field| {
                let pixels = in_pixels(&device, field.rect);
                FieldBox { field, pixels }
            })
            .collect();
        let boxes = Arc::new(boxes);
        self.page_fields.insert(page, Arc::clone(&boxes));
        boxes
    }

    #[must_use]
    pub fn begin_add_field(
        &mut self,
        page: usize,
        pixels: [f64; 4],
        kind: pdf_edit::new_field::NewFieldKind,
        (name, options): (Option<String>, Vec<String>),
    ) -> Option<EditJob> {
        let rect = self.rect_in_user_space(page, pixels)?;
        self.begin(Step::AddField {
            page,
            rect,
            kind,
            name,
            options,
        })
    }

    #[must_use]
    pub fn begin_set_field_box(
        &mut self,
        page: usize,
        widget: pdf_syntax::Reference,
        pixels: [f64; 4],
    ) -> Option<EditJob> {
        let rect = self.rect_in_user_space(page, pixels)?;
        self.begin(Step::SetFieldBox { page, widget, rect })
    }

    #[must_use]
    pub fn begin_set_field_boxes(
        &mut self,
        page: usize,
        boxes: &[(pdf_syntax::Reference, [f64; 4])],
    ) -> Option<EditJob> {
        let boxes = self.boxes_in_user_space(page, boxes)?;
        self.begin(Step::SetFieldBoxes { page, boxes })
    }

    #[must_use]
    pub fn begin_copy_fields(
        &mut self,
        page: usize,
        copies: &[(pdf_syntax::Reference, [f64; 4])],
    ) -> Option<EditJob> {
        let copies = self.boxes_in_user_space(page, copies)?;
        self.begin(Step::CopyFields { page, copies })
    }

    pub fn named_places(&mut self) -> Arc<Vec<pdf_edit::destination::Spot>> {
        if let Some(places) = self.named_places.as_ref() {
            return Arc::clone(places);
        }
        let places = Arc::new(
            self.session
                .as_ref()
                .and_then(|session| pdf_edit::destination::spots_of(session.source()).ok())
                .unwrap_or_default(),
        );
        self.named_places = Some(Arc::clone(&places));
        places
    }

    pub fn links_on(&mut self, page: usize) -> Arc<Vec<LinkBox>> {
        if let Some(links) = self.page_link_boxes.get(&page) {
            return Arc::clone(links);
        }
        let (Some(resolver), Some(leaf)) = (self.links.as_ref(), self.read.get(&page)) else {
            return Arc::new(Vec::new());
        };
        let Some(device) = overlay_device(&leaf.view) else {
            return Arc::new(Vec::new());
        };
        let Ok(read) = resolver.links(page) else {
            return Arc::new(Vec::new());
        };
        let boxes: Vec<LinkBox> = read
            .links
            .iter()
            .filter_map(|link| {
                let target = pdf_edit::link::target_of(&link.followed());
                Some(LinkBox {
                    look: pdf_edit::link::Look::default(),
                    link: link.reference?,
                    pixels: in_pixels(&device, link.rect),
                    target,
                })
            })
            .collect();
        let mut boxes = boxes;
        if let Some(session) = self.session.as_ref() {
            let references: Vec<pdf_syntax::Reference> =
                boxes.iter().map(|found| found.link).collect();
            if let Ok(looks) =
                pdf_edit::link::looks_of(session.source(), &self.credential, &references)
            {
                for (found, look) in boxes.iter_mut().zip(looks) {
                    found.look = look;
                }
            }
            if let Ok(names) =
                pdf_edit::link::names_of(session.source(), &self.credential, &references)
            {
                for (found, name) in boxes.iter_mut().zip(names) {
                    if let Some(name) = name {
                        found.target = Some(pdf_edit::link::Target::Name(name));
                    }
                }
            }
        }
        let boxes = Arc::new(boxes);
        self.page_link_boxes.insert(page, Arc::clone(&boxes));
        boxes
    }

    #[must_use]
    pub fn begin_add_link(
        &mut self,
        page: usize,
        pixels: [f64; 4],
        (target, look): (pdf_edit::link::Target, pdf_edit::link::Look),
    ) -> Option<EditJob> {
        let rect = self.rect_in_user_space(page, pixels)?;
        self.begin(Step::AddLink {
            page,
            rect,
            target,
            look,
        })
    }

    #[must_use]
    pub fn begin_add_links(
        &mut self,
        page: usize,
        links: &[([f64; 4], pdf_edit::link::Target, pdf_edit::link::Look)],
    ) -> Option<EditJob> {
        let boxes: Vec<(pdf_syntax::Reference, [f64; 4])> = links
            .iter()
            .map(|(pixels, _, _)| (pdf_syntax::Reference::new(0, 0), *pixels))
            .collect();
        let rects = self.boxes_in_user_space(page, &boxes)?;
        let links: Vec<([f64; 4], pdf_edit::link::Target, pdf_edit::link::Look)> = rects
            .into_iter()
            .zip(links)
            .map(|((_, rect), (_, target, look))| (rect, target.clone(), *look))
            .collect();
        self.begin(Step::AddLinks { page, links })
    }

    #[must_use]
    pub fn begin_set_link_properties(
        &mut self,
        page: usize,
        link: pdf_syntax::Reference,
        (target, look): (Option<pdf_edit::link::Target>, Option<pdf_edit::link::Look>),
    ) -> Option<EditJob> {
        self.begin(Step::SetLinkProperties {
            page,
            link,
            target,
            look,
        })
    }

    #[must_use]
    pub fn begin_stamp(
        &mut self,
        pages: Vec<usize>,
        stamp: pdf_edit::stamp::Stamp,
        (start, name, today): (i64, String, String),
    ) -> Option<EditJob> {
        self.begin(Step::Stamp {
            pages,
            stamp,
            start,
            name,
            today,
        })
    }

    #[must_use]
    pub fn begin_text_layers(
        &mut self,
        layers: Vec<(usize, pdf_edit::text_layer::TextLayer)>,
        (confidence, had_text, unread): (u8, usize, usize),
    ) -> Option<EditJob> {
        self.begin(Step::TextLayers {
            layers,
            confidence,
            had_text,
            unread,
        })
    }

    pub fn text_layers(
        &mut self,
        layers: Vec<(usize, pdf_edit::text_layer::TextLayer)>,
        (confidence, had_text, unread): (u8, usize, usize),
    ) -> Applied {
        self.here(Step::TextLayers {
            layers,
            confidence,
            had_text,
            unread,
        })
    }

    pub fn stamp(
        &mut self,
        pages: Vec<usize>,
        stamp: pdf_edit::stamp::Stamp,
        (start, name, today): (i64, String, String),
    ) -> Applied {
        self.here(Step::Stamp {
            pages,
            stamp,
            start,
            name,
            today,
        })
    }

    pub fn stamp_landing(
        &self,
        page: usize,
        stamp: &pdf_edit::stamp::Stamp,
        facts: &pdf_edit::stamp::Facts,
    ) -> Result<(pdf_edit::stamp::Landing, Option<[f64; 4]>), Message> {
        let session = self.session.as_ref().ok_or(Message::Quiet)?;
        let Some(leaf) = self.read.get(&page) else {
            return Err(Message::StampPreviewNotShown { page });
        };
        let view = &leaf.view;
        let landing = pdf_edit::stamp::landing(
            &pdf_edit::spike_move_text::PlannerPage {
                program: &view.program,
                operations: &view.operations,
                graph: &view.graph,
                fonts: session.font_provider(),
                restrictions: session.restrictions(),
                credential: session.credential(),
            },
            stamp,
            facts,
        )
        .map_err(|error| Message::Refused(why_a_stamp_is_refused(&error)))?;
        let pixels = overlay_device(view).map(|device| in_pixels(&device, landing.frame));
        Ok((landing, pixels))
    }

    #[must_use]
    pub fn begin_change_naming(
        &mut self,
        page: usize,
        change: pdf_edit::destination::Naming,
    ) -> Option<EditJob> {
        self.begin(Step::ChangeNaming { page, change })
    }

    #[must_use]
    pub fn begin_set_link_box(
        &mut self,
        page: usize,
        link: pdf_syntax::Reference,
        pixels: [f64; 4],
    ) -> Option<EditJob> {
        let rect = self.rect_in_user_space(page, pixels)?;
        self.begin(Step::SetLinkBox { page, link, rect })
    }

    #[must_use]
    pub fn begin_set_link_boxes(
        &mut self,
        page: usize,
        boxes: &[(pdf_syntax::Reference, [f64; 4])],
    ) -> Option<EditJob> {
        let boxes = self.boxes_in_user_space(page, boxes)?;
        self.begin(Step::SetLinkBoxes { page, boxes })
    }

    #[must_use]
    pub fn begin_remove_links(
        &mut self,
        page: usize,
        links: &[pdf_syntax::Reference],
    ) -> Option<EditJob> {
        self.begin(Step::RemoveLinks {
            page,
            links: links.to_vec(),
        })
    }

    #[must_use]
    pub fn begin_remove_link(
        &mut self,
        page: usize,
        link: pdf_syntax::Reference,
    ) -> Option<EditJob> {
        self.begin(Step::RemoveLink { page, link })
    }

    #[must_use]
    pub fn begin_change_outline(
        &mut self,
        page: usize,
        change: pdf_edit::outline::Change,
    ) -> Option<EditJob> {
        self.begin(Step::ChangeOutline { page, change })
    }

    pub fn bookmarks(&mut self) -> Arc<Vec<pdf_edit::outline::Bookmark>> {
        if let Some(read) = self.outline.as_ref() {
            return Arc::clone(read);
        }
        let found = self.session.as_ref().map_or_else(Vec::new, |session| {
            pdf_edit::outline::read_outline(session.source(), &self.credential).unwrap_or_default()
        });
        let found = Arc::new(found);
        self.outline = Some(Arc::clone(&found));
        found
    }

    #[must_use]
    pub fn first_line_of(&self, page: usize) -> Option<String> {
        let leaf = self.leaf(page)?;
        let first = leaf.overlay.clusters.first()?.line;
        let line: String = leaf
            .overlay
            .clusters
            .iter()
            .take_while(|cluster| cluster.line == first)
            .filter_map(|cluster| cluster.text.clone())
            .collect();
        let line = line.trim();
        (!line.is_empty()).then(|| line.chars().take(80).collect())
    }

    #[must_use]
    pub fn begin_set_tab_order(
        &mut self,
        page: usize,
        widgets: Vec<pdf_syntax::Reference>,
        order: pdf_edit::tab_order::TabOrder,
    ) -> Option<EditJob> {
        self.begin(Step::SetTabOrder {
            page,
            widgets,
            order,
        })
    }

    pub fn fields_in_document(&mut self) -> Arc<Vec<(usize, pdf_edit::form::FormField)>> {
        if let Some(fields) = self.document_fields.as_ref() {
            return Arc::clone(fields);
        }
        let found = self.session.as_ref().map_or_else(Vec::new, |session| {
            pdf_edit::form::fields_of_document(session.source(), &self.credential)
                .unwrap_or_default()
        });
        let found = Arc::new(found);
        self.document_fields = Some(Arc::clone(&found));
        found
    }

    #[must_use]
    pub fn begin_remove_fields(
        &mut self,
        page: usize,
        widgets: Vec<pdf_syntax::Reference>,
    ) -> Option<EditJob> {
        self.begin(Step::RemoveFields { page, widgets })
    }

    fn boxes_in_user_space(
        &self,
        page: usize,
        boxes: &[(pdf_syntax::Reference, [f64; 4])],
    ) -> Option<Vec<(pdf_syntax::Reference, [f64; 4])>> {
        boxes
            .iter()
            .map(|(widget, pixels)| Some((*widget, self.rect_in_user_space(page, *pixels)?)))
            .collect()
    }

    #[must_use]
    pub fn begin_set_field_settings(
        &mut self,
        page: usize,
        widget: pdf_syntax::Reference,
        settings: pdf_edit::field_settings::FieldSettings,
    ) -> Option<EditJob> {
        self.begin(Step::SetFieldSettings {
            page,
            widget,
            settings,
        })
    }

    #[must_use]
    pub fn begin_remove_field(
        &mut self,
        page: usize,
        widget: pdf_syntax::Reference,
    ) -> Option<EditJob> {
        self.begin(Step::RemoveField { page, widget })
    }

    #[must_use]
    pub fn rect_in_user_space(&self, page: usize, [x0, y0, x1, y1]: [f64; 4]) -> Option<[f64; 4]> {
        let inverse = self.overlay_device(page)?.matrix.inverse()?;
        let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
            .map(|(x, y)| inverse.transform(pdf_paint::Point { x, y }));
        let edge = |pick: fn(f64, f64) -> f64, along: fn(&pdf_paint::Point) -> f64, start| {
            Self::thousandths(corners.iter().map(along).fold(start, pick))
        };
        Some([
            edge(f64::min, |at| at.x, f64::INFINITY),
            edge(f64::min, |at| at.y, f64::INFINITY),
            edge(f64::max, |at| at.x, f64::NEG_INFINITY),
            edge(f64::max, |at| at.y, f64::NEG_INFINITY),
        ])
    }

    #[must_use]
    pub fn field_names(&self) -> Vec<String> {
        self.session.as_ref().map_or_else(Vec::new, |session| {
            pdf_edit::form::field_names(session.source(), &self.credential)
        })
    }

    #[must_use]
    pub fn begin_fill_field(
        &mut self,
        page: usize,
        widget: pdf_syntax::Reference,
        value: pdf_edit::form::FieldValue,
    ) -> Option<EditJob> {
        self.begin(Step::FillField {
            page,
            widget,
            value,
        })
    }

    pub fn fill_field(
        &mut self,
        page: usize,
        widget: pdf_syntax::Reference,
        value: pdf_edit::form::FieldValue,
    ) -> Applied {
        self.here(Step::FillField {
            page,
            widget,
            value,
        })
    }

    pub fn link_at(&mut self, page: usize, point: (f64, f64)) -> Option<pdf_content::Link> {
        let device = pdf_render::DeviceTransform::for_page(
            self.geometry(page)?,
            OVERLAY_SCALE,
            pdf_render::RenderLimits::default(),
        )
        .ok()?;
        let [x, y] = device.user_point([point.0, point.1])?;
        if !self.page_links.contains_key(&page) {
            let links = self.links.as_ref()?.links(page).ok()?;
            self.page_links.insert(page, Arc::new(links));
        }
        let hidden = pdf_content::AnnotationFlags::HIDDEN | pdf_content::AnnotationFlags::NO_VIEW;
        self.page_links
            .get(&page)?
            .links
            .iter()
            .rev()
            .find(|link| link.flags & hidden == 0 && crate::links::contains(link, x, y))
            .cloned()
    }

    #[must_use]
    pub fn leaf(&self, page: usize) -> Option<&Arc<Leaf>> {
        self.read.get(&page)
    }

    #[must_use]
    pub fn grouping(&self, page: usize) -> Option<Arc<Grouping>> {
        self.groupings.get(&page).map(Arc::clone)
    }

    pub fn adopt_page(&mut self, page: usize, view: Arc<PageView>) {
        let overlay = overlay_of(&view).unwrap_or_default();
        self.adopt_page_laid(page, view, overlay);
    }

    fn adopt_page_laid(&mut self, page: usize, view: Arc<PageView>, mut overlay: Overlay) {
        let seeds: Vec<[f64; 4]> = overlay
            .blocks
            .iter()
            .map(|block| block.layout_pixels)
            .collect();
        let turns: Vec<f64> = overlay.blocks.iter().map(|block| block.turn).collect();
        self.frames
            .establish_turned(page, seeds.clone(), turns.clone());
        self.frames.fit(page, &seeds, &turns);
        self.frames.retune(page, &turns, &seeds);
        apply_frames(&mut overlay, self.frames.page(page), overlay_device(&view));
        add_empty_lines(&view, &mut overlay, self.frames.page(page), |block| {
            (
                self.frames.edges(page, block),
                self.frames.breaks(page, block),
            )
        });
        self.seen.insert(page);
        if self
            .groupings
            .get(&page)
            .is_none_or(|_| view.index.report.clusters_outside_the_grouping > 0)
        {
            self.groupings
                .insert(page, Arc::new(Grouping::of(&view.index)));
        }
        if let Some(session) = self.session.as_mut() {
            session.establish_grouping(page, Arc::clone(&self.groupings[&page]));
        }
        self.read.insert(
            page,
            Arc::new(Leaf {
                view,
                overlay,
                epoch: self.epoch,
            }),
        );
    }

    pub fn keep_pages(&mut self, wanted: &BTreeSet<usize>, keep: usize, bytes: usize) {
        self.asks += 1;
        let asks = self.asks;
        for page in wanted {
            self.wanted_at.insert(*page, asks);
        }
        if self.read.len() <= keep.max(wanted.len()) && self.held_bytes() <= bytes {
            return;
        }
        let mut ages: Vec<(u64, usize)> = self
            .read
            .keys()
            .filter(|page| !wanted.contains(page))
            .map(|page| (self.wanted_at.get(page).copied().unwrap_or(0), *page))
            .collect();
        ages.sort_unstable();
        let room = keep.saturating_sub(wanted.len());
        let dropping = ages.len().saturating_sub(room);
        let mut ages = ages.into_iter();
        for (_, page) in ages.by_ref().take(dropping) {
            self.read.remove(&page);
            self.wanted_at.remove(&page);
        }
        for (_, page) in ages {
            if self.held_bytes() <= bytes {
                break;
            }
            self.read.remove(&page);
            self.wanted_at.remove(&page);
        }
    }

    #[must_use]
    pub fn held_bytes(&self) -> usize {
        self.read.values().map(|leaf| leaf.view.footprint()).sum()
    }

    #[must_use]
    pub fn held_pages(&self) -> usize {
        self.read.len()
    }

    #[must_use]
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    #[must_use]
    pub fn credential(&self) -> &[u8] {
        &self.credential
    }

    #[must_use]
    pub const fn status(&self) -> &Message {
        &self.status
    }

    pub fn say(&mut self, sentence: Message) {
        self.status = sentence;
    }

    #[must_use]
    pub fn fonts(&self) -> Option<Arc<dyn pdf_content::FontProvider>> {
        self.fonts.clone()
    }

    pub fn read_block_by_glyphs(&mut self, page: usize, block: usize) -> bool {
        let Some(anchors) = self
            .leaf(page)
            .and_then(|leaf| leaf.overlay.blocks.get(block))
            .map(|owner| owner.anchors.clone())
        else {
            return false;
        };
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        let Ok(view) = session.page(page) else {
            return false;
        };
        let atoms: Vec<usize> = view
            .graph
            .atoms
            .iter()
            .enumerate()
            .filter(|(_, atom)| {
                let anchor = SourceAnchor::of(&atom.id).encode();
                anchors.contains(&anchor)
            })
            .map(|(at, _)| at)
            .collect();
        if atoms.is_empty() || !session.reads_by_glyphs(page, &atoms).unwrap_or(false) {
            return false;
        }
        if !session.read_by_glyphs(page, &atoms).unwrap_or(false) {
            return false;
        }
        let family = session.replacement_family(page, &atoms).ok().flatten();
        let Ok(view) = session.page_for_display(page) else {
            return false;
        };
        self.read.remove(&page);
        self.adopt_page(page, view);
        if let Some(family) = family {
            self.set_block_family(page, block, family);
        }
        true
    }

    fn set_block_family(&mut self, page: usize, block: usize, family: String) {
        let Some(end) = self.leaf(page).and_then(|leaf| {
            let rows = &leaf.overlay.blocks.get(block)?.lines;
            let last = *rows.last()?;
            let offset = leaf
                .overlay
                .carets
                .iter()
                .filter(|stop| stop.line == last)
                .map(|stop| stop.offset)
                .max()?;
            Some((rows.len() - 1, offset))
        }) else {
            return;
        };
        let applied = self.here(Step::Style {
            page,
            block,
            range: BlockRange::Between {
                from: (0, 0),
                to: end,
            },
            style: pdf_edit::TextStyle {
                family: Some(family.clone()),
                ..pdf_edit::TextStyle::default()
            },
        });
        self.status = match applied {
            Applied::Changed { .. } => Message::Done(Done::SetInReadableFace { family }),
            Applied::Refused(reason) => Message::Refused(Refusal::FaceForReadingRefused {
                family,
                why: Box::new(reason),
            }),
            Applied::Unchanged => return,
        };
    }

    #[must_use]
    pub fn editing_restricted(&self) -> bool {
        self.restricted
            && self
                .session
                .as_ref()
                .is_none_or(|session| session.restrictions() == pdf_edit::Restrictions::Respect)
    }

    #[must_use]
    pub fn restrictions_set_aside(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.restrictions() != pdf_edit::Restrictions::Respect)
    }

    #[must_use]
    pub fn whole_block(&self, page: usize, block: usize) -> Option<BlockRange> {
        let reading = self.block_reading(page, block)?;
        let last = reading.lines.len().checked_sub(1)?;
        Some(BlockRange::Between {
            from: (0, 0),
            to: (last, reading.lines[last].clusters.len()),
        })
    }

    pub fn set_aside_restrictions(&mut self) -> bool {
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        session.set_aside_restrictions();
        if matches!(self.status, Message::Refused(Refusal::EditingRestricted)) {
            self.status = Message::Quiet;
        }
        true
    }

    fn restricted_refusal(&self, status: Message) -> Message {
        match status {
            Message::Refused(_) if self.editing_restricted() => {
                Message::Refused(Refusal::EditingRestricted)
            }
            other => other,
        }
    }

    #[must_use]
    pub const fn landed_caret(&self) -> Option<(usize, usize)> {
        self.landed_caret
    }

    #[must_use]
    pub const fn landed_anchor(&self) -> Option<(usize, usize)> {
        self.landed_anchor
    }

    #[must_use]
    pub fn text_just_written(&self) -> Option<&str> {
        self.wrote_text.as_deref()
    }

    #[must_use]
    pub const fn is_busy(&self) -> bool {
        self.session.is_none()
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.session.is_some() && self.frames.source_step(true).is_some()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.session.is_some() && self.frames.source_step(false).is_some()
    }

    #[must_use]
    pub fn source(&self) -> Option<&ByteStore> {
        self.session.as_ref().map(Session::source)
    }

    #[must_use]
    pub fn opened(&self) -> Option<(pdf_bytes::SourceId, usize)> {
        self.session
            .as_ref()
            .map(|session| session.history().opened())
    }

    #[must_use]
    pub fn last_region(&self) -> Option<[f64; 4]> {
        self.session.as_ref()?.history().last_region()
    }

    pub fn export(&mut self) -> Result<Export, String> {
        let session = self
            .session
            .as_mut()
            .ok_or("an edit is still running; nothing is saved while it is")?;
        let bytes = session.source().clone();
        let mut placements = 0;
        let pages: Vec<usize> = self.seen.iter().copied().collect();
        for page in pages {
            let showing = session
                .page(page)
                .map_err(|error| format!("page {} no longer interprets: {error}", page + 1))?;
            let reopened = pdf_session::interpret_page_fully(
                &bytes,
                page,
                &self.credential,
                None,
                pdf_cli::font_provider(),
            )
            .map_err(|error| format!("the saved bytes do not reopen: {error}"))?;
            let (before, after) = (
                pdf_paint::glyph_placement_signature(&showing.graph),
                pdf_paint::glyph_placement_signature(&reopened.graph),
            );
            if before != after {
                return Err(format!(
                    "the saved bytes paint page {} differently: {} glyph placements against {}",
                    page + 1,
                    after.len(),
                    before.len()
                ));
            }
            placements += before.len();
        }
        Ok(Export {
            bytes: bytes.to_vec(),
            placements,
        })
    }

    pub fn move_run(&mut self, page: usize, anchor: &str, dx: f64, dy: f64) -> Applied {
        self.here(Step::Move {
            page,
            anchor: anchor.to_owned(),
            dx,
            dy,
        })
    }

    pub fn move_block(&mut self, page: usize, anchors: &[String], dx: f64, dy: f64) -> Applied {
        self.here(Step::MoveBlock {
            page,
            anchors: anchors.to_vec(),
            block: None,
            group: false,
            dx,
            dy,
        })
    }

    pub fn move_group(
        &mut self,
        page: usize,
        anchors: &[String],
        objects: &[String],
        (dx, dy): (f64, f64),
    ) -> Applied {
        self.here(Step::MoveGroup {
            page,
            anchors: anchors.to_vec(),
            objects: objects.to_vec(),
            dx,
            dy,
        })
    }

    #[must_use]
    pub fn begin_move_group(
        &mut self,
        page: usize,
        anchors: &[String],
        objects: &[String],
        (dx, dy): (f64, f64),
    ) -> Option<EditJob> {
        self.begin(Step::MoveGroup {
            page,
            anchors: anchors.to_vec(),
            objects: objects.to_vec(),
            dx,
            dy,
        })
    }

    pub fn copy_objects(&self, page: usize, anchors: &[String]) -> Result<Copied, String> {
        let named = decoded_anchors(anchors)?;
        let Some(leaf) = self.read.get(&page) else {
            return Err("that page has not been read".to_owned());
        };
        let from = self
            .session
            .as_ref()
            .ok_or_else(|| "an edit is running".to_owned())?
            .source()
            .id();
        pdf_edit::copy_from(&leaf.view.graph, &named, from).map_err(|error| error.to_string())
    }

    #[must_use]
    pub fn begin_paste(
        &mut self,
        page: usize,
        copied: Copied,
        (dx, dy): (f64, f64),
        elsewhere: Option<pdf_bytes::ByteStore>,
    ) -> Option<EditJob> {
        self.begin(Step::Paste {
            page,
            copied,
            dx,
            dy,
            elsewhere,
        })
    }

    pub fn paste_objects(
        &mut self,
        page: usize,
        copied: Copied,
        (dx, dy): (f64, f64),
        elsewhere: Option<pdf_bytes::ByteStore>,
    ) -> Applied {
        self.here(Step::Paste {
            page,
            copied,
            dx,
            dy,
            elsewhere,
        })
    }

    pub fn move_text_block(&mut self, page: usize, block: usize, dx: f64, dy: f64) -> Applied {
        match self.block_anchors(page, block) {
            Some(anchors) => self.here(Step::MoveBlock {
                page,
                anchors,
                block: Some(block),
                group: false,
                dx,
                dy,
            }),
            None => self.refuse(Refusal::BlockNotRead),
        }
    }

    #[must_use]
    pub fn begin_move_text_block(
        &mut self,
        page: usize,
        block: usize,
        dx: f64,
        dy: f64,
    ) -> Option<EditJob> {
        let anchors = self.block_anchors(page, block)?;
        self.begin(Step::MoveBlock {
            page,
            anchors,
            block: Some(block),
            group: false,
            dx,
            dy,
        })
    }

    #[must_use]
    pub fn begin_move_block_pointed(
        &mut self,
        page: usize,
        anchors: &[String],
        block: Option<usize>,
        (dx, dy): (f64, f64),
    ) -> Option<EditJob> {
        self.begin(Step::MoveBlock {
            page,
            anchors: anchors.to_vec(),
            block,
            group: false,
            dx,
            dy,
        })
    }

    fn block_anchors(&self, page: usize, block: usize) -> Option<Vec<String>> {
        Some(
            self.read
                .get(&page)?
                .overlay
                .blocks
                .get(block)?
                .anchors
                .clone(),
        )
    }

    #[must_use]
    pub fn begin_shape(
        &mut self,
        page: usize,
        anchor: &str,
        matrix: Matrix,
        about: (f64, f64),
    ) -> Option<EditJob> {
        self.begin(Step::Shape {
            page,
            anchor: anchor.to_owned(),
            matrix,
            about,
        })
    }

    #[must_use]
    pub fn begin_shape_block(
        &mut self,
        page: usize,
        anchors: &[String],
        matrix: Matrix,
        about: (f64, f64),
    ) -> Option<EditJob> {
        self.begin(Step::ShapeBlock {
            page,
            anchors: anchors.to_vec(),
            matrix,
            about,
        })
    }

    #[must_use]
    pub fn begin_set_size(
        &mut self,
        page: usize,
        anchors: &[String],
        points: f64,
    ) -> Option<EditJob> {
        self.begin(Step::SetSize {
            page,
            anchors: anchors.to_vec(),
            points,
        })
    }

    #[must_use]
    pub fn begin_set_angles(
        &mut self,
        page: usize,
        anchors: &[String],
        turn: Option<f64>,
        slant: Option<f64>,
    ) -> Option<EditJob> {
        self.begin(Step::SetAngles {
            page,
            anchors: anchors.to_vec(),
            turn,
            slant,
        })
    }

    #[must_use]
    pub fn begin_place(&mut self, page: usize, anchor: &str, dx: f64, dy: f64) -> Option<EditJob> {
        self.begin(Step::Place {
            page,
            anchor: anchor.to_owned(),
            dx,
            dy,
        })
    }

    pub fn place(&mut self, page: usize, anchor: &str, dx: f64, dy: f64) -> Applied {
        self.here(Step::Place {
            page,
            anchor: anchor.to_owned(),
            dx,
            dy,
        })
    }

    pub fn delete(&mut self, page: usize, line: usize, from: usize, to: usize) -> Applied {
        self.type_text(page, line, from, to, "")
    }

    pub fn type_text(
        &mut self,
        page: usize,
        line: usize,
        from: usize,
        to: usize,
        text: &str,
    ) -> Applied {
        match self.range_on_row(page, line, from, to) {
            Some((block, range)) => self.edit(page, block, range, text),
            None => self.refuse(Refusal::RowNotRead),
        }
    }

    pub fn edit(&mut self, page: usize, block: usize, range: BlockRange, text: &str) -> Applied {
        self.here(Step::Type {
            page,
            block,
            range,
            text: text.to_owned(),
            style: None,
        })
    }

    pub fn edit_in_style(
        &mut self,
        page: usize,
        block: usize,
        range: BlockRange,
        text: &str,
        style: pdf_edit::TextStyle,
    ) -> Applied {
        self.here(Step::Type {
            page,
            block,
            range,
            text: text.to_owned(),
            style: Some(style),
        })
    }

    pub fn style(
        &mut self,
        page: usize,
        block: usize,
        range: BlockRange,
        style: pdf_edit::TextStyle,
    ) -> Applied {
        self.here(Step::Style {
            page,
            block,
            range,
            style,
        })
    }

    #[must_use]
    pub fn begin_style(
        &mut self,
        page: usize,
        block: usize,
        range: BlockRange,
        style: pdf_edit::TextStyle,
    ) -> Option<EditJob> {
        self.begin(Step::Style {
            page,
            block,
            range,
            style,
        })
    }

    #[must_use]
    pub fn begin_flow_round(&mut self, page: usize, over: [f64; 4]) -> Option<EditJob> {
        let blocks = self.blocks_reached_by(page, over);
        if blocks.is_empty() {
            self.status = Message::NothingInTheWay;
            return None;
        }
        self.begin(Step::FlowRound { page, blocks })
    }

    #[must_use]
    pub fn begin_resize_frame(
        &mut self,
        page: usize,
        block: usize,
        started: [f64; 4],
    ) -> Option<EditJob> {
        let now = self.frame_boxes(page).get(block).copied()?;
        if !crate::view::frame_width_changed(started, now) {
            self.finish_frame_resize(page, block, started);
            let at = self.frame_boxes(page).get(block).copied().unwrap_or(now);
            self.status = Message::FrameDeclared {
                wide: at[2] - at[0],
                high: at[3] - at[1],
                relaid: false,
            };
            return None;
        }
        self.begin(Step::ResizeFrame {
            page,
            block,
            started,
        })
    }

    #[must_use]
    pub fn blocks_reached_by(&self, page: usize, over: [f64; 4]) -> Vec<usize> {
        self.frames
            .page(page)
            .iter()
            .enumerate()
            .filter(|(_, frame)| {
                frame[0] < over[2] && frame[2] > over[0] && frame[1] < over[3] && frame[3] > over[1]
            })
            .map(|(block, _)| block)
            .collect()
    }

    #[must_use]
    pub fn begin_edit(
        &mut self,
        page: usize,
        block: usize,
        range: BlockRange,
        text: &str,
    ) -> Option<EditJob> {
        self.begin(Step::Type {
            page,
            block,
            range,
            text: text.to_owned(),
            style: None,
        })
    }

    #[must_use]
    pub fn begin_edit_in_style(
        &mut self,
        page: usize,
        block: usize,
        range: BlockRange,
        text: &str,
        style: pdf_edit::TextStyle,
    ) -> Option<EditJob> {
        self.begin(Step::Type {
            page,
            block,
            range,
            text: text.to_owned(),
            style: Some(style),
        })
    }

    pub fn live_block(
        &mut self,
        page: usize,
        block: usize,
        range: BlockRange,
        text: &str,
    ) -> Result<pdf_edit::LiveBlock, String> {
        if let Some(session) = self.session.as_ref() {
            session.begin_timing();
        }
        let laid = self
            .typed_command(page, block, range, text)
            .and_then(|command| {
                let session = self.session.as_mut().ok_or("an edit is running")?;
                session
                    .lay_out_live(&command)
                    .map_err(|error| error.to_string())
            });
        if let Some(session) = self.session.as_ref() {
            self.last_edit = session.end_timing();
        }
        laid
    }

    fn typed_command(
        &mut self,
        page: usize,
        block: usize,
        range: BlockRange,
        text: &str,
    ) -> Result<Command, String> {
        let session = self.session.as_mut().ok_or("an edit is running")?;
        let view = session.page(page).map_err(|error| error.to_string())?;
        let rows = block_rows(&view, block).ok_or("row has no block owner")?;
        if rows.iter().all(Vec::is_empty) {
            return Err("the block is empty".to_owned());
        }
        let frame = *self
            .frames
            .page(page)
            .get(block)
            .ok_or("no frame for this block")?;
        let user = frame_edges_in_user_space(&view, frame).map_err(|error| error.to_string())?;
        Ok(block_command(
            page,
            (
                rows,
                user,
                self.frames.edges(page, block),
                self.frames.breaks(page, block),
            ),
            range,
            (text, None, None),
            self.frames.is_declared(page, block),
            paragraph_of(&self.paragraphs, page, block),
        ))
    }

    pub fn block_refusal(
        &mut self,
        page: usize,
        block: usize,
        range: BlockRange,
        text: &str,
    ) -> Option<String> {
        let session = self.session.as_mut()?;
        let view = session.page(page).ok()?;
        let rows = block_rows(&view, block)?;
        let frame = *self.frames.page(page).get(block)?;
        let user = frame_edges_in_user_space(&view, frame).ok()?;
        match session.plan(&Command::RewriteBlock {
            page_index: page,
            rows,
            frame: user,
            edges: self.frames.edges(page, block),
            breaks: self.frames.breaks(page, block),
            frame_declared: self.frames.is_declared(page, block),
            range,
            text: text.to_owned(),
            paragraph: paragraph_of(&self.paragraphs, page, block),
        }) {
            Ok(_) => None,
            Err(error) => Some(error.to_string()),
        }
    }

    #[must_use]
    pub fn begin_type(
        &mut self,
        page: usize,
        line: usize,
        from: usize,
        to: usize,
        text: &str,
    ) -> Option<EditJob> {
        let (block, range) = self.range_on_row(page, line, from, to)?;
        self.begin_edit(page, block, range, text)
    }

    #[must_use]
    pub fn begin_replace(
        &mut self,
        page: usize,
        from: (usize, usize),
        to: (usize, usize),
        text: &str,
    ) -> Option<EditJob> {
        let (block, range) = self.range_across_rows(page, from, to)?;
        self.begin_edit(page, block, range, text)
    }

    fn range_across_rows(
        &self,
        page: usize,
        from: (usize, usize),
        to: (usize, usize),
    ) -> Option<(usize, BlockRange)> {
        let overlay = &self.read.get(&page)?.overlay;
        let block = overlay
            .blocks
            .iter()
            .position(|block| block.lines.contains(&from.0) && block.lines.contains(&to.0))?;
        let row_of = |line: usize| {
            overlay.blocks[block]
                .lines
                .iter()
                .position(|held| *held == line)
        };
        Some((
            block,
            BlockRange::Between {
                from: (row_of(from.0)?, from.1),
                to: (row_of(to.0)?, to.1),
            },
        ))
    }

    fn range_on_row(
        &self,
        page: usize,
        line: usize,
        from: usize,
        to: usize,
    ) -> Option<(usize, BlockRange)> {
        let overlay = &self.read.get(&page)?.overlay;
        let block = overlay
            .blocks
            .iter()
            .position(|block| block.lines.contains(&line))?;
        let row = overlay.blocks[block]
            .lines
            .iter()
            .position(|held| *held == line)?;
        Some((
            block,
            BlockRange::Between {
                from: (row, from),
                to: (row, to),
            },
        ))
    }

    #[must_use]
    pub fn copy_text(
        &self,
        page: usize,
        block: usize,
        from: (usize, usize),
        to: (usize, usize),
    ) -> Option<String> {
        let leaf = self.read.get(&page)?;
        let view = &leaf.view;
        let frame = *self.frames.page(page).get(block)?;
        if let Ok(user) = frame_edges_in_user_space(view, frame)
            && let Some(rows) = block_rows(view, block)
            && let Ok(reading) = pdf_edit::read_block(
                &view.program,
                &view.graph,
                &rows,
                user,
                self.frames.edges(page, block),
                self.frames.breaks(page, block).as_ref(),
            )
        {
            return reading.text_between(from, to);
        }
        let overlay = &leaf.overlay;
        let lines = &overlay.blocks.get(block)?.lines;
        let (first, last) = if from <= to { (from, to) } else { (to, from) };
        let mut text = String::new();
        for row in first.0..=last.0 {
            let line = *lines.get(row)?;
            let start = if row == first.0 { first.1 } else { 0 };
            let end = if row == last.0 { last.1 } else { usize::MAX };
            for cluster in overlay.clusters.iter().filter(|cluster| {
                cluster.line == line
                    && cluster.index_in_line >= start
                    && cluster.index_in_line < end
            }) {
                text.push_str(cluster.text.as_deref().unwrap_or(""));
            }
            if row != last.0 {
                text.push('\n');
            }
        }
        Some(text)
    }

    #[must_use]
    pub fn begin_delete_block(&mut self, page: usize, anchors: &[String]) -> Option<EditJob> {
        self.begin(Step::DeleteBlock {
            page,
            anchors: anchors.to_vec(),
        })
    }

    #[must_use]
    pub fn begin_delete_group(
        &mut self,
        page: usize,
        anchors: &[String],
        objects: &[String],
    ) -> Option<EditJob> {
        self.begin(Step::DeleteGroup {
            page,
            anchors: anchors.to_vec(),
            objects: objects.to_vec(),
        })
    }

    pub fn delete_the_group(
        &mut self,
        page: usize,
        anchors: &[String],
        objects: &[String],
    ) -> Applied {
        self.here(Step::DeleteGroup {
            page,
            anchors: anchors.to_vec(),
            objects: objects.to_vec(),
        })
    }

    #[must_use]
    pub fn begin_describe(&mut self, edit: pdf_edit::info::InfoEdit) -> Option<EditJob> {
        self.begin(Step::Describe { edit })
    }

    #[must_use]
    pub fn facts(&self) -> Option<pdf_edit::info::DocumentFacts> {
        let source = self.source()?;
        pdf_edit::info::document_facts(source, self.credential()).ok()
    }

    pub fn describe(&mut self, edit: &pdf_edit::info::InfoEdit) -> Applied {
        self.here(Step::Describe { edit: edit.clone() })
    }

    #[must_use]
    pub fn begin_rotate_pages(&mut self, pages: &[usize], quarter_turns: i32) -> Option<EditJob> {
        self.begin(Step::RotatePages {
            pages: pages.to_vec(),
            quarter_turns,
        })
    }

    pub fn rotate_pages(&mut self, pages: &[usize], quarter_turns: i32) -> Applied {
        self.here(Step::RotatePages {
            pages: pages.to_vec(),
            quarter_turns,
        })
    }

    #[must_use]
    pub fn begin_remove_pages(&mut self, pages: &[usize]) -> Option<EditJob> {
        self.begin(Step::RemovePages {
            pages: pages.to_vec(),
        })
    }

    pub fn remove_pages(&mut self, pages: &[usize]) -> Applied {
        self.here(Step::RemovePages {
            pages: pages.to_vec(),
        })
    }

    #[must_use]
    pub fn begin_move_pages(&mut self, pages: &[usize], to: usize) -> Option<EditJob> {
        self.begin(Step::MovePages {
            pages: pages.to_vec(),
            to,
        })
    }

    pub fn move_pages(&mut self, pages: &[usize], to: usize) -> Applied {
        self.here(Step::MovePages {
            pages: pages.to_vec(),
            to,
        })
    }

    #[must_use]
    pub fn begin_insert_pages(
        &mut self,
        (beside, before): (usize, bool),
        document: Arc<[u8]>,
        pages: &[usize],
    ) -> Option<EditJob> {
        self.begin(Step::InsertPages {
            beside,
            before,
            document,
            pages: pages.to_vec(),
        })
    }

    pub fn insert_pages(
        &mut self,
        (beside, before): (usize, bool),
        document: Arc<[u8]>,
        pages: &[usize],
    ) -> Applied {
        self.here(Step::InsertPages {
            beside,
            before,
            document,
            pages: pages.to_vec(),
        })
    }

    #[must_use]
    pub const fn pages_changed(&self) -> Option<&PageChange> {
        self.pages_changed.as_ref()
    }

    #[must_use]
    pub const fn pages_redrawn(&self) -> bool {
        self.pages_redrawn
    }

    #[must_use]
    pub const fn redraw_reason(&self) -> Option<&'static str> {
        self.redraw_reason
    }

    #[must_use]
    pub fn begin_add_page(
        &mut self,
        beside: usize,
        before: bool,
        size: [f64; 2],
    ) -> Option<EditJob> {
        self.begin(Step::AddPage {
            beside,
            before,
            size,
        })
    }

    pub fn add_page(&mut self, beside: usize, before: bool, size: [f64; 2]) -> Applied {
        self.here(Step::AddPage {
            beside,
            before,
            size,
        })
    }

    #[must_use]
    pub fn begin_place_text(
        &mut self,
        page: usize,
        frame: [f64; 4],
        text: &str,
        style: &NewTextStyle,
    ) -> Option<EditJob> {
        self.begin(Step::PlaceText {
            page,
            frame,
            text: text.to_owned(),
            family: style.family.clone(),
            size: style.size,
            bold: style.bold,
            italic: style.italic,
            fill: style.fill,
            paragraph: style.paragraph,
        })
    }

    pub fn place_text(
        &mut self,
        page: usize,
        frame: [f64; 4],
        text: &str,
        style: &NewTextStyle,
    ) -> Applied {
        self.here(Step::PlaceText {
            page,
            frame,
            text: text.to_owned(),
            family: style.family.clone(),
            size: style.size,
            bold: style.bold,
            italic: style.italic,
            fill: style.fill,
            paragraph: style.paragraph,
        })
    }

    #[must_use]
    pub fn begin_place_image(
        &mut self,
        page: usize,
        pixels: [f64; 4],
        file: Arc<[u8]>,
    ) -> Option<EditJob> {
        self.begin_place_images(page, vec![(pixels, file)])
    }

    #[must_use]
    pub fn begin_place_images(
        &mut self,
        page: usize,
        pictures: Vec<([f64; 4], Arc<[u8]>)>,
    ) -> Option<EditJob> {
        let pictures = self.placements(page, pictures)?;
        self.begin(Step::PlaceImage { page, pictures })
    }

    pub fn place_image(&mut self, page: usize, pixels: [f64; 4], file: Arc<[u8]>) -> Applied {
        self.place_images(page, vec![(pixels, file)])
    }

    pub fn place_images(&mut self, page: usize, pictures: Vec<([f64; 4], Arc<[u8]>)>) -> Applied {
        let Some(pictures) = self.placements(page, pictures) else {
            return Applied::Unchanged;
        };
        self.here(Step::PlaceImage { page, pictures })
    }

    fn placements(
        &self,
        page: usize,
        pictures: Vec<([f64; 4], Arc<[u8]>)>,
    ) -> Option<Vec<(pdf_paint::Matrix, Arc<[u8]>)>> {
        if pictures.is_empty() {
            return None;
        }
        pictures
            .into_iter()
            .map(|(pixels, file)| Some((self.placement_in_user_space(page, pixels)?, file)))
            .collect()
    }

    #[must_use]
    pub fn placement_in_user_space(
        &self,
        page: usize,
        [x0, y0, x1, y1]: [f64; 4],
    ) -> Option<pdf_paint::Matrix> {
        let view = &self.read.get(&page)?.view;
        let device = pdf_render::DeviceTransform::for_page(
            &view.program.geometry,
            OVERLAY_SCALE,
            pdf_render::RenderLimits::default(),
        )
        .ok()?;
        let on_screen = pdf_paint::Matrix {
            a: x1 - x0,
            b: 0.0,
            c: 0.0,
            d: y0 - y1,
            e: x0,
            f: y1,
        };
        Some(device.matrix.inverse()?.multiply(on_screen))
    }

    #[must_use]
    pub fn begin_draw_path(
        &mut self,
        page: usize,
        steps: Vec<pdf_edit::PenStep>,
        (stroke, fill): (Option<pdf_edit::PenStroke>, Option<[f64; 3]>),
        (closed, drew): (bool, Drew),
    ) -> Option<EditJob> {
        self.begin(Step::DrawLine {
            page,
            steps,
            stroke,
            fill,
            closed,
            drew,
        })
    }

    pub fn draw_path(
        &mut self,
        page: usize,
        steps: Vec<pdf_edit::PenStep>,
        (stroke, fill): (Option<pdf_edit::PenStroke>, Option<[f64; 3]>),
        (closed, drew): (bool, Drew),
    ) -> Applied {
        self.here(Step::DrawLine {
            page,
            steps,
            stroke,
            fill,
            closed,
            drew,
        })
    }

    #[must_use]
    pub fn steps_in_user_space(
        &self,
        page: usize,
        steps: &[pdf_edit::PenStep],
    ) -> Option<Vec<pdf_edit::PenStep>> {
        let inverse = self.overlay_device(page)?.matrix.inverse()?;
        let carried = |(x, y): (f64, f64)| {
            let point = inverse.transform(pdf_paint::Point { x, y });
            (Self::thousandths(point.x), Self::thousandths(point.y))
        };
        Some(
            steps
                .iter()
                .map(|step| match step {
                    pdf_edit::PenStep::Move(point) => pdf_edit::PenStep::Move(carried(*point)),
                    pdf_edit::PenStep::Line(point) => pdf_edit::PenStep::Line(carried(*point)),
                    pdf_edit::PenStep::Curve(one, other, end) => {
                        pdf_edit::PenStep::Curve(carried(*one), carried(*other), carried(*end))
                    }
                })
                .collect(),
        )
    }

    #[must_use]
    pub fn length_in_user_space(&self, page: usize, pixels: f64) -> Option<f64> {
        let matrix = self.overlay_device(page)?.matrix;
        let scale = matrix
            .a
            .mul_add(matrix.d, -(matrix.b * matrix.c))
            .abs()
            .sqrt();
        (scale > 0.0).then(|| Self::thousandths(pixels / scale))
    }

    #[must_use]
    fn thousandths(value: f64) -> f64 {
        (value * 1000.0).round() / 1000.0
    }

    fn overlay_device(&self, page: usize) -> Option<pdf_render::DeviceTransform> {
        let view = &self.read.get(&page)?.view;
        pdf_render::DeviceTransform::for_page(
            &view.program.geometry,
            OVERLAY_SCALE,
            pdf_render::RenderLimits::default(),
        )
        .ok()
    }

    #[must_use]
    pub fn begin_remove_object(&mut self, page: usize, anchor: &str) -> Option<EditJob> {
        self.begin(Step::RemoveObject {
            page,
            anchor: anchor.to_owned(),
            rubbing: false,
        })
    }

    #[must_use]
    pub fn begin_remove_drawing(&mut self, page: usize, anchor: &str) -> Option<EditJob> {
        self.begin(Step::RemoveObject {
            page,
            anchor: anchor.to_owned(),
            rubbing: true,
        })
    }

    #[must_use]
    pub fn begin_reorder_objects(
        &mut self,
        page: usize,
        anchors: &[String],
        order: pdf_edit::Stacking,
    ) -> Option<EditJob> {
        self.begin(Step::ReorderObjects {
            page,
            anchors: anchors.to_vec(),
            order,
        })
    }

    pub fn reorder_objects(
        &mut self,
        page: usize,
        anchors: &[String],
        order: pdf_edit::Stacking,
    ) -> Applied {
        self.here(Step::ReorderObjects {
            page,
            anchors: anchors.to_vec(),
            order,
        })
    }

    pub fn remove_object(&mut self, page: usize, anchor: &str) -> Applied {
        self.here(Step::RemoveObject {
            page,
            anchor: anchor.to_owned(),
            rubbing: false,
        })
    }

    pub fn delete_block(&mut self, page: usize, anchors: &[String]) -> Applied {
        self.here(Step::DeleteBlock {
            page,
            anchors: anchors.to_vec(),
        })
    }

    pub fn undo(&mut self) -> Applied {
        self.here(Step::Undo)
    }

    pub fn redo(&mut self) -> Applied {
        self.here(Step::Redo)
    }

    #[must_use]
    pub fn begin_move(&mut self, page: usize, anchor: &str, dx: f64, dy: f64) -> Option<EditJob> {
        self.begin(Step::Move {
            page,
            anchor: anchor.to_owned(),
            dx,
            dy,
        })
    }

    #[must_use]
    pub fn begin_move_block(
        &mut self,
        page: usize,
        anchors: &[String],
        dx: f64,
        dy: f64,
    ) -> Option<EditJob> {
        self.begin(Step::MoveBlock {
            page,
            anchors: anchors.to_vec(),
            block: None,
            group: false,
            dx,
            dy,
        })
    }

    #[must_use]
    pub fn begin_move_block_as_group(
        &mut self,
        page: usize,
        anchors: &[String],
        dx: f64,
        dy: f64,
    ) -> Option<EditJob> {
        self.begin(Step::MoveBlock {
            page,
            anchors: anchors.to_vec(),
            block: None,
            group: true,
            dx,
            dy,
        })
    }

    #[must_use]
    pub fn begin_delete(
        &mut self,
        page: usize,
        line: usize,
        from: usize,
        to: usize,
    ) -> Option<EditJob> {
        self.begin_type(page, line, from, to, "")
    }

    #[must_use]
    pub fn begin_undo(&mut self) -> Option<EditJob> {
        self.begin(Step::Undo)
    }

    #[must_use]
    pub fn begin_redo(&mut self) -> Option<EditJob> {
        self.begin(Step::Redo)
    }

    pub const fn keep_a_ledger(&mut self, keep: bool) {
        self.tracing = keep;
    }

    #[must_use]
    pub fn revision(&self) -> Option<u64> {
        Some(self.session.as_ref()?.revision().get())
    }

    #[must_use]
    pub fn bytes_now(&self) -> Option<std::sync::Arc<[u8]>> {
        Some(std::sync::Arc::from(
            self.session.as_ref()?.source().to_vec(),
        ))
    }

    pub fn take_records(&mut self) -> Vec<crate::ledger::CommandRecord> {
        self.ledger.take()
    }

    #[must_use]
    pub const fn forgotten_records(&self) -> u64 {
        self.ledger.forgotten()
    }

    fn document_state(&self) -> Option<crate::ledger::DocumentState> {
        let session = self.session.as_ref()?;
        let bytes = &session.source().to_vec();
        Some(crate::ledger::DocumentState {
            revision: session.revision().get(),
            bytes: bytes.len(),
            digest: pdf_content::sha256_hex(bytes),
            can_undo: self.frames.source_step(true).is_some(),
            can_redo: self.frames.source_step(false).is_some(),
        })
    }

    fn begin(&mut self, step: Step) -> Option<EditJob> {
        if self.tracing {
            if let Some(before) = self.document_state() {
                self.pending = Some(Pending {
                    request: self.ledger.next_request(),
                    command: step.name(),
                    arguments: step.arguments(),
                    before,
                });
            } else {
                let request = self.ledger.next_request();
                self.ledger.push(crate::ledger::CommandRecord {
                    request,
                    command: step.name().to_owned(),
                    arguments: step.arguments(),
                    before: crate::ledger::DocumentState {
                        revision: 0,
                        bytes: 0,
                        digest: String::new(),
                        can_undo: false,
                        can_redo: false,
                    },
                    after: crate::ledger::DocumentState {
                        revision: 0,
                        bytes: 0,
                        digest: String::new(),
                        can_undo: false,
                        can_redo: false,
                    },
                    outcome: crate::ledger::Outcome::Dropped(
                        "an edit was already running".to_owned(),
                    ),
                    status: self.status.to_string(),
                });
            }
        }
        let move_frame = match &step {
            Step::MoveBlock {
                page,
                anchors,
                block,
                dx,
                dy,
                ..
            } => self.read.get(page).and_then(|leaf| {
                let named = |index: &usize| {
                    leaf.overlay
                        .blocks
                        .get(*index)
                        .is_some_and(|owner| owner.anchors == *anchors)
                };
                let block = block.filter(named).or_else(|| {
                    leaf.overlay
                        .blocks
                        .iter()
                        .position(|owner| owner.anchors == *anchors)
                })?;
                let (dx, dy) = overlay_device(&leaf.view).map_or((*dx, *dy), |device| {
                    into_turned_pixels(device, leaf.overlay.blocks[block].turn, (*dx, *dy))
                });
                Some((block, dx, dy))
            }),
            _ => None,
        };
        let group_frames = match &step {
            Step::MoveBlock {
                page,
                anchors,
                dx,
                dy,
                ..
            } if move_frame.is_none() => self.frames_touching(*page, anchors, (*dx, *dy)),
            Step::MoveGroup {
                page,
                anchors,
                dx,
                dy,
                ..
            } => self.frames_touching(*page, anchors, (*dx, *dy)),
            _ => Vec::new(),
        };
        let whole_block = match &step {
            Step::SetSize { page, anchors, .. } => self
                .read
                .get(page)
                .and_then(|leaf| whole_block(&leaf.overlay, anchors)),
            _ => None,
        };
        Some(EditJob {
            session: self.session.take()?,
            wrote_text: None,
            step,
            frames: std::mem::take(&mut self.frames),
            paragraphs: std::mem::take(&mut self.paragraphs),
            flowed: std::mem::take(&mut self.flowed),
            move_frame,
            group_frames,
            resize_frame: None,
            whole_block,
            laid: None,
        })
    }

    fn frames_touching(
        &self,
        page: usize,
        anchors: &[String],
        (dx, dy): (f64, f64),
    ) -> Vec<(usize, f64, f64)> {
        let Some(leaf) = self.read.get(&page) else {
            return Vec::new();
        };
        let device = overlay_device(&leaf.view);
        leaf.overlay
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, owner)| owner.anchors.iter().any(|anchor| anchors.contains(anchor)))
            .map(|(block, owner)| {
                let (dx, dy) = device.map_or((dx, dy), |device| {
                    into_turned_pixels(device, owner.turn, (dx, dy))
                });
                (block, dx, dy)
            })
            .collect()
    }

    fn here(&mut self, step: Step) -> Applied {
        let Some(job) = self.begin(step) else {
            return self.refuse(Refusal::EditAlreadyRunning);
        };
        self.adopt(job.run())
    }

    pub fn set_clock(&mut self, clock: fn() -> f64) {
        if let Some(session) = self.session.as_mut() {
            session.set_clock(clock);
        }
    }

    pub const fn take_last_edit(&mut self) -> Option<pdf_session::stages::Stages> {
        self.last_edit.take()
    }

    pub fn adopt(&mut self, outcome: EditOutcome) -> Applied {
        let EditOutcome {
            session,
            applied,
            status,
            frames,
            paragraphs,
            flowed,
            caret,
            anchor,
            wrote_text,
            laid,
            timed,
        } = outcome;
        self.last_edit = timed;
        self.paragraphs = paragraphs;
        self.flowed = flowed;
        self.wrote_text = wrote_text.filter(|_| matches!(applied, Applied::Changed { .. }));
        self.pages_changed = None;
        self.pages_redrawn = false;
        self.redraw_reason = None;
        self.landed_caret = caret.filter(|_| matches!(applied, Applied::Changed { .. }));
        self.landed_anchor = anchor.filter(|_| matches!(applied, Applied::Changed { .. }));
        self.frames = frames;
        for (page, leaf) in &mut self.read {
            let device = overlay_device(&leaf.view);
            apply_frames(
                &mut Arc::make_mut(leaf).overlay,
                self.frames.page(*page),
                device,
            );
        }
        self.session = Some(session);
        self.status = self.restricted_refusal(status);
        if let Applied::Changed { page, .. } = applied {
            self.epoch += 1;
            let spread = self.session.as_ref().is_some_and(|session| {
                session.last_pages().is_some() || !session.last_spread().is_empty()
            });
            if spread {
                self.read.clear();
                self.wanted_at.clear();
            } else {
                self.read.remove(&page);
                self.wanted_at.remove(&page);
            }
            if let Applied::Changed { page, .. } = applied
                && let Some(session) = self.session.as_ref()
            {
                if let Some(grouping) = session.grouping(page) {
                    self.groupings.insert(page, grouping);
                } else {
                    self.groupings.remove(&page);
                }
                if let Some(view) = session.held_page(page) {
                    match laid {
                        Some((at, overlay)) if at == page => {
                            self.adopt_page_laid(page, view, overlay);
                        }
                        _ => self.adopt_page(page, view),
                    }
                }
            }
            if let Some(session) = self.session.as_ref()
                && let Ok(geometries) = session.page_geometries()
            {
                self.pages_changed = session.last_pages().cloned();
                self.redraw_reason = if self.pages_changed.is_some() {
                    Some("pages went in, out or elsewhere")
                } else if !laid_out_the_same(&geometries, &self.geometries) {
                    Some("a page is not the size it was")
                } else if !session.last_spread().is_empty() {
                    Some("the step was over several pages")
                } else {
                    None
                };
                self.pages_redrawn = self.redraw_reason.is_some();
                if self.pages_changed.is_some() {
                    self.groupings = session.groupings().clone();
                    self.seen.clear();
                    self.page_links.clear();
                    self.page_link_boxes.clear();
                    self.named_places = None;
                }
                self.strip = Strip::of(&geometries);
                self.geometries = geometries;
                self.links = links_of(session.source(), &self.credential);
                self.page_links.clear();
                self.page_link_boxes.clear();
                self.named_places = None;
                self.page_fields.clear();
                self.document_fields = None;
                self.outline = None;
            }
        }
        self.close_record(&applied);
        applied
    }

    pub fn abandon_record(&mut self, why: &str) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        self.ledger.push(crate::ledger::CommandRecord {
            request: pending.request,
            command: pending.command.to_owned(),
            arguments: pending.arguments,
            before: pending.before,
            after: crate::ledger::DocumentState {
                revision: 0,
                bytes: 0,
                digest: String::new(),
                can_undo: false,
                can_redo: false,
            },
            outcome: crate::ledger::Outcome::Dropped(why.to_owned()),
            status: self.status.to_string(),
        });
    }

    fn close_record(&mut self, applied: &Applied) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let Some(after) = self.document_state() else {
            return;
        };
        let outcome = match applied {
            Applied::Changed { page, region } => crate::ledger::Outcome::Changed {
                page: *page,
                region: region.map(|region| format!("{region:?}")),
            },
            Applied::Unchanged => crate::ledger::Outcome::Unchanged,
            Applied::Refused(reason) => crate::ledger::Outcome::Refused(reason.to_string()),
        };
        self.ledger.push(crate::ledger::CommandRecord {
            request: pending.request,
            command: pending.command.to_owned(),
            arguments: pending.arguments,
            before: pending.before,
            after,
            outcome,
            status: self.status.to_string(),
        });
    }

    fn refuse(&mut self, reason: Refusal) -> Applied {
        self.status = self.restricted_refusal(Message::Refused(reason.clone()));
        Applied::Refused(reason)
    }

    #[must_use]
    pub fn frame_boxes(&self, page: usize) -> &[[f64; 4]] {
        self.frames.page(page)
    }

    #[must_use]
    pub fn block_edges(&self, page: usize, block: usize) -> Edges {
        self.frames.edges(page, block)
    }

    #[must_use]
    pub fn block_breaks(&self, page: usize, block: usize) -> Breaks {
        self.frames.breaks(page, block)
    }

    #[must_use]
    pub fn block_pitch(&self, page: usize, block: usize) -> Option<f64> {
        self.block_reading(page, block).map(|reading| reading.pitch)
    }

    #[must_use]
    pub fn block_paragraph(&self, page: usize, block: usize) -> Option<(f64, pdf_edit::Alignment)> {
        let reading = self.block_reading(page, block)?;
        let set = self
            .paragraphs
            .get(&(page, block))
            .and_then(|had| had.alignment);
        Some((reading.pitch, set.unwrap_or(reading.alignment)))
    }

    #[must_use]
    pub fn paragraph_of(&self, page: usize, block: usize) -> pdf_edit::ParagraphLayout {
        paragraph_of(&self.paragraphs, page, block)
    }

    #[must_use]
    pub fn flows_round(&self, page: usize, block: usize) -> bool {
        self.paragraph_of(page, block).flow_round
    }

    pub fn set_alignment(&mut self, page: usize, block: usize, alignment: pdf_edit::Alignment) {
        self.paragraphs.entry((page, block)).or_default().alignment = Some(alignment);
    }

    pub fn set_flow_round(&mut self, page: usize, block: usize, round: bool) {
        if round && self.paragraph_of(page, block).alignment.is_none() {
            let read_as = self.block_paragraph(page, block).map(|(_, set)| set);
            self.paragraphs.entry((page, block)).or_default().alignment = read_as;
        }
        self.paragraphs.entry((page, block)).or_default().flow_round = round;
    }

    #[must_use]
    pub fn anything_stands_in(&self, page: usize, block: usize) -> bool {
        self.blocked_in(page, block)
            .is_some_and(|rows| !rows.is_empty())
    }

    #[must_use]
    pub fn free_bands(&self, page: usize, block: usize) -> Option<Vec<[f64; 4]>> {
        if !self.flows_round(page, block) {
            return None;
        }
        let (rows, grid) = self.rows_in_the_way(page, block)?;
        if rows.is_empty() {
            return None;
        }
        let view = &self.read.get(&page)?.view;
        let pixels = *self.frames.page(page).get(block)?;
        let user = frame_in_user_space(view, pixels).ok()?;
        let width = user[2] - user[0];
        let height = user[3] - user[1];
        if width <= 0.0 || height <= 0.0 || !width.is_finite() || !height.is_finite() {
            return None;
        }
        let scale = (pixels[2] - pixels[0]) / width;
        let pitch = self.block_pitch(page, block).unwrap_or(pdf_edit::WRAP_ROW);
        if pitch <= 0.0 || !pitch.is_finite() {
            return None;
        }
        let grid_top = grid;
        let mut bands: Vec<[f64; 4]> = Vec::new();
        let mut top = 0.0;
        while grid_top + top < height {
            let bottom = (top + pitch).min(height - grid_top);
            if let Some((left, run)) = pdf_edit::widest_free_run(width, &rows, top, bottom - top) {
                let band = [
                    pixels[0] + left * scale,
                    pixels[1] + (grid_top + top).max(0.0) * scale,
                    pixels[0] + (left + run) * scale,
                    pixels[1] + (grid_top + bottom) * scale,
                ];
                match bands.last_mut() {
                    Some(last)
                        if (last[0] - band[0]).abs() < 1e-6
                            && (last[2] - band[2]).abs() < 1e-6
                            && (last[3] - band[1]).abs() < 1e-6 =>
                    {
                        last[3] = band[3];
                    }
                    _ => bands.push(band),
                }
            }
            top = bottom;
        }
        Some(bands)
    }

    #[must_use]
    pub fn blocked_in(&self, page: usize, block: usize) -> Option<Vec<pdf_edit::Blocked>> {
        self.rows_in_the_way(page, block).map(|(rows, _)| rows)
    }

    fn rows_in_the_way(&self, page: usize, block: usize) -> Option<(Vec<pdf_edit::Blocked>, f64)> {
        let view = &self.read.get(&page)?.view;
        let frame = *self.frames.page(page).get(block)?;
        let user = frame_in_user_space(view, frame).ok()?;
        let reading = self.block_reading(page, block);
        let first = reading.as_ref().and_then(|reading| {
            reading
                .lines
                .first()
                .map(|line| (line.origin.1, reading.pitch))
        });
        match first {
            Some((baseline, pitch)) if pitch.is_finite() && pitch > 0.0 => {
                let rows = pdf_edit::blocked_for_block(
                    &view.graph,
                    view.program.geometry.crop_box[1],
                    (user[0], user[2]),
                    (baseline, pitch),
                );
                Some((rows, user[3] - (baseline + pitch)))
            }
            _ => {
                let pitch = self.block_pitch(page, block).unwrap_or(pdf_edit::WRAP_ROW);
                Some((pdf_edit::blocked_in_frame(&view.graph, user, pitch), 0.0))
            }
        }
    }

    #[must_use]
    pub fn block_reading(&self, page: usize, block: usize) -> Option<pdf_edit::BlockReading> {
        let view = &self.read.get(&page)?.view;
        let frame = *self.frames.page(page).get(block)?;
        let user = frame_edges_in_user_space(view, frame).ok()?;
        let rows = block_rows(view, block)?;
        pdf_edit::read_block(
            &view.program,
            &view.graph,
            &rows,
            user,
            self.frames.edges(page, block),
            self.frames.breaks(page, block).as_ref(),
        )
        .ok()
    }

    #[must_use]
    pub fn block_faces(
        &self,
        page: usize,
        block: usize,
    ) -> Option<Vec<Vec<pdf_edit::ClusterFace>>> {
        let view = &self.read.get(&page)?.view;
        let frame = *self.frames.page(page).get(block)?;
        let user = frame_edges_in_user_space(view, frame).ok()?;
        let rows = block_rows(view, block)?;
        pdf_edit::read_block_faces(
            &view.program,
            &view.graph,
            &rows,
            user,
            self.frames.edges(page, block),
            self.frames.breaks(page, block).as_ref(),
        )
        .ok()
    }

    #[must_use]
    pub fn frame_in_user_space(&self, page: usize, frame: [f64; 4]) -> Option<[f64; 4]> {
        let view = &self.read.get(&page)?.view;
        let device = pdf_render::DeviceTransform::for_page(
            &view.program.geometry,
            OVERLAY_SCALE,
            pdf_render::RenderLimits::default(),
        )
        .ok()?;
        let inverse = device.matrix.inverse()?;
        let one = inverse.transform(pdf_paint::Point {
            x: frame[0],
            y: frame[1],
        });
        let other = inverse.transform(pdf_paint::Point {
            x: frame[2],
            y: frame[3],
        });
        Some([
            one.x.min(other.x),
            one.y.min(other.y),
            one.x.max(other.x),
            one.y.max(other.y),
        ])
    }

    #[must_use]
    pub fn frame_is_declared(&self, page: usize, block: usize) -> bool {
        self.frames.is_declared(page, block)
    }

    pub fn preview_frame(&mut self, page: usize, block: usize, bounds: [f64; 4]) {
        if self.is_busy() {
            return;
        }
        self.frames.preview(page, block, bounds);
        if let Some(leaf) = self.read.get_mut(&page) {
            let device = overlay_device(&leaf.view);
            apply_frames(
                &mut Arc::make_mut(leaf).overlay,
                self.frames.page(page),
                device,
            );
        }
    }

    pub fn finish_frame_resize(&mut self, page: usize, block: usize, started: [f64; 4]) {
        if !self.is_busy() {
            self.frames.finish_resize(page, block, started);
        }
    }

    pub fn declare_frame(&mut self, page: usize, block: usize) {
        self.frames.declare(page, block);
    }
}

fn same_box(one: [f64; 4], other: [f64; 4]) -> bool {
    one.iter()
        .zip(other)
        .all(|(one, other)| one.to_bits() == other.to_bits())
}

fn laid_out_the_same(one: &[PageGeometry], other: &[PageGeometry]) -> bool {
    one.len() == other.len()
        && one.iter().zip(other).all(|(one, other)| {
            same_box(one.media_box, other.media_box)
                && same_box(one.crop_box, other.crop_box)
                && one.rotate == other.rotate
        })
}

fn why_typing_failed(
    view: &PageView,
    line: usize,
    offset: usize,
    typed: &str,
    reason: Refusal,
) -> Refusal {
    let Ok(overlay) = page_overlay_view(view, OVERLAY_SCALE) else {
        return reason;
    };
    let Some(font) = pdf_cli::anchor_under_caret(&overlay.clusters, line, offset)
        .and_then(|anchor| overlay.runs.iter().find(|run| run.anchor == anchor))
        .map(|run| Arc::clone(&run.text))
    else {
        return reason;
    };
    match pdf_cli::typing_verdict(&font, typed) {
        pdf_cli::Typing::NeedsANewGlyph { character } => Refusal::NeedsANewGlyph {
            character,
            why: Box::new(reason),
        },
        pdf_cli::Typing::Ambiguous { character, codes } => {
            Refusal::AmbiguousCode { character, codes }
        }
        pdf_cli::Typing::NoMeaning { refused: true } => Refusal::ToUnicodeUnreadable,
        pdf_cli::Typing::NoMeaning { refused: false } => Refusal::NoToUnicode,
        pdf_cli::Typing::WithinTheFont { .. } | pdf_cli::Typing::Nowhere => {
            Refusal::FontHasTheCodes {
                why: Box::new(reason),
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameFit {
    pub over: [f64; 4],
    pub allowed: f64,
}

impl FrameFit {
    #[must_use]
    pub fn worst(&self) -> (Side, f64) {
        const SIDES: [Side; 4] = [Side::Left, Side::Top, Side::Right, Side::Bottom];
        let mut worst = (SIDES[0], self.over[0]);
        for (name, over) in SIDES.into_iter().zip(self.over).skip(1) {
            if over > worst.1 {
                worst = (name, over);
            }
        }
        worst
    }

    #[must_use]
    pub fn fits(&self) -> bool {
        self.worst().1 <= self.allowed
    }

    #[must_use]
    pub fn refusal_because(&self, why: Refusal) -> Refusal {
        let (side, over) = self.worst();
        Refusal::PastTheFrame {
            side,
            over,
            why: Box::new(why),
        }
    }
}

#[must_use]
pub fn frame_fit(frame: [f64; 4], bounds: [f64; 4], allowed: f64) -> FrameFit {
    FrameFit {
        over: [
            frame[0] - bounds[0],
            frame[1] - bounds[1],
            bounds[2] - frame[2],
            bounds[3] - frame[3],
        ],
        allowed,
    }
}

pub const FRAME_SLACK: f64 = 1e-6;

fn apply_frames(overlay: &mut Overlay, frames: &[[f64; 4]], device: Option<Matrix>) {
    for (block, bounds) in overlay.blocks.iter_mut().zip(frames) {
        if block.turn != 0.0 {
            if let Some(quad) = device.and_then(|device| turned_frame(device, block.turn, *bounds))
            {
                block.box_pixels = quad.iter().fold(
                    [
                        f64::INFINITY,
                        f64::INFINITY,
                        f64::NEG_INFINITY,
                        f64::NEG_INFINITY,
                    ],
                    |had, corner| {
                        [
                            had[0].min(corner[0]),
                            had[1].min(corner[1]),
                            had[2].max(corner[0]),
                            had[3].max(corner[1]),
                        ]
                    },
                );
                block.quad = quad;
            }
            continue;
        }
        if !crate::view::Quad::from_pixels(block.quad).upright() {
            continue;
        }
        block.box_pixels = *bounds;
        block.quad = [
            [bounds[0], bounds[1]],
            [bounds[2], bounds[1]],
            [bounds[2], bounds[3]],
            [bounds[0], bounds[3]],
        ];
    }
}

fn overlay_device(view: &PageView) -> Option<Matrix> {
    pdf_render::DeviceTransform::for_page(
        &view.program.geometry,
        OVERLAY_SCALE,
        pdf_render::RenderLimits::default(),
    )
    .ok()
    .map(|device| device.matrix)
}

fn turned_frame(device: Matrix, turn: f64, frame: [f64; 4]) -> Option<[[f64; 2]; 4]> {
    let inverse = device.inverse()?;
    let corner = |x, y| {
        let turned = inverse.transform(pdf_paint::Point { x, y });
        let placed = device.transform(pdf_edit::turned_out_of(turn, turned));
        [placed.x, placed.y]
    };
    Some([
        corner(frame[0], frame[1]),
        corner(frame[2], frame[1]),
        corner(frame[2], frame[3]),
        corner(frame[0], frame[3]),
    ])
}

fn into_turned_pixels(device: Matrix, turn: f64, (dx, dy): (f64, f64)) -> (f64, f64) {
    if turn == 0.0 {
        return (dx, dy);
    }
    let linear = Matrix {
        e: 0.0,
        f: 0.0,
        ..device
    };
    let Some(inverse) = linear.inverse() else {
        return (dx, dy);
    };
    let user = inverse.transform(pdf_paint::Point { x: dx, y: dy });
    let turned = pdf_edit::rotation(-turn).transform(user);
    let placed = linear.transform(turned);
    (placed.x, placed.y)
}

const PANICKED: &str = "this edit could not finish \u{2014} something in it panicked, so nothing it had not \
     already committed was kept";

type Dispatched = (
    Applied,
    Message,
    Option<(usize, usize)>,
    Option<(usize, usize)>,
);

impl EditJob {
    #[must_use]
    pub fn run(mut self) -> EditOutcome {
        self.session.begin_timing();
        let dispatched =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.run_dispatch()));
        let timed = self.session.end_timing();
        let (applied, status, caret, anchor) = dispatched.unwrap_or_else(|_| {
            let (applied, status) = refused(PANICKED);
            (applied, status, None, None)
        });
        EditOutcome {
            paragraphs: std::mem::take(&mut self.paragraphs),
            flowed: std::mem::take(&mut self.flowed),
            session: self.session,
            applied,
            status,
            frames: self.frames,
            caret,
            anchor,
            wrote_text: self.wrote_text,
            laid: self.laid,
            timed,
        }
    }

    #[expect(clippy::too_many_lines, reason = "a dispatch, one arm per step")]
    fn run_dispatch(&mut self) -> Dispatched {
        let mut caret = None;
        let mut anchor = None;
        let (applied, status) = match &self.step {
            Step::Type {
                page,
                block,
                range,
                text,
                style,
            } => {
                let (page, block, range, text, style) =
                    (*page, *block, *range, text.clone(), style.clone());
                let (applied, status, landed) =
                    self.typed_step(page, block, range, &text, style.as_ref());
                caret = landed;
                (applied, status)
            }
            Step::Style {
                page,
                block,
                range,
                style,
            } => {
                let (page, block, range, style) = (*page, *block, *range, style.clone());
                let (applied, status, landed) = self.style_range(page, block, range, &style);
                if let Some((at, start)) = landed {
                    caret = Some(at);
                    anchor = start;
                }
                (applied, status)
            }
            Step::FlowRound { page, blocks } => {
                let (page, blocks) = (*page, blocks.clone());
                self.flow_round(page, &blocks)
            }
            Step::ResizeFrame {
                page,
                block,
                started,
            } => {
                let (page, block, started) = (*page, *block, *started);
                self.resize_and_relay(page, block, started)
            }
            Step::PlaceText { .. } => self.run_new_text(),
            Step::PlaceImage { page, pictures } => {
                let (page, pictures) = (*page, pictures.clone());
                self.place_new_images(page, pictures)
            }
            Step::DrawLine {
                page,
                steps,
                stroke,
                fill,
                closed,
                drew,
            } => {
                let drawn = Drawn {
                    steps: steps.clone(),
                    stroke: *stroke,
                    fill: *fill,
                    closed: *closed,
                    drew: *drew,
                };
                let page = *page;
                self.draw_new_path(page, drawn)
            }
            Step::AddPage {
                beside,
                before,
                size,
            } => {
                let (beside, before, size) = (*beside, *before, *size);
                self.add_page(beside, before, size)
            }
            Step::RemovePages { .. }
            | Step::MovePages { .. }
            | Step::RotatePages { .. }
            | Step::InsertPages { .. } => self.run_page_change(),
            Step::Describe { edit } => {
                let edit = edit.clone();
                self.describe(&edit)
            }
            Step::Stamp { .. } => self.run_stamp(),
            Step::TextLayers { .. } => self.run_text_layers(),
            Step::Move {
                page,
                anchor,
                dx,
                dy,
            } => {
                let (page, anchor, dx, dy) = (*page, anchor.clone(), *dx, *dy);
                self.move_run(page, &anchor, dx, dy)
            }
            Step::MoveBlock {
                page,
                anchors,
                group,
                dx,
                dy,
                ..
            } => {
                let (page, anchors, dx, dy) = (*page, anchors.clone(), *dx, *dy);
                let group = *group;
                self.move_block(page, &anchors, group, dx, dy)
            }
            Step::MoveGroup {
                page,
                anchors,
                objects,
                dx,
                dy,
            } => {
                let (page, anchors, objects) = (*page, anchors.clone(), objects.clone());
                self.move_group(page, &anchors, &objects, (*dx, *dy))
            }
            Step::Paste {
                page,
                copied,
                dx,
                dy,
                elsewhere,
            } => {
                let (page, copied, elsewhere) = (*page, copied.clone(), elsewhere.clone());
                self.paste(page, copied, (*dx, *dy), elsewhere)
            }
            Step::Place { .. }
            | Step::Shape { .. }
            | Step::ShapeBlock { .. }
            | Step::SetSize { .. }
            | Step::SetAngles { .. } => self.run_placement(),
            Step::DeleteBlock { page, anchors } => {
                let (page, anchors) = (*page, anchors.clone());
                self.delete_block(page, &anchors)
            }
            Step::DeleteGroup {
                page,
                anchors,
                objects,
            } => {
                let (page, anchors, objects) = (*page, anchors.clone(), objects.clone());
                self.delete_group(page, &anchors, &objects)
            }
            Step::RemoveObject {
                page,
                anchor,
                rubbing,
            } => {
                let (page, anchor, rubbing) = (*page, anchor.clone(), *rubbing);
                self.remove_object(page, &anchor, rubbing)
            }
            Step::ReorderObjects {
                page,
                anchors,
                order,
            } => {
                let (page, anchors, order) = (*page, anchors.clone(), *order);
                self.reorder_objects(page, &anchors, order)
            }
            Step::AddField {
                page,
                rect,
                kind,
                name,
                options,
            } => {
                let command = Command::AddField {
                    page_index: *page,
                    rect: *rect,
                    kind: *kind,
                    name: name.clone(),
                    options: options.clone(),
                };
                self.form_edit(&command, Done::AddedField)
            }
            Step::SetFieldBoxes { page, boxes } => {
                let command = Command::SetFieldBoxes {
                    page_index: *page,
                    boxes: boxes.clone(),
                };
                self.form_edit(&command, Done::MovedFields { count: boxes.len() })
            }
            Step::CopyFields { page, copies } => {
                let command = Command::CopyFields {
                    page_index: *page,
                    copies: copies.clone(),
                };
                self.form_edit(
                    &command,
                    Done::CopiedFields {
                        count: copies.len(),
                    },
                )
            }
            Step::AddLink {
                page,
                rect,
                target,
                look,
            } => {
                let command = Command::AddLink {
                    page_index: *page,
                    rect: *rect,
                    target: target.clone(),
                    look: *look,
                };
                self.form_edit(&command, Done::AddedLink)
            }
            Step::AddLinks { page, links } => {
                let command = Command::AddLinks {
                    page_index: *page,
                    links: links.clone(),
                };
                self.form_edit(&command, Done::AddedLinks { count: links.len() })
            }
            Step::SetLinkProperties {
                page,
                link,
                target,
                look,
            } => {
                let command = Command::SetLinkProperties {
                    page_index: *page,
                    link: *link,
                    target: target.clone(),
                    look: *look,
                };
                let said = if target.is_some() {
                    Done::ChangedLink
                } else {
                    Done::ChangedLinkLook
                };
                self.form_edit(&command, said)
            }
            Step::ChangeNaming { page, change } => {
                let command = Command::ChangeNaming {
                    page_index: *page,
                    change: change.clone(),
                };
                let said = match change {
                    pdf_edit::destination::Naming::Name { .. } => Done::NamedAPlace,
                    pdf_edit::destination::Naming::Rename { .. } => Done::RenamedAPlace,
                    pdf_edit::destination::Naming::Remove { .. } => Done::RemovedAPlace,
                };
                self.form_edit(&command, said)
            }
            Step::SetLinkBox { page, link, rect } => {
                let command = Command::SetLinkBox {
                    page_index: *page,
                    link: *link,
                    rect: *rect,
                };
                self.form_edit(&command, Done::MovedLink)
            }
            Step::SetLinkBoxes { page, boxes } => {
                let command = Command::SetLinkBoxes {
                    page_index: *page,
                    boxes: boxes.clone(),
                };
                self.form_edit(&command, Done::MovedLinks { count: boxes.len() })
            }
            Step::RemoveLinks { page, links } => {
                let command = Command::RemoveLinks {
                    page_index: *page,
                    links: links.clone(),
                };
                self.form_edit(&command, Done::RemovedLinks { count: links.len() })
            }
            Step::RemoveLink { page, link } => {
                let command = Command::RemoveLink {
                    page_index: *page,
                    link: *link,
                };
                self.form_edit(&command, Done::RemovedLink)
            }
            Step::ChangeOutline { page, change } => {
                let command = Command::ChangeOutline {
                    page_index: *page,
                    change: change.clone(),
                };
                self.form_edit(&command, Done::ChangedBookmarks)
            }
            Step::SetTabOrder {
                page,
                widgets,
                order,
            } => {
                let command = Command::SetTabOrder {
                    page_index: *page,
                    widgets: widgets.clone(),
                    order: *order,
                };
                self.form_edit(&command, Done::OrderedFields)
            }
            Step::RemoveFields { page, widgets } => {
                let command = Command::RemoveFields {
                    page_index: *page,
                    widgets: widgets.clone(),
                };
                self.form_edit(
                    &command,
                    Done::RemovedFields {
                        count: widgets.len(),
                    },
                )
            }
            Step::SetFieldBox { page, widget, rect } => {
                let command = Command::SetFieldBox {
                    page_index: *page,
                    widget: *widget,
                    rect: *rect,
                };
                self.form_edit(&command, Done::MovedField)
            }
            Step::SetFieldSettings {
                page,
                widget,
                settings,
            } => {
                let command = Command::SetFieldSettings {
                    page_index: *page,
                    widget: *widget,
                    settings: settings.clone(),
                };
                self.form_edit(&command, Done::ChangedField)
            }
            Step::RemoveField { page, widget } => {
                let command = Command::RemoveField {
                    page_index: *page,
                    widget: *widget,
                };
                self.form_edit(&command, Done::RemovedField)
            }
            Step::FillField {
                page,
                widget,
                value,
            } => {
                let (page, widget, value) = (*page, *widget, value.clone());
                self.fill_field(page, widget, value)
            }
            Step::Undo | Step::Redo => self.walk(matches!(self.step, Step::Undo)),
            #[cfg(test)]
            Step::Panics => panic!("test: this step always panics"),
        };
        self.keep_frames(&applied);
        let (applied, status) = match self.keep_the_flow(&applied) {
            Some(kept @ Message::TextMadeWay { .. }) => match applied {
                Applied::Changed { page, .. } => (Applied::Changed { page, region: None }, kept),
                other => (other, kept),
            },
            Some(kept) => (applied, kept),
            None => (applied, status),
        };
        (applied, status, caret, anchor)
    }

    #[expect(
        clippy::type_complexity,
        reason = "the step's three answers, taken apart at once"
    )]
    fn style_range(
        &mut self,
        page: usize,
        block: usize,
        range: BlockRange,
        style: &pdf_edit::TextStyle,
    ) -> (
        Applied,
        Message,
        Option<((usize, usize), Option<(usize, usize)>)>,
    ) {
        let typed = self
            .session
            .page(page)
            .map_err(|error| Edited::from(error.to_string()))
            .and_then(|view| {
                self.type_in_block(&view, page, block, range, ("", None), Some(style))
            });
        match typed {
            Ok(typed) => {
                self.resize_frame = typed
                    .frame
                    .map(|frame| (typed.block, frame, typed.edges, typed.breaks.clone()));
                (
                    self.landed(),
                    Done::Styled.into(),
                    Some(((typed.row, typed.offset), typed.anchor)),
                )
            }
            Err(Edited::Nothing) => (Applied::Unchanged, Message::Quiet, None),
            Err(Edited::Refused(reason)) => {
                let (applied, status) = refused(reason);
                (applied, status, None)
            }
        }
    }

    fn keep_frames(&mut self, applied: &Applied) {
        let Applied::Changed { page, .. } = *applied else {
            return;
        };
        match self.step {
            Step::Undo | Step::Redo | Step::FlowRound { .. } | Step::ResizeFrame { .. } => {}
            Step::RotatePages { ref pages, .. } | Step::Stamp { ref pages, .. } => {
                let pages = pages.clone();
                self.frames.pages_forgotten(&pages);
            }
            Step::TextLayers { ref layers, .. } => {
                let pages: Vec<usize> = layers.iter().map(|(page, _)| *page).collect();
                self.frames.pages_forgotten(&pages);
            }
            Step::AddPage { .. }
            | Step::RemovePages { .. }
            | Step::MovePages { .. }
            | Step::InsertPages { .. } => {
                let _ = page;
                if let Some(change) = self.session.last_pages().cloned() {
                    self.frames.pages_changed(&change);
                }
                self.paragraphs.clear();
                self.flowed.clear();
            }
            _ => match self.resize_frame.take() {
                Some((block, frame, edges, breaks)) => {
                    self.frames
                        .source_resized(page, block, frame, edges, breaks);
                }
                None if self.group_frames.is_empty() => {
                    self.frames.source_changed(page, self.move_frame);
                }
                None => {
                    let moved = std::mem::take(&mut self.group_frames);
                    self.frames.source_moved(page, &moved);
                }
            },
        }
    }

    fn run_new_text(&mut self) -> (Applied, Message) {
        let Step::PlaceText {
            page,
            frame,
            text,
            family,
            size,
            bold,
            italic,
            fill,
            paragraph,
        } = &self.step
        else {
            return (Applied::Unchanged, Done::NothingChanged.into());
        };
        let (page, frame, text) = (*page, *frame, text.clone());
        let style = NewTextStyle {
            family: family.clone(),
            size: *size,
            bold: *bold,
            italic: *italic,
            fill: *fill,
            paragraph: *paragraph,
        };
        self.place_text(page, frame, &text, &style)
    }

    fn run_placement(&mut self) -> (Applied, Message) {
        match &self.step {
            Step::Place {
                page,
                anchor,
                dx,
                dy,
            } => {
                let (page, anchor, dx, dy) = (*page, anchor.clone(), *dx, *dy);
                self.place(page, &anchor, dx, dy)
            }
            Step::Shape {
                page,
                anchor,
                matrix,
                about,
            } => {
                let (page, anchor, matrix, about) = (*page, anchor.clone(), *matrix, *about);
                self.shape(page, &anchor, matrix, about)
            }
            Step::ShapeBlock {
                page,
                anchors,
                matrix,
                about,
            } => {
                let (page, anchors, matrix, about) = (*page, anchors.clone(), *matrix, *about);
                self.shape_block(page, &anchors, matrix, about)
            }
            Step::SetSize {
                page,
                anchors,
                points,
            } => {
                let (page, anchors, points) = (*page, anchors.clone(), *points);
                self.set_size(page, &anchors, points)
            }
            Step::SetAngles {
                page,
                anchors,
                turn,
                slant,
            } => {
                let (page, anchors, turn, slant) = (*page, anchors.clone(), *turn, *slant);
                self.set_angles(page, &anchors, turn, slant)
            }
            _ => (Applied::Unchanged, Done::NothingChanged.into()),
        }
    }

    fn move_run(&mut self, page: usize, anchor: &str, dx: f64, dy: f64) -> (Applied, Message) {
        let plan = match self.plan_move(page, anchor, dx, dy) {
            Ok(plan) => plan,
            Err(reason) => return refused(reason),
        };
        let capability = format!("{:?}", plan.capability());
        let hidden = how_hidden(plan.showing());
        if let Err(error) = self.session.apply(plan) {
            return refused(error.to_string());
        }
        (self.landed(), Done::MovedRun { capability, hidden }.into())
    }

    fn move_block(
        &mut self,
        page: usize,
        anchors: &[String],
        group: bool,
        dx: f64,
        dy: f64,
    ) -> (Applied, Message) {
        let mut runs = Vec::with_capacity(anchors.len());
        for anchor in anchors {
            let Some(decoded) = SourceAnchor::decode(anchor) else {
                return refused("malformed anchor");
            };
            runs.push(decoded);
        }
        let offset = match self.session.page(page) {
            Ok(view) => page_offset_view(&view, OVERLAY_SCALE, dx, dy),
            Err(error) => Err(error.to_string()),
        };
        let (dx, dy) = match offset {
            Ok(offset) => offset,
            Err(reason) => return refused(reason),
        };
        let shared = self.shared_block(page);
        let command = match shared {
            Some(block) => self.shift_command(page, block, dx, dy),
            None => Ok(Command::MoveTextBlock {
                page_index: page,
                runs,
                dx,
                dy,
            }),
        };
        let mut planned = match command {
            Ok(command) => self.session.plan(&command),
            Err(reason) => return refused(reason),
        };
        let wanted = match &planned {
            Err(pdf_session::PlanError::Plan(SpikeError::BlockNotChainAligned)) => true,
            Ok(plan) => plan.showing() != pdf_edit::Showing::Whole,
            Err(_) => false,
        };
        if shared.is_none()
            && wanted
            && let Some((block, _, _)) = self.move_frame
            && let Ok(command) = self.shift_command(page, block, dx, dy)
            && let Ok(plan) = self.session.plan(&command)
            && (planned.is_err() || plan.showing() == pdf_edit::Showing::Whole)
        {
            planned = Ok(plan);
        }
        let plan = match planned {
            Ok(plan) => plan,
            Err(error) => {
                let mut reason = why_a_block_will_not_move(&error);
                if shared.is_some() {
                    reason = Refusal::BlockSharesOperators {
                        why: Box::new(reason),
                    };
                }
                return refused(reason);
            }
        };
        let capability = format!("{:?}", plan.capability());
        let moved = plan.effect().moved.len();
        let hidden = how_hidden(plan.showing());
        if let Err(error) = self.session.apply(plan) {
            return refused(error.to_string());
        }
        let done = if group {
            Done::MovedTextGroup {
                commands: moved,
                capability,
                hidden,
            }
        } else {
            Done::MovedBlock {
                commands: moved,
                capability,
                hidden,
            }
        };
        (self.landed(), done.into())
    }

    fn move_group(
        &mut self,
        page: usize,
        anchors: &[String],
        objects: &[String],
        (dx, dy): (f64, f64),
    ) -> (Applied, Message) {
        let decode = |named: &[String]| -> Option<Vec<SourceAnchor>> {
            named
                .iter()
                .map(|anchor| SourceAnchor::decode(anchor))
                .collect()
        };
        let (Some(runs), Some(objects)) = (decode(anchors), decode(objects)) else {
            return refused("malformed anchor");
        };
        let offset = match self.session.page(page) {
            Ok(view) => page_offset_view(&view, OVERLAY_SCALE, dx, dy),
            Err(error) => Err(error.to_string()),
        };
        let (dx, dy) = match offset {
            Ok(offset) => offset,
            Err(reason) => return refused(reason),
        };
        let planned = self.session.plan(&Command::MoveGroup {
            page_index: page,
            runs,
            objects,
            dx,
            dy,
        });
        let plan = match planned {
            Ok(plan) => plan,
            Err(error) => {
                return refused(why_a_block_will_not_move(&error));
            }
        };
        let moved = plan.effect().moved.len();
        if let Err(error) = self.session.apply(plan) {
            return refused(error.to_string());
        }
        (self.landed(), Done::MovedGroup { pieces: moved }.into())
    }

    fn shared_block(&mut self, page: usize) -> Option<usize> {
        let (block, _, _) = self.move_frame?;
        let view = self.session.page(page).ok()?;
        let index = &view.index;
        let atoms_of = |owner: &pdf_semantics::Block| -> std::collections::BTreeSet<usize> {
            owner
                .lines
                .iter()
                .flat_map(|line| index.lines[*line].clusters.iter())
                .map(|cluster| index.clusters[*cluster].atom)
                .collect()
        };
        let own = atoms_of(index.blocks.get(block)?);
        index
            .blocks
            .iter()
            .enumerate()
            .any(|(other, owner)| other != block && !atoms_of(owner).is_disjoint(&own))
            .then_some(block)
    }

    fn shift_command(
        &mut self,
        page: usize,
        block: usize,
        dx: f64,
        dy: f64,
    ) -> Result<Command, Refusal> {
        let view = self.session.page(page).map_err(|error| error.to_string())?;
        let rows = block_rows(&view, block).ok_or("row has no block owner")?;
        let frame = *self
            .frames
            .page(page)
            .get(block)
            .ok_or("no frame for this block")?;
        let user = frame_edges_in_user_space(&view, frame)?;
        let breaks = self
            .frames
            .breaks(page, block)
            .unwrap_or_else(|| pdf_edit::RowEnds::paragraphs((0..rows.len()).collect()));
        Ok(Command::ShiftBlock {
            page_index: page,
            rows,
            frame: user,
            edges: self.frames.edges(page, block),
            breaks: Some(breaks),
            dx,
            dy,
            paragraph: paragraph_of(&self.paragraphs, page, block),
        })
    }

    fn place(&mut self, page: usize, anchor: &str, dx: f64, dy: f64) -> (Applied, Message) {
        let Some(decoded) = SourceAnchor::decode(anchor) else {
            return refused("malformed anchor");
        };
        let offset = match self.session.page(page) {
            Ok(view) => page_offset_view(&view, OVERLAY_SCALE, dx, dy),
            Err(error) => Err(error.to_string()),
        };
        let (dx, dy) = match offset {
            Ok(offset) => offset,
            Err(reason) => return refused(reason),
        };
        let planned = self.session.plan(&Command::PlaceObject {
            page_index: page,
            target: ObjectSelection::Painted(decoded),
            transform: Matrix {
                e: dx,
                f: dy,
                ..Matrix::IDENTITY
            },
            about: FixedPoint::Origin,
        });
        let plan = match planned {
            Ok(plan) => plan,
            Err(error) => {
                return refused(why_a_picture_will_not_move(&error));
            }
        };
        let capability = format!("{:?}", plan.capability());
        let hidden = how_hidden(plan.showing());
        if let Err(error) = self.session.apply(plan) {
            return refused(error.to_string());
        }
        (
            self.landed(),
            Done::MovedPicture { capability, hidden }.into(),
        )
    }

    fn shape(
        &mut self,
        page: usize,
        anchor: &str,
        matrix: Matrix,
        about: (f64, f64),
    ) -> (Applied, Message) {
        let Some(decoded) = SourceAnchor::decode(anchor) else {
            return refused("malformed anchor");
        };
        let carried = match self.session.page(page) {
            Ok(view) => page_transform_view(&view, OVERLAY_SCALE, matrix, about),
            Err(error) => Err(error.to_string()),
        };
        let (transform, held) = match carried {
            Ok(carried) => carried,
            Err(reason) => return refused(reason),
        };
        let planned = self.session.plan(&Command::PlaceObject {
            page_index: page,
            target: ObjectSelection::Painted(decoded),
            transform,
            about: FixedPoint::At(held),
        });
        let plan = match planned {
            Ok(plan) => plan,
            Err(error) => {
                return refused(why_a_picture_will_not_move(&error));
            }
        };
        let capability = format!("{:?}", plan.capability());
        if let Err(error) = self.session.apply(plan) {
            return refused(error.to_string());
        }
        (self.landed(), Done::ShapedPicture { capability }.into())
    }

    fn shape_block(
        &mut self,
        page: usize,
        anchors: &[String],
        matrix: Matrix,
        about: (f64, f64),
    ) -> (Applied, Message) {
        let mut runs = Vec::with_capacity(anchors.len());
        for anchor in anchors {
            let Some(decoded) = SourceAnchor::decode(anchor) else {
                return refused("malformed anchor");
            };
            runs.push(decoded);
        }
        let carried = match self.session.page(page) {
            Ok(view) => page_transform_view(&view, OVERLAY_SCALE, matrix, about),
            Err(error) => Err(error.to_string()),
        };
        let (transform, held) = match carried {
            Ok(carried) => carried,
            Err(reason) => return refused(reason),
        };
        let planned = self.session.plan(&Command::PlaceObject {
            page_index: page,
            target: ObjectSelection::Text(runs),
            transform,
            about: FixedPoint::At(held),
        });
        let plan = match planned {
            Ok(plan) => plan,
            Err(error) => {
                return refused(why_a_block_will_not_move(&error));
            }
        };
        let capability = format!("{:?}", plan.capability());
        if let Err(error) = self.session.apply(plan) {
            return refused(error.to_string());
        }
        (self.landed(), Done::ShapedParagraph { capability }.into())
    }

    fn set_size(&mut self, page: usize, anchors: &[String], points: f64) -> (Applied, Message) {
        if let Some((block, end)) = self.whole_block {
            let style = pdf_edit::TextStyle {
                size: Some(points),
                ..pdf_edit::TextStyle::default()
            };
            let (applied, status, _) = self.style_range(
                page,
                block,
                BlockRange::Between {
                    from: (0, 0),
                    to: end,
                },
                &style,
            );
            if matches!(applied, Applied::Changed { .. }) {
                return (applied, status);
            }
            self.resize_frame = None;
        }
        let runs = match decoded_anchors(anchors) {
            Ok(runs) => runs,
            Err(reason) => return refused(reason),
        };
        let planned = self.session.plan(&Command::SetTextSize {
            page_index: page,
            runs,
            points,
        });
        self.applied(planned, |capability| Done::Sized { points, capability })
    }

    fn set_angles(
        &mut self,
        page: usize,
        anchors: &[String],
        turn: Option<f64>,
        slant: Option<f64>,
    ) -> (Applied, Message) {
        let runs = match decoded_anchors(anchors) {
            Ok(runs) => runs,
            Err(reason) => return refused(reason),
        };
        let planned = self.session.plan(&Command::SetTextShape {
            page_index: page,
            runs,
            turn: turn.map(f64::to_radians),
            slant: slant.map(f64::to_radians),
        });
        self.applied(planned, |capability| match (turn, slant) {
            (Some(degrees), _) => Done::Turned {
                degrees,
                capability,
            },
            (_, Some(degrees)) => Done::Slanted {
                degrees,
                capability,
            },
            _ => Done::AnglesUnchanged { capability },
        })
    }

    fn applied(
        &mut self,
        planned: Result<pdf_edit::plan::Plan, pdf_session::PlanError>,
        said: impl FnOnce(String) -> Done,
    ) -> (Applied, Message) {
        let plan = match planned {
            Ok(plan) => plan,
            Err(error) => {
                return refused(why_a_block_will_not_move(&error));
            }
        };
        let capability = format!("{:?}", plan.capability());
        if let Err(error) = self.session.apply(plan) {
            return refused(error.to_string());
        }
        (self.landed(), said(capability).into())
    }

    fn delete_on_row(
        &mut self,
        page: usize,
        line: usize,
        from: usize,
        to: usize,
    ) -> Result<(usize, bool), Refusal> {
        let (plan, closed, clusters) =
            if let Ok((plan, clusters)) = self.plan_delete(page, line, from, to, true) {
                (plan, true, clusters)
            } else {
                let (plan, clusters) = self.plan_delete(page, line, from, to, false)?;
                (plan, false, clusters)
            };
        self.session
            .apply(plan)
            .map_err(|error| error.to_string())?;
        Ok((clusters, closed))
    }

    fn typed_step(
        &mut self,
        page: usize,
        block: usize,
        range: BlockRange,
        text: &str,
        style: Option<&pdf_edit::TextStyle>,
    ) -> (Applied, Message, Option<(usize, usize)>) {
        match self.type_text(page, block, range, text, style) {
            Ok(typed) => {
                self.resize_frame = typed
                    .frame
                    .map(|frame| (typed.block, frame, typed.edges, typed.breaks.clone()));
                (
                    self.landed(),
                    typed_status(&typed, text.is_empty()),
                    Some((typed.row, typed.offset)),
                )
            }
            Err(Edited::Nothing) => (Applied::Unchanged, Done::NothingToDeleteHere.into(), None),
            Err(Edited::Refused(reason)) => {
                let (applied, said) = refused(reason);
                (applied, said, None)
            }
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the block path and the row path it falls back to, read in the order they are tried"
    )]
    fn type_text(
        &mut self,
        page: usize,
        block: usize,
        range: BlockRange,
        text: &str,
        typed: Option<&pdf_edit::TextStyle>,
    ) -> Result<Typed, Edited> {
        let view = self.session.page(page).map_err(|error| error.to_string())?;
        let lines = view
            .index
            .blocks
            .get(block)
            .ok_or("row has no block owner")?
            .lines
            .clone();
        let note = match self.type_in_block(&view, page, block, range, (text, typed), None) {
            Ok(typed) => return Ok(typed),
            Err(Edited::Nothing) => return Err(Edited::Nothing),
            Err(Edited::Refused(reason)) if typed.is_some() => {
                return Err(Edited::Refused(reason));
            }
            Err(Edited::Refused(reason))
                if text.contains(['\n', pdf_edit::LINE_BREAK])
                    || matches!(
                        reason,
                        Refusal::Layout {
                            why: LayoutWhy::EveryCharacterDeleted,
                            ..
                        }
                    ) =>
            {
                return Err(Edited::Refused(reason));
            }
            Err(Edited::Refused(reason)) => reason,
        };
        let frame = self.frames.page(page).get(block).copied();
        let reading = frame
            .and_then(|frame| frame_edges_in_user_space(&view, frame).ok())
            .and_then(|user| {
                pdf_edit::read_block(
                    &view.program,
                    &view.graph,
                    &block_rows(&view, block)?,
                    user,
                    self.frames.edges(page, block),
                    self.frames.breaks(page, block).as_ref(),
                )
                .ok()
            });
        let row_of = |reading_row: usize| match &reading {
            Some(reading) => reading.lines.get(reading_row)?.row,
            None => (reading_row < lines.len()).then_some(reading_row),
        };
        let clusters_on = |row: usize| view.index.lines[lines[row]].clusters.len();
        let (reading_row, row, from, to) = match range {
            BlockRange::Between { from, to } if from.0 == to.0 => {
                let row = row_of(from.0).ok_or_else(|| Refusal::EmptyLineNeedsLayout {
                    why: Box::new(note.clone()),
                })?;
                (from.0, row, from.1.min(to.1), from.1.max(to.1))
            }
            BlockRange::Units {
                at,
                backwards,
                count,
            } => {
                let row = row_of(at.0).ok_or_else(|| Refusal::DeleteNeedsLayout {
                    why: Box::new(note.clone()),
                })?;
                let (from, to) = if backwards {
                    (at.1.checked_sub(count), Some(at.1))
                } else {
                    (
                        Some(at.1),
                        Some(at.1 + count).filter(|end| *end <= clusters_on(row)),
                    )
                };
                match (from, to) {
                    (Some(from), Some(to)) => (at.0, row, from, to),
                    _ if reading.is_none() && at == (0, 0) && backwards => {
                        return Err(Edited::Nothing);
                    }
                    _ => {
                        return Err(Refusal::DeleteAcrossLines {
                            why: Box::new(note),
                        }
                        .into());
                    }
                }
            }
            BlockRange::Between { .. } => {
                return Err(Refusal::EditAcrossLines {
                    why: Box::new(note),
                }
                .into());
            }
        };
        let line = lines[row];
        if text.is_empty() {
            if from == to {
                return Err(Edited::Nothing);
            }
            let (clusters, gap_closed) = self.delete_on_row(page, line, from, to)?;
            return Ok(Typed {
                anchor: None,
                block,
                row: reading_row,
                offset: from,
                frame: None,
                edges: self.frames.edges(page, block),
                breaks: self.frames.breaks(page, block),
                overflow: false,
                brought_in: None,
                drawn_from: None,
                cropped: false,
                status: Some(Done::DeletedOnRow {
                    clusters,
                    gap_closed,
                    not_laid_out: Box::new(note.clone()),
                }),
                note: Some(note),
            });
        }
        let before = view
            .index
            .lines
            .get(line)
            .ok_or("row is not on this page")?
            .clusters
            .len();
        let runs = match pdf_cli::page_replacement_view(&view, line, from, to, text) {
            Ok(runs) => runs,
            Err(reason) => {
                let reason = Refusal::Both(Box::new(reason.into()), Box::new(note));
                return Err(why_typing_failed(&view, line, from, text, reason).into());
            }
        };
        let plan = self
            .session
            .plan(&Command::RewriteText {
                page_index: page,
                runs,
            })
            .map_err(|error| error.to_string())?;
        let candidate = self.session.preview(&plan)?;
        let overlay = page_overlay_view(&candidate, OVERLAY_SCALE)?;
        let frame = *self
            .frames
            .page(page)
            .get(block)
            .ok_or("no frame for this block")?;
        let bounds = overlay
            .blocks
            .get(block)
            .ok_or("replacement lost block ownership")?
            .layout_pixels;
        let fit = frame_fit(frame, bounds, FRAME_SLACK);
        if candidate.index.report.clusters_outside_the_grouping
            > view.index.report.clusters_outside_the_grouping
        {
            return Err(Refusal::TypedTextLeftTheBlock {
                why: Box::new(note),
            }
            .into());
        }
        if !other_blocks_stay(view.as_ref(), &candidate, block) {
            return Err(Refusal::OtherBlockWouldShift {
                why: Box::new(note),
            }
            .into());
        }
        if !fit.fits() {
            return Err(fit.refusal_because(note).into());
        }
        let after = candidate
            .index
            .blocks
            .get(block)
            .and_then(|owner| owner.lines.get(row))
            .and_then(|line| candidate.index.lines.get(*line))
            .ok_or("replacement lost the row it typed into")?
            .clusters
            .len();
        let caret = (to + after)
            .checked_sub(before)
            .ok_or("replacement left fewer clusters than the text after the selection")?;
        self.session
            .apply_holding(plan, Some(Arc::new(candidate)))
            .map_err(|error| error.to_string())?;
        self.laid = Some((page, overlay_from(overlay)));
        Ok(Typed {
            anchor: None,
            block,
            row: reading_row,
            offset: caret,
            frame: None,
            edges: self.frames.edges(page, block),
            breaks: self.frames.breaks(page, block),
            overflow: false,
            brought_in: None,
            drawn_from: pdf_cli::substituted_family_on_row(&view, line, from),
            cropped: false,
            note: Some(note),
            status: None,
        })
    }

    fn type_in_block(
        &mut self,
        view: &pdf_session::PageView,
        page: usize,
        block: usize,
        range: BlockRange,
        (text, typed): (&str, Option<&pdf_edit::TextStyle>),
        style: Option<&pdf_edit::TextStyle>,
    ) -> Result<Typed, Edited> {
        let rows = block_rows(view, block).ok_or("row has no block owner")?;
        let frame = *self
            .frames
            .page(page)
            .get(block)
            .ok_or("no frame for this block")?;
        let user = frame_edges_in_user_space(view, frame)?;
        let empty = (rows.iter().all(Vec::is_empty) && style.is_none())
            .then(|| empty_show_anchor(view, frame))
            .transpose()?;
        if empty.is_some() && text.is_empty() {
            return Err(Edited::Nothing);
        }
        let (edges, mut breaks) = (
            self.frames.edges(page, block),
            self.frames.breaks(page, block),
        );
        if style.is_some() && breaks.is_none() {
            breaks = Some(pdf_edit::RowEnds::paragraphs((0..rows.len()).collect()));
        }
        let expected = (style.is_none() && empty.is_none())
            .then(|| expected_reading(view, &rows, (user, edges, breaks.as_ref()), range, text))
            .flatten();
        let declared = self.frames.is_declared(page, block);
        let paragraph = paragraph_of(&self.paragraphs, page, block);
        let command = match empty {
            Some(run) => empty_block_command(page, run, user, text, typed, paragraph),
            None => block_command(
                page,
                (rows, user, edges, breaks),
                range,
                (text, typed, style),
                declared,
                paragraph,
            ),
        };
        let plan = match self.session.plan(&command) {
            Ok(plan) => plan,
            Err(error) => {
                let reason = error.to_string();
                return Err(if reason.contains(NOTHING_TO_DELETE) {
                    Edited::Nothing
                } else {
                    Edited::Refused(explain_block_refusal(&reason))
                });
            }
        };
        let outcome = plan
            .block()
            .cloned()
            .ok_or("the block rewrite said nothing about its layout")?;
        let candidate = self.session.preview(&plan)?;
        let overlay = page_overlay_view(&candidate, OVERLAY_SCALE)?;
        let laid = overlay
            .blocks
            .get(block)
            .ok_or("the block lost its place when it was laid out again")?
            .layout_pixels;
        let resized = Self::frame_after(frame, laid, &outcome, declared);
        let anchor = outcome
            .anchor
            .and_then(|planned| planned_in(&candidate, block, user, &outcome, planned));
        if let Some(expected) = expected
            && !outcome.empty
        {
            reads_back(&candidate, block, user, &outcome, &expected)?;
        }
        let (row, offset) = match (caret_in(&candidate, block, user, &outcome), style) {
            _ if outcome.empty => (0, 0),
            (Some(caret), _) => caret,
            (None, Some(_)) => anchor.unwrap_or((0, 0)),
            (None, None) => {
                return Err("the block laid out again has no place for the caret".into());
            }
        };
        let drawn_from = candidate
            .index
            .blocks
            .get(block)
            .and_then(|owner| owner.lines.get(row))
            .and_then(|line| pdf_cli::substituted_family_on_row(&candidate, *line, offset));
        self.session
            .apply_holding(plan, Some(Arc::new(candidate)))
            .map_err(|error| error.to_string())?;
        self.laid = Some((page, overlay_from(overlay)));
        Ok(Typed {
            anchor,
            block,
            row,
            offset,
            frame: Some(resized),
            edges: outcome.edges,
            breaks: Some(outcome.breaks.clone()),
            overflow: outcome.overflow,
            brought_in: outcome.brought_in.first().cloned(),
            drawn_from,
            cropped: outcome.cropped,
            note: None,
            status: None,
        })
    }

    fn frame_after(
        frame: [f64; 4],
        laid: [f64; 4],
        outcome: &BlockOutcome,
        declared: bool,
    ) -> [f64; 4] {
        if outcome.empty {
            return frame;
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "a count of empty lines in one block"
        )]
        let below = outcome.edges.1 as f64 * outcome.pitch * OVERLAY_SCALE;
        grown_frame(frame, laid, below, declared)
    }

    fn plan_delete(
        &mut self,
        page: usize,
        line: usize,
        from: usize,
        to: usize,
        close_gap: bool,
    ) -> Result<(pdf_edit::Plan, usize), String> {
        let target = match self.session.page(page) {
            Ok(view) => page_selection_between_view(&view, line, from, to, close_gap),
            Err(error) => Err(error.to_string()),
        }?;
        let clusters = target.clusters;
        self.session
            .plan(&Command::RewriteText {
                page_index: page,
                runs: target.runs,
            })
            .map(|plan| (plan, clusters))
            .map_err(|error| error.to_string())
    }

    fn delete_block(&mut self, page: usize, anchors: &[String]) -> (Applied, Message) {
        let runs = match decoded_anchors(anchors) {
            Ok(runs) => runs,
            Err(reason) => return refused(reason),
        };
        let removals = match self.session.page(page) {
            Ok(view) => pdf_cli::page_block_removal_view(&view, &runs),
            Err(error) => Err(error.to_string()),
        };
        let removals = match removals {
            Ok(removals) => removals,
            Err(reason) => return refused(reason),
        };
        let glyphs: usize = removals
            .iter()
            .map(|run| match &run.glyphs {
                Some(pdf_edit::GlyphChange::Remove { glyphs, .. }) => glyphs.len(),
                _ => 0,
            })
            .sum();
        let planned = self.session.plan(&Command::RewriteText {
            page_index: page,
            runs: removals,
        });
        let plan = match planned {
            Ok(plan) => plan,
            Err(error) => {
                return refused(why_a_block_will_not_move(&error));
            }
        };
        let capability = format!("{:?}", plan.capability());
        if let Err(error) = self.session.apply(plan) {
            return refused(error.to_string());
        }
        (
            self.landed(),
            Done::DeletedBlock { glyphs, capability }.into(),
        )
    }

    fn delete_group(
        &mut self,
        page: usize,
        anchors: &[String],
        objects: &[String],
    ) -> (Applied, Message) {
        let runs = match decoded_anchors(anchors) {
            Ok(runs) => runs,
            Err(reason) => return refused(reason),
        };
        let targets = match decoded_anchors(objects) {
            Ok(targets) => targets,
            Err(reason) => return refused(reason),
        };
        let removals = if runs.is_empty() {
            Ok(Vec::new())
        } else {
            match self.session.page(page) {
                Ok(view) => pdf_cli::page_block_removal_view(&view, &runs),
                Err(error) => Err(error.to_string()),
            }
        };
        let removals = match removals {
            Ok(removals) => removals,
            Err(reason) => return refused(reason),
        };
        let planned = self.session.plan(&Command::DeleteGroup {
            page_index: page,
            runs: removals,
            objects: targets,
        });
        let plan = match planned {
            Ok(plan) => plan,
            Err(error) => return refused(why_a_block_will_not_move(&error)),
        };
        let capability = format!("{:?}", plan.capability());
        let pieces = plan.effect().moved.len();
        if let Err(error) = self.session.apply(plan) {
            return refused(error.to_string());
        }
        (
            self.landed(),
            Done::DeletedGroup { pieces, capability }.into(),
        )
    }

    fn run_page_change(&mut self) -> (Applied, Message) {
        let command = match &self.step {
            Step::RemovePages { pages } => Command::RemovePages {
                pages: pages.clone(),
            },
            Step::MovePages { pages, to } => Command::MovePages {
                pages: pages.clone(),
                to: *to,
            },
            Step::RotatePages {
                pages,
                quarter_turns,
            } => Command::RotatePages {
                pages: pages.clone(),
                quarter_turns: *quarter_turns,
            },
            Step::InsertPages {
                beside,
                before,
                document,
                pages,
            } => Command::InsertPages {
                beside: *beside,
                before: *before,
                document: Arc::clone(document),
                pages: pages.clone(),
            },
            _ => return (Applied::Unchanged, Done::NothingChanged.into()),
        };
        self.change_pages(&command)
    }

    fn run_stamp(&mut self) -> (Applied, Message) {
        let Step::Stamp {
            pages,
            stamp,
            start,
            name,
            today,
        } = &self.step
        else {
            return (Applied::Unchanged, Done::NothingChanged.into());
        };
        let count = match self.session.page_count() {
            Ok(count) => i64::try_from(count).unwrap_or(i64::MAX),
            Err(error) => return refused(error.to_string()),
        };
        let first = pages.first().copied().unwrap_or(0);
        let commands: Vec<Command> = pages
            .iter()
            .map(|&page_index| Command::Stamp {
                page_index,
                stamp: stamp.clone(),
                facts: pdf_edit::stamp::Facts {
                    number: start
                        .saturating_add(i64::try_from(page_index - first).unwrap_or(i64::MAX)),
                    count,
                    name: name.clone(),
                    today: today.clone(),
                },
                share_from: (page_index != first).then_some(first),
            })
            .collect();
        let stamped = pages.len();
        if let Err(error) = self.session.apply_each(&commands) {
            return refused(why_a_stamp_is_refused(&error));
        }
        (
            Applied::Changed {
                page: first,
                region: None,
            },
            Done::Stamped { count: stamped }.into(),
        )
    }

    fn run_text_layers(&mut self) -> (Applied, Message) {
        let Step::TextLayers {
            layers,
            confidence,
            had_text,
            unread,
        } = &self.step
        else {
            return (Applied::Unchanged, Done::NothingChanged.into());
        };
        let (confidence, had_text) = (*confidence, *had_text);
        let mut kept = Vec::new();
        let mut refused_pages = *unread;
        let mut why = None;
        for (page_index, layer) in layers {
            let alone = Command::TextLayer {
                page_index: *page_index,
                layer: layer.clone(),
                share_from: None,
            };
            match self.session.plan(&alone) {
                Ok(_) => kept.push((*page_index, layer.clone())),
                Err(error) => {
                    refused_pages += 1;
                    why.get_or_insert_with(|| error.to_string());
                }
            }
        }
        let Some(&(first, _)) = kept.first() else {
            return refused(format!(
                "none of the {} pages read could be written: {}",
                layers.len(),
                why.unwrap_or_default()
            ));
        };
        let commands: Vec<Command> = kept
            .into_iter()
            .map(|(page_index, layer)| Command::TextLayer {
                page_index,
                layer,
                share_from: (page_index != first).then_some(first),
            })
            .collect();
        let written = commands.len();
        if let Err(error) = self.session.apply_each(&commands) {
            return refused(error.to_string());
        }
        (
            Applied::Changed {
                page: first,
                region: None,
            },
            Done::Recognized {
                pages: written,
                confidence,
                had_text,
                refused: refused_pages,
            }
            .into(),
        )
    }

    fn change_pages(&mut self, command: &Command) -> (Applied, Message) {
        let plan = match self.session.plan(command) {
            Ok(plan) => plan,
            Err(error) => return refused(why_a_block_will_not_move(&error)),
        };
        let change = plan.pages().cloned();
        if let Err(error) = self.session.apply(plan) {
            return refused(error.to_string());
        }
        let (page, done) = match (command, change) {
            (Command::RotatePages { pages, .. }, _) => (
                command.page_index(),
                Done::RotatedPages { count: pages.len() },
            ),
            (Command::InsertPages { pages, .. }, Some(change)) => (
                change.first(),
                Done::InsertedPages {
                    count: pages.len(),
                    at: change.first() + 1,
                },
            ),
            (Command::MovePages { pages, to }, _) => (
                *to,
                Done::MovedPages {
                    count: pages.len(),
                    to: to + 1,
                },
            ),
            (_, Some(change)) => (
                change.first(),
                Done::RemovedPages {
                    count: match &change {
                        PageChange::Removed(gone) => gone.len(),
                        _ => 0,
                    },
                },
            ),
            _ => return refused("the page command changed no page"),
        };
        (Applied::Changed { page, region: None }, done.into())
    }

    fn describe(&mut self, edit: &pdf_edit::info::InfoEdit) -> (Applied, Message) {
        let command = Command::SetDocumentInfo { edit: edit.clone() };
        let plan = match self.session.plan(&command) {
            Ok(plan) => plan,
            Err(error) => return refused(why_a_block_will_not_move(&error)),
        };
        if let Err(error) = self.session.apply(plan) {
            return refused(error.to_string());
        }
        (
            Applied::Changed {
                page: 0,
                region: None,
            },
            Done::Described.into(),
        )
    }

    fn add_page(&mut self, beside: usize, before: bool, size: [f64; 2]) -> (Applied, Message) {
        let planned = self.session.plan(&Command::AddBlankPage {
            beside,
            before,
            size,
        });
        let plan = match planned {
            Ok(plan) => plan,
            Err(error) => return refused(why_a_block_will_not_move(&error)),
        };
        let at = plan.pages().map_or(beside, PageChange::first);
        if let Err(error) = self.session.apply(plan) {
            return refused(error.to_string());
        }
        (
            Applied::Changed {
                page: at,
                region: None,
            },
            Done::AddedPage { page: at + 1 }.into(),
        )
    }

    fn place_text(
        &mut self,
        page: usize,
        frame: [f64; 4],
        text: &str,
        style: &NewTextStyle,
    ) -> (Applied, Message) {
        let planned = self.session.plan(&Command::PlaceNewText {
            page_index: page,
            frame,
            text: text.to_owned(),
            family: style.family.clone(),
            size: style.size,
            bold: style.bold,
            italic: style.italic,
            fill: style.fill,
            paragraph: style.paragraph,
        });
        let plan = match planned {
            Ok(plan) => plan,
            Err(error) => return refused(why_a_block_will_not_move(&error)),
        };
        let anchor = plan.effect().first().anchor.encode();
        if let Err(error) = self.session.apply(plan) {
            return refused(error.to_string());
        }
        self.wrote_text = Some(anchor);
        (
            self.landed(),
            Done::AddedText {
                characters: text.chars().count(),
            }
            .into(),
        )
    }

    fn place_new_images(
        &mut self,
        page: usize,
        mut pictures: Vec<(pdf_paint::Matrix, Arc<[u8]>)>,
    ) -> (Applied, Message) {
        if pictures.len() > 1 {
            let count = pictures.len();
            let commands: Vec<Command> = pictures
                .into_iter()
                .map(|(placement, file)| Command::PlaceNewImage {
                    page_index: page,
                    placement,
                    file,
                })
                .collect();
            if let Err(error) = self.session.apply_each(&commands) {
                return refused(why_a_block_will_not_move(&pdf_session::PlanError::Plan(
                    error,
                )));
            }
            return (
                Applied::Changed { page, region: None },
                Done::AddedPictures { count }.into(),
            );
        }
        let Some((placement, file)) = pictures.pop() else {
            return (Applied::Unchanged, Done::NothingChanged.into());
        };
        let planned = self.session.plan(&Command::PlaceNewImage {
            page_index: page,
            placement,
            file,
        });
        let plan = match planned {
            Ok(plan) => plan,
            Err(error) => return refused(why_a_block_will_not_move(&error)),
        };
        if let Err(error) = self.session.apply(plan) {
            return refused(error.to_string());
        }
        (self.landed(), Done::AddedPicture.into())
    }

    fn flow_round(&mut self, page: usize, blocks: &[usize]) -> (Applied, Message) {
        if blocks.is_empty() {
            return (Applied::Unchanged, Message::NothingInTheWay);
        }
        for block in blocks {
            if paragraph_of(&self.paragraphs, page, *block)
                .alignment
                .is_none()
                && let Some(reading) = self.block_reading(page, *block)
            {
                self.paragraphs.entry((page, *block)).or_default().alignment =
                    Some(reading.alignment);
            }
            self.paragraphs
                .entry((page, *block))
                .or_default()
                .flow_round = true;
        }
        match self.lay_out_again(page, blocks) {
            Ok(count) => (
                Applied::Changed { page, region: None },
                Message::TextMadeWay { blocks: count },
            ),
            Err(reason) => refused(reason.as_str()),
        }
    }

    fn lay_out_again(&mut self, page: usize, blocks: &[usize]) -> Result<usize, String> {
        for block in blocks {
            let command = self.relay_out(page, *block)?;
            self.session
                .apply_each(std::slice::from_ref(&command))
                .map_err(|error| error.to_string())?;
            let grown = self.grown_frames(page, std::slice::from_ref(block));
            self.frames.source_relaid(page, &grown);
        }
        Ok(blocks.len())
    }

    fn keep_dragged_frame(
        &mut self,
        page: usize,
        block: usize,
        started: [f64; 4],
        why: String,
    ) -> (Applied, Message) {
        self.frames.finish_resize(page, block, started);
        let at = self
            .frames
            .page(page)
            .get(block)
            .copied()
            .unwrap_or(started);
        (
            Applied::Unchanged,
            Message::FrameKeptNotRelaid {
                wide: at[2] - at[0],
                high: at[3] - at[1],
                why,
            },
        )
    }

    fn resize_and_relay(
        &mut self,
        page: usize,
        block: usize,
        started: [f64; 4],
    ) -> (Applied, Message) {
        if paragraph_of(&self.paragraphs, page, block)
            .alignment
            .is_none()
            && let Some(reading) = self.block_reading(page, block)
        {
            self.paragraphs.entry((page, block)).or_default().alignment = Some(reading.alignment);
        }
        let typed = self
            .session
            .page(page)
            .map_err(|error| Edited::from(error.to_string()))
            .and_then(|view| {
                self.type_in_block(
                    &view,
                    page,
                    block,
                    BlockRange::Between {
                        from: (0, 0),
                        to: (0, 0),
                    },
                    ("", None),
                    Some(&pdf_edit::TextStyle::default()),
                )
            });
        let typed = match typed {
            Ok(typed) => typed,
            Err(Edited::Nothing) => {
                return self.keep_dragged_frame(
                    page,
                    block,
                    started,
                    "the block is empty".to_owned(),
                );
            }
            Err(Edited::Refused(reason)) => {
                return self.keep_dragged_frame(page, block, started, reason.say(Lang::English));
            }
        };
        let Some(frame) = typed.frame else {
            return self.keep_dragged_frame(
                page,
                block,
                started,
                "the row was edited alone".to_owned(),
            );
        };
        self.frames
            .resized_and_relaid(page, block, started, frame, typed.edges, typed.breaks);
        (
            self.landed(),
            Message::FrameDeclared {
                wide: frame[2] - frame[0],
                high: frame[3] - frame[1],
                relaid: true,
            },
        )
    }

    fn relay_out(&mut self, page: usize, block: usize) -> Result<Command, String> {
        let view = self.session.page(page).map_err(|error| error.to_string())?;
        let rows = block_rows(&view, block).ok_or("row has no block owner")?;
        if rows.iter().all(Vec::is_empty) {
            return Err("the block is empty".to_owned());
        }
        let frame = *self
            .frames
            .page(page)
            .get(block)
            .ok_or("no frame for this block")?;
        let user = frame_edges_in_user_space(&view, frame).map_err(|error| error.to_string())?;
        let breaks = self
            .frames
            .breaks(page, block)
            .or_else(|| Some(pdf_edit::RowEnds::paragraphs((0..rows.len()).collect())));
        Ok(Command::StyleBlock {
            page_index: page,
            rows,
            frame: user,
            edges: self.frames.edges(page, block),
            breaks,
            range: BlockRange::Between {
                from: (0, 0),
                to: (0, 0),
            },
            style: pdf_edit::TextStyle::default(),
            paragraph: paragraph_of(&self.paragraphs, page, block),
        })
    }

    fn block_reading(&mut self, page: usize, block: usize) -> Option<pdf_edit::BlockReading> {
        let frame = *self.frames.page(page).get(block)?;
        let (edges, breaks) = (
            self.frames.edges(page, block),
            self.frames.breaks(page, block),
        );
        let view = self.session.page(page).ok()?;
        let user = frame_edges_in_user_space(&view, frame).ok()?;
        let rows = block_rows(&view, block)?;
        pdf_edit::read_block(
            &view.program,
            &view.graph,
            &rows,
            user,
            edges,
            breaks.as_ref(),
        )
        .ok()
    }

    fn rows_standing_in(&mut self, page: usize, block: usize) -> Option<Vec<pdf_edit::Blocked>> {
        let reading = self.block_reading(page, block)?;
        let first = reading.lines.first()?.origin.1;
        let frame = *self.frames.page(page).get(block)?;
        let view = self.session.page(page).ok()?;
        let edges = frame_edges_in_user_space(&view, frame).ok()?;
        Some(pdf_edit::blocked_for_block(
            &view.graph,
            view.program.geometry.crop_box[1],
            edges,
            (first, reading.pitch),
        ))
    }

    fn grown_frames(&mut self, page: usize, blocks: &[usize]) -> Vec<(usize, [f64; 4])> {
        let Ok(view) = self.session.page(page) else {
            return Vec::new();
        };
        let Ok(overlay) = page_overlay_view(&view, OVERLAY_SCALE) else {
            return Vec::new();
        };
        let mut grown = Vec::with_capacity(blocks.len());
        for block in blocks {
            let (Some(frame), Some(laid)) = (
                self.frames.page(page).get(*block).copied(),
                overlay.blocks.get(*block).map(|held| held.layout_pixels),
            ) else {
                continue;
            };
            #[expect(
                clippy::cast_precision_loss,
                reason = "a count of empty lines in one block"
            )]
            let below = self.frames.edges(page, *block).1 as f64
                * self
                    .block_reading(page, *block)
                    .map_or(pdf_edit::WRAP_ROW, |reading| reading.pitch)
                * OVERLAY_SCALE;
            let declared = self.frames.is_declared(page, *block);
            grown.push((*block, grown_frame(frame, laid, below, declared)));
        }
        grown
    }

    fn keep_the_flow(&mut self, applied: &Applied) -> Option<Message> {
        let Applied::Changed { page, .. } = *applied else {
            return None;
        };
        let walking = matches!(self.step, Step::Undo | Step::Redo);
        let blocks: Vec<usize> = self
            .paragraphs
            .iter()
            .filter(|((at, _), paragraph)| *at == page && paragraph.flow_round)
            .map(|((_, block), _)| *block)
            .collect();
        self.flowed
            .retain(|(at, block), _| *at != page || blocks.binary_search(block).is_ok());
        let mut again = Vec::new();
        let mut laid = Vec::new();
        for block in blocks {
            let Some(rows) = self.rows_standing_in(page, block) else {
                continue;
            };
            if !walking
                && self
                    .flowed
                    .get(&(page, block))
                    .is_some_and(|was| *was != rows)
            {
                again.push(block);
            }
            laid.push(((page, block), rows));
        }
        let count = again.len();
        if count > 0 && self.lay_out_again(page, &again).is_err() {
            return Some(Message::FlowRoundFellBehind);
        }
        self.flowed.extend(laid);
        (count > 0).then_some(Message::TextMadeWay { blocks: count })
    }

    fn form_edit(&mut self, command: &Command, done: Done) -> (Applied, Message) {
        let plan = match self.session.plan(command) {
            Ok(plan) => plan,
            Err(error) => return refused(why_a_block_will_not_move(&error)),
        };
        if let Err(error) = self.session.apply(plan) {
            return refused(error.to_string());
        }
        (self.landed(), done.into())
    }

    fn fill_field(
        &mut self,
        page: usize,
        widget: pdf_syntax::Reference,
        value: pdf_edit::form::FieldValue,
    ) -> (Applied, Message) {
        let cleared = value == pdf_edit::form::FieldValue::Empty;
        let planned = self.session.plan(&Command::FillField {
            page_index: page,
            widget,
            value,
        });
        let plan = match planned {
            Ok(plan) => plan,
            Err(error) => return refused(why_a_block_will_not_move(&error)),
        };
        if let Err(error) = self.session.apply(plan) {
            return refused(error.to_string());
        }
        let said = if cleared {
            Done::ClearedField
        } else {
            Done::FilledField
        };
        (self.landed(), said.into())
    }

    fn draw_new_path(&mut self, page: usize, drawn: Drawn) -> (Applied, Message) {
        let drew = drawn.drew;
        let planned = self.session.plan(&Command::DrawPath {
            page_index: page,
            steps: drawn.steps,
            closed: drawn.closed,
            stroke: drawn.stroke,
            fill: drawn.fill,
        });
        let plan = match planned {
            Ok(plan) => plan,
            Err(error) => return refused(why_a_block_will_not_move(&error)),
        };
        if let Err(error) = self.session.apply(plan) {
            return refused(error.to_string());
        }
        let said = match drew {
            Drew::Mark => Done::MarkedThePage,
            Drew::Shape => Done::DrewShape,
            Drew::Line => Done::DrewLine,
        };
        (self.landed(), said.into())
    }

    fn reorder_objects(
        &mut self,
        page: usize,
        anchors: &[String],
        order: pdf_edit::Stacking,
    ) -> (Applied, Message) {
        let mut targets = Vec::with_capacity(anchors.len());
        for anchor in anchors {
            let Some(decoded) = SourceAnchor::decode(anchor) else {
                return refused("malformed anchor");
            };
            targets.push(decoded);
        }
        let count = targets.len();
        let planned = self.session.plan(&Command::ReorderObjects {
            page_index: page,
            targets,
            order,
        });
        let plan = match planned {
            Ok(plan) => plan,
            Err(error) => return refused(error.to_string()),
        };
        let capability = format!("{:?}", plan.capability());
        if let Err(error) = self.session.apply(plan) {
            return refused(error.to_string());
        }
        (
            self.landed(),
            Done::Reordered {
                order,
                count,
                capability,
            }
            .into(),
        )
    }

    fn paste(
        &mut self,
        page: usize,
        copied: Copied,
        (dx, dy): (f64, f64),
        elsewhere: Option<pdf_bytes::ByteStore>,
    ) -> (Applied, Message) {
        let objects = copied.objects.len();
        if objects == 0 {
            return (Applied::Unchanged, Done::NothingToPaste.into());
        }
        let offset = match self.session.page(page) {
            Ok(view) => page_offset_view(&view, OVERLAY_SCALE, dx, dy),
            Err(error) => Err(error.to_string()),
        };
        let (dx, dy) = match offset {
            Ok(offset) => offset,
            Err(reason) => return refused(reason),
        };
        let planned = self.session.plan(&Command::PasteObjects {
            page_index: page,
            copied,
            dx,
            dy,
            elsewhere,
        });
        let plan = match planned {
            Ok(plan) => plan,
            Err(error) => return refused(why_a_paste_is_refused(&error)),
        };
        if let Err(error) = self.session.apply(plan) {
            return refused(error.to_string());
        }
        (self.landed(), Done::Pasted { objects }.into())
    }

    fn remove_object(&mut self, page: usize, anchor: &str, rubbing: bool) -> (Applied, Message) {
        let Some(target) = SourceAnchor::decode(anchor) else {
            return refused("malformed anchor");
        };
        let planned = self.session.plan(&Command::RemoveObject {
            page_index: page,
            target,
        });
        let plan = match planned {
            Ok(plan) => plan,
            Err(error) => {
                return refused(why_a_block_will_not_move(&error));
            }
        };
        let capability = format!("{:?}", plan.capability());
        if let Err(error) = self.session.apply(plan) {
            return refused(error.to_string());
        }
        let said = if rubbing {
            Done::DeletedDrawing
        } else {
            Done::RemovedPicture { capability }
        };
        (self.landed(), said.into())
    }

    fn walk(&mut self, backwards: bool) -> (Applied, Message) {
        match self.frames.source_step(backwards) {
            None => return (Applied::Unchanged, Done::NothingToWalk.into()),
            Some(false) => {
                self.frames.walk(backwards);
                return (Applied::Unchanged, Done::FrameRestored.into());
            }
            Some(true) => {}
        }
        let walked = if backwards {
            self.session.undo()
        } else {
            self.session.redo()
        };
        match walked {
            Ok(true) => {
                self.frames.walk(backwards);
                (
                    self.landed(),
                    if backwards {
                        Done::Undone
                    } else {
                        Done::Redone
                    }
                    .into(),
                )
            }
            Ok(false) => (
                Applied::Unchanged,
                if backwards {
                    Done::NothingToUndo
                } else {
                    Done::NothingToRedo
                }
                .into(),
            ),
            Err(error) => refused(error.to_string()),
        }
    }

    fn plan_move(
        &mut self,
        page: usize,
        anchor: &str,
        dx: f64,
        dy: f64,
    ) -> Result<pdf_edit::Plan, String> {
        let decoded = SourceAnchor::decode(anchor).ok_or("malformed anchor")?;
        let view = self.session.page(page).map_err(|error| error.to_string())?;
        let (dx, dy) = page_run_offset_view(&view, anchor, OVERLAY_SCALE, dx, dy)?;
        drop(view);
        self.session
            .plan(&Command::MoveTextRun {
                page_index: page,
                selection: TextRunSelection::Anchored(decoded),
                dx,
                dy,
            })
            .map_err(|error| error.to_string())
    }

    fn landed(&self) -> Applied {
        let history = self.session.history();
        match history.last_page() {
            Some(page) => Applied::Changed {
                page,
                region: history.last_region(),
            },
            None => Applied::Unchanged,
        }
    }
}

fn why_a_stamp_is_refused(error: &SpikeError) -> Refusal {
    if let SpikeError::RetypeUnsupported(reason) = error
        && let Some(why) = crate::wording::StampWhy::of(reason)
    {
        return Refusal::Stamp(why);
    }
    why_a_block_will_not_move(&pdf_session::PlanError::Plan(error.clone()))
}

fn decoded_anchors(anchors: &[String]) -> Result<Vec<SourceAnchor>, String> {
    anchors
        .iter()
        .map(|anchor| SourceAnchor::decode(anchor).ok_or_else(|| "malformed anchor".to_owned()))
        .collect()
}

const fn how_hidden(showing: pdf_edit::Showing) -> Hidden {
    match showing {
        pdf_edit::Showing::Whole => Hidden::Nothing,
        pdf_edit::Showing::PartlyHidden => Hidden::Part,
        pdf_edit::Showing::OutOfSight => Hidden::All,
    }
}

fn why_a_paste_is_refused(error: &pdf_session::PlanError) -> Refusal {
    match error {
        pdf_session::PlanError::Plan(SpikeError::RetypeUnsupported(reason)) => {
            Refusal::Engine(format!("cannot paste: {reason}"))
        }
        pdf_session::PlanError::Plan(error @ SpikeError::PasteNotIsolated) => {
            Refusal::Engine(format!("cannot paste: {error}"))
        }
        other => why_a_block_will_not_move(other),
    }
}

fn why_a_block_will_not_move(error: &pdf_session::PlanError) -> Refusal {
    let pdf_session::PlanError::Plan(error) = error else {
        return error.to_string().into();
    };
    Refusal::BlockWillNotMove(match error {
        SpikeError::ClipNotRectangular => BlockMove::ClipHasNoArea,
        SpikeError::ClipIsCurved => BlockMove::ClipIsCurved,
        SpikeError::ClipIsConcave => BlockMove::ClipIsConcave,
        SpikeError::RunExtentUnknown => BlockMove::FontNotEmbedded,
        SpikeError::BlockSpansSeveralStreams => BlockMove::SeveralStreams,
        SpikeError::BlockRunInsideForm => BlockMove::InsideForm,
        SpikeError::BlockNotChainAligned => BlockMove::FirstRunMidLine,
        SpikeError::BlockRunNotInTargetStream => BlockMove::OutsideTheStream,
        SpikeError::BlockOffsetNotRepresentable => BlockMove::OffsetNotRepresentable,
        SpikeError::MoveNotIsolated => BlockMove::WouldTouchOthers,
        SpikeError::MoveNotProvable => BlockMove::NotProvable,
        SpikeError::SharedPageContentStream => BlockMove::SharedStream,
        SpikeError::RunNotDirectlyOnPage => BlockMove::DrawnThroughPattern,
        SpikeError::ObjectIsCropped => BlockMove::AlreadyCropped,
        SpikeError::ObjectLeavesClip => BlockMove::PlacementLeavesClip,
        SpikeError::PlacementNotInvertible | SpikeError::ObjectCtmSingular => {
            BlockMove::PlacementFlat
        }
        SpikeError::AnchorNamesNothing | SpikeError::BlockNamesNoRun => BlockMove::SelectionGone,
        other => BlockMove::Other(other.to_string()),
    })
}

fn why_a_picture_will_not_move(error: &pdf_session::PlanError) -> Refusal {
    let pdf_session::PlanError::Plan(error) = error else {
        return error.to_string().into();
    };
    Refusal::PictureWillNotMove(match error {
        SpikeError::ObjectLeavesClip => PictureMove::LeavesClip,
        SpikeError::ObjectIsCropped => PictureMove::ClipNotSeparable,
        SpikeError::ClipNotRectangular => PictureMove::HeldClipHasNoArea,
        SpikeError::ClipIsCurved => PictureMove::HeldClipIsCurved,
        SpikeError::ClipIsConcave => PictureMove::HeldClipIsConcave,
        SpikeError::ObjectInsideForm => PictureMove::InsideForm,
        SpikeError::RunNotDirectlyOnPage => PictureMove::DrawnThroughPattern,
        SpikeError::SharedPageContentStream => PictureMove::SharedStream,
        SpikeError::ObjectCtmSingular | SpikeError::PlacementNotInvertible => {
            PictureMove::MatrixHasNoArea
        }
        SpikeError::MoveNotIsolated => PictureMove::WouldTouchOthers,
        SpikeError::MoveNotProvable => PictureMove::NotProvable,
        SpikeError::AnchorNamesNothing => PictureMove::PictureGone,
        other => PictureMove::Other(other.to_string()),
    })
}

fn in_pixels(device: &pdf_paint::Matrix, [x0, y0, x1, y1]: [f64; 4]) -> [f64; 4] {
    let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
        .map(|(x, y)| device.transform(pdf_paint::Point { x, y }));
    [
        corners.iter().map(|at| at.x).fold(f64::INFINITY, f64::min),
        corners.iter().map(|at| at.y).fold(f64::INFINITY, f64::min),
        corners
            .iter()
            .map(|at| at.x)
            .fold(f64::NEG_INFINITY, f64::max),
        corners
            .iter()
            .map(|at| at.y)
            .fold(f64::NEG_INFINITY, f64::max),
    ]
}

fn links_of(source: &ByteStore, credential: &[u8]) -> Option<Arc<pdf_content::LinkResolver>> {
    pdf_content::open_link_resolver_tolerating_damage(
        source,
        pdf_content::PageContentLimits::default(),
        pdf_content::RecoverLimits::default(),
        credential,
    )
    .ok()
    .map(|found| Arc::new(found.into_parts().0))
}

fn overlay_of(view: &PageView) -> Result<Overlay, String> {
    page_overlay_view(view, OVERLAY_SCALE).map(overlay_from)
}

fn overlay_from(overlay: pdf_cli::PageOverlay) -> Overlay {
    Overlay {
        runs: overlay
            .runs
            .into_iter()
            .map(|run| RunBox {
                anchor: run.anchor,
                bounds: run.box_pixels,
                text: run.text,
                em: run.em,
                fill: run.fill,
                family: run.family,
            })
            .collect(),
        clusters: overlay.clusters,
        carets: overlay.carets,
        blocks: overlay.blocks,
        objects: overlay.objects,
        rows: overlay.rows,
    }
}

fn empty_show_anchor(
    view: &pdf_session::PageView,
    frame: [f64; 4],
) -> Result<SourceAnchor, Edited> {
    empty_show_in(view, frame)
        .map(|(atom, ..)| SourceAnchor::of(&view.graph.atoms[atom].id))
        .ok_or_else(|| "the emptied block's show is not on the page".into())
}

fn empty_block_command(
    page_index: usize,
    run: SourceAnchor,
    frame: (f64, f64),
    text: &str,
    typed: Option<&pdf_edit::TextStyle>,
    paragraph: pdf_edit::ParagraphLayout,
) -> Command {
    Command::RewriteEmptyBlock {
        page_index,
        run,
        frame,
        text: text.to_owned(),
        style: typed.cloned(),
        paragraph,
    }
}

fn block_command(
    page_index: usize,
    (rows, frame, edges, breaks): (Vec<Vec<pdf_edit::ClusterRef>>, (f64, f64), Edges, Breaks),
    range: BlockRange,
    (text, typed, style): (
        &str,
        Option<&pdf_edit::TextStyle>,
        Option<&pdf_edit::TextStyle>,
    ),
    frame_declared: bool,
    paragraph: pdf_edit::ParagraphLayout,
) -> Command {
    let text = text.to_owned();
    match (style, typed) {
        (Some(style), _) => Command::StyleBlock {
            page_index,
            rows,
            frame,
            edges,
            breaks,
            range,
            style: style.clone(),
            paragraph,
        },
        (None, Some(typed)) => Command::RewriteBlockInStyle {
            page_index,
            rows,
            frame,
            edges,
            breaks,
            frame_declared,
            range,
            text,
            style: typed.clone(),
            paragraph,
        },
        (None, None) => Command::RewriteBlock {
            page_index,
            rows,
            frame,
            edges,
            breaks,
            frame_declared,
            range,
            text,
            paragraph,
        },
    }
}

fn frame_edges_in_user_space(
    view: &pdf_session::PageView,
    frame: [f64; 4],
) -> Result<(f64, f64), Refusal> {
    let rect = frame_in_user_space(view, frame)?;
    Ok((rect[0], rect[2]))
}

fn frame_in_user_space(view: &pdf_session::PageView, frame: [f64; 4]) -> Result<[f64; 4], Refusal> {
    let device = pdf_render::DeviceTransform::for_page(
        &view.program.geometry,
        OVERLAY_SCALE,
        pdf_render::RenderLimits::default(),
    )
    .map_err(|_| Refusal::PageHasNoSize)?;
    let matrix = device.matrix;
    if matrix.b.abs() > 1e-9 || matrix.c.abs() > 1e-9 {
        return Err(Refusal::TurnedPageCannotBeLaidOut);
    }
    let inverse = matrix.inverse().ok_or(Refusal::PageHasNoSize)?;
    let one = inverse.transform(pdf_paint::Point {
        x: frame[0],
        y: frame[1],
    });
    let other = inverse.transform(pdf_paint::Point {
        x: frame[2],
        y: frame[3],
    });
    Ok([
        one.x.min(other.x),
        one.y.min(other.y),
        one.x.max(other.x),
        one.y.max(other.y),
    ])
}

fn caret_in(
    view: &pdf_session::PageView,
    block: usize,
    frame: (f64, f64),
    outcome: &BlockOutcome,
) -> Option<(usize, usize)> {
    planned_in(view, block, frame, outcome, outcome.caret?)
}

fn planned_in(
    view: &pdf_session::PageView,
    block: usize,
    frame: (f64, f64),
    outcome: &BlockOutcome,
    planned: PlannedCaret,
) -> Option<(usize, usize)> {
    let reading = pdf_edit::read_block(
        &view.program,
        &view.graph,
        &block_rows(view, block)?,
        frame,
        outcome.edges,
        Some(&outcome.breaks),
    )
    .ok()?;
    match planned {
        PlannedCaret::EmptyLine { line } => reading
            .lines
            .get(line)
            .is_some_and(|read| read.row.is_none())
            .then_some((line, 0)),
        PlannedCaret::Beside { cluster, after } => {
            let (row, offset) = view
                .index
                .blocks
                .get(block)?
                .lines
                .iter()
                .enumerate()
                .find_map(|(row, line)| {
                    view.index.lines[*line]
                        .clusters
                        .iter()
                        .position(|held| {
                            let held = &view.index.clusters[*held];
                            held.atom == cluster.atom && held.glyphs.contains(&cluster.glyph)
                        })
                        .map(|offset| (row, offset + usize::from(after)))
                })?;
            let line = reading
                .lines
                .iter()
                .position(|read| read.row == Some(row))?;
            Some((line, offset))
        }
    }
}

fn expected_reading(
    view: &pdf_session::PageView,
    rows: &[Vec<ClusterRef>],
    (frame, edges, breaks): ((f64, f64), (usize, usize), Option<&pdf_edit::RowEnds>),
    range: BlockRange,
    text: &str,
) -> Option<String> {
    let BlockRange::Between { from, to } = range else {
        return None;
    };
    let before =
        pdf_edit::read_block(&view.program, &view.graph, rows, frame, edges, breaks).ok()?;
    let last = before.lines.len().checked_sub(1)?;
    let end = (last, before.lines[last].clusters.len());
    let (from, to) = if before.position(from)? <= before.position(to)? {
        (from, to)
    } else {
        (to, from)
    };
    let head = before.text_between((0, 0), from)?;
    let selected = before.text_between(from, to)?;
    let tail = before.text_between(to, end)?;
    let all: Vec<char> = format!("{head}{selected}{tail}").chars().collect();
    let first =
        head.chars().count() + pdf_edit::insertion_after_marks(&head, &format!("{selected}{tail}"));
    let second = head.chars().count()
        + selected.chars().count()
        + pdf_edit::insertion_after_marks(&format!("{head}{selected}"), &tail);
    let (first, second) = (first.min(second), first.max(second));
    Some(format!(
        "{}{text}{}",
        all[..first].iter().collect::<String>(),
        all[second..].iter().collect::<String>()
    ))
}

fn grown_frame(frame: [f64; 4], laid: [f64; 4], below: f64, declared: bool) -> [f64; 4] {
    let bottom = (laid[3] + below).max(frame[1] + 1.0);
    [
        frame[0],
        frame[1],
        frame[2],
        if declared {
            bottom.max(frame[3])
        } else {
            bottom
        },
    ]
}

fn whole_block(overlay: &Overlay, anchors: &[String]) -> Option<(usize, (usize, usize))> {
    let block = overlay
        .blocks
        .iter()
        .position(|block| block.anchors == anchors)?;
    let rows = &overlay.blocks[block].lines;
    let last = *rows.last()?;
    let offset = overlay
        .carets
        .iter()
        .filter(|stop| stop.line == last)
        .map(|stop| stop.offset)
        .max()?;
    Some((block, (rows.len() - 1, offset)))
}

fn reads_back(
    candidate: &pdf_session::PageView,
    block: usize,
    frame: (f64, f64),
    outcome: &BlockOutcome,
    expected: &str,
) -> Result<(), Edited> {
    let read = block_rows(candidate, block)
        .and_then(|rows| {
            pdf_edit::read_block(
                &candidate.program,
                &candidate.graph,
                &rows,
                frame,
                outcome.edges,
                Some(&outcome.breaks),
            )
            .ok()
        })
        .and_then(|after| {
            let last = after.lines.len().checked_sub(1)?;
            after.text_between((0, 0), (last, after.lines[last].clusters.len()))
        });
    if read.as_deref().map(without_space) == Some(without_space(expected)) {
        Ok(())
    } else {
        Err(Refusal::ReadsBackOtherwise {
            read: read.unwrap_or_default(),
        }
        .into())
    }
}

fn without_space(text: &str) -> String {
    pdf_edit::in_compatibility_form(text)
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn other_blocks_stay(
    before: &pdf_session::PageView,
    after: &pdf_session::PageView,
    block: usize,
) -> bool {
    let places = |view: &pdf_session::PageView, owner: &pdf_semantics::Block| {
        let mut points: Vec<(f64, f64)> = owner
            .lines
            .iter()
            .flat_map(|line| view.index.lines[*line].clusters.iter())
            .map(|cluster| {
                let point = view.index.clusters[*cluster].baseline;
                (point.x, point.y)
            })
            .collect();
        points.sort_by(|one, other| one.0.total_cmp(&other.0).then(one.1.total_cmp(&other.1)));
        points
    };
    before
        .index
        .blocks
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != block)
        .all(|(index, owner)| {
            let (was, now) = (
                places(before, owner),
                after
                    .index
                    .blocks
                    .get(index)
                    .map(|owner| places(after, owner)),
            );
            now.is_some_and(|now| {
                now.len() == was.len()
                    && was.iter().zip(&now).all(|(one, other)| {
                        (one.0 - other.0).abs() <= 0.01 && (one.1 - other.1).abs() <= 0.01
                    })
            })
        })
}

fn block_rows(view: &pdf_session::PageView, block: usize) -> Option<Vec<Vec<ClusterRef>>> {
    Some(
        view.index
            .blocks
            .get(block)?
            .lines
            .iter()
            .map(|line| {
                view.index.lines[*line]
                    .clusters
                    .iter()
                    .map(|cluster| {
                        let cluster = &view.index.clusters[*cluster];
                        ClusterRef {
                            anchor: SourceAnchor::of(&view.graph.atoms[cluster.atom].id),
                            glyphs: cluster.glyphs.clone(),
                        }
                    })
                    .collect()
            })
            .collect(),
    )
}

fn add_empty_lines(
    view: &pdf_session::PageView,
    overlay: &mut Overlay,
    frames: &[[f64; 4]],
    edges_of: impl Fn(usize) -> (Edges, Breaks),
) {
    if overlay.blocks.len() != view.index.blocks.len() {
        return;
    }
    let Ok(device) = pdf_render::DeviceTransform::for_page(
        &view.program.geometry,
        OVERLAY_SCALE,
        pdf_render::RenderLimits::default(),
    ) else {
        return;
    };
    let matrix = device.matrix;
    let mut next = overlay
        .carets
        .iter()
        .map(|stop| stop.line + 1)
        .max()
        .unwrap_or(0)
        .max(view.index.lines.len());
    for (block, owner) in view.index.blocks.iter().enumerate() {
        if owner.lines.is_empty() {
            if emptied_block_line(view, overlay, frames, block, next) {
                next += 1;
            }
            continue;
        }
        let (edges, breaks) = edges_of(block);
        if edges == (0, 0) && owner.lines.len() < 2 {
            continue;
        }
        let Some(frame) = frames.get(block) else {
            continue;
        };
        let (Ok(user), Some(rows)) = (
            frame_edges_in_user_space(view, *frame),
            block_rows(view, block),
        ) else {
            continue;
        };
        let Ok(reading) = pdf_edit::read_block(
            &view.program,
            &view.graph,
            &rows,
            user,
            edges,
            breaks.as_ref(),
        ) else {
            continue;
        };
        let owned = overlay.blocks[block].lines.clone();
        if reading.lines.iter().all(|read| read.row.is_some())
            || reading
                .lines
                .iter()
                .filter_map(|read| read.row)
                .any(|row| row >= owned.len())
        {
            continue;
        }
        let mut lines = Vec::with_capacity(reading.lines.len());
        for read in &reading.lines {
            if let Some(row) = read.row {
                lines.push(owned[row]);
                continue;
            }
            let at = matrix.transform(pdf_edit::turned_out_of(
                reading.turn,
                pdf_paint::Point {
                    x: read.origin.0,
                    y: read.origin.1,
                },
            ));
            let up = pdf_edit::turned_out_of(reading.turn, pdf_paint::Point { x: 0.0, y: read.em });
            overlay.carets.push(CaretStop {
                line: next,
                offset: 0,
                at: [at.x, at.y],
                up: [
                    matrix.a.mul_add(up.x, matrix.c * up.y),
                    matrix.b.mul_add(up.x, matrix.d * up.y),
                ],
            });
            lines.push(next);
            next += 1;
        }
        overlay.blocks[block].lines = lines;
    }
}

fn empty_show_in(
    view: &pdf_session::PageView,
    frame: [f64; 4],
) -> Option<(usize, [f64; 2], [f64; 2])> {
    let device = pdf_render::DeviceTransform::for_page(
        &view.program.geometry,
        OVERLAY_SCALE,
        pdf_render::RenderLimits::default(),
    )
    .ok()?;
    view.graph
        .atoms
        .iter()
        .enumerate()
        .find_map(|(atom, paint)| {
            let pdf_paint::PaintAtomKind::Text(text) = &paint.kind else {
                return None;
            };
            if !text.glyphs.is_empty()
                || !matches!(
                    text.state.text.rendering_mode.value,
                    pdf_paint::TextRenderingMode::Fill
                        | pdf_paint::TextRenderingMode::Stroke
                        | pdf_paint::TextRenderingMode::FillStroke
                )
            {
                return None;
            }
            let to_device = device
                .matrix
                .multiply(text.state.ctm.value)
                .multiply(text.matrices.text.value);
            let at = to_device.transform(pdf_paint::Point { x: 0.0, y: 0.0 });
            let size = text.state.text.font_size.value;
            let top = to_device.transform(pdf_paint::Point { x: 0.0, y: size });
            let up = [top.x - at.x, top.y - at.y];
            let slack = up[0].hypot(up[1]);
            let inside = at.x >= frame[0] - slack
                && at.x <= frame[2] + slack
                && at.y >= frame[1] - slack
                && at.y <= frame[3] + slack;
            inside.then_some((atom, [at.x, at.y], up))
        })
}

fn emptied_block_line(
    view: &pdf_session::PageView,
    overlay: &mut Overlay,
    frames: &[[f64; 4]],
    block: usize,
    next: usize,
) -> bool {
    if let Some((atom, at, up)) = frames
        .get(block)
        .and_then(|frame| empty_show_in(view, *frame))
    {
        if let pdf_paint::PaintAtomKind::Text(text) = &view.graph.atoms[atom].kind {
            let owner = &mut overlay.blocks[block];
            owner.font = text
                .font_request
                .as_ref()
                .map(|request| request.family.trim().to_owned())
                .filter(|family| !family.is_empty());
            owner.shape = text
                .placed_shape()
                .zip(text.size_on_page())
                .map(|(shape, size)| pdf_cli::BlockShape {
                    size,
                    turn: shape.turn.to_degrees(),
                    slant: shape.slant.to_degrees(),
                });
        }
        overlay.carets.push(CaretStop {
            line: next,
            offset: 0,
            at,
            up,
        });
        overlay.blocks[block].lines = vec![next];
        return true;
    }
    false
}

fn typed_status(typed: &Typed, deleted: bool) -> Message {
    if let Some(status) = &typed.status {
        return status.clone().into();
    }
    Done::Typed {
        deleted,
        layout: typed.note.as_ref().map_or(Layout::InFrame, |note| {
            Layout::OnItsRow(Box::new(note.clone()))
        }),
        overflow: typed.overflow,
        brought_in: typed.brought_in.clone(),
        drawn_from: typed.drawn_from.clone(),
        cropped: typed.cropped,
    }
    .into()
}

fn explain_block_refusal(reason: &str) -> Refusal {
    const NAMED: [(&str, LayoutWhy); 15] = [
        (
            "closer together than its line pitch",
            LayoutWhy::RowsOffPitch,
        ),
        ("no embedded outline font", LayoutWhy::NoEmbeddedOutline),
        ("kerned with TJ", LayoutWhy::Kerned),
        (
            "placed under different transforms",
            LayoutWhy::MixedTransforms,
        ),
        (
            "colour space that cannot be written yet",
            LayoutWhy::UnwritableColour,
        ),
        ("sheared or mirrored", LayoutWhy::ShearedOrMirrored),
        ("states no line height", LayoutWhy::NoLineHeight),
        ("also paints text outside it", LayoutWhy::PaintsTextOutside),
        ("no Unicode meaning", LayoutWhy::NoUnicodeMeaning),
        (
            "more than one character",
            LayoutWhy::CodeForSeveralCharacters,
        ),
        (EMPTYING_THE_BLOCK, LayoutWhy::EveryCharacterDeleted),
        (
            "does not decode in the block's font",
            LayoutWhy::NotInTheFont,
        ),
        ("tabs and control characters", LayoutWhy::ControlCharacters),
        ("invisible or clips", LayoutWhy::InvisibleOrClips),
        ("frame has no width", LayoutWhy::FrameHasNoWidth),
    ];
    NAMED
        .iter()
        .find(|(english, _)| reason.contains(english))
        .map_or_else(
            || reason.into(),
            |(_, why)| Refusal::Layout {
                why: *why,
                engine: reason.to_owned(),
            },
        )
}

fn refused(reason: impl Into<Refusal>) -> (Applied, Message) {
    let reason = reason.into();
    (Applied::Refused(reason.clone()), Message::Refused(reason))
}

fn blank_pdf() -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << >> >>",
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let table = bytes.len();
    bytes.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{table}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    bytes
}

#[cfg(test)]
mod geometry_tests {
    use super::{PageGeometry, laid_out_the_same};

    fn a4(at: usize) -> PageGeometry {
        PageGeometry {
            media_box: [0.0, 0.0, 595.0, 842.0],
            media_box_span: pdf_bytes::SourceSpan::new(pdf_bytes::SourceId::new(1), at, at + 20)
                .expect("a span of twenty bytes"),
            crop_box: [0.0, 0.0, 595.0, 842.0],
            crop_box_span: None,
            rotate: 0,
            rotate_span: None,
        }
    }

    #[test]
    fn the_same_page_written_somewhere_else_is_the_same_page() {
        assert!(laid_out_the_same(&[a4(120)], &[a4(90_000)]));
    }

    #[test]
    fn a_page_of_another_size_is_another_page() {
        let mut wider = a4(120);
        wider.crop_box = [0.0, 0.0, 842.0, 842.0];
        assert!(!laid_out_the_same(&[wider], &[a4(120)]));
        let mut turned = a4(120);
        turned.rotate = 90;
        assert!(!laid_out_the_same(&[turned], &[a4(120)]));
        assert!(!laid_out_the_same(&[a4(120), a4(200)], &[a4(120)]));
    }
}

#[cfg(test)]
mod trace_tests {
    use super::Step;

    fn a_move(group: bool) -> Step {
        Step::MoveBlock {
            page: 0,
            anchors: vec!["4:0:49".to_owned(), "4:0:105".to_owned()],
            block: None,
            group,
            dx: 9.0,
            dy: 0.0,
        }
    }

    #[test]
    fn a_group_move_says_so_in_the_trace() {
        assert!(
            a_move(true).arguments().ends_with(" group"),
            "{}",
            a_move(true).arguments()
        );
        assert!(!a_move(false).arguments().contains("group"));
        assert_eq!(a_move(true).name(), a_move(false).name());
    }
}

#[cfg(test)]
mod panic_recovery_tests {
    use super::{Applied, Editor, Step};

    #[test]
    fn a_panicking_step_comes_back_as_a_refusal_and_frees_the_session() {
        let mut editor = Editor::blank([595.28, 841.89]).expect("a blank document opens");
        assert!(!editor.is_busy());

        let applied = editor.here(Step::Panics);

        assert!(
            matches!(applied, Applied::Refused(_)),
            "a panicking step must answer as a refusal, not {applied:?}"
        );
        assert!(
            !editor.is_busy(),
            "the session must come back even though the step panicked, or the \
             document can never be saved, discarded or left"
        );
    }
}
