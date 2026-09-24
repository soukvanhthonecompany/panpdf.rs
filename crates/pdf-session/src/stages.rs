#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage {
    Read,
    Plan,
    Write,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stages {
    pub read: f64,
    pub plan: f64,
    pub write: f64,
    pub total: f64,
}

impl Stages {
    #[must_use]
    pub fn rest(&self) -> f64 {
        (self.total - self.read - self.plan - self.write).max(0.0)
    }

    #[must_use]
    pub fn logged(&self) -> String {
        format!(
            "edit {:.0} ms: read {:.0} plan {:.0} write {:.0} rest {:.0}",
            self.total,
            self.read,
            self.plan,
            self.write,
            self.rest()
        )
    }

    fn add(&mut self, stage: Stage, ms: f64) {
        match stage {
            Stage::Read => self.read += ms,
            Stage::Plan => self.plan += ms,
            Stage::Write => self.write += ms,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Open {
    stage: Stage,
    started: f64,
    inside: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Stopwatch {
    clock: Option<fn() -> f64>,
    began: Option<f64>,
    open: Vec<Open>,
    stages: Stages,
}

impl Stopwatch {
    #[must_use]
    pub fn reading(clock: fn() -> f64) -> Self {
        Self {
            clock: Some(clock),
            ..Self::default()
        }
    }

    fn now(&self) -> Option<f64> {
        self.clock.map(|clock| clock())
    }

    pub fn begin(&mut self) {
        self.open.clear();
        self.stages = Stages::default();
        self.began = self.now();
    }

    pub fn end(&mut self) -> Option<Stages> {
        let (began, now) = (self.began.take()?, self.now()?);
        let mut stages = self.stages;
        stages.total = (now - began).max(0.0);
        Some(stages)
    }

    pub fn enter(&mut self, stage: Stage) {
        if let Some(started) = self.now() {
            self.open.push(Open {
                stage,
                started,
                inside: 0.0,
            });
        }
    }

    pub fn leave(&mut self) {
        let (Some(open), Some(now)) = (self.open.pop(), self.now()) else {
            return;
        };
        let took = (now - open.started).max(0.0);
        self.stages.add(open.stage, (took - open.inside).max(0.0));
        if let Some(caller) = self.open.last_mut() {
            caller.inside += took;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::{Stage, Stages, Stopwatch};

    thread_local! {
        static NOW: Cell<f64> = const { Cell::new(0.0) };
    }

    fn clock() -> f64 {
        NOW.with(Cell::get)
    }

    fn at(ms: f64) {
        NOW.with(|now| now.set(ms));
    }

    #[test]
    fn an_inner_call_is_counted_once_in_its_own_stage() {
        at(0.0);
        let mut watch = Stopwatch::reading(clock);
        watch.begin();
        at(10.0);
        watch.enter(Stage::Plan);
        at(15.0);
        watch.enter(Stage::Read);
        at(35.0);
        watch.leave();
        at(40.0);
        watch.leave();
        at(70.0);
        watch.enter(Stage::Write);
        at(85.0);
        watch.leave();
        at(100.0);
        let stages = watch.end().expect("an edit was timed");
        assert_eq!(
            stages,
            Stages {
                read: 20.0,
                plan: 10.0,
                write: 15.0,
                total: 100.0,
            }
        );
        assert!((stages.rest() - 55.0).abs() < 1e-9);
        assert_eq!(
            stages.logged(),
            "edit 100 ms: read 20 plan 10 write 15 rest 55"
        );
    }

    #[test]
    fn without_a_clock_there_is_nothing_to_say() {
        let mut watch = Stopwatch::default();
        watch.begin();
        watch.enter(Stage::Read);
        watch.leave();
        assert_eq!(watch.end(), None);
    }

    #[test]
    fn each_edit_starts_from_nothing() {
        at(0.0);
        let mut watch = Stopwatch::reading(clock);
        watch.enter(Stage::Read);
        at(50.0);
        watch.leave();
        watch.begin();
        at(60.0);
        let stages = watch.end().expect("timed");
        assert!((stages.read).abs() < 1e-9 && (stages.total - 10.0).abs() < 1e-9);
    }
}
