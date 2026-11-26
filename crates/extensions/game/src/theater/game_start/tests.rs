use super::*;
use crate::level_boot::{MODE_CAMPAIGN, MODE_MAINMENU, MODE_MULTIPLAYER, OPT_MODE};
use crate::theater::actions::note_first_update;
use crate::theater::tests::{arena, host_facts, options};
use crate::theater::{Facts, Mode, Session};
use runtime::guest::Permissions;
use std::{fs, path::PathBuf, sync::Arc};

pub(in crate::theater) const GLOBALS: u32 = 0xc0_0000;
pub(in crate::theater) const RANDOM: u32 = 0xc1_0000;

pub(in crate::theater) struct Temp(pub(in crate::theater) PathBuf);
impl Temp {
    pub(in crate::theater) fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("film-start-{}", live_proto::sign::random_id()));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The options block a synthetic game starts from, distinct for each `salt`.
pub(in crate::theater) fn block(mode: u32, salt: u8) -> Vec<u8> {
    let mut block: Vec<u8> = (0..OPTIONS_BYTES).map(|i| (i * 31 + 7) as u8 ^ salt).collect();
    block[OPT_MODE as usize..OPT_MODE as usize + 4].copy_from_slice(&mode.to_le_bytes());
    block
}

/// A guest with the engine's arena, a game's globals holding `options`
/// active, and the two shared random words.
pub(in crate::theater) fn game(options: &[u8], words: [u32; 2]) -> runtime::Guest {
    let mut g = arena();
    g.map_zeroed(crate::sim_glue::RANDOM_SEED_PTR & !4095, 4096, Permissions::RW)
        .unwrap();
    g.map_zeroed(GLOBALS, 0x2000, Permissions::RW).unwrap();
    g.map_zeroed(RANDOM, 0x1000, Permissions::RW).unwrap();
    g.set_u32(crate::telemetry::layout::GAME_GLOBALS_PTR, GLOBALS).unwrap();
    g.set_u32(crate::sim_glue::RANDOM_SEED_PTR, RANDOM).unwrap();
    g.set_u32(RANDOM, words[0]).unwrap();
    g.set_u32(RANDOM + 4, words[1]).unwrap();
    g.write_bytes(GLOBALS + crate::telemetry::layout::OPTIONS, options, 1).unwrap();
    // Activation sets the globals' "map active" byte.
    g.set_u8(GLOBALS + 0x1120, 1).unwrap();
    g
}

fn recorder(dir: &Temp, name: &str) -> Arc<Session> {
    let facts = Facts { light: false, player_features: true, ..host_facts() };
    Session::open(Some(dir.0.join(name)), None, facts, b"xbe", &options(&dir.0))
        .unwrap()
        .unwrap()
}

#[test]
fn the_active_options_and_both_random_words_are_copied_whole() {
    for mode in [MODE_CAMPAIGN, MODE_MULTIPLAYER] {
        let options = block(mode, 0);
        let g = game(&options, [0xdead_beef, 0x0bad_f00d]);
        assert_eq!(copy_options(&g), Some(options.clone()));
        assert_eq!(random_words(&g).unwrap(), [0xdead_beef, 0x0bad_f00d]);
        let dir = Temp::new();
        let rec = recorder(&dir, "a.h2film");
        let header = rec.lock().header.clone();
        let start = first_update(&g, options.clone(), &header, None, 91).unwrap();
        assert_eq!(start.options, options);
        assert_eq!(start.random, [0xdead_beef, 0x0bad_f00d]);
        assert_eq!(start.game.first_update, 91);
        assert_eq!(
            Some(start.start_hash),
            lockstep::hash_gameplay(&g, lockstep::SimRegion::HALO2)
        );
        assert_eq!(start.stamp.xbe_sha256, header.xbe_sha256);
        assert_eq!(start.stamp.simulation_abi, header.runtime_stamp);
        assert_eq!(start.settings.switches, header.features);
    }
}

#[test]
fn the_main_menu_and_a_block_of_another_size_have_no_start() {
    let menu = game(&block(MODE_MAINMENU, 0), [1, 2]);
    assert!(copy_options(&menu).is_none());
    let g = game(&block(MODE_MULTIPLAYER, 0), [1, 2]);
    let dir = Temp::new();
    let header = recorder(&dir, "a.h2film").lock().header.clone();
    assert!(first_update(&g, vec![0; OPTIONS_BYTES - 1], &header, None, 1).is_err());
    // No random words set is a named refusal, not a zero.
    let mut bare = arena();
    bare.map_zeroed(crate::sim_glue::RANDOM_SEED_PTR & !4095, 4096, Permissions::RW)
        .unwrap();
    assert!(random_words(&bare).is_err());
}

#[test]
fn a_reactivation_of_the_game_in_play_is_not_a_new_game() {
    let mut starts = Starts::default();
    let first = block(MODE_CAMPAIGN, 1);
    starts.activated(first.clone());
    assert!(starts.waiting());
    assert_eq!(starts.take_options(), Some(first.clone()));
    assert!(!starts.waiting());
    // A campaign revert activates the same map with the same options.
    starts.activated(first.clone());
    assert!(!starts.waiting());
    // Another game's options are a new game; so is the same one after the
    // game was left.
    starts.activated(block(MODE_CAMPAIGN, 2));
    assert!(starts.waiting());
    starts.next_game();
    starts.activated(first);
    assert!(starts.waiting());
}

