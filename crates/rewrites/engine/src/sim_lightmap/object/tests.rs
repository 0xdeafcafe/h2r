//! The object sample's ports on a hand-built world: one object, a lit
//! structure with one cluster triangle, a sky, and entry J scripted.
use super::*;
use crate::sim_lightmap::Store;
use std::cell::RefCell;
use std::rc::Rc;

/// Guest memory: 6 MiB from 0, so the globals sit where the guest has them.
struct Flat(Vec<u8>);

impl Memory for Flat {
    fn read_bytes(&self, addr: u32, dst: &mut [u8], _: u32) -> Result<(), GuestFault> {
        let o = addr as usize;
        match self.0.get(o..o + dst.len()) {
            Some(b) => {
                dst.copy_from_slice(b);
                Ok(())
            }
            None => Err(GuestFault {
                addr,
                len: dst.len(),
                access: runtime::guest::Access::Read,
                reason: runtime::guest::FaultReason::Unmapped,
            }),
        }
    }

    fn write_bytes(&mut self, addr: u32, src: &[u8], _: u32) -> Result<(), GuestFault> {
        let o = addr as usize;
        self.0[o..o + src.len()].copy_from_slice(src);
        Ok(())
    }
}

impl Flat {
    fn f(&mut self, at: u32, v: f32) {
        self.set_u32(at, v.to_bits()).unwrap();
    }
    fn w(&mut self, at: u32, v: u32) {
        self.set_u32(at, v).unwrap();
    }
    fn h(&mut self, at: u32, v: u16) {
        self.set_u16(at, v).unwrap();
    }
    fn vec(&mut self, at: u32, v: [f32; 3]) {
        for (i, c) in v.into_iter().enumerate() {
            self.f(at + 4 * i as u32, c);
        }
    }
    fn fv(&self, at: u32) -> f32 {
        f32::from_bits(self.u32(at).unwrap())
    }
}

const FPU: Fpu = Fpu {
    mxcsr: 0x1f80,
    x87_control: 0x027f,
};
const OBJECT: u32 = 0x0001_0002;
const HEADERS: u32 = 0x10_1000;
const BODY: u32 = 0x11_0000;
const TAG_ARRAY: u32 = 0x12_0000;
const DEFINITION: u32 = 0x13_0000;
const LM: u32 = 0x14_0000;
const GROUPS: u32 = 0x14_1000;
const STRUCTURE: u32 = 0x15_0000;
const RUNS: u32 = 0x15_1000;
const CLUSTERS: u32 = 0x15_2000;
const SECTION: u32 = 0x15_3000;
const TRIANGLES: u32 = 0x15_4000;
const SURFACE_LIST: u32 = 0x15_5000;
const SURFACES: u32 = 0x15_6000;
const PARTS: u32 = 0x15_7000;
const INDICES: u32 = 0x15_8000;
const VERTEX_BLOCK: u32 = 0x15_9000;
const VERTICES: u32 = 0x15_a000;
const SCENARIO_DATA: u32 = 0x16_0000;
const SKIES: u32 = 0x16_1000;
const SKY_TAG: u32 = 0x17_0000;
const SKY_LIGHTS: u32 = 0x17_1000;
const POWERS: u32 = 0x17_2000;
const OUT: u32 = 0x18_0000;
const ENTRY: u32 = 0x58_0000;
const SURFACE: u32 = 7;

/// Tag `index` at `addr`.
fn tag(m: &mut Flat, index: u32, addr: u32) {
    m.w(TAG_ARRAY + index * 16 + 8, addr);
}

