use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{
    Arc,
    mpsc::{self, Receiver, TryRecvError},
};
use std::thread;

use eframe::egui;
use pdf_agent::attach;
use pdf_agent::connect::{
    Attachment, ConnectError, Connection, Effort, Model, Provider as WireProvider, Reply, Said,
    ToolResult, Turn, settle_dangling_calls,
};
use pdf_agent::tools::DocumentBrief;

use pdf_app::ai_layout;
use pdf_app::ai_permission::{Answer as Allowed, Mode, describe_call};
use pdf_app::wording::{Lang, Message};

use crate::ai_actions::{MOST_ROUNDS, Question, QuestionReply, Tools, tools_are_offered};
use crate::icons::Icon;
use crate::window_state::Window;

mod drawers;
mod going_back;
mod history;
mod keeping;

const MAX_CONTEXT: usize = 12_000;

const GAP: f32 = 8.0;

const LEAST_CONVERSATION: f32 = 80.0;

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

fn shorten_model(id: &str) -> String {
    let last = id.rsplit('/').next().unwrap_or(id);
    if last.chars().count() <= 28 {
        return last.to_owned();
    }
    let mut clipped: String = last.chars().take(27).collect();
    clipped.push('\u{2026}');
    clipped
}

fn small_button_width(ui: &egui::Ui, text: &str) -> f32 {
    let font = egui::TextStyle::Small.resolve(ui.style());
    let colour = ui.visuals().text_color();
    let letters = ui
        .ctx()
        .fonts_mut(|fonts| fonts.layout_no_wrap(text.to_owned(), font, colour).size().x);
    letters + ui.spacing().button_padding.x * 2.0
}

fn control_height(ui: &egui::Ui) -> f32 {
    ui.spacing()
        .interact_size
        .y
        .max(crate::format::CONTROL_HEIGHT)
}

#[derive(Clone, Copy, Debug, Default)]
struct Asked {
    asked: bool,
    attach: bool,
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
    crate::own_folder::own_file("ai")
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Arriving {
    said: String,
    thinking: String,
}

impl Arriving {
    fn is_empty(&self) -> bool {
        self.said.is_empty() && self.thinking.is_empty()
    }
}

enum Answer {
    Models(u64, Result<Vec<Model>, ConnectError>),
    Partial(u64, Arriving),
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

struct PendingAttachment {
    name: String,
    bytes: usize,
    kind: attach::Kind,
    outcome: Result<Vec<Attachment>, String>,
}

impl PendingAttachment {
    fn label(&self, lang: Lang) -> String {
        if self.outcome.is_err() {
            return Message::AiAttachRefused.say(lang);
        }
        match self.kind {
            attach::Kind::Picture => Message::AiAttachPicture,
            attach::Kind::Pdf => Message::AiAttachPdf,
            attach::Kind::Office | attach::Kind::Text => Message::AiAttachText,
            attach::Kind::Unsupported => Message::AiAttachRefused,
        }
        .say(lang)
    }
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "four independent things a person can turn on: the panel, its \
              settings, whether a provider answered, and whether the page \
              goes with the question"
)]
pub(crate) struct AiState {
    pub(crate) open: bool,
    pub(crate) width: f32,
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
    partial: Option<Arriving>,
    wrapped_rows: usize,
    chips_height: f32,
    second_row_height: f32,
    drawn: Vec<Option<(TurnShape, f32)>>,
    drawn_width: f32,
    effort: Effort,
    pub(crate) mode: Mode,
    pub(crate) tools: Tools,
    context: String,
    brief: DocumentBrief,
    notes: Vec<(usize, Message)>,
    remember_key: bool,
    chat_id: String,
    saved_turns: usize,
    documents: Vec<String>,
    places: Vec<String>,
    confirming_free: bool,
    history: Option<Vec<pdf_agent::history::Chat>>,
    pending: Vec<PendingAttachment>,
    recall: pdf_app::ai_recall::Recall,
    rewound: Option<going_back::Rewound>,
    send_now: bool,
    tried_at_start: bool,
    checking: bool,
    confirming_delete: bool,
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
            width: 360.0,
            connected: false,
            include_context: false,
            generation: 0,
            asking: None,
            partial: None,
            wrapped_rows: ai_layout::LEAST_ROWS,
            chips_height: 0.0,
            second_row_height: 0.0,
            drawn: Vec::new(),
            drawn_width: 0.0,
            effort: Effort::Off,
            mode: Mode::default(),
            tools: Tools::default(),
            context: String::new(),
            brief: DocumentBrief::default(),
            notes: Vec::new(),
            remember_key: false,
            chat_id: String::new(),
            saved_turns: 0,
            documents: Vec::new(),
            places: Vec::new(),
            confirming_free: false,
            history: None,
            pending: Vec::new(),
            recall: pdf_app::ai_recall::Recall::default(),
            rewound: None,
            send_now: false,
            tried_at_start: false,
            checking: false,
            confirming_delete: false,
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
        state.take_the_kept_key();
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

    fn may_send(&self) -> bool {
        !self.composer.trim().is_empty() || self.pending.iter().any(|item| item.outcome.is_ok())
    }

