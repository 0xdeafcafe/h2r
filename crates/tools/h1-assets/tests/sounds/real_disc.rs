//! Checks on the owner's discs, all `#[ignore]`d. No map bytes or samples
//! are kept: everything is read and compared in memory.
//!
//! ```text
//! H1_ISO=disc/halo1.iso cargo test -p h1sounds --release -- --ignored every_h1 --nocapture
//! H2_DISC=disc/halo2.iso cargo test -p h1sounds --release -- --ignored retail_gestalts --nocapture
//! H2_DISC=disc/halo2.iso cargo test -p h1sounds --release -- --ignored every_retail_sound_merges
//! H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso \
//!     cargo test -p h1sounds --release -- --ignored converted --nocapture
//! ```

#![allow(clippy::chunks_exact_to_as_chunks)]

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufReader, Cursor};

use h1map::H1Map;
use h1_assets::sounds::gestalt::{SoundGestalt, blocks};
use h1_assets::sounds::h1::{Compression, SND, Sound};
use h1_assets::sounds::{class, convert_sound, decode_all, install_sounds};
use map::build::TagSet;
use map::{FourCC, Map, RawPointer, ResourceFile, TagId};

fn h1_disc() -> xiso::Xiso<BufReader<std::fs::File>> {
    let path = std::env::var("H1_ISO").expect("set H1_ISO to the Halo 1 disc");
    xiso::Xiso::open(BufReader::new(std::fs::File::open(path).unwrap())).unwrap()
}

fn h1_maps() -> Vec<String> {
    let mut iso = h1_disc();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    d.maps
        .iter()
        .map(|e| h1map::disc::map_stem(e).to_string())
        .collect()
}

fn h1_map(name: &str) -> H1Map {
    let mut iso = h1_disc();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = d.map(name).expect("map on the disc").clone();
    H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

fn h2_disc() -> String {
    std::env::var("H2_DISC").expect("set H2_DISC to the Halo 2 disc")
}

fn h2_stems() -> Vec<String> {
    let mut iso =
        xiso::Xiso::open(BufReader::new(std::fs::File::open(h2_disc()).unwrap())).unwrap();
    iso.entries()
        .unwrap()
        .into_iter()
        .filter(|e| !e.is_dir && e.path.ends_with(".map"))
        .map(|e| {
            e.path
                .rsplit(['/', '\\'])
                .next()
                .unwrap()
                .trim_end_matches(".map")
                .to_string()
        })
        .collect()
}

fn h2_bytes(stem: &str) -> Vec<u8> {
    let mut iso =
        xiso::Xiso::open(BufReader::new(std::fs::File::open(h2_disc()).unwrap())).unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| {
            !e.is_dir && e.path.ends_with(&format!("/{stem}.map"))
                || e.path == format!("{stem}.map")
        })
        .unwrap_or_else(|| panic!("{stem}.map on the Halo 2 disc"));
    iso.read(&e).unwrap()
}

