//! The Pogo Stick's pose in a frame (docs/game/pogo-stick.md, "The
//! pose"): the riders' node rows posed for the draw and put back after it.
//!
//! **Where.** An object's node rows are in its body
//! (`guest_layout::Object::nodes`), written each tick by the simulation
//! and read by the object's render setup, which runs inside
//! `points::RENDER_VIEWS`. A hook before that point writes the posed rows
//! of every rider ([`crate::pogo_ride::ridden`]) and of the stick each
//! holds; a hook after it writes the game's own rows back. Both the
//! native objects pass and the D3D path draw from those rows, so one
//! write covers both.
//!
//! **The simulation never sees a posed row.** The frame is drawn between
//! ticks on the game's thread, the bytes kept are the rows' own and are
//! written back whole ([`restore`]), and nothing outside the two bodies'
//! row arrays is written. A film's digest of memory after a tick is the
//! same with the feature on or off.
//!
//! A biped is posed once its skeleton has been read from the map file
//! ([`crate::pogo_skeleton`]); until then, and on a model the pose's
//! roles are not all found in, it is drawn as the game has it.

use std::sync::{Arc, Mutex};

use guest_layout::memory::GuestMemory;
use guest_layout::view::objects;
use guest_layout::{DatumHandle, points};
use frame_scene::NodeMatrix;
use hooks::Flow;
use pogo::RIG;

use crate::pogo_pose::{Keys, STICK_NODES, Skeleton, pose};
use crate::pogo_ride::Ridden;

/// A row's size in a body.
pub const ROW: u32 = guest_layout::object::NODE_MATRIX_SIZE;
/// The most rows a body is read for: no model has more nodes.
pub const MAX_ROWS: u32 = 255;

/// The rows of an object as they were before the pose.
#[derive(Clone, Debug, PartialEq)]
pub struct Kept {
    /// The object's datum.
    pub object: u32,
    /// Where its rows are, and how many.
    pub addr: u32,
    pub count: u32,
    /// The rows' bytes.
    pub bytes: Vec<u8>,
}

/// The body of the live object `object`: its type and address.
fn body_of(m: &dyn GuestMemory, object: u32) -> Option<(u8, objects::Body)> {
    objects::get(m, DatumHandle(object)).ok().flatten()
}

/// Where the live object `object`'s node rows are, and how many (the
/// body's size and offset words, as `guest_layout::Object::nodes` reads
/// them). `None` for an object that is gone or has no rows.
pub fn node_rows(m: &dyn GuestMemory, object: u32) -> Option<(u32, u32)> {
    let (_, body) = body_of(m, object)?;
    let size = u32::from(
        m.u16(
            body.addr
                .checked_add(guest_layout::Object::NODES_SIZE.offset)?,
        )
        .ok()?,
    );
    let offset = m
        .u16(
            body.addr
                .checked_add(guest_layout::Object::NODES_OFFSET.offset)?,
        )
        .ok()? as i16;
    let count = size / ROW;
    if offset <= 0 || count == 0 || count > MAX_ROWS {
        return None;
    }
    let addr = body.addr.checked_add(offset as u32)?;
    addr.checked_add(count * ROW)?;
    Some((addr, count))
}

fn rows_of(bytes: &[u8]) -> Vec<NodeMatrix> {
    bytes
        .chunks_exact(ROW as usize)
        .map(|row| {
            let mut w = [0.0f32; 13];
            for (slot, b) in w.iter_mut().zip(row.chunks_exact(4)) {
                *slot = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            }
            NodeMatrix::from_words(&w)
        })
        .collect()
}

