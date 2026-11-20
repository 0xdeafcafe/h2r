//! Lighting and atmosphere fields reviewed against the retail engine.
//! See `docs/formats/lighting.md` for per-field provenance and explicit opaque areas.
//! The shared BSP reader owns geometry and lightmap linkage; this module
//! extends it. Every block read goes through the shared [`TagView`], so
//! aliased blocks are charged to its one per-payload budget.
use crate::geometry::{Section, TagView, VertexBuffer, f32_at, u32_at};
use crate::structure::{Lightmap, LightmapGroup};
use crate::wire::TagReference;
use crate::{Error, Result};
use map::{FourCC, Map, TagId};
use std::{
    borrow::Cow,
    io::{Read, Seek},
};

pub const ATMOSPHERE_GROUPS: [FourCC; 7] = [
    FourCC(*b"sky "),
    FourCC(*b"fog "),
    FourCC(*b"fpch"),
    FourCC(*b"gldf"),
    FourCC(*b"MGS2"),
    FourCC(*b"weat"),
    FourCC(*b"snde"),
];

fn floats<const N: usize>(b: &[u8], o: usize) -> Result<[f32; N]> {
    let values = std::array::from_fn(|i| f32_at(b, o + 4 * i));
    if values.iter().any(|v| !v.is_finite()) {
        return Err(Error::Invalid("non-finite reviewed lighting scalar"));
    }
    Ok(values)
}
fn reference(b: &[u8], o: usize) -> TagReference {
    TagReference {
        group: FourCC::from_u32(u32_at(b, o)),
        id: TagId(u32_at(b, o + 4)),
    }
}
fn checked_reference(b: &[u8], o: usize, expected: FourCC) -> Result<TagReference> {
    let r = reference(b, o);
    if r.id != TagId::NULL && r.group != expected {
        return Err(Error::Invalid(
            "reviewed atmosphere reference has wrong class",
        ));
    }
    Ok(r)
}
fn read<R: Read + Seek>(map: &mut Map<R>, id: TagId, group: FourCC) -> Result<(Vec<u8>, u32)> {
    let tag = map.tag(id)?;
    if tag.group != group {
        return Err(Error::Invalid("wrong lighting tag group"));
    }
    let location = tag.location.ok_or(map::Error::NoTagData)?;
    // Retail can give a second identity zero bytes at another same-class
    // tag's exact metadata address. Resolve that explicit alias only.
    let source = if location.size == 0 {
        map.tags()
            .iter()
            .filter(|t| t.group == group)
            .filter_map(|t| {
                let l = t.location?;
                (l.address == location.address && l.space == location.space && l.size != 0)
                    .then_some((l.size, t.id))
            })
            .max_by_key(|(size, _)| *size)
            .map(|(_, id)| id)
            .ok_or(Error::Invalid(
                "zero-sized lighting tag has no same-class root alias",
            ))?
    } else {
        id
    };
    let source_location = map.tag(source)?.location.ok_or(map::Error::NoTagData)?;
    let offset = source_location.file_offset()?;
    Ok((
        map.read_file_range(offset, u64::from(source_location.size))?,
        location.address,
    ))
}

/// A spatial lighting sample. Three channels each have nine coefficients.
/// The coefficient basis/order is deliberately not assigned a shader formula.
#[derive(Clone, Debug)]
pub struct LightProbe {
    pub position: [f32; 3],
    pub coefficients: [[f32; 9]; 3],
    pub rotation: f32,
    /// Interpolated vector passed out alongside the coefficients by 0x22cf0.
    pub interpolated_direction: [f32; 3],
    pub raw: [u8; 0xdc],
}
impl LightProbe {
    fn decode(b: &[u8]) -> Result<Self> {
        Ok(Self {
            position: floats(b, 0)?,
            coefficients: [floats(b, 0xc)?, floats(b, 0x30)?, floats(b, 0x54)?],
            rotation: floats::<1>(b, 0xa0)?[0],
            interpolated_direction: floats(b, 0xa4)?,
            raw: b
                .try_into()
                .map_err(|_| Error::Invalid("light probe size"))?,
        })
    }
}

