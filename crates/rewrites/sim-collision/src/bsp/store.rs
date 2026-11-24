//! Immutable, content-addressed BSP columns. Guest addresses and the watch
//! generation belong to the caller. Only guest indices and decoded values
//! cross a process boundary.
use super::{Bsp, Consts, HEADER_WORDS, Leaf, Node, Node2d, Ref, RingEdge, Surface, block, ring};
use bytemuck::Pod;
use memmap2::{Mmap, MmapOptions};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    ops::{Deref, Range},
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};

// Small objects cost more to map and index on disk than to decode privately.
pub(super) const MIN_BYTES: u64 = 64 << 10;
const MAGIC: &[u8; 8] = b"H2BSP001";
const HEADER: usize = 96;
const ALIGN: usize = 32;

/// A decoded array backed by a private allocation or a checked read-only file range.
#[derive(Debug)]
pub struct Column<T: Pod>(Backing<T>);

#[derive(Debug)]
enum Backing<T: Pod> {
    Owned(Vec<T>),
    Mapped(Arc<Mmap>, Range<usize>),
}

impl<T: Pod> From<Vec<T>> for Column<T> {
    fn from(value: Vec<T>) -> Self {
        Self(Backing::Owned(value))
    }
}

impl<T: Pod> FromIterator<T> for Column<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Vec::from_iter(iter).into()
    }
}

impl<T: Pod> Deref for Column<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        match &self.0 {
            Backing::Owned(values) => values,
            Backing::Mapped(file, span) => bytemuck::cast_slice(&file[span.clone()]),
        }
    }
}

impl<T: Pod> Column<T> {
    fn mapped(map: &Arc<Mmap>, span: Range<usize>) -> Option<Self> {
        bytemuck::try_cast_slice::<u8, T>(map.get(span.clone())?).ok()?;
        Some(Self(Backing::Mapped(map.clone(), span)))
    }

    pub(super) fn capacity(&self) -> usize {
        match &self.0 {
            Backing::Owned(values) => values.capacity(),
            Backing::Mapped(_, range) => range.len() / size_of::<T>(),
        }
    }
}

struct Layout {
    primary: [Range<usize>; 7],
    edges: Range<usize>,
    groups: Range<usize>,
    primary_len: usize,
    optional_len: usize,
}

impl Layout {
    fn overhead(&self) -> usize {
        self.primary_len - self.primary.iter().map(Range::len).sum::<usize>()
    }

    fn fits(&self, room: u64) -> bool {
        (self.overhead() + self.optional_len) as u64 <= room
    }

    fn new(header: &[u32; HEADER_WORDS], edges: u32, groups: u32) -> Option<Self> {
        fn span(next: &mut usize, count: u32, bytes: usize) -> Option<Range<usize>> {
            let start = next.checked_next_multiple_of(ALIGN)?;
            *next = start.checked_add((count as usize).checked_mul(bytes)?)?;
            Some(start..*next)
        }
        let count = |i: usize| header[2 * i];
        let mut primary = HEADER;
        let ranges = [
            span(&mut primary, count(block::NODES), size_of::<Node>())?,
            span(&mut primary, count(block::PLANES), 16)?,
            span(&mut primary, count(block::PLANES), 1)?,
            span(&mut primary, count(block::LEAVES), size_of::<Leaf>())?,
            span(&mut primary, count(block::REFS), size_of::<Ref>())?,
            span(&mut primary, count(block::NODES_2D), size_of::<Node2d>())?,
            span(&mut primary, count(block::SURFACES), size_of::<Surface>())?,
        ];
        let mut optional = 0;
        Some(Self {
            primary: ranges,
            edges: span(&mut optional, edges, size_of::<RingEdge>())?,
            groups: span(&mut optional, groups, size_of::<ring::EdgeGroup>())?,
            primary_len: primary.checked_next_multiple_of(ALIGN)?,
            optional_len: optional,
        })
    }
}

pub(super) struct Key {
    path: PathBuf,
    digest: [u8; 32],
}

impl Key {
    pub(super) fn new(dir: &Path, raw: &[Vec<u8>; 8], consts: &Consts) -> Self {
        let mut hash = Sha256::new();
        hash.update(MAGIC);
        hash.update([u8::from(sim_core::det::host_fast_env())]);
        hash.update(consts.zero.to_le_bytes());
        for value in consts.projection.iter().flatten() {
            hash.update(value.to_le_bytes());
        }
        for block in raw {
            hash.update((block.len() as u64).to_le_bytes());
            hash.update(block);
        }
        let digest = hash.finalize();
        Self {
            path: dir.join(format!("{digest:x}.bsp")),
            digest: digest.into(),
        }
    }

