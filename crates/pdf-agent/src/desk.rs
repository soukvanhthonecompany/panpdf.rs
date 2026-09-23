use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::{BlockRange, BlockReading, ClusterRef, Command, LineEnd, SourceAnchor};
use pdf_paint::Point;
use pdf_session::{PageView, Session};

pub const MOST_CHARACTERS: usize = 60_000;

pub const MOST_PIXELS: f64 = 2_400.0;

pub type Refused = String;

struct Open {
    path: PathBuf,
    original: Arc<[u8]>,
    session: Session,
    revision: u64,
    arranged: u64,
    named: BTreeMap<(usize, usize), Named>,
    restrictions_set_aside: bool,
}

#[derive(Clone, Debug)]
struct Named {
    revision: u64,
    arranged: u64,
    text: String,
    area: [f64; 4],
}

#[derive(Clone, Debug)]
pub struct Block {
    pub page: usize,
    pub index: usize,
    pub text: String,
    pub area: [f64; 4],
    pub size: f64,
    pub fixed: Option<&'static str>,
}

impl Block {
    #[must_use]
    pub fn name(&self) -> String {
        format!("p{}-b{}", self.page + 1, self.index + 1)
    }
}

struct Parts {
    rows: Vec<Vec<ClusterRef>>,
    frame: (f64, f64),
    reading: BlockReading,
}

#[derive(Clone, Debug)]
pub struct Summary {
    pub handle: String,
    pub pages: usize,
    pub title: String,
    pub protected: bool,
    pub restricted: bool,
}

pub struct Desk {
    open: BTreeMap<String, Open>,
    opened: u64,
    fonts: Option<Arc<dyn pdf_content::FontProvider>>,
}

impl Default for Desk {
    fn default() -> Self {
        Self::with_fonts(pdf_cli::font_provider())
    }
}

impl Desk {
    #[must_use]
    pub fn with_fonts(fonts: Option<Arc<dyn pdf_content::FontProvider>>) -> Self {
        Self {
            open: BTreeMap::new(),
            opened: 0,
            fonts,
        }
    }

    pub fn open(
        &mut self,
        path: &Path,
        password: &str,
        set_aside_restrictions: bool,
    ) -> Result<Summary, Refused> {
        let bytes = std::fs::read(path)
            .map_err(|error| format!("{} cannot be read: {error}", path.display()))?;
        let original: Arc<[u8]> = Arc::from(bytes);
        let source = ByteStore::new(SourceId::new(0), Arc::clone(&original));
        if pdf_edit::info::lock(&source, password.as_bytes()) == pdf_edit::info::Lock::Refused {
            return Err(if password.is_empty() {
                "this document is protected by a password: ask the person for it and open it again with `password`".to_owned()
            } else {
                "that password does not open this document".to_owned()
            });
        }
        let mut session = Session::with_fonts(source, password.as_bytes(), self.fonts.clone());
        let pages = session
            .page_count()
            .map_err(|error| format!("this document's pages cannot be read: {error}"))?;
        if pages == 0 {
            return Err("this document has no pages".to_owned());
        }
        let restricted = session.restricts_editing().unwrap_or(true);
        if restricted && set_aside_restrictions {
            session.set_aside_restrictions();
        }
        let facts = pdf_edit::info::document_facts(session.source(), password.as_bytes()).ok();
        self.opened += 1;
        let handle = format!("doc-{}", self.opened);
        self.open.insert(
            handle.clone(),
            Open {
                path: path.to_owned(),
                original,
                session,
                revision: 0,
                arranged: 0,
                named: BTreeMap::new(),
                restrictions_set_aside: restricted && set_aside_restrictions,
            },
        );
        Ok(Summary {
            handle,
            pages,
            title: facts
                .as_ref()
                .map(|facts| facts.info.title.clone())
                .unwrap_or_default(),
            protected: facts.is_some_and(|facts| facts.protection.is_some()),
            restricted,
        })
    }

