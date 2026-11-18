//! The occlusion test's side in guest memory: the camera and the objects
//! as the game holds them while it builds a view's object render records,
//! and the skip itself (docs/graphics/occlusion-culling.md, "The read").
//!
//! Every address and offset is a research point from the read of the 1.0
//! XBE (labels there); no game data.

use super::*;

/// The main view's visibility struct (`0x00547f88`); the dynamic-light
/// views use `0x0054aa68`.
pub const MAIN_VIS: u32 = 0x0054_7f88;
/// Its view kind (0 the main view).
pub const VIS_KIND: u32 = 0x2a78;
/// Its object list pointer: `+4` the u16 count, `+8` the u16 flags, `+0xc`
/// the u32 handles.
pub const VIS_OBJECTS: u32 = 0x14;
pub const LIST_COUNT: u32 = 0x4;
pub const LIST_FLAGS: u32 = 0x8;
pub const LIST_HANDLES: u32 = 0xc;
/// The flag bit the setup's own "too small to draw" exit sets.
pub const FLAG_NOT_DRAWN: u8 = 1;

/// The object datum array pointer; its `+0x44` is the 12-byte headers,
/// whose `+8` is the object body.
pub const OBJECTS: u32 = 0x004e_0300;
pub const OBJECTS_DATA: u32 = 0x44;
pub const HEADER_SIZE: u32 = 12;
pub const HEADER_BODY: u32 = 8;
/// Body fields.
pub const BODY_PARENT: u32 = 0x14;
pub const BODY_SPHERE_CENTER: u32 = 0x30;
pub const BODY_SPHERE_RADIUS: u32 = 0x3c;

/// The render camera (0x74 bytes): position `+0`, forward `+0xc`, up
/// `+0x18`, vertical FOV `+0x28`, viewport `+0x30` (i16 top, left, bottom,
/// right), near `+0x40` (S).
pub const CAMERA: u32 = 0x004b_9da0;
/// The view index `render_game_view` stores (`[view+4]`).
pub const VIEW_INDEX: u32 = 0x004b_a050;
/// The view's player unit (S: the argument `render_view_scene` keeps).
pub const VIEW_PLAYER: u32 = 0x004b_9ed8;
/// The frame counter `render_views` increments.
pub const FRAME: u32 = 0x004b_a034;

/// The camera as the game left it for this view.
pub fn read_camera(m: &dyn GuestMemory) -> Result<Option<Camera>, MemFault> {
    let position = m.vec3(CAMERA)?;
    let forward = m.vec3(CAMERA + 0xc)?;
    let up = m.vec3(CAMERA + 0x18)?;
    let fov_y = m.f32(CAMERA + 0x28)?;
    let top = m.u16(CAMERA + 0x30)? as i16 as f32;
    let left = m.u16(CAMERA + 0x32)? as i16 as f32;
    let bottom = m.u16(CAMERA + 0x34)? as i16 as f32;
    let right = m.u16(CAMERA + 0x36)? as i16 as f32;
    let near = m.f32(CAMERA + 0x40)?;
    let (w, h) = (right - left, bottom - top);
    if !(w > 0.0 && h > 0.0 && fov_y > 0.0 && fov_y < 3.1) {
        return Ok(None);
    }
    let near = if near.is_finite() && near > 0.0 && near < 10.0 {
        near
    } else {
        0.05
    };
    let c = Camera::from_fov(position, forward, up, fov_y, w / h, near);
    Ok(c.valid().then_some(c))
}

/// An object's body pointer from its handle (index in the low 16 bits).
pub fn body(m: &dyn GuestMemory, handle: u32) -> Result<u32, MemFault> {
    let data = m.u32(m.u32(OBJECTS)? + OBJECTS_DATA)?;
    m.u32(data + (handle & 0xffff) * HEADER_SIZE + HEADER_BODY)
}

/// The handle of list entry `index` of a visibility struct.
pub fn listed(m: &dyn GuestMemory, vis: u32, index: i16) -> Result<u32, MemFault> {
    let list = m.u32(vis + VIS_OBJECTS)?;
    m.u32(
        m.u32(list + LIST_HANDLES)?
            .wrapping_add((index as i32 as u32).wrapping_mul(4)),
    )
}