fn world() -> Flat {
    let mut m = Flat(vec![0; 0x60_0000]);
    m.w(OBJECTS, 0x10_0000);
    m.w(0x10_0000 + 0x44, HEADERS);
    m.w(HEADERS + 2 * 12 + 8, BODY);
    m.w(BODY, 3);
    m.vec(BODY + 0x30, [0.25, 0.25, 1.0]);
    m.set_u8(BODY + 0x3dc, 2).unwrap();
    m.w(TAGS, TAG_ARRAY);
    tag(&mut m, 3, DEFINITION);
    // A biped (kind 0).
    m.h(DEFINITION, 0);
    // The lit structure.
    m.w(LIGHTMAP, LM);
    m.w(LM + 0x80, 1);
    m.w(LM + 0x84, GROUPS);
    m.w(GROUPS + 0x1c, 5);
    m.w(GROUPS + 4, 0x1234);
    m.w(BSP, STRUCTURE);
    m.w(STRUCTURE + 8, 0x1234);
    // One cluster, its run of one triangle on surface 7, its section
    // in the tag (resident).
    m.w(STRUCTURE + 0x30, RUNS);
    m.h(RUNS, 0);
    m.h(RUNS + 2, 1);
    m.w(RUNS + 4, 0);
    m.w(STRUCTURE + 0xa0, CLUSTERS);
    let info = CLUSTERS + 0x28;
    m.w(info, NONE);
    m.h(info + 0x1c, 0x24);
    m.set_u8(info + 0x1e, 1).unwrap();
    m.w(info + 0x28, SECTION);
    m.w(STRUCTURE + 0x50, TRIANGLES);
    m.h(TRIANGLES, 0);
    m.h(TRIANGLES + 2, 9);
    m.w(TRIANGLES + 4, 0);
    m.w(STRUCTURE + 0x18, SURFACE_LIST);
    m.w(SURFACE_LIST + 4, SURFACES);
    m.h(SURFACES, SURFACE as u16);
    m.w(SECTION, 1);
    m.w(SECTION + 4, PARTS);
    m.h(PARTS + 6, 0);
    m.h(PARTS + 8, 3);
    m.w(SECTION + 0x24, INDICES);
    for (i, v) in [0u16, 1, 2].into_iter().enumerate() {
        m.h(INDICES + 2 * i as u32, v);
    }
    m.w(SECTION + 0x3c, VERTEX_BLOCK);
    m.h(VERTEX_BLOCK + 2, 3);
    m.w(VERTEX_BLOCK + 8, VERTICES);
    m.vec(VERTICES, [0.0, 0.0, 0.0]);
    m.vec(VERTICES + 12, [1.0, 0.0, 0.0]);
    m.vec(VERTICES + 24, [0.0, 1.0, 0.0]);
    // The constants.
    m.f(PROBE, 1.0);
    m.vec(ONE_DIRECTION, [0.0, 0.0, -1.0]);
    m.f(0x0045_dbc0, 1.0);
    m.f(0x0045_dbdc, 0.0001);
    m.f(0x0045_dbd8, 0.0);
    m.f(0x0045_ddfc, 0.0);
    m.f(0x0045_df0c, 0.5);
    m.f(0x0045_dc38, 255.0);
    // The axes for a normal along +z: x and y.
    m.h(0x0044_0b94 + 6 * 5, 0);
    m.h(0x0044_0b94 + 6 * 5 + 2, 1);
    // The sky: none lit until a test gives it lights.
    m.set_u8(0x004b_9ee9, 1).unwrap();
    m.w(0x004b_9eec, 0);
    m.w(0x004e_0350, SCENARIO_DATA);
    m.w(SCENARIO_DATA + 8, 1);
    m.w(SCENARIO_DATA + 0xc, SKIES);
    m.w(SKIES + 4, 4);
    tag(&mut m, 4, SKY_TAG);
    m.vec(SKY_TAG + 0x34, [0.5, 0.25, 1.0]);
    m.f(0x0045_dbcc, -1.0);
    m.w(0x0046_87a4, 0x0046_9000);
    m.w(0x0046_8718, 0x0046_9100);
    m.f(0x0045_dc0c, 1.0);
    m.f(0x0044_5420, 10.0);
    m
}

/// Entry J's answers in turn (`None` a miss, else words to write into
/// the record), and the arguments of every call.
#[derive(Default)]
struct Script {
    answers: Vec<Option<Vec<(u32, u32)>>>,
    calls: Vec<[u32; 6]>,
}

fn scripted(s: &Rc<RefCell<Script>>) -> impl FnMut(&mut Flat, u32) -> Result<bool, GuestFault> + use<> {
    let s = Rc::clone(s);
    move |g: &mut Flat, esp: u32| {
        let args: [u32; 6] = std::array::from_fn(|i| g.u32(esp + 4 + 4 * i as u32).unwrap());
        let mut s = s.borrow_mut();
        s.calls.push(args);
        let answer = if s.answers.is_empty() { None } else { s.answers.remove(0) };
        match answer {
            None => Ok(false),
            Some(words) => {
                for (o, v) in words {
                    g.set_u32(args[5] + o, v)?;
                }
                Ok(true)
            }
        }
    }
}

