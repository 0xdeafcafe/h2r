use crate::{Result, invalid};

pub(crate) fn word(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
pub(crate) fn short(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}
fn exact(b: &[u8], size: usize) -> Result<()> {
    if b.len() != size {
        return Err(invalid(format!("record size {}, expected {size}", b.len())));
    }
    Ok(())
}

/// A salted node handle. NULL is the only end-of-list sentinel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Datum(pub u32);
impl Datum {
    pub const NULL: Self = Self(u32::MAX);
    pub fn slot(self) -> usize {
        (self.0 & 0xffff) as usize
    }
    pub fn salt(self) -> u16 {
        (self.0 >> 16) as u16
    }
}

/// Twenty-byte wire record, including dead slots. Only salt != 0 is live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Node {
    pub(crate) salt: u16,
    /// Builtin/script index on calls; source value type on literal atoms.
    pub(crate) opcode: u16,
    pub(crate) value_type: u16,
    pub(crate) flags: u16,
    pub(crate) next: Datum,
    /// Token offset for symbolic atoms; obsolete source offset on other nodes.
    pub(crate) string_offset: u32,
    /// Child datum, global index, or literal bits, depending on flags/type.
    pub(crate) data: u32,
}
impl Node {
    pub fn salt(&self) -> u16 {
        self.salt
    }
    pub fn opcode(&self) -> u16 {
        self.opcode
    }
    pub fn value_type(&self) -> u16 {
        self.value_type
    }
    pub fn flags(&self) -> u16 {
        self.flags
    }
    pub fn next(&self) -> Datum {
        self.next
    }
    pub fn string_offset(&self) -> u32 {
        self.string_offset
    }
    pub fn data(&self) -> u32 {
        self.data
    }
    pub const SIZE: usize = 20;
    pub fn decode(b: &[u8]) -> Result<Self> {
        exact(b, Self::SIZE)?;
        Ok(Self {
            salt: short(b, 0),
            opcode: short(b, 2),
            value_type: short(b, 4),
            flags: short(b, 6),
            next: Datum(word(b, 8)),
            string_offset: word(b, 12),
            data: word(b, 16),
        })
    }
    pub fn encode(self) -> [u8; Self::SIZE] {
        let mut b = [0; Self::SIZE];
        for (i, v) in [self.salt, self.opcode, self.value_type, self.flags]
            .iter()
            .enumerate()
        {
            b[i * 2..i * 2 + 2].copy_from_slice(&v.to_le_bytes());
        }
        for (i, v) in [self.next.0, self.string_offset, self.data]
            .iter()
            .enumerate()
        {
            b[8 + i * 4..12 + i * 4].copy_from_slice(&v.to_le_bytes());
        }
        b
    }
    pub fn live(self) -> bool {
        self.salt != 0
    }
    pub fn atom(self) -> bool {
        self.flags & 1 != 0
    }
    pub fn script_call(self) -> bool {
        !self.atom() && self.flags & 2 != 0
    }
    pub fn global(self) -> bool {
        self.atom() && self.flags & 4 != 0
    }
}

/// Scenario script definition: name, kind, return type, expression root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    pub(crate) name_bytes: [u8; 32],
    pub(crate) kind: u16,
    pub(crate) return_type: u16,
    pub(crate) root: Datum,
}
impl Script {
    pub fn kind(&self) -> u16 {
        self.kind
    }
    pub fn return_type(&self) -> u16 {
        self.return_type
    }
    pub fn root(&self) -> Datum {
        self.root
    }
    pub const SIZE: usize = 40;
    pub fn decode(b: &[u8]) -> Result<Self> {
        exact(b, Self::SIZE)?;
        let v = Self {
            name_bytes: b[..32].try_into().unwrap(),
            kind: short(b, 32),
            return_type: short(b, 34),
            root: Datum(word(b, 36)),
        };
        v.name()?;
        Ok(v)
    }
    pub fn name(&self) -> Result<&str> {
        fixed_name(&self.name_bytes)
    }
    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0; Self::SIZE];
        b[..32].copy_from_slice(&self.name_bytes);
        b[32..34].copy_from_slice(&self.kind.to_le_bytes());
        b[34..36].copy_from_slice(&self.return_type.to_le_bytes());
        b[36..40].copy_from_slice(&self.root.0.to_le_bytes());
        b
    }
}

/// Scenario global definition. The +0x22 word is retained without interpretation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Global {
    pub(crate) name_bytes: [u8; 32],
    pub(crate) value_type: u16,
    pub(crate) reserved: u16,
    pub(crate) root: Datum,
}
impl Global {
    pub fn value_type(&self) -> u16 {
        self.value_type
    }
    pub fn reserved(&self) -> u16 {
        self.reserved
    }
    pub fn root(&self) -> Datum {
        self.root
    }
    pub const SIZE: usize = 40;
    pub fn decode(b: &[u8]) -> Result<Self> {
        let s = Script::decode(b)?;
        Ok(Self {
            name_bytes: s.name_bytes,
            value_type: s.kind,
            reserved: s.return_type,
            root: s.root,
        })
    }
    pub fn name(&self) -> Result<&str> {
        fixed_name(&self.name_bytes)
    }
    pub fn encode(&self) -> [u8; Self::SIZE] {
        Script {
            name_bytes: self.name_bytes,
            kind: self.value_type,
            return_type: self.reserved,
            root: self.root,
        }
        .encode()
    }
}
fn fixed_name(bytes: &[u8; 32]) -> Result<&str> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    std::str::from_utf8(&bytes[..end]).map_err(|_| invalid("non-UTF-8 definition name"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reference {
    pub(crate) group: map::FourCC,
    pub(crate) id: map::TagId,
}
impl Reference {
    pub fn group(&self) -> map::FourCC {
        self.group
    }
    pub fn id(&self) -> map::TagId {
        self.id
    }
    pub const SIZE: usize = 8;
    pub fn decode(b: &[u8]) -> Result<Self> {
        exact(b, Self::SIZE)?;
        Ok(Self {
            group: map::FourCC::from_u32(word(b, 0)),
            id: map::TagId(word(b, 4)),
        })
    }
    pub fn encode(self) -> [u8; Self::SIZE] {
        let mut b = [0; Self::SIZE];
        b[..4].copy_from_slice(&u32::from_be_bytes(self.group.0).to_le_bytes());
        b[4..].copy_from_slice(&self.id.0.to_le_bytes());
        b
    }
}
