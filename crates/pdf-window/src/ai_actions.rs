use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant};

use eframe::egui;
use pdf_agent::connect::{Picture, ToolCall, ToolResult};
use pdf_agent::desk::{self, Block, MOST_CHARACTERS};
use pdf_agent::tools::{self, request::Request};
use pdf_app::Applied;
use pdf_app::ai_permission::{Decision, Mode, Why, decide, refusal_text};
use pdf_app::wording::{Lang, Message};

use crate::window_state::Window;

const WAIT: Duration = Duration::from_secs(30);

pub(crate) const MOST_ROUNDS: usize = 16;

const MOST_SEARCHED: usize = 120;

const THUMBNAIL: f64 = 240.0;

pub(crate) struct Pending {
    pub(crate) call: ToolCall,
    pub(crate) request: Request,
    pub(crate) may_allow_for_chat: bool,
}

struct Sent {
    call: ToolCall,
    request: Request,
    was: Option<(usize, [f64; 4])>,
}

#[derive(Clone)]
struct Named {
    epoch: u64,
    arranged: u64,
    text: String,
    area: [f64; 4],
}

struct Gathering {
    call: String,
    span: (usize, usize),
    next: usize,
    pages: Vec<(usize, Vec<Block>)>,
    characters: usize,
}

#[derive(Default)]
pub(crate) struct Tools {
    pub(crate) queue: VecDeque<ToolCall>,
    pub(crate) results: Vec<ToolResult>,
    pub(crate) ask: Option<Pending>,
    pub(crate) allowed_for_chat: BTreeSet<String>,
    allowed_now: Option<String>,
    waiting: Option<(String, Instant)>,
    sent: Option<Sent>,
    landed: Option<Applied>,
    gathering: Option<Gathering>,
    pub(crate) rounds: usize,
    named: BTreeMap<(usize, usize), Named>,
    arranged: u64,
    pub(crate) called: BTreeMap<String, String>,
    pub(crate) pictures: BTreeMap<String, egui::ColorImage>,
}

impl Tools {
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn drop_the_queue(&mut self) {
        self.queue.clear();
        self.results.clear();
        self.ask = None;
        self.waiting = None;
        self.sent = None;
        self.landed = None;
        self.gathering = None;
        self.allowed_now = None;
        self.rounds = 0;
    }

    pub(crate) fn busy(&self) -> bool {
        !self.queue.is_empty() || self.ask.is_some() || self.sent.is_some()
    }

    pub(crate) fn take(&mut self, calls: &[ToolCall]) {
        for call in calls {
            self.called.insert(call.id.clone(), call.name.clone());
            self.queue.push_back(call.clone());
        }
    }

    pub(crate) fn note_applied(&mut self, applied: &Applied) {
        if self.sent.is_some() {
            self.landed = Some(applied.clone());
        }
    }

    pub(crate) fn pages_moved(&mut self) {
        self.arranged += 1;
    }

    fn answer(&mut self, result: ToolResult) {
        self.queue.pop_front();
        self.waiting = None;
        self.gathering = None;
        self.allowed_now = None;
        self.results.push(result);
    }

    fn allowed_just_now(&self, call: &ToolCall) -> bool {
        self.allowed_now.as_deref() == Some(call.id.as_str())
    }

    pub(crate) fn answer_the_card(&mut self, answer: pdf_app::ai_permission::Answer) {
        use pdf_app::ai_permission::Answer;
        let Some(pending) = self.ask.take() else {
            return;
        };
        match answer {
            Answer::Refuse => {
                self.answer(ToolResult::failed(&pending.call.id, refusal_text()));
            }
            Answer::ForThisChat => {
                self.allowed_for_chat.insert(pending.call.name.clone());
                self.allowed_now = Some(pending.call.id.clone());
            }
            Answer::Once => self.allowed_now = Some(pending.call.id.clone()),
        }
    }
}

