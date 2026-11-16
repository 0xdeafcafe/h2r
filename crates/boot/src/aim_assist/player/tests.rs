//! Synthetic memory only: every value here is made up for the test.

use super::super::tests::{Fake, f32_at, set_f32};
use super::*;
use crate::aim_assist::{SETTING_AUTO, SETTING_STRONG, level_now};

const CAMPAIGN: u32 = 1;
const PAD: u32 = 1;
const REMOTE: u32 = u32::MAX;

const MATG: u32 = 0x30_0000;
const CONTROLS: u32 = 0x31_0000;
const FRICTION: f32 = 0.6;
const ADHESION: f32 = 0.9;
/// Where `0x1a50a0` writes its result in these tests.
const OUT: u32 = 0x50_0000;
const OUT2: u32 = 0x50_1000;

/// A made-up result of `0x1a50a0`: autoaim 0.05 rad / 20 wu, magnetism
/// 0.1 rad / 15 wu, bend 0.06, flags 0x20, a second weapon 0.04 / 10 wu,
/// class range 20.
const PARAMS: [(u32, f32); 8] = [
    (params::AUTOAIM_ANGLE, 0.05),
    (params::AUTOAIM_RANGE, 20.0),
    (params::MAGNETISM_ANGLE, 0.1),
    (params::MAGNETISM_RANGE, 15.0),
    (params::BEND_ANGLE, 0.06),
    (params::SECOND_AUTOAIM_ANGLE, 0.04),
    (params::SECOND_AUTOAIM_RANGE, 10.0),
    (params::CLASS_RANGE, 20.0),
];

/// Guest memory with the globals tag (one player-control element) and a
/// fresh `0x1a50a0` result at `out`.
fn world() -> Fake {
    let mut m = Fake::default();
    m.set_u32(GLOBALS_TAG, MATG);
    m.set_u32(MATG + MATG_PLAYER_CONTROL, 1);
    m.set_u32(MATG + MATG_PLAYER_CONTROL + 4, CONTROLS);
    set_f32(&mut m, CONTROLS + MAGNETISM_FRICTION, FRICTION);
    set_f32(&mut m, CONTROLS + MAGNETISM_ADHESION, ADHESION);
    set_f32(&mut m, CONTROLS + 8, 0.5);
    built(&mut m, OUT);
    built(&mut m, OUT2);
    m
}

/// What `0x1a50a0` writes, from the (made-up) tags.
fn built(m: &mut Fake, out: u32) {
    for (off, v) in PARAMS {
        set_f32(m, out + off, v);
    }
    m.set_u32(out + params::FLAGS, 0x20);
}

fn result(m: &Fake, out: u32) -> Vec<u8> {
    let mut b = vec![0; params::SIZE as usize];
    assert!(m.rd(out, &mut b));
    b
}

#[test]
fn only_the_touch_player_in_multiplayer_gets_a_level() {
    let mp = Some(MODE_MULTIPLAYER);
    // The touch player: Auto is Strong, the others as chosen.
    assert_eq!(
        player_level(SETTING_AUTO, mp, TOUCH_CONTROLLER, true),
        Level::Strong
    );
    assert_eq!(
        player_level(SETTING_STRONG, mp, TOUCH_CONTROLLER, true),
        Level::Strong
    );
    assert_eq!(
        player_level(SETTING_VERY_STRONG, mp, TOUCH_CONTROLLER, true),
        Level::VeryStrong
    );
    assert_eq!(
        player_level(SETTING_DEFAULT, mp, TOUCH_CONTROLLER, true),
        Level::Default
    );
    // Touch controls hidden: player one is on a pad or keyboard.
    for s in [SETTING_AUTO, SETTING_STRONG, SETTING_VERY_STRONG] {
        assert_eq!(player_level(s, mp, TOUCH_CONTROLLER, false), Level::Default);
        // Another local pad, and a remote or unbound player.
        assert_eq!(player_level(s, mp, PAD, true), Level::Default);
        assert_eq!(player_level(s, mp, REMOTE, true), Level::Default);
        // Campaign, co-op, the menus, or no active game: the tag path's.
        for mode in [Some(CAMPAIGN), Some(3), None] {
            assert_eq!(
                player_level(s, mode, TOUCH_CONTROLLER, true),
                Level::Default
            );
        }
    }
}