/// One game of a synthetic session: activation, the first update, the
/// observer's description, then `ticks` ticks. Returns the film tick of the
/// first update.
fn play(rec: &Session, menus: &runtime::Guest, options: &[u8], words: [u32; 2], ticks: u64) -> u64 {
    let g = game(options, words);
    rec.game_activated(&g);
    let first = {
        let mut s = rec.lock();
        let Mode::Record(r) = &s.mode else { panic!("a recording") };
        let first = r.film.as_ref().unwrap().lock().unwrap().ticks() + 1;
        note_first_update(rec.video_menu.get(), &mut s, &g);
        let Mode::Record(r) = &mut s.mode else { panic!("a recording") };
        r.meta.start_for_test(first, "zanzibar", film::blam::Mode::Custom);
        first
    };
    // The tick's observer pushes the film tick, and the start with it.
    for _ in 0..ticks {
        rec.checkpoint(menus);
    }
    first
}

fn end_game(rec: &Session, start: u64, slices: u64) {
    {
        let mut s = rec.lock();
        let Mode::Record(r) = &mut s.mode else { panic!("a recording") };
        r.meta.start_for_test(start, "zanzibar", film::blam::Mode::Custom);
        r.game_started_ms = Some(1_795_000_000_000 + start);
        r.match_over = true;
    }
    rec.finish(slices, false).unwrap();
}

fn starts_of(path: &std::path::Path) -> Vec<film::start::GameStart> {
    let mut reader =
        film::file::FilmReader::open(fs::File::open(film::file::beside(path)).unwrap()).unwrap();
    let blocks = reader.game_starts.clone();
    blocks.iter().map(|b| reader.game_start_at(b).unwrap()).collect()
}

#[test]
fn a_session_of_two_games_writes_a_game_start_for_each_at_its_first_update() {
    let dir = Temp::new();
    let rec = recorder(&dir, "auto-20261118-110640-000.h2film");
    let menus = arena();
    for _ in 0..10 {
        rec.checkpoint(&menus);
    }
    let first_options = block(MODE_MULTIPLAYER, 1);
    let one = play(&rec, &menus, &first_options, [11, 12], 30);
    assert_eq!(one, 11, "the tick after the menu's ten");
    end_game(&rec, one, 80);
    for _ in 0..5 {
        rec.checkpoint(&menus);
    }
    let second_options = block(MODE_CAMPAIGN, 2);
    let two = play(&rec, &menus, &second_options, [21, 22], 20);
    assert_eq!(two, 46);
    end_game(&rec, two, 200);
    rec.finish(210, true).unwrap();

    let films: Vec<_> = fs::read_dir(&dir.0)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| super::super::autorecord::is_auto(p) && p.with_extension("h2f").exists())
        .collect();
    assert_eq!(films.len(), 2, "one film for each game");
    // The first game's film holds its own start; the second's, cut from the
    // same file as it stood, holds the first's and then its own.
    let mut all: Vec<_> = films.iter().map(|p| starts_of(p)).collect();
    all.sort_by_key(|starts| starts.len());
    let (a, b) = (&all[0], &all[1]);
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].game.first_update, 11);
    assert_eq!(a[0].options, first_options);
    assert_eq!(a[0].random, [11, 12]);
    assert_eq!(b.len(), 2);
    assert_eq!(b[0].game.first_update, 11);
    assert_eq!(b[1].game.first_update, 46);
    assert_eq!(b[1].options, second_options);
    assert_eq!(b[1].random, [21, 22]);
    // Each carries the map the observer named and the recording's stamp.
    for start in a.iter().chain(b) {
        assert_eq!(start.game.descriptor.map.name, "zanzibar");
        assert_eq!(start.stamp.simulation_abi, "h2-sim/2");
        assert!(start.game.started_ms > 0);
    }
}

#[test]
fn a_game_whose_first_update_cannot_be_read_still_records_its_ticks() {
    let dir = Temp::new();
    let rec = recorder(&dir, "auto-20261118-110641-000.h2film");
    // No random words: the start cannot be taken, and the film still is.
    let mut g = arena();
    g.map_zeroed(GLOBALS, 0x2000, Permissions::RW).unwrap();
    g.set_u32(crate::telemetry::layout::GAME_GLOBALS_PTR, GLOBALS).unwrap();
    g.write_bytes(GLOBALS + crate::telemetry::layout::OPTIONS, &block(MODE_MULTIPLAYER, 3), 1)
        .unwrap();
    g.set_u8(GLOBALS + 0x1120, 1).unwrap();
    rec.game_activated(&g);
    {
        let mut s = rec.lock();
        note_first_update(None, &mut s, &g);
        let Mode::Record(r) = &mut s.mode else { panic!("a recording") };
        assert!(!r.starts.waiting() && r.starts.release(&mut r.meta).unwrap().is_none());
    }
    for _ in 0..3 {
        rec.checkpoint(&arena());
    }
    assert!(rec.lock().failure.is_none());
    rec.finish(10, true).unwrap();
}
