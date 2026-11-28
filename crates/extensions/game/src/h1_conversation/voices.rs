//! Authored voice banks and checkpointed per-unit voice state.

use hs::h1_conversation::spawn::*;

use super::*;

const STRINGS: u32 = 0x1b0;
const DEFS: u32 = 0x1c4;
const NODES: u32 = 0x238;
const NODE_BYTES: u32 = 20;

/// The temporary table covers units made while the scenario's HaloScript
/// globals are still being allocated. It is drained after script map init.
pub(super) const BOOTSTRAP_BYTES: u32 = 8 + VOICE_RECORDS * 8;

#[derive(Clone, Copy, Default)]
pub(super) struct Profile {
    pub voice: i16,
    pub leader: bool,
}

impl Profile {
    fn read(bits: u32) -> Self {
        Self {
            voice: bits as i16,
            leader: bits & 0x1_0000 != 0,
        }
    }

    fn bits(self) -> u32 {
        u32::from(self.voice as u16) | (u32::from(self.leader) << 16)
    }
}

#[derive(Clone, Copy)]
pub(super) struct Registry {
    pub state: u32,
    pub squads: u32,
    scenario: u32,
    at: u32,
    end: u32,
    banks: u32,
}

#[derive(Clone, Copy)]
pub(super) struct Squad {
    pub encounter: u16,
    pub rule: u16,
    bank: u16,
    starts: u16,
    count: u16,
}

impl Registry {
    pub fn read(m: &dyn GuestMemory) -> Result<Option<Self>, MemFault> {
        let scenario = m.u32(points::SCENARIO.base)?;
        if scenario == 0 || m.u32(scenario + STRINGS)? < HEADER as u32 {
            return Ok(None);
        }
        let at = m.u32(scenario + STRINGS + 4)?;
        if at == 0 || m.u32(at + 4)? != SIGNATURE {
            return Ok(None);
        }
        let state = m.u32(at + 8)?;
        let banks = m.u32(at + 12)?;
        let squads = m.u32(at + 16)?;
        let n = m.u32(scenario + STRINGS)?;
        if banks > GLOBAL_CAPACITY
            || squads > u32::from(u16::MAX)
            || squads > m.u32(scenario + 0x160)?
            || state.saturating_add(VOICE_RECORDS * 2 - 1) >= m.u32(scenario + 0x1c0)?
            || HEADER as u32 + BANK_BYTES as u32 * banks + SQUAD_BYTES as u32 * squads > n
        {
            return Ok(None);
        }
        Ok(Some(Self {
            scenario,
            at,
            end: at.saturating_add(n),
            state,
            banks,
            squads,
        }))
    }

    pub fn squad(self, m: &dyn GuestMemory, index: u16) -> Result<Option<Squad>, MemFault> {
        if u32::from(index) >= self.squads {
            return Ok(None);
        }
        let at = self.at
            + HEADER as u32
            + self.banks * BANK_BYTES as u32
            + u32::from(index) * SQUAD_BYTES as u32;
        Ok(Some(Squad {
            encounter: m.u16(at)?,
            rule: m.u16(at + 2)?,
            bank: m.u16(at + 4)?,
            starts: m.u16(at + 6)?,
            count: m.u16(at + 8)?,
        }))
    }

    pub fn spawn_bank(
        self,
        m: &dyn GuestMemory,
        squad: Squad,
        start: u16,
    ) -> Result<u16, MemFault> {
        if start >= squad.count {
            return Ok(squad.bank);
        }
        let at = self.at
            + HEADER as u32
            + self.banks * BANK_BYTES as u32
            + self.squads * SQUAD_BYTES as u32
            + 2 * (u32::from(squad.starts) + u32::from(start));
        if at.saturating_add(2) > self.end {
            return Ok(NO_BANK);
        }
        m.u16(at)
    }

    pub fn bank_for_tag(self, m: &dyn GuestMemory, tag: u32) -> Result<u16, MemFault> {
        let defs = m.u32(self.scenario + DEFS)?;
        let nodes = m.u32(self.scenario + NODES + 4)?;
        let count = m.u32(self.scenario + NODES)?;
        if defs == 0 || nodes == 0 || tag == NONE {
            return Ok(NO_BANK);
        }
        for i in 0..self.banks {
            let entry = self.at + HEADER as u32 + i * BANK_BYTES as u32;
            let global = u32::from(m.u16(entry)?);
            if global >= m.u32(self.scenario + 0x1c0)? {
                continue;
            }
            let root = m.u32(defs + global * 0x28 + 0x24)?;
            let slot = root & 0xffff;
            if slot < count && m.u32(nodes + slot * NODE_BYTES + 0x10)? == tag {
                return Ok(i as u16);
            }
        }
        Ok(NO_BANK)
    }

