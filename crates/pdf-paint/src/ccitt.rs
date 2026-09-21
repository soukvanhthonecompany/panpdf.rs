#![allow(clippy::unreadable_literal)]

#[derive(Clone, Copy, Debug)]
struct Code {
    bits: u8,
    value: u16,
    run: u16,
}

const WHITE: [Code; 104] = [
    Code {
        bits: 4,
        value: 0b0111,
        run: 2,
    },
    Code {
        bits: 4,
        value: 0b1000,
        run: 3,
    },
    Code {
        bits: 4,
        value: 0b1011,
        run: 4,
    },
    Code {
        bits: 4,
        value: 0b1100,
        run: 5,
    },
    Code {
        bits: 4,
        value: 0b1110,
        run: 6,
    },
    Code {
        bits: 4,
        value: 0b1111,
        run: 7,
    },
    Code {
        bits: 5,
        value: 0b00111,
        run: 10,
    },
    Code {
        bits: 5,
        value: 0b01000,
        run: 11,
    },
    Code {
        bits: 5,
        value: 0b10010,
        run: 128,
    },
    Code {
        bits: 5,
        value: 0b10011,
        run: 8,
    },
    Code {
        bits: 5,
        value: 0b10100,
        run: 9,
    },
    Code {
        bits: 5,
        value: 0b11011,
        run: 64,
    },
    Code {
        bits: 6,
        value: 0b000011,
        run: 13,
    },
    Code {
        bits: 6,
        value: 0b000111,
        run: 1,
    },
    Code {
        bits: 6,
        value: 0b001000,
        run: 12,
    },
    Code {
        bits: 6,
        value: 0b010111,
        run: 192,
    },
    Code {
        bits: 6,
        value: 0b011000,
        run: 1664,
    },
    Code {
        bits: 6,
        value: 0b101010,
        run: 16,
    },
    Code {
        bits: 6,
        value: 0b101011,
        run: 17,
    },
    Code {
        bits: 6,
        value: 0b110100,
        run: 14,
    },
    Code {
        bits: 6,
        value: 0b110101,
        run: 15,
    },
    Code {
        bits: 7,
        value: 0b0000011,
        run: 22,
    },
    Code {
        bits: 7,
        value: 0b0000100,
        run: 23,
    },
    Code {
        bits: 7,
        value: 0b0001000,
        run: 20,
    },
    Code {
        bits: 7,
        value: 0b0001100,
        run: 19,
    },
    Code {
        bits: 7,
        value: 0b0010011,
        run: 26,
    },
    Code {
        bits: 7,
        value: 0b0010111,
        run: 21,
    },
    Code {
        bits: 7,
        value: 0b0011000,
        run: 28,
    },
    Code {
        bits: 7,
        value: 0b0100100,
        run: 27,
    },
    Code {
        bits: 7,
        value: 0b0100111,
        run: 18,
    },
    Code {
        bits: 7,
        value: 0b0101000,
        run: 24,
    },
    Code {
        bits: 7,
        value: 0b0101011,
        run: 25,
    },
    Code {
        bits: 7,
        value: 0b0110111,
        run: 256,
    },
    Code {
        bits: 8,
        value: 0b00000010,
        run: 29,
    },
    Code {
        bits: 8,
        value: 0b00000011,
        run: 30,
    },
    Code {
        bits: 8,
        value: 0b00000100,
        run: 45,
    },
    Code {
        bits: 8,
        value: 0b00000101,
        run: 46,
    },
    Code {
        bits: 8,
        value: 0b00001010,
        run: 47,
    },
    Code {
        bits: 8,
        value: 0b00001011,
        run: 48,
    },
    Code {
        bits: 8,
        value: 0b00010010,
        run: 33,
    },
    Code {
        bits: 8,
        value: 0b00010011,
        run: 34,
    },
    Code {
        bits: 8,
        value: 0b00010100,
        run: 35,
    },
    Code {
        bits: 8,
        value: 0b00010101,
        run: 36,
    },
    Code {
        bits: 8,
        value: 0b00010110,
        run: 37,
    },
    Code {
        bits: 8,
        value: 0b00010111,
        run: 38,
    },
    Code {
        bits: 8,
        value: 0b00011010,
        run: 31,
    },
    Code {
        bits: 8,
        value: 0b00011011,
        run: 32,
    },
    Code {
        bits: 8,
        value: 0b00100100,
        run: 53,
    },
    Code {
        bits: 8,
        value: 0b00100101,
        run: 54,
    },
    Code {
        bits: 8,
        value: 0b00101000,
        run: 39,
    },
    Code {
        bits: 8,
        value: 0b00101001,
        run: 40,
    },
    Code {
        bits: 8,
        value: 0b00101010,
        run: 41,
    },
    Code {
        bits: 8,
        value: 0b00101011,
        run: 42,
    },
    Code {
        bits: 8,
        value: 0b00101100,
        run: 43,
    },
    Code {
        bits: 8,
        value: 0b00101101,
        run: 44,
    },
    Code {
        bits: 8,
        value: 0b00110010,
        run: 61,
    },
    Code {
        bits: 8,
        value: 0b00110011,
        run: 62,
    },
    Code {
        bits: 8,
        value: 0b00110100,
        run: 63,
    },
    Code {
        bits: 8,
        value: 0b00110101,
        run: 0,
    },
    Code {
        bits: 8,
        value: 0b00110110,
        run: 320,
    },
    Code {
        bits: 8,
        value: 0b00110111,
        run: 384,
    },
    Code {
        bits: 8,
        value: 0b01001010,
        run: 59,
    },
    Code {
        bits: 8,
        value: 0b01001011,
        run: 60,
    },
    Code {
        bits: 8,
        value: 0b01010010,
        run: 49,
    },
    Code {
        bits: 8,
        value: 0b01010011,
        run: 50,
    },
    Code {
        bits: 8,
        value: 0b01010100,
        run: 51,
    },
    Code {
        bits: 8,
        value: 0b01010101,
        run: 52,
    },
    Code {
        bits: 8,
        value: 0b01011000,
        run: 55,
    },
    Code {
        bits: 8,
        value: 0b01011001,
        run: 56,
    },
    Code {
        bits: 8,
        value: 0b01011010,
        run: 57,
    },
    Code {
        bits: 8,
        value: 0b01011011,
        run: 58,
    },
    Code {
        bits: 8,
        value: 0b01100100,
        run: 448,
    },
    Code {
        bits: 8,
        value: 0b01100101,
        run: 512,
    },
    Code {
        bits: 8,
        value: 0b01100111,
        run: 640,
    },
    Code {
        bits: 8,
        value: 0b01101000,
        run: 576,
    },
    Code {
        bits: 9,
        value: 0b010011000,
        run: 1472,
    },
    Code {
        bits: 9,
        value: 0b010011001,
        run: 1536,
    },
    Code {
        bits: 9,
        value: 0b010011010,
        run: 1600,
    },
    Code {
        bits: 9,
        value: 0b010011011,
        run: 1728,
    },
    Code {
        bits: 9,
        value: 0b011001100,
        run: 704,
    },
    Code {
        bits: 9,
        value: 0b011001101,
        run: 768,
    },
    Code {
        bits: 9,
        value: 0b011010010,
        run: 832,
    },
    Code {
        bits: 9,
        value: 0b011010011,
        run: 896,
    },
    Code {
        bits: 9,
        value: 0b011010100,
        run: 960,
    },
    Code {
        bits: 9,
        value: 0b011010101,
        run: 1024,
    },
    Code {
        bits: 9,
        value: 0b011010110,
        run: 1088,
    },
    Code {
        bits: 9,
        value: 0b011010111,
        run: 1152,
    },
    Code {
        bits: 9,
        value: 0b011011000,
        run: 1216,
    },
    Code {
        bits: 9,
        value: 0b011011001,
        run: 1280,
    },
    Code {
        bits: 9,
        value: 0b011011010,
        run: 1344,
    },
    Code {
        bits: 9,
        value: 0b011011011,
        run: 1408,
    },
    Code {
        bits: 11,
        value: 0b00000001000,
        run: 1792,
    },
    Code {
        bits: 11,
        value: 0b00000001100,
        run: 1856,
    },
    Code {
        bits: 11,
        value: 0b00000001101,
        run: 1920,
    },
    Code {
        bits: 12,
        value: 0b000000010010,
        run: 1984,
    },
    Code {
        bits: 12,
        value: 0b000000010011,
        run: 2048,
    },
    Code {
        bits: 12,
        value: 0b000000010100,
        run: 2112,
    },
    Code {
        bits: 12,
        value: 0b000000010101,
        run: 2176,
    },
    Code {
        bits: 12,
        value: 0b000000010110,
        run: 2240,
    },
    Code {
        bits: 12,
        value: 0b000000010111,
        run: 2304,
    },
    Code {
        bits: 12,
        value: 0b000000011100,
        run: 2368,
    },
    Code {
        bits: 12,
        value: 0b000000011101,
        run: 2432,
    },
    Code {
        bits: 12,
        value: 0b000000011110,
        run: 2496,
    },
    Code {
        bits: 12,
        value: 0b000000011111,
        run: 2560,
    },
];

