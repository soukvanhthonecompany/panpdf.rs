#[must_use]
pub fn every(count: usize, size: usize) -> Vec<Vec<usize>> {
    if size == 0 {
        return Vec::new();
    }
    (0..count)
        .step_by(size)
        .map(|first| (first..(first + size).min(count)).collect())
        .collect()
}

#[must_use]
pub fn at(count: usize, starts: &[usize]) -> Vec<Vec<usize>> {
    if count == 0 {
        return Vec::new();
    }
    let mut edges: Vec<usize> = starts
        .iter()
        .copied()
        .filter(|start| *start > 0 && *start < count)
        .collect();
    edges.push(0);
    edges.sort_unstable();
    edges.dedup();
    edges.push(count);
    edges
        .windows(2)
        .map(|pair| (pair[0]..pair[1]).collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{at, every};

    #[test]
    fn files_of_so_many_pages_leave_the_remainder_in_the_last() {
        assert_eq!(every(5, 2), vec![vec![0, 1], vec![2, 3], vec![4]]);
        assert_eq!(every(4, 2), vec![vec![0, 1], vec![2, 3]]);
        assert_eq!(every(3, 1), vec![vec![0], vec![1], vec![2]]);
        assert_eq!(every(3, 9), vec![vec![0, 1, 2]]);
        assert!(every(3, 0).is_empty(), "no file holds no pages");
        assert!(every(0, 2).is_empty(), "a document of no pages");
    }

    #[test]
    fn files_start_where_they_are_told_to() {
        assert_eq!(at(6, &[2, 4]), vec![vec![0, 1], vec![2, 3], vec![4, 5]]);
        assert_eq!(at(6, &[4, 2, 4, 0, 6, 99]), at(6, &[2, 4]));
        assert_eq!(at(6, &[]), vec![vec![0, 1, 2, 3, 4, 5]]);
        assert_eq!(at(1, &[0]), vec![vec![0]]);
        assert!(at(0, &[2]).is_empty(), "a document of no pages");
    }

    #[test]
    fn every_page_lands_in_exactly_one_file() {
        for pieces in [every(17, 5), at(17, &[1, 9, 16]), every(17, 1)] {
            let all: Vec<usize> = pieces.concat();
            assert_eq!(all, (0..17).collect::<Vec<usize>>());
        }
    }
}
