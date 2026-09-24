use std::collections::BTreeSet;
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use pdf_bytes::ByteStore;
use pdf_semantics::Grouping;
use pdf_session::PageView;

pub use crate::tiles::TileId;

#[derive(Clone, Debug)]
pub struct TilePixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

enum Task {
    Read {
        page: usize,
        source: ByteStore,
        credential: Vec<u8>,
        grouping: Option<Arc<Grouping>>,
        fonts: Option<Arc<dyn pdf_content::FontProvider>>,
        epoch: u64,
    },
    Tile {
        id: TileId,
        view: Arc<PageView>,
        scale: f64,
        window: [u32; 4],
        epoch: u64,
    },
    Search {
        page: usize,
        source: ByteStore,
        credential: Vec<u8>,
        fonts: Option<Arc<dyn pdf_content::FontProvider>>,
        needle: String,
        epoch: u64,
    },
    #[cfg(target_arch = "wasm32")]
    Far(FarTile),
}

#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy, Debug)]
pub struct FarTile {
    pub id: TileId,
    pub scale: f64,
    pub window: [u32; 4],
    pub epoch: u64,
}

impl Task {
    fn tracked(&self) -> Option<(Wanted, u64)> {
        match self {
            Self::Read { page, epoch, .. } => Some((Wanted::Page(*page), *epoch)),
            Self::Tile { id, epoch, .. } => Some((Wanted::Tile(*id), *epoch)),
            #[cfg(target_arch = "wasm32")]
            Self::Far(far) => Some((Wanted::Tile(far.id), far.epoch)),
            Self::Search { .. } => None,
        }
    }
}

