//! A populated entry E: the cluster's MOPP ray, each terminal's mark and
//! the nearest instance hit, answered in Rust ([`populated`]).
//! [`ReturnPrediction`] folds the same plan onto the entry bytes as the
//! differential tests' oracle, so a write to a missed placement, padding
//! or an unvisited mark is visible there.
use super::*;

const RECORD_BYTES: usize = 0x5c;
const MARK_STATE: u32 = MARK_BASE - 8; // active byte, padding, generation, slots
const MARK_BYTES: u32 = MARK_BASE - MARK_STATE + 4 * MARK_SLOTS;

struct Word {
    at: u32,
    value: u32,
}

impl Word {
    fn read(g: &impl GuestRead, at: u32) -> Result<Self, Bail> {
        Ok(Self {
            at,
            value: g.u32(at)?,
        })
    }
}

pub struct ReturnPrediction {
    al: u8,
    record_at: u32,
    record: [u8; RECORD_BYTES],
    marks: Vec<u8>,
    refcount: Word,
    handler: Word,
    sp: u32,
    saved: [u32; 4],
}

/// What a populated E writes, decided before anything is written.
struct Plan {
    /// The return address.
    target: u32,
    record_at: u32,
    record: [u8; RECORD_BYTES],
    marks: Vec<u8>,
    prediction: Prediction,
    refcount: Word,
    handler: Word,
}

fn plan(c: &CpuAbi, g: &mut impl CollisionMemory, world: &CollisionWorld) -> Result<Plan, Bail> {
    if g.trapped() {
        return Err(Bail::Trap);
    }
    let m = Read(&*g, (0, 0));
    let structure = m.u32(layout::ACTIVE_STRUCTURE)?;
    let cluster = cluster_at(&m, structure, c.eax)?;
    if (m.u32(cluster.wrapping_add(CODE_COUNT))? as i32) <= 0 {
        return Err(Bail::Data);
    }
    let code = m.u32(cluster.wrapping_add(0xac))?;
    let refcount = Word::read(&m, code.wrapping_add(mopp::REFCOUNT))?;
    let handler = Word::read(&m, c.fs_base)?;
    let arg = |i: u32| m.u32(c.esp.wrapping_add(i * 4));
    let target = arg(0)?;
    let record_at = arg(3)?;
    let mut record = [0; RECORD_BYTES];
    m.read(record_at, &mut record)?;
    let q = Query {
        cluster: c.eax,
        start: m.vec3(c.esi)?,
        delta: m.vec3(c.edi)?,
        flags: arg(1)?,
        filter: arg(2)?,
        limit: u32::from_le_bytes(record[4..8].try_into().unwrap()),
    };
    let marks = m.bytes(MARK_STATE, MARK_BYTES as usize)?;
    let epoch = u32::from_le_bytes(marks[4..8].try_into().unwrap());

    // A recursive ray frame can reach entry C from the terminal.
    // The caller can put its result above the consumed arguments.
    let aligned = c.esp.checked_sub(4).ok_or(Bail::Alias)? & !15;
    let depth = 0x710 + 0x1c0 * mopp::MAX_FRAMES + layout::VECTOR_DEAD_BELOW;
    let mut guarded = super::super::query::Guard::with_frame(
        g,
        aligned.saturating_sub(depth),
        c.esp,
        &[
            (c.esp, 16),
            (record_at, RECORD_BYTES as u32),
            (c.fs_base, 4),
            (MARK_STATE, MARK_BYTES),
        ],
    )?;
    for (at, len) in [(refcount.at, 4), (c.esi, 12), (c.edi, 12)] {
        guarded.separate(at, len)?;
    }
    // Read the marks only through their own table. A BSP, MOPP,
    // transform or scalar that aliases a mark declines, including a
    // source retained in the world from an earlier call.
    let p = predict(c, &mut guarded, world, q, epoch, &[], |_, key| {
        let offset = (MARK_BASE - MARK_STATE + key * 4) as usize;
        Ok(marks[offset..offset + 4] == epoch.to_le_bytes())
    });
    if guarded.alias() {
        return Err(Bail::Alias);
    }
    Ok(Plan {
        target,
        record_at,
        record,
        marks,
        prediction: p?,
        refcount,
        handler,
    })
}

