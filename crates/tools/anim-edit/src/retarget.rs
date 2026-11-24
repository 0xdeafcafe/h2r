//! Copy sparse animation channels between skeletons using local rest transforms.

use std::collections::HashMap;
use std::sync::Arc;

use tags::anim_keys::Kind;
use tags::anim_sample::{NodeDefault, NodeTransform, Pose, hamilton};

use crate::{AnimDoc, ClipKind, Key, Node, normalize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeMapping {
    pub source: usize,
    /// A manual `None` leaves the source unmapped, even if its name matches.
    pub target: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DroppedChannel {
    pub node: usize,
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetargetReport {
    /// One row per source node, in source order.
    pub nodes: Vec<NodeMapping>,
    pub unmatched_source: Vec<usize>,
    pub unmatched_target: Vec<usize>,
    pub dropped_channels: Vec<DroppedChannel>,
}

/// Resolve manual pairs first, then match names unique among the remaining
/// nodes on each skeleton. The table and dropped tracks can be inspected
/// without a render model or a new document.
pub fn retarget_report(
    source: &AnimDoc,
    target_nodes: &[Node],
    overrides: &[NodeMapping],
) -> Result<RetargetReport, String> {
    let mut pairs = vec![None; source.nodes().len()];
    let mut manual = vec![false; source.nodes().len()];
    let mut used = vec![false; target_nodes.len()];
    for &pair in overrides {
        let fixed = manual
            .get_mut(pair.source)
            .ok_or_else(|| format!("source node {} is out of range", pair.source))?;
        if *fixed {
            return Err(format!("source node {} is mapped twice", pair.source));
        }
        *fixed = true;
        if let Some(target) = pair.target {
            let claimed = used
                .get_mut(target)
                .ok_or_else(|| format!("target node {target} is out of range"))?;
            if *claimed {
                return Err(format!("target node {target} is mapped twice"));
            }
            *claimed = true;
        }
        pairs[pair.source] = pair.target;
    }

    let source_names = available_names(source.nodes(), &manual);
    let target_names = available_names(target_nodes, &used);
    for (name, source) in source_names {
        if let (Some(source), Some(Some(target))) = (source, target_names.get(&name)) {
            pairs[source] = Some(*target);
            used[*target] = true;
        }
    }
    Ok(RetargetReport {
        unmatched_source: indices_where(pairs.iter().map(Option::is_none)),
        unmatched_target: indices_where(used.into_iter().map(|matched| !matched)),
        dropped_channels: source
            .channels()
            .filter(|(node, _, _)| pairs[*node].is_none())
            .map(|(node, kind, _)| DroppedChannel { node, kind })
            .collect(),
        nodes: pairs
            .into_iter()
            .enumerate()
            .map(|(source, target)| NodeMapping { source, target })
            .collect(),
    })
}

fn available_names(nodes: &[Node], reserved: &[bool]) -> HashMap<String, Option<usize>> {
    let mut names = HashMap::new();
    for (index, node) in nodes.iter().enumerate() {
        if reserved[index] {
            continue;
        }
        if let Some(name) = node.name.as_ref().filter(|name| !name.trim().is_empty()) {
            names
                .entry(name.to_ascii_lowercase())
                .and_modify(|found| *found = None)
                .or_insert(Some(index));
        }
    }
    names
}

fn indices_where(flags: impl Iterator<Item = bool>) -> Vec<usize> {
    flags
        .enumerate()
        .filter_map(|(i, include)| include.then_some(i))
        .collect()
}

/// Apply a clip to the destination skeleton without changing its key times
/// or interpolation modes. Base and replacement rotations are
/// `target_rest * inverse(source_rest) * key`. Rest-relative translation
/// deltas and their slopes scale by the ratio of local bone lengths.
/// Root and zero-length links keep the delta's magnitude. Overlay tracks
/// are already additive, so only the translation ratio applies to them.
pub fn retarget(
    source: &AnimDoc,
    source_rest: &[NodeDefault],
    target_nodes: &[Node],
    target_rest: &[NodeDefault],
    overrides: &[NodeMapping],
    name: String,
    kind: ClipKind,
) -> Result<(AnimDoc, RetargetReport), String> {
    let report = retarget_report(source, target_nodes, overrides)?;
    let source_rest = checked_rest(source.nodes(), source_rest, "source")?;
    let target_rest = checked_rest(target_nodes, target_rest, "target")?;
    let overlay = kind == ClipKind::Overlay;
    let mut output = AnimDoc::new(name, source.frames(), target_nodes.to_vec())?;
    for (from, kind, keys) in source.channels() {
        let Some(to) = report.nodes[from].target else {
            continue;
        };
        let track = (to, kind);
        let convert = || {
            let keys = retarget_keys(kind, keys, source_rest[from], target_rest[to], overlay)?;
            output.validate_keys(track, &keys)?;
            Ok::<_, String>(keys)
        };
        let keys = convert().map_err(|err| {
            format!("retargeting {} from node {from} to {to}: {err}", kind.name())
        })?;
        output.tracks.insert(track, Arc::new(keys));
    }
    Ok((output, report))
}

fn checked_rest(nodes: &[Node], rest: &[NodeDefault], side: &str) -> Result<Vec<NodeDefault>, String> {
    if nodes.len() != rest.len() {
        return Err(format!(
            "{side} skeleton has {} nodes and {} rest transforms",
            nodes.len(),
            rest.len()
        ));
    }
    let parents: Vec<_> = nodes.iter().map(|node| node.parent).collect();
    // Use the same index and cycle checks as the pose composer.
    Pose {
        nodes: vec![NodeTransform::default(); nodes.len()],
    }
    .world_matrices(&parents)
    .map_err(|err| format!("{side} skeleton: {err}"))?;

    nodes
        .iter()
        .zip(rest)
        .enumerate()
        .map(|(index, (node, rest))| {
            if node.parent != rest.parent {
                return Err(format!("{side} rest parent differs on node {index}"));
            }
            if !rest.translation.into_iter().chain([rest.scale]).all(f32::is_finite) {
                return Err(format!("{side} rest transform on node {index} must be finite"));
            }
            let rotation = normalize(rest.rotation)
                .ok_or_else(|| format!("{side} rest rotation on node {index} must be finite and nonzero"))?;
            Ok(NodeDefault { rotation, ..*rest })
        })
        .collect()
}

fn bone_ratio(source: NodeDefault, target: NodeDefault) -> f64 {
    let length = |node: NodeDefault| {
        node.translation
            .into_iter()
            .map(|value| f64::from(value).powi(2))
            .sum::<f64>()
            .sqrt()
    };
    let (a, b) = (length(source), length(target));
    if source.parent < 0 || target.parent < 0 || a == 0.0 || b == 0.0 {
        1.0
    } else {
        b / a
    }
}

fn retarget_keys(
    kind: Kind,
    keys: &[Key],
    source: NodeDefault,
    target: NodeDefault,
    overlay: bool,
) -> Result<Vec<Key>, String> {
    let mut keys = keys.to_vec();
    match kind {
        Kind::Rotation if !overlay => {
            let [x, y, z, w] = source.rotation;
            let rest_delta = hamilton(target.rotation, [-x, -y, -z, w]);
            for key in &mut keys {
                key.value = normalize(hamilton(rest_delta, key.value))
                    .ok_or_else(|| format!("invalid rotation at frame {}", key.frame))?;
                // Quaternion multiplication by the constant delta is linear.
                key.tangent = hamilton(rest_delta, key.tangent);
            }
        }
        Kind::Translation => {
            let ratio = bone_ratio(source, target);
            let (from, to) = if overlay {
                ([0.0; 3], [0.0; 3])
            } else {
                (source.translation, target.translation)
            };
            for key in &mut keys {
                for axis in 0..3 {
                    let delta = f64::from(key.value[axis]) - f64::from(from[axis]);
                    key.value[axis] = (f64::from(to[axis]) + delta * ratio) as f32;
                    key.tangent[axis] = (f64::from(key.tangent[axis]) * ratio) as f32;
                }
            }
        }
        Kind::Scale if !overlay => {
            if source.scale == 0.0 {
                return Err("source rest scale is zero".to_owned());
            }
            let ratio = f64::from(target.scale) / f64::from(source.scale);
            for key in &mut keys {
                key.value[0] = (f64::from(key.value[0]) * ratio) as f32;
                key.tangent[0] = (f64::from(key.tangent[0]) * ratio) as f32;
            }
        }
        Kind::Rotation | Kind::Scale => {}
    }
    Ok(keys)
}

#[cfg(test)]
#[path = "retarget/tests.rs"]
mod tests;