pub enum Done {
    Read {
        page: usize,
        view: Result<Arc<PageView>, String>,
        epoch: u64,
    },
    Tile {
        id: TileId,
        pixels: Result<TilePixels, String>,
        epoch: u64,
    },
    Searched {
        page: usize,
        needle: String,
        hits: Vec<crate::find::Hit>,
        epoch: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Wanted {
    Page(usize),
    Tile(TileId),
}

#[derive(Default)]
struct Queue {
    tasks: Vec<Task>,
    in_flight: BTreeSet<(Wanted, u64)>,
    closed: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
    answers: std::sync::mpsc::Sender<Done>,
    wake: Mutex<Option<Wake>>,
}

type Wake = Arc<dyn Fn() + Send + Sync>;

impl Shared {
    fn answer(&self, done: Done) -> bool {
        if self.answers.send(done).is_err() {
            return false;
        }
        let wake = self
            .wake
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(wake) = wake {
            wake();
        }
        true
    }
}

pub struct Painter {
    shared: Arc<Shared>,
    answers: std::sync::mpsc::Receiver<Done>,
    #[cfg(not(target_arch = "wasm32"))]
    workers: Vec<std::thread::JoinHandle<()>>,
    outstanding: BTreeSet<(Wanted, u64)>,
    #[cfg(target_arch = "wasm32")]
    far: bool,
}

impl Painter {
    #[must_use]
    pub fn new() -> Self {
        let threads = std::env::var("PANPDF_WORKERS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or_else(|| {
                std::thread::available_parallelism()
                    .map_or(2, std::num::NonZeroUsize::get)
                    .saturating_sub(1)
            })
            .clamp(1, 6);
        let (answers, receiver) = std::sync::mpsc::channel();
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue::default()),
            ready: Condvar::new(),
            answers,
            wake: Mutex::new(None),
        });
        #[cfg(not(target_arch = "wasm32"))]
        let workers = (0..threads)
            .map(|_| {
                let shared = Arc::clone(&shared);
                std::thread::spawn(move || work(&shared))
            })
            .collect();
        #[cfg(target_arch = "wasm32")]
        let _ = threads;
        Self {
            shared,
            answers: receiver,
            #[cfg(not(target_arch = "wasm32"))]
            workers,
            outstanding: BTreeSet::new(),
            #[cfg(target_arch = "wasm32")]
            far: false,
        }
    }

    #[must_use]
    pub fn busy(&self) -> usize {
        self.outstanding.len()
    }

    #[must_use]
    pub fn wakes_the_window(&self) -> bool {
        self.shared
            .wake
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    pub fn set_waker(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        *self
            .shared
            .wake
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(wake));
    }

    #[must_use]
    pub fn reading(&self, page: usize, epoch: u64) -> bool {
        self.outstanding.contains(&(Wanted::Page(page), epoch))
    }

    #[must_use]
    pub fn drawing(&self, id: TileId, epoch: u64) -> bool {
        self.outstanding.contains(&(Wanted::Tile(id), epoch))
    }

    pub fn read(
        &mut self,
        page: usize,
        source: ByteStore,
        credential: &[u8],
        grouping: Option<Arc<Grouping>>,
        fonts: Option<Arc<dyn pdf_content::FontProvider>>,
        epoch: u64,
    ) {
        if !self.outstanding.insert((Wanted::Page(page), epoch)) {
            return;
        }
        self.push(Task::Read {
            page,
            source,
            credential: credential.to_vec(),
            grouping,
            fonts,
            epoch,
        });
    }

    pub fn search(
        &mut self,
        page: usize,
        source: ByteStore,
        credential: &[u8],
        fonts: Option<Arc<dyn pdf_content::FontProvider>>,
        needle: &str,
        epoch: u64,
    ) {
        self.push_under(Task::Search {
            page,
            source,
            credential: credential.to_vec(),
            fonts,
            needle: needle.to_owned(),
            epoch,
        });
    }

    pub fn draw(
        &mut self,
        id: TileId,
        view: Arc<PageView>,
        scale: f64,
        window: [u32; 4],
        epoch: u64,
    ) {
        if !self.outstanding.insert((Wanted::Tile(id), epoch)) {
            return;
        }
        #[cfg(target_arch = "wasm32")]
        if self.far {
            self.push(Task::Far(FarTile {
                id,
                scale,
                window,
                epoch,
            }));
            return;
        }
        self.push(Task::Tile {
            id,
            view,
            scale,
            window,
            epoch,
        });
    }

    pub fn forget(&mut self, keep: &dyn Fn(TileId) -> bool) {
        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        queue.tasks.retain(|task| match task {
            Task::Tile { id, .. } => keep(*id),
            #[cfg(target_arch = "wasm32")]
            Task::Far(far) => keep(far.id),
            Task::Read { .. } | Task::Search { .. } => true,
        });
        let queued: BTreeSet<(Wanted, u64)> =
            queue.tasks.iter().filter_map(Task::tracked).collect();
        self.outstanding
            .retain(|key| queued.contains(key) || queue.in_flight.contains(key));
    }

    #[cfg(target_arch = "wasm32")]
    pub fn work_while(&mut self, reading: bool, mut more: impl FnMut() -> bool) {
        loop {
            let Some(task) = take(&self.shared, reading) else {
                return;
            };
            if !self.shared.answer(perform(task)) || !more() {
                return;
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn send_tiles_elsewhere(&mut self) {
        self.far = true;
    }

    #[cfg(target_arch = "wasm32")]
    #[must_use]
    pub const fn tiles_go_elsewhere(&self) -> bool {
        self.far
    }

    #[cfg(target_arch = "wasm32")]
    pub fn draw_far(&mut self, id: TileId, scale: f64, window: [u32; 4], epoch: u64) {
        if !self.outstanding.insert((Wanted::Tile(id), epoch)) {
            return;
        }
        self.push(Task::Far(FarTile {
            id,
            scale,
            window,
            epoch,
        }));
    }

    #[cfg(target_arch = "wasm32")]
    pub fn take_far(&mut self) -> Option<FarTile> {
        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let at = queue
            .tasks
            .iter()
            .rposition(|task| matches!(task, Task::Far(_)))?;
        let Task::Far(far) = queue.tasks.remove(at) else {
            return None;
        };
        queue.in_flight.insert((Wanted::Tile(far.id), far.epoch));
        Some(far)
    }

    #[cfg(target_arch = "wasm32")]
    pub fn answer_far(&mut self, id: TileId, epoch: u64, pixels: Result<TilePixels, String>) {
        let _ = self.shared.answer(Done::Tile { id, pixels, epoch });
    }

    pub fn collect(&mut self) -> Vec<Done> {
        let mut taken = Vec::new();
        let mut answered = Vec::new();
        while let Ok(done) = self.answers.try_recv() {
            let key = match &done {
                Done::Read { page, epoch, .. } => Some((Wanted::Page(*page), *epoch)),
                Done::Tile { id, epoch, .. } => Some((Wanted::Tile(*id), *epoch)),
                Done::Searched { .. } => None,
            };
            if let Some(key) = key {
                self.outstanding.remove(&key);
                answered.push(key);
            }
            taken.push(done);
        }
        if !answered.is_empty() {
            let mut queue = self
                .shared
                .queue
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            for key in answered {
                queue.in_flight.remove(&key);
            }
        }
        taken
    }

    fn push_under(&mut self, task: Task) {
        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        queue.tasks.insert(0, task);
        drop(queue);
        self.shared.ready.notify_one();
    }

    fn push(&mut self, task: Task) {
        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        queue.tasks.push(task);
        drop(queue);
        self.shared.ready.notify_one();
    }
}

impl Default for Painter {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Painter {
    fn drop(&mut self) {
        {
            let mut queue = self
                .shared
                .queue
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            queue.closed = true;
            queue.tasks.clear();
        }
        self.shared.ready.notify_all();
        #[cfg(not(target_arch = "wasm32"))]
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn work(shared: &Shared) {
    loop {
        let task = {
            let mut queue = shared.queue.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if queue.closed {
                    return;
                }
                if let Some(task) = queue.tasks.pop() {
                    if let Some(key) = task.tracked() {
                        queue.in_flight.insert(key);
                    }
                    break task;
                }
                queue = shared
                    .ready
                    .wait(queue)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        if !shared.answer(perform(task)) {
            return;
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn take(shared: &Shared, reading: bool) -> Option<Task> {
    let mut queue = shared.queue.lock().unwrap_or_else(PoisonError::into_inner);
    if queue.closed {
        return None;
    }
    let at = queue.tasks.iter().rposition(|task| match task {
        Task::Tile { .. } => true,
        Task::Read { .. } | Task::Search { .. } => reading,
        Task::Far(_) => false,
    })?;
    let task = queue.tasks.remove(at);
    if let Some(key) = task.tracked() {
        queue.in_flight.insert(key);
    }
    Some(task)
}

fn perform(task: Task) -> Done {
    match task {
        Task::Read {
            page,
            source,
            credential,
            grouping,
            fonts,
            epoch,
        } => {
            let view = pdf_session::interpret_page_for_display(
                &source,
                page,
                &credential,
                grouping.as_deref(),
                fonts,
            )
            .map(Arc::new)
            .map_err(|error| error.to_string());
            Done::Read { page, view, epoch }
        }
        Task::Tile {
            id,
            view,
            scale,
            window,
            epoch,
        } => {
            let pixels = draw_tile(&view, scale, window);
            Done::Tile { id, pixels, epoch }
        }
        #[cfg(target_arch = "wasm32")]
        Task::Far(far) => Done::Tile {
            id: far.id,
            pixels: Err("this tile is drawn by a worker".to_owned()),
            epoch: far.epoch,
        },
        Task::Search {
            page,
            source,
            credential,
            fonts,
            needle,
            epoch,
        } => {
            let hits =
                pdf_session::interpret_page_for_display(&source, page, &credential, None, fonts)
                    .ok()
                    .and_then(|view| {
                        let overlay =
                            pdf_cli::page_overlay_view(&view, crate::document::OVERLAY_SCALE)
                                .ok()?;
                        Some(crate::find::hits_in(page, &overlay.clusters, &needle))
                    })
                    .unwrap_or_default();
            Done::Searched {
                page,
                needle,
                hits,
                epoch,
            }
        }
    }
}

pub fn draw_page(
    view: &PageView,
    scale: f64,
) -> Result<(pdf_render::Canvas, pdf_render::RenderReport), pdf_render::RenderError> {
    let options = pdf_render::RenderOptions {
        scale,
        ..pdf_render::RenderOptions::default()
    };
    pdf_render::render_page_layers(&view.layers(), &view.program.geometry, options)
}

pub fn draw_region(
    view: &PageView,
    scale: f64,
    window: [u32; 4],
) -> Result<(pdf_render::Canvas, pdf_render::RenderReport), pdf_render::RenderError> {
    let options = pdf_render::RenderOptions {
        scale,
        ..pdf_render::RenderOptions::default()
    };
    pdf_render::render_region_layers(&view.layers(), &view.program.geometry, options, window)
}

fn draw_tile(view: &PageView, scale: f64, window: [u32; 4]) -> Result<TilePixels, String> {
    let (canvas, _) = draw_region(view, scale, window).map_err(|error| error.to_string())?;
    Ok(TilePixels {
        width: canvas.width,
        height: canvas.height,
        rgba: rgba(&canvas),
    })
}

#[must_use]
pub fn rgba(canvas: &pdf_render::Canvas) -> Vec<u8> {
    canvas.to_rgba8()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idle_painter() -> Painter {
        let (answers, receiver) = std::sync::mpsc::channel();
        Painter {
            shared: Arc::new(Shared {
                queue: Mutex::new(Queue::default()),
                ready: Condvar::new(),
                answers,
                wake: Mutex::new(None),
            }),
            answers: receiver,
            #[cfg(not(target_arch = "wasm32"))]
            workers: Vec::new(),
            outstanding: BTreeSet::new(),
            #[cfg(target_arch = "wasm32")]
            far: false,
        }
    }

    fn tile() -> TileId {
        TileId {
            page: 3,
            zoom: 4,
            col: 0,
            row: 1,
        }
    }

    #[test]
    fn scrolling_away_keeps_a_running_tile_tracked_until_its_answer_is_collected() {
        let mut painter = idle_painter();
        let key = (Wanted::Tile(tile()), 7);
        painter.outstanding.insert(key);
        painter.shared.queue.lock().unwrap().in_flight.insert(key);
        painter.forget(&|_| false);
        assert!(
            painter.drawing(tile(), 7),
            "returning must not start a duplicate"
        );
        assert!(
            !painter.drawing(tile(), 8),
            "a new revision still needs its own work"
        );
        painter
            .shared
            .answers
            .send(Done::Tile {
                id: tile(),
                pixels: Err("test refusal".to_owned()),
                epoch: 7,
            })
            .unwrap();
        painter.forget(&|_| false);
        assert!(painter.drawing(tile(), 7));
        assert_eq!(painter.collect().len(), 1);
        assert!(!painter.drawing(tile(), 7));
        assert!(painter.shared.queue.lock().unwrap().in_flight.is_empty());
    }

    #[test]
    fn an_unstarted_unwanted_tile_is_forgotten() {
        let mut painter = idle_painter();
        painter.outstanding.insert((Wanted::Tile(tile()), 7));
        painter.forget(&|_| false);
        assert!(!painter.drawing(tile(), 7));
    }

    #[test]
    fn a_finished_answer_wakes_the_front_end_after_it_is_available() {
        let mut painter = idle_painter();
        let (sent, received) = std::sync::mpsc::channel();
        painter.set_waker(move || {
            sent.send(()).unwrap();
        });
        assert!(
            received.try_recv().is_err(),
            "idle workers do not request frames"
        );
        let shared = Arc::clone(&painter.shared);
        let worker = std::thread::spawn(move || {
            shared.answer(Done::Tile {
                id: tile(),
                pixels: Err("test refusal".to_owned()),
                epoch: 7,
            })
        });
        received
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(matches!(
            painter.collect().as_slice(),
            [Done::Tile { epoch: 7, .. }]
        ));
        assert!(worker.join().unwrap());
        assert!(received.try_recv().is_err(), "one answer requests one wake");
    }
}
