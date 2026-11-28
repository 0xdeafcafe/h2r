//! Shader parameter writes at 0x1ae70, 0x17170, 0x19be0 and 0x19ca0.
//! The live cache is both an input and an output. Fragment-relative
//! destinations, dirty masks and partial registers survive material changes.

use crate::light_constants::{Error, Sse, X87};
use guest_layout::memory::{GuestMemory, MemFault};

const SCALAR: u32 = 0x5c;
const VECTOR: u32 = 0x108;
const INTEGER: u32 = 0x300;
const SHADER_INPUT: u32 = 0x0048_5a58;
const VIEW_VECTORS: u32 = 0x004e_6950;
const DEFAULT_TRANSFORM: u32 = 0x0046_86d4;
const DEFAULT_COLOUR: u32 = 0x0046_8710;
const ONE: u32 = 0x0045_dbc0;
const HALF: u32 = 0x0045_dbbc;
const ZERO_THRESHOLD: u32 = 0x0045_dbd8;
const UNIT_TO_BYTE: u32 = 0x0045_dc38;
const BYTE_TO_UNIT: u32 = 0x0045_dbb4;
const WAVE_PERIOD: u32 = 0x0043_f17c;
const LANE_MASK: u32 = 0x0046_7068;
const VERTEX_DEFAULT: u32 = 0x1530;
const VERTEX_CONSTANT: u32 = 0x1630;
const VERTEX_DIRTY: u32 = 0x1730;
const PS_COLOUR: u32 = 0x144c;
const PS_FINAL: u32 = 0x14d0;
const PS_DIRTY: u32 = 0x1518;
const PS_FRAGMENT_OFFSET: u32 = 0x1520;

fn words3(m: &dyn GuestMemory, p: u32) -> Result<[u32; 3], MemFault> {
    Ok([m.u32(p)?, m.u32(p+4)?, m.u32(p+8)?])
}

fn lighting(m: &dyn GuestMemory, offset: u32) -> Result<u32, MemFault> {
    let p = m.u32(SHADER_INPUT)?;
    Ok(if p == 0 { m.u32(DEFAULT_TRANSFORM)? + 4 } else { p + offset })
}

/// Validate the implementation's input dispatches and vertex binding
/// modes while its sorted recipe can still be declined. Empty slots in
/// the retail selector tables have no evaluator.
pub fn validate(m: &dyn GuestMemory, definition: u32, implementation: u32)
    -> Result<(), &'static str> {
    for (offset, base, scalar) in [
        (0x10c, 0, true), (0x110, 32, true),
        (0x104, 0, false), (0x108, 32, false),
    ] {
        let mut mask = m.u32(implementation+offset).map_err(|_| "shader input mask")?;
        while mask != 0 {
            let input = base + mask.trailing_zeros();
            mask &= mask-1;
            let available = if scalar { matches!(input, 0 | 2..=4 | 10 | 12 | 13 | 16..=42) }
                else { matches!(input, 0 | 3..=6 | 14..=19 | 21..=45) };
            if !available {
                return Err(if scalar { "unassigned scalar shader input" } else { "unassigned vector shader input" });
            }
        }
    }
    if m.u32(implementation+0x114).map_err(|_| "integer shader input mask")? >> 8 != 0 {
        return Err("unassigned integer shader input");
    }
    let span = m.u16(implementation+0x124).map_err(|_| "vertex input bindings")?;
    if span >> 9 != 0 {
        let list = m.u32(definition+0x3c).map_err(|_| "vertex input binding table")?;
        for i in 0..u32::from(span >> 9) {
            let at = list.wrapping_add((u32::from(span & 511)+i)*4);
            let operand = m.u16(at).map_err(|_| "vertex input binding")?;
            if operand >> 5 & 31 > 20 { return Err("unassigned vertex input binding mode"); }
        }
    }
    Ok(())
}

