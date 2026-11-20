//! Every Halo 1 looping sound and sound environment on the owner's disc
//! converts. `#[ignore]`d; nothing from the disc is kept.
//!
//! ```text
//! H1_ISO=disc/halo1.iso cargo test -p h1sounds --test looping_disc -- --ignored --nocapture
//! ```

use std::collections::HashMap;
use std::io::{BufReader, Cursor};

use h1map::H1Map;
use h1_assets::sounds::looping::{H2_DETAIL, H2_LEN, H2_TRACK, LSND, SNDE, read_environment, read_looping};
use map::TagId;

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn every_looping_sound_and_environment_converts() {
    let path = std::env::var("H1_ISO").expect("set H1_ISO to the Halo 1 disc");
    let mut iso = xiso::Xiso::open(BufReader::new(std::fs::File::open(path).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let (mut loops, mut tracks, mut details, mut sounds, mut environments) = (0, 0, 0, 0, 0);
    let mut failed = Vec::new();
    for e in disc.maps.clone() {
        let name = h1map::disc::map_stem(&e).to_string();
        let h1 = H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
        for t in h1.tags().iter().filter(|t| t.group == LSND) {
            let l = read_looping(&h1, t).unwrap_or_else(|e| panic!("{name}: {}: {e}", t.name));
            failed.extend(l.notes.iter().map(|n| format!("{name}: {n}")));
            // b30's waves are detail sounds alone.
            assert!(
                !l.tracks.is_empty() || !l.details.is_empty(),
                "{name}: {} plays nothing",
                t.name
            );
            // Every sound named converts, and the tag names each one.
            let names = l.sound_names();
            assert_eq!(names.len(), l.sounds.len() + l.notes.len(), "{}", t.name);
            let ids: HashMap<String, TagId> = names
                .iter()
                .enumerate()
                .map(|(k, n)| (n.clone(), TagId(0xe000_0000 | k as u32)))
                .collect();
            let c = l.to_h2(&ids);
            assert_eq!(c.root.bytes.len(), H2_LEN);
            let mut refs = 0;
            for f in &c.root.fixups {
                if let map::build::FixupKind::Block { count, child } = &f.kind {
                    let len = if f.offset == 0x1c {
                        H2_TRACK
                    } else {
                        H2_DETAIL
                    };
                    assert_eq!(child.bytes.len(), *count as usize * len, "{}", t.name);
                    refs += child.fixups.len();
                }
            }
            let named = l
                .tracks
                .iter()
                .flat_map(|t| t.sounds.iter())
                .chain(l.details.iter().map(|d| &d.sound))
                .flatten()
                .count();
            assert_eq!(refs, named, "{}", t.name);
            loops += 1;
            tracks += l.tracks.len();
            details += l.details.len();
            sounds += l.sounds.len();
        }
        for t in h1.tags().iter().filter(|t| t.group == SNDE) {
            let c = read_environment(&h1, t).unwrap_or_else(|e| panic!("{name}: {}: {e}", t.name));
            let f = |at: usize| f32::from_le_bytes(c.root.bytes[at..at + 4].try_into().unwrap());
            // Intensities in decibels: at most 0.
            for at in [0x08, 0x0c, 0x1c, 0x24] {
                assert!(f(at) <= 0.0, "{name}: {} +{at:#x} = {}", t.name, f(at));
            }
            environments += 1;
        }
    }
    println!(
        "{loops} looping sounds ({tracks} tracks, {details} detail sounds, {sounds} sounds), \
         {environments} sound environments; {} sounds didn't convert: {failed:#?}",
        failed.len()
    );
    assert!(loops > 0 && environments > 0);
    assert!(failed.is_empty(), "{failed:#?}");
}

/// Loudness and character of one decoded permutation: seconds, RMS in
/// dBFS, zero crossings per second (per channel), and the RMS of the
/// first difference over the RMS (near 0 for a low drone, near 1.4 for
/// white noise).
fn character(p: &h1_assets::sounds::decode::Pcm) -> (f64, f64, f64, f64) {
    let ch = p.channels;
    let n = p.frames().max(1);
    let x: Vec<f64> = p
        .samples
        .iter()
        .step_by(ch)
        .map(|&s| f64::from(s) / 32768.0)
        .collect();
    let rms = (x.iter().map(|v| v * v).sum::<f64>() / n as f64).sqrt();
    let d = (x.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum::<f64>() / n as f64).sqrt();
    let zc = x
        .windows(2)
        .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
        .count();
    let secs = p.seconds();
    (
        secs,
        20.0 * rms.max(1e-9).log10(),
        zc as f64 / secs.max(1e-6),
        d / rms.max(1e-9),
    )
}

/// What a level's cutscene scripts and ambience can start, with each
/// sound's header and the character of its samples (`$H1_LEVEL`, default
/// a10): every `lsnd` and `snd!` in the scenario's script references
/// (`scnr +0x4b4`), every structure BSP's background sounds, and every
/// object definition with a looping sound attached. Prints only names,
/// fields and statistics.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn level_loops_and_their_character() {
    use h1_assets::sounds::h1::{SND, Sound};
    let path = std::env::var("H1_ISO").expect("set H1_ISO to the Halo 1 disc");
    let level = std::env::var("H1_LEVEL").unwrap_or_else(|_| "a10".into());
    let mut iso = xiso::Xiso::open(BufReader::new(std::fs::File::open(path).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = disc.map(&level).unwrap().clone();
    let h1 = H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
    let f32_at = |b: &[u8], at: usize| f32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    let u32_at = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    let sound = |name: &str, indent: &str| {
        let Some(t) = h1.find(SND, name) else {
            println!("{indent}{name}: missing");
            return;
        };
        let s = Sound::read(&h1, t).unwrap();
        println!(
            "{indent}snd! {name}: flags {:#x} class {} rate {} ch {} gain {} dist {:?} scale0 {:?} scale1 {:?} ranges {}",
            s.flags,
            s.class,
            s.hz().unwrap(),
            s.channels().unwrap(),
            s.gain_modifier,
            s.distance,
            s.zero_modifiers,
            s.one_modifiers,
            s.pitch_ranges.len()
        );
        for (r, p, pcm) in h1_assets::sounds::decode_all(&s, h1.bytes()).unwrap() {
            let (secs, db, zc, hf) = character(&pcm);
            println!(
                "{indent}  r{r}p{p}: {secs:.2} s, {db:.1} dBFS, {zc:.0} zc/s, hf {hf:.3}, gain {}",
                s.pitch_ranges[r].permutations[p].gain
            );
        }
    };
    let looping = |t: &h1map::Tag, indent: &str| {
        let l = read_looping(&h1, t).unwrap();
        let b = h1.tag_data(t).unwrap();
        println!(
            "{indent}lsnd {}: flags {:#x} max distance {} tracks {} details {}",
            t.name,
            u32_at(b, 0),
            l.max_distance,
            l.tracks.len(),
            l.details.len()
        );
        for (k, tr) in l.tracks.iter().enumerate() {
            println!(
                "{indent}  track {k}: flags {:#x} gain {} dB fades {} {} sounds {:?}",
                tr.flags, tr.gain_db, tr.fade_in, tr.fade_out, tr.sounds
            );
        }
        for (k, d) in l.details.iter().enumerate() {
            println!(
                "{indent}  detail {k}: {:?} period {:?} gain {} dB flags {:#x}",
                d.sound, d.period, d.gain_db, d.flags
            );
        }
        for n in l.sound_names() {
            sound(&n, &format!("{indent}    "));
        }
    };
    // The scripts' references.
    let scnr = h1.tag_data(h1.scenario()).unwrap();
    let refs = h1map::Block::read(scnr, 0x4b4).unwrap();
    let refs = h1.block(refs, 0x28).unwrap();
    println!("== {level}: script references");
    for e in refs.chunks_exact(0x28) {
        let Some(t) = h1map::TagRef::read(e, 0x18).and_then(|r| h1.tag(r.id).ok()) else {
            continue;
        };
        if t.group == LSND {
            looping(t, "");
        } else if t.group == SND {
            sound(&t.name.clone(), "");
        }
    }
    println!("== {level}: background sounds");
    for (k, bsp) in h1.structure_bsps().unwrap().into_iter().enumerate() {
        let s = h1.structure_bsp(&bsp).unwrap();
        let mut uses: std::collections::BTreeMap<(i16, i16), usize> = Default::default();
        for c in s.space.block_at(s.root, 0x134, 0x68).unwrap() {
            let w = |at: usize| i16::from_le_bytes([c[at], c[at + 1]]);
            *uses.entry((w(4), w(6))).or_default() += 1;
        }
        println!(
            "bsp {k} {}: clusters by (background, environment): {uses:?}",
            bsp.name
        );
        for e in s.space.block_at(s.root, 0x1fc, 0x74).unwrap() {
            let n = String::from_utf8_lossy(&e[..32])
                .trim_end_matches('\0')
                .to_string();
            println!("{}: {n}", bsp.name);
            if let Some(t) = h1map::TagRef::read(e, 0x20).and_then(|r| h1.tag(r.id).ok()) {
                looping(t, "  ");
            }
        }
    }
    println!("== {level}: looping sounds attached to objects");
    let objects = [
        "bipd", "vehi", "weap", "eqip", "garb", "proj", "scen", "mach", "ctrl", "lifi", "plac",
        "ssce",
    ];
    for t in h1.tags() {
        let g = t.group.0.to_be_bytes();
        if !objects.iter().any(|o| o.as_bytes() == g) {
            continue;
        }
        let b = h1.tag_data(t).unwrap();
        let Some(blk) = h1map::Block::read(b, 0x140) else {
            continue;
        };
        let Ok(es) = h1.block(blk, 0x48) else {
            continue;
        };
        for a in es.chunks_exact(0x48) {
            if u32_at(a, 0) != u32::from_be_bytes(*b"lsnd") {
                continue;
            }
            let Some(l) = h1map::TagRef::read(a, 0).and_then(|r| h1.tag(r.id).ok()) else {
                continue;
            };
            println!(
                "{} {}: {} at {:?}, words 0x30.. {:x?}, scale-ish {}",
                String::from_utf8_lossy(&g),
                t.name,
                l.name,
                String::from_utf8_lossy(&a[0x10..0x30]).trim_end_matches('\0'),
                &a[0x30..0x48],
                f32_at(a, 0x30)
            );
        }
    }
}
