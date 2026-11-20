//! A light layer's sorted entries into the scene
//! (docs/graphics/framescene.md, slice 4, "The light passes and the
//! shadow layers"): what the guest's first geometry handler `0x0004d0b0`
//! supplies for a gathered group, read at the layer build instead, so
//! the native light and shadow stages can draw the layer from the scene
//! alone ([`frame_scene::LightGroup`]).
//!
//! Research points (**Code**, the same reads as
//! `render_bridge::native_lights::walk`, whose build gives the sorted
//! entries this reads; `h2 disasm`):
//!
//! - A sorted entry (`0x18` bytes, after `0x0004cbf0` and the sort): `+0`
//!   the item's index in the list, `+4` the implementation, `+8` the
//!   vertex classification.
//! - The item (`+0x14` of the list, `0x18` each): `+0`/`+4` the build and
//!   draw ([`GEOMETRY_BUILD`], [`GEOMETRY_DRAW`]), `+8` the shader, `+0xc`
//!   the argument (render record `<< 8`, the part in its low byte),
//!   `+0x14` the pass selector byte and `+0x15` the variant.
//! - The render record: `+0` flags (bit 6 visible subparts, bit 9 strip
//!   topology, bit 11 the auxiliary state toggle, bit 12 a placement),
//!   `+4` the model (`-1` the structure's), `+0xc` the key (section bits
//!   9..17, placement bits 18..28), `+0x14` the visible-subpart mask,
//!   byte `+0x1a` (`0xff` unless the handler sets state around the part).
//! - The section's data: a model's (`mode` `+0x28`, `0x5c` each, data at
//!   `+0x34`); a cluster's (the structure BSP [`ACTIVE_BSP`] `+0xa0`,
//!   `0xb0` each, data at `+0x50`); a placement's definition's (BSP
//!   `+0x144` placements, `0x58` each, the definition at `+0x34`; the
//!   definitions at `+0x13c`, `0xc8` each, data at `+0x50`). The data's
//!   `+4` parts (`0x48` each: `+2` flags, bit 3 a triangle list; `+6` the
//!   first index; `+8` the index count), `+8` subparts, `+0x20` the
//!   indices.
//! - What the handler does beyond binding the material and drawing the
//!   part, which the scene does not carry, and so is the entry's
//!   remainder ([`Cause`]): the auxiliary state toggle, state around one
//!   part (`+0x1a`), visible subparts, the shader's `+0x16` bit 0, a
//!   material animation (the shader's `+0x24` block `+0x3c`), and a
//!   template callback (the shader's `stem` `+0x44`, `+0x50`).
//! - The shadow projection block [`SHADOW_PROJECTION`] (`0x00048ee0`'s
//!   36 words for `c[40..48]`) is written before layer 10 begins; layers
//!   10 and 11 read it. **S**: the additional casters' pair `0x00030ed0`
//!   is not traced; its layers read the block as it is at their build.

use frame_scene::{
    BuildError, GroupSite, InstanceKey, LayerOwner, LightBinding, LightEntry, LightGroup, LightPose, LightStream,
    PROJECTION_ROWS, Row, SceneBuilder, Topology,
};

use crate::GuestRead;
use crate::layout::object_nodes::TAG_INSTANCES;
use crate::layout::objects::{RecordOwner, record_owner, record_rows};
use crate::layout::skip::{GEOMETRY_BUILD, GEOMETRY_DRAW, ITEM_SIZE, RENDER_RECORD_SIZE, RENDER_RECORDS};

/// The structure BSP's pointer.
pub const ACTIVE_BSP: u32 = 0x004e_0348;
/// The shadow projection block, 36 words.
pub const SHADOW_PROJECTION: u32 = 0x004c_9798;

/// One sorted entry as the layer build leaves it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sorted {
    /// The item's index in the list.
    pub index: u16,
    pub implementation: u8,
    pub vertex_class: u16,
}

/// Why a sorted entry is not in the scene: the guest's handler still
/// draws it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// Memory that does not read, or an index outside the list.
    Memory,
    /// Another build or draw than the standard geometry pair.
    Callback,
    /// A record that is not one, or whose owner does not resolve.
    Owner,
    /// The section or part does not read, or a tag is not its group.
    Section,
    /// The part's index range is empty or past its section's indices.
    Range,
    /// The auxiliary state toggle, or state around the one part.
    State,
    /// The record draws chosen subparts.
    Subparts,
    /// A shader flag, a material animation or a template callback.
    Material,
    /// The object's node rows do not read.
    Pose,
}

