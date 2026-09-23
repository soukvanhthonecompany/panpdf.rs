#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Way {
    Up,
    Down,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Recall {
    at: Option<usize>,
    draft: String,
}

impl Recall {
    pub fn forget(&mut self) {
        *self = Self::default();
    }

    #[must_use]
    pub fn showing(&self, said: &[String], in_the_box: &str) -> bool {
        self.at
            .and_then(|at| said.get(at))
            .is_some_and(|recalled| recalled == in_the_box)
    }

    pub fn step(
        &mut self,
        said: &[String],
        in_the_box: &str,
        caret_at_start: bool,
        way: Way,
    ) -> Option<String> {
        let showing = self.showing(said, in_the_box);
        if !showing {
            self.at = None;
        }
        match way {
            Way::Up => {
                let next = if let Some(at) = self.at {
                    at + 1
                } else if in_the_box.is_empty() || caret_at_start {
                    0
                } else {
                    return None;
                };
                let recalled = said.get(next)?;
                if self.at.is_none() {
                    in_the_box.clone_into(&mut self.draft);
                }
                self.at = Some(next);
                Some(recalled.clone())
            }
            Way::Down => {
                let at = self.at?;
                if at == 0 {
                    self.at = None;
                    return Some(std::mem::take(&mut self.draft));
                }
                self.at = Some(at - 1);
                said.get(at - 1).cloned()
            }
        }
    }
}

#[must_use]
pub fn distinct(newest_first: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for question in newest_first {
        if question.trim().is_empty() {
            continue;
        }
        if out.last() != Some(&question) {
            out.push(question);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Recall, Way, distinct};

    fn asked() -> Vec<String> {
        distinct(
            ["third", "second", "second", "", "first"]
                .into_iter()
                .map(str::to_owned),
        )
    }

    #[test]
    fn up_goes_back_and_down_comes_home() {
        let said = asked();
        assert_eq!(said, ["third", "second", "first"]);
        let mut recall = Recall::default();
        let mut shown = String::new();
        for want in ["third", "second", "first"] {
            shown = recall
                .step(&said, &shown, false, Way::Up)
                .expect("an older one");
            assert_eq!(shown, want);
        }
        assert_eq!(recall.step(&said, &shown, false, Way::Up), None);
        for want in ["second", "third", ""] {
            shown = recall
                .step(&said, &shown, false, Way::Down)
                .expect("a newer one");
            assert_eq!(shown, want);
        }
        assert_eq!(recall.step(&said, &shown, false, Way::Down), None);
    }

    #[test]
    fn a_draft_is_kept_and_its_caret_keeps_the_key() {
        let said = asked();
        let mut recall = Recall::default();
        assert_eq!(recall.step(&said, "half a thought", false, Way::Up), None);
        let shown = recall
            .step(&said, "half a thought", true, Way::Up)
            .expect("recalled from the start");
        assert_eq!(shown, "third");
        assert_eq!(
            recall.step(&said, &shown, false, Way::Down).as_deref(),
            Some("half a thought")
        );
    }

    #[test]
    fn a_changed_recall_is_the_persons_own() {
        let said = asked();
        let mut recall = Recall::default();
        let shown = recall.step(&said, "", false, Way::Up).expect("third");
        assert!(recall.showing(&said, &shown));
        let changed = format!("{shown}, please");
        assert!(!recall.showing(&said, &changed));
        assert_eq!(recall.step(&said, &changed, false, Way::Up), None);
        assert_eq!(recall.step(&said, &changed, false, Way::Down), None);
    }

    #[test]
    fn with_nothing_asked_up_is_the_carets() {
        let mut recall = Recall::default();
        assert_eq!(recall.step(&[], "", true, Way::Up), None);
        assert_eq!(recall.step(&[], "", true, Way::Down), None);
    }
}
