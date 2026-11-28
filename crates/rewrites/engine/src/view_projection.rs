//! The view projection builder `0x0002fb70`, bit for bit
//! (docs/graphics/native-visibility.md, "The view records").
//!
//! Its one caller, `0x0002fd90` at `0x0002fd99`, passes the render camera
//! in ECX, the tangent extents in EDX (null for the default
//! `[min, one, min, one]`) and a 0x40-byte stack buffer in EAX; the
//! builder returns with `ret`. [`build`] reads what the original reads and
//! returns the buffer's sixteen words and the two registers the original
//! leaves changed: ECX (the tangent, which it pops from its scratch slot)
//! and EDX's low byte (the last flag byte it tested).
//!
//! | Word | Value |
//! |---|---|
//! | `+0x00..+0x0c` | the extents `e0..e3`, after the display adjustment of `e2`, `e3` |
//! | `+0x10`, `+0x14` | the viewport's width and height |
//! | `+0x18`, `+0x1c` | `xr = (e1 − e0)·half`, `yr = (e3 − e2)·half` |
//! | `+0x20`, `+0x24` | `ox = (e1 + e0)/xr·s`, `oy = (e2 + e3)/yr·s` |
//! | `+0x28`, `+0x2c` | `sx = one/((xr·aspect)·t)`, `sy = one/(yr·t)` |
//! | `+0x30..+0x3c` | `0 − (ox ± one)·(one/sx)`, `0 − (oy ± one)·(one/sy)` |
//!
//! `t` is the x87's `fptan` of `fov·half` at double precision, stored to
//! single. The aspect is `width/height`, times the anamorphic factor on a
//! widescreen display when the camera override's lens test does not hold;
//! with the display flag the widescreen branch scales `t`, the other
//! branch scales `e2` and `e3` by `height·[0x0045dd50]`.
//!
//! A NaN anywhere (inputs, constants or results) is refused: SSE's NaN
//! payload rules are x86's, not the host's. So is any overlap of the
//! buffer with what the builder reads after writing it.

use self::guest::*;
use guest_layout::memory::GuestMemory;

/// The image's words the builder reads.
pub mod guest {
    /// `one`, the extents' maximum and the offsets' unit.
    pub const ONE: u32 = 0x0045_dbc0;
    /// The default extents' minimum.
    pub const EXTENT_MIN: u32 = 0x0045_dbcc;
    /// `half`: the extents' half span and the field of view's.
    pub const HALF: u32 = 0x0045_dbbc;
    /// `s`, the offsets' scale.
    pub const OFFSET_SCALE: u32 = 0x0045_dbc4;
    /// The anamorphic aspect factor and the tangent factor it takes with
    /// the display flag.
    pub const ANAMORPHIC: u32 = 0x0045_dfdc;
    pub const ANAMORPHIC_TAN: u32 = 0x0045_e108;
    /// The factor on the height that scales `e2` and `e3` with the
    /// display flag off the widescreen branch.
    pub const DISPLAY_EXTENT: u32 = 0x0045_dd50;
    /// The camera override pointer: its byte `+1`, word `+2` and kind `+8`.
    pub const CAMERA_OVERRIDE: u32 = 0x004e_6948;
    /// The widescreen byte.
    pub const WIDESCREEN: u32 = 0x0048_5ac2;
    /// The display settings pointer; its byte `+5` is the flag.
    pub const DISPLAY: u32 = 0x0051_0c50;
    /// In the render camera: the vertical field of view and the viewport
    /// (`i16` top, left, bottom, right).
    pub const CAMERA_FOV: u32 = 0x28;
    pub const CAMERA_VIEWPORT: u32 = 0x30;
    /// The output buffer's size.
    pub const OUTPUT_SIZE: u32 = 0x40;
}

/// Why the port declines; the original runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Memory,
    /// The buffer overlaps what the builder reads after writing it, or an
    /// address wraps.
    Layout,
    /// A NaN, an out-of-range tangent, or a float state other than the one
    /// the port computes under.
    Unsupported,
}

impl From<guest_layout::memory::MemFault> for Refusal {
    fn from(_: guest_layout::memory::MemFault) -> Self {
        Self::Memory
    }
}

/// The call: the three argument registers and EDX as the caller left it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Call {
    pub camera: u32,
    pub extents: u32,
    pub output: u32,
}

/// What the call leaves: the buffer and the changed registers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Built {
    pub words: [u32; 16],
    /// The tangent's bits.
    pub ecx: u32,
    /// EDX on return.
    pub edx: u32,
}

