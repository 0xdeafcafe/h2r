//! Fixed-size little-endian codecs. These are disk records, not native ABI structs.
use crate::{Error, Result};
use map::{FourCC, TagId};

/// A field category for tools, bounds validation, and editors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    U8,
    U16,
    I16,
    U32,
    F32,
    Enum16,
    Enum32,
    Flags8,
    Flags16,
    StringId,
    TagRef,
    DataRef,
    Block(u32),
    EmptyBlock,
    Bytes(usize),
    U32s(usize),
    F32s(usize),
}
/// Location and wire size of one reviewed field.
#[derive(Clone, Copy, Debug)]
pub struct FieldMeta {
    pub name: &'static str,
    pub offset: usize,
    pub size: usize,
    pub kind: FieldKind,
    /// Enum discriminants or flag masks; unknown values remain representable.
    pub choices: &'static [(u32, &'static str)],
    /// Generated record type for a decoded child block; None for opaque blocks.
    pub element: Option<&'static str>,
}
/// Metadata emitted from the same schema as the binary codec.
#[derive(Clone, Copy, Debug)]
pub struct RecordMeta {
    pub name: &'static str,
    pub size: usize,
    pub fields: &'static [FieldMeta],
}
/// Generated records accept exactly their fixed wire size. Callers extract a
/// prefix from a larger tag payload; the tag's total size includes child blocks.
pub trait BinaryRecord: Sized + std::fmt::Debug {
    const SIZE: usize;
    const META: RecordMeta;
    fn decode(bytes: &[u8]) -> Result<Self>;
    /// Encode a fixed record, retaining all unknown bytes. This does not relocate
    /// blocks, resize a cache, update checksums, or constitute a map writer.
    fn encode_into(&self, bytes: &mut [u8]) -> Result<()>;
}

pub(crate) fn exact(actual: usize, expected: usize) -> Result<()> {
    if actual != expected {
        return Err(Error::RecordSize { actual, expected });
    }
    Ok(())
}
pub(crate) trait Wire: Sized {
    const SIZE: usize;
    fn read(bytes: &[u8]) -> Result<Self>;
    fn write(&self, bytes: &mut [u8]) -> Result<()>;
}
macro_rules! scalar {
    ($($t:ty),*) => {$(impl Wire for $t {
        const SIZE:usize=std::mem::size_of::<$t>();
        fn read(b:&[u8])->Result<Self>{exact(b.len(),Self::SIZE)?;Ok(Self::from_le_bytes(b.try_into().expect("checked scalar size")))}
        fn write(&self,b:&mut[u8])->Result<()>{exact(b.len(),Self::SIZE)?;b.copy_from_slice(&self.to_le_bytes());Ok(())}
    })*};
}
scalar!(u8, u16, i16, u32, f32);
impl<T: Wire, const N: usize> Wire for [T; N] {
    const SIZE: usize = T::SIZE * N;
    fn read(b: &[u8]) -> Result<Self> {
        exact(b.len(), Self::SIZE)?;
        let values: Vec<T> = (0..N)
            .map(|i| T::read(&b[i * T::SIZE..(i + 1) * T::SIZE]))
            .collect::<Result<_>>()?;
        values.try_into().map_err(|_| Error::Invalid("array size"))
    }
    fn write(&self, b: &mut [u8]) -> Result<()> {
        exact(b.len(), Self::SIZE)?;
        for (i, v) in self.iter().enumerate() {
            v.write(&mut b[i * T::SIZE..(i + 1) * T::SIZE])?;
        }
        Ok(())
    }
}
macro_rules! wrapper {
    ($n:ident,$t:ty,$doc:literal) => {
        #[doc=$doc]
        #[derive(Clone, Copy, PartialEq, Eq, Default)]
        pub struct $n(pub $t);
        impl std::fmt::Debug for $n {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}({:#x})", stringify!($n), self.0)
            }
        }
        impl Wire for $n {
            const SIZE: usize = <$t as Wire>::SIZE;
            fn read(b: &[u8]) -> Result<Self> {
                Ok(Self(<$t>::read(b)?))
            }
            fn write(&self, b: &mut [u8]) -> Result<()> {
                self.0.write(b)
            }
        }
    };
}
wrapper!(
    Enum16,
    u16,
    "A 16-bit enum, preserving unrecognized discriminants."
);
wrapper!(
    Enum32,
    u32,
    "A 32-bit enum, preserving unrecognized discriminants."
);
wrapper!(Flags16, u16, "A 16-bit flag set, preserving unknown bits.");
wrapper!(
    StringId,
    u32,
    "A string-ID value: lower 24 bits index, upper 8 bits byte length."
);
/// Decoded Xbox StringID: a 24-bit name index and an 8-bit name length.
/// Bits 16..23 belong to the index; this format has no set field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StringIdParts {
    pub index: u32,
    pub length: u8,
}
impl StringId {
    pub const NULL: Self = Self(u32::MAX);
    pub const EMPTY: Self = Self(0);

