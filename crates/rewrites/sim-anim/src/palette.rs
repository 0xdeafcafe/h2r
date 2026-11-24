//! The first-person bone palette: the identity fill and the model-node
//! overlay of `16760c` (docs/engine/palette.md; phase 1c of
//! docs/engine/sim-rewrite.md).
//!
//! A first-person seat owns a palette of 32-byte node records (a
//! quaternion, a translation, a scale), addressed by slot. `16760c`'s
//! palette step fills every slot from the identity record, then walks two
//! models' nodes and lays each node over the slot its palette index
//! names: the node's rotation, its translation, and the scale 1.0. A
//! `-1` index (the sentinel `0xffff`) leaves the slot as the fill left
//! it, and an index is a signed word, so a model can name slots below
//! the palette.
//!
//! [`build`] is pure byte work — no float arithmetic, so no fast-path
//! declines; the engine adapter (`engine::sim_anim::palette`) gathers
//! the identity record, the nodes and the indices, and writes the
//! palette span the fills and overlays cover, from the first slot
//! written to one past the last.

/// One palette record, 32 bytes: rotation `+0`, translation `+0x10`,
/// scale `+0x1c`.
pub const NODE_BYTES: usize = 32;
/// One model node, 96 bytes: the translation at `+0xc`, the rotation at
/// `+0x18`.
pub const MODEL_NODE_BYTES: usize = 96;

/// One model's nodes to lay over the palette: `nodes` holds `count`
/// 96-byte records and `indices` the palette slot each names (`0xffff`:
/// none).
#[derive(Clone, Copy)]
pub struct Overlay<'a> {
    pub nodes: &'a [u8],
    pub indices: &'a [u16],
}

/// The palette bytes the step writes, and the slot they start at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    pub first: i32,
    pub bytes: Vec<u8>,
}

/// The palette the guest's step leaves: every slot `0..slots` filled
/// with the 32-byte `identity` record (when `slots` is positive), then
/// each overlay's nodes written over the slots their indices name, the
/// later overlays winning. `one` is the image's 1.0 `[0x45dbc0]`, copied
/// into each overlaid record's scale.
pub fn build(
    identity: &[u8; NODE_BYTES],
    slots: i32,
    one: u32,
    overlays: &[Overlay<'_>],
) -> Palette {
    let mut first = if slots > 0 { 0 } else { i32::MAX };
    let mut last = slots.max(0);
    for o in overlays {
        for &index in o.indices {
            if index == u16::MAX {
                continue;
            }
            let slot = i32::from(index as i16);
            first = first.min(slot);
            last = last.max(slot + 1);
        }
    }
    if first == i32::MAX {
        return Palette {
            first: 0,
            bytes: Vec::new(),
        };
    }
    let mut bytes = vec![0u8; (last - first) as usize * NODE_BYTES];
    for slot in 0..slots {
        let at = (slot - first) as usize * NODE_BYTES;
        bytes[at..at + NODE_BYTES].copy_from_slice(identity);
    }
    for o in overlays {
        for (n, &index) in o.indices.iter().enumerate() {
            if index == u16::MAX {
                continue;
            }
            let slot = i32::from(index as i16);
            let node = n * MODEL_NODE_BYTES;
            let at = (slot - first) as usize * NODE_BYTES;
            bytes[at..at + 16].copy_from_slice(&o.nodes[node + 0x18..node + 0x28]);
            bytes[at + 16..at + 28].copy_from_slice(&o.nodes[node + 0xc..node + 0x18]);
            bytes[at + 28..at + 32].copy_from_slice(&one.to_le_bytes());
        }
    }
    Palette { first, bytes }
}
