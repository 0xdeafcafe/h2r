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
//! tag paths. [`set_resolver`] gives a host its lookup of a sound tag's datum
//! by path; a host with none refuses a script that plays a sound
//! ([`refusal`]), by the sound's path.

use std::sync::{Mutex, MutexGuard};

use ::rules::caps::id;
use ::rules::ir::{Ir, Op, TagGroup};
use ::rules::show::{self as cue, Audience, Cue, SoundCue, To};
use ::rules::vm::{Arg, Call as VmCall};
use agent::sim::calls::addr;
use engine::GuestMemory;
use engine::entry::{Cx, EntryError, GuestCall};

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

/// Whether this host can find a sound tag by its path.
pub fn can_resolve() -> bool {
    resolver().is_some()
}

fn resolve(path: &str) -> Option<u32> {
    resolver().as_ref().and_then(|lookup| lookup(path))
}

/// Why a host that `resolves` or not cannot run `ir`, by the sound's path:
/// the script plays a sound and the host has no lookup of a sound tag.
pub fn refusal(ir: &Ir, resolves: bool) -> Option<String> {
    if resolves {
        return None;
    }
    let plays = ir.handlers.iter().flat_map(|h| &h.code).any(|op| {
        matches!(op, Op::Call { entry, .. }
            if matches!(*entry, id::SOUND_TO | id::SOUND_TO_TEAM | id::SOUND_TO_ALL))
    });
    if !plays {
        return None;
    }
    let sound = ir.tags.iter().find(|t| t.group == TagGroup::Sound)?;
    Some(format!(
        "the script plays the sound {}, and this host cannot find a sound tag by its path yet \
         (docs/game/rules-world-api.md, \"Audio\")",
        sound.path
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
    let sound = resolve(path).ok_or_else(|| anyhow::anyhow!("no sound tag {path} found on this machine"))?;
    let mut failed = None;
    for unit in cx.read(|m| units(m, audience)) {
        if let Err(e) = start(cx, unit, sound).await {
            failed.get_or_insert(e);
        }
    }
    failed.map_or(Ok(()), Err)
}

fn guest(e: impl Into<EntryError>) -> anyhow::Error {
    anyhow::anyhow!("{}", e.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::rules::caps;

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
        let ir = ir();
        let why = refusal(&ir, false).unwrap();
        assert!(why.contains(STINGER), "{why}");
        assert_eq!(refusal(&ir, true), None);
        // A script that plays no sound is no concern of the lookup.
        let quiet = ::rules::compile("rules \"Q\" 1\ntype slayer\nglobal var n: int = 0\non tick { n = n + 1 }\n");
        assert_eq!(refusal(&quiet.unwrap(), false), None);
    }
}
