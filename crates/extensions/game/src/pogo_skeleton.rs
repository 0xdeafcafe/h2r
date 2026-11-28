//! The skeleton a rider is posed on (docs/game/pogo-stick.md, "The
//! pose"): a biped's render model's nodes, by name, read from the map file.
//!
//! The game keeps no node names in memory, so the names come from the map
//! file the game loaded ([`crate::native_map::open`]), once a map and
//! render model, on a worker thread: [`request`] gives the skeleton when
//! it is ready and `None` until then, or for good when it did not read
//! (said once, by name). [`mode_of`] is the guest's side: an object's
//! definition tag to its render model's tag.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use guest_layout::DatumHandle;
use guest_layout::memory::GuestMemory;
use guest_layout::tags::GroupTag;
use guest_layout::view::Tags;

use crate::pogo_pose::{Bone, Skeleton, rest_of};

/// An object definition's model reference (`hlmt`), its datum.
pub const DEFINITION_MODEL: u32 = 0x38;
/// A model's render model reference (`mode`), its datum.
pub const MODEL_RENDER: u32 = 0x4;
/// Skeletons kept before the oldest map's are dropped.
pub const KEPT: usize = 64;

/// The render model (`mode`) tag datum of the object definition tag
/// `definition`: the definition's `hlmt` at [`DEFINITION_MODEL`], its
/// `mode` at [`MODEL_RENDER`], each a tag of its group in the published
/// index. `None` when any step does not resolve.
pub fn mode_of(m: &dyn GuestMemory, definition: u32) -> Option<u32> {
    let tag = |datum: u32, group: [u8; 4]| {
        Tags.get(m, DatumHandle(datum), GroupTag::from_chars(group))
            .ok()
            .flatten()
            .filter(|at| *at != 0)
    };
    let def = tag(definition, *b"obje")?;
    let model = m.u32(def.checked_add(DEFINITION_MODEL)?).ok()?;
    let hlmt = tag(model, *b"hlmt")?;
    let mode = m.u32(hlmt.checked_add(MODEL_RENDER)?).ok()?;
    tag(mode, *b"mode")?;
    Some(mode)
}

/// One node of a render model as a bone: its name (lower case; none is
/// empty), its parent (`-1` none) and its rest frame from the inverse
/// rest frame the node carries. `None` when the frame is not finite.
pub fn bone(name: Option<&str>, parent: i16, inverse: &[f32]) -> Option<Bone> {
    Some(Bone {
        name: name.unwrap_or_default().to_ascii_lowercase(),
        parent: usize::try_from(parent).ok(),
        rest: rest_of(inverse)?,
    })
}

/// Which skeleton: a map's render model.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    /// The loaded map's name.
    pub map: String,
    /// The `mode` tag's datum.
    pub mode: u32,
}

#[derive(Clone, Debug)]
enum Slot {
    Loading,
    Ready(Arc<Skeleton>),
    Failed,
}

/// What [`Cache::ask`] says.
#[derive(Clone, Debug, PartialEq)]
pub enum Ask {
    Ready(Arc<Skeleton>),
    /// Being read, or it did not read.
    Wait,
    /// Not asked for before: the caller reads it and calls
    /// [`Cache::finish`].
    Start,
}

/// The skeletons read so far.
#[derive(Debug, Default)]
pub struct Cache {
    slots: HashMap<Key, Slot>,
}

impl Cache {
    pub fn ask(&mut self, key: &Key) -> Ask {
        match self.slots.get(key) {
            Some(Slot::Ready(s)) => Ask::Ready(Arc::clone(s)),
            Some(Slot::Loading | Slot::Failed) => Ask::Wait,
            None => {
                if self.slots.len() >= KEPT {
                    // Another map's, long since: all but this map's go.
                    self.slots.retain(|k, _| k.map == key.map);
                }
                self.slots.insert(key.clone(), Slot::Loading);
                Ask::Start
            }
        }
    }

    /// Files what reading `key` came to. The message to print, once: why
    /// it did not read, or why the model it read is not posed.
    pub fn finish(&mut self, key: &Key, read: Result<Skeleton, String>) -> Option<String> {
        let said = |why: &str| {
            format!(
                "pogo: {}: the rider of render model {:#010x} is not posed: {why}",
                key.map, key.mode
            )
        };
        match read {
            Ok(s) => {
                let why = s.roles().err().map(said);
                self.slots.insert(key.clone(), Slot::Ready(Arc::new(s)));
                why
            }
            Err(e) => {
                self.slots.insert(key.clone(), Slot::Failed);
                Some(said(&e))
            }
        }
    }

    /// Lets `key` be asked for again (a reader that never started).
    pub fn forget(&mut self, key: &Key) {
        self.slots.remove(key);
    }
}