const BLACK: [Code; 104] = [
    Code {
        bits: 2,
        value: 0b10,
        run: 3,
    },
    Code {
        bits: 2,
        value: 0b11,
        run: 2,
    },
    Code {
        bits: 3,
        value: 0b010,
        run: 1,
    },
    Code {
        bits: 3,
        value: 0b011,
        run: 4,
    },
    Code {
        bits: 4,
        value: 0b0010,
        run: 6,
    },
    Code {
        bits: 4,
        value: 0b0011,
        run: 5,
    },
    Code {
        bits: 5,
        value: 0b00011,
        run: 7,
    },
    Code {
        bits: 6,
        value: 0b000100,
        run: 9,
    },
    Code {
        bits: 6,
        value: 0b000101,
        run: 8,
    },
    Code {
        bits: 7,
        value: 0b0000100,
        run: 10,
    },
    Code {
        bits: 7,
        value: 0b0000101,
        run: 11,
    },
    Code {
        bits: 7,
        value: 0b0000111,
        run: 12,
    },
    Code {
        bits: 8,
        value: 0b00000100,
        run: 13,
    },
    Code {
        bits: 8,
        value: 0b00000111,
        run: 14,
    },
    Code {
        bits: 9,
        value: 0b000011000,
        run: 15,
    },
    Code {
        bits: 10,
        value: 0b0000001000,
        run: 18,
    },
    Code {
        bits: 10,
        value: 0b0000001111,
        run: 64,
    },
    Code {
        bits: 10,
        value: 0b0000010111,
        run: 16,
    },
    Code {
        bits: 10,
        value: 0b0000011000,
        run: 17,
    },
    Code {
        bits: 10,
        value: 0b0000110111,
        run: 0,
    },
    Code {
        bits: 11,
        value: 0b00000001000,
        run: 1792,
    },
    Code {
        bits: 11,
        value: 0b00000001100,
        run: 1856,
    },
    Code {
        bits: 11,
        value: 0b00000001101,
        run: 1920,
    },
    Code {
        bits: 11,
        value: 0b00000010111,
        run: 24,
    },
    Code {
        bits: 11,
        value: 0b00000011000,
        run: 25,
    },
    Code {
        bits: 11,
        value: 0b00000101000,
        run: 23,
    },
    Code {
        bits: 11,
        value: 0b00000110111,
        run: 22,
    },
    Code {
        bits: 11,
        value: 0b00001100111,
        run: 19,
    },
    Code {
        bits: 11,
        value: 0b00001101000,
        run: 20,
    },
    Code {
        bits: 11,
        value: 0b00001101100,
        run: 21,
    },
    Code {
        bits: 12,
        value: 0b000000010010,
        run: 1984,
    },
    Code {
        bits: 12,
        value: 0b000000010011,
        run: 2048,
    },
    Code {
        bits: 12,
        value: 0b000000010100,
        run: 2112,
    },
    Code {
        bits: 12,
        value: 0b000000010101,
        run: 2176,
    },
    Code {
        bits: 12,
        value: 0b000000010110,
        run: 2240,
    },
    Code {
        bits: 12,
        value: 0b000000010111,
        run: 2304,
    },
    Code {
        bits: 12,
        value: 0b000000011100,
        run: 2368,
    },
    Code {
        bits: 12,
        value: 0b000000011101,
        run: 2432,
    },
    Code {
        bits: 12,
        value: 0b000000011110,
        run: 2496,
    },
    Code {
        bits: 12,
        value: 0b000000011111,
        run: 2560,
    },
    Code {
        bits: 12,
        value: 0b000000100100,
        run: 52,
    },
    Code {
        bits: 12,
        value: 0b000000100111,
        run: 55,
    },
    Code {
        bits: 12,
        value: 0b000000101000,
        run: 56,
    },
    Code {
        bits: 12,
        value: 0b000000101011,
        run: 59,
    },
    Code {
        bits: 12,
        value: 0b000000101100,
        run: 60,
    },
    Code {
        bits: 12,
        value: 0b000000110011,
        run: 320,
    },
    Code {
        bits: 12,
        value: 0b000000110100,
        run: 384,
    },
    Code {
        bits: 12,
        value: 0b000000110101,
        run: 448,
    },
    Code {
        bits: 12,
        value: 0b000000110111,
        run: 53,
    },
    Code {
        bits: 12,
        value: 0b000000111000,
        run: 54,
    },
    Code {
        bits: 12,
        value: 0b000001010010,
        run: 50,
    },
    Code {
        bits: 12,
        value: 0b000001010011,
        run: 51,
    },
    Code {
        bits: 12,
        value: 0b000001010100,
        run: 44,
    },
    Code {
        bits: 12,
        value: 0b000001010101,
        run: 45,
    },
    Code {
        bits: 12,
        value: 0b000001010110,
        run: 46,
    },
    Code {
        bits: 12,
        value: 0b000001010111,
        run: 47,
    },
    Code {
        bits: 12,
        value: 0b000001011000,
        run: 57,
    },
    Code {
        bits: 12,
        value: 0b000001011001,
        run: 58,
    },
    Code {
        bits: 12,
        value: 0b000001011010,
        run: 61,
    },
    Code {
        bits: 12,
        value: 0b000001011011,
        run: 256,
    },
    Code {
        bits: 12,
        value: 0b000001100100,
        run: 48,
    },
    Code {
        bits: 12,
        value: 0b000001100101,
        run: 49,
    },
    Code {
        bits: 12,
        value: 0b000001100110,
        run: 62,
    },
    Code {
        bits: 12,
        value: 0b000001100111,
        run: 63,
    },
    Code {
        bits: 12,
        value: 0b000001101000,
        run: 30,
    },
    Code {
        bits: 12,
        value: 0b000001101001,
        run: 31,
    },
    Code {
        bits: 12,
        value: 0b000001101010,
        run: 32,
    },
    Code {
        bits: 12,
        value: 0b000001101011,
        run: 33,
    },
    Code {
        bits: 12,
        value: 0b000001101100,
        run: 40,
    },
    Code {
        bits: 12,
        value: 0b000001101101,
        run: 41,
    },
    Code {
        bits: 12,
        value: 0b000011001000,
        run: 128,
    },
    Code {
        bits: 12,
        value: 0b000011001001,
        run: 192,
    },
    Code {
        bits: 12,
        value: 0b000011001010,
        run: 26,
    },
    Code {
        bits: 12,
        value: 0b000011001011,
        run: 27,
    },
    Code {
        bits: 12,
        value: 0b000011001100,
        run: 28,
    },
    Code {
        bits: 12,
        value: 0b000011001101,
        run: 29,
    },
    Code {
        bits: 12,
        value: 0b000011010010,
        run: 34,
    },
    Code {
        bits: 12,
        value: 0b000011010011,
        run: 35,
    },
    Code {
        bits: 12,
        value: 0b000011010100,
        run: 36,
    },
    Code {
        bits: 12,
        value: 0b000011010101,
        run: 37,
    },
    Code {
        bits: 12,
        value: 0b000011010110,
        run: 38,
    },
    Code {
        bits: 12,
        value: 0b000011010111,
        run: 39,
    },
    Code {
        bits: 12,
        value: 0b000011011010,
        run: 42,
    },
    Code {
        bits: 12,
        value: 0b000011011011,
        run: 43,
    },
    Code {
        bits: 13,
        value: 0b0000001001010,
        run: 640,
    },
    Code {
        bits: 13,
        value: 0b0000001001011,
        run: 704,
    },
    Code {
        bits: 13,
        value: 0b0000001001100,
        run: 768,
    },
    Code {
        bits: 13,
        value: 0b0000001001101,
        run: 832,
    },
    Code {
        bits: 13,
        value: 0b0000001010010,
        run: 1280,
    },
    Code {
        bits: 13,
        value: 0b0000001010011,
        run: 1344,
    },
    Code {
        bits: 13,
        value: 0b0000001010100,
        run: 1408,
    },
    Code {
        bits: 13,
        value: 0b0000001010101,
        run: 1472,
    },
    Code {
        bits: 13,
        value: 0b0000001011010,
        run: 1536,
    },
    Code {
        bits: 13,
        value: 0b0000001011011,
        run: 1600,
    },
    Code {
        bits: 13,
        value: 0b0000001100100,
        run: 1664,
    },
    Code {
        bits: 13,
        value: 0b0000001100101,
        run: 1728,
    },
    Code {
        bits: 13,
        value: 0b0000001101100,
        run: 512,
    },
    Code {
        bits: 13,
        value: 0b0000001101101,
        run: 576,
    },
    Code {
        bits: 13,
        value: 0b0000001110010,
        run: 896,
    },
    Code {
        bits: 13,
        value: 0b0000001110011,
        run: 960,
    },
    Code {
        bits: 13,
        value: 0b0000001110100,
        run: 1024,
    },
    Code {
        bits: 13,
        value: 0b0000001110101,
        run: 1088,
    },
    Code {
        bits: 13,
        value: 0b0000001110110,
        run: 1152,
    },
    Code {
        bits: 13,
        value: 0b0000001110111,
        run: 1216,
    },
];

