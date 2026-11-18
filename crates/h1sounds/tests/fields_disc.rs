//! What retail Halo 2 sounds hold in the fields a converted Halo 1 sound
//! fills, next to what the converter writes for a Halo 1 level, per class.
//! `#[ignore]`d; prints tallies only, nothing from the discs is kept.
//!
//! ```text
//! H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso \
//!     cargo test -p h1sounds --release --test fields_disc -- --ignored --nocapture
//! ```

#![allow(clippy::chunks_exact_to_as_chunks)]

use std::collections::BTreeMap;
use std::io::{BufReader, Cursor};

use h1map::H1Map;
use h1sounds::convert::snd;
use h1sounds::gestalt::{SoundGestalt, blocks};
use h1sounds::h1::{SND, Sound};
use h1sounds::{class, convert_sound};
use map::{FourCC, Map};

type Tally = BTreeMap<u8, BTreeMap<&'static str, BTreeMap<String, usize>>>;

fn f(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn i16_at(b: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([b[at], b[at + 1]])
}

fn count(t: &mut Tally, class: u8, field: &'static str, v: String) {
    *t.entry(class)
        .or_default()
        .entry(field)
        .or_default()
        .entry(v)
        .or_default() += 1;
}

/// One sound's fields: its header, playback, scale, and its first pitch
/// range's parameters (and how many ranges).
fn tally(t: &mut Tally, h: &[u8], playback: &[u8], scale: &[u8], params: &[u8], ranges: usize) {
    let c = h[snd::CLASS];
    count(t, c, "sounds", String::new());
    count(
        t,
        c,
        "flags",
        format!("{:#06x}", u16::from_le_bytes([h[0], h[1]])),
    );
    count(t, c, "rate/enc/comp", format!("{}/{}/{}", h[3], h[4], h[5]));
    count(
        t,
        c,
        "distance",
        format!("{:.1}..{:.1}", f(playback, 0), f(playback, 4)),
    );
    count(t, c, "skip", format!("{:.2}", f(playback, 8)));
    count(t, c, "max bend", format!("{:.2}", f(playback, 0xc)));
    count(
        t,
        c,
        "gain base/var dB",
        format!("{:.1}/{:.1}", f(playback, 0x10), f(playback, 0x14)),
    );
    count(
        t,
        c,
        "random pitch",
        format!("{}..{}", i16_at(playback, 0x18), i16_at(playback, 0x1a)),
    );
    count(
        t,
        c,
        "cone in/out/gain",
        format!(
            "{:.2}/{:.2}/{:.1}",
            f(playback, 0x1c),
            f(playback, 0x20),
            f(playback, 0x24)
        ),
    );
    count(
        t,
        c,
        "pb flags/azimuth",
        format!(
            "{:#x}/{:.2}",
            u32::from_le_bytes(playback[0x28..0x2c].try_into().unwrap()),
            f(playback, 0x2c)
        ),
    );
    count(
        t,
        c,
        "positional/1st-person",
        format!("{:.2}/{:.2}", f(playback, 0x30), f(playback, 0x34)),
    );
    count(
        t,
        c,
        "scale gain",
        format!("{:.1}..{:.1}", f(scale, 0), f(scale, 4)),
    );
    count(
        t,
        c,
        "scale pitch",
        format!("{}..{}", i16_at(scale, 8), i16_at(scale, 0xa)),
    );
    count(
        t,
        c,
        "scale skip",
        format!("{:.2}..{:.2}", f(scale, 0xc), f(scale, 0x10)),
    );
    let p: Vec<i16> = (0..5).map(|k| i16_at(params, 2 * k)).collect();
    count(
        t,
        c,
        "ranges, params[0]",
        format!("{} {p:?}", ranges.min(3)),
    );
}

fn print(title: &str, t: &Tally) {
    println!("\n== {title}");
    for (c, fields) in t {
        let name = class::H2_NAMES
            .get(usize::from(*c))
            .copied()
            .flatten()
            .unwrap_or("?");
        println!(
            "class {c} {name}: {} sounds",
            fields["sounds"].values().sum::<usize>()
        );
        for (field, values) in fields.iter().filter(|(k, _)| **k != "sounds") {
            let mut v: Vec<_> = values.iter().collect();
            v.sort_by(|a, b| b.1.cmp(a.1));
            let top: Vec<String> = v.iter().take(4).map(|(k, n)| format!("{k} x{n}")).collect();
            println!("  {field:>22}: {}", top.join(", "));
        }
    }
}

fn h2_bytes(iso: &str, stem: &str) -> Vec<u8> {
    let mut iso = xiso::Xiso::open(BufReader::new(std::fs::File::open(iso).unwrap())).unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| !e.is_dir && e.path.ends_with(&format!("{stem}.map")))
        .unwrap_or_else(|| panic!("{stem}.map"));
    iso.read(&e).unwrap()
}