enum Performed {
    Done(ToolResult),
    NeedPages(Vec<usize>),
    Sent,
    Busy,
}

impl Window {
    pub(crate) fn document_brief(&self) -> tools::DocumentBrief {
        tools::DocumentBrief {
            file_name: self
                .opened
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            title: self
                .editor
                .facts()
                .map(|facts| facts.info.title)
                .unwrap_or_default(),
            pages: self.editor.page_count(),
        }
    }

    pub(crate) fn advance_tools(&mut self, ctx: &egui::Context) {
        self.collect_a_sent_edit();
        if self.ai.tools.ask.is_some() || self.ai.busy() {
            return;
        }
        while let Some(call) = self.ai.tools.queue.front().cloned() {
            if self.ai.tools.sent.is_some() {
                return;
            }
            let decision = if self.ai.tools.allowed_just_now(&call) {
                Decision::Run
            } else {
                self.decide_about(&call)
            };
            match decision {
                Decision::Refuse(why) => {
                    let said = match why {
                        Why::ToolsAreOff => {
                            "This chat is in Chat only, so no tool may be used. The person can \
                             change that beside the model's name. Answer with what you know."
                        }
                        Why::UnknownTool => "there is no tool of that name",
                    };
                    self.ai.tools.answer(ToolResult::failed(&call.id, said));
                    continue;
                }
                Decision::Ask { may_allow_for_chat } => {
                    match pdf_agent::tools::request::parse(&call.name, &call.arguments) {
                        Ok(request) => {
                            self.ai.tools.ask = Some(Pending {
                                call,
                                request,
                                may_allow_for_chat,
                            });
                            ctx.request_repaint();
                            return;
                        }
                        Err(why) => {
                            self.ai.tools.answer(ToolResult::failed(&call.id, why));
                            continue;
                        }
                    }
                }
                Decision::Run => {}
            }
            match self.perform(&call) {
                Performed::Done(result) => self.ai.tools.answer(result),
                Performed::Sent => return,
                Performed::Busy => {
                    ctx.request_repaint();
                    return;
                }
                Performed::NeedPages(pages) => {
                    let since = match self.ai.tools.waiting.take() {
                        Some((id, since)) if id == call.id => since,
                        _ => Instant::now(),
                    };
                    if since.elapsed() > WAIT {
                        let page = pages.first().map_or(1, |page| page + 1);
                        let said = self.failed.values().next().map_or_else(
                            || format!("page {page} could not be read in time"),
                            |why| format!("page {page} cannot be read: {why}"),
                        );
                        self.ai.tools.answer(ToolResult::failed(&call.id, said));
                        continue;
                    }
                    for page in pages {
                        self.ask_the_painter_for(page);
                    }
                    self.ai.tools.waiting = Some((call.id.clone(), since));
                    ctx.request_repaint();
                    return;
                }
            }
        }
        self.finish_the_round(ctx);
    }

    fn decide_about(&self, call: &ToolCall) -> Decision {
        decide(
            self.ai.mode,
            &call.name,
            tools::facts(&call.name).map(|facts| (facts.read_only, facts.destructive)),
            self.ai.tools.allowed_for_chat.contains(&call.name),
        )
    }

    fn finish_the_round(&mut self, ctx: &egui::Context) {
        if self.ai.tools.results.is_empty() {
            return;
        }
        let results = std::mem::take(&mut self.ai.tools.results);
        if self.ai.round_done(results) {
            let brief = self.document_brief();
            self.ai.start_round(ctx, &brief);
        } else {
            self.ai.stop_for_too_many_rounds();
        }
    }

