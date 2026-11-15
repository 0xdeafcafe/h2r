//! Halo 2 animation graphs (`jmad`): skeleton nodes, modes/classes/types/actions,
//! the animation list, and the per-animation record that locates compressed
//! frame data. Sampling lives in [`crate::anim_sample`]; the format notes and
//! evidence are in `docs/animation.md`.
//!
//! Layout provenance. Every offset below was read from the retail engine's
//! animation code (see `docs/engine-map.md`, "Animation") and checked against
//! every `jmad` tag on the disc: block bounds fit the payload, and the blocks
//! whose stride is modelled never overlap one another. Names of fields the
//! engine never labels are community vocabulary, marked (S) in the docs.
//! Fields nobody could explain are kept as raw words.
//!
//! The module is hand-written and self-contained, like [`crate::sound`].
// Block strides are runtime values of the layout tables; `chunks_exact` with a
// named stride constant reads better than `as_chunks` here.
#![allow(clippy::chunks_exact_to_as_chunks)]
use crate::anim_sample::AnimationClip;
use crate::sound::ResourceReader;
use crate::{Error, Result, wire::StringId, wire::TagReference};
use map::{FourCC, Map, RawPointer, TagId};
use std::io::{Read, Seek};
use std::sync::Arc;

/// Largest animation resource `read_resource` will load.
pub const MAX_RESOURCE_BYTES: u64 = 256 << 20;

pub(crate) fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
pub(crate) fn i16_at(b: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([b[o], b[o + 1]])
}
pub(crate) fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
pub(crate) fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_bits(u32_at(b, o))
}

/// Size of the fixed `jmad` header; the first block's elements start here.
pub const HEADER_SIZE: usize = 0xbc;
/// Sizes of the element types, as strides of the owning blocks.
pub const NODE_SIZE: usize = 0x20;
pub const REFERENCE_SIZE: usize = 0x0c;
pub const BLEND_SCREEN_SIZE: usize = 0x1c;
pub const ANIMATION_SIZE: usize = 0x6c;
pub const MODE_SIZE: usize = 0x14;
pub const CLASS_SIZE: usize = 0x14;
pub const TYPE_SIZE: usize = 0x34;
pub const ACTION_SIZE: usize = 8;
pub const INHERITANCE_SIZE: usize = 0x20;
pub const RESOURCE_SIZE: usize = 0x14;
pub const STATIC_POSE_SIZE: usize = 0x18;

/// Header offsets of the blocks (count at `off`, pointer at `off + 4`).
mod hdr {
    pub const NODES: usize = 0x0c;
    pub const SOUND_REFS: usize = 0x14;
    pub const EFFECT_REFS: usize = 0x1c;
    pub const BLEND_SCREENS: usize = 0x24;
    pub const ANIMATIONS: usize = 0x2c;
    pub const MODES: usize = 0x34;
    pub const UNMODELLED_A: usize = 0x3c;
    pub const UNMODELLED_B: usize = 0x44;
    pub const INHERITANCE: usize = 0x4c;
    pub const NAME_PAIRS: usize = 0x54;
    pub const RESOURCES: usize = 0xac;
    pub const STATIC_POSES: usize = 0xb4;
}

/// Elements of a block whose count/pointer pair sits at `off` in `owner` (the
/// tag header or an element inside `tag`), bounds-checked against `tag`.
fn sub_block<'a>(
    tag: &'a [u8],
    base: u32,
    owner: &[u8],
    off: usize,
    stride: usize,
) -> Result<&'a [u8]> {
    let count = u32_at(owner, off);
    if count == 0 {
        return Ok(&[]);
    }
    let address = u32_at(owner, off + 4);
    let size = u64::from(count) * stride as u64;
    let bad = || Error::Bounds { address, size };
    let start = address.checked_sub(base).ok_or_else(bad)? as usize;
    let end = start.checked_add(size as usize).ok_or_else(bad)?;
    tag.get(start..end).ok_or_else(bad)
}
fn block(bytes: &[u8], base: u32, off: usize, stride: usize) -> Result<&[u8]> {
    sub_block(bytes, base, bytes, off, stride)
}
/// Payload span of a block, for overlap checks: `(start, end)` relative to the tag.
fn span(bytes: &[u8], base: u32, off: usize, stride: usize) -> Option<(usize, usize)> {
    let count = u32_at(bytes, off) as usize;
    if count == 0 {
        return None;
    }
    let start = (u32_at(bytes, off + 4).wrapping_sub(base)) as usize;
    Some((start, start + count * stride))
}

