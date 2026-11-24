//! Share the integer MOPP's unfolded test records. Each mapping is bound to
//! the current code object's pointer and floating-ray source at acquisition.
use super::{NODES_PER_BYTE, NONE, Node, Source, Tree};
use crate::store::atomic_write;
use memmap2::{Mmap, MmapOptions};
use sha2::{Digest, Sha256};
use std::{fs::File, mem::size_of_val, path::Path, sync::Arc};

const MAGIC: &[u8; 8] = b"H2MOP001";
const HEADER: usize = 72;
const MIN_SOURCE: usize = 4096;

#[derive(Clone, Debug)]
pub(super) enum Nodes {
    Heap(Vec<Node>),
    File(Arc<Mmap>),
}

impl Nodes {
    pub(super) fn as_slice(&self) -> &[Node] {
        match self {
            Self::Heap(v) => v,
            Self::File(map) => {
                let rows = &map[HEADER..];
                // open checks the alignment, whole rows, and every discriminant
                // with bytemuck. Publication never mutates an existing inode.
                unsafe {
                    std::slice::from_raw_parts(
                        rows.as_ptr().cast::<Node>(),
                        rows.len() / size_of::<Node>(),
                    )
                }
            }
        }
    }

    pub(super) fn bytes(&self) -> usize {
        match self {
            Self::Heap(v) => v.capacity() * size_of::<Node>(),
            Self::File(map) => map.len(),
        }
    }
}

impl Node {
    // Copy only initialized fields. repr(u8) fixes the tag at offset zero
    // and makes each variant a repr(C) struct starting with that tag.
    fn encode(&self, row: &mut [u8]) {
        let start = std::ptr::from_ref(self).addr();
        let mut field = |ptr: usize, bits: &[u8]| {
            let at = ptr - start;
            row[at..at + bits.len()].copy_from_slice(bits);
        };
        macro_rules! fields {
            ($($value:ident),+) => {$(
                field(std::ptr::from_ref($value).addr(), &$value.to_le_bytes());
            )+};
        }
        let tag = match self {
            Node::Split {
                axis,
                below,
                above,
                low,
                high,
            } => {
                fields!(axis, below, above, low, high);
                0
            }
            Node::Diagonal {
                op,
                shift,
                below,
                above,
                low,
                high,
            } => {
                fields!(op, shift, below, above, low, high);
                1
            }
            Node::Bound {
                axis,
                above,
                below,
                next,
            } => {
                fields!(axis, above, below, next);
                2
            }
            Node::Leaf { key } => {
                fields!(key);
                3
            }
            Node::Trap { pc } => {
                fields!(pc);
                4
            }
        };
        row[0] = tag;
    }
}

fn checksum(bytes: &[u8]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(&bytes[..40]);
    hash.update(&bytes[HEADER..]);
    hash.finalize().into()
}

fn open(path: &Path, key: &[u8; 32], max_rows: usize, room: u64) -> Option<Nodes> {
    let file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len <= HEADER as u64
        || len > room
        || len - HEADER as u64 > (max_rows as u64 * size_of::<Node>() as u64)
    {
        return None;
    }
    // Atomic publication only, as for the BSP columns.
    let map = unsafe { MmapOptions::new().map(&file).ok()? };
    if &map[..8] != MAGIC || &map[8..40] != key || checksum(&map)[..] != map[40..HEADER] {
        return None;
    }
    let nodes = bytemuck::checked::try_cast_slice::<u8, Node>(&map[HEADER..]).ok()?;
    let mut parent = vec![false; nodes.len()];
    for (i, node) in nodes.iter().enumerate() {
        let mut link = |child: u32| {
            child == NONE
                || (child as usize > i
                    && parent
                        .get_mut(child as usize)
                        .is_some_and(|p| !std::mem::replace(p, true)))
        };
        let valid = match *node {
            Node::Split {
                axis, low, high, ..
            } => axis < 3 && link(low) && link(high),
            Node::Diagonal {
                op,
                shift,
                low,
                high,
                ..
            } => {
                (0x13..=0x1c).contains(&op)
                    && u32::from(shift) <= ::mopp::code::ROOT_SHIFT
                    && link(low)
                    && link(high)
            }
            Node::Bound { axis, next, .. } => axis < 3 && link(next),
            Node::Leaf { .. } | Node::Trap { .. } => true,
        };
        if !valid {
            return None;
        }
    }
    if parent[1..].contains(&false) {
        return None;
    }
    Some(Nodes::File(Arc::new(map)))
}

