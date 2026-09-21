use std::fmt;
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceSpan};

use crate::SourceCode;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CMapLimits {
    pub max_tokens: usize,
    pub max_ranges: usize,
    pub max_code_bytes: usize,
    pub max_code_spaces: usize,
    pub max_use_depth: usize,
}

impl Default for CMapLimits {
    fn default() -> Self {
        Self {
            max_tokens: 1_000_000,
            max_ranges: 1_000_000,
            max_code_bytes: 4,
            max_code_spaces: 256,
            max_use_depth: 4,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CMapOrigin {
    Embedded,
    Predefined(&'static str),
    Identity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CMap {
    codespaces: Vec<CodeSpace>,
    mappings: Vec<CidMapping>,
    notdefs: Vec<CidMapping>,
    inherited: Option<Arc<CMap>>,
    identity: bool,
    origin: CMapOrigin,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CodeSpace {
    low: Vec<u8>,
    high: Vec<u8>,
    byte_len: usize,
    span: SourceSpan,
}

impl CodeSpace {
    fn admits(&self, code: &[u8]) -> bool {
        code.len() == self.byte_len
            && code
                .iter()
                .zip(&self.low)
                .zip(&self.high)
                .all(|((byte, low), high)| byte >= low && byte <= high)
    }

    fn contains_range(&self, byte_len: usize, low: u32, high: u32) -> bool {
        byte_len == self.byte_len
            && self.admits(&code_bytes(low, byte_len))
            && self.admits(&code_bytes(high, byte_len))
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.byte_len == other.byte_len
            && self
                .low
                .iter()
                .zip(&self.high)
                .zip(other.low.iter().zip(&other.high))
                .all(|((low, high), (other_low, other_high))| {
                    low <= other_high && other_low <= high
                })
    }

    fn begins_with(&self, byte: u8) -> bool {
        self.low.first().is_some_and(|low| byte >= *low)
            && self.high.first().is_some_and(|high| byte <= *high)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CidMapping {
    low: u32,
    high: u32,
    byte_len: usize,
    first_cid: u32,
    span: SourceSpan,
}

impl CMap {
    #[must_use]
    pub fn identity_horizontal() -> Self {
        Self {
            codespaces: Vec::new(),
            mappings: Vec::new(),
            notdefs: Vec::new(),
            inherited: None,
            identity: true,
            origin: CMapOrigin::Identity,
        }
    }

    #[must_use]
    pub const fn origin(&self) -> CMapOrigin {
        self.origin
    }

    fn effective_codespaces(&self) -> &[CodeSpace] {
        if !self.codespaces.is_empty() {
            return &self.codespaces;
        }
        self.inherited
            .as_ref()
            .map_or(&self.codespaces, |inherited| {
                inherited.effective_codespaces()
            })
    }

    fn selected_cid(&self, byte_len: usize, value: u32) -> Option<(u32, Option<SourceSpan>)> {
        let mine = matches!(self.origin, CMapOrigin::Embedded);
        if let Some(mapping) = find_mapping(&self.mappings, byte_len, value) {
            let cid = mapping.first_cid.checked_add(value - mapping.low)?;
            return Some((cid, mine.then_some(mapping.span)));
        }
        if let Some(inherited) = self.inherited.as_ref()
            && let Some(found) = inherited.selected_cid(byte_len, value)
        {
            return Some(found);
        }
        let notdef = find_mapping(&self.notdefs, byte_len, value)?;
        Some((notdef.first_cid, mine.then_some(notdef.span)))
    }

    pub fn source_codes<F>(&self, bytes: &[u8], mut width: F) -> Result<Vec<SourceCode>, CMapError>
    where
        F: FnMut(u32) -> f64,
    {
        if self.identity {
            return Ok(bytes
                .chunks(2)
                .enumerate()
                .map(|(index, code)| {
                    let value = u32::from_be_bytes([0, 0, code[0], *code.get(1).unwrap_or(&0)]);
                    SourceCode {
                        bytes: code.to_vec(),
                        value,
                        byte_offset: index * 2,
                        cid: Some(value),
                        mapping_span: None,
                        completed_bytes: 2 - code.len(),
                        width: width(value),
                    }
                })
                .collect());
        }

        let codespaces = self.effective_codespaces();
        let mut result = Vec::new();
        let mut offset = 0;
        while offset < bytes.len() {
            let declared = Self::code_length(codespaces, &bytes[offset..])?;
            let present = declared.min(bytes.len() - offset);
            let code_bytes = &bytes[offset..offset + present];
            let mut completed = code_bytes.to_vec();
            completed.resize(declared, 0);
            let value = bytes_to_u32(&completed);
            let admitted = codespaces.iter().any(|space| space.admits(&completed));
            let selected = if admitted {
                self.selected_cid(declared, value)
            } else {
                None
            };
            let (cid, mapping_span) = match selected {
                Some((cid, span)) => (cid, span),
                None => (0, None),
            };
            result.push(SourceCode {
                bytes: code_bytes.to_vec(),
                value,
                byte_offset: offset,
                cid: Some(cid),
                mapping_span,
                completed_bytes: declared - present,
                width: width(cid),
            });
            offset += present;
        }
        Ok(result)
    }

    fn code_length(codespaces: &[CodeSpace], rest: &[u8]) -> Result<usize, CMapError> {
        if codespaces.is_empty() {
            return Err(CMapError::MissingCodeSpace);
        }
        let mut admitted = None;
        for space in codespaces {
            let Some(candidate) = rest.get(..space.byte_len) else {
                continue;
            };
            if !space.admits(candidate) {
                continue;
            }
            if admitted.is_some_and(|length| length != space.byte_len) {
                return Err(CMapError::AmbiguousCodeSpace);
            }
            admitted = Some(space.byte_len);
        }
        if let Some(length) = admitted {
            return Ok(length);
        }
        let first = *rest.first().ok_or(CMapError::TruncatedCode)?;
        let mut partial = None;
        for space in codespaces.iter().filter(|space| space.begins_with(first)) {
            if partial.is_some_and(|length| length != space.byte_len) {
                return Err(CMapError::AmbiguousCodeSpace);
            }
            partial = Some(space.byte_len);
        }
        partial.ok_or(CMapError::CodeOutsideCodeSpace)
    }
}

pub type UseCMapResolver<'a> = dyn Fn(&[u8]) -> Option<Result<CMap, CMapError>> + 'a;

pub fn parse_cmap(source: &ByteStore, limits: CMapLimits) -> Result<CMap, CMapError> {
    parse_cmap_using(source, limits, CMapOrigin::Embedded, &|_| None)
}

#[allow(clippy::too_many_lines)]
pub fn parse_cmap_using(
    source: &ByteStore,
    limits: CMapLimits,
    origin: CMapOrigin,
    resolve: &UseCMapResolver<'_>,
) -> Result<CMap, CMapError> {
    if limits.max_use_depth == 0 {
        return Err(CMapError::UseCMapDepth);
    }
    let tokens = tokenize(source, limits)?;
    for window in tokens.windows(3) {
        if window[0].word_is(b"/WMode") && window[2].word_is(b"def") {
            match window[1].integer() {
                Some(0) => {}
                Some(1) => return Err(CMapError::VerticalWritingUnsupported),
                _ => return Err(CMapError::InvalidWritingMode),
            }
        }
    }
    let inherited = parse_use_cmap(&tokens, resolve)?;
    if tokens.iter().any(|token| {
        matches!(
            &token.kind,
            TokenKind::Word(word) if matches!(word.as_slice(), b"beginbfchar" | b"beginbfrange")
        )
    }) {
        return Err(CMapError::UnsupportedMappingOperator);
    }
    let mut codespaces = Vec::new();
    let mut mappings = Vec::new();
    let mut notdefs = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let TokenKind::Word(operator) = &tokens[index].kind else {
            index += 1;
            continue;
        };
        let is_begin = matches!(
            operator.as_slice(),
            b"begincodespacerange"
                | b"begincidchar"
                | b"begincidrange"
                | b"beginnotdefchar"
                | b"beginnotdefrange"
        );
        if !is_begin {
            index += 1;
            continue;
        }
        let count = index
            .checked_sub(1)
            .and_then(|operand| tokens[operand].integer())
            .ok_or(CMapError::InvalidCount)?;
        let count = usize::try_from(count).map_err(|_| CMapError::InvalidCount)?;
        match operator.as_slice() {
            b"begincodespacerange" => {
                check_capacity(codespaces.len(), count, limits.max_code_spaces)?;
                index += 1;
                for _ in 0..count {
                    let low = required_hex(&tokens, &mut index)?;
                    let high = required_hex(&tokens, &mut index)?;
                    let (_, _, byte_len) = validate_code_range(&low, &high, limits)?;
                    codespaces.push(CodeSpace {
                        low: low.bytes.clone(),
                        high: high.bytes.clone(),
                        byte_len,
                        span: covering_span(low.span, high.span)?,
                    });
                }
                require_word(&tokens, &mut index, b"endcodespacerange")?;
            }
            b"begincidchar" | b"beginnotdefchar" => {
                let into = if operator.as_slice() == b"begincidchar" {
                    &mut mappings
                } else {
                    &mut notdefs
                };
                check_capacity(into.len(), count, limits.max_ranges)?;
                index += 1;
                for _ in 0..count {
                    let code = required_hex(&tokens, &mut index)?;
                    validate_code_bytes(&code.bytes, limits)?;
                    let cid = required_integer(&tokens, &mut index)?;
                    into.push(CidMapping {
                        low: bytes_to_u32(&code.bytes),
                        high: bytes_to_u32(&code.bytes),
                        byte_len: code.bytes.len(),
                        first_cid: cid,
                        span: covering_span(code.span, tokens[index - 1].span)?,
                    });
                }
                let end: &[u8] = if operator.as_slice() == b"begincidchar" {
                    b"endcidchar"
                } else {
                    b"endnotdefchar"
                };
                require_word(&tokens, &mut index, end)?;
            }
            b"begincidrange" | b"beginnotdefrange" => {
                let is_notdef = operator.as_slice() == b"beginnotdefrange";
                let into = if is_notdef {
                    &mut notdefs
                } else {
                    &mut mappings
                };
                check_capacity(into.len(), count, limits.max_ranges)?;
                index += 1;
                for _ in 0..count {
                    let low = required_hex(&tokens, &mut index)?;
                    let high = required_hex(&tokens, &mut index)?;
                    let first_cid = required_integer(&tokens, &mut index)?;
                    let (low_value, high_value, byte_len) =
                        validate_code_range(&low, &high, limits)?;
                    if !is_notdef {
                        first_cid
                            .checked_add(high_value - low_value)
                            .ok_or(CMapError::CidOverflow)?;
                    }
                    into.push(CidMapping {
                        low: low_value,
                        high: high_value,
                        byte_len,
                        first_cid,
                        span: covering_span(low.span, tokens[index - 1].span)?,
                    });
                }
                let end: &[u8] = if is_notdef {
                    b"endnotdefrange"
                } else {
                    b"endcidrange"
                };
                require_word(&tokens, &mut index, end)?;
            }
            _ => index += 1,
        }
    }
    if codespaces.is_empty()
        && inherited
            .as_ref()
            .is_none_or(|used| used.effective_codespaces().is_empty())
    {
        return Err(CMapError::MissingCodeSpace);
    }
    codespaces.sort_unstable_by(|left, right| {
        (left.byte_len, &left.low).cmp(&(right.byte_len, &right.low))
    });
    mappings.sort_unstable_by_key(|range| (range.byte_len, range.low));
    notdefs.sort_unstable_by_key(|range| (range.byte_len, range.low));
    validate_non_overlapping(&codespaces, &mappings, &notdefs)?;
    let effective = if codespaces.is_empty() {
        inherited
            .as_ref()
            .map_or(&codespaces[..], |used| used.effective_codespaces())
    } else {
        &codespaces[..]
    };
    for mapping in mappings.iter().chain(&notdefs) {
        if !effective
            .iter()
            .any(|codespace| codespace.contains_range(mapping.byte_len, mapping.low, mapping.high))
        {
            return Err(CMapError::MappingOutsideCodeSpace);
        }
    }
    Ok(CMap {
        codespaces,
        mappings,
        notdefs,
        inherited: inherited.map(Arc::new),
        identity: false,
        origin,
    })
}

fn parse_use_cmap(
    tokens: &[Token],
    resolve: &UseCMapResolver<'_>,
) -> Result<Option<CMap>, CMapError> {
    let mut found = None;
    for (index, token) in tokens.iter().enumerate() {
        if !token.word_is(b"usecmap") {
            continue;
        }
        if found.is_some() {
            return Err(CMapError::AmbiguousUseCMap);
        }
        let TokenKind::Word(name) = &tokens
            .get(index.wrapping_sub(1))
            .ok_or(CMapError::UseCMapUnsupported)?
            .kind
        else {
            return Err(CMapError::UseCMapUnsupported);
        };
        let name = name
            .strip_prefix(b"/")
            .ok_or(CMapError::UseCMapUnsupported)?;
        found = Some(resolve(name).ok_or(CMapError::UseCMapUnsupported)??);
    }
    Ok(found)
}

pub(crate) fn parse_bf_mappings(
    source: &ByteStore,
    limits: CMapLimits,
) -> Result<std::collections::BTreeMap<crate::tounicode::Code, crate::tounicode::Meaning>, CMapError>
{
    let tokens = tokenize(source, limits)?;
    if tokens.iter().any(|token| token.word_is(b"usecmap")) {
        return Err(CMapError::UseCMapUnsupported);
    }
    let mut codespaces = Vec::new();
    let mut mapped = std::collections::BTreeMap::new();
    let mut index = 0;
    while index < tokens.len() {
        let TokenKind::Word(operator) = &tokens[index].kind else {
            index += 1;
            continue;
        };
        let operator = operator.clone();
        if !matches!(
            operator.as_slice(),
            b"begincodespacerange" | b"beginbfchar" | b"beginbfrange"
        ) {
            index += 1;
            continue;
        }
        let count = index
            .checked_sub(1)
            .and_then(|operand| tokens[operand].integer())
            .ok_or(CMapError::InvalidCount)?;
        let count = usize::try_from(count).map_err(|_| CMapError::InvalidCount)?;
        check_capacity(mapped.len(), count, limits.max_ranges)?;
        index += 1;
        match operator.as_slice() {
            b"begincodespacerange" => {
                for _ in 0..count {
                    let low = required_hex(&tokens, &mut index)?;
                    let high = required_hex(&tokens, &mut index)?;
                    let (_, _, byte_len) = validate_code_range(&low, &high, limits)?;
                    codespaces.push(CodeSpace {
                        low: low.bytes.clone(),
                        high: high.bytes.clone(),
                        byte_len,
                        span: covering_span(low.span, high.span)?,
                    });
                }
                require_word(&tokens, &mut index, b"endcodespacerange")?;
            }
            b"beginbfchar" => {
                read_bf_chars(&tokens, &mut index, count, limits, &mut mapped)?;
            }
            _ => {
                read_bf_ranges(&tokens, &mut index, count, limits, &mut mapped)?;
            }
        }
    }
    if codespaces.is_empty() {
        return Err(CMapError::MissingCodeSpace);
    }
    codespaces.sort_unstable_by(|left, right| {
        (left.byte_len, &left.low).cmp(&(right.byte_len, &right.low))
    });
    if let Some(outside) = mapped
        .keys()
        .find(|code| find_codespace(&codespaces, code.byte_len, code.value).is_none())
    {
        let _ = outside;
        return Err(CMapError::MappingOutsideCodeSpace);
    }
    Ok(mapped)
}

fn read_bf_chars(
    tokens: &[Token],
    index: &mut usize,
    count: usize,
    limits: CMapLimits,
    mapped: &mut std::collections::BTreeMap<crate::tounicode::Code, crate::tounicode::Meaning>,
) -> Result<(), CMapError> {
    use crate::tounicode::{Code, Confidence, Meaning};

    for _ in 0..count {
        let code = required_hex(tokens, index)?;
        validate_code_bytes(&code.bytes, limits)?;
        let text = required_text(tokens, index)?;
        if is_a_declaration(&text) {
            mapped.insert(
                Code {
                    value: bytes_to_u32(&code.bytes),
                    byte_len: code.bytes.len(),
                },
                Meaning {
                    text,
                    confidence: Confidence::Declared,
                },
            );
        }
    }
    require_word(tokens, index, b"endbfchar")
}

fn read_bf_ranges(
    tokens: &[Token],
    index: &mut usize,
    count: usize,
    limits: CMapLimits,
    mapped: &mut std::collections::BTreeMap<crate::tounicode::Code, crate::tounicode::Meaning>,
) -> Result<(), CMapError> {
    use crate::tounicode::{Code, Confidence, Meaning};

    for _ in 0..count {
        let low = required_hex(tokens, index)?;
        let high = required_hex(tokens, index)?;
        let (low_value, high_value, byte_len) = validate_code_range(&low, &high, limits)?;
        let width = usize::try_from(high_value - low_value).map_err(|_| CMapError::RangeLimit)?;
        check_capacity(mapped.len(), width + 1, limits.max_ranges)?;
        if tokens.get(*index).is_some_and(|token| token.word_is(b"[")) {
            *index += 1;
            for value in low_value..=high_value {
                let text = required_text(tokens, index)?;
                if is_a_declaration(&text) {
                    mapped.insert(
                        Code { value, byte_len },
                        Meaning {
                            text,
                            confidence: Confidence::Declared,
                        },
                    );
                }
            }
            require_word(tokens, index, b"]")?;
            continue;
        }
        let units = required_units(tokens, index)?;
        for value in low_value..=high_value {
            let mut units = units.clone();
            let step = u16::try_from(value - low_value).map_err(|_| CMapError::RangeLimit)?;
            let last = units.last_mut().ok_or(CMapError::OperandType)?;
            *last = last.checked_add(step).ok_or(CMapError::CidOverflow)?;
            let text = String::from_utf16(&units).map_err(|_| CMapError::OperandType)?;
            if is_a_declaration(&text) {
                mapped.insert(
                    Code { value, byte_len },
                    Meaning {
                        text,
                        confidence: Confidence::Declared,
                    },
                );
            }
        }
    }
    require_word(tokens, index, b"endbfrange")
}

fn required_units(tokens: &[Token], index: &mut usize) -> Result<Vec<u16>, CMapError> {
    let token = tokens.get(*index).ok_or(CMapError::UnexpectedEnd)?;
    let TokenKind::Hex(bytes) = &token.kind else {
        return Err(CMapError::OperandType);
    };
    if bytes.is_empty() || bytes.len() % 2 != 0 {
        return Err(CMapError::OddHexDigits);
    }
    *index += 1;
    Ok(bytes
        .chunks_exact(2)
        .map(|pair| u16::from(pair[0]) << 8 | u16::from(pair[1]))
        .collect())
}

pub(crate) fn is_a_declaration(text: &str) -> bool {
    !text.is_empty() && !text.contains(['\u{0}', '\u{fffd}'])
}

fn required_text(tokens: &[Token], index: &mut usize) -> Result<String, CMapError> {
    let units = required_units(tokens, index)?;
    String::from_utf16(&units).map_err(|_| CMapError::OperandType)
}

fn find_codespace(ranges: &[CodeSpace], byte_len: usize, value: u32) -> Option<&CodeSpace> {
    let bytes = code_bytes(value, byte_len);
    ranges.iter().find(|range| range.admits(&bytes))
}

fn find_mapping(ranges: &[CidMapping], byte_len: usize, value: u32) -> Option<&CidMapping> {
    let insertion = ranges.partition_point(|range| {
        range.byte_len < byte_len || (range.byte_len == byte_len && range.low <= value)
    });
    ranges
        .get(insertion.checked_sub(1)?)
        .filter(|range| range.byte_len == byte_len && value <= range.high)
}

fn validate_non_overlapping(
    codespaces: &[CodeSpace],
    mappings: &[CidMapping],
    notdefs: &[CidMapping],
) -> Result<(), CMapError> {
    for (index, left) in codespaces.iter().enumerate() {
        if codespaces[index + 1..]
            .iter()
            .any(|right| left.overlaps(right))
        {
            return Err(CMapError::AmbiguousCodeSpace);
        }
    }
    for ranges in [mappings, notdefs] {
        for pair in ranges.windows(2) {
            if pair[0].byte_len == pair[1].byte_len && pair[1].low <= pair[0].high {
                return Err(CMapError::AmbiguousMapping);
            }
        }
    }
    Ok(())
}

fn check_capacity(current: usize, additional: usize, maximum: usize) -> Result<(), CMapError> {
    if current
        .checked_add(additional)
        .is_some_and(|total| total <= maximum)
    {
        Ok(())
    } else {
        Err(CMapError::RangeLimit)
    }
}

fn validate_code_range(
    low: &HexToken,
    high: &HexToken,
    limits: CMapLimits,
) -> Result<(u32, u32, usize), CMapError> {
    validate_code_bytes(&low.bytes, limits)?;
    validate_code_bytes(&high.bytes, limits)?;
    if low.bytes.len() != high.bytes.len() {
        return Err(CMapError::RangeLengthMismatch);
    }
    let low_value = bytes_to_u32(&low.bytes);
    let high_value = bytes_to_u32(&high.bytes);
    if low_value > high_value {
        return Err(CMapError::ReversedRange);
    }
    Ok((low_value, high_value, low.bytes.len()))
}

fn validate_code_bytes(bytes: &[u8], limits: CMapLimits) -> Result<(), CMapError> {
    if bytes.is_empty() || bytes.len() > limits.max_code_bytes || bytes.len() > 4 {
        Err(CMapError::InvalidCodeLength)
    } else {
        Ok(())
    }
}

fn code_bytes(value: u32, byte_len: usize) -> Vec<u8> {
    value.to_be_bytes()[4 - byte_len.min(4)..].to_vec()
}

fn bytes_to_u32(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .fold(0, |value, byte| (value << 8) | u32::from(*byte))
}

fn covering_span(first: SourceSpan, last: SourceSpan) -> Result<SourceSpan, CMapError> {
    if first.source() != last.source() || first.start() > last.end() {
        return Err(CMapError::SourceSpanFailure);
    }
    SourceSpan::new(first.source(), first.start(), last.end())
        .map_err(|_| CMapError::SourceSpanFailure)
}

#[derive(Clone, Debug)]
struct Token {
    kind: TokenKind,
    span: SourceSpan,
}

impl Token {
    fn integer(&self) -> Option<u32> {
        let TokenKind::Integer(value) = self.kind else {
            return None;
        };
        Some(value)
    }

    fn word_is(&self, expected: &[u8]) -> bool {
        matches!(&self.kind, TokenKind::Word(word) if word == expected)
    }
}

#[derive(Clone, Debug)]
enum TokenKind {
    Integer(u32),
    Hex(Vec<u8>),
    Word(Vec<u8>),
}

#[derive(Clone, Debug)]
struct HexToken {
    bytes: Vec<u8>,
    span: SourceSpan,
}

fn tokenize(source: &ByteStore, limits: CMapLimits) -> Result<Vec<Token>, CMapError> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        while offset < bytes.len() && is_whitespace(bytes[offset]) {
            offset += 1;
        }
        if bytes.get(offset) == Some(&b'%') {
            while offset < bytes.len() && !matches!(bytes[offset], b'\r' | b'\n') {
                offset += 1;
            }
            continue;
        }
        if offset == bytes.len() {
            break;
        }
        if tokens.len() >= limits.max_tokens {
            return Err(CMapError::TokenLimit);
        }
        let start = offset;
        if matches!(bytes[offset], b'<' | b'>') && bytes.get(offset + 1) == Some(&bytes[offset]) {
            offset += 2;
            let span = SourceSpan::new(source.id(), start, offset)
                .map_err(|_| CMapError::SourceSpanFailure)?;
            tokens.push(Token {
                kind: TokenKind::Word(bytes[start..offset].to_vec()),
                span,
            });
            continue;
        }
        let kind = if bytes[offset] == b'<' {
            offset += 1;
            let mut nibbles = Vec::new();
            while offset < bytes.len() && bytes[offset] != b'>' {
                if is_whitespace(bytes[offset]) {
                    offset += 1;
                    continue;
                }
                nibbles.push(hex_value(bytes[offset]).ok_or(CMapError::InvalidHex)?);
                offset += 1;
            }
            if offset == bytes.len() {
                return Err(CMapError::UnterminatedHex);
            }
            offset += 1;
            if nibbles.len() % 2 != 0 {
                return Err(CMapError::OddHexDigits);
            }
            let decoded = nibbles
                .chunks_exact(2)
                .map(|pair| pair[0] << 4 | pair[1])
                .collect();
            TokenKind::Hex(decoded)
        } else {
            while offset < bytes.len()
                && !is_whitespace(bytes[offset])
                && !matches!(bytes[offset], b'<' | b'>' | b'%')
            {
                offset += 1;
            }
            if offset == start {
                offset += 1;
            }
            let word = &bytes[start..offset];
            if !word.is_empty() && word.iter().all(u8::is_ascii_digit) {
                let text = std::str::from_utf8(word).map_err(|_| CMapError::InvalidInteger)?;
                TokenKind::Integer(text.parse().map_err(|_| CMapError::InvalidInteger)?)
            } else {
                TokenKind::Word(word.to_vec())
            }
        };
        let span = SourceSpan::new(source.id(), start, offset)
            .map_err(|_| CMapError::SourceSpanFailure)?;
        tokens.push(Token { kind, span });
    }
    Ok(tokens)
}

const fn is_whitespace(byte: u8) -> bool {
    matches!(byte, 0 | b'\t' | b'\n' | 0x0c | b'\r' | b' ')
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn required_hex(tokens: &[Token], index: &mut usize) -> Result<HexToken, CMapError> {
    let token = tokens.get(*index).ok_or(CMapError::UnexpectedEnd)?;
    let TokenKind::Hex(bytes) = &token.kind else {
        return Err(CMapError::OperandType);
    };
    *index += 1;
    Ok(HexToken {
        bytes: bytes.clone(),
        span: token.span,
    })
}

fn required_integer(tokens: &[Token], index: &mut usize) -> Result<u32, CMapError> {
    let token = tokens.get(*index).ok_or(CMapError::UnexpectedEnd)?;
    let TokenKind::Integer(value) = token.kind else {
        return Err(CMapError::OperandType);
    };
    *index += 1;
    Ok(value)
}

fn require_word(tokens: &[Token], index: &mut usize, expected: &[u8]) -> Result<(), CMapError> {
    let token = tokens.get(*index).ok_or(CMapError::UnexpectedEnd)?;
    let TokenKind::Word(word) = &token.kind else {
        return Err(CMapError::MissingEndOperator);
    };
    if word != expected {
        return Err(CMapError::MissingEndOperator);
    }
    *index += 1;
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CMapError {
    TokenLimit,
    RangeLimit,
    InvalidCount,
    InvalidInteger,
    InvalidHex,
    UnterminatedHex,
    OddHexDigits,
    UnexpectedEnd,
    OperandType,
    MissingEndOperator,
    MissingCodeSpace,
    InvalidCodeLength,
    RangeLengthMismatch,
    ReversedRange,
    AmbiguousCodeSpace,
    AmbiguousMapping,
    CodeOutsideCodeSpace,
    MappingOutsideCodeSpace,
    TruncatedCode,
    CidOverflow,
    VerticalWritingUnsupported,
    InvalidWritingMode,
    UseCMapUnsupported,
    UseCMapDepth,
    UnknownPredefinedCMap,
    AmbiguousUseCMap,
    UnsupportedMappingOperator,
    SourceSpanFailure,
}

impl fmt::Display for CMapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid CMap: {self:?}")
    }
}

impl std::error::Error for CMapError {}

#[cfg(test)]
mod tests {
    use super::{CMapError, CMapLimits, CMapOrigin, parse_cmap, parse_cmap_using};
    use pdf_bytes::{ByteStore, SourceId};
    use std::sync::Arc;

    fn source(bytes: &'static [u8]) -> ByteStore {
        ByteStore::new(SourceId::new(91), Arc::<[u8]>::from(bytes))
    }

    #[test]
    fn maps_variable_length_codes_to_cids_with_provenance() {
        let source = source(b"2 begincodespacerange <00> <7f> <8100> <81ff> endcodespacerange 1 begincidchar <20> 3 endcidchar 1 begincidrange <8100> <8102> 40 endcidrange");
        let cmap = parse_cmap(&source, CMapLimits::default()).expect("CMap");
        let codes = cmap
            .source_codes(&[0x20, 0x81, 0x02, 0x21], |cid| f64::from(cid) + 500.0)
            .expect("codes");
        assert_eq!(
            codes.iter().map(|code| code.value).collect::<Vec<_>>(),
            vec![0x20, 0x8102, 0x21]
        );
        assert_eq!(
            codes.iter().map(|code| code.cid).collect::<Vec<_>>(),
            vec![Some(3), Some(42), Some(0)]
        );
        assert_eq!(
            codes
                .iter()
                .map(|code| code.byte_offset)
                .collect::<Vec<_>>(),
            vec![0, 1, 3]
        );
        assert!(codes[0].mapping_span.is_some());
        assert!(codes[2].mapping_span.is_none());
        assert!((codes[1].width - 542.0).abs() < f64::EPSILON);
    }

    #[test]
    fn rejects_ambiguous_and_truncated_inputs() {
        let overlap = source(b"2 begincodespacerange <00> <ff> <20> <30> endcodespacerange");
        assert_eq!(
            parse_cmap(&overlap, CMapLimits::default()),
            Err(CMapError::AmbiguousCodeSpace)
        );
        let gapped = source(b"1 begincodespacerange <20> <30> endcodespacerange");
        let cmap = parse_cmap(&gapped, CMapLimits::default()).expect("CMap");
        assert_eq!(
            cmap.source_codes(&[0x99], |_| 0.0),
            Err(CMapError::CodeOutsideCodeSpace)
        );
    }

    #[test]
    fn a_code_space_is_matched_byte_by_byte_and_not_by_value() {
        let source = source(
            b"2 begincodespacerange <00> <80> <8140> <9ffc> endcodespacerange \
              1 begincidrange <8140> <91fc> 633 endcidrange",
        );
        let cmap = parse_cmap(&source, CMapLimits::default()).expect("CMap");

        let inside = cmap.source_codes(&[0x81, 0x50], |_| 0.0).expect("codes");
        assert_eq!(inside.len(), 1);
        assert_eq!(inside[0].value, 0x8150);
        assert_eq!(inside[0].cid, Some(633 + 0x10));

        let between = cmap.source_codes(&[0x82, 0x00], |_| 0.0).expect("codes");
        assert_eq!(between.len(), 1);
        assert_eq!(between[0].bytes.len(), 2);
        assert_eq!(between[0].cid, Some(0));
        assert!(between[0].mapping_span.is_none());

        assert_eq!(
            cmap.source_codes(&[0xff], |_| 0.0),
            Err(CMapError::CodeOutsideCodeSpace)
        );

        let short = cmap.source_codes(&[0x81], |_| 0.0).expect("codes");
        assert_eq!(short.len(), 1);
        assert_eq!(short[0].bytes, vec![0x81], "only the file's byte is kept");
        assert_eq!(short[0].value, 0x8100, "completed with a zero byte");
        assert_eq!(short[0].completed_bytes, 1);
    }

    #[test]
    fn an_incomplete_identity_code_is_completed_and_recorded() {
        let identity = super::CMap::identity_horizontal();
        let codes = identity
            .source_codes(&[0x00, 0x41, 0x2e], |_| 0.0)
            .expect("codes");
        assert_eq!(codes.len(), 2);
        assert_eq!(codes[0].value, 0x0041);
        assert_eq!(codes[0].completed_bytes, 0);
        assert_eq!(codes[1].bytes, vec![0x2e]);
        assert_eq!(codes[1].value, 0x2e00);
        assert_eq!(codes[1].cid, Some(0x2e00));
        assert_eq!(codes[1].completed_bytes, 1);
        assert_eq!(codes[1].byte_offset, 2);
    }

    #[test]
    fn a_notdef_range_substitutes_one_cid_and_does_not_increment() {
        let source = source(
            b"1 begincodespacerange <0000> <ffff> endcodespacerange \
              1 beginnotdefrange <0000> <001f> 1 endnotdefrange \
              1 begincidrange <0010> <0011> 700 endcidrange",
        );
        let cmap = parse_cmap(&source, CMapLimits::default()).expect("CMap");
        let codes = cmap
            .source_codes(&[0x00, 0x00, 0x00, 0x1f, 0x00, 0x11, 0x00, 0x20], |_| 0.0)
            .expect("codes");
        assert_eq!(
            codes.iter().map(|code| code.cid).collect::<Vec<_>>(),
            vec![Some(1), Some(1), Some(701), Some(0)]
        );
    }

    #[test]
    fn usecmap_inherits_what_the_using_cmap_does_not_answer() {
        let base = source(
            b"1 begincodespacerange <0000> <ffff> endcodespacerange \
              1 begincidrange <0041> <0043> 10 endcidrange",
        );
        let using = source(b"/Base usecmap 1 begincidchar <0042> 99 endcidchar");
        let cmap = parse_cmap_using(
            &using,
            CMapLimits::default(),
            CMapOrigin::Embedded,
            &|name| (name == b"Base").then(|| parse_cmap(&base, CMapLimits::default())),
        )
        .expect("CMap");
        let codes = cmap
            .source_codes(&[0x00, 0x41, 0x00, 0x42, 0x00, 0x43, 0x00, 0x50], |_| 0.0)
            .expect("codes");
        assert_eq!(
            codes.iter().map(|code| code.cid).collect::<Vec<_>>(),
            vec![Some(10), Some(99), Some(12), Some(0)]
        );
    }

    #[test]
    fn an_inherited_predefined_mapping_offers_no_span_into_the_document() {
        let base = source(
            b"1 begincodespacerange <0000> <ffff> endcodespacerange \
              1 begincidchar <0041> 5 endcidchar",
        );
        let using = source(b"/Base usecmap 1 begincidchar <0042> 6 endcidchar");
        let cmap = parse_cmap_using(&using, CMapLimits::default(), CMapOrigin::Embedded, &|_| {
            Some(parse_cmap_using(
                &base,
                CMapLimits::default(),
                CMapOrigin::Predefined("Base"),
                &|_| None,
            ))
        })
        .expect("CMap");
        let codes = cmap
            .source_codes(&[0x00, 0x41, 0x00, 0x42], |_| 0.0)
            .expect("codes");
        assert_eq!(codes[0].cid, Some(5));
        assert!(
            codes[0].mapping_span.is_none(),
            "the resource's instruction is not in this document"
        );
        assert_eq!(codes[1].cid, Some(6));
        assert!(
            codes[1].mapping_span.is_some(),
            "the using CMap's own instruction is"
        );
    }

    #[test]
    fn unresolvable_inheritance_fails_closed() {
        let base = b"1 begincodespacerange <0000> <ffff> endcodespacerange";
        let unresolved =
            source(b"/Missing usecmap 1 begincodespacerange <00> <ff> endcodespacerange");
        assert_eq!(
            parse_cmap(&unresolved, CMapLimits::default()),
            Err(CMapError::UseCMapUnsupported)
        );
        let twice =
            source(b"/A usecmap /B usecmap 1 begincodespacerange <00> <ff> endcodespacerange");
        assert_eq!(
            parse_cmap_using(&twice, CMapLimits::default(), CMapOrigin::Embedded, &|_| {
                Some(parse_cmap(&source(base), CMapLimits::default()))
            }),
            Err(CMapError::AmbiguousUseCMap)
        );
        let broken = source(b"/A usecmap 1 begincodespacerange <00> <ff> endcodespacerange");
        assert_eq!(
            parse_cmap_using(
                &broken,
                CMapLimits::default(),
                CMapOrigin::Embedded,
                &|_| { Some(Err(CMapError::OddHexDigits)) }
            ),
            Err(CMapError::OddHexDigits)
        );
        let deep = source(b"/A usecmap 1 begincodespacerange <00> <ff> endcodespacerange");
        let limits = CMapLimits {
            max_use_depth: 0,
            ..CMapLimits::default()
        };
        assert_eq!(
            parse_cmap_using(&deep, limits, CMapOrigin::Embedded, &|_| Some(parse_cmap(
                &source(base),
                CMapLimits::default()
            ))),
            Err(CMapError::UseCMapDepth)
        );
    }

    #[test]
    fn resource_limits_refuse_rather_than_stall() {
        let many = source(
            b"3 begincodespacerange <00> <10> <20> <30> <40> <50> endcodespacerange \
              1 begincidchar <00> 1 endcidchar",
        );
        assert_eq!(
            parse_cmap(
                &many,
                CMapLimits {
                    max_code_spaces: 2,
                    ..CMapLimits::default()
                }
            ),
            Err(CMapError::RangeLimit)
        );
        assert_eq!(
            parse_cmap(
                &many,
                CMapLimits {
                    max_ranges: 0,
                    ..CMapLimits::default()
                }
            ),
            Err(CMapError::RangeLimit)
        );
        assert_eq!(
            parse_cmap(
                &many,
                CMapLimits {
                    max_tokens: 4,
                    ..CMapLimits::default()
                }
            ),
            Err(CMapError::TokenLimit)
        );
    }

    #[test]
    fn malformed_cid_cmaps_are_named_rather_than_repaired() {
        let no_space = source(b"1 begincidchar <20> 3 endcidchar");
        assert_eq!(
            parse_cmap(&no_space, CMapLimits::default()),
            Err(CMapError::MissingCodeSpace)
        );
        let outside = source(
            b"1 begincodespacerange <00> <7f> endcodespacerange \
              1 begincidchar <9020> 3 endcidchar",
        );
        assert_eq!(
            parse_cmap(&outside, CMapLimits::default()),
            Err(CMapError::MappingOutsideCodeSpace)
        );
        let twice = source(
            b"1 begincodespacerange <0000> <ffff> endcodespacerange \
              2 begincidrange <0010> <0020> 5 <0018> <0028> 9 endcidrange",
        );
        assert_eq!(
            parse_cmap(&twice, CMapLimits::default()),
            Err(CMapError::AmbiguousMapping)
        );
        let reversed = source(
            b"1 begincodespacerange <0000> <ffff> endcodespacerange \
              1 begincidrange <0030> <0020> 5 endcidrange",
        );
        assert_eq!(
            parse_cmap(&reversed, CMapLimits::default()),
            Err(CMapError::ReversedRange)
        );
        let mismatched = source(
            b"1 begincodespacerange <0000> <ffff> endcodespacerange \
              1 begincidrange <0030> <40> 5 endcidrange",
        );
        assert_eq!(
            parse_cmap(&mismatched, CMapLimits::default()),
            Err(CMapError::RangeLengthMismatch)
        );
        let unterminated = source(
            b"1 begincodespacerange <0000> <ffff> endcodespacerange \
              1 begincidchar <0030> 5",
        );
        assert_eq!(
            parse_cmap(&unterminated, CMapLimits::default()),
            Err(CMapError::UnexpectedEnd)
        );
    }

    #[test]
    fn rejects_vertical_inheritance_and_semantic_cmaps_explicitly() {
        let vertical = source(b"/WMode 1 def 1 begincodespacerange <00> <ff> endcodespacerange");
        assert_eq!(
            parse_cmap(&vertical, CMapLimits::default()),
            Err(CMapError::VerticalWritingUnsupported)
        );
        let inherited =
            source(b"/Identity-H usecmap 1 begincodespacerange <00> <ff> endcodespacerange");
        assert_eq!(
            parse_cmap(&inherited, CMapLimits::default()),
            Err(CMapError::UseCMapUnsupported)
        );
        let unicode = source(
            b"1 begincodespacerange <00> <ff> endcodespacerange 1 beginbfchar <20> <0041> endbfchar",
        );
        assert_eq!(
            parse_cmap(&unicode, CMapLimits::default()),
            Err(CMapError::UnsupportedMappingOperator)
        );
    }
}
