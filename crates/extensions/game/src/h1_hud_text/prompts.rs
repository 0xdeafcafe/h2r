//! Interactions that do not name a native string ID in the object tag.

use std::sync::{Arc, Mutex};

use guest_layout::handle::DatumHandle;
use engine::hs_actions;
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points::{
    HUD_INTERACTION_VISIBLE, PLAYER_PICKUP_CANDIDATE, PLAYER_PICKUP_SCAN, PLAYER_PRESS_INTERACT,
};
use guest_layout::tags::{entry_layout, tag_entry_checked};
use hooks::{Builder, Flow, GuestCall, Ret};
use h1_contract::hud::{
    EXIT_VEHICLE, MARKER, MARKER_AT, PROMPT_SUBJECT_SIZE, PROMPT_SUBJECTS, SWAP_POWERUP,
};

use super::{HUD, TEXT, TEXT_ID, events, flash, mission_hud};

const OBJECTS: u32 = 0x004e_0300;
const LOCAL_USERS: u32 = 0x004e_8c20;
const PLAYERS: u32 = 0x004e_8c24;
const OBJECT_PARENT: u32 = 0x14;
const UNIT_SEAT: u32 = 0x1fc;
const UNIT_EQUIPMENT: u32 = 0x238;
const NONE: u32 = u32::MAX;
const VEHICLE: u8 = 1;
const EQUIPMENT: u8 = 3;
const FROM_INTERACTION: u32 = 0x0019_197b;
const ATTACH_OBJECT: u32 = 0x000b_8ee0;
const DETACH_OBJECT: u32 = 0x000b_9a50;
const HIDE_OBJECT: u32 = 0x000b_9c60;

#[derive(Clone, Copy)]
struct Candidate {
    tag: u32,
    player: u32,
    tick: u32,
    object: u32,
    distance: f32,
}

const PLAYER_LIMIT: usize = 16;
type Candidates = [Option<Candidate>; PLAYER_LIMIT];

fn word(mem: &dyn GuestMemory, addr: u32) -> Option<u32> {
    mem.u32(addr).ok()
}

fn current_hud(mem: &dyn GuestMemory) -> Option<u32> {
    mission_hud(mem)
        .ok()
        .flatten()
        .filter(|&tag| word(mem, tag + MARKER_AT) == Some(MARKER))
}

fn player_for_user(mem: &dyn GuestMemory, user: u32) -> Option<u32> {
    let users = word(mem, LOCAL_USERS)?;
    if users == 0 || user >= 4 {
        return None;
    }
    word(mem, users + 0xc + 4 * user).filter(|&player| player != NONE)
}

fn player_slot(player: u32) -> Option<usize> {
    let slot = (player & 0xffff) as usize;
    (slot < PLAYER_LIMIT).then_some(slot)
}

fn datum_slot(mem: &dyn GuestMemory, global: u32, datum: u32, stride: u32) -> Option<u32> {
    let array = word(mem, global)?;
    let slot = datum & 0xffff;
    if datum == NONE || datum >> 16 == 0 || array == 0 || slot >= word(mem, array + 0x38)? {
        return None;
    }
    let header = word(mem, array + 0x44)? + stride * slot;
    (u32::from(mem.u16(header).ok()?) == datum >> 16).then_some(header)
}

fn object(mem: &dyn GuestMemory, datum: u32, kind: Option<u8>) -> Option<u32> {
    let header = datum_slot(mem, OBJECTS, datum, 0xc)?;
    if mem.u8(header + 2).ok()? & 0x10 != 0
        || kind.is_some_and(|kind| mem.u8(header + 3).ok() != Some(kind))
    {
        return None;
    }
    word(mem, header + 8).filter(|&body| body != 0)
}

fn controlled_unit(mem: &dyn GuestMemory, player: u32) -> Option<(u32, u32)> {
    let player = datum_slot(mem, PLAYERS, player, 0x21c)?;
    let unit = word(mem, player + 0x2c)?;
    Some((unit, object(mem, unit, None)?))
}

