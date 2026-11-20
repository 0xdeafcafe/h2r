//! The field mask is the schema. One declaration per layer drives
//! comparison, patching and both directions of the wire codec.

use super::{Object, Player, invalid};
use std::io;

pub(super) trait Field: Sized {
    fn same(&self, other: &Self) -> bool;
    fn write(&self, out: &mut Vec<u8>) -> io::Result<()>;
    fn read(input: &mut &[u8]) -> io::Result<Self>;
}

macro_rules! integers {
    ($($ty:ty),+) => {$(
        impl Field for $ty {
            fn same(&self, other: &Self) -> bool { self == other }
            fn write(&self, out: &mut Vec<u8>) -> io::Result<()> {
                out.extend_from_slice(&self.to_le_bytes());
                Ok(())
            }
            fn read(input: &mut &[u8]) -> io::Result<Self> {
                let (bytes, rest) = input.split_first_chunk()
                    .ok_or_else(|| invalid("truncated live field"))?;
                *input = rest;
                Ok(Self::from_le_bytes(*bytes))
            }
        }
    )+};
}

integers!(u8, u16, i16, u32, u64);

impl Field for f32 {
    fn same(&self, other: &Self) -> bool {
        self.to_bits() == other.to_bits()
    }
    fn write(&self, out: &mut Vec<u8>) -> io::Result<()> {
        self.to_bits().write(out)
    }
    fn read(input: &mut &[u8]) -> io::Result<Self> {
        Ok(Self::from_bits(u32::read(input)?))
    }
}

impl Field for [f32; 3] {
    fn same(&self, other: &Self) -> bool {
        self.iter().zip(other).all(|(a, b)| a.same(b))
    }
    fn write(&self, out: &mut Vec<u8>) -> io::Result<()> {
        for component in self {
            component.write(out)?;
        }
        Ok(())
    }
    fn read(input: &mut &[u8]) -> io::Result<Self> {
        Ok([f32::read(input)?, f32::read(input)?, f32::read(input)?])
    }
}

impl<T: Field + PartialEq> Field for Option<T> {
    fn same(&self, other: &Self) -> bool {
        self == other
    }
    fn write(&self, out: &mut Vec<u8>) -> io::Result<()> {
        out.push(u8::from(self.is_some()));
        if let Some(value) = self {
            value.write(out)?;
        }
        Ok(())
    }
    fn read(input: &mut &[u8]) -> io::Result<Self> {
        match u8::read(input)? {
            0 => Ok(None),
            1 => Ok(Some(T::read(input)?)),
            _ => Err(invalid("invalid optional live field")),
        }
    }
}

impl Field for String {
    fn same(&self, other: &Self) -> bool {
        self == other
    }
    fn write(&self, out: &mut Vec<u8>) -> io::Result<()> {
        let len = u16::try_from(self.len()).map_err(|_| invalid("player name too long"))?;
        len.write(out)?;
        out.extend_from_slice(self.as_bytes());
        Ok(())
    }
    fn read(input: &mut &[u8]) -> io::Result<Self> {
        let len = u16::read(input)? as usize;
        let (bytes, rest) = input
            .split_at_checked(len)
            .ok_or_else(|| invalid("truncated player name"))?;
        let name = std::str::from_utf8(bytes).map_err(|_| invalid("invalid UTF-8 player name"))?;
        *input = rest;
        Ok(name.to_owned())
    }
}

pub(super) trait Record: Default + Clone {
    type Id: Field + Ord + Copy + std::fmt::Debug;
    const ALL: u16;
    const KEY_ONLY: u16;
    fn id(&self) -> Self::Id;
    fn diff(&self, old: &Self) -> u16;
    fn patch(&mut self, mask: u16, from: &Self);
    fn write_fields(&self, mask: u16, out: &mut Vec<u8>) -> io::Result<()>;
    fn read_fields(id: Self::Id, mask: u16, input: &mut &[u8]) -> io::Result<Self>;
}

macro_rules! record {
    ($ty:ident, $id:ident: $id_ty:ty, key_only: $key_only:expr, {$($bit:literal: $field:ident),+ $(,)?}) => {
        impl Record for $ty {
            type Id = $id_ty;
            const ALL: u16 = 0 $(| (1 << $bit))+;
            const KEY_ONLY: u16 = $key_only;
            fn id(&self) -> Self::Id { self.$id }
            fn diff(&self, old: &Self) -> u16 {
                let mut mask = 0;
                $(if !self.$field.same(&old.$field) { mask |= 1 << $bit; })+
                mask
            }
            fn patch(&mut self, mask: u16, from: &Self) {
                $(if mask & (1 << $bit) != 0 { self.$field.clone_from(&from.$field); })+
            }
            fn write_fields(&self, mask: u16, out: &mut Vec<u8>) -> io::Result<()> {
                $(if mask & (1 << $bit) != 0 { self.$field.write(out)?; })+
                Ok(())
            }
            fn read_fields(id: Self::Id, mask: u16, input: &mut &[u8]) -> io::Result<Self> {
                let mut row = Self { $id: id, ..Self::default() };
                $(if mask & (1 << $bit) != 0 { row.$field = Field::read(input)?; })+
                Ok(row)
            }
        }
    };
}

record!(Object, id: u32, key_only: 0, {
    0: tag, 1: kind, 2: flags, 3: parent, 4: position, 5: forward,
    6: up, 7: scale, 8: velocity, 9: body, 10: shield, 11: placement,
});
record!(Player, index: u16, key_only: 1, {
    0: name, 1: team, 2: unit, 3: held, 4: aim, 5: local_user,
});
