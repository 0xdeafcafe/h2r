//! The dependency solver: decides when a symbol's address is authoritative.
//!
//! Signatures refer to other symbols by address, so matching proceeds in
//! waves. The rules:
//!
//! - A *function* symbol's address is the unique hit of its highest matching
//!   revision. It is recomputed after every wave from the tasks that are
//!   currently valid, so it can move when a higher revision finally resolves.
//! - A *data* symbol (device pointer, state offsets) has no signature; its
//!   address is voted from the code that uses it and then never changes.
//! - Every finished task remembers the addresses it was checked against. If
//!   one of them later changes or disappears the task is thrown away and
//!   re-run, so no result survives on a stale reference.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::scan::{self, Task};

/// Upper bound on waves, a guard against references that flip forever.
const MAX_WAVES: usize = 256;

#[derive(Debug, Default)]
pub struct Outcome {
    /// Per task: finished with a trustworthy result.
    pub done: Vec<bool>,
    /// Probed runs dropped for matching many places with unchecked references.
    pub dropped_ambiguous: usize,
    /// Final addresses of function and data symbols.
    pub known: HashMap<String, u32>,
}

pub struct Input<'a> {
    /// Section bytes per database index.
    pub sections: &'a [Vec<(u32, &'a [u8])>],
    /// Symbol name each `(db, entry)` publishes its address under.
    pub symbol_of: &'a dyn Fn(usize, usize) -> &'a str,
    /// Names that other signatures can resolve to a function address.
    pub resolvable: &'a HashSet<&'a str>,
    /// Read a dword of guest memory (used to derive data symbols).
    pub read_u32: &'a dyn Fn(u32) -> Option<u32>,
}

pub fn solve(tasks: &mut [Task], inp: &Input) -> Result<Outcome, String> {
    let mut derived: HashMap<String, u32> = HashMap::new();
    let mut funcs: HashMap<String, u32> = HashMap::new();
    let mut done = vec![false; tasks.len()];
    let mut probed = vec![false; tasks.len()];
    let mut finalize = false;

    for _ in 0..MAX_WAVES {
        let known = merged(&derived, &funcs);
        let mut ready = Vec::new();
        let mut probe = Vec::new();
        for i in (0..tasks.len()).filter(|&i| !done[i]) {
            let (mut fn_ok, mut var_ok) = (true, true);
            for (_, n) in &tasks[i].xrefs {
                if !known.contains_key(n.as_str()) {
                    if inp.resolvable.contains(n.as_str()) {
                        fn_ok = false;
                    } else {
                        var_ok = false;
                    }
                }
            }
            if fn_ok && var_ok {
                ready.push(i);
            } else if fn_ok && !probed[i] {
                probe.push(i);
            }
        }
        if !ready.is_empty() {
            run_group(tasks, &ready, &known, inp)?;
            for &i in &ready {
                done[i] = true;
            }
        } else if !probe.is_empty() && !finalize {
            run_group(tasks, &probe, &known, inp)?;
            let mut votes: HashMap<&str, HashMap<u32, usize>> = HashMap::new();
            for &i in &probe {
                probed[i] = true;
                let t = &tasks[i];
                if let [hit] = t.hits[..] {
                    for (off, n) in t.xrefs.iter().filter(|(_, n)| !known.contains_key(n.as_str())) {
                        let v = hit.checked_add(u32::from(*off)).and_then(|a| (inp.read_u32)(a));
                        if let Some(v) = v {
                            *votes.entry(n.as_str()).or_default().entry(v).or_default() += 1;
                        }
                    }
                }
            }
            let mut fresh = Vec::new();
            for (n, vs) in votes {
                let top = vs.values().max().copied().unwrap_or(0);
                let winners: Vec<_> = vs.iter().filter(|(_, c)| **c == top).collect();
                if let [(v, _)] = winners[..] {
                    fresh.push((n.to_string(), *v));
                }
            }
            finalize = fresh.is_empty();
            derived.extend(fresh);
        } else {
            break;
        }
        // Publish function addresses from the tasks that are valid right now,
        // then invalidate every task that was checked against an address that
        // has since moved.
        funcs = publish(tasks, &done, inp);
        let known = merged(&derived, &funcs);
        invalidate(tasks, &mut done, &known);
    }
    // Whatever is left must be consistent with the final addresses.
    funcs = publish(tasks, &done, inp);
    let known = merged(&derived, &funcs);
    invalidate(tasks, &mut done, &known);

    // Probed tasks whose data symbols never resolved: keep a unique hit, drop
    // the rest (a signature with its pinning reference removed matches noise).
    let mut dropped = 0;
    for i in 0..tasks.len() {
        if !done[i] && probed[i] {
            if tasks[i].hits.len() > 1 {
                tasks[i].hits.clear();
                dropped += 1;
            }
            done[i] = true;
        }
    }
    Ok(Outcome { done, dropped_ambiguous: dropped, known })
}

fn merged(derived: &HashMap<String, u32>, funcs: &HashMap<String, u32>) -> HashMap<String, u32> {
    let mut m = derived.clone();
    m.extend(funcs.iter().map(|(k, v)| (k.clone(), *v)));
    m
}

