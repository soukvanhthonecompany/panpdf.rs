use std::sync::{Arc, OnceLock};

use pdf_bytes::{ByteStore, SourceId};

use crate::cmap::{CMapError, CMapLimits};
use crate::substitute::{FontRequest, UnresolvedReason};
use crate::tounicode::{Code, ToUnicode};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyEncoding {
    GbkEuc,
}

impl LegacyEncoding {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::GbkEuc => "GBK-EUC",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyByte {
    Meaning(char),
    Unresolved(UnresolvedReason),
}

impl FontRequest {
    #[must_use]
    pub fn legacy_multibyte_encoding(&self) -> Option<LegacyEncoding> {
        self.has_ambiguous_cjk_encoding()
            .then_some(LegacyEncoding::GbkEuc)
    }
}

#[must_use]
pub fn decode_legacy_bytes(encoding: LegacyEncoding, bytes: &[u8]) -> Vec<LegacyByte> {
    let table = match encoding {
        LegacyEncoding::GbkEuc => gbk_euc_ucs2(),
    };
    let Some(table) = table else {
        return vec![LegacyByte::Unresolved(UnresolvedReason::AmbiguousEncoding); bytes.len()];
    };
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let lead = bytes[index];
        if lead <= 0x80 || lead == 0xFF {
            out.push(character(table, u32::from(lead), 1));
            index += 1;
            continue;
        }
        let Some(trail) = bytes.get(index + 1).copied() else {
            out.push(LegacyByte::Unresolved(UnresolvedReason::TruncatedMultibyte));
            index += 1;
            continue;
        };
        if !(0x40..=0xFE).contains(&trail) {
            out.push(LegacyByte::Unresolved(UnresolvedReason::TruncatedMultibyte));
            index += 1;
            continue;
        }
        let value = (u32::from(lead) << 8) | u32::from(trail);
        let found = character(table, value, 2);
        out.push(match found {
            LegacyByte::Meaning(character) => LegacyByte::Meaning(character),
            LegacyByte::Unresolved(_) => {
                LegacyByte::Unresolved(UnresolvedReason::UnmappedMultibyte)
            }
        });
        out.push(LegacyByte::Unresolved(
            UnresolvedReason::MultibyteContinuation,
        ));
        index += 2;
    }
    out
}

fn character(table: &ToUnicode, value: u32, byte_len: usize) -> LegacyByte {
    let Some(meaning) = table.text_of(Code { value, byte_len }) else {
        return LegacyByte::Unresolved(UnresolvedReason::UnmappedMultibyte);
    };
    let mut characters = meaning.text.chars();
    match (characters.next(), characters.next()) {
        (Some(one), None) if one != '\u{0}' => LegacyByte::Meaning(one),
        _ => LegacyByte::Unresolved(UnresolvedReason::UnmappedMultibyte),
    }
}

fn gbk_euc_ucs2() -> Option<&'static ToUnicode> {
    static TABLE: OnceLock<Result<ToUnicode, CMapError>> = OnceLock::new();
    TABLE
        .get_or_init(|| {
            let source = ByteStore::new(
                SourceId::new(u64::MAX - 1),
                Arc::<[u8]>::from(GBK_EUC_UCS2_BYTES),
            );
            ToUnicode::from_cmap(&source, CMapLimits::default())
        })
        .as_ref()
        .ok()
}

const GBK_EUC_UCS2_BYTES: &[u8] = include_bytes!("../resources/cmap/Adobe-GB1/GBK-EUC-UCS2");

#[cfg(test)]
mod tests {
    use super::{
        GBK_EUC_UCS2_BYTES, LegacyByte, LegacyEncoding, decode_legacy_bytes, gbk_euc_ucs2,
    };
    use crate::substitute::UnresolvedReason;

    #[test]
    fn the_shipped_table_declares_the_code_spaces_this_decoder_assumes() {
        let text = String::from_utf8_lossy(GBK_EUC_UCS2_BYTES);
        let start = text
            .find("begincodespacerange")
            .expect("the resource declares a code space");
        let end = text[start..]
            .find("endcodespacerange")
            .expect("the code space ends");
        let block = &text[start..start + end];
        let ranges: Vec<&str> = block
            .lines()
            .skip(1)
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        assert_eq!(
            ranges,
            vec!["<00>   <80>", "<8140> <FEFE>", "<FF>   <FF>"],
            "the decoder's byte ranges are written from these three lines"
        );
    }