    pub(crate) fn attach_file(&mut self, path: &std::path::Path) {
        let name = path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.pending.push(PendingAttachment {
                    name,
                    bytes: 0,
                    kind: attach::Kind::Unsupported,
                    outcome: Err(error.to_string()),
                });
                return;
            }
        };
        let so_far: Vec<(String, usize)> = self
            .pending
            .iter()
            .filter(|item| item.outcome.is_ok())
            .map(|item| (item.name.clone(), item.bytes))
            .collect();
        let kind = attach::kind_of(&bytes);
        let outcome = attach::fits(&so_far, (&name, bytes.len()))
            .and_then(|()| attach::prepare(&name, &bytes))
            .map_err(|error| error.to_string());
        self.pending.push(PendingAttachment {
            name,
            bytes: bytes.len(),
            kind,
            outcome,
        });
    }

    pub(crate) fn remove_pending(&mut self, at: usize) {
        if at < self.pending.len() {
            self.pending.remove(at);
        }
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
        self.remember_key = false;
        self.keep_the_key();
        self.invalidate();
    }
    fn cancel(&mut self) {
        if let Some(asking) = self.asking.take() {
            asking.stop.store(true, Ordering::Relaxed);
        }
        self.checking = false;
        if let Some(arrived) = self.partial.take()
            && !arrived.said.trim().is_empty()
        {
            self.turns.push(Turn::model(arrived.said));
        }
        self.generation += 1;
    }
    fn poll(&mut self, ctx: &egui::Context) {
        let Some(asking) = self.asking.as_ref() else {
            return;
        };
        let mut result = None;
        let mut fresh = false;
        loop {
            match asking.answers.try_recv() {
                Ok(Answer::Partial(generation, said)) => {
                    if generation == self.generation {
                        self.partial = Some(said);
                        fresh = true;
                    }
                }
                Ok(answer) => {
                    result = Some(answer);
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.asking = None;
                    self.checking = false;
                    self.partial = None;
                    self.notice = Some(Notice::plain(Message::AiWorkerStopped));
                    return;
                }
            }
        }
        let Some(answer) = result else {
            if fresh {
                ctx.request_repaint();
            }
            return;
        };
        self.asking = None;
        self.checking = false;
        self.asked = None;
        self.partial = None;
        let generation = match &answer {
            Answer::Models(g, _) | Answer::Partial(g, _) | Answer::Chat(g, _) => *g,
        };
        if generation != self.generation {
            return;
        }
        match answer {
            Answer::Partial(..) => {}
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
                        if reply.cut_short {
                            self.notes
                                .push((self.turns.len(), Message::AiAnswerCutShort));
                        }
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
        self.checking = true;
        let repaint = ctx.clone();
        thread::spawn(move || {
            let result = connection.models(&worker_flag);
            let _ = tx.send(Answer::Models(generation, result));
            repaint.request_repaint();
        });
    }
    fn start_answer(&mut self, ctx: &egui::Context, context: String, brief: &DocumentBrief) {
        if self.checking {
            self.cancel();
        }
        if self.busy() || !self.may_send() {
            return;
        }
        settle_dangling_calls(&mut self.turns);
        self.tools.drop_the_queue();
        self.rewound = None;
        self.recall.forget();
        let asked = std::mem::take(&mut self.composer);
        let attachments: Vec<Attachment> = std::mem::take(&mut self.pending)
            .into_iter()
            .filter_map(|item| item.outcome.ok())
            .flatten()
            .collect();
        self.turns
            .push(Turn::person_with(asked.trim(), attachments));
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
        let turns = pdf_agent::connect::without_the_old_pictures(&self.turns);
        let context = self.context.clone();
        let (tools, system) = if tools_are_offered(self.mode) {
            (
                pdf_agent::tools::offered_to_a_window(),
                Some(
                    pdf_agent::tools::window_instructions(&self.brief)
                        + &self.what_changed_hands().unwrap_or_default(),
                ),
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
            let told = tx.clone();
            let told_repaint = repaint.clone();
            let result = connection.converse_streaming(
                &turns,
                (!context.is_empty()).then_some(context.as_str()),
                &tools,
                system.as_deref(),
                &worker_flag,
                &mut |far| {
                    let arrived = Arriving {
                        said: far.said.to_owned(),
                        thinking: far.thinking.to_owned(),
                    };
                    if told.send(Answer::Partial(generation, arrived)).is_ok() {
                        told_repaint.request_repaint();
                    }
                },
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
        self.partial = None;
        self.pending.clear();
        self.rewound = None;
        self.recall.forget();
        self.confirming_delete = false;
        self.chat_id.clear();
        self.saved_turns = 0;
        let here = self.documents.pop();
        self.documents.clear();
        self.documents.extend(here);
        let place = self.places.pop();
        self.places.clear();
        self.places.extend(place);
    }

    pub(crate) fn document_arrived(&mut self, name: String, place: String) {
        if !place.is_empty() && self.places.last() == Some(&place) {
            return;
        }
        if !place.is_empty() && self.places.contains(&place) {
            self.moved_to(name, place);
            return;
        }
        self.save_the_chat();
        let found = pdf_agent::history::chat_about(self.the_chats(), &place).cloned();
        if let Some(chat) = found {
            self.take_up(&chat);
            self.notes
                .push((self.turns.len(), Message::AiThisDocumentsChat));
            self.moved_to(name, place);
        } else {
            if !self.turns.is_empty() || self.working() {
                self.new_chat();
            }
            self.documents = vec![name];
            self.places = if place.is_empty() {
                Vec::new()
            } else {
                vec![place]
            };
        }
    }

    fn moved_to(&mut self, name: String, place: String) {
        if !place.is_empty() && !self.places.contains(&place) {
            self.places.push(place);
            self.saved_turns = usize::MAX;
        }
        if self.documents.last() == Some(&name) {
            return;
        }
        if self.turns.is_empty() {
            self.documents.clear();
        } else {
            self.notes
                .push((self.turns.len(), Message::AiNowOnDocument(name.clone())));
            self.saved_turns = usize::MAX;
        }
        self.documents.push(name);
    }

    pub(crate) fn saved_as(&mut self, place: String) {
        if place.is_empty() || self.places.contains(&place) {
            return;
        }
        self.places.push(place);
        self.saved_turns = usize::MAX;
    }

    fn what_changed_hands(&self) -> Option<String> {
        if self.documents.len() < 2 {
            return None;
        }
        Some(format!(
            "\n\nThis conversation has moved between documents: {}. Only the last is open now. \
             Block names, page numbers and text quoted earlier belong to whichever document was \
             open when they were said: read the open document again before you change it.",
            self.documents.join(", then "),
        ))
    }

    fn working(&self) -> bool {
        (self.busy() && !self.checking) || self.tools.busy()
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

    fn the_heading(&mut self, ui: &mut egui::Ui, lang: Lang) -> bool {
        let say = |message: Message| message.say(lang);
        let mut close = false;
        ui.horizontal(|ui| {
            self.the_history(ui, lang);
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
                    !self.turns.is_empty(),
                )
                .clicked()
                {
                    self.new_chat();
                }
                self.the_chat_menu(ui, lang);
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
        });
        close
    }

    fn the_chat_menu(&mut self, ui: &mut egui::Ui, lang: Lang) {
        let say = |message: Message| message.say(lang);
        let button = crate::format::icon_button(
            ui,
            crate::icons::Icon::More,
            &say(Message::AiChatMenu),
            false,
            !self.turns.is_empty(),
        );
        let mut delete = false;
        let shown = egui::Popup::menu(&button)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.set_min_width(220.0);
                if ui.button(say(Message::AiCopyWholeChat)).clicked() {
                    ui.ctx().copy_text(self.the_whole_chat(lang));
                    ui.close();
                }
                let (words, colour) = if self.confirming_delete {
                    (
                        say(Message::AiDeleteThisChatSure),
                        ui.visuals().error_fg_color,
                    )
                } else {
                    (say(Message::AiForgetChat), ui.visuals().text_color())
                };
                if ui
                    .add_enabled(
                        !self.working(),
                        egui::Button::new(egui::RichText::new(words).color(colour)),
                    )
                    .clicked()
                {
                    if self.confirming_delete {
                        delete = true;
                        ui.close();
                    } else {
                        self.confirming_delete = true;
                    }
                }
            });
        if shown.is_none() {
            self.confirming_delete = false;
        }
        if delete {
            let id = self.chat_id.clone();
            if !id.is_empty() {
                self.forget_a_chat(&id);
            }
            self.new_chat();
        }
    }

    fn who_answers(&mut self, ui: &mut egui::Ui, lang: Lang) {
        self.whether_it_is_connected(ui, lang);
    }

    fn whether_it_is_connected(&mut self, ui: &mut egui::Ui, lang: Lang) {
        if self.settings_open {
            return;
        }
        let ready = self.connected && !self.model.is_empty() && !self.key_missing();
        ui.horizontal(|ui| {
            if ready {
                ui.label(
                    egui::RichText::new(Message::AiConnectedTo.say(lang))
                        .small()
                        .color(egui::Color32::from_rgb(0x1f, 0x7a, 0x34)),
                );
                ui.weak(egui::RichText::new(&self.model).small());
            } else if self.checking {
                slow_spinner(ui);
                ui.weak(egui::RichText::new(Message::AiCheckingConnection.say(lang)).small());
            } else {
                let missing = if self.key_missing() {
                    Message::AiNotConnected
                } else if self.model.is_empty() {
                    Message::AiNoModelChosen
                } else {
                    Message::AiNotConnectedYet
                };
                if ui
                    .add(
                        egui::Label::new(
                            egui::RichText::new(missing.say(lang))
                                .small()
                                .color(ui.visuals().warn_fg_color),
                        )
                        .sense(egui::Sense::click()),
                    )
                    .clicked()
                {
                    self.settings_open = true;
                }
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
                            self.keep_the_key();
                        }
                        ui.end_row();
                    });
                ui.add_space(GAP * 0.5);
                if ui
                    .checkbox(&mut self.remember_key, say(Message::AiKeepTheKey))
                    .on_hover_text(say(Message::AiKeepTheKeyMeans))
                    .changed()
                {
                    self.keep_the_key();
                }
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

    fn try_the_kept_connection(&mut self, ctx: &egui::Context) {
        if std::mem::replace(&mut self.tried_at_start, true) || self.model.is_empty() {
            return;
        }
        self.models_if_possible(ctx);
    }

    fn models_if_possible(&mut self, ctx: &egui::Context) {
        if self.busy() || self.base_url.is_empty() || self.key_missing() {
            return;
        }
        self.start_models(ctx);
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one conversation, drawn top to bottom: turns, notes, cards, status"
    )]
    fn the_conversation(&mut self, ui: &mut egui::Ui, lang: Lang) -> Option<Allowed> {
        let say = |message: Message| message.say(lang);
        let card = self.tools.ask.as_ref().map(|pending| {
            (
                describe_call(&pending.request, lang),
                pending.may_allow_for_chat,
            )
        });
        let mut answered = None;
        let idle = !self.working();
        let last_answer = self.last_question().and_then(|_| {
            self.turns.iter().rposition(
                |turn| matches!(turn, Turn::Model { text, .. } if !text.trim().is_empty()),
            )
        });
        let mut action = None;
        let mut put_back = false;
        let mut replied = None;
        let names: BTreeMap<String, String> = self
            .turns
            .iter()
            .filter_map(|turn| match turn {
                Turn::Model { calls, .. } => Some(calls),
                _ => None,
            })
            .flatten()
            .map(|call| (call.id.clone(), call.name.clone()))
            .collect();
        let document_stays = self
            .rewound
            .as_ref()
            .map(going_back::Rewound::changed_the_document);
        let width = ui.available_width();
        if (width - self.drawn_width).abs() > 0.5 {
            self.drawn.clear();
            self.drawn_width = width;
        }
        self.drawn.resize(self.turns.len(), None);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show_viewport(ui, |ui, viewport| {
                let origin = ui.cursor().top();
                let seen = viewport.expand2(egui::vec2(0.0, viewport.height()));
                if self.turns.is_empty() && document_stays.is_none() {
                    ui.add_space(GAP);
                    ui.weak(say(Message::AiNothingAskedYet));
                }
                for (at, turn) in self.turns.iter().enumerate() {
                    let shape = TurnShape::of(turn, at, &self.notes);
                    let top = ui.cursor().top();
                    if let Some((_, height)) = self.drawn[at].filter(|(was, _)| *was == shape)
                        && !seen
                            .y_range()
                            .intersects(egui::Rangef::new(top - origin, top - origin + height))
                    {
                        ui.add_space(height);
                        continue;
                    }
                    for (_, said) in self.notes.iter().filter(|(index, _)| *index == at) {
                        a_note(ui, &said.say(lang));
                    }
                    match turn {
                        Turn::Results { results } => {
                            what_the_tools_did(ui, results, (&self.tools, &names), lang);
                        }
                        Turn::Model { text, .. } if text.trim().is_empty() => {}
                        _ => {
                            let offers = Offers {
                                edit: idle && turn.said() == Said::Person,
                                ask_again: idle && Some(at) == last_answer,
                            };
                            let salt = format!("turn-{at}");
                            if let Some(chosen) =
                                said_by(ui, turn, (&self.model, lang), &salt, offers)
                            {
                                action = Some((at, chosen));
                            }
                        }
                    }
                    self.drawn[at] = Some((shape, ui.cursor().top() - top));
                }
                for (_, said) in self
                    .notes
                    .iter()
                    .filter(|(index, _)| *index >= self.turns.len())
                {
                    a_note(ui, &said.say(lang));
                }
                if let Some(document_stays) = document_stays
                    && idle
                {
                    put_back = went_back(ui, document_stays, lang);
                }
                if let Some((wants, may_remember)) = card {
                    answered = the_card(ui, (&wants, may_remember), lang);
                }
                if let Some(question) = self.tools.question.as_mut() {
                    replied = the_question(ui, question, lang);
                }
                if let Some(arrived) = self.partial.as_ref().filter(|far| !far.is_empty()) {
                    if !arrived.thinking.is_empty() {
                        thinking_so_far(ui, &arrived.thinking, lang);
                    }
                    if !arrived.said.is_empty() {
                        said_by(
                            ui,
                            &Turn::model(arrived.said.clone()),
                            (&self.model, lang),
                            "arriving",
                            Offers::default(),
                        );
                    }
                }
                if let Some((icon, doing)) = self.what_is_happening(lang) {
                    ui.add_space(GAP);
                    ui.horizontal(|ui| {
                        slow_spinner(ui);
                        if let Some(icon) = icon {
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                            icon.draw(ui.painter(), rect, ui.visuals().text_color());
                        }
                        ui.weak(doing);
                    });
                }
            });
        if put_back {
            self.put_back();
        }
        if let Some(reply) = replied {
            self.tools.answer_the_question(reply);
        }
        match action {
            Some((at, TurnAction::Edit)) => self.go_back_to(at, false),
            Some((_, TurnAction::AskAgain)) => {
                if let Some(at) = self.last_question() {
                    self.go_back_to(at, true);
                }
            }
            None => {}
        }
        answered
    }

    fn what_is_happening(&self, lang: Lang) -> Option<(Option<Icon>, String)> {
        use crate::ai_actions::Doing;
        match self.tools.doing() {
            Some(Doing::Writing { written, pieces }) => Some((
                Some(tool_icon("write_pages")),
                Message::AiWritingPieces { written, pieces }.say(lang),
            )),
            Some(Doing::Tool(name, request)) => Some((
                Some(tool_icon(&name)),
                format!("{}\u{2026}", pdf_app::ai_status::doing(&request, lang)),
            )),
            None if self.busy() && !self.checking => {
                let arrived = self.partial.as_ref();
                let said = if arrived.is_some_and(|far| !far.said.trim().is_empty()) {
                    Message::AiWritingTheAnswer
                } else if arrived.is_some_and(|far| !far.thinking.trim().is_empty())
                    || self.model.is_empty()
                {
                    Message::AiThinking
                } else {
                    Message::AiWaitingForModel(shorten_model(&self.model))
                };
                Some((None, said.say(lang)))
            }
            Some(Doing::Asking | Doing::Allowing) | None => None,
        }
    }

    fn pending_chips(&mut self, ui: &mut egui::Ui, lang: Lang) {
        if self.pending.is_empty() {
            self.chips_height = 0.0;
            return;
        }
        let mut remove = None;
        let row = ui.horizontal_wrapped(|ui| {
            for (at, item) in self.pending.iter().enumerate() {
                let refused = item.outcome.is_err();
                let chip = egui::Frame::group(ui.style())
                    .fill(if refused {
                        ui.visuals().error_fg_color.gamma_multiply(0.12)
                    } else {
                        ui.visuals().faint_bg_color
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let text = egui::RichText::new(&item.name).small();
                            ui.label(if refused {
                                text.color(ui.visuals().error_fg_color)
                            } else {
                                text
                            });
                            ui.weak(egui::RichText::new(item.label(lang)).small());
                            if crate::format::icon_button(
                                ui,
                                crate::icons::Icon::Close,
                                &Message::Close.say(lang),
                                false,
                                true,
                            )
                            .clicked()
                            {
                                remove = Some(at);
                            }
                        });
                    });
                chip.response.on_hover_text(match &item.outcome {
                    Ok(_) => item.label(lang),
                    Err(why) => why.clone(),
                });
            }
        });
        self.chips_height = row.response.rect.height() + ui.spacing().item_spacing.y;
        if let Some(at) = remove {
            self.remove_pending(at);
        }
    }

    fn recall_keys(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> bool {
        use pdf_app::ai_recall::Way;
        if !ui.memory(|memory| memory.has_focus(composer_id())) {
            return false;
        }
        for (key, way) in [
            (egui::Key::ArrowUp, Way::Up),
            (egui::Key::ArrowDown, Way::Down),
        ] {
            if !ui.input(|input| input.key_pressed(key) && input.modifiers.is_none()) {
                continue;
            }
            let caret_at_start = egui::TextEdit::load_state(ctx, composer_id())
                .and_then(|state| state.cursor.char_range())
                .is_some_and(|range| range.primary.index.0 == 0 && range.secondary.index.0 == 0);
            if self.recall_key(way, caret_at_start) {
                ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, key));
                return true;
            }
        }
        false
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one box and the one row of controls under it"
    )]
    fn the_composer(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        lang: Lang,
        (rows, row_height): (usize, f32),
    ) -> Asked {
        let say = |message: Message| message.say(lang);
        let mut asked = false;
        let mut attach = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            self.pending_chips(ui, lang);
            let recalled = self.recall_keys(ui, ctx);
            let mut typed = egui::ScrollArea::vertical()
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
            if recalled {
                let end = self.composer.chars().count();
                typed
                    .state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::one(
                        egui::text::CCursor::new(end),
                    )));
                typed.state.store(ui.ctx(), typed.response.id);
            }
            if typed.response.has_focus() && plain_enter(ui) && self.may_send() {
                asked = true;
            }
            if std::mem::take(&mut self.send_now) && self.may_send() {
                asked = true;
            }
            let one_row = ai_layout::fits_on_one_row(
                ui.available_width(),
                &[
                    crate::format::CONTROL_HEIGHT,
                    small_button_width(ui, &self.mode_label(lang)),
                    small_button_width(ui, &self.model_label(lang)),
                    crate::format::CONTROL_HEIGHT,
                ],
                GAP * 2.0,
            );
            let mut left = |ui: &mut egui::Ui, this: &mut Self| {
                if this.the_attach_button(ui, lang) {
                    attach = true;
                }
                this.what_it_may_do(ui, lang);
            };
            let mut right = |ui: &mut egui::Ui, this: &mut Self| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if this.working() {
                        if crate::format::icon_button(
                            ui,
                            Icon::Stop,
                            &say(Message::AiCancel),
                            false,
                            true,
                        )
                        .clicked()
                        {
                            this.cancel();
                            this.tools.drop_the_queue();
                            this.asked = None;
                        }
                    } else {
                        let ready = this.ready() && this.may_send();
                        let send = format!(
                            "{} \u{2014} {}",
                            say(Message::AiSend),
                            say(Message::AiEnterSends)
                        );
                        if crate::format::icon_button(ui, Icon::Send, &send, ready, ready).clicked()
                        {
                            asked = true;
                        }
                    }
                    this.the_model_button(ui, ctx, lang);
                });
            };
            ui.horizontal(|ui| {
                left(ui, self);
                if one_row {
                    right(ui, self);
                }
            });
            self.second_row_height = 0.0;
            if !one_row {
                let second = ui.horizontal(|ui| right(ui, self));
                self.second_row_height =
                    second.response.rect.height() + ui.spacing().item_spacing.y;
            }
        });
        if asked && !self.ready() {
            self.settings_open = true;
            self.notice = Some(Notice::plain(if self.key_missing() {
                Message::AiKeyNeeded
            } else {
                Message::AiNoModelChosen
            }));
            asked = false;
        }
        Asked { asked, attach }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TurnAction {
    Edit,
    AskAgain,
}

