//! Typed views over guest memory: an address plus accessors, with the
//! layout written once, beside its evidence (docs/feature-engine.md, "Guest
//! data access").
//!
//! A view never caches and never owns: every accessor reads or writes
//! through [`GuestMemory`] at the moment it is called, so it works on the
//! runtime's guest, on [`FakeGuest`] and on the differential harness's
//! memory alike. Offsets are named research points; no game data is here.
//!
//! [`record!`] declares a fixed-size guest structure:
//!
//! ```
//! engine::record! {
//!     /// A thing (docs/x.md).
//!     pub struct Thing: 0x10 {
//!         /// Its count.
//!         count @ 0x0: u16,
//!         position @ 0x4: engine::view::Vec3,
//!     }
//! }
//! # use engine::{GuestMemory, testing::FakeGuest};
//! let mut g = FakeGuest::new();
//! Thing::build().count(3).write(&mut g, 0x1000);
//! assert_eq!(Thing::at(0x1000).count(&g), Ok(3));
//! assert_eq!(Thing::COUNT, 0x0);
//! ```
//!
//! It generates `Thing::at(addr)`, `SIZE`, an offset constant per field
//! (`Thing::COUNT`), a getter and a setter per field (`count(m)`,
//! `set_count(m, v)`) and a builder for [`FakeGuest`] (`Thing::build()`).
//!
//! On top of records: [`Datums`] (a datum array of records, through the
//! ported datum functions), [`Tags`] (the published tag index, through the
//! ported checked lookup), [`players`] and [`objects`].

use crate::datum::DatumArray;
use crate::handle::DatumHandle;
use crate::memory::{GuestMemory, MemFault};
use crate::tags::{self, GroupTag};
use crate::testing::FakeGuest;

pub type R<T> = Result<T, MemFault>;

/// Three `f32`s, as the guest stores points and vectors.
pub type Vec3 = [f32; 3];

/// A value a record field holds: how it is read and written.
pub trait Field: Sized + Copy + Default {
    const SIZE: u32;
    fn read(m: &dyn GuestMemory, addr: u32) -> R<Self>;
    fn write(self, m: &mut dyn GuestMemory, addr: u32) -> R<()>;
}

macro_rules! int_field {
    ($($t:ty),*) => {$(
        impl Field for $t {
            const SIZE: u32 = std::mem::size_of::<$t>() as u32;
            fn read(m: &dyn GuestMemory, addr: u32) -> R<Self> {
                let mut b = [0; std::mem::size_of::<$t>()];
                m.read(addr, &mut b)?;
                Ok(<$t>::from_le_bytes(b))
            }
            fn write(self, m: &mut dyn GuestMemory, addr: u32) -> R<()> {
                m.write(addr, &self.to_le_bytes())
            }
        }
    )*};
}
int_field!(u8, i8, u16, i16, u32, i32, f32);

impl Field for Vec3 {
    const SIZE: u32 = 12;
    fn read(m: &dyn GuestMemory, addr: u32) -> R<Self> {
        m.vec3(addr)
    }
    fn write(self, m: &mut dyn GuestMemory, addr: u32) -> R<()> {
        for (i, v) in self.iter().enumerate() {
            m.set_f32(addr.wrapping_add(4 * i as u32), *v)?;
        }
        Ok(())
    }
}

/// A salted datum handle field; `None` is the guest's `0xffffffff`.
impl Field for Option<DatumHandle> {
    const SIZE: u32 = 4;
    fn read(m: &dyn GuestMemory, addr: u32) -> R<Self> {
        m.u32(addr).map(DatumHandle::from_raw)
    }
    fn write(self, m: &mut dyn GuestMemory, addr: u32) -> R<()> {
        m.set_u32(addr, crate::handle::raw_or_none(self))
    }
}

/// A fixed-size guest structure at an address. Implemented by [`record!`].
pub trait Record: Copy {
    const SIZE: u32;
    fn at(addr: u32) -> Self;
    fn addr(&self) -> u32;
}

