//! A speaking unit's facial sequence over guest memory: the unit's entry
//! `0x10e480`, the custom-state test `0x11b930` and the channel blend
//! `0x10de40` (docs/engine/sound-instances.md, "Owners").
//!
//! The sequence is a run of key tracks the sound's extra-info resource
//! holds; the blend samples every track at the line's time, mixes it with
//! what the unit showed last tick and keeps the 12 strongest as signed
//! bytes in the unit's dialogue state.
//!
//! | Data | Where |
//! |---|---|
//! | the unit's dialogue state `S` | object `+ i16 [object+0x33e]`: line tag `+0x38`, sequence on `+0x36` (not `0xffff`) |
//! | the channel state `C` | `S + 0x30`: time `+0xc`, 12 track indices `+0x14` (i8), 12 values `+0x20` (i8, 1/128) |
//! | the blend weight | the unit tag's `+0x1e4` when above 0, else 0.95 (`0x45ddac`) |
//! | the custom-state gate | `[0x4de2f4] + 0x81`, then `0x11b930` |
//! | a sequence `H` | `+0` i16 track count, `+2` version `0x96c`, `+0xc` f32 seconds a key unit, `+0x10` tracks (i16 key count, i16 first key), then keys (i16 time, i16 value at 1e-4) |
//! | the previous value scale | `[0x547648]`, written at startup |
//!
//! Floats follow `sim_core::det`: the SSE steps through [`Fp::guest`] under
//! the live game's MXCSR, the two x87 chains through [`X87`] under its
//! control word.

use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points::{OBJECTS, TAG_ENTRIES};
use sim_core::det::{self, Fp, X87};
use std::cmp::Ordering;

const NONE: u32 = u32::MAX;
/// The live game's MXCSR and x87 control word.
pub const MXCSR: u32 = 0x1f80;
pub const FCW: u16 = 0x027f;
/// `0x45ddac`: the blend weight of a unit tag with none above 0.
pub const DEFAULT_WEIGHT: u32 = 0x3f73_3333;
/// `[0x4de2f4] + 0x81`: the custom-state gate is on.
pub const GATE: u32 = 0x004d_e2f4;
pub const GATE_ON: u32 = 0x81;
/// `0x11b930`'s state string id.
pub const CUSTOM_STATE: u32 = 0x0e00_00c2;
/// `[0x547648]`: the scale of a stored value byte back to a float.
pub const PREVIOUS_SCALE: u32 = 0x0054_7648;
/// A sequence's version halfword.
pub const VERSION: u16 = 0x96c;
/// `0x45dbdc`: the key value unit, the span floor and the drop floor.
const EPS: u32 = 0x38d1_b717;
/// `0x45dbd4`, `0x45dc0c`, `0x45dbc0`: the smoothstep's 3, 2 and 1.
const THREE: u32 = 0x4040_0000;
const TWO: u32 = 0x4000_0000;
const ONE: u32 = 0x3f80_0000;
/// `0x44aed4`: the weakest-slot search's start.
const FLT_MAX: u32 = 0x7f7f_ffff;
/// `0x45dd60`: a kept value's byte scale.
const BYTE_SCALE: u32 = 0x4300_0000;
/// Slots the state keeps and tracks the previous values cover.
pub const SLOTS: usize = 12;
const TRACKS: i32 = 24;
/// Tracks the custom state holds still (`0x10e063..0x10e070`).
const HELD: [i32; 3] = [0x12, 0x13, 0x14];

/// A facial sequence as `0x18ca20` hands it on: its byte length and where
/// it starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Curve {
    pub size: u32,
    pub data: u32,
}

/// `[[0x4e0300] + 0x44] + 12 * (unit & 0xffff) + 8`: the object's data,
/// at the header stride the callers fix.
pub fn object(m: &dyn GuestMemory, unit: u32) -> Result<u32, MemFault> {
    let headers = m.u32(m.u32(OBJECTS.base)?.wrapping_add(0x44))?;
    m.u32(headers.wrapping_add((unit & 0xffff) * 12).wrapping_add(8))
}

fn tag_definition(m: &dyn GuestMemory, tag: u32) -> Result<u32, MemFault> {
    let entries = m.u32(TAG_ENTRIES.base)?;
    m.u32(entries.wrapping_add((tag & 0xffff) << 4).wrapping_add(8))
}

fn i16_at(m: &dyn GuestMemory, at: u32) -> Result<i32, MemFault> {
    Ok(i32::from(m.u16(at)? as i16))
}

