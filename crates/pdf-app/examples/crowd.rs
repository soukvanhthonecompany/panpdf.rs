#![allow(
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    clippy::struct_field_names,
    reason = "a stress harness: long checking functions read top to bottom, and its numbers are counts and millipoints far inside f64"
)]

use std::collections::{BTreeMap, HashMap};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use pdf_app::{Applied, Editor};
use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::BlockRange;
use pdf_paint::Matrix;

const WATCHDOG_SECONDS: u64 = 180;

const TURN_TOLERANCE: f64 = 0.02;

const MARGIN_SLACK: f64 = 2.0;

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = arguments.first() else {
        eprintln!("usage: crowd file.pdf page seed [steps] | crowd file.pdf --scan from to");
        std::process::exit(2);
    };
    let bytes = std::fs::read(path).expect("the file reads");
    let source = ByteStore::new(SourceId::new(7), Arc::<[u8]>::from(bytes));
    if arguments.get(1).map(String::as_str) == Some("--scan") {
        let from: usize = arguments.get(2).and_then(|v| v.parse().ok()).unwrap_or(0);
        let to: usize = arguments
            .get(3)
            .and_then(|v| v.parse().ok())
            .unwrap_or(from);
        scan(path, &source, from, to);
        return;
    }
    let page: usize = arguments.get(1).and_then(|v| v.parse().ok()).unwrap_or(0);
    let seed: u64 = arguments.get(2).and_then(|v| v.parse().ok()).unwrap_or(1);
    let steps: usize = arguments.get(3).and_then(|v| v.parse().ok()).unwrap_or(500);
    let Some(mut run) = Run::open(source, page, seed) else {
        println!("{path} page {page}: does not read");
        std::process::exit(2);
    };
    println!(
        "{path} page {page} seed {seed}: {} blocks, {} objects ({})",
        run.shot().blocks.len(),
        run.shot().objects.len(),
        run.kinds()
    );
    let watchdog = Watchdog::start();
    run.crowd(steps, &watchdog);
    run.report(path, page, seed);
}

fn scan(path: &str, source: &ByteStore, from: usize, to: usize) {
    let Ok(mut editor) = open(source.clone()) else {
        println!("{path}: does not open");
        return;
    };
    let count = editor.page_count();
    for page in from..=to.min(count.saturating_sub(1)) {
        if !read(&mut editor, page) {
            println!("{path} page {page}: does not read");
            continue;
        }
        let leaf = editor.leaf(page).expect("just read");
        let blocks = leaf.overlay.blocks.len();
        let pictures = leaf
            .overlay
            .objects
            .iter()
            .filter(|object| matches!(object.kind, pdf_semantics::ObjectKind::Image))
            .count();
        let drawings = leaf
            .overlay
            .objects
            .iter()
            .filter(|object| matches!(object.kind, pdf_semantics::ObjectKind::Path))
            .count();
        let words: usize = every_block(&editor, page)
            .iter()
            .map(|text| text.as_ref().map_or(0, |text| text.chars().count()))
            .sum();
        println!(
            "{path} page {page}: {blocks} blocks, {pictures} pictures, {drawings} drawings, {words} characters"
        );
    }
}

fn open(source: ByteStore) -> Result<Editor, String> {
    let mut editor = Editor::open(source)?;
    if editor.editing_restricted() && !editor.set_aside_restrictions() {
        return Err("restrictions could not be set aside".to_owned());
    }
    Ok(editor)
}

fn read(editor: &mut Editor, page: usize) -> bool {
    let Some(source) = editor.source() else {
        return false;
    };
    match pdf_session::interpret_page_fully(
        source,
        page,
        b"",
        editor.grouping(page).as_deref(),
        pdf_cli::font_provider(),
    ) {
        Ok(view) => {
            editor.adopt_page(page, Arc::new(view));
            true
        }
        Err(_) => false,
    }
}

fn last_stop(editor: &Editor, page: usize, block: usize) -> Option<(usize, usize)> {
    let overlay = &editor.leaf(page)?.overlay;
    let lines = &overlay.blocks.get(block)?.lines;
    let row = lines.len().checked_sub(1)?;
    let offset = overlay
        .carets
        .iter()
        .filter(|stop| stop.line == lines[row])
        .map(|stop| stop.offset)
        .max()?;
    Some((row, offset))
}

fn whole(editor: &Editor, page: usize, block: usize) -> Option<String> {
    let last = last_stop(editor, page, block)?;
    if last == (0, 0) {
        return Some(String::new());
    }
    editor.copy_text(page, block, (0, 0), last)
}

fn plain(text: &str) -> String {
    pdf_edit::in_compatibility_form(text)
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn every_block(editor: &Editor, page: usize) -> Vec<Option<String>> {
    let count = editor
        .leaf(page)
        .map_or(0, |leaf| leaf.overlay.blocks.len());
    (0..count)
        .map(|block| whole(editor, page, block).map(|text| plain(&text)))
        .collect()
}

struct Dice(u64);

impl Dice {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        usize::try_from(self.next() % n as u64).unwrap_or(0)
    }

    #[allow(clippy::cast_precision_loss)]
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn between(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.unit()
    }

    fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }
}

fn milli(value: f64) -> i64 {
    #[allow(clippy::cast_possible_truncation)]
    let rounded = (value * 1000.0).round() as i64;
    rounded
}

fn milli_box(rect: [f64; 4]) -> [i64; 4] {
    rect.map(milli)
}

