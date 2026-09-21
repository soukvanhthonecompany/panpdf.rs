use pdf_bytes::{ByteStore, SourceSpan};

use crate::filter::{decode_stream, decode_xref_stream_prefix};
use crate::indirect::parse_indirect_stream_prefix_strict;
use crate::value::{name_object_equals, parse_unsigned};
use crate::{
    DictionaryEntry, IndirectObject, IndirectObjectErrorKind, NumberKind, Object, ObjectKind,
    Reference, ResolvedStreamLength, XrefEntry, XrefEntryKind, XrefError, XrefErrorKind,
    XrefLimits, parse_indirect_object_strict, parse_indirect_object_with_resolved_length_strict,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BootstrapStreamLength {
    reference: Reference,
    derived_value: usize,
    source_offset: usize,
}

impl BootstrapStreamLength {
    pub(crate) const fn reference(self) -> Reference {
        self.reference
    }

    pub(crate) const fn derived_value(self) -> usize {
        self.derived_value
    }

    pub(crate) const fn source_offset(self) -> usize {
        self.source_offset
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XrefStreamSection {
    byte_offset: usize,
    object: IndirectObject,
    entries: Vec<XrefEntry>,
    previous_byte_offset: Option<usize>,
    bootstrap_length: Option<BootstrapStreamLength>,
}

impl XrefStreamSection {
    #[must_use]
    pub const fn byte_offset(&self) -> usize {
        self.byte_offset
    }

    #[must_use]
    pub fn entries(&self) -> &[XrefEntry] {
        &self.entries
    }

    #[must_use]
    pub const fn trailer(&self) -> &Object {
        self.object.value()
    }

    #[must_use]
    pub const fn previous_byte_offset(&self) -> Option<usize> {
        self.previous_byte_offset
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.object.span()
    }

    #[must_use]
    pub const fn object(&self) -> &IndirectObject {
        &self.object
    }

    pub(crate) const fn bootstrap_length(&self) -> Option<BootstrapStreamLength> {
        self.bootstrap_length
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct XrefStreamParameters {
    widths: [usize; 3],
    ranges: Vec<(u32, usize)>,
    previous_byte_offset: Option<usize>,
}

pub(crate) fn parse_xref_stream_section_strict(
    source: &ByteStore,
    offset: usize,
    limits: XrefLimits,
) -> Result<XrefStreamSection, XrefError> {
    let (object, bootstrap_length) =
        match parse_indirect_object_strict(source, offset, limits.objects) {
            Ok(object) => (object, None),
            Err(error) => match error.kind() {
                IndirectObjectErrorKind::UnresolvedStreamLength(reference) => {
                    bootstrap_indirect_length(source, offset, limits, reference)?
                }
                _ => return Err(error.into()),
            },
        };
    if object.reference().generation() != 0 {
        return Err(XrefError::new(offset, XrefErrorKind::XrefStreamWrongType));
    }
    let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
        return Err(XrefError::new(
            object.value().span().start(),
            XrefErrorKind::XrefStreamNotDictionary,
        ));
    };
    let stream = object.stream().ok_or_else(|| {
        XrefError::new(
            object.value().span().end(),
            XrefErrorKind::XrefStreamMissingData,
        )
    })?;

    let parameters = parse_parameters(source, object.value(), limits)?;

    let decoded = decode_stream(source, dictionary, stream, limits.max_decoded_stream_bytes)?;
    let entries = parse_entries(
        &decoded,
        stream.data_span(),
        &parameters.ranges,
        parameters.widths,
    )?;

    Ok(XrefStreamSection {
        byte_offset: offset,
        object,
        entries,
        previous_byte_offset: parameters.previous_byte_offset,
        bootstrap_length,
    })
}

fn bootstrap_indirect_length(
    source: &ByteStore,
    offset: usize,
    limits: XrefLimits,
    expected_reference: Reference,
) -> Result<(IndirectObject, Option<BootstrapStreamLength>), XrefError> {
    let prefix = parse_indirect_stream_prefix_strict(source, offset, limits.objects)?;
    if prefix.length_reference() != expected_reference {
        return Err(XrefError::new(
            prefix.length_offset(),
            XrefErrorKind::IndirectXrefLengthUnresolvable,
        ));
    }
    if prefix.reference().generation() != 0 {
        return Err(XrefError::new(offset, XrefErrorKind::XrefStreamWrongType));
    }
    let ObjectKind::Dictionary(dictionary) = prefix.value().kind() else {
        return Err(XrefError::new(
            prefix.value().span().start(),
            XrefErrorKind::XrefStreamNotDictionary,
        ));
    };
    let parameters = parse_parameters(source, prefix.value(), limits)?;
    let expected_decoded_length = expected_decoded_length(&parameters)?;
    let decoded = decode_xref_stream_prefix(
        source,
        dictionary,
        prefix.data_start(),
        expected_decoded_length,
        limits.max_decoded_stream_bytes,
    )?;
    if decoded.decoded().len() != expected_decoded_length {
        return Err(XrefError::new(
            prefix.data_start(),
            XrefErrorKind::DecodedXrefLengthMismatch,
        ));
    }

    let derived_value = decoded.encoded_length();
    let object = parse_indirect_object_with_resolved_length_strict(
        source,
        offset,
        limits.objects,
        Some(ResolvedStreamLength::new(expected_reference, derived_value)),
    )?;
    Ok((
        object,
        Some(BootstrapStreamLength {
            reference: expected_reference,
            derived_value,
            source_offset: prefix.length_offset(),
        }),
    ))
}

fn parse_parameters(
    source: &ByteStore,
    value: &Object,
    limits: XrefLimits,
) -> Result<XrefStreamParameters, XrefError> {
    let ObjectKind::Dictionary(dictionary) = value.kind() else {
        return Err(XrefError::new(
            value.span().start(),
            XrefErrorKind::XrefStreamNotDictionary,
        ));
    };
    let type_value = required_unique(
        source,
        dictionary,
        b"/Type",
        XrefErrorKind::XrefStreamWrongType,
    )?;
    if !name_object_equals(source, type_value, b"/XRef") {
        return Err(XrefError::new(
            type_value.span().start(),
            XrefErrorKind::XrefStreamWrongType,
        ));
    }
    let size_value = required_unique(source, dictionary, b"/Size", XrefErrorKind::MissingXrefSize)?;
    let size = integer_usize(source, size_value)
        .filter(|value| *value > 0)
        .ok_or_else(|| XrefError::new(size_value.span().start(), XrefErrorKind::InvalidXrefSize))?;
    let widths_value =
        required_unique(source, dictionary, b"/W", XrefErrorKind::MissingXrefWidths)?;
    let widths = parse_widths(source, widths_value)?;
    let ranges = parse_index(source, dictionary, size, limits.max_entries)?;
    let previous_byte_offset = optional_integer(source, dictionary, b"/Prev")?;
    if previous_byte_offset.is_some_and(|previous| previous >= source.len()) {
        return Err(XrefError::new(
            value.span().start(),
            XrefErrorKind::PreviousOffsetOutOfBounds,
        ));
    }
    Ok(XrefStreamParameters {
        widths,
        ranges,
        previous_byte_offset,
    })
}

fn expected_decoded_length(parameters: &XrefStreamParameters) -> Result<usize, XrefError> {
    let entry_count = parameters
        .ranges
        .iter()
        .try_fold(0_usize, |total, (_, count)| total.checked_add(*count));
    entry_count
        .and_then(|count| count.checked_mul(parameters.widths.iter().sum()))
        .ok_or_else(|| XrefError::new(0, XrefErrorKind::EntryLimit))
}

fn required_unique<'a>(
    source: &ByteStore,
    dictionary: &'a [DictionaryEntry],
    key: &[u8],
    missing: XrefErrorKind,
) -> Result<&'a Object, XrefError> {
    optional_unique(source, dictionary, key)?.ok_or_else(|| {
        XrefError::new(
            dictionary
                .first()
                .map_or(0, |entry| entry.key().span().start()),
            missing,
        )
    })
}

fn optional_unique<'a>(
    source: &ByteStore,
    dictionary: &'a [DictionaryEntry],
    key: &[u8],
) -> Result<Option<&'a Object>, XrefError> {
    let mut matches = dictionary
        .iter()
        .filter(|entry| entry.key_equals(source, key));
    let Some(first) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err(XrefError::new(
            first.key().span().start(),
            XrefErrorKind::DuplicateXrefParameter,
        ));
    }
    Ok(Some(first.value()))
}

