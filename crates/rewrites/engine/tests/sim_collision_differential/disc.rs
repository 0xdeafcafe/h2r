//! Ladder step 3 for the point and location queries. Both arms read the
//! same tag payload at its original virtual address. The report contains
//! counts and decoded sizes.
use super::*;
use map::{FourCC, Map};
use sim_collision::bsp::block;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use tags::collision::{BSP_SIZE, Bsp as TagBsp, COLL, COLL_ROOT_SIZE};
use tags::geometry::TagView;

const SBSP: FourCC = FourCC(*b"sbsp");
const STRUCTURE_POINTS: u32 = 10_000;
const OBJECT_POINTS: u32 = 1_000;

fn word(record: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(record[offset..offset + 4].try_into().unwrap())
}

/// The BSP pointer entry A takes starts at the first block descriptor.
/// In a coll element that is the word after the node/flags header.
fn bsp_headers<'a>(view: &TagView<'a>, group: FourCC, address: u32) -> Vec<&'a [u8]> {
    if group == SBSP {
        let root = view.range(address, 0x1c).unwrap();
        return view.block(root, 0x14, TagBsp::STRUCTURE_SIZE).unwrap();
    }
    // An ordinary tag can have a zero-sized index entry whose address is
    // a real root in another tag. Its children can also be shared.
    let root = view.range(address, COLL_ROOT_SIZE as u64).unwrap();
    let mut headers = Vec::new();
    for region in view.block(root, 0x1c, 0xc).unwrap() {
        for permutation in view.block(region, 4, 0x14).unwrap() {
            for element in view.block(permutation, 4, BSP_SIZE).unwrap() {
                headers.push(&element[4..]);
            }
        }
    }
    headers
}

#[test]
fn ordinary_alias_roots_and_shared_blocks_resolve_in_map_space() {
    const BASE: u32 = 0x1000;
    // Two tag roots name the same region. A zero-index-size alias can
    // name either root without owning any of its bytes.
    let mut bytes = [0; 0x180];
    for (descriptor, address) in [
        (0x20 + 0x1c, BASE + 0xa0),
        (0x60 + 0x1c, BASE + 0xa0),
        (0xa0 + 4, BASE + 0xc0),
        (0xc0 + 4, BASE + 0x100),
    ] {
        bytes[descriptor..descriptor + 4].copy_from_slice(&1u32.to_le_bytes());
        bytes[descriptor + 4..descriptor + 8].copy_from_slice(&address.to_le_bytes());
    }
    for root in [BASE + 0x20, BASE + 0x60, BASE + 0x20] {
        let view = TagView::new(&bytes, BASE);
        let headers = bsp_headers(&view, COLL, root);
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].as_ptr(), bytes[0x104..].as_ptr());
        assert!(Samples::new(&view, headers[0]).vertices.is_empty());
    }
}

/// Sample in the BSP's own coordinate system. One point in eight lands
/// exactly on a vertex. The others sample the surrounding volume.
struct Samples<'a> {
    vertices: &'a [u8],
    missing_vertices: bool,
    low: [f32; 3],
    high: [f32; 3],
}

impl<'a> Samples<'a> {
    fn new(view: &TagView<'a>, header: &[u8]) -> Self {
        let field = 8 * block::VERTICES;
        let bytes = u64::from(word(header, field)) * u64::from(block::STRIDE[block::VERTICES]);
        // Sampling the optional bounding box must not panic before the
        // adapter can report a source outside this tag mapping.
        let range = view.range(word(header, field + 4), bytes);
        let missing_vertices = range.is_err();
        let vertices = range.unwrap_or_default();
        let mut low = [f32::INFINITY; 3];
        let mut high = [f32::NEG_INFINITY; 3];
        for v in vertices.chunks_exact(16) {
            for axis in 0..3 {
                let x = f32::from_bits(word(v, 4 * axis));
                if x.is_finite() {
                    low[axis] = low[axis].min(x);
                    high[axis] = high[axis].max(x);
                }
            }
        }
        for axis in 0..3 {
            if !low[axis].is_finite() {
                (low[axis], high[axis]) = (-1.0, 1.0);
            }
        }
        Self {
            vertices,
            missing_vertices,
            low,
            high,
        }
    }

