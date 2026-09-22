use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{
    Arc,
    mpsc::{self, Receiver, TryRecvError},
};
use std::thread;

use eframe::egui;
use pdf_agent::connect::{
    ConnectError, Connection, Effort, Model, Provider as WireProvider, Reply, Said, ToolResult,
    Turn, settle_dangling_calls,
};
use pdf_agent::tools::DocumentBrief;

use pdf_app::ai_layout;
use pdf_app::ai_permission::{Answer as Allowed, Mode, describe_call};
use pdf_app::wording::{Lang, Message};

use crate::ai_actions::{MOST_ROUNDS, Tools, tools_are_offered};
use crate::window_state::Window;

const MAX_CONTEXT: usize = 12_000;

const GAP: f32 = 8.0;

const LEAST_CONVERSATION: f32 = 80.0;

fn one_row<R>(
    ui: &mut egui::Ui,
    (parts, fixed): (usize, f32),
    add: impl FnOnce(&mut egui::Ui, f32) -> R,
) -> R {
    let each = ai_layout::split_row(ui.available_width(), fixed, GAP, parts);
    let size = egui::vec2(ui.available_width(), control_height(ui));
    ui.allocate_ui_with_layout(
        size,
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| add(ui, each),
    )
    .inner
}

fn plain_enter(ui: &mut egui::Ui) -> bool {
    ui.input_mut(|input| {
        let before = input.events.len();
        input.events.retain(|event| {
            !matches!(
                event,
                egui::Event::Key {
                    key: egui::Key::Enter,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.is_none()
            )
        });
        input.events.len() < before
    })
}

fn control_height(ui: &egui::Ui) -> f32 {
    ui.spacing()
        .interact_size
        .y
        .max(crate::format::CONTROL_HEIGHT)
}

fn composer_chrome(ui: &egui::Ui) -> f32 {
    let frame = egui::Frame::group(ui.style());
    frame.inner_margin.sum().y + frame.stroke.width * 2.0 + control_height(ui) + GAP * 0.75
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Provider {
    OpenAi,
    Claude,
    Gemini,
    Ollama,
    LmStudio,
    Custom,
}

impl Provider {
    const ALL: [Self; 6] = [
        Self::OpenAi,
        Self::Claude,
        Self::Gemini,
        Self::Ollama,
        Self::LmStudio,
        Self::Custom,
    ];
    const fn name(self) -> &'static str {
        match self {
            Self::OpenAi => "OpenAI",
            Self::Claude => "Claude",
            Self::Gemini => "Gemini",
            Self::Ollama => "Ollama",
            Self::LmStudio => "LM Studio",
            Self::Custom => "Custom compatible",
        }
    }
    fn by_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|one| one.name() == name)
    }
    const fn wire(self) -> WireProvider {
        match self {
            Self::OpenAi => WireProvider::OpenAi,
            Self::Claude => WireProvider::Anthropic,
            Self::Gemini => WireProvider::Gemini,
            Self::Ollama => WireProvider::Ollama,
            Self::LmStudio => WireProvider::LmStudio,
            Self::Custom => WireProvider::Custom,
        }
    }
    const fn base_url(self) -> &'static str {
        self.wire().default_base_url()
    }
}

fn choice_file() -> Option<std::path::PathBuf> {
    if cfg!(test) {
        return None;
    }
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|home| std::path::PathBuf::from(home).join(".local").join("state"))
        })?;
    Some(state.join("panpdf").join("ai"))
}

enum Answer {
    Models(u64, Result<Vec<Model>, ConnectError>),
    Chat(u64, Result<Reply, ConnectError>),
}

#[derive(Clone, Debug, PartialEq)]
struct Notice {
    said: Message,
    detail: Option<String>,
}

impl Notice {
    fn plain(said: Message) -> Self {
        Self { said, detail: None }
    }
}

fn notice_of(error: &ConnectError) -> Notice {
    let (said, detail) = match error {
        ConnectError::Invalid(why) => (Message::AiConnectionInvalid, Some(why)),
        ConnectError::Cancelled => (Message::AiStopped, None),
        ConnectError::Curl(why) => (Message::AiCouldNotReach, Some(why)),
        ConnectError::ResponseTooLarge => (Message::AiAnswerTooLarge, None),
        ConnectError::Http(why) => (Message::AiServiceRefused, Some(why)),
        ConnectError::Protocol(why) => (Message::AiAnswerUnexpected, Some(why)),
    };
    Notice {
        said,
        detail: detail.cloned(),
    }
}

struct Asking {
    stop: Arc<AtomicBool>,
    answers: Receiver<Answer>,
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "four independent things a person can turn on: the panel, its \
              settings, whether a provider answered, and whether the page \
              goes with the question"
)]
pub(crate) struct AiState {
    pub(crate) open: bool,
    provider: Provider,
    base_url: String,
    key: String,
    model: String,
    models: Vec<Model>,
    turns: Vec<Turn>,
    composer: String,
    asked: Option<String>,
    settings_open: bool,
    notice: Option<Notice>,
    connected: bool,
    include_context: bool,
    generation: u64,
    asking: Option<Asking>,
    wrapped_rows: usize,
    effort: Effort,
    pub(crate) mode: Mode,
    pub(crate) tools: Tools,
    context: String,
    brief: DocumentBrief,
    notes: Vec<(usize, Message)>,
}

