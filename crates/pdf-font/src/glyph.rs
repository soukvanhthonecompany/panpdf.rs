#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GlyphSegment {
    MoveTo {
        x: f64,
        y: f64,
    },
    LineTo {
        x: f64,
        y: f64,
    },
    CurveTo {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        x: f64,
        y: f64,
    },
    Close,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlyphPath {
    pub segments: Vec<GlyphSegment>,
}

#[derive(Default)]
pub struct PathCache(std::sync::RwLock<std::collections::HashMap<u16, Option<GlyphPath>>>);

impl PathCache {
    pub fn get_or(
        &self,
        glyph: u16,
        draw: impl FnOnce() -> Option<GlyphPath>,
    ) -> Option<GlyphPath> {
        if let Some(known) = self
            .0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&glyph)
        {
            return known.clone();
        }
        let drawn = draw();
        self.0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(glyph, drawn.clone());
        drawn
    }
}

impl Clone for PathCache {
    fn clone(&self) -> Self {
        Self(std::sync::RwLock::new(
            self.0
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        ))
    }
}

impl PartialEq for PathCache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for PathCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let known = self
            .0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len();
        write!(formatter, "PathCache({known} glyphs)")
    }
}

impl GlyphPath {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    pub fn move_to(&mut self, x: f64, y: f64) {
        self.segments.push(GlyphSegment::MoveTo { x, y });
    }

    pub fn line_to(&mut self, x: f64, y: f64) {
        self.segments.push(GlyphSegment::LineTo { x, y });
    }

    pub fn curve_to(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, x: f64, y: f64) {
        self.segments.push(GlyphSegment::CurveTo {
            x1,
            y1,
            x2,
            y2,
            x,
            y,
        });
    }

    pub fn quadratic_to(&mut self, start: (f64, f64), control: (f64, f64), end: (f64, f64)) {
        const THIRD: f64 = 1.0 / 3.0;
        self.curve_to(
            (control.0 - start.0).mul_add(2.0 * THIRD, start.0),
            (control.1 - start.1).mul_add(2.0 * THIRD, start.1),
            (control.0 - end.0).mul_add(2.0 * THIRD, end.0),
            (control.1 - end.1).mul_add(2.0 * THIRD, end.1),
            end.0,
            end.1,
        );
    }

    pub fn translate(&mut self, dx: f64, dy: f64) {
        for segment in &mut self.segments {
            match segment {
                GlyphSegment::MoveTo { x, y } | GlyphSegment::LineTo { x, y } => {
                    *x += dx;
                    *y += dy;
                }
                GlyphSegment::CurveTo {
                    x1,
                    y1,
                    x2,
                    y2,
                    x,
                    y,
                } => {
                    *x1 += dx;
                    *y1 += dy;
                    *x2 += dx;
                    *y2 += dy;
                    *x += dx;
                    *y += dy;
                }
                GlyphSegment::Close => {}
            }
        }
    }

    pub fn close(&mut self) {
        self.segments.push(GlyphSegment::Close);
    }
}

fn looks_like_type1(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(1024)];
    let postscript = head.starts_with(b"%!") || bytes.first() == Some(&0x80);
    let eexec = bytes
        .windows(b"eexec".len())
        .any(|window| window == b"eexec");
    postscript && eexec
}

#[derive(Clone, Debug, PartialEq)]
pub enum GlyphProgram {
    TrueType(crate::truetype::TrueTypeFont),
    Cff {
        font: Box<crate::cff::CffFont>,
        data: Vec<u8>,
        sfnt: Option<Box<crate::truetype::TrueTypeFont>>,
        paths: PathCache,
    },
    Type1 {
        font: Box<crate::type1::Type1Font>,
        data: Vec<u8>,
    },
}

impl GlyphProgram {
    pub fn parse(bytes: Vec<u8>) -> Result<Self, GlyphProgramError> {
        Self::parse_face(bytes, 0)
    }

    #[must_use]
    pub fn face_count(bytes: &[u8]) -> u32 {
        crate::truetype::TrueTypeFont::face_count(bytes).unwrap_or(1)
    }