fn bytes_of(rows: &[NodeMatrix]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rows.len() * ROW as usize);
    for r in rows {
        out.extend_from_slice(&r.scale.to_le_bytes());
        for v in [r.forward, r.left, r.up, r.position].iter().flatten() {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

/// Reads `object`'s rows as they are: what to keep, and the rows.
fn read(m: &dyn GuestMemory, object: u32) -> Option<(Kept, Vec<NodeMatrix>)> {
    let (addr, count) = node_rows(m, object)?;
    let bytes = m.bytes(addr, (count * ROW) as usize).ok()?;
    let rows = rows_of(&bytes);
    Some((
        Kept {
            object,
            addr,
            count,
            bytes,
        },
        rows,
    ))
}

/// Poses the rider `r` on `sk` and the stick it holds: writes the posed
/// rows into the two bodies and adds the rows they had to `kept`, each
/// before its write. `false` when nothing was written: the pose is the
/// game's own, the biped's rows are not the skeleton's, the stick has
/// other than a frame and a piston, or either object is gone.
pub fn pose_rider(
    m: &mut dyn GuestMemory,
    r: &Ridden,
    sk: &Skeleton,
    keys: &Keys,
    kept: &mut Vec<Kept>,
) -> bool {
    if !r.sample.posed() || r.unit == r.weapon {
        return false;
    }
    let Some((biped, biped_rows)) = read(m, r.unit) else {
        return false;
    };
    let Some((stick, stick_rows)) = read(m, r.weapon) else {
        return false;
    };
    if biped_rows.len() != sk.len() {
        return false;
    }
    let Ok(stick_rows) = <[NodeMatrix; STICK_NODES]>::try_from(stick_rows) else {
        return false;
    };
    let Some(posed) = pose(sk, keys, &RIG, &biped_rows, &stick_rows, &r.sample) else {
        return false;
    };
    let mut wrote = false;
    for (was, rows) in [(biped, &posed.biped[..]), (stick, &posed.stick[..])] {
        let (addr, bytes) = (was.addr, bytes_of(rows));
        // Kept before the write, so a write that stops part way is undone.
        kept.push(was);
        wrote |= m.write(addr, &bytes).is_ok();
    }
    wrote
}

/// Writes every kept row back and empties `kept`: how many objects were
/// written. An object that has gone since, or whose rows are no longer
/// where they were or as many, is left alone.
pub fn restore(m: &mut dyn GuestMemory, kept: &mut Vec<Kept>) -> usize {
    let mut written = 0;
    // Last kept first, so an object kept twice ends on its first rows.
    for k in kept.drain(..).rev() {
        if node_rows(m, k.object) == Some((k.addr, k.count)) && m.write(k.addr, &k.bytes).is_ok() {
            written += 1;
        }
    }
    written
}

/// The skeleton of the biped `unit` in the loaded map `map`, when it has
/// been read.
fn skeleton_for(m: &dyn GuestMemory, map: &str, unit: u32) -> Option<Arc<Skeleton>> {
    let (kind, body) = body_of(m, unit)?;
    if kind != objects::BIPED {
        return None;
    }
    let definition = body.definition(m).ok()??;
    let mode = crate::pogo_skeleton::mode_of(m, definition.raw())?;
    crate::pogo_skeleton::request(map, mode)
}

/// Poses every rider in `ridden` that has a pose and a skeleton; the rows
/// they had go to `kept`.
pub fn pose_all(m: &mut dyn GuestMemory, ridden: &[Ridden], keys: &Keys, kept: &mut Vec<Kept>) {
    let mut map = None;
    for r in ridden.iter().filter(|r| r.sample.posed()) {
        if map.is_none() {
            map = crate::health::loaded_map_name(m).ok();
        }
        let Some(name) = map.as_deref().filter(|n| !n.is_empty()) else {
            return;
        };
        if let Some(sk) = skeleton_for(m, name, r.unit) {
            pose_rider(m, r, &sk, keys, kept);
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The two hooks around the frame's views.
pub fn install_hooks(b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
    let kept = Arc::new(Mutex::new(Vec::<Kept>::new()));
    let keeping = kept.clone();
    b.raw()
        .before(
            &points::RENDER_VIEWS,
            "pogo: pose the riders",
            move |call| {
                let ridden = crate::pogo_ride::ridden();
                if ridden.iter().any(|r| r.sample.posed())
                    && let Ok(keys) = crate::pogo_pose::keys()
                {
                    let mut guest = call.ctx.memory();
                    let mut kept = lock(&keeping);
                    // Rows a frame that never reached its end left posed.
                    restore(&mut *guest, &mut kept);
                    pose_all(&mut *guest, &ridden, keys, &mut kept);
                }
                Ok(Flow::Continue)
            },
        )
        // The rows are posed before any view of the frame is drawn,
        // including the agent spawn view's and Theater's camera.
        .after_feature("agent-frames")
        .after_feature("theater-camera")
        .context_only();
    b.raw()
        .after(
            &points::RENDER_VIEWS,
            "pogo: put the rows back",
            move |call| {
                if !lock(&kept).is_empty() {
                    let mut guest = call.ctx.memory();
                    restore(&mut *guest, &mut lock(&kept));
                }
                Ok(())
            },
        )
        // After any other hook that reads the rows as drawn.
        .last()
        .context_only();
    Ok(())
}

#[cfg(test)]
mod tests {
    use guest_layout::testing::FakeGuest;
    use guest_layout::view::fake;
    use guest_layout::{Object, ObjectHeader};

    use super::*;
    use crate::pogo_pose::tests::{carried_stick, keys, sample, spartan, standing};

    const POOL: u32 = 0x5000;
    const BIPED_BODY: u32 = 0x2_0000;
    const STICK_BODY: u32 = 0x3_0000;
    /// Where the rows start in each body.
    const ROWS_AT: u32 = 0x400;
    const BODY_LEN: u32 = 0x1000;
    const AT: [f32; 3] = [12.0, -7.0, 3.0];

    struct World {
        m: FakeGuest,
        biped: u32,
        stick: u32,
        stick_header: u32,
    }

    fn put_rows(m: &mut FakeGuest, body: u32, rows: &[NodeMatrix]) {
        m.put_u16(
            body + Object::NODES_SIZE.offset,
            (rows.len() as u32 * ROW) as u16,
        )
        .put_u16(body + Object::NODES_OFFSET.offset, ROWS_AT as u16)
        .put(body + ROWS_AT, &bytes_of(rows));
    }

    /// A biped standing with a stick in its hand, each a live object
    /// whose body is filled with a pattern around its rows.
    fn world(sk: &Skeleton) -> World {
        let mut m = FakeGuest::new();
        let pool = fake::datums::<objects::Header>(&mut m, POOL, 2);
        let (biped, biped_header) = fake::push(&mut m, &pool);
        let (stick, stick_header) = fake::push(&mut m, &pool);
        m.put_u32(points::OBJECTS.base, POOL);
        for (header, kind, body) in [
            (biped_header.addr, objects::BIPED, BIPED_BODY),
            (stick_header.addr, objects::WEAPON, STICK_BODY),
        ] {
            let pattern: Vec<u8> = (0..BODY_LEN).map(|i| (i * 7 + body) as u8).collect();
            m.put(body, &pattern)
                .put_u8(header + ObjectHeader::KIND.offset, kind)
                .put_u32(header + ObjectHeader::BODY.offset, body);
        }
        put_rows(&mut m, BIPED_BODY, &standing(sk, 1.0, AT));
        put_rows(&mut m, STICK_BODY, &carried_stick(AT));
        World {
            m,
            biped: biped.raw(),
            stick: stick.raw(),
            stick_header: stick_header.addr,
        }
    }

    fn bodies(m: &FakeGuest) -> Vec<u8> {
        let mut all = m.bytes(BIPED_BODY, BODY_LEN as usize).unwrap();
        all.extend(m.bytes(STICK_BODY, BODY_LEN as usize).unwrap());
        all
    }

    fn rider(w: &World, mounted: f32) -> Ridden {
        Ridden {
            slot: 0,
            local: true,
            unit: w.biped,
            weapon: w.stick,
            sample: sample(mounted, 0.5, 1.0, None),
        }
    }

    #[test]
    fn rows_go_to_bytes_and_back() {
        let sk = spartan();
        let rows = standing(&sk, 0.3, AT);
        let bytes = bytes_of(&rows);
        assert_eq!(bytes.len(), rows.len() * ROW as usize);
        assert_eq!(rows_of(&bytes), rows);
        // The guest's order: scale, forward, left, up, position.
        assert_eq!(bytes[..4], rows[0].scale.to_le_bytes());
        assert_eq!(bytes[4..8], rows[0].forward[0].to_le_bytes());
        assert_eq!(bytes[0x28..0x2c], rows[0].position[0].to_le_bytes());
    }

    #[test]
    fn the_pose_is_written_and_every_byte_is_put_back() {
        let sk = spartan();
        let mut w = world(&sk);
        let before = bodies(&w.m);
        let mut kept = Vec::new();
        let posed = rider(&w, 1.0);
        assert!(pose_rider(
            &mut w.m,
            &posed,
            &sk,
            keys(),
            &mut kept
        ));
        assert_eq!(kept.len(), 2);
        let during = bodies(&w.m);
        assert_ne!(during, before);
        // Only the rows changed: the bytes around them are the body's.
        for body in [0, BODY_LEN as usize] {
            let rows_end = (ROWS_AT + sk.len() as u32 * ROW) as usize;
            assert_eq!(
                during[body..body + ROWS_AT as usize],
                before[body..body + ROWS_AT as usize]
            );
            assert_eq!(
                during[body + rows_end..body + BODY_LEN as usize],
                before[body + rows_end..body + BODY_LEN as usize]
            );
        }
        // What is drawn is the pose's rows.
        let (_, drawn) = read(&w.m, w.biped).unwrap();
        let posed = pose(
            &sk,
            keys(),
            &RIG,
            &standing(&sk, 1.0, AT),
            &carried_stick(AT),
            &rider(&w, 1.0).sample,
        )
        .unwrap();
        assert_eq!(drawn, posed.biped);
        assert_eq!(read(&w.m, w.stick).unwrap().1, posed.stick);

        assert_eq!(restore(&mut w.m, &mut kept), 2);
        assert!(kept.is_empty());
        assert_eq!(bodies(&w.m), before);
        // Nothing kept, nothing written.
        assert_eq!(restore(&mut w.m, &mut kept), 0);
        assert_eq!(bodies(&w.m), before);
    }

    #[test]
    fn a_pose_over_a_pose_still_ends_on_the_games_rows() {
        // A frame that never reached its end, then the next one's pose.
        let sk = spartan();
        let mut w = world(&sk);
        let before = bodies(&w.m);
        let mut kept = Vec::new();
        let posed = rider(&w, 1.0);
        assert!(pose_rider(
            &mut w.m,
            &posed,
            &sk,
            keys(),
            &mut kept
        ));
        let posed = rider(&w, 0.5);
        assert!(pose_rider(
            &mut w.m,
            &posed,
            &sk,
            keys(),
            &mut kept
        ));
        assert_eq!(kept.len(), 4);
        restore(&mut w.m, &mut kept);
        assert_eq!(bodies(&w.m), before);
    }

    #[test]
    fn a_carried_stick_writes_nothing() {
        let sk = spartan();
        let mut w = world(&sk);
        let before = bodies(&w.m);
        let mut kept = Vec::new();
        let mut r = rider(&w, 0.0);
        r.sample.compress = 0.0;
        assert!(!pose_rider(&mut w.m, &r, &sk, keys(), &mut kept));
        assert!(kept.is_empty());
        assert_eq!(bodies(&w.m), before);
    }

    #[test]
    fn rows_that_are_not_the_models_are_skipped() {
        let sk = spartan();
        // A stick with three nodes.
        let mut w = world(&sk);
        let stick = carried_stick(AT);
        put_rows(&mut w.m, STICK_BODY, &[stick[0], stick[1], stick[1]]);
        let before = bodies(&w.m);
        let mut kept = Vec::new();
        let posed = rider(&w, 1.0);
        assert!(!pose_rider(
            &mut w.m,
            &posed,
            &sk,
            keys(),
            &mut kept
        ));
        // And with one.
        put_rows(&mut w.m, STICK_BODY, &stick[..1]);
        let one = bodies(&w.m);
        let posed = rider(&w, 1.0);
        assert!(!pose_rider(
            &mut w.m,
            &posed,
            &sk,
            keys(),
            &mut kept
        ));
        assert_eq!(bodies(&w.m), one);
        assert_ne!(one, before);
        // A biped with a node fewer than its skeleton.
        let mut w = world(&sk);
        let rows = standing(&sk, 1.0, AT);
        put_rows(&mut w.m, BIPED_BODY, &rows[..rows.len() - 1]);
        let before = bodies(&w.m);
        let posed = rider(&w, 1.0);
        assert!(!pose_rider(
            &mut w.m,
            &posed,
            &sk,
            keys(),
            &mut kept
        ));
        assert_eq!(bodies(&w.m), before);
        // A body with no rows, and a rider whose stick is its own body.
        w.m.put_u16(BIPED_BODY + Object::NODES_OFFSET.offset, 0);
        assert_eq!(node_rows(&w.m, w.biped), None);
        let posed = rider(&w, 1.0);
        assert!(!pose_rider(
            &mut w.m,
            &posed,
            &sk,
            keys(),
            &mut kept
        ));
        let mut own = rider(&w, 1.0);
        own.weapon = own.unit;
        assert!(!pose_rider(&mut w.m, &own, &sk, keys(), &mut kept));
        assert!(kept.is_empty());
    }

    #[test]
    fn an_object_gone_or_changed_since_is_not_written() {
        let sk = spartan();
        // The stick's object goes between the pose and the restore.
        let mut w = world(&sk);
        let before = bodies(&w.m);
        let mut kept = Vec::new();
        let posed = rider(&w, 1.0);
        assert!(pose_rider(
            &mut w.m,
            &posed,
            &sk,
            keys(),
            &mut kept
        ));
        let during = bodies(&w.m);
        w.m.put_u16(w.stick_header + ObjectHeader::SALT.offset, 0x7777);
        assert_eq!(restore(&mut w.m, &mut kept), 1);
        assert!(kept.is_empty());
        let after = bodies(&w.m);
        let half = BODY_LEN as usize;
        assert_eq!(after[..half], before[..half]);
        assert_eq!(after[half..], during[half..]);

        // The biped's rows are fewer than they were.
        let mut w = world(&sk);
        let posed = rider(&w, 1.0);
        assert!(pose_rider(
            &mut w.m,
            &posed,
            &sk,
            keys(),
            &mut kept
        ));
        w.m.put_u16(
            BIPED_BODY + Object::NODES_SIZE.offset,
            ((sk.len() as u32 - 1) * ROW) as u16,
        );
        let during = bodies(&w.m);
        assert_eq!(restore(&mut w.m, &mut kept), 1);
        assert_eq!(bodies(&w.m)[..half], during[..half]);

        // A rider whose unit has gone is not posed at all.
        let w = world(&sk);
        let mut gone = rider(&w, 1.0);
        gone.unit = 0xdead_0005;
        let mut m = w.m;
        assert!(!pose_rider(&mut m, &gone, &sk, keys(), &mut kept));
        assert!(kept.is_empty());
        assert_eq!(node_rows(&FakeGuest::new(), 0), None);
    }

    #[test]
    fn no_map_and_no_tags_poses_no_one() {
        // The skeleton comes by the loaded map's name and the tag index;
        // a guest with neither is left as it is.
        let sk = spartan();
        let mut w = world(&sk);
        let before = bodies(&w.m);
        let mut kept = Vec::new();
        let r = rider(&w, 1.0);
        pose_all(&mut w.m, &[r], keys(), &mut kept);
        assert!(kept.is_empty());
        assert_eq!(bodies(&w.m), before);
    }
}