fn integer_usize(source: &ByteStore, object: &Object) -> Option<usize> {
    if !matches!(object.kind(), ObjectKind::Number(NumberKind::Integer)) {
        return None;
    }
    source
        .resolve(object.span())
        .ok()
        .and_then(parse_unsigned)
        .and_then(|value| usize::try_from(value).ok())
}

fn parse_widths(source: &ByteStore, object: &Object) -> Result<[usize; 3], XrefError> {
    let ObjectKind::Array(values) = object.kind() else {
        return Err(XrefError::new(
            object.span().start(),
            XrefErrorKind::InvalidXrefWidths,
        ));
    };
    let widths: [usize; 3] = values
        .iter()
        .map(|value| integer_usize(source, value))
        .collect::<Option<Vec<_>>>()
        .and_then(|values| values.try_into().ok())
        .filter(|widths: &[usize; 3]| widths.iter().all(|width| *width <= 8))
        .ok_or_else(|| XrefError::new(object.span().start(), XrefErrorKind::InvalidXrefWidths))?;
    if widths.iter().sum::<usize>() == 0 {
        return Err(XrefError::new(
            object.span().start(),
            XrefErrorKind::InvalidXrefWidths,
        ));
    }
    Ok(widths)
}

fn parse_index(
    source: &ByteStore,
    dictionary: &[DictionaryEntry],
    size: usize,
    max_entries: usize,
) -> Result<Vec<(u32, usize)>, XrefError> {
    let Some(index) = optional_unique(source, dictionary, b"/Index")? else {
        u32::try_from(size).map_err(|_| XrefError::new(0, XrefErrorKind::InvalidXrefSize))?;
        if size > max_entries {
            return Err(XrefError::new(0, XrefErrorKind::EntryLimit));
        }
        return Ok(vec![(0, size)]);
    };
    let ObjectKind::Array(values) = index.kind() else {
        return Err(XrefError::new(
            index.span().start(),
            XrefErrorKind::InvalidXrefIndex,
        ));
    };
    if values.is_empty() || values.len() % 2 != 0 {
        return Err(XrefError::new(
            index.span().start(),
            XrefErrorKind::InvalidXrefIndex,
        ));
    }

    let mut ranges = Vec::with_capacity(values.len() / 2);
    let mut total = 0_usize;
    for pair in values.chunks_exact(2) {
        let first = integer_usize(source, &pair[0])
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| {
                XrefError::new(pair[0].span().start(), XrefErrorKind::InvalidXrefIndex)
            })?;
        let count = integer_usize(source, &pair[1]).ok_or_else(|| {
            XrefError::new(pair[1].span().start(), XrefErrorKind::InvalidXrefIndex)
        })?;
        let end = usize::try_from(first)
            .ok()
            .and_then(|first| first.checked_add(count))
            .filter(|end| *end <= size)
            .ok_or_else(|| {
                XrefError::new(pair[0].span().start(), XrefErrorKind::InvalidXrefIndex)
            })?;
        let _ = end;
        total = total
            .checked_add(count)
            .ok_or_else(|| XrefError::new(pair[1].span().start(), XrefErrorKind::EntryLimit))?;
        if total > max_entries {
            return Err(XrefError::new(
                pair[1].span().start(),
                XrefErrorKind::EntryLimit,
            ));
        }
        ranges.push((first, count));
    }
    Ok(ranges)
}