    fn point(&self, rng: &mut Rng, n: u32) -> [f32; 3] {
        if n.is_multiple_of(8) && !self.vertices.is_empty() {
            let at = 16 * rng.below((self.vertices.len() / 16) as u32) as usize;
            return std::array::from_fn(|i| f32::from_bits(word(self.vertices, at + i * 4)));
        }
        std::array::from_fn(|i| {
            let min = f64::from(self.low[i]);
            let extent = f64::from(self.high[i]) - min;
            // One eighth of the extent outside each face. A flat axis
            // still samples both sides of the plane.
            let pad = if extent == 0.0 { 1.0 } else { extent * 0.125 };
            let unit = f64::from(rng.next()) / f64::from(u32::MAX);
            (min - pad + unit * (extent + 2.0 * pad)) as f32
        })
    }
}

#[derive(Default)]
struct Counts {
    empty: u64,
    missing_vertices: u64,
    decoded: u64,
    refused: u64,
    native: u64,
    locations: u64,
    bails: [u64; sim_collision::BAILS],
}

/// A and B reproduce their stack writes. A store of the byte already
/// present is equivalent to leaving that byte alone.
fn same_memory(native: &Ram<'_>, guest: &Ram<'_>, label: &str, n: u32) {
    if native.overlay != guest.overlay {
        for p in native.touched(guest, STACK, (STACK, STACK)) {
            assert_eq!(
                native.get(p),
                guest.get(p),
                "{label} point {n}: byte at {p:#x}"
            );
        }
    }
}

fn same_cpu(c: &CpuAbi, ret: TransferAbi, expected: &Cpu, label: &str, n: u32) {
    let mut native = ir_cpu(c);
    native.eip = ret.target_pc;
    assert_eq!(&native, expected, "{label} point {n}: CPU");
}

/// Maxima sampled after each BSP and (separately) cluster-table admission.
#[derive(Default)]
struct Residency {
    charged: u64,
    arrays: u64,
    dropped_rings: bool,
    budget_bails: u64,
    live_table_reads: u64,
}

impl Residency {
    fn sample(&mut self, world: &CollisionWorld, bsp: u32) {
        let stats = world.stats();
        assert!(
            stats.bytes <= stats.budget_bytes,
            "BSP {bsp:#010x}: {} bytes retained, limit {}",
            stats.bytes,
            stats.budget_bytes
        );
        self.charged = self.charged.max(stats.bytes);
        self.arrays = self.arrays.max(stats.array_bytes);
        self.dropped_rings |= stats.ring_fallbacks != 0;
        self.live_table_reads = self.live_table_reads.max(stats.cluster_budget_reads);
        let bails = stats
            .entries
            .iter()
            .map(|entry| entry.bails[sim_collision::Bail::Budget as usize])
            .sum::<u64>();
        self.budget_bails = self.budget_bails.max(bails);
    }
}

/// Re-run entry A per unique header, then B for an active structure.
#[derive(Clone, Copy)]
struct Probe {
    bsp: u32,
    point: [f32; 3],
}

impl Probe {
    fn warm(
        self,
        image: &[u8],
        mapping: (u32, &[u8], TableVersion),
        structure: bool,
        world: &CollisionWorld,
        residency: &mut Residency,
    ) {
        let mut ram = fresh(image);
        ram.tag = Some(mapping);
        put_vector(&mut ram, POINT, self.point);
        let entry = CpuAbi {
            esp: STACK,
            edx: POINT,
            ecx: self.bsp,
            esi: LOCATION,
            mxcsr: 0x1fa0,
            ..CpuAbi::default()
        };
        let mut cpu = entry;
        let _ = adapter::point_to_leaf(&mut cpu, &mut ram, world).unwrap();
        residency.sample(world, self.bsp);
        if structure {
            // B can give up expanded rings to fit the packed leaf table.
            // Its final footprint can be smaller than A's intermediate one.
            ram.word(layout::BSP_INDEX, 3);
            ram.word(layout::STRUCTURE_BSP, self.bsp);
            ram.word(layout::ACTIVE_STRUCTURE, mapping.0);
            let mut cpu = entry;
            let _ = adapter::location(&mut cpu, &mut ram, world).unwrap();
            residency.sample(world, self.bsp);
        }
    }
}

static EPOCH: Epoch = Epoch::new();