#[test]
#[ignore = "reads both discs ($H1_ISO, $H2_DISC)"]
fn retail_and_converted_sound_fields_by_class() {
    let stems = std::env::var("H2_STEMS").unwrap_or("05a_deltaapproach,03a_oldmombasa".into());
    let mut retail = Tally::new();
    for stem in stems.split(',') {
        let bytes = h2_bytes(&std::env::var("H2_DISC").unwrap(), stem);
        let mut m = Map::from_reader(Cursor::new(&bytes[..])).unwrap();
        let ugh = m
            .tags()
            .iter()
            .find(|t| t.group == FourCC(*b"ugh!"))
            .unwrap()
            .clone();
        let gb = m.read_tag_bytes(ugh.id).unwrap();
        let g = SoundGestalt::decode(&gb, ugh.location.unwrap().address).unwrap();
        let ids: Vec<_> = m
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"snd!") && t.location.is_some())
            .map(|t| t.id)
            .collect();
        for id in ids {
            let h = m.read_tag_bytes(id).unwrap();
            let pb = usize::from(u16::from_le_bytes([h[6], h[7]]));
            let first = usize::from(u16::from_le_bytes([h[8], h[9]]));
            let range = g.element(blocks::PITCH_RANGES, first);
            let params = usize::from(u16::from_le_bytes([range[2], range[3]]));
            let params = if params == 0xffff {
                &[0u8; 10][..]
            } else {
                g.element(blocks::PITCH_RANGE_PARAMETERS, params)
            };
            tally(
                &mut retail,
                &h,
                g.element(blocks::PLAYBACKS, pb),
                g.element(blocks::SCALES, usize::from(h[0xb])),
                params,
                usize::from(h[0xa]),
            );
        }
    }
    print(&format!("retail {stems}"), &retail);

    let level = std::env::var("H1_LEVEL").unwrap_or("a10".into());
    let path = std::env::var("H1_ISO").unwrap();
    let mut iso = xiso::Xiso::open(BufReader::new(std::fs::File::open(path).unwrap())).unwrap();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = d.map(&level).expect("level on the disc").clone();
    let h1 = H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
    let mut converted = Tally::new();
    let mut h1_raw = Tally::new();
    for t in h1.tags().iter().filter(|t| t.group == SND) {
        let s = Sound::read(&h1, t).unwrap();
        let Ok(c) = convert_sound(&s, h1.bytes()) else {
            continue;
        };
        tally(
            &mut converted,
            &c.header,
            &c.playback,
            &c.scale,
            &c.ranges[0].parameters,
            c.ranges.len(),
        );
        let k = c.header[snd::CLASS];
        let raw = |v: &[f32]| format!("{v:.2?}");
        count(&mut h1_raw, k, "h1 distance", raw(&s.distance));
        count(&mut h1_raw, k, "h1 gain modifier", raw(&[s.gain_modifier]));
        count(
            &mut h1_raw,
            k,
            "h1 random pitch",
            raw(&s.random_pitch_bounds),
        );
        count(&mut h1_raw, k, "h1 zero s/g/p", raw(&s.zero_modifiers));
        count(&mut h1_raw, k, "h1 one s/g/p", raw(&s.one_modifiers));
        count(&mut h1_raw, k, "h1 skip", raw(&[s.skip_fraction]));
        count(
            &mut h1_raw,
            k,
            "h1 cone",
            raw(&[s.cone_angles[0], s.cone_angles[1], s.outer_cone_gain]),
        );
        let r = &s.pitch_ranges[0];
        count(
            &mut h1_raw,
            k,
            "h1 range0 natural/bend",
            raw(&[r.natural_pitch, r.bend_bounds[0], r.bend_bounds[1]]),
        );
        let gains: Vec<f32> = r.permutations.iter().map(|p| p.gain).collect();
        count(
            &mut h1_raw,
            k,
            "h1 perm gain min",
            raw(&[gains.iter().cloned().fold(9.0, f32::min)]),
        );
        count(&mut h1_raw, k, "sounds", String::new());
    }
    print(&format!("converted {level}"), &converted);
    print(&format!("halo 1 {level} as stored"), &h1_raw);
}