/// Every sound on every map decodes: each playable permutation's samples
/// are whole ADPCM blocks inside the file, between the bitmaps and the tag
/// data, overlapping nothing. Prints the census.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn every_h1_sound_decodes() {
    let mut classes: BTreeMap<u16, (usize, Vec<String>)> = BTreeMap::new();
    let mut formats: BTreeMap<(u16, u16, Compression), (usize, f64)> = BTreeMap::new();
    let (mut sounds, mut playable, mut perms, mut chained, mut seconds) =
        (0usize, 0usize, 0usize, 0usize, 0f64);
    let (mut mouth, mut subtitles, mut promotions, mut multi_range) =
        (0usize, 0usize, 0usize, 0usize);
    let mut flags: BTreeMap<u32, usize> = BTreeMap::new();
    let mut mixed_compression = 0;
    let mut failures = Vec::new();
    let mut unmapped: BTreeSet<u16> = BTreeSet::new();
    let maps = h1_maps();
    for name in &maps {
        let m = h1_map(name);
        let file = m.bytes();
        let tag_start = m.header().index_offset as usize;
        let mut spans = Vec::new();
        let mut n = 0;
        for t in m.tags().iter().filter(|t| t.group == SND) {
            let s = match Sound::read(&m, t) {
                Ok(s) => s,
                Err(e) => {
                    failures.push(format!("{name} {}: {e}", t.name));
                    continue;
                }
            };
            n += 1;
            sounds += 1;
            let c = classes.entry(s.class).or_default();
            c.0 += 1;
            if c.1.len() < 3 {
                c.1.push(t.name.clone());
            }
            if class::to_h2(s.class).is_err() {
                unmapped.insert(s.class);
            }
            *flags.entry(s.flags).or_default() += 1;
            promotions += usize::from(s.promotion_id().is_some());
            multi_range += usize::from(s.pitch_ranges.len() > 1);
            for r in &s.pitch_ranges {
                for p in &r.permutations {
                    perms += 1;
                    chained += usize::from(p.next != h1_assets::sounds::h1::NO_NEXT);
                    mouth += usize::from(p.mouth.size > 0);
                    subtitles += usize::from(p.subtitles.size > 0);
                    mixed_compression += usize::from(p.compression != s.compression);
                    if p.samples.size > 0 {
                        spans.push((p.samples.file_offset as usize, p.samples.size as usize));
                    }
                }
            }
            match decode_all(&s, file) {
                Ok(all) => {
                    let f = formats
                        .entry((s.sample_rate, s.encoding, s.compression))
                        .or_default();
                    f.0 += 1;
                    for (_, _, pcm) in &all {
                        playable += 1;
                        seconds += pcm.seconds();
                        f.1 += pcm.seconds();
                    }
                }
                Err(e) => failures.push(format!("{name} {}: {e}", t.name)),
            }
        }
        // Samples: inside the file, before the tag data, never overlapping
        // each other (shared samples are the same span).
        spans.sort();
        spans.dedup();
        let overlaps = spans
            .windows(2)
            .filter(|w| w[0].0 + w[0].1 > w[1].0)
            .count();
        let past = spans.iter().filter(|s| s.0 + s.1 > tag_start).count();
        let (lo, hi) = spans
            .iter()
            .fold((usize::MAX, 0), |(l, h), s| (l.min(s.0), h.max(s.0 + s.1)));
        println!(
            "{name:>12}: {n:>5} sounds, samples {lo:#x}..{hi:#x}, tag data {tag_start:#x}, {} spans, {overlaps} overlaps, {past} past tag data",
            spans.len()
        );
        assert_eq!((overlaps, past), (0, 0), "{name}");
    }
    println!(
        "\n{} maps, {sounds} sounds, {perms} permutations ({chained} continue another), {playable} playable, {:.1} h",
        maps.len(),
        seconds / 3600.0
    );
    println!(
        "lip-sync on {mouth} permutations, subtitles on {subtitles}, {promotions} sounds promote, {multi_range} have several pitch ranges, {mixed_compression} permutations differ from their sound's compression"
    );
    println!("\nrate, encoding, compression: sounds, hours");
    for ((r, e, c), (n, s)) in &formats {
        println!("  {r} {e} {c:?}: {n}, {:.2}", s / 3600.0);
    }
    println!("\nflags: {flags:?}");
    println!("\nclass: sounds, examples");
    for (c, (n, ex)) in &classes {
        let name = class::H1_NAMES
            .get(usize::from(*c))
            .copied()
            .flatten()
            .unwrap_or("?");
        let h2 = class::to_h2(*c)
            .ok()
            .and_then(|v| class::H2_NAMES[usize::from(v)])
            .unwrap_or("-");
        println!("  {c:>2} {name} -> {h2}: {n}  {ex:?}");
    }
    assert!(
        failures.is_empty(),
        "{} failures: {:#?}",
        failures.len(),
        &failures[..failures.len().min(20)]
    );
    assert!(unmapped.is_empty(), "classes with no mapping: {unmapped:?}");
}

struct Local<'a>(&'a [u8]);
impl tags::sound::ResourceReader for Local<'_> {
    fn check(&self, p: RawPointer, size: u32) -> tags::Result<()> {
        if p.file != ResourceFile::Local || p.offset as usize + size as usize > self.0.len() {
            return Err(tags::Error::Invalid("chunk not in this map"));
        }
        Ok(())
    }
    fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        self.check(p, size)?;
        Ok(self.0[p.offset as usize..][..size as usize].to_vec())
    }
}

fn gestalt_of(bytes: &[u8]) -> (SoundGestalt, String) {
    let mut m = Map::from_reader(Cursor::new(bytes)).unwrap();
    let t = m
        .tags()
        .iter()
        .find(|t| t.group == FourCC(*b"ugh!"))
        .unwrap()
        .clone();
    let b = m.read_tag_bytes(t.id).unwrap();
    (
        SoundGestalt::decode(&b, t.location.unwrap().address).unwrap(),
        t.name.unwrap_or_default(),
    )
}

