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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_tick_v1_has_a_stable_wire_digest() {
        assert_eq!(gameplay(&Snapshot::default(), None), 0x8ac123d6f7dce585);
    }
}
