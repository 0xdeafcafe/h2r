//! Synthetic sound-tag tests: every byte below is invented, no game data.
use super::*;

const BASE: u32 = 0x8000_0000;

/// Invented gestalt: 1 pitch range -> 2 permutations -> 3 chunks.
fn gestalt_bytes(second_size: u32) -> Vec<u8> {
    let mut b = vec![0u8; GESTALT_HEADER];
    let put_block = |b: &mut Vec<u8>, idx: usize, count: u32, body: &[u8]| {
        let addr = BASE + b.len() as u32;
        b[idx * 8..idx * 8 + 4].copy_from_slice(&count.to_le_bytes());
        b[idx * 8 + 4..idx * 8 + 8].copy_from_slice(&addr.to_le_bytes());
        b.extend_from_slice(body);
    };
    let mut range = vec![0u8; 8];
    range.extend([0, 0, 2, 0]); // first permutation 0, count 2
    put_block(&mut b, BLOCK_PITCH_RANGES, 1, &range);
    let mut perms = Vec::new();
    for (size, first, count) in [(36u32, 0u16, 1u16), (second_size, 1, 2)] {
        perms.extend([0u8; 8]);
        perms.extend(size.to_le_bytes());
        perms.extend(first.to_le_bytes());
        perms.extend(count.to_le_bytes());
    }
    put_block(&mut b, BLOCK_PERMUTATIONS, 2, &perms);
    let mut chunks = Vec::new();
    for (ptr, size) in [
        (0x800u32, 36u32),
        (0x4000_1000, 36),
        (0x8000_0020, 0x8000_0000 | 36),
    ] {
        chunks.extend(ptr.to_le_bytes());
        chunks.extend(size.to_le_bytes());
        chunks.extend(u32::MAX.to_le_bytes());
    }
    put_block(&mut b, BLOCK_CHUNKS, 3, &chunks);
    b
}
fn header(first: u16, count: u8) -> SoundHeader {
    let mut b = [0u8; 20];
    b[2] = 0x32;
    b[3] = 1;
    b[4] = 1;
    b[5] = 1;
    b[8..10].copy_from_slice(&first.to_le_bytes());
    b[10] = count;
    b[16..20].copy_from_slice(&352u32.to_le_bytes());
    SoundHeader::decode(&b).unwrap()
}

#[test]
fn header_fields() {
    let h = header(3, 2);
    assert_eq!(h.sample_rate.hz(), Some(44100));
    assert_eq!(h.encoding, Encoding::Stereo);
    assert_eq!(h.compression, Compression::XboxAdpcm);
    assert_eq!((h.first_pitch_range, h.pitch_range_count, h.max_play_time), (3, 2, 352));
    assert!(SoundHeader::decode(&[0; 19]).is_err());
}

#[test]
fn resolves_chunks_with_selectors_and_flag() {
    // Permutation 0 is chunk 0 (36 bytes); permutation 1 is chunks 1 and 2.
    let g = Gestalt::decode(&gestalt_bytes(72), BASE).unwrap();
    let s = g.resolve(&header(0, 1)).unwrap();
    let perms = &s.ranges[0].permutations;
    assert_eq!(perms.len(), 2);
    assert_eq!(g.chunks_of(&perms[0])[0].pointer.file, map::ResourceFile::Local);
    assert_eq!(g.chunks_of(&perms[1])[0].pointer.file, map::ResourceFile::MainMenu);
    assert_eq!(g.chunks_of(&perms[1])[0].pointer.offset, 0x1000);
    // Bit 31 of the size is a flag, not part of the size.
    assert!(g.chunks_of(&perms[1])[1].flag);
    assert_eq!(g.chunks_of(&perms[1])[1].size, 36);
    assert_eq!(g.chunks_of(&perms[1])[1].pointer.file, map::ResourceFile::Shared);
}

#[test]
fn rejects_inconsistent_sizes_and_indices() {
    // Chunk sizes sum to 72 for permutation 1; 73 must be refused.
    let bad = Gestalt::decode(&gestalt_bytes(73), BASE).unwrap();
    assert!(bad.resolve(&header(0, 1)).is_err());
    let g = Gestalt::decode(&gestalt_bytes(72), BASE).unwrap();
    assert!(g.resolve(&header(1, 1)).is_err());
    assert!(g.resolve(&header(0, 2)).is_err());
}