/// Every retail gestalt is accounted for exactly once by the model, and
/// survives a rebuild: a map rebuilt with its `ugh!` as a built tree
/// decodes to the same gestalt, and every `snd!` still resolves. Prints
/// what the converter relies on: the pitch range `+4`/`+6` rule, the
/// `snd!` index bytes against the block counts, flags and classes.
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn retail_gestalts_decode_and_rebuild() {
    let mut classes: BTreeMap<u8, (usize, Vec<String>)> = BTreeMap::new();
    let mut flags: BTreeMap<u16, usize> = BTreeMap::new();
    let (mut rule_ok, mut rule_bad) = (0usize, 0usize);
    let (mut chunk_flag, mut chunks) = (0usize, 0usize);
    let mut idx_bad = BTreeMap::<&str, usize>::new();
    let mut why = BTreeMap::<&str, usize>::new();
    let mut plus4 = BTreeMap::<(u16, u16), usize>::new();
    let mut flag_pos = BTreeMap::<(usize, usize, bool, bool), usize>::new();
    for stem in h2_stems() {
        eprintln!("{stem}");
        let bytes = h2_bytes(&stem);
        let (g, name) = gestalt_of(&bytes);
        let counts: Vec<usize> = (0..11).map(|i| g.count(i)).collect();
        println!("{stem:>22}: ugh! {name:?} counts {counts:?}");
        // Pitch range +4/+6: a single permutation has +6 none; more than
        // one own the runtime flag bits +6.. +6+n of block 7, which never
        // overlap another range's and stay inside the block. +4 is 0 or none.
        let mut bits = Vec::new();
        for e in g.flat[blocks::PITCH_RANGES].chunks_exact(12) {
            let (enc, fl, n) = (
                u16::from_le_bytes([e[4], e[5]]),
                u16::from_le_bytes([e[6], e[7]]),
                u16::from_le_bytes([e[10], e[11]]),
            );
            *plus4.entry((n.min(2), enc)).or_default() += 1;
            if n == 1 {
                if fl == 0xffff {
                    rule_ok += 1
                } else {
                    rule_bad += 1;
                    *why.entry("single with +6").or_default() += 1
                }
            } else if fl == 0xffff {
                rule_bad += 1;
                *why.entry("several, +6 none").or_default() += 1;
            } else if enc == 0 {
                // The other retail form (+4 = 0): +6 is not a bit offset
                // into block 7; not interpreted, and not what we write.
                *why.entry("several, +4 = 0 (other form)").or_default() += 1;
            } else {
                bits.push((usize::from(fl), usize::from(n)));
            }
        }
        bits.sort();
        bits.dedup();
        for (i, &(at, n)) in bits.iter().enumerate() {
            let fits =
                at + n <= counts[7] * 8 && bits.get(i + 1).is_none_or(|&(next, _)| at + n <= next);
            if fits {
                rule_ok += 1
            } else if at + n > counts[7] * 8 {
                rule_bad += 1;
                *why.entry("past block 7").or_default() += 1
            } else {
                rule_bad += 1;
                *why.entry("overlap").or_default() += 1
            }
        }
        for c in &g.chunks {
            chunks += 1;
            chunk_flag += usize::from(c.size >> 31 != 0);
        }
        for p in g.flat[blocks::PERMUTATIONS].chunks_exact(16) {
            let (first, n) = (
                usize::from(u16::from_le_bytes([p[12], p[13]])),
                usize::from(u16::from_le_bytes([p[14], p[15]])),
            );
            for (k, c) in g.chunks[first..first + n].iter().enumerate() {
                if c.size >> 31 != 0 {
                    *flag_pos
                        .entry((
                            n.min(3),
                            k.min(2),
                            k + 1 == n,
                            c.size & 0x7fff_ffff == 0x7ff8,
                        ))
                        .or_default() += 1;
                }
            }
        }
        let mut m = Map::from_reader(Cursor::new(&bytes[..])).unwrap();
        let ids: Vec<(TagId, String)> = m
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"snd!") && t.location.is_some())
            .map(|t| (t.id, t.name.clone().unwrap_or_default()))
            .collect();
        for (id, n) in &ids {
            let h = m.read_tag_bytes(*id).unwrap();
            *flags.entry(u16::from_le_bytes([h[0], h[1]])).or_default() += 1;
            let c = classes.entry(h[2]).or_default();
            c.0 += 1;
            if c.1.len() < 3 && !c.1.contains(n) {
                c.1.push(n.clone());
            }
            let mut bad = |what, v: usize, none: usize, count: usize| {
                if v != none && v >= count {
                    *idx_bad.entry(what).or_default() += 1;
                }
            };
            bad("scale +b", usize::from(h[0xb]), 0xff, counts[1]);
            bad("promotion +c", usize::from(h[0xc]), 0xff, counts[9]);
            bad("custom playback +d", usize::from(h[0xd]), 0xff, counts[6]);
            bad(
                "extra info +e",
                usize::from(u16::from_le_bytes([h[0xe], h[0xf]])),
                0xffff,
                counts[10],
            );
            bad(
                "playback +6",
                usize::from(u16::from_le_bytes([h[6], h[7]])),
                0xffff,
                counts[0],
            );
        }

        // Rebuild with the gestalt as a built tree.
        let mut set = TagSet::from_map(&mut m).unwrap();
        let before = tags::sound::Gestalt::read(&mut m).unwrap();
        SoundGestalt::from_set(&set)
            .unwrap()
            .install(&mut set, "x")
            .unwrap();
        let (out, _) = set.build().unwrap();
        let (again, _) = gestalt_of(&out);
        assert_eq!(again, g, "{stem}: the rebuilt gestalt differs");
        let mut m2 = Map::from_reader(Cursor::new(&out[..])).unwrap();
        let after = tags::sound::Gestalt::read(&mut m2).unwrap();
        assert_eq!(after.chunks, before.chunks, "{stem}");
        for (id, _) in &ids {
            let h = tags::sound::read_header(&mut m2, *id).unwrap();
            after.resolve(&h).unwrap();
        }
    }
    println!("\npitch range +6 rule (distinct runs): {rule_ok} hold, {rule_bad} don't");
    println!("  (permutations min 2, +4): {plus4:x?}; failures {why:?}");
    println!("chunks with size bit 31: {chunk_flag} of {chunks}");
    println!("  by (chunks min 3, index min 2, last, 0x7ff8): {flag_pos:?}");
    println!("snd! index bytes out of their block: {idx_bad:?}");
    println!("flags: {flags:x?}");
    println!("\nclass: sounds, examples");
    for (c, (n, ex)) in &classes {
        let name = class::H2_NAMES
            .get(usize::from(*c))
            .copied()
            .flatten()
            .unwrap_or("?");
        println!("  {c:>2} {name}: {n}  {ex:?}");
    }
}

