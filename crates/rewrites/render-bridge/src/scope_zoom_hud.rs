//! Whether the HUD screen pass is drawing a converted scope-zoom profile
//! (an `egor` tag in the `tags::scope_zoom` format, from a Halo 1 map),
//! and the blur such a call asks for (docs/graphics/native-post.md, "The
//! Halo 1 scope"). The Halo 1 scope-zoom hook fills the call's values
//! (`h1_runtime::h1_scope_zoom`); the native post plans the blur with
//! [`zoom_blur`].

use engine::glow::BlurArgs;
use engine::handle::DatumHandle;
use engine::memory::GuestMemory;
use engine::tags::{entry_layout, tag_entry_checked};
use tags::scope_zoom::{MAGIC, Profile, WORDS};

/// The return address of the HUD screen pass's call to the screen effect.
pub const HUD_SCREEN_RETURN: u32 = 0x0022_bbc0;

/// A tag's definition address and size, when the datum is a tag of
/// `group` with a definition.
pub fn tag_def(mem: &dyn GuestMemory, tag: u32, group: [u8; 4]) -> Option<(u32, u32)> {
    let entry = tag_entry_checked(mem, DatumHandle::from_raw(tag)?).ok()??;
    if mem.u32(entry + entry_layout::GROUP).ok()? != u32::from_be_bytes(group) {
        return None;
    }
    let def = mem.u32(entry + entry_layout::ADDRESS).ok()?;
    let size = mem.u32(entry + entry_layout::SIZE).ok()?;
    (def != 0).then_some((def, size))
}

/// The scope-zoom profile an `egor` tag holds, if it holds one.
pub fn profile(mem: &dyn GuestMemory, datum: u32) -> Option<Profile> {
    let (def, size) = tag_def(mem, datum, *b"egor")?;
    if size < (WORDS * 4) as u32 || mem.u32(def).ok()? != MAGIC {
        return None;
    }
    let mut bytes = [0; WORDS * 4];
    mem.read(def, &mut bytes).ok()?;
    Profile::decode(std::array::from_fn(|i| {
        u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap())
    }))
}

/// The profile of the screen effect the HUD screen pass is calling, if
/// the call is that pass's (`esp` holds [`HUD_SCREEN_RETURN`]).
pub fn hud_profile(mem: &dyn GuestMemory, datum: u32, esp: u32) -> Option<Profile> {
    if mem.u32(esp).ok()? != HUD_SCREEN_RETURN {
        return None;
    }
    profile(mem, datum)
}

/// The return address of `0x0002d140`'s call of the blur `0x000211a0`.
pub const CONVOLUTION_RETURN: u32 = 0x0002_da9b;

/// The blur's step count and first scale (f32 bits) for a screen effect
/// called with `values`, from the tag's `steps` and `scale`. `None` when
/// the values are not a scope's (the fourth is not [`MAGIC`]). The third
/// value is the radius: none or below, zero steps (`0x000211a0` returns
/// its source); else half the radius as the scale, the convolution being
/// drawn at half resolution.
pub fn blur_words(values: [u32; 4], steps: u32, scale: u32) -> Option<(u32, u32)> {
    if values[3] != MAGIC {
        return None;
    }
    let radius = f32::from_bits(values[2]);
    Some(if radius <= 0.0 { (0, scale) } else { (steps, (radius * 0.5).to_bits()) })
}

/// [`blur_words`] on a planned blur's arguments
/// (`engine::screen_effect::plan_with`).
pub fn zoom_blur(values: [u32; 4], args: &mut BlurArgs) {
    if let Some((steps, scale)) = blur_words(values, args.steps, args.scale) {
        (args.steps, args.scale) = (steps, scale);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> BlurArgs {
        BlurArgs {
            source: 1,
            steps: 1.5f32.to_bits(),
            scale: 1.0f32.to_bits(),
            growth: 0.25f32.to_bits(),
            dest: 0x19,
            other: 0x1a,
            last: u32::MAX,
        }
    }

    #[test]
    fn the_scope_radius_sets_the_blur() {
        let mut a = args();
        zoom_blur([0, 0, 6.0f32.to_bits(), MAGIC], &mut a);
        assert_eq!((a.steps, a.scale), (1.5f32.to_bits(), 3.0f32.to_bits()), "half the radius");
        let mut a = args();
        zoom_blur([0, 0, 0, MAGIC], &mut a);
        assert_eq!((a.steps, a.scale), (0, 1.0f32.to_bits()), "no radius: no steps");
        zoom_blur([0, 0, (-1.0f32).to_bits(), MAGIC], &mut a);
        assert_eq!(a.steps, 0);
    }

    #[test]
    fn other_effects_keep_the_tag_blur() {
        let mut a = args();
        zoom_blur([0, 0, 6.0f32.to_bits(), 0], &mut a);
        assert_eq!(a, args());
        assert_eq!(blur_words([0; 4], 7, 8), None);
    }
}
