//! Invented sounds: ADPCM blocks built here, no game data.
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use map::{RawPointer, ResourceFile, TagId};
use tags::sound::{Gestalt, SoundHeader};

use super::*;

const WAIT: Duration = Duration::from_secs(10);

/// One mono Xbox ADPCM block (36 bytes) whose first sample is `first`.
fn block(first: i16) -> Vec<u8> {
    let mut b = first.to_le_bytes().to_vec();
    b.extend([0, 0]);
    b.extend([0u8; 32]);
    b
}

/// A header: `rate` 1 is 44.1 kHz, 0 is 22.05 kHz; `encoding` 0 mono, 2
/// codec; `compression` 1 Xbox ADPCM, 4 WMA.
fn header(rate: u8, encoding: u8, compression: u8) -> SoundHeader {
    let mut b = [0u8; 20];
    b[3] = rate;
    b[4] = encoding;
    b[5] = compression;
    b[10] = 1;
    SoundHeader::decode(&b).unwrap()
}

/// Sounds held in memory: per tag a header and per permutation its chunks.
struct Memory {
    headers: HashMap<TagId, SoundHeader>,
    chunks: HashMap<(TagId, u16, u16), Vec<Vec<u8>>>,
    reads: Arc<AtomicUsize>,
}

impl Memory {
    fn new() -> Self {
        Self {
            headers: HashMap::new(),
            chunks: HashMap::new(),
            reads: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn with(mut self, tag: u32, h: SoundHeader, perm: u16, chunks: Vec<Vec<u8>>) -> Self {
        self.headers.insert(TagId(tag), h);
        self.chunks.insert((TagId(tag), 0, perm), chunks);
        self
    }
}

impl Library for Memory {
    fn header(&self, tag: TagId) -> Result<SoundHeader, ClipError> {
        self.headers
            .get(&tag)
            .cloned()
            .ok_or_else(|| ClipError::Failed("no such sound".into()))
    }

    fn chunk_count(&self, key: &ClipKey) -> Result<usize, ClipError> {
        self.chunks
            .get(&(key.tag, key.range, key.permutation))
            .map(Vec::len)
            .ok_or_else(|| ClipError::Failed("no such permutation".into()))
    }

    fn read_chunk(&self, key: &ClipKey, index: usize) -> Result<Vec<u8>, ClipError> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        Ok(self.chunks[&(key.tag, key.range, key.permutation)][index].clone())
    }
}

fn key(tag: u32, permutation: u16) -> ClipKey {
    ClipKey {
        tag: TagId(tag),
        range: 0,
        permutation,
    }
}

#[test]
fn an_adpcm_permutation_decodes_at_the_headers_rate() {
    for (rate_code, hz) in [(0u8, 22050), (1, 44100)] {
        let lib = Memory::new().with(1, header(rate_code, 0, 1), 0, vec![block(9), block(-9)]);
        let source = ClipSource::new(lib, None, ClipSource::DEFAULT_BUDGET);
        let clip = source.wait(key(1, 0), WAIT).unwrap();
        assert_eq!((clip.rate, clip.channels, clip.frames()), (hz, 1, 128));
        assert_eq!((clip.pcm[0], clip.pcm[64]), (9, -9));
    }
}

#[test]
fn a_codec_stream_without_tables_is_refused_by_name() {
    for compression in [1u8, 4] {
        let h = header(1, 2, compression);
        assert_eq!(
            decode(&h, &[0; 64], None),
            Err(ClipError::Refused(NEEDS_EXECUTABLE.into()))
        );
        let lib = Memory::new().with(1, h, 0, vec![vec![0; 64]]);
        let source = ClipSource::new(lib, None, ClipSource::DEFAULT_BUDGET);
        assert_eq!(
            source.get(key(1, 0)).unwrap_err(),
            ClipError::Refused(NEEDS_EXECUTABLE.into())
        );
        assert!(matches!(
            source.stream(key(1, 0)),
            Err(ClipError::Refused(m)) if m == NEEDS_EXECUTABLE
        ));
        // Counted, and the refusal is the answer every time after.
        assert!(source.get(key(1, 0)).is_err());
        assert_eq!(source.stats().refused_without_tables, 2);
    }
    // An executable that is not the verified one never supplies tables.
    assert!(matches!(
        load_tables(b"XBEH not the game"),
        Err(ClipError::Refused(_))
    ));
}

#[test]
fn an_unknown_compression_is_refused_not_guessed() {
    for compression in [0u8, 2, 3, 7] {
        let h = header(1, 0, compression);
        let Err(ClipError::Refused(why)) = decode(&h, &[0; 36], None) else {
            panic!("compression {compression} was not refused");
        };
        assert!(why.contains("not decoded"), "{why}");
        let lib = Memory::new().with(1, h, 0, vec![block(1)]);
        let source = ClipSource::new(lib, None, ClipSource::DEFAULT_BUDGET);
        assert!(matches!(source.get(key(1, 0)), Err(ClipError::Refused(_))));
    }
    // An unknown rate selector is named too, not defaulted.
    assert!(matches!(
        decode(&header(9, 0, 1), &block(1), None),
        Err(ClipError::Refused(m)) if m.contains("sample rate")
    ));
}

#[test]
fn a_key_is_decoded_once_and_shared() {
    let lib = Memory::new().with(1, header(1, 0, 1), 0, vec![block(5)]);
    let reads = Arc::clone(&lib.reads);
    let source = ClipSource::new(lib, None, ClipSource::DEFAULT_BUDGET);
    let a = source.wait(key(1, 0), WAIT).unwrap();
    let b = source.wait(key(1, 0), WAIT).unwrap();
    let Got::Ready(c) = source.get(key(1, 0)).unwrap() else {
        panic!("a decoded key answers Ready");
    };
    assert!(Arc::ptr_eq(&a, &b) && Arc::ptr_eq(&a, &c));
    assert_eq!(reads.load(Ordering::Relaxed), 1);
    assert_eq!(source.stats().decoded, 1);
}

#[test]
fn the_cache_never_drops_a_clip_a_voice_holds() {
    // Each clip is 64 samples = 128 bytes; the budget holds one.
    let mut lib = Memory::new();
    for p in 0..4 {
        lib = lib.with(1, header(1, 0, 1), p, vec![block(p as i16)]);
    }
    let source = ClipSource::new(lib, None, 128);
    let held = source.wait(key(1, 0), WAIT).unwrap();
    for p in 1..4 {
        source.wait(key(1, p), WAIT).unwrap();
    }
    // Over budget, but the held clip is still the cached one.
    let Got::Ready(again) = source.get(key(1, 0)).unwrap() else {
        panic!("the held clip stayed cached");
    };
    assert!(Arc::ptr_eq(&held, &again));
    let stats = source.stats();
    assert_eq!(stats.decoded, 4);
    assert!(stats.evicted >= 2, "{stats:?}");
    // An unheld clip was dropped and decodes again.
    drop(again);
    let before = source.stats().decoded;
    source.wait(key(1, 1), WAIT).unwrap();
    assert_eq!(source.stats().decoded, before + 1);
    // Once released, the clip can go.
    drop(held);
    source.wait(key(1, 2), WAIT).unwrap();
    assert!(source.stats().cached_bytes <= 256);
}

#[test]
fn a_long_permutation_streams_chunk_by_chunk() {
    // Four blocks, cut so a block spans a chunk boundary.
    let whole: Vec<u8> = (0..4).flat_map(|i| block(100 + i)).collect();
    let cuts = [50usize, 22, 36, 36];
    let mut chunks = Vec::new();
    let mut at = 0;
    for n in cuts {
        chunks.push(whole[at..at + n].to_vec());
        at += n;
    }
    let lib = Memory::new().with(1, header(1, 0, 1), 0, chunks);
    let source = ClipSource::new(lib, None, ClipSource::DEFAULT_BUDGET);
    let mut reader = source.stream(key(1, 0)).unwrap();
    assert_eq!((reader.rate, reader.channels), (44100, 1));
    let mut sizes = Vec::new();
    let mut pcm = Vec::new();
    while let Some(c) = reader.next_chunk().unwrap() {
        sizes.push(c.pcm.len());
        pcm.extend(c.pcm);
    }
    // Chunk 0 holds one whole block, chunk 1 completes the second, then one each.
    assert_eq!(sizes, vec![64, 64, 64, 64]);
    let all = decode(&header(1, 0, 1), &whole, None).unwrap();
    assert_eq!(pcm, all.pcm.to_vec());
    // A permutation that ends mid-block is a named failure.
    let lib = Memory::new().with(1, header(1, 0, 1), 0, vec![vec![0; 40]]);
    let source = ClipSource::new(lib, None, ClipSource::DEFAULT_BUDGET);
    let mut reader = source.stream(key(1, 0)).unwrap();
    assert_eq!(reader.next_chunk().unwrap().unwrap().pcm.len(), 64);
    assert!(matches!(reader.next_chunk(), Err(ClipError::Failed(m)) if m.contains("whole block")));
}

/// An invented gestalt: one pitch range, one permutation of `chunks`
/// chunks of one ADPCM block each, packed from offset 0x10 of the local
/// resource file.
fn gestalt(chunks: u32) -> Vec<u8> {
    const BASE: u32 = 0x8000_0000;
    let mut b = vec![0u8; 0x58];
    let put = |b: &mut Vec<u8>, idx: usize, count: u32, body: &[u8]| {
        let at = BASE + b.len() as u32;
        b[idx * 8..idx * 8 + 4].copy_from_slice(&count.to_le_bytes());
        b[idx * 8 + 4..idx * 8 + 8].copy_from_slice(&at.to_le_bytes());
        b.extend_from_slice(body);
    };
    let mut range = vec![0u8; 8];
    range.extend([0, 0, 1, 0]);
    put(&mut b, 4, 1, &range);
    let mut perm = vec![0u8; 8];
    perm.extend((chunks * 36).to_le_bytes());
    perm.extend([0, 0]);
    perm.extend((chunks as u16).to_le_bytes());
    put(&mut b, 5, 1, &perm);
    let mut body = Vec::new();
    for i in 0..chunks {
        body.extend((0x10 + i * 36).to_le_bytes());
        body.extend(36u32.to_le_bytes());
        body.extend(u32::MAX.to_le_bytes());
    }
    put(&mut b, 8, chunks, &body);
    b
}

#[test]
fn a_gestalt_library_reads_chunks_from_the_resource_files() {
    let h = header(1, 0, 1);
    let g = Gestalt::decode(&gestalt(3), 0x8000_0000).unwrap();
    let mut file = vec![0xeeu8; 0x10];
    file.extend([block(11), block(22), block(33)].concat());
    let mut files = assets::ResourceFiles::new();
    files.set_bytes(ResourceFile::Local, file.clone().into());
    let sound = g.resolve(&h).unwrap();
    let p = sound.ranges[0].permutations[0];
    let direct = decode_permutation(&h, &g, &p, &mut Raw(&mut files), None).unwrap();
    assert_eq!(
        (direct.pcm[0], direct.pcm[64], direct.pcm[128]),
        (11, 22, 33)
    );

    struct Bytes(Vec<u8>);
    impl ResourceReader for Bytes {
        fn check(&self, p: RawPointer, size: u32) -> tags::Result<()> {
            if (p.offset + size) as usize > self.0.len() {
                return Err(tags::Error::Invalid("past the end"));
            }
            Ok(())
        }
        fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
            self.check(p, size)?;
            Ok(self.0[p.offset as usize..(p.offset + size) as usize].to_vec())
        }
    }
    let mut lib = GestaltLibrary::new(g, Box::new(Bytes(file)));
    lib.add(TagId(7), h);
    let source = ClipSource::new(lib, None, ClipSource::DEFAULT_BUDGET);
    let clip = source.wait(key(7, 0), WAIT).unwrap();
    assert_eq!(clip.pcm, direct.pcm);
    // A permutation the sound does not have is a named failure, kept.
    assert!(matches!(
        source.wait(key(7, 5), WAIT),
        Err(ClipError::Failed(m)) if m.contains("no such permutation")
    ));
}