const LONGEST: u32 = 13;

struct Table {
    entries: Vec<u32>,
}

const NONE: u32 = u32::MAX;

impl Table {
    fn of(codes: &[Code]) -> Option<Self> {
        let mut entries = vec![NONE; 1 << LONGEST];
        for entry in codes {
            let span = LONGEST - u32::from(entry.bits);
            let first = u32::from(entry.value) << span;
            let packed = (u32::from(entry.run) << 4) | u32::from(entry.bits);
            for value in first..first + (1 << span) {
                let slot = entries.get_mut(value as usize)?;
                if *slot != NONE {
                    return None;
                }
                *slot = packed;
            }
        }
        Some(Self { entries })
    }

    fn lookup(&self, peeked: u32) -> Option<(u16, u32)> {
        let packed = *self.entries.get(peeked as usize)?;
        if packed == NONE {
            return None;
        }
        #[allow(clippy::cast_possible_truncation)]
        Some(((packed >> 4) as u16, packed & 0xf))
    }
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CcittParameters {
    pub k: i64,
    pub columns: u32,
    pub rows: u32,
    pub black_is_1: bool,
    pub byte_align: bool,
    pub end_of_line: bool,
    pub end_of_block: bool,
}

impl Default for CcittParameters {
    fn default() -> Self {
        Self {
            k: 0,
            columns: 1728,
            rows: 0,
            black_is_1: false,
            byte_align: false,
            end_of_line: false,
            end_of_block: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CcittError {
    UnsupportedK(i64),
    Columns(u32),
    Rows(u32),
    ColumnsWidthMismatch { columns: u32, width: u32 },
    Budget { needed: usize, allowed: usize },
    Truncated { row: u32, of: u32 },
    UnknownCode { row: u32 },
    RowNotProgressing { row: u32 },
    RowsShortOfImage { rows: u32, height: u32 },
    MissingEndOfLine { row: u32 },
    OutOfLine { row: u32 },
}

impl core::fmt::Display for CcittError {
    fn fmt(&self, out: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnsupportedK(k) => write!(
                out,
                "CCITT /K {k}: only two-dimensional coding (/K negative) is decoded"
            ),
            Self::Columns(columns) => write!(out, "CCITT /Columns {columns} is not a fax line"),
            Self::Rows(rows) => write!(out, "CCITT image of {rows} rows"),
            Self::ColumnsWidthMismatch { columns, width } => write!(
                out,
                "CCITT /Columns {columns} against an image {width} samples wide"
            ),
            Self::Budget { needed, allowed } => {
                write!(out, "CCITT image needs {needed} bytes, limit {allowed}")
            }
            Self::Truncated { row, of } => {
                write!(out, "CCITT data ends after row {row} of {of}")
            }
            Self::UnknownCode { row } => write!(out, "CCITT row {row}: no such code"),
            Self::RowNotProgressing { row } => {
                write!(out, "CCITT row {row} does not advance")
            }
            Self::RowsShortOfImage { rows, height } => write!(
                out,
                "CCITT /Rows {rows} ends the data before the image's {height} rows"
            ),
            Self::MissingEndOfLine { row } => {
                write!(out, "CCITT row {row} begins with no /EndOfLine code")
            }
            Self::OutOfLine { row } => {
                write!(
                    out,
                    "CCITT row {row} places an element past its last column"
                )
            }
        }
    }
}

const MAX_COLUMNS: u32 = 65535;
const MAX_ROWS: u32 = 65535;

struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Bits<'a> {
    const fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    const fn len(&self) -> usize {
        self.data.len() * 8
    }

    const fn spent(&self) -> bool {
        self.at >= self.len()
    }

    fn next(&mut self) -> Option<bool> {
        let bit = self.peek(1)?;
        self.at += 1;
        Some(bit == 1)
    }

    fn peek(&self, count: u32) -> Option<u32> {
        if self.spent() {
            return None;
        }
        let mut value = 0_u32;
        for step in 0..count as usize {
            let at = self.at + step;
            let bit = if at >= self.len() {
                0
            } else {
                u32::from(self.data[at / 8] >> (7 - (at % 8)) & 1)
            };
            value = (value << 1) | bit;
        }
        Some(value)
    }

    fn skip(&mut self, count: usize) {
        self.at = self.at.saturating_add(count);
    }

    fn align(&mut self) {
        while !self.at.is_multiple_of(8) {
            if self.at >= self.len() {
                self.at = self.at.div_ceil(8) * 8;
                return;
            }
            if self.data[self.at / 8] >> (7 - (self.at % 8)) & 1 == 1 {
                return;
            }
            self.at += 1;
        }
    }

    fn skip_eol(&mut self) -> bool {
        let start = self.at;
        while self.at < self.len() {
            let one = self.data[self.at / 8] >> (7 - (self.at % 8)) & 1 == 1;
            self.at += 1;
            if one {
                if self.at - start <= 11 {
                    self.at = start;
                    return false;
                }
                return true;
            }
        }
        self.at = start;
        false
    }
}

fn bit_at(row: &[u8], column: usize) -> bool {
    row.get(column / 8)
        .is_some_and(|byte| byte >> (7 - (column % 8)) & 1 == 1)
}

fn find_bit(row: &[u8], columns: usize, from: usize, want: bool) -> usize {
    let mut at = from;
    while at < columns {
        if bit_at(row, at) == want {
            return at;
        }
        at += 1;
    }
    columns
}

fn fill_black(row: &mut [u8], columns: usize, from: isize, to: isize) {
    let start = usize::try_from(from.max(0)).unwrap_or(0);
    let end = usize::try_from(to.max(0)).unwrap_or(0).min(columns);
    for column in start..end {
        if let Some(byte) = row.get_mut(column / 8) {
            *byte &= !(1 << (7 - (column % 8)));
        }
    }
}

fn changing_elements(
    reference: &[u8],
    columns: usize,
    a0: isize,
    a0_white: bool,
) -> (usize, usize) {
    let mut colour = a0 < 0 || usize::try_from(a0).is_ok_and(|column| bit_at(reference, column));
    let start = usize::try_from((a0 + 1).max(0)).unwrap_or(0);
    let mut b1 = find_bit(reference, columns, start, !colour);
    if b1 >= columns {
        return (columns, columns);
    }
    if colour != a0_white {
        b1 = find_bit(reference, columns, b1 + 1, colour);
        colour = !colour;
    }
    if b1 >= columns {
        return (columns, columns);
    }
    (b1, find_bit(reference, columns, b1 + 1, colour))
}

enum RowEnd {
    Complete,
    OutOfData,
    EndOfBlock,
    UnknownCode,
    NotProgressing,
    OutOfLine,
}

fn read_run(bits: &mut Bits<'_>, table: &Table, columns: usize) -> Result<usize, RowEnd> {
    let mut total = 0_usize;
    loop {
        if bits.spent() {
            return Err(RowEnd::OutOfData);
        }
        let peeked = bits.peek(LONGEST).ok_or(RowEnd::OutOfData)?;
        let Some((run, length)) = table.lookup(peeked) else {
            return Err(RowEnd::UnknownCode);
        };
        if bits.at + length as usize > bits.len() {
            return Err(RowEnd::OutOfData);
        }
        bits.skip(length as usize);
        total += usize::from(run);
        if total > columns {
            return Err(RowEnd::UnknownCode);
        }
        if run < 64 {
            return Ok(total);
        }
    }
}

enum Mode {
    Vertical(isize),
    Horizontal,
    Pass,
    Stop,
    OutOfData,
}

fn read_mode(bits: &mut Bits<'_>) -> Mode {
    let Some(first) = bits.next() else {
        return Mode::OutOfData;
    };
    if first {
        return Mode::Vertical(0);
    }
    let (Some(second), Some(third)) = (bits.next(), bits.next()) else {
        return Mode::OutOfData;
    };
    if second {
        return Mode::Vertical(if third { 1 } else { -1 });
    }
    if third {
        return Mode::Horizontal;
    }
    let Some(fourth) = bits.next() else {
        return Mode::OutOfData;
    };
    if fourth {
        return Mode::Pass;
    }
    let (Some(fifth), Some(sixth)) = (bits.next(), bits.next()) else {
        return Mode::OutOfData;
    };
    if fifth {
        return Mode::Vertical(if sixth { 2 } else { -2 });
    }
    if sixth {
        return match bits.next() {
            Some(seventh) => Mode::Vertical(if seventh { 3 } else { -3 }),
            None => Mode::OutOfData,
        };
    }
    Mode::Stop
}

fn decode_row(
    bits: &mut Bits<'_>,
    row: &mut [u8],
    reference: &[u8],
    columns: usize,
    white: &Table,
    black: &Table,
) -> RowEnd {
    let width = isize::try_from(columns).unwrap_or(isize::MAX);
    let at = |value: usize| isize::try_from(value).unwrap_or(isize::MAX);
    let mut a0: isize = -1;
    let mut a0_white = true;
    let mut turns = 0_usize;
    let ceiling = columns.saturating_mul(2).saturating_add(64);
    loop {
        turns += 1;
        if turns > ceiling {
            return RowEnd::NotProgressing;
        }
        if bits.spent() {
            return RowEnd::OutOfData;
        }
        let (b1, b2) = changing_elements(reference, columns, a0, a0_white);
        let delta = match read_mode(bits) {
            Mode::Vertical(delta) => delta,
            Mode::OutOfData => return RowEnd::OutOfData,
            Mode::Stop => return RowEnd::EndOfBlock,
            Mode::Pass => {
                if !a0_white {
                    fill_black(row, columns, a0, at(b2));
                }
                if b2 >= columns {
                    return RowEnd::Complete;
                }
                if at(b2) < a0 {
                    return RowEnd::NotProgressing;
                }
                a0 = at(b2);
                continue;
            }
            Mode::Horizontal => {
                let (near, far) = if a0_white {
                    (white, black)
                } else {
                    (black, white)
                };
                let first = match read_run(bits, near, columns) {
                    Ok(run) => run,
                    Err(end) => return end,
                };
                let a1 = a0 + at(first) + isize::from(a0 < 0);
                if a1 > width {
                    return RowEnd::OutOfLine;
                }
                if !a0_white {
                    fill_black(row, columns, a0, a1);
                }
                let second = match read_run(bits, far, columns) {
                    Ok(run) => run,
                    Err(end) => return end,
                };
                let a2 = a1 + at(second);
                if a2 > width {
                    return RowEnd::OutOfLine;
                }
                if a0_white {
                    fill_black(row, columns, a1, a2);
                }
                if a2 < a0 {
                    return RowEnd::NotProgressing;
                }
                a0 = a2;
                if a0 >= width {
                    return RowEnd::Complete;
                }
                continue;
            }
        };
        let a1 = at(b1) + delta;
        if a1 > width {
            return RowEnd::OutOfLine;
        }
        if !a0_white {
            fill_black(row, columns, a0, a1);
        }
        if a1 >= width {
            return RowEnd::Complete;
        }
        if a1 <= a0 {
            return RowEnd::NotProgressing;
        }
        a0 = a1;
        a0_white = !a0_white;
    }
}

pub fn decode(
    data: &[u8],
    parameters: &CcittParameters,
    width: u32,
    height: u32,
    max_bytes: usize,
) -> Result<Vec<u8>, CcittError> {
    if parameters.k >= 0 {
        return Err(CcittError::UnsupportedK(parameters.k));
    }
    let columns = parameters.columns;
    if columns == 0 || columns > MAX_COLUMNS {
        return Err(CcittError::Columns(columns));
    }
    if columns != width {
        return Err(CcittError::ColumnsWidthMismatch { columns, width });
    }
    if !parameters.end_of_block && parameters.rows != 0 && parameters.rows < height {
        return Err(CcittError::RowsShortOfImage {
            rows: parameters.rows,
            height,
        });
    }
    let rows = height;
    if rows == 0 || rows > MAX_ROWS {
        return Err(CcittError::Rows(rows));
    }
    let pitch = (columns as usize).div_ceil(8);
    let needed = pitch.checked_mul(rows as usize).ok_or(CcittError::Budget {
        needed: usize::MAX,
        allowed: max_bytes,
    })?;
    if needed > max_bytes {
        return Err(CcittError::Budget {
            needed,
            allowed: max_bytes,
        });
    }
    let white = Table::of(&WHITE).ok_or(CcittError::UnknownCode { row: 0 })?;
    let black = Table::of(&BLACK).ok_or(CcittError::UnknownCode { row: 0 })?;

    let mut bits = Bits::new(data);
    let mut out = vec![0_u8; needed];
    let mut reference = vec![0xff_u8; pitch];
    for row in 0..rows {
        if parameters.byte_align {
            bits.align();
        }
        let marked = bits.skip_eol();
        if parameters.end_of_line && !marked {
            return Err(CcittError::MissingEndOfLine { row });
        }
        if bits.spent() {
            return Err(CcittError::Truncated { row, of: rows });
        }
        let start = row as usize * pitch;
        let line = &mut out[start..start + pitch];
        line.fill(0xff);
        let end = decode_row(
            &mut bits,
            line,
            &reference,
            columns as usize,
            &white,
            &black,
        );
        match end {
            RowEnd::Complete => {}
            RowEnd::OutOfData | RowEnd::EndOfBlock => {
                return Err(CcittError::Truncated { row, of: rows });
            }
            RowEnd::UnknownCode => return Err(CcittError::UnknownCode { row }),
            RowEnd::NotProgressing => return Err(CcittError::RowNotProgressing { row }),
            RowEnd::OutOfLine => return Err(CcittError::OutOfLine { row }),
        }
        reference.copy_from_slice(line);
    }
    if parameters.black_is_1 {
        for byte in &mut out {
            *byte = !*byte;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{BLACK, CcittError, CcittParameters, Code, LONGEST, Table, WHITE, decode};

    fn hex(text: &str) -> Vec<u8> {
        text.as_bytes()
            .chunks(2)
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).expect("ascii"), 16).expect("hex")
            })
            .collect()
    }

