use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[cfg(not(target_arch = "wasm32"))]
mod no_graphics;

use eframe::egui;
use pdf_app::draft::{Intent, Landing as TextLanding};
use pdf_app::painter::{Done, Painter};
use pdf_app::strip::tile_box;
use pdf_app::tiles::{Arrival, Held, Ledger, Slot};
use pdf_app::view::{block_position, stop_at, zoom_anchor};
use pdf_app::wording::{Lang, Message, ObjectKind};

use crate::input::history_keys;
use crate::window_state::{
    CLOSEST, Caret, Chosen, FURTHEST, Pointing, Running, TextPositions, TypedKey, Typing, Window,
    ZOOMS, desk, install_fonts, install_look,
};
use pdf_app::{Applied, EditJob, Editor};

pub struct Locked {
    pub path: PathBuf,
    pub source: pdf_bytes::ByteStore,
}

fn made(
    context: &eframe::CreationContext<'_>,
    editor: Editor,
    opened: PathBuf,
    library: Vec<PathBuf>,
    locked: Option<Locked>,
) -> Window {
    install_fonts(&context.egui_ctx);
    install_look(&context.egui_ctx);
    let mut window = Window::new(editor, opened, library);
    let repaint = context.egui_ctx.clone();
    window.painter.set_waker(move || repaint.request_repaint());
    window.recent = crate::hub::load_recent();
    window.home = !window.has_document();
    if let Some(locked) = locked {
        window.unlocking = Some(crate::unlock::Unlock {
            path: locked.path,
            page: 0,
            source: locked.source,
            typed: String::new(),
            tried: false,
            shown: false,
            focus: true,
        });
    }
    window.remember_here();
    window.keep_a_trace();
    window
}

#[cfg(not(target_arch = "wasm32"))]
const RENDERERS: [eframe::Renderer; 2] = [eframe::Renderer::Wgpu, eframe::Renderer::Glow];

#[cfg(not(target_arch = "wasm32"))]
pub fn run(
    editor: Editor,
    opened: PathBuf,
    library: Vec<PathBuf>,
    locked: Option<Locked>,
) -> Result<(), String> {
    let waiting = std::rc::Rc::new(std::cell::RefCell::new(Some((
        editor, opened, library, locked,
    ))));
    let mut refusals: Vec<String> = Vec::new();
    for renderer in RENDERERS {
        let options = eframe::NativeOptions {
            renderer,
            viewport: egui::ViewportBuilder::default()
                .with_inner_size(crate::room::PREFERRED)
                .with_min_inner_size([crate::room::MIN_WIDTH, crate::room::MIN_HEIGHT]),
            ..eframe::NativeOptions::default()
        };
        let start = std::rc::Rc::clone(&waiting);
        let result = eframe::run_native(
            "PanPDF",
            options,
            Box::new(move |context| {
                let (editor, opened, library, locked) =
                    start.borrow_mut().take().ok_or_else(|| {
                        Box::<dyn std::error::Error + Send + Sync>::from(
                            "the window was already started",
                        )
                    })?;
                Ok(Box::new(made(context, editor, opened, library, locked)))
            }),
        );
        match result {
            Ok(()) => return Ok(()),
            Err(error) => {
                refusals.push(format!("{renderer}: {error}"));
                if waiting.borrow().is_none() {
                    break;
                }
            }
        }
    }
    let refused = refusals.join("; ");
    no_graphics::tell(&refused);
    Err(refused)
}

#[cfg(target_arch = "wasm32")]
pub async fn start(canvas: web_sys::HtmlCanvasElement) -> Result<(), String> {
    let editor = Editor::stand_in().map_err(|error| error.to_string())?;
    eframe::WebRunner::new()
        .start(
            canvas,
            eframe::WebOptions::default(),
            Box::new(move |context| {
                Ok(Box::new(made(
                    context,
                    editor,
                    PathBuf::new(),
                    Vec::new(),
                    None,
                )))
            }),
        )
        .await
        .map_err(|error| format!("{error:?}"))
}