/// The handles that are never culled for this view: the view's player
/// unit, the objects it rides in (its parents), and so everything attached
/// to either (checked by [`exempt`]).
pub fn protected(m: &dyn GuestMemory) -> Result<Vec<u32>, MemFault> {
    let mut out = Vec::new();
    let mut h = m.u32(VIEW_PLAYER)?;
    while h != u32::MAX && out.len() < 8 {
        out.push(h);
        h = m.u32(body(m, h)? + BODY_PARENT)?;
    }
    Ok(out)
}

/// Whether `handle` is protected or attached (through up to eight
/// parents) to a protected object: a held weapon, a passenger.
pub fn exempt(m: &dyn GuestMemory, handle: u32, protected: &[u32]) -> Result<bool, MemFault> {
    let mut h = handle;
    for _ in 0..8 {
        if h == u32::MAX {
            return Ok(false);
        }
        if protected.contains(&h) {
            return Ok(true);
        }
        h = m.u32(body(m, h)? + BODY_PARENT)?;
    }
    Ok(false)
}

/// An object's bounds for the main view (the sphere at body `+0x30`).
pub fn bounds(m: &dyn GuestMemory, handle: u32, protected: &[u32]) -> Result<Bounds, MemFault> {
    let b = body(m, handle)?;
    Ok(Bounds {
        key: handle,
        center: m.vec3(b + BODY_SPHERE_CENTER)?,
        radius: m.f32(b + BODY_SPHERE_RADIUS)?,
        exempt: exempt(m, handle, protected)?,
    })
}

/// The setup's "not drawn" exit, done in place of the call (ESP at the
/// return address, as on entry): the entry's flag bit 0 set, AL 0 (no
/// retry), ECX the flag's address and EDX the flags' base as the original
/// leaves them. The caller pops `0x14` bytes with the return.
pub fn skip_setup(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
    let index = m.u16(r.esp.wrapping_add(4))? as i16 as i32 as u32;
    let list = m.u32(r.esi.wrapping_add(VIS_OBJECTS))?;
    let flags = m.u32(list + LIST_FLAGS)?;
    let at = flags.wrapping_add(index.wrapping_mul(2));
    let v = m.u8(at)?;
    m.set_u8(at, v | FLAG_NOT_DRAWN)?;
    r.ecx = at;
    r.edx = flags;
    r.eax &= !0xff;
    Ok(())
}

/// Depth frames handed over by the GPU side, newest per view.
#[derive(Debug, Default)]
pub struct Mailbox {
    slots: Mutex<Vec<(u32, Arc<Frame>)>>,
}

impl Mailbox {
    pub const fn new() -> Self {
        Self {
            slots: Mutex::new(Vec::new()),
        }
    }
    pub fn publish(&self, view: u32, frame: Frame) {
        let mut s = self.slots.lock().unwrap();
        let frame = Arc::new(frame);
        match s.iter_mut().find(|(v, _)| *v == view) {
            Some(slot) => slot.1 = frame,
            None => s.push((view, frame)),
        }
    }
    pub fn latest(&self, view: u32) -> Option<Arc<Frame>> {
        let s = self.slots.lock().unwrap();
        s.iter()
            .find(|(v, _)| *v == view)
            .map(|(_, f)| Arc::clone(f))
    }
}

/// The per-view trackers and what a run of hooks needs between calls.
#[derive(Debug, Default)]
pub struct Occlusion {
    pub rules: Rules,
    trackers: HashMap<u32, Tracker>,
    /// The view being built and its protected handles; `None` outside a
    /// main view's record build.
    current: Option<(u32, Vec<u32>)>,
}

impl Occlusion {
    pub fn new(rules: Rules) -> Self {
        Self {
            rules,
            trackers: HashMap::new(),
            current: None,
        }
    }

    /// At `OBJECT_RENDER_RECORDS` (EAX the visibility struct): starts a
    /// main view's frame. Other structs (the light views) are left alone.
    pub fn begin_records(
        &mut self,
        m: &dyn GuestMemory,
        vis: u32,
        mail: &Mailbox,
    ) -> Result<(), MemFault> {
        self.current = None;
        if vis != MAIN_VIS || m.u32(vis + VIS_KIND)? != 0 {
            return Ok(());
        }
        let view = m.u32(VIEW_INDEX)?;
        let frame = m.u32(FRAME)? as u64;
        let Some(camera) = read_camera(m)? else {
            return Ok(());
        };
        let rules = self.rules;
        let t = self
            .trackers
            .entry(view)
            .or_insert_with(|| Tracker::new(rules));
        t.begin(frame, camera, mail.latest(view));
        self.current = Some((view, protected(m)?));
        Ok(())
    }

