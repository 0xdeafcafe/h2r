//! Links written as retail data (docs/game/forge-design.md, "What the
//! engine allows at runtime"). A teleporter link sets the shared identifier
//! of a sender and receiver netgame flag through the scenario editor. A
//! device group is a new scenario device group that the linked machines
//! name as their position group; the scenario editor has no device group
//! block, so those bytes are written after the save, before the kit.
use std::io::Cursor;

use forge_doc::{BlockKind, TELEPORTER_RECEIVER, TELEPORTER_SENDER, TeleporterFlag};
use map::Map;
use scenario_edit::{Command, Session, Value as ScenarioValue, layout};

use crate::{Error, kit, placement::invalid};

/// Device groups (`scnr+0xa0`, stride `0x28`): a 32-byte name, the initial
/// value (`f32`, `+0x20`) and flags (`u32`, `+0x24`). V on 01b, 03a and 05a
/// (`h1campaign::scripts::level::h2::DEVICE_GROUPS`).
const DEVICE_GROUPS: u32 = 0xa0;
const DEVICE_GROUP_STRIDE: u32 = 0x28;
const DEVICE_GROUP_LIMIT: usize = i16::MAX as usize + 1;
/// A machine placement's position group (`i16`, `+0x36`): -1 or a device
/// group index. V on 01b, 03a, 05a and 08a
/// (`h1campaign::scenario::H2_DEVICE_FIELDS`).
const POSITION_GROUP: u32 = 0x36;

/// Set each linked flag's identifier to its channel. The flag must already
/// be a teleporter of the role the link names.
pub(super) fn teleporters(
    session: &mut Session,
    flags: &[TeleporterFlag],
) -> forge_doc::Result<()> {
    let what = BlockKind::NetgameFlags.name();
    let field = layout::block(BlockKind::NetgameFlags)
        .field("type")
        .ok_or_else(|| invalid(what, "netgame flags have no type field"))?;
    for flag in flags {
        let scenario = session
            .scenario()
            .ok_or_else(|| invalid(what, "scenario is not open"))?;
        let kind = scenario
            .get(BlockKind::NetgameFlags, flag.index as usize, field.name)
            .map_err(|e| invalid(what, e))?
            .as_int();
        let (want, role) = if flag.sender {
            (TELEPORTER_SENDER, "sender")
        } else {
            (TELEPORTER_RECEIVER, "receiver")
        };
        if kind != Some(i64::from(want)) {
            return Err(invalid(
                what,
                format!("flag {} is not a teleporter {role}", flag.index),
            ));
        }
        session
            .apply(Command::set(
                BlockKind::NetgameFlags,
                flag.index as usize,
                "identifier",
                ScenarioValue::Int(i64::from(flag.channel)),
            ))
            .map_err(|e| invalid(what, e))?;
    }
    Ok(())
}

/// Append one device group per linked component and point its machines'
/// position group at it. The groups follow the map's own, in the order
/// `forge_doc::Links::device_groups` gives.
pub(super) fn device_groups(bytes: Vec<u8>, groups: &[Vec<u32>]) -> Result<Vec<u8>, Error> {
    if groups.is_empty() {
        return Ok(bytes);
    }
    let mut map = Map::from_reader(Cursor::new(bytes.as_slice()))?;
    let scenario = map.scenario().ok_or(Error::Links("missing scenario"))?.id;
    let root = map
        .tag(scenario)?
        .location
        .ok_or(Error::Links("scenario has no local payload"))?
        .address;
    let fields = map.read_tag_bytes(scenario)?;
    let pair = |offset: u32| {
        fields
            .get(offset as usize..offset as usize + 8)
            .map(|p| (kit::word(&p[..4]), kit::word(&p[4..])))
            .ok_or(Error::Links("scenario root too short"))
    };
    let (count, address) = pair(DEVICE_GROUPS)?;
    let machines = layout::block(BlockKind::Machines);
    let (machine_count, machine_address) = pair(machines.offset)?;
    if count as usize + groups.len() > DEVICE_GROUP_LIMIT {
        return Err(Error::Links("too many device groups"));
    }
    let mut data = kit::read_entries(&mut map, address, count * DEVICE_GROUP_STRIDE)?;
    for n in 0..groups.len() {
        data.extend(group_entry(n));
    }
    let mut editor = map.edit();
    let pointer = editor.append_metadata(&data, 16)?;
    let mut root_pair = (count + groups.len() as u32).to_le_bytes().to_vec();
    root_pair.extend(pointer.to_le_bytes());
    editor.patch_metadata(root + DEVICE_GROUPS, &root_pair)?;
    for (n, members) in groups.iter().enumerate() {
        let group = (count as usize + n) as i16;
        for &machine in members {
            if machine >= machine_count {
                return Err(Error::Links("a device link names a missing machine"));
            }
            editor.patch_metadata(
                machine_address + machine * machines.stride + POSITION_GROUP,
                &group.to_le_bytes(),
            )?;
        }
    }
    let mut output = Cursor::new(Vec::new());
    editor.write_to(&mut output)?;
    Ok(output.into_inner())
}

/// A new group: our own name, initial value 0 and no flags.
fn group_entry(n: usize) -> Vec<u8> {
    let mut entry = vec![0; DEVICE_GROUP_STRIDE as usize];
    let name = format!("forge link {}", n + 1);
    entry[..name.len()].copy_from_slice(name.as_bytes());
    entry
}
