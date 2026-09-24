use pdf_bytes::SourceSpan;

pub trait MulAdd: Sized {
    #[must_use]
    fn madd(self, a: Self, b: Self) -> Self;
}

impl MulAdd for f64 {
    #[inline]
    fn madd(self, a: Self, b: Self) -> Self {
        #[cfg(target_arch = "wasm32")]
        {
            self * a + b
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.mul_add(a, b)
        }
    }
}

impl MulAdd for f32 {
    #[inline]
    fn madd(self, a: Self, b: Self) -> Self {
        #[cfg(target_arch = "wasm32")]
        {
            self * a + b
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.mul_add(a, b)
        }
    }
}

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
            c: shear.madd(cos, -(self.across * sin)),
            d: shear.madd(sin, self.across * cos),
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
            a: self.a.madd(right.a, self.c * right.b),
            b: self.b.madd(right.a, self.d * right.b),
            c: self.a.madd(right.c, self.c * right.d),
            d: self.b.madd(right.c, self.d * right.d),
            e: self.a.madd(right.e, self.c.madd(right.f, self.e)),
            f: self.b.madd(right.e, self.d.madd(right.f, self.f)),
        }
    }

    #[must_use]
    pub fn shape(self) -> Option<Shape> {
        let along = self.a.hypot(self.b);
        let determinant = self.a.madd(self.d, -(self.b * self.c));
        if !along.is_finite() || along == 0.0 || !determinant.is_finite() || determinant == 0.0 {
            return None;
        }
        let across = determinant / along;
        let shear = self.a.madd(self.c, self.b * self.d) / determinant;
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
        let determinant = self.a.madd(self.d, -(self.b * self.c));
        if !determinant.is_finite() || determinant == 0.0 {
            return None;
        }
        let inverted = Self {
            a: self.d / determinant,
            b: -self.b / determinant,
            c: -self.c / determinant,
            d: self.a / determinant,
            e: self.c.madd(self.f, -(self.d * self.e)) / determinant,
            f: self.b.madd(self.e, -(self.a * self.f)) / determinant,
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
            x: self.a.madd(point.x, self.c.madd(point.y, self.e)),
            y: self.b.madd(point.x, self.d.madd(point.y, self.f)),
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
