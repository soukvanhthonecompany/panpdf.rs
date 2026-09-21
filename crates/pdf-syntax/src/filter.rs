use std::fmt;
use std::io::Read;

use flate2::bufread::ZlibDecoder as BufferedZlibDecoder;
use pdf_bytes::{ByteStore, SourceSpan};

use crate::value::{name_object_equals, parse_unsigned};
use crate::{DictionaryEntry, NumberKind, Object, ObjectKind, StreamObject};

pub(crate) fn decode_stream(
    source: &ByteStore,
    dictionary: &[DictionaryEntry],
    stream: &StreamObject,
    max_decoded_bytes: usize,
) -> Result<Vec<u8>, StreamDecodeError> {
    let encoded = source.resolve(stream.data_span()).map_err(|_| {
        StreamDecodeError::new(
            stream.data_span().start(),
            StreamDecodeErrorKind::SourceSpanFailure,
        )
    })?;
    decode_stream_bytes(
        source,
        dictionary,
        encoded,
        stream.data_span().start(),
        max_decoded_bytes,
    )
}

pub fn decode_stream_bytes(
    source: &ByteStore,
    dictionary: &[DictionaryEntry],
    encoded: &[u8],
    encoded_offset: usize,
    max_decoded_bytes: usize,
) -> Result<Vec<u8>, StreamDecodeError> {
    decode_pipeline(
        source,
        dictionary,
        encoded,
        encoded_offset,
        max_decoded_bytes,
        false,
        Tolerance::Strict,
    )
    .map(|decoded| decoded.bytes)
}

