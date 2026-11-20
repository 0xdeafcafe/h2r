//! Version 1 of the sim report's post-tick gameplay digest. All inputs come
//! from the existing observation, except for the engine's shared RNG word.
use crate::telemetry::Snapshot;

struct Digest(u64);

impl Digest {
    // FNV-1a over a sequence of little-endian 64-bit words. Avoid building
    // or serializing a tick-sized buffer on the game thread.
    fn word(&mut self, word: u64) {
        for byte in word.to_le_bytes() {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }

    fn optional<T>(&mut self, value: Option<T>, write: impl FnOnce(&mut Self, T)) {
        self.word(u64::from(value.is_some()));
        if let Some(value) = value {
            write(self, value);
        }
    }

    fn vector(&mut self, v: [f64; 3]) {
        for x in v {
            self.word(x.to_bits());
        }
    }
}

pub(super) fn gameplay(s: &Snapshot, rng: Option<u32>) -> u64 {
    let mut h = Digest(0xcbf29ce484222325);
    h.word(s.tick);
    h.optional(rng, |h, r| h.word(r.into()));
    h.optional(s.bsp, |h, b| h.word(b.into()));
    h.word(u64::from(s.has_stats));
    h.word(u64::from(s.cinematic));
    // The reader enumerates guest player slots, then numeric team indices.
    h.word(s.players.len() as u64);
    for (slot, p) in s.players.iter().enumerate() {
        h.word(p.player.index.into());
        h.word(p.datum.into());
        h.word(p.unit.into());
        h.word(p.player.team as u64);
        h.word(p.stats.score as u64);
        for count in [
            p.stats.kills,
            p.stats.deaths,
            p.stats.assists,
            p.stats.suicides,
        ] {
            h.word(count.into());
        }
        h.optional(s.objects.get(slot).and_then(Option::as_ref), |h, o| {
            h.vector(o.pos);
            h.vector(o.forward);
            h.vector(o.up);
            h.optional(o.aim, Digest::vector);
            h.optional(o.velocity, Digest::vector);
            h.vector(o.motion.velocity);
            h.optional(o.vitality, |h, (body, shield)| {
                h.word(body.to_bits());
                h.word(shield.to_bits());
            });
            for v in [
                o.vehicle,
                o.mount,
                o.seat.map(u32::from),
                o.definition,
                o.vehicle_definition,
                o.weapon.as_ref().map(|w| w.datum),
                o.motion.grenades,
                o.motion.unit_flags,
                o.motion.biped_flags.map(u32::from),
            ] {
                h.optional(v, |h, v| h.word(v.into()));
            }
            h.optional(o.motion.grenade, |h, v| h.word(v as u64));
            h.optional(o.motion.weapons, |h, slots| {
                for w in slots {
                    h.word(w.into());
                }
            });
            h.optional(o.motion.magazine, |h, (reserve, loaded)| {
                h.word(reserve as u64);
                h.word(loaded as u64);
            });
            h.optional(o.motion.crouch, |h, v| h.word(v.to_bits()));
            h.optional(o.motion.physics_mode, |h, v| h.word(v.into()));
            h.optional(o.motion.animation, |h, a| {
                h.word(a.graph.into());
                h.word(a.inherited as u64);
                h.word(a.index as u64);
                h.word(a.frame.to_bits().into());
            });
        });
    }
    h.word(s.teams.len() as u64);
    for t in &s.teams {
        h.word(t.team as u64);
        h.word(t.score as u64);
    }
    h.0
}

/// The same state as [`gameplay`], named: one `(component, value)` per
/// player pose, velocity, facing, health, weapon state and so on, plus the
/// RNG seed, tick counter and team scores, for `h2 sim diff`. Values are
/// plain decimal text (floats print so they read back exactly) so a report
/// shows both sides. The checkpoint arena's blocks are added by the caller.
pub(super) fn components(s: &Snapshot, rng: Option<u32>) -> Vec<(String, String)> {
    use std::fmt::Write as _;

    fn v3(v: [f64; 3]) -> String {
        format!("{},{},{}", v[0], v[1], v[2])
    }
    fn opt<T>(v: Option<T>, f: impl FnOnce(T) -> String) -> String {
        v.map_or_else(|| "none".into(), f)
    }
    fn hex(v: u32) -> String {
        format!("{v:#x}")
    }

    let mut out: Vec<(String, String)> = Vec::with_capacity(16 + s.players.len() * 14);
    let mut add = |name: String, value: String| out.push((name, value));
    add("time.tick".into(), s.tick.to_string());
    add("rng.seed".into(), opt(rng, hex));
    add("world.bsp".into(), opt(s.bsp, |b| b.to_string()));
    add(
        "world.flags".into(),
        format!("stats={} cinematic={}", s.has_stats, s.cinematic),
    );
    for t in &s.teams {
        add(format!("team.{:02}.score", t.team), t.score.to_string());
    }
    for (slot, p) in s.players.iter().enumerate() {
        let n = format!("player.{slot:02}");
        add(
            format!("{n}.id"),
            format!(
                "index={} datum={} unit={} team={}",
                p.player.index,
                hex(p.datum),
                hex(p.unit),
                p.player.team
            ),
        );
        let st = &p.stats;
        add(
            format!("{n}.stats"),
            format!(
                "score={} kills={} deaths={} assists={} suicides={}",
                st.score, st.kills, st.deaths, st.assists, st.suicides
            ),
        );
        let Some(Some(o)) = s.objects.get(slot) else {
            add(format!("{n}.object"), "none".into());
            continue;
        };
        let m = &o.motion;
        add(format!("{n}.pos"), v3(o.pos));
        add(format!("{n}.velocity"), opt(o.velocity, v3));
        add(format!("{n}.unit_velocity"), v3(m.velocity));
        add(format!("{n}.facing"), format!("fwd={} up={}", v3(o.forward), v3(o.up)));
        add(format!("{n}.aim"), opt(o.aim, v3));
        add(
            format!("{n}.health"),
            opt(o.vitality, |(body, shield)| format!("body={body} shield={shield}")),
        );
        let mut w = String::new();
        let _ = write!(w, "held={}", opt(o.weapon.as_ref(), |w| hex(w.datum)));
        let _ = write!(w, " slots={}", opt(m.weapons, |s| s.map(hex).join(",")));
        let _ = write!(
            w,
            " magazine={}",
            opt(m.magazine, |(reserve, loaded)| format!("{reserve}/{loaded}"))
        );
        let _ = write!(
            w,
            " grenade={} grenades={}",
            opt(m.grenade, |g| g.to_string()),
            opt(m.grenades, |g| g.to_string())
        );
        add(format!("{n}.weapon"), w);
        add(
            format!("{n}.vehicle"),
            format!(
                "vehicle={} mount={} seat={} def={} vdef={}",
                opt(o.vehicle, hex),
                opt(o.mount, hex),
                opt(o.seat, |s| s.to_string()),
                opt(o.definition, hex),
                opt(o.vehicle_definition, hex)
            ),
        );
        add(
            format!("{n}.anim"),
            opt(m.animation, |a| {
                format!(
                    "graph={} inherited={} index={} frame={}",
                    hex(a.graph),
                    a.inherited,
                    a.index,
                    a.frame
                )
            }),
        );
        add(
            format!("{n}.motion"),
            format!(
                "crouch={} physics={} unit_flags={} biped_flags={}",
                opt(m.crouch, |c| c.to_string()),
                opt(m.physics_mode, |p| p.to_string()),
                opt(m.unit_flags, hex),
                opt(m.biped_flags, |f| hex(f.into()))
            ),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_render_updates_change_the_tick_digest() {
        use render_bridge::pipeline_render::{RANDOM_A, RANDOM_C};
        use runtime::{Guest, guest::Permissions};

        const SEED: u32 = 0x1000;
        const PARTICLE_FLAGS: u32 = 0x1004;
        const ANIMATION_STAMP: u32 = 0x1008;
        const FRAME: u32 = 10;
        let mut live = Guest::new();
        live.map_zeroed(SEED, 0x1000, Permissions::RW).unwrap();
        live.set_u32(SEED, 7).unwrap();
        live.set_u8(PARTICLE_FLAGS, 4).unwrap();
        live.set_u32(ANIMATION_STAMP, 1).unwrap();

        let tick = |memory: &Guest| {
            // Synthetic branches consume an extra sample when they see a
            // stale particle flag or cache stamp. The digest is the actual
            // state_hashes_v1 gameplay digest.
            let particle_step = memory.u8(PARTICLE_FLAGS).unwrap() & 4 != 0;
            let animation_request = FRAME - memory.u32(ANIMATION_STAMP).unwrap() > 2;
            let mut seed = memory.u32(SEED).unwrap();
            for _ in 0..1 + u32::from(particle_step) + u32::from(animation_request) {
                seed = seed.wrapping_mul(RANDOM_A).wrapping_add(RANDOM_C);
            }
            let snapshot = Snapshot {
                tick: 11,
                ..Default::default()
            };
            gameplay(&snapshot, Some(seed))
        };

        let (mut view, _) = live.cow_snapshot();
        view.set_u8(PARTICLE_FLAGS, 0).unwrap();
        view.set_u32(ANIMATION_STAMP, FRAME).unwrap();
        let expected = tick(&view);

        // Publishing either mutation alone leaves a tick-visible dependency.
        assert_ne!(tick(&live), expected);
        live.set_u8(PARTICLE_FLAGS, 0).unwrap();
        assert_ne!(tick(&live), expected);
        live.set_u8(PARTICLE_FLAGS, 4).unwrap();
        live.set_u32(ANIMATION_STAMP, FRAME).unwrap();
        let tick_before_hand_back = tick(&live);
        assert_ne!(tick_before_hand_back, expected);
        live.set_u8(PARTICLE_FLAGS, 0).unwrap();
        assert_eq!(tick(&live), expected);
    }

    #[test]
    fn replaying_a_render_rng_step_after_the_tick_cannot_repair_its_digest() {
        use render_bridge::pipeline_render::{RANDOM_A, RANDOM_C};

        let step = |seed: u32| seed.wrapping_mul(RANDOM_A).wrapping_add(RANDOM_C);
        let initial = 7;
        let serial_post_tick = step(step(initial));
        let early_post_tick = step(initial);
        let after_join = step(early_post_tick);
        let snapshot = Snapshot::default();

        assert_eq!(after_join, serial_post_tick);
        assert_ne!(
            gameplay(&snapshot, Some(early_post_tick)),
            gameplay(&snapshot, Some(serial_post_tick))
        );
    }

    #[test]
    fn components_name_rng_tick_and_players() {
        let snapshot = Snapshot {
            tick: 9,
            ..Default::default()
        };
        let c = components(&snapshot, Some(0x1234));
        assert!(c.contains(&("time.tick".into(), "9".into())));
        assert!(c.contains(&("rng.seed".into(), "0x1234".into())));
        let again = components(&snapshot, Some(0x1235));
        assert_ne!(c, again);
    }

    #[test]
    fn empty_tick_v1_has_a_stable_wire_digest() {
        assert_eq!(gameplay(&Snapshot::default(), None), 0x8ac123d6f7dce585);
    }
}

#[cfg(test)]
#[path = "state_hash_pipeline_test.rs"]
mod pipeline;
