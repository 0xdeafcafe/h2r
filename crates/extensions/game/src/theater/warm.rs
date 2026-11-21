//! Machine state for jumps within a running film. Device writebacks happen
//! once, at the recorded barrier, before either RAM or the remaining sections
//! are saved. Feature policies may call into the film while saving or loading.
use super::ring::Ring;
use crate::snapshot_glue::{BootMachine, RAMHASH, check_hash};
use anyhow::{Context, Result, ensure};
use film::manifest::DiskTracker;
use snapshot::{BuildKey, Snapshot, SnapshotWriter};
use std::sync::Arc;

struct Frame<C> {
    cursor: C,
    sections: Box<[u8]>,
    disk_delta: Box<[u8]>,
}

pub(super) struct Warm<C> {
    ring: Ring<Arc<Frame<C>>>,
    disk: DiskTracker,
}

impl<C: Clone> Warm<C> {
    pub(super) fn new(cap: usize, disk: DiskTracker) -> Self {
        Self {
            ring: Ring::new(cap),
            disk,
        }
    }

    pub(super) fn closest(&self, target: u64) -> Option<u64> {
        self.ring.closest(target)
    }

    pub(super) fn ticks(&self) -> Vec<u64> {
        self.ring.ticks()
    }

    pub(super) fn bytes(&self) -> usize {
        self.ring.bytes()
    }

    pub(super) fn cap(&self) -> usize {
        self.ring.cap()
    }

    pub(super) fn clear(&mut self, machine: &BootMachine<'_>) {
        if let Some(rt) = machine.rt {
            self.ring.clear(&mut rt.memory());
        }
    }

    /// The writer already holds the device section from replay's successful
    /// D3D barrier. A later blocker can skip this entry without repeating it.
    pub(super) fn capture(
        &mut self,
        machine: &mut BootMachine<'_>,
        tick: u64,
        cursor: C,
        cursor_bytes: usize,
        barrier: SnapshotWriter,
    ) -> Result<bool> {
        if !agent::SnapshotMachine::blockers(machine).is_empty() {
            return Ok(false);
        }
        let rt = machine.rt.context("ring capture needs a running guest")?;
        vfs::flush_pending_metadata();
        let disk_delta = self.disk.capture()?.into_boxed_slice();
        let sections = machine
            .save_ring_sections(barrier)
            .map_err(anyhow::Error::msg)?
            .into_boxed_slice();
        if !agent::SnapshotMachine::blockers(machine).is_empty() {
            return Ok(false);
        }
        let bytes = sections
            .len()
            .saturating_add(disk_delta.len())
            .saturating_add(cursor_bytes);
        self.ring.capture(
            tick,
            &mut rt.memory(),
            Arc::new(Frame {
                cursor,
                sections,
                disk_delta,
            }),
            bytes,
        )
    }

    /// Check the live directory even if playback has not reached another
    /// capture point. Saves can be written between two barriers.
    pub(super) fn disk_matches(&mut self, machine: &BootMachine<'_>, delta: &[u8]) -> Result<bool> {
        if !machine.film_load_blockers().is_empty() {
            return Ok(false);
        }
        vfs::flush_pending_metadata();
        Ok(self.disk.capture()?.as_slice() == delta)
    }

    /// None means the unmodified child cannot take this route. An error
    /// during a load means discard the child before it executes any slice.
    pub(super) fn restore(
        &mut self,
        machine: &mut BootMachine<'_>,
        tick: u64,
    ) -> Result<Option<C>> {
        let Some(frame) = self.ring.get(tick).cloned() else {
            return Ok(None);
        };
        if !self.disk_matches(machine, &frame.disk_delta)? {
            return Ok(None);
        }
        let snap = Snapshot::open(&frame.sections, machine.shelf.build())?;
        ensure!(snap.header.tick == tick, "ring snapshot tick differs");
        machine
            .load_ring_sections(&snap, |guest, layout| {
                self.ring
                    .restore_ram(tick, guest, layout)
                    .map_err(|e| e.to_string())
            })
            .map_err(anyhow::Error::msg)?;
        let hash = check_hash(&snap, &(machine.hash)());
        ensure!(
            hash.checked && hash.equal,
            "ring RAM mismatch at tick {tick}"
        );
        Ok(Some(frame.cursor.clone()))
    }

    /// Verification compares against the independently saved keyframe hash.
    pub(super) fn matches_recorded_ram(
        &self,
        tick: u64,
        frame: &film::blam::Keyframe,
        build: &BuildKey,
    ) -> Result<bool> {
        let entry = self
            .ring
            .get(tick)
            .context("missing verification ring entry")?;
        let captured = Snapshot::open(&entry.sections, build)?;
        let recorded = Snapshot::open(&frame.snapshot, build)?;
        ensure!(
            frame.tick == tick && recorded.header.tick == tick && captured.header.tick == tick,
            "keyframe RAM comparison requires the same tick"
        );
        Ok(recorded.require(RAMHASH, 1..=1)? == captured.require(RAMHASH, 1..=1)?)
    }
}

#[cfg(test)]
mod tests;