fn cache() -> MutexGuard<'static, Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

#[cfg(feature = "gpu")]
fn read(key: &Key) -> Result<Skeleton, String> {
    use tags::model::RenderModelGeometry;
    let keys = crate::pogo_pose::keys()?;
    let (mut file, files) = crate::native_map::open(&key.map).map_err(|e| e.to_string())?;
    let ctx = tags::object::Context::read(&mut file).map_err(|e| e.to_string())?;
    let mut raw = crate::native_world_glue::Raw(files);
    let geo =
        RenderModelGeometry::read_in(&mut file, ::map::TagId(key.mode), &mut raw, Some(&ctx.meta))
            .map_err(|e| e.to_string())?;
    let bones = geo
        .nodes
        .iter()
        .map(|n| {
            bone(
                n.name.as_deref(),
                n.wire.parent_index,
                &n.wire.unknown_028[..],
            )
        })
        .collect::<Option<Vec<_>>>()
        .ok_or("a node's rest frame is not finite")?;
    Ok(Skeleton::new(bones, keys))
}

#[cfg(not(feature = "gpu"))]
fn read(_: &Key) -> Result<Skeleton, String> {
    Err("this build has no map access".into())
}

/// The skeleton of render model `mode` of the loaded map `map`: `None`
/// while it is being read (the first call starts that, on its own thread)
/// and when it did not read.
pub fn request(map: &str, mode: u32) -> Option<Arc<Skeleton>> {
    let key = Key {
        map: map.to_owned(),
        mode,
    };
    match cache().ask(&key) {
        Ask::Ready(s) => Some(s),
        Ask::Wait => None,
        Ask::Start => {
            let k = key.clone();
            let spawned = std::thread::Builder::new()
                .name("pogo-skeleton".into())
                .spawn(move || {
                    let found = std::panic::catch_unwind(|| read(&k))
                        .unwrap_or_else(|_| Err("reading it panicked".into()));
                    let said = cache().finish(&k, found);
                    if let Some(said) = said {
                        eprintln!("{said}");
                    }
                });
            if spawned.is_err() {
                cache().forget(&key);
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use guest_layout::points;
    use guest_layout::testing::FakeGuest;
    use guest_layout::tags as guest_tags;

    use super::*;

    #[test]
    fn a_node_is_a_bone_with_its_name_parent_and_rest_frame() {
        // The identity's inverse is the identity, moved back.
        let mut inverse = [0.0f32; 14];
        inverse[0] = 1.0;
        inverse[1] = 1.0;
        inverse[5] = 1.0;
        inverse[9] = 1.0;
        inverse[10..13].copy_from_slice(&[-0.1, 0.2, -0.3]);
        let b = bone(Some("L_Thigh"), 3, &inverse).unwrap();
        assert_eq!(b.name, "l_thigh");
        assert_eq!(b.parent, Some(3));
        for (got, want) in b.rest.position.iter().zip([0.1, -0.2, 0.3]) {
            assert!((got - want).abs() < 1e-6);
        }
        assert_eq!(b.rest.forward, [1.0, 0.0, 0.0]);
        let root = bone(None, -1, &inverse).unwrap();
        assert_eq!((root.name.as_str(), root.parent), ("", None));
        inverse[4] = f32::INFINITY;
        assert!(bone(Some("x"), 0, &inverse).is_none());
        assert!(bone(Some("x"), 0, &inverse[..5]).is_none());
    }

    fn key(mode: u32) -> Key {
        Key {
            map: "court".into(),
            mode,
        }
    }

    fn empty() -> Skeleton {
        let keys = crate::pogo_pose::keys().unwrap();
        Skeleton::new(Vec::new(), keys)
    }

    #[test]
    fn a_skeleton_is_read_once_and_a_failure_is_said_once() {
        let mut c = Cache::default();
        assert_eq!(c.ask(&key(1)), Ask::Start);
        assert_eq!(c.ask(&key(1)), Ask::Wait);
        assert_eq!(c.ask(&key(2)), Ask::Start);
        // One that read, though no role is found in it: kept, and said.
        let said = c.finish(&key(1), Ok(empty())).unwrap();
        assert!(
            said.contains("court") && said.contains("0x00000001"),
            "{said}"
        );
        assert!(said.contains("no pelvis node"), "{said}");
        assert!(matches!(c.ask(&key(1)), Ask::Ready(_)));
        // One that did not read: never asked for again.
        let said = c.finish(&key(2), Err("no such tag".into())).unwrap();
        assert!(said.contains("no such tag"), "{said}");
        assert_eq!(c.ask(&key(2)), Ask::Wait);
        assert_eq!(c.ask(&key(2)), Ask::Wait);
        // A reader that never started may be started again.
        assert_eq!(c.ask(&key(3)), Ask::Start);
        c.forget(&key(3));
        assert_eq!(c.ask(&key(3)), Ask::Start);
    }

    #[test]
    fn a_full_cache_drops_other_maps() {
        let mut c = Cache::default();
        for mode in 0..KEPT as u32 {
            assert_eq!(c.ask(&key(mode)), Ask::Start);
        }
        let other = Key {
            map: "beaver".into(),
            mode: 0,
        };
        assert_eq!(c.ask(&other), Ask::Start);
        assert_eq!(c.ask(&other), Ask::Wait);
        // The first map's were dropped for it.
        assert_eq!(c.ask(&key(0)), Ask::Start);
    }

    const INDEX: u32 = 0x1_0000;
    const GROUPS: u32 = 0x1_1000;
    const ENTRIES: u32 = 0x1_2000;
    const BIPED: u32 = 0x2_0000;
    const MODEL: u32 = 0x2_1000;
    const RENDER: u32 = 0x2_2000;
    const BIPED_TAG: u32 = 0xe174_0001;
    const MODEL_TAG: u32 = 0xe175_0002;
    const RENDER_TAG: u32 = 0xe176_0003;

    fn group(chars: &[u8; 4]) -> u32 {
        GroupTag::from_chars(*chars).0
    }

    /// A published index of three tags: a biped, its model and its
    /// render model.
    fn tagged() -> FakeGuest {
        use guest_tags::{entry_layout as entry, group_layout as record, index_layout as index};
        let mut m = FakeGuest::new();
        m.map(INDEX, 0x40).map(BIPED, 0x100).map(MODEL, 0x100);
        m.map(RENDER, 0x100);
        m.put_u32(points::TAG_INDEX.base, INDEX)
            .put_u32(points::TAG_ENTRIES.base, ENTRIES)
            .put_u32(INDEX + index::GROUPS, GROUPS)
            .put_u32(INDEX + index::GROUP_COUNT, 3)
            .put_u32(INDEX + index::TAG_COUNT, 4);
        // Sorted, as the search wants: bipd, hlmt, mode.
        let groups = [
            (group(b"bipd"), group(b"unit"), group(b"obje")),
            (group(b"hlmt"), u32::MAX, u32::MAX),
            (group(b"mode"), u32::MAX, u32::MAX),
        ];
        for (i, (g, parent, grand)) in groups.into_iter().enumerate() {
            let at = GROUPS + i as u32 * record::STRIDE;
            m.put_u32(at + record::GROUP, g)
                .put_u32(at + record::PARENT, parent)
                .put_u32(at + record::GRANDPARENT, grand);
        }
        let entries = [
            (0, u32::MAX, 0),
            (group(b"bipd"), BIPED_TAG, BIPED),
            (group(b"hlmt"), MODEL_TAG, MODEL),
            (group(b"mode"), RENDER_TAG, RENDER),
        ];
        for (i, (g, datum, at)) in entries.into_iter().enumerate() {
            let e = ENTRIES + i as u32 * entry::STRIDE;
            m.put_u32(e + entry::GROUP, g)
                .put_u32(e + entry::DATUM, datum)
                .put_u32(e + entry::ADDRESS, at);
        }
        m.put_u32(BIPED + DEFINITION_MODEL, MODEL_TAG)
            .put_u32(MODEL + MODEL_RENDER, RENDER_TAG);
        m
    }

    #[test]
    fn a_definition_leads_to_its_render_model() {
        let mut m = tagged();
        assert_eq!(mode_of(&m, BIPED_TAG), Some(RENDER_TAG));
        // A stale datum, a tag that is no object, and no tag at all.
        assert_eq!(mode_of(&m, 0xe999_0001), None);
        assert_eq!(mode_of(&m, MODEL_TAG), None);
        assert_eq!(mode_of(&m, u32::MAX), None);
        // A definition with no model, and a model with no render model.
        m.put_u32(MODEL + MODEL_RENDER, u32::MAX);
        assert_eq!(mode_of(&m, BIPED_TAG), None);
        m.put_u32(MODEL + MODEL_RENDER, RENDER_TAG)
            .put_u32(BIPED + DEFINITION_MODEL, u32::MAX);
        assert_eq!(mode_of(&m, BIPED_TAG), None);
        // A model reference that names a tag of another group.
        m.put_u32(BIPED + DEFINITION_MODEL, RENDER_TAG);
        assert_eq!(mode_of(&m, BIPED_TAG), None);
        assert_eq!(mode_of(&FakeGuest::new(), BIPED_TAG), None);
    }
}