/// Skeleton node. Offsets: engine-confirmed only for the block stride (`0x20`,
/// equal to each animation's node count on the disc); field names are (S).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Node {
    pub name: StringId,
    /// Sibling and child links are node indices, -1 for none.
    pub next_sibling: i16,
    pub first_child: i16,
    pub parent: i16,
    /// +0xa, small flag word.
    pub model_flags: u16,
    /// +0xc.
    pub joint_flags: u32,
    /// +0x10, three floats; zero on every node of the retail disc samples read.
    pub base_vector: [f32; 3],
    /// +0x1c.
    pub vector_range: f32,
}

/// `{tag reference, flags}` used for the sound and effect reference blocks.
/// Engine: the reference datum is read at `+4`, a flag word at `+8`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reference {
    pub tag: TagReference,
    pub flags: u16,
}

/// Aim ("blend") screen. 28 bytes; the nine fields are the community layout
/// shape (S), checked only for size and that float fields are finite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlendScreen {
    pub name: StringId,
    pub yaw_per_frame: [f32; 2],
    pub yaw_frames: [u16; 2],
    pub pitch_per_frame: [f32; 2],
    pub pitch_frames: [u16; 2],
}

/// `{name, animation reference}`: the leaves of the mode/class/type tree.
/// Engine: the graph index is a position in the inheritance list (-1 means this
/// graph); the animation index selects an [`Animation`] there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Action {
    pub name: StringId,
    pub graph_index: i16,
    pub animation_index: i16,
}

/// Weapon type, 52 bytes: a name, an action block, three blocks this reader
/// only counts, and two int lists (engine: used to preload resources).
#[derive(Clone, Debug, PartialEq)]
pub struct WeaponType {
    pub name: StringId,
    pub actions: Vec<Action>,
    /// Counts of the blocks at +0xc, +0x14 and +0x1c, whose elements are not modelled.
    pub other_block_counts: [u32; 3],
    /// Resource-index lists at +0x24 and +0x2c (engine: indices into the resource block).
    pub preload_a: Vec<i32>,
    pub preload_b: Vec<i32>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct WeaponClass {
    pub name: StringId,
    pub types: Vec<WeaponType>,
    /// Count of the block at +0xc (IK, elements not modelled).
    pub ik_count: u32,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Mode {
    pub name: StringId,
    pub classes: Vec<WeaponClass>,
    /// Count of the block at +0xc (IK, elements not modelled).
    pub ik_count: u32,
}

/// An entry of the inheritance list (engine: datum at +4 selects a parent graph).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inheritance {
    pub graph: TagReference,
    /// Bytes +8..+0x20, unmodelled.
    pub raw: [u8; 24],
}

/// Raw animation data in a cache file: the `0x14`-byte records the engine's
/// animation cache loads (`+4` size, `+8` raw pointer, `+0x10` load flag).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DataResource {
    /// +0, the owning tag's datum.
    pub owner: TagId,
    pub size: u32,
    /// +8, a raw-resource pointer (selector in the top two bits).
    pub pointer: Option<RawPointer>,
    /// +0xc, a runtime cache handle; `0xffffffff` on disc.
    pub runtime: u32,
    /// +0x10; the engine tests its low byte before loading.
    pub flags: u32,
}

/// Default node pose record, 24 bytes: quantized rotation, translation, scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StaticPose {
    pub rotation: [i16; 4],
    pub translation: [f32; 3],
    pub scale: f32,
}

