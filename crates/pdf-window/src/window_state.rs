use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;

use eframe::egui;

use pdf_app::draft::Intent;
use pdf_app::painter::Painter;
use pdf_app::tiles::{Ledger, TileId};
use pdf_app::view::{Placement, Quad, ROTATE_HANDLE, Step};
use pdf_app::wording::Lang;
use pdf_app::{EditOutcome, Editor};

pub(crate) struct Opening {
    pub(crate) path: PathBuf,
    pub(crate) page: usize,
    pub(crate) changed_protection: bool,
    pub(crate) tried_a_password: bool,
    pub(crate) handle: JoinHandle<Opened>,
}

pub(crate) enum Opened {
    Document(Box<Editor>),
    Locked(pdf_bytes::ByteStore),
    Refused(String),
}

pub(crate) fn install_look(ctx: &egui::Context) {
    set_dark(ctx, false);
    ctx.options_mut(|options| options.zoom_with_keyboard = false);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(2.0, 4.0);
        style.spacing.button_padding = egui::vec2(6.0, 4.0);
    });
}

pub(crate) fn set_dark(ctx: &egui::Context, dark: bool) {
    ctx.set_theme(if dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    });
}

pub(crate) const fn desk(dark: bool) -> egui::Color32 {
    if dark {
        egui::Color32::from_rgb(0x2a, 0x2b, 0x2e)
    } else {
        egui::Color32::from_rgb(0xe6, 0xe8, 0xeb)
    }
}

static FACES_FOUND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn packaged_faces_found() -> bool {
    FACES_FOUND.load(std::sync::atomic::Ordering::Relaxed)
}

