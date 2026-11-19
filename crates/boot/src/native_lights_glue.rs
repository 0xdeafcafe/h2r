//! The native lights' host (docs/graphics/native-renderer.md, "Stage 4:
//! lights and shadows", slice 3), started: the device side of a skipped
//! light, with nothing drawn yet. Inert unless `impl.render.native_lights`
//! is on (the hooks are not placed otherwise).
//!
//! - [`Host::tail`]: `0x00031170`'s tail on the device, in render order,
//!   through `d3d::Device::light_passes_tail` over the hook's guest memory
//!   (lock order as the D3D entries': the guest's memory, which the hook
//!   holds, then the device).
//! - [`Host::target`]: the device's bound target
//!   (`d3d::Device::bound_target`), which the feature records before the
//!   view's lights and compares at each light.
//! - [`Host::accepts`] says no to every light until the native light
//!   passes exist (slice 3's drawing), so every light keeps its D3D passes
//!   and [`Host::open`] is never reached.

use std::sync::Arc;

use engine::memory::{GuestMemory, MemFault};
use feature::native_lights::Host;
use native_render::camera::RenderCamera;
use native_render::light_gpu::{LayeredImage, TextureDim};
use native_render::lights::{Light, Run};

/// The hook's guest memory as the device's.
pub struct DeviceMemory<'a>(pub &'a mut dyn GuestMemory);

fn guest_error(f: MemFault) -> d3d::GuestError {
    d3d::GuestError {
        addr: f.addr,
        len: f.len,
    }
}

impl d3d::GuestMemory for DeviceMemory<'_> {
    fn read(&self, addr: u32, out: &mut [u8]) -> Result<(), d3d::GuestError> {
        self.0.read(addr, out).map_err(guest_error)
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), d3d::GuestError> {
        self.0.write(addr, data).map_err(guest_error)
    }
}

struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn accepts(&self, _mem: &dyn GuestMemory, _light: &Light) -> bool {
        false
    }

    fn open(&self, _camera: &RenderCamera, _run: Arc<Run>) -> bool {
        false
    }

    fn tail(&self, mem: &mut dyn GuestMemory, clip: [u32; 2]) -> bool {
        let Ok(mut d) = self.device.lock() else { return false };
        d.light_passes_tail(&mut DeviceMemory(mem), clip).unwrap_or(false)
    }

    fn target(&self) -> Option<[u32; 8]> {
        self.device.lock().ok().map(|d| d.bound_target())
    }
}

/// Whether the host may draw `light` natively: its constants computed at
/// the hook and no shadow (the shadow maps are not drawn natively, so a
/// shadowed light keeps all its D3D passes).
pub fn drawable(light: &Light) -> bool {
    light.constants.is_some_and(|c| !c.shadowed())
}

/// The `matg` tag block of rasterizer data (`+0x10c`, count then
/// address); `0x00022590` keeps its first element at `[0x00485a80]`.
pub const RASTERIZER_DATA: u32 = 0x10c;
/// The element's bitmap datums the light passes sample: engine texture 1
/// (`+0xc`, the normalization cube) and 9 (`+4`, the attenuation volume).
pub const NORMALIZATION_DATUM: u32 = 0xc;
pub const ATTENUATION_DATUM: u32 = 4;

/// The light textures' bitmaps named by the `matg` at `matg` (its payload
/// address), read through `read(address, len)`: `(normalization,
/// attenuation)`, each `None` when null or unreadable.
pub fn light_bitmaps(read: &dyn Fn(u32, usize) -> Option<Vec<u8>>, matg: u32) -> (Option<map::TagId>, Option<map::TagId>) {
    let word = |a: u32| -> Option<u32> { Some(u32::from_le_bytes(read(a, 4)?.try_into().ok()?)) };
    let element = || -> Option<u32> {
        let count = word(matg.wrapping_add(RASTERIZER_DATA))?;
        (count > 0).then_some(())?;
        word(matg.wrapping_add(RASTERIZER_DATA + 4))
    };
    let Some(e) = element() else { return (None, None) };
    let datum = |o: u32| word(e.wrapping_add(o)).filter(|&d| d != u32::MAX).map(map::TagId);
    (datum(NORMALIZATION_DATUM), datum(ATTENUATION_DATUM))
}

/// A decoded bitmap's first image as a cube (six faces of level 0) or a
/// volume (level 0's slices); `None` when it is not that shape.
pub fn layered(levels: Vec<assets::bitmap::Level>, dim: TextureDim) -> Option<LayeredImage> {
    let mut top: Vec<_> = levels.into_iter().filter(|l| l.level == 0).collect();
    match dim {
        TextureDim::Cube => LayeredImage::cube(top.into_iter().map(|l| (l.face, l.width, l.height, l.rgba)).collect()),
        TextureDim::D3 => {
            top.retain(|l| l.face == 0);
            let [l] = <[_; 1]>::try_from(top).ok()?;
            LayeredImage::volume(l.width, l.height, l.depth, l.rgba)
        }
        TextureDim::D2 => None,
    }
}

