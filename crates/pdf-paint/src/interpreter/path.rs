use crate::color::Color;
use crate::error::{InterpretError, InterpretErrorKind};
use crate::geometry::{FillRule, PathSegment, Point};
use crate::graph::{PaintAtom, PaintAtomKind, PaintId, PathPaint};
use crate::interpreter::Interpreter;
use crate::state::ClipPath;
use pdf_content::Operation;

impl Interpreter {
    pub(super) fn move_to(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        let [x, y] = self.numbers::<2>(operation)?;
        let point = Point { x, y };
        self.push_segment(
            operation,
            PathSegment::MoveTo {
                point,
                provenance: operation.span(),
            },
        )?;
        self.current_point = Some(point);
        self.subpath_start = Some(point);
        Ok(())
    }

    pub(super) fn line_to(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        let [x, y] = self.numbers::<2>(operation)?;
        self.require_current_point(operation)?;
        let point = Point { x, y };
        self.push_segment(
            operation,
            PathSegment::LineTo {
                point,
                provenance: operation.span(),
            },
        )?;
        self.current_point = Some(point);
        Ok(())
    }

    pub(super) fn curve_to(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        let values = self.numbers::<6>(operation)?;
        self.require_current_point(operation)?;
        self.push_curve(
            operation,
            Point {
                x: values[0],
                y: values[1],
            },
            Point {
                x: values[2],
                y: values[3],
            },
            Point {
                x: values[4],
                y: values[5],
            },
        )
    }

    pub(super) fn curve_to_v(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        let [x2, y2, x3, y3] = self.numbers::<4>(operation)?;
        let control_1 = self.require_current_point(operation)?;
        self.push_curve(
            operation,
            control_1,
            Point { x: x2, y: y2 },
            Point { x: x3, y: y3 },
        )
    }

    pub(super) fn curve_to_y(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        let [x1, y1, x3, y3] = self.numbers::<4>(operation)?;
        self.require_current_point(operation)?;
        let end = Point { x: x3, y: y3 };
        self.push_curve(operation, Point { x: x1, y: y1 }, end, end)
    }

    pub(super) fn push_curve(
        &mut self,
        operation: &Operation,
        control_1: Point,
        control_2: Point,
        end: Point,
    ) -> Result<(), InterpretError> {
        self.push_segment(
            operation,
            PathSegment::CubicTo {
                control_1,
                control_2,
                end,
                provenance: operation.span(),
            },
        )?;
        self.current_point = Some(end);
        Ok(())
    }

    pub(super) fn close_path(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        Self::expect_count(operation, 0)?;
        let start = self.subpath_start.ok_or_else(|| {
            InterpretError::at(operation, InterpretErrorKind::CurrentPointMissing)
        })?;
        self.push_segment(
            operation,
            PathSegment::ClosePath {
                provenance: operation.span(),
            },
        )?;
        self.current_point = Some(start);
        Ok(())
    }

    pub(super) fn rectangle(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        let [x, y, width, height] = self.numbers::<4>(operation)?;
        let origin = Point { x, y };
        self.push_segment(
            operation,
            PathSegment::Rectangle {
                origin,
                width,
                height,
                provenance: operation.span(),
            },
        )?;
        self.current_point = Some(origin);
        self.subpath_start = Some(origin);
        Ok(())
    }

    pub(super) fn mark_clip(
        &mut self,
        operation: &Operation,
        rule: FillRule,
    ) -> Result<(), InterpretError> {
        Self::expect_count(operation, 0)?;
        self.pending_clip = Some((rule, operation.span()));
        Ok(())
    }

    pub(super) fn finish_path(
        &mut self,
        operation: &Operation,
        stroke: bool,
        fill: Option<FillRule>,
        close: bool,
    ) -> Result<(), InterpretError> {
        Self::expect_count(operation, 0)?;
        if (stroke && matches!(&self.state.stroke_color.value, Color::PatternUnspecified))
            || (fill.is_some() && matches!(&self.state.fill_color.value, Color::PatternUnspecified))
        {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::PatternColorMissing,
            ));
        }
        if close && self.subpath_start.is_some() && !self.path.segments.is_empty() {
            self.push_segment(
                operation,
                PathSegment::ClosePath {
                    provenance: operation.span(),
                },
            )?;
        }

        let path = std::mem::take(&mut self.path);
        if !path.segments.is_empty() && (stroke || fill.is_some()) {
            if self.paint_atoms >= self.limits.max_paint_atoms {
                return Err(InterpretError::at(
                    operation,
                    InterpretErrorKind::PaintAtomLimit,
                ));
            }
            let ordinal = self.graph.atoms.len();
            self.emit(PaintAtom {
                id: PaintId {
                    page: self.page,
                    stream: self.stream,
                    operator_span: operation.operator_span(),
                    invocation_path: self.invocation_path.clone(),
                    pattern_path: self.pattern_path.clone(),
                    ordinal,
                },
                kind: PaintAtomKind::Path(PathPaint {
                    path: path.clone(),
                    stroke,
                    fill,
                    state: self.state.clone(),
                }),
                marks: self.marks.clone(),
            });
            self.paint_atoms += 1;
        }

        if let Some((rule, provenance)) = self.pending_clip.take()
            && !path.segments.is_empty()
        {
            if self.state.clip_paths.len() >= self.limits.max_clip_paths {
                return Err(InterpretError::at(
                    operation,
                    InterpretErrorKind::ClipPathLimit,
                ));
            }
            self.state.clip_paths.push(ClipPath {
                path,
                rule,
                ctm: self.state.ctm.clone(),
                provenance,
            });
        }
        self.current_point = None;
        self.subpath_start = None;
        Ok(())
    }

    pub(super) fn abandon_path(&mut self) {
        self.path.segments.clear();
        self.pending_clip = None;
        self.current_point = None;
        self.subpath_start = None;
    }

    pub(super) fn push_segment(
        &mut self,
        operation: &Operation,
        segment: PathSegment,
    ) -> Result<(), InterpretError> {
        if self.path.segments.len() >= self.limits.max_path_segments {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::PathSegmentLimit,
            ));
        }
        self.path.segments.push(segment);
        Ok(())
    }

    pub(super) fn require_current_point(
        &self,
        operation: &Operation,
    ) -> Result<Point, InterpretError> {
        self.current_point
            .ok_or_else(|| InterpretError::at(operation, InterpretErrorKind::CurrentPointMissing))
    }
}