fn definition(mem: &dyn GuestMemory, datum: u32) -> Option<u32> {
    let entry = tag_entry_checked(mem, DatumHandle(datum)).ok()??;
    word(mem, entry + entry_layout::ADDRESS).filter(|&data| data != 0)
}

fn subject(mem: &dyn GuestMemory, tag: u32, definition: u32) -> Option<u16> {
    let count = word(mem, tag + PROMPT_SUBJECTS)?;
    let rows = word(mem, tag + PROMPT_SUBJECTS + 4)?;
    if count > u32::from(u16::MAX) || rows == 0 || definition == NONE {
        return None;
    }
    (0..count).find_map(|index| {
        let row = rows + index * PROMPT_SUBJECT_SIZE;
        (word(mem, row)? == definition)
            .then(|| mem.u16(row + 4).ok())
            .flatten()
    })
}

fn exit_subject(mem: &dyn GuestMemory, tag: u32, unit: u32) -> Option<u16> {
    let seat = u32::try_from(mem.u16(unit + UNIT_SEAT).ok()? as i16).ok()?;
    // A seated unit can have its input disabled (player_enable_input,
    // 0x2a8f16) or its action reserved for a script. In either case a
    // contextual exit prompt would advertise an unavailable interaction.
    let users = word(mem, LOCAL_USERS)?;
    let control = word(mem, hs_actions::PLAYER_CONTROL)?;
    if users == 0 || control == 0 || mem.u8(users + 6).ok()? != 0 {
        return None;
    }
    let claimed = word(mem, control + hs_actions::CAPTURE)?
        | word(mem, control + hs_actions::UNTIL_RELEASE)?;
    if claimed & hs_actions::ACTION != 0 {
        return None;
    }
    let vehicle = object(mem, word(mem, unit + OBJECT_PARENT)?, Some(VEHICLE))?;
    let id = word(mem, vehicle)?;
    let def = definition(mem, id)?;
    if seat >= word(mem, def + 0x1c8)? {
        return None;
    }
    subject(mem, tag, id)
}

#[derive(Clone, Copy)]
struct Swap {
    held: u32,
    offered: u32,
    target: u32,
    arguments: [u16; 2],
    distance: f32,
}

fn powerup(mem: &dyn GuestMemory, body: u32) -> Option<(u32, u16)> {
    let datum = word(mem, body)?;
    let kind = mem.u16(definition(mem, datum)? + 0x12c).ok()?;
    (1..=5).contains(&kind).then_some((datum, kind))
}

fn swap(mem: &dyn GuestMemory, tag: u32, player: u32, target: u32) -> Option<Swap> {
    if mem.u16(tag + SWAP_POWERUP).ok()? == u16::MAX {
        return None;
    }
    let (unit_datum, unit) = controlled_unit(mem, player)?;
    if word(mem, unit + OBJECT_PARENT)? != NONE {
        return None;
    }
    let held = word(mem, unit + UNIT_EQUIPMENT)?;
    let current = object(mem, held, Some(EQUIPMENT))?;
    let offered = object(mem, target, Some(EQUIPMENT))?;
    if word(mem, offered + OBJECT_PARENT)? != NONE || word(mem, offered + 0x14c)? == unit_datum {
        return None;
    }
    let (old_id, old_kind) = powerup(mem, current)?;
    let (new_id, new_kind) = powerup(mem, offered)?;
    if old_kind == new_kind {
        return None;
    }
    // Use the distance test of the native pickup candidate routine at 0x150ecf.
    let mut distance = 0.0;
    for axis in [0x30, 0x34, 0x38] {
        let delta = mem.f32(unit + axis).ok()? - mem.f32(offered + axis).ok()?;
        distance += delta * delta;
    }
    let radius = mem.f32(unit + 0x3c).ok()? + mem.f32(0x0044_ae90).ok()?;
    if !distance.is_finite() || !radius.is_finite() || distance >= radius * radius {
        return None;
    }
    Some(Swap {
        held,
        offered,
        target,
        arguments: [subject(mem, tag, old_id)?, subject(mem, tag, new_id)?],
        distance,
    })
}