/// Entry E with a positive code count, natively: the terminals' marks in
/// visit order, the nearest hit's fields in the caller's result, `al`
/// whether any terminal hit, `ret 0xc`. The callee-saved registers, the
/// code's reference count and `fs:[0]` are as they were at entry; the
/// lifted shape constructor's transient writes to them are elided.
pub fn populated(
    c: &mut CpuAbi,
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
) -> Result<Option<TransferAbi>, GuestFault> {
    let plan = plan(c, g, world);
    world.record(Entry::InstanceTrace, &plan);
    let Ok(p) = plan else {
        return Ok(None);
    };
    for &(at, epoch) in &p.prediction.marks {
        g.set_u32(at, epoch)?;
    }
    let hit = p.prediction.hit.is_some();
    if let Some(h) = &p.prediction.hit {
        let mut wrote = Ok(());
        h.writes(|offset, bytes| {
            if wrote.is_ok() {
                wrote = g.write_bytes(p.record_at.wrapping_add(offset), bytes, 1);
            }
        });
        wrote?;
    }
    c.eax = (c.eax & !0xff) | u32::from(hit);
    Ok(Some(finish(c, p.target, 12)))
}

impl ReturnPrediction {
    pub fn gather(
        c: &CpuAbi,
        g: &mut impl CollisionMemory,
        world: &CollisionWorld,
    ) -> Result<Self, Bail> {
        let Plan {
            record_at,
            mut record,
            mut marks,
            prediction: p,
            refcount,
            handler,
            ..
        } = plan(c, g, world)?;
        for (at, value) in p.marks {
            let offset = (at - MARK_STATE) as usize;
            marks[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        if let Some(hit) = &p.hit {
            hit.writes(|offset, value| {
                let start = offset as usize;
                record[start..start + value.len()].copy_from_slice(value);
            });
        }
        Ok(Self {
            al: u8::from(p.hit.is_some()),
            record_at,
            record,
            marks,
            refcount,
            handler,
            sp: c.esp.wrapping_add(16),
            saved: [c.ebx, c.ebp, c.esi, c.edi],
        })
    }

    pub fn compare(&self, c: &CpuAbi, g: &impl CollisionMemory) -> Result<Option<String>, Bail> {
        if g.trapped() {
            return Err(Bail::Trap);
        }
        for (name, expected, actual) in [
            ("AL", self.al as u32, c.eax & 255),
            ("ESP", self.sp, c.esp),
            ("EBX", self.saved[0], c.ebx),
            ("EBP", self.saved[1], c.ebp),
            ("ESI", self.saved[2], c.esi),
            ("EDI", self.saved[3], c.edi),
            ("FS base", self.handler.at, c.fs_base),
        ] {
            if expected != actual {
                return Ok(Some(format!(
                    "{name}: expected {expected:#x}, guest {actual:#x}"
                )));
            }
        }
        let m = Read(g, (0, 0));
        for (name, at, expected) in [
            ("result", self.record_at, self.record.as_slice()),
            ("instance marks", MARK_STATE, self.marks.as_slice()),
        ] {
            let actual = m.bytes(at, expected.len())?;
            if let Some(i) = expected.iter().zip(&actual).position(|(a, b)| a != b) {
                return Ok(Some(format!(
                    "{name}+{i:#x}: expected {:#04x}, guest {:#04x}",
                    expected[i], actual[i]
                )));
            }
        }
        for (name, word) in [("code refcount", &self.refcount), ("FS:[0]", &self.handler)] {
            let actual = m.u32(word.at)?;
            if word.value != actual {
                return Ok(Some(format!(
                    "{name}: expected {:#x}, guest {actual:#x}",
                    word.value
                )));
            }
        }
        Ok(None)
    }
}
