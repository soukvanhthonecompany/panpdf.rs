use std::fmt;

use pdf_bytes::SourceSpan;
use pdf_content::{Operation, PageContentErrorKind, ResourceFontError};

use crate::{ccitt, postscript};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InterpretError {
    operator: Option<SourceSpan>,
    kind: InterpretErrorKind,
}

impl InterpretErrorKind {
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub const fn is_skippable(self) -> bool {
        match self {
            Self::UnsupportedOperator
            | Self::OperandCount { .. }
            | Self::OperandType
            | Self::InvalidNumber
            | Self::InvalidValue
            | Self::CurrentPointMissing
            | Self::ResourceScopeMissing
            | Self::ResourceNotFound
            | Self::ResourceSourceFailure
            | Self::FontProgram(_)
            | Self::ImageResource(_)
            | Self::UnsupportedImageCodec(_)
            | Self::DctDecodeFailure
            | Self::CcittDecodeFailure(_)
            | Self::Jbig2DecodeFailure(_)
            | Self::JpxDecodeFailure(_)
            | Self::JpxMetadataMismatch
            | Self::DctMetadataMismatch
            | Self::UnsupportedDctPrecision
            | Self::InvalidImageCodecParameters
            | Self::UnsupportedImageCodecParameters
            | Self::InvalidImageEntry
            | Self::UnsupportedImageEntry
            | Self::ImageMissingEntry
            | Self::ImageSampleShortfall
            | Self::FormResource(_)
            | Self::FormContent
            | Self::FormMissingBBox
            | Self::InvalidFormEntry
            | Self::FormInvocationCycle
            | Self::UnsupportedXObject
            | Self::GroupResource(_)
            | Self::GroupMissingEntry
            | Self::InvalidGroupEntry
            | Self::UnsupportedGroupSubtype
            | Self::UnsupportedGroupColorSpace
            | Self::ExtGStateNotDictionary
            | Self::DuplicateExtGStateEntry
            | Self::InvalidExtGStateEntry
            | Self::UnsupportedExtGStateEntry
            | Self::UnsupportedBlendMode
            | Self::SoftMaskMissingEntry
            | Self::InvalidSoftMaskEntry
            | Self::UnsupportedSoftMaskEntry
            | Self::SoftMaskResource(_)
            | Self::UnsupportedSoftMaskTransfer
            | Self::ShadingNotDictionary
            | Self::ShadingMissingEntry
            | Self::InvalidShadingEntry
            | Self::UnsupportedShadingEntry
            | Self::UnsupportedShadingType
            | Self::UnsupportedShadingColorSpace
            | Self::FunctionMissingEntry
            | Self::InvalidFunction
            | Self::UnsupportedFunction
            | Self::FunctionResource(_)
            | Self::FunctionMissingData
            | Self::PostScriptFunction(_)
            | Self::UnsupportedFunctionEntry
            | Self::PatternResource(_)
            | Self::PatternContent
            | Self::PatternMissingEntry
            | Self::InvalidPatternEntry
            | Self::UncoloredPatternUnsupported
            | Self::PatternInvocationCycle
            | Self::PatternColorMissing
            | Self::IccProfileMissingEntry
            | Self::InvalidIccProfileEntry
            | Self::UnsupportedIccProfileEntry
            | Self::InvalidIccProfile
            | Self::UnsupportedIccAlternate
            | Self::InvalidSeparationColorSpace
            | Self::InvalidDeviceNColorSpace
            | Self::UnsupportedTintAlternate
            | Self::ColorSpaceResource(_)
            | Self::InvalidTintTransform
            | Self::InvalidIndexedColorSpace
            | Self::UnsupportedIndexedBase
            | Self::InvalidIndexedLookup
            | Self::IndexedLookupLength { .. }
            | Self::UnsupportedIndexedLookupEntry
            | Self::IndexedLookupResource(_)
            | Self::UnsupportedColorSpace
            | Self::ColorSpaceMissingEntry
            | Self::InvalidColorSpaceEntry
            | Self::IccProfileResource(_)
            | Self::FontNotDictionary
            | Self::TextFontMissing
            | Self::FontDecode(_)
            | Self::Type3Resource(_)
            | Self::Type3NotSimpleFont
            | Self::Type3ProcedureContent
            | Self::GlyphMetricOutsideProcedure
            | Self::InvalidTextString
            | Self::TextClippingUnsupported
            | Self::TextClipWithoutOutlines => true,

            Self::GraphicsStackUnderflow
            | Self::UnbalancedGraphicsState { .. }
            | Self::UnbalancedMarkedContent { .. }
            | Self::MarkedContentDepthLimit
            | Self::UnbalancedCompatibilitySection { .. }
            | Self::CompatibilitySectionMissing
            | Self::CompatibilitySectionLimit
            | Self::InvalidMarkedContentProperties
            | Self::UnsupportedOptionalContent
            | Self::NestedTextObject
            | Self::TextObjectMissing
            | Self::UnclosedTextObject
            | Self::SourceSpanFailure
            | Self::ImageLimit
            | Self::GraphicsStackLimit
            | Self::PathSegmentLimit
            | Self::PaintAtomLimit
            | Self::ClipPathLimit
            | Self::FunctionInputLimit
            | Self::FunctionDepthLimit
            | Self::FunctionCountLimit
            | Self::FormDepthLimit
            | Self::FormInvocationLimit
            | Self::IndexedLookupLimit
            | Self::PatternDepthLimit
            | Self::PatternInvocationLimit
            | Self::TextStringLimit
            | Self::ColorSpaceDepthLimit
            | Self::OperatorInsideTextObject => false,
        }
    }
}