impl Default for AiState {
    fn default() -> Self {
        let provider = Provider::OpenAi;
        Self {
            open: false,
            provider,
            base_url: provider.base_url().into(),
            key: String::new(),
            model: String::new(),
            models: Vec::new(),
            turns: Vec::new(),
            composer: String::new(),
            asked: None,
            settings_open: true,
            notice: None,
            connected: false,
            include_context: false,
            generation: 0,
            asking: None,
            wrapped_rows: ai_layout::LEAST_ROWS,
            effort: Effort::Off,
            mode: Mode::default(),
            tools: Tools::default(),
            context: String::new(),
            brief: DocumentBrief::default(),
            notes: Vec::new(),
        }
    }
}

impl AiState {
    pub(crate) fn remembered() -> Self {
        let mut state = Self::default();
        let Some(choice) = choice_file()
            .and_then(|file| std::fs::read_to_string(file).ok())
            .and_then(|text| pdf_app::ai_choice::read(&text))
        else {
            return state;
        };
        let Some(provider) = Provider::by_name(&choice.provider) else {
            return state;
        };
        state.provider = provider;
        state.base_url = if choice.base_url.is_empty() {
            provider.base_url().into()
        } else {
            choice.base_url
        };
        state.model = choice.model;
        state.effort = Effort::parse(&choice.effort).unwrap_or(Effort::Off);
        state.mode = Mode::parse(&choice.mode)
            .unwrap_or_else(|| Mode::parse(pdf_app::ai_choice::DEFAULT_MODE).unwrap_or_default());
        state.settings_open = state.model.is_empty() || state.key_missing();
        state
    }

    fn remember(&self) {
        let choice = pdf_app::ai_choice::Choice {
            provider: self.provider.name().to_owned(),
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            effort: self.effort.as_str().to_owned(),
            mode: self.mode.as_str().to_owned(),
        };
        let (Some(file), Some(line)) = (choice_file(), pdf_app::ai_choice::write(&choice)) else {
            return;
        };
        if let Some(folder) = file.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let temporary = file.with_extension("new");
        if std::fs::write(&temporary, line).is_ok() {
            let _ = std::fs::rename(&temporary, &file);
        }
    }

    pub(crate) fn busy(&self) -> bool {
        self.asking.is_some()
    }

