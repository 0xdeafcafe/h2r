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
