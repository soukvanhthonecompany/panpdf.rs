#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::fmt;

pub mod cff;
pub mod decipher;
pub mod glyph;
pub mod truetype;
pub mod type1;

use pdf_bytes::{ByteStore, SourceSpan};
use pdf_syntax::{NumberKind, Object, ObjectKind, Reference, decode_name};

mod cmap;
pub mod legacy_cjk;
pub mod outline_match;
pub mod predefined;
pub mod sha256;
pub mod shaping;
pub mod standard14;
pub mod subset;
pub mod substitute;
pub mod system_fonts;
mod tables;
pub mod tounicode;

use standard14::Standard14;

pub use cmap::{
    CMap, CMapError, CMapLimits, CMapOrigin, UseCMapResolver, parse_cmap, parse_cmap_using,
};
pub use tounicode::{Code, Confidence, Meaning, ToUnicode};

type CodeNames = Box<[Option<Vec<u8>>; 256]>;

type EncodingTable = (Option<CodeNames>, Vec<(u8, char)>);

#[derive(Clone, Debug, PartialEq)]
enum Metrics {
    Declared { first_char: u8, widths: Vec<f64> },
    Standard(Standard14),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SimpleFont {
    subtype: Vec<u8>,
    metrics: Metrics,
    dictionary_span: SourceSpan,
    encoding: Option<CodeNames>,
    duplicates: Vec<(u8, char)>,
}

impl SimpleFont {
    #[must_use]
    pub fn subtype(&self) -> &[u8] {
        &self.subtype
    }

    #[must_use]
    pub const fn dictionary_span(&self) -> SourceSpan {
        self.dictionary_span
    }

    #[must_use]
    pub fn glyph_name(&self, code: u8) -> Option<&[u8]> {
        self.encoding.as_ref()?[usize::from(code)].as_deref()
    }

    #[must_use]
    pub fn standard_duplicate(&self, code: u8) -> Option<char> {
        self.duplicates
            .iter()
            .find(|(duplicate, _)| *duplicate == code)
            .map(|(_, character)| *character)
    }

    #[must_use]
    pub fn width(&self, code: u8) -> f64 {
        match &self.metrics {
            Metrics::Declared { first_char, widths } => code
                .checked_sub(*first_char)
                .and_then(|index| widths.get(usize::from(index)))
                .copied()
                .unwrap_or(0.0),
            Metrics::Standard(font) => self
                .glyph_name(code)
                .and_then(|name| font.width(name))
                .unwrap_or(0.0),
        }
    }

    #[must_use]
    pub const fn standard_face(&self) -> Option<Standard14> {
        match self.metrics {
            Metrics::Standard(font) => Some(font),
            Metrics::Declared { .. } => None,
        }
    }

