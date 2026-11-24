//! The named export search inside the object function evaluator.

/// The position saved by the caller while a matching export is evaluated.
/// Both fields belong to the guest frame and are restored independently.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExportCursor {
    pub index: u32,
    pub offset: u32,
}

impl ExportCursor {
    pub fn advance(&mut self) {
        self.index = self.index.wrapping_add(1);
        self.offset = self.offset.wrapping_add(0x20);
    }

    /// Search from a record already admitted by the caller's count check.
    /// There are no writes or callbacks between rejected names, so the
    /// table pointer and requested name are stable over this scan. Return
    /// on the first match, before touching a later (possibly unmapped) row.
    pub fn find<E>(
        &mut self,
        definition: u32,
        name_slot: u32,
        mut read: impl FnMut(u32) -> Result<u32, E>,
    ) -> Result<ExportStop, E> {
        let base = read(definition.wrapping_add(0x68))?;
        let mut record = base.wrapping_add(self.offset);
        let mut name = read(record.wrapping_add(8))?;
        let wanted = read(name_slot)?;
        if name == wanted {
            return Ok(ExportStop::Evaluate { record, name });
        }
        // At entry the caller spills its body pointer after its initial
        // count check. Read the count here, after that store and the name.
        let count = read(definition.wrapping_add(0x64))? as i32;
        loop {
            self.advance();
            if self.index as i32 >= count {
                return Ok(ExportStop::End {
                    count,
                    last_name: name,
                });
            }
            record = base.wrapping_add(self.offset);
            name = read(record.wrapping_add(8))?;
            if name == wanted {
                return Ok(ExportStop::Evaluate { record, name });
            }
        }
    }
}

/// Besides the decision, the guest exposes the last name it read and the
/// final count at the transfer out of the scan.
pub enum ExportStop {
    Evaluate { record: u32, name: u32 },
    End { count: i32, last_name: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    // The count, table pointer and name slot here are synthetic. Only names
    // are mapped, so an access past the table is observable.
    const DEF: u32 = 0x100;
    const TABLE: u32 = 0x800;
    const QUERY: u32 = 0x40;

    #[test]
    fn matches_a_linear_search_and_resumes_at_duplicate_names() {
        let names: Vec<u32> = (0..4097)
            .map(|i| if i % 511 == 0 { 5 } else { 2 })
            .collect();
        for wanted in [2, 5, 9] {
            let mut cursor = ExportCursor {
                index: 0,
                offset: 0,
            };
            let mut found = Vec::new();
            while (cursor.index as usize) < names.len() {
                let stop = cursor
                    .find(DEF, QUERY, |addr| match addr {
                        a if a == DEF + 0x68 => Ok(TABLE),
                        a if a == DEF + 0x64 => Ok(names.len() as u32),
                        QUERY => Ok(wanted),
                        a if a >= TABLE + 8 && (a - TABLE - 8) % 32 == 0 => {
                            names.get(((a - TABLE - 8) / 32) as usize).copied().ok_or(a)
                        }
                        _ => Err(addr),
                    })
                    .unwrap();
                assert_eq!(cursor.offset, cursor.index * 32);
                match stop {
                    ExportStop::Evaluate { record, name } => {
                        assert_eq!(record, TABLE + cursor.offset);
                        assert_eq!(name, wanted);
                        found.push(cursor.index as usize);
                        cursor.advance();
                    }
                    ExportStop::End { count, last_name } => {
                        assert_eq!(count, names.len() as i32);
                        assert_eq!(cursor.index as usize, names.len());
                        assert_eq!(last_name, *names.last().unwrap());
                    }
                }
            }
            let expected: Vec<_> = (0..names.len()).filter(|&i| names[i] == wanted).collect();
            assert_eq!(found, expected);
        }
    }

    #[test]
    fn first_match_needs_neither_a_count_nor_a_later_record() {
        let mut cursor = ExportCursor {
            index: 12,
            offset: 0x60,
        };
        let read = |addr| match addr {
            a if a == DEF + 0x68 => Ok(TABLE),
            a if a == TABLE + 0x68 || a == QUERY => Ok(4),
            _ => Err(addr),
        };
        let stop = cursor.find(DEF, QUERY, read).unwrap();
        assert!(matches!(stop, ExportStop::Evaluate { record, name: 4 } if record == TABLE + 0x60));
        assert_eq!(cursor.index, 12);
    }
}