pub fn decode_stream_bytes_recovering(
    source: &ByteStore,
    dictionary: &[DictionaryEntry],
    encoded: &[u8],
    encoded_offset: usize,
    max_decoded_bytes: usize,
) -> Result<DecodedStream, StreamDecodeError> {
    decode_pipeline(
        source,
        dictionary,
        encoded,
        encoded_offset,
        max_decoded_bytes,
        false,
        Tolerance::EndOfDataEndsIt,
    )
    .map(|decoded| DecodedStream {
        bytes: decoded.bytes,
        repairs: decoded.repairs,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageStreamData {
    pub bytes: Vec<u8>,
    pub codec: Option<ImageCodec>,
    pub codec_span: Option<SourceSpan>,
    pub codec_parameters: Option<Object>,
    pub repairs: Vec<StreamRepair>,
}

pub fn decode_image_stream_bytes(
    source: &ByteStore,
    dictionary: &[DictionaryEntry],
    encoded: &[u8],
    encoded_offset: usize,
    max_decoded_bytes: usize,
) -> Result<ImageStreamData, StreamDecodeError> {
    decode_image_stream_with(
        source,
        dictionary,
        encoded,
        encoded_offset,
        max_decoded_bytes,
        Tolerance::Strict,
    )
}

pub fn decode_image_stream_bytes_recovering(
    source: &ByteStore,
    dictionary: &[DictionaryEntry],
    encoded: &[u8],
    encoded_offset: usize,
    max_decoded_bytes: usize,
) -> Result<ImageStreamData, StreamDecodeError> {
    decode_image_stream_with(
        source,
        dictionary,
        encoded,
        encoded_offset,
        max_decoded_bytes,
        Tolerance::EndOfDataEndsIt,
    )
}

fn decode_image_stream_with(
    source: &ByteStore,
    dictionary: &[DictionaryEntry],
    encoded: &[u8],
    encoded_offset: usize,
    max_decoded_bytes: usize,
    tolerance: Tolerance,
) -> Result<ImageStreamData, StreamDecodeError> {
    let filters = parse_filter_pipeline(source, dictionary)?;
    let codec_at = filters
        .iter()
        .position(|filter| matches!(filter.kind, FilterKind::Image(_)));
    let Some(codec_at) = codec_at else {
        let decoded = decode_pipeline(
            source,
            dictionary,
            encoded,
            encoded_offset,
            max_decoded_bytes,
            false,
            tolerance,
        )?;
        return Ok(ImageStreamData {
            bytes: decoded.bytes,
            codec: None,
            codec_span: None,
            codec_parameters: None,
            repairs: decoded.repairs,
        });
    };
    if codec_at + 1 != filters.len() {
        return Err(StreamDecodeError::new(
            filters[codec_at].offset,
            StreamDecodeErrorKind::UnsupportedFilter,
        ));
    }
    let FilterKind::Image(codec) = filters[codec_at].kind else {
        unreachable!("position matched an image filter")
    };
    let general = &filters[..codec_at];
    let mut repairs = Vec::new();
    let bytes = if general.is_empty() {
        if encoded.len() > max_decoded_bytes {
            return Err(StreamDecodeError::new(
                encoded_offset,
                StreamDecodeErrorKind::ExpansionLimit,
            ));
        }
        encoded.to_vec()
    } else {
        let decoded = decode_pipeline_with_filters(
            source,
            general,
            encoded,
            encoded_offset,
            max_decoded_bytes,
            false,
            tolerance,
        )?;
        repairs = decoded.repairs;
        decoded.bytes
    };
    Ok(ImageStreamData {
        bytes,
        repairs,
        codec: Some(codec),
        codec_span: Some(filters[codec_at].span),
        codec_parameters: filters[codec_at].parameters.cloned(),
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PrefixDecodedStream {
    decoded: Vec<u8>,
    encoded_length: usize,
}

impl PrefixDecodedStream {
    pub(crate) fn decoded(&self) -> &[u8] {
        &self.decoded
    }

    pub(crate) const fn encoded_length(&self) -> usize {
        self.encoded_length
    }
}

pub(crate) fn decode_xref_stream_prefix(
    source: &ByteStore,
    dictionary: &[DictionaryEntry],
    data_start: usize,
    expected_decoded_length: usize,
    max_decoded_bytes: usize,
) -> Result<PrefixDecodedStream, StreamDecodeError> {
    let filters = parse_filter_pipeline(source, dictionary)?;
    if filters.is_empty() {
        if expected_decoded_length > max_decoded_bytes {
            return Err(StreamDecodeError::new(
                data_start,
                StreamDecodeErrorKind::ExpansionLimit,
            ));
        }
        let data_end = data_start
            .checked_add(expected_decoded_length)
            .ok_or_else(|| {
                StreamDecodeError::new(data_start, StreamDecodeErrorKind::SourceSpanFailure)
            })?;
        let span = source.span(data_start..data_end).map_err(|_| {
            StreamDecodeError::new(data_start, StreamDecodeErrorKind::SourceSpanFailure)
        })?;
        let decoded = source.resolve(span).map_err(|_| {
            StreamDecodeError::new(data_start, StreamDecodeErrorKind::SourceSpanFailure)
        })?;
        return Ok(PrefixDecodedStream {
            decoded: decoded.to_vec(),
            encoded_length: expected_decoded_length,
        });
    }

    let encoded = source.as_bytes().get(data_start..).ok_or_else(|| {
        StreamDecodeError::new(data_start, StreamDecodeErrorKind::SourceSpanFailure)
    })?;
    let decoded = decode_pipeline_with_filters(
        source,
        &filters,
        encoded,
        data_start,
        max_decoded_bytes,
        true,
        Tolerance::Strict,
    )?;
    Ok(PrefixDecodedStream {
        decoded: decoded.bytes,
        encoded_length: decoded.encoded_length,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FilterKind {
    AsciiHex,
    Ascii85,
    Flate,
    Lzw,
    RunLength,
    Image(ImageCodec),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageCodec {
    Dct,
    CcittFax,
    Jbig2,
    Jpx,
    Crypt,
}

impl ImageCodec {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Dct => "/DCTDecode",
            Self::CcittFax => "/CCITTFaxDecode",
            Self::Jbig2 => "/JBIG2Decode",
            Self::Jpx => "/JPXDecode",
            Self::Crypt => "/Crypt",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct FilterSpec<'a> {
    kind: FilterKind,
    parameters: Option<&'a Object>,
    offset: usize,
    span: SourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PipelineOutput {
    bytes: Vec<u8>,
    encoded_length: usize,
    repairs: Vec<StreamRepair>,
}

fn decode_pipeline(
    source: &ByteStore,
    dictionary: &[DictionaryEntry],
    encoded: &[u8],
    offset: usize,
    max_decoded_bytes: usize,
    prefix: bool,
    tolerance: Tolerance,
) -> Result<PipelineOutput, StreamDecodeError> {
    let filters = parse_filter_pipeline(source, dictionary)?;
    if filters.is_empty() {
        if encoded.len() > max_decoded_bytes {
            return Err(StreamDecodeError::new(
                offset,
                StreamDecodeErrorKind::ExpansionLimit,
            ));
        }
        return Ok(PipelineOutput {
            bytes: encoded.to_vec(),
            encoded_length: encoded.len(),
            repairs: Vec::new(),
        });
    }
    decode_pipeline_with_filters(
        source,
        &filters,
        encoded,
        offset,
        max_decoded_bytes,
        prefix,
        tolerance,
    )
}

fn decode_pipeline_with_filters(
    source: &ByteStore,
    filters: &[FilterSpec<'_>],
    encoded: &[u8],
    offset: usize,
    max_decoded_bytes: usize,
    prefix: bool,
    tolerance: Tolerance,
) -> Result<PipelineOutput, StreamDecodeError> {
    let mut current = None;
    let mut encoded_length = encoded.len();
    let mut repairs = Vec::new();
    for (index, filter) in filters.iter().enumerate() {
        let input = current.as_deref().unwrap_or(encoded);
        let decoded = decode_filter(source, *filter, input, offset, max_decoded_bytes, tolerance)?;
        if index == 0 {
            encoded_length = decoded.consumed;
        }
        if decoded.ended_without_eod
            && let Some(filter) = self_delimiting(filter.kind)
        {
            repairs.push(StreamRepair::FilterEndedWithoutEod {
                byte_offset: offset.saturating_add(decoded.consumed),
                filter,
            });
        }
        if !(prefix && index == 0)
            && decoded.consumed != input.len()
            && !input[decoded.consumed..]
                .iter()
                .all(|byte| crate::token::is_whitespace(*byte))
        {
            return Err(StreamDecodeError::new(
                offset.saturating_add(decoded.consumed),
                StreamDecodeErrorKind::TrailingFilterData,
            ));
        }
        current = Some(decoded.bytes);
    }
    Ok(PipelineOutput {
        bytes: current.unwrap_or_default(),
        encoded_length,
        repairs,
    })
}

const fn self_delimiting(kind: FilterKind) -> Option<SelfDelimitingFilter> {
    match kind {
        FilterKind::AsciiHex => Some(SelfDelimitingFilter::AsciiHex),
        FilterKind::Ascii85 => Some(SelfDelimitingFilter::Ascii85),
        FilterKind::Lzw => Some(SelfDelimitingFilter::Lzw),
        FilterKind::RunLength => Some(SelfDelimitingFilter::RunLength),
        FilterKind::Flate | FilterKind::Image(_) => None,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FilterOutput {
    bytes: Vec<u8>,
    consumed: usize,
    ended_without_eod: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Tolerance {
    Strict,
    EndOfDataEndsIt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelfDelimitingFilter {
    AsciiHex,
    Ascii85,
    Lzw,
    RunLength,
}

impl fmt::Display for SelfDelimitingFilter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::AsciiHex => "/ASCIIHexDecode",
            Self::Ascii85 => "/ASCII85Decode",
            Self::Lzw => "/LZWDecode",
            Self::RunLength => "/RunLengthDecode",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamRepair {
    FilterEndedWithoutEod {
        byte_offset: usize,
        filter: SelfDelimitingFilter,
    },
}

impl fmt::Display for StreamRepair {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FilterEndedWithoutEod { filter, .. } => write!(
                formatter,
                "{filter} data ended without its end marker; the end of the data stood in for it"
            ),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedStream {
    pub bytes: Vec<u8>,
    pub repairs: Vec<StreamRepair>,
}

fn decode_filter(
    source: &ByteStore,
    filter: FilterSpec<'_>,
    encoded: &[u8],
    offset: usize,
    max_decoded_bytes: usize,
    tolerance: Tolerance,
) -> Result<FilterOutput, StreamDecodeError> {
    match filter.kind {
        FilterKind::Image(_) => Err(StreamDecodeError::new(
            filter.offset,
            StreamDecodeErrorKind::UnsupportedFilter,
        )),
        FilterKind::AsciiHex => {
            require_no_parameters(filter)?;
            decode_ascii_hex(encoded, offset, max_decoded_bytes, tolerance)
        }
        FilterKind::Ascii85 => {
            require_no_parameters(filter)?;
            decode_ascii85(encoded, offset, max_decoded_bytes, tolerance)
        }
        FilterKind::Flate => {
            let predictor = parse_predictor(source, filter.parameters)?;
            let inflated_limit = match predictor {
                Predictor::None | Predictor::Tiff(_) => max_decoded_bytes,
                Predictor::Png(parameters) => parameters
                    .inflated_limit(max_decoded_bytes)
                    .ok_or_else(|| {
                        StreamDecodeError::new(
                            value_offset(filter.parameters),
                            StreamDecodeErrorKind::ExpansionLimit,
                        )
                    })?,
            };
            let (bytes, consumed) = decode_flate_prefix(encoded, offset, inflated_limit)?;
            let bytes = match predictor {
                Predictor::None => bytes,
                Predictor::Tiff(parameters) => {
                    decode_tiff_predictor(bytes, parameters, value_offset(filter.parameters))?
                }
                Predictor::Png(parameters) => {
                    decode_png_predictor(bytes, parameters, value_offset(filter.parameters))?
                }
            };
            Ok(FilterOutput {
                bytes,
                consumed,
                ended_without_eod: false,
            })
        }
        FilterKind::Lzw => {
            let predictor = parse_predictor(source, filter.parameters)?;
            let early_change = parse_early_change(source, filter.parameters)?;
            let inflated_limit = match predictor {
                Predictor::None | Predictor::Tiff(_) => max_decoded_bytes,
                Predictor::Png(parameters) => parameters
                    .inflated_limit(max_decoded_bytes)
                    .ok_or_else(|| {
                        StreamDecodeError::new(
                            value_offset(filter.parameters),
                            StreamDecodeErrorKind::ExpansionLimit,
                        )
                    })?,
            };
            let (bytes, consumed, ended_without_eod) =
                decode_lzw_prefix(encoded, offset, inflated_limit, early_change, tolerance)?;
            let bytes = match predictor {
                Predictor::None => bytes,
                Predictor::Tiff(parameters) => {
                    decode_tiff_predictor(bytes, parameters, value_offset(filter.parameters))?
                }
                Predictor::Png(parameters) => {
                    decode_png_predictor(bytes, parameters, value_offset(filter.parameters))?
                }
            };
            Ok(FilterOutput {
                bytes,
                consumed,
                ended_without_eod,
            })
        }
        FilterKind::RunLength => {
            require_no_parameters(filter)?;
            decode_run_length(encoded, offset, max_decoded_bytes, tolerance)
        }
    }
}

fn parse_filter_pipeline<'a>(
    source: &ByteStore,
    dictionary: &'a [DictionaryEntry],
) -> Result<Vec<FilterSpec<'a>>, StreamDecodeError> {
    let filter = unique_entry(source, dictionary, b"/Filter")?;
    let decode_parameters = unique_entry(source, dictionary, b"/DecodeParms")?;
    let Some(filter) = filter else {
        if decode_parameters.is_some_and(|value| !matches!(value.kind(), ObjectKind::Null)) {
            return Err(StreamDecodeError::new(
                value_offset(decode_parameters),
                StreamDecodeErrorKind::InvalidDecodeParameters,
            ));
        }
        return Ok(Vec::new());
    };

    let filter_values: Vec<&Object> = match filter.kind() {
        ObjectKind::Name => vec![filter],
        ObjectKind::Array(values) if !values.is_empty() => values.iter().collect(),
        _ => {
            return Err(StreamDecodeError::new(
                filter.span().start(),
                StreamDecodeErrorKind::InvalidFilter,
            ));
        }
    };
    let parameters = match decode_parameters {
        None => vec![None; filter_values.len()],
        Some(value) if matches!(value.kind(), ObjectKind::Null) => {
            vec![None; filter_values.len()]
        }
        Some(value) if filter_values.len() == 1 => match value.kind() {
            ObjectKind::Dictionary(_) => vec![Some(value)],
            ObjectKind::Array(values) if values.len() == 1 => {
                vec![decode_parameter(values.first().expect("length checked"))?]
            }
            _ => {
                return Err(StreamDecodeError::new(
                    value.span().start(),
                    StreamDecodeErrorKind::InvalidDecodeParameters,
                ));
            }
        },
        Some(value) => {
            let ObjectKind::Array(values) = value.kind() else {
                return Err(StreamDecodeError::new(
                    value.span().start(),
                    StreamDecodeErrorKind::InvalidDecodeParameters,
                ));
            };
            if values.len() != filter_values.len() {
                return Err(StreamDecodeError::new(
                    value.span().start(),
                    StreamDecodeErrorKind::InvalidDecodeParameters,
                ));
            }
            values
                .iter()
                .map(decode_parameter)
                .collect::<Result<Vec<_>, _>>()?
        }
    };

    filter_values
        .into_iter()
        .zip(parameters)
        .map(|(value, parameters)| {
            Ok(FilterSpec {
                kind: parse_filter_kind(source, value)?,
                parameters,
                offset: value.span().start(),
                span: value.span(),
            })
        })
        .collect()
}

fn decode_parameter(value: &Object) -> Result<Option<&Object>, StreamDecodeError> {
    match value.kind() {
        ObjectKind::Null => Ok(None),
        ObjectKind::Dictionary(_) => Ok(Some(value)),
        _ => Err(StreamDecodeError::new(
            value.span().start(),
            StreamDecodeErrorKind::InvalidDecodeParameters,
        )),
    }
}

fn parse_filter_kind(source: &ByteStore, value: &Object) -> Result<FilterKind, StreamDecodeError> {
    if !matches!(value.kind(), ObjectKind::Name) {
        return Err(StreamDecodeError::new(
            value.span().start(),
            StreamDecodeErrorKind::InvalidFilter,
        ));
    }
    if name_object_equals(source, value, b"/ASCIIHexDecode")
        || name_object_equals(source, value, b"/AHx")
    {
        Ok(FilterKind::AsciiHex)
    } else if name_object_equals(source, value, b"/ASCII85Decode")
        || name_object_equals(source, value, b"/A85")
    {
        Ok(FilterKind::Ascii85)
    } else if name_object_equals(source, value, b"/FlateDecode")
        || name_object_equals(source, value, b"/Fl")
    {
        Ok(FilterKind::Flate)
    } else if name_object_equals(source, value, b"/LZWDecode")
        || name_object_equals(source, value, b"/LZW")
    {
        Ok(FilterKind::Lzw)
    } else if name_object_equals(source, value, b"/RunLengthDecode")
        || name_object_equals(source, value, b"/RL")
    {
        Ok(FilterKind::RunLength)
    } else if name_object_equals(source, value, b"/DCTDecode")
        || name_object_equals(source, value, b"/DCT")
    {
        Ok(FilterKind::Image(ImageCodec::Dct))
    } else if name_object_equals(source, value, b"/CCITTFaxDecode")
        || name_object_equals(source, value, b"/CCF")
    {
        Ok(FilterKind::Image(ImageCodec::CcittFax))
    } else if name_object_equals(source, value, b"/JBIG2Decode") {
        Ok(FilterKind::Image(ImageCodec::Jbig2))
    } else if name_object_equals(source, value, b"/JPXDecode") {
        Ok(FilterKind::Image(ImageCodec::Jpx))
    } else if name_object_equals(source, value, b"/Crypt") {
        Ok(FilterKind::Image(ImageCodec::Crypt))
    } else {
        Err(StreamDecodeError::new(
            value.span().start(),
            StreamDecodeErrorKind::UnsupportedFilter,
        ))
    }
}

fn require_no_parameters(filter: FilterSpec<'_>) -> Result<(), StreamDecodeError> {
    if filter.parameters.is_some() {
        return Err(StreamDecodeError::new(
            filter.offset,
            StreamDecodeErrorKind::UnsupportedDecodeParameters,
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Predictor {
    None,
    Tiff(TiffParameters),
    Png(PngParameters),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TiffParameters {
    colors: usize,
    bits_per_component: usize,
    samples_per_row: usize,
    row_bits: usize,
    row_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PngParameters {
    row_bytes: usize,
    bytes_per_pixel: usize,
}

impl PngParameters {
    fn inflated_limit(self, decoded_limit: usize) -> Option<usize> {
        let rows = decoded_limit / self.row_bytes;
        rows.checked_mul(self.row_bytes.checked_add(1)?)
    }
}

fn parse_predictor(
    source: &ByteStore,
    decode_parameters: Option<&Object>,
) -> Result<Predictor, StreamDecodeError> {
    let Some(parameters) = decode_parameters else {
        return Ok(Predictor::None);
    };
    if matches!(parameters.kind(), ObjectKind::Null) {
        return Ok(Predictor::None);
    }
    let ObjectKind::Dictionary(entries) = parameters.kind() else {
        return Err(StreamDecodeError::new(
            parameters.span().start(),
            StreamDecodeErrorKind::UnsupportedDecodeParameters,
        ));
    };

    let predictor = unsigned_parameter(source, entries, b"/Predictor", 1)?;
    if predictor == 1 {
        return Ok(Predictor::None);
    }
    if predictor != 2 && !(10..=15).contains(&predictor) {
        return Err(StreamDecodeError::new(
            parameters.span().start(),
            StreamDecodeErrorKind::UnsupportedDecodeParameters,
        ));
    }

    let colors = unsigned_parameter(source, entries, b"/Colors", 1)?;
    let bits_per_component = unsigned_parameter(source, entries, b"/BitsPerComponent", 8)?;
    let columns = unsigned_parameter(source, entries, b"/Columns", 1)?;
    if colors == 0 || columns == 0 || !matches!(bits_per_component, 1 | 2 | 4 | 8 | 16) {
        return Err(StreamDecodeError::new(
            parameters.span().start(),
            StreamDecodeErrorKind::InvalidDecodeParameters,
        ));
    }

    let bits_per_pixel = colors
        .checked_mul(bits_per_component)
        .ok_or_else(|| invalid_parameters(parameters))?;
    let row_bits = bits_per_pixel
        .checked_mul(columns)
        .ok_or_else(|| invalid_parameters(parameters))?;
    let bytes_per_pixel = bits_per_pixel
        .checked_add(7)
        .map(|bits| bits / 8)
        .ok_or_else(|| invalid_parameters(parameters))?;
    let row_bytes = row_bits
        .checked_add(7)
        .map(|bits| bits / 8)
        .ok_or_else(|| invalid_parameters(parameters))?;
    if predictor == 2 {
        let samples_per_row = colors
            .checked_mul(columns)
            .ok_or_else(|| invalid_parameters(parameters))?;
        Ok(Predictor::Tiff(TiffParameters {
            colors,
            bits_per_component,
            samples_per_row,
            row_bits,
            row_bytes,
        }))
    } else {
        Ok(Predictor::Png(PngParameters {
            row_bytes,
            bytes_per_pixel,
        }))
    }
}

fn parse_early_change(
    source: &ByteStore,
    decode_parameters: Option<&Object>,
) -> Result<usize, StreamDecodeError> {
    let Some(parameters) = decode_parameters else {
        return Ok(1);
    };
    if matches!(parameters.kind(), ObjectKind::Null) {
        return Ok(1);
    }
    let ObjectKind::Dictionary(entries) = parameters.kind() else {
        return Err(StreamDecodeError::new(
            parameters.span().start(),
            StreamDecodeErrorKind::InvalidDecodeParameters,
        ));
    };
    let early_change = unsigned_parameter(source, entries, b"/EarlyChange", 1)?;
    if !matches!(early_change, 0 | 1) {
        return Err(StreamDecodeError::new(
            parameters.span().start(),
            StreamDecodeErrorKind::InvalidDecodeParameters,
        ));
    }
    Ok(early_change)
}

fn unsigned_parameter(
    source: &ByteStore,
    dictionary: &[DictionaryEntry],
    key: &[u8],
    default: usize,
) -> Result<usize, StreamDecodeError> {
    let Some(value) = unique_entry(source, dictionary, key)? else {
        return Ok(default);
    };
    if !matches!(value.kind(), ObjectKind::Number(NumberKind::Integer)) {
        return Err(invalid_parameters(value));
    }
    source
        .resolve(value.span())
        .ok()
        .and_then(parse_unsigned)
        .and_then(|number| usize::try_from(number).ok())
        .ok_or_else(|| invalid_parameters(value))
}

fn invalid_parameters(object: &Object) -> StreamDecodeError {
    StreamDecodeError::new(
        object.span().start(),
        StreamDecodeErrorKind::InvalidDecodeParameters,
    )
}

fn decode_png_predictor(
    mut decoded: Vec<u8>,
    parameters: PngParameters,
    offset: usize,
) -> Result<Vec<u8>, StreamDecodeError> {
    let stride = parameters.row_bytes.checked_add(1).ok_or_else(|| {
        StreamDecodeError::new(offset, StreamDecodeErrorKind::InvalidDecodeParameters)
    })?;
    if !decoded.len().is_multiple_of(stride) {
        return Err(StreamDecodeError::new(
            offset,
            StreamDecodeErrorKind::MalformedPredictorData,
        ));
    }

    let rows = decoded.len() / stride;
    for row in 0..rows {
        let read_start = row * stride;
        let write_start = row * parameters.row_bytes;
        let algorithm = decoded[read_start];
        if algorithm > 4 {
            return Err(StreamDecodeError::new(
                offset,
                StreamDecodeErrorKind::MalformedPredictorData,
            ));
        }
        for column in 0..parameters.row_bytes {
            let raw = decoded[read_start + 1 + column];
            let left = if column >= parameters.bytes_per_pixel {
                decoded[write_start + column - parameters.bytes_per_pixel]
            } else {
                0
            };
            let above = if row > 0 {
                decoded[write_start + column - parameters.row_bytes]
            } else {
                0
            };
            let upper_left = if row > 0 && column >= parameters.bytes_per_pixel {
                decoded[write_start + column - parameters.row_bytes - parameters.bytes_per_pixel]
            } else {
                0
            };
            let predicted = match algorithm {
                0 => 0,
                1 => left,
                2 => above,
                3 => u8::midpoint(left, above),
                4 => paeth(left, above, upper_left),
                _ => unreachable!("algorithm checked above"),
            };
            decoded[write_start + column] = raw.wrapping_add(predicted);
        }
    }
    decoded.truncate(rows * parameters.row_bytes);
    Ok(decoded)
}

fn decode_tiff_predictor(
    mut decoded: Vec<u8>,
    parameters: TiffParameters,
    offset: usize,
) -> Result<Vec<u8>, StreamDecodeError> {
    if !decoded.len().is_multiple_of(parameters.row_bytes) {
        return Err(StreamDecodeError::new(
            offset,
            StreamDecodeErrorKind::MalformedPredictorData,
        ));
    }

    let sample_mask = (1_usize << parameters.bits_per_component) - 1;
    for row in decoded.chunks_exact_mut(parameters.row_bytes) {
        for sample_index in 0..parameters.samples_per_row {
            let bit_offset = sample_index * parameters.bits_per_component;
            let difference = read_sample(row, bit_offset, parameters.bits_per_component);
            let left = if sample_index >= parameters.colors {
                read_sample(
                    row,
                    bit_offset - parameters.colors * parameters.bits_per_component,
                    parameters.bits_per_component,
                )
            } else {
                0
            };
            let sample = difference.wrapping_add(left) & sample_mask;
            write_sample(row, bit_offset, parameters.bits_per_component, sample);
        }

        let padding_bits = parameters.row_bytes * 8 - parameters.row_bits;
        if padding_bits > 0 {
            let keep_mask = u8::MAX << padding_bits;
            *row.last_mut().expect("a valid predictor row is non-empty") &= keep_mask;
        }
    }
    Ok(decoded)
}

fn read_sample(bytes: &[u8], bit_offset: usize, width: usize) -> usize {
    let mut value = 0_usize;
    for bit in bit_offset..bit_offset + width {
        let byte = bytes[bit / 8];
        value = (value << 1) | usize::from((byte >> (7 - bit % 8)) & 1);
    }
    value
}

fn write_sample(bytes: &mut [u8], bit_offset: usize, width: usize, value: usize) {
    for index in 0..width {
        let bit = bit_offset + index;
        let mask = 1_u8 << (7 - bit % 8);
        if value & (1_usize << (width - index - 1)) == 0 {
            bytes[bit / 8] &= !mask;
        } else {
            bytes[bit / 8] |= mask;
        }
    }
}

fn paeth(left: u8, above: u8, upper_left: u8) -> u8 {
    let estimate = i32::from(left) + i32::from(above) - i32::from(upper_left);
    let left_distance = (estimate - i32::from(left)).abs();
    let above_distance = (estimate - i32::from(above)).abs();
    let upper_left_distance = (estimate - i32::from(upper_left)).abs();
    if left_distance <= above_distance && left_distance <= upper_left_distance {
        left
    } else if above_distance <= upper_left_distance {
        above
    } else {
        upper_left
    }
}

fn unique_entry<'a>(
    source: &ByteStore,
    dictionary: &'a [DictionaryEntry],
    key: &[u8],
) -> Result<Option<&'a crate::Object>, StreamDecodeError> {
    let mut matches = dictionary
        .iter()
        .filter(|entry| entry.key_equals(source, key));
    let Some(first) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err(StreamDecodeError::new(
            first.key().span().start(),
            StreamDecodeErrorKind::DuplicateParameter,
        ));
    }
    Ok(Some(first.value()))
}

fn value_offset(value: Option<&crate::Object>) -> usize {
    value.map_or(0, |object| object.span().start())
}

fn decode_ascii_hex(
    encoded: &[u8],
    offset: usize,
    max_decoded_bytes: usize,
    tolerance: Tolerance,
) -> Result<FilterOutput, StreamDecodeError> {
    let mut output = Vec::with_capacity((encoded.len() / 2).min(max_decoded_bytes));
    let mut high_nibble = None;
    for (index, &byte) in encoded.iter().enumerate() {
        if is_pdf_whitespace(byte) {
            continue;
        }
        if byte == b'>' {
            if let Some(high) = high_nibble {
                push_bounded(&mut output, high << 4, max_decoded_bytes, offset + index)?;
            }
            return Ok(FilterOutput {
                bytes: output,
                consumed: index + 1,
                ended_without_eod: false,
            });
        }
        let nibble = hex_nibble(byte).ok_or_else(|| {
            StreamDecodeError::new(offset + index, StreamDecodeErrorKind::MalformedAsciiHexData)
        })?;
        if let Some(high) = high_nibble.take() {
            push_bounded(
                &mut output,
                (high << 4) | nibble,
                max_decoded_bytes,
                offset + index,
            )?;
        } else {
            high_nibble = Some(nibble);
        }
    }
    if tolerance == Tolerance::Strict {
        return Err(StreamDecodeError::new(
            offset + encoded.len(),
            StreamDecodeErrorKind::MissingFilterEod,
        ));
    }
    if let Some(high) = high_nibble {
        push_bounded(
            &mut output,
            high << 4,
            max_decoded_bytes,
            offset + encoded.len(),
        )?;
    }
    Ok(FilterOutput {
        bytes: output,
        consumed: encoded.len(),
        ended_without_eod: true,
    })
}

fn decode_ascii85(
    encoded: &[u8],
    offset: usize,
    max_decoded_bytes: usize,
    tolerance: Tolerance,
) -> Result<FilterOutput, StreamDecodeError> {
    let mut output = Vec::with_capacity(encoded.len().min(max_decoded_bytes));
    let mut group = [0_u8; 5];
    let mut group_length = 0_usize;
    let mut index = 0_usize;
    while index < encoded.len() {
        let byte = encoded[index];
        if is_pdf_whitespace(byte) {
            index += 1;
            continue;
        }
        if byte == b'~' {
            if encoded.get(index + 1) != Some(&b'>') {
                return Err(StreamDecodeError::new(
                    offset + index,
                    StreamDecodeErrorKind::MalformedAscii85Data,
                ));
            }
            if group_length == 1 {
                return Err(StreamDecodeError::new(
                    offset + index,
                    StreamDecodeErrorKind::MalformedAscii85Data,
                ));
            }
            if group_length > 1 {
                group[group_length..].fill(84);
                let decoded = decode_ascii85_group(group, offset + index)?;
                extend_bounded(
                    &mut output,
                    &decoded[..group_length - 1],
                    max_decoded_bytes,
                    offset + index,
                )?;
            }
            return Ok(FilterOutput {
                bytes: output,
                consumed: index + 2,
                ended_without_eod: false,
            });
        }
        if byte == b'z' {
            if group_length != 0 {
                return Err(StreamDecodeError::new(
                    offset + index,
                    StreamDecodeErrorKind::MalformedAscii85Data,
                ));
            }
            extend_bounded(&mut output, &[0; 4], max_decoded_bytes, offset + index)?;
            index += 1;
            continue;
        }
        if !(b'!'..=b'u').contains(&byte) {
            return Err(StreamDecodeError::new(
                offset + index,
                StreamDecodeErrorKind::MalformedAscii85Data,
            ));
        }
        group[group_length] = byte - b'!';
        group_length += 1;
        if group_length == 5 {
            let decoded = decode_ascii85_group(group, offset + index)?;
            extend_bounded(&mut output, &decoded, max_decoded_bytes, offset + index)?;
            group_length = 0;
        }
        index += 1;
    }
    if tolerance == Tolerance::Strict {
        return Err(StreamDecodeError::new(
            offset + encoded.len(),
            StreamDecodeErrorKind::MissingFilterEod,
        ));
    }
    if group_length > 1 {
        group[group_length..].fill(84);
        let decoded = decode_ascii85_group(group, offset + encoded.len())?;
        extend_bounded(
            &mut output,
            &decoded[..group_length - 1],
            max_decoded_bytes,
            offset + encoded.len(),
        )?;
    }
    Ok(FilterOutput {
        bytes: output,
        consumed: encoded.len(),
        ended_without_eod: true,
    })
}

fn decode_ascii85_group(group: [u8; 5], offset: usize) -> Result<[u8; 4], StreamDecodeError> {
    let value = group
        .into_iter()
        .try_fold(0_u64, |value, digit| {
            value
                .checked_mul(85)
                .and_then(|value| value.checked_add(u64::from(digit)))
        })
        .ok_or_else(|| {
            StreamDecodeError::new(offset, StreamDecodeErrorKind::MalformedAscii85Data)
        })?;
    u32::try_from(value)
        .map(u32::to_be_bytes)
        .map_err(|_| StreamDecodeError::new(offset, StreamDecodeErrorKind::MalformedAscii85Data))
}

fn push_bounded(
    output: &mut Vec<u8>,
    byte: u8,
    limit: usize,
    offset: usize,
) -> Result<(), StreamDecodeError> {
    if output.len() >= limit {
        return Err(StreamDecodeError::new(
            offset,
            StreamDecodeErrorKind::ExpansionLimit,
        ));
    }
    output.push(byte);
    Ok(())
}

fn extend_bounded(
    output: &mut Vec<u8>,
    bytes: &[u8],
    limit: usize,
    offset: usize,
) -> Result<(), StreamDecodeError> {
    if output
        .len()
        .checked_add(bytes.len())
        .is_none_or(|length| length > limit)
    {
        return Err(StreamDecodeError::new(
            offset,
            StreamDecodeErrorKind::ExpansionLimit,
        ));
    }
    output.extend_from_slice(bytes);
    Ok(())
}

const fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

const fn is_pdf_whitespace(byte: u8) -> bool {
    matches!(byte, 0x00 | b'\t' | b'\n' | 0x0c | b'\r' | b' ')
}

fn decode_lzw_prefix(
    encoded: &[u8],
    offset: usize,
    max_decoded_bytes: usize,
    early_change: usize,
    tolerance: Tolerance,
) -> Result<(Vec<u8>, usize, bool), StreamDecodeError> {
    let mut reader = BitReader::new(encoded);
    let mut table = initial_lzw_table();
    let mut code_width = 9_usize;
    let mut previous: Option<Vec<u8>> = None;
    let mut output = Vec::with_capacity(encoded.len().min(max_decoded_bytes));

    loop {
        let Some(code) = reader.read(code_width) else {
            if tolerance == Tolerance::Strict {
                return Err(StreamDecodeError::new(
                    offset + encoded.len(),
                    StreamDecodeErrorKind::MissingFilterEod,
                ));
            }
            return Ok((output, encoded.len(), true));
        };
        match code {
            256 => {
                table.truncate(258);
                code_width = 9;
                previous = None;
            }
            257 => {
                if !reader.padding_is_zero() {
                    return Err(StreamDecodeError::new(
                        offset + reader.consumed_bytes().saturating_sub(1),
                        StreamDecodeErrorKind::MalformedLzwData,
                    ));
                }
                return Ok((output, reader.consumed_bytes(), false));
            }
            _ => {
                let entry = if let Some(entry) = table.get(code).filter(|entry| !entry.is_empty()) {
                    entry.clone()
                } else if code == table.len() {
                    let previous = previous.as_ref().ok_or_else(|| {
                        StreamDecodeError::new(
                            offset + reader.consumed_bytes(),
                            StreamDecodeErrorKind::MalformedLzwData,
                        )
                    })?;
                    let mut entry = previous.clone();
                    entry.push(previous[0]);
                    entry
                } else {
                    return Err(StreamDecodeError::new(
                        offset + reader.consumed_bytes(),
                        StreamDecodeErrorKind::MalformedLzwData,
                    ));
                };

                extend_bounded(
                    &mut output,
                    &entry,
                    max_decoded_bytes,
                    offset + reader.consumed_bytes(),
                )?;
                if let Some(previous_entry) = previous.as_ref() {
                    if table.len() >= 4096 {
                        return Err(StreamDecodeError::new(
                            offset + reader.consumed_bytes(),
                            StreamDecodeErrorKind::MalformedLzwData,
                        ));
                    }
                    let mut new_entry = previous_entry.clone();
                    new_entry.push(entry[0]);
                    table.push(new_entry);
                    let new_code = table.len() - 1;
                    if code_width < 12 && matches!(new_code + early_change, 511 | 1023 | 2047) {
                        code_width += 1;
                    }
                }
                previous = Some(entry);
            }
        }
    }
}

fn initial_lzw_table() -> Vec<Vec<u8>> {
    let mut table = Vec::with_capacity(4096);
    table.extend((u8::MIN..=u8::MAX).map(|byte| vec![byte]));
    table.push(Vec::new());
    table.push(Vec::new());
    table
}

struct BitReader<'a> {
    bytes: &'a [u8],
    bit_position: usize,
}

impl<'a> BitReader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            bit_position: 0,
        }
    }

    fn read(&mut self, width: usize) -> Option<usize> {
        if self
            .bit_position
            .checked_add(width)
            .is_none_or(|end| end > self.bytes.len().saturating_mul(8))
        {
            return None;
        }
        let mut value = 0_usize;
        for _ in 0..width {
            let byte = self.bytes[self.bit_position / 8];
            let shift = 7 - (self.bit_position % 8);
            value = (value << 1) | usize::from((byte >> shift) & 1);
            self.bit_position += 1;
        }
        Some(value)
    }

    fn consumed_bytes(&self) -> usize {
        self.bit_position.div_ceil(8)
    }

    fn padding_is_zero(&self) -> bool {
        let used = self.bit_position % 8;
        if used == 0 {
            return true;
        }
        let mask = (1_u8 << (8 - used)) - 1;
        self.bytes[self.bit_position / 8] & mask == 0
    }
}

fn decode_run_length(
    encoded: &[u8],
    offset: usize,
    max_decoded_bytes: usize,
    tolerance: Tolerance,
) -> Result<FilterOutput, StreamDecodeError> {
    let mut output = Vec::with_capacity(encoded.len().min(max_decoded_bytes));
    let mut cursor = 0_usize;
    while let Some(&length) = encoded.get(cursor) {
        cursor += 1;
        match length {
            128 => {
                return Ok(FilterOutput {
                    bytes: output,
                    consumed: cursor,
                    ended_without_eod: false,
                });
            }
            0..=127 => {
                let count = usize::from(length) + 1;
                let end = cursor.checked_add(count).ok_or_else(|| {
                    StreamDecodeError::new(
                        offset + cursor,
                        StreamDecodeErrorKind::MalformedRunLengthData,
                    )
                })?;
                let bytes = match encoded.get(cursor..end) {
                    Some(bytes) => bytes,
                    None if tolerance == Tolerance::EndOfDataEndsIt => {
                        extend_bounded(
                            &mut output,
                            &encoded[cursor..],
                            max_decoded_bytes,
                            offset + cursor,
                        )?;
                        return Ok(FilterOutput {
                            bytes: output,
                            consumed: encoded.len(),
                            ended_without_eod: true,
                        });
                    }
                    None => {
                        return Err(StreamDecodeError::new(
                            offset + cursor,
                            StreamDecodeErrorKind::MalformedRunLengthData,
                        ));
                    }
                };
                extend_bounded(&mut output, bytes, max_decoded_bytes, offset + cursor)?;
                cursor = end;
            }
            129..=255 => {
                let Some(&byte) = encoded.get(cursor) else {
                    if tolerance == Tolerance::EndOfDataEndsIt {
                        return Ok(FilterOutput {
                            bytes: output,
                            consumed: encoded.len(),
                            ended_without_eod: true,
                        });
                    }
                    return Err(StreamDecodeError::new(
                        offset + cursor,
                        StreamDecodeErrorKind::MalformedRunLengthData,
                    ));
                };
                cursor += 1;
                let count = 257_usize - usize::from(length);
                if output
                    .len()
                    .checked_add(count)
                    .is_none_or(|length| length > max_decoded_bytes)
                {
                    return Err(StreamDecodeError::new(
                        offset + cursor - 1,
                        StreamDecodeErrorKind::ExpansionLimit,
                    ));
                }
                output.resize(output.len() + count, byte);
            }
        }
    }
    if tolerance == Tolerance::Strict {
        return Err(StreamDecodeError::new(
            offset + encoded.len(),
            StreamDecodeErrorKind::MissingFilterEod,
        ));
    }
    Ok(FilterOutput {
        bytes: output,
        consumed: encoded.len(),
        ended_without_eod: true,
    })
}

pub fn inflate_zlib(
    encoded: &[u8],
    max_decoded_bytes: usize,
) -> Result<Vec<u8>, StreamDecodeError> {
    decode_flate_prefix(encoded, 0, max_decoded_bytes).map(|(bytes, _)| bytes)
}

#[must_use]
pub fn deflate_zlib(bytes: &[u8]) -> Vec<u8> {
    use std::io::Write as _;
    let mut encoder = flate2::write::ZlibEncoder::new(
        Vec::with_capacity(bytes.len() / 2),
        flate2::Compression::default(),
    );
    if encoder.write_all(bytes).is_err() {
        unreachable!("a Vec accepts every byte written to it");
    }
    encoder
        .finish()
        .unwrap_or_else(|_| unreachable!("a Vec accepts every byte written to it"))
}

fn decode_flate_prefix(
    encoded: &[u8],
    offset: usize,
    max_decoded_bytes: usize,
) -> Result<(Vec<u8>, usize), StreamDecodeError> {
    let mut decoder = BufferedZlibDecoder::new(encoded);
    let mut output = Vec::with_capacity(encoded.len().min(max_decoded_bytes));
    let mut buffer = [0_u8; 8192];
    loop {
        let count = decoder.read(&mut buffer).map_err(|_| {
            StreamDecodeError::new(offset, StreamDecodeErrorKind::MalformedFlateData)
        })?;
        if count == 0 {
            let encoded_length = usize::try_from(decoder.total_in()).map_err(|_| {
                StreamDecodeError::new(offset, StreamDecodeErrorKind::SourceSpanFailure)
            })?;
            return Ok((output, encoded_length));
        }
        if output
            .len()
            .checked_add(count)
            .is_none_or(|length| length > max_decoded_bytes)
        {
            return Err(StreamDecodeError::new(
                offset,
                StreamDecodeErrorKind::ExpansionLimit,
            ));
        }
        output.extend_from_slice(&buffer[..count]);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StreamDecodeError {
    offset: usize,
    kind: StreamDecodeErrorKind,
}

impl StreamDecodeError {
    const fn new(offset: usize, kind: StreamDecodeErrorKind) -> Self {
        Self { offset, kind }
    }

    #[must_use]
    pub const fn offset(self) -> usize {
        self.offset
    }

    #[must_use]
    pub const fn kind(self) -> StreamDecodeErrorKind {
        self.kind
    }
}

impl fmt::Display for StreamDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at byte {}", self.kind, self.offset)
    }
}

impl std::error::Error for StreamDecodeError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamDecodeErrorKind {
    DuplicateParameter,
    InvalidFilter,
    UnsupportedFilter,
    UnsupportedDecodeParameters,
    InvalidDecodeParameters,
    MissingFilterEod,
    TrailingFilterData,
    MalformedAsciiHexData,
    MalformedAscii85Data,
    MalformedLzwData,
    MalformedRunLengthData,
    MalformedPredictorData,
    MalformedFlateData,
    ExpansionLimit,
    SourceSpanFailure,
}

impl fmt::Display for StreamDecodeErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::DuplicateParameter => "stream dictionary repeats a decoding parameter",
            Self::InvalidFilter => "stream Filter is not a name or non-empty name array",
            Self::UnsupportedFilter => "stream uses an unsupported filter chain",
            Self::UnsupportedDecodeParameters => "stream uses unsupported decode parameters",
            Self::InvalidDecodeParameters => "stream has invalid decode parameters",
            Self::MissingFilterEod => "self-delimiting stream filter has no end marker",
            Self::TrailingFilterData => "encoded stream contains data after its filter end marker",
            Self::MalformedAsciiHexData => "stream contains malformed ASCII hexadecimal data",
            Self::MalformedAscii85Data => "stream contains malformed ASCII base-85 data",
            Self::MalformedLzwData => "stream contains malformed LZW data",
            Self::MalformedRunLengthData => "stream contains malformed run-length data",
            Self::MalformedPredictorData => "stream contains malformed predictor data",
            Self::MalformedFlateData => "stream contains malformed Flate data",
            Self::ExpansionLimit => "decoded stream exceeds its expansion limit",
            Self::SourceSpanFailure => "encoded stream span does not belong to the source",
        })
    }
}

#[cfg(test)]
mod tests {
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use pdf_bytes::{ByteStore, SourceId};
    use std::io::Write;

    use super::{
        DecodedStream, ImageCodec, StreamDecodeError, StreamDecodeErrorKind,
        decode_image_stream_bytes, decode_stream, decode_stream_bytes,
        decode_stream_bytes_recovering,
    };
    use crate::{ObjectKind, ParseLimits, parse_indirect_object_strict};

    fn flate(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(bytes).expect("encode fixture");
        encoder.finish().expect("finish fixture")
    }

    fn ascii_hex(bytes: &[u8]) -> Vec<u8> {
        const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
        let mut encoded = Vec::with_capacity(bytes.len() * 2 + 1);
        for &byte in bytes {
            encoded.push(DIGITS[usize::from(byte >> 4)]);
            encoded.push(DIGITS[usize::from(byte & 0x0f)]);
        }
        encoded.push(b'>');
        encoded
    }

    fn ascii85(bytes: &[u8]) -> Vec<u8> {
        let mut encoded = Vec::new();
        for chunk in bytes.chunks(4) {
            let mut padded = [0_u8; 4];
            padded[..chunk.len()].copy_from_slice(chunk);
            if chunk.len() == 4 && padded == [0; 4] {
                encoded.push(b'z');
                continue;
            }
            let mut value = u32::from_be_bytes(padded);
            let mut digits = [0_u8; 5];
            for digit in digits.iter_mut().rev() {
                *digit = u8::try_from(value % 85).expect("base-85 digit") + b'!';
                value /= 85;
            }
            let output_length = if chunk.len() == 4 { 5 } else { chunk.len() + 1 };
            encoded.extend_from_slice(&digits[..output_length]);
        }
        encoded.extend_from_slice(b"~>");
        encoded
    }

    fn pack_9_bit_codes(codes: &[u16]) -> Vec<u8> {
        let mut encoded = Vec::new();
        let mut accumulator = 0_u32;
        let mut bits = 0_usize;
        for &code in codes {
            accumulator = (accumulator << 9) | u32::from(code);
            bits += 9;
            while bits >= 8 {
                bits -= 8;
                encoded.push(u8::try_from(accumulator >> bits).expect("packed byte"));
                accumulator &= (1_u32 << bits) - 1;
            }
        }
        if bits > 0 {
            encoded.push(u8::try_from(accumulator << (8 - bits)).expect("final packed byte"));
        }
        encoded
    }

    fn stream_source(filter: &[u8], encoded: &[u8]) -> ByteStore {
        stream_source_with_parameters(filter, b"", encoded)
    }

    fn stream_source_with_parameters(
        filter: &[u8],
        decode_parameters: &[u8],
        encoded: &[u8],
    ) -> ByteStore {
        let mut bytes = format!(
            "1 0 obj\n<< /Length {} /Filter {} {} >>\nstream\n",
            encoded.len(),
            String::from_utf8_lossy(filter),
            String::from_utf8_lossy(decode_parameters)
        )
        .into_bytes();
        bytes.extend_from_slice(encoded);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        ByteStore::new(SourceId::new(41), bytes)
    }

    fn recovered(filter: &[u8], encoded: &[u8]) -> Result<DecodedStream, StreamDecodeError> {
        let source = stream_source(filter, encoded);
        let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect("valid encoded stream");
        let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
            panic!("stream dictionary");
        };
        let stream = object.stream().expect("stream envelope");
        let data = source.resolve(stream.data_span()).expect("stream data");
        decode_stream_bytes_recovering(&source, dictionary, data, stream.data_span().start(), 4096)
    }

    fn strict(filter: &[u8], encoded: &[u8]) -> Result<Vec<u8>, StreamDecodeError> {
        let source = stream_source(filter, encoded);
        let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect("valid encoded stream");
        let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
            panic!("stream dictionary");
        };
        let stream = object.stream().expect("stream envelope");
        let data = source.resolve(stream.data_span()).expect("stream data");
        decode_stream_bytes(&source, dictionary, data, stream.data_span().start(), 4096)
    }

    const SAMPLES: &[u8] = &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0xa0];

    #[test]
    fn a_filter_that_never_reaches_its_end_marker_still_refuses_the_strict_way() {
        for (filter, encoded) in [
            (&b"/ASCIIHexDecode"[..], trimmed(ascii_hex(SAMPLES), 1)),
            (&b"/ASCII85Decode"[..], trimmed(ascii85(SAMPLES), 2)),
            (&b"/RunLengthDecode"[..], trimmed(run_length(SAMPLES), 1)),
        ] {
            let error = strict(filter, &encoded)
                .expect_err("the strict door is exactly as strict as it was");
            assert_eq!(error.kind(), StreamDecodeErrorKind::MissingFilterEod);
        }
    }

    #[test]
    fn the_end_of_the_data_stands_in_for_an_end_marker_that_never_comes() {
        for (filter, encoded) in [
            (&b"/ASCIIHexDecode"[..], trimmed(ascii_hex(SAMPLES), 1)),
            (&b"/ASCII85Decode"[..], trimmed(ascii85(SAMPLES), 2)),
            (&b"/RunLengthDecode"[..], trimmed(run_length(SAMPLES), 1)),
        ] {
            let decoded = recovered(filter, &encoded).expect("the data ends, so the filter ends");
            assert_eq!(
                decoded.bytes,
                SAMPLES,
                "{} lost bytes it had",
                String::from_utf8_lossy(filter)
            );
            assert_eq!(
                decoded.repairs.len(),
                1,
                "the repair is reported, not swallowed"
            );
        }
    }

    #[test]
    fn an_odd_final_hex_digit_is_completed_with_a_zero_when_no_marker_arrives() {
        let encoded = trimmed(ascii_hex(SAMPLES), 2);
        let decoded = recovered(b"/ASCIIHexDecode", &encoded).expect("odd digit completes");
        assert_eq!(decoded.bytes, SAMPLES);
    }

    #[test]
    fn an_unfinished_base85_group_is_flushed_and_a_lone_character_is_dropped() {
        let flushed = recovered(b"/ASCII85Decode", &trimmed(ascii85(SAMPLES), 4))
            .expect("a group of three flushes what it holds");
        assert_eq!(flushed.bytes, &SAMPLES[..6]);

        let dropped = recovered(b"/ASCII85Decode", &trimmed(ascii85(SAMPLES), 6))
            .expect("a lone character is the end of the data, not an error");
        assert_eq!(dropped.bytes, &SAMPLES[..4]);
    }

    #[test]
    fn a_literal_run_longer_than_its_data_emits_the_bytes_it_actually_has() {
        let encoded = trimmed(run_length(SAMPLES), 4);
        let decoded = recovered(b"/RunLengthDecode", &encoded).expect("the run ends with the data");
        assert_eq!(decoded.bytes, &SAMPLES[..5]);
    }

    #[test]
    fn a_well_formed_stream_decodes_identically_through_either_door() {
        for (filter, encoded) in [
            (&b"/ASCIIHexDecode"[..], ascii_hex(SAMPLES)),
            (&b"/ASCII85Decode"[..], ascii85(SAMPLES)),
            (&b"/RunLengthDecode"[..], run_length(SAMPLES)),
            (&b"/FlateDecode"[..], flate(SAMPLES)),
        ] {
            let decoded = recovered(filter, &encoded).expect("well formed");
            assert_eq!(
                decoded.bytes,
                strict(filter, &encoded).expect("well formed")
            );
            assert!(
                decoded.repairs.is_empty(),
                "{} reported a repair it did not make",
                String::from_utf8_lossy(filter)
            );
        }
    }

    fn run_length(bytes: &[u8]) -> Vec<u8> {
        let mut encoded = Vec::new();
        for run in bytes.chunks(128) {
            encoded.push(u8::try_from(run.len() - 1).expect("run length"));
            encoded.extend_from_slice(run);
        }
        encoded.push(128);
        encoded
    }

    fn trimmed(mut encoded: Vec<u8>, count: usize) -> Vec<u8> {
        encoded.truncate(encoded.len() - count);
        encoded
    }

    #[test]
    fn decodes_flate_and_enforces_the_expansion_limit() {
        let encoded = flate(b"known answer");
        let source = stream_source(b"/FlateDecode", &encoded);
        let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect("valid encoded stream");
        let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
            panic!("stream dictionary");
        };
        let stream = object.stream().expect("stream envelope");

        assert_eq!(
            decode_stream(&source, dictionary, stream, 12),
            Ok(b"known answer".to_vec())
        );
        let error = decode_stream(&source, dictionary, stream, 11)
            .expect_err("known output is one byte over the limit");
        assert_eq!(error.kind(), StreamDecodeErrorKind::ExpansionLimit);
    }

    #[test]
    fn supports_abbreviated_filter_names_and_rejects_unknown_filters() {
        let encoded = flate(b"abc");
        let source = stream_source(b"/Fl", &encoded);
        let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect("valid encoded stream");
        let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
            panic!("stream dictionary");
        };
        assert_eq!(
            decode_stream(&source, dictionary, object.stream().expect("stream"), 3),
            Ok(b"abc".to_vec())
        );

        let source = stream_source(b"/DCTDecode", b"not jpeg");
        let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect("syntactically valid stream");
        let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
            panic!("stream dictionary");
        };
        let error = decode_stream(&source, dictionary, object.stream().expect("stream"), 100)
            .expect_err("unknown filter must be explicit");
        assert_eq!(error.kind(), StreamDecodeErrorKind::UnsupportedFilter);
    }

    #[test]
    fn image_pipeline_decodes_general_prefix_and_names_terminal_codec() {
        let jpeg = [0xff, 0xd8, 0xff, 0xd9];
        let encoded = ascii_hex(&jpeg);
        let source = stream_source_with_parameters(
            b"[/ASCIIHexDecode /DCTDecode]",
            b"/DecodeParms [null << /ColorTransform 0 >>]",
            &encoded,
        );
        let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect("image stream pipeline");
        let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
            panic!("stream dictionary");
        };
        let stream = object.stream().expect("stream envelope");
        let decoded = decode_image_stream_bytes(
            &source,
            dictionary,
            source.resolve(stream.data_span()).expect("encoded bytes"),
            stream.data_span().start(),
            jpeg.len(),
        )
        .expect("general prefix before image codec");

        assert_eq!(decoded.bytes, jpeg);
        assert_eq!(decoded.codec, Some(ImageCodec::Dct));
        assert_eq!(
            source
                .resolve(decoded.codec_span.expect("DCT filter span"))
                .expect("DCT filter source"),
            b"/DCTDecode"
        );
        let parameters = decoded.codec_parameters.expect("DCT parameters");
        assert_eq!(
            source
                .resolve(parameters.span())
                .expect("DCT parameters source"),
            b"<< /ColorTransform 0 >>"
        );

        let source = stream_source(b"[/DCTDecode /ASCIIHexDecode]", &jpeg);
        let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect("syntactically valid invalid-order pipeline");
        let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
            panic!("stream dictionary");
        };
        let stream = object.stream().expect("stream envelope");
        let error = decode_image_stream_bytes(
            &source,
            dictionary,
            source.resolve(stream.data_span()).expect("encoded bytes"),
            stream.data_span().start(),
            1024,
        )
        .expect_err("an image codec cannot feed a later general filter");
        assert_eq!(error.kind(), StreamDecodeErrorKind::UnsupportedFilter);
    }

    #[test]
    fn decodes_ascii_hex_and_ascii85_with_their_strict_end_markers() {
        let source = stream_source(b"/AHx", b"61 6\n>");
        let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect("valid ASCII hexadecimal stream");
        let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
            panic!("stream dictionary");
        };
        assert_eq!(
            decode_stream(&source, dictionary, object.stream().expect("stream"), 2),
            Ok(vec![b'a', b'`'])
        );

        let expected = b"\0\0\0\0Thai PDF!";
        let encoded = ascii85(expected);
        let source = stream_source(b"/A85", &encoded);
        let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect("valid ASCII base-85 stream");
        let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
            panic!("stream dictionary");
        };
        assert_eq!(
            decode_stream(
                &source,
                dictionary,
                object.stream().expect("stream"),
                expected.len()
            ),
            Ok(expected.to_vec())
        );

        let source = stream_source(b"/ASCII85Decode", b"9jqo^BlbD-BleB1DJ+*+F(f,q~>");
        let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect("published base-85 known answer");
        let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
            panic!("stream dictionary");
        };
        assert_eq!(
            decode_stream(&source, dictionary, object.stream().expect("stream"), 20),
            Ok(b"Man is distinguished".to_vec())
        );
    }

    #[test]
    fn applies_filter_arrays_and_aligned_decode_parameter_arrays_in_order() {
        let expected = b"filter pipeline known answer";
        let encoded = ascii_hex(&flate(expected));
        let source = stream_source_with_parameters(
            b"[/ASCIIHexDecode /FlateDecode]",
            b"/DecodeParms [null null]",
            &encoded,
        );
        let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect("valid filtered stream");
        let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
            panic!("stream dictionary");
        };

        assert_eq!(
            decode_stream(&source, dictionary, object.stream().expect("stream"), 1024),
            Ok(expected.to_vec())
        );
    }

    #[test]
    fn decodes_run_length_data_and_enforces_its_record_boundaries() {
        let source = stream_source(b"/RL", &[2, b'a', b'b', b'c', 253, b'x', 128]);
        let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect("valid run-length stream");
        let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
            panic!("stream dictionary");
        };
        assert_eq!(
            decode_stream(&source, dictionary, object.stream().expect("stream"), 7),
            Ok(b"abcxxxx".to_vec())
        );

        for encoded in [&[2, b'a', 128][..], &[255][..]] {
            let source = stream_source(b"/RunLengthDecode", encoded);
            let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
                .expect("syntactically valid stream");
            let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
                panic!("stream dictionary");
            };
            let error = decode_stream(&source, dictionary, object.stream().expect("stream"), 100)
                .expect_err("truncated run must fail");
            assert_eq!(error.kind(), StreamDecodeErrorKind::MalformedRunLengthData);
        }
    }

    #[test]
    fn decodes_the_pdf_reference_lzw_example_and_checks_end_padding() {
        let encoded = [0x80, 0x0b, 0x60, 0x50, 0x22, 0x0c, 0x0c, 0x85, 0x01];
        let source = stream_source(b"/LZWDecode", &encoded);
        let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect("PDF reference LZW example");
        let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
            panic!("stream dictionary");
        };
        assert_eq!(
            decode_stream(&source, dictionary, object.stream().expect("stream"), 10),
            Ok(b"-----A---B".to_vec())
        );

        let mut bad_padding = pack_9_bit_codes(&[256, 65, 257]);
        *bad_padding.last_mut().expect("packed data") |= 1;
        let source = stream_source(b"/LZWDecode", &bad_padding);
        let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect("syntactically valid stream");
        let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
            panic!("stream dictionary");
        };
        let error = decode_stream(&source, dictionary, object.stream().expect("stream"), 10)
            .expect_err("non-zero bits after EOD must fail");
        assert_eq!(error.kind(), StreamDecodeErrorKind::MalformedLzwData);

        let missing_eod = pack_9_bit_codes(&[256, 65]);
        let source = stream_source(b"/LZWDecode", &missing_eod);
        let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect("syntactically valid stream");
        let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
            panic!("stream dictionary");
        };
        let error = decode_stream(&source, dictionary, object.stream().expect("stream"), 10)
            .expect_err("LZW stream without EOD must fail");
        assert_eq!(error.kind(), StreamDecodeErrorKind::MissingFilterEod);
    }

    #[test]
    fn rejects_invalid_lzw_early_change() {
        let encoded = pack_9_bit_codes(&[256, 65, 257]);
        let source = stream_source_with_parameters(
            b"/LZWDecode",
            b"/DecodeParms << /EarlyChange 2 >>",
            &encoded,
        );
        let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect("syntactically valid stream");
        let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
            panic!("stream dictionary");
        };
        let error = decode_stream(&source, dictionary, object.stream().expect("stream"), 10)
            .expect_err("EarlyChange must be zero or one");
        assert_eq!(error.kind(), StreamDecodeErrorKind::InvalidDecodeParameters);
    }

    #[test]
    fn rejects_malformed_ascii_data_trailing_bytes_and_misaligned_parameters() {
        for (filter, parameters, encoded, expected) in [
            (
                &b"/ASCIIHexDecode"[..],
                &b""[..],
                &b"GG>"[..],
                StreamDecodeErrorKind::MalformedAsciiHexData,
            ),
            (
                &b"/ASCIIHexDecode"[..],
                &b""[..],
                &b"61"[..],
                StreamDecodeErrorKind::MissingFilterEod,
            ),
            (
                &b"/ASCII85Decode"[..],
                &b""[..],
                &b"!~>"[..],
                StreamDecodeErrorKind::MalformedAscii85Data,
            ),
            (
                &b"/ASCII85Decode"[..],
                &b""[..],
                &b"uuuuu~>"[..],
                StreamDecodeErrorKind::MalformedAscii85Data,
            ),
            (
                &b"/ASCIIHexDecode"[..],
                &b""[..],
                &b"61>00"[..],
                StreamDecodeErrorKind::TrailingFilterData,
            ),
            (
                &b"[/ASCIIHexDecode /FlateDecode]"[..],
                &b"/DecodeParms [null]"[..],
                &b">"[..],
                StreamDecodeErrorKind::InvalidDecodeParameters,
            ),
        ] {
            let source = stream_source_with_parameters(filter, parameters, encoded);
            let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
                .expect("syntactically valid stream");
            let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
                panic!("stream dictionary");
            };
            let error = decode_stream(&source, dictionary, object.stream().expect("stream"), 100)
                .expect_err("malformed filter input must fail");
            assert_eq!(error.kind(), expected);
        }
    }

    #[test]
    fn decodes_every_png_predictor_row_algorithm() {
        let predicted = [
            0, 10, 20, 30, 40, 1, 10, 10, 10, 10, 2, 0, 0, 0, 0, 3, 5, 5, 5, 5, 4, 0, 0, 0, 0,
        ];
        let encoded = flate(&predicted);
        let source = stream_source_with_parameters(
            b"/FlateDecode",
            b"/DecodeParms << /Columns 4 /Predictor 12 >>",
            &encoded,
        );
        let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect("valid PNG-predicted stream");
        let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
            panic!("stream dictionary");
        };
        let expected = [
            10, 20, 30, 40, 10, 20, 30, 40, 10, 20, 30, 40, 10, 20, 30, 40, 10, 20, 30, 40,
        ];

        assert_eq!(
            decode_stream(
                &source,
                dictionary,
                object.stream().expect("stream"),
                expected.len()
            ),
            Ok(expected.to_vec())
        );
        let error = decode_stream(
            &source,
            dictionary,
            object.stream().expect("stream"),
            expected.len() - 1,
        )
        .expect_err("predictor tag overhead must not bypass the decoded limit");
        assert_eq!(error.kind(), StreamDecodeErrorKind::ExpansionLimit);
    }

    #[test]
    fn decodes_tiff_predictor_at_every_pdf_component_width() {
        let cases = [
            (8, 1, 1, &b"\x5d"[..], &b"\x69"[..]),
            (4, 1, 2, &b"\x1b"[..], &b"\x1e"[..]),
            (
                8,
                2,
                4,
                &b"\xaa\xf1\xf1\xf1\xe2\x00\xb0\x78"[..],
                &b"\xaa\x9b\x8c\x7d\x5f\x5f\x0f\x77"[..],
            ),
            (
                16,
                1,
                8,
                &b"\x01\x01\x01\x01\x01\x01\x01\x01\x01\x01\x01\x01\x01\x01\x01\x01\
                    \x01\x01\x01\x01\x01\x01\x01\x01\x01\x01\x01\x01\x01\x01\x01\x01\
                    \x57\x01\xff\xff\xff\xff\xff\xff\xff\xf6\xf6\xf6\x00\x01\x02\x01"[..],
                &b"\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\x0e\x0f\x10\
                    \x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\x0e\x0f\x10\
                    \x57\x58\x57\x56\x55\x54\x53\x52\x51\x47\x3d\x33\x33\x34\x36\x37"[..],
            ),
            (
                4,
                1,
                16,
                &b"\x55\x55\xcd\xf0\x64\x20\x39\x5b"[..],
                &b"\x55\x55\x23\x45\x87\x65\xc0\xc0"[..],
            ),
        ];

        for (columns, colors, bits, predicted, expected) in cases {
            let encoded = flate(predicted);
            let parameters = format!(
                "/DecodeParms << /Predictor 2 /Columns {columns} /Colors {colors} /BitsPerComponent {bits} >>"
            );
            let source =
                stream_source_with_parameters(b"/FlateDecode", parameters.as_bytes(), &encoded);
            let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
                .expect("valid TIFF-predicted stream");
            let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
                panic!("stream dictionary");
            };
            assert_eq!(
                decode_stream(
                    &source,
                    dictionary,
                    object.stream().expect("stream"),
                    expected.len()
                ),
                Ok(expected.to_vec())
            );
        }
    }

    #[test]
    fn canonicalizes_tiff_row_padding_and_rejects_partial_rows() {
        let encoded = flate(b"\xff");
        let source = stream_source_with_parameters(
            b"/FlateDecode",
            b"/DecodeParms << /Predictor 2 /Columns 3 /BitsPerComponent 1 >>",
            &encoded,
        );
        let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect("valid bit-packed TIFF predictor row");
        let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
            panic!("stream dictionary");
        };
        assert_eq!(
            decode_stream(&source, dictionary, object.stream().expect("stream"), 1),
            Ok(vec![0xa0])
        );

        let encoded = flate(b"\x01\x01\x01");
        let source = stream_source_with_parameters(
            b"/FlateDecode",
            b"/DecodeParms << /Predictor 2 /Columns 2 /BitsPerComponent 16 >>",
            &encoded,
        );
        let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect("syntactically valid stream");
        let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
            panic!("stream dictionary");
        };
        let error = decode_stream(&source, dictionary, object.stream().expect("stream"), 3)
            .expect_err("partial predictor row must not be silently padded");
        assert_eq!(error.kind(), StreamDecodeErrorKind::MalformedPredictorData);
    }

    #[test]
    fn applies_tiff_predictor_after_lzw() {
        let encoded = pack_9_bit_codes(&[256, 1, 1, 1, 1, 257]);
        let source = stream_source_with_parameters(
            b"/LZWDecode",
            b"/DecodeParms << /Predictor 2 /Columns 4 >>",
            &encoded,
        );
        let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect("valid LZW stream with TIFF predictor");
        let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
            panic!("stream dictionary");
        };
        assert_eq!(
            decode_stream(&source, dictionary, object.stream().expect("stream"), 4),
            Ok(vec![1, 2, 3, 4])
        );
    }

    #[test]
    fn rejects_malformed_or_unsupported_predictors() {
        for (parameters, predicted, expected) in [
            (
                &b"/DecodeParms << /Columns 4 /Predictor 12 >>"[..],
                &b"\x05\x00\x00\x00\x00"[..],
                StreamDecodeErrorKind::MalformedPredictorData,
            ),
            (
                &b"/DecodeParms << /Columns 4 /Predictor 3 >>"[..],
                &b"\x00\x00\x00\x00"[..],
                StreamDecodeErrorKind::UnsupportedDecodeParameters,
            ),
            (
                &b"/DecodeParms << /Columns 0 /Predictor 12 >>"[..],
                &b""[..],
                StreamDecodeErrorKind::InvalidDecodeParameters,
            ),
        ] {
            let encoded = flate(predicted);
            let source = stream_source_with_parameters(b"/FlateDecode", parameters, &encoded);
            let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
                .expect("syntactically valid stream");
            let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
                panic!("stream dictionary");
            };

            let error = decode_stream(&source, dictionary, object.stream().expect("stream"), 100)
                .expect_err("invalid predictor must fail explicitly");
            assert_eq!(error.kind(), expected);
        }
    }
}