impl Window {
    fn assistant_is_open(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.ai.open
        }
        #[cfg(target_arch = "wasm32")]
        false
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one window, one field a line: splitting it would hide what is set"
    )]
    pub(crate) fn new(editor: Editor, opened: PathBuf, library: Vec<PathBuf>) -> Self {
        let destination = crate::save_file::unused_copy(&opened);
        let remembered = crate::pages::remembered_view();
        Self {
            saved_epoch: editor.epoch(),
            saved_digest: None,
            leaving: None,
            close_confirmed: false,
            editor,
            #[cfg(not(target_arch = "wasm32"))]
            ai: crate::ai_panel::AiState::remembered(),
            #[cfg(not(target_arch = "wasm32"))]
            agents_open: false,
            painter: Painter::new(),
            tiles: Ledger::new(),
            textures: BTreeMap::new(),
            waiting_textures: BTreeMap::new(),
            stale_answers: 0,
            spare: BTreeMap::new(),
            failed: BTreeMap::new(),
            library,
            title: opened.display().to_string(),
            opened,
            untitled: false,
            properties: None,
            unlocking: None,
            protection_changed: false,
            loading: None,
            zoom: 1.0,
            dark: false,
            asking_to_open: false,
            restriction_answered: false,
            lang: Lang::default(),
            show_clusters: false,
            show_frames: true,
            framed: crate::window_state::Framed::default(),
            pointing: Pointing::Nothing,
            chosen: Chosen::default(),
            context: None,
            held_still: None,
            running: None,
            destination,
            drag: None,
            tool: crate::window_state::Tool::default(),
            landscape: false,
            turning_to: None,
            text_draft: None,
            pictures: Vec::new(),
            picture_minis: Vec::new(),
            finding: None,
            filling: None,
            pen: crate::window_state::Pen::default(),
            marker: crate::window_state::Pen::marker(),
            shape: crate::window_state::Shape::default(),
            shape_fill: false,
            form_tool: crate::window_state::FormTool::default(),
            screen_fitted: false,
            pages_folded: remembered.pages_folded,
            pages_width: remembered.pages_width,
            pages_window_width: 0.0,
            toolbar_choices: 0.0,
            toolbar_slack: 0.0,
            toolbar_compact: false,
            chosen_fields: None,
            field_clipboard: None,
            field_clipboard_text: None,
            pastes: 0,
            landing_fields: None,
            field_nudge: (0.0, 0.0),
            field_draft: None,
            properties_tab: crate::window_state::PropertiesTab::General,
            show_contents: false,
            chosen_bookmark: None,
            chosen_links: None,
            link_draft: None,
            naming_draft: None,
            stamp_draft: None,
            ocr_draft: None,
            scan_notice_shut: None,
            print_draft: None,
            print_choices: crate::window_state::PrintChoices::default(),
            link_tab: crate::window_state::LinkTab::default(),
            landing_link: None,
            renaming: None,
            field_draft_origin: None,
            ink: None,
            draft_pixels: None,
            entering: None,
            landing: None,
            scenes: BTreeMap::new(),
            reselect: None,
            reselect_object: None,
            typing: Typing::default(),
            colours: crate::palette::Colours::default(),
            spacing: None,
            faces: None,
            resume: None,
            resume_anchor: None,
            input: pdf_app::draft::Input::default(),
            live: None,
            offset: egui::Vec2::ZERO,
            view: egui::vec2(1000.0, 800.0),
            view_corner: egui::Pos2::ZERO,
            wanted_offset: None,
            laid: Vec::new(),
            focus: 0,
            frame: 0,
            painted_at: None,
            trace: None,
            meter: None,
            reveal_caret: false,
            thumbs: BTreeMap::new(),
            thumbs_wanted: Vec::new(),
            home: false,
            recent: Vec::new(),
            chooser: None,
            choosing_for: crate::page_actions::Choosing::Open,
            chosen_pages: std::collections::BTreeSet::new(),
            page_drag: None,
            page_motion: crate::page_motion::Motion::default(),
            page_preview: None,
            renumber: None,
            page_panel_shape: None,
            file_hover_gap: None,
            panel_menu: None,
            arriving: std::collections::VecDeque::new(),
            thumb_width: 116.0,
            writing: None,
            splitting: None,
            exporting: None,
            making: None,
        }
    }

    fn keep_a_trace(&mut self) {
        self.trace = crate::trace::Trace::from_env();
        if self.trace.is_some() {
            self.editor.keep_a_ledger(true);
        }
        self.meter = crate::meter::Meter::from_env();
    }

    fn take_in_what_was_drawn(&mut self, ctx: &egui::Context) {
        let taking = crate::moment::Moment::now();
        self.gather(ctx);
        if let Some(meter) = self.meter.as_mut() {
            meter.span("gather", taking.elapsed());
        }
        for (id, held, slot) in self.tiles.drop_stale_except(self.rung()) {
            self.retire(id, held, slot);
        }
        if let Some(landing) = self.landing
            && self.running.is_none()
            && !self.tiles.stale_on(landing.page)
        {
            self.landing = None;
        }
    }

    fn hold_the_frame_rate(&mut self, ctx: &egui::Context) {
        if ctx.current_pass_index() > 0 {
            return;
        }
        let now = crate::moment::Moment::now();
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(interval) = crate::window_state::frame_interval()
            && let Some(last) = self.painted_at
            && let Some(rest) = interval.checked_sub(now.since(last))
        {
            std::thread::sleep(rest);
            self.painted_at = Some(now.after(rest));
            return;
        }
        self.painted_at = Some(now);
    }

    fn close_the_frame(&mut self, began: crate::moment::Moment) {
        let (frame, held, waiting) = (self.frame, self.tiles.len(), self.tiles.waiting_count());
        let held_mb = self.tiles.held_bytes() / (1 << 20);
        if let Some(meter) = self.meter.as_mut() {
            meter.count("held", held);
            meter.count("held_MB", held_mb);
            meter.count("waiting", waiting);
            meter.end(frame, began.elapsed());
        }
    }

    pub(crate) fn point_at(&mut self, pointing: Pointing) {
        let pointing = match pointing {
            Pointing::Block { page, block } if !self.block_is_there(page, block) => {
                Pointing::Nothing
            }
            Pointing::Block { .. } | Pointing::Text { .. } if !self.tool.edits_text() => {
                Pointing::Nothing
            }
            Pointing::Object { .. } if self.tool.draws_on_the_page() => Pointing::Nothing,
            other => other,
        };
        if !self.chosen.holds(pointing) {
            self.chosen = Chosen::default();
        }
        if let Pointing::Text { page, block, .. } = pointing
            && self.pointing.typing_in(page) != Some(block)
            && !self.editor.is_busy()
        {
            self.editor.read_block_by_glyphs(page, block);
        }
        self.pointing = pointing;
    }

    pub(crate) fn choose(&mut self, page: usize, mut blocks: Vec<usize>, objects: Vec<usize>) {
        blocks.retain(|block| self.block_is_there(page, *block));
        let first = match (blocks.first(), objects.first()) {
            (Some(block), _) => Pointing::Block {
                page,
                block: *block,
            },
            (None, Some(object)) => Pointing::Object {
                page,
                object: *object,
            },
            (None, None) => Pointing::Nothing,
        };
        self.chosen = Chosen {
            page,
            blocks,
            objects,
        };
        self.pointing = first;
        if !self.chosen.holds(first) {
            self.chosen = Chosen::default();
        }
    }

    pub(crate) fn block_is_there(&self, page: usize, block: usize) -> bool {
        self.overlay(page)
            .is_some_and(|overlay| Pointing::Nothing.block_stands(overlay, page, block))
    }

    pub(crate) fn rung(&self) -> usize {
        ZOOMS
            .iter()
            .position(|step| *step >= self.zoom - 1e-9)
            .unwrap_or(ZOOMS.len() - 1)
    }

    pub(crate) fn send(&mut self, job: Option<EditJob>) {
        let Some(job) = job else { return };
        self.running = Some(Running {
            handle: std::thread::spawn(move || job.run()),
        });
    }

    fn collect(&mut self, ctx: &egui::Context) {
        let Some(running) = &self.running else { return };
        if !running.handle.is_finished() {
            ctx.request_repaint();
            return;
        }
        let Some(running) = self.running.take() else {
            return;
        };
        let Ok(outcome) = running.handle.join() else {
            self.reselect = None;
            self.resume = None;
            self.input.landed(pdf_app::draft::Landing::Refused(
                Message::EditFailedUnexpectedly,
            ));
            self.editor.say(Message::EditFailedUnexpectedly);
            self.editor.abandon_record("the edit thread panicked");
            return;
        };
        self.took_back(outcome);
    }

    pub(crate) fn took_back(&mut self, outcome: pdf_app::EditOutcome) -> Applied {
        let applied = self.editor.adopt(outcome);
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.ai.tools.note_applied(&applied);
            if self.editor.pages_redrawn() {
                self.ai.tools.pages_moved();
            }
        }
        if self.editor.pages_redrawn() {
            if self.turning_to.is_none() {
                self.chosen_pages.clear();
            }
            let renumber = self
                .renumber
                .take()
                .filter(|renumber| renumber.was.len() == self.editor.page_count());
            if let Some(renumber) = renumber {
                let mut thumbs = renumber.follow(&self.thumbs);
                for page in &renumber.turned {
                    if let Some(thumb) = thumbs.get_mut(page) {
                        thumb.turn += renumber.quarter_turns;
                        thumb.fresh = false;
                    }
                }
                self.thumbs = thumbs;
                self.page_motion.renumber(&renumber);
            } else {
                self.thumbs.clear();
                self.page_motion.forget();
            }
            self.page_preview = None;
            self.scenes.clear();
            self.failed.clear();
            self.tiles = Ledger::new();
            self.textures.clear();
            self.waiting_textures.clear();
            let why = self.editor.redraw_reason().unwrap_or("no reason given");
            if let Some(meter) = self.meter.as_mut() {
                meter.note(&format!("every picture let go: {why}"));
            }
            if let Some(page) = self.turning_to.take() {
                self.goto(page.min(self.editor.page_count().saturating_sub(1)));
            }
        } else {
            self.turning_to = None;
            self.renumber = None;
            self.page_preview = None;
        }
        if self.editor.pages_redrawn() {
            self.search_the_document_again();
        }
        if let Applied::Changed { page, region } = applied {
            self.forget_what_was_found_on(page);
            self.failed.clear();
            self.input.landed(pdf_app::draft::Landing::Committed);
            if let (Some((row, stop)), Some(resume)) =
                (self.editor.landed_caret(), self.resume.as_mut())
            {
                resume.2 = row;
                resume.3 = stop;
            }
            self.resume_anchor = self.editor.landed_anchor();
            self.point_at(Pointing::Nothing);
            self.retire_stale(page, region);
            if let Some(thumb) = self.thumbs.get_mut(&page) {
                thumb.fresh = false;
            }
            if let Some((target_page, _)) = self.reselect
                && target_page != page
            {
                self.reselect = None;
            }
            if self.editor.leaf(page).is_some() {
                self.settle_on(page);
            }
        } else {
            self.reselect = None;
            self.reselect_object = None;
            self.resume = None;
            let landing = match &applied {
                Applied::Refused(reason) => TextLanding::Refused(Message::Refused(reason.clone())),
                _ => TextLanding::Unchanged,
            };
            let refused = self.input.landed(landing);
            if refused > 0 {
                let said = Message::AndPressesRefused {
                    said: Box::new(self.editor.status().clone()),
                    refused,
                };
                self.editor.say(said);
            }
        }
        applied
    }

    fn settle_on(&mut self, page: usize) {
        self.find_the_moved_block(page);
        self.point_at_the_shaped_picture(page);
        self.enter_the_new_text(page);
        self.restore_caret(page);
    }

    fn point_at_the_shaped_picture(&mut self, page: usize) {
        let Some((wanted, put)) = self.reselect_object else {
            return;
        };
        if wanted != page {
            return;
        }
        self.reselect_object = None;
        if let Some(object) = self
            .overlay(page)
            .and_then(|overlay| pdf_app::view::object_put_at(&overlay.objects, &put))
        {
            self.point_at(Pointing::Object { page, object });
        }
    }

    fn find_the_moved_block(&mut self, page: usize) {
        let Some((wanted, block)) = self.reselect else {
            return;
        };
        if wanted != page {
            return;
        }
        self.reselect = None;
        let Some(overlay) = self.overlay(page) else {
            return;
        };
        if overlay.blocks.get(block).is_some() {
            self.point_at(Pointing::Block { page, block });
        }
    }

    fn page_read(
        &mut self,
        page: usize,
        view: Result<std::sync::Arc<pdf_session::PageView>, String>,
    ) {
        match view {
            Ok(view) => {
                self.editor.adopt_page(page, view);
                self.settle_on(page);
            }
            Err(reason) => {
                if self.resume.is_some_and(|(wanted, ..)| wanted == page) {
                    self.resume = None;
                    self.input
                        .set_aside(&Message::PageWillNotReadBack(page + 1));
                }
                let said = Message::PageSaid {
                    page: page + 1,
                    said: reason.clone(),
                };
                self.editor.say(said);
                self.failed.insert(page, reason);
            }
        }
    }

    fn tile_arrived(
        &mut self,
        ctx: &egui::Context,
        id: pdf_app::tiles::TileId,
        pixels: Result<pdf_app::painter::TilePixels, String>,
        (was, epoch): (u64, u64),
    ) -> bool {
        if id.zoom == crate::pages::THUMB_RUNG {
            if was == epoch
                && let Ok(pixels) = pixels
            {
                self.thumbnail_arrived(ctx, id.page, &pixels);
            }
            return false;
        }
        if was != epoch {
            self.stale_answers += 1;
            return false;
        }
        let pixels = match pixels {
            Ok(pixels) => pixels,
            Err(reason) => {
                if let Some(held) = self.tiles.give_up(id) {
                    self.retire(id, held, Slot::Shown);
                }
                let frame = self.frame;
                if let Some(trace) = self.trace.as_mut() {
                    trace.note(frame, "tile-refused", &format!("{id:?} {reason}"));
                }
                return false;
            }
        };
        let Some(scale) = ZOOMS.get(id.zoom) else {
            return false;
        };
        let Some(page_pixels) = self.editor.page_pixels(id.page, *scale) else {
            return false;
        };
        let Some(box_pixels) = tile_box(page_pixels, id.col, id.row) else {
            return false;
        };
        let size = (pixels.width as usize, pixels.height as usize);
        let image = egui::ColorImage::from_rgba_unmultiplied([size.0, size.1], &pixels.rgba);
        let options = egui::TextureOptions {
            magnification: egui::TextureFilter::Linear,
            minification: egui::TextureFilter::Linear,
            ..egui::TextureOptions::LINEAR
        };
        let texture = match self.spare.get_mut(&size).and_then(std::vec::Vec::pop) {
            Some(mut spare) => {
                spare.set(image, options);
                spare
            }
            None => ctx.load_texture(format!("tile {}x{}", size.0, size.1), image, options),
        };
        let held = Held {
            box_pixels,
            size,
            epoch: was,
            used: self.frame,
            stale: false,
        };
        let replaced = match self.tiles.arrive(id, held) {
            Arrival::Shown => self.textures.insert(id, texture),
            Arrival::Waiting => self.waiting_textures.insert(id, texture),
        };
        if let Some(old) = replaced {
            self.keep_spare(old);
        }
        true
    }

    fn gather(&mut self, ctx: &egui::Context) {
        let epoch = self.editor.epoch();
        let mut uploaded = 0_usize;
        for done in self.painter.collect() {
            match done {
                Done::Read {
                    page,
                    view,
                    epoch: was,
                } => {
                    if was == epoch {
                        self.page_read(page, view);
                    }
                }
                Done::Searched {
                    page,
                    needle,
                    hits,
                    epoch: was,
                } => {
                    if was == epoch {
                        self.page_searched(page, &needle, hits);
                    }
                }
                Done::Tile {
                    id,
                    pixels,
                    epoch: was,
                } => {
                    if self.tile_arrived(ctx, id, pixels, (was, epoch)) {
                        uploaded += 1;
                    }
                }
            }
        }
        if uploaded > 0
            && let Some(meter) = self.meter.as_mut()
        {
            meter.count("uploaded", uploaded);
        }
        let evicted = self.tiles.evict(crate::window_state::keep_bytes());
        if !evicted.is_empty()
            && let Some(meter) = self.meter.as_mut()
        {
            meter.count("evicted", evicted.len());
        }
        for (id, held) in evicted {
            self.retire(id, held, Slot::Shown);
        }
    }

    pub(crate) fn zoom_to(&mut self, zoom: f64, pointer: Option<egui::Vec2>) {
        let zoom = zoom.clamp(FURTHEST, CLOSEST);
        if (zoom - self.zoom).abs() < 1e-9 {
            return;
        }
        #[allow(clippy::cast_possible_truncation)]
        let ratio = (zoom / self.zoom) as f32;
        let (width, height) = self.editor.strip().size();
        #[allow(clippy::cast_possible_truncation)]
        let content = ((width * self.zoom) as f32, (height * self.zoom) as f32);
        let pointer = pointer.unwrap_or(egui::vec2(self.view.x / 2.0, self.view.y / 2.0));
        let (x, y) = zoom_anchor(
            (self.offset.x, self.offset.y),
            (pointer.x, pointer.y),
            ratio,
            content,
            (self.view.x, self.view.y),
            crate::canvas::DESK_MARGIN,
        );
        self.wanted_offset = Some(egui::vec2(x, y));
        self.zoom = zoom;
    }

    pub(crate) fn zoom_by(&mut self, closer: bool, pointer: Option<egui::Vec2>) {
        let next = if closer {
            ZOOMS.iter().copied().find(|step| *step > self.zoom + 1e-9)
        } else {
            ZOOMS
                .iter()
                .rev()
                .copied()
                .find(|step| *step < self.zoom - 1e-9)
        };
        if let Some(next) = next {
            self.zoom_to(next, pointer);
        }
    }

    pub(crate) fn open_address(&mut self, uri: String) {
        if !pdf_app::links::is_openable(&uri) {
            let said = Message::WillNotOpenFromDocument(uri);
            self.editor.say(said);
            return;
        }
        let said = match open_externally(&uri) {
            Ok(()) => Message::Opened(uri),
            Err(error) => Message::CouldNotOpen {
                uri,
                why: error.to_string(),
            },
        };
        self.editor.say(said);
    }

    pub(crate) fn follow(&mut self, link: &pdf_content::Link) {
        match link.followed() {
            pdf_content::Followed::Page(destination) => {
                let wanted = destination
                    .page
                    .filter(|page| *page < self.editor.page_count());
                let view = destination.view;
                let said = if let Some(page) = wanted {
                    self.goto_view(page, view);
                    Message::WentToPage(page + 1)
                } else {
                    Message::LinkLeadsOffTheDocument
                };
                self.editor.say(said);
            }
            pdf_content::Followed::Uri(uri) => {
                let uri = String::from_utf8_lossy(uri).into_owned();
                self.open_address(uri);
            }
            pdf_content::Followed::Other(_) => {
                let said = Message::LinkKindNotOpenable;
                self.editor.say(said);
            }
            pdf_content::Followed::Nothing => {
                let said = Message::LinkPointsNowhere;
                self.editor.say(said);
            }
        }
    }

    pub(crate) fn goto_view(&mut self, page: usize, view: pdf_content::View) {
        if let Some(zoom) = self.zoom_for(page, view) {
            self.zoom_to(zoom, None);
        }
        self.goto(page);
        self.scroll_down_the_page(page, Self::top_of(view));
    }

    fn zoom_for(&self, page: usize, view: pdf_content::View) -> Option<f64> {
        let (width, height) = self.editor.strip().page_size(page)?;
        if width <= 0.0 || height <= 0.0 {
            return None;
        }
        let across = f64::from(self.view.x - 2.0 * crate::canvas::DESK_MARGIN);
        let down = f64::from(self.view.y - 2.0 * crate::canvas::DESK_MARGIN);
        if across <= 0.0 || down <= 0.0 {
            return None;
        }
        match view {
            pdf_content::View::Fit | pdf_content::View::FitB => {
                Some((across / width).min(down / height))
            }
            pdf_content::View::FitH { .. } | pdf_content::View::FitBH { .. } => {
                Some(across / width)
            }
            pdf_content::View::FitV { .. } | pdf_content::View::FitBV { .. } => Some(down / height),
            pdf_content::View::FitR { rect } => {
                let [x0, y0, x1, y1] = rect;
                let (wide, tall) = ((x1 - x0).abs(), (y1 - y0).abs());
                (wide > 0.0 && tall > 0.0).then(|| (across / wide).min(down / tall))
            }
            pdf_content::View::Xyz { zoom, .. } => zoom.filter(|zoom| *zoom > 0.0),
            pdf_content::View::Unknown => None,
        }
    }

    const fn top_of(view: pdf_content::View) -> Option<f64> {
        match view {
            pdf_content::View::Xyz { top, .. }
            | pdf_content::View::FitH { top }
            | pdf_content::View::FitBH { top } => top,
            pdf_content::View::FitR { rect } => Some(rect[3]),
            _ => None,
        }
    }

    fn scroll_down_the_page(&mut self, page: usize, top: Option<f64>) {
        let (Some(top), Some((_, height))) = (top, self.editor.strip().page_size(page)) else {
            return;
        };
        let below = (height - top).clamp(0.0, height);
        if below <= 0.0 {
            return;
        }
        let Some(wanted) = self.wanted_offset else {
            return;
        };
        #[allow(clippy::cast_possible_truncation)]
        let down = (below * self.zoom) as f32;
        self.wanted_offset = Some(egui::vec2(wanted.x, wanted.y + down));
    }

    pub(crate) fn goto(&mut self, page: usize) {
        let Some((_, top)) = self.editor.strip().origin(page) else {
            return;
        };
        #[allow(clippy::cast_possible_truncation)]
        let y = if page == 0 {
            0.0
        } else {
            crate::canvas::DESK_MARGIN + ((top - pdf_app::strip::GAP / 2.0) * self.zoom) as f32
        };
        self.wanted_offset = Some(egui::vec2(self.offset.x, y));
        self.point_at(Pointing::Nothing);
        self.put_the_drag_down();
    }

    pub(crate) fn overlay(&self, page: usize) -> Option<&pdf_app::Overlay> {
        self.editor.leaf(page).map(|leaf| &leaf.overlay)
    }

    pub(crate) fn rows_of(&self, page: usize, block: usize) -> Option<Vec<usize>> {
        let overlay = self.overlay(page)?;
        Some(overlay.blocks.get(block)?.lines.clone())
    }

    pub(crate) fn frames_of(&self, page: usize) -> &[[f64; 4]] {
        self.editor.frame_boxes(page)
    }

    pub(crate) fn layout_in(&self, page: usize, frame: usize) -> Option<[f64; 4]> {
        let block = self.editor.leaf(page)?.overlay.blocks.get(frame)?;
        (!block.anchors.is_empty()).then_some(block.layout_pixels)
    }
}

