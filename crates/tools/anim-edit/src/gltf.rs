//! Read one glTF 2.0 animation (`.gltf` with its buffers, or `.glb`) as a
//! sparse clip on the file's own node tree.
//!
//! The file's axes (Y up, +Z forward) are rewritten to the engine's
//! (Z up, +X forward, +Y left) by one rotation applied to every rest
//! transform and key, so local rotations keep their meaning. Times become
//! frames at 30 Hz. Channels the editor cannot hold are listed in the
//! clip's notes rather than guessed.

use std::collections::BTreeMap;
use std::path::{Component, Path};
use std::sync::Arc;

use serde_json::Value;
use tags::anim_keys::Kind;
use tags::anim_sample::NodeDefault;

use crate::{AnimDoc, Interp, Key, Node, normalize};

const HZ: f64 = 30.0;
const FLOAT: u64 = 5126;

/// One imported clip: keys on the file's nodes, the nodes' rest
/// transforms and what was left out.
pub struct GltfClip {
    pub clip: AnimDoc,
    pub rest: Vec<NodeDefault>,
    pub notes: Vec<String>,
}

/// The animation names of a file, in file order. An unnamed animation is
/// called `animation N`.
pub fn gltf_clips(path: &Path) -> Result<Vec<String>, String> {
    let bytes = read_file(path)?;
    clip_names(&bytes)
}

/// Read the animation at `index` of a file.
pub fn read_gltf(path: &Path, index: usize) -> Result<GltfClip, String> {
    let bytes = read_file(path)?;
    read_gltf_bytes(&bytes, path.parent(), index)
}

pub fn clip_names(bytes: &[u8]) -> Result<Vec<String>, String> {
    let (json, _) = split(bytes)?;
    Ok(array(&json, "animations")
        .iter()
        .enumerate()
        .map(|(i, a)| {
            a.get("name")
                .and_then(Value::as_str)
                .map_or_else(|| format!("animation {i}"), str::to_owned)
        })
        .collect())
}

/// `dir` resolves relative buffer files of a `.gltf`.
pub fn read_gltf_bytes(bytes: &[u8], dir: Option<&Path>, index: usize) -> Result<GltfClip, String> {
    let (json, bin) = split(bytes)?;
    let names = clip_names(bytes)?;
    let animation = array(&json, "animations")
        .get(index)
        .ok_or_else(|| format!("the file has {} animations, no {index}", names.len()))?;
    let mut notes = Vec::new();
    let (nodes, rest) = skeleton(&json, &mut notes)?;
    let buffers = Buffers {
        json: &json,
        bin,
        dir,
    };
    let mut tracks: BTreeMap<(usize, Kind), Vec<Key>> = BTreeMap::new();
    for (c, channel) in array(animation, "channels").iter().enumerate() {
        let at = format!("channel {c}");
        let Some(node) = channel.pointer("/target/node").and_then(Value::as_u64) else {
            notes.push(format!("{at}: no target node, skipped"));
            continue;
        };
        let node = usize::try_from(node)
            .ok()
            .filter(|&n| n < nodes.len())
            .ok_or_else(|| format!("{at}: node {node} does not exist"))?;
        let path = channel
            .pointer("/target/path")
            .and_then(Value::as_str)
            .unwrap_or("");
        let kind = match path {
            "translation" => Kind::Translation,
            "rotation" => Kind::Rotation,
            "scale" => Kind::Scale,
            other => {
                notes.push(format!("{at}: {other:?} channels are not supported, skipped"));
                continue;
            }
        };
        let sampler = channel
            .get("sampler")
            .and_then(Value::as_u64)
            .and_then(|s| array(animation, "samplers").get(s as usize))
            .ok_or_else(|| format!("{at}: no such sampler"))?;
        let keys = sampler_keys(&buffers, sampler, kind, &at, &mut notes)?;
        let Some(keys) = keys else { continue };
        if tracks.insert((node, kind), keys).is_some() {
            return Err(format!("{at}: node {node} {path} is animated twice"));
        }
    }
    if tracks.is_empty() {
        return Err("the animation has no channel the editor can hold".to_owned());
    }
    let frames = tracks
        .values()
        .filter_map(|keys| keys.last())
        .map(|key| key.frame)
        .max()
        .map_or(1, |last| last + 1);
    let name = names[index].clone();
    let mut clip = AnimDoc::new(name, frames, nodes)?;
    for (track, keys) in tracks {
        clip.validate_keys(track, &keys)?;
        clip.tracks.insert(track, Arc::new(keys));
    }
    Ok(GltfClip { clip, rest, notes })
}

fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// The JSON document and a GLB's binary chunk.
fn split(bytes: &[u8]) -> Result<(Value, Option<Vec<u8>>), String> {
    if !bytes.starts_with(b"glTF") {
        let json = serde_json::from_slice(bytes).map_err(|e| format!("not glTF JSON: {e}"))?;
        return Ok((json, None));
    }
    let word = |at: usize| -> Result<usize, String> {
        bytes
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
            .ok_or_else(|| "the .glb is cut short".to_owned())
    };
    if word(4)? != 2 {
        return Err("only glTF 2 is supported".to_owned());
    }
    let (mut json, mut bin) = (None, None);
    let mut at = 12;
    while at < bytes.len() {
        let (len, kind) = (word(at)?, word(at + 4)?);
        let data = bytes
            .get(at + 8..at + 8 + len)
            .ok_or_else(|| "a .glb chunk is cut short".to_owned())?;
        match kind {
            0x4E4F_534A if json.is_none() => json = Some(data),
            0x004E_4942 if bin.is_none() => bin = Some(data.to_vec()),
            _ => {}
        }
        at += 8 + len.div_ceil(4) * 4;
    }
    let json = json.ok_or_else(|| "the .glb has no JSON chunk".to_owned())?;
    let json = serde_json::from_slice(json).map_err(|e| format!("not glTF JSON: {e}"))?;
    Ok((json, bin))
}

/// The engine's axes for a vector or a quaternion's vector part: the
/// file's (x, y, z) is the engine's (z, x, y), a proper rotation.
fn axes(v: [f32; 3]) -> [f32; 3] {
    [v[2], v[0], v[1]]
}

fn axes_q(q: [f32; 4]) -> [f32; 4] {
    [q[2], q[0], q[1], q[3]]
}

fn vector<const N: usize>(value: Option<&Value>, default: [f32; N], what: &str) -> Result<[f32; N], String> {
    let Some(value) = value else { return Ok(default) };
    let items = value
        .as_array()
        .filter(|a| a.len() == N)
        .ok_or_else(|| format!("{what} needs {N} numbers"))?;
    let mut out = [0.0; N];
    for (slot, item) in out.iter_mut().zip(items) {
        let n = item.as_f64().ok_or_else(|| format!("{what} needs numbers"))?;
        *slot = n as f32;
    }
    if !out.iter().all(|v| v.is_finite()) {
        return Err(format!("{what} must be finite"));
    }
    Ok(out)
}

fn skeleton(json: &Value, notes: &mut Vec<String>) -> Result<(Vec<Node>, Vec<NodeDefault>), String> {
    let list = array(json, "nodes");
    if list.is_empty() || list.len() > i16::MAX as usize {
        return Err(format!("the file has {} nodes", list.len()));
    }
    let mut parents = vec![-1i16; list.len()];
    for (i, node) in list.iter().enumerate() {
        for child in array(node, "children") {
            let child = child
                .as_u64()
                .and_then(|c| usize::try_from(c).ok())
                .filter(|&c| c < list.len())
                .ok_or_else(|| format!("node {i} has a child that does not exist"))?;
            if parents[child] >= 0 {
                return Err(format!("node {child} has two parents"));
            }
            parents[child] = i as i16;
        }
    }
    let mut nodes = Vec::new();
    let mut rest = Vec::new();
    for (i, node) in list.iter().enumerate() {
        if node.get("matrix").is_some() {
            return Err(format!("node {i} uses a matrix; export translation, rotation and scale"));
        }
        let at = format!("node {i}");
        let rotation = vector(node.get("rotation"), [0.0, 0.0, 0.0, 1.0], &at)?;
        let rotation = normalize(rotation).ok_or_else(|| format!("{at}: rotation must be nonzero"))?;
        let translation = vector(node.get("translation"), [0.0; 3], &at)?;
        let scale = vector(node.get("scale"), [1.0; 3], &at)?;
        if !uniform(scale) {
            notes.push(format!("{at}: non-uniform rest scale, the x scale is used"));
        }
        nodes.push(Node {
            name: node.get("name").and_then(Value::as_str).map(str::to_owned),
            parent: parents[i],
        });
        rest.push(NodeDefault {
            parent: parents[i],
            rotation: axes_q(rotation),
            translation: axes(translation),
            scale: scale[0],
        });
    }
    Ok((nodes, rest))
}

