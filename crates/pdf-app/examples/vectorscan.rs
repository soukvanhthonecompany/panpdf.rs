use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};

fn percent(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        return 0.0;
    }
    let (part, whole) = (
        u32::try_from(part).unwrap_or(u32::MAX),
        u32::try_from(whole).unwrap_or(u32::MAX),
    );
    100.0 * f64::from(part) / f64::from(whole)
}

#[derive(Default)]
struct Tally {
    images: usize,
    served: usize,
    many_atoms: usize,
    too_big: usize,
    no_bounds: usize,
    shading: usize,
    form: usize,
    widest: usize,
}

impl Tally {
    fn paths(&self) -> usize {
        self.served + self.too_big + self.no_bounds
    }

    fn add(&mut self, other: &Self) {
        self.images += other.images;
        self.served += other.served;
        self.many_atoms += other.many_atoms;
        self.too_big += other.too_big;
        self.no_bounds += other.no_bounds;
        self.shading += other.shading;
        self.form += other.form;
        self.widest = self.widest.max(other.widest);
    }
}

fn scan(view: &pdf_session::PageView) -> Tally {
    let mut tally = Tally::default();
    let [x0, y0, x1, y1] = view.program.geometry.media_box;
    let paper = (x1 - x0).abs() * (y1 - y0).abs();
    for object in &view.index.objects {
        match object.kind {
            pdf_semantics::ObjectKind::Image => tally.images += 1,
            pdf_semantics::ObjectKind::Shading => tally.shading += 1,
            pdf_semantics::ObjectKind::Form => tally.form += 1,
            pdf_semantics::ObjectKind::Path => {
                if object.members.len() > 1 {
                    tally.many_atoms += 1;
                    tally.widest = tally.widest.max(object.members.len());
                }
                let Some([bx0, by0, bx1, by1]) = object.bounds else {
                    tally.no_bounds += 1;
                    continue;
                };
                if (bx1 - bx0).abs() * (by1 - by0).abs() * 4.0 <= paper {
                    tally.served += 1;
                } else {
                    tally.too_big += 1;
                }
            }
            pdf_semantics::ObjectKind::Text(_) | pdf_semantics::ObjectKind::TextRun => {}
        }
    }
    tally
}

fn main() {
    let target = std::env::args().nth(1).expect("a directory or a file");
    let page_number = std::env::args()
        .skip_while(|argument| argument != "--page")
        .nth(1)
        .and_then(|value| value.parse::<usize>().ok())
        .map_or(0, |value| value.saturating_sub(1));
    let target = std::path::PathBuf::from(target);
    let mut paths: Vec<_> = if target.is_dir() {
        std::fs::read_dir(&target)
            .expect("readdir")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
            })
            .collect()
    } else {
        vec![target]
    };
    paths.sort();

    let (mut total, mut pages) = (Tally::default(), 0_usize);
    for path in &paths {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
        let Ok(view) = pdf_session::interpret_page(&source, page_number) else {
            continue;
        };
        pages += 1;
        let here = scan(&view);
        total.add(&here);
        let name = path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        eprintln!(
            "{name}: images={} paths={} served={} many_atoms={} too_big={} no_bounds={} \
             shading={} form={}",
            here.images,
            here.paths(),
            here.served,
            here.many_atoms,
            here.too_big,
            here.no_bounds,
            here.shading,
            here.form,
        );
    }

    let drawings = total.paths() + total.shading + total.form;
    let reachable = total.served;
    println!("pages {pages}");
    println!("images {} (all served)", total.images);
    println!("drawings {drawings}, of which reachable {reachable}");
    println!(
        "  served     {:6}  {:5.1}%",
        total.served,
        percent(total.served, drawings)
    );
    println!(
        "  many_atoms {:6}  {:5.1}%  (widest {} atoms)",
        total.many_atoms,
        percent(total.many_atoms, drawings),
        total.widest
    );
    println!(
        "  too_big    {:6}  {:5.1}%",
        total.too_big,
        percent(total.too_big, drawings)
    );
    println!(
        "  no_bounds  {:6}  {:5.1}%",
        total.no_bounds,
        percent(total.no_bounds, drawings)
    );
    println!(
        "  shading    {:6}  {:5.1}%",
        total.shading,
        percent(total.shading, drawings)
    );
    println!(
        "  form       {:6}  {:5.1}%",
        total.form,
        percent(total.form, drawings)
    );
}