/// Converts Halo 1 sounds onto a Halo 2 base map, builds it, and reads every
/// converted sound back through `crates/formats/tags`' sound decoding and the audio
/// crate's decoder: the PCM is sample-identical to the Halo 1 decode. All
/// of beavercreek's sounds, and a30's dialogue (every `H1_DIALOG_STEP`th,
/// default 4). The base map's own sounds still resolve to the same chunks.
#[test]
#[ignore = "reads both discs ($H1_ISO, $H2_DISC)"]
fn converted_sounds_read_back_sample_identical() {
    let base = std::env::var("H2_BASE").unwrap_or_else(|_| "beavercreek".into());
    let step: usize = std::env::var("H1_DIALOG_STEP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);
    for (h1, dialogue_only) in [("beavercreek", false), ("a30", true)] {
        let m = h1_map(h1);
        let mut picked = Vec::new();
        for t in m.tags().iter().filter(|t| t.group == SND) {
            let s = Sound::read(&m, t).unwrap();
            if dialogue_only && !matches!(s.class, 19 | 44 | 46 | 47) {
                continue;
            }
            picked.push((t.name.clone(), s));
        }
        if dialogue_only {
            picked = picked.into_iter().step_by(step).collect();
        }
        let base_bytes = h2_bytes(&base);
        let mut bm = Map::from_reader(Cursor::new(&base_bytes[..])).unwrap();
        let before = tags::sound::Gestalt::read(&mut bm).unwrap();
        let old: Vec<TagId> = bm
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"snd!") && t.location.is_some())
            .map(|t| t.id)
            .collect();
        let mut set = TagSet::from_map(&mut bm).unwrap();
        h1_assets::sounds::convert::make_index_room(&mut set);
        let mut converted = h1_assets::sounds::convert::read_source_sounds(
            &m,
            picked.iter().map(|(name, _)| name.clone()),
            |_| false,
        )
        .unwrap();
        for (name, sound) in &mut converted {
            *name = format!("halo1\\{name}");
            if let Some(target) = sound.promotion.as_mut().and_then(|p| p.target.as_mut()) {
                *target = format!("halo1\\{target}");
            }
        }
        let ids = install_sounds(&mut set, converted).unwrap();
        let (out, _) = set.build().unwrap();
        let mut built = Map::from_reader(Cursor::new(&out[..])).unwrap();
        let g = tags::sound::Gestalt::read(&mut built).unwrap();
        assert_eq!(
            &g.chunks[..before.chunks.len()],
            &before.chunks[..],
            "old chunks kept"
        );
        for id in old {
            let h = tags::sound::read_header(&mut built, id).unwrap();
            g.resolve(&h).unwrap();
        }
        let (mut perms, mut samples, mut secs) = (0usize, 0usize, 0f64);
        for ((name, s), id) in picked.iter().zip(&ids) {
            let h = tags::sound::read_header(&mut built, *id).unwrap();
            assert_eq!(
                built.tag(*id).unwrap().name.as_deref(),
                Some(&*format!("halo1\\{name}"))
            );
            let resolved = g.resolve(&h).unwrap();
            let channels = match h.encoding {
                tags::sound::Encoding::Mono => 1,
                tags::sound::Encoding::Stereo => 2,
                e => panic!("{name}: {e:?}"),
            };
            assert_eq!(h.compression, tags::sound::Compression::XboxAdpcm, "{name}");
            assert_eq!(h.sample_rate.hz(), Some(s.hz().unwrap()), "{name}");
            let expect = decode_all(s, m.bytes()).unwrap();
            let got: Vec<_> = resolved
                .ranges
                .iter()
                .flat_map(|r| &r.permutations)
                .map(|p| {
                    let enc = g.read_encoded(p, &mut Local(&out)).unwrap();
                    audio::adpcm::decode(&enc, channels).unwrap()
                })
                .collect();
            assert_eq!(got.len(), expect.len(), "{name}: permutations");
            let mut longest = 0;
            for (a, (_, _, b)) in got.iter().zip(&expect) {
                assert!(a == &b.samples, "{name}: PCM differs");
                perms += 1;
                samples += a.len();
                secs += b.seconds();
                longest = longest.max(b.frames());
            }
            assert_eq!(
                u64::from(h.max_play_time),
                longest as u64 * 1000 / u64::from(s.hz().unwrap()),
                "{name}"
            );
        }
        println!(
            "{h1} onto {base}: {} sounds, {perms} permutations, {samples} samples ({secs:.1} s) identical; map {} -> {} bytes",
            picked.len(),
            base_bytes.len(),
            out.len()
        );
    }
}

/// Keeps names and pointers as they are.
struct Same;

impl h1_assets::sounds::MergeMap for Same {
    fn name(&mut self, id: u32) -> h1_assets::sounds::Result<u32> {
        Ok(id)
    }
    fn raw(
        &mut self,
        p: h1_assets::sounds::gestalt::ChunkPointer,
        _: u32,
    ) -> h1_assets::sounds::Result<h1_assets::sounds::gestalt::ChunkPointer> {
        Ok(p)
    }
}

