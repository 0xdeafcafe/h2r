//! `set_loadout` on the host (docs/game/rules-world-api.md, "Loadouts"):
//! the player's living unit gets the weapons the script names, at the
//! next tick boundary.
//!
//! A script names a weapon by its tag path; the host gives only the
//! multiplayer weapon list's (`rules::weapons`), by the starting-weapon
//! value the game maps to the list's slot. The game's own routine
//! [`STARTING_WEAPON_PICK`](guest_layout::points::STARTING_WEAPON_PICK)
//! (`0x19eda0`) answers the tag of a value from the map's multiplayer
//! globals, as the spawn's weapon pass asks it. Each weapon is then made
//! and added as a starting profile does (`agent::sim::calls`, "Give"):
//! `placement_data_new`, `object_new`, `unit_add_weapon`, and
//! `object_delete` when the unit refuses it. The primary is added in mode
//! 2, which empties the unit's weapon slots first and readies it; the
//! secondary in mode 1, beside it (mode 2 when there is no primary).
//!
//! The calls need `0xc4` bytes of guest memory for the placement:
//! [`set_scratch`] keeps the block the feature allocates at install.

use std::sync::atomic::{AtomicU32, Ordering};

use agent::sim::calls::addr;
use guest_layout::entry::{Cx, GuestCall};
use guest_layout::points;

/// The scratch block's size: a placement, rounded up.
pub const SCRATCH_SIZE: u32 = 0x100;
const _: () = assert!(addr::PLACEMENT_SIZE <= SCRATCH_SIZE);

/// "No weapon", in and out of the pick routine.
pub const NO_TAG: u32 = u32::MAX;

static SCRATCH: AtomicU32 = AtomicU32::new(0);

/// Keeps the guest block the placement is built in.
pub fn set_scratch(at: u32) {
    SCRATCH.store(at, Ordering::SeqCst);
}

fn scratch() -> Option<u32> {
    Some(SCRATCH.load(Ordering::SeqCst)).filter(|&a| a != 0)
}

/// One weapon to give: the starting-weapon value and `unit_add_weapon`'s
/// mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Give {
    pub value: u8,
    pub mode: u32,
}

/// The weapons a loadout gives, in order: the primary in mode 2, the
/// secondary in mode 1 (in mode 2 when there is no primary). A path off
/// the multiplayer list is refused by name; nothing named gives nothing.
pub fn plan(primary: Option<&str>, secondary: Option<&str>) -> Result<Vec<Give>, String> {
    let value = |path: &str| {
        ::rules::weapons::value_of(path).ok_or_else(|| {
            format!("{path} is not a weapon of the multiplayer list, the only weapons a host gives")
        })
    };
    let mut out = Vec::new();
    if let Some(p) = primary {
        out.push(Give {
            value: value(p)?,
            mode: addr::MODE_REPLACE,
        });
    }
    if let Some(s) = secondary {
        out.push(Give {
            value: value(s)?,
            mode: if out.is_empty() { addr::MODE_REPLACE } else { addr::MODE_ADD },
        });
    }
    Ok(out)
}

/// Gives player `player`'s living unit the loadout.
pub async fn give(cx: &Cx, player: u8, primary: Option<&str>, secondary: Option<&str>) -> anyhow::Result<()> {
    let gives = plan(primary, secondary).map_err(anyhow::Error::msg)?;
    if gives.is_empty() {
        return Ok(());
    }
    let scratch = scratch().ok_or_else(|| anyhow::anyhow!("the loadout's scratch memory is not set"))?;
    let unit = cx
        .read(|m| {
            crate::firefight::runtime::read_players(m)
                .ok()?
                .into_iter()
                .find(|(p, _)| p.slot == player && p.alive)
                .and_then(|(_, unit)| unit)
        })
        .ok_or_else(|| anyhow::anyhow!("player {player} has no living unit"))?;
    for g in gives {
        let tag = cx
            .call(GuestCall::new(points::STARTING_WEAPON_PICK.base).args([NO_TAG, u32::from(g.value), NO_TAG]))
            .await
            .map_err(guest)?
            .eax;
        anyhow::ensure!(
            tag != NO_TAG,
            "the map's multiplayer weapon list has no weapon for value {}",
            g.value
        );
        cx.call(GuestCall::new(addr::PLACEMENT_NEW).esi(scratch).args([tag, unit, 0]))
            .await
            .map_err(guest)?;
        let made = cx
            .call(GuestCall::new(addr::OBJECT_NEW).args([scratch]))
            .await
            .map_err(guest)?
            .eax;
        anyhow::ensure!(made != u32::MAX, "object_new refused to make tag {tag:#010x}");
        let added = cx
            .call(GuestCall::new(addr::UNIT_ADD_WEAPON).args([unit, made, g.mode]))
            .await
            .map_err(guest)?
            .eax
            & 0xff;
        if added == 0 {
            cx.call(GuestCall::new(addr::OBJECT_DELETE).args([made]))
                .await
                .map_err(guest)?;
            anyhow::bail!("unit_add_weapon (mode {}) refused tag {tag:#010x}; the weapon was deleted", g.mode);
        }
    }
    Ok(())
}

fn guest(e: impl Into<guest_layout::entry::EntryError>) -> anyhow::Error {
    anyhow::anyhow!("{}", e.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BR: &str = "objects/weapons/rifle/battle_rifle/battle_rifle";
    const MAGNUM: &str = "objects/weapons/pistol/magnum/magnum";

    #[test]
    fn the_primary_replaces_and_the_secondary_adds() {
        assert_eq!(
            plan(Some(BR), Some(MAGNUM)).unwrap(),
            vec![
                Give { value: 3, mode: addr::MODE_REPLACE },
                Give { value: 4, mode: addr::MODE_ADD },
            ]
        );
        assert_eq!(plan(Some(BR), None).unwrap(), vec![Give { value: 3, mode: addr::MODE_REPLACE }]);
        // A secondary alone empties the slots too.
        assert_eq!(plan(None, Some(MAGNUM)).unwrap(), vec![Give { value: 4, mode: addr::MODE_REPLACE }]);
        assert_eq!(plan(None, None).unwrap(), vec![]);
    }

    #[test]
    fn a_weapon_off_the_list_is_refused_by_its_path() {
        let why = plan(Some(BR), Some("objects/weapons/multiplayer/flag/flag")).unwrap_err();
        assert!(why.contains("objects/weapons/multiplayer/flag/flag"), "{why}");
        assert!(why.contains("multiplayer list"), "{why}");
    }
}
