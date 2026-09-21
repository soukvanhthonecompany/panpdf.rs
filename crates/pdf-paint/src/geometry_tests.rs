use crate::geometry::{Matrix, Point};

#[test]
fn a_shape_says_what_a_matrix_does_and_builds_the_same_matrix_back() {
    use crate::Shape;
    for turn in [0.0_f64, 0.4, -1.2, 3.0] {
        for slant in [0.0_f64, 0.3, -0.45] {
            for (along, across) in [(1.0, 1.0), (12.0, 12.0), (3.0, 7.5), (0.25, 0.25)] {
                let wanted = Shape {
                    along,
                    across,
                    turn,
                    slant,
                };
                let read = wanted.matrix().shape().expect("it has an area");
                for (had, want) in [
                    (read.along, along),
                    (read.across, across),
                    (read.turn, turn),
                    (read.slant, slant),
                ] {
                    assert!((had - want).abs() < 1e-12, "{read:?} != {wanted:?}");
                }
            }
        }
    }
    let matrix = crate::Matrix {
        a: 2.0,
        b: -0.5,
        c: 0.75,
        d: 3.25,
        e: 40.0,
        f: -9.0,
    };
    let rebuilt = matrix.shape().expect("it has an area").matrix();
    for (had, want) in [
        (rebuilt.a, matrix.a),
        (rebuilt.b, matrix.b),
        (rebuilt.c, matrix.c),
        (rebuilt.d, matrix.d),
    ] {
        assert!((had - want).abs() < 1e-12, "{rebuilt:?}");
    }
    assert!(rebuilt.e == 0.0 && rebuilt.f == 0.0);
    let turned = crate::Matrix {
        a: 0.0,
        b: 12.0,
        c: -12.0,
        d: 0.0,
        ..crate::Matrix::IDENTITY
    };
    let read = turned.shape().expect("it has an area");
    assert!((read.along - 12.0).abs() < 1e-12 && (read.across - 12.0).abs() < 1e-12);
    assert!((read.turn - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
    assert!(read.slant.abs() < 1e-12);
    let leaning = crate::Shape {
        along: 10.0,
        across: 10.0,
        turn: 0.0,
        slant: 0.4,
    }
    .matrix();
    assert!(leaning.c.hypot(leaning.d) > 10.5, "the column is longer");
    assert!((leaning.shape().expect("area").across - 10.0).abs() < 1e-12);
    assert!(
        crate::Matrix {
            a: 1.0,
            d: 0.0,
            ..crate::Matrix::IDENTITY
        }
        .shape()
        .is_none()
    );
}

#[test]
fn an_inverse_undoes_the_matrix_and_a_collapsed_one_has_none() {
    let turned = Matrix {
        a: 2.0,
        b: 0.5,
        c: -0.25,
        d: 3.0,
        e: 17.0,
        f: -4.0,
    };
    let back = turned.inverse().expect("this matrix has area");
    let round_trip = turned.multiply(back);
    for (had, want) in [
        (round_trip.a, 1.0),
        (round_trip.b, 0.0),
        (round_trip.c, 0.0),
        (round_trip.d, 1.0),
        (round_trip.e, 0.0),
        (round_trip.f, 0.0),
    ] {
        assert!((had - want).abs() < 1e-12, "{had} is not {want}");
    }
    let point = Point { x: 5.0, y: -9.0 };
    let there_and_back = back.transform(turned.transform(point));
    assert!((there_and_back.x - point.x).abs() < 1e-12);
    assert!((there_and_back.y - point.y).abs() < 1e-12);

    assert_eq!(
        Matrix {
            a: 1.0,
            b: 2.0,
            c: 2.0,
            d: 4.0,
            e: 0.0,
            f: 0.0,
        }
        .inverse(),
        None
    );
}