    pub(super) fn open(
        &self,
        addr: u32,
        header: [u32; HEADER_WORDS],
        consts: &Consts,
        hash: u64,
        ring_limit: u64,
    ) -> Option<Bsp> {
        use std::io::Read;
        let mut file = File::open(&self.path).ok()?;
        let mut head = [0; HEADER];
        file.read_exact(&mut head).ok()?;
        if head[..8] != MAGIC[..] || head[8..40] != self.digest || head[80..] != [0; 16] {
            return None;
        }
        let edges = super::word(&head, 40);
        let groups = super::word(&head, 44);
        let layout = Layout::new(&header, edges, groups)?;
        if !layout.fits(ring_limit) {
            return None;
        }
        if file.metadata().ok()?.len()
            != layout.primary_len.checked_add(layout.optional_len)? as u64
        {
            return None;
        }
        // Writers publish new inodes by rename and never modify a published
        // inode. The optional mapping can be released while kernels retain
        // references to the essential columns.
        let primary =
            Arc::new(unsafe { MmapOptions::new().len(layout.primary_len).map(&file).ok()? });
        let optional = if layout.optional_len == 0 {
            None
        } else {
            Some(Arc::new(unsafe {
                MmapOptions::new()
                    .offset(layout.primary_len as u64)
                    .len(layout.optional_len)
                    .map(&file)
                    .ok()?
            }))
        };
        let mut verifier = Sha256::new();
        verifier.update(&head[..48]);
        verifier.update(&primary[HEADER..]);
        if let Some(bytes) = &optional {
            verifier.update(&bytes[..]);
        }
        if verifier.finalize()[..] != head[48..80] {
            return None;
        }
        let mapped_overhead = layout.overhead();
        let mapped_padding = layout.optional_len - layout.edges.len() - layout.groups.len();
        let (edges, packed) = match optional {
            Some(bytes) => (
                Column::mapped(&bytes, layout.edges)?,
                Column::mapped(&bytes, layout.groups)?,
            ),
            None => (Vec::new().into(), Vec::new().into()),
        };
        let [nodes, planes, axis, leaves, refs, nodes_2d, surfaces] = layout.primary;
        Some(Bsp {
            addr,
            header,
            hash,
            mapped_overhead,
            nodes: Column::mapped(&primary, nodes)?,
            planes: Column::mapped(&primary, planes)?,
            axis: Column::mapped(&primary, axis)?,
            leaves: Column::mapped(&primary, leaves)?,
            refs: Column::mapped(&primary, refs)?,
            nodes_2d: Column::mapped(&primary, nodes_2d)?,
            surfaces: Column::mapped(&primary, surfaces)?,
            projection: consts.projection.map(super::projection_axes),
            ring: RwLock::new(Some(ring::Rings {
                edges,
                packed,
                complete: true,
                mapped_padding,
            })),
        })
    }