/// Every retail `snd!` on every map, merged one by one into an empty
/// gestalt, names the same entries there as it did in its own: playback,
/// scale, each range's name, parameters and runtime flag bits, each
/// permutation's bytes and chunks, custom playback and extra info. The
/// merged gestalt still builds, with the sound-relative promotion rules
/// and their timer blocks carried over.
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn every_retail_sound_merges() {
    use h1_assets::sounds::convert::snd;
    let u16_at = |b: &[u8], o: usize| usize::from(u16::from_le_bytes([b[o], b[o + 1]]));
    let (mut merged, mut promoted) = (0usize, 0usize);
    for stem in h2_stems() {
        let bytes = h2_bytes(&stem);
        let (src, _) = gestalt_of(&bytes);
        let mut m = Map::from_reader(Cursor::new(&bytes)).unwrap();
        let sounds: Vec<_> = m
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"snd!") && t.location.is_some())
            .map(|t| t.id)
            .collect();
        let owner = m
            .tags()
            .iter()
            .find(|t| t.group == FourCC(*b"ugh!"))
            .unwrap()
            .id;
        let mut dst = SoundGestalt::default();
        for id in sounds {
            let h = m.read_tag_bytes(id).unwrap();
            let out = h1_assets::sounds::merge_sound(&src, &h, &mut dst, owner.0, &mut Same)
                .unwrap_or_else(|e| panic!("{stem} {id:?}: {e}"));
            promoted += usize::from(out[snd::PROMOTION] != u8::MAX);
            merged += 1;
            assert_eq!(out[snd::MAX_PLAY_TIME..], h[snd::MAX_PLAY_TIME..snd::LEN]);
            assert_eq!(
                dst.element(blocks::PLAYBACKS, u16_at(&out, snd::PLAYBACK)),
                src.element(blocks::PLAYBACKS, u16_at(&h, snd::PLAYBACK))
            );
            assert_eq!(
                dst.element(blocks::SCALES, out[snd::SCALE].into()),
                src.element(blocks::SCALES, h[snd::SCALE].into())
            );
            let flag = |g: &SoundGestalt, at: usize| {
                g.flat[blocks::RUNTIME_PERMUTATION_FLAGS][at / 8] >> (at % 8) & 1
            };
            for r in 0..usize::from(h[snd::PITCH_RANGE_COUNT]) {
                let a = src.element(blocks::PITCH_RANGES, u16_at(&h, snd::FIRST_PITCH_RANGE) + r);
                let b = dst.element(
                    blocks::PITCH_RANGES,
                    u16_at(&out, snd::FIRST_PITCH_RANGE) + r,
                );
                assert_eq!(
                    src.element(blocks::IMPORT_NAMES, u16_at(a, 0)),
                    dst.element(blocks::IMPORT_NAMES, u16_at(b, 0))
                );
                assert_eq!(
                    src.element(blocks::PITCH_RANGE_PARAMETERS, u16_at(a, 2)),
                    dst.element(blocks::PITCH_RANGE_PARAMETERS, u16_at(b, 2))
                );
                let n = u16_at(a, 10);
                assert_eq!((u16_at(a, 4), n), (u16_at(b, 4), u16_at(b, 10)));
                if u16_at(a, 4) == 0xffff {
                    for p in 0..n {
                        assert_eq!(flag(&src, u16_at(a, 6) + p), flag(&dst, u16_at(b, 6) + p));
                    }
                } else {
                    assert_eq!(u16_at(a, 6), u16_at(b, 6));
                }
                for p in 0..n {
                    let pa = src.element(blocks::PERMUTATIONS, u16_at(a, 8) + p);
                    let pb = dst.element(blocks::PERMUTATIONS, u16_at(b, 8) + p);
                    assert_eq!(
                        src.element(blocks::IMPORT_NAMES, u16_at(pa, 0)),
                        dst.element(blocks::IMPORT_NAMES, u16_at(pb, 0))
                    );
                    assert_eq!(pa[2..12], pb[2..12]);
                    let c = u16_at(pa, 14);
                    assert_eq!(c, u16_at(pb, 14));
                    assert_eq!(
                        src.chunks[u16_at(pa, 12)..][..c],
                        dst.chunks[u16_at(pb, 12)..][..c]
                    );
                }
            }
            for (i, at) in [
                (blocks::CUSTOM_PLAYBACKS, snd::CUSTOM_PLAYBACK),
                (blocks::PROMOTIONS, snd::PROMOTION),
            ] {
                assert_eq!(h[at] == u8::MAX, out[at] == u8::MAX);
                if h[at] != u8::MAX {
                    assert_eq!(
                        src.nested[i][usize::from(h[at])],
                        dst.nested[i][usize::from(out[at])]
                    );
                }
            }
            if u16_at(&h, snd::EXTRA_INFO) != 0xffff {
                let a = &src.nested[blocks::EXTRA_INFOS][u16_at(&h, snd::EXTRA_INFO)];
                let b = &dst.nested[blocks::EXTRA_INFOS][u16_at(&out, snd::EXTRA_INFO)];
                assert_eq!(a.children, b.children);
                assert_eq!(a.bytes, b.bytes);
            }
        }
        let _ = dst.to_block();
        println!(
            "{stem:>22}: merged into {:?}",
            (0..11).map(|i| dst.count(i)).collect::<Vec<_>>()
        );
    }
    assert_eq!((merged, promoted), (65_309, 231));
}

