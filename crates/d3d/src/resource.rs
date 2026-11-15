//! Guest resource headers (`X_D3DResource` and friends).
//!
//! Every Xbox D3D resource starts with three dwords, `Common`, `Data` and
//! `Lock`. Pixel containers (textures, surfaces) add `Format` and `Size`;
//! surfaces add `Parent`. These headers live in guest memory, are created
//! and edited by guest code as well as by D3D (Halo builds many of its
//! texture headers itself and only hands D3D the address), and are the
//! identity of a resource: the device keys everything by header address.
//!
//! Layouts follow Cxbx-Reloaded's `XbD3D8Types.h` description at
//! `585c49a50af1255ab155099e06f24505f9c5a800` and agree with what the 5849
//! code here reads: `SetTexture` pushes `[tex+4]` (Data) and `[tex+0xC]`
//! (Format), `[tex+0x10]` (Size); `SetRenderTarget` tests `Common` against
//! `0x00070000 == 0x00050000` (surface) and reads `[surf+0x14]` (Parent).

use crate::guest::{GuestError, GuestMemory};

/// `Common` bits.
pub mod common {
    /// External reference count.
    pub const REFCOUNT_MASK: u32 = 0x0000_FFFF;
    /// Resource type field.
    pub const TYPE_MASK: u32 = 0x0007_0000;
    /// Shift of the type field.
    pub const TYPE_SHIFT: u32 = 16;
    /// Internal (GPU-side) reference count; D3D adds `0x80000` per binding.
    pub const INTREFCOUNT_MASK: u32 = 0x0078_0000;
    /// One internal reference.
    pub const INTREFCOUNT_ONE: u32 = 0x0008_0000;
    /// Created by D3D (as opposed to a header the title registered).
    pub const D3DCREATED: u32 = 0x0100_0000;
}

/// The `Common` type field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResourceType {
    /// `D3DCOMMON_TYPE_VERTEXBUFFER` (0).
    VertexBuffer,
    /// `D3DCOMMON_TYPE_INDEXBUFFER` (1).
    IndexBuffer,
    /// `D3DCOMMON_TYPE_PUSHBUFFER` (2).
    PushBuffer,
    /// `D3DCOMMON_TYPE_PALETTE` (3).
    Palette,
    /// `D3DCOMMON_TYPE_TEXTURE` (4): textures, cube and volume textures.
    Texture,
    /// `D3DCOMMON_TYPE_SURFACE` (5).
    Surface,
    /// `D3DCOMMON_TYPE_FIXUP` (6).
    Fixup,
    /// 7: not defined.
    Unknown,
}

impl ResourceType {
    /// Decodes the type field of a `Common` word.
    pub fn from_common(common: u32) -> Self {
        match (common & common::TYPE_MASK) >> common::TYPE_SHIFT {
            0 => Self::VertexBuffer,
            1 => Self::IndexBuffer,
            2 => Self::PushBuffer,
            3 => Self::Palette,
            4 => Self::Texture,
            5 => Self::Surface,
            6 => Self::Fixup,
            _ => Self::Unknown,
        }
    }

    /// The raw type value.
    pub fn raw(self) -> u32 {
        match self {
            Self::VertexBuffer => 0,
            Self::IndexBuffer => 1,
            Self::PushBuffer => 2,
            Self::Palette => 3,
            Self::Texture => 4,
            Self::Surface => 5,
            Self::Fixup => 6,
            Self::Unknown => 7,
        }
    }
}

/// `X_D3DResource`: `Common`, `Data`, `Lock`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ResourceHeader {
    /// Reference counts, type and flags.
    pub common: u32,
    /// Where the contents live. For textures, surfaces, palettes and vertex
    /// buffers this is a GPU (physical) address; see [`AddressMap`].
    pub data: u32,
    /// GPU time stamp of the last use (written by D3D when it binds).
    pub lock: u32,
}