fn milli_quad(quad: [[f64; 2]; 4]) -> [[i64; 2]; 4] {
    quad.map(|corner| corner.map(milli))
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct BlockShot {
    text: Option<String>,
    frame: [i64; 4],
    quad: [[i64; 2]; 4],
    layout: [i64; 4],
    turn: i64,
}

impl BlockShot {
    fn rectangle(&self) -> [[f64; 2]; 4] {
        if self.turn == 0 {
            let [x0, y0, x1, y1] = self.layout.map(|v| v as f64 / 1000.0);
            [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
        } else {
            unmilli(self.quad)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ObjectShot {
    kind: String,
    quad: [[i64; 2]; 4],
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Shot {
    blocks: Vec<BlockShot>,
    objects: Vec<ObjectShot>,
    atoms: usize,
}

impl Shot {
    fn texts(&self) -> Vec<Option<String>> {
        self.blocks.iter().map(|block| block.text.clone()).collect()
    }

    fn characters(&self) -> usize {
        self.blocks
            .iter()
            .map(|block| block.text.as_ref().map_or(0, |text| text.chars().count()))
            .sum()
    }

    fn sorted_blocks(&self) -> Vec<BlockShot> {
        let mut blocks = self.blocks.clone();
        blocks.sort();
        blocks
    }

    fn sorted_objects(&self) -> Vec<ObjectShot> {
        let mut objects = self.objects.clone();
        objects.sort();
        objects
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Op {
    MoveBlockOnto,
    MoveObjectOnto,
    ShrinkFrame,
    GrowFrame,
    TurnBlock,
    TurnObject,
    TurnBack,
    FlowOn,
    FlowOff,
    TypeInto,
    Backspace,
    GroupMove,
    GroupDelete,
    CopyPaste,
    Reorder,
    UndoRedoBurst,
}

impl Op {
    const ALL: [Self; 16] = [
        Self::MoveBlockOnto,
        Self::MoveObjectOnto,
        Self::ShrinkFrame,
        Self::GrowFrame,
        Self::TurnBlock,
        Self::TurnObject,
        Self::TurnBack,
        Self::FlowOn,
        Self::FlowOff,
        Self::TypeInto,
        Self::Backspace,
        Self::GroupMove,
        Self::GroupDelete,
        Self::CopyPaste,
        Self::Reorder,
        Self::UndoRedoBurst,
    ];

    const fn weight(self) -> u64 {
        match self {
            Self::MoveBlockOnto | Self::MoveObjectOnto => 10,
            Self::ShrinkFrame | Self::TurnBlock | Self::TurnObject | Self::TypeInto => 7,
            Self::TurnBack | Self::FlowOn | Self::CopyPaste | Self::Reorder | Self::GroupMove => 5,
            Self::GrowFrame | Self::FlowOff | Self::Backspace | Self::UndoRedoBurst => 4,
            Self::GroupDelete => 2,
        }
    }

    fn draw(dice: &mut Dice) -> Self {
        let total: u64 = Self::ALL.iter().map(|op| op.weight()).sum();
        let mut pick = dice.next() % total;
        for op in Self::ALL {
            if pick < op.weight() {
                return op;
            }
            pick -= op.weight();
        }
        Self::MoveBlockOnto
    }

    const fn touches_text(self) -> bool {
        matches!(
            self,
            Self::TypeInto
                | Self::Backspace
                | Self::GroupDelete
                | Self::CopyPaste
                | Self::ShrinkFrame
                | Self::GrowFrame
        )
    }
}

struct Watchdog {
    started: Arc<AtomicU64>,
    step: Arc<AtomicUsize>,
    zero: Instant,
}

impl Watchdog {
    fn start() -> Self {
        let started = Arc::new(AtomicU64::new(u64::MAX));
        let step = Arc::new(AtomicUsize::new(0));
        let zero = Instant::now();
        let (watch_started, watch_step) = (Arc::clone(&started), Arc::clone(&step));
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                let began = watch_started.load(Ordering::Relaxed);
                if began == u64::MAX {
                    continue;
                }
                let now = zero.elapsed().as_secs();
                if now.saturating_sub(began) > WATCHDOG_SECONDS {
                    println!(
                        "HANG: step {} has run for more than {WATCHDOG_SECONDS} s",
                        watch_step.load(Ordering::Relaxed)
                    );
                    std::process::exit(4);
                }
            }
        });
        Self {
            started,
            step,
            zero,
        }
    }

    fn begin(&self, step: usize) {
        self.step.store(step, Ordering::Relaxed);
        self.started
            .store(self.zero.elapsed().as_secs(), Ordering::Relaxed);
    }

    fn end(&self) {
        self.started.store(u64::MAX, Ordering::Relaxed);
    }
}

fn rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmRSS:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<u64>().ok())
        })
        .map_or(0, |kb| kb / 1024)
}

fn normalise(reason: &str) -> String {
    let mut out = String::new();
    let mut in_number = false;
    for character in reason.chars() {
        if character.is_ascii_digit() || (in_number && character == '.') {
            if !in_number {
                out.push('#');
                in_number = true;
            }
        } else {
            in_number = false;
            out.push(character);
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Thing {
    Block(usize),
    Object(usize),
}

struct Turned {
    angle: f64,
    quad: [[f64; 2]; 4],
    about: (f64, f64),
}

struct Run {
    editor: Editor,
    page: usize,
    dice: Dice,
    start: Shot,
    tally: BTreeMap<String, usize>,
    refusals: BTreeMap<String, usize>,
    outcomes: BTreeMap<(Op, &'static str), usize>,
    turned: HashMap<Thing, Turned>,
    turn_sign: Option<f64>,
    durations: Vec<(usize, Op, u128)>,
    rss: Vec<(usize, u64)>,
    stopped: Option<String>,
    steps_run: usize,
    trace: bool,
}

impl Run {
    fn open(source: ByteStore, page: usize, seed: u64) -> Option<Self> {
        let mut editor = open(source).ok()?;
        if !read(&mut editor, page) {
            return None;
        }
        let mut run = Self {
            editor,
            page,
            dice: Dice::new(seed),
            start: Shot {
                blocks: Vec::new(),
                objects: Vec::new(),
                atoms: 0,
            },
            tally: BTreeMap::new(),
            refusals: BTreeMap::new(),
            outcomes: BTreeMap::new(),
            turned: HashMap::new(),
            turn_sign: None,
            durations: Vec::new(),
            rss: Vec::new(),
            stopped: None,
            steps_run: 0,
            trace: std::env::var_os("CROWD_TRACE").is_some(),
        };
        run.start = run.shot();
        Some(run)
    }

    fn kinds(&self) -> String {
        let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
        for object in &self.editor.leaf(self.page).expect("read").overlay.objects {
            *kinds.entry(format!("{:?}", object.kind)).or_default() += 1;
        }
        kinds
            .iter()
            .map(|(kind, count)| format!("{count} {kind}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn ensure_read(&mut self) -> bool {
        if self.editor.leaf(self.page).is_some() {
            return true;
        }
        read(&mut self.editor, self.page)
    }

    fn shot(&mut self) -> Shot {
        if !self.ensure_read() {
            return Shot {
                blocks: Vec::new(),
                objects: Vec::new(),
                atoms: usize::MAX,
            };
        }
        let texts = every_block(&self.editor, self.page);
        let leaf = self.editor.leaf(self.page).expect("read");
        let frames = self.editor.frame_boxes(self.page);
        let blocks = leaf
            .overlay
            .blocks
            .iter()
            .enumerate()
            .map(|(index, block)| BlockShot {
                text: texts.get(index).cloned().flatten(),
                frame: frames.get(index).copied().map_or([0; 4], milli_box),
                quad: milli_quad(block.quad),
                layout: milli_box(block.layout_pixels),
                turn: milli(block.turn.to_degrees()),
            })
            .collect();
        let objects = leaf
            .overlay
            .objects
            .iter()
            .map(|object| ObjectShot {
                kind: format!("{:?}", object.kind),
                quad: milli_quad(object.quad),
            })
            .collect();
        Shot {
            blocks,
            objects,
            atoms: leaf.view.graph.atoms.len(),
        }
    }

    fn violation(&mut self, step: usize, op: Op, cause: &str, detail: &str) {
        *self.tally.entry(cause.to_owned()).or_default() += 1;
        println!("step {step} {op:?}: {cause}: {detail}");
    }

    fn block_count(&self) -> usize {
        self.editor
            .leaf(self.page)
            .map_or(0, |leaf| leaf.overlay.blocks.len())
    }

    fn object_count(&self) -> usize {
        self.editor
            .leaf(self.page)
            .map_or(0, |leaf| leaf.overlay.objects.len())
    }

    fn block_with_text(&mut self) -> Option<usize> {
        let texts = every_block(&self.editor, self.page);
        let candidates: Vec<usize> = texts
            .iter()
            .enumerate()
            .filter(|(_, text)| text.as_ref().is_some_and(|text| !text.is_empty()))
            .map(|(block, _)| block)
            .collect();
        (!candidates.is_empty()).then(|| candidates[self.dice.below(candidates.len())])
    }

    fn object(&mut self) -> Option<usize> {
        let count = self.object_count();
        (count > 0).then(|| self.dice.below(count))
    }

    fn picture_or_drawing(&mut self) -> Option<usize> {
        let leaf = self.editor.leaf(self.page)?;
        let candidates: Vec<usize> = leaf
            .overlay
            .objects
            .iter()
            .enumerate()
            .filter(|(_, object)| {
                matches!(
                    object.kind,
                    pdf_semantics::ObjectKind::Image | pdf_semantics::ObjectKind::Path
                )
            })
            .map(|(index, _)| index)
            .collect();
        (!candidates.is_empty()).then(|| candidates[self.dice.below(candidates.len())])
    }

    fn block_box(&self, block: usize) -> Option<[f64; 4]> {
        Some(
            self.editor
                .leaf(self.page)?
                .overlay
                .blocks
                .get(block)?
                .box_pixels,
        )
    }

    fn object_box(&self, object: usize) -> Option<[f64; 4]> {
        Some(
            self.editor
                .leaf(self.page)?
                .overlay
                .objects
                .get(object)?
                .box_pixels,
        )
    }

    fn anchors_of(&self, block: usize) -> Option<Vec<String>> {
        Some(
            self.editor
                .leaf(self.page)?
                .overlay
                .blocks
                .get(block)?
                .anchors
                .clone(),
        )
    }

    fn object_anchor(&self, object: usize) -> Option<String> {
        Some(
            self.editor
                .leaf(self.page)?
                .overlay
                .objects
                .get(object)?
                .anchor
                .clone(),
        )
    }

    fn target_box(&mut self) -> Option<[f64; 4]> {
        let blocks = self.block_count();
        let objects = self.object_count();
        if blocks + objects == 0 {
            return None;
        }
        let pick = self.dice.below(blocks + objects);
        if pick < blocks {
            self.block_box(pick)
        } else {
            self.object_box(pick - blocks)
        }
    }

    fn offset_onto(&mut self, from: [f64; 4], to: [f64; 4]) -> (f64, f64) {
        let jitter = self.dice.between(-12.0, 12.0);
        let jitter_y = self.dice.between(-12.0, 12.0);
        (
            (to[0] + to[2] - from[0] - from[2]) / 2.0 + jitter,
            (to[1] + to[3] - from[1] - from[3]) / 2.0 + jitter_y,
        )
    }

    fn crowd(&mut self, steps: usize, watchdog: &Watchdog) {
        self.rss.push((0, rss_mb()));
        for step in 1..=steps {
            if self.stopped.is_some() {
                break;
            }
            let op = Op::draw(&mut self.dice);
            watchdog.begin(step);
            let began = Instant::now();
            let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| self.step(step, op)));
            let took = began.elapsed().as_millis();
            watchdog.end();
            self.durations.push((step, op, took));
            self.steps_run = step;
            if step % 50 == 0 {
                self.rss.push((step, rss_mb()));
            }
            match outcome {
                Ok(()) => {}
                Err(panic) => {
                    let what = panic
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                        .unwrap_or_else(|| "(no message)".to_owned());
                    self.violation(step, op, "panic", &what);
                    self.stopped = Some(format!("panicked at step {step}: {what}"));
                }
            }
        }
        self.rss.push((self.steps_run, rss_mb()));
        if self.stopped.is_none() {
            self.walk_all_the_way_back();
        }
    }

    fn step(&mut self, step: usize, op: Op) {
        let before = self.shot();
        let before_flow: Vec<bool> = (0..before.blocks.len())
            .map(|block| self.editor.flows_round(self.page, block))
            .collect();
        let Some(done) = self.perform(step, op) else {
            *self.outcomes.entry((op, "nothing to do")).or_default() += 1;
            if self.trace {
                println!("  #{step} {op:?}: nothing to do");
            }
            return;
        };
        if self.trace {
            println!(
                "  #{step} {op:?} block {:?} object {:?}: {:?} -- {}",
                done.block,
                done.object,
                done.applied,
                self.editor.status()
            );
            if let Some(block) = done.block {
                let was = before.blocks.get(block);
                self.ensure_read();
                if std::env::var_os("CROWD_DUMP").is_some() {
                    self.dump_block(block);
                }
                let now = self
                    .editor
                    .leaf(self.page)
                    .and_then(|leaf| leaf.overlay.blocks.get(block).cloned());
                let frame = self.editor.frame_boxes(self.page).get(block).copied();
                println!(
                    "     before: frame {:?} quad0 {:?}\n     after:  frame {:?} box {:?} layout {:?} turn {:?}",
                    was.map(|b| b.frame),
                    was.map(|b| b.quad[0]),
                    frame.map(|f| f.map(|v| (v * 10.0).round() / 10.0)),
                    now.as_ref()
                        .map(|b| b.box_pixels.map(|v| (v * 10.0).round() / 10.0)),
                    now.as_ref()
                        .map(|b| b.layout_pixels.map(|v| (v * 10.0).round() / 10.0)),
                    now.as_ref().map(|b| b.turn)
                );
            }
        }
        match &done.applied {
            Applied::Changed { .. } => {
                *self.outcomes.entry((op, "committed")).or_default() += 1;
            }
            Applied::Refused(refusal) => {
                *self.outcomes.entry((op, "refused")).or_default() += 1;
                let reason = refusal.to_string();
                let reason = reason.as_str();
                if reason.trim().is_empty() {
                    self.violation(step, op, "refused without a reason", "");
                }
                *self.refusals.entry(normalise(reason)).or_default() += 1;
                let status = self.editor.status().to_string();
                if status.trim().is_empty() {
                    self.violation(step, op, "refused with an empty status", reason);
                }
                let mut after = self.shot();
                let mut was = before.clone();
                if done.frames_only {
                    for shot in after.blocks.iter_mut().chain(was.blocks.iter_mut()) {
                        shot.frame = [0; 4];
                        shot.quad = [[0; 2]; 4];
                    }
                }
                if after != was {
                    self.violation(
                        step,
                        op,
                        "a refusal changed the page",
                        &format!("{reason}: {}", differences(&was, &after)),
                    );
                }
                return;
            }
            Applied::Unchanged => {
                *self.outcomes.entry((op, "unchanged")).or_default() += 1;
                let after = self.shot();
                if after != before && !done.frames_only {
                    self.violation(step, op, "'nothing changed' but the page changed", "");
                }
                if !done.frames_only {
                    return;
                }
            }
        }
        if !self.ensure_read() {
            self.violation(step, op, "the page no longer reads", "");
            self.stopped = Some(format!("page unreadable after step {step}"));
            return;
        }
        let after = self.shot();
        self.check_texts(step, op, &before, &after, &done);
        self.check_geometry(step, op, &before, &after, &done);
        self.check_flow(step, op);
        self.check_undo_redo(step, op, &before, &after, &before_flow);
    }

    #[allow(clippy::too_many_lines)]
    fn perform(&mut self, step: usize, op: Op) -> Option<Done> {
        let page = self.page;
        match op {
            Op::MoveBlockOnto => {
                let block = self.block_with_text()?;
                let from = self.block_box(block)?;
                let to = self.target_box()?;
                let (dx, dy) = self.offset_onto(from, to);
                self.turned.remove(&Thing::Block(block));
                let applied = self.editor.move_text_block(page, block, dx, dy);
                Some(Done::of(applied).about_block(block))
            }
            Op::MoveObjectOnto => {
                let object = self.picture_or_drawing()?;
                let from = self.object_box(object)?;
                let to = self.target_box()?;
                let (dx, dy) = self.offset_onto(from, to);
                let anchor = self.object_anchor(object)?;
                self.turned.remove(&Thing::Object(object));
                let applied = self.editor.place(page, &anchor, dx, dy);
                Some(Done::of(applied).about_object(object))
            }
            Op::ShrinkFrame | Op::GrowFrame => {
                let block = self.block_with_text()?;
                let frame = *self.editor.frame_boxes(page).get(block)?;
                let width = frame[2] - frame[0];
                let factor = if op == Op::ShrinkFrame {
                    self.dice.between(0.15, 0.7)
                } else {
                    self.dice.between(1.2, 2.5)
                };
                let new_width = (width * factor).max(12.0);
                let wanted = [frame[0], frame[1], frame[0] + new_width, frame[3]];
                self.turned.remove(&Thing::Block(block));
                let nudged = [frame[0], frame[1], frame[2] + 1.0, frame[3]];
                self.editor.preview_frame(page, block, nudged);
                self.editor.finish_frame_resize(page, block, frame);
                self.editor.preview_frame(page, block, wanted);
                self.editor.finish_frame_resize(page, block, nudged);
                let got = *self.editor.frame_boxes(page).get(block)?;
                if got.map(milli) != wanted.map(milli) {
                    self.violation(
                        step,
                        op,
                        "the frame is not the rectangle dragged",
                        &format!("wanted {wanted:?}, got {got:?}"),
                    );
                }
                let end = last_stop(&self.editor, page, block)?;
                let applied =
                    self.editor
                        .edit(page, block, BlockRange::Between { from: end, to: end }, "x");
                Some(
                    Done::of(applied)
                        .about_block(block)
                        .with_frames()
                        .typed("x"),
                )
            }
            Op::TurnBlock => {
                let block = self.block_with_text()?;
                let anchors = self.anchors_of(block)?;
                let shot = self.editor.leaf(page)?.overlay.blocks.get(block)?.clone();
                let quad = if shot.turn == 0.0 {
                    let [x0, y0, x1, y1] = shot.layout_pixels;
                    [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
                } else {
                    shot.quad
                };
                let angle = self.angle();
                let matrix = rotation(angle);
                let about = centre(quad);
                let job = self
                    .editor
                    .begin_shape_block(page, &anchors, matrix, about)?;
                let applied = self.editor.adopt(job.run());
                if matches!(applied, Applied::Changed { .. }) {
                    let entry = self.turned.entry(Thing::Block(block)).or_insert(Turned {
                        angle: 0.0,
                        quad,
                        about,
                    });
                    entry.angle += angle;
                }
                Some(Done::of(applied).about_block(block).turned(angle))
            }
            Op::TurnObject => {
                let object = self.picture_or_drawing()?;
                let anchor = self.object_anchor(object)?;
                let quad = self.editor.leaf(page)?.overlay.objects.get(object)?.quad;
                let angle = self.angle();
                let matrix = rotation(angle);
                let about = centre(quad);
                let job = self.editor.begin_shape(page, &anchor, matrix, about)?;
                let applied = self.editor.adopt(job.run());
                if matches!(applied, Applied::Changed { .. }) {
                    let entry = self.turned.entry(Thing::Object(object)).or_insert(Turned {
                        angle: 0.0,
                        quad,
                        about,
                    });
                    entry.angle += angle;
                }
                Some(Done::of(applied).about_object(object).turned(angle))
            }
            Op::TurnBack => {
                let mut keys: Vec<Thing> = self
                    .turned
                    .iter()
                    .filter(|(thing, turned)| {
                        matches!(thing, Thing::Object(_)) && turned.angle.abs() > 1e-9
                    })
                    .map(|(thing, _)| *thing)
                    .collect();
                if self.turn_sign.is_some()
                    && let Some(leaf) = self.editor.leaf(page)
                {
                    keys.extend(
                        leaf.overlay
                            .blocks
                            .iter()
                            .enumerate()
                            .filter(|(_, block)| block.turn.abs() > 1e-9)
                            .map(|(block, _)| Thing::Block(block)),
                    );
                }
                if keys.is_empty() {
                    return None;
                }
                let thing = keys[self.dice.below(keys.len())];
                let remembered = self.turned.get(&thing);
                let (angle, reference, about) = match (thing, remembered) {
                    (Thing::Block(block), _) => {
                        let shot = self.editor.leaf(page)?.overlay.blocks.get(block)?.clone();
                        let sign = self.turn_sign?;
                        let angle = -shot.turn * sign;
                        match remembered {
                            Some(turned) => (angle, Some(turned.quad), turned.about),
                            None => (angle, None, centre(shot.quad)),
                        }
                    }
                    (Thing::Object(_), Some(turned)) => {
                        (-turned.angle, Some(turned.quad), turned.about)
                    }
                    (Thing::Object(_), None) => return None,
                };
                let matrix = rotation(angle);
                let applied = match thing {
                    Thing::Block(block) => {
                        let anchors = self.anchors_of(block)?;
                        let job = self
                            .editor
                            .begin_shape_block(page, &anchors, matrix, about)?;
                        self.editor.adopt(job.run())
                    }
                    Thing::Object(object) => {
                        let anchor = self.object_anchor(object)?;
                        let job = self.editor.begin_shape(page, &anchor, matrix, about)?;
                        self.editor.adopt(job.run())
                    }
                };
                if matches!(applied, Applied::Changed { .. }) {
                    self.turned.remove(&thing);
                }
                let done = match thing {
                    Thing::Block(block) => Done::of(applied).about_block(block),
                    Thing::Object(object) => Done::of(applied).about_object(object),
                };
                let done = done.turned(angle).levelled();
                Some(match reference {
                    Some(reference) => done.back_to(reference),
                    None => done,
                })
            }
            Op::FlowOn => {
                let object = self.picture_or_drawing()?;
                let over = self.object_box(object)?;
                let Some(job) = self.editor.begin_flow_round(page, over) else {
                    let status = self.editor.status().to_string();
                    return Some(Done::of(Applied::Refused(status.into())));
                };
                let applied = self.editor.adopt(job.run());
                Some(Done::of(applied))
            }
            Op::FlowOff => {
                let flowing: Vec<usize> = (0..self.block_count())
                    .filter(|block| self.editor.flows_round(page, *block))
                    .collect();
                if flowing.is_empty() {
                    return None;
                }
                let block = flowing[self.dice.below(flowing.len())];
                self.editor.set_flow_round(page, block, false);
                let applied = self.relay(block);
                Some(Done::of(applied).about_block(block))
            }
            Op::TypeInto => {
                let block = self.block_with_text()?;
                let end = last_stop(&self.editor, page, block)?;
                let words = ["ab", " x", "ทด", "Q", " end"];
                let typed = words[self.dice.below(words.len())];
                self.turned.remove(&Thing::Block(block));
                let applied = self.editor.edit(
                    page,
                    block,
                    BlockRange::Between { from: end, to: end },
                    typed,
                );
                Some(Done::of(applied).about_block(block).typed(typed))
            }
            Op::Backspace => {
                let block = self.block_with_text()?;
                let at = last_stop(&self.editor, page, block)?;
                self.turned.remove(&Thing::Block(block));
                let applied = self.editor.edit(
                    page,
                    block,
                    BlockRange::Units {
                        at,
                        backwards: true,
                        count: 1,
                    },
                    "",
                );
                Some(Done::of(applied).about_block(block).erased())
            }
            Op::GroupMove | Op::GroupDelete => {
                let (blocks, objects) = self.sweep()?;
                let anchors = self.group_anchors(&blocks)?;
                let names: Vec<String> = objects
                    .iter()
                    .filter_map(|object| self.object_anchor(*object))
                    .collect();
                self.turned.clear();
                if op == Op::GroupMove {
                    let dx = self.dice.between(-60.0, 60.0);
                    let dy = self.dice.between(-60.0, 60.0);
                    let applied = if names.is_empty() {
                        self.editor.move_block(page, &anchors, dx, dy)
                    } else {
                        self.editor.move_group(page, &anchors, &names, (dx, dy))
                    };
                    Some(Done::of(applied))
                } else {
                    let applied = self.editor.delete_the_group(page, &anchors, &names);
                    Some(Done::of(applied).deleted(blocks, objects.len()))
                }
            }
            Op::CopyPaste => {
                let take_block = self.dice.chance(0.5) || self.object_count() == 0;
                let (anchors, block, object) = if take_block {
                    let block = self.block_with_text()?;
                    (self.anchors_of(block)?, Some(block), None)
                } else {
                    let object = self.picture_or_drawing()?;
                    (vec![self.object_anchor(object)?], None, Some(object))
                };
                let copied = match self.editor.copy_objects(page, &anchors) {
                    Ok(copied) => copied,
                    Err(reason) => return Some(Done::of(Applied::Refused(reason.into()))),
                };
                let offset = if self.dice.chance(0.5) {
                    (0.0, 0.0)
                } else {
                    (
                        self.dice.between(-40.0, 40.0),
                        self.dice.between(-40.0, 40.0),
                    )
                };
                self.turned.clear();
                let applied = self.editor.paste_objects(page, copied, offset);
                let copied_characters = block.and_then(|block| {
                    every_block(&self.editor, page)
                        .get(block)
                        .cloned()
                        .flatten()
                        .map(|text| text.chars().count())
                });
                Some(Done::of(applied).pasted(copied_characters, object.is_some()))
            }
            Op::Reorder => {
                let anchors = if self.dice.chance(0.5) && self.object_count() > 0 {
                    let object = self.object()?;
                    vec![self.object_anchor(object)?]
                } else {
                    let block = self.block_with_text()?;
                    self.anchors_of(block)?
                };
                let orders = pdf_edit::Stacking::ALL;
                let order = orders[self.dice.below(orders.len())];
                self.turned.clear();
                let applied = self.editor.reorder_objects(page, &anchors, order);
                Some(Done::of(applied).reordered())
            }
            Op::UndoRedoBurst => {
                let count = 1 + self.dice.below(5);
                let mut undone = 0;
                for _ in 0..count {
                    if !self.editor.can_undo() {
                        break;
                    }
                    if let Applied::Refused(reason) = self.editor.undo() {
                        return Some(Done::of(Applied::Refused(format!("undo: {reason}").into())));
                    }
                    undone += 1;
                }
                for _ in 0..undone {
                    if !self.editor.can_redo() {
                        break;
                    }
                    if let Applied::Refused(reason) = self.editor.redo() {
                        return Some(Done::of(Applied::Refused(format!("redo: {reason}").into())));
                    }
                }
                Some(Done::of(Applied::Unchanged).burst())
            }
        }
    }

    fn dump_block(&self, block: usize) {
        let Some(leaf) = self.editor.leaf(self.page) else {
            return;
        };
        let Some(shot) = leaf.overlay.blocks.get(block) else {
            println!("     (no block {block})");
            return;
        };
        println!(
            "     block {block}: lines {:?}, last stop {:?}, whole {:?}",
            shot.lines,
            last_stop(&self.editor, self.page, block),
            whole(&self.editor, self.page, block).map(|t| t.chars().count())
        );
        for line in &shot.lines {
            let mut clusters: Vec<&pdf_cli::TextClusterBox> = leaf
                .overlay
                .clusters
                .iter()
                .filter(|cluster| cluster.line == *line)
                .collect();
            clusters.sort_by_key(|cluster| cluster.index_in_line);
            let said: Vec<String> = clusters
                .iter()
                .map(|cluster| {
                    format!(
                        "{}@{}{}",
                        cluster
                            .text
                            .clone()
                            .unwrap_or_else(|| "?".to_owned())
                            .escape_unicode(),
                        cluster
                            .box_pixels
                            .map_or("-".to_owned(), |b| format!("{:.2}", b[0])),
                        if cluster.stacked { "^" } else { "" }
                    )
                })
                .collect();
            println!("       line {line}: {}", said.join(" "));
        }
    }

    fn relay(&mut self, block: usize) -> Applied {
        let Some(end) = last_stop(&self.editor, self.page, block) else {
            return Applied::Refused("the block has no last caret stop".into());
        };
        self.editor.style(
            self.page,
            block,
            BlockRange::Between {
                from: (0, 0),
                to: end,
            },
            pdf_edit::TextStyle::default(),
        )
    }

    fn angle(&mut self) -> f64 {
        let choices = [5.0, 45.0, 90.0, 180.0, -30.0, 15.0];
        let degrees = if self.dice.chance(0.25) {
            self.dice.between(-179.0, 179.0)
        } else {
            choices[self.dice.below(choices.len())]
        };
        degrees.to_radians()
    }

    fn sweep(&mut self) -> Option<(Vec<usize>, Vec<usize>)> {
        let one = self.target_box()?;
        let two = self.target_box()?;
        let band = [
            one[0].min(two[0]) - 2.0,
            one[1].min(two[1]) - 2.0,
            one[2].max(two[2]) + 2.0,
            one[3].max(two[3]) + 2.0,
        ];
        let leaf = self.editor.leaf(self.page)?;
        let inside = |quad: &[[f64; 2]; 4]| {
            quad.iter()
                .all(|[x, y]| *x >= band[0] && *x <= band[2] && *y >= band[1] && *y <= band[3])
        };
        let blocks: Vec<usize> = leaf
            .overlay
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| inside(&block.quad))
            .map(|(index, _)| index)
            .collect();
        let objects: Vec<usize> = leaf
            .overlay
            .objects
            .iter()
            .enumerate()
            .filter(|(_, object)| inside(&object.quad))
            .map(|(index, _)| index)
            .collect();
        (blocks.len() + objects.len() >= 2).then_some((blocks, objects))
    }

    fn group_anchors(&self, blocks: &[usize]) -> Option<Vec<String>> {
        let mut anchors: Vec<String> = Vec::new();
        for block in blocks {
            for anchor in self.anchors_of(*block)? {
                if !anchors.contains(&anchor) {
                    anchors.push(anchor);
                }
            }
        }
        Some(anchors)
    }

    fn check_texts(&mut self, step: usize, op: Op, before: &Shot, after: &Shot, done: &Done) {
        let (was, now) = (before.texts(), after.texts());
        if !op.touches_text() {
            if was.len() != now.len() {
                self.violation(
                    step,
                    op,
                    "block count changed by a geometry step",
                    &format!("{} -> {}", was.len(), now.len()),
                );
            } else if was != now {
                let which: Vec<usize> = (0..was.len()).filter(|i| was[*i] != now[*i]).collect();
                self.violation(
                    step,
                    op,
                    "a block's text changed by a geometry step",
                    &format!(
                        "blocks {which:?}: {:?} -> {:?}",
                        pick(&was, &which),
                        pick(&now, &which)
                    ),
                );
            }
            return;
        }
        match &done.expect {
            Expect::Typed { block, typed } => {
                let wanted: Vec<Option<String>> = was
                    .iter()
                    .enumerate()
                    .map(|(index, text)| {
                        if index == *block {
                            text.as_ref().map(|text| format!("{text}{}", plain(typed)))
                        } else {
                            text.clone()
                        }
                    })
                    .collect();
                if wanted != now {
                    let which: Vec<usize> = (0..wanted.len().max(now.len()))
                        .filter(|i| wanted.get(*i) != now.get(*i))
                        .collect();
                    self.violation(
                        step,
                        op,
                        "typing did not read back as typed",
                        &format!(
                            "block {block} typed {typed:?}; differs at {which:?}: wanted {:?}, got {:?}",
                            pick(&wanted, &which),
                            pick(&now, &which)
                        ),
                    );
                }
            }
            Expect::Erased { block } => {
                if was.len() != now.len() {
                    self.violation(step, op, "backspace changed the block count", "");
                    return;
                }
                for index in 0..was.len() {
                    if index == *block {
                        let (Some(before_text), Some(after_text)) = (&was[index], &now[index])
                        else {
                            continue;
                        };
                        if !(after_text.chars().count() < before_text.chars().count()
                            || after_text == before_text)
                            || !before_text.starts_with(after_text.as_str())
                        {
                            self.violation(
                                step,
                                op,
                                "backspace did not take one letter off the end",
                                &format!("{before_text:?} -> {after_text:?}"),
                            );
                        }
                    } else if was[index] != now[index] {
                        self.violation(
                            step,
                            op,
                            "backspace changed another block",
                            &format!("block {index}: {:?} -> {:?}", was[index], now[index]),
                        );
                    }
                }
            }
            Expect::Deleted { blocks, objects } => {
                if before.objects.len() != after.objects.len() + objects {
                    self.violation(
                        step,
                        op,
                        "deleting a group did not remove exactly its objects",
                        &format!(
                            "{objects} chosen, {} -> {} objects",
                            before.objects.len(),
                            after.objects.len()
                        ),
                    );
                }
                if was.len() != now.len() {
                    self.violation(
                        step,
                        op,
                        "deleting a group changed the block count",
                        &format!("{} -> {}", was.len(), now.len()),
                    );
                    return;
                }
                for index in 0..was.len() {
                    let emptied = now[index].as_ref().is_none_or(String::is_empty);
                    if blocks.contains(&index) {
                        if !emptied {
                            self.violation(
                                step,
                                op,
                                "a deleted block still says something",
                                &format!("block {index}: {:?}", now[index]),
                            );
                        }
                    } else if was[index] != now[index] {
                        self.violation(
                            step,
                            op,
                            "deleting a group changed a block outside it",
                            &format!("block {index}: {:?} -> {:?}", was[index], now[index]),
                        );
                    }
                }
            }
            Expect::Pasted {
                characters,
                picture,
            } => {
                if *picture && after.objects.len() != before.objects.len() + 1 {
                    self.violation(
                        step,
                        op,
                        "pasting a picture did not add exactly one object",
                        &format!("{} -> {}", before.objects.len(), after.objects.len()),
                    );
                }
                let kept = now.iter().take(was.len()).cloned().collect::<Vec<_>>();
                let mut was_sorted = was.clone();
                was_sorted.sort();
                let mut now_sorted = now.clone();
                now_sorted.sort();
                let old_present = was_sorted.iter().all(|text| {
                    now_sorted.iter().filter(|t| *t == text).count()
                        >= was_sorted.iter().filter(|t| *t == text).count()
                });
                if !old_present {
                    self.violation(
                        step,
                        op,
                        "paste changed a block that was there",
                        &format!("before {:?}, after {:?}", short(&was), short(&kept)),
                    );
                }
                if let Some(characters) = characters {
                    let gained = after.characters().saturating_sub(before.characters());
                    if gained < *characters {
                        self.violation(
                            step,
                            op,
                            "paste lost characters",
                            &format!("copied {characters} characters, page gained {gained}"),
                        );
                    }
                    if now.len() < was.len() + 1 {
                        self.violation(
                            step,
                            op,
                            "pasted text was grouped into a block that was there",
                            &format!("{} blocks before, {} after", was.len(), now.len()),
                        );
                    }
                }
            }
            Expect::None => {}
        }
    }

    fn check_geometry(&mut self, step: usize, op: Op, before: &Shot, after: &Shot, done: &Done) {
        if done.reordered {
            if before.sorted_blocks() != after.sorted_blocks() {
                self.violation(step, op, "reordering moved a block", "");
            }
            if before.sorted_objects() != after.sorted_objects() {
                self.violation(step, op, "reordering moved an object", "");
            }
            return;
        }
        let matched = match_blocks(before, after);
        let (gone, new) = unmatched_objects(before, after);
        let block_after = if op.touches_text() {
            done.block
        } else {
            done.block
                .and_then(|block| matched.get(block).copied().flatten())
        };
        if let Some(block) = done.block
            && block_after.is_none()
            && !op.touches_text()
        {
            self.violation(
                step,
                op,
                "the block the step was about cannot be found afterwards",
                &format!("block {block}"),
            );
        }
        let object_after = done.object.and_then(|_| (new.len() == 1).then(|| new[0]));
        if let Some(angle) = done.turned {
            let (was, now) = match (done.block, done.object) {
                (Some(block), _) => (
                    before.blocks.get(block).map(BlockShot::rectangle),
                    block_after
                        .and_then(|at| after.blocks.get(at))
                        .map(BlockShot::rectangle),
                ),
                (_, Some(object)) => (
                    before.objects.get(object).map(|o| unmilli(o.quad)),
                    object_after
                        .and_then(|at| after.objects.get(at))
                        .map(|o| unmilli(o.quad)),
                ),
                _ => (None, None),
            };
            if let (Some(was), Some(now)) = (was, now) {
                let slack = if done.block.is_some() { 2.5 } else { 0.05 };
                if let Some(why) = not_the_same_rectangle(was, now, slack) {
                    self.violation(
                        step,
                        op,
                        "a turn changed the object's size or squareness",
                        &format!("by {:.1} degrees: {why}", angle.to_degrees()),
                    );
                }
                if done.block.is_some()
                    && self.turn_sign.is_none()
                    && let Some(shot) = block_after.and_then(|at| after.blocks.get(at))
                {
                    #[allow(clippy::cast_precision_loss)]
                    let read = shot.turn as f64 / 1000.0;
                    if read.abs() > 0.5 {
                        self.turn_sign = Some((read / angle.to_degrees()).signum());
                    }
                }
            }
            if done.levelled
                && done.block.is_some()
                && let Some(shot) = block_after.and_then(|at| after.blocks.get(at))
                && shot.turn != 0
            {
                self.violation(
                    step,
                    op,
                    "turned back to level, but the block still reads as turned",
                    &format!("{} millidegrees", shot.turn),
                );
            }
            if let Some(reference) = done.back_to {
                let now = match (done.block, done.object) {
                    (Some(_), _) => block_after
                        .and_then(|at| after.blocks.get(at))
                        .map(BlockShot::rectangle),
                    (_, Some(_)) => object_after
                        .and_then(|at| after.objects.get(at))
                        .map(|o| unmilli(o.quad)),
                    _ => None,
                };
                if let Some(now) = now {
                    let worst = reference
                        .iter()
                        .zip(now.iter())
                        .map(|(a, b)| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt())
                        .fold(0.0_f64, f64::max);
                    if worst > TURN_TOLERANCE {
                        self.violation(
                            step,
                            op,
                            "turned back to level, but not where it started",
                            &format!("worst corner off by {worst:.3} pt"),
                        );
                    }
                }
            }
        }
        if matches!(
            op,
            Op::MoveBlockOnto | Op::MoveObjectOnto | Op::TurnBlock | Op::TurnObject | Op::TurnBack
        ) {
            let flowing: Vec<usize> = (0..after.blocks.len())
                .filter(|block| self.editor.flows_round(self.page, *block))
                .collect();
            for (index, was) in before.blocks.iter().enumerate() {
                if Some(index) == done.block {
                    continue;
                }
                let Some(now) = matched[index].and_then(|at| after.blocks.get(at)) else {
                    continue;
                };
                if flowing.contains(&matched[index].unwrap_or(usize::MAX)) {
                    continue;
                }
                if was.quad != now.quad {
                    self.violation(
                        step,
                        op,
                        "another block moved",
                        &format!("block {index}: {:?} -> {:?}", was.quad, now.quad),
                    );
                }
            }
            let allowed = usize::from(done.object.is_some());
            if gone.len() > allowed || new.len() > allowed {
                self.violation(
                    step,
                    op,
                    "another object moved",
                    &format!(
                        "objects gone {:?}, new {:?}",
                        gone.iter()
                            .map(|i| before.objects[*i].quad[0])
                            .collect::<Vec<_>>(),
                        new.iter()
                            .map(|i| after.objects[*i].quad[0])
                            .collect::<Vec<_>>()
                    ),
                );
            }
        }
        if let Some(block) = block_after
            && matches!(op, Op::ShrinkFrame | Op::GrowFrame | Op::TypeInto)
            && let Some(leaf) = self.editor.leaf(self.page)
            && let Some(frame) = self.editor.frame_boxes(self.page).get(block).copied()
            && let Some(shot) = leaf.overlay.blocks.get(block)
            && shot.turn.abs() < 1e-9
        {
            let lines = shot.lines.clone();
            let mut outside = 0;
            let mut worst = (0.0_f64, [0.0; 4]);
            for cluster in &leaf.overlay.clusters {
                if !lines.contains(&cluster.line) {
                    continue;
                }
                let Some(ink) = cluster.box_pixels else {
                    continue;
                };
                let off = (frame[0] - ink[0]).max(ink[2] - frame[2]);
                if off > 1.0 && ink[2] - ink[0] < frame[2] - frame[0] {
                    outside += 1;
                    if off > worst.0 {
                        worst = (off, ink);
                    }
                }
            }
            if outside > 0 {
                let declared = self.editor.frame_is_declared(self.page, block);
                self.violation(
                    step,
                    op,
                    "ink outside the frame's sides after a layout",
                    &format!(
                        "block {block} (declared {declared}, {} lines): {outside} clusters, worst {:.1} pt past the side; frame {:?}, cluster {:?}",
                        lines.len(),
                        worst.0,
                        frame.map(|v| (v * 10.0).round() / 10.0),
                        worst.1.map(|v| (v * 10.0).round() / 10.0)
                    ),
                );
            }
        }
    }

    fn check_flow(&mut self, step: usize, op: Op) {
        let page = self.page;
        let Some(leaf) = self.editor.leaf(page).cloned() else {
            return;
        };
        let keep = pdf_edit::KEEP_CLEAR;
        for (block, shot) in leaf.overlay.blocks.iter().enumerate() {
            if !self.editor.flows_round(page, block) {
                continue;
            }
            if shot.turn.abs() > 1e-9 {
                *self
                    .tally
                    .entry("(info) a turned block flows".to_owned())
                    .or_default() += 1;
                continue;
            }
            let Some(frame) = self.editor.frame_boxes(page).get(block).copied() else {
                continue;
            };
            let ink: Vec<[f64; 4]> = leaf
                .overlay
                .clusters
                .iter()
                .filter(|cluster| shot.lines.contains(&cluster.line))
                .filter_map(|cluster| cluster.box_pixels)
                .collect();
            let mut over = 0;
            let mut near = 0;
            let mut example = String::new();
            let mut near_example = String::new();
            for object in &leaf.overlay.objects {
                if !matches!(object.kind, pdf_semantics::ObjectKind::Image) {
                    continue;
                }
                let picture = object.box_pixels;
                if !overlaps(picture, frame) {
                    continue;
                }
                let grown = [
                    picture[0] - keep + MARGIN_SLACK,
                    picture[1] - keep + MARGIN_SLACK,
                    picture[2] + keep - MARGIN_SLACK,
                    picture[3] + keep - MARGIN_SLACK,
                ];
                for cluster in &ink {
                    let said = format!(
                        "cluster {:?} against picture {:?}",
                        cluster.map(|v| (v * 10.0).round() / 10.0),
                        picture.map(|v| (v * 10.0).round() / 10.0)
                    );
                    if overlaps(*cluster, picture) {
                        over += 1;
                        if example.is_empty() {
                            example = said;
                        }
                    } else if overlaps(*cluster, grown) {
                        near += 1;
                        if near_example.is_empty() {
                            near_example = said;
                        }
                    }
                }
            }
            if over > 0 {
                self.violation(
                    step,
                    op,
                    "flowing text drawn over a picture",
                    &format!("block {block}: {over} clusters over a picture; {example}"),
                );
            }
            if near > 0 {
                self.violation(
                    step,
                    op,
                    "flowing text inside a picture's keep-clear margin",
                    &format!("block {block}: {near} clusters within {keep} pt; {near_example}"),
                );
            }
            if let Some(reading) = self.editor.block_reading(page, block)
                && let Some(first) = reading.lines.first().map(|line| line.origin.1)
                && let Some(user) = self.editor.frame_in_user_space(page, frame)
            {
                let floor = leaf.view.program.geometry.crop_box[1];
                let rows = pdf_edit::blocked_for_block(
                    &leaf.view.graph,
                    floor,
                    (user[0], user[2]),
                    (first, reading.pitch),
                );
                let grid_top = first + reading.pitch;
                let mut blocked = 0;
                let mut example = String::new();
                for cluster in &ink {
                    let Some(u) = self.editor.rect_in_user_space(page, *cluster) else {
                        continue;
                    };
                    let local = [
                        u[0] - user[0],
                        grid_top - u[3],
                        u[2] - user[0],
                        grid_top - u[1],
                    ];
                    for row in &rows {
                        let rect = [
                            row.left + keep + 1.0,
                            row.top + keep + 1.0,
                            row.right - keep - 1.0,
                            row.bottom - keep - 1.0,
                        ];
                        if overlaps(local, rect) {
                            blocked += 1;
                            if example.is_empty() {
                                example = format!(
                                    "cluster {:?} in row {:?}",
                                    local.map(|v| (v * 10.0).round() / 10.0),
                                    [row.left, row.top, row.right, row.bottom]
                                        .map(|v| (v * 10.0).round() / 10.0)
                                );
                            }
                            break;
                        }
                    }
                }
                if blocked > 0 {
                    let standing: Vec<String> = leaf
                        .overlay
                        .objects
                        .iter()
                        .filter(|object| overlaps(object.box_pixels, frame))
                        .map(|object| {
                            let q = object.quad;
                            let turned = (q[0][1] - q[1][1]).abs() > 1e-6;
                            format!(
                                "{:?} box {:?} turned {turned}",
                                object.kind,
                                object.box_pixels.map(|v| (v * 10.0).round() / 10.0)
                            )
                        })
                        .collect();
                    self.violation(
                        step,
                        op,
                        "flowing text inside a row the planner says is blocked",
                        &format!(
                            "block {block}: {blocked} clusters; {example}; frame {:?} user {:?} first baseline {first:.1} turn {:.3} lines {} grid_top {grid_top:.1} pitch {:.1}; standing: {}",
                            frame.map(|v| (v * 10.0).round() / 10.0),
                            user.map(|v| (v * 10.0).round() / 10.0),
                            reading.turn,
                            reading.lines.len(),
                            reading.pitch,
                            standing.join(" / ")
                        ),
                    );
                }
            }
        }
    }

    fn check_undo_redo(
        &mut self,
        step: usize,
        op: Op,
        before: &Shot,
        after: &Shot,
        before_flow: &[bool],
    ) {
        let limit = 2 + before.blocks.len().min(12);
        let mut undone = 0;
        let mut restored = false;
        for _ in 0..limit {
            if !self.editor.can_undo() {
                break;
            }
            let walked = self.editor.undo();
            if let Applied::Refused(reason) = walked {
                self.violation(step, op, "undo refused", &reason.to_string());
                break;
            }
            undone += 1;
            if self.shot() == *before {
                restored = true;
                break;
            }
        }
        if !restored {
            let now = self.shot();
            self.violation(
                step,
                op,
                "undo did not give the page back",
                &format!("after {undone} undo(s): {}", differences(before, &now)),
            );
        }
        let mut redone = 0;
        for _ in 0..undone {
            if !self.editor.can_redo() {
                break;
            }
            if let Applied::Refused(reason) = self.editor.redo() {
                self.violation(step, op, "redo refused", &reason.to_string());
                break;
            }
            redone += 1;
        }
        if redone != undone {
            self.violation(
                step,
                op,
                "redo ran out before every undone step was redone",
                &format!("undone {undone}, redone {redone}"),
            );
        }
        let now = self.shot();
        if restored && now != *after {
            self.violation(
                step,
                op,
                "redo did not give back what the step left",
                &differences(after, &now),
            );
        }
        let flow_now: Vec<bool> = (0..now.blocks.len())
            .map(|block| self.editor.flows_round(self.page, block))
            .collect();
        if restored
            && redone == undone
            && !matches!(
                op,
                Op::FlowOn | Op::FlowOff | Op::GroupDelete | Op::CopyPaste
            )
            && flow_now != before_flow
        {
            self.violation(
                step,
                op,
                "walking the history changed which blocks flow",
                &format!("{before_flow:?} -> {flow_now:?}"),
            );
        }
    }

    fn walk_all_the_way_back(&mut self) {
        let mut undone = 0;
        for _ in 0..20_000 {
            if !self.editor.can_undo() {
                break;
            }
            if let Applied::Refused(reason) = self.editor.undo() {
                self.violation(
                    self.steps_run + 1,
                    Op::UndoRedoBurst,
                    "undo refused on the walk back",
                    &reason.to_string(),
                );
                break;
            }
            undone += 1;
        }
        let now = self.shot();
        let start = self.start.clone();
        if now == start {
            println!("walked back {undone} steps to the page as opened: identical");
        } else {
            self.violation(
                self.steps_run + 1,
                Op::UndoRedoBurst,
                "undoing the whole run did not give back the page as opened",
                &format!("after {undone} undos: {}", differences(&start, &now)),
            );
        }
        let mut redone = 0;
        for _ in 0..undone {
            if !self.editor.can_redo() {
                break;
            }
            if let Applied::Refused(reason) = self.editor.redo() {
                self.violation(
                    self.steps_run + 1,
                    Op::UndoRedoBurst,
                    "redo refused on the walk forward",
                    &reason.to_string(),
                );
                break;
            }
            redone += 1;
        }
        println!("walked forward {redone} steps");
    }

    #[allow(clippy::cast_precision_loss)]
    fn report(&self, path: &str, page: usize, seed: u64) {
        println!();
        println!("== outcomes by command");
        for ((op, what), count) in &self.outcomes {
            println!("  {op:?} {what}: {count}");
        }
        println!("== refusals by reason");
        for (reason, count) in &self.refusals {
            println!("  {count:4}  {reason}");
        }
        println!("== violations by cause");
        let mut total = 0;
        for (cause, count) in &self.tally {
            if !cause.starts_with("(info)") {
                total += count;
            }
            println!("  {count:4}  {cause}");
        }
        let mut times: Vec<u128> = self.durations.iter().map(|(_, _, ms)| *ms).collect();
        times.sort_unstable();
        let median = times.get(times.len() / 2).copied().unwrap_or(0);
        let max = times.last().copied().unwrap_or(0);
        let slowest = self
            .durations
            .iter()
            .max_by_key(|(_, _, ms)| *ms)
            .map(|(step, op, ms)| format!("step {step} {op:?} {ms} ms"))
            .unwrap_or_default();
        let first: Vec<u128> = self.durations.iter().take(100).map(|d| d.2).collect();
        let last: Vec<u128> = self.durations.iter().rev().take(100).map(|d| d.2).collect();
        let mean = |v: &[u128]| {
            if v.is_empty() {
                0.0
            } else {
                v.iter().sum::<u128>() as f64 / v.len() as f64
            }
        };
        println!(
            "== time per step: median {median} ms, max {max} ms ({slowest}); mean of first 100 {:.0} ms, of last 100 {:.0} ms",
            mean(&first),
            mean(&last)
        );
        let rss_first = self.rss.first().map_or(0, |r| r.1);
        let rss_last = self.rss.last().map_or(0, |r| r.1);
        let rss_max = self.rss.iter().map(|r| r.1).max().unwrap_or(0);
        println!("== rss: start {rss_first} MB, end {rss_last} MB, max {rss_max} MB");
        if let Some(stopped) = &self.stopped {
            println!("== stopped: {stopped}");
        }
        println!(
            "{{\"file\": {path:?}, \"page\": {page}, \"seed\": {seed}, \"steps\": {}, \"violations\": {total}, \"median_ms\": {median}, \"max_ms\": {max}, \"rss_start_mb\": {rss_first}, \"rss_end_mb\": {rss_last}, \"stopped\": {}}}",
            self.steps_run,
            self.stopped.is_some()
        );
    }
}

enum Expect {
    None,
    Typed {
        block: usize,
        typed: String,
    },
    Erased {
        block: usize,
    },
    Deleted {
        blocks: Vec<usize>,
        objects: usize,
    },
    Pasted {
        characters: Option<usize>,
        picture: bool,
    },
}

struct Done {
    applied: Applied,
    block: Option<usize>,
    object: Option<usize>,
    turned: Option<f64>,
    back_to: Option<[[f64; 2]; 4]>,
    levelled: bool,
    reordered: bool,
    frames_only: bool,
    expect: Expect,
}

impl Done {
    fn of(applied: Applied) -> Self {
        Self {
            applied,
            block: None,
            object: None,
            turned: None,
            back_to: None,
            levelled: false,
            reordered: false,
            frames_only: false,
            expect: Expect::None,
        }
    }

    fn about_block(mut self, block: usize) -> Self {
        self.block = Some(block);
        self
    }

    fn about_object(mut self, object: usize) -> Self {
        self.object = Some(object);
        self
    }

    fn turned(mut self, angle: f64) -> Self {
        self.turned = Some(angle);
        self
    }

    fn back_to(mut self, quad: [[f64; 2]; 4]) -> Self {
        self.back_to = Some(quad);
        self
    }

    fn reordered(mut self) -> Self {
        self.reordered = true;
        self
    }

    fn levelled(mut self) -> Self {
        self.levelled = true;
        self
    }

    fn with_frames(mut self) -> Self {
        self.frames_only = true;
        self
    }

    fn burst(mut self) -> Self {
        self.frames_only = false;
        self
    }

    fn typed(mut self, typed: &str) -> Self {
        if let Some(block) = self.block {
            self.expect = Expect::Typed {
                block,
                typed: typed.to_owned(),
            };
        }
        self
    }

    fn erased(mut self) -> Self {
        if let Some(block) = self.block {
            self.expect = Expect::Erased { block };
        }
        self
    }

    fn deleted(mut self, blocks: Vec<usize>, objects: usize) -> Self {
        self.expect = Expect::Deleted { blocks, objects };
        self
    }

    fn pasted(mut self, characters: Option<usize>, picture: bool) -> Self {
        self.expect = Expect::Pasted {
            characters,
            picture,
        };
        self
    }
}

fn rotation(angle: f64) -> Matrix {
    Matrix {
        a: angle.cos(),
        b: angle.sin(),
        c: -angle.sin(),
        d: angle.cos(),
        e: 0.0,
        f: 0.0,
    }
}

fn centre(quad: [[f64; 2]; 4]) -> (f64, f64) {
    (
        quad.iter().map(|c| c[0]).sum::<f64>() / 4.0,
        quad.iter().map(|c| c[1]).sum::<f64>() / 4.0,
    )
}

#[allow(clippy::cast_precision_loss)]
fn unmilli(quad: [[i64; 2]; 4]) -> [[f64; 2]; 4] {
    quad.map(|c| c.map(|v| v as f64 / 1000.0))
}

fn side(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn not_the_same_rectangle(was: [[f64; 2]; 4], now: [[f64; 2]; 4], slack: f64) -> Option<String> {
    for i in 0..4 {
        let (a, b) = (
            side(was[i], was[(i + 1) % 4]),
            side(now[i], now[(i + 1) % 4]),
        );
        if (a - b).abs() > slack + a * 1e-4 {
            return Some(format!("side {i}: {a:.3} -> {b:.3}"));
        }
    }
    for i in 0..4 {
        let p = now[i];
        let q = now[(i + 1) % 4];
        let r = now[(i + 3) % 4];
        let dot = (q[0] - p[0]) * (r[0] - p[0]) + (q[1] - p[1]) * (r[1] - p[1]);
        let lengths = side(p, q) * side(p, r);
        if lengths > 1e-6 && (dot / lengths).abs() > 1e-3 {
            return Some(format!(
                "corner {i} is not square (cos {:.4})",
                dot / lengths
            ));
        }
    }
    None
}

fn match_blocks(before: &Shot, after: &Shot) -> Vec<Option<usize>> {
    let mut taken = vec![false; after.blocks.len()];
    before
        .blocks
        .iter()
        .map(|was| {
            let mut best: Option<(usize, i64)> = None;
            for (index, now) in after.blocks.iter().enumerate() {
                if taken[index] || now.text != was.text {
                    continue;
                }
                let distance = (now.quad[0][0] - was.quad[0][0]).abs()
                    + (now.quad[0][1] - was.quad[0][1]).abs();
                if best.is_none_or(|(_, nearest)| distance < nearest) {
                    best = Some((index, distance));
                }
            }
            best.map(|(index, _)| {
                taken[index] = true;
                index
            })
        })
        .collect()
}

fn unmatched_objects(before: &Shot, after: &Shot) -> (Vec<usize>, Vec<usize>) {
    let mut taken = vec![false; after.objects.len()];
    let mut gone = Vec::new();
    for (index, was) in before.objects.iter().enumerate() {
        let found = after
            .objects
            .iter()
            .enumerate()
            .find(|(at, now)| !taken[*at] && *now == was)
            .map(|(at, _)| at);
        match found {
            Some(at) => taken[at] = true,
            None => gone.push(index),
        }
    }
    let new: Vec<usize> = (0..after.objects.len()).filter(|at| !taken[*at]).collect();
    (gone, new)
}

fn overlaps(one: [f64; 4], other: [f64; 4]) -> bool {
    one[0] < other[2] && one[2] > other[0] && one[1] < other[3] && one[3] > other[1]
}

fn pick(texts: &[Option<String>], which: &[usize]) -> Vec<String> {
    which
        .iter()
        .map(|i| {
            texts
                .get(*i)
                .cloned()
                .flatten()
                .map_or("(none)".to_owned(), |t| t.chars().take(30).collect())
        })
        .collect()
}

fn short(texts: &[Option<String>]) -> Vec<String> {
    texts
        .iter()
        .map(|t| {
            t.as_ref()
                .map_or("(none)".to_owned(), |t| t.chars().take(12).collect())
        })
        .collect()
}

fn text_difference(a: Option<&String>, b: Option<&String>) -> String {
    let (Some(a), Some(b)) = (a, b) else {
        return format!(
            "{:?} -> {:?}",
            a.map(|t| t.chars().take(20).collect::<String>()),
            b.map(|t| t.chars().take(20).collect::<String>())
        );
    };
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let at = a
        .iter()
        .zip(b.iter())
        .position(|(x, y)| x != y)
        .unwrap_or(a.len().min(b.len()));
    let from = at.saturating_sub(6);
    let cut = |s: &[char]| s[from..(at + 8).min(s.len())].iter().collect::<String>();
    format!(
        "at {at}: {:?} -> {:?} (lengths {} -> {})",
        cut(&a),
        cut(&b),
        a.len(),
        b.len()
    )
}

fn differences(wanted: &Shot, got: &Shot) -> String {
    let mut out = Vec::new();
    if wanted.blocks.len() != got.blocks.len() {
        out.push(format!(
            "{} blocks wanted, {} got",
            wanted.blocks.len(),
            got.blocks.len()
        ));
    }
    for (index, (a, b)) in wanted.blocks.iter().zip(got.blocks.iter()).enumerate() {
        if a.text != b.text {
            out.push(format!(
                "block {index} text {}",
                text_difference(a.text.as_ref(), b.text.as_ref())
            ));
        } else if a.quad != b.quad {
            out.push(format!(
                "block {index} quad {:?} -> {:?}",
                a.quad[0], b.quad[0]
            ));
        } else if a.frame != b.frame {
            out.push(format!(
                "block {index} frame {:?} -> {:?}",
                a.frame, b.frame
            ));
        } else if a.turn != b.turn {
            out.push(format!("block {index} turn {} -> {}", a.turn, b.turn));
        }
        if out.len() > 4 {
            break;
        }
    }
    if wanted.objects.len() != got.objects.len() {
        out.push(format!(
            "{} objects wanted, {} got",
            wanted.objects.len(),
            got.objects.len()
        ));
    }
    for (index, (a, b)) in wanted.objects.iter().zip(got.objects.iter()).enumerate() {
        if a != b {
            out.push(format!("object {index} {:?} -> {:?}", a.quad[0], b.quad[0]));
            break;
        }
    }
    if wanted.atoms != got.atoms {
        out.push(format!("{} atoms wanted, {} got", wanted.atoms, got.atoms));
    }
    if out.is_empty() {
        "(no difference found)".to_owned()
    } else {
        out.join("; ")
    }
}