    #[test]
    fn a_two_byte_code_decodes_to_the_character_the_table_names() {
        let table = gbk_euc_ucs2().expect("the shipped table parses");
        assert_eq!(
            table
                .text_of(crate::tounicode::Code {
                    value: 0x8140,
                    byte_len: 2
                })
                .map(|meaning| meaning.text.as_str()),
            Some("\u{4E02}")
        );

        let decoded = decode_legacy_bytes(LegacyEncoding::GbkEuc, &[0x81, 0x40]);
        assert_eq!(
            decoded,
            vec![
                LegacyByte::Meaning('\u{4E02}'),
                LegacyByte::Unresolved(UnresolvedReason::MultibyteContinuation),
            ],
            "one character, drawn by its lead byte, with the trail accounted for"
        );
    }

    #[test]
    fn the_family_name_these_files_are_named_by_decodes_to_itself() {
        let decoded = decode_legacy_bytes(LegacyEncoding::GbkEuc, &[0xCB, 0xCE, 0xCC, 0xE5]);
        assert_eq!(
            decoded,
            vec![
                LegacyByte::Meaning('宋'),
                LegacyByte::Unresolved(UnresolvedReason::MultibyteContinuation),
                LegacyByte::Meaning('体'),
                LegacyByte::Unresolved(UnresolvedReason::MultibyteContinuation),
            ]
        );
    }

    #[test]
    fn a_one_byte_code_is_still_one_code() {
        let decoded = decode_legacy_bytes(LegacyEncoding::GbkEuc, b"Ab1");
        assert_eq!(
            decoded,
            vec![
                LegacyByte::Meaning('A'),
                LegacyByte::Meaning('b'),
                LegacyByte::Meaning('1'),
            ]
        );
    }

    #[test]
    fn a_lead_byte_with_no_trail_is_refused() {
        assert_eq!(
            decode_legacy_bytes(LegacyEncoding::GbkEuc, &[0xCB]),
            vec![LegacyByte::Unresolved(UnresolvedReason::TruncatedMultibyte)]
        );
        assert_eq!(
            decode_legacy_bytes(LegacyEncoding::GbkEuc, &[0xCB, 0x20]),
            vec![
                LegacyByte::Unresolved(UnresolvedReason::TruncatedMultibyte),
                LegacyByte::Meaning(' '),
            ]
        );
    }

    #[test]
    fn a_code_the_table_does_not_define_stays_unresolved() {
        let table = gbk_euc_ucs2().expect("the shipped table parses");
        for lead in [0x81_u8, 0xCB, 0xFE] {
            let value = (u32::from(lead) << 8) | 0x7F;
            assert!(
                table
                    .text_of(crate::tounicode::Code { value, byte_len: 2 })
                    .is_none_or(|meaning| meaning.text.starts_with('\u{0}')),
                "{value:04X} is expected to be a hole in the shipped table"
            );
            assert_eq!(
                decode_legacy_bytes(LegacyEncoding::GbkEuc, &[lead, 0x7F]),
                vec![
                    LegacyByte::Unresolved(UnresolvedReason::UnmappedMultibyte),
                    LegacyByte::Unresolved(UnresolvedReason::MultibyteContinuation),
                ]
            );
        }
        assert_eq!(
            decode_legacy_bytes(LegacyEncoding::GbkEuc, &[0xCB, 0xCE])[0],
            LegacyByte::Meaning('宋')
        );
    }

    #[test]
    fn there_is_exactly_one_answer_for_every_input_byte() {
        for bytes in [
            vec![],
            vec![0x41],
            vec![0xCB, 0xCE],
            vec![0xCB, 0xCE, 0x41, 0xB7, 0xC2, 0xFF],
            vec![0xFE],
        ] {
            let decoded = decode_legacy_bytes(LegacyEncoding::GbkEuc, &bytes);
            assert_eq!(decoded.len(), bytes.len(), "for {bytes:02X?}");
        }
    }
}
