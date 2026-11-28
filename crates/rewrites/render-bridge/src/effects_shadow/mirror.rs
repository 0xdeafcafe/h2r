//! R2's particle mirror (docs/graphics/retire-d3d-r2.md, "R2's particle
//! mirror"): the particles the answer takes at `0x00224550`, held as GPU
//! particle system births ([`native_render::particle_mirror`]) and drawn
//! together by the host ([`MirrorHost`], `ParticleDraw` in the game) when
//! the particle draw `0x002243c0` returns.
//!
//! Order is the guest's: a particle the mirror does not take, or one of
//! another [`Key`], first draws the held batch, then goes on as before
//! (the answer's direct draw). A batch the host then refuses has no guest
//! draw left (the callbacks returned): it is dropped by name,
//! `layer_plan_15_effects_shadow_mirror_dropped_<cause>`. What is not
//! taken is
//! `scene_kept_15_particle_mirror_<cause>`.

use std::cell::RefCell;
use std::sync::Arc;
#[cfg(not(test))]
use std::sync::Mutex;

use engine::function_eval::constant::BYTE;
use engine::memory::GuestMemory;
use engine::particle_emission::{Prediction, constant::TINT};
use native_render::particle_mirror::{self as nm, Key};
use native_render::particle_system::Spawn;

use super::count;

/// One system's particles of one particle draw, in the guest's order.
#[derive(Clone, Debug, PartialEq)]
pub struct Batch {
    pub key: Key,
    pub spawns: Vec<Spawn>,
}

/// Draws mirrored batches (the game: a GPU particle system a `prt3` tag,
/// its `Definition` from `particle_definition::definition`).
pub trait MirrorHost: Send + Sync {
    /// Whether particles of `key` can be drawn: the definition built from
    /// the tag and its bitmap resident. `Err` names why not.
    fn accepts(&self, m: &dyn GuestMemory, key: &Key) -> Result<(), &'static str>;
    /// Draws `batch` where the particle draw would have (through the
    /// device, which drains the guest's push buffer: `m` is mutable).
    /// `Err` names why nothing was drawn.
    fn draw(&self, m: &mut dyn GuestMemory, batch: &Batch) -> Result<(), &'static str>;
}

#[cfg(not(test))]
static HOST: Mutex<Option<Arc<dyn MirrorHost>>> = Mutex::new(None);

