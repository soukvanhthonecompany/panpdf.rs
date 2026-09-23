use std::collections::VecDeque;

use crate::view::Step;
use crate::wording::Message;

pub const SEND_LIMIT: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Target {
    pub page: usize,
    pub block: usize,
    pub from: (usize, usize),
    pub to: (usize, usize),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Intent {
    Insert(String),
    Delete { backwards: bool, count: usize },
    Caret { step: Step, extend: bool },
    Undo,
    Redo,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    pub text: String,
    pub reason: Message,
    pub target: Option<Target>,
    pub epoch: u64,
}

impl Draft {
    #[must_use]
    pub fn can_retry(&self, epoch: u64) -> bool {
        self.target.is_some() && self.epoch == epoch
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Landing {
    Committed,
    Unchanged,
    Refused(Message),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Accounting {
    pub typed: u64,
    pub committed: u64,
    pub discarded: u64,
    pub commands: u64,
    pub commands_done: u64,
    pub commands_refused: u64,
}

#[derive(Clone, Debug, PartialEq)]
struct Flight {
    text: String,
    presses: u64,
    target: Option<Target>,
    epoch: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Input {
    queued: VecDeque<Intent>,
    flight: Option<Flight>,
    draft: Option<Draft>,
    parked: Option<Draft>,
    accounting: Accounting,
}

fn chars(text: &str) -> u64 {
    u64::try_from(text.chars().count()).unwrap_or(u64::MAX)
}

impl Input {
    pub fn accept(&mut self, text: &str) {
        const BREAKS: [char; 2] = ['\n', pdf_edit::LINE_BREAK];
        if text.is_empty() {
            return;
        }
        self.accounting.typed += chars(text);
        if let Some(draft) = self.draft.as_mut() {
            draft.text.push_str(text);
            return;
        }
        if !text.contains(BREAKS)
            && let Some(Intent::Insert(last)) = self.queued.back_mut()
            && !last.contains(BREAKS)
        {
            last.push_str(text);
            return;
        }
        self.queued.push_back(Intent::Insert(text.to_owned()));
    }

    pub fn press(&mut self, intent: Intent) -> Result<(), Message> {
        let presses = match &intent {
            Intent::Insert(text) => {
                self.accept(&text.clone());
                return Ok(());
            }
            Intent::Delete { count, .. } => u64::try_from(*count).unwrap_or(u64::MAX),
            Intent::Caret { .. } | Intent::Undo | Intent::Redo => 1,
        };
        self.accounting.commands += presses;
        if self.draft.is_some() && matches!(intent, Intent::Delete { .. }) {
            self.accounting.commands_refused += presses;
            return Err(Message::DraftBlocksDelete);
        }
        if let (
            Intent::Delete { backwards, count },
            Some(Intent::Delete {
                backwards: had,
                count: held,
            }),
        ) = (&intent, self.queued.back_mut())
            && backwards == had
        {
            *held += count;
            return Ok(());
        }
        self.queued.push_back(intent);
        Ok(())
    }

    #[must_use]
    pub fn front(&self) -> Option<&Intent> {
        if self.flight.is_some() {
            return None;
        }
        self.queued.front()
    }

    pub fn send(&mut self, target: Target, epoch: u64) -> Option<String> {
        if self.flight.is_some() || self.draft.is_some() {
            return None;
        }
        let Some(Intent::Insert(text)) = self.queued.front() else {
            return None;
        };
        if text.len() > SEND_LIMIT {
            let Some(Intent::Insert(text)) = self.queued.pop_front() else {
                return None;
            };
            let reason = Message::DraftTooLongForOneEdit {
                bytes: text.len(),
                limit: SEND_LIMIT,
            };
            self.keep(text, &reason, Some(target), epoch);
            return None;
        }
        let Some(Intent::Insert(text)) = self.queued.pop_front() else {
            return None;
        };
        self.flight = Some(Flight {
            text: text.clone(),
            presses: 0,
            target: Some(target),
            epoch,
        });
        Some(text)
    }

    pub fn hold_front(&mut self) -> Option<Intent> {
        if self.flight.is_some() || self.draft.is_some() {
            return None;
        }
        let intent = self.queued.pop_front()?;
        match &intent {
            Intent::Insert(text) => self.accounting.committed += chars(text),
            Intent::Delete { count, .. } => {
                self.accounting.commands_done += u64::try_from(*count).unwrap_or(u64::MAX);
            }
            Intent::Caret { .. } | Intent::Undo | Intent::Redo => {
                self.accounting.commands_done += 1;
            }
        }
        Some(intent)
    }

    pub fn live_refused(&mut self, text: String, reason: &Message, target: Target, epoch: u64) {
        self.accounting.committed = self.accounting.committed.saturating_sub(chars(&text));
        let (behind, refused) = self.drain_queue();
        self.accounting.commands_refused += refused;
        let mut all = text;
        all.push_str(&behind);
        if !all.is_empty() {
            self.keep(all, reason, Some(target), epoch);
        }
    }

    pub fn take(&mut self) -> Option<Intent> {
        if self.flight.is_some() || matches!(self.queued.front(), Some(Intent::Insert(_)) | None) {
            return None;
        }
        let intent = self.queued.pop_front()?;
        match &intent {
            Intent::Caret { .. } => self.accounting.commands_done += 1,
            Intent::Delete { count, .. } => {
                self.flight = Some(Flight {
                    text: String::new(),
                    presses: u64::try_from(*count).unwrap_or(u64::MAX),
                    target: None,
                    epoch: 0,
                });
            }
            Intent::Undo | Intent::Redo => {
                self.flight = Some(Flight {
                    text: String::new(),
                    presses: 1,
                    target: None,
                    epoch: 0,
                });
            }
            Intent::Insert(_) => {}
        }
        Some(intent)
    }

    pub fn take_one(&mut self) -> Option<Intent> {
        if self.flight.is_some() {
            return None;
        }
        let Some(Intent::Delete { backwards, count }) = self.queued.front_mut() else {
            return None;
        };
        let backwards = *backwards;
        if *count > 1 {
            *count -= 1;
        } else {
            self.queued.pop_front();
        }
        self.flight = Some(Flight {
            text: String::new(),
            presses: 1,
            target: None,
            epoch: 0,
        });
        Some(Intent::Delete {
            backwards,
            count: 1,
        })
    }

    pub fn landed(&mut self, landing: Landing) -> u64 {
        let flight = self.flight.take();
        match landing {
            Landing::Committed | Landing::Unchanged => {
                if let Some(flight) = flight {
                    self.accounting.committed += chars(&flight.text);
                    self.accounting.commands_done += flight.presses;
                }
                0
            }
            Landing::Refused(reason) => {
                let Some(flight) = flight else {
                    return self.set_aside(&reason);
                };
                self.accounting.commands_refused += flight.presses;
                if flight.text.is_empty() {
                    return self.set_aside(&reason);
                }
                let (behind, refused) = self.drain_queue();
                let mut text = flight.text;
                text.push_str(&behind);
                self.keep(text, &reason, flight.target, flight.epoch);
                refused
            }
        }
    }

    pub fn set_aside(&mut self, reason: &Message) -> u64 {
        let (text, refused) = self.drain_queue();
        if !text.is_empty() {
            self.keep(text, reason, None, 0);
        }
        refused
    }

    pub fn document_replaced(&mut self, reason: &Message) {
        if let Some(flight) = self.flight.take() {
            self.accounting.commands_refused += flight.presses;
            if !flight.text.is_empty() {
                self.queued.push_front(Intent::Insert(flight.text));
            }
        }
        self.set_aside(reason);
        for draft in [self.draft.as_mut(), self.parked.as_mut()]
            .into_iter()
            .flatten()
        {
            draft.target = None;
        }
    }

    pub fn retry(&mut self, epoch: u64) -> Result<(Target, String), Message> {
        let parked = self.draft.is_none();
        let Some(draft) = self.draft.as_ref().or(self.parked.as_ref()) else {
            return Err(Message::NoDraftToRetry);
        };
        if self.flight.is_some() {
            return Err(Message::DraftWaitsForAnotherEdit);
        }
        let Some(target) = draft.target else {
            return Err(Message::DraftPlaceIsGone);
        };
        if draft.epoch != epoch {
            return Err(Message::DraftPlaceIsUncertain);
        }
        let taken = if parked {
            self.parked.take()
        } else {
            self.draft.take()
        };
        let Some(draft) = taken else {
            return Err(Message::NoDraftToRetry);
        };
        self.flight = Some(Flight {
            text: draft.text.clone(),
            presses: 0,
            target: Some(target),
            epoch,
        });
        Ok((target, draft.text))
    }

    pub fn discard(&mut self) -> Option<Draft> {
        let draft = self.draft.take().or_else(|| self.parked.take())?;
        self.accounting.discarded += chars(&draft.text);
        Some(draft)
    }

    pub fn abandon(&mut self) {
        let (mut lost, _) = self.drain_queue();
        if let Some(flight) = self.flight.take() {
            self.accounting.commands_refused += flight.presses;
            lost.push_str(&flight.text);
        }
        if let Some(draft) = self.draft.take() {
            lost.push_str(&draft.text);
        }
        if let Some(draft) = self.parked.take() {
            lost.push_str(&draft.text);
        }
        self.accounting.discarded += chars(&lost);
    }

    #[must_use]
    pub fn draft(&self) -> Option<&Draft> {
        self.draft.as_ref().or(self.parked.as_ref())
    }

    #[must_use]
    pub const fn collecting(&self) -> bool {
        self.draft.is_some()
    }

    pub fn park(&mut self) {
        let Some(draft) = self.draft.take() else {
            return;
        };
        match self.parked.as_mut() {
            Some(parked) => {
                parked.text.push_str(&draft.text);
                parked.reason = draft.reason;
                parked.target = None;
            }
            None => self.parked = Some(draft),
        }
    }

    #[must_use]
    pub fn has_queued(&self) -> bool {
        !self.queued.is_empty()
    }

    #[must_use]
    pub fn pending(&self) -> bool {
        !self.queued.is_empty() || self.flight.is_some()
    }

    #[must_use]
    pub fn queued_chars(&self) -> usize {
        self.queued
            .iter()
            .map(|intent| match intent {
                Intent::Insert(text) => text.chars().count(),
                _ => 0,
            })
            .sum()
    }

    #[must_use]
    pub const fn accounting(&self) -> Accounting {
        self.accounting
    }

    #[must_use]
    pub fn balanced(&self) -> bool {
        let held = u64::try_from(self.queued_chars()).unwrap_or(u64::MAX)
            + self.flight.as_ref().map_or(0, |flight| chars(&flight.text))
            + self.draft.as_ref().map_or(0, |draft| chars(&draft.text))
            + self.parked.as_ref().map_or(0, |draft| chars(&draft.text));
        self.accounting.typed == self.accounting.committed + self.accounting.discarded + held
    }

    fn drain_queue(&mut self) -> (String, u64) {
        let mut text = String::new();
        let mut refused = 0;
        for intent in std::mem::take(&mut self.queued) {
            match intent {
                Intent::Insert(more) => text.push_str(&more),
                Intent::Delete { count, .. } => {
                    refused += u64::try_from(count).unwrap_or(u64::MAX);
                }
                Intent::Caret { .. } | Intent::Undo | Intent::Redo => refused += 1,
            }
        }
        self.accounting.commands_refused += refused;
        (text, refused)
    }

    fn keep(&mut self, text: String, reason: &Message, target: Option<Target>, epoch: u64) {
        match self.draft.as_mut() {
            Some(draft) => draft.text.push_str(&text),
            None => {
                self.draft = Some(Draft {
                    text,
                    reason: reason.to_owned(),
                    target,
                    epoch,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(said: &str) -> Message {
        Message::Plain(said.to_owned())
    }

    const AT: Target = Target {
        page: 0,
        block: 2,
        from: (1, 3),
        to: (1, 3),
    };

    fn delete(backwards: bool) -> Intent {
        Intent::Delete {
            backwards,
            count: 1,
        }
    }

    fn drain(input: &mut Input) -> Vec<Intent> {
        let mut out = Vec::new();
        while let Some(front) = input.front().cloned() {
            match front {
                Intent::Insert(_) => {
                    let text = input.send(AT, 4).unwrap();
                    out.push(Intent::Insert(text));
                }
                _ => out.push(input.take().unwrap()),
            }
            input.landed(Landing::Committed);
        }
        out
    }

    #[test]
    fn text_typed_behind_a_refused_edit_is_kept_with_it_byte_for_byte() {
        let mut input = Input::default();
        input.accept("สวัสดี");
        assert_eq!(input.send(AT, 4).as_deref(), Some("สวัสดี"));
        input.accept("ABC");
        assert_eq!(input.send(AT, 4), None, "one edit at a time");
        input.landed(Landing::Refused(plain("past the frame")));
        let draft = input.draft().unwrap();
        assert_eq!(draft.text.as_bytes(), "สวัสดีABC".as_bytes());
        assert_eq!(draft.reason, plain("past the frame"));
        assert_eq!(draft.target, Some(AT));
        assert!(!input.has_queued());
        assert!(input.balanced());
    }

    #[test]
    fn a_commit_lets_what_was_typed_meanwhile_go_next() {
        let mut input = Input::default();
        input.accept("A");
        input.send(AT, 4).unwrap();
        input.accept("B");
        input.landed(Landing::Committed);
        assert_eq!(input.draft(), None);
        assert_eq!(input.send(AT, 5).as_deref(), Some("B"));
        input.landed(Landing::Committed);
        assert_eq!(input.accounting().committed, 2);
        assert!(input.balanced());
    }

    #[test]
    fn text_breaks_and_deletes_keep_the_order_they_were_pressed_in() {
        let mut input = Input::default();
        input.accept("a");
        input.accept("\n");
        input.accept("b");
        input.press(delete(true)).unwrap();
        input.accept("c");
        input.accept("d");
        input.accept(&pdf_edit::LINE_BREAK.to_string());
        input.accept("e");
        assert_eq!(
            drain(&mut input),
            vec![
                Intent::Insert("a".to_owned()),
                Intent::Insert("\n".to_owned()),
                Intent::Insert("b".to_owned()),
                delete(true),
                Intent::Insert("cd".to_owned()),
                Intent::Insert(pdf_edit::LINE_BREAK.to_string()),
                Intent::Insert("e".to_owned()),
            ]
        );
        assert!(input.balanced());
    }

    #[test]
    fn repeated_presses_are_counted_and_merged_only_with_their_own_kind() {
        let mut input = Input::default();
        input.accept("\n");
        input.accept("\n");
        for _ in 0..3 {
            input.press(delete(true)).unwrap();
        }
        input.press(delete(false)).unwrap();
        input
            .press(Intent::Caret {
                step: Step::Left,
                extend: false,
            })
            .unwrap();
        input.press(delete(true)).unwrap();
        assert_eq!(
            drain(&mut input),
            vec![
                Intent::Insert("\n".to_owned()),
                Intent::Insert("\n".to_owned()),
                Intent::Delete {
                    backwards: true,
                    count: 3
                },
                delete(false),
                Intent::Caret {
                    step: Step::Left,
                    extend: false
                },
                delete(true),
            ]
        );
        let counted = input.accounting();
        assert_eq!((counted.commands, counted.commands_done), (6, 6));
    }

    #[test]
    fn a_delete_pressed_while_an_edit_is_away_waits_its_turn() {
        let mut input = Input::default();
        input.accept("A");
        input.send(AT, 4).unwrap();
        input.press(delete(true)).unwrap();
        assert_eq!(input.front(), None, "nothing goes past the flight");
        assert_eq!(input.take(), None);
        input.landed(Landing::Committed);
        assert_eq!(input.take(), Some(delete(true)));
        assert!(input.pending(), "in flight until it lands");
        input.landed(Landing::Unchanged);
        assert!(!input.pending());
        assert_eq!(input.accounting().commands_done, 1);
    }

    #[test]
    fn a_merged_delete_can_be_taken_one_press_at_a_time() {
        let mut input = Input::default();
        for _ in 0..3 {
            input.press(delete(true)).unwrap();
        }
        assert_eq!(input.take_one(), Some(delete(true)));
        assert_eq!(input.take_one(), None, "one in flight at a time");
        input.landed(Landing::Committed);
        assert_eq!(
            input.front(),
            Some(&Intent::Delete {
                backwards: true,
                count: 2
            })
        );
        assert_eq!(
            input.take(),
            Some(Intent::Delete {
                backwards: true,
                count: 2
            })
        );
        input.landed(Landing::Committed);
        let counted = input.accounting();
        assert_eq!((counted.commands, counted.commands_done), (3, 3));
        assert!(!input.pending());
    }

    #[test]
    fn a_refusal_refuses_the_commands_behind_it_and_keeps_the_text() {
        let mut input = Input::default();
        input.accept("x");
        input.send(AT, 4).unwrap();
        input.accept("a");
        input.press(delete(true)).unwrap();
        input.press(delete(true)).unwrap();
        input.accept("\n");
        input.accept("b");
        let refused = input.landed(Landing::Refused(plain("no")));
        assert_eq!(refused, 2);
        assert_eq!(input.draft().unwrap().text, "xa\nb");
        let counted = input.accounting();
        assert_eq!((counted.commands, counted.commands_refused), (2, 2));
        assert!(input.balanced());
        assert!(input.press(delete(true)).is_err());
        assert!(!input.has_queued());
        assert!(
            input
                .press(Intent::Caret {
                    step: Step::Right,
                    extend: true
                })
                .is_ok()
        );
    }

    #[test]
    fn a_paste_over_the_limit_is_kept_whole_and_not_sent() {
        let mut input = Input::default();
        let paste = "ก".repeat(SEND_LIMIT);
        input.accept(&paste);
        assert_eq!(input.send(AT, 4), None);
        let draft = input.draft().unwrap();
        assert_eq!(draft.text, paste);
        assert!(draft.can_retry(4), "aimed where it was pasted");
        assert!(input.balanced());
        let mut input = Input::default();
        input.accept(&"A".repeat(SEND_LIMIT));
        assert_eq!(input.send(AT, 4).map(|text| text.len()), Some(SEND_LIMIT));
    }

    #[test]
    fn a_draft_clicked_away_from_stops_collecting_and_keeps_its_text() {
        let mut input = Input::default();
        input.accept("AB");
        assert_eq!(input.send(AT, 4).as_deref(), Some("AB"));
        input.landed(Landing::Refused(plain("past the frame")));
        assert!(input.collecting());
        input.accept("C");
        assert_eq!(input.draft().unwrap().text, "ABC");

        input.park();
        assert!(!input.collecting());
        input.accept("xyz");
        let elsewhere = Target {
            page: 0,
            block: 9,
            from: (0, 0),
            to: (0, 0),
        };
        assert_eq!(
            input.send(elsewhere, 4).as_deref(),
            Some("xyz"),
            "sent, not drafted"
        );
        assert_eq!(input.draft().unwrap().text, "ABC", "the draft is whole");
        assert!(input.balanced());
        input.landed(Landing::Committed);
        assert_eq!(input.retry(4), Ok((AT, "ABC".to_owned())));
        assert!(input.balanced());
    }

    #[test]
    fn typing_while_a_draft_waits_joins_the_draft_and_sends_nothing() {
        let mut input = Input::default();
        input.accept("A");
        input.send(AT, 4).unwrap();
        input.landed(Landing::Refused(plain("no")));
        input.accept("B");
        assert_eq!(input.send(AT, 4), None);
        assert_eq!(input.draft().unwrap().text, "AB");
    }

    #[test]
    fn a_retry_sends_the_whole_draft_to_its_own_place_only_under_its_epoch() {
        let mut input = Input::default();
        input.accept("AB");
        input.send(AT, 4).unwrap();
        input.landed(Landing::Refused(plain("no")));

        assert!(input.retry(5).is_err());
        assert_eq!(input.draft().unwrap().text, "AB");

        assert_eq!(input.retry(4), Ok((AT, "AB".to_owned())));
        assert_eq!(input.draft(), None);
        input.landed(Landing::Refused(plain("still no")));
        assert_eq!(input.draft().unwrap().text, "AB");
        assert_eq!(input.draft().unwrap().reason, plain("still no"));
        assert!(input.balanced());
    }

    #[test]
    fn a_caret_that_is_lost_sets_the_queue_aside_with_no_place_to_retry() {
        let mut input = Input::default();
        input.accept("AB");
        input.set_aside(&plain("caret gone"));
        let draft = input.draft().unwrap();
        assert_eq!(draft.text, "AB");
        assert_eq!(draft.target, None);
        assert!(!draft.can_retry(draft.epoch));
        assert!(input.retry(draft.epoch).is_err());
        let mut empty = Input::default();
        empty.set_aside(&plain("caret gone"));
        assert_eq!(empty, Input::default());
    }

    #[test]
    fn a_document_replaced_under_a_flight_keeps_every_byte_and_no_place() {
        let mut input = Input::default();
        input.accept("A");
        input.send(AT, 4).unwrap();
        input.accept("B");
        input.document_replaced(&plain("another file"));
        let draft = input.draft().unwrap();
        assert_eq!(draft.text, "AB");
        assert_eq!(draft.target, None);
        input.landed(Landing::Refused(plain("late")));
        assert_eq!(input.draft().unwrap().text, "AB");
        assert_eq!(input.draft().unwrap().reason, plain("another file"));
        assert!(input.balanced());
    }

    #[test]
    fn a_later_refusal_joins_the_draft_and_keeps_the_first_reason_and_place() {
        let mut input = Input::default();
        input.accept("A");
        input.set_aside(&plain("first"));
        input.landed(Landing::Refused(plain("second")));
        assert_eq!(input.draft().unwrap().reason, plain("first"));
        let mut input = Input::default();
        input.accept("A");
        input.send(AT, 4).unwrap();
        input.landed(Landing::Refused(plain("first")));
        input.accept("B");
        input.set_aside(&plain("second"));
        let draft = input.draft().unwrap();
        assert_eq!(
            (draft.text.as_str(), &draft.reason),
            ("AB", &plain("first"))
        );
        assert_eq!(draft.target, Some(AT));
    }

    #[test]
    fn only_discard_drops_text() {
        let mut input = Input::default();
        input.accept("A");
        input.set_aside(&plain("x"));
        assert_eq!(
            input.discard().map(|draft| draft.text).as_deref(),
            Some("A")
        );
        assert_eq!(input.accounting().discarded, 1);
        assert!(input.balanced());
    }

    #[test]
    fn abandon_drops_a_queued_command_and_whatever_is_in_flight() {
        let mut input = Input::default();
        input.accept("AB");
        input.send(AT, 4).unwrap();
        input
            .press(Intent::Caret {
                step: Step::Left,
                extend: false,
            })
            .unwrap();
        assert!(input.pending());

        input.abandon();

        assert!(!input.pending(), "the flight and the queue are both gone");
        assert_eq!(input.draft(), None);
        assert_eq!(input.accounting().discarded, 2, "the flight's two letters");
        assert_eq!(
            input.accounting().commands_refused,
            1,
            "the queued caret move, never carried out"
        );
        assert!(input.balanced());
    }

    #[test]
    fn abandon_drops_the_draft_and_the_one_parked_before_it() {
        let mut input = Input::default();
        input.accept("P");
        input.send(AT, 4).unwrap();
        input.landed(Landing::Refused(plain("no")));
        input.park();
        assert_eq!(input.draft().unwrap().text, "P");

        let elsewhere = Target {
            page: 0,
            block: 9,
            from: (0, 0),
            to: (0, 0),
        };
        input.accept("Q");
        input.send(elsewhere, 4).unwrap();
        input.landed(Landing::Refused(plain("no again")));
        assert_eq!(
            input.draft().unwrap().text,
            "Q",
            "the new draft, not the parked one"
        );

        input.abandon();

        assert_eq!(input.draft(), None);
        assert_eq!(
            input.accounting().discarded,
            2,
            "\"P\" parked and \"Q\" drafted"
        );
        assert!(input.balanced());
        let mut only_active = Input::default();
        only_active.accept("Q");
        only_active.send(elsewhere, 4).unwrap();
        only_active.landed(Landing::Refused(plain("no again")));
        only_active.abandon();
        assert_eq!(only_active.accounting().discarded, 1);
    }

    #[test]
    fn the_balance_catches_a_character_that_went_missing() {
        let mut input = Input::default();
        input.accept("AB");
        assert!(input.balanced());
        if let Some(Intent::Insert(text)) = input.queued.front_mut() {
            text.pop();
        }
        assert!(!input.balanced());
    }
}
