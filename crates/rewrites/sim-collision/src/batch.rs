//! Ordered queries over a gathered collision world. A visit that marks an
//! object can change what a later visit sees when a generation wraps.
use crate::mopp::{Corners, Tree};
use crate::objects::{Visit, VisitQuery, object_visit};
use crate::{Bail, Fault, GuestRead, Topology};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Sparse final bytes of earlier queries' epoch and mark stores. Addresses
/// are offsets in the caller's gathered memory, or physical offsets when
/// the caller works on live guest mappings.
#[derive(Default)]
pub struct MarkBytes(BTreeMap<u32, u8>);

impl MarkBytes {
    pub fn store(&mut self, addr: u32, value: &[u8]) -> Result<(), Bail> {
        let end = u64::from(addr) + value.len() as u64;
        if end > 1 << 32 {
            return Err(Bail::Fault);
        }
        self.0
            .extend(value.iter().enumerate().map(|(i, &b)| (addr + i as u32, b)));
        Ok(())
    }

    pub fn overlaps(&self, addr: u32, len: u32) -> bool {
        if len == 0 {
            return false;
        }
        self.0
            .range(addr..=addr.saturating_add(len - 1))
            .next()
            .is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn forward(&self, addr: u32, dst: &mut [u8]) {
        if !dst.is_empty() {
            let end = (u64::from(addr) + dst.len() as u64 - 1).min(u32::MAX.into()) as u32;
            for (&at, &value) in self.0.range(addr..=end) {
                dst[(at - addr) as usize] = value;
            }
        }
    }
}

struct VisitMemory<'a, G> {
    g: &'a G,
    marks: MarkBytes,
}

impl<G: GuestRead> GuestRead for VisitMemory<'_, G> {
    fn read(&self, addr: u32, dst: &mut [u8]) -> Result<(), Fault> {
        self.g.read(addr, dst)?;
        self.marks.forward(addr, dst);
        Ok(())
    }
}

/// A visit's result and the epoch values to scatter with its marks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisitAnswer {
    pub visit: Visit,
    pub generation: u32,
    pub flood_epoch: Option<u32>,
}

/// Plan a batch against one snapshot. All queries supply the initial
/// generation and flood epoch of that snapshot. Only visits that flood
/// advance its cluster epoch. Guest memory stays untouched on a decline.
/// The gathered graph and lists must be independent of guest output buffers.
pub fn visit_batch(
    g: &impl GuestRead,
    queries: &[VisitQuery],
    mut topology: impl FnMut() -> Result<Arc<Topology>, Bail>,
) -> Result<Vec<VisitAnswer>, Bail> {
    let mut out = Vec::with_capacity(queries.len());
    let Some(first) = queries.first() else {
        return Ok(out);
    };
    let mut memory = VisitMemory {
        g,
        marks: MarkBytes::default(),
    };
    let (mut generation, mut flood_epoch) = (first.generation, first.flood_epoch);
    for q in queries {
        if (q.generation, q.flood_epoch, q.object_marks, q.cluster_marks)
            != (
                first.generation,
                first.flood_epoch,
                first.object_marks,
                first.cluster_marks,
            )
        {
            return Err(Bail::Data);
        }
        let visit = object_visit(
            &memory,
            &VisitQuery {
                generation,
                flood_epoch,
                ..*q
            },
            &mut topology,
        )?;
        let flood = if let Some(f) = &visit.flood {
            flood_epoch = flood_epoch.wrapping_add(1);
            for &cluster in &f.clusters {
                memory.marks.store(
                    q.cluster_marks.wrapping_add(4 * u32::from(cluster)),
                    &flood_epoch.to_le_bytes(),
                )?;
            }
            Some(flood_epoch)
        } else {
            None
        };
        generation = generation.wrapping_add(1);
        for &slot in &visit.marks {
            memory.marks.store(
                q.object_marks.wrapping_add(4 * u32::from(slot)),
                &generation.to_le_bytes(),
            )?;
        }
        out.push(VisitAnswer {
            visit,
            generation,
            flood_epoch: flood,
        });
    }
    Ok(out)
}

/// Walk up to eight boxes per packet. Collect keys in input order, with
/// the same first error as serial unlimited-capacity box queries.
pub fn box_batch(tree: &Tree, boxes: &[Corners]) -> Result<Vec<Vec<u32>>, Bail> {
    boxes
        .chunks(8)
        .flat_map(|packet| tree.query_packet(packet))
        .collect()
}

#[cfg(test)]
mod tests;