    fn collect_a_sent_edit(&mut self) {
        let Some(applied) = self.ai.tools.landed.take() else {
            return;
        };
        let Some(sent) = self.ai.tools.sent.take() else {
            return;
        };
        let now = match (&applied, &sent.request) {
            (Applied::Changed { .. }, Request::ReplaceText { .. }) => sent
                .was
                .and_then(|(page, area)| self.block_over(page, area)),
            _ => None,
        };
        if let Some(block) = &now {
            self.remember_the_name(block);
        }
        if let Applied::Changed { page, .. } = &applied {
            self.ai.say_the_document_changed(*page + 1);
        }
        let result = result_of(&sent.call.id, (&applied, &sent.request), now.as_ref());
        self.ai.tools.answer(result);
    }

    fn block_over(&self, page: usize, area: [f64; 4]) -> Option<Block> {
        let leaf = self.editor.leaf(page)?;
        (0..leaf.view.index.blocks.len())
            .filter_map(|index| desk::read_block(&leaf.view, page, index))
            .filter(|block| desk::overlaps(block.area, area))
            .max_by(|one, other| {
                desk::overlap(one.area, area).total_cmp(&desk::overlap(other.area, area))
            })
    }

    fn perform(&mut self, call: &ToolCall) -> Performed {
        let request = match pdf_agent::tools::request::parse(&call.name, &call.arguments) {
            Ok(request) => request,
            Err(why) => return Performed::Done(ToolResult::failed(&call.id, why)),
        };
        let pages = self.editor.page_count();
        match &request {
            Request::DocumentInfo => {
                let sizes = self.shown_page_sizes();
                let set_aside = self.editor.restrictions_set_aside();
                let Some(source) = self.editor.source().cloned() else {
                    return Performed::Done(ToolResult::failed(
                        &call.id,
                        "there is no document open in this window",
                    ));
                };
                let credential = self.editor.credential().to_vec();
                Performed::Done(said(
                    &call.id,
                    pdf_agent::about::describe(&source, &credential, sizes, set_aside)
                        .map(|answer| (answer.text, None)),
                ))
            }
            Request::ReadText { first, last } => self.read_text(call, (*first, *last), pages),
            Request::FindText {
                text,
                match_case,
                first,
                last,
            } => self.find_text(call, (text, *match_case), (*first, *last), pages),
            Request::RenderPage { page, dpi } => self.render_page(call, *page, *dpi),
            Request::ListFonts { name } => Performed::Done(ToolResult::said(
                &call.id,
                tools::list_fonts_named(name.as_deref()).text,
            )),
            Request::ReplaceText { block, find, text } => {
                self.replace_text(call, request.clone(), (block, find.as_deref(), text))
            }
            Request::AddText {
                page, area, text, ..
            } => self.add_text(call, request.clone(), (*page, *area, text.clone())),
            Request::SetProperties(edit) => {
                let job = self.editor.begin_describe(edit.clone());
                self.send_for(call, request.clone(), job, None)
            }
            Request::FillField { name, value } => {
                self.fill_field(call, request.clone(), name, value)
            }
            Request::AddBlankPage { after, size } => {
                self.put_a_blank_page(call, request.clone(), *after, *size, pages)
            }
            Request::DeletePages(wanted) => {
                if let Some(said) = no_such_page(wanted, pages) {
                    return Performed::Done(ToolResult::failed(&call.id, said));
                }
                let job = self.editor.begin_remove_pages(wanted);
                self.send_for(call, request.clone(), job, None)
            }
            Request::MovePages { pages: wanted, to } => {
                if let Some(said) = no_such_page(wanted, pages) {
                    return Performed::Done(ToolResult::failed(&call.id, said));
                }
                let job = self.editor.begin_move_pages(wanted, *to);
                self.send_for(call, request.clone(), job, None)
            }
            Request::RotatePages {
                pages: wanted,
                quarter_turns,
            } => {
                if let Some(said) = no_such_page(wanted, pages) {
                    return Performed::Done(ToolResult::failed(&call.id, said));
                }
                let job = self.editor.begin_rotate_pages(wanted, *quarter_turns);
                self.send_for(call, request.clone(), job, None)
            }
            Request::InsertPages { .. } => self.insert_pages(call, request.clone(), pages),
            Request::Undo | Request::Redo => {
                let back = matches!(request, Request::Undo);
                if self.running.is_some() {
                    return Performed::Busy;
                }
                if self.walk_history(back) {
                    self.ai.tools.sent = Some(Sent {
                        call: call.clone(),
                        request,
                        was: None,
                    });
                    Performed::Sent
                } else {
                    Performed::Done(ToolResult::said(
                        &call.id,
                        if back {
                            "There is nothing to undo."
                        } else {
                            "There is nothing to redo."
                        },
                    ))
                }
            }
        }
    }

