//! `sound_to*` on the host (`super::super::world::audio`): the VM's call
//! becomes a held sound until the authority answer is in, and a host with
//! no lookup of a sound tag refuses the script by the sound's path.

use super::*;
use crate::rules::world::audio;

const STINGER: &str = "sound/stinger";
const SRC: &str = "rules \"Stinger\" 1\ntype slayer\nrequires audio.play\n\
                   on kill(victim, killer) { sound_to_all(\"sound/stinger\") }\n";

#[test]
fn a_host_with_no_sound_lookup_names_the_sound_it_cannot_find() {
    let _g = audio::SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    audio::set_resolver(None);
    let ir = ::rules::compile(SRC).unwrap();
    let why = audio::refusal(&ir).unwrap();
    assert!(why.contains(STINGER), "{why}");
}

#[test]
fn a_sound_is_held_until_the_authority_answers() {
    let _g = audio::SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    audio::set_resolver(Some(Box::new(|path: &str| (path == STINGER).then_some(0xe123_0045))));
    let ir = ::rules::compile(SRC).unwrap();
    assert_eq!(host_refusal(&ir), None);
    let store = Local::default();
    let blob = ir.encode();
    let s = shared(&store, Box::new(move |_: [u8; 8]| Ok::<_, String>(blob.clone())));
    let (m, _) = guest(block(prefix(&ir.id()), 0), SLAYER);
    let line = s.lock().start(&m, false);
    assert!(s.lock().state().running, "{line}");
    s.lock().kill(&m, 0, Some(1));
    // The boundary makes the VM's queued call; with no authority answer yet
    // the sound waits and is neither a play nor a cue.
    let work = s.lock().pre_tick(&m).expect("the authority check");
    assert!(work.ask_authority);
    assert!(work.calls.is_empty());
    assert_eq!(
        s.lock().running().unwrap().pending(),
        [Call::SoundTo {
            to: ::rules::show::To::Everyone,
            path: STINGER.into(),
        }]
    );
}