pub(crate) fn name_of(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

impl Window {}

impl Window {
    pub(crate) fn anchors_of(&self, page: usize, block: usize) -> Option<Vec<String>> {
        Some(self.overlay(page)?.blocks.get(block)?.anchors.clone())
    }

    pub(crate) fn history(&mut self, ctx: &egui::Context) {
        let in_text = self.pointing.editing() || self.resume.is_some() || self.input.pending();
        if self.editor.is_busy() && !in_text {
            return;
        }
        let (back, forward) = ctx.input_mut(history_keys);
        if !back && !forward {
            return;
        }
        if in_text {
            let intent = if back { Intent::Undo } else { Intent::Redo };
            if let Err(reason) = self.input.press(intent) {
                self.editor.say(reason);
            }
            return;
        }
        self.walk_history(back);
    }

    pub(crate) fn walk_history(&mut self, back: bool) -> bool {
        let job = if back && self.editor.can_undo() {
            self.editor.begin_undo()
        } else if !back && self.editor.can_redo() {
            self.editor.begin_redo()
        } else {
            None
        };
        let started = job.is_some();
        if started {
            self.put_the_drag_down();
        }
        self.send(job);
        started
    }

    pub(crate) fn record_text_events(&mut self, ctx: &egui::Context) {
        if self.trace.is_none() {
            return;
        }
        let seen: Vec<(&'static str, String)> = ctx.input(|input| {
            input
                .events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::Text(text) => Some(("text", text.clone())),
                    egui::Event::Paste(text) => Some(("paste", text.clone())),
                    egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                        Some(("ime.commit", text.clone()))
                    }
                    egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                        Some(if text.is_empty() {
                            ("ime.cancel", String::new())
                        } else {
                            ("ime.preedit", text.clone())
                        })
                    }
                    _ => None,
                })
                .collect()
        });
        let context = self.input_context();
        let frame = self.frame;
        for (kind, value) in seen {
            if let Some(trace) = self.trace.as_mut() {
                trace.event(frame, kind, &value, &context);
            }
        }
    }

    pub(crate) fn take_key(&mut self, ctx: &egui::Context, key: TypedKey, in_text: bool) {
        match key {
            TypedKey::Intent(Intent::Insert(text)) => {
                if !in_text && !self.input.collecting() {
                    if matches!(self.pointing, Pointing::Block { .. }) {
                        self.editor.say(Message::DoubleClickToTypeHere);
                    }
                    return;
                }
                self.input.accept(&text);
                if self.input.collecting() {
                    self.editor.say(Message::DraftIsCollectingWhatYouType);
                }
            }
            TypedKey::Intent(Intent::Delete { .. }) if !in_text => {
                if !self.editor.is_busy() && !self.remove_the_chosen_fields() {
                    self.delete_the_object();
                }
            }
            TypedKey::Intent(intent) => {
                if in_text && let Err(reason) = self.input.press(intent) {
                    self.editor.say(reason);
                }
            }
            TypedKey::Copy if in_text => {
                self.copy_selection(ctx);
            }
            TypedKey::Cut if in_text => {
                if self.copy_selection(ctx) {
                    self.delete_direction(true);
                }
            }
            TypedKey::SelectAll if self.pointing.editing() => self.select_the_block(),
            TypedKey::Copy | TypedKey::Cut | TypedKey::SelectAll => {}
        }
    }

    pub(crate) fn pump(&mut self) {
        loop {
            if self.editor.is_busy() || self.resume.is_some() || self.entering.is_some() {
                return;
            }
            if matches!(self.live_take(), crate::live_typing::LiveStep::Took) {
                continue;
            }
            if self.editor.is_busy() || self.resume.is_some() || self.presenting() {
                return;
            }
            let Some(front) = self.input.front().cloned() else {
                return;
            };
            match front {
                Intent::Caret { step, extend } => {
                    self.input.take();
                    self.step_caret(step, extend);
                }
                Intent::Insert(_) => {
                    self.send_text();
                    return;
                }
                Intent::Delete { .. } => {
                    let selected = matches!(
                        self.pointing,
                        Pointing::Text { caret, .. } if caret.at != caret.anchor
                    );
                    let taken = if selected {
                        self.input.take_one()
                    } else {
                        self.input.take()
                    };
                    if let Some(Intent::Delete { backwards, count }) = taken {
                        self.delete_at_caret(backwards, count);
                    }
                    return;
                }
                Intent::Undo | Intent::Redo => {
                    self.input.take();
                    if !self.walk_history(front == Intent::Undo) {
                        self.input.landed(TextLanding::Unchanged);
                    }
                    return;
                }
            }
        }
    }

    pub(crate) fn text_positions(&self) -> Option<TextPositions> {
        let Pointing::Text { page, block, caret } = self.pointing else {
            return None;
        };
        let overlay = self.overlay(page)?;
        let rows = &overlay.blocks.get(block)?.lines;
        Some((
            page,
            block,
            block_position(&overlay.carets, rows, caret.at)?,
            block_position(&overlay.carets, rows, caret.anchor)?,
        ))
    }

    fn copy_selection(&mut self, ctx: &egui::Context) -> bool {
        let Some((page, block, at, anchor)) = self.text_positions() else {
            return false;
        };
        if at == anchor {
            return false;
        }
        let Some(text) = self.editor.copy_text(page, block, anchor, at) else {
            self.editor.say(Message::RangeCannotBeCopied);
            return false;
        };
        let count = text.chars().count();
        ctx.copy_text(text);
        let said = Message::Copied(count);
        self.editor.say(said);
        true
    }

    fn select_the_block(&mut self) {
        let Pointing::Text { page, block, .. } = self.pointing else {
            return;
        };
        let Some(overlay) = self.overlay(page) else {
            return;
        };
        let Some(lines) = overlay.blocks.get(block).map(|block| &block.lines) else {
            return;
        };
        let (Some(first), Some(last)) = (lines.first(), lines.last()) else {
            return;
        };
        let end = overlay
            .carets
            .iter()
            .filter(|stop| stop.line == *last)
            .map(|stop| stop.offset)
            .max()
            .unwrap_or(0);
        let (Some(anchor), Some(at)) = (
            stop_at(&overlay.carets, *first, 0),
            stop_at(&overlay.carets, *last, end),
        ) else {
            return;
        };
        self.pointing = Pointing::Text {
            page,
            block,
            caret: Caret { at, anchor },
        };
    }

    pub(crate) fn reveal_the_caret(&mut self, clip: egui::Rect) {
        if !std::mem::take(&mut self.reveal_caret) {
            return;
        }
        let Pointing::Text {
            page, block, caret, ..
        } = self.pointing
        else {
            return;
        };
        let Some(laid) = self.laid.iter().copied().find(|laid| laid.page == page) else {
            return;
        };
        let frame = self.frames_of(page).get(block).copied();
        let Some(stop) = self.overlay(page).and_then(|overlay| {
            pdf_app::view::shown_caret(&overlay.carets, &overlay.clusters, caret.at, frame)
        }) else {
            return;
        };
        let (top, bottom) = laid.placed.caret_line(&stop);
        let corner = egui::pos2(laid.placed.origin.0, laid.placed.origin.1);
        let (top, bottom) = (
            corner + egui::vec2(top[0], top[1]),
            corner + egui::vec2(bottom[0], bottom[1]),
        );
        let margin: f32 = 24.0;
        let mut shift = egui::Vec2::ZERO;
        let (high, low) = (top.y.min(bottom.y), top.y.max(bottom.y));
        if high < clip.min.y {
            shift.y = high - clip.min.y - margin;
        } else if low > clip.max.y {
            shift.y = low - clip.max.y + margin;
        }
        if top.x < clip.min.x {
            shift.x = top.x - clip.min.x - margin;
        } else if top.x > clip.max.x {
            shift.x = top.x - clip.max.x + margin;
        }
        if shift != egui::Vec2::ZERO {
            self.wanted_offset = Some(self.offset + shift);
        }
    }

    pub(crate) fn input_context(&self) -> String {
        let pointing = match self.pointing {
            Pointing::Nothing => "nothing".to_owned(),
            Pointing::Block { page, block } => format!("block page={page} block={block}"),
            Pointing::Text { page, block, caret } => format!(
                "text page={page} block={block} caret={} anchor={}",
                caret.at, caret.anchor
            ),
            Pointing::Object { page, object } => format!("object page={page} object={object}"),
        };
        let counted = self.input.accounting();
        format!(
            "pointing={pointing} busy={} resume={} queued={} draft={} presenting={} epoch={} stale_answers={} typed={} committed={} discarded={} commands={}/{}/{} balanced={}",
            self.editor.is_busy(),
            self.resume.is_some(),
            self.input.queued_chars(),
            self.input
                .draft()
                .map_or(0, |draft| draft.text.chars().count()),
            self.presenting(),
            self.editor.epoch(),
            self.stale_answers,
            counted.typed,
            counted.committed,
            counted.discarded,
            counted.commands,
            counted.commands_done,
            counted.commands_refused,
            self.input.balanced(),
        )
    }

    pub(crate) fn pointer_in_view(&self, ctx: &egui::Context) -> Option<egui::Vec2> {
        ctx.input(|input| input.pointer.hover_pos())
            .map(|at| at - self.view_corner)
            .filter(|at| at.x >= 0.0 && at.y >= 0.0 && at.x < self.view.x && at.y < self.view.y)
    }
}