    fn shown_page_sizes(&self) -> Vec<[f64; 2]> {
        let geometries: Vec<pdf_content::PageGeometry> = (0..self.editor.page_count())
            .filter_map(|page| self.editor.geometry(page).copied())
            .collect();
        desk::shown_sizes(&geometries)
    }

    fn read_text(
        &mut self,
        call: &ToolCall,
        (first, last): (Option<usize>, Option<usize>),
        pages: usize,
    ) -> Performed {
        let (first, last) = match tools::page_span(first, last, pages) {
            Ok(span) => span,
            Err(why) => return Performed::Done(ToolResult::failed(&call.id, why)),
        };
        match self.gather_pages(call, (first, last), MOST_CHARACTERS) {
            Ok(gathered) => {
                let answer =
                    tools::format_pages((first, last), &mut |page| Ok(taken(&gathered, page)));
                Performed::Done(said(&call.id, answer.map(|answer| (answer.text, None))))
            }
            Err(waiting) => waiting,
        }
    }

    fn find_text(
        &mut self,
        call: &ToolCall,
        (text, match_case): (&str, bool),
        (first, last): (Option<usize>, Option<usize>),
        pages: usize,
    ) -> Performed {
        let (first, last) = match tools::page_span(first, last, pages) {
            Ok(span) => span,
            Err(why) => return Performed::Done(ToolResult::failed(&call.id, why)),
        };
        let stop = last.min(first + MOST_SEARCHED - 1);
        match self.gather_pages(call, (first, stop), usize::MAX) {
            Ok(gathered) => {
                let answer =
                    tools::format_hits((text, match_case), (first, stop, pages), &mut |page| {
                        Ok(taken(&gathered, page))
                    });
                let more = (stop < last).then(|| {
                    format!(
                        "\nOnly pages {} to {} were searched, which is as many as one search \
                         reads. Search on from page {} with first_page.",
                        first + 1,
                        stop + 1,
                        stop + 2
                    )
                });
                Performed::Done(said(&call.id, answer.map(|answer| (answer.text, more))))
            }
            Err(waiting) => waiting,
        }
    }

    fn gather_pages(
        &mut self,
        call: &ToolCall,
        span: (usize, usize),
        room: usize,
    ) -> Result<Vec<(usize, Vec<Block>)>, Performed> {
        let gathering = match self.ai.tools.gathering.take() {
            Some(gathering) if gathering.call == call.id && gathering.span == span => gathering,
            _ => Gathering {
                call: call.id.clone(),
                span,
                next: span.0,
                pages: Vec::new(),
                characters: 0,
            },
        };
        let mut gathering = gathering;
        while gathering.next <= span.1 {
            let page = gathering.next;
            if let Some(why) = self.failed.get(&page) {
                let said = format!("page {} cannot be read: {why}", page + 1);
                return Err(Performed::Done(ToolResult::failed(&call.id, said)));
            }
            let Some(leaf) = self.editor.leaf(page) else {
                self.ai.tools.gathering = Some(gathering);
                return Err(Performed::NeedPages(vec![page]));
            };
            let blocks: Vec<Block> = (0..leaf.view.index.blocks.len())
                .filter_map(|index| desk::read_block(&leaf.view, page, index))
                .collect();
            let size = tools::reading_size(&blocks);
            if gathering.characters > 0 && gathering.characters.saturating_add(size) > room {
                break;
            }
            gathering.characters = gathering.characters.saturating_add(size.min(room));
            for block in &blocks {
                self.remember_the_name(block);
            }
            gathering.pages.push((page, blocks));
            gathering.next += 1;
        }
        Ok(gathering.pages)
    }

