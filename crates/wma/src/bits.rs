use crate::{Error, Result};

/// Checked MSB-first reader. Lookahead is zero-extended, consumption never is.
#[derive(Clone)]
pub(crate) struct Bits<'a> {
    pub data: &'a [u8],
    pub pos: usize,
    pub end: usize,
}
impl<'a> Bits<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            end: data.len() * 8,
        }
    }
    pub fn remaining(&self) -> usize {
        self.end - self.pos
    }
    pub fn peek(&self, n: usize) -> u32 {
        let mut v = 0;
        for i in 0..n {
            v <<= 1;
            if self.pos + i < self.end {
                v |= u32::from((self.data[(self.pos + i) / 8] >> (7 - (self.pos + i) % 8)) & 1);
            }
        }
        v
    }
    pub fn read(&mut self, n: usize) -> Result<u32> {
        if n > 32 || n > self.remaining() {
            return Err(Error::Truncated { bit: self.pos });
        }
        let v = self.peek(n);
        self.pos += n;
        Ok(v)
    }
    pub fn skip(&mut self, n: usize) -> Result<()> {
        if n > self.remaining() {
            return Err(Error::Truncated { bit: self.pos });
        }
        self.pos += n;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn msb_checked() {
        let mut b = Bits::new(&[0x81, 0x7f]);
        assert_eq!(b.read(1).unwrap(), 1);
        assert_eq!(b.read(8).unwrap(), 2);
        assert_eq!(b.peek(8), 254);
        assert!(b.read(8).is_err());
        assert_eq!(b.read(7).unwrap(), 127);
        assert!(b.read(1).is_err());
    }
}