/// `0x11b930(ecx = unit) -> al`: the unit's custom animation state is
/// `0x0e0000c2`.
pub fn custom_state(m: &dyn GuestMemory, unit: u32) -> Result<bool, MemFault> {
    if unit == NONE {
        return Ok(false);
    }
    let obj = object(m, unit)?;
    let a = obj.wrapping_add(i16_at(m, obj.wrapping_add(0x12a))? as u32);
    let id = if m.u32(a.wrapping_add(0x68))? == NONE
        || m.u32(a)? == NONE
        || m.u16(a.wrapping_add(6))? == 0xffff
    {
        NONE
    } else {
        m.u32(a.wrapping_add(0x7c))?
    };
    Ok(id == CUSTOM_STATE)
}

/// `0x10e480(ecx = unit, tag, &curve, seconds)`: the line `tag` is the
/// unit's, and with its sequence on the curve blends in at `seconds`.
pub fn unit_sequence(
    m: &mut dyn GuestMemory,
    unit: u32,
    tag: u32,
    curve: Curve,
    seconds: u32,
) -> Result<(), MemFault> {
    let obj = object(&*m, unit)?;
    let state = obj.wrapping_add(i16_at(&*m, obj.wrapping_add(0x33e))? as u32);
    let def = tag_definition(&*m, m.u32(obj)?)?;
    let tagged = m.u32(def.wrapping_add(0x1e4))?;
    let weight = if det::comiss(tagged, 0) == Some(Ordering::Greater) {
        tagged
    } else {
        DEFAULT_WEIGHT
    };
    m.set_u32(state.wrapping_add(0x38), tag)?;
    if m.u16(state.wrapping_add(0x36))? == 0xffff {
        return Ok(());
    }
    let held = m.u8(m.u32(GATE)?.wrapping_add(GATE_ON))? != 0 && custom_state(&*m, unit)?;
    blend(m, state.wrapping_add(0x30), curve, seconds, weight, held)
}

/// `cvttss2si`: the indefinite integer for a NaN or out-of-range value.
fn cvtt(x: u32) -> i32 {
    if det::truncates(x) {
        det::truncate(x)
    } else {
        let v = f32::from_bits(x);
        if v.is_finite() && v.abs() < 2_147_483_648.0 {
            v as i32
        } else {
            i32::MIN
        }
    }
}

fn greater(a: u32, b: u32) -> bool {
    det::comiss(a, b) == Some(Ordering::Greater)
}

/// One track's value at `time` (`0x10e078..0x10e252`), the f32 bits.
fn track_value(
    fp: &mut Fp,
    x: &X87,
    m: &dyn GuestMemory,
    keys: u32,
    n: i32,
    scale: u32,
    time: u32,
) -> Result<u32, MemFault> {
    let key_time = |fp: &mut Fp, k: i32| -> Result<u32, MemFault> {
        Ok(fp.mul(
            det::from_int(i16_at(m, keys.wrapping_add((k * 4) as u32))?),
            scale,
        ))
    };
    let key_value = |fp: &mut Fp, k: i32| -> Result<u32, MemFault> {
        Ok(fp.mul(
            det::from_int(i16_at(m, keys.wrapping_add((k * 4 + 2) as u32))?),
            EPS,
        ))
    };
    let last = if n > 1 { n - 1 } else { 0 };
    let first_time = key_time(fp, 0)?;
    let first_value = key_value(fp, 0)?;
    let last_time = key_time(fp, last)?;
    let last_value = key_value(fp, last)?;
    // `jae`: ordered and not below.
    if matches!(
        det::comiss(first_time, time),
        Some(Ordering::Greater | Ordering::Equal)
    ) || n == 1
    {
        return Ok(first_value);
    }
    if matches!(
        det::comiss(time, last_time),
        Some(Ordering::Greater | Ordering::Equal)
    ) {
        return Ok(last_value);
    }
    let mut after = None;
    for k in 0..n {
        if greater(key_time(fp, k)?, time) {
            after = Some(k);
            break;
        }
    }
    let Some(b) = after else { return Ok(0) };
    let a = b - 1;
    if a < 0 {
        return Ok(0);
    }
    let (ta, va) = (key_time(fp, a)?, key_value(fp, a)?);
    let (tb, vb) = (key_time(fp, a + 1)?, key_value(fp, a + 1)?);
    let span = fp.sub(tb, ta);
    if !greater(span, EPS) {
        return Ok(va);
    }
    let along = fp.sub(time, ta);
    let u = fp.div(along, span);
    let u2 = fp.mul(u, u);
    let square = fp.mul(u2, THREE);
    let cube = fp.mul(u2, u);
    let cube = fp.mul(cube, TWO);
    let s = fp.sub(square, cube);
    let s = if greater(0, s) {
        0
    } else if greater(s, ONE) {
        ONE
    } else {
        s
    };
    let st = x.add(
        x.mul(x.sub(x.load32(vb), x.load32(va)), x.load32(s)),
        x.load32(va),
    );
    let v = x.store32(st);
    if x.compare(x.load32(EPS), x.abs(st), false) == Some(Ordering::Greater) {
        return Ok(0);
    }
    Ok(v)
}

