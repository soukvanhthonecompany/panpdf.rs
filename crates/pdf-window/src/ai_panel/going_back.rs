use pdf_agent::connect::{Attachment, AttachmentKind, Said, Turn};
use pdf_app::ai_recall::{Way, distinct};
use pdf_app::wording::Message;

use super::{AiState, PendingAttachment};

pub(super) struct Rewound {
    at: usize,
    tail: Vec<Turn>,
    notes: Vec<(usize, Message)>,
    changed_the_document: bool,
}

impl Rewound {
    pub(super) const fn changed_the_document(&self) -> bool {
        self.changed_the_document
    }
}

impl AiState {
    pub(super) fn go_back_to(&mut self, at: usize, and_ask: bool) {
        let Some(Turn::Person { text, attachments }) = self.turns.get(at).cloned() else {
            return;
        };
        if self.working() {
            return;
        }
        let tail = self.turns.split_off(at);
        let (kept, gone): (Vec<_>, Vec<_>) = std::mem::take(&mut self.notes)
            .into_iter()
            .partition(|(index, _)| *index < at);
        self.notes = kept;
        let changed_the_document = gone
            .iter()
            .any(|(_, said)| matches!(said, Message::AiChangedTheDocument { .. }));
        self.rewound = Some(Rewound {
            at,
            tail,
            notes: gone,
            changed_the_document,
        });
        self.composer = text;
        self.pending = attachments
            .into_iter()
            .filter(|attachment| !attachment.bytes.is_empty())
            .map(pending_again)
            .collect();
        self.recall.forget();
        self.notice = None;
        self.send_now = and_ask;
    }

    pub(super) fn put_back(&mut self) {
        let Some(rewound) = self.rewound.take() else {
            return;
        };
        if self.working() || self.turns.len() != rewound.at {
            return;
        }
        self.turns.extend(rewound.tail);
        self.notes.extend(rewound.notes);
        self.composer.clear();
        self.pending.clear();
    }

    pub(super) fn last_question(&self) -> Option<usize> {
        let at = self
            .turns
            .iter()
            .rposition(|turn| turn.said() == Said::Person)?;
        (at + 1 < self.turns.len()).then_some(at)
    }

    fn asked_before(&mut self) -> Vec<String> {
        let here: Vec<String> = self
            .turns
            .iter()
            .rev()
            .filter_map(|turn| match turn {
                Turn::Person { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        let chat_id = self.chat_id.clone();
        let elsewhere: Vec<String> = self
            .the_chats()
            .iter()
            .filter(|chat| chat.id != chat_id)
            .flat_map(|chat| {
                chat.turns
                    .iter()
                    .rev()
                    .filter_map(|turn| match turn {
                        Turn::Person { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        distinct(here.into_iter().chain(elsewhere))
    }

    pub(super) fn recall_key(&mut self, way: Way, caret_at_start: bool) -> bool {
        let said = self.asked_before();
        match self.recall.step(&said, &self.composer, caret_at_start, way) {
            Some(text) => {
                self.composer = text;
                true
            }
            None => false,
        }
    }

    pub(super) fn the_whole_chat(&self, lang: pdf_app::wording::Lang) -> String {
        let you = Message::AiYou.say(lang);
        let you = you.as_str();
        let answered_by = if self.model.is_empty() {
            Message::AiResponse.say(lang)
        } else {
            self.model.clone()
        };
        let mut out = String::new();
        for turn in &self.turns {
            let (who, text) = match turn {
                Turn::Person { text, .. } => (you, text.as_str()),
                Turn::Model { text, .. } if !text.trim().is_empty() => {
                    (answered_by.as_str(), text.as_str())
                }
                _ => continue,
            };
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(who);
            out.push_str(":\n");
            out.push_str(text.trim());
        }
        out
    }
}

fn pending_again(attachment: Attachment) -> PendingAttachment {
    let kind = match attachment.kind {
        AttachmentKind::Image { .. } => pdf_agent::attach::Kind::Picture,
        AttachmentKind::Text => pdf_agent::attach::Kind::Text,
    };
    PendingAttachment {
        name: attachment.name.clone(),
        bytes: attachment.bytes.len(),
        kind,
        outcome: Ok(vec![attachment]),
    }
}
