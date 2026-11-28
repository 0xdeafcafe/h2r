//! Field values, typed offsets and the [`View`] trait every layout
//! implements.

use std::fmt;
use std::marker::PhantomData;

use crate::mem::{Fault, Mem, MemMut};

/// The largest field a view reads in one access (a player's name is 64).
pub const MAX_FIELD: usize = 64;

/// A value a field holds, little-endian as the guest stores it.
pub trait Field: Copy {
    const SIZE: usize;
    /// `b` is exactly [`SIZE`](Field::SIZE) bytes.
    fn decode(b: &[u8]) -> Self;
    /// `out` is exactly [`SIZE`](Field::SIZE) bytes.
    fn encode(self, out: &mut [u8]);
}

macro_rules! le_field {
    ($($t:ty),*) => {$(
        impl Field for $t {
            const SIZE: usize = std::mem::size_of::<$t>();
            #[inline]
            fn decode(b: &[u8]) -> Self {
                let mut a = [0u8; std::mem::size_of::<$t>()];
                a.copy_from_slice(b);
                <$t>::from_le_bytes(a)
            }
            #[inline]
            fn encode(self, out: &mut [u8]) {
                out.copy_from_slice(&self.to_le_bytes());
            }
        }
    )*};
}
le_field!(u8, i8, u16, i16, u32, i32, f32);

/// Arrays of fields: vectors (`[f32; 3]`), weapon slots, names.
impl<T: Field, const N: usize> Field for [T; N] {
    const SIZE: usize = T::SIZE * N;
    #[inline]
    fn decode(b: &[u8]) -> Self {
        std::array::from_fn(|i| T::decode(&b[i * T::SIZE..(i + 1) * T::SIZE]))
    }
    #[inline]
    fn encode(self, out: &mut [u8]) {
        for (i, v) in self.into_iter().enumerate() {
            v.encode(&mut out[i * T::SIZE..(i + 1) * T::SIZE]);
        }
    }
}

/// A salted datum handle as the guest stores it: salt in the high half,
/// index in the low; `0xffffffff` is none. `guest_layout::handle::DatumHandle::from_raw`
/// takes [`Datum::raw`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Datum(pub u32);

impl Datum {
    pub const NONE: Datum = Datum(u32::MAX);

    pub const fn raw(self) -> u32 {
        self.0
    }
    pub const fn is_none(self) -> bool {
        self.0 == u32::MAX
    }
    /// The raw handle, `None` for [`Datum::NONE`].
    pub const fn get(self) -> Option<u32> {
        if self.is_none() { None } else { Some(self.0) }
    }
    pub const fn index(self) -> u16 {
        self.0 as u16
    }
    pub const fn salt(self) -> u16 {
        (self.0 >> 16) as u16
    }
}

impl fmt::Debug for Datum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_none() {
            write!(f, "Datum(none)")
        } else {
            write!(f, "Datum({:08x})", self.0)
        }
    }
}

impl Field for Datum {
    const SIZE: usize = 4;
    #[inline]
    fn decode(b: &[u8]) -> Self {
        Datum(u32::decode(b))
    }
    #[inline]
    fn encode(self, out: &mut [u8]) {
        self.0.encode(out)
    }
}

/// A field's offset inside its structure, typed by what it holds. Views
/// declare one per field (`Unit::PLAYER`); `.offset` is the number for
/// code that still adds by hand.
pub struct Off<T> {
    pub offset: u32,
    _t: PhantomData<fn() -> T>,
}

impl<T> Off<T> {
    pub const fn new(offset: u32) -> Self {
        Off {
            offset,
            _t: PhantomData,
        }
    }
    /// The field's address in the structure at `base`.
    #[inline]
    pub const fn at(self, base: u32) -> u32 {
        base.wrapping_add(self.offset)
    }
}

impl<T> Clone for Off<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Off<T> {}
impl<T> PartialEq for Off<T> {
    fn eq(&self, other: &Self) -> bool {
        self.offset == other.offset
    }
}
impl<T> Eq for Off<T> {}
impl<T> fmt::Debug for Off<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "+{:#x}", self.offset)
    }
}

impl<T: Field> Off<T> {
    /// The field of the structure at `base`.
    #[inline]
    pub fn read<M: Mem + ?Sized>(self, m: &M, base: u32) -> Result<T, Fault> {
        const { assert!(T::SIZE <= MAX_FIELD, "field larger than MAX_FIELD") };
        let mut buf = [0u8; MAX_FIELD];
        let b = &mut buf[..T::SIZE];
        m.read_bytes(self.at(base), b)?;
        Ok(T::decode(b))
    }
    /// Stores `v` in the field of the structure at `base`.
    #[inline]
    pub fn write<M: MemMut + ?Sized>(self, m: &mut M, base: u32, v: T) -> Result<(), Fault> {
        const { assert!(T::SIZE <= MAX_FIELD, "field larger than MAX_FIELD") };
        let mut buf = [0u8; MAX_FIELD];
        let b = &mut buf[..T::SIZE];
        v.encode(b);
        m.write_bytes(self.at(base), b)
    }
}

/// One declared field, for tests and listings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldInfo {
    pub name: &'static str,
    pub offset: u32,
    pub size: u32,
}

/// A guest structure at an address. Implemented by [`layout!`](crate::layout).
pub trait View: Copy {
    /// The structure's name, as declared.
    const NAME: &'static str;
    /// The bytes the view covers. The guest structure may be longer where
    /// the rest is not read yet; where its size is known, this is it.
    const SIZE: u32;
    /// Every declared field.
    const FIELDS: &'static [FieldInfo];
    fn at(addr: u32) -> Self;
    fn addr(&self) -> u32;
}
