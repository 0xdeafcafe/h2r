//! A pack relates an installed package's full content ID to its base level.
//! Retail list IDs belong to this machine and can change when offers rebuild.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use content::MapId;
use live_hle::mods::ModEntry;
use serde::{Deserialize, Serialize};

use super::{Catalogue, Category, Map, PREPARED, SOURCES, lock};

const INDEX: &str = "live-mod-packs.json";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct PackIndex(BTreeMap<String, Pack>);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Pack {
    name: String,
    members: BTreeMap<MapId, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct InstalledPack {
    pub key: String,
    pub name: String,
    // Slot and base in the order the player sees.
    pub members: Vec<(u32, String)>,
}

impl PackIndex {
    pub fn read(root: &Path) -> Self {
        std::fs::read(root.join(INDEX))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn update(&mut self, items: &[ModEntry]) -> bool {
        let mut updates = BTreeMap::<String, Pack>::new();
        for e in items {
            if let Some(info) = &e.pack_info
                && info.published_pack
            {
                let group = updates.entry(info.id.clone()).or_insert_with(|| Pack {
                    name: info.name.clone(),
                    members: BTreeMap::new(),
                });
                group.members.insert(e.id, e.base.to_ascii_lowercase());
            }
        }
        if updates
            .iter()
            .all(|(id, pack)| self.0.get(id) == Some(pack))
        {
            return false;
        }
        let moved: BTreeSet<_> = updates
            .values()
            .flat_map(|p| p.members.keys().copied())
            .collect();
        self.0.retain(|id, pack| {
            if updates.contains_key(id) {
                return false;
            }
            pack.members.retain(|id, _| !moved.contains(id));
            !pack.members.is_empty()
        });
        self.0.extend(updates);
        true
    }

    pub(super) fn installed(&self, cat: &Catalogue) -> Vec<InstalledPack> {
        self.0
            .iter()
            .filter_map(|(key, pack)| {
                let mut members = pack
                    .members
                    .iter()
                    .map(|(id, base)| {
                        let slot = *cat.package_slots.get(id)?;
                        cat.maps(Category::Mods).iter().find(|m| m.id == slot)?;
                        Some((slot, base.clone()))
                    })
                    .collect::<Option<Vec<_>>>()?;
                members.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
                (!members.is_empty()).then(|| InstalledPack {
                    key: key.clone(),
                    name: pack.name.clone(),
                    members,
                })
            })
            .collect()
    }
}

impl InstalledPack {
    pub(super) fn representative(&self) -> u32 {
        self.members[0].0
    }

    pub(super) fn slot_for(&self, map: &Map) -> Option<u32> {
        let base = map.short_name();
        let mut ids = self.members.iter().filter(|(_, name)| *name == base);
        let (slot, _) = ids.next()?;
        // An old curated pack can have two variants for one map. Let the
        // player choose an exact member inside the pack in that case.
        ids.next().is_none().then_some(*slot)
    }
}

/// Keep the server's metadata beside the local library. A partial page updates
/// only the packs it contains. Each returned pack contains its full membership.
pub fn remember_packs(items: &[ModEntry]) {
    let (Some(sel), Some(sources)) = (PREPARED.get(), SOURCES.get()) else {
        return;
    };
    let mut index = lock(&sel.pack_index);
    if !index.update(items) {
        return;
    }
    match serde_json::to_vec(&*index) {
        Ok(bytes) => {
            if let Err(e) = content::store::write_atomic(&sources.data_root.join(INDEX), &bytes) {
                eprintln!("mods: could not save installed pack metadata: {e}");
            }
        }
        Err(e) => eprintln!("mods: could not encode pack metadata: {e}"),
    }
    let mut cat = lock(&sel.catalogue);
    let packs = index.installed(&cat);
    std::sync::Arc::make_mut(&mut *cat).packs = packs;
}

pub(crate) fn active(key: &str) -> bool {
    PREPARED
        .get()
        .is_some_and(|sel| sel.catalogue().active_pack.as_deref() == Some(key))
}

pub(crate) fn playable(key: &str) -> bool {
    PREPARED
        .get()
        .is_some_and(|sel| sel.catalogue().packs.iter().any(|p| p.key == key))
}

pub(crate) fn toggle(key: &str) -> Option<bool> {
    let sel = PREPARED.get()?;
    let mut cat = lock(&sel.catalogue);
    let cat = std::sync::Arc::make_mut(&mut *cat);
    if cat.active_pack.as_deref() == Some(key) {
        cat.active_pack = None;
        Some(false)
    } else if cat.packs.iter().any(|p| p.key == key) {
        cat.active_pack = Some(key.to_owned());
        Some(true)
    } else {
        None
    }
}