/// A character's whole tree moved from one retail map onto another
/// (`h1_assets::sounds::transplant`: 04b's flood combat elite onto 05a, which has
/// none) takes its sounds: after a build, every moved `snd!` names, in the
/// new map's gestalt, the source's playback and scale, and per permutation
/// the same import name (as a string) and chunks whose bytes are the
/// source's (local raw copied, shared-cache pointers kept).
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn a_transplanted_tree_takes_its_sounds() {
    use h1_assets::sounds::convert::snd;
    let u16_at = |b: &[u8], o: usize| usize::from(u16::from_le_bytes([b[o], b[o + 1]]));
    let name = "objects\\characters\\floodcombat_elite\\ai\\floodcombat_elite";
    let src_bytes = h2_bytes("04b_floodlab");
    let (src_g, _) = gestalt_of(&src_bytes);
    let mut src = Map::from_reader(Cursor::new(src_bytes.as_slice())).unwrap();
    let root = src
        .tags()
        .iter()
        .find(|t| t.group == FourCC(*b"char") && t.name.as_deref() == Some(name))
        .unwrap()
        .id;
    let base = h2_bytes("05a_deltaapproach");
    let mut set =
        TagSet::from_map(&mut Map::from_reader(Cursor::new(base.as_slice())).unwrap()).unwrap();
    let mut g = SoundGestalt::from_set(&set).unwrap();
    let schema = tags::schema::Schema::builtin();
    let moved = h1_assets::sounds::transplant(schema, &mut src, &src_g, root, &mut set, &mut g)
        .unwrap()
        .unwrap();
    g.install(&mut set, "g").unwrap();
    let (out, _) = set.build().unwrap();
    let (dst_g, _) = gestalt_of(&out);
    let mut dst = Map::from_reader(Cursor::new(out.as_slice())).unwrap();
    let raw = |bytes: &[u8], p: u32, size: u32| -> Option<Vec<u8>> {
        (p >> 30 == 0).then(|| bytes[p as usize..][..size as usize].to_vec())
    };
    let (mut sounds, mut chunks, mut promotions) = (0, 0, 0);
    for from in &moved.tree.added {
        if src.tag(*from).unwrap().group != FourCC(*b"snd!") {
            continue;
        }
        let to = moved.tree.ids[from];
        let h = src.read_tag_bytes(*from).unwrap();
        let o = dst.read_tag_bytes(to).unwrap();
        sounds += 1;
        assert_eq!(h[snd::PROMOTION] == u8::MAX, o[snd::PROMOTION] == u8::MAX);
        if h[snd::PROMOTION] != u8::MAX {
            promotions += 1;
            assert_eq!(
                src_g.nested[blocks::PROMOTIONS][usize::from(h[snd::PROMOTION])],
                dst_g.nested[blocks::PROMOTIONS][usize::from(o[snd::PROMOTION])],
                "{}",
                src.tag(*from).unwrap().name.as_deref().unwrap(),
            );
        }
        assert_eq!(
            src_g.element(blocks::PLAYBACKS, u16_at(&h, snd::PLAYBACK)),
            dst_g.element(blocks::PLAYBACKS, u16_at(&o, snd::PLAYBACK))
        );
        assert_eq!(
            src_g.element(blocks::SCALES, h[snd::SCALE].into()),
            dst_g.element(blocks::SCALES, o[snd::SCALE].into())
        );
        assert_eq!(h[snd::PITCH_RANGE_COUNT], o[snd::PITCH_RANGE_COUNT]);
        for r in 0..usize::from(h[snd::PITCH_RANGE_COUNT]) {
            let a = src_g.element(blocks::PITCH_RANGES, u16_at(&h, snd::FIRST_PITCH_RANGE) + r);
            let b = dst_g.element(blocks::PITCH_RANGES, u16_at(&o, snd::FIRST_PITCH_RANGE) + r);
            for p in 0..u16_at(a, 10) {
                let pa = src_g.element(blocks::PERMUTATIONS, u16_at(a, 8) + p);
                let pb = dst_g.element(blocks::PERMUTATIONS, u16_at(b, 8) + p);
                let string = |m: &Map<Cursor<&[u8]>>, g: &SoundGestalt, e: &[u8]| {
                    let id = u32::from_le_bytes(
                        g.element(blocks::IMPORT_NAMES, u16_at(e, 0))
                            .try_into()
                            .unwrap(),
                    );
                    m.strings()[(id & 0xff_ffff) as usize].clone()
                };
                assert_eq!(string(&src, &src_g, pa), string(&dst, &dst_g, pb));
                for c in 0..u16_at(pa, 14) {
                    let ca = src_g.chunks[u16_at(pa, 12) + c];
                    let cb = dst_g.chunks[u16_at(pb, 12) + c];
                    let (
                        h1_assets::sounds::gestalt::ChunkPointer::Stored(x),
                        h1_assets::sounds::gestalt::ChunkPointer::Stored(y),
                    ) = (ca.pointer, cb.pointer)
                    else {
                        panic!("decoded chunks are stored");
                    };
                    assert_eq!(ca.size, cb.size);
                    let size = ca.size & 0x7fff_ffff;
                    match raw(&src_bytes, x, size) {
                        Some(a) => assert_eq!(Some(a), raw(&out, y, size), "chunk bytes"),
                        None => assert_eq!(x, y, "a shared-cache pointer stays"),
                    }
                    chunks += 1;
                }
            }
        }
    }
    println!(
        "{name}: {} tags, {sounds} sounds, {chunks} chunks, {promotions} promotions, {:?} raw copied",
        moved.tree.added.len(),
        moved.raw
    );
    assert_eq!((sounds, promotions), (moved.sounds, moved.promotions));
    assert_eq!(promotions, 1);
    assert!(sounds > 0 && chunks > 0);
}