/// The scalar/vector object-root cases use the shared object's datum walk.
/// Unpopulated evaluator slots return a data error as dispatch would fault.
pub fn inputs(m: &mut dyn GuestMemory, cache: u32, fcw: u16, mxcsr: u32,
    root: impl Fn(&dyn GuestMemory, u32) -> Option<u32>) -> Result<(), Error> {
    let mut masks = [0; 5];
    m.set_u32(cache + 0x58, 0)?;
    for (i, mask) in masks.iter_mut().enumerate() {
        let offset = 4 * i as u32;
        *mask = m.u32(cache + 0x30 + offset)? & m.u32(cache + 0x44 + offset)?;
    }
    let mut s = Sse { mxcsr };
    for (i, mut mask) in masks.into_iter().enumerate() {
        let dirty = cache + 0x30 + i as u32 * 4;
        while mask != 0 {
            let bit = mask.trailing_zeros();
            let flag = 1 << bit;
            let source = bit + (i as u32 % 2) * 32;
            let root_value = |s: &mut Sse| -> Result<u32, Error> {
                let handle = m.u32(m.u32(SHADER_INPUT)?)?;
                let value = root(m, handle).ok_or(Error::Data)?;
                s.clamp(value, 0, m.u32(ONE)?)
            };
            match i {
                0 | 1 => {
                    let value = if source == 37 { root_value(&mut s)? } else { scalar(m, &mut s, source)? };
                    m.set_u32(cache + SCALAR + 4*source, value)?;
                }
                2 | 3 => {
                    let value = if source == 40 { [root_value(&mut s)?; 3] } else { vector(m, &mut s, source, fcw)? };
                    for (j, v) in value.into_iter().enumerate() {
                        m.set_u32(cache + VECTOR + 12*source + j as u32*4, v)?;
                    }
                }
                _ => {
                    let value = integer(m, cache, bit)?;
                    m.set_u32(cache + INTEGER + 4*bit, value)?;
                    let changed = m.u32(cache+0x58)? | flag;
                    m.set_u32(cache+0x58, changed)?;
                }
            }
            let remaining = m.u32(dirty)? & !flag;
            m.set_u32(dirty, remaining)?;
            mask &= !flag;
        }
    }
    Ok(())
}

/// The scalar input selector at 0x336f0. Global constants and view
/// properties are read from the render context at the evaluation point.
fn scalar(m: &dyn GuestMemory, s: &mut Sse, input: u32) -> Result<u32, Error> {
    let material = 0x0048_5768;
    let fade = 0x0048_5858;
    Ok(match input {
        0 | 2 | 13 | 38 => 0,
        3 | 33 => m.u32(material+0x90)?,
        4 | 25 => m.u32(fade+12)?,
        10 | 24 => m.u32(material+12)?,
        12 | 22 => m.u32(ONE)?,
        16 => {
            let weight = m.u32(0x0048_5a30)?;
            let one = m.u32(ONE)?;
            if s.above(one, weight)? { weight } else { one }
        }
        17 => m.u32(0x0048_5a30)?,
        18 | 27 => s.mul(m.u32(fade+12)?, m.u32(material+12)?)?,
        19 | 36 => s.mul(m.u32(fade)?, m.u32(material+0x90)?)?,
        20 | 35 => {
            let weight = s.sub(m.u32(ONE)?, m.u32(fade)?)?;
            s.mul(weight, m.u32(material+0x90)?)?
        }
        21 => m.u32(0x0048_5a6c)?,
        23 => {
            let p = m.u32(SHADER_INPUT)?;
            let one = m.u32(ONE)?;
            if p == 0 { return Ok(one); }
            let channel = |s: &mut Sse, offset: u32| -> Result<u32, Error> {
                let sum = s.add(m.u32(p+0x10+offset)?, m.u32(p+0x30+offset)?)?;
                s.mul(sum, m.u32(0x0045_dcb4+offset)?)
            };
            let blue = channel(s, 8)?;
            let green = channel(s, 4)?;
            let red = channel(s, 0)?;
            let bg = s.add(blue, green)?;
            let rgb = s.add(bg, red)?;
            s.clamp(rgb, 0, one)?
        }
        26 => {
            let weight = s.sub(m.u32(ONE)?, m.u32(fade+12)?)?;
            s.mul(weight, m.u32(material+0x90)?)?
        }
        28 => m.u32(material+0x24)?,
        29 => m.u32(fade+0x10)?,
        30 => {
            let weight = s.sub(m.u32(ONE)?, m.u32(fade+0x10)?)?;
            s.mul(weight, m.u32(material+0x24)?)?
        }
        31 => s.mul(m.u32(fade+0x10)?, m.u32(material+12)?)?,
        32 => m.u32(material+0x3c)?,
        34 => m.u32(fade)?,
        39 => if let Some(inv) = fog_reciprocal(m,s)? {
            let v = s.mul(inv, m.u32(0x004b_9f80)?)?;
            s.sub(0,v)?
        } else { 0 },
        40 => {
            let view = m.i32(0x004b_9ed8)?;
            if (0..4).contains(&view) { m.u32(0x004e_69c0+view as u32*4)? } else { m.u32(ONE)? }
        }
        41 | 42 => m.u32(0x004c_19b0+(input-41)*4)?,
        _ => return Err(Error::Data),
    })
}