/// Animation kind (`+0x10`). The engine distinguishes 0 and 1; 2 follows the
/// community naming (S).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationType {
    Base,
    Overlay,
    Replacement,
    Unknown(u8),
}
impl AnimationType {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Base,
            1 => Self::Overlay,
            2 => Self::Replacement,
            n => Self::Unknown(n),
        }
    }
    pub fn as_u8(self) -> u8 {
        match self {
            Self::Base => 0,
            Self::Overlay => 1,
            Self::Replacement => 2,
            Self::Unknown(n) => n,
        }
    }
}

/// Sizes of the pieces of an animation's data blob, from the 16 bytes at
/// animation `+0x3c`. See `docs/animation.md` for the layout they describe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DataSizes {
    /// +0: bytes of the static per-channel flag set (engine adds it, signed
    /// byte, when locating the animated flag set).
    pub static_flags: u8,
    /// +1: bytes of the animated flag set (observed equal to three arrays).
    pub animated_flags: u8,
    /// +2, +3: unexplained; the engine never reads them in the sampling path.
    pub unknown: [u8; 2],
    /// +4, unexplained, u16.
    pub unknown_4: u16,
    /// +6: size of the static stream at blob offset 0.
    pub static_size: u16,
    /// +8, unexplained.
    pub unknown_8: u32,
    /// +0xc: size of the animated stream that follows the static one.
    pub animated_size: u32,
}

/// Frame, sound and effect events attached to an animation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameEvent {
    /// Engine: `1 << event_type` is set in the returned event mask.
    pub event_type: i16,
    pub frame: i16,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoundEvent {
    /// Index into [`AnimationGraph::sound_refs`].
    pub sound: i16,
    pub frame: i16,
    /// +4, passed to the event callback unmodified.
    pub raw: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectEvent {
    /// Index into [`AnimationGraph::effect_refs`].
    pub effect: i16,
    pub frame: i16,
}

/// One entry of the animation block (`0x6c` bytes).
#[derive(Clone, Debug, PartialEq)]
pub struct Animation {
    pub name: StringId,
    /// +4: identical across a graph on disc, apparently the skeleton checksum (S).
    pub node_list_checksum: u32,
    /// +8, +0xc: two checksums (S), not read by the sampling path.
    pub checksums: [u32; 2],
    pub anim_type: AnimationType,
    /// +0x11, passed to the movement decoder (0 none, 1 dx/dy, 2 +yaw, ... S).
    pub frame_info_type: u8,
    /// +0x12, unexplained.
    pub unknown_12: u8,
    /// +0x13.
    pub node_count: u8,
    /// +0x14.
    pub frame_count: i16,
    /// +0x16; bit `0x10` selects "data lives in a cache resource".
    pub flags: u16,
    /// +0x18..+0x20, unexplained.
    pub unknown_18: [u8; 8],
    /// +0x20 (observed 0xffffffff).
    pub unknown_20: u32,
    /// +0x24: index into the graph's resource block (when `flags & 0x10`).
    pub resource_index: u32,
    /// +0x28: byte offset of this animation's blob inside that resource.
    pub data_offset: u32,
    /// +0x2c: index into the static-pose block, -1 for none.
    pub static_pose: i16,
    pub unknown_2e: u16,
    /// +0x30 and +0x32: animation indices the engine pre-loads (S: parent / next).
    pub link_a: i16,
    pub link_b: i16,
    /// +0x34, +0x38: inline data (size, pointer) used when `flags & 0x10 == 0`.
    pub inline_size: u32,
    pub inline_address: u32,
    pub sizes: DataSizes,
    pub frame_events: Vec<FrameEvent>,
    pub sound_events: Vec<SoundEvent>,
    pub effect_events: Vec<EffectEvent>,
    /// Count of the block at +0x64 (unmodelled).
    pub extra_block_count: u32,
}

impl Animation {
    pub const FLAG_IN_RESOURCE: u16 = 0x10;
    pub fn in_resource(&self) -> bool {
        self.flags & Self::FLAG_IN_RESOURCE != 0
    }
}

/// A block present in the header whose elements are not modelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawBlock {
    pub header_offset: u16,
    pub count: u32,
    pub address: u32,
}