fn slow_spinner(ui: &mut egui::Ui) {
    let size = ui.style().spacing.interact_size.y;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(100));
    let radius = rect.height().min(rect.width()) / 2.0 - 2.0;
    let time = ui.input(|input| input.time);
    let start = time * std::f64::consts::TAU;
    let end = start + 240_f64.to_radians() * time.sin();
    let points: Vec<egui::Pos2> = (0..24_u32)
        .map(|at| {
            let angle = start + (end - start) * f64::from(at) / 24.0;
            let (sin, cos) = angle.sin_cos();
            #[expect(clippy::cast_possible_truncation, reason = "a point on the screen")]
            let offset = egui::vec2(cos as f32, sin as f32);
            rect.center() + radius * offset
        })
        .collect();
    ui.painter().add(egui::Shape::line(
        points,
        egui::Stroke::new(3.0, ui.visuals().strong_text_color()),
    ));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TurnShape {
    text: usize,
    attachments: usize,
    results: usize,
    notes: usize,
}

impl TurnShape {
    fn of(turn: &Turn, at: usize, notes: &[(usize, Message)]) -> Self {
        Self {
            text: turn.text().len(),
            attachments: turn.attachments().len(),
            results: match turn {
                Turn::Results { results } => results.len(),
                _ => 0,
            },
            notes: notes.iter().filter(|(index, _)| *index == at).count(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Offers {
    edit: bool,
    ask_again: bool,
}

fn said_by(
    ui: &mut egui::Ui,
    turn: &Turn,
    (model, lang): (&str, Lang),
    salt: &str,
    offers: Offers,
) -> Option<TurnAction> {
    let mine = turn.said() == Said::Person;
    let name = if mine {
        Message::AiYou.say(lang)
    } else if model.is_empty() {
        Message::AiResponse.say(lang)
    } else {
        model.to_owned()
    };
    let mut action = None;
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
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let quiet = crate::format::quiet_icon_button;
                if quiet(ui, Icon::Copy, &Message::DraftCopy.say(lang)).clicked() {
                    ui.ctx().copy_text(turn.text().to_owned());
                }
                if offers.edit && quiet(ui, Icon::Edit, &Message::AiEditMeans.say(lang)).clicked() {
                    action = Some(TurnAction::Edit);
                }
                if offers.ask_again
                    && quiet(ui, Icon::AskAgain, &Message::AiAskAgainMeans.say(lang)).clicked()
                {
                    action = Some(TurnAction::AskAgain);
                }
            });
        });
        if !turn.attachments().is_empty() {
            ui.horizontal_wrapped(|ui| {
                for attachment in turn.attachments() {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.label(egui::RichText::new(&attachment.name).small());
                    });
                }
            });
        }
        crate::ai_written::written(ui, turn.text(), salt, lang);
    });
    action
}