impl Cause {
    pub const ALL: [Cause; 9] = [
        Cause::Memory,
        Cause::Callback,
        Cause::Owner,
        Cause::Section,
        Cause::Range,
        Cause::State,
        Cause::Subparts,
        Cause::Material,
        Cause::Pose,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Cause::Memory => "memory",
            Cause::Callback => "callback",
            Cause::Owner => "owner",
            Cause::Section => "section",
            Cause::Range => "range",
            Cause::State => "state",
            Cause::Subparts => "subparts",
            Cause::Material => "material",
            Cause::Pose => "pose",
        }
    }
}

/// What [`push`] put in the scene.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Extracted {
    pub entries: u32,
    /// Entries left to the guest, by [`Cause::ALL`]'s order.
    pub remainder: [u32; Cause::ALL.len()],
}

impl Extracted {
    pub fn remainder(&self) -> u32 {
        self.remainder.iter().sum()
    }
}

type R<T> = Result<T, Cause>;

fn word(mem: &dyn GuestRead, at: u32) -> R<u32> {
    mem.read_u32(at).ok_or(Cause::Memory)
}

fn byte(mem: &dyn GuestRead, at: u32) -> R<u8> {
    Ok((word(mem, at & !3)? >> (8 * (at & 3))) as u8)
}

fn half(mem: &dyn GuestRead, at: u32) -> R<u16> {
    Ok(u16::from(byte(mem, at)?) | u16::from(byte(mem, at.checked_add(1).ok_or(Cause::Memory)?)?) << 8)
}

fn at(base: u32, offset: u32) -> R<u32> {
    base.checked_add(offset).ok_or(Cause::Memory)
}

fn index(base: u32, n: u32, stride: u32) -> R<u32> {
    n.checked_mul(stride).and_then(|o| base.checked_add(o)).ok_or(Cause::Memory)
}

fn ptr(mem: &dyn GuestRead, base: u32, offset: u32) -> R<u32> {
    match word(mem, at(base, offset)?)? {
        0 => Err(Cause::Section),
        p => Ok(p),
    }
}

/// The tag data of `datum`, checked to be of `group`.
fn tag(mem: &dyn GuestRead, datum: u32, group: &[u8; 4]) -> R<u32> {
    let table = ptr(mem, TAG_INSTANCES, 0)?;
    let slot = index(table, datum & 0xffff, 16)?;
    if datum as u16 > i16::MAX as u16
        || word(mem, slot)? != u32::from_be_bytes(*group)
        || word(mem, at(slot, 4)?)? != datum
    {
        return Err(Cause::Section);
    }
    ptr(mem, slot, 8)
}

/// What the handler would do beyond binding and drawing: refused by
/// name ([`Cause::Material`]).
fn material(mem: &dyn GuestRead, shader: u32) -> R<()> {
    let shad = tag(mem, shader, b"shad").map_err(|_| Cause::Material)?;
    if byte(mem, at(shad, 0x16)?)? & 1 != 0 {
        return Err(Cause::Material);
    }
    if word(mem, at(ptr(mem, shad, 0x24)?, 0x3c)?)? != 0 {
        return Err(Cause::Material);
    }
    let stem = tag(mem, word(mem, at(shad, 4)?)?, b"stem").map_err(|_| Cause::Material)?;
    if [0x44, 0x50].into_iter().any(|o| mem.read_u32(stem.wrapping_add(o)) != Some(u32::MAX)) {
        return Err(Cause::Material);
    }
    Ok(())
}

