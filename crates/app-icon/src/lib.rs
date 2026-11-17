//! The app icon where there is no bundle to carry it (art/app-icon).
//!
//! `h2 play` and `h2-editor` run as bare executables, so macOS has no
//! `.icns` to show; [`set_dock_icon`] gives the running app the plated Mac
//! icon instead. [`status_template`] is the menu bar glyph `h2 live
//! menubar` draws its state onto.

/// The Mac icon, plate drawn in: art/app-icon's macos/AppIcon-512.png.
pub const DOCK_PNG: &[u8] = include_bytes!("../assets/AppIcon-512.png");

/// The menu bar template glyph at 2x (36 px for an 18 pt item): black
/// with alpha, art/app-icon's menubar/StatusIconTemplate@2x.png.
pub const STATUS_TEMPLATE_PNG: &[u8] = include_bytes!("../assets/StatusIconTemplate@2x.png");

/// Straight RGBA8, row by row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Decodes an 8-bit RGBA PNG.
pub fn decode(bytes: &[u8]) -> Result<Rgba, String> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(
        png::Transformations::normalize_to_color8() | png::Transformations::ALPHA,
    );
    let mut r = dec.read_info().map_err(|e| e.to_string())?;
    let mut pixels = vec![0; r.output_buffer_size().ok_or("image too large")?];
    let info = r.next_frame(&mut pixels).map_err(|e| e.to_string())?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(format!(
            "{:?} {:?}, expected 8-bit RGBA",
            info.color_type, info.bit_depth
        ));
    }
    pixels.truncate(info.buffer_size());
    Ok(Rgba {
        width: info.width,
        height: info.height,
        pixels,
    })
}

/// The menu bar glyph, decoded.
pub fn status_template() -> Rgba {
    decode(STATUS_TEMPLATE_PNG).expect("the embedded status glyph decodes")
}

/// Sets the Dock (and app switcher) icon of this process to [`DOCK_PNG`].
/// Call it on the main thread once the event loop exists; elsewhere it
/// does nothing, as it does on other platforms.
pub fn set_dock_icon() {
    #[cfg(target_os = "macos")]
    mac::set_dock_icon();
}

#[cfg(target_os = "macos")]
mod mac {
    use objc2::{AllocAnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    pub fn set_dock_icon() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let data = NSData::with_bytes(super::DOCK_PNG);
        let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
            return;
        };
        // SAFETY: a valid image, on the main thread.
        unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dock_icon_is_a_512_rgba_square_with_a_clear_margin() {
        let i = decode(DOCK_PNG).unwrap();
        assert_eq!((i.width, i.height), (512, 512));
        // The plate is 824/1024 of the canvas: the corners are clear.
        assert_eq!(i.pixels[3], 0, "top-left corner");
        assert_eq!(i.pixels[i.pixels.len() - 1], 0, "bottom-right corner");
        let centre = ((256 * 512 + 256) * 4 + 3) as usize;
        assert_eq!(i.pixels[centre], 255, "an opaque plate");
    }

    #[test]
    fn the_status_glyph_is_an_18pt_template_at_2x() {
        let g = status_template();
        assert_eq!((g.width, g.height), (36, 36));
        assert_eq!(g.pixels.len(), 36 * 36 * 4);
        assert!(
            g.pixels.chunks(4).all(|p| p[..3] == [0, 0, 0]),
            "template: black only"
        );
        let ink = g.pixels.chunks(4).filter(|p| p[3] > 127).count();
        assert!((100..36 * 36 / 2).contains(&ink), "{ink} inked pixels");
    }
}
