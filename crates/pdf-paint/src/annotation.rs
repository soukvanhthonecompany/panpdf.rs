use std::sync::Arc;

use pdf_bytes::SourceSpan;
use pdf_content::{
    AnnotationFlags, Annotations, Appearance, FontProvider, Medium, PageContentErrorKind,
    PageResources,
};
use pdf_syntax::Reference;

use crate::error::InterpretError;
use crate::form::{form_bbox_if_present, form_matrix};
use crate::geometry::{Matrix, Point};
use crate::graph::{InterpretRepair, PaintGraph, RepairKind};
use crate::interpreter::{AppearanceRun, run_appearance};
use crate::state::PaintLimits;

const GENERATED_BY_THE_VIEWER: [&[u8]; 9] = [
    b"Text",
    b"FreeText",
    b"Square",
    b"Circle",
    b"Highlight",
    b"Underline",
    b"Squiggly",
    b"StrikeOut",
    b"Ink",
];

#[derive(Clone, Debug, PartialEq)]
pub enum AnnotationOutcome {
    Drawn,
    DrawnFromStaleFieldAppearance,
    Hidden,
    Popup,
    WidgetNotAField,
    StateMissing,
    EmptyPlacement,
    AppearanceNotGenerated,
    NoAppearance,
    Unreadable(PageContentErrorKind),
    FormUnreadable,
    Failed(InterpretError),
}

impl AnnotationOutcome {
    #[must_use]
    pub const fn is_faithful(&self) -> bool {
        match self {
            Self::Drawn
            | Self::Hidden
            | Self::Popup
            | Self::WidgetNotAField
            | Self::StateMissing
            | Self::EmptyPlacement
            | Self::NoAppearance
            | Self::Unreadable(PageContentErrorKind::InvalidAnnotationRect) => true,
            Self::DrawnFromStaleFieldAppearance
            | Self::AppearanceNotGenerated
            | Self::Unreadable(_)
            | Self::FormUnreadable
            | Self::Failed(_) => false,
        }
    }
}

impl std::fmt::Display for AnnotationOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Drawn => formatter.write_str("drawn"),
            Self::DrawnFromStaleFieldAppearance => formatter
                .write_str("drawn from the stored appearance of a form that asks for new ones"),
            Self::Hidden => formatter.write_str("hidden by its flags"),
            Self::Popup => formatter.write_str("a popup, which is not drawn"),
            Self::WidgetNotAField => formatter.write_str("a widget that is not a form field"),
            Self::StateMissing => formatter.write_str("appearance state not present"),
            Self::EmptyPlacement => formatter.write_str("appearance encloses no area"),
            Self::AppearanceNotGenerated => {
                formatter.write_str("no appearance, and generating one is not implemented")
            }
            Self::NoAppearance => formatter.write_str("no appearance"),
            Self::Unreadable(kind) => write!(formatter, "unreadable: {kind}"),
            Self::FormUnreadable => formatter.write_str("the form's field tree is unreadable"),
            Self::Failed(error) => write!(formatter, "appearance failed: {}", error.kind()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AnnotationPaint {
    pub reference: Option<Reference>,
    pub subtype: Option<Vec<u8>>,
    pub outcome: AnnotationOutcome,
    pub graph: PaintGraph,
}

#[must_use]
pub fn interpret_annotations(
    annotations: &Annotations,
    page: Reference,
    rotate: u16,
    resources: &PageResources,
    limits: PaintLimits,
    fonts: Option<&Arc<dyn FontProvider>>,
) -> Vec<AnnotationPaint> {
    let mut painted = Vec::with_capacity(annotations.entries.len() + annotations.unreadable.len());
    for annotation in &annotations.entries {
        let medium = resources.optional_content().medium();
        let (outcome, graph) = match chosen_appearance(annotation, annotations, medium) {
            Err(outcome) => (outcome, PaintGraph::default()),
            Ok((form, span)) => {
                let flags = annotation.flags;
                draw(&Placing {
                    form,
                    span,
                    rect: annotation.rect,
                    rect_span: annotation.rect_span,
                    upright: flags.has(AnnotationFlags::NO_ROTATE).then_some(rotate),
                    page,
                    resources,
                    limits,
                    fonts: fonts.cloned(),
                    stale: annotations.need_appearances && annotation.field,
                })
            }
        };
        painted.push(AnnotationPaint {
            reference: annotation.reference,
            subtype: annotation.subtype.clone(),
            outcome,
            graph,
        });
    }
    painted.extend(
        annotations
            .unreadable
            .iter()
            .map(|unreadable| AnnotationPaint {
                reference: unreadable.reference,
                subtype: None,
                outcome: AnnotationOutcome::Unreadable(unreadable.error.kind()),
                graph: PaintGraph::default(),
            }),
    );
    painted
}

fn chosen_appearance<'a>(
    annotation: &'a pdf_content::Annotation,
    annotations: &Annotations,
    medium: Medium,
) -> Result<(&'a pdf_content::FormXObject, SourceSpan), AnnotationOutcome> {
    let flags = annotation.flags;
    let hidden = flags.has(AnnotationFlags::HIDDEN)
        || match medium {
            Medium::Screen => flags.has(AnnotationFlags::NO_VIEW),
            Medium::Print => !flags.has(AnnotationFlags::PRINT),
        };
    if hidden {
        return Err(AnnotationOutcome::Hidden);
    }
    if annotation.is(b"Popup") {
        return Err(AnnotationOutcome::Popup);
    }
    if annotation.is(b"Widget") && !annotation.field {
        return Err(if annotations.form_unreadable.is_some() {
            AnnotationOutcome::FormUnreadable
        } else {
            AnnotationOutcome::WidgetNotAField
        });
    }
    match &annotation.appearance {
        Appearance::Stream { form, span } => Ok((form, *span)),
        Appearance::Absent => Err(
            if annotation
                .subtype
                .as_deref()
                .is_some_and(|subtype| GENERATED_BY_THE_VIEWER.contains(&subtype))
            {
                AnnotationOutcome::AppearanceNotGenerated
            } else {
                AnnotationOutcome::NoAppearance
            },
        ),
        Appearance::MissingState { .. } => Err(AnnotationOutcome::StateMissing),
        Appearance::Unreadable(error) => Err(AnnotationOutcome::Unreadable(error.kind())),
    }
}

