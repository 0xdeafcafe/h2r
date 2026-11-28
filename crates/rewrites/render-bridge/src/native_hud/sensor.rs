//! The motion sensor in Rust (docs/graphics/native-hud-overlay.md, "The
//! motion sensor in Rust"): the HUD render's call of `HUD_SENSOR_DRAW`
//! (`0x001a2823`) is Rust's. [`plan`] works out the point the render gives
//! it and every draw the call asks for (`engine::hud_sensor`): the blips
//! of the ring's ten frames, each as the quad its draw `0x00254490` makes
//! (`engine::hud_sensor::blip_draw`), and the composite's centre and value.
//! The host draws them ([`super::Host::motion_sensor`]); the guest's body,
//! its blip draws and its composite never run.

use engine::hud_sensor::{self, Blip, BlipDraw, BlipRefusal};
use guest_layout::memory::{GuestMemory, MemFault};

/// The return address of `0x001a2197`'s call of `HUD_SENSOR_BLIP`: the
/// caller `blip_draw`'s frame names.
pub const BLIP_RETURN: u32 = 0x001a_2219;

/// Bytes under the hook's stack pointer where [`plan`] builds a blip's
/// call frame (free stack).
pub const SCRATCH: u32 = 0x100;
/// A blip's frame: the return address and the five arguments, then the
/// position the first argument points at.
const POSITION: u32 = 0x20;

/// One call of `HUD_SENSOR_DRAW` that draws.
#[derive(Clone, Debug, PartialEq)]
pub struct Draw {
    /// The local user.
    pub user: u32,
    /// The point the call was given.
    pub point: [i16; 2],
    /// The point plus the window's corner, as the composite's first
    /// argument holds it (floats as bits).
    pub centre: [u32; 2],
    /// The composite's second argument (a float as bits).
    pub value: u32,
    /// The blip draws, in order, each as its body makes it.
    pub blips: Vec<BlipDraw>,
}

/// Why the call is not worked out.
#[derive(Debug, PartialEq, Eq)]
pub enum Why {
    /// The point, the ring or the camera did not read, or an SSE
    /// exception is unmasked.
    Read(String),
    /// A blip's bitmap is missing or not resident: its body asks the
    /// texture cache.
    Bitmap,
    /// Guest memory refused the blip's frame.
    Frame,
}

impl std::fmt::Display for Why {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Why::Read(what) => write!(f, "{what}"),
            Why::Bitmap => write!(f, "a blip's bitmap is not resident"),
            Why::Frame => write!(f, "a blip's frame does not write"),
        }
    }
}

/// A blip's call frame as `0x001a2197` leaves it for `HUD_SENSOR_BLIP`
/// (`fn(position, shade, size, colour, pulse) ret 20`), written at `at`:
/// the return, the five arguments, and the position they point at. `at`
/// is the stack pointer to read the call with ([`hud_sensor::blip_draw`]).
fn put_blip(mem: &mut dyn GuestMemory, at: u32, blip: &Blip) -> Result<(), MemFault> {
    let position = at.wrapping_add(POSITION);
    for (i, word) in [BLIP_RETURN, position, blip.shade, blip.size, blip.colour, u32::from(blip.pulse)].into_iter().enumerate() {
        mem.set_u32(at.wrapping_add(4 * i as u32), word)?;
    }
    mem.set_u32(position, blip.position[0])?;
    mem.set_u32(position.wrapping_add(4), blip.position[1])
}