    pub fn parse_face(bytes: Vec<u8>, face: u32) -> Result<Self, GlyphProgramError> {
        match crate::truetype::TrueTypeFont::parse_face(bytes.clone(), face) {
            Ok(sfnt) => {
                if sfnt.has_outlines() {
                    return Ok(Self::TrueType(sfnt));
                }
                let table = sfnt
                    .cff_table()
                    .ok_or(GlyphProgramError::NoOutlines)?
                    .to_vec();
                let font = crate::cff::CffFont::parse(&table).map_err(GlyphProgramError::Cff)?;
                Ok(Self::Cff {
                    font: Box::new(font),
                    data: table,
                    sfnt: Some(Box::new(sfnt)),
                    paths: PathCache::default(),
                })
            }
            Err(sfnt_error) => {
                if looks_like_type1(&bytes) {
                    let font =
                        crate::type1::Type1Font::parse(&bytes).map_err(GlyphProgramError::Type1)?;
                    return Ok(Self::Type1 {
                        font: Box::new(font),
                        data: bytes,
                    });
                }
                let font = crate::cff::CffFont::parse(&bytes).map_err(|cff_error| {
                    if matches!(sfnt_error, crate::truetype::TrueTypeError::NotSfnt) {
                        GlyphProgramError::Cff(cff_error)
                    } else {
                        GlyphProgramError::TrueType(sfnt_error)
                    }
                })?;
                Ok(Self::Cff {
                    font: Box::new(font),
                    data: bytes,
                    sfnt: None,
                    paths: PathCache::default(),
                })
            }
        }
    }
    #[must_use]
    pub fn program_bytes(&self) -> &[u8] {
        match self {
            Self::TrueType(font) => font.program_bytes(),
            Self::Cff { data, sfnt, .. } => sfnt
                .as_ref()
                .map_or(data.as_slice(), |font| font.program_bytes()),
            Self::Type1 { data, .. } => data.as_slice(),
        }
    }

    #[must_use]
    pub const fn technology(&self) -> &'static str {
        match self {
            Self::TrueType(_) => "truetype",
            Self::Cff { .. } => "cff",
            Self::Type1 { .. } => "type1",
        }
    }

    #[must_use]
    pub fn units_per_em(&self) -> u16 {
        match self {
            Self::TrueType(font) => font.units_per_em(),
            Self::Cff { font, .. } => font.units_per_em(),
            Self::Type1 { font, .. } => font.units_per_em(),
        }
    }

    #[must_use]
    pub fn vertical_metrics(&self) -> Option<(f64, f64)> {
        match self {
            Self::TrueType(font) => font.line_metrics(),
            Self::Cff { sfnt, .. } => sfnt.as_ref()?.line_metrics(),
            Self::Type1 { .. } => None,
        }
    }

    #[must_use]
    pub fn path(&self, glyph: u16) -> Option<GlyphPath> {
        match self {
            Self::TrueType(font) => font.path(glyph),
            Self::Cff {
                font, data, paths, ..
            } => paths.get_or(glyph, || font.outline(data, glyph)),
            Self::Type1 { font, .. } => font.path(glyph),
        }
    }

    #[must_use]
    pub fn glyph_for_code(&self, code: u8) -> Option<u16> {
        match self {
            Self::TrueType(font) => font.glyph_for_code(code).map(|(glyph, _)| glyph),
            Self::Cff { font, sfnt, .. } => sfnt
                .as_ref()
                .and_then(|sfnt| sfnt.glyph_for_code(code))
                .map(|(glyph, _)| glyph)
                .or_else(|| font.glyph_for_code(code)),
            Self::Type1 { font, .. } => font.glyph_for_code(code),
        }
    }

    #[must_use]
    pub fn glyph_for_char(&self, character: char) -> Option<u16> {
        match self {
            Self::TrueType(font) => font.glyph_for_char(character),
            Self::Cff { sfnt, .. } => sfnt.as_ref()?.glyph_for_char(character),
            Self::Type1 { .. } => None,
        }
    }

    #[must_use]
    pub fn is_cid_keyed(&self) -> bool {
        match self {
            Self::TrueType(_) | Self::Type1 { .. } => false,
            Self::Cff { font, .. } => font.is_cid(),
        }
    }

    #[must_use]
    pub fn glyph_for_cid(&self, cid: u16) -> Option<u16> {
        match self {
            Self::TrueType(_) | Self::Type1 { .. } => None,
            Self::Cff { font, .. } => font.glyph_for_cid(cid),
        }
    }

    #[must_use]
    pub fn glyph_for_name(&self, name: &[u8]) -> Option<u16> {
        match self {
            Self::TrueType(font) => font.glyph_for_name(name).map(|(glyph, _)| glyph),
            Self::Cff { font, sfnt, .. } => font.glyph_for_name(name).or_else(|| {
                sfnt.as_ref()
                    .and_then(|sfnt| sfnt.glyph_for_name(name))
                    .map(|(glyph, _)| glyph)
            }),
            Self::Type1 { font, .. } => font.glyph_for_name(name),
        }
    }

    #[must_use]
    pub fn glyph_name(&self, glyph: u16) -> Option<&[u8]> {
        match self {
            Self::TrueType(font) => font.glyph_name(glyph),
            Self::Cff { font, .. } => font.glyph_name(glyph),
            Self::Type1 { font, .. } => font.glyph_name(glyph),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GlyphProgramError {
    TrueType(crate::truetype::TrueTypeError),
    Cff(crate::cff::CffError),
    Type1(crate::type1::Type1Error),
    NoOutlines,
}

impl std::fmt::Display for GlyphProgramError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TrueType(error) => write!(formatter, "{error}"),
            Self::Cff(error) => write!(formatter, "{error}"),
            Self::Type1(error) => write!(formatter, "{error}"),
            Self::NoOutlines => formatter.write_str("font program carries no glyph outlines"),
        }
    }
}

impl std::error::Error for GlyphProgramError {}
