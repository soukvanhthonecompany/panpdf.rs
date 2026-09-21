use crate::annotation::{appearance_placement, upright_rotation};
use crate::geometry::{Matrix, Point};

fn lands(matrix: Matrix, x: f64, y: f64) -> (f64, f64) {
    let point = matrix.transform(Point { x, y });
    (point.x, point.y)
}

fn close(actual: (f64, f64), expected: (f64, f64)) -> bool {
    (actual.0 - expected.0).abs() < 1e-9 && (actual.1 - expected.1).abs() < 1e-9
}

#[test]
fn a_box_the_size_of_the_rectangle_is_only_moved() {
    let placement = appearance_placement(
        [8.0, 8.0, 24.0, 24.0],
        [0.0, 0.0, 16.0, 16.0],
        Matrix::IDENTITY,
    )
    .expect("both have area");
    assert!(close(lands(placement, 0.0, 0.0), (8.0, 8.0)));
    assert!(close(lands(placement, 16.0, 16.0), (24.0, 24.0)));
}

#[test]
fn a_larger_box_is_scaled_into_the_rectangle() {
    let placement = appearance_placement(
        [8.0, 8.0, 24.0, 24.0],
        [0.0, 0.0, 32.0, 32.0],
        Matrix::IDENTITY,
    )
    .expect("both have area");
    assert!(close(lands(placement, 32.0, 16.0), (24.0, 16.0)));
}

#[test]
fn a_box_away_from_the_origin_is_brought_to_the_rectangle() {
    let placement = appearance_placement(
        [8.0, 8.0, 24.0, 24.0],
        [32.0, 32.0, 48.0, 48.0],
        Matrix::IDENTITY,
    )
    .expect("both have area");
    assert!(close(lands(placement, 32.0, 32.0), (8.0, 8.0)));
    assert!(close(lands(placement, 40.0, 48.0), (16.0, 24.0)));
}

#[test]
fn the_form_matrix_turns_the_box_before_it_is_fitted() {
    let quarter = Matrix {
        a: 0.0,
        b: 1.0,
        c: -1.0,
        d: 0.0,
        e: 0.0,
        f: 0.0,
    };
    let placement = appearance_placement([8.0, 8.0, 40.0, 24.0], [0.0, 0.0, 32.0, 16.0], quarter)
        .expect("both have area");
    assert!(close(lands(placement, 0.0, 0.0), (40.0, 8.0)));
    assert!(close(lands(placement, 16.0, 0.0), (40.0, 16.0)));
    assert!(close(lands(placement, 0.0, 16.0), (8.0, 8.0)));
    assert!(close(lands(placement, 16.0, 16.0), (8.0, 16.0)));
}

#[test]
fn nothing_is_placed_when_either_rectangle_has_no_area() {
    let identity = Matrix::IDENTITY;
    assert!(appearance_placement([8.0, 8.0, 24.0, 24.0], [0.0, 0.0, 0.0, 0.0], identity).is_none());
    assert!(appearance_placement([8.0, 8.0, 8.0, 8.0], [0.0, 0.0, 16.0, 16.0], identity).is_none());
}

#[test]
fn the_comparison_notices_a_wrong_placement() {
    let wrong = Matrix {
        e: 8.0,
        f: 8.0,
        ..Matrix::IDENTITY
    };
    assert!(!close(lands(wrong, 40.0, 48.0), (16.0, 24.0)));
}

#[test]
fn a_no_rotate_annotation_turns_back_about_its_upper_left_corner() {
    let rect = [8.0, 8.0, 40.0, 24.0];
    let turn = upright_rotation(rect, 90);
    assert!(close(lands(turn, 8.0, 24.0), (8.0, 24.0)));
    assert!(close(lands(turn, 40.0, 24.0), (8.0, 56.0)));
    assert!(close(lands(turn, 8.0, 8.0), (24.0, 24.0)));
    assert_eq!(upright_rotation(rect, 0), Matrix::IDENTITY);
}