fn fog_reciprocal(m: &dyn GuestMemory, s: &mut Sse) -> Result<Option<u32>, Error> {
    if m.u32(0x004b_9f5c)? == u32::MAX { return Ok(None); }
    let (near, far) = (m.u32(0x004b_9f80)?, m.u32(0x004b_9f84)?);
    if !s.above(far,near)? { return Ok(None); }
    let distance = s.sub(far,near)?;
    Ok(Some(s.div(m.u32(ONE)?,distance)?))
}

fn vector(m:&dyn GuestMemory, s:&mut Sse, input:u32, fcw:u16) -> Result<[u32;3],Error> {
    let material = 0x0048_5768;
    let at = match input {
        0 => 0x0048_5624,
        3 | 4 | 21 | 22 | 42..=45 => {
            let slot = match input { 3|4 => input-3, 21|22 => input-19, _ => input-42 };
            let p = m.u32(SHADER_INPUT)?;
            if p != 0 && slot < u32::from(m.u8(p+0x74)?) {
                let colour = m.u32(p+0x64+4*slot)?;
                let scale = m.u32(BYTE_TO_UNIT)?;
                let mut channels = [0;3];
                let mut x = X87::new(fcw);
                for (lane, shift) in channels.iter_mut().zip([16,8,0]) {
                    x.fild(((colour >> shift) & 255) as i32)?;
                    x.fmul_m(scale)?;
                    *lane = x.fstp_m32()?;
                }
                return Ok(channels);
            }
            m.u32(DEFAULT_COLOUR)?
        }
        5 => 0x0048_59dc,
        6 => 0x0048_59ec,
        14..=17 => lighting(m,[0x3c,0x30,0x54,0x48][(input-14) as usize])?,
        18 | 25 => material,
        19 | 31 => material+0x84,
        23 | 24 | 35 => {
            let one = m.u32(ONE)?;
            let (at, scale) = if input == 24 {
                (lighting(m,0x10)?,s.add(m.u32(0x0048_5a70)?,one)?)
            } else { (lighting(m,0x30)?,m.u32(0x0048_5a6c)?) };
            let mut value = words3(m,at)?;
            for lane in &mut value {
                let mut v = if input == 24 { s.mul(scale,*lane)? } else { s.mul(*lane,scale)? };
                if input == 35 { v = s.mul(v,m.u32(HALF)?)?; }
                *lane = s.clamp(v,0,one)?;
            }
            return Ok(value);
        }
        26 | 28 | 30 | 32 => {
            let base = material + match input { 26=>0,28=>0x18,30=>0x30,_=>0x84 };
            let mut v = words3(m,base)?;
            let factor = m.u32(base+12)?;
            for x in &mut v { *x = s.mul(*x,factor)?; }
            return Ok(v);
        }
        27 => material+0x18,
        29 => material+0x30,
        33 | 34 => VIEW_VECTORS + (input-33)*12,
        36..=39 => VIEW_VECTORS + 0x40 + (input-36)*12,
        41 => if let Some(inv) = fog_reciprocal(m,s)? {
            let mut v = words3(m,0x004b_9dac)?;
            for lane in &mut v {
                let scaled = s.mul(*lane,inv)?;
                *lane = s.sub(0,scaled)?;
            }
            return Ok(v);
        } else { m.u32(DEFAULT_TRANSFORM)?+4 },
        _ => return Err(Error::Data),
    };
    Ok(words3(m,at)?)
}