impl InterpretError {
    pub(crate) const fn new(operator: Option<SourceSpan>, kind: InterpretErrorKind) -> Self {
        Self { operator, kind }
    }

    pub(crate) fn at(operation: &Operation, kind: InterpretErrorKind) -> Self {
        Self::new(Some(operation.operator_span()), kind)
    }

    pub(crate) const fn at_span(span: SourceSpan, kind: InterpretErrorKind) -> Self {
        Self::new(Some(span), kind)
    }

    #[must_use]
    pub const fn operator_span(self) -> Option<SourceSpan> {
        self.operator
    }

    #[must_use]
    pub const fn kind(self) -> InterpretErrorKind {
        self.kind
    }
}

impl fmt::Display for InterpretError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(span) = self.operator {
            write!(formatter, "{} at byte {}", self.kind, span.start())
        } else {
            self.kind.fmt(formatter)
        }
    }
}

impl std::error::Error for InterpretError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterpretErrorKind {
    UnsupportedOperator,
    OperandCount { expected: usize, actual: usize },
    OperandType,
    InvalidNumber,
    InvalidValue,
    CurrentPointMissing,
    GraphicsStackUnderflow,
    UnbalancedGraphicsState { depth: usize },
    UnbalancedMarkedContent { depth: usize },
    MarkedContentDepthLimit,
    UnbalancedCompatibilitySection { depth: usize },
    CompatibilitySectionMissing,
    CompatibilitySectionLimit,
    InvalidMarkedContentProperties,
    UnsupportedOptionalContent,
    FontProgram(PageContentErrorKind),
    ImageResource(PageContentErrorKind),
    FormResource(PageContentErrorKind),
    UnsupportedImageCodec(pdf_syntax::ImageCodec),
    DctDecodeFailure,
    CcittDecodeFailure(ccitt::CcittError),
    Jbig2DecodeFailure(crate::jbig2::Jbig2Error),
    JpxDecodeFailure(jpeg2000::ErrorKind),
    JpxMetadataMismatch,
    DctMetadataMismatch,
    UnsupportedDctPrecision,
    InvalidImageCodecParameters,
    UnsupportedImageCodecParameters,
    InvalidImageEntry,
    UnsupportedImageEntry,
    ImageMissingEntry,
    ImageSampleShortfall,
    ImageLimit,
    GraphicsStackLimit,
    PathSegmentLimit,
    PaintAtomLimit,
    ClipPathLimit,
    SourceSpanFailure,
    ResourceScopeMissing,
    ResourceNotFound,
    ExtGStateNotDictionary,
    ResourceSourceFailure,
    DuplicateExtGStateEntry,
    InvalidExtGStateEntry,
    UnsupportedExtGStateEntry,
    UnsupportedBlendMode,
    SoftMaskMissingEntry,
    InvalidSoftMaskEntry,
    UnsupportedSoftMaskEntry,
    SoftMaskResource(PageContentErrorKind),
    UnsupportedSoftMaskTransfer,
    UnsupportedXObject,
    GroupResource(PageContentErrorKind),
    GroupMissingEntry,
    InvalidGroupEntry,
    UnsupportedGroupSubtype,
    UnsupportedGroupColorSpace,
    ShadingNotDictionary,
    ShadingMissingEntry,
    InvalidShadingEntry,
    UnsupportedShadingEntry,
    UnsupportedShadingType,
    UnsupportedShadingColorSpace,
    FunctionMissingEntry,
    InvalidFunction,
    UnsupportedFunction,
    FunctionResource(PageContentErrorKind),
    FunctionMissingData,
    FunctionInputLimit,
    PostScriptFunction(postscript::PostScriptError),
    UnsupportedFunctionEntry,
    FunctionDepthLimit,
    FunctionCountLimit,
    FormContent,
    FormMissingBBox,
    InvalidFormEntry,
    FormInvocationCycle,
    FormDepthLimit,
    FormInvocationLimit,
    NestedTextObject,
    TextObjectMissing,
    UnclosedTextObject,
    OperatorInsideTextObject,
    FontNotDictionary,
    TextFontMissing,
    FontDecode(ResourceFontError),
    Type3Resource(PageContentErrorKind),
    Type3NotSimpleFont,
    Type3ProcedureContent,
    GlyphMetricOutsideProcedure,
    InvalidTextString,
    TextStringLimit,
    TextClippingUnsupported,
    TextClipWithoutOutlines,
    UnsupportedColorSpace,
    ColorSpaceMissingEntry,
    InvalidColorSpaceEntry,
    ColorSpaceDepthLimit,
    IccProfileResource(PageContentErrorKind),
    IccProfileMissingEntry,
    InvalidIccProfileEntry,
    UnsupportedIccProfileEntry,
    InvalidIccProfile,
    UnsupportedIccAlternate,
    InvalidSeparationColorSpace,
    InvalidDeviceNColorSpace,
    UnsupportedTintAlternate,
    ColorSpaceResource(PageContentErrorKind),
    InvalidTintTransform,
    InvalidIndexedColorSpace,
    UnsupportedIndexedBase,
    InvalidIndexedLookup,
    IndexedLookupLength { expected: usize, actual: usize },
    UnsupportedIndexedLookupEntry,
    IndexedLookupResource(PageContentErrorKind),
    IndexedLookupLimit,
    PatternResource(PageContentErrorKind),
    PatternContent,
    PatternMissingEntry,
    InvalidPatternEntry,
    UncoloredPatternUnsupported,
    PatternInvocationCycle,
    PatternDepthLimit,
    PatternInvocationLimit,
    PatternColorMissing,
}

