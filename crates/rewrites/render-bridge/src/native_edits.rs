//! The native renderer's refresh queue for live edits
//! (docs/editor/realtime-edits.md, "The native refresh").
//!
//! The hot-edit glue pushes one [`Edit`] per owner a batch changed, after
//! the batch applies: a raw owner's ([`Edit::Section`], [`Edit::Model`],
//! [`Edit::Image`]) and, from the tag-write notice, each tag whose
//! metadata changed ([`Edit::Tag`]; a shader's, template's, pass's or
//! vertex shader's is queued as [`Edit::Shader`]). Each native glue that
//! holds a copy of tag data (the world, the objects, first person, the
//! lights and the decorators) drains its own share ([`Consumer`]) at a
//! frame boundary,
//! once their map's copy is resident, and rebuild what the edit touched
//! from the map file as the raw overlay serves it. Plain integers keep
//! this crate free of the agent's types.
//!
//! An edit belongs to the map load it was pushed in: a new load
//! (`native_map_work::generation`) drops what is still queued, since the
//! new load reads the overlaid file whole.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};

/// [`Edit::Section`]'s `kind`: a cluster, by index in `sbsp+0x9c`.
pub const KIND_CLUSTER: u8 = 0;
/// [`Edit::Section`]'s `kind`: an instanced-geometry definition, by index
/// in the BSP's definitions; every placement of it is refreshed.
pub const KIND_INSTANCE_DEF: u8 = 1;
/// [`Edit::Section`]'s `kind`: a `mode` section, with `bsp` the render
/// model's tag datum. Queued as [`Edit::Model`] of that tag, since the
/// native objects replace a model whole.
pub const KIND_MODE: u8 = 2;

/// One owner whose bytes a live edit changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Edit {
    /// A render section of structure BSP `bsp` (its tag datum): `kind` one
    /// of [`KIND_CLUSTER`], [`KIND_INSTANCE_DEF`], [`KIND_MODE`]; `grown`
    /// when its vertex or index counts changed, which reloads the world.
    Section { bsp: u32, kind: u8, index: u32, grown: bool },
    /// A render model (`mode`), by tag datum.
    Model { tag: u32 },
    /// Image `image` of bitmap tag `bitmap`, same size and format.
    Image { bitmap: u32, image: u32 },
    /// Tag `tag`'s metadata, of group `group` (big-endian four-character
    /// code, `u32::from_be_bytes(*b"shad")`), written by a field edit or
    /// its revert. The lifted code and the D3D path read the field where
    /// it lies; this tells the native copies.
    Tag { group: u32, tag: u32 },
    /// A shader (`shad`), template (`stem`), pass (`spas`) or vertex
    /// shader (`vrtx`) tag, by datum: the native programs compiled from
    /// it are refreshed ([`SHADER_GROUPS`]).
    Shader { tag: u32 },
}

/// The groups whose [`Edit::Tag`] is queued as [`Edit::Shader`].
pub const SHADER_GROUPS: [u32; 4] = [
    u32::from_be_bytes(*b"shad"),
    u32::from_be_bytes(*b"stem"),
    u32::from_be_bytes(*b"spas"),
    u32::from_be_bytes(*b"vrtx"),
];

impl Edit {
    /// The edit as it is queued: a `mode` section is its model.
    fn normalized(self) -> Self {
        match self {
            Edit::Section { bsp, kind: KIND_MODE, .. } => Edit::Model { tag: bsp },
            Edit::Tag { group, tag } if SHADER_GROUPS.contains(&group) => Edit::Shader { tag },
            other => other,
        }
    }