    fn render_page(&mut self, call: &ToolCall, page: usize, dpi: f64) -> Performed {
        if page >= self.editor.page_count() {
            let said = format!(
                "there is no page {}: the document has {}",
                page + 1,
                self.editor.page_count()
            );
            return Performed::Done(ToolResult::failed(&call.id, said));
        }
        if let Some(why) = self.failed.get(&page) {
            let said = format!("page {} cannot be read: {why}", page + 1);
            return Performed::Done(ToolResult::failed(&call.id, said));
        }
        let Some(leaf) = self.editor.leaf(page).map(std::sync::Arc::clone) else {
            return Performed::NeedPages(vec![page]);
        };
        match desk::picture_of(&leaf.view, dpi) {
            Ok((png, width, height)) => {
                if let Some(image) = thumbnail(&leaf.view) {
                    self.ai.tools.pictures.insert(call.id.clone(), image);
                }
                let mut result = ToolResult::said(
                    &call.id,
                    format!("Page {} as shown, {width} x {height} pixels.", page + 1),
                );
                result.picture = Some(Picture {
                    media_type: "image/png".to_owned(),
                    base64: pdf_agent::json::base64(&png),
                });
                Performed::Done(result)
            }
            Err(why) => Performed::Done(ToolResult::failed(
                &call.id,
                format!("page {} cannot be drawn: {why}", page + 1),
            )),
        }
    }

    fn replace_text(
        &mut self,
        call: &ToolCall,
        request: Request,
        (name, find, text): (&str, Option<&str>, &str),
    ) -> Performed {
        let (page, block) = match self.block_named(name) {
            Ok(found) => found,
            Err(why) => return Performed::Done(ToolResult::failed(&call.id, why)),
        };
        let range = match find {
            None => self.editor.whole_block(page, block),
            Some(find) => match self.editor.block_reading(page, block) {
                Some(reading) => match desk::range_within(&reading, find) {
                    Ok(range) => Some(range),
                    Err(problem) => {
                        let said = format!("{name}: {problem}");
                        return Performed::Done(ToolResult::failed(&call.id, said));
                    }
                },
                None => None,
            },
        };
        let Some(range) = range else {
            let said = format!("{name} cannot be read as text");
            return Performed::Done(ToolResult::failed(&call.id, said));
        };
        let was = self
            .ai
            .tools
            .named
            .get(&(page, block))
            .map(|named| (page, named.area));
        let job = self
            .editor
            .begin_edit(page, block, range, &text.replace("\r\n", "\n"));
        self.send_for(call, request, job, was)
    }

    fn add_text(
        &mut self,
        call: &ToolCall,
        request: Request,
        (page, area, text): (usize, [f64; 4], String),
    ) -> Performed {
        let Request::AddText { style, .. } = &request else {
            return Performed::Done(ToolResult::failed(&call.id, "this is not new text"));
        };
        let style = pdf_app::NewTextStyle {
            family: style.family.clone(),
            size: style.size,
            bold: style.bold,
            italic: style.italic,
            fill: style.fill,
            paragraph: pdf_edit::ParagraphLayout::default(),
        };
        if page >= self.editor.page_count() {
            let said = format!(
                "there is no page {}: the document has {}",
                page + 1,
                self.editor.page_count()
            );
            return Performed::Done(ToolResult::failed(&call.id, said));
        }
        let Some(leaf) = self.editor.leaf(page).map(std::sync::Arc::clone) else {
            return Performed::NeedPages(vec![page]);
        };
        let frame = match desk::to_user(&leaf.view, area) {
            Ok(frame) => frame,
            Err(why) => return Performed::Done(ToolResult::failed(&call.id, why)),
        };
        let job = self.editor.begin_place_text(page, frame, &text, &style);
        self.send_for(call, request, job, None)
    }