    /// Split the packed value, including sentinels, without validating a name.
    /// The length describes the identifier name, not its localized text.
    pub const fn decode(self) -> StringIdParts {
        StringIdParts {
            index: self.0 & 0x00ff_ffff,
            length: (self.0 >> 24) as u8,
        }
    }
    pub const fn index(self) -> usize {
        self.decode().index as usize
    }
    pub const fn length(self) -> usize {
        self.decode().length as usize
    }
    /// Check both index and encoded length. Null is `0xffffffff`; zero may
    /// resolve the empty string and is not treated as a tag datum.
    pub fn resolve(self, strings: &[Option<String>]) -> Result<Option<&str>> {
        if self == Self::NULL {
            return Ok(None);
        }
        let s = strings
            .get(self.index())
            .and_then(Option::as_deref)
            .ok_or(Error::Invalid("string ID index"))?;
        if s.len() != self.length() {
            return Err(Error::Invalid("string ID length"));
        }
        Ok(Some(s))
    }
}
/// An Xbox cache tag_block: count and virtual pointer, exactly eight bytes.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct TagBlock {
    pub count: u32,
    pub address: u32,
}
/// A byte count and metadata pointer, not a shared-resource pointer.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct DataReference {
    pub size: u32,
    pub address: u32,
}
/// An eight-byte cache tag reference. Editing-kit references have another size.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TagReference {
    pub group: FourCC,
    pub id: TagId,
}
impl Wire for TagBlock {
    const SIZE: usize = 8;
    fn read(b: &[u8]) -> Result<Self> {
        exact(b.len(), 8)?;
        Ok(Self {
            count: u32::read(&b[..4])?,
            address: u32::read(&b[4..])?,
        })
    }
    fn write(&self, b: &mut [u8]) -> Result<()> {
        exact(b.len(), 8)?;
        self.count.write(&mut b[..4])?;
        self.address.write(&mut b[4..])
    }
}
impl Wire for DataReference {
    const SIZE: usize = 8;
    fn read(b: &[u8]) -> Result<Self> {
        let v = TagBlock::read(b)?;
        Ok(Self {
            size: v.count,
            address: v.address,
        })
    }
    fn write(&self, b: &mut [u8]) -> Result<()> {
        TagBlock {
            count: self.size,
            address: self.address,
        }
        .write(b)
    }
}
impl Wire for TagReference {
    const SIZE: usize = 8;
    fn read(b: &[u8]) -> Result<Self> {
        let v = TagBlock::read(b)?;
        Ok(Self {
            group: FourCC::from_u32(v.count),
            id: TagId(v.address),
        })
    }
    fn write(&self, b: &mut [u8]) -> Result<()> {
        TagBlock {
            count: self.group.as_u32(),
            address: self.id.0,
        }
        .write(b)
    }
}
impl std::fmt::Debug for TagBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "TagBlock {{ count: {}, address: {:#010x} }}",
            self.count, self.address
        )
    }
}
impl std::fmt::Debug for DataReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "DataReference {{ size: {}, address: {:#010x} }}",
            self.size, self.address
        )
    }
}
impl std::fmt::Debug for TagReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.id == TagId::NULL {
            return f.write_str("none");
        }
        write!(
            f,
            "TagReference {{ group: {}, id: {} }}",
            self.group, self.id
        )
    }
}

