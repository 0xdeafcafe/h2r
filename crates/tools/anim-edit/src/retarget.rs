//! Carry a clip onto another skeleton (AN5 of
//! `docs/editor/studio-redesign.md`, "Retarget").
//!
//! Nodes pair by name, and a table fixes the rest. A rotation key carries
//! as its delta from the source node's rest rotation, applied to the
//! target node's rest rotation. A translation key carries as its offset
//! from the source rest translation, scaled by the ratio of the two rest
//! translations' lengths (the bone length) and added to the target's
//! rest. A scale key carries unchanged.

use std::collections::BTreeMap;
use std::sync::Arc;

use tags::anim_keys::Kind;
use tags::anim_sample::NodeDefault;

use crate::{AnimDoc, Key, Node};

/// What a retarget did, for the caller to show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// `(source node, target node)`, by name or by the table.
    pub matched: Vec<(usize, usize)>,
    /// Source nodes that carry keys and pair with no target node: their
    /// channels are dropped.
    pub dropped_nodes: Vec<usize>,
    /// How many channels (a node's rotation, translation or scale) the
    /// dropped nodes held.
    pub dropped_channels: usize,
    /// Target nodes with no source node: left unkeyed.
    pub unmatched_targets: Vec<usize>,
    /// `(target node, ratio)` for each matched node whose bone length
    /// scales its translation keys.
    pub scaled: Vec<(usize, f32)>,
}

type Q = [f32; 4];

fn mul(a: Q, b: Q) -> Q {
    let [ax, ay, az, aw] = a.map(f64::from);
    let [bx, by, bz, bw] = b.map(f64::from);
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
    .map(|v| v as f32)
}

fn conjugate(q: Q) -> Q {
    [-q[0], -q[1], -q[2], q[3]]
}

fn unit(q: Q) -> Result<Q, String> {
    crate::normalize(q).ok_or_else(|| "a rotation is not a nonzero quaternion".to_owned())
}

fn length(t: [f32; 3]) -> f64 {
    t.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>().sqrt()
}

/// Retarget `source` (on a skeleton with rest pose `source_rest`) onto
/// `target` nodes with rest pose `target_rest`. `table` pairs
/// `(source node, target node)` and wins over a name match; a target node
/// named by the table takes only that source node.
pub fn retarget(
    source: &AnimDoc,
    source_rest: &[NodeDefault],
    target: &[Node],
    target_rest: &[NodeDefault],
    name: String,
    table: &[(usize, usize)],
) -> Result<(AnimDoc, Report), String> {
    if source_rest.len() != source.nodes().len() {
        return Err(format!(
            "the source rest pose has {} nodes, the clip {}",
            source_rest.len(),
            source.nodes().len()
        ));
    }
    if target_rest.len() != target.len() {
        return Err(format!(
            "the target rest pose has {} nodes, its skeleton {}",
            target_rest.len(),
            target.len()
        ));
    }
    let mut pair: BTreeMap<usize, usize> = BTreeMap::new();
    for &(from, to) in table {
        if from >= source.nodes().len() || to >= target.len() {
            return Err(format!(
                "the table pairs {from} with {to}, but the clip has {} nodes and the target {}",
                source.nodes().len(),
                target.len()
            ));
        }
        if pair.insert(to, from).is_some_and(|old| old != from) {
            return Err(format!("the table pairs target node {to} twice"));
        }
    }
    for (to, node) in target.iter().enumerate() {
        if pair.contains_key(&to) {
            continue;
        }
        let Some(want) = node.name.as_deref() else {
            continue;
        };
        let mut same = source
            .nodes()
            .iter()
            .enumerate()
            .filter(|(_, n)| n.name.as_deref() == Some(want))
            .map(|(i, _)| i);
        if let (Some(from), None) = (same.next(), same.next()) {
            pair.insert(to, from);
        }
    }
    let mut doc = AnimDoc::new(name, source.frames(), target.to_vec())?;
    let mut report = Report::default();
    let mut by_source: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (&to, &from) in &pair {
        by_source.entry(from).or_default().push(to);
        report.matched.push((from, to));
    }
    report.matched.sort_unstable();
    for to in 0..target.len() {
        if !pair.contains_key(&to) {
            report.unmatched_targets.push(to);
        }
    }
    let mut dropped = std::collections::BTreeSet::new();
    for (node, kind, keys) in source.channels() {
        let Some(targets) = by_source.get(&node) else {
            if dropped.insert(node) {
                report.dropped_nodes.push(node);
            }
            report.dropped_channels += 1;
            continue;
        };
        for &to in targets {
            let (rs, rt) = (&source_rest[node], &target_rest[to]);
            let (ls, lt) = (length(rs.translation), length(rt.translation));
            let ratio = if ls > 1e-6 && lt > 1e-6 {
                (lt / ls) as f32
            } else {
                1.0
            };
            if kind == Kind::Translation && (ratio - 1.0).abs() > 1e-6 {
                report.scaled.push((to, ratio));
            }
            let mut out: Vec<Key> = Vec::with_capacity(keys.len());
            for key in keys {
                let mut moved = *key;
                match kind {
                    Kind::Rotation => {
                        let delta = mul(conjugate(unit(rs.rotation)?), key.value);
                        let mut q = unit(mul(unit(rt.rotation)?, delta))?;
                        if out
                            .last()
                            .is_some_and(|last| crate::dot(last.value, q) < 0.0)
                        {
                            q = q.map(|v| -v);
                        }
                        moved.value = q;
                        // A tangent does not carry through a change of rest
                        // rotation: the key keeps its interpolation alone.
                        moved.tangent = [0.0; 4];
                    }
                    Kind::Translation => {
                        for i in 0..3 {
                            moved.value[i] =
                                rt.translation[i] + (key.value[i] - rs.translation[i]) * ratio;
                            moved.tangent[i] = key.tangent[i] * ratio;
                        }
                    }
                    Kind::Scale => {}
                }
                out.push(moved);
            }
            if !out
                .iter()
                .flat_map(|k| k.value.into_iter().chain(k.tangent))
                .all(f32::is_finite)
            {
                return Err("the retargeted keys are not finite".to_owned());
            }
            doc.tracks.insert((to, kind), Arc::new(out));
        }
    }
    report.scaled.sort_by_key(|&(to, _)| to);
    report.scaled.dedup_by_key(|s| s.0);
    Ok((doc, report))
}

#[cfg(test)]
mod tests;