impl Window {
    fn record_the_frame(&mut self) {
        if self.trace.is_none() {
            return;
        }
        let frame = self.frame;
        let mut trace = self.trace.take();
        if let Some(trace) = trace.as_mut() {
            trace.commands(frame, &mut self.editor);
        }
        self.trace = trace;
    }

    fn ai_windows(&mut self, ctx: &egui::Context) {
        #[cfg(not(target_arch = "wasm32"))]
        self.agents_window(ctx);
        #[cfg(target_arch = "wasm32")]
        let _ = ctx;
    }
}

impl Window {
    fn fit_the_window_to_the_screen(&mut self, ctx: &egui::Context) {
        if self.screen_fitted {
            return;
        }
        let Some(monitor) = ctx.input(|input| input.viewport().monitor_size) else {
            return;
        };
        self.screen_fitted = true;
        let (size, at) = crate::room::opening_rect([monitor.x, monitor.y]);
        let now = ctx.input(|input| input.viewport().inner_rect);
        let outside = now.is_none_or(|rect| {
            !crate::room::wholly_on_screen(
                [monitor.x, monitor.y],
                [rect.width(), rect.height()],
                [rect.left(), rect.top()],
            )
        });
        if !outside {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
            size[0], size[1],
        )));
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
            at[0], at[1],
        )));
    }
}

