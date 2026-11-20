//! Private INDX rows declare the disjoint runs of one machine-state section.
//! TICK and CIDX have their own public directory and never enter those runs.
use super::range::{CIDX, TICK};

pub fn is_machine(tag: [u8; 4]) -> bool {
    tag != TICK && tag != CIDX && tag != *b"INDX"
}

/// Streaming validation for native playback and Live's upload inspector.
/// Older flat indexes have no run markers anywhere.
#[derive(Default)]
pub struct MachineRuns {
    explicit: Option<bool>,
    previous_machine: bool,
    remaining: u32,
}

impl MachineRuns {
    pub fn feed(&mut self, tag: [u8; 4], machine_span: Option<u32>) -> Result<(), &'static str> {
        if !is_machine(tag) {
            if machine_span.is_some() || self.remaining != 0 {
                return Err("machine section overlaps a public film range");
            }
            self.previous_machine = false;
            return Ok(());
        }
        let explicit = *self.explicit.get_or_insert(machine_span.is_some());
        if !explicit {
            if machine_span.is_some() {
                return Err("mixed implicit and explicit machine sections");
            }
        } else {
            if !self.previous_machine {
                self.remaining = machine_span.ok_or("missing machine section run")?;
            } else if machine_span.is_some() {
                return Err("overlapping machine section runs");
            }
            self.remaining = self
                .remaining
                .checked_sub(1)
                .ok_or("short machine section run")?;
        }
        self.previous_machine = true;
        Ok(())
    }

    pub fn finish(self) -> Result<(), &'static str> {
        if self.remaining != 0 {
            return Err("unfinished machine section run");
        }
        Ok(())
    }
}