/// One sorted entry of the list whose items are at `items`, with the
/// node rows of an object's pose.
pub fn entry(mem: &dyn GuestRead, items: u32, sorted: Sorted) -> R<(LightEntry, Vec<Row>)> {
    let item = index(items, u32::from(sorted.index), ITEM_SIZE)?;
    if word(mem, item)? != GEOMETRY_BUILD || word(mem, at(item, 4)?)? != GEOMETRY_DRAW {
        return Err(Cause::Callback);
    }
    let shader = word(mem, at(item, 8)?)?;
    let argument = word(mem, at(item, 0xc)?)?;
    let selector = byte(mem, at(item, 0x14)?)?;
    let variant = byte(mem, at(item, 0x15)?)?;
    let record = ((argument as i32) >> 8) as u16;
    if (record as i16) < 0 {
        return Err(Cause::Owner);
    }
    let rec = index(RENDER_RECORDS, u32::from(record), RENDER_RECORD_SIZE)?;
    let flags = word(mem, rec)?;
    let model = word(mem, at(rec, 4)?)?;
    let key = word(mem, at(rec, 0xc)?)?;
    if flags & 1 << 11 != 0 || byte(mem, at(rec, 0x1a)?)? != 0xff {
        return Err(Cause::State);
    }
    let section = (key >> 9) & 0x1ff;
    let (owner, stream_section, data, pose) = if model == u32::MAX {
        let bsp = ptr(mem, ACTIVE_BSP, 0)?;
        if flags & 1 << 12 == 0 {
            let info = index(ptr(mem, bsp, 0xa0)?, section, 0xb0)?;
            (LayerOwner::Cluster(section as u16), section as u16, ptr(mem, info, 0x50)?, LightPose::World)
        } else {
            let placement = (key >> 18) & 0x7ff;
            let at_placement = index(ptr(mem, bsp, 0x144)?, placement, 0x58)?;
            let definition = half(mem, at(at_placement, 0x34)?)?;
            if definition > i16::MAX as u16 {
                return Err(Cause::Section);
            }
            let info = index(ptr(mem, bsp, 0x13c)?, u32::from(definition), 0xc8)?;
            let p = placement as u16;
            (LayerOwner::Placement(p), definition, ptr(mem, info, 0x50)?, LightPose::Placement(p))
        }
    } else {
        let handle = match record_owner(mem, record).ok_or(Cause::Memory)? {
            RecordOwner::Object(h) => h,
            _ => return Err(Cause::Owner),
        };
        let mode = tag(mem, model, b"mode")?;
        let base = index(ptr(mem, mode, 0x28)?, section, 0x5c)?;
        (LayerOwner::Object(InstanceKey(handle)), section as u16, ptr(mem, base, 0x34)?, LightPose::Rows(0..0))
    };
    if flags & 1 << 6 != 0 && word(mem, at(rec, 0x14)?)? != 0 && word(mem, at(data, 8)?)? != 0 {
        return Err(Cause::Subparts);
    }
    let part = index(ptr(mem, data, 4)?, argument & 0xff, 0x48)?;
    let part_flags = half(mem, at(part, 2)?)?;
    let first_index = u32::from(half(mem, at(part, 6)?)?);
    let index_count = u32::from(half(mem, at(part, 8)?)?);
    if index_count < 3 || first_index + index_count > word(mem, at(data, 0x20)?)? {
        return Err(Cause::Range);
    }
    material(mem, shader)?;
    let rows = match pose {
        LightPose::Rows(_) => record_rows(mem, record).map_err(|_| Cause::Pose)?,
        _ => Vec::new(),
    };
    let topology = if flags & 1 << 9 != 0 && part_flags & 8 == 0 { Topology::Strip } else { Topology::List };
    let entry = LightEntry {
        slot: sorted.index,
        record,
        owner,
        model,
        stream: LightStream {
            section: stream_section,
            part: argument as u8,
            vertex_class: sorted.vertex_class,
            first_index,
            index_count,
            topology,
        },
        pose,
        binding: LightBinding { shader, selector, variant, implementation: sorted.implementation },
    };
    Ok((entry, rows))
}

/// The shadow projection block as nine registers; `None` when a word
/// does not read.
pub fn projection(mem: &dyn GuestRead) -> Option<[[u32; 4]; PROJECTION_ROWS]> {
    let mut out = [[0; 4]; PROJECTION_ROWS];
    for (r, row) in out.iter_mut().enumerate() {
        for (c, w) in row.iter_mut().enumerate() {
            *w = mem.read_u32(SHADOW_PROJECTION + 4 * (4 * r + c) as u32)?;
        }
    }
    Some(out)
}