struct Placing<'a> {
    form: &'a pdf_content::FormXObject,
    span: SourceSpan,
    rect: [f64; 4],
    rect_span: SourceSpan,
    upright: Option<u16>,
    page: Reference,
    resources: &'a PageResources,
    limits: PaintLimits,
    fonts: Option<Arc<dyn FontProvider>>,
    stale: bool,
}

fn draw(placing: &Placing<'_>) -> (AnnotationOutcome, PaintGraph) {
    let failed = |error| (AnnotationOutcome::Failed(error), PaintGraph::default());
    let (bbox, bbox_span, bbox_assumed) = match form_bbox_if_present(placing.form, placing.span) {
        Ok(Some((bbox, span))) => (bbox, span, false),
        Ok(None) => (
            [
                0.0,
                0.0,
                placing.rect[2] - placing.rect[0],
                placing.rect[3] - placing.rect[1],
            ],
            placing.rect_span,
            true,
        ),
        Err(error) => return failed(error),
    };
    let (matrix, matrix_span) = match form_matrix(placing.form, placing.span) {
        Ok(found) => found,
        Err(error) => return failed(error),
    };
    let Some(mut placement) = appearance_placement(placing.rect, bbox, matrix) else {
        return (AnnotationOutcome::EmptyPlacement, PaintGraph::default());
    };
    if let Some(rotate) = placing.upright {
        placement = upright_rotation(placing.rect, rotate).multiply(placement);
    }
    let mut provenance = vec![placing.rect_span, placing.span];
    provenance.extend(matrix_span);
    let run = run_appearance(AppearanceRun {
        form: placing.form,
        page: placing.page,
        placement,
        provenance,
        bbox,
        bbox_span,
        resources: placing.resources,
        limits: placing.limits,
        fonts: placing.fonts.clone(),
    });
    match run {
        Ok(mut graph) => {
            if bbox_assumed {
                graph.repairs.push(InterpretRepair {
                    kind: RepairKind::AppearanceWithoutBBox,
                    operator_span: placing.span,
                });
            }
            let outcome = if placing.stale {
                AnnotationOutcome::DrawnFromStaleFieldAppearance
            } else {
                AnnotationOutcome::Drawn
            };
            (outcome, graph)
        }
        Err(error) => failed(error),
    }
}

#[must_use]
pub fn appearance_placement(rect: [f64; 4], bbox: [f64; 4], matrix: Matrix) -> Option<Matrix> {
    let corners = [
        Point {
            x: bbox[0],
            y: bbox[1],
        },
        Point {
            x: bbox[2],
            y: bbox[1],
        },
        Point {
            x: bbox[0],
            y: bbox[3],
        },
        Point {
            x: bbox[2],
            y: bbox[3],
        },
    ]
    .map(|corner| matrix.transform(corner));
    let low_x = corners
        .iter()
        .map(|point| point.x)
        .fold(f64::INFINITY, f64::min);
    let low_y = corners
        .iter()
        .map(|point| point.y)
        .fold(f64::INFINITY, f64::min);
    let high_x = corners
        .iter()
        .map(|point| point.x)
        .fold(f64::NEG_INFINITY, f64::max);
    let high_y = corners
        .iter()
        .map(|point| point.y)
        .fold(f64::NEG_INFINITY, f64::max);
    let (box_width, box_height) = (high_x - low_x, high_y - low_y);
    let (rect_width, rect_height) = (rect[2] - rect[0], rect[3] - rect[1]);
    let has_area = |width: f64, height: f64| width > 0.0 && height > 0.0;
    if !has_area(box_width, box_height) || !has_area(rect_width, rect_height) {
        return None;
    }
    let scale_x = rect_width / box_width;
    let scale_y = rect_height / box_height;
    let onto_rect = Matrix {
        a: scale_x,
        b: 0.0,
        c: 0.0,
        d: scale_y,
        e: rect[0] - low_x * scale_x,
        f: rect[1] - low_y * scale_y,
    };
    let placement = onto_rect.multiply(matrix);
    [
        placement.a,
        placement.b,
        placement.c,
        placement.d,
        placement.e,
        placement.f,
    ]
    .iter()
    .all(|value| value.is_finite())
    .then_some(placement)
}

#[must_use]
pub fn upright_rotation(rect: [f64; 4], rotate: u16) -> Matrix {
    let (cos, sin) = match rotate % 360 {
        90 => (0.0, 1.0),
        180 => (-1.0, 0.0),
        270 => (0.0, -1.0),
        _ => return Matrix::IDENTITY,
    };
    let (pivot_x, pivot_y) = (rect[0], rect[3]);
    Matrix {
        a: cos,
        b: sin,
        c: -sin,
        d: cos,
        e: pivot_x - (cos * pivot_x - sin * pivot_y),
        f: pivot_y - (sin * pivot_x + cos * pivot_y),
    }
}
