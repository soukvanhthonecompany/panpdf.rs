#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Jbig2Error {
    Malformed,
    SizeDisagrees {
        declared: (u32, u32),
        decoded: (u32, u32),
    },
    TooLarge,
}

impl std::fmt::Display for Jbig2Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed => formatter.write_str("malformed JBIG2 image"),
            Self::SizeDisagrees { declared, decoded } => write!(
                formatter,
                "JBIG2 image is {}x{} and its dictionary declares {}x{}",
                decoded.0, decoded.1, declared.0, declared.1
            ),
            Self::TooLarge => formatter.write_str("oversized JBIG2 image"),
        }
    }
}

struct Rows {
    samples: Vec<u8>,
    byte: u8,
    filled: u8,
    overflowed: bool,
    budget: usize,
}

impl Rows {
    fn new(width: u32, height: u32, budget: usize) -> Self {
        let row_bytes = (width as usize).div_ceil(8);
        Self {
            samples: Vec::with_capacity(row_bytes.saturating_mul(height as usize).min(budget)),
            byte: 0,
            filled: 0,
            overflowed: false,
            budget,
        }
    }

    fn push(&mut self, black: bool) {
        if self.overflowed {
            return;
        }
        self.byte = (self.byte << 1) | u8::from(!black);
        self.filled += 1;
        if self.filled == 8 {
            self.emit();
        }
    }

    fn emit(&mut self) {
        if self.samples.len() >= self.budget {
            self.overflowed = true;
            return;
        }
        self.samples.push(self.byte);
        self.byte = 0;
        self.filled = 0;
    }

    fn end_row(&mut self) {
        while self.filled != 0 {
            self.push(false);
        }
    }
}

impl hayro_jbig2::Decoder for Rows {
    fn push_pixel(&mut self, black: bool) {
        self.push(black);
    }

    fn push_pixel_chunk(&mut self, black: bool, chunk_count: u32) {
        for _ in 0..chunk_count * 8 {
            self.push(black);
        }
    }

    fn next_line(&mut self) {
        self.end_row();
    }
}

pub fn decode(
    data: &[u8],
    globals: Option<&[u8]>,
    width: u32,
    height: u32,
    max_bytes: usize,
) -> Result<Vec<u8>, Jbig2Error> {
    let expected = (width as usize)
        .div_ceil(8)
        .checked_mul(height as usize)
        .ok_or(Jbig2Error::TooLarge)?;
    if expected > max_bytes {
        return Err(Jbig2Error::TooLarge);
    }
    let image =
        hayro_jbig2::Image::new_embedded(data, globals).map_err(|_| Jbig2Error::Malformed)?;
    if image.width() != width || image.height() != height {
        return Err(Jbig2Error::SizeDisagrees {
            declared: (width, height),
            decoded: (image.width(), image.height()),
        });
    }
    let mut rows = Rows::new(width, height, max_bytes);
    image.decode(&mut rows).map_err(|_| Jbig2Error::Malformed)?;
    rows.end_row();
    if rows.overflowed {
        return Err(Jbig2Error::TooLarge);
    }
    rows.samples.resize(expected, 0xff);
    Ok(rows.samples)
}

#[cfg(test)]
mod tests {
    use super::{Jbig2Error, decode};

    fn one_region(width: u32, height: u32, mmr_data: &[u8]) -> Vec<u8> {
        let mut page = Vec::new();
        page.extend_from_slice(&1_u32.to_be_bytes());
        page.push(48);
        page.push(0x00);
        page.push(1);
        page.extend_from_slice(&19_u32.to_be_bytes());
        page.extend_from_slice(&width.to_be_bytes());
        page.extend_from_slice(&height.to_be_bytes());
        page.extend_from_slice(&0_u32.to_be_bytes());
        page.extend_from_slice(&0_u32.to_be_bytes());
        page.push(0);
        page.extend_from_slice(&0_u16.to_be_bytes());

        let mut region = Vec::new();
        region.extend_from_slice(&2_u32.to_be_bytes());
        region.push(38);
        region.push(0x00);
        region.push(1);
        let mut data = Vec::new();
        data.extend_from_slice(&width.to_be_bytes());
        data.extend_from_slice(&height.to_be_bytes());
        data.extend_from_slice(&0_u32.to_be_bytes());
        data.extend_from_slice(&0_u32.to_be_bytes());
        data.push(0);
        data.push(0x01);
        data.extend_from_slice(mmr_data);
        region.extend_from_slice(&u32::try_from(data.len()).expect("small").to_be_bytes());
        region.extend_from_slice(&data);

        [page, region].concat()
    }

    #[test]
    fn a_decoded_jbig2_image_is_white_where_the_codec_says_white() {
        let mmr = [0b1100_0000];
        let file = one_region(8, 2, &mmr);
        let samples = decode(&file, None, 8, 2, 1 << 20).expect("a decodable image");
        assert_eq!(
            samples,
            vec![0xff, 0xff],
            "eight white pixels a row, and white is 1 in /DeviceGray"
        );
    }

    #[test]
    fn a_jbig2_image_is_refused_rather_than_guessed_at() {
        assert_eq!(
            decode(b"not a jbig2 file at all", None, 8, 2, 1 << 20),
            Err(Jbig2Error::Malformed)
        );
        let file = one_region(8, 2, &[0b1001_1100, 0b1100_0000]);
        assert_eq!(
            decode(&file, None, 16, 2, 1 << 20),
            Err(Jbig2Error::SizeDisagrees {
                declared: (16, 2),
                decoded: (8, 2)
            }),
            "the dictionary decides what is drawn, so a disagreement is refused"
        );
        assert_eq!(
            decode(&file, None, 8, 2, 1),
            Err(Jbig2Error::TooLarge),
            "and the caller's budget is a budget"
        );
    }
}
