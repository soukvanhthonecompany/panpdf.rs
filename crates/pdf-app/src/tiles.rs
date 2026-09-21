use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TileId {
    pub page: usize,
    pub zoom: usize,
    pub col: u32,
    pub row: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Held {
    pub box_pixels: [u32; 4],
    pub size: (usize, usize),
    pub epoch: u64,
    pub used: u64,
    pub stale: bool,
}

impl Held {
    #[must_use]
    pub const fn bytes(&self) -> usize {
        self.size.0 * self.size.1 * 4
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Slot {
    Shown,
    Waiting,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Arrival {
    Shown,
    Waiting,
}

#[derive(Debug, Default, PartialEq)]
pub struct Swap {
    pub shown: Vec<TileId>,
    pub dropped: Vec<(TileId, Held)>,
}

#[derive(Debug, Default)]
pub struct Ledger {
    held: BTreeMap<TileId, Held>,
    waiting: BTreeMap<TileId, Held>,
}

impl Ledger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.held.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    #[must_use]
    pub fn waiting_count(&self) -> usize {
        self.waiting.len()
    }

    #[must_use]
    pub fn get(&self, id: TileId) -> Option<&Held> {
        self.held.get(&id)
    }

    #[must_use]
    pub fn waiting(&self, id: TileId) -> Option<&Held> {
        self.waiting.get(&id)
    }

    pub fn on_page(&self, page: usize) -> impl Iterator<Item = (TileId, &Held)> {
        self.held
            .iter()
            .filter(move |(id, _)| id.page == page)
            .map(|(id, held)| (*id, held))
    }

    pub fn adopt(&mut self, id: TileId, held: Held) {
        self.waiting.remove(&id);
        self.held.insert(id, held);
    }

    pub fn arrive(&mut self, id: TileId, held: Held) -> Arrival {
        if self.stale_on(id.page) {
            self.waiting.insert(id, held);
            Arrival::Waiting
        } else {
            self.adopt(id, held);
            Arrival::Shown
        }
    }

    pub fn touched_on(&mut self, id: TileId, frame: u64) {
        if let Some(held) = self.held.get_mut(&id) {
            held.used = frame;
        }
    }

    #[must_use]
    pub fn wants(&self, id: TileId, epoch: u64) -> bool {
        if self
            .waiting
            .get(&id)
            .is_some_and(|waiting| waiting.epoch == epoch)
        {
            return false;
        }
        self.held.get(&id).is_none_or(|held| held.epoch != epoch)
    }

    pub fn commit(
        &mut self,
        page: usize,
        touched: Option<&BTreeMap<usize, [u32; 4]>>,
        epoch: u64,
        showing: usize,
    ) -> Vec<(TileId, Held, Slot)> {
        let still_true = |id: &TileId, held: &Held| {
            id.page != page
                || touched.is_some_and(|touched| {
                    touched
                        .get(&id.zoom)
                        .is_some_and(|region| !overlap(held.box_pixels, *region))
                })
        };
        let mut dropped = Vec::new();
        self.held.retain(|id, held| {
            if still_true(id, held) {
                if !held.stale {
                    held.epoch = epoch;
                }
                return true;
            }
            if id.zoom == showing {
                held.stale = true;
                return true;
            }
            dropped.push((*id, *held, Slot::Shown));
            false
        });
        self.waiting.retain(|id, held| {
            if still_true(id, held) {
                held.epoch = epoch;
                return true;
            }
            dropped.push((*id, *held, Slot::Waiting));
            false
        });
        dropped
    }

    pub fn swap_if_ready(&mut self, page: usize, visible: &BTreeSet<TileId>) -> Option<Swap> {
        let has_waiting = self.waiting.keys().any(|id| id.page == page);
        if !self.stale_on(page) && !has_waiting {
            return None;
        }
        let ready = self
            .held
            .iter()
            .filter(|(id, held)| id.page == page && held.stale && visible.contains(id))
            .all(|(id, _)| self.waiting.contains_key(id));
        if !ready {
            return None;
        }
        let mut swap = Swap::default();
        let arrived: Vec<TileId> = self
            .waiting
            .keys()
            .copied()
            .filter(|id| id.page == page)
            .collect();
        for id in arrived {
            if let Some(held) = self.waiting.remove(&id) {
                self.held.insert(id, held);
                swap.shown.push(id);
            }
        }
        self.held.retain(|id, held| {
            if id.page == page && held.stale {
                swap.dropped.push((*id, *held));
                return false;
            }
            true
        });
        Some(swap)
    }

    pub fn give_up(&mut self, id: TileId) -> Option<Held> {
        if self.held.get(&id).is_some_and(|held| held.stale) {
            return self.held.remove(&id);
        }
        None
    }

    pub fn drop_stale_except(&mut self, showing: usize) -> Vec<(TileId, Held, Slot)> {
        let mut dropped = Vec::new();
        self.held.retain(|id, held| {
            if held.stale && id.zoom != showing {
                dropped.push((*id, *held, Slot::Shown));
                return false;
            }
            true
        });
        dropped
    }

    #[must_use]
    pub fn stale_on(&self, page: usize) -> bool {
        self.held
            .iter()
            .any(|(id, held)| id.page == page && held.stale)
    }

    pub fn forget_page(&mut self, page: usize) -> Vec<(TileId, Held, Slot)> {
        let mut dropped = Vec::new();
        self.held.retain(|id, held| {
            if id.page == page {
                dropped.push((*id, *held, Slot::Shown));
                return false;
            }
            true
        });
        self.waiting.retain(|id, held| {
            if id.page == page {
                dropped.push((*id, *held, Slot::Waiting));
                return false;
            }
            true
        });
        dropped
    }

    pub fn clear(&mut self) -> Vec<(TileId, Held, Slot)> {
        let mut dropped: Vec<(TileId, Held, Slot)> = std::mem::take(&mut self.held)
            .into_iter()
            .map(|(id, held)| (id, held, Slot::Shown))
            .collect();
        dropped.extend(
            std::mem::take(&mut self.waiting)
                .into_iter()
                .map(|(id, held)| (id, held, Slot::Waiting)),
        );
        dropped
    }

    #[must_use]
    pub fn held_bytes(&self) -> usize {
        self.held.values().map(Held::bytes).sum()
    }

    pub fn evict(&mut self, budget: usize) -> Vec<(TileId, Held)> {
        let mut over = self.held_bytes();
        if over <= budget {
            return Vec::new();
        }
        let mut ages: Vec<(u64, TileId)> = self
            .held
            .iter()
            .filter(|(_, held)| !held.stale)
            .map(|(id, held)| (held.used, *id))
            .collect();
        ages.sort_unstable();
        let mut dropped = Vec::new();
        for (_, id) in ages {
            if over <= budget {
                break;
            }
            if let Some(held) = self.held.remove(&id) {
                over -= held.bytes();
                dropped.push((id, held));
            }
        }
        dropped
    }
}

const fn overlap(one: [u32; 4], other: [u32; 4]) -> bool {
    one[0] < other[2] && other[0] < one[2] && one[1] < other[3] && other[1] < one[3]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(page: usize, zoom: usize, col: u32, row: u32) -> TileId {
        TileId {
            page,
            zoom,
            col,
            row,
        }
    }

    const SQUARE: usize = 512 * 512 * 4;

    fn held(box_pixels: [u32; 4]) -> Held {
        Held {
            box_pixels,
            size: (512, 512),
            epoch: 0,
            used: 0,
            stale: false,
        }
    }

    fn drawn(box_pixels: [u32; 4], epoch: u64) -> Held {
        Held {
            epoch,
            ..held(box_pixels)
        }
    }

    fn ids(dropped: &[(TileId, Held, Slot)]) -> Vec<TileId> {
        dropped.iter().map(|(id, _, _)| *id).collect()
    }

    const SHOWING: usize = 4;

    fn ledger() -> Ledger {
        let mut ledger = Ledger::new();
        ledger.adopt(id(0, 4, 0, 0), held([0, 0, 512, 512]));
        ledger.adopt(id(0, 4, 1, 0), held([512, 0, 1024, 512]));
        ledger.adopt(id(0, 4, 0, 1), held([0, 512, 512, 1024]));
        ledger.adopt(id(0, 0, 0, 0), held([0, 0, 149, 211]));
        ledger.adopt(id(1, 4, 0, 0), held([0, 0, 512, 512]));
        ledger
    }

    #[test]
    fn a_commit_keeps_the_touched_tile_on_screen_and_drops_it_everywhere_else() {
        let mut ledger = ledger();
        let touched = BTreeMap::from([(4, [600, 100, 700, 200]), (0, [40, 20, 60, 40])]);

        let dropped = ledger.commit(0, Some(&touched), 1, SHOWING);

        assert_eq!(
            ids(&dropped),
            vec![id(0, 0, 0, 0)],
            "the coarse tile covers the whole page so every edit touches it, and \
             it is not the rung being shown"
        );
        let stale = ledger
            .get(id(0, 4, 1, 0))
            .expect("kept, at the rung on screen");
        assert!(stale.stale, "and marked as the old picture it is");
        assert_eq!(stale.epoch, 0, "at its old epoch, so it is asked for again");
        assert!(ledger.get(id(0, 0, 0, 0)).is_none());
        assert_eq!(ledger.get(id(0, 4, 0, 0)).map(|h| h.epoch), Some(1));
        assert_eq!(ledger.get(id(0, 4, 0, 1)).map(|h| h.epoch), Some(1));
        assert!(!ledger.get(id(0, 4, 0, 0)).expect("kept").stale);
    }

    #[test]
    fn a_stale_tile_is_still_asked_for() {
        let mut ledger = ledger();
        let touched = BTreeMap::from([(4, [600, 100, 700, 200]), (0, [40, 20, 60, 40])]);

        ledger.commit(0, Some(&touched), 1, SHOWING);

        assert!(
            ledger.wants(id(0, 4, 1, 0), 1),
            "it is stale, so it is wanted"
        );
        assert!(ledger.stale_on(0));
        assert!(!ledger.stale_on(1));
    }

    #[test]
    fn the_tile_that_replaces_it_ends_the_staleness() {
        let mut ledger = ledger();
        let touched = BTreeMap::from([(4, [600, 100, 700, 200]), (0, [40, 20, 60, 40])]);
        ledger.commit(0, Some(&touched), 1, SHOWING);

        ledger.adopt(id(0, 4, 1, 0), drawn([512, 0, 1024, 512], 1));

        assert!(!ledger.get(id(0, 4, 1, 0)).expect("replaced").stale);
        assert!(!ledger.wants(id(0, 4, 1, 0), 1));
        assert!(!ledger.stale_on(0), "the edit is on screen now");
    }

    #[test]
    fn a_stale_tile_the_next_commit_misses_is_still_stale_and_still_wanted() {
        let (a, b) = (id(0, 4, 0, 0), id(0, 4, 1, 0));
        let mut ledger = Ledger::new();
        ledger.adopt(a, held([0, 0, 100, 100]));
        ledger.adopt(b, held([100, 0, 200, 100]));

        ledger.commit(0, Some(&BTreeMap::from([(4, [0, 0, 200, 100])])), 1, 4);
        assert!(ledger.wants(a, 1) && ledger.wants(b, 1));
        ledger.adopt(b, drawn([100, 0, 200, 100], 1));
        ledger.commit(0, Some(&BTreeMap::from([(4, [100, 0, 200, 100])])), 2, 4);

        let left = ledger.get(a).expect("kept on screen");
        assert_eq!((left.epoch, left.stale), (0, true));
        assert!(ledger.wants(a, 2), "still owed its drawing");
        assert!(ledger.wants(b, 2), "and its neighbour is owed a new one");

        let mut control = Ledger::new();
        control.adopt(a, held([0, 0, 100, 100]));
        control.commit(0, Some(&BTreeMap::from([(4, [100, 0, 200, 100])])), 1, 4);
        assert!(!control.wants(a, 1));
        control.commit(0, Some(&BTreeMap::from([(4, [0, 0, 100, 100])])), 2, 4);
        assert!(control.wants(a, 2));
        control.adopt(a, drawn([0, 0, 100, 100], 2));
        assert!(!control.wants(a, 2));
    }

    #[test]
    fn another_pages_stale_tile_stays_stale_across_a_commit_elsewhere() {
        let mut ledger = ledger();
        ledger.commit(1, None, 1, SHOWING);
        assert!(ledger.get(id(1, 4, 0, 0)).expect("kept").stale);

        ledger.commit(0, Some(&BTreeMap::from([(4, [0, 0, 1, 1])])), 2, SHOWING);

        let other = ledger.get(id(1, 4, 0, 0)).expect("still there");
        assert_eq!((other.epoch, other.stale), (0, true));
        assert!(ledger.wants(id(1, 4, 0, 0), 2));
    }

    #[test]
    fn replacements_wait_and_go_on_screen_together() {
        let (a, b) = (id(0, 4, 0, 0), id(0, 4, 1, 0));
        let mut ledger = Ledger::new();
        ledger.adopt(a, held([0, 0, 512, 512]));
        ledger.adopt(b, held([512, 0, 1024, 512]));
        ledger.commit(0, None, 1, 4);
        let visible = BTreeSet::from([a, b]);

        assert_eq!(
            ledger.arrive(b, drawn([512, 0, 1024, 512], 1)),
            Arrival::Waiting
        );
        assert!(!ledger.wants(b, 1), "its replacement is here");
        assert_eq!(
            ledger.get(b).map(|held| held.epoch),
            Some(0),
            "still the old picture"
        );
        assert_eq!(ledger.swap_if_ready(0, &visible), None, "a is still owed");

        assert_eq!(
            ledger.arrive(a, drawn([0, 0, 512, 512], 1)),
            Arrival::Waiting
        );
        let swap = ledger.swap_if_ready(0, &visible).expect("ready");
        assert_eq!(swap.shown, vec![a, b]);
        assert!(swap.dropped.is_empty());
        assert!(!ledger.stale_on(0));
        assert_eq!(
            ledger.get(a).map(|held| (held.epoch, held.stale)),
            Some((1, false))
        );
        assert_eq!(
            ledger.get(b).map(|held| (held.epoch, held.stale)),
            Some((1, false))
        );

        assert_eq!(ledger.arrive(a, drawn([0, 0, 512, 512], 1)), Arrival::Shown);
    }

    #[test]
    fn an_unseen_stale_tile_does_not_hold_the_swap_and_goes_with_it() {
        let (seen, unseen) = (id(0, 4, 0, 0), id(0, 4, 0, 5));
        let mut ledger = Ledger::new();
        ledger.adopt(seen, held([0, 0, 512, 512]));
        ledger.adopt(unseen, held([0, 2560, 512, 3072]));
        ledger.commit(0, None, 1, 4);

        ledger.arrive(seen, drawn([0, 0, 512, 512], 1));
        let swap = ledger
            .swap_if_ready(0, &BTreeSet::from([seen]))
            .expect("ready");

        assert_eq!(swap.shown, vec![seen]);
        assert_eq!(
            swap.dropped.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![unseen]
        );
        assert!(ledger.get(unseen).is_none());
        assert!(ledger.wants(unseen, 1));
    }

    #[test]
    fn a_commit_drops_a_waiting_tile_it_touches_and_carries_one_it_misses() {
        let (a, b) = (id(0, 4, 0, 0), id(0, 4, 1, 0));
        let mut ledger = Ledger::new();
        ledger.adopt(a, held([0, 0, 512, 512]));
        ledger.adopt(b, held([512, 0, 1024, 512]));
        ledger.commit(0, None, 1, 4);
        ledger.arrive(a, drawn([0, 0, 512, 512], 1));
        ledger.arrive(b, drawn([512, 0, 1024, 512], 1));

        let dropped = ledger.commit(0, Some(&BTreeMap::from([(4, [600, 0, 700, 10])])), 2, 4);

        assert_eq!(
            dropped
                .iter()
                .map(|(id, _, slot)| (*id, *slot))
                .collect::<Vec<_>>(),
            vec![(b, Slot::Waiting)]
        );
        assert_eq!(ledger.waiting(a).map(|held| held.epoch), Some(2));
        assert!(!ledger.wants(a, 2));
        assert!(ledger.wants(b, 2));
        assert_eq!(
            ledger.get(b).map(|held| held.stale),
            Some(true),
            "still shown meanwhile"
        );
    }

    #[test]
    fn a_tile_that_cannot_be_drawn_stops_holding_the_swap() {
        let (a, b) = (id(0, 4, 0, 0), id(0, 4, 1, 0));
        let mut ledger = Ledger::new();
        ledger.adopt(a, held([0, 0, 512, 512]));
        ledger.adopt(b, held([512, 0, 1024, 512]));
        ledger.commit(0, None, 1, 4);
        ledger.arrive(b, drawn([512, 0, 1024, 512], 1));
        let visible = BTreeSet::from([a, b]);
        assert_eq!(ledger.swap_if_ready(0, &visible), None);

        assert!(ledger.give_up(a).is_some());

        let swap = ledger.swap_if_ready(0, &visible).expect("ready");
        assert_eq!(swap.shown, vec![b]);
        assert!(ledger.give_up(b).is_none());
    }

    #[test]
    fn a_stale_tile_stops_being_worth_keeping_when_the_rung_moves() {
        let mut ledger = ledger();
        let touched = BTreeMap::from([(4, [600, 100, 700, 200]), (0, [40, 20, 60, 40])]);
        ledger.commit(0, Some(&touched), 1, SHOWING);

        let dropped = ledger.drop_stale_except(7);

        assert_eq!(ids(&dropped), vec![id(0, 4, 1, 0)]);
        assert!(!ledger.stale_on(0));
        assert_eq!(
            ledger.on_page(0).count(),
            2,
            "the tiles the region missed are still true and still there"
        );
    }

    #[test]
    fn a_tile_the_region_misses_is_not_asked_for_again() {
        let mut ledger = ledger();
        let touched = BTreeMap::from([(4, [600, 100, 700, 200]), (0, [0, 0, 1, 1])]);

        ledger.commit(0, Some(&touched), 1, SHOWING);

        assert!(!ledger.wants(id(0, 4, 0, 0), 1));
        assert!(ledger.wants(id(0, 4, 1, 0), 1), "it was dropped");
    }

    #[test]
    fn a_stale_tile_at_another_zoom_is_dropped_at_once() {
        let mut ledger = ledger();
        let touched = BTreeMap::from([(4, [0, 0, 2380, 3368]), (0, [0, 0, 149, 211])]);

        ledger.commit(0, Some(&touched), 1, SHOWING);

        assert!(
            ledger.on_page(0).all(|(id, _)| id.zoom == SHOWING),
            "nothing is left of the old page except at the rung on screen"
        );
        assert_eq!(ledger.on_page(1).count(), 1, "another page is untouched");
    }

    #[test]
    fn a_step_that_declared_no_region_makes_the_whole_page_untrue() {
        let mut ledger = ledger();

        ledger.commit(0, None, 1, SHOWING);

        assert!(
            ledger
                .on_page(0)
                .all(|(id, held)| id.zoom == SHOWING && held.stale),
            "every tile of the page is stale, and only the rung on screen is kept"
        );
        assert_eq!(ledger.on_page(1).count(), 1);
    }

    #[test]
    fn a_rung_the_caller_could_not_place_is_dropped() {
        let mut ledger = ledger();
        let touched = BTreeMap::from([(4, [600, 100, 700, 200])]);

        ledger.commit(0, Some(&touched), 1, SHOWING);

        assert!(
            ledger.get(id(0, 0, 0, 0)).is_none(),
            "rung 0 was not placed, and it is not the rung on screen"
        );
        assert!(ledger.get(id(0, 4, 0, 0)).is_some());
    }

    #[test]
    fn a_rung_on_screen_the_caller_could_not_place_is_kept_stale() {
        let mut ledger = ledger();
        let touched = BTreeMap::from([(0, [40, 20, 60, 40])]);

        ledger.commit(0, Some(&touched), 1, SHOWING);

        assert!(
            ledger
                .on_page(0)
                .all(|(id, held)| id.zoom != SHOWING || held.stale)
        );
        assert!(ledger.stale_on(0));
    }

    #[test]
    fn another_pages_tiles_survive_a_commit_at_the_new_epoch() {
        let mut ledger = ledger();
        let touched = BTreeMap::from([(4, [0, 0, 2380, 3368]), (0, [0, 0, 149, 211])]);

        ledger.commit(0, Some(&touched), 1, SHOWING);

        assert_eq!(ledger.get(id(1, 4, 0, 0)).map(|h| h.epoch), Some(1));
        assert!(!ledger.wants(id(1, 4, 0, 0), 1));
    }

    #[test]
    fn eviction_takes_the_ones_drawn_longest_ago() {
        let mut ledger = Ledger::new();
        for row in 0..4 {
            ledger.adopt(id(0, 4, 0, row), held([0, 0, 512, 512]));
        }
        ledger.touched_on(id(0, 4, 0, 3), 10);
        ledger.touched_on(id(0, 4, 0, 2), 9);

        let dropped = ledger.evict(2 * SQUARE);

        let gone: Vec<TileId> = dropped.iter().map(|(id, _)| *id).collect();
        assert_eq!(gone, vec![id(0, 4, 0, 0), id(0, 4, 0, 1)]);
        assert_eq!(ledger.len(), 2);
    }

    #[test]
    fn eviction_leaves_a_stale_tile_alone() {
        let mut ledger = Ledger::new();
        for row in 0..3 {
            ledger.adopt(id(0, 4, 0, row), held([0, 512 * row, 512, 512 * (row + 1)]));
        }
        ledger.commit(0, Some(&BTreeMap::from([(4, [0, 0, 512, 1])])), 1, 4);
        ledger.touched_on(id(0, 4, 0, 2), 10);

        let dropped = ledger.evict(SQUARE);

        assert_eq!(
            dropped.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![id(0, 4, 0, 1), id(0, 4, 0, 2)]
        );
        assert!(ledger.get(id(0, 4, 0, 0)).is_some_and(|held| held.stale));
    }

    #[test]
    fn the_budget_counts_what_a_tile_costs_not_how_many_there_are() {
        let mut ledger = Ledger::new();
        for row in 0..4 {
            let mut small = held([0, 256 * row, 256, 256 * (row + 1)]);
            small.size = (256, 256);
            ledger.adopt(id(0, 4, 0, row), small);
        }
        assert_eq!(ledger.held_bytes(), SQUARE);

        assert!(ledger.evict(SQUARE).is_empty());
        assert_eq!(ledger.len(), 4);

        ledger.touched_on(id(0, 4, 0, 2), 9);
        ledger.touched_on(id(0, 4, 0, 3), 10);
        let dropped = ledger.evict(SQUARE / 2);
        assert_eq!(
            dropped.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![id(0, 4, 0, 0), id(0, 4, 0, 1)]
        );
        assert_eq!(ledger.held_bytes(), SQUARE / 2);
    }

    #[test]
    fn eviction_below_the_limit_drops_nothing() {
        let mut ledger = ledger();

        assert!(ledger.evict(100 * SQUARE).is_empty());
        assert_eq!(ledger.len(), 5);
    }
}
