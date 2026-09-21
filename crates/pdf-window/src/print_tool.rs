use std::sync::atomic::AtomicBool;
use std::sync::{Arc, mpsc};

use eframe::egui;

use pdf_app::wording::{Command, Message, PrintScalingKind};
use pdf_edit::stamp::Only;
use pdf_print::{Order, Orientation, PerSheet, Scaling, Settings, Sheet};

use crate::window_state::{
    JobNews, PrintChoices, PrintDraft, PrintJob, PrintWhich, Printing, Window,
};

const PREVIEW: egui::Vec2 = egui::vec2(430.0, 520.0);

const CHOICES_WIDTH: f32 = 300.0;

pub(crate) fn settings_of(choices: &PrintChoices) -> Settings {
    let papers = pdf_print::papers();
    let paper = papers
        .get(choices.paper)
        .or_else(|| papers.first())
        .map_or_else(|| Settings::default().paper, |(_, paper)| *paper);
    let per_sheet = if choices.per_sheet == 0 {
        PerSheet::Grid {
            columns: choices.columns,
            rows: choices.rows,
        }
    } else {
        PerSheet::Pages(choices.per_sheet)
    };
    Settings {
        paper,
        orientation: choices.orientation,
        scaling: match choices.scaling {
            PrintScalingKind::Fit => Scaling::Fit,
            PrintScalingKind::ShrinkOversized => Scaling::ShrinkOversized,
            PrintScalingKind::ActualSize => Scaling::ActualSize,
            PrintScalingKind::Custom => Scaling::Custom(choices.percent),
        },
        per_sheet,
        order: choices.order,
        auto_rotate: choices.auto_rotate,
        margin: choices.margin * 72.0 / 25.4,
    }
}

pub(crate) fn job_of(choices: &PrintChoices, title: String) -> pdf_print::service::JobSettings {
    pdf_print::service::JobSettings {
        title,
        copies: choices.copies,
        media: pdf_print::media_name(&paper_name(choices))
            .unwrap_or_default()
            .to_owned(),
        colour: choices.printing == Printing::InColour,
        sides: choices.sides,
        hold: false,
    }
}

pub(crate) fn printed_pages(
    choices: &PrintChoices,
    count: usize,
    current: usize,
) -> Result<Vec<usize>, Message> {
    let mut pages = match choices.which {
        PrintWhich::All => pdf_edit::stamp::pages_of("", count, choices.only),
        PrintWhich::Current => {
            pdf_edit::stamp::pages_of(&(current + 1).to_string(), count, Only::Every)
        }
        PrintWhich::Some => pdf_edit::stamp::pages_of(&choices.range, count, choices.only),
    }
    .map_err(|error| Message::refusal(&error))?;
    if choices.reverse {
        pages.reverse();
    }
    Ok(pages)
}

fn paper_name(choices: &PrintChoices) -> String {
    pdf_print::papers()
        .get(choices.paper)
        .map_or_else(String::new, |(name, _)| (*name).to_owned())
}

fn printer_margin(found: &pdf_print::service::Capabilities, choices: &PrintChoices) -> Option<i32> {
    let paper = settings_of(choices).paper;
    #[allow(clippy::cast_possible_truncation)]
    let hundredths =
        [paper.width, paper.height].map(|points| (points * 2540.0 / 72.0).round() as i32);
    found.margin_for(hundredths)
}

fn duplex_offered(found: Option<&pdf_print::service::Capabilities>) -> bool {
    found.is_none_or(|found| found.two_sided)
}

fn resolved_sides(chosen: pdf_print::service::Sides, offered: bool) -> pdf_print::service::Sides {
    if offered {
        chosen
    } else {
        pdf_print::service::Sides::One
    }
}

fn key_of(sheet: &Sheet, dpi: f64, (epoch, borders): (u64, bool)) -> String {
    format!("{sheet:?} {dpi:.2} {epoch} {borders}")
}