fn went_back(ui: &mut egui::Ui, document_stays: bool, lang: Lang) -> bool {
    let mut put_back = false;
    ui.add_space(GAP);
    egui::Frame::group(ui.style())
        .fill(ui.visuals().faint_bg_color)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.weak(egui::RichText::new(Message::AiWentBack.say(lang)).small());
            if document_stays {
                ui.label(
                    egui::RichText::new(Message::AiWentBackDocumentStays.say(lang))
                        .small()
                        .color(ui.visuals().warn_fg_color),
                );
            }
            if ui.small_button(Message::AiPutBack.say(lang)).clicked() {
                put_back = true;
            }
        });
    put_back
}

pub(crate) fn composer_id() -> egui::Id {
    egui::Id::new("ai-composer-text")
}

impl Window {
    pub(crate) fn open_ai_panel(&mut self, now: f64) {
        if self.ai.open {
            return;
        }
        self.ai.open = true;
        self.ai_flow = Some(crate::room::Flow::new(0.0, self.ai.width, now));
    }

    pub(crate) fn ai_panel_rect(&self) -> Option<egui::Rect> {
        (!self.home && self.ai.open)
            .then_some(self.ai_panel_shape)
            .flatten()
    }

    fn the_chat_handle(&mut self, ui: &egui::Ui) {
        let space = ui.max_rect();
        let tall = crate::room::handle_tall(space.height());
        if tall <= 0.0 {
            return;
        }
        let strip = egui::Rect::from_min_size(
            egui::pos2(
                space.right() - crate::room::HANDLE_WIDE,
                space.center().y - tall / 2.0,
            ),
            egui::vec2(crate::room::HANDLE_WIDE, tall),
        );
        let ctx = ui.ctx().clone();
        let mut open = false;
        egui::Area::new(egui::Id::new("ai-handle"))
            .order(egui::Order::Foreground)
            .fixed_pos(strip.min)
            .show(&ctx, |ui| {
                let handle = ui.interact(
                    strip,
                    egui::Id::new("ai-panel-handle"),
                    egui::Sense::click_and_drag(),
                );
                let live = handle.hovered() || handle.dragged();
                if live {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                }
                if handle.clicked() || handle.drag_delta().x < -2.0 {
                    open = true;
                }
                let visuals = ui.visuals();
                let ink = if live {
                    visuals.selection.stroke.color
                } else {
                    visuals.weak_text_color()
                };
                let fill = if live {
                    visuals.widgets.hovered.bg_fill
                } else {
                    visuals.widgets.inactive.bg_fill
                };
                let painter = ui.painter();
                painter.rect_filled(strip, 4.0, fill);
                painter.rect_stroke(
                    strip,
                    4.0,
                    egui::Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
                    egui::StrokeKind::Inside,
                );
                let middle = strip.center();
                let arm = egui::Stroke::new(1.6, ink);
                painter.line_segment(
                    [
                        egui::pos2(middle.x + 2.0, middle.y - 4.0),
                        egui::pos2(middle.x - 2.0, middle.y),
                    ],
                    arm,
                );
                painter.line_segment(
                    [
                        egui::pos2(middle.x - 2.0, middle.y),
                        egui::pos2(middle.x + 2.0, middle.y + 4.0),
                    ],
                    arm,
                );
                handle.on_hover_text(Message::AiTitle.say(self.lang));
            });
        if open {
            let now = ui.input(|input| input.time);
            self.open_ai_panel(now);
        }
    }

