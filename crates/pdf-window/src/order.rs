use pdf_edit::Stacking;

use crate::window_state::{Pointing, Window};

impl Window {
    pub(crate) fn chosen_for_order(&self) -> Option<(usize, Vec<String>)> {
        let page = self
            .pointing
            .page()
            .or_else(|| (self.chosen.count() > 0).then_some(self.chosen.page))?;
        let overlay = self.overlay(page)?;
        let mut anchors = Vec::new();
        if self.chosen.count() > 0 && self.chosen.page == page {
            for block in &self.chosen.blocks {
                anchors.extend(overlay.blocks.get(*block)?.anchors.iter().cloned());
            }
            for object in &self.chosen.objects {
                anchors.push(overlay.objects.get(*object)?.anchor.clone());
            }
        } else {
            match self.pointing {
                Pointing::Block { block, .. } | Pointing::Text { block, .. } => {
                    anchors.extend(overlay.blocks.get(block)?.anchors.iter().cloned());
                }
                Pointing::Object { object, .. } => {
                    anchors.push(overlay.objects.get(object)?.anchor.clone());
                }
                Pointing::Nothing => return None,
            }
        }
        (!anchors.is_empty()).then_some((page, anchors))
    }

    pub(crate) fn can_order(&self) -> bool {
        self.chosen_for_order().is_some()
    }

    pub(crate) fn put_in_order(&mut self, order: Stacking) {
        if self.editor.is_busy() {
            return;
        }
        let Some((page, anchors)) = self.chosen_for_order() else {
            return;
        };
        let job = self.editor.begin_reorder_objects(page, &anchors, order);
        self.send(job);
    }
}