/// Suppress alternate Debug expansion for scalar arrays and small wrappers.
pub(crate) struct Compact<'a, T: ?Sized>(pub &'a T);
impl<T: std::fmt::Debug + ?Sized> std::fmt::Debug for Compact<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

/// Generated records use the same field metadata for codecs and readable dumps.
/// Formatting re-encodes the edited fields, so it never displays stale originals.
pub(crate) fn debug_record<T: BinaryRecord>(
    record: &T,
    f: &mut std::fmt::Formatter<'_>,
) -> std::fmt::Result {
    let mut bytes = vec![0; T::SIZE];
    record
        .encode_into(&mut bytes)
        .map_err(|_| std::fmt::Error)?;
    let mut d = f.debug_struct(T::META.name);
    let mut empty = 0usize;
    for field in T::META.fields {
        let b = &bytes[field.offset..field.offset + field.size];
        if matches!(field.kind, FieldKind::Block(_) | FieldKind::EmptyBlock)
            && u32::read(&b[..4]).map_err(|_| std::fmt::Error)? == 0
        {
            empty += 1;
            continue;
        }
        d.field(field.name, &FieldValue(field, b));
    }
    if empty != 0 {
        d.field("empty_blocks", &empty);
    }
    d.finish()
}
struct FieldValue<'a>(&'a FieldMeta, &'a [u8]);
impl std::fmt::Debug for FieldValue<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use FieldKind::*;
        let Self(meta, b) = self;
        macro_rules! read {
            ($t:ty) => {
                <$t as Wire>::read(b).map_err(|_| std::fmt::Error)?
            };
        }
        match meta.kind {
            Bytes(_) if meta.name.starts_with("opaque_") => write!(f, "{} opaque bytes", b.len()),
            Bytes(_) => write!(f, "{b:?}"),
            U8 => write!(f, "{}", read!(u8)),
            U16 => write!(f, "{}", read!(u16)),
            I16 => write!(f, "{}", read!(i16)),
            U32 => write!(f, "{:#x}", read!(u32)),
            F32 => write!(f, "{:?}", read!(f32)),
            Enum16 | Enum32 | Flags8 | Flags16 => {
                let value = match b.len() {
                    1 => u32::from(read!(u8)),
                    2 => u32::from(read!(u16)),
                    _ => read!(u32),
                };
                if matches!(meta.kind, Enum16 | Enum32) {
                    if let Some((_, name)) = meta.choices.iter().find(|(v, _)| *v == value) {
                        return write!(f, "{name} ({value})");
                    }
                } else {
                    for (mask, name) in meta.choices {
                        if *mask != 0 && value & mask == *mask {
                            write!(f, "{name} | ")?;
                        }
                    }
                }
                write!(f, "{value:#x}")
            }
            StringId => write!(f, "{:?}", read!(self::StringId)),
            TagRef => write!(f, "{:?}", read!(TagReference)),
            DataRef => write!(f, "{:?}", read!(DataReference)),
            Block(_) | EmptyBlock => write!(f, "{:?}", read!(TagBlock)),
            U32s(_) | F32s(_) => {
                f.write_str("[")?;
                for (i, chunk) in b.as_chunks::<4>().0.iter().enumerate() {
                    if i != 0 {
                        f.write_str(", ")?;
                    }
                    if matches!(meta.kind, U32s(_)) {
                        write!(f, "{:#x}", u32::from_le_bytes(*chunk))?;
                    } else {
                        write!(f, "{:?}", f32::from_le_bytes(*chunk))?;
                    }
                }
                f.write_str("]")
            }
        }
    }
}