/// A J record of the cluster triangle at (x, y, 0).
fn cluster_hit(x: f32, y: f32) -> Vec<(u32, u32)> {
    vec![
        (0, 1),
        (8, x.to_bits()),
        (0xc, y.to_bits()),
        (0x10, 0),
        (0x3c, NONE),
        (0x40, NONE),
        (0x4c, 0),
        (0x54, SURFACE),
        (0x58, 0),
    ]
}

#[test]
fn an_unlit_structure_samples_nothing_and_succeeds() {
    let mut m = world();
    m.w(LM + 0x80, 0);
    let s = Rc::new(RefCell::new(Script::default()));
    let store = Store::empty();
    let mut r = Run::new(&mut m, &store, FPU, scripted(&s));
    assert!(sample(&mut r, ENTRY, OBJECT, [1, OUT]).unwrap());
    assert!(s.borrow().calls.is_empty());
}

#[test]
fn a_miss_gives_a_biped_the_skys_colour_without_lights() {
    let mut m = world();
    let s = Rc::new(RefCell::new(Script::default()));
    let store = Store::empty();
    let mut r = Run::new(&mut m, &store, FPU, scripted(&s));
    assert!(sample(&mut r, ENTRY, OBJECT, [1, OUT]).unwrap());
    // One direction: J from the centre less the probe along it.
    let calls = s.borrow().calls.clone();
    assert_eq!(calls.len(), 1);
    let [flags, start, delta, ignore, other, _] = calls[0];
    assert_eq!((flags, delta, ignore, other), (0x4480_000d, ONE_DIRECTION, OBJECT, NONE));
    assert_eq!([m.fv(start), m.fv(start + 4), m.fv(start + 8)], [0.25, 0.25, 2.0]);
    let axis = -(1.0f32 / (3.0f64.sqrt() as f32));
    assert_eq!([m.fv(OUT), m.fv(OUT + 4), m.fv(OUT + 8)], [axis; 3]);
    assert_eq!(m.u32(OUT + 0xc).unwrap(), 0xff55_5555);
    // 127.5 rounds to even.
    assert_eq!(m.u32(OUT + 0x10).unwrap(), 0x0080_40ff);
}

#[test]
fn a_kind_the_sky_does_not_light_fails_on_a_miss() {
    let mut m = world();
    m.h(DEFINITION, 5);
    let s = Rc::new(RefCell::new(Script::default()));
    let store = Store::empty();
    let mut r = Run::new(&mut m, &store, FPU, scripted(&s));
    assert!(!sample(&mut r, ENTRY, OBJECT, [1, OUT]).unwrap());
    assert_eq!(m.u32(OUT + 0x10).unwrap(), 0);
}

#[test]
fn the_sky_blends_its_lights_when_its_ray_reaches_it() {
    for (open, colour) in [(true, 0x0080_0080), (false, 0x0080_40ff)] {
        let mut m = world();
        m.w(SKY_TAG + 0x78, 2);
        m.w(SKY_TAG + 0x7c, SKY_LIGHTS);
        for (i, c) in [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0]].into_iter().enumerate() {
            let light = SKY_LIGHTS + 0x34 * i as u32;
            let power = POWERS + 0x20 * i as u32;
            m.w(light + 0x2c, 1);
            m.w(light + 0x30, power);
            m.vec(power + 4, c);
            m.f(power + 0x10, 1.0);
        }
        let s = Rc::new(RefCell::new(Script::default()));
        let mut script = Script::default();
        // The direction's ray misses; the sky's ray reaches the sky.
        script.answers = vec![None, open.then(|| vec![(0x58, 0xffff_0000)])];
        *s.borrow_mut() = script;
        let store = Store::empty();
        let mut r = Run::new(&mut m, &store, FPU, scripted(&s));
        assert!(sample(&mut r, ENTRY, OBJECT, [1, OUT]).unwrap());
        let calls = s.borrow().calls.clone();
        assert_eq!(calls.len(), 2);
        let [flags, start, delta, ignore, other, _] = calls[1];
        assert_eq!((flags, start, ignore, other), (0x1000_0001, BODY + 0x30, OBJECT, NONE));
        assert_eq!([m.fv(delta), m.fv(delta + 4), m.fv(delta + 8)], [10.0, 0.0, 0.0]);
        assert_eq!([m.fv(OUT), m.fv(OUT + 4), m.fv(OUT + 8)], [1.0, 0.0, 0.0]);
        // Half way to the second light: weight 1 over 1, over 1 + 1.
        assert_eq!(m.u32(OUT + 0x10).unwrap(), colour, "open {open}");
    }
}