/// Installs the mirror's host (boot).
#[cfg(not(test))]
pub fn set_host(host: Arc<dyn MirrorHost>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

#[cfg(not(test))]
fn host() -> Option<Arc<dyn MirrorHost>> {
    HOST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

// Tests run in parallel: each thread keeps its own host.
#[cfg(test)]
thread_local! {
    static HOST: RefCell<Option<Arc<dyn MirrorHost>>> = const { RefCell::new(None) };
}

#[cfg(test)]
pub fn set_host(host: Arc<dyn MirrorHost>) {
    HOST.with(|h| *h.borrow_mut() = Some(host));
}

#[cfg(test)]
fn host() -> Option<Arc<dyn MirrorHost>> {
    HOST.with(|h| h.borrow().clone())
}

thread_local! {
    static HELD: RefCell<Option<Batch>> = const { RefCell::new(None) };
}

fn kept(cause: &str) {
    crate::native_layers::count_kept_as("", 15, "particle", &format!("mirror_{cause}"), 1);
}

/// A word as the callback leaves it: the prediction's write over memory.
fn word(m: &dyn GuestMemory, p: &Prediction, at: u32) -> Option<u32> {
    let mut b = [0u8; 4];
    for (i, byte) in b.iter_mut().enumerate() {
        let a = at.wrapping_add(i as u32);
        *byte = match p.writes.get(&a) {
            Some(&v) => v,
            None => m.u8(a).ok()?,
        };
    }
    Some(u32::from_le_bytes(b))
}

/// The key and birth of the particle whose callback was entered with
/// `esp`, once `p` is made.
fn particle(m: &dyn GuestMemory, esp: u32, p: &Prediction) -> Result<(Key, Spawn), &'static str> {
    let w = |at: u32| word(m, p, at).ok_or("unreadable");
    let arg = |i: u32| w(esp.wrapping_add(4 + 4 * i));
    let (particle, state, position, size, tint, draw) = (arg(0)?, arg(1)?, arg(2)?, arg(4)?, arg(5)?, arg(6)?);
    let definition = w(draw)?;
    let flags = w(definition)?;
    let class = w(definition + 4)? as u16;
    nm::check(flags, class, f32::from_bits(w(BYTE)?), f32::from_bits(w(TINT)?)).map_err(nm::Refused::name)?;
    let mut inputs = [0.0f32; 16];
    for (i, v) in inputs.iter_mut().enumerate() {
        *v = f32::from_bits(w(state + 4 * i as u32)?);
    }
    let position = [w(position)?, w(position + 4)?, w(position + 8)?].map(f32::from_bits);
    let key = Key {
        definition,
        bitmap: w(draw + 4)?,
        tint: [w(tint)?, w(tint + 4)?, w(tint + 8)?, w(tint + 12)?],
        size,
    };
    Ok((key, nm::spawn(&inputs, position, w(particle + 0x14)?, w(particle + 0x3c)?, particle)))
}

/// At an answered callback (`esp` its entry's), its emission `p` made and
/// not yet written. True when the mirror holds the particle: the caller
/// makes the writes and returns. False draws the held batch first, so the
/// caller's own draw keeps the guest's order.
pub(super) fn take(m: &mut dyn GuestMemory, esp: u32, p: &Prediction) -> bool {
    let Some(host) = host() else {
        kept("no_host");
        return false;
    };
    let (key, spawn) = match particle(&*m, esp, p).and_then(|(k, s)| host.accepts(&*m, &k).map(|()| (k, s))) {
        Ok(v) => v,
        Err(cause) => {
            finish(m);
            kept(cause);
            return false;
        }
    };
    if HELD.with(|h| h.borrow().as_ref().is_some_and(|b| b.key != key)) {
        finish(m);
    }
    HELD.with(|h| h.borrow_mut().get_or_insert_with(|| Batch { key, spawns: Vec::new() }).spawns.push(spawn));
    true
}

/// Draws the held batch: at the particle draw's return, and before a
/// particle drawn on its own.
pub(super) fn finish(m: &mut dyn GuestMemory) {
    let Some(batch) = HELD.with(|h| h.borrow_mut().take()) else {
        return;
    };
    let n = batch.spawns.len() as u64;
    match host().ok_or("no_host").and_then(|h| h.draw(m, &batch)) {
        Ok(()) => count("answer_mirrored", n),
        Err(cause) => {
            count(&format!("mirror_dropped_{cause}"), n);
        }
    }
}

/// The frame's state is let go: a batch no draw's return took (its hook
/// missing) is dropped by name.
pub(super) fn forget() {
    if let Some(b) = HELD.with(|h| h.borrow_mut().take()) {
        count("mirror_dropped_unfinished", b.spawns.len() as u64);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::effects_shadow::fixtures::{ESP, world};
    use engine::memory::FlatMemory;
    use engine::particle_emission as pe;

    /// Takes what it is given; refuses one definition or every draw.
    #[derive(Default)]
    pub(crate) struct Records {
        pub batches: Mutex<Vec<Batch>>,
        pub refuse_definition: Option<u32>,
        pub refuse_draw: bool,
    }

    impl MirrorHost for Records {
        fn accepts(&self, _: &dyn GuestMemory, key: &Key) -> Result<(), &'static str> {
            if self.refuse_definition == Some(key.definition) { Err("definition") } else { Ok(()) }
        }
        fn draw(&self, _: &mut dyn GuestMemory, batch: &Batch) -> Result<(), &'static str> {
            if self.refuse_draw {
                return Err("target");
            }
            self.batches.lock().unwrap().push(batch.clone());
            Ok(())
        }
    }

    fn kept_count(cause: &str) -> u64 {
        let name = format!("scene_kept_15_particle_mirror_{cause}");
        crate::native_layers::counters().into_iter().find(|(k, _)| *k == name).map_or(0, |(_, v)| v)
    }

    /// The test world with a view-facing class and the retail tint scale.
    pub(crate) fn mirror_world() -> FlatMemory {
        let mut m = world();
        let definition = m.u32(m.u32(ESP + 28).unwrap()).unwrap();
        m.set_u16(definition + 4, 0).unwrap();
        m.set_u32(BYTE, (1.0f32 / 255.0).to_bits()).unwrap();
        m.set_u32(TINT, 1.0f32.to_bits()).unwrap();
        m
    }

    #[test]
    fn a_particle_draws_batched_at_the_draws_return_from_its_state_as_the_callback_leaves_it() {
        let mut m = mirror_world();
        let state = m.u32(ESP + 8).unwrap();
        // A stale life the callback recomputes (its mask bit is clear) and
        // the age it reads as memory holds it.
        m.set_u32(state, 0.75f32.to_bits()).unwrap();
        m.set_u32(state + 4, 2.0f32.to_bits()).unwrap();
        let host = Arc::new(Records::default());
        set_host(host.clone());
        let p = pe::predict(&m, ESP, 0x1f80, 0x027f).unwrap();
        assert!(p.writes.contains_key(&state), "the callback caches the life");
        assert!(take(&mut m, ESP, &p));
        assert!(take(&mut m, ESP, &p), "a second particle of the same system joins the batch");
        assert!(host.batches.lock().unwrap().is_empty(), "nothing draws before the return");
        finish(&mut m);
        let batches = host.batches.lock().unwrap();
        assert_eq!(batches.len(), 1);
        let life = f32::from_bits(word(&m, &p, state).unwrap());
        let s = batches[0].spawns[0];
        assert_eq!(batches[0].spawns.len(), 2);
        assert_eq!(s.age, 2.0);
        assert_eq!(s.lifetime, if life > 0.0 { 2.0 / life } else { f32::INFINITY }, "the life the callback cached, not memory's stale one");
        assert_eq!(batches[0].key.size, m.u32(ESP + 20).unwrap());
    }

    #[test]
    fn a_particle_the_mirror_refuses_draws_the_held_batch_first_and_is_kept_by_name() {
        let mut m = mirror_world();
        let p = pe::predict(&m, ESP, 0x1f80, 0x027f).unwrap();
        let host = Arc::new(Records::default());
        set_host(host.clone());
        assert!(take(&mut m, ESP, &p));
        let definition = m.u32(m.u32(ESP + 28).unwrap()).unwrap();
        set_host(Arc::new(Records { refuse_definition: Some(definition), ..Records::default() }));
        let before = kept_count("definition");
        // The held batch goes to the host installed when it is drawn.
        assert!(!take(&mut m, ESP, &p));
        assert_eq!(kept_count("definition"), before + 1);
        assert!(HELD.with(|h| h.borrow().is_none()), "the batch drew before the refused particle");

        // A class other than view-facing keeps its direct draw.
        set_host(host.clone());
        m.set_u16(definition + 4, 3).unwrap();
        let before = kept_count("orientation");
        assert!(!take(&mut m, ESP, &p));
        assert_eq!(kept_count("orientation"), before + 1);
    }

    #[test]
    fn a_batch_the_host_refuses_is_dropped_by_name() {
        let mut m = mirror_world();
        let p = pe::predict(&m, ESP, 0x1f80, 0x027f).unwrap();
        set_host(Arc::new(Records { refuse_draw: true, ..Records::default() }));
        let dropped = || crate::effects_shadow::tested_prefix("mirror_dropped_");
        let before = dropped();
        assert!(take(&mut m, ESP, &p));
        finish(&mut m);
        assert_eq!(dropped(), before + 1, "dropped by name");
    }
}