fn sweep_bsp(
    oracle: &mut Oracle<'_>,
    image: &[u8],
    view: &TagView<'_>,
    header: &[u8],
    bsp: u32,
    structure: bool,
    counts: &mut Counts,
    label: &str,
) -> Probe {
    let mut ram = fresh(image);
    // The archive uses high virtual tag addresses. Keep both the immutable
    // tag and the synthetic low RAM accessible under their actual addresses.
    assert!(
        view.address() as usize >= image.len(),
        "{label}: tag overlaps low RAM"
    );
    ram.tag = Some((view.address(), view.bytes(), TableVersion::fresh()));
    if structure {
        ram.word(layout::BSP_INDEX, 3); // synthetic active structure index
        ram.word(layout::STRUCTURE_BSP, bsp);
        ram.word(layout::ACTIVE_STRUCTURE, view.address());
    }
    let mut rng = Rng((0x9e37_79b9_7f4a_7c15 ^ u64::from(bsp)) | 1);
    let mut entry = entry_cpu(&mut rng, POINT_TO_LEAF.point.base);
    entry.mxcsr = 0x1fa0;
    entry.eax = 0;
    entry.ecx = bsp;
    entry.edx = POINT;

    // Two structure tags may occupy the same virtual range in different
    // BSP loads. Keep this world warm across points in exactly one BSP.
    let world = CollisionWorld::new(&EPOCH);
    let points = Samples::new(view, header);
    counts.missing_vertices += u64::from(points.missing_vertices);
    // A header without a root still exercises the decoder. There is no
    // valid root query to ask the guest.
    let empty = word(header, 8 * block::NODES) == 0;
    let samples = if structure {
        STRUCTURE_POINTS
    } else {
        OBJECT_POINTS
    };
    let attempts = if empty { 1 } else { samples };
    let mut gm = ram.clone();
    let mut probe = Probe {
        bsp,
        point: [0.0; 3],
    };
    for n in 0..attempts {
        let point = points.point(&mut rng, n);
        put_vector(&mut ram, POINT, point);
        put_vector(&mut gm, POINT, point);
        let mut c = entry;
        let got = adapter::point_to_leaf(&mut c, &mut ram, &world).unwrap();
        let Some(ret) = got else {
            assert_eq!(entry, c, "{label} point {n}: decline changed registers");
            same_memory(&ram, &gm, label, n);
            continue;
        };
        assert!(!empty, "{label}: empty tree answered a root query");
        let mut reference = ir_cpu(&entry);
        oracle.run(&mut reference, &mut gm, entry.eip);
        same_cpu(&c, ret, &reference, label, n);
        same_memory(&ram, &gm, label, n);
        // Exercise B against the real leaf-to-cluster table as well.
        if structure && c.eax != u32::MAX {
            // Carry a non-solid sample from anywhere in the A sweep so B
            // exercises the cluster table in the shared-world probes.
            probe.point = point;
        }
        if structure && n.is_multiple_of(31) {
            let mut c = entry;
            c.eip = LOCATION_ENTRY.point.base;
            c.esi = LOCATION;
            let mut reference = ir_cpu(&c);
            let poison = [0xa5; 8];
            ram.store(LOCATION, &poison);
            gm.store(LOCATION, &poison);
            let ret = adapter::location(&mut c, &mut ram, &world)
                .unwrap()
                .unwrap_or_else(|| panic!("{label} point {n}: location declined"));
            oracle.run(&mut reference, &mut gm, LOCATION_ENTRY.point.base);
            same_cpu(&c, ret, &reference, label, n);
            same_memory(&ram, &gm, label, n);
            counts.locations += 1;
        }
    }
    let stats = world.stats();
    let point = &stats.entries[sim_collision::Entry::PointToLeaf as usize];
    counts.empty += u64::from(empty);
    counts.decoded += stats.bsps;
    counts.refused += u64::from(stats.bsps == 0);
    counts.native += point.native;
    for (count, bails) in counts.bails.iter_mut().zip(point.bails) {
        *count += bails;
    }
    assert_eq!(
        point.native + point.bails.iter().sum::<u64>(),
        u64::from(attempts),
        "{label}: each sample is either compared or reported as a bail",
    );
    probe
}