#[test]
fn rejects_out_of_bounds_blocks() {
    let mut b = gestalt_bytes(72);
    b.truncate(b.len() - 4);
    assert!(Gestalt::decode(&b, BASE).is_err());
    assert!(Gestalt::decode(&b[..0x20], BASE).is_err());
    // A pointer below the tag's own base is rejected, not wrapped.
    assert!(Gestalt::decode(&gestalt_bytes(72), BASE + 0x1000).is_err());
}

#[test]
fn reads_chunks_through_reader() {
    struct Fake;
    impl ResourceReader for Fake {
        fn check(&self, _: RawPointer, _: u32) -> Result<()> {
            Ok(())
        }
        fn read_raw(&mut self, p: RawPointer, size: u32) -> Result<Vec<u8>> {
            Ok(vec![p.offset as u8; size as usize])
        }
    }
    let g = Gestalt::decode(&gestalt_bytes(72), BASE).unwrap();
    let s = g.resolve(&header(0, 1)).unwrap();
    let data = g.read_encoded(&s.ranges[0].permutations[1], &mut Fake).unwrap();
    assert_eq!(data.len(), 72);
    assert_eq!(data[0], 0x00);
    assert_eq!(data[36], 0x20);
}

#[test]
fn aliased_metadata_is_bounded() {
    // One chunk, referenced by 65,535 permutations in one range: the permutation
    // budget must refuse before per-permutation work is multiplied out.
    let mut g = Gestalt::decode(&gestalt_bytes(72), BASE).unwrap();
    let perm = g.permutations[0];
    g.permutations = vec![perm; 65_535];
    g.pitch_ranges[0].first_permutation = 0;
    g.pitch_ranges[0].permutation_count = 65_535;
    assert!(g.resolve(&header(0, 1)).is_err());
    // Chunk budget: few permutations that each claim every shared chunk.
    g.chunks = vec![g.chunks[0]; 65_535];
    g.permutations = vec![Permutation { chunk_count: 65_535, first_chunk: 0, ..perm }; 4];
    g.pitch_ranges[0].permutation_count = 4;
    assert!(g.resolve(&header(0, 1)).is_err());
}

#[test]
fn oversized_declared_permutation_is_refused_before_allocation() {
    struct Reject(bool);
    impl ResourceReader for Reject {
        fn check(&self, _: RawPointer, _: u32) -> Result<()> {
            if self.0 { Err(Error::Invalid("outside file")) } else { Ok(()) }
        }
        fn read_raw(&mut self, _: RawPointer, _: u32) -> Result<Vec<u8>> {
            panic!("must not read");
        }
    }
    let mut g = Gestalt::decode(&gestalt_bytes(72), BASE).unwrap();
    g.chunks[0].size = 0x7fff_ffff;
    let mut p = g.permutations[0];
    p.encoded_size = 0x7fff_ffff;
    assert!(g.read_encoded(&p, &mut Reject(true)).is_err());
    // In-file but over the in-memory cap: refused without reading.
    assert!(g.read_encoded(&p, &mut Reject(false)).is_err());
}

/// The extra-info resource as an `hklb` block, laid out as the converter
/// writes it and retail holds it: a header, the curve data and the
/// descriptors, each found by its locator.
fn mouth_resource(curves: &[&[u8]]) -> Vec<u8> {
    let mut data = Vec::new();
    let mut descriptors = Vec::new();
    for curve in curves {
        descriptors.extend_from_slice(&(data.len() as u32).to_le_bytes());
        descriptors.extend_from_slice(&(curve.len() as u32).to_le_bytes());
        // No facial sequence.
        descriptors.extend_from_slice(&[0; 8]);
        data.extend_from_slice(curve);
    }
    let block_start = (4 + data.len()).next_multiple_of(4) + 4;
    let resource_size = block_start + descriptors.len();
    let mut raw = b"hklb".to_vec();
    for value in [resource_size as u32, data.len() as u32, 0, curves.len() as u32, 0] {
        raw.extend_from_slice(&value.to_le_bytes());
    }
    raw.extend_from_slice(b"crsr");
    raw.extend_from_slice(&data);
    raw.resize(24 + block_start - 4, 0);
    raw.extend_from_slice(b"crsr");
    raw.extend_from_slice(&descriptors);
    raw.extend_from_slice(b"fklb");
    raw
}