impl Window {
    pub(crate) fn open_the_print_dialog(&mut self) {
        if !self.has_document() || self.print_draft.is_some() {
            return;
        }
        let allowance = self
            .editor
            .source()
            .and_then(|source| pdf_print::allowance(source, self.editor.credential()).ok());
        let printers = pdf_print::service::printers().map_err(|error| error.to_string());
        if let Ok((list, default)) = &printers {
            let named = |name: &String| list.iter().any(|printer| &printer.name == name);
            if !self.print_choices.printer.as_ref().is_some_and(named) {
                self.print_choices.printer = default
                    .clone()
                    .filter(named)
                    .or_else(|| list.first().map(|printer| printer.name.clone()));
            }
        }
        self.print_draft = Some(PrintDraft {
            sheet: 0,
            allowance,
            shown: None,
            drawing: None,
            printers,
            capabilities: None,
            job: None,
            failed: None,
        });
    }

    fn print_sheets(&self) -> Result<Vec<Sheet>, Message> {
        let choices = &self.print_choices;
        let pages = printed_pages(choices, self.editor.page_count(), self.focus)?;
        let sized: Vec<(usize, [f64; 2])> = pages
            .iter()
            .filter_map(|page| {
                self.editor.geometry(*page).map(|geometry| {
                    let (width, height) = geometry.rotated_size();
                    (*page, [width, height])
                })
            })
            .collect();
        pdf_print::lay_out(&sized, &settings_of(choices)).map_err(Message::PrintLayout)
    }

    fn know_the_printer(&mut self) {
        let Some(draft) = self.print_draft.as_mut() else {
            return;
        };
        let Some(name) = self.print_choices.printer.clone() else {
            draft.capabilities = None;
            return;
        };
        if draft
            .capabilities
            .as_ref()
            .is_some_and(|(asked, _)| *asked == name)
        {
            return;
        }
        let found = pdf_print::service::capabilities(&name).map_err(|error| error.to_string());
        if let Ok(found) = &found
            && !found.colour
        {
            self.print_choices.printing = Printing::BlackAndWhite;
        }
        draft.capabilities = Some((name, found));
    }

    fn print_problems(&self) -> (Vec<Message>, Vec<Message>) {
        let mut blocking = Vec::new();
        let mut warnings = Vec::new();
        let Some(draft) = self.print_draft.as_ref() else {
            return (blocking, warnings);
        };
        match draft.allowance {
            Some(pdf_content::PrintAllowance::Refused) => blocking.push(Message::PrintRefused),
            Some(pdf_content::PrintAllowance::Degraded) => warnings.push(Message::PrintDegraded),
            _ => {}
        }
        match &draft.printers {
            Err(why) => blocking.push(Message::PrintNoService(why.clone())),
            Ok((list, _)) if list.is_empty() => blocking.push(Message::PrintNoPrinter),
            Ok(_) => {}
        }
        let printer = self.printer_info();
        if let Some((_, Ok(found))) = &draft.capabilities {
            let paper = paper_name(&self.print_choices);
            let media = pdf_print::media_name(&paper);
            if !found.media.is_empty()
                && media.is_none_or(|media| !found.media.iter().any(|has| has == media))
            {
                blocking.push(Message::PrintPaperMissing {
                    printer: printer.clone(),
                    paper,
                });
            }
            if let Some(edge) = printer_margin(found, &self.print_choices)
                && self.print_choices.margin * 100.0 < f64::from(edge)
            {
                warnings.push(Message::PrintEdgeTooNarrow {
                    printer,
                    millimetres: format!("{:.1}", f64::from(edge) / 100.0),
                });
            }
        } else if let Some((_, Err(why))) = &draft.capabilities {
            blocking.push(Message::PrintNoService(why.clone()));
        }
        (blocking, warnings)
    }

    fn printer_info(&self) -> String {
        let name = self.print_choices.printer.clone().unwrap_or_default();
        self.print_draft
            .as_ref()
            .and_then(|draft| draft.printers.as_ref().ok())
            .and_then(|(list, _)| list.iter().find(|printer| printer.name == name))
            .map_or(name, |printer| printer.info.clone())
    }

    fn margin_of_printer(&self) -> Option<i32> {
        let (_, found) = self.print_draft.as_ref()?.capabilities.as_ref()?;
        printer_margin(found.as_ref().ok()?, &self.print_choices)
    }

