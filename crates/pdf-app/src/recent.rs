use std::path::{Path, PathBuf};

pub const KEPT: usize = 20;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Recent {
    pub path: PathBuf,
    pub page: usize,
    pub when: u64,
}

#[must_use]
pub fn read(text: &str) -> Vec<Recent> {
    let mut list: Vec<Recent> = Vec::new();
    for line in text.lines() {
        let mut fields = line.splitn(3, '\t');
        let (Some(when), Some(page), Some(path)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let (Ok(when), Ok(page)) = (when.parse(), page.parse()) else {
            continue;
        };
        if path.is_empty() || list.iter().any(|kept| kept.path == Path::new(path)) {
            continue;
        }
        list.push(Recent {
            path: PathBuf::from(path),
            page,
            when,
        });
    }
    list.truncate(KEPT);
    list
}

#[must_use]
pub fn write(list: &[Recent]) -> String {
    list.iter()
        .filter_map(|entry| {
            let path = writable(&entry.path)?;
            Some(format!("{}\t{}\t{path}\n", entry.when, entry.page))
        })
        .collect()
}

#[must_use]
pub fn remember(list: &[Recent], path: &Path, page: usize, when: u64) -> Vec<Recent> {
    if writable(path).is_none() {
        return list.to_vec();
    }
    let mut next = vec![Recent {
        path: path.to_path_buf(),
        page,
        when,
    }];
    next.extend(list.iter().filter(|kept| kept.path != path).cloned());
    next.truncate(KEPT);
    next
}

#[must_use]
pub fn forget(list: &[Recent], path: &Path) -> Vec<Recent> {
    list.iter()
        .filter(|kept| kept.path != path)
        .cloned()
        .collect()
}

fn writable(path: &Path) -> Option<&str> {
    let text = path.to_str()?;
    (!text.is_empty() && !text.contains(['\n', '\r'])).then_some(text)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ago {
    JustNow,
    Minutes(u64),
    Hours(u64),
    Yesterday,
    Days(u64),
    Long,
}

#[must_use]
pub const fn ago(then: u64, now: u64) -> Ago {
    let seconds = now.saturating_sub(then);
    let minutes = seconds / 60;
    let hours = minutes / 60;
    let days = hours / 24;
    if minutes == 0 {
        Ago::JustNow
    } else if hours == 0 {
        Ago::Minutes(minutes)
    } else if days == 0 {
        Ago::Hours(hours)
    } else if days == 1 {
        Ago::Yesterday
    } else if days <= 30 {
        Ago::Days(days)
    } else {
        Ago::Long
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Ago, KEPT, Recent, ago, forget, read, remember, write};

    fn entry(path: &str, page: usize, when: u64) -> Recent {
        Recent {
            path: PathBuf::from(path),
            page,
            when,
        }
    }

    #[test]
    fn a_list_reads_back_as_it_was_written() {
        let list = vec![
            entry("/home/someone/report.pdf", 4, 1_700_000_000),
            entry("/home/someone/tab\there.pdf", 0, 1_600_000_000),
            entry("/home/someone/ລາວ ไทย.pdf", 260, 5),
        ];
        let text = write(&list);
        assert_eq!(
            text,
            "1700000000\t4\t/home/someone/report.pdf\n1600000000\t0\t/home/someone/tab\there.pdf\n5\t260\t/home/someone/ລາວ ไทย.pdf\n"
        );
        assert_eq!(read(&text), list);
    }

    #[test]
    fn a_damaged_line_costs_only_itself() {
        let text = "12\t3\t/a.pdf\nnot a line\nx\t1\t/b.pdf\n9\t-1\t/c.pdf\n7\t2\t\n8\t1\t/a.pdf\n5\t0\t/d.pdf";
        assert_eq!(
            read(text),
            vec![entry("/a.pdf", 3, 12), entry("/d.pdf", 0, 5)]
        );
    }

    #[test]
    fn remembering_moves_a_document_to_the_head_once() {
        let list = vec![entry("/a.pdf", 1, 10), entry("/b.pdf", 2, 20)];
        let next = remember(&list, Path::new("/b.pdf"), 7, 30);
        assert_eq!(next, vec![entry("/b.pdf", 7, 30), entry("/a.pdf", 1, 10)]);
        let next = remember(&next, Path::new("/c.pdf"), 0, 40);
        assert_eq!(next.len(), 3);
        assert_eq!(next[0], entry("/c.pdf", 0, 40));
        assert_eq!(forget(&next, Path::new("/b.pdf")).len(), 2);
    }

    #[test]
    fn the_list_is_bounded_and_refuses_a_path_it_could_not_write_back() {
        let mut list = Vec::new();
        for n in 0..KEPT + 5 {
            list = remember(&list, Path::new(&format!("/{n}.pdf")), 0, n as u64);
        }
        assert_eq!(list.len(), KEPT);
        assert_eq!(list[0].path, Path::new(&format!("/{}.pdf", KEPT + 4)));
        assert_eq!(remember(&list, Path::new("/a\nb.pdf"), 0, 99), list);
        assert_eq!(remember(&list, Path::new(""), 0, 99), list);
    }

    #[test]
    fn ages_step_at_their_boundaries() {
        let now = 1_700_000_000;
        assert_eq!(ago(now - 59, now), Ago::JustNow);
        assert_eq!(ago(now - 60, now), Ago::Minutes(1));
        assert_eq!(ago(now - 3_599, now), Ago::Minutes(59));
        assert_eq!(ago(now - 3_600, now), Ago::Hours(1));
        assert_eq!(ago(now - 86_399, now), Ago::Hours(23));
        assert_eq!(ago(now - 86_400, now), Ago::Yesterday);
        assert_eq!(ago(now - 2 * 86_400, now), Ago::Days(2));
        assert_eq!(ago(now - 30 * 86_400, now), Ago::Days(30));
        assert_eq!(ago(now - 31 * 86_400, now), Ago::Long);
        assert_eq!(ago(now + 500, now), Ago::JustNow);
    }
}
