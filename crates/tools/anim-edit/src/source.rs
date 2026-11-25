//! The package's sparse `.anim.yaml` source and its bake to codecs 1 and 3.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tags::anim::AnimationType;
use tags::anim_keys::Kind;
use tags::anim_sample::flag_array_bytes;
use tags::anim_write::{
    SelectedChannels, animated_stream_selected, flag_set_selected, static_stream_selected,
};

use crate::{AnimDoc, Interp, Key, Node};

/// An animation item joins an existing local graph. The graph path has no
/// group prefix. The skeleton is in graph order, including unkeyed nodes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub graph: String,
    pub name: String,
    pub operation: Operation,
    pub frames: usize,
    pub nodes: Vec<SourceNode>,
    #[serde(default)]
    pub channels: Vec<SourceChannel>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Replace {
        animation: String,
    },
    Add {
        mode: String,
        class: String,
        #[serde(default = "any_type")]
        weapon_type: String,
        action: String,
        #[serde(default)]
        kind: ClipKind,
    },
}

fn any_type() -> String {
    "any".to_owned()
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipKind {
    #[default]
    Base,
    Overlay,
    Replacement,
}

impl From<ClipKind> for AnimationType {
    fn from(kind: ClipKind) -> Self {
        match kind {
            ClipKind::Base => Self::Base,
            ClipKind::Overlay => Self::Overlay,
            ClipKind::Replacement => Self::Replacement,
        }
    }
}

impl TryFrom<AnimationType> for ClipKind {
    type Error = String;

    fn try_from(kind: AnimationType) -> Result<Self, Self::Error> {
        match kind {
            AnimationType::Base => Ok(Self::Base),
            AnimationType::Overlay => Ok(Self::Overlay),
            AnimationType::Replacement => Ok(Self::Replacement),
            AnimationType::Unknown(n) => Err(format!("unsupported animation type {n}")),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceNode {
    pub name: Option<String>,
    pub parent: i16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Rotation,
    Translation,
    Scale,
}

impl From<SourceKind> for Kind {
    fn from(kind: SourceKind) -> Self {
        match kind {
            SourceKind::Rotation => Self::Rotation,
            SourceKind::Translation => Self::Translation,
            SourceKind::Scale => Self::Scale,
        }
    }
}

impl From<Kind> for SourceKind {
    fn from(kind: Kind) -> Self {
        match kind {
            Kind::Rotation => Self::Rotation,
            Kind::Translation => Self::Translation,
            Kind::Scale => Self::Scale,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceChannel {
    pub node: usize,
    pub channel: SourceKind,
    pub keys: Vec<SourceKey>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceKey {
    pub frame: usize,
    pub value: Vec<f32>,
    #[serde(default)]
    pub interp: Interp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tangent: Option<Vec<f32>>,
}

impl Source {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        serde_yaml_ng::from_slice(bytes).map_err(|err| err.to_string())
    }

    pub fn to_yaml(&self) -> Result<String, String> {
        serde_yaml_ng::to_string(self).map_err(|err| err.to_string())
    }

    /// Save sparse keys without reducing or converting rotations to angles.
    pub fn from_doc(graph: String, operation: Operation, doc: &AnimDoc) -> Self {
        Self {
            graph,
            name: doc.name().to_owned(),
            operation,
            frames: doc.frames(),
            nodes: doc
                .nodes()
                .iter()
                .map(|node| SourceNode {
                    name: node.name.clone(),
                    parent: node.parent,
                })
                .collect(),
            channels: doc
                .channels()
                .map(|(node, kind, keys)| SourceChannel {
                    node,
                    channel: kind.into(),
                    keys: keys
                        .iter()
                        .map(|key| SourceKey {
                            frame: key.frame,
                            value: key.value[..kind.width()].to_vec(),
                            interp: key.interp,
                            tangent: (key.tangent != [0.0; 4])
                                .then(|| key.tangent[..kind.width()].to_vec()),
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    /// Validate the item and make a document with empty undo history.
    pub fn doc(&self) -> Result<AnimDoc, String> {
        name("graph", &self.graph)?;
        name("name", &self.name)?;
        if self.graph.contains(':') {
            return Err("graph is a tag path without a group prefix".to_owned());
        }
        match &self.operation {
            Operation::Replace { animation } => name("animation", animation)?,
            Operation::Add {
                mode,
                class,
                weapon_type,
                action,
                ..
            } => {
                for (key, value) in [
                    ("mode", mode),
                    ("class", class),
                    ("weapon_type", weapon_type),
                    ("action", action),
                ] {
                    name(key, value)?;
                }
            }
        }
        node_count(self.nodes.len())?;
        let nodes = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, node)| {
                if let Ok(parent) = usize::try_from(node.parent)
                    && (parent >= self.nodes.len() || parent == i)
                {
                    return Err(format!("node {i} has invalid parent {parent}"));
                }
                Ok(Node {
                    name: node.name.clone(),
                    parent: node.parent,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut doc = AnimDoc::new(self.name.clone(), self.frames, nodes)?;
        for channel in &self.channels {
            let kind = Kind::from(channel.channel);
            let track = (channel.node, kind);
            let keys = channel.keys().map_err(|err| channel.problem(err))?;
            doc.validate_keys(track, &keys)
                .map_err(|err| channel.problem(err))?;
            if doc.tracks.insert(track, Arc::new(keys)).is_some() {
                return Err(channel.problem("channel appears twice"));
            }
        }
        Ok(doc)
    }
}

fn name(field: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.as_bytes().contains(&0) {
        return Err(format!("{field} needs a nonempty name without NUL bytes"));
    }
    Ok(())
}

fn node_count(nodes: usize) -> Result<(), String> {
    if nodes == 0 {
        return Err("an animation graph needs at least one skeleton node".to_owned());
    }
    if nodes > usize::from(u8::MAX) {
        return Err(format!("a jmad animation has at most {} nodes", u8::MAX));
    }
    Ok(())
}

impl SourceChannel {
    fn problem(&self, message: impl std::fmt::Display) -> String {
        let kind = Kind::from(self.channel);
        format!("node {} {}: {message}", self.node, kind.name())
    }

    fn keys(&self) -> Result<Vec<Key>, String> {
        if self.keys.is_empty() {
            return Err("channel has no keys".to_owned());
        }
        let kind = Kind::from(self.channel);
        self.keys
            .iter()
            .map(|key| {
                let components = |field, values: &[f32]| {
                    if values.len() != kind.width() {
                        return Err(format!(
                            "frame {} {field} needs {} values",
                            key.frame,
                            kind.width()
                        ));
                    }
                    let mut out = [0.0; 4];
                    out[..values.len()].copy_from_slice(values);
                    Ok(out)
                };
                Ok(Key {
                    frame: key.frame,
                    value: components("value", &key.value)?,
                    interp: key.interp,
                    tangent: key
                        .tangent
                        .as_deref()
                        .map(|slope| components("tangent", slope))
                        .transpose()?
                        .unwrap_or([0.0; 4]),
                })
            })
            .collect()
    }
}

#[derive(Debug)]
pub struct Encoded {
    pub bytes: Vec<u8>,
    pub static_size: u16,
    pub animated_size: u32,
    /// Byte length of each of the two flag sets.
    pub flag_size: u8,
}

/// Bake sparse poses at integer frames. A base clip can put a constant
/// channel in codec 1. The engine reads only the animated stream for other
/// kinds, so even a constant keyed delta must use codec 3 there.
pub fn encode(doc: &AnimDoc, kind: ClipKind) -> Result<Encoded, String> {
    let nodes = doc.nodes().len();
    node_count(nodes)?;
    let flag_size = (3 * flag_array_bytes(nodes)) as u8;
    let sample = |node, channel, frame| {
        doc.value_at(node, channel, frame as f32)
            .expect("the selected channel has keys")
    };

    let mut static_channels = SelectedChannels::default();
    let mut animated_channels = SelectedChannels::default();
    for (node, channel, keys) in doc.channels() {
        let constant = kind == ClipKind::Base
            && (keys.len() == 1 || {
                let first = keys[0].value;
                (1..doc.frames()).all(|frame| {
                    let value = sample(node, channel, frame);
                    value == first || (channel == Kind::Rotation && value == first.map(|v| -v))
                })
            });
        let index = match channel {
            Kind::Rotation => 0,
            Kind::Translation => 1,
            Kind::Scale => 2,
        };
        if constant {
            static_channels[index].push(node);
        } else {
            animated_channels[index].push(node);
        }
    }

    let mut bytes = if static_channels.iter().any(|nodes| !nodes.is_empty()) {
        static_stream_selected(&static_channels, |node, channel| sample(node, channel, 0))
    } else {
        Vec::new()
    };
    // Even a graph with the maximum node count fits the static size's
    // signed 16-bit field and the animated size's 32-bit field.
    let static_size = bytes.len() as u16;
    let animated_size = if animated_channels.iter().any(|nodes| !nodes.is_empty()) {
        let animated = animated_stream_selected(doc.frames(), &animated_channels, false, sample);
        bytes.extend(&animated);
        animated.len() as u32
    } else {
        0
    };
    bytes.extend(flag_set_selected(&static_channels, nodes));
    bytes.extend(flag_set_selected(&animated_channels, nodes));
    Ok(Encoded {
        bytes,
        static_size,
        animated_size,
        flag_size,
    })
}

#[cfg(test)]
mod tests;