    /// Who rebuilds from it. An image reaches every glue that holds a
    /// texture made from one (the world's glue covers the lightmap pages
    /// and the fog, water and layer stage images); a model reaches the
    /// objects, first person, the light models and the decorators; a
    /// section that is not a model's reaches the world and the decorators.
    /// A tag notice reaches the world and objects; a shader notice also
    /// reaches first person.
    pub fn consumers(self) -> &'static [Consumer] {
        match self.normalized() {
            Edit::Section { .. } => &[Consumer::World, Consumer::Decorators],
            Edit::Model { .. } => &[Consumer::Objects, Consumer::FirstPerson, Consumer::Lights, Consumer::Decorators],
            Edit::Image { .. } => &[
                Consumer::World,
                Consumer::Objects,
                Consumer::FirstPerson,
                Consumer::Lights,
                Consumer::Decorators,
            ],
            Edit::Tag { .. } => &[Consumer::World, Consumer::Objects],
            Edit::Shader { .. } => &[Consumer::World, Consumer::Objects, Consumer::FirstPerson],
        }
    }

    /// Two edits of the same owner merge; a grown section stays grown.
    fn same_owner(self, other: Self) -> bool {
        match (self, other) {
            (
                Edit::Section { bsp, kind, index, .. },
                Edit::Section { bsp: b, kind: k, index: i, .. },
            ) => (bsp, kind, index) == (b, k, i),
            (a, b) => a == b,
        }
    }
}

/// A drainer of the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Consumer {
    /// `native_world_glue`: sections, and the world's material textures
    /// with the lightmap pages and the fog, water and layer stage images.
    World = 0,
    /// `native_objects_glue`: models and the objects' material textures.
    Objects = 1,
    /// `native_first_person_glue`: the first-person models and their
    /// textures.
    FirstPerson = 2,
    /// `native_lights_glue`: the light models, the light passes' stage
    /// textures and the gels.
    Lights = 3,
    /// `native_decorators_glue`: the decorator materials' geometry and
    /// stage textures.
    Decorators = 4,
}

/// How many [`Consumer`]s there are.
const CONSUMERS: usize = 5;

#[derive(Default)]
struct Queue {
    generation: u64,
    edits: [Vec<Edit>; CONSUMERS],
}

impl Queue {
    const fn new() -> Self {
        Self { generation: 0, edits: [const { Vec::new() }; CONSUMERS] }
    }

    fn renew(&mut self, generation: u64) {
        if self.generation != generation {
            self.generation = generation;
            self.edits.iter_mut().for_each(Vec::clear);
        }
    }

    fn push(&mut self, generation: u64, edit: Edit) {
        self.renew(generation);
        let edit = edit.normalized();
        for &consumer in edit.consumers() {
            let list = &mut self.edits[consumer as usize];
            match list.iter_mut().find(|e| e.same_owner(edit)) {
                Some(Edit::Section { grown, .. }) => {
                    if let Edit::Section { grown: g, .. } = edit {
                        *grown |= g;
                    }
                }
                Some(_) => {}
                None => list.push(edit),
            }
        }
    }

    fn drain(&mut self, generation: u64, consumer: Consumer) -> Vec<Edit> {
        self.renew(generation);
        std::mem::take(&mut self.edits[consumer as usize])
    }

    fn mask(&self) -> u8 {
        self.edits.iter().enumerate().fold(0, |m, (n, l)| if l.is_empty() { m } else { m | 1 << n })
    }
}

static QUEUE: Mutex<Queue> = Mutex::new(Queue::new());
/// Bit `consumer` while its list is not empty: the per-frame check reads
/// only this.
static PENDING: AtomicU8 = AtomicU8::new(0);

#[cfg(feature = "native-render")]
fn generation() -> u64 {
    crate::native_map_work::generation()
}

#[cfg(not(feature = "native-render"))]
fn generation() -> u64 {
    0
}

/// Queues `edit` for its consumers. Called by the hot-edit glue after a
/// batch applies, on the game thread.
pub fn push(edit: Edit) {
    let mut q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    q.push(generation(), edit);
    PENDING.store(q.mask(), Ordering::Release);
}

/// Whether `consumer` has edits queued (a load of one atomic).
#[inline]
pub fn pending(consumer: Consumer) -> bool {
    PENDING.load(Ordering::Acquire) & 1 << consumer as u8 != 0
}