    pub(crate) fn print_dialog(&mut self, ctx: &egui::Context) {
        if self.print_draft.is_none() || !self.has_document() {
            return;
        }
        self.keep_printing();
        if self.print_draft.is_none() {
            return;
        }
        self.know_the_printer();
        let lang = self.lang;
        let sheets = self.print_sheets();
        let count = sheets.as_ref().map_or(0, Vec::len);
        if let Some(draft) = self.print_draft.as_mut() {
            draft.sheet = draft.sheet.min(count.saturating_sub(1));
        }
        self.see_the_sheet(ctx, sheets.as_ref().ok());
        let pages = sheets.as_ref().map_or(0, |sheets| {
            sheets.iter().map(|sheet| sheet.placements.len()).sum()
        });
        let paper = paper_name(&self.print_choices);
        let page_count = self.editor.page_count();
        let margin_edge = self.margin_of_printer();
        let (blocking, warnings) = self.print_problems();
        let Some(draft) = self.print_draft.as_mut() else {
            return;
        };
        let choices = &mut self.print_choices;
        let (mut close, mut print, mut stop) = (false, false, false);
        let printing = draft.job.is_some();
        let title = Message::Command(Command::Print).say(lang);
        let modal = egui::Modal::new(egui::Id::new("print-dialog")).show(ctx, |ui| {
            ui.heading(title.trim_end_matches('\u{2026}'));
            ui.label(
                egui::RichText::new(Message::PrintWhy.say(lang))
                    .size(11.0)
                    .color(ui.visuals().weak_text_color()),
            );
            ui.separator();
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(CHOICES_WIDTH);
                    ui.add_enabled_ui(!printing, |ui| {
                        which_printer(ui, choices, draft, lang);
                        ui.separator();
                        which_pages(ui, choices, (lang, page_count));
                        ui.separator();
                        what_paper(ui, choices, (margin_edge, lang));
                        ui.separator();
                        how_large(ui, choices, lang);
                    });
                });
                ui.add_space(16.0);
                ui.vertical(|ui| preview(ui, draft, (&sheets, count), lang));
            });
            let asked = what_is_asked(
                ui,
                (draft, &sheets),
                (&blocking, &warnings),
                (
                    Message::PrintSummary {
                        pages,
                        sheets: count,
                        paper: paper.clone(),
                    },
                    lang,
                ),
            );
            (print, close, stop) = asked;
        });
        if stop && let Some(job) = draft.job.as_ref() {
            job.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if (close || modal.should_close()) && !printing {
            self.print_draft = None;
            return;
        }
        if print && let Ok(sheets) = sheets {
            self.start_printing(ctx, sheets);
        }
    }

    fn start_printing(&mut self, ctx: &egui::Context, sheets: Vec<Sheet>) {
        let (Some(source), Some(printer)) = (
            self.editor.source().cloned(),
            self.print_choices.printer.clone(),
        ) else {
            return;
        };
        let info = self.printer_info();
        let Some(draft) = self.print_draft.as_mut() else {
            return;
        };
        let resolution = draft
            .capabilities
            .as_ref()
            .and_then(|(_, found)| found.as_ref().ok())
            .and_then(|found| found.resolution);
        let dpi = pdf_print::job_dpi(
            resolution,
            draft
                .allowance
                .unwrap_or(pdf_content::PrintAllowance::Faithful),
        );
        let choices = &self.print_choices;
        let title = self.opened.file_name().map_or_else(
            || "PanPDF".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        let settings = job_of(choices, title);
        let borders = choices.borders;
        let credential = self.editor.credential().to_vec();
        let fonts = self.editor.fonts();
        let stop = Arc::new(AtomicBool::new(false));
        let (send, news) = mpsc::channel();
        let total = sheets.len();
        let (worker_stop, repaint) = (Arc::clone(&stop), ctx.clone());
        std::thread::spawn(move || {
            let told = send.clone();
            let tell = repaint.clone();
            let handed = pdf_print::job::print_sheets(
                &sheets,
                (dpi, borders),
                |page, scale, region| {
                    pdf_print::draw_printed(
                        &source,
                        (&credential, fonts.clone()),
                        page,
                        scale,
                        region,
                    )
                },
                (&printer, &settings),
                (
                    move |done| {
                        let _ = told.send(JobNews::Drawn(done));
                        tell.request_repaint();
                    },
                    &worker_stop,
                ),
            );
            let _ = send.send(JobNews::Finished(handed.map_err(|error| match error {
                pdf_print::job::JobError::Stopped => String::new(),
                other => other.to_string(),
            })));
            repaint.request_repaint();
        });
        draft.failed = None;
        draft.job = Some(PrintJob {
            stop,
            news,
            done: 0,
            total,
            printer: info,
        });
    }

    fn keep_printing(&mut self) {
        let Some(draft) = self.print_draft.as_mut() else {
            return;
        };
        let Some(job) = draft.job.as_mut() else {
            return;
        };
        let mut finished = None;
        while let Ok(news) = job.news.try_recv() {
            match news {
                JobNews::Drawn(done) => job.done = done,
                JobNews::Finished(result) => finished = Some(result),
            }
        }
        let Some(result) = finished else {
            return;
        };
        let (sheets, printer) = (job.total, job.printer.clone());
        draft.job = None;
        match result {
            Ok(number) => {
                self.print_draft = None;
                self.editor.say(Message::PrintSent {
                    sheets,
                    printer,
                    job: number,
                });
            }
            Err(why) if why.is_empty() => {}
            Err(why) => draft.failed = Some(why),
        }
    }

    fn see_the_sheet(&mut self, ctx: &egui::Context, sheets: Option<&Vec<Sheet>>) {
        let Some(draft) = self.print_draft.as_mut() else {
            return;
        };
        if let Some((key, drawing)) = draft.drawing.as_ref()
            && let Ok(drawn) = drawing.try_recv()
        {
            let shown = drawn.map(|image| {
                let size = [image.width as usize, image.height as usize];
                ctx.load_texture(
                    "print-preview",
                    egui::ColorImage::from_rgb(size, &image.rgb),
                    egui::TextureOptions::LINEAR,
                )
            });
            draft.shown = Some((key.clone(), shown));
            draft.drawing = None;
        }
        let Some(sheet) = sheets.and_then(|sheets| sheets.get(draft.sheet)) else {
            return;
        };
        let room = PREVIEW - egui::vec2(8.0, 40.0);
        let fit = f64::from(room.x) / sheet.size[0];
        let fit = fit.min(f64::from(room.y) / sheet.size[1]);
        let dpi = 72.0 * fit * f64::from(ctx.pixels_per_point());
        let key = key_of(
            sheet,
            dpi,
            (self.editor.epoch(), self.print_choices.borders),
        );
        let asked = draft.drawing.as_ref().map(|(held, _)| held);
        let shown = draft.shown.as_ref().map(|(held, _)| held);
        if asked == Some(&key) || (asked.is_none() && shown == Some(&key)) {
            return;
        }
        let Some(source) = self.editor.source().cloned() else {
            return;
        };
        let credential = self.editor.credential().to_vec();
        let fonts = self.editor.fonts();
        let borders = self.print_choices.borders;
        let sheet = sheet.clone();
        let (send, drawn) = mpsc::channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let image = pdf_print::draw_sheet(&sheet, dpi, borders, |page, scale, region| {
                pdf_print::draw_printed(&source, (&credential, fonts.clone()), page, scale, region)
            })
            .map_err(|error| error.to_string());
            let _ = send.send(image);
            repaint.request_repaint();
        });
        draft.drawing = Some((key, drawn));
    }
}