    fn bank(self, m: &dyn GuestMemory, index: u16) -> Result<(u32, u32), MemFault> {
        if u32::from(index) >= self.banks {
            return Ok((0, 0));
        }
        let at = self.at + HEADER as u32 + u32::from(index) * BANK_BYTES as u32;
        Ok((at + 4, u32::from(m.u16(at + 2)?).min(16)))
    }
}

#[derive(Clone, Copy)]
pub(super) enum Table {
    Globals(u32),
    Pending(u32),
}

impl Table {
    pub fn current(m: &dyn GuestMemory, r: Registry, pending: u32) -> Result<Self, MemFault> {
        if m.u32(pending)? == r.at {
            Ok(Self::Globals(r.state))
        } else {
            Ok(Self::Pending(pending))
        }
    }

    fn pair(self, m: &dyn GuestMemory, index: u32) -> Result<Option<(u32, u32)>, MemFault> {
        match self {
            Self::Globals(base) => {
                let i = base.saturating_add(2 * index);
                Ok(global(m, i, LONG)?.zip(global(m, i.saturating_add(1), LONG)?))
            }
            Self::Pending(pending) => {
                let key = pending + 8 + 8 * index;
                Ok(Some((key, key + 4)))
            }
        }
    }

    pub fn get(self, m: &dyn GuestMemory, unit: u32) -> Result<Profile, MemFault> {
        for index in probe(unit) {
            let Some((key, value)) = self.pair(m, index)? else {
                break;
            };
            let occupant = m.u32(key)?;
            if occupant == unit {
                return Ok(Profile::read(m.u32(value)?));
            }
            if occupant == 0 {
                break;
            }
        }
        Ok(Profile::default())
    }

    pub fn put(self, m: &mut dyn GuestMemory, unit: u32, profile: Profile) -> Result<(), MemFault> {
        let mut reusable = None;
        for index in probe(unit) {
            let Some((key, value)) = self.pair(m, index)? else {
                break;
            };
            let occupant = m.u32(key)?;
            if occupant == unit {
                return m.set_u32(value, profile.bits());
            }
            if occupant == 0 {
                reusable.get_or_insert((key, value));
                break;
            }
            if reusable.is_none() && object_address(m, occupant)?.is_none() {
                reusable = Some((key, value));
            }
        }
        if profile.bits() == 0 {
            return Ok(());
        }
        let Some((key, value)) = reusable else {
            return Err(MemFault {
                addr: unit,
                len: 0,
                write: true,
            });
        };
        m.set_u32(value, profile.bits())?;
        m.set_u32(key, unit)
    }
}

fn probe(unit: u32) -> impl Iterator<Item = u32> {
    (0..VOICE_RECORDS).map(move |i| (u32::from(unit as u16) + i) % VOICE_RECORDS)
}

pub(super) fn bind(
    m: &mut dyn GuestMemory,
    r: Registry,
    table: Table,
    counter: u32,
    unit: u32,
    bank: u16,
    leader_voice: i16,
) -> Result<(), MemFault> {
    let voice = if leader_voice != 0 {
        leader_voice
    } else {
        let (entries, count) = r.bank(m, bank)?;
        if count == 0 {
            0
        } else {
            let index = m.u32(counter)?;
            m.set_u32(counter, index.wrapping_add(1))?;
            read_i16(m, entries + (index % count) * 2)?
        }
    };
    table.put(
        m,
        unit,
        Profile {
            voice,
            leader: leader_voice >= 100,
        },
    )
}