    pub(super) fn publish(&self, bsp: &Bsp, ring_limit: u64) -> io::Result<()> {
        let held = bsp.ring.read().unwrap_or_else(|err| err.into_inner());
        let rings = held
            .as_ref()
            .filter(|r| r.complete)
            .ok_or_else(|| io::Error::other("partial ring cache"))?;
        let edges = u32::try_from(rings.edges.len()).map_err(io::Error::other)?;
        let groups = u32::try_from(rings.packed.len()).map_err(io::Error::other)?;
        let layout = Layout::new(&bsp.header, edges, groups)
            .ok_or_else(|| io::Error::other("column size"))?;
        if !layout.fits(ring_limit) {
            return Err(io::Error::other("mapped columns exceed budget"));
        }
        let mut bytes = vec![0; layout.primary_len + layout.optional_len];
        bytes[..8].copy_from_slice(MAGIC);
        bytes[8..40].copy_from_slice(&self.digest);
        bytes[40..44].copy_from_slice(&edges.to_le_bytes());
        bytes[44..48].copy_from_slice(&groups.to_le_bytes());
        let data: [&[u8]; 7] = [
            bytemuck::cast_slice(&bsp.nodes),
            bytemuck::cast_slice(&bsp.planes),
            &bsp.axis,
            bytemuck::cast_slice(&bsp.leaves),
            bytemuck::cast_slice(&bsp.refs),
            bytemuck::cast_slice(&bsp.nodes_2d),
            bytemuck::cast_slice(&bsp.surfaces),
        ];
        for (range, src) in layout.primary.into_iter().zip(data) {
            bytes[range].copy_from_slice(src);
        }
        let optional = &mut bytes[layout.primary_len..];
        optional[layout.edges].copy_from_slice(bytemuck::cast_slice(&rings.edges));
        optional[layout.groups].copy_from_slice(bytemuck::cast_slice(&rings.packed));
        let mut checksum = Sha256::new();
        checksum.update(&bytes[..48]);
        checksum.update(&bytes[HEADER..]);
        bytes[48..80].copy_from_slice(&checksum.finalize());
        drop(held);
        atomic_write(&self.path, &bytes)
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fs::create_dir_all(path.parent().unwrap())?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // PID and sequence distinguish simultaneous publishers. Skip leftovers
    // from terminated processes that had the same PID.
    let (tmp, mut file) = loop {
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        let tmp = path.with_extension(format!("{}.{number}.tmp", std::process::id()));
        match options.open(&tmp) {
            Ok(file) => break (tmp, file),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    };
    let result = file.write_all(bytes);
    drop(file);
    let result = result.and_then(|_| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(all(test, target_endian = "little"))]
mod tests {
    use super::*;
    use crate::synth::{self, Builder, CONSTS, Image, Rng};

    fn fixture(at: u32, b: &Builder) -> Image {
        let mut image = Image::new(at, b.size() as usize);
        b.write(at, |addr, bytes| image.put(addr, bytes));
        image
    }

    fn read(image: &Image, dir: Option<&Path>, room: u64) -> Bsp {
        let at = image.base;
        Bsp::decode_cached(
            image,
            at,
            Bsp::read_header(image, at).unwrap(),
            &CONSTS,
            room,
            dir,
        )
        .unwrap()
    }

    #[test]
    fn mapped_columns_rebind_guest_addresses_and_repair_damaged_files() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = loop {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("h2-bsp-columns-{}-{n}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => break path,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("{e}"),
            }
        };
        let mut rng = Rng(0x53e27d591);
        let mut b = synth::random(&mut rng, 4);
        b.planes.resize(4096, [1.0, 2.0, 3.0, 4.0]); // enough data to share
        let first = fixture(0x1000, &b);
        let moved = fixture(0x1_0000, &b);
        let room = crate::world::MAP_BUDGET;
        let lazy = read(&moved, None, room);
        let original = read(&first, Some(&dir), room);
        let shared = read(&moved, Some(&dir), room);
        assert!(matches!(original.nodes.0, Backing::Mapped(..)));
        assert!(matches!(shared.nodes.0, Backing::Mapped(..)));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        assert_ne!(original.plane_addr(0), shared.plane_addr(0));
        assert_ne!(original.source_hash(), shared.source_hash());
        assert_eq!(shared.plane_addr(0), lazy.plane_addr(0));
        assert_eq!(&*shared.nodes, &*lazy.nodes);
        assert_eq!(&*shared.planes, &*lazy.planes);
        assert_eq!(&*shared.axis, &*lazy.axis);
        assert_eq!(&*shared.leaves, &*lazy.leaves);
        assert_eq!(&*shared.refs, &*lazy.refs);
        assert_eq!(&*shared.nodes_2d, &*lazy.nodes_2d);
        assert_eq!(&*shared.surfaces, &*lazy.surfaces);
        for index in 0..shared.surfaces.len() as u32 {
            assert_eq!(
                shared.read_surface_ring(&moved, index),
                lazy.read_surface_ring(&moved, index)
            );
        }
        for _ in 0..64 {
            let point = std::array::from_fn(|_| rng.eighths(16).to_bits());
            assert_eq!(shared.point_to_leaf(0, point), lazy.point_to_leaf(0, point));
        }
        assert!(read(&moved, Some(&dir), 0).scalar_rings());
        let path = fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
        let valid = fs::read(&path).unwrap();
        for damage in [0, 8, 40, 48, HEADER, valid.len() - 1] {
            let mut bad = valid.clone();
            bad[damage] ^= 0x80;
            atomic_write(&path, &bad).unwrap();
            let fresh = read(&moved, Some(&dir), room);
            assert_eq!(fs::read(&path).unwrap(), valid, "repaired byte {damage}");
            assert_eq!(&*fresh.surfaces, &*lazy.surfaces);
        }
        atomic_write(&path, &valid[..12]).unwrap();
        assert_eq!(&*read(&moved, Some(&dir), room).planes, &*lazy.planes);
        assert_eq!(fs::read(&path).unwrap(), valid);
        b.planes[0][3] = 713.0;
        let edited = fixture(first.base, &b);
        let changed = read(&edited, Some(&dir), room);
        assert_ne!(changed.planes[0], original.planes[0]);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
        fs::remove_dir_all(dir).unwrap();
    }
}