impl eframe::App for Window {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.hold_the_frame_rate(&ctx);
        self.frame += 1;
        let began = crate::moment::Moment::now();
        self.fit_the_window_to_the_screen(&ctx);
        self.collect(&ctx);
        self.ai_windows(&ctx);
        self.record_the_frame();
        self.keep_live(&ctx);
        self.collect_open(&ctx);
        self.collect_writing(&ctx);
        self.collect_making(&ctx);
        self.take_in_what_was_drawn(&ctx);
        self.guard_close(&ctx);
        if self.leaving.is_some() && self.loading.is_none() {
            self.pump();
        }
        if !self.asks_for_a_password() {
            self.read_the_new_document_key(&ctx);
            self.read_the_properties_key(&ctx);
        }
        if self.leaving.is_none()
            && self.loading.is_none()
            && !self.home
            && !self.asks_about_restrictions()
            && !self.asks_for_a_password()
            && self.print_draft.is_none()
            && self.splitting.is_none()
            && self.exporting.is_none()
            && self.chooser.is_none()
            && !self.assistant_is_open()
        {
            self.keys(&ctx);
            if ctx.input_mut(|input| {
                input.consume_key(
                    egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                    egui::Key::S,
                )
            }) {
                self.save_a_copy_as();
            }
            if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::S)) {
                self.save();
            }
            if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::P)) {
                self.open_the_print_dialog();
            }
        }
        if self.leaving.is_some() || self.loading.is_some() {
            ui.disable();
        }
        self.menu_bar(ui);
        self.choose_a_file(&ctx);
        self.take_dropped_files(&ctx);
        self.ask_for_the_password(&ctx);
        if self.home {
            self.status_bar(ui);
            self.home_screen(ui);
            self.confirm_leaving(&ctx);
            return;
        }
        self.keep_the_page_in_the_list();
        self.toolbar(ui);
        self.status_bar(ui);
        self.draft_bar(ui);
        self.page_panel(ui);
        self.contents_panel(ui);
        self.fields_panel(ui);
        #[cfg(not(target_arch = "wasm32"))]
        self.ai_panel(ui);
        self.ask_for_thumbnails(ctx.pixels_per_point());
        let desk = egui::Frame::NONE.fill(desk(self.dark));
        egui::CentralPanel::no_frame().frame(desk).show(ui, |ui| {
            let mut area = egui::ScrollArea::both().auto_shrink([false, false]);
            let asked = self.wanted_offset.take();
            if let Some(offset) = asked {
                area = area.scroll_offset(offset);
            }
            let shown = area.show(ui, |ui| self.document_area(ui, &ctx));
            if let Some(offset) = asked {
                let reach = shown.content_size - shown.inner_rect.size();
                let within =
                    offset.x <= reach.x.max(0.0) + 0.5 && offset.y <= reach.y.max(0.0) + 0.5;
                if within && (shown.state.offset - offset).length() > 0.5 {
                    self.wanted_offset = Some(offset);
                    ctx.request_repaint();
                }
            }
            self.offset = shown.state.offset;
            self.view = shown.inner_rect.size();
            self.view_corner = shown.inner_rect.min;
        });
        if self.painter.busy() > 0 && !self.painter.wakes_the_window() {
            ctx.request_repaint();
        }
        self.warn_of_restrictions(&ctx);
        self.confirm_leaving(&ctx);
        self.close_the_frame(began);
    }
}

