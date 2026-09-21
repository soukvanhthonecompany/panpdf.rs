pub type Box4 = [f64; 4];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Arrangement {
    AlignLeft,
    AlignRight,
    AlignTop,
    AlignBottom,
    AlignHorizontalCentres,
    AlignVerticalCentres,
    DistributeHorizontally,
    DistributeVertically,
    MatchWidth,
    MatchHeight,
    MatchBoth,
    CentreHorizontallyOnPage,
    CentreVerticallyOnPage,
}

impl Arrangement {
    #[must_use]
    pub const fn needs(self) -> usize {
        match self {
            Self::CentreHorizontallyOnPage | Self::CentreVerticallyOnPage => 1,
            Self::DistributeHorizontally | Self::DistributeVertically => 3,
            _ => 2,
        }
    }
}

fn centre_x(boxed: &Box4) -> f64 {
    f64::midpoint(boxed[0], boxed[2])
}

fn centre_y(boxed: &Box4) -> f64 {
    f64::midpoint(boxed[1], boxed[3])
}

#[must_use]
pub fn shifted(boxed: Box4, (dx, dy): (f64, f64)) -> Box4 {
    [boxed[0] + dx, boxed[1] + dy, boxed[2] + dx, boxed[3] + dy]
}

#[must_use]
pub fn arranged(
    boxes: &[Box4],
    anchor: usize,
    arrangement: Arrangement,
    page: (f64, f64),
) -> Vec<Box4> {
    let Some(held) = boxes.get(anchor).copied() else {
        return boxes.to_vec();
    };
    if boxes.len() < arrangement.needs() {
        return boxes.to_vec();
    }
    let each = |change: &dyn Fn(Box4) -> Box4| boxes.iter().map(|boxed| change(*boxed)).collect();
    match arrangement {
        Arrangement::AlignLeft => each(&|b| shifted(b, (held[0] - b[0], 0.0))),
        Arrangement::AlignRight => each(&|b| shifted(b, (held[2] - b[2], 0.0))),
        Arrangement::AlignTop => each(&|b| shifted(b, (0.0, held[1] - b[1]))),
        Arrangement::AlignBottom => each(&|b| shifted(b, (0.0, held[3] - b[3]))),
        Arrangement::AlignHorizontalCentres => {
            each(&|b| shifted(b, (centre_x(&held) - centre_x(&b), 0.0)))
        }
        Arrangement::AlignVerticalCentres => {
            each(&|b| shifted(b, (0.0, centre_y(&held) - centre_y(&b))))
        }
        Arrangement::MatchWidth => each(&|b| [b[0], b[1], b[0] + (held[2] - held[0]), b[3]]),
        Arrangement::MatchHeight => each(&|b| [b[0], b[1], b[2], b[1] + (held[3] - held[1])]),
        Arrangement::MatchBoth => each(&|b| {
            [
                b[0],
                b[1],
                b[0] + (held[2] - held[0]),
                b[1] + (held[3] - held[1]),
            ]
        }),
        Arrangement::DistributeHorizontally => {
            distributed(boxes, centre_x, |b, by| shifted(b, (by, 0.0)))
        }
        Arrangement::DistributeVertically => {
            distributed(boxes, centre_y, |b, by| shifted(b, (0.0, by)))
        }
        Arrangement::CentreHorizontallyOnPage => {
            let (left, right) = boxes
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, r), b| {
                    (l.min(b[0]), r.max(b[2]))
                });
            let by = page.0 / 2.0 - f64::midpoint(left, right);
            each(&|b| shifted(b, (by, 0.0)))
        }
        Arrangement::CentreVerticallyOnPage => {
            let (top, bottom) = boxes
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(t, u), b| {
                    (t.min(b[1]), u.max(b[3]))
                });
            let by = page.1 / 2.0 - f64::midpoint(top, bottom);
            each(&|b| shifted(b, (0.0, by)))
        }
    }
}

fn distributed(
    boxes: &[Box4],
    centre: fn(&Box4) -> f64,
    moved: fn(Box4, f64) -> Box4,
) -> Vec<Box4> {
    let mut order: Vec<usize> = (0..boxes.len()).collect();
    order.sort_by(|one, other| centre(&boxes[*one]).total_cmp(&centre(&boxes[*other])));
    let (Some(&first), Some(&last)) = (order.first(), order.last()) else {
        return boxes.to_vec();
    };
    let start = centre(&boxes[first]);
    #[allow(
        clippy::cast_precision_loss,
        reason = "a group of fields is a few hundred at most"
    )]
    let step = (centre(&boxes[last]) - start) / (order.len() - 1) as f64;
    let mut out = boxes.to_vec();
    for (rank, index) in order.into_iter().enumerate() {
        #[allow(
            clippy::cast_precision_loss,
            reason = "a group of fields is a few hundred at most"
        )]
        let wanted = start + step * rank as f64;
        out[index] = moved(boxes[index], wanted - centre(&boxes[index]));
    }
    out
}

