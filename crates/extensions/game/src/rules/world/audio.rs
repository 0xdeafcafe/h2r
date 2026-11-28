//! `sound_to`, `sound_to_team` and `sound_to_all` (docs/game/rules-world-api.md,
//! "Audio"): a sound of the map, played in the ears of an audience.
//!
//! On the authority, [`publish`] turns each sound the VM made since the last
//! boundary into a sound cue for the show channel (`super::show`) and a
//! [`Call::Play`] that [`play`] makes for this machine's own players. A
//! `Viewer` plays the cue it receives the same way
//! (`super::show::present`). A sound is started at each audience player's
//! living unit by the game's own sound start at the unit's head marker
//! (`SOUND_START_AT_MARKER`, `0x1891d0`), the call a replicated dialogue line
//! and the agent's `audio.play` make; no positioned or unpositioned
//! `sound_impulse_start` is read for it (research row R4).
//!
//! The entries the research rows R4 and R5 leave open are listed by
//! `rules::caps` and refused by name: `say`, `sound_at`, `music`,
//! `music_stop` and `announce`.
//!
//! A script names a sound by its tag path, and the running game holds no
//! tag paths. The loaded map's file does: its tag index names each `snd!`
//! tag and gives its datum, the datum the game loaded the tag under.
//! [`activate`] reads the loaded map's file at each game start and installs
//! [`set_resolver`]'s lookup from it; a host with no lookup, or a map that
//! does not hold a sound the script plays, refuses the script ([`refusal`]),
//! by the sound's path.

use std::collections::HashMap;
use std::io::{Read, Seek};
use std::sync::{Mutex, MutexGuard};

use ::rules::caps::id;
use ::rules::ir::{Ir, Op, TagGroup};
use ::rules::labels::ANNOUNCER;
use ::rules::show::{self as cue, Audience, Cue, SoundCue, To};
use ::rules::vm::{Arg, Call as VmCall};
use agent::sim::calls::addr;
use guest_layout::GuestMemory;
use guest_layout::entry::{Cx, EntryError, GuestCall};

use super::super::runtime::Call;
use super::show;
use crate::firefight::runtime::read_players;

/// A host's lookup of a sound tag's datum by its path.
pub type Resolver = Box<dyn Fn(&str) -> Option<u32> + Send + Sync>;

static RESOLVER: Mutex<Option<Resolver>> = Mutex::new(None);

fn resolver() -> MutexGuard<'static, Option<Resolver>> {
    RESOLVER.lock().unwrap_or_else(|e| e.into_inner())
}

/// Gives the host its lookup of a sound tag by path, or takes it away.
pub fn set_resolver(lookup: Option<Resolver>) {
    *resolver() = lookup;
}

fn resolve(path: &str) -> Option<u32> {
    resolver().as_ref().and_then(|lookup| lookup(path))
}

/// The sound tags `map` names: each `snd!` tag's path (with `/`, as the
/// start gate's census spells it) and its datum.
pub fn sounds_of<R: Read + Seek>(map: &map::Map<R>) -> HashMap<String, u32> {
    map.tags()
        .iter()
        .filter(|t| &t.group.0 == b"snd!")
        .filter_map(|t| {
            let name = t.name.as_deref().filter(|n| !n.is_empty())?;
            Some((name.replace('\\', "/"), t.id.0))
        })
        .collect()
}

/// Installs the lookup of the sounds in `source`, a map file; none (a map
/// that cannot be read) takes the lookup away, so a script that plays a
/// sound is refused by its path.
pub fn install_map(source: Option<vfs::FileSource>) {
    let table = source.and_then(|source| match source {
        vfs::FileSource::Memory(bytes) => {
            map::Map::from_reader(std::io::Cursor::new(&bytes[..])).ok().map(|m| sounds_of(&m))
        }
        vfs::FileSource::Host(f) => {
            let file = std::fs::File::open(&f.path).ok()?;
            let m = map::Map::from_region(file, f.offset, f.len).ok()?;
            Some(sounds_of(&m))
        }
        vfs::FileSource::Spliced(f) => {
            let m = map::Map::from_reader(f.reader().ok()?).ok()?;
            Some(sounds_of(&m))
        }
    });
    set_resolver(table.map(|t| Box::new(move |path: &str| t.get(path).copied()) as Resolver));
}

