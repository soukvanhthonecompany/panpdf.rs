use std::collections::BTreeMap;

use pdf_bytes::ByteStore;

use crate::cmap::{CMapError, CMapLimits};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Confidence {
    Declared,
    Named,
    Matched,
    Deciphered,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Meaning {
    pub text: String,
    pub confidence: Confidence,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Code {
    pub value: u32,
    pub byte_len: usize,
}

impl Code {
    #[must_use]
    pub fn bytes(self) -> Vec<u8> {
        (0..self.byte_len)
            .rev()
            .map(|index| {
                let shift = u32::try_from(index * 8).unwrap_or(u32::MAX);
                u8::try_from((self.value >> shift) & 0xff).unwrap_or(0)
            })
            .collect()
    }
}

const fn mt_extra(code: u8) -> Option<char> {
    Some(match code {
        0x09 => ' ',
        0x2C => '\u{200A}',
        0x3A => '\u{223C}',
        0x3B => '\u{2243}',
        0x3C => '\u{25C1}',
        0x3D => '\u{226A}',
        0x3E => '\u{25B7}',
        0x3F => '\u{226B}',
        0x40 => '\u{225C}',
        0x41 => '\u{2259}',
        0x42 => '\u{2250}',
        0x43 => '\u{2210}',
        0x44 => '\u{019B}',
        0x45 => '\u{2A3D}',
        0x46 => '\u{2A3C}',
        0x47 => '\u{2310}',
        0x48 => '\u{FFE2}',
        0x49 => '\u{22C2}',
        0x4A => '\u{2127}',
        0x4B => '\u{2026}',
        0x4C => '\u{22EF}',
        0x4D => '\u{22EE}',
        0x4E => '\u{22F0}',
        0x4F => '\u{22F1}',
        0x50 => '\u{2225}',
        0x51 => '\u{2235}',
        0x52 => '\u{2221}',
        0x53 => '\u{2222}',
        0x55 => '\u{22C3}',
        0x56 => '\u{25B3}',
        0x57 => '\u{2B1C}',
        0x61 => '\u{21A6}',
        0x68 => '\u{210F}',
        0x6C => '\u{2113}',
        _ => return None,
    })
}

const fn symbol_piece(code: u8) -> Option<char> {
    Some(match code {
        0xE6 => '\u{239B}',
        0xE7 => '\u{239C}',
        0xE8 => '\u{239D}',
        0xE9 => '\u{23A1}',
        0xEA => '\u{23A2}',
        0xEB => '\u{23A3}',
        0xEC => '\u{23A7}',
        0xED => '\u{23A8}',
        0xEE => '\u{23A9}',
        0xEF => '\u{23AA}',
        0xF4 => '\u{23AE}',
        0xF6 => '\u{239E}',
        0xF7 => '\u{239F}',
        0xF8 => '\u{23A0}',
        0xF9 => '\u{23A4}',
        0xFA => '\u{23A5}',
        0xFB => '\u{23A6}',
        0xFC => '\u{23AB}',
        0xFD => '\u{23AC}',
        0xFE => '\u{23AD}',
        _ => return None,
    })
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ToUnicode {
    forward: BTreeMap<Code, Meaning>,
    refused: Option<CMapError>,
    declared: BTreeMap<Code, Option<Meaning>>,
}

impl ToUnicode {
    pub fn from_cmap(source: &ByteStore, limits: CMapLimits) -> Result<Self, CMapError> {
        Ok(Self {
            forward: parse_to_unicode(source, limits)?,
            refused: None,
            declared: BTreeMap::new(),
        })
    }

    #[must_use]
    pub fn read_or_refused(source: &ByteStore, limits: CMapLimits) -> Self {
        match parse_to_unicode(source, limits) {
            Ok(forward) => Self {
                forward,
                refused: None,
                declared: BTreeMap::new(),
            },
            Err(error) => Self {
                forward: BTreeMap::new(),
                refused: Some(error),
                declared: BTreeMap::new(),
            },
        }
    }

    #[must_use]
    pub const fn refused(&self) -> Option<CMapError> {
        self.refused
    }

    pub fn add_glyph_names<'a, I>(&mut self, names: I)
    where
        I: IntoIterator<Item = (u8, &'a [u8])>,
    {
        let declared: std::collections::BTreeSet<String> = self
            .forward
            .values()
            .filter(|meaning| meaning.confidence == Confidence::Declared)
            .map(|meaning| meaning.text.clone())
            .collect();
        for (code, name) in names {
            let code = Code {
                value: u32::from(code),
                byte_len: 1,
            };
            if self.forward.contains_key(&code) {
                continue;
            }
            let text = text_from_glyph_name(name).filter(|text| !declared.contains(text));
            if let Some(text) = text {
                self.forward.insert(
                    code,
                    Meaning {
                        text,
                        confidence: Confidence::Named,
                    },
                );
            }
        }
    }

    pub fn add_characters<I>(&mut self, characters: I)
    where
        I: IntoIterator<Item = (u8, char)>,
    {
        let mut seen: BTreeMap<u8, Option<char>> = BTreeMap::new();
        for (code, character) in characters {
            seen.entry(code)
                .and_modify(|held| {
                    if *held != Some(character) {
                        *held = None;
                    }
                })
                .or_insert(Some(character));
        }
        for (code, character) in seen {
            let Some(character) = character else { continue };
            let text = character.to_string();
            if !crate::cmap::is_a_declaration(&text) {
                continue;
            }
            let code = Code {
                value: u32::from(code),
                byte_len: 1,
            };
            if self.forward.contains_key(&code) {
                continue;
            }
            self.forward.insert(
                code,
                Meaning {
                    text,
                    confidence: Confidence::Named,
                },
            );
        }
    }

    pub fn add_matched<I>(&mut self, matched: I)
    where
        I: IntoIterator<Item = (Code, char)>,
    {
        for (code, character) in matched {
            let text = character.to_string();
            if self.forward.contains_key(&code) || !crate::cmap::is_a_declaration(&text) {
                continue;
            }
            self.forward.insert(
                code,
                Meaning {
                    text,
                    confidence: Confidence::Matched,
                },
            );
        }
    }

    pub fn add_deciphered<I>(&mut self, deciphered: I)
    where
        I: IntoIterator<Item = (Code, String)>,
    {
        for (code, text) in deciphered {
            if !crate::cmap::is_a_declaration(&text) {
                continue;
            }
            let before = self.forward.insert(
                code,
                Meaning {
                    text,
                    confidence: Confidence::Deciphered,
                },
            );
            self.declared.entry(code).or_insert(before);
        }
    }

    pub fn entries(&self) -> impl Iterator<Item = (&Code, &Meaning)> {
        self.forward.iter()
    }

    pub fn read_symbol_private_use(&mut self) {
        let Some(encoding) = crate::standard14::Standard14::Symbol.built_in_encoding() else {
            return;
        };
        for meaning in self.forward.values_mut() {
            let mut characters = meaning.text.chars();
            let (Some(character), None) = (characters.next(), characters.next()) else {
                continue;
            };
            let Some(code) = u32::from(character)
                .checked_sub(0xF000)
                .and_then(|code| u8::try_from(code).ok())
                .filter(|code| *code >= 0x20)
            else {
                continue;
            };
            let read = symbol_piece(code)
                .or_else(|| {
                    encoding[usize::from(code)]
                        .and_then(crate::tables::unicode_for_glyph_name)
                        .and_then(char::from_u32)
                })
                .filter(|read| !(0xE000..=0xF8FF).contains(&u32::from(*read)));
            if let Some(read) = read {
                meaning.text = read.to_string();
            }
        }
    }

    pub fn read_mt_extra_private_use(&mut self) {
        for meaning in self.forward.values_mut() {
            let mut characters = meaning.text.chars();
            let (Some(character), None) = (characters.next(), characters.next()) else {
                continue;
            };
            let code = u32::from(character);
            let Some(code) = code
                .checked_sub(0xF000)
                .filter(|code| *code <= 0xFF)
                .or(Some(code).filter(|code| *code <= 0xFF))
                .and_then(|code| u8::try_from(code).ok())
            else {
                continue;
            };
            if let Some(read) = mt_extra(code) {
                meaning.text = read.to_string();
            }
        }
    }

    #[must_use]
    pub fn names_the_mt_extra_family(base_font: &[u8]) -> bool {
        let name = String::from_utf8_lossy(base_font);
        let bare = name.trim_start_matches('/');
        let bare = bare.split_once('+').map_or(bare, |(_, rest)| rest);
        let squashed: String = bare
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase();
        squashed == "mtextra"
            || squashed.starts_with("mtextrabold")
            || squashed.starts_with("mtextraitalic")
    }

    #[must_use]
    pub fn names_the_symbol_family(base_font: &[u8]) -> bool {
        let name = String::from_utf8_lossy(base_font);
        let family = name
            .trim_start_matches('/')
            .split([',', '-'])
            .next()
            .unwrap_or_default();
        matches!(
            crate::substitute::normalize_name(family).as_str(),
            "symbol" | "symbolmt"
        )
    }

    #[must_use]
    pub fn text_of(&self, code: Code) -> Option<&Meaning> {
        self.forward.get(&code)
    }

    #[must_use]
    pub fn declared_text_of(&self, code: Code) -> Option<&Meaning> {
        match self.declared.get(&code) {
            Some(before) => before.as_ref(),
            None => self.forward.get(&code),
        }
    }

    #[must_use]
    pub fn codes_for(&self, text: &str) -> Vec<Code> {
        self.forward
            .iter()
            .filter(|(_, meaning)| meaning.text == text)
            .map(|(code, _)| *code)
            .collect()
    }

    #[must_use]
    pub fn codes_to_write(&self, text: &str) -> Vec<Code> {
        let found: Vec<(Code, Confidence)> = self
            .forward
            .iter()
            .filter(|(_, meaning)| meaning.text == text)
            .map(|(code, meaning)| (*code, meaning.confidence))
            .collect();
        let Some(strongest) = found.iter().map(|(_, confidence)| *confidence).min() else {
            return Vec::new();
        };
        found
            .into_iter()
            .filter(|(_, confidence)| *confidence == strongest)
            .map(|(code, _)| code)
            .collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.forward.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.forward.is_empty()
    }

    #[must_use]
    pub fn by_confidence(&self, confidence: Confidence) -> usize {
        self.forward
            .values()
            .filter(|meaning| meaning.confidence == confidence)
            .count()
    }
}

#[must_use]
pub fn text_from_glyph_name(name: &[u8]) -> Option<String> {
    let base = name.split(|byte| *byte == b'.').next()?;
    if base.is_empty() {
        return None;
    }
    let mut text = String::new();
    for component in base.split(|byte| *byte == b'_') {
        text.push_str(&component_text(component)?);
    }
    Some(text)
}

fn component_text(component: &[u8]) -> Option<String> {
    if let Some(text) = spelled_code_point(component) {
        return Some(text);
    }
    let value = crate::tables::listed_unicode(component)?;
    let character = char::from_u32(value)?;
    let private = matches!(value, 0xE000..=0xF8FF | 0xF_0000..=0x10_FFFF);
    (!private && !character.is_control()).then(|| character.to_string())
}

fn spelled_code_point(base: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(base).ok()?;
    if let Some(digits) = text.strip_prefix("uni") {
        if digits.is_empty() || digits.len() % 4 != 0 {
            return None;
        }
        let mut units = Vec::with_capacity(digits.len() / 4);
        let mut rest = digits;
        while !rest.is_empty() {
            let (head, tail) = rest.split_at(4);
            units.push(u16::from_str_radix(head, 16).ok()?);
            rest = tail;
        }
        return String::from_utf16(&units).ok().filter(|s| !s.is_empty());
    }
    let digits = text.strip_prefix('u')?;
    if !(4..=6).contains(&digits.len()) {
        return None;
    }
    let value = u32::from_str_radix(digits, 16).ok()?;
    char::from_u32(value).map(String::from)
}

pub fn parse_to_unicode(
    source: &ByteStore,
    limits: CMapLimits,
) -> Result<BTreeMap<Code, Meaning>, CMapError> {
    crate::cmap::parse_bf_mappings(source, limits)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};

    use super::{Code, Confidence, ToUnicode, text_from_glyph_name};
    use crate::cmap::CMapLimits;

    fn source(bytes: &'static [u8]) -> ByteStore {
        ByteStore::new(SourceId::new(77), Arc::<[u8]>::from(bytes))
    }

    fn one_byte(value: u32) -> Code {
        Code { value, byte_len: 1 }
    }

    #[test]
    fn a_symbol_fonts_private_use_text_reads_as_the_signs_it_stands_for() {
        let cmap = b"1 begincodespacerange <00> <ff> endcodespacerange \
              5 beginbfchar <10> <F02D> <AA> <F0E9> <41> <0041> <42> <F041> <43> <F0F0> endbfchar";
        let mut map = ToUnicode::from_cmap(&source(cmap), CMapLimits::default()).expect("reads");
        let unread = map.clone();
        map.read_symbol_private_use();
        let text = |map: &ToUnicode, code: u32| map.text_of(one_byte(code)).map(|m| m.text.clone());
        assert_eq!(text(&map, 0x10).as_deref(), Some("\u{2212}"));
        assert_eq!(text(&map, 0xAA).as_deref(), Some("\u{23A1}"));
        assert_eq!(text(&map, 0x42).as_deref(), Some("\u{0391}"));
        assert_eq!(text(&map, 0x41).as_deref(), Some("A"));
        assert_eq!(text(&map, 0x43).as_deref(), Some("\u{F0F0}"));
        assert_eq!(map.codes_for("\u{2212}"), vec![one_byte(0x10)]);
        assert_eq!(text(&unread, 0x10).as_deref(), Some("\u{F02D}"));
    }

    #[test]
    fn an_mt_extra_fonts_private_use_text_reads_as_the_signs_it_stands_for() {
        let cmap = b"1 begincodespacerange <00> <ff> endcodespacerange \
              3 beginbfchar <10> <F061> <11> <006C> <12> <F06F> endbfchar";
        let mut map = ToUnicode::from_cmap(&source(cmap), CMapLimits::default()).expect("reads");
        let unread = map.clone();
        map.read_mt_extra_private_use();
        let text = |map: &ToUnicode, code: u32| map.text_of(one_byte(code)).map(|m| m.text.clone());
        assert_eq!(text(&map, 0x10).as_deref(), Some("\u{21A6}"));
        assert_eq!(text(&map, 0x11).as_deref(), Some("\u{2113}"));
        assert_eq!(
            text(&map, 0x12).as_deref(),
            Some("\u{F06F}"),
            "a code no map carries is left as the file wrote it"
        );
        assert_eq!(text(&unread, 0x10).as_deref(), Some("\u{F061}"));
    }

    #[test]
    fn only_the_mt_extra_family_has_its_private_use_text_read_as_mt_extra() {
        for name in [&b"NIPCIF+MT-Extra"[..], b"/MTExtra", b"MT Extra,Bold"] {
            assert!(ToUnicode::names_the_mt_extra_family(name), "{name:?}");
        }
        for name in [&b"NIPCGC+SymbolMT"[..], b"ArialMT", b"MTSYN"] {
            assert!(!ToUnicode::names_the_mt_extra_family(name), "{name:?}");
        }
    }

    #[test]
    fn only_the_symbol_family_has_its_private_use_text_read_as_symbol() {
        for name in [&b"NIPCGC+SymbolMT"[..], b"/Symbol", b"Symbol,Bold"] {
            assert!(ToUnicode::names_the_symbol_family(name), "{name:?}");
        }
        for name in [&b"ArialMT"[..], b"NIPCIF+MT-Extra", b"SymbolicSans"] {
            assert!(!ToUnicode::names_the_symbol_family(name), "{name:?}");
        }
    }

    #[test]
    fn a_to_unicode_cmap_reads_both_ways() {
        let map = ToUnicode::from_cmap(
            &source(
                b"1 begincodespacerange <00> <ff> endcodespacerange \
                  2 beginbfchar <41> <0041> <42> <00660069> endbfchar \
                  2 beginbfrange <61> <63> <0061> <70> <72> [<0058> <0059> <005a>] endbfrange",
            ),
            CMapLimits::default(),
        )
        .expect("a well-formed ToUnicode CMap");

        assert_eq!(
            map.text_of(one_byte(0x41)).map(|m| m.text.as_str()),
            Some("A")
        );
        assert_eq!(
            map.text_of(one_byte(0x42)).map(|m| m.text.as_str()),
            Some("fi")
        );

        assert_eq!(
            map.text_of(one_byte(0x61)).map(|m| m.text.as_str()),
            Some("a")
        );
        assert_eq!(
            map.text_of(one_byte(0x62)).map(|m| m.text.as_str()),
            Some("b")
        );
        assert_eq!(
            map.text_of(one_byte(0x63)).map(|m| m.text.as_str()),
            Some("c")
        );

        assert_eq!(
            map.text_of(one_byte(0x70)).map(|m| m.text.as_str()),
            Some("X")
        );
        assert_eq!(
            map.text_of(one_byte(0x72)).map(|m| m.text.as_str()),
            Some("Z")
        );

        assert_eq!(map.codes_for("b"), vec![one_byte(0x62)]);
        assert_eq!(map.codes_for("fi"), vec![one_byte(0x42)]);
        assert_eq!(
            map.codes_for("q"),
            Vec::new(),
            "a character this font has no code for is Class C, not a guess"
        );
        assert_eq!(
            map.by_confidence(Confidence::Declared),
            8,
            "two bfchar, three incremented, three named"
        );
    }

    #[test]
    fn a_cmap_with_the_usual_postscript_preamble_reads() {
        let map = ToUnicode::from_cmap(
            &source(
                b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap                   /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def                   /CMapName /Adobe-Identity-UCS def /CMapType 2 def                   1 begincodespacerange <0000> <ffff> endcodespacerange                   1 beginbfchar <0003> <0020> endbfchar                   endcmap CMapName currentdict /CMap defineresource pop end end",
            ),
            CMapLimits::default(),
        )
        .expect("the preamble is not part of the mapping");
        assert_eq!(
            map.text_of(Code {
                value: 3,
                byte_len: 2
            })
            .map(|m| m.text.as_str()),
            Some(" ")
        );
    }

    #[test]
    fn a_refused_cmap_says_it_was_refused() {
        let broken = ToUnicode::read_or_refused(
            &source(b"1 beginbfchar <41> <0041> endbfchar"),
            CMapLimits::default(),
        );
        assert!(broken.is_empty());
        assert_eq!(
            broken.refused(),
            Some(crate::cmap::CMapError::MissingCodeSpace),
            "a CMap with no code space is refused, and an interface can say so"
        );
        let absent = ToUnicode::default();
        assert!(absent.is_empty());
        assert_eq!(
            absent.refused(),
            None,
            "a file that declared nothing is a different fact from one we refused"
        );
    }

    #[test]
    fn a_destination_of_nothing_is_not_a_declaration() {
        let map = ToUnicode::from_cmap(
            &source(
                b"1 begincodespacerange <00> <ff> endcodespacerange \
                  3 beginbfchar <41> <0041> <42> <0000> <43> <fffd> endbfchar",
            ),
            CMapLimits::default(),
        )
        .expect("a well-formed `CMap`");
        assert_eq!(
            map.text_of(one_byte(0x41)).map(|m| m.text.as_str()),
            Some("A")
        );
        assert_eq!(map.text_of(one_byte(0x42)), None, "`U+0000` says nothing");
        assert_eq!(
            map.text_of(one_byte(0x43)),
            None,
            "and so does the replacement character"
        );
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn a_text_two_codes_can_write_is_reported_as_two() {
        let map = ToUnicode::from_cmap(
            &source(
                b"1 begincodespacerange <00> <ff> endcodespacerange \
                  2 beginbfchar <20> <0020> <a0> <0020> endbfchar",
            ),
            CMapLimits::default(),
        )
        .expect("a well-formed ToUnicode CMap");
        assert_eq!(map.codes_for(" "), vec![one_byte(0x20), one_byte(0xa0)]);
    }

    #[test]
    fn a_declared_mapping_outranks_a_glyph_name() {
        let mut map = ToUnicode::from_cmap(
            &source(
                b"1 begincodespacerange <00> <ff> endcodespacerange \
                  1 beginbfchar <41> <0391> endbfchar",
            ),
            CMapLimits::default(),
        )
        .expect("a well-formed ToUnicode CMap");
        map.add_glyph_names([
            (0x41_u8, b"uni0041".as_slice()),
            (0x42, b"uni0042"),
            (0x43, b"g19"),
        ]);
        let declared = map.text_of(one_byte(0x41)).expect("code 0x41 is mapped");
        assert_eq!(
            declared.text, "\u{391}",
            "the file said Alpha, so it is Alpha"
        );
        assert_eq!(declared.confidence, Confidence::Declared);
        let named = map.text_of(one_byte(0x42)).expect("code 0x42 is named");
        assert_eq!(named.text, "B");
        assert_eq!(named.confidence, Confidence::Named);
        assert_eq!(
            map.text_of(one_byte(0x43)),
            None,
            "`g19` says nothing, and nothing is not a guess"
        );
    }

    #[test]
    fn a_glyph_name_repeating_another_codes_declaration_is_not_added() {
        let mut map = ToUnicode::from_cmap(
            &source(
                b"1 begincodespacerange <00> <ff> endcodespacerange \
                  1 beginbfchar <31> <0031> endbfchar",
            ),
            CMapLimits::default(),
        )
        .expect("a well-formed ToUnicode CMap");
        map.add_glyph_names([(0x22_u8, b"one".as_slice()), (0x32, b"two")]);
        assert_eq!(map.text_of(one_byte(0x22)), None);
        assert_eq!(map.codes_for("1"), vec![one_byte(0x31)]);
        assert_eq!(
            map.text_of(one_byte(0x32)).map(|meaning| &meaning.text[..]),
            Some("2"),
            "control: a name no declaration contradicts is still read"
        );
    }

    #[test]
    fn a_glyph_two_characters_reach_is_refused_rather_than_decided() {
        let mut map = ToUnicode::from_cmap(
            &source(
                b"1 begincodespacerange <00> <ff> endcodespacerange \
                  1 beginbfchar <41> <0391> endbfchar",
            ),
            CMapLimits::default(),
        )
        .expect("a well-formed ToUnicode CMap");
        map.add_characters([
            (0x41_u8, 'A'),
            (0x42, '-'),
            (0x42, '\u{2212}'),
            (0x43, 'ก'),
            (0x44, '\u{fffd}'),
        ]);
        assert_eq!(
            map.text_of(one_byte(0x41)).map(|meaning| &meaning.text[..]),
            Some("\u{391}"),
            "the file's own declaration still stands"
        );
        assert_eq!(
            map.text_of(one_byte(0x42)),
            None,
            "a glyph two characters reach says nothing"
        );
        let named = map
            .text_of(one_byte(0x43))
            .expect("code 0x43 is reached once");
        assert_eq!(named.text, "ก");
        assert_eq!(
            named.confidence,
            Confidence::Named,
            "evidence from the font, not a declaration by the producer"
        );
        assert_eq!(map.text_of(one_byte(0x44)), None);
    }

    #[test]
    fn a_glyph_name_says_what_it_encodes_and_no_more() {
        assert_eq!(text_from_glyph_name(b"uni0E01").as_deref(), Some("\u{e01}"));
        assert_eq!(
            text_from_glyph_name(b"uni0E010E38").as_deref(),
            Some("\u{e01}\u{e38}"),
            "a `uni` name may carry several units"
        );
        assert_eq!(
            text_from_glyph_name(b"u1F600").as_deref(),
            Some("\u{1f600}")
        );
        assert_eq!(
            text_from_glyph_name(b"uni0041.sc").as_deref(),
            Some("A"),
            "a variant suffix changes the glyph, not the character"
        );
        assert_eq!(text_from_glyph_name(b"g19"), None);
        assert_eq!(text_from_glyph_name(b"uni041"), None, "not a whole unit");
        assert_eq!(text_from_glyph_name(b"uniZZZZ"), None);
        assert_eq!(text_from_glyph_name(b"u123"), None, "too few digits");
        assert_eq!(text_from_glyph_name(b"u1234567"), None, "too many");
        assert_eq!(
            text_from_glyph_name(b"uniD83D"),
            None,
            "half a character is a corruption that renders"
        );
        assert_eq!(text_from_glyph_name(b"g19.alt"), None);
        assert_eq!(text_from_glyph_name(b""), None);
        assert_eq!(text_from_glyph_name(b".notdef"), None);
    }

    #[test]
    fn a_listed_glyph_name_reads_through_the_adobe_glyph_list() {
        assert_eq!(text_from_glyph_name(b"A").as_deref(), Some("A"));
        assert_eq!(text_from_glyph_name(b"space").as_deref(), Some(" "));
        assert_eq!(text_from_glyph_name(b"eacute").as_deref(), Some("é"));
        assert_eq!(text_from_glyph_name(b"kokaithai").as_deref(), Some("ก"));
        assert_eq!(text_from_glyph_name(b"one.oldstyle").as_deref(), Some("1"));
        assert_eq!(
            text_from_glyph_name(b"f_i").as_deref(),
            Some("fi"),
            "a ligature's name lists its components"
        );
        assert_eq!(
            text_from_glyph_name(b"f_g19"),
            None,
            "a component nothing names makes the whole name say nothing"
        );
        assert_eq!(
            text_from_glyph_name(b"Acutesmall"),
            None,
            "a list entry in the private-use area names a glyph, not a character"
        );
        assert_eq!(
            text_from_glyph_name(b"dalethiriq"),
            None,
            "a list entry naming two characters is not read as its first"
        );
    }

    #[test]
    fn a_character_is_written_with_its_strongest_code() {
        let mut map = ToUnicode::from_cmap(
            &source(
                b"1 begincodespacerange <00> <ff> endcodespacerange \
                  3 beginbfchar <49> <0033> <50> <0034> <51> <0034> endbfchar",
            ),
            CMapLimits::default(),
        )
        .expect("a well-formed ToUnicode CMap");
        map.add_matched([(one_byte(0x42), '3'), (one_byte(0x43), '7')]);
        assert_eq!(map.codes_for("3"), vec![one_byte(0x42), one_byte(0x49)]);
        assert_eq!(map.codes_to_write("3"), vec![one_byte(0x49)]);
        assert_eq!(
            map.codes_to_write("7"),
            vec![one_byte(0x43)],
            "a match is used where nothing stronger says the character"
        );
        assert_eq!(
            map.codes_to_write("4"),
            vec![one_byte(0x50), one_byte(0x51)],
            "two declarations stay a choice"
        );
        assert!(map.codes_to_write("9").is_empty());
    }

    #[test]
    fn a_code_writes_back_at_the_width_it_was_read_at() {
        assert_eq!(one_byte(0x41).bytes(), vec![0x41]);
        assert_eq!(
            Code {
                value: 0x0041,
                byte_len: 2
            }
            .bytes(),
            vec![0x00, 0x41],
            "a composite font's code is two bytes even when its value fits in one"
        );
    }
}
