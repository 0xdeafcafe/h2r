//! Every Halo 1 looping sound and sound environment on the owner's disc
//! converts. `#[ignore]`d; nothing from the disc is kept.
//!
//! ```text
//! H1_ISO=disc/halo1.iso cargo test -p h1sounds --test looping_disc -- --ignored --nocapture
//! ```

use std::collections::HashMap;
use std::io::{BufReader, Cursor};

use h1map::H1Map;
use h1sounds::looping::{H2_DETAIL, H2_LEN, H2_TRACK, LSND, SNDE, read_environment, read_looping};
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
}
