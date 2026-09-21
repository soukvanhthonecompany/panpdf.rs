use crate::image_file::crc32;

#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is checked to be finite, positive and under u32::MAX first"
)]
pub fn per_metre(dpi: f64) -> u32 {
    let metres = dpi * 10_000.0 / 254.0;
    if metres.is_finite() && metres >= 1.0 {
        metres.min(f64::from(u32::MAX)).round() as u32
    } else {
        0
    }
}

pub fn write(
    (width, height): (u32, u32),
    pixels: &[u8],
    dots_per_metre: Option<(u32, u32)>,
) -> Result<Vec<u8>, &'static str> {
    if width == 0 || height == 0 {
        return Err("a picture with no pixels cannot be written");
    }
    let row = usize::try_from(width)
        .ok()
        .and_then(|width| width.checked_mul(3))
        .ok_or("the picture is wider than this machine can write")?;
    let rows = usize::try_from(height).map_err(|_| "the picture is taller than this can write")?;
    if pixels.len()
        != row
            .checked_mul(rows)
            .ok_or("the picture is too large to write")?
    {
        return Err("the pixels given are not three bytes for every pixel");
    }
    let mut out = Vec::from(*b"\x89PNG\r\n\x1a\n");
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, *b"IHDR", &header);
    if let Some((across, down)) = dots_per_metre.filter(|(x, y)| *x > 0 && *y > 0) {
        let mut physical = Vec::with_capacity(9);
        physical.extend_from_slice(&across.to_be_bytes());
        physical.extend_from_slice(&down.to_be_bytes());
        physical.push(1);
        chunk(&mut out, *b"pHYs", &physical);
    }
    chunk(
        &mut out,
        *b"IDAT",
        &pdf_syntax::deflate_zlib(&filtered(pixels, row)),
    );
    chunk(&mut out, *b"IEND", &[]);
    Ok(out)
}

fn filtered(pixels: &[u8], row: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels.len() + pixels.len() / row.max(1) + 1);
    let mut previous = vec![0_u8; row];
    let mut candidate = vec![0_u8; row];
    for line in pixels.chunks_exact(row) {
        let mut best: Option<(u64, u8, Vec<u8>)> = None;
        for filter in 0..=4_u8 {
            apply(filter, line, &previous, &mut candidate);
            let cost = candidate
                .iter()
                .map(|byte| u64::from(i8::from_ne_bytes([*byte]).unsigned_abs()))
                .sum::<u64>();
            if best.as_ref().is_none_or(|(least, _, _)| cost < *least) {
                best = Some((cost, filter, candidate.clone()));
            }
        }
        if let Some((_, filter, bytes)) = best {
            out.push(filter);
            out.extend_from_slice(&bytes);
        }
        previous.copy_from_slice(line);
    }
    out
}

fn apply(filter: u8, line: &[u8], previous: &[u8], out: &mut [u8]) {
    const LEFT: usize = 3;
    for at in 0..line.len() {
        let a = if at >= LEFT { line[at - LEFT] } else { 0 };
        let b = previous[at];
        let c = if at >= LEFT { previous[at - LEFT] } else { 0 };
        out[at] = match filter {
            1 => line[at].wrapping_sub(a),
            2 => line[at].wrapping_sub(b),
            3 => {
                let mean = u16::midpoint(u16::from(a), u16::from(b));
                line[at].wrapping_sub(u8::try_from(mean).unwrap_or(0))
            }
            4 => line[at].wrapping_sub(paeth(a, b, c)),
            _ => line[at],
        };
    }
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (left, above, corner) = (i16::from(a), i16::from(b), i16::from(c));
    let guess = left + above - corner;
    let (da, db, dc) = (
        (guess - left).abs(),
        (guess - above).abs(),
        (guess - corner).abs(),
    );
    if da <= db && da <= dc {
        a
    } else if db <= dc {
        b
    } else {
        c
    }
}

