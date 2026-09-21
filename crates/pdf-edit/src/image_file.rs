use std::fmt;

pub const MOST_PIXELS: u64 = 16_384 * 16_384;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageFileError(&'static str);

impl ImageFileError {
    #[must_use]
    pub const fn reason(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for ImageFileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for ImageFileError {}

const fn refused(reason: &'static str) -> ImageFileError {
    ImageFileError(reason)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Colour {
    Gray,
    Rgb,
    Cmyk,
}

impl Colour {
    pub(crate) const fn components(self) -> usize {
        match self {
            Self::Gray => 1,
            Self::Rgb => 3,
            Self::Cmyk => 4,
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Gray => "/DeviceGray",
            Self::Rgb => "/DeviceRGB",
            Self::Cmyk => "/DeviceCMYK",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Stored<'a> {
    Jpeg(&'a [u8]),
    Samples {
        bits: u8,
        colour: Vec<u8>,
        alpha: Option<Vec<u8>>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageFile<'a> {
    pub width: u32,
    pub height: u32,
    pub orientation: u8,
    pub density: Option<(u32, u32)>,
    pub(crate) colour: Colour,
    pub(crate) inverted: bool,
    pub(crate) stored: Stored<'a>,
}

impl<'a> ImageFile<'a> {
    pub fn read(bytes: &'a [u8]) -> Result<Self, ImageFileError> {
        if bytes.starts_with(&[0xFF, 0xD8]) {
            read_jpeg(bytes)
        } else if bytes.starts_with(PNG_SIGNATURE) {
            read_png(bytes)
        } else {
            Err(refused("this is not a JPEG or PNG picture"))
        }
    }

    #[must_use]
    pub fn dpi(&self) -> Option<(f64, f64)> {
        let (across, down) = self.density?;
        if across == 0 || down == 0 {
            return None;
        }
        Some((
            f64::from(across) * 254.0 / 10_000.0,
            f64::from(down) * 254.0 / 10_000.0,
        ))
    }

    #[must_use]
    pub const fn upright(&self) -> (u32, u32) {
        if self.orientation >= 5 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }

    #[must_use]
    pub const fn orientation_matrix(&self) -> [f64; 6] {
        match self.orientation {
            2 => [-1.0, 0.0, 0.0, 1.0, 1.0, 0.0],
            3 => [-1.0, 0.0, 0.0, -1.0, 1.0, 1.0],
            4 => [1.0, 0.0, 0.0, -1.0, 0.0, 1.0],
            5 => [0.0, -1.0, -1.0, 0.0, 1.0, 1.0],
            6 => [0.0, -1.0, 1.0, 0.0, 0.0, 1.0],
            7 => [0.0, 1.0, 1.0, 0.0, 0.0, 0.0],
            8 => [0.0, 1.0, -1.0, 0.0, 1.0, 0.0],
            _ => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl ImageFile<'_> {
    #[must_use]
    pub fn thumbnail(&self, most: u32) -> Option<Thumbnail> {
        let (channels, colour, alpha) = self.eight_bit_samples()?;
        let rgba = to_rgba(
            (self.width, self.height),
            channels,
            &colour,
            alpha.as_deref(),
        )?;
        let small = shrunk(
            Thumbnail {
                width: self.width,
                height: self.height,
                rgba,
            },
            most,
        );
        Some(turned_upright(small, self.orientation))
    }

    fn eight_bit_samples(&self) -> Option<(usize, Vec<u8>, Option<Vec<u8>>)> {
        match &self.stored {
            Stored::Jpeg(bytes) => {
                let pixels = pdf_paint::picture::jpeg_pixels(bytes)?;
                Some((pixels.channels, pixels.samples, None))
            }
            Stored::Samples {
                bits,
                colour,
                alpha,
            } => {
                let eight = |bytes: &[u8]| -> Option<Vec<u8>> {
                    match bits {
                        8 => Some(bytes.to_vec()),
                        16 => Some(bytes.iter().step_by(2).copied().collect()),
                        _ => None,
                    }
                };
                Some((
                    self.colour.components(),
                    eight(colour)?,
                    match alpha.as_deref() {
                        Some(alpha) => Some(eight(alpha)?),
                        None => None,
                    },
                ))
            }
        }
    }
}

fn to_rgba(
    (width, height): (u32, u32),
    channels: usize,
    colour: &[u8],
    alpha: Option<&[u8]>,
) -> Option<Vec<u8>> {
    let count = (width as usize).checked_mul(height as usize)?;
    if colour.len() < count.checked_mul(channels)? {
        return None;
    }
    if alpha.is_some_and(|alpha| alpha.len() < count) {
        return None;
    }
    let mut rgba = Vec::with_capacity(count * 4);
    for pixel in 0..count {
        let at = pixel * channels;
        let (red, green, blue) = match channels {
            1 => (colour[at], colour[at], colour[at]),
            3 => (colour[at], colour[at + 1], colour[at + 2]),
            4 => {
                let black = u32::from(colour[at + 3]);
                let ink = |value: u8| -> u8 {
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "a product of two bytes over 255 is a byte"
                    )]
                    let out = (u32::from(value) * black / 255) as u8;
                    out
                };
                (ink(colour[at]), ink(colour[at + 1]), ink(colour[at + 2]))
            }
            _ => return None,
        };
        rgba.extend_from_slice(&[red, green, blue, alpha.map_or(255, |alpha| alpha[pixel])]);
    }
    Some(rgba)
}

fn shrunk(picture: Thumbnail, most: u32) -> Thumbnail {
    let longest = picture.width.max(picture.height);
    if longest <= most || most == 0 || picture.width == 0 || picture.height == 0 {
        return picture;
    }
    let scale = f64::from(most) / f64::from(longest);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a side of at most `most` pixels, and never negative"
    )]
    let side = |value: u32| ((f64::from(value) * scale).round() as u32).max(1);
    let (width, height) = (side(picture.width), side(picture.height));
    let mut rgba = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for row in 0..height {
        for column in 0..width {
            let from = |value: u32, out: u32, source: u32| -> (usize, usize) {
                let span = |value: u32| -> usize {
                    let at = u64::from(value) * u64::from(source) / u64::from(out);
                    usize::try_from(at).unwrap_or(usize::MAX)
                };
                let (low, high) = (span(value), span(value + 1));
                (low, high.max(low + 1).min(source as usize))
            };
            let (x0, x1) = from(column, width, picture.width);
            let (y0, y1) = from(row, height, picture.height);
            let mut totals = [0_u64; 4];
            let mut count = 0_u64;
            for y in y0..y1 {
                for x in x0..x1 {
                    let at = (y * (picture.width as usize) + x) * 4;
                    for (channel, total) in totals.iter_mut().enumerate() {
                        *total += u64::from(picture.rgba[at + channel]);
                    }
                    count += 1;
                }
            }
            let count = count.max(1);
            #[expect(clippy::cast_possible_truncation, reason = "a mean of bytes is a byte")]
            for total in totals {
                rgba.push((total / count) as u8);
            }
        }
    }
    Thumbnail {
        width,
        height,
        rgba,
    }
}

fn turned_upright(picture: Thumbnail, orientation: u8) -> Thumbnail {
    if !(2..=8).contains(&orientation) {
        return picture;
    }
    let turns = orientation >= 5;
    let (width, height) = if turns {
        (picture.height, picture.width)
    } else {
        (picture.width, picture.height)
    };
    let mut rgba = vec![0_u8; (width as usize) * (height as usize) * 4];
    for row in 0..picture.height {
        for column in 0..picture.width {
            let (last_x, last_y) = (picture.width - 1, picture.height - 1);
            let (x, y) = match orientation {
                2 => (last_x - column, row),
                3 => (last_x - column, last_y - row),
                4 => (column, last_y - row),
                5 => (row, column),
                6 => (last_y - row, column),
                7 => (last_y - row, last_x - column),
                _ => (row, last_x - column),
            };
            let from = ((row as usize) * (picture.width as usize) + column as usize) * 4;
            let to = ((y as usize) * (width as usize) + x as usize) * 4;
            rgba[to..to + 4].copy_from_slice(&picture.rgba[from..from + 4]);
        }
    }
    Thumbnail {
        width,
        height,
        rgba,
    }
}

fn checked_size(width: u32, height: u32) -> Result<(), ImageFileError> {
    if width == 0 || height == 0 {
        return Err(refused("a picture with no pixels"));
    }
    if u64::from(width) * u64::from(height) > MOST_PIXELS {
        return Err(refused("the picture has more pixels than this places"));
    }
    Ok(())
}

fn read_jpeg(bytes: &[u8]) -> Result<ImageFile<'_>, ImageFileError> {
    let truncated = || refused("the JPEG ends before its picture starts");
    let mut at = 2;
    let mut orientation = 1;
    let mut inverted = false;
    let mut density = None;
    loop {
        if *bytes.get(at).ok_or_else(truncated)? != 0xFF {
            return Err(refused("the JPEG is malformed"));
        }
        while bytes.get(at) == Some(&0xFF) {
            at += 1;
        }
        let marker = *bytes.get(at).ok_or_else(truncated)?;
        at += 1;
        if matches!(marker, 0x01 | 0xD0..=0xD7) {
            continue;
        }
        if matches!(marker, 0xD8..=0xDA) {
            return Err(refused("the JPEG has no frame header"));
        }
        let length = usize::from(u16::from_be_bytes([
            *bytes.get(at).ok_or_else(truncated)?,
            *bytes.get(at + 1).ok_or_else(truncated)?,
        ]));
        let segment = bytes
            .get(at + 2..at + length)
            .filter(|_| length >= 2)
            .ok_or_else(truncated)?;
        match marker {
            0xE1 => {
                if let Some(found) = exif_orientation(segment) {
                    orientation = found;
                }
            }
            0xE0 => density = density.or_else(|| jfif_density(segment)),
            0xEE => inverted |= segment.starts_with(b"Adobe"),
            0xC0..=0xC2 => {
                let [precision, h0, h1, w0, w1, components, ..] = *segment else {
                    return Err(truncated());
                };
                if precision != 8 {
                    return Err(refused("the JPEG is not 8 bits a sample"));
                }
                let height = u32::from(u16::from_be_bytes([h0, h1]));
                let width = u32::from(u16::from_be_bytes([w0, w1]));
                checked_size(width, height)?;
                let colour = match components {
                    1 => Colour::Gray,
                    3 => Colour::Rgb,
                    4 => Colour::Cmyk,
                    _ => return Err(refused("the JPEG has a colour this does not place")),
                };
                return Ok(ImageFile {
                    width,
                    height,
                    orientation,
                    density,
                    colour,
                    inverted,
                    stored: Stored::Jpeg(bytes),
                });
            }
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                return Err(refused(
                    "the JPEG is lossless, hierarchical or arithmetic-coded",
                ));
            }
            _ => {}
        }
        at += length;
    }
}

fn jfif_density(segment: &[u8]) -> Option<(u32, u32)> {
    let body = segment.strip_prefix(b"JFIF\0")?;
    let [_major, _minor, units, x0, x1, y0, y1, ..] = *body else {
        return None;
    };
    let across = u32::from(u16::from_be_bytes([x0, x1]));
    let down = u32::from(u16::from_be_bytes([y0, y1]));
    if across == 0 || down == 0 {
        return None;
    }
    match units {
        1 => Some((across * 10_000 / 254, down * 10_000 / 254)),
        2 => Some((across * 100, down * 100)),
        _ => None,
    }
}

fn physical_density(body: &[u8]) -> Option<(u32, u32)> {
    let [x0, x1, x2, x3, y0, y1, y2, y3, 1] = *body else {
        return None;
    };
    let across = u32::from_be_bytes([x0, x1, x2, x3]);
    let down = u32::from_be_bytes([y0, y1, y2, y3]);
    (across > 0 && down > 0).then_some((across, down))
}

fn exif_orientation(segment: &[u8]) -> Option<u8> {
    let tiff = segment.strip_prefix(b"Exif\0\0")?;
    let big = match tiff.get(..4)? {
        [b'M', b'M', 0, 42] => true,
        [b'I', b'I', 42, 0] => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let pair = [*tiff.get(at)?, *tiff.get(at + 1)?];
        Some(if big {
            u16::from_be_bytes(pair)
        } else {
            u16::from_le_bytes(pair)
        })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let four = [
            *tiff.get(at)?,
            *tiff.get(at + 1)?,
            *tiff.get(at + 2)?,
            *tiff.get(at + 3)?,
        ];
        Some(if big {
            u32::from_be_bytes(four)
        } else {
            u32::from_le_bytes(four)
        })
    };
    let directory = usize::try_from(u32_at(4)?).ok()?;
    let count = usize::from(u16_at(directory)?);
    (0..count).find_map(|index| {
        let entry = directory.checked_add(2 + 12 * index)?;
        if u16_at(entry)? != 0x0112 || u16_at(entry + 2)? != 3 || u32_at(entry + 4)? != 1 {
            return None;
        }
        let value = u16_at(entry + 8)?;
        u8::try_from(value)
            .ok()
            .filter(|value| (1..=8).contains(value))
    })
}

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

#[derive(Clone, Copy)]
struct Header {
    width: u32,
    height: u32,
    depth: u8,
    kind: u8,
    interlaced: bool,
}

impl Header {
    const fn channels(self) -> usize {
        match self.kind {
            2 => 3,
            4 => 2,
            6 => 4,
            _ => 1,
        }
    }

    const fn pixel_bits(self) -> usize {
        self.channels() * self.depth as usize
    }

    fn row_bytes(self, width: usize) -> Option<usize> {
        width
            .checked_mul(self.pixel_bits())?
            .checked_add(7)
            .map(|bits| bits / 8)
    }
}

fn read_png(bytes: &[u8]) -> Result<ImageFile<'static>, ImageFileError> {
    let truncated = || refused("the PNG ends before its picture does");
    let mut at = PNG_SIGNATURE.len();
    let mut header = None;
    let mut palette: Option<&[u8]> = None;
    let mut transparency: Option<&[u8]> = None;
    let mut density = None;
    let mut data = Vec::new();
    loop {
        let length = usize::try_from(u32::from_be_bytes(
            bytes
                .get(at..at + 4)
                .and_then(|four| four.try_into().ok())
                .ok_or_else(truncated)?,
        ))
        .map_err(|_| truncated())?;
        let kind = bytes.get(at + 4..at + 8).ok_or_else(truncated)?;
        let end = at
            .checked_add(8)
            .and_then(|start| start.checked_add(length))
            .ok_or_else(truncated)?;
        let body = bytes.get(at + 8..end).ok_or_else(truncated)?;
        let stated = bytes.get(end..end + 4).ok_or_else(truncated)?;
        if crc32(&bytes[at + 4..end]).to_be_bytes() != stated {
            return Err(refused("the PNG is damaged: a checksum does not match"));
        }
        at = end + 4;
        match kind {
            b"IHDR" => {
                let [
                    w0,
                    w1,
                    w2,
                    w3,
                    h0,
                    h1,
                    h2,
                    h3,
                    depth,
                    colour,
                    0,
                    0,
                    interlace,
                ] = *body
                else {
                    return Err(refused("the PNG header is malformed"));
                };
                let width = u32::from_be_bytes([w0, w1, w2, w3]);
                let height = u32::from_be_bytes([h0, h1, h2, h3]);
                checked_size(width, height)?;
                let allowed: &[u8] = match colour {
                    0 => &[1, 2, 4, 8, 16],
                    3 => &[1, 2, 4, 8],
                    2 | 4 | 6 => &[8, 16],
                    _ => &[],
                };
                if !allowed.contains(&depth) || interlace > 1 {
                    return Err(refused("the PNG header is malformed"));
                }
                header = Some(Header {
                    width,
                    height,
                    depth,
                    kind: colour,
                    interlaced: interlace == 1,
                });
            }
            b"PLTE" => palette = Some(body),
            b"tRNS" => transparency = Some(body),
            b"pHYs" => density = physical_density(body),
            b"IDAT" => data.extend_from_slice(body),
            b"IEND" => break,
            _ if kind[0].is_ascii_uppercase() => {
                return Err(refused("the PNG uses a feature this does not read"));
            }
            _ => {}
        }
    }
    let header = header.ok_or_else(|| refused("the PNG has no header"))?;
    let raw = unfiltered(header, &data)?;
    let (colour, samples, alpha) = expanded(header, &raw, palette, transparency)?;
    Ok(ImageFile {
        width: header.width,
        height: header.height,
        orientation: 1,
        density,
        colour,
        inverted: false,
        stored: Stored::Samples {
            bits: if header.depth == 16 { 16 } else { 8 },
            colour: samples,
            alpha,
        },
    })
}

const ADAM7: [(usize, usize, usize, usize); 7] = [
    (0, 0, 8, 8),
    (4, 0, 8, 8),
    (0, 4, 4, 8),
    (2, 0, 4, 4),
    (0, 2, 2, 4),
    (1, 0, 2, 2),
    (0, 1, 1, 2),
];

fn unfiltered(header: Header, data: &[u8]) -> Result<Vec<u8>, ImageFileError> {
    let too_big = || refused("the picture has more pixels than this places");
    let width = usize::try_from(header.width).map_err(|_| too_big())?;
    let height = usize::try_from(header.height).map_err(|_| too_big())?;
    let passes: Vec<(usize, usize, usize, usize)> = if header.interlaced {
        ADAM7.to_vec()
    } else {
        vec![(0, 0, 1, 1)]
    };
    let mut expected = 0_usize;
    let mut shapes = Vec::new();
    for (x0, y0, dx, dy) in passes {
        let across = width.saturating_sub(x0).div_ceil(dx);
        let down = height.saturating_sub(y0).div_ceil(dy);
        if across == 0 || down == 0 {
            continue;
        }
        let row = header.row_bytes(across).ok_or_else(too_big)?;
        expected = (row + 1)
            .checked_mul(down)
            .and_then(|pass| pass.checked_add(expected))
            .ok_or_else(too_big)?;
        shapes.push((x0, y0, dx, dy, across, down, row));
    }
    let inflated = pdf_syntax::inflate_zlib(data, expected)
        .map_err(|_| refused("the PNG's picture data is damaged"))?;
    if inflated.len() != expected {
        return Err(refused(
            "the PNG's picture data is not the size its header says",
        ));
    }
    let full_row = header.row_bytes(width).ok_or_else(too_big)?;
    let mut image = vec![0_u8; full_row.checked_mul(height).ok_or_else(too_big)?];
    let step = header.pixel_bits().div_ceil(8).max(1);
    let bits = header.pixel_bits();
    let mut at = 0;
    for (x0, y0, dx, dy, across, down, row) in shapes {
        let mut previous = vec![0_u8; row];
        for pass_row in 0..down {
            let filter = inflated[at];
            let mut current = inflated[at + 1..at + 1 + row].to_vec();
            at += row + 1;
            unfilter(filter, &mut current, &previous, step)?;
            let y = y0 + pass_row * dy;
            if header.interlaced {
                for pass_column in 0..across {
                    let x = x0 + pass_column * dx;
                    copy_bits(
                        &current,
                        pass_column * bits,
                        &mut image[y * full_row..],
                        x * bits,
                        bits,
                    );
                }
            } else {
                image[y * full_row..(y + 1) * full_row].copy_from_slice(&current);
            }
            previous = current;
        }
    }
    Ok(image)
}

fn unfilter(
    filter: u8,
    row: &mut [u8],
    previous: &[u8],
    step: usize,
) -> Result<(), ImageFileError> {
    for index in 0..row.len() {
        let left = if index >= step { row[index - step] } else { 0 };
        let up = previous[index];
        let corner = if index >= step {
            previous[index - step]
        } else {
            0
        };
        let predicted = match filter {
            0 => 0,
            1 => left,
            2 => up,
            3 => u8::try_from(u16::midpoint(u16::from(left), u16::from(up))).unwrap_or(0),
            4 => paeth(left, up, corner),
            _ => return Err(refused("the PNG's picture data is damaged")),
        };
        row[index] = row[index].wrapping_add(predicted);
    }
    Ok(())
}

fn paeth(left: u8, up: u8, corner: u8) -> u8 {
    let estimate = i16::from(left) + i16::from(up) - i16::from(corner);
    let to_left = (estimate - i16::from(left)).abs();
    let to_up = (estimate - i16::from(up)).abs();
    let to_corner = (estimate - i16::from(corner)).abs();
    if to_left <= to_up && to_left <= to_corner {
        left
    } else if to_up <= to_corner {
        up
    } else {
        corner
    }
}

fn copy_bits(source: &[u8], from: usize, target: &mut [u8], to: usize, count: usize) {
    if from.is_multiple_of(8) && to.is_multiple_of(8) && count.is_multiple_of(8) {
        target[to / 8..(to + count) / 8].copy_from_slice(&source[from / 8..(from + count) / 8]);
        return;
    }
    for bit in 0..count {
        let set = source[(from + bit) / 8] & (0x80 >> ((from + bit) % 8)) != 0;
        let mask = 0x80 >> ((to + bit) % 8);
        if set {
            target[(to + bit) / 8] |= mask;
        } else {
            target[(to + bit) / 8] &= !mask;
        }
    }
}

type Expanded = (Colour, Vec<u8>, Option<Vec<u8>>);

#[expect(clippy::too_many_lines, reason = "one arm per PNG colour type")]
fn expanded(
    header: Header,
    raw: &[u8],
    palette: Option<&[u8]>,
    transparency: Option<&[u8]>,
) -> Result<Expanded, ImageFileError> {
    let width = header.width as usize;
    let height = header.height as usize;
    let pixels = width * height;
    let row = header.row_bytes(width).unwrap_or(0);
    let bits = header.pixel_bits();
    let wide = header.depth == 16;
    let sample = if wide { 2 } else { 1 };
    let value = |x: usize, y: usize, channel: usize| -> u16 {
        let base = y * row;
        match header.depth {
            16 => {
                let at = base + (x * header.channels() + channel) * 2;
                u16::from_be_bytes([raw[at], raw[at + 1]])
            }
            8 => u16::from(raw[base + x * header.channels() + channel]),
            depth => {
                let bit = x * bits;
                let byte = raw[base + bit / 8];
                let shift = 8 - usize::from(depth) - bit % 8;
                u16::from((byte >> shift) & ((1 << depth) - 1))
            }
        }
    };
    let push = |out: &mut Vec<u8>, value: u16| {
        if wide {
            out.extend_from_slice(&value.to_be_bytes());
        } else {
            out.push(u8::try_from(value).unwrap_or(u8::MAX));
        }
    };
    let full = if wide { u16::MAX } else { 255 };
    let colour = match header.kind {
        0 | 4 => Colour::Gray,
        _ => Colour::Rgb,
    };
    let mut samples = Vec::with_capacity(pixels * colour.components() * sample);
    let mut alpha = Vec::with_capacity(pixels * sample);
    let mut opaque = true;
    let key = |at: usize| -> Option<u16> {
        let pair = transparency?.get(at..at + 2)?;
        Some(u16::from_be_bytes([pair[0], pair[1]]))
    };
    let palette = match header.kind {
        3 => {
            let palette = palette.ok_or_else(|| refused("the PNG has no palette"))?;
            if palette.len() % 3 != 0 || palette.is_empty() {
                return Err(refused("the PNG's palette is malformed"));
            }
            palette
        }
        _ => &[],
    };
    for y in 0..height {
        for x in 0..width {
            let see = match header.kind {
                0 => {
                    let stored = value(x, y, 0);
                    let scaled = if header.depth < 8 {
                        stored * (255 / ((1 << header.depth) - 1))
                    } else {
                        stored
                    };
                    push(&mut samples, scaled);
                    if key(0) == Some(stored) { 0 } else { full }
                }
                2 => {
                    let stored = [value(x, y, 0), value(x, y, 1), value(x, y, 2)];
                    for channel in stored {
                        push(&mut samples, channel);
                    }
                    let keyed = key(0).zip(key(2)).zip(key(4));
                    if keyed == Some(((stored[0], stored[1]), stored[2])) {
                        0
                    } else {
                        full
                    }
                }
                3 => {
                    let index = usize::from(value(x, y, 0));
                    let entry = palette.get(index * 3..index * 3 + 3).ok_or_else(|| {
                        refused("the PNG names a colour its palette does not have")
                    })?;
                    for channel in entry {
                        push(&mut samples, u16::from(*channel));
                    }
                    transparency
                        .and_then(|alphas| alphas.get(index))
                        .map_or(full, |alpha| u16::from(*alpha))
                }
                4 => {
                    push(&mut samples, value(x, y, 0));
                    value(x, y, 1)
                }
                _ => {
                    for channel in 0..3 {
                        push(&mut samples, value(x, y, channel));
                    }
                    value(x, y, 3)
                }
            };
            opaque &= see == full;
            push(&mut alpha, see);
        }
    }
    Ok((colour, samples, (!opaque).then_some(alpha)))
}

pub(crate) fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{Colour, ImageFile, Stored, Thumbnail, crc32, shrunk, to_rgba, turned_upright};

    fn chunk(kind: [u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = u32::try_from(body.len())
            .expect("chunk")
            .to_be_bytes()
            .to_vec();
        out.extend_from_slice(&kind);
        out.extend_from_slice(body);
        let mut checked = kind.to_vec();
        checked.extend_from_slice(body);
        out.extend_from_slice(&crc32(&checked).to_be_bytes());
        out
    }

    pub(crate) fn png(
        (width, height, depth, kind, interlace): (u32, u32, u8, u8, u8),
        extra: &[(&[u8; 4], &[u8])],
        rows: &[u8],
    ) -> Vec<u8> {
        let mut out = super::PNG_SIGNATURE.to_vec();
        let mut header = width.to_be_bytes().to_vec();
        header.extend_from_slice(&height.to_be_bytes());
        header.extend_from_slice(&[depth, kind, 0, 0, interlace]);
        out.extend(chunk(*b"IHDR", &header));
        for (kind, body) in extra {
            out.extend(chunk(**kind, body));
        }
        out.extend(chunk(*b"IDAT", &pdf_syntax::deflate_zlib(rows)));
        out.extend(chunk(*b"IEND", b""));
        out
    }

    fn samples(file: &ImageFile<'_>) -> (u8, Vec<u8>, Option<Vec<u8>>) {
        match &file.stored {
            Stored::Samples {
                bits,
                colour,
                alpha,
            } => (*bits, colour.clone(), alpha.clone()),
            Stored::Jpeg(_) => panic!("a PNG is stored as samples"),
        }
    }

    #[test]
    fn the_checksum_is_the_one_png_states() {
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
    }

    #[test]
    fn every_filter_is_undone() {
        let pixel = [10_u8, 20, 30, 200, 100, 50];
        let mut rows = vec![0];
        rows.extend_from_slice(&pixel);
        rows.push(1);
        rows.extend_from_slice(&[10, 20, 30, 190, 80, 20]);
        rows.push(2);
        rows.extend_from_slice(&[0; 6]);
        rows.push(3);
        rows.extend_from_slice(&[5, 10, 15, 200 - 105, 100 - 60, 50 - 40]);
        rows.push(4);
        rows.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        let bytes = png((2, 5, 8, 2, 0), &[], &rows);
        let file = ImageFile::read(&bytes).expect("reads");
        let (bits, colour, alpha) = samples(&file);
        assert_eq!((bits, file.colour, alpha), (8, Colour::Rgb, None));
        assert_eq!(colour, pixel.repeat(5));
    }

    #[test]
    fn a_palette_with_transparency_becomes_colour_and_alpha() {
        let bytes = png(
            (3, 1, 2, 3, 0),
            &[
                (b"PLTE", &[255, 0, 0, 0, 255, 0, 0, 0, 255]),
                (b"tRNS", &[0, 128]),
            ],
            &[0, 0b0001_1000],
        );
        let file = ImageFile::read(&bytes).expect("reads");
        let (_, colour, alpha) = samples(&file);
        assert_eq!(colour, [255, 0, 0, 0, 255, 0, 0, 0, 255]);
        assert_eq!(alpha, Some(vec![0, 128, 255]));
    }

    #[test]
    fn low_bit_grey_is_scaled_and_a_colour_key_is_alpha() {
        let bytes = png((3, 1, 1, 0, 0), &[(b"tRNS", &[0, 0])], &[0, 0b1010_0000]);
        let file = ImageFile::read(&bytes).expect("reads");
        let (_, colour, alpha) = samples(&file);
        assert_eq!((file.colour, colour), (Colour::Gray, vec![255, 0, 255]));
        assert_eq!(alpha, Some(vec![255, 0, 255]));
    }

    #[test]
    fn an_opaque_alpha_channel_is_no_alpha_and_sixteen_bits_stay_sixteen() {
        let bytes = png((1, 1, 16, 4, 0), &[], &[0, 0x12, 0x34, 0xFF, 0xFF]);
        let file = ImageFile::read(&bytes).expect("reads");
        assert_eq!(samples(&file), (16, vec![0x12, 0x34], None));
    }

    #[test]
    fn an_interlaced_picture_is_the_picture_it_would_be_otherwise() {
        let rows = [0, 1, 0, 3, 0, 7, 9, 0, 2, 0, 8, 0, 4, 5, 6];
        let bytes = png((3, 3, 8, 0, 1), &[], &rows);
        let file = ImageFile::read(&bytes).expect("reads");
        assert_eq!(samples(&file).1, (1..=9).collect::<Vec<u8>>());
    }

    #[test]
    fn damage_is_refused() {
        let mut bytes = png((1, 1, 8, 0, 0), &[], &[0, 7]);
        let checksum_of_header = 8 + 8 + 13;
        bytes[checksum_of_header] ^= 1;
        assert!(ImageFile::read(&bytes).is_err(), "a wrong checksum");
        let short = png((2, 1, 8, 0, 0), &[], &[0, 7]);
        assert!(ImageFile::read(&short).is_err(), "too few samples");
        let unknown = png((1, 1, 8, 0, 0), &[(b"ZZZZ", b"")], &[0, 7]);
        assert!(
            ImageFile::read(&unknown).is_err(),
            "an unknown critical chunk"
        );
        assert!(
            ImageFile::read(b"GIF89a").is_err(),
            "not a picture this reads"
        );
    }

    pub(crate) fn jpeg_header(marker: u8, components: u8, orientation: Option<u8>) -> Vec<u8> {
        let mut out = vec![0xFF, 0xD8];
        if let Some(orientation) = orientation {
            let mut tiff = b"Exif\0\0II*\0".to_vec();
            tiff.extend_from_slice(&8_u32.to_le_bytes());
            tiff.extend_from_slice(&1_u16.to_le_bytes());
            tiff.extend_from_slice(&0x0112_u16.to_le_bytes());
            tiff.extend_from_slice(&3_u16.to_le_bytes());
            tiff.extend_from_slice(&1_u32.to_le_bytes());
            tiff.extend_from_slice(&[orientation, 0, 0, 0]);
            out.extend_from_slice(&[0xFF, 0xE1]);
            out.extend_from_slice(&u16::try_from(tiff.len() + 2).expect("len").to_be_bytes());
            out.extend_from_slice(&tiff);
        }
        out.extend_from_slice(&[
            0xFF,
            marker,
            0,
            8 + 3 * components,
            8,
            0,
            20,
            0,
            30,
            components,
        ]);
        for component in 0..components {
            out.extend_from_slice(&[component + 1, 0x11, 0]);
        }
        out.extend_from_slice(&[0xFF, 0xD9]);
        out
    }

    #[test]
    fn a_picture_that_states_its_resolution_is_read_at_it() {
        let physical = |body: &[u8]| {
            let rows = [vec![0, 1, 2, 3], vec![0, 4, 5, 6]].concat();
            let bytes = png((1, 2, 8, 2, 0), &[(b"pHYs", body)], &rows);
            let file = ImageFile::read(&bytes).expect("reads");
            (file.density, file.dpi())
        };
        let mut chunk = 11_811_u32.to_be_bytes().to_vec();
        chunk.extend_from_slice(&11_811_u32.to_be_bytes());
        chunk.push(1);
        let (density, dpi) = physical(&chunk);
        assert_eq!(density, Some((11_811, 11_811)));
        let (across, down) = dpi.expect("dots an inch");
        assert!((across - 300.0).abs() < 0.05 && (down - 300.0).abs() < 0.05);
        let mut ratio = chunk.clone();
        ratio[8] = 0;
        assert_eq!(physical(&ratio), (None, None));
        let rows = [vec![0, 1, 2, 3], vec![0, 4, 5, 6]].concat();
        let quiet = png((1, 2, 8, 2, 0), &[], &rows);
        let silent = ImageFile::read(&quiet).expect("reads");
        assert_eq!((silent.density, silent.dpi()), (None, None));
        let jfif = |units: u8, across: u16, down: u16| {
            let mut body = b"JFIF\0".to_vec();
            body.extend_from_slice(&[1, 2, units]);
            body.extend_from_slice(&across.to_be_bytes());
            body.extend_from_slice(&down.to_be_bytes());
            body.push(0);
            body.push(0);
            let mut out = vec![0xFF, 0xD8, 0xFF, 0xE0];
            out.extend_from_slice(&u16::try_from(body.len() + 2).expect("len").to_be_bytes());
            out.extend_from_slice(&body);
            out.extend_from_slice(&jpeg_header(0xC0, 3, None)[2..]);
            let file = ImageFile::read(&out).expect("reads");
            file.dpi()
                .map(|(across, down)| (across.round(), down.round()))
        };
        assert_eq!(jfif(1, 300, 300), Some((300.0, 300.0)));
        assert_eq!(jfif(2, 118, 118), Some((300.0, 300.0)));
        assert_eq!(jfif(0, 1, 1), None);
        assert_eq!(jfif(1, 0, 300), None);
        assert_eq!(
            ImageFile::read(&jpeg_header(0xC0, 3, None))
                .expect("reads")
                .dpi(),
            None
        );
    }

    #[test]
    fn a_jpeg_header_says_its_size_colour_and_orientation() {
        let bytes = jpeg_header(0xC2, 3, Some(6));
        let file = ImageFile::read(&bytes).expect("reads");
        assert_eq!((file.width, file.height), (30, 20));
        assert_eq!((file.colour, file.orientation), (Colour::Rgb, 6));
        assert_eq!(file.upright(), (20, 30));
        assert_eq!(file.stored, Stored::Jpeg(&bytes));
        assert_eq!(
            ImageFile::read(&jpeg_header(0xC0, 4, None))
                .expect("reads")
                .colour,
            Colour::Cmyk
        );
        assert!(
            ImageFile::read(&jpeg_header(0xC9, 3, None)).is_err(),
            "arithmetic"
        );
        assert!(
            ImageFile::read(&jpeg_header(0xC0, 2, None)).is_err(),
            "two components"
        );
    }

    #[test]
    fn each_orientation_turns_the_stored_corners_where_exif_says_they_belong() {
        let wanted = [
            (0.0, 1.0),
            (1.0, 1.0),
            (1.0, 0.0),
            (0.0, 0.0),
            (0.0, 1.0),
            (1.0, 1.0),
            (1.0, 0.0),
            (0.0, 0.0),
        ];
        let right = [
            (1.0, 1.0),
            (0.0, 1.0),
            (0.0, 0.0),
            (1.0, 0.0),
            (0.0, 0.0),
            (1.0, 0.0),
            (1.0, 1.0),
            (0.0, 1.0),
        ];
        for orientation in 1..=8_u8 {
            let bytes = jpeg_header(0xC0, 1, Some(orientation));
            let m = ImageFile::read(&bytes).expect("reads").orientation_matrix();
            let at = |u: f64, v: f64| (m[0] * u + m[2] * v + m[4], m[1] * u + m[3] * v + m[5]);
            let index = usize::from(orientation - 1);
            assert_eq!(
                at(0.0, 1.0),
                wanted[index],
                "top-left, orientation {orientation}"
            );
            assert_eq!(
                at(1.0, 1.0),
                right[index],
                "top-right, orientation {orientation}"
            );
        }
    }

    #[test]
    fn shrinking_averages_the_pixels_that_fall_into_one() {
        let grey = |value: u8| [value, value, value, 255];
        let rgba: Vec<u8> = [grey(0), grey(60), grey(120), grey(180)].concat();
        let small = shrunk(
            Thumbnail {
                width: 2,
                height: 2,
                rgba,
            },
            1,
        );
        assert_eq!(small.width, 1);
        assert_eq!(small.height, 1);
        assert_eq!(small.rgba, vec![90, 90, 90, 255]);
    }

    #[test]
    fn shrinking_never_makes_a_picture_larger() {
        let picture = Thumbnail {
            width: 2,
            height: 1,
            rgba: vec![1, 2, 3, 4, 5, 6, 7, 8],
        };
        assert_eq!(shrunk(picture.clone(), 64), picture);
    }

    #[test]
    fn samples_of_each_kind_become_rgba() {
        assert_eq!(
            to_rgba((2, 1), 1, &[10, 200], None),
            Some(vec![10, 10, 10, 255, 200, 200, 200, 255])
        );
        assert_eq!(
            to_rgba((1, 1), 3, &[1, 2, 3], Some(&[128])),
            Some(vec![1, 2, 3, 128])
        );
        assert_eq!(
            to_rgba((1, 1), 4, &[255, 255, 255, 255], None),
            Some(vec![255, 255, 255, 255])
        );
        assert_eq!(to_rgba((2, 2), 3, &[1, 2, 3], None), None);
    }

    #[test]
    fn a_turned_picture_is_shown_the_way_up_it_was_taken() {
        let picture = Thumbnail {
            width: 2,
            height: 1,
            rgba: vec![1, 1, 1, 255, 2, 2, 2, 255],
        };
        let upright = turned_upright(picture.clone(), 6);
        assert_eq!((upright.width, upright.height), (1, 2));
        assert_eq!(upright.rgba, vec![1, 1, 1, 255, 2, 2, 2, 255]);
        assert_eq!(turned_upright(picture.clone(), 1), picture);
    }

    #[test]
    fn a_png_gives_a_thumbnail() {
        let rows = [0, 0, 60, 0, 120, 180];
        let bytes = png((2, 2, 8, 0, 0), &[], &rows);
        let file = ImageFile::read(&bytes).expect("the PNG reads");
        let small = file.thumbnail(1).expect("it makes a thumbnail");
        assert_eq!((small.width, small.height), (1, 1));
        assert_eq!(small.rgba, vec![90, 90, 90, 255]);
    }
}