    fn bitmap(rows: &[&str]) -> Vec<u8> {
        let columns = rows[0].len();
        let pitch = columns.div_ceil(8);
        let mut out = vec![0xff_u8; pitch * rows.len()];
        for (y, row) in rows.iter().enumerate() {
            assert_eq!(row.len(), columns, "the fixture's rows are not equal");
            for (x, cell) in row.bytes().enumerate() {
                if cell == b'#' {
                    out[y * pitch + x / 8] &= !(1 << (7 - (x % 8)));
                }
            }
        }
        out
    }

    fn g4(columns: u32) -> CcittParameters {
        CcittParameters {
            k: -1,
            columns,
            ..CcittParameters::default()
        }
    }

    #[test]
    fn the_code_tables_are_prefix_free_and_cover_every_run() {
        assert_eq!(
            WHITE.len(),
            104,
            "T.4 tables 2, 3 and 4 hold 104 white codes"
        );
        assert_eq!(BLACK.len(), 104);
        assert!(
            Table::of(&WHITE).is_some(),
            "the white table is not prefix-free"
        );
        assert!(Table::of(&BLACK).is_some());
        for table in [&WHITE, &BLACK] {
            assert!(
                table.iter().all(|code| u32::from(code.bits) <= LONGEST),
                "a code is longer than the peek this decoder uses"
            );
            for run in 0..=63_u16 {
                assert!(
                    table.iter().any(|code| code.run == run),
                    "terminating run {run} is missing"
                );
            }
        }
    }