/// A decoded `jmad` tag.
#[derive(Clone, Debug)]
pub struct AnimationGraph {
    /// +0: a `jmad` reference, null on most graphs.
    pub parent: TagReference,
    /// +8: observed `0x0006350x`, same on nearly every tag (S: a version word).
    pub version_word: u32,
    pub nodes: Vec<Node>,
    pub sound_refs: Vec<Reference>,
    pub effect_refs: Vec<Reference>,
    pub blend_screens: Vec<BlendScreen>,
    pub animations: Vec<Animation>,
    pub modes: Vec<Mode>,
    pub inheritance: Vec<Inheritance>,
    /// +0x54 block: pairs of string IDs, stride 8 (semantics unknown).
    pub name_pairs: Vec<[StringId; 2]>,
    pub resources: Vec<DataResource>,
    pub static_poses: Vec<StaticPose>,
    /// Blocks at +0x3c and +0x44, present on a minority of tags, stride unknown.
    pub unmodelled_blocks: Vec<RawBlock>,
    /// Non-block words at +0x5c..+0x88, kept verbatim as `(offset, value)`.
    pub tail_words: Vec<(u16, u32)>,
}

impl AnimationGraph {
    /// Decode the payload of a `jmad` tag loaded at virtual address `base`.
    /// Every block is bounds-checked against `bytes`, and the blocks with a
    /// modelled stride must not overlap each other (they are separate
    /// allocations in the cache).
    pub fn decode(bytes: &[u8], base: u32) -> Result<Self> {
        if bytes.len() < HEADER_SIZE {
            return Err(Error::RecordSize {
                actual: bytes.len(),
                expected: HEADER_SIZE,
            });
        }
        let parent = TagReference {
            group: FourCC::from_u32(u32_at(bytes, 0)),
            id: TagId(u32_at(bytes, 4)),
        };
        let nodes = block(bytes, base, hdr::NODES, NODE_SIZE)?
            .chunks_exact(NODE_SIZE)
            .map(|e| Node {
                name: StringId(u32_at(e, 0)),
                next_sibling: i16_at(e, 4),
                first_child: i16_at(e, 6),
                parent: i16_at(e, 8),
                model_flags: u16_at(e, 0xa),
                joint_flags: u32_at(e, 0xc),
                base_vector: [f32_at(e, 0x10), f32_at(e, 0x14), f32_at(e, 0x18)],
                vector_range: f32_at(e, 0x1c),
            })
            .collect::<Vec<_>>();
        let references = |off| -> Result<Vec<Reference>> {
            Ok(block(bytes, base, off, REFERENCE_SIZE)?
                .chunks_exact(REFERENCE_SIZE)
                .map(|e| Reference {
                    tag: TagReference {
                        group: FourCC::from_u32(u32_at(e, 0)),
                        id: TagId(u32_at(e, 4)),
                    },
                    flags: u16_at(e, 8),
                })
                .collect())
        };
        let sound_refs = references(hdr::SOUND_REFS)?;
        let effect_refs = references(hdr::EFFECT_REFS)?;
        let blend_screens = block(bytes, base, hdr::BLEND_SCREENS, BLEND_SCREEN_SIZE)?
            .chunks_exact(BLEND_SCREEN_SIZE)
            .map(|e| BlendScreen {
                name: StringId(u32_at(e, 0)),
                yaw_per_frame: [f32_at(e, 4), f32_at(e, 8)],
                yaw_frames: [u16_at(e, 0xc), u16_at(e, 0xe)],
                pitch_per_frame: [f32_at(e, 0x10), f32_at(e, 0x14)],
                pitch_frames: [u16_at(e, 0x18), u16_at(e, 0x1a)],
            })
            .collect();
        let animations = block(bytes, base, hdr::ANIMATIONS, ANIMATION_SIZE)?
            .chunks_exact(ANIMATION_SIZE)
            .map(|e| decode_animation(e, bytes, base))
            .collect::<Result<Vec<_>>>()?;
        let modes = block(bytes, base, hdr::MODES, MODE_SIZE)?
            .chunks_exact(MODE_SIZE)
            .map(|e| decode_mode(e, bytes, base))
            .collect::<Result<Vec<_>>>()?;
        let inheritance = block(bytes, base, hdr::INHERITANCE, INHERITANCE_SIZE)?
            .chunks_exact(INHERITANCE_SIZE)
            .map(|e| Inheritance {
                graph: TagReference {
                    group: FourCC::from_u32(u32_at(e, 0)),
                    id: TagId(u32_at(e, 4)),
                },
                raw: e[8..].try_into().expect("slice length"),
            })
            .collect();
        let name_pairs = block(bytes, base, hdr::NAME_PAIRS, 8)?
            .chunks_exact(8)
            .map(|e| [StringId(u32_at(e, 0)), StringId(u32_at(e, 4))])
            .collect();
        let resources = block(bytes, base, hdr::RESOURCES, RESOURCE_SIZE)?
            .chunks_exact(RESOURCE_SIZE)
            .map(|e| DataResource {
                owner: TagId(u32_at(e, 0)),
                size: u32_at(e, 4),
                pointer: RawPointer::decode(u32_at(e, 8)),
                runtime: u32_at(e, 0xc),
                flags: u32_at(e, 0x10),
            })
            .collect();
        let static_poses = block(bytes, base, hdr::STATIC_POSES, STATIC_POSE_SIZE)?
            .chunks_exact(STATIC_POSE_SIZE)
            .map(|e| StaticPose {
                rotation: [i16_at(e, 0), i16_at(e, 2), i16_at(e, 4), i16_at(e, 6)],
                translation: [f32_at(e, 8), f32_at(e, 12), f32_at(e, 16)],
                scale: f32_at(e, 20),
            })
            .collect();
        let mut unmodelled_blocks = Vec::new();
        for off in [hdr::UNMODELLED_A, hdr::UNMODELLED_B] {
            let count = u32_at(bytes, off);
            if count != 0 {
                let address = u32_at(bytes, off + 4);
                // Without a stride only the start can be range-checked.
                if address
                    .checked_sub(base)
                    .is_none_or(|s| s as usize >= bytes.len())
                {
                    return Err(Error::Bounds {
                        address,
                        size: u64::from(count),
                    });
                }
                unmodelled_blocks.push(RawBlock {
                    header_offset: off as u16,
                    count,
                    address,
                });
            }
        }
        let tail_words = (0x5c..0x88)
            .step_by(4)
            .map(|o| (o as u16, u32_at(bytes, o)))
            .filter(|&(_, w)| w != 0)
            .collect();
        let g = Self {
            parent,
            version_word: u32_at(bytes, 8),
            nodes,
            sound_refs,
            effect_refs,
            blend_screens,
            animations,
            modes,
            inheritance,
            name_pairs,
            resources,
            static_poses,
            unmodelled_blocks,
            tail_words,
        };
        g.check_disjoint(bytes, base)?;
        Ok(g)
    }