fn run_group(tasks: &mut [Task], idxs: &[usize], known: &HashMap<String, u32>, inp: &Input) -> Result<(), String> {
    let mut by_db: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &i in idxs {
        by_db.entry(tasks[i].db).or_default().push(i);
    }
    for (db, group) in by_db {
        let hits = scan::run(&inp.sections[db], tasks, &group, known)?;
        for (i, h) in group.iter().zip(hits) {
            let t = &mut tasks[*i];
            t.used = t.xrefs.iter().map(|(_, n)| known.get(n.as_str()).copied()).collect();
            t.hits = h;
        }
    }
    Ok(())
}

/// Function addresses implied by the finished tasks. A name two entries place
/// at different addresses is ambiguous and gets none.
fn publish(tasks: &[Task], done: &[bool], inp: &Input) -> HashMap<String, u32> {
    let mut best: BTreeMap<(usize, usize), (u16, &Vec<u32>)> = BTreeMap::new();
    for (i, t) in tasks.iter().enumerate() {
        if done[i] && !t.hits.is_empty() {
            let e = best.entry((t.db, t.entry)).or_insert((t.version, &t.hits));
            if t.version >= e.0 {
                *e = (t.version, &t.hits);
            }
        }
    }
    let mut by_name: BTreeMap<&str, BTreeSet<u32>> = BTreeMap::new();
    for ((db, entry), (_, hits)) in best {
        if let [va] = hits[..] {
            by_name.entry((inp.symbol_of)(db, entry)).or_default().insert(va);
        }
    }
    by_name
        .into_iter()
        .filter_map(|(n, v)| if let [va] = v.iter().copied().collect::<Vec<_>>()[..] { Some((n.to_string(), va)) } else { None })
        .collect()
}

/// Un-finish every task whose recorded reference addresses differ from `known`.
fn invalidate(tasks: &mut [Task], done: &mut [bool], known: &HashMap<String, u32>) {
    for (i, t) in tasks.iter_mut().enumerate() {
        if !done[i] {
            continue;
        }
        let stale = t.xrefs.iter().zip(&t.used).any(|((_, n), used)| used.is_some() && known.get(n.as_str()).copied() != *used);
        if stale {
            done[i] = false;
            t.hits.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(entry: usize, version: u16, bytes: &[(u16, u8)], xrefs: &[(u16, &str)]) -> Task {
        Task {
            db: 0,
            entry,
            version,
            bytes: bytes.to_vec(),
            xrefs: xrefs.iter().map(|(o, n)| (*o, n.to_string())).collect(),
            hits: Vec::new(),
            used: Vec::new(),
        }
    }

    fn put(data: &mut [u8], at: usize, bytes: &[u8]) {
        data[at..at + bytes.len()].copy_from_slice(bytes);
    }

    /// Foo v1 resolves early to X; Foo v2 waits for Bar and lands elsewhere.
    /// A Baz match checked against Foo's early address must not survive.
    #[test]
    fn dependents_follow_a_moved_reference() {
        let base = 0x11000u32;
        let mut data = vec![0u8; 0x400];
        put(&mut data, 0x000, &[0x11, 0x22]); // Foo v1 at 0x11000
        put(&mut data, 0x010, &[0x33]); // Foo v2 at 0x11010, refs Bar
        put(&mut data, 0x011, &0x11100u32.to_le_bytes());
        put(&mut data, 0x100, &[0x44]); // Bar at 0x11100
        put(&mut data, 0x200, &[0x55]); // Baz v1 refs the old Foo
        put(&mut data, 0x201, &0x11000u32.to_le_bytes());
        put(&mut data, 0x210, &[0x56]); // Baz v2 refs the final Foo
        put(&mut data, 0x211, &0x11010u32.to_le_bytes());
        let mut tasks = vec![
            task(0, 1, &[(0, 0x11), (1, 0x22)], &[]),
            task(0, 2, &[(0, 0x33)], &[(1, "Bar")]),
            task(1, 1, &[(0, 0x44)], &[]),
            task(2, 1, &[(0, 0x55)], &[(1, "Foo")]),
            task(2, 2, &[(0, 0x56)], &[(1, "Foo")]),
        ];
        let names = ["Foo", "Bar", "Baz"];
        let resolvable: HashSet<&str> = names.iter().copied().collect();
        let sections = vec![vec![(base, &data[..])]];
        let symbol_of = |_: usize, e: usize| names[e];
        let read = |_: u32| None;
        let inp = Input { sections: &sections, symbol_of: &symbol_of, resolvable: &resolvable, read_u32: &read };
        let out = solve(&mut tasks, &inp).unwrap();
        assert_eq!(out.known["Foo"], 0x11010);
        assert!(out.done.iter().all(|d| *d));
        assert!(tasks[3].hits.is_empty(), "stale Baz v1 survived: {:?}", tasks[3].hits);
        assert_eq!(tasks[4].hits, vec![0x11210]);
    }
}
