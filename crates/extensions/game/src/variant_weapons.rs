//! The variant's other weapons as the starting and secondary weapon
//! (docs/game/game-options.md, "Any weapon as the starting or secondary
//! weapon").
//!
//! The spawn's weapon pass (`0x19ef40`) maps the record's two weapon bytes
//! to tags through one routine ([`points::STARTING_WEAPON_PICK`],
//! `0x19eda0`), which can only name a slot of the multiplayer globals'
//! list. When the block names one of our weapons
//! ([`variants::ext_weapons`]) for the slot the call is for, this hook
//! answers with that weapon's tag instead; the pass then treats it as any
//! weapon. The record keeps its own byte as the fallback: a map without the
//! tag, or a call this hook does not answer, gets what retail gives.
//!
//! **Where it applies.** The spawn is the host's: a machine alone and a
//! Live host apply the block ([`Decision::weapons`]); a guest's unit comes
//! from its host. A Superbounce game whose variant has the stick on keeps
//! the stick as its secondary ([`crate::pogo_loadout`]), so this hook
//! leaves that call to it.
//!
//! **Which tag.** A tag's path is not in guest memory, so the datum is read
//! from the loaded map's file, found through the VFS as our skulls' tag is
//! ([`crate::skulls_custom::MapLocator`]), once per map and scenario. Every
//! machine that makes the choice reads the same file, so the choice is the
//! same on each.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use guest_layout::GuestMemory;
use guest_layout::points;
use hooks::{BuildError, Builder, Flow, Manifest};
use variants::ext_weapons;

use crate::pogo_loadout::{ARG_BYTES, SECONDARY_RETURNS};

/// The feature id (`features.variant-weapons` switches it).
pub const FEATURE: &str = "variant-weapons";

/// Where the weapon pass's calls for the primary weapon return to (the
/// secondary's are [`SECONDARY_RETURNS`]; `0x19ee18` is the routine's own
/// random retry).
pub const PRIMARY_RETURNS: [u32; 2] = [0x0019_f056, 0x0019_f063];

/// Which weapon of the pass a call returning to `ret` is for: `Some(false)`
/// the primary, `Some(true)` the secondary, `None` for anything else (the
/// routine's own retry, a call of ours).
pub fn slot_of(ret: u32) -> Option<bool> {
    if PRIMARY_RETURNS.contains(&ret) {
        Some(false)
    } else if SECONDARY_RETURNS.contains(&ret) {
        Some(true)
    } else {
        None
    }
}

/// The id the block asks for in a slot, if any: the primary's or the
/// secondary's, 0 meaning the record's own.
pub fn id_for(weapons: (u16, u16), secondary: bool) -> Option<u16> {
    let id = if secondary { weapons.1 } else { weapons.0 };
    (id != 0).then_some(id)
}

/// The tag the call is answered with: the block's weapon for the slot,
/// resolved by its path through `tag_of`. `None` leaves the routine to
/// answer as retail does.
pub fn answer(
    ret: u32,
    weapons: Option<(u16, u16)>,
    tag_of: impl FnOnce(&str) -> Option<u32>,
) -> Option<u32> {
    let secondary = slot_of(ret)?;
    let id = id_for(weapons?, secondary)?;
    let weapon = ext_weapons::get(id)?;
    tag_of(weapon.path).filter(|&t| t != crate::pogo_loadout::NO_TAG)
}

/// The `weap` tags of a map as (datum, group, name) rows to a path to
/// datum table, paths canonical ([`ext_weapons::canonical`]).
pub fn weapon_table<'a>(
    rows: impl IntoIterator<Item = (u32, [u8; 4], &'a str)>,
) -> HashMap<String, u32> {
    rows.into_iter()
        .filter(|(_, g, _)| g == b"weap")
        .map(|(d, _, n)| (ext_weapons::canonical(n), d))
        .collect()
}

/// A map's weapon tags, with the map and scenario they were read for.
struct Cached {
    key: (String, u32),
    tags: HashMap<String, u32>,
}

/// The feature: the hook on the pick routine.
pub struct Weapons(Arc<State>);

impl Weapons {
    /// A feature that finds map files through `maps`.
    pub fn new(maps: Option<crate::skulls_custom::MapLocator>) -> Self {
        Self(Arc::new(State {
            maps,
            cache: Mutex::new(None),
            said: Mutex::new(None),
        }))
    }
}

