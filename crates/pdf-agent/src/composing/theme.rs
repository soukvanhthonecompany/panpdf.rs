use super::Colour;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    pub ink: Colour,
    pub accent: Colour,
    pub on_accent: Colour,
    pub soft: Colour,
    pub stripe: Colour,
    pub line: Colour,
    pub shadow: Colour,
    pub code: Colour,
    pub paper: Option<Colour>,
    pub palette: [Colour; 6],
    pub rise: Colour,
    pub fall: Colour,
    pub dressed: bool,
}

#[must_use]
pub const fn rgb(hex: u32) -> Colour {
    [
        ((hex >> 16) & 0xFF) as f64 / 255.0,
        ((hex >> 8) & 0xFF) as f64 / 255.0,
        (hex & 0xFF) as f64 / 255.0,
    ]
}

const INK: Colour = rgb(0x1F_29_37);
const WHITE: Colour = rgb(0xFF_FF_FF);
const SHADOW: Colour = rgb(0xD5_DB_E5);
const CODE: Colour = rgb(0xF3_F4_F6);
const RISE: Colour = rgb(0x16_A3_4A);
const FALL: Colour = rgb(0xDC_26_26);

pub const THEMES: [Theme; 9] = [
    Theme {
        name: "classic",
        ink: INK,
        accent: rgb(0x1E_3A_8A),
        on_accent: WHITE,
        soft: rgb(0xEE_F2_FF),
        stripe: rgb(0xF5_F7_FB),
        line: rgb(0xCB_D5_E1),
        shadow: SHADOW,
        code: CODE,
        paper: None,
        palette: [
            rgb(0x25_63_EB),
            rgb(0xF5_9E_0B),
            rgb(0x10_B9_81),
            rgb(0xEF_44_44),
            rgb(0x8B_5C_F6),
            rgb(0x06_B6_D4),
        ],
        rise: RISE,
        fall: FALL,
        dressed: true,
    },
    Theme {
        name: "ocean",
        ink: INK,
        accent: rgb(0x0E_74_90),
        on_accent: WHITE,
        soft: rgb(0xEC_FE_FF),
        stripe: rgb(0xF0_FB_FD),
        line: rgb(0xBA_E6_FD),
        shadow: SHADOW,
        code: CODE,
        paper: None,
        palette: [
            rgb(0x08_91_B2),
            rgb(0x25_63_EB),
            rgb(0x14_B8_A6),
            rgb(0x63_66_F1),
            rgb(0x0E_A5_E9),
            rgb(0x22_C5_5E),
        ],
        rise: RISE,
        fall: FALL,
        dressed: true,
    },
    Theme {
        name: "sunset",
        ink: INK,
        accent: rgb(0xC2_41_0C),
        on_accent: WHITE,
        soft: rgb(0xFF_F7_ED),
        stripe: rgb(0xFF_FA_F3),
        line: rgb(0xFE_D7_AA),
        shadow: SHADOW,
        code: CODE,
        paper: None,
        palette: [
            rgb(0x00F9_7316),
            rgb(0xE1_1D_48),
            rgb(0xF5_9E_0B),
            rgb(0xDB_27_77),
            rgb(0x7C_2D_12),
            rgb(0xFA_CC_15),
        ],
        rise: RISE,
        fall: FALL,
        dressed: true,
    },
    Theme {
        name: "forest",
        ink: INK,
        accent: rgb(0x16_65_34),
        on_accent: WHITE,
        soft: rgb(0xF0_FD_F4),
        stripe: rgb(0xF6_FD_F8),
        line: rgb(0xBB_F7_D0),
        shadow: SHADOW,
        code: CODE,
        paper: None,
        palette: [
            rgb(0x16_A3_4A),
            rgb(0xCA_8A_04),
            rgb(0x0D_94_88),
            rgb(0x65_A3_0D),
            rgb(0x92_40_0E),
            rgb(0x0084_CC16),
        ],
        rise: RISE,
        fall: FALL,
        dressed: true,
    },
    Theme {
        name: "grape",
        ink: INK,
        accent: rgb(0x6D_28_D9),
        on_accent: WHITE,
        soft: rgb(0xF5_F3_FF),
        stripe: rgb(0xFA_F8_FF),
        line: rgb(0xDD_D6_FE),
        shadow: SHADOW,
        code: CODE,
        paper: None,
        palette: [
            rgb(0x7C_3A_ED),
            rgb(0xDB_27_77),
            rgb(0x25_63_EB),
            rgb(0xF5_9E_0B),
            rgb(0xA8_55_F7),
            rgb(0x06_B6_D4),
        ],
        rise: RISE,
        fall: FALL,
        dressed: true,
    },
    Theme {
        name: "rose",
        ink: INK,
        accent: rgb(0xBE_18_5D),
        on_accent: WHITE,
        soft: rgb(0xFD_F2_F8),
        stripe: rgb(0xFE_F7_FB),
        line: rgb(0xFB_CF_E8),
        shadow: SHADOW,
        code: CODE,
        paper: None,
        palette: [
            rgb(0xDB_27_77),
            rgb(0x7C_3A_ED),
            rgb(0xF4_3F_5E),
            rgb(0xF5_9E_0B),
            rgb(0xEC_48_99),
            rgb(0x0E_A5_E9),
        ],
        rise: RISE,
        fall: FALL,
        dressed: true,
    },
    Theme {
        name: "slate",
        ink: INK,
        accent: rgb(0x33_41_55),
        on_accent: WHITE,
        soft: rgb(0xF1_F5_F9),
        stripe: rgb(0xF8_FA_FC),
        line: rgb(0xCB_D5_E1),
        shadow: SHADOW,
        code: CODE,
        paper: None,
        palette: [
            rgb(0x33_41_55),
            rgb(0x25_63_EB),
            rgb(0x94_A3_B8),
            rgb(0xF5_9E_0B),
            rgb(0x64_74_8B),
            rgb(0x10_B9_81),
        ],
        rise: RISE,
        fall: FALL,
        dressed: true,
    },
    Theme {
        name: "midnight",
        ink: rgb(0xE2_E8_F0),
        accent: rgb(0x38_BD_F8),
        on_accent: rgb(0x0F_17_2A),
        soft: rgb(0x1E_29_3B),
        stripe: rgb(0x16_20_33),
        line: rgb(0x33_41_55),
        shadow: rgb(0x02_06_17),
        code: rgb(0x1E_29_3B),
        paper: Some(rgb(0x0F_17_2A)),
        palette: [
            rgb(0x38_BD_F8),
            rgb(0xF4_72_B6),
            rgb(0x4A_DE_80),
            rgb(0xFA_CC_15),
            rgb(0xA7_8B_FA),
            rgb(0xFB_92_3C),
        ],
        rise: rgb(0x4A_DE_80),
        fall: rgb(0xF8_71_71),
        dressed: true,
    },
    Theme {
        name: "plain",
        ink: rgb(0x00_00_00),
        accent: rgb(0x00_00_00),
        on_accent: WHITE,
        soft: rgb(0xFF_FF_FF),
        stripe: rgb(0xFF_FF_FF),
        line: rgb(0x8C_8C_8C),
        shadow: rgb(0xFF_FF_FF),
        code: rgb(0xFF_FF_FF),
        paper: None,
        palette: [
            rgb(0x33_33_33),
            rgb(0x77_77_77),
            rgb(0xAA_AA_AA),
            rgb(0x55_55_55),
            rgb(0x99_99_99),
            rgb(0x22_22_22),
        ],
        rise: rgb(0x33_33_33),
        fall: rgb(0xAA_AA_AA),
        dressed: false,
    },
];

#[must_use]
pub fn named(name: &str) -> Option<&'static Theme> {
    THEMES
        .iter()
        .find(|theme| theme.name.eq_ignore_ascii_case(name.trim()))
}

#[must_use]
pub fn default_theme() -> &'static Theme {
    &THEMES[0]
}

#[must_use]
pub fn names() -> Vec<&'static str> {
    THEMES.iter().map(|theme| theme.name).collect()
}

#[must_use]
pub fn paler(colour: Colour, share: f64) -> Colour {
    colour.map(|part| part + (1.0 - part) * share.clamp(0.0, 1.0))
}

#[must_use]
pub fn darker(colour: Colour, share: f64) -> Colour {
    colour.map(|part| part * (1.0 - share.clamp(0.0, 1.0)))
}