/// Follow both resource locators as the Xbox cache loader does. Offsets in
/// the permutation descriptors are relative to the first locator's data.
fn mouth_resource<'a>(e: &h1_assets::sounds::gestalt::Nested, file: &'a [u8]) -> (&'a [u8], &'a [u8]) {
    let word = |b: &[u8], i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap()) as usize;
    let raw = &file[word(&e.bytes, 8)..][..word(&e.bytes, 12)];
    assert_eq!(
        (&raw[..4], &raw[raw.len() - 4..]),
        (&b"hklb"[..], &b"fklb"[..])
    );
    assert_eq!(raw.len(), 28 + word(raw, 4));
    assert_eq!(word(&e.bytes, 0x10), 16);
    assert_eq!(word(&e.bytes, 0x14), word(raw, 4));
    let loc = &e.children[1];
    assert_eq!(loc.count, 2);
    assert_eq!(&loc.bytes[..8], &[1, 2, 0, 0, 0, 0, 255, 255]);
    assert_eq!(&loc.bytes[16..24], &[0, 2, 8, 0, 8, 0, 16, 0]);
    let child = |at: usize| {
        let start = 24 + word(&loc.bytes, at + 12);
        assert_eq!(&raw[start - 4..start], b"crsr");
        &raw[start..][..word(&loc.bytes, at + 8)]
    };
    let (data, descriptors) = (child(0), child(16));
    assert_eq!(word(raw, 8), data.len());
    assert_eq!(word(raw, 16) * 16, descriptors.len());
    (data, descriptors)
}

