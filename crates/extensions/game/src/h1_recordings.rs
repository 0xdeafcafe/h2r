//! Apply converted single-weapon recordings with Halo 2's unit control
//! function. Only the recording player's call (0x1fb630) can enter here.

use guest_layout::DatumHandle;
use guest_layout::entry::{Cx, EntryFuture, EntryResult, GuestCall, Ret};
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points::UNIT_APPLY_CONTROL;
use guest_layout::view::objects;
use hooks::{BuildError, Builder, Feature, Flow, Manifest, SnapshotPolicy};
use map::recording;

const PLAYBACK_RETURN: u32 = 0x001f_b635;
const UNIT_ACTION: u32 = 0x000e_68c0;
const THROW_GRENADE: u32 = 1 << 26;
const CONTROL_SIZE: usize = 0x7c; // 31 words zeroed at 0xc6ef5

/// Prepare the control packet the native leaf consumes. Keep the playback datum's
/// decoded state separately, so releasing an override restores its latest
/// recorded mode and facing even if both changed while it was held.
fn native_control(
    m: &dyn GuestMemory,
    body: objects::Body,
    kind: u8,
    bytes: &mut [u8; CONTROL_SIZE],
) -> Result<u32, MemFault> {
    let flags = u32::from_le_bytes(bytes[0x10..0x14].try_into().unwrap());
    let mode = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    let mode = if flags & recording::FORCE_ALERT != 0 {
        recording::COMBAT
    } else {
        recording::expand_mode(mode)
    };
    bytes[..4].copy_from_slice(&mode.to_le_bytes());
    bytes[0x10..0x14].copy_from_slice(&(flags & !recording::HINTS).to_le_bytes());
    // H1 0x3b3d3 treats -1 as no slot request. H2 0xc6e2b writes
    // both requested slots on generation equality. Keep the other hand.
    let generation = m.u16(body.addr.wrapping_add(0x214))?;
    let generation = if bytes[8] == 0xff {
        !generation
    } else {
        generation
    };
    bytes[6..8].copy_from_slice(&generation.to_le_bytes());
    bytes[9] = m.u8(body.addr.wrapping_add(0x217))?;
    if kind == objects::BIPED && flags & recording::HOLD_FACING != 0 {
        // The biped compares control facing (+0x150) to object forward
        // (+0x70) at 0xe02de..0xe0494. Its aiming/looking stay independent.
        m.read(
            body.addr.wrapping_add(objects::Body::FORWARD),
            &mut bytes[0x28..0x34],
        )?;
    }
    Ok(flags)
}

async fn apply(cx: Cx) -> EntryResult {
    let regs = cx.entry();
    let Some((kind, body)) = cx.read(|m| objects::get(m, DatumHandle(regs.eax)))? else {
        return Ok(Ret::new(0, 0));
    };
    let mut native = [0; CONTROL_SIZE];
    let flags = cx.read(|m| {
        m.read(regs.ecx, &mut native)?;
        native_control(m, body, kind, &mut native)
    })?;
    // Pass a private copy on this guest call's stack. GuestCall pushes the
    // argument words immediately below ESP, then the return address. ECX
    // addresses the first word. The leaf ignores stack arguments and the
    // entry adapter restores ESP on return. The decoder's state stays intact.
    let packet = regs.esp.wrapping_sub(CONTROL_SIZE as u32);
    let words = native
        .as_chunks::<4>()
        .0
        .iter()
        .copied()
        .map(u32::from_le_bytes);
    let applied = cx
        .call(
            GuestCall::new(UNIT_APPLY_CONTROL.base)
                .eax(regs.eax)
                .ecx(packet)
                .args(words),
        )
        .await?;
    if flags & THROW_GRENADE != 0 {
        // The player path at 0x14fe58 sends a zeroed type-22 action. The
        // recording bypasses that path. 0xe68c0 constructs the same action.
        cx.call(GuestCall::new(UNIT_ACTION).ecx(0x16).args([regs.eax]))
            .await?;
    }
    if flags & recording::EXIT_SEAT != 0 && cx.read(|m| body.parent(m))?.is_some() {
        // Normal seat exit request, as on the host at 0x14fb55.
        cx.call(GuestCall::new(UNIT_ACTION).ecx(0x1d).args([regs.eax]))
            .await?;
    }
    Ok(Ret::new(applied.eax, 0))
}

pub struct Playback;