    #[test]
    fn a_table_whose_codes_overlap_is_refused() {
        let broken = [
            Code {
                bits: 2,
                value: 0b01,
                run: 1,
            },
            Code {
                bits: 3,
                value: 0b011,
                run: 2,
            },
        ];
        assert!(
            Table::of(&broken).is_none(),
            "a code that is a prefix of another was accepted"
        );
    }

    #[test]
    fn a_single_dot_comes_back_where_it_was_put() {
        let rows = ["...#....", "........"];
        let decoded = decode(&hex("30a3001001"), &g4(8), 8, 2, 4096).expect("it decodes");
        assert_eq!(decoded, bitmap(&rows));
    }

    #[test]
    fn stripes_repeat_down_the_reference_line() {
        let rows = [
            "##..##..##..##..",
            "##..##..##..##..",
            "##..##..##..##..",
            "##..##..##..##..",
        ];
        let decoded =
            decode(&hex("26b97cbe5ffffffff0010010"), &g4(16), 16, 4, 4096).expect("it decodes");
        assert_eq!(decoded, bitmap(&rows));
    }

    #[test]
    fn a_run_longer_than_a_terminating_code_uses_the_make_up_tables() {
        let mut middle = String::new();
        middle.push_str(&".".repeat(10));
        middle.push_str(&"#".repeat(180));
        middle.push_str(&".".repeat(10));
        let blank = ".".repeat(200);
        let rows = [blank.as_str(), middle.as_str(), blank.as_str()];
        let decoded = decode(&hex("9386401246002002"), &g4(200), 200, 3, 4096).expect("it decodes");
        assert_eq!(decoded, bitmap(&rows));
    }

