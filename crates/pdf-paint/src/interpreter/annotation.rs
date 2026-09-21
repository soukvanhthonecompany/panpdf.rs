use std::sync::Arc;

use pdf_bytes::SourceSpan;
use pdf_content::{
    ContentLimits, FontProvider, FormXObject, PageResources, parse_operations_strict,
};
use pdf_syntax::Reference;

use super::{Interpreter, PaintStream};
use crate::error::{InterpretError, InterpretErrorKind};
use crate::geometry::{FillRule, Matrix, Path, PathSegment, Point};
use crate::graph::PaintGraph;
use crate::state::{ClipPath, PaintLimits};

pub(crate) struct AppearanceRun<'a> {
    pub(crate) form: &'a FormXObject,
    pub(crate) page: Reference,
    pub(crate) placement: Matrix,
    pub(crate) provenance: Vec<SourceSpan>,
    pub(crate) bbox: [f64; 4],
    pub(crate) bbox_span: SourceSpan,
    pub(crate) resources: &'a PageResources,
    pub(crate) limits: PaintLimits,
    pub(crate) fonts: Option<Arc<dyn FontProvider>>,
}

pub(crate) fn run_appearance(run: AppearanceRun<'_>) -> Result<PaintGraph, InterpretError> {
    let operations = parse_operations_strict(&run.form.bytes, ContentLimits::default())
        .map_err(|_| InterpretError::at_span(run.bbox_span, InterpretErrorKind::FormContent))?;
    let resources = run.form.resources.as_ref().unwrap_or(run.resources);
    let mut interpreter = Interpreter::new(
        &run.form.bytes,
        run.page,
        run.form.reference,
        &[],
        Some(resources),
        run.limits,
        run.fonts,
    );
    interpreter.tolerate_unsupported = true;
    interpreter.state.ctm.value = run.placement;
    for span in run.provenance {
        interpreter.state.ctm.provenance.push(span);
    }
    interpreter.pattern_base = run.placement;
    let [x0, y0, x1, y1] = run.bbox;
    interpreter.state.clip_paths.push(ClipPath {
        path: Path {
            segments: vec![PathSegment::Rectangle {
                origin: Point { x: x0, y: y0 },
                width: x1 - x0,
                height: y1 - y0,
                provenance: run.bbox_span,
            }],
        },
        rule: FillRule::Nonzero,
        ctm: interpreter.state.ctm.clone(),
        provenance: run.bbox_span,
    });
    interpreter.run(&[PaintStream {
        source: &run.form.bytes,
        reference: run.form.reference,
        operations: &operations,
    }])
}