fn span(base: u32, len: u32) -> Result<(u32, u32), Refusal> {
    Ok((base, base.checked_add(len).ok_or(Refusal::Layout)?))
}

fn overlaps(a: (u32, u32), b: (u32, u32)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

/// The x87 half of the builder: `fld dword fov; fmul dword half; fptan;
/// fstp st0; fstp dword`, under control word 0x027f.
pub fn tangent(fov: f32, half: f32) -> Option<f32> {
    crate::scene_list_math::view_tangent(fov, half)
}

/// `0x0002fb70` for `call` with EDX `edx`, under `mxcsr` and the x87
/// control word `x87_control`.
pub fn build(
    m: &dyn GuestMemory,
    call: Call,
    mxcsr: u32,
    x87_control: u16,
) -> Result<Built, Refusal> {
    if mxcsr & !0x3f != 0x1f80 || x87_control != 0x027f {
        return Err(Refusal::Unsupported);
    }
    let out = span(call.output, OUTPUT_SIZE)?;
    // Read after the first write: the field of view, the extents (read as
    // they are copied) and every global.
    let mut late = vec![
        span(
            call.camera.checked_add(CAMERA_FOV).ok_or(Refusal::Layout)?,
            4,
        )?,
        span(ONE, 4)?,
        span(EXTENT_MIN, 4)?,
        span(HALF, 4)?,
        span(OFFSET_SCALE, 4)?,
        span(ANAMORPHIC, 4)?,
        span(ANAMORPHIC_TAN, 4)?,
        span(DISPLAY_EXTENT, 4)?,
        span(CAMERA_OVERRIDE, 4)?,
        span(WIDESCREEN, 1)?,
        span(DISPLAY, 4)?,
    ];
    if call.extents != 0 {
        late.push(span(call.extents, 16)?);
    }
    let override_ptr = m.u32(CAMERA_OVERRIDE)?;
    let display_ptr = m.u32(DISPLAY)?;
    if override_ptr != 0 {
        late.push(span(override_ptr, 12)?);
    }
    if display_ptr != 0 {
        late.push(span(display_ptr.checked_add(5).ok_or(Refusal::Layout)?, 1)?);
    }
    if late.iter().any(|&r| overlaps(r, out)) {
        return Err(Refusal::Layout);
    }

    let f = |addr: u32| -> Result<f32, Refusal> {
        let v = m.f32(addr)?;
        if v.is_nan() {
            Err(Refusal::Unsupported)
        } else {
            Ok(v)
        }
    };
    let viewport = call
        .camera
        .checked_add(CAMERA_VIEWPORT)
        .ok_or(Refusal::Layout)?;
    let half_at = |k: u32| -> Result<i32, Refusal> {
        Ok(i32::from(m.u16(viewport.wrapping_add(2 * k))? as i16))
    };
    let (top, left, bottom, right) = (half_at(0)?, half_at(1)?, half_at(2)?, half_at(3)?);
    let one = f(ONE)?;
    let width = (right - left) as f32;
    let height = (bottom - top) as f32;
    let (mut edx, mut e) = (call.extents, [0f32; 4]);
    if call.extents == 0 {
        let min = f(EXTENT_MIN)?;
        e = [min, one, min, one];
    } else {
        for (k, v) in e.iter_mut().enumerate() {
            *v = f(call.extents + 4 * k as u32)?;
        }
        edx = m.u32(call.extents + 12)?;
    }
    let half = f(HALF)?;
    let s = f(OFFSET_SCALE)?;
    let xr = (e[1] - e[0]) * half;
    let yr = (e[3] - e[2]) * half;
    let ox = (e[1] + e[0]) / xr * s;
    let oy = (e[2] + e[3]) / yr * s;
    let fov = f(call.camera + CAMERA_FOV)?;
    let t = tangent(fov, half).ok_or(Refusal::Unsupported)?;
    let mut aspect = width / height;

    let override_lens = override_ptr != 0 && {
        let on = m.u8(override_ptr + 1)?;
        edx = (edx & !0xff) | u32::from(on);
        on != 0 && m.u16(override_ptr + 2)? != u16::MAX && m.u32(override_ptr + 8)? == 3
    };
    let display = if display_ptr != 0 {
        let flag = m.u8(display_ptr + 5)?;
        edx = (edx & !0xff) | u32::from(flag);
        flag != 0
    } else {
        false
    };
    let widescreen = !override_lens && m.u8(WIDESCREEN)? != 0;
    let lens_t = if widescreen {
        let anamorphic = f(ANAMORPHIC)?;
        aspect *= anamorphic;
        if display {
            t * anamorphic * f(ANAMORPHIC_TAN)?
        } else {
            t
        }
    } else {
        if display {
            let k = height * f(DISPLAY_EXTENT)?;
            e[2] *= k;
            e[3] *= k;
        }
        t
    };

    let sx = one / ((xr * aspect) * lens_t);
    let inv_sx = one / sx;
    let sy = one / (yr * lens_t);
    let inv_sy = one / sy;
    let values = [
        e[0],
        e[1],
        e[2],
        e[3],
        width,
        height,
        xr,
        yr,
        ox,
        oy,
        sx,
        sy,
        0.0 - (ox + one) * inv_sx,
        0.0 - (ox - one) * inv_sx,
        0.0 - (oy + one) * inv_sy,
        0.0 - (oy - one) * inv_sy,
    ];
    if values.iter().any(|v| v.is_nan()) {
        return Err(Refusal::Unsupported);
    }
    Ok(Built {
        words: values.map(f32::to_bits),
        ecx: t.to_bits(),
        edx,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    const CAMERA: u32 = 0x0010_0000;
    const EXTENTS: u32 = 0x0010_1000;
    const OUT: u32 = 0x0010_2000;
    const OVERRIDE: u32 = 0x0010_3000;
    const SETTINGS: u32 = 0x0010_4000;

    fn image() -> FakeGuest {
        let mut m = FakeGuest::new();
        m.put_f32(ONE, 1.0);
        m.put_f32(EXTENT_MIN, -1.0);
        m.put_f32(HALF, 0.5);
        m.put_f32(OFFSET_SCALE, 0.5);
        m.put_f32(ANAMORPHIC, 4.0 / 3.0);
        m.put_f32(ANAMORPHIC_TAN, 0.75);
        m.put_f32(DISPLAY_EXTENT, 1.0 / 480.0);
        m.put_u32(CAMERA_OVERRIDE, 0);
        m.put_u8(WIDESCREEN, 0);
        m.put_u32(DISPLAY, 0);
        m.put_f32(CAMERA + CAMERA_FOV, 1.2);
        for (k, v) in [0u16, 0, 480, 640].into_iter().enumerate() {
            m.put_u16(CAMERA + CAMERA_VIEWPORT + 2 * k as u32, v);
        }
        m.map(OUT, OUTPUT_SIZE);
        m
    }

    fn call(extents: u32) -> Call {
        Call {
            camera: CAMERA,
            extents,
            output: OUT,
        }
    }

    fn word(b: &Built, offset: u32) -> f32 {
        f32::from_bits(b.words[offset as usize / 4])
    }

    #[test]
    fn the_default_extents_make_a_centred_lens() {
        let m = image();
        let b = build(&m, call(0), 0x1f80, 0x027f).unwrap();
        assert_eq!(
            [word(&b, 0), word(&b, 4), word(&b, 8), word(&b, 0xc)],
            [-1.0, 1.0, -1.0, 1.0]
        );
        assert_eq!((word(&b, 0x10), word(&b, 0x14)), (640.0, 480.0));
        assert_eq!((word(&b, 0x18), word(&b, 0x1c)), (1.0, 1.0));
        assert_eq!((word(&b, 0x20), word(&b, 0x24)), (0.0, 0.0));
        let t = tangent(1.2, 0.5).unwrap();
        assert_eq!(b.ecx, t.to_bits());
        assert_eq!(
            word(&b, 0x28).to_bits(),
            (1.0 / ((1.0 * (640.0f32 / 480.0)) * t)).to_bits()
        );
        assert_eq!(word(&b, 0x2c).to_bits(), (1.0 / t).to_bits());
        let inv = 1.0 / word(&b, 0x28);
        assert_eq!(word(&b, 0x30).to_bits(), (0.0 - inv).to_bits());
        assert_eq!(
            word(&b, 0x34).to_bits(),
            (0.0 - (0.0f32 - 1.0) * inv).to_bits()
        );
        assert_eq!(b.edx, 0, "EDX keeps the null extents");
    }

    #[test]
    fn given_extents_are_copied_and_leave_edx_their_last_word() {
        let mut m = image();
        for (k, v) in [-0.5f32, 1.5, -1.0, 0.25].into_iter().enumerate() {
            m.put_f32(EXTENTS + 4 * k as u32, v);
        }
        let b = build(&m, call(EXTENTS), 0x1f80, 0x027f).unwrap();
        assert_eq!(word(&b, 0), -0.5);
        assert_eq!(word(&b, 0xc), 0.25);
        assert_eq!(word(&b, 0x18), (1.5f32 - -0.5) * 0.5);
        assert_eq!(word(&b, 0x20), (1.5f32 + -0.5) / 1.0 * 0.5);
        assert_eq!(b.edx, 0.25f32.to_bits());
    }

    #[test]
    fn widescreen_widens_the_aspect_and_the_display_flag_the_tangent() {
        let mut m = image();
        m.put_u8(WIDESCREEN, 1);
        let wide = build(&m, call(0), 0x1f80, 0x027f).unwrap();
        let t = tangent(1.2, 0.5).unwrap();
        let aspect = 640.0f32 / 480.0 * (4.0 / 3.0);
        assert_eq!(
            word(&wide, 0x28).to_bits(),
            (1.0 / ((1.0 * aspect) * t)).to_bits()
        );
        m.put_u32(DISPLAY, SETTINGS);
        m.put_u8(SETTINGS + 5, 1);
        let shown = build(&m, call(0), 0x1f80, 0x027f).unwrap();
        let lens_t = t * (4.0 / 3.0) * 0.75;
        assert_eq!(word(&shown, 0x2c).to_bits(), (1.0 / lens_t).to_bits());
        assert_eq!(
            shown.ecx,
            t.to_bits(),
            "ECX holds the tangent before the factors"
        );
        assert_eq!(shown.edx & 0xff, 1);
    }

    #[test]
    fn the_override_lens_skips_widescreen_and_the_display_flag_scales_the_extents() {
        let mut m = image();
        m.put_u8(WIDESCREEN, 1);
        m.put_u32(CAMERA_OVERRIDE, OVERRIDE);
        m.put_u8(OVERRIDE + 1, 1);
        m.put_u16(OVERRIDE + 2, 7);
        m.put_u32(OVERRIDE + 8, 3);
        m.put_u32(DISPLAY, SETTINGS);
        m.put_u8(SETTINGS + 5, 1);
        let b = build(&m, call(0), 0x1f80, 0x027f).unwrap();
        let k = 480.0f32 * (1.0 / 480.0);
        assert_eq!(word(&b, 8), -k);
        assert_eq!(word(&b, 0xc), k * 1.0);
        let t = tangent(1.2, 0.5).unwrap();
        assert_eq!(
            word(&b, 0x28).to_bits(),
            (1.0 / ((1.0 * (640.0f32 / 480.0)) * t)).to_bits()
        );
        // A kind other than 3 is no override lens.
        m.put_u32(OVERRIDE + 8, 2);
        let wide = build(&m, call(0), 0x1f80, 0x027f).unwrap();
        assert_ne!(wide.words[10], b.words[10]);
    }

    #[test]
    fn the_last_flag_byte_tested_is_left_in_dl() {
        let mut m = image();
        m.put_u32(CAMERA_OVERRIDE, OVERRIDE);
        m.put_u8(OVERRIDE + 1, 0);
        let b = build(
            &m,
            Call {
                camera: CAMERA,
                extents: 0,
                output: OUT,
            },
            0x1f80,
            0x027f,
        )
        .unwrap();
        assert_eq!(b.edx, 0);
        m.put_u8(OVERRIDE + 1, 9);
        m.put_u16(OVERRIDE + 2, 0xffff);
        assert_eq!(build(&m, call(0), 0x1f80, 0x027f).unwrap().edx, 9);
        m.put_u32(DISPLAY, SETTINGS);
        m.put_u8(SETTINGS + 5, 0);
        assert_eq!(build(&m, call(0), 0x1f80, 0x027f).unwrap().edx, 0);
    }

    #[test]
    fn nan_aliasing_and_other_float_states_decline() {
        let mut m = image();
        assert_eq!(
            build(&m, call(0), 0x1f80 | 0x8000, 0x027f),
            Err(Refusal::Unsupported)
        );
        assert_eq!(
            build(&m, call(0), 0x1f80, 0x037f),
            Err(Refusal::Unsupported)
        );
        let aliased = Call {
            output: CAMERA + 0x10,
            ..call(0)
        };
        assert_eq!(build(&m, aliased, 0x1f80, 0x027f), Err(Refusal::Layout));
        m.put_u16(CAMERA + CAMERA_VIEWPORT + 4, 0);
        m.put_u16(CAMERA + CAMERA_VIEWPORT + 6, 0);
        assert_eq!(
            build(&m, call(0), 0x1f80, 0x027f),
            Err(Refusal::Unsupported),
            "0/0 aspect"
        );
    }
}