#[test]
#[ignore = "reads the owner's Halo 2 ISO (H2_DISC) and XBE (H2_XBE)"]
fn every_disc_bsp_point_matches_the_guest() {
    assert!(
        common::xbe_path().is_some(),
        "set H2_XBE to run the disc sweep"
    );
    let disc = std::env::var_os("H2_DISC")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../disc/halo2.iso")
        });
    with_oracle("collision point disc sweep", |oracle, image| {
        let mut iso = xiso::Xiso::open(BufReader::new(File::open(&disc).unwrap())).unwrap();
        let part = iso.partition_offset();
        let mut entries = iso.entries().unwrap();
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        let (mut structure_points, mut object_points, mut locations) = (0, 0, 0);
        for file in entries {
            let path = file.path.to_ascii_lowercase();
            if file.is_dir || !path.starts_with("/maps/") || !path.ends_with(".map") {
                continue;
            }
            let mut map = Map::from_region(
                BufReader::new(File::open(&disc).unwrap()),
                part + u64::from(file.sector) * xiso::SECTOR,
                u64::from(file.size),
            )
            .unwrap();
            let mut tags: Vec<_> = map
                .tags()
                .iter()
                .filter(|t| t.group == COLL || t.group == SBSP)
                .map(|t| (t.id, t.group, t.location))
                .collect();
            // Read the ordinary address space once. It owns aliased roots
            // and blocks outside an individual coll tag's index extent.
            let metadata = map.metadata_space().map(|space| {
                let bytes = map
                    .read_file_range(space.file_offset, u64::from(space.size))
                    .unwrap();
                (space.virtual_base, bytes)
            });
            // Warm the same ordinary BSPs for every active structure. Two
            // different structures may reuse a virtual address.
            tags.sort_by_key(|(_, group, _)| *group == SBSP);
            let mut resident_world = CollisionWorld::new(&EPOCH);
            let mut residency = Residency::default();
            let mut had_structure = false;
            let mut object_probes: Vec<Probe> = Vec::new();
            let mut seen_objects = BTreeSet::new();
            let mut shared_headers = 0u64;
            let mut structure = Counts::default();
            let mut objects = Counts::default();
            let mut external = 0;
            for (id, group, location) in tags {
                let Some(location) = location else {
                    // References without local storage contribute no BSP payload.
                    external += 1;
                    continue;
                };
                let structure_tag = group == SBSP;
                let raw;
                let view = if structure_tag {
                    raw = map.read_tag_bytes(id).unwrap();
                    TagView::new(&raw, location.address)
                } else {
                    let (base, bytes) = metadata.as_ref().expect("coll tag has ordinary metadata");
                    TagView::new(bytes, *base)
                };
                if structure_tag {
                    if had_structure {
                        resident_world = CollisionWorld::new(&EPOCH);
                        if let Some((base, bytes)) = &metadata {
                            let mapping = (*base, bytes.as_slice(), TableVersion::fresh());
                            for probe in &object_probes {
                                probe.warm(image, mapping, false, &resident_world, &mut residency);
                            }
                        }
                    }
                    had_structure = true;
                }
                let mapping = (view.address(), view.bytes(), TableVersion::fresh());
                let mut seen_structure = BTreeSet::new();
                let seen = if structure_tag {
                    &mut seen_structure
                } else {
                    &mut seen_objects
                };
                let counts = if structure_tag {
                    &mut structure
                } else {
                    &mut objects
                };
                for (i, h) in bsp_headers(&view, group, location.address)
                    .into_iter()
                    .enumerate()
                {
                    let offset = h.as_ptr() as usize - view.bytes().as_ptr() as usize;
                    let bsp = view
                        .address()
                        .checked_add(offset.try_into().unwrap())
                        .unwrap();
                    if !seen.insert(bsp) {
                        shared_headers += 1;
                        continue;
                    }
                    let probe = sweep_bsp(
                        oracle,
                        image,
                        &view,
                        h,
                        bsp,
                        structure_tag,
                        counts,
                        &format!("{path} {group} {id} BSP {i}"),
                    );
                    probe.warm(
                        image,
                        mapping,
                        structure_tag,
                        &resident_world,
                        &mut residency,
                    );
                    if !structure_tag {
                        object_probes.push(probe);
                    }
                }
            }
            eprintln!(
                "{path}: fixed immutable mapping (ordinary BSPs + one active structure), \
                max sampled charged bytes = {}, max sampled array bytes = {}, budget = {}, \
                external references = {external}, shared BSP headers = {shared_headers}",
                residency.charged,
                residency.arrays,
                resident_world.stats().budget_bytes
            );
            eprintln!(
                "  shared-world probes: dropped rings = {}, \
                max budget bails = {}, max live table reads for budget = {}",
                residency.dropped_rings, residency.budget_bails, residency.live_table_reads
            );
            for (kind, c) in [("structure", &structure), ("object", &objects)] {
                eprintln!(
                    "  {kind}: {} decoded, {} refused, {} empty, {} missing vertex ranges, {} native points",
                    c.decoded, c.refused, c.empty, c.missing_vertices, c.native
                );
                for why in sim_collision::Bail::ALL {
                    let count = c.bails[why as usize];
                    if count > 0 {
                        eprintln!("    {}: {count}", why.name());
                    }
                }
            }
            eprintln!("  {} locations compared", structure.locations);
            locations += structure.locations;
            structure_points += structure.native;
            object_points += objects.native;
        }
        assert!(
            structure_points > 0,
            "no structure point reached the oracle"
        );
        assert!(object_points > 0, "no object point reached the oracle");
        assert!(locations > 0, "no location reached the oracle");
    });
}