    #[test]
    fn a_random_field_comes_back_exactly() {
        let rows = [
            ".#.###.####..",
            "###.####.####",
            "..##.###.####",
            ".......####..",
            "...##.#.#..##",
            ".....##..##..",
            "...#..##.##.#",
            "#...##...#...",
            "..########.#.",
        ];
        let data = hex("23a23c4774c108223b0d91ec4310a63450e1050ec28409bd4102615310861280080080");
        let decoded = decode(&data, &g4(13), 13, 9, 4096).expect("it decodes");
        assert_eq!(decoded, bitmap(&rows));
    }

    #[test]
    fn black_is_one_inverts_every_byte_and_changes_no_geometry() {
        let plain = decode(&hex("30a3001001"), &g4(8), 8, 2, 4096).expect("it decodes");
        let inverted = decode(
            &hex("30a3001001"),
            &CcittParameters {
                black_is_1: true,
                ..g4(8)
            },
            8,
            2,
            4096,
        )
        .expect("it decodes");
        assert_eq!(
            inverted,
            plain.iter().map(|byte| !byte).collect::<Vec<u8>>()
        );
    }

    #[test]
    fn encoded_byte_align_starts_each_row_on_a_boundary() {
        let packed = decode(&[0b1110_0000], &g4(8), 8, 3, 4096).expect("three V(0) rows");
        assert_eq!(packed, bitmap(&["........", "........", "........"]));

        let aligned = CcittParameters {
            byte_align: true,
            ..g4(8)
        };
        let spread = decode(&[0x80, 0x80, 0x80], &aligned, 8, 3, 4096).expect("three padded rows");
        assert_eq!(spread, packed);

        assert!(
            decode(&[0x80, 0x80, 0x80], &g4(8), 8, 3, 4096).is_err(),
            "padding was read as codes and still produced an image"
        );
    }