    /// At `OBJECT_RENDER_SETUP` (ESI the visibility struct, ESP at the
    /// return address): whether to skip this object. When it answers
    /// `true` the caller does [`skip_setup`] and returns.
    pub fn decide(&mut self, m: &dyn GuestMemory, r: &Regs) -> Result<bool, MemFault> {
        let Some((view, protected)) = &self.current else {
            return Ok(false);
        };
        if r.esi != MAIN_VIS || m.u32(r.esi + VIS_KIND)? != 0 {
            return Ok(false);
        }
        let index = m.u16(r.esp.wrapping_add(4))? as i16;
        let handle = listed(m, r.esi, index)?;
        let b = bounds(m, handle, protected)?;
        let t = self.trackers.get_mut(view).expect("begun");
        Ok(t.decide(&b))
    }

    /// The main view whose records are being built, if any.
    pub fn current_view(&self) -> Option<u32> {
        self.current.as_ref().map(|(v, _)| *v)
    }

    /// The most telling test over every view since the last call
    /// ([`Tracker::take_probe`]).
    pub fn take_probe(&mut self) -> Option<Probe> {
        let mut best: Option<Probe> = None;
        for t in self.trackers.values_mut() {
            if let Some(p) = t.take_probe()
                && best.as_ref().is_none_or(|b| p.better(b))
            {
                best = Some(p);
            }
        }
        best
    }

