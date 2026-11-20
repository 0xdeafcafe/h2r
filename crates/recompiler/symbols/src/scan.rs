//! Signature search over section bytes.

use std::collections::HashMap;

use crate::parse::Sig;

pub struct Task {
    pub db: usize,
    pub entry: usize,
    pub version: u16,
    pub bytes: Vec<(u16, u8)>,
    pub xrefs: Vec<(u16, String)>,
    pub hits: Vec<u32>,
    /// Address each reference was checked against in the last run
    /// (`None`: unchecked wildcard). Empty before the first run.
    pub used: Vec<Option<u32>>,
}

impl Task {
    pub fn new(db: usize, entry: usize, version: u16, sig: &Sig) -> Self {
        Task { db, entry, version, bytes: sig.bytes.clone(), xrefs: sig.xrefs.clone(), hits: Vec::new(), used: Vec::new() }
    }
}

/// Does the dword at `va` refer to `target`, as an absolute address or a
/// relative displacement (call/jmp rel32)?
fn refers_to(data: &[u8], base: u32, at: usize, target: u32) -> bool {
    let Some(b) = data.get(at..at + 4) else { return false };
    let v = u32::from_le_bytes(b.try_into().unwrap());
    v == target || v.wrapping_add(base).wrapping_add(at as u32).wrapping_add(4) == target
}

/// Search `sections` for each task in `idxs`; returns the hit addresses per
/// task, in order.
pub fn run(
    sections: &[(u32, &[u8])],
    tasks: &[Task],
    idxs: &[usize],
    known: &HashMap<String, u32>,
) -> Result<Vec<Vec<u32>>, String> {
    for &(base, data) in sections {
        if u64::from(base) + data.len() as u64 > 1 << 32 {
            return Err(format!("section at {base:#x} with {:#x} bytes wraps the address space", data.len()));
        }
    }
    // Anchor each task on its first byte constraint: group by offset, then
    // bucket by value.
    let mut groups: Vec<(u16, Vec<Vec<usize>>)> = Vec::new(); // (offset, 256 buckets of task slots)
    let mut unanchored = Vec::new();
    for (slot, &ti) in idxs.iter().enumerate() {
        match tasks[ti].bytes.first() {
            Some(&(off, val)) => {
                let g = match groups.iter().position(|g| g.0 == off) {
                    Some(g) => g,
                    None => {
                        groups.push((off, vec![Vec::new(); 256]));
                        groups.len() - 1
                    }
                };
                groups[g].1[val as usize].push(slot);
            }
            None => unanchored.push(slot),
        }
    }
    let _ = unanchored; // signatures with no byte constraint cannot be located

    // Resolve each xref once: Some(addr) to verify, None for a wildcard.
    let xr: Vec<Vec<(u16, Option<u32>)>> = idxs
        .iter()
        .map(|&ti| {
            tasks[ti]
                .xrefs
                .iter()
                .map(|(off, n)| (*off, known.get(n.as_str()).copied()))
                .collect()
        })
        .collect();

    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut out: Vec<Vec<u32>> = vec![Vec::new(); idxs.len()];
    for &(base, data) in sections {
        let chunk = data.len().div_ceil(threads).max(1);
        let parts: Vec<Vec<(usize, u32)>> = std::thread::scope(|sc| -> Result<_, String> {
            let hs: Vec<_> = (0..threads)
                .map(|t| {
                    let (groups, xr) = (&groups, &xr);
                    sc.spawn(move || {
                        let lo = t * chunk;
                        let hi = ((t + 1) * chunk).min(data.len());
                        let mut found = Vec::new();
                        for p in lo..hi {
                            for (off, buckets) in groups {
                                let Some(&b) = data.get(p + *off as usize) else { continue };
                                'cand: for &slot in &buckets[b as usize] {
                                    let task = &tasks[idxs[slot]];
                                    for &(o, v) in &task.bytes {
                                        if data.get(p + o as usize) != Some(&v) {
                                            continue 'cand;
                                        }
                                    }
                                    for &(o, addr) in &xr[slot] {
                                        if let Some(a) = addr
                                            && !refers_to(data, base, p + o as usize, a)
                                        {
                                            continue 'cand;
                                        }
                                    }
                                    found.push((slot, base + p as u32)); // in range: checked above
                                }
                            }
                        }
                        found
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().map_err(|_| "signature scan thread panicked".to_string())).collect()
        })?;
        for part in parts {
            for (slot, va) in part {
                out[slot].push(va);
            }
        }
    }
    for h in &mut out {
        h.sort_unstable();
        h.dedup();
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(bytes: &[(u16, u8)]) -> Task {
        Task { db: 0, entry: 0, version: 1, bytes: bytes.to_vec(), xrefs: vec![], hits: vec![], used: vec![] }
    }

    #[test]
    fn wrapping_section_is_rejected() {
        let data = [0x90u8, 0xc3];
        let tasks = [t(&[(0, 0xc3)])];
        let r = run(&[(0xffff_ffff, &data)], &tasks, &[0], &HashMap::new());
        assert!(r.is_err());
    }

    #[test]
    fn last_byte_of_address_space_matches() {
        let data = [0x90u8, 0xc3];
        let tasks = [t(&[(0, 0xc3)])];
        let r = run(&[(0xffff_fffe, &data)], &tasks, &[0], &HashMap::new()).unwrap();
        assert_eq!(r, vec![vec![0xffff_ffff]]);
    }
}