#[test]
fn the_two_paths_never_stack() {
    // In multiplayer the tags stay retail; outside it nobody gets a
    // per-player level, so at most one path is ever away from retail.
    // (Whatever `input::touch::controls_shown` says: other tests set it.)
    for s in [
        SETTING_AUTO,
        SETTING_DEFAULT,
        SETTING_STRONG,
        SETTING_VERY_STRONG,
    ] {
        for touch in [false, true] {
            for mode in [Some(CAMPAIGN), Some(MODE_MULTIPLAYER), Some(3), None] {
                let tags = level_now(s, mode);
                for c in [TOUCH_CONTROLLER, PAD, REMOTE] {
                    let per = player_level(s, mode, c, touch);
                    assert!(
                        tags == Level::Default || per == Level::Default,
                        "setting {s} mode {mode:?} controller {c} touch {touch}"
                    );
                }
            }
        }
    }
}

#[test]
fn scaling_the_result_matches_the_tag_factors() {
    let mut m = world();
    let f = Level::Strong.factors();
    let n = scale_params(&mut m, OUT, f);
    assert_eq!(f32_at(&m, OUT + params::AUTOAIM_ANGLE), 0.05 * 1.5);
    assert_eq!(f32_at(&m, OUT + params::AUTOAIM_RANGE), 20.0 * 1.25);
    assert_eq!(f32_at(&m, OUT + params::MAGNETISM_ANGLE), 0.1 * 1.5);
    assert_eq!(f32_at(&m, OUT + params::MAGNETISM_RANGE), 15.0 * 1.25);
    assert_eq!(f32_at(&m, OUT + params::SECOND_AUTOAIM_ANGLE), 0.04 * 1.5);
    assert_eq!(f32_at(&m, OUT + params::SECOND_AUTOAIM_RANGE), 10.0 * 1.25);
    assert_eq!(f32_at(&m, OUT + params::CLASS_RANGE), 20.0 * 1.25);
    // Bend is max(deviation, autoaim): 0.075 now beats 0.06.
    assert_eq!(f32_at(&m, OUT + params::BEND_ANGLE), 0.05 * 1.5);
    assert_eq!(m.u32(OUT + params::FLAGS), Some(0x20));
    assert_eq!(n, 8);

    // Caps, zeros (no magnetism) and a bend already past the autoaim.
    let mut m = world();
    set_f32(&mut m, OUT + params::MAGNETISM_ANGLE, 0.4);
    set_f32(&mut m, OUT + params::MAGNETISM_RANGE, 0.0);
    set_f32(&mut m, OUT + params::BEND_ANGLE, 0.3);
    set_f32(&mut m, OUT + params::CLASS_RANGE, 0.0);
    scale_params(&mut m, OUT, Level::VeryStrong.factors());
    assert_eq!(f32_at(&m, OUT + params::MAGNETISM_ANGLE), MAX_ANGLE);
    assert_eq!(f32_at(&m, OUT + params::MAGNETISM_RANGE), 0.0);
    assert_eq!(f32_at(&m, OUT + params::CLASS_RANGE), 0.0);
    assert_eq!(f32_at(&m, OUT + params::BEND_ANGLE), 0.3);
}

