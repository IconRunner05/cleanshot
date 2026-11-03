/// One monitor's pixels as Windows copies them: top-down rows of 8-bit blue, green and red,
/// then a fourth byte that is never read, as a capture of the screen is opaque.
#[derive(Debug)]
pub struct BgraFrame {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl BgraFrame {
    /// `pixels` as a frame `width` x `height`, or `None` unless both are at least 1 and
    /// `pixels` holds exactly `width * height * 4` bytes.
    #[must_use]
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Option<Self> {
        let expected = u64::from(width) * u64::from(height) * 4;
        (width > 0 && height > 0 && u64::try_from(pixels.len()) == Ok(expected)).then_some(Self {
            width,
            height,
            pixels,
        })
    }

    /// Width in physical pixels, at least 1.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in physical pixels, at least 1.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The pixels, `width * height * 4` bytes in blue, green, red order.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub(super) fn pixels_mut(&mut self) -> &mut [u8] {
        &mut self.pixels
    }
}

/// Writes the BGRA pixels of `bgra` into `rgba` as opaque RGBA; both are the same length.
pub(super) fn to_rgba(bgra: &[u8], rgba: &mut [u8]) {
    for (from, to) in bgra
        .as_chunks::<4>()
        .0
        .iter()
        .zip(rgba.as_chunks_mut::<4>().0)
    {
        *to = [from[2], from[1], from[0], u8::MAX];
    }
}