/// The sorted entries of `layer` on `list` into a group of the open
/// view's last light (its last object shadow for
/// [`GroupSite::ObjectShadow`]): each that reads with its streams, pose and
/// bindings, the rest the group's remainder by cause. Layers 10 and 11
/// take the shadow projection.
pub fn push(
    mem: &dyn GuestRead,
    list: u32,
    layer: u8,
    site: GroupSite,
    sorted: &[Sorted],
    builder: &mut SceneBuilder,
) -> Result<Extracted, BuildError> {
    let mut out = Extracted::default();
    let mut entries = Vec::with_capacity(sorted.len());
    let items = mem.read_u32(list.wrapping_add(0x14)).filter(|&p| p != 0);
    for &s in sorted {
        match items.ok_or(Cause::Memory).and_then(|items| entry(mem, items, s)) {
            Ok(e) => entries.push(e),
            Err(cause) => {
                let n = Cause::ALL.iter().position(|&c| c == cause).expect("a cause of ALL");
                out.remainder[n] += 1;
            }
        }
    }
    out.entries = entries.len() as u32;
    let projection = matches!(layer, 10 | 11).then(|| projection(mem)).flatten();
    let group = LightGroup { light: 0, layer, site, projection, entries: 0..0, remainder: out.remainder() };
    match site {
        GroupSite::ObjectShadow => builder.push_shadow_group(group, &entries)?,
        _ => builder.push_light_group(group, &entries)?,
    };
    Ok(out)
}

#[cfg(test)]
mod tests {
    use frame_scene::{LightStep, LightTarget, SceneId, SceneLight, Stamp, ViewKey, ViewKind};

    use super::*;
    use crate::layout::object_nodes::RECORD_MATRIX;
    use crate::testing::{Mem, camera_record, lighting, object, record};

    const TABLE: u32 = 0x0060_0000;
    const LIST: u32 = 0x004c_1a60;
    const ITEMS: u32 = 0x0061_0000;
    const BSP: u32 = 0x0062_0000;
    const CLUSTERS: u32 = 0x0062_1000;
    const PLACEMENTS: u32 = 0x0062_2000;
    const DEFINITIONS: u32 = 0x0062_3000;
    const DATA: u32 = 0x0063_0000;
    const PARTS: u32 = 0x0063_1000;
    const MODE: u32 = 0x0064_0000;
    const SECTIONS: u32 = 0x0064_1000;
    const SHAD: u32 = 0x0065_0000;
    const POST: u32 = 0x0065_1000;
    const STEM: u32 = 0x0065_2000;
    const MATRIX: u32 = 0x0066_0000;
    const SHADER: u32 = 1;
    const TEMPLATE: u32 = 2;
    const MODEL: u32 = 3;

    fn tag_entry(m: &mut Mem, datum: u32, group: &[u8; 4], data: u32) {
        let slot = TABLE + 16 * (datum & 0xffff);
        m.w(slot, u32::from_be_bytes(*group));
        m.w(slot + 4, datum);
        m.w(slot + 8, data);
    }

    fn set_half(m: &mut Mem, at: u32, v: u16) {
        let w = m.read_u32(at & !3).unwrap_or(0);
        let shift = 8 * (at & 2);
        m.w(at & !3, (w & !(0xffff << shift)) | u32::from(v) << shift);
    }

    /// A list of four items: item `n` draws part `n` of the section its
    /// record names, record `n + 1`, six indices from `8n`.
    fn fixture() -> Mem {
        let mut m = Mem::default();
        m.w(TAG_INSTANCES, TABLE);
        tag_entry(&mut m, SHADER, b"shad", SHAD);
        tag_entry(&mut m, TEMPLATE, b"stem", STEM);
        tag_entry(&mut m, MODEL, b"mode", MODE);
        m.w(SHAD + 4, TEMPLATE);
        m.w(SHAD + 0x14, 0);
        m.w(SHAD + 0x24, POST);
        m.w(POST + 0x3c, 0);
        m.w(STEM + 0x44, u32::MAX);
        m.w(STEM + 0x50, u32::MAX);
        m.w(ACTIVE_BSP, BSP);
        m.w(BSP + 0xa0, CLUSTERS);
        m.w(BSP + 0x144, PLACEMENTS);
        m.w(BSP + 0x13c, DEFINITIONS);
        // Cluster 2, placement 3 of definition 1 and model section 0 all
        // share one section data.
        m.w(CLUSTERS + 2 * 0xb0 + 0x50, DATA);
        m.w(DEFINITIONS + 0xc8 + 0x50, DATA);
        set_half(&mut m, PLACEMENTS + 3 * 0x58 + 0x34, 1);
        m.w(MODE + 0x28, SECTIONS);
        m.w(SECTIONS + 0x34, DATA);
        m.w(DATA + 4, PARTS);
        m.w(DATA + 8, 0);
        m.w(DATA + 0x20, 64);
        for n in 0..4 {
            set_half(&mut m, PARTS + 0x48 * n + 2, 8);
            set_half(&mut m, PARTS + 0x48 * n + 6, (8 * n) as u16);
            set_half(&mut m, PARTS + 0x48 * n + 8, 6);
            let item = ITEMS + ITEM_SIZE * n;
            m.w(item, GEOMETRY_BUILD);
            m.w(item + 4, GEOMETRY_DRAW);
            m.w(item + 8, SHADER);
            m.w(item + 0xc, (n + 1) << 8 | n);
            m.w(item + 0x14, 0x0201);
            let rec = RENDER_RECORDS + RENDER_RECORD_SIZE * (n + 1);
            m.w(rec, 0);
            m.w(rec + 4, u32::MAX);
            m.w(rec + 0xc, 2 << 9);
            m.w(rec + 0x18, 0x00ff_0000);
        }
        m.w(LIST + 0x14, ITEMS);
        m
    }