    fn connection(&self) -> Connection {
        Connection {
            provider: self.provider.wire(),
            base_url: self.base_url.trim_end_matches('/').to_owned(),
            model: self.model.clone(),
            api_key: self.key.clone(),
            effort: self.effort,
        }
    }
    fn invalidate(&mut self) {
        self.generation += 1;
        self.connected = false;
        self.models.clear();
        self.notice = None;
    }
    fn disconnect(&mut self) {
        self.cancel();
        self.key.clear();
        self.invalidate();
    }
    fn cancel(&mut self) {
        if let Some(asking) = self.asking.take() {
            asking.stop.store(true, Ordering::Relaxed);
        }
        self.generation += 1;
    }
    fn poll(&mut self, ctx: &egui::Context) {
        let Some(asking) = self.asking.as_ref() else {
            return;
        };
        let result = match asking.answers.try_recv() {
            Ok(answer) => Some(answer),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.asking = None;
                self.notice = Some(Notice::plain(Message::AiWorkerStopped));
                return;
            }
        };
        let Some(answer) = result else { return };
        self.asking = None;
        self.asked = None;
        let generation = match &answer {
            Answer::Models(g, _) | Answer::Chat(g, _) => *g,
        };
        if generation != self.generation {
            return;
        }
        match answer {
            Answer::Models(_, result) => match result {
                Ok(models) => {
                    self.models = models;
                    self.connected = true;
                }
                Err(error) => {
                    self.connected = false;
                    self.notice = Some(notice_of(&error));
                }
            },
            Answer::Chat(_, result) => {
                match result {
                    Ok(reply) => {
                        self.turns.push(Turn::answered(&reply));
                        self.tools.take(&reply.calls);
                        if reply.calls.is_empty() {
                            self.tools.rounds = 0;
                        }
                        self.settings_open = false;
                        self.connected = true;
                    }
                    Err(error) => {
                        self.tools.drop_the_queue();
                        if self.turns.last().is_some_and(|turn| {
                            turn.said() == Said::Person && !turn.text().is_empty()
                        }) && let Some(turn) = self.turns.pop()
                        {
                            turn.text().clone_into(&mut self.composer);
                        }
                        self.notice = Some(notice_of(&error));
                    }
                }
            }
        }
        ctx.request_repaint();
    }
    fn start_models(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let generation = self.generation;
        let connection = self.connection();
        let flag = Arc::new(AtomicBool::new(false));
        let worker_flag = flag.clone();
        let (tx, rx) = mpsc::channel();
        self.notice = None;
        self.asking = Some(Asking {
            stop: flag,
            answers: rx,
        });
        let repaint = ctx.clone();
        thread::spawn(move || {
            let result = connection.models(&worker_flag);
            let _ = tx.send(Answer::Models(generation, result));
            repaint.request_repaint();
        });
    }
    fn start_answer(&mut self, ctx: &egui::Context, context: String, brief: &DocumentBrief) {
        if self.busy() || self.composer.trim().is_empty() {
            return;
        }
        settle_dangling_calls(&mut self.turns);
        self.tools.drop_the_queue();
        let asked = std::mem::take(&mut self.composer);
        self.turns.push(Turn::person(asked.trim()));
        self.asked = Some(asked.trim().to_owned());
        self.context = context;
        self.brief = brief.clone();
        self.ask_the_model(ctx);
    }

    pub(crate) fn start_round(&mut self, ctx: &egui::Context, brief: &DocumentBrief) {
        if self.busy() {
            return;
        }
        self.brief = brief.clone();
        self.ask_the_model(ctx);
    }

    pub(crate) fn round_done(&mut self, results: Vec<ToolResult>) -> bool {
        self.turns.push(Turn::Results { results });
        self.tools.rounds += 1;
        self.tools.rounds <= MOST_ROUNDS
    }

    pub(crate) fn say_the_document_changed(&mut self, page: usize) {
        self.notes
            .push((self.turns.len(), Message::AiChangedTheDocument { page }));
    }

    pub(crate) fn stop_for_too_many_rounds(&mut self) {
        self.tools.drop_the_queue();
        settle_dangling_calls(&mut self.turns);
        self.notice = Some(Notice::plain(Message::AiTooManyRounds));
    }

    fn ask_the_model(&mut self, ctx: &egui::Context) {
        let generation = self.generation;
        let connection = self.connection();
        let turns = self.turns.clone();
        let context = self.context.clone();
        let (tools, system) = if tools_are_offered(self.mode) {
            (
                pdf_agent::tools::offered_to_a_window(),
                Some(pdf_agent::tools::window_instructions(&self.brief)),
            )
        } else {
            (Vec::new(), None)
        };
        let flag = Arc::new(AtomicBool::new(false));
        let worker_flag = flag.clone();
        let (tx, rx) = mpsc::channel();
        self.notice = None;
        self.asking = Some(Asking {
            stop: flag,
            answers: rx,
        });
        let repaint = ctx.clone();
        thread::spawn(move || {
            let result = connection.converse_with(
                &turns,
                (!context.is_empty()).then_some(context.as_str()),
                &tools,
                system.as_deref(),
                &worker_flag,
            );
            let _ = tx.send(Answer::Chat(generation, result));
            repaint.request_repaint();
        });
    }

    fn new_chat(&mut self) {
        self.cancel();
        self.turns.clear();
        self.notes.clear();
        self.tools.clear();
        self.asked = None;
        self.notice = None;
        self.context.clear();
    }

    fn working(&self) -> bool {
        self.busy() || self.tools.busy()
    }

    fn ready(&self) -> bool {
        !self.base_url.is_empty() && !self.model.is_empty() && !self.key_missing()
    }

    fn key_missing(&self) -> bool {
        self.key.trim().is_empty()
            && matches!(
                self.provider,
                Provider::OpenAi | Provider::Claude | Provider::Gemini
            )
    }

    fn who_answers(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, lang: Lang) {
        let say = |message: Message| message.say(lang);
        one_row(ui, (2, crate::format::CONTROL_HEIGHT), |ui, each| {
            let mut chosen = self.provider;
            egui::ComboBox::from_id_salt("ai-provider")
                .width(each)
                .selected_text(self.provider.name())
                .show_ui(ui, |ui| {
                    for provider in Provider::ALL {
                        ui.selectable_value(&mut chosen, provider, provider.name());
                    }
                });
            if chosen != self.provider && !self.busy() {
                self.provider = chosen;
                self.base_url = chosen.base_url().into();
                self.model.clear();
                self.invalidate();
                self.settings_open = self.key_missing();
                self.remember();
                self.models_if_possible(ctx);
            }
            let named = if self.model.is_empty() {
                say(Message::AiNoModelChosen)
            } else {
                self.model.clone()
            };
            let mut picked = None;
            egui::ComboBox::from_id_salt("ai-model")
                .width(each)
                .selected_text(named)
                .show_ui(ui, |ui| {
                    if self.models.is_empty() {
                        ui.weak(if self.key_missing() {
                            say(Message::AiKeyNeeded)
                        } else {
                            say(Message::AiFindModels)
                        });
                    }
                    for model in &self.models {
                        if ui
                            .selectable_label(self.model == model.id, &model.id)
                            .clicked()
                        {
                            picked = Some(model.id.clone());
                        }
                    }
                });
            if let Some(model) = picked {
                self.model = model;
                self.notice = None;
                self.connected = true;
                self.settings_open = false;
                self.remember();
            }
            if crate::format::icon_button(
                ui,
                crate::icons::Icon::Settings,
                &say(Message::AiConnection),
                self.settings_open,
                true,
            )
            .clicked()
            {
                self.settings_open = !self.settings_open;
            }
        });
    }

    fn the_notice(&mut self, ui: &mut egui::Ui, lang: Lang) {
        let Some(notice) = self.notice.clone() else {
            return;
        };
        let mut close = false;
        egui::Frame::group(ui.style())
            .fill(ui.visuals().faint_bg_color)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                    if crate::format::icon_button(
                        ui,
                        crate::icons::Icon::Close,
                        &Message::AiDismiss.say(lang),
                        false,
                        true,
                    )
                    .clicked()
                    {
                        close = true;
                    }
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(notice.said.say(lang))
                                .color(ui.visuals().error_fg_color),
                        )
                        .wrap(),
                    );
                });
                if let Some(detail) = &notice.detail {
                    ui.add(
                        egui::Label::new(egui::RichText::new(detail).small().weak())
                            .selectable(true)
                            .wrap(),
                    );
                }
            });
        if close {
            self.notice = None;
        }
    }

    fn the_connection(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, lang: Lang) {
        let say = |message: Message| message.say(lang);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add_enabled_ui(!self.busy(), |ui| {
                egui::Grid::new("ai-connection")
                    .num_columns(2)
                    .spacing([GAP, GAP * 0.75])
                    .show(ui, |ui| {
                        ui.label(say(Message::AiBaseUrl));
                        let address = ui.add(
                            egui::TextEdit::singleline(&mut self.base_url)
                                .desired_width(f32::INFINITY),
                        );
                        if address.changed() {
                            self.invalidate();
                        }
                        if address.lost_focus() {
                            self.remember();
                        }
                        ui.end_row();
                        ui.label(say(Message::AiApiKey));
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut self.key)
                                    .password(true)
                                    .desired_width(f32::INFINITY),
                            )
                            .lost_focus()
                        {
                            self.models_if_possible(ctx);
                        }
                        ui.end_row();
                    });
                ui.add_space(GAP * 0.5);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !self.base_url.is_empty(),
                            egui::Button::new(say(Message::AiFindModels)),
                        )
                        .clicked()
                    {
                        self.start_models(ctx);
                    }
                    if ui
                        .add_enabled(
                            !self.key.is_empty(),
                            egui::Button::new(say(Message::AiDisconnect)),
                        )
                        .clicked()
                    {
                        self.disconnect();
                    }
                    if self.connected {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.weak(say(Message::AiConnected));
                        });
                    }
                });
                ui.add_space(GAP * 0.5);
                ui.weak(egui::RichText::new(say(Message::AiPrivacy)).small());
            });
        });
    }

    fn models_if_possible(&mut self, ctx: &egui::Context) {
        if self.busy() || self.base_url.is_empty() || self.key_missing() {
            return;
        }
        self.start_models(ctx);
    }

    fn the_conversation(&mut self, ui: &mut egui::Ui, lang: Lang) -> Option<Allowed> {
        let say = |message: Message| message.say(lang);
        let card = self.tools.ask.as_ref().map(|pending| {
            (
                describe_call(&pending.request, lang),
                pending.may_allow_for_chat,
            )
        });
        let mut answered = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                if self.turns.is_empty() {
                    ui.add_space(GAP);
                    ui.weak(say(Message::AiNothingAskedYet));
                }
                for (at, turn) in self.turns.iter().enumerate() {
                    for (_, said) in self.notes.iter().filter(|(index, _)| *index == at) {
                        a_note(ui, &said.say(lang));
                    }
                    match turn {
                        Turn::Results { results } => {
                            what_the_tools_did(ui, results, &self.tools);
                        }
                        _ => said_by(ui, turn, (&self.model, lang)),
                    }
                }
                for (_, said) in self
                    .notes
                    .iter()
                    .filter(|(index, _)| *index >= self.turns.len())
                {
                    a_note(ui, &said.say(lang));
                }
                if let Some((wants, may_remember)) = card {
                    answered = the_card(ui, (&wants, may_remember), lang);
                }
                if self.working() {
                    ui.add_space(GAP);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.weak(say(Message::AiThinking));
                    });
                }
            });
        answered
    }

    fn what_it_may_do(&mut self, ui: &mut egui::Ui, lang: Lang) {
        let say = |message: Message| message.say(lang);
        one_row(ui, (2, 0.0), |ui, each| {
            let mut effort = self.effort;
            egui::ComboBox::from_id_salt("ai-effort")
                .width(each)
                .selected_text(say(effort_said(self.effort)))
                .show_ui(ui, |ui| {
                    for one in [Effort::Off, Effort::Low, Effort::Medium, Effort::High] {
                        ui.selectable_value(&mut effort, one, say(effort_said(one)));
                    }
                });
            if effort != self.effort {
                self.effort = effort;
                self.remember();
            }
            let mut mode = self.mode;
            egui::ComboBox::from_id_salt("ai-mode")
                .width(each)
                .selected_text(say(mode_said(self.mode)))
                .show_ui(ui, |ui| {
                    for one in [Mode::ChatOnly, Mode::AskBeforeChanges, Mode::DoIt] {
                        ui.selectable_value(&mut mode, one, say(mode_said(one)))
                            .on_hover_text(say(mode_means(one)));
                    }
                })
                .response
                .on_hover_text(say(mode_means(self.mode)));
            if mode != self.mode {
                self.mode = mode;
                self.remember();
            }
        });
    }

    fn the_composer(
        &mut self,
        ui: &mut egui::Ui,
        lang: Lang,
        (rows, row_height): (usize, f32),
    ) -> bool {
        let say = |message: Message| message.say(lang);
        let mut asked = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let typed = egui::ScrollArea::vertical()
                .id_salt("ai-composer")
                .max_height(ai_layout::composer_height(rows, row_height, 0.0))
                .show(ui, |ui| {
                    egui::TextEdit::multiline(&mut self.composer)
                        .id(composer_id())
                        .hint_text(say(Message::AiAskHint))
                        .frame(egui::Frame::NONE)
                        .desired_width(f32::INFINITY)
                        .desired_rows(rows)
                        .return_key(Some(egui::KeyboardShortcut::new(
                            egui::Modifiers::SHIFT,
                            egui::Key::Enter,
                        )))
                        .show(ui)
                })
                .inner;
            self.wrapped_rows = typed.galley.rows.len();
            if typed.response.has_focus() && plain_enter(ui) && !self.composer.trim().is_empty() {
                asked = true;
            }
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.include_context, say(Message::AiSendThePage))
                    .on_hover_text(say(Message::AiIncludeContext {
                        characters: MAX_CONTEXT,
                    }));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.working() {
                        if ui.button(say(Message::AiCancel)).clicked() {
                            self.cancel();
                            self.tools.drop_the_queue();
                            self.asked = None;
                        }
                    } else {
                        let ready = self.ready() && !self.composer.trim().is_empty();
                        if ui
                            .add_enabled(ready, egui::Button::new(say(Message::AiSend)))
                            .on_hover_text(say(Message::AiEnterSends))
                            .clicked()
                        {
                            asked = true;
                        }
                    }
                });
            });
        });
        if asked && !self.ready() {
            self.settings_open = true;
            self.notice = Some(Notice::plain(if self.key_missing() {
                Message::AiKeyNeeded
            } else {
                Message::AiNoModelChosen
            }));
            return false;
        }
        asked
    }
}

