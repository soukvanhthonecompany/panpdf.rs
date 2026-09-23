pub const LEAST_ROWS: usize = 2;

pub const LEAST_WIDTH: f32 = 70.0;

const MOST_ROWS_EVER: f32 = 512.0;

#[must_use]
pub const fn rows_shown(wrapped: usize, most: usize) -> usize {
    let ceiling = if most < LEAST_ROWS { LEAST_ROWS } else { most };
    if wrapped < LEAST_ROWS {
        LEAST_ROWS
    } else if wrapped > ceiling {
        ceiling
    } else {
        wrapped
    }
}

#[must_use]
pub fn most_rows(panel_height: f32, row_height: f32, chrome: f32) -> usize {
    if !row_height.is_finite() || row_height <= 0.0 {
        return LEAST_ROWS;
    }
    let room = panel_height / 2.0 - chrome;
    if !room.is_finite() || room <= 0.0 {
        return LEAST_ROWS;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "floored and clamped to 0..=512 on the line above, so the \
                  cast is exact and not negative"
    )]
    let fits = (room / row_height).floor().clamp(0.0, MOST_ROWS_EVER) as usize;
    if fits < LEAST_ROWS { LEAST_ROWS } else { fits }
}

#[must_use]
pub fn composer_height(rows: usize, row_height: f32, chrome: f32) -> f32 {
    let rows = if rows < LEAST_ROWS { LEAST_ROWS } else { rows };
    #[expect(
        clippy::cast_precision_loss,
        reason = "a row count is small; no panel has 2^24 rows in it"
    )]
    let rows = rows as f32;
    rows * row_height + chrome
}

#[must_use]
pub fn conversation_room(available: f32, composer: f32, least: f32) -> f32 {
    (available - composer).max(least)
}

#[must_use]
pub fn split_row(available: f32, fixed: f32, gap: f32, parts: usize) -> f32 {
    if parts == 0 {
        return LEAST_WIDTH;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "a control count is small; no row has 2^24 controls in it"
    )]
    let parts = parts as f32;
    ((available - fixed - gap * parts) / parts).max(LEAST_WIDTH)
}

#[must_use]
pub fn fits_on_one_row(available: f32, widths: &[f32], gap: f32) -> bool {
    let controls: f32 = widths.iter().sum();
    #[expect(
        clippy::cast_precision_loss,
        reason = "a control count is small; no row has 2^24 controls in it"
    )]
    let gaps = widths.len().saturating_sub(1) as f32 * gap;
    controls + gaps <= available
}

#[cfg(test)]
mod tests {
    use super::{
        LEAST_ROWS, LEAST_WIDTH, composer_height, conversation_room, fits_on_one_row, most_rows,
        rows_shown, split_row,
    };

    #[test]
    fn a_row_fits_only_with_its_gaps() {
        let widths = [30.0, 110.0, 150.0, 50.0];
        assert!(fits_on_one_row(388.0, &widths, 16.0));
        assert!(!fits_on_one_row(387.0, &widths, 16.0));
        assert!(!fits_on_one_row(340.0, &widths, 16.0));
        assert!(fits_on_one_row(0.0, &[], 16.0));
    }

    #[test]
    fn the_box_shows_what_was_typed_within_its_bounds() {
        assert_eq!(rows_shown(0, 8), 2);
        assert_eq!(rows_shown(5, 8), 5);
        assert_eq!(rows_shown(40, 8), 8);
        assert_eq!(rows_shown(40, 1), LEAST_ROWS);
    }

    #[test]
    fn the_ceiling_is_half_the_panel() {
        assert_eq!(most_rows(600.0, 18.0, 44.0), 14);
        let whole_panel = ((600.0 - 44.0) / 18.0_f32).floor();
        assert!(
            (whole_panel - 30.0).abs() < 0.5,
            "the control formula answers 30, not {whole_panel}"
        );
        assert_ne!(most_rows(600.0, 18.0, 44.0), 30);
        assert_eq!(most_rows(40.0, 18.0, 44.0), LEAST_ROWS);
        assert_eq!(most_rows(600.0, 0.0, 44.0), LEAST_ROWS);
    }

    #[expect(
        clippy::float_cmp,
        reason = "every number here is exact in binary, and the point of the \
                  test is the exact height"
    )]
    #[test]
    fn the_box_is_its_rows_and_its_furniture() {
        assert_eq!(composer_height(2, 18.0, 44.0), 80.0);
        assert_eq!(composer_height(14, 18.0, 44.0), 296.0);
        assert_eq!(composer_height(2, 18.0, 0.0), 36.0);
        assert_eq!(composer_height(0, 18.0, 44.0), 80.0);
    }

    #[expect(
        clippy::float_cmp,
        reason = "every number here is exact in binary, and the point of the \
                  test is the exact height"
    )]
    #[test]
    fn the_conversation_keeps_a_floor_under_it() {
        assert_eq!(conversation_room(300.0, 260.0, 80.0), 80.0);
        assert_eq!(conversation_room(600.0, 260.0, 80.0), 340.0);
    }

    #[expect(
        clippy::float_cmp,
        reason = "every number here is exact in binary, and the point of the \
                  test is the exact width"
    )]
    #[test]
    fn controls_sharing_a_row_are_equal() {
        assert_eq!(split_row(360.0, 30.0, 8.0, 2), 157.0);
        let forgot_a_gap = (360.0 - 30.0 - 8.0) / 2.0;
        assert_ne!(split_row(360.0, 30.0, 8.0, 2), forgot_a_gap);
        assert_eq!(split_row(360.0, 0.0, 8.0, 2), 172.0);
        assert_eq!(split_row(100.0, 30.0, 8.0, 2), LEAST_WIDTH);
        assert_eq!(split_row(360.0, 30.0, 8.0, 0), LEAST_WIDTH);
    }
}