    fn chat_width_now(&mut self, ui: &egui::Ui) -> (f32, bool) {
        let now = ui.input(|input| input.time);
        let wanted = if self.ai.open { self.ai.width } else { 0.0 };
        let drifting = self
            .ai_flow
            .filter(|flow| (flow.to() - wanted).abs() <= crate::room::SAME_WIDTH);
        if let Some(flow) = drifting
            && flow.running(now)
        {
            return (flow.at(now), true);
        }
        self.ai_flow = None;
        (wanted, false)
    }

    pub(crate) fn ai_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.ai.poll(&ctx);
        if self.home {
            self.ai_panel_shape = None;
            return;
        }
        let (width, moving) = self.chat_width_now(ui);
        if moving {
            ctx.request_repaint();
        }
        if width < crate::room::PANEL_GONE {
            self.ai_panel_shape = None;
            self.the_chat_handle(ui);
            return;
        }
        self.ai.try_the_kept_connection(&ctx);
        let lang = self.lang;
        let mut asked = Asked::default();
        let mut close = false;
        let mut answered = None;
        let margin = 8_i8;
        let frame = egui::Frame::side_top_panel(&ui.style().clone())
            .inner_margin(egui::Margin::symmetric(margin, margin));
        let mut panel = egui::Panel::right("ai chat")
            .resizable(!moving)
            .default_size(self.ai.width)
            .size_range(300.0..=640.0)
            .frame(frame);
        if moving {
            panel = panel.exact_size(width);
        }
        let panel = panel.show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(GAP, GAP * 0.75);
            let panel_height = ui.available_height();
            close = self.ai.the_heading(ui, lang);
            self.ai.who_answers(ui, lang);
            if self.ai.settings_open {
                self.ai.the_connection(ui, &ctx, lang);
            }
            self.ai.the_notice(ui, lang);
            ui.separator();
            let row_height = ui.text_style_height(&egui::TextStyle::Body);
            let chrome = composer_chrome(ui) + self.ai.chips_height + self.ai.second_row_height;
            let rows = ai_layout::rows_shown(
                self.ai.wrapped_rows,
                ai_layout::most_rows(panel_height, row_height, chrome),
            );
            let composer = ai_layout::composer_height(rows, row_height, chrome);
            let room =
                ai_layout::conversation_room(ui.available_height(), composer, LEAST_CONVERSATION);
            ui.allocate_ui(egui::vec2(ui.available_width(), room), |ui| {
                answered = self.ai.the_conversation(ui, lang);
            });
            asked = self.ai.the_composer(ui, &ctx, lang, (rows, row_height));
        });
        self.ai_panel_shape = Some(panel.response.rect);
        if !moving {
            self.ai.width = panel.response.rect.width().clamp(300.0, 640.0);
        }
        if let Some(answer) = answered {
            self.ai.tools.answer_the_card(answer);
        }
        self.ai.confirm_full_access(&ctx, lang);
        if asked.attach {
            self.choosing_for = crate::page_actions::Choosing::ChatAttachment;
            self.asking_to_open = true;
        }
        if asked.asked {
            let context = if self.ai.include_context {
                current_page_text(self, MAX_CONTEXT)
            } else {
                String::new()
            };
            let brief = self.document_brief();
            self.ai.start_answer(&ctx, context, &brief);
        }
        self.advance_tools(&ctx);
        self.ai.save_the_chat();
        if close {
            self.ai.open = false;
            let now = ctx.input(|input| input.time);
            self.ai_flow = Some(crate::room::Flow::new(self.ai.width, 0.0, now));
        }
    }
}