/// The loaded map's name and the VFS path of its active slot, read from
/// guest memory.
pub fn loaded(guest: &runtime::Guest) -> Option<(String, Option<String>)> {
    let name = crate::telemetry::loaded_map_name(guest).ok().filter(|n| !n.is_empty())?;
    Some((name, crate::map_slots::active_file(guest)))
}

/// A game started: the lookup is of the loaded map's file ([`loaded`]), read
/// through the VFS. Call it with no lock on the guest's memory held.
pub fn activate(vfs: &Mutex<vfs::Vfs>, loaded: Option<(String, Option<String>)>) {
    let source = loaded.and_then(|(name, slot)| {
        let mut v = vfs.lock().unwrap_or_else(|e| e.into_inner());
        crate::map_override::map_source(&mut v, &name, slot.as_deref())
    });
    install_map(source);
}

/// Why this host cannot run `ir`, by the sound's path: the script plays a
/// sound and the host has no lookup of sound tags, or the loaded map holds
/// no sound tag at a path the script names.
pub fn refusal(ir: &Ir) -> Option<String> {
    // An announcer line is one of ours: the clip is named, or refused by name.
    if let Some(t) = ir.tags.iter().find(|t| {
        t.group == TagGroup::Sound
            && t.path.starts_with(ANNOUNCER)
            && !is_line(&t.path)
    }) {
        return Some(format!(
            "the script plays the announcer line {}, which is not one of ours \
             (art/announcer/, docs/game/rules-world-api.md, \"Audio\")",
            t.path
        ));
    }
    let plays = ir.handlers.iter().flat_map(|h| &h.code).any(|op| {
        matches!(op, Op::Call { entry, .. }
            if matches!(*entry, id::SOUND_TO | id::SOUND_TO_TEAM | id::SOUND_TO_ALL))
    });
    if !plays {
        return None;
    }
    let held = resolver();
    let Some(lookup) = held.as_ref() else {
        let sound = ir
            .tags
            .iter()
            .find(|t| t.group == TagGroup::Sound && !t.path.starts_with(ANNOUNCER))?;
        return Some(format!(
            "the script plays the sound {}, and this host has not read the loaded map's sound tags \
             (docs/game/rules-world-api.md, \"Audio\")",
            sound.path
        ));
    };
    let missing = ir
        .tags
        .iter()
        .find(|t| {
            t.group == TagGroup::Sound && !t.path.starts_with(ANNOUNCER) && lookup(&t.path).is_none()
        })?;
    Some(format!(
        "the script plays the sound {}, which the loaded map does not hold",
        missing.path
    ))
}

/// The sound of a `sound_to*` call and who it is for. A call through
/// `none` plays nothing.
pub fn outgoing(call: &VmCall) -> Option<(To, String)> {
    let Some(Arg::Tag(Some(path))) = call.args.first() else {
        return None;
    };
    let to = match (call.entry.id, call.args.get(1)) {
        (id::SOUND_TO, Some(Arg::Player(Some(p)))) => To::Player(*p),
        (id::SOUND_TO_TEAM, Some(Arg::Team(Some(t)))) => To::Team(*t),
        (id::SOUND_TO_ALL, _) => To::Everyone,
        _ => return None,
    };
    Some((to, path.clone()))
}

/// The cues and local plays of `sounds`, with `audience` resolving each
/// `to`. A sound with an empty audience, or one the blob's tag table does
/// not hold as a sound, is nothing.
pub fn plan(ir: &Ir, sounds: &[(To, String)], audience: impl Fn(&To) -> Audience) -> (Vec<Cue>, Vec<Call>) {
    let mut cues = Vec::new();
    let mut plays = Vec::new();
    for (to, path) in sounds {
        let audience = audience(to);
        let tag = ir
            .tags
            .iter()
            .position(|t| t.group == TagGroup::Sound && t.path == *path)
            .and_then(|i| u16::try_from(i).ok());
        let Some(sound) = tag.filter(|_| audience != 0) else {
            continue;
        };
        cues.push(Cue::Sound(SoundCue { audience, sound }));
        plays.push(Call::Play {
            audience,
            path: path.clone(),
        });
    }
    (cues, plays)
}

/// The sounds the VM made since the last boundary, at `tick`, on the
/// authority: each queued for the channel, and the plays this machine makes
/// for its own players.
pub fn publish(m: &dyn GuestMemory, ir: &Ir, tick: u32, sounds: Vec<(To, String)>) -> Vec<Call> {
    if sounds.is_empty() {
        return Vec::new();
    }
    let (cues, plays) = plan(ir, &sounds, |to| show::audience(m, to));
    for c in cues {
        show::push(tick, c);
    }
    plays
}