    pub fn close(&mut self, handle: &str) -> Result<bool, Refused> {
        let open = self.open.remove(handle).ok_or_else(|| unknown(handle))?;
        Ok(open.revision != 0 && open.session.source().as_bytes() != &*open.original)
    }

    #[must_use]
    pub fn handles(&self) -> Vec<(String, PathBuf, usize, bool)> {
        self.open
            .iter()
            .map(|(handle, open)| {
                (
                    handle.clone(),
                    open.path.clone(),
                    open.session.page_count().unwrap_or(0),
                    open.session.source().as_bytes() != &*open.original,
                )
            })
            .collect()
    }

    fn get(&mut self, handle: &str) -> Result<&mut Open, Refused> {
        self.open.get_mut(handle).ok_or_else(|| unknown(handle))
    }

    pub fn page_count(&mut self, handle: &str) -> Result<usize, Refused> {
        let open = self.get(handle)?;
        open.session
            .page_count()
            .map_err(|error| format!("the pages cannot be counted: {error}"))
    }

    pub fn page_sizes(&mut self, handle: &str) -> Result<Vec<[f64; 2]>, Refused> {
        let open = self.get(handle)?;
        let geometries = open
            .session
            .page_geometries()
            .map_err(|error| format!("the pages cannot be measured: {error}"))?;
        Ok(shown_sizes(&geometries))
    }

    pub fn source(&mut self, handle: &str) -> Result<(ByteStore, Vec<u8>), Refused> {
        let open = self.get(handle)?;
        Ok((
            open.session.source().clone(),
            open.session.credential().to_vec(),
        ))
    }

    pub fn blocks(&mut self, handle: &str, page: usize) -> Result<Vec<Block>, Refused> {
        let open = self.get(handle)?;
        let view = view_of(&mut open.session, page)?;
        let blocks: Vec<Block> = (0..view.index.blocks.len())
            .filter_map(|index| read(&view, page, index).map(|(block, _)| block))
            .collect();
        for block in &blocks {
            open.named.insert(
                (page, block.index),
                Named {
                    revision: open.revision,
                    arranged: open.arranged,
                    text: block.text.clone(),
                    area: block.area,
                },
            );
        }
        Ok(blocks)
    }

