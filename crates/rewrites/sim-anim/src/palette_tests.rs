//! The palette kernel against hand-built palettes: the identity fill, the
//! overlay order, the sentinels and the negative slots.
use crate::palette::*;

const ONE: u32 = 0x3f80_0000;

fn identity() -> [u8; NODE_BYTES] {
    core::array::from_fn(|i| 0x70 + i as u8)
}

/// One 96-byte model node: recognizable bytes in the translation and
/// rotation fields.
fn node(tag: u8) -> Vec<u8> {
    let mut n = vec![0u8; MODEL_NODE_BYTES];
    for (k, b) in n.iter_mut().enumerate() {
        let at = k as u32;
        *b = if (0xc..0x18).contains(&at) {
            0x10 + tag
        } else if (0x18..0x28).contains(&at) {
            0x20 + tag
        } else {
            0
        };
    }
    n
}

#[test]
fn an_empty_step_writes_nothing() {
    let out = build(&identity(), 0, ONE, &[]);
    assert_eq!(
        out,
        Palette {
            first: 0,
            bytes: Vec::new()
        }
    );
    let out = build(&identity(), -2, ONE, &[]);
    assert_eq!(
        out,
        Palette {
            first: 0,
            bytes: Vec::new()
        }
    );
}

#[test]
fn the_fill_covers_the_slots_with_the_identity() {
    let out = build(&identity(), 3, ONE, &[]);
    assert_eq!(out.first, 0);
    assert_eq!(out.bytes.len(), 3 * NODE_BYTES);
    for slot in 0..3 {
        let at = slot * NODE_BYTES;
        assert_eq!(&out.bytes[at..at + NODE_BYTES], &identity());
    }
}

#[test]
fn an_overlay_lays_its_nodes_over_the_fill() {
    let nodes: Vec<u8> = [node(1), node(2), node(3)].concat();
    let overlay = Overlay {
        nodes: &nodes,
        indices: &[2, u16::MAX, 0],
    };
    let out = build(&identity(), 2, ONE, &[overlay]);
    assert_eq!(out.first, 0);
    assert_eq!(out.bytes.len(), 2 * NODE_BYTES);
    // Node 0 landed in slot 2, node 2 in slot 0; slot 1 kept the fill.
    let slot = |i: usize| &out.bytes[i * NODE_BYTES..(i + 1) * NODE_BYTES];
    assert_eq!(&slot(0)[..16], &nodes[2 * MODEL_NODE_BYTES + 0x18..][..16]);
    assert_eq!(&slot(0)[16..28], &nodes[2 * MODEL_NODE_BYTES + 0xc..][..12]);
    assert_eq!(&slot(1), &identity());
    assert_eq!(&slot(2)[..16], &nodes[0x18..0x28]);
    assert_eq!(&slot(2)[16..28], &nodes[0xc..0x18]);
    // The scale is the image's 1.0; the fourth word of the fill survives
    // nowhere an overlay landed.
    assert_eq!(&slot(0)[28..32], &ONE.to_le_bytes());
    assert_eq!(&slot(2)[28..32], &ONE.to_le_bytes());
    assert_eq!(&slot(1)[28..32], &identity()[28..32]);
}

#[test]
fn the_later_overlay_wins() {
    let one = [node(1)].concat();
    let two = [node(2)].concat();
    let overlays = [
        Overlay {
            nodes: &one,
            indices: &[0],
        },
        Overlay {
            nodes: &two,
            indices: &[0],
        },
    ];
    let out = build(&identity(), 1, ONE, &overlays);
    assert_eq!(&out.bytes[..16], &two[0x18..0x28]);
}

#[test]
fn a_negative_index_reaches_below_the_palette() {
    let nodes = [node(9)].concat();
    let overlay = Overlay {
        nodes: &nodes,
        indices: &[0x8000],
    };
    let out = build(&identity(), 2, ONE, &[overlay]);
    assert_eq!(out.first, -32768);
    assert_eq!(out.bytes.len(), (2 + 32768) * NODE_BYTES);
    let at = 32768 * NODE_BYTES;
    assert_eq!(&out.bytes[at..at + 16], &nodes[0x18..0x28]);
    // The fill still covers the positive slots.
    let fill = (out.bytes.len() - 2 * NODE_BYTES) as usize;
    assert_eq!(&out.bytes[fill..fill + NODE_BYTES], &identity());
}

#[test]
fn an_overlay_without_the_fill_spans_only_its_writes() {
    let nodes = [node(4), node(5)].concat();
    let overlay = Overlay {
        nodes: &nodes,
        indices: &[5, 6],
    };
    let out = build(&identity(), 0, ONE, &[overlay]);
    assert_eq!(out.first, 5);
    assert_eq!(out.bytes.len(), 2 * NODE_BYTES);
    assert_eq!(&out.bytes[..16], &nodes[0x18..0x28]);
}
