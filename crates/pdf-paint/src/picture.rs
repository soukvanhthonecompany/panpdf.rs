use std::io::Cursor;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub channels: usize,
    pub samples: Vec<u8>,
}

const MOST_BYTES: usize = 100 * 1024 * 1024;

#[must_use]
pub fn jpeg_pixels(bytes: &[u8]) -> Option<Pixels> {
    let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(bytes));
    decoder.set_max_decoding_buffer_size(MOST_BYTES);
    let samples = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| decoder.decode()))
        .ok()?
        .ok()?;
    let info = decoder.info()?;
    let channels = match info.pixel_format {
        jpeg_decoder::PixelFormat::L8 => 1,
        jpeg_decoder::PixelFormat::RGB24 => 3,
        jpeg_decoder::PixelFormat::CMYK32 => 4,
        jpeg_decoder::PixelFormat::L16 => return None,
    };
    let (width, height) = (u32::from(info.width), u32::from(info.height));
    if samples.len() < (width as usize) * (height as usize) * channels {
        return None;
    }
    let samples = if channels == 4 && crate::image::jpeg_has_adobe_app14(bytes) {
        samples.iter().map(|byte| 255 - byte).collect()
    } else {
        samples
    };
    Some(Pixels {
        width,
        height,
        channels,
        samples,
    })
}
