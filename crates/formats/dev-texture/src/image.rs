/// An RGBA8 raster, row 0 on top.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, rows top to bottom.
    pub rgba: Vec<u8>,
}

impl Image {
    pub(crate) fn filled(width: u32, height: u32, colour: [u8; 3]) -> Image {
        let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
        for _ in 0..width as usize * height as usize {
            rgba.extend_from_slice(&[colour[0], colour[1], colour[2], 255]);
        }
        Image {
            width,
            height,
            rgba,
        }
    }

    pub(crate) fn put(&mut self, x: u32, y: u32, colour: [u8; 3]) {
        let at = (y as usize * self.width as usize + x as usize) * 4;
        self.rgba[at..at + 4].copy_from_slice(&[colour[0], colour[1], colour[2], 255]);
    }

    /// The texel at (`x`, `y`) as RGBA.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let at = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.rgba[at],
            self.rgba[at + 1],
            self.rgba[at + 2],
            self.rgba[at + 3],
        ]
    }
}
