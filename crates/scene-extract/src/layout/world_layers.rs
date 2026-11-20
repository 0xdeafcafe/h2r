//! Guest identifiers shared by opaque-layer preparation and comparison.

use crate::GuestRead;
use crate::layout::object_nodes::{TEXACCUM_HEADER, TEXACCUM_TARGET, texaccum_bound};
use crate::layout::objects::{RecordOwner, record_owner};
use crate::layout::skip::{RENDER_RECORD_SIZE, RENDER_RECORDS};

/// The indexed draw entry used by the normal geometry emitter.
pub const DRAW_INDEXED_VERTICES: u32 = 0x003f_afa0;
/// The indexed draw returns inside the geometry emitter (0x0004daa0).
const INDEXED_RETURNS: [u32; 4] = [0x0004_dae0, 0x0004_db45, 0x0004_dbe1, 0x0004_dc3f];
/// Stack distance from the D3D call to the emitter's caller return.
const EMITTER_RETURN_SLOT: u32 = 0x28;
/// The call from the standard geometry handler, 0x0004d0b0.
const GEOMETRY_EMITTER_RETURN: u32 = 0x0004_d626;
/// Stack distance to the standard handler's return. The seven stdcall
/// arguments follow it as in layer-draw.md.
const HANDLER_FRAME: u32 = 0x78;

/// The separate sky-model renderer, called by 0x0003ede0.
pub const SKY_INDEXED_RETURN: u32 = 0x0004_c97b;

/// The caller that set up this layer's list, attachments and state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Site {
    Scene,
    AlternateView,
    FirstPerson,
    Sky,
    Other,
}

/// Return addresses after calls to the layer draw in 0x0002bcd0 and
/// 0x0003fdf0. Match the layer as well: a scene's target-1 setup is
/// specific to its layer-1 call.
pub fn layer_site(layer: u32, return_address: u32) -> Site {
    match (layer, return_address) {
        (1, 0x0002_be4a)
        | (3, 0x0002_bed5)
        | (5, 0x0002_bf39)
        | (8, 0x0002_bfaa)
        | (7, 0x0002_c039)
        | (2, 0x0002_c060) => Site::Scene,
        (1, 0x0002_bd5c) | (3, 0x0002_bd74) | (5, 0x0002_bd8c) | (2, 0x0002_bda8) => {
            Site::AlternateView
        }
        (1, 0x0004_00ab) | (3, 0x0004_0136) | (8, 0x0004_01a7) | (7, 0x0004_0271) => {
            Site::FirstPerson
        }
        _ => Site::Other,
    }
}

/// Scene layers whose indexed draws can be compared.
pub fn opaque_layer(layer: u32) -> bool {
    matches!(layer, 1 | 2 | 3 | 5 | 7 | 8)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    Cluster(u32),
    Placement(u32),
    Object {
        handle: u32,
        model: u32,
        section: u32,
    },
}

/// The live render record and resolved implementation for an indexed draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub shader: u32,
    pub variant: u8,
    pub implementation: u8,
    pub sorted_slot: u32,
    pub record: u16,
    pub part: u8,
    pub owner: Owner,
}

impl Geometry {
    /// Decode only the standard handler calling the standard emitter.
    /// A sky or special handler can share the D3D entry and the layer.
    pub fn read(mem: &dyn GuestRead, esp: u32, layer: u32) -> Option<Self> {
        let stack = |offset| mem.read_u32(esp.checked_add(offset)?);
        if !INDEXED_RETURNS.contains(&stack(0)?)
            || stack(EMITTER_RETURN_SLOT)? != GEOMETRY_EMITTER_RETURN
            || stack(HANDLER_FRAME + 16)? != layer
        {
            return None;
        }
        let shader = stack(HANDLER_FRAME + 4)?;
        let variant = stack(HANDLER_FRAME + 12)? as u8;
        let argument = stack(HANDLER_FRAME + 24)?;
        let entry = stack(HANDLER_FRAME + 28)?;
        if entry == 0 {
            return None;
        }
        let implementation = mem.read_u32(entry.checked_add(4)?)? as u8;
        let record = (argument >> 8) as u16;
        if (record as i16) < 0 {
            return None;
        }
        let base = RENDER_RECORDS + RENDER_RECORD_SIZE * u32::from(record);
        let packed = mem.read_u32(base + 0xc)?;
        let section = (packed >> 9) & 0x1ff;
        let owner = match record_owner(mem, record)? {
            RecordOwner::World if mem.read_u32(base)? & (1 << 12) == 0 => Owner::Cluster(section),
            RecordOwner::World => Owner::Placement((packed >> 18) & 0x7ff),
            RecordOwner::Object(handle) => Owner::Object {
                handle,
                model: mem.read_u32(base + 4)?,
                section,
            },
            RecordOwner::Unresolved => return None,
        };
        Some(Self {
            shader,
            variant,
            implementation,
            sorted_slot: entry,
            record,
            part: argument as u8,
            owner,
        })
    }
}