fn what_is_asked(
    ui: &mut egui::Ui,
    (draft, sheets): (&PrintDraft, &Result<Vec<Sheet>, Message>),
    (blocking, warnings): (&[Message], &[Message]),
    (summary, lang): (Message, pdf_app::wording::Lang),
) -> (bool, bool, bool) {
    let (mut print, mut close, mut stop) = (false, false, false);
    ui.separator();
    if sheets.is_ok() {
        ui.label(summary.say(lang));
    }
    for why in blocking {
        ui.colored_label(ui.visuals().error_fg_color, why.say(lang));
    }
    for why in warnings {
        ui.colored_label(ui.visuals().warn_fg_color, why.say(lang));
    }
    if let Some(why) = &draft.failed {
        ui.colored_label(
            ui.visuals().error_fg_color,
            Message::PrintFailed(why.clone()).say(lang),
        );
    }
    if let Some(job) = &draft.job {
        #[allow(clippy::cast_precision_loss)]
        let part = job.done as f32 / job.total.max(1) as f32;
        ui.add(
            egui::ProgressBar::new(part).text(
                Message::PrintPreparing {
                    done: job.done,
                    total: job.total,
                }
                .say(lang),
            ),
        );
    }
    ui.horizontal(|ui| {
        let printing = draft.job.is_some();
        let ready = sheets.is_ok() && blocking.is_empty() && !printing;
        print = ui
            .add_enabled(ready, egui::Button::new(Message::PrintButton.say(lang)))
            .clicked();
        if printing {
            stop = ui.button(Message::OcrStop.say(lang)).clicked();
        } else {
            close = ui.button(Message::Close.say(lang)).clicked();
        }
    });
    (print, close, stop)
}