    /// The header-level blocks with a modelled stride must be disjoint.
    fn check_disjoint(&self, bytes: &[u8], base: u32) -> Result<()> {
        let mut spans: Vec<(usize, usize)> = [
            (hdr::NODES, NODE_SIZE),
            (hdr::SOUND_REFS, REFERENCE_SIZE),
            (hdr::EFFECT_REFS, REFERENCE_SIZE),
            (hdr::BLEND_SCREENS, BLEND_SCREEN_SIZE),
            (hdr::ANIMATIONS, ANIMATION_SIZE),
            (hdr::MODES, MODE_SIZE),
            (hdr::INHERITANCE, INHERITANCE_SIZE),
            (hdr::NAME_PAIRS, 8),
            (hdr::RESOURCES, RESOURCE_SIZE),
            (hdr::STATIC_POSES, STATIC_POSE_SIZE),
        ]
        .into_iter()
        .filter_map(|(o, s)| span(bytes, base, o, s))
        .collect();
        spans.sort_unstable();
        if spans.windows(2).any(|w| w[0].1 > w[1].0) {
            return Err(Error::Invalid("animation graph blocks overlap"));
        }
        Ok(())
    }

    /// Read and decode a `jmad` tag of a map.
    pub fn read<R: Read + Seek>(map: &mut Map<R>, id: TagId) -> Result<Self> {
        let tag = map.tag(id)?;
        if tag.group != FourCC(*b"jmad") {
            return Err(Error::UnsupportedGroup(tag.group));
        }
        let address = tag.location.ok_or(map::Error::NoTagData)?.address;
        let bytes = map.read_tag_bytes(id)?;
        Self::decode(&bytes, address)
    }