/// The two views of the same texaccum resource. The layer-1 attachment
/// comes from 0x0001dcf0(1); engine texture 3 uses 0x0001dcc0(1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Texaccum {
    pub surface: Option<u32>,
    pub texture: u32,
    pub size: (u32, u32),
}

impl Texaccum {
    pub fn read(mem: &dyn GuestRead) -> Option<Self> {
        if !texaccum_bound(mem) {
            return None;
        }
        let w = mem.read_u32(TEXACCUM_TARGET + 0x80)?;
        let h = mem.read_u32(TEXACCUM_TARGET + 0x84)?;
        if w == 0 || h == 0 {
            return None;
        }
        Some(Self {
            surface: (mem.read_u32(TEXACCUM_TARGET + 0x78)? as i32 > 0)
                .then_some(TEXACCUM_TARGET + 0x18),
            texture: TEXACCUM_HEADER,
            size: (w, h),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Mem;

    const STACK: u32 = 0x0069_0000;
    const ENTRY: u32 = 0x0069_0100;

    fn draw(mem: &mut Mem, record: u16, flags: u32, packed: u32) {
        mem.w(STACK, INDEXED_RETURNS[0]);
        mem.w(STACK + EMITTER_RETURN_SLOT, GEOMETRY_EMITTER_RETURN);
        mem.w(STACK + HANDLER_FRAME + 4, 0xe000_0042);
        mem.w(STACK + HANDLER_FRAME + 12, 2);
        mem.w(STACK + HANDLER_FRAME + 16, 1);
        mem.w(STACK + HANDLER_FRAME + 24, u32::from(record) << 8 | 7);
        mem.w(STACK + HANDLER_FRAME + 28, ENTRY);
        mem.w(ENTRY + 4, 3);
        crate::testing::record(mem, u32::from(record), None);
        let base = RENDER_RECORDS + RENDER_RECORD_SIZE * u32::from(record);
        mem.w(base, flags);
        mem.w(base + 0xc, packed);
    }

    #[test]
    fn indexed_draw_identity_includes_part_variant_and_implementation() {
        let mut mem = Mem::default();
        draw(&mut mem, 4, 0, 9 << 9);
        assert_eq!(
            Geometry::read(&mem, STACK, 1),
            Some(Geometry {
                shader: 0xe000_0042,
                variant: 2,
                implementation: 3,
                sorted_slot: ENTRY,
                record: 4,
                part: 7,
                owner: Owner::Cluster(9),
            })
        );
        draw(&mut mem, 4, 1 << 12, 17 << 18);
        assert_eq!(
            Geometry::read(&mem, STACK, 1).unwrap().owner,
            Owner::Placement(17)
        );
        let handle = crate::testing::object(&mut mem, 5, 0xe000_0100, 1);
        draw(&mut mem, 4, 0, 29 << 9);
        crate::testing::record(&mut mem, 4, Some(crate::testing::lighting(5 + 7)));
        // The object record helper writes its own key word; restore the section.
        mem.w(RENDER_RECORDS + RENDER_RECORD_SIZE * 4 + 0xc, 29 << 9);
        assert_eq!(
            Geometry::read(&mem, STACK, 1).unwrap().owner,
            Owner::Object {
                handle,
                model: 0xe100_0004,
                section: 29
            },
        );
        assert_eq!(Geometry::read(&mem, STACK, 3), None);
        mem.w(STACK + EMITTER_RETURN_SLOT, 0xbeef);
        assert_eq!(Geometry::read(&mem, STACK, 1), None);
    }

    #[test]
    fn the_same_layer_number_has_different_view_sites() {
        assert_eq!(layer_site(1, 0x0002_be4a), Site::Scene);
        assert_eq!(layer_site(1, 0x0002_bd5c), Site::AlternateView);
        assert_eq!(layer_site(1, 0x0004_00ab), Site::FirstPerson);
        assert_eq!(layer_site(3, 0x0002_be4a), Site::Other);
        assert_eq!(layer_site(2, 0x0002_c060), Site::Scene);
    }

    #[test]
    fn texaccum_surface_is_distinct_from_its_sampled_header() {
        let mut mem = Mem::default();
        mem.w(TEXACCUM_TARGET + 0x88, 1);
        mem.w(TEXACCUM_TARGET + 0x94, 0);
        mem.w(TEXACCUM_TARGET + 0x80, 640);
        mem.w(TEXACCUM_TARGET + 0x84, 480);
        mem.w(TEXACCUM_TARGET + 0x78, 1);
        let target = Texaccum::read(&mem).unwrap();
        assert_eq!(target.size, (640, 480));
        assert_eq!(target.surface, Some(TEXACCUM_TARGET + 0x18));
        assert_eq!(target.texture, TEXACCUM_HEADER);
        mem.w(TEXACCUM_TARGET + 0x78, 0);
        assert_eq!(Texaccum::read(&mem).unwrap().surface, None);
        mem.w(TEXACCUM_TARGET + 0x94, 1 << 8);
        assert_eq!(Texaccum::read(&mem), None);
    }
}