    #[test]
    fn end_of_line_codes_are_tolerated_by_default_and_required_when_declared() {
        let marked = [0b0000_0000, 0b0001_1000, 0b0000_0000, 0b0000_1100];
        let required = CcittParameters {
            end_of_line: true,
            ..g4(8)
        };
        let two = bitmap(&["........", "........"]);
        assert_eq!(
            decode(&marked, &required, 8, 2, 4096),
            Ok(two.clone()),
            "a declared marker that is present was not read"
        );
        assert_eq!(
            decode(&marked, &g4(8), 8, 2, 4096),
            Ok(two.clone()),
            "the default forbade a marker rather than tolerating it"
        );

        let bare = hex("c0040040");
        assert_eq!(decode(&bare, &g4(8), 8, 2, 4096), Ok(two));
        assert_eq!(
            decode(&bare, &required, 8, 2, 4096),
            Err(CcittError::MissingEndOfLine { row: 0 }),
            "a required marker that is absent was not noticed"
        );
    }

    #[test]
    fn end_of_block_overrides_rows_and_a_terminating_short_rows_is_refused() {
        let stream = hex("c0040040");
        let two = bitmap(&["........", "........"]);
        assert_eq!(decode(&stream, &g4(8), 8, 2, 4096), Ok(two.clone()));

        let overridden = CcittParameters { rows: 1, ..g4(8) };
        assert_eq!(
            decode(&stream, &overridden, 8, 2, 4096),
            Ok(two),
            "/Rows shortened an image that /EndOfBlock terminates"
        );

        let terminating = CcittParameters {
            rows: 1,
            end_of_block: false,
            ..g4(8)
        };
        assert_eq!(
            decode(&stream, &terminating, 8, 2, 4096),
            Err(CcittError::RowsShortOfImage { rows: 1, height: 2 })
        );
        let exact = CcittParameters {
            rows: 2,
            end_of_block: false,
            ..g4(8)
        };
        assert!(decode(&stream, &exact, 8, 2, 4096).is_ok());
    }