/// The local resource file: the extra info's pointer names 0x1000.
struct Resource(Vec<u8>);
impl ResourceReader for Resource {
    fn check(&self, p: RawPointer, size: u32) -> Result<()> {
        let end = u64::from(p.offset) + u64::from(size);
        if p.file != map::ResourceFile::Local
            || p.offset != 0x1000
            || end > 0x1000 + self.0.len() as u64
        {
            return Err(Error::Invalid("outside the resource file"));
        }
        Ok(())
    }
    fn read_raw(&mut self, p: RawPointer, size: u32) -> Result<Vec<u8>> {
        self.check(p, size)?;
        Ok(self.0[p.offset as usize - 0x1000..][..size as usize].to_vec())
    }
}

/// Where the extra-info element and its locators sit in the bytes below.
const EXTRA_AT: usize = 0x84;
const LOCATORS_AT: usize = EXTRA_AT + 0x2c;

/// An invented gestalt: one range, two permutations (descriptor indices 0
/// and 1) and one extra info whose pointer names 0x1000. `curves[0]` and
/// `curves[1]` are their mouth curves; an empty one is unvoiced. Returns
/// the gestalt bytes and the resource.
fn mouth_gestalt(curves: &[&[u8]]) -> (Vec<u8>, Vec<u8>) {
    let resource = mouth_resource(curves);
    let mut b = vec![0u8; GESTALT_HEADER];
    let put_block = |b: &mut Vec<u8>, idx: usize, body: &[u8]| {
        let addr = BASE + b.len() as u32;
        b[idx * 8..idx * 8 + 4].copy_from_slice(&1u32.to_le_bytes());
        b[idx * 8 + 4..idx * 8 + 8].copy_from_slice(&addr.to_le_bytes());
        b.extend_from_slice(body);
    };
    let mut range = vec![0u8; 8];
    range.extend([0, 0, 2, 0]); // two permutations
    put_block(&mut b, BLOCK_PITCH_RANGES, &range);
    let mut perms = Vec::new();
    for i in 0u8..2 {
        let mut e = [0u8; 16];
        e[5] = i; // the extra-info descriptor index
        perms.extend_from_slice(&e);
    }
    put_block(&mut b, BLOCK_PERMUTATIONS, &perms);
    // The extra-info element lands at 0x84, its locators after it.
    assert_eq!(b.len(), EXTRA_AT);
    let data_len: u32 = curves.iter().map(|c| c.len() as u32).sum();
    let descriptors_len = (curves.len() * 16) as u32;
    let block_start = (4 + data_len as usize).next_multiple_of(4) + 4;
    let mut e = [0u8; 0x2c];
    e[8..12].copy_from_slice(&0x1000u32.to_le_bytes());
    e[0xc..0x10].copy_from_slice(&(resource.len() as u32).to_le_bytes());
    e[0x10..0x14].copy_from_slice(&16u32.to_le_bytes());
    e[0x14..0x18].copy_from_slice(&((block_start + descriptors_len as usize) as u32).to_le_bytes());
    e[0x18..0x1c].copy_from_slice(&2u32.to_le_bytes());
    e[0x1c..0x20].copy_from_slice(&(BASE + LOCATORS_AT as u32).to_le_bytes());
    e[0x24..0x28].copy_from_slice(&0x0100_fff8u32.to_le_bytes());
    e[0x28..0x2c].copy_from_slice(&u32::MAX.to_le_bytes());
    b.extend_from_slice(&e);
    for (head, size, offset) in [
        ([1, 2, 0, 0, 0, 0, 0xff, 0xff], data_len, 4u32),
        ([0, 2, 8, 0, 8, 0, 16, 0], descriptors_len, block_start as u32),
    ] {
        b.extend_from_slice(&head);
        b.extend_from_slice(&size.to_le_bytes());
        b.extend_from_slice(&offset.to_le_bytes());
    }
    b[BLOCK_EXTRA_INFOS * 8..BLOCK_EXTRA_INFOS * 8 + 4].copy_from_slice(&1u32.to_le_bytes());
    b[BLOCK_EXTRA_INFOS * 8 + 4..BLOCK_EXTRA_INFOS * 8 + 8]
        .copy_from_slice(&(BASE + EXTRA_AT as u32).to_le_bytes());
    (b, resource)
}

