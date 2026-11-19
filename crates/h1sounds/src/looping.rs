//! Halo 1 looping sounds (`lsnd`) and sound environments (`snde`) as Halo
//! 2 tags, with the `snd!` tags a looping sound plays converted alongside
//! ([`crate::convert_sound`]). Shared by the multiplayer converter (a
//! level's background sounds) and the campaign (its scripts'
//! `looping_sound` references): [`read_looping`] once per Halo 1 tag, then
//! [`install_looping`] into the set.
//!
//! Halo 1 `lsnd` (D: every `lsnd` on Blood Gulch and a10):
//!
//! | at | field |
//! |---|---|
//! | `+0x00` | flags (bit 0 deafening to AIs, 1 not a loop, 2 stops music) |
//! | `+0x20` | maximum distance (`f32`) |
//! | `+0x2c` | continuous damage effect (`cdmg` reference, 16 bytes) |
//! | `+0x3c` | tracks: block, `0xa0` each |
//! | `+0x48` | detail sounds: block, `0x68` each |
//!
//! A track: flags `+0x00` (bit 0 fade in at start, 1 fade out at stop, 2
//! fade in alternate), gain `+0x04` (linear), fade in `+0x08` and out
//! `+0x0c` (seconds), and `snd!` references (16 bytes each) start `+0x30`,
//! loop `+0x40`, end `+0x50`, alternate loop `+0x80`, alternate end
//! `+0x90`. A detail sound: `snd!` `+0x00`, random period bounds `+0x10`,
//! gain `+0x18` (linear), flags `+0x1c`, yaw bounds `+0x50`, pitch bounds
//! `+0x58`, distance bounds `+0x60`.
//!
//! Halo 2 `lsnd` (`0x2c`, schema plus D: Coagulation's and Beaver Creek's
//! eleven): flags `+0x00`, two distances `+0x0c`/`+0x10` (retail 0.9..15
//! and 5..60; written 1 and Halo 1's maximum distance), `cdmg` `+0x14`
//! (8 bytes), tracks `+0x1c` (`0x58` each), detail sounds `+0x24` (`0x34`
//! each). A track: name `+0x00` (string ID), flags `+0x04`, gain `+0x08`
//! (dB), fade in `+0x0c`, fade out `+0x10`, `snd!` in `+0x14`, loop
//! `+0x1c`, out `+0x24`, alternate loop `+0x2c`, alternate out `+0x34`,
//! output effect `+0x3c` (retail 1 front and 2 rear speakers), alternate
//! transition in `+0x40` and out `+0x48`, alternate crossfade `+0x50` and
//! fade out `+0x54`. A detail sound: name `+0x00`, `snd!` `+0x04`, random
//! period bounds `+0x0c`, gain `+0x14` (dB), flags `+0x18`, yaw `+0x1c`,
//! pitch `+0x24` and distance bounds `+0x2c`.
//!
//! `snde` is `0x48` in both games, the same twelve reverb values from
//! `+0x08` (D: Halo 1's hallway and Halo 2's `cpaul_hallway` share decay
//! time 1.49, decay HF ratio 0.59, reflection delay 0.007, reverb delay
//! 0.011 and HF reference 5000), but the four intensities (room `+0x08`,
//! room HF `+0x0c`, reflections `+0x1c`, reverb `+0x24`) are linear in
//! Halo 1 and decibels in Halo 2.

use std::collections::HashMap;

use h1map::{Block as H1Block, Group, H1Map, Tag, TagRef};
use h1models::Converted;
use map::build::{Block, TagSet};
use map::{FourCC, TagId};
use tags::defs::{self, Defs, Target, Val};

use crate::convert::{ConvertedSound, convert_sound, decibels};
use crate::h1::{SND, Sound};
use crate::{Error, Result};

/// Halo 1 `lsnd` group.
pub const LSND: Group = Group::new(b"lsnd");
/// Halo 1 `snde` group.
pub const SNDE: Group = Group::new(b"snde");