    fn sorted(index: u16) -> Sorted {
        Sorted { index, implementation: 2, vertex_class: 1 }
    }

    /// Item 1's record a placement, item 2's an object's rigid part.
    fn placement_and_object(m: &mut Mem) -> u32 {
        let rec = |n: u32| RENDER_RECORDS + RENDER_RECORD_SIZE * n;
        m.w(rec(2), 1 << 12);
        m.w(rec(2) + 0xc, 3 << 18);
        let handle = object(m, 5, 0xe500_0000, 1);
        record(m, 3, Some(lighting(5 + 7)));
        m.w(rec(3) + 4, MODEL);
        m.w(rec(3) + 0x18, 0x00ff_0000);
        m.w(rec(3) + RECORD_MATRIX, MATRIX);
        for k in 0..12 {
            m.f(MATRIX + 4 + 4 * k, k as f32);
        }
        handle
    }

    fn scene_with_light(b: &mut SceneBuilder) {
        let mut cam = Mem::default();
        camera_record(&mut cam, 0x1000);
        b.begin(SceneId::default());
        b.open_view(ViewKey { kind: ViewKind::Game, player: 0, view: 0 }, crate::layout::camera::read(&cam, 0x1000).unwrap())
            .unwrap();
        let light = SceneLight { handle: 1, definition: 2, flags: 4, definition_flags: 0, additional_casters: true, steps: 0..0, ..Default::default() };
        let step = LightStep { layer: 10, stencil: None, target: LightTarget::ShadowMap, draws: true };
        b.push_light(light, &[step]).unwrap();
    }

    #[test]
    fn a_gathered_cluster_placement_and_object_carry_their_streams_poses_and_bindings() {
        let mut m = fixture();
        let handle = placement_and_object(&mut m);
        let (cluster, rows) = entry(&m, ITEMS, sorted(0)).unwrap();
        assert!(rows.is_empty());
        assert_eq!((cluster.owner, cluster.pose.clone(), cluster.record), (LayerOwner::Cluster(2), LightPose::World, 1));
        assert_eq!(cluster.stream, LightStream {
            section: 2, part: 0, vertex_class: 1, first_index: 0, index_count: 6, topology: Topology::List,
        });
        assert_eq!(cluster.binding, LightBinding { shader: SHADER, selector: 1, variant: 2, implementation: 2 });
        let (placement, _) = entry(&m, ITEMS, sorted(1)).unwrap();
        assert_eq!((placement.owner, placement.pose), (LayerOwner::Placement(3), LightPose::Placement(3)));
        assert_eq!((placement.stream.section, placement.stream.first_index), (1, 8), "the placement's definition");
        let (object, rows) = entry(&m, ITEMS, sorted(2)).unwrap();
        assert_eq!((object.owner, object.model), (LayerOwner::Object(InstanceKey(handle)), MODEL));
        assert_eq!(object.stream.section, 0);
        assert_eq!(rows.len(), 3, "a rigid record's one matrix");
        assert_eq!(rows[0], [0.0, 3.0, 6.0, 9.0]);
    }

    #[test]
    fn strip_topology_needs_record_bit_9_on_a_part_without_the_list_flag() {
        let mut m = fixture();
        m.w(RENDER_RECORDS + RENDER_RECORD_SIZE, 1 << 9);
        assert_eq!(entry(&m, ITEMS, sorted(0)).unwrap().0.stream.topology, Topology::List);
        set_half(&mut m, PARTS + 2, 0);
        assert_eq!(entry(&m, ITEMS, sorted(0)).unwrap().0.stream.topology, Topology::Strip);
    }