/// [`light_bitmaps`] of the map's `matg`, decoded: `(normalization cube,
/// attenuation volume)`. Logs what does not load (the light passes that
/// sample it stay on the D3D path).
pub(crate) fn light_textures<R: std::io::Read + std::io::Seek>(
    map: &mut map::Map<R>,
    meta: &tags::shader::Metadata,
    files: &mut assets::ResourceFiles,
) -> (Option<LayeredImage>, Option<LayeredImage>) {
    let matg = map
        .tags()
        .iter()
        .find(|t| t.group.0 == *b"matg")
        .and_then(|t| t.location.as_ref().map(|l| l.address));
    let Some(matg) = matg else {
        eprintln!("native-lights: no matg; the light textures do not load");
        return (None, None);
    };
    let (n, a) = light_bitmaps(&|addr, len| meta.bytes(addr, len).ok().map(<[u8]>::to_vec), matg);
    let mut decode = |id: Option<map::TagId>, dim, what| {
        let image = id.and_then(|id| {
            let tags::TagData::Bitmap(b) = tags::DecodedTag::read(map, id).ok()?.data else {
                return None;
            };
            let d = assets::decode_image(b.images.first()?, files).ok()?;
            layered(d.levels, dim)
        });
        if image.is_none() {
            eprintln!("native-lights: the {what} ({id:?}) does not load");
        }
        image
    };
    (decode(n, TextureDim::Cube, "normalization cube"), decode(a, TextureDim::D3, "attenuation volume"))
}

/// Installs the host on the device (`d3d_glue`, with the world's).
pub fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    feature::native_lights::set_host(Arc::new(Glue {
        device: Arc::clone(device),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::FlatMemory;

    fn level(face: u32, level: u32, depth: u32) -> assets::bitmap::Level {
        assets::bitmap::Level {
            face,
            level,
            width: 2,
            height: 2,
            depth,
            rgba: vec![face as u8; (16 * depth) as usize],
        }
    }

    #[test]
    fn the_light_bitmaps_come_from_the_rasterizer_data() {
        let mut m = std::collections::HashMap::new();
        let mut put = |a: u32, v: u32| {
            m.insert(a, v);
        };
        let matg = 0x8000_0000;
        put(matg + RASTERIZER_DATA, 1);
        put(matg + RASTERIZER_DATA + 4, 0x8000_1000);
        put(0x8000_1000 + NORMALIZATION_DATUM, 0xe001_0002);
        put(0x8000_1000 + ATTENUATION_DATUM, u32::MAX);
        let read = |a: u32, len: usize| (len == 4).then(|| m.get(&a).map(|v| v.to_le_bytes().to_vec())).flatten();
        assert_eq!(light_bitmaps(&read, matg), (Some(map::TagId(0xe001_0002)), None));
        assert_eq!(light_bitmaps(&read, 0x9000_0000), (None, None), "unreadable");
    }

    #[test]
    fn cube_and_volume_shapes_from_decoded_levels() {
        let faces: Vec<_> = (0..6).flat_map(|f| [level(f, 0, 1), level(f, 1, 1)]).collect();
        let c = layered(faces, TextureDim::Cube).expect("six faces at level 0");
        assert_eq!((c.layers, c.rgba.len()), (6, 96));
        assert!(layered(vec![level(0, 0, 4)], TextureDim::Cube).is_none());
        let v = layered(vec![level(0, 0, 4), level(0, 1, 2)], TextureDim::D3).expect("level 0's slices");
        assert_eq!((v.layers, v.rgba.len()), (4, 64));
        assert!(layered(vec![level(0, 1, 4)], TextureDim::D3).is_none(), "no level 0");
    }

    #[test]
    fn only_unshadowed_lights_with_constants_are_drawable() {
        use native_render::lights::LightConstants;
        let c = LightConstants {
            frame: [0; 36],
            flags: 0,
            colour_a: [0; 3],
            colour_b: [0; 3],
            mode: 0,
            definition_flags: 0,
            definition_8a: 0,
            definition_80: 0,
            falloff_0: 0,
            gel: None,
        };
        let light = |constants| Light {
            handle: 1,
            definition: 2,
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            colour_a: [1.0; 3],
            colour_b: [0.0; 3],
            intensity: 1.0,
            bounds: ([0.0; 3], 1.0),
            shadow: false,
            weight: 1.0,
            constants,
        };
        assert!(drawable(&light(Some(c))));
        assert!(!drawable(&light(Some(LightConstants { flags: 4 | 0x20, ..c }))), "shadowed");
        assert!(!drawable(&light(None)), "no constants");
    }

    #[test]
    fn the_device_sees_the_hooks_memory_and_its_faults() {
        let mut m = FlatMemory::new(0x1000, 0x100);
        let mut dm = DeviceMemory(&mut m);
        d3d::GuestMemory::write_u32(&mut dm, 0x1010, 0xdead_beef).unwrap();
        assert_eq!(d3d::GuestMemory::read_u32(&dm, 0x1010).unwrap(), 0xdead_beef);
        let e = d3d::GuestMemory::read_u32(&dm, 0x10fe).unwrap_err();
        assert_eq!((e.addr, e.len), (0x10fe, 4));
        assert!(d3d::GuestMemory::write(&mut dm, 0x0ffc, &[0; 4]).is_err());
        assert_eq!(m.u32(0x1010).unwrap(), 0xdead_beef);
    }
}