    fn fill_field(
        &mut self,
        call: &ToolCall,
        request: Request,
        name: &str,
        value: &pdf_agent::json::Json,
    ) -> Performed {
        let Some(source) = self.editor.source().cloned() else {
            return Performed::Done(ToolResult::failed(
                &call.id,
                "there is no document open in this window",
            ));
        };
        let credential = self.editor.credential().to_vec();
        let command = match pdf_agent::about::field_to_fill(&source, &credential, name, value) {
            Ok(command) => command,
            Err(why) => return Performed::Done(ToolResult::failed(&call.id, why)),
        };
        let pdf_edit::Command::FillField {
            page_index,
            widget,
            value,
        } = command
        else {
            return Performed::Done(ToolResult::failed(&call.id, "that field cannot be filled"));
        };
        let job = self.editor.begin_fill_field(page_index, widget, value);
        self.send_for(call, request, job, None)
    }

    fn put_a_blank_page(
        &mut self,
        call: &ToolCall,
        request: Request,
        after: usize,
        size: Option<[f64; 2]>,
        pages: usize,
    ) -> Performed {
        if after > pages {
            let said = format!("there is no page {after}: the document has {pages}");
            return Performed::Done(ToolResult::failed(&call.id, said));
        }
        let (beside, before) = tools::beside_after(after);
        let size = match size {
            Some(size) => size,
            None => match self.shown_page_sizes().get(beside).copied() {
                Some(size) => size,
                None => {
                    return Performed::Done(ToolResult::failed(
                        &call.id,
                        "the page beside this one has no size: pass width and height",
                    ));
                }
            },
        };
        let job = self.editor.begin_add_page(beside, before, size);
        self.send_for(call, request, job, None)
    }

    fn insert_pages(&mut self, call: &ToolCall, request: Request, pages: usize) -> Performed {
        let Request::InsertPages {
            from,
            pages: chosen,
            after,
        } = &request
        else {
            return Performed::Done(ToolResult::failed(&call.id, "this is not an insertion"));
        };
        if *after > pages {
            let said = format!("there is no page {after}: the document has {pages}");
            return Performed::Done(ToolResult::failed(&call.id, said));
        }
        let bytes: std::sync::Arc<[u8]> = match std::fs::read(from) {
            Ok(bytes) => bytes.into(),
            Err(error) => {
                let said = format!("{} cannot be read: {error}", from.display());
                return Performed::Done(ToolResult::failed(&call.id, said));
            }
        };
        let other =
            pdf_bytes::ByteStore::new(pdf_bytes::SourceId::new(1), std::sync::Arc::clone(&bytes));
        if pdf_edit::info::lock(&other, b"") == pdf_edit::info::Lock::Refused {
            let said = format!(
                "{} is protected by a password, and pages cannot be taken out of a protected \
                 file yet -- not even with the password. Open it in PanPDF, save an unprotected \
                 copy, and take the pages from that.",
                from.display()
            );
            return Performed::Done(ToolResult::failed(&call.id, said));
        }
        let available = match pdf_session::Session::new(other, b"").page_count() {
            Ok(available) => available,
            Err(error) => {
                let said = format!("{} has no pages this can read: {error}", from.display());
                return Performed::Done(ToolResult::failed(&call.id, said));
            }
        };
        let wanted: Vec<usize> = match chosen {
            Some(chosen) => chosen.clone(),
            None => (0..available).collect(),
        };
        if let Some(missing) = wanted.iter().find(|page| **page >= available) {
            let said = format!(
                "{} has no page {}: it has {available}",
                from.display(),
                missing + 1
            );
            return Performed::Done(ToolResult::failed(&call.id, said));
        }
        let (beside, before) = tools::beside_after(*after);
        let job = self
            .editor
            .begin_insert_pages((beside, before), bytes, &wanted);
        self.send_for(call, request, job, None)
    }

