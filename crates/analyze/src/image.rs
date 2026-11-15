//! What each byte of the code sections has been claimed as.

use std::borrow::Cow;
use std::collections::HashMap;

use xbe::Xbe;

/// Sections that carry the executable flag on Xbox but hold data.
pub const DATA_SECTIONS: &[&str] = &[
    ".rdata", ".data", ".data1", "DOLBY", "XON_RD", "BINKDATA", "$$XTIMAGE", "$$XSIMAGE",
    "DSPImage", ".XTLID",
];

pub fn is_code_section(s: &xbe::Section) -> bool {
    s.flags.executable() && !DATA_SECTIONS.contains(&s.name.as_str())
}

/// A code section's bytes as the loader maps them: its raw data, cut to the
/// virtual size, or zero-filled up to it. Two Bink sections end one byte
/// short of their last instruction (the `00` of a `ret 4`) and rely on the
/// fill.
pub fn mapped_bytes<'a>(xbe: &Xbe<'a>, s: &xbe::Section) -> Result<Cow<'a, [u8]>, xbe::Error> {
    let raw = xbe.section_bytes(s)?;
    let size = s.vsize as usize;
    Ok(if raw.len() >= size {
        Cow::Borrowed(&raw[..size])
    } else {
        let mut b = raw.to_vec();
        b.resize(size, 0);
        Cow::Owned(b)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Byte {
    Unknown,
    InsnStart,
    InsnBody,
    Data,
}

/// Why a decode was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConflictKind {
    /// Bytes don't decode as an instruction.
    InvalidOpcode,
    /// An instruction would start inside another one.
    IntoInsnBody,
    /// An instruction would start on bytes claimed as data.
    IntoData,
    /// An instruction's tail would run over already-claimed bytes.
    OverlapsClaimed,
    /// Runs off the end of the section.
    RunsOffSection,
    /// Control flow leaves the code sections.
    OutsideCode,
    /// A jump table entry points outside the function's section.
    BadJumpTable,
    /// A path ends on int3/ud2 without a call that never returns.
    FallsIntoTrap,
}

pub struct Region<'a> {
    pub name: String,
    pub va: u32,
    pub bytes: Cow<'a, [u8]>,
    /// Bytes backed by the file; the rest of `bytes` is the loader's zero
    /// fill up to the virtual size.
    pub raw: usize,
    pub state: Vec<Byte>,
    /// Function whose exploration claimed each byte (0 = none).
    pub owner: Vec<u32>,
}

impl Region<'_> {
    pub fn contains(&self, va: u32) -> bool {
        va >= self.va && ((va - self.va) as usize) < self.bytes.len()
    }
}

pub struct Image<'a> {
    pub regions: Vec<Region<'a>>,
    /// Functions whose claimed bytes are treated as free, for re-trying a
    /// candidate as if a weaker competitor weren't there.
    pub yielding: Option<u32>,
}

/// Uncommitted claims made while trying out a speculative function.
#[derive(Default)]
pub struct Overlay {
    pub bytes: HashMap<u32, Byte>,
}

impl<'a> Image<'a> {
    pub fn new(xbe: &'a Xbe<'a>) -> Result<Self, xbe::Error> {
        let mut regions = Vec::new();
        for s in xbe.sections.iter().filter(|s| is_code_section(s)) {
            let bytes = mapped_bytes(xbe, s)?;
            let len = bytes.len();
            regions.push(Region {
                name: s.name.clone(),
                va: s.va,
                bytes,
                raw: len.min(s.raw_size as usize),
                state: vec![Byte::Unknown; len],
                owner: vec![0; len],
            });
        }
        Ok(Self { regions, yielding: None })
    }

    pub fn region(&self, va: u32) -> Option<usize> {
        self.regions.iter().position(|r| r.contains(va))
    }

    pub fn state(&self, va: u32) -> Option<Byte> {
        let r = &self.regions[self.region(va)?];
        Some(r.state[(va - r.va) as usize])
    }

    /// Bytes from `va` to the end of its section.
    pub fn bytes_from(&self, va: u32) -> Option<&[u8]> {
        let r = &self.regions[self.region(va)?];
        Some(&r.bytes[(va - r.va) as usize..])
    }