fn integer(m:&dyn GuestMemory, cache:u32, input:u32) -> Result<u32,Error> {
    let definition = || m.u32(0x0048_58c4);
    Ok(match input {
        0 | 1 => m.u32(0x0048_5a2c)?,
        2 | 3 => m.u16(definition()? + if input==2 { 0x8a } else { 0x80 })? as i16 as u32,
        4 | 5 => m.u32(0x0048_5a64+(input-4)*4)?,
        6 => u32::from(m.u8(definition()?)? & 2 == 0 && m.u32(0x004b_a014)? & 2 != 0 &&
            m.u32(m.u32(cache+0x14)?+2)? & (1<<13) != 0),
        7 => u32::from(m.u32(definition()?)? & 2 == 0),
        _ => return Err(Error::Data),
    })
}

/// Destination of an instance or animated colour, or an engine colour.
/// Selector equality is tested even for final-combiner writes. The signed
/// fragment offset applies only to an ordinary combiner register.
fn colour_destination(m: &dyn GuestMemory, cache: u32, target: u16)
    -> Result<Option<(u32, u32, u32)>, MemFault> {
    let mut reg = u32::from(target >> 6 & 15);
    let bank = u32::from(target >> 10 & 1);
    let fragments = m.u16(m.u32(cache+0x24)?+0xf6)?;
    let fin = reg == 8;
    if fragments >> 9 != 0 {
        let fragment = u32::from(target & 7);
        let list = m.u32(m.u32(cache+0x20)?+0x24)?;
        let index = list + (u32::from(fragments & 511)+fragment)*4;
        let selector = m.u16(index)? as i16;
        if selector != -1 && m.u32((cache+INTEGER).wrapping_add_signed(i32::from(selector)*4))?
                != u32::from(target >> 3 & 7) {
            return Ok(None);
        }
        if !fin {
            reg = reg.wrapping_add_signed(i32::from(m.u8(cache+PS_FRAGMENT_OFFSET+fragment)? as i8));
        }
    }
    let at = if fin { cache+PS_FINAL+bank*4 }
        else { (cache+PS_COLOUR+bank*32).wrapping_add(reg.wrapping_mul(4)) };
    Ok(Some((at, cache+PS_DIRTY+4*bank, 1u32.wrapping_shl(reg))))
}

fn put_colour(m: &mut dyn GuestMemory, dest: (u32,u32,u32), mask: u32, value: u32)
    -> Result<(),MemFault> {
    let (word,dirty,bit) = dest;
    let merged = (m.u32(word)? & !mask) | (value & mask);
    m.set_u32(word,merged)?;
    let bits = m.u32(dirty)? | bit;
    m.set_u32(dirty,bits)
}

pub fn write_colour(m:&mut dyn GuestMemory, cache:u32, target:u16, mode:u8, value:u32)
    -> Result<(),MemFault> {
    if let Some(dest) = colour_destination(m,cache,target)? {
        let mask = m.u32(LANE_MASK + 4*u32::from(mode & 7))?;
        put_colour(m,dest,mask,value)?;
    }
    Ok(())
}

fn bindings(m:&dyn GuestMemory,range:u32,list:u32) -> Result<std::ops::Range<u32>,MemFault> {
    let span = m.u16(range)?;
    let start = list.wrapping_add(u32::from(span & 511)*4);
    Ok(start..start.wrapping_add(u32::from(span >> 9)*4))
}

pub fn instance_colours(m: &mut dyn GuestMemory, cache:u32, range:u32) -> Result<(), MemFault> {
    let colours = m.u32(m.u32(cache+0xc)?+0x10)?;
    let list = m.u32(m.u32(cache+0x10)?+0x24)?;
    for binding in bindings(m,range,list)?.step_by(4) {
        let target = m.u16(binding)?;
        let index = u32::from(m.u8(binding+3)?);
        let mode = m.u8(binding+2)?;
        let value = m.u32(colours+index*4)?;
        write_colour(m,cache,target,mode,value)?;
    }
    Ok(())
}