pub(crate) fn install_fonts(ctx: &egui::Context) {
    use crate::interface_fonts::{Host, candidates, choose, windows_fonts};

    let packaged = pdf_cli::package_root()
        .map(|root| root.join("packaged"))
        .filter(|directory| directory.is_dir());
    FACES_FOUND.store(packaged.is_some(), std::sync::atomic::Ordering::Relaxed);
    let chosen = choose(
        candidates(Host::this(), packaged.as_deref(), &windows_fonts()),
        |path| std::fs::read(path).ok(),
    );
    if chosen.is_empty() {
        return;
    }
    let names: Vec<String> = chosen.iter().map(|face| face.name.to_owned()).collect();
    let mut fonts = egui::FontDefinitions::default();
    for face in chosen {
        fonts.font_data.insert(
            face.name.to_owned(),
            std::sync::Arc::new(egui::FontData::from_owned(face.bytes)),
        );
    }
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        if let Some(list) = fonts.families.get_mut(&family) {
            list.extend(names.iter().cloned());
        }
    }
    ctx.set_fonts(fonts);
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Tool {
    #[default]
    Select,
    Text,
    Picture,
    Pen,
    Highlighter,
    Shape,
    Form,
    Link,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ChosenLinks {
    pub(crate) page: usize,
    pub(crate) links: Vec<pdf_syntax::Reference>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LinkDraft {
    pub(crate) page: usize,
    pub(crate) link: Option<pdf_syntax::Reference>,
    pub(crate) pixels: Option<[f64; 4]>,
    pub(crate) goes: Goes,
    pub(crate) page_number: String,
    pub(crate) address: String,
    pub(crate) look: pdf_edit::link::Look,
    pub(crate) arrival: pdf_edit::link::Arrival,
    pub(crate) percent: String,
    pub(crate) file: String,
    pub(crate) name: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Goes {
    #[default]
    APage,
    AnAddress,
    ADocument,
    AName,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct NamingDraft {
    pub(crate) name: String,
    pub(crate) arrival: pdf_edit::link::Arrival,
    pub(crate) percent: String,
    pub(crate) renaming: Option<(String, String)>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PrintWhich {
    #[default]
    All,
    Current,
    Some,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PrintChoices {
    pub(crate) which: PrintWhich,
    pub(crate) range: String,
    pub(crate) only: pdf_edit::stamp::Only,
    pub(crate) reverse: bool,
    pub(crate) paper: usize,
    pub(crate) orientation: pdf_print::Orientation,
    pub(crate) per_sheet: u8,
    pub(crate) columns: u8,
    pub(crate) rows: u8,
    pub(crate) scaling: pdf_app::wording::PrintScalingKind,
    pub(crate) percent: f64,
    pub(crate) order: pdf_print::Order,
    pub(crate) borders: bool,
    pub(crate) auto_rotate: bool,
    pub(crate) margin: f64,
    pub(crate) nudge: [f64; 2],
    pub(crate) printer: Option<String>,
    pub(crate) copies: u16,
    pub(crate) printing: Printing,
    pub(crate) sides: pdf_print::service::Sides,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Printing {
    #[default]
    InColour,
    BlackAndWhite,
}

impl Default for PrintChoices {
    fn default() -> Self {
        Self {
            which: PrintWhich::All,
            range: String::new(),
            only: pdf_edit::stamp::Only::Every,
            reverse: false,
            paper: 0,
            orientation: pdf_print::Orientation::Auto,
            per_sheet: 1,
            columns: 2,
            rows: 2,
            scaling: pdf_app::wording::PrintScalingKind::Fit,
            percent: 100.0,
            order: pdf_print::Order::Horizontal,
            borders: false,
            auto_rotate: true,
            margin: 5.0,
            nudge: [0.0, 0.0],
            printer: None,
            copies: 1,
            printing: Printing::InColour,
            sides: pdf_print::service::Sides::One,
        }
    }
}

pub(crate) struct PrintDraft {
    pub(crate) sheet: usize,
    pub(crate) dragging: Option<egui::Vec2>,
    pub(crate) corner: bool,
    pub(crate) landed: Option<(egui::Vec2, bool)>,
    pub(crate) put_back: bool,
    pub(crate) allowance: Option<pdf_content::PrintAllowance>,
    pub(crate) shown: Option<(String, Result<egui::TextureHandle, String>)>,
    pub(crate) drawing: Option<(
        String,
        std::sync::mpsc::Receiver<Result<pdf_print::SheetImage, String>>,
    )>,
    pub(crate) printers: Result<(Vec<pdf_print::service::Printer>, Option<String>), String>,
    pub(crate) capabilities: Option<(String, Result<pdf_print::service::Capabilities, String>)>,
    pub(crate) job: Option<PrintJob>,
    pub(crate) failed: Option<String>,
}

pub(crate) struct PrintJob {
    pub(crate) stop: Arc<std::sync::atomic::AtomicBool>,
    pub(crate) news: std::sync::mpsc::Receiver<JobNews>,
    pub(crate) done: usize,
    pub(crate) total: usize,
    pub(crate) printer: String,
}

pub(crate) enum JobNews {
    Drawn(usize),
    Finished(Result<Option<i32>, String>),
}

pub(crate) struct OcrDraft {
    pub(crate) engine: Option<pdf_ocr::Tesseract>,
    pub(crate) languages: Vec<(String, bool)>,
    pub(crate) here: Vec<String>,
    pub(crate) choice: pdf_app::ocr_choice::Choice,
    pub(crate) which: OcrWhich,
    pub(crate) range: String,
    pub(crate) reading: Option<OcrReading>,
    pub(crate) fetching: Option<OcrFetch>,
    pub(crate) trouble: Option<pdf_app::wording::Message>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum OcrWhich {
    #[default]
    ThisPage,
    All,
    Some,
}

pub(crate) struct OcrFetch {
    pub(crate) code: Option<String>,
    pub(crate) seen: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pub(crate) total: u64,
    pub(crate) cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub(crate) answer: std::sync::mpsc::Receiver<Result<(), String>>,
    pub(crate) worker: Option<std::thread::JoinHandle<()>>,
}

pub(crate) struct OcrReading {
    pub(crate) cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub(crate) answers: std::sync::mpsc::Receiver<(usize, PageRead)>,
    pub(crate) workers: Vec<std::thread::JoinHandle<()>>,
    pub(crate) pages: Vec<usize>,
    pub(crate) read: std::collections::BTreeMap<usize, PageRead>,
    pub(crate) epoch: u64,
}

pub(crate) enum PageRead {
    Read(pdf_ocr::Reading),
    HadText,
    Failed(String),
}

#[derive(Clone, Debug)]
pub(crate) struct StampDraft {
    pub(crate) door: pdf_app::wording::Command,
    pub(crate) wording: String,
    pub(crate) spot: pdf_edit::stamp::Spot,
    pub(crate) family: String,
    pub(crate) size: f64,
    pub(crate) bold: bool,
    pub(crate) colour: [f32; 3],
    pub(crate) opacity: u8,
    pub(crate) margin: f64,
    pub(crate) range: String,
    pub(crate) only: pdf_edit::stamp::Only,
    pub(crate) start: i64,
    pub(crate) seen: std::collections::BTreeMap<usize, (String, SeenStamp)>,
}

pub(crate) type SeenStamp = Result<(String, Option<[f64; 4]>), pdf_app::wording::Message>;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum LinkTab {
    #[default]
    Goes,
    Appearance,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ChosenFields {
    pub(crate) page: usize,
    pub(crate) widgets: Vec<pdf_syntax::Reference>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Clipboard {
    pub(crate) copied: pdf_edit::Copied,
    pub(crate) marker: String,
    pub(crate) bounds: [f64; 4],
    pub(crate) from: pdf_bytes::ByteStore,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PropertiesTab {
    #[default]
    General,
    Appearance,
    Options,
}

#[derive(Clone, Debug, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one for each checkbox of Acrobat's Properties window"
)]
pub(crate) struct FieldDraft {
    pub(crate) widget: pdf_syntax::Reference,
    pub(crate) name: String,
    pub(crate) tooltip: String,
    pub(crate) visibility: pdf_edit::form::Visibility,
    pub(crate) required: bool,
    pub(crate) read_only: bool,
    pub(crate) border_on: bool,
    pub(crate) border: [f32; 3],
    pub(crate) fill_on: bool,
    pub(crate) fill: [f32; 3],
    pub(crate) border_width: f64,
    pub(crate) border_style: pdf_edit::form::BorderStyle,
    pub(crate) text_colour: [f32; 3],
    pub(crate) fits: bool,
    pub(crate) size: f64,
    pub(crate) quadding: pdf_edit::form::Quadding,
    pub(crate) default_value: String,
    pub(crate) limit_on: bool,
    pub(crate) limit: usize,
    pub(crate) flags: u32,
    pub(crate) items: Vec<(String, String)>,
    pub(crate) new_item: String,
    pub(crate) new_export: String,
    pub(crate) picked_item: Option<usize>,
    pub(crate) button_style: pdf_edit::form::ButtonStyle,
    pub(crate) export_value: String,
    pub(crate) checked_by_default: bool,
    pub(crate) caption: String,
    pub(crate) link: String,
    pub(crate) date_format: String,
}

#[derive(Clone, Debug)]
pub(crate) struct FormTool {
    pub(crate) kind: Option<pdf_edit::new_field::NewFieldKind>,
    pub(crate) choices: String,
    pub(crate) group: String,
    pub(crate) caption: String,
}

impl Default for FormTool {
    fn default() -> Self {
        Self {
            kind: None,
            choices: "Option 1, Option 2, Option 3".to_owned(),
            group: String::new(),
            caption: "Button".to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Shape {
    #[default]
    Line,
    Arrow,
    Rectangle,
    Ellipse,
}

impl Shape {
    pub(crate) const ALL: [Self; 4] = [Self::Line, Self::Arrow, Self::Rectangle, Self::Ellipse];

    pub(crate) const fn is_closed(self) -> bool {
        matches!(self, Self::Rectangle | Self::Ellipse)
    }
}

impl Tool {
    pub(crate) const fn is_drawing(self) -> bool {
        matches!(self, Self::Pen | Self::Highlighter | Self::Shape)
    }

    pub(crate) const fn edits_text(self) -> bool {
        matches!(self, Self::Select | Self::Text)
    }

    pub(crate) const fn places_on_click(self) -> bool {
        matches!(self, Self::Text | Self::Picture)
    }

    pub(crate) const fn is_freehand(self) -> bool {
        matches!(self, Self::Pen)
    }

    pub(crate) const fn draws_on_the_page(self) -> bool {
        matches!(
            self,
            Self::Pen | Self::Highlighter | Self::Shape | Self::Picture
        )
    }

    pub(crate) const fn drags_a_box(self) -> bool {
        matches!(self, Self::Shape | Self::Highlighter)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Pen {
    pub(crate) colour: [f64; 3],
    pub(crate) width: f64,
}

#[derive(Clone, Debug)]
pub(crate) struct Filling {
    pub(crate) page: usize,
    pub(crate) field: pdf_app::document::FieldBox,
    pub(crate) text: String,
    pub(crate) take_the_keyboard: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Finding {
    pub(crate) needle: String,
    pub(crate) searched: pdf_app::find::Found,
    pub(crate) asked: std::collections::BTreeSet<usize>,
    pub(crate) take_the_keyboard: bool,
    pub(crate) epoch: u64,
    pub(crate) replacement: Option<String>,
    pub(crate) replacing: Option<Replacing>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Replacing {
    pub(crate) wanted: usize,
    pub(crate) done: usize,
    pub(crate) refused: usize,
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Ink {
    pub(crate) page: usize,
    pub(crate) points: Vec<(f64, f64)>,
    pub(crate) drawn_to: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct ChosenPicture {
    pub(crate) file: std::sync::Arc<[u8]>,
    pub(crate) upright: (u32, u32),
    pub(crate) mini: Option<pdf_edit::image_file::Thumbnail>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TextDraft {
    pub(crate) page: usize,
    pub(crate) frame: [f64; 4],
    pub(crate) family: String,
    pub(crate) size: f64,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) fill: Option<[f64; 3]>,
    pub(crate) alignment: pdf_edit::Alignment,
    pub(crate) flows_round: bool,
}

impl TextDraft {
    pub(crate) fn styling(&self) -> pdf_edit::TextStyle {
        pdf_edit::TextStyle {
            size: Some(self.size),
            fill: self.fill,
            bold: Some(self.bold),
            italic: Some(self.italic),
            family: Some(self.family.clone()),
            ..pdf_edit::TextStyle::default()
        }
    }

    pub(crate) fn restyle(&mut self, wanted: &crate::format::Wanted) {
        match wanted {
            crate::format::Wanted::Size(points) => self.size = *points,
            crate::format::Wanted::Style(style) => {
                if let Some(family) = style.family.clone() {
                    self.family = family;
                }
                if let Some(size) = style.size {
                    self.size = size;
                }
                if let Some(bold) = style.bold {
                    self.bold = bold;
                }
                if let Some(italic) = style.italic {
                    self.italic = italic;
                }
                if style.fill.is_some() {
                    self.fill = style.fill;
                }
            }
            crate::format::Wanted::Align(alignment) => self.alignment = *alignment,
            crate::format::Wanted::FlowRound(round) => self.flows_round = *round,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Carrying {
    NewText,
    NewPicture,
    NewLine,
    NewShape,
    NewField,
    MovingField {
        fields: Vec<(pdf_syntax::Reference, [f64; 4])>,
        resize: bool,
    },
    NewLink,
    MovingLinks {
        links: Vec<(pdf_syntax::Reference, [f64; 4])>,
        resize: bool,
    },
    FieldSweep {
        adding: bool,
    },
    Run(String),
    Block(Vec<String>),
    Object(String),
    ObjectHandle {
        anchor: String,
        handle: usize,
    },
    BlockTurn(Vec<String>),
    Selection {
        block: usize,
    },
    Marquee,
    Panning {
        was: egui::Vec2,
    },
    Handle {
        frame: usize,
        handle: usize,
    },
}

#[derive(Clone, Default)]
pub(crate) struct Chosen {
    pub(crate) page: usize,
    pub(crate) blocks: Vec<usize>,
    pub(crate) objects: Vec<usize>,
}

impl Chosen {
    pub(crate) fn is_a_group(&self) -> bool {
        self.blocks.len() + self.objects.len() > 1
    }

    pub(crate) fn count(&self) -> usize {
        self.blocks.len() + self.objects.len()
    }

    pub(crate) fn holds(&self, pointing: Pointing) -> bool {
        match pointing {
            Pointing::Block { page, block } | Pointing::Text { page, block, .. } => {
                page == self.page && self.blocks.contains(&block)
            }
            Pointing::Object { page, object } => {
                page == self.page && self.objects.contains(&object)
            }
            Pointing::Nothing => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Regrouping {
    pub(crate) page: usize,
    pub(crate) blocks: Vec<Quad>,
    pub(crate) objects: Vec<Quad>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Around {
    Word,
    Row,
}

pub(crate) type BlockFaces = std::sync::Arc<Vec<Vec<pdf_edit::ClusterFace>>>;

#[derive(Default)]
pub(crate) struct Typing {
    pub(crate) at: Option<(usize, usize)>,
    pub(crate) size: Option<String>,
    pub(crate) next: Option<(usize, pdf_edit::TextStyle)>,
}

impl Typing {
    pub(crate) fn about(&mut self, block: Option<(usize, usize)>) {
        if self.at != block {
            *self = Self {
                at: block,
                ..Self::default()
            };
        }
    }
}

pub(crate) const fn resize_cursor(handle: usize) -> egui::CursorIcon {
    match handle {
        0 | 2 => egui::CursorIcon::ResizeNwSe,
        1 | 3 => egui::CursorIcon::ResizeNeSw,
        4 | 6 => egui::CursorIcon::ResizeVertical,
        ROTATE_HANDLE => egui::CursorIcon::Crosshair,
        _ => egui::CursorIcon::ResizeHorizontal,
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PageDrag {
    pub(crate) pages: Vec<usize>,
    pub(crate) gap: Option<usize>,
    pub(crate) grab: egui::Vec2,
}

pub(crate) struct Drag {
    pub(crate) what: Carrying,
    pub(crate) page: usize,
    pub(crate) from: egui::Pos2,
    pub(crate) to: egui::Pos2,
    pub(crate) started_at: Quad,
    pub(crate) straight: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Landing {
    pub(crate) page: usize,
    pub(crate) source: Quad,
    pub(crate) going: Quad,
}

pub(crate) struct Running {
    pub(crate) handle: JoinHandle<EditOutcome>,
}

pub(crate) type TextPositions = (usize, usize, (usize, usize), (usize, usize));

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Caret {
    pub(crate) at: usize,
    pub(crate) anchor: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Pointing {
    Nothing,
    Block {
        page: usize,
        block: usize,
    },
    Text {
        page: usize,
        block: usize,
        caret: Caret,
    },
    Object {
        page: usize,
        object: usize,
    },
}

impl Pointing {
    pub(crate) const fn page(self) -> Option<usize> {
        match self {
            Self::Nothing => None,
            Self::Block { page, .. } | Self::Text { page, .. } | Self::Object { page, .. } => {
                Some(page)
            }
        }
    }

    pub(crate) const fn block(self) -> Option<usize> {
        match self {
            Self::Nothing | Self::Object { .. } => None,
            Self::Block { block, .. } | Self::Text { block, .. } => Some(block),
        }
    }

    pub(crate) const fn object_on(self, page: usize) -> Option<usize> {
        match self {
            Self::Object { page: at, object } if at == page => Some(object),
            _ => None,
        }
    }

    pub(crate) const fn block_on(self, page: usize) -> Option<usize> {
        match self {
            Self::Block { page: at, block }
            | Self::Text {
                page: at, block, ..
            } if at == page => Some(block),
            _ => None,
        }
    }

    pub(crate) const fn caret(self) -> Option<Caret> {
        match self {
            Self::Text { caret, .. } => Some(caret),
            _ => None,
        }
    }

    pub(crate) const fn editing(self) -> bool {
        matches!(self, Self::Text { .. })
    }

    pub(crate) const fn typing_in(self, page: usize) -> Option<usize> {
        match self {
            Self::Text {
                page: at, block, ..
            } if at == page => Some(block),
            _ => None,
        }
    }

    pub(crate) fn block_stands(
        self,
        overlay: &pdf_app::Overlay,
        page: usize,
        block: usize,
    ) -> bool {
        let holds_text = overlay
            .blocks
            .get(block)
            .is_some_and(|owner| !owner.anchors.is_empty());
        pdf_app::view::stands(holds_text, block, self.typing_in(page))
    }
}

pub(crate) const HOLD_FOR: f64 = 0.6;

pub(crate) const STILL_ENOUGH: f32 = 4.0;

pub const ZOOMS: [f64; 11] = [0.25, 0.35, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 2.5, 3.0, 4.0];
pub(crate) const FURTHEST: f64 = ZOOMS[0];
pub(crate) const CLOSEST: f64 = ZOOMS[ZOOMS.len() - 1];

pub(crate) const AHEAD: f32 = 256.0;

pub(crate) const COARSE: usize = 0;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn frame_interval() -> Option<std::time::Duration> {
    let fps = std::env::var("PANPDF_MAX_FPS")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    (fps > 0).then(|| std::time::Duration::from_nanos(1_000_000_000 / u64::from(fps)))
}

pub(crate) const SPARE_TEXTURES: usize = 160;

pub(crate) fn keep_bytes() -> usize {
    const DEFAULT_MB: usize = 256;
    std::env::var("PANPDF_TILE_BUDGET_MB")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_MB)
        .saturating_mul(1 << 20)
}

pub(crate) const KEEP_PAGES: usize = 12;

pub(crate) const KEEP_PAGE_BYTES: usize = 128 * 1024 * 1024;

impl Window {
    pub(crate) fn spare_count(&self) -> usize {
        self.spare.values().map(Vec::len).sum()
    }

    pub(crate) fn spare_bytes(&self) -> usize {
        self.spare
            .iter()
            .map(|((width, height), kept)| width * height * 4 * kept.len())
            .sum()
    }

    pub(crate) fn thumb_bytes(&self) -> usize {
        self.thumbs
            .values()
            .map(|thumb| {
                let [width, height] = thumb.texture.size();
                width * height * 4
            })
            .sum()
    }
}

pub(crate) type ReadingKey = (usize, usize, usize);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Framed {
    pub(crate) text: bool,
    pub(crate) pictures: bool,
    pub(crate) drawings: bool,
}

impl Default for Framed {
    fn default() -> Self {
        Self {
            text: true,
            pictures: true,
            drawings: true,
        }
    }
}

#[derive(Clone)]
pub(crate) struct Scene {
    pub(crate) leaf: Arc<pdf_app::Leaf>,
    pub(crate) pointing: Pointing,
    pub(crate) chosen: Chosen,
    pub(crate) frames: Vec<[f64; 4]>,
    pub(crate) bands: Option<Vec<[f64; 4]>>,
}

#[derive(Clone, Copy)]
pub(crate) struct Laid {
    pub(crate) page: usize,
    pub(crate) rect: egui::Rect,
    pub(crate) placed: Placement,
}

pub(crate) enum Leaving {
    Open(PathBuf, usize),
    New([f64; 2]),
    Close,
    LetGo,
}

#[derive(Clone, Copy)]
pub(crate) enum LeaveChoice {
    Save,
    Discard,
    Cancel,
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "each is one thing the window is or is not doing, and a struct of \
              enums would name the same states less clearly"
)]
pub(crate) struct Window {
    pub(crate) saved_epoch: u64,
    pub(crate) saved_digest: Option<String>,
    pub(crate) leaving: Option<Leaving>,
    pub(crate) close_confirmed: bool,
    pub(crate) editor: Editor,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) ai: crate::ai_panel::AiState,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) agents_open: bool,
    pub(crate) dark: bool,
    pub(crate) asking_to_open: bool,
    pub(crate) restriction_answered: bool,
    pub(crate) lang: Lang,
    pub(crate) painter: Painter,
    pub(crate) tiles: Ledger,
    pub(crate) textures: BTreeMap<TileId, egui::TextureHandle>,
    pub(crate) waiting_textures: BTreeMap<TileId, egui::TextureHandle>,
    pub(crate) stale_answers: u64,
    pub(crate) spare: BTreeMap<(usize, usize), Vec<egui::TextureHandle>>,
    pub(crate) failed: BTreeMap<usize, String>,
    pub(crate) library: Vec<PathBuf>,
    pub(crate) opened: PathBuf,
    pub(crate) untitled: bool,
    pub(crate) properties: Option<crate::properties::Properties>,
    pub(crate) unlocking: Option<crate::unlock::Unlock>,
    pub(crate) protection_changed: bool,
    pub(crate) loading: Option<Opening>,
    pub(crate) zoom: f64,
    pub(crate) show_clusters: bool,
    pub(crate) show_frames: bool,
    pub(crate) framed: Framed,
    pub(crate) pointing: Pointing,
    pub(crate) chosen: Chosen,
    pub(crate) context: Option<egui::Pos2>,
    pub(crate) toolbar_area: Option<egui::Rect>,
    pub(crate) held_still: Option<(egui::Pos2, f64)>,
    pub(crate) running: Option<Running>,
    pub(crate) title: String,
    pub(crate) destination: PathBuf,
    pub(crate) drag: Option<Drag>,
    pub(crate) tool: Tool,
    pub(crate) landscape: bool,
    pub(crate) turning_to: Option<usize>,
    pub(crate) text_draft: Option<TextDraft>,
    pub(crate) pictures: Vec<ChosenPicture>,
    pub(crate) picture_minis: Vec<Option<egui::TextureHandle>>,
    pub(crate) finding: Option<Finding>,
    pub(crate) filling: Option<Filling>,
    pub(crate) pen: Pen,
    pub(crate) marker: Pen,
    pub(crate) shape: Shape,
    pub(crate) form_tool: FormTool,
    pub(crate) toolbar_choices: f32,
    pub(crate) toolbar_slack: f32,
    pub(crate) screen_fitted: bool,
    pub(crate) pages_folded: bool,
    pub(crate) pages_width: f32,
    pub(crate) pages_window_width: f32,
    pub(crate) toolbar_compact: bool,
    pub(crate) chosen_fields: Option<ChosenFields>,
    pub(crate) field_clipboard: Option<ChosenFields>,
    pub(crate) field_clipboard_text: Option<String>,
    pub(crate) clipboard: Option<Clipboard>,
    pub(crate) pastes: usize,
    pub(crate) landing_fields: Option<(usize, Vec<[f64; 4]>)>,
    pub(crate) field_nudge: (f64, f64),
    pub(crate) field_draft: Option<FieldDraft>,
    pub(crate) chosen_links: Option<ChosenLinks>,
    pub(crate) link_draft: Option<LinkDraft>,
    pub(crate) link_tab: LinkTab,
    pub(crate) naming_draft: Option<NamingDraft>,
    pub(crate) stamp_draft: Option<StampDraft>,
    pub(crate) ocr_draft: Option<OcrDraft>,
    pub(crate) scan_notice_shut: Option<PathBuf>,
    pub(crate) print_draft: Option<PrintDraft>,
    pub(crate) print_choices: PrintChoices,
    pub(crate) landing_link: Option<(usize, [f64; 4])>,
    pub(crate) show_contents: bool,
    pub(crate) chosen_bookmark: Option<pdf_syntax::Reference>,
    pub(crate) renaming: Option<(Option<pdf_syntax::Reference>, String)>,
    pub(crate) properties_tab: PropertiesTab,
    pub(crate) field_draft_origin: Option<FieldDraft>,
    pub(crate) shape_fill: bool,
    pub(crate) ink: Option<Ink>,
    pub(crate) draft_pixels: Option<(usize, [f64; 4])>,
    pub(crate) entering: Option<(usize, [f64; 4])>,
    pub(crate) landing: Option<Landing>,
    pub(crate) scenes: BTreeMap<usize, Scene>,
    pub(crate) reselect: Option<(usize, usize)>,
    pub(crate) reselect_object: Option<(usize, Quad)>,
    pub(crate) regroup: Option<Regrouping>,
    pub(crate) resume: Option<(usize, usize, usize, usize)>,
    pub(crate) resume_anchor: Option<(usize, usize)>,
    pub(crate) typing: Typing,
    pub(crate) colours: crate::palette::Colours,
    pub(crate) spacing: Option<(ReadingKey, Option<(f64, pdf_edit::Alignment)>)>,
    pub(crate) faces: Option<(ReadingKey, Option<BlockFaces>)>,
    pub(crate) input: pdf_app::draft::Input,
    pub(crate) live: Option<crate::live_typing::LiveTyping>,
    pub(crate) offset: egui::Vec2,
    pub(crate) view: egui::Vec2,
    pub(crate) view_corner: egui::Pos2,
    pub(crate) wanted_offset: Option<egui::Vec2>,
    pub(crate) laid: Vec<Laid>,
    pub(crate) focus: usize,
    pub(crate) frame: u64,
    pub(crate) painted_at: Option<crate::moment::Moment>,
    pub(crate) trace: Option<crate::trace::Trace>,
    pub(crate) meter: Option<crate::meter::Meter>,
    pub(crate) speed: pdf_app::speed::Speed,
    pub(crate) show_speed: bool,
    pub(crate) last_edit: Option<pdf_session::stages::Stages>,
    pub(crate) reveal_caret: bool,
    pub(crate) thumbs: BTreeMap<usize, crate::pages::Thumb>,
    pub(crate) thumbs_wanted: Vec<usize>,
    pub(crate) home: bool,
    pub(crate) recent: Vec<pdf_app::recent::Recent>,
    pub(crate) chooser: Option<crate::chooser::Chooser>,
    pub(crate) choosing_for: crate::page_actions::Choosing,
    pub(crate) chosen_pages: std::collections::BTreeSet<usize>,
    pub(crate) page_drag: Option<PageDrag>,
    pub(crate) page_motion: crate::page_motion::Motion,
    pub(crate) page_preview: Option<Vec<usize>>,
    pub(crate) renumber: Option<crate::page_motion::Renumber>,
    pub(crate) page_panel_shape: Option<crate::page_motion::PanelShape>,
    pub(crate) ai_panel_shape: Option<egui::Rect>,
    pub(crate) ai_flow: Option<crate::room::Flow>,
    pub(crate) file_hover_gap: Option<usize>,
    pub(crate) panel_menu: Option<(crate::pages::PanelMenu, egui::Pos2, u64)>,
    pub(crate) arriving: std::collections::VecDeque<(crate::page_motion::Arriving, usize)>,
    pub(crate) thumb_width: f32,
    pub(crate) writing: Option<crate::take_out::Writing>,
    pub(crate) splitting: Option<crate::take_out::SplitChoices>,
    pub(crate) exporting: Option<crate::pictures::ExportChoices>,
    pub(crate) making: Option<crate::pictures::Making>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ArrowPress {
    pub(crate) step: Step,
    pub(crate) shift: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TypedKey {
    Intent(Intent),
    Copy,
    Cut,
    Paste(String),
    SelectAll,
}
