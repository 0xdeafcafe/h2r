//! Clip dependencies used by the animation graph preload. A graph owns the
//! links and resource indices. Resource residency is sampled by the caller at
//! each candidate, and a walk ends before the first cache request.

/// Size of an animation element in the graph's animation block.
pub const CLIP_BYTES: usize = 0x6c;

struct ClipLink {
    next: i16,
    external: bool,
    resource: u32,
}

/// Only the fields used by the dependency walk, in clip index order.
pub struct Dependencies(Box<[ClipLink]>);

#[derive(Debug, PartialEq, Eq)]
pub enum Preload {
    End { clip: usize },
    Request { from: usize, resource: u32, active: u8 },
}

impl Dependencies {
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if !bytes.len().is_multiple_of(CLIP_BYTES) {
            return None;
        }
        Some(Self(bytes.chunks_exact(CLIP_BYTES).map(|record| ClipLink {
            next: i16::from_le_bytes([record[0x32], record[0x33]]),
            external: record[0x16] & 0x10 != 0,
            resource: u32::from_le_bytes(record[0x24..0x28].try_into().unwrap()),
        }).collect()))
    }

    /// The first request has already run. After a chain request the caller
    /// reloads the link at `start` before testing any further dependencies.
    /// An out-of-block link or an unproductive cycle uses the guest path.
    pub fn preload(
        &self,
        start: usize,
        after_chain_request: bool,
        mut active: impl FnMut(u32) -> Option<u8>,
    ) -> Option<Preload> {
        let mut clip = start;
        if after_chain_request {
            clip = usize::try_from(self.0.get(clip)?.next).ok()?;
        }
        for _ in 0..self.0.len() {
            let next = self.0.get(clip)?.next;
            if next == -1 {
                return Some(Preload::End { clip });
            }
            let target = usize::try_from(next).ok()?;
            let link = self.0.get(target)?;
            if link.external {
                let active = active(link.resource)?;
                if active != 0 {
                    return Some(Preload::Request { from: clip, resource: link.resource, active });
                }
            }
            clip = target;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stops_at_the_first_active_resource_and_resumes_through_the_live_link() {
        let mut bytes = vec![0; 4 * CLIP_BYTES];
        for (row, (next, resource)) in bytes.chunks_exact_mut(CLIP_BYTES)
            .zip([(1_i16, None), (2, Some(5_u32)), (3, Some(8)), (-1, None)])
        {
            row[0x32..0x34].copy_from_slice(&next.to_le_bytes());
            if let Some(index) = resource {
                row[0x16] = 0x10;
                row[0x24..0x28].copy_from_slice(&index.to_le_bytes());
            }
        }
        let table = Dependencies::decode(&bytes).unwrap();
        let mut calls = Vec::new();
        assert_eq!(table.preload(0, false, |index| {
            calls.push(index);
            Some(if index == 8 { 0x84 } else { 0 })
        }), Some(Preload::Request { from: 1, resource: 8, active: 0x84 }));
        assert_eq!(calls, [5, 8]);
        // A cache callback edits clip 1 to point straight at the last clip.
        bytes[CLIP_BYTES + 0x32..CLIP_BYTES + 0x34].copy_from_slice(&3_i16.to_le_bytes());
        let changed = Dependencies::decode(&bytes).unwrap();
        assert_eq!(changed.preload(1, true, |_| panic!("unexpected request")), Some(Preload::End { clip: 3 }));
    }

    #[test]
    fn declines_cycles_and_invalid_signed_indices() {
        for next in [0_i16, -2, i16::MAX] {
            let mut row = [0; CLIP_BYTES];
            row[0x32..0x34].copy_from_slice(&next.to_le_bytes());
            let table = Dependencies::decode(&row).unwrap();
            assert_eq!(table.preload(0, false, |_| None), None);
        }
    }
}