/// The hook's state: how to find map files, and the table last read.
struct State {
    maps: Option<crate::skulls_custom::MapLocator>,
    cache: Mutex<Option<Cached>>,
    /// The failure last printed, so a game says each once.
    said: Mutex<Option<String>>,
}

impl State {
    fn say_once(&self, line: String) {
        let mut said = self.said.lock().unwrap_or_else(|e| e.into_inner());
        if said.as_ref() != Some(&line) {
            eprintln!("{line}");
            *said = Some(line);
        }
    }

    /// The datum of the weapon at `path` in the loaded map.
    fn tag_of(&self, m: &dyn GuestMemory, path: &str) -> Option<u32> {
        let map = crate::health::loaded_map_name(m).ok()?;
        let scenario = guest_layout::view::Tags.scenario(m).ok().flatten()?.raw();
        let key = (map.clone(), scenario);
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.as_ref().map(|c| &c.key) != Some(&key) {
            let file = crate::map_slots::active_file(&crate::layout_glue::ReadGm(m));
            match self.read_tags(&map, file.as_deref(), scenario) {
                Ok(tags) => *cache = Some(Cached { key, tags }),
                Err(why) => {
                    *cache = None;
                    drop(cache);
                    self.say_once(format!("variant weapons: {map}: {why}"));
                    return None;
                }
            }
        }
        let found = cache
            .as_ref()?
            .tags
            .get(&ext_weapons::canonical(path))
            .copied();
        drop(cache);
        if found.is_none() {
            self.say_once(format!(
                "variant weapons: {map} has no {path}; the record's weapon is used"
            ));
        }
        found
    }

    fn read_tags(
        &self,
        map: &str,
        path: Option<&str>,
        scenario: u32,
    ) -> Result<HashMap<String, u32>, String> {
        let locate = self.maps.as_ref().ok_or("no map locator (no VFS)")?;
        let file = locate(map, path).ok_or("no host file")?;
        let reader =
            std::fs::File::open(&file.path).map_err(|e| format!("{}: {e}", file.path.display()))?;
        let parsed =
            ::map::Map::from_region(std::io::BufReader::new(reader), file.offset, file.len)
                .map_err(|e| format!("{}: {e}", file.path.display()))?;
        let file_scenario = parsed.scenario().map(|t| t.id.0);
        if file_scenario != Some(scenario) {
            return Err(format!(
                "{} is not the loaded map (scenario {file_scenario:x?}, loaded {scenario:#x})",
                file.path.display()
            ));
        }
        Ok(weapon_table(parsed.tags().iter().filter_map(|t| {
            Some((t.id.0, t.group.0, t.name.as_deref()?))
        })))
    }

    /// The tag for the call whose stack is at `esp`, if the block asks for
    /// one of ours.
    fn decide(&self, m: &dyn GuestMemory, esp: u32) -> Option<u32> {
        let ret = m.u32(esp).ok()?;
        slot_of(ret)?;
        let weapons = crate::variant_ext::in_game(&crate::layout_glue::ReadGm(m)).weapons();
        weapons?;
        // The stick keeps the secondary of a Superbounce game.
        if crate::pogo_loadout::decide(m, esp).is_some() {
            return None;
        }
        answer(ret, weapons, |path| self.tag_of(m, path))
    }
}