fn optional_integer(
    source: &ByteStore,
    dictionary: &[DictionaryEntry],
    key: &[u8],
) -> Result<Option<usize>, XrefError> {
    optional_unique(source, dictionary, key)?
        .map(|value| {
            integer_usize(source, value).ok_or_else(|| {
                XrefError::new(value.span().start(), XrefErrorKind::InvalidPreviousOffset)
            })
        })
        .transpose()
}

fn parse_entries(
    decoded: &[u8],
    encoded_span: SourceSpan,
    ranges: &[(u32, usize)],
    widths: [usize; 3],
) -> Result<Vec<XrefEntry>, XrefError> {
    let record_width = widths.iter().sum::<usize>();
    let entry_count = ranges
        .iter()
        .try_fold(0_usize, |total, (_, count)| total.checked_add(*count));
    let expected_length = entry_count.and_then(|count| count.checked_mul(record_width));
    if expected_length != Some(decoded.len()) {
        return Err(XrefError::new(
            encoded_span.start(),
            XrefErrorKind::DecodedXrefLengthMismatch,
        ));
    }

    let mut entries = Vec::with_capacity(entry_count.unwrap_or(0));
    let mut cursor = 0_usize;
    for &(first, count) in ranges {
        for index in 0..count {
            let object_number = first
                .checked_add(u32::try_from(index).map_err(|_| {
                    XrefError::new(encoded_span.start(), XrefErrorKind::ObjectNumberOverflow)
                })?)
                .ok_or_else(|| {
                    XrefError::new(encoded_span.start(), XrefErrorKind::ObjectNumberOverflow)
                })?;
            let entry_offset = cursor;
            let entry_type = if widths[0] == 0 {
                1
            } else {
                read_field(decoded, &mut cursor, widths[0])
            };
            let second = read_field(decoded, &mut cursor, widths[1]);
            let third = read_field(decoded, &mut cursor, widths[2]);
            let (generation, kind) = parse_entry_kind(entry_type, second, third, encoded_span)?;
            entries.push(XrefEntry::from_stream(
                object_number,
                generation,
                kind,
                encoded_span,
                entry_offset,
            ));
        }
    }
    Ok(entries)
}