#[test]
fn the_touch_players_update_is_scaled_and_put_back() {
    let pp = PerPlayer::default();
    let mut m = world();
    let before = m.clone();

    assert!(pp.enter(&mut m, Level::Strong));
    assert!(pp.open());
    assert_eq!(f32_at(&m, CONTROLS + MAGNETISM_FRICTION), FRICTION * 1.25);
    // 0.9 * 1.25 is past the engine's own cap of 1.0.
    assert_eq!(f32_at(&m, CONTROLS + MAGNETISM_ADHESION), MAX_STICK);
    // The inconsequential-target scale is not ours.
    assert_eq!(f32_at(&m, CONTROLS + 8), 0.5);
    // The search's `0x1a50a0` call inside the window.
    assert_eq!(pp.params(&mut m, OUT), 8);
    assert_eq!(f32_at(&m, OUT + params::AUTOAIM_ANGLE), 0.05 * 1.5);

    pp.leave(&mut m);
    assert!(!pp.open());
    // Friction and adhesion back bit for bit; only our result buffer moved.
    built(&mut m, OUT);
    assert_eq!(m, before);
}

#[test]
fn everyone_else_sees_retail_bit_for_bit() {
    let pp = PerPlayer::default();
    let mut m = world();
    let before = m.clone();

    // No window: a shot's `0x1a50a0` (weapon fire), any player.
    assert_eq!(pp.params(&mut m, OUT), 0);
    // A pad player's update: Default opens nothing and writes nothing.
    assert!(!pp.enter(&mut m, Level::Default));
    assert_eq!(pp.params(&mut m, OUT), 0);
    pp.leave(&mut m);
    assert_eq!(m, before);

    // The touch player, then a pad player in the same tick: the second
    // update sees retail, and so does a shot after both.
    pp.enter(&mut m, Level::VeryStrong);
    pp.params(&mut m, OUT);
    pp.leave(&mut m);
    assert!(!pp.enter(&mut m, Level::Default));
    assert_eq!(pp.params(&mut m, OUT2), 0);
    pp.leave(&mut m);
    assert_eq!(pp.params(&mut m, OUT2), 0);
    assert_eq!(result(&m, OUT2), result(&before, OUT2));
    assert_eq!(
        m.u32(CONTROLS + MAGNETISM_FRICTION),
        before.u32(CONTROLS + MAGNETISM_FRICTION)
    );
    assert_eq!(
        m.u32(CONTROLS + MAGNETISM_ADHESION),
        before.u32(CONTROLS + MAGNETISM_ADHESION)
    );
}

#[test]
fn a_window_left_open_is_closed_by_the_next() {
    let pp = PerPlayer::default();
    let mut m = world();
    let before = m.clone();
    pp.enter(&mut m, Level::Strong);
    // No `after` (it cannot happen in game; belt and braces).
    pp.enter(&mut m, Level::Strong);
    assert_eq!(f32_at(&m, CONTROLS + MAGNETISM_FRICTION), FRICTION * 1.25);
    pp.leave(&mut m);
    assert_eq!(m, before);
}

#[test]
fn a_reload_inside_the_window_is_left_alone() {
    let pp = PerPlayer::default();
    let mut m = world();
    pp.enter(&mut m, Level::Strong);
    set_f32(&mut m, CONTROLS + MAGNETISM_FRICTION, 0.2);
    pp.leave(&mut m);
    assert_eq!(f32_at(&m, CONTROLS + MAGNETISM_FRICTION), 0.2);
    assert_eq!(f32_at(&m, CONTROLS + MAGNETISM_ADHESION), ADHESION);
}

#[test]
fn odd_globals_are_not_touched() {
    let pp = PerPlayer::default();
    for odd in [
        |m: &mut Fake| {
            m.set_u32(GLOBALS_TAG, 0);
        },
        |m: &mut Fake| {
            m.set_u32(MATG + MATG_PLAYER_CONTROL, 17);
        },
        |m: &mut Fake| {
            m.set_u32(MATG + MATG_PLAYER_CONTROL + 4, 0);
        },
    ] {
        let mut m = world();
        odd(&mut m);
        let before = m.clone();
        pp.enter(&mut m, Level::VeryStrong);
        pp.leave(&mut m);
        assert_eq!(m, before);
    }
}