#[cfg(test)]
#[expect(clippy::float_cmp, reason = "every number here is exact in binary")]
mod tests {
    use super::{Arrangement, arranged};

    const PAGE: (f64, f64) = (600.0, 800.0);

    fn three() -> Vec<[f64; 4]> {
        vec![
            [10.0, 10.0, 110.0, 30.0],
            [40.0, 50.0, 100.0, 60.0],
            [25.0, 200.0, 225.0, 240.0],
        ]
    }

    #[test]
    fn aligning_lines_edges_up_with_the_anchor() {
        let left = arranged(&three(), 1, Arrangement::AlignLeft, PAGE);
        assert_eq!(
            left,
            vec![
                [40.0, 10.0, 140.0, 30.0],
                [40.0, 50.0, 100.0, 60.0],
                [40.0, 200.0, 240.0, 240.0]
            ]
        );
        let right = arranged(&three(), 0, Arrangement::AlignRight, PAGE);
        assert!(right.iter().all(|b| b[2] == 110.0));
        let bottom = arranged(&three(), 2, Arrangement::AlignBottom, PAGE);
        assert!(bottom.iter().all(|b| b[3] == 240.0));
        assert_eq!(bottom[1][3] - bottom[1][1], 10.0, "a box keeps its height");
    }

    #[test]
    fn centres_line_up() {
        let across = arranged(&three(), 0, Arrangement::AlignHorizontalCentres, PAGE);
        assert!(across.iter().all(|b| f64::midpoint(b[0], b[2]) == 60.0));
        let down = arranged(&three(), 1, Arrangement::AlignVerticalCentres, PAGE);
        assert!(down.iter().all(|b| f64::midpoint(b[1], b[3]) == 55.0));
    }

    #[test]
    fn distributing_spaces_centres_evenly() {
        let boxes = vec![
            [0.0, 0.0, 10.0, 10.0],
            [100.0, 0.0, 110.0, 10.0],
            [20.0, 0.0, 30.0, 10.0],
        ];
        let spaced = arranged(&boxes, 0, Arrangement::DistributeHorizontally, PAGE);
        assert_eq!(spaced[0], boxes[0]);
        assert_eq!(spaced[1], boxes[1]);
        assert_eq!(spaced[2], [50.0, 0.0, 60.0, 10.0]);
    }

    #[test]
    fn matching_size_keeps_the_top_left_corner() {
        let both = arranged(&three(), 2, Arrangement::MatchBoth, PAGE);
        assert_eq!(both[0], [10.0, 10.0, 210.0, 50.0]);
        let wide = arranged(&three(), 0, Arrangement::MatchWidth, PAGE);
        assert_eq!(wide[2], [25.0, 200.0, 125.0, 240.0]);
        let tall = arranged(&three(), 1, Arrangement::MatchHeight, PAGE);
        assert_eq!(tall[0], [10.0, 10.0, 110.0, 20.0]);
    }

    #[test]
    fn a_group_is_centred_as_one() {
        let centred = arranged(&three(), 0, Arrangement::CentreHorizontallyOnPage, PAGE);
        let (left, right) = (
            centred.iter().map(|b| b[0]).fold(f64::INFINITY, f64::min),
            centred.iter().map(|b| b[2]).fold(0.0, f64::max),
        );
        assert_eq!(f64::midpoint(left, right), 300.0);
        assert_eq!(
            centred[1][0] - centred[0][0],
            30.0,
            "the group keeps its shape"
        );
    }

    #[test]
    fn too_few_boxes_move_nothing() {
        let one = vec![[0.0, 0.0, 10.0, 10.0]];
        assert_eq!(arranged(&one, 0, Arrangement::AlignLeft, PAGE), one);
        assert_eq!(
            arranged(&one, 0, Arrangement::DistributeHorizontally, PAGE),
            one
        );
        assert_eq!(
            arranged(&three()[..2], 0, Arrangement::DistributeVertically, PAGE),
            three()[..2].to_vec()
        );
        assert_eq!(arranged(&three(), 9, Arrangement::AlignLeft, PAGE), three());
    }
}