#[test]
fn a_structure_hit_resolves_its_triangle_and_the_sampler_answers() {
    let mut m = world();
    // The mesh is unlit (type 4): the sampler stores its defaults and 0.
    m.h(PARTS, 4);
    let s = Rc::new(RefCell::new(Script::default()));
    s.borrow_mut().answers = vec![Some(cluster_hit(0.25, 0.5))];
    let store = Store::empty();
    let mut r = Run::new(&mut m, &store, FPU, scripted(&s));
    assert!(sample(&mut r, ENTRY, OBJECT, [1, OUT]).unwrap());
    let hit = ENTRY - 0x2c - 0x38;
    assert_eq!(m.u32(hit).unwrap(), 0);
    assert_eq!(m.u32(hit + 4).unwrap(), NONE);
    assert_eq!(m.u32(hit + 8).unwrap(), NONE);
    assert_eq!(m.u32(hit + 0x18).unwrap(), 0);
    assert_eq!(m.u32(hit + 0x20).unwrap(), 0);
    assert_eq!(m.u32(hit + 0x24).unwrap(), 0);
    assert_eq!(m.u32(hit + 0x28).unwrap(), 9);
    // The weights: towards v1 (x) then towards v2 (y).
    assert_eq!((m.fv(hit + 0x30), m.fv(hit + 0x34)), (0.25, 0.5));
    assert_eq!(m.u32(OUT + 0xc).unwrap(), 0xff40_4040);
    assert_eq!(m.fv(OUT + 8), -1.0);
    assert_eq!(s.borrow().calls.len(), 1);
}

#[test]
fn a_hit_off_the_triangle_restarts_while_the_record_asks() {
    let mut m = world();
    let mut hit = cluster_hit(2.0, 2.0);
    hit.push((0x58, 1));
    let s = Rc::new(RefCell::new(Script::default()));
    s.borrow_mut().answers = vec![Some(hit.clone()), Some(hit.clone()), Some(hit)];
    let store = Store::empty();
    let mut r = Run::new(&mut m, &store, FPU, scripted(&s));
    let (point, start, out) = (0x20_0000, 0x20_0100, 0x20_0200);
    let entry = ENTRY;
    r.set_vec3(point, [1.0f32.to_bits(), 2.0f32.to_bits(), 3.0f32.to_bits()]).unwrap();
    assert!(!ray(&mut r, entry, 0, point, start, [ONE_DIRECTION, OBJECT, 1, out]).unwrap());
    let calls = s.borrow().calls.clone();
    assert_eq!(calls.len(), 3);
    assert!(calls.iter().all(|c| c[0] == 0x0480_0c0d && c[1] == start));
    // Two steps of half the delta.
    assert_eq!([m.fv(start), m.fv(start + 4), m.fv(start + 8)], [1.0, 2.0, 2.0]);
    assert_eq!(m.u32(out).unwrap(), NONE);
    assert_eq!(m.u32(out + 0x18).unwrap(), 0);
}

/// Object 5: a scenery body whose model has a render model.
fn other_object(m: &mut Flat, lit: bool) -> u32 {
    let other = 0x0002_0005;
    let body = 0x11_1000;
    m.w(HEADERS + 5 * 12 + 8, body);
    m.w(body, 6);
    tag(m, 6, 0x13_1000);
    m.w(0x13_1000 + 0x38, 7);
    tag(m, 7, 0x13_2000);
    m.w(0x13_2000 + 4, 8);
    m.w(0x13_2000 + 0xc, 9);
    tag(m, 8, 0x13_3000);
    if lit {
        m.w(0x13_3000 + 0x74, 1);
        m.w(0x13_3000 + 0x7c, 1);
    }
    other
}

#[test]
fn an_object_hit_asks_again_for_the_structure_alone() {
    let mut m = world();
    let other = other_object(&mut m, false);
    let s = Rc::new(RefCell::new(Script::default()));
    s.borrow_mut().answers = vec![Some(vec![(0x40, other), (0x58, 0)])];
    let store = Store::empty();
    let mut r = Run::new(&mut m, &store, FPU, scripted(&s));
    let (point, start, out) = (0x20_0000, 0x20_0100, 0x20_0200);
    assert!(!ray(&mut r, ENTRY, 1, point, start, [ONE_DIRECTION, OBJECT, 1, out]).unwrap());
    let calls = s.borrow().calls.clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1][0], 0x0480_0005);
    assert_eq!((calls[1][3], calls[1][4]), (NONE, NONE));
    assert_eq!((m.u32(out + 0xc).unwrap(), m.u32(out + 0x10).unwrap()), (8, 9));
}