fn uniform(s: [f32; 3]) -> bool {
    let tolerance = 1e-4 * s[0].abs().max(1.0);
    (s[1] - s[0]).abs() <= tolerance && (s[2] - s[0]).abs() <= tolerance
}

struct Buffers<'a> {
    json: &'a Value,
    bin: Option<Vec<u8>>,
    dir: Option<&'a Path>,
}

impl Buffers<'_> {
    fn buffer(&self, index: usize) -> Result<Vec<u8>, String> {
        let buffer = array(self.json, "buffers")
            .get(index)
            .ok_or_else(|| format!("buffer {index} does not exist"))?;
        match buffer.get("uri").and_then(Value::as_str) {
            None if index == 0 => self
                .bin
                .clone()
                .ok_or_else(|| "buffer 0 has no uri and the file has no binary chunk".to_owned()),
            None => Err(format!("buffer {index} has no uri")),
            Some(uri) if uri.starts_with("data:") => {
                let (_, data) = uri
                    .split_once(";base64,")
                    .ok_or_else(|| format!("buffer {index}: only base64 data uris are supported"))?;
                base64(data).ok_or_else(|| format!("buffer {index}: bad base64"))
            }
            Some(uri) => {
                let relative = Path::new(uri);
                if relative
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_)))
                {
                    return Err(format!("buffer {index}: {uri:?} must be a plain relative file"));
                }
                let dir = self
                    .dir
                    .ok_or_else(|| format!("buffer {index}: {uri:?} has no directory to read from"))?;
                read_file(&dir.join(relative))
            }
        }
    }

    /// An accessor's float elements, `width` per element.
    fn floats(&self, accessor: usize, width: usize) -> Result<Vec<f32>, String> {
        let a = array(self.json, "accessors")
            .get(accessor)
            .ok_or_else(|| format!("accessor {accessor} does not exist"))?;
        let at = format!("accessor {accessor}");
        let expect = match width {
            1 => "SCALAR",
            3 => "VEC3",
            _ => "VEC4",
        };
        if a.get("type").and_then(Value::as_str) != Some(expect) {
            return Err(format!("{at} is not {expect}"));
        }
        if a.get("componentType").and_then(Value::as_u64) != Some(FLOAT) || a.get("normalized").and_then(Value::as_bool) == Some(true) {
            return Err(format!("{at}: only unnormalized float data is supported"));
        }
        if a.get("sparse").is_some() {
            return Err(format!("{at}: sparse accessors are not supported"));
        }
        let count = a.get("count").and_then(Value::as_u64).unwrap_or(0) as usize;
        let view = a
            .get("bufferView")
            .and_then(Value::as_u64)
            .and_then(|v| array(self.json, "bufferViews").get(v as usize))
            .ok_or_else(|| format!("{at} has no buffer view"))?;
        let number = |v: &Value, key: &str| v.get(key).and_then(Value::as_u64).unwrap_or(0) as usize;
        let element = width * 4;
        let stride = match number(view, "byteStride") {
            0 => element,
            s if s >= element => s,
            _ => return Err(format!("{at}: the stride is smaller than an element")),
        };
        let buffer = self.buffer(number(view, "buffer"))?;
        let start = number(view, "byteOffset") + number(a, "byteOffset");
        let mut out = Vec::with_capacity(count * width);
        for i in 0..count {
            let from = start + i * stride;
            let bytes = buffer
                .get(from..from + element)
                .ok_or_else(|| format!("{at} reads past its buffer"))?;
            out.extend(bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])));
        }
        if !out.iter().all(|v| v.is_finite()) {
            return Err(format!("{at}: values must be finite"));
        }
        Ok(out)
    }
}