fn tool_icon(name: &str) -> Icon {
    match name {
        "document_info" | "read_text" | "list_fonts" => Icon::Document,
        "find_text" => Icon::ZoomIn,
        "render_page" => Icon::Picture,
        "replace_text" | "set_properties" | "fill_field" => Icon::Edit,
        "add_text" => Icon::Text,
        "write_pages" => Icon::Pen,
        "add_blank_page" => Icon::NewDocument,
        "insert_pages" => Icon::Open,
        "delete_pages" => Icon::Delete,
        "move_pages" => Icon::Arrange,
        "rotate_pages" => Icon::RotateRight,
        "undo" => Icon::Undo,
        "redo" => Icon::Redo,
        "ask_person" => Icon::RadioButton,
        _ => Icon::Settings,
    }
}

fn what_the_tools_did(
    ui: &mut egui::Ui,
    results: &[ToolResult],
    (tools, names): (&Tools, &BTreeMap<String, String>),
    lang: Lang,
) {
    for result in results {
        ui.add_space(GAP * 0.5);
        ui.horizontal(|ui| {
            let name = tools
                .called
                .get(&result.call_id)
                .or_else(|| names.get(&result.call_id))
                .map_or("", String::as_str);
            let (icon_colour, colour) = if result.is_error {
                (ui.visuals().error_fg_color, ui.visuals().error_fg_color)
            } else {
                (ui.visuals().text_color(), ui.visuals().weak_text_color())
            };
            let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
            tool_icon(name).draw(ui.painter(), rect, icon_colour);
            ui.label(
                egui::RichText::new(pdf_app::ai_status::did(name, lang))
                    .small()
                    .color(icon_colour),
            );
            ui.add(
                egui::Label::new(
                    egui::RichText::new(one_line(&result.text))
                        .small()
                        .color(colour),
                )
                .truncate(),
            );
        });
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

fn the_question(ui: &mut egui::Ui, question: &mut Question, lang: Lang) -> Option<QuestionReply> {
    let say = |message: Message| message.say(lang);
    let mut reply = None;
    ui.add_space(GAP);
    egui::Frame::group(ui.style())
        .fill(ui.visuals().faint_bg_color)
        .stroke(egui::Stroke::new(1.0, ui.visuals().selection.stroke.color))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.weak(egui::RichText::new(say(Message::AiQuestionForYou)).small());
            ui.add(
                egui::Label::new(egui::RichText::new(&question.asked).strong())
                    .selectable(true)
                    .wrap(),
            );
            ui.add_space(GAP * 0.5);
            for (label, means) in &question.options {
                let mut job = egui::text::LayoutJob::default();
                let style = ui.style();
                job.append(
                    label,
                    0.0,
                    egui::TextFormat {
                        font_id: egui::TextStyle::Body.resolve(style),
                        color: style.visuals.text_color(),
                        ..egui::TextFormat::default()
                    },
                );
                if !means.is_empty() {
                    job.append(
                        &format!("\n{means}"),
                        0.0,
                        egui::TextFormat {
                            font_id: egui::TextStyle::Small.resolve(style),
                            color: style.visuals.weak_text_color(),
                            ..egui::TextFormat::default()
                        },
                    );
                }
                let width = ui.available_width();
                job.wrap.max_width = width - 2.0 * ui.spacing().button_padding.x;
                if ui
                    .add(egui::Button::new(job).min_size(egui::vec2(width, 0.0)))
                    .clicked()
                {
                    reply = Some(QuestionReply::Said(label.clone()));
                }
            }
            ui.add_space(GAP * 0.5);
            ui.horizontal(|ui| {
                let skip = ui.button(say(Message::AiSkipQuestion));
                let answer = ui.add_enabled(
                    !question.own.trim().is_empty(),
                    egui::Button::new(say(Message::AiAnswer)),
                );
                let typed = ui.add(
                    egui::TextEdit::singleline(&mut question.own)
                        .hint_text(say(Message::AiOwnAnswer))
                        .desired_width(ui.available_width()),
                );
                let entered =
                    typed.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if (answer.clicked() || entered) && !question.own.trim().is_empty() {
                    reply = Some(QuestionReply::Said(question.own.trim().to_owned()));
                }
                if skip.clicked() {
                    reply = Some(QuestionReply::Skipped);
                }
            });
        });
    reply
}

