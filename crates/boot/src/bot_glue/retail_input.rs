//! A phantom is local user zero in a private copy of the host world.
//! Only its pad history, player-control history and returned record survive.
use anyhow::Result;
use bots::phantom::{SeatInput, session::at::ACTION_SIZE};
use runtime::{
    CpuAbi, Runtime,
    executor::{ExecutorError, SecondExecutor},
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

mod animation;
use animation::AnimationRead;

pub(super) const WORK_BYTES: u32 = 0x1000;
const RAW: u32 = 0x4e61dc;
const OBSERVER: u32 = 0x4e9c8c;
const DIRECTOR: u32 = 0x4e8c44;
const ANIMATION_RESOURCE_REQUEST: u32 = 0x1236f0;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum BuildFailure {
    Refused(u32),
    Fault,
    Cancelled,
    Stack,
    NoSnapshot,
    Memory,
    ColdAnimation,
}

impl BuildFailure {
    fn kind(err: &anyhow::Error) -> Self {
        match err.downcast_ref::<ExecutorError>() {
            Some(ExecutorError::Refused(r)) => Self::Refused(r.target),
            Some(ExecutorError::Fault { .. }) => Self::Fault,
            Some(ExecutorError::Cancelled) => Self::Cancelled,
            Some(ExecutorError::Stack(_)) => Self::Stack,
            Some(ExecutorError::NoSnapshot) => Self::NoSnapshot,
            None => Self::Memory,
        }
    }
}

struct History {
    raw: [u8; 0x48],
    control: [u8; 0x94],
    layout: [u8; 0x50],
    latch: u8,
}

pub(super) struct Builder {
    exec: SecondExecutor,
    workspace: u32,
    tick: Option<u32>,
    history: BTreeMap<u16, History>,
    reported: BTreeSet<BuildFailure>,
    cold: animation::ColdRead,
}

/// Only the live animation cache may submit or wait for a resource read.
/// Retry the pad against fresh RAM once after a synchronous cache fill.
pub(super) async fn gather(
    input: &Mutex<Option<Builder>>,
    main: &Runtime,
    workspace: u32,
    cpu: CpuAbi,
    seat: SeatInput,
    cx: &engine::entry::Cx,
    film: Option<&dyn film::tape::Tap>,
) -> Option<[u8; ACTION_SIZE]> {
    let attempt = |refresh| {
        const OP: &str = "Bot.BuildAction";
        let args = [
            seat.tick,
            seat.player.raw(),
            seat.unit.raw(),
            u32::from(refresh),
        ];
        if let Some(film) = film.filter(|film| film.replaying()) {
            let Some(call) = film.next(OP, &args, 0) else {
                return Ok(None);
            };
            match (call.result, call.outputs.as_slice()) {
                (Ok(0), []) => return Ok(None),
                (Ok(1), [data]) if data.len() == ACTION_SIZE => {
                    return Ok(data.as_slice().try_into().ok());
                }
                (Ok(2), [data]) if data.len() == 12 => {
                    return Err(AnimationRead::from_film(
                        data.as_slice().try_into().unwrap(),
                    ));
                }
                _ => {
                    film.fault("malformed phantom action build".into());
                    return Ok(None);
                }
            }
        }

        let mut guard = input.lock().unwrap_or_else(|e| e.into_inner());
        let builder = guard.get_or_insert_with(|| Builder::new(main, workspace));
        if refresh {
            builder.tick = None;
        }
        let outcome = builder.sample(main, cpu, seat);
        if let Some(film) = film {
            let (kind, outputs) = match &outcome {
                Ok(None) => (0, vec![]),
                Ok(Some(bytes)) => (1, vec![bytes.to_vec()]),
                Err(cold) => (2, vec![cold.for_film().to_vec()]),
            };
            film.record(
                film::tape::BTAP,
                film::tape::Call {
                    index: 0,
                    op: OP.into(),
                    args: args.to_vec(),
                    thread: 0,
                    result: Ok(kind),
                    outputs,
                },
            );
        }
        outcome
    };
    let read = match attempt(false) {
        Ok(action) => return action,
        Err(read) => read,
    };
    if let Some(call) = cx.read(|m| read.live_call(m)) {
        cx.call(call).await.ok()?;
        match attempt(true) {
            Ok(action) => return action,
            Err(read) => report_cold(input, read, seat),
        }
    } else {
        report_cold(input, read, seat);
    }
    None
}

fn report_cold(input: &Mutex<Option<Builder>>, read: AnimationRead, seat: SeatInput) {
    let mut guard = input.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(builder) = guard.as_mut()
        && builder.reported.insert(BuildFailure::ColdAnimation)
    {
        eprintln!(
            "phantoms: missed action for player {} at tick {}: animation resource {:#x} unavailable",
            seat.player.index(),
            seat.tick,
            read.record
        );
    }
}

impl Builder {
    fn new(main: &Runtime, workspace: u32) -> Self {
        let exec = SecondExecutor::new(main, |_, _| false);
        exec.runtime().edit_dispatch(|d| {
            d.prefer_generated();
            // Weapon and animation lookups predict resources after choosing an
            // animation. Queuing I/O here would post a job to a private queue
            // with no worker. The prediction returns void and takes no stack
            // arguments. Actual resource reads still require resident data.
            d.register_override(
                ANIMATION_RESOURCE_REQUEST,
                "phantom: skip animation prefetch",
                |cpu, ctx, _| ctx.memory().ret(cpu, 0),
            );
            d.register_override(
                xapi::register::addr::XINPUT_GET_STATE,
                "seat XInput report",
                move |cpu, ctx, _| {
                    let mut m = ctx.memory();
                    let out = m.u32(cpu.esp + 8)?;
                    let report = m.read::<{ input::State::SIZE }>(workspace)?;
                    m.write(out, report)?;
                    cpu.eax = 0;
                    m.ret(cpu, 8)
                },
            );
        });
        let cold = animation::intercept_cold_reads(exec.runtime());
        Self {
            exec,
            workspace,
            tick: None,
            history: Default::default(),
            reported: BTreeSet::new(),
            cold,
        }
    }

    /// An isolated failure is a missing controller sample at the gather hook.
    fn sample(
        &mut self,
        main: &Runtime,
        cpu: CpuAbi,
        seat: SeatInput,
    ) -> Result<Option<[u8; ACTION_SIZE]>, AnimationRead> {
        match self.action(main, cpu, seat) {
            Ok(action) => Ok(Some(action)),
            Err(err) => {
                let cold = self.cold.lock().unwrap_or_else(|e| e.into_inner()).take();
                if cold.is_none() && self.reported.insert(BuildFailure::kind(&err)) {
                    let trace = self
                        .exec
                        .refused_cpu()
                        .map(|at| self.exec.context().backtrace(at.eip));
                    eprintln!(
                        "phantoms: missed action for player {} at tick {}: {err:#} (private call chain {trace:x?})",
                        seat.player.index(),
                        seat.tick
                    );
                }
                // Abandon the private stack and its callbacks. The next seat
                // starts from live RAM, even if it gathers in this same tick.
                self.exec.context().clear_continuations();
                self.exec.context().clear_backtrace();
                self.tick = None;
                match cold {
                    Some(read) => Err(read),
                    None => Ok(None),
                }
            }
        }
    }

    fn action(&mut self, main: &Runtime, cpu: CpuAbi, s: SeatInput) -> Result<[u8; ACTION_SIZE]> {
        let idx = s.player.index();
        if s.reset {
            self.history.remove(&idx);
        }
        if self.tick != Some(s.tick) || s.reset {
            self.exec.snapshot(main);
            self.tick = Some(s.tick);
        } else {
            // Each seat sees the same world, even when the search wrote scratch globals.
            for page in self.exec.written() {
                self.exec.revert(page.physical, runtime::guest::PAGE_SIZE);
            }
        }
        let work = self.workspace;
        let ctrl;
        {
            let mut m = self.exec.runtime().memory();
            let users = m.u32(0x4e8c20)?;
            m.set_u32(users + 8, 1)?;
            m.write(users + 12, [0xff; 32])?;
            m.set_u32(users + 12, s.player.raw())?;
            m.set_u32(users + 0x1c, s.player.raw())?;
            m.set_u32(s.player_addr + 0x24, 0)?;
            m.set_u16(s.player_addr + 0x28, 0)?;
            m.write(0x4e61cc, [0; 16])?;
            m.set_u32(0x4e61cc, 1)?;
            m.set_u8(0x4e61b9, 0)?;
            m.write(work, s.pad.to_bytes())?;
            ctrl = m.u32(0x4ed284)? + 0x14;
            // A gameplay director, no menu input or observer belonging to another player.
            m.write(DIRECTOR, [0; 0x140])?;
            m.set_u16(0x4686c4, 0)?;
            // The phantom has no open dialogs. Clear the port and global screen
            // lists checked by 0x148fff in this view only.
            for list in [0x54d884, 0x54d76c, 0x54d62c, 0x54d99c, 0x54dba8, 0x54d9e0] {
                m.write(list + 8, [0; 8])?;
            }
            // On a playerless host slot zero has no profile or local-user index.
            // Supply the same profile binding as a signed-in bot in the private view.
            let slot = bots::guest::slot(0);
            m.set_u32(slot, bots::guest::SLOT_PROFILE)?;
            m.set_u32(slot + bots::guest::SLOT_LOCAL_USER, 0)?;
            m.write(0x54e9f4, [0, 0, 0, 0, 0, 0, bots::guest::BOT_SENSITIVITY])?;
            if let Some(h) = self.history.get(&idx) {
                m.write(RAW, h.raw)?;
                m.write(ctrl, h.control)?;
                m.write(0x51ea88, h.layout)?;
                m.set_u8(0x51ebcc, h.latch)?;
            } else {
                m.write(RAW, [0; 0x48])?;
                m.write(0x51ea88, [0; 0x50])?;
                m.set_u8(0x51ebcc, 0)?;
                // Discard user zero's previous shot-assist block
                // before retail initializes control for the newly bound unit.
                let assist = ctrl + crate::touch_glue::PC_TARGET;
                let mut empty = [0; 0x24];
                empty[..12].fill(0xff); // object, marker and controlling player
                m.write(assist, empty)?;
                m.set_u32(ctrl, u32::MAX)?;
            }
        }
        // The COW view includes the calling game thread's stack and FS. The
        // guest calls can push below the gather frame without touching it.
        let stack = cpu.esp;
        let call = |exec: &mut SecondExecutor, addr, eax, ecx, args: &[u32]| -> Result<u32> {
            let mut regs = CpuAbi {
                esp: stack,
                eax,
                ecx,
                ..cpu
            };
            exec.call(&mut regs, addr, args)?;
            Ok(regs.eax)
        };
        call(&mut self.exec, 0x1872d0, 0, s.unit.raw(), &[])?; // bind/reset control if unit changed
        call(&mut self.exec, 0x190c34, 0, 0, &[])?; // profile -> look rates and Default layout
        let director = call(
            &mut self.exec,
            0x1557c0,
            s.unit.raw(),
            0,
            &[OBSERVER, work + 0x20],
        )?;
        self.exec.runtime().memory().set_u32(
            DIRECTOR + 12,
            if director as u16 == 0 {
                0x23c110
            } else {
                0x23cbb0
            },
        )?;
        // Exactly the reader's milliseconds, including the fractional tick over time.
        let ms = |t: u64| (t as f64 * f64::from(s.dt) * 1000.).round() as u64;
        let elapsed = ms(u64::from(s.tick) + 1).saturating_sub(ms(u64::from(s.tick))) as u32;
        call(&mut self.exec, 0x124c00, 0, 0, &[elapsed])?;
        call(&mut self.exec, 0x217c40, 0, 0, &[])?;
        let out = work + 0x40;
        call(
            &mut self.exec,
            0x186ab0,
            0,
            0,
            &[s.player.raw(), s.dt.to_bits(), s.dt.to_bits(), out],
        )?;
        let m = self.exec.runtime().memory();
        let action = m.read(out)?;
        self.history.insert(
            idx,
            History {
                raw: m.read(RAW)?,
                control: m.read(ctrl)?,
                layout: m.read(0x51ea88)?,
                latch: m.u8(0x51ebcc)?,
            },
        );
        Ok(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::DatumHandle;
    use runtime::{Dispatch, Guest, guest::Permissions};

    #[test]
    fn private_prefetch_skips_io_and_a_refused_wait_discards_only_the_sample() {
        const WORK: u32 = 0x560000;
        const PLAYER: u32 = 0x550800;
        let mut g = Guest::new();
        g.map_zeroed(0x120000, 0x100000, Permissions::RX).unwrap();
        g.map_zeroed(0x460000, 0x110000, Permissions::RW).unwrap();
        g.set_u32(0x4e8c20, 0x550000).unwrap();
        g.set_u32(0x4ed284, 0x550100).unwrap();
        let mut d = Dispatch::default();
        for address in [0x1872d0, 0x190c34, 0x1557c0, 0x124c00, 0x217c40] {
            d.register_generated_rust(address, |cpu, ctx, _| ctx.memory().ret(cpu, 0))
                .unwrap();
        }
        d.register_generated_rust(ANIMATION_RESOURCE_REQUEST, |_, _, _| {
            panic!("animation prediction reached the I/O queue")
        })
        .unwrap();
        d.register_generated_rust(0x186ab0, |cpu, ctx, _| {
            let mut m = ctx.memory();
            let next = m.u8(RAW)? + 1;
            m.set_u8(RAW, next)?;
            let out = m.u32(cpu.esp + 16)?;
            m.write(out, [next; ACTION_SIZE])?;
            let ordinal = m.u32(WORK)?;
            if ordinal != 0 {
                return m.call(cpu, runtime::kernel::function_token(ordinal), 0x2d6307);
            }
            Ok(runtime::TransferAbi::jump(ANIMATION_RESOURCE_REQUEST))
        })
        .unwrap();
        let main = Runtime::new(g, d);
        let mut builder = Builder::new(&main, WORK);
        let cpu = CpuAbi {
            esp: WORK + 0xfff0,
            ..Default::default()
        };
        // A complete build, repeated refusals after the private writes, and a
        // good sample. A failed reset must also discard the old unit's history.
        for (tick, reset, ordinal, expected) in [
            (1, true, 0, Some(1)),
            (2, false, 234, None),
            (3, false, 234, None),
            (4, false, 0, Some(2)),
            (5, true, 234, None),
            (6, false, 0, Some(1)),
            (7, false, 128, None),
        ] {
            let sample = SeatInput {
                player: DatumHandle::new(21, 5),
                player_addr: PLAYER,
                unit: DatumHandle::new(if tick < 5 { 50 } else { 51 }, 9),
                tick,
                reset,
                dt: 1. / 30.,
                pad: input::State {
                    packet_number: ordinal,
                    ..Default::default()
                },
            };
            assert_eq!(
                builder.sample(&main, cpu, sample).unwrap(),
                expected.map(|n| [n; ACTION_SIZE])
            );
            assert_eq!(main.memory().u8(RAW).unwrap(), 0);
            assert_eq!(
                main.memory().read::<ACTION_SIZE>(WORK + 0x40).unwrap(),
                [0; ACTION_SIZE]
            );
        }
        assert_eq!(
            builder.reported,
            BTreeSet::from([
                BuildFailure::Refused(runtime::kernel::function_token(234)),
                BuildFailure::Refused(runtime::kernel::function_token(128)),
            ])
        );
    }

    // Stand-ins at the traced entries check the register/stack contract. The
    // retail code itself comes from the coordinator's compiled game image.
    #[test]
    fn retail_input_binds_the_requested_seat_and_keeps_its_history_private() {
        const USERS: u32 = 0x550000;
        const CONTROLS: u32 = USERS + 0x100;
        const PLAYER: u32 = USERS + 0x400;
        const WORK: u32 = 0x560000;
        let mut g = Guest::new();
        g.map_zeroed(0x120000, 0x100000, Permissions::RX).unwrap();
        g.map_zeroed(0x460000, 0x110000, Permissions::RW).unwrap();
        g.set_u32(0x4e8c20, USERS).unwrap();
        g.set_u32(0x4ed284, CONTROLS).unwrap();
        g.set_u32(USERS + 12, 0x1234).unwrap();
        g.set_u32(PLAYER + 0x24, u32::MAX).unwrap();
        g.set_u32(
            bots::guest::slot(0) + bots::guest::SLOT_LOCAL_USER,
            u32::MAX,
        )
        .unwrap();
        let target = CONTROLS + 0x14 + crate::touch_glue::PC_TARGET;
        let strength = CONTROLS + 0x14 + crate::touch_glue::PC_CONE;
        g.set_u32(target, 0xdead0002).unwrap();
        g.set_u32(strength, 0.875f32.to_bits()).unwrap();
        let player = DatumHandle::new(21, 5);
        let unit = DatumHandle::new(50, 9);
        let mut d = Dispatch::default();
        for address in [0x1872d0, 0x190c34, 0x1557c0, 0x124c00, 0x217c40, 0x186ab0] {
            d.register_generated_rust(address, move |cpu, ctx, _| {
                let mut m = ctx.memory();
                match address {
                    0x1872d0 => {
                        assert_eq!((cpu.eax, cpu.ecx), (0, unit.raw()));
                        if m.u32(CONTROLS + 0x14)? == u32::MAX {
                            assert_eq!(m.u32(target)?, u32::MAX);
                            assert_eq!(m.u32(strength)?, 0);
                        }
                        m.set_u32(CONTROLS + 0x14, cpu.ecx)?;
                    }
                    0x1557c0 => {
                        assert_eq!(cpu.eax, unit.raw());
                        m.set_u32(OBSERVER, 0x11223344)?;
                        cpu.eax = 1;
                    }
                    0x124c00 => {
                        assert!((33..=34).contains(&m.u32(cpu.esp + 4)?));
                        let history = m.u8(RAW)? + 1;
                        m.set_u8(RAW, history)?;
                    }
                    0x186ab0 => {
                        assert_eq!(m.u32(cpu.esp + 4)?, player.raw());
                        assert_eq!(m.u32(USERS + 12)?, player.raw());
                        assert_eq!(m.u32(USERS + 0x1c)?, player.raw());
                        assert_eq!(m.u32(PLAYER + 0x24)?, 0);
                        assert_eq!(m.u32(bots::guest::slot(0))?, bots::guest::SLOT_PROFILE);
                        assert_eq!(
                            m.u32(bots::guest::slot(0) + bots::guest::SLOT_LOCAL_USER)?,
                            0
                        );
                        assert_eq!(m.u32(CONTROLS + 0x14)?, unit.raw());
                        assert_eq!(m.u32(DIRECTOR + 12)?, 0x23cbb0);
                        assert_eq!(m.u32(OBSERVER)?, 0x11223344);
                        let out = m.u32(cpu.esp + 16)?;
                        let history = m.u8(RAW)?;
                        // This seat acquired a target on the previous sample.
                        // Its retained assist is independent of the host's.
                        if history > 1 {
                            assert_eq!(m.u32(target)?, 0xe0010004);
                            assert_eq!(m.u32(strength)?, 0.5f32.to_bits());
                        }
                        m.set_u32(target, 0xe0010004)?;
                        m.set_u32(strength, 0.5f32.to_bits())?;
                        m.write(out, [history; ACTION_SIZE])?;
                    }
                    _ => (),
                }
                m.ret(cpu, 0)
            })
            .unwrap();
        }
        let main = Runtime::new(g, d);
        let mut builder = Builder::new(&main, WORK);
        for (tick, reset, count) in [(1, true, 1), (2, false, 2), (9, true, 1)] {
            let seat = SeatInput {
                player,
                player_addr: PLAYER,
                unit,
                tick,
                dt: 1. / 30.,
                pad: Default::default(),
                reset,
            };
            assert_eq!(
                builder
                    .action(
                        &main,
                        CpuAbi {
                            esp: WORK + 0xffff - 15,
                            ..Default::default()
                        },
                        seat
                    )
                    .unwrap(),
                [count; ACTION_SIZE]
            );
        }
        let m = main.memory();
        assert_eq!(m.u32(PLAYER + 0x24).unwrap(), u32::MAX);
        assert_eq!(m.u32(USERS + 12).unwrap(), 0x1234);
        assert_eq!(m.u8(RAW).unwrap(), 0);
        assert_eq!(m.u32(target).unwrap(), 0xdead0002);
        assert_eq!(m.u32(strength).unwrap(), 0.875f32.to_bits());
        assert_eq!(m.u32(bots::guest::slot(0)).unwrap(), 0);
        assert_eq!(
            m.u32(bots::guest::slot(0) + bots::guest::SLOT_LOCAL_USER)
                .unwrap(),
            u32::MAX
        );
    }
}

/// Image-backed counterpart to the synthetic dispatch and transport tests.
/// A local multiplayer checkpoint supplies real player, BSP and weapon tags.
#[cfg(test)]
#[test]
#[ignore = "requires a built retail module and a local multiplayer checkpoint with an opponent under the reticle"]
fn retail_pad_sequence_matches_local_angles_target_and_fire() -> Result<()> {
    use engine::DatumHandle;
    use runtime::guest::Permissions;
    use std::path::PathBuf;

    let file = |key| {
        std::env::var_os(key)
            .map(PathBuf::from)
            .ok_or_else(|| anyhow::anyhow!("missing {key}"))
    };
    let xbe = crate::read_xbe(&file("H2_DISC")?)?;
    let module = file("H2_RETAIL_MODULE")?;
    let bytes = std::fs::read(file("H2_PAD_CHECKPOINT")?)?;
    let header = snapshot::read_header(&bytes)?;
    let saved = snapshot::Snapshot::open(&bytes, &header.build)?;
    let mut loaded = runtime::load_xbe_with(&xbe, &runtime::loader::LoadOptions::halo2())?;
    let kernel = kernel::Kernel::for_image(&mut loaded)?;
    let main = &loaded.runtime;
    // SAFETY: H2_RETAIL_MODULE is the coordinator's generated module for this XBE.
    let _module = unsafe { crate::load_module(&module, main, &xbe)? };
    snapshot::guest::load_guest(&saved, &mut main.memory(), None)?;
    const WORK: u32 = 0xa3fe0000;
    let (player, player_addr, unit) = {
        let mut m = main.memory();
        m.map_zeroed(WORK, 0x10000, Permissions::RW)?;
        let user = m.u32(0x4e8c20)?;
        let player = DatumHandle(m.u32(user + 12)?);
        anyhow::ensure!(!player.is_none(), "fixture needs local user zero");
        let array = m.u32(0x4e8c24)?;
        let addr = m.u32(array + 0x44)? + 0x21c * u32::from(player.index());
        let unit = DatumHandle(m.u32(addr + 0x2c)?);
        anyhow::ensure!(
            m.u32(addr + 0x24)? == 0 && !unit.is_none(),
            "fixture needs a spawned player on controller zero"
        );
        m.write(RAW, [0; 0x48])?;
        m.write(0x51ea88, [0; 0x50])?;
        m.set_u8(0x51ebcc, 0)?;
        let controls = m.u32(0x4ed284)?;
        m.set_u32(controls + 0x14, u32::MAX)?;
        m.write(0x54e9f4, [0, 0, 0, 0, 0, 0, bots::guest::BOT_SENSITIVITY])?;
        (player, addr, unit)
    };
    let cpu = CpuAbi {
        esp: WORK + 0xfff0,
        fs_base: kernel.kpcr(),
        ..CpuAbi::xbox_thread()
    };
    let mut local = SecondExecutor::new(main, |_, _| false);
    local.runtime().edit_dispatch(|d| {
        d.prefer_generated();
        d.register_override(
            xapi::register::addr::XINPUT_GET_STATE,
            "fixture controller",
            |cpu, ctx, _| {
                let mut m = ctx.memory();
                let report = m.read::<{ input::State::SIZE }>(WORK)?;
                let dest = m.u32(cpu.esp + 8)?;
                m.write(dest, report)?;
                cpu.eax = 0;
                m.ret(cpu, 8)
            },
        );
    });
    local.snapshot(main);
    // The reference retains the game's actual local tables. The other side
    // must bind this same world actor starting from a remote player record.
    {
        let mut m = main.memory();
        m.set_u32(player_addr + 0x24, u32::MAX)?;
        m.set_u16(player_addr + 0x28, u16::MAX)?;
        let slot = bots::guest::slot(0);
        m.set_u32(slot, 0)?;
        m.set_u32(slot + bots::guest::SLOT_LOCAL_USER, u32::MAX)?;
        let users = m.u32(0x4e8c20)?;
        m.set_u32(users + 12, u32::MAX)?;
        m.set_u32(users + 0x1c, u32::MAX)?;
    }
    let mut remote = Builder::new(main, WORK);
    let dt = 1.0f32 / 30.0;
    let mut acquired = false;
    let mut fired = false;
    let mut angles = Vec::new();
    for tick in 0..30 {
        let mut pad = input::State {
            packet_number: tick,
            ..Default::default()
        };
        pad.gamepad.thumb_rx = if tick < 10 {
            0
        } else if tick < 22 {
            19000
        } else {
            -22000
        };
        pad.gamepad.thumb_ry = if tick < 10 { 0 } else { 11000 };
        pad.gamepad.analog[input::AnalogButton::RightTrigger as usize] =
            if tick % 6 < 3 { 255 } else { 0 };
        local.runtime().memory().write(WORK, pad.to_bytes())?;
        let mut call = |pc, eax, ecx, args: &[u32]| -> Result<()> {
            local.call(&mut CpuAbi { eax, ecx, ..cpu }, pc, args)?;
            Ok(())
        };
        if tick == 0 {
            call(0x1872d0, 0, unit.raw(), &[])?;
            call(0x190c34, 0, 0, &[])?;
        }
        call(0x124c00, 0, 0, &[if tick % 3 == 1 { 34 } else { 33 }])?;
        call(0x217c40, 0, 0, &[])?;
        call(
            0x186ab0,
            0,
            0,
            &[player.raw(), dt.to_bits(), dt.to_bits(), WORK + 0x40],
        )?;
        let expected = local.runtime().memory().read::<ACTION_SIZE>(WORK + 0x40)?;
        let action = remote.action(
            main,
            cpu,
            SeatInput {
                player,
                player_addr,
                unit,
                tick,
                dt,
                pad,
                reset: tick == 0,
            },
        )?;
        assert_eq!(action, expected, "retail action at sample {tick}");
        let word = |off: usize| u32::from_le_bytes(action[off..off + 4].try_into().unwrap());
        acquired |= word(0x34) != u32::MAX
            && word(0x3c) != u32::MAX
            && (f32::from_bits(word(0x50)) > 0. || f32::from_bits(word(0x54)) > 0.);
        fired |= action[0] & 0x40 != 0;
        angles.push(action[4..12].to_vec());
    }
    assert!(fired && angles.windows(2).any(|p| p[0] != p[1]));
    assert!(
        acquired,
        "fixture needs a visible opposing player within weapon assist range"
    );
    Ok(())
}