    #[test]
    fn what_the_handler_does_beyond_binding_and_drawing_is_a_named_remainder() {
        let rec = RENDER_RECORDS + RENDER_RECORD_SIZE;
        let cases: [(&dyn Fn(&mut Mem), Cause); 9] = [
            (&|m| m.w(ITEMS, 0x8484), Cause::Callback),
            (&|m| m.w(rec, 1 << 11), Cause::State),
            (&|m| m.w(rec + 0x18, 0x0001_0000), Cause::State),
            (&|m| {
                m.w(rec, 1 << 6);
                m.w(rec + 0x14, 1);
                m.w(DATA + 8, 2);
            }, Cause::Subparts),
            (&|m| set_half(m, PARTS + 8, 2), Cause::Range),
            (&|m| m.w(DATA + 0x20, 4), Cause::Range),
            (&|m| m.w(POST + 0x3c, 1), Cause::Material),
            (&|m| m.w(STEM + 0x50, 7), Cause::Material),
            (&|m| {
                m.w(rec + 4, MODEL);
                m.w(rec + 0x10, 0);
            }, Cause::Owner),
        ];
        for (n, (change, cause)) in cases.into_iter().enumerate() {
            let mut m = fixture();
            assert!(entry(&m, ITEMS, sorted(0)).is_ok());
            change(&mut m);
            assert_eq!(entry(&m, ITEMS, sorted(0)).unwrap_err(), cause, "case {n}");
        }
        let mut m = fixture();
        m.w(SHAD + 0x14, 1 << 16);
        assert_eq!(entry(&m, ITEMS, sorted(0)).unwrap_err(), Cause::Material, "shader +0x16 bit 0");
    }

    #[test]
    fn a_group_keeps_the_sorted_order_and_duplicates_and_counts_the_rest_by_cause() {
        let mut m = fixture();
        placement_and_object(&mut m);
        for w in 0..36 {
            m.w(SHADOW_PROJECTION + 4 * w, 0x100 + w);
        }
        m.w(ITEMS + 3 * ITEM_SIZE, 0x8484);
        let mut b = SceneBuilder::default();
        scene_with_light(&mut b);
        let order = [sorted(2), sorted(0), sorted(0), sorted(3), sorted(1)];
        let got = push(&m, LIST, 10, GroupSite::Light, &order, &mut b).unwrap();
        assert_eq!((got.entries, got.remainder()), (4, 1));
        assert_eq!(got.remainder[1], 1, "the callback");
        let got = push(&m, LIST, 12, GroupSite::Light, &[sorted(0)], &mut b).unwrap();
        assert_eq!(got.entries, 1);
        let s = b.seal(Stamp::default()).unwrap();
        s.check().unwrap();
        let groups = s.groups_of(0);
        assert_eq!(groups.len(), 2);
        let projection = groups[0].projection.unwrap();
        assert_eq!((projection[0][0], projection[8][3]), (0x100, 0x100 + 35));
        assert_eq!(groups[1].projection, None, "layer 12 takes none");
        assert_eq!(groups[0].remainder, 1);
        let slots: Vec<u16> = s.entries_of(&groups[0]).iter().map(|e| e.slot).collect();
        assert_eq!(slots, [2, 0, 0, 1]);
        assert_eq!(s.pose_rows(&s.entries_of(&groups[0])[0]).len(), 3);
    }

    #[test]
    fn an_object_shadows_layers_go_to_its_shadow_not_the_light() {
        let mut m = fixture();
        placement_and_object(&mut m);
        let mut b = SceneBuilder::default();
        scene_with_light(&mut b);
        assert_eq!(push(&m, LIST, 10, GroupSite::ObjectShadow, &[sorted(2)], &mut b), Err(BuildError::NoShadow));
        b.push_object_shadow([7, 8]).unwrap();
        let got = push(&m, LIST, 10, GroupSite::ObjectShadow, &[sorted(2), sorted(0)], &mut b).unwrap();
        assert_eq!(got.entries, 2);
        let s = b.seal(Stamp::default()).unwrap();
        s.check().unwrap();
        assert!(s.groups_of(0).is_empty());
        let shadow = &s.object_shadows_of(0)[0];
        let group = &s.shadow_groups_of(shadow)[0];
        assert_eq!((group.layer, group.site, s.entries_of(group).len()), (10, GroupSite::ObjectShadow, 2));
    }
}