pub(super) fn scalar_transform(m:&dyn GuestMemory, s:&mut Sse, value:u32, transform:u8, fcw:u16)
    -> Result<u32,Error> {
    if transform == 1 { return s.sub(m.u32(ONE)?,value); }
    if !(2..=5).contains(&transform) { return Ok(value); }
    let mut x = X87::new(fcw);
    x.fld(value)?;
    x.fmul_m(m.u32(WAVE_PERIOD)?)?;
    if transform <= 3 { x.fcos()?; } else { x.fsin()?; }
    if transform == 2 || transform == 4 {
        x.fadd_m(m.u32(ONE)?)?;
        x.fmul_m(m.u32(HALF)?)?;
    } else {
        x.fmul_m(m.u32(HALF)?)?;
        x.arith_m(x87::abi::arith_op::SUBR, m.u32(HALF)?)?;
    }
    // The threshold test uses ST(1), before narrowing to the stored float.
    let value = x.fst_m32()?;
    x.fld(m.u32(ZERO_THRESHOLD)?)?;
    if x.fcomip(1)? & 0x41 == 0 { return Ok(0); }
    let one = m.u32(ONE)?;
    Ok(if s.above(value,one)? { one } else { value })
}

/// One call of 0x1b2c0. Lanes not named by the mode keep their bits.
fn evaluate(m:&dyn GuestMemory, cache:u32, out:&mut [u32;4], mode:u8, stage:u32, source:u32,
    transform:u8, fcw:u16, s:&mut Sse) -> Result<(),Error> {
    match mode {
        0..=3 => {
            let input = m.u32(cache+SCALAR+source*4)?;
            out[usize::from(mode)] = scalar_transform(m,s,input,transform,fcw)?;
        }
        4 => out[..3].copy_from_slice(&words3(m,cache+VECTOR+source*12)?),
        5..=20 => {
            let one = m.u32(ONE)?;
            let (w,h) = if stage == 0 { (one,one) } else {
                let size = cache + 0x12e8 + stage*64;
                (s.int(m.i32(size)?)?,s.int(m.i32(size+4)?)?)
            };
            match mode {
                5..=8 => out[(mode-5) as usize] = w,
                9 => out[..2].copy_from_slice(&[w,h]),
                10 => out[2..].copy_from_slice(&[w,h]),
                11 => out[..2].fill(0),
                12 => out[2..].fill(0),
                13 => *out = [w,h,0,0],
                14 | 18 => *out = [w,0,0,0],
                15 | 19 => *out = [0,h,0,0],
                16 => out[..3].copy_from_slice(&[w,h,one]),
                17 => out[..3].fill(0),
                _ => *out = [0;4],
            }
        }
        _ => return Err(Error::Data),
    }
    Ok(())
}

pub fn vertex(m:&mut dyn GuestMemory, cache:u32, range:u32, mxcsr:u32) -> Result<(),Error> {
    let list = m.u32(m.u32(cache+0x20)?+0x3c)?;
    let mut s = Sse { mxcsr };
    for binding in bindings(m,range,list)?.step_by(4) {
        let w = u32::from(m.u16(binding)?);
        let source = u32::from(m.u8(binding+3)?);
        let bank = w >> 4 & 1;
        let reg = w & 15;
        let at = cache + if bank == 0 { VERTEX_DEFAULT } else { VERTEX_CONSTANT } + reg*16;
        let mode = ((w >> 5) & 31) as u8;
        let mut row = [0; 4];
        evaluate(m,cache,&mut row,mode,w>>13,source,0,0,&mut s)?;
        let (first, count) = match mode {
            0..=3 => (usize::from(mode), 1),
            4 | 16 | 17 => (0, 3),
            5..=8 => (usize::from(mode-5), 1),
            9 | 11 => (0, 2),
            10 | 12 => (2, 2),
            _ => (0, 4),
        };
        let mut bytes = [0; 16];
        for (dst, value) in bytes.chunks_exact_mut(4).zip(&row[first..first+count]) {
            dst.copy_from_slice(&value.to_le_bytes());
        }
        m.write(at+4*first as u32, &bytes[..4*count])?;
        let dirty = cache+VERTEX_DIRTY+bank*4;
        let bits = m.u32(dirty)? | (1<<reg);
        m.set_u32(dirty,bits)?;
    }
    Ok(())
}

