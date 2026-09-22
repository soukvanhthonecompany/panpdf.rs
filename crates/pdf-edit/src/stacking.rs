#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stacking {
    ToFront,
    Forward,
    Backward,
    ToBack,
}

impl Stacking {
    pub const ALL: [Self; 4] = [Self::ToFront, Self::Forward, Self::Backward, Self::ToBack];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StackingError {
    NothingToOrder,
    NothingChosen,
    NotInTheList { index: usize, count: usize },
    ChosenTwice { index: usize },
}

impl std::fmt::Display for StackingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NothingToOrder => formatter.write_str("there is nothing on the page to order"),
            Self::NothingChosen => formatter.write_str("nothing is chosen"),
            Self::NotInTheList { index, count } => write!(
                formatter,
                "there is no {index} among the {count} things on the page"
            ),
            Self::ChosenTwice { index } => {
                write!(formatter, "{index} is chosen twice")
            }
        }
    }
}

impl std::error::Error for StackingError {}

pub fn reordered(
    count: usize,
    chosen: &[usize],
    command: Stacking,
) -> Result<Vec<usize>, StackingError> {
    if count == 0 {
        return Err(StackingError::NothingToOrder);
    }
    if chosen.is_empty() {
        return Err(StackingError::NothingChosen);
    }
    let mut is_chosen = vec![false; count];
    for index in chosen {
        if *index >= count {
            return Err(StackingError::NotInTheList {
                index: *index,
                count,
            });
        }
        if is_chosen[*index] {
            return Err(StackingError::ChosenTwice { index: *index });
        }
        is_chosen[*index] = true;
    }
    Ok(match command {
        Stacking::ToFront => gathered(count, &is_chosen, true),
        Stacking::ToBack => gathered(count, &is_chosen, false),
        Stacking::Forward => stepped(count, &is_chosen, true),
        Stacking::Backward => stepped(count, &is_chosen, false),
    })
}

#[must_use]
pub fn is_unchanged(order: &[usize]) -> bool {
    order.iter().enumerate().all(|(at, was)| at == *was)
}

fn gathered(count: usize, is_chosen: &[bool], to_front: bool) -> Vec<usize> {
    let (chosen, rest): (Vec<usize>, Vec<usize>) = (0..count).partition(|at| is_chosen[*at]);
    let mut order = Vec::with_capacity(count);
    if to_front {
        order.extend(rest);
        order.extend(chosen);
    } else {
        order.extend(chosen);
        order.extend(rest);
    }
    order
}