/// The living units of this machine's own players in `audience`.
fn units(m: &dyn GuestMemory, audience: Audience) -> Vec<u32> {
    let local = show::local_slots(m);
    read_players(m)
        .unwrap_or_default()
        .into_iter()
        .filter(|(p, _)| p.alive && local.contains(&p.slot) && audience & cue::player(p.slot) != 0)
        .filter_map(|(_, unit)| unit)
        .collect()
}

/// Starts the sound tag `sound` at `unit`'s head marker. One stack argument
/// points to the four parameter words just above it, as the dialogue line's
/// start passes them.
async fn start(cx: &Cx, unit: u32, sound: u32) -> anyhow::Result<()> {
    let parameters = cx.entry().esp.wrapping_sub(16);
    let instance = cx
        .call(
            GuestCall::new(addr::SOUND_START_AT_MARKER)
                .esi(unit)
                .eax(addr::HEAD_MARKER)
                .args([parameters, u32::MAX, sound, 1.0f32.to_bits(), 0]),
        )
        .await
        .map_err(guest)?
        .eax;
    anyhow::ensure!(
        instance != u32::MAX,
        "the game started no sound from tag {sound:#010x} (a tag with no permutation, or a full sound table)"
    );
    Ok(())
}

/// Plays the sound `path` to this machine's own players in `audience`, each
/// at its living unit. A player with no living unit hears nothing.
pub async fn play(cx: &Cx, audience: Audience, path: &str) -> anyhow::Result<()> {
    if path.starts_with(ANNOUNCER) {
        return say(cx.read(|m| show::local_slots(m)), audience, path);
    }
    let units = cx.read(|m| units(m, audience));
    play_at(cx, &units, path).await
}

/// The announcer line `path` (`announcer/<clip>`): the clip joins the
/// announcer's queue when one of this machine's own players is in
/// `audience`, dead or alive. An unknown clip is an error naming it.
fn say(local: impl IntoIterator<Item = u8>, audience: Audience, path: &str) -> anyhow::Result<()> {
    let id = path.strip_prefix(ANNOUNCER).filter(|id| is_line(path) && !id.is_empty());
    let id = id.ok_or_else(|| anyhow::anyhow!("no announcer line {path}"))?;
    if local.into_iter().any(|s| audience & cue::player(s) != 0) {
        crate::medal_announcer::announce_id(id);
    }
    Ok(())
}

/// Whether `path` (`announcer/<clip>`) names a clip of ours.
fn is_line(path: &str) -> bool {
    path.strip_prefix(ANNOUNCER)
        .is_some_and(|id| crate::medal_announcer::clip_bytes(id).is_some())
}

/// Starts the sound `path` at each of `units`; the first failure is the
/// answer, after every unit has been tried.
async fn play_at(cx: &Cx, units: &[u32], path: &str) -> anyhow::Result<()> {
    let sound = resolve(path).ok_or_else(|| anyhow::anyhow!("no sound tag {path} found on this machine"))?;
    let mut failed = None;
    for &unit in units {
        if let Err(e) = start(cx, unit, sound).await {
            failed.get_or_insert(e);
        }
    }
    failed.map_or(Ok(()), Err)
}

fn guest(e: impl Into<EntryError>) -> anyhow::Error {
    anyhow::anyhow!("{}", e.into())
}