    #[test]
    fn an_element_past_the_last_column_is_refused_rather_than_clipped() {
        assert_eq!(
            decode(&[0b0110_0000], &g4(8), 8, 1, 4096),
            Err(CcittError::OutOfLine { row: 0 })
        );
        assert_eq!(
            decode(&[0b0011_1110, 0b1100_0000], &g4(8), 8, 1, 4096),
            Err(CcittError::OutOfLine { row: 0 })
        );
        assert_eq!(
            decode(&[0b1000_0000], &g4(8), 8, 1, 4096),
            Ok(bitmap(&["........"]))
        );
    }

    #[test]
    fn one_dimensional_coding_is_refused_by_name() {
        for k in [0, 1, 4] {
            let parameters = CcittParameters { k, ..g4(8) };
            assert_eq!(
                decode(&hex("30a3001001"), &parameters, 8, 2, 4096),
                Err(CcittError::UnsupportedK(k))
            );
        }
    }

    #[test]
    fn an_impossible_line_shape_is_refused() {
        assert_eq!(
            decode(
                &[0xff],
                &CcittParameters {
                    columns: 0,
                    ..g4(8)
                },
                0,
                2,
                4096
            ),
            Err(CcittError::Columns(0))
        );
        assert_eq!(
            decode(&[0xff], &g4(1728), 16, 2, 4096),
            Err(CcittError::ColumnsWidthMismatch {
                columns: 1728,
                width: 16
            }),
            "a stream whose columns are not the image's width was decoded anyway"
        );
        assert_eq!(
            decode(&[0xff], &g4(8), 8, 0, 4096),
            Err(CcittError::Rows(0))
        );
    }

    #[test]
    fn an_image_larger_than_the_budget_is_refused() {
        let parameters = g4(4000);
        assert_eq!(
            decode(&[0xff; 16], &parameters, 4000, 4000, 1024),
            Err(CcittError::Budget {
                needed: 500 * 4000,
                allowed: 1024
            })
        );
    }

    #[test]
    fn a_stream_that_ends_early_is_refused_rather_than_left_white() {
        assert_eq!(
            decode(&[0b1110_0000], &g4(8), 8, 4, 4096),
            Err(CcittError::Truncated { row: 3, of: 4 })
        );
        let full = hex("30a3001001");
        assert!(
            decode(&full, &g4(8), 8, 2, 4096).is_ok(),
            "the whole fixture is what is being cut down"
        );
        let short = &full[..1];
        assert!(
            matches!(
                decode(short, &g4(8), 8, 2, 4096),
                Err(CcittError::Truncated { .. } | CcittError::UnknownCode { .. })
            ),
            "a cut stream produced an image: {:?}",
            decode(short, &g4(8), 8, 2, 4096)
        );
    }

    #[test]
    fn bits_that_are_not_a_code_are_refused() {
        let stream = [0b0010_0000, 0b0000_0000, 0b0000_0000, 0b0000_0000];
        assert!(
            matches!(
                decode(&stream, &g4(64), 64, 1, 4096),
                Err(CcittError::UnknownCode { row: 0 })
            ),
            "an impossible code was decoded as something"
        );
    }

    #[test]
    fn every_truncation_of_a_real_stream_is_answered_rather_than_crashed() {
        let full = hex("23a23c4774c108223b0d91ec4310a63450e1050ec28409bd4102615310861280080080");
        for length in 0..=full.len() {
            if let Ok(samples) = decode(&full[..length], &g4(13), 13, 9, 4096) {
                assert_eq!(samples.len(), 2 * 9);
            }
        }
        for flip in 0..full.len() {
            let mut damaged = full.clone();
            damaged[flip] ^= 0xa5;
            if let Ok(samples) = decode(&damaged, &g4(13), 13, 9, 4096) {
                assert_eq!(samples.len(), 2 * 9);
            }
        }
    }
}