/// One machine in a multiplayer game: its local players' controllers and
/// whether its touch controls show.
struct Peer {
    mem: Fake,
    pp: PerPlayer,
    local: Vec<u32>,
    touch: bool,
}

impl Peer {
    fn new(local: Vec<u32>, touch: bool) -> Self {
        Peer {
            mem: world(),
            pp: PerPlayer::default(),
            local,
            touch,
        }
    }

    /// One tick: each local player's control update (with the search's
    /// `0x1a50a0` at `OUT`), returning those results; then a shot by every
    /// player in the game (`0x1a50a0` at `OUT2` from the barrel fire),
    /// returning what the simulation saw.
    fn tick(&mut self, setting: i64, players: usize) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
        let mut actions = Vec::new();
        for &c in &self.local {
            let level = player_level(setting, Some(MODE_MULTIPLAYER), c, self.touch);
            built(&mut self.mem, OUT);
            self.pp.enter(&mut self.mem, level);
            self.pp.params(&mut self.mem, OUT);
            actions.push(result(&self.mem, OUT));
            self.pp.leave(&mut self.mem);
        }
        let mut shots = Vec::new();
        for _ in 0..players {
            built(&mut self.mem, OUT2);
            self.pp.params(&mut self.mem, OUT2);
            let mut seen = result(&self.mem, OUT2);
            let mut b = [0; 8];
            assert!(self.mem.rd(CONTROLS, &mut b));
            seen.extend_from_slice(&b);
            shots.push(seen);
        }
        (actions, shots)
    }
}

#[test]
fn two_peers_simulate_the_same_values() {
    // Host: the touch player alone. Guest: two pads, split screen.
    let mut host = Peer::new(vec![TOUCH_CONTROLLER], true);
    let mut guest = Peer::new(vec![TOUCH_CONTROLLER, PAD], false);
    let retail = result(&world(), OUT);
    for setting in [SETTING_AUTO, SETTING_STRONG, SETTING_VERY_STRONG] {
        let (h_actions, h_shots) = host.tick(setting, 3);
        let (g_actions, g_shots) = guest.tick(setting, 3);
        // Everything the simulation reads is the same on both, and retail.
        assert_eq!(h_shots, g_shots, "setting {setting}");
        assert_eq!(result(&host.mem, OUT2), retail);
        assert_eq!(host.mem.u32(CONTROLS), Some(FRICTION.to_bits()));
        // Only the touch player's own action differs from retail.
        assert_ne!(h_actions[0], retail);
        assert_eq!(g_actions, vec![retail.clone(), retail.clone()]);
    }
    // The same inputs on two machines give the same results.
    let mut a = Peer::new(vec![TOUCH_CONTROLLER, PAD], true);
    let mut b = Peer::new(vec![TOUCH_CONTROLLER, PAD], true);
    for setting in [SETTING_AUTO, SETTING_VERY_STRONG, SETTING_DEFAULT] {
        assert_eq!(a.tick(setting, 4), b.tick(setting, 4), "setting {setting}");
        assert_eq!(a.mem, b.mem);
    }
}

#[test]
fn the_points_read_as_the_engine_does() {
    use feature::points::{Loc, Sig};
    assert_eq!(PLAYER_CONTROL_UPDATE.base, 0x0018_5be0);
    assert_eq!(PLAYER_CONTROL_UPDATE.sig.arg("controller"), Some(Loc::Edx));
    assert_eq!(PLAYER_CONTROL_UPDATE.sig.arg("user"), Some(Loc::Stack(0)));
    assert!(matches!(
        PLAYER_CONTROL_UPDATE.sig,
        Sig::Fn { pop: 0x14, .. }
    ));
    assert_eq!(AIM_ASSIST_PARAMS.base, 0x001a_50a0);
    assert_eq!(AIM_ASSIST_PARAMS.sig.arg("out"), Some(Loc::Esi));
    assert!(matches!(AIM_ASSIST_PARAMS.sig, Sig::Fn { pop: 4, .. }));
}