impl ResourceHeader {
    /// Size of the header in bytes.
    pub const SIZE: u32 = 12;

    /// Reads a header.
    pub fn read(mem: &dyn GuestMemory, addr: u32) -> Result<Self, GuestError> {
        let w = mem.read_words(addr, 3)?;
        Ok(Self {
            common: w[0],
            data: w[1],
            lock: w[2],
        })
    }

    /// The resource type.
    pub fn kind(&self) -> ResourceType {
        ResourceType::from_common(self.common)
    }
}

/// `X_D3DPixelContainer` (texture or surface): the resource header plus
/// `Format` and `Size` (`gfx::texture::TextureDesc::from_header` decodes them).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PixelContainer {
    /// The common header.
    pub resource: ResourceHeader,
    /// `Format` word.
    pub format: u32,
    /// `Size` word (zero for swizzled/compressed).
    pub size: u32,
}

impl PixelContainer {
    /// Size in bytes.
    pub const SIZE: u32 = 20;

    /// Reads a pixel container.
    pub fn read(mem: &dyn GuestMemory, addr: u32) -> Result<Self, GuestError> {
        let w = mem.read_words(addr, 5)?;
        Ok(Self {
            resource: ResourceHeader {
                common: w[0],
                data: w[1],
                lock: w[2],
            },
            format: w[3],
            size: w[4],
        })
    }

    /// The header words that define the image (identity for caching: a
    /// title that rewrites a header in place gets a fresh upload).
    pub fn identity(&self) -> [u32; 3] {
        [self.resource.data, self.format, self.size]
    }
}

/// `X_D3DSurface`: a pixel container plus `Parent` (the texture it is a
/// level of, or null).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SurfaceHeader {
    /// The pixel container.
    pub pixels: PixelContainer,
    /// Owning texture, or 0.
    pub parent: u32,
}

impl SurfaceHeader {
    /// Size in bytes.
    pub const SIZE: u32 = 24;

    /// Reads a surface header.
    ///
    /// All 24 bytes are range-checked first: a header whose first 20
    /// bytes fit below 4 GiB but whose `Parent` would wrap is an error,
    /// never an overflow.
    pub fn read(mem: &dyn GuestMemory, addr: u32) -> Result<Self, GuestError> {
        let w = mem.read_words(addr, 6)?;
        Ok(Self {
            pixels: PixelContainer {
                resource: ResourceHeader {
                    common: w[0],
                    data: w[1],
                    lock: w[2],
                },
                format: w[3],
                size: w[4],
            },
            parent: w[5],
        })
    }

    /// Writes a surface header.
    pub fn write(&self, mem: &mut dyn GuestMemory, addr: u32) -> Result<(), GuestError> {
        crate::guest::check_range(addr, Self::SIZE as usize)?;
        let p = &self.pixels;
        let mut bytes = Vec::with_capacity(Self::SIZE as usize);
        for w in [
            p.resource.common,
            p.resource.data,
            p.resource.lock,
            p.format,
            p.size,
            self.parent,
        ] {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        mem.write(addr, &bytes)
    }

    /// A linear (pitched) 2D surface header in the encoding `TextureDesc::from_header` reads:
    /// format word = 2D, one level, `format` in bits 15..8; size word =
    /// `width-1 | (height-1) << 12 | (pitch/64 - 1) << 24`.
    pub fn linear(format: gfx::D3DFormat, width: u32, height: u32, pitch: u32, data: u32) -> Self {
        let format_word = (2 << 4) | ((format.raw() & 0xFF) << 8) | (1 << 16);
        let size = (width - 1) | ((height - 1) << 12) | ((pitch / 64).saturating_sub(1) << 24);
        let common = 1 | (ResourceType::Surface.raw() << common::TYPE_SHIFT) | common::D3DCREATED;
        Self {
            pixels: PixelContainer {
                resource: ResourceHeader {
                    common,
                    data,
                    lock: 0,
                },
                format: format_word,
                size,
            },
            parent: 0,
        }
    }
}

/// `X_D3DPalette`: the common header; the entry count is in the top two bits
/// of `Common` (`D3DPALETTE_COMMON_PALETTESIZE_MASK`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PaletteHeader {
    /// The common header.
    pub resource: ResourceHeader,
}

