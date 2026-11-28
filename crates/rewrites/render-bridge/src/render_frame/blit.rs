//! The present's back-buffer blit `0x00013d80` (docs/graphics/
//! retire-d3d-display.md, "The final display"), read in Rust so a
//! published frame draws it natively.
//!
//! `0x00013d50` (three callers in `0x002134ef..0x0021359d`, the disc-error
//! screen) sets the image `[0x005093c8]`, its rectangle `0x005093c0` and
//! its scale `[0x005093cc]` (`0x55` from every caller). With an image set,
//! the present's `0x00013d80` (**Code** `0x00013d80..0x00013f0e`) writes it
//! into the back buffer with the CPU:
//!
//! | Guest | What |
//! |---|---|
//! | `[0x005093c8]` 0 | nothing |
//! | the words at `0x005093c0`: top, left, bottom, right | nothing unless `top >= 0`, `left >= 0`, `right <= 640`, `bottom <= 480` |
//! | the back buffer `[device + 0x1a14]`, its size against `width * height * 4`, then locked | nothing when the size differs or the lock fails |
//! | each byte of the image, row-major, `right - left` to a row, `bottom - top` rows | `byte * scale` clamped to 0..255, written to all four bytes of the pixel at `(left + x, top + y)` |
//!
//! [`read`] is the table up to the back buffer: the image as BGRA texels
//! at the rectangle. **S**: that the back buffer the guest checks is
//! always the linear 640 x 480 one, so the size check and the lock pass
//! (the device's back buffer is made at the display's guest size).

use guest_layout::memory::{GuestMemory, MemFault};

/// The guest words `0x00013d50` sets and `0x00013d80` reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Addrs {
    /// The image, one byte a pixel; 0 is none.
    pub image: u32,
    /// Top and left (16 bits each), then bottom and right.
    pub rect: u32,
    /// What each byte is multiplied by.
    pub scale: u32,
}

impl Addrs {
    pub const HALO2: Addrs = Addrs { image: 0x0050_93c8, rect: 0x0050_93c0, scale: 0x0050_93cc };
}

/// The back buffer's size the rectangle must fit, in guest pixels.
pub const SCREEN: [i32; 2] = [640, 480];

/// The id the blit's picture goes by in the overlay's uploads: the
/// guest's entry, which no movie handle is.
pub const PICTURE: u32 = 0x0001_3d80;

/// What `0x00013d80` writes: `size` texels at `at` (left, top), four
/// bytes each in B, G, R, A order, row-major.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blit {
    pub at: [u32; 2],
    pub size: [u32; 2],
    pub bgra: Vec<u8>,
}

impl Blit {
    /// A number that moves when the picture does (FNV-1a over its size
    /// and texels): the upload writes the texture again when it moves.
    pub fn frame(&self) -> u32 {
        let mut h: u32 = 0x811c_9dc5;
        let size = [self.size[0].to_le_bytes(), self.size[1].to_le_bytes()];
        for b in size.iter().flatten().chain(self.bgra.iter()) {
            h = (h ^ u32::from(*b)).wrapping_mul(0x0100_0193);
        }
        h
    }
}

fn halves(w: u32) -> (i32, i32) {
    (i32::from(w as u16 as i16), i32::from((w >> 16) as u16 as i16))
}

/// What the blit would write this present; `None` when it writes
/// nothing (no image, or a rectangle the guest's checks pass over).
pub fn read(a: &Addrs, g: &dyn GuestMemory) -> Result<Option<Blit>, MemFault> {
    let image = g.u32(a.image)?;
    if image == 0 {
        return Ok(None);
    }
    let (top, left) = halves(g.u32(a.rect)?);
    let (bottom, right) = halves(g.u32(a.rect + 4)?);
    if top < 0 || left < 0 || right > SCREEN[0] || bottom > SCREEN[1] {
        return Ok(None);
    }
    let (width, rows) = (right - left, bottom - top);
    if width <= 0 || rows <= 0 {
        return Ok(None);
    }
    let scale = g.u32(a.scale)? as i32;
    let mut bytes = vec![0u8; (width * rows) as usize];
    g.read(image, &mut bytes)?;
    let mut bgra = Vec::with_capacity(bytes.len() * 4);
    for b in bytes {
        let v = i32::from(b).wrapping_mul(scale).clamp(0, 255) as u8;
        bgra.extend_from_slice(&[v; 4]);
    }
    Ok(Some(Blit { at: [left as u32, top as u32], size: [width as u32, rows as u32], bgra }))
}