fn a_note(ui: &mut egui::Ui, said: &str) {
    ui.add_space(GAP * 0.5);
    ui.add(
        egui::Label::new(egui::RichText::new(said).small().italics())
            .selectable(true)
            .wrap(),
    );
}

fn thinking_so_far(ui: &mut egui::Ui, thinking: &str, lang: Lang) {
    const TAIL: usize = 600;
    ui.add_space(GAP * 0.5);
    let counted = thinking.chars().count();
    let tail: String = thinking
        .chars()
        .skip(counted.saturating_sub(TAIL))
        .collect();
    egui::CollapsingHeader::new(
        egui::RichText::new(Message::AiThinkingAloud.say(lang))
            .small()
            .weak(),
    )
    .id_salt("ai-thinking")
    .default_open(true)
    .show(ui, |ui| {
        ui.add(
            egui::Label::new(egui::RichText::new(tail).small().weak())
                .selectable(true)
                .wrap(),
        );
    });
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
        Effort::None => Message::AiEffortNone,
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
        Mode::Free => Message::AiModeFree,
    }
}

const fn mode_means(mode: Mode) -> Message {
    match mode {
        Mode::ChatOnly => Message::AiModeChatOnlyWhat,
        Mode::AskBeforeChanges => Message::AiModeAskBeforeChangesWhat,
        Mode::DoIt => Message::AiModeDoItWhat,
        Mode::Free => Message::AiModeFreeWhat,
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
        AiState, Answer, Arriving, Asking, MOST_ROUNDS, Notice, Provider, Turn, claude_code_line,
        client_configuration, notice_of,
    };
    use pdf_app::ai_layout;
    use pdf_app::wording::{Lang, Message};

    fn arriving(said: &str) -> Arriving {
        Arriving {
            said: said.to_owned(),
            thinking: String::new(),
        }
    }

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
    fn the_answer_so_far_is_held_while_it_is_still_arriving() {
        let (send, answers) = mpsc::channel();
        let mut state = AiState {
            asking: Some(Asking {
                stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                answers,
            }),
            ..AiState::default()
        };
        state.turns.push(Turn::person("what is this page about?"));
        for said in ["a", "a map"] {
            send.send(Answer::Partial(state.generation, arriving(said)))
                .expect("the worker streams");
        }
        state.poll(&egui::Context::default());
        assert_eq!(
            state.partial.as_ref().map(|far| far.said.as_str()),
            Some("a map")
        );
        assert_eq!(state.turns.len(), 1, "{:?}", state.turns);
        assert!(state.busy(), "the reading is not over");
    }

    #[test]
    fn an_answer_that_ran_out_of_room_says_so() {
        for (cut_short, notes) in [(true, 1), (false, 0)] {
            let (send, answers) = mpsc::channel();
            let mut state = AiState {
                asking: Some(Asking {
                    stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    answers,
                }),
                ..AiState::default()
            };
            state.turns.push(Turn::person("go on then"));
            let reply = Reply {
                cut_short,
                ..said("half of an ans")
            };
            send.send(Answer::Chat(state.generation, Ok(reply)))
                .expect("the worker answers");
            state.poll(&egui::Context::default());
            assert_eq!(state.notes.len(), notes, "cut_short = {cut_short}");
        }
    }

    #[test]
    fn the_finished_answer_replaces_the_half_written_one() {
        let (send, answers) = mpsc::channel();
        let mut state = AiState {
            asking: Some(Asking {
                stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                answers,
            }),
            ..AiState::default()
        };
        state.turns.push(Turn::person("what is this page about?"));
        send.send(Answer::Partial(state.generation, arriving("a ma")))
            .expect("the worker streams");
        send.send(Answer::Chat(state.generation, Ok(said("a map"))))
            .expect("the worker answers");
        state.poll(&egui::Context::default());
        assert_eq!(state.partial, None);
        assert_eq!(state.turns.len(), 2);
        assert_eq!(state.turns[1], Turn::model("a map"));
    }

    #[test]
    fn stopping_keeps_the_part_of_the_answer_that_arrived() {
        let (send, answers) = mpsc::channel();
        let mut state = AiState {
            asking: Some(Asking {
                stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                answers,
            }),
            ..AiState::default()
        };
        state.turns.push(Turn::person("tell me about this page"));
        send.send(Answer::Partial(
            state.generation,
            arriving("it is a map of"),
        ))
        .expect("the worker streams");
        state.poll(&egui::Context::default());
        state.cancel();
        assert_eq!(state.turns.len(), 2);
        assert_eq!(state.turns[1], Turn::model("it is a map of"));
        assert_eq!(state.partial, None);

        let (_send, answers) = mpsc::channel();
        let mut nothing_yet = AiState {
            asking: Some(Asking {
                stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                answers,
            }),
            ..AiState::default()
        };
        nothing_yet.turns.push(Turn::person("tell me"));
        nothing_yet.cancel();
        assert_eq!(nothing_yet.turns.len(), 1, "nothing arrived, nothing kept");
    }

    #[test]
    fn a_partial_from_the_old_connection_is_not_shown() {
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
        send.send(Answer::Partial(asked_under, arriving("stale")))
            .expect("the worker streams");
        state.poll(&egui::Context::default());
        assert_eq!(state.partial, None);
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

    fn a_conversation() -> Vec<Turn> {
        vec![
            Turn::person("one"),
            Turn::model("first answer"),
            Turn::person("three"),
            Turn::model("second answer"),
        ]
    }

    #[test]
    fn going_back_to_a_question_can_be_put_back() {
        let mut state = AiState {
            turns: a_conversation(),
            ..AiState::default()
        };
        assert_eq!(state.last_question(), Some(2));
        state.go_back_to(2, false);
        assert_eq!(state.turns.len(), 2);
        assert_eq!(state.composer, "three");
        assert!(!state.send_now);
        state.put_back();
        assert_eq!(state.turns, a_conversation());
        assert!(state.composer.is_empty());

        state.go_back_to(state.last_question().expect("answered"), true);
        assert_eq!(state.turns.len(), 2);
        assert!(state.send_now, "Ask again sends it");

        let (_tx, rx) = mpsc::channel();
        let mut busy = AiState {
            turns: a_conversation(),
            asking: Some(Asking {
                stop: std::sync::Arc::default(),
                answers: rx,
            }),
            ..AiState::default()
        };
        busy.go_back_to(2, false);
        assert_eq!(busy.turns.len(), 4, "nothing is cut while it is answering");
    }

    #[test]
    fn an_unanswered_question_is_not_asked_again() {
        let state = AiState {
            turns: vec![
                Turn::person("one"),
                Turn::model("two"),
                Turn::person("three"),
            ],
            ..AiState::default()
        };
        assert_eq!(state.last_question(), None);
    }

    #[test]
    fn up_walks_back_through_what_was_asked() {
        use pdf_app::ai_recall::Way;
        let mut state = AiState {
            turns: a_conversation(),
            history: Some(vec![pdf_agent::history::Chat {
                id: "old".to_owned(),
                turns: vec![Turn::person("elsewhere")],
                ..pdf_agent::history::Chat::default()
            }]),
            ..AiState::default()
        };
        for want in ["three", "one", "elsewhere"] {
            assert!(state.recall_key(Way::Up, false));
            assert_eq!(state.composer, want);
        }
        assert!(!state.recall_key(Way::Up, false), "nothing older");
        assert!(state.recall_key(Way::Down, false));
        assert!(state.recall_key(Way::Down, false));
        assert_eq!(state.composer, "three");
    }

    fn kept(id: &str, place: &str) -> pdf_agent::history::Chat {
        pdf_agent::history::Chat {
            id: id.to_owned(),
            documents: vec!["x.pdf".to_owned()],
            places: vec![place.to_owned()],
            turns: vec![Turn::person("about x"), Turn::model("x is a report")],
            ..pdf_agent::history::Chat::default()
        }
    }

    #[test]
    fn a_document_brings_back_its_own_chat() {
        let mut state = AiState {
            history: Some(vec![kept("kept", "/a/x.pdf")]),
            ..AiState::default()
        };
        state.document_arrived("x.pdf".to_owned(), "/a/x.pdf".to_owned());
        assert_eq!(state.chat_id, "kept");
        assert_eq!(state.turns.len(), 2);

        state.document_arrived("y.pdf".to_owned(), "/b/y.pdf".to_owned());
        assert!(state.turns.is_empty(), "a new chat");
        assert!(state.chat_id.is_empty());
        assert_eq!(state.places, ["/b/y.pdf"]);
        assert_eq!(state.documents, ["y.pdf"]);

        state.document_arrived("x.pdf".to_owned(), "/b/x.pdf".to_owned());
        assert!(state.turns.is_empty(), "the same name elsewhere is not it");
    }

    #[test]
    fn a_chat_carried_to_another_file_is_that_files_too() {
        let mut state = AiState {
            history: Some(vec![kept("kept", "/a/x.pdf")]),
            ..AiState::default()
        };
        state.document_arrived("y.pdf".to_owned(), "/b/y.pdf".to_owned());
        state.open_a_chat(&kept("kept", "/a/x.pdf"));
        assert_eq!(state.places, ["/a/x.pdf", "/b/y.pdf"]);
        assert_eq!(state.documents.last().map(String::as_str), Some("y.pdf"));
        state.saved_as("/b/y-edited.pdf".to_owned());
        assert_eq!(state.places.len(), 3);
        let kept_now = pdf_agent::history::Chat {
            places: state.places.clone(),
            ..kept("kept", "/a/x.pdf")
        };
        assert_eq!(
            pdf_agent::history::chat_about(&[kept_now], "/b/y-edited.pdf")
                .map(|chat| chat.id.as_str()),
            Some("kept")
        );
    }
}
