//! Read access to guest memory for a kernel's gather step. The engine
//! implements [`GuestRead`] over its guest; tests implement it over a
//! byte map. No runtime dependency: a kernel never holds guest memory
//! across a call.

/// A read that failed: the first guest address that is not mapped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fault(pub u32);

pub trait GuestRead {
    fn u32(&self, addr: u32) -> Result<u32, Fault>;
    fn u16(&self, addr: u32) -> Result<u16, Fault>;
    fn bytes(&self, addr: u32, out: &mut [u8]) -> Result<(), Fault>;

    fn u8(&self, addr: u32) -> Result<u8, Fault> {
        let mut b = [0];
        self.bytes(addr, &mut b)?;
        Ok(b[0])
    }

    fn f32(&self, addr: u32) -> Result<f32, Fault> {
        self.u32(addr).map(f32::from_bits)
    }

    fn vec(&self, addr: u32, len: usize) -> Result<Vec<u8>, Fault> {
        let mut v = vec![0; len];
        self.bytes(addr, &mut v)?;
        Ok(v)
    }
}

/// A sparse byte map, for tests.
#[derive(Default, Debug)]
pub struct Bytes(pub std::collections::BTreeMap<u32, u8>);

impl Bytes {
    pub fn put(&mut self, addr: u32, data: &[u8]) {
        for (i, b) in data.iter().enumerate() {
            self.0.insert(addr + i as u32, *b);
        }
    }
    pub fn put_u32(&mut self, addr: u32, v: u32) {
        self.put(addr, &v.to_le_bytes());
    }
}

impl GuestRead for Bytes {
    fn u32(&self, addr: u32) -> Result<u32, Fault> {
        let mut b = [0; 4];
        self.bytes(addr, &mut b)?;
        Ok(u32::from_le_bytes(b))
    }
    fn u16(&self, addr: u32) -> Result<u16, Fault> {
        let mut b = [0; 2];
        self.bytes(addr, &mut b)?;
        Ok(u16::from_le_bytes(b))
    }
    fn bytes(&self, addr: u32, out: &mut [u8]) -> Result<(), Fault> {
        for (i, o) in out.iter_mut().enumerate() {
            let a = addr.wrapping_add(i as u32);
            *o = *self.0.get(&a).ok_or(Fault(a))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_little_endian_and_names_the_first_unmapped_byte() {
        let mut m = Bytes::default();
        m.put_u32(0x100, 0x1122_3344);
        assert_eq!(m.u32(0x100), Ok(0x1122_3344));
        assert_eq!(m.u16(0x102), Ok(0x1122));
        assert_eq!(m.u8(0x103), Ok(0x11));
        assert_eq!(m.u32(0x102), Err(Fault(0x104)));
    }
}
