//! XcSHAInit / XcSHAUpdate / XcSHAFinal (kernel 335-337), a real SHA-1
//! whose running state is kept in the guest's opaque context buffer:
//! h[5] (20 bytes), total length (8), pending block (64), pending length (4).

use kernel::abi::{Args, Value};
use runtime::{Guest, GuestFault};

fn compress(h: &mut [u32; 5], block: &[u8]) {
    let mut w = [0u32; 80];
    for i in 0..16 {
        w[i] = u32::from_be_bytes(block[4 * i..4 * i + 4].try_into().unwrap());
    }
    for i in 16..80 {
        w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
    }
    let [mut a, mut b, mut c, mut d, mut e] = *h;
    for (i, wi) in w.iter().enumerate() {
        let (f, k) = match i {
            0..=19 => ((b & c) | (!b & d), 0x5a82_7999),
            20..=39 => (b ^ c ^ d, 0x6ed9_eba1),
            40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
            _ => (b ^ c ^ d, 0xca62_c1d6),
        };
        let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = t;
    }
    for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
        *x = x.wrapping_add(y);
    }
}

struct State {
    h: [u32; 5],
    total: u64,
    buf: Vec<u8>,
}

fn load(m: &Guest, p: u32) -> Result<State, GuestFault> {
    let mut h = [0u32; 5];
    for (i, x) in h.iter_mut().enumerate() {
        *x = m.u32(p + 4 * i as u32)?;
    }
    let total = m.u32(p + 20)? as u64 | (m.u32(p + 24)? as u64) << 32;
    let n = (m.u32(p + 92)? as usize).min(64);
    let mut buf = vec![0u8; n];
    m.read_bytes(p + 28, &mut buf, 1)?;
    Ok(State { h, total, buf })
}

fn store(m: &mut Guest, p: u32, s: &State) -> Result<(), GuestFault> {
    for (i, x) in s.h.iter().enumerate() {
        m.set_u32(p + 4 * i as u32, *x)?;
    }
    m.set_u32(p + 20, s.total as u32)?;
    m.set_u32(p + 24, (s.total >> 32) as u32)?;
    let mut b = [0u8; 64];
    b[..s.buf.len()].copy_from_slice(&s.buf);
    m.write_bytes(p + 28, &b, 1)?;
    m.set_u32(p + 92, s.buf.len() as u32)
}

fn update(s: &mut State, data: &[u8]) {
    s.total += data.len() as u64;
    s.buf.extend_from_slice(data);
    let full = s.buf.len() / 64 * 64;
    for c in s.buf[..full].chunks(64) {
        compress(&mut s.h, c);
    }
    s.buf.drain(..full);
}

pub fn init(a: &Args, m: &mut Guest) -> Result<Value, GuestFault> {
    let s = State { h: [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476, 0xc3d2_e1f0], total: 0, buf: vec![] };
    store(m, a.get(0), &s)?;
    Ok(Value::Void)
}

pub fn update_g(a: &Args, m: &mut Guest) -> Result<Value, GuestFault> {
    let (p, src, len) = (a.get(0), a.get(1), a.get(2));
    let mut s = load(m, p)?;
    let mut data = vec![0u8; len as usize];
    m.read_bytes(src, &mut data, 1)?;
    update(&mut s, &data);
    store(m, p, &s)?;
    Ok(Value::Void)
}

pub fn final_g(a: &Args, m: &mut Guest) -> Result<Value, GuestFault> {
    let (p, out) = (a.get(0), a.get(1));
    let mut s = load(m, p)?;
    let bits = s.total * 8;
    let mut pad = vec![0x80u8];
    while (s.buf.len() + pad.len()) % 64 != 56 {
        pad.push(0);
    }
    pad.extend_from_slice(&bits.to_be_bytes());
    let t = s.total;
    update(&mut s, &pad);
    s.total = t;
    let digest: Vec<u8> = s.h.iter().flat_map(|x| x.to_be_bytes()).collect();
    m.write_bytes(out, &digest, 1)?;
    Ok(Value::Void)
}

#[cfg(test)]
mod tests {
    #[test]
    fn abc() {
        let mut s = super::State { h: [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476, 0xc3d2_e1f0], total: 0, buf: vec![] };
        super::update(&mut s, b"abc");
        let mut pad = vec![0x80u8];
        while (s.buf.len() + pad.len()) % 64 != 56 {
            pad.push(0);
        }
        pad.extend_from_slice(&24u64.to_be_bytes());
        super::update(&mut s, &pad);
        assert_eq!(s.h, [0xa999_3e36, 0x4706_816a, 0xba3e_2571, 0x7850_c26c, 0x9cd0_d89d]);
    }
}