fn chunk(out: &mut Vec<u8>, kind: [u8; 4], body: &[u8]) {
    let length = u32::try_from(body.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&length.to_be_bytes());
    let start = out.len();
    out.extend_from_slice(&kind);
    out.extend_from_slice(body);
    let checksum = crc32(&out[start..]);
    out.extend_from_slice(&checksum.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use crate::image_file::{Colour, ImageFile, Stored};

    use super::{filtered, per_metre, write};

    fn picture(width: u32, height: u32) -> Vec<u8> {
        let mut pixels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let across = u8::try_from(x % 256).unwrap_or(0);
                let down = u8::try_from(y % 256).unwrap_or(0);
                pixels.extend_from_slice(&[across, down, across ^ down]);
            }
        }
        pixels
    }

    #[test]
    fn what_was_written_reads_back_pixel_for_pixel() {
        let (width, height) = (7, 5);
        let pixels = picture(width, height);
        let file = write((width, height), &pixels, None).expect("written");
        let read = ImageFile::read(&file).expect("read back");
        assert_eq!((read.width, read.height), (width, height));
        assert_eq!(read.orientation, 1);
        let Stored::Samples {
            bits,
            colour,
            alpha,
        } = &read.stored
        else {
            panic!("a PNG is stored as samples");
        };
        assert_eq!(*bits, 8);
        assert_eq!(alpha, &None);
        assert_eq!(colour, &pixels);
        assert_eq!(read.colour, Colour::Rgb);
    }

    #[test]
    fn one_pixel_is_a_picture() {
        let file = write((1, 1), &[9, 8, 7], None).expect("written");
        let read = ImageFile::read(&file).expect("read back");
        assert_eq!((read.width, read.height), (1, 1));
        let Stored::Samples { colour, .. } = &read.stored else {
            panic!("samples");
        };
        assert_eq!(colour, &[9, 8, 7]);
    }

    #[test]
    fn a_larger_picture_reads_back_as_itself() {
        let (width, height) = (61, 47);
        let pixels = picture(width, height);
        let file = write((width, height), &pixels, Some((11_811, 11_811))).expect("written");
        assert!(file.len() < pixels.len(), "the rows were compressed");
        let read = ImageFile::read(&file).expect("read back");
        let Stored::Samples { colour, .. } = &read.stored else {
            panic!("samples");
        };
        assert_eq!(colour, &pixels);
    }

    #[test]
    fn the_resolution_is_written_as_dots_a_metre() {
        assert_eq!(per_metre(300.0), 11_811);
        assert_eq!(per_metre(72.0), 2835);
        assert_eq!(per_metre(0.0), 0);
        assert_eq!(per_metre(f64::NAN), 0);
        let file = write((2, 2), &picture(2, 2), Some((11_811, 11_811))).expect("written");
        let at = file
            .windows(4)
            .position(|four| four == b"pHYs")
            .expect("the picture says its resolution");
        assert_eq!(&file[at + 4..at + 12], &11_811_u32.to_be_bytes().repeat(2));
        assert_eq!(file[at + 12], 1, "the unit is the metre");
        let silent = write((2, 2), &picture(2, 2), None).expect("written");
        assert!(!silent.windows(4).any(|four| four == b"pHYs"));
        let ignored = write((2, 2), &picture(2, 2), Some((0, 11_811))).expect("written");
        assert!(!ignored.windows(4).any(|four| four == b"pHYs"));
    }

    #[test]
    fn each_row_takes_the_filter_that_costs_least() {
        let flat = [7_u8; 9].to_vec();
        let two_flat_rows = [flat.clone(), flat].concat();
        let written = filtered(&two_flat_rows, 9);
        assert_eq!(written.len(), 20);
        assert_eq!(written[0], 1, "left, which the first row repeats");
        assert_eq!(&written[1..10], &[7, 7, 7, 0, 0, 0, 0, 0, 0]);
        assert_eq!(written[10], 2, "above, which the second row repeats");
        assert_eq!(&written[11..20], &[0; 9]);
    }

    #[test]
    fn a_picture_whose_pixels_do_not_add_up_is_refused() {
        assert!(write((0, 4), &[], None).is_err());
        assert!(write((4, 0), &[], None).is_err());
        assert!(write((2, 2), &[0; 11], None).is_err());
        assert!(write((2, 2), &[0; 13], None).is_err());
        assert!(write((2, 2), &[0; 12], None).is_ok());
    }

    #[test]
    fn a_damaged_picture_is_caught_by_its_checksum() {
        let mut file = write((4, 4), &picture(4, 4), None).expect("written");
        let last = file.len() - 10;
        file[last] ^= 0xFF;
        assert!(ImageFile::read(&file).is_err());
    }
}