/// Takes `consumer`'s queued edits of the current map load, each owner
/// once, in the order first pushed.
pub fn drain(consumer: Consumer) -> Vec<Edit> {
    let mut q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    let edits = q.drain(generation(), consumer);
    PENDING.store(q.mask(), Ordering::Release);
    edits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_go_to_their_consumers_once_per_owner() {
        let mut q = Queue::new();
        let section = |grown| Edit::Section { bsp: 0xe1a0_0010, kind: KIND_CLUSTER, index: 3, grown };
        q.push(1, section(false));
        q.push(1, Edit::Image { bitmap: 7, image: 0 });
        q.push(1, section(true));
        q.push(1, section(false));
        q.push(1, Edit::Image { bitmap: 7, image: 0 });
        q.push(1, Edit::Section { bsp: 0x55, kind: KIND_MODE, index: 2, grown: false });
        assert_eq!(q.mask(), 0b11111);
        assert_eq!(q.drain(1, Consumer::World), vec![section(true), Edit::Image { bitmap: 7, image: 0 }]);
        assert_eq!(q.mask(), 0b11110);
        assert_eq!(
            q.drain(1, Consumer::Objects),
            vec![Edit::Image { bitmap: 7, image: 0 }, Edit::Model { tag: 0x55 }],
            "a mode section is its model's edit"
        );
        assert_eq!(q.mask(), 0b11100);
    }

    #[test]
    fn an_image_reaches_every_holder_and_a_model_the_glue_that_copies_models() {
        let mut q = Queue::new();
        q.push(1, Edit::Image { bitmap: 7, image: 2 });
        q.push(1, Edit::Model { tag: 9 });
        let image = Edit::Image { bitmap: 7, image: 2 };
        for consumer in [Consumer::World, Consumer::Objects, Consumer::FirstPerson, Consumer::Lights, Consumer::Decorators] {
            assert!(q.edits[consumer as usize].contains(&image), "{consumer:?} takes images");
        }
        let model = Edit::Model { tag: 9 };
        for (consumer, takes) in [
            (Consumer::World, false),
            (Consumer::Objects, true),
            (Consumer::FirstPerson, true),
            (Consumer::Lights, true),
            (Consumer::Decorators, true),
        ] {
            assert_eq!(q.edits[consumer as usize].contains(&model), takes, "{consumer:?} and models");
        }
    }

    #[test]
    fn a_section_of_a_bsp_reaches_the_world_and_the_decorators() {
        let mut q = Queue::new();
        let section = Edit::Section { bsp: 0xe1a0_0010, kind: KIND_INSTANCE_DEF, index: 1, grown: false };
        q.push(1, section);
        assert_eq!(q.drain(1, Consumer::World), vec![section]);
        assert_eq!(q.drain(1, Consumer::Decorators), vec![section]);
        assert!(q.drain(1, Consumer::FirstPerson).is_empty());
        assert!(q.drain(1, Consumer::Lights).is_empty());
    }

    #[test]
    fn a_tag_write_goes_to_both_once() {
        let mut q = Queue::new();
        let weap = Edit::Tag { group: u32::from_be_bytes(*b"weap"), tag: 0xe111_0001 };
        q.push(1, weap);
        q.push(1, weap);
        q.push(1, Edit::Tag { group: u32::from_be_bytes(*b"bitm"), tag: 0xe222_0002 });
        assert_eq!(q.drain(1, Consumer::World).len(), 2);
        assert_eq!(q.drain(1, Consumer::Objects)[0], weap);
        assert!(q.drain(1, Consumer::FirstPerson).is_empty());
    }

    #[test]
    fn a_shader_groups_write_is_a_shader_edit_for_its_native_consumers() {
        let mut q = Queue::new();
        for (n, group) in [b"shad", b"stem", b"spas", b"vrtx"].into_iter().enumerate() {
            q.push(1, Edit::Tag { group: u32::from_be_bytes(*group), tag: 0xe000_0000 + n as u32 });
        }
        q.push(1, Edit::Tag { group: u32::from_be_bytes(*b"shad"), tag: 0xe000_0000 });
        let want: Vec<Edit> = (0..4).map(|n| Edit::Shader { tag: 0xe000_0000 + n }).collect();
        assert_eq!(q.mask(), 0b111);
        for consumer in [Consumer::World, Consumer::Objects, Consumer::FirstPerson] {
            assert_eq!(q.drain(1, consumer), want, "{consumer:?}");
        }
    }

    #[test]
    fn a_new_map_load_drops_what_is_queued() {
        let mut q = Queue::new();
        q.push(1, Edit::Model { tag: 9 });
        assert!(q.drain(2, Consumer::Objects).is_empty());
        q.push(2, Edit::Model { tag: 9 });
        q.push(3, Edit::Image { bitmap: 1, image: 1 });
        assert_eq!(q.drain(3, Consumer::Objects), vec![Edit::Image { bitmap: 1, image: 1 }]);
    }
}