/// The lookup is process-wide: a test that sets it holds this for its run.
#[cfg(test)]
pub(crate) static SERIAL: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use ::rules::caps;
    use map::build::{Block, TagPayload, TagSet};
    use map::FourCC;

    fn serial() -> MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// An invented map: a scenario, a globals tag and one sound.
    fn invented_map() -> (Vec<u8>, u32) {
        let (matg, scnr, snd) = (FourCC(*b"matg"), FourCC(*b"scnr"), FourCC(*b"snd!"));
        let mut set = TagSet::new(map::CacheType::Multiplayer, "invented", "test");
        for g in [matg, scnr, snd] {
            set.add_group(g, None, None);
        }
        set.globals = set.add_tag(matg, "globals", TagPayload::Built(Block::zeroed(0x284))).unwrap();
        set.scenario = set
            .add_tag(scnr, "levels\\test\\example", TagPayload::Built(Block::zeroed(0x3e0)))
            .unwrap();
        let stinger = set
            .add_tag(snd, "sound\\stinger", TagPayload::Built(Block::zeroed(0x14)))
            .unwrap();
        (set.build().unwrap().0, stinger.0)
    }

    fn install_invented() -> u32 {
        let (bytes, datum) = invented_map();
        install_map(Some(vfs::FileSource::Memory(bytes.into())));
        datum
    }

    const STINGER: &str = "sound/stinger";

    fn ir() -> Ir {
        ::rules::compile(&format!(
            "rules \"A\" 1\ntype slayer\nrequires audio.play\n\
             on kill(victim, killer) {{ sound_to_all(\"{STINGER}\") }}\n"
        ))
        .unwrap()
    }

    fn call(entry: u16, args: Vec<Arg>) -> VmCall {
        VmCall {
            entry: caps::entry(entry, caps::API_VERSION).unwrap(),
            args,
        }
    }

    fn tag() -> Arg {
        Arg::Tag(Some(STINGER.into()))
    }

    #[test]
    fn a_sound_call_keeps_its_audience_and_path() {
        assert_eq!(
            outgoing(&call(id::SOUND_TO, vec![tag(), Arg::Player(Some(3))])),
            Some((To::Player(3), STINGER.into()))
        );
        assert_eq!(
            outgoing(&call(id::SOUND_TO_TEAM, vec![tag(), Arg::Team(Some(1))])),
            Some((To::Team(1), STINGER.into()))
        );
        assert_eq!(
            outgoing(&call(id::SOUND_TO_ALL, vec![tag()])),
            Some((To::Everyone, STINGER.into()))
        );
        // A call through none plays nothing.
        assert_eq!(outgoing(&call(id::SOUND_TO, vec![tag(), Arg::Player(None)])), None);
        assert_eq!(outgoing(&call(id::SOUND_TO_TEAM, vec![tag(), Arg::Team(None)])), None);
        assert_eq!(outgoing(&call(id::SOUND_TO_ALL, vec![Arg::Tag(None)])), None);
    }

    #[test]
    fn a_plan_reaches_only_its_audience_and_names_only_the_blobs_sounds() {
        let ir = ir();
        let at = ir.tags.iter().position(|t| t.path == STINGER).unwrap() as u16;
        let roster = |to: &To| match to {
            To::Player(p) => cue::player(*p),
            To::Team(1) => cue::player(1) | cue::player(3),
            To::Team(_) => 0,
            To::Everyone => cue::EVERYONE,
        };
        let sounds = vec![
            (To::Player(2), STINGER.to_string()),
            (To::Team(1), STINGER.to_string()),
            // Nobody on the team: nothing.
            (To::Team(5), STINGER.to_string()),
            // A path the blob does not hold as a sound: nothing.
            (To::Everyone, "sound/other".to_string()),
        ];
        let (cues, plays) = plan(&ir, &sounds, roster);
        assert_eq!(
            cues,
            [
                Cue::Sound(SoundCue { audience: cue::player(2), sound: at }),
                Cue::Sound(SoundCue { audience: cue::player(1) | cue::player(3), sound: at }),
            ]
        );
        assert_eq!(
            plays,
            [
                Call::Play { audience: cue::player(2), path: STINGER.into() },
                Call::Play { audience: cue::player(1) | cue::player(3), path: STINGER.into() },
            ]
        );
        // Each cue is one the wire carries and a viewer's check accepts.
        for c in &cues {
            let bytes = c.encode().unwrap();
            let Cue::Sound(back) = Cue::decode(c.kind(), &bytes, &ir.strings).unwrap() else {
                panic!("a sound cue")
            };
            assert_eq!(back.check(&ir.tags), Ok(()));
        }
    }

    #[test]
    fn a_host_with_no_sound_lookup_refuses_a_script_that_plays_one_by_its_path() {
        let _g = serial();
        set_resolver(None);
        let ir = ir();
        let why = refusal(&ir).unwrap();
        assert!(why.contains(STINGER), "{why}");
        // A script that plays no sound is no concern of the lookup.
        let quiet = ::rules::compile("rules \"Q\" 1\ntype slayer\nglobal var n: int = 0\non tick { n = n + 1 }\n");
        assert_eq!(refusal(&quiet.unwrap()), None);
    }

    #[test]
    fn a_sound_the_loaded_map_names_resolves_to_its_datum_and_runs() {
        let _g = serial();
        let datum = install_invented();
        assert_eq!(resolve(STINGER), Some(datum));
        assert_eq!(datum & 0xffff, 2, "the third tag's slot");
        assert_eq!(refusal(&ir()), None);
        // The map's other tags are no sounds.
        assert_eq!(resolve("globals"), None);
        set_resolver(None);
    }

    #[test]
    fn a_sound_the_loaded_map_lacks_is_refused_by_its_path() {
        let _g = serial();
        install_invented();
        let other = ::rules::compile(
            "rules \"A\" 1\ntype slayer\nrequires audio.play\n\
             on kill(victim, killer) { sound_to_all(\"sound/missing\") }\n",
        )
        .unwrap();
        let why = refusal(&other).unwrap();
        assert!(why.contains("sound/missing"), "{why}");
        // A map the host cannot read takes the lookup away.
        install_map(Some(vfs::FileSource::Memory(vec![0u8; 16].into())));
        let why = refusal(&ir()).unwrap();
        assert!(why.contains(STINGER), "{why}");
        set_resolver(None);
    }

    #[test]
    fn a_played_sound_starts_at_each_unit_by_the_maps_datum() {
        use guest_layout::entry::{self, EntryFuture, Regs, Step};
        let _g = serial();
        let datum = install_invented();
        let cx = Cx::new(std::sync::Mutex::new(guest_layout::testing::FakeGuest::new()), Regs::default());
        let units = [0x8001_0003u32, 0x8002_0004];
        let mut f: EntryFuture = {
            let cx = cx.clone();
            Box::pin(async move {
                play_at(&cx, &units, STINGER).await.map_err(|e| EntryError::IsolatedCall(e.to_string()))?;
                Ok(entry::Ret::new(0, 0))
            })
        };
        for unit in units {
            let Step::Call(call) = entry::poll(&mut f, &cx) else {
                panic!("a sound start")
            };
            assert_eq!(call.target, addr::SOUND_START_AT_MARKER);
            assert_eq!(call.esi, Some(unit));
            assert_eq!(call.args[2], datum);
            entry::resume(&cx, Regs { eax: 7, ..Regs::default() });
        }
        assert!(matches!(entry::poll(&mut f, &cx), Step::Done(Ok(_))));
        // A path the map lacks starts nothing and says so.
        let mut f: EntryFuture = {
            let cx = cx.clone();
            Box::pin(async move {
                let e = play_at(&cx, &units, "sound/missing").await.unwrap_err();
                assert!(e.to_string().contains("sound/missing"), "{e}");
                Ok(entry::Ret::new(0, 0))
            })
        };
        assert!(matches!(entry::poll(&mut f, &cx), Step::Done(Ok(_))));
        set_resolver(None);
    }

    /// A script plays an announcer line by `announcer/<clip>`: one of ours
    /// needs no sound tag of the map, an unknown one is refused by name,
    /// and the start gate's census is not asked about it.
    #[test]
    fn an_announcer_line_is_ours_and_an_unknown_one_is_refused_by_name() {
        let _g = serial();
        set_resolver(None);
        let src = |line: &str| {
            ::rules::compile(&format!(
                "rules \"A\" 1\ntype slayer\nrequires audio.play\n\
                 on kill(victim, killer) {{ sound_to_all(\"{line}\") }}\n"
            ))
            .unwrap()
        };
        assert_eq!(refusal(&src("announcer/attack")), None);
        let why = refusal(&src("announcer/no-such-line")).unwrap();
        assert!(why.contains("announcer/no-such-line"), "{why}");
        let census = ::rules::labels::Census {
            sounds: Some(Default::default()),
            ..Default::default()
        };
        assert!(::rules::labels::check(&src("announcer/attack"), &census, &[0, 1]).is_empty());
        assert!(!::rules::labels::check(&src("sound/stinger"), &census, &[0, 1]).is_empty());
    }

    /// The line joins the announcer's queue for a machine whose own player
    /// is in the audience, and for no other; an unknown clip is an error.
    #[test]
    fn an_announcer_line_queues_for_the_audience_only() {
        let _g = serial();
        crate::medal_announcer::take_waiting();
        let team = cue::player(0) | cue::player(2);
        say([0u8], team, "announcer/attack").unwrap();
        say([1u8], team, "announcer/attack").unwrap();
        say([2u8, 3], team, "announcer/defend").unwrap();
        assert_eq!(crate::medal_announcer::take_waiting(), ["attack", "defend"]);
        let e = say([0u8], team, "announcer/nope").unwrap_err();
        assert!(e.to_string().contains("announcer/nope"), "{e}");
        assert!(crate::medal_announcer::take_waiting().is_empty());
    }
}