/// Clamp and pack 0x131e50's four x87 products with the guest rounding
/// control. The caller's scratch row retains the remapped/clamped values
/// for the next binding's partial writes (0x19ca0).
pub fn pack(m:&dyn GuestMemory, row:&mut [u32;4], half:bool, fcw:u16,mxcsr:u32) -> Result<u32,Error> {
    let mut s = Sse { mxcsr };
    let mut x = X87::new(fcw);
    let scale = m.u32(UNIT_TO_BYTE)?;
    let one = m.u32(ONE)?;
    let mut packed = 0;
    for (lane,shift) in row.iter_mut().zip([16,8,0,24]) {
        if half {
            let biased = s.add(*lane,one)?;
            *lane = s.mul(biased,m.u32(HALF)?)?;
        }
        *lane = s.clamp(*lane,0,one)?;
        // A finite, clamped colour times 255 fits in one byte. With
        // masked exceptions use the same precision-and-rounding model
        // as the dynamic render-state alpha setter.
        if fcw & 0x3f == 0x3f && scale == 255.0f32.to_bits() && f32::from_bits(*lane).is_finite() {
            let byte = super::states::alpha_value(f32::from_bits(*lane),255.0,fcw) >> 24;
            packed |= byte << shift;
            continue;
        }
        x.fld(*lane)?;
        x.fmul_m(scale)?;
        let (code,store) = x87::abi::st_i32(&x.0,0);
        X87::ok(code)?;
        x87::abi::store_commit(&mut x.0,store.status,true);
        packed |= (store.value as u32) << shift;
    }
    Ok(packed)
}