fn read_field(decoded: &[u8], cursor: &mut usize, width: usize) -> u64 {
    let mut value = 0_u64;
    for &byte in &decoded[*cursor..*cursor + width] {
        value = (value << 8) | u64::from(byte);
    }
    *cursor += width;
    value
}

fn parse_entry_kind(
    entry_type: u64,
    second: u64,
    third: u64,
    span: SourceSpan,
) -> Result<(u16, XrefEntryKind), XrefError> {
    match entry_type {
        0 => Ok((
            u16::try_from(third)
                .map_err(|_| XrefError::new(span.start(), XrefErrorKind::InvalidEntry))?,
            XrefEntryKind::Free {
                next_free_object: u32::try_from(second)
                    .map_err(|_| XrefError::new(span.start(), XrefErrorKind::InvalidEntry))?,
            },
        )),
        1 => Ok((
            u16::try_from(third)
                .map_err(|_| XrefError::new(span.start(), XrefErrorKind::InvalidEntry))?,
            XrefEntryKind::InUse {
                byte_offset: second,
            },
        )),
        2 => Ok((
            0,
            XrefEntryKind::Compressed {
                object_stream_number: u32::try_from(second)
                    .map_err(|_| XrefError::new(span.start(), XrefErrorKind::InvalidEntry))?,
                index: u32::try_from(third)
                    .map_err(|_| XrefError::new(span.start(), XrefErrorKind::InvalidEntry))?,
            },
        )),
        _ => Err(XrefError::new(
            span.start(),
            XrefErrorKind::UnsupportedXrefEntryType,
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::Arc;

    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use pdf_bytes::{ByteStore, SourceId};

    use crate::{
        XrefEntryKind, XrefErrorKind, XrefLimits, XrefSection, parse_revision_chain_strict,
    };

    fn store(bytes: Vec<u8>) -> ByteStore {
        ByteStore::new(SourceId::new(51), Arc::<[u8]>::from(bytes))
    }

    fn encode_entry(kind: u8, second: u32, third: u16) -> [u8; 7] {
        let second = second.to_be_bytes();
        let third = third.to_be_bytes();
        [
            kind, second[0], second[1], second[2], second[3], third[0], third[1],
        ]
    }

    #[derive(Clone, Copy)]
    enum FixtureEncoding {
        Raw,
        Flate,
        AsciiHexFlate,
        Ascii85Flate,
        Lzw,
        RunLength,
    }

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

    fn run_length(bytes: &[u8]) -> Vec<u8> {
        let mut encoded = Vec::new();
        for chunk in bytes.chunks(128) {
            encoded.push(u8::try_from(chunk.len() - 1).expect("literal run length"));
            encoded.extend_from_slice(chunk);
        }
        encoded.push(128);
        encoded
    }

    fn lzw_literals(bytes: &[u8]) -> Vec<u8> {
        let mut codes = Vec::with_capacity(bytes.len() + 2);
        codes.push(256_u16);
        codes.extend(bytes.iter().copied().map(u16::from));
        codes.push(257);

        let mut encoded = Vec::new();
        let mut accumulator = 0_u32;
        let mut bits = 0_usize;
        for code in codes {
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

    fn encode_fixture_stream(decoded: &[u8], encoding: FixtureEncoding) -> (&'static str, Vec<u8>) {
        match encoding {
            FixtureEncoding::Raw => ("", decoded.to_vec()),
            FixtureEncoding::Flate => (" /Filter /FlateDecode", flate(decoded)),
            FixtureEncoding::AsciiHexFlate => (
                " /Filter [/ASCIIHexDecode /FlateDecode]",
                ascii_hex(&flate(decoded)),
            ),
            FixtureEncoding::Ascii85Flate => (
                " /Filter [/ASCII85Decode /FlateDecode]",
                ascii85(&flate(decoded)),
            ),
            FixtureEncoding::Lzw => (" /Filter /LZWDecode", lzw_literals(decoded)),
            FixtureEncoding::RunLength => (" /Filter /RunLengthDecode", run_length(decoded)),
        }
    }

    fn xref_stream_pdf(compressed: bool) -> (Vec<u8>, usize) {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let catalog = bytes.len();
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog >>\nendobj\n");
        let xref = bytes.len();
        let mut decoded = Vec::new();
        decoded.extend_from_slice(&encode_entry(0, 0, u16::MAX));
        decoded.extend_from_slice(&encode_entry(1, u32::try_from(catalog).unwrap(), 0));
        decoded.extend_from_slice(&encode_entry(1, u32::try_from(xref).unwrap(), 0));
        decoded.extend_from_slice(&encode_entry(2, 9, 4));

        let (filter, data) = if compressed {
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
            encoder.write_all(&decoded).expect("encode fixture");
            (
                " /Filter /FlateDecode",
                encoder.finish().expect("finish fixture"),
            )
        } else {
            ("", decoded)
        };
        bytes.extend_from_slice(
            format!(
                "2 0 obj\n<< /Type /XRef /Size 4 /W [1 4 2] /Length {}{} >>\nstream\n",
                data.len(),
                filter
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(&data);
        bytes.extend_from_slice(b"\nendstream\nendobj\nstartxref\n");
        bytes.extend_from_slice(xref.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        (bytes, xref)
    }

    fn indirect_length_xref_stream_pdf(
        encoding: FixtureEncoding,
        declared_length_delta: usize,
    ) -> (Vec<u8>, usize) {
        let mut declared_length = 0_usize;
        for _ in 0..16 {
            let mut bytes = b"%PDF-1.7\n".to_vec();
            let catalog = bytes.len();
            bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog >>\nendobj\n");
            let length_object = bytes.len();
            bytes.extend_from_slice(format!("2 0 obj\n{declared_length}\nendobj\n").as_bytes());
            let xref = bytes.len();

            let mut decoded = Vec::new();
            decoded.extend_from_slice(&encode_entry(0, 0, u16::MAX));
            decoded.extend_from_slice(&encode_entry(1, u32::try_from(catalog).unwrap(), 0));
            decoded.extend_from_slice(&encode_entry(1, u32::try_from(length_object).unwrap(), 0));
            decoded.extend_from_slice(&encode_entry(1, u32::try_from(xref).unwrap(), 0));
            let (filter, data) = encode_fixture_stream(&decoded, encoding);
            let next_declared_length = data
                .len()
                .checked_add(declared_length_delta)
                .expect("fixture length");
            if next_declared_length != declared_length {
                declared_length = next_declared_length;
                continue;
            }

            bytes.extend_from_slice(
                format!(
                    "3 0 obj\n<< /Type /XRef /Size 4 /W [1 4 2] /Length 2 0 R{filter} >>\nstream\n"
                )
                .as_bytes(),
            );
            bytes.extend_from_slice(&data);
            bytes.extend_from_slice(b"\nendstream\nendobj\nstartxref\n");
            bytes.extend_from_slice(xref.to_string().as_bytes());
            bytes.extend_from_slice(b"\n%%EOF\n");
            return (bytes, xref);
        }
        panic!("indirect-length fixture did not converge");
    }

    #[test]
    fn parses_raw_and_flate_xref_streams_with_compressed_entries() {
        for compressed in [false, true] {
            let (bytes, xref) = xref_stream_pdf(compressed);
            let chain = parse_revision_chain_strict(&store(bytes), XrefLimits::default())
                .expect("valid xref stream");
            assert_eq!(chain.startxref(), xref);
            let XrefSection::Stream(section) = &chain.revisions()[0] else {
                panic!("stream revision");
            };
            assert_eq!(section.entries().len(), 4);
            assert_eq!(
                section.entries()[3].kind(),
                XrefEntryKind::Compressed {
                    object_stream_number: 9,
                    index: 4
                }
            );
            assert_eq!(section.entries()[3].decoded_byte_offset(), Some(21));
        }
    }

    #[test]
    fn resolves_indirect_xref_lengths_through_every_supported_filter_pipeline() {
        for encoding in [
            FixtureEncoding::Raw,
            FixtureEncoding::Flate,
            FixtureEncoding::AsciiHexFlate,
            FixtureEncoding::Ascii85Flate,
            FixtureEncoding::Lzw,
            FixtureEncoding::RunLength,
        ] {
            let (bytes, xref) = indirect_length_xref_stream_pdf(encoding, 0);
            let chain = parse_revision_chain_strict(&store(bytes), XrefLimits::default())
                .expect("indirect Length is selected from the parsed revision");

            assert_eq!(chain.startxref(), xref);
            let XrefSection::Stream(section) = &chain.revisions()[0] else {
                panic!("stream revision");
            };
            assert_eq!(section.entries().len(), 4);
        }
    }

    #[test]
    fn rejects_an_indirect_xref_stream_length_that_disagrees_with_the_encoded_data() {
        let (bytes, _) = indirect_length_xref_stream_pdf(FixtureEncoding::Flate, 1);
        let error = parse_revision_chain_strict(&store(bytes), XrefLimits::default())
            .expect_err("resolved Length must confirm the derived zlib boundary");

        assert_eq!(error.kind(), XrefErrorKind::IndirectXrefLengthMismatch);
    }

    #[test]
    fn validates_an_older_xref_length_in_its_own_revision_context() {
        let (mut bytes, older_xref) = indirect_length_xref_stream_pdf(FixtureEncoding::Raw, 0);
        let replacement = bytes.len();
        bytes.extend_from_slice(b"2 0 obj\n999\nendobj\n");
        let newest_xref = bytes.len();
        bytes.extend_from_slice(b"xref\n2 1\n");
        bytes.extend_from_slice(format!("{replacement:010} 00000 n \n").as_bytes());
        bytes.extend_from_slice(
            format!("trailer\n<< /Size 4 /Prev {older_xref} >>\nstartxref\n{newest_xref}\n%%EOF\n")
                .as_bytes(),
        );

        let chain = parse_revision_chain_strict(&store(bytes), XrefLimits::default())
            .expect("a newer replacement must not redefine an older revision's Length");
        assert_eq!(chain.revisions().len(), 2);
        assert!(matches!(chain.revisions()[0], XrefSection::Classic(_)));
        assert!(matches!(chain.revisions()[1], XrefSection::Stream(_)));
    }

    #[test]
    fn rejects_unknown_entry_types_and_decoded_length_disagreement() {
        let (bytes, _) = xref_stream_pdf(false);
        let position = bytes
            .windows(7)
            .position(|window| window == encode_entry(2, 9, 4))
            .expect("known entry bytes");
        let mut unknown = bytes.clone();
        unknown[position] = 3;
        let error = parse_revision_chain_strict(&store(unknown), XrefLimits::default())
            .expect_err("unknown entry type must fail");
        assert_eq!(error.kind(), XrefErrorKind::UnsupportedXrefEntryType);

        let mut wrong = bytes;
        let width_position = wrong
            .windows(b"/W [1 4 2]".len())
            .position(|window| window == b"/W [1 4 2]")
            .expect("known W array");
        wrong[width_position + b"/W [1 4 ".len()] = b'1';
        let error = parse_revision_chain_strict(&store(wrong), XrefLimits::default())
            .expect_err("record width mismatch must fail");
        assert_eq!(error.kind(), XrefErrorKind::DecodedXrefLengthMismatch);
    }

    #[test]
    fn decoded_limit_is_applied_before_entry_parsing() {
        let (bytes, _) = xref_stream_pdf(true);
        let error = parse_revision_chain_strict(
            &store(bytes),
            XrefLimits {
                max_decoded_stream_bytes: 27,
                ..XrefLimits::default()
            },
        )
        .expect_err("known 28-byte stream exceeds limit by one byte");
        assert!(matches!(error.kind(), XrefErrorKind::StreamDecode(_)));
    }

    #[test]
    fn follows_prev_from_an_xref_stream_to_a_classic_table() {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let catalog = bytes.len();
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog >>\nendobj\n");
        let classic = bytes.len();
        bytes.extend_from_slice(b"xref\n0 2\n0000000000 65535 f \n");
        bytes.extend_from_slice(format!("{catalog:010} 00000 n \n").as_bytes());
        bytes.extend_from_slice(b"trailer\n<< /Size 2 /Root 1 0 R >>\nstartxref\n");
        bytes.extend_from_slice(classic.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");

        let stream_offset = bytes.len();
        let stream_entry =
            encode_entry(1, u32::try_from(stream_offset).expect("fixture offset"), 0);
        bytes.extend_from_slice(
            format!(
                "2 0 obj\n<< /Type /XRef /Size 3 /Index [2 1] /W [1 4 2] /Prev {classic} /Length 7 >>\nstream\n"
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(&stream_entry);
        bytes.extend_from_slice(b"\nendstream\nendobj\nstartxref\n");
        bytes.extend_from_slice(stream_offset.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");

        let chain = parse_revision_chain_strict(&store(bytes), XrefLimits::default())
            .expect("mixed revision chain");
        assert_eq!(chain.revisions().len(), 2);
        assert!(matches!(chain.revisions()[0], XrefSection::Stream(_)));
        assert!(matches!(chain.revisions()[1], XrefSection::Classic(_)));
    }
}