    fn send_for(
        &mut self,
        call: &ToolCall,
        request: Request,
        job: Option<pdf_app::EditJob>,
        was: Option<(usize, [f64; 4])>,
    ) -> Performed {
        let Some(job) = job else {
            return Performed::Busy;
        };
        self.ai.tools.sent = Some(Sent {
            call: call.clone(),
            request,
            was,
        });
        self.send(Some(job));
        Performed::Sent
    }

    fn ask_the_painter_for(&mut self, page: usize) {
        let epoch = self.editor.epoch();
        if self.editor.leaf(page).is_some()
            || self.painter.reading(page, epoch)
            || self.failed.contains_key(&page)
        {
            return;
        }
        let Some(source) = self.editor.source().cloned() else {
            return;
        };
        let credential = self.editor.credential().to_vec();
        let grouping = self.editor.grouping(page);
        self.painter.read(
            page,
            source,
            &credential,
            grouping,
            self.editor.fonts(),
            epoch,
        );
    }

    fn remember_the_name(&mut self, block: &Block) {
        self.ai.tools.named.insert(
            (block.page, block.index),
            Named {
                epoch: self.editor.epoch(),
                arranged: self.ai.tools.arranged,
                text: block.text.clone(),
                area: block.area,
            },
        );
    }

    fn block_named(&self, name: &str) -> Result<(usize, usize), String> {
        let (page, block) = parse_block_name(name)?;
        let named = self.ai.tools.named.get(&(page, block)).ok_or_else(|| {
            format!("{name} has not been read yet: call read_text or find_text for that page first")
        })?;
        let now: Option<Vec<Block>> = (named.epoch != self.editor.epoch())
            .then(|| {
                self.editor.leaf(page).map(|leaf| {
                    (0..leaf.view.index.blocks.len())
                        .filter_map(|index| desk::read_block(&leaf.view, page, index))
                        .collect()
                })
            })
            .flatten();
        the_same_block(
            name,
            (page, block),
            named,
            (self.ai.tools.arranged, self.editor.epoch()),
            now.as_deref(),
        )
    }
}

fn parse_block_name(name: &str) -> Result<(usize, usize), String> {
    let bad = || format!("{name:?} is not a block name: they look like p3-b12");
    let rest = name.trim().strip_prefix('p').ok_or_else(bad)?;
    let (page, block) = rest.split_once("-b").ok_or_else(bad)?;
    let page: usize = page.parse().map_err(|_| bad())?;
    let block: usize = block.parse().map_err(|_| bad())?;
    if page == 0 || block == 0 {
        return Err(bad());
    }
    Ok((page - 1, block - 1))
}

fn the_same_block(
    name: &str,
    (page, block): (usize, usize),
    named: &Named,
    (arranged, epoch): (u64, u64),
    now: Option<&[Block]>,
) -> Result<(usize, usize), String> {
    if named.arranged != arranged {
        return Err(format!(
            "{name} was read before the pages were moved about, and its page number no \
             longer means the page it meant: read_text again and use the name it gives now"
        ));
    }
    if named.epoch == epoch {
        return Ok((page, block));
    }
    let Some(now) = now else {
        return Err(format!(
            "{name} is on a page that has not been read since it changed: read_text page {} again",
            page + 1
        ));
    };
    let same: Vec<usize> = now
        .iter()
        .filter(|block| block.text == named.text && desk::overlaps(block.area, named.area))
        .map(|block| block.index)
        .collect();
    match same[..] {
        [only] => Ok((page, only)),
        [] => Err(format!(
            "{name} is no longer on the page as it was read: read_text page {} again",
            page + 1
        )),
        _ => Err(format!(
            "{name} cannot be told apart from another block since the page changed: \
             read_text page {} again",
            page + 1
        )),
    }
}