fn which_printer(
    ui: &mut egui::Ui,
    choices: &mut PrintChoices,
    draft: &PrintDraft,
    lang: pdf_app::wording::Lang,
) {
    let found = draft
        .capabilities
        .as_ref()
        .and_then(|(_, found)| found.as_ref().ok());
    let colour_offered = found.is_none_or(|found| found.colour);
    let sides_offered = duplex_offered(found);
    choices.sides = resolved_sides(choices.sides, sides_offered);
    egui::Grid::new("print-printer")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label(Message::PrintPrinter.say(lang));
            let list = draft
                .printers
                .as_ref()
                .map(|(list, _)| list.clone())
                .unwrap_or_default();
            let shown = choices
                .printer
                .as_ref()
                .and_then(|name| list.iter().find(|printer| &printer.name == name))
                .map_or_else(String::new, |printer| printer.info.clone());
            egui::ComboBox::from_id_salt("print-printer")
                .selected_text(shown)
                .width(190.0)
                .show_ui(ui, |ui| {
                    for printer in &list {
                        ui.selectable_value(
                            &mut choices.printer,
                            Some(printer.name.clone()),
                            printer.info.clone(),
                        );
                    }
                });
            ui.end_row();
            ui.label(Message::PrintSides.say(lang));
            ui.add_enabled_ui(sides_offered, |ui| {
                egui::ComboBox::from_id_salt("print-sides")
                    .selected_text(Message::PrintSidesIs(choices.sides).say(lang))
                    .width(190.0)
                    .show_ui(ui, |ui| {
                        for sides in [
                            pdf_print::service::Sides::One,
                            pdf_print::service::Sides::TwoLongEdge,
                            pdf_print::service::Sides::TwoShortEdge,
                        ] {
                            ui.selectable_value(
                                &mut choices.sides,
                                sides,
                                Message::PrintSidesIs(sides).say(lang),
                            );
                        }
                    });
            });
            ui.end_row();
            ui.label(Message::PrintCopies.say(lang));
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut choices.copies).range(1..=99));
                ui.add_enabled_ui(colour_offered, |ui| {
                    ui.radio_value(
                        &mut choices.printing,
                        Printing::InColour,
                        Message::PrintColour.say(lang),
                    );
                });
                ui.radio_value(
                    &mut choices.printing,
                    Printing::BlackAndWhite,
                    Message::PrintMonochrome.say(lang),
                );
            });
            ui.end_row();
        });
}

fn which_pages(
    ui: &mut egui::Ui,
    choices: &mut PrintChoices,
    (lang, count): (pdf_app::wording::Lang, usize),
) {
    ui.strong(Message::StampPages.say(lang));
    ui.radio_value(
        &mut choices.which,
        PrintWhich::All,
        Message::AllPages.say(lang),
    );
    ui.radio_value(
        &mut choices.which,
        PrintWhich::Current,
        Message::PrintCurrentPage.say(lang),
    );
    ui.horizontal(|ui| {
        ui.radio_value(
            &mut choices.which,
            PrintWhich::Some,
            Message::SomePages.say(lang),
        );
        let range = ui.add(
            egui::TextEdit::singleline(&mut choices.range)
                .hint_text(format!("1-{count}"))
                .desired_width(150.0),
        );
        if range.changed() {
            choices.which = PrintWhich::Some;
        }
    });
    ui.horizontal(|ui| {
        ui.add_enabled_ui(choices.which != PrintWhich::Current, |ui| {
            egui::ComboBox::from_id_salt("print-only")
                .selected_text(Message::StampOnly(choices.only).say(lang))
                .show_ui(ui, |ui| {
                    for only in [Only::Every, Only::Odd, Only::Even] {
                        ui.selectable_value(
                            &mut choices.only,
                            only,
                            Message::StampOnly(only).say(lang),
                        );
                    }
                });
        });
        ui.checkbox(&mut choices.reverse, Message::PrintReverse.say(lang));
    });
}