    /// Read one of the graph's data resources in full. The caller owns file
    /// selection (see [`ResourceReader`]); sizes are bounds-checked first and
    /// capped at [`MAX_RESOURCE_BYTES`]. The result is shared: every clip
    /// built from it with [`Self::clip_from_shared`] borrows it, so many
    /// animations of one resource cost one copy of it in total.
    pub fn read_resource(
        &self,
        index: usize,
        reader: &mut dyn ResourceReader,
    ) -> Result<Arc<[u8]>> {
        let res = self
            .resources
            .get(index)
            .ok_or(Error::Invalid("resource index out of range"))?;
        let pointer = res
            .pointer
            .ok_or(Error::Invalid("null animation resource pointer"))?;
        if u64::from(res.size) > MAX_RESOURCE_BYTES {
            return Err(Error::Invalid("animation resource larger than the limit"));
        }
        reader.check(pointer, res.size)?;
        Ok(Arc::from(reader.read_raw(pointer, res.size)?))
    }

    fn clip_animation(&self, index: usize) -> Result<&Animation> {
        let anim = self
            .animations
            .get(index)
            .ok_or(Error::Invalid("animation index out of range"))?;
        if !anim.in_resource() {
            return Err(Error::Invalid(
                "animation data is not in a resource (inline data is unsupported)",
            ));
        }
        if usize::from(anim.node_count) != self.nodes.len() {
            return Err(Error::Invalid(
                "animation node count differs from the graph's node count",
            ));
        }
        Ok(anim)
    }

    /// Build the clip of animation `index`, sharing `resource` (as returned by
    /// [`Self::read_resource`]) without copying it.
    pub fn clip_from_shared(&self, index: usize, resource: &Arc<[u8]>) -> Result<AnimationClip> {
        let anim = self.clip_animation(index)?;
        if anim.data_offset as usize > resource.len() {
            return Err(Error::Invalid("animation data offset beyond its resource"));
        }
        AnimationClip::from_shared(anim, Arc::clone(resource), anim.data_offset as usize)
    }

    /// Build the clip of animation `index` from the bytes of its resource,
    /// copying only the animation's declared span.
    pub fn clip_from_resource(&self, index: usize, resource: &[u8]) -> Result<AnimationClip> {
        let anim = self.clip_animation(index)?;
        let blob = resource
            .get(anim.data_offset as usize..)
            .ok_or(Error::Invalid("animation data offset beyond its resource"))?;
        let len = AnimationClip::declared_len(anim)?.min(blob.len());
        AnimationClip::new(anim, blob[..len].to_vec())
    }

    /// Read the animation's resource and build its clip.
    pub fn load_clip(
        &self,
        index: usize,
        reader: &mut dyn ResourceReader,
    ) -> Result<AnimationClip> {
        let anim = self
            .animations
            .get(index)
            .ok_or(Error::Invalid("animation index out of range"))?;
        let data = self.read_resource(anim.resource_index as usize, reader)?;
        self.clip_from_shared(index, &data)
    }

    /// Find an animation by its string-ID name. `strings` is the map's string table.
    pub fn find_animation(&self, strings: &[Option<String>], name: &str) -> Option<usize> {
        self.animations
            .iter()
            .position(|a| a.name.resolve(strings).ok().flatten() == Some(name))
    }
}