#[test]
#[ignore = "reads a10 from H1_ISO and 01b_spacestation from H2_DISC"]
fn a10_dialogue_mouth_curves_reach_the_retail_resource_layout() {
    use h1_assets::sounds::convert::snd;
    let word = |b: &[u8], i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
    let short = |b: &[u8], i: usize| u16::from_le_bytes([b[i], b[i + 1]]) as usize;
    {
        let retail = h2_bytes("01b_spacestation");
        let mut map = Map::from_reader(Cursor::new(&retail)).unwrap();
        let owner = map
            .tags()
            .iter()
            .find(|t| t.group == FourCC(*b"ugh!"))
            .unwrap()
            .id;
        let (g, _) = gestalt_of(&retail);
        let example = g.nested[blocks::EXTRA_INFOS]
            .iter()
            .find(|e| {
                let p = word(&e.bytes, 8);
                p != 0 && p >> 30 == 0 && e.children[1].count == 2
            })
            .unwrap();
        assert_eq!(word(&example.bytes, 0x20), owner.0);
        let (data, desc) = mouth_resource(example, &retail);
        assert!(desc.chunks_exact(16).any(|d| {
            let start = word(d, 0) as usize;
            let len = word(d, 4) as usize;
            data[start..start + len].iter().any(|&x| x != 0)
        }));
        let marine = map
            .tags()
            .iter()
            .find(|t| {
                t.group == FourCC(*b"jmad")
                    && t.name.as_deref() == Some(r"objects\characters\marine\marine")
            })
            .unwrap()
            .id;
        let graph = tags::anim::AnimationGraph::read(&mut map, marine).unwrap();
        for key in ["combat", "patrol"] {
            let mode = graph
                .modes
                .iter()
                .find(|m| m.name.resolve(map.strings()).unwrap() == Some(key))
                .unwrap();
            let class = mode
                .classes
                .iter()
                .find(|c| c.name.resolve(map.strings()).unwrap() == Some("any"))
                .unwrap();
            let ty = class
                .types
                .iter()
                .find(|t| t.name.resolve(map.strings()).unwrap() == Some("any"))
                .unwrap();
            let talk = ty
                .overlays
                .iter()
                .find(|a| a.name.resolve(map.strings()).unwrap() == Some("talk"))
                .unwrap();
            assert_eq!(
                graph.animations[talk.animation_index as usize].frame_count,
                1
            );
        }
    }

    let h1 = h1_map("a10");
    let mut set = TagSet::new(map::CacheType::Campaign, "a10", "a10");
    let mut sounds = Vec::new();
    let mut expected = Vec::new();
    for tag in h1.tags().iter().filter(|t| t.group == SND) {
        let s = Sound::read(&h1, tag).unwrap();
        let mut ranges = Vec::new();
        for (r, range) in s.pitch_ranges.iter().enumerate() {
            let mut curves = Vec::new();
            for chain in s.chains(r).unwrap() {
                let p = &range.permutations[chain[0]];
                let curve = if p.mouth.size > 0 {
                    assert_eq!(chain.len(), 1);
                    h1.read(p.mouth.address, p.mouth.size as usize).unwrap()
                } else {
                    &[]
                };
                curves.push(curve);
            }
            ranges.push(curves);
        }
        if ranges.iter().flatten().all(|curve| curve.is_empty()) {
            continue;
        }
        sounds.push((tag.name.clone(), convert_sound(&s, h1.bytes()).unwrap()));
        expected.push(ranges);
    }
    let ids = install_sounds(&mut set, sounds).unwrap();
    assert_eq!(ids.len(), 1038);
    let (bytes, _) = set.build().unwrap();
    let (g, _) = gestalt_of(&bytes);
    let mut map = Map::from_reader(Cursor::new(&bytes)).unwrap();
    let owner = map
        .tags()
        .iter()
        .find(|t| t.group == FourCC(*b"ugh!"))
        .unwrap()
        .id;
    let mut count = 0;
    for (id, ranges) in ids.into_iter().zip(expected) {
        let h = map.read_tag_bytes(id).unwrap();
        let e = &g.nested[blocks::EXTRA_INFOS][short(&h, snd::EXTRA_INFO)];
        assert_eq!(word(&e.bytes, 0x20), owner.0);
        let (data, descriptors) = mouth_resource(e, &bytes);
        for (r, curves) in ranges.into_iter().enumerate() {
            let range = g.element(blocks::PITCH_RANGES, short(&h, snd::FIRST_PITCH_RANGE) + r);
            assert_eq!(short(range, 10), curves.len());
            for (i, curve) in curves.into_iter().enumerate() {
                let perm = g.element(blocks::PERMUTATIONS, short(range, 8) + i);
                let at = usize::try_from(perm[5] as i8).unwrap() * 16;
                let d = &descriptors[at..at + 16];
                assert_eq!(word(d, 12), 0);
                let offset = word(d, 0) as usize;
                assert_eq!(&data[offset..][..word(d, 4) as usize], curve);
                count += usize::from(!curve.is_empty());
            }
        }
    }
    assert_eq!(count, 4659);
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn every_retail_halo1_promotion_round_trips() {
    use tags::sound::h1_promotion::*;

    let word = |bytes: &[u8], at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());

    let mut iso = h1_disc();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let (mut maps, mut redirects, mut rate_limits) = (0, 0, 0);
    for entry in &disc.maps {
        let source = H1Map::from_bytes(&iso.read(entry).unwrap()).unwrap();
        if !matches!(
            source.header().cache_type,
            h1map::CacheType::Campaign | h1map::CacheType::Multiplayer
        ) {
            continue;
        }
        maps += 1;
        let level = &source.header().name;
        let rules: Vec<_> = source
            .tags()
            .iter()
            .filter(|tag| tag.group == SND)
            .filter_map(|tag| {
                let bytes = source.tag_data(tag).unwrap();
                let target = h1map::TagId(word(bytes, 0x7c));
                let count = u16::from_le_bytes([bytes[0x80], bytes[0x81]]);
                (count != 0 || !target.is_none()).then_some((tag, count, word(bytes, 0x84), target))
            })
            .collect();
        let mut set = TagSet::new(map::CacheType::Multiplayer, level, level);
        let ids = h1_assets::sounds::convert::install_source_sounds(
            &source,
            &mut set,
            rules.iter().map(|(tag, _, _, _)| tag.name.clone()),
        )
        .unwrap_or_else(|e| panic!("{level}: {e}"));
        let (bytes, _) = set.build().unwrap();
        let mut converted = Map::from_reader(Cursor::new(bytes)).unwrap();
        let gestalt = tags::sound::Gestalt::read(&mut converted).unwrap();
        for (tag, count, millis, target) in rules {
            let id = ids[&tag.name];
            let bytes = converted.read_tag_bytes(id).unwrap();
            assert!(present(&bytes), "{level}: {} lost its rule", tag.name);
            assert_eq!(word(&bytes, COUNT as usize), count as u32);
            assert_eq!(word(&bytes, MILLIS as usize), millis);
            let target_out = TagId(word(&bytes, TARGET as usize));
            if target.is_none() {
                rate_limits += 1;
                assert_eq!(target_out, TagId::NULL);
            } else {
                redirects += 1;
                let source_target = source.tag(target).unwrap();
                let rebuilt = converted.tag(target_out).unwrap();
                assert_eq!(rebuilt.group, FourCC(*b"snd!"));
                assert_eq!(rebuilt.name.as_ref(), Some(&source_target.name));
                let header = tags::sound::read_header(&mut converted, target_out).unwrap();
                assert!(!gestalt.resolve(&header).unwrap().ranges.is_empty());
            }
        }
    }
    assert_eq!((maps, redirects, rate_limits), (23, 253, 77));
}