impl Feature for Playback {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "h1-recordings",
            "Apply converted campaign recording controls",
        )
        .doc("docs/halo1/campaign.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.raw()
            .before(&UNIT_APPLY_CONTROL, "apply a recorded unit control", |c| {
                {
                    let m = c.ctx.runtime().memory();
                    if m.u32(c.cpu.esp)? != PLAYBACK_RETURN
                        || m.u16(c.cpu.ecx.wrapping_add(6))? != recording::WEAPON_SET
                        || m.u8(c.cpu.ecx.wrapping_add(9))? != recording::SECOND_WEAPON
                    {
                        return Ok(Flow::Continue);
                    }
                }
                c.answer_async(|cx| -> EntryFuture { Box::pin(apply(cx)) })
            });
        b.snapshot(SnapshotPolicy::Stateless);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::entry::{Regs, testing::run};
    use guest_layout::testing::FakeGuest;
    use guest_layout::view::fake;

    #[test]
    fn converted_packet_reaches_the_native_unit_and_action_readers() {
        const BODY: u32 = 0x5000;
        const CONTROL: u32 = 0x7000;
        const STACK: u32 = 0x9000;
        let forward = [0x55; 12];
        let mode = (recording::FLEE as i8 as i32) as u32;
        let native_flags = THROW_GRENADE | (1 << 16);
        for (held, kind) in [
            (true, objects::BIPED),
            (true, objects::VEHICLE),
            (false, objects::BIPED),
        ] {
            let mut source = [0xab; CONTROL_SIZE];
            source[..4].copy_from_slice(&mode.to_le_bytes());
            source[6..8].copy_from_slice(&recording::WEAPON_SET.to_le_bytes());
            source[8] = if held { 1 } else { 0xff };
            source[9] = recording::SECOND_WEAPON;
            let flags = if held {
                recording::HINTS | native_flags
            } else {
                0
            };
            source[0x10..0x14].copy_from_slice(&flags.to_le_bytes());
            let mut guest = FakeGuest::new();
            let headers = fake::datums::<objects::Header>(&mut guest, 0x3000, 1);
            let (unit, header) = fake::push(&mut guest, &headers);
            header.set_kind(&mut guest, kind).unwrap();
            header.set_body(&mut guest, BODY).unwrap();
            let seated = kind == objects::BIPED;
            guest
                .put_u32(guest_layout::points::OBJECTS.base, 0x3000)
                .put(CONTROL, &source)
                .put(BODY + 0x70, &forward)
                .put_u32(BODY + 0x14, if seated { 1 } else { u32::MAX })
                .put_u16(BODY + 0x214, 17)
                .put_u8(BODY + 0x217, 2);
            let regs = Regs {
                eax: unit.raw(),
                ecx: CONTROL,
                esp: STACK,
                ..Regs::default()
            };
            let played = run(&|cx| Box::pin(apply(cx)), guest, regs, |call, _| Regs {
                eax: call.target,
                ..Regs::default()
            });
            assert_eq!(played.result, Ok(Ret::new(UNIT_APPLY_CONTROL.base, 0)));
            let copy = &played.calls[0];
            assert_eq!(copy.target, UNIT_APPLY_CONTROL.base);
            assert_eq!(copy.eax, Some(unit.raw()));
            assert_eq!(copy.ecx, Some(STACK - CONTROL_SIZE as u32));
            let packet: Vec<u8> = copy.args.iter().flat_map(|w| w.to_le_bytes()).collect();
            let mut expected = source;
            let pose = if held {
                recording::COMBAT
            } else {
                recording::FLEE
            };
            expected[..4].copy_from_slice(&pose.to_le_bytes());
            let generation = if held { 17u16 } else { !17u16 };
            expected[6..8].copy_from_slice(&generation.to_le_bytes());
            expected[9] = 2;
            expected[0x10..0x14]
                .copy_from_slice(&(if held { native_flags } else { 0 }).to_le_bytes());
            if held && kind == objects::BIPED {
                expected[0x28..0x34].copy_from_slice(&forward);
            }
            assert_eq!(packet[..], expected[..]);
            let actions: Vec<_> = played.calls[1..]
                .iter()
                .map(|call| {
                    assert_eq!(call.target, UNIT_ACTION);
                    assert_eq!(call.args, [unit.raw()]);
                    call.ecx.unwrap()
                })
                .collect();
            let expected_actions: &[u32] = match (held, seated) {
                (true, true) => &[0x16, 0x1d],
                (true, false) => &[0x16],
                _ => &[],
            };
            assert_eq!(actions.as_slice(), expected_actions);
            let mut decoder_state = [0; CONTROL_SIZE];
            played.guest.read(CONTROL, &mut decoder_state).unwrap();
            assert_eq!(decoder_state, source);
        }
    }
}