impl hooks::Feature for Weapons {
    fn manifest(&self) -> Manifest {
        Manifest::new(FEATURE, "Game variant weapons outside the globals' list")
            .doc("docs/game/game-options.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let me = Arc::clone(&self.0);
        b.raw().before(
            &points::STARTING_WEAPON_PICK,
            "variant: a weapon of ours is the starting or secondary weapon",
            move |c| {
                let tag = {
                    let guest = c.ctx.runtime().memory();
                    let m: &dyn GuestMemory = &*guest;
                    me.decide(m, c.cpu.esp)
                };
                let Some(tag) = tag else {
                    return Ok(Flow::Continue);
                };
                c.cpu.eax = tag;
                c.ret(ARG_BYTES).map(Flow::Return)
            },
        )
        // A Superbounce game's stick answers the secondary pick first.
        .after_feature(crate::pogo_mode::FEATURE);
        // The table is read again from the map's file when the map or
        // scenario differs; nothing here a snapshot would miss.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMG_SILENCED: &str = "objects/weapons/rifle/smg_silenced/smg_silenced";

    fn table() -> HashMap<String, u32> {
        weapon_table([
            (
                0xe100_0001,
                *b"weap",
                "objects\\weapons\\rifle\\smg_silenced\\smg_silenced",
            ),
            (
                0xe100_0002,
                *b"weap",
                "Objects\\Weapons\\Melee\\Gravity_Hammer\\gravity_hammer",
            ),
            (
                0xe100_0003,
                *b"scnr",
                "objects\\weapons\\rifle\\spiker\\spiker",
            ),
        ])
    }

    #[test]
    fn the_calls_slot_is_told_by_its_return_address() {
        for r in PRIMARY_RETURNS {
            assert_eq!(slot_of(r), Some(false));
        }
        for r in SECONDARY_RETURNS {
            assert_eq!(slot_of(r), Some(true));
        }
        // The routine's own random retry, and a call of ours.
        assert_eq!(slot_of(0x0019_ee18), None);
        assert_eq!(slot_of(0x1234_5678), None);
    }

    #[test]
    fn only_weap_tags_are_kept_and_paths_match_in_any_slash_and_case() {
        let t = table();
        assert_eq!(t.len(), 2);
        assert_eq!(t[SMG_SILENCED], 0xe100_0001);
        assert_eq!(
            t["objects/weapons/melee/gravity_hammer/gravity_hammer"],
            0xe100_0002
        );
        assert!(!t.contains_key("objects/weapons/rifle/spiker/spiker"));
    }

    #[test]
    fn a_slot_is_answered_with_its_own_weapon() {
        let t = table();
        let tag = |p: &str| t.get(&ext_weapons::canonical(p)).copied();
        let silenced = ext_weapons::id_of_path(SMG_SILENCED).unwrap();
        let hammer =
            ext_weapons::id_of_path("objects/weapons/melee/gravity_hammer/gravity_hammer").unwrap();
        let w = Some((silenced, hammer));
        assert_eq!(answer(PRIMARY_RETURNS[0], w, tag), Some(0xe100_0001));
        assert_eq!(answer(PRIMARY_RETURNS[1], w, tag), Some(0xe100_0001));
        assert_eq!(answer(SECONDARY_RETURNS[0], w, tag), Some(0xe100_0002));
        assert_eq!(answer(SECONDARY_RETURNS[1], w, tag), Some(0xe100_0002));
        // One slot only: the other keeps the record's weapon.
        let w = Some((0, hammer));
        assert_eq!(answer(PRIMARY_RETURNS[0], w, tag), None);
        assert_eq!(answer(SECONDARY_RETURNS[0], w, tag), Some(0xe100_0002));
    }

    #[test]
    fn nothing_asked_nothing_known_or_no_tag_leaves_the_routine() {
        let t = table();
        let tag = |p: &str| t.get(&ext_weapons::canonical(p)).copied();
        let id = ext_weapons::TABLE[0].id;
        // The block holds nothing for this machine (retail, a guest, System Link).
        assert_eq!(answer(PRIMARY_RETURNS[0], None, tag), None);
        assert_eq!(answer(PRIMARY_RETURNS[0], Some((0, 0)), tag), None);
        // A call that is not the pass's.
        assert_eq!(answer(0x0019_ee18, Some((id, id)), tag), None);
        // An id this build lacks.
        assert_eq!(answer(PRIMARY_RETURNS[0], Some((999, 0)), tag), None);
        // The map has no such tag, or the datum is "no tag".
        assert_eq!(answer(PRIMARY_RETURNS[0], Some((3, 0)), tag), None);
        assert_eq!(
            answer(PRIMARY_RETURNS[0], Some((id, 0)), |_| Some(u32::MAX)),
            None
        );
        // The path the lookup is asked for is the weapon's own.
        let mut asked = None;
        let _ = answer(PRIMARY_RETURNS[0], Some((id, 0)), |p| {
            asked = Some(p.to_string());
            None
        });
        assert_eq!(asked.as_deref(), Some(ext_weapons::TABLE[0].path));
    }

    #[test]
    fn a_missing_locator_says_why_and_reads_nothing() {
        let w = Weapons::new(None);
        assert_eq!(
            w.0.read_tags("m", None, 1).unwrap_err(),
            "no map locator (no VFS)"
        );
    }
}
