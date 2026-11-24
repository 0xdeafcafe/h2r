//! Plan at J's entry, compare the guest answer after J returns. A plan
//! owns its expected bytes, including the incoming result record's padding.
use super::*;
use std::{cell::RefCell, collections::HashMap};

pub struct Prediction {
    plan: Plan,
    out: u32,
    flags_at: u32,
}

impl Prediction {
    pub fn gather(
        cpu: &CpuAbi,
        g: &mut impl CollisionMemory,
        world: &CollisionWorld,
    ) -> Result<Self, Bail> {
        let m = Read(&*g, (0, 0));
        let arg = |i: u32| m.u32(cpu.esp.wrapping_add(4 * i));
        let flags = arg(1)?;
        let (start, delta) = (arg(2)?, arg(3)?);
        let ignore = [arg(4)?, arg(5)?];
        let out = arg(6)?;
        // The caller can pass a live output on its own stack. The frame J
        // reserves below its entry, and the consumed arguments above, must
        // remain separate from the start, delta and output for a prediction.
        let outs = super::super::Outputs::new(
            g,
            cpu.esp,
            0x454,
            [(cpu.esp.wrapping_add(4), 0x18), (out, RECORD_SIZE as u32)],
        )?;
        outs.separate(g, start, 12)?;
        outs.separate(g, delta, 12)?;
        let ray = Ray {
            start: m.vec3(start)?,
            delta: m.vec3(delta)?,
            flags,
            ignore,
        };
        let mut initial = Record([0; RECORD_SIZE]);
        m.read(out, &mut initial.0)?;
        let p = plan(cpu, g, world, ray, initial)?;
        p.marks
            .expected(|at, v| outs.separate(g, at, v.len() as u32))?;
        Ok(Self {
            plan: p,
            out,
            flags_at: cpu.esp.wrapping_add(4),
        })
    }

    pub fn compare(&self, al: u8, g: &impl CollisionMemory) -> Result<Option<String>, Bail> {
        let m = Read(g, (0, 0));
        let expected_al = u8::from(self.plan.record.word(0) != 0);
        if al != expected_al {
            return Ok(Some(format!("AL: expected {expected_al}, guest {al}")));
        }
        let flags = m.u32(self.flags_at)?;
        if flags != self.plan.flags {
            return Ok(Some(format!(
                "flags: expected {:#010x}, guest {flags:#010x}",
                self.plan.flags
            )));
        }
        let mut actual = [0; RECORD_SIZE];
        m.read(self.out, &mut actual)?;
        if let Some(i) = actual
            .iter()
            .zip(&self.plan.record.0)
            .position(|(a, b)| a != b)
        {
            return Ok(Some(format!(
                "result+{i:#x}: expected {:#04x}, guest {:#04x}",
                self.plan.record.0[i], actual[i]
            )));
        }
        let mut first = None;
        self.plan.marks.expected(|at, expected| {
            if first.is_none() {
                let mut actual = [0; 4];
                let used = &mut actual[..expected.len()];
                m.read(at, used)?;
                if used != expected {
                    first = Some(format!(
                        "mark at {at:#x}: expected {expected:02x?}, guest {used:02x?}"
                    ));
                }
            }
            Ok(())
        })?;
        Ok(first)
    }
}

thread_local! {
    static OPEN:RefCell<HashMap<u32, Vec<(u32,Prediction)>>> = RefCell::default();
}

pub fn begin(thread: u32, cpu: &CpuAbi, g: &mut runtime::Guest, world: &CollisionWorld) {
    let ret_sp = cpu.esp.wrapping_add(4 + 0x18);
    let predicted = if g.has_trap() {
        Err(Bail::Trap)
    } else {
        Prediction::gather(cpu, g, world)
    };
    OPEN.with_borrow_mut(|threads| {
        let stack = threads.entry(thread).or_default();
        // Discard returns abandoned by a reverted or unwound guest frame.
        stack.retain(|(outer, _)| *outer > ret_sp);
        match predicted {
            Ok(p) => stack.push((ret_sp, p)),
            Err(why) => world.record_query(Err(why)),
        }
        if stack.is_empty() {
            threads.remove(&thread);
        }
    });
}

pub fn end(thread: u32, cpu: &CpuAbi, g: &runtime::Guest, world: &CollisionWorld) {
    OPEN.with_borrow_mut(|threads| {
        let Some(stack) = threads.get_mut(&thread) else {
            return;
        };
        if !stack.last().is_some_and(|(sp, _)| *sp == cpu.esp) {
            return;
        }
        let (_, p) = stack.pop().unwrap();
        if stack.is_empty() {
            threads.remove(&thread);
        }
        world.record_query(if g.has_trap() {
            Err(Bail::Trap)
        } else {
            p.compare(cpu.eax as u8, g)
        });
    });
}