impl PaletteHeader {
    /// Reads a palette header.
    pub fn read(mem: &dyn GuestMemory, addr: u32) -> Result<Self, GuestError> {
        Ok(Self {
            resource: ResourceHeader::read(mem, addr)?,
        })
    }

    /// Number of `A8R8G8B8` entries: 256, 128, 64 or 32 for size codes 0..3.
    pub fn entries(&self) -> u32 {
        256 >> (self.resource.common >> 30)
    }
}

/// Converts the `Data` field of a resource (a GPU address) to the guest
/// virtual address the CPU and this device read it through.
///
/// On the Xbox, contiguous (GPU-visible) memory is mapped at
/// `0x80000000 | physical`; D3D stores the physical address in `Data`
/// (`SetTexture` pushes it unmodified as the NV2A texture offset). The
/// runtime owns the real mapping, so the device takes the conversion as
/// configuration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AddressMap {
    /// `virtual = 0x80000000 | (data & 0x0FFFFFFF)`: the Xbox contiguous
    /// mapping. The default.
    #[default]
    XboxContiguous,
    /// `Data` already is a guest virtual address (tests, or a runtime that
    /// maps physical memory 1:1).
    Identity,
    /// `virtual = data + offset` (wrapping).
    Offset(u32),
}

impl AddressMap {
    /// Translates a `Data` value.
    pub fn to_virtual(self, data: u32) -> u32 {
        match self {
            Self::XboxContiguous => 0x8000_0000 | (data & 0x0FFF_FFFF),
            Self::Identity => data,
            Self::Offset(o) => data.wrapping_add(o),
        }
    }

    /// The `Data` value D3D would store for a buffer at guest virtual `va`.
    pub fn to_data(self, va: u32) -> u32 {
        match self {
            Self::XboxContiguous => va & 0x0FFF_FFFF,
            Self::Identity => va,
            Self::Offset(o) => va.wrapping_sub(o),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guest::SparseMemory;

    #[test]
    fn surface_header_roundtrip_and_desc() {
        let mut m = SparseMemory::new();
        m.map(0x1000, 0x100);
        let h = SurfaceHeader::linear(gfx::D3DFormat::LinA8R8G8B8, 640, 480, 2560, 0x0123_4000);
        h.write(&mut m, 0x1000).unwrap();
        let r = SurfaceHeader::read(&m, 0x1000).unwrap();
        assert_eq!(r, h);
        assert_eq!(r.pixels.resource.kind(), ResourceType::Surface);
        let d = gfx::TextureDesc::from_header(r.pixels.format, r.pixels.size).unwrap();
        assert_eq!((d.width, d.height, d.pitch), (640, 480, 2560));
        assert_eq!(d.format, gfx::D3DFormat::LinA8R8G8B8);
    }

    #[test]
    fn palette_sizes_and_address_map() {
        let p = |c| {
            PaletteHeader {
                resource: ResourceHeader {
                    common: c,
                    data: 0,
                    lock: 0,
                },
            }
            .entries()
        };
        assert_eq!(p(0x0003_0001), 256);
        assert_eq!(p(0x4003_0001), 128);
        assert_eq!(p(0xC003_0001), 32);
        assert_eq!(
            AddressMap::XboxContiguous.to_virtual(0x0012_3400),
            0x8012_3400
        );
        assert_eq!(AddressMap::XboxContiguous.to_data(0x8012_3400), 0x0012_3400);
        assert_eq!(AddressMap::Identity.to_virtual(5), 5);
    }
}