/// What `HUD_SENSOR_DRAW` draws for local user `user` given the point at
/// `at` (EAX), under the caller's MXCSR and x87 control word, with
/// `scratch` free stack for the blips' frames. `None`: the camera draws no
/// sensor.
pub fn plan(mem: &mut dyn GuestMemory, scratch: u32, mxcsr: u32, control: u16, user: u32, at: u32) -> Result<Option<Draw>, Why> {
    let unread = |_: MemFault| Why::Read("its point does not read".into());
    let point = [mem.u16(at).map_err(unread)? as i16, mem.u16(at.wrapping_add(2)).map_err(unread)? as i16];
    let Some(sensor) = hud_sensor::draw(&*mem, mxcsr, control, user, point).map_err(|e| Why::Read(format!("{e:?}")))? else {
        return Ok(None);
    };
    // Each blip's body starts with the MXCSR the one before it left.
    let mut mxcsr = mxcsr;
    let mut blips = Vec::with_capacity(sensor.blips.len());
    for blip in &sensor.blips {
        put_blip(mem, scratch, blip).map_err(|_| Why::Frame)?;
        let draw = hud_sensor::blip_draw(&*mem, scratch, mxcsr).map_err(|why| match why {
            BlipRefusal::Bitmap => Why::Bitmap,
            BlipRefusal::Unread => Why::Read("a blip's bitmap does not read".into()),
        })?;
        mxcsr = draw.mxcsr;
        blips.push(draw);
    }
    Ok(Some(Draw { user, point, centre: sensor.centre, value: sensor.value, blips }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    const FRAME: u32 = 0x0070_0000;

    fn blip() -> Blip {
        Blip { position: [10.0f32.to_bits(), 20.0f32.to_bits()], shade: 0.5f32.to_bits(), size: 4.0f32.to_bits(), colour: 0x0044_5320 + 12, pulse: true }
    }

    #[test]
    fn a_blips_frame_is_the_call_0x001a2197_makes() {
        let mut m = FakeGuest::new();
        m.put(FRAME, &[0u8; 0x40]);
        put_blip(&mut m, FRAME, &blip()).unwrap();
        let b = blip();
        // The return, then the five arguments in the order the point declares.
        assert_eq!(m.u32(FRAME).unwrap(), BLIP_RETURN);
        let position = m.u32(FRAME + 4).unwrap();
        assert_eq!([m.u32(FRAME + 8).unwrap(), m.u32(FRAME + 12).unwrap(), m.u32(FRAME + 16).unwrap()], [b.shade, b.size, b.colour]);
        assert_eq!(m.u32(FRAME + 20).unwrap() as u8, 1);
        // The first argument points at the position's two floats, clear of the frame.
        assert!(position >= FRAME + 24);
        assert_eq!([m.u32(position).unwrap(), m.u32(position + 4).unwrap()], b.position);
        // A blip that does not pulse passes 0.
        put_blip(&mut m, FRAME, &Blip { pulse: false, ..b }).unwrap();
        assert_eq!(m.u32(FRAME + 20).unwrap(), 0);
    }

    #[test]
    fn the_frames_arguments_read_back_as_the_blips() {
        let mut m = FakeGuest::new();
        m.put(FRAME, &[0u8; 0x40]);
        put_blip(&mut m, FRAME, &blip()).unwrap();
        let arg = |n: u32| m.u32(FRAME + 4 + 4 * n).unwrap();
        let at = arg(0);
        let read = Blip {
            position: [m.u32(at).unwrap(), m.u32(at + 4).unwrap()],
            shade: arg(1),
            size: arg(2),
            colour: arg(3),
            pulse: arg(4) as u8 != 0,
        };
        assert_eq!(read, blip());
    }

    #[test]
    fn a_call_that_does_not_read_is_named_and_not_drawn() {
        // No point at EAX.
        let mut m = FakeGuest::new();
        let why = plan(&mut m, FRAME, 0x1f80, 0x027f, 0, 0x0060_0000).unwrap_err();
        assert!(matches!(&why, Why::Read(what) if what.contains("point")), "{why}");
        // A point, but no camera or ring behind it.
        let mut m = FakeGuest::new();
        m.put_u16(0x0060_0000, 73).put_u16(0x0060_0002, 414);
        assert!(matches!(plan(&mut m, FRAME, 0x1f80, 0x027f, 0, 0x0060_0000), Err(Why::Read(_))));
    }

    #[test]
    fn the_causes_read_as_what_they_are() {
        assert_eq!(Why::Bitmap.to_string(), "a blip's bitmap is not resident");
        assert_eq!(Why::Frame.to_string(), "a blip's frame does not write");
    }
}