impl Tree {
    pub(crate) fn decode_shared(
        code: u32,
        source: Source,
        directory: Option<&Path>,
        budget: u64,
    ) -> Self {
        let Some(dir) = directory.filter(|_| {
            cfg!(all(target_endian = "little", target_pointer_width = "64"))
                && source.bytecode.len() >= MIN_SOURCE
        }) else {
            return Self::decode(code, source);
        };
        let mut hash = Sha256::new();
        hash.update(MAGIC);
        hash.update([size_of::<Node>() as u8, align_of::<Node>() as u8]);
        hash.update(&source.bytecode);
        let digest = hash.finalize();
        let path = dir.join(format!("{digest:x}.mopp"));
        let key = digest.into();
        let max_rows = NODES_PER_BYTE * source.bytecode.len();
        let room = budget.saturating_sub((size_of::<Self>() + source.bytecode.capacity()) as u64);
        if let Some(nodes) = open(&path, &key, max_rows, room) {
            return Self {
                code,
                source,
                nodes,
                root: 0,
                guest_only: false,
            };
        }
        let mut tree = Self::decode(code, source);
        let rows = tree.nodes();
        if rows.is_empty() || (size_of_val(rows) + HEADER) as u64 > room {
            return tree;
        }
        let mut bytes = vec![0; HEADER + size_of_val(rows)];
        bytes[..8].copy_from_slice(MAGIC);
        bytes[8..40].copy_from_slice(&key);
        for (node, row) in rows
            .iter()
            .zip(bytes[HEADER..].chunks_exact_mut(size_of::<Node>()))
        {
            node.encode(row);
        }
        let checksum = checksum(&bytes);
        bytes[40..HEADER].copy_from_slice(&checksum);
        if atomic_write(&path, &bytes).is_ok()
            && let Some(nodes) = open(&path, &key, max_rows, room)
        {
            tree.nodes = nodes;
        }
        tree
    }
}

#[cfg(all(test, target_endian = "little", target_pointer_width = "64"))]
mod tests {
    use super::*;
    use crate::mopp::Corners;
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    #[test]
    fn mapped_integer_walk_reuses_the_tree_at_a_new_code_pointer() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = loop {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("h2-mopp-tree-{}-{n}", std::process::id()));
            match fs::create_dir(&dir) {
                Ok(()) => break dir,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("{e}"),
            }
        };
        // A split with two different terminals; padding after the reached
        // program also participates in the code digest.
        let mut bytecode = vec![0x10, 80, 40, 1, 0x31, 0x33];
        bytecode.resize(MIN_SOURCE, 0);
        let source = Source {
            bytecode,
            origin: [0; 3],
            scale: 1f32.to_bits(),
        };
        let lazy = Tree::decode(0x31000, source.clone());
        let first = Tree::decode_shared(0x7000, source.clone(), Some(&dir), 1 << 20);
        let moved = Tree::decode_shared(0x31000, source.clone(), Some(&dir), 1 << 20);
        assert!(matches!(first.nodes, Nodes::File(..)));
        assert!(matches!(moved.nodes, Nodes::File(..)));
        assert_eq!(moved.span(), lazy.span());
        assert_ne!(moved.span(), first.span());
        assert_eq!(moved.nodes(), lazy.nodes());
        for k in 0..100 {
            let q = Corners {
                lo: [k << 16; 3],
                hi: [(k + 32) << 16; 3],
            };
            assert_eq!(moved.query_box(&q, 10), lazy.query_box(&q, 10));
        }
        let path = fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
        let valid = fs::read(&path).unwrap();
        let key: [u8; 32] = valid[8..40].try_into().unwrap();
        for node in [
            Node::Split {
                axis: 3,
                below: 0,
                above: 0,
                low: 1,
                high: NONE,
            },
            Node::Split {
                axis: 0,
                below: 0,
                above: 0,
                low: 0,
                high: NONE,
            },
            Node::Diagonal {
                op: 0x00,
                shift: 0,
                below: 0,
                above: 0,
                low: 1,
                high: 2,
            },
        ] {
            let mut bad = valid.clone();
            node.encode(&mut bad[HEADER..HEADER + size_of::<Node>()]);
            let sum = checksum(&bad);
            bad[40..HEADER].copy_from_slice(&sum);
            atomic_write(&path, &bad).unwrap();
            assert!(open(&path, &key, 100, 1 << 20).is_none());
        }
        let mut bad = valid.clone();
        bad[HEADER] = 0xfe;
        let sum = checksum(&bad);
        bad[40..HEADER].copy_from_slice(&sum);
        atomic_write(&path, &bad).unwrap();
        assert!(open(&path, &key, 100, 1 << 20).is_none());
        let repaired = Tree::decode_shared(0x31000, source, Some(&dir), 1 << 20);
        assert_eq!(repaired.nodes(), lazy.nodes());
        assert_eq!(fs::read(&path).unwrap(), valid);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn every_stored_variant_round_trips_without_reading_padding() {
        let all = [
            Node::Split {
                axis: 2,
                below: -413,
                above: 777,
                low: 3,
                high: 9,
            },
            Node::Diagonal {
                op: 0x15,
                shift: 4,
                below: 1234,
                above: 101,
                low: 6,
                high: 10,
            },
            Node::Bound {
                axis: 1,
                above: -43,
                below: 412,
                next: 123,
            },
            Node::Leaf { key: 0x889911 },
            Node::Trap { pc: 19 },
        ];
        // u64 backing grants the enum's required alignment.
        let mut storage = vec![0_u64; size_of_val(&all) / size_of::<u64>()];
        let bytes = bytemuck::cast_slice_mut::<_, u8>(&mut storage);
        for (node, row) in all.iter().zip(bytes.chunks_exact_mut(size_of::<Node>())) {
            node.encode(row);
        }
        assert_eq!(
            bytemuck::checked::try_cast_slice::<_, Node>(bytes).unwrap(),
            all
        );
    }
}