fn tick(mem: &dyn GuestMemory) -> Option<u32> {
    let time = word(mem, flash::GAME_TIME)?;
    (time != 0).then(|| word(mem, time + 8)).flatten()
}

fn current_swap(
    mem: &dyn GuestMemory,
    tag: u32,
    player: u32,
    candidate: Option<Candidate>,
) -> Option<Swap> {
    let candidate = candidate?;
    if (tag, player) != (candidate.tag, candidate.player)
        || tick(mem)?.wrapping_sub(candidate.tick) > 1
    {
        return None;
    }
    swap(mem, tag, player, candidate.object)
}

fn prompt(mem: &dyn GuestMemory, user: u32, candidates: &Candidates) -> Option<Vec<u8>> {
    let tag = current_hud(mem)?;
    let player = player_for_user(mem, user)?;
    let (_, unit) = controlled_unit(mem, player)?;
    let (template, arguments) = if let Some(vehicle) = exit_subject(mem, tag, unit) {
        (EXIT_VEHICLE, [vehicle, u16::MAX])
    } else {
        let swap = current_swap(mem, tag, player, candidates[player_slot(player)?])?;
        (SWAP_POWERUP, swap.arguments)
    };
    events::render(mem, tag, mem.u16(tag + template).ok()?, arguments)
        .ok()
        .flatten()
}

fn show(mem: &mut dyn GuestMemory, user: u32, text: &[u8]) -> Result<(), MemFault> {
    let hud = mem.u32(HUD)? + 0x4e0 * user;
    // The inline message fields written by 0x24ca1d and the visibility
    // transition fields maintained by 0x24c98c.
    mem.write(hud + TEXT, text)?;
    mem.set_u32(hud + TEXT_ID, 0)?;
    if mem.u8(hud + 0x444)? == 0 {
        mem.set_u8(hud + 0x44e, mem.u8(hud + 0x44e)? | 1)?;
    }
    mem.set_u8(hud + 0x44f, 1)?;
    mem.set_u8(hud + 0x444, 1)
}