#[derive(Clone, Debug)]
pub struct LightmapLighting {
    /// Existing decoder: bitmap, palettes, render info and bucket geometry.
    pub linkage: Lightmap,
    /// One probe list per lightmap group, in the same order as `linkage.groups`.
    pub probes: Vec<Vec<LightProbe>>,
    /// Root +0x34: positive values enable time cycling of probe coefficients.
    pub cycle_period: f32,
}
impl LightmapLighting {
    pub fn read<R: Read + Seek>(map: &mut Map<R>, id: TagId) -> Result<Self> {
        let (bytes, address) = read(map, id, FourCC(*b"ltmp"))?;
        Self::decode(id, &TagView::new(&bytes, address))
    }
    /// Decode the shared linkage, then each group's probe block (+0x38),
    /// all charged to `view`'s one budget.
    pub fn decode(id: TagId, view: &TagView<'_>) -> Result<Self> {
        let h = view.at(0, 0x104)?;
        let linkage = Lightmap::decode(id, view)?;
        let probes = linkage
            .groups
            .iter()
            .map(|g| Self::probes(view, g))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            linkage,
            probes,
            cycle_period: floats::<1>(h, 0x34)?[0],
        })
    }
    fn probes(view: &TagView<'_>, g: &LightmapGroup) -> Result<Vec<LightProbe>> {
        view.block(&g.raw, 0x38, 0xdc)?
            .into_iter()
            .map(LightProbe::decode)
            .collect()
    }
}

/// Convert one P8 texel index through a lightmap palette (A8R8G8B8 words).
/// Palette alpha is retained; this does not apply a material/shader equation.
pub fn palette_rgba(palette: &[u32; 256], index: u8) -> [u8; 4] {
    let [b, g, r, a] = palette[index as usize].to_le_bytes();
    [r, g, b, a]
}

/// Direction, lens flare and the raw flags of the first lighting parameter.
#[derive(Clone, Debug)]
pub struct SkyLight {
    pub yaw_pitch: [f32; 2],
    pub lens_flare: TagReference,
    /// First word of the +0x2c child block, absent when the block is empty.
    pub parameter_flags: Option<u32>,
    pub raw: [u8; 0x34],
}
#[derive(Clone, Debug)]
pub struct Sky {
    pub model: TagReference,
    pub scale: f32,
    pub camera_translation_scale: f32,
    /// Fallback object-lighting RGB at +0x34 (0x000d1a30).
    pub fallback_rgb: [f32; 3],
    pub rotation: f32,
    pub lights: Vec<SkyLight>,
    /// Light records resolve into a different same-class tag payload.
    pub lights_alias: bool,
    /// Entire payload, including unreviewed blocks and fields.
    pub opaque: Vec<u8>,
}
impl Sky {
    fn decode(view: &TagView<'_>) -> Result<Self> {
        Self::decode_with_aliases(view, |_, _| {
            Err(Error::Invalid("sky block outside payload"))
        })
    }