/// `0x10de40(eax = &curve, edx = channel, time, weight, held)`: sample
/// every track at `time`, blend with the channel's previous values by
/// `weight`, and keep the 12 strongest. `held` skips tracks 0x12..0x14.
pub fn blend(
    m: &mut dyn GuestMemory,
    channel: u32,
    curve: Curve,
    time: u32,
    weight: u32,
    held: bool,
) -> Result<(), MemFault> {
    m.set_u32(channel.wrapping_add(0xc), time)?;
    if curve.size <= 0x10 || curve.data == 0 || m.u16(curve.data.wrapping_add(2))? != VERSION {
        return Ok(());
    }
    let h = curve.data;
    let mut fp = Fp::guest(MXCSR);
    let x = X87::new(FCW);
    let count = i16_at(&*m, h)?;
    let restore = m.u32(PREVIOUS_SCALE)?;
    let mut previous = [0u32; TRACKS as usize];
    for j in 0..SLOTS as u32 {
        let c = i32::from(m.u8(channel.wrapping_add(0x14 + j))? as i8);
        if c >= 0 && c < count && c < TRACKS {
            let byte = i32::from(m.u8(channel.wrapping_add(0x20 + j))? as i8);
            previous[c as usize] = fp.mul(det::from_int(byte), restore);
        }
    }
    let mut values = [0u32; SLOTS];
    let mut tracks = [0i32; SLOTS];
    m.fill(channel.wrapping_add(0x14), 0x18, 0)?;
    if count <= 0 {
        return Ok(());
    }
    let scale = m.u32(h.wrapping_add(0xc))?;
    let table = h.wrapping_add(0x10);
    let keys_base = table.wrapping_add((count * 4) as u32);
    let mut kept = 0usize;
    for t in 0..count {
        let track = table.wrapping_add((t * 4) as u32);
        let n = i16_at(&*m, track)?;
        if n <= 0 || (held && HELD.contains(&t)) {
            continue;
        }
        let keys = keys_base.wrapping_add((i16_at(&*m, track.wrapping_add(2))? * 4) as u32);
        let value = track_value(&mut fp, &x, &*m, keys, n, scale, time)?;
        // `previous` only covers the first 24 tracks; the guest reads its
        // frame past them (`[esp+0xb8+4t]`), which the 12-slot arrays and
        // locals above fill. A sequence of more tracks is outside it.
        let Some(&before) = previous.get(t as usize) else {
            unimplemented!(
                "sound mouth: facial sequence of {count} tracks reads the blend's frame past track 24"
            );
        };
        let st = x.add(
            x.mul(x.sub(x.load32(value), x.load32(before)), x.load32(weight)),
            x.load32(before),
        );
        let v = x.store32(st);
        let magnitude = x.abs(st);
        let stored = x.store32(magnitude);
        if x.compare(x.load32(EPS), magnitude, false) == Some(Ordering::Greater) {
            continue;
        }
        if kept < SLOTS {
            values[kept] = v;
            tracks[kept] = t;
            kept += 1;
            continue;
        }
        let (mut weakest, mut at) = (FLT_MAX, 0);
        for (i, &w) in values.iter().enumerate() {
            let a = w & 0x7fff_ffff;
            if greater(weakest, a) {
                weakest = a;
                at = i;
            }
        }
        if greater(stored, weakest) {
            values[at] = v;
            tracks[at] = t;
        }
    }
    for i in 0..kept {
        m.set_u8(channel.wrapping_add(0x14 + i as u32), tracks[i] as u8)?;
        let scaled = fp.mul(values[i], BYTE_SCALE);
        let q = cvtt(scaled).clamp(-128, 127);
        m.set_u8(channel.wrapping_add(0x20 + i as u32), q as u8)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    const CHANNEL: u32 = 0x0070_0000;
    const SEQ: u32 = 0x0071_0000;
    const HALF: u32 = 0x3f00_0000;

    /// A sequence of `tracks`, each a list of (time, value) keys, at one
    /// second a key unit.
    fn sequence(g: &mut FakeGuest, tracks: &[&[(i16, i16)]]) -> Curve {
        g.put_u16(SEQ, tracks.len() as u16)
            .put_u16(SEQ + 2, VERSION)
            .put_u32(SEQ + 0xc, ONE);
        g.put_u32(PREVIOUS_SCALE, (1.0f32 / 128.0).to_bits());
        let keys = SEQ + 0x10 + 4 * tracks.len() as u32;
        let mut next = 0u16;
        for (t, track) in tracks.iter().enumerate() {
            let at = SEQ + 0x10 + 4 * t as u32;
            g.put_u16(at, track.len() as u16).put_u16(at + 2, next);
            for &(time, value) in track.iter() {
                let k = keys + 4 * u32::from(next);
                g.put_u16(k, time as u16).put_u16(k + 2, value as u16);
                next += 1;
            }
        }
        g.map(CHANNEL, 0x40);
        Curve {
            size: 0x100,
            data: SEQ,
        }
    }

    fn slots(g: &FakeGuest) -> (Vec<i8>, Vec<i8>) {
        let t = (0..12)
            .map(|i| g.u8(CHANNEL + 0x14 + i).unwrap() as i8)
            .collect();
        let v = (0..12)
            .map(|i| g.u8(CHANNEL + 0x20 + i).unwrap() as i8)
            .collect();
        (t, v)
    }

    #[test]
    fn a_track_between_keys_takes_the_smoothstep_at_full_weight() {
        let mut g = FakeGuest::new();
        // Value 5000 is 0.5; halfway the smoothstep is 0.5, so 0.25.
        let c = sequence(&mut g, &[&[(0, 0), (2, 5000)]]);
        blend(&mut g, CHANNEL, c, ONE, ONE, false).unwrap();
        let (t, v) = slots(&g);
        assert_eq!((t[0], v[0]), (0, 32), "0.25 * 128");
        assert_eq!(g.u32(CHANNEL + 0xc), Ok(ONE), "the time is stored");
    }

    #[test]
    fn before_the_first_key_and_after_the_last_the_ends_hold() {
        let mut g = FakeGuest::new();
        let c = sequence(&mut g, &[&[(1, 2500), (2, 7500)]]);
        blend(&mut g, CHANNEL, c, 0, ONE, false).unwrap();
        assert_eq!(slots(&g).1[0], 32);
        blend(&mut g, CHANNEL, c, 0x4040_0000, ONE, false).unwrap();
        assert_eq!(slots(&g).1[0], 96);
    }

    #[test]
    fn the_previous_value_mixes_in_by_the_weight() {
        let mut g = FakeGuest::new();
        let c = sequence(&mut g, &[&[(0, 5000)]]);
        // Last tick: track 0 at 64/128 = 0.5; now 0.5 again at half
        // weight stays 0.5. A fresh state at half weight gives 0.25.
        g.put_u8(CHANNEL + 0x14, 0).put_u8(CHANNEL + 0x20, 64);
        blend(&mut g, CHANNEL, c, 0, HALF, false).unwrap();
        assert_eq!(slots(&g).1[0], 64);
        g.put_u8(CHANNEL + 0x20, 0);
        blend(&mut g, CHANNEL, c, 0, HALF, false).unwrap();
        assert_eq!(slots(&g).1[0], 32);
    }

    #[test]
    fn a_silent_track_keeps_no_slot_and_a_held_one_is_skipped() {
        let mut g = FakeGuest::new();
        let mut tracks: Vec<&[(i16, i16)]> = vec![&[(0, 0)][..]; 0x15];
        tracks[0x12] = &[(0, 5000)][..];
        tracks[0x14] = &[(0, 2500)][..];
        let c = sequence(&mut g, &tracks);
        blend(&mut g, CHANNEL, c, 0, ONE, false).unwrap();
        let (t, v) = slots(&g);
        assert_eq!((&t[..3], &v[..3]), (&[0x12, 0x14, 0][..], &[64, 32, 0][..]));
        blend(&mut g, CHANNEL, c, 0, ONE, true).unwrap();
        assert_eq!(
            slots(&g),
            (vec![0; 12], vec![0; 12]),
            "held: nothing kept, the slots cleared"
        );
    }

    #[test]
    fn past_twelve_the_strongest_replace_the_weakest() {
        let mut g = FakeGuest::new();
        let keys: Vec<[(i16, i16); 1]> = (1..=13).map(|i| [(0, i * 500)]).collect();
        let tracks: Vec<&[(i16, i16)]> = keys.iter().map(|k| &k[..]).collect();
        let c = sequence(&mut g, &tracks);
        blend(&mut g, CHANNEL, c, 0, ONE, false).unwrap();
        let (t, _) = slots(&g);
        assert_eq!(
            t[0], 12,
            "track 12 (the strongest) replaced track 0 (the weakest)"
        );
        assert_eq!(&t[1..], &(1..12).collect::<Vec<i8>>()[..]);
    }

    #[test]
    fn a_wrong_version_or_a_short_curve_only_stores_the_time() {
        let mut g = FakeGuest::new();
        let c = sequence(&mut g, &[&[(0, 5000)]]);
        g.put_u8(CHANNEL + 0x20, 9);
        blend(&mut g, CHANNEL, Curve { size: 0x10, ..c }, HALF, ONE, false).unwrap();
        assert_eq!(g.u8(CHANNEL + 0x20), Ok(9));
        g.put_u16(SEQ + 2, 0x96b);
        blend(&mut g, CHANNEL, c, HALF, ONE, false).unwrap();
        assert_eq!(g.u8(CHANNEL + 0x20), Ok(9));
        assert_eq!(g.u32(CHANNEL + 0xc), Ok(HALF));
    }

    const ARRAY: u32 = 0x0060_0000;
    const HEADERS: u32 = 0x0060_1000;
    const OBJECT: u32 = 0x0060_2000;
    const ENTRIES: u32 = 0x0062_0000;
    const UNIT_DEF: u32 = 0x0063_0000;
    const UNIT: u32 = 0x0007_0002;
    const STATE: u32 = 0x100;
    const CUSTOM: u32 = 0x200;

    fn unit(g: &mut FakeGuest) {
        g.put_u32(OBJECTS.base, ARRAY)
            .put_u32(ARRAY + 0x44, HEADERS)
            .put_u32(HEADERS + 2 * 12 + 8, OBJECT);
        g.put_u32(OBJECT, 5)
            .put_u32(TAG_ENTRIES.base, ENTRIES)
            .put_u32(ENTRIES + 5 * 16 + 8, UNIT_DEF)
            .map(UNIT_DEF + 0x1e4, 4);
        g.put_u16(OBJECT + 0x33e, STATE as u16)
            .put_u16(OBJECT + STATE + 0x36, 0);
        g.put_u16(OBJECT + 0x12a, CUSTOM as u16)
            .map(OBJECT + CUSTOM, 0x80);
        g.put_u32(GATE, 0x0064_0000).map(0x0064_0000 + GATE_ON, 1);
    }

    #[test]
    fn the_unit_entry_records_the_line_and_blends_at_its_weight() {
        let mut g = FakeGuest::new();
        unit(&mut g);
        let c = sequence(&mut g, &[&[(0, 5000)]]);
        unit_sequence(&mut g, UNIT, 0xe100_0001, c, 0).unwrap();
        assert_eq!(g.u32(OBJECT + STATE + 0x38), Ok(0xe100_0001));
        // 0.95 * 0.5 * 128 = 60.8, truncated.
        assert_eq!(g.u8(OBJECT + STATE + 0x30 + 0x20), Ok(60));
        g.put_f32(UNIT_DEF + 0x1e4, 0.5);
        unit_sequence(&mut g, UNIT, 0xe100_0001, c, 0).unwrap();
        // 60/128 + (0.5 - 60/128) * 0.5 = 0.484375 -> 62.
        assert_eq!(g.u8(OBJECT + STATE + 0x30 + 0x20), Ok(62));
    }

    #[test]
    fn a_unit_with_its_sequence_off_only_records_the_line() {
        let mut g = FakeGuest::new();
        unit(&mut g);
        let c = sequence(&mut g, &[&[(0, 5000)]]);
        g.put_u16(OBJECT + STATE + 0x36, 0xffff);
        unit_sequence(&mut g, UNIT, 7, c, ONE).unwrap();
        assert_eq!(g.u32(OBJECT + STATE + 0x38), Ok(7));
        assert_eq!(g.u32(OBJECT + STATE + 0x30 + 0xc), Ok(0), "no blend ran");
    }

    #[test]
    fn the_custom_state_needs_a_whole_state_and_its_id() {
        let mut g = FakeGuest::new();
        unit(&mut g);
        let a = OBJECT + CUSTOM;
        assert_eq!(custom_state(&g, UNIT), Ok(false));
        g.put_u32(a + 0x7c, CUSTOM_STATE);
        assert_eq!(custom_state(&g, UNIT), Ok(true));
        g.put_u16(a + 6, 0xffff);
        assert_eq!(custom_state(&g, UNIT), Ok(false));
        assert_eq!(custom_state(&g, NONE), Ok(false));
    }
}