fn mouth_header(extra_info: u16) -> SoundHeader {
    let mut b = [0u8; 20];
    b[10] = 1; // one pitch range
    b[0xe..0x10].copy_from_slice(&extra_info.to_le_bytes());
    SoundHeader::decode(&b).unwrap()
}

#[test]
fn one_voiced_and_one_unvoiced_permutation_give_one_curve_and_one_null() {
    let (bytes, resource) = mouth_gestalt(&[&[10, 200, 255], &[]]);
    let g = Gestalt::decode(&bytes, BASE).unwrap();
    let h = mouth_header(0);
    assert_eq!(h.extra_info, 0);
    let mut r = Resource(resource);
    assert_eq!(g.mouth_curve(&h, &g.permutations[0], &mut r).unwrap(), Some(vec![10, 200, 255]));
    assert_eq!(g.mouth_curve(&h, &g.permutations[1], &mut r).unwrap(), None);
}

#[test]
fn a_sound_without_extra_info_or_a_negative_descriptor_has_no_curve() {
    let (bytes, resource) = mouth_gestalt(&[&[7], &[]]);
    let g = Gestalt::decode(&bytes, BASE).unwrap();
    // 0xffff is none.
    let none = mouth_header(u16::MAX);
    assert_eq!(
        g.mouth_curve(&none, &g.permutations[0], &mut Resource(resource.clone())).unwrap(),
        None
    );
    // A negative descriptor index is no descriptor.
    let h = mouth_header(0);
    let mut p = g.permutations[0];
    p.raw[5] = 0xff;
    assert_eq!(g.mouth_curve(&h, &p, &mut Resource(resource)).unwrap(), None);
}

#[test]
fn a_bad_extra_info_or_resource_is_refused_by_name() {
    let (bytes, resource) = mouth_gestalt(&[&[1, 2, 3], &[]]);
    let g = Gestalt::decode(&bytes, BASE).unwrap();
    let h = mouth_header(0);
    // The index is out of the block.
    let out = mouth_header(4);
    assert!(g.mouth_curve(&out, &g.permutations[0], &mut Resource(resource.clone())).is_err());
    // The descriptor index is out of the table.
    let mut p = g.permutations[0];
    p.raw[5] = 5;
    assert!(g.mouth_curve(&h, &p, &mut Resource(resource.clone())).is_err());
    // A resource that is not an hklb block.
    let mut not_hklb = resource.clone();
    not_hklb[0] = b'X';
    assert!(g.mouth_curve(&h, &g.permutations[0], &mut Resource(not_hklb)).is_err());
    // A curve that runs outside the data: descriptor 0's length word. The
    // descriptors start after the resource's header (24), the data and the
    // second crsr: 24 + block_start.
    let mut long = resource.clone();
    let block_start = (4usize + 3).next_multiple_of(4) + 4;
    let at = 24 + block_start + 4;
    long[at..at + 4].copy_from_slice(&9u32.to_le_bytes());
    assert!(g.mouth_curve(&h, &g.permutations[0], &mut Resource(long)).is_err());
    // A locator that is not the known one.
    let mut wrong_head = bytes.clone();
    wrong_head[LOCATORS_AT] = 9;
    let g = Gestalt::decode(&wrong_head, BASE).unwrap();
    assert!(g.mouth_curve(&h, &g.permutations[0], &mut Resource(resource)).is_err());
}

#[test]
fn an_extra_info_with_an_unknown_child_block_is_refused() {
    let (mut bytes, _) = mouth_gestalt(&[&[1], &[]]);
    // The child block at +0 has no known layout: present is refused.
    bytes[EXTRA_AT..EXTRA_AT + 4].copy_from_slice(&1u32.to_le_bytes());
    assert!(Gestalt::decode(&bytes, BASE).is_err());
}
