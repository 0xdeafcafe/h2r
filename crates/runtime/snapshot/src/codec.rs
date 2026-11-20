//! Little-endian field encoding shared by every section. Lengths are
//! checked against what is left before anything is allocated, so a
//! damaged or hostile file costs at most its own size.
use crate::Error;

#[derive(Default)]
pub struct Writer {
    pub(crate) bytes: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.bytes.push(v);
        self
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.u8(v as u8)
    }
    /// Raw bytes, no length.
    pub fn raw(&mut self, v: &[u8]) -> &mut Self {
        self.bytes.extend_from_slice(v);
        self
    }
    /// A u32 length, then the bytes.
    pub fn blob(&mut self, v: &[u8]) -> &mut Self {
        self.u32(u32::try_from(v.len()).expect("blob over 4 GB"));
        self.raw(v)
    }
    pub fn str(&mut self, v: &str) -> &mut Self {
        self.blob(v.as_bytes())
    }
    /// A u32 count, then each item.
    pub fn list<T>(&mut self, items: &[T], mut f: impl FnMut(&mut Self, &T)) -> &mut Self {
        self.u32(u32::try_from(items.len()).expect("list over 4G items"));
        for item in items {
            f(self, item);
        }
        self
    }
}

pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    /// Named in errors.
    what: &'static str,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8], what: &'static str) -> Self {
        Self { bytes, at: 0, what }
    }
    pub fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }
    fn short(&self) -> Error {
        Error::Truncated(self.what)
    }
    pub fn raw(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if n > self.remaining() {
            return Err(self.short());
        }
        let out = &self.bytes[self.at..self.at + n];
        self.at += n;
        Ok(out)
    }
    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        Ok(self.raw(N)?.try_into().expect("length checked"))
    }
    pub fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.array::<1>()?[0])
    }
    pub fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    pub fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.array()?))
    }
    pub fn bool(&mut self) -> Result<bool, Error> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::Corrupt(self.what, "boolean is not 0 or 1".into())),
        }
    }
    pub fn blob(&mut self) -> Result<&'a [u8], Error> {
        let n = self.u32()? as usize;
        self.raw(n)
    }
    pub fn str(&mut self) -> Result<&'a str, Error> {
        let what = self.what;
        std::str::from_utf8(self.blob()?)
            .map_err(|_| Error::Corrupt(what, "string is not UTF-8".into()))
    }
    /// A u32 count, then each item. `min_item` is the fewest bytes one item
    /// can take, so a count larger than the input can hold is refused
    /// before allocating.
    pub fn list<T>(
        &mut self,
        min_item: usize,
        mut f: impl FnMut(&mut Self) -> Result<T, Error>,
    ) -> Result<Vec<T>, Error> {
        let n = self.u32()? as usize;
        if n.saturating_mul(min_item.max(1)) > self.remaining() {
            return Err(self.short());
        }
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(f(self)?);
        }
        Ok(out)
    }
    /// Every byte was read; trailing bytes mean a writer this reader does
    /// not understand.
    pub fn finish(&self) -> Result<(), Error> {
        if self.remaining() != 0 {
            return Err(Error::Corrupt(
                self.what,
                format!("{} trailing bytes", self.remaining()),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_round_trip() {
        let mut w = Writer::new();
        w.u8(7)
            .u32(0xdead_beef)
            .u64(u64::MAX - 1)
            .bool(true)
            .str("courtyard");
        w.list(&[1u32, 2, 3], |w, v| {
            w.u32(*v);
        });
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes, "test");
        assert_eq!(r.u8().unwrap(), 7);
        assert_eq!(r.u32().unwrap(), 0xdead_beef);
        assert_eq!(r.u64().unwrap(), u64::MAX - 1);
        assert!(r.bool().unwrap());
        assert_eq!(r.str().unwrap(), "courtyard");
        assert_eq!(r.list(4, |r| r.u32()).unwrap(), vec![1, 2, 3]);
        r.finish().unwrap();
    }

    #[test]
    fn short_and_oversized_inputs_are_refused_without_allocating() {
        let mut r = Reader::new(&[1, 2], "test");
        assert_eq!(r.u32(), Err(Error::Truncated("test")));
        // A count of 4 billion items over a 4-byte input.
        let bytes = u32::MAX.to_le_bytes();
        let mut r = Reader::new(&bytes, "test");
        assert_eq!(r.list(1, |r| r.u8()), Err(Error::Truncated("test")));
        let mut r = Reader::new(&[2], "test");
        assert!(matches!(r.bool(), Err(Error::Corrupt(..))));
        let r = Reader::new(&[0], "test");
        assert!(matches!(r.finish(), Err(Error::Corrupt(..))));
    }
}