#[test]
#[should_panic(expected = "lightmap: a hit on object")]
fn an_object_hit_on_a_lit_render_model_is_not_ported() {
    let mut m = world();
    let other = other_object(&mut m, true);
    let s = Rc::new(RefCell::new(Script::default()));
    s.borrow_mut().answers = vec![Some(vec![(0x40, other)])];
    let store = Store::empty();
    let mut r = Run::new(&mut m, &store, FPU, scripted(&s));
    let _ = ray(&mut r, ENTRY, 1, 0x20_0000, 0x20_0100, [ONE_DIRECTION, OBJECT, 1, 0x20_0200]);
}

#[test]
fn the_triangle_test_takes_points_on_it_and_refuses_the_rest() {
    let mut m = world();
    let store = Store::empty();
    let s = Rc::new(RefCell::new(Script::default()));
    let mut r = Run::new(&mut m, &store, FPU, scripted(&s));
    let v = |x: f32, y: f32, z: f32| [x.to_bits(), y.to_bits(), z.to_bits()];
    let tri = [v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0)];
    let w = ray::inside(&mut r, v(0.25, 0.125, 0.0), tri).unwrap().unwrap();
    assert_eq!((f32::from_bits(w.0), f32::from_bits(w.1)), (0.25, 0.125));
    assert_eq!(ray::inside(&mut r, v(0.75, 0.75, 0.0), tri).unwrap(), None);
    assert_eq!(ray::inside(&mut r, v(0.25, 0.25, 1.0), tri).unwrap(), None);
    assert_eq!(ray::largest(&mut r, v(-3.0, 2.0, 1.0)).unwrap(), 0);
    assert_eq!(ray::largest(&mut r, v(1.0, -3.0, 3.0)).unwrap(), 2);
    assert_eq!(ray::largest(&mut r, v(1.0, -3.0, 2.0)).unwrap(), 1);
}

#[test]
fn the_part_search_and_the_placement() {
    let mut m = world();
    m.w(SECTION, 2);
    m.h(PARTS + 0x48 + 6, 3);
    m.h(PARTS + 0x48 + 8, 6);
    // A placement: scale 2, axes x, y, z, origin (1, 0, 0).
    let placed = 0x20_0000;
    m.f(placed, 2.0);
    for (i, a) in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].into_iter().enumerate() {
        m.vec(placed + 4 + 12 * i as u32, a);
    }
    m.vec(placed + 0x28, [1.0, 0.0, 0.0]);
    let store = Store::empty();
    let s = Rc::new(RefCell::new(Script::default()));
    let mut r = Run::new(&mut m, &store, FPU, scripted(&s));
    let outs = [0x20_1000, 0x20_1004, 0x20_1008];
    r.set_u32(outs[0], 7).unwrap();
    ray::part(&mut r, SECTION, 4, outs).unwrap();
    assert_eq!([r.u32(outs[0]).unwrap(), r.u32(outs[1]).unwrap(), r.u32(outs[2]).unwrap()], [0, 1, 1]);
    ray::part(&mut r, SECTION, 9, outs).unwrap();
    assert_eq!(r.u32(outs[1]).unwrap(), NONE);
    let p = ray::untransform(&mut r, placed, [5.0f32.to_bits(), 2.0f32.to_bits(), 0]).unwrap();
    assert_eq!(p.map(f32::from_bits), [2.0, 1.0, 0.0]);
}

#[test]
fn a_biped_of_another_kind_or_a_model_without_lighting() {
    let mut m = world();
    let other = other_object(&mut m, false);
    let store = Store::empty();
    let s = Rc::new(RefCell::new(Script::default()));
    let mut r = Run::new(&mut m, &store, FPU, scripted(&s));
    assert_eq!(r.unit_kind(OBJECT).unwrap(), 2);
    assert!(!r.model_lighting(other, 0x20_0000, 0x20_0004).unwrap());
    assert_eq!(r.u32(0x20_0000).unwrap(), 8);
    // No model: nothing stored.
    r.set_u32(0x13_1000 + 0x38, NONE).unwrap();
    r.set_u32(0x20_0000, 0).unwrap();
    assert!(!r.model_lighting(other, 0x20_0000, 0x20_0004).unwrap());
    assert_eq!(r.u32(0x20_0000).unwrap(), 0);
}