fn said_by(ui: &mut egui::Ui, turn: &Turn, (model, lang): (&str, Lang)) {
    let mine = turn.said() == Said::Person;
    let name = if mine {
        Message::AiYou.say(lang)
    } else if model.is_empty() {
        Message::AiResponse.say(lang)
    } else {
        model.to_owned()
    };
    ui.add_space(GAP);
    let frame = if mine {
        egui::Frame::group(ui.style()).fill(ui.visuals().faint_bg_color)
    } else {
        egui::Frame::group(ui.style())
    };
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.weak(egui::RichText::new(name).small());
            if !mine {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button(Message::DraftCopy.say(lang)).clicked() {
                        ui.ctx().copy_text(turn.text().to_owned());
                    }
                });
            }
        });
        ui.add(egui::Label::new(turn.text()).selectable(true).wrap());
    });
}

pub(crate) fn composer_id() -> egui::Id {
    egui::Id::new("ai-composer-text")
}

impl Window {
    pub(crate) fn open_ai_panel(&mut self) {
        self.ai.open = true;
    }

    pub(crate) fn ai_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.ai.poll(&ctx);
        if !self.ai.open {
            return;
        }
        let lang = self.lang;
        let say = |message: Message| message.say(lang);
        let mut asked = false;
        let mut close = false;
        let mut answered = None;
        let margin = 8_i8;
        let frame = egui::Frame::side_top_panel(&ui.style().clone())
            .inner_margin(egui::Margin::symmetric(margin, margin));
        egui::Panel::right("ai chat")
            .resizable(true)
            .default_size(360.0)
            .size_range(300.0..=640.0)
            .frame(frame)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(GAP, GAP * 0.75);
                let panel_height = ui.available_height();
                ui.horizontal(|ui| {
                    ui.strong(say(Message::AiTitle));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if crate::format::icon_button(
                            ui,
                            crate::icons::Icon::Close,
                            &say(Message::Close),
                            false,
                            true,
                        )
                        .clicked()
                        {
                            close = true;
                        }
                        if crate::format::icon_button(
                            ui,
                            crate::icons::Icon::Plus,
                            &say(Message::AiNewChat),
                            false,
                            !self.ai.turns.is_empty(),
                        )
                        .clicked()
                        {
                            self.ai.new_chat();
                        }
                    });
                });
                self.ai.who_answers(ui, &ctx, lang);
                self.ai.what_it_may_do(ui, lang);
                if self.ai.settings_open {
                    self.ai.the_connection(ui, &ctx, lang);
                }
                self.ai.the_notice(ui, lang);
                ui.separator();
                let row_height = ui.text_style_height(&egui::TextStyle::Body);
                let chrome = composer_chrome(ui);
                let rows = ai_layout::rows_shown(
                    self.ai.wrapped_rows,
                    ai_layout::most_rows(panel_height, row_height, chrome),
                );
                let composer = ai_layout::composer_height(rows, row_height, chrome);
                let room = ai_layout::conversation_room(
                    ui.available_height(),
                    composer,
                    LEAST_CONVERSATION,
                );
                ui.allocate_ui(egui::vec2(ui.available_width(), room), |ui| {
                    answered = self.ai.the_conversation(ui, lang);
                });
                asked = self.ai.the_composer(ui, lang, (rows, row_height));
            });
        if let Some(answer) = answered {
            self.ai.tools.answer_the_card(answer);
        }
        if asked {
            let context = if self.ai.include_context {
                current_page_text(self, MAX_CONTEXT)
            } else {
                String::new()
            };
            let brief = self.document_brief();
            self.ai.start_answer(&ctx, context, &brief);
        }
        self.advance_tools(&ctx);
        if close {
            self.ai.open = false;
        }
    }
}

