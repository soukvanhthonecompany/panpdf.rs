#![forbid(unsafe_code)]

use pdf_paint::MulAdd as _;
use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use pdf_content::PageGeometry;
use pdf_paint::SoftMaskSubtype;
use pdf_paint::{
    BlendMode, Color, ColorSpace, FillRule, GraphicsState, Matrix, PaintAtom, PaintAtomKind,
    PaintGraph, PaintId, Path, ShadingGeometry, ShadingPaint, SoftMask, TransparencyGroupPaint,
};

mod cmyk_samples;
mod color;
mod cull;
mod geometry;
pub mod icc;
mod raster;
mod shading;
mod stroke;

pub use color::{color_to_rgb, components_to_rgb};
pub use geometry::{DeviceTransform, Polygon};
pub use raster::Mask;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderLimits {
    pub max_pixels: usize,
    pub max_edges: usize,
    pub max_flatten_depth: u32,
    pub max_group_depth: usize,
    pub max_soft_mask_pixels: u64,
}

impl Default for RenderLimits {
    fn default() -> Self {
        Self {
            max_pixels: 64 * 1024 * 1024,
            max_edges: 1_000_000,
            max_flatten_depth: 16,
            max_group_depth: 16,
            max_soft_mask_pixels: 16 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderOptions {
    pub scale: f64,
    pub limits: RenderLimits,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            scale: 1.0,
            limits: RenderLimits::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Canvas {
    pub origin: (u32, u32),
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 3]>,
}

impl Canvas {
    #[must_use]
    pub fn blank(width: u32, height: u32) -> Self {
        Self::window(0, 0, width, height)
    }

    #[must_use]
    pub fn window(x0: u32, y0: u32, width: u32, height: u32) -> Self {
        Self {
            origin: (x0, y0),
            width,
            height,
            pixels: vec![[1.0, 1.0, 1.0]; width as usize * height as usize],
        }
    }

    #[must_use]
    pub const fn bounds(&self) -> (u32, u32, u32, u32) {
        (
            self.origin.0,
            self.origin.1,
            self.origin.0 + self.width,
            self.origin.1 + self.height,
        )
    }

    #[must_use]
    pub const fn columns(&self) -> std::ops::Range<u32> {
        self.origin.0..self.origin.0 + self.width
    }

    #[must_use]
    pub const fn rows(&self) -> std::ops::Range<u32> {
        self.origin.1..self.origin.1 + self.height
    }

    #[must_use]
    pub fn index(&self, x: u32, y: u32) -> Option<usize> {
        let column = x.checked_sub(self.origin.0)?;
        let row = y.checked_sub(self.origin.1)?;
        if column >= self.width || row >= self.height {
            return None;
        }
        Some(row as usize * self.width as usize + column as usize)
    }

    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> [f32; 3] {
        self.pixels[self.index(x, y).expect("pixel inside this canvas")]
    }

    #[must_use]
    pub fn ink_fraction(&self) -> f64 {
        if self.pixels.is_empty() {
            return 0.0;
        }
        let inked = self
            .pixels
            .iter()
            .filter(|pixel| pixel.iter().any(|channel| to_byte(*channel) <= 248))
            .count();
        ratio(inked, self.pixels.len())
    }

    #[must_use]
    pub fn to_rgb8(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.pixels.len() * 3);
        for pixel in &self.pixels {
            bytes.extend(pixel.iter().map(|channel| to_byte(*channel)));
        }
        bytes
    }

    #[must_use]
    pub fn to_rgba8(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.pixels.len() * 4);
        for [red, green, blue] in &self.pixels {
            bytes.extend_from_slice(&[to_byte(*red), to_byte(*green), to_byte(*blue), u8::MAX]);
        }
        bytes
    }

    #[must_use]
    pub fn to_ppm(&self) -> Vec<u8> {
        let mut bytes = format!("P6\n{} {}\n255\n", self.width, self.height).into_bytes();
        for pixel in &self.pixels {
            bytes.extend(pixel.iter().map(|channel| to_byte(*channel)));
        }
        bytes
    }

    #[must_use]
    pub fn to_bmp(&self) -> Vec<u8> {
        const FILE_HEADER_BYTES: u32 = 14;
        const INFO_HEADER_BYTES: u32 = 40;
        const PIXEL_OFFSET: u32 = FILE_HEADER_BYTES + INFO_HEADER_BYTES;

        let row_bytes = self.width * 3;
        let row_padding = (4 - row_bytes % 4) % 4;
        let stored_row_bytes = row_bytes + row_padding;
        let image_bytes = stored_row_bytes * self.height;
        let file_bytes = PIXEL_OFFSET + image_bytes;

        let mut bytes = Vec::with_capacity(file_bytes as usize);
        bytes.extend_from_slice(b"BM");
        bytes.extend_from_slice(&file_bytes.to_le_bytes());
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&PIXEL_OFFSET.to_le_bytes());

        bytes.extend_from_slice(&INFO_HEADER_BYTES.to_le_bytes());
        bytes.extend_from_slice(&self.width.to_le_bytes());
        bytes.extend_from_slice(&self.height.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&24_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&image_bytes.to_le_bytes());
        bytes.extend_from_slice(&2_835_u32.to_le_bytes());
        bytes.extend_from_slice(&2_835_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());

        for y in (0..self.height).rev() {
            for x in 0..self.width {
                let [red, green, blue] = self.pixel(x, y);
                bytes.extend_from_slice(&[to_byte(blue), to_byte(green), to_byte(red)]);
            }
            bytes.extend(std::iter::repeat_n(0, row_padding as usize));
        }
        bytes
    }
}

fn ratio(part: usize, total: usize) -> f64 {
    if total == 0 {
        return 0.0;
    }
    let part = u32::try_from(part).map_or(f64::from(u32::MAX), f64::from);
    let total = u32::try_from(total).map_or(f64::from(u32::MAX), f64::from);
    part / total
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn to_byte(channel: f32) -> u8 {
    ((channel.clamp(0.0, 1.0) * 255.0) + 0.5) as u8
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Unsupported {
    TextProgram,
    TextGlyph,
    TextSubstituteGlyph,
    TextProgramUnreadable,
    TextUnresolvedCode,
    TextClusterUnplaceable,
    Pattern,
    Shading,
    Color,
    GroupDepth,
    Image,
}

impl fmt::Display for Unsupported {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::TextProgram => "text in a font that embeds no program",
            Self::TextGlyph => "text with glyphs that resolved to no outline",
            Self::TextSubstituteGlyph => "text whose substituted face has no glyph for it",
            Self::TextProgramUnreadable => {
                "text whose embedded font program this build could not read"
            }
            Self::TextUnresolvedCode => "text whose codes the file gives no meaning for",
            Self::TextClusterUnplaceable => {
                "text whose meaning is a cluster this build cannot place"
            }
            Self::Pattern => "tiling pattern paint",
            Self::Shading => "shading that could not be evaluated",
            Self::Color => "colour that could not be resolved",
            Self::GroupDepth => "transparency group nesting limit",
            Self::Image => "image that could not be placed or coloured",
        })
    }
}

