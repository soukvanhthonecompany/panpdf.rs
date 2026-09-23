use eframe::egui;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DropZone {
    Pages,
    Ai,
    Canvas,
}

pub(crate) fn zone_at(pointer: egui::Pos2, panels: &[(DropZone, Option<egui::Rect>)]) -> DropZone {
    panels
        .iter()
        .find(|(_, rect)| rect.is_some_and(|rect| rect.contains(pointer)))
        .map_or(DropZone::Canvas, |(zone, _)| *zone)
}

#[cfg(test)]
mod tests {
    use eframe::egui;

    use super::{DropZone, zone_at};

    fn rect(min: (f32, f32), max: (f32, f32)) -> egui::Rect {
        egui::Rect::from_min_max(egui::pos2(min.0, min.1), egui::pos2(max.0, max.1))
    }

    #[test]
    fn three_positions_choose_three_zones_when_both_panels_are_open() {
        let pages = Some(rect((0.0, 0.0), (200.0, 800.0)));
        let ai = Some(rect((500.0, 0.0), (700.0, 800.0)));
        let panels = [(DropZone::Pages, pages), (DropZone::Ai, ai)];
        assert_eq!(zone_at(egui::pos2(50.0, 50.0), &panels), DropZone::Pages);
        assert_eq!(zone_at(egui::pos2(550.0, 50.0), &panels), DropZone::Ai);
        assert_eq!(zone_at(egui::pos2(300.0, 50.0), &panels), DropZone::Canvas);
    }

    #[test]
    fn the_ai_panel_closed_never_chooses_the_chat() {
        let pages = Some(rect((0.0, 0.0), (200.0, 800.0)));
        let panels = [(DropZone::Pages, pages), (DropZone::Ai, None)];
        assert_eq!(zone_at(egui::pos2(550.0, 50.0), &panels), DropZone::Canvas);
    }

    #[test]
    fn the_page_panel_folded_never_chooses_a_gap() {
        let ai = Some(rect((500.0, 0.0), (700.0, 800.0)));
        let panels = [(DropZone::Pages, None), (DropZone::Ai, ai)];
        assert_eq!(zone_at(egui::pos2(50.0, 50.0), &panels), DropZone::Canvas);
    }

    #[test]
    fn both_panels_closed_every_position_is_the_canvas() {
        let panels = [(DropZone::Pages, None), (DropZone::Ai, None)];
        for at in [
            egui::pos2(50.0, 50.0),
            egui::pos2(550.0, 50.0),
            egui::pos2(300.0, 50.0),
            egui::pos2(-40.0, 900.0),
        ] {
            assert_eq!(zone_at(at, &panels), DropZone::Canvas, "{at:?}");
        }
    }

    #[test]
    fn a_third_zone_needs_no_change_to_the_function() {
        let pages = Some(rect((0.0, 0.0), (100.0, 100.0)));
        let ai = Some(rect((200.0, 0.0), (300.0, 100.0)));
        let third = Some(rect((400.0, 0.0), (500.0, 100.0)));
        let panels = [
            (DropZone::Pages, pages),
            (DropZone::Ai, ai),
            (DropZone::Pages, third),
        ];
        assert_eq!(zone_at(egui::pos2(450.0, 50.0), &panels), DropZone::Pages);
    }
}
