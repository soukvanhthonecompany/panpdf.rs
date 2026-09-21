#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    Changed { page: usize, region: Option<String> },
    Unchanged,
    Refused(String),
    Dropped(String),
}

impl Outcome {
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Changed { .. } => "changed",
            Self::Unchanged => "unchanged",
            Self::Refused(_) => "refused",
            Self::Dropped(_) => "dropped",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentState {
    pub revision: u64,
    pub bytes: usize,
    pub digest: String,
    pub can_undo: bool,
    pub can_redo: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandRecord {
    pub request: u64,
    pub command: String,
    pub arguments: String,
    pub before: DocumentState,
    pub after: DocumentState,
    pub outcome: Outcome,
    pub status: String,
}

pub const MEASURED_FIELDS: &str = "bytes,digest,revision,undo_available,redo_available";

impl CommandRecord {
    #[must_use]
    pub fn measured_state_unchanged(&self) -> bool {
        !self.before.digest.is_empty()
            && !self.after.digest.is_empty()
            && self.before.digest == self.after.digest
            && self.before.bytes == self.after.bytes
            && self.before.revision == self.after.revision
            && self.before.can_undo == self.after.can_undo
            && self.before.can_redo == self.after.can_redo
    }

    #[must_use]
    pub fn json(&self) -> String {
        let (outcome, reason) = match &self.outcome {
            Outcome::Changed { page, region } => (
                format!(
                    r#""changed","page":{page},"region":{}"#,
                    region.as_deref().map_or("null".to_owned(), quoted)
                ),
                String::new(),
            ),
            Outcome::Unchanged => (
                r#""unchanged","page":null,"region":null"#.to_owned(),
                String::new(),
            ),
            Outcome::Refused(reason) => (
                r#""refused","page":null,"region":null"#.to_owned(),
                reason.clone(),
            ),
            Outcome::Dropped(reason) => (
                r#""dropped","page":null,"region":null"#.to_owned(),
                reason.clone(),
            ),
        };
        format!(
            concat!(
                r#"{{"record":"command","schema":2,"request":{request},"command":{command},"#,
                r#""arguments":{arguments},"outcome":{outcome},"reason":{reason},"#,
                r#""status":{status},"#,
                r#""revision_before":{rb},"revision_after":{ra},"#,
                r#""bytes_before":{bb},"bytes_after":{ba},"#,
                r#""digest_before":{db},"digest_after":{da},"#,
                r#""undo_before":{ub},"undo_after":{ua},"#,
                r#""redo_before":{rdb},"redo_after":{rda},"#,
                r#""measured":{measured},"measured_state_unchanged":{unchanged}}}"#
            ),
            request = self.request,
            command = quoted(&self.command),
            arguments = quoted(&self.arguments),
            outcome = outcome,
            reason = quoted(&reason),
            status = quoted(&self.status),
            rb = self.before.revision,
            ra = self.after.revision,
            bb = self.before.bytes,
            ba = self.after.bytes,
            db = quoted(&self.before.digest),
            da = quoted(&self.after.digest),
            ub = self.before.can_undo,
            ua = self.after.can_undo,
            rdb = self.before.can_redo,
            rda = self.after.can_redo,
            measured = quoted(MEASURED_FIELDS),
            unchanged = self.measured_state_unchanged(),
        )
    }
}

#[must_use]
pub fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if (other as u32) < 0x20 => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", other as u32);
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

pub const LEDGER_LIMIT: usize = 4096;

#[derive(Clone, Debug, Default)]
pub struct Ledger {
    records: Vec<CommandRecord>,
    forgotten: u64,
    requests: u64,
}

impl Ledger {
    pub const fn next_request(&mut self) -> u64 {
        self.requests += 1;
        self.requests
    }

    pub fn push(&mut self, record: CommandRecord) {
        if self.records.len() >= LEDGER_LIMIT {
            self.records.remove(0);
            self.forgotten += 1;
        }
        self.records.push(record);
    }

    #[must_use]
    pub fn records(&self) -> &[CommandRecord] {
        &self.records
    }

    #[must_use]
    pub const fn forgotten(&self) -> u64 {
        self.forgotten
    }

    pub fn take(&mut self) -> Vec<CommandRecord> {
        std::mem::take(&mut self.records)
    }
}

#[cfg(test)]
mod ledger_tests {
    use super::{
        CommandRecord, DocumentState, LEDGER_LIMIT, Ledger, MEASURED_FIELDS, Outcome, quoted,
    };

    fn state(revision: u64, digest: &str) -> DocumentState {
        DocumentState {
            revision,
            bytes: 10,
            digest: digest.to_owned(),
            can_undo: false,
            can_redo: false,
        }
    }

    fn record(outcome: Outcome, before: DocumentState, after: DocumentState) -> CommandRecord {
        CommandRecord {
            request: 1,
            command: "Type".to_owned(),
            arguments: "page=0 line=0 from=1 to=1".to_owned(),
            before,
            after,
            outcome,
            status: "ok".to_owned(),
        }
    }

    #[test]
    fn a_refusal_that_left_the_bytes_alone_is_unchanged_and_one_that_did_not_is_reported() {
        let refused = record(
            Outcome::Refused("no".to_owned()),
            state(3, "aa"),
            state(3, "aa"),
        );
        assert!(refused.measured_state_unchanged());
        let leaked = record(
            Outcome::Refused("no".to_owned()),
            state(3, "aa"),
            state(4, "bb"),
        );
        assert!(!leaked.measured_state_unchanged());
        let stepped = record(Outcome::Unchanged, state(3, "aa"), state(4, "aa"));
        assert!(!stepped.measured_state_unchanged());
    }

    #[test]
    fn an_unmeasured_end_is_never_reported_as_unchanged() {
        let unread = record(
            Outcome::Dropped("an edit was already running".to_owned()),
            state(0, ""),
            state(0, ""),
        );
        assert!(!unread.measured_state_unchanged());
        let line = unread.json();
        assert!(
            line.contains(r#""measured_state_unchanged":false"#),
            "{line}"
        );
        assert!(line.contains(r#""outcome":"dropped""#), "{line}");
    }

    #[test]
    fn a_change_of_what_history_offers_counts_as_a_change_of_the_measured_state() {
        let mut after = state(3, "aa");
        after.can_undo = true;
        let record = record(Outcome::Refused("no".to_owned()), state(3, "aa"), after);
        assert!(!record.measured_state_unchanged());
    }

    #[test]
    fn a_record_names_the_fields_its_claim_is_made_over() {
        let line = record(Outcome::Unchanged, state(3, "aa"), state(3, "aa")).json();
        assert!(line.contains(r#""schema":2"#), "{line}");
        assert!(
            line.contains(&format!(r#""measured":"{MEASURED_FIELDS}""#)),
            "{line}"
        );
        assert!(
            !line.contains("document_untouched"),
            "the wider claim is gone: {line}"
        );
        for field in MEASURED_FIELDS.split(',') {
            assert!(
                matches!(
                    field,
                    "bytes" | "digest" | "revision" | "undo_available" | "redo_available"
                ),
                "an unmeasured field was announced as measured: {field}"
            );
        }
    }

    #[test]
    fn every_json_escape_survives_the_text_a_person_can_type() {
        assert_eq!(quoted("a\"b"), "\"a\\\"b\"");
        assert_eq!(quoted("a\\b"), "\"a\\\\b\"");
        assert_eq!(quoted("a\nb"), "\"a\\nb\"");
        assert_eq!(quoted("a\tb"), "\"a\\tb\"");
        assert_eq!(quoted("a\u{1}b"), "\"a\\u0001b\"");
        assert_eq!(quoted("\u{e01}"), "\"\u{e01}\"");
    }

    #[test]
    fn a_records_json_line_reports_the_outcome_and_both_digests() {
        let line = record(
            Outcome::Refused("cannot type here".to_owned()),
            state(3, "aa"),
            state(3, "aa"),
        )
        .json();
        assert!(line.contains(r#""outcome":"refused""#), "{line}");
        assert!(line.contains(r#""reason":"cannot type here""#), "{line}");
        assert!(line.contains(r#""digest_before":"aa""#), "{line}");
        assert!(
            line.contains(r#""measured_state_unchanged":true"#),
            "{line}"
        );
        assert!(!line.contains('\n'), "a record is one line: {line}");
    }

    #[test]
    fn a_changed_record_names_its_page_and_region() {
        let line = record(
            Outcome::Changed {
                page: 2,
                region: Some("[1, 2, 3, 4]".to_owned()),
            },
            state(3, "aa"),
            state(4, "bb"),
        )
        .json();
        assert!(line.contains(r#""outcome":"changed","page":2"#), "{line}");
        assert!(line.contains(r#""region":"[1, 2, 3, 4]""#), "{line}");
        assert!(
            line.contains(r#""measured_state_unchanged":false"#),
            "{line}"
        );
    }

    #[test]
    fn request_numbers_count_every_request_and_the_bound_forgets_the_oldest() {
        let mut ledger = Ledger::default();
        assert_eq!(ledger.next_request(), 1);
        assert_eq!(ledger.next_request(), 2);
        for index in 0..LEDGER_LIMIT + 3 {
            let mut kept = record(Outcome::Unchanged, state(0, "aa"), state(0, "aa"));
            kept.request = index as u64;
            ledger.push(kept);
        }
        assert_eq!(ledger.records().len(), LEDGER_LIMIT);
        assert_eq!(ledger.forgotten(), 3);
        assert_eq!(ledger.records()[0].request, 3);
        assert_eq!(ledger.take().len(), LEDGER_LIMIT);
        assert!(ledger.records().is_empty());
    }
}