fn what_the_tools_did(ui: &mut egui::Ui, results: &[ToolResult], tools: &Tools) {
    for result in results {
        ui.add_space(GAP * 0.5);
        ui.horizontal(|ui| {
            let name = tools
                .called
                .get(&result.call_id)
                .map_or("tool", String::as_str);
            let colour = if result.is_error {
                ui.visuals().error_fg_color
            } else {
                ui.visuals().weak_text_color()
            };
            ui.label(
                egui::RichText::new(format!("{name} \u{00b7} {}", one_line(&result.text)))
                    .small()
                    .color(colour),
            );
        });
        if let Some(image) = tools.pictures.get(&result.call_id) {
            let texture = ui.ctx().load_texture(
                format!("ai-picture-{}", result.call_id),
                image.clone(),
                egui::TextureOptions::LINEAR,
            );
            let width = ui.available_width().min(160.0);
            let scale = width / texture.size_vec2().x;
            ui.add(egui::Image::new(&texture).fit_to_exact_size(texture.size_vec2() * scale));
        }
    }
}

fn the_card(ui: &mut egui::Ui, (wants, may_remember): (&str, bool), lang: Lang) -> Option<Allowed> {
    let say = |message: Message| message.say(lang);
    let mut answered = None;
    ui.add_space(GAP);
    egui::Frame::group(ui.style())
        .fill(ui.visuals().faint_bg_color)
        .stroke(egui::Stroke::new(1.0, ui.visuals().warn_fg_color))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.weak(egui::RichText::new(say(Message::AiWantsTo)).small());
            ui.add(egui::Label::new(wants).selectable(true).wrap());
            ui.add_space(GAP * 0.5);
            ui.horizontal_wrapped(|ui| {
                if ui.button(say(Message::AiAllowOnce)).clicked() {
                    answered = Some(Allowed::Once);
                }
                if may_remember && ui.button(say(Message::AiAllowForThisChat)).clicked() {
                    answered = Some(Allowed::ForThisChat);
                }
                if ui.button(say(Message::AiRefuse)).clicked() {
                    answered = Some(Allowed::Refuse);
                }
            });
        });
    answered
}