fn base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut bits, mut have) = (0u32, 0);
    for c in text.bytes().filter(|&c| c != b'=') {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        bits = bits << 6 | u32::from(v);
        have += 6;
        if have >= 8 {
            have -= 8;
            out.push((bits >> have) as u8);
            bits &= (1 << have) - 1;
        }
    }
    Some(out)
}

fn sampler_keys(
    buffers: &Buffers<'_>,
    sampler: &Value,
    kind: Kind,
    at: &str,
    notes: &mut Vec<String>,
) -> Result<Option<Vec<Key>>, String> {
    let index = |key: &str| {
        sampler
            .get(key)
            .and_then(Value::as_u64)
            .map(|v| v as usize)
            .ok_or_else(|| format!("{at}: the sampler has no {key}"))
    };
    let times = buffers.floats(index("input")?, 1).map_err(|e| format!("{at}: {e}"))?;
    let width = if kind == Kind::Rotation { 4 } else { 3 };
    let output = buffers.floats(index("output")?, width).map_err(|e| format!("{at}: {e}"))?;
    let (interp, cubic) = match sampler.get("interpolation").and_then(Value::as_str).unwrap_or("LINEAR") {
        "LINEAR" => (Interp::Linear, false),
        "STEP" => (Interp::Step, false),
        "CUBICSPLINE" => (Interp::Smooth, true),
        other => return Err(format!("{at}: unknown interpolation {other:?}")),
    };
    let per_key = if cubic { 3 } else { 1 };
    if output.len() != times.len() * per_key * width {
        return Err(format!("{at}: the sampler's input and output counts differ"));
    }
    let take = |slot: usize| -> [f32; 4] {
        let mut v = [0.0; 4];
        v[..width].copy_from_slice(&output[slot * width..(slot + 1) * width]);
        v
    };
    let mut keys: Vec<Key> = Vec::new();
    let mut folded = 0;
    let mut scale_dropped = false;
    for (i, &time) in times.iter().enumerate() {
        let frame = (f64::from(time) * HZ).round();
        if !(0.0..=f64::from(i16::MAX)).contains(&frame) {
            return Err(format!("{at}: time {time} is outside 0 to {} frames", i16::MAX));
        }
        let frame = frame as usize;
        if keys.last().is_some_and(|k| frame <= k.frame) {
            folded += 1;
            continue;
        }
        let (value, slope) = if cubic {
            (take(i * 3 + 1), take(i * 3 + 2))
        } else {
            (take(i), [0.0; 4])
        };
        let (value, tangent) = match kind {
            Kind::Rotation => {
                let q = normalize(value).ok_or_else(|| format!("{at}: rotation must be nonzero"))?;
                let t = axes_q(slope);
                (axes_q(q), t)
            }
            Kind::Translation => {
                let v = axes([value[0], value[1], value[2]]);
                let t = axes([slope[0], slope[1], slope[2]]);
                ([v[0], v[1], v[2], 0.0], [t[0], t[1], t[2], 0.0])
            }
            _ => {
                if !uniform([value[0], value[1], value[2]]) {
                    scale_dropped = true;
                    break;
                }
                ([value[0], 0.0, 0.0, 0.0], [slope[0], 0.0, 0.0, 0.0])
            }
        };
        keys.push(Key {
            frame,
            value,
            interp,
            tangent: tangent.map(|t| (f64::from(t) / HZ) as f32),
        });
    }
    if scale_dropped {
        notes.push(format!("{at}: non-uniform scale is not supported, skipped"));
        return Ok(None);
    }
    if folded > 0 {
        notes.push(format!("{at}: {folded} keys fell on a frame already keyed and were dropped"));
    }
    if cubic {
        notes.push(format!("{at}: cubic spline in-tangents are replaced by the out-tangents"));
    }
    Ok(Some(keys))
}

#[cfg(test)]
#[path = "gltf/tests.rs"]
mod tests;