    fn owner(&self, va: u32) -> u32 {
        self.region(va).map_or(0, |ri| {
            let r = &self.regions[ri];
            r.owner[(va - r.va) as usize]
        })
    }

    fn effective(&self, va: u32, overlay: Option<&Overlay>) -> Option<Byte> {
        let committed = self.state(va)?;
        let yielded = self.yielding.is_some_and(|y| committed != Byte::Unknown && self.owner(va) == y);
        if committed != Byte::Unknown && !yielded {
            return Some(committed);
        }
        Some(overlay.and_then(|o| o.bytes.get(&va).copied()).unwrap_or(Byte::Unknown))
    }

    /// Check an instruction at `va` against claimed bytes. `Ok(true)` means
    /// the same instruction is already claimed there (x86 decoding is
    /// deterministic, so a matching start means a matching instruction).
    pub fn check_insn(&self, va: u32, len: usize, overlay: Option<&Overlay>) -> Result<bool, ConflictKind> {
        let ri = self.region(va).ok_or(ConflictKind::OutsideCode)?;
        let r = &self.regions[ri];
        if (va - r.va) as usize + len > r.bytes.len() {
            return Err(ConflictKind::RunsOffSection);
        }
        match self.effective(va, overlay).unwrap() {
            Byte::InsnStart => return Ok(true),
            Byte::InsnBody => return Err(ConflictKind::IntoInsnBody),
            Byte::Data => return Err(ConflictKind::IntoData),
            Byte::Unknown => {}
        }
        for i in 1..len as u32 {
            if self.effective(va + i, overlay) != Some(Byte::Unknown) {
                return Err(ConflictKind::OverlapsClaimed);
            }
        }
        Ok(false)
    }

    /// Check a run of data (e.g. a jump table) against claimed bytes. Data
    /// may be claimed twice (two switches sharing a table).
    pub fn check_data(&self, va: u32, len: usize, overlay: Option<&Overlay>) -> Result<(), ConflictKind> {
        for i in 0..len as u32 {
            match self.effective(va + i, overlay) {
                None => return Ok(()), // table outside code sections: nothing to claim
                Some(Byte::Unknown | Byte::Data) => {}
                Some(_) => return Err(ConflictKind::OverlapsClaimed),
            }
        }
        Ok(())
    }

    pub fn set(&mut self, va: u32, b: Byte, owner: u32) {
        if let Some(ri) = self.region(va) {
            let r = &mut self.regions[ri];
            r.state[(va - r.va) as usize] = b;
            r.owner[(va - r.va) as usize] = owner;
        }
    }

    pub fn claim_insn(&mut self, va: u32, len: usize, owner: u32) {
        self.set(va, Byte::InsnStart, owner);
        for i in 1..len as u32 {
            self.set(va + i, Byte::InsnBody, owner);
        }
    }

    pub fn claim_data(&mut self, va: u32, len: usize, owner: u32) {
        for i in 0..len as u32 {
            self.set(va + i, Byte::Data, owner);
        }
    }

    /// Instruction starts claimed by function `owner`.
    pub fn starts_owned_by(&self, owner: u32) -> Vec<u32> {
        let Some(r) = self.region(owner).map(|i| &self.regions[i]) else { return Vec::new() };
        (0..r.state.len())
            .filter(|&i| r.owner[i] == owner && r.state[i] == Byte::InsnStart)
            .map(|i| r.va + i as u32)
            .collect()
    }

    /// The first owner of a claimed byte in `[va, va + len)`.
    pub fn owner_in(&self, va: u32, len: u32) -> Option<u32> {
        (0..len).find_map(|i| {
            let ri = self.region(va + i)?;
            let r = &self.regions[ri];
            let o = r.owner[(va + i - r.va) as usize];
            (o != 0).then_some(o)
        })
    }
}

impl Overlay {
    pub fn claim_insn(&mut self, va: u32, len: usize) {
        self.bytes.insert(va, Byte::InsnStart);
        for i in 1..len as u32 {
            self.bytes.insert(va + i, Byte::InsnBody);
        }
    }

    pub fn claim_data(&mut self, va: u32, len: usize) {
        for i in 0..len as u32 {
            self.bytes.entry(va + i).or_insert(Byte::Data);
        }
    }
}