fn a_note(ui: &mut egui::Ui, said: &str) {
    ui.add_space(GAP * 0.5);
    ui.add(
        egui::Label::new(egui::RichText::new(said).small().italics())
            .selectable(true)
            .wrap(),
    );
}

fn one_line(text: &str) -> String {
    let first = text.lines().next().unwrap_or_default();
    if first.chars().count() <= 80 {
        first.to_owned()
    } else {
        let mut clipped: String = first.chars().take(80).collect();
        clipped.push('\u{2026}');
        clipped
    }
}

const fn effort_said(effort: Effort) -> Message {
    match effort {
        Effort::Off => Message::AiEffortOff,
        Effort::Low => Message::AiEffortLow,
        Effort::Medium => Message::AiEffortMedium,
        Effort::High => Message::AiEffortHigh,
    }
}

const fn mode_said(mode: Mode) -> Message {
    match mode {
        Mode::ChatOnly => Message::AiModeChatOnly,
        Mode::AskBeforeChanges => Message::AiModeAskBeforeChanges,
        Mode::DoIt => Message::AiModeDoIt,
    }
}

const fn mode_means(mode: Mode) -> Message {
    match mode {
        Mode::ChatOnly => Message::AiModeChatOnlyWhat,
        Mode::AskBeforeChanges => Message::AiModeAskBeforeChangesWhat,
        Mode::DoIt => Message::AiModeDoItWhat,
    }
}

fn server_path() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|program| program.parent().map(|beside| beside.join("panpdf-mcp")))
        .map_or_else(
            || "panpdf-mcp".to_owned(),
            |path| path.display().to_string(),
        )
}

fn claude_code_line(server: &str) -> String {
    format!("claude mcp add panpdf -- {server}")
}

fn client_configuration(server: &str) -> String {
    let quoted: String = server
        .chars()
        .flat_map(|letter| match letter {
            '\\' => vec!['\\', '\\'],
            '"' => vec!['\\', '"'],
            other => vec![other],
        })
        .collect();
    format!("{{\n  \"mcpServers\": {{\n    \"panpdf\": {{ \"command\": \"{quoted}\" }}\n  }}\n}}")
}

impl Window {
    pub(crate) fn open_the_agents_window(&mut self) {
        self.agents_open = true;
    }

    pub(crate) fn agents_window(&mut self, ctx: &egui::Context) {
        if !self.agents_open {
            return;
        }
        let lang = self.lang;
        let say = |message: Message| message.say(lang);
        let server = server_path();
        let mut open = self.agents_open;
        egui::Window::new(say(Message::AgentsTitle))
            .open(&mut open)
            .resizable(true)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.label(say(Message::AgentsWhat));
                for (heading, snippet) in [
                    (Message::AgentsForClaudeCode, claude_code_line(&server)),
                    (Message::AgentsForClients, client_configuration(&server)),
                ] {
                    ui.separator();
                    ui.strong(say(heading));
                    ui.add(
                        egui::TextEdit::multiline(&mut snippet.as_str())
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY),
                    );
                    if ui.button(say(Message::DraftCopy)).clicked() {
                        ui.ctx().copy_text(snippet.clone());
                    }
                }
                ui.separator();
                ui.small(say(Message::AgentsNote));
            });
        self.agents_open = open;
    }
}