impl fmt::Display for InterpretErrorKind {
    #[allow(clippy::too_many_lines)]
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedOperator => formatter.write_str("unsupported content operator"),
            Self::OperandCount { expected, actual } => {
                write!(formatter, "expected {expected} operands, found {actual}")
            }
            Self::OperandType => formatter.write_str("content operand has the wrong type"),
            Self::InvalidNumber => formatter.write_str("content number is invalid or non-finite"),
            Self::InvalidValue => {
                formatter.write_str("content operand value is outside its domain")
            }
            Self::CurrentPointMissing => formatter.write_str("path has no current point"),
            Self::GraphicsStackUnderflow => formatter.write_str("graphics-state stack underflow"),
            Self::UnbalancedMarkedContent { depth } => {
                write!(formatter, "unbalanced marked content, depth {depth}")
            }
            Self::MarkedContentDepthLimit => {
                formatter.write_str("marked-content nesting limit exceeded")
            }
            Self::UnbalancedCompatibilitySection { depth } => {
                write!(formatter, "compatibility section ends at depth {depth}")
            }
            Self::CompatibilitySectionMissing => formatter.write_str("EX has no matching BX"),
            Self::CompatibilitySectionLimit => {
                formatter.write_str("compatibility-section nesting limit exceeded")
            }
            Self::InvalidMarkedContentProperties => {
                formatter.write_str("marked-content property list has the wrong shape")
            }
            Self::FontProgram(error) => write!(formatter, "font program: {error}"),
            Self::UnsupportedOptionalContent => {
                formatter.write_str("optional content (/OC) is not connected yet")
            }
            Self::ImageResource(error) => write!(formatter, "image XObject: {error}"),
            Self::FormResource(error) => write!(formatter, "Form XObject: {error}"),
            Self::UnsupportedImageCodec(codec) => {
                write!(formatter, "unsupported image codec {}", codec.name())
            }
            Self::DctDecodeFailure => formatter.write_str("malformed or oversized DCT image"),
            Self::CcittDecodeFailure(error) => write!(formatter, "{error}"),
            Self::Jbig2DecodeFailure(error) => write!(formatter, "{error}"),
            Self::JpxDecodeFailure(kind) => {
                write!(formatter, "malformed JPEG 2000 image: {kind}")
            }
            Self::JpxMetadataMismatch => {
                formatter.write_str("JPEG 2000 samples disagree with the PDF image dictionary")
            }
            Self::DctMetadataMismatch => {
                formatter.write_str("DCT samples disagree with the PDF image dictionary")
            }
            Self::UnsupportedDctPrecision => {
                formatter.write_str("unsupported greater-than-8-bit DCT image")
            }
            Self::InvalidImageCodecParameters => {
                formatter.write_str("image codec parameters have the wrong shape or value")
            }
            Self::UnsupportedImageCodecParameters => {
                formatter.write_str("image codec parameters contain an unsupported entry")
            }
            Self::InvalidImageEntry => {
                formatter.write_str("image dictionary entry has the wrong shape or value")
            }
            Self::UnsupportedImageEntry => {
                formatter.write_str("unsupported image dictionary entry")
            }
            Self::ImageMissingEntry => {
                formatter.write_str("image dictionary is missing a required entry")
            }
            Self::ImageSampleShortfall => {
                formatter.write_str("image stream is shorter than its declared samples")
            }
            Self::ImageLimit => formatter.write_str("image limit exceeded"),
            Self::UnbalancedGraphicsState { depth } => {
                write!(formatter, "graphics-state stack ends at depth {depth}")
            }
            Self::GraphicsStackLimit => formatter.write_str("graphics-state stack limit exceeded"),
            Self::PathSegmentLimit => formatter.write_str("path segment limit exceeded"),
            Self::PaintAtomLimit => formatter.write_str("paint atom limit exceeded"),
            Self::ClipPathLimit => formatter.write_str("clip path limit exceeded"),
            Self::SourceSpanFailure => {
                formatter.write_str("content source span cannot be resolved")
            }
            Self::ResourceScopeMissing => {
                formatter.write_str("content operator requires a resource scope")
            }
            Self::ResourceNotFound => formatter.write_str("named resource was not found"),
            Self::ExtGStateNotDictionary => {
                formatter.write_str("ExtGState resource is not a dictionary")
            }
            Self::ResourceSourceFailure => {
                formatter.write_str("resource source span cannot be resolved")
            }
            Self::DuplicateExtGStateEntry => {
                formatter.write_str("ExtGState dictionary contains a duplicate entry")
            }
            Self::InvalidExtGStateEntry => {
                formatter.write_str("ExtGState entry has the wrong type or shape")
            }
            Self::UnsupportedExtGStateEntry => formatter.write_str("unsupported ExtGState entry"),
            Self::UnsupportedBlendMode => formatter.write_str("unsupported PDF blend mode"),
            Self::SoftMaskMissingEntry => {
                formatter.write_str("soft-mask dictionary is missing a required entry")
            }
            Self::InvalidSoftMaskEntry => {
                formatter.write_str("soft-mask entry has the wrong type or shape")
            }
            Self::UnsupportedSoftMaskEntry => {
                formatter.write_str("unsupported soft-mask dictionary entry")
            }
            Self::SoftMaskResource(error) => write!(formatter, "soft-mask group: {error}"),
            Self::UnsupportedSoftMaskTransfer => {
                formatter.write_str("soft-mask transfer function is unsupported")
            }
            Self::UnsupportedXObject => formatter.write_str("XObject is not a supported Form"),
            Self::GroupResource(error) => write!(formatter, "transparency group: {error}"),
            Self::GroupMissingEntry => {
                formatter.write_str("transparency group is missing a required entry")
            }
            Self::InvalidGroupEntry => {
                formatter.write_str("transparency-group entry has the wrong type or shape")
            }
            Self::UnsupportedGroupSubtype => formatter.write_str("unsupported Form group subtype"),
            Self::UnsupportedGroupColorSpace => {
                formatter.write_str("unsupported transparency-group blend colour space")
            }
            Self::ShadingNotDictionary => {
                formatter.write_str("shading resource is not a dictionary")
            }
            Self::ShadingMissingEntry => formatter.write_str("shading is missing a required entry"),
            Self::InvalidShadingEntry => {
                formatter.write_str("shading entry has the wrong type or value")
            }
            Self::UnsupportedShadingEntry => {
                formatter.write_str("unsupported shading dictionary entry")
            }
            Self::UnsupportedShadingType => formatter.write_str("unsupported PDF shading type"),
            Self::UnsupportedShadingColorSpace => {
                formatter.write_str("unsupported shading colour space")
            }
            Self::FunctionMissingEntry => {
                formatter.write_str("function is missing a required entry")
            }
            Self::InvalidFunction => formatter.write_str("function has the wrong type or value"),
            Self::UnsupportedFunction => formatter.write_str("unsupported PDF function type"),
            Self::FunctionResource(error) => write!(formatter, "function object: {error}"),
            Self::FunctionMissingData => {
                formatter.write_str("function object has no stream data to run")
            }
            Self::FunctionInputLimit => formatter.write_str("function input limit exceeded"),
            Self::PostScriptFunction(error) => write!(formatter, "{error}"),
            Self::UnsupportedFunctionEntry => formatter.write_str("unsupported function entry"),
            Self::FunctionDepthLimit => {
                formatter.write_str("shading-function depth limit exceeded")
            }
            Self::FunctionCountLimit => {
                formatter.write_str("shading-function count limit exceeded")
            }
            Self::FormContent => formatter.write_str("Form content stream is malformed"),
            Self::FormMissingBBox => formatter.write_str("Form XObject has no /BBox"),
            Self::InvalidFormEntry => formatter.write_str("Form matrix or BBox is invalid"),
            Self::FormInvocationCycle => formatter.write_str("Form invocation contains a cycle"),
            Self::FormDepthLimit => formatter.write_str("Form invocation depth limit exceeded"),
            Self::FormInvocationLimit => formatter.write_str("Form invocation limit exceeded"),
            Self::NestedTextObject => formatter.write_str("text objects cannot be nested"),
            Self::TextObjectMissing => formatter.write_str("text operator is outside BT/ET"),
            Self::UnclosedTextObject => formatter.write_str("text object is missing ET"),
            Self::OperatorInsideTextObject => {
                formatter.write_str("operator is not allowed inside a text object")
            }
            Self::FontNotDictionary => formatter.write_str("font resource is not a dictionary"),
            Self::TextFontMissing => {
                formatter.write_str("text-show operation has no selected font")
            }
            Self::FontDecode(error) => write!(formatter, "selected font: {error}"),
            Self::Type3Resource(error) => write!(formatter, "Type 3 font: {error}"),
            Self::Type3NotSimpleFont => {
                formatter.write_str("Type 3 font reached as a composite font's descendant")
            }
            Self::Type3ProcedureContent => {
                formatter.write_str("Type 3 glyph procedure is not a content stream")
            }
            Self::GlyphMetricOutsideProcedure => {
                formatter.write_str("d0 or d1 outside a Type 3 glyph procedure")
            }
            Self::InvalidTextString => formatter.write_str("text-show string is malformed"),
            Self::TextStringLimit => formatter.write_str("decoded text-string limit exceeded"),
            Self::TextClippingUnsupported => {
                formatter.write_str("text clipping requires glyph outlines")
            }
            Self::TextClipWithoutOutlines => formatter.write_str(
                "a clipping text mode showed a glyph with no outline, which would clip too little",
            ),
            Self::UnsupportedColorSpace => {
                formatter.write_str("selected colour space is unsupported")
            }
            Self::ColorSpaceMissingEntry => {
                formatter.write_str("colour-space dictionary is missing a required entry")
            }
            Self::InvalidColorSpaceEntry => {
                formatter.write_str("colour-space entry has the wrong type or value")
            }
            Self::ColorSpaceDepthLimit => {
                formatter.write_str("nested colour-space depth limit exceeded")
            }
            Self::IccProfileResource(error) => write!(formatter, "ICC profile: {error}"),
            Self::IccProfileMissingEntry => {
                formatter.write_str("ICC profile dictionary is missing a required entry")
            }
            Self::InvalidIccProfileEntry => {
                formatter.write_str("ICC profile dictionary entry has the wrong type or value")
            }
            Self::UnsupportedIccProfileEntry => {
                formatter.write_str("unsupported ICC profile stream entry")
            }
            Self::InvalidIccProfile => formatter.write_str("ICC profile bytes are malformed"),
            Self::UnsupportedIccAlternate => {
                formatter.write_str("ICC profile alternate colour space is unsupported")
            }
            Self::InvalidSeparationColorSpace => {
                formatter.write_str("Separation colour space has the wrong shape or value")
            }
            Self::InvalidDeviceNColorSpace => {
                formatter.write_str("DeviceN colour space has the wrong shape or value")
            }
            Self::ColorSpaceResource(error) => write!(formatter, "colour space: {error}"),
            Self::UnsupportedTintAlternate => {
                formatter.write_str("unsupported Separation/DeviceN alternate colour space")
            }
            Self::InvalidTintTransform => {
                formatter.write_str("tint transform arity disagrees with the colourant list")
            }
            Self::InvalidIndexedColorSpace => {
                formatter.write_str("Indexed colour space has the wrong shape or value")
            }
            Self::UnsupportedIndexedBase => {
                formatter.write_str("Indexed base colour space is unsupported")
            }
            Self::InvalidIndexedLookup => formatter.write_str("Indexed lookup table is malformed"),
            Self::IndexedLookupLength { expected, actual } => write!(
                formatter,
                "Indexed lookup table holds {actual} bytes where the colour space declares {expected}"
            ),
            Self::UnsupportedIndexedLookupEntry => {
                formatter.write_str("unsupported Indexed lookup stream entry")
            }
            Self::IndexedLookupResource(error) => write!(formatter, "Indexed lookup: {error}"),
            Self::IndexedLookupLimit => formatter.write_str("Indexed lookup byte limit exceeded"),
            Self::PatternResource(error) => write!(formatter, "selected pattern: {error}"),
            Self::PatternContent => formatter.write_str("pattern content stream is malformed"),
            Self::PatternMissingEntry => {
                formatter.write_str("tiling pattern is missing a required entry")
            }
            Self::InvalidPatternEntry => {
                formatter.write_str("tiling-pattern entry has the wrong type or value")
            }
            Self::UncoloredPatternUnsupported => formatter
                .write_str("uncoloured tiling pattern selected through a space with no base"),
            Self::PatternInvocationCycle => {
                formatter.write_str("tiling-pattern invocation contains a cycle")
            }
            Self::PatternDepthLimit => {
                formatter.write_str("tiling-pattern invocation depth limit exceeded")
            }
            Self::PatternInvocationLimit => {
                formatter.write_str("tiling-pattern invocation limit exceeded")
            }
            Self::PatternColorMissing => {
                formatter.write_str("Pattern colour space has no selected pattern")
            }
        }
    }
}
