//! The fake guest every test uses: sparse 4 KiB pages, zeroed when mapped,
//! and a fault on any access that touches an unmapped page.
//!
//! It is ordinary code (not `cfg(test)`) so other crates' tests can build
//! guest state with it. It holds no game data: tests write synthetic bytes.

use std::collections::BTreeMap;

use crate::memory::{GuestMemory, MemFault};

const PAGE: u32 = 0x1000;

/// Sparse guest memory for tests. See the module docs.
#[derive(Clone, Debug, Default)]
pub struct FakeGuest {
    pages: BTreeMap<u32, Box<[u8; PAGE as usize]>>,
}

impl FakeGuest {
    pub fn new() -> Self {
        Self::default()
    }

    /// Maps (zeroed) every page that `addr..addr + len` touches. Pages that
    /// are already mapped keep their bytes.
    pub fn map(&mut self, addr: u32, len: u32) -> &mut Self {
        if len == 0 {
            return self;
        }
        let first = addr & !(PAGE - 1);
        let last = addr.wrapping_add(len - 1) & !(PAGE - 1);
        let mut p = first;
        loop {
            self.pages
                .entry(p)
                .or_insert_with(|| Box::new([0; PAGE as usize]));
            if p == last {
                break;
            }
            p = p.wrapping_add(PAGE);
        }
        self
    }

    /// Maps the pages under `addr..addr + data.len()` and writes `data`.
    pub fn put(&mut self, addr: u32, data: &[u8]) -> &mut Self {
        self.map(addr, data.len() as u32);
        self.write(addr, data).expect("just mapped");
        self
    }

    pub fn put_u8(&mut self, addr: u32, v: u8) -> &mut Self {
        self.put(addr, &[v])
    }
    pub fn put_u16(&mut self, addr: u32, v: u16) -> &mut Self {
        self.put(addr, &v.to_le_bytes())
    }
    pub fn put_u32(&mut self, addr: u32, v: u32) -> &mut Self {
        self.put(addr, &v.to_le_bytes())
    }
    pub fn put_f32(&mut self, addr: u32, v: f32) -> &mut Self {
        self.put_u32(addr, v.to_bits())
    }

    pub fn is_mapped(&self, addr: u32) -> bool {
        self.pages.contains_key(&(addr & !(PAGE - 1)))
    }

    /// Visits each byte range of `addr..addr + len` page by page; `None`
    /// when a page is unmapped or the range wraps past 4 GiB.
    fn spans(&self, addr: u32, len: usize) -> Option<Vec<(u32, usize, usize)>> {
        if addr as u64 + len as u64 > 1 << 32 {
            return None;
        }
        let mut out = Vec::new();
        let mut done = 0usize;
        while done < len {
            let at = addr + done as u32;
            let page = at & !(PAGE - 1);
            if !self.pages.contains_key(&page) {
                return None;
            }
            let off = (at - page) as usize;
            let n = (PAGE as usize - off).min(len - done);
            out.push((page, off, n));
            done += n;
        }
        Some(out)
    }
}

impl GuestMemory for FakeGuest {
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        let fault = MemFault {
            addr,
            len: buf.len() as u32,
            write: false,
        };
        let mut at = 0;
        for (page, off, n) in self.spans(addr, buf.len()).ok_or(fault)? {
            buf[at..at + n].copy_from_slice(&self.pages[&page][off..off + n]);
            at += n;
        }
        Ok(())
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        let fault = MemFault {
            addr,
            len: data.len() as u32,
            write: true,
        };
        let spans = self.spans(addr, data.len()).ok_or(fault)?;
        let mut at = 0;
        for (page, off, n) in spans {
            let p = self.pages.get_mut(&page).expect("checked");
            p[off..off + n].copy_from_slice(&data[at..at + n]);
            at += n;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapped_pages_read_back_and_unmapped_ones_fault() {
        let mut g = FakeGuest::new();
        g.put_u32(0x1ffe, 0xdead_beef); // straddles two pages
        assert_eq!(g.u32(0x1ffe), Ok(0xdead_beef));
        assert!(g.is_mapped(0x1000) && g.is_mapped(0x2000));
        assert_eq!(
            g.u32(0x2ffe).unwrap_err().addr,
            0x2ffe,
            "0x3000 is unmapped"
        );
        assert!(g.set_u32(0x5000, 1).is_err());
        assert!(g.u8(0xffff_ffff).is_err());
        g.map(0xffff_f000, 0x1000);
        assert!(g.u32(0xffff_fffe).is_err(), "no wrap past 4 GiB");
    }

    #[test]
    fn a_faulting_write_writes_nothing() {
        let mut g = FakeGuest::new();
        g.map(0x1000, 0x1000);
        assert!(g.write(0x1ffe, &[1, 2, 3, 4]).is_err());
        assert_eq!(g.u16(0x1ffe), Ok(0));
    }

    #[test]
    fn strings_and_floats() {
        let mut g = FakeGuest::new();
        g.put(0x1000, b"halo\0").put_f32(0x1010, 1.5);
        g.put(0x1020, &[b'h', 0, b'i', 0, 0, 0]);
        assert_eq!(g.cstr(0x1000, 16).unwrap(), b"halo");
        assert!(g.cstr(0x1000, 3).is_err(), "no NUL within the limit");
        assert_eq!(g.utf16(0x1020, 8).unwrap(), "hi");
        assert_eq!(g.f32(0x1010), Ok(1.5));
        assert_eq!(g.vec3(0x1010).unwrap()[0], 1.5);
    }
}