fn floats(b: &[u8], ats: &[usize]) -> String {
    let v: Vec<String> = ats.iter().map(|&a| format!("{:.3}", f(b, a))).collect();
    v.join(" ")
}

/// Looping sounds and sound environments: retail Halo 2's fields, and
/// what the converter writes for a Halo 1 level's.
#[test]
#[ignore = "reads both discs ($H1_ISO, $H2_DISC)"]
fn retail_and_converted_looping_fields() {
    let stems = std::env::var("H2_STEMS").unwrap_or("05a_deltaapproach,03a_oldmombasa".into());
    let mut t: BTreeMap<&'static str, BTreeMap<String, usize>> = BTreeMap::new();
    let add = |t: &mut BTreeMap<&'static str, BTreeMap<String, usize>>,
               k: &'static str,
               v: String| { *t.entry(k).or_default().entry(v).or_default() += 1 };
    for stem in stems.split(',') {
        let bytes = h2_bytes(&std::env::var("H2_DISC").unwrap(), stem);
        let mut m = Map::from_reader(Cursor::new(&bytes[..])).unwrap();
        let tags: Vec<_> = m
            .tags()
            .iter()
            .filter(|t| t.location.is_some())
            .map(|t| (t.id, t.group))
            .collect();
        for (id, g) in tags {
            if g == FourCC(*b"snde") {
                let b = m.read_tag_bytes(id).unwrap();
                add(
                    &mut t,
                    "h2 snde u16 0/2/4",
                    format!(
                        "{:?}",
                        [i16_at(&b, 0), i16_at(&b, 2), i16_at(&b, 4), i16_at(&b, 6)]
                    ),
                );
                add(
                    &mut t,
                    "h2 snde 08 room/roomhf/rolloff/decay/hfratio",
                    floats(&b, &[8, 0xc, 0x10, 0x14, 0x18]),
                );
                add(
                    &mut t,
                    "h2 snde 1c refl/delay/reverb/delay",
                    floats(&b, &[0x1c, 0x20, 0x24, 0x28]),
                );
                add(
                    &mut t,
                    "h2 snde 2c diff/dens/hfref/rest",
                    floats(&b, &[0x2c, 0x30, 0x34, 0x38, 0x3c, 0x40, 0x44]),
                );
            }
            if g == FourCC(*b"lsnd") {
                let b = m.read_tag_bytes(id).unwrap();
                add(
                    &mut t,
                    "h2 lsnd flags",
                    format!("{:#x}", u32::from_le_bytes(b[0..4].try_into().unwrap())),
                );
                add(&mut t, "h2 lsnd 04..14", floats(&b, &[4, 8, 0xc, 0x10]));
                let block = |m: &mut Map<_>, at: usize, len: usize| -> Vec<u8> {
                    let n = u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
                    let a = u32::from_le_bytes(b[at + 4..at + 8].try_into().unwrap());
                    if n == 0 {
                        return vec![];
                    }
                    let o = m.file_offset(a, n * len as u32).unwrap();
                    m.read_file_range(o, u64::from(n) * len as u64).unwrap()
                };
                for e in block(&mut m, 0x1c, 0x58).chunks_exact(0x58) {
                    add(
                        &mut t,
                        "h2 track flags",
                        format!("{:#x}", u32::from_le_bytes(e[4..8].try_into().unwrap())),
                    );
                    add(
                        &mut t,
                        "h2 track gain/fadein/fadeout",
                        floats(e, &[8, 0xc, 0x10]),
                    );
                    add(
                        &mut t,
                        "h2 track 3c output/40..58",
                        format!(
                            "{:#x} {}",
                            u32::from_le_bytes(e[0x3c..0x40].try_into().unwrap()),
                            floats(e, &[0x50, 0x54])
                        ),
                    );
                }
                for e in block(&mut m, 0x24, 0x34).chunks_exact(0x34) {
                    add(
                        &mut t,
                        "h2 detail period/gain",
                        floats(e, &[0xc, 0x10, 0x14]),
                    );
                    add(
                        &mut t,
                        "h2 detail flags",
                        format!(
                            "{:#x}",
                            u32::from_le_bytes(e[0x18..0x1c].try_into().unwrap())
                        ),
                    );
                    add(
                        &mut t,
                        "h2 detail yaw/pitch/dist",
                        floats(e, &[0x1c, 0x20, 0x24, 0x28, 0x2c, 0x30]),
                    );
                }
            }
        }
    }
    let level = std::env::var("H1_LEVEL").unwrap_or("a10".into());
    let path = std::env::var("H1_ISO").unwrap();
    let mut iso = xiso::Xiso::open(BufReader::new(std::fs::File::open(path).unwrap())).unwrap();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = d.map(&level).expect("level on the disc").clone();
    let h1 = H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
    for tag in h1.tags() {
        if tag.group == h1sounds::looping::SNDE {
            let b = h1.tag_data(tag).unwrap();
            add(
                &mut t,
                "h1 snde u16 0/2/4",
                format!(
                    "{:?}",
                    [i16_at(b, 0), i16_at(b, 2), i16_at(b, 4), i16_at(b, 6)]
                ),
            );
            add(
                &mut t,
                "h1 snde 08 room/roomhf/rolloff/decay/hfratio",
                floats(b, &[8, 0xc, 0x10, 0x14, 0x18]),
            );
            add(
                &mut t,
                "h1 snde 1c refl/delay/reverb/delay",
                floats(b, &[0x1c, 0x20, 0x24, 0x28]),
            );
            add(
                &mut t,
                "h1 snde 2c diff/dens/hfref/rest",
                floats(b, &[0x2c, 0x30, 0x34, 0x38, 0x3c, 0x40, 0x44]),
            );
        }
        if tag.group == h1sounds::looping::LSND {
            let b = h1.tag_data(tag).unwrap();
            add(
                &mut t,
                "h1 lsnd 00..2c",
                floats(b, &[4, 8, 0xc, 0x10, 0x14, 0x18, 0x1c, 0x20, 0x24, 0x28]),
            );
            let l = h1sounds::looping::read_looping(&h1, tag).unwrap();
            for tr in &l.tracks {
                add(&mut t, "h1 track flags", format!("{:#x}", tr.flags));
                add(
                    &mut t,
                    "h1 track gain/fadein/fadeout",
                    format!("{:.3} {:.3} {:.3}", tr.gain_db, tr.fade_in, tr.fade_out),
                );
            }
            for dt in &l.details {
                add(
                    &mut t,
                    "h1 detail period/gain",
                    format!("{:.3?} {:.3}", dt.period, dt.gain_db),
                );
                add(
                    &mut t,
                    "h1 detail yaw/pitch/dist",
                    format!("{:.3?} {:.3?} {:.3?}", dt.yaw, dt.pitch, dt.distance),
                );
            }
        }
    }
    for (k, v) in &t {
        let mut v: Vec<_> = v.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1));
        println!("{k}: {} distinct", v.len());
        for (s, n) in v.iter().take(6) {
            println!("    x{n:<4} {s}");
        }
    }
}

