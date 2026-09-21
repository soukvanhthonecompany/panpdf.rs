use std::fmt;

use pdf_bytes::ByteStore;

use crate::{Object, ObjectKind};

pub fn decode_string(
    source: &ByteStore,
    object: &Object,
    max_decoded_bytes: usize,
) -> Result<Vec<u8>, StringDecodeError> {
    let raw = source.resolve(object.span()).map_err(|_| {
        StringDecodeError::new(
            object.span().start(),
            StringDecodeErrorKind::SourceSpanFailure,
        )
    })?;
    match object.kind() {
        ObjectKind::LiteralString => decode_literal(raw, object.span().start(), max_decoded_bytes),
        ObjectKind::HexString => decode_hex(raw, object.span().start(), max_decoded_bytes),
        _ => Err(StringDecodeError::new(
            object.span().start(),
            StringDecodeErrorKind::NotAString,
        )),
    }
}

fn decode_literal(
    raw: &[u8],
    offset: usize,
    max_decoded_bytes: usize,
) -> Result<Vec<u8>, StringDecodeError> {
    let contents = raw
        .strip_prefix(b"(")
        .and_then(|raw| raw.strip_suffix(b")"))
        .ok_or_else(|| StringDecodeError::new(offset, StringDecodeErrorKind::MalformedString))?;
    let mut output = Vec::with_capacity(contents.len().min(max_decoded_bytes));
    let mut cursor = 0_usize;
    while cursor < contents.len() {
        let byte = contents[cursor];
        cursor += 1;
        if byte == b'\\' {
            let escaped = *contents.get(cursor).ok_or_else(|| {
                StringDecodeError::new(offset + cursor, StringDecodeErrorKind::MalformedString)
            })?;
            cursor += 1;
            match escaped {
                b'n' => push_bounded(&mut output, b'\n', max_decoded_bytes, offset + cursor)?,
                b'r' => push_bounded(&mut output, b'\r', max_decoded_bytes, offset + cursor)?,
                b't' => push_bounded(&mut output, b'\t', max_decoded_bytes, offset + cursor)?,
                b'b' => push_bounded(&mut output, 0x08, max_decoded_bytes, offset + cursor)?,
                b'f' => push_bounded(&mut output, 0x0c, max_decoded_bytes, offset + cursor)?,
                b'(' | b')' | b'\\' => {
                    push_bounded(&mut output, escaped, max_decoded_bytes, offset + cursor)?;
                }
                b'\n' => {}
                b'\r' => {
                    if contents.get(cursor) == Some(&b'\n') {
                        cursor += 1;
                    }
                }
                b'0'..=b'7' => {
                    let mut value = escaped - b'0';
                    let mut digits = 1;
                    while digits < 3 {
                        let Some(&next) = contents.get(cursor) else {
                            break;
                        };
                        if !(b'0'..=b'7').contains(&next) {
                            break;
                        }
                        value = value.wrapping_mul(8).wrapping_add(next - b'0');
                        cursor += 1;
                        digits += 1;
                    }
                    push_bounded(&mut output, value, max_decoded_bytes, offset + cursor)?;
                }
                _ => push_bounded(&mut output, escaped, max_decoded_bytes, offset + cursor)?,
            }
        } else if byte == b'\r' {
            if contents.get(cursor) == Some(&b'\n') {
                cursor += 1;
            }
            push_bounded(&mut output, b'\n', max_decoded_bytes, offset + cursor)?;
        } else {
            push_bounded(&mut output, byte, max_decoded_bytes, offset + cursor)?;
        }
    }
    Ok(output)
}

