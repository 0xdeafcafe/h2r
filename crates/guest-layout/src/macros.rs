//! [`layout!`]: the declarative definition of a guest structure.

/// Declares a guest structure: a view type holding an address, a typed
/// offset constant per field, a getter per field, and the field list.
///
/// ```
/// guest_layout::layout! {
///     /// A thing (docs/x.md).
///     pub struct Thing: 0x10 {
///         /// Its count.
///         COUNT count @ 0x0: u16,
///         POSITION position @ 0x4: [f32; 3],
///     }
/// }
/// use guest_layout::{FlatRam, View};
/// let mut m = FlatRam::new(0x1000, 0x10);
/// let t = Thing::at(0x1000);
/// t.set(&mut m, Thing::COUNT, 3).unwrap();
/// assert_eq!(t.count(&m), Ok(3));
/// assert_eq!(Thing::COUNT.offset, 0x0);
/// assert_eq!(Thing::FIELDS.len(), 2);
/// ```
///
/// Each field line is `CONST name @ offset: Type`. The constant is an
/// [`Off<Type>`](crate::Off) (`Thing::COUNT.offset` is the number); the
/// getter takes any [`Mem`](crate::Mem). Writes go through `set` with the
/// constant, so a field is named once whichever way it is used.
#[macro_export]
macro_rules! layout {
    (
        $(#[$m:meta])*
        $vis:vis struct $name:ident : $size:literal {
            $( $(#[$fm:meta])* $konst:ident $field:ident @ $off:literal : $t:ty ),* $(,)?
        }
    ) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        $vis struct $name {
            pub addr: u32,
        }

        #[allow(dead_code)]
        impl $name {
            /// The bytes this view covers (as `View::SIZE`).
            pub const SIZE: u32 = $size;
            $(
                $(#[$fm])*
                pub const $konst: $crate::Off<$t> = $crate::Off::new($off);
            )*
            /// Every declared field, in declaration order.
            pub const FIELDS: &'static [$crate::FieldInfo] = &[$(
                $crate::FieldInfo {
                    name: stringify!($field),
                    offset: $off,
                    size: <$t as $crate::Field>::SIZE as u32,
                }
            ),*];

            #[inline]
            pub const fn at(addr: u32) -> Self {
                $name { addr }
            }

            $(
                $(#[$fm])*
                #[inline]
                pub fn $field<M: $crate::Mem + ?Sized>(
                    &self,
                    m: &M,
                ) -> ::core::result::Result<$t, $crate::Fault> {
                    Self::$konst.read(m, self.addr)
                }
            )*

            /// Reads `field` of this structure.
            #[inline]
            pub fn get<T: $crate::Field, M: $crate::Mem + ?Sized>(
                &self,
                m: &M,
                field: $crate::Off<T>,
            ) -> ::core::result::Result<T, $crate::Fault> {
                field.read(m, self.addr)
            }

            /// Writes `field` of this structure.
            #[inline]
            pub fn set<T: $crate::Field, M: $crate::MemMut + ?Sized>(
                &self,
                m: &mut M,
                field: $crate::Off<T>,
                v: T,
            ) -> ::core::result::Result<(), $crate::Fault> {
                field.write(m, self.addr, v)
            }

            /// The address of `field` in this structure.
            #[inline]
            pub const fn addr_of<T>(&self, field: $crate::Off<T>) -> u32 {
                self.addr.wrapping_add(field.offset)
            }
        }

        impl $crate::View for $name {
            const NAME: &'static str = stringify!($name);
            const SIZE: u32 = $size;
            const FIELDS: &'static [$crate::FieldInfo] = $name::FIELDS;
            #[inline]
            fn at(addr: u32) -> Self {
                $name { addr }
            }
            #[inline]
            fn addr(&self) -> u32 {
                self.addr
            }
        }
    };
}

/// Declares named [`Global`](crate::global::Global)s: `NAME @ 0x...; "doc";`,
/// and a slice of them named by the first token.
#[macro_export]
macro_rules! globals {
    ($all:ident; $( $(#[doc = $d:literal])* $name:ident @ $addr:literal ; $doc:literal ; )*) => {
        $(
            $(#[doc = $d])*
            pub static $name: $crate::global::Global = $crate::global::Global {
                name: stringify!($name),
                base: $addr,
                doc: $doc,
            };
        )*
        /// Every global declared in this table.
        pub static $all: &[&$crate::global::Global] = &[$(&$name),*];
    };
}