/// The `snd!` flags the engine reads for distance and cone follow one rule
/// on every retail map: a sound whose playback distances are 0 sets the
/// class distance bits (`0x400` minimum, `0x800` maximum) and one with
/// distances of its own sets neither; every sound sets `0x1000` (no cone).
/// Every Halo 1 sound on the disc converts to the same rule.
#[test]
#[ignore = "reads both discs ($H1_ISO, $H2_DISC)"]
fn distance_and_cone_flags_follow_the_retail_rule() {
    use h1sounds::convert::snd::{CLASS_MAX_DISTANCE, CLASS_MIN_DISTANCE, NO_CONE};
    let rule = |fl: u16, pb: &[u8]| {
        (fl & CLASS_MIN_DISTANCE != 0) == (f(pb, 0) == 0.0)
            && (fl & CLASS_MAX_DISTANCE != 0) == (f(pb, 4) == 0.0)
            && fl & NO_CONE != 0
    };
    let iso = std::env::var("H2_DISC").expect("set H2_DISC to the Halo 2 disc");
    let mut x = xiso::Xiso::open(BufReader::new(std::fs::File::open(&iso).unwrap())).unwrap();
    let stems: Vec<String> = x
        .entries()
        .unwrap()
        .into_iter()
        .filter(|e| !e.is_dir && e.path.ends_with(".map"))
        .map(|e| {
            let n = e.path.rsplit(['/', '\\']).next().unwrap();
            n.trim_end_matches(".map").to_string()
        })
        .collect();
    let (mut retail, mut defaults, mut broken) = (0usize, 0usize, Vec::new());
    for stem in stems {
        let bytes = h2_bytes(&iso, &stem);
        let mut m = Map::from_reader(Cursor::new(&bytes[..])).unwrap();
        let Some(ugh) = m
            .tags()
            .iter()
            .find(|t| t.group == FourCC(*b"ugh!"))
            .cloned()
        else {
            continue;
        };
        let gb = m.read_tag_bytes(ugh.id).unwrap();
        let g = SoundGestalt::decode(&gb, ugh.location.unwrap().address).unwrap();
        let ids: Vec<_> = m
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"snd!") && t.location.is_some())
            .map(|t| t.id)
            .collect();
        for id in ids {
            let h = m.read_tag_bytes(id).unwrap();
            let fl = u16::from_le_bytes([h[0], h[1]]);
            let pb = g.element(
                blocks::PLAYBACKS,
                usize::from(u16::from_le_bytes([h[6], h[7]])),
            );
            retail += 1;
            defaults += usize::from(fl & CLASS_MAX_DISTANCE != 0);
            if !rule(fl, pb) {
                broken.push(format!("{stem}: {id:?} flags {fl:#x}"));
            }
        }
    }
    println!("retail: {retail} sounds, {defaults} with the class distances");
    assert!(
        broken.is_empty(),
        "{} break the rule: {:?}",
        broken.len(),
        &broken[..broken.len().min(10)]
    );

    let path = std::env::var("H1_ISO").expect("set H1_ISO to the Halo 1 disc");
    let mut iso = xiso::Xiso::open(BufReader::new(std::fs::File::open(path).unwrap())).unwrap();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let (mut converted, mut defaults) = (0usize, 0usize);
    for e in d.maps.clone() {
        let h1 = H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
        for tag in h1.tags().iter().filter(|t| t.group == SND) {
            let s = Sound::read(&h1, tag).unwrap();
            let Ok(c) = convert_sound(&s, h1.bytes()) else {
                continue;
            };
            let fl = u16::from_le_bytes([c.header[0], c.header[1]]);
            assert!(rule(fl, &c.playback), "{}: flags {fl:#x}", tag.name);
            converted += 1;
            defaults += usize::from(fl & CLASS_MAX_DISTANCE != 0);
        }
    }
    println!("halo 1: {converted} sounds converted, {defaults} with the class distances");
}