/// Declares a [`Record`]. See the module docs.
#[macro_export]
macro_rules! record {
    (
        $(#[$m:meta])*
        $vis:vis struct $name:ident : $size:literal {
            $( $(#[$fm:meta])* $field:ident @ $off:literal : $t:ty ),* $(,)?
        }
    ) => {
        $crate::view::paste::paste! {
            $(#[$m])*
            #[derive(Clone, Copy, Debug, PartialEq, Eq)]
            $vis struct $name {
                pub addr: u32,
            }

            #[allow(dead_code)]
            impl $name {
                pub const SIZE: u32 = $size;
                $(
                    #[doc = concat!("Offset of `", stringify!($field), "`.")]
                    pub const [<$field:upper>]: u32 = $off;
                )*
                pub const fn at(addr: u32) -> Self {
                    $name { addr }
                }
                $(
                    $(#[$fm])*
                    pub fn $field(&self, m: &dyn $crate::GuestMemory) -> $crate::view::R<$t> {
                        <$t as $crate::view::Field>::read(m, self.addr.wrapping_add($off))
                    }
                    pub fn [<set_ $field>](
                        &self,
                        m: &mut dyn $crate::GuestMemory,
                        v: $t,
                    ) -> $crate::view::R<()> {
                        $crate::view::Field::write(v, m, self.addr.wrapping_add($off))
                    }
                )*
                /// A builder that writes this record into a [`FakeGuest`](
                /// $crate::testing::FakeGuest), for tests.
                pub fn build() -> [<$name Builder>] {
                    [<$name Builder>]::default()
                }
            }

            impl $crate::view::Record for $name {
                const SIZE: u32 = $size;
                fn at(addr: u32) -> Self {
                    $name { addr }
                }
                fn addr(&self) -> u32 {
                    self.addr
                }
            }

            #[doc = concat!("Builds a [`", stringify!($name), "`] in a fake guest.")]
            #[derive(Clone, Copy, Debug, Default)]
            #[allow(dead_code)]
            $vis struct [<$name Builder>] {
                $( $field: Option<$t>, )*
            }

            #[allow(dead_code)]
            impl [<$name Builder>] {
                $(
                    pub fn $field(mut self, v: $t) -> Self {
                        self.$field = Some(v);
                        self
                    }
                )*
                /// Maps the record's pages, writes the fields that were set
                /// (the rest stay as they were) and returns the view.
                pub fn write(self, g: &mut $crate::testing::FakeGuest, addr: u32) -> $name {
                    g.map(addr, $size);
                    $(
                        if let Some(v) = self.$field {
                            $crate::view::Field::write(v, g, addr.wrapping_add($off))
                                .expect("mapped");
                        }
                    )*
                    $name { addr }
                }
            }
        }
    };
}

#[doc(hidden)]
pub use paste;

/// A datum array whose elements are `T`: lookups go through the ported
/// datum functions, so a handle is accepted exactly when the game's
/// `datum_try_and_get` would accept it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Datums<T> {
    pub array: DatumArray,
    _t: std::marker::PhantomData<T>,
}

impl<T: Record> Datums<T> {
    pub const fn at(header: u32) -> Self {
        Datums {
            array: DatumArray::at(header),
            _t: std::marker::PhantomData,
        }
    }

    /// The array a global points at (`*global`), or `None` while it is
    /// null.
    pub fn from_global(m: &dyn GuestMemory, global: u32) -> R<Option<Self>> {
        let header = m.u32(global)?;
        Ok((header != 0).then(|| Self::at(header)))
    }

    /// The live element `handle` names (`datum_try_and_get`).
    pub fn get(&self, m: &dyn GuestMemory, handle: DatumHandle) -> R<Option<T>> {
        Ok(self.array.try_get(m, handle)?.map(T::at))
    }

    /// The live element in slot `index`, salt not compared
    /// (`datum_get_absolute`).
    pub fn get_absolute(&self, m: &dyn GuestMemory, index: i32) -> R<Option<T>> {
        Ok(self.array.get_absolute(m, index)?.map(T::at))
    }

    /// Slots below the high-water mark whose salt is not zero, with their
    /// handles, in slot order. At most `limit` slots are looked at.
    pub fn live(&self, m: &dyn GuestMemory, limit: i32) -> R<Vec<(DatumHandle, T)>> {
        let high_water = self
            .array
            .header_field(m, crate::datum::layout::HIGH_WATER)?
            .min(limit);
        let mut out = Vec::new();
        for i in 0..high_water.max(0) {
            let elem = self.array.element_addr(m, i)?;
            let salt = m.u16(elem)?;
            if salt != 0 {
                out.push((DatumHandle::new(salt, i as u32), T::at(elem)));
            }
        }
        Ok(out)
    }
}

/// The published tag index (`*TAG_INDEX`, `*TAG_ENTRIES`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tags;

/// One tag-entry record (`docs/engine/tag_get_checked.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tag {
    /// The entry's address in the entry table.
    pub entry: u32,
}

impl Tag {
    pub fn group(&self, m: &dyn GuestMemory) -> R<GroupTag> {
        m.u32(self.entry.wrapping_add(tags::entry_layout::GROUP))
            .map(GroupTag)
    }
    pub fn datum(&self, m: &dyn GuestMemory) -> R<Option<DatumHandle>> {
        m.u32(self.entry.wrapping_add(tags::entry_layout::DATUM))
            .map(DatumHandle::from_raw)
    }
    /// The tag's metadata address.
    pub fn address(&self, m: &dyn GuestMemory) -> R<u32> {
        m.u32(self.entry.wrapping_add(tags::entry_layout::ADDRESS))
    }
    pub fn size(&self, m: &dyn GuestMemory) -> R<u32> {
        m.u32(self.entry.wrapping_add(tags::entry_layout::SIZE))
    }
}

impl Tags {
    /// The entry for `tag`, if its slot is in range and its datum matches
    /// (the bounds-and-salt half of `tag_get_checked`).
    pub fn entry(&self, m: &dyn GuestMemory, tag: DatumHandle) -> R<Option<Tag>> {
        Ok(tags::tag_entry_checked(m, tag)?.map(|entry| Tag { entry }))
    }
    /// The metadata of `tag` if it is of group `group` or a child of it
    /// (`tag_get_checked`, quirks included).
    pub fn get(&self, m: &dyn GuestMemory, tag: DatumHandle, group: GroupTag) -> R<Option<u32>> {
        tags::tag_get_checked(m, tag, group)
    }
    /// The loaded scenario's tag datum: the index header's `+0xc`.
    pub fn scenario(&self, m: &dyn GuestMemory) -> R<Option<DatumHandle>> {
        let index = m.u32(tags::TAG_INDEX_GLOBAL)?;
        m.u32(index.wrapping_add(tags::index_layout::SCENARIO))
            .map(DatumHandle::from_raw)
    }
}

/// Players (docs/live-gamestate.md).
pub mod players {
    use super::*;

    crate::record! {
        /// One player datum (`0x21c` bytes). Field meanings as Live
        /// telemetry reads them.
        pub struct Player: 0x21c {
            /// Datum salt; zero when the slot is free.
            salt @ 0x00: u16,
            /// Bit 1: the player is leaving and is not listed.
            flags @ 0x02: u16,
            /// Local controller index, `-1` for a remote player.
            local_user @ 0x28: i16,
            /// The unit object the player controls.
            unit @ 0x2c: Option<DatumHandle>,
            /// Team index (signed byte).
            team @ 0xc0: i8,
        }
    }

    /// The name's offset and capacity: UTF-16, 32 code units with the NUL.
    pub const NAME: u32 = 0x44;
    pub const NAME_UNITS: usize = 32;

    impl Player {
        /// The name, up to its NUL (a full buffer is taken whole).
        pub fn name(&self, m: &dyn GuestMemory) -> R<String> {
            let mut units = Vec::with_capacity(NAME_UNITS);
            for i in 0..NAME_UNITS as u32 {
                let u = m.u16(self.addr.wrapping_add(NAME + 2 * i))?;
                if u == 0 {
                    break;
                }
                units.push(u);
            }
            Ok(String::from_utf16_lossy(&units))
        }
        /// Listed in a roster: live and not leaving.
        pub fn listed(&self, m: &dyn GuestMemory) -> R<bool> {
            Ok(self.salt(m)? != 0 && self.flags(m)? & 2 == 0)
        }
    }

    /// The players array (`*PLAYERS`).
    pub fn array(m: &dyn GuestMemory) -> R<Option<Datums<Player>>> {
        Datums::from_global(m, crate::points::PLAYERS.base)
    }
}

/// Objects (docs/objects.md, docs/live-gamestate.md).
pub mod objects {
    use super::*;

    crate::record! {
        /// An object header (`0xc` bytes), the element of the objects array.
        pub struct Header: 0xc {
            salt @ 0x0: u16,
            /// The concrete type: 0 biped, 1 vehicle, 2 weapon, ...
            kind @ 0x3: u8,
            /// The object's body (its full state).
            body @ 0x8: u32,
        }
    }

    crate::record! {
        /// The start of an object's body, the part every type shares.
        pub struct Body: 0x10c {
            /// The definition tag.
            definition @ 0x0: Option<DatumHandle>,
            /// The object this one is attached to.
            parent @ 0x14: Option<DatumHandle>,
            position @ 0x64: Vec3,
            forward @ 0x70: Vec3,
            up @ 0x7c: Vec3,
            /// Bit 2: body depleted.
            damage_flags @ 0x10a: u16,
        }
    }

    pub const BIPED: u8 = 0;
    pub const VEHICLE: u8 = 1;
    pub const WEAPON: u8 = 2;

    /// The objects array (`*OBJECTS`).
    pub fn array(m: &dyn GuestMemory) -> R<Option<Datums<Header>>> {
        Datums::from_global(m, crate::points::OBJECTS.base)
    }

    /// The type and body of the live object `handle` names, if it has a
    /// body.
    pub fn get(m: &dyn GuestMemory, handle: DatumHandle) -> R<Option<(u8, Body)>> {
        let Some(objects) = array(m)? else {
            return Ok(None);
        };
        let Some(header) = objects.get(m, handle)? else {
            return Ok(None);
        };
        let body = header.body(m)?;
        if body == 0 {
            return Ok(None);
        }
        Ok(Some((header.kind(m)?, Body::at(body))))
    }
}

/// Test builders for whole structures (datum arrays, the tag index) on a
/// [`FakeGuest`], laid out by the ported functions themselves.
pub mod fake {
    use super::*;

    /// A datum array of `max` elements of `T` at `header`, laid out and
    /// initialised by the ported `data_initialize_contiguous` and
    /// `data_make_valid` (data right after the header, then the bitmap).
    /// Its name string goes at `header - 0x20`. Returns the view.
    pub fn datums<T: Record>(g: &mut FakeGuest, header: u32, max: i32) -> Datums<T> {
        let name = header - 0x20;
        g.put(name, b"fake\0");
        let len = crate::datum::layout::HEADER_SIZE
            + 4
            + max as u32 * T::SIZE
            + crate::datum::bitmap_bytes(max);
        g.map(header, len);
        let a = DatumArray::at(header);
        a.initialize_contiguous(g, name, max, T::SIZE as i32, 2, 0)
            .expect("mapped");
        a.make_valid(g).expect("mapped");
        Datums::at(header)
    }

    /// Allocates the next element of `d` (`datum_new`).
    pub fn push<T: Record>(g: &mut FakeGuest, d: &Datums<T>) -> (DatumHandle, T) {
        let h = d.array.new_datum(g).expect("mapped").expect("room");
        (h, d.get(g, h).expect("mapped").expect("just made"))
    }
}