    fn resolve(open: &mut Open, name: &str) -> Result<(usize, usize, Arc<PageView>), Refused> {
        let (page, index) = parse_name(name)?;
        let named = open.named.get(&(page, index)).cloned().ok_or_else(|| {
            format!("{name} has not been read yet: call read_text or find_text for that page first")
        })?;
        if named.arranged != open.arranged {
            return Err(format!(
                "{name} was read before the pages were moved about, and its page number \
                 no longer means the page it meant: read_text again and use the name it \
                 gives now"
            ));
        }
        let view = view_of(&mut open.session, page)?;
        if named.revision == open.revision {
            return Ok((page, index, view));
        }
        let same: Vec<usize> = (0..view.index.blocks.len())
            .filter(|at| {
                read(&view, page, *at).is_some_and(|(block, _)| {
                    block.text == named.text && overlaps(block.area, named.area)
                })
            })
            .collect();
        match same[..] {
            [only] => Ok((page, only, view)),
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

    pub fn rewrite(
        &mut self,
        handle: &str,
        name: &str,
        find: Option<&str>,
        text: &str,
    ) -> Result<Block, Refused> {
        let open = self.get(handle)?;
        let (page, index, view) = Self::resolve(open, name)?;
        let (block, parts) =
            read(&view, page, index).ok_or_else(|| format!("{name} cannot be read as text"))?;
        if let Some(reason) = block.fixed {
            return Err(format!("{name} cannot be rewritten: {reason}"));
        }
        let Parts {
            rows,
            frame,
            reading,
        } = parts;
        let range = match find {
            None => whole(&reading),
            Some(find) => {
                range_within(&reading, find).map_err(|problem| format!("{name}: {problem}"))?
            }
        };
        let text = text.replace("\r\n", "\n");
        let command = Command::RewriteBlock {
            page_index: page,
            rows,
            frame,
            edges: (0, 0),
            breaks: None,
            frame_declared: false,
            paragraph: pdf_edit::ParagraphLayout::default(),
            range,
            text,
        };
        apply(open, &command)?;
        let view = view_of(&mut open.session, page)?;
        let now = (0..view.index.blocks.len())
            .filter_map(|at| read(&view, page, at).map(|(block, _)| block))
            .filter(|now| overlaps(now.area, block.area))
            .max_by(|one, other| {
                overlap(one.area, block.area).total_cmp(&overlap(other.area, block.area))
            });
        let now = now.ok_or_else(|| {
            "the edit was made, but the block could not be read back: read_text the page again"
                .to_owned()
        })?;
        open.named.insert(
            (page, now.index),
            Named {
                revision: open.revision,
                arranged: open.arranged,
                text: now.text.clone(),
                area: now.area,
            },
        );
        Ok(now)
    }

    pub fn place_text(
        &mut self,
        handle: &str,
        page: usize,
        area: [f64; 4],
        text: &str,
        style: (&str, f64, bool, bool, Option<[f64; 3]>),
    ) -> Result<(), Refused> {
        let open = self.get(handle)?;
        let view = view_of(&mut open.session, page)?;
        let frame = to_user(&view, area)?;
        let (family, size, bold, italic, fill) = style;
        let command = Command::PlaceNewText {
            page_index: page,
            frame,
            text: text.replace("\r\n", "\n"),
            family: family.to_owned(),
            size,
            bold,
            italic,
            fill,
            paragraph: pdf_edit::ParagraphLayout::default(),
        };
        apply(open, &command)
    }

    pub fn draw(
        &mut self,
        handle: &str,
        page: usize,
        steps: &[pdf_edit::PenStep],
        stroke: Option<([f64; 3], f64)>,
        fill: Option<[f64; 3]>,
    ) -> Result<(), Refused> {
        let open = self.get(handle)?;
        let view = view_of(&mut open.session, page)?;
        let command = Command::DrawPath {
            page_index: page,
            steps: steps_in_user_space(&view, steps)?,
            closed: false,
            stroke: stroke.map(|(colour, width)| pdf_edit::PenStroke::pen(colour, width)),
            fill,
        };
        apply(open, &command)
    }

    #[must_use]
    pub fn fonts(&self) -> Option<Arc<dyn pdf_content::FontProvider>> {
        self.fonts.clone()
    }

    pub fn command(&mut self, handle: &str, command: &Command) -> Result<(), Refused> {
        let open = self.get(handle)?;
        apply(open, command)
    }

    pub fn walk(&mut self, handle: &str, back: bool) -> Result<bool, Refused> {
        let open = self.get(handle)?;
        let walked = if back {
            open.session.undo()
        } else {
            open.session.redo()
        }
        .map_err(|error| error.to_string())?;
        if walked {
            open.revision += 1;
            note_any_page_move(open);
        }
        Ok(walked)
    }

    pub fn picture(
        &mut self,
        handle: &str,
        page: usize,
        dpi: f64,
    ) -> Result<(Vec<u8>, u32, u32), Refused> {
        let open = self.get(handle)?;
        let count = open
            .session
            .page_count()
            .map_err(|error| error.to_string())?;
        if page >= count {
            return Err(no_page(page, count));
        }
        let view = open
            .session
            .page_for_display(page)
            .map_err(|error| format!("page {} cannot be read: {error}", page + 1))?;
        picture_of(&view, dpi)
    }

    pub fn save(
        &mut self,
        handle: &str,
        destination: &Path,
        replace: bool,
    ) -> Result<u64, Refused> {
        let open = self.get(handle)?;
        let exists = destination.exists();
        if exists && !replace {
            return Err(format!(
                "{} already exists: save under another name, or pass replace: true once the person agrees",
                destination.display()
            ));
        }
        let same_file = exists
            && std::fs::canonicalize(destination).ok() == std::fs::canonicalize(&open.path).ok();
        if same_file {
            let on_disk = std::fs::read(destination).map_err(|error| error.to_string())?;
            if on_disk.as_slice() != &*open.original {
                return Err(format!(
                    "{} changed on disk since it was opened: save under another name",
                    destination.display()
                ));
            }
        } else if exists && !destination.is_file() {
            return Err(format!("{} is not a file", destination.display()));
        }
        let bytes = open.session.source().as_bytes();
        let directory = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let name = destination
            .file_name()
            .ok_or_else(|| "a file name is needed".to_owned())?;
        let mut temporary_name = std::ffi::OsString::from(".");
        temporary_name.push(name);
        temporary_name.push(format!(".panpdf-{}.tmp", std::process::id()));
        let temporary = directory.join(temporary_name);
        let written = (|| {
            use std::io::Write as _;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            std::fs::rename(&temporary, destination)
        })();
        if let Err(error) = written {
            let _ = std::fs::remove_file(&temporary);
            return Err(format!(
                "{} could not be written: {error}",
                destination.display()
            ));
        }
        if same_file {
            open.original = Arc::from(bytes);
        }
        Ok(bytes.len() as u64)
    }

    pub fn restrictions_set_aside(&mut self, handle: &str) -> Result<bool, Refused> {
        Ok(self.get(handle)?.restrictions_set_aside)
    }
}

fn unknown(handle: &str) -> Refused {
    format!(
        "no document is open as {handle}: open_document first, or list_documents to see what is open"
    )
}

fn no_page(page: usize, count: usize) -> Refused {
    format!("there is no page {}: the document has {count}", page + 1)
}

pub fn picture_of(view: &PageView, dpi: f64) -> Result<(Vec<u8>, u32, u32), Refused> {
    let device = pdf_render::DeviceTransform::for_page(
        &view.program.geometry,
        1.0,
        pdf_render::RenderLimits::default(),
    )
    .map_err(|_| "this page has no size".to_owned())?;
    let longest = f64::from(device.width.max(device.height)).max(1.0);
    let scale = (dpi / 72.0).min(MOST_PIXELS / longest).max(0.05);
    let (canvas, _) = pdf_cli::render_page_view(view, scale)
        .map_err(|error| format!("it cannot be drawn: {error}"))?;
    let png = pdf_edit::png::write((canvas.width, canvas.height), &canvas.to_rgb8(), None)
        .map_err(str::to_owned)?;
    Ok((png, canvas.width, canvas.height))
}

#[must_use]
pub fn text_of_page(view: &PageView, page: usize, most: usize) -> String {
    let mut text = String::new();
    for index in 0..view.index.blocks.len() {
        if text.chars().count() >= most {
            break;
        }
        let Some((block, _)) = read(view, page, index) else {
            continue;
        };
        if !text.is_empty() {
            text.push('\n');
        }
        let room = most.saturating_sub(text.chars().count());
        text.extend(block.text.chars().take(room));
    }
    text
}

fn view_of(session: &mut Session, page: usize) -> Result<Arc<PageView>, Refused> {
    let count = session.page_count().map_err(|error| error.to_string())?;
    if page >= count {
        return Err(no_page(page, count));
    }
    session
        .page(page)
        .map_err(|error| format!("page {} cannot be read: {error}", page + 1))
}

fn apply(open: &mut Open, command: &Command) -> Result<(), Refused> {
    let plan = open
        .session
        .plan(command)
        .map_err(|error| error.to_string())?;
    open.session
        .apply(plan)
        .map_err(|error| error.to_string())?;
    open.revision += 1;
    note_any_page_move(open);
    Ok(())
}

fn note_any_page_move(open: &mut Open) {
    if open.session.last_pages().is_some() {
        open.arranged += 1;
    }
}

fn parse_name(name: &str) -> Result<(usize, usize), Refused> {
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

fn device(view: &PageView) -> Result<pdf_render::DeviceTransform, Refused> {
    pdf_render::DeviceTransform::for_page(
        &view.program.geometry,
        1.0,
        pdf_render::RenderLimits::default(),
    )
    .map_err(|_| "this page has no size".to_owned())
}

fn to_shown(device: &pdf_render::DeviceTransform, [x0, y0, x1, y1]: [f64; 4]) -> [f64; 4] {
    let corners = [(x0, y0), (x0, y1), (x1, y0), (x1, y1)]
        .map(|(x, y)| device.matrix.transform(Point { x, y }));
    let left = corners
        .iter()
        .map(|point| point.x)
        .fold(f64::INFINITY, f64::min);
    let right = corners
        .iter()
        .map(|point| point.x)
        .fold(f64::NEG_INFINITY, f64::max);
    let top = corners
        .iter()
        .map(|point| point.y)
        .fold(f64::INFINITY, f64::min);
    let bottom = corners
        .iter()
        .map(|point| point.y)
        .fold(f64::NEG_INFINITY, f64::max);
    [left, top, right, bottom].map(|value| (value * 100.0).round() / 100.0)
}

pub fn to_user(view: &PageView, [left, top, right, bottom]: [f64; 4]) -> Result<[f64; 4], Refused> {
    let device = device(view)?;
    let inverse = device
        .matrix
        .inverse()
        .ok_or_else(|| "this page has no size".to_owned())?;
    let one = inverse.transform(Point { x: left, y: top });
    let other = inverse.transform(Point {
        x: right,
        y: bottom,
    });
    Ok([
        one.x.min(other.x),
        one.y.min(other.y),
        one.x.max(other.x),
        one.y.max(other.y),
    ])
}

pub fn steps_in_user_space(
    view: &PageView,
    steps: &[pdf_edit::PenStep],
) -> Result<Vec<pdf_edit::PenStep>, Refused> {
    let device = device(view)?;
    let inverse = device
        .matrix
        .inverse()
        .ok_or_else(|| "this page has no size".to_owned())?;
    let thousandths = |value: f64| (value * 1000.0).round() / 1000.0;
    let point = |(x, y): (f64, f64)| {
        let at = inverse.transform(Point { x, y });
        (thousandths(at.x), thousandths(at.y))
    };
    Ok(steps
        .iter()
        .map(|step| match *step {
            pdf_edit::PenStep::Move(at) => pdf_edit::PenStep::Move(point(at)),
            pdf_edit::PenStep::Line(at) => pdf_edit::PenStep::Line(point(at)),
            pdf_edit::PenStep::Curve(one, other, end) => {
                pdf_edit::PenStep::Curve(point(one), point(other), point(end))
            }
        })
        .collect())
}

#[must_use]
pub fn overlap(one: [f64; 4], other: [f64; 4]) -> f64 {
    let width = one[2].min(other[2]) - one[0].max(other[0]);
    let height = one[3].min(other[3]) - one[1].max(other[1]);
    if width <= 0.0 || height <= 0.0 {
        0.0
    } else {
        width * height
    }
}

#[must_use]
pub fn overlaps(one: [f64; 4], other: [f64; 4]) -> bool {
    overlap(one, other) > 0.0
}

#[must_use]
pub fn read_block(view: &PageView, page: usize, index: usize) -> Option<Block> {
    read(view, page, index).map(|(block, _)| block)
}

#[must_use]
pub fn shown_sizes(geometries: &[pdf_content::PageGeometry]) -> Vec<[f64; 2]> {
    geometries
        .iter()
        .map(|geometry| {
            pdf_render::DeviceTransform::for_page(
                geometry,
                1.0,
                pdf_render::RenderLimits::default(),
            )
            .map_or([0.0, 0.0], |device| {
                [f64::from(device.width), f64::from(device.height)]
            })
        })
        .collect()
}

fn read(view: &PageView, page: usize, index: usize) -> Option<(Block, Parts)> {
    let semantic = view.index.blocks.get(index)?;
    let rows: Vec<Vec<ClusterRef>> = semantic
        .lines
        .iter()
        .map(|line| {
            view.index.lines[*line]
                .clusters
                .iter()
                .map(|cluster| {
                    let cluster = &view.index.clusters[*cluster];
                    ClusterRef {
                        anchor: SourceAnchor::of(&view.graph.atoms[cluster.atom].id),
                        glyphs: cluster.glyphs.clone(),
                    }
                })
                .collect()
        })
        .collect();
    let laid = semantic.layout.or(semantic.bounds)?;
    let device = device(view).ok()?;
    let upright = device.matrix.b.abs() < 1e-9 && device.matrix.c.abs() < 1e-9;
    let frame = (laid[0], laid[2]);
    let reading =
        pdf_edit::read_block(&view.program, &view.graph, &rows, frame, (0, 0), None).ok()?;
    let end = reading
        .lines
        .len()
        .checked_sub(1)
        .map(|last| (last, reading.lines[last].clusters.len()))?;
    let text = reading.text_between((0, 0), end)?;
    if text.trim().is_empty() {
        return None;
    }
    let fixed = if !upright {
        Some("the page is shown turned, and text on a turned page is not laid out again yet")
    } else if reading.turn.abs() > 1e-9 {
        Some("the text is set at an angle, and turned text is not laid out again yet")
    } else {
        None
    };
    let size = reading.lines.first().map_or(0.0, |line| line.em);
    Some((
        Block {
            page,
            index,
            text,
            area: to_shown(&device, laid),
            size: (size * 10.0).round() / 10.0,
            fixed,
        },
        Parts {
            rows,
            frame,
            reading,
        },
    ))
}

fn units(reading: &BlockReading) -> Vec<((usize, usize), String)> {
    let mut out = Vec::new();
    for (line, read) in reading.lines.iter().enumerate() {
        for (stop, cluster) in read.clusters.iter().enumerate() {
            out.push(((line, stop), cluster.clone()));
        }
        let last = line + 1 == reading.lines.len();
        match read.end {
            _ if last => {}
            LineEnd::Wrap => {}
            LineEnd::WrapWithSpace => out.push(((line, read.clusters.len()), " ".to_owned())),
            _ => out.push(((line, read.clusters.len()), "\n".to_owned())),
        }
    }
    out
}

fn whole(reading: &BlockReading) -> BlockRange {
    let last = reading.lines.len().saturating_sub(1);
    let stops = reading
        .lines
        .get(last)
        .map_or(0, |line| line.clusters.len());
    BlockRange::Between {
        from: (0, 0),
        to: (last, stops),
    }
}

pub fn range_within(reading: &BlockReading, find: &str) -> Result<BlockRange, String> {
    if find.is_empty() {
        return Err("`find` is empty".to_owned());
    }
    let units = units(reading);
    let mut text = String::new();
    let mut starts = Vec::with_capacity(units.len() + 1);
    for (_, unit) in &units {
        starts.push(text.len());
        text.push_str(unit);
    }
    starts.push(text.len());
    let found: Vec<usize> = text.match_indices(find).map(|(at, _)| at).collect();
    let at = match found[..] {
        [only] => only,
        [] => return Err(format!("{find:?} is not in the block")),
        _ => {
            return Err(format!(
                "{find:?} is in the block {} times: give more of the text around it so it is found once",
                found.len()
            ));
        }
    };
    let end = at + find.len();
    let first = starts.iter().position(|start| *start == at);
    let last = starts.iter().position(|start| *start == end);
    let (Some(first), Some(last)) = (first, last) else {
        return Err(format!(
            "{find:?} starts or ends inside one character of the block: include the whole character"
        ));
    };
    let position = |unit: usize| -> (usize, usize) {
        units.get(unit).map_or_else(
            || {
                let line = reading.lines.len().saturating_sub(1);
                (
                    line,
                    reading
                        .lines
                        .get(line)
                        .map_or(0, |read| read.clusters.len()),
                )
            },
            |(position, _)| *position,
        )
    };
    Ok(BlockRange::Between {
        from: position(first),
        to: position(last),
    })
}

#[cfg(test)]
pub(crate) mod tests;
