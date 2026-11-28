//! Playing a film that begins at a start point instead of at boot
//! (docs/game/input-film.md, "Start points"; docs/game/theater-design.md,
//! slice 15). A film with `FLAG_START_POINT` holds one start point block
//! before its first tick chunk. [`plan`] reads and checks it; [`apply`]
//! puts its state into the running game, at the tick the player stands
//! at after booting the film's map, and gives the tick the first film
//! tick follows.
//!
//! The player still has to boot the match's map to its first tick (the
//! arena has to exist) and then call [`apply`] before it consumes the
//! film's first tick chunk; the film's tick axis then starts at
//! [`StartPlan::first_tick`]. Both are the player's wiring, which waits on
//! the engine film reader (the film crate's list is in the slice 15 report
//! in docs/game/theater-design.md).
use super::start_point::StartPoint;
use super::start_point_state::{self as state, ExtRecords};
use anyhow::{Result, bail, ensure};
use guest_layout::GuestMemory;
use film::file::{FLAG_START_POINT, FilmReader};
use std::io::{Read, Seek};

/// A start point read and checked, and where the film's ticks resume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartPlan {
    pub point: StartPoint,
}

impl StartPlan {
    /// The first film tick the player simulates after the load: the tick
    /// after the one whose end the state is.
    pub fn first_tick(&self) -> u32 {
        self.point.tick + 1
    }
}

/// The start point of a film, from its flags and its start point blocks.
/// `None` for a film from boot. A film that says it begins at a start
/// point without one, with a block it does not say, or with two, is
/// refused; so is any block whose tag, digest, size or hash is wrong
/// ([`StartPoint::decode`]).
pub fn plan_from_blocks(
    flags: u16,
    blocks: &[Vec<u8>],
    key: &[u8],
    arena_len: usize,
    film_hash: u64,
) -> Result<Option<StartPlan>> {
    let flagged = flags & FLAG_START_POINT != 0;
    match (flagged, blocks.len()) {
        (false, 0) => return Ok(None),
        (false, _) => bail!("film holds a start point its header does not name"),
        (true, 0) => bail!("film begins at a start point and holds none"),
        (true, 1) => {}
        (true, _) => bail!("film holds more than one start point"),
    }
    let point = StartPoint::decode(&blocks[0], key, arena_len, film_hash)?;
    ensure!(
        point.tick > 0,
        "start point is at tick zero; a film from boot has none"
    );
    Ok(Some(StartPlan { point }))
}

/// [`plan_from_blocks`] over an opened film, reading the stored blocks.
pub fn plan<R: Read + Seek>(
    film: &mut FilmReader<R>,
    key: &[u8],
    film_hash: u64,
) -> Result<Option<StartPlan>> {
    let mut blocks = Vec::new();
    for block in film.start_points.clone() {
        blocks.push(film.read_block(&block)?);
    }
    plan_from_blocks(film.flags, &blocks, key, state::arena_len(), film_hash)
}

/// Load the plan's state and check it: the hash after the load must be
/// the one the tag names. On an error the game holds a state that is not
/// the film's, so the caller discards the process
/// ([`super::replay`]'s keyframe restore does the same).
pub fn apply(plan: &StartPlan, mem: &mut dyn GuestMemory, ext: &mut dyn ExtRecords) -> Result<u32> {
    let after = state::load(mem, ext, &plan.point)?;
    plan.point.check_loaded(after)?;
    Ok(plan.first_tick())
}

#[cfg(test)]
mod tests {
    use super::super::start_point_state::fakes::{Ext, guest};
    use super::*;

    const KEY: &[u8] = b"synthetic key";

    fn block(tick: u32) -> (Vec<u8>, u64) {
        let ext = Ext {
            bytes: vec![7],
            refuse: false,
        };
        let mut p = state::capture(&guest(1), &ext, tick, vec![1, 2]).unwrap();
        p.tick = tick;
        (p.encode(KEY).unwrap(), p.arena_hash)
    }

    #[test]
    fn a_film_from_boot_has_no_plan() {
        assert_eq!(
            plan_from_blocks(0, &[], KEY, state::arena_len(), 0).unwrap(),
            None
        );
        let (bytes, hash) = block(10);
        assert!(plan_from_blocks(0, &[bytes], KEY, state::arena_len(), hash).is_err());
    }

    #[test]
    fn the_flag_and_the_blocks_must_agree() {
        let (bytes, hash) = block(10);
        let n = state::arena_len();
        assert!(plan_from_blocks(FLAG_START_POINT, &[], KEY, n, hash).is_err());
        let two = [bytes.clone(), bytes];
        assert!(plan_from_blocks(FLAG_START_POINT, &two, KEY, n, hash).is_err());
    }

    #[test]
    fn a_film_at_a_start_point_loads_and_resumes_after_its_tick() {
        let (bytes, hash) = block(120);
        let plan = plan_from_blocks(FLAG_START_POINT, &[bytes], KEY, state::arena_len(), hash)
            .unwrap()
            .unwrap();
        assert_eq!(plan.first_tick(), 121);
        let mut target = guest(5);
        let mut ext = Ext::default();
        assert_eq!(apply(&plan, &mut target, &mut ext).unwrap(), 121);
        assert_eq!(state::current_hash(&target, &ext).unwrap(), hash);
        assert_eq!(ext.bytes, [7]);
    }

    #[test]
    fn a_start_point_with_one_byte_changed_is_refused_whatever_the_byte() {
        let (bytes, hash) = block(120);
        let n = state::arena_len();
        for at in (0..bytes.len()).step_by(97).chain([bytes.len() - 1]) {
            let mut bad = bytes.clone();
            bad[at] ^= 1;
            assert!(
                plan_from_blocks(FLAG_START_POINT, &[bad], KEY, n, hash).is_err(),
                "byte {at} changed and was accepted"
            );
        }
        assert!(plan_from_blocks(FLAG_START_POINT, std::slice::from_ref(&bytes), b"other", n, hash).is_err());
        assert!(plan_from_blocks(FLAG_START_POINT, &[bytes], KEY, n, hash ^ 1).is_err());
    }

    #[test]
    fn a_state_that_loads_to_another_hash_fails_the_apply() {
        let (bytes, hash) = block(30);
        let mut plan = plan_from_blocks(FLAG_START_POINT, &[bytes], KEY, state::arena_len(), hash)
            .unwrap()
            .unwrap();
        // The tag was right but the records differ from what the hash had.
        plan.point.ext = vec![9];
        let mut target = guest(5);
        let mut ext = Ext::default();
        assert!(apply(&plan, &mut target, &mut ext).is_err());
    }

    #[test]
    fn tick_zero_is_not_a_start_point() {
        let (bytes, hash) = block(0);
        assert!(
            plan_from_blocks(FLAG_START_POINT, &[bytes], KEY, state::arena_len(), hash).is_err()
        );
    }
}
