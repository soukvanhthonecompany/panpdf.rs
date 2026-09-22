pub type Box4 = [f64; 4];

#[must_use]
pub fn offset(pointer: Option<(f64, f64)>, bounds: Box4) -> (f64, f64) {
    match pointer {
        Some((x, y)) => (x - bounds[0], y - bounds[1]),
        None => (0.0, 0.0),
    }
}

#[must_use]
pub fn bounds_of(boxes: impl IntoIterator<Item = Box4>) -> Option<Box4> {
    boxes.into_iter().reduce(|one, other| {
        [
            one[0].min(other[0]),
            one[1].min(other[1]),
            one[2].max(other[2]),
            one[3].max(other[3]),
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::{bounds_of, offset};

    #[test]
    fn the_top_left_corner_lands_under_the_pointer() {
        let moved = offset(Some((100.0, 200.0)), [30.0, 50.0, 80.0, 90.0]);
        assert_eq!(moved, (70.0, 150.0));
        assert_eq!((30.0 + moved.0, 50.0 + moved.1), (100.0, 200.0));
        assert_eq!(
            offset(Some((30.0, 50.0)), [30.0, 50.0, 80.0, 90.0]),
            (0.0, 0.0),
            "a pointer already on the corner moves nothing"
        );
    }

    #[test]
    fn with_no_pointer_the_copy_lands_on_the_original() {
        let bounds = [30.0, 50.0, 80.0, 90.0];
        assert_eq!(offset(None, bounds), (0.0, 0.0));
        assert_ne!(
            offset(Some((0.0, 0.0)), bounds),
            offset(None, bounds),
            "a pointer at the origin is not the same as no pointer"
        );
    }

    #[test]
    fn it_is_the_top_left_corner_and_not_another_point_of_the_box() {
        let bounds = [30.0, 50.0, 80.0, 90.0];
        let pointer = (100.0, 200.0);
        let centre = (pointer.0 - 55.0, pointer.1 - 70.0);
        let bottom_right = (pointer.0 - 80.0, pointer.1 - 90.0);
        let got = offset(Some(pointer), bounds);
        assert_ne!(got, centre);
        assert_ne!(got, bottom_right);
    }

    #[test]
    fn several_boxes_are_one_box_round_them_all() {
        assert_eq!(
            bounds_of([[30.0, 50.0, 80.0, 90.0], [10.0, 70.0, 40.0, 120.0]]),
            Some([10.0, 50.0, 80.0, 120.0])
        );
        assert_eq!(
            bounds_of([[1.0, 2.0, 3.0, 4.0]]),
            Some([1.0, 2.0, 3.0, 4.0])
        );
        assert_eq!(bounds_of([]), None);
    }
}