/// The blit as one overlay item over the 640 x 480 canvas: the picture
/// ([`PICTURE`]) at its rectangle, point-sampled, written with no blend,
/// as the guest's CPU writes replace the pixels.
#[cfg(feature = "native-render")]
pub fn overlay(blit: &Blit) -> frame_scene::Overlay {
    use frame_scene::{
        ClipRect, Combiner, Overlay, OverlayBlend, OverlayMaterial, OverlayMovie, OverlayTexture, OverlayVertex,
        Program, Sampler,
    };
    let canvas = [SCREEN[0] as u32, SCREEN[1] as u32];
    let [x0, y0] = blit.at.map(|v| v as f32);
    let [x1, y1] = [x0 + blit.size[0] as f32, y0 + blit.size[1] as f32];
    let corner = |x, y, u, v| OverlayVertex { position: [x, y], uv: [u, v], color: [1.0; 4] };
    let corners = [corner(x0, y0, 0.0, 0.0), corner(x1, y0, 1.0, 0.0), corner(x1, y1, 1.0, 1.0), corner(x0, y1, 0.0, 1.0)];
    let material = OverlayMaterial {
        program: Program::Fixed(Combiner::Texture),
        texture: OverlayTexture::None,
        sampler: Sampler { wrap: false, point: true },
        blend: OverlayBlend::Off,
    };
    let mut o = Overlay::new(canvas);
    // The rectangle is inside the canvas (`read`'s checks), so the push
    // cannot refuse it.
    let _ = o.push_movie_frame(material, ClipRect::full(canvas), corners, OverlayMovie { movie: PICTURE, frame: blit.frame() });
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    const A: Addrs = Addrs::HALO2;
    const IMAGE: u32 = 0x0060_0000;

    /// The words `0x00013d50` leaves for `rect` (top, left, bottom, right).
    fn set(g: &mut FakeGuest, image: u32, rect: [i16; 4], scale: u32) {
        let pair = |a: i16, b: i16| u32::from(a as u16) | u32::from(b as u16) << 16;
        g.put_u32(A.image, image).put_u32(A.rect, pair(rect[0], rect[1])).put_u32(A.rect + 4, pair(rect[2], rect[3]));
        g.put_u32(A.scale, scale);
    }

    fn guest() -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(0x0050_9000, 0x1000).map(IMAGE, 0x5_0000);
        g
    }

    #[test]
    fn each_byte_is_scaled_clamped_and_written_to_all_four_channels_at_the_rectangle() {
        let mut g = guest();
        // Two rows of three: 0..3 at the disc-error screen's 0x55, and 4
        // past white.
        g.put(IMAGE, &[0, 1, 2, 3, 4, 255]);
        set(&mut g, IMAGE, [10, 20, 12, 23], 0x55);
        let b = read(&A, &g).unwrap().expect("an image in a rectangle on screen");
        assert_eq!((b.at, b.size), ([20, 10], [3, 2]));
        let grey: Vec<u8> = b.bgra.chunks(4).map(|p| {
            assert!(p.iter().all(|&c| c == p[0]), "every byte of the pixel is the grey");
            p[0]
        }).collect();
        assert_eq!(grey, [0, 0x55, 0xaa, 0xff, 0xff, 0xff]);
    }

    #[test]
    fn a_negative_product_is_black() {
        let mut g = guest();
        g.put(IMAGE, &[2]);
        set(&mut g, IMAGE, [0, 0, 1, 1], (-3i32) as u32);
        assert_eq!(read(&A, &g).unwrap().unwrap().bgra, [0; 4]);
    }

    #[test]
    fn what_the_guests_checks_pass_over_writes_nothing() {
        let mut g = guest();
        g.put(IMAGE, &[9; 16]);
        set(&mut g, 0, [0, 0, 4, 4], 1);
        assert_eq!(read(&A, &g).unwrap(), None, "no image");
        for rect in [[-1, 0, 4, 4], [0, -1, 4, 4], [0, 0, 481, 4], [0, 0, 4, 641], [4, 0, 4, 4], [0, 4, 4, 2]] {
            set(&mut g, IMAGE, rect, 1);
            assert_eq!(read(&A, &g).unwrap(), None, "{rect:?}");
        }
        set(&mut g, IMAGE, [476, 636, 480, 640], 1);
        assert!(read(&A, &g).unwrap().is_some(), "the bottom right corner is on screen");
    }

    #[test]
    fn the_frame_moves_with_the_picture() {
        let mut g = guest();
        g.put(IMAGE, &[1, 2]);
        set(&mut g, IMAGE, [0, 0, 1, 2], 1);
        let a = read(&A, &g).unwrap().unwrap();
        assert_eq!(a.frame(), read(&A, &g).unwrap().unwrap().frame());
        g.put(IMAGE, &[1, 3]);
        assert_ne!(a.frame(), read(&A, &g).unwrap().unwrap().frame());
    }

    #[cfg(feature = "native-render")]
    #[test]
    fn the_overlay_is_one_opaque_picture_at_the_rectangle() {
        let b = Blit { at: [100, 40], size: [8, 4], bgra: vec![0x55; 8 * 4 * 4] };
        let o = overlay(&b);
        assert_eq!(o.size, [640, 480]);
        assert_eq!(o.items.len(), 1);
        let item = &o.items[0];
        assert_eq!(o.movie_of(item).map(|m| (m.movie, m.frame)), Some((PICTURE, b.frame())));
        assert_eq!(item.material.blend, frame_scene::OverlayBlend::Off);
        assert_eq!(item.material.texture, frame_scene::OverlayTexture::Movie { movie: PICTURE });
    }
}
