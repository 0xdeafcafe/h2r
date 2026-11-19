//! A phantom is local user zero in a private copy of the host world.
//! Only its pad history, player-control history and returned record survive.
use anyhow::Result;
use bots::phantom::{SeatInput, session::at::ACTION_SIZE};
use runtime::{CpuAbi, Runtime, executor::SecondExecutor};
use std::collections::BTreeMap;

pub(super) const WORK_BYTES: u32 = 0x10000;
const RAW: u32 = 0x4e61dc;
const OBSERVER: u32 = 0x4e9c8c;
const DIRECTOR: u32 = 0x4e8c44;

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
}

impl Builder {
    pub(super) fn new(main: &Runtime, workspace: u32) -> Self {
        let exec = SecondExecutor::new(main, |_, _| false);
        exec.runtime().edit_dispatch(|d| {
            d.prefer_generated();
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
        Self {
            exec,
            workspace,
            tick: None,
            history: Default::default(),
        }
    }

    pub(super) fn action(
        &mut self,
        main: &Runtime,
        cpu: CpuAbi,
        s: SeatInput,
    ) -> Result<[u8; ACTION_SIZE]> {
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
        let idx = s.player.index();
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
            m.write(0x54e9f4, [0, 0, 0, 0, 0, 0, bots::guest::BOT_SENSITIVITY])?;
            if let Some(h) = self.history.get(&idx).filter(|_| !s.reset) {
                m.write(RAW, h.raw)?;
                m.write(ctrl, h.control)?;
                m.write(0x51ea88, h.layout)?;
                m.set_u8(0x51ebcc, h.latch)?;
            } else {
                m.write(RAW, [0; 0x48])?;
                m.write(0x51ea88, [0; 0x50])?;
                m.set_u8(0x51ebcc, 0)?;
                m.set_u32(ctrl, u32::MAX)?;
            }
        }
        let stack = work + WORK_BYTES - 16;
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
        self.history.insert(
            idx,
            History {
                raw: m.read(RAW)?,
                control: m.read(ctrl)?,
                layout: m.read(0x51ea88)?,
                latch: m.u8(0x51ebcc)?,
            },
        );
        Ok(m.read(out)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::DatumHandle;
    use runtime::{Dispatch, Guest, guest::Permissions};

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
        let player = DatumHandle::new(21, 5);
        let unit = DatumHandle::new(50, 9);
        let mut d = Dispatch::default();
        for address in [0x1872d0, 0x190c34, 0x1557c0, 0x124c00, 0x217c40, 0x186ab0] {
            d.register_generated_rust(address, move |cpu, ctx, _| {
                let mut m = ctx.memory();
                match address {
                    0x1872d0 => {
                        assert_eq!((cpu.eax, cpu.ecx), (0, unit.raw()));
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
                        assert_eq!(m.u32(CONTROLS + 0x14)?, unit.raw());
                        assert_eq!(m.u32(DIRECTOR + 12)?, 0x23cbb0);
                        assert_eq!(m.u32(OBSERVER)?, 0x11223344);
                        let out = m.u32(cpu.esp + 16)?;
                        let history = m.u8(RAW)?;
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
                builder.action(&main, CpuAbi::default(), seat).unwrap(),
                [count; ACTION_SIZE]
            );
        }
        let m = main.memory();
        assert_eq!(m.u32(PLAYER + 0x24).unwrap(), u32::MAX);
        assert_eq!(m.u32(USERS + 12).unwrap(), 0x1234);
        assert_eq!(m.u8(RAW).unwrap(), 0);
    }
}