/// Halo 1 `lsnd` header length.
pub const H1_LEN: usize = 0x54;
/// Halo 1 track and detail sound element lengths.
pub const H1_TRACK: usize = 0xa0;
pub const H1_DETAIL: usize = 0x68;
/// Halo 2 `lsnd` header, track and detail sound lengths.
pub const H2_LEN: usize = 0x2c;
pub const H2_TRACK: usize = 0x58;
pub const H2_DETAIL: usize = 0x34;
/// `snde` length, both games.
pub const SNDE_LEN: usize = 0x48;

/// Flags both games give the same bit: deafening to AIs, not a loop,
/// stops music.
const SHARED_FLAGS: u32 = 0b111;

/// One track: its sounds by Halo 1 tag path, in Halo 2's order (in, loop,
/// out, alternate loop, alternate out).
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub flags: u32,
    pub gain_db: f32,
    pub fade_in: f32,
    pub fade_out: f32,
    pub sounds: [Option<String>; 5],
}

/// One detail sound, played at random.
#[derive(Debug, Clone, PartialEq)]
pub struct Detail {
    pub sound: Option<String>,
    pub period: [f32; 2],
    pub gain_db: f32,
    pub flags: u32,
    pub yaw: [f32; 2],
    pub pitch: [f32; 2],
    pub distance: [f32; 2],
}

/// A Halo 1 looping sound, read and its sounds converted, ready to add.
#[derive(Debug, Clone)]
pub struct Looping {
    /// The Halo 1 tag path (the new tag's name).
    pub name: String,
    pub flags: u32,
    pub max_distance: f32,
    pub tracks: Vec<Track>,
    pub details: Vec<Detail>,
    /// Every `snd!` the tracks and details name, converted, by path.
    pub sounds: Vec<(String, ConvertedSound)>,
    /// Sounds that didn't convert (their references are left null).
    pub notes: Vec<String>,
}

fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(b[at..at + 4].try_into().expect("4"))
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("4"))
}

/// A Halo 1 sound's decibels; silence is -100 dB (not [`decibels`]'s 0).
fn gain_db(linear: f32) -> f32 {
    if linear <= 0.0 {
        -100.0
    } else {
        decibels(linear)
    }
}

/// The elements of a Halo 1 block field, `len` bytes each.
fn elements<'a>(h1: &'a H1Map, b: &[u8], at: usize, len: usize) -> Result<Vec<&'a [u8]>> {
    let block =
        H1Block::read(b, at).ok_or_else(|| Error::Invalid(format!("no block at {at:#x}")))?;
    Ok(h1.block(block, len)?.chunks_exact(len).collect())
}

/// Reads `tag` (an `lsnd`) and converts every sound it names.
pub fn read_looping(h1: &H1Map, tag: &Tag) -> Result<Looping> {
    let mut out = read_definition(h1, tag)?;
    for name in out.sound_names() {
        let t = h1.find(SND, &name).expect("named by a reference above");
        match Sound::read(h1, t).and_then(|s| convert_sound(&s, h1.bytes())) {
            Ok(c) => out.sounds.push((name, c)),
            Err(e) => out.notes.push(format!("{name}: {e}")),
        }
    }
    Ok(out)
}