pub(super) fn init(m: &mut dyn GuestMemory, pending: u32) -> Result<(), MemFault> {
    let Some(r) = Registry::read(m)? else {
        return Ok(());
    };
    let table = Table::Globals(r.state);
    if table.pair(m, 0)?.is_none() {
        return Ok(());
    }
    for i in 0..VOICE_RECORDS {
        let (key, value) = Table::Pending(pending).pair(m, i)?.unwrap();
        let unit = m.u32(key)?;
        if unit != 0 && object_address(m, unit)?.is_some() {
            let profile = Profile::read(m.u32(value)?);
            table.put(m, unit, profile)?;
        }
    }
    m.fill(pending + 8, BOOTSTRAP_BYTES - 8, 0)?;
    m.set_u32(pending, r.at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::datum::layout;
    use guest_layout::testing::FakeGuest;

    const PENDING: u32 = 0x1000;
    const STR: u32 = 0x2000;
    const SCENARIO: u32 = 0x3000;
    const DEFINITIONS: u32 = 0x18000;
    const SYNTAX: u32 = 0x1e000;
    const GLOBALS: u32 = 0x4000;
    const ROWS: u32 = 0x10000;
    const OBJECTS: u32 = 0x20000;
    const OBJECT_ROWS: u32 = 0x21000;
    const BODY: u32 = 0x22000;

    fn fixture() -> FakeGuest {
        let mut m = FakeGuest::new();
        let bank = STR + HEADER as u32;
        m.map(PENDING, BOOTSTRAP_BYTES)
            .put_u32(points::SCENARIO.base, SCENARIO)
            .put_u32(SCENARIO + STRINGS, (HEADER + BANK_BYTES) as u32)
            .put_u32(SCENARIO + STRINGS + 4, STR)
            .put_u32(SCENARIO + 0x1c0, 2 * VOICE_RECORDS)
            .put_u32(STR + 4, SIGNATURE)
            .put_u32(STR + 12, 1)
            .put_u16(bank + 2, 3)
            .put_u16(bank + 4, 2)
            .put_u16(bank + 6, 4)
            .put_u16(bank + 8, 1);
        m
    }

    #[test]
    fn generic_voices_cycle_at_birth_and_leaders_do_not_advance_the_cycle() {
        let mut m = fixture();
        let r = Registry::read(&m).unwrap().unwrap();
        let table = Table::Pending(PENDING);
        for (index, expected) in [2, 100, 4, 1, 2].into_iter().enumerate() {
            let unit = 0x8400_0000 | index as u32;
            let leader = if index == 1 { 100 } else { 0 };
            bind(&mut m, r, table, PENDING + 4, unit, 0, leader).unwrap();
            let profile = table.get(&m, unit).unwrap();
            assert_eq!((profile.voice, profile.leader), (expected, leader != 0));
        }
        assert_eq!(m.u32(PENDING + 4).unwrap(), 4);
    }

    #[test]
    fn a_placed_units_tag_resolves_before_script_globals_initialize() {
        let mut m = fixture();
        let tag = 0x8100_0042;
        m.put_u32(SCENARIO + DEFS, DEFINITIONS)
            .put_u32(SCENARIO + NODES, 2)
            .put_u32(SCENARIO + NODES + 4, SYNTAX)
            .put_u32(DEFINITIONS + 0x24, 0x8700_0001)
            .put_u32(SYNTAX + NODE_BYTES + 0x10, tag);
        let r = Registry::read(&m).unwrap().unwrap();
        assert_eq!(r.bank_for_tag(&m, tag).unwrap(), 0);
        assert_eq!(r.bank_for_tag(&m, 0x8101_0042).unwrap(), NO_BANK);
    }

    #[test]
    fn early_leader_follows_the_unit_through_migration_before_its_first_cast() {
        let mut m = fixture();
        let unit = 0x8400_0001;
        let r = Registry::read(&m).unwrap().unwrap();
        let table = Table::current(&m, r, PENDING).unwrap();
        bind(&mut m, r, table, PENDING + 4, unit, 0, 101).unwrap();

        m.put_u32(points::OBJECTS.base, OBJECTS)
            .put_u32(OBJECTS + layout::DATA, OBJECT_ROWS)
            .put_u32(OBJECTS + layout::HIGH_WATER, 2)
            .put_u32(OBJECTS + layout::ELEMENT_SIZE, 12)
            .put_u16(OBJECT_ROWS + 12, 0x8400)
            .put_u32(OBJECT_ROWS + 20, BODY)
            .put_u32(BODY + super::super::cast::UNIT_ACTOR, 0x9600_0003)
            .put_u32(SCENARIO + DEFS, DEFINITIONS)
            .put_u32(GLOBAL_VALUES, GLOBALS)
            .put_u32(GLOBALS + layout::DATA, ROWS)
            .put_u32(
                GLOBALS + layout::HIGH_WATER,
                SCENARIO_GLOBAL_OFFSET + 2 * VOICE_RECORDS,
            )
            .put_u32(GLOBALS + layout::ELEMENT_SIZE, 8);
        for i in 0..2 * VOICE_RECORDS {
            m.put_u16(DEFINITIONS + i * 0x28 + 0x20, LONG)
                .put_u32(ROWS + (SCENARIO_GLOBAL_OFFSET + i) * 8, 0x8000)
                .put_u32(ROWS + (SCENARIO_GLOBAL_OFFSET + i) * 8 + 4, 0);
        }
        init(&mut m, PENDING).unwrap();
        m.put_u32(BODY + super::super::cast::UNIT_ACTOR, 0x9700_0006);
        let table = Table::current(&m, r, PENDING).unwrap();
        let profile = table.get(&m, unit).unwrap();
        assert_eq!((profile.voice, profile.leader), (101, true));
        assert_eq!(table.get(&m, 0x8401_0001).unwrap().voice, 0);
    }
}
