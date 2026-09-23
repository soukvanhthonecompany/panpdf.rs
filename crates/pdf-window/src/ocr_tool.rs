use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};

use eframe::egui;

use pdf_app::ocr_choice::Choice;
use pdf_app::wording::{Command, Done, Message};
use pdf_edit::stamp::Only;
use pdf_ocr::Quality;
use pdf_paint::PaintAtomKind;

use crate::window_state::{OcrDraft, OcrFetch, OcrReading, OcrWhich, PageRead, Window};

const PANEL_WIDTH: f32 = 340.0;

const MOST_WORKERS: usize = 4;

const KNOWN: [&str; 3] = ["lao", "tha", "eng"];

fn choice_file() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("state"))
        })?;
    Some(state.join("panpdf").join("ocr"))
}

fn remembered() -> Choice {
    choice_file()
        .and_then(|file| std::fs::read_to_string(file).ok())
        .and_then(|text| pdf_app::ocr_choice::read(&text))
        .unwrap_or_else(Choice::fresh)
}

fn remember(choice: Choice) {
    let Some(file) = choice_file() else {
        return;
    };
    if let Some(folder) = file.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    let temporary = file.with_extension("new");
    if std::fs::write(&temporary, pdf_app::ocr_choice::write(&choice)).is_ok() {
        let _ = std::fs::rename(&temporary, &file);
    }
}

fn draft() -> OcrDraft {
    let choice = remembered();
    let mut engine = pdf_ocr::Tesseract::locate().ok();
    if let Some(engine) = engine.as_mut() {
        engine.own = pdf_ocr::store::models_dir(choice.quality);
    }
    let mut draft = OcrDraft {
        engine,
        languages: KNOWN
            .iter()
            .map(|code| ((*code).to_owned(), true))
            .collect(),
        here: Vec::new(),
        choice,
        which: OcrWhich::default(),
        range: String::new(),
        reading: None,
        fetching: None,
        trouble: None,
    };
    draft.take_stock();
    draft
}

impl OcrDraft {
    fn take_stock(&mut self) {
        if let Some(engine) = self.engine.as_mut() {
            engine.own = pdf_ocr::store::models_dir(self.choice.quality);
        }
        let system = self
            .engine
            .as_ref()
            .and_then(|engine| engine.installed_languages().ok())
            .unwrap_or_default();
        let own = pdf_ocr::store::models_dir(self.choice.quality);
        self.here = KNOWN
            .iter()
            .filter(|code| {
                system.iter().any(|have| have == *code)
                    || own.as_deref().is_some_and(|dir| {
                        pdf_ocr::models::model(code, self.choice.quality)
                            .is_some_and(|model| pdf_ocr::store::have(dir, model))
                    })
            })
            .map(|code| (*code).to_owned())
            .collect();
        for (code, chosen) in &mut self.languages {
            if !self.here.contains(code) {
                *chosen = false;
            }
        }
        if self.here.iter().all(|code| {
            !self
                .languages
                .iter()
                .any(|(name, chosen)| name == code && *chosen)
        }) {
            for (code, chosen) in &mut self.languages {
                if self.here.contains(code) {
                    *chosen = true;
                }
            }
        }
    }

    fn chosen(&self) -> Vec<String> {
        self.languages
            .iter()
            .filter(|(_, chosen)| *chosen)
            .map(|(code, _)| code.clone())
            .collect()
    }
}

fn has_text(view: &pdf_session::PageView) -> bool {
    view.graph
        .atoms
        .iter()
        .any(|atom| matches!(atom.kind, PaintAtomKind::Text(_)))
}

pub(crate) fn is_a_scan(view: &pdf_session::PageView) -> bool {
    !has_text(view)
        && view
            .graph
            .atoms
            .iter()
            .any(|atom| matches!(atom.kind, PaintAtomKind::Image(_)))
}