    /// The counts over every view.
    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for t in self.trackers.values() {
            c.tested += t.counts.tested;
            c.culled += t.counts.culled;
            c.unknown += t.counts.unknown;
            c.exempt += t.counts.exempt;
            c.resets += t.counts.resets;
            c.no_depth += t.counts.no_depth;
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeGuest;

    const DATA: u32 = 0x0060_0000;
    const BODIES: u32 = 0x0061_0000;
    const LIST: u32 = 0x0062_0000;
    const FLAGS: u32 = 0x0062_1000;
    const HANDLES: u32 = 0x0062_2000;
    const STACK: u32 = 0x0063_0f00;
    const ARRAY: u32 = 0x0064_0000;

    fn handle(i: u32) -> u32 {
        0x8000_0000 | (i * 0x101) << 16 | i
    }

    /// Objects 0..n with spheres at `centers`, listed in order; object 0
    /// is the player, object 1 rides object 2 (a vehicle) and object 3 is
    /// held by the player.
    fn world(centers: &[[f32; 3]]) -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(0x004b_9000, 0x2000)
            .map(0x004e_0000, 0x1000)
            .map(0x0054_7000, 0x7000)
            .map(0x0060_0000, 0x4_1000);
        g.set_u32(OBJECTS, ARRAY).unwrap();
        g.set_u32(ARRAY + OBJECTS_DATA, DATA).unwrap();
        for (i, c) in centers.iter().enumerate() {
            let i = i as u32;
            let body = BODIES + i * 0x100;
            g.set_u32(DATA + i * HEADER_SIZE + HEADER_BODY, body)
                .unwrap();
            g.set_u32(body + BODY_PARENT, u32::MAX).unwrap();
            for (k, v) in c.iter().enumerate() {
                g.set_u32(body + BODY_SPHERE_CENTER + 4 * k as u32, v.to_bits())
                    .unwrap();
            }
            g.set_u32(body + BODY_SPHERE_RADIUS, 1.0f32.to_bits())
                .unwrap();
            g.set_u32(HANDLES + 4 * i, handle(i)).unwrap();
        }
        g.set_u32(BODIES + 0x100 + BODY_PARENT, handle(2)).unwrap();
        g.set_u32(BODIES + 0x300 + BODY_PARENT, handle(0)).unwrap();
        g.set_u32(MAIN_VIS + VIS_OBJECTS, LIST).unwrap();
        g.set_u32(MAIN_VIS + VIS_KIND, 0).unwrap();
        g.set_u32(LIST + LIST_COUNT, centers.len() as u32).unwrap();
        g.set_u32(LIST + LIST_FLAGS, FLAGS).unwrap();
        g.set_u32(LIST + LIST_HANDLES, HANDLES).unwrap();
        g.set_u32(VIEW_PLAYER, handle(0)).unwrap();
        g.set_u32(VIEW_INDEX, 0).unwrap();
        // The camera: at the origin down +x, z up, 90 degrees vertical,
        // a square viewport.
        let cam = [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        for (k, v) in cam.iter().enumerate() {
            g.set_u32(CAMERA + 4 * k as u32, v.to_bits()).unwrap();
        }
        g.set_u32(CAMERA + 0x28, std::f32::consts::FRAC_PI_2.to_bits())
            .unwrap();
        for (k, v) in [0u16, 0, 480, 480].iter().enumerate() {
            g.write(CAMERA + 0x30 + 2 * k as u32, &v.to_le_bytes())
                .unwrap();
        }
        g.set_u32(CAMERA + 0x40, 0.05f32.to_bits()).unwrap();
        g
    }

    fn wall(frame: u64) -> Frame {
        let mut g = DepthGrid::far(64, 32);
        g.depth.iter_mut().for_each(|d| *d = 5.0);
        Frame {
            hiz: HiZ::build(g),
            camera: Camera::from_fov(
                [0.0; 3],
                [1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
                std::f32::consts::FRAC_PI_2,
                1.0,
                0.05,
            ),
            frame,
        }
    }

    fn call_regs(g: &mut FakeGuest, index: u16) -> Regs {
        // ESP at the return address; the index word is stack 0.
        g.set_u32(STACK, 0xdead_beef).unwrap();
        g.set_u32(STACK + 4, 0xffff_0000 | index as u32).unwrap();
        Regs {
            esi: MAIN_VIS,
            esp: STACK,
            eax: 0x1234_5601,
            ..Regs::default()
        }
    }

    #[test]
    fn camera_reads_from_the_render_camera() {
        let g = world(&[]);
        let c = read_camera(&g).unwrap().unwrap();
        assert_eq!(c.position, [0.0; 3]);
        assert_eq!(c.forward, [1.0, 0.0, 0.0]);
        assert!((c.tan_half_y - 1.0).abs() < 1e-6 && (c.tan_half_x - 1.0).abs() < 1e-6);
        // A zero viewport or a silly FOV: no camera, nothing culled.
        let mut g = world(&[]);
        g.write(CAMERA + 0x36, &0u16.to_le_bytes()).unwrap();
        assert_eq!(read_camera(&g).unwrap(), None);
    }

    /// The camera's `+0x28` is the vertical FOV: the projection builder
    /// `0x0002fb70` takes `t = tan(+0x28 / 2)` (`fptan`), scales y by `1 / t`
    /// and x by `1 / (t · aspect)`, aspect the viewport's width over its
    /// height (`+0x30..+0x36`). A 16:9 viewport with a 60-degree `+0x28`:
    /// a point just inside the top edge lands in row 0, one just inside the
    /// right edge (at the wider horizontal half-angle) in the last column,
    /// and one just past the top edge is off screen. With the FOV taken as
    /// horizontal, the first two would land a third of the way in.
    #[test]
    fn camera_fov_is_vertical_and_x_takes_the_aspect() {
        let mut g = world(&[]);
        let fov = 60f32.to_radians();
        g.set_u32(CAMERA + 0x28, fov.to_bits()).unwrap();
        for (k, v) in [0u16, 0, 360, 640].iter().enumerate() {
            g.write(CAMERA + 0x30 + 2 * k as u32, &v.to_le_bytes())
                .unwrap();
        }
        let c = read_camera(&g).unwrap().unwrap();
        let t = (fov / 2.0).tan();
        assert!((c.tan_half_y - t).abs() < 1e-6);
        assert!((c.tan_half_x - t * 16.0 / 9.0).abs() < 1e-5);
        // The camera looks down +x with z up (`world`); left is +y.
        let d = 10.0;
        let top = c.project([d, 0.0, d * t * 0.98], 0.01, 64, 32).unwrap();
        assert_eq!(top.y0, 0, "{top:?}");
        let right = c
            .project([d, -d * t * 16.0 / 9.0 * 0.98, 0.0], 0.01, 64, 32)
            .unwrap();
        assert_eq!(right.x1, 63, "{right:?}");
        assert!(c.project([d, 0.0, d * t * 1.1], 0.01, 64, 32).is_none());
        // Just past the vertical edge but inside the horizontal one, were
        // the FOV horizontal: still off screen.
        assert!(c.project([d, 0.0, d * t * 1.2], 0.01, 64, 32).is_none());
    }

    #[test]
    fn protected_covers_the_player_its_vehicle_and_what_it_holds() {
        let mut g = world(&[[20.0, 0.0, 0.0]; 5]);
        // The player rides object 2.
        g.set_u32(BODIES + BODY_PARENT, handle(2)).unwrap();
        let p = protected(&g).unwrap();
        assert_eq!(p, vec![handle(0), handle(2)]);
        assert!(exempt(&g, handle(3), &p).unwrap(), "held by the player");
        assert!(
            exempt(&g, handle(1), &p).unwrap(),
            "rides the player's vehicle"
        );
        assert!(!exempt(&g, handle(4), &p).unwrap());
    }

    /// The hooks' sequence over three frames: begin the records, decide
    /// per listed object, skip. The occluded object is skipped from the
    /// second frame with depth; the player, its weapon, an object in
    /// front of the wall and every light-view call are never touched.
    #[test]
    fn hooks_skip_occluded_objects_only_in_the_main_view() {
        // 0 player (behind the wall too), 1 rides 2, 2 behind, 3 held by
        // the player, 4 in front of the wall.
        let mut g = world(&[
            [20.0, 0.0, 0.0],
            [20.0, 2.0, 0.0],
            [20.0, 2.0, 0.0],
            [20.0, -2.0, 0.0],
            [3.0, 0.0, 0.0],
        ]);
        let mail = Mailbox::new();
        let mut o = Occlusion::new(Rules::default());
        let mut skipped = Vec::new();
        for frame in 1..=3u32 {
            g.set_u32(FRAME, frame).unwrap();
            if frame > 1 {
                mail.publish(0, wall(frame as u64 - 1));
            }
            o.begin_records(&g, MAIN_VIS, &mail).unwrap();
            let mut this = Vec::new();
            for i in 0..5u16 {
                let mut r = call_regs(&mut g, i);
                if o.decide(&g, &r).unwrap() {
                    skip_setup(&mut r, &mut g).unwrap();
                    assert_eq!(r.eax & 0xff, 0);
                    assert_eq!(r.edx, FLAGS);
                    assert_eq!(r.ecx, FLAGS + 2 * i as u32);
                    assert_eq!(r.eax & !0xff, 0x1234_5600);
                    this.push(i);
                }
            }
            skipped.push(this);
            // A light view's build in between: nothing decided there.
            o.begin_records(&g, 0x0054_aa68, &mail).unwrap();
            let mut r = call_regs(&mut g, 2);
            r.esi = 0x0054_aa68;
            assert!(!o.decide(&g, &r).unwrap());
        }
        // Object 1 rides object 2, which is not the player's: culled too.
        assert_eq!(skipped, vec![vec![], vec![], vec![1, 2]]);
        assert_eq!(g.u8(FLAGS + 2).unwrap(), FLAG_NOT_DRAWN);
        assert_eq!(g.u8(FLAGS + 4).unwrap(), FLAG_NOT_DRAWN);
        assert_eq!(g.u8(FLAGS).unwrap(), 0);
        let c = o.counts();
        assert_eq!(c.tested, 15);
        assert_eq!(c.culled, 2);
        assert_eq!(c.exempt, 6);
    }

    #[test]
    fn a_main_struct_in_another_kind_decides_nothing() {
        let mut g = world(&[[20.0, 0.0, 0.0], [20.0, 2.0, 0.0]]);
        g.set_u32(MAIN_VIS + VIS_KIND, 1).unwrap();
        let mail = Mailbox::new();
        let mut o = Occlusion::new(Rules::default());
        for frame in 1..=4u32 {
            g.set_u32(FRAME, frame).unwrap();
            mail.publish(0, wall(frame as u64 - 1));
            o.begin_records(&g, MAIN_VIS, &mail).unwrap();
            let r = call_regs(&mut g, 1);
            assert!(!o.decide(&g, &r).unwrap());
        }
        assert_eq!(o.counts().tested, 0);
    }
}