fn undrawn_reason(
    glyph: &pdf_paint::PositionedGlyph,
    program: Option<&pdf_content::ProgramEvidence>,
) -> Unsupported {
    use pdf_paint::UnresolvedReason;

    let substituted = program.is_some();
    let unreadable_program = matches!(
        program,
        Some(pdf_content::ProgramEvidence::Unreadable { .. })
    );
    match glyph.unresolved {
        Some(
            UnresolvedReason::NoEvidence
            | UnresolvedReason::AmbiguousEncoding
            | UnresolvedReason::TruncatedMultibyte
            | UnresolvedReason::UnmappedMultibyte,
        ) if unreadable_program => Unsupported::TextProgramUnreadable,
        Some(
            UnresolvedReason::NoEvidence
            | UnresolvedReason::AmbiguousEncoding
            | UnresolvedReason::TruncatedMultibyte
            | UnresolvedReason::UnmappedMultibyte,
        ) => Unsupported::TextUnresolvedCode,
        Some(UnresolvedReason::ClusterTooLong | UnresolvedReason::MultipleBaseCharacters) => {
            Unsupported::TextClusterUnplaceable
        }
        Some(UnresolvedReason::NoFaceCoverage) => Unsupported::TextSubstituteGlyph,
        Some(UnresolvedReason::MultibyteContinuation) | None => {
            if substituted {
                Unsupported::TextSubstituteGlyph
            } else {
                Unsupported::TextGlyph
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StencilPattern {
    Drew,
    Nothing,
    Refused,
}

struct Tile {
    width: u32,
    height: u32,
    colour: Vec<[f32; 3]>,
    alpha: Vec<f32>,
}

fn tile_extent(pixels: f64) -> u32 {
    const MAX: u32 = 512;
    if !pixels.is_finite() || pixels <= 1.0 {
        return 1;
    }
    if pixels >= f64::from(MAX) {
        return MAX;
    }
    let mut extent = 1_u32;
    while f64::from(extent) < pixels && extent < MAX {
        extent += 1;
    }
    extent
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Approximation {
    UnmanagedCmyk,
    CalibratedAsDevice,
    IccAlternate,
    TintTransform,
    StrokeJoins,
    TransparencyGroup,
    InvalidDash,
    NearestNeighbour,
    IncompleteCharacterCode,
    SubstitutedFont,
    SubstitutedCluster,
}

impl fmt::Display for Approximation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnmanagedCmyk => "DeviceCMYK converted from ink primaries, not a profile",
            Self::CalibratedAsDevice => "calibrated colour drawn as device colour",
            Self::IccAlternate => "ICC profile replaced by its declared alternate",
            Self::TintTransform => "separation /All drawn as one alternate colour",
            Self::StrokeJoins => "stroke joins drawn round, miter limit ignored",
            Self::TransparencyGroup => "transparency group composited by alpha only",
            Self::InvalidDash => "invalid dash array drawn solid",
            Self::NearestNeighbour => "interpolated image magnified without interpolation",
            Self::IncompleteCharacterCode => {
                "a text string ended inside a character code, completed with zero bytes"
            }
            Self::SubstitutedFont => "text drawn from a substituted face, not the embedded font",
            Self::SubstitutedCluster => {
                "a substituted base and its combining marks drawn at one origin, unshaped"
            }
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SkippedAtom {
    pub id: PaintId,
    pub reason: Unsupported,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderReport {
    pub visited: usize,
    pub drawn: usize,
    pub culled: usize,
    pub skipped: Vec<SkippedAtom>,
    pub approximations: BTreeSet<Approximation>,
}

impl RenderReport {
    #[must_use]
    pub fn is_faithful(&self) -> bool {
        self.skipped.is_empty() && self.approximations.is_empty()
    }

    #[must_use]
    pub fn skipped_counts(&self) -> Vec<(Unsupported, usize)> {
        let mut kinds: Vec<Unsupported> = self
            .skipped
            .iter()
            .map(|atom| atom.reason)
            .collect::<Vec<_>>();
        kinds.sort_unstable();
        let mut counts: Vec<(Unsupported, usize)> = Vec::new();
        for kind in kinds {
            match counts.last_mut() {
                Some((last, count)) if *last == kind => *count += 1,
                _ => counts.push((kind, 1)),
            }
        }
        counts
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderError {
    InvalidScale,
    PageTooLarge,
    PathTooComplex,
    InvalidRegion,
}

impl fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidScale => "render scale must be finite and positive",
            Self::PageTooLarge => "page exceeds the configured pixel limit",
            Self::PathTooComplex => "path exceeds the configured edge limit",
            Self::InvalidRegion => "render region is empty or outside the page",
        })
    }
}

impl std::error::Error for RenderError {}

pub fn render_page(
    graph: &PaintGraph,
    geometry: &PageGeometry,
    options: RenderOptions,
) -> Result<(Canvas, RenderReport), RenderError> {
    render_page_layers(&[graph], geometry, options)
}

pub fn render_page_layers(
    layers: &[&PaintGraph],
    geometry: &PageGeometry,
    options: RenderOptions,
) -> Result<(Canvas, RenderReport), RenderError> {
    let device = DeviceTransform::for_page(geometry, options.scale, options.limits)?;
    let canvas = Canvas::blank(device.width, device.height);
    draw_onto(layers, device, options.limits, canvas)
}

pub fn render_region(
    graph: &PaintGraph,
    geometry: &PageGeometry,
    options: RenderOptions,
    region: [u32; 4],
) -> Result<(Canvas, RenderReport), RenderError> {
    render_region_layers(&[graph], geometry, options, region)
}

pub fn render_region_layers(
    layers: &[&PaintGraph],
    geometry: &PageGeometry,
    options: RenderOptions,
    region: [u32; 4],
) -> Result<(Canvas, RenderReport), RenderError> {
    let device = DeviceTransform::for_page(geometry, options.scale, options.limits)?;
    let [x0, y0, x1, y1] = region;
    if x1 <= x0 || y1 <= y0 || x1 > device.width || y1 > device.height {
        return Err(RenderError::InvalidRegion);
    }
    let canvas = Canvas::window(x0, y0, x1 - x0, y1 - y0);
    draw_onto(layers, device, options.limits, canvas)
}

pub fn render_region_replacing(
    (page, hidden, standing_in): (&PaintGraph, &std::collections::BTreeSet<usize>, &PaintGraph),
    above: &[&PaintGraph],
    geometry: &PageGeometry,
    options: RenderOptions,
    region: [u32; 4],
) -> Result<(Canvas, RenderReport), RenderError> {
    let device = DeviceTransform::for_page(geometry, options.scale, options.limits)?;
    let [x0, y0, x1, y1] = region;
    if x1 <= x0 || y1 <= y0 || x1 > device.width || y1 > device.height {
        return Err(RenderError::InvalidRegion);
    }
    let mut canvas = Canvas::window(x0, y0, x1 - x0, y1 - y0);
    let mut report = RenderReport::default();
    let mut renderer = Renderer {
        device,
        limits: options.limits,
        report: &mut report,
        outlines: std::collections::HashMap::new(),
        clip_cache: None,
        soft_mask_cache: None,
    };
    let first = hidden.first().copied();
    for (ordinal, atom) in page.atoms.iter().enumerate() {
        if Some(ordinal) == first {
            renderer.draw_graph(&mut canvas, standing_in, 1.0, 0)?;
        }
        if !hidden.contains(&ordinal) {
            renderer.draw_atom(&mut canvas, atom, 1.0, 0)?;
        }
    }
    if first.is_none() {
        renderer.draw_graph(&mut canvas, standing_in, 1.0, 0)?;
    }
    for graph in above {
        renderer.draw_graph(&mut canvas, graph, 1.0, 0)?;
    }
    Ok((canvas, report))
}

fn draw_onto(
    layers: &[&PaintGraph],
    device: DeviceTransform,
    limits: RenderLimits,
    mut canvas: Canvas,
) -> Result<(Canvas, RenderReport), RenderError> {
    let mut report = RenderReport::default();
    let mut renderer = Renderer {
        device,
        limits,
        report: &mut report,
        outlines: std::collections::HashMap::new(),
        clip_cache: None,
        soft_mask_cache: None,
    };
    for graph in layers {
        renderer.draw_graph(&mut canvas, graph, 1.0, 0)?;
    }
    Ok((canvas, report))
}

struct Outlines {
    paths: Vec<(pdf_paint::GlyphPath, u16)>,
    boxes: Vec<Option<[f64; 4]>>,
}

impl Outlines {
    fn of(paths: Vec<(pdf_paint::GlyphPath, u16)>) -> Self {
        let boxes = paths
            .iter()
            .map(|(outline, _)| cull::outline_box(outline, Matrix::IDENTITY))
            .collect();
        Self { paths, boxes }
    }

    fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

#[derive(Clone, Copy)]
struct Ink<'a> {
    clip: Option<&'a Mask>,
    rgb: Option<[f64; 3]>,
    stroke_rgb: Option<[f64; 3]>,
    alpha: f64,
}

struct Renderer<'a> {
    device: DeviceTransform,
    limits: RenderLimits,
    report: &'a mut RenderReport,
    outlines: std::collections::HashMap<(usize, u16), Arc<Outlines>>,
    clip_cache: Option<(Vec<pdf_bytes::SourceSpan>, std::sync::Arc<Mask>)>,
    soft_mask_cache: Option<(pdf_bytes::SourceSpan, std::sync::Arc<Mask>)>,
}

impl Renderer<'_> {
    fn draw_graph(
        &mut self,
        canvas: &mut Canvas,
        graph: &PaintGraph,
        alpha: f64,
        depth: usize,
    ) -> Result<(), RenderError> {
        for atom in &graph.atoms {
            self.draw_atom(canvas, atom, alpha, depth)?;
        }
        Ok(())
    }

    fn draw_atom(
        &mut self,
        canvas: &mut Canvas,
        atom: &PaintAtom,
        alpha: f64,
        depth: usize,
    ) -> Result<(), RenderError> {
        self.report.visited += 1;
        if let Some(box_of) = cull::atom_box(atom, self.device.matrix)
            && !cull::meets(box_of, canvas.bounds())
        {
            self.report.culled += 1;
            return Ok(());
        }
        match &atom.kind {
            PaintAtomKind::Text(text) => {
                if let Some(reason) = unsupported_state(&text.state) {
                    self.skip(atom, reason);
                    return Ok(());
                }
                self.draw_text(canvas, atom, text, alpha, depth)
            }
            PaintAtomKind::Path(paint) => {
                if let Some(reason) = unsupported_state(&paint.state) {
                    self.skip(atom, reason);
                    return Ok(());
                }
                self.draw_path(canvas, atom, &paint.path, paint, alpha, depth)
            }
            PaintAtomKind::Shading(paint) => {
                if let Some(reason) = unsupported_state(&paint.state) {
                    self.skip(atom, reason);
                    return Ok(());
                }
                self.draw_shading(canvas, atom, paint, alpha)
            }
            PaintAtomKind::TransparencyGroup(group) => {
                self.draw_group(canvas, atom, group, alpha, depth)
            }
            PaintAtomKind::Image(image) => {
                if let Some(reason) = unsupported_state(&image.state) {
                    self.skip(atom, reason);
                    return Ok(());
                }
                self.draw_image(canvas, atom, image, alpha, depth)
            }
        }
    }

    fn draw_image(
        &mut self,
        canvas: &mut Canvas,
        atom: &PaintAtom,
        image: &pdf_paint::ImagePaint,
        alpha: f64,
        depth: usize,
    ) -> Result<(), RenderError> {
        let matrix = self.device.matrix.multiply(image.state.ctm.value);
        let Some(inverse) = invert(matrix) else {
            self.skip(atom, Unsupported::Image);
            return Ok(());
        };
        if image.image_mask.value
            && let Color::TilingPattern(pattern) = &image.state.fill_color.value
        {
            let clip = self.clip_mask(&image.state, canvas.bounds())?;
            match self.composite_stencil_pattern(
                canvas,
                image,
                inverse,
                pattern,
                clip.as_deref(),
                alpha,
                depth,
            )? {
                StencilPattern::Drew => self.report.drawn += 1,
                StencilPattern::Nothing => {}
                StencilPattern::Refused => self.skip(atom, Unsupported::Pattern),
            }
            return Ok(());
        }
        let stencil = if image.image_mask.value {
            let Some(rgb) = self.resolve(
                atom,
                &image.state.fill_color_space.value,
                &image.state.fill_color.value,
            ) else {
                return Ok(());
            };
            Some(rgb)
        } else {
            None
        };
        let rates = sample_rates(inverse, image.width.value, image.height.value);
        let grid = (taps_for(rates.0), taps_for(rates.1));
        if image.interpolate.value && rates.0 < 1.0 && rates.1 < 1.0 {
            self.report
                .approximations
                .insert(Approximation::NearestNeighbour);
        }
        let clip = self.clip_mask(&image.state, canvas.bounds())?;
        let Some(space) = image.color_space.as_ref().map(|space| &space.value) else {
            if stencil.is_none() {
                self.skip(atom, Unsupported::Image);
                return Ok(());
            }
            self.paint_samples(
                canvas,
                image,
                inverse,
                grid,
                clip.as_deref(),
                stencil,
                None,
                alpha,
            );
            self.report.drawn += 1;
            return Ok(());
        };
        self.paint_samples(
            canvas,
            image,
            inverse,
            grid,
            clip.as_deref(),
            stencil,
            Some(space),
            alpha,
        );
        self.report.drawn += 1;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_samples(
        &mut self,
        canvas: &mut Canvas,
        image: &pdf_paint::ImagePaint,
        inverse: Matrix,
        grid: (u32, u32),
        clip: Option<&Mask>,
        stencil: Option<[f64; 3]>,
        space: Option<&ColorSpace>,
        alpha: f64,
    ) {
        let blend = Blend::of(&image.state.blend_mode.value);
        let mut sampler = Sampler::new(image, inverse, grid, stencil, space);
        let Some(window) = Self::image_window(canvas, inverse, clip) else {
            return;
        };
        let (x0, y0, x1, y1) = window;
        for y in y0..y1 {
            for x in x0..x1 {
                let clip_coverage = clip.map_or(1.0, |mask| mask.at(x, y));
                if clip_coverage <= 0.0 {
                    continue;
                }
                let Some((rgb, covered)) = sampler.pixel(x, y) else {
                    continue;
                };
                #[allow(clippy::cast_possible_truncation)]
                let coverage = clip_coverage
                    * ((image.state.fill_alpha.value * alpha * covered).clamp(0.0, 1.0) as f32);
                blend_pixel(canvas, x, y, rgb, coverage, blend);
            }
        }
        for kind in sampler.approximation.approximations() {
            self.report.approximations.insert(kind);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn composite_stencil_pattern(
        &mut self,
        canvas: &mut Canvas,
        image: &pdf_paint::ImagePaint,
        inverse: Matrix,
        pattern: &pdf_paint::TilingPatternPaint,
        clip: Option<&Mask>,
        alpha: f64,
        depth: usize,
    ) -> Result<StencilPattern, RenderError> {
        let rates = sample_rates(inverse, image.width.value, image.height.value);
        let grid = (taps_for(rates.0), taps_for(rates.1));
        let Some((x0, y0, x1, y1)) = Self::image_window(canvas, inverse, clip) else {
            return Ok(StencilPattern::Nothing);
        };
        let (width, height) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
        if width == 0 || height == 0 {
            return Ok(StencilPattern::Nothing);
        }
        let mut sampler = Sampler::new(image, inverse, grid, Some([0.0; 3]), None);
        let mut coverage = vec![0.0_f32; width as usize * height as usize];
        let mut any = false;
        for y in y0..y1 {
            for x in x0..x1 {
                let Some((_, covered)) = sampler.pixel(x, y) else {
                    continue;
                };
                if covered <= 0.0 {
                    continue;
                }
                let index = (y - y0) as usize * width as usize + (x - x0) as usize;
                #[allow(clippy::cast_possible_truncation)]
                let value = covered.clamp(0.0, 1.0) as f32;
                coverage[index] = value;
                any = true;
            }
        }
        for kind in sampler.approximation.approximations() {
            self.report.approximations.insert(kind);
        }
        if !any {
            return Ok(StencilPattern::Nothing);
        }
        let mask = Mask::from_coverage(x0, y0, width, height, coverage);
        let drew = self.composite_tiling(
            canvas,
            &mask,
            clip,
            pattern,
            image.state.fill_alpha.value * alpha,
            Blend::of(&image.state.blend_mode.value),
            depth,
        )?;
        Ok(if drew {
            StencilPattern::Drew
        } else {
            StencilPattern::Refused
        })
    }

    fn image_window(
        canvas: &Canvas,
        inverse: Matrix,
        clip: Option<&Mask>,
    ) -> Option<(u32, u32, u32, u32)> {
        let window = match clip {
            Some(mask) => intersect_boxes(canvas.bounds(), mask.bounds())?,
            None => canvas.bounds(),
        };
        let forward = invert(inverse)?;
        device_box(forward, window)
    }

    fn draw_text(
        &mut self,
        canvas: &mut Canvas,
        atom: &PaintAtom,
        text: &pdf_paint::TextShowPaint,
        alpha: f64,
        depth: usize,
    ) -> Result<(), RenderError> {
        if text
            .glyphs
            .iter()
            .any(|glyph| glyph.code.completed_bytes > 0)
        {
            self.report
                .approximations
                .insert(Approximation::IncompleteCharacterCode);
        }
        if matches!(
            text.state.text.rendering_mode.value,
            pdf_paint::TextRenderingMode::Invisible | pdf_paint::TextRenderingMode::Clip
        ) {
            self.report.drawn += 1;
            return Ok(());
        }
        if text.type3 {
            return self.draw_type3_text(canvas, text, alpha, depth);
        }
        let substituted = text.substitution.is_some();
        if text.program.is_none() && !substituted {
            self.skip(atom, Unsupported::TextProgram);
            return Ok(());
        }
        let (rgb, stroke_rgb) = self.text_colours(atom, text);
        if rgb.is_none() && stroke_rgb.is_none() {
            return Ok(());
        }
        if substituted {
            self.report
                .approximations
                .insert(Approximation::SubstitutedFont);
            if text.has_unshaped_cluster() {
                self.report
                    .approximations
                    .insert(Approximation::SubstitutedCluster);
            }
        }
        let clip = self.clip_mask(&text.state, canvas.bounds())?;
        let base = self.device.matrix.multiply(text.state.ctm.value);
        let pen_scale = matrix_scale(base);
        let (drew, mut unresolved) = self.paint_the_run(
            canvas,
            text,
            (base, pen_scale),
            Ink {
                clip: clip.as_deref(),
                rgb,
                stroke_rgb,
                alpha,
            },
        );
        unresolved.sort_unstable();
        for reason in unresolved {
            self.skip(atom, reason);
        }
        if drew {
            self.report.drawn += 1;
        }
        Ok(())
    }

    fn outlines_of(
        &mut self,
        text: &pdf_paint::TextShowPaint,
        glyph: &pdf_paint::PositionedGlyph,
    ) -> Arc<Outlines> {
        let key = match (text.program.as_ref(), glyph.glyph) {
            (Some(program), Some(index)) if glyph.substituted.is_empty() => {
                (Arc::as_ptr(program) as usize, index)
            }
            _ => return Arc::new(Outlines::of(text.outlines_of(glyph))),
        };
        if let Some(held) = self.outlines.get(&key) {
            return Arc::clone(held);
        }
        let outlines = Arc::new(Outlines::of(text.outlines_of(glyph)));
        self.outlines.insert(key, Arc::clone(&outlines));
        outlines
    }

    fn paint_the_run(
        &mut self,
        canvas: &mut Canvas,
        text: &pdf_paint::TextShowPaint,
        (base, pen_scale): (Matrix, f64),
        ink: Ink<'_>,
    ) -> (bool, Vec<Unsupported>) {
        let Ink {
            clip,
            rgb,
            stroke_rgb,
            alpha,
        } = ink;
        let mut unresolved: Vec<Unsupported> = Vec::new();
        let mut drew = false;
        for glyph in &text.glyphs {
            let outlines = self.outlines_of(text, glyph);
            if outlines.is_empty() {
                if !glyph.silent {
                    let reason = undrawn_reason(
                        glyph,
                        text.substitution
                            .as_deref()
                            .map(|substitution| &substitution.request.program),
                    );
                    if !unresolved.contains(&reason) {
                        unresolved.push(reason);
                    }
                }
                continue;
            }
            for ((outline, units_per_em), own_box) in outlines.paths.iter().zip(&outlines.boxes) {
                let units_per_em = *units_per_em;

                if outline.is_empty() {
                    continue;
                }
                let scale = 1.0 / f64::from(units_per_em);
                let to_user = Matrix {
                    a: scale,
                    b: 0.0,
                    c: 0.0,
                    d: scale,
                    e: 0.0,
                    f: 0.0,
                };
                let matrix = base.multiply(glyph.matrix).multiply(to_user);
                if let Some(own_box) = own_box
                    && !cull::meets(cull::placed_box(*own_box, matrix), canvas.bounds())
                {
                    continue;
                }
                let polygon = glyph_polygon(outline, matrix);
                if polygon.is_empty() {
                    continue;
                }
                paint_glyph(
                    canvas,
                    &polygon,
                    clip,
                    text,
                    (rgb, stroke_rgb),
                    pen_scale,
                    alpha,
                );
                drew = true;
            }
        }
        (drew, unresolved)
    }

    fn text_colours(
        &mut self,
        atom: &PaintAtom,
        text: &pdf_paint::TextShowPaint,
    ) -> (Option<[f64; 3]>, Option<[f64; 3]>) {
        let mode = text.state.text.rendering_mode.value;
        let fills = matches!(
            mode,
            pdf_paint::TextRenderingMode::Fill
                | pdf_paint::TextRenderingMode::FillStroke
                | pdf_paint::TextRenderingMode::FillClip
                | pdf_paint::TextRenderingMode::FillStrokeClip
        );
        let strokes = matches!(
            mode,
            pdf_paint::TextRenderingMode::Stroke
                | pdf_paint::TextRenderingMode::FillStroke
                | pdf_paint::TextRenderingMode::StrokeClip
                | pdf_paint::TextRenderingMode::FillStrokeClip
        );
        let rgb = if fills {
            self.resolve(
                atom,
                &text.state.fill_color_space.value,
                &text.state.fill_color.value,
            )
        } else {
            None
        };
        let stroke_rgb = if strokes {
            self.resolve(
                atom,
                &text.state.stroke_color_space.value,
                &text.state.stroke_color.value,
            )
        } else {
            None
        };
        (rgb, stroke_rgb)
    }

    fn draw_type3_text(
        &mut self,
        canvas: &mut Canvas,
        text: &pdf_paint::TextShowPaint,
        alpha: f64,
        depth: usize,
    ) -> Result<(), RenderError> {
        let mut drew = false;
        for glyph in &text.glyphs {
            let Some(procedure) = glyph.procedure.as_ref() else {
                continue;
            };
            for nested in &procedure.atoms {
                self.draw_atom(canvas, nested, alpha, depth + 1)?;
            }
            drew = true;
        }
        if drew {
            self.report.drawn += 1;
        }
        Ok(())
    }

    fn draw_path(
        &mut self,
        canvas: &mut Canvas,
        atom: &PaintAtom,
        path: &Path,
        paint: &pdf_paint::PathPaint,
        alpha: f64,
        depth: usize,
    ) -> Result<(), RenderError> {
        let matrix = self.device.matrix.multiply(paint.state.ctm.value);
        let (polygon, closed) = geometry::flatten_subpaths(path, matrix, self.limits)?;
        if polygon.is_empty() && !paint.stroke {
            return Ok(());
        }
        let clip = self.clip_mask(&paint.state, canvas.bounds())?;
        let mut drew = false;
        if let Some(rule) = paint.fill {
            let mask = raster::rasterise_fill_within(&polygon, rule, canvas.bounds());
            if let Color::TilingPattern(pattern) = &paint.state.fill_color.value {
                drew = self.composite_tiling(
                    canvas,
                    &mask,
                    clip.as_deref(),
                    pattern,
                    paint.state.fill_alpha.value * alpha,
                    Blend::of(&paint.state.blend_mode.value),
                    depth,
                )?;
                if !drew {
                    self.skip(atom, Unsupported::Pattern);
                }
            } else if let Color::ShadingPattern(pattern) = &paint.state.fill_color.value {
                drew = self.composite_shading_pattern(
                    canvas,
                    &mask,
                    clip.as_deref(),
                    pattern,
                    paint.state.fill_alpha.value * alpha,
                );
                if !drew {
                    self.skip(atom, Unsupported::Shading);
                }
            } else if let Some(rgb) = self.resolve(
                atom,
                &paint.state.fill_color_space.value,
                &paint.state.fill_color.value,
            ) {
                composite(
                    canvas,
                    &mask,
                    clip.as_deref(),
                    rgb,
                    paint.state.fill_alpha.value * alpha,
                    Blend::of(&paint.state.blend_mode.value),
                );
                drew = true;
            }
        }
        if paint.stroke {
            drew |= self.stroke_path(
                canvas,
                atom,
                (&polygon, &closed),
                paint,
                matrix,
                clip.as_deref(),
                alpha,
                depth,
            )?;
        }
        if drew {
            self.report.drawn += 1;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_lines)]
    fn stroke_path(
        &mut self,
        canvas: &mut Canvas,
        atom: &PaintAtom,
        (polygon, closed): (&geometry::Polygon, &[bool]),
        paint: &pdf_paint::PathPaint,
        matrix: Matrix,
        clip: Option<&Mask>,
        alpha: f64,
        depth: usize,
    ) -> Result<bool, RenderError> {
        let stroke_pattern = if let Color::ShadingPattern(pattern) = &paint.state.stroke_color.value
        {
            Some(pattern.clone())
        } else {
            None
        };
        let tiling = if let Color::TilingPattern(pattern) = &paint.state.stroke_color.value {
            Some(pattern.clone())
        } else {
            None
        };
        let rgb = if stroke_pattern.is_some() || tiling.is_some() {
            None
        } else {
            self.resolve(
                atom,
                &paint.state.stroke_color_space.value,
                &paint.state.stroke_color.value,
            )
        };
        if rgb.is_none() && stroke_pattern.is_none() && tiling.is_none() {
            return Ok(false);
        }
        let scale = matrix_scale(matrix);
        let outline = stroke::outline(
            polygon,
            paint.state.line_width.value * scale,
            stroke::StrokeStyle {
                cap: paint.state.line_cap.value,
                join: paint.state.line_join.value,
                miter_limit: paint.state.miter_limit.value,
            },
            &paint.state.dash.value,
            scale,
            closed,
        );
        if !dash_is_usable(&paint.state.dash.value) {
            self.report
                .approximations
                .insert(Approximation::InvalidDash);
        }
        let mask = raster::rasterise_within(&outline, FillRule::Nonzero, canvas.bounds());
        if let Some(pattern) = stroke_pattern {
            if self.composite_shading_pattern(
                canvas,
                &mask,
                clip,
                &pattern,
                paint.state.stroke_alpha.value * alpha,
            ) {
                return Ok(true);
            }
            self.skip(atom, Unsupported::Shading);
            return Ok(false);
        }
        if let Some(pattern) = tiling {
            let drew = self.composite_tiling(
                canvas,
                &mask,
                clip,
                &pattern,
                paint.state.stroke_alpha.value * alpha,
                Blend::of(&paint.state.blend_mode.value),
                depth,
            )?;
            if !drew {
                self.skip(atom, Unsupported::Pattern);
            }
            return Ok(drew);
        }
        let Some(rgb) = rgb else {
            return Ok(false);
        };
        composite(
            canvas,
            &mask,
            clip,
            rgb,
            paint.state.stroke_alpha.value * alpha,
            Blend::of(&paint.state.blend_mode.value),
        );
        Ok(true)
    }

    fn draw_shading(
        &mut self,
        canvas: &mut Canvas,
        atom: &PaintAtom,
        paint: &ShadingPaint,
        alpha: f64,
    ) -> Result<(), RenderError> {
        let matrix = self.device.matrix.multiply(paint.state.ctm.value);
        let Some(inverse) = invert(matrix) else {
            self.skip(atom, Unsupported::Shading);
            return Ok(());
        };
        let clip = self.clip_mask(&paint.state, canvas.bounds())?;
        let bbox = paint.bbox.as_ref().map(|bbox| {
            let [x0, y0, x1, y1] = bbox.value;
            let corners =
                [[x0, y0], [x1, y0], [x1, y1], [x0, y1]].map(|corner| apply(matrix, corner));
            raster::rasterise_fill_within(
                &geometry::Polygon {
                    contours: vec![corners.to_vec()],
                },
                FillRule::Nonzero,
                canvas.bounds(),
            )
        });
        let outcome = self.shade_region(
            canvas,
            paint,
            inverse,
            bbox.as_ref(),
            clip.as_deref(),
            paint.state.fill_alpha.value * alpha,
        );
        if outcome.painted {
            self.report.drawn += 1;
        }
        if outcome.unevaluated {
            self.skip(atom, Unsupported::Shading);
        }
        Ok(())
    }

    fn shade_region(
        &mut self,
        canvas: &mut Canvas,
        paint: &ShadingPaint,
        inverse: Matrix,
        mask: Option<&Mask>,
        clip: Option<&Mask>,
        alpha: f64,
    ) -> ShadeOutcome {
        let mut outcome = ShadeOutcome::default();
        let blend = Blend::of(&paint.state.blend_mode.value);
        let mut window = canvas.bounds();
        for narrower in [mask, clip].into_iter().flatten() {
            let Some(narrowed) = intersect_boxes(window, narrower.bounds()) else {
                return outcome;
            };
            window = narrowed;
        }
        let (wx0, wy0, wx1, wy1) = window;
        let own_space = match &paint.geometry {
            ShadingGeometry::Function { matrix, .. } => match invert(matrix.value) {
                Some(inverse) => Some(inverse),
                None => return outcome,
            },
            ShadingGeometry::Axial(_) | ShadingGeometry::Radial(_) => None,
        };
        let mut inputs = [0.0; 2];
        for y in wy0..wy1 {
            for x in wx0..wx1 {
                let mut coverage = mask.map_or(1.0, |mask| mask.at(x, y));
                if coverage <= 0.0 {
                    continue;
                }
                if let Some(clip) = clip {
                    coverage *= clip.at(x, y);
                    if coverage <= 0.0 {
                        continue;
                    }
                }
                let user = apply(inverse, [f64::from(x) + 0.5, f64::from(y) + 0.5]);
                let [low, high] = paint.domain.value;
                let taken = match &paint.geometry {
                    ShadingGeometry::Function { domain, .. } => {
                        let [px, py] = own_space.map_or(user, |back| apply(back, user));
                        let [x0, x1, y0, y1] = domain.value;
                        if !(x0..=x1).contains(&px) || !(y0..=y1).contains(&py) {
                            continue;
                        }
                        inputs = [px, py];
                        2
                    }
                    ShadingGeometry::Axial(coords) => {
                        let Some(parameter) =
                            shading::axial_parameter(coords.value, user, paint.extend.value)
                        else {
                            continue;
                        };
                        inputs[0] = (high - low).madd(parameter, low);
                        1
                    }
                    ShadingGeometry::Radial(coords) => {
                        let Some(parameter) =
                            shading::radial_parameter(coords.value, user, paint.extend.value)
                        else {
                            continue;
                        };
                        inputs[0] = (high - low).madd(parameter, low);
                        1
                    }
                };
                let Some(components) = paint.function.evaluate(&inputs[..taken]) else {
                    outcome.unevaluated = true;
                    continue;
                };
                let Some((rgb, resolved)) =
                    color::components_to_rgb(&paint.color_space.value, &components)
                else {
                    outcome.unevaluated = true;
                    continue;
                };
                self.report.approximations.extend(resolved.approximations());
                #[allow(clippy::cast_possible_truncation)]
                let coverage = coverage * alpha as f32;
                blend_pixel(canvas, x, y, rgb, coverage, blend);
                outcome.painted = true;
            }
        }
        outcome
    }

    fn composite_shading_pattern(
        &mut self,
        canvas: &mut Canvas,
        mask: &Mask,
        clip: Option<&Mask>,
        pattern: &pdf_paint::ShadingPatternPaint,
        alpha: f64,
    ) -> bool {
        let to_device = self
            .device
            .matrix
            .multiply(pattern.base)
            .multiply(pattern.matrix.value);
        let Some(inverse) = invert(to_device) else {
            return false;
        };
        let outcome = self.shade_region(canvas, &pattern.shading, inverse, Some(mask), clip, alpha);
        outcome.painted || !outcome.unevaluated
    }

    fn draw_group(
        &mut self,
        canvas: &mut Canvas,
        atom: &PaintAtom,
        group: &TransparencyGroupPaint,
        alpha: f64,
        depth: usize,
    ) -> Result<(), RenderError> {
        if depth >= self.limits.max_group_depth {
            self.skip(atom, Unsupported::GroupDepth);
            return Ok(());
        }
        if let Some(reason) = unsupported_state(&group.state) {
            self.skip(atom, reason);
            return Ok(());
        }
        self.report
            .approximations
            .insert(Approximation::TransparencyGroup);
        let combined = group.state.fill_alpha.value * alpha;
        let mask = if matches!(group.state.soft_mask.value, SoftMask::Dictionary(_)) {
            self.clip_mask(&group.state, canvas.bounds())?
        } else {
            None
        };
        let Some(mask) = mask else {
            let saved = self.clip_cache.take();
            self.draw_graph(canvas, &group.graph, combined, depth + 1)?;
            self.clip_cache = saved;
            self.report.drawn += 1;
            return Ok(());
        };
        let mut offscreen = canvas.clone();
        let saved_clip = self.clip_cache.take();
        let saved_soft = self.soft_mask_cache.take();
        self.draw_graph(&mut offscreen, &group.graph, combined, depth + 1)?;
        self.clip_cache = saved_clip;
        self.soft_mask_cache = saved_soft;
        let (x0, y0, x1, y1) = shared_box(mask.bounds(), canvas.bounds());
        for y in y0..y1 {
            for x in x0..x1 {
                let coverage = mask.at(x, y);
                if coverage <= 0.0 {
                    continue;
                }
                let (Some(from), Some(into)) = (offscreen.index(x, y), canvas.index(x, y)) else {
                    continue;
                };
                let source = offscreen.pixels[from];
                let pixel = &mut canvas.pixels[into];
                for (channel, value) in pixel.iter_mut().zip(source) {
                    *channel = coverage.madd(value - *channel, *channel);
                }
            }
        }
        self.report.drawn += 1;
        Ok(())
    }

    fn render_pattern_tile(
        &mut self,
        pattern: &pdf_paint::TilingPatternPaint,
        depth: usize,
    ) -> Result<Option<Tile>, RenderError> {
        let [x_step, y_step] = [pattern.x_step.value.abs(), pattern.y_step.value.abs()];
        if !x_step.is_finite() || !y_step.is_finite() || x_step <= 0.0 || y_step <= 0.0 {
            return Ok(None);
        }
        let to_device = self.device.matrix.multiply(pattern.matrix.value);
        let width = tile_extent(x_step * matrix_scale(to_device));
        let height = tile_extent(y_step * matrix_scale(to_device));
        let to_tile = Matrix {
            a: f64::from(width) / x_step,
            b: 0.0,
            c: 0.0,
            d: -f64::from(height) / y_step,
            e: -pattern.bbox[0] * f64::from(width) / x_step,
            f: f64::from(height) + pattern.bbox[1] * f64::from(height) / y_step,
        };
        let mut over_white = Canvas::blank(width, height);
        let mut over_black = Canvas::blank(width, height);
        for pixel in &mut over_black.pixels {
            *pixel = [0.0, 0.0, 0.0];
        }
        for canvas in [&mut over_white, &mut over_black] {
            let mut renderer = Renderer {
                outlines: std::collections::HashMap::new(),
                device: DeviceTransform {
                    matrix: to_tile,
                    width,
                    height,
                    scale: self.device.scale,
                },
                limits: self.limits,
                report: self.report,
                clip_cache: None,
                soft_mask_cache: None,
            };
            renderer.draw_graph(canvas, &pattern.graph, 1.0, depth + 1)?;
        }
        let mut tile = Tile {
            width,
            height,
            colour: vec![[0.0; 3]; width as usize * height as usize],
            alpha: vec![0.0; width as usize * height as usize],
        };
        for (index, (white, black)) in over_white.pixels.iter().zip(&over_black.pixels).enumerate()
        {
            let alpha = 1.0
                - white
                    .iter()
                    .zip(black.iter())
                    .map(|(white, black)| white - black)
                    .fold(0.0_f32, f32::max)
                    .clamp(0.0, 1.0);
            tile.alpha[index] = alpha;
            if alpha > 0.0 {
                for (slot, value) in tile.colour[index].iter_mut().zip(black.iter()) {
                    *slot = (value / alpha).clamp(0.0, 1.0);
                }
            }
        }
        Ok(Some(tile))
    }

    #[allow(clippy::too_many_arguments)]
    fn composite_tiling(
        &mut self,
        canvas: &mut Canvas,
        mask: &Mask,
        clip: Option<&Mask>,
        pattern: &pdf_paint::TilingPatternPaint,
        alpha: f64,
        blend: Blend,
        depth: usize,
    ) -> Result<bool, RenderError> {
        let Some(tile) = self.render_pattern_tile(pattern, depth)? else {
            return Ok(false);
        };

        let to_device = self.device.matrix.multiply(pattern.matrix.value);
        let Some(inverse) = invert(to_device) else {
            return Ok(false);
        };
        let [x_step, y_step] = [pattern.x_step.value.abs(), pattern.y_step.value.abs()];
        #[allow(clippy::cast_possible_truncation)]
        let alpha = alpha.clamp(0.0, 1.0) as f32;
        let Some(mut window) = intersect_boxes(canvas.bounds(), mask.bounds()) else {
            return Ok(true);
        };
        if let Some(clip) = clip {
            match intersect_boxes(window, clip.bounds()) {
                Some(narrowed) => window = narrowed,
                None => return Ok(true),
            }
        }
        let (wx0, wy0, wx1, wy1) = window;
        for y in wy0..wy1 {
            for x in wx0..wx1 {
                let mut coverage = mask.at(x, y);
                if coverage <= 0.0 {
                    continue;
                }
                if let Some(clip) = clip {
                    coverage *= clip.at(x, y);
                    if coverage <= 0.0 {
                        continue;
                    }
                }
                let point = apply(inverse, [f64::from(x) + 0.5, f64::from(y) + 0.5]);
                let u = (point[0] - pattern.bbox[0]).rem_euclid(x_step) / x_step;
                let v = (point[1] - pattern.bbox[1]).rem_euclid(y_step) / y_step;
                let (Some(column), Some(row)) = (
                    scaled_index(u, tile.width),
                    scaled_index(1.0 - v, tile.height),
                ) else {
                    continue;
                };
                let index = row as usize * tile.width as usize + column as usize;
                let cell_alpha = tile.alpha[index];
                if cell_alpha <= 0.0 {
                    continue;
                }
                let colour = tile.colour[index];
                blend_pixel(
                    canvas,
                    x,
                    y,
                    [
                        f64::from(colour[0]),
                        f64::from(colour[1]),
                        f64::from(colour[2]),
                    ],
                    coverage * alpha * cell_alpha,
                    blend,
                );
            }
        }
        Ok(true)
    }

    fn resolve(&mut self, atom: &PaintAtom, space: &ColorSpace, color: &Color) -> Option<[f64; 3]> {
        if matches!(color, Color::TilingPattern(_) | Color::PatternUnspecified) {
            self.skip(atom, Unsupported::Pattern);
            return None;
        }
        if matches!(color, Color::ShadingPattern(_)) {
            self.skip(atom, Unsupported::Pattern);
            return None;
        }
        if color::is_invisible(space) {
            return None;
        }
        let Some((rgb, resolved)) = color::color_to_rgb(space, color) else {
            self.skip(atom, Unsupported::Color);
            return None;
        };
        self.report.approximations.extend(resolved.approximations());
        Some(rgb)
    }

    fn clip_mask(
        &mut self,
        state: &GraphicsState,
        window: (u32, u32, u32, u32),
    ) -> Result<Option<std::sync::Arc<Mask>>, RenderError> {
        let clip = self.clip_paths_mask(state, window)?;
        let SoftMask::Dictionary(mask) = &state.soft_mask.value else {
            return Ok(clip);
        };
        let Some(soft) = self.soft_mask(mask, window)? else {
            return Ok(clip);
        };
        let Some(clip) = clip else {
            return Ok(Some(soft));
        };
        let mut combined = (*clip).clone();
        combined.intersect(&soft);
        Ok(Some(std::sync::Arc::new(combined)))
    }

    fn clip_paths_mask(
        &mut self,
        state: &GraphicsState,
        window: (u32, u32, u32, u32),
    ) -> Result<Option<std::sync::Arc<Mask>>, RenderError> {
        if state.clip_paths.is_empty() {
            return Ok(None);
        }
        let key: Vec<pdf_bytes::SourceSpan> = state
            .clip_paths
            .iter()
            .map(|clip| clip.provenance)
            .collect();

        if let Some((cached_key, mask)) = &self.clip_cache
            && *cached_key == key
        {
            return Ok(Some(std::sync::Arc::clone(mask)));
        }
        let mut mask: Option<Mask> = None;
        for clip in &state.clip_paths {
            let matrix = self.device.matrix.multiply(clip.ctm.value);
            let polygon = geometry::flatten(&clip.path, matrix, self.limits)?;
            let layer = raster::rasterise_clip_within(&polygon, clip.rule, window);
            match &mut mask {
                Some(mask) => mask.intersect(&layer),
                None => mask = Some(layer),
            }
        }
        let mask = mask.unwrap_or_else(|| Mask::opaque(self.device.width, self.device.height));
        let mask = std::sync::Arc::new(mask);
        self.clip_cache = Some((key, std::sync::Arc::clone(&mask)));
        Ok(Some(mask))
    }

    fn soft_mask(
        &mut self,
        mask: &pdf_paint::SoftMaskPaint,
        window: (u32, u32, u32, u32),
    ) -> Result<Option<std::sync::Arc<Mask>>, RenderError> {
        if let Some((cached, coverage)) = &self.soft_mask_cache
            && *cached == mask.dictionary_span
        {
            return Ok(Some(std::sync::Arc::clone(coverage)));
        }
        let nothing = || Ok(Some(std::sync::Arc::new(Mask::empty(0, 0))));
        let group = mask.group.as_ref();
        let matrix = self
            .device
            .matrix
            .multiply(group.state.ctm.value)
            .multiply(group.matrix.value);
        let to_bbox = Matrix {
            a: group.bbox[2] - group.bbox[0],
            b: 0.0,
            c: 0.0,
            d: group.bbox[3] - group.bbox[1],
            e: group.bbox[0],
            f: group.bbox[1],
        };
        let Some((x0, y0, x1, y1)) = device_box(matrix.multiply(to_bbox), window) else {
            return nothing();
        };
        let (width, height) = (x1 - x0, y1 - y0);
        if u64::from(width) * u64::from(height) > self.limits.max_soft_mask_pixels {
            return nothing();
        }
        let luminosity = matches!(mask.subtype.value, SoftMaskSubtype::Luminosity);
        let backdrop = if luminosity {
            mask.backdrop_color
                .as_ref()
                .and_then(|colour| {
                    let space = group.blend_space.as_ref()?;
                    color::components_to_rgb(&space.value, &colour.value)
                })
                .map_or([0.0, 0.0, 0.0], |(rgb, _)| rgb)
        } else {
            [1.0, 1.0, 1.0]
        };
        let mut over_first = Canvas::window(x0, y0, width, height);
        let mut over_second = Canvas::window(x0, y0, width, height);
        #[allow(clippy::cast_possible_truncation)]
        let first = [backdrop[0] as f32, backdrop[1] as f32, backdrop[2] as f32];
        for pixel in &mut over_first.pixels {
            *pixel = first;
        }
        for pixel in &mut over_second.pixels {
            *pixel = if luminosity { first } else { [0.0, 0.0, 0.0] };
        }
        let saved_clip = self.clip_cache.take();
        let saved_soft = self.soft_mask_cache.take();
        for canvas in [&mut over_first, &mut over_second] {
            let mut renderer = Renderer {
                outlines: std::collections::HashMap::new(),
                device: self.device,
                limits: self.limits,
                report: self.report,
                clip_cache: None,
                soft_mask_cache: None,
            };
            renderer.draw_graph(canvas, &group.graph, 1.0, self.limits.max_group_depth - 1)?;
        }
        self.clip_cache = saved_clip;
        self.soft_mask_cache = saved_soft;

        let mut coverage = Vec::with_capacity(width as usize * height as usize);
        for (over_white, over_black) in over_first.pixels.iter().zip(&over_second.pixels) {
            let value = if luminosity {
                0.212_67_f32.madd(
                    over_white[0],
                    0.715_16_f32.madd(over_white[1], 0.072_17 * over_white[2]),
                )
            } else {
                1.0 - over_white
                    .iter()
                    .zip(over_black.iter())
                    .map(|(white, black)| white - black)
                    .fold(0.0_f32, f32::max)
            };
            coverage.push(value.clamp(0.0, 1.0));
        }
        let coverage = std::sync::Arc::new(Mask::from_coverage(x0, y0, width, height, coverage));
        self.soft_mask_cache = Some((mask.dictionary_span, std::sync::Arc::clone(&coverage)));
        Ok(Some(coverage))
    }

    fn skip(&mut self, atom: &PaintAtom, reason: Unsupported) {
        self.report.skipped.push(SkippedAtom {
            id: atom.id.clone(),
            reason,
        });
    }
}

fn unsupported_state(_state: &GraphicsState) -> Option<Unsupported> {
    None
}

fn dash_is_usable(dash: &pdf_paint::DashPattern) -> bool {
    dash.array.is_empty()
        || (dash
            .array
            .iter()
            .all(|entry| entry.is_finite() && *entry >= 0.0)
            && dash.array.iter().any(|entry| *entry > 0.0))
}

fn matrix_scale(matrix: Matrix) -> f64 {
    let determinant = matrix.a.madd(matrix.d, -(matrix.b * matrix.c));
    determinant.abs().sqrt()
}

fn invert(matrix: Matrix) -> Option<Matrix> {
    let determinant = matrix.a.madd(matrix.d, -(matrix.b * matrix.c));
    if !determinant.is_finite() || determinant == 0.0 {
        return None;
    }
    Some(Matrix {
        a: matrix.d / determinant,
        b: -matrix.b / determinant,
        c: -matrix.c / determinant,
        d: matrix.a / determinant,
        e: matrix.c.madd(matrix.f, -(matrix.d * matrix.e)) / determinant,
        f: matrix.b.madd(matrix.e, -(matrix.a * matrix.f)) / determinant,
    })
}

fn glyph_polygon(path: &pdf_paint::GlyphPath, matrix: Matrix) -> Polygon {
    let mut polygon = Polygon::default();
    let mut contour: Vec<[f64; 2]> = Vec::new();
    let mut cursor = [0.0_f64; 2];
    for segment in &path.segments {
        match *segment {
            pdf_paint::GlyphSegment::MoveTo { x, y } => {
                if contour.len() >= 3 {
                    polygon.contours.push(std::mem::take(&mut contour));
                } else {
                    contour.clear();
                }
                cursor = apply(matrix, [x, y]);
                contour.push(cursor);
            }
            pdf_paint::GlyphSegment::LineTo { x, y } => {
                cursor = apply(matrix, [x, y]);
                contour.push(cursor);
            }
            pdf_paint::GlyphSegment::CurveTo {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                let control_1 = apply(matrix, [x1, y1]);
                let control_2 = apply(matrix, [x2, y2]);
                let end = apply(matrix, [x, y]);
                push_cubic(&mut contour, cursor, control_1, control_2, end);
                cursor = end;
            }
            pdf_paint::GlyphSegment::Close => {
                if contour.len() >= 3 {
                    polygon.contours.push(std::mem::take(&mut contour));
                } else {
                    contour.clear();
                }
            }
        }
    }
    if contour.len() >= 3 {
        polygon.contours.push(contour);
    }
    polygon
}

fn paint_glyph(
    canvas: &mut Canvas,
    polygon: &Polygon,
    clip: Option<&Mask>,
    text: &pdf_paint::TextShowPaint,
    (rgb, stroke_rgb): (Option<[f64; 3]>, Option<[f64; 3]>),
    pen_scale: f64,
    alpha: f64,
) {
    if let Some(rgb) = rgb {
        let mask = raster::rasterise_within(polygon, FillRule::Nonzero, canvas.bounds());
        composite(
            canvas,
            &mask,
            clip,
            rgb,
            text.state.fill_alpha.value * alpha,
            Blend::of(&text.state.blend_mode.value),
        );
    }
    if let Some(stroke_rgb) = stroke_rgb {
        let closed = vec![true; polygon.contours.len()];
        let outline = stroke::outline(
            polygon,
            text.state.line_width.value * pen_scale,
            stroke::StrokeStyle {
                cap: text.state.line_cap.value,
                join: text.state.line_join.value,
                miter_limit: text.state.miter_limit.value,
            },
            &text.state.dash.value,
            pen_scale,
            &closed,
        );
        let mask = raster::rasterise_within(&outline, FillRule::Nonzero, canvas.bounds());
        composite(
            canvas,
            &mask,
            clip,
            stroke_rgb,
            text.state.stroke_alpha.value * alpha,
            Blend::of(&text.state.blend_mode.value),
        );
    }
}

fn push_cubic(
    points: &mut Vec<[f64; 2]>,
    start: [f64; 2],
    control_1: [f64; 2],
    control_2: [f64; 2],
    end: [f64; 2],
) {
    const STEPS: u8 = 8;
    for step in 1..=STEPS {
        let along = f64::from(step) / f64::from(STEPS);
        let inverse = 1.0 - along;
        let basis = [
            inverse * inverse * inverse,
            3.0 * inverse * inverse * along,
            3.0 * inverse * along * along,
            along * along * along,
        ];
        points.push([
            basis[0].madd(start[0], basis[1] * control_1[0])
                + basis[2].madd(control_2[0], basis[3] * end[0]),
            basis[0].madd(start[1], basis[1] * control_1[1])
                + basis[2].madd(control_2[1], basis[3] * end[1]),
        ]);
    }
}

struct Sampler<'a> {
    image: &'a pdf_paint::ImagePaint,
    space: Option<&'a ColorSpace>,
    stencil: Option<[f64; 3]>,
    inverse: Matrix,
    grid: (u32, u32),
    pooled: bool,
    values: Vec<f64>,
    pool: Vec<f64>,
    sum: [f64; 3],
    weight: f64,
    converted: Option<((u32, u32), [f64; 3])>,
    approximation: color::Resolved,
}

impl<'a> Sampler<'a> {
    fn new(
        image: &'a pdf_paint::ImagePaint,
        inverse: Matrix,
        grid: (u32, u32),
        stencil: Option<[f64; 3]>,
        space: Option<&'a ColorSpace>,
    ) -> Self {
        let components = image.components();
        let pooled = stencil.is_none()
            && space.is_some_and(averages_in_sample_space)
            && image
                .soft_mask
                .as_ref()
                .is_none_or(|mask| mask.matte.is_none());
        Self {
            image,
            space,
            stencil,
            inverse,
            grid,
            pooled,
            values: vec![0.0; components],
            pool: vec![0.0; components],
            sum: [0.0; 3],
            weight: 0.0,
            converted: None,
            approximation: color::Resolved::EXACT,
        }
    }

    fn pixel(&mut self, x: u32, y: u32) -> Option<([f64; 3], f64)> {
        let (across, down) = self.grid;
        self.pool.fill(0.0);
        self.sum = [0.0; 3];
        self.weight = 0.0;
        for tap_down in 0..down {
            for tap_across in 0..across {
                let unit = apply(
                    self.inverse,
                    [
                        f64::from(x) + (f64::from(tap_across) + 0.5) / f64::from(across),
                        f64::from(y) + (f64::from(tap_down) + 0.5) / f64::from(down),
                    ],
                );
                self.add_tap(unit);
            }
        }
        if self.weight <= 0.0 {
            return None;
        }
        let rgb = if self.pooled {
            for slot in &mut self.pool {
                *slot /= self.weight;
            }
            let space = self.space?;
            let (rgb, resolved) = color::components_to_rgb(space, &self.pool)?;
            self.approximation = self.approximation.and(resolved);
            rgb
        } else {
            [
                self.sum[0] / self.weight,
                self.sum[1] / self.weight,
                self.sum[2] / self.weight,
            ]
        };
        Some((rgb, self.weight / f64::from(across * down)))
    }

    fn add_tap(&mut self, unit: [f64; 2]) {
        let Some((column, row)) = image_sample_position(unit, self.image) else {
            return;
        };
        if self.image.masked_out(column, row) {
            return;
        }
        let mask_alpha = self.image.soft_mask.as_ref().map_or(1.0, |mask| {
            image_sample_position(unit, mask)
                .and_then(|(column, row)| mask.sample(column, row, 0))
                .unwrap_or(1.0)
                .clamp(0.0, 1.0)
        });
        if mask_alpha <= 0.0 {
            return;
        }
        if self.pooled {
            if !self.image.sample_pixel(column, row, &mut self.values) {
                return;
            }
            for (slot, value) in self.pool.iter_mut().zip(self.values.iter()) {
                *slot += value * mask_alpha;
            }
            self.weight += mask_alpha;
            return;
        }
        let Some(rgb) = self.tap_colour(column, row) else {
            return;
        };
        let Some(rgb) = self.unmatted(rgb, mask_alpha) else {
            return;
        };
        for (slot, channel) in self.sum.iter_mut().zip(rgb) {
            *slot += channel * mask_alpha;
        }
        self.weight += mask_alpha;
    }

    fn tap_colour(&mut self, column: u32, row: u32) -> Option<[f64; 3]> {
        if let Some(rgb) = self.stencil {
            let sample = self.image.sample(column, row, 0)?;
            return (sample < 0.5).then_some(rgb);
        }
        if let Some((texel, rgb)) = self.converted
            && texel == (column, row)
        {
            return Some(rgb);
        }
        let space = self.space?;
        if !self.image.sample_pixel(column, row, &mut self.values) {
            return None;
        }
        let (rgb, resolved) = color::components_to_rgb(space, &self.values)?;
        self.approximation = self.approximation.and(resolved);
        self.converted = Some(((column, row), rgb));
        Some(rgb)
    }

    fn unmatted(&self, rgb: [f64; 3], mask_alpha: f64) -> Option<[f64; 3]> {
        let Some(matte) = self
            .image
            .soft_mask
            .as_ref()
            .and_then(|mask| mask.matte.as_ref())
        else {
            return Some(rgb);
        };
        let space = self.space?;
        let (backdrop, _) = color::components_to_rgb(space, &matte.value)?;
        let mut undone = [0.0; 3];
        for (channel, slot) in undone.iter_mut().enumerate() {
            *slot = ((rgb[channel] - backdrop[channel]) / mask_alpha + backdrop[channel])
                .clamp(0.0, 1.0);
        }
        Some(undone)
    }
}

fn intersect_boxes(
    left: (u32, u32, u32, u32),
    right: (u32, u32, u32, u32),
) -> Option<(u32, u32, u32, u32)> {
    let x0 = left.0.max(right.0);
    let y0 = left.1.max(right.1);
    let x1 = left.2.min(right.2);
    let y1 = left.3.min(right.3);
    (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
}

#[derive(Default)]
struct ShadeOutcome {
    painted: bool,
    unevaluated: bool,
}

fn averages_in_sample_space(space: &ColorSpace) -> bool {
    match space {
        ColorSpace::DeviceGray
        | ColorSpace::DeviceRgb
        | ColorSpace::DeviceCmyk
        | ColorSpace::CalGray(_)
        | ColorSpace::CalRgb(_)
        | ColorSpace::Lab(_)
        | ColorSpace::IccBased(_) => true,
        ColorSpace::Indexed(_)
        | ColorSpace::Separation(_)
        | ColorSpace::DeviceN(_)
        | ColorSpace::Pattern(_) => false,
    }
}

const MIN_TAPS: u32 = 2;
const MAX_TAPS: u32 = 4;

fn device_box(matrix: Matrix, window: (u32, u32, u32, u32)) -> Option<(u32, u32, u32, u32)> {
    let (wx0, wy0, wx1, wy1) = window;
    if wx1 <= wx0 || wy1 <= wy0 {
        return None;
    }
    let corners =
        [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]].map(|corner| apply(matrix, corner));
    let mut lowest = [f64::INFINITY; 2];
    let mut highest = [f64::NEG_INFINITY; 2];
    for corner in corners {
        if !corner[0].is_finite() || !corner[1].is_finite() {
            return Some(window);
        }
        lowest[0] = lowest[0].min(corner[0]);
        lowest[1] = lowest[1].min(corner[1]);
        highest[0] = highest[0].max(corner[0]);
        highest[1] = highest[1].max(corner[1]);
    }
    let x0 = pixel_floor(lowest[0] - 1.0).max(wx0);
    let y0 = pixel_floor(lowest[1] - 1.0).max(wy0);
    let x1 = pixel_ceil(highest[0] + 1.0, wx1).min(wx1);
    let y1 = pixel_ceil(highest[1] + 1.0, wy1).min(wy1);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((x0, y0, x1, y1))
}

fn pixel_floor(value: f64) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    let mut low = 0_u32;
    let mut high = u32::MAX;
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        if f64::from(middle) <= value {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    low
}

fn pixel_ceil(value: f64, limit: u32) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    let floor = pixel_floor(value);
    if f64::from(floor) < value {
        floor.saturating_add(1).min(limit)
    } else {
        floor.min(limit)
    }
}

fn sample_rates(inverse: Matrix, width: u32, height: u32) -> (f64, f64) {
    let width = f64::from(width);
    let height = f64::from(height);
    (
        (inverse.a * width).hypot(inverse.b * height),
        (inverse.c * width).hypot(inverse.d * height),
    )
}

fn taps_for(rate: f64) -> u32 {
    if !rate.is_finite() {
        return MIN_TAPS;
    }
    let mut count = MIN_TAPS;
    while count < MAX_TAPS && f64::from(count) < rate {
        count += 1;
    }
    count
}

fn image_sample_position(unit: [f64; 2], image: &pdf_paint::ImagePaint) -> Option<(u32, u32)> {
    let [u, v] = unit;
    if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
        return None;
    }
    Some((
        scaled_index(u, image.width.value)?,
        scaled_index(1.0 - v, image.height.value)?,
    ))
}

fn scaled_index(fraction: f64, extent: u32) -> Option<u32> {
    if extent == 0 || !(0.0..1.0).contains(&fraction) {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let index = (fraction * f64::from(extent)) as u32;
    Some(index.min(extent - 1))
}

fn apply(matrix: Matrix, point: [f64; 2]) -> [f64; 2] {
    [
        matrix.a.madd(point[0], matrix.c.madd(point[1], matrix.e)),
        matrix.b.madd(point[0], matrix.d.madd(point[1], matrix.f)),
    ]
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Blend {
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl Blend {
    fn of(mode: &BlendMode) -> Self {
        for name in &mode.names {
            if let Some(blend) = Self::named(name) {
                return blend;
            }
        }
        Self::Normal
    }

    fn named(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"/Normal" | b"/Compatible" => Self::Normal,
            b"/Multiply" => Self::Multiply,
            b"/Screen" => Self::Screen,
            b"/Overlay" => Self::Overlay,
            b"/Darken" => Self::Darken,
            b"/Lighten" => Self::Lighten,
            b"/ColorDodge" => Self::ColorDodge,
            b"/ColorBurn" => Self::ColorBurn,
            b"/HardLight" => Self::HardLight,
            b"/SoftLight" => Self::SoftLight,
            b"/Difference" => Self::Difference,
            b"/Exclusion" => Self::Exclusion,
            b"/Hue" => Self::Hue,
            b"/Saturation" => Self::Saturation,
            b"/Color" => Self::Color,
            b"/Luminosity" => Self::Luminosity,
            _ => return None,
        })
    }

    fn apply(self, backdrop: [f32; 3], source: [f32; 3]) -> [f32; 3] {
        match self {
            Self::Normal => source,
            Self::Hue | Self::Saturation | Self::Color | Self::Luminosity => {
                self.non_separable(backdrop, source)
            }
            _ => {
                let mut out = [0.0_f32; 3];
                for (channel, slot) in out.iter_mut().enumerate() {
                    *channel_or(slot) = self.separable(backdrop[channel], source[channel]);
                }
                out
            }
        }
    }

    fn separable(self, backdrop: f32, source: f32) -> f32 {
        match self {
            Self::Multiply => backdrop * source,
            Self::Screen => source.madd(-backdrop, backdrop + source),
            Self::Overlay => Self::HardLight.separable(source, backdrop),
            Self::Darken => backdrop.min(source),
            Self::Lighten => backdrop.max(source),
            Self::ColorDodge => {
                if backdrop <= 0.0 {
                    0.0
                } else if source >= 1.0 {
                    1.0
                } else {
                    (backdrop / (1.0 - source)).min(1.0)
                }
            }
            Self::ColorBurn => {
                if backdrop >= 1.0 {
                    1.0
                } else if source <= 0.0 {
                    0.0
                } else {
                    1.0 - ((1.0 - backdrop) / source).min(1.0)
                }
            }
            Self::HardLight => {
                if source <= 0.5 {
                    Self::Multiply.separable(backdrop, 2.0 * source)
                } else {
                    Self::Screen.separable(backdrop, source.madd(2.0, -1.0))
                }
            }
            Self::SoftLight => {
                let d = if backdrop <= 0.25 {
                    ((16.0 * backdrop - 12.0) * backdrop + 4.0) * backdrop
                } else {
                    backdrop.sqrt()
                };
                if source <= 0.5 {
                    source
                        .madd(-2.0, 1.0)
                        .madd(-(backdrop * (1.0 - backdrop)), backdrop)
                } else {
                    source.madd(2.0, -1.0).madd(d - backdrop, backdrop)
                }
            }
            Self::Difference => (backdrop - source).abs(),
            Self::Exclusion => (backdrop * source).madd(-2.0, backdrop + source),
            _ => source,
        }
    }

    fn non_separable(self, backdrop: [f32; 3], source: [f32; 3]) -> [f32; 3] {
        match self {
            Self::Hue => set_luminosity(
                set_saturation(source, saturation(backdrop)),
                luminosity(backdrop),
            ),
            Self::Saturation => set_luminosity(
                set_saturation(backdrop, saturation(source)),
                luminosity(backdrop),
            ),
            Self::Color => set_luminosity(source, luminosity(backdrop)),
            Self::Luminosity => set_luminosity(backdrop, luminosity(source)),
            _ => source,
        }
    }
}

fn channel_or(slot: &mut f32) -> &mut f32 {
    slot
}

fn luminosity(colour: [f32; 3]) -> f32 {
    0.3_f32.madd(colour[0], 0.59_f32.madd(colour[1], 0.11 * colour[2]))
}

fn clip_colour(colour: [f32; 3]) -> [f32; 3] {
    let lum = luminosity(colour);
    let lowest = colour[0].min(colour[1]).min(colour[2]);
    let highest = colour[0].max(colour[1]).max(colour[2]);
    let mut out = colour;
    if lowest < 0.0 {
        for (slot, value) in out.iter_mut().zip(colour) {
            *slot = lum + (value - lum) * lum / (lum - lowest);
        }
    }
    if highest > 1.0 {
        let scale = (1.0 - lum) / (highest - lum);
        for (slot, value) in out.iter_mut().zip(colour) {
            *slot = (value - lum).madd(scale, lum);
        }
    }
    out
}

fn set_luminosity(colour: [f32; 3], target: f32) -> [f32; 3] {
    let delta = target - luminosity(colour);
    clip_colour([colour[0] + delta, colour[1] + delta, colour[2] + delta])
}

fn saturation(colour: [f32; 3]) -> f32 {
    colour[0].max(colour[1]).max(colour[2]) - colour[0].min(colour[1]).min(colour[2])
}

fn set_saturation(colour: [f32; 3], target: f32) -> [f32; 3] {
    let lowest = colour[0].min(colour[1]).min(colour[2]);
    let highest = colour[0].max(colour[1]).max(colour[2]);
    let mut out = [0.0_f32; 3];
    if highest > lowest {
        for (slot, value) in out.iter_mut().zip(colour) {
            *slot = (value - lowest) * target / (highest - lowest);
        }
    }
    out
}

fn composite(
    canvas: &mut Canvas,
    mask: &Mask,
    clip: Option<&Mask>,
    rgb: [f64; 3],
    alpha: f64,
    blend: Blend,
) {
    #[allow(clippy::cast_possible_truncation)]
    let alpha = alpha.clamp(0.0, 1.0) as f32;
    if alpha <= 0.0 {
        return;
    }
    let (x0, y0, x1, y1) = shared_box(mask.bounds(), canvas.bounds());
    for y in y0..y1 {
        for x in x0..x1 {
            let mut coverage = mask.at(x, y);
            if coverage <= 0.0 {
                continue;
            }
            if let Some(clip) = clip {
                coverage *= clip.at(x, y);
                if coverage <= 0.0 {
                    continue;
                }
            }
            blend_pixel(canvas, x, y, rgb, coverage * alpha, blend);
        }
    }
}

fn shared_box(left: (u32, u32, u32, u32), right: (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
    (
        left.0.max(right.0),
        left.1.max(right.1),
        left.2.min(right.2),
        left.3.min(right.3),
    )
}

fn blend_pixel(canvas: &mut Canvas, x: u32, y: u32, rgb: [f64; 3], coverage: f32, blend: Blend) {
    let coverage = coverage.clamp(0.0, 1.0);
    if coverage <= 0.0 {
        return;
    }
    let Some(index) = canvas.index(x, y) else {
        return;
    };
    let pixel = &mut canvas.pixels[index];
    #[allow(clippy::cast_possible_truncation)]
    let mut source = [
        rgb[0].clamp(0.0, 1.0) as f32,
        rgb[1].clamp(0.0, 1.0) as f32,
        rgb[2].clamp(0.0, 1.0) as f32,
    ];
    if blend != Blend::Normal {
        source = blend.apply(*pixel, source);
    }
    for (channel, value) in pixel.iter_mut().zip(source) {
        *channel = coverage.madd(value - *channel, *channel);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};
    use pdf_content::{
        ContentLimits, PageContentLimits, load_page_program_strict, parse_operations_strict,
    };
    use pdf_paint::{PaintLimits, PaintStream, interpret_stream_sequence_with_resources};

    use super::{
        Approximation, Blend, BlendMode, Canvas, MAX_TAPS, MIN_TAPS, Matrix, PaintGraph,
        RenderError, RenderOptions, RenderReport, Unsupported, luminosity, render_page,
        render_region, sample_rates, taps_for, to_byte,
    };
    use pdf_content::PageGeometry;

    fn page_fixture(page_entries: &[u8], resources: &[u8], content: &[u8]) -> ByteStore {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R ");
        bytes.extend_from_slice(page_entries);
        bytes.extend_from_slice(b" /Resources << ");
        bytes.extend_from_slice(resources);
        bytes.extend_from_slice(b" >> >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
        bytes.extend_from_slice(content.len().to_string().as_bytes());
        bytes.extend_from_slice(b" >>\nstream\n");
        bytes.extend_from_slice(content);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        let xref = bytes.len();
        bytes.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(b"trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n");
        bytes.extend_from_slice(xref.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        ByteStore::new(SourceId::new(90), Arc::<[u8]>::from(bytes))
    }

    fn image_page_fixture(
        image_entries: &[u8],
        image_data: &[u8],
        soft_mask: Option<(&[u8], &[u8])>,
        content: &[u8],
    ) -> ByteStore {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 2 2] /Kids [3 0 R] /Count 1 >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Im1 5 0 R >> >> >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
        bytes.extend_from_slice(content.len().to_string().as_bytes());
        bytes.extend_from_slice(b" >>\nstream\n");
        bytes.extend_from_slice(content);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"5 0 obj\n<< /Type /XObject /Subtype /Image ");
        bytes.extend_from_slice(image_entries);
        bytes.extend_from_slice(b" /Length ");
        bytes.extend_from_slice(image_data.len().to_string().as_bytes());
        bytes.extend_from_slice(b" >>\nstream\n");
        bytes.extend_from_slice(image_data);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        if let Some((mask_entries, mask_data)) = soft_mask {
            offsets.push(bytes.len());
            bytes.extend_from_slice(b"6 0 obj\n<< /Type /XObject /Subtype /Image ");
            bytes.extend_from_slice(mask_entries);
            bytes.extend_from_slice(b" /Length ");
            bytes.extend_from_slice(mask_data.len().to_string().as_bytes());
            bytes.extend_from_slice(b" >>\nstream\n");
            bytes.extend_from_slice(mask_data);
            bytes.extend_from_slice(b"\nendstream\nendobj\n");
        }
        let size = offsets.len() + 1;
        let xref = bytes.len();
        bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n").as_bytes(),
        );
        bytes.extend_from_slice(xref.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        ByteStore::new(SourceId::new(91), Arc::<[u8]>::from(bytes))
    }

    fn render(page_entries: &[u8], resources: &[u8], content: &[u8]) -> (Canvas, RenderReport) {
        let source = page_fixture(page_entries, resources, content);
        render_source(&source)
    }

    fn render_source(source: &ByteStore) -> (Canvas, RenderReport) {
        let program = load_page_program_strict(source, 0, PageContentLimits::default())
            .expect("fixture page");
        let operations =
            parse_operations_strict(&program.streams[0].bytes, ContentLimits::default())
                .expect("fixture operations");
        let graph = interpret_stream_sequence_with_resources(
            &[PaintStream {
                source: &program.streams[0].bytes,
                reference: program.streams[0].reference,
                operations: &operations,
            }],
            program.page,
            &[],
            &program.resources,
            PaintLimits::default(),
        )
        .expect("fixture paint graph");
        render_page(&graph, &program.geometry, RenderOptions::default()).expect("rendered page")
    }

    fn assert_pixel(canvas: &Canvas, x: u32, y: u32, expected: [f64; 3], tolerance: f64) {
        let pixel = canvas.pixel(x, y);
        for (channel, expected) in pixel.iter().zip(expected) {
            assert!(
                (f64::from(*channel) - expected).abs() <= tolerance,
                "pixel ({x}, {y}) is {pixel:?}, expected {expected:?}"
            );
        }
    }

    #[test]
    fn an_undrawn_code_is_reported_by_the_reason_the_substitution_recorded() {
        use pdf_paint::{Matrix as PaintMatrix, PositionedGlyph, UnresolvedReason};

        let substitution = |unreadable: bool| {
            if unreadable {
                pdf_content::ProgramEvidence::Unreadable {
                    key: b"/FontFile".to_vec(),
                    reason: "a Type 1 program this build does not parse".to_owned(),
                }
            } else {
                pdf_content::ProgramEvidence::NotEmbedded
            }
        };

        let glyph = |unresolved| PositionedGlyph {
            code: pdf_content::SourceCode {
                bytes: vec![0x00, 0x2e],
                value: 0x002e,
                byte_offset: 0,
                cid: None,
                mapping_span: None,
                completed_bytes: 0,
                width: 0.0,
            },
            glyph: None,
            matrix: PaintMatrix::IDENTITY,
            text_matrix: PaintMatrix::IDENTITY,
            procedure: None,
            substituted: Vec::new(),
            silent: false,
            unresolved,
        };

        for reason in [
            UnresolvedReason::NoEvidence,
            UnresolvedReason::AmbiguousEncoding,
            UnresolvedReason::TruncatedMultibyte,
            UnresolvedReason::UnmappedMultibyte,
        ] {
            assert_eq!(
                super::undrawn_reason(&glyph(Some(reason)), Some(&substitution(false))),
                Unsupported::TextUnresolvedCode,
                "{reason:?} was reported as something other than a mapping gap"
            );
        }
        for reason in [
            UnresolvedReason::ClusterTooLong,
            UnresolvedReason::MultipleBaseCharacters,
        ] {
            assert_eq!(
                super::undrawn_reason(&glyph(Some(reason)), Some(&substitution(false))),
                Unsupported::TextClusterUnplaceable,
                "{reason:?} was reported as a missing glyph"
            );
        }
        assert_eq!(
            super::undrawn_reason(
                &glyph(Some(UnresolvedReason::NoFaceCoverage)),
                Some(&substitution(false))
            ),
            Unsupported::TextSubstituteGlyph
        );
        assert_eq!(
            super::undrawn_reason(&glyph(None), None),
            Unsupported::TextGlyph
        );
        assert_eq!(
            super::undrawn_reason(&glyph(None), Some(&substitution(false))),
            Unsupported::TextSubstituteGlyph
        );
        assert_eq!(
            super::undrawn_reason(
                &glyph(Some(UnresolvedReason::NoEvidence)),
                Some(&substitution(true))
            ),
            Unsupported::TextProgramUnreadable
        );
        assert_eq!(
            super::undrawn_reason(
                &glyph(Some(UnresolvedReason::NoFaceCoverage)),
                Some(&substitution(true))
            ),
            Unsupported::TextSubstituteGlyph
        );
        assert_eq!(
            Unsupported::TextUnresolvedCode.to_string(),
            "text whose codes the file gives no meaning for"
        );
        assert_ne!(
            Unsupported::TextUnresolvedCode.to_string(),
            Unsupported::TextSubstituteGlyph.to_string()
        );
    }

    #[test]
    fn bmp_known_answer_has_bgr_bottom_up_rows_and_padding() {
        let canvas = Canvas {
            origin: (0, 0),
            width: 2,
            height: 2,
            pixels: vec![
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 1.0, 1.0],
            ],
        };
        let bytes = canvas.to_bmp();

        assert_eq!(&bytes[..2], b"BM");
        assert_eq!(u32::from_le_bytes(bytes[2..6].try_into().unwrap()), 70);
        assert_eq!(u32::from_le_bytes(bytes[10..14].try_into().unwrap()), 54);
        assert_eq!(u32::from_le_bytes(bytes[18..22].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(bytes[22..26].try_into().unwrap()), 2);
        assert_eq!(u16::from_le_bytes(bytes[28..30].try_into().unwrap()), 24);
        assert_eq!(
            &bytes[54..],
            &[255, 0, 0, 255, 255, 255, 0, 0, 0, 0, 255, 0, 255, 0, 0, 0,]
        );
    }

    #[test]
    fn a_quarter_page_fill_covers_exactly_one_quarter() {
        let (canvas, report) = render(b"/MediaBox [0 0 100 100]", b"", b"0 0 0 rg 0 0 50 50 re f");
        assert_eq!((canvas.width, canvas.height), (100, 100));
        assert!((canvas.ink_fraction() - 0.25).abs() < f64::EPSILON);
        assert_pixel(&canvas, 10, 90, [0.0, 0.0, 0.0], 0.0);
        assert_pixel(&canvas, 10, 10, [1.0, 1.0, 1.0], 0.0);
        assert_eq!(report.drawn, 1);
        assert!(report.skipped.is_empty());
        assert!(report.is_faithful());
    }

    #[test]
    fn even_odd_leaves_the_hole_that_nonzero_fills() {
        let (nonzero, _) = render(
            b"/MediaBox [0 0 100 100]",
            b"",
            b"0 0 0 rg 0 0 100 100 re 25 25 50 50 re f",
        );
        assert!((nonzero.ink_fraction() - 1.0).abs() < f64::EPSILON);
        let (even_odd, _) = render(
            b"/MediaBox [0 0 100 100]",
            b"",
            b"0 0 0 rg 0 0 100 100 re 25 25 50 50 re f*",
        );
        assert!((even_odd.ink_fraction() - 0.75).abs() < f64::EPSILON);
        assert_pixel(&even_odd, 50, 50, [1.0, 1.0, 1.0], 0.0);
        assert_pixel(&even_odd, 10, 10, [0.0, 0.0, 0.0], 0.0);
    }

    #[test]
    fn a_clip_removes_exactly_the_area_outside_it() {
        let (canvas, _) = render(
            b"/MediaBox [0 0 100 100]",
            b"",
            b"0 0 50 100 re W n 0 0 0 rg 0 0 100 100 re f",
        );
        assert!((canvas.ink_fraction() - 0.5).abs() < f64::EPSILON);
        assert_pixel(&canvas, 25, 50, [0.0, 0.0, 0.0], 0.0);
        assert_pixel(&canvas, 75, 50, [1.0, 1.0, 1.0], 0.0);
    }

    #[test]
    fn every_supported_colour_space_reaches_a_known_srgb_value() {
        let (canvas, report) = render(
            b"/MediaBox [0 0 100 100]",
            b"/ColorSpace << /LAB [/Lab << /WhitePoint [.9505 1 1.089] >>] \
              /IDX [/Indexed /DeviceRGB 1 <FF000000FF00>] \
              /SEP [/Separation /Spot /DeviceRGB << /FunctionType 2 /Domain [0 1] /C0 [1 1 1] /C1 [0 .5 0] /N 1 >>] >>",
            b"/DeviceCMYK cs 1 0 0 0 sc 0 0 20 100 re f \
              /LAB cs 50 0 0 sc 20 0 20 100 re f \
              /IDX cs 1 sc 40 0 20 100 re f \
              /SEP cs 1 scn 60 0 20 100 re f \
              0 0 1 rg 80 0 20 100 re f",
        );
        assert_pixel(&canvas, 10, 50, [0.0, 174.0 / 255.0, 239.0 / 255.0], 1e-6);
        assert_pixel(&canvas, 30, 50, [0.4663, 0.4663, 0.4663], 0.001);
        assert_pixel(&canvas, 50, 50, [0.0, 1.0, 0.0], 1e-6);
        assert_pixel(&canvas, 70, 50, [0.0, 0.5, 0.0], 1e-6);
        assert_pixel(&canvas, 90, 50, [0.0, 0.0, 1.0], 1e-6);
        assert_eq!(report.drawn, 5);
        assert!(
            report
                .approximations
                .contains(&Approximation::UnmanagedCmyk)
        );
        assert!(
            !report
                .approximations
                .contains(&Approximation::TintTransform)
        );
        assert!(!report.is_faithful());
    }

    #[test]
    fn a_separation_marks_exactly_when_its_colourant_says_it_should() {
        let spaces: &[u8] = b"/ColorSpace << \
              /NAMED [/Separation /Spot /DeviceRGB << /FunctionType 2 /Domain [0 1] /C0 [1 1 1] /C1 [0 .5 0] /N 1 >>] \
              /ALL [/Separation /All /DeviceRGB << /FunctionType 2 /Domain [0 1] /C0 [1 1 1] /C1 [0 .5 0] /N 1 >>] \
              /NONE [/Separation /None /DeviceRGB << /FunctionType 2 /Domain [0 1] /C0 [1 1 1] /C1 [0 .5 0] /N 1 >>] >>";

        let (canvas, report) = render(
            b"/MediaBox [0 0 30 100]",
            spaces,
            b"/NAMED cs 1 scn 0 0 10 100 re f",
        );
        assert_pixel(&canvas, 5, 50, [0.0, 0.5, 0.0], 1e-6);
        assert!(report.is_faithful(), "a named colourant is drawn as asked");

        let (canvas, report) = render(
            b"/MediaBox [0 0 30 100]",
            spaces,
            b"/ALL cs 1 scn 0 0 10 100 re f",
        );
        assert_pixel(&canvas, 5, 50, [0.0, 0.5, 0.0], 1e-6);
        assert!(
            report
                .approximations
                .contains(&Approximation::TintTransform),
            "/All asks for every colourant and three is not every"
        );

        let (canvas, report) = render(
            b"/MediaBox [0 0 30 100]",
            spaces,
            b"/NONE cs 1 scn 0 0 10 100 re f",
        );
        assert_pixel(&canvas, 5, 50, [1.0, 1.0, 1.0], 1e-6);
        assert!(
            (canvas.ink_fraction() - 0.0).abs() < f64::EPSILON,
            "/None shall not be painted"
        );
        assert!(report.skipped.is_empty(), "obeying /None is not a refusal");
        assert!(report.is_faithful());
    }

    #[test]
    fn a_nested_colour_space_reports_the_shortcut_its_alternate_took() {
        let (canvas, report) = render(
            b"/MediaBox [0 0 100 100]",
            b"/ColorSpace << /SEP [/Separation /Spot /DeviceCMYK \
              << /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [1 0 0 0] /N 1 >>] >>",
            b"/SEP cs 1 scn 0 0 100 100 re f",
        );
        assert_pixel(&canvas, 50, 50, [0.0, 174.0 / 255.0, 239.0 / 255.0], 1e-6);
        assert!(
            report
                .approximations
                .contains(&Approximation::UnmanagedCmyk),
            "the alternate's shortcut is the one that reached the page"
        );
        assert!(
            !report
                .approximations
                .contains(&Approximation::TintTransform)
        );
    }

    #[test]
    fn an_axial_shading_interpolates_between_its_end_colours() {
        let (canvas, report) = render(
            b"/MediaBox [0 0 100 100]",
            b"/Shading << /S1 << /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 100 0] \
              /Extend [true true] /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] \
              /C1 [0 0 1] /N 1 >> >> >>",
            b"/S1 sh",
        );
        assert_pixel(&canvas, 0, 50, [0.995, 0.0, 0.005], 0.01);
        assert_pixel(&canvas, 50, 50, [0.495, 0.0, 0.505], 0.01);
        assert_pixel(&canvas, 99, 50, [0.005, 0.0, 0.995], 0.01);
        assert_eq!(report.drawn, 1);
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn a_function_shading_colours_each_point_of_its_placed_rectangle() {
        let content = b"/S1 sh";
        let program = b"{ 0 exch }";
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for object in [
            &b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n"[..],
            b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n",
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /MediaBox [0 0 100 100] \
              /Resources << /Shading << /S1 << /ShadingType 1 /ColorSpace /DeviceRGB \
              /Matrix [50 0 0 50 25 25] /Function 5 0 R >> >> >> >>\nendobj\n",
        ] {
            offsets.push(bytes.len());
            bytes.extend_from_slice(object);
        }
        for (number, entries, data) in [
            (4, &b""[..], &content[..]),
            (
                5,
                b" /FunctionType 4 /Domain [0 1 0 1] /Range [0 1 0 1 0 1]",
                &program[..],
            ),
        ] {
            offsets.push(bytes.len());
            bytes
                .extend_from_slice(format!("{number} 0 obj\n<< /Length {}", data.len()).as_bytes());
            bytes.extend_from_slice(entries);
            bytes.extend_from_slice(b" >>\nstream\n");
            bytes.extend_from_slice(data);
            bytes.extend_from_slice(b"\nendstream\nendobj\n");
        }
        let xref = bytes.len();
        bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        let (canvas, report) =
            render_source(&ByteStore::new(SourceId::new(91), Arc::<[u8]>::from(bytes)));
        assert_pixel(&canvas, 30, 69, [0.11, 0.0, 0.11], 0.01);
        assert_pixel(&canvas, 70, 30, [0.91, 0.0, 0.89], 0.01);
        assert_pixel(&canvas, 10, 10, [1.0, 1.0, 1.0], 0.0);
        assert_pixel(&canvas, 90, 50, [1.0, 1.0, 1.0], 0.0);
        assert_eq!(report.drawn, 1);
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn raw_images_map_rows_masks_and_stencils_to_known_pixels() {
        let source = image_page_fixture(
            b"/Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8",
            &[255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255],
            None,
            b"2 0 0 2 0 0 cm /Im1 Do",
        );
        let (canvas, report) = render_source(&source);
        assert_eq!((canvas.width, canvas.height), (2, 2));
        assert_pixel(&canvas, 0, 0, [1.0, 0.0, 0.0], 0.0);
        assert_pixel(&canvas, 1, 0, [0.0, 1.0, 0.0], 0.0);
        assert_pixel(&canvas, 0, 1, [0.0, 0.0, 1.0], 0.0);
        assert_pixel(&canvas, 1, 1, [1.0, 1.0, 1.0], 0.0);
        assert_eq!(report.drawn, 1);
        assert!(report.is_faithful());

        let source = image_page_fixture(
            b"/Width 2 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask 6 0 R",
            &[255, 0, 0, 255, 0, 0],
            Some((
                b"/Width 2 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8",
                &[255, 0],
            )),
            b"2 0 0 1 0 1 cm /Im1 Do",
        );
        let (canvas, report) = render_source(&source);
        assert_pixel(&canvas, 0, 0, [1.0, 0.0, 0.0], 0.0);
        assert_pixel(&canvas, 1, 0, [1.0, 1.0, 1.0], 0.0);
        assert_eq!(report.drawn, 1);
        assert!(report.is_faithful());

        let source = image_page_fixture(
            b"/Width 2 /Height 1 /ImageMask true",
            &[0b0100_0000],
            None,
            b"1 0 0 rg 2 0 0 1 0 1 cm /Im1 Do",
        );
        let (canvas, report) = render_source(&source);
        assert_pixel(&canvas, 0, 0, [1.0, 0.0, 0.0], 0.0);
        assert_pixel(&canvas, 1, 0, [1.0, 1.0, 1.0], 0.0);
        assert_eq!(report.drawn, 1);
        assert!(report.is_faithful());
    }

    #[test]
    fn a_stencil_mask_filled_with_a_tiling_pattern_paints_the_pattern() {
        let content = b"/Pattern cs /P1 scn 2 0 0 1 0 0 cm /Im1 Do";
        let cell = b"0 0 1 rg 0 0 1 1 re f";
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 2 1] /Kids [3 0 R] /Count 1 >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources \
              << /XObject << /Im1 5 0 R >> /Pattern << /P1 6 0 R >> >> >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
        bytes.extend_from_slice(content.len().to_string().as_bytes());
        bytes.extend_from_slice(b" >>\nstream\n");
        bytes.extend_from_slice(content);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"5 0 obj\n<< /Type /XObject /Subtype /Image /Width 2 /Height 1 \
              /ImageMask true /Length 1 >>\nstream\n",
        );
        bytes.push(0b0100_0000);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"6 0 obj\n<< /Type /Pattern /PatternType 1 /PaintType 1 /TilingType 2 \
              /BBox [0 0 1 1] /XStep 1 /YStep 1 /Resources << >> /Length ",
        );
        bytes.extend_from_slice(cell.len().to_string().as_bytes());
        bytes.extend_from_slice(b" >>\nstream\n");
        bytes.extend_from_slice(cell);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        let size = offsets.len() + 1;
        let xref = bytes.len();
        bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n").as_bytes(),
        );
        bytes.extend_from_slice(xref.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        let source = ByteStore::new(SourceId::new(97), Arc::<[u8]>::from(bytes));

        let (canvas, report) = render_source(&source);
        assert_pixel(&canvas, 0, 0, [0.0, 0.0, 1.0], 0.02);
        assert_pixel(&canvas, 1, 0, [1.0, 1.0, 1.0], 0.0);
        assert!(report.drawn >= 1, "the stencil drew nothing");
        assert!(
            report.skipped.is_empty(),
            "the pattern colour was refused: {:?}",
            report.skipped_counts()
        );
    }

    #[test]
    fn a_reduced_image_averages_what_it_covers_instead_of_keeping_one_texel() {
        let mut samples = Vec::new();
        for row in 0..4 {
            for column in 0..4 {
                let value = if (row + column) % 2 == 0 { 255 } else { 0 };
                samples.extend_from_slice(&[value, value, value]);
            }
        }
        let source = image_page_fixture(
            b"/Width 4 /Height 4 /ColorSpace /DeviceRGB /BitsPerComponent 8",
            &samples,
            None,
            b"1 0 0 1 0 0 cm /Im1 Do",
        );
        let (canvas, report) = render_source(&source);
        assert_pixel(&canvas, 0, 1, [0.5, 0.5, 0.5], 0.01);
        assert_pixel(&canvas, 1, 1, [1.0, 1.0, 1.0], 0.0);
        assert_eq!(report.drawn, 1);
        assert!(report.is_faithful());
    }

    #[test]
    fn an_images_edge_carries_coverage_rather_than_a_stair() {
        let source = image_page_fixture(
            b"/Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8",
            &[0, 0, 0],
            None,
            b"1.5 0 0 1 0 0 cm /Im1 Do",
        );
        let (canvas, _) = render_source(&source);
        assert_pixel(&canvas, 0, 1, [0.0, 0.0, 0.0], 0.0);
        let partial = canvas.pixels[canvas.width as usize + 1][0];
        assert!(
            partial > 0.1 && partial < 0.9,
            "the split pixel is neither black nor white: {partial}"
        );
    }

    #[test]
    fn the_tap_grid_follows_how_many_texels_a_pixel_covers() {
        assert_eq!(taps_for(0.1), MIN_TAPS);
        assert_eq!(taps_for(1.0), MIN_TAPS);
        assert_eq!(taps_for(2.0), 2);
        assert_eq!(taps_for(2.5), 3);
        assert_eq!(taps_for(4.0), MAX_TAPS);
        assert_eq!(taps_for(1000.0), MAX_TAPS);
        assert_eq!(taps_for(f64::NAN), MIN_TAPS);
    }

    #[test]
    fn the_sampling_rate_is_texels_per_device_pixel() {
        let inverse = Matrix {
            a: 1.0 / 25.0,
            b: 0.0,
            c: 0.0,
            d: 1.0 / 25.0,
            e: 0.0,
            f: 0.0,
        };
        let (across, down) = sample_rates(inverse, 100, 100);
        assert!((across - 4.0).abs() < 1e-9, "across: {across}");
        assert!((down - 4.0).abs() < 1e-9, "down: {down}");
    }

    #[test]
    fn a_blend_mode_composites_against_what_is_already_there() {
        let (canvas, report) = render(
            b"/MediaBox [0 0 100 100]",
            b"/ExtGState << /GS << /BM /Multiply >> >>",
            b"0.5 g 0 0 100 100 re f /GS gs 0 0 50 50 re f",
        );
        assert_eq!(report.visited, 2);
        assert_eq!(report.drawn, 2);
        assert!(report.skipped_counts().is_empty());
        assert_pixel(&canvas, 25, 75, [0.25, 0.25, 0.25], 0.01);
        assert_pixel(&canvas, 75, 25, [0.5, 0.5, 0.5], 0.01);
    }

    #[test]
    fn the_blend_functions_answer_what_the_specification_says() {
        let cases: &[(Blend, f32, f32, f32)] = &[
            (Blend::Normal, 0.2, 0.8, 0.8),
            (Blend::Multiply, 0.5, 0.5, 0.25),
            (Blend::Screen, 0.5, 0.5, 0.75),
            (Blend::Darken, 0.2, 0.8, 0.2),
            (Blend::Lighten, 0.2, 0.8, 0.8),
            (Blend::Difference, 0.2, 0.8, 0.6),
            (Blend::Exclusion, 0.5, 0.5, 0.5),
            (Blend::HardLight, 0.5, 0.25, 0.25),
            (Blend::HardLight, 0.5, 0.75, 0.75),
            (Blend::Overlay, 0.25, 0.5, 0.25),
            (Blend::ColorDodge, 0.0, 0.5, 0.0),
            (Blend::ColorDodge, 0.4, 1.0, 1.0),
            (Blend::ColorBurn, 1.0, 0.5, 1.0),
            (Blend::ColorBurn, 0.4, 0.0, 0.0),
        ];
        for (blend, backdrop, source, expected) in cases {
            let got = blend.separable(*backdrop, *source);
            assert!(
                (got - expected).abs() < 1e-6,
                "{blend:?}({backdrop}, {source}) = {got}, expected {expected}"
            );
        }
        let out = Blend::Luminosity.apply([0.2, 0.4, 0.6], [0.5, 0.5, 0.5]);
        assert!((luminosity(out) - 0.5).abs() < 1e-6, "{out:?}");
        let out = Blend::Color.apply([0.5, 0.5, 0.5], [0.2, 0.4, 0.6]);
        assert!((luminosity(out) - 0.5).abs() < 1e-6, "{out:?}");
    }

    #[test]
    fn an_unknown_blend_name_is_normal_and_an_array_picks_the_first_known_one() {
        let named = |names: &[&[u8]]| {
            Blend::of(&BlendMode {
                names: names.iter().map(|name| name.to_vec()).collect(),
            })
        };
        assert_eq!(named(&[b"/Normal"]), Blend::Normal);
        assert_eq!(named(&[b"/NotABlendMode"]), Blend::Normal);
        assert_eq!(named(&[b"/NotABlendMode", b"/Screen"]), Blend::Screen);
        assert_eq!(named(&[b"/Darken", b"/Screen"]), Blend::Darken);
        assert_eq!(named(&[]), Blend::Normal);
    }

    #[test]
    fn page_rotation_swaps_the_grid_and_places_the_origin() {
        let (canvas, _) = render(
            b"/MediaBox [0 0 100 50] /Rotate 90",
            b"",
            b"0 0 0 rg 0 0 10 10 re f",
        );
        assert_eq!((canvas.width, canvas.height), (50, 100));
        assert_pixel(&canvas, 5, 5, [0.0, 0.0, 0.0], 0.0);
        assert_pixel(&canvas, 45, 95, [1.0, 1.0, 1.0], 0.0);
    }

    #[test]
    fn the_crop_box_defines_the_grid_and_the_origin() {
        let (canvas, _) = render(
            b"/MediaBox [0 0 200 100] /CropBox [50 0 150 100]",
            b"",
            b"0 0 0 rg 0 0 60 100 re f",
        );
        assert_eq!((canvas.width, canvas.height), (100, 100));
        assert!((canvas.ink_fraction() - 0.1).abs() < f64::EPSILON);
        assert_pixel(&canvas, 5, 50, [0.0, 0.0, 0.0], 0.0);
        assert_pixel(&canvas, 50, 50, [1.0, 1.0, 1.0], 0.0);
    }

    #[test]
    fn a_channel_rounds_to_the_byte_the_search_found() {
        fn searched(channel: f32) -> u8 {
            let scaled = (channel.clamp(0.0, 1.0) * 255.0) + 0.5;
            let mut low = 0_u8;
            let mut high = u8::MAX;
            while low < high {
                let middle = low + (high - low).div_ceil(2);
                if f32::from(middle) <= scaled {
                    low = middle;
                } else {
                    high = middle - 1;
                }
            }
            low
        }
        let mut tried = 0_u32;
        for byte in 0..=255_u16 {
            let boundary = (f32::from(byte) + 0.5) / 255.0;
            for channel in [
                f32::from_bits(boundary.to_bits() - 1),
                boundary,
                f32::from_bits(boundary.to_bits() + 1),
                f32::from(byte) / 255.0,
            ] {
                assert_eq!(to_byte(channel), searched(channel), "{channel}");
                tried += 1;
            }
        }
        for bits in (0..=1.0_f32.to_bits()).step_by(997) {
            let channel = f32::from_bits(bits);
            assert_eq!(to_byte(channel), searched(channel), "{channel}");
            tried += 1;
        }
        for channel in [f32::NAN, -0.0, -1.0, 1.5, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(to_byte(channel), searched(channel), "{channel}");
        }
        assert!(tried > 1_000_000);
        let mut canvas = Canvas::blank(3, 1);
        canvas.pixels[0] = [0.0, 0.5, 1.0];
        canvas.pixels[1] = [0.2, 0.4, 0.6];
        let rgba: Vec<u8> = canvas
            .to_rgb8()
            .chunks_exact(3)
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
            .collect();
        assert_eq!(canvas.to_rgba8(), rgba);
    }

    #[test]
    fn the_ink_threshold_matches_the_browser_probe() {
        assert_eq!(to_byte(248.0 / 255.0), 248);
        assert_eq!(to_byte(249.0 / 255.0), 249);
        assert_eq!(to_byte(0.0), 0);
        assert_eq!(to_byte(1.0), 255);
        let mut canvas = Canvas::blank(2, 1);
        canvas.pixels[0] = [248.0 / 255.0, 1.0, 1.0];
        assert!((canvas.ink_fraction() - 0.5).abs() < f64::EPSILON);
        canvas.pixels[0] = [249.0 / 255.0, 1.0, 1.0];
        assert!((canvas.ink_fraction() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn ppm_output_carries_the_canvas_exactly() {
        let (canvas, _) = render(b"/MediaBox [0 0 4 2]", b"", b"0 0 0 rg 0 0 2 2 re f");
        let ppm = canvas.to_ppm();
        assert!(ppm.starts_with(b"P6\n4 2\n255\n"));
        assert_eq!(ppm.len(), b"P6\n4 2\n255\n".len() + 4 * 2 * 3);
        assert_eq!(&ppm[ppm.len() - 3..], &[255, 255, 255]);
    }

    fn same_pixel(left: [f32; 3], right: [f32; 3]) -> bool {
        left.iter()
            .zip(right)
            .all(|(left, right)| left.to_bits() == right.to_bits())
    }

    fn graph_of(source: &ByteStore) -> (PaintGraph, PageGeometry) {
        let program = load_page_program_strict(source, 0, PageContentLimits::default())
            .expect("fixture page");
        let operations =
            parse_operations_strict(&program.streams[0].bytes, ContentLimits::default())
                .expect("fixture operations");
        let graph = interpret_stream_sequence_with_resources(
            &[PaintStream {
                source: &program.streams[0].bytes,
                reference: program.streams[0].reference,
                operations: &operations,
            }],
            program.page,
            &[],
            &program.resources,
            PaintLimits::default(),
        )
        .expect("fixture paint graph");
        (graph, program.geometry)
    }

    fn mixed_page() -> ByteStore {
        page_fixture(
            b"/MediaBox [0 0 60 40]",
            b"/Shading << /Sh0 << /ShadingType 2 /ColorSpace /DeviceRGB \
/Coords [0 0 60 0] /Extend [true true] \
/Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 0 1] /N 1 >> >> >>",
            b"0.2 0.4 0.9 rg 4 4 20 12 re f \
0 0 0 RG 2 w 0 0 m 60 40 l S \
q 12 10 30 20 re W n 0.9 0.3 0.1 rg 0 0 60 40 re f Q \
q 30 4 26 14 re W n /Sh0 sh Q",
        )
    }

    #[test]
    fn render_region_matches_the_crop_of_a_full_render() {
        let source = mixed_page();
        let (graph, geometry) = graph_of(&source);
        let (full, _) =
            render_page(&graph, &geometry, RenderOptions::default()).expect("the page renders");
        assert_eq!((full.width, full.height), (60, 40));
        assert!(
            full.ink_fraction() > 0.4,
            "the fixture should cover much of the page, not {}",
            full.ink_fraction()
        );

        for region in [
            [0, 0, 60, 40],
            [0, 0, 12, 9],
            [11, 7, 41, 33],
            [48, 30, 60, 40],
            [29, 3, 31, 5],
        ] {
            let (window, _) = render_region(&graph, &geometry, RenderOptions::default(), region)
                .unwrap_or_else(|error| panic!("region {region:?} renders: {error}"));
            let [x0, y0, x1, y1] = region;
            assert_eq!((window.width, window.height), (x1 - x0, y1 - y0));
            assert_eq!(window.origin, (x0, y0));
            for y in y0..y1 {
                for x in x0..x1 {
                    assert!(
                        same_pixel(window.pixel(x, y), full.pixel(x, y)),
                        "region {region:?} differs from the full render at ({x}, {y}): {:?} vs {:?}",
                        window.pixel(x, y),
                        full.pixel(x, y)
                    );
                }
            }
        }
    }

    #[test]
    fn a_region_of_an_image_is_the_crop_of_the_whole_image() {
        let source = image_page_fixture(
            b"/Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8",
            &[255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0],
            None,
            b"q 2 0 0 2 0 0 cm /Im1 Do Q",
        );
        let (graph, geometry) = graph_of(&source);
        let options = RenderOptions {
            scale: 20.0,
            ..RenderOptions::default()
        };
        let (full, _) = render_page(&graph, &geometry, options).expect("the page renders");
        assert_eq!((full.width, full.height), (40, 40));
        let region = [9, 11, 33, 30];
        let (window, _) =
            render_region(&graph, &geometry, options, region).expect("the region renders");
        for y in region[1]..region[3] {
            for x in region[0]..region[2] {
                assert!(
                    same_pixel(window.pixel(x, y), full.pixel(x, y)),
                    "the image sampled differently at ({x}, {y}): {:?} vs {:?}",
                    window.pixel(x, y),
                    full.pixel(x, y)
                );
            }
        }
        assert!(
            window.ink_fraction() > 0.9,
            "the window should be inside the image, not {}",
            window.ink_fraction()
        );
    }

    #[test]
    fn an_unusable_region_is_refused() {
        let source = mixed_page();
        let (graph, geometry) = graph_of(&source);
        for region in [
            [0, 0, 0, 40],
            [0, 0, 60, 0],
            [10, 10, 5, 20],
            [0, 0, 61, 40],
            [0, 0, 60, 41],
        ] {
            let error = render_region(&graph, &geometry, RenderOptions::default(), region)
                .expect_err("an unusable region is not a rectangle to guess at");
            assert_eq!(error, RenderError::InvalidRegion, "region {region:?}");
        }
    }

    #[test]
    fn a_window_holds_only_its_own_pixels() {
        let source = mixed_page();
        let (graph, geometry) = graph_of(&source);
        let (full, _) =
            render_page(&graph, &geometry, RenderOptions::default()).expect("the page renders");
        let (window, _) = render_region(
            &graph,
            &geometry,
            RenderOptions::default(),
            [20, 10, 26, 14],
        )
        .expect("the region renders");
        assert_eq!(window.pixels.len(), 24);
        assert_eq!(full.pixels.len(), 2400);
    }

    #[test]
    fn a_unit_square_maps_to_the_pixels_it_can_touch() {
        let window = (0, 0, 200, 100);
        let matrix = super::Matrix {
            a: 50.0,
            b: 0.0,
            c: 0.0,
            d: 20.0,
            e: 30.0,
            f: 40.0,
        };
        assert_eq!(
            super::device_box(matrix, window),
            Some((29, 39, 81, 61)),
            "the square's own box, one pixel either way"
        );

        let elsewhere = super::Matrix {
            e: 500.0,
            f: 500.0,
            ..matrix
        };
        assert_eq!(super::device_box(elsewhere, window), None);

        let huge = super::Matrix {
            a: 1000.0,
            d: 1000.0,
            e: -100.0,
            f: -100.0,
            ..matrix
        };
        assert_eq!(super::device_box(huge, window), Some(window));

        let flat = super::Matrix {
            a: 0.0,
            b: 0.0,
            c: 0.0,
            d: 0.0,
            e: 10.0,
            f: 10.0,
        };
        assert_eq!(super::device_box(flat, window), Some((9, 9, 11, 11)));
    }

    #[test]
    fn two_boxes_meet_where_they_overlap_and_nowhere_else() {
        assert_eq!(
            super::intersect_boxes((0, 0, 10, 10), (5, 5, 20, 20)),
            Some((5, 5, 10, 10))
        );
        assert_eq!(
            super::intersect_boxes((0, 0, 10, 10), (10, 0, 20, 10)),
            None
        );
        assert_eq!(
            super::intersect_boxes((0, 0, 10, 10), (0, 0, 10, 10)),
            Some((0, 0, 10, 10))
        );
    }
}