    fn decode_with_aliases(
        view: &TagView<'_>,
        mut read_alias: impl FnMut(u32, u32) -> Result<Vec<u8>>,
    ) -> Result<Self> {
        // Sky records and their parameter blocks can belong to another sky.
        // Charge each visit (even repeated aliases) before reading it.
        let mut block = |record: &[u8], field, stride: u32| -> Result<Cow<'_, [u8]>> {
            let count = u32_at(record, field);
            let address = u32_at(record, field + 4);
            let size = count
                .checked_mul(stride)
                .ok_or(Error::Invalid("sky block size"))?;
            if count != 0 && !address.is_multiple_of(4) {
                return Err(Error::Invalid("unaligned sky block"));
            }
            view.charge(u64::from(size))?;
            Ok(match view.range(address, u64::from(size)) {
                Ok(local) => Cow::Borrowed(local),
                Err(_) => Cow::Owned(read_alias(address, size)?),
            })
        };
        let h = view.at(0, 0xac)?;
        let records = block(h, 0x78, 0x34)?;
        let lights_alias = matches!(&records, Cow::Owned(_));
        let lights = records
            .chunks_exact(0x34)
            .map(|b| {
                let params = block(b, 0x2c, 0x28)?;
                Ok(SkyLight {
                    yaw_pitch: floats(b, 0xc)?,
                    lens_flare: checked_reference(b, 0x14, FourCC(*b"lens"))?,
                    parameter_flags: (!params.is_empty()).then(|| u32_at(&params, 0)),
                    raw: b.try_into().map_err(|_| Error::Invalid("sky light size"))?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            model: checked_reference(h, 0, FourCC(*b"mode"))?,
            scale: floats::<1>(h, 0x14)?[0],
            camera_translation_scale: floats::<1>(h, 0x18)?[0],
            fallback_rgb: floats(h, 0x34)?,
            rotation: floats::<1>(h, 0x80)?[0],
            lights,
            lights_alias,
            opaque: view.bytes().to_vec(),
        })
    }
}

/// Planar fog's four sound references. Optical parameters and patch descriptors
/// are opaque until their engine consumers have been reviewed.
#[derive(Clone, Debug)]
pub struct Fog {
    pub sound_references: [TagReference; 4],
    pub opaque: Vec<u8>,
}
/// Fog patch: the disc establishes a bitmap reference, not its shading formula.
#[derive(Clone, Debug)]
pub struct FogPatch {
    pub bitmap: TagReference,
    pub opaque: Vec<u8>,
}
/// A reviewed light-volume layer prefix; function/mapping blobs remain opaque.
#[derive(Clone, Debug)]
pub struct LightVolumeLayer {
    pub flags: u32,
    pub bitmap: TagReference,
    pub sample_count: u32,
    pub raw: [u8; 0x98],
}
#[derive(Clone, Debug)]
pub struct LightVolume {
    /// Root +0/+4: the camera-distance fade interval `[near, far]`. The
    /// engine (0x0004f061..0x0004f098) forms camera minus volume position,
    /// takes its length via 0x00030bf0 (`sqrt(x*x + y*y + z*z)`), and fades
    /// from 1 at `near` to 0 at `far`; see [`Self::distance_fade_factor`].
    pub distance_fade: [f32; 2],
    pub layers: Vec<LightVolumeLayer>,
    pub opaque: Vec<u8>,
}
impl LightVolume {
    /// Engine fade for a camera-minus-volume displacement (0x0004f0a3..
    /// 0x0004f115): with `far > near`, `clamp((far - |d|) / (far - near),
    /// 0, 1)`; otherwise the initial 1.0 (constant at 0x0045dbc0) is kept.
    /// The engine then scales this by a per-instance value (`[ebx+30h]`),
    /// which is outside the tag.
    pub fn distance_fade_factor(&self, displacement: [f32; 3]) -> f32 {
        let [near, far] = self.distance_fade;
        if far <= near {
            return 1.0;
        }
        let distance = displacement.iter().map(|v| v * v).sum::<f32>().sqrt();
        ((far - distance) / (far - near)).clamp(0.0, 1.0)
    }
    fn decode(view: &TagView<'_>) -> Result<Self> {
        let h = view.at(0, 0x10)?;
        let layers = view
            .block(h, 8, 0x98)?
            .into_iter()
            .map(|b| {
                Ok(LightVolumeLayer {
                    flags: u32_at(b, 0),
                    bitmap: checked_reference(b, 4, FourCC(*b"bitm"))?,
                    sample_count: u32_at(b, 0xc),
                    raw: b
                        .try_into()
                        .map_err(|_| Error::Invalid("light volume layer size"))?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            distance_fade: floats(h, 0)?,
            layers,
            opaque: view.bytes().to_vec(),
        })
    }
}
/// The twelve environment floats consumed by 0x0021ece0. No DSP emulation.
#[derive(Clone, Debug)]
pub struct SoundEnvironment {
    pub room_db: f32,
    pub room_hf_db: f32,
    pub room_rolloff: f32,
    pub decay_seconds: f32,
    pub decay_hf_ratio: f32,
    pub reflections_db: f32,
    pub reflections_delay_seconds: f32,
    pub reverb_db: f32,
    pub reverb_delay_seconds: f32,
    pub diffusion: f32,
    pub density: f32,
    pub hf_reference_hz: f32,
    pub opaque: Vec<u8>,
}
impl SoundEnvironment {
    fn decode(view: &TagView<'_>) -> Result<Self> {
        let h = view.at(0, 0x48)?;
        let v = floats::<12>(h, 8)?;
        Ok(Self {
            room_db: v[0],
            room_hf_db: v[1],
            room_rolloff: v[2],
            decay_seconds: v[3],
            decay_hf_ratio: v[4],
            reflections_db: v[5],
            reflections_delay_seconds: v[6],
            reverb_db: v[7],
            reverb_delay_seconds: v[8],
            diffusion: v[9],
            density: v[10],
            hf_reference_hz: v[11],
            opaque: view.bytes().to_vec(),
        })
    }
}
/// A known group whose internals have not been reviewed. No pointers followed.
#[derive(Clone, Debug)]
pub struct OpaqueAtmosphere {
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug)]
pub enum Atmosphere {
    Sky(Sky),
    Fog(Fog),
    FogPatch(FogPatch),
    LightVolume(LightVolume),
    SoundEnvironment(SoundEnvironment),
    GlobalLighting(OpaqueAtmosphere),
    Weather(OpaqueAtmosphere),
}
impl Atmosphere {
    pub fn read<R: Read + Seek>(map: &mut Map<R>, id: TagId) -> Result<Self> {
        let group = map.tag(id)?.group;
        let (bytes, address) = read(map, id, group)?;
        let view = TagView::new(&bytes, address);
        let value = if group == FourCC(*b"sky ") {
            let location = map.tag(id)?.location.ok_or(map::Error::NoTagData)?;
            Self::Sky(Sky::decode_with_aliases(&view, |pointer, size| {
                let owned = map.tags().iter().any(|t| {
                    t.group == group
                        && t.location.is_some_and(|l| {
                            l.space == location.space
                                && pointer >= l.address
                                && u64::from(pointer) + u64::from(size)
                                    <= u64::from(l.address) + u64::from(l.size)
                        })
                });
                if !owned {
                    return Err(Error::Invalid("sky block has no same-class owner"));
                }
                let offset = location.space.translate(pointer, size)?;
                Ok(map.read_file_range(offset, u64::from(size))?)
            })?)
        } else {
            Self::decode(group, &view)?
        };
        for r in value.references() {
            if r.id != TagId::NULL && map.tag(r.id)?.group != r.group {
                return Err(Error::Invalid("atmosphere reference group mismatch"));
            }
        }
        Ok(value)
    }
    pub fn decode(group: FourCC, view: &TagView<'_>) -> Result<Self> {
        Ok(match &group.0 {
            b"sky " => Self::Sky(Sky::decode(view)?),
            b"fog " => {
                let h = view.at(0, 0x60)?;
                Self::Fog(Fog {
                    sound_references: [
                        checked_reference(h, 0x38, FourCC(*b"lsnd"))?,
                        checked_reference(h, 0x40, FourCC(*b"snde"))?,
                        checked_reference(h, 0x50, FourCC(*b"snd!"))?,
                        checked_reference(h, 0x58, FourCC(*b"snd!"))?,
                    ],
                    opaque: view.bytes().to_vec(),
                })
            }
            b"fpch" => {
                let h = view.at(0, 0x50)?;
                Self::FogPatch(FogPatch {
                    bitmap: checked_reference(h, 0x14, FourCC(*b"bitm"))?,
                    opaque: view.bytes().to_vec(),
                })
            }
            b"MGS2" => Self::LightVolume(LightVolume::decode(view)?),
            b"snde" => Self::SoundEnvironment(SoundEnvironment::decode(view)?),
            b"gldf" => Self::GlobalLighting(OpaqueAtmosphere {
                bytes: view.bytes().to_vec(),
            }),
            b"weat" => Self::Weather(OpaqueAtmosphere {
                bytes: view.bytes().to_vec(),
            }),
            _ => return Err(Error::UnsupportedGroup(group)),
        })
    }
    pub fn references(&self) -> Vec<TagReference> {
        match self {
            Self::Sky(s) => std::iter::once(s.model)
                .chain(s.lights.iter().map(|l| l.lens_flare))
                .collect(),
            Self::Fog(f) => f.sound_references.to_vec(),
            Self::FogPatch(f) => vec![f.bitmap],
            Self::LightVolume(v) => v.layers.iter().map(|l| l.bitmap).collect(),
            _ => Vec::new(),
        }
    }
    pub fn opaque_bytes(&self) -> usize {
        match self {
            Self::Sky(s) => s.opaque.len(),
            Self::Fog(s) => s.opaque.len(),
            Self::FogPatch(s) => s.opaque.len(),
            Self::LightVolume(s) => s.opaque.len(),
            Self::SoundEnvironment(s) => s.opaque.len(),
            Self::GlobalLighting(s) | Self::Weather(s) => s.bytes.len(),
        }
    }
}

/// Scenario BSP order, never tag-index order. References retain full salted IDs.
pub fn bsp_lightmaps<R: Read + Seek>(map: &mut Map<R>) -> Result<Vec<(TagId, Option<TagId>)>> {
    let scenario = map.scenario().ok_or(Error::Invalid("scenario missing"))?.id;
    let (bytes, address) = read(map, scenario, FourCC(*b"scnr"))?;
    let view = TagView::new(&bytes, address);
    let h = view.at(0, 0x218)?;
    let mut out = Vec::new();
    for b in view.block(h, 0x210, 0x44)? {
        let bsp = TagId(u32_at(b, 0x14));
        if map.tag(bsp)?.group != FourCC(*b"sbsp") {
            return Err(Error::Invalid("scenario BSP class"));
        }
        let lightmap = TagId(u32_at(b, 0x1c));
        let lightmap = if lightmap == TagId::NULL {
            None
        } else {
            if map.tag(lightmap)?.group != FourCC(*b"ltmp") {
                return Err(Error::Invalid("scenario lightmap class"));
            }
            Some(lightmap)
        };
        out.push((bsp, lightmap));
    }
    Ok(out)
}

/// Reviewed extra per-vertex data. `0x30` is a signed lighting vector;
/// it is not a unit normal. Other stream IDs remain in the geometry section.
#[derive(Clone, Debug, PartialEq)]
pub enum VertexLighting {
    Color(Vec<[f32; 3]>),
    Vector(Vec<[f32; 3]>),
}
pub fn unpack_lighting_vector(word: u32) -> [f32; 3] {
    let component = |bits: u32, width: u32| {
        let signed = ((bits << (32 - width)) as i32) >> (32 - width);
        (2 * signed + 1) as f32 / ((1u32 << width) - 1) as f32
    };
    [
        component(word & 0x7ff, 11),
        component((word >> 11) & 0x7ff, 11),
        component(word >> 22, 10),
    ]
}
/// Decode one of `section`'s streams, if it is a reviewed lighting stream.
pub fn vertex_lighting(section: &Section, stream: &VertexBuffer) -> Result<Option<VertexLighting>> {
    let expected = match stream.type_id {
        0x2e | 0x30 => 4,
        0x2f => 3,
        _ => return Ok(None),
    };
    let data = section.data(stream);
    if stream.stride != expected || !data.len().is_multiple_of(expected as usize) {
        return Err(Error::Invalid("lighting stream stride/size mismatch"));
    }
    let values = data
        .chunks_exact(expected as usize)
        .map(|b| match stream.type_id {
            0x2f => [b[0], b[1], b[2]].map(|v| v as f32 / 255.0),
            0x2e => {
                let [b, g, r, _] = u32_at(b, 0).to_le_bytes();
                [r, g, b].map(|v| v as f32 / 255.0)
            }
            _ => unpack_lighting_vector(u32_at(b, 0)),
        })
        .collect();
    Ok(Some(if stream.type_id == 0x30 {
        VertexLighting::Vector(values)
    } else {
        VertexLighting::Color(values)
    }))
}

/// Retail real spherical-harmonic basis, order 3 (nine terms), from
/// 0x00143360. The caller supplies a unit direction; no normalization/clamping.
/// Term zero is constant ambient; 1..3 are linear directional terms and
/// 4..8 the quadratic terms. Signs follow the engine, not a library convention.
pub fn sh_basis([x, y, z]: [f32; 3]) -> [f32; 9] {
    let c = 1.0 / std::f32::consts::PI.sqrt();
    let l1 = 3.0f32.sqrt() * c * 0.5;
    let l2 = 15.0f32.sqrt() * c * 0.5;
    [
        c * 0.5,
        -l1 * y,
        l1 * z,
        -l1 * x,
        l2 * x * y,
        -l2 * y * z,
        (3.0 * z * z - 1.0) * 5.0f32.sqrt() * c * 0.25,
        -l2 * z * x,
        (x * x - y * y) * l2 * 0.5,
    ]
}
impl LightProbe {
    /// Direction-independent contribution. Values are linear floats, neither
    /// clamped nor gamma converted; coefficient zero carries normalization.
    pub fn ambient_rgb(&self) -> [f32; 3] {
        self.coefficients
            .map(|channel| channel[0] * sh_basis([0.0; 3])[0])
    }
    /// Dot each RGB coefficient array with the engine's nine-term basis.
    /// This is probe evaluation, not a complete material or visibility shader.
    pub fn evaluate(&self, direction: [f32; 3]) -> [f32; 3] {
        let basis = sh_basis(direction);
        self.coefficients
            .map(|channel| channel.into_iter().zip(basis).map(|(c, b)| c * b).sum())
    }
}