    #[must_use]
    pub fn source_codes(&self, bytes: &[u8]) -> Vec<SourceCode> {
        bytes
            .iter()
            .enumerate()
            .map(|(offset, code)| SourceCode {
                bytes: vec![*code],
                value: u32::from(*code),
                byte_offset: offset,
                cid: None,
                mapping_span: None,
                completed_bytes: 0,
                width: self.width(*code),
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceCode {
    pub bytes: Vec<u8>,
    pub value: u32,
    pub byte_offset: usize,
    pub cid: Option<u32>,
    pub mapping_span: Option<SourceSpan>,
    pub completed_bytes: usize,
    pub width: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CidFont {
    subtype: Vec<u8>,
    default_width: f64,
    widths: Vec<CidWidth>,
    dictionary_span: SourceSpan,
}

#[derive(Clone, Debug, PartialEq)]
struct CidWidth {
    first: u32,
    last: u32,
    width: f64,
    provenance: SourceSpan,
}

impl CidFont {
    #[must_use]
    pub fn subtype(&self) -> &[u8] {
        &self.subtype
    }

    #[must_use]
    pub const fn dictionary_span(&self) -> SourceSpan {
        self.dictionary_span
    }

    #[must_use]
    pub fn width(&self, cid: u32) -> f64 {
        let insertion = self.widths.partition_point(|entry| entry.first <= cid);
        insertion
            .checked_sub(1)
            .and_then(|index| self.widths.get(index))
            .filter(|entry| cid <= entry.last)
            .map_or(self.default_width, |entry| entry.width)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompositeFont {
    encoding: CMap,
    descendant: CidFont,
    dictionary_span: SourceSpan,
}

impl CompositeFont {
    #[must_use]
    pub fn new(encoding: CMap, descendant: CidFont, dictionary_span: SourceSpan) -> Self {
        Self {
            encoding,
            descendant,
            dictionary_span,
        }
    }

    #[must_use]
    pub const fn dictionary_span(&self) -> SourceSpan {
        self.dictionary_span
    }

    pub fn source_codes(&self, bytes: &[u8]) -> Result<Vec<SourceCode>, CMapError> {
        self.encoding
            .source_codes(bytes, |cid| self.descendant.width(cid))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Font {
    Simple(SimpleFont),
    Composite(CompositeFont),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Type0Encoding {
    IdentityHorizontal,
    IdentityVertical,
    Predefined(&'static str),
    Stream(Reference),
}

#[derive(Clone, Debug)]
pub struct Type0Font {
    pub encoding: Type0Encoding,
    pub descendant: Descendant,
    pub dictionary_span: SourceSpan,
}

#[derive(Clone, Debug)]
pub enum Descendant {
    Indirect(Reference),
    Direct(ByteStore, Object),
}

pub type FontResolver<'a> = dyn Fn(Reference) -> Result<(ByteStore, Object), FontError> + 'a;

fn follow(
    source: &ByteStore,
    object: &Object,
    resolve: &FontResolver<'_>,
) -> Result<(ByteStore, Object), FontError> {
    let ObjectKind::Reference(reference) = object.kind() else {
        return Ok((source.clone(), object.clone()));
    };
    let (resolved_source, resolved) = resolve(*reference)?;
    if matches!(resolved.kind(), ObjectKind::Reference(_)) {
        return Err(FontError::IndirectEntryUnresolved);
    }
    Ok((resolved_source, resolved))
}

pub fn parse_type0_font(
    source: &ByteStore,
    object: &Object,
    resolve: &FontResolver<'_>,
) -> Result<Type0Font, FontError> {
    let ObjectKind::Dictionary(entries) = object.kind() else {
        return Err(FontError::NotDictionary);
    };
    reject_duplicate_entries(source, entries)?;
    let subtype = decode_name(source, required_entry(source, entries, b"/Subtype")?)
        .map_err(|_| FontError::InvalidSubtype)?;
    if subtype != b"/Type0" {
        return Err(FontError::InvalidSubtype);
    }
    let encoding_object = required_entry(source, entries, b"/Encoding")?;
    let encoding = match encoding_object.kind() {
        ObjectKind::Name => {
            let name = decode_name(source, encoding_object)
                .map_err(|_| FontError::InvalidType0Encoding)?;
            match name.as_slice() {
                b"/Identity-H" => Type0Encoding::IdentityHorizontal,
                b"/Identity-V" => Type0Encoding::IdentityVertical,
                other => {
                    let name = other
                        .strip_prefix(b"/")
                        .and_then(predefined::is_shipped)
                        .ok_or(FontError::PredefinedCMapUnsupported)?;
                    Type0Encoding::Predefined(name)
                }
            }
        }
        ObjectKind::Reference(reference) => Type0Encoding::Stream(*reference),
        _ => return Err(FontError::InvalidType0Encoding),
    };
    let descendants = required_entry(source, entries, b"/DescendantFonts")?;
    let (descendant_source, descendants) = follow(source, descendants, resolve)?;
    let ObjectKind::Array(descendants) = descendants.kind() else {
        return Err(FontError::InvalidDescendantFonts);
    };
    let [descendant] = descendants.as_slice() else {
        return Err(FontError::InvalidDescendantFonts);
    };
    let descendant = match descendant.kind() {
        ObjectKind::Reference(reference) => Descendant::Indirect(*reference),
        ObjectKind::Dictionary(_) => {
            Descendant::Direct(descendant_source.clone(), descendant.clone())
        }
        _ => return Err(FontError::InvalidDescendantFonts),
    };
    Ok(Type0Font {
        encoding,
        descendant,
        dictionary_span: object.span(),
    })
}

impl Font {
    pub fn source_codes(&self, bytes: &[u8]) -> Result<Vec<SourceCode>, CMapError> {
        match self {
            Self::Simple(font) => Ok(font.source_codes(bytes)),
            Self::Composite(font) => font.source_codes(bytes),
        }
    }
}

pub fn parse_cid_font(
    source: &ByteStore,
    object: &Object,
    resolve: &FontResolver<'_>,
) -> Result<CidFont, FontError> {
    let ObjectKind::Dictionary(entries) = object.kind() else {
        return Err(FontError::NotDictionary);
    };
    reject_duplicate_entries(source, entries)?;
    let subtype = decode_name(source, required_entry(source, entries, b"/Subtype")?)
        .map_err(|_| FontError::InvalidSubtype)?;
    if !matches!(subtype.as_slice(), b"/CIDFontType0" | b"/CIDFontType2") {
        return Err(FontError::InvalidSubtype);
    }
    let default_width = optional_entry(source, entries, b"/DW")
        .map_or(Ok(1000.0), |value| number(source, value))?;
    let mut widths = Vec::new();
    if let Some(widths_object) = optional_entry(source, entries, b"/W") {
        let (source, widths_object) = &follow(source, widths_object, resolve)?;
        let source = &source.clone();
        let ObjectKind::Array(items) = widths_object.kind() else {
            return Err(FontError::WidthsNotArray);
        };
        let mut index = 0;
        while index < items.len() {
            let first = cid(source, &items[index])?;
            index += 1;
            let next = items.get(index).ok_or(FontError::InvalidCidWidths)?;
            if let ObjectKind::Array(values) = next.kind() {
                if values.is_empty() {
                    return Err(FontError::InvalidCidWidths);
                }
                for (offset, value) in values.iter().enumerate() {
                    let offset = u32::try_from(offset).map_err(|_| FontError::InvalidCidWidths)?;
                    let current = first
                        .checked_add(offset)
                        .ok_or(FontError::InvalidCidWidths)?;
                    if current > 65_535 {
                        return Err(FontError::InvalidCidWidths);
                    }
                    widths.push(CidWidth {
                        first: current,
                        last: current,
                        width: number(source, value)?,
                        provenance: value.span(),
                    });
                }
                index += 1;
            } else {
                let last = cid(source, next)?;
                let value = items.get(index + 1).ok_or(FontError::InvalidCidWidths)?;
                if last < first {
                    return Err(FontError::InvalidCidWidths);
                }
                widths.push(CidWidth {
                    first,
                    last,
                    width: number(source, value)?,
                    provenance: value.span(),
                });
                index += 2;
            }
        }
    }
    for (index, left) in widths.iter().enumerate() {
        if widths[index + 1..]
            .iter()
            .any(|right| left.first <= right.last && right.first <= left.last)
        {
            return Err(FontError::AmbiguousCidWidths);
        }
    }
    widths.sort_unstable_by_key(|entry| entry.first);
    Ok(CidFont {
        subtype,
        default_width,
        widths,
        dictionary_span: object.span(),
    })
}

pub fn parse_simple_font(
    source: &ByteStore,
    object: &Object,
    resolve: &FontResolver<'_>,
) -> Result<SimpleFont, FontError> {
    let ObjectKind::Dictionary(entries) = object.kind() else {
        return Err(FontError::NotDictionary);
    };
    reject_duplicate_entries(source, entries)?;
    let subtype_object = required_entry(source, entries, b"/Subtype")?;
    let subtype = decode_name(source, subtype_object).map_err(|_| FontError::InvalidSubtype)?;
    if subtype == b"/Type0" {
        return Err(FontError::CompositeFontUnsupported);
    }
    if !matches!(
        subtype.as_slice(),
        b"/Type1" | b"/MMType1" | b"/TrueType" | b"/Type3"
    ) {
        return Err(FontError::InvalidSubtype);
    }
    let standard = optional_entry(source, entries, b"/BaseFont")
        .and_then(|base| decode_name(source, base).ok())
        .and_then(|base| Standard14::from_base_font(&base));
    let metrics = match optional_entry(source, entries, b"/Widths") {
        Some(widths_object) => {
            let first = integer(source, required_entry(source, entries, b"/FirstChar")?)?;
            let last = integer(source, required_entry(source, entries, b"/LastChar")?)?;
            if !(0..=255).contains(&first) || !(0..=255).contains(&last) || last < first {
                return Err(FontError::InvalidCharacterRange);
            }
            let (widths_source, widths_object) = follow(source, widths_object, resolve)?;
            let ObjectKind::Array(width_objects) = widths_object.kind() else {
                return Err(FontError::WidthsNotArray);
            };
            let expected =
                usize::try_from(last - first + 1).map_err(|_| FontError::InvalidCharacterRange)?;
            if width_objects.len() != expected {
                return Err(FontError::WidthCountMismatch);
            }
            Metrics::Declared {
                first_char: u8::try_from(first).map_err(|_| FontError::InvalidCharacterRange)?,
                widths: width_objects
                    .iter()
                    .map(|width| number(&widths_source, width))
                    .collect::<Result<Vec<_>, _>>()?,
            }
        }
        None => Metrics::Standard(standard.ok_or(FontError::MissingEntry)?),
    };
    let (mut encoding, duplicates) = parse_simple_encoding(source, entries, resolve)?;
    if let Metrics::Standard(face) = metrics {
        apply_standard_face_encoding(face, &mut encoding);
    }
    Ok(SimpleFont {
        subtype,
        metrics,
        dictionary_span: object.span(),
        encoding,
        duplicates,
    })
}

fn apply_standard_face_encoding(face: Standard14, encoding: &mut Option<CodeNames>) {
    let base = face
        .built_in_encoding()
        .unwrap_or(&tables::STANDARD_ENCODING);
    let table = encoding.get_or_insert_with(|| Box::new([const { None }; 256]));
    for (code, glyph) in base.iter().enumerate() {
        if table[code].is_none()
            && let Some(glyph) = glyph
        {
            table[code] = Some((*glyph).to_vec());
        }
    }
}

fn parse_simple_encoding(
    source: &ByteStore,
    entries: &[pdf_syntax::DictionaryEntry],
    resolve: &FontResolver<'_>,
) -> Result<EncodingTable, FontError> {
    let Some(object) = optional_entry(source, entries, b"/Encoding") else {
        return Ok((None, Vec::new()));
    };
    let (encoding_source, object) = follow(source, object, resolve)?;
    let mut table: CodeNames = Box::new([const { None }; 256]);
    let mut duplicates = Vec::new();
    let differences = match object.kind() {
        ObjectKind::Name => {
            duplicates = apply_base_encoding(&encoding_source, &object, &mut table)?;
            None
        }
        ObjectKind::Dictionary(encoding_entries) => {
            reject_duplicate_entries(&encoding_source, encoding_entries)?;
            if let Some(base) = optional_entry(&encoding_source, encoding_entries, b"/BaseEncoding")
            {
                duplicates = apply_base_encoding(&encoding_source, base, &mut table)?;
            }
            optional_entry(&encoding_source, encoding_entries, b"/Differences").cloned()
        }
        _ => return Err(FontError::InvalidEncoding),
    };
    let Some(differences) = differences else {
        return Ok((Some(table), duplicates));
    };
    let (differences_source, differences) = follow(&encoding_source, &differences, resolve)?;
    let ObjectKind::Array(items) = differences.kind() else {
        return Err(FontError::InvalidEncoding);
    };
    let mut code: Option<u16> = None;
    for item in items {
        match item.kind() {
            ObjectKind::Number(_) => {
                let start = integer(&differences_source, item)?;
                if !(0..=255).contains(&start) {
                    return Err(FontError::InvalidEncoding);
                }
                code = Some(u16::try_from(start).map_err(|_| FontError::InvalidEncoding)?);
            }
            ObjectKind::Name => {
                let at = code.ok_or(FontError::InvalidEncoding)?;
                if at > 255 {
                    return Err(FontError::InvalidEncoding);
                }
                let name = decode_name(&differences_source, item)
                    .map_err(|_| FontError::InvalidEncoding)?;
                table[usize::from(at)] = Some(name.get(1..).unwrap_or_default().to_vec());
                duplicates.retain(|(duplicate, _)| u16::from(*duplicate) != at);
                code = Some(at + 1);
            }
            _ => return Err(FontError::InvalidEncoding),
        }
    }
    Ok((Some(table), duplicates))
}

fn apply_base_encoding(
    source: &ByteStore,
    object: &Object,
    table: &mut [Option<Vec<u8>>; 256],
) -> Result<Vec<(u8, char)>, FontError> {
    let name = decode_name(source, object).map_err(|_| FontError::InvalidEncoding)?;
    let base = match name.as_slice() {
        b"/StandardEncoding" => &tables::STANDARD_ENCODING,
        b"/WinAnsiEncoding" => &tables::WIN_ANSI_ENCODING,
        b"/MacRomanEncoding" => &tables::MAC_ROMAN_ENCODING,
        _ => return Err(FontError::UnsupportedBaseEncoding),
    };
    for (code, glyph) in base.iter().enumerate() {
        if let Some(glyph) = glyph {
            table[code] = Some((*glyph).to_vec());
        }
    }
    Ok(match name.as_slice() {
        b"/WinAnsiEncoding" => vec![(0xA0, '\u{a0}'), (0xAD, '\u{ad}')],
        b"/MacRomanEncoding" => vec![(0xCA, '\u{a0}')],
        _ => Vec::new(),
    })
}

fn reject_duplicate_entries(
    source: &ByteStore,
    entries: &[pdf_syntax::DictionaryEntry],
) -> Result<(), FontError> {
    let mut seen = HashSet::new();
    for entry in entries {
        let key = entry
            .decoded_key(source)
            .map_err(|_| FontError::SourceSpanFailure)?;
        if !seen.insert(key) {
            return Err(FontError::DuplicateEntry);
        }
    }
    Ok(())
}

fn optional_entry<'a>(
    source: &ByteStore,
    entries: &'a [pdf_syntax::DictionaryEntry],
    key: &[u8],
) -> Option<&'a Object> {
    entries
        .iter()
        .find(|entry| entry.key_equals(source, key))
        .map(pdf_syntax::DictionaryEntry::value)
}

fn required_entry<'a>(
    source: &ByteStore,
    entries: &'a [pdf_syntax::DictionaryEntry],
    key: &[u8],
) -> Result<&'a Object, FontError> {
    entries
        .iter()
        .find(|entry| entry.key_equals(source, key))
        .map(pdf_syntax::DictionaryEntry::value)
        .ok_or(FontError::MissingEntry)
}

fn integer(source: &ByteStore, object: &Object) -> Result<i64, FontError> {
    if !matches!(object.kind(), ObjectKind::Number(NumberKind::Integer)) {
        return Err(FontError::InvalidNumber);
    }
    let bytes = source
        .resolve(object.span())
        .map_err(|_| FontError::SourceSpanFailure)?;
    let text = std::str::from_utf8(bytes).map_err(|_| FontError::InvalidNumber)?;
    text.parse().map_err(|_| FontError::InvalidNumber)
}

fn cid(source: &ByteStore, object: &Object) -> Result<u32, FontError> {
    let value = integer(source, object)?;
    u32::try_from(value)
        .ok()
        .filter(|value| *value <= 65_535)
        .ok_or(FontError::InvalidCidWidths)
}

fn number(source: &ByteStore, object: &Object) -> Result<f64, FontError> {
    if !matches!(object.kind(), ObjectKind::Number(_)) {
        return Err(FontError::InvalidNumber);
    }
    let bytes = source
        .resolve(object.span())
        .map_err(|_| FontError::SourceSpanFailure)?;
    let text = std::str::from_utf8(bytes).map_err(|_| FontError::InvalidNumber)?;
    let value: f64 = text.parse().map_err(|_| FontError::InvalidNumber)?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(FontError::InvalidNumber)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FontError {
    IndirectEntryUnresolved,
    NotDictionary,
    DuplicateEntry,
    MissingEntry,
    InvalidSubtype,
    CompositeFontUnsupported,
    InvalidCharacterRange,
    WidthsNotArray,
    WidthCountMismatch,
    InvalidNumber,
    InvalidCidWidths,
    AmbiguousCidWidths,
    InvalidType0Encoding,
    InvalidEncoding,
    UnsupportedBaseEncoding,
    PredefinedCMapUnsupported,
    InvalidDescendantFonts,
    VerticalWritingUnsupported,
    SourceSpanFailure,
}

impl fmt::Display for FontError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::IndirectEntryUnresolved => {
                "font dictionary entry could not be resolved to a direct object"
            }
            Self::NotDictionary => "font resource is not a dictionary",
            Self::DuplicateEntry => "font dictionary contains a duplicate entry",
            Self::MissingEntry => "simple font is missing required metrics",
            Self::InvalidSubtype => "font has an invalid or unsupported simple subtype",
            Self::CompositeFontUnsupported => "Type0 composite font CMaps are not implemented",
            Self::InvalidCharacterRange => "simple font character range is invalid",
            Self::WidthsNotArray => "simple font /Widths is not an array",
            Self::WidthCountMismatch => "simple font /Widths length does not match its range",
            Self::InvalidNumber => "font metric is not a finite number",
            Self::InvalidCidWidths => "CIDFont /W metrics are malformed",
            Self::AmbiguousCidWidths => "CIDFont /W metrics overlap",
            Self::InvalidType0Encoding => "Type0 /Encoding is malformed",
            Self::InvalidEncoding => "simple font /Encoding is malformed",
            Self::UnsupportedBaseEncoding => "simple font /BaseEncoding is not supported",
            Self::PredefinedCMapUnsupported => "selected predefined CMap is unsupported",
            Self::InvalidDescendantFonts => "Type0 /DescendantFonts is not one CIDFont dictionary",
            Self::VerticalWritingUnsupported => "vertical Type0 writing is not implemented",
            Self::SourceSpanFailure => "font source span cannot be resolved",
        })
    }
}

impl std::error::Error for FontError {}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};
    use pdf_syntax::{ObjectParser, ParseLimits};

    use pdf_syntax::Reference;

    use super::{
        CMap, CompositeFont, Descendant, FontError, FontResolver, Type0Encoding, parse_cid_font,
        parse_simple_font, parse_type0_font,
    };

    fn no_indirect_entries() -> &'static FontResolver<'static> {
        &|_| Err(FontError::IndirectEntryUnresolved)
    }

    fn parse(bytes: &[u8]) -> (ByteStore, pdf_syntax::Object) {
        let source = ByteStore::new(SourceId::new(71), Arc::<[u8]>::from(bytes));
        let object = ObjectParser::new(&source, 0, ParseLimits::default())
            .parse_next()
            .unwrap()
            .unwrap();
        (source, object)
    }

    #[test]
    fn preserves_each_source_byte_and_uses_declared_widths() {
        let (source, object) = parse(
            b"<< /Type /Font /Subtype /TrueType /FirstChar 32 /LastChar 34 /Widths [250 500 750] >>",
        );
        let font = parse_simple_font(&source, &object, no_indirect_entries())
            .expect("simple font metrics");
        let codes = font.source_codes(&[32, 34, 200]);
        assert_eq!(codes[0].bytes, vec![32]);
        assert_eq!(codes[0].byte_offset, 0);
        assert!((codes[0].width - 250.0).abs() < f64::EPSILON);
        assert!((codes[1].width - 750.0).abs() < f64::EPSILON);
        assert!((codes[2].width - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn an_encoding_names_codes_from_a_base_and_then_its_differences() {
        let (source, object) = parse(
            b"<< /Subtype /Type1 /FirstChar 32 /LastChar 32 /Widths [250]\
              /Encoding << /BaseEncoding /WinAnsiEncoding\
              /Differences [65 /alpha /beta 200 /gamma] >> >>",
        );
        let font = parse_simple_font(&source, &object, no_indirect_entries()).expect("simple font");
        assert_eq!(font.glyph_name(65), Some(&b"alpha"[..]));
        assert_eq!(font.glyph_name(66), Some(&b"beta"[..]));
        assert_eq!(font.glyph_name(200), Some(&b"gamma"[..]));
        assert_eq!(font.glyph_name(67), Some(&b"C"[..]));
        assert_eq!(font.glyph_name(0xE9), Some(&b"eacute"[..]));
        assert_eq!(font.glyph_name(0x27), Some(&b"quotesingle"[..]));
        assert_eq!(font.glyph_name(0x60), Some(&b"grave"[..]));
    }

    #[test]
    fn a_base_encodings_second_space_and_hyphen_mean_what_appendix_d_says() {
        let font_with = |encoding: &[u8]| {
            let mut bytes =
                b"<< /Subtype /TrueType /FirstChar 32 /LastChar 32 /Widths [250] /Encoding "
                    .to_vec();
            bytes.extend_from_slice(encoding);
            bytes.extend_from_slice(b" >>");
            let (source, object) = parse(&bytes);
            parse_simple_font(&source, &object, no_indirect_entries()).expect("simple font")
        };

        let win = font_with(b"/WinAnsiEncoding");
        assert_eq!(win.standard_duplicate(0xA0), Some('\u{a0}'));
        assert_eq!(win.standard_duplicate(0xAD), Some('\u{ad}'));
        assert_eq!(win.standard_duplicate(0x20), None, "the space itself");
        assert_eq!(win.standard_duplicate(0x2D), None, "the hyphen itself");
        assert_eq!(
            win.glyph_name(0xA0),
            Some(&b"space"[..]),
            "and it is still drawn with the space glyph"
        );

        let renamed = font_with(b"<< /BaseEncoding /WinAnsiEncoding /Differences [160 /space] >>");
        assert_eq!(
            renamed.standard_duplicate(0xA0),
            None,
            "a code the file renamed is the file's statement"
        );
        assert_eq!(renamed.standard_duplicate(0xAD), Some('\u{ad}'));

        let mac = font_with(b"/MacRomanEncoding");
        assert_eq!(mac.standard_duplicate(0xCA), Some('\u{a0}'));
        assert_eq!(mac.standard_duplicate(0xA0), None);

        let standard = font_with(b"/StandardEncoding");
        assert!((0..=u8::MAX).all(|code| standard.standard_duplicate(code).is_none()));
    }

    #[test]
    fn an_encoding_may_be_a_bare_name_or_differences_alone() {
        let (source, object) = parse(
            b"<< /Subtype /Type1 /FirstChar 32 /LastChar 32 /Widths [250]\
              /Encoding /StandardEncoding >>",
        );
        let font = parse_simple_font(&source, &object, no_indirect_entries()).expect("named");
        assert_eq!(font.glyph_name(0x27), Some(&b"quoteright"[..]));
        assert_eq!(font.glyph_name(0x60), Some(&b"quoteleft"[..]));

        let (source, object) = parse(
            b"<< /Subtype /Type1 /FirstChar 32 /LastChar 32 /Widths [250]\
              /Encoding << /Differences [65 /alpha] >> >>",
        );
        let font = parse_simple_font(&source, &object, no_indirect_entries()).expect("differences");
        assert_eq!(font.glyph_name(65), Some(&b"alpha"[..]));
        assert_eq!(font.glyph_name(66), None);

        let (source, object) =
            parse(b"<< /Subtype /Type1 /FirstChar 32 /LastChar 32 /Widths [250] >>");
        let font = parse_simple_font(&source, &object, no_indirect_entries()).expect("none");
        assert_eq!(font.glyph_name(65), None);
    }

    #[test]
    fn malformed_and_unsupported_encodings_fail_closed() {
        for (dictionary, expected) in [
            (
                &b"/Encoding << /BaseEncoding /MacExpertEncoding >>"[..],
                FontError::UnsupportedBaseEncoding,
            ),
            (
                b"/Encoding /NotAnEncoding",
                FontError::UnsupportedBaseEncoding,
            ),
            (
                b"/Encoding << /Differences [/alpha] >>",
                FontError::InvalidEncoding,
            ),
            (
                b"/Encoding << /Differences [256 /alpha] >>",
                FontError::InvalidEncoding,
            ),
            (
                b"/Encoding << /Differences [65 (alpha)] >>",
                FontError::InvalidEncoding,
            ),
            (
                b"/Encoding << /Differences 5 >>",
                FontError::InvalidEncoding,
            ),
            (b"/Encoding [/WinAnsiEncoding]", FontError::InvalidEncoding),
        ] {
            let mut bytes =
                b"<< /Subtype /Type1 /FirstChar 32 /LastChar 32 /Widths [250] ".to_vec();
            bytes.extend_from_slice(dictionary);
            bytes.extend_from_slice(b" >>");
            let (source, object) = parse(&bytes);
            assert_eq!(
                parse_simple_font(&source, &object, no_indirect_entries()),
                Err(expected),
                "{}",
                String::from_utf8_lossy(dictionary)
            );
        }
    }

    #[test]
    fn rejects_ambiguous_metrics_and_composite_fonts_explicitly() {
        let (source, duplicate) =
            parse(b"<< /Subtype /Type1 /FirstChar 0 /FirstChar 1 /LastChar 1 /Widths [2] >>");
        assert_eq!(
            parse_simple_font(&source, &duplicate, no_indirect_entries()),
            Err(FontError::DuplicateEntry)
        );
        let (source, composite) = parse(b"<< /Subtype /Type0 >>");
        assert_eq!(
            parse_simple_font(&source, &composite, no_indirect_entries()),
            Err(FontError::CompositeFontUnsupported)
        );
    }

    #[test]
    fn cid_widths_drive_identity_h_codes_without_inventing_unicode() {
        let (source, object) =
            parse(b"<< /Subtype /CIDFontType2 /DW 900 /W [40 42 700 3 [250 500]] >>");
        let descendant =
            parse_cid_font(&source, &object, no_indirect_entries()).expect("CID metrics");
        assert!((descendant.width(3) - 250.0).abs() < f64::EPSILON);
        assert!((descendant.width(4) - 500.0).abs() < f64::EPSILON);
        assert!((descendant.width(41) - 700.0).abs() < f64::EPSILON);
        assert!((descendant.width(99) - 900.0).abs() < f64::EPSILON);

        let font = CompositeFont::new(CMap::identity_horizontal(), descendant, object.span());
        let codes = font.source_codes(&[0, 3, 0, 41]).expect("Identity-H codes");
        assert_eq!(codes[0].bytes, vec![0, 3]);
        assert_eq!(codes[0].cid, Some(3));
        assert_eq!(codes[1].cid, Some(41));
        assert!((codes[1].width - 700.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_type0_encoding_name_is_resolved_or_refused_by_name() {
        let dictionary = |encoding: &[u8]| {
            let mut bytes =
                b"<< /Subtype /Type0 /BaseFont /X /DescendantFonts [7 0 R] /Encoding ".to_vec();
            bytes.extend_from_slice(encoding);
            bytes.extend_from_slice(b" >>");
            bytes
        };
        for (encoding, expected) in [
            (&b"/Identity-H"[..], Type0Encoding::IdentityHorizontal),
            (b"/Identity-V", Type0Encoding::IdentityVertical),
            (
                b"/UniKS-UTF16-H",
                Type0Encoding::Predefined("UniKS-UTF16-H"),
            ),
        ] {
            let bytes = dictionary(encoding);
            let (source, object) = parse(&bytes);
            let font =
                parse_type0_font(&source, &object, no_indirect_entries()).expect("Type0 font");
            assert_eq!(font.encoding, expected);
        }
        for encoding in [&b"/90ms-RKSJ-H"[..], b"/UniKS-UTF16-V", b"/NotAnEncoding"] {
            let bytes = dictionary(encoding);
            let (source, object) = parse(&bytes);
            assert_eq!(
                parse_type0_font(&source, &object, no_indirect_entries()).err(),
                Some(FontError::PredefinedCMapUnsupported),
                "{}",
                String::from_utf8_lossy(encoding)
            );
        }
    }

    #[test]
    fn rejects_overlapping_cid_widths() {
        let (source, object) = parse(b"<< /Subtype /CIDFontType0 /W [1 3 500 3 [600]] >>");
        assert_eq!(
            parse_cid_font(&source, &object, no_indirect_entries()),
            Err(FontError::AmbiguousCidWidths)
        );
    }

    #[test]
    fn a_descendant_written_inline_is_read_like_one_behind_a_reference() {
        let bytes = b"<< /Subtype /Type0 /BaseFont /X /Encoding /Identity-H \
/DescendantFonts [<< /Subtype /CIDFontType2 /BaseFont /X /DW 600 >>] >>";
        let (source, object) = parse(bytes);
        let font = parse_type0_font(&source, &object, no_indirect_entries()).expect("Type0 font");
        let Descendant::Direct(descendant_source, descendant) = font.descendant else {
            panic!("a direct descendant dictionary");
        };
        let cid = parse_cid_font(&descendant_source, &descendant, no_indirect_entries())
            .expect("the inline CIDFont");
        assert!(
            (cid.width(7) - 600.0).abs() < f64::EPSILON,
            "its /DW is read"
        );

        let bytes = b"<< /Subtype /Type0 /BaseFont /X /Encoding /Identity-H \
/DescendantFonts [7 0 R] >>";
        let (source, object) = parse(bytes);
        let font = parse_type0_font(&source, &object, no_indirect_entries()).expect("Type0 font");
        assert!(matches!(
            font.descendant,
            Descendant::Indirect(reference) if reference == Reference::new(7, 0)
        ));
    }

    #[test]
    fn a_descendant_array_of_the_wrong_shape_is_still_refused() {
        for descendants in [
            &b"[]"[..],
            b"[7 0 R 8 0 R]",
            b"[<< /Subtype /CIDFontType2 >> << /Subtype /CIDFontType2 >>]",
            b"[42]",
            b"<< >>",
        ] {
            let mut bytes =
                b"<< /Subtype /Type0 /BaseFont /X /Encoding /Identity-H /DescendantFonts ".to_vec();
            bytes.extend_from_slice(descendants);
            bytes.extend_from_slice(b" >>");
            let (source, object) = parse(&bytes);
            assert_eq!(
                parse_type0_font(&source, &object, no_indirect_entries()).err(),
                Some(FontError::InvalidDescendantFonts),
                "{}",
                String::from_utf8_lossy(descendants)
            );
        }
    }
}