pub(super) fn register(b: &mut Builder) {
    let candidates: Arc<Mutex<Candidates>> = Arc::new(Mutex::new([None; PLAYER_LIMIT]));
    let scan = Arc::clone(&candidates);
    b.raw()
        .before(
            &PLAYER_PICKUP_SCAN,
            "start a fresh powerup scan",
            move |c| {
                let mem = c.ctx.memory();
                if let Some(slot) = word(&*mem, c.cpu.esp + 4).and_then(player_slot) {
                    scan.lock().unwrap()[slot] = None;
                }
                Ok(Flow::Continue)
            },
        )
        .context_only();
    let scan = Arc::clone(&candidates);
    b.raw()
        .before(
            &PLAYER_PICKUP_CANDIDATE,
            "select an in-range powerup swap",
            move |c| {
                let sp = c.cpu.esp;
                let mem = c.ctx.memory();
                if let Some((slot, candidate)) = (|| {
                    let player = word(&*mem, sp + 4)?;
                    let slot = player_slot(player)?;
                    let tag = current_hud(&*mem)?;
                    let object = word(&*mem, sp + 8)?;
                    let swap = swap(&*mem, tag, player, object)?;
                    Some((
                        slot,
                        Candidate {
                            tag,
                            player,
                            tick: tick(&*mem)?,
                            object,
                            distance: swap.distance,
                        },
                    ))
                })() {
                    let mut scan = scan.lock().unwrap();
                    if scan[slot].is_none_or(|previous| {
                        (previous.tag, previous.player, previous.tick)
                            != (candidate.tag, candidate.player, candidate.tick)
                            || previous.distance > candidate.distance
                    }) {
                        scan[slot] = Some(candidate);
                    }
                }
                Ok(Flow::Continue)
            },
        )
        .context_only();
    let hud_candidates = Arc::clone(&candidates);
    b.raw()
        .before(
            &HUD_INTERACTION_VISIBLE,
            "show a contextual Halo 1 prompt",
            move |c| {
                let (user, sp) = (c.cpu.eax, c.cpu.esp);
                let shown = {
                    let mut mem = c.ctx.memory();
                    user < 4
                        && word(&*mem, sp) == Some(FROM_INTERACTION)
                        && prompt(&*mem, user, &hud_candidates.lock().unwrap())
                            .is_some_and(|text| show(&mut *mem, user, &text).is_ok())
                };
                if shown {
                    return c.ret(4).map(Flow::Return);
                }
                Ok(Flow::Continue)
            },
        )
        .context_only();
    b.raw().before(
        &PLAYER_PRESS_INTERACT,
        "swap the selected powerup",
        move |c| {
            let selected = (|| {
                let mem = c.ctx.memory();
                let player = c.cpu.eax;
                let action = word(&*mem, c.cpu.esp + 4)?;
                if action == 0 || mem.u16(action).ok()? != 0 {
                    return None;
                }
                let tag = current_hud(&*mem)?;
                let slot = player_slot(player)?;
                let swap = current_swap(&*mem, tag, player, candidates.lock().unwrap()[slot])?;
                let (unit, body) = controlled_unit(&*mem, player)?;
                Some((unit, body, swap))
            })();
            let Some((unit, body, swap)) = selected else {
                return Ok(Flow::Continue);
            };
            c.answer_async(move |cx| {
                Box::pin(async move {
                    cx.call(
                        GuestCall::new(ATTACH_OBJECT)
                            .eax(0)
                            .args([unit, 0, swap.target]),
                    )
                    .await?;
                    let attached =
                        cx.read(|mem| word(mem, swap.offered + OBJECT_PARENT) == Some(unit));
                    if !attached {
                        return Ok(Ret::new(0, 4));
                    }
                    cx.call(GuestCall::new(HIDE_OBJECT).eax(swap.target).args([1]))
                        .await?;
                    if cx.read(|mem| {
                        object(mem, swap.held, Some(EQUIPMENT))
                            .is_some_and(|held| word(mem, held + OBJECT_PARENT) == Some(unit))
                    }) {
                        cx.call(GuestCall::new(DETACH_OBJECT).eax(swap.held))
                            .await?;
                    }
                    cx.call(GuestCall::new(HIDE_OBJECT).eax(swap.held).args([0]))
                        .await?;
                    cx.write(|mem| mem.set_u32(body + UNIT_EQUIPMENT, swap.target))?;
                    Ok(Ret::new(1, 4))
                })
            })
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;
    use guest_layout::points::SCENARIO;
    use guest_layout::tags::{TAG_ENTRIES_GLOBAL, TAG_INDEX_GLOBAL, index_layout};
    use h1_contract::hud::{GROUP, SCENARIO_HMT, SHARED_MESSAGES, SIZE};

    const TAG: u32 = 0x0045_1000;
    const USER: u32 = 1;
    const PLAYER: u32 = 0x8001_0000;
    const UNIT: u32 = 0x8002_0000;
    const VEHICLE_DATUM: u32 = 0x8002_0001;
    const HELD: u32 = 0x8002_0002;
    const NEARBY: u32 = 0x8002_0003;
    const UNIT_BODY: u32 = 0x0045_2000;
    const TARGET_BODY: u32 = 0x0045_2c00;
    const TARGET_DEF: u32 = 0x0045_3c00;
    const HUD_SCREENS: u32 = 0x0046_0000;
    const TIME: u32 = 0x0045_1480;

    fn fixture() -> FlatMemory {
        // Up to the tag index global at 0x547f00.
        let mut mem = FlatMemory::new(0x0044_0000, 0x0011_0000);
        let scenario = 0x0045_0000;
        let index = 0x0045_1800;
        let entries = 0x0045_1900;
        let users = 0x0045_1400;
        let players = 0x0045_1500;
        let player = 0x0045_1600;
        let objects = 0x0045_1a00;
        let headers = 0x0045_1b00;
        let parts = 0x0045_1100;
        let rows = 0x0045_1200;
        let blob = 0x0045_1400 - 0x80;
        let subjects = 0x0045_1450;
        for (at, value) in [
            (SCENARIO.base, scenario),
            (scenario + SCENARIO_HMT, GROUP.as_u32()),
            (scenario + SCENARIO_HMT + 4, 0x8003_0000),
            (TAG_INDEX_GLOBAL, index),
            (TAG_ENTRIES_GLOBAL, entries),
            (index + index_layout::TAG_COUNT, 4),
            (entries + entry_layout::GROUP, GROUP.as_u32()),
            (entries + entry_layout::ADDRESS, TAG),
            (entries + entry_layout::SIZE, SIZE),
            (TAG + MARKER_AT, MARKER),
            (TAG + SHARED_MESSAGES, 5 << 16),
            (TAG + PROMPT_SUBJECTS, 3),
            (TAG + PROMPT_SUBJECTS + 4, subjects),
            (TAG + 4, blob),
            (TAG + 8, 10),
            (TAG + 0xc, parts),
            (TAG + 0x10, 5),
            (TAG + 0x14, rows),
            (LOCAL_USERS, users),
            (hs_actions::PLAYER_CONTROL, 0x0045_4000),
            (users + 0xc + 4 * USER, PLAYER),
            (PLAYERS, players),
            (players + 0x38, 1),
            (players + 0x44, player),
            (player + 0x2c, UNIT),
            (OBJECTS, objects),
            (objects + 0x38, 4),
            (objects + 0x44, headers),
            (flash::GAME_TIME, TIME),
            (TIME + 8, 50),
            (UNIT_BODY + OBJECT_PARENT, NONE),
            (UNIT_BODY + UNIT_EQUIPMENT, HELD),
            (UNIT_BODY + 0x3c, 0.4f32.to_bits()),
            (0x0044_ae90, 0.3f32.to_bits()),
            (TARGET_BODY + OBJECT_PARENT, NONE),
            (TARGET_BODY + 0x30, 0.5f32.to_bits()),
            (TARGET_BODY + 0x14c, NONE),
            (HUD, HUD_SCREENS),
        ] {
            mem.set_u32(at, value).unwrap();
        }
        mem.set_u16(player, 0x8001).unwrap();
        mem.set_u16(UNIT_BODY + UNIT_SEAT, u16::MAX).unwrap();
        mem.set_u16(TAG + EXIT_VEHICLE, 0).unwrap();
        mem.set_u16(TAG + SWAP_POWERUP, 1).unwrap();
        let mut text = Vec::new();
        for (i, (first, count, run)) in [
            (0, 3, " exit "),
            (3, 4, " for "),
            (7, 1, "Warthog"),
            (8, 1, "Overshield"),
            (9, 1, "Camouflage"),
        ]
        .into_iter()
        .enumerate()
        {
            let row = rows + i as u32 * 0x40;
            mem.set_u16(row + 0x20, (text.len() / 2) as u16).unwrap();
            mem.set_u16(row + 0x22, first).unwrap();
            mem.set_u8(row + 0x24, count).unwrap();
            text.extend(run.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
        }
        mem.set_u32(TAG, text.len() as u32).unwrap();
        mem.write(blob, &text).unwrap();
        mem.write(
            parts,
            &[
                1, 18, 0, 7, 1, 32, 1, 18, 1, 32, 0, 6, 1, 33, 0, 8, 0, 11, 0, 11,
            ],
        )
        .unwrap();
        for (n, (body, kind, def)) in [
            (UNIT_BODY, 0, 0),
            (0x0045_2400, VEHICLE, 0x0045_3000),
            (0x0045_2800, EQUIPMENT, 0x0045_3400),
            (TARGET_BODY, EQUIPMENT, TARGET_DEF),
        ]
        .into_iter()
        .enumerate()
        {
            let n = n as u32;
            let slot = headers + n * 0xc;
            mem.set_u16(slot, 0x8002).unwrap();
            mem.set_u8(slot + 3, kind).unwrap();
            mem.set_u32(slot + 8, body).unwrap();
            let id = 0x8003_0000 + n;
            mem.set_u32(entries + n * 0x10 + entry_layout::DATUM, id)
                .unwrap();
            if n != 0 {
                mem.set_u32(entries + n * 0x10 + entry_layout::ADDRESS, def)
                    .unwrap();
                mem.set_u32(body, id).unwrap();
                let subject = subjects + (n - 1) * PROMPT_SUBJECT_SIZE;
                mem.set_u32(subject, id).unwrap();
                mem.set_u16(subject + 4, (n + 1) as u16).unwrap();
            }
        }
        mem.set_u32(0x0045_3000 + 0x1c8, 2).unwrap();
        mem.set_u16(0x0045_3400 + 0x12c, 2).unwrap();
        mem.set_u16(TARGET_DEF + 0x12c, 3).unwrap();
        mem
    }

    #[test]
    fn vehicle_exit_names_the_parent_and_requires_a_valid_seat() {
        let mut mem = fixture();
        mem.set_u32(UNIT_BODY + OBJECT_PARENT, VEHICLE_DATUM)
            .unwrap();
        let candidates = [None; PLAYER_LIMIT];
        assert!(prompt(&mem, USER, &candidates).is_none());
        mem.set_u16(UNIT_BODY + UNIT_SEAT, 1).unwrap();
        let text = prompt(&mem, USER, &candidates).unwrap();
        show(&mut mem, USER, &text).unwrap();
        let screen = HUD_SCREENS + 0x4e0 * USER;
        assert_eq!(
            mem.utf16(screen + TEXT, 32).unwrap(),
            "\u{e448} exit Warthog"
        );
        assert_eq!(mem.u8(screen + 0x444).unwrap(), 1);
        assert_eq!(mem.u8(HUD_SCREENS + 0x444).unwrap(), 0);
        let users = mem.u32(LOCAL_USERS).unwrap();
        mem.set_u8(users + 6, 1).unwrap();
        assert!(prompt(&mem, USER, &candidates).is_none());
        mem.set_u8(users + 6, 0).unwrap();
        let control = mem.u32(hs_actions::PLAYER_CONTROL).unwrap();
        for (capture, held) in [(1, 1), (0, 1)] {
            mem.set_u32(control + hs_actions::CAPTURE, capture).unwrap();
            mem.set_u32(control + hs_actions::UNTIL_RELEASE, held).unwrap();
            assert!(prompt(&mem, USER, &candidates).is_none());
        }
        mem.set_u32(control + hs_actions::UNTIL_RELEASE, 0).unwrap();
        assert!(prompt(&mem, USER, &candidates).is_some());
        mem.set_u32(UNIT_BODY + OBJECT_PARENT, NONE).unwrap();
        assert!(prompt(&mem, USER, &candidates).is_none());
    }

    #[test]
    fn powerup_prompt_requires_live_different_equipment_and_a_fresh_in_range_target() {
        let mut mem = fixture();
        let mut candidates = [None; PLAYER_LIMIT];
        candidates[0] = Some(Candidate {
            tag: TAG,
            player: PLAYER,
            tick: 50,
            object: NEARBY,
            distance: 0.25,
        });
        let utf16: Vec<_> = "\u{e448}Overshield for Camouflage\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(prompt(&mem, USER, &candidates), Some(utf16));
        mem.set_u32(TIME + 8, 52).unwrap();
        assert!(prompt(&mem, USER, &candidates).is_none());
        mem.set_u32(TIME + 8, 50).unwrap();
        mem.set_u16(TARGET_DEF + 0x12c, 2).unwrap();
        assert!(prompt(&mem, USER, &candidates).is_none());
        mem.set_u16(TARGET_DEF + 0x12c, 6).unwrap();
        assert!(prompt(&mem, USER, &candidates).is_none());
        mem.set_u16(TARGET_DEF + 0x12c, 3).unwrap();
        mem.set_u32(TARGET_BODY + 0x30, 0.8f32.to_bits()).unwrap();
        assert!(prompt(&mem, USER, &candidates).is_none());
        mem.set_u32(TARGET_BODY + 0x30, 0.5f32.to_bits()).unwrap();
        mem.set_u32(UNIT_BODY + UNIT_EQUIPMENT, NONE).unwrap();
        assert!(prompt(&mem, USER, &candidates).is_none());
    }
}