fn what_paper(
    ui: &mut egui::Ui,
    choices: &mut PrintChoices,
    (margin_edge, lang): (Option<i32>, pdf_app::wording::Lang),
) {
    let papers = pdf_print::papers();
    egui::Grid::new("print-paper")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label(Message::PrintPaper.say(lang));
            let named = |at: usize| {
                papers.get(at).map_or_else(String::new, |(name, paper)| {
                    format!(
                        "{name}  {:.0} \u{d7} {:.0} mm",
                        paper.width * 25.4 / 72.0,
                        paper.height * 25.4 / 72.0
                    )
                })
            };
            egui::ComboBox::from_id_salt("print-paper")
                .selected_text(named(choices.paper))
                .width(190.0)
                .show_ui(ui, |ui| {
                    for at in 0..papers.len() {
                        ui.selectable_value(&mut choices.paper, at, named(at));
                    }
                });
            ui.end_row();
            ui.label(Message::PrintOrientation.say(lang));
            egui::ComboBox::from_id_salt("print-orientation")
                .selected_text(Message::PrintOrientationIs(choices.orientation).say(lang))
                .width(190.0)
                .show_ui(ui, |ui| {
                    for orientation in [
                        Orientation::Auto,
                        Orientation::Portrait,
                        Orientation::Landscape,
                    ] {
                        ui.selectable_value(
                            &mut choices.orientation,
                            orientation,
                            Message::PrintOrientationIs(orientation).say(lang),
                        );
                    }
                });
            ui.end_row();
            ui.label(Message::PrintMargin.say(lang));
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut choices.margin)
                        .range(0.0..=30.0)
                        .speed(0.2)
                        .max_decimals(1)
                        .suffix(" mm"),
                );
                if let Some(edge) = margin_edge {
                    let millimetres = format!("{:.1}", f64::from(edge) / 100.0);
                    if ui
                        .button(Message::PrintMarginOfPrinter { millimetres }.say(lang))
                        .clicked()
                    {
                        choices.margin = f64::from(edge) / 100.0;
                    }
                }
            });
            ui.end_row();
        });
}

fn how_large(ui: &mut egui::Ui, choices: &mut PrintChoices, lang: pdf_app::wording::Lang) {
    egui::Grid::new("print-size")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label(Message::PrintPerSheet.say(lang));
            ui.horizontal(|ui| {
                let named = |count: u8| {
                    if count == 0 {
                        Message::PrintCustomGrid.say(lang)
                    } else {
                        count.to_string()
                    }
                };
                egui::ComboBox::from_id_salt("print-per-sheet")
                    .selected_text(named(choices.per_sheet))
                    .width(70.0)
                    .show_ui(ui, |ui| {
                        for count in PerSheet::COUNTS.into_iter().chain([0]) {
                            ui.selectable_value(&mut choices.per_sheet, count, named(count));
                        }
                    });
                if choices.per_sheet == 0 {
                    ui.add(egui::DragValue::new(&mut choices.columns).range(1..=10));
                    ui.label("\u{d7}");
                    ui.add(egui::DragValue::new(&mut choices.rows).range(1..=10));
                }
            });
            ui.end_row();
        });
    let several = choices.per_sheet != 1;
    ui.add_enabled_ui(!several, |ui| {
        ui.label(Message::Size.say(lang));
        for kind in [
            PrintScalingKind::Fit,
            PrintScalingKind::ShrinkOversized,
            PrintScalingKind::ActualSize,
        ] {
            ui.radio_value(
                &mut choices.scaling,
                kind,
                Message::PrintScaling(kind).say(lang),
            );
        }
        ui.horizontal(|ui| {
            ui.radio_value(
                &mut choices.scaling,
                PrintScalingKind::Custom,
                Message::PrintScaling(PrintScalingKind::Custom).say(lang),
            );
            let scale = ui.add(
                egui::DragValue::new(&mut choices.percent)
                    .range(10.0..=400.0)
                    .speed(1.0)
                    .max_decimals(0)
                    .suffix(" %"),
            );
            if scale.changed() {
                choices.scaling = PrintScalingKind::Custom;
            }
        });
    });
    ui.add_enabled_ui(several, |ui| {
        ui.horizontal(|ui| {
            ui.label(Message::PrintOrder.say(lang));
            egui::ComboBox::from_id_salt("print-order")
                .selected_text(Message::PrintOrderIs(choices.order).say(lang))
                .width(190.0)
                .show_ui(ui, |ui| {
                    for order in [
                        Order::Horizontal,
                        Order::HorizontalReversed,
                        Order::Vertical,
                        Order::VerticalReversed,
                    ] {
                        ui.selectable_value(
                            &mut choices.order,
                            order,
                            Message::PrintOrderIs(order).say(lang),
                        );
                    }
                });
        });
        ui.checkbox(&mut choices.borders, Message::PrintBorders.say(lang));
    });
    ui.checkbox(&mut choices.auto_rotate, Message::PrintAutoRotate.say(lang));
}

