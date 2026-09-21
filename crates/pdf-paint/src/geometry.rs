use pdf_bytes::SourceSpan;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    pub along: f64,
    pub across: f64,
    pub turn: f64,
    pub slant: f64,
}

impl Shape {
    #[must_use]
    pub fn matrix(self) -> Matrix {
        let (sin, cos) = self.turn.sin_cos();
        let shear = self.across * self.slant.tan();
        Matrix {
            a: self.along * cos,
            b: self.along * sin,
            c: shear.mul_add(cos, -(self.across * sin)),
            d: shear.mul_add(sin, self.across * cos),
            e: 0.0,
            f: 0.0,
        }
    }
}

impl Matrix {
    pub const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    #[must_use]
    pub fn multiply(self, right: Self) -> Self {
        Self {
            a: self.a.mul_add(right.a, self.c * right.b),
            b: self.b.mul_add(right.a, self.d * right.b),
            c: self.a.mul_add(right.c, self.c * right.d),
            d: self.b.mul_add(right.c, self.d * right.d),
            e: self.a.mul_add(right.e, self.c.mul_add(right.f, self.e)),
            f: self.b.mul_add(right.e, self.d.mul_add(right.f, self.f)),
        }
    }

    #[must_use]
    pub fn shape(self) -> Option<Shape> {
        let along = self.a.hypot(self.b);
        let determinant = self.a.mul_add(self.d, -(self.b * self.c));
        if !along.is_finite() || along == 0.0 || !determinant.is_finite() || determinant == 0.0 {
            return None;
        }
        let across = determinant / along;
        let shear = self.a.mul_add(self.c, self.b * self.d) / determinant;
        let shape = Shape {
            along,
            across,
            turn: self.b.atan2(self.a),
            slant: shear.atan(),
        };
        [shape.along, shape.across, shape.turn, shape.slant]
            .iter()
            .all(|value| value.is_finite())
            .then_some(shape)
    }

    #[must_use]
    pub fn inverse(self) -> Option<Self> {
        let determinant = self.a.mul_add(self.d, -(self.b * self.c));
        if !determinant.is_finite() || determinant == 0.0 {
            return None;
        }
        let inverted = Self {
            a: self.d / determinant,
            b: -self.b / determinant,
            c: -self.c / determinant,
            d: self.a / determinant,
            e: self.c.mul_add(self.f, -(self.d * self.e)) / determinant,
            f: self.b.mul_add(self.e, -(self.a * self.f)) / determinant,
        };
        [
            inverted.a, inverted.b, inverted.c, inverted.d, inverted.e, inverted.f,
        ]
        .iter()
        .all(|value| value.is_finite())
        .then_some(inverted)
    }

    #[must_use]
    pub fn transform(self, point: Point) -> Point {
        Point {
            x: self.a.mul_add(point.x, self.c.mul_add(point.y, self.e)),
            y: self.b.mul_add(point.x, self.d.mul_add(point.y, self.f)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FillRule {
    Nonzero,
    EvenOdd,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PathSegment {
    MoveTo {
        point: Point,
        provenance: SourceSpan,
    },
    LineTo {
        point: Point,
        provenance: SourceSpan,
    },
    CubicTo {
        control_1: Point,
        control_2: Point,
        end: Point,
        provenance: SourceSpan,
    },
    ClosePath {
        provenance: SourceSpan,
    },
    Rectangle {
        origin: Point,
        width: f64,
        height: f64,
        provenance: SourceSpan,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Path {
    pub segments: Vec<PathSegment>,
}

pub(crate) fn normalize_rectangle(corners: [f64; 4]) -> [f64; 4] {
    [
        corners[0].min(corners[2]),
        corners[1].min(corners[3]),
        corners[0].max(corners[2]),
        corners[1].max(corners[3]),
    ]
}
