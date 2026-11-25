//! Locate code objects by following their owners' descriptors. The same
//! virtual address may refer to different structures on the same map.
use super::*;
use mopp::Code;
use tags::collision::{BSP_PHYSICS_SIZE, PHMO};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Structure,
    Cluster,
    CollisionModel,
    PhysicsModel,
}

fn blocks(view: &TagView<'_>, address: u32, group: FourCC, mut at: impl FnMut(Kind, u32, u32)) {
    let mut descriptor = |kind, record: &[u8], field| {
        let count = word(record, field);
        if count > 0 {
            at(kind, word(record, field + 4), count);
        }
    };
    if group == SBSP {
        let root = view.range(address, 0x20c).unwrap();
        descriptor(Kind::Structure, root, 0x1e0);
        descriptor(Kind::Structure, root, 0x204);
        for cluster in view.block(root, 0x9c, 0xb0).unwrap() {
            if (word(cluster, 0xa8) as i32) > 0 {
                descriptor(Kind::Cluster, cluster, 0xa8);
            }
        }
    } else if group == COLL {
        let root = view.range(address, COLL_ROOT_SIZE as u64).unwrap();
        for region in view.block(root, 0x1c, 0xc).unwrap() {
            for permutation in view.block(region, 4, 0x14).unwrap() {
                for slot in view.block(permutation, 0xc, BSP_PHYSICS_SIZE).unwrap() {
                    descriptor(Kind::CollisionModel, slot, 0x64);
                }
            }
        }
    } else {
        assert_eq!(group, PHMO);
        let root = view.range(address, 0xb0).unwrap();
        let length = word(root, 0xa8);
        let base = word(root, 0xac);
        let bytes = view.range(base, u64::from(length)).unwrap();
        let mut cursor = 0usize;
        for _ in view.block(root, 0xa0, 0x14).unwrap() {
            let size = word(&bytes[cursor..], 0x20);
            assert!(
                size as usize >= mopp::code::HEADER_SIZE,
                "short phmo MOPP run"
            );
            let addr = base.checked_add(cursor.try_into().unwrap()).unwrap();
            cursor = cursor
                .checked_add((size as usize).next_multiple_of(16))
                .unwrap();
            assert!(cursor <= bytes.len(), "phmo MOPP run crosses code block");
            at(Kind::PhysicsModel, addr, size);
        }
        assert_eq!(cursor, bytes.len(), "phmo MOPP runs did not consume block");
    }
}

pub(crate) fn for_each(
    kinds: &[Kind],
    mut visit: impl FnMut(&str, Kind, &TagView<'_>, u32, Code, Option<&Samples<'_>>),
) {
    let groups: Vec<_> = [SBSP, COLL, PHMO]
        .into_iter()
        .filter(|group| {
            if *group == SBSP {
                kinds.contains(&Kind::Structure) || kinds.contains(&Kind::Cluster)
            } else if *group == COLL {
                kinds.contains(&Kind::CollisionModel)
            } else {
                kinds.contains(&Kind::PhysicsModel)
            }
        })
        .collect();
    for_each_tag(&groups, |label, group, view, root| {
        let samples = if group == SBSP && kinds.contains(&Kind::Structure) {
            bsp_headers(view, group, root)
                .first()
                .map(|h| Samples::new(view, h))
        } else {
            None
        };
        blocks(view, root, group, |kind, at, size| {
            if kinds.contains(&kind) {
                let bytes = view.range(at, u64::from(size)).unwrap();
                let code =
                    Code::parse(bytes).unwrap_or_else(|e| panic!("{label} {kind:?} {at:#x}: {e}"));
                visit(label, kind, view, at, code, samples.as_ref());
            }
        });
    });
}

#[test]
fn phmo_uses_each_runs_padded_extent() {
    const BASE: u32 = 0x3000;
    let mut bytes = [0u8; 0x280];
    for (offset, value) in [
        (0xa0, 2),
        (0xa4, BASE + 0x120),
        (0xa8, 0x90),
        (0xac, BASE + 0x180),
        (0x180 + 0x20, 0x32),
        (0x1c0 + 0x20, 0x45),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    let mut got = Vec::new();
    blocks(&TagView::new(&bytes, BASE), BASE, PHMO, |kind, at, len| {
        got.push((kind, at, len))
    });
    assert_eq!(
        got,
        [
            (Kind::PhysicsModel, BASE + 0x180, 0x32),
            (Kind::PhysicsModel, BASE + 0x1c0, 0x45)
        ]
    );
}

#[test]
fn empty_clusters_do_not_dereference_stale_code_pointers() {
    const BASE: u32 = 0x3000;
    let mut bytes = [0u8; 0x400];
    for (offset, value) in [
        (0x9c, 2),
        (0xa0, BASE + 0x240),
        (0x240 + 0xac, u32::MAX),
        (0x2f0 + 0xa8, 0x40),
        (0x2f0 + 0xac, BASE + 0x3c0),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    let mut got = Vec::new();
    blocks(&TagView::new(&bytes, BASE), BASE, SBSP, |kind, at, len| {
        got.push((kind, at, len))
    });
    assert_eq!(got, [(Kind::Cluster, BASE + 0x3c0, 0x40)]);
}
