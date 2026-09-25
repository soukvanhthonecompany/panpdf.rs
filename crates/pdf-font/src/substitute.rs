use std::sync::Arc;

use crate::glyph::GlyphProgram;
use crate::standard14::Standard14;

pub const POLICY_VERSION: &str = "substitute-3";

pub const SUBSTITUTION_POLICY: &str = POLICY_VERSION;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FontFlags(pub u32);

impl FontFlags {
    #[must_use]
    pub const fn fixed_pitch(self) -> bool {
        self.0 & 1 != 0
    }
    #[must_use]
    pub const fn serif(self) -> bool {
        self.0 & (1 << 1) != 0
    }
    #[must_use]
    pub const fn symbolic(self) -> bool {
        self.0 & (1 << 2) != 0
    }
    #[must_use]
    pub const fn script(self) -> bool {
        self.0 & (1 << 3) != 0
    }
    #[must_use]
    pub const fn nonsymbolic(self) -> bool {
        self.0 & (1 << 5) != 0
    }
    #[must_use]
    pub const fn italic(self) -> bool {
        self.0 & (1 << 6) != 0
    }
    #[must_use]
    pub const fn force_bold(self) -> bool {
        self.0 & (1 << 18) != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FontStyle {
    pub weight: u16,
    pub italic: bool,
}

impl Default for FontStyle {
    fn default() -> Self {
        Self {
            weight: 400,
            italic: false,
        }
    }
}

impl FontStyle {
    #[must_use]
    pub const fn is_bold(self) -> bool {
        self.weight >= 600
    }

    #[must_use]
    pub fn distance(self, other: Self) -> u32 {
        let slope = u32::from(self.italic != other.italic) * 10_000;
        slope + u32::from(self.weight.abs_diff(other.weight))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GenericFamily {
    Serif,
    SansSerif,
    Monospace,
    Symbol,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProgramEvidence {
    NotEmbedded,
    NoDescriptor,
    Embedded { key: Vec<u8> },
    Unreadable { key: Vec<u8>, reason: String },
}

impl ProgramEvidence {
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::NotEmbedded => "not embedded",
            Self::NoDescriptor => "no descriptor",
            Self::Embedded { .. } => "embedded",
            Self::Unreadable { .. } => "embedded but unreadable",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FontRequest {
    pub base_font: Vec<u8>,
    pub family: String,
    pub style: FontStyle,
    pub subtype: Vec<u8>,
    pub cid_subtype: Option<Vec<u8>>,
    pub registry: Option<String>,
    pub ordering: Option<String>,
    pub encoding: Option<String>,
    pub flags: FontFlags,
    pub italic_angle: Option<f64>,
    pub ascent: Option<f64>,
    pub descent: Option<f64>,
    pub standard_face: Option<Standard14>,
    pub program: ProgramEvidence,
}

impl FontRequest {
    #[must_use]
    pub fn has_ambiguous_cjk_encoding(&self) -> bool {
        self.subtype == b"/TrueType"
            && self.flags.symbolic()
            && self
                .encoding
                .as_deref()
                .is_some_and(|name| name.trim_start_matches('/') == "WinAnsiEncoding")
            && !byte_family_aliases(&self.base_font).is_empty()
    }

    #[must_use]
    pub fn for_family(family: &str, style: FontStyle) -> Self {
        Self {
            base_font: family.replace(' ', "").into_bytes(),
            family: family.to_owned(),
            style,
            subtype: b"/Type0".to_vec(),
            cid_subtype: Some(b"/CIDFontType2".to_vec()),
            registry: None,
            ordering: None,
            encoding: None,
            flags: FontFlags(0),
            italic_angle: None,
            ascent: None,
            descent: None,
            standard_face: None,
            program: ProgramEvidence::NotEmbedded,
        }
    }

    #[must_use]
    pub fn generic(&self) -> GenericFamily {
        if let Some(face) = self.standard_face {
            return match face {
                Standard14::Symbol | Standard14::ZapfDingbats => GenericFamily::Symbol,
                Standard14::Courier
                | Standard14::CourierBold
                | Standard14::CourierBoldOblique
                | Standard14::CourierOblique => GenericFamily::Monospace,
                Standard14::TimesRoman
                | Standard14::TimesBold
                | Standard14::TimesItalic
                | Standard14::TimesBoldItalic => GenericFamily::Serif,
                _ => GenericFamily::SansSerif,
            };
        }
        if self.flags.fixed_pitch() {
            return GenericFamily::Monospace;
        }
        if self.flags.serif() {
            return GenericFamily::Serif;
        }
        GenericFamily::SansSerif
    }

    #[must_use]
    pub fn is_composite(&self) -> bool {
        self.cid_subtype.is_some()
    }

    #[must_use]
    pub fn code_is_utf16(&self) -> bool {
        self.encoding.as_ref().is_some_and(|name| {
            let name = name.strip_prefix('/').unwrap_or(name);
            matches!(
                name,
                "UniGB-UTF16-H"
                    | "UniGB-UTF16-V"
                    | "UniCNS-UTF16-H"
                    | "UniCNS-UTF16-V"
                    | "UniJIS-UTF16-H"
                    | "UniJIS-UTF16-V"
                    | "UniKS-UTF16-H"
                    | "UniKS-UTF16-V"
            )
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubstitutionReason {
    ExactFamily,
    AliasedFamily,
    StandardFace,
    Generic,
    ScriptCoverage,
}

impl std::fmt::Display for SubstitutionReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::ExactFamily => "exact family",
            Self::AliasedFamily => "aliased family",
            Self::StandardFace => "standard face",
            Self::Generic => "generic shape",
            Self::ScriptCoverage => "script coverage",
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FaceIdentity {
    pub family: String,
    pub subfamily: String,
    pub origin: String,
    pub sha256: String,
    pub face_index: u32,
    pub style: FontStyle,
}

#[derive(Clone, Debug)]
pub struct SubstitutedFace {
    pub program: Arc<GlyphProgram>,
    pub identity: Arc<FaceIdentity>,
    pub reason: SubstitutionReason,
}

impl PartialEq for SubstitutedFace {
    fn eq(&self, other: &Self) -> bool {
        self.identity == other.identity && self.reason == other.reason
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum MappingRoute {
    EncodingGlyphName,
    ToUnicode,
    Utf16Code,
    StandardEncoding,
    StandardFaceEncoding,
    RecoveredLegacyEncoding,
}

impl MappingRoute {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::EncodingGlyphName => "encoding glyph name",
            Self::ToUnicode => "to-unicode",
            Self::Utf16Code => "utf-16 code",
            Self::StandardEncoding => "standard encoding",
            Self::StandardFaceEncoding => "standard face encoding",
            Self::RecoveredLegacyEncoding => "recovered legacy encoding",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum UnresolvedReason {
    NoEvidence,
    AmbiguousEncoding,
    MultibyteContinuation,
    TruncatedMultibyte,
    UnmappedMultibyte,
    ClusterTooLong,
    MultipleBaseCharacters,
    NoFaceCoverage,
}

impl UnresolvedReason {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::NoEvidence => "no evidence",
            Self::AmbiguousEncoding => "ambiguous encoding",
            Self::MultibyteContinuation => "multibyte continuation",
            Self::TruncatedMultibyte => "truncated multibyte",
            Self::UnmappedMultibyte => "unmapped multibyte",
            Self::ClusterTooLong => "cluster too long",
            Self::MultipleBaseCharacters => "multiple base characters",
            Self::NoFaceCoverage => "no face coverage",
        }
    }

    #[must_use]
    pub const fn is_evidence_gap(self) -> bool {
        !matches!(self, Self::NoFaceCoverage | Self::MultibyteContinuation)
    }

    #[must_use]
    pub const fn follows_an_established_meaning(self) -> bool {
        matches!(
            self,
            Self::NoFaceCoverage | Self::ClusterTooLong | Self::MultipleBaseCharacters
        )
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MappingCensus {
    pub source_codes: u32,
    pub routes: Vec<(MappingRoute, u32)>,
    pub unresolved: Vec<(UnresolvedReason, u32)>,
    pub clusters_drawn: u32,
    pub outlines_drawn: u32,
}

impl MappingCensus {
    #[must_use]
    pub fn mapped_codes(&self) -> u32 {
        self.routes.iter().map(|(_, count)| count).sum()
    }

    #[must_use]
    pub fn unresolved_codes(&self) -> u32 {
        self.unresolved.iter().map(|(_, count)| count).sum()
    }

    #[must_use]
    pub fn codes_without_a_route(&self) -> u32 {
        self.unresolved
            .iter()
            .filter(|(reason, _)| !reason.follows_an_established_meaning())
            .map(|(_, count)| count)
            .sum()
    }

    #[must_use]
    pub fn evidence_gaps(&self) -> u32 {
        self.unresolved
            .iter()
            .filter(|(reason, _)| reason.is_evidence_gap())
            .map(|(_, count)| count)
            .sum()
    }

    pub fn count_route(&mut self, route: MappingRoute) {
        match self
            .routes
            .binary_search_by_key(&route, |(found, _)| *found)
        {
            Ok(index) => self.routes[index].1 += 1,
            Err(index) => self.routes.insert(index, (route, 1)),
        }
    }

    pub fn count_unresolved(&mut self, reason: UnresolvedReason) {
        match self
            .unresolved
            .binary_search_by_key(&reason, |(found, _)| *found)
        {
            Ok(index) => self.unresolved[index].1 += 1,
            Err(index) => self.unresolved.insert(index, (reason, 1)),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FontSubstitution {
    pub request: Arc<FontRequest>,
    pub primary: SubstitutedFace,
    pub fallbacks: Vec<SubstitutedFace>,
    pub unmapped: Vec<(char, u32)>,
    pub unknown_codes: u32,
    pub census: MappingCensus,
    pub policy: &'static str,
}

pub trait FontProvider: Send + Sync + std::fmt::Debug {
    fn primary_face(&self, request: &FontRequest) -> Option<SubstitutedFace>;

    fn fallback_face(&self, request: &FontRequest, character: char) -> Option<SubstitutedFace>;

    fn description(&self) -> String;

    fn decipher_regions(&self, page: pdf_syntax::Reference) -> Vec<[f64; 4]> {
        let _ = page;
        Vec::new()
    }
}

#[derive(Debug)]
pub struct DecipherScope {
    inner: Arc<dyn FontProvider>,
    regions: std::sync::RwLock<std::collections::HashMap<pdf_syntax::Reference, Vec<[f64; 4]>>>,
}

impl DecipherScope {
    #[must_use]
    pub fn new(inner: Arc<dyn FontProvider>) -> Self {
        Self {
            inner,
            regions: std::sync::RwLock::default(),
        }
    }

    pub fn allow(&self, page: pdf_syntax::Reference, region: [f64; 4]) -> bool {
        let Ok(mut regions) = self.regions.write() else {
            return false;
        };
        let held = regions.entry(page).or_default();
        let covered = held.iter().any(|known| {
            known[0] <= region[0]
                && known[1] <= region[1]
                && known[2] >= region[2]
                && known[3] >= region[3]
        });
        if !covered {
            held.push(region);
        }
        !covered
    }
}

impl FontProvider for DecipherScope {
    fn primary_face(&self, request: &FontRequest) -> Option<SubstitutedFace> {
        self.inner.primary_face(request)
    }

    fn fallback_face(&self, request: &FontRequest, character: char) -> Option<SubstitutedFace> {
        self.inner.fallback_face(request, character)
    }

    fn description(&self) -> String {
        self.inner.description()
    }

    fn decipher_regions(&self, page: pdf_syntax::Reference) -> Vec<[f64; 4]> {
        self.regions
            .read()
            .ok()
            .and_then(|regions| regions.get(&page).cloned())
            .unwrap_or_default()
    }
}

#[must_use]
pub fn normalize_name(name: &str) -> String {
    let name = strip_subset_prefix(name);
    let name = match name.find('+') {
        Some(0) | None => name,
        Some(position) => &name[..position],
    };
    name.chars()
        .filter(|character| !matches!(character, ' ' | '-' | ',' | '_'))
        .flat_map(char::to_lowercase)
        .collect()
}

#[must_use]
pub fn character_for_glyph_name(name: &[u8]) -> Option<char> {
    crate::tables::unicode_for_glyph_name(name).and_then(char::from_u32)
}

#[must_use]
pub fn standard_encoding_name(code: u8) -> Option<&'static [u8]> {
    crate::tables::STANDARD_ENCODING[usize::from(code)]
}

#[must_use]
pub fn is_combining_mark(character: char) -> bool {
    matches!(
        u32::from(character),
        0x0300..=0x036F
            | 0x0483..=0x0489
            | 0x0591..=0x05BD
            | 0x05BF
            | 0x05C1..=0x05C2
            | 0x05C4..=0x05C5
            | 0x0610..=0x061A
            | 0x064B..=0x065F
            | 0x0670
            | 0x06D6..=0x06DC
            | 0x06DF..=0x06E4
            | 0x06E7..=0x06E8
            | 0x06EA..=0x06ED
            | 0x0711
            | 0x0730..=0x074A
            | 0x07A6..=0x07B0
            | 0x0900..=0x0902
            | 0x093A
            | 0x093C
            | 0x0941..=0x0948
            | 0x094D
            | 0x0951..=0x0957
            | 0x0962..=0x0963
            | 0x0E31
            | 0x0E34..=0x0E3A
            | 0x0E47..=0x0E4E
            | 0x0EB1
            | 0x0EB4..=0x0EBC
            | 0x0EC8..=0x0ECD
            | 0x0F18..=0x0F19
            | 0x0F35
            | 0x0F37
            | 0x0F39
            | 0x0F71..=0x0F84
            | 0x0F86..=0x0F87
            | 0x0F8D..=0x0FBC
            | 0x102D..=0x1030
            | 0x1032..=0x1037
            | 0x1039..=0x103A
            | 0x1058..=0x1059
            | 0x17B7..=0x17BD
            | 0x17C6
            | 0x17C9..=0x17D3
            | 0x1AB0..=0x1AFF
            | 0x1DC0..=0x1DFF
            | 0x20D0..=0x20F0
            | 0xFE00..=0xFE0F
            | 0xFE20..=0xFE2F
    )
}

#[must_use]
pub fn is_ignorable(character: char) -> bool {
    matches!(
        u32::from(character),
        0x00AD | 0x200B..=0x200F | 0x2028..=0x202E | 0x2060..=0x206F | 0xFEFF
    )
}

fn strip_subset_prefix(name: &str) -> &str {
    let bytes = name.as_bytes();
    if bytes.len() > 7 && bytes[6] == b'+' && bytes[..6].iter().all(u8::is_ascii_uppercase) {
        return &name[7..];
    }
    name
}

const STYLE_WORDS: &[(&str, u16, bool)] = &[
    ("bolditalic", 700, true),
    ("boldoblique", 700, true),
    ("semibold", 600, false),
    ("extrabold", 800, false),
    ("ultrabold", 800, false),
    ("demibold", 600, false),
    ("blackitalic", 900, true),
    ("lightitalic", 300, true),
    ("mediumitalic", 500, true),
    ("italic", 400, true),
    ("oblique", 400, true),
    ("bold", 700, false),
    ("black", 900, false),
    ("heavy", 900, false),
    ("medium", 500, false),
    ("light", 300, false),
    ("thin", 100, false),
    ("regular", 400, false),
    ("roman", 400, false),
    ("book", 400, false),
    ("reguital", 400, true),
    ("mediital", 500, true),
    ("regu", 400, false),
    ("medi", 500, false),
    ("bol", 700, false),
    ("ital", 400, true),
    ("obli", 400, true),
    ("reg", 400, false),
];

#[must_use]
pub fn split_family_style(base_font: &[u8]) -> (String, FontStyle) {
    let text = String::from_utf8_lossy(base_font);
    let text = text.strip_prefix('/').unwrap_or(&text);
    let text = strip_subset_prefix(text);
    let (family, style_text) = if let Some(position) = text.find(',') {
        (&text[..position], Some(&text[position + 1..]))
    } else if let Some(position) = text.rfind('-') {
        (&text[..position], Some(&text[position + 1..]))
    } else {
        (text, None)
    };
    let mut style = FontStyle::default();
    let mut family = family.to_owned();
    if let Some(declared) = style_text {
        let normalized = normalize_name(declared);
        if let Some(found) = match_style_word(&normalized) {
            style = found;
        } else {
            text.clone_into(&mut family);
        }
    }
    if style == FontStyle::default() {
        let normalized = normalize_name(&family);
        for (word, weight, italic) in STYLE_WORDS {
            if normalized.len() > word.len() && normalized.ends_with(word) {
                style = FontStyle {
                    weight: *weight,
                    italic: *italic,
                };
                family.truncate(family.len() - trailing_len(&family, word.len()));
                break;
            }
        }
    }
    (family.trim().to_owned(), style)
}

fn trailing_len(family: &str, normalized_len: usize) -> usize {
    let mut seen = 0;
    let mut bytes = 0;
    for character in family.chars().rev() {
        bytes += character.len_utf8();
        if !matches!(character, ' ' | '-' | ',' | '_') {
            seen += character.to_lowercase().count();
        }
        if seen >= normalized_len {
            break;
        }
    }
    bytes
}

fn match_style_word(normalized: &str) -> Option<FontStyle> {
    STYLE_WORDS
        .iter()
        .find(|(word, _, _)| *word == normalized)
        .map(|(_, weight, italic)| FontStyle {
            weight: *weight,
            italic: *italic,
        })
}

#[must_use]
pub fn byte_family_aliases(base_font: &[u8]) -> &'static [&'static str] {
    let name = base_font.strip_prefix(b"/").unwrap_or(base_font);
    let name = if name.len() > 7 && name[6] == b'+' {
        &name[7..]
    } else {
        name
    };
    for (bytes, candidates) in BYTE_FAMILY_ALIASES {
        if *bytes == name {
            return candidates;
        }
    }
    &[]
}

const BYTE_FAMILY_ALIASES: &[(&[u8], &[&str])] = &[
    (
        &[0xCB, 0xCE, 0xCC, 0xE5],
        &[
            "SimSun",
            "Noto Serif CJK SC",
            "Noto Sans CJK SC",
            "Droid Sans Fallback",
        ],
    ),
    (
        &[0xBA, 0xDA, 0xCC, 0xE5],
        &["SimHei", "Noto Sans CJK SC", "Droid Sans Fallback"],
    ),
    (
        &[0xBF, 0xAC, 0xCC, 0xE5],
        &["KaiTi", "Noto Serif CJK SC", "Noto Sans CJK SC"],
    ),
    (
        &[0xB7, 0xC2, 0xCB, 0xCE],
        &["FangSong", "Noto Serif CJK SC", "Noto Sans CJK SC"],
    ),
    (
        &[
            0xB7, 0xC2, 0xCB, 0xCE, b'_', b'G', b'B', b'2', b'3', b'1', b'2',
        ],
        &["FangSong", "Noto Serif CJK SC", "Noto Sans CJK SC"],
    ),
    (
        &[
            0xBF, 0xAC, 0xCC, 0xE5, b'_', b'G', b'B', b'2', b'3', b'1', b'2',
        ],
        &["KaiTi", "Noto Serif CJK SC", "Noto Sans CJK SC"],
    ),
    (
        &[0xC1, 0xA5, 0xCA, 0xE9],
        &["SimLi", "Noto Serif CJK SC", "Noto Sans CJK SC"],
    ),
    (
        &[0xD0, 0xC2, 0xCB, 0xCE],
        &["NSimSun", "Noto Serif CJK SC", "Noto Sans CJK SC"],
    ),
];

#[must_use]
pub fn family_aliases(normalized_family: &str) -> &'static [&'static str] {
    for (name, candidates) in FAMILY_ALIASES {
        if *name == normalized_family {
            return candidates;
        }
    }
    &[]
}

#[must_use]
pub fn standard_face_aliases(face: Standard14) -> &'static [&'static str] {
    match face {
        Standard14::TimesRoman
        | Standard14::TimesBold
        | Standard14::TimesItalic
        | Standard14::TimesBoldItalic => SERIF_FACES,
        Standard14::Courier
        | Standard14::CourierBold
        | Standard14::CourierBoldOblique
        | Standard14::CourierOblique => MONO_FACES,
        Standard14::Symbol => SYMBOL_FACES,
        Standard14::ZapfDingbats => DINGBAT_FACES,
        _ => SANS_FACES,
    }
}

#[must_use]
pub const fn generic_faces(generic: GenericFamily) -> &'static [&'static str] {
    match generic {
        GenericFamily::Serif => SERIF_FACES,
        GenericFamily::SansSerif => SANS_FACES,
        GenericFamily::Monospace => MONO_FACES,
        GenericFamily::Symbol => SYMBOL_FACES,
    }
}

const SERIF_FACES: &[&str] = &[
    "Times New Roman",
    "Liberation Serif",
    "Tinos",
    "Nimbus Roman",
    "Nimbus Roman No9 L",
    "TeX Gyre Termes",
    "FreeSerif",
    "DejaVu Serif",
    "Noto Serif",
];

const SANS_FACES: &[&str] = &[
    "Arial",
    "Helvetica",
    "Liberation Sans",
    "Arimo",
    "Nimbus Sans",
    "Nimbus Sans L",
    "TeX Gyre Heros",
    "FreeSans",
    "DejaVu Sans",
    "Noto Sans",
];

const MONO_FACES: &[&str] = &[
    "Courier New",
    "Liberation Mono",
    "Cousine",
    "Nimbus Mono PS",
    "Nimbus Mono L",
    "FreeMono",
    "DejaVu Sans Mono",
    "Noto Sans Mono",
];

const SYMBOL_FACES: &[&str] = &[
    "Standard Symbols PS",
    "Standard Symbols L",
    "Symbol",
    "OpenSymbol",
    "DejaVu Sans",
];

const DINGBAT_FACES: &[&str] = &["D050000L", "Dingbats", "ZapfDingbats", "OpenSymbol"];

const FAMILY_ALIASES: &[(&str, &[&str])] = &[
    ("arial", SANS_FACES),
    ("arialmt", SANS_FACES),
    ("arialunicodems", SANS_FACES),
    ("helvetica", SANS_FACES),
    ("helveticaneue", SANS_FACES),
    ("verdana", &["Verdana", "DejaVu Sans", "Liberation Sans"]),
    ("tahoma", &["Tahoma", "DejaVu Sans", "Liberation Sans"]),
    ("segoeui", &["Segoe UI", "Noto Sans", "Liberation Sans"]),
    ("calibri", &["Calibri", "Carlito", "Liberation Sans"]),
    ("cambria", &["Cambria", "Caladea", "Liberation Serif"]),
    ("candara", &["Candara", "Noto Sans", "Liberation Sans"]),
    (
        "consolas",
        &["Consolas", "Liberation Mono", "DejaVu Sans Mono"],
    ),
    ("georgia", &["Georgia", "Gelasio", "Liberation Serif"]),
    ("timesnewroman", SERIF_FACES),
    ("timesnewromanps", SERIF_FACES),
    ("timesnewromanpsmt", SERIF_FACES),
    ("times", SERIF_FACES),
    ("timesroman", SERIF_FACES),
    ("garamond", &["Garamond", "EB Garamond", "Liberation Serif"]),
    ("bookantiqua", &["Book Antiqua", "P052", "Liberation Serif"]),
    ("palatino", &["Palatino", "P052", "Liberation Serif"]),
    ("palatinolinotype", &["Palatino Linotype", "P052"]),
    ("centuryschoolbook", &["Century Schoolbook L", "C059"]),
    ("bookman", &["Bookman", "URW Bookman", "URW Bookman L"]),
    ("bookmanoldstyle", &["URW Bookman", "URW Bookman L"]),
    ("avantgarde", &["URW Gothic", "URW Gothic L"]),
    ("newcenturyschlbk", &["C059", "Century Schoolbook L"]),
    ("zapfchancery", &["Z003", "URW Chancery L"]),
    ("couriernew", MONO_FACES),
    ("couriernewpsmt", MONO_FACES),
    ("courier", MONO_FACES),
    ("couriernewps", MONO_FACES),
    (
        "cordiaupc",
        &["Noto Sans Thai", "Noto Sans Thai Looped", "Waree"],
    ),
    (
        "cordianew",
        &["Noto Sans Thai", "Noto Sans Thai Looped", "Waree"],
    ),
    ("angsanaupc", &["Noto Serif Thai", "Norasi"]),
    ("angsananew", &["Noto Serif Thai", "Norasi"]),
    ("browalliaupc", &["Noto Sans Thai", "Garuda"]),
    ("browallianew", &["Noto Sans Thai", "Garuda"]),
    (
        "dokchampa",
        &["Noto Sans Lao", "Phetsarath OT", "Noto Sans Lao Looped"],
    ),
    ("saysettha", &["Noto Sans Lao", "Phetsarath OT"]),
    ("saysetthaot", &["Noto Sans Lao", "Phetsarath OT"]),
    ("phetsarath", &["Phetsarath OT", "Noto Sans Lao"]),
    ("phetsarathot", &["Phetsarath OT", "Noto Sans Lao"]),
    ("laoui", &["Noto Sans Lao", "Phetsarath OT"]),
    (
        "simsun",
        &["SimSun", "Noto Serif CJK SC", "Noto Sans CJK SC"],
    ),
    ("simhei", &["SimHei", "Noto Sans CJK SC"]),
    ("mssong", &["Noto Serif CJK SC", "Noto Sans CJK SC"]),
    ("msmincho", &["Noto Serif CJK JP", "Noto Sans CJK JP"]),
    ("msgothic", &["Noto Sans CJK JP"]),
    ("heiseimin", &["Noto Serif CJK JP", "Noto Sans CJK JP"]),
    ("heiseikakugo", &["Noto Sans CJK JP"]),
    ("batang", &["Noto Serif CJK KR", "Noto Sans CJK KR"]),
    ("gulim", &["Noto Sans CJK KR"]),
    (
        "centurygothic",
        &[
            "Century Gothic",
            "URW Gothic",
            "URW Gothic L",
            "Muli",
            "Liberation Sans",
        ],
    ),
    ("nimbusromno9l", SERIF_FACES),
    ("nimbusroman", SERIF_FACES),
    ("nimbussanl", SANS_FACES),
    ("nimbussans", SANS_FACES),
    ("nimbusmonl", MONO_FACES),
    ("nimbusmono", MONO_FACES),
    ("urwpalladiol", &["P052", "Palatino", "Liberation Serif"]),
    (
        "opensans",
        &["Open Sans", "Noto Sans", "DejaVu Sans", "Liberation Sans"],
    ),
    ("lucidatypewriter", MONO_FACES),
    ("lettergothic", MONO_FACES),
    (
        "arialnarrow",
        &[
            "Arial Narrow",
            "Liberation Sans Narrow",
            "Nimbus Sans Narrow",
            "Liberation Sans",
        ],
    ),
    (
        "arialblack",
        &["Arial Black", "Liberation Sans", "DejaVu Sans"],
    ),
    (
        "traditionalarabic",
        &["Noto Naskh Arabic", "Noto Sans Arabic", "FreeSerif"],
    ),
    ("cmr", SERIF_FACES),
    ("cmb", SERIF_FACES),
    ("cmbx", SERIF_FACES),
    ("cmti", SERIF_FACES),
    ("cmss", SANS_FACES),
    ("cmssbx", SANS_FACES),
    ("cmtt", MONO_FACES),
    ("cmmi", &["DejaVu Serif", "FreeSerif", "Noto Serif"]),
    ("cmsy", &["DejaVu Sans", "Noto Sans Math", "FreeSerif"]),
    ("cmex", &["DejaVu Sans", "Noto Sans Math", "FreeSerif"]),
    ("beraserif", SERIF_FACES),
    ("berasans", SANS_FACES),
    ("beramono", MONO_FACES),
    ("wingdings", &["Wingdings", "OpenSymbol", "Dingbats"]),
    ("symbol", SYMBOL_FACES),
    ("zapfdingbats", DINGBAT_FACES),
];

#[must_use]
pub fn coverage_faces(character: char) -> &'static [&'static str] {
    match u32::from(character) {
        0x0E80..=0x0EFF => &[
            "Noto Sans Lao",
            "Noto Serif Lao",
            "Noto Looped Lao",
            "Phetsarath OT",
        ],
        0x0E00..=0x0E7F => &[
            "Noto Sans Thai",
            "Noto Serif Thai",
            "Noto Looped Thai",
            "Garuda",
        ],
        0x1780..=0x17FF => &["Noto Sans Khmer", "Noto Serif Khmer"],
        0x0F00..=0x0FFF => &["Noto Serif Tibetan", "Noto Sans Tibetan"],
        0x1000..=0x109F => &["Noto Sans Myanmar", "Noto Serif Myanmar"],
        0x0900..=0x097F => &["Noto Sans Devanagari", "Noto Serif Devanagari"],
        0x0980..=0x09FF => &["Noto Sans Bengali", "Noto Serif Bengali", "FreeSerif"],
        0x0A00..=0x0A7F => &["Noto Sans Gurmukhi", "Noto Serif Gurmukhi", "FreeSerif"],
        0x0A80..=0x0AFF => &["Noto Sans Gujarati", "Noto Serif Gujarati", "FreeSerif"],
        0x0B00..=0x0B7F => &["Noto Sans Oriya", "Noto Serif Oriya", "FreeSerif"],
        0x0B80..=0x0BFF => &["Noto Sans Tamil", "Noto Serif Tamil", "FreeSerif"],
        0x0C00..=0x0C7F => &["Noto Sans Telugu", "Noto Serif Telugu", "FreeSerif"],
        0x0C80..=0x0CFF => &["Noto Sans Kannada", "Noto Serif Kannada", "FreeSerif"],
        0x0D00..=0x0D7F => &["Noto Sans Malayalam", "Noto Serif Malayalam", "FreeSerif"],
        0x0D80..=0x0DFF => &["Noto Sans Sinhala", "Noto Serif Sinhala", "FreeSerif"],
        0x1200..=0x139F => &["Noto Sans Ethiopic", "Noto Serif Ethiopic", "FreeSerif"],
        0x0530..=0x058F => &["Noto Sans Armenian", "Noto Serif Armenian", "DejaVu Sans"],
        0x10A0..=0x10FF => &["Noto Sans Georgian", "Noto Serif Georgian", "DejaVu Sans"],
        0x0600..=0x06FF | 0x0750..=0x077F => &["Noto Naskh Arabic", "Noto Sans Arabic"],
        0x0590..=0x05FF => &["Noto Sans Hebrew", "Noto Serif Hebrew"],
        0x0370..=0x052F => &["DejaVu Sans", "Liberation Sans", "Noto Sans"],
        0x1100..=0x11FF | 0xAC00..=0xD7AF => {
            &["Noto Sans CJK KR", "Noto Serif CJK KR", "Noto Sans CJK SC"]
        }
        0x3040..=0x30FF | 0x31F0..=0x31FF => {
            &["Noto Sans CJK JP", "Noto Serif CJK JP", "Noto Sans CJK SC"]
        }
        0x2E80..=0x2EFF
        | 0x3000..=0x303F
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE1F
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFFEF => &[
            "Noto Sans CJK SC",
            "Noto Serif CJK SC",
            "Noto Sans CJK TC",
            "Droid Sans Fallback",
        ],
        0x2000..=0x2BFF => &[
            "DejaVu Sans",
            "Noto Sans Symbols 2",
            "Noto Sans Math",
            "FreeSerif",
            "OpenSymbol",
        ],
        _ => &["DejaVu Sans", "FreeSerif", "Noto Sans", "Liberation Sans"],
    }
}

#[cfg(test)]
mod tests {
    #[derive(Debug)]
    struct NoFaces;

    impl FontProvider for NoFaces {
        fn primary_face(&self, _request: &FontRequest) -> Option<SubstitutedFace> {
            None
        }

        fn fallback_face(
            &self,
            _request: &FontRequest,
            _character: char,
        ) -> Option<SubstitutedFace> {
            None
        }

        fn description(&self) -> String {
            "none".to_owned()
        }
    }

    #[test]
    fn a_decipher_scope_holds_only_the_regions_it_was_given() {
        let scope = DecipherScope::new(std::sync::Arc::new(NoFaces));
        let (page, other) = (
            pdf_syntax::Reference::new(3, 0),
            pdf_syntax::Reference::new(4, 0),
        );
        assert!(scope.decipher_regions(page).is_empty());
        assert!(
            NoFaces.decipher_regions(page).is_empty(),
            "a provider allows nothing"
        );
        assert!(scope.allow(page, [0.0, 0.0, 100.0, 50.0]));
        assert!(
            !scope.allow(page, [10.0, 10.0, 90.0, 40.0]),
            "already covered"
        );
        assert!(scope.allow(page, [0.0, 60.0, 100.0, 80.0]));
        assert_eq!(scope.decipher_regions(page).len(), 2);
        assert!(scope.decipher_regions(other).is_empty());
    }

    use super::*;

    #[test]
    fn comma_separates_family_from_style() {
        let (family, style) = split_family_style(b"Arial,BoldItalic");
        assert_eq!(family, "Arial");
        assert_eq!(
            style,
            FontStyle {
                weight: 700,
                italic: true
            }
        );
    }

    #[test]
    fn hyphen_separates_family_from_style() {
        let (family, style) = split_family_style(b"Times-BoldItalic");
        assert_eq!(family, "Times");
        assert!(style.italic && style.is_bold());
    }

    #[test]
    fn a_hyphen_inside_a_family_name_is_not_a_style() {
        let (family, style) = split_family_style(b"Lucida-Sans-Unicode");
        assert_eq!(family, "Lucida-Sans-Unicode");
        assert_eq!(style, FontStyle::default());
    }

    #[test]
    fn a_subset_tag_is_removed() {
        let (family, _) = split_family_style(b"ABCDEF+Helvetica");
        assert_eq!(family, "Helvetica");
    }

    #[test]
    fn a_trailing_style_word_without_a_separator_is_matched() {
        let (family, style) = split_family_style(b"ArialBold");
        assert_eq!(family, "Arial");
        assert!(style.is_bold());
    }

    #[test]
    fn a_name_that_is_only_a_style_word_stays_a_family() {
        let (family, style) = split_family_style(b"Bold");
        assert_eq!(family, "Bold");
        assert_eq!(style, FontStyle::default());
    }

    #[test]
    fn normalisation_matches_the_upstream_rule() {
        assert_eq!(normalize_name("Times New Roman"), "timesnewroman");
        assert_eq!(normalize_name("ABCDEF+Arial-Bold"), "arialbold");
        assert_eq!(normalize_name("Foo+Bar"), "foo");
        assert_eq!(normalize_name("Nimbus Sans L"), "nimbussansl");
    }

    #[test]
    fn lao_is_never_answered_by_a_latin_face() {
        let faces = coverage_faces('\u{0EA5}');
        assert!(!faces.is_empty());
        assert!(
            faces
                .iter()
                .all(|name| name.contains("Lao") || *name == "Phetsarath OT")
        );
        for latin in ["DejaVu Sans", "Liberation Sans", "Noto Sans", "FreeSerif"] {
            assert!(!faces.contains(&latin), "{latin} cannot draw Lao");
        }
    }

    #[test]
    fn style_distance_prefers_slope_over_weight() {
        let upright = FontStyle {
            weight: 400,
            italic: false,
        };
        let bold = FontStyle {
            weight: 700,
            italic: false,
        };
        let italic = FontStyle {
            weight: 400,
            italic: true,
        };
        assert!(upright.distance(bold) < upright.distance(italic));
    }
}
