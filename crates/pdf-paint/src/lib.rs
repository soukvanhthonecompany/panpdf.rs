#![forbid(unsafe_code)]

pub mod ccitt;
pub mod jbig2;
pub mod postscript;
pub mod signature;

mod annotation;
mod color;
pub mod decipher_fonts;
mod error;
mod form;
mod function;
mod geometry;
mod graph;
mod image;
mod interpreter;
mod operand;
mod pattern;
pub mod picture;
mod provenance;
pub mod reading_order;
mod shading;
mod state;
mod text;

pub use annotation::{
    AnnotationOutcome, AnnotationPaint, appearance_placement, interpret_annotations,
    upright_rotation,
};
pub use color::{
    CalGraySpace, CalRgbSpace, Color, ColorSpace, Colorant, DeviceNSpace, IccAlternate,
    IccBasedSpace, IccProfileHeader, IndexedBase, IndexedLookupSource, IndexedSpace, LabSpace,
    SeparationSpace,
};
pub use error::{InterpretError, InterpretErrorKind};
pub use function::{
    ExponentialFunction, Function, PostScriptFunction, SampledFunction, StitchingFunction,
};
pub use geometry::{FillRule, Matrix, MulAdd, Path, PathSegment, Point, Shape};
pub use graph::{
    FormInvocation, GroupBackdrop, InterpretRepair, MarkedContent, MarkedProperties, ObjectScope,
    PaintAtom, PaintAtomKind, PaintGraph, PaintId, PathPaint, PatternInvocation, PositionedGlyph,
    RepairKind, ShadingPatternPaint, SoftMaskPaint, SubstitutedGlyph, TextShowElement,
    TextShowPaint, TilingPatternPaint, TransparencyGroupPaint, Type3Glyph,
};
pub use image::{DctColorTransform, DctParameterLocation, ImageMask, ImagePaint};
pub use interpreter::{
    PaintContext, PaintStream, interpret_operations, interpret_stream_sequence,
    interpret_stream_sequence_tolerating_unsupported, interpret_stream_sequence_with_fonts,
    interpret_stream_sequence_with_resources,
};
pub use provenance::{Derived, Provenance};
pub use shading::{ShadingGeometry, ShadingPaint};
pub use state::{
    AppliedExtGState, AppliedFont, BlendMode, ClipPath, DashPattern, GraphicsState, LineCap,
    LineJoin, PaintLimits, SoftMask, SoftMaskSubtype, SoftMaskTransfer, TextMatrices,
    TextRenderingMode, TextState,
};
pub use text::{code_advance, position_text, substitute_run};

pub use pdf_content::{
    Code, Confidence, GlyphPath, GlyphProgram, GlyphSegment, Meaning, ToUnicode, UnresolvedReason,
};
pub use signature::{colour_signature, glyph_placement_signature, paint_signature};

#[cfg(test)]
mod test_fixtures;

#[cfg(test)]
mod annotation_tests;
#[cfg(test)]
mod color_tests;
#[cfg(test)]
mod form_tests;
#[cfg(test)]
mod function_tests;
#[cfg(test)]
mod geometry_tests;
#[cfg(test)]
mod image_tests;
#[cfg(test)]
mod interpreter_tests;
#[cfg(test)]
mod pattern_tests;
#[cfg(test)]
mod shading_tests;
#[cfg(test)]
mod signature_tests;
#[cfg(test)]
mod substitute_tests;
#[cfg(test)]
mod text_tests;