fn preview(
    ui: &mut egui::Ui,
    draft: &mut PrintDraft,
    (sheets, count): (&Result<Vec<Sheet>, Message>, usize),
    lang: pdf_app::wording::Lang,
) {
    ui.set_min_size(PREVIEW);
    let sheets = match sheets {
        Ok(sheets) => sheets,
        Err(why) => {
            ui.colored_label(ui.visuals().error_fg_color, why.say(lang));
            return;
        }
    };
    ui.horizontal(|ui| {
        if ui
            .add_enabled(draft.sheet > 0, egui::Button::new("\u{25c0}"))
            .clicked()
        {
            draft.sheet -= 1;
        }
        ui.label(
            Message::PrintSheet {
                at: draft.sheet + 1,
                of: count,
            }
            .say(lang),
        );
        if ui
            .add_enabled(draft.sheet + 1 < count, egui::Button::new("\u{25b6}"))
            .clicked()
        {
            draft.sheet += 1;
        }
    });
    let Some(sheet) = sheets.get(draft.sheet) else {
        return;
    };
    let room = PREVIEW - egui::vec2(8.0, 40.0);
    #[allow(clippy::cast_possible_truncation)]
    let fit = (f64::from(room.x) / sheet.size[0]).min(f64::from(room.y) / sheet.size[1]) as f32;
    #[allow(clippy::cast_possible_truncation)]
    let size = egui::vec2(sheet.size[0] as f32 * fit, sheet.size[1] as f32 * fit);
    let (area, _) = ui.allocate_exact_size(room, egui::Sense::hover());
    let paper = egui::Rect::from_center_size(area.center(), size);
    let painter = ui.painter_at(area);
    painter.rect_filled(
        paper.translate(egui::vec2(3.0, 3.0)),
        0.0,
        egui::Color32::from_black_alpha(60),
    );
    painter.rect_filled(paper, 0.0, egui::Color32::WHITE);
    match &draft.shown {
        Some((_, Ok(texture))) => {
            painter.image(
                texture.id(),
                paper,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        Some((_, Err(why))) => {
            painter.text(
                paper.center(),
                egui::Align2::CENTER_CENTER,
                Message::PrintSheetFailed(why.clone()).say(lang),
                egui::FontId::proportional(12.0),
                ui.visuals().error_fg_color,
            );
        }
        None => {}
    }
    if draft.drawing.is_some() {
        painter.text(
            paper.left_top() + egui::vec2(6.0, 6.0),
            egui::Align2::LEFT_TOP,
            Message::PrintDrawing.say(lang),
            egui::FontId::proportional(11.0),
            egui::Color32::GRAY,
        );
    }
}

#[cfg(test)]
mod tests {
    use pdf_app::wording::{Message, PrintScalingKind, Refusal, StampWhy};
    use pdf_edit::stamp::Only;
    use pdf_print::{PerSheet, Scaling};

    use pdf_print::service::{Capabilities, Sides};

    use super::{
        duplex_offered, job_of, printed_pages, printer_margin, resolved_sides, settings_of,
    };
    use crate::window_state::{PrintChoices, PrintWhich, Printing};

    #[test]
    fn the_pages_printed_are_the_pages_chosen() {
        let mut choices = PrintChoices::default();
        assert_eq!(printed_pages(&choices, 5, 2), Ok(vec![0, 1, 2, 3, 4]));
        choices.only = Only::Even;
        assert_eq!(printed_pages(&choices, 5, 2), Ok(vec![1, 3]));
        choices.which = PrintWhich::Current;
        assert_eq!(
            printed_pages(&choices, 5, 2),
            Ok(vec![2]),
            "odd or even is for a range"
        );
        choices.which = PrintWhich::Some;
        choices.only = Only::Every;
        choices.range = "2-4".to_owned();
        choices.reverse = true;
        assert_eq!(printed_pages(&choices, 5, 0), Ok(vec![3, 2, 1]));
        choices.only = Only::Odd;
        choices.range = "2".to_owned();
        assert_eq!(
            printed_pages(&choices, 5, 0),
            Err(Message::Refused(Refusal::Stamp(
                StampWhy::RangeNamesNothing
            ))),
            "page 2 is not odd"
        );
        choices.range = "9".to_owned();
        assert_eq!(
            printed_pages(&choices, 5, 0),
            Err(Message::Refused(Refusal::Stamp(StampWhy::RangePastTheEnd)))
        );
    }

    #[test]
    fn the_choices_become_what_the_printer_is_asked_for() {
        let mut choices = PrintChoices {
            copies: 3,
            ..PrintChoices::default()
        };
        let job = job_of(&choices, "book.pdf".to_owned());
        assert_eq!(job.title, "book.pdf");
        assert_eq!(job.copies, 3);
        assert_eq!(job.media, "iso_a4_210x297mm");
        assert!(job.colour);
        assert!(!job.hold, "Print means print");
        choices.printing = Printing::BlackAndWhite;
        choices.paper = 4;
        let job = job_of(&choices, String::new());
        assert!(!job.colour);
        assert_eq!(job.media, "jis_b5_182x257mm");
    }

    #[test]
    fn the_choices_become_the_layouts_settings() {
        let mut choices = PrintChoices::default();
        let settings = settings_of(&choices);
        assert!((settings.margin - 5.0 * 72.0 / 25.4).abs() < 1e-9);
        assert!(
            (settings.paper.width - 210.0 * 72.0 / 25.4).abs() < 1e-9,
            "A4 first"
        );
        assert_eq!(settings.per_sheet, PerSheet::Pages(1));
        assert_eq!(settings.scaling, Scaling::Fit);
        choices.per_sheet = 0;
        choices.columns = 3;
        choices.rows = 2;
        choices.scaling = PrintScalingKind::Custom;
        choices.percent = 80.0;
        choices.paper = 5;
        let settings = settings_of(&choices);
        assert_eq!(
            settings.per_sheet,
            PerSheet::Grid {
                columns: 3,
                rows: 2
            }
        );
        assert_eq!(settings.scaling, Scaling::Custom(80.0));
        assert!((settings.paper.width - 612.0).abs() < 1e-9, "Letter");
    }

    #[test]
    fn the_job_asks_for_the_sides_chosen() {
        let choices = PrintChoices {
            sides: Sides::TwoShortEdge,
            ..PrintChoices::default()
        };
        let job = job_of(&choices, String::new());
        assert_eq!(job.sides, Sides::TwoShortEdge);
    }

    #[test]
    fn the_printers_margin_is_the_edge_it_states() {
        let choices = PrintChoices::default();
        let found = Capabilities {
            margins: vec![([21000, 29700], [0, 0, 0, 300])],
            ..Capabilities::default()
        };
        let edge = printer_margin(&found, &choices).expect("A4 is in the margins");
        assert_eq!(edge, 300);
        assert_eq!(format!("{:.1}", f64::from(edge) / 100.0), "3.0");
        let other = PrintChoices {
            paper: 4,
            ..PrintChoices::default()
        };
        assert_eq!(printer_margin(&found, &other), None);
    }

    #[test]
    fn a_one_sided_printer_keeps_the_job_one_sided() {
        let one_sided = Capabilities {
            two_sided: false,
            ..Capabilities::default()
        };
        assert!(!duplex_offered(Some(&one_sided)));
        assert_eq!(
            resolved_sides(Sides::TwoLongEdge, duplex_offered(Some(&one_sided))),
            Sides::One
        );
        let two_sided = Capabilities {
            two_sided: true,
            ..Capabilities::default()
        };
        assert!(duplex_offered(Some(&two_sided)));
        assert_eq!(
            resolved_sides(Sides::TwoLongEdge, duplex_offered(Some(&two_sided))),
            Sides::TwoLongEdge
        );
        assert!(duplex_offered(None));
    }
}