fn decode_hex(
    raw: &[u8],
    offset: usize,
    max_decoded_bytes: usize,
) -> Result<Vec<u8>, StringDecodeError> {
    let contents = raw
        .strip_prefix(b"<")
        .and_then(|raw| raw.strip_suffix(b">"))
        .ok_or_else(|| StringDecodeError::new(offset, StringDecodeErrorKind::MalformedString))?;
    let mut output = Vec::with_capacity((contents.len() / 2).min(max_decoded_bytes));
    let mut high = None;
    for (index, &byte) in contents.iter().enumerate() {
        if is_pdf_whitespace(byte) {
            continue;
        }
        let nibble = hex_nibble(byte).ok_or_else(|| {
            StringDecodeError::new(offset + index + 1, StringDecodeErrorKind::MalformedString)
        })?;
        if let Some(first) = high.take() {
            push_bounded(
                &mut output,
                (first << 4) | nibble,
                max_decoded_bytes,
                offset + index + 1,
            )?;
        } else {
            high = Some(nibble);
        }
    }
    if let Some(first) = high {
        push_bounded(
            &mut output,
            first << 4,
            max_decoded_bytes,
            offset + raw.len() - 1,
        )?;
    }
    Ok(output)
}

fn push_bounded(
    output: &mut Vec<u8>,
    byte: u8,
    limit: usize,
    offset: usize,
) -> Result<(), StringDecodeError> {
    if output.len() >= limit {
        return Err(StringDecodeError::new(
            offset,
            StringDecodeErrorKind::DecodedLengthLimit,
        ));
    }
    output.push(byte);
    Ok(())
}

const fn is_pdf_whitespace(byte: u8) -> bool {
    matches!(byte, 0x00 | b'\t' | b'\n' | 0x0c | b'\r' | b' ')
}

const fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StringDecodeError {
    offset: usize,
    kind: StringDecodeErrorKind,
}

impl StringDecodeError {
    const fn new(offset: usize, kind: StringDecodeErrorKind) -> Self {
        Self { offset, kind }
    }

    #[must_use]
    pub const fn offset(self) -> usize {
        self.offset
    }

    #[must_use]
    pub const fn kind(self) -> StringDecodeErrorKind {
        self.kind
    }
}

impl fmt::Display for StringDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at byte {}", self.kind, self.offset)
    }
}

impl std::error::Error for StringDecodeError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StringDecodeErrorKind {
    NotAString,
    MalformedString,
    DecodedLengthLimit,
    SourceSpanFailure,
}

impl fmt::Display for StringDecodeErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NotAString => "object is not a PDF string",
            Self::MalformedString => "PDF string encoding is malformed",
            Self::DecodedLengthLimit => "decoded PDF string exceeds its configured limit",
            Self::SourceSpanFailure => "PDF string span does not belong to the source",
        })
    }
}

#[cfg(test)]
mod tests {
    use pdf_bytes::{ByteStore, SourceId};

    use super::{StringDecodeErrorKind, decode_string};
    use crate::{ObjectParser, ParseLimits};

    fn decode(raw: &'static [u8], limit: usize) -> Result<Vec<u8>, StringDecodeErrorKind> {
        let source = ByteStore::new(SourceId::new(17), raw);
        let object = ObjectParser::new(&source, 0, ParseLimits::default())
            .parse_next()
            .expect("valid test object")
            .expect("one test object");
        decode_string(&source, &object, limit).map_err(super::StringDecodeError::kind)
    }

    #[test]
    fn decodes_literal_escapes_octal_and_line_endings() {
        assert_eq!(
            decode(b"(a\\n\\053\\(\\)\\\\\\z\\\r\nnext\rline)", 64),
            Ok(b"a\n+()\\znext\nline".to_vec())
        );
    }

    #[test]
    fn decodes_hex_whitespace_and_odd_nibble() {
        assert_eq!(decode(b"<48 65 6c 6c 6f 2>", 6), Ok(b"Hello ".to_vec()));
    }

    #[test]
    fn enforces_output_limit_and_string_kind() {
        assert_eq!(
            decode(b"(abc)", 2),
            Err(StringDecodeErrorKind::DecodedLengthLimit)
        );
        assert_eq!(decode(b"42", 2), Err(StringDecodeErrorKind::NotAString));
    }
}