fn taken(gathered: &[(usize, Vec<Block>)], page: usize) -> Vec<Block> {
    gathered
        .iter()
        .find(|(at, _)| *at == page)
        .map(|(_, blocks)| blocks.clone())
        .unwrap_or_default()
}

fn said(call: &str, answer: Result<(String, Option<String>), String>) -> ToolResult {
    match answer {
        Ok((text, more)) => ToolResult::said(call, format!("{text}{}", more.unwrap_or_default())),
        Err(why) => ToolResult::failed(call, why),
    }
}

fn result_of(
    call: &str,
    (applied, request): (&Applied, &Request),
    now: Option<&Block>,
) -> ToolResult {
    match applied {
        Applied::Changed { page, .. } => ToolResult::said(call, what_changed(request, *page, now)),
        Applied::Unchanged => ToolResult::said(
            call,
            match request {
                Request::Undo => "There is nothing to undo.",
                Request::Redo => "There is nothing to redo.",
                _ => "Nothing changed.",
            },
        ),
        Applied::Refused(reason) => {
            ToolResult::failed(call, Message::Refused(reason.clone()).say(Lang::English))
        }
    }
}

fn what_changed(request: &Request, page: usize, now: Option<&Block>) -> String {
    match request {
        Request::ReplaceText { .. } => now.map_or_else(
            || "Done. The block could not be read back: read_text that page again.".to_owned(),
            |block| format!("Done. {} now reads: {}", block.name(), block.text),
        ),
        Request::AddText { style, .. } => format!(
            "Written on page {} in {}, {} pt.",
            page + 1,
            style.family,
            style.size
        ),
        Request::SetProperties(_) => "Properties set.".to_owned(),
        Request::FillField { name, .. } => format!("{name} is filled."),
        Request::AddBlankPage { after, .. } => format!("A blank page is now page {}.", after + 1),
        Request::DeletePages(pages) => {
            format!("Took out {} page{}.", pages.len(), plural(pages.len()))
        }
        Request::MovePages { .. } => "Moved.".to_owned(),
        Request::RotatePages { .. } => "Turned.".to_owned(),
        Request::InsertPages { pages, after, .. } => match pages {
            Some(pages) => format!(
                "Put in {} page{} after page {after}.",
                pages.len(),
                plural(pages.len())
            ),
            None => format!("Put the file's pages in after page {after}."),
        },
        Request::Undo => "Took back the last change.".to_owned(),
        Request::Redo => "Put the change back.".to_owned(),
        _ => "Done.".to_owned(),
    }
}

fn no_such_page(wanted: &[usize], pages: usize) -> Option<String> {
    wanted
        .iter()
        .find(|page| **page >= pages)
        .map(|page| format!("there is no page {}: the document has {pages}", page + 1))
}

fn thumbnail(view: &pdf_session::PageView) -> Option<egui::ColorImage> {
    let device = pdf_render::DeviceTransform::for_page(
        &view.program.geometry,
        1.0,
        pdf_render::RenderLimits::default(),
    )
    .ok()?;
    let longest = f64::from(device.width.max(device.height)).max(1.0);
    let (canvas, _) =
        pdf_cli::render_page_view(view, (THUMBNAIL / longest).clamp(0.02, 1.0)).ok()?;
    let rgb = canvas.to_rgb8();
    let size = [canvas.width as usize, canvas.height as usize];
    let pixels: Vec<egui::Color32> = rgb
        .chunks_exact(3)
        .map(|pixel| egui::Color32::from_rgb(pixel[0], pixel[1], pixel[2]))
        .collect();
    (pixels.len() == size[0] * size[1]).then(|| egui::ColorImage::new(size, pixels))
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

pub(crate) const fn tools_are_offered(mode: Mode) -> bool {
    !matches!(mode, Mode::ChatOnly)
}

#[cfg(test)]
mod tests;