fn stepped(count: usize, is_chosen: &[bool], forward: bool) -> Vec<usize> {
    let mut order: Vec<usize> = (0..count).collect();
    if forward {
        let mut barrier = count;
        for at in (0..count).rev() {
            if !is_chosen[order[at]] {
                continue;
            }
            if at + 1 == barrier {
                barrier = at;
            } else {
                order.swap(at, at + 1);
            }
        }
    } else {
        let mut barrier = 0;
        for at in 0..count {
            if !is_chosen[order[at]] {
                continue;
            }
            if at == barrier {
                barrier = at + 1;
            } else {
                order.swap(at, at - 1);
            }
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::{Stacking, StackingError, is_unchanged, reordered};

    const KNOWN: &[(usize, &[usize], Stacking, &[usize])] = &[
        (1, &[0], Stacking::ToFront, &[0]),
        (1, &[0], Stacking::Forward, &[0]),
        (1, &[0], Stacking::Backward, &[0]),
        (1, &[0], Stacking::ToBack, &[0]),
        (4, &[3], Stacking::ToFront, &[0, 1, 2, 3]),
        (4, &[3], Stacking::Forward, &[0, 1, 2, 3]),
        (4, &[0], Stacking::ToBack, &[0, 1, 2, 3]),
        (4, &[0], Stacking::Backward, &[0, 1, 2, 3]),
        (5, &[0], Stacking::Forward, &[1, 0, 2, 3, 4]),
        (5, &[0], Stacking::ToFront, &[1, 2, 3, 4, 0]),
        (5, &[4], Stacking::Backward, &[0, 1, 2, 4, 3]),
        (5, &[4], Stacking::ToBack, &[4, 0, 1, 2, 3]),
        (5, &[2], Stacking::Forward, &[0, 1, 3, 2, 4]),
        (5, &[2], Stacking::Backward, &[0, 2, 1, 3, 4]),
        (5, &[2], Stacking::ToFront, &[0, 1, 3, 4, 2]),
        (5, &[2], Stacking::ToBack, &[2, 0, 1, 3, 4]),
        (5, &[0, 1], Stacking::Forward, &[2, 0, 1, 3, 4]),
        (5, &[0, 1], Stacking::ToFront, &[2, 3, 4, 0, 1]),
        (5, &[1, 3], Stacking::ToBack, &[1, 3, 0, 2, 4]),
        (5, &[1, 3], Stacking::Forward, &[0, 2, 1, 4, 3]),
        (5, &[4, 3], Stacking::Forward, &[0, 1, 2, 3, 4]),
        (5, &[0, 1], Stacking::Backward, &[0, 1, 2, 3, 4]),
        (5, &[2, 3], Stacking::Forward, &[0, 1, 4, 2, 3]),
        (3, &[0, 1, 2], Stacking::Forward, &[0, 1, 2]),
        (3, &[0, 1, 2], Stacking::ToFront, &[0, 1, 2]),
    ];

    #[test]
    fn every_known_answer_is_the_answer() {
        for (count, chosen, command, want) in KNOWN {
            let had = reordered(*count, chosen, *command).expect("the row is a fair question");
            assert_eq!(had, *want, "{count} items, {chosen:?} {command:?}");
        }
    }

    #[test]
    fn the_wrong_command_fails_a_row_the_right_one_passes() {
        for right in Stacking::ALL {
            for wrong in Stacking::ALL {
                if wrong == right {
                    continue;
                }
                let caught = KNOWN.iter().any(|(count, chosen, command, want)| {
                    *command == right
                        && reordered(*count, chosen, wrong).expect("a fair question") != *want
                });
                assert!(
                    caught,
                    "a control answering {right:?} by doing {wrong:?} passes every row of the table"
                );
            }
        }
    }

    #[test]
    fn stepping_far_enough_arrives_where_the_end_commands_go() {
        for chosen in [vec![0], vec![2], vec![0, 1], vec![1, 3]] {
            let count = 5;
            for (step, end) in [
                (Stacking::Forward, Stacking::ToFront),
                (Stacking::Backward, Stacking::ToBack),
            ] {
                let mut walked: Vec<usize> = (0..count).collect();
                for _ in 0..count {
                    let here: Vec<usize> = walked
                        .iter()
                        .enumerate()
                        .filter(|(_, was)| chosen.contains(was))
                        .map(|(at, _)| at)
                        .collect();
                    let order = reordered(count, &here, step).expect("a fair question");
                    walked = order.iter().map(|at| walked[*at]).collect();
                }
                let arrived = reordered(count, &chosen, end).expect("a fair question");
                assert_eq!(
                    walked, arrived,
                    "{chosen:?} {step:?} repeated is not {end:?}"
                );
            }
        }
    }

    #[test]
    fn nothing_is_lost_and_nothing_is_added() {
        for (count, chosen, command, _) in KNOWN {
            let mut sorted = reordered(*count, chosen, *command).expect("a fair question");
            sorted.sort_unstable();
            assert_eq!(sorted, (0..*count).collect::<Vec<_>>());
        }
    }

    #[test]
    fn a_chosen_group_keeps_its_own_order() {
        let chosen = [1, 2, 4];
        for command in Stacking::ALL {
            let order = reordered(6, &chosen, command).expect("a fair question");
            let kept: Vec<usize> = order
                .into_iter()
                .filter(|was| chosen.contains(was))
                .collect();
            assert_eq!(kept, chosen, "{command:?} shuffled the chosen among itself");
        }
    }

    #[test]
    fn an_order_that_changes_nothing_says_so() {
        assert!(is_unchanged(
            &reordered(4, &[3], Stacking::ToFront).expect("a fair question")
        ));
        assert!(!is_unchanged(
            &reordered(4, &[2], Stacking::ToFront).expect("a fair question")
        ));
    }

    #[test]
    fn what_cannot_be_ordered_is_refused_rather_than_repaired() {
        for command in Stacking::ALL {
            assert_eq!(
                reordered(0, &[0], command),
                Err(StackingError::NothingToOrder)
            );
            assert_eq!(
                reordered(3, &[], command),
                Err(StackingError::NothingChosen)
            );
            assert_eq!(
                reordered(3, &[3], command),
                Err(StackingError::NotInTheList { index: 3, count: 3 })
            );
            assert_eq!(
                reordered(3, &[1, 1], command),
                Err(StackingError::ChosenTwice { index: 1 })
            );
        }
    }
}