fn current_page_text(window: &Window, limit: usize) -> String {
    let mut text = String::new();
    if let Some(leaf) = window.editor.leaf(window.focus) {
        for cluster in &leaf.overlay.clusters {
            if let Some(value) = &cluster.text {
                text.push_str(value);
            }
        }
    }
    text.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use eframe::egui;

    use pdf_agent::connect::{ConnectError, Reply, Said, ToolResult};

    use super::{
        AiState, Answer, Asking, MOST_ROUNDS, Notice, Provider, Turn, claude_code_line,
        client_configuration, notice_of,
    };
    use pdf_app::ai_layout;
    use pdf_app::wording::{Lang, Message};

    fn said(text: &str) -> Reply {
        Reply {
            text: text.to_owned(),
            ..Reply::default()
        }
    }

    #[test]
    fn a_new_panel_holds_no_key_and_no_model() {
        let state = AiState::default();
        assert!(state.key.is_empty());
        assert!(state.model.is_empty());
        assert!(!state.connected);
        assert!(!state.busy());
        assert_eq!(state.base_url, Provider::OpenAi.base_url());
    }

    #[test]
    fn disconnecting_forgets_the_key_and_stops_the_question() {
        let (_send, answers) = mpsc::channel::<Answer>();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut state = AiState {
            key: "a-secret".to_owned(),
            connected: true,
            asking: Some(Asking {
                stop: std::sync::Arc::clone(&stop),
                answers,
            }),
            ..AiState::default()
        };
        state.disconnect();
        assert!(state.key.is_empty());
        assert!(!state.connected);
        assert!(!state.busy());
        assert!(
            stop.load(std::sync::atomic::Ordering::Relaxed),
            "the worker is told to stop, not left asking"
        );
    }

    #[test]
    fn an_answer_from_the_old_connection_is_not_shown() {
        let (send, answers) = mpsc::channel();
        let mut state = AiState {
            asking: Some(Asking {
                stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                answers,
            }),
            ..AiState::default()
        };
        let asked_under = state.generation;
        state.invalidate();
        assert!(state.generation > asked_under);
        send.send(Answer::Chat(asked_under, Ok(said("stale"))))
            .expect("the worker answers");
        let context = egui::Context::default();
        state.poll(&context);
        assert!(
            state.turns.is_empty(),
            "an answer to a question about another connection is not an answer to this one"
        );
        assert!(!state.busy());
    }

    #[test]
    fn an_answer_becomes_the_model_s_turn() {
        let (send, answers) = mpsc::channel();
        let mut state = AiState {
            settings_open: true,
            asking: Some(Asking {
                stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                answers,
            }),
            ..AiState::default()
        };
        state.turns.push(Turn::person("what is this page about?"));
        send.send(Answer::Chat(state.generation, Ok(said("a map"))))
            .expect("the worker answers");
        state.poll(&egui::Context::default());
        assert_eq!(state.turns.len(), 2);
        assert_eq!(state.turns[1], Turn::model("a map"));
        assert!(!state.settings_open, "an answer settles the connection");
        assert!(state.connected);
    }

    #[test]
    fn a_refused_question_is_given_back_to_be_asked_again() {
        let (send, answers) = mpsc::channel();
        let mut state = AiState {
            asking: Some(Asking {
                stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                answers,
            }),
            ..AiState::default()
        };
        state.turns.push(Turn::person("summarise this"));
        send.send(Answer::Chat(
            state.generation,
            Err(ConnectError::Http("401 Unauthorized".to_owned())),
        ))
        .expect("the worker answers");
        state.poll(&egui::Context::default());
        assert_eq!(state.composer, "summarise this");
        assert!(state.turns.is_empty(), "nothing was said after all");
        assert_eq!(
            state.notice,
            Some(Notice {
                said: Message::AiServiceRefused,
                detail: Some("401 Unauthorized".to_owned()),
            })
        );
    }

    #[test]
    fn a_new_chat_forgets_the_conversation() {
        let mut state = AiState::default();
        state.turns.push(Turn::person("one"));
        state.turns.push(Turn::model("two"));
        state.notice = Some(Notice::plain(Message::AiStopped));
        let before = state.generation;
        state.new_chat();
        assert!(state.turns.is_empty());
        assert!(state.notice.is_none());
        assert!(state.generation > before, "an answer on its way is dropped");
    }

    #[test]
    fn a_provider_s_name_reads_back_as_itself() {
        for provider in Provider::ALL {
            assert_eq!(Provider::by_name(provider.name()), Some(provider));
        }
        assert_eq!(Provider::by_name("Something else"), None);
    }

    #[test]
    fn only_the_hosted_providers_ask_for_a_key() {
        let asks = |provider| {
            AiState {
                provider,
                ..AiState::default()
            }
            .key_missing()
        };
        assert!(asks(Provider::OpenAi) && asks(Provider::Claude) && asks(Provider::Gemini));
        assert!(!asks(Provider::Ollama) && !asks(Provider::LmStudio));
    }

    #[test]
    fn a_worker_that_stopped_without_answering_is_said_so() {
        let (send, answers) = mpsc::channel::<Answer>();
        let mut state = AiState {
            asking: Some(Asking {
                stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                answers,
            }),
            ..AiState::default()
        };
        drop(send);
        let context = egui::Context::default();
        state.poll(&context);
        assert!(!state.busy());
        assert_eq!(state.notice, Some(Notice::plain(Message::AiWorkerStopped)));
        assert!(!Message::AiWorkerStopped.say(Lang::English).is_empty());
    }

    #[test]
    fn every_way_a_connection_fails_is_said_differently() {
        let every = [
            ConnectError::Invalid("no model".to_owned()),
            ConnectError::Cancelled,
            ConnectError::Curl("could not connect to host".to_owned()),
            ConnectError::ResponseTooLarge,
            ConnectError::Http("401 Unauthorized".to_owned()),
            ConnectError::Protocol("no choices in the answer".to_owned()),
        ];
        let notices: Vec<Notice> = every.iter().map(notice_of).collect();
        for (at, notice) in notices.iter().enumerate() {
            for (also, other) in notices.iter().enumerate() {
                assert!(
                    at == also || notice.said != other.said,
                    "{:?} and {:?} are said the same way",
                    every[at],
                    every[also]
                );
            }
            for lang in Lang::ALL {
                assert!(!notice.said.say(*lang).trim().is_empty());
            }
        }
        assert_eq!(
            notices[4],
            Notice {
                said: Message::AiServiceRefused,
                detail: Some("401 Unauthorized".to_owned()),
            }
        );
        assert_eq!(notices[1], Notice::plain(Message::AiStopped));
        assert_eq!(notices[3], Notice::plain(Message::AiAnswerTooLarge));
        assert_eq!(
            notices[2].detail.as_deref(),
            Some("could not connect to host"),
            "the provider's own words are kept, not translated away"
        );
        assert_eq!(notices[0].detail.as_deref(), Some("no model"));
        assert_eq!(
            notices[5].detail.as_deref(),
            Some("no choices in the answer")
        );
    }

    #[test]
    fn enter_alone_sends_and_enter_with_a_modifier_does_not() {
        fn frame_with(modifiers: egui::Modifiers) -> egui::RawInput {
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            }
        }
        fn asked_in(raw: egui::RawInput, take: fn(&mut egui::Ui) -> bool) -> bool {
            let mut asked = false;
            let context = egui::Context::default();
            let _ = context.run_ui(raw, |ui| asked = take(ui));
            asked
        }
        let plain = |ui: &mut egui::Ui| super::plain_enter(ui);
        assert!(asked_in(frame_with(egui::Modifiers::NONE), plain));
        assert!(!asked_in(frame_with(egui::Modifiers::SHIFT), plain));
        assert!(!asked_in(frame_with(egui::Modifiers::CTRL), plain));
        assert!(!asked_in(frame_with(egui::Modifiers::ALT), plain));
        assert!(!asked_in(egui::RawInput::default(), plain));
        let logically = |ui: &mut egui::Ui| {
            ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
        };
        assert!(asked_in(frame_with(egui::Modifiers::SHIFT), logically));
    }

    #[test]
    fn an_empty_box_asks_for_the_two_rows_it_starts_with() {
        let state = AiState::default();
        assert_eq!(state.wrapped_rows, ai_layout::LEAST_ROWS);
        assert_eq!(
            ai_layout::rows_shown(state.wrapped_rows, ai_layout::most_rows(600.0, 18.0, 44.0)),
            2
        );
    }

    #[test]
    fn what_the_tools_answered_goes_back_as_one_turn() {
        let mut state = AiState::default();
        state.turns.push(Turn::person("change the heading"));
        assert!(state.round_done(vec![
            ToolResult::said("call_1", "Done. p2-b3 now reads: X"),
            ToolResult::failed("call_2", "there is no page 40"),
        ]));
        assert_eq!(state.turns.len(), 2);
        let last = state.turns.last().expect("the results are a turn");
        assert_eq!(last.said(), Said::Person);
        assert_eq!(last.text(), "");
        assert!(last.calls().is_empty());
        assert_eq!(state.tools.rounds, 1);
    }

    #[test]
    fn the_sixteenth_round_is_the_last() {
        let mut state = AiState::default();
        state.turns.push(Turn::person("tidy the whole document"));
        for round in 1..=MOST_ROUNDS {
            assert!(
                state.round_done(vec![ToolResult::said("call_1", "Done.")]),
                "round {round} should have been performed"
            );
            assert_eq!(state.tools.rounds, round);
        }
        assert!(
            !state.round_done(vec![ToolResult::said("call_1", "Done.")]),
            "the seventeenth round is refused"
        );
        state.stop_for_too_many_rounds();
        assert_eq!(state.notice, Some(Notice::plain(Message::AiTooManyRounds)));
        assert_eq!(
            state.tools.rounds, 0,
            "the count starts again with the next question"
        );
        let english = Message::AiTooManyRounds.say(Lang::English);
        assert!(!english.is_empty());
    }

    #[test]
    fn a_new_chat_forgets_what_was_allowed() {
        let mut state = AiState::default();
        state.turns.push(Turn::person("change page 2"));
        state
            .tools
            .allowed_for_chat
            .insert("replace_text".to_owned());
        state
            .notes
            .push((0, Message::AiChangedTheDocument { page: 2 }));
        state.new_chat();
        assert!(state.turns.is_empty());
        assert!(state.notes.is_empty());
        assert!(state.tools.allowed_for_chat.is_empty());
        assert!(!state.tools.busy());
    }

    #[test]
    fn the_snippets_carry_the_servers_own_path() {
        assert_eq!(
            claude_code_line("/opt/panpdf/panpdf-mcp"),
            "claude mcp add panpdf -- /opt/panpdf/panpdf-mcp"
        );
        let windows = client_configuration("C:\\Program Files\\PanPDF\\panpdf-mcp.exe");
        assert!(
            windows.contains("\"command\": \"C:\\\\Program Files\\\\PanPDF\\\\panpdf-mcp.exe\""),
            "{windows}"
        );
        assert!(windows.starts_with('{') && windows.trim_end().ends_with('}'));
    }
}