pub(crate) const fn kind_name(kind: pdf_semantics::ObjectKind) -> ObjectKind {
    match kind {
        pdf_semantics::ObjectKind::Image => ObjectKind::Picture,
        pdf_semantics::ObjectKind::Form => ObjectKind::Group,
        pdf_semantics::ObjectKind::Shading => ObjectKind::Shading,
        pdf_semantics::ObjectKind::Text(_) | pdf_semantics::ObjectKind::TextRun => ObjectKind::Text,
        pdf_semantics::ObjectKind::Path => ObjectKind::Drawing,
    }
}

pub(crate) const NEAR_STEP: f64 = 1.0;
pub(crate) const FAR_STEP: f64 = 10.0;

fn open_externally(uri: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = std::process::Command::new("xdg-open");
    command.arg(uri).spawn().map(drop)
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    use crate::window_state::{LeaveChoice, Leaving};

    fn window() -> Window {
        let source = pdf_bytes::ByteStore::new(
            pdf_bytes::SourceId::new(0),
            &include_bytes!("../../pdf-edit/tests/data/modifiable-r3.pdf")[..],
        );
        let editor = Editor::open_with(source, b"view").unwrap();
        Window::new(editor, PathBuf::from("/missing/original.pdf"), Vec::new())
    }

    fn move_text(window: &mut Window) {
        let view = pdf_session::interpret_page_fully(
            window.editor.source().unwrap(),
            0,
            b"view",
            None,
            pdf_cli::font_provider(),
        )
        .unwrap();
        let anchor = pdf_edit::SourceAnchor::of(&view.graph.atoms[0].id).encode();
        window.editor.adopt_page(0, std::sync::Arc::new(view));
        assert!(matches!(
            window.editor.move_run(0, &anchor, 1.0, 0.0),
            Applied::Changed { .. }
        ));
    }

    #[test]
    fn the_drawn_frame_stays_on_screen_while_the_first_character_is_away() {
        use pdf_app::draft::Intent;
        let mut window = window();
        let view = pdf_session::interpret_page_fully(
            window.editor.source().unwrap(),
            0,
            b"view",
            window.editor.grouping(0).as_deref(),
            pdf_cli::font_provider(),
        )
        .unwrap();
        window.editor.adopt_page(0, std::sync::Arc::new(view));
        let frame = window
            .editor
            .frame_in_user_space(0, [20.0, 40.0, 220.0, 80.0])
            .expect("a frame on the page");
        window.text_draft = Some(crate::window_state::TextDraft {
            page: 0,
            frame,
            family: "Helvetica".to_owned(),
            size: 12.0,
            bold: false,
            italic: false,
            fill: None,
            alignment: pdf_edit::Alignment::Start,
            flows_round: false,
        });
        window.draft_pixels = Some((0, [20.0, 40.0, 220.0, 80.0]));
        window
            .input
            .press(Intent::Insert("H".to_owned()))
            .expect("one character queued");

        window.send_text();

        assert!(
            window.text_draft.is_none(),
            "the draft became an edit and is no longer waiting for a first key"
        );
        assert_eq!(
            window.draft_pixels,
            Some((0, [20.0, 40.0, 220.0, 80.0])),
            "and its rectangle is still drawn, with the block not yet back"
        );
        assert_eq!(
            window.entering,
            Some((0, [20.0, 40.0, 220.0, 80.0])),
            "the same rectangle is what the block will be found for"
        );
    }

    #[test]
    fn text_is_not_chosen_with_a_tool_that_does_not_edit_it() {
        use crate::window_state::Tool;
        let mut window = window();
        let caret = crate::window_state::Caret { at: 0, anchor: 0 };
        let typing = Pointing::Text {
            page: 0,
            block: 0,
            caret,
        };
        for tool in [
            Tool::Pen,
            Tool::Highlighter,
            Tool::Shape,
            Tool::Form,
            Tool::Link,
            Tool::Picture,
        ] {
            window.tool = tool;
            window.point_at(typing);
            assert_eq!(window.pointing, Pointing::Nothing, "{tool:?}");
            window.point_at(Pointing::Block { page: 0, block: 0 });
            assert_eq!(window.pointing, Pointing::Nothing, "{tool:?}");
        }
        window.tool = Tool::Select;
        window.point_at(typing);
        assert_eq!(window.pointing, typing);
    }

    #[test]
    fn a_drawing_is_not_chosen_with_a_tool_that_draws() {
        use crate::window_state::Tool;
        let mut window = window();
        let drawing = Pointing::Object { page: 0, object: 0 };
        for tool in [Tool::Pen, Tool::Highlighter, Tool::Shape, Tool::Picture] {
            window.tool = tool;
            window.point_at(drawing);
            assert_eq!(window.pointing, Pointing::Nothing, "{tool:?}");
        }
        for tool in [Tool::Select, Tool::Text, Tool::Form, Tool::Link] {
            window.tool = tool;
            window.point_at(drawing);
            assert_eq!(window.pointing, drawing, "{tool:?}");
        }
    }

    #[test]
    fn keys_typed_into_a_block_are_written_once_when_it_is_left() {
        use pdf_app::draft::Intent;
        let mut window = window();
        let view = pdf_session::interpret_page_fully(
            window.editor.source().unwrap(),
            0,
            b"view",
            window.editor.grouping(0).as_deref(),
            pdf_cli::font_provider(),
        )
        .unwrap();
        window.editor.adopt_page(0, std::sync::Arc::new(view));
        let overlay = window.editor.leaf(0).unwrap().overlay.clone();
        let (block, row) = overlay
            .blocks
            .iter()
            .enumerate()
            .find_map(|(index, block)| Some((index, *block.lines.first()?)))
            .expect("a block with a row");
        let first = window.editor.frame_boxes(0)[block];
        window
            .editor
            .preview_frame(0, block, [first[0], first[1], first[2] + 80.0, first[3]]);
        window.editor.finish_frame_resize(0, block, first);
        let stops = overlay
            .carets
            .iter()
            .filter(|stop| stop.line == row)
            .count()
            - 1;
        let at = overlay
            .carets
            .iter()
            .position(|stop| stop.line == row && stop.offset == stops)
            .unwrap();
        let letter = window
            .editor
            .copy_text(0, block, (0, stops - 1), (0, stops))
            .expect("the row's last letter");
        window.pointing = Pointing::Text {
            page: 0,
            block,
            caret: crate::window_state::Caret { at, anchor: at },
        };
        let before = window.editor.source().unwrap().as_bytes().to_vec();
        for key in [
            Intent::Insert(letter.clone()),
            Intent::Insert(letter.clone()),
            Intent::Delete {
                backwards: true,
                count: 1,
            },
            Intent::Insert(letter.clone()),
            Intent::Undo,
        ] {
            window.input.press(key).unwrap();
        }
        window.pump();

        assert!(window.input.front().is_none(), "every key was taken");
        assert!(!window.editor.is_busy(), "and none of them started an edit");
        assert_eq!(
            window.editor.source().unwrap().as_bytes(),
            &before[..],
            "the file is as it was"
        );
        let live = window
            .live
            .as_ref()
            .expect("the block is being typed into live");
        assert!(!live.written);
        assert!(window.input.balanced(), "every character is accounted for");

        window.write_live(true);
        let after = window.editor.source().unwrap().as_bytes().to_vec();
        assert_ne!(after, before, "leaving the block wrote it");
        assert!(
            window.live.as_ref().is_none_or(|live| live.written),
            "and nothing is waiting to be written"
        );
        if window.editor.leaf(0).is_none() {
            let view = pdf_session::interpret_page_fully(
                window.editor.source().unwrap(),
                0,
                b"view",
                window.editor.grouping(0).as_deref(),
                pdf_cli::font_provider(),
            )
            .unwrap();
            window.editor.adopt_page(0, std::sync::Arc::new(view));
        }
        assert_eq!(
            window
                .editor
                .copy_text(0, block, (0, stops - 1), (0, stops + 1))
                .as_deref(),
            Some(format!("{letter}{letter}").as_str()),
            "what the file has is what was shown: two letters typed, one taken \
             back, one typed again and undone"
        );
        assert!(window.input.balanced());
    }

    #[test]
    fn a_drawn_frame_is_let_go_of_only_when_the_person_says_so() {
        let mut window = window();
        window.text_draft = Some(crate::window_state::TextDraft {
            page: 0,
            frame: [0.0, 0.0, 10.0, 10.0],
            family: "Helvetica".to_owned(),
            size: 12.0,
            bold: false,
            italic: false,
            fill: None,
            alignment: pdf_edit::Alignment::Start,
            flows_round: false,
        });
        window.draft_pixels = Some((0, [0.0, 0.0, 10.0, 10.0]));

        window.take_up(crate::window_state::Tool::Pen);
        assert!(
            window.text_draft.is_some(),
            "picking up another tool leaves the frame where it was drawn"
        );

        window.drop_the_text_draft();
        assert!(window.text_draft.is_none());
        assert!(window.draft_pixels.is_none());
    }

    #[test]
    fn a_selected_block_is_something_delete_acts_on() {
        let mut window = window();
        assert!(!window.selected());
        window.pointing = crate::window_state::Pointing::Block { page: 0, block: 0 };
        assert!(window.selected(), "the toolbar's Delete is offered for it");
    }

    #[test]
    fn a_deleted_block_cannot_be_selected_by_any_way_of_selecting() {
        use crate::window_state::Pointing;
        let mut window = window();
        let read = |window: &mut Window| {
            let view = pdf_session::interpret_page_fully(
                window.editor.source().unwrap(),
                0,
                b"view",
                window.editor.grouping(0).as_deref(),
                pdf_cli::font_provider(),
            )
            .unwrap();
            window.editor.adopt_page(0, std::sync::Arc::new(view));
        };
        read(&mut window);
        let anchors = window.overlay(0).unwrap().blocks[0].anchors.clone();
        window.choose(0, vec![0], Vec::new());
        assert_eq!(window.pointing, Pointing::Block { page: 0, block: 0 });
        assert!(matches!(
            window.editor.delete_block(0, &anchors),
            Applied::Changed { .. }
        ));
        read(&mut window);
        assert_eq!(window.overlay(0).unwrap().blocks.len(), 1, "kept for undo");

        window.point_at(Pointing::Block { page: 0, block: 0 });
        assert_eq!(window.pointing, Pointing::Nothing, "a click or Tab");
        window.choose(0, vec![0], Vec::new());
        assert_eq!(window.pointing, Pointing::Nothing, "a marquee");
        assert_eq!(window.chosen.count(), 0);
    }

    #[test]
    fn a_new_document_is_one_blank_page_with_no_file_until_it_is_saved() {
        let mut window = window();
        move_text(&mut window);
        assert!(window.unsaved());
        window.new_document(crate::chrome::A4);
        assert!(
            matches!(window.leaving, Some(Leaving::New(_))),
            "unsaved work is asked about first"
        );
        assert!(!window.untitled, "nothing was replaced yet");
        let ctx = egui::Context::default();
        window.resolve_leaving(LeaveChoice::Discard, &ctx);
        assert!(window.untitled);
        assert!(window.has_document(), "there is a document to edit");
        assert!(window.opened.as_os_str().is_empty(), "it came from no file");
        assert_eq!(window.editor.page_count(), 1);
        let geometry = window.editor.geometry(0).expect("the page lays out");
        let [x0, y0, x1, y1] = geometry.media_box;
        let a4 = crate::chrome::A4;
        assert!(
            x0.abs() < 0.01 && y0.abs() < 0.01,
            "the page starts at the origin"
        );
        assert!(
            (x1 - a4[0]).abs() < 0.01 && (y1 - a4[1]).abs() < 0.01,
            "A4: {x1} by {y1}"
        );
        assert!(!window.unsaved(), "nothing has been done to it yet");
        assert!(!window.save());
        assert!(window.chooser.is_some());
        assert!(window.destination.as_os_str().is_empty());
    }

    #[test]
    fn what_is_typed_in_the_properties_window_is_what_the_document_says() {
        let mut window = window();
        window.open_the_properties();
        let panel = window.properties.as_mut().expect("the window opens");
        assert!(
            panel.changed().is_none(),
            "nothing typed is nothing to save"
        );
        let producer = panel.was.producer.clone();
        panel.title = "  รายงานประจำปี  ".to_owned();
        panel.author = "Phan".to_owned();
        let edit = panel.changed().expect("a title typed is a change");
        assert_eq!(
            edit.title.as_deref(),
            Some("รายงานประจำปี"),
            "the spaces round what was typed are not part of it"
        );
        assert!(
            edit.subject.is_none() && edit.keywords.is_none(),
            "only what was typed is asked for: {edit:?}"
        );
        assert!(edit.modified.is_some(), "a change is dated");

        let applied = window.editor.describe(&edit);
        assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
        let facts = window
            .editor
            .facts()
            .expect("the document describes itself");
        assert_eq!(facts.info.title, "รายงานประจำปี");
        assert_eq!(facts.info.author, "Phan");
        assert_eq!(
            facts.info.producer, producer,
            "what was not typed over is still what it was"
        );
        assert!(
            facts.protection.is_some(),
            "this document is protected, which is the case the writing has to get right"
        );
        assert!(
            window.editor.can_undo(),
            "describing a document is one step a person can take back"
        );
        assert!(matches!(window.editor.undo(), Applied::Changed { .. }));
        assert_eq!(
            window
                .editor
                .facts()
                .expect("it still describes itself")
                .info
                .title,
            "",
            "taking it back leaves the document saying what it said before"
        );
    }

    #[test]
    fn switching_documents_preserves_unsaved_work_until_a_choice() {
        let mut window = window();
        assert!(!window.unsaved());
        move_text(&mut window);
        let changed = window.editor.source().unwrap().as_bytes().to_vec();
        assert!(window.unsaved());
        window.open(Path::new("/missing/second.pdf"));
        assert!(matches!(window.leaving, Some(Leaving::Open(..))));
        assert!(window.loading.is_none());
        let ctx = egui::Context::default();
        window.resolve_leaving(LeaveChoice::Save, &ctx);
        assert!(window.loading.is_none(), "failed save must not leave");
        assert!(window.unsaved());
        assert_eq!(window.editor.source().unwrap().as_bytes(), changed);
        window.resolve_leaving(LeaveChoice::Cancel, &ctx);
        assert!(window.leaving.is_none());
        assert_eq!(window.editor.source().unwrap().as_bytes(), changed);
        window.leaving = Some(Leaving::Close);
        window.resolve_leaving(LeaveChoice::Discard, &ctx);
        assert!(window.close_confirmed);
    }

    #[test]
    fn a_native_close_event_is_cancelled_only_when_work_needs_a_decision() {
        let mut window = window();
        let ctx = egui::Context::default();
        let close = |window: &mut Window| {
            let mut input = egui::RawInput::default();
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .events
                .push(egui::ViewportEvent::Close);
            let output = ctx.run_ui(input, |ui| window.guard_close(ui.ctx()));
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::CancelClose))
        };
        assert!(!close(&mut window), "clean close is the control");
        move_text(&mut window);
        assert!(close(&mut window));
        assert!(matches!(window.leaving, Some(Leaving::Close)));
        window.resolve_leaving(LeaveChoice::Cancel, &ctx);
        assert!(!window.close_confirmed);
        assert!(window.unsaved());
    }

    #[test]
    fn only_a_successful_disk_save_marks_work_saved_and_drafts_still_block_leaving() {
        let mut window = window();
        move_text(&mut window);
        let dir = std::env::temp_dir().join(format!("panpdf-window-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        window.destination = dir.join("copy.pdf");
        let _ = std::fs::remove_file(&window.destination);
        assert!(window.save());
        assert!(!window.unsaved());
        move_text(&mut window);
        assert!(window.unsaved());
        assert!(
            window.save(),
            "same session may atomically replace its own save"
        );
        assert!(!window.unsaved());
        window.input.accept("not yet applied");
        assert!(window.unsaved());
        assert!(!window.save(), "queued text must not be reported as saved");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