fn decode_animation(e: &[u8], tag: &[u8], base: u32) -> Result<Animation> {
    let events = |off: usize, stride: usize| sub_block(tag, base, e, off, stride);
    Ok(Animation {
        name: StringId(u32_at(e, 0)),
        node_list_checksum: u32_at(e, 4),
        checksums: [u32_at(e, 8), u32_at(e, 0xc)],
        anim_type: AnimationType::from_u8(e[0x10]),
        frame_info_type: e[0x11],
        unknown_12: e[0x12],
        node_count: e[0x13],
        frame_count: i16_at(e, 0x14),
        flags: u16_at(e, 0x16),
        unknown_18: e[0x18..0x20].try_into().expect("slice length"),
        unknown_20: u32_at(e, 0x20),
        resource_index: u32_at(e, 0x24),
        data_offset: u32_at(e, 0x28),
        static_pose: i16_at(e, 0x2c),
        unknown_2e: u16_at(e, 0x2e),
        link_a: i16_at(e, 0x30),
        link_b: i16_at(e, 0x32),
        inline_size: u32_at(e, 0x34),
        inline_address: u32_at(e, 0x38),
        sizes: DataSizes {
            static_flags: e[0x3c],
            animated_flags: e[0x3d],
            unknown: [e[0x3e], e[0x3f]],
            unknown_4: u16_at(e, 0x40),
            static_size: u16_at(e, 0x42),
            unknown_8: u32_at(e, 0x44),
            animated_size: u32_at(e, 0x48),
        },
        frame_events: events(0x4c, 4)?
            .chunks_exact(4)
            .map(|x| FrameEvent {
                event_type: i16_at(x, 0),
                frame: i16_at(x, 2),
            })
            .collect(),
        sound_events: events(0x54, 8)?
            .chunks_exact(8)
            .map(|x| SoundEvent {
                sound: i16_at(x, 0),
                frame: i16_at(x, 2),
                raw: u32_at(x, 4),
            })
            .collect(),
        effect_events: events(0x5c, 4)?
            .chunks_exact(4)
            .map(|x| EffectEvent {
                effect: i16_at(x, 0),
                frame: i16_at(x, 2),
            })
            .collect(),
        extra_block_count: u32_at(e, 0x64),
    })
}

fn decode_mode(e: &[u8], tag: &[u8], base: u32) -> Result<Mode> {
    let classes = sub_block(tag, base, e, 4, CLASS_SIZE)?
        .chunks_exact(CLASS_SIZE)
        .map(|c| {
            let types = sub_block(tag, base, c, 4, TYPE_SIZE)?
                .chunks_exact(TYPE_SIZE)
                .map(|t| decode_type(t, tag, base))
                .collect::<Result<Vec<_>>>()?;
            Ok(WeaponClass {
                name: StringId(u32_at(c, 0)),
                types,
                ik_count: u32_at(c, 0xc),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Mode {
        name: StringId(u32_at(e, 0)),
        classes,
        ik_count: u32_at(e, 0xc),
    })
}

fn decode_type(t: &[u8], tag: &[u8], base: u32) -> Result<WeaponType> {
    let actions = sub_block(tag, base, t, 4, ACTION_SIZE)?
        .chunks_exact(ACTION_SIZE)
        .map(|a| Action {
            name: StringId(u32_at(a, 0)),
            graph_index: i16_at(a, 4),
            animation_index: i16_at(a, 6),
        })
        .collect();
    let ints = |off| -> Result<Vec<i32>> {
        Ok(sub_block(tag, base, t, off, 4)?
            .chunks_exact(4)
            .map(|x| u32_at(x, 0) as i32)
            .collect())
    };
    Ok(WeaponType {
        name: StringId(u32_at(t, 0)),
        actions,
        other_block_counts: [u32_at(t, 0xc), u32_at(t, 0x14), u32_at(t, 0x1c)],
        preload_a: ints(0x24)?,
        preload_b: ints(0x2c)?,
    })
}