/// The looping definition and its named dependencies, before reading any samples.
pub fn read_definition(h1: &H1Map, tag: &Tag) -> Result<Looping> {
    if tag.group != LSND {
        return Err(Error::Invalid(format!("{} is a {}", tag.name, tag.group)));
    }
    let b = h1.tag_data(tag)?;
    if b.len() < H1_LEN {
        return Err(Error::Invalid(format!("{}: {} bytes", tag.name, b.len())));
    }
    if !TagRef::read(b, 0x2c)
        .expect("complete lsnd header")
        .id
        .is_none()
    {
        return Err(Error::Unsupported(format!(
            "{}: continuous damage on a looping sound",
            tag.name
        )));
    }
    let sound_at = |e: &[u8], at: usize| -> Result<Option<String>> {
        let r = TagRef::read(e, at)
            .ok_or_else(|| Error::Invalid(format!("{}: truncated snd! reference", tag.name)))?;
        if r.id.is_none() {
            return Ok(None);
        }
        let t = h1.tag(r.id)?;
        if r.group != SND || t.group != SND {
            return Err(Error::Invalid(format!(
                "{}: expected snd!, found {} {}",
                tag.name, t.group, t.name
            )));
        }
        Ok(Some(t.name.clone()))
    };
    let tracks = elements(h1, b, 0x3c, H1_TRACK)?
        .into_iter()
        .map(|e| {
            Ok(Track {
                // Fade in at start, fade out at stop; Halo 1's "fade in
                // alternate" is Halo 2's crossfade of the alternate loop.
                flags: u32_at(e, 0) & 0b111,
                gain_db: gain_db(f32_at(e, 4)),
                fade_in: f32_at(e, 8),
                fade_out: f32_at(e, 0xc),
                sounds: [
                    sound_at(e, 0x30)?,
                    sound_at(e, 0x40)?,
                    sound_at(e, 0x50)?,
                    sound_at(e, 0x80)?,
                    sound_at(e, 0x90)?,
                ],
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let details = elements(h1, b, 0x48, H1_DETAIL)?
        .into_iter()
        .map(|e| {
            Ok(Detail {
                sound: sound_at(e, 0)?,
                period: [f32_at(e, 0x10), f32_at(e, 0x14)],
                gain_db: gain_db(f32_at(e, 0x18)),
                // Don't play with, or without, the alternate: the same bits.
                flags: u32_at(e, 0x1c) & 0b11,
                yaw: [f32_at(e, 0x50), f32_at(e, 0x54)],
                pitch: [f32_at(e, 0x58), f32_at(e, 0x5c)],
                distance: [f32_at(e, 0x60), f32_at(e, 0x64)],
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Looping {
        name: tag.name.clone(),
        flags: u32_at(b, 0) & SHARED_FLAGS,
        max_distance: f32_at(b, 0x20),
        tracks,
        details,
        sounds: Vec::new(),
        notes: Vec::new(),
    })
}

impl Looping {
    /// Every sound path the tracks and details name, once each, in order.
    pub fn sound_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        let all = self
            .tracks
            .iter()
            .flat_map(|t| t.sounds.iter())
            .chain(self.details.iter().map(|d| &d.sound));
        for n in all.flatten() {
            if !names.contains(n) {
                names.push(n.clone());
            }
        }
        names
    }

    /// The Halo 2 tag, its sounds named through `ids` (path to datum); a
    /// path `ids` lacks stays a null reference.
    pub fn to_h2(&self, ids: &HashMap<String, TagId>) -> Converted {
        let def = Defs::builtin()
            .group(FourCC(*b"lsnd"))
            .expect("lsnd definition");
        let mut root = defs::default_record(def);
        let snd_ref = |name: Option<&String>| Val::Ref {
            group: FourCC(*b"snd!").as_u32(),
            target: name
                .and_then(|n| ids.get(n))
                .copied()
                .map_or(Target::Null, Target::Id),
        };
        root.set("flags", Val::Int(self.flags.into()))
            .expect("lsnd flags");
        root.set("distance_c", Val::Float(1.0))
            .expect("lsnd distance");
        root.set("distance_10", Val::Float(self.max_distance))
            .expect("lsnd distance");
        root.set(
            "continuous_damage",
            Val::Ref {
                group: FourCC(*b"cdmg").as_u32(),
                target: Target::Null,
            },
        )
        .expect("lsnd damage");
        let track = def.child("tracks").expect("lsnd track");
        let mut tracks = Vec::with_capacity(self.tracks.len());
        for t in &self.tracks {
            let mut r = defs::default_record(track);
            for (key, value) in [
                ("flags", Val::Int(t.flags.into())),
                ("gain", Val::Float(t.gain_db)),
                ("fade_in", Val::Float(t.fade_in)),
                ("fade_out", Val::Float(t.fade_out)),
            ] {
                r.set(key, value).expect("lsnd track field");
            }
            for (key, name) in [
                "sound_in",
                "sound_loop",
                "sound_out",
                "sound_alternate_loop",
                "sound_alternate_out",
            ]
            .into_iter()
            .zip(&t.sounds)
            {
                r.set(key, snd_ref(name.as_ref())).expect("track sound");
            }
            for key in [
                "sound_alternate_transition_in",
                "sound_alternate_transition_out",
            ] {
                r.set(key, snd_ref(None)).expect("transition sound");
            }
            tracks.push(r);
        }
        root.set("tracks", Val::Block(tracks)).expect("lsnd tracks");
        let detail = def.child("detail_sounds").expect("lsnd detail");
        let mut details = Vec::with_capacity(self.details.len());
        for d in &self.details {
            let mut r = defs::default_record(detail);
            for (key, value) in [
                ("sound", snd_ref(d.sound.as_ref())),
                ("random_period_bounds", Val::floats(&d.period)),
                ("gain", Val::Float(d.gain_db)),
                ("flags", Val::Int(d.flags.into())),
                ("yaw_bounds", Val::floats(&d.yaw)),
                ("pitch_bounds", Val::floats(&d.pitch)),
                ("distance_bounds", Val::floats(&d.distance)),
            ] {
                r.set(key, value).expect("lsnd detail field");
            }
            details.push(r);
        }
        root.set("detail_sounds", Val::Block(details))
            .expect("lsnd details");
        Converted {
            group: FourCC(*b"lsnd"),
            root: defs::to_built(def, &root, &mut |_| None).expect("lsnd fields match definition"),
            raw: Vec::new(),
        }
    }
}

/// Install every source loop, including ones with no script or BSP reference.
/// Each source sample tag is read once even when several tracks share it.
/// Replace a retail namesake in its original slot so existing datums stay valid.
pub fn install_level_loops(h1: &H1Map, set: &mut TagSet) -> Result<Vec<TagId>> {
    let loops = h1
        .tags()
        .iter()
        .filter(|t| t.group == LSND)
        .map(|t| read_definition(h1, t))
        .collect::<Result<Vec<_>>>()?;
    let names = loops.iter().flat_map(Looping::sound_names);
    let sounds = crate::convert::install_source_sounds(h1, set, names)?;
    loops
        .iter()
        .map(|l| crate::convert::install_named(l.to_h2(&sounds), set, &l.name))
        .collect()
}

/// Adds `loops` to `set`: every sound they name that the set has no `snd!`
/// of that path for (one [`crate::install_sounds`] pass), then each
/// looping sound, or the set's own `lsnd` of that path. Returns the
/// looping sounds' datums, in order.
pub fn install_looping(set: &mut TagSet, loops: &[Looping]) -> Result<Vec<TagId>> {
    let snd = FourCC(*b"snd!");
    let mut ids: HashMap<String, TagId> = HashMap::new();
    let mut new = Vec::new();
    for (name, s) in loops.iter().flat_map(|l| l.sounds.iter()) {
        if ids.contains_key(name) || new.iter().any(|(n, _)| n == name) {
            continue;
        }
        match set.find(snd, name) {
            Some(id) => {
                ids.insert(name.clone(), id);
            }
            None => new.push((name.clone(), s.clone())),
        }
    }
    let names: Vec<String> = new.iter().map(|(n, _)| n.clone()).collect();
    if !new.is_empty() {
        let got = crate::install_sounds(set, new)?;
        ids.extend(names.into_iter().zip(got));
    }
    let mut out = Vec::with_capacity(loops.len());
    for l in loops {
        let id = match set.find(FourCC(*b"lsnd"), &l.name) {
            Some(id) => id,
            None => l
                .to_h2(&ids)
                .install(set, &l.name)
                .map_err(|e| Error::Invalid(e.to_string()))?,
        };
        out.push(id);
    }
    Ok(out)
}

/// A Halo 1 sound environment as Halo 2's (module docs).
pub fn read_environment(h1: &H1Map, tag: &Tag) -> Result<Converted> {
    if tag.group != SNDE {
        return Err(Error::Invalid(format!("{} is a {}", tag.name, tag.group)));
    }
    let b = h1.tag_data(tag)?;
    let b = b
        .get(..SNDE_LEN)
        .ok_or_else(|| Error::Invalid(format!("{}: {} bytes", tag.name, b.len())))?;
    Ok(environment(b))
}

/// [`read_environment`] on the tag's bytes.
pub fn environment(h1: &[u8]) -> Converted {
    let mut root = Block::new(h1[..SNDE_LEN].to_vec());
    for at in [0x08, 0x0c, 0x1c, 0x24] {
        root.put_f32(at, gain_db(f32_at(h1, at)));
    }
    Converted {
        group: FourCC(*b"snde"),
        root,
        raw: Vec::new(),
    }
}

/// Adds a converted sound environment under `name`, or finds the set's
/// own `snde` of that path.
pub fn install_environment(set: &mut TagSet, name: &str, c: &Converted) -> Result<TagId> {
    match set.find(FourCC(*b"snde"), name) {
        Some(id) => Ok(id),
        None => c
            .clone()
            .install(set, name)
            .map_err(|e| Error::Invalid(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_unreferenced_loops_share_one_sample_and_keep_existing_datums() {
        use h1map::testing::{BASE, SCNR_ID, Spec, build_with_extra_tags, put};
        use map::build::TagPayload;
        use map::{CacheType, Map};
        use std::io::Cursor;

        let sample = "engine_sample";
        let spec = |group, name, len| Spec {
            group,
            parent: None,
            name,
            data: Some(vec![0; len]),
        };
        let mut fixture = build_with_extra_tags(
            &Default::default(),
            vec![
                spec(b"snd!", sample, 0x180),
                spec(b"lsnd", "engine", H1_LEN + H1_DETAIL),
                spec(b"lsnd", "ambient", H1_LEN + H1_DETAIL),
            ],
        );
        let slot = fixture.data_at.len() - 3;
        let sound = fixture.data_at[slot].unwrap();
        let range = sound + crate::h1::HEADER_LEN;
        let perm = range + crate::h1::PITCH_RANGE_LEN;
        let address = |off: usize| BASE + (off - h1map::HEADER_LEN) as u32;
        let f = &mut fixture.file;
        put(f, sound + 4, 13);
        put(f, sound + 0x7c, u32::MAX);
        put(f, sound + 0x98, 1);
        put(f, sound + 0x9c, address(range));
        put(f, range + 0x2c, 1);
        put(f, range + 0x3c, 1);
        put(f, range + 0x40, address(perm));
        f[perm + 0x2a..perm + 0x2c].copy_from_slice(&u16::MAX.to_le_bytes());
        put(f, perm + 0x40, 4);
        put(f, perm + 0x48, (perm + crate::h1::PERMUTATION_LEN) as u32);
        f[perm + crate::h1::PERMUTATION_LEN..][..4].copy_from_slice(&[1, 2, 3, 4]);

        for off in &fixture.data_at[slot + 1..] {
            let at = off.unwrap();
            let detail = at + H1_LEN;
            put(f, at + 0x38, u32::MAX);
            put(f, at + 0x48, 1);
            put(f, at + 0x4c, address(detail));
            put(f, detail, SND.0);
            put(f, detail + 12, SCNR_ID + slot as u32);
        }
        let source = H1Map::from_decompressed(fixture.file).unwrap();
        let mut set = TagSet::new(CacheType::Campaign, "tiny", "tiny");
        let old_sound = set
            .add_tag(FourCC(*b"snd!"), sample, TagPayload::Empty)
            .unwrap();
        let old_loop = set
            .add_tag(FourCC(*b"lsnd"), "engine", TagPayload::Empty)
            .unwrap();
        let loops = install_level_loops(&source, &mut set).unwrap();
        assert_eq!(loops.len(), 2);
        assert_eq!(loops[0], old_loop);
        assert_eq!(set.find(FourCC(*b"snd!"), sample), Some(old_sound));
        assert_eq!(set.raw.len(), 1);
        assert_eq!(set.raw[0].bytes, [1, 2, 3, 4]);

        let (mut bytes, _) = set.build().unwrap();
        let clean = crate::audit::audit(&bytes, Some(&source), &[]).unwrap();
        assert_eq!((clean.loops, clean.converted), (2, 1));
        assert!(clean.problems.is_empty(), "{:?}", clean.problems);

        // Replace a valid detail sound's datum with a stale one. The sound
        // tag and its samples still audit cleanly, but the loop cannot play it.
        let mut m = Map::from_reader(Cursor::new(&bytes)).unwrap();
        let root = m.read_tag_bytes(old_loop).unwrap();
        let at = m.file_offset(u32_at(&root, 0x28) + 8, 4).unwrap() as usize;
        put(&mut bytes, at, old_sound.0 ^ (1 << 16));
        let audit = crate::audit::audit(&bytes, Some(&source), &[]).unwrap();
        assert!(
            audit
                .problems
                .iter()
                .any(|s| s.contains("engine: detail 0"))
        );
    }

    #[test]
    fn environment_intensities_become_decibels() {
        let mut b = vec![0u8; SNDE_LEN];
        let put =
            |b: &mut Vec<u8>, at: usize, v: f32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut b, 0x08, 1.0);
        put(&mut b, 0x0c, 0.1);
        put(&mut b, 0x14, 1.49);
        put(&mut b, 0x1c, 0.0);
        put(&mut b, 0x24, 0.01);
        put(&mut b, 0x34, 5000.0);
        let c = environment(&b);
        let f = |at: usize| f32_at(&c.root.bytes, at);
        assert_eq!(c.group, FourCC(*b"snde"));
        assert_eq!(f(0x08), 0.0);
        assert!((f(0x0c) + 20.0).abs() < 1e-4);
        assert_eq!(f(0x14), 1.49);
        assert_eq!(f(0x1c), -100.0);
        assert!((f(0x24) + 40.0).abs() < 1e-4);
        assert_eq!(f(0x34), 5000.0);
    }

    #[test]
    fn a_looping_sound_names_its_sounds_or_null() {
        let l = Looping {
            name: "sound\\a\\loop".into(),
            flags: 2,
            max_distance: 3.0,
            tracks: vec![Track {
                flags: 3,
                gain_db: -6.0,
                fade_in: 0.5,
                fade_out: 0.25,
                sounds: [Some("in".into()), Some("loop".into()), None, None, None],
            }],
            details: vec![Detail {
                sound: Some("bird".into()),
                period: [1.0, 3.0],
                gain_db: 0.0,
                flags: 1,
                yaw: [-1.0, 1.0],
                pitch: [-0.5, 0.5],
                distance: [0.0, 0.0],
            }],
            sounds: Vec::new(),
            notes: Vec::new(),
        };
        assert_eq!(l.sound_names(), ["in", "loop", "bird"]);
        let ids: HashMap<String, TagId> = [
            ("loop".to_string(), TagId(0xe0010001)),
            ("bird".to_string(), TagId(0xe0020002)),
        ]
        .into();
        let c = l.to_h2(&ids);
        assert_eq!(c.group, FourCC(*b"lsnd"));
        assert_eq!(c.root.bytes.len(), H2_LEN);
        assert_eq!(u32_at(&c.root.bytes, 0), 2);
        assert_eq!(f32_at(&c.root.bytes, 0x10), 3.0);
        let (tracks, details) = match &c.root.fixups[..] {
            [t, d] => match (&t.kind, &d.kind) {
                (
                    map::build::FixupKind::Block { count: 1, child: t },
                    map::build::FixupKind::Block { count: 1, child: d },
                ) => (t, d),
                _ => panic!("two one-element blocks"),
            },
            _ => panic!("two fixups"),
        };
        assert_eq!(f32_at(&tracks.bytes, 8), -6.0);
        // "in" isn't in `ids`: a null reference; "loop" is a fixup.
        assert_eq!(u32_at(&tracks.bytes, 0x14), FourCC(*b"snd!").as_u32());
        assert_eq!(u32_at(&tracks.bytes, 0x18), u32::MAX);
        let refs: Vec<(u32, TagId)> = tracks
            .fixups
            .iter()
            .map(|f| match f.kind {
                map::build::FixupKind::TagRef(id) => (f.offset, id),
                _ => panic!("a tag reference"),
            })
            .collect();
        assert_eq!(refs, [(0x1c, TagId(0xe0010001))]);
        assert_eq!(details.fixups.len(), 1);
        assert_eq!(details.fixups[0].offset, 4);
        assert_eq!(f32_at(&details.bytes, 0x10), 3.0);
        assert_eq!(u32_at(&details.bytes, 0x18), 1);
    }
}
