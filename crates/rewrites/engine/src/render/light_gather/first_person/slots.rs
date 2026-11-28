//! The ordered table consumed by the first-person light loop. Rebuild it
//! from the dynamic passes while each light's source records still exist.
use super::{SLOT_COUNT, SLOT_LIGHT, SLOT_RECORDS, SLOTS, slot_base};
use guest_layout::memory::GuestMemory;
use crate::render::light_gather::{Refusal, Result, frame::FIRST_PERSON_COUNT, items};

type Record = [u8; items::RECORD as usize];

#[derive(Default)]
pub struct Slots(Vec<(u32, Vec<Record>)>);

impl Slots {
    pub fn handles(&self) -> Vec<u32> {
        self.0.iter().map(|(light, _)| *light).collect()
    }

    /// At the dynamic pass's constants call the item walk has returned
    /// and the record tail still includes its attachments.
    pub fn gather(&mut self, m: &dyn GuestMemory, light: u32) -> Result<()> {
        if self.0.len() < SLOTS as usize {
            self.0.push((light, Vec::new()));
        }
        let (_, records) = self.0.last_mut().expect("at least one light slot");
        if records.len() == SLOT_RECORDS as usize {
            return Ok(());
        }
        let start = m.u16(items::MAIN_COUNT)? as u32;
        let end = m.u16(items::COUNT)? as u32;
        if end > 0x8000 {
            return Err(Refusal::Layout);
        }
        for index in start..end {
            let at = items::record_at(index);
            if m.u32(at)? & (1 << 10) != 0 {
                let mut record = Record::default();
                m.read(at, &mut record)?;
                records.push(record);
                if records.len() == SLOT_RECORDS as usize {
                    break;
                }
            }
        }
        Ok(())
    }

    /// Compare slot order, including empty slots. The loop skips an
    /// empty slot; a nonempty slot is replayed even if its records are
    /// subsequently rejected by the current mode.
    pub fn first_difference(&self, m: &dyn GuestMemory) -> Result<Option<String>> {
        let count = m.i32(FIRST_PERSON_COUNT)?;
        if !(0..=SLOTS).contains(&count) {
            return Err(Refusal::Layout);
        }
        if self.0.len() != count as usize {
            return Ok(Some(format!(
                "slot count: Rust {}, guest {count}",
                self.0.len()
            )));
        }
        for (index, (light, records)) in self.0.iter().enumerate() {
            let base = slot_base(index as u32);
            let handle = m.u32(base + SLOT_LIGHT)?;
            if *light != handle {
                return Ok(Some(format!(
                    "slot {index}: Rust light {light:#010x}, guest {handle:#010x}"
                )));
            }
            let n = m.u16(base + SLOT_COUNT)? as i16;
            if !(0..=SLOT_RECORDS).contains(&n) {
                return Err(Refusal::Layout);
            }
            if n as usize != records.len() {
                return Ok(Some(format!(
                    "slot {index} record count: Rust {}, guest {n}",
                    records.len()
                )));
            }
            for (i, record) in records.iter().enumerate() {
                let mut guest = Record::default();
                m.read(base + i as u32 * items::RECORD, &mut guest)?;
                if *record != guest {
                    return Ok(Some(format!("slot {index} light {light:#010x} record {i}")));
                }
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    #[test]
    fn a_full_slot_ignores_later_records_and_a_missing_record_differs() {
        let mut m = FlatMemory::new(items::RECORDS, 0x6_0000);
        let mut slots = Slots::default();
        // Leave a gap between the invented records and the counts.
        let start = 4;
        let n = SLOT_RECORDS as u32;
        m.set_u16(items::MAIN_COUNT, start as u16).unwrap();
        m.set_u16(items::COUNT, (start + n + 1) as u16).unwrap();
        for i in 0..=n {
            m.set_u32(items::record_at(start + i), 1 << 10).unwrap();
            m.set_u32(items::record_at(start + i) + 4, 70 + i).unwrap();
            if i < n {
                m.set_u32(slot_base(0) + i * items::RECORD, 1 << 10)
                    .unwrap();
                m.set_u32(slot_base(0) + i * items::RECORD + 4, 70 + i)
                    .unwrap();
            }
        }
        slots.gather(&m, 27).unwrap();
        m.set_i32(FIRST_PERSON_COUNT, 1).unwrap();
        m.set_u32(slot_base(0) + SLOT_LIGHT, 27).unwrap();
        m.set_u16(slot_base(0) + SLOT_COUNT, n as u16).unwrap();
        assert_eq!(slots.first_difference(&m).unwrap(), None);
        m.set_u16(slot_base(0) + SLOT_COUNT, (n - 1) as u16)
            .unwrap();
        assert_eq!(
            slots.first_difference(&m).unwrap().as_deref(),
            Some("slot 0 record count: Rust 32, guest 31")
        );
    }

    #[test]
    fn records_of_a_ninth_light_join_the_last_slot_and_a_changed_copy_differs() {
        let mut m = FlatMemory::new(items::RECORDS, 0x6_0000);
        let mut slots = Slots::default();
        m.set_u16(items::MAIN_COUNT, 1).unwrap();
        m.set_u16(items::COUNT, 2).unwrap();
        for n in 0..SLOTS as u32 {
            slots.gather(&m, 50 + n).unwrap();
            m.set_u32(slot_base(n) + SLOT_LIGHT, 50 + n).unwrap();
        }
        let last = slot_base(SLOTS as u32 - 1);
        m.set_i32(FIRST_PERSON_COUNT, SLOTS).unwrap();
        m.set_u32(items::record_at(1), 1 << 10).unwrap();
        slots.gather(&m, 99).unwrap();
        m.set_u32(last, 1 << 10).unwrap();
        m.set_u16(last + SLOT_COUNT, 1).unwrap();
        assert_eq!(slots.first_difference(&m).unwrap(), None);
        m.set_u32(last + 4, 71).unwrap();
        assert_eq!(
            slots.first_difference(&m).unwrap().as_deref(),
            Some("slot 7 light 0x00000039 record 0")
        );
    }
}