pub fn pixel(m:&mut dyn GuestMemory, cache:u32,range:u32,fcw:u16,mxcsr:u32) -> Result<(),Error> {
    let list = m.u32(m.u32(cache+0x20)?+0x3c)?;
    let mut s = Sse { mxcsr };
    let mut row = [0;4];
    for binding in bindings(m,range,list)?.step_by(4) {
        let w = m.u16(binding)?;
        let Some(dest) = colour_destination(m,cache,w)? else { continue };
        let mode = m.u8(binding+2)?;
        let mask = m.u32(LANE_MASK + u32::from(mode & 7)*4)?;
        let source = u32::from(m.u8(binding+3)?);
        evaluate(m,cache,&mut row,mode & 7,0,source,mode >> 3 & 7,fcw,&mut s)?;
        let colour = pack(m,&mut row,w & (1<<11) != 0,fcw,mxcsr)?;
        put_colour(m,dest,mask,colour)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;
    #[test]
    fn pixel_register_writes_select_fragments_and_accumulate_dirty_bits() {
        let c = 0x1000;
        let mut m = FlatMemory::new(c,0x9000);
        m.set_u32(c+0x24,0x3000).unwrap();
        m.set_u32(c+0x20,0x3400).unwrap();
        m.set_u32(0x3400+0x24,0x3800).unwrap();
        m.set_u16(0x3000+0xf6,(2<<9)|3).unwrap();
        m.set_u16(0x3800+(3+1)*4,4).unwrap();
        m.set_u32(c+INTEGER+4*4,2).unwrap();
        let target = 1 | (3<<3) | 0x400 | (2<<6);
        assert_eq!(colour_destination(&m,c,target).unwrap(),None);
        m.set_u32(c+INTEGER+4*4,3).unwrap();
        m.set_u8(c+PS_FRAGMENT_OFFSET+1,3).unwrap();
        let word = c+PS_COLOUR+32+5*4;
        let dirty = c+PS_DIRTY+4;
        m.set_u32(word,0xabcd_2345).unwrap();
        m.set_u32(dirty,0x88).unwrap();
        let dest = colour_destination(&m,c,target).unwrap().unwrap();
        put_colour(&mut m,dest,0x00ff_ff00,0x9a43_65ef).unwrap();
        assert_eq!(m.u32(word),Ok(0xab43_6545));
        assert_eq!(m.u32(dirty),Ok(0xa8));
    }

    #[test]
    fn input_walk_consumes_only_the_selected_dirty_bits() {
        let cache = 0x1800;
        let mut m = FlatMemory::new(0, 0x4d0000);
        m.set_u32(ONE, 1.0f32.to_bits()).unwrap();
        let (scalar_low, scalar_high, vector_low, integers) =
            (cache+0x30, cache+0x34, cache+0x38, cache+0x40);
        m.set_u32(scalar_low, (1<<21) | (1<<17)).unwrap();
        m.set_u32(scalar_high, 1<<(41-32)).unwrap();
        m.set_u32(vector_low, 1<<5).unwrap();
        m.set_u32(integers, 1<<4).unwrap();
        m.set_u32(cache+0x44, 1<<21).unwrap();
        m.set_u32(cache+0x48, 1<<(41-32)).unwrap();
        m.set_u32(cache+0x4c, 1<<5).unwrap();
        m.set_u32(cache+0x54, 1<<4).unwrap();
        m.set_u32(cache+0x58, u32::MAX).unwrap();
        m.set_u32(0x0048_5a6c, 0x1234_0000).unwrap();
        m.set_u32(0x004c_19b0, 0x1234_0001).unwrap();
        for n in 0..3 { m.set_u32(0x0048_59dc+n*4, 0x400+n).unwrap(); }
        m.set_u32(0x0048_5a64, 2).unwrap();
        let root = |_:&dyn GuestMemory,_:u32| -> Option<u32> { unreachable!() };
        inputs(&mut m, cache, 0x37f, 0x1f80, root).unwrap();
        assert_eq!(m.u32(cache+SCALAR+21*4), Ok(0x1234_0000));
        assert_eq!(m.u32(cache+SCALAR+41*4), Ok(0x1234_0001));
        assert_eq!(words3(&m,cache+VECTOR+5*12), Ok([0x400,0x401,0x402]));
        assert_eq!(m.u32(cache+INTEGER+4*4), Ok(2));
        assert_eq!(m.u32(scalar_low), Ok(1<<17));
        for a in [scalar_high,vector_low,integers] { assert_eq!(m.u32(a),Ok(0)); }
        assert_eq!(m.u32(cache+0x58), Ok(1<<4));
        inputs(&mut m, cache, 0x37f, 0x1f80, root).unwrap();
        assert_eq!(m.u32(cache+0x58), Ok(0));
    }

    #[test]
    fn stage_sizes_replace_only_the_named_components_and_dirty_the_right_bank() {
        let (cache, definition, list, range) = (0x1000,0x3000,0x4000,0x4200);
        let mut m = FlatMemory::new(0,0x460000);
        m.set_u32(ONE, 1.0f32.to_bits()).unwrap();
        m.set_u32(cache+0x20, definition).unwrap();
        m.set_u32(definition+0x3c, list).unwrap();
        m.set_u16(range, (1<<9)|1).unwrap();
        // Stage two, mode 10 (size in zw), vertex constant bank, reg 7.
        m.set_u16(list+4, (2<<13)|(10<<5)|(1<<4)|7).unwrap();
        m.set_i32(cache+0x12e8+2*64, 512).unwrap();
        m.set_i32(cache+0x12ec+2*64, 256).unwrap();
        let dest = cache+VERTEX_CONSTANT+7*16;
        m.set_u32(dest, 0x8000_0000).unwrap();
        m.set_u32(dest+4, 0x7fc0_0001).unwrap();
        m.set_u32(cache+VERTEX_DIRTY+4, 1<<9).unwrap();
        vertex(&mut m,cache,range,0x1f80).unwrap();
        assert_eq!(m.u32(dest), Ok(0x8000_0000));
        assert_eq!(m.u32(dest+4), Ok(0x7fc0_0001));
        assert_eq!(m.u32(dest+8), Ok(512.0f32.to_bits()));
        assert_eq!(m.u32(dest+12), Ok(256.0f32.to_bits()));
        assert_eq!(m.u32(cache+VERTEX_DIRTY), Ok(0));
        assert_eq!(m.u32(cache+VERTEX_DIRTY+4), Ok((1<<7)|(1<<9)));
    }

    #[test]
    fn colour_pack_uses_fistp_rounding_and_keeps_the_clamped_scratch_row() {
        let mut m = FlatMemory::new(0x0045_0000, 0x10000);
        m.set_u32(ONE, 1.0f32.to_bits()).unwrap();
        m.set_u32(UNIT_TO_BYTE, 255.0f32.to_bits()).unwrap();
        let mut row = [0.5f32,1.5,-0.25,1.0].map(f32::to_bits);
        assert_eq!(pack(&m,&mut row,false,0x037f,0x1f80).unwrap(),0xff80_ff00);
        assert_eq!(row, [0.5f32,1.0,0.0,1.0].map(f32::to_bits));
        assert_eq!(pack(&m,&mut row,false,0x077f,0x1f80).unwrap(),0xff7f_ff00);
    }
}