fn read_one(
    source: &pdf_bytes::ByteStore,
    page: usize,
    (credential, fonts): (&[u8], Option<Arc<dyn pdf_content::FontProvider>>),
    (engine, languages, skip_text): (&pdf_ocr::Tesseract, &[String], bool),
    cancel: &AtomicBool,
) -> PageRead {
    let view = match pdf_session::interpret_page_for_display(source, page, credential, None, fonts)
    {
        Ok(view) => view,
        Err(error) => return PageRead::Failed(error.to_string()),
    };
    if skip_text && has_text(&view) {
        return PageRead::HadText;
    }
    match pdf_ocr::read_page(
        &view.layers(),
        &view.program.geometry,
        engine,
        languages,
        cancel,
    ) {
        Ok(reading) => PageRead::Read(reading),
        Err(error) => PageRead::Failed(error.to_string()),
    }
}

impl Window {
    pub(crate) fn scan_notice(&mut self, ctx: &egui::Context, area: egui::Rect) {
        if self.ocr_draft.is_some()
            || self.editor.is_busy()
            || self.scan_notice_shut.as_ref() == Some(&self.opened)
            || !self
                .editor
                .leaf(self.focus)
                .is_some_and(|leaf| is_a_scan(&leaf.view))
        {
            return;
        }
        let lang = self.lang;
        let (mut read, mut shut) = (false, false);
        egui::Area::new(egui::Id::new("scan-notice"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::CENTER_TOP)
            .fixed_pos(area.center_top() + egui::vec2(0.0, 10.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(Message::ThisPageIsAScan.say(lang));
                        let label = Message::Command(Command::RecognizeText).say(lang);
                        let button = egui::Button::new(
                            egui::RichText::new(label.trim_end_matches('\u{2026}'))
                                .color(ui.visuals().selection.stroke.color),
                        )
                        .fill(ui.visuals().selection.bg_fill.gamma_multiply(0.4));
                        read = ui.add(button).clicked();
                        shut = ui.small_button("\u{d7}").clicked();
                    });
                });
            });
        if read {
            self.open_the_ocr_panel();
        }
        if shut {
            self.scan_notice_shut = Some(self.opened.clone());
        }
    }

    pub(crate) fn open_the_ocr_panel(&mut self) {
        if self
            .ocr_draft
            .as_ref()
            .is_some_and(|draft| draft.reading.is_some())
        {
            return;
        }
        self.ocr_draft = Some(draft());
    }

    fn ocr_pages(&self, draft: &OcrDraft) -> Result<Vec<usize>, Message> {
        let count = self.editor.page_count();
        match draft.which {
            OcrWhich::ThisPage => {
                pdf_edit::stamp::pages_of(&(self.focus + 1).to_string(), count, Only::Every)
            }
            OcrWhich::All => pdf_edit::stamp::pages_of("", count, Only::Every),
            OcrWhich::Some => pdf_edit::stamp::pages_of(&draft.range, count, Only::Every),
        }
        .map_err(|error| Message::Refused(error.to_string().into()))
    }

    pub(crate) fn ocr_panel(&mut self, ctx: &egui::Context) {
        if self.ocr_draft.is_none() || !self.has_document() {
            return;
        }
        self.keep_reading();
        self.keep_fetching(ctx);
        let lang = self.lang;
        let count = self.editor.page_count();
        let pages = self.ocr_draft.as_ref().map(|draft| self.ocr_pages(draft));
        let Some(draft) = self.ocr_draft.as_mut() else {
            return;
        };
        let mut asked = Asked::default();
        let title = Message::Command(Command::RecognizeText).say(lang);
        egui::Window::new(title.trim_end_matches('\u{2026}'))
            .id(egui::Id::new("ocr-panel"))
            .collapsible(false)
            .resizable(false)
            .default_width(PANEL_WIDTH)
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 56.0))
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(Message::OcrWhy.say(lang))
                        .size(11.0)
                        .color(ui.visuals().weak_text_color()),
                );
                ui.separator();
                the_choices(ui, draft, (lang, count), &mut asked);
                what_stands_in_the_way(ui, draft, lang, pages.as_ref());
                ui.separator();
                how_far(ui, draft, lang);
                the_buttons(ui, draft, (lang, pages.as_ref()), &mut asked);
            });
        if asked.pressed == Pressed::Stop {
            if let Some(reading) = draft.reading.as_ref() {
                reading.cancel.store(true, Ordering::Relaxed);
            }
            if let Some(fetch) = draft.fetching.as_ref() {
                fetch.cancel.store(true, Ordering::Relaxed);
            }
        }
        if asked.quality != draft.choice.quality {
            draft.choice.quality = asked.quality;
            draft.take_stock();
            asked.remember = true;
        }
        if asked.remember {
            remember(draft.choice);
        }
        match asked.pressed {
            Pressed::Close => self.ocr_draft = None,
            Pressed::GetModel(code) => self.fetch_model(ctx, &code),
            Pressed::GetEngine => self.fetch_engine(ctx),
            Pressed::Read => self.start_reading(ctx),
            Pressed::Nothing | Pressed::Stop => {}
        }
    }

    fn fetch_model(&mut self, ctx: &egui::Context, code: &str) {
        let Some(draft) = self.ocr_draft.as_mut() else {
            return;
        };
        let (Some(model), Some(dir)) = (
            pdf_ocr::models::model(code, draft.choice.quality),
            pdf_ocr::store::models_dir(draft.choice.quality),
        ) else {
            draft.trouble = Some(Message::OcrModelFailed(
                pdf_ocr::FetchError::NoHome.to_string(),
            ));
            return;
        };
        draft.trouble = None;
        let (cancel, seen) = (
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicU64::new(0)),
        );
        let (send, answer) = mpsc::channel();
        let (worker_cancel, worker_seen, ctx) =
            (Arc::clone(&cancel), Arc::clone(&seen), ctx.clone());
        let worker = std::thread::spawn(move || {
            let landed = pdf_ocr::store::fetch(
                model,
                &dir,
                &|bytes| {
                    worker_seen.store(bytes, Ordering::Relaxed);
                    ctx.request_repaint();
                },
                &worker_cancel,
            );
            let _ = send.send(landed.map(|_| ()).map_err(|why| why.to_string()));
            ctx.request_repaint();
        });
        draft.fetching = Some(OcrFetch {
            code: Some(code.to_owned()),
            seen,
            total: model.bytes,
            cancel,
            answer,
            worker: Some(worker),
        });
    }

    fn fetch_engine(&mut self, ctx: &egui::Context) {
        let Some(draft) = self.ocr_draft.as_mut() else {
            return;
        };
        let Some(into) = pdf_ocr::store::own_engine() else {
            draft.trouble = Some(Message::OcrEngineFailed(
                pdf_ocr::FetchError::NoHome.to_string(),
            ));
            return;
        };
        draft.trouble = None;
        let (cancel, seen) = (
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicU64::new(0)),
        );
        let (send, answer) = mpsc::channel();
        let (worker_cancel, ctx) = (Arc::clone(&cancel), ctx.clone());
        let worker = std::thread::spawn(move || {
            let done = pdf_ocr::setup::install(&into, &|_| {}, &worker_cancel);
            let _ = send.send(done.map(|_| ()).map_err(|why| why.to_string()));
            ctx.request_repaint();
        });
        draft.fetching = Some(OcrFetch {
            code: None,
            seen,
            total: 0,
            cancel,
            answer,
            worker: Some(worker),
        });
    }

    fn keep_fetching(&mut self, ctx: &egui::Context) {
        let Some(draft) = self.ocr_draft.as_mut() else {
            return;
        };
        let Some(fetch) = draft.fetching.as_mut() else {
            return;
        };
        let Ok(answer) = fetch.answer.try_recv() else {
            return;
        };
        let was_a_model = fetch.code.is_some();
        if let Some(worker) = fetch.worker.take() {
            let _ = worker.join();
        }
        draft.fetching = None;
        draft.trouble = match answer {
            Ok(()) => None,
            Err(why) if why == pdf_ocr::FetchError::Cancelled.to_string() => None,
            Err(why) if was_a_model => Some(Message::OcrModelFailed(why)),
            Err(why) => Some(Message::OcrEngineFailed(why)),
        };
        if !was_a_model {
            let mut engine = pdf_ocr::Tesseract::locate().ok();
            if let Some(engine) = engine.as_mut() {
                engine.own = pdf_ocr::store::models_dir(draft.choice.quality);
            }
            draft.engine = engine;
        }
        draft.take_stock();
        ctx.request_repaint();
    }

    fn start_reading(&mut self, ctx: &egui::Context) {
        let Some(draft) = self.ocr_draft.as_ref() else {
            return;
        };
        let (Some(engine), Some(source)) = (draft.engine.clone(), self.editor.source().cloned())
        else {
            return;
        };
        let pages = match self.ocr_pages(draft) {
            Ok(pages) => pages,
            Err(why) => {
                self.editor.say(why);
                return;
            }
        };
        let languages = draft.chosen();
        let skip_text = draft.choice.skip_text;
        let cancel = Arc::new(AtomicBool::new(false));
        let next = Arc::new(AtomicUsize::new(0));
        let shared_pages = Arc::new(pages.clone());
        let (send, answers) = mpsc::channel();
        let credential = self.editor.credential().to_vec();
        let fonts = self.editor.fonts();
        let workers = std::thread::available_parallelism()
            .map_or(1, |cores| cores.get().saturating_sub(1))
            .clamp(1, MOST_WORKERS)
            .min(pages.len());
        let handles = (0..workers)
            .map(|_| {
                let (cancel, next, pages, send) = (
                    Arc::clone(&cancel),
                    Arc::clone(&next),
                    Arc::clone(&shared_pages),
                    send.clone(),
                );
                let (source, credential, fonts, engine, languages, ctx) = (
                    source.clone(),
                    credential.clone(),
                    fonts.clone(),
                    engine.clone(),
                    languages.clone(),
                    ctx.clone(),
                );
                std::thread::spawn(move || {
                    loop {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        let Some(&page) = pages.get(next.fetch_add(1, Ordering::Relaxed)) else {
                            break;
                        };
                        let read = read_one(
                            &source,
                            page,
                            (&credential, fonts.clone()),
                            (&engine, &languages, skip_text),
                            &cancel,
                        );
                        if send.send((page, read)).is_err() {
                            break;
                        }
                        ctx.request_repaint();
                    }
                })
            })
            .collect();
        if let Some(draft) = self.ocr_draft.as_mut() {
            draft.reading = Some(OcrReading {
                cancel,
                answers,
                workers: handles,
                pages,
                read: BTreeMap::new(),
                epoch: self.editor.epoch(),
            });
        }
    }

    fn keep_reading(&mut self) {
        let Some(reading) = self
            .ocr_draft
            .as_mut()
            .and_then(|draft| draft.reading.as_mut())
        else {
            return;
        };
        while let Ok((page, read)) = reading.answers.try_recv() {
            reading.read.insert(page, read);
        }
        let stopped = reading.cancel.load(Ordering::Relaxed);
        let finished = reading
            .workers
            .iter()
            .all(std::thread::JoinHandle::is_finished);
        if !(finished && (stopped || reading.read.len() == reading.pages.len())) {
            return;
        }
        if self.editor.is_busy() {
            return;
        }
        let Some(reading) = self
            .ocr_draft
            .as_mut()
            .and_then(|draft| draft.reading.take())
        else {
            return;
        };
        for worker in reading.workers {
            let _ = worker.join();
        }
        if stopped {
            self.editor.say(Done::RecognitionStopped.into());
            return;
        }
        if reading.epoch != self.editor.epoch() {
            self.editor.say(Done::RecognitionOutdated.into());
            return;
        }
        self.write_what_was_read(reading.read);
    }

    fn write_what_was_read(&mut self, read: BTreeMap<usize, PageRead>) {
        let mut layers = Vec::new();
        let (mut had_text, mut unread, mut failed) = (0, 0, None);
        let (mut weight, mut sum) = (0.0_f64, 0.0_f64);
        for (page, outcome) in read {
            match outcome {
                PageRead::Read(reading) => {
                    if reading.layer.words.is_empty() {
                        continue;
                    }
                    #[allow(clippy::cast_precision_loss)]
                    let words = reading.layer.words.len() as f64;
                    weight += words;
                    sum += words * f64::from(reading.confidence.unwrap_or(0.0));
                    layers.push((page, reading.layer));
                }
                PageRead::HadText => had_text += 1,
                PageRead::Failed(why) => {
                    unread += 1;
                    failed.get_or_insert(why);
                }
            }
        }
        if layers.is_empty() {
            let said = match failed {
                Some(why) => Message::Refused(why.into()),
                None if had_text > 0 => Done::NothingToRecognize.into(),
                None => Done::NothingChanged.into(),
            };
            self.editor.say(said);
            return;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let confidence = (sum / weight.max(1.0)).round().clamp(0.0, 100.0) as u8;
        let job = self
            .editor
            .begin_text_layers(layers, (confidence, had_text, unread));
        if job.is_none() {
            self.editor.say(Message::AnotherEditIsRunning);
            return;
        }
        self.send(job);
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
enum Pressed {
    #[default]
    Nothing,
    Read,
    Stop,
    Close,
    GetModel(String),
    GetEngine,
}

#[derive(Clone, Debug)]
struct Asked {
    pressed: Pressed,
    quality: Quality,
    remember: bool,
}

impl Default for Asked {
    fn default() -> Self {
        Self {
            pressed: Pressed::Nothing,
            quality: Quality::Accurate,
            remember: false,
        }
    }
}

fn which_languages(
    ui: &mut egui::Ui,
    draft: &mut OcrDraft,
    lang: pdf_app::wording::Lang,
    asked: &mut Asked,
) {
    ui.horizontal_wrapped(|ui| {
        for (code, chosen) in &mut draft.languages {
            let name = Message::OcrLanguage(code.clone()).say(lang);
            if draft.here.contains(code) {
                ui.checkbox(chosen, name);
            } else if let Some(model) = pdf_ocr::models::model(code, draft.choice.quality) {
                let label = Message::OcrGetModel {
                    code: code.clone(),
                    bytes: model.bytes,
                }
                .say(lang);
                if ui
                    .small_button(format!("\u{2b07} {label}"))
                    .on_hover_text(model.url())
                    .clicked()
                {
                    asked.pressed = Pressed::GetModel(code.clone());
                }
            }
        }
    });
}

fn which_models(
    ui: &mut egui::Ui,
    draft: &mut OcrDraft,
    lang: pdf_app::wording::Lang,
    asked: &mut Asked,
) {
    asked.quality = draft.choice.quality;
    ui.horizontal(|ui| {
        ui.label(Message::OcrModel.say(lang));
        for quality in Quality::ALL {
            ui.selectable_value(
                &mut asked.quality,
                quality,
                Message::OcrQuality(quality).say(lang),
            );
        }
    });
    let chosen = draft.chosen();
    let about: Vec<String> = KNOWN
        .iter()
        .filter(|code| chosen.is_empty() || chosen.iter().any(|ticked| ticked == *code))
        .filter_map(|code| pdf_ocr::models::model(code, asked.quality))
        .filter_map(|model| {
            model.cer.map(|cer| {
                Message::OcrErrorRate {
                    code: model.code.to_owned(),
                    cer,
                }
                .say(lang)
            })
        })
        .collect();
    if !about.is_empty() {
        ui.label(
            egui::RichText::new(about.join(" \u{b7} "))
                .size(11.0)
                .color(ui.visuals().weak_text_color()),
        );
    }
}

fn which_pages(
    ui: &mut egui::Ui,
    draft: &mut OcrDraft,
    (lang, count): (pdf_app::wording::Lang, usize),
) {
    ui.horizontal_wrapped(|ui| {
        ui.radio_value(
            &mut draft.which,
            OcrWhich::ThisPage,
            Message::OcrThisPage.say(lang),
        );
        ui.radio_value(&mut draft.which, OcrWhich::All, Message::AllPages.say(lang));
        ui.radio_value(
            &mut draft.which,
            OcrWhich::Some,
            Message::SomePages.say(lang),
        );
        let range = ui.add(
            egui::TextEdit::singleline(&mut draft.range)
                .hint_text(format!("1-{count}"))
                .desired_width(90.0),
        );
        if range.changed() {
            draft.which = OcrWhich::Some;
        }
    });
}

fn the_recogniser_itself(
    ui: &mut egui::Ui,
    draft: &OcrDraft,
    lang: pdf_app::wording::Lang,
    asked: &mut Asked,
) {
    if !pdf_ocr::setup::possible() {
        ui.colored_label(
            ui.visuals().error_fg_color,
            Message::OcrEngineElsewhere.say(lang),
        );
        return;
    }
    ui.colored_label(
        ui.visuals().error_fg_color,
        Message::OcrNotInstalled.say(lang),
    );
    ui.add_enabled_ui(draft.fetching.is_none(), |ui| {
        if ui
            .button(format!("\u{2b07} {}", Message::OcrGetEngine.say(lang)))
            .clicked()
        {
            asked.pressed = Pressed::GetEngine;
        }
    });
}

fn how_far(ui: &mut egui::Ui, draft: &OcrDraft, lang: pdf_app::wording::Lang) {
    if let Some(reading) = &draft.reading {
        let total = reading.pages.len();
        let done = reading.read.len();
        #[allow(clippy::cast_precision_loss)]
        let fraction = done as f32 / total.max(1) as f32;
        ui.add(
            egui::ProgressBar::new(fraction).text(Message::OcrProgress { done, total }.say(lang)),
        );
    }
    let Some(fetch) = &draft.fetching else {
        return;
    };
    let said = match &fetch.code {
        Some(code) => Message::OcrGettingModel(code.clone()).say(lang),
        None => Message::OcrGettingEngine.say(lang),
    };
    let seen = fetch.seen.load(Ordering::Relaxed);
    let bar = if fetch.total > 0 {
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        let fraction = (seen as f64 / fetch.total as f64) as f32;
        egui::ProgressBar::new(fraction.clamp(0.0, 1.0))
    } else {
        egui::ProgressBar::new(0.0).animate(true)
    };
    ui.add(bar.text(said));
}

fn the_choices(
    ui: &mut egui::Ui,
    draft: &mut OcrDraft,
    (lang, count): (pdf_app::wording::Lang, usize),
    asked: &mut Asked,
) {
    if draft.engine.is_none() {
        the_recogniser_itself(ui, draft, lang, asked);
        ui.separator();
    }
    let idle = draft.reading.is_none() && draft.fetching.is_none();
    ui.add_enabled_ui(idle, |ui| {
        which_languages(ui, draft, lang, asked);
        which_models(ui, draft, lang, asked);
        ui.separator();
        which_pages(ui, draft, (lang, count));
        if ui
            .checkbox(&mut draft.choice.skip_text, Message::OcrSkipText.say(lang))
            .changed()
        {
            asked.remember = true;
        }
    });
}

fn what_stands_in_the_way(
    ui: &mut egui::Ui,
    draft: &OcrDraft,
    lang: pdf_app::wording::Lang,
    pages: Option<&Result<Vec<usize>, Message>>,
) {
    if let Some(Err(why)) = pages {
        ui.colored_label(ui.visuals().error_fg_color, why.say(lang));
    }
    if draft.engine.is_some() && draft.chosen().is_empty() {
        ui.colored_label(ui.visuals().warn_fg_color, Message::OcrNoLanguage.say(lang));
    }
    if let Some(trouble) = &draft.trouble {
        ui.colored_label(ui.visuals().error_fg_color, trouble.say(lang));
    }
}

fn the_buttons(
    ui: &mut egui::Ui,
    draft: &OcrDraft,
    (lang, pages): (pdf_app::wording::Lang, Option<&Result<Vec<usize>, Message>>),
    asked: &mut Asked,
) {
    ui.horizontal(|ui| {
        if draft.reading.is_some() || draft.fetching.is_some() {
            if ui.button(Message::OcrStop.say(lang)).clicked() {
                asked.pressed = Pressed::Stop;
            }
            return;
        }
        let count = pages
            .and_then(|pages| pages.as_ref().ok())
            .map_or(0, Vec::len);
        let ready = count > 0 && !draft.chosen().is_empty();
        if ui
            .add_enabled(
                ready,
                egui::Button::new(egui::RichText::new(Message::OcrStart(count).say(lang)).strong())
                    .min_size(egui::vec2(160.0, 30.0)),
            )
            .clicked()
        {
            asked.pressed = Pressed::Read;
        }
        if ui.button(Message::Close.say(lang)).clicked() {
            asked.pressed = Pressed::Close;
        }
    });
}
